//! `BitBlt`-based capture of the composited desktop.
//!
//! One blit from the screen DC into a top-down 32-bit DIB section, then a
//! straight `memcpy` out of the DIB bits. No intermediate `GetDIBits` copy and
//! no per-row work, so a 4K grab costs roughly one memory copy of the region.
//!
//! Handles are owned by [`Gdi`] guards so every early return -- including a
//! panic -- releases them. GDI handle leaks are invisible until the process has
//! burned through its 10 000 handle quota, at which point every later capture
//! fails, so ownership is enforced structurally rather than by discipline.

use kova_screen_core::{Bitmap, Error, PixelFormat, Rect, Result};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SRCCOPY,
    SelectObject,
};

/// Owns the screen device context, released on drop.
struct ScreenDc(HDC);

impl ScreenDc {
    fn acquire() -> Result<Self> {
        // SAFETY: a null HWND requests the DC of the entire virtual screen.
        let hdc = unsafe { GetDC(None) };
        if hdc.is_invalid() {
            return Err(Error::Capture(
                "could not obtain the screen device context".into(),
            ));
        }
        Ok(Self(hdc))
    }
}

impl Drop for ScreenDc {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from GetDC(None) and is released exactly once.
        unsafe {
            ReleaseDC(Some(HWND::default()), self.0);
        }
    }
}

/// Owns a memory DC, deleted on drop.
struct MemoryDc(HDC);

impl MemoryDc {
    fn create(source: HDC) -> Result<Self> {
        // SAFETY: `source` is a live DC obtained from GetDC.
        let hdc = unsafe { CreateCompatibleDC(Some(source)) };
        if hdc.is_invalid() {
            return Err(Error::Capture(
                "could not create a memory device context".into(),
            ));
        }
        Ok(Self(hdc))
    }
}

impl Drop for MemoryDc {
    fn drop(&mut self) {
        // SAFETY: created by CreateCompatibleDC and deleted exactly once.
        unsafe {
            let _ = DeleteDC(self.0);
        }
    }
}

/// Owns a DIB section and the pointer to its pixels.
struct DibSection {
    bitmap: HBITMAP,
    bits: *mut u8,
    len: usize,
}

impl DibSection {
    /// Creates a top-down 32bpp BGRA DIB.
    ///
    /// The negative height is what makes it top-down. Without it Windows hands
    /// back a bottom-up bitmap and every capture is vertically mirrored.
    fn create(dc: HDC, width: u32, height: u32) -> Result<Self> {
        let len = Bitmap::checked_byte_len(width, height)?;

        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: `info` describes a valid 32bpp DIB and `bits` receives the
        // pixel pointer, which stays valid until the HBITMAP is deleted.
        let bitmap = unsafe {
            CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).map_err(|e| {
                Error::Capture(format!("could not allocate a {width}x{height} bitmap: {e}"))
            })?
        };

        if bits.is_null() {
            // SAFETY: `bitmap` is valid here and is not referenced afterwards.
            unsafe {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
            }
            return Err(Error::Capture(
                "the bitmap was created without pixel storage".into(),
            ));
        }

        Ok(Self {
            bitmap,
            bits: bits.cast::<u8>(),
            len,
        })
    }

    /// Copies the DIB pixels into an owned buffer.
    fn to_vec(&self) -> Vec<u8> {
        // SAFETY: `bits` points to `len` bytes owned by the DIB section, which
        // outlives this borrow, and the buffer is fully initialised by the blit.
        unsafe { std::slice::from_raw_parts(self.bits, self.len) }.to_vec()
    }
}

impl Drop for DibSection {
    fn drop(&mut self) {
        // SAFETY: created by CreateDIBSection and deleted exactly once. The
        // caller must have deselected it from any DC first.
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
        }
    }
}

/// Captures `rect` from the composited desktop.
///
/// `rect` is in virtual-desktop physical pixels and is clamped to the visible
/// area first, so a selection dragged into the gap between two mismatched
/// monitors yields the on-screen part instead of undefined pixels.
pub fn capture_rect(rect: Rect) -> Result<Bitmap> {
    let rect = crate::monitor::clamp_to_desktop(rect)?;
    if rect.is_empty() {
        return Err(Error::Capture("the selected region is empty".into()));
    }

    let screen = ScreenDc::acquire()?;
    let mem = MemoryDc::create(screen.0)?;
    let dib = DibSection::create(screen.0, rect.width, rect.height)?;

    // SAFETY: both handles are live; the previous object is restored below.
    let previous = unsafe { SelectObject(mem.0, HGDIOBJ(dib.bitmap.0)) };

    // CAPTUREBLT includes layered windows, which is what makes menus, tooltips
    // and the Start menu appear in the screenshot instead of showing through.
    // SAFETY: destination is the DIB selected into `mem`, source is the screen
    // DC, and the source rectangle was clamped to the virtual desktop above.
    let ok = unsafe {
        windows::Win32::Graphics::Gdi::BitBlt(
            mem.0,
            0,
            0,
            rect.width as i32,
            rect.height as i32,
            Some(screen.0),
            rect.x,
            rect.y,
            SRCCOPY | CAPTUREBLT,
        )
    };

    // Restore before evaluating the result so the DIB is never deleted while
    // still selected into a DC, which would leak it.
    if !previous.is_invalid() {
        // SAFETY: restoring the object the DC held before our SelectObject.
        unsafe {
            SelectObject(mem.0, previous);
        }
    }

    ok.map_err(|e| Error::Capture(format!("screen blit failed: {e}")))?;

    let mut data = dib.to_vec();
    // BitBlt does not define the alpha channel: a window with a layered alpha
    // of 0 would otherwise save as fully transparent. Force opacity.
    for px in data.as_chunks_mut::<4>().0 {
        px[3] = 0xFF;
    }

    Bitmap::from_raw(rect.width, rect.height, PixelFormat::Bgra8, data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor;

    #[test]
    fn captures_a_small_region_with_the_requested_extent() {
        let bmp = capture_rect(Rect::new(0, 0, 64, 32)).expect("capture 64x32");
        assert_eq!(bmp.width(), 64);
        assert_eq!(bmp.height(), 32);
        assert_eq!(bmp.data().len(), 64 * 32 * 4);
        assert_eq!(bmp.format(), PixelFormat::Bgra8);
    }

    #[test]
    fn captured_pixels_are_fully_opaque() {
        let bmp = capture_rect(Rect::new(0, 0, 32, 32)).unwrap();
        assert!(
            bmp.data().as_chunks::<4>().0.iter().all(|p| p[3] == 0xFF),
            "alpha must be forced opaque or the png saves blank"
        );
    }

    #[test]
    fn captures_a_full_monitor() {
        let m = monitor::primary().expect("a primary monitor");
        let bmp = capture_rect(m.bounds).expect("full monitor capture");
        assert_eq!(bmp.width(), m.bounds.width);
        assert_eq!(bmp.height(), m.bounds.height);
    }

    #[test]
    fn captures_the_whole_virtual_desktop() {
        let desktop = monitor::virtual_desktop_bounds().unwrap();
        let bmp = capture_rect(desktop).expect("virtual desktop capture");
        assert_eq!(bmp.size(), desktop.size());
    }

    #[test]
    fn a_region_running_off_screen_is_clamped_rather_than_failing() {
        let desktop = monitor::virtual_desktop_bounds().unwrap();
        let overhang = Rect::new(desktop.right() - 10, desktop.y, 500, 100);
        let bmp = capture_rect(overhang).expect("clamped capture");
        assert_eq!(bmp.width(), 10);
    }

    #[test]
    fn rejects_an_empty_region() {
        assert!(capture_rect(Rect::new(0, 0, 0, 0)).is_err());
        assert!(capture_rect(Rect::new(0, 0, 100, 0)).is_err());
    }

    #[test]
    fn rejects_a_fully_off_screen_region() {
        let desktop = monitor::virtual_desktop_bounds().unwrap();
        assert!(capture_rect(Rect::new(desktop.right() + 5000, 0, 100, 100)).is_err());
    }

    #[test]
    fn repeated_captures_do_not_leak_gdi_handles() {
        // Each capture allocates a screen DC, a memory DC and a DIB. If any is
        // leaked the process handle count climbs and later captures eventually
        // fail outright. 200 iterations is far more than the ~3 handle delta a
        // healthy run shows, but well under the 10k per-process quota.
        let before = gdi_handle_count();
        for _ in 0..200 {
            capture_rect(Rect::new(0, 0, 64, 64)).expect("capture during leak check");
        }
        let after = gdi_handle_count();
        assert!(
            after <= before + 16,
            "gdi handle count grew from {before} to {after} over 200 captures"
        );
    }

    fn gdi_handle_count() -> u32 {
        use windows::Win32::System::Threading::{
            GR_GDIOBJECTS, GetCurrentProcess, GetGuiResources,
        };
        // SAFETY: the pseudo-handle for the current process is always valid.
        unsafe { GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS) }
    }
}

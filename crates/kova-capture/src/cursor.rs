//! Optional cursor compositing.
//!
//! Windows does not include the pointer in a `BitBlt` or a WGC frame, so it is
//! drawn afterwards. The whole module is best-effort: the cursor is decoration,
//! and no failure here may cost the user their screenshot.

use kova_screen_core::{Bitmap, Error, PixelFormat, Point, Rect, Result};
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GetDC, HGDIOBJ, ReleaseDC, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, GetCursorInfo, GetCursorPos, GetIconInfo,
    HICON, ICONINFO,
};

/// Current pointer position in virtual-desktop physical pixels.
pub fn position() -> Result<Point> {
    let mut pt = POINT::default();
    // SAFETY: `pt` is a live stack local for the duration of the call.
    unsafe { GetCursorPos(&mut pt) }
        .map_err(|e| Error::Capture(format!("could not read the cursor position: {e}")))?;
    Ok(Point::new(pt.x, pt.y))
}

/// Composites the pointer onto `bitmap`.
///
/// `origin` is the region the bitmap represents, so the cursor lands in the
/// right place for a region capture as well as a full-screen one. A hidden
/// cursor, or one whose icon cannot be read, is a no-op rather than an error.
///
/// The cursor is rendered into its own small BGRA buffer and alpha-blended in
/// software. Drawing straight onto the captured DIB would be simpler, but the
/// bitmap has already left GDI by this point and re-uploading a 4K image just
/// to stamp a 32x32 pointer onto it is not worth the copy.
pub fn draw_into(bitmap: &mut Bitmap, origin: Rect) -> Result<()> {
    let mut info = CURSORINFO {
        cbSize: size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `cbSize` is initialised as the API requires.
    unsafe { GetCursorInfo(&mut info) }
        .map_err(|e| Error::Capture(format!("could not read cursor info: {e}")))?;

    if info.flags != CURSOR_SHOWING || info.hCursor.is_invalid() {
        return Ok(());
    }

    let mut icon = ICONINFO::default();
    // SAFETY: `hCursor` is valid and `icon` receives owned bitmap handles that
    // we delete below.
    unsafe { GetIconInfo(HICON(info.hCursor.0), &mut icon) }
        .map_err(|e| Error::Capture(format!("could not read the cursor icon: {e}")))?;

    // GetIconInfo hands us two bitmaps we now own; release them on every path.
    let _guard = IconInfoGuard(&icon);

    // The hotspot is the offset from the icon origin to the actual pointer tip.
    let dest_x = info.ptScreenPos.x - icon.xHotspot as i32 - origin.x;
    let dest_y = info.ptScreenPos.y - icon.yHotspot as i32 - origin.y;

    let sprite = render_cursor_sprite(info.hCursor)?;
    blend(bitmap, &sprite, dest_x, dest_y);
    Ok(())
}

struct IconInfoGuard<'a>(&'a ICONINFO);

impl Drop for IconInfoGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: both handles were produced by GetIconInfo and are freed once.
        unsafe {
            if !self.0.hbmMask.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(self.0.hbmMask.0));
            }
            if !self.0.hbmColor.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(self.0.hbmColor.0));
            }
        }
    }
}

/// Fixed sprite extent for the rendered cursor.
///
/// Large enough for the biggest standard pointer at 300% scaling; `DrawIconEx`
/// clips anything larger rather than overflowing the buffer.
const SPRITE: u32 = 128;

/// Draws the cursor into a transparent BGRA buffer via `DrawIconEx`.
fn render_cursor_sprite(
    hcursor: windows::Win32::UI::WindowsAndMessaging::HCURSOR,
) -> Result<Bitmap> {
    // SAFETY: null HWND yields the screen DC, released by the guard below.
    let screen = unsafe { GetDC(None) };
    if screen.is_invalid() {
        return Err(Error::Capture("no screen dc for cursor rendering".into()));
    }
    let _screen_guard = scopeguard(move || {
        // SAFETY: released exactly once, matching the GetDC above.
        unsafe {
            ReleaseDC(None, screen);
        }
    });

    // SAFETY: `screen` is a live DC.
    let dc = unsafe { CreateCompatibleDC(Some(screen)) };
    if dc.is_invalid() {
        return Err(Error::Capture("no memory dc for cursor rendering".into()));
    }
    let _dc_guard = scopeguard(move || {
        // SAFETY: created by CreateCompatibleDC, deleted once.
        unsafe {
            let _ = DeleteDC(dc);
        }
    });

    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: SPRITE as i32,
            // Negative height: top-down, matching our Bitmap row order.
            biHeight: -(SPRITE as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: `info` describes a valid 32bpp DIB; `bits` receives the pixels.
    let hbm = unsafe { CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) }
        .map_err(|e| Error::Capture(format!("cursor bitmap allocation failed: {e}")))?;
    if bits.is_null() {
        // SAFETY: `hbm` is valid and unused afterwards.
        unsafe {
            let _ = DeleteObject(HGDIOBJ(hbm.0));
        }
        return Err(Error::Capture("cursor bitmap has no storage".into()));
    }
    let _bm_guard = scopeguard(move || {
        // SAFETY: deselected below before deletion.
        unsafe {
            let _ = DeleteObject(HGDIOBJ(hbm.0));
        }
    });

    // SAFETY: both handles are live.
    let previous = unsafe { SelectObject(dc, HGDIOBJ(hbm.0)) };

    // The DIB starts zeroed, i.e. fully transparent, so only the cursor pixels
    // end up with non-zero alpha.
    // SAFETY: destination is the DIB selected into `dc`, and the icon handle
    // came from GetCursorInfo.
    let drawn = unsafe { DrawIconEx(dc, 0, 0, HICON(hcursor.0), 0, 0, 0, None, DI_NORMAL) };

    // SAFETY: restore before the guard deletes the bitmap.
    unsafe {
        if !previous.is_invalid() {
            SelectObject(dc, previous);
        }
    }

    drawn.map_err(|e| Error::Capture(format!("could not draw the cursor: {e}")))?;

    // SAFETY: `bits` addresses SPRITE*SPRITE*4 bytes owned by the live DIB.
    let data =
        unsafe { std::slice::from_raw_parts(bits.cast::<u8>(), (SPRITE * SPRITE * 4) as usize) }
            .to_vec();

    Bitmap::from_raw(SPRITE, SPRITE, PixelFormat::Bgra8, data)
}

/// Alpha-blends `sprite` onto `dest` at `(x, y)`, clipping at every edge.
///
/// Both buffers are BGRA. `DrawIconEx` produces straight (non-premultiplied)
/// alpha, so this is the standard `src * a + dst * (1 - a)` blend.
fn blend(dest: &mut Bitmap, sprite: &Bitmap, x: i32, y: i32) {
    let dw = dest.width() as i64;
    let dh = dest.height() as i64;
    let sw = sprite.width() as i64;
    let sh = sprite.height() as i64;
    let dest_stride = dest.stride();
    let src_stride = sprite.stride();
    let src = sprite.data().to_vec();
    let dst = dest.data_mut();

    for row in 0..sh {
        let dy = y as i64 + row;
        if dy < 0 || dy >= dh {
            continue;
        }
        for col in 0..sw {
            let dx = x as i64 + col;
            if dx < 0 || dx >= dw {
                continue;
            }
            let s = (row as usize) * src_stride + (col as usize) * 4;
            let alpha = src[s + 3] as u32;
            if alpha == 0 {
                continue;
            }
            let d = (dy as usize) * dest_stride + (dx as usize) * 4;
            if alpha == 255 {
                dst[d..d + 4].copy_from_slice(&src[s..s + 4]);
                continue;
            }
            let inv = 255 - alpha;
            for c in 0..3 {
                dst[d + c] = ((src[s + c] as u32 * alpha + dst[d + c] as u32 * inv) / 255) as u8;
            }
            dst[d + 3] = 0xFF;
        }
    }
}

/// Minimal drop guard, so this module needs no extra dependency.
fn scopeguard<F: FnMut()>(f: F) -> ScopeGuard<F> {
    ScopeGuard(f)
}

struct ScopeGuard<F: FnMut()>(F);

impl<F: FnMut()> Drop for ScopeGuard<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(w: u32, h: u32) -> Bitmap {
        Bitmap::from_raw(w, h, PixelFormat::Bgra8, vec![0u8; (w * h * 4) as usize]).unwrap()
    }

    fn sprite_of(w: u32, h: u32, px: [u8; 4]) -> Bitmap {
        let data = px
            .iter()
            .copied()
            .cycle()
            .take((w * h * 4) as usize)
            .collect();
        Bitmap::from_raw(w, h, PixelFormat::Bgra8, data).unwrap()
    }

    #[test]
    fn opaque_sprite_overwrites_the_destination() {
        let mut dest = canvas(4, 4);
        let sprite = sprite_of(2, 2, [10, 20, 30, 255]);
        blend(&mut dest, &sprite, 0, 0);
        assert_eq!(&dest.data()[..4], &[10, 20, 30, 255]);
        // Outside the sprite the canvas is untouched.
        let last = dest.data().len() - 4;
        assert_eq!(&dest.data()[last..], &[0, 0, 0, 0]);
    }

    #[test]
    fn transparent_sprite_leaves_the_destination_alone() {
        let mut dest = canvas(4, 4);
        let before = dest.data().to_vec();
        blend(&mut dest, &sprite_of(2, 2, [9, 9, 9, 0]), 0, 0);
        assert_eq!(dest.data(), &before[..]);
    }

    #[test]
    fn half_alpha_blends_towards_the_source() {
        let mut dest = Bitmap::from_raw(1, 1, PixelFormat::Bgra8, vec![0, 0, 0, 255]).unwrap();
        blend(&mut dest, &sprite_of(1, 1, [255, 255, 255, 128]), 0, 0);
        let px = dest.data();
        assert!(
            (100..=160).contains(&px[0]),
            "expected a mid grey, got {}",
            px[0]
        );
        assert_eq!(px[3], 255);
    }

    #[test]
    fn sprite_is_clipped_at_every_edge_without_panicking() {
        let sprite = sprite_of(8, 8, [1, 2, 3, 255]);
        for (x, y) in [(-4, -4), (-4, 0), (0, -4), (2, 2), (-100, -100), (100, 100)] {
            let mut dest = canvas(4, 4);
            blend(&mut dest, &sprite, x, y);
        }
    }

    #[test]
    fn sprite_entirely_outside_is_a_no_op() {
        let mut dest = canvas(4, 4);
        let before = dest.data().to_vec();
        blend(&mut dest, &sprite_of(2, 2, [7, 7, 7, 255]), 50, 50);
        assert_eq!(dest.data(), &before[..]);
    }

    #[test]
    fn cursor_position_is_inside_the_virtual_desktop() {
        crate::require_interactive_desktop!();
        let p = position().expect("a cursor position");
        let desktop = crate::monitor::virtual_desktop_bounds().unwrap();
        // The pointer can sit exactly on the far edge, hence the inclusive bound.
        assert!(p.x >= desktop.left() && p.x <= desktop.right());
        assert!(p.y >= desktop.top() && p.y <= desktop.bottom());
    }

    #[test]
    fn drawing_the_live_cursor_never_corrupts_the_bitmap() {
        crate::require_interactive_desktop!();
        let mut bmp = canvas(200, 200);
        // Best effort: on a headless agent the cursor may be hidden, which is a
        // successful no-op. Either way the buffer must stay the right size.
        let _ = draw_into(&mut bmp, Rect::new(0, 0, 200, 200));
        assert_eq!(bmp.data().len(), 200 * 200 * 4);
    }
}

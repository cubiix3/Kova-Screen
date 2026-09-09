//! Static recording outline. The hollow window neither blocks input nor
//! allocates a full-desktop bitmap, and is excluded from captured frames.

use kova_screen_core::{Error, Rect, Result};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::{HSTRING, PCWSTR};

use crate::win32::{self, ModuleHandle};

pub(super) struct BoundaryWindow(HWND);

impl BoundaryWindow {
    pub(super) fn create(rect: Rect) -> Result<Self> {
        let class = win32::class_name("KovaScreenRecordingBoundary");
        let module = ModuleHandle::current()?;
        let description = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: module.handle().into(),
            lpszClassName: win32::class_ptr(&class),
            ..Default::default()
        };
        win32::register_class(&description, "recording boundary")?;
        let width = rect.width as i32;
        let height = rect.height as i32;
        // SAFETY: registered class, valid module and no borrowed window state.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE
                    | WS_EX_LAYERED
                    | WS_EX_TRANSPARENT,
                PCWSTR(class.as_ptr()),
                &HSTRING::from("Kova Screen Recording Area"),
                WS_POPUP,
                rect.x,
                rect.y,
                width,
                height,
                None,
                None,
                Some(module.handle().into()),
                None,
            )
        }
        .map_err(|e| Error::Platform(format!("could not create recording boundary: {e}")))?;
        let window = Self(hwnd);
        // SAFETY: all regions are owned locally until SetWindowRgn succeeds.
        unsafe {
            let outer = CreateRectRgn(0, 0, width, height);
            let inner = CreateRectRgn(2, 2, (width - 2).max(2), (height - 2).max(2));
            if outer.is_invalid() || inner.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(outer.0));
                let _ = DeleteObject(HGDIOBJ(inner.0));
                return Err(Error::Platform(
                    "could not allocate recording boundary".into(),
                ));
            }
            let combined = CombineRgn(Some(outer), Some(outer), Some(inner), RGN_DIFF);
            let _ = DeleteObject(HGDIOBJ(inner.0));
            if combined == RGN_ERROR || SetWindowRgn(hwnd, Some(outer), false) == 0 {
                let _ = DeleteObject(HGDIOBJ(outer.0));
                return Err(Error::Platform("could not shape recording boundary".into()));
            }
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA).map_err(|e| {
                Error::Platform(format!("could not make boundary click-through: {e}"))
            })?;
            SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE).map_err(|e| {
                Error::Platform(format!("could not exclude recording boundary: {e}"))
            })?;
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        Ok(window)
    }
}

impl Drop for BoundaryWindow {
    fn drop(&mut self) {
        // SAFETY: created and destroyed on the recorder overlay thread.
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_PAINT => {
            // SAFETY: balanced paint and brush lifetime; the hollow window
            // region clips painting to just the two-pixel outline.
            unsafe {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                let brush = CreateSolidBrush(COLORREF(0x00F4_D586));
                FillRect(dc, &ps.rcPaint, brush);
                let _ = DeleteObject(HGDIOBJ(brush.0));
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn outline_is_hollow_excluded_and_released() {
        let window = BoundaryWindow::create(Rect::new(40, 40, 320, 240)).unwrap();
        let hwnd = window.0;
        unsafe {
            let region = CreateRectRgn(0, 0, 0, 0);
            assert_ne!(GetWindowRgn(hwnd, region), RGN_ERROR);
            assert!(PtInRegion(region, 0, 100).as_bool());
            assert!(!PtInRegion(region, 100, 100).as_bool());
            let _ = DeleteObject(HGDIOBJ(region.0));
            let mut affinity = 0;
            GetWindowDisplayAffinity(hwnd, &mut affinity).unwrap();
            assert_eq!(affinity, WDA_EXCLUDEFROMCAPTURE.0);
            assert_ne!(
                GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT.0,
                0
            );
        }
        drop(window);
        assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
    }
}

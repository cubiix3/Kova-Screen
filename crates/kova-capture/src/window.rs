//! Top-level window enumeration and geometry.

use kova_screen_core::{Error, Rect, Result};
use windows::Win32::Foundation::{HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible, WS_EX_TOOLWINDOW,
};
use windows_core::BOOL;

/// Identifies a top-level window.
///
/// Holds the raw `HWND` as an integer so it can cross the IPC boundary. Handles
/// go stale the moment a window closes, so every use re-validates with
/// `IsWindow` and reports a clear error rather than capturing whatever new
/// window inherited the handle value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct WindowId(pub isize);

impl WindowId {
    fn hwnd(self) -> HWND {
        HWND(self.0 as *mut core::ffi::c_void)
    }

    /// Whether the handle still refers to a live window.
    pub fn is_alive(self) -> bool {
        // SAFETY: IsWindow is explicitly documented as safe for stale handles.
        unsafe { IsWindow(Some(self.hwnd())).as_bool() }
    }
}

/// A window offered in the window picker.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WindowInfo {
    pub id: WindowId,
    pub title: String,
    /// Visible bounds, i.e. the DWM extended frame.
    pub bounds: Rect,
    pub process_id: u32,
}

/// Lists windows a user could plausibly want to capture.
///
/// Filters out anything invisible, minimised, cloaked, untitled, or marked as a
/// tool window. Cloaking is the important one: without that check the list is
/// full of the phantom UWP host windows Windows keeps around on other virtual
/// desktops, which capture as solid black.
///
/// Ordered front-to-back as far as `EnumWindows` reports z-order, so the
/// picker highlights the topmost window under the cursor first.
pub fn list() -> Vec<WindowInfo> {
    let mut windows: Vec<WindowInfo> = Vec::new();
    let ptr = &mut windows as *mut Vec<WindowInfo>;

    // SAFETY: `enum_proc` matches the expected signature and `ptr` outlives the
    // synchronous enumeration.
    let _ = unsafe { EnumWindows(Some(enum_proc), LPARAM(ptr as isize)) };

    windows
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` is the &mut Vec passed to EnumWindows, alive throughout.
    let windows = unsafe { &mut *(lparam.0 as *mut Vec<WindowInfo>) };

    if !is_capturable(hwnd) {
        return TRUE;
    }

    let Some(title) = title_of(hwnd) else {
        return TRUE;
    };
    if title.trim().is_empty() {
        return TRUE;
    }

    let Ok(bounds) = frame_bounds(hwnd) else {
        return TRUE;
    };
    if bounds.is_empty() {
        return TRUE;
    }

    let mut process_id = 0u32;
    // SAFETY: `hwnd` is valid here and the out-pointer is a live local.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };

    windows.push(WindowInfo {
        id: WindowId(hwnd.0 as isize),
        title,
        bounds,
        process_id,
    });

    TRUE
}

/// Whether a window is visible, restored and not a tool or cloaked window.
fn is_capturable(hwnd: HWND) -> bool {
    // SAFETY: all three take a window handle and tolerate stale ones.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return false;
        }
        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
            return false;
        }
    }
    !is_cloaked(hwnd)
}

/// Whether DWM is hiding the window (another virtual desktop, a suspended UWP
/// app). Cloaked windows are still "visible" to Win32 but capture as black.
fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked: u32 = 0;
    // SAFETY: the out-buffer is a live u32 and the size matches the attribute.
    let hr = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            std::ptr::from_mut(&mut cloaked).cast(),
            size_of::<u32>() as u32,
        )
    };
    hr.is_ok() && cloaked != 0
}

fn title_of(hwnd: HWND) -> Option<String> {
    // SAFETY: valid handle; returns 0 for a window with no title.
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return None;
    }
    // +1 for the terminating NUL GetWindowTextW always writes.
    let mut buf = vec![0u16; len as usize + 1];
    // SAFETY: `buf` has room for `len + 1` UTF-16 units including the NUL.
    let written = unsafe { GetWindowTextW(hwnd, &mut buf) };
    if written <= 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..written as usize]))
}

/// Visible bounds of a window in virtual-desktop physical pixels.
///
/// Prefers the DWM extended frame over `GetWindowRect`. Since Windows 10,
/// `GetWindowRect` includes an invisible resize border of up to 8px per side,
/// so capturing that rect yields a screenshot with a band of desktop around the
/// window. Falls back to `GetWindowRect` if DWM declines (it does for some
/// console and full-screen exclusive windows).
pub fn frame_bounds(hwnd: HWND) -> Result<Rect> {
    let mut rect = RECT::default();
    // SAFETY: the out-buffer is a live RECT and the size matches the attribute.
    let hr = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            std::ptr::from_mut(&mut rect).cast(),
            size_of::<RECT>() as u32,
        )
    };

    if hr.is_err() {
        // SAFETY: valid handle; fails only for a destroyed window.
        unsafe { GetWindowRect(hwnd, &mut rect) }
            .map_err(|e| Error::Capture(format!("could not read the window bounds: {e}")))?;
    }

    let width = (rect.right as i64 - rect.left as i64).max(0) as u32;
    let height = (rect.bottom as i64 - rect.top as i64).max(0) as u32;
    Ok(Rect::new(rect.left, rect.top, width, height))
}

/// Bounds of a window by id, validating the handle first.
pub fn bounds(id: WindowId) -> Result<Rect> {
    if !id.is_alive() {
        return Err(Error::Capture("that window has closed".into()));
    }
    frame_bounds(id.hwnd())
}

/// Looks up a window by id.
pub fn find(id: WindowId) -> Option<WindowInfo> {
    list().into_iter().find(|w| w.id == id)
}

/// The window currently in the foreground, if it is one we would capture.
///
/// Used as the default target for the window-screenshot hotkey.
pub fn foreground() -> Option<WindowInfo> {
    // SAFETY: returns a null handle when no window has focus.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return None;
    }
    find(WindowId(hwnd.0 as isize))
}

/// The topmost capturable window containing `point`.
///
/// Used by the window picker to resolve what is under the cursor. Walks the
/// z-ordered list from `list()` and takes the first hit, so an overlapping
/// window in front wins.
pub fn from_point(point: kova_screen_core::Point) -> Option<WindowInfo> {
    list().into_iter().find(|w| w.bounds.contains(point))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumeration_only_returns_titled_windows_with_real_extents() {
        for w in list() {
            assert!(
                !w.title.trim().is_empty(),
                "an untitled window leaked into the picker"
            );
            assert!(!w.bounds.is_empty(), "{} has an empty extent", w.title);
            assert!(w.process_id > 0, "{} has no owning process", w.title);
        }
    }

    #[test]
    fn enumerated_windows_are_alive_and_resolvable() {
        for w in list().into_iter().take(10) {
            assert!(w.id.is_alive());
            assert!(bounds(w.id).is_ok());
        }
    }

    #[test]
    fn a_stale_handle_is_reported_rather_than_captured() {
        // A handle value that cannot belong to a window.
        let bogus = WindowId(1);
        assert!(!bogus.is_alive());
        assert!(bounds(bogus).is_err());
        assert!(find(bogus).is_none());
    }

    #[test]
    fn ids_are_unique_across_the_enumeration() {
        let windows = list();
        let mut ids: Vec<isize> = windows.iter().map(|w| w.id.0).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total, "EnumWindows reported a window twice");
    }

    #[test]
    fn point_lookup_agrees_with_the_reported_bounds() {
        let Some(w) = list().into_iter().next() else {
            return; // No windows in this session.
        };
        let centre = kova_screen_core::Point::new(
            w.bounds.x + w.bounds.width as i32 / 2,
            w.bounds.y + w.bounds.height as i32 / 2,
        );
        // Some other window may sit on top, but a hit must be returned.
        assert!(from_point(centre).is_some());
    }
}

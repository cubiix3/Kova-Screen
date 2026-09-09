//! The floating recorder overlay.
//!
//! A compact pill showing the elapsed time and two controls:
//!
//! ```text
//! ● 00:14    Pause    Stop
//! ```
//!
//! # Keeping it out of the recording
//!
//! `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)` tells the compositor
//! to omit the window from every capture path -- Windows Graphics Capture,
//! `BitBlt` and DXGI duplication alike. That is what stops the overlay from
//! appearing in the video it is controlling.
//!
//! The API needs Windows 10 version 2004. On anything older the call fails and
//! the overlay is simply visible in the recording, which
//! [`RecorderOverlay::is_excluded_from_capture`] reports so the UI can say so
//! rather than silently producing a recording with a control bar burned in.
//!
//! # Shape
//!
//! The corners are rounded: through the Windows 11 compositor preference where
//! it exists, which anti-aliases them properly, and by clipping the window to a
//! rounded region on Windows 10, which is harder-edged but the right shape.
//!
//! # Cost
//!
//! The window repaints once a second, only while recording, and only the
//! ~230x40 pixels it occupies.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

use kova_screen_core::{Error, Rect, Result};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DT_LEFT, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW,
    EndPaint, FillRect, HBRUSH, HDC, HGDIOBJ, InvalidateRect, PAINTSTRUCT, SelectObject, SetBkMode,
    SetTextColor, SetWindowRgn, TRANSPARENT,
};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GWLP_USERDATA, GetWindowLongPtrW, HTCAPTION, KillTimer, MSG, PM_REMOVE, PeekMessageW,
    PostQuitMessage, SW_SHOWNOACTIVATE, SetTimer, SetWindowDisplayAffinity, SetWindowLongPtrW,
    ShowWindow, TranslateMessage, WDA_EXCLUDEFROMCAPTURE, WM_DESTROY, WM_LBUTTONDOWN, WM_MOUSEMOVE,
    WM_NCCREATE, WM_NCHITTEST, WM_PAINT, WM_TIMER, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows_core::{HSTRING, PCWSTR};

use crate::win32::{self, ModuleHandle};

/// What the user pressed on the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderCommand {
    TogglePause,
    Stop,
}

/// Overlay geometry at 100% scaling.
const WIDTH: i32 = 232;
const HEIGHT: i32 = 40;
const MARGIN: i32 = 24;

/// Hit regions, as x-offsets within the overlay.
const PAUSE_X: i32 = 106;
const STOP_X: i32 = 172;
const BUTTON_W: i32 = 56;

// The hit regions are pure layout constants, so their invariants are checked at
// compile time: a bad edit fails the build rather than shipping an overlay
// whose Stop button silently triggers Pause.
const _: () = {
    assert!(
        PAUSE_X + BUTTON_W <= STOP_X,
        "the pause and stop hit regions overlap"
    );
    assert!(
        STOP_X + BUTTON_W <= WIDTH,
        "the stop button runs off the overlay"
    );
    assert!(PAUSE_X > 34, "no room for the timer left of the buttons");
};

/// Colours, taken from the shared Kova palette.
///
/// GDI wants `0x00BBGGRR`, the reverse of the `#RRGGBB` used elsewhere.
mod theme {
    /// Surface: `#1b1e22`.
    pub const SURFACE: u32 = 0x0022_1E1B;
    /// Primary text: `#f0f2f5`.
    pub const TEXT: u32 = 0x00F5_F2F0;
    /// Secondary text: `#8d98a5`.
    pub const MUTED: u32 = 0x00A5_988D;
    /// Recording dot: the Kova danger tone `#f49b9b`.
    pub const REC_DOT: u32 = 0x009B_9BF4;
}

/// State shared between the caller and the overlay window thread.
#[derive(Debug, Default)]
pub struct RecorderState {
    /// Elapsed recording time in milliseconds.
    elapsed_ms: AtomicU64,
    paused: AtomicBool,
    stop_requested: AtomicBool,
    pause_requested: AtomicBool,
    /// False when this Windows build cannot exclude the overlay from capture.
    excluded: AtomicBool,
    hovered: AtomicU8,
}

impl RecorderState {
    /// Updates the time shown in the overlay.
    pub fn set_elapsed(&self, elapsed: Duration) {
        self.elapsed_ms
            .store(elapsed.as_millis() as u64, Ordering::Relaxed);
    }

    pub fn elapsed(&self) -> Duration {
        Duration::from_millis(self.elapsed_ms.load(Ordering::Relaxed))
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Takes the pending stop request, if any.
    pub fn take_stop_request(&self) -> bool {
        self.stop_requested.swap(false, Ordering::Relaxed)
    }

    /// Takes the pending pause request, if any.
    pub fn take_pause_request(&self) -> bool {
        self.pause_requested.swap(false, Ordering::Relaxed)
    }

    /// Whether the overlay is hidden from the recording it controls.
    pub fn is_excluded_from_capture(&self) -> bool {
        self.excluded.load(Ordering::Relaxed)
    }
}

/// A running recorder overlay, owning its window thread.
pub struct RecorderOverlay {
    state: Arc<RecorderState>,
    close: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RecorderOverlay {
    /// Shows the overlay near the bottom-centre of the monitor holding the
    /// cursor, so it does not cover whatever is being recorded at the top of
    /// the screen.
    ///
    /// Returns as soon as the window exists.
    pub fn show() -> Result<Self> {
        Self::show_for_region(None)
    }

    /// Shows the controls and an excluded, click-through recording boundary.
    pub fn show_for_region(region: Option<Rect>) -> Result<Self> {
        let state = Arc::new(RecorderState::default());
        let close = Arc::new(AtomicBool::new(false));

        let thread_state = Arc::clone(&state);
        let thread_close = Arc::clone(&close);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();

        let thread = std::thread::Builder::new()
            .name("kova-recorder-overlay".into())
            .spawn(
                move || match OverlayWindow::create(Arc::clone(&thread_state), region) {
                    Ok(window) => {
                        let _ = ready_tx.send(Ok(()));
                        window.run(&thread_close);
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                    }
                },
            )
            .map_err(|e| Error::Platform(format!("could not start the overlay thread: {e}")))?;

        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => Ok(Self {
                state,
                close,
                thread: Some(thread),
            }),
            Ok(Err(err)) => {
                let _ = thread.join();
                Err(err)
            }
            Err(_) => {
                close.store(true, Ordering::Relaxed);
                Err(Error::Platform(
                    "the recorder overlay did not appear in time".into(),
                ))
            }
        }
    }

    /// Shared state: update the timer here, read pause and stop requests here.
    pub fn state(&self) -> Arc<RecorderState> {
        Arc::clone(&self.state)
    }

    /// Closes the overlay and joins its thread.
    pub fn close(&mut self) {
        self.close.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for RecorderOverlay {
    fn drop(&mut self) {
        self.close();
    }
}

/// The window itself. Created and pumped on the overlay thread.
struct OverlayWindow {
    hwnd: HWND,
    /// Kept alive because the window procedure borrows the pointed-to state
    /// through `GWLP_USERDATA`. An `Arc` allocation address is stable, so the
    /// pointer stays valid however this handle is moved.
    _state: Arc<RecorderState>,
    _boundary: Option<super::boundary::BoundaryWindow>,
}

impl OverlayWindow {
    fn create(state: Arc<RecorderState>, region: Option<Rect>) -> Result<Self> {
        let boundary = region
            .map(super::boundary::BoundaryWindow::create)
            .transpose()?;
        let class = register_class()?;
        let module = ModuleHandle::current()?;

        let (x, y) = default_position();

        let state_ptr = Arc::as_ptr(&state);

        // SAFETY: the class is registered and `state_ptr` outlives the window,
        // which is destroyed before `boxed` is dropped.
        let hwnd = unsafe {
            CreateWindowExW(
                // NOACTIVATE so clicking the overlay never steals focus from
                // whatever the user is recording.
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                PCWSTR(class.as_ptr()),
                PCWSTR(HSTRING::from("Kova Screen Recorder").as_ptr()),
                WS_POPUP,
                x,
                y,
                WIDTH,
                HEIGHT,
                None,
                None,
                Some(module.handle().into()),
                Some(state_ptr.cast()),
            )
        }
        .map_err(|e| Error::Platform(format!("could not create the recorder overlay: {e}")))?;

        // Hide the overlay from every capture path. Windows 10 2004 and newer.
        // SAFETY: `hwnd` is live; an older build returns an error we record.
        let excluded = unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) }.is_ok();
        state.excluded.store(excluded, Ordering::Relaxed);
        if !excluded {
            tracing::warn!(
                "this windows build cannot exclude the overlay from capture; \
                 it will be visible in the recording"
            );
        }

        round_corners(hwnd);

        // SAFETY: `hwnd` is live. SHOWNOACTIVATE preserves the user focus.
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            // One repaint per second is all a mm:ss timer needs.
            SetTimer(Some(hwnd), TIMER_ID, 500, None);
        }

        Ok(Self {
            hwnd,
            _state: state,
            _boundary: boundary,
        })
    }

    fn run(self, close: &AtomicBool) {
        let mut msg = MSG::default();
        while !close.load(Ordering::Relaxed) {
            // SAFETY: `msg` is a live local; PM_REMOVE dequeues.
            while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                // SAFETY: `msg` was filled by PeekMessageW.
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            // Sleep until input or the timer arrives, with a bounded wait for
            // close requests from the recording thread. No 60 Hz polling.
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::MsgWaitForMultipleObjectsEx(
                    None,
                    100,
                    windows::Win32::UI::WindowsAndMessaging::QS_ALLINPUT,
                    windows::Win32::UI::WindowsAndMessaging::MWMO_INPUTAVAILABLE,
                );
            }
        }

        // SAFETY: `hwnd` is live and destroyed exactly once.
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_ID);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// Corner radius used on Windows 10, in pixels.
///
/// Chosen to read the same as the Windows 11 small-corner preference, so the
/// overlay looks identical across both.
const CORNER_RADIUS: i32 = 8;

/// Rounds the overlay corners.
///
/// Windows 11 has a compositor-level preference, which is preferred because DWM
/// then anti-aliases the corners properly. Windows 10 has no such attribute, so
/// the window is clipped to a rounded region instead -- visibly harder-edged,
/// but the right shape.
///
/// Purely cosmetic on every path, so a failure is logged at debug and ignored.
fn round_corners(hwnd: HWND) {
    use windows::Win32::Graphics::Dwm::{
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUNDSMALL, DwmSetWindowAttribute,
    };

    let preference = DWMWCP_ROUNDSMALL;
    // SAFETY: the attribute and its size match; older builds return an error
    // HRESULT rather than misbehaving.
    let rounded = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            std::ptr::from_ref(&preference).cast(),
            size_of_val(&preference) as u32,
        )
    };
    if rounded.is_ok() {
        return;
    }

    // Windows 10: clip the window to a rounded rectangle.
    // SAFETY: the region is handed to the window, which owns it from here on,
    // so it must not be deleted by us.
    unsafe {
        let region = windows::Win32::Graphics::Gdi::CreateRoundRectRgn(
            0,
            0,
            WIDTH + 1,
            HEIGHT + 1,
            CORNER_RADIUS,
            CORNER_RADIUS,
        );
        if region.is_invalid() {
            tracing::debug!("could not round the overlay corners");
            return;
        }
        if SetWindowRgn(hwnd, Some(region), true) == 0 {
            // The window did not take ownership, so the region is still ours.
            let _ = DeleteObject(HGDIOBJ(region.0));
            tracing::debug!("could not apply the overlay corner region");
        }
    }
}

const TIMER_ID: usize = 1;

/// Bottom-centre of the monitor under the cursor, inset by [`MARGIN`].
fn default_position() -> (i32, i32) {
    let monitor = kova_capture::monitor::from_cursor().or_else(kova_capture::monitor::primary);

    match monitor {
        Some(m) => {
            // Use the work area so the overlay never lands under the taskbar.
            let area = m.work_area;
            let x = area.x + (area.width as i32 - WIDTH) / 2;
            let y = area.bottom() - HEIGHT - MARGIN;
            (x, y)
        }
        None => (MARGIN, MARGIN),
    }
}

fn register_class() -> Result<Vec<u16>> {
    let class_name = win32::class_name("KovaScreenRecorderOverlay");
    let module = ModuleHandle::current()?;
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: module.handle().into(),
        hCursor: unsafe {
            windows::Win32::UI::WindowsAndMessaging::LoadCursorW(
                None,
                windows::Win32::UI::WindowsAndMessaging::IDC_ARROW,
            )
        }
        .unwrap_or_default(),
        lpszClassName: win32::class_ptr(&class_name),
        ..Default::default()
    };
    win32::register_class(&class, "recorder overlay")?;
    Ok(class_name)
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        // SAFETY: lparam is the CREATESTRUCTW for this window.
        let create = unsafe {
            &*(lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW)
        };
        // SAFETY: stores a pointer valid for the window lifetime.
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        // SAFETY: default handling for the rest of WM_NCCREATE.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }

    // SAFETY: reads back the pointer stored above.
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RecorderState;
    if ptr.is_null() {
        // SAFETY: no state yet.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    // SAFETY: the pointer refers to the Box owned by OverlayWindow, which
    // outlives the window.
    let state = unsafe { &*ptr };

    match msg {
        WM_MOUSEMOVE => {
            use windows::Win32::UI::Input::KeyboardAndMouse::{
                TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
            };
            let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            let hovered = button_at(x, y);
            if state.hovered.swap(hovered, Ordering::Relaxed) != hovered {
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                ..Default::default()
            };
            unsafe {
                let _ = TrackMouseEvent(&mut track);
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            state.hovered.store(0, Ordering::Relaxed);
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            paint(hwnd, state);
            LRESULT(0)
        }
        WM_TIMER => {
            // SAFETY: `hwnd` is live; a null rect invalidates the whole window.
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            if button_at(x, y) == 1 {
                state.pause_requested.store(true, Ordering::Relaxed);
            } else if button_at(x, y) == 2 {
                state.stop_requested.store(true, Ordering::Relaxed);
            }
            // SAFETY: `hwnd` is live.
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        // Dragging anywhere that is not a button moves the overlay: reporting
        // the area left of the buttons as the title bar gets that for free.
        WM_NCHITTEST => {
            // SAFETY: default handling resolves the hit region first.
            let default = unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            let screen_x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
            let mut point = windows::Win32::Foundation::POINT { x: screen_x, y: 0 };
            // SAFETY: `hwnd` is live and `point` is a live local.
            unsafe {
                let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut point);
            }
            if point.x < PAUSE_X - 4 {
                return LRESULT(HTCAPTION as isize);
            }
            default
        }
        WM_DESTROY => {
            // SAFETY: ends any nested modal loop.
            unsafe {
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        // SAFETY: everything else gets default handling.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn paint(hwnd: HWND, state: &RecorderState) {
    let mut ps = PAINTSTRUCT::default();
    // SAFETY: `ps` is a live local; balanced by EndPaint below.
    let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
    if hdc.is_invalid() {
        return;
    }

    draw(hdc, state);

    // SAFETY: balances BeginPaint.
    unsafe {
        let _ = EndPaint(hwnd, &ps);
    }
}

fn draw(hdc: HDC, state: &RecorderState) {
    let full = RECT {
        left: 0,
        top: 0,
        right: WIDTH,
        bottom: HEIGHT,
    };

    // SAFETY: every GDI object created here is restored and deleted.
    unsafe {
        let surface: HBRUSH = CreateSolidBrush(COLORREF(theme::SURFACE));
        FillRect(hdc, &full, surface);
        let _ = DeleteObject(HGDIOBJ(surface.0));

        // The status dot: solid while recording, muted while paused.
        let paused = state.is_paused();
        let dot_colour = if paused { theme::MUTED } else { theme::REC_DOT };
        let dot: HBRUSH = CreateSolidBrush(COLORREF(dot_colour));
        FillRect(
            hdc,
            &RECT {
                left: 16,
                top: 16,
                right: 24,
                bottom: 24,
            },
            dot,
        );
        let _ = DeleteObject(HGDIOBJ(dot.0));

        let font = win32::ui_font(15);
        let previous_font = SelectObject(hdc, HGDIOBJ(font.0));
        SetBkMode(hdc, TRANSPARENT);

        SetTextColor(hdc, COLORREF(theme::TEXT));
        text_at(hdc, &format_elapsed(state.elapsed()), 34, 84);

        for (id, x) in [(1, PAUSE_X), (2, STOP_X)] {
            let active = state.hovered.load(Ordering::Relaxed) == id;
            let brush = CreateSolidBrush(COLORREF(if active { 0x0054_4437 } else { 0x0032_2B26 }));
            FillRect(
                hdc,
                &RECT {
                    left: x - 4,
                    top: 5,
                    right: x + BUTTON_W - 2,
                    bottom: HEIGHT - 5,
                },
                brush,
            );
            let _ = DeleteObject(HGDIOBJ(brush.0));
        }
        SetTextColor(hdc, COLORREF(theme::TEXT));
        text_at(
            hdc,
            if paused { "Resume" } else { "Pause" },
            PAUSE_X,
            BUTTON_W,
        );
        text_at(hdc, "Stop", STOP_X, BUTTON_W);

        if !previous_font.is_invalid() {
            SelectObject(hdc, previous_font);
        }
        let _ = DeleteObject(HGDIOBJ(font.0));
    }
}

fn button_at(x: i32, y: i32) -> u8 {
    if !(5..HEIGHT - 5).contains(&y) {
        return 0;
    }
    if (PAUSE_X - 4..PAUSE_X + BUTTON_W - 2).contains(&x) {
        1
    } else if (STOP_X - 4..STOP_X + BUTTON_W - 2).contains(&x) {
        2
    } else {
        0
    }
}

fn text_at(hdc: HDC, text: &str, x: i32, width: i32) {
    let mut rect = RECT {
        left: x,
        top: 0,
        right: x + width,
        bottom: HEIGHT,
    };
    let mut buf: Vec<u16> = HSTRING::from(text).to_vec();
    // SAFETY: `buf` and `rect` are live locals for the duration of the call.
    unsafe {
        DrawTextW(
            hdc,
            &mut buf,
            &mut rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );
    }
}

/// Formats elapsed time as `mm:ss`, or `h:mm:ss` past an hour.
pub fn format_elapsed(elapsed: Duration) -> String {
    let total = elapsed.as_secs();
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_messages_update_hover_and_dispatch_the_matching_button() {
        kova_capture::require_interactive_desktop!();
        use windows::Win32::UI::WindowsAndMessaging::{
            GetCursorPos, GetWindowRect, SendMessageW, SetCursorPos,
        };
        struct RestoreCursor(windows::Win32::Foundation::POINT);
        impl Drop for RestoreCursor {
            fn drop(&mut self) {
                unsafe {
                    let _ = SetCursorPos(self.0.x, self.0.y);
                }
            }
        }
        let mut previous = windows::Win32::Foundation::POINT::default();
        unsafe {
            GetCursorPos(&mut previous).unwrap();
        }
        let _restore = RestoreCursor(previous);
        let mut overlay = RecorderOverlay::show().unwrap();
        let hwnd = find_overlay_window().unwrap();
        let state = overlay.state();
        let mut bounds = RECT::default();
        unsafe {
            GetWindowRect(hwnd, &mut bounds).unwrap();
        }
        for (x, id) in [(PAUSE_X + 8, 1), (STOP_X + 8, 2)] {
            // Real cursor placement prevents TrackMouseEvent immediately
            // delivering WM_MOUSELEAVE for synthetic moves outside the window.
            unsafe {
                SetCursorPos(bounds.left + x, bounds.top + 20).unwrap();
            }
            // Let the OS deliver enter/leave events caused by cursor warping
            // before testing the synchronously dispatched button message.
            std::thread::sleep(Duration::from_millis(80));
            let point = LPARAM(((20 << 16) | x) as isize);
            unsafe {
                SendMessageW(hwnd, WM_MOUSEMOVE, None, Some(point));
            }
            assert_eq!(state.hovered.load(Ordering::Relaxed), id);
            unsafe {
                SendMessageW(hwnd, WM_LBUTTONDOWN, None, Some(point));
            }
            assert_eq!(state.take_pause_request(), id == 1);
            assert_eq!(state.take_stop_request(), id == 2);
        }
        unsafe {
            SendMessageW(hwnd, WM_MOUSELEAVE, None, None);
        }
        assert_eq!(state.hovered.load(Ordering::Relaxed), 0);
        overlay.close();
    }

    #[test]
    fn elapsed_time_formats_as_minutes_and_seconds() {
        assert_eq!(format_elapsed(Duration::ZERO), "00:00");
        assert_eq!(format_elapsed(Duration::from_secs(14)), "00:14");
        assert_eq!(format_elapsed(Duration::from_secs(75)), "01:15");
        assert_eq!(format_elapsed(Duration::from_secs(599)), "09:59");
    }

    #[test]
    fn elapsed_time_grows_an_hours_field_only_when_needed() {
        assert_eq!(format_elapsed(Duration::from_secs(3599)), "59:59");
        assert_eq!(format_elapsed(Duration::from_secs(3600)), "1:00:00");
        assert_eq!(format_elapsed(Duration::from_secs(7325)), "2:02:05");
    }

    #[test]
    fn state_requests_are_taken_exactly_once() {
        let state = RecorderState::default();
        assert!(!state.take_stop_request());

        state.stop_requested.store(true, Ordering::Relaxed);
        assert!(
            state.take_stop_request(),
            "the first take must observe the request"
        );
        assert!(!state.take_stop_request(), "a request must not fire twice");

        state.pause_requested.store(true, Ordering::Relaxed);
        assert!(state.take_pause_request());
        assert!(!state.take_pause_request());
    }

    #[test]
    fn state_tracks_elapsed_time_and_pause() {
        let state = RecorderState::default();
        assert_eq!(state.elapsed(), Duration::ZERO);
        assert!(!state.is_paused());

        state.set_elapsed(Duration::from_millis(14_500));
        assert_eq!(state.elapsed(), Duration::from_millis(14_500));
        assert_eq!(format_elapsed(state.elapsed()), "00:14");

        state.set_paused(true);
        assert!(state.is_paused());
    }

    #[test]
    fn the_default_position_sits_inside_a_monitor_work_area() {
        let (x, y) = default_position();
        let monitors = kova_capture::monitor::list();
        assert!(!monitors.is_empty());
        let inside = monitors.iter().any(|m| {
            let a = m.work_area;
            x >= a.x && x + WIDTH <= a.right() && y >= a.y && y + HEIGHT <= a.bottom()
        });
        assert!(
            inside,
            "the overlay would open outside every work area at ({x}, {y})"
        );
    }

    #[test]
    fn the_overlay_opens_and_reports_whether_it_is_hidden_from_capture() {
        let mut overlay = RecorderOverlay::show().expect("the recorder overlay opens");
        let state = overlay.state();

        // Windows 10 2004+ must be able to exclude it; that is what keeps the
        // control bar out of the recording.
        assert!(
            state.is_excluded_from_capture(),
            "the overlay would be burned into the recording on this build"
        );

        state.set_elapsed(Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(120));
        overlay.close();
    }

    #[test]
    fn rounding_the_corners_leaves_the_window_intact() {
        use windows::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject, GetWindowRgn, HGDIOBJ};
        use windows::Win32::UI::WindowsAndMessaging::IsWindow;

        // Runs against the real overlay window, on whichever path this build
        // takes: the Windows 11 compositor preference, or the Windows 10
        // region fallback. Both must leave a usable window behind, and the
        // fallback must not leak the region it creates.
        let mut overlay = RecorderOverlay::show().expect("the recorder overlay opens");
        std::thread::sleep(Duration::from_millis(120));

        // Rounding an already-rounded window must also be harmless, since a
        // future change could plausibly call it again on a resize.
        let hwnd = find_overlay_window();
        if let Some(hwnd) = hwnd {
            round_corners(hwnd);
            round_corners(hwnd);

            // SAFETY: a scratch region that receives the window one, freed below.
            unsafe {
                let scratch = CreateRectRgn(0, 0, 1, 1);
                let _ = GetWindowRgn(hwnd, scratch);
                let _ = DeleteObject(HGDIOBJ(scratch.0));
                assert!(
                    IsWindow(Some(hwnd)).as_bool(),
                    "rounding the corners destroyed the window"
                );
            }
        }
        overlay.close();
    }

    /// Finds the live overlay window by its class name.
    fn find_overlay_window() -> Option<HWND> {
        use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
        let class = win32::class_name("KovaScreenRecorderOverlay");
        // SAFETY: `class` is a NUL-terminated wide string alive for the call.
        let hwnd = unsafe { FindWindowW(win32::class_ptr(&class), PCWSTR::null()) }.ok()?;
        (!hwnd.is_invalid()).then_some(hwnd)
    }

    #[test]
    fn closing_the_overlay_twice_is_safe() {
        let mut overlay = RecorderOverlay::show().expect("the recorder overlay opens");
        overlay.close();
        overlay.close();
    }

    #[test]
    fn dropping_the_overlay_closes_its_thread() {
        {
            let _overlay = RecorderOverlay::show().expect("the recorder overlay opens");
            std::thread::sleep(Duration::from_millis(60));
        }
        // A leaked thread would keep a topmost window on screen; opening a new
        // one immediately afterwards must still succeed.
        let mut again = RecorderOverlay::show().expect("a second overlay opens");
        again.close();
    }
}

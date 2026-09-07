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
//! # Cost
//!
//! The window repaints once a second, only while recording, and only the
//! ~230x40 pixels it occupies.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use kova_screen_core::{Error, Result};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DT_LEFT, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW,
    EndPaint, FillRect, HBRUSH, HDC, HGDIOBJ, InvalidateRect, PAINTSTRUCT, SelectObject, SetBkMode,
    SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GWLP_USERDATA, GetWindowLongPtrW, HTCAPTION, KillTimer, MSG, PM_REMOVE, PeekMessageW,
    PostQuitMessage, SW_SHOWNOACTIVATE, SetTimer, SetWindowDisplayAffinity, SetWindowLongPtrW,
    ShowWindow, TranslateMessage, WDA_EXCLUDEFROMCAPTURE, WM_DESTROY, WM_LBUTTONDOWN, WM_NCCREATE,
    WM_NCHITTEST, WM_PAINT, WM_TIMER, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
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
        let state = Arc::new(RecorderState::default());
        let close = Arc::new(AtomicBool::new(false));

        let thread_state = Arc::clone(&state);
        let thread_close = Arc::clone(&close);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();

        let thread = std::thread::Builder::new()
            .name("kova-recorder-overlay".into())
            .spawn(
                move || match OverlayWindow::create(Arc::clone(&thread_state)) {
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
}

impl OverlayWindow {
    fn create(state: Arc<RecorderState>) -> Result<Self> {
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

        // SAFETY: `hwnd` is live. SHOWNOACTIVATE preserves the user focus.
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            // One repaint per second is all a mm:ss timer needs.
            SetTimer(Some(hwnd), TIMER_ID, 500, None);
        }

        Ok(Self {
            hwnd,
            _state: state,
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
            std::thread::sleep(Duration::from_millis(16));
        }

        // SAFETY: `hwnd` is live and destroyed exactly once.
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_ID);
            let _ = DestroyWindow(self.hwnd);
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
            if (PAUSE_X..PAUSE_X + BUTTON_W).contains(&x) {
                state.pause_requested.store(true, Ordering::Relaxed);
            } else if (STOP_X..STOP_X + BUTTON_W).contains(&x) {
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
            if point.x < PAUSE_X {
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

        SetTextColor(hdc, COLORREF(theme::MUTED));
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

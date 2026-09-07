//! The region-selection overlay.
//!
//! # Why this is native Win32 and not a WebView
//!
//! This window sits directly between the hotkey and the screenshot, so its
//! open latency is the app perceived speed. A WebView costs 150-300 ms to spawn
//! and paint its first frame; this window appears in a few milliseconds because
//! it is one `CreateWindowExW` plus one blit of a bitmap we already have.
//!
//! # How it renders
//!
//! The desktop is captured **once**, before the window is shown, and that
//! bitmap is what the overlay paints. Nothing is composited live:
//!
//! 1. Blit the captured desktop into a back buffer.
//! 2. Alpha-blend black over the four regions outside the selection.
//! 3. Stroke the selection border and draw the size readout.
//!
//! Freezing the desktop this way is also what makes the result correct: the
//! pixels the user selects are exactly the pixels they saw when the hotkey
//! fired, even if something animated underneath in the meantime. The selected
//! rectangle is then cropped straight out of that same bitmap, so no second
//! capture -- and no chance of a mismatch -- is ever needed.
//!
//! A single window spans the whole virtual desktop rather than one per monitor,
//! which makes a drag that crosses a monitor boundary work with no extra code.

use kova_screen_core::{Bitmap, Error, PixelFormat, Point, Rect, Result};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_OVER, AlphaBlend, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, BeginPaint,
    CreateCompatibleDC, CreateDIBSection, CreatePen, CreateSolidBrush, DIB_RGB_COLORS, DT_CENTER,
    DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, HBRUSH, HDC,
    HGDIOBJ, InvalidateRect, PAINTSTRUCT, PS_SOLID, Rectangle, SRCCOPY, SelectObject, SetBkMode,
    SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GWLP_USERDATA, GetMessageW, GetWindowLongPtrW, HCURSOR, IDC_CROSS, LoadCursorW, MSG,
    PostQuitMessage, SW_SHOW, SetForegroundWindow, SetWindowLongPtrW, ShowWindow, TranslateMessage,
    WM_DESTROY, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCREATE, WM_PAINT,
    WM_RBUTTONDOWN, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows_core::{HSTRING, PCWSTR};

use crate::win32::{self, ModuleHandle};

/// The result of showing the overlay.
#[derive(Debug, Clone)]
pub struct Selection {
    /// The chosen rectangle, in virtual-desktop physical pixels.
    pub rect: Rect,
    /// The selected pixels, cropped from the frozen desktop capture.
    ///
    /// Returned alongside the rectangle so the caller never has to re-capture,
    /// which would risk grabbing a different frame than the one selected.
    pub bitmap: Bitmap,
}

/// Colours, taken from the shared Kova palette.
///
/// GDI wants `0x00BBGGRR`, which is the reverse of the `#RRGGBB` the rest of
/// the family writes, so each constant notes the hex it corresponds to.
mod theme {
    /// Selection border: Kova accent `#86d5f4`.
    pub const BORDER: u32 = 0x00F4_D586;
    /// Readout surface: `#1b1e22`.
    pub const READOUT_BG: u32 = 0x0022_1E1B;
    /// Readout text: `#f0f2f5`.
    pub const READOUT_FG: u32 = 0x00F5_F2F0;
    /// Dim strength over the unselected area, 0-255.
    pub const DIM_ALPHA: u8 = 115;
}

/// Height of the size readout pill, in pixels at 100% scaling.
const READOUT_H: i32 = 26;
const READOUT_W: i32 = 116;

/// Shows the overlay and blocks until the user selects or cancels.
///
/// Returns `Ok(None)` when the user pressed Escape, right-clicked, or released
/// without dragging a usable area -- all of which are ordinary cancellations,
/// not errors.
///
/// Must be called on a thread that can run a message loop. The caller should
/// spawn a thread for it rather than blocking the UI thread, which would freeze
/// the tray while the overlay is up.
pub fn select() -> Result<Option<Selection>> {
    let Some((rect, desktop, origin)) = show_overlay()? else {
        return Ok(None);
    };
    let bitmap = desktop.crop(rect.to_local(origin))?;
    Ok(Some(Selection { rect, bitmap }))
}

/// Shows the overlay and returns only the chosen rectangle.
///
/// Used to pick a recording area, where the frozen still is not wanted: a
/// recording captures live frames afterwards, so cropping the frozen desktop
/// would only cost a pointless copy of a potentially very large bitmap.
pub fn select_rect() -> Result<Option<Rect>> {
    Ok(show_overlay()?.map(|(rect, _, _)| rect))
}

/// Runs the overlay, returning the selection, the frozen desktop it was drawn
/// from, and that bitmap origin on the virtual desktop.
fn show_overlay() -> Result<Option<(Rect, Bitmap, Point)>> {
    let desktop_rect = kova_capture::monitor::virtual_desktop_bounds()?;
    // Capture first, then show the window, so the overlay itself can never
    // appear in the frozen frame.
    let desktop = kova_capture::gdi::capture_rect(desktop_rect)?;

    let mut state = Box::new(OverlayState::new(desktop_rect, desktop)?);
    let state_ptr = std::ptr::from_mut(state.as_mut());

    let class = register_class()?;
    let module = ModuleHandle::current()?;

    // SAFETY: the class is registered, and `state_ptr` outlives the window
    // because `state` is dropped only after the message loop returns.
    let hwnd = unsafe {
        CreateWindowExW(
            // TOOLWINDOW keeps it out of the taskbar and Alt+Tab.
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            PCWSTR(class.as_ptr()),
            PCWSTR(HSTRING::from("Kova Screen").as_ptr()),
            WS_POPUP,
            desktop_rect.x,
            desktop_rect.y,
            desktop_rect.width as i32,
            desktop_rect.height as i32,
            None,
            None,
            Some(module.handle().into()),
            Some(state_ptr.cast()),
        )
    }
    .map_err(|e| Error::Platform(format!("could not create the capture overlay: {e}")))?;

    // SAFETY: `hwnd` is live.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
    take_foreground(hwnd);

    run_message_loop();

    // The window destroys itself; make sure of it on every path.
    // SAFETY: destroying an already-destroyed window returns an error we ignore.
    unsafe {
        let _ = DestroyWindow(hwnd);
    }

    match state.result.take() {
        Some(rect) => Ok(Some((
            rect,
            std::mem::replace(
                &mut state.desktop,
                Bitmap::new_zeroed(1, 1, PixelFormat::Bgra8)?,
            ),
            Point::new(desktop_rect.x, desktop_rect.y),
        ))),
        None => Ok(None),
    }
}

/// Brings the overlay to the foreground and gives it keyboard focus.
///
/// Focus matters more than it looks: without it, Escape goes to whatever was
/// focused before and the overlay cannot be cancelled with the keyboard.
///
/// `SetForegroundWindow` is refused when Windows decides the calling process
/// has no right to steal focus. We usually do have that right, because the user
/// just pressed our hotkey, but the rules are subtle and the call is made from
/// a worker thread rather than the one that received the input. When it is
/// refused, the documented workaround is to attach our input queue to the
/// current foreground thread, which makes Windows treat the two as one thread
/// for focus purposes, and try again.
fn take_foreground(hwnd: HWND) {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::Input::KeyboardAndMouse::{SetActiveWindow, SetFocus};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    // SAFETY: `hwnd` is live for the whole function.
    unsafe {
        if SetForegroundWindow(hwnd).as_bool() {
            let _ = SetFocus(Some(hwnd));
            return;
        }

        let foreground = GetForegroundWindow();
        if foreground.is_invalid() {
            return;
        }

        let other = GetWindowThreadProcessId(foreground, None);
        let ours = GetCurrentThreadId();
        if other == 0 || other == ours {
            return;
        }

        // Attach, retry, detach. Leaving the queues attached would tie our
        // input state to another application for the rest of the session.
        if AttachThreadInput(ours, other, true).as_bool() {
            let _ = SetForegroundWindow(hwnd);
            let _ = SetActiveWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
            let _ = AttachThreadInput(ours, other, false);
        }
    }
}

/// Smallest selection considered deliberate.
///
/// Below this the user almost certainly clicked rather than dragged, and a 3x2
/// pixel screenshot is never what they meant.
const MIN_SELECTION: u32 = 4;

const _: () = assert!(
    MIN_SELECTION > 2,
    "a 2x2 drag must still count as a stray click"
);

fn run_message_loop() {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a live local. GetMessageW returns 0 on WM_QUIT, which
    // the window procedure posts when the overlay finishes.
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        // SAFETY: `msg` was filled by GetMessageW.
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Everything the window procedure needs. Owned by [`select`], borrowed by the
/// window through `GWLP_USERDATA`.
struct OverlayState {
    /// Origin and extent of the virtual desktop, so window-local coordinates
    /// can be converted back to desktop coordinates.
    origin: Point,
    desktop: Bitmap,
    /// The frozen desktop as a GDI bitmap, ready to blit.
    frozen: FrozenDesktop,
    anchor: Option<Point>,
    cursor: Point,
    dragging: bool,
    result: Option<Rect>,
}

impl OverlayState {
    fn new(rect: Rect, desktop: Bitmap) -> Result<Self> {
        let frozen = FrozenDesktop::new(&desktop)?;
        Ok(Self {
            origin: Point::new(rect.x, rect.y),
            desktop,
            frozen,
            anchor: None,
            cursor: Point::new(0, 0),
            dragging: false,
            result: None,
        })
    }

    /// The current selection in window-local coordinates.
    fn selection(&self) -> Option<Rect> {
        let anchor = self.anchor?;
        let rect = Rect::from_corners(anchor, self.cursor);
        (!rect.is_empty()).then_some(rect)
    }
}

/// A DIB section holding the frozen desktop, plus the memory DC it is selected
/// into. Both are released on drop.
struct FrozenDesktop {
    dc: HDC,
    bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
    previous: HGDIOBJ,
    width: i32,
    height: i32,
}

impl FrozenDesktop {
    fn new(source: &Bitmap) -> Result<Self> {
        // SAFETY: a null source DC creates a DC compatible with the screen.
        let dc = unsafe { CreateCompatibleDC(None) };
        if dc.is_invalid() {
            return Err(Error::Platform(
                "could not create the overlay device context".into(),
            ));
        }

        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: source.width() as i32,
                // Negative: top-down, matching our Bitmap row order.
                biHeight: -(source.height() as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: `info` describes a valid 32bpp DIB; `bits` receives the pixels.
        let bitmap =
            unsafe { CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) }
                .map_err(|e| {
                    Error::Platform(format!("could not allocate the overlay bitmap: {e}"))
                })?;

        if bits.is_null() {
            // SAFETY: both handles are valid and unused afterwards.
            unsafe {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                let _ = DeleteDC(dc);
            }
            return Err(Error::Platform("the overlay bitmap has no storage".into()));
        }

        // SAFETY: `bits` addresses exactly as many bytes as the source holds.
        unsafe {
            std::ptr::copy_nonoverlapping(
                source.data().as_ptr(),
                bits.cast::<u8>(),
                source.data().len(),
            );
        }

        // SAFETY: both handles are live.
        let previous = unsafe { SelectObject(dc, HGDIOBJ(bitmap.0)) };

        Ok(Self {
            dc,
            bitmap,
            previous,
            width: source.width() as i32,
            height: source.height() as i32,
        })
    }
}

impl Drop for FrozenDesktop {
    fn drop(&mut self) {
        // SAFETY: deselect before deleting, then release both handles once.
        unsafe {
            if !self.previous.is_invalid() {
                SelectObject(self.dc, self.previous);
            }
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

fn register_class() -> Result<Vec<u16>> {
    let class_name = win32::class_name("KovaScreenRegionOverlay");
    let module = ModuleHandle::current()?;
    // SAFETY: IDC_CROSS is a built-in cursor id.
    let cursor: HCURSOR = unsafe { LoadCursorW(None, IDC_CROSS) }.unwrap_or_default();

    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: module.handle().into(),
        hCursor: cursor,
        lpszClassName: win32::class_ptr(&class_name),
        ..Default::default()
    };

    win32::register_class(&class, "region overlay")?;
    Ok(class_name)
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // The state pointer is stashed at creation and read back on every message.
    if msg == WM_NCCREATE {
        // SAFETY: for WM_NCCREATE, lparam is a CREATESTRUCTW whose
        // lpCreateParams is the pointer we passed to CreateWindowExW.
        let create = unsafe {
            &*(lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW)
        };
        // SAFETY: storing a pointer we own for the window lifetime.
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        // SAFETY: default handling for the rest of WM_NCCREATE.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }

    // SAFETY: reads back the pointer stored above; null before WM_NCCREATE.
    let state_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut OverlayState;
    if state_ptr.is_null() {
        // SAFETY: no state yet, so nothing custom to do.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    // SAFETY: the pointer refers to the Box owned by `select`, which outlives
    // the window, and Win32 delivers messages on one thread at a time.
    let state = unsafe { &mut *state_ptr };

    match msg {
        WM_PAINT => {
            paint(hwnd, state);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let point = point_from_lparam(lparam);
            state.anchor = Some(point);
            state.cursor = point;
            state.dragging = true;
            // Capture keeps receiving moves if the pointer leaves the window,
            // which happens at the very edge of the virtual desktop.
            // SAFETY: `hwnd` is live.
            unsafe {
                SetCapture(hwnd);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if state.dragging {
                state.cursor = point_from_lparam(lparam);
                // SAFETY: `hwnd` is live; a null rect invalidates the whole window.
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if state.dragging {
                state.dragging = false;
                state.cursor = point_from_lparam(lparam);
                // SAFETY: balances SetCapture.
                unsafe {
                    let _ = ReleaseCapture();
                }

                match state.selection() {
                    // A stray click, not a drag: treat as a cancel.
                    Some(rect) if rect.width >= MIN_SELECTION && rect.height >= MIN_SELECTION => {
                        state.result = Some(Rect::new(
                            rect.x + state.origin.x,
                            rect.y + state.origin.y,
                            rect.width,
                            rect.height,
                        ));
                    }
                    _ => state.result = None,
                }
                finish(hwnd);
            }
            LRESULT(0)
        }
        WM_KEYDOWN if wparam.0 as u16 == VK_ESCAPE.0 => {
            state.result = None;
            finish(hwnd);
            LRESULT(0)
        }
        // Right-click is the other conventional cancel.
        WM_RBUTTONDOWN => {
            state.result = None;
            finish(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: ends the loop in `run_message_loop`.
            unsafe {
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        // SAFETY: everything else gets default handling.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn finish(hwnd: HWND) {
    // SAFETY: `hwnd` is live; this triggers WM_DESTROY, which quits the loop.
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

/// Extracts window-local coordinates from a mouse message.
fn point_from_lparam(lparam: LPARAM) -> Point {
    // The low and high words are signed 16-bit coordinates.
    let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    Point::new(x, y)
}

fn paint(hwnd: HWND, state: &OverlayState) {
    let mut ps = PAINTSTRUCT::default();
    // SAFETY: `ps` is a live local; BeginPaint is balanced by EndPaint below.
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

fn draw(hdc: HDC, state: &OverlayState) {
    let (w, h) = (state.frozen.width, state.frozen.height);

    // 1. The frozen desktop, at full brightness.
    // SAFETY: both DCs are live and the extent matches the frozen bitmap.
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::BitBlt(
            hdc,
            0,
            0,
            w,
            h,
            Some(state.frozen.dc),
            0,
            0,
            SRCCOPY,
        );
    }

    let selection = state.selection();

    // 2. Dim everything outside the selection. With no selection yet the whole
    // screen is dimmed, which is the cue that the overlay is active.
    match selection {
        None => dim_rect(
            hdc,
            RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            },
        ),
        Some(sel) => {
            let (l, t) = (sel.x, sel.y);
            let (r, b) = (sel.right(), sel.bottom());
            // Four bands around the selection, avoiding any overdraw.
            dim_rect(
                hdc,
                RECT {
                    left: 0,
                    top: 0,
                    right: w,
                    bottom: t,
                },
            );
            dim_rect(
                hdc,
                RECT {
                    left: 0,
                    top: b,
                    right: w,
                    bottom: h,
                },
            );
            dim_rect(
                hdc,
                RECT {
                    left: 0,
                    top: t,
                    right: l,
                    bottom: b,
                },
            );
            dim_rect(
                hdc,
                RECT {
                    left: r,
                    top: t,
                    right: w,
                    bottom: b,
                },
            );
        }
    }

    // 3. The selection border and the size readout.
    if let Some(sel) = selection {
        stroke_selection(hdc, sel);
        draw_readout(hdc, sel, w, h);
    }
}

/// Alpha-blends black over `rect`.
fn dim_rect(hdc: HDC, rect: RECT) {
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 0 || height <= 0 {
        return;
    }

    // A 1x1 black source stretched over the region: no per-pixel allocation.
    // SAFETY: creates a scratch DC and bitmap, both released below.
    unsafe {
        let src_dc = CreateCompatibleDC(Some(hdc));
        if src_dc.is_invalid() {
            return;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: 1,
                biHeight: -1,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(Some(src_dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        else {
            let _ = DeleteDC(src_dc);
            return;
        };
        if !bits.is_null() {
            // Opaque black; the blend function supplies the constant alpha.
            std::ptr::write_bytes(bits.cast::<u8>(), 0, 3);
            *bits.cast::<u8>().add(3) = 0xFF;
        }
        let previous = SelectObject(src_dc, HGDIOBJ(bitmap.0));

        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: theme::DIM_ALPHA,
            AlphaFormat: 0,
        };
        let _ = AlphaBlend(
            hdc, rect.left, rect.top, width, height, src_dc, 0, 0, 1, 1, blend,
        );

        if !previous.is_invalid() {
            SelectObject(src_dc, previous);
        }
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(src_dc);
    }
}

fn stroke_selection(hdc: HDC, sel: Rect) {
    // SAFETY: pen and brush are created, selected, then restored and deleted.
    unsafe {
        let pen = CreatePen(PS_SOLID, 1, COLORREF(theme::BORDER));
        let previous_pen = SelectObject(hdc, HGDIOBJ(pen.0));
        // A null brush keeps the interior (the bright selection) untouched.
        let previous_brush = SelectObject(
            hdc,
            windows::Win32::Graphics::Gdi::GetStockObject(
                windows::Win32::Graphics::Gdi::NULL_BRUSH,
            ),
        );

        let _ = Rectangle(hdc, sel.x, sel.y, sel.right(), sel.bottom());

        if !previous_brush.is_invalid() {
            SelectObject(hdc, previous_brush);
        }
        if !previous_pen.is_invalid() {
            SelectObject(hdc, previous_pen);
        }
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }
}

/// Draws the `1920 x 1080` pill near the selection.
///
/// Placed below the selection normally, above it when there is no room, and
/// always kept inside the desktop so it cannot be drawn off-screen.
fn draw_readout(hdc: HDC, sel: Rect, screen_w: i32, screen_h: i32) {
    let text = format!("{} x {}", sel.width, sel.height);
    let wide: Vec<u16> = HSTRING::from(text.as_str()).to_vec();

    let mut left = sel.x;
    let mut top = sel.bottom() + 8;
    if top + READOUT_H > screen_h {
        top = sel.y - READOUT_H - 8;
    }
    top = top.clamp(0, (screen_h - READOUT_H).max(0));
    left = left.clamp(0, (screen_w - READOUT_W).max(0));

    let mut rect = RECT {
        left,
        top,
        right: left + READOUT_W,
        bottom: top + READOUT_H,
    };

    // SAFETY: every GDI object created here is restored and deleted below.
    unsafe {
        let brush: HBRUSH = CreateSolidBrush(COLORREF(theme::READOUT_BG));
        FillRect(hdc, &rect, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));

        let font = win32::ui_font(15);
        let previous_font = SelectObject(hdc, HGDIOBJ(font.0));

        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, COLORREF(theme::READOUT_FG));
        let mut text_buf = wide.clone();
        DrawTextW(
            hdc,
            &mut text_buf,
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );

        if !previous_font.is_invalid() {
            SelectObject(hdc, previous_font);
        }
        let _ = DeleteObject(HGDIOBJ(font.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kova_screen_core::PixelFormat;

    fn desktop(width: u32, height: u32) -> Bitmap {
        let mut data = Vec::new();
        for y in 0..height {
            for x in 0..width {
                data.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, 90, 255]);
            }
        }
        Bitmap::from_raw(width, height, PixelFormat::Bgra8, data).unwrap()
    }

    #[test]
    fn mouse_coordinates_decode_including_negative_values() {
        // Positive coordinates.
        let p = point_from_lparam(LPARAM(((300i32 << 16) | 200i32) as isize));
        assert_eq!(p, Point::new(200, 300));

        // A monitor left of the primary produces negative window-local values,
        // which must sign-extend rather than becoming ~65000.
        let packed = (((-50i32 as u16 as i32) << 16) | (-20i32 as u16 as i32)) as isize;
        assert_eq!(point_from_lparam(LPARAM(packed)), Point::new(-20, -50));
    }

    #[test]
    fn selection_normalises_a_drag_in_any_direction() {
        let mut state = OverlayState::new(Rect::new(0, 0, 100, 100), desktop(100, 100)).unwrap();
        state.anchor = Some(Point::new(80, 60));
        state.cursor = Point::new(20, 10);
        assert_eq!(state.selection(), Some(Rect::new(20, 10, 60, 50)));
    }

    #[test]
    fn there_is_no_selection_before_a_drag_begins() {
        let state = OverlayState::new(Rect::new(0, 0, 100, 100), desktop(100, 100)).unwrap();
        assert_eq!(state.selection(), None);
    }

    #[test]
    fn a_zero_area_drag_yields_no_selection() {
        let mut state = OverlayState::new(Rect::new(0, 0, 100, 100), desktop(100, 100)).unwrap();
        state.anchor = Some(Point::new(40, 40));
        state.cursor = Point::new(40, 40);
        assert_eq!(state.selection(), None);
    }

    #[test]
    fn the_frozen_desktop_can_be_built_and_released() {
        // Exercises the DIB and DC lifecycle that the overlay depends on.
        for _ in 0..20 {
            let frozen = FrozenDesktop::new(&desktop(320, 240)).expect("frozen desktop");
            assert_eq!((frozen.width, frozen.height), (320, 240));
        }
    }

    #[test]
    fn selection_maps_back_to_desktop_coordinates() {
        // A window on a monitor at (-1920, 0): a local (10, 20) selection must
        // become (-1910, 20) on the virtual desktop.
        let origin = Point::new(-1920, 0);
        let local = Rect::new(10, 20, 100, 50);
        let global = Rect::new(
            local.x + origin.x,
            local.y + origin.y,
            local.width,
            local.height,
        );
        assert_eq!(global, Rect::new(-1910, 20, 100, 50));
        // And the inverse must recover the crop rectangle exactly.
        assert_eq!(global.to_local(origin), local);
    }

    #[test]
    fn cropping_the_frozen_capture_yields_the_selected_pixels() {
        // The end-to-end invariant: what is selected is cropped from the same
        // frozen frame, never re-captured.
        let bmp = desktop(64, 64);
        let selection = Rect::new(16, 8, 32, 16);
        let cropped = bmp.crop(selection).unwrap();
        assert_eq!(cropped.width(), 32);
        assert_eq!(cropped.height(), 16);
        // Top-left of the crop must equal the source pixel at (16, 8).
        let src_index = (8 * 64 + 16) * 4;
        assert_eq!(&cropped.data()[..4], &bmp.data()[src_index..src_index + 4]);
    }

    #[test]
    fn the_minimum_selection_rejects_a_stray_click() {
        // A 1x1 or 2x2 drag is a click, not a selection.
        let rect = Rect::from_corners(Point::new(10, 10), Point::new(11, 11));
        assert!(rect.width < MIN_SELECTION || rect.height < MIN_SELECTION);
    }

    #[test]
    fn the_window_class_registers_and_tolerates_re_registration() {
        // The overlay opens many times per session; the second registration
        // must not be treated as a failure.
        register_class().expect("first registration");
        register_class().expect("re-registration must be tolerated");
    }
}

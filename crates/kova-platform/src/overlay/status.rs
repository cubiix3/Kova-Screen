//! Lightweight in-app status card, including portable builds without a Start
//! menu registration. One bounded queue and one sleeping worker; no WebView.

use crate::win32::{self, ModuleHandle};
use kova_screen_core::{Error, Result};
use std::sync::{
    OnceLock,
    mpsc::{self, SyncSender},
};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::{HSTRING, PCWSTR};

struct Message {
    title: String,
    body: String,
}
static QUEUE: OnceLock<std::result::Result<SyncSender<Message>, String>> = OnceLock::new();

pub fn show(title: &str, body: &str) -> Result<()> {
    let sender = QUEUE
        .get_or_init(|| {
            let (tx, rx) = mpsc::sync_channel::<Message>(8);
            std::thread::Builder::new()
                .name("kova-status".into())
                .spawn(move || {
                    while let Ok(message) = rx.recv() {
                        if let Err(err) = show_message(message, 4000) {
                            tracing::warn!(%err, "could not show capture status");
                        }
                    }
                })
                .map(|_| tx)
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| Error::Platform(e.clone()))?;
    sender
        .try_send(Message {
            title: title.chars().take(120).collect(),
            body: body.chars().take(800).collect(),
        })
        .map_err(|e| Error::Platform(format!("could not queue capture status: {e}")))
}

fn show_message(message: Message, lifetime_ms: u32) -> Result<()> {
    let class = win32::class_name("KovaScreenStatus");
    let module = ModuleHandle::current()?;
    win32::register_class(
        &WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: module.handle().into(),
            lpszClassName: win32::class_ptr(&class),
            ..Default::default()
        },
        "capture status",
    )?;
    let monitor = kova_capture::monitor::from_cursor()
        .or_else(kova_capture::monitor::primary)
        .ok_or_else(|| Error::Platform("no monitor for capture status".into()))?;
    let area = monitor.work_area;
    let margin_x = 16.min((area.width as i32 / 2).max(0));
    let margin_y = 16.min((area.height as i32 / 2).max(0));
    let width = 360.min(area.width as i32 - margin_x * 2);
    let height = 104.min(area.height as i32 - margin_y * 2);
    let message = Box::new(message);
    // SAFETY: message is boxed and outlives the window and its message loop.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            PCWSTR(class.as_ptr()),
            &HSTRING::from("Kova Screen Status"),
            WS_POPUP,
            area.right() - width - margin_x,
            area.bottom() - height - margin_y,
            width,
            height,
            None,
            None,
            Some(module.handle().into()),
            Some(std::ptr::from_ref(message.as_ref()).cast()),
        )
    }
    .map_err(|e| Error::Platform(format!("could not create status card: {e}")))?;
    // SAFETY: live window; no focus change and no capture of the status card.
    unsafe {
        let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, 16, 16);
        if !region.is_invalid() && SetWindowRgn(hwnd, Some(region), false) == 0 {
            // Windows owns the region only after a successful SetWindowRgn.
            let _ = DeleteObject(HGDIOBJ(region.0));
        }
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        if SetTimer(Some(hwnd), 1, lifetime_ms, None) == 0 {
            let _ = DestroyWindow(hwnd);
            return Err(Error::Platform(
                "could not schedule status dismissal".into(),
            ));
        }
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if IsWindow(Some(hwnd)).as_bool() {
            let _ = DestroyWindow(hwnd);
        }
    }
    Ok(())
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: WM_NCCREATE carries the CREATESTRUCTW supplied at creation.
    if msg == WM_NCCREATE {
        unsafe {
            let create = &*(lp.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
    }
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_TIMER | WM_LBUTTONUP => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                let _ = KillTimer(Some(hwnd), 1);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            // SAFETY: message outlives the window; all GDI objects are released.
            unsafe {
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Message;
                if ptr.is_null() {
                    return DefWindowProcW(hwnd, msg, wp, lp);
                }
                let message = &*ptr;
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                let mut full = RECT::default();
                let _ = GetClientRect(hwnd, &mut full);
                let brush = CreateSolidBrush(COLORREF(0x0026_2321));
                FillRect(dc, &full, brush);
                let _ = DeleteObject(HGDIOBJ(brush.0));
                let outline = CreateRoundRectRgn(0, 0, full.right, full.bottom, 16, 16);
                let border = CreateSolidBrush(COLORREF(0x003D_3935));
                let _ = FrameRgn(dc, outline, border, 1, 1);
                let _ = DeleteObject(HGDIOBJ(border.0));
                let _ = DeleteObject(HGDIOBJ(outline.0));
                SetBkMode(dc, TRANSPARENT);
                let font = win32::ui_font(15);
                let previous = SelectObject(dc, HGDIOBJ(font.0));
                SetTextColor(dc, COLORREF(0x00EE_EAE6));
                let mut title: Vec<u16> = message.title.encode_utf16().collect();
                DrawTextW(
                    dc,
                    &mut title,
                    &mut RECT {
                        left: 16,
                        top: 15,
                        right: full.right - 16,
                        bottom: 34,
                    },
                    DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
                );
                let body_font = win32::ui_font(14);
                SelectObject(dc, HGDIOBJ(body_font.0));
                SetTextColor(dc, COLORREF(0x00B5_AFA8));
                let mut body: Vec<u16> = message.body.encode_utf16().collect();
                DrawTextW(
                    dc,
                    &mut body,
                    &mut RECT {
                        left: 16,
                        top: 40,
                        right: full.right - 16,
                        bottom: full.bottom - 12,
                    },
                    DT_WORDBREAK | DT_END_ELLIPSIS | DT_NOPREFIX,
                );
                SelectObject(dc, previous);
                let _ = DeleteObject(HGDIOBJ(font.0));
                let _ = DeleteObject(HGDIOBJ(body_font.0));
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
    use windows::Win32::System::Threading::{
        GR_GDIOBJECTS, GR_USEROBJECTS, GetCurrentProcess, GetGuiResources,
    };
    #[test]
    fn repeated_status_cards_release_their_windows_and_graphics() {
        let cycle = || {
            show_message(
                Message {
                    title: "Kova Screen - notification test".into(),
                    body: "Local status card lifecycle check".into(),
                },
                20,
            )
            .unwrap()
        };
        cycle();
        let count = || unsafe {
            (
                GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS),
                GetGuiResources(GetCurrentProcess(), GR_USEROBJECTS),
            )
        };
        let before = count();
        for _ in 0..20 {
            cycle();
        }
        assert_eq!(count(), before, "status card leaked Windows resources");
    }
}

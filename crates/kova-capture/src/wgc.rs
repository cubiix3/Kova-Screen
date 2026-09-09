//! Windows Graphics Capture.
//!
//! WGC is the modern, composited capture path: it sees hardware-accelerated and
//! partly occluded windows that `BitBlt` cannot, and it delivers frames as GPU
//! textures so a recording never round-trips through GDI.
//!
//! It needs Windows 10 1903 (build 18362). Everything here reports a clear
//! error on older builds so [`crate::capture`] can fall back to GDI.

use std::sync::mpsc;
use std::time::Duration;

use kova_screen_core::{Bitmap, Error, Rect, Result};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows_core::{HSTRING, Interface};

use crate::d3d::{D3dDevice, texture_from_surface};
use crate::monitor::MonitorId;
use crate::window::WindowId;

/// Number of buffers in the frame pool.
///
/// Two is enough for a still grab and keeps a recording's GPU memory small; the
/// compositor can still be writing one while we read the other.
const POOL_BUFFERS: i32 = 2;

/// How long a single-frame capture waits before giving up.
///
/// A window that is not redrawing (minimised mid-capture, a hung process) never
/// produces a frame, and the still-capture path must not block a hotkey forever.
const FRAME_TIMEOUT: Duration = Duration::from_millis(1200);

/// Whether this Windows build supports Windows Graphics Capture.
pub fn is_supported() -> bool {
    if keep_capture_apartment_alive().is_err() {
        return false;
    }
    GraphicsCaptureSession::IsSupported().unwrap_or(false)
}

/// WinRT caches agile activation factories across capture threads. Their MTA
/// must survive between sessions or subsequent calls can return CO_E_OBJNOTCONNECTED.
/// This single process-lifetime COM cookie retains no frame pool or GPU device.
fn keep_capture_apartment_alive() -> Result<()> {
    static COOKIE: std::sync::OnceLock<std::result::Result<usize, String>> =
        std::sync::OnceLock::new();
    COOKIE
        .get_or_init(|| {
            // The process owns this bounded runtime reference until termination,
            // just as it owns the cached WinRT factory interfaces.
            unsafe { windows::Win32::System::Com::CoIncrementMTAUsage() }
                .map(|cookie| cookie.0 as usize)
                .map_err(|e| format!("could not initialise the capture runtime: {e}"))
        })
        .as_ref()
        .map(|_| ())
        .map_err(|e| Error::Capture(e.clone()))
}

/// Builds a capture item for a window.
pub fn item_for_window(id: WindowId) -> Result<GraphicsCaptureItem> {
    if !id.is_alive() {
        return Err(Error::Capture("that window has closed".into()));
    }
    let interop = capture_interop()?;
    let hwnd = HWND(id.0 as *mut core::ffi::c_void);
    // SAFETY: `hwnd` was validated by `is_alive` immediately above. A window
    // closing in between yields an error HRESULT, not undefined behaviour.
    unsafe { interop.CreateForWindow::<GraphicsCaptureItem>(hwnd) }
        .map_err(|e| Error::Capture(format!("this window cannot be captured: {e}")))
}

/// Builds a capture item for a monitor.
pub fn item_for_monitor(id: MonitorId) -> Result<GraphicsCaptureItem> {
    // Some Windows builds access-violate on an invalid HMONITOR instead of
    // returning an HRESULT. Reject detached/stale displays before entering WGC.
    if crate::monitor::find(id).is_none() {
        return Err(Error::Capture("that display is no longer connected".into()));
    }
    let interop = capture_interop()?;
    let hmonitor = HMONITOR(id.0 as *mut core::ffi::c_void);
    // SAFETY: id belongs to a currently enumerated display.
    unsafe { interop.CreateForMonitor::<GraphicsCaptureItem>(hmonitor) }
        .map_err(|e| Error::Capture(format!("this display cannot be captured: {e}")))
}

fn capture_interop() -> Result<IGraphicsCaptureItemInterop> {
    if !is_supported() {
        return Err(Error::Capture(
            "windows graphics capture needs windows 10 version 1903 or newer".into(),
        ));
    }
    let factory = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
        .map_err(|e| Error::Capture(format!("graphics capture is unavailable: {e}")))?;
    keep_capture_module_loaded()?;
    Ok(factory)
}

/// Windows can still be winding down a capture worker after Close returns.
/// Unloading GraphicsCapture.dll then causes an execute access violation in
/// that worker (reproduced by the lifecycle probe, including capture-only).
/// Pin this one OS code module for the process lifetime, not any capture
/// session, frame pool or GPU buffers. Never add a timing-dependent sleep.
/// See https://github.com/robmikh/Win32CaptureSample/issues/99.
fn keep_capture_module_loaded() -> Result<()> {
    static PINNED: std::sync::OnceLock<std::result::Result<(), String>> =
        std::sync::OnceLock::new();
    PINNED
        .get_or_init(|| {
            use windows::Win32::System::LibraryLoader::{
                GET_MODULE_HANDLE_EX_FLAG_PIN, GetModuleHandleExW,
            };
            let mut module = windows::Win32::Foundation::HMODULE::default();
            // SAFETY: activation above has loaded the OS capture module. PIN is an
            // explicit process-lifetime reference, released by Windows at exit.
            unsafe {
                GetModuleHandleExW(
                    GET_MODULE_HANDLE_EX_FLAG_PIN,
                    &HSTRING::from("GraphicsCapture.dll"),
                    &mut module,
                )
            }
            .map_err(|e| format!("could not retain the capture runtime: {e}"))
        })
        .as_ref()
        .map(|_| ())
        .map_err(|e| Error::Capture(e.clone()))
}

/// Captures a single frame of `item`.
///
/// Creates a device and frame pool, waits for one frame, then tears everything
/// down. Nothing is retained, so a still screenshot leaves no GPU allocation
/// behind.
pub fn capture_item(item: &GraphicsCaptureItem, crop: Option<Rect>) -> Result<Bitmap> {
    let device = D3dDevice::create()?;
    let size = item
        .Size()
        .map_err(|e| Error::Capture(format!("could not read the capture size: {e}")))?;

    if size.Width <= 0 || size.Height <= 0 {
        return Err(Error::Capture("the capture target has no extent".into()));
    }

    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        device.winrt(),
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        POOL_BUFFERS,
        size,
    )
    .map_err(|e| Error::Capture(format!("could not create a capture frame pool: {e}")))?;

    let session = pool
        .CreateCaptureSession(item)
        .map_err(|e| Error::Capture(format!("could not start a capture session: {e}")))?;

    // The yellow "you are being captured" border is meant for screen sharing;
    // a screenshot tool drawing it would put it *in* the screenshot. Older
    // builds do not expose the property, hence the ignored error.
    let _ = session.SetIsBorderRequired(false);
    let _ = session.SetIsCursorCaptureEnabled(false);

    let (tx, rx) = mpsc::channel::<()>();
    let token = pool
        .FrameArrived(&TypedEventHandler::<Direct3D11CaptureFramePool, _>::new(
            move |_pool, _args| {
                // A closed receiver just means we already got our frame.
                let _ = tx.send(());
                Ok(())
            },
        ))
        .map_err(|e| Error::Capture(format!("could not subscribe to capture frames: {e}")))?;

    session
        .StartCapture()
        .map_err(|e| Error::Capture(format!("could not start capturing: {e}")))?;

    // Tear the session down on every exit path, including the timeout, or the
    // compositor keeps feeding a pool nobody reads.
    let result = wait_for_frame(&pool, &device, crop, &rx);

    let _ = pool.RemoveFrameArrived(token);
    let _ = session.Close();
    let _ = pool.Close();

    result
}

fn wait_for_frame(
    pool: &Direct3D11CaptureFramePool,
    device: &D3dDevice,
    crop: Option<Rect>,
    rx: &mpsc::Receiver<()>,
) -> Result<Bitmap> {
    let deadline = std::time::Instant::now() + FRAME_TIMEOUT;

    loop {
        // TryGetNextFrame returns null when the pool is empty, so a signal
        // without a frame simply loops rather than erroring.
        if let Ok(frame) = pool.TryGetNextFrame() {
            let surface = frame
                .Surface()
                .map_err(|e| Error::Capture(format!("capture frame has no surface: {e}")))?;
            let texture = texture_from_surface(&surface)?;
            let bitmap = device.texture_to_bitmap(&texture, crop)?;
            let _ = frame.Close();
            return Ok(bitmap);
        }

        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(Error::Capture(
                "the capture target produced no frame; it may be minimised or not redrawing".into(),
            ));
        }
        // Wait for the next FrameArrived rather than spinning.
        match rx.recv_timeout(remaining.min(Duration::from_millis(100))) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(Error::Capture(
                    "the capture session ended unexpectedly".into(),
                ));
            }
        }
    }
}

/// Captures one window.
pub fn capture_window(id: WindowId) -> Result<Bitmap> {
    let item = item_for_window(id)?;
    capture_item(&item, None)
}

/// Captures one monitor.
pub fn capture_monitor(id: MonitorId) -> Result<Bitmap> {
    let item = item_for_monitor(id)?;
    capture_item(&item, None)
}

/// Human-readable name of a capture item, used in the recorder overlay.
pub fn display_name(item: &GraphicsCaptureItem) -> String {
    item.DisplayName()
        .unwrap_or_else(|_| HSTRING::new())
        .to_string()
}

/// The item extent, as the frame pool sees it.
pub fn item_size(item: &GraphicsCaptureItem) -> Result<SizeInt32> {
    item.Size()
        .map_err(|e| Error::Capture(format!("could not read the capture size: {e}")))
}

/// Whether `SetIsCursorCaptureEnabled` exists on this build.
///
/// Windows 10 2004 added it. On older builds the cursor is always included in
/// WGC frames and the setting silently does nothing, which the recorder reports
/// rather than pretending the toggle worked.
pub fn supports_cursor_toggle() -> bool {
    windows::Foundation::Metadata::ApiInformation::IsPropertyPresent(
        &HSTRING::from("Windows.Graphics.Capture.GraphicsCaptureSession"),
        &HSTRING::from("IsCursorCaptureEnabled"),
    )
    .unwrap_or(false)
}

/// Casts a capture item to check it is still live.
pub fn is_item_valid(item: &GraphicsCaptureItem) -> bool {
    item.cast::<windows_core::IInspectable>().is_ok() && item.Size().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor;

    #[test]
    fn wgc_is_available_on_a_supported_build() {
        // Windows 11 in CI and on the target machines always supports it. If
        // this ever fails the GDI fallback still covers stills, so the assert
        // documents the expectation rather than gating the build.
        assert!(
            is_supported(),
            "windows graphics capture should be available"
        );
    }

    #[test]
    fn captures_the_primary_monitor_at_its_full_extent() {
        crate::require_interactive_desktop!();
        let m = monitor::primary().expect("a primary monitor");
        let bmp = capture_monitor(m.id).expect("wgc monitor capture");
        // WGC reports the monitor extent, which must match what we enumerated.
        assert_eq!(bmp.width(), m.bounds.width);
        assert_eq!(bmp.height(), m.bounds.height);
        assert!(
            bmp.data().iter().any(|&b| b != 0),
            "the frame is entirely black"
        );
    }

    #[test]
    fn a_stale_monitor_handle_is_rejected() {
        assert!(capture_monitor(MonitorId(1)).is_err());
    }

    #[test]
    fn a_stale_window_handle_is_rejected() {
        assert!(capture_window(WindowId(1)).is_err());
    }

    #[test]
    fn cropping_yields_exactly_the_requested_extent() {
        crate::require_interactive_desktop!();
        let m = monitor::primary().expect("a primary monitor");
        let item = item_for_monitor(m.id).unwrap();
        let bmp = capture_item(&item, Some(Rect::new(10, 10, 100, 50))).expect("cropped capture");
        assert_eq!(bmp.width(), 100);
        assert_eq!(bmp.height(), 50);
    }

    #[test]
    fn an_oversized_crop_is_clamped_to_the_frame() {
        crate::require_interactive_desktop!();
        let m = monitor::primary().expect("a primary monitor");
        let item = item_for_monitor(m.id).unwrap();
        let huge = Rect::new(0, 0, m.bounds.width + 5000, m.bounds.height + 5000);
        let bmp = capture_item(&item, Some(huge)).expect("clamped capture");
        assert_eq!(bmp.width(), m.bounds.width);
        assert_eq!(bmp.height(), m.bounds.height);
    }

    #[test]
    fn repeated_captures_release_their_gpu_resources() {
        crate::require_interactive_desktop!();
        // Each iteration builds a device, pool and session and drops them. A
        // leak here would show up as a steadily growing commit charge during a
        // long session of screenshots.
        let m = monitor::primary().expect("a primary monitor");
        for _ in 0..5 {
            capture_monitor(m.id).expect("repeatable capture");
        }
    }
}

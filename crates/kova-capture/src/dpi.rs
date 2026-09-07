//! DPI awareness.
//!
//! Kova Screen must run Per-Monitor-DPI-V2. Under any lesser mode Windows lies
//! to the process: `GetSystemMetrics` and monitor rects come back in scaled
//! ("logical") pixels, and the compositor stretches the overlay window. The
//! visible symptoms are a selection rectangle that does not line up with the
//! cursor on a scaled display, and screenshots that are blurry and the wrong
//! size on anything other than 100%.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, GetDpiForWindow,
    MDT_EFFECTIVE_DPI, SetProcessDpiAwarenessContext,
};

/// The DPI Windows treats as 100% scaling.
pub const USER_DEFAULT_SCREEN_DPI: u32 = 96;

/// Opts the process into Per-Monitor-DPI-V2.
///
/// Must run before any window is created and before any monitor geometry is
/// read. Safe to call more than once.
///
/// Returns `false` if the request was rejected, which happens when awareness
/// was already set (for example by an application manifest, the supported path
/// for the shipped build). That is not an error: the manifest value wins and is
/// the one we wanted.
pub fn enable_per_monitor_dpi_awareness() -> bool {
    // SAFETY: no arguments to validate; the call only mutates process state and
    // is documented as safe to invoke from any thread before UI exists.
    let ok = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    if let Err(err) = ok {
        tracing::debug!(
            ?err,
            "dpi awareness already set, keeping the existing context"
        );
        return false;
    }
    true
}

/// Effective DPI of `monitor`, or 96 when the query fails.
///
/// Falling back to 96 keeps the caller working at 1:1 rather than propagating an
/// error through the capture path for something purely cosmetic.
pub fn for_monitor(monitor: HMONITOR) -> u32 {
    let mut dpi_x = 0u32;
    let mut dpi_y = 0u32;
    // SAFETY: both out-pointers reference live stack locals for the whole call.
    let hr = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    match hr {
        Ok(()) if dpi_x > 0 => dpi_x,
        _ => USER_DEFAULT_SCREEN_DPI,
    }
}

/// DPI of the monitor `hwnd` currently sits on, or 96 when unknown.
pub fn for_window(hwnd: HWND) -> u32 {
    // SAFETY: `hwnd` may be stale; the API returns 0 rather than faulting.
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        USER_DEFAULT_SCREEN_DPI
    } else {
        dpi
    }
}

/// Scale factor for a DPI value, where 96 DPI is 1.0.
pub fn scale_factor(dpi: u32) -> f64 {
    dpi as f64 / USER_DEFAULT_SCREEN_DPI as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_factor_maps_the_standard_windows_levels() {
        assert_eq!(scale_factor(96), 1.0);
        assert_eq!(scale_factor(120), 1.25);
        assert_eq!(scale_factor(144), 1.5);
        assert_eq!(scale_factor(192), 2.0);
    }

    #[test]
    fn enabling_awareness_twice_does_not_panic() {
        // The second call is expected to be rejected; it must not be fatal.
        let _ = enable_per_monitor_dpi_awareness();
        let _ = enable_per_monitor_dpi_awareness();
    }

    #[test]
    fn invalid_handles_degrade_to_100_percent() {
        assert_eq!(
            for_monitor(HMONITOR(std::ptr::null_mut())),
            USER_DEFAULT_SCREEN_DPI
        );
        assert_eq!(
            for_window(HWND(std::ptr::null_mut())),
            USER_DEFAULT_SCREEN_DPI
        );
    }
}

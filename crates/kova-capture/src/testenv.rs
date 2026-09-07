//! Test-only helpers for probing the session Kova Screen is running in.
//!
//! Screen capture needs an unlocked, interactive desktop. On a locked
//! workstation, in a disconnected RDP session, or on a CI runner with no
//! attached display, Windows refuses the blit with `ERROR_ACCESS_DENIED` and
//! Windows Graphics Capture returns frames that are entirely black.
//!
//! Neither is a defect in this crate, so the tests that genuinely need real
//! pixels check [`interactive_desktop`] first and skip with a printed reason
//! rather than failing. Tests that assert *error* handling -- rejecting an
//! empty rect, a stale handle, a bad crop -- do not gate on this, because they
//! must hold in every session.

use crate::monitor;
use kova_screen_core::Rect;

/// Whether this session can actually read the screen.
///
/// Probes with a real 8x8 capture, which is the same call path the tests use,
/// so it cannot report available when the tests would fail.
pub fn interactive_desktop() -> bool {
    let Ok(desktop) = monitor::virtual_desktop_bounds() else {
        return false;
    };
    crate::gdi::capture_rect(Rect::new(desktop.x, desktop.y, 8, 8)).is_ok()
}

/// Skips the calling test when the desktop cannot be captured.
///
/// Expands to an early `return`, printing why so a skipped run is visible in
/// the test output rather than silently passing.
#[macro_export]
macro_rules! require_interactive_desktop {
    () => {
        if !$crate::testenv::interactive_desktop() {
            eprintln!(
                "skipping {}: this session cannot capture the screen \
                 (locked workstation, disconnected session, or no display)",
                module_path!()
            );
            return;
        }
    };
}

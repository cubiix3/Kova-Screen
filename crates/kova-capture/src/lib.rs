//! Screen, window and region capture for Windows.
//!
//! # Coordinate system
//!
//! Everything in this crate speaks **physical pixels on the virtual desktop**.
//! The virtual desktop origin is the top-left of the primary monitor, so a
//! display placed to the left of the primary has negative `x`. The process must
//! be Per-Monitor-DPI-V2 aware ([`dpi::enable_per_monitor_dpi_awareness`]) for
//! those numbers to be physical rather than scaled; without it, a capture on a
//! 150% display comes back blurry and the wrong size.
//!
//! # Backends
//!
//! Two capture paths exist, picked per request rather than globally:
//!
//! - [`wgc`] -- Windows Graphics Capture. GPU-side, composited, and the only
//!   way to correctly capture a window that is partly occluded or hardware
//!   accelerated. Requires Windows 10 1903+. Used for windows and for
//!   recording, where its frame-pool model avoids a per-frame GDI round trip.
//! - [`gdi`] -- `BitBlt` from the screen DC. Works everywhere, needs no GPU
//!   device, and is the fastest way to grab one arbitrary rectangle of the
//!   composited desktop. Used for region and fullscreen stills, and as the
//!   fallback when WGC is unavailable.

#![cfg(windows)]

pub mod cursor;
pub mod d3d;
pub mod dpi;
pub mod gdi;
pub mod monitor;
pub mod session;
// Not `cfg(test)`: downstream crates' tests need the same probe, and a
// cfg(test) module is not visible across a crate boundary.
#[doc(hidden)]
pub mod testenv;
pub mod wgc;
pub mod window;

use kova_screen_core::{Bitmap, Error, Rect, Result};

pub use monitor::Monitor;
pub use session::{CaptureSession, FrameSink};
pub use window::WindowInfo;

/// What a capture request targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureTarget {
    /// An arbitrary rectangle on the virtual desktop, spanning monitors if need be.
    Region(Rect),
    /// One monitor, identified by its device handle.
    Monitor(monitor::MonitorId),
    /// One top-level window, identified by its `HWND`.
    Window(window::WindowId),
    /// Every monitor, stitched into one image covering the virtual desktop.
    AllMonitors,
}

impl CaptureTarget {
    /// The rectangle this target occupies on the virtual desktop.
    ///
    /// For windows this is the DWM extended frame, i.e. what the user sees,
    /// excluding the invisible resize border Win32 reports.
    pub fn bounds(&self) -> Result<Rect> {
        match *self {
            CaptureTarget::Region(rect) => Ok(rect),
            CaptureTarget::Monitor(id) => monitor::find(id).map(|m| m.bounds).ok_or_else(|| {
                Error::Capture("the selected monitor is no longer connected".into())
            }),
            CaptureTarget::Window(id) => window::bounds(id),
            CaptureTarget::AllMonitors => monitor::virtual_desktop_bounds(),
        }
    }
}

/// Options that apply to a single still capture.
#[derive(Debug, Clone, Copy, Default)]
pub struct CaptureOptions {
    /// Composite the mouse cursor into the result.
    pub include_cursor: bool,
}

/// Takes one screenshot.
///
/// Backend choice is per target: windows go through WGC (falling back to GDI if
/// the OS or GPU refuses), everything else uses GDI directly because it is a
/// single blit with no device setup.
pub fn capture(target: CaptureTarget, options: CaptureOptions) -> Result<Bitmap> {
    let mut bitmap = match target {
        CaptureTarget::Window(id) => capture_window_best_effort(id)?,
        CaptureTarget::Region(rect) => {
            if rect.is_empty() {
                return Err(Error::Capture("the selected region is empty".into()));
            }
            gdi::capture_rect(rect)?
        }
        CaptureTarget::Monitor(_) | CaptureTarget::AllMonitors => {
            gdi::capture_rect(target.bounds()?)?
        }
    };

    if options.include_cursor {
        let origin = target.bounds()?;
        // A missing cursor must not fail the screenshot the user already took.
        if let Err(err) = cursor::draw_into(&mut bitmap, origin) {
            tracing::debug!(%err, "could not composite the cursor");
        }
    }

    Ok(bitmap)
}

/// Captures a window via WGC, falling back to a GDI screen blit.
///
/// The fallback matters on Windows 10 builds before 1903 and on machines where
/// D3D device creation fails (stale drivers, RDP sessions). A worse-looking
/// screenshot beats no screenshot.
fn capture_window_best_effort(id: window::WindowId) -> Result<Bitmap> {
    match wgc::capture_window(id) {
        Ok(bitmap) => Ok(bitmap),
        Err(err) => {
            tracing::warn!(%err, "windows graphics capture failed, falling back to gdi");
            gdi::capture_rect(window::bounds(id)?)
        }
    }
}

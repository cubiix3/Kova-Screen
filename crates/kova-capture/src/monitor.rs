//! Monitor enumeration and virtual-desktop geometry.

use kova_screen_core::{Error, Point, Rect, Result};
use windows::Win32::Foundation::{LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MONITORINFOEXW, MonitorFromPoint,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};
use windows_core::BOOL;

use crate::dpi;

/// Stable-per-session identifier for a monitor.
///
/// Wraps the raw `HMONITOR` as an integer so it can cross the Tauri IPC
/// boundary. `HMONITOR` values are only valid until the display topology
/// changes, so every lookup re-enumerates and returns `None` for a monitor that
/// has since been unplugged rather than trusting a cached handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MonitorId(pub isize);

/// A connected display.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Monitor {
    pub id: MonitorId,
    /// Full monitor rectangle in virtual-desktop physical pixels.
    pub bounds: Rect,
    /// Bounds minus the taskbar and other appbars.
    pub work_area: Rect,
    /// Effective DPI; 96 means 100% scaling.
    pub dpi: u32,
    pub is_primary: bool,
    /// Device name such as `\\.\DISPLAY1`, shown in the monitor picker.
    pub device_name: String,
}

impl Monitor {
    /// Scale factor where 1.0 is 100%.
    pub fn scale_factor(&self) -> f64 {
        dpi::scale_factor(self.dpi)
    }
}

fn rect_from_win32(r: RECT) -> Rect {
    // RECT is exclusive on right/bottom, so the differences are the extent.
    // Widen to i64 first: a virtual desktop spanning negative coordinates can
    // make `right - left` overflow i32 only for absurd values, but the widening
    // costs nothing and removes the edge case.
    let width = (r.right as i64 - r.left as i64).max(0) as u32;
    let height = (r.bottom as i64 - r.top as i64).max(0) as u32;
    Rect::new(r.left, r.top, width, height)
}

/// Enumerates every connected monitor.
///
/// Returns an empty vector only in a session with no display (a service or a
/// disconnected RDP session), which callers surface as a capture error.
pub fn list() -> Vec<Monitor> {
    let mut monitors: Vec<Monitor> = Vec::new();
    let ptr = &mut monitors as *mut Vec<Monitor>;

    // SAFETY: `enum_proc` is a valid callback for this signature, and `ptr`
    // outlives the enumeration because `EnumDisplayMonitors` is synchronous.
    let _ = unsafe { EnumDisplayMonitors(None, None, Some(enum_proc), LPARAM(ptr as isize)) };

    monitors
}

unsafe extern "system" fn enum_proc(
    handle: HMONITOR,
    _hdc: HDC,
    _clip: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    // SAFETY: `lparam` is the `&mut Vec<Monitor>` handed to EnumDisplayMonitors,
    // which is alive for the duration of the synchronous enumeration.
    let monitors = unsafe { &mut *(lparam.0 as *mut Vec<Monitor>) };

    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };

    // SAFETY: `cbSize` is set to the MONITORINFOEXW size, which is what the
    // API uses to decide whether the device-name tail is present.
    let ok = unsafe {
        GetMonitorInfoW(handle, std::ptr::from_mut(&mut info).cast::<MONITORINFO>()).as_bool()
    };
    if !ok {
        // Skip a monitor that vanished mid-enumeration rather than aborting.
        return TRUE;
    }

    let device_name = String::from_utf16_lossy(&info.szDevice)
        .trim_end_matches('\0')
        .to_string();

    monitors.push(Monitor {
        id: MonitorId(handle.0 as isize),
        bounds: rect_from_win32(info.monitorInfo.rcMonitor),
        work_area: rect_from_win32(info.monitorInfo.rcWork),
        dpi: dpi::for_monitor(handle),
        // MONITORINFOF_PRIMARY == 1
        is_primary: info.monitorInfo.dwFlags & 1 != 0,
        device_name,
    });

    TRUE
}

/// Looks up a monitor by id, re-enumerating so a stale handle returns `None`.
pub fn find(id: MonitorId) -> Option<Monitor> {
    list().into_iter().find(|m| m.id == id)
}

/// The primary monitor, or the first one if Windows reports no primary.
pub fn primary() -> Option<Monitor> {
    let monitors = list();
    monitors
        .iter()
        .find(|m| m.is_primary)
        .cloned()
        .or_else(|| monitors.first().cloned())
}

/// The monitor containing `point`, or the nearest one if it lies in a gap
/// between displays.
pub fn from_point(point: Point) -> Option<Monitor> {
    let pt = windows::Win32::Foundation::POINT {
        x: point.x,
        y: point.y,
    };
    // SAFETY: takes a POINT by value; MONITOR_DEFAULTTONEAREST guarantees a
    // non-null handle whenever at least one display exists.
    let handle = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
    find(MonitorId(handle.0 as isize))
}

/// The monitor under the mouse cursor, used to pick a target for a fullscreen
/// capture triggered by a hotkey.
pub fn from_cursor() -> Option<Monitor> {
    crate::cursor::position().ok().and_then(from_point)
}

/// Bounding rectangle of every monitor combined.
///
/// Read from `SM_*VIRTUALSCREEN` rather than by unioning the enumerated
/// monitors: the metrics are what `BitBlt` on the screen DC uses as its origin,
/// so sourcing both from the same place keeps the offsets consistent.
pub fn virtual_desktop_bounds() -> Result<Rect> {
    // SAFETY: GetSystemMetrics takes an index and cannot fail.
    let (x, y, w, h) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    if w <= 0 || h <= 0 {
        return Err(Error::Capture(
            "the virtual desktop has no extent; is a display connected?".into(),
        ));
    }
    Ok(Rect::new(x, y, w as u32, h as u32))
}

/// Clamps `rect` to the visible desktop.
///
/// A selection can be dragged into the dead space between two differently sized
/// monitors. Blitting that area returns undefined pixels, so the region is
/// trimmed to what is actually on screen before capture.
pub fn clamp_to_desktop(rect: Rect) -> Result<Rect> {
    let desktop = virtual_desktop_bounds()?;
    rect.intersect(&desktop)
        .ok_or_else(|| Error::Capture("the selected region is off-screen".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_conversion_handles_negative_origins() {
        // A monitor left of and above the primary.
        let r = RECT {
            left: -1920,
            top: -180,
            right: 0,
            bottom: 900,
        };
        assert_eq!(rect_from_win32(r), Rect::new(-1920, -180, 1920, 1080));
    }

    #[test]
    fn rect_conversion_never_yields_negative_extent() {
        let inverted = RECT {
            left: 100,
            top: 100,
            right: 0,
            bottom: 0,
        };
        let r = rect_from_win32(inverted);
        assert_eq!(r.width, 0);
        assert_eq!(r.height, 0);
    }

    #[test]
    fn enumeration_reports_at_least_one_monitor_with_a_real_extent() {
        let monitors = list();
        assert!(
            !monitors.is_empty(),
            "a desktop session must expose a monitor"
        );
        for m in &monitors {
            assert!(
                !m.bounds.is_empty(),
                "{} has an empty extent",
                m.device_name
            );
            assert!(m.dpi >= 96, "{} reported an implausible dpi", m.device_name);
        }
    }

    #[test]
    fn exactly_one_monitor_is_primary() {
        let primaries = list().into_iter().filter(|m| m.is_primary).count();
        assert_eq!(primaries, 1);
    }

    #[test]
    fn every_monitor_lies_inside_the_virtual_desktop() {
        let desktop = virtual_desktop_bounds().unwrap();
        for m in list() {
            assert_eq!(
                m.bounds.intersect(&desktop),
                Some(m.bounds),
                "{} is not contained in the virtual desktop",
                m.device_name
            );
        }
    }

    #[test]
    fn work_area_is_contained_in_monitor_bounds() {
        for m in list() {
            assert_eq!(m.work_area.intersect(&m.bounds), Some(m.work_area));
        }
    }

    #[test]
    fn monitors_can_be_looked_up_by_id() {
        for m in list() {
            assert_eq!(find(m.id).as_ref(), Some(&m));
        }
        // A handle that was never real must not resolve.
        assert!(find(MonitorId(-1)).is_none());
    }

    #[test]
    fn point_lookup_finds_the_containing_monitor() {
        let p = primary().expect("a primary monitor");
        let centre = Point::new(
            p.bounds.x + p.bounds.width as i32 / 2,
            p.bounds.y + p.bounds.height as i32 / 2,
        );
        assert_eq!(from_point(centre).map(|m| m.id), Some(p.id));
    }

    #[test]
    fn point_far_outside_snaps_to_the_nearest_monitor() {
        // MONITOR_DEFAULTTONEAREST must never leave us without a monitor.
        assert!(from_point(Point::new(i32::MAX / 2, i32::MAX / 2)).is_some());
    }

    #[test]
    fn clamping_trims_a_region_that_runs_off_screen() {
        let desktop = virtual_desktop_bounds().unwrap();
        let overhang = Rect::new(
            desktop.x - 500,
            desktop.y - 500,
            desktop.width,
            desktop.height,
        );
        let clamped = clamp_to_desktop(overhang).unwrap();
        assert_eq!(clamped.x, desktop.x);
        assert_eq!(clamped.y, desktop.y);
        assert!(clamped.width <= desktop.width);
    }

    #[test]
    fn clamping_rejects_a_fully_off_screen_region() {
        let desktop = virtual_desktop_bounds().unwrap();
        let far = Rect::new(desktop.right() + 1000, desktop.bottom() + 1000, 100, 100);
        assert!(clamp_to_desktop(far).is_err());
    }
}

//! The two WebView windows: Settings and Recent Captures.
//!
//! Both are created on demand and destroyed when closed, never hidden. A hidden
//! WebView still holds its renderer process and tens of megabytes of RAM, which
//! would defeat the point of an app designed to sit in the tray for days. The
//! cost is a short spawn the first time each is opened, which is acceptable for
//! a window a user visits occasionally -- unlike the capture overlay, which is
//! native precisely because it cannot afford that.

use std::sync::Arc;

use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

use crate::app::AppState;

/// Window labels, also used as the route the frontend reads.
pub const SETTINGS: &str = "settings";
pub const HISTORY: &str = "history";

/// Opens (or focuses) the settings window.
pub fn open_settings<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    open(app, SETTINGS, "Kova Screen \u{2014} Settings", 720.0, 620.0)
}

/// Opens (or focuses) the recent-captures window.
pub fn open_history<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    open(
        app,
        HISTORY,
        "Kova Screen \u{2014} Recent Captures",
        760.0,
        560.0,
    )
}

fn open<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
    title: &str,
    width: f64,
    height: f64,
) -> tauri::Result<()> {
    // Already open: bring it forward rather than spawning a second copy.
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }

    let url = WebviewUrl::App(format!("index.html?view={label}").into());
    WebviewWindowBuilder::new(app, label, url)
        .title(title)
        .inner_size(width, height)
        .min_inner_size(560.0, 420.0)
        .resizable(true)
        .center()
        // The frontend paints the Kova canvas colour; matching it here removes
        // the white flash a WebView otherwise shows while it loads.
        .background_color(tauri::window::Color(0x10, 0x12, 0x14, 0xFF))
        .build()?;

    Ok(())
}

/// Opens the capture folder in Explorer, creating it if needed.
pub fn reveal_capture_folder<R: Runtime>(
    app: &AppHandle<R>,
    state: &Arc<AppState>,
) -> tauri::Result<()> {
    use tauri_plugin_opener::OpenerExt;

    let dir = state
        .capture_dir()
        .map_err(|e| tauri::Error::Anyhow(e.into()))?;

    app.opener()
        .open_path(dir.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| tauri::Error::Anyhow(anyhow::anyhow!(e.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_labels_are_distinct_and_stable() {
        // The frontend switches on these exact strings.
        assert_ne!(SETTINGS, HISTORY);
        assert_eq!(SETTINGS, "settings");
        assert_eq!(HISTORY, "history");
    }
}

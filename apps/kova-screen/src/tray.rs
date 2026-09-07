//! The system tray icon and its menu.
//!
//! Kova Screen has no main window: the tray *is* the app. The menu mirrors the
//! product spec exactly, and each item dispatches the same [`crate::actions::Action`]
//! a hotkey would, so there is only ever one code path per capture.

use std::sync::Arc;

use tauri::menu::{Menu, MenuBuilder, MenuEvent, MenuItemBuilder, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

use crate::actions::{self, Action};
use crate::app::AppState;
use crate::windows;

/// Menu item ids. Kept as constants so the builder and the handler cannot drift.
mod id {
    pub const REGION: &str = "screenshot_region";
    pub const WINDOW: &str = "screenshot_window";
    pub const FULLSCREEN: &str = "screenshot_fullscreen";
    pub const ALL_MONITORS: &str = "screenshot_all_monitors";
    pub const RECORD_MP4: &str = "record_mp4";
    pub const RECORD_GIF: &str = "record_gif";
    pub const STOP_RECORDING: &str = "stop_recording";
    pub const HISTORY: &str = "recent_captures";
    pub const OPEN_FOLDER: &str = "open_capture_folder";
    pub const SETTINGS: &str = "settings";
    pub const EXIT: &str = "exit";
}

/// Builds the tray icon and wires up its menu.
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let menu = build_menu(app)?;

    TrayIconBuilder::with_id("kova-screen")
        .icon(app.default_window_icon().cloned().ok_or_else(|| {
            tauri::Error::AssetNotFound("the tray icon is missing from the bundle".into())
        })?)
        .tooltip("Kova Screen")
        .menu(&menu)
        // The menu belongs on right-click; a left click opens the history,
        // which is the thing people reach for most often.
        .show_menu_on_left_click(false)
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = windows::open_history(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

fn build_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let item = |id: &str, label: &str| MenuItemBuilder::with_id(id, label).build(app);

    MenuBuilder::new(app)
        .items(&[
            &item(id::REGION, "Screenshot Region")?,
            &item(id::WINDOW, "Screenshot Window")?,
            &item(id::FULLSCREEN, "Screenshot Fullscreen")?,
            &item(id::ALL_MONITORS, "Screenshot All Displays")?,
            &PredefinedMenuItem::separator(app)?,
            &item(id::RECORD_MP4, "Record MP4")?,
            &item(id::RECORD_GIF, "Record GIF")?,
            &item(id::STOP_RECORDING, "Stop Recording")?,
            &PredefinedMenuItem::separator(app)?,
            &item(id::HISTORY, "Recent Captures")?,
            &item(id::OPEN_FOLDER, "Open Capture Folder")?,
            &item(id::SETTINGS, "Settings")?,
            &PredefinedMenuItem::separator(app)?,
            &item(id::EXIT, "Exit")?,
        ])
        .build()
}

fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let state = app.state::<Arc<AppState>>().inner().clone();

    match event.id().as_ref() {
        id::REGION => actions::dispatch(state, Action::ScreenshotRegion),
        id::WINDOW => actions::dispatch(state, Action::ScreenshotWindow),
        id::FULLSCREEN => actions::dispatch(state, Action::ScreenshotFullscreen),
        id::ALL_MONITORS => actions::dispatch(state, Action::ScreenshotAllMonitors),
        id::RECORD_MP4 => actions::dispatch(state, Action::RecordMp4),
        id::RECORD_GIF => actions::dispatch(state, Action::RecordGif),
        id::STOP_RECORDING => actions::dispatch(state, Action::StopRecording),
        id::HISTORY => {
            if let Err(err) = windows::open_history(app) {
                tracing::error!(%err, "could not open the history window");
            }
        }
        id::SETTINGS => {
            if let Err(err) = windows::open_settings(app) {
                tracing::error!(%err, "could not open the settings window");
            }
        }
        id::OPEN_FOLDER => {
            if let Err(err) = windows::reveal_capture_folder(app, &state) {
                tracing::error!(%err, "could not open the capture folder");
            }
        }
        id::EXIT => {
            // Stop any recording first so its file is finalised and playable.
            actions::finish_recording(&state);
            app.exit(0);
        }
        other => tracing::debug!(id = other, "unhandled tray menu item"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_id_is_distinct() {
        let ids = [
            id::REGION,
            id::WINDOW,
            id::FULLSCREEN,
            id::ALL_MONITORS,
            id::RECORD_MP4,
            id::RECORD_GIF,
            id::STOP_RECORDING,
            id::HISTORY,
            id::OPEN_FOLDER,
            id::SETTINGS,
            id::EXIT,
        ];
        let mut sorted = ids.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "two menu items share an id");
    }

    #[test]
    fn the_capture_menu_ids_match_the_hotkey_ids() {
        // The tray and the hotkeys must dispatch the same actions; sharing the
        // id strings is what keeps them from drifting apart.
        assert_eq!(
            Action::from_hotkey_id(id::RECORD_MP4),
            Some(Action::RecordMp4)
        );
        assert_eq!(
            Action::from_hotkey_id(id::RECORD_GIF),
            Some(Action::RecordGif)
        );
        assert_eq!(
            Action::from_hotkey_id(id::STOP_RECORDING),
            Some(Action::StopRecording)
        );
    }
}

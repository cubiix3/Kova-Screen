//! IPC commands for the Settings and Recent Captures windows.
//!
//! # The key never crosses this boundary
//!
//! The vgy.me user key can be *written* from the UI and its presence can be
//! *queried*, but there is deliberately no command that reads it back. Once
//! saved it lives only in Credential Manager and is read directly by the upload
//! thread, so it never enters the WebView, a devtools console, or an IPC log.
//!
//! # Paths are validated, not trusted
//!
//! Anything from the UI that becomes a filesystem path is checked against the
//! history database first, so a crafted IPC call cannot make the app open or
//! delete an arbitrary file.

use std::path::PathBuf;
use std::sync::Arc;

use kova_history::Entry;
use kova_screen_core::settings::Settings;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::actions::{self, Action};
use crate::app::AppState;

/// IPC errors are plain strings: the frontend shows them verbatim, so they must
/// already read as user-facing text.
type CmdResult<T> = std::result::Result<T, String>;

fn describe(error: kova_screen_core::Error) -> String {
    error.to_string()
}

/// Everything the settings window needs in one round trip.
#[derive(serde::Serialize)]
pub struct SettingsView {
    settings: Settings,
    /// Whether a vgy.me key is saved. Never the key itself.
    has_user_key: bool,
    /// Bindings that could not be registered, with a reason.
    hotkey_failures: Vec<HotkeyFailureView>,
    capture_dir: String,
    version: &'static str,
    /// True when the WebP encoder cannot honour the quality slider.
    webp_is_lossless: bool,
}

#[derive(serde::Serialize)]
pub struct HotkeyFailureView {
    action: String,
    binding: String,
    reason: String,
    taken_by_another_app: bool,
}

#[tauri::command]
pub fn get_settings(state: State<'_, Arc<AppState>>) -> CmdResult<SettingsView> {
    let settings = state.settings();
    Ok(SettingsView {
        capture_dir: settings
            .capture_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        settings,
        has_user_key: kova_platform::credentials::exists(kova_platform::credentials::VGY_TARGET),
        hotkey_failures: state
            .hotkey_failures()
            .into_iter()
            .map(|f| HotkeyFailureView {
                action: f.action,
                binding: f.binding,
                reason: f.reason,
                taken_by_another_app: f.taken_by_another_app,
            })
            .collect(),
        version: env!("CARGO_PKG_VERSION"),
        // Documented honestly rather than showing a slider that does nothing.
        webp_is_lossless: true,
    })
}

/// Saves settings and re-applies the side effects they control.
#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    settings: Settings,
) -> CmdResult<SettingsView> {
    // Reject a conflicting hotkey set before writing anything, so the user is
    // never left with a saved config that silently drops a binding.
    let conflicts = settings.hotkeys.conflicts();
    if let Some((a, b)) = conflicts.first() {
        return Err(format!(
            "{} and {} are bound to the same shortcut",
            humanise(a),
            humanise(b)
        ));
    }

    let previous = state.settings();
    state.save_settings(settings.clone()).map_err(describe)?;

    // Autostart is external state, so it is only touched when it changed.
    if previous.general.launch_with_windows != settings.general.launch_with_windows {
        match std::env::current_exe() {
            Ok(exe) => {
                if let Err(err) = kova_platform::autostart::set_enabled(
                    settings.general.launch_with_windows,
                    &exe,
                ) {
                    tracing::warn!(%err, "could not update autostart");
                    return Err(format!(
                        "Settings saved, but autostart could not be changed: {err}"
                    ));
                }
            }
            Err(err) => {
                return Err(format!(
                    "Settings saved, but autostart could not be changed: {err}"
                ));
            }
        }
    }

    // Re-register hotkeys so a changed binding takes effect immediately.
    if previous.hotkeys != settings.hotkeys {
        crate::hotkeys::reregister(&app, state.inner());
    }

    get_settings(state)
}

fn humanise(action: &str) -> String {
    match action {
        "region_screenshot" => "Region screenshot".into(),
        "fullscreen_screenshot" => "Fullscreen screenshot".into(),
        "window_screenshot" => "Window screenshot".into(),
        "record_mp4" => "Record MP4".into(),
        "record_gif" => "Record GIF".into(),
        "stop_recording" => "Stop recording".into(),
        other => other.replace('_', " "),
    }
}

/// Stores the vgy.me user key in Credential Manager.
///
/// An empty string clears it. There is no matching getter by design.
#[tauri::command]
pub fn set_user_key(key: String) -> CmdResult<bool> {
    kova_platform::credentials::store(kova_platform::credentials::VGY_TARGET, key.trim())
        .map_err(describe)?;
    Ok(kova_platform::credentials::exists(
        kova_platform::credentials::VGY_TARGET,
    ))
}

/// Reports what a "Test Connection" can honestly determine.
///
/// vgy.me exposes no endpoint that validates a key without uploading, and this
/// app will not upload a file the user did not ask for just to check a
/// credential. So this confirms the key is stored and says plainly that it is
/// verified on the next real upload.
#[tauri::command]
pub fn test_user_key() -> CmdResult<String> {
    if kova_platform::credentials::exists(kova_platform::credentials::VGY_TARGET) {
        Ok("Key saved. vgy.me has no endpoint to verify a key without uploading, so it is checked on your next upload.".into())
    } else {
        Ok("No key saved. Uploads will be anonymous.".into())
    }
}

/// Recent captures for the history window.
#[tauri::command]
pub fn get_history(state: State<'_, Arc<AppState>>, limit: Option<u32>) -> CmdResult<Vec<Entry>> {
    let Some(history) = state.history() else {
        return Err(state
            .history_error()
            .unwrap_or("History is unavailable in this session.")
            .to_string());
    };
    history
        .recent(limit.unwrap_or(200).min(1000))
        .map_err(describe)
}

/// Looks a capture up by id, which is the only way this module accepts a path.
fn entry_of(state: &State<'_, Arc<AppState>>, id: i64) -> CmdResult<Entry> {
    let history = state
        .history()
        .ok_or_else(|| "History is unavailable in this session.".to_string())?;
    history
        .get(id)
        .map_err(describe)?
        .ok_or_else(|| "That capture is no longer in the history.".to_string())
}

#[tauri::command]
pub fn open_capture(app: AppHandle, state: State<'_, Arc<AppState>>, id: i64) -> CmdResult<()> {
    let entry = entry_of(&state, id)?;
    if !entry.path.exists() {
        return Err("That file no longer exists.".into());
    }
    app.opener()
        .open_path(entry.path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn reveal_capture(app: AppHandle, state: State<'_, Arc<AppState>>, id: i64) -> CmdResult<()> {
    let entry = entry_of(&state, id)?;
    let dir = entry
        .path
        .parent()
        .ok_or_else(|| "That capture has no folder.".to_string())?;
    app.opener()
        .open_path(dir.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_capture_folder(app: AppHandle, state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    crate::windows::reveal_capture_folder(&app, state.inner()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn copy_capture_file(state: State<'_, Arc<AppState>>, id: i64) -> CmdResult<()> {
    let entry = entry_of(&state, id)?;
    if !entry.path.exists() {
        return Err("That file no longer exists.".into());
    }
    kova_platform::clipboard::set_file(&entry.path).map_err(describe)
}

#[tauri::command]
pub fn copy_capture_path(state: State<'_, Arc<AppState>>, id: i64) -> CmdResult<()> {
    let entry = entry_of(&state, id)?;
    kova_platform::clipboard::set_text(&entry.path.to_string_lossy()).map_err(describe)
}

#[tauri::command]
pub fn copy_capture_url(state: State<'_, Arc<AppState>>, id: i64) -> CmdResult<()> {
    let entry = entry_of(&state, id)?;
    let settings = state.settings();

    let url = match settings.upload.url_kind {
        kova_screen_core::settings::UrlKind::DirectImage => {
            entry.direct_url.clone().or(entry.page_url.clone())
        }
        kova_screen_core::settings::UrlKind::PageUrl => {
            entry.page_url.clone().or(entry.direct_url.clone())
        }
    }
    .ok_or_else(|| "That capture has not been uploaded.".to_string())?;

    kova_platform::clipboard::set_text(&url).map_err(describe)
}

/// Uploads a capture on demand. Blocking, so the frontend shows a spinner.
#[tauri::command]
pub fn upload_capture(state: State<'_, Arc<AppState>>, id: i64) -> CmdResult<String> {
    let entry = entry_of(&state, id)?;
    let kind = kova_upload::MediaKind::from_path(&entry.path)
        .ok_or_else(|| "That file type cannot be uploaded.".to_string())?;

    let settings = state.settings();
    let result =
        crate::upload::run(state.inner(), entry.path.clone(), kind, Some(id)).map_err(describe)?;

    Ok(result.url_for(settings.upload.url_kind).to_string())
}

/// Deletes the local file and forgets the row.
#[tauri::command]
pub fn delete_capture_file(state: State<'_, Arc<AppState>>, id: i64) -> CmdResult<()> {
    let entry = entry_of(&state, id)?;

    // A file that is already gone is the desired end state, not an error.
    if entry.path.exists() {
        std::fs::remove_file(&entry.path).map_err(|e| format!("Could not delete the file: {e}"))?;
    }
    if let Some(history) = state.history() {
        history.remove(id).map_err(describe)?;
    }
    Ok(())
}

/// Follows the provider deletion link to remove the online copy.
///
/// The link is read from the database and used immediately; it is never sent to
/// the frontend, so the UI can offer this action without ever holding the
/// capability itself.
#[tauri::command]
pub fn delete_capture_upload(state: State<'_, Arc<AppState>>, id: i64) -> CmdResult<()> {
    let entry = entry_of(&state, id)?;
    let delete_url = entry
        .delete_url
        .clone()
        .ok_or_else(|| "That capture has no deletion link.".to_string())?;

    kova_upload::vgy::visit_delete_url(&delete_url).map_err(describe)?;

    if let Some(history) = state.history() {
        history.clear_upload(id).map_err(describe)?;
    }
    Ok(())
}

/// Drops rows whose file the user removed outside the app.
#[tauri::command]
pub fn prune_missing(state: State<'_, Arc<AppState>>) -> CmdResult<u32> {
    let Some(history) = state.history() else {
        return Ok(0);
    };
    history.forget_missing_files().map_err(describe)
}

/// Sets the capture directory after checking it is usable.
#[tauri::command]
pub fn set_capture_dir(state: State<'_, Arc<AppState>>, dir: String) -> CmdResult<String> {
    let path = PathBuf::from(dir.trim());
    if path.as_os_str().is_empty() {
        return Err("Choose a folder for your captures.".into());
    }
    if !path.is_absolute() {
        return Err("The capture folder must be an absolute path.".into());
    }
    // Fail here rather than at the moment of the user next screenshot.
    kova_screen_core::paths::ensure_dir(&path).map_err(describe)?;

    let mut settings = state.settings();
    settings.storage.capture_dir = Some(path.clone());
    state.save_settings(settings).map_err(describe)?;

    Ok(path.to_string_lossy().to_string())
}

/// Triggers a capture from the UI, using the same path as the tray and hotkeys.
#[tauri::command]
pub fn run_action(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    action: String,
) -> CmdResult<()> {
    let action = match action.as_str() {
        "screenshot_region" => Action::ScreenshotRegion,
        "screenshot_fullscreen" => Action::ScreenshotFullscreen,
        "screenshot_all_monitors" => Action::ScreenshotAllMonitors,
        "screenshot_window" => Action::ScreenshotWindow,
        "record_mp4" => Action::RecordMp4,
        "record_gif" => Action::RecordGif,
        "stop_recording" => Action::StopRecording,
        other => return Err(format!("Unknown action: {other}")),
    };

    // Hide the window that triggered it, or it would appear in the screenshot.
    if matches!(
        action,
        Action::ScreenshotRegion | Action::ScreenshotFullscreen | Action::ScreenshotAllMonitors
    ) {
        for label in [crate::windows::SETTINGS, crate::windows::HISTORY] {
            if let Some(window) = app.get_webview_window(label) {
                let _ = window.hide();
            }
        }
    }

    actions::dispatch(state.inner().clone(), action);
    Ok(())
}

/// Whether a recording is currently running, for the UI to reflect.
#[tauri::command]
pub fn recording_status(state: State<'_, Arc<AppState>>) -> CmdResult<bool> {
    Ok(state.is_recording())
}

/// Registers every command with the Tauri builder.
///
/// Concrete in the runtime rather than generic: the commands take a plain
/// `AppHandle`, which is `AppHandle<Wry>`, so a generic runtime here would not
/// satisfy the argument bounds.
pub fn handlers() -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        get_settings,
        save_settings,
        set_user_key,
        test_user_key,
        get_history,
        open_capture,
        reveal_capture,
        open_capture_folder,
        copy_capture_file,
        copy_capture_path,
        copy_capture_url,
        upload_capture,
        delete_capture_file,
        delete_capture_upload,
        prune_missing,
        set_capture_dir,
        run_action,
        recording_status,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_command_returns_the_user_key() {
        // The security property this module exists to hold: the key can be
        // written and its presence queried, but never read back.
        //
        // Only the code above the test module is scanned, because this test
        // necessarily mentions the very name it is looking for.
        let source = include_str!("commands.rs");
        let commands = source
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("the test module marker");

        assert!(
            !commands.contains("credentials::load"),
            "a command reads the user key back into the frontend"
        );
        assert!(commands.contains("credentials::exists"));
        assert!(commands.contains("credentials::store"));
    }

    #[test]
    fn the_settings_view_carries_no_secret_field() {
        let view = SettingsView {
            settings: Settings::default(),
            has_user_key: true,
            hotkey_failures: Vec::new(),
            capture_dir: "C:\\captures".into(),
            version: "0.1.0",
            webp_is_lossless: true,
        };
        let json = serde_json::to_string(&view).unwrap().to_lowercase();
        assert!(json.contains("has_user_key"));
        assert!(!json.contains("userkey"));
        assert!(
            !json.contains("user_key\":\""),
            "a key value was serialised"
        );
    }

    #[test]
    fn hotkey_action_ids_get_readable_labels() {
        assert_eq!(humanise("region_screenshot"), "Region screenshot");
        assert_eq!(humanise("record_mp4"), "Record MP4");
        // An unknown id still reads as words rather than a raw identifier.
        assert_eq!(humanise("some_new_action"), "some new action");
    }
}

//! The actions the tray, the hotkeys and the UI all trigger.
//!
//! Every entry point here returns immediately and does its work on a worker
//! thread. That matters for two reasons:
//!
//! - hotkey callbacks arrive on the hotkey message-pump thread, and blocking it
//!   would stop every other hotkey from firing
//! - the region overlay runs its own message loop, so it cannot share a thread
//!   with anything that also needs to pump messages
//!
//! Because a recording outlives the call that started it, a watcher thread
//! polls the overlay and stops the recording when the user presses Stop.

use std::sync::Arc;
use std::time::Duration;

use kova_history::NewEntry;
use kova_screen_core::Result;
use kova_upload::MediaKind;

use crate::app::AppState;
use crate::notify;
use crate::pipeline::{self, ShotRequest};
use crate::recorder::{RecorderHandle, RecordingFormat, RecordingSummary, RecordingTarget};

/// Everything a user can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ScreenshotRegion,
    ScreenshotFullscreen,
    ScreenshotAllMonitors,
    ScreenshotWindow,
    RecordMp4,
    RecordGif,
    /// Stops whichever recording is running. A no-op if none is.
    StopRecording,
}

impl Action {
    /// Maps a hotkey action id to an [`Action`].
    pub fn from_hotkey_id(id: &str) -> Option<Self> {
        Some(match id {
            "region_screenshot" => Action::ScreenshotRegion,
            "fullscreen_screenshot" => Action::ScreenshotFullscreen,
            "window_screenshot" => Action::ScreenshotWindow,
            "record_mp4" => Action::RecordMp4,
            "record_gif" => Action::RecordGif,
            "stop_recording" => Action::StopRecording,
            _ => return None,
        })
    }
}

/// Runs `action` on a worker thread.
pub fn dispatch(state: Arc<AppState>, action: Action) {
    let spawned = std::thread::Builder::new()
        .name("kova-action".into())
        .spawn(move || run(&state, action));

    if let Err(err) = spawned {
        tracing::error!(%err, "could not start the action thread");
    }
}

fn run(state: &Arc<AppState>, action: Action) {
    // Ignore repeated capture hotkeys while a picker/startup is already active.
    // Stop remains available even while another action is finishing.
    static ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    struct ActiveGuard;
    impl Drop for ActiveGuard {
        fn drop(&mut self) {
            ACTIVE.store(false, std::sync::atomic::Ordering::Release);
        }
    }
    let _active = if matches!(action, Action::StopRecording) {
        None
    } else {
        if ACTIVE
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::Acquire,
                std::sync::atomic::Ordering::Relaxed,
            )
            .is_err()
        {
            return;
        }
        Some(ActiveGuard)
    };
    // A hotkey pressed during a recording should stop it rather than start a
    // second one, which would fight over the encoder and the overlay.
    if state.is_recording() && !matches!(action, Action::StopRecording) {
        tracing::info!(
            ?action,
            "a recording is already running; stopping it instead"
        );
        finish_recording(state);
        return;
    }

    let result = match action {
        Action::ScreenshotRegion => shot(state, ShotRequest::Region),
        Action::ScreenshotFullscreen => shot(state, ShotRequest::Fullscreen),
        Action::ScreenshotAllMonitors => shot(state, ShotRequest::AllMonitors),
        Action::ScreenshotWindow => shot(state, ShotRequest::ActiveWindow),
        Action::RecordMp4 => start_recording(state, RecordingFormat::Mp4),
        Action::RecordGif => start_recording(state, RecordingFormat::Gif),
        Action::StopRecording => {
            finish_recording(state);
            Ok(())
        }
    };

    if let Err(err) = result {
        tracing::error!(%err, ?action, "the action failed");
        notify::show_if_enabled(&state.settings(), "Kova Screen", &err.to_string());
    }
}

/// Takes a screenshot and reports it.
fn shot(state: &Arc<AppState>, request: ShotRequest) -> Result<()> {
    let outcome = pipeline::take_screenshot(state, request)?;
    notify::report_capture(&state.settings(), &outcome);
    Ok(())
}

/// Asks for a region and starts recording it.
pub fn start_recording(state: &Arc<AppState>, format: RecordingFormat) -> Result<()> {
    // Both MP4 and GIF record a region: it is the only target that needs no
    // extra picker, and it is what the product spec asks for.
    let Some(rect) = kova_platform::overlay::region::select_rect()? else {
        return Ok(()); // Cancelled.
    };

    let settings = state.settings();
    let path = pipeline::reserve_recording_path(&settings, format.extension())?;

    let handle = RecorderHandle::start(&settings, format, RecordingTarget::Region(rect), path)?;

    *state.recorder().lock() = Some(handle);
    watch_recording(Arc::clone(state));

    Ok(())
}

/// Polls the overlay until the user stops the recording or it ends itself.
fn watch_recording(state: Arc<AppState>) {
    let spawned = std::thread::Builder::new()
        .name("kova-recorder-watch".into())
        .spawn(move || {
            loop {
                // The lock is held only for the poll, never across the sleep,
                // so the UI can still read the recorder state.
                let should_stop = match state.recorder().lock().as_ref() {
                    Some(handle) => handle.poll_overlay(),
                    None => true, // Someone else already stopped it.
                };

                if should_stop {
                    finish_recording(&state);
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });

    if let Err(err) = spawned {
        tracing::error!(%err, "could not start the recording watcher");
    }
}

/// Stops the active recording, saves it and reports it.
pub fn finish_recording(state: &Arc<AppState>) {
    let Some(handle) = state.recorder().lock().take() else {
        return; // Nothing running.
    };

    let format = handle.format();
    match handle.stop() {
        Ok(summary) => report_recording(state, format, summary),
        Err(err) => {
            tracing::error!(%err, "the recording could not be finalised");
            notify::show_if_enabled(&state.settings(), "Recording failed", &err.to_string());
        }
    }
}

/// Records a finished recording in history, the clipboard and a toast.
fn report_recording(state: &Arc<AppState>, format: RecordingFormat, summary: RecordingSummary) {
    let settings = state.settings();
    let mut warnings = Vec::new();

    let history_id = state.history().and_then(|history| {
        let entry = NewEntry {
            path: summary.path.clone(),
            kind: format.capture_kind(),
            size_bytes: summary.size_bytes,
            width: 0,
            height: 0,
        };
        match history.insert(&entry) {
            Ok(id) => {
                if let Err(err) = history.prune(settings.storage.history_limit) {
                    tracing::warn!(%err, "could not prune the history");
                }
                Some(id)
            }
            Err(err) => {
                tracing::warn!(%err, "could not record the recording in history");
                None
            }
        }
    });

    if settings.upload.copy_recording_path
        && let Err(err) = kova_platform::clipboard::set_text(&summary.path.to_string_lossy())
    {
        warnings.push(format!("Could not copy the path: {err}"));
    }

    if summary.truncated {
        warnings.push("The GIF reached its size limit and was cut short.".into());
    }

    let auto_upload = match format {
        RecordingFormat::Mp4 => settings.upload.auto_upload_recordings,
        RecordingFormat::Gif => settings.upload.auto_upload_gifs,
    };
    if settings.upload.enabled && auto_upload {
        let kind = match format {
            RecordingFormat::Mp4 => MediaKind::Video,
            RecordingFormat::Gif => MediaKind::Gif,
        };
        crate::upload::spawn(Arc::clone(state), summary.path.clone(), kind, history_id);
    }

    if !settings.general.notifications {
        return;
    }

    let name = summary
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut body = format!(
        "{name}\n{} \u{00b7} {}",
        format_duration(summary.duration),
        kova_upload::provider::format_size(summary.size_bytes)
    );
    for warning in &warnings {
        body.push('\n');
        body.push_str(warning);
    }

    notify::show("Recording saved", &body);
}

/// `mm:ss`, or `h:mm:ss` past an hour.
fn format_duration(duration: Duration) -> String {
    let total = duration.as_secs();
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_hotkey_id_maps_to_an_action() {
        // Guards against a rename in settings silently unbinding a hotkey.
        let hotkeys = kova_screen_core::settings::HotkeySettings::default();
        for (id, _) in hotkeys.bindings() {
            assert!(
                Action::from_hotkey_id(id).is_some(),
                "the hotkey id `{id}` has no action"
            );
        }
    }

    #[test]
    fn hotkey_ids_map_to_the_action_they_name() {
        assert_eq!(
            Action::from_hotkey_id("region_screenshot"),
            Some(Action::ScreenshotRegion)
        );
        assert_eq!(
            Action::from_hotkey_id("fullscreen_screenshot"),
            Some(Action::ScreenshotFullscreen)
        );
        assert_eq!(
            Action::from_hotkey_id("window_screenshot"),
            Some(Action::ScreenshotWindow)
        );
        assert_eq!(
            Action::from_hotkey_id("record_mp4"),
            Some(Action::RecordMp4)
        );
        assert_eq!(
            Action::from_hotkey_id("record_gif"),
            Some(Action::RecordGif)
        );
        assert_eq!(
            Action::from_hotkey_id("stop_recording"),
            Some(Action::StopRecording)
        );
    }

    #[test]
    fn an_unknown_hotkey_id_is_ignored_rather_than_guessed() {
        assert_eq!(Action::from_hotkey_id("something_else"), None);
        assert_eq!(Action::from_hotkey_id(""), None);
    }

    #[test]
    fn durations_format_as_the_overlay_shows_them() {
        assert_eq!(format_duration(Duration::ZERO), "00:00");
        assert_eq!(format_duration(Duration::from_secs(14)), "00:14");
        assert_eq!(format_duration(Duration::from_secs(3661)), "1:01:01");
    }
}

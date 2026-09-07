//! The capture pipeline: the single path every screenshot takes.
//!
//! # Failure isolation
//!
//! This module exists to enforce one product rule: **a capture the user can see
//! is a capture that succeeded.** Only two steps can fail the whole operation,
//! because after them there is nothing to give the user:
//!
//! 1. grabbing the pixels
//! 2. encoding them
//!
//! Everything after that -- writing the file, the clipboard, the history row,
//! the upload -- is recorded as a *warning* attached to a successful outcome.
//! A clipboard that is locked by another app must never produce "Screenshot
//! failed" when the PNG is sitting in the capture folder.
//!
//! [`CaptureOutcome`] carries that distinction, and the notification layer
//! renders it as "Screenshot saved / Upload failed" rather than a single
//! misleading line.

use std::path::PathBuf;
use std::sync::Arc;

use kova_capture::{CaptureOptions, CaptureTarget};
use kova_encode::still::encode_still;
use kova_history::{CaptureKind, NewEntry};
use kova_screen_core::settings::{ImageFormat, Settings};
use kova_screen_core::{Bitmap, Error, Result, filename, paths};
use kova_upload::{MediaKind, UploadRequest};

use crate::app::AppState;

/// What the user asked to capture.
#[derive(Debug, Clone, Copy)]
pub enum ShotRequest {
    /// Drag a region on the frozen desktop.
    Region,
    /// The monitor under the cursor.
    Fullscreen,
    /// Every monitor, as one image.
    AllMonitors,
    /// The foreground window.
    ActiveWindow,
    /// A specific window, chosen in the picker.
    Window(kova_capture::window::WindowId),
}

/// The result of a capture, including everything that went wrong afterwards.
#[derive(Debug, Clone, Default)]
pub struct CaptureOutcome {
    /// Where the file landed. `None` only if saving failed.
    pub path: Option<PathBuf>,
    /// The image is on the clipboard.
    pub copied: bool,
    /// History row id, if one was recorded.
    pub history_id: Option<i64>,
    /// Non-fatal problems, in the order they happened.
    pub warnings: Vec<String>,
    /// Pixel extent of the capture.
    pub width: u32,
    pub height: u32,
    /// True when the user cancelled the region overlay. Not an error.
    pub cancelled: bool,
}

impl CaptureOutcome {
    /// A cancelled capture: no file, no error, nothing to report.
    pub fn cancelled() -> Self {
        Self {
            cancelled: true,
            ..Default::default()
        }
    }

    /// The headline for a notification.
    pub fn title(&self) -> &'static str {
        if self.path.is_some() {
            "Screenshot saved"
        } else if self.copied {
            "Screenshot copied"
        } else {
            "Screenshot captured"
        }
    }

    /// The body for a notification, or `None` when everything worked.
    pub fn detail(&self) -> Option<String> {
        if self.warnings.is_empty() {
            return None;
        }
        Some(self.warnings.join("\n"))
    }
}

/// Runs a still capture end to end.
///
/// Returns `Err` only when there is genuinely nothing to give the user.
pub fn take_screenshot(state: &Arc<AppState>, request: ShotRequest) -> Result<CaptureOutcome> {
    let settings = state.settings();

    // The region overlay returns the pixels it froze, so there is no second
    // capture and no chance of a mismatch with what the user selected.
    let bitmap = match request {
        ShotRequest::Region => match kova_platform::overlay::region::select()? {
            Some(selection) => selection.bitmap,
            None => return Ok(CaptureOutcome::cancelled()),
        },
        other => {
            let target = resolve_target(other)?;
            apply_delay(&settings);
            kova_capture::capture(
                target,
                CaptureOptions {
                    include_cursor: settings.capture.include_cursor,
                },
            )?
        }
    };

    finish_still(state, &settings, bitmap)
}

/// Everything after the pixels exist: encode, save, clipboard, history, upload.
fn finish_still(
    state: &Arc<AppState>,
    settings: &Settings,
    bitmap: Bitmap,
) -> Result<CaptureOutcome> {
    let format = settings.capture.format;

    // Encoding is the second and last step that can fail the whole capture.
    let encoded = encode_still(&bitmap, format, settings.capture.quality)?;

    let mut outcome = CaptureOutcome {
        width: bitmap.width(),
        height: bitmap.height(),
        ..Default::default()
    };

    // --- Save -------------------------------------------------------------
    match save_capture(settings, &encoded, format.extension()) {
        Ok(path) => outcome.path = Some(path),
        Err(err) => outcome
            .warnings
            .push(format!("Could not save the file: {err}")),
    }

    // --- Clipboard --------------------------------------------------------
    if settings.capture.copy_to_clipboard {
        // PNG is what non-Windows-native apps read from the clipboard. Reuse the
        // encoded bytes when the output format is already PNG, and skip the
        // extra copy otherwise rather than compressing the image twice.
        let png = if format == ImageFormat::Png {
            encoded.clone()
        } else {
            encode_still(&bitmap, ImageFormat::Png, 100).unwrap_or_default()
        };
        match kova_platform::clipboard::set_image(&bitmap, &png) {
            Ok(()) => outcome.copied = true,
            Err(err) => outcome
                .warnings
                .push(format!("Could not copy to the clipboard: {err}")),
        }
    }

    // --- History ----------------------------------------------------------
    if let (Some(history), Some(path)) = (state.history(), outcome.path.as_ref()) {
        let entry = NewEntry {
            path: path.clone(),
            kind: CaptureKind::Screenshot,
            size_bytes: encoded.len() as u64,
            width: outcome.width,
            height: outcome.height,
        };
        match history.insert(&entry) {
            Ok(id) => {
                outcome.history_id = Some(id);
                let limit = settings.storage.history_limit;
                if let Err(err) = history.prune(limit) {
                    tracing::warn!(%err, "could not prune the history");
                }
            }
            // Deliberately not a user-visible warning: history is an
            // afterthought and the capture itself is intact.
            Err(err) => tracing::warn!(%err, "could not record the capture in history"),
        }
    }

    // --- Upload -----------------------------------------------------------
    if settings.upload.enabled
        && settings.upload.auto_upload_screenshots
        && let Some(path) = outcome.path.clone()
    {
        crate::upload::spawn(
            Arc::clone(state),
            path,
            MediaKind::Screenshot,
            outcome.history_id,
        );
    }

    Ok(outcome)
}

/// Writes `data` into the capture directory under a collision-free name.
///
/// Uses `create_new`, so if another process wins the race between choosing the
/// name and creating the file, this fails loudly rather than overwriting a
/// capture the user already has.
pub fn save_capture(settings: &Settings, data: &[u8], extension: &str) -> Result<PathBuf> {
    use std::io::Write;

    let dir = settings.capture_dir()?;
    paths::ensure_dir(&dir)?;

    let stem = filename::expand_template(&settings.storage.filename_template, now());
    let path =
        filename::unique_path(&dir, &stem, extension, 999).ok_or_else(|| Error::Storage {
            path: dir.join(format!("{stem}.{extension}")),
            source: std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "every candidate filename is taken",
            ),
        })?;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|source| Error::Storage {
            path: path.clone(),
            source,
        })?;
    file.write_all(data).map_err(|source| Error::Storage {
        path: path.clone(),
        source,
    })?;
    file.flush().map_err(|source| Error::Storage {
        path: path.clone(),
        source,
    })?;

    Ok(path)
}

/// Reserves a collision-free path for a recording, which is written in place.
pub fn reserve_recording_path(settings: &Settings, extension: &str) -> Result<PathBuf> {
    let dir = settings.capture_dir()?;
    paths::ensure_dir(&dir)?;
    let stem = filename::expand_template(&settings.storage.filename_template, now());
    filename::unique_path(&dir, &stem, extension, 999).ok_or_else(|| Error::Storage {
        path: dir.join(format!("{stem}.{extension}")),
        source: std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "every candidate filename is taken",
        ),
    })
}

/// Turns a request into a concrete capture target.
fn resolve_target(request: ShotRequest) -> Result<CaptureTarget> {
    Ok(match request {
        ShotRequest::Fullscreen => {
            // The monitor under the cursor is the one the user is looking at.
            let monitor = kova_capture::monitor::from_cursor()
                .or_else(kova_capture::monitor::primary)
                .ok_or_else(|| Error::Capture("no display is connected".into()))?;
            CaptureTarget::Monitor(monitor.id)
        }
        ShotRequest::AllMonitors => CaptureTarget::AllMonitors,
        ShotRequest::ActiveWindow => {
            let window = kova_capture::window::foreground().ok_or_else(|| {
                Error::Capture("no window is in the foreground to capture".into())
            })?;
            CaptureTarget::Window(window.id)
        }
        ShotRequest::Window(id) => CaptureTarget::Window(id),
        // Handled before this point, because it needs the overlay.
        ShotRequest::Region => {
            return Err(Error::Capture("a region capture needs the overlay".into()));
        }
    })
}

/// Honours the configured shutter delay.
fn apply_delay(settings: &Settings) {
    if settings.capture.delay_ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(
            settings.capture.delay_ms as u64,
        ));
    }
}

/// Local wall-clock time, falling back to UTC if the offset is unavailable.
///
/// Filenames should read in the user local time; `local_offset_at` can fail in
/// a multi-threaded process, and a UTC timestamp is far better than no capture.
pub fn now() -> time::OffsetDateTime {
    time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc())
}

/// Builds an upload request from the current settings.
pub fn upload_request(settings: &Settings, path: PathBuf, kind: MediaKind) -> UploadRequest {
    UploadRequest {
        path,
        kind,
        // Read at send time from Credential Manager, never held in settings.
        user_key: kova_platform::credentials::load(kova_platform::credentials::VGY_TARGET)
            .ok()
            .flatten(),
        max_bytes: settings.upload.max_upload_mb as u64 * 1024 * 1024,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kova_screen_core::PixelFormat;

    fn settings_in(dir: &std::path::Path) -> Settings {
        let mut settings = Settings::default();
        settings.storage.capture_dir = Some(dir.to_path_buf());
        settings
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("kova-pipeline-tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_capture_is_written_with_the_templated_name() {
        let dir = temp_dir("save");
        let path = save_capture(&settings_in(&dir), b"payload", "png").unwrap();

        assert_eq!(path.parent().unwrap(), dir);
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("KovaScreen_"), "unexpected name: {name}");
        assert!(name.ends_with(".png"));
        assert_eq!(std::fs::read(&path).unwrap(), b"payload");
    }

    #[test]
    fn two_captures_in_the_same_second_do_not_collide() {
        // The template has one-second resolution, so this is the common case
        // for a user pressing the hotkey twice quickly.
        let dir = temp_dir("collide");
        let settings = settings_in(&dir);
        let first = save_capture(&settings, b"one", "png").unwrap();
        let second = save_capture(&settings, b"two", "png").unwrap();

        assert_ne!(first, second, "the second capture overwrote the first");
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert_eq!(std::fs::read(&second).unwrap(), b"two");
    }

    #[test]
    fn the_capture_directory_is_created_on_first_use() {
        let dir = temp_dir("create").join("nested").join("deeper");
        assert!(!dir.exists());
        let path = save_capture(&settings_in(&dir), b"x", "png").unwrap();
        assert!(path.exists());
    }

    #[test]
    fn a_traversing_template_cannot_escape_the_capture_directory() {
        let dir = temp_dir("traversal");
        let mut settings = settings_in(&dir);
        settings.storage.filename_template = "../../../evil".into();

        let path = save_capture(&settings, b"x", "png").unwrap();
        assert_eq!(
            path.parent().unwrap(),
            dir,
            "the template escaped to {}",
            path.display()
        );
    }

    #[test]
    fn a_reserved_device_name_template_still_produces_a_file() {
        let dir = temp_dir("reserved");
        let mut settings = settings_in(&dir);
        settings.storage.filename_template = "CON".into();
        let path = save_capture(&settings, b"x", "png").unwrap();
        assert!(path.exists(), "a reserved name was not rewritten");
    }

    #[test]
    fn recording_paths_are_reserved_without_creating_a_file() {
        let dir = temp_dir("reserve");
        let path = reserve_recording_path(&settings_in(&dir), "mp4").unwrap();
        assert!(path.to_str().unwrap().ends_with(".mp4"));
        // The encoder creates it; reserving must not.
        assert!(!path.exists());
    }

    #[test]
    fn reserved_recording_paths_are_unique() {
        let dir = temp_dir("reserve-unique");
        let settings = settings_in(&dir);
        let first = reserve_recording_path(&settings, "mp4").unwrap();
        // Simulate the encoder creating the file.
        std::fs::write(&first, b"").unwrap();
        let second = reserve_recording_path(&settings, "mp4").unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn a_cancelled_capture_is_not_an_error_and_reports_nothing() {
        let outcome = CaptureOutcome::cancelled();
        assert!(outcome.cancelled);
        assert!(outcome.path.is_none());
        assert!(outcome.warnings.is_empty());
        assert!(outcome.detail().is_none());
    }

    #[test]
    fn the_notification_title_reflects_what_actually_happened() {
        let saved = CaptureOutcome {
            path: Some(PathBuf::from("a.png")),
            ..Default::default()
        };
        assert_eq!(saved.title(), "Screenshot saved");

        // Saving failed but the clipboard worked: the user still has the image,
        // so the headline must not claim failure.
        let copied = CaptureOutcome {
            copied: true,
            ..Default::default()
        };
        assert_eq!(copied.title(), "Screenshot copied");
    }

    #[test]
    fn warnings_are_reported_beside_a_successful_capture() {
        let outcome = CaptureOutcome {
            path: Some(PathBuf::from("a.png")),
            warnings: vec!["Could not copy to the clipboard: busy".into()],
            ..Default::default()
        };
        // The headline stays positive; the detail carries the problem.
        assert_eq!(outcome.title(), "Screenshot saved");
        assert!(outcome.detail().unwrap().contains("clipboard"));
    }

    #[test]
    fn a_region_target_cannot_be_resolved_without_the_overlay() {
        // Guards against a refactor routing Region down the direct path, which
        // would capture the whole screen instead of asking the user.
        assert!(resolve_target(ShotRequest::Region).is_err());
    }

    #[test]
    fn fullscreen_resolves_to_a_real_monitor() {
        let target = resolve_target(ShotRequest::Fullscreen).unwrap();
        assert!(matches!(target, CaptureTarget::Monitor(_)));
        assert!(!target.bounds().unwrap().is_empty());
    }

    #[test]
    fn all_monitors_resolves_to_the_virtual_desktop() {
        let target = resolve_target(ShotRequest::AllMonitors).unwrap();
        let bounds = target.bounds().unwrap();
        assert_eq!(
            bounds,
            kova_capture::monitor::virtual_desktop_bounds().unwrap()
        );
    }

    #[test]
    fn the_upload_request_takes_its_limit_from_settings() {
        let mut settings = Settings::default();
        settings.upload.max_upload_mb = 8;
        let request = upload_request(&settings, PathBuf::from("a.png"), MediaKind::Screenshot);
        assert_eq!(request.max_bytes, 8 * 1024 * 1024);
        assert_eq!(request.kind, MediaKind::Screenshot);
    }

    #[test]
    fn encoding_produces_bytes_for_every_configured_format() {
        let bitmap =
            Bitmap::from_raw(2, 2, PixelFormat::Bgra8, [10, 20, 30, 255].repeat(4)).unwrap();
        for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::Webp] {
            let bytes = encode_still(&bitmap, format, 90).unwrap();
            assert!(!bytes.is_empty(), "{format:?} produced nothing");
        }
    }
}

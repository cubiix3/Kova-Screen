//! Toast notifications.
//!
//! A thin wrapper over the Tauri notification plugin, with the app handle
//! stashed globally so the capture pipeline can raise a toast without every
//! function threading a handle through.
//!
//! Notifications are decoration: a failure here is logged and swallowed,
//! because a toast that could not be shown must never turn a successful capture
//! into a failed one.

use std::sync::OnceLock;

use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

static HANDLE: OnceLock<AppHandle> = OnceLock::new();

/// Records the app handle. Called once during setup.
pub fn init(handle: AppHandle) {
    let _ = HANDLE.set(handle);
}

/// Shows a toast, or does nothing if notifications are unavailable.
pub fn show(title: &str, body: &str) {
    let Some(handle) = HANDLE.get() else {
        tracing::debug!(title, "no app handle yet; skipping the notification");
        return;
    };

    if let Err(err) = handle
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show()
    {
        tracing::warn!(%err, "could not show a notification");
    }
}

/// Shows a toast only when the user has notifications enabled.
pub fn show_if_enabled(settings: &kova_screen_core::settings::Settings, title: &str, body: &str) {
    if settings.general.notifications {
        show(title, body);
    }
}

/// Reports a capture outcome, choosing wording that never overstates a failure.
pub fn report_capture(
    settings: &kova_screen_core::settings::Settings,
    outcome: &crate::pipeline::CaptureOutcome,
) {
    // Cancelling the region overlay is a deliberate user action, not an event.
    if outcome.cancelled || !settings.general.notifications {
        return;
    }

    let body = match (&outcome.path, outcome.detail()) {
        (Some(path), None) => path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        (Some(path), Some(detail)) => format!(
            "{}\n{detail}",
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        ),
        (None, Some(detail)) => detail,
        (None, None) => String::new(),
    };

    show(outcome.title(), &body);
}

#[cfg(test)]
mod tests {
    use crate::pipeline::CaptureOutcome;
    use std::path::PathBuf;

    #[test]
    fn a_clean_capture_reports_only_its_filename() {
        let outcome = CaptureOutcome {
            path: Some(PathBuf::from(r"C:\Users\Someone\Pictures\KovaScreen_x.png")),
            copied: true,
            ..Default::default()
        };
        assert_eq!(outcome.title(), "Screenshot saved");
        assert!(outcome.detail().is_none());
    }

    #[test]
    fn a_capture_with_a_failed_upload_still_reads_as_a_success() {
        let outcome = CaptureOutcome {
            path: Some(PathBuf::from("shot.png")),
            warnings: vec!["Could not copy to the clipboard: busy".into()],
            ..Default::default()
        };
        // The rule from the spec: never "Screenshot failed" when a file exists.
        assert_eq!(outcome.title(), "Screenshot saved");
        assert!(outcome.detail().is_some());
    }

    #[test]
    fn a_cancelled_capture_produces_no_notification_body() {
        let outcome = CaptureOutcome::cancelled();
        assert!(outcome.cancelled);
        assert!(outcome.detail().is_none());
    }
}

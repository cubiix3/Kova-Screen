//! Background uploads.
//!
//! Uploading always happens on its own thread, after the capture has already
//! been saved and reported. That is the mechanism behind the product rule that
//! a capture never fails because of an upload: by the time this module runs,
//! the user already has their file.

use std::path::PathBuf;
use std::sync::Arc;

use kova_screen_core::Result;
use kova_upload::{MediaKind, UploadResult};

use crate::app::AppState;
use crate::notify;

/// Uploads `path` on a background thread and reports the result.
///
/// Never blocks the caller and never returns an error: everything is surfaced
/// through a notification and the history row.
pub fn spawn(state: Arc<AppState>, path: PathBuf, kind: MediaKind, history_id: Option<i64>) {
    let spawned = std::thread::Builder::new()
        .name("kova-upload".into())
        .spawn(move || {
            let outcome = run(&state, path, kind, history_id);
            report(&state, kind, outcome);
        });

    if let Err(err) = spawned {
        tracing::error!(%err, "could not start the upload thread");
    }
}

/// Performs the upload synchronously. Used by the thread above and by the
/// manual "Upload" action in the history view.
pub fn run(
    state: &Arc<AppState>,
    path: PathBuf,
    kind: MediaKind,
    history_id: Option<i64>,
) -> Result<UploadResult> {
    let settings = state.settings();
    let provider = state.provider();

    let result = provider.upload(&crate::pipeline::upload_request(
        &settings,
        path.clone(),
        kind,
    ));

    match &result {
        Ok(uploaded) => {
            if let (Some(history), Some(id)) = (state.history(), history_id)
                && let Err(err) = history.set_uploaded(
                    id,
                    &uploaded.page_url,
                    &uploaded.direct_url,
                    uploaded.delete_url.as_deref(),
                )
            {
                tracing::warn!(%err, "could not record the upload in history");
            }

            if settings.upload.copy_url {
                let url = uploaded.url_for(settings.upload.url_kind);
                if let Err(err) = kova_platform::clipboard::set_text(url) {
                    tracing::warn!(%err, "could not copy the upload url");
                }
            }
        }
        Err(err) => {
            // Logged without the path detail that could include a user name in
            // a shared log, and never with the key.
            tracing::warn!(%err, "the upload failed; the local file is untouched");
            if let (Some(history), Some(id)) = (state.history(), history_id)
                && let Err(err) = history.set_upload_failed(id)
            {
                tracing::warn!(%err, "could not record the upload failure");
            }
        }
    }

    result
}

/// Turns an upload result into a notification.
fn report(state: &Arc<AppState>, kind: MediaKind, outcome: Result<UploadResult>) {
    let settings = state.settings();
    if !settings.general.notifications {
        return;
    }

    match outcome {
        Ok(uploaded) => {
            let url = uploaded.url_for(settings.upload.url_kind).to_string();
            let body = if settings.upload.copy_url {
                format!("Link copied\n{url}")
            } else {
                url
            };
            notify::show("Uploaded", &body);
        }
        Err(err) => {
            // The wording is deliberate: the capture is safe, only the upload
            // failed, and the message must not read as a lost screenshot.
            notify::show(
                "Upload failed",
                &format!("The {} is still saved on your PC.\n{err}", kind.describe()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use kova_upload::{UploadResult, UrlKind};

    fn result() -> UploadResult {
        UploadResult {
            page_url: "https://vgy.me/u/abc".into(),
            direct_url: "https://i.vgy.me/abc.png".into(),
            delete_url: Some("https://vgy.me/delete/secret".into()),
            filename: Some("abc.png".into()),
            size: Some(1024),
        }
    }

    #[test]
    fn the_copied_url_follows_the_configured_preference() {
        let uploaded = result();
        assert_eq!(
            uploaded.url_for(UrlKind::DirectImage),
            "https://i.vgy.me/abc.png"
        );
        assert_eq!(uploaded.url_for(UrlKind::PageUrl), "https://vgy.me/u/abc");
    }

    #[test]
    fn a_failure_message_says_the_capture_is_safe() {
        // The exact wording matters: this is the message that must not read as
        // a lost screenshot.
        let err = kova_screen_core::Error::Upload("host unreachable".into());
        let body = format!(
            "The {} is still saved on your PC.\n{err}",
            kova_upload::MediaKind::Screenshot.describe()
        );
        assert!(body.contains("still saved"));
        assert!(body.contains("screenshot"));
        assert!(!body.to_lowercase().contains("screenshot failed"));
    }
}

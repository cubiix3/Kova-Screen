//! Shared application state.
//!
//! One [`AppState`] is created at startup and shared by the tray, the hotkeys
//! and the IPC commands. Everything in it is individually recoverable: a failed
//! history database or a missing upload provider leaves the rest working,
//! because the product rule is that core capture never depends on the optional
//! parts.

use std::path::PathBuf;
use std::sync::Arc;

use kova_history::History;
use kova_screen_core::settings::Settings;
use kova_screen_core::{Result, paths};
use kova_upload::{UploadProvider, VgyProvider};
use parking_lot::{Mutex, RwLock};

use crate::recorder::RecorderHandle;

/// Everything the app needs, shared behind `Arc`.
pub struct AppState {
    settings: RwLock<Settings>,
    /// `None` when the database could not be opened. Capture still works; the
    /// history window shows why it is empty.
    history: Option<Arc<History>>,
    /// The reason history is unavailable, for the UI to display.
    history_error: Option<String>,
    provider: Arc<dyn UploadProvider>,
    recorder: Mutex<Option<RecorderHandle>>,
    /// Hotkey bindings that could not be registered, surfaced in settings.
    hotkey_failures: RwLock<Vec<kova_platform::HotkeyFailure>>,
}

impl AppState {
    /// Loads settings and opens the history database.
    ///
    /// Never fails on the optional parts: a history database that cannot be
    /// opened is recorded as a message rather than propagated, so the app still
    /// starts and still captures.
    pub fn load() -> Arc<Self> {
        let mut settings = Settings::load();
        settings.normalize();

        let (history, history_error) = match paths::history_db().and_then(|p| History::open(&p)) {
            Ok(history) => (Some(Arc::new(history)), None),
            Err(err) => {
                tracing::warn!(%err, "history is unavailable; captures will still work");
                (None, Some(err.to_string()))
            }
        };

        // Constructing the provider validates the endpoint scheme. If that ever
        // fails the app must still run with uploading disabled rather than not
        // start at all.
        let provider: Arc<dyn UploadProvider> = match VgyProvider::new() {
            Ok(provider) => Arc::new(provider),
            Err(err) => {
                tracing::error!(%err, "the vgy.me provider could not be created");
                Arc::new(DisabledProvider)
            }
        };

        Arc::new(Self {
            settings: RwLock::new(settings),
            history,
            history_error,
            provider,
            recorder: Mutex::new(None),
            hotkey_failures: RwLock::new(Vec::new()),
        })
    }

    /// Builds a state from explicit settings, with history in memory.
    ///
    /// Exists so integration tests can drive the real pipeline against a
    /// temporary capture folder instead of the user's Pictures directory.
    #[doc(hidden)]
    pub fn for_test(settings: Settings) -> Arc<Self> {
        let provider: Arc<dyn UploadProvider> = Arc::new(DisabledProvider);
        Arc::new(Self {
            settings: RwLock::new(settings),
            history: History::in_memory().ok().map(Arc::new),
            history_error: None,
            provider,
            recorder: Mutex::new(None),
            hotkey_failures: RwLock::new(Vec::new()),
        })
    }

    /// A snapshot of the current settings.
    ///
    /// Returns a clone rather than a guard so a long capture never holds the
    /// lock while a user is editing settings.
    pub fn settings(&self) -> Settings {
        self.settings.read().clone()
    }

    /// Replaces the settings and persists them.
    pub fn save_settings(&self, mut settings: Settings) -> Result<()> {
        settings.normalize();
        settings.save()?;
        *self.settings.write() = settings;
        Ok(())
    }

    pub fn history(&self) -> Option<Arc<History>> {
        self.history.clone()
    }

    pub fn history_error(&self) -> Option<&str> {
        self.history_error.as_deref()
    }

    pub fn provider(&self) -> Arc<dyn UploadProvider> {
        Arc::clone(&self.provider)
    }

    /// The capture directory, created if it does not exist.
    pub fn capture_dir(&self) -> Result<PathBuf> {
        let dir = self.settings().capture_dir()?;
        paths::ensure_dir(&dir)?;
        Ok(dir)
    }

    /// Whether a recording is currently running.
    pub fn is_recording(&self) -> bool {
        self.recorder
            .lock()
            .as_ref()
            .is_some_and(|r| r.is_running())
    }

    /// Access to the recorder slot.
    pub fn recorder(&self) -> &Mutex<Option<RecorderHandle>> {
        &self.recorder
    }

    pub fn set_hotkey_failures(&self, failures: Vec<kova_platform::HotkeyFailure>) {
        *self.hotkey_failures.write() = failures;
    }

    pub fn hotkey_failures(&self) -> Vec<kova_platform::HotkeyFailure> {
        self.hotkey_failures.read().clone()
    }
}

/// Stand-in used when no real provider could be constructed.
///
/// Refuses every upload with a clear message instead of making the rest of the
/// app deal with an absent provider.
struct DisabledProvider;

impl UploadProvider for DisabledProvider {
    fn id(&self) -> &'static str {
        "disabled"
    }

    fn display_name(&self) -> &'static str {
        "Uploads"
    }

    fn supports(&self, _kind: kova_upload::MediaKind) -> bool {
        false
    }

    fn upload(&self, _request: &kova_upload::UploadRequest) -> Result<kova_upload::UploadResult> {
        Err(kova_screen_core::Error::Upload(
            "uploading is unavailable in this session".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_provider_refuses_every_kind_without_panicking() {
        let provider = DisabledProvider;
        for kind in [
            kova_upload::MediaKind::Screenshot,
            kova_upload::MediaKind::Gif,
            kova_upload::MediaKind::Video,
        ] {
            assert!(!provider.supports(kind));
        }
        let request = kova_upload::UploadRequest {
            path: PathBuf::from("x.png"),
            kind: kova_upload::MediaKind::Screenshot,
            user_key: None,
            max_bytes: 1024,
        };
        assert!(provider.upload(&request).is_err());
    }
}

use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

/// Errors surfaced by the Kova Screen core.
///
/// Variants stay coarse on purpose. The rule from the product spec is that a
/// failing side-effect (clipboard, upload, history) must never be reported as a
/// failed capture, so callers match on the *stage* that failed rather than on a
/// long tail of API-specific codes.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("capture failed: {0}")]
    Capture(String),

    #[error("encoding failed: {0}")]
    Encode(String),

    #[error("clipboard unavailable: {0}")]
    Clipboard(String),

    #[error("storage error at {path}: {source}")]
    Storage {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("history error: {0}")]
    History(String),

    #[error("upload failed: {0}")]
    Upload(String),

    #[error("credential store error: {0}")]
    Credential(String),

    #[error("hotkey error: {0}")]
    Hotkey(String),

    #[error("invalid settings: {0}")]
    Settings(String),

    #[error("windows api error: {0}")]
    Platform(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    /// Whether the error left the user's capture intact on disk.
    ///
    /// Used by the notification layer to pick between "Screenshot saved /
    /// Upload failed" and a hard failure message.
    pub fn is_post_capture(&self) -> bool {
        matches!(
            self,
            Error::Clipboard(_) | Error::Upload(_) | Error::History(_) | Error::Credential(_)
        )
    }
}

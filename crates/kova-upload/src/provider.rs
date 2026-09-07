//! The provider abstraction.

use std::path::{Path, PathBuf};

use kova_screen_core::{Error, Result};

pub use kova_screen_core::settings::UrlKind;

/// What kind of artefact is being uploaded.
///
/// Providers advertise which kinds they accept, so the app can refuse an
/// unsupported upload *before* spending the user bandwidth on it and can say
/// why in a message that names the format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Screenshot,
    Gif,
    Video,
}

impl MediaKind {
    /// Infers the kind from a file extension.
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "png" | "jpg" | "jpeg" | "webp" | "bmp" => MediaKind::Screenshot,
            "gif" => MediaKind::Gif,
            "mp4" | "webm" | "mkv" | "mov" => MediaKind::Video,
            _ => return None,
        })
    }

    pub fn describe(self) -> &'static str {
        match self {
            MediaKind::Screenshot => "screenshot",
            MediaKind::Gif => "GIF",
            MediaKind::Video => "recording",
        }
    }
}

/// One upload.
#[derive(Debug, Clone)]
pub struct UploadRequest {
    /// The file to send. Read at upload time, not held in memory beforehand.
    pub path: PathBuf,
    pub kind: MediaKind,
    /// Optional provider account key. Never logged.
    pub user_key: Option<String>,
    /// Refuse anything larger than this many bytes.
    pub max_bytes: u64,
}

impl UploadRequest {
    /// Validates the request against the local file before any network call.
    ///
    /// Catching a missing or oversized file here means the user gets an instant,
    /// specific message instead of waiting for a server rejection.
    pub fn validate(&self) -> Result<u64> {
        let metadata = std::fs::metadata(&self.path).map_err(|source| Error::Storage {
            path: self.path.clone(),
            source,
        })?;

        if !metadata.is_file() {
            return Err(Error::Upload(format!(
                "{} is not a file",
                self.path.display()
            )));
        }

        let size = metadata.len();
        if size == 0 {
            return Err(Error::Upload("the file is empty".into()));
        }
        if size > self.max_bytes {
            return Err(Error::Upload(format!(
                "the file is {} but the upload limit is {}",
                format_size(size),
                format_size(self.max_bytes)
            )));
        }
        Ok(size)
    }

    /// The file name sent in the multipart part.
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("capture")
            .to_string()
    }
}

/// A successful upload.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UploadResult {
    /// The human-facing page, e.g. `https://vgy.me/u/abc123`.
    pub page_url: String,
    /// The direct media link, e.g. `https://i.vgy.me/abc123.png`.
    pub direct_url: String,
    /// Provider-side deletion link.
    ///
    /// Stored with the capture so the user can revoke an upload later. Treated
    /// as sensitive: it is never logged and never rendered into a shareable
    /// string, because anyone holding it can delete the upload.
    pub delete_url: Option<String>,
    /// Provider-side identifier.
    pub filename: Option<String>,
    /// Size the provider reported, in bytes.
    pub size: Option<u64>,
}

impl UploadResult {
    /// The URL to place on the clipboard for the configured preference.
    ///
    /// Falls back to whichever URL exists if the preferred one is missing, so a
    /// provider that omits a field still yields something usable.
    pub fn url_for(&self, kind: UrlKind) -> &str {
        match kind {
            UrlKind::DirectImage => {
                if self.direct_url.is_empty() {
                    &self.page_url
                } else {
                    &self.direct_url
                }
            }
            UrlKind::PageUrl => {
                if self.page_url.is_empty() {
                    &self.direct_url
                } else {
                    &self.page_url
                }
            }
        }
    }
}

/// A destination Kova Screen can upload to.
pub trait UploadProvider: Send + Sync {
    /// Stable identifier, stored with history rows.
    fn id(&self) -> &'static str;

    /// Name shown in settings.
    fn display_name(&self) -> &'static str;

    /// Whether this provider accepts `kind`.
    fn supports(&self, kind: MediaKind) -> bool;

    /// Performs the upload. Blocking; callers run it off the UI thread.
    fn upload(&self, request: &UploadRequest) -> Result<UploadResult>;

    /// A message explaining why `kind` is not supported.
    fn unsupported_message(&self, kind: MediaKind) -> String {
        format!(
            "{} does not accept {} uploads",
            self.display_name(),
            kind.describe()
        )
    }
}

/// Formats a byte count for a user-facing message.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join("kova-upload-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn request(path: PathBuf, max_bytes: u64) -> UploadRequest {
        UploadRequest {
            kind: MediaKind::from_path(&path).unwrap_or(MediaKind::Screenshot),
            path,
            user_key: None,
            max_bytes,
        }
    }

    #[test]
    fn media_kind_is_inferred_from_the_extension() {
        assert_eq!(
            MediaKind::from_path(Path::new("a.png")),
            Some(MediaKind::Screenshot)
        );
        assert_eq!(
            MediaKind::from_path(Path::new("a.JPG")),
            Some(MediaKind::Screenshot)
        );
        assert_eq!(
            MediaKind::from_path(Path::new("a.webp")),
            Some(MediaKind::Screenshot)
        );
        assert_eq!(
            MediaKind::from_path(Path::new("a.gif")),
            Some(MediaKind::Gif)
        );
        assert_eq!(
            MediaKind::from_path(Path::new("a.mp4")),
            Some(MediaKind::Video)
        );
        assert_eq!(MediaKind::from_path(Path::new("a.txt")), None);
        assert_eq!(MediaKind::from_path(Path::new("noextension")), None);
    }

    #[test]
    fn validation_accepts_a_normal_file_and_reports_its_size() {
        let path = temp_file("ok.png", &vec![0u8; 1234]);
        assert_eq!(request(path, 10_000).validate().unwrap(), 1234);
    }

    #[test]
    fn validation_rejects_a_missing_file() {
        let path = std::env::temp_dir()
            .join("kova-upload-tests")
            .join("nope.png");
        let _ = std::fs::remove_file(&path);
        assert!(request(path, 10_000).validate().is_err());
    }

    #[test]
    fn validation_rejects_an_empty_file() {
        let path = temp_file("empty.png", b"");
        let err = request(path, 10_000).validate().unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn validation_rejects_an_oversized_file_with_readable_sizes() {
        let path = temp_file("big.png", &vec![0u8; 5000]);
        let err = request(path, 1024).validate().unwrap_err().to_string();
        // The message must name both numbers so the user can act on it.
        assert!(err.contains("4.9 KB"), "unhelpful message: {err}");
        assert!(err.contains("1.0 KB"), "unhelpful message: {err}");
    }

    #[test]
    fn validation_rejects_a_directory() {
        let dir = std::env::temp_dir().join("kova-upload-tests");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(request(dir, 10_000).validate().is_err());
    }

    #[test]
    fn the_file_name_falls_back_when_the_path_has_none() {
        let req = request(PathBuf::from("shot.png"), 100);
        assert_eq!(req.file_name(), "shot.png");
        let req = request(PathBuf::from("/"), 100);
        assert_eq!(req.file_name(), "capture");
    }

    #[test]
    fn the_clipboard_url_follows_the_configured_preference() {
        let result = UploadResult {
            page_url: "https://vgy.me/u/abc123".into(),
            direct_url: "https://i.vgy.me/abc123.png".into(),
            delete_url: None,
            filename: None,
            size: None,
        };
        assert_eq!(
            result.url_for(UrlKind::DirectImage),
            "https://i.vgy.me/abc123.png"
        );
        assert_eq!(result.url_for(UrlKind::PageUrl), "https://vgy.me/u/abc123");
    }

    #[test]
    fn a_missing_preferred_url_falls_back_to_the_other() {
        let only_page = UploadResult {
            page_url: "https://vgy.me/u/abc123".into(),
            direct_url: String::new(),
            delete_url: None,
            filename: None,
            size: None,
        };
        // Rather than putting an empty string on the clipboard.
        assert_eq!(
            only_page.url_for(UrlKind::DirectImage),
            "https://vgy.me/u/abc123"
        );

        let only_direct = UploadResult {
            page_url: String::new(),
            direct_url: "https://i.vgy.me/abc123.png".into(),
            delete_url: None,
            filename: None,
            size: None,
        };
        assert_eq!(
            only_direct.url_for(UrlKind::PageUrl),
            "https://i.vgy.me/abc123.png"
        );
    }

    #[test]
    fn sizes_format_readably() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }
}

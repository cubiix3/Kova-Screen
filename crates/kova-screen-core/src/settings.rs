//! User settings.
//!
//! Two rules shape this module:
//!
//! 1. **No secrets.** The vgy.me user key is deliberately absent from every
//!    struct here. It lives in Windows Credential Manager and is fetched on
//!    demand by the upload layer, so it can never reach `settings.json`, a log
//!    line or a crash report.
//! 2. **Forward and backward compatible.** Every field carries a `#[serde(default)]`
//!    so a config written by an older or newer build still loads. A settings
//!    file that fails to parse is replaced by defaults rather than blocking
//!    startup -- the user can always still take a screenshot.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Error, Result, filename, paths};

/// Output format for still screenshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    #[default]
    Png,
    Jpeg,
    Webp,
}

impl ImageFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Webp => "webp",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Webp => "image/webp",
        }
    }

    /// Whether the format can represent transparency.
    ///
    /// JPEG cannot, so the encoder flattens alpha onto an opaque background
    /// instead of producing the black fringes a naive drop would give.
    pub fn supports_alpha(self) -> bool {
        !matches!(self, ImageFormat::Jpeg)
    }
}

/// Which vgy.me URL to place on the clipboard after an upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum UrlKind {
    /// `https://i.vgy.me/abc123.png` -- embeds directly. The documented default.
    #[default]
    DirectImage,
    /// `https://vgy.me/u/abc123` -- the human-facing page.
    PageUrl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeneralSettings {
    pub launch_with_windows: bool,
    pub start_minimized: bool,
    pub notifications: bool,
    pub theme: Theme,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            launch_with_windows: false,
            start_minimized: true,
            notifications: true,
            theme: Theme::Dark,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CaptureSettings {
    pub format: ImageFormat,
    /// Quality for the lossy formats, 1-100. Ignored for PNG.
    pub quality: u8,
    pub copy_to_clipboard: bool,
    pub include_cursor: bool,
    /// Delay before the shutter fires, in milliseconds. 0 disables it.
    pub delay_ms: u32,
    /// Play the Windows shutter sound after a successful still capture.
    pub play_sound: bool,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self {
            format: ImageFormat::Png,
            quality: 90,
            copy_to_clipboard: true,
            include_cursor: false,
            delay_ms: 0,
            play_sound: false,
        }
    }
}

/// Recording quality preset, mapped to a bitrate by the encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RecordingQuality {
    Low,
    #[default]
    Medium,
    High,
}

impl RecordingQuality {
    /// Target H.264 bitrate in bits per second for the given frame extent.
    ///
    /// Scales with pixel count and frame rate rather than using fixed presets,
    /// so a small region recording does not get a 1080p bitrate (and a 4K one
    /// does not look like mush). Clamped to a range hardware encoders accept.
    pub fn bitrate_for(self, width: u32, height: u32, fps: u32) -> u32 {
        let bits_per_pixel = match self {
            RecordingQuality::Low => 0.05_f64,
            RecordingQuality::Medium => 0.10_f64,
            RecordingQuality::High => 0.20_f64,
        };
        let pixels = width as f64 * height as f64;
        let raw = pixels * fps.max(1) as f64 * bits_per_pixel;
        raw.clamp(500_000.0, 80_000_000.0) as u32
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecordingSettings {
    /// Frames per second for MP4 capture. Clamped to 10..=60 on load.
    pub mp4_fps: u32,
    /// Frames per second for GIF capture. Clamped to 5..=30 on load.
    pub gif_fps: u32,
    pub quality: RecordingQuality,
    pub include_cursor: bool,
    /// Prefer a hardware H.264 encoder when Media Foundation offers one.
    pub hardware_encoding: bool,
    /// Hard stop for a single recording, in seconds, so a forgotten recording
    /// cannot fill the disk. 0 disables the limit.
    pub max_duration_secs: u32,
    /// Largest GIF we will write, in megabytes. The encoder drops frame rate
    /// and then stops early rather than producing a 400 MB GIF.
    pub gif_max_size_mb: u32,
}

impl Default for RecordingSettings {
    fn default() -> Self {
        Self {
            mp4_fps: 30,
            gif_fps: 15,
            quality: RecordingQuality::Medium,
            include_cursor: true,
            hardware_encoding: false,
            max_duration_secs: 1800,
            gif_max_size_mb: 32,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UploadSettings {
    /// Master switch. When false, no upload code path runs at all.
    pub enabled: bool,
    pub auto_upload_screenshots: bool,
    pub auto_upload_gifs: bool,
    pub auto_upload_recordings: bool,
    pub url_kind: UrlKind,
    /// Copy the resulting URL to the clipboard, replacing the image.
    pub copy_url: bool,
    /// Copy a recording file path to the clipboard when it is saved.
    pub copy_recording_path: bool,
    /// Refuse to upload anything larger than this, in megabytes.
    pub max_upload_mb: u32,
}

impl Default for UploadSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_upload_screenshots: false,
            auto_upload_gifs: false,
            auto_upload_recordings: false,
            url_kind: UrlKind::DirectImage,
            copy_url: true,
            copy_recording_path: false,
            max_upload_mb: 64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StorageSettings {
    /// Empty means "use [`paths::default_capture_dir`]".
    pub capture_dir: Option<PathBuf>,
    pub filename_template: String,
    /// Number of history entries to retain. 0 means unlimited.
    pub history_limit: u32,
}

impl Default for StorageSettings {
    fn default() -> Self {
        Self {
            capture_dir: None,
            filename_template: filename::DEFAULT_TEMPLATE.to_string(),
            history_limit: 500,
        }
    }
}

/// A parsed global hotkey binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HotkeySettings {
    pub region_screenshot: String,
    pub fullscreen_screenshot: String,
    pub window_screenshot: String,
    pub record_mp4: String,
    pub record_gif: String,
    /// Stops whichever recording is running. Shares a default with nothing else.
    pub stop_recording: String,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        Self {
            region_screenshot: "PrintScreen".into(),
            fullscreen_screenshot: "Ctrl+PrintScreen".into(),
            window_screenshot: "Shift+PrintScreen".into(),
            record_mp4: "Ctrl+Shift+R".into(),
            record_gif: "Ctrl+Shift+G".into(),
            stop_recording: "Ctrl+Shift+S".into(),
        }
    }
}

impl HotkeySettings {
    /// All bindings paired with the action id used by the hotkey registry.
    pub fn bindings(&self) -> [(&'static str, &str); 6] {
        [
            ("region_screenshot", &self.region_screenshot),
            ("fullscreen_screenshot", &self.fullscreen_screenshot),
            ("window_screenshot", &self.window_screenshot),
            ("record_mp4", &self.record_mp4),
            ("record_gif", &self.record_gif),
            ("stop_recording", &self.stop_recording),
        ]
    }

    /// Returns pairs of action ids bound to the same combination.
    ///
    /// Reported to the user as a settings validation error instead of letting
    /// one binding silently win at registration time.
    pub fn conflicts(&self) -> Vec<(&'static str, &'static str)> {
        let bindings = self.bindings();
        let mut out = Vec::new();
        for i in 0..bindings.len() {
            for j in (i + 1)..bindings.len() {
                let (a_id, a) = bindings[i];
                let (b_id, b) = bindings[j];
                if a.trim().is_empty() || b.trim().is_empty() {
                    continue;
                }
                if a.eq_ignore_ascii_case(b.trim()) {
                    out.push((a_id, b_id));
                }
            }
        }
        out
    }
}

/// The complete settings document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub general: GeneralSettings,
    pub capture: CaptureSettings,
    pub recording: RecordingSettings,
    pub upload: UploadSettings,
    pub storage: StorageSettings,
    pub hotkeys: HotkeySettings,
}

impl Settings {
    /// Loads settings from `path`, falling back to defaults.
    ///
    /// A missing file is normal on first run. A corrupt file is logged and
    /// replaced by defaults: refusing to start because of a bad config would
    /// violate the rule that capture must keep working.
    pub fn load_from(path: &std::path::Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<Settings>(&text) {
                Ok(mut s) => {
                    s.normalize();
                    s
                }
                Err(err) => {
                    tracing::warn!(%err, "settings file is invalid, using defaults");
                    Settings::default()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Settings::default(),
            Err(err) => {
                tracing::warn!(%err, "settings file is unreadable, using defaults");
                Settings::default()
            }
        }
    }

    /// Loads from the standard location.
    pub fn load() -> Self {
        match paths::settings_file() {
            Ok(path) => Self::load_from(&path),
            Err(err) => {
                tracing::warn!(%err, "cannot resolve settings path, using defaults");
                Settings::default()
            }
        }
    }

    /// Writes settings atomically: a temporary file plus a rename.
    ///
    /// Prevents a crash mid-write from leaving a truncated config that would
    /// reset the user preferences on next start.
    pub fn save_to(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            paths::ensure_dir(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| Error::Settings(format!("cannot serialise settings: {e}")))?;

        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json.as_bytes()).map_err(|source| Error::Storage {
            path: tmp.clone(),
            source,
        })?;
        std::fs::rename(&tmp, path).map_err(|source| {
            let _ = std::fs::remove_file(&tmp);
            Error::Storage {
                path: path.to_path_buf(),
                source,
            }
        })
    }

    /// Writes to the standard location.
    pub fn save(&self) -> Result<()> {
        self.save_to(&paths::settings_file()?)
    }

    /// The effective capture directory.
    pub fn capture_dir(&self) -> Result<PathBuf> {
        match &self.storage.capture_dir {
            Some(dir) if !dir.as_os_str().is_empty() => Ok(dir.clone()),
            _ => paths::default_capture_dir(),
        }
    }

    /// Clamps every numeric field into a range the engines can honour.
    ///
    /// Called after deserialisation so a hand-edited config with `mp4_fps: 9999`
    /// degrades to something sane instead of wedging the encoder.
    pub fn normalize(&mut self) {
        self.capture.quality = self.capture.quality.clamp(1, 100);
        self.capture.delay_ms = self.capture.delay_ms.min(10_000);
        self.recording.mp4_fps = self.recording.mp4_fps.clamp(10, 60);
        self.recording.gif_fps = self.recording.gif_fps.clamp(5, 30);
        self.recording.max_duration_secs = self.recording.max_duration_secs.min(6 * 3600);
        self.recording.gif_max_size_mb = self.recording.gif_max_size_mb.clamp(1, 512);
        self.upload.max_upload_mb = self.upload.max_upload_mb.clamp(1, 1024);

        if self.storage.filename_template.trim().is_empty() {
            self.storage.filename_template = filename::DEFAULT_TEMPLATE.to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_product_spec() {
        let s = Settings::default();
        assert_eq!(s.capture.format, ImageFormat::Png);
        assert_eq!(s.recording.gif_fps, 15);
        assert_eq!(s.recording.mp4_fps, 30);
        assert_eq!(s.upload.url_kind, UrlKind::DirectImage);
        assert_eq!(s.general.theme, Theme::Dark);
        // Upload is opt-in: a fresh install must not send anything anywhere.
        assert!(!s.upload.enabled);
        assert!(!s.upload.auto_upload_screenshots);
    }

    #[test]
    fn round_trips_through_json() {
        let mut s = Settings::default();
        s.capture.format = ImageFormat::Webp;
        s.recording.gif_fps = 20;
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap(), s);
    }

    #[test]
    fn serialised_settings_never_contain_a_key_field() {
        // Regression guard: the vgy user key must not be reachable from here.
        let json = serde_json::to_string(&Settings::default())
            .unwrap()
            .to_lowercase();
        assert!(!json.contains("userkey"));
        assert!(!json.contains("user_key"));
        assert!(!json.contains("secret"));
        assert!(!json.contains("token"));
    }

    #[test]
    fn partial_config_fills_in_defaults() {
        let json = r#"{"capture":{"format":"jpeg"}}"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(s.capture.format, ImageFormat::Jpeg);
        // Untouched fields keep their defaults.
        assert_eq!(s.capture.quality, 90);
        assert_eq!(s.recording.gif_fps, 15);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults_instead_of_failing() {
        let path = std::env::temp_dir().join("kova-bad-settings.json");
        std::fs::write(&path, b"{ this is not json").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_yields_defaults() {
        let path = std::env::temp_dir().join("kova-nonexistent-settings.json");
        let _ = std::fs::remove_file(&path);
        assert_eq!(Settings::load_from(&path), Settings::default());
    }

    #[test]
    fn normalize_clamps_out_of_range_values() {
        let mut s = Settings::default();
        s.recording.mp4_fps = 9999;
        s.recording.gif_fps = 1;
        s.capture.quality = 0;
        s.upload.max_upload_mb = 99_999;
        s.storage.filename_template = "   ".into();
        s.normalize();
        assert_eq!(s.recording.mp4_fps, 60);
        assert_eq!(s.recording.gif_fps, 5);
        assert_eq!(s.capture.quality, 1);
        assert_eq!(s.upload.max_upload_mb, 1024);
        assert_eq!(s.storage.filename_template, filename::DEFAULT_TEMPLATE);
    }

    #[test]
    fn save_then_load_preserves_settings() {
        let path = std::env::temp_dir().join("kova-settings-roundtrip.json");
        let mut s = Settings::default();
        s.general.launch_with_windows = true;
        s.storage.filename_template = "cap_{date}".into();
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), s);
        // The temporary file must not survive a successful save.
        assert!(!path.with_extension("json.tmp").exists());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn default_hotkeys_do_not_conflict() {
        assert!(HotkeySettings::default().conflicts().is_empty());
    }

    #[test]
    fn duplicate_hotkeys_are_reported() {
        let hk = HotkeySettings {
            record_gif: "Ctrl+Shift+R".into(),
            ..Default::default()
        };
        let conflicts = hk.conflicts();
        assert_eq!(conflicts.len(), 1);
        assert!(conflicts[0].0 == "record_mp4" || conflicts[0].1 == "record_mp4");
    }

    #[test]
    fn empty_hotkeys_are_unbound_not_conflicting() {
        let hk = HotkeySettings {
            record_gif: String::new(),
            record_mp4: String::new(),
            ..Default::default()
        };
        assert!(hk.conflicts().is_empty());
    }

    #[test]
    fn bitrate_scales_with_resolution_and_frame_rate() {
        let q = RecordingQuality::Medium;
        let hd30 = q.bitrate_for(1920, 1080, 30);
        let hd60 = q.bitrate_for(1920, 1080, 60);
        let small = q.bitrate_for(640, 480, 30);
        assert!(hd60 > hd30, "60 fps must get more bits than 30 fps");
        assert!(small < hd30, "a small region must not get a 1080p bitrate");
        assert!(RecordingQuality::High.bitrate_for(1920, 1080, 30) > hd30);
    }

    #[test]
    fn bitrate_stays_within_encoder_limits() {
        // A tiny region and an 8K wall must both land in the accepted range.
        let tiny = RecordingQuality::Low.bitrate_for(16, 16, 10);
        let huge = RecordingQuality::High.bitrate_for(7680, 4320, 60);
        assert!((500_000..=80_000_000).contains(&tiny));
        assert!((500_000..=80_000_000).contains(&huge));
    }

    #[test]
    fn jpeg_is_the_only_format_without_alpha() {
        assert!(!ImageFormat::Jpeg.supports_alpha());
        assert!(ImageFormat::Png.supports_alpha());
        assert!(ImageFormat::Webp.supports_alpha());
    }
}

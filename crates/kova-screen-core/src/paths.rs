//! Well-known locations for captures, configuration and the history database.
//!
//! Resolution is env-var driven rather than hard-coded so the paths stay
//! correct on redirected profiles, and so tests can point the whole app at a
//! temporary directory.

use std::path::PathBuf;

use crate::{Error, Result};

/// Directory name used under both Pictures and AppData.
pub const APP_DIR_NAME: &str = "Kova Screen";

/// Default capture directory: `%USERPROFILE%\Pictures\Kova Screen`.
pub fn default_capture_dir() -> Result<PathBuf> {
    Ok(user_profile()?.join("Pictures").join(APP_DIR_NAME))
}

/// Per-user configuration directory: `%APPDATA%\Kova Screen`.
pub fn config_dir() -> Result<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| {
            user_profile()
                .ok()
                .map(|p| p.join("AppData").join("Roaming"))
        })
        .ok_or_else(|| Error::Settings("neither APPDATA nor USERPROFILE is set".into()))?;
    Ok(base.join(APP_DIR_NAME))
}

/// Path of the settings file.
pub fn settings_file() -> Result<PathBuf> {
    Ok(config_dir()?.join("settings.json"))
}

/// Path of the capture history database.
pub fn history_db() -> Result<PathBuf> {
    Ok(config_dir()?.join("history.db"))
}

/// Directory for transient recording scratch files.
///
/// Lives under `%LOCALAPPDATA%` rather than Roaming so partially written
/// recordings are never synced to a domain profile.
pub fn temp_dir() -> Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    Ok(base.join(APP_DIR_NAME).join("temp"))
}

fn user_profile() -> Result<PathBuf> {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .ok_or_else(|| Error::Settings("USERPROFILE is not set".into()))
}

/// Creates `dir` and all parents, mapping IO failures to a storage error that
/// names the offending path.
pub fn ensure_dir(dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|source| Error::Storage {
        path: dir.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_dir_sits_under_pictures() {
        // USERPROFILE is always set on Windows; skip elsewhere.
        let Ok(dir) = default_capture_dir() else {
            return;
        };
        assert!(dir.ends_with(PathBuf::from("Pictures").join(APP_DIR_NAME)));
    }

    #[test]
    fn config_paths_share_one_directory() {
        let Ok(cfg) = config_dir() else { return };
        assert_eq!(settings_file().unwrap().parent().unwrap(), cfg);
        assert_eq!(history_db().unwrap().parent().unwrap(), cfg);
    }

    #[test]
    fn ensure_dir_is_idempotent() {
        let dir = std::env::temp_dir().join("kova-ensure-dir-test");
        ensure_dir(&dir).unwrap();
        ensure_dir(&dir).unwrap();
        assert!(dir.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

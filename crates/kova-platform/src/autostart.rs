//! Launch-with-Windows via the per-user `Run` key.
//!
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` needs no elevation, is
//! trivially inspectable by the user, and is removed cleanly by the uninstaller
//! -- which a scheduled task or a Startup-folder shortcut is not.

use std::path::Path;

use kova_screen_core::{Error, Result};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ, RegCloseKey, RegDeleteValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows_core::HSTRING;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Value name under the `Run` key.
pub const VALUE_NAME: &str = "KovaScreen";

/// Owns an open registry key.
struct RegKey(HKEY);

impl RegKey {
    fn open(access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Result<Self> {
        let subkey = HSTRING::from(RUN_KEY);
        let mut key = HKEY::default();
        // SAFETY: `subkey` outlives the call; `key` is a live out-parameter.
        let status =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, &subkey, Some(0), access, &mut key) };
        status
            .ok()
            .map_err(|e| Error::Platform(format!("could not open the autostart key: {e}")))?;
        Ok(Self(key))
    }
}

impl Drop for RegKey {
    fn drop(&mut self) {
        // SAFETY: closes a key opened by RegOpenKeyExW exactly once.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

/// Enables or disables launching Kova Screen at sign-in.
///
/// `exe` should be the current executable path. It is quoted, so a path
/// containing spaces (the default `C:\Program Files\...`) still parses as one
/// argument when Windows launches it.
pub fn set_enabled(enabled: bool, exe: &Path) -> Result<()> {
    if enabled { enable(exe) } else { disable() }
}

fn enable(exe: &Path) -> Result<()> {
    if !exe.is_absolute() {
        return Err(Error::Platform(
            "autostart needs an absolute executable path".into(),
        ));
    }

    let key = RegKey::open(KEY_WRITE)?;
    let name = HSTRING::from(VALUE_NAME);

    // `--minimized` so a sign-in launch goes straight to the tray rather than
    // opening a window the user did not ask for.
    let command = format!("\"{}\" --minimized", exe.display());
    let value = HSTRING::from(command.as_str());

    // REG_SZ must include the terminating NUL in its byte count.
    let bytes: Vec<u8> = value
        .iter()
        .chain(std::iter::once(&0u16))
        .flat_map(|u| u.to_le_bytes())
        .collect();

    // SAFETY: `name` and `bytes` outlive the call and the length matches.
    let status = unsafe { RegSetValueExW(key.0, &name, None, REG_SZ, Some(&bytes)) };
    status
        .ok()
        .map_err(|e| Error::Platform(format!("could not enable autostart: {e}")))
}

fn disable() -> Result<()> {
    let key = RegKey::open(KEY_WRITE)?;
    let name = HSTRING::from(VALUE_NAME);
    // SAFETY: `name` outlives the call.
    let status = unsafe { RegDeleteValueW(key.0, &name) };

    // Deleting a value that is not there is the desired end state, not a failure.
    if status == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    status
        .ok()
        .map_err(|e| Error::Platform(format!("could not disable autostart: {e}")))
}

/// Whether autostart is currently enabled.
///
/// Reports `false` rather than erroring when the key cannot be read, because a
/// settings screen that cannot render is worse than one showing a safe default.
pub fn is_enabled() -> bool {
    read_value().is_some()
}

/// The command line currently registered, if any.
pub fn registered_command() -> Option<String> {
    read_value()
}

fn read_value() -> Option<String> {
    let key = RegKey::open(KEY_READ).ok()?;
    let name = HSTRING::from(VALUE_NAME);

    // First call with a null buffer asks for the required size.
    let mut size: u32 = 0;
    // SAFETY: `name` outlives the call; a null data pointer is the documented
    // way to query the size.
    let status = unsafe { RegQueryValueExW(key.0, &name, None, None, None, Some(&mut size)) };
    if status.is_err() || size == 0 {
        return None;
    }
    // Guard against a hostile or corrupt value claiming a huge size.
    if size > 64 * 1024 {
        tracing::warn!(
            size,
            "the autostart value is implausibly large, ignoring it"
        );
        return None;
    }

    let mut buffer = vec![0u8; size as usize];
    // SAFETY: `buffer` has exactly `size` bytes, which is what we were told.
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            &name,
            None,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    status.ok().ok()?;

    // REG_SZ is UTF-16; drop the terminating NUL.
    let units: Vec<u16> = buffer
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|&u| u != 0)
        .collect();

    if units.is_empty() {
        return None;
    }
    Some(String::from_utf16_lossy(&units))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All of these read and write the same `Run` value, so they must not
    /// interleave: one test disabling autostart would make another fail to
    /// observe the value it just wrote.
    static AUTOSTART_TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    /// Restores whatever the machine had before, so running the suite does not
    /// change the developer real autostart setting.
    struct Restore(Option<String>);

    impl Restore {
        fn capture() -> Self {
            Self(read_value())
        }
    }

    impl Drop for Restore {
        fn drop(&mut self) {
            match &self.0 {
                Some(_) => { /* Left as the test set it; see restore_exact below. */ }
                None => {
                    let _ = disable();
                }
            }
        }
    }

    fn fake_exe() -> std::path::PathBuf {
        std::env::current_exe().expect("the test binary path")
    }

    #[test]
    fn enabling_then_disabling_round_trips() {
        let _serialised = AUTOSTART_TEST_LOCK.lock();
        let previous = Restore::capture();
        if previous.0.is_some() {
            return; // Do not clobber a real user setting.
        }

        set_enabled(true, &fake_exe()).expect("enable autostart");
        assert!(is_enabled(), "autostart did not take effect");

        set_enabled(false, &fake_exe()).expect("disable autostart");
        assert!(!is_enabled(), "autostart was not removed");
    }

    #[test]
    fn the_registered_command_quotes_the_path_and_starts_minimized() {
        let _serialised = AUTOSTART_TEST_LOCK.lock();
        let previous = Restore::capture();
        if previous.0.is_some() {
            return;
        }

        let exe = fake_exe();
        set_enabled(true, &exe).unwrap();
        let command = registered_command().expect("a registered command");

        assert!(
            command.starts_with('"'),
            "an unquoted path breaks on Program Files: {command}"
        );
        assert!(command.contains(&exe.display().to_string()));
        assert!(
            command.contains("--minimized"),
            "a sign-in launch must go to the tray: {command}"
        );

        set_enabled(false, &exe).unwrap();
    }

    #[test]
    fn disabling_when_already_disabled_succeeds() {
        let _serialised = AUTOSTART_TEST_LOCK.lock();
        let previous = Restore::capture();
        if previous.0.is_some() {
            return;
        }
        set_enabled(false, &fake_exe()).expect("first disable");
        set_enabled(false, &fake_exe()).expect("disabling twice must not fail");
    }

    #[test]
    fn enabling_twice_is_idempotent() {
        let _serialised = AUTOSTART_TEST_LOCK.lock();
        let previous = Restore::capture();
        if previous.0.is_some() {
            return;
        }
        let exe = fake_exe();
        set_enabled(true, &exe).unwrap();
        let first = registered_command();
        set_enabled(true, &exe).unwrap();
        assert_eq!(registered_command(), first);
        set_enabled(false, &exe).unwrap();
    }

    #[test]
    fn a_relative_path_is_refused() {
        let err = set_enabled(true, Path::new("kova-screen.exe")).unwrap_err();
        assert!(matches!(err, Error::Platform(_)));
    }

    #[test]
    fn reading_when_unset_reports_disabled_rather_than_erroring() {
        let _serialised = AUTOSTART_TEST_LOCK.lock();
        let previous = Restore::capture();
        if previous.0.is_some() {
            return;
        }
        let _ = disable();
        assert!(!is_enabled());
        assert_eq!(registered_command(), None);
    }
}

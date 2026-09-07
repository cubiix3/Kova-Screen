//! Secret storage backed by Windows Credential Manager.
//!
//! The vgy.me user key is the only secret Kova Screen holds, and it never
//! touches `settings.json`, a log line or a crash report. It lives here, in the
//! per-user credential vault, encrypted by Windows with the user login secret.
//!
//! Everything in this module treats the secret as opaque bytes and none of it
//! is ever formatted into an error message -- an error says *that* the vault
//! call failed, never what was in it.

use kova_screen_core::{Error, Result};
use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW,
    CredWriteW,
};
use windows_core::{HSTRING, PWSTR};

/// Credential target for the vgy.me user key.
///
/// Appears in Credential Manager under this name, so a user can inspect or
/// remove it with the standard Windows UI rather than needing Kova Screen.
pub const VGY_TARGET: &str = "KovaScreen:vgy.me";

/// Largest secret we will store or read back.
///
/// A vgy.me user key is a short token; the cap stops a malformed or hostile
/// vault entry from turning into a large allocation.
const MAX_SECRET_BYTES: usize = 8 * 1024;

/// Stores `secret` under `target`, replacing any existing value.
///
/// An empty secret deletes the entry instead, which is what "clear the field
/// and save" should mean.
pub fn store(target: &str, secret: &str) -> Result<()> {
    if secret.is_empty() {
        return delete(target);
    }
    if secret.len() > MAX_SECRET_BYTES {
        return Err(Error::Credential("the key is too long to store".into()));
    }

    let target_name = HSTRING::from(target);
    let mut blob = secret.as_bytes().to_vec();

    let credential = CREDENTIALW {
        Flags: Default::default(),
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target_name.as_ptr() as *mut u16),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        // LOCAL_MACHINE rather than SESSION so the key survives a reboot, which
        // is the whole point of not making the user retype it.
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        ..Default::default()
    };

    // SAFETY: every pointer in `credential` refers to a local that outlives the
    // call, and the blob length matches the buffer.
    unsafe { CredWriteW(&credential, 0) }.map_err(|e| {
        Error::Credential(format!("could not save the key to credential manager: {e}"))
    })
}

/// Reads the secret stored under `target`.
///
/// `Ok(None)` means "not set", which is the normal state before the user enters
/// a key and must not be reported as an error.
pub fn load(target: &str) -> Result<Option<String>> {
    let target_name = HSTRING::from(target);
    let mut credential: *mut CREDENTIALW = std::ptr::null_mut();

    // SAFETY: `target_name` outlives the call; `credential` receives a pointer
    // that must be released with CredFree, which the guard below does.
    let read = unsafe { CredReadW(&target_name, CRED_TYPE_GENERIC, None, &mut credential) };

    if let Err(err) = read {
        // A missing credential is not a failure.
        if err.code() == ERROR_NOT_FOUND.to_hresult() {
            return Ok(None);
        }
        return Err(Error::Credential(format!(
            "could not read from credential manager: {err}"
        )));
    }

    if credential.is_null() {
        return Ok(None);
    }
    let _guard = CredGuard(credential);

    // SAFETY: `credential` is non-null and owned by us until CredFree.
    let (ptr, len) = unsafe {
        (
            (*credential).CredentialBlob,
            (*credential).CredentialBlobSize as usize,
        )
    };

    if ptr.is_null() || len == 0 {
        return Ok(None);
    }
    if len > MAX_SECRET_BYTES {
        return Err(Error::Credential(
            "the stored key is implausibly large".into(),
        ));
    }

    // SAFETY: the vault reports `len` readable bytes at `ptr`.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };

    // A blob written by another tool need not be valid UTF-8. Report that
    // rather than panicking or silently sending mojibake to the upload host.
    let secret = String::from_utf8(bytes.to_vec())
        .map_err(|_| Error::Credential("the stored key is not valid text".into()))?;

    Ok(Some(secret))
}

/// Removes the secret stored under `target`. Deleting a missing entry succeeds.
pub fn delete(target: &str) -> Result<()> {
    let target_name = HSTRING::from(target);
    // SAFETY: `target_name` outlives the call.
    match unsafe { CredDeleteW(&target_name, CRED_TYPE_GENERIC, None) } {
        Ok(()) => Ok(()),
        Err(err) if err.code() == ERROR_NOT_FOUND.to_hresult() => Ok(()),
        Err(err) => Err(Error::Credential(format!(
            "could not remove the key from credential manager: {err}"
        ))),
    }
}

/// Whether a secret is stored, without reading it back.
///
/// Lets the settings UI show "a key is saved" without ever pulling the secret
/// across the IPC boundary into the WebView.
pub fn exists(target: &str) -> bool {
    matches!(load(target), Ok(Some(_)))
}

/// Frees a credential returned by `CredReadW`.
struct CredGuard(*mut CREDENTIALW);

impl Drop for CredGuard {
    fn drop(&mut self) {
        // SAFETY: the pointer came from CredReadW and is freed exactly once.
        unsafe {
            CredFree(self.0.cast());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique target per test, so tests do not disturb a real saved key.
    fn target(name: &str) -> String {
        format!("KovaScreenTest:{name}:{}", std::process::id())
    }

    #[test]
    fn a_secret_round_trips_through_the_vault() {
        let t = target("roundtrip");
        store(&t, "vgy-user-key-abc123").unwrap();
        assert_eq!(load(&t).unwrap().as_deref(), Some("vgy-user-key-abc123"));
        delete(&t).unwrap();
    }

    #[test]
    fn a_missing_secret_reads_as_none_not_an_error() {
        let t = target("missing");
        let _ = delete(&t);
        assert_eq!(load(&t).unwrap(), None);
    }

    #[test]
    fn storing_twice_replaces_the_value() {
        let t = target("replace");
        store(&t, "first").unwrap();
        store(&t, "second").unwrap();
        assert_eq!(load(&t).unwrap().as_deref(), Some("second"));
        delete(&t).unwrap();
    }

    #[test]
    fn storing_an_empty_secret_clears_it() {
        let t = target("clear");
        store(&t, "something").unwrap();
        store(&t, "").unwrap();
        assert_eq!(load(&t).unwrap(), None);
    }

    #[test]
    fn deleting_a_missing_secret_succeeds() {
        let t = target("delete-missing");
        let _ = delete(&t);
        delete(&t).expect("deleting a missing credential must not fail");
    }

    #[test]
    fn exists_reflects_the_stored_state() {
        let t = target("exists");
        let _ = delete(&t);
        assert!(!exists(&t));
        store(&t, "key").unwrap();
        assert!(exists(&t));
        delete(&t).unwrap();
        assert!(!exists(&t));
    }

    #[test]
    fn unicode_and_punctuation_survive_intact() {
        let t = target("unicode");
        let secret = "kéy-wíth-ünïcode-and-symbols_!@#$%^&*()";
        store(&t, secret).unwrap();
        assert_eq!(load(&t).unwrap().as_deref(), Some(secret));
        delete(&t).unwrap();
    }

    #[test]
    fn an_oversized_secret_is_refused() {
        let t = target("oversized");
        let err = store(&t, &"x".repeat(MAX_SECRET_BYTES + 1)).unwrap_err();
        assert!(matches!(err, Error::Credential(_)));
    }

    #[test]
    fn errors_never_quote_the_secret() {
        // Guards the rule that a secret must not reach a log or an error string.
        let t = target("no-leak");
        let marker = "SUPERSECRETMARKER";
        let oversized = marker.repeat(MAX_SECRET_BYTES);

        let err = store(&t, &oversized).unwrap_err().to_string();
        assert!(
            !err.contains(marker),
            "the rejection message quoted the secret: {err}"
        );

        // The same must hold for a target that cannot be read back.
        let err = load("").err().map(|e| e.to_string()).unwrap_or_default();
        assert!(!err.contains(marker));
    }

    #[test]
    fn the_production_target_is_namespaced_to_the_app() {
        // A generic target name would collide with other tools in the vault.
        assert!(VGY_TARGET.starts_with("KovaScreen:"));
    }
}

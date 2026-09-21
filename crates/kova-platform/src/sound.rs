//! The capture sound.
//!
//! Played only after a still capture the user can actually keep. The call
//! returns immediately. A missing system sound is ignored, because the
//! capture has already succeeded.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::Win32::Media::Audio::{PlaySoundW, SND_FILENAME, SND_NODEFAULT, SND_NOWAIT, SND_SYNC};
use windows_core::PCWSTR;

/// Plays the Windows notification sound, asynchronously from the caller's point
/// of view. A capture must not wait on audio.
pub fn play_capture() {
    let spawned = std::thread::Builder::new()
        .name("kova-sound".into())
        .spawn(play_blocking);
    if spawned.is_err() {
        tracing::warn!("could not start the capture sound");
    }
}

fn play_blocking() {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    let path = std::path::PathBuf::from(root).join(r"Media\Windows Notify System Generic.wav");
    let wide: Vec<u16> = OsStr::new(path.as_os_str())
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a NUL-terminated path that outlives this synchronous
    // playback. SND_NODEFAULT keeps a missing file from playing something else.
    let played = unsafe {
        PlaySoundW(
            PCWSTR(wide.as_ptr()),
            None,
            SND_FILENAME | SND_SYNC | SND_NODEFAULT | SND_NOWAIT,
        )
    };
    if !played.as_bool() {
        tracing::debug!("the capture sound file was not played");
    }
}

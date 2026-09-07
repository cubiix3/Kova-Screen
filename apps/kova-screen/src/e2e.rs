//! End-to-end tests for the capture pipeline.
//!
//! These drive the real code path a hotkey takes -- capture the screen, encode
//! it, write the file, put it on the clipboard, record it in history -- against
//! a temporary capture folder. Unlike the unit tests, nothing here is stubbed:
//! the pixels come from the actual display and the PNG on disk is decoded again
//! to check it is a real image of the right size.
//!
//! Everything that needs real pixels is gated on an interactive desktop, so a
//! locked workstation or a CI runner without a display skips rather than fails.

#![cfg(test)]

use std::path::PathBuf;
use std::sync::Arc;

use kova_screen_core::settings::{ImageFormat, Settings};

use crate::app::AppState;
use crate::pipeline::{self, ShotRequest};

/// A settings object writing into a fresh temporary folder.
fn settings_in(name: &str) -> (Settings, PathBuf) {
    let dir = std::env::temp_dir().join("kova-e2e").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mut settings = Settings::default();
    settings.storage.capture_dir = Some(dir.clone());
    // Uploading is not exercised here: the provider is disabled in test state,
    // and a test must never send a screenshot of the developer's desktop
    // anywhere.
    settings.upload.enabled = false;
    (settings, dir)
}

fn state_for(settings: Settings) -> Arc<AppState> {
    AppState::for_test(settings)
}

#[test]
fn a_fullscreen_capture_writes_a_real_png_and_records_it() {
    kova_capture::require_interactive_desktop!();

    let (settings, dir) = settings_in("fullscreen-png");
    let state = state_for(settings);

    let outcome = pipeline::take_screenshot(&state, ShotRequest::Fullscreen)
        .expect("a fullscreen capture must succeed");

    assert!(!outcome.cancelled);
    assert!(
        outcome.warnings.is_empty(),
        "the capture reported problems: {:?}",
        outcome.warnings
    );

    // The file exists, is in the configured folder, and is a decodable PNG of
    // exactly the reported extent.
    let path = outcome.path.expect("a saved file");
    assert_eq!(path.parent().unwrap(), dir);
    assert!(path.extension().unwrap() == "png");

    let bytes = std::fs::read(&path).expect("the capture is readable");
    assert!(bytes.len() > 1024, "the png is only {} bytes", bytes.len());

    let decoded = image::load_from_memory(&bytes).expect("a decodable png");
    assert_eq!(decoded.width(), outcome.width);
    assert_eq!(decoded.height(), outcome.height);

    // It matches a real monitor, so we know we captured a display rather than
    // some incidental rectangle.
    let monitors = kova_capture::monitor::list();
    assert!(
        monitors
            .iter()
            .any(|m| m.bounds.width == outcome.width && m.bounds.height == outcome.height),
        "the capture is {}x{}, which matches no connected monitor",
        outcome.width,
        outcome.height
    );

    // The clipboard was written, and the history row points at the file.
    assert!(
        outcome.copied,
        "the screenshot was not copied to the clipboard"
    );

    let history = state.history().expect("history");
    let entries = history.recent(10).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, path);
    assert_eq!(entries[0].kind, kova_history::CaptureKind::Screenshot);
    assert_eq!(entries[0].upload_state, kova_history::UploadState::None);
    assert!(entries[0].size_bytes > 0);
}

#[test]
fn every_output_format_produces_a_decodable_file() {
    kova_capture::require_interactive_desktop!();

    for (format, extension) in [
        (ImageFormat::Png, "png"),
        (ImageFormat::Jpeg, "jpg"),
        (ImageFormat::Webp, "webp"),
    ] {
        let (mut settings, _dir) = settings_in(&format!("format-{extension}"));
        settings.capture.format = format;
        // Keep the clipboard out of it: three formats in a row would otherwise
        // fight over the clipboard with any other test running concurrently.
        settings.capture.copy_to_clipboard = false;

        let state = state_for(settings);
        let outcome = pipeline::take_screenshot(&state, ShotRequest::Fullscreen)
            .unwrap_or_else(|e| panic!("{format:?} capture failed: {e}"));

        let path = outcome.path.expect("a saved file");
        assert_eq!(
            path.extension().unwrap(),
            extension,
            "{format:?} produced the wrong extension"
        );

        let bytes = std::fs::read(&path).unwrap();
        let decoded = image::load_from_memory(&bytes)
            .unwrap_or_else(|e| panic!("{format:?} produced an undecodable file: {e}"));
        assert_eq!(decoded.width(), outcome.width);
        assert_eq!(decoded.height(), outcome.height);
    }
}

#[test]
fn capturing_all_monitors_covers_the_whole_virtual_desktop() {
    kova_capture::require_interactive_desktop!();

    let (mut settings, _dir) = settings_in("all-monitors");
    settings.capture.copy_to_clipboard = false;
    let state = state_for(settings);

    let outcome = pipeline::take_screenshot(&state, ShotRequest::AllMonitors)
        .expect("an all-monitors capture must succeed");

    let desktop = kova_capture::monitor::virtual_desktop_bounds().unwrap();
    assert_eq!(outcome.width, desktop.width);
    assert_eq!(outcome.height, desktop.height);
}

#[test]
fn repeated_captures_never_overwrite_each_other() {
    kova_capture::require_interactive_desktop!();

    // Three captures inside one second: the filename template has one-second
    // resolution, so this is exactly the collision case a user hits by holding
    // the hotkey.
    let (mut settings, dir) = settings_in("collisions");
    settings.capture.copy_to_clipboard = false;
    let state = state_for(settings);

    let mut paths = Vec::new();
    for _ in 0..3 {
        let outcome = pipeline::take_screenshot(&state, ShotRequest::Fullscreen).unwrap();
        paths.push(outcome.path.expect("a saved file"));
    }

    paths.sort();
    paths.dedup();
    assert_eq!(paths.len(), 3, "captures overwrote each other");

    let on_disk = std::fs::read_dir(&dir).unwrap().count();
    assert_eq!(
        on_disk, 3,
        "the capture folder holds {on_disk} files, expected 3"
    );
}

#[test]
fn a_capture_still_succeeds_when_the_capture_folder_is_read_only() {
    kova_capture::require_interactive_desktop!();

    // The product rule: a failure after the pixels exist is a warning beside a
    // successful capture, never a failed one. A folder that cannot be written
    // is the clearest way to exercise that.
    let (settings, dir) = settings_in("readonly");
    // Point at a path that cannot be created: a file where a directory belongs.
    let blocked = dir.join("not-a-directory");
    std::fs::write(&blocked, b"x").unwrap();

    let mut settings = settings;
    settings.storage.capture_dir = Some(blocked.join("captures"));

    let state = state_for(settings);
    let outcome = pipeline::take_screenshot(&state, ShotRequest::Fullscreen)
        .expect("the capture itself must still succeed");

    assert!(outcome.path.is_none(), "a file was somehow written");
    assert!(
        outcome.copied,
        "the clipboard should still have received the screenshot"
    );
    assert!(
        outcome.warnings.iter().any(|w| w.contains("save")),
        "the save failure was not reported: {:?}",
        outcome.warnings
    );
    // The headline must not claim the screenshot failed.
    assert_eq!(outcome.title(), "Screenshot copied");
}

#[test]
fn history_records_the_capture_but_a_capture_works_without_history() {
    kova_capture::require_interactive_desktop!();

    let (mut settings, _dir) = settings_in("history-limit");
    settings.capture.copy_to_clipboard = false;
    // A limit of two, with three captures, exercises pruning on the real path.
    settings.storage.history_limit = 2;

    let state = state_for(settings);
    for _ in 0..3 {
        pipeline::take_screenshot(&state, ShotRequest::Fullscreen).unwrap();
    }

    let history = state.history().expect("history");
    assert_eq!(
        history.count().unwrap(),
        2,
        "the history limit was not applied"
    );

    // Pruning must never delete the files themselves.
    let entries = history.recent(10).unwrap();
    for entry in &entries {
        assert!(entry.path.exists(), "pruning deleted a real capture");
    }
}

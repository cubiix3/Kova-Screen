# Changelog

All notable changes to Kova Screen are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.2] - 2026-09-10

### Fixed

- Tolerate a profile without a `Run` key: enabling autostart creates it, and
  disabling when it is missing reads as already disabled instead of failing.

## [0.1.1] - 2026-09-09

### Fixed

- Serialize hotkey replacement so concurrent settings updates cannot leave
  shortcuts unregistered; ignore held-key repeats while editing shortcuts.
- Normalize the Space key to the spelling accepted by the hotkey parser.

- Exclude pauses from recording timestamps, the timer and the duration limit.
- Stop GIF capture automatically when its size budget is reached.
- Apply screenshot delay and cursor preferences to region selections.
- Bound automatic uploads to one worker and eight queued files; report saturation.
- Ignore repeated capture actions while a selection or startup is in progress.
- Keep manual upload and online deletion off the UI event loop; show upload
  progress and disable conflicting row actions while an upload is pending.
- Default new settings to software encoding; label the optional hardware path
  experimental because its native resource-retention issue is not resolved.

- Compose region selections off-screen to prevent the dimmed desktop flickering.
- Create only one tray icon.
- Keep the tray app running after the last settings/history window closes.
- Add hover feedback to recorder controls and a click-through recording outline.
- Show compact, rounded status cards at the bottom-right, including portable builds.
- Avoid full-frame clones and redundant GPU readbacks; skip readback while paused.
- Reuse the Media Foundation runtime across recordings to reduce native worker
  handle growth, and shut it down when the application exits.
- Keep the Windows capture code module loaded to prevent a native teardown
  crash; capture sessions, frame pools and GPU buffers are still released.
- Release the overlay device context when bitmap allocation fails.

## [0.1.0] - 2026-09-07

First release.

### Added

**Screenshots**

- Region capture with a native overlay: the desktop is frozen and dimmed, the
  selection size is shown while dragging, `Esc` or right-click cancels
- Fullscreen capture of the display under the cursor, or of all displays as one
  image
- Window capture through Windows Graphics Capture, with a GDI fallback
- PNG, JPEG and WebP output; PNG by default
- Optional cursor and optional shutter delay

**Recording**

- MP4 recording with H.264 via Media Foundation, hardware-accelerated where
  available and falling back to the software encoder
- GIF recording with streaming encoding, proportional downscaling and a size
  budget, so no temporary frame folder is created
- A floating recorder overlay with elapsed time, Pause and Stop, excluded from
  the recording it controls

**After the capture**

- Automatic save to `%USERPROFILE%\Pictures\Kova Screen`, with a configurable
  path, a filename template, and no filename collisions
- Clipboard support as DIBv5 plus PNG, so pasting works in both native and
  web-based applications
- Optional vgy.me upload behind a provider abstraction, with the direct or page
  URL copied to the clipboard
- Recent Captures window with open, reveal, copy file, copy path, copy URL,
  upload, delete local and delete online

**Windows integration**

- System tray with the full capture menu
- Global hotkeys with conflict detection that names the affected binding
- Launch with Windows via the per-user `Run` key
- Toast notifications
- Per-Monitor-DPI-V2, multi-monitor and mixed scaling support

**Settings**

- General, Capture, Recording, Upload, Hotkeys, Storage and About sections
- The vgy.me user key stored in Windows Credential Manager

### Known limitations

- Audio is not recorded in MP4
- WebP is encoded losslessly, so the quality setting does not apply to it
- vgy.me does not accept MP4; recordings stay local and the app says so
- HDR displays are captured in SDR
- Window capture uses the foreground window rather than offering a picker

[Unreleased]: https://github.com/cubiix3/Kova-Screen/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/cubiix3/Kova-Screen/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/cubiix3/Kova-Screen/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/cubiix3/Kova-Screen/releases/tag/v0.1.0

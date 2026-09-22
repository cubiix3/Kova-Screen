# Changelog

All notable changes to Kova Screen are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.4] - 2026-09-22

### Added

- The selector can click a window, press Enter for the display under the
  pointer, nudge with the arrow keys, and show a magnifier. Space, or
  Alt+Print Screen, repeats the previous rectangle.
- A hotkey for capturing every display.
- Recording the window or display chosen in that selector, without copying the
  whole desktop first. An optional countdown runs before the selector.
- A shutter sound, off by default.
- Recent Captures can copy a screenshot as an image. Delete asks first. The
  list reloads when a capture is saved and loads older entries on demand.
- Recordings store their dimensions. The saved-file notice says when a GIF was
  scaled down, when a region was trimmed to one display, and when Windows
  cannot hide the recorder overlay.
- A GIF maximum-width setting. About opens the release list.
- A notice when a capture came from an HDR display. The image is still SDR.

### Changed

- MP4 files are no longer offered for upload. vgy.me does not accept them.
- GIF frames are quantised on their own thread behind a two-frame queue, so a
  slow frame no longer delays the next capture. Memory stays bounded; a frame
  that arrives while the queue is full is dropped and the previous one stays on
  screen longer.
- A JPEG or WebP screenshot encodes the clipboard PNG alongside the file
  instead of after it.
- Loading Recent Captures, copying an image from it, deleting a file and
  pruning missing files run off the main thread, so a large image or a slow
  folder no longer freezes the windows and the tray.

### Fixed

- The capture-folder picker and the Recent Captures auto-reload have the
  WebView permissions Tauri requires for them.
- Hotkeys that differ only in modifier order or key alias, such as
  `Shift+Ctrl+R` and `Control+Shift+R`, are reported as a conflict instead of
  one silently failing to register. The conflict message names the
  all-displays and repeat-region actions properly.
- A screenshot that could not be written completely no longer leaves a
  truncated file in the capture folder.
- Settings saved at the same moment from two places can no longer overwrite
  each other or share a temporary file.
- Recent Captures shows thumbnails from the selected capture folder.
- Local deletion and history cleanup keep uploaded entries and their online
  deletion links. Deleting an online copy now asks for confirmation.
- Rapid settings changes are saved in order instead of overwriting each other.
- `settings.json` ignores fields it does not know, so a file written by a newer
  build no longer makes an older build fall back to defaults and silently reset
  every preference.
- A window capture no longer keeps Window's composited alpha, which could paste
  as transparent holes; WGC frames are forced opaque like the GDI path.
- A clipboard copy that placed the image but could not add the PNG or file
  format is no longer reported as a failed copy; the optional formats are
  logged instead.
- A failed PNG encode for a non-PNG capture is logged rather than silently
  copying DIBv5 only.
- A rejected upload now shows vgy.me's own explanation. The HTTP client treated
  any non-2xx status as an unreachable server and discarded the response body,
  so a refused GIF read as "could not reach vgy.me" instead of the real reason.

### Security

- The CI token can read the repository and write its own cache and artifacts.
- A saved upload key is copied out of Credential Manager before that record is
  freed. The selection dim no longer writes through a raw bitmap pointer.
- The library tests start on a clean machine because comctl32 is delay-loaded,
  replacing the prebuilt compiler wrapper the repository used to ship. Every
  `unsafe` block is now required by Clippy to state its safety invariant.

## [0.1.3] - 2026-09-14

### Added

- Optionally copy a saved screenshot as a file alongside the image clipboard
  formats, so Explorer and apps that accept file drops can receive it.

### Fixed

- Keep file clipboard data opt-in and omit it when saving fails or image
  clipboard copying is disabled.
- Validate file-drop paths before opening the clipboard and preserve Unicode
  paths with the required double terminator.

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

[Unreleased]: https://github.com/cubiix3/Kova-Screen/compare/v0.1.4...HEAD
[0.1.4]: https://github.com/cubiix3/Kova-Screen/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/cubiix3/Kova-Screen/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/cubiix3/Kova-Screen/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/cubiix3/Kova-Screen/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/cubiix3/Kova-Screen/releases/tag/v0.1.0

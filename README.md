<div align="center">

<img src="apps/kova-screen/icons/128x128.png" alt="" width="96" height="96">

# Kova Screen

**Fast, minimal screen capture for Windows.**<br>
Screenshots, GIF and MP4 recording, clipboard integration and optional upload,
without the feature sprawl.

[![CI](https://github.com/cubiix3/Kova-Screen/actions/workflows/ci.yml/badge.svg)](https://github.com/cubiix3/Kova-Screen/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/cubiix3/Kova-Screen?color=86d5f4)](https://github.com/cubiix3/Kova-Screen/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/cubiix3/Kova-Screen/total?color=86d5f4)](https://github.com/cubiix3/Kova-Screen/releases)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-86d5f4)](#install)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-86d5f4)](#license)

[**Download**](https://github.com/cubiix3/Kova-Screen/releases/latest) ·
[Features](#features) ·
[Hotkeys](#hotkeys) ·
[Privacy](#privacy) ·
[Build from source](#build-from-source) ·
[Changelog](CHANGELOG.md)

</div>

---

```
Hotkey  →  select  →  capture  →  save  →  clipboard  →  (optional) upload
```

Press a key, drag a rectangle, paste the result. Kova Screen lives in the tray,
starts nothing until you ask for it, and has no image editor, no OCR, no
account and no cloud. It takes screenshots, quickly.

Part of the Kova family, alongside [Kova Image](https://github.com/cubiix3/Kova-Image).

## Why Kova Screen

|  |  |
| --- | --- |
| ⚡ **Native capture path** | The selector and the recorder controls are plain Win32. No web view sits between the hotkey and your pixels. |
| 🎯 **Exact pixels** | Per-Monitor DPI V2 and physical-pixel capture, mixed scaling included. What you release on is what lands in the file. |
| 🧠 **Honest results** | A locked clipboard or a failed upload never reads as a failed screenshot. You get *"Screenshot saved / Upload failed"*. |
| 🪶 **Light at idle** | A tray icon and a hotkey listener. No capture session, GPU device, web view or polling loop until you press a key. |
| 🔒 **Offline by default** | No telemetry, no update check, no account. Upload is off until you turn it on. |

## Features

### Screenshots

| Mode | What you get |
| --- | --- |
| **Region** | Drag a rectangle on the frozen desktop. |
| **Window** | Click a window in the selector, or press <kbd>Shift</kbd>+<kbd>Print Screen</kbd> for the foreground window. Uses Windows Graphics Capture, so covered and GPU-accelerated windows come out correct. |
| **Fullscreen** | The display under the pointer. |
| **All displays** | Every display as one image. |
| **Repeat** | The rectangle you dragged last time, captured again. |

PNG by default, or JPEG or lossless WebP. The pointer, a shutter delay and a
shutter sound are optional.

### The selector

Region screenshots and both recordings open the same selector, with a
magnifier that follows the pointer.

| Input | Does |
| --- | --- |
| Drag | Choose a rectangle; the size is shown while you drag |
| Click | Capture the window under the pointer |
| <kbd>Enter</kbd> | Capture the display under the pointer |
| <kbd>Space</kbd> | Repeat the previous rectangle |
| Arrow keys | Nudge the corner; with <kbd>Shift</kbd>, ten pixels |
| <kbd>Esc</kbd> or right-click | Cancel |

For a still image the desktop is frozen first, so the file matches what you
selected. A recording only needs the target, so its selector does not copy the
desktop.

### Recording

- **MP4**: H.264 through Media Foundation at 30 or 60 FPS. Software encoding
  by default; hardware encoding is optional and marked experimental.
- **GIF**: 10, 15, 20 or 30 FPS. Frames are quantised on their own thread and
  streamed straight to the file, with no temporary frame folder. Wide GIFs are
  scaled down (1280 px by default) and a size budget stops runaway files.
- Record a region, a window (followed as it moves) or a whole display, with an
  optional countdown.
- A small overlay shows the elapsed time, **Pause** and **Stop**, and is kept
  out of the recording itself.

There is no audio yet.

### After the capture

- **Saved** under `Pictures\Kova Screen` or a folder you choose, with a
  filename template and no collisions.
- **Copied** to the clipboard as DIBv5 and PNG, so it pastes into Paint, Word,
  browsers, Discord and Slack. Optionally the file itself is offered too, for
  apps that accept file drops.
- **Uploaded**, if you enable it, to [vgy.me](https://vgy.me) (screenshots and
  GIFs; vgy.me does not accept MP4), with the link copied for you if you like.
- **Recent Captures** lists everything with open, reveal, copy image, copy
  file, copy path, upload and delete, and refreshes as you capture.

### Displays

Mixed-scale setups work: a selection dragged across two displays, or into the
gap between mismatched ones, is clamped to what is really on screen. HDR
displays are saved in SDR, and the notice tells you when that happened.

## Hotkeys

| Action | Default |
| --- | --- |
| Region screenshot | <kbd>Print Screen</kbd> |
| Fullscreen screenshot | <kbd>Ctrl</kbd>+<kbd>Print Screen</kbd> |
| Window screenshot | <kbd>Shift</kbd>+<kbd>Print Screen</kbd> |
| All displays | <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>Print Screen</kbd> |
| Repeat last region | <kbd>Alt</kbd>+<kbd>Print Screen</kbd> |
| Record MP4 | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>R</kbd> |
| Record GIF | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>G</kbd> |
| Stop recording | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>S</kbd> |

Every binding can be changed in Settings. Two actions on the same shortcut are
refused, however the modifiers are written, and if another application already
owns a combination, Settings names it instead of the hotkey silently doing
nothing.

## Install

1. Download `Kova-Screen_<version>_x64-setup.exe` from the
   [latest release](https://github.com/cubiix3/Kova-Screen/releases/latest).
2. Run it. It installs per user, so no administrator rights are needed.
3. Press <kbd>Print Screen</kbd>.

It uninstalls cleanly through **Settings › Apps**. Prefer not to install? The
release also has a **portable ZIP** that runs from any folder.

**Requirements:** Windows 10 1903 or newer, or Windows 11. WebView2 is
preinstalled on current systems; the installer fetches it if it is missing.

<details>
<summary>Verify a download</summary>

Compare the hash with `SHA256SUMS.txt` on the release page:

```powershell
Get-FileHash .\Kova-Screen_*_x64-setup.exe -Algorithm SHA256
```

</details>

## Privacy

- **No network by default.** No telemetry, analytics, update check or account.
  The only request the app can make is an upload you turned on.
- **Your upload key stays in Windows Credential Manager.** Settings can save it
  and ask whether one exists, but nothing can read it back out, and it never
  appears in a log or an error message.
- **Deletion links are secrets.** They are stored locally so you can revoke an
  upload later, and are kept out of logs, debug output and the UI.
- **About › Releases** opens GitHub in your browser. That request is your
  browser's, not the app's.

## Build from source

You need [Rust](https://rustup.rs) (pinned in `rust-toolchain.toml`),
[Node.js](https://nodejs.org) 20 or newer, and the MSVC build tools from Visual
Studio's *Desktop development with C++* workload.

```powershell
git clone https://github.com/cubiix3/Kova-Screen
cd Kova-Screen
cd ui; npm ci; cd ..

# Run with hot reload
npx @tauri-apps/cli@2 dev --config apps/kova-screen/tauri.conf.json

# Build the installer into target/release/bundle/nsis/
npx @tauri-apps/cli@2 build --config apps/kova-screen/tauri.conf.json
```

Not in a Developer PowerShell? `scripts/cargo-msvc.ps1` finds Visual Studio and
runs cargo inside its environment.

<details>
<summary>The checks CI runs</summary>

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --test-threads=1
cd ui && npm run lint && npm run build
```

Some tests capture the real screen. On a locked workstation or a session with
no display they skip and print the reason instead of failing.

</details>

<details>
<summary>Architecture</summary>

```
kova-screen-core   types, settings, filename templating, paths
kova-capture       WGC + GDI capture, monitors, windows, DPI, frame sessions
kova-encode        PNG/JPEG/WebP, streaming GIF, H.264 MP4 via Media Foundation
kova-platform      clipboard, credentials, hotkeys, autostart, native overlays
kova-upload        provider abstraction + vgy.me
kova-history       SQLite capture history
apps/kova-screen   tray app, capture pipeline, IPC commands
ui/                settings and history windows
```

**Capture is native, settings are a web view.** The selector sits between the
hotkey and the screenshot, so its startup time *is* how fast the app feels,
and the recorder overlay has to be excluded from the recording it controls.
Settings and history are opened occasionally and are easier to lay out as a
web view.

**Upload never decides whether a capture succeeded.** Only grabbing and
encoding the pixels can fail the operation. Saving, the clipboard, the history
row and the upload are warnings beside a capture that already worked.

</details>

## Roadmap

Being considered, in no committed order:

- Audio in MP4 recordings, desktop and microphone
- Another upload provider that accepts MP4
- Scrolling capture

**Out of scope, on purpose:** an image editor, OCR, cloud sync, accounts, a
plugin system, a video editor and AI features. Kova Screen stays a capture
tool.

## Contributing

Issues and pull requests are welcome; start with
[CONTRIBUTING.md](CONTRIBUTING.md). Security reports go through
[SECURITY.md](SECURITY.md).

## License

Licensed under either the [Apache License 2.0](LICENSE-APACHE) or the
[MIT license](LICENSE-MIT), at your option, like the rest of the Kova family.
Dependency licenses are listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md);
none is GPL, LGPL-only or AGPL.

Flameshot and ShareX were a functional inspiration. No code from either is used
here.

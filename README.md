# Kova Screen

[![CI](https://github.com/cubiix3/Kova-Screen/actions/workflows/ci.yml/badge.svg)](https://github.com/cubiix3/Kova-Screen/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/cubiix3/Kova-Screen?color=86d5f4)](https://github.com/cubiix3/Kova-Screen/releases/latest)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-86d5f4)](#license)

Fast, minimal screen capture for Windows. Screenshots, GIF and MP4 recording,
clipboard integration and optional upload — without the feature sprawl.

Part of the Kova family, alongside [Kova Image](https://github.com/cubiix3/Kova-Image).

```
Hotkey  →  select  →  capture  →  save  →  clipboard  →  (optional) upload
```

No image editor. No OCR. No account. No cloud. It takes screenshots, quickly.

The [latest release](https://github.com/cubiix3/Kova-Screen/releases/latest) is
**0.1.3**. This page describes the current source. The selector, the extra
recording targets and the history changes below are in
[Unreleased](CHANGELOG.md#unreleased) and ship with the next release.

---

## Screenshots

| Mode | What you get |
| --- | --- |
| **Region** | Drag a rectangle on the dimmed desktop. The pixels you release on are the pixels in the file. |
| **Fullscreen** | The display under the pointer. |
| **All displays** | Every display, as one image. |
| **Window** | `Shift + Print Screen` takes the foreground window. A click in the selector takes the window under the pointer. Both use Windows Graphics Capture, so hardware-accelerated and covered windows come out correct. |

Still images are **PNG** by default, or JPEG or WebP. The pointer and a shutter
delay are optional. A shutter sound is optional and off by default. WebP is
lossless, so it has no quality slider; on a typical screenshot it is smaller
than PNG.

## Selector

Region screenshots and both recordings open the same selector.

| | |
| --- | --- |
| Drag | Choose a rectangle. The size is shown while you drag. |
| Click | Capture the window under the pointer. |
| Enter | Capture the display under the pointer. |
| Space | Repeat the previous rectangle. |
| Arrow keys | Nudge the corner. Shift moves ten pixels. |
| Esc or right-click | Cancel. |

A magnifier follows the pointer. For a still image the desktop is frozen first,
so the file matches what you selected. A recording only needs the target, so
that selector does not copy the desktop.

## Recording

MP4 is H.264 through Media Foundation, at 30 or 60 FPS. Software encoding is
the default. Hardware encoding is optional and marked experimental: some GPU
encoders keep native handles between recordings.

GIF runs at 10, 15, 20 or 30 FPS, default 15. Frames are quantised and written
straight to the file, so there is never a temporary frame folder. A GIF wider
than the configured maximum (1280 px by default) is scaled down, and the notice
says when that happened. A size budget stops the file from growing without
limit.

Drag a region, click a window, or press Enter for the display under the pointer.
An optional countdown runs before the selector. A rectangle that crosses two
displays is recorded on the one that holds its centre, and the result says so.
Window recordings follow the window as it moves.

A small overlay shows the elapsed time, Pause and Stop. It is kept out of the
recording. On a Windows build that cannot do that, the saved-file notice says
the controls may appear in the video.

There is no audio.

## After the capture

- Saved under `%USERPROFILE%\Pictures\Kova Screen`, or a folder you choose, with
  a filename template and no collisions.
- Copied to the clipboard as DIBv5 and PNG, so it pastes into Paint, Word,
  Chrome, Discord and Slack.
- **Settings › Capture › Also copy the screenshot file** additionally offers the
  saved file to apps that accept file drops. Off by default, and only while
  **Copy to clipboard** is on. Some apps then paste an attachment instead of an
  inline image. If saving fails, only the image is copied.
- In Claude Code on Windows, paste an image with **Alt+V**
  ([keyboard shortcuts](https://code.claude.com/docs/en/interactive-mode)).
  Ordinary terminal paste does not accept images or file drops.
- Optional upload to [vgy.me](https://vgy.me). Screenshots and GIFs only. MP4
  stays in the capture folder, because vgy.me does not accept it. The upload is
  off until you turn it on. A failed upload reads *"Screenshot saved / Upload
  failed"*, never *"Screenshot failed"*.
- **Recent Captures** lists the files with open, reveal, copy image, copy file,
  copy path, upload and delete. Delete asks first. The list reloads when a
  capture is saved.

## Displays

The app runs Per-Monitor DPI V2 and captures in physical pixels, including
mixed scaling. A selection dragged across two displays, or into the gap between
mismatched ones, is clamped to what is actually on screen.

HDR displays are saved in SDR. When the capture came from an HDR display, the
notice says so.

---

## Hotkeys

| Action | Default |
| --- | --- |
| Region screenshot | `Print Screen` |
| Fullscreen screenshot | `Ctrl + Print Screen` |
| Window screenshot | `Shift + Print Screen` |
| All displays | `Ctrl + Alt + Print Screen` |
| Repeat last region | `Alt + Print Screen` |
| Record MP4 | `Ctrl + Shift + R` |
| Record GIF | `Ctrl + Shift + G` |
| Stop recording | `Ctrl + Shift + S` |

Every binding is editable in Settings. If another application already owns a
combination, Settings names the binding and the reason, instead of the hotkey
silently doing nothing.

`Alt + Print Screen` repeats the last rectangle you dragged. Take one region
first.

---

## Installation

Download the installer from the [latest release](https://github.com/cubiix3/Kova-Screen/releases)
and run it. It installs per user, so no administrator rights are required,
adds a Start menu entry, and uninstalls through Settings › Apps.

Requirements:

- Windows 10 version 1903 or newer, or Windows 11
- WebView2. It is preinstalled on Windows 11 and current Windows 10. The
  installer fetches it when it is missing.

A portable ZIP is published next to the installer. It runs from any folder and
writes its settings to `%APPDATA%\Kova Screen`.

Check a download against `SHA256SUMS.txt` on the release:

```powershell
Get-FileHash .\Kova-Screen_*_x64-setup.exe -Algorithm SHA256
```

---

## Build from source

You need [Rust](https://rustup.rs) (the version is pinned in
`rust-toolchain.toml`), [Node.js](https://nodejs.org) 20 or newer, and the MSVC
build tools from Visual Studio's *Desktop development with C++* workload.

```bash
git clone https://github.com/cubiix3/Kova-Screen
cd Kova-Screen

cd ui && npm ci && cd ..

npx @tauri-apps/cli@2 dev --config apps/kova-screen/tauri.conf.json
npx @tauri-apps/cli@2 build --config apps/kova-screen/tauri.conf.json
```

The installer is written to `target/release/bundle/nsis/`.

The same checks CI runs:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --test-threads=1
cd ui && npm run lint && npm run build
```

Some tests capture the real screen. On a locked workstation or a session with
no display they skip and print the reason, rather than failing.

---

## Architecture

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

**Capture is native. Settings are a web view.** The selector and the recorder
overlay are Win32. The selector sits between the hotkey and the screenshot, so
its startup time is what the app feels like, and the recorder overlay has to be
excluded from the recording it controls. Neither is something a web view can
do. Settings and history are a web view because they are opened occasionally
and are easier to lay out that way.

**Upload never decides whether a capture succeeded.** Only grabbing the pixels
and encoding them can fail the operation. Saving, the clipboard, the history
row and the upload are warnings beside a capture that already worked.

---

## Privacy

Kova Screen works offline. It has no telemetry, no analytics, no update check
and no account.

The only network request it can make is an upload you turned on, to the host
you chose. Uploads are off by default. About can open the GitHub release page
in your browser. That request is the browser's, not the app's.

The vgy.me user key is stored in Windows Credential Manager, never in a config
file. The settings window can save it and ask whether one is saved. Nothing can
read it back out. It never appears in a log line or an error message.

Deletion links are stored locally so you can revoke an upload later. They are
secrets: excluded from logs, from debug output, and from everything sent to
the UI.

---

## Roadmap

Shipped through 0.1.3: the capture, recording, clipboard and upload described
in that release. See [CHANGELOG.md](CHANGELOG.md).

Being considered, in no committed order:

- Audio in MP4 recordings, desktop and microphone
- Another upload provider that can accept MP4
- Scrolling capture

Out of scope, on purpose: an image editor, OCR, cloud sync, accounts, a plugin
system, a video editor, AI features. Kova Screen stays a capture tool.

---

## Contributing

Issues and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md).
For security reports, see [SECURITY.md](SECURITY.md).

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option, matching the rest of the Kova family.

Dependency licenses are summarised in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
They are permissive or weak file-level copyleft. There is no GPL, LGPL-only or
AGPL dependency.

Flameshot and ShareX were a functional inspiration. No code from either is used
here.

# Kova Screen

Fast, minimal screen capture for Windows. Screenshots, GIF and MP4 recording,
clipboard integration and optional upload — without the feature sprawl.

Part of the Kova family, alongside [Kova File](https://github.com/cubiix3/Kova-File-Manager)
and [Kova Image](https://github.com/cubiix3/Kova-Image).

```
Hotkey  →  select  →  capture  →  save  →  clipboard  →  (optional) upload
```

No image editor. No OCR. No account. No cloud. It takes screenshots, quickly.

---

## Features

Everything listed here is implemented and covered by tests.

### Screenshots

- **Region** — the desktop dims, the cursor becomes a crosshair, drag a
  rectangle and release. The size is shown while you drag; `Esc` or right-click
  cancels.
- **Fullscreen** — the display under the cursor, or all displays as one image.
- **Window** — the foreground window, captured through Windows Graphics Capture
  so hardware-accelerated and partly covered windows come out correct.
- Output as **PNG** (default), **JPEG** or **WebP**.
- Optional cursor, optional shutter delay.

### Recording

- **MP4** — H.264 via Media Foundation, using a hardware encoder when your GPU
  offers one and the Microsoft software encoder otherwise. 30 or 60 FPS.
- **GIF** — 10/15/20/30 FPS, default 15. Frames are quantised and written
  straight to the file, so no temporary frame folder is ever created.
- A small floating overlay shows the elapsed time with Pause and Stop. It is
  excluded from the recording it controls.

### After the capture

- Saved to `%USERPROFILE%\Pictures\Kova Screen`, configurable, with a filename
  template and no collisions.
- Copied to the clipboard as DIBv5 and PNG, so it pastes into Paint, Word,
  Chrome, Discord and Slack alike.
- Optionally uploaded to [vgy.me](https://vgy.me), with the resulting link
  copied to the clipboard.
- Recorded in a small **Recent Captures** list with open, reveal, copy, upload
  and delete actions.

### Multi-monitor and DPI

The app runs Per-Monitor-DPI-V2. Captures are taken in physical pixels, mixed
scaling factors are handled, and a selection dragged across two displays or into
the gap between mismatched monitors is clamped to what is actually on screen.

---

## Hotkeys

| Action                | Default              |
| --------------------- | -------------------- |
| Region screenshot     | `Print Screen`       |
| Fullscreen screenshot | `Ctrl + Print Screen`|
| Window screenshot     | `Shift + Print Screen`|
| Record MP4            | `Ctrl + Shift + R`   |
| Record GIF            | `Ctrl + Shift + G`   |
| Stop recording        | `Ctrl + Shift + S`   |

All of them are editable in Settings. If another application already owns a
combination, Settings says which binding is affected and why, rather than the
hotkey silently doing nothing.

---

## Installation

Download the installer from the [latest release](https://github.com/cubiix3/Kova-Screen/releases)
and run it. It installs per-user, so **no administrator rights are required**,
adds a Start menu entry, and uninstalls cleanly through Settings › Apps.

Requirements:

- Windows 10 version 1903 or newer, or Windows 11
- WebView2 runtime — preinstalled on Windows 11 and current Windows 10; the
  installer fetches it if it is missing

A portable ZIP is published alongside the installer. It runs from any folder and
writes its settings to `%APPDATA%\Kova Screen`.

Verify a download against the `SHA256SUMS.txt` on the release:

```powershell
Get-FileHash .\Kova-Screen_0.1.0_x64-setup.exe -Algorithm SHA256
```

---

## Build from source

You need [Rust](https://rustup.rs) (the toolchain is pinned in
`rust-toolchain.toml`), [Node.js](https://nodejs.org) 20+, and the MSVC build
tools that come with Visual Studio's *Desktop development with C++* workload.

```bash
git clone https://github.com/cubiix3/Kova-Screen
cd Kova-Screen

# Frontend
cd ui && npm ci && cd ..

# Run in development
npx @tauri-apps/cli@2 dev --config apps/kova-screen/tauri.conf.json

# Build the installer
npx @tauri-apps/cli@2 build --config apps/kova-screen/tauri.conf.json
```

The installer lands in `target/release/bundle/nsis/`.

Checks, the same ones CI runs:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd ui && npm run lint && npm run build
```

Some tests capture the real screen. They detect a locked workstation or a
session with no display and skip with a printed reason rather than failing.

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

Two decisions are worth knowing about:

**The capture path is native, the configuration path is web.** The region
overlay and the recorder overlay are plain Win32. The overlay sits between your
hotkey and your screenshot, so its startup latency is what the app *feels* like,
and the recorder overlay has to be excluded from the recording it controls —
neither is something a WebView can do. Settings and history are a WebView,
because they are opened occasionally and are easier to lay out that way.

**Upload is never load-bearing.** Only grabbing the pixels and encoding them can
fail a capture. Saving, the clipboard, the history row and the upload are all
reported as warnings beside a capture that already succeeded. A failed upload
says *"Screenshot saved / Upload failed"*, never *"Screenshot failed"*.

---

## Privacy

Kova Screen works entirely offline and has no telemetry, no analytics, no
update check and no account.

The only network request the app can ever make is an upload you explicitly
enabled, to the host you chose. Uploads are off by default.

Your vgy.me user key is stored in **Windows Credential Manager**, never in a
config file. It is write-only across the app's internal boundary: the settings
window can save it and ask whether one is saved, but nothing can read it back
out. It never appears in a log line or an error message.

Deletion links returned by an upload host are stored locally so you can revoke
an upload later. They are treated as secrets — excluded from logs, from debug
output, and from everything sent to the UI.

---

## Roadmap

Done in v0.1: everything under [Features](#features).

Being considered, in no committed order:

- Audio in MP4 recordings (desktop and microphone)
- More upload providers behind the existing provider abstraction
- A window picker for choosing a specific window rather than the foreground one
- Scrolling capture

Deliberately **out of scope**: image editor, OCR, cloud sync, accounts, a plugin
system, a video editor, AI features. Kova Screen is meant to stay a capture
tool.

---

## Contributing

Issues and pull requests are welcome — see [CONTRIBUTING.md](CONTRIBUTING.md).
For security reports, see [SECURITY.md](SECURITY.md).

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option, matching the rest of the Kova family.

Flameshot and ShareX were a functional inspiration; no code from either is used
here.

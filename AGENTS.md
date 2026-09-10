# Kova Screen project instructions

Kova Screen is an independent Rust/Tauri Windows capture application.
This repository owns screenshots, GIF/MP4 recording, clipboard integration and optional upload.
Parent-directory viewer instructions do not apply to this capture project.

## Context and paths

- `README.md`, `CONTRIBUTING.md`, `SECURITY.md`: scope, checks and safety requirements.
- `apps/kova-screen`: application wiring, native windows and hotkeys.
- `crates/kova-screen-core`, `kova-capture`, `kova-encode`, `kova-platform`, `kova-upload`, `kova-history`: domain, capture, encoding, OS, upload and history.
- `ui/`: frontend for settings/history; `Docs/`: dated performance/verification findings.
- Use the Rust version pinned in `rust-toolchain.toml`, Node 20+ and Windows MSVC C++ tools.

## Architecture and safety

- Priority order: capture speed, stability, resource use, simple operation, Windows integration, then styling.
- Capture paths stay in Rust/Win32; the frontend is for settings/history.
- Preserve physical-pixel capture and Per-Monitor-DPI-V2 handling, including mixed-scale monitors.
- Keep the capture overlay excluded from its recording. GIF frames stream to the file without a temporary frame directory.
- Win32 handles need owning guard types with Drop; every unsafe block needs a `SAFETY:` invariant.
- vgy.me keys and deletion links must never reach logs, error/Debug output or frontend data.
- Keep upload optional. Preserve the documented exclusions such as image editing, OCR and accounts.

## Verification and delivery

- Install frontend dependencies with `npm ci` in `ui/`.
- Dev command from root: `npx @tauri-apps/cli@2 dev --config apps/kova-screen/tauri.conf.json`.
- Before pushing: Cargo fmt, workspace/all-target Clippy with warnings denied, `cargo test --workspace -- --test-threads=1`, then UI lint/build.
- Real capture tests use `require_interactive_desktop!()`; report skips on locked/headless sessions.
- Error-path tests must run without that gate. Clipboard/Run-key tests use the existing module mutex.
- Review logs before sharing; use `Docs/` for recorded performance workloads and limitations.
- Preserve per-user installation and settings under `%APPDATA%\Kova Screen`.

# Contributing to Kova Screen

Thanks for taking a look. This document covers what you need to build the
project and what a change is expected to look like.

## Scope

Kova Screen is deliberately small. The priority order is:

1. capture speed
2. stability
3. low resource use
4. simple operation
5. good Windows integration
6. the Kova look

A feature that makes the core more complicated is more likely to be declined
than accepted, however well implemented. The README lists what is explicitly out
of scope; please open an issue before building anything in that direction.

## Setup

- [Rust](https://rustup.rs) — the version is pinned by `rust-toolchain.toml`
- [Node.js](https://nodejs.org) 20 or newer
- MSVC build tools (Visual Studio, *Desktop development with C++*)

```bash
cd ui && npm ci && cd ..
npx @tauri-apps/cli@2 dev --config apps/kova-screen/tauri.conf.json
```

## Before you push

Run what CI runs:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --test-threads=1
cd ui && npm run lint && npm run build
```

All four must pass. Clippy is treated as an error, not a suggestion.

## Tests

Tests are expected with a change, and they should fail for a real reason. A test
that only asserts a function returns `Ok` is not much use; a test that pins down
behaviour someone could plausibly break is.

Some tests capture the real screen. They call `require_interactive_desktop!()`,
which skips them with a printed reason on a locked workstation or a session with
no display — so a skipped run is visible rather than silently green. Tests that
assert *error* handling must not gate on it: those have to hold everywhere.

Tests that touch a single global resource — the clipboard, the `Run` registry
key — serialise on a module-level mutex. Add yours to it rather than hoping the
scheduler is kind.

## Code

Rust is the default. Reach for the frontend only for the settings and history
windows; anything on the capture path belongs in Rust, and usually in Win32.

- `unsafe` needs a `// SAFETY:` comment stating the invariant that makes it
  sound, not a restatement of what the call does.
- Win32 handles belong in a guard type with a `Drop`. This app runs for days;
  a leaked GDI object degrades the whole desktop, not just this process.
- Errors that reach a user should read like a sentence they can act on.
- Secrets — the vgy.me key, upload deletion links — must not reach a log line,
  an error message, a `Debug` rendering, or the frontend. There are tests
  asserting this; keep them passing.

## Commits

Small and logically separate, with a subject line in the imperative:

```
feat: add region capture
fix: keep the capture when the clipboard is locked
ci: build the installer on tags
docs: explain the upload failure wording
```

Explain *why* in the body when the reason is not obvious from the diff.

## Reporting bugs

Please include your Windows version and build, your monitor setup (how many, at
which scaling factors), and what you expected instead. For capture problems,
`KOVA_LOG=debug` produces a verbose log — check it before pasting; it should
contain no secrets, and it is a bug if it does.

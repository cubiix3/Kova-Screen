//! Windows integration for Kova Screen.
//!
//! Everything in this crate is a thin, owned wrapper over a Win32 or WinRT API:
//! clipboard, Credential Manager, global hotkeys, autostart, and the two native
//! overlay windows. Each module keeps its handles in RAII guards, because the
//! app runs for days in the tray and a leaked GDI object or an unreleased
//! clipboard would degrade the whole desktop rather than just this process.

#![cfg(windows)]

pub mod autostart;
pub mod clipboard;
pub mod credentials;
pub mod hotkeys;
pub mod overlay;
mod win32;

pub use hotkeys::{Hotkey, HotkeyFailure, HotkeyManager};
pub use overlay::{RecorderOverlay, RecorderState, Selection};

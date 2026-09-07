//! Small Win32 helpers shared by the overlay windows.

use kova_screen_core::{Error, Result};
use windows::Win32::Foundation::{ERROR_CLASS_ALREADY_EXISTS, GetLastError, HMODULE};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH,
    FF_DONTCARE, FW_SEMIBOLD, HFONT, OUT_TT_PRECIS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{RegisterClassExW, WNDCLASSEXW};
use windows_core::{HSTRING, PCWSTR};

/// The typeface both overlays use.
///
/// Segoe UI Variable Text is the Windows 11 UI face. Windows falls back to
/// Segoe UI automatically on Windows 10, so no explicit fallback is needed.
const UI_TYPEFACE: &str = "Segoe UI Variable Text";

/// The module handle for the running executable.
///
/// Needed when registering a window class and creating a window. It is a
/// borrowed handle owned by the loader, so there is nothing to release.
#[derive(Debug, Clone, Copy)]
pub struct ModuleHandle(HMODULE);

impl ModuleHandle {
    pub fn current() -> Result<Self> {
        // SAFETY: a null name asks for the handle of the current executable,
        // which always exists for a running process.
        let handle = unsafe { GetModuleHandleW(None) }
            .map_err(|e| Error::Platform(format!("could not resolve the module handle: {e}")))?;
        Ok(Self(handle))
    }

    pub fn handle(&self) -> HMODULE {
        self.0
    }
}

/// Builds the UI font at `height` pixels.
///
/// The caller owns the returned `HFONT` and must delete it.
pub fn ui_font(height: i32) -> HFONT {
    // SAFETY: every argument is a documented constant and the face name is a
    // NUL-terminated wide string that outlives the call.
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            FW_SEMIBOLD.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_TT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            &HSTRING::from(UI_TYPEFACE),
        )
    }
}

/// Converts a UTF-8 name into the NUL-terminated buffer a window class needs.
///
/// The returned buffer must outlive every use of the class name, because
/// `WNDCLASSEXW` stores a borrowed pointer rather than copying the string.
pub fn class_name(name: &str) -> Vec<u16> {
    HSTRING::from(name)
        .iter()
        .copied()
        .chain(std::iter::once(0))
        .collect()
}

/// Registers a window class, tolerating a class that already exists.
///
/// Both overlays are opened many times per session, and a class survives for
/// the process lifetime, so the second registration necessarily fails with
/// `ERROR_CLASS_ALREADY_EXISTS`. That is the expected steady state, not an
/// error; anything else is reported.
pub fn register_class(class: &WNDCLASSEXW, description: &str) -> Result<()> {
    // SAFETY: `class` is fully initialised by the caller and its `lpszClassName`
    // points at a buffer the caller keeps alive.
    let atom = unsafe { RegisterClassExW(class) };
    if atom != 0 {
        return Ok(());
    }

    // SAFETY: reads the thread-local error code for the failed call above.
    let last = unsafe { GetLastError() };
    if last == ERROR_CLASS_ALREADY_EXISTS {
        return Ok(());
    }
    Err(Error::Platform(format!(
        "could not register the {description} window class: {last:?}"
    )))
}

/// Convenience wrapper producing a `PCWSTR` for a class-name buffer.
pub fn class_ptr(buffer: &[u16]) -> PCWSTR {
    PCWSTR(buffer.as_ptr())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Gdi::{DeleteObject, HGDIOBJ};

    #[test]
    fn the_current_module_handle_resolves() {
        let module = ModuleHandle::current().expect("a module handle");
        assert!(!module.handle().is_invalid());
    }

    #[test]
    fn the_ui_font_is_created_and_can_be_released() {
        let font = ui_font(15);
        assert!(!font.is_invalid(), "the ui font could not be created");
        // SAFETY: the font is ours and is deleted exactly once.
        unsafe {
            let _ = DeleteObject(HGDIOBJ(font.0));
        }
    }

    #[test]
    fn class_names_are_nul_terminated() {
        let buffer = class_name("KovaTest");
        assert_eq!(buffer.last(), Some(&0));
        assert_eq!(buffer.len(), "KovaTest".len() + 1);
    }
}

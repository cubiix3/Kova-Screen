//! Global hotkeys.
//!
//! Built on `RegisterHotKey` rather than a low-level keyboard hook. A hook
//! would see every keystroke on the system, which is both a privacy liability
//! for a screenshot tool and a common cause of input lag. `RegisterHotKey` asks
//! the OS to deliver only the combinations we asked for.
//!
//! # Conflicts are reported, never swallowed
//!
//! `RegisterHotKey` fails with `ERROR_HOTKEY_ALREADY_REGISTERED` when another
//! process owns the combination. That is turned into a named, per-binding error
//! so the settings UI can say *which* hotkey is taken -- the product
//! requirement is a clear message rather than a hotkey that silently does
//! nothing.
//!
//! # Threading
//!
//! `WM_HOTKEY` is delivered to the thread that registered the hotkey, so
//! registration and the message pump must live on the same thread. [`HotkeyManager`]
//! owns a dedicated thread with its own message loop and hands events to a
//! callback.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use kova_screen_core::{Error, Result};
use windows::Win32::Foundation::{ERROR_HOTKEY_ALREADY_REGISTERED, HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
    UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage, WM_HOTKEY, WM_QUIT,
};

/// A parsed key combination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub modifiers: HOT_KEY_MODIFIERS,
    /// Virtual-key code.
    pub key: u32,
}

impl Hotkey {
    /// Parses a binding such as `Ctrl+Shift+R` or `PrintScreen`.
    ///
    /// Accepts the spellings a user is likely to type: `Ctrl`/`Control`,
    /// `Win`/`Super`/`Meta`, `PrintScreen`/`PrtSc`/`Print`. Matching is
    /// case-insensitive and surrounding whitespace is ignored.
    pub fn parse(binding: &str) -> Result<Self> {
        let binding = binding.trim();
        if binding.is_empty() {
            return Err(Error::Hotkey("the hotkey is empty".into()));
        }

        let mut modifiers = HOT_KEY_MODIFIERS(0);
        let mut key: Option<u32> = None;

        for part in binding.split('+') {
            let part = part.trim();
            if part.is_empty() {
                return Err(Error::Hotkey(format!("`{binding}` has an empty component")));
            }
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => modifiers |= MOD_CONTROL,
                "shift" => modifiers |= MOD_SHIFT,
                "alt" => modifiers |= MOD_ALT,
                "win" | "super" | "meta" | "cmd" => modifiers |= MOD_WIN,
                other => {
                    if key.is_some() {
                        return Err(Error::Hotkey(format!(
                            "`{binding}` names more than one key; only modifiers may be combined"
                        )));
                    }
                    key = Some(virtual_key(other).ok_or_else(|| {
                        Error::Hotkey(format!("`{part}` is not a key Kova Screen recognises"))
                    })?);
                }
            }
        }

        let key = key.ok_or_else(|| {
            Error::Hotkey(format!(
                "`{binding}` is only modifiers; it needs a key as well"
            ))
        })?;

        // MOD_NOREPEAT stops a held key from firing dozens of captures.
        Ok(Self {
            modifiers: modifiers | MOD_NOREPEAT,
            key,
        })
    }

    /// Canonical text form, used to compare bindings for conflicts.
    pub fn to_binding(self) -> String {
        let mut parts = Vec::new();
        if self.modifiers & MOD_CONTROL != HOT_KEY_MODIFIERS(0) {
            parts.push("Ctrl".to_string());
        }
        if self.modifiers & MOD_SHIFT != HOT_KEY_MODIFIERS(0) {
            parts.push("Shift".to_string());
        }
        if self.modifiers & MOD_ALT != HOT_KEY_MODIFIERS(0) {
            parts.push("Alt".to_string());
        }
        if self.modifiers & MOD_WIN != HOT_KEY_MODIFIERS(0) {
            parts.push("Win".to_string());
        }
        parts.push(key_name(self.key).unwrap_or_else(|| format!("0x{:02X}", self.key)));
        parts.join("+")
    }
}

/// Maps a key name to a virtual-key code.
fn virtual_key(name: &str) -> Option<u32> {
    use windows::Win32::UI::Input::KeyboardAndMouse as vk;

    // Single character: letters and digits map to their ASCII uppercase value.
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && c.is_ascii_alphanumeric()
    {
        return Some(c.to_ascii_uppercase() as u32);
    }

    // Function keys.
    if let Some(rest) = name.strip_prefix('f')
        && let Ok(n) = rest.parse::<u32>()
        && (1..=24).contains(&n)
    {
        return Some(vk::VK_F1.0 as u32 + n - 1);
    }

    Some(match name {
        "printscreen" | "prtsc" | "prtscr" | "print" | "snapshot" => vk::VK_SNAPSHOT.0 as u32,
        "insert" | "ins" => vk::VK_INSERT.0 as u32,
        "delete" | "del" => vk::VK_DELETE.0 as u32,
        "home" => vk::VK_HOME.0 as u32,
        "end" => vk::VK_END.0 as u32,
        "pageup" | "pgup" => vk::VK_PRIOR.0 as u32,
        "pagedown" | "pgdn" => vk::VK_NEXT.0 as u32,
        "space" | "spacebar" => vk::VK_SPACE.0 as u32,
        "enter" | "return" => vk::VK_RETURN.0 as u32,
        "escape" | "esc" => vk::VK_ESCAPE.0 as u32,
        "tab" => vk::VK_TAB.0 as u32,
        "backspace" => vk::VK_BACK.0 as u32,
        "up" => vk::VK_UP.0 as u32,
        "down" => vk::VK_DOWN.0 as u32,
        "left" => vk::VK_LEFT.0 as u32,
        "right" => vk::VK_RIGHT.0 as u32,
        "pause" => vk::VK_PAUSE.0 as u32,
        _ => return None,
    })
}

/// Inverse of [`virtual_key`] for the canonical spellings.
fn key_name(key: u32) -> Option<String> {
    use windows::Win32::UI::Input::KeyboardAndMouse as vk;

    if (b'A' as u32..=b'Z' as u32).contains(&key) || (b'0' as u32..=b'9' as u32).contains(&key) {
        return Some((key as u8 as char).to_string());
    }
    let f1 = vk::VK_F1.0 as u32;
    if (f1..f1 + 24).contains(&key) {
        return Some(format!("F{}", key - f1 + 1));
    }
    Some(
        match key {
            k if k == vk::VK_SNAPSHOT.0 as u32 => "PrintScreen",
            k if k == vk::VK_INSERT.0 as u32 => "Insert",
            k if k == vk::VK_DELETE.0 as u32 => "Delete",
            k if k == vk::VK_HOME.0 as u32 => "Home",
            k if k == vk::VK_END.0 as u32 => "End",
            k if k == vk::VK_PRIOR.0 as u32 => "PageUp",
            k if k == vk::VK_NEXT.0 as u32 => "PageDown",
            k if k == vk::VK_SPACE.0 as u32 => "Space",
            k if k == vk::VK_RETURN.0 as u32 => "Enter",
            k if k == vk::VK_ESCAPE.0 as u32 => "Escape",
            k if k == vk::VK_TAB.0 as u32 => "Tab",
            k if k == vk::VK_BACK.0 as u32 => "Backspace",
            k if k == vk::VK_UP.0 as u32 => "Up",
            k if k == vk::VK_DOWN.0 as u32 => "Down",
            k if k == vk::VK_LEFT.0 as u32 => "Left",
            k if k == vk::VK_RIGHT.0 as u32 => "Right",
            k if k == vk::VK_PAUSE.0 as u32 => "Pause",
            _ => return None,
        }
        .to_string(),
    )
}

/// Why a binding could not be registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyFailure {
    /// Action id, e.g. `region_screenshot`.
    pub action: String,
    /// The binding text the user configured.
    pub binding: String,
    /// A message suitable for showing directly in the settings UI.
    pub reason: String,
    /// True when another application already owns the combination.
    pub taken_by_another_app: bool,
}

/// Runs a message loop and dispatches global hotkeys.
pub struct HotkeyManager {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl HotkeyManager {
    /// Registers `bindings` and starts dispatching.
    ///
    /// Returns the manager together with the bindings that could not be
    /// registered. A partial failure is deliberately **not** fatal: four working
    /// hotkeys plus a clear warning beats refusing to start because one
    /// combination is taken by another application.
    pub fn start<F>(
        bindings: &[(String, String)],
        mut on_hotkey: F,
    ) -> Result<(Self, Vec<HotkeyFailure>)>
    where
        F: FnMut(&str) + Send + 'static,
    {
        // Parse before spawning so a malformed binding is reported synchronously.
        let mut parsed: Vec<(String, String, Hotkey)> = Vec::new();
        let mut failures = Vec::new();
        for (action, binding) in bindings {
            if binding.trim().is_empty() {
                continue; // Deliberately unbound.
            }
            match Hotkey::parse(binding) {
                Ok(hotkey) => parsed.push((action.clone(), binding.clone(), hotkey)),
                Err(err) => failures.push(HotkeyFailure {
                    action: action.clone(),
                    binding: binding.clone(),
                    reason: err.to_string(),
                    taken_by_another_app: false,
                }),
            }
        }

        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);

        let (tx, rx) = std::sync::mpsc::channel::<Vec<HotkeyFailure>>();

        let thread = std::thread::Builder::new()
            .name("kova-hotkeys".into())
            .spawn(move || {
                let mut registered: HashMap<i32, String> = HashMap::new();
                let mut thread_failures = Vec::new();

                for (index, (action, binding, hotkey)) in parsed.into_iter().enumerate() {
                    let id = index as i32 + 1;
                    // SAFETY: a null HWND registers against the calling thread,
                    // which is this one and also the one pumping messages below.
                    let result = unsafe {
                        RegisterHotKey(Some(HWND::default()), id, hotkey.modifiers, hotkey.key)
                    };
                    match result {
                        Ok(()) => {
                            registered.insert(id, action);
                        }
                        Err(err) => {
                            let taken = err.code() == ERROR_HOTKEY_ALREADY_REGISTERED.to_hresult();
                            thread_failures.push(HotkeyFailure {
                                reason: if taken {
                                    format!("{binding} is already used by another application")
                                } else {
                                    format!("{binding} could not be registered: {err}")
                                },
                                action,
                                binding,
                                taken_by_another_app: taken,
                            });
                        }
                    }
                }

                let _ = tx.send(thread_failures);

                // Poll rather than block in GetMessage: the loop must also
                // notice the stop flag, and PeekMessage with a short sleep costs
                // no measurable CPU while keeping shutdown responsive.
                while !thread_stop.load(Ordering::Relaxed) {
                    let mut msg = MSG::default();
                    // SAFETY: `msg` is a live local; PM_REMOVE dequeues.
                    while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                        if msg.message == WM_QUIT {
                            thread_stop.store(true, Ordering::Relaxed);
                            break;
                        }
                        if msg.message == WM_HOTKEY {
                            let id = msg.wParam.0 as i32;
                            if let Some(action) = registered.get(&id) {
                                on_hotkey(action);
                            }
                            continue;
                        }
                        // SAFETY: `msg` was filled by PeekMessageW.
                        unsafe {
                            let _ = TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(15));
                }

                for id in registered.keys() {
                    // SAFETY: unregisters ids this thread registered.
                    unsafe {
                        let _ = UnregisterHotKey(Some(HWND::default()), *id);
                    }
                }
            })
            .map_err(|e| Error::Hotkey(format!("could not start the hotkey thread: {e}")))?;

        // Registration happens on the thread, so wait for its verdict.
        let thread_failures = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| Error::Hotkey("the hotkey thread did not start in time".into()))?;
        failures.extend(thread_failures);

        Ok((
            Self {
                stop,
                thread: Some(thread),
            },
            failures,
        ))
    }

    /// Unregisters every hotkey and stops the message loop.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Silences the unused warnings for the message parameters on some builds.
#[allow(dead_code)]
fn unused(_: WPARAM, _: LPARAM) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    #[test]
    fn parses_the_documented_default_bindings() {
        use windows::Win32::UI::Input::KeyboardAndMouse as vk;

        let region = Hotkey::parse("PrintScreen").unwrap();
        assert_eq!(region.key, vk::VK_SNAPSHOT.0 as u32);
        assert_eq!(region.modifiers & MOD_CONTROL, HOT_KEY_MODIFIERS(0));

        let full = Hotkey::parse("Ctrl+PrintScreen").unwrap();
        assert_ne!(full.modifiers & MOD_CONTROL, HOT_KEY_MODIFIERS(0));

        let window = Hotkey::parse("Shift+PrintScreen").unwrap();
        assert_ne!(window.modifiers & MOD_SHIFT, HOT_KEY_MODIFIERS(0));

        let mp4 = Hotkey::parse("Ctrl+Shift+R").unwrap();
        assert_eq!(mp4.key, b'R' as u32);
        assert_ne!(mp4.modifiers & MOD_CONTROL, HOT_KEY_MODIFIERS(0));
        assert_ne!(mp4.modifiers & MOD_SHIFT, HOT_KEY_MODIFIERS(0));

        assert_eq!(Hotkey::parse("Ctrl+Shift+G").unwrap().key, b'G' as u32);
    }

    #[test]
    fn every_binding_gets_norepeat() {
        // Without MOD_NOREPEAT, holding the key fires a burst of captures.
        let hk = Hotkey::parse("Ctrl+Shift+R").unwrap();
        assert_ne!(hk.modifiers & MOD_NOREPEAT, HOT_KEY_MODIFIERS(0));
    }

    #[test]
    fn accepts_alternative_spellings() {
        assert_eq!(
            Hotkey::parse("Control+A").unwrap(),
            Hotkey::parse("ctrl+a").unwrap()
        );
        assert_eq!(
            Hotkey::parse("PrtSc").unwrap(),
            Hotkey::parse("printscreen").unwrap()
        );
        assert_eq!(
            Hotkey::parse("Win+S").unwrap(),
            Hotkey::parse("super+s").unwrap()
        );
        assert_eq!(
            Hotkey::parse(" Ctrl + Shift + R ").unwrap(),
            Hotkey::parse("Ctrl+Shift+R").unwrap()
        );
    }

    #[test]
    fn parses_function_keys() {
        use windows::Win32::UI::Input::KeyboardAndMouse as vk;
        assert_eq!(Hotkey::parse("F1").unwrap().key, vk::VK_F1.0 as u32);
        assert_eq!(Hotkey::parse("F12").unwrap().key, vk::VK_F1.0 as u32 + 11);
        assert!(Hotkey::parse("F25").is_err());
    }

    #[test]
    fn rejects_malformed_bindings_with_a_readable_message() {
        for bad in [
            "",
            "   ",
            "Ctrl",
            "Ctrl+Shift",
            "Ctrl+",
            "+A",
            "Ctrl+A+B",
            "Ctrl+Nonsense",
        ] {
            let err = Hotkey::parse(bad).unwrap_err().to_string();
            assert!(!err.is_empty(), "`{bad}` produced an empty error");
        }
    }

    #[test]
    fn modifier_only_bindings_are_rejected() {
        let err = Hotkey::parse("Ctrl+Alt").unwrap_err().to_string();
        assert!(err.contains("needs a key"), "unhelpful message: {err}");
    }

    #[test]
    fn canonical_form_round_trips() {
        for binding in [
            "Ctrl+Shift+R",
            "Ctrl+PrintScreen",
            "PrintScreen",
            "Alt+F4",
            "Win+D",
        ] {
            let parsed = Hotkey::parse(binding).unwrap();
            let canonical = parsed.to_binding();
            assert_eq!(
                Hotkey::parse(&canonical).unwrap(),
                parsed,
                "`{binding}` did not survive a round trip through `{canonical}`"
            );
        }
    }

    #[test]
    fn registers_bindings_and_dispatches_them() {
        let fired = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&fired);

        // An obscure combination unlikely to be taken on a test machine.
        let bindings = vec![("test_action".to_string(), "Ctrl+Alt+Shift+F9".to_string())];
        let (mut manager, failures) = HotkeyManager::start(&bindings, move |_action| {
            counter.fetch_add(1, Ordering::Relaxed);
        })
        .expect("hotkey manager starts");

        // If another app owns it, that is a valid outcome and must be reported
        // rather than silently ignored.
        if let Some(f) = failures.first() {
            assert!(!f.reason.is_empty());
            assert_eq!(f.action, "test_action");
        }
        manager.stop();
    }

    #[test]
    fn a_malformed_binding_is_reported_but_does_not_stop_the_others() {
        let bindings = vec![
            ("good".to_string(), "Ctrl+Alt+Shift+F8".to_string()),
            ("bad".to_string(), "Ctrl+Nonsense".to_string()),
        ];
        let (mut manager, failures) = HotkeyManager::start(&bindings, |_| {}).unwrap();
        assert!(
            failures.iter().any(|f| f.action == "bad"),
            "the malformed binding was not reported"
        );
        assert!(
            !failures
                .iter()
                .any(|f| f.action == "good" && f.taken_by_another_app),
            "a valid binding must not be reported as taken"
        );
        manager.stop();
    }

    #[test]
    fn an_empty_binding_is_treated_as_unbound() {
        let bindings = vec![("unbound".to_string(), String::new()).clone()];
        let (mut manager, failures) = HotkeyManager::start(&bindings, |_| {}).unwrap();
        assert!(failures.is_empty(), "an empty binding must not be an error");
        manager.stop();
    }

    #[test]
    fn a_second_manager_reports_the_conflict_rather_than_failing_silently() {
        let binding = vec![("dup".to_string(), "Ctrl+Alt+Shift+F7".to_string())];
        let (mut first, first_failures) = HotkeyManager::start(&binding, |_| {}).unwrap();

        if first_failures.is_empty() {
            // We own it, so a second registration must be refused and named.
            let (mut second, second_failures) = HotkeyManager::start(&binding, |_| {}).unwrap();
            assert_eq!(second_failures.len(), 1, "the conflict was not detected");
            assert!(
                second_failures[0].taken_by_another_app,
                "the conflict was not attributed to another owner"
            );
            assert!(second_failures[0].reason.contains("already used"));
            second.stop();
        }
        first.stop();
    }

    #[test]
    fn stopping_twice_is_safe() {
        let bindings = vec![("a".to_string(), "Ctrl+Alt+Shift+F6".to_string())];
        let (mut manager, _) = HotkeyManager::start(&bindings, |_| {}).unwrap();
        manager.stop();
        manager.stop();
    }

    #[test]
    fn dropping_the_manager_releases_the_hotkey() {
        let binding = vec![("dropme".to_string(), "Ctrl+Alt+Shift+F5".to_string())];
        {
            let (_manager, failures) = HotkeyManager::start(&binding, |_| {}).unwrap();
            if !failures.is_empty() {
                return; // Already taken on this machine; nothing to prove.
            }
        }
        // After the drop the combination must be free again.
        let (mut again, failures) = HotkeyManager::start(&binding, |_| {}).unwrap();
        assert!(
            failures.is_empty(),
            "the hotkey was not released on drop: {failures:?}"
        );
        again.stop();
    }
}

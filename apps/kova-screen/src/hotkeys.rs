//! Global hotkey registration for the app.
//!
//! Wraps [`kova_platform::HotkeyManager`] with the app-specific bits: mapping
//! action ids to [`Action`]s, keeping the manager alive for the process
//! lifetime, and recording the bindings that could not be registered so the
//! settings window can explain *which* shortcut is taken and by what.
//!
//! Re-registration replaces the whole manager rather than diffing bindings.
//! Windows has no way to change a registered hotkey in place, and a full
//! rebuild takes a few milliseconds on a path the user hits only when they edit
//! a shortcut.

use std::sync::Arc;

use parking_lot::Mutex;
use tauri::{AppHandle, Runtime};

use crate::actions::{self, Action};
use crate::app::AppState;

/// The live manager. Held for the process lifetime; dropping it unregisters
/// every hotkey, which is exactly what should happen on exit.
static MANAGER: Mutex<Option<kova_platform::HotkeyManager>> = Mutex::new(None);

/// Registers the configured hotkeys.
///
/// Never fails the caller: a binding that cannot be registered is recorded on
/// [`AppState`] and surfaced in settings. Refusing to start because one
/// shortcut is taken would be far worse than starting with five working ones.
pub fn register<R: Runtime>(_app: &AppHandle<R>, state: &Arc<AppState>) {
    replace_manager(state);
}

fn replace_manager(state: &Arc<AppState>) {
    // Serialize the entire replacement, including unregistration and startup.
    // Otherwise concurrent settings saves can register against a manager that
    // another save has just installed, then replace it with a failed manager.
    let mut manager_slot = MANAGER.lock();
    drop(manager_slot.take());
    let settings = state.settings();
    let bindings: Vec<(String, String)> = settings
        .hotkeys
        .bindings()
        .iter()
        .map(|(id, binding)| ((*id).to_string(), (*binding).to_string()))
        .collect();

    let dispatch_state = Arc::clone(state);
    let result = kova_platform::HotkeyManager::start(&bindings, move |id| {
        match Action::from_hotkey_id(id) {
            Some(action) => actions::dispatch(Arc::clone(&dispatch_state), action),
            None => tracing::warn!(id, "a hotkey fired for an action that no longer exists"),
        }
    });

    match result {
        Ok((manager, failures)) => {
            for failure in &failures {
                tracing::warn!(
                    action = %failure.action,
                    binding = %failure.binding,
                    "a hotkey could not be registered: {}",
                    failure.reason
                );
            }
            state.set_hotkey_failures(failures);
            *manager_slot = Some(manager);
        }
        Err(err) => {
            tracing::error!(%err, "no hotkeys could be registered");
            state.set_hotkey_failures(vec![kova_platform::HotkeyFailure {
                action: "all".into(),
                binding: String::new(),
                reason: err.to_string(),
                taken_by_another_app: false,
            }]);
        }
    }
}

/// Drops the current manager and registers the current settings again.
pub fn reregister<R: Runtime>(app: &AppHandle<R>, state: &Arc<AppState>) {
    register(app, state);
}

/// Unregisters every hotkey. Called on exit.
pub fn shutdown() {
    *MANAGER.lock() = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_replacements_keep_the_binding_registered() {
        // An obscure combination: the property under test is the replacement
        // serialisation, not the binding, and a bare key like Home is regularly
        // taken by other software on a developer machine.
        let binding_text = "Ctrl+Alt+Shift+F3";
        let mut settings = kova_screen_core::settings::Settings::default();
        settings.hotkeys.region_screenshot = binding_text.into();
        let state = AppState::for_test(settings);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let state = Arc::clone(&state);
                scope.spawn(move || {
                    for _ in 0..5 {
                        replace_manager(&state);
                    }
                });
            }
        });
        assert!(
            !state
                .hotkey_failures()
                .iter()
                .any(|failure| failure.binding == binding_text)
        );
        let binding = vec![("probe".into(), binding_text.into())];
        let (probe, failures) = kova_platform::HotkeyManager::start(&binding, |_| {}).unwrap();
        assert!(
            failures.iter().any(|failure| failure.taken_by_another_app),
            "{binding_text} lost its registration"
        );
        drop(probe);
        shutdown();
        let (probe, failures) = kova_platform::HotkeyManager::start(&binding, |_| {}).unwrap();
        assert!(failures.is_empty(), "{binding_text} was not released");
        drop(probe);
    }

    #[test]
    fn the_default_bindings_all_map_to_actions() {
        let settings = kova_screen_core::settings::Settings::default();
        for (id, binding) in settings.hotkeys.bindings() {
            assert!(
                Action::from_hotkey_id(id).is_some(),
                "`{id}` ({binding}) has no action"
            );
        }
    }

    #[test]
    fn shutdown_is_safe_when_nothing_was_registered() {
        shutdown();
        shutdown();
    }
}

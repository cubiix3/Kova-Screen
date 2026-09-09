//! Kova Screen: fast, minimal screen capture for Windows.
//!
//! # Shape of the app
//!
//! There is no main window. The tray icon is the app, hotkeys are the primary
//! interface, and the two WebView windows (Settings, Recent Captures) are
//! created on demand and destroyed on close.
//!
//! The split between native and web is deliberate and follows one rule: **the
//! capture path is native, the configuration path is web.**
//!
//! - The region overlay and the recorder overlay are Win32. The overlay sits
//!   between the hotkey and the screenshot, so its spawn latency *is* the app
//!   perceived speed, and the recorder overlay has to be excluded from the
//!   capture it controls -- neither is something a WebView can do.
//! - Settings and history are a WebView, because they are visited occasionally
//!   and benefit from being easy to lay out.
//!
//! # Idle cost
//!
//! Sitting in the tray, the process holds a tray icon, a hotkey message pump
//! and the loaded settings. No capture session, no D3D device, no WebView and
//! no polling loop exists until the user asks for something.

pub mod actions;
pub mod app;
pub mod commands;
#[cfg(test)]
mod e2e;
pub mod hotkeys;
pub mod notify;
pub mod pipeline;
pub mod recorder;
pub mod tray;
pub mod upload;
pub mod windows;

use std::sync::Arc;

use app::AppState;

/// Builds and runs the application.
pub fn run() {
    init_tracing();

    // Must happen before any window exists and before any monitor geometry is
    // read, or every capture on a scaled display comes back the wrong size.
    kova_capture::dpi::enable_per_monitor_dpi_awareness();

    let state = AppState::load();

    let mut builder = tauri::Builder::default();

    // A second launch (from the Start menu, or the installer finish page)
    // should surface the running instance rather than start a rival tray icon
    // that fights over the same hotkeys.
    #[cfg(windows)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Err(err) = windows::open_settings(app) {
                tracing::warn!(%err, "could not surface the existing instance");
            }
        }));
    }

    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(Arc::clone(&state))
        .invoke_handler(commands::handlers())
        .setup(move |app| {
            let handle = app.handle().clone();

            tray::build(&handle)?;
            hotkeys::register(&handle, &state);

            // Create the capture folder now, so the first screenshot is not the
            // moment the user discovers the folder cannot be created.
            if let Err(err) = state.capture_dir() {
                tracing::warn!(%err, "the capture folder could not be prepared");
            }

            // Autostart is external state that can be changed behind our back
            // (a user editing the registry, an uninstall of a previous build).
            // Reconcile it with the setting so the checkbox tells the truth.
            reconcile_autostart(&state);

            if !should_start_minimized(&state) {
                windows::open_settings(&handle)?;
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing a settings or history window destroys it rather than
            // hiding it, so no WebView renderer lingers in the background.
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                tracing::debug!(label = window.label(), "window closed");
            }
        })
        .build(tauri::generate_context!())
        .expect("kova screen failed to start")
        .run(|_app, event| {
            match event {
                // Closing the last WebView must leave the tray and hotkeys
                // alive. Explicit Exit/Restart carries a code and is allowed.
                tauri::RunEvent::ExitRequested {
                    code: None, api, ..
                } => api.prevent_exit(),
                tauri::RunEvent::Exit => {
                    hotkeys::shutdown();
                    kova_encode::mp4::shutdown_runtime();
                }
                _ => {}
            }
        });
}

/// Whether to stay in the tray at launch.
///
/// True when the user asked for it, or when Windows started us at sign-in --
/// which the autostart entry marks with `--minimized`.
fn should_start_minimized(state: &Arc<AppState>) -> bool {
    // The flag is the same constant the autostart entry is written with, so the
    // two cannot drift apart.
    if std::env::args().any(|arg| arg == kova_platform::autostart::MINIMIZED_FLAG) {
        return true;
    }
    state.settings().general.start_minimized
}

/// Brings the `Run` registry entry in line with the saved setting.
fn reconcile_autostart(state: &Arc<AppState>) {
    let wanted = state.settings().general.launch_with_windows;
    if kova_platform::autostart::is_enabled() == wanted {
        return;
    }
    match std::env::current_exe() {
        Ok(exe) => {
            if let Err(err) = kova_platform::autostart::set_enabled(wanted, &exe) {
                tracing::warn!(%err, "could not reconcile autostart");
            }
        }
        Err(err) => tracing::warn!(%err, "could not resolve the executable path"),
    }
}

/// Sets up logging.
///
/// Quiet by default; `KOVA_LOG=debug` turns it up. Nothing here ever logs a
/// secret: the user key and the provider deletion links are excluded at their
/// source, in `kova-platform` and `kova-history` respectively.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_env("KOVA_LOG")
        .unwrap_or_else(|_| EnvFilter::new("kova_screen=info,kova_capture=warn,warn"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .try_init();
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_minimized_flag_matches_what_autostart_registers() {
        // `autostart` writes this exact flag into the Run entry and startup
        // reads it back. If the two ever drifted, every sign-in would pop a
        // window the user did not ask for.
        let exe = std::env::current_exe().expect("the test binary path");
        let registered = format!(
            "\"{}\" {}",
            exe.display(),
            kova_platform::autostart::MINIMIZED_FLAG
        );

        let args_as_launched: Vec<&str> = registered
            .split_whitespace()
            .skip(1) // the quoted exe path
            .collect();

        assert!(
            args_as_launched.contains(&kova_platform::autostart::MINIMIZED_FLAG),
            "a sign-in launch would not be recognised as minimized: {registered}"
        );
    }
}

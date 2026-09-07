// Kova Screen runs in the tray and must not flash a console window when it is
// started from the Start menu or at sign-in. The `console` subsystem is kept
// for debug builds so `cargo run` still shows tracing output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    kova_screen_lib::run();
}

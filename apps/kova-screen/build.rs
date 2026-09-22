fn main() {
    tauri_build::build();
    delay_load_common_controls();
}

/// Resolves comctl32 on first use instead of at process start.
///
/// The dialog plugin imports `TaskDialogIndirect`, which only the version 6
/// common controls export, and Windows binds version 6 only for a binary that
/// embeds the common-controls manifest. The application has that manifest; the
/// library test harness does not, and on a clean machine it would not start.
/// Delay-loading defers the lookup to the first call: the application still
/// gets version 6 through its manifest, and the tests never call a dialog.
fn delay_load_common_controls() {
    let target = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() != Some("windows") || target != "msvc" {
        return;
    }
    println!("cargo:rustc-link-arg=/DELAYLOAD:comctl32.dll");
    println!("cargo:rustc-link-lib=delayimp");
}

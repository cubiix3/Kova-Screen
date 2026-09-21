fn main() {
    tauri_build::build();
    compile_test_manifest();
}

/// Writes a common-controls manifest for the library test harness.
///
/// Tauri embeds that manifest in the application binary only. The harness
/// still imports `TaskDialogIndirect`, and Windows will not start it without
/// the manifest. `rc.exe` comes from the Windows SDK already required to link.
fn compile_test_manifest() {
    if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() != Some("windows") {
        return;
    }
    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("../../target"));
    let res = target_dir.join("kova-screen-tests.res");
    if let Some(parent) = res.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let status = std::process::Command::new("rc.exe")
        .current_dir(&manifest_dir)
        .arg("/nologo")
        .arg(format!("/fo{}", res.display()))
        .arg("windows-test-manifest.rc")
        .status()
        .expect("rc.exe was not found; the Windows SDK is required to build tests");
    if !status.success() {
        panic!("rc.exe could not compile the test manifest");
    }
    println!("cargo:rerun-if-changed=windows-test-manifest.rc");
    println!("cargo:rerun-if-changed=windows-test-manifest.xml");
}

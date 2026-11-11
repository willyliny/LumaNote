fn main() {
    // Put our own commands under the ACL so a window can only call the ones its
    // capability file allows (generates `allow-app-info`, `allow-open-window`, `allow-set-floating-expanded`).
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "app_info",
            "open_window",
            "set_floating_expanded",
        ]),
    ))
    .expect("failed to run tauri-build");
}

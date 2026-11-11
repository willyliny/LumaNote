fn main() {
    // Put our own commands under the ACL so a window can only call the ones its
    // capability file allows (generates `allow-app-info` / `allow-open-window`).
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(
            tauri_build::AppManifest::new().commands(&["app_info", "open_window"]),
        ),
    )
    .expect("failed to run tauri-build");
}

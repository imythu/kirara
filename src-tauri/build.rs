fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&["api_request", "logs_open", "logs_close"]),
    ))
    .expect("failed to build the desktop application");
}

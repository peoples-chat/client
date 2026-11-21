//! App shell. It hosts the web view and forwards commands to the Rust core.
//! No protocol logic belongs here.

/// Version of the linked core. The placeholder screen calls this once at start.
#[tauri::command]
fn core_version() -> &'static str {
    let version = peoples_chat_core::version();
    log::info!("core_version requested by the interface: {version}");
    version
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().plugin(
        tauri_plugin_log::Builder::new()
            .level(log::LevelFilter::Info)
            .build(),
    );

    // Scanning QR codes with the camera. The plugin exists for iOS and Android only.
    #[cfg(mobile)]
    let builder = builder.plugin(tauri_plugin_barcode_scanner::init());

    builder
        .invoke_handler(tauri::generate_handler![core_version])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

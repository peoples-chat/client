//! App shell. It hosts the web view and forwards commands to the Rust core.
//! No protocol logic belongs here.

#[cfg(debug_assertions)]
mod spike;

/// Stack size of the async runtime's threads. Unoptimized OpenMLS, inside MDK, can outgrow the
/// default 2 MiB when it creates a group; MDK's own runtime tests use 8 MiB. `RUST_MIN_STACK` in
/// `.cargo/config.toml` does not help here: it reaches only processes Cargo starts, not an app
/// that a phone starts.
const WORKER_STACK: usize = 8 * 1024 * 1024;

/// Version of the linked core. The placeholder screen calls this once at start.
#[tauri::command]
fn core_version() -> &'static str {
    let version = peoples_chat_core::version();
    log::info!("core_version requested by the interface: {version}");
    version
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Tauri runs commands and MDK's tasks on this runtime. It lives as long as the process.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(WORKER_STACK)
        .build()
        .expect("failed to build the async runtime");
    tauri::async_runtime::set(Box::leak(Box::new(runtime)).handle().clone());

    let builder = tauri::Builder::default().plugin(
        tauri_plugin_log::Builder::new()
            .level(log::LevelFilter::Info)
            .build(),
    );

    // Scanning QR codes with the camera. The plugin exists for iOS and Android only.
    #[cfg(mobile)]
    let builder = builder.plugin(tauri_plugin_barcode_scanner::init());

    #[cfg(debug_assertions)]
    let builder = builder
        .setup(|app| {
            spike::run_from_env(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            core_version,
            spike::spike_relay_round_trip
        ]);
    #[cfg(not(debug_assertions))]
    let builder = builder.invoke_handler(tauri::generate_handler![core_version]);

    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

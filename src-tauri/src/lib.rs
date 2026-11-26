//! App shell. It hosts the web view and forwards commands to the Rust core.
//! No protocol logic belongs here.

mod bridge;

// The e2e plugins open a WebDriver server inside the app: never in a release build.
#[cfg(all(feature = "e2e", not(debug_assertions)))]
compile_error!("the e2e feature is for debug builds only (pnpm e2e builds with --debug)");

/// Stack size of the async runtime's threads. Unoptimized OpenMLS, inside MDK, can outgrow the
/// default 2 MiB when it creates a group; MDK's own runtime tests use 8 MiB. `RUST_MIN_STACK` in
/// `.cargo/config.toml` does not help here: it reaches only processes Cargo starts, not an app
/// that a phone starts.
const WORKER_STACK: usize = 8 * 1024 * 1024;

/// Version of the linked core, shown at the bottom of Settings.
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

    // The device unlock before a key export (#86): Android, iOS and macOS.
    let builder = tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_biometric::init());

    // Scanning QR codes with the camera, the system share sheet, and opening
    // https://peoples.chat/i links in the app (#72). All three are for iOS and Android only.
    #[cfg(mobile)]
    let builder = builder
        .plugin(tauri_plugin_barcode_scanner::init())
        .plugin(tauri_plugin_share_sheet::init())
        .plugin(tauri_plugin_deep_link::init());

    // Desktop end-to-end tests (pnpm e2e): WebdriverIO drives the app through these.
    #[cfg(feature = "e2e")]
    let builder = builder
        .plugin(tauri_plugin_wdio::init())
        .plugin(tauri_plugin_wdio_webdriver::init());

    builder
        .setup(|app| {
            // Added here rather than in capabilities/, so builds without the plugins never name them.
            #[cfg(feature = "e2e")]
            {
                use tauri::Manager;
                app.add_capability(include_str!("../e2e-capability.json"))?;
            }
            // The core starts with the app, not when the web view first asks for it.
            bridge::start(app.handle())?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            core_version,
            bridge::core_snapshot,
            bridge::create_account,
            bridge::update_profile,
            bridge::set_profile_photo,
            bridge::remove_profile_photo,
            bridge::delete_account,
            bridge::add_contact,
            bridge::create_invite,
            bridge::preview_invite,
            bridge::accept_invitation,
            bridge::send_message,
            bridge::set_force_radio,
            bridge::cancel_message,
            bridge::export_account_key,
            // Pairing a radio over USB (#24). Phones will pair over Bluetooth (#23).
            #[cfg(desktop)]
            bridge::radio_ports,
            #[cfg(desktop)]
            bridge::pair_radio,
            #[cfg(desktop)]
            bridge::forget_radio
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

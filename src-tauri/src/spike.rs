//! Debug builds only: runs the core's messaging inside the app against a relay, to check that
//! MDK works on each platform (#15). It is not part of the product.
//!
//! Two ways to start it:
//! - At launch, when `PEOPLES_CHAT_SPIKE_RELAY` holds a relay URL. On the iOS simulator, pass it as
//!   `SIMCTL_CHILD_PEOPLES_CHAT_SPIKE_RELAY` to `xcrun simctl launch`.
//! - From the web view: `__TAURI_INTERNALS__.invoke("spike_relay_round_trip", { relay })`.
//!
//! Each run uses a fresh folder under `spike/` in the app's data folder. The outcome goes to the
//! log and to `spike/latest.json`. Findings: `docs/spikes/15-mdk-on-phones.md`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use peoples_chat_core::{Messaging, MessagingOptions};
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};

const RELAY_ENV: &str = "PEOPLES_CHAT_SPIKE_RELAY";

#[derive(Debug, Serialize)]
pub struct SpikeReport {
    os: &'static str,
    arch: &'static str,
    relay: String,
    data_dir: PathBuf,
    public_key_hex: String,
    open_ms: u128,
    create_ms: u128,
    publish_ms: u128,
    close_ms: u128,
}

/// Opens the core's messaging in a fresh folder, creates an account and waits until the relay
/// has acknowledged its relay lists, profile and KeyPackage.
async fn run<R: Runtime>(app: &AppHandle<R>, relay: String) -> Result<SpikeReport, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let data_dir = spike_root(app)?.join(stamp.to_string());
    std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;

    let started = Instant::now();
    let messaging = Messaging::open(MessagingOptions {
        data_dir: data_dir.clone(),
        relays: vec![relay.clone()],
        keychain_service: "chat.peoples.client.spike".to_owned(),
        allow_loopback_relays: true,
    })
    .await
    .map_err(|e| format!("open: {e}"))?;
    let open_ms = started.elapsed().as_millis();

    let started = Instant::now();
    let account = messaging
        .create_account("Spike")
        .await
        .map_err(|e| format!("create account: {e}"))?;
    let create_ms = started.elapsed().as_millis();

    let started = Instant::now();
    messaging
        .wait_until_published(&account, Duration::from_secs(60))
        .await
        .map_err(|e| format!("publish: {e}"))?;
    let publish_ms = started.elapsed().as_millis();

    let started = Instant::now();
    messaging.close().await.map_err(|e| format!("close: {e}"))?;
    let close_ms = started.elapsed().as_millis();

    Ok(SpikeReport {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        relay,
        data_dir,
        public_key_hex: account.public_key_hex,
        open_ms,
        create_ms,
        publish_ms,
        close_ms,
    })
}

fn spike_root<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("spike"))
}

/// Logs the outcome and writes it to `spike/latest.json`.
fn record<R: Runtime>(app: &AppHandle<R>, outcome: &Result<SpikeReport, String>) {
    let json = match outcome {
        Ok(report) => serde_json::json!({ "ok": true, "report": report }),
        Err(error) => serde_json::json!({ "ok": false, "error": error }),
    };
    log::info!("spike: {json}");
    if let Ok(root) = spike_root(app) {
        let _ = std::fs::create_dir_all(&root);
        let _ = write(&root.join("latest.json"), &json);
    }
}

fn write(path: &Path, json: &serde_json::Value) -> std::io::Result<()> {
    std::fs::write(path, serde_json::to_vec_pretty(json)?)
}

#[tauri::command]
pub async fn spike_relay_round_trip<R: Runtime>(
    app: AppHandle<R>,
    relay: String,
) -> Result<SpikeReport, String> {
    let outcome = run(&app, relay).await;
    record(&app, &outcome);
    outcome
}

/// Starts a run at launch when [`RELAY_ENV`] is set.
pub fn run_from_env<R: Runtime>(app: &AppHandle<R>) {
    let Ok(relay) = std::env::var(RELAY_ENV) else {
        return;
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let outcome = run(&app, relay).await;
        record(&app, &outcome);
    });
}

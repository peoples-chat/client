//! The bridge between the web view and the core (#16). The shell starts the core when the app
//! launches, passes every core event to the web view as the Tauri event `core-event`, and turns
//! the web view's commands into calls on the core. The contract: `docs/core-interface.md`.

use peoples_chat_core::interface::{Account, CommandError, CoreEvent, Snapshot};
use peoples_chat_core::{Core, MessagingOptions};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::sync::OnceCell;
use tokio::sync::broadcast::{self, error::RecvError};

/// The Tauri event that carries core events to the web view.
const EVENT: &str = "core-event";

/// Keychain service for account keys: the app's identifier.
const KEYCHAIN_SERVICE: &str = "chat.peoples.client";

/// Relays for new accounts, comma-separated, taken from the environment when the app is built.
const RELAYS: Option<&str> = option_env!("PEOPLES_CHAT_RELAYS");

/// Debug builds without `PEOPLES_CHAT_RELAYS` use the relay that `core/examples/local_relay.rs`
/// runs on the Mac. MDK needs a relay to set an account up, though not a reachable one.
const DEV_RELAY: &str = "ws://127.0.0.1:7777";

/// The core, once it has started. Commands that arrive while it starts wait for it.
pub struct CoreSlot {
    core: OnceCell<Core>,
    options: MessagingOptions,
}

impl CoreSlot {
    /// The started core. Starts it if nobody has yet, and then starts passing its events on.
    async fn get<R: Runtime>(&self, app: &AppHandle<R>) -> Result<&Core, CommandError> {
        self.core
            .get_or_try_init(|| async {
                std::fs::create_dir_all(&self.options.data_dir).map_err(start_failed)?;
                let core = Core::start(self.options.clone())
                    .await
                    .map_err(start_failed)?;
                forward_events(app.clone(), core.subscribe());
                log::info!("core started");
                Ok(core)
            })
            .await
    }
}

/// Starts the core at launch, before and without any web view asking for it.
pub fn start<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    app.manage(CoreSlot {
        core: OnceCell::new(),
        options: options(app)?,
    });
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // A failed start is logged; the next command tries again.
        let _ = app.state::<CoreSlot>().get(&app).await;
    });
    Ok(())
}

fn options<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<MessagingOptions> {
    Ok(MessagingOptions {
        data_dir: app.path().app_data_dir()?.join("core"),
        relays: relays(),
        keychain_service: KEYCHAIN_SERVICE.to_owned(),
        allow_loopback_relays: cfg!(debug_assertions),
    })
}

fn relays() -> Vec<String> {
    match RELAYS {
        Some(list) => list
            .split(',')
            .map(str::trim)
            .filter(|relay| !relay.is_empty())
            .map(str::to_owned)
            .collect(),
        None if cfg!(debug_assertions) => vec![DEV_RELAY.to_owned()],
        // Release builds get default relays with #17. Until then, creating an account says so.
        None => Vec::new(),
    }
}

fn start_failed(error: impl std::fmt::Display) -> CommandError {
    log::error!("the core could not start: {error}");
    CommandError::new("Could not start the core. Restart the app and try again.")
}

/// Passes each core event to the web view, whether or not one is listening.
fn forward_events<R: Runtime>(app: AppHandle<R>, mut events: broadcast::Receiver<CoreEvent>) {
    tauri::async_runtime::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => {
                    if let Err(error) = app.emit(EVENT, &event) {
                        log::warn!("could not pass a core event to the web view: {error}");
                    }
                }
                Err(RecvError::Lagged(missed)) => {
                    log::warn!("the web view missed {missed} core events");
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
}

#[tauri::command]
pub async fn core_snapshot<R: Runtime>(
    app: AppHandle<R>,
    slot: State<'_, CoreSlot>,
) -> Result<Snapshot, CommandError> {
    Ok(slot.get(&app).await?.snapshot())
}

#[tauri::command]
pub async fn create_account<R: Runtime>(
    app: AppHandle<R>,
    slot: State<'_, CoreSlot>,
    name: String,
) -> Result<Account, CommandError> {
    slot.get(&app).await?.create_account(&name).await
}

#[tauri::command]
pub async fn delete_account<R: Runtime>(
    app: AppHandle<R>,
    slot: State<'_, CoreSlot>,
) -> Result<(), CommandError> {
    slot.get(&app).await?.delete_account().await
}

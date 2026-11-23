//! The bridge between the web view and the core (#16). The shell starts the core when the app
//! launches, passes every core event to the web view as the Tauri event `core-event`, and turns
//! the web view's commands into calls on the core. The contract: `docs/core-interface.md`.

use peoples_chat_core::interface::{
    Account, Chat, CommandError, Contact, CoreEvent, Message, RadioStatus, SentInvite, Snapshot,
};
use peoples_chat_core::{Core, DEFAULT_RELAYS, MessagingOptions};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tauri_plugin_biometric::{AuthOptions, BiometricExt};
use tokio::sync::OnceCell;
use tokio::sync::broadcast::{self, error::RecvError};

/// The Tauri event that carries core events to the web view.
const EVENT: &str = "core-event";

/// Keychain service for account keys: the app's identifier.
const KEYCHAIN_SERVICE: &str = "chat.peoples.client";

/// What the device unlock says it is for.
const UNLOCK_REASON: &str = "Export your account key";

/// Relays for new accounts, comma-separated, taken from the environment when the app is built.
/// Without it the app uses the core's public [`DEFAULT_RELAYS`]. To use the relay that
/// `core/examples/local_relay.rs` runs on the Mac, build with
/// `TAURI_PEOPLES_CHAT_RELAYS=ws://127.0.0.1:7777`; only debug builds accept loopback relays.
///
/// The name starts with `TAURI` because the Tauri CLI passes only such variables (and `CARGO_`,
/// `RUST_` and `WRY` ones) on to the iOS and Android builds that Xcode and Gradle run.
const RELAYS: Option<&str> = option_env!("TAURI_PEOPLES_CHAT_RELAYS");

/// The media server for profile photos, taken from the environment when the app is built (#94).
/// Without it the app uses the core's `DEFAULT_MEDIA_SERVER`. To keep photos on the Mac, run
/// `core/examples/local_media.rs` and build with
/// `TAURI_PEOPLES_CHAT_MEDIA_SERVER=http://127.0.0.1:7778`; only debug builds accept it.
const MEDIA_SERVER: Option<&str> = option_env!("TAURI_PEOPLES_CHAT_MEDIA_SERVER");

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
                // Computers pair radios over USB, and open the paired one again (#24). Phones
                // will pair over Bluetooth (#23).
                #[cfg(desktop)]
                core.use_radio_ports(std::sync::Arc::new(peoples_chat_core::radio::SerialPorts));
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
    let relays = relays();
    log::info!("relays for new accounts: {}", relays.join(", "));
    if let Some(server) = MEDIA_SERVER {
        log::info!("media server for profile photos: {server}");
    }
    Ok(MessagingOptions {
        data_dir: app.path().app_data_dir()?.join("core"),
        relays,
        keychain_service: KEYCHAIN_SERVICE.to_owned(),
        allow_loopback: cfg!(debug_assertions),
        media_server: MEDIA_SERVER
            .map(str::trim)
            .filter(|server| !server.is_empty())
            .map(str::to_owned),
    })
}

/// `TAURI_PEOPLES_CHAT_RELAYS` when it names at least one relay, otherwise the core's defaults.
fn relays() -> Vec<String> {
    let chosen: Vec<String> = RELAYS
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|relay| !relay.is_empty())
        .map(str::to_owned)
        .collect();
    if chosen.is_empty() {
        DEFAULT_RELAYS
            .iter()
            .map(|relay| (*relay).to_owned())
            .collect()
    } else {
        chosen
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
pub async fn update_profile<R: Runtime>(
    app: AppHandle<R>,
    slot: State<'_, CoreSlot>,
    name: String,
    about: String,
) -> Result<Account, CommandError> {
    slot.get(&app).await?.update_profile(&name, &about).await
}

/// The photo comes as the request's raw body, not as JSON (#94).
#[tauri::command]
pub async fn set_profile_photo<R: Runtime>(
    app: AppHandle<R>,
    slot: State<'_, CoreSlot>,
    request: tauri::ipc::Request<'_>,
) -> Result<Account, CommandError> {
    let tauri::ipc::InvokeBody::Raw(photo) = request.body() else {
        return Err(CommandError::new(
            "This photo can't be used. Try another one.",
        ));
    };
    slot.get(&app).await?.set_profile_photo(photo).await
}

#[tauri::command]
pub async fn remove_profile_photo<R: Runtime>(
    app: AppHandle<R>,
    slot: State<'_, CoreSlot>,
) -> Result<Account, CommandError> {
    slot.get(&app).await?.remove_profile_photo().await
}

#[tauri::command]
pub async fn create_invite<R: Runtime>(
    app: AppHandle<R>,
    slot: State<'_, CoreSlot>,
    name: String,
) -> Result<SentInvite, CommandError> {
    slot.get(&app).await?.create_invite(&name).await
}

#[tauri::command]
pub async fn preview_invite<R: Runtime>(
    app: AppHandle<R>,
    slot: State<'_, CoreSlot>,
    invite_link: String,
) -> Result<Contact, CommandError> {
    slot.get(&app).await?.preview_invite(&invite_link).await
}

#[tauri::command]
pub async fn delete_account<R: Runtime>(
    app: AppHandle<R>,
    slot: State<'_, CoreSlot>,
) -> Result<(), CommandError> {
    slot.get(&app).await?.delete_account().await
}

#[tauri::command]
pub async fn add_contact<R: Runtime>(
    app: AppHandle<R>,
    slot: State<'_, CoreSlot>,
    invite_link: String,
) -> Result<Chat, CommandError> {
    slot.get(&app).await?.add_contact(&invite_link).await
}

#[tauri::command]
pub async fn accept_invitation<R: Runtime>(
    app: AppHandle<R>,
    slot: State<'_, CoreSlot>,
    invitation_id: String,
) -> Result<Chat, CommandError> {
    slot.get(&app)
        .await?
        .accept_invitation(&invitation_id)
        .await
}

/// Exports the account key (#86): a NIP-49 copy protected by `password`, or the plain key when
/// there is none. The device's owner unlocks first: the shell asks, so the web view can't skip
/// it.
#[tauri::command]
pub async fn export_account_key<R: Runtime>(
    app: AppHandle<R>,
    slot: State<'_, CoreSlot>,
    password: Option<String>,
) -> Result<String, CommandError> {
    let core = slot.get(&app).await?;
    let key = core
        .export_account_key(password.as_deref(), || confirm_owner(app.clone()))
        .await?;
    // The web view needs it as a plain string; the zeroizing copy is wiped here.
    Ok(key.as_str().to_owned())
}

/// The device unlock: Face ID, Touch ID or a fingerprint, or the device's passcode instead.
async fn confirm_owner<R: Runtime>(app: AppHandle<R>) -> Result<(), CommandError> {
    let options = AuthOptions {
        allow_device_credential: true,
        title: Some(UNLOCK_REASON.to_owned()),
        ..AuthOptions::default()
    };
    // The prompt blocks until the person answers it.
    let answer = tauri::async_runtime::spawn_blocking(move || {
        app.biometric()
            .authenticate(UNLOCK_REASON.to_owned(), options)
    })
    .await;
    match answer {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            log::info!("the device unlock did not pass: {error}");
            Err(CommandError::new("Unlock your device to export the key."))
        }
        Err(error) => {
            log::warn!("the device unlock failed: {error}");
            Err(CommandError::new("Unlock your device to export the key."))
        }
    }
}

#[tauri::command]
pub async fn send_message<R: Runtime>(
    app: AppHandle<R>,
    slot: State<'_, CoreSlot>,
    chat_id: String,
    text: String,
) -> Result<Message, CommandError> {
    slot.get(&app).await?.send_message(&chat_id, &text).await
}

#[tauri::command]
pub async fn cancel_message<R: Runtime>(
    app: AppHandle<R>,
    slot: State<'_, CoreSlot>,
    chat_id: String,
    message_id: String,
) -> Result<(), CommandError> {
    slot.get(&app)
        .await?
        .cancel_message(&chat_id, &message_id)
        .await
}

#[tauri::command]
pub async fn set_force_radio<R: Runtime>(
    app: AppHandle<R>,
    slot: State<'_, CoreSlot>,
    forced: bool,
) -> Result<RadioStatus, CommandError> {
    slot.get(&app).await?.set_force_radio(forced)
}

/// The USB ports a radio may be plugged into (#24). Computers only.
#[cfg(desktop)]
#[tauri::command]
pub async fn radio_ports<R: Runtime>(
    app: AppHandle<R>,
    slot: State<'_, CoreSlot>,
) -> Result<Vec<peoples_chat_core::interface::RadioPort>, CommandError> {
    slot.get(&app).await?.radio_ports().await
}

/// Pairs the radio on the USB port `port` (#24). Computers only.
#[cfg(desktop)]
#[tauri::command]
pub async fn pair_radio<R: Runtime>(
    app: AppHandle<R>,
    slot: State<'_, CoreSlot>,
    port: String,
) -> Result<RadioStatus, CommandError> {
    slot.get(&app).await?.pair_radio(&port).await
}

/// Forgets the paired radio (#24). Computers only.
#[cfg(desktop)]
#[tauri::command]
pub async fn forget_radio<R: Runtime>(
    app: AppHandle<R>,
    slot: State<'_, CoreSlot>,
) -> Result<RadioStatus, CommandError> {
    slot.get(&app).await?.forget_radio().await
}

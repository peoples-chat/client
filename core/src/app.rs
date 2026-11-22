//! The core as an interface sees it (#16): a snapshot, commands and events.
//!
//! The app shell starts the core when the app launches, whether or not a web view is open, and
//! forwards its events to the web view when there is one. Background work, such as radio
//! messages while the phone is locked, needs the core to run without a web view.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use marmot_app::AppError;
use tokio::sync::broadcast;

use crate::interface::{Account, CommandError, CoreEvent, Snapshot};
use crate::messaging::{Messaging, MessagingError, MessagingOptions};

/// How many events a listener may fall behind before it misses some.
const EVENT_BUFFER: usize = 64;

/// The protocol core. Clones share one core.
#[derive(Clone)]
pub struct Core {
    inner: Arc<Inner>,
}

struct Inner {
    messaging: Messaging,
    state: Mutex<Snapshot>,
    events: broadcast::Sender<CoreEvent>,
    /// Commands that create or delete the account run one at a time.
    account_commands: tokio::sync::Mutex<()>,
}

impl Core {
    /// Opens the core's stores and starts messaging. The snapshot then holds the account that is
    /// already on this device, if any. Must run inside a Tokio runtime.
    pub async fn start(options: MessagingOptions) -> Result<Self, MessagingError> {
        let messaging = Messaging::open(options).await?;
        // The proof of concept has one account per device.
        let account = match messaging.accounts()?.into_iter().next() {
            Some(existing) => Some(account_view(
                existing.public_key_hex,
                existing.name.unwrap_or_default(),
            )?),
            None => None,
        };
        log::info!(
            "core started {} an account",
            if account.is_some() { "with" } else { "without" }
        );
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        Ok(Self {
            inner: Arc::new(Inner {
                messaging,
                state: Mutex::new(Snapshot {
                    account,
                    ..Snapshot::default()
                }),
                events,
                account_commands: tokio::sync::Mutex::new(()),
            }),
        })
    }

    /// Everything an interface shows, as it is now.
    pub fn snapshot(&self) -> Snapshot {
        self.state().clone()
    }

    /// Follows the core's events from now on. A listener that falls more than 64 events behind
    /// misses some, and should load a new snapshot.
    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.inner.events.subscribe()
    }

    /// First launch: creates the account's key and `name` as its profile name. Emits
    /// [`CoreEvent::AccountCreated`].
    ///
    /// The key stays in the core: it goes to the platform's key store, and only the public key
    /// leaves.
    pub async fn create_account(&self, name: &str) -> Result<Account, CommandError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(CommandError::new("Enter a name."));
        }
        let _one_at_a_time = self.inner.account_commands.lock().await;
        if self.state().account.is_some() {
            return Err(CommandError::new("This device already has an account."));
        }

        // MDK needs relays to set an account up, though not reachable ones: publishing waits.
        let created =
            self.inner
                .messaging
                .create_account(name)
                .await
                .map_err(|error| match error {
                    MessagingError::Mdk(AppError::MissingDefaultRelays) => {
                        CommandError::new("This build has no relays set up.")
                    }
                    error => failed("create the account", error),
                })?;
        let account = account_view(created.public_key_hex, name.to_owned())
            .map_err(|error| failed("create the account", error))?;

        log::info!("account created");
        self.state().account = Some(account.clone());
        self.emit(CoreEvent::AccountCreated {
            account: account.clone(),
        });
        Ok(account)
    }

    /// Removes the account and its data from this device, including its key. Emits
    /// [`CoreEvent::AccountDeleted`].
    pub async fn delete_account(&self) -> Result<(), CommandError> {
        let _one_at_a_time = self.inner.account_commands.lock().await;
        let Some(account) = self.state().account.clone() else {
            return Err(CommandError::new("There is no account on this device."));
        };

        self.inner
            .messaging
            .delete_account(&account.public_key)
            .await
            .map_err(|error| failed("delete the account", error))?;

        log::info!("account deleted");
        *self.state() = Snapshot::default();
        self.emit(CoreEvent::AccountDeleted);
        Ok(())
    }

    /// Stops messaging and closes the stores. The core cannot be used afterwards.
    pub async fn close(&self) -> Result<(), MessagingError> {
        self.inner.messaging.close().await
    }

    fn state(&self) -> MutexGuard<'_, Snapshot> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn emit(&self, event: CoreEvent) {
        // Having no listener is fine: the core runs without a web view.
        let _ = self.inner.events.send(event);
    }
}

/// The account as an interface sees it. The invite link is the NIP-21 form of the public key
/// for now; #19 settles the link format.
fn account_view(public_key_hex: String, name: String) -> Result<Account, MessagingError> {
    let npub = marmot_app::npub_for_account_id(&public_key_hex)?;
    Ok(Account {
        name,
        invite_link: format!("nostr:{npub}"),
        public_key: public_key_hex,
    })
}

/// Logs what went wrong and returns an error an interface can show.
fn failed(action: &str, error: impl std::fmt::Display) -> CommandError {
    log::warn!("could not {action}: {error}");
    CommandError::new(format!("Could not {action}. Try again."))
}

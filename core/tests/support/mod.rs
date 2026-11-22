//! What the two-phone tests share: phones whose cores keep their folders and keys across
//! restarts, waiting for events, and two people who are already contacts.

// Each test file uses its own part of this.
#![allow(dead_code)]

mod relay;

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use peoples_chat_core::interface::{CoreEvent, Invitation};
use peoples_chat_core::{Core, MessagingOptions};
use tempfile::TempDir;
use tokio::sync::broadcast;

pub use relay::TestRelay;

/// How long a test waits for the relay and the other core. Each step takes well under a second.
pub const WAIT: Duration = Duration::from_secs(60);

/// Lets one test with cores run at a time. Several MDK runtimes in one process slow each other
/// down: three such tests took 31 s together and 11 s one after another. The app runs one.
pub async fn take_turn() -> tokio::sync::MutexGuard<'static, ()> {
    static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    TURN.lock().await
}

/// Account keys go to an in-memory keychain, never to the machine's own.
pub fn use_mock_keychain() {
    if keyring_core::get_default_store().is_none() {
        keyring_core::set_default_store(keyring_core::mock::Store::new().unwrap());
    }
}

pub fn options(data_dir: &Path, relay: &str, keychain_service: &str) -> MessagingOptions {
    MessagingOptions {
        data_dir: data_dir.to_path_buf(),
        relays: vec![relay.to_owned()],
        keychain_service: keychain_service.to_owned(),
        allow_loopback_relays: true,
    }
}

/// One phone: a data folder and a keychain service of its own, so its core can restart.
pub struct Phone {
    data: TempDir,
    keychain_service: String,
    relay: String,
}

impl Phone {
    /// `name` must be unique among the phones of a test file.
    pub fn new(name: &str, relay: &str) -> Self {
        use_mock_keychain();
        Self {
            data: tempfile::tempdir().unwrap(),
            keychain_service: format!("chat.peoples.client.test-{name}"),
            relay: relay.to_owned(),
        }
    }

    pub async fn start(&self) -> Core {
        Core::start(options(
            self.data.path(),
            &self.relay,
            &self.keychain_service,
        ))
        .await
        .unwrap()
    }
}

pub async fn next_event(events: &mut broadcast::Receiver<CoreEvent>) -> CoreEvent {
    tokio::time::timeout(WAIT, events.recv())
        .await
        .expect("no event in time")
        .expect("the core stopped sending events")
}

pub async fn next_invitation(events: &mut broadcast::Receiver<CoreEvent>) -> Invitation {
    match next_event(events).await {
        CoreEvent::InvitationReceived { invitation } => invitation,
        other => panic!("expected an invitation, got {other:?}"),
    }
}

/// Waits until `found` returns something.
pub async fn until<T>(what: &str, mut found: impl FnMut() -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if let Some(value) = found() {
            return value;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub fn now_ms() -> u64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    since.as_millis().try_into().unwrap()
}

/// Makes `ada` and `bea`, who have accounts, contacts: Ada adds Bea from her invite link, and
/// Bea accepts. Returns the id of their chat.
pub async fn make_contacts(ada: &Core, bea: &Core) -> String {
    ada.wait_until_published(WAIT).await.unwrap();
    bea.wait_until_published(WAIT).await.unwrap();
    let bea_link = bea.snapshot().account.unwrap().invite_link;
    let mut bea_events = bea.subscribe();
    let chat = ada.add_contact(&bea_link).await.unwrap();
    let invitation = next_invitation(&mut bea_events).await;
    bea.accept_invitation(&invitation.id).await.unwrap();
    chat.id
}

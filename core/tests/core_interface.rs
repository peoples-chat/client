//! The core as an interface sees it (#16): the JSON both sides agree on, and the account
//! commands with their events, through MDK. The account key stays in the key store (#17).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use nostr_relay_builder::MockRelay;
use nostr_sdk::prelude::{FromBech32, Keys, PublicKey, ToBech32};
use peoples_chat_core::interface::{CommandError, CoreEvent, Snapshot};
use peoples_chat_core::{Core, MessagingOptions};
use serde_json::Value;
use tokio::sync::broadcast;

/// One example of every event, a snapshot and a command error, as `src/core/types.ts` expects
/// them. The interface's tests read the same file.
const FIXTURE: &str = include_str!("fixtures/interface.json");

#[test]
fn the_shared_json_matches_the_core_types() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();

    let snapshot: Snapshot = serde_json::from_value(fixture["snapshot"].clone()).unwrap();
    assert_eq!(
        serde_json::to_value(&snapshot).unwrap(),
        fixture["snapshot"]
    );

    let mut kinds = BTreeSet::new();
    for json in fixture["events"].as_array().unwrap() {
        let event: CoreEvent = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(&serde_json::to_value(&event).unwrap(), json);
        kinds.insert(kind(&event));
    }
    assert_eq!(
        kinds.len(),
        16,
        "the fixture needs one example of every event"
    );

    let error: CommandError = serde_json::from_value(fixture["commandError"].clone()).unwrap();
    assert_eq!(
        serde_json::to_value(&error).unwrap(),
        fixture["commandError"]
    );
}

/// Stops compiling when an event is added: add it to the fixture and to `src/core/types.ts`.
fn kind(event: &CoreEvent) -> &'static str {
    match event {
        CoreEvent::AccountCreated { .. } => "accountCreated",
        CoreEvent::AccountDeleted => "accountDeleted",
        CoreEvent::ChatAdded { .. } => "chatAdded",
        CoreEvent::ChatChanged { .. } => "chatChanged",
        CoreEvent::InvitationReceived { .. } => "invitationReceived",
        CoreEvent::InvitationRemoved { .. } => "invitationRemoved",
        CoreEvent::MessageAdded { .. } => "messageAdded",
        CoreEvent::DeliveryChanged { .. } => "deliveryChanged",
        CoreEvent::MessageRemoved { .. } => "messageRemoved",
        CoreEvent::ReachChanged { .. } => "reachChanged",
        CoreEvent::DisappearingChanged { .. } => "disappearingChanged",
        CoreEvent::RelaysChanged { .. } => "relaysChanged",
        CoreEvent::AccountChanged { .. } => "accountChanged",
        CoreEvent::ContactChanged { .. } => "contactChanged",
        CoreEvent::InviteChanged { .. } => "inviteChanged",
        CoreEvent::RadioChanged { .. } => "radioChanged",
    }
}

/// Account keys go to an in-memory keychain, never to the machine's own.
fn use_mock_keychain() {
    if keyring_core::get_default_store().is_none() {
        keyring_core::set_default_store(keyring_core::mock::Store::new().unwrap());
    }
}

/// The next event, skipping relay updates: they come whenever a connection opens or closes.
async fn next_event(events: &mut broadcast::Receiver<CoreEvent>) -> CoreEvent {
    loop {
        let event = events.recv().await.unwrap();
        if !matches!(event, CoreEvent::RelaysChanged { .. }) {
            return event;
        }
    }
}

fn options(data_dir: &Path, relays: Vec<String>, keychain_service: &str) -> MessagingOptions {
    MessagingOptions {
        data_dir: data_dir.to_path_buf(),
        relays,
        keychain_service: keychain_service.to_owned(),
        keys_in_file: false,
        allow_loopback: true,
        media_server: None,
        profile_copies: Vec::new(),
        chat_media_servers: Vec::new(),
        lookup_relays: Vec::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_account_is_created_kept_across_a_restart_and_deleted() {
    use_mock_keychain();
    let relay = MockRelay::run().await.unwrap();
    let relays = vec![relay.url().await.to_string()];
    let data = tempfile::tempdir().unwrap();
    let service = "chat.peoples.client.test-account";
    let same_options = || options(data.path(), relays.clone(), service);

    let core = Core::start(same_options()).await.unwrap();
    assert_eq!(core.snapshot(), Snapshot::default());

    let mut events = core.subscribe();
    let account = core.create_account("  Emil Dahl ").await.unwrap();
    assert_eq!(account.name, "Emil Dahl");
    assert_eq!(account.public_key.len(), 64);
    // Issue #129: the npub White Noise takes, of the same key.
    assert!(account.npub.starts_with("npub1"), "{}", account.npub);
    assert_eq!(
        PublicKey::from_bech32(&account.npub).unwrap().to_hex(),
        account.public_key
    );
    assert!(
        account.invite_link.starts_with("nostr:nprofile1"),
        "{}",
        account.invite_link
    );
    assert_eq!(
        next_event(&mut events).await,
        CoreEvent::AccountCreated {
            account: account.clone()
        }
    );
    assert_eq!(core.snapshot().account.as_ref(), Some(&account));

    // One account per device.
    let error = core.create_account("Astrid Holm").await.unwrap_err();
    assert_eq!(error.message, "This device already has an account.");
    core.close().await.unwrap();

    // After a restart the core finds the account again.
    let core = Core::start(same_options()).await.unwrap();
    assert_eq!(core.snapshot().account, Some(account));

    let mut events = core.subscribe();
    core.delete_account().await.unwrap();
    assert_eq!(next_event(&mut events).await, CoreEvent::AccountDeleted);
    assert_eq!(core.snapshot(), Snapshot::default());
    core.close().await.unwrap();

    let core = Core::start(same_options()).await.unwrap();
    assert_eq!(core.snapshot().account, None);
    core.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_account_key_stays_in_the_key_store() {
    use_mock_keychain();
    let relay = MockRelay::run().await.unwrap();
    let relays = vec![relay.url().await.to_string()];
    let data = tempfile::tempdir().unwrap();
    let service = "chat.peoples.client.test-key";
    let same_options = || options(data.path(), relays.clone(), service);

    let core = Core::start(same_options()).await.unwrap();
    let mut events = core.subscribe();
    let account = core.create_account("Emil Dahl").await.unwrap();
    let created = next_event(&mut events).await;

    // MDK keeps the secret key in hex, under the account's public key.
    let entry = keyring_core::Entry::new(service, &account.public_key).unwrap();
    let secret_hex = entry.get_password().unwrap();
    let keys = Keys::parse(&secret_hex).unwrap();
    assert_eq!(keys.public_key().to_hex(), account.public_key);
    let nsec = keys.secret_key().to_bech32().unwrap();
    let holds_key = |text: &[u8]| {
        [secret_hex.as_bytes(), nsec.as_bytes()]
            .iter()
            .any(|key| text.windows(key.len()).any(|window| window == *key))
    };

    // Everything the web view receives, as JSON: command results, events and snapshots.
    let mut sent = vec![
        serde_json::to_string(&account).unwrap(),
        serde_json::to_string(&created).unwrap(),
        serde_json::to_string(&core.snapshot()).unwrap(),
    ];
    core.close().await.unwrap();

    // MDK's plain-text key file is not in use: no file in the data folder holds the key.
    let files = files_under(data.path());
    assert!(!files.is_empty());
    for file in &files {
        assert!(
            !holds_key(&std::fs::read(file).unwrap()),
            "{} holds the account key",
            file.display()
        );
    }

    let core = Core::start(same_options()).await.unwrap();
    sent.push(serde_json::to_string(&core.snapshot()).unwrap());
    for json in &sent {
        assert!(
            !holds_key(json.as_bytes()),
            "the account key reached the interface: {json}"
        );
    }

    // Deleting the account removes the key.
    core.delete_account().await.unwrap();
    assert!(matches!(
        entry.get_password(),
        Err(keyring_core::Error::NoEntry)
    ));
    core.close().await.unwrap();
}

/// Every file in `dir` and its subfolders.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut folders = vec![dir.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                folders.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commands_say_what_is_wrong() {
    use_mock_keychain();
    let data = tempfile::tempdir().unwrap();
    // No relays: MDK cannot set an account up without them.
    let core = Core::start(options(
        data.path(),
        Vec::new(),
        "chat.peoples.client.test-errors",
    ))
    .await
    .unwrap();

    let error = core.create_account("   ").await.unwrap_err();
    assert_eq!(error.message, "Enter a name.");
    let error = core.create_account("Emil Dahl").await.unwrap_err();
    assert_eq!(error.message, "This build has no relays set up.");
    let error = core.delete_account().await.unwrap_err();
    assert_eq!(error.message, "There is no account on this device.");

    core.close().await.unwrap();
}

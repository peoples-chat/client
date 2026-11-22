//! The core as an interface sees it (#16): the JSON both sides agree on, and the account
//! commands with their events, through MDK.

use std::collections::BTreeSet;
use std::path::Path;

use nostr_relay_builder::MockRelay;
use peoples_chat_core::interface::{CommandError, CoreEvent, Snapshot};
use peoples_chat_core::{Core, MessagingOptions};
use serde_json::Value;

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
        7,
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
        CoreEvent::InvitationReceived { .. } => "invitationReceived",
        CoreEvent::InvitationRemoved { .. } => "invitationRemoved",
        CoreEvent::MessageAdded { .. } => "messageAdded",
        CoreEvent::DeliveryChanged { .. } => "deliveryChanged",
    }
}

/// Account keys go to an in-memory keychain, never to the machine's own.
fn use_mock_keychain() {
    if keyring_core::get_default_store().is_none() {
        keyring_core::set_default_store(keyring_core::mock::Store::new().unwrap());
    }
}

fn options(data_dir: &Path, relays: Vec<String>, keychain_service: &str) -> MessagingOptions {
    MessagingOptions {
        data_dir: data_dir.to_path_buf(),
        relays,
        keychain_service: keychain_service.to_owned(),
        allow_loopback_relays: true,
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
    assert!(
        account.invite_link.starts_with("nostr:npub1"),
        "{}",
        account.invite_link
    );
    assert_eq!(
        events.recv().await.unwrap(),
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
    assert_eq!(events.recv().await.unwrap(), CoreEvent::AccountDeleted);
    assert_eq!(core.snapshot(), Snapshot::default());
    core.close().await.unwrap();

    let core = Core::start(same_options()).await.unwrap();
    assert_eq!(core.snapshot().account, None);
    core.close().await.unwrap();
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

//! The first link to MDK (#59): the core creates an account, MDK publishes the account's
//! bootstrap events to a relay, and an independent client reads them back from that relay.

use std::collections::BTreeSet;
use std::time::Duration;

use nostr_relay_builder::MockRelay;
use nostr_sdk::prelude::{Client, Filter, Kind, Metadata, PublicKey, ReqTarget};
use peoples_chat_core::{Messaging, MessagingOptions};

/// MDK's addressable KeyPackage kind. The pinned nostr-sdk has no name for it.
const KEY_PACKAGE_KIND: u16 = 30443;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_account_publishes_its_profile_relay_lists_and_key_package() {
    // Account keys go to an in-memory keychain, never to the machine's own.
    if keyring_core::get_default_store().is_none() {
        keyring_core::set_default_store(keyring_core::mock::Store::new().unwrap());
    }
    let relay = MockRelay::run().await.unwrap();
    let url = relay.url().await.to_string();
    let data = tempfile::tempdir().unwrap();

    let messaging = Messaging::open(MessagingOptions {
        data_dir: data.path().to_path_buf(),
        relays: vec![url.clone()],
        keychain_service: "chat.peoples.client.test".to_owned(),
        allow_loopback_relays: true,
    })
    .await
    .unwrap();

    let account = messaging.create_account("Emil Dahl").await.unwrap();
    messaging
        .wait_until_published(&account, Duration::from_secs(30))
        .await
        .unwrap();

    // Read the account's events back from the relay with a client that is not MDK.
    let author = PublicKey::from_hex(&account.public_key_hex).unwrap();
    let client = Client::builder().build();
    client.add_relay(&url).await.unwrap();
    client.connect().await;
    let events = client
        .fetch_events(ReqTarget::single(&url, [Filter::new().author(author)]))
        .timeout(Duration::from_secs(5))
        .await
        .unwrap();

    let kinds: BTreeSet<u16> = events.iter().map(|event| event.kind.as_u16()).collect();
    for kind in [
        Kind::Metadata.as_u16(),
        Kind::RelayList.as_u16(),
        Kind::InboxRelays.as_u16(),
        KEY_PACKAGE_KIND,
    ] {
        assert!(kinds.contains(&kind), "kind {kind} missing from {kinds:?}");
    }

    let profile = events
        .iter()
        .find(|event| event.kind == Kind::Metadata)
        .unwrap();
    let metadata = Metadata::try_from(profile).unwrap();
    assert_eq!(metadata.name.as_deref(), Some("Emil Dahl"));

    let relay_list = events
        .iter()
        .find(|event| event.kind == Kind::RelayList)
        .unwrap();
    let listed: Vec<&str> = relay_list
        .tags
        .iter()
        .filter_map(|tag| tag.as_slice().get(1).map(String::as_str))
        .collect();
    assert!(
        listed
            .iter()
            .any(|listed| listed.trim_end_matches('/') == url.trim_end_matches('/')),
        "{url} not in the relay list {listed:?}"
    );

    messaging.close().await.unwrap();
}

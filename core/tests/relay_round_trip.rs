//! The first link to MDK (#59): the core creates an account, MDK publishes the account's
//! bootstrap events to a relay, and an independent client reads them back from that relay.
//! Another Marmot client finds the account through the relay alone (#18).

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use cgka_traits::TransportEndpoint;
use marmot_account::AccountHome;
use marmot_app::{MarmotApp, MarmotAppConfig};
use nostr_relay_builder::MockRelay;
use nostr_sdk::prelude::{Client, Filter, Kind, Metadata, PublicKey, ReqTarget};
use peoples_chat_core::{Messaging, MessagingOptions};

/// MDK's addressable KeyPackage kind. The pinned nostr-sdk has no name for it.
const KEY_PACKAGE_KIND: u16 = 30443;

/// Account keys go to an in-memory keychain, never to the machine's own.
fn use_mock_keychain() {
    if keyring_core::get_default_store().is_none() {
        keyring_core::set_default_store(keyring_core::mock::Store::new().unwrap());
    }
}

fn options(data_dir: &Path, relay: &str, keychain_service: &str) -> MessagingOptions {
    MessagingOptions {
        data_dir: data_dir.to_path_buf(),
        relays: vec![relay.to_owned()],
        keychain_service: keychain_service.to_owned(),
        keys_in_file: false,
        allow_loopback: true,
        media_server: None,
        profile_copies: Vec::new(),
    }
}

fn same_relay(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_account_publishes_its_profile_relay_lists_and_key_package() {
    use_mock_keychain();
    let relay = MockRelay::run().await.unwrap();
    let url = relay.url().await.to_string();
    let data = tempfile::tempdir().unwrap();

    let messaging = Messaging::open(options(data.path(), &url, "chat.peoples.client.test"))
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
        listed.iter().any(|listed| same_relay(listed, &url)),
        "{url} not in the relay list {listed:?}"
    );

    messaging.close().await.unwrap();
}

/// What a client needs before it can invite the account: its relay lists and a KeyPackage.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_marmot_client_fetches_the_relay_lists_and_key_package() {
    use_mock_keychain();
    let relay = MockRelay::run().await.unwrap();
    let url = relay.url().await.to_string();
    let data = tempfile::tempdir().unwrap();

    let messaging = Messaging::open(options(
        data.path(),
        &url,
        "chat.peoples.client.test-fetched",
    ))
    .await
    .unwrap();
    let account = messaging.create_account("Astrid Holm").await.unwrap();
    messaging
        .wait_until_published(&account, Duration::from_secs(30))
        .await
        .unwrap();

    // Another client: MDK in its own folder, with no accounts, that knows only the relay.
    let other_data = tempfile::tempdir().unwrap();
    let home = AccountHome::open_with_keychain(other_data.path(), "chat.peoples.client.test-other")
        .unwrap();
    let other = MarmotApp::try_with_relays_and_account_home_and_config(
        other_data.path(),
        vec![url.clone()],
        home,
        MarmotAppConfig::default().with_allow_loopback_relay_endpoints(true),
    )
    .unwrap();
    let fetched = other
        .directory()
        .fetch_latest_key_package_for_account_id(
            &account.public_key_hex,
            vec![TransportEndpoint(url.clone())],
        )
        .await
        .unwrap();

    assert_eq!(fetched.account_id_hex, account.public_key_hex);
    assert!(
        fetched.relay_lists.complete,
        "relay lists missing: {:?}",
        fetched.relay_lists.missing
    );
    assert!(
        fetched
            .source_relays
            .iter()
            .any(|source| same_relay(source, &url)),
        "the KeyPackage came from {:?}, not {url}",
        fetched.source_relays
    );

    messaging.close().await.unwrap();
}

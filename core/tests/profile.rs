//! Editing the profile (#93): a new name and about are published and kept across a restart, and
//! a contact's core shows the new name once it fetches the profiles again. Changes are also
//! copied to White Noise's relays (#113), but never from a test relay.

mod support;

use std::time::Duration;

use nostr_sdk::prelude::{Client, Filter, Kind, Metadata, PublicKey, ReqTarget};
use peoples_chat_core::interface::{Contact, CoreEvent, RelayStatus};
use peoples_chat_core::{MAX_ABOUT_CHARS, MAX_NAME_CHARS};
use support::{Phone, TestRelay, WAIT, make_contacts, next_event, until};
use tokio::time::Instant;

/// The names in `author`'s profiles on the relay at `url`, read with a client that isn't MDK.
async fn profile_names(url: &str, author: PublicKey) -> Vec<String> {
    let client = Client::builder().build();
    client.add_relay(url).await.unwrap();
    client.connect().await;
    let events = client
        .fetch_events(ReqTarget::single(
            url,
            [Filter::new().author(author).kind(Kind::Metadata)],
        ))
        .timeout(Duration::from_secs(5))
        .await
        .unwrap();
    events
        .iter()
        .filter_map(|event| Metadata::try_from(event).ok()?.name)
        .collect()
}

/// Profile changes are copied to White Noise's relays (#113), but never from an account on
/// loopback relays. That keeps these tests, and builds on the Mac's test relay, off public relays.
/// MDK makes that call. A second relay stands in for White Noise's here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_account_on_a_test_relay_copies_its_profile_nowhere() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let copies = TestRelay::start().await;
    let phone = Phone::new("profile-copies", &relay.url()).with_profile_copies(&copies.url());
    let core = phone.start().await;
    core.create_account("Cleo Berg").await.unwrap();
    until("the relay", || {
        let relays = core.snapshot().relays;
        relays
            .iter()
            .any(|relay| relay.status == RelayStatus::Connected)
            .then_some(())
    })
    .await;
    let account = core.update_profile("Cleo B.", "").await.unwrap();
    let author = PublicKey::parse(&account.public_key).unwrap();

    // The change reached the account's relay.
    let deadline = Instant::now() + WAIT;
    while !profile_names(&relay.url(), author)
        .await
        .contains(&"Cleo B.".to_owned())
    {
        assert!(
            Instant::now() < deadline,
            "the profile didn't reach the relay"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // The stand-in for White Noise's relays got nothing, even after a while.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        profile_names(&copies.url(), author).await,
        Vec::<String>::new()
    );
    core.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_name_reaches_a_contact_and_the_profile_stays() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new("profile-ada", &url);
    let bea_phone = Phone::new("profile-bea", &url);
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    ada.create_account("Ada Lund").await.unwrap();
    bea.create_account("Bea Holm").await.unwrap();
    let chat_id = make_contacts(&ada, &bea).await;
    assert_eq!(ada.snapshot().account.unwrap().about, "");

    // Ada changes her name and adds a few words about herself.
    let mut ada_events = ada.subscribe();
    let account = ada
        .update_profile("  Ada L. ", " Aarhus. Usually reachable by radio. ")
        .await
        .unwrap();
    assert_eq!(account.name, "Ada L.");
    assert_eq!(account.about, "Aarhus. Usually reachable by radio.");
    assert_eq!(
        next_event(&mut ada_events).await,
        CoreEvent::AccountChanged {
            account: account.clone()
        }
    );
    assert_eq!(ada.snapshot().account.as_ref(), Some(&account));
    // The same profile again is no change.
    let same = ada
        .update_profile("Ada L.", "Aarhus. Usually reachable by radio.")
        .await
        .unwrap();
    assert_eq!(same, account);

    // Bea's core shows the new name once it fetches the profiles again.
    let mut bea_events = bea.subscribe();
    bea.refresh_contacts().await;
    assert_eq!(
        next_event(&mut bea_events).await,
        CoreEvent::ContactChanged {
            contact: Contact {
                public_key: account.public_key.clone(),
                name: "Ada L.".to_owned(),
                photo: None,
                meshes: Vec::new(),
            }
        }
    );
    let chat = bea.snapshot().chats.into_iter().find(|c| c.id == chat_id);
    assert_eq!(chat.unwrap().contact.name, "Ada L.");
    // Fetching again finds nothing new. Relay updates can come at any time, so only contact
    // changes count.
    bea.refresh_contacts().await;
    while let Ok(event) = bea_events.try_recv() {
        assert!(
            !matches!(event, CoreEvent::ContactChanged { .. }),
            "{event:?}"
        );
    }

    // Ada's profile stays across a restart.
    ada.close().await.unwrap();
    let ada = ada_phone.start().await;
    assert_eq!(ada.snapshot().account.as_ref(), Some(&account));

    // Without a relay, the profile can't change, and Ada is told why.
    relay.block();
    until("the relay to close", || {
        let relays = ada.snapshot().relays;
        relays
            .iter()
            .all(|relay| relay.status != RelayStatus::Connected)
            .then_some(())
    })
    .await;
    let error = ada.update_profile("Ada Offline", "").await.unwrap_err();
    assert_eq!(
        error.message,
        "Could not reach the relays. Try again when you are online."
    );
    assert_eq!(ada.snapshot().account.as_ref(), Some(&account));
    relay.unblock();
    until("the relay to come back", || {
        let relays = ada.snapshot().relays;
        relays
            .iter()
            .any(|relay| relay.status == RelayStatus::Connected)
            .then_some(())
    })
    .await;

    // About can be emptied again; a name can't.
    let cleared = ada.update_profile("Ada L.", "  ").await.unwrap();
    assert_eq!(cleared.about, "");
    let error = ada.update_profile("  ", "").await.unwrap_err();
    assert_eq!(error.message, "Enter a name.");
    let error = ada
        .update_profile(&"n".repeat(MAX_NAME_CHARS + 1), "")
        .await
        .unwrap_err();
    assert_eq!(error.message, "Your name can have up to 64 characters.");
    let error = ada
        .update_profile("Ada L.", &"æ".repeat(MAX_ABOUT_CHARS + 1))
        .await
        .unwrap_err();
    assert_eq!(error.message, "About can have up to 280 characters.");
    // Characters, not bytes: 280 of them fit.
    let full = ada
        .update_profile("Ada L.", &"æ".repeat(MAX_ABOUT_CHARS))
        .await
        .unwrap();
    assert_eq!(full.about.chars().count(), MAX_ABOUT_CHARS);
    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

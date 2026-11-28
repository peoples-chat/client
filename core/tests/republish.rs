//! Republishing radio messages and dropping duplicates (#47): during an outage Ada and Bea chat
//! by radio in both directions, and each has a message too long for radio waiting. When the relay
//! is back, MDK publishes the radio messages' relay copies, the waiting ones go, and both phones
//! end up with the same history: each message once, the radio ones still marked as radio. The
//! relay copies are really there: without its radio store, Bea's app still has every message.
//!
//! The relay goes on answering NIP-11 requests while it is blocked. Without that, MDK can hold
//! what it fetches after reconnecting for minutes, which is #78, not this.

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use peoples_chat_core::Core;
use peoples_chat_core::interface::{Mesh, Message, Reach, Transport};
use peoples_chat_core::radio::MockMesh;
use support::{Phone, TestRelay, WAIT, make_contacts, until};
use tokio::time::Instant;

fn messages(core: &Core, chat_id: &str) -> Vec<Message> {
    core.snapshot()
        .chats
        .into_iter()
        .find(|chat| chat.id == chat_id)
        .map(|chat| chat.messages)
        .unwrap_or_default()
}

/// The chat's messages by id: their text, when they were sent and how they came. Panics on a
/// duplicate.
fn history(core: &Core, chat_id: &str) -> BTreeMap<String, (String, u64, Transport)> {
    let mut history = BTreeMap::new();
    for message in messages(core, chat_id) {
        let known = history.insert(
            message.id.clone(),
            (message.text.clone(), message.sent_at, message.transport),
        );
        assert!(known.is_none(), "{} twice", message.text);
    }
    history
}

async fn reach_becomes(core: &Core, chat_id: &str, wanted: Reach) {
    until(&format!("the chat to be {wanted:?}"), || {
        core.snapshot()
            .chats
            .iter()
            .any(|chat| chat.id == chat_id && chat.reach == wanted)
            .then_some(())
    })
    .await;
}

async fn knows_radio(core: &Core, chat_id: &str) {
    until("the contact's radio", || {
        core.snapshot()
            .chats
            .iter()
            .any(|chat| chat.id == chat_id && chat.contact.meshes == vec![Mesh::MeshCore])
            .then_some(())
    })
    .await;
}

async fn settled(one: &Core, other: &Core, chat_id: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        if let (Ok(a), Ok(b)) = (
            one.chat_membership(chat_id).await,
            other.chat_membership(chat_id).await,
        ) && a.epoch == b.epoch
        {
            return;
        }
        assert!(Instant::now() < deadline, "the chat didn't settle");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn after_an_outage_both_phones_have_the_same_history() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new("republish-ada", &url);
    let bea_phone = Phone::new("republish-bea", &url);
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    ada.create_account("Ada Lund").await.unwrap();
    bea.create_account("Bea Holm").await.unwrap();
    let chat_id = make_contacts(&ada, &bea).await;
    until("Bea's chat", || {
        bea.snapshot()
            .chats
            .iter()
            .any(|chat| chat.id == chat_id)
            .then_some(())
    })
    .await;
    settled(&ada, &bea, &chat_id).await;
    let mesh = MockMesh::new();
    let ada_radio = Arc::new(mesh.radio("Ada's radio"));
    let bea_radio = Arc::new(mesh.radio("Bea's radio"));
    ada.attach_radio(ada_radio.clone()).await.unwrap();
    bea.attach_radio(bea_radio.clone()).await.unwrap();
    knows_radio(&ada, &chat_id).await;
    knows_radio(&bea, &chat_id).await;
    ada.send_message(&chat_id, "Before the outage")
        .await
        .unwrap();

    // The outage. Both radios announce themselves, so each chat goes by radio.
    relay.block_messages();
    reach_becomes(&ada, &chat_id, Reach::Unreachable).await;
    reach_becomes(&bea, &chat_id, Reach::Unreachable).await;
    ada_radio.advertise();
    bea_radio.advertise();
    reach_becomes(&ada, &chat_id, Reach::Radio).await;
    reach_becomes(&bea, &chat_id, Reach::Radio).await;

    // Both ways by radio, and one message each too long for radio.
    let too_long = |who: &str| {
        format!(
            "{who} here. This one is too long for a single radio packet, so it waits for the internet to come back."
        )
    };
    let mut by_radio = Vec::new();
    for (from, text) in [
        (&ada, "Ada by radio, one"),
        (&bea, "Bea by radio, one"),
        (&ada, "Ada by radio, two"),
        (&bea, "Bea by radio, two"),
    ] {
        let sent = from.send_message(&chat_id, text).await.unwrap();
        assert_eq!(sent.transport, Transport::Radio, "{text}");
        by_radio.push(sent.id);
    }
    assert!(too_long("Ada").len() > 96);
    let ada_long = ada.send_message(&chat_id, &too_long("Ada")).await.unwrap();
    let bea_long = bea.send_message(&chat_id, &too_long("Bea")).await.unwrap();
    assert!(ada_long.cancellable && bea_long.cancellable);
    for core in [&ada, &bea] {
        until("the radio messages on both phones", || {
            let history = history(core, &chat_id);
            by_radio
                .iter()
                .all(|id| history.contains_key(id))
                .then_some(())
        })
        .await;
    }

    // The relay is back. The waiting messages go, and the radio messages' relay copies arrive.
    relay.unblock();
    let both = until("the same history on both phones", || {
        let ada_history = history(&ada, &chat_id);
        let bea_history = history(&bea, &chat_id);
        let complete = |history: &BTreeMap<String, (String, u64, Transport)>| {
            history.len() == 7
                && history.values().all(|(text, ..)| {
                    // No waiting message is left.
                    !messages(&ada, &chat_id)
                        .iter()
                        .chain(messages(&bea, &chat_id).iter())
                        .any(|m| m.cancellable && &m.text == text)
                })
        };
        (ada_history == bea_history && complete(&ada_history)).then_some(ada_history)
    })
    .await;
    for id in &by_radio {
        assert_eq!(both[id].2, Transport::Radio, "{}", both[id].0);
    }
    for who in ["Ada", "Bea"] {
        let long = both
            .values()
            .find(|(text, ..)| *text == too_long(who))
            .unwrap();
        assert_eq!(long.2, Transport::Internet);
    }

    // The relay copies are in MDK: without its radio store, Bea's app still has every message.
    bea.close().await.unwrap();
    let store = bea_phone.data_dir().join("radio.json");
    std::fs::rename(&store, store.with_extension("json.aside")).unwrap();
    let bea = bea_phone.start().await;
    let without_store = until("Bea's history from MDK", || {
        let history = history(&bea, &chat_id);
        (history.len() == 7).then_some(history)
    })
    .await;
    let ids = |history: &BTreeMap<String, (String, u64, Transport)>| {
        history
            .iter()
            .map(|(id, (text, sent_at, _))| (id.clone(), text.clone(), *sent_at))
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&without_store), ids(&both));

    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

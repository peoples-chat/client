//! The bench test (#49), with mock radios: the proof of concept's headline scenario, one chat
//! that survives an outage. On hardware day the same steps run with two phones and two radios on
//! a desk; this is the regression test for them. `docs/tests/49-bench.md` describes both.
//!
//! 1. Both online: a message each way over the relay.
//! 2. The internet is cut on both. The chat goes to radio by itself once each hears the other's
//!    radio.
//! 3. Ten messages each way by radio, each confirmed by the other radio.
//! 4. Ada switches her radio off and writes a message: it waits.
//! 5. The internet is back: the waiting message goes, and the radio messages are republished.
//! 6. Both histories are identical, with no duplicates, each message marked with how it
//!    travelled, also after both restart. Each radio sent only its user's ten messages (#52).
//!
//! The relay keeps answering NIP-11 requests while it is cut, which keeps #78's delay out.

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use peoples_chat_core::Core;
use peoples_chat_core::interface::{DeliveryState, Mesh, Message, RadioIssue, Reach, Transport};
use peoples_chat_core::radio::{MockMesh, Radio};
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

/// The chat's messages by id, with their text, time and transport. Panics on a duplicate.
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

async fn message(core: &Core, chat_id: &str, id: &str) -> Message {
    until("the message", || {
        messages(core, chat_id).into_iter().find(|m| m.id == id)
    })
    .await
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
            one.radio_keys(chat_id).await,
            other.radio_keys(chat_id).await,
        ) && a.epoch == b.epoch
        {
            return;
        }
        assert!(Instant::now() < deadline, "the chat didn't settle");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_chat_survives_an_outage() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new("bench-ada", &url);
    let bea_phone = Phone::new("bench-bea", &url);
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

    // 1. Both online: a message each way over the relay.
    for (from, to, text) in [
        (&ada, &bea, "Online from Ada"),
        (&bea, &ada, "Online from Bea"),
    ] {
        let sent = from.send_message(&chat_id, text).await.unwrap();
        assert_eq!(sent.transport, Transport::Internet);
        let received = message(to, &chat_id, &sent.id).await;
        assert_eq!(received.transport, Transport::Internet);
    }

    // 2. The internet is cut. Once each hears the other's radio, the chat goes to radio.
    relay.block_messages();
    reach_becomes(&ada, &chat_id, Reach::Unreachable).await;
    reach_becomes(&bea, &chat_id, Reach::Unreachable).await;
    ada_radio.advertise();
    bea_radio.advertise();
    reach_becomes(&ada, &chat_id, Reach::Radio).await;
    reach_becomes(&bea, &chat_id, Reach::Radio).await;

    // 3. Ten messages each way by radio, each confirmed by the other radio.
    for round in 1..=10 {
        for (from, to, who) in [(&ada, &bea, "Ada"), (&bea, &ada, "Bea")] {
            let text = format!("{who} by radio, {round} of 10");
            let sent = from.send_message(&chat_id, &text).await.unwrap();
            assert_eq!(sent.transport, Transport::Radio, "{text}");
            let received = message(to, &chat_id, &sent.id).await;
            assert_eq!(received.transport, Transport::Radio, "{text}");
            until(&format!("{text:?} confirmed"), || {
                messages(from, &chat_id)
                    .iter()
                    .any(|m| m.id == sent.id && m.delivery == Some(DeliveryState::Confirmed))
                    .then_some(())
            })
            .await;
        }
    }

    // 4. Ada switches her radio off and writes a message. It waits.
    ada_radio.disconnect().await.unwrap();
    until("Ada's chat without her radio", || {
        ada.snapshot()
            .chats
            .iter()
            .any(|chat| chat.id == chat_id && chat.radio_issue == Some(RadioIssue::NoRadio))
            .then_some(())
    })
    .await;
    let waiting = ada
        .send_message(&chat_id, "Written with the radio off")
        .await
        .unwrap();
    assert_eq!(waiting.delivery, Some(DeliveryState::Waiting));
    assert!(waiting.cancellable);

    // 5. The internet is back: the waiting message goes, and the radio messages are republished.
    relay.unblock();
    let both = until("identical histories", || {
        let ada_history = history(&ada, &chat_id);
        let bea_history = history(&bea, &chat_id);
        let gone = ada_history
            .values()
            .any(|(text, ..)| text == "Written with the radio off")
            && messages(&ada, &chat_id).iter().all(|m| !m.cancellable);
        (gone && ada_history == bea_history).then_some(ada_history)
    })
    .await;

    // 6. Each message once, marked with how it travelled.
    assert_eq!(both.len(), 2 + 20 + 1);
    for (text, _, transport) in both.values() {
        let expected = if text.contains("by radio") {
            Transport::Radio
        } else {
            Transport::Internet
        };
        assert_eq!(*transport, expected, "{text}");
    }

    // Only what the users sent went by radio (#52): ten messages each, and nothing else, such as
    // the radio address records, which travel in the chat.
    for core in [&ada, &bea] {
        let log = core.radio_transmissions();
        assert_eq!(log.len(), 10);
        assert!(log.iter().all(|sent| sent.reason == "message"));
    }

    // Also after both restart.
    ada.close().await.unwrap();
    bea.close().await.unwrap();
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    assert_eq!(history(&ada, &chat_id), both);
    assert_eq!(history(&bea, &chat_id), both);

    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

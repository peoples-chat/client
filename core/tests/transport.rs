//! Choosing the transport (#45): with no one touching a switch, Ada's messages go over the relay
//! while it is reachable, by radio while it isn't and Bea's radio has been heard, and wait when
//! neither is the case. Bea's app keeps one copy of each, marked with how it came.

mod support;

use std::sync::Arc;
use std::time::Duration;

use peoples_chat_core::Core;
use peoples_chat_core::interface::{DeliveryState, Mesh, Message, RadioIssue, Reach, Transport};
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
async fn each_message_takes_the_path_there_is() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new("transport-ada", &url);
    let bea_phone = Phone::new("transport-bea", &url);
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
    // Bea's radio announces itself, as radios do now and then.
    bea_radio.advertise();

    // The relay is there: the internet, although Bea's radio has been heard.
    let online = ada.send_message(&chat_id, "Over the relay").await.unwrap();
    assert_eq!(online.transport, Transport::Internet);
    let received = message(&bea, &chat_id, &online.id).await;
    assert_eq!(received.transport, Transport::Internet);

    // The relay goes away: radio, with no switch touched.
    relay.block();
    reach_becomes(&ada, &chat_id, Reach::Radio).await;
    let by_radio = ada.send_message(&chat_id, "By radio").await.unwrap();
    assert_eq!(by_radio.transport, Transport::Radio);
    let received = message(&bea, &chat_id, &by_radio.id).await;
    assert_eq!(received.transport, Transport::Radio);
    until("Bea's radio confirms", || {
        messages(&ada, &chat_id)
            .iter()
            .any(|m| m.id == by_radio.id && m.delivery == Some(DeliveryState::Confirmed))
            .then_some(())
    })
    .await;

    // Bea's app heard Ada's radio in that message, so Bea's reply goes by radio too.
    reach_becomes(&bea, &chat_id, Reach::Radio).await;
    let reply = bea.send_message(&chat_id, "Got it by radio").await.unwrap();
    assert_eq!(reply.transport, Transport::Radio);
    message(&ada, &chat_id, &reply.id).await;

    // Once Bea's radio hasn't been heard for the window, Ada's message waits.
    ada.set_radio_heard_window(Duration::from_millis(500));
    until("Ada's chat to be unreachable", || {
        ada.snapshot()
            .chats
            .iter()
            .any(|chat| {
                chat.id == chat_id
                    && chat.reach == Reach::Unreachable
                    && chat.radio_issue == Some(RadioIssue::NotHeard)
            })
            .then_some(())
    })
    .await;
    let waiting = ada.send_message(&chat_id, "Waiting").await.unwrap();
    assert_eq!(waiting.transport, Transport::Internet);
    assert_eq!(waiting.delivery, Some(DeliveryState::Waiting));
    assert!(waiting.cancellable);

    // The relay is back: the waiting message goes, and so does the next, over the relay.
    relay.unblock();
    reach_becomes(&ada, &chat_id, Reach::Internet).await;
    // It went under MDK's id, in place of the waiting one (#46).
    until("the waiting message to be sent", || {
        messages(&ada, &chat_id)
            .iter()
            .any(|m| {
                m.text == "Waiting" && m.id != waiting.id && m.delivery == Some(DeliveryState::Sent)
            })
            .then_some(())
    })
    .await;
    let back = ada.send_message(&chat_id, "Back online").await.unwrap();
    assert_eq!(back.transport, Transport::Internet);
    let received = message(&bea, &chat_id, &back.id).await;
    assert_eq!(received.transport, Transport::Internet);

    // Each message once on Bea's side, the radio ones still marked as radio.
    let kept = messages(&bea, &chat_id);
    let mut ids: Vec<&str> = kept.iter().map(|m| m.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), kept.len(), "{kept:?}");
    for id in [&by_radio.id, &reply.id] {
        let radio = kept.iter().find(|m| &m.id == id).unwrap();
        assert_eq!(radio.transport, Transport::Radio);
    }

    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

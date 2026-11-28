//! Messages with no path (#46): with the relay blocked and Bea's radio not heard, what Ada writes
//! waits, in order and across a restart, and can be cancelled. When Bea's radio is heard, the
//! waiting messages go by radio, except one too long for radio; that one goes when the relay is
//! back.

mod support;

use std::sync::Arc;
use std::time::Duration;

use peoples_chat_core::Core;
use peoples_chat_core::interface::{CoreEvent, DeliveryState, Mesh, Message, Reach, Transport};
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

/// The chat's messages with `text`.
fn with_text(core: &Core, chat_id: &str, text: &str) -> Vec<Message> {
    messages(core, chat_id)
        .into_iter()
        .filter(|m| m.text == text)
        .collect()
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
async fn messages_wait_for_a_path_and_go_when_one_returns() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new("waiting-ada", &url);
    let bea_phone = Phone::new("waiting-bea", &url);
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
    let online = ada.send_message(&chat_id, "Before").await.unwrap();
    assert!(!online.cancellable);

    // No relay, and Bea's radio hasn't been heard: what Ada writes waits.
    relay.block();
    reach_becomes(&ada, &chat_id, Reach::Unreachable).await;
    let long = "This is longer than a radio packet carries, so it waits for the internet whatever the radio does.";
    assert!(long.len() > 96);
    let mut written = Vec::new();
    for text in ["First", "Second", long, "Third"] {
        let message = ada.send_message(&chat_id, text).await.unwrap();
        assert_eq!(message.delivery, Some(DeliveryState::Waiting));
        assert!(message.cancellable, "{text}");
        written.push(message);
    }

    // Ada cancels the second.
    let mut ada_events = ada.subscribe();
    ada.cancel_message(&chat_id, &written[1].id).await.unwrap();
    let removed = loop {
        match tokio::time::timeout(WAIT, ada_events.recv())
            .await
            .unwrap()
            .unwrap()
        {
            CoreEvent::MessageRemoved { message_id, .. } => break message_id,
            _ => continue,
        }
    };
    assert_eq!(removed, written[1].id);
    assert!(with_text(&ada, &chat_id, "Second").is_empty());
    let error = ada
        .cancel_message(&chat_id, &written[1].id)
        .await
        .unwrap_err();
    assert_eq!(error.message, "This message no longer exists.");
    let error = ada.cancel_message(&chat_id, &online.id).await.unwrap_err();
    assert_eq!(error.message, "This message is on its way already.");

    // They wait across a restart, in order.
    ada.close().await.unwrap();
    let ada = ada_phone.start().await;
    let waiting: Vec<String> = messages(&ada, &chat_id)
        .into_iter()
        .filter(|m| m.cancellable)
        .map(|m| m.text)
        .collect();
    assert_eq!(waiting, vec!["First", long, "Third"]);
    ada.attach_radio(ada_radio.clone()).await.unwrap();

    // Bea's radio is heard: the short ones go by radio, in order; the long one waits.
    bea_radio.advertise();
    reach_becomes(&ada, &chat_id, Reach::Radio).await;
    for text in ["First", "Third"] {
        let sent = until(&format!("{text:?} to go"), || {
            let sent = with_text(&ada, &chat_id, text);
            (sent.len() == 1 && !sent[0].cancellable).then(|| sent[0].clone())
        })
        .await;
        assert_eq!(sent.transport, Transport::Radio);
        let received = until(&format!("{text:?} at Bea"), || {
            with_text(&bea, &chat_id, text).into_iter().next()
        })
        .await;
        assert_eq!(received.transport, Transport::Radio);
        assert_eq!(received.id, sent.id);
    }
    let still = with_text(&ada, &chat_id, long);
    assert!(still.len() == 1 && still[0].cancellable, "{still:?}");
    let first = &with_text(&ada, &chat_id, "First")[0];
    let third = &with_text(&ada, &chat_id, "Third")[0];
    assert!(first.sent_at <= third.sent_at);

    // The relay is back: the long one goes over it.
    relay.unblock();
    reach_becomes(&ada, &chat_id, Reach::Internet).await;
    let sent = until("the long one to be acknowledged", || {
        let sent = with_text(&ada, &chat_id, long);
        (sent.len() == 1 && sent[0].delivery == Some(DeliveryState::Sent)).then(|| sent[0].clone())
    })
    .await;
    assert_eq!(sent.transport, Transport::Internet);
    assert!(!sent.cancellable);

    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

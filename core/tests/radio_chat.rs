//! Chat text over radio (#40): Ada and Bea each have a radio on one mock mesh, and their apps
//! exchange the radios' address records (#34). With the relay blocked and radio forced, they
//! exchange text both ways. Each message is marked as radio and
//! confirmed by the other radio, and stays across a restart; a captured packet sent again shows
//! nothing, before and after the restart.

mod support;

use std::sync::Arc;
use std::time::Duration;

use peoples_chat_core::Core;
use peoples_chat_core::interface::{
    CoreEvent, DeliveryState, Direction, Mesh, Message, RadioConnection, Transport,
};
use peoples_chat_core::radio::{MockMesh, Radio, RadioEvent};
use support::{Phone, TestRelay, WAIT, make_contacts, next_event, until};
use tokio::time::Instant;

fn messages(core: &Core, chat_id: &str) -> Vec<Message> {
    core.snapshot()
        .chats
        .into_iter()
        .find(|chat| chat.id == chat_id)
        .map(|chat| chat.messages)
        .unwrap_or_default()
}

/// The message `id` in the chat, once it is there.
async fn message(core: &Core, chat_id: &str, id: &str) -> Message {
    until("the message", || {
        messages(core, chat_id).into_iter().find(|m| m.id == id)
    })
    .await
}

/// Waits until the chat's contact has a radio, from their radio address record (#34).
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

/// Waits until both phones have the chat's radio keys in the same epoch: the chat has settled
/// on both after they became contacts.
async fn settled(ada: &Core, bea: &Core, chat_id: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        if let (Ok(ada_keys), Ok(bea_keys)) =
            (ada.radio_keys(chat_id).await, bea.radio_keys(chat_id).await)
            && ada_keys.epoch == bea_keys.epoch
        {
            return;
        }
        assert!(Instant::now() < deadline, "the chat didn't settle");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_phones_chat_by_radio_while_the_relay_is_blocked() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new("radio-chat-ada", &url);
    let bea_phone = Phone::new("radio-chat-bea", &url);
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

    // Ada has a radio. Without the switch on, messages go by the internet.
    let mesh = MockMesh::new();
    let ada_radio = Arc::new(mesh.radio("Ada's radio"));
    let bea_radio = Arc::new(mesh.radio("Bea's radio"));
    let status = ada.attach_radio(ada_radio.clone()).await.unwrap();
    let paired = status.paired.unwrap();
    assert_eq!(paired.name, "Ada's radio");
    assert_eq!(paired.public_key, ada_radio.public_key());
    assert_eq!(paired.connection, RadioConnection::Connected);
    assert!(!status.forced);
    // A radio handed over, not paired: no port.
    assert_eq!(paired.port, None);

    // The switch: the radio carries everything from now on, and says so.
    let mut ada_events = ada.subscribe();
    assert!(ada.set_force_radio(true).unwrap().forced);
    match next_event(&mut ada_events).await {
        CoreEvent::RadioChanged { radio } => assert!(radio.forced),
        other => panic!("expected the radio's change, got {other:?}"),
    }
    assert!(ada.snapshot().radio.forced);

    // The radio needs Bea's radio, and she has none yet.
    let error = ada.send_message(&chat_id, "Hello?").await.unwrap_err();
    assert_eq!(error.message, "Bea Holm's radio isn't known yet.");

    // Bea gets one. Their apps exchange the radios' address records in the chat (#34).
    bea.attach_radio(bea_radio.clone()).await.unwrap();
    bea.set_force_radio(true).unwrap();
    knows_radio(&ada, &chat_id).await;
    knows_radio(&bea, &chat_id).await;

    // The relay goes away.
    relay.block();

    // Ada to Bea. Bea's radio confirms it.
    let hello = ada
        .send_message(&chat_id, "Hello Bea, by radio")
        .await
        .unwrap();
    assert_eq!(hello.transport, Transport::Radio);
    assert_eq!(hello.direction, Direction::Out);
    until("Bea's radio confirms", || {
        messages(&ada, &chat_id)
            .into_iter()
            .any(|m| m.id == hello.id && m.delivery == Some(DeliveryState::Confirmed))
            .then_some(())
    })
    .await;
    let received = message(&bea, &chat_id, &hello.id).await;
    assert_eq!(received.direction, Direction::In);
    assert_eq!(received.transport, Transport::Radio);
    assert_eq!(received.text, "Hello Bea, by radio");
    assert_eq!(received.sent_at, hello.sent_at);

    // Bea to Ada, with a character of three bytes.
    let reply = bea.send_message(&chat_id, "Hej Ada! ☔").await.unwrap();
    assert_eq!(reply.transport, Transport::Radio);
    let received = message(&ada, &chat_id, &reply.id).await;
    assert_eq!(received.direction, Direction::In);
    assert_eq!(received.transport, Transport::Radio);
    assert_eq!(received.text, "Hej Ada! ☔");

    // Radio carries 96 bytes at most.
    let error = ada
        .send_message(&chat_id, &"x".repeat(97))
        .await
        .unwrap_err();
    assert_eq!(
        error.message,
        "That is too long for radio. Keep it to 96 bytes."
    );

    // Eve captures Ada's next packet in the air and sends it to Bea again. It shows nothing.
    let mut air = bea_radio.events();
    let third = ada.send_message(&chat_id, "Third").await.unwrap();
    let captured = loop {
        match tokio::time::timeout(WAIT, air.recv())
            .await
            .unwrap()
            .unwrap()
        {
            RadioEvent::TextReceived { text, .. } => break text,
            _ => continue,
        }
    };
    message(&bea, &chat_id, &third.id).await;
    let eve_radio = mesh.radio("Eve's radio");
    eve_radio
        .send_text(bea_radio.public_key(), &captured)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(messages(&bea, &chat_id).len(), 3);

    // Bea restarts, with the relay still blocked. The messages are as they were, and the switch
    // is still on; without a radio, a message can't go.
    bea.close().await.unwrap();
    let bea = bea_phone.start().await;
    let kept = messages(&bea, &chat_id);
    let summary: Vec<(&str, Direction, Transport)> = kept
        .iter()
        .map(|m| (m.text.as_str(), m.direction, m.transport))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("Hello Bea, by radio", Direction::In, Transport::Radio),
            ("Hej Ada! ☔", Direction::Out, Transport::Radio),
            ("Third", Direction::In, Transport::Radio),
        ]
    );
    assert_eq!(kept[1].delivery, Some(DeliveryState::Confirmed));
    let status = bea.snapshot().radio;
    assert!(status.forced && status.paired.is_none());
    let error = bea
        .send_message(&chat_id, "Still there?")
        .await
        .unwrap_err();
    assert_eq!(error.message, "Pair a radio first.");

    // With the radio back, the captured packet is caught by its message id.
    bea.attach_radio(bea_radio.clone()).await.unwrap();
    eve_radio
        .send_text(bea_radio.public_key(), &captured)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(messages(&bea, &chat_id).len(), 3);

    // And the chat goes on by radio.
    let after = bea.send_message(&chat_id, "Still there?").await.unwrap();
    let received = message(&ada, &chat_id, &after.id).await;
    assert_eq!(received.transport, Transport::Radio);

    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

/// With the relay up and radio forced, a message goes both ways: the radio carries it, and
/// MDK publishes it. The other phone keeps one copy.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_that_comes_both_ways_shows_once() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new("radio-both-ada", &url);
    let bea_phone = Phone::new("radio-both-bea", &url);
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
    ada.set_force_radio(true).unwrap();

    let sent = ada.send_message(&chat_id, "Both ways").await.unwrap();
    message(&bea, &chat_id, &sent.id).await;
    // A relay acknowledges it too, which changes nothing once the radio has confirmed it.
    until("Ada's message confirmed", || {
        messages(&ada, &chat_id)
            .into_iter()
            .any(|m| m.id == sent.id && m.delivery == Some(DeliveryState::Confirmed))
            .then_some(())
    })
    .await;

    // Bea's reply goes by the internet only, and comes after the relay copy of Ada's message.
    let reply = bea.send_message(&chat_id, "Got it").await.unwrap();
    assert_eq!(reply.transport, Transport::Internet);
    message(&ada, &chat_id, &reply.id).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let ids: Vec<String> = messages(&bea, &chat_id).into_iter().map(|m| m.id).collect();
    assert_eq!(ids, vec![sent.id.clone(), reply.id.clone()]);

    // Also after a restart, when the history comes from MDK and the radio store together. (MDK
    // orders messages of the same second by id, so the order may differ.)
    bea.close().await.unwrap();
    let bea = bea_phone.start().await;
    let kept = messages(&bea, &chat_id);
    let mut ids: Vec<&str> = kept.iter().map(|m| m.id.as_str()).collect();
    ids.sort_unstable();
    let mut expected = vec![sent.id.as_str(), reply.id.as_str()];
    expected.sort_unstable();
    assert_eq!(ids, expected);
    let radio = kept.iter().find(|m| m.id == sent.id).unwrap();
    assert_eq!(radio.transport, Transport::Radio);

    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

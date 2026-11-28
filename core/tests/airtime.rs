//! The radio airtime cap (#51): what the user sends by radio stays within an hourly airtime
//! budget. Past it, a message waits, nothing is transmitted, and the chat says why. It goes once
//! airtime comes free. Every transmission is logged with its reason (#52).

mod support;

use std::sync::Arc;
use std::time::Duration;

use peoples_chat_core::Core;
use peoples_chat_core::interface::{Mesh, Message, RadioIssue, Reach, Transport};
use peoples_chat_core::radio::airtime::{meshcore_text_bytes, time_on_air};
use peoples_chat_core::radio::format::OVERHEAD_BYTES;
use peoples_chat_core::radio::{MOCK_SETTINGS, MockMesh, Radio, RadioEvent};
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

fn chat_state(core: &Core, chat_id: &str) -> Option<(Reach, Option<RadioIssue>)> {
    core.snapshot()
        .chats
        .into_iter()
        .find(|chat| chat.id == chat_id)
        .map(|chat| (chat.reach, chat.radio_issue))
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
async fn messages_past_the_hours_airtime_wait() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new("airtime-ada", &url);
    let bea_phone = Phone::new("airtime-bea", &url);
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    ada.create_account("Ada Lund").await.unwrap();
    bea.create_account("Bea Holm").await.unwrap();
    let chat_id = make_contacts(&ada, &bea).await;
    until("Bea's chat", || chat_state(&bea, &chat_id)).await;
    settled(&ada, &bea, &chat_id).await;
    let mesh = MockMesh::new();
    let ada_radio = Arc::new(mesh.radio("Ada's radio"));
    let bea_radio = Arc::new(mesh.radio("Bea's radio"));
    ada.attach_radio(ada_radio.clone()).await.unwrap();
    bea.attach_radio(bea_radio.clone()).await.unwrap();
    knows_radio(&ada, &chat_id).await;

    // Ada's radio may send two of these messages this hour, not three.
    let each = time_on_air(
        &MOCK_SETTINGS,
        meshcore_text_bytes((4 * (OVERHEAD_BYTES + "Short 1".len())).div_ceil(3)),
    );
    assert!(each > Duration::from_millis(400) && each < Duration::from_millis(800));
    ada.set_radio_hourly_airtime(each * 2 + Duration::from_millis(10));

    relay.block();
    until("Ada's chat cut off", || {
        (chat_state(&ada, &chat_id).map(|(reach, _)| reach) == Some(Reach::Unreachable))
            .then_some(())
    })
    .await;
    bea_radio.advertise();
    until("Ada's chat by radio", || {
        (chat_state(&ada, &chat_id) == Some((Reach::Radio, None))).then_some(())
    })
    .await;

    // Two go by radio.
    let mut air = bea_radio.events();
    for text in ["Short 1", "Short 2"] {
        let sent = ada.send_message(&chat_id, text).await.unwrap();
        assert_eq!(sent.transport, Transport::Radio, "{text}");
    }

    // That is the hour's airtime: the chat says so, and the third waits.
    until("the airtime used", || {
        (chat_state(&ada, &chat_id) == Some((Reach::Unreachable, Some(RadioIssue::AirtimeUsed))))
            .then_some(())
    })
    .await;
    let third = ada.send_message(&chat_id, "Short 3").await.unwrap();
    assert!(third.cancellable);
    tokio::time::sleep(Duration::from_secs(1)).await;
    let mut received = Vec::new();
    while let Ok(event) = air.try_recv() {
        if let RadioEvent::TextReceived { text, .. } = event {
            received.push(text);
        }
    }
    assert_eq!(received.len(), 2, "Bea's radio received {received:?}");
    let log = ada.radio_transmissions();
    assert_eq!(log.len(), 2);
    assert!(log.iter().all(|sent| sent.reason == "message"));
    let logged: u64 = log.iter().map(|sent| sent.airtime_ms).sum();
    assert_eq!(logged, 2 * each.as_millis() as u64);

    // Airtime comes free: the third goes by radio.
    ada.set_radio_hourly_airtime(Duration::from_secs(36));
    let sent = until("the third to go", || {
        messages(&ada, &chat_id)
            .into_iter()
            .find(|m| m.text == "Short 3" && !m.cancellable)
    })
    .await;
    assert_eq!(sent.transport, Transport::Radio);
    until("the third at Bea", || {
        messages(&bea, &chat_id)
            .into_iter()
            .find(|m| m.text == "Short 3")
    })
    .await;
    assert_eq!(ada.radio_transmissions().len(), 3);

    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

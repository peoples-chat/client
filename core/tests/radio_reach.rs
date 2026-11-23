//! Radio reach (#44): with the relay blocked, Ada's chat with Bea is reachable by radio only
//! while Bea's radio has been heard lately, on the preset of Ada's radio. Ada's app sends nothing
//! to find out: Bea's radio advertising, or a message from it, is what counts.

mod support;

use std::sync::Arc;
use std::time::Duration;

use peoples_chat_core::Core;
use peoples_chat_core::interface::{Mesh, RadioIssue, Reach};
use peoples_chat_core::radio::{MOCK_SETTINGS, MockMesh, Radio, RadioEvent, RadioSettings};
use support::{Phone, TestRelay, WAIT, make_contacts, until};
use tokio::time::Instant;

/// The window the test gives Ada's app, in place of the hour the app has.
const WINDOW: Duration = Duration::from_secs(4);

fn reach(core: &Core, chat_id: &str) -> Option<(Reach, Option<RadioIssue>)> {
    core.snapshot()
        .chats
        .into_iter()
        .find(|chat| chat.id == chat_id)
        .map(|chat| (chat.reach, chat.radio_issue))
}

async fn reach_becomes(core: &Core, chat_id: &str, wanted: (Reach, Option<RadioIssue>)) {
    until(&format!("the chat to be {wanted:?}"), || {
        (reach(core, chat_id) == Some(wanted)).then_some(())
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
async fn a_chat_is_reachable_by_radio_while_the_contacts_radio_is_heard() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new("reach-ada", &url);
    let bea_phone = Phone::new("reach-bea", &url);
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
    ada.set_radio_heard_window(WINDOW);
    // Everything Bea's radio hears from now on.
    let mut bea_air = bea_radio.events();

    // On the internet, radio doesn't matter.
    assert_eq!(reach(&ada, &chat_id), Some((Reach::Internet, None)));

    // The relay goes away. Bea's radio hasn't been heard.
    relay.block();
    reach_becomes(
        &ada,
        &chat_id,
        (Reach::Unreachable, Some(RadioIssue::NotHeard)),
    )
    .await;

    // Bea's radio advertises: Ada's chat can go by radio, until the window has passed.
    bea_radio.advertise();
    let heard = Instant::now();
    reach_becomes(&ada, &chat_id, (Reach::Radio, None)).await;
    reach_becomes(
        &ada,
        &chat_id,
        (Reach::Unreachable, Some(RadioIssue::NotHeard)),
    )
    .await;
    let lasted = heard.elapsed();
    assert!(
        lasted >= WINDOW && lasted < WINDOW + Duration::from_secs(3),
        "reachable for {lasted:?}"
    );

    // A message from Bea's radio counts as hearing it too.
    bea.set_force_radio(true).unwrap();
    bea.send_message(&chat_id, "Still here").await.unwrap();
    reach_becomes(&ada, &chat_id, (Reach::Radio, None)).await;

    // Ada's app sent nothing all this time: Bea's radio heard neither a message nor an advert.
    let mut heard_from_ada = Vec::new();
    while let Ok(event) = bea_air.try_recv() {
        if matches!(
            event,
            RadioEvent::TextReceived { .. } | RadioEvent::ContactHeard { .. }
        ) {
            heard_from_ada.push(event);
        }
    }
    assert!(heard_from_ada.is_empty(), "{heard_from_ada:?}");

    // Bea switches to a radio on another preset. Its record reaches Ada over the relay.
    relay.unblock();
    reach_becomes(&ada, &chat_id, (Reach::Internet, None)).await;
    let other_preset = RadioSettings {
        spreading_factor: 7,
        ..MOCK_SETTINGS
    };
    let bea_other = Arc::new(mesh.radio_with("Bea's other radio", other_preset));
    bea.attach_radio(bea_other.clone()).await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Without the relay again: heard or not, Bea's radio can't reach Ada's.
    relay.block();
    reach_becomes(
        &ada,
        &chat_id,
        (Reach::Unreachable, Some(RadioIssue::OtherPreset)),
    )
    .await;
    bea_other.advertise();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        reach(&ada, &chat_id),
        Some((Reach::Unreachable, Some(RadioIssue::OtherPreset)))
    );

    // Without its link to its own radio, Ada's app can't reach anyone by radio.
    ada_radio.disconnect().await.unwrap();
    reach_becomes(
        &ada,
        &chat_id,
        (Reach::Unreachable, Some(RadioIssue::NoRadio)),
    )
    .await;

    // The internet back, radio doesn't matter again.
    relay.unblock();
    reach_becomes(&ada, &chat_id, (Reach::Internet, None)).await;

    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

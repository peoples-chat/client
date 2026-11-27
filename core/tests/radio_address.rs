//! Radio address records (#34): once a phone has a radio, its app shares the radio's record in
//! each direct chat, and the contact's app keeps it as their radio, with no one doing anything.
//! A changed radio replaces the record, also one that comes while the app is closed, and a
//! request's record counts once the request is accepted.

mod support;

use std::sync::Arc;
use std::time::Duration;

use peoples_chat_core::Core;
use peoples_chat_core::interface::{CoreEvent, Mesh};
use peoples_chat_core::radio::{MockMesh, MockRadio, Radio, RadioEvent};
use support::{Phone, TestRelay, WAIT, make_contacts, next_invitation, until};
use tokio::sync::broadcast;
use tokio::time::Instant;

fn meshes(core: &Core, chat_id: &str) -> Vec<Mesh> {
    core.snapshot()
        .chats
        .into_iter()
        .find(|chat| chat.id == chat_id)
        .map(|chat| chat.contact.meshes)
        .unwrap_or_default()
}

async fn knows_radio(core: &Core, chat_id: &str) {
    until("the contact's radio", || {
        (meshes(core, chat_id) == vec![Mesh::MeshCore]).then_some(())
    })
    .await;
}

/// Waits until the chat has settled on both phones.
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

/// Waits until the radio whose events these are receives a message, for up to `within`.
async fn receives(events: &mut broadcast::Receiver<RadioEvent>, within: Duration) -> bool {
    let received = async {
        loop {
            match events.recv().await {
                Ok(RadioEvent::TextReceived { .. }) => return,
                Ok(_) => continue,
                Err(error) => panic!("the radio stopped: {error}"),
            }
        }
    };
    tokio::time::timeout(within, received).await.is_ok()
}

/// `from` sends a message by radio, and it reaches the radio `to`: `from` has the record of
/// `to`.
async fn reaches(from: &Core, chat_id: &str, to: &MockRadio, text: &str) {
    let mut air = to.events();
    from.send_message(chat_id, text).await.unwrap();
    assert!(receives(&mut air, WAIT).await, "{text:?} didn't arrive");
}

/// Like [`reaches`], but the record of `to` may still be on its way: until it arrives, messages
/// go to the radio before it, which is no harm.
async fn comes_to_reach(from: &Core, chat_id: &str, to: &MockRadio) {
    let deadline = Instant::now() + WAIT;
    loop {
        let mut air = to.events();
        from.send_message(chat_id, "Which radio?").await.unwrap();
        if receives(&mut air, Duration::from_secs(2)).await {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the new radio's record never came"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn contacts_learn_each_others_radios_from_the_chat() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new("address-ada", &url);
    let bea_phone = Phone::new("address-bea", &url);
    let cid_phone = Phone::new("address-cid", &url);
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
    assert!(meshes(&bea, &chat_id).is_empty());

    // Ada gets a radio. Bea's app learns of it, and says so.
    let mesh = MockMesh::new();
    let ada_radio = Arc::new(mesh.radio("Ada's radio"));
    let mut bea_events = bea.subscribe();
    ada.attach_radio(ada_radio.clone()).await.unwrap();
    let changed = loop {
        match tokio::time::timeout(WAIT, bea_events.recv())
            .await
            .unwrap()
            .unwrap()
        {
            CoreEvent::ContactChanged { contact } => break contact,
            _ => continue,
        }
    };
    assert_eq!(changed.name, "Ada Lund");
    assert_eq!(changed.meshes, vec![Mesh::MeshCore]);
    assert!(meshes(&ada, &chat_id).is_empty());

    // Bea gets one too, and each reaches the other's radio.
    let bea_radio = Arc::new(mesh.radio("Bea's radio"));
    bea.attach_radio(bea_radio.clone()).await.unwrap();
    knows_radio(&ada, &chat_id).await;
    ada.set_force_radio(true).unwrap();
    bea.set_force_radio(true).unwrap();
    reaches(&ada, &chat_id, &bea_radio, "To Bea's radio").await;
    reaches(&bea, &chat_id, &ada_radio, "To Ada's radio").await;

    // Ada changes radios. Bea's app takes the new record, and her messages go to it.
    let ada_second = Arc::new(mesh.radio("Ada's second radio"));
    ada.attach_radio(ada_second.clone()).await.unwrap();
    comes_to_reach(&bea, &chat_id, &ada_second).await;

    // A record that comes while Bea's app is closed counts when it starts again.
    bea.close().await.unwrap();
    let ada_third = Arc::new(mesh.radio("Ada's third radio"));
    ada.attach_radio(ada_third.clone()).await.unwrap();
    // Give the record time to reach the relay.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let bea = bea_phone.start().await;
    assert_eq!(meshes(&bea, &chat_id), vec![Mesh::MeshCore]);
    bea.attach_radio(bea_radio.clone()).await.unwrap();
    comes_to_reach(&bea, &chat_id, &ada_third).await;

    // Cid, a stranger with a radio, sends Bea a request. His record counts once she accepts.
    let cid = cid_phone.start().await;
    cid.create_account("Cid Berg").await.unwrap();
    cid.wait_until_published(WAIT).await.unwrap();
    let cid_radio = Arc::new(mesh.radio("Cid's radio"));
    cid.attach_radio(cid_radio.clone()).await.unwrap();
    let mut bea_events = bea.subscribe();
    let bea_link = bea.snapshot().account.unwrap().invite_link;
    let cid_chat = cid.add_contact(&bea_link).await.unwrap();
    let invitation = next_invitation(&mut bea_events).await;
    assert_eq!(invitation.id, cid_chat.id);
    assert!(invitation.from.meshes.is_empty());
    // Cid's app shares the record in the new chat; Bea's keeps it out while it is a request.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(bea.snapshot().invitations[0].from.meshes.is_empty());
    bea.accept_invitation(&invitation.id).await.unwrap();
    knows_radio(&bea, &cid_chat.id).await;

    ada.close().await.unwrap();
    bea.close().await.unwrap();
    cid.close().await.unwrap();
}

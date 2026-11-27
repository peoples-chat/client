//! Radio keys (#128): each member's app makes a random key for the chat once it has a radio, and
//! sends it there as an app event of kind 9912. Ada and Bea each have a radio on one mock mesh.

mod support;

use std::sync::Arc;
use std::time::Duration;

use peoples_chat_core::interface::{DeliveryState, Mesh, Message, Transport};
use peoples_chat_core::radio::{
    Mesh as RadioMesh, MockMesh, MockRadio, Radio, RadioEvent, RadioKey, RadioKeyRecord, format,
    new_radio_key,
};
use peoples_chat_core::{Core, Messaging, RADIO_KEY_KIND};
use support::{Phone, TestRelay, WAIT, make_contacts, now_ms, options, until};
use tokio::time::Instant;

fn messages(core: &Core, chat_id: &str) -> Vec<Message> {
    core.snapshot()
        .chats
        .into_iter()
        .find(|chat| chat.id == chat_id)
        .map(|chat| chat.messages)
        .unwrap_or_default()
}

fn texts(core: &Core, chat_id: &str) -> Vec<String> {
    messages(core, chat_id)
        .into_iter()
        .map(|message| message.text)
        .collect()
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

/// Waits until `to` has the MeshCore key that `from` sends with now, and returns it.
async fn has_key_of(to: &Core, from: &Core, chat_id: &str) -> RadioKeyRecord {
    until("the contact's radio key", || {
        let own = from.radio_own_key(chat_id, RadioMesh::MeshCore)?;
        (to.radio_contact_key(chat_id, RadioMesh::MeshCore)? == own).then_some(own)
    })
    .await
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

/// Two contacts, their phones, and a radio each.
struct Pair {
    relay: TestRelay,
    ada_phone: Phone,
    bea_phone: Phone,
    ada: Core,
    bea: Core,
    mesh: MockMesh,
    ada_radio: Arc<MockRadio>,
    bea_radio: Arc<MockRadio>,
    chat_id: String,
}

impl Pair {
    /// Attaches the radios and does nothing else: the apps exchange keys and addresses by
    /// themselves.
    async fn new(name: &str) -> Self {
        let relay = TestRelay::start().await;
        let url = relay.url();
        let ada_phone = Phone::new(&format!("{name}-ada"), &url);
        let bea_phone = Phone::new(&format!("{name}-bea"), &url);
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
        // No radio, no key.
        assert_eq!(ada.radio_own_key(&chat_id, RadioMesh::MeshCore), None);

        let mesh = MockMesh::new();
        let ada_radio = Arc::new(mesh.radio("Ada's radio"));
        let bea_radio = Arc::new(mesh.radio("Bea's radio"));
        ada.attach_radio(ada_radio.clone()).await.unwrap();
        bea.attach_radio(bea_radio.clone()).await.unwrap();
        has_key_of(&ada, &bea, &chat_id).await;
        has_key_of(&bea, &ada, &chat_id).await;
        knows_radio(&ada, &chat_id).await;
        knows_radio(&bea, &chat_id).await;
        Self {
            relay,
            ada_phone,
            bea_phone,
            ada,
            bea,
            mesh,
            ada_radio,
            bea_radio,
            chat_id,
        }
    }

    /// Ada sends `text` by radio, and Bea gets it.
    async fn ada_to_bea(&self, text: &str) {
        by_radio(&self.ada, &self.bea, &self.chat_id, text).await;
    }

    /// Hands Bea's radio a packet sealed with `key`, as Ada's radio would send it.
    async fn inject(&self, key: &RadioKey, number: u64, counter: u64, sent_at: u64, text: &str) {
        let sent_at = u32::try_from(sent_at / 1000).unwrap();
        let packet = format::seal_text(key, number, counter, sent_at, text);
        self.ada_radio
            .send_text(
                self.bea_radio.public_key(),
                &format::to_text_message(&packet),
            )
            .await
            .unwrap();
    }

    async fn close(self) {
        self.ada.close().await.unwrap();
        self.bea.close().await.unwrap();
    }
}

async fn by_radio(from: &Core, to: &Core, chat_id: &str, text: &str) {
    from.set_force_radio(true).unwrap();
    let sent = from.send_message(chat_id, text).await.unwrap();
    assert_eq!(sent.transport, Transport::Radio, "{text}");
    let received = until(text, || {
        messages(to, chat_id).into_iter().find(|m| m.id == sent.id)
    })
    .await;
    assert_eq!(received.text, text);
    assert_eq!(received.transport, Transport::Radio, "{text}");
    until("the confirmation", || {
        messages(from, chat_id)
            .iter()
            .any(|m| m.id == sent.id && m.delivery == Some(DeliveryState::Confirmed))
            .then_some(())
    })
    .await;
}

/// The next packet that `radio` hears.
async fn heard(air: &mut tokio::sync::broadcast::Receiver<RadioEvent>) -> Vec<u8> {
    loop {
        let event = tokio::time::timeout(WAIT, air.recv()).await.unwrap();
        if let RadioEvent::TextReceived { text, .. } = event.unwrap() {
            return format::from_text_message(&text).unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_128_keys_are_exchanged_by_themselves_and_survive_a_restart() {
    let _turn = support::take_turn().await;
    let pair = Pair::new("128-exchange").await;
    let chat_id = pair.chat_id.clone();
    let ada_key = pair
        .ada
        .radio_own_key(&chat_id, RadioMesh::MeshCore)
        .unwrap();
    let bea_key = pair
        .bea
        .radio_own_key(&chat_id, RadioMesh::MeshCore)
        .unwrap();
    assert_ne!(ada_key.key, bea_key.key);
    assert_eq!(ada_key.mesh, "meshcore");
    assert_eq!(ada_key.radio_key().unwrap().len(), 32);
    // The keys are not chat messages.
    assert!(messages(&pair.ada, &chat_id).is_empty());
    assert!(messages(&pair.bea, &chat_id).is_empty());

    pair.relay.block();
    let mut air = pair.bea_radio.events();
    pair.ada_to_bea("Hello Bea, with my key").await;
    // Sealed with Ada's key, and with no other.
    let packet = heard(&mut air).await;
    let counter = u64::from(format::Header::parse(&packet).unwrap().counter_low);
    let opens = |record: &RadioKeyRecord| {
        format::open_text(&record.radio_key().unwrap(), &packet, counter).is_ok()
    };
    assert!(opens(&ada_key));
    assert!(!opens(&bea_key));
    by_radio(&pair.bea, &pair.ada, &chat_id, "Hello Ada, with mine").await;

    // Both restart, with the relay still blocked. The keys are as they were.
    let ada_account = pair.ada.snapshot().account.unwrap().public_key;
    let bea_account = pair.bea.snapshot().account.unwrap().public_key;
    pair.ada.close().await.unwrap();
    pair.bea.close().await.unwrap();

    // What travelled: an app event of kind 9912 in the chat, authored by Ada.
    assert_eq!(RADIO_KEY_KIND, 9_912);
    let bea_messaging = Messaging::open(options(
        pair.bea_phone.data_dir(),
        &pair.relay.url(),
        pair.bea_phone.keychain_service(),
    ))
    .await
    .unwrap();
    let records = bea_messaging
        .radio_key_records(&bea_account, &chat_id)
        .unwrap();
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].sender, ada_account);
    assert_eq!(RadioKeyRecord::parse(&records[0].record).unwrap(), ada_key);
    bea_messaging.close().await.unwrap();

    let ada = pair.ada_phone.start().await;
    let bea = pair.bea_phone.start().await;
    for (core, own, contact) in [(&ada, &ada_key, &bea_key), (&bea, &bea_key, &ada_key)] {
        assert_eq!(
            core.radio_own_key(&chat_id, RadioMesh::MeshCore).as_ref(),
            Some(own)
        );
        assert_eq!(
            core.radio_contact_key(&chat_id, RadioMesh::MeshCore)
                .as_ref(),
            Some(contact)
        );
    }
    ada.attach_radio(pair.ada_radio.clone()).await.unwrap();
    bea.attach_radio(pair.bea_radio.clone()).await.unwrap();
    // Attaching again makes no new key.
    assert_eq!(
        ada.radio_own_key(&chat_id, RadioMesh::MeshCore).as_ref(),
        Some(&ada_key)
    );
    by_radio(&ada, &bea, &chat_id, "Still the same keys").await;
    by_radio(&bea, &ada, &chat_id, "On this side too").await;

    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_128_a_membership_change_brings_a_new_key_and_retires_the_old_one() {
    let _turn = support::take_turn().await;
    let pair = Pair::new("128-membership").await;
    let chat_id = pair.chat_id.clone();
    let old = pair
        .ada
        .radio_own_key(&chat_id, RadioMesh::MeshCore)
        .unwrap();
    let old_key = old.radio_key().unwrap();

    // Nothing changed: the same key.
    pair.ada.refresh_radio_keys().await;
    assert_eq!(
        pair.ada.radio_own_key(&chat_id, RadioMesh::MeshCore),
        Some(old.clone())
    );

    // Bea's side of the membership changes, as Ada sees it.
    pair.ada.set_radio_contact_devices(&chat_id, 2);
    pair.ada.refresh_radio_keys().await;
    let new = pair
        .ada
        .radio_own_key(&chat_id, RadioMesh::MeshCore)
        .unwrap();
    assert_ne!(new.key, old.key);
    assert_eq!(new.n, old.n + 1);
    assert!(new.at > old.at);
    assert_eq!(has_key_of(&pair.bea, &pair.ada, &chat_id).await, new);

    // What Ada sends now, someone who holds only the old key can't open.
    let mut air = pair.bea_radio.events();
    pair.ada_to_bea("After the change").await;
    let packet = heard(&mut air).await;
    let header = format::Header::parse(&packet).unwrap();
    assert_eq!(header.key_hint, new.n as u8);
    let counter = u64::from(header.counter_low);
    assert_eq!(
        format::open_text(&old_key, &packet, counter),
        Err(format::PacketError::NotAuthentic)
    );
    assert!(format::open_text(&new.radio_key().unwrap(), &packet, counter).is_ok());

    // And the old key no longer opens new packets on Bea's phone: one sealed with it after the
    // new key was made shows nothing. One that was on the way already still does.
    pair.inject(&old_key, old.n, 1_000, new.at + 2_000, "Sealed too late")
        .await;
    pair.inject(&old_key, old.n, 1_001, old.at, "Was on the way")
        .await;
    until("the packet that was on the way", || {
        texts(&pair.bea, &chat_id)
            .contains(&"Was on the way".to_owned())
            .then_some(())
    })
    .await;
    assert!(!texts(&pair.bea, &chat_id).contains(&"Sealed too late".to_owned()));

    // A packet under a key Bea never got is dropped silently: nothing shows, nothing is sent.
    let before = texts(&pair.bea, &chat_id);
    let eve = pair.mesh.radio("Eve's radio");
    let mut eve_air = eve.events();
    for number in [new.n, new.n + 40] {
        let packet = format::seal_text(
            &new_radio_key(),
            number,
            0,
            u32::try_from(now_ms() / 1000).unwrap(),
            "Under an unknown key",
        );
        eve.send_text(
            pair.bea_radio.public_key(),
            &format::to_text_message(&packet),
        )
        .await
        .unwrap();
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(texts(&pair.bea, &chat_id), before);
    assert!(pair.bea.radio_transmissions().is_empty());
    while let Ok(event) = eve_air.try_recv() {
        assert!(
            !matches!(event, RadioEvent::TextReceived { .. }),
            "{event:?}"
        );
    }

    pair.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_128_each_mesh_has_its_key_and_an_unknown_mesh_is_kept_and_ignored() {
    let _turn = support::take_turn().await;
    let pair = Pair::new("128-meshes").await;
    let chat_id = pair.chat_id.clone();
    let meshcore = pair
        .bea
        .radio_own_key(&chat_id, RadioMesh::MeshCore)
        .unwrap();

    // Bea's app has no Meshtastic radio to make these for, so the test sends them. The newer
    // record of a version nobody knows is refused.
    let meshtastic = RadioKeyRecord::new("meshtastic", &new_radio_key(), 1, now_ms());
    let unknown = RadioKeyRecord::new("a-mesh-of-the-future", &new_radio_key(), 1, now_ms());
    let later_version = meshcore.to_json().replace(r#""v":0"#, r#""v":1"#);
    for record in [later_version, unknown.to_json(), meshtastic.to_json()] {
        pair.bea
            .send_radio_key_record(&chat_id, &record)
            .await
            .unwrap();
    }
    until("Bea's three keys", || {
        (pair.ada.radio_contact_keys(&chat_id).len() == 3).then_some(())
    })
    .await;
    let kept = pair.ada.radio_contact_keys(&chat_id);
    for record in [&meshcore, &meshtastic, &unknown] {
        assert!(kept.contains(record), "{record:?}");
    }

    // Each mesh picks its own key.
    assert_eq!(
        pair.ada.radio_contact_key(&chat_id, RadioMesh::MeshCore),
        Some(meshcore.clone())
    );
    assert_eq!(
        pair.ada.radio_contact_key(&chat_id, RadioMesh::Meshtastic),
        Some(meshtastic.clone())
    );

    // On a MeshCore radio, only the MeshCore key opens packets.
    pair.relay.block();
    by_radio(&pair.bea, &pair.ada, &chat_id, "By MeshCore").await;
    for (record, text) in [(&meshtastic, "Wrong mesh"), (&unknown, "Unknown mesh")] {
        let packet = format::seal_text(
            &record.radio_key().unwrap(),
            record.n,
            500,
            u32::try_from(now_ms() / 1000).unwrap(),
            text,
        );
        pair.bea_radio
            .send_text(
                pair.ada_radio.public_key(),
                &format::to_text_message(&packet),
            )
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(texts(&pair.ada, &chat_id), vec!["By MeshCore".to_owned()]);

    pair.close().await;
}

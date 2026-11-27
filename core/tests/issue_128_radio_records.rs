//! Radio records, version 1 (#128): each member's app keeps its radio address record, with its
//! radio key, in the direct chat's state, as an MLS app component. Ada and Bea each have a radio
//! on one mock mesh. Where a test looks at what travelled, or stands in for another app, it
//! drives MDK directly, as the spike (#117) did.

mod support;

use std::sync::Arc;
use std::time::Duration;

use cgka_traits::{GroupId, TransportEndpoint};
use marmot_account::AccountHome;
use marmot_app::{
    AppGroupLifecycleState, MarmotApp, MarmotAppConfig, MarmotAppRuntime, TimelineMessageQuery,
};
use peoples_chat_core::Core;
use peoples_chat_core::interface::{DeliveryState, Mesh, Message, Transport};
use peoples_chat_core::radio::{
    Mesh as RadioMesh, MockMesh, MockRadio, Radio, RadioEvent, RadioKey, RadioRecord,
    encode_component, format, new_radio_key, radio_component_id,
};
use serde_json::Value;
use support::{Phone, TestRelay, WAIT, make_contacts, now_ms, until};
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

fn meshes(core: &Core, chat_id: &str) -> Vec<Mesh> {
    core.snapshot()
        .chats
        .into_iter()
        .find(|chat| chat.id == chat_id)
        .map(|chat| chat.contact.meshes)
        .unwrap_or_default()
}

fn account(core: &Core) -> String {
    core.snapshot().account.unwrap().public_key
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

/// Waits until `to` has the record that `from` has for `mesh` now, and returns it.
async fn has_record_of(to: &Core, from: &Core, chat_id: &str, mesh: RadioMesh) -> RadioRecord {
    until("the contact's radio record", || {
        let own = from.radio_own_record(chat_id, mesh)?;
        (to.radio_contact_record(chat_id, mesh)? == own).then_some(own)
    })
    .await
}

/// Two contacts, their phones, and a chat that both have.
struct Chat {
    relay: TestRelay,
    ada_phone: Phone,
    bea_phone: Phone,
    ada: Core,
    bea: Core,
    mesh: MockMesh,
    chat_id: String,
}

impl Chat {
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
        Self {
            relay,
            ada_phone,
            bea_phone,
            ada,
            bea,
            mesh: MockMesh::new(),
            chat_id,
        }
    }

    /// Gives each a MeshCore radio and does nothing else: the apps exchange their records by
    /// themselves.
    async fn radios(&self) -> (Arc<MockRadio>, Arc<MockRadio>) {
        let ada_radio = Arc::new(self.mesh.radio("Ada's radio"));
        let bea_radio = Arc::new(self.mesh.radio("Bea's radio"));
        self.ada.attach_radio(ada_radio.clone()).await.unwrap();
        self.bea.attach_radio(bea_radio.clone()).await.unwrap();
        self.exchanged(RadioMesh::MeshCore).await;
        (ada_radio, bea_radio)
    }

    /// Waits until each has the other's record for `mesh`.
    async fn exchanged(&self, mesh: RadioMesh) {
        has_record_of(&self.ada, &self.bea, &self.chat_id, mesh).await;
        has_record_of(&self.bea, &self.ada, &self.chat_id, mesh).await;
        settled(&self.ada, &self.bea, &self.chat_id).await;
    }

    async fn close(self) {
        self.ada.close().await.unwrap();
        self.bea.close().await.unwrap();
    }
}

/// Hands the radio `to` a packet sealed with `key`, from the radio `from`.
async fn inject(
    from: &MockRadio,
    to: &MockRadio,
    key: &RadioKey,
    number: u64,
    counter: u64,
    sent_at: u64,
    text: &str,
) {
    let sent_at = u32::try_from(sent_at / 1000).unwrap();
    let packet = format::seal_text(key, number, counter, sent_at, text);
    from.send_text(to.public_key(), &format::to_text_message(&packet))
        .await
        .unwrap();
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

/// The next packet that the radio with these events hears.
async fn heard(air: &mut tokio::sync::broadcast::Receiver<RadioEvent>) -> Vec<u8> {
    loop {
        let event = tokio::time::timeout(WAIT, air.recv()).await.unwrap();
        if let RadioEvent::TextReceived { text, .. } = event.unwrap() {
            return format::from_text_message(&text).unwrap();
        }
    }
}

/// MDK on a phone whose core is closed.
async fn mdk(phone: &Phone, relay: &str) -> MarmotAppRuntime {
    let home = AccountHome::open_with_keychain(phone.data_dir(), phone.keychain_service()).unwrap();
    let config = MarmotAppConfig::default().with_allow_loopback_relay_endpoints(true);
    let app = MarmotApp::try_with_relays_and_account_home_and_config(
        phone.data_dir(),
        vec![TransportEndpoint(relay.to_owned()).0],
        home,
        config,
    )
    .unwrap();
    let runtime = app.runtime();
    runtime.start().await.unwrap();
    runtime
}

fn group(chat_id: &str) -> GroupId {
    GroupId::new(hex::decode(chat_id).unwrap())
}

/// The records in the component of `owner`, in the chat with `peer`, as `device` has them.
async fn component(
    runtime: &MarmotAppRuntime,
    device: &str,
    chat_id: &str,
    owner: &str,
    peer: &str,
) -> Vec<Value> {
    let data = runtime
        .group_app_component(device, &group(chat_id), radio_component_id(owner, peer))
        .await
        .unwrap()
        .expect("the component is there");
    serde_json::from_slice(&data).unwrap()
}

/// Writes `data` as the component of `owner`, once MDK has the chat settled.
async fn write_component(
    runtime: &MarmotAppRuntime,
    owner: &str,
    chat_id: &str,
    peer: &str,
    data: Vec<u8>,
) {
    let id = radio_component_id(owner, peer);
    let deadline = Instant::now() + WAIT;
    let mut refused = None;
    loop {
        let state = runtime
            .group_mls_state(owner, &group(chat_id))
            .await
            .unwrap();
        if state.lifecycle_state == AppGroupLifecycleState::Stable {
            if runtime
                .group_app_component(owner, &group(chat_id), id)
                .await
                .unwrap()
                .as_ref()
                == Some(&data)
            {
                return;
            }
            if let Err(error) = runtime
                .update_app_component(owner, &group(chat_id), id, data.clone())
                .await
            {
                refused = Some(error.to_string());
            }
        }
        assert!(
            Instant::now() < deadline,
            "the component wasn't written: {refused:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_128_records_travel_in_the_chat_state_show_nothing_and_survive_a_restart() {
    let _turn = support::take_turn().await;
    let chat = Chat::new("128-state").await;
    let chat_id = chat.chat_id.clone();
    // No radio, no record.
    assert_eq!(
        chat.ada.radio_own_record(&chat_id, RadioMesh::MeshCore),
        None
    );
    let (ada_radio, bea_radio) = chat.radios().await;

    let ada_record = chat
        .ada
        .radio_own_record(&chat_id, RadioMesh::MeshCore)
        .unwrap();
    let bea_record = chat
        .bea
        .radio_own_record(&chat_id, RadioMesh::MeshCore)
        .unwrap();
    assert_eq!(ada_record.v, 1);
    assert_eq!(ada_record.name, "Ada's radio");
    assert_eq!(ada_record.addr.key(), ada_radio.public_key());
    assert_eq!(ada_record.radio_key().unwrap().len(), 32);
    assert_ne!(ada_record.rk, bea_record.rk);
    assert!(ada_record.to_json().len() < 300);
    assert_eq!(meshes(&chat.ada, &chat_id), vec![Mesh::MeshCore]);
    assert_eq!(meshes(&chat.bea, &chat_id), vec![Mesh::MeshCore]);
    // Nothing shows.
    assert!(messages(&chat.ada, &chat_id).is_empty());
    assert!(messages(&chat.bea, &chat_id).is_empty());

    chat.relay.block();
    let mut air = bea_radio.events();
    by_radio(&chat.ada, &chat.bea, &chat_id, "Hello Bea, with my key").await;
    // Sealed with Ada's key, and with no other.
    let packet = heard(&mut air).await;
    let counter = u64::from(format::Header::parse(&packet).unwrap().counter_low);
    let opens = |record: &RadioRecord| {
        format::open_text(&record.radio_key().unwrap(), &packet, counter).is_ok()
    };
    assert!(opens(&ada_record));
    assert!(!opens(&bea_record));
    by_radio(&chat.bea, &chat.ada, &chat_id, "Hello Ada, with mine").await;

    // Both restart, with the relay still blocked.
    let (ada_account, bea_account) = (account(&chat.ada), account(&chat.bea));
    chat.ada.close().await.unwrap();
    chat.bea.close().await.unwrap();

    // What Bea's copy of the chat holds, as MDK has it: each record in its owner's component,
    // and no event of the app's kinds.
    let runtime = mdk(&chat.bea_phone, &chat.relay.url()).await;
    for (record, owner, peer) in [
        (&ada_record, &ada_account, &bea_account),
        (&bea_record, &bea_account, &ada_account),
    ] {
        assert_eq!(
            component(&runtime, &bea_account, &chat_id, owner, peer).await,
            vec![serde_json::to_value(record).unwrap()]
        );
    }
    assert_ne!(
        radio_component_id(&ada_account, &bea_account),
        radio_component_id(&bea_account, &ada_account)
    );
    let rows = runtime
        .timeline_messages_with_query(
            &bea_account,
            TimelineMessageQuery {
                group_id_hex: Some(chat_id.clone()),
                ..TimelineMessageQuery::default()
            },
        )
        .unwrap();
    // Only the texts sent by radio, which waited for the relay: kind 9.
    assert!(
        rows.messages.iter().all(|row| row.kind == 9),
        "{:?}",
        rows.messages.iter().map(|row| row.kind).collect::<Vec<_>>()
    );
    runtime.shutdown_and_close().await.unwrap();

    let ada = chat.ada_phone.start().await;
    let bea = chat.bea_phone.start().await;
    for (core, own, contact) in [
        (&ada, &ada_record, &bea_record),
        (&bea, &bea_record, &ada_record),
    ] {
        assert_eq!(
            core.radio_own_record(&chat_id, RadioMesh::MeshCore)
                .as_ref(),
            Some(own)
        );
        assert_eq!(
            core.radio_contact_record(&chat_id, RadioMesh::MeshCore)
                .as_ref(),
            Some(contact)
        );
    }
    ada.attach_radio(ada_radio.clone()).await.unwrap();
    bea.attach_radio(bea_radio.clone()).await.unwrap();
    // The same radio makes no new key.
    assert_eq!(
        ada.radio_own_record(&chat_id, RadioMesh::MeshCore).as_ref(),
        Some(&ada_record)
    );
    by_radio(&ada, &bea, &chat_id, "Still the same keys").await;
    by_radio(&bea, &ada, &chat_id, "On this side too").await;

    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_128_a_membership_change_or_another_radio_brings_a_new_key() {
    let _turn = support::take_turn().await;
    let chat = Chat::new("128-membership").await;
    let chat_id = chat.chat_id.clone();
    let (ada_radio, bea_radio) = chat.radios().await;
    let old = chat
        .ada
        .radio_own_record(&chat_id, RadioMesh::MeshCore)
        .unwrap();
    let old_key = old.radio_key().unwrap();

    // Nothing changed: the same record.
    chat.ada.refresh_radio_records().await;
    assert_eq!(
        chat.ada.radio_own_record(&chat_id, RadioMesh::MeshCore),
        Some(old.clone())
    );

    // Bea's side of the membership changes, as Ada sees it.
    chat.ada.set_radio_contact_devices(&chat_id, 2);
    chat.ada.refresh_radio_records().await;
    let new = chat
        .ada
        .radio_own_record(&chat_id, RadioMesh::MeshCore)
        .unwrap();
    assert_ne!(new.rk, old.rk);
    assert_eq!(new.rkn, old.rkn + 1);
    assert!(new.at > old.at);
    assert_eq!(
        has_record_of(&chat.bea, &chat.ada, &chat_id, RadioMesh::MeshCore).await,
        new
    );
    settled(&chat.ada, &chat.bea, &chat_id).await;

    // What Ada sends now, someone who holds only the old key can't open.
    let mut air = bea_radio.events();
    by_radio(&chat.ada, &chat.bea, &chat_id, "After the change").await;
    let packet = heard(&mut air).await;
    let header = format::Header::parse(&packet).unwrap();
    assert_eq!(header.key_hint, new.rkn as u8);
    let counter = u64::from(header.counter_low);
    assert_eq!(
        format::open_text(&old_key, &packet, counter),
        Err(format::PacketError::NotAuthentic)
    );
    assert!(format::open_text(&new.radio_key().unwrap(), &packet, counter).is_ok());

    // And the old key no longer opens new packets on Bea's phone: one sealed with it after the
    // new key was made shows nothing. One that was on the way already still does.
    let late = new.at + 2_000;
    inject(
        &ada_radio, &bea_radio, &old_key, old.rkn, 1_000, late, "Too late",
    )
    .await;
    inject(
        &ada_radio,
        &bea_radio,
        &old_key,
        old.rkn,
        1_001,
        old.at,
        "On the way",
    )
    .await;
    until("the packet that was on the way", || {
        texts(&chat.bea, &chat_id)
            .contains(&"On the way".to_owned())
            .then_some(())
    })
    .await;
    assert!(!texts(&chat.bea, &chat_id).contains(&"Too late".to_owned()));

    // A packet under a key Bea never got is dropped silently: nothing shows, nothing is sent.
    let before = texts(&chat.bea, &chat_id);
    let eve = chat.mesh.radio("Eve's radio");
    let mut eve_air = eve.events();
    for number in [new.rkn, new.rkn + 40] {
        let unknown = new_radio_key();
        inject(
            &eve,
            &bea_radio,
            &unknown,
            number,
            0,
            now_ms(),
            "Unknown key",
        )
        .await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(texts(&chat.bea, &chat_id), before);
    assert!(chat.bea.radio_transmissions().is_empty());
    while let Ok(event) = eve_air.try_recv() {
        assert!(
            !matches!(event, RadioEvent::TextReceived { .. }),
            "{event:?}"
        );
    }

    // Another radio, or the same one on other settings, gets a new record with a new key.
    let second = Arc::new(chat.mesh.radio("Ada's second radio"));
    chat.ada.attach_radio(second.clone()).await.unwrap();
    let with_second = has_record_of(&chat.bea, &chat.ada, &chat_id, RadioMesh::MeshCore).await;
    assert_eq!(with_second.addr.key(), second.public_key());
    assert_ne!(with_second.rk, new.rk);
    assert_eq!(with_second.rkn, new.rkn + 1);
    let mut settings = peoples_chat_core::radio::MOCK_SETTINGS;
    settings.spreading_factor = 7;
    let narrow = Arc::new(chat.mesh.radio_with("Ada's other settings", settings));
    chat.ada.attach_radio(narrow).await.unwrap();
    let with_settings = has_record_of(&chat.bea, &chat.ada, &chat_id, RadioMesh::MeshCore).await;
    assert_eq!(with_settings.sf, 7);
    assert_ne!(with_settings.rk, with_second.rk);

    chat.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_128_a_meshcore_and_a_meshtastic_record_are_each_used_for_their_mesh() {
    let _turn = support::take_turn().await;
    let chat = Chat::new("128-meshes").await;
    let chat_id = chat.chat_id.clone();
    let (ada_radio, bea_radio) = chat.radios().await;
    let meshcore = chat
        .ada
        .radio_own_record(&chat_id, RadioMesh::MeshCore)
        .unwrap();

    // Each moves to a Meshtastic radio. Both records stay in the chat's state.
    let ada_node = Arc::new(chat.mesh.meshtastic_radio("Ada's node"));
    let bea_node = Arc::new(chat.mesh.meshtastic_radio("Bea's node"));
    chat.ada.attach_radio(ada_node.clone()).await.unwrap();
    chat.bea.attach_radio(bea_node.clone()).await.unwrap();
    chat.exchanged(RadioMesh::Meshtastic).await;
    let meshtastic = chat
        .bea
        .radio_contact_record(&chat_id, RadioMesh::Meshtastic)
        .unwrap();
    assert_eq!(meshtastic.preset.as_deref(), Some("LONG_FAST"));
    assert_eq!(meshtastic.region.as_deref(), Some("EU_868"));
    assert_eq!(meshtastic.addr.key(), ada_node.public_key());
    assert_ne!(meshtastic.rk, meshcore.rk);
    assert_eq!(
        chat.bea
            .radio_contact_record(&chat_id, RadioMesh::MeshCore)
            .as_ref(),
        Some(&meshcore)
    );
    assert_eq!(
        chat.bea.radio_contact_meshes(&chat_id),
        vec!["meshcore", "meshtastic"]
    );

    // No message goes through a Meshtastic radio today.
    chat.relay.block();
    chat.ada.set_force_radio(true).unwrap();
    let refused = chat.ada.send_message(&chat_id, "By node").await;
    assert_eq!(
        refused.unwrap_err().message,
        "Only a MeshCore radio can send messages."
    );

    // On her Meshtastic radio, Bea opens packets with Ada's Meshtastic key alone.
    let key = |record: &RadioRecord| record.radio_key().unwrap();
    inject(
        &ada_node,
        &bea_node,
        &key(&meshcore),
        meshcore.rkn,
        500,
        now_ms(),
        "Wrong mesh",
    )
    .await;
    inject(
        &ada_node,
        &bea_node,
        &key(&meshtastic),
        meshtastic.rkn,
        501,
        now_ms(),
        "Meshtastic key",
    )
    .await;
    until("the Meshtastic packet", || {
        (texts(&chat.bea, &chat_id) == vec!["Meshtastic key".to_owned()]).then_some(())
    })
    .await;

    // Back on MeshCore, the records and keys are those from before, and each is used for it.
    chat.ada.attach_radio(ada_radio.clone()).await.unwrap();
    chat.bea.attach_radio(bea_radio.clone()).await.unwrap();
    assert_eq!(
        chat.ada
            .radio_own_record(&chat_id, RadioMesh::MeshCore)
            .as_ref(),
        Some(&meshcore)
    );
    inject(
        &ada_radio,
        &bea_radio,
        &key(&meshtastic),
        meshtastic.rkn,
        502,
        now_ms(),
        "Wrong mesh again",
    )
    .await;
    by_radio(&chat.ada, &chat.bea, &chat_id, "By MeshCore").await;
    assert_eq!(
        texts(&chat.bea, &chat_id),
        vec!["Meshtastic key".to_owned(), "By MeshCore".to_owned()]
    );

    chat.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_128_a_record_with_an_unknown_mesh_is_kept_and_ignored() {
    let _turn = support::take_turn().await;
    let chat = Chat::new("128-unknown").await;
    let chat_id = chat.chat_id.clone();
    let ada_radio = Arc::new(chat.mesh.radio("Ada's radio"));
    chat.ada.attach_radio(ada_radio.clone()).await.unwrap();

    // Another app of Bea's writes her component: a mesh this app doesn't know, and MeshCore.
    let (ada_account, bea_account) = (account(&chat.ada), account(&chat.bea));
    let bea_radio = chat.mesh.radio("Bea's radio");
    let record = RadioRecord::new(
        &bea_radio.info().await.unwrap(),
        &new_radio_key(),
        1,
        now_ms(),
    );
    let future_key = new_radio_key();
    let mut future = serde_json::to_value(&record).unwrap();
    future["mesh"] = "a-mesh-of-the-future".into();
    future["rk"] = hex::encode(future_key.as_slice()).into();
    let data = format!("[{future},{}]", record.to_json()).into_bytes();
    chat.bea.close().await.unwrap();
    let runtime = mdk(&chat.bea_phone, &chat.relay.url()).await;
    write_component(&runtime, &bea_account, &chat_id, &ada_account, data).await;

    until("Bea's two records", || {
        (chat.ada.radio_contact_meshes(&chat_id) == vec!["a-mesh-of-the-future", "meshcore"])
            .then_some(())
    })
    .await;
    assert_eq!(
        chat.ada.radio_contact_record(&chat_id, RadioMesh::MeshCore),
        Some(record.clone())
    );
    assert_eq!(meshes(&chat.ada, &chat_id), vec![Mesh::MeshCore]);

    // Only the MeshCore key opens packets.
    inject(
        &bea_radio,
        &ada_radio,
        &future_key,
        1,
        0,
        now_ms(),
        "Unknown mesh",
    )
    .await;
    inject(
        &bea_radio,
        &ada_radio,
        &record.radio_key().unwrap(),
        1,
        1,
        now_ms(),
        "Known mesh",
    )
    .await;
    until("the MeshCore packet", || {
        (texts(&chat.ada, &chat_id) == vec!["Known mesh".to_owned()]).then_some(())
    })
    .await;

    // A list with nothing the app can use, or no list at all, is no reason to fail.
    write_component(
        &runtime,
        &bea_account,
        &chat_id,
        &ada_account,
        encode_component([]),
    )
    .await;
    until("no records", || {
        chat.ada
            .radio_contact_meshes(&chat_id)
            .is_empty()
            .then_some(())
    })
    .await;

    runtime.shutdown_and_close().await.unwrap();
    chat.ada.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_128_a_version_0_event_from_an_older_build_is_still_read() {
    let _turn = support::take_turn().await;
    let chat = Chat::new("128-version-0").await;
    let chat_id = chat.chat_id.clone();
    let bea_account = account(&chat.bea);
    let bea_radio = chat.mesh.radio("Bea's radio");
    let old =
        peoples_chat_core::radio::RadioAddress::new(&bea_radio.info().await.unwrap(), now_ms());

    // An older build on Bea's phone sends its record as an app event of kind 9911.
    chat.bea.close().await.unwrap();
    let runtime = mdk(&chat.bea_phone, &chat.relay.url()).await;
    runtime
        .send_custom_event(
            &bea_account,
            &group(&chat_id),
            9_911,
            Vec::new(),
            old.to_json(),
        )
        .await
        .unwrap();

    until("Bea's radio", || {
        (meshes(&chat.ada, &chat_id) == vec![Mesh::MeshCore]).then_some(())
    })
    .await;
    // The address and settings only: no key. And nothing shows.
    assert_eq!(
        chat.ada.radio_contact_record(&chat_id, RadioMesh::MeshCore),
        None
    );
    assert!(messages(&chat.ada, &chat_id).is_empty());

    runtime.shutdown_and_close().await.unwrap();
    chat.ada.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_128_two_members_writing_at_the_same_moment_each_keep_their_record() {
    let _turn = support::take_turn().await;
    let chat = Chat::new("128-race").await;
    let chat_id = chat.chat_id.clone();

    // Both pair a radio during an outage: both changes are made in the same epoch.
    chat.relay.block();
    let ada_radio = Arc::new(chat.mesh.radio("Ada's radio"));
    let bea_radio = Arc::new(chat.mesh.radio("Bea's radio"));
    chat.ada.attach_radio(ada_radio.clone()).await.unwrap();
    chat.bea.attach_radio(bea_radio.clone()).await.unwrap();
    // The writes wait for the relay, and a message can still be written meanwhile.
    assert!(chat.ada.chat_membership(&chat_id).await.is_err());
    assert!(chat.bea.chat_membership(&chat_id).await.is_err());
    chat.ada
        .send_message(&chat_id, "Written during the outage")
        .await
        .unwrap();

    chat.relay.unblock();
    chat.exchanged(RadioMesh::MeshCore).await;
    // Once MDK has settled the race, both records are still there.
    tokio::time::sleep(Duration::from_secs(3)).await;
    chat.exchanged(RadioMesh::MeshCore).await;
    until("the message written during the outage", || {
        texts(&chat.bea, &chat_id)
            .contains(&"Written during the outage".to_owned())
            .then_some(())
    })
    .await;

    chat.relay.block();
    by_radio(&chat.ada, &chat.bea, &chat_id, "Both kept theirs").await;
    by_radio(&chat.bea, &chat.ada, &chat_id, "So they did").await;

    chat.close().await;
}

/// Joins the group `group_id` on a phone whose core is closed, once MDK has the invitation.
async fn join(runtime: &MarmotAppRuntime, device: &str, group_id: &str) {
    let deadline = Instant::now() + WAIT;
    while let Err(error) = runtime.accept_group_invite(device, &group(group_id)).await {
        assert!(
            Instant::now() < deadline,
            "the group wasn't joined: {error}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The component `id` of the group `group_id`, as `device` has it.
async fn raw_component(
    runtime: &MarmotAppRuntime,
    device: &str,
    group_id: &str,
    id: u16,
) -> Option<Vec<u8>> {
    runtime
        .group_app_component(device, &group(group_id), id)
        .await
        .unwrap()
}

/// Starts each phone's core with its radio.
async fn start_all(phones: &[Phone], radios: &[Arc<MockRadio>]) -> Vec<Core> {
    let mut cores = Vec::new();
    for (phone, radio) in phones.iter().zip(radios) {
        let core = phone.start().await;
        core.attach_radio(radio.clone()).await.unwrap();
        cores.push(core);
    }
    cores
}

async fn close_all(cores: Vec<Core>) {
    for core in cores {
        core.close().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_128_a_group_chat_gets_no_radio_record() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let phones = ["ada", "bea", "cai"].map(|name| Phone::new(&format!("128-group-{name}"), &url));
    let mut cores = Vec::new();
    for (phone, name) in phones.iter().zip(["Ada Lund", "Bea Holm", "Cai Berg"]) {
        let core = phone.start().await;
        core.create_account(name).await.unwrap();
        cores.push(core);
    }
    let accounts: Vec<String> = cores.iter().map(account).collect();

    // All three are direct contacts of each other, each with a radio, and each direct chat gets
    // both records.
    let pairs = [(0, 1), (0, 2), (1, 2)];
    let mut chats = Vec::new();
    for (one, other) in pairs {
        let chat_id = make_contacts(&cores[one], &cores[other]).await;
        until("the other side of the chat", || {
            cores[other]
                .snapshot()
                .chats
                .iter()
                .any(|chat| chat.id == chat_id)
                .then_some(())
        })
        .await;
        settled(&cores[one], &cores[other], &chat_id).await;
        chats.push(chat_id);
    }
    let mesh = MockMesh::new();
    let radios =
        ["Ada's radio", "Bea's radio", "Cai's radio"].map(|name| Arc::new(mesh.radio(name)));
    for (core, radio) in cores.iter().zip(&radios) {
        core.attach_radio(radio.clone()).await.unwrap();
    }
    // For each pair, the record that each of the two has from the other.
    let mut records = Vec::new();
    for ((one, other), chat_id) in pairs.into_iter().zip(&chats) {
        let of_one = has_record_of(&cores[other], &cores[one], chat_id, RadioMesh::MeshCore).await;
        let of_other =
            has_record_of(&cores[one], &cores[other], chat_id, RadioMesh::MeshCore).await;
        settled(&cores[one], &cores[other], chat_id).await;
        records.push((of_one, of_other));
    }

    // Ada makes a group of the three through MDK, and the others join it. Ada's MDK keeps
    // running meanwhile: it sends the invitations after making the group.
    close_all(cores).await;
    let ada = mdk(&phones[0], &url).await;
    let group_id = ada
        .create_group(&accounts[0], "Trail crew", &accounts[1..], None)
        .await
        .unwrap()
        .operation
        .expect("the group was made")
        .to_string();
    for member in [1, 2] {
        let runtime = mdk(&phones[member], &url).await;
        join(&runtime, &accounts[member], &group_id).await;
        runtime.shutdown_and_close().await.unwrap();
    }
    ada.shutdown_and_close().await.unwrap();

    // The apps run again with their radios, and look at their records.
    let cores = start_all(&phones, &radios).await;
    for core in &cores {
        core.refresh_radio_records().await;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    for core in &cores {
        core.refresh_radio_records().await;
        assert!(core.snapshot().chats.iter().all(|chat| chat.id != group_id));
        assert_eq!(core.radio_own_record(&group_id, RadioMesh::MeshCore), None);
        assert!(core.radio_contact_meshes(&group_id).is_empty());
    }
    close_all(cores).await;

    // As MDK has it on each phone: the group has no radio component of anyone's and no event of
    // the app's kinds, and the direct chats have their records.
    for (member, phone) in phones.iter().enumerate() {
        let runtime = mdk(phone, &url).await;
        let device = &accounts[member];
        let members = runtime
            .group_members(device, &group(&group_id))
            .await
            .unwrap();
        assert_eq!(members.len(), 3);
        for (&(a, b), (chat_id, (of_a, of_b))) in pairs.iter().zip(chats.iter().zip(&records)) {
            for (record, owner, peer) in [(of_a, a, b), (of_b, b, a)] {
                let id = radio_component_id(&accounts[owner], &accounts[peer]);
                assert_eq!(raw_component(&runtime, device, &group_id, id).await, None);
                if member == a || member == b {
                    assert_eq!(
                        component(&runtime, device, chat_id, &accounts[owner], &accounts[peer])
                            .await,
                        vec![serde_json::to_value(record).unwrap()]
                    );
                }
            }
        }
        let rows = runtime
            .timeline_messages_with_query(
                device,
                TimelineMessageQuery {
                    group_id_hex: Some(group_id.clone()),
                    ..TimelineMessageQuery::default()
                },
            )
            .unwrap();
        assert!(
            rows.messages
                .iter()
                .all(|row| row.kind != 9_911 && row.kind != 9_912)
        );
        runtime.shutdown_and_close().await.unwrap();
    }

    // Another app of Ada's writes a radio component into the group, and it reaches the others.
    let key = new_radio_key();
    let info = radios[0].info().await.unwrap();
    let injected = RadioRecord::new(&info, &key, 100, now_ms());
    let data = format!("[{}]", injected.to_json()).into_bytes();
    let ada = mdk(&phones[0], &url).await;
    for peer in [1, 2] {
        write_component(&ada, &accounts[0], &group_id, &accounts[peer], data.clone()).await;
    }
    for member in [1, 2] {
        let runtime = mdk(&phones[member], &url).await;
        let id = radio_component_id(&accounts[0], &accounts[member]);
        let deadline = Instant::now() + WAIT;
        while raw_component(&runtime, &accounts[member], &group_id, id)
            .await
            .as_ref()
            != Some(&data)
        {
            assert!(Instant::now() < deadline, "the component didn't arrive");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        runtime.shutdown_and_close().await.unwrap();
    }
    ada.shutdown_and_close().await.unwrap();

    // No app takes a record or a key from it: the direct chats' records are those from before.
    let cores = start_all(&phones, &radios).await;
    for core in &cores {
        core.refresh_radio_records().await;
        assert_eq!(core.radio_own_record(&group_id, RadioMesh::MeshCore), None);
        assert!(core.radio_contact_meshes(&group_id).is_empty());
    }
    for (&(a, b), (chat_id, (of_a, of_b))) in pairs.iter().zip(chats.iter().zip(&records)) {
        for (to, record) in [(a, of_b), (b, of_a)] {
            assert_eq!(
                cores[to]
                    .radio_contact_record(chat_id, RadioMesh::MeshCore)
                    .as_ref(),
                Some(record)
            );
            assert_eq!(cores[to].radio_contact_meshes(chat_id), vec!["meshcore"]);
        }
    }
    // A packet under the key written into the group shows nowhere.
    for member in [1, 2] {
        inject(
            &radios[0],
            &radios[member],
            &key,
            injected.rkn,
            900,
            now_ms(),
            "Under the group's key",
        )
        .await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    for core in &cores {
        for chat in core.snapshot().chats {
            assert!(chat.messages.is_empty(), "{:?}", chat.messages);
        }
    }

    close_all(cores).await;
}

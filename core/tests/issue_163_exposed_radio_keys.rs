//! Radio records of a direct chat that someone else gets into (#163). Ada and Bea have a radio
//! each and a direct chat with both records. Another app of Bea's, MDK driven directly, adds Cai
//! and leaves the records where they are.

mod support;

use std::sync::Arc;
use std::time::Duration;

use cgka_traits::{GroupId, TransportEndpoint};
use marmot_account::AccountHome;
use marmot_app::{AppGroupLifecycleState, MarmotApp, MarmotAppConfig, MarmotAppRuntime};
use peoples_chat_core::Core;
use peoples_chat_core::interface::ChatState;
use peoples_chat_core::radio::{
    Mesh as RadioMesh, MockMesh, MockRadio, Radio, RadioKey, RadioRecord, decode_component,
    encode_component, format, new_radio_key, radio_component_id,
};
use support::{Phone, TestRelay, WAIT, make_contacts, now_ms, until};
use tokio::time::Instant;

fn account(core: &Core) -> String {
    core.snapshot().account.unwrap().public_key
}

fn state_of(core: &Core, chat_id: &str) -> Option<ChatState> {
    core.snapshot()
        .chats
        .into_iter()
        .find(|chat| chat.id == chat_id)
        .map(|chat| chat.state)
}

fn texts(core: &Core, chat_id: &str) -> Vec<String> {
    core.snapshot()
        .chats
        .into_iter()
        .find(|chat| chat.id == chat_id)
        .map(|chat| chat.messages.into_iter().map(|m| m.text).collect())
        .unwrap_or_default()
}

fn own(core: &Core, chat_id: &str) -> RadioRecord {
    core.radio_own_record(chat_id, RadioMesh::MeshCore).unwrap()
}

/// Waits until `to` has the record that `from` has now.
async fn has_record_of(to: &Core, from: &Core, chat_id: &str) -> RadioRecord {
    until("the contact's radio record", || {
        let own = from.radio_own_record(chat_id, RadioMesh::MeshCore)?;
        (to.radio_contact_record(chat_id, RadioMesh::MeshCore)? == own).then_some(own)
    })
    .await
}

/// MDK on a phone whose core is closed: another app.
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

/// Waits until MDK has no change to the chat that is still on its way.
async fn stable(runtime: &MarmotAppRuntime, device: &str, chat_id: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        let state = runtime.group_mls_state(device, &group(chat_id)).await;
        if state.is_ok_and(|state| state.lifecycle_state == AppGroupLifecycleState::Stable) {
            return;
        }
        assert!(Instant::now() < deadline, "the chat didn't settle");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Waits until `device` has `wanted` records in the component of `owner`, and returns them.
async fn component(
    runtime: &MarmotAppRuntime,
    device: &str,
    chat_id: &str,
    (owner, peer): (&str, &str),
    wanted: impl Fn(&[RadioRecord]) -> bool,
) -> Vec<RadioRecord> {
    let deadline = Instant::now() + WAIT;
    loop {
        let data = runtime
            .group_app_component(device, &group(chat_id), radio_component_id(owner, peer))
            .await
            .unwrap()
            .unwrap_or_default();
        let records: Vec<RadioRecord> = decode_component(&data)
            .iter()
            .filter_map(RadioRecord::from_value)
            .collect();
        if wanted(&records) {
            return records;
        }
        assert!(
            Instant::now() < deadline,
            "the component didn't change: {} records",
            records.len()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Hands the radio `to` a packet sealed with `record`'s key, from the radio `from`.
async fn inject(from: &MockRadio, to: &MockRadio, record: &RadioRecord, counter: u64, text: &str) {
    let key: RadioKey = record.radio_key().unwrap();
    let sent_at = u32::try_from(now_ms() / 1000).unwrap();
    let packet = format::seal_text(&key, record.rkn, counter, sent_at, text);
    from.send_text(to.public_key(), &format::to_text_message(&packet))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_163_someone_added_by_another_app_gets_no_key_that_is_used_again() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let [ada_phone, bea_phone, cai_phone] =
        ["ada", "bea", "cai"].map(|name| Phone::new(&format!("163-{name}"), &url));

    let cai = cai_phone.start().await;
    cai.create_account("Cai Berg").await.unwrap();
    cai.wait_until_published(WAIT).await.unwrap();
    let cai_account = account(&cai);
    cai.close().await.unwrap();

    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    ada.create_account("Ada Lund").await.unwrap();
    bea.create_account("Bea Holm").await.unwrap();
    let (ada_account, bea_account) = (account(&ada), account(&bea));
    let first = make_contacts(&ada, &bea).await;
    until("Bea's chat", || state_of(&bea, &first)).await;

    // A second direct chat of the two, as another app can make one.
    ada.close().await.unwrap();
    bea.close().await.unwrap();
    let ada_app = mdk(&ada_phone, &url).await;
    let second = ada_app
        .create_group(&ada_account, "", std::slice::from_ref(&bea_account), None)
        .await
        .unwrap()
        .operation
        .expect("the chat was made")
        .to_string();
    let bea_app = mdk(&bea_phone, &url).await;
    let deadline = Instant::now() + WAIT;
    while let Err(error) = bea_app
        .accept_group_invite(&bea_account, &group(&second))
        .await
    {
        assert!(Instant::now() < deadline, "the chat wasn't joined: {error}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    bea_app.shutdown_and_close().await.unwrap();
    ada_app.shutdown_and_close().await.unwrap();

    // With radios, the first chat gets both records, and the second one Ada's.
    let mesh = MockMesh::new();
    let ada_radio = Arc::new(mesh.radio("Ada's radio"));
    let bea_radio = Arc::new(mesh.radio("Bea's radio"));
    // Sends packets under whatever key a test says. Ada doesn't know it, so she tries them all.
    let injector = mesh.radio("A third radio");
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    ada.attach_radio(ada_radio.clone()).await.unwrap();
    bea.attach_radio(bea_radio.clone()).await.unwrap();
    until("the second chat", || state_of(&ada, &second)).await;
    let bea_old = has_record_of(&ada, &bea, &first).await;
    let ada_old = has_record_of(&bea, &ada, &first).await;
    let ada_old_second = has_record_of(&bea, &ada, &second).await;
    inject(&injector, &ada_radio, &bea_old, 100, "While direct").await;
    until("the packet in the direct chat", || {
        texts(&ada, &first)
            .contains(&"While direct".to_owned())
            .then_some(())
    })
    .await;

    // Bea's other app adds Cai, and clears nothing.
    bea.close().await.unwrap();
    let bea_app = mdk(&bea_phone, &url).await;
    stable(&bea_app, &bea_account, &first).await;
    bea_app
        .invite_members(
            &bea_account,
            &group(&first),
            std::slice::from_ref(&cai_account),
        )
        .await
        .unwrap();

    // Ada's app empties its component, leaving Bea's, which only Bea's apps write.
    let of_ada = (ada_account.as_str(), bea_account.as_str());
    let of_bea = (bea_account.as_str(), ada_account.as_str());
    component(&bea_app, &bea_account, &first, of_ada, |records| {
        records.is_empty()
    })
    .await;
    component(&bea_app, &bea_account, &first, of_bea, |records| {
        records == std::slice::from_ref(&bea_old)
    })
    .await;
    // It has new keys for both chats, and the second chat's state gets its own.
    assert_eq!(state_of(&ada, &first), Some(ChatState::Group));
    let ada_new = own(&ada, &first);
    assert_ne!(ada_new.rk, ada_old.rk);
    assert_eq!(ada_new.rkn, ada_old.rkn + 1);
    let ada_new_second = until("a new key in the second chat", || {
        Some(own(&ada, &second)).filter(|record| record.rk != ada_old_second.rk)
    })
    .await;
    component(&bea_app, &bea_account, &second, of_ada, |records| {
        records == std::slice::from_ref(&ada_new_second)
    })
    .await;
    assert_eq!(ada.radio_contact_record(&first, RadioMesh::MeshCore), None);
    inject(&injector, &ada_radio, &bea_old, 101, "With Cai there").await;

    // Cai goes again, so the chat is direct once more. Bea's exposed key is still in its state,
    // and is not taken back.
    stable(&bea_app, &bea_account, &first).await;
    bea_app
        .remove_members(
            &bea_account,
            &group(&first),
            std::slice::from_ref(&cai_account),
        )
        .await
        .unwrap();
    until("the chat to be direct again", || {
        (state_of(&ada, &first) == Some(ChatState::Direct)).then_some(())
    })
    .await;
    let back = component(&bea_app, &bea_account, &first, of_ada, |records| {
        !records.is_empty()
    })
    .await;
    assert_ne!(back[0].rk, ada_old.rk);
    assert_ne!(back[0].rk, ada_new_second.rk);
    // The record is read again, for the radio's address, and its key opens nothing.
    until("Bea's record, still in the state", || {
        (ada.radio_contact_record(&first, RadioMesh::MeshCore)? == bea_old).then_some(())
    })
    .await;
    inject(&injector, &ada_radio, &bea_old, 102, "Under the old key").await;

    // A new key of Bea's is taken, and a packet under it is shown.
    let bea_new = RadioRecord::new(
        &bea_radio.info().await.unwrap(),
        &new_radio_key(),
        bea_old.rkn + 1,
        now_ms(),
    );
    stable(&bea_app, &bea_account, &first).await;
    bea_app
        .update_app_component(
            &bea_account,
            &group(&first),
            radio_component_id(&bea_account, &ada_account),
            encode_component([&bea_new]),
        )
        .await
        .unwrap();
    until("Bea's new record", || {
        (ada.radio_contact_record(&first, RadioMesh::MeshCore)? == bea_new).then_some(())
    })
    .await;
    inject(&injector, &ada_radio, &bea_old, 103, "Still the old key").await;
    inject(&injector, &ada_radio, &bea_new, 0, "Under the new key").await;
    until("the packet under the new key", || {
        texts(&ada, &first)
            .contains(&"Under the new key".to_owned())
            .then_some(())
    })
    .await;
    // The radio hands packets over in order, so the earlier ones have been dropped by now.
    for refused in ["With Cai there", "Under the old key", "Still the old key"] {
        for chat_id in [&first, &second] {
            assert!(
                !texts(&ada, chat_id).contains(&refused.to_owned()),
                "{refused}"
            );
        }
    }

    bea_app.shutdown_and_close().await.unwrap();
    ada.close().await.unwrap();
}

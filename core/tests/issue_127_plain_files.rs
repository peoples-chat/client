//! Message text stays out of plain files (#127). Ada and Bea chat by radio with the relay
//! blocked: Bea's radio store keeps the text she received encrypted, Ada's keeps none of what she
//! sent, and a message waiting for a path is encrypted too. Once the relay copy arrives, Bea's
//! store drops the text and keeps the id, so a replayed packet still shows nothing. A deletion
//! then leaves no copy to bring the message back. In a chat whose messages disappear, a radio
//! message without its relay copy goes from the screen and the disk by itself.
//!
//! The deletion and the disappearing messages setting come from MDK driven directly on Ada's
//! data folder, as another Marmot app would: peoples.chat offers neither yet.

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use cgka_traits::GroupId;
use marmot_account::AccountHome;
use marmot_app::{MarmotApp, MarmotAppConfig, MarmotAppRuntime};
use peoples_chat_core::Core;
use peoples_chat_core::interface::{Mesh, Message, Reach, Transport};
use peoples_chat_core::radio::{MockMesh, MockRadio, Radio, RadioEvent};
use support::{Phone, TestRelay, WAIT, make_contacts, until};
use tokio::time::Instant;

const BY_RADIO: &str = "The key is under the mat";
const WAITING: &str =
    "This one is too long for a single radio packet, so it waits for the internet to come back to us.";

fn messages(core: &Core, chat_id: &str) -> Vec<Message> {
    core.snapshot()
        .chats
        .into_iter()
        .find(|chat| chat.id == chat_id)
        .map(|chat| chat.messages)
        .unwrap_or_default()
}

fn has(core: &Core, chat_id: &str, id: &str) -> bool {
    messages(core, chat_id).iter().any(|m| m.id == id)
}

/// The radio store's entry for the message `id`.
fn entry(data: &Path, id: &str) -> serde_json::Value {
    let json = std::fs::read(data.join("radio.json")).unwrap();
    let store: serde_json::Value = serde_json::from_slice(&json).unwrap();
    store["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["id"] == id)
        .cloned()
        .unwrap_or_else(|| panic!("no entry for {id}"))
}

/// No file the core writes itself holds any of `texts`, and only the owner reads them.
fn assert_no_plain_text(data: &Path, texts: &[&str]) {
    for name in ["radio.json", "outbox.json"] {
        let path = data.join(name);
        let Ok(file) = std::fs::read_to_string(&path) else {
            continue;
        };
        for text in texts {
            assert!(!file.contains(text), "{name} holds {text:?}: {file}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{name}");
        }
    }
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

async fn settled(one: &Core, other: &Core, chat_id: &str) -> u64 {
    let deadline = Instant::now() + WAIT;
    loop {
        if let (Ok(a), Ok(b)) = (
            one.radio_keys(chat_id).await,
            other.radio_keys(chat_id).await,
        ) && a.epoch == b.epoch
        {
            return a.epoch;
        }
        assert!(Instant::now() < deadline, "the chat didn't settle");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

struct Pair {
    relay: TestRelay,
    ada_phone: Phone,
    bea_phone: Phone,
    ada: Core,
    bea: Core,
    chat_id: String,
    mesh: MockMesh,
    ada_radio: Arc<MockRadio>,
    bea_radio: Arc<MockRadio>,
}

/// Ada and Bea, contacts, each with a radio the other knows, and Ada forced to radio.
async fn pair(name: &str, keys_in_file: bool) -> Pair {
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new(&format!("{name}-ada"), &url).with_keys_in_file(keys_in_file);
    let bea_phone = Phone::new(&format!("{name}-bea"), &url).with_keys_in_file(keys_in_file);
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
    for core in [&ada, &bea] {
        until("the contact's radio", || {
            core.snapshot()
                .chats
                .iter()
                .any(|chat| chat.contact.meshes == vec![Mesh::MeshCore])
                .then_some(())
        })
        .await;
    }
    ada.set_force_radio(true).unwrap();
    Pair {
        relay,
        ada_phone,
        bea_phone,
        ada,
        bea,
        chat_id,
        mesh,
        ada_radio,
        bea_radio,
    }
}

/// MDK on the data folder of `phone`, whose core must be closed.
async fn mdk(phone: &Phone, relay: &str) -> (MarmotApp, MarmotAppRuntime, String) {
    let home =
        AccountHome::open_with_keychain(phone.data_dir(), phone.keychain_service()).unwrap();
    let account = home.accounts().unwrap().remove(0).account_id_hex;
    let app = MarmotApp::try_with_relays_and_account_home_and_config(
        phone.data_dir(),
        vec![relay.to_owned()],
        home,
        MarmotAppConfig::default().with_allow_loopback_relay_endpoints(true),
    )
    .unwrap();
    let runtime = app.runtime();
    runtime.start().await.unwrap();
    (app, runtime, account)
}

fn group(chat_id: &str) -> GroupId {
    GroupId::new(hex::decode(chat_id).unwrap())
}

/// A restart reads the files back: run with the keys in the keychain, and in a file as
/// development builds keep them.
async fn text_stays_encrypted_until_the_relay_copy_arrives(keys_in_file: bool) {
    let name = if keys_in_file {
        "issue-127-file"
    } else {
        "issue-127-keychain"
    };
    let Pair {
        relay,
        ada_phone,
        bea_phone,
        ada,
        bea,
        chat_id,
        mesh,
        ada_radio: _,
        bea_radio,
    } = pair(name, keys_in_file).await;
    relay.block();
    reach_becomes(&ada, &chat_id, Reach::Radio).await;

    // By radio, and one too long for it, which waits.
    let mut air = bea_radio.events();
    let sent = ada.send_message(&chat_id, BY_RADIO).await.unwrap();
    assert_eq!(sent.transport, Transport::Radio);
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
    until("Bea has it", || {
        has(&bea, &chat_id, &sent.id).then_some(())
    })
    .await;
    ada.set_force_radio(false).unwrap();
    let waiting = ada.send_message(&chat_id, WAITING).await.unwrap();
    assert!(waiting.cancellable);

    let texts = [BY_RADIO, WAITING];
    assert_no_plain_text(ada_phone.data_dir(), &texts);
    assert_no_plain_text(bea_phone.data_dir(), &texts);
    let received = entry(bea_phone.data_dir(), &sent.id);
    assert!(received["sealed"].is_string(), "{received}");
    // What was sent is in MDK already.
    let own = entry(ada_phone.data_dir(), &sent.id);
    assert!(own.get("sealed").is_none() && own.get("text").is_none(), "{own}");

    // Both restart with the relay still blocked: the texts are read back.
    ada.close().await.unwrap();
    bea.close().await.unwrap();
    let bea = bea_phone.start().await;
    let ada = ada_phone.start().await;
    for (core, text) in [(&bea, BY_RADIO), (&ada, BY_RADIO), (&ada, WAITING)] {
        until("the message after a restart", || {
            messages(core, &chat_id)
                .iter()
                .any(|m| m.text == text)
                .then_some(())
        })
        .await;
    }

    // The relay copy arrives: Bea's store drops the text, and keeps the id and chat.
    relay.unblock();
    until("Bea's store to drop the text", || {
        entry(bea_phone.data_dir(), &sent.id)
            .get("sealed")
            .is_none()
            .then_some(())
    })
    .await;
    let dropped = entry(bea_phone.data_dir(), &sent.id);
    assert_eq!(dropped["chat_id"], chat_id.as_str());
    assert!(dropped.get("text").is_none(), "{dropped}");
    until("the waiting message arrives", || {
        messages(&bea, &chat_id)
            .iter()
            .any(|m| m.text == WAITING)
            .then_some(())
    })
    .await;
    assert_no_plain_text(ada_phone.data_dir(), &texts);
    assert_no_plain_text(bea_phone.data_dir(), &texts);

    // The packet again shows nothing, and the history comes from MDK after a restart.
    bea.attach_radio(bea_radio.clone()).await.unwrap();
    mesh.radio("Eve's radio")
        .send_text(bea_radio.public_key(), &captured)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(messages(&bea, &chat_id).len(), 2);
    bea.close().await.unwrap();
    let bea = bea_phone.start().await;
    let kept = until("Bea's history", || {
        let kept = messages(&bea, &chat_id);
        (kept.len() == 2).then_some(kept)
    })
    .await;
    let radio = kept.iter().find(|m| m.id == sent.id).unwrap();
    assert_eq!(
        (radio.text.as_str(), radio.transport),
        (BY_RADIO, Transport::Radio)
    );

    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_127_text_is_encrypted_until_the_relay_copy_arrives() {
    let _turn = support::take_turn().await;
    text_stays_encrypted_until_the_relay_copy_arrives(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_127_the_same_with_keys_in_a_file() {
    let _turn = support::take_turn().await;
    text_stays_encrypted_until_the_relay_copy_arrives(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_127_a_deleted_message_does_not_come_back_from_the_radio_store() {
    let _turn = support::take_turn().await;
    let Pair {
        relay,
        ada_phone,
        bea_phone,
        ada,
        bea,
        chat_id,
        ..
    } = pair("issue-127-delete", false).await;
    relay.block();
    reach_becomes(&ada, &chat_id, Reach::Radio).await;
    let sent = ada.send_message(&chat_id, BY_RADIO).await.unwrap();
    until("Bea has it", || {
        has(&bea, &chat_id, &sent.id).then_some(())
    })
    .await;

    // The relay copy arrives, then Ada deletes the message for everyone.
    relay.unblock();
    until("Bea's store to drop the text", || {
        entry(bea_phone.data_dir(), &sent.id)
            .get("sealed")
            .is_none()
            .then_some(())
    })
    .await;
    ada.close().await.unwrap();
    let (_app, runtime, account) = mdk(&ada_phone, &relay.url()).await;
    runtime
        .delete_message(&account, &group(&chat_id), &sent.id)
        .await
        .unwrap();

    // Bea's history comes from MDK alone, so once MDK has the deletion the message is gone.
    let deadline = Instant::now() + WAIT;
    let mut bea = bea;
    loop {
        bea.close().await.unwrap();
        bea = bea_phone.start().await;
        until("Bea's chat", || {
            (!bea.snapshot().chats.is_empty()).then_some(())
        })
        .await;
        if !has(&bea, &chat_id, &sent.id) {
            break;
        }
        assert!(Instant::now() < deadline, "the deleted message stayed");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    assert_no_plain_text(bea_phone.data_dir(), &[BY_RADIO]);
    assert!(entry(bea_phone.data_dir(), &sent.id).get("sealed").is_none());

    runtime.shutdown_and_close().await.unwrap();
    bea.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_127_a_radio_message_disappears_when_its_time_is_up() {
    const SECS: u64 = 15;
    let _turn = support::take_turn().await;
    let Pair {
        relay,
        ada_phone,
        bea_phone,
        ada,
        bea,
        chat_id,
        ada_radio,
        ..
    } = pair("issue-127-expiry", false).await;

    // Ada turns disappearing messages on, as another Marmot app would.
    let before = settled(&ada, &bea, &chat_id).await;
    ada.close().await.unwrap();
    let (app, runtime, account) = mdk(&ada_phone, &relay.url()).await;
    runtime
        .update_message_retention(&account, &group(&chat_id), SECS)
        .await
        .unwrap();
    let deadline = Instant::now() + WAIT;
    while !bea
        .radio_keys(&chat_id)
        .await
        .is_ok_and(|keys| keys.epoch > before)
    {
        assert!(Instant::now() < deadline, "Bea didn't get the setting");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    runtime.shutdown_and_close().await.unwrap();
    drop((app, runtime));
    let ada = ada_phone.start().await;
    until("Ada's chat", || {
        (!ada.snapshot().chats.is_empty()).then_some(())
    })
    .await;
    settled(&ada, &bea, &chat_id).await;
    ada.attach_radio(ada_radio.clone()).await.unwrap();
    ada.set_force_radio(true).unwrap();

    relay.block();
    reach_becomes(&ada, &chat_id, Reach::Radio).await;
    let sent = ada.send_message(&chat_id, BY_RADIO).await.unwrap();
    until("Bea has it", || {
        has(&bea, &chat_id, &sent.id).then_some(())
    })
    .await;
    assert!(entry(bea_phone.data_dir(), &sent.id)["sealed"].is_string());

    // Its time passes without the relay copy.
    until("the message to disappear", || {
        (!has(&bea, &chat_id, &sent.id)).then_some(())
    })
    .await;
    assert!(entry(bea_phone.data_dir(), &sent.id).get("sealed").is_none());
    bea.close().await.unwrap();
    let bea = bea_phone.start().await;
    until("Bea's chat", || {
        (!bea.snapshot().chats.is_empty()).then_some(())
    })
    .await;
    assert!(!has(&bea, &chat_id, &sent.id));

    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

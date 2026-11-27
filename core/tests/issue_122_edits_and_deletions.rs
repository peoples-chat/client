//! Deletions and edits as they happen (#122). The other person's app is MDK itself, driven
//! directly as another Marmot app such as White Noise would: peoples.chat can't edit or delete
//! yet. Their edit and their deletion change the message in this core at once, without a
//! restart, and a restart shows the same. A deleted radio message's text goes from the radio
//! store too.
//!
//! A withdrawn message comes only from MDK's convergence after two versions of a chat's state
//! raced; `core/src/app.rs` tests how the core takes it.

mod support;

use std::path::Path;
use std::time::Duration;

use cgka_traits::{GroupId, TransportEndpoint};
use marmot_account::AccountHome;
use marmot_app::{
    AccountSetupReadiness, AccountSetupRequest, MarmotApp, MarmotAppConfig, MarmotAppRuntime,
    UserProfileMetadata,
};
use peoples_chat_core::Core;
use peoples_chat_core::interface::{CoreEvent, Message};
use support::{Phone, TestRelay, WAIT, next_event, next_invitation, until};
use tempfile::TempDir;
use tokio::sync::broadcast;

/// The other person: MDK in a folder of its own, with one account.
struct OtherApp {
    _data: TempDir,
    _app: MarmotApp,
    runtime: MarmotAppRuntime,
    account: String,
}

impl OtherApp {
    async fn start(relay: &str, name: &str) -> Self {
        support::use_mock_keychain();
        let data = tempfile::tempdir().unwrap();
        let home =
            AccountHome::open_with_keychain(data.path(), "chat.peoples.client.test-issue-122-wren")
                .unwrap();
        let app = MarmotApp::try_with_relays_and_account_home_and_config(
            data.path(),
            vec![relay.to_owned()],
            home,
            MarmotAppConfig::default().with_allow_loopback_relay_endpoints(true),
        )
        .unwrap();
        let runtime = app.runtime();
        runtime.start().await.unwrap();
        let relays = vec![TransportEndpoint(relay.to_owned())];
        let created = runtime
            .create_identity_local_ready_with_initial_profile(
                AccountSetupRequest {
                    default_relays: relays.clone(),
                    bootstrap_relays: relays,
                    publish_missing_relay_lists: true,
                    publish_initial_key_package: true,
                    ..Default::default()
                },
                UserProfileMetadata {
                    name: Some(name.to_owned()),
                    display_name: Some(name.to_owned()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let account = created.account.account_id_hex;
        until("the other app's account on the relay", || {
            matches!(
                runtime.account_setup_readiness(&account),
                Ok(AccountSetupReadiness::NetworkReady)
            )
            .then_some(())
        })
        .await;
        Self {
            _data: data,
            _app: app,
            runtime,
            account,
        }
    }

    /// Sends `text` in the chat, and returns the message's id.
    async fn send(&self, group: &GroupId, text: &str) -> String {
        let token = format!("issue-122-{}", support::now_ms());
        self.runtime
            .submit_text(&self.account, group, text.to_owned(), token)
            .await
            .unwrap()
            .message_id_hex
    }

    async fn close(self) {
        self.runtime.shutdown_and_close().await.unwrap();
    }
}

/// The next event that adds or changes a message in `chat_id`, and the message.
async fn next_message(events: &mut broadcast::Receiver<CoreEvent>, chat_id: &str) -> CoreEvent {
    loop {
        match next_event(events).await {
            event @ (CoreEvent::MessageAdded { .. } | CoreEvent::MessageChanged { .. }) => {
                let (CoreEvent::MessageAdded { chat_id: id, .. }
                | CoreEvent::MessageChanged { chat_id: id, .. }) = &event
                else {
                    unreachable!()
                };
                assert_eq!(id, chat_id);
                return event;
            }
            // Accepting the invitation, and the contact's name and photo, may come meanwhile.
            CoreEvent::InvitationRemoved { .. }
            | CoreEvent::ChatAdded { .. }
            | CoreEvent::ContactChanged { .. }
            | CoreEvent::ReachChanged { .. } => {}
            other => panic!("expected a message, got {other:?}"),
        }
    }
}

fn changed(event: CoreEvent) -> Message {
    match event {
        CoreEvent::MessageChanged { message, .. } => message,
        other => panic!("expected messageChanged, got {other:?}"),
    }
}

fn shown(core: &Core, chat_id: &str, id: &str) -> Option<Message> {
    core.snapshot()
        .chats
        .into_iter()
        .find(|chat| chat.id == chat_id)?
        .messages
        .into_iter()
        .find(|message| message.id == id)
}

/// Writes a copy of the message into the radio store in `data`, as if it had come by radio
/// (#40). The core is closed meanwhile.
fn keep_as_radio(data: &Path, message: &Message, chat_id: &str) {
    let path = data.join("radio.json");
    let mut store: serde_json::Value = std::fs::read(&path)
        .map(|json| serde_json::from_slice(&json).unwrap())
        .unwrap_or_else(|_| serde_json::json!({}));
    let kept = serde_json::json!({
        "chat_id": chat_id,
        "id": message.id,
        "outgoing": false,
        "text": message.text,
        "sent_at": message.sent_at,
        "kept_at": message.sent_at,
    });
    match store["messages"].as_array_mut() {
        Some(messages) => messages.push(kept),
        None => store["messages"] = serde_json::json!([kept]),
    }
    std::fs::write(&path, serde_json::to_vec(&store).unwrap()).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_122_edits_and_deletions_show_without_a_restart() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new("issue-122-ada", &url);
    let ada = ada_phone.start().await;
    let ada_account = ada.create_account("Ada Lund").await.unwrap();
    ada.wait_until_published(WAIT).await.unwrap();
    let wren = OtherApp::start(&url, "Wren Test").await;

    // Wren starts a chat with Ada, and Ada accepts.
    let mut events = ada.subscribe();
    let group = wren
        .runtime
        .start_direct_conversation(&wren.account, &ada_account.public_key)
        .await
        .unwrap();
    let invitation = next_invitation(&mut events).await;
    let chat_id = invitation.id.clone();
    assert_eq!(chat_id, group.to_string());
    ada.accept_invitation(&chat_id).await.unwrap();

    // Wren writes, then edits.
    let id = wren.send(&group, "Meet at the old mill").await;
    let CoreEvent::MessageAdded { message, .. } = next_message(&mut events, &chat_id).await else {
        panic!("expected the message first");
    };
    assert_eq!(
        (message.id.as_str(), message.text.as_str()),
        (id.as_str(), "Meet at the old mill")
    );
    assert!(!message.edited && !message.deleted);

    wren.runtime
        .edit_message(&wren.account, &group, &id, "Meet at the new mill")
        .await
        .unwrap();
    let edited = changed(next_message(&mut events, &chat_id).await);
    assert_eq!(edited.id, id);
    assert_eq!(edited.text, "Meet at the new mill");
    assert!(edited.edited && !edited.deleted && !edited.withdrawn);
    assert_eq!(edited.sent_at, message.sent_at, "an edit keeps its place");
    assert_eq!(shown(&ada, &chat_id, &id), Some(edited.clone()));

    // Another message, which Wren deletes for everyone.
    let gone = wren.send(&group, "My door code is 4711").await;
    let CoreEvent::MessageAdded {
        message: second, ..
    } = next_message(&mut events, &chat_id).await
    else {
        panic!("expected the second message");
    };
    assert_eq!(second.id, gone);
    wren.runtime
        .delete_message(&wren.account, &group, &gone)
        .await
        .unwrap();
    let deleted = changed(next_message(&mut events, &chat_id).await);
    assert_eq!(deleted.id, gone);
    assert!(deleted.deleted && !deleted.edited);
    assert_eq!(deleted.text, "");
    assert_eq!(shown(&ada, &chat_id, &gone), Some(deleted.clone()));

    // A third, which came by radio too: its deletion takes the text from the radio store.
    let radio_id = wren.send(&group, "Spare key under the mat").await;
    let CoreEvent::MessageAdded { message: third, .. } = next_message(&mut events, &chat_id).await
    else {
        panic!("expected the third message");
    };
    ada.close().await.unwrap();
    keep_as_radio(ada_phone.data_dir(), &third, &chat_id);
    let ada = ada_phone.start().await;
    let mut events = ada.subscribe();
    assert_eq!(
        shown(&ada, &chat_id, &radio_id).map(|m| m.text),
        Some("Spare key under the mat".to_owned())
    );
    wren.runtime
        .delete_message(&wren.account, &group, &radio_id)
        .await
        .unwrap();
    let deleted_radio = loop {
        let message = changed(next_message(&mut events, &chat_id).await);
        if message.id == radio_id {
            break message;
        }
    };
    assert!(deleted_radio.deleted);
    assert_eq!(deleted_radio.text, "");
    let store = std::fs::read_to_string(ada_phone.data_dir().join("radio.json")).unwrap();
    assert!(!store.contains("Spare key"), "{store}");

    // A restart shows the same: the edit, and the deleted messages as deleted.
    ada.close().await.unwrap();
    let ada = ada_phone.start().await;
    let after = until("the chat after a restart", || {
        let chat = ada.snapshot().chats.into_iter().find(|c| c.id == chat_id)?;
        (chat.messages.len() >= 3).then_some(chat.messages)
    })
    .await;
    let by_id = |id: &str| after.iter().find(|m| m.id == id).cloned().unwrap();
    assert_eq!(by_id(&id).text, "Meet at the new mill");
    assert!(by_id(&id).edited);
    assert!(by_id(&gone).deleted && by_id(&gone).text.is_empty());
    assert!(by_id(&radio_id).deleted && by_id(&radio_id).text.is_empty());

    ada.close().await.unwrap();
    wren.close().await;
    // Lets the relay's last answers go before it stops.
    tokio::time::sleep(Duration::from_millis(50)).await;
}

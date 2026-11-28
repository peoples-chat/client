//! A chat that stops being a two-person chat (#125): when the contact deletes their account,
//! the other core keeps the chat, says that the contact left, stops sending there, and still
//! has the chat after a restart.

mod support;

use peoples_chat_core::Messaging;
use peoples_chat_core::interface::{ChatState, CoreEvent, DeliveryState, Sender, Transport};
use support::{Phone, TestRelay, WAIT, make_contacts, until};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_125_the_chat_says_the_contact_left_after_they_delete_their_account() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new("left-ada", &url);
    let bea_phone = Phone::new("left-bea", &url);
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    ada.create_account("Ada Lund").await.unwrap();
    bea.create_account("Bea Holm").await.unwrap();
    let chat_id = make_contacts(&ada, &bea).await;

    // They talk, as a direct chat.
    let hello = ada.send_message(&chat_id, "Hello Bea").await.unwrap();
    until("Bea to get Ada's message", || {
        bea.snapshot()
            .chats
            .iter()
            .any(|chat| chat.messages.iter().any(|m| m.id == hello.id))
            .then_some(())
    })
    .await;
    let chat = ada.snapshot().chats.into_iter().next().unwrap();
    assert_eq!(chat.state, ChatState::Direct);
    let mut ada_events = ada.subscribe();

    // Bea deletes her account: her app leaves the chat, and Ada's commits it.
    bea.delete_account().await.unwrap();
    bea.close().await.unwrap();

    let changed = tokio::time::timeout(WAIT, async {
        loop {
            match ada_events.recv().await.unwrap() {
                CoreEvent::ChatChanged { chat } => return chat,
                _ => continue,
            }
        }
    })
    .await
    .expect("no chatChanged in time");
    assert_eq!(changed.id, chat_id);
    assert_eq!(changed.state, ChatState::ContactLeft);
    assert_eq!(changed.contact.public_key, chat.contact.public_key);
    assert_eq!(changed.contact.name, "Bea Holm");
    // The chat keeps its messages; in a direct chat that ended, they still need no sender.
    assert!(changed.messages.iter().any(|m| m.id == hello.id));
    assert!(changed.messages.iter().all(|m| m.sender.is_none()));
    assert_eq!(ada.snapshot().chats, vec![changed.clone()]);

    // Nothing more goes there, instead of showing "Sent" to nobody.
    let error = ada
        .send_message(&chat_id, "Are you there?")
        .await
        .unwrap_err();
    assert_eq!(
        error.message,
        "Bea Holm left this chat. Messages can't be sent here any more."
    );
    let messages = &ada.snapshot().chats[0].messages;
    assert!(!messages.iter().any(|m| m.text == "Are you there?"));
    assert!(
        messages
            .iter()
            .filter(|m| m.delivery.is_some())
            .all(|m| m.delivery == Some(DeliveryState::Sent)),
        "{messages:?}"
    );

    // After a restart the chat is still there, and still says so.
    ada.close().await.unwrap();
    let ada = ada_phone.start().await;
    let chat = until("the chat after a restart", || {
        ada.snapshot().chats.into_iter().find(|c| c.id == chat_id)
    })
    .await;
    assert_eq!(chat.state, ChatState::ContactLeft);
    assert_eq!(chat.contact.name, "Bea Holm");
    assert!(chat.messages.iter().any(|m| m.id == hello.id));
    let error = ada.send_message(&chat_id, "Hello?").await.unwrap_err();
    assert!(error.message.starts_with("Bea Holm left this chat."));
    ada.close().await.unwrap();
}

/// When the chat gets a name in another app, it is no longer direct: the other core says so,
/// shows who sent each incoming message, and still sends there, by internet.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_125_a_chat_that_gets_a_name_says_who_sent_each_message() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new("named-ada", &url);
    let bea_phone = Phone::new("named-bea", &url);
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    ada.create_account("Ada Lund").await.unwrap();
    bea.create_account("Bea Holm").await.unwrap();
    let bea_hex = bea.snapshot().account.unwrap().public_key;
    // Bea starts the chat, so she may rename it.
    let chat_id = make_contacts(&bea, &ada).await;

    let hello = bea.send_message(&chat_id, "Hello Ada").await.unwrap();
    let chat = until("Ada to get Bea's message", || {
        ada.snapshot()
            .chats
            .into_iter()
            .find(|chat| chat.messages.iter().any(|m| m.id == hello.id))
    })
    .await;
    assert_eq!(chat.state, ChatState::Direct);
    assert!(chat.messages.iter().all(|m| m.sender.is_none()));
    let mine = ada.send_message(&chat_id, "Hi Bea").await.unwrap();
    bea.close().await.unwrap();
    let mut ada_events = ada.subscribe();

    // In another app, Bea gives the chat a name.
    let bea = Messaging::open(support::options(
        bea_phone.data_dir(),
        &url,
        bea_phone.keychain_service(),
    ))
    .await
    .unwrap();
    bea.rename_chat(&bea_hex, &chat_id, "Book club")
        .await
        .unwrap();

    let changed = tokio::time::timeout(WAIT, async {
        loop {
            if let CoreEvent::ChatChanged { chat } = ada_events.recv().await.unwrap() {
                return chat;
            }
        }
    })
    .await
    .expect("no chatChanged in time");
    assert_eq!(changed.id, chat_id);
    assert_eq!(changed.state, ChatState::Group);
    assert_eq!(changed.contact.public_key, bea_hex);
    // Bea's message now says it is hers; Ada's own needs no sender.
    let sender = |id: &str| {
        changed
            .messages
            .iter()
            .find(|m| m.id == id)
            .unwrap_or_else(|| panic!("{id} is missing: {:?}", changed.messages))
            .sender
            .clone()
    };
    let bea_sender = Sender {
        public_key: bea_hex.clone(),
        name: "Bea Holm".to_owned(),
    };
    assert_eq!(sender(&hello.id), Some(bea_sender.clone()));
    assert_eq!(sender(&mine.id), None);

    // A new message from her arrives with her name on it.
    let later = bea
        .send_text(&bea_hex, &chat_id, "Welcome to the club")
        .await
        .unwrap();
    let added = tokio::time::timeout(WAIT, async {
        loop {
            if let CoreEvent::MessageAdded { message, .. } = ada_events.recv().await.unwrap()
                && message.id == later
            {
                return message;
            }
        }
    })
    .await
    .expect("no messageAdded in time");
    assert_eq!(added.sender, Some(bea_sender.clone()));

    // Ada can still write there, by internet only.
    let sent = ada.send_message(&chat_id, "Thanks").await.unwrap();
    assert_eq!(sent.transport, Transport::Internet);
    assert_eq!(sent.sender, None);

    // After a restart the chat is still a group, and still says who sent what.
    ada.close().await.unwrap();
    let ada = ada_phone.start().await;
    let chat = until("the chat after a restart", || {
        ada.snapshot().chats.into_iter().find(|c| c.id == chat_id)
    })
    .await;
    assert_eq!(chat.state, ChatState::Group);
    let restored = chat.messages.iter().find(|m| m.id == later).unwrap();
    assert_eq!(restored.sender, Some(bea_sender));
    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

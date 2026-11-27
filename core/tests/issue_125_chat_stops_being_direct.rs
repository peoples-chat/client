//! A chat that stops being a two-person chat (#125): when the contact deletes their account,
//! the other core keeps the chat, says that the contact left, stops sending there, and still
//! has the chat after a restart.

mod support;

use peoples_chat_core::interface::{ChatState, CoreEvent, DeliveryState};
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

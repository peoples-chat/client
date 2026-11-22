//! Text in a direct chat (#20): two contacts send each other messages over a relay, delivery
//! moves from waiting to sent, and both histories survive a restart with the same ids.

mod support;

use nostr_relay_builder::MockRelay;
use peoples_chat_core::Core;
use peoples_chat_core::interface::{CoreEvent, DeliveryState, Direction, Message};
use support::{Phone, make_contacts, next_event, next_invitation, until};
use tokio::sync::broadcast;

/// The next `messageAdded` event, which must be for `chat_id`.
async fn next_message(events: &mut broadcast::Receiver<CoreEvent>, chat_id: &str) -> Message {
    match next_event(events).await {
        CoreEvent::MessageAdded {
            chat_id: added_to,
            message,
        } if added_to == chat_id => message,
        other => panic!("expected a message, got {other:?}"),
    }
}

/// Waits until the user's own message `id` is `sent`, and returns the chat's messages.
async fn until_sent(core: &Core, chat_id: &str, id: &str) -> Vec<Message> {
    until("the relay's acknowledgement", || {
        let chat = core
            .snapshot()
            .chats
            .into_iter()
            .find(|c| c.id == chat_id)?;
        let sent = chat
            .messages
            .iter()
            .any(|m| m.id == id && m.delivery == Some(DeliveryState::Sent));
        sent.then_some(chat.messages)
    })
    .await
}

fn sorted(mut messages: Vec<Message>) -> Vec<Message> {
    // Messages within one second may come back in another order after a restart.
    messages.sort_by(|a, b| (a.sent_at, &a.id).cmp(&(b.sent_at, &b.id)));
    messages
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn contacts_exchange_text_and_keep_it_across_a_restart() {
    let _turn = support::take_turn().await;
    let relay = MockRelay::run().await.unwrap();
    let url = relay.url().await.to_string();
    let ada_phone = Phone::new("exchange-ada", &url);
    let bea_phone = Phone::new("exchange-bea", &url);
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    ada.create_account("Ada Lund").await.unwrap();
    bea.create_account("Bea Holm").await.unwrap();
    let chat_id = make_contacts(&ada, &bea).await;
    let mut ada_events = ada.subscribe();
    let mut bea_events = bea.subscribe();

    // Ada writes. The message is hers, waiting or already sent, and its text is trimmed.
    let hello = ada.send_message(&chat_id, "  Hello Bea  ").await.unwrap();
    assert_eq!(hello.direction, Direction::Out);
    assert_eq!(hello.text, "Hello Bea");
    assert!(hello.delivery.is_some());
    assert_eq!(next_message(&mut ada_events, &chat_id).await, hello);

    // A relay acknowledges it.
    if hello.delivery == Some(DeliveryState::Waiting) {
        assert_eq!(
            next_event(&mut ada_events).await,
            CoreEvent::DeliveryChanged {
                chat_id: chat_id.clone(),
                message_id: hello.id.clone(),
                delivery: DeliveryState::Sent,
            }
        );
    }

    // Bea receives it, under the same id and time.
    let received = next_message(&mut bea_events, &chat_id).await;
    assert_eq!(received.id, hello.id);
    assert_eq!(received.direction, Direction::In);
    assert_eq!(received.text, "Hello Bea");
    assert_eq!(received.sent_at, hello.sent_at);
    assert_eq!(received.delivery, None);

    // And answers.
    let reply = bea.send_message(&chat_id, "Hi Ada").await.unwrap();
    let answer = next_message(&mut ada_events, &chat_id).await;
    assert_eq!(answer.id, reply.id);
    assert_eq!(answer.direction, Direction::In);
    assert_eq!(answer.text, "Hi Ada");

    let ada_history = until_sent(&ada, &chat_id, &hello.id).await;
    let bea_history = until_sent(&bea, &chat_id, &reply.id).await;
    assert_eq!(ada_history.len(), 2);
    assert_eq!(bea_history.len(), 2);

    // Both histories survive a restart, with the same ids and delivery.
    ada.close().await.unwrap();
    bea.close().await.unwrap();
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    let ada_after = ada.snapshot().chats.into_iter().next().unwrap().messages;
    let bea_after = bea.snapshot().chats.into_iter().next().unwrap().messages;
    assert_eq!(sorted(ada_after), sorted(ada_history));
    assert_eq!(sorted(bea_after), sorted(bea_history));

    // The same text twice within a second makes two messages.
    let first = ada.send_message(&chat_id, "Twice").await.unwrap();
    let second = ada.send_message(&chat_id, "Twice").await.unwrap();
    assert_ne!(first.id, second.id);
    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn messages_sent_before_the_invitation_is_accepted_come_with_it() {
    let _turn = support::take_turn().await;
    let relay = MockRelay::run().await.unwrap();
    let url = relay.url().await.to_string();
    let ada_phone = Phone::new("early-ada", &url);
    let bea_phone = Phone::new("early-bea", &url);
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    ada.create_account("Ada Lund").await.unwrap();
    let bea_account = bea.create_account("Bea Holm").await.unwrap();
    ada.wait_until_published(support::WAIT).await.unwrap();
    bea.wait_until_published(support::WAIT).await.unwrap();
    let mut bea_events = bea.subscribe();

    // Ada adds Bea and writes at once, before Bea has accepted.
    let chat = ada.add_contact(&bea_account.invite_link).await.unwrap();
    let early = ada.send_message(&chat.id, "Are you there?").await.unwrap();

    // The message reaches Bea's invitation: with it, or right after it.
    let invitation = next_invitation(&mut bea_events).await;
    assert_eq!(invitation.id, chat.id);
    let messages = until("the message on the invitation", || {
        let invitation = bea.snapshot().invitations.into_iter().next()?;
        (!invitation.messages.is_empty()).then_some(invitation.messages)
    })
    .await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, early.id);
    assert_eq!(messages[0].text, "Are you there?");

    // Accepting keeps it.
    let accepted = bea.accept_invitation(&invitation.id).await.unwrap();
    assert_eq!(accepted.messages, messages);
    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sending_says_what_is_wrong() {
    let _turn = support::take_turn().await;
    let relay = MockRelay::run().await.unwrap();
    let url = relay.url().await.to_string();
    let phone = Phone::new("send-errors", &url);
    let core = phone.start().await;

    let error = core.send_message("0123abcd", "Hello").await.unwrap_err();
    assert_eq!(error.message, "Create an account first.");

    core.create_account("Ada Lund").await.unwrap();
    let error = core.send_message("0123abcd", "Hello").await.unwrap_err();
    assert_eq!(error.message, "This chat no longer exists.");
    let error = core.send_message("0123abcd", "   ").await.unwrap_err();
    assert_eq!(error.message, "Write a message first.");
    core.close().await.unwrap();
}

//! Something for every message (#123): what someone sends from White Noise shows up in the
//! chat, as text where the core can show it, else as a placeholder that says what it is. A photo
//! with its caption, a reaction, a reply's quote, a thread message, a poll and a kind the core
//! doesn't know all show; the app's own events stay hidden.
//!
//! The other person here is a plain MDK app, as White Noise is, sending each of these the way
//! White Noise does: through MDK's own calls. A sticker needs a published sticker pack, so
//! `core/src/messaging.rs`'s unit tests cover stickers instead.

mod support;

use image::codecs::jpeg::JpegEncoder;
use image::{Rgb, RgbImage};
use marmot_app::{MediaUploadAttachmentRequest, MediaUploadRequest, PollType};
use peoples_chat_core::interface::{CoreEvent, Direction, Message, Placeholder, Quote, Reaction};
use peoples_chat_core::{INVITE_TOKEN_KIND, RADIO_ADDRESS_KIND};
use support::{OtherApp, Phone, TestBlossom, TestRelay, next_invitation, until};

/// A kind no app in the chat knows.
const UNKNOWN_KIND: u64 = 4_242;

fn photo() -> Vec<u8> {
    let image = RgbImage::from_fn(64, 48, |x, y| Rgb([(x * 4) as u8, (y * 5) as u8, 90]));
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 90)
        .encode_image(&image)
        .unwrap();
    jpeg
}

fn find<'a>(messages: &'a [Message], what: impl Fn(&Message) -> bool) -> Option<&'a Message> {
    messages.iter().find(|message| what(message))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_123_what_white_noise_sends_shows_up_in_some_form() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let media = TestBlossom::start().await;
    let ada_phone = Phone::new("issue-123-ada", &relay.url());
    let ada = ada_phone.start().await;
    let ada_hex = ada.create_account("Ada Lund").await.unwrap().public_key;
    ada.wait_until_published(support::WAIT).await.unwrap();

    // Bea is on White Noise. She starts the chat, with a media server, as White Noise does.
    let bea = OtherApp::start_with_media_servers(
        &relay.url(),
        "chat.peoples.client.test-123-bea",
        "Bea Holm",
        vec![media.url()],
    )
    .await;
    let mut ada_events = ada.subscribe();
    let group = bea
        .runtime
        .start_direct_conversation(&bea.account_hex, &ada_hex)
        .await
        .unwrap();
    let chat_id = group.to_string();
    let invitation = next_invitation(&mut ada_events).await;
    assert_eq!(invitation.id, chat_id);
    ada.accept_invitation(&chat_id).await.unwrap();

    // Ada writes; Bea's app has the message before she answers it.
    let asked = ada
        .send_message(&chat_id, "Shall we meet at three?")
        .await
        .unwrap();
    until("Ada's message on Bea's app", || {
        bea.runtime
            .timeline_message(&bea.account_hex, &chat_id, &asked.id)
            .ok()
            .flatten()
    })
    .await;

    // Bea reacts, replies, answers in a thread, sends a photo with a caption, a poll, something
    // no app here knows, and the app's own events, which only peoples.chat sends.
    let send = &bea.runtime;
    let bea_hex = bea.account_hex.as_str();
    send.react_to_message(bea_hex, &group, &asked.id, "❤️")
        .await
        .unwrap();
    send.reply_to_message(bea_hex, &group, &asked.id, "Three works")
        .await
        .unwrap();
    send.send_custom_event(
        bea_hex,
        &group,
        1_111,
        vec![
            vec!["e".to_owned(), asked.id.clone()],
            vec!["k".to_owned(), "9".to_owned()],
        ],
        "In the thread".to_owned(),
    )
    .await
    .unwrap();
    send.upload_media(
        bea_hex,
        &group,
        MediaUploadRequest {
            attachments: vec![MediaUploadAttachmentRequest {
                file_name: "hill.jpg".to_owned(),
                media_type: "image/jpeg".to_owned(),
                plaintext: photo(),
                dim: Some("64x48".to_owned()),
                thumbhash: None,
            }],
            caption: Some("The view from the hill".to_owned()),
            send: true,
            blossom_server: None,
            message_tags: Vec::new(),
        },
    )
    .await
    .unwrap();
    send.create_poll(
        bea_hex,
        &group,
        "Lunch after?".to_owned(),
        vec!["Yes".to_owned(), "No".to_owned()],
        PollType::SingleChoice,
        None,
    )
    .await
    .unwrap();
    send.send_custom_event(bea_hex, &group, UNKNOWN_KIND, Vec::new(), "{}".to_owned())
        .await
        .unwrap();
    for kind in [INVITE_TOKEN_KIND, RADIO_ADDRESS_KIND] {
        send.send_custom_event(bea_hex, &group, kind, Vec::new(), "not shown".to_owned())
            .await
            .unwrap();
    }

    let quote = Quote {
        message_id: asked.id.clone(),
        direction: Some(Direction::Out),
        text: "Shall we meet at three?".to_owned(),
        placeholder: None,
    };
    let check = |messages: &[Message]| -> Option<()> {
        // The reaction, on the message it reacts to.
        let reacted = find(messages, |m| m.id == asked.id)?;
        if reacted.reactions
            != [Reaction {
                emoji: "❤️".to_owned(),
                count: 1,
                mine: false,
            }]
        {
            return None;
        }
        // The reply, with its quote.
        let reply = find(messages, |m| m.text == "Three works")?;
        assert_eq!(reply.direction, Direction::In);
        assert_eq!(reply.placeholder, None);
        assert_eq!(reply.reply_to.as_ref(), Some(&quote));
        // The thread message, as a reply to its parent.
        let thread = find(messages, |m| m.text == "In the thread")?;
        assert_eq!(thread.placeholder, None);
        assert_eq!(thread.reply_to.as_ref(), Some(&quote));
        // The photo, named, with its caption.
        let photo = find(messages, |m| m.placeholder == Some(Placeholder::Photo))?;
        assert_eq!(photo.text, "The view from the hill");
        // The poll, with its question.
        let poll = find(messages, |m| m.placeholder == Some(Placeholder::Poll))?;
        assert_eq!(poll.text, "Lunch after?");
        // The unknown kind, saying only that something came.
        let unknown = find(messages, |m| {
            m.placeholder == Some(Placeholder::Unsupported)
        })?;
        assert_eq!(unknown.text, "");
        // And nothing else: the app's own events stay hidden.
        assert_eq!(messages.len(), 6, "{messages:#?}");
        Some(())
    };
    let chat_messages = |core: &peoples_chat_core::Core| {
        core.snapshot()
            .chats
            .into_iter()
            .find(|chat| chat.id == chat_id)
            .map(|chat| chat.messages)
            .unwrap_or_default()
    };
    until("everything Bea sent", || check(&chat_messages(&ada))).await;

    // The reaction came as a change to Ada's message, which the interface shows.
    let mut changed = false;
    while let Ok(event) = ada_events.try_recv() {
        if let CoreEvent::MessageChanged { message, .. } = event
            && message.id == asked.id
            && !message.reactions.is_empty()
        {
            changed = true;
        }
    }
    assert!(changed, "no messageChanged for the reaction");

    // All of it is in the chat's history after a restart too.
    ada.close().await.unwrap();
    let ada = ada_phone.start().await;
    check(&chat_messages(&ada)).expect("the history after a restart");

    ada.close().await.unwrap();
    bea.close().await;
}

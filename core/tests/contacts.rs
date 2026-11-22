//! Adding a contact (#19): one core starts a direct chat from another's invite link, the other
//! receives an invitation and accepts it, and both keep the chat across a restart. Two cores
//! with their own folders and keys share a relay, as two phones would.

mod support;

use nostr_sdk::prelude::{Keys, ToBech32};
use peoples_chat_core::interface::CoreEvent;
use support::{Phone, TestRelay, WAIT, next_event, next_invitation, now_ms};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_people_become_contacts_from_an_invite_link() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new("ada", &url);
    let bea_phone = Phone::new("bea", &url);

    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    let ada_account = ada.create_account("Ada Lund").await.unwrap();
    let bea_account = bea.create_account("Bea Holm").await.unwrap();
    assert!(
        bea_account.invite_link.starts_with("nostr:nprofile1"),
        "{}",
        bea_account.invite_link
    );
    // Others can invite an account once its relay lists, profile and KeyPackage are out.
    ada.wait_until_published(WAIT).await.unwrap();
    bea.wait_until_published(WAIT).await.unwrap();
    let mut ada_events = ada.subscribe();
    let mut bea_events = bea.subscribe();

    // Ada scans Bea's code, and the chat opens on her side.
    let chat = ada.add_contact(&bea_account.invite_link).await.unwrap();
    assert_eq!(chat.contact.public_key, bea_account.public_key);
    assert_eq!(chat.contact.name, "Bea Holm");
    assert!(chat.messages.is_empty());
    assert_eq!(
        next_event(&mut ada_events).await,
        CoreEvent::ChatAdded { chat: chat.clone() }
    );
    assert_eq!(ada.snapshot().chats, vec![chat.clone()]);

    // Adding her again opens the same chat.
    assert_eq!(
        ada.add_contact(&bea_account.invite_link).await.unwrap(),
        chat
    );

    // Bea receives Ada's invitation.
    let invitation = next_invitation(&mut bea_events).await;
    assert_eq!(invitation.id, chat.id);
    assert_eq!(invitation.from.public_key, ada_account.public_key);
    assert_eq!(invitation.from.name, "Ada Lund");
    assert!(invitation.messages.is_empty());
    let received_ago = now_ms().saturating_sub(invitation.received_at);
    assert!(
        received_ago < WAIT.as_millis() as u64 * 2,
        "received {received_ago} ms ago"
    );
    assert_eq!(bea.snapshot().invitations, vec![invitation.clone()]);
    assert!(bea.snapshot().chats.is_empty());

    // She accepts, and the invitation becomes the chat.
    let accepted = bea.accept_invitation(&invitation.id).await.unwrap();
    assert_eq!(accepted.id, chat.id);
    assert_eq!(accepted.contact, invitation.from);
    assert_eq!(
        next_event(&mut bea_events).await,
        CoreEvent::InvitationRemoved {
            invitation_id: invitation.id.clone()
        }
    );
    assert_eq!(
        next_event(&mut bea_events).await,
        CoreEvent::ChatAdded {
            chat: accepted.clone()
        }
    );
    let snapshot = bea.snapshot();
    assert_eq!(snapshot.chats, vec![accepted.clone()]);
    assert!(snapshot.invitations.is_empty());

    let error = bea.accept_invitation(&invitation.id).await.unwrap_err();
    assert_eq!(error.message, "This request is no longer available.");
    // Adding Ada now opens the chat they share.
    assert_eq!(
        bea.add_contact(&ada_account.invite_link).await.unwrap(),
        accepted
    );

    // Both keep the chat, with the contact's name, across a restart.
    ada.close().await.unwrap();
    bea.close().await.unwrap();
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    assert_eq!(ada.snapshot().chats, vec![chat]);
    let snapshot = bea.snapshot();
    assert_eq!(snapshot.chats, vec![accepted]);
    assert!(snapshot.invitations.is_empty());
    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adding_a_contact_says_what_is_wrong() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let phone = Phone::new("contact-errors", &url);
    let core = phone.start().await;

    // Someone who never published an account.
    let stranger = Keys::generate().public_key().to_bech32().unwrap();
    let error = core.add_contact(&stranger).await.unwrap_err();
    assert_eq!(error.message, "Create an account first.");

    let account = core.create_account("Ada Lund").await.unwrap();
    let secret = Keys::generate().secret_key().to_bech32().unwrap();
    for (text, message) in [
        (
            account.invite_link.as_str(),
            "That is your own invite link.",
        ),
        (
            "bea@example.org",
            "Finding people by name is not possible yet. Paste an invite link instead.",
        ),
        (secret.as_str(), "That is your secret key. Never share it."),
        ("https://example.org", "That is not an invite link."),
    ] {
        let error = core.add_contact(text).await.unwrap_err();
        assert_eq!(error.message, message, "{text}");
    }

    let error = core.add_contact(&stranger).await.unwrap_err();
    assert_eq!(
        error.message,
        "Could not find this person's account. Try again later."
    );
    assert!(core.snapshot().chats.is_empty());

    let error = core.accept_invitation("0123abcd").await.unwrap_err();
    assert_eq!(error.message, "This request is no longer available.");
    core.close().await.unwrap();
}

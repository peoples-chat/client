//! Personal invitations (#95): Ada invites Bea with a link. Bea, who has no account yet, sees who
//! invited her, creates an account and starts the chat; Ada's core opens it without a request,
//! because Bea's app sends the link's token back. The link works once: Cid, who uses it after
//! her, arrives as an ordinary request. No server takes part, only the relay.

mod support;

use peoples_chat_core::interface::CoreEvent;
use support::{Phone, TestRelay, until};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_invitation_opens_the_chat_once() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new("invites-ada", &url);
    let bea_phone = Phone::new("invites-bea", &url);
    let cid_phone = Phone::new("invites-cid", &url);
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    let cid = cid_phone.start().await;
    let ada_account = ada.create_account("Ada Lund").await.unwrap();
    ada.wait_until_published(support::WAIT).await.unwrap();

    // Ada invites Bea.
    let mut ada_events = ada.subscribe();
    let invite = ada.create_invite(" Bea ").await.unwrap();
    assert_eq!(invite.name, "Bea");
    assert!(
        invite
            .link
            .starts_with(&format!("{}/", ada_account.share_link)),
        "{}",
        invite.link
    );
    assert_eq!(invite.joined_chat_id, None);
    assert_eq!(ada.snapshot().invites, vec![invite.clone()]);
    assert!(matches!(
        ada_events.recv().await.unwrap(),
        CoreEvent::InviteChanged { .. }
    ));
    let error = ada.create_invite("  ").await.unwrap_err();
    assert_eq!(error.message, "Enter a name.");

    // Bea has no account yet, and sees who invited her.
    let from = bea.preview_invite(&invite.link).await.unwrap();
    assert_eq!(from.name, "Ada Lund");
    assert_eq!(from.public_key, ada_account.public_key);

    // She creates an account and starts the chat from the link.
    bea.create_account("Bea Holm").await.unwrap();
    bea.wait_until_published(support::WAIT).await.unwrap();
    let chat = bea.add_contact(&invite.link).await.unwrap();

    // Ada's core opens the chat without a request, and marks the invitation as used.
    until("Ada's chat with Bea", || {
        let snapshot = ada.snapshot();
        snapshot
            .chats
            .iter()
            .any(|known| known.id == chat.id)
            .then_some(())
    })
    .await;
    let snapshot = ada.snapshot();
    assert!(
        snapshot.invitations.is_empty(),
        "{:?}",
        snapshot.invitations
    );
    assert_eq!(
        snapshot.invites[0].joined_chat_id.as_deref(),
        Some(&*chat.id)
    );

    // The link works once: Cid, who uses it next, is an ordinary request.
    cid.create_account("Cid Berg").await.unwrap();
    cid.wait_until_published(support::WAIT).await.unwrap();
    let cid_chat = cid.add_contact(&invite.link).await.unwrap();
    until("Cid's request at Ada", || {
        let snapshot = ada.snapshot();
        snapshot
            .invitations
            .iter()
            .any(|invitation| invitation.id == cid_chat.id)
            .then_some(())
    })
    .await;
    // Give a token time to arrive, then check that it opened nothing.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let snapshot = ada.snapshot();
    assert!(snapshot.chats.iter().all(|known| known.id != cid_chat.id));
    assert_eq!(snapshot.invites.len(), 1);

    // The invitation stays across a restart, used.
    ada.close().await.unwrap();
    let ada = ada_phone.start().await;
    let invites = ada.snapshot().invites;
    assert_eq!(invites.len(), 1);
    assert_eq!(invites[0].joined_chat_id.as_deref(), Some(&*chat.id));
    assert_eq!(invites[0].link, invite.link);

    // Deleting the account deletes the invitations.
    ada.delete_account().await.unwrap();
    assert!(ada.snapshot().invites.is_empty());
    assert!(!ada_phone.data_dir().join("invites.json").exists());
    ada.close().await.unwrap();
    bea.close().await.unwrap();
    cid.close().await.unwrap();
}

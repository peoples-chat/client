//! #126: a chat whose invitation hasn't reached the contact says so, also after a restart, and
//! stops saying so once the invitation has gone.
//!
//! The relay refuses gift wraps (kind 1059), which carry invitations, until the test lets them
//! through. Everything else, such as KeyPackages and chat messages, it takes.

mod support;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use nostr_relay_builder::prelude::{
    BoxedFuture, Event, Kind, PolicyResult, RelayBuilder, WritePolicy,
};
use peoples_chat_core::Core;
use peoples_chat_core::interface::{CoreEvent, UndeliveredInvitation};
use support::{Phone, TestRelay, until};

/// Refuses gift wraps while its switch is on.
#[derive(Debug)]
struct RefuseInvitations(Arc<AtomicBool>);

impl WritePolicy for RefuseInvitations {
    fn admit_event<'a>(
        &'a self,
        event: &'a Event,
        _addr: &'a SocketAddr,
    ) -> BoxedFuture<'a, PolicyResult> {
        let refuse = self.0.load(Ordering::SeqCst) && event.kind == Kind::GiftWrap;
        Box::pin(async move {
            if refuse {
                PolicyResult::Reject("blocked: this relay takes no invitations now".into())
            } else {
                PolicyResult::Accept
            }
        })
    }
}

fn undelivered(core: &Core, chat_id: &str) -> Option<UndeliveredInvitation> {
    core.snapshot()
        .chats
        .into_iter()
        .find(|chat| chat.id == chat_id)
        .and_then(|chat| chat.undelivered)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_126_a_chat_says_when_its_invitation_has_not_reached_the_contact() {
    let _turn = support::take_turn().await;
    let refusing = Arc::new(AtomicBool::new(true));
    let relay = TestRelay::start_with(
        RelayBuilder::default().write_policy(RefuseInvitations(Arc::clone(&refusing))),
        "127.0.0.1:0",
    )
    .await;
    let ada_phone = Phone::new("issue-126-ada", &relay.url());
    let bea_phone = Phone::new("issue-126-bea", &relay.url());
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    ada.create_account("Ada Lund").await.unwrap();
    bea.create_account("Bea Holm").await.unwrap();
    ada.wait_until_published(support::WAIT).await.unwrap();
    bea.wait_until_published(support::WAIT).await.unwrap();

    // The chat opens at once, and then says that Bea hasn't got the invitation.
    let mut ada_events = ada.subscribe();
    let bea_link = bea.snapshot().account.unwrap().invite_link;
    let chat = ada.add_contact(&bea_link).await.unwrap();
    let shown = until("the chat to say the invitation is undelivered", || {
        undelivered(&ada, &chat.id)
    })
    .await;
    // MDK gives a reason only for an invitation refused as too large.
    assert_eq!(shown.reason, None);
    assert!(shown.since > 0);
    let mut announced = false;
    while let Ok(event) = ada_events.try_recv() {
        announced |= matches!(
            &event,
            CoreEvent::InvitationDeliveryChanged { chat_id, undelivered: Some(_) } if *chat_id == chat.id
        );
    }
    assert!(announced, "no invitationDeliveryChanged for the chat");
    assert!(bea.snapshot().invitations.is_empty());

    // It still says so after a restart: the core reads the waiting invitations as it starts.
    ada.close().await.unwrap();
    let ada = ada_phone.start().await;
    let after_restart = until("the chat after the restart", || {
        ada.snapshot()
            .chats
            .into_iter()
            .find(|known| known.id == chat.id)
    })
    .await;
    assert_eq!(after_restart.undelivered, Some(shown));

    // Once the relay takes invitations again, the core's next try delivers it: Bea gets the
    // invitation, and Ada's chat stops saying it hasn't arrived.
    let mut ada_events = ada.subscribe();
    refusing.store(false, Ordering::SeqCst);
    until("Bea to get the invitation", || {
        bea.snapshot()
            .invitations
            .into_iter()
            .find(|invitation| invitation.id == chat.id)
    })
    .await;
    until("the chat to show the invitation delivered", || {
        undelivered(&ada, &chat.id).is_none().then_some(())
    })
    .await;
    let mut cleared = false;
    while let Ok(event) = ada_events.try_recv() {
        cleared |= matches!(
            &event,
            CoreEvent::InvitationDeliveryChanged { chat_id, undelivered: None } if *chat_id == chat.id
        );
    }
    assert!(cleared, "no invitationDeliveryChanged that clears the chat");

    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

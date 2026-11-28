//! Invitations that haven't reached the contact (#126). Adding a contact opens the chat at once,
//! and MDK sends the invitation, an MLS Welcome, to the contact's inbox relays afterwards. When
//! no relay takes it, MDK keeps it and lists it in `pending_welcome_deliveries` until one does,
//! and announces it once with a `WelcomeDeliveryPending` notice.
//!
//! The core reads that list as it starts following the account, after each chat it starts, and
//! after each notice. A chat with an invitation in the list shows it as `undelivered`. The core
//! sends the invitations in the list again every [`REDELIVERY_EVERY`] and as soon as relays are
//! back after an outage, and reads the list again after each try: a chat whose invitation is no
//! longer in it has reached a relay, and `undelivered` goes away.

use std::sync::{Arc, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use super::{Core, Inner, upgrade};
use crate::interface::{CoreEvent, UndeliveredInvitation as UndeliveredView, UndeliveredReason};
use crate::messaging::{InvitationFailures, UndeliveredInvitation};

/// How often the core sends an undelivered invitation again. One a relay refused as too large is
/// sent again only when relays come back, since the same relays would refuse it again.
pub(super) const REDELIVERY_EVERY: Duration = Duration::from_secs(20);

/// The interface's view of the oldest undelivered invitation to `chat_id` in `list`, if any.
pub(super) fn view_for(list: &[UndeliveredInvitation], chat_id: &str) -> Option<UndeliveredView> {
    list.iter()
        .find(|invitation| invitation.chat_id == chat_id)
        .map(|invitation| UndeliveredView {
            since: invitation.since,
            reason: invitation.too_large.then_some(UndeliveredReason::TooLarge),
        })
}

impl Core {
    /// The undelivered invitations as the core last read them, oldest first. Whoever also holds
    /// the state lock takes that first.
    pub(super) fn undelivered(&self) -> MutexGuard<'_, Vec<UndeliveredInvitation>> {
        self.inner
            .undelivered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Reads MDK's list of undelivered invitations, and shows it in the chats.
    pub(super) async fn read_undelivered(&self) {
        let Some(account) = self.state().account.clone() else {
            return;
        };
        match self
            .inner
            .messaging
            .undelivered_invitations(&account.public_key)
            .await
        {
            Ok(list) => self.set_undelivered(list),
            Err(error) => log::warn!("could not read the undelivered invitations: {error}"),
        }
    }

    /// Takes `list` as the undelivered invitations, and emits
    /// [`CoreEvent::InvitationDeliveryChanged`] for each chat whose state changed. Chats the
    /// snapshot doesn't have yet get theirs when they arrive.
    fn set_undelivered(&self, list: Vec<UndeliveredInvitation>) {
        let mut state = self.state();
        let mut undelivered = self.undelivered();
        for chat in &mut state.chats {
            let view = view_for(&list, &chat.id);
            if chat.undelivered != view {
                log::info!(
                    "a chat's invitation is {}",
                    if view.is_some() {
                        "not delivered yet"
                    } else {
                        "delivered"
                    }
                );
                chat.undelivered = view.clone();
                self.emit(CoreEvent::InvitationDeliveryChanged {
                    chat_id: chat.id.clone(),
                    undelivered: view,
                });
            }
        }
        *undelivered = list;
    }

    /// Sends the undelivered invitations again, and reads the list again afterwards. With
    /// `all`, also those a relay refused as too large.
    async fn redeliver_invitations(&self, all: bool) {
        let Some(account) = self.state().account.clone() else {
            return;
        };
        let due: Vec<String> = self
            .undelivered()
            .iter()
            .filter(|invitation| all || !invitation.too_large)
            .map(|invitation| invitation.message_id.clone())
            .collect();
        if due.is_empty() {
            return;
        }
        for message_id in due {
            match self
                .inner
                .messaging
                .redeliver_invitation(&account.public_key, &message_id)
                .await
            {
                Ok(()) => log::info!("an invitation was delivered on another try"),
                Err(error) => log::info!("an invitation still can't be delivered: {error}"),
            }
        }
        self.read_undelivered().await;
    }

    /// Reads the undelivered invitations again soon, without waiting for it: once a chat has
    /// started, the read waits for its invitation's first try.
    pub(super) fn read_undelivered_soon(&self) {
        let core = self.clone();
        tokio::spawn(async move { core.read_undelivered().await });
    }
}

/// Follows MDK's notices about undelivered invitations, and sends them again from time to time
/// and when `relays_back` is notified.
pub(super) async fn watch_invitations(
    core: Weak<Inner>,
    mut failures: InvitationFailures,
    relays_back: Arc<tokio::sync::Notify>,
) {
    loop {
        let waiting = match upgrade(&core) {
            Some(core) => core.undelivered().iter().any(|i| !i.too_large),
            None => return,
        };
        let retry = async {
            if waiting {
                tokio::time::sleep(REDELIVERY_EVERY).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        // `None`: read the list again. `Some(all)`: send again, too-large ones too if `all`.
        let redeliver = tokio::select! {
            failure = failures.next() => match failure {
                Some(_) => None,
                None => return,
            },
            () = retry => Some(false),
            () = relays_back.notified() => Some(true),
        };
        let Some(core) = upgrade(&core) else { return };
        match redeliver {
            None => core.read_undelivered().await,
            Some(all) => core.redeliver_invitations(all).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(
        chat_id: &str,
        message_id: &str,
        since: u64,
        too_large: bool,
    ) -> UndeliveredInvitation {
        UndeliveredInvitation {
            chat_id: chat_id.to_owned(),
            message_id: message_id.to_owned(),
            since,
            too_large,
        }
    }

    #[test]
    fn issue_126_a_chat_shows_its_oldest_undelivered_invitation_with_the_reason() {
        let list = [
            pending("a", "1", 1_000, true),
            pending("b", "2", 2_000, false),
            pending("a", "3", 3_000, false),
        ];
        assert_eq!(
            view_for(&list, "a"),
            Some(UndeliveredView {
                since: 1_000,
                reason: Some(UndeliveredReason::TooLarge)
            })
        );
        assert_eq!(
            view_for(&list, "b"),
            Some(UndeliveredView {
                since: 2_000,
                reason: None
            })
        );
        assert_eq!(view_for(&list, "c"), None);
    }
}

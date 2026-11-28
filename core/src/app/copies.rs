//! The core's own copies of message text (#127). MDK keeps messages in its encrypted databases;
//! the core keeps a received radio message's text only until MDK has the message too, encrypted
//! meanwhile. Once MDK has it, in whatever state, MDK's record is the only one: a deletion (#122)
//! or an expiry (#124) of it has no copy left to reach.
//!
//! A radio message whose relay copy hasn't come is out of MDK's reach, so in a chat whose
//! messages disappear, the core removes it itself when its time is up.

use std::time::Duration;

use super::{Core, now_ms, radio_view, upgrade};
use crate::interface::Message;
use crate::radio_store::RadioMessage;

impl Core {
    /// Takes the account's data key into use, and encrypts what an earlier version kept in plain.
    pub(super) fn use_data_key(&self, account_hex: &str) {
        let vault = &self.inner.vault;
        vault.use_account(account_hex);
        let sealed = self
            .inner
            .radio_store
            .seal_plain(|text, id| vault.seal(text, id));
        if let Err(error) = sealed.and_then(|()| self.inner.outbox.seal_plain()) {
            log::warn!("could not encrypt the messages kept so far: {error}");
        }
    }

    /// Drops the texts MDK has by now or whose time is up, and waits for the others' time. Their
    /// relay copies may have come while the app was closed.
    pub(super) fn settle_radio_copies(&self, account_hex: &str) {
        for kept in self.inner.radio_store.messages_with_text() {
            if self.settle_radio_copy(account_hex, &kept) {
                self.expire_radio_copy_later(&kept.id, kept.expires_at);
            }
        }
    }

    /// Drops the text of `kept` if MDK has the message or its time is up. Returns whether the
    /// core still keeps it.
    fn settle_radio_copy(&self, account_hex: &str, kept: &RadioMessage) -> bool {
        let done = kept.expires_at.is_some_and(|at| at <= now_ms())
            || self
                .inner
                .messaging
                .has_message(account_hex, &kept.chat_id, &kept.id);
        if done {
            self.drop_radio_text(&kept.id);
        }
        !done
    }

    /// MDK has the message `id`, or its time is up: the radio store keeps its text no longer.
    pub(super) fn drop_radio_text(&self, id: &str) {
        if let Err(error) = self.inner.radio_store.forget_text(id) {
            log::warn!("could not drop a radio message's text: {error}");
        }
    }

    /// The message `kept` as a chat's history shows it while only the radio store has it.
    pub(super) fn radio_only_message(
        &self,
        account_hex: &str,
        kept: &RadioMessage,
    ) -> Option<Message> {
        if !kept.holds_text() || !self.settle_radio_copy(account_hex, kept) {
            return None;
        }
        let text = match &kept.sealed {
            Some(sealed) => self.inner.vault.open(sealed, &kept.id)?,
            None => kept.plain.clone()?,
        };
        Some(radio_view(kept, text))
    }

    /// Removes the radio message `id` at `expires_at`, if it disappears at all.
    pub(super) fn expire_radio_copy_later(&self, id: &str, expires_at: Option<u64>) {
        let Some(expires_at) = expires_at else { return };
        let wait = Duration::from_millis(expires_at.saturating_sub(now_ms()));
        let core = std::sync::Arc::downgrade(&self.inner);
        let id = id.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            if let Some(core) = upgrade(&core) {
                core.expire_radio_copy(&id);
            }
        });
    }

    /// The time of the radio message `id` is up. If only the radio store had it, it leaves the
    /// disk and the chat.
    fn expire_radio_copy(&self, id: &str) {
        let Some(kept) = self.inner.radio_store.message(id) else {
            return;
        };
        let Ok(account) = self.account() else { return };
        // With its relay copy in MDK, the message's expiry is MDK's to see to.
        if !kept.holds_text()
            || self
                .inner
                .messaging
                .has_message(&account.public_key, &kept.chat_id, id)
        {
            self.drop_radio_text(id);
            return;
        }
        self.drop_radio_text(id);
        log::info!("a radio message disappeared");
        self.remove_message(&kept.chat_id, id);
    }
}

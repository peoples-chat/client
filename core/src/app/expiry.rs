//! Disappearing messages (#124).
//!
//! Anyone in a chat can turn on Marmot's message retention, as White Noise for Android offers in
//! direct chats. Each message sent then carries when it expires: the sender's clock when it was
//! sent, plus the chat's timer then. Expiry is advisory, since it counts from the sender's
//! clock. MDK deletes expired messages only when the app asks, and holds back the expired
//! messages the user hasn't read. So the core:
//!
//! - hides a message once its time is up, with `messageRemoved`, as White Noise for Android does
//!   before its sweep;
//! - drops the text of radio messages then too, keeping their ids for the replay check;
//! - has MDK delete expired messages soon after start, shortly after a message expires, after
//!   the user reads a chat, and every minute;
//! - marks a chat read when the interface says the user has seen it ([`Core::mark_read`]);
//! - follows MDK's deletions, and the chat's timer as it changes (`disappearingChanged`).

use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

/// How often the core has MDK delete expired messages while nothing expires sooner, as White
/// Noise for Android does in an open chat.
const SWEEP_EVERY: Duration = Duration::from_secs(60);
/// How long after a message's time is up the core asks MDK to delete it: MDK waits out 5
/// seconds of clock skew first.
const SWEEP_AFTER_EXPIRY: Duration = Duration::from_secs(6);

/// Wakes the watcher of disappearing messages.
#[derive(Default)]
pub(super) struct ExpiryWake {
    notify: tokio::sync::Notify,
    /// The next wake also asks MDK to delete expired messages, as after the user read a chat.
    sweep: AtomicBool,
}

impl ExpiryWake {
    /// Look again at when the next message expires.
    pub(super) fn look_again(&self) {
        self.notify.notify_one();
    }

    /// Ask MDK to delete what has expired, now.
    pub(super) fn sweep_now(&self) {
        self.sweep.store(true, Ordering::Release);
        self.notify.notify_one();
    }
}

impl Core {
    /// The user has seen the chat `chat_id` up to its newest message (#124). The interface calls
    /// this while it shows the chat. MDK's read mark stays on this device; nothing is sent, so
    /// the other person isn't told. MDK deletes an expired message only once it is read.
    pub async fn mark_read(&self, chat_id: &str) -> Result<(), CommandError> {
        let account = self.account()?;
        if self.chat(chat_id).is_none() {
            return Err(CommandError::new("This chat no longer exists."));
        }
        self.inner
            .messaging
            .mark_chat_read(&account.public_key, chat_id)
            .map_err(|error| failed("mark the chat as read", error))?;
        self.inner.expiry.sweep_now();
        Ok(())
    }

    /// When a message sent at `sent_at` in the chat `chat_id` expires, by the chat's timer now.
    /// For messages MDK hasn't stamped: radio packets carry no expiry.
    pub(super) fn expiry_for(&self, chat_id: &str, sent_at: u64) -> Option<u64> {
        let secs = self
            .state()
            .chats
            .iter()
            .find(|chat| chat.id == chat_id)?
            .disappear_after?;
        Some(sent_at.saturating_add(secs.saturating_mul(1000)))
    }

    /// `sent`, one of the user's messages MDK has just taken, with when it disappears. MDK
    /// stamps that only once the message is published, so until then the chat's timer says.
    pub(super) fn stamp_expiry(&self, mut sent: TextMessage) -> TextMessage {
        if sent.expires_at.is_none() {
            sent.expires_at = self.expiry_for(&sent.chat_id, sent.sent_at);
        }
        sent
    }

    /// Takes the chat's timer as MDK has it now, and emits `disappearingChanged` if it changed.
    pub(super) fn disappearing_changed(&self, chat: &DirectChat) {
        let mut state = self.state();
        let Some(known) = state.chats.iter_mut().find(|known| known.id == chat.id) else {
            return;
        };
        if known.disappear_after == chat.disappear_after {
            return;
        }
        known.disappear_after = chat.disappear_after;
        log::info!(
            "a chat's messages now disappear after {:?} seconds",
            chat.disappear_after
        );
        self.emit(CoreEvent::DisappearingChanged {
            chat_id: chat.id.clone(),
            disappear_after: chat.disappear_after,
        });
    }

    /// Takes away the messages whose time is up at `now`, and the text of radio messages.
    /// Returns whether anything expired.
    fn expire_due(&self, now: u64) -> bool {
        let due = |message: &Message| message.expires_at.is_some_and(|at| at <= now);
        let removed = self.remove_messages(due);
        let radio = self
            .inner
            .radio_store
            .expire(now, &[])
            .unwrap_or_else(|error| {
                log::warn!("could not drop the text of expired radio messages: {error}");
                Vec::new()
            });
        if !removed.is_empty() || !radio.is_empty() {
            log::info!(
                "{} messages disappeared, {} of them kept from the radio",
                removed.len().max(radio.len()),
                radio.len()
            );
        }
        !removed.is_empty() || !radio.is_empty()
    }

    /// MDK deleted the message `message_id`, whose time was up.
    pub(super) fn message_expired(&self, message_id: &str) {
        self.remove_messages(|message| message.id == message_id);
        if let Err(error) = self
            .inner
            .radio_store
            .expire(now_ms(), &[message_id.to_owned()])
        {
            log::warn!("could not drop the text of an expired radio message: {error}");
        }
    }

    /// Takes the messages that `gone` picks out of the chats and invitations, and emits
    /// `messageRemoved` for each.
    fn remove_messages(&self, gone: impl Fn(&Message) -> bool) -> Vec<String> {
        let mut state = self.state();
        let state = &mut *state;
        let lists = state
            .chats
            .iter_mut()
            .map(|chat| (&chat.id, &mut chat.messages))
            .chain(
                state
                    .invitations
                    .iter_mut()
                    .map(|invitation| (&invitation.id, &mut invitation.messages)),
            );
        let mut removed = Vec::new();
        for (chat_id, messages) in lists {
            messages.retain(|message| {
                if !gone(message) {
                    return true;
                }
                removed.push((chat_id.clone(), message.id.clone()));
                false
            });
        }
        for (chat_id, message_id) in &removed {
            self.emit(CoreEvent::MessageRemoved {
                chat_id: chat_id.clone(),
                message_id: message_id.clone(),
            });
        }
        removed.into_iter().map(|(_, id)| id).collect()
    }

    /// When the next message expires, in milliseconds since the Unix epoch.
    fn next_expiry(&self) -> Option<u64> {
        let state = self.state();
        let shown = state
            .chats
            .iter()
            .flat_map(|chat| &chat.messages)
            .chain(state.invitations.iter().flat_map(|i| &i.messages))
            .filter_map(|message| message.expires_at)
            .min();
        [shown, self.inner.radio_store.next_expiry()]
            .into_iter()
            .flatten()
            .min()
    }

    /// Has MDK delete expired messages. Its deletions come back through the message watcher.
    async fn sweep(&self) {
        let Some(account) = self.state().account.clone() else {
            return;
        };
        match self
            .inner
            .messaging
            .sweep_expired(&account.public_key, now_ms())
            .await
        {
            Ok(0) => {}
            Ok(pruned) => log::info!("MDK deleted {pruned} expired messages"),
            Err(error) => log::warn!("could not delete expired messages: {error}"),
        }
    }
}

/// Hides messages as their time is up, and has MDK delete them: soon after start, shortly after
/// a message expires, when asked, and every minute.
pub(super) async fn watch_expiry(core: Weak<Inner>, wake: Arc<ExpiryWake>) {
    let mut sweep_at = Instant::now();
    loop {
        let Some(core) = upgrade(&core) else { return };
        let now = now_ms();
        if core.expire_due(now) {
            sweep_at = sweep_at.min(Instant::now() + SWEEP_AFTER_EXPIRY);
        }
        if wake.sweep.swap(false, Ordering::AcqRel) || Instant::now() >= sweep_at {
            core.sweep().await;
            sweep_at = Instant::now() + SWEEP_EVERY;
        }
        let mut next = sweep_at;
        if let Some(at) = core.next_expiry() {
            next = next.min(Instant::now() + Duration::from_millis(at.saturating_sub(now_ms())));
        }
        drop(core);
        tokio::select! {
            () = tokio::time::sleep_until(next) => {}
            () = wake.notify.notified() => {}
        }
    }
}

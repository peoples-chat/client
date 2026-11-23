//! Chat text over radio (#40): sending, receiving and confirming through the radio the core
//! uses.
//!
//! - **Sending.** MDK gets the message first. It keeps it and publishes it once a relay can be
//!   reached: that copy is the permanent record (#47). The radio carries a packet with the same
//!   text and time, so the other phone rebuilds the same id (`docs/radio-format-v0.md`).
//! - **Receiving.** A packet goes through the [`RadioInbox`] (#42). What passes it is kept in the
//!   radio store and shown, marked as radio. When the relay copy arrives later, the chat keeps one
//!   message.
//! - **Confirming.** When the other radio confirms a message, it shows as confirmed.
//!
//! The core uses the radio only while the switch that forces it is on; choosing the transport
//! by itself is #45.

use std::collections::HashSet;
use std::sync::{Arc, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use tokio::sync::broadcast::{self, error::RecvError};
use tokio::task::JoinHandle;

use super::{Core, Inner, failed, upgrade};
use crate::interface::{Account, CommandError, CoreEvent, Message, PairedRadio, RadioStatus};
use crate::messaging::TextMessage;
use crate::radio::format::{self, MAX_RADIO_TEXT_BYTES};
use crate::radio::{Radio, RadioEvent, RadioInbox, RadioInfo, ReceivedText, Refusal};
use crate::radio_store::{ContactRadio, RadioMessage};

/// How often the core takes each chat's current radio keys while it has a radio. Keys can't be
/// had while a chat waits for a relay to confirm a change, so the newest are kept beforehand.
const KEY_REFRESH: Duration = Duration::from_secs(60);

/// The radio the core uses, and the watcher that follows its events.
pub(super) struct Attached {
    radio: Arc<dyn Radio>,
    info: RadioInfo,
    connected: bool,
    watcher: JoinHandle<()>,
}

impl Drop for Attached {
    fn drop(&mut self) {
        self.watcher.abort();
    }
}

/// The radio parts of the core's state.
#[derive(Default)]
pub(super) struct RadioParts {
    pub(super) attached: std::sync::Mutex<Option<Attached>>,
    /// The direct chats' radio keys and replay windows.
    pub(super) inbox: std::sync::Mutex<RadioInbox>,
    /// Chats with a radio send in flight. A new outgoing message of theirs waits for the send to
    /// finish, so that the one it sends shows as radio from the start.
    pub(super) sending: std::sync::Mutex<HashSet<String>>,
}

impl Core {
    /// Uses `radio` for radio messages from now on, in place of any radio used before. Emits
    /// [`CoreEvent::RadioChanged`].
    ///
    /// Pairing (#24) will connect a radio and hand it over here. Until then, tests hand over
    /// mock radios.
    pub async fn attach_radio(&self, radio: Arc<dyn Radio>) -> Result<RadioStatus, CommandError> {
        let info = radio
            .info()
            .await
            .map_err(|error| failed("use the radio", error))?;
        // Following starts before anything else, so that no message is missed.
        let events = radio.events();
        let watcher = tokio::spawn(watch_radio(Arc::downgrade(&self.inner), events));
        let previous = self.attached().replace(Attached {
            radio,
            info,
            connected: true,
            watcher,
        });
        drop(previous);
        self.keep_radio_keys().await;
        Ok(self.radio_changed())
    }

    /// Stops using the radio. Emits [`CoreEvent::RadioChanged`] if there was one.
    pub fn detach_radio(&self) {
        let previous = self.attached().take();
        if previous.is_some() {
            drop(previous);
            self.radio_changed();
        }
    }

    /// Sends every message by radio, or stops doing so: the switch for testing until the core
    /// chooses the transport by itself (#45). It stays across restarts. Emits
    /// [`CoreEvent::RadioChanged`].
    pub fn set_force_radio(&self, forced: bool) -> Result<RadioStatus, CommandError> {
        self.inner
            .radio_store
            .set_forced(forced)
            .map_err(|error| failed("change the radio setting", error))?;
        Ok(self.radio_changed())
    }

    /// Records which radio the contact `contact_public_key` has: the address book that the
    /// radio address records will fill from the chat (#34). Until then, tests fill it.
    pub fn set_contact_radio(
        &self,
        contact_public_key: &str,
        radio_public_key: &str,
    ) -> Result<(), CommandError> {
        let is_key = |key: &str| key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit());
        if !is_key(contact_public_key) || !is_key(radio_public_key) {
            return Err(CommandError::new("That is not a public key."));
        }
        self.inner
            .radio_store
            .set_contact_radio(
                contact_public_key,
                ContactRadio {
                    public_key: radio_public_key.to_ascii_lowercase(),
                },
            )
            .map_err(|error| failed("keep the contact's radio", error))
    }

    /// The radio as an interface shows it.
    pub(super) fn radio_status(&self) -> RadioStatus {
        let paired = self.attached().as_ref().map(|attached| PairedRadio {
            name: attached.info.name.clone(),
            public_key: attached.info.public_key.clone(),
            connected: attached.connected,
        });
        RadioStatus {
            paired,
            forced: self.inner.radio_store.forced(),
        }
    }

    /// Puts the radio's status into the snapshot, and emits it if it changed.
    fn radio_changed(&self) -> RadioStatus {
        let status = self.radio_status();
        let changed = {
            let mut state = self.state();
            let changed = state.radio != status;
            state.radio = status.clone();
            changed
        };
        if changed {
            self.emit(CoreEvent::RadioChanged {
                radio: status.clone(),
            });
        }
        status
    }

    /// Sends `text` in the chat `chat_id` by radio, as `send_message` does with the switch on.
    ///
    /// Checks first what would stop the radio, and says so without sending anything. Once the
    /// message is MDK's, it is sent whatever the radio does: if the radio fails now, the message
    /// waits for the internet.
    pub(super) async fn send_by_radio(
        &self,
        account: &Account,
        chat_id: &str,
        text: &str,
    ) -> Result<Message, CommandError> {
        if !format::fits(text) {
            return Err(CommandError::new(format!(
                "That is too long for radio. Keep it to {MAX_RADIO_TEXT_BYTES} bytes."
            )));
        }
        let chat = self
            .chat(chat_id)
            .ok_or_else(|| CommandError::new("This chat no longer exists."))?;
        let Some(radio) = self.attached().as_ref().map(|a| Arc::clone(&a.radio)) else {
            return Err(CommandError::new("Pair a radio first."));
        };
        let Some(contact_radio) = self
            .inner
            .radio_store
            .contact_radio(&chat.contact.public_key)
        else {
            return Err(CommandError::new(format!(
                "{}'s radio isn't known yet.",
                chat.contact.name
            )));
        };
        // The chat's current keys if it is settled, else the newest kept ones.
        self.keep_chat_keys(chat_id, &chat.contact.public_key).await;
        let Some((epoch, key)) = self.inbox().sending_key(chat_id) else {
            return Err(CommandError::new(
                "Could not send by radio: the chat's keys aren't ready. Try again.",
            ));
        };

        self.radio_sending().insert(chat_id.to_owned());
        let sent = self
            .radio_send(account, chat_id, text, radio, &contact_radio, epoch, &key)
            .await;
        self.radio_sending().remove(chat_id);
        let message = sent?;
        self.apply_message(&message);
        // Anything else sent in the chat meanwhile waited; it comes in now.
        for other in self
            .inner
            .messaging
            .chat_messages(&account.public_key, chat_id)
            .unwrap_or_default()
        {
            if other.outgoing {
                self.apply_message(&other);
            }
        }
        self.chat(chat_id)
            .and_then(|chat| chat.messages.into_iter().find(|m| m.id == message.id))
            .ok_or_else(|| failed("send the message", "the message is missing after sending"))
    }

    /// Hands `text` to MDK, then to the radio. Returns the message as MDK keeps it.
    #[allow(clippy::too_many_arguments)]
    async fn radio_send(
        &self,
        account: &Account,
        chat_id: &str,
        text: &str,
        radio: Arc<dyn Radio>,
        contact_radio: &ContactRadio,
        epoch: u64,
        key: &crate::radio::RadioKey,
    ) -> Result<TextMessage, CommandError> {
        let message_id = self
            .inner
            .messaging
            .send_text(&account.public_key, chat_id, text)
            .await
            .map_err(|error| failed("send the message", error))?;
        let Some(message) =
            self.inner
                .messaging
                .chat_message(&account.public_key, chat_id, &message_id)
        else {
            return Err(failed(
                "send the message",
                "MDK doesn't have the message after taking it",
            ));
        };
        let sent_at = u32::try_from(message.sent_at / 1000).unwrap_or(u32::MAX);
        // A text MDK tags, such as one with mentions, would get another id from a packet.
        if format::message_id(&account.public_key, sent_at, text) != message_id {
            log::info!("a message waits for the internet: a packet can't carry its id");
            return Ok(message);
        }
        // Stored as used before the packet goes out, so that it is never used twice.
        let counter = match self.inner.radio_store.take_counter(chat_id, epoch) {
            Ok(counter) => counter,
            Err(error) => {
                log::warn!("a message waits for the internet: {error}");
                return Ok(message);
            }
        };
        let packet = format::seal_text(key, epoch, counter, sent_at, text);
        match radio
            .send_text(&contact_radio.public_key, &format::to_text_message(&packet))
            .await
        {
            Ok(sent) => {
                let kept = self.inner.radio_store.add_message(RadioMessage {
                    chat_id: chat_id.to_owned(),
                    id: message_id,
                    outgoing: true,
                    text: text.to_owned(),
                    sent_at: message.sent_at,
                    ack: Some(sent.ack),
                    confirmed: false,
                    kept_at: super::now_ms(),
                });
                if let Err(error) = kept {
                    log::warn!("could not keep a message sent by radio: {error}");
                }
            }
            Err(error) => log::warn!("a message waits for the internet: the radio failed: {error}"),
        }
        Ok(message)
    }

    /// Whether a radio send is in flight in the chat `chat_id`.
    pub(super) fn radio_sending_in(&self, chat_id: &str) -> bool {
        self.radio_sending().contains(chat_id)
    }

    /// Takes each chat's current radio keys, where the chat is settled.
    pub(super) async fn keep_radio_keys(&self) {
        let chats: Vec<(String, String)> = self
            .state()
            .chats
            .iter()
            .map(|chat| (chat.id.clone(), chat.contact.public_key.clone()))
            .collect();
        for (chat_id, contact) in chats {
            self.keep_chat_keys(&chat_id, &contact).await;
        }
    }

    async fn keep_chat_keys(&self, chat_id: &str, contact: &str) {
        match self.radio_keys(chat_id).await {
            Ok(keys) => self.inbox().keep_keys(chat_id, contact, keys),
            Err(error) => log::info!("no new radio keys for a chat now: {}", error.message),
        }
    }

    async fn on_radio_event(&self, event: RadioEvent) {
        match event {
            RadioEvent::TextReceived { from, text, .. } => {
                self.receive_by_radio(from.as_deref(), &text).await;
            }
            RadioEvent::Delivered { ack } => self.radio_confirmed(&ack),
            // Reachability from heard radios is #44.
            RadioEvent::ContactHeard { .. } => {}
            RadioEvent::Disconnected => {
                if let Some(attached) = self.attached().as_mut() {
                    attached.connected = false;
                }
                self.radio_changed();
            }
        }
    }

    /// Takes a text message the radio received. Only a packet that passes the inbox is shown;
    /// anything else is dropped silently, and nothing is sent for it (#42).
    async fn receive_by_radio(&self, from_radio: Option<&str>, text: &str) {
        let Ok(packet) = format::from_text_message(text) else {
            // Ordinary text, most likely from someone on a stock MeshCore app.
            return;
        };
        let Some(account) = self.state().account.clone() else {
            return;
        };
        // The chat of the contact whose radio sent it, when the address book knows the radio.
        let from_chat = from_radio
            .and_then(|radio| self.inner.radio_store.contact_with_radio(radio))
            .and_then(|contact| self.chat_with(&contact))
            .map(|chat| chat.id);
        let mut opened = self.open_packet(&account, &packet, from_chat.as_deref());
        if matches!(
            opened,
            Err(Refusal::UnknownChat | Refusal::UnknownEpoch | Refusal::NotAuthentic)
        ) {
            // The chat may have moved to an epoch, or be new, since the keys were taken.
            self.keep_radio_keys().await;
            opened = self.open_packet(&account, &packet, from_chat.as_deref());
        }
        let received = match opened {
            Ok(received) => received,
            Err(refusal) => {
                log::info!("a radio packet was dropped: {refusal}");
                return;
            }
        };
        let message = TextMessage {
            chat_id: received.chat_id.clone(),
            id: received.message_id.clone(),
            outgoing: false,
            text: received.text.clone(),
            sent_at: u64::from(received.sent_at) * 1000,
            acknowledged: false,
        };
        let kept = self.inner.radio_store.add_message(RadioMessage {
            chat_id: message.chat_id.clone(),
            id: message.id.clone(),
            outgoing: false,
            text: message.text.clone(),
            sent_at: message.sent_at,
            ack: None,
            confirmed: false,
            kept_at: super::now_ms(),
        });
        if let Err(error) = kept {
            // Shown anyway; after a restart it is gone until its relay copy arrives.
            log::warn!("could not keep a message received by radio: {error}");
        }
        self.apply_message(&message);
    }

    fn open_packet(
        &self,
        account: &Account,
        packet: &[u8],
        from_chat: Option<&str>,
    ) -> Result<ReceivedText, Refusal> {
        let known = |chat_id: &str, id: &str| {
            self.inner
                .radio_store
                .message(id)
                .is_some_and(|message| message.chat_id == chat_id)
                || self
                    .inner
                    .messaging
                    .chat_message(&account.public_key, chat_id, id)
                    .is_some()
        };
        self.inbox().receive(packet, from_chat, known)
    }

    /// The other radio confirmed the message sent with `ack`.
    fn radio_confirmed(&self, ack: &str) {
        let confirmed = match self.inner.radio_store.confirm(ack) {
            Ok(Some(message)) => message,
            Ok(None) => return,
            Err(error) => {
                log::warn!("could not keep a radio confirmation: {error}");
                return;
            }
        };
        let Some(account) = self.state().account.clone() else {
            return;
        };
        if let Some(message) = self.inner.messaging.chat_message(
            &account.public_key,
            &confirmed.chat_id,
            &confirmed.id,
        ) {
            self.apply_message(&message);
        }
    }

    /// Stops following the radio, as when the core closes.
    pub(super) fn stop_radio(&self) {
        drop(self.attached().take());
    }

    /// Forgets the radio's keys and stops using it, as when the account is deleted.
    pub(super) fn forget_radio(&self) {
        self.stop_radio();
        self.inbox().clear();
        if let Err(error) = self.inner.radio_store.clear() {
            log::warn!("could not remove the radio store: {error}");
        }
    }

    fn attached(&self) -> MutexGuard<'_, Option<Attached>> {
        self.inner
            .radio
            .attached
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn inbox(&self) -> MutexGuard<'_, RadioInbox> {
        self.inner
            .radio
            .inbox
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn radio_sending(&self) -> MutexGuard<'_, HashSet<String>> {
        self.inner
            .radio
            .sending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Follows the radio's events, and keeps the chats' radio keys current while it does.
async fn watch_radio(core: Weak<Inner>, mut events: broadcast::Receiver<RadioEvent>) {
    let mut refresh = tokio::time::interval(KEY_REFRESH);
    refresh.tick().await;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) => {
                    let Some(core) = upgrade(&core) else { return };
                    core.on_radio_event(event).await;
                }
                Err(RecvError::Lagged(missed)) => log::warn!("missed {missed} radio events"),
                Err(RecvError::Closed) => return,
            },
            _ = refresh.tick() => {
                let Some(core) = upgrade(&core) else { return };
                core.keep_radio_keys().await;
            }
        }
    }
}

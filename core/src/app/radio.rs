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
//! - **Addresses (#34).** With a radio, the core shares its radio address record in each direct
//!   chat, and keeps the records its contacts share as their radios.
//!
//! The core uses the radio only while the switch that forces it is on; choosing the transport
//! by itself is #45.

use std::collections::HashSet;
use std::sync::{Arc, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use tokio::sync::broadcast::{self, error::RecvError};
use tokio::task::JoinHandle;

use super::{Core, Inner, failed, now_ms, upgrade};
use crate::interface::{Account, CommandError, CoreEvent, Mesh, Message, PairedRadio, RadioStatus};
use crate::messaging::{RadioAddressItem, TextMessage};
use crate::radio::format::{self, MAX_RADIO_TEXT_BYTES};
use crate::radio::{Radio, RadioAddress, RadioEvent, RadioInbox, RadioInfo, ReceivedText, Refusal};
use crate::radio_store::RadioMessage;
use crate::reach::Heard;

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
    /// When each radio was last heard (#44). Taken after `state` and `reach`.
    pub(super) heard: std::sync::Mutex<Heard>,
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
        let status = self.radio_changed();
        self.reach_changed();
        self.share_radio_address(None).await;
        Ok(status)
    }

    /// Stops using the radio. Emits [`CoreEvent::RadioChanged`] if there was one.
    pub fn detach_radio(&self) {
        let previous = self.attached().take();
        if previous.is_some() {
            drop(previous);
            self.radio_changed();
            self.reach_changed();
        }
    }

    /// How long a radio counts as heard after the last sign of it, in place of
    /// [`crate::reach::HEARD_WINDOW`]: for tests, and for tuning on hardware day (#44).
    pub fn set_radio_heard_window(&self, window: Duration) {
        self.heard().set_window(window);
        self.reach_changed();
    }

    /// The record of the radio the core uses, for comparing presets, while its link is open.
    pub(super) fn own_radio_record(&self) -> Option<RadioAddress> {
        self.attached()
            .as_ref()
            .filter(|attached| attached.connected)
            .map(|attached| RadioAddress::new(&attached.info, 0))
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

    /// Shares this phone's radio address record in the direct chat `only`, or in every direct
    /// chat, where it hasn't shared this record yet (#34). Without a radio it shares nothing. A
    /// chat it can't reach now gets it at the next try: when a chat starts, and every minute
    /// while there is a radio.
    pub(super) async fn share_radio_address(&self, only: Option<&str>) {
        let Some(info) = self
            .attached()
            .as_ref()
            .map(|attached| attached.info.clone())
        else {
            return;
        };
        let Some(account) = self.state().account.clone() else {
            return;
        };
        let record = match self.inner.radio_store.own_record(&info, now_ms()) {
            Ok(record) => record,
            Err(error) => {
                log::warn!("could not keep the radio's address record: {error}");
                return;
            }
        };
        let chats: Vec<String> = self
            .state()
            .chats
            .iter()
            .map(|chat| chat.id.clone())
            .filter(|chat_id| only.is_none_or(|only| only == chat_id))
            .collect();
        for chat_id in chats {
            if self.inner.radio_store.is_shared(&chat_id, &record) {
                continue;
            }
            let sent = self
                .inner
                .messaging
                .send_radio_address(&account.public_key, &chat_id, &record.to_json())
                .await;
            match sent {
                Ok(()) => {
                    if let Err(error) = self.inner.radio_store.set_shared(&chat_id, record.clone())
                    {
                        log::warn!("could not note a shared radio address: {error}");
                    }
                }
                Err(error) => log::info!("a radio address waits for the next try: {error}"),
            }
        }
    }

    /// Takes a radio address record that came in a chat. A newer record of the chat's contact
    /// becomes their radio; anything else changes nothing. Records in a chat that is still a
    /// request are read once it is accepted.
    pub(super) fn radio_address_received(&self, item: &RadioAddressItem) {
        let record = match RadioAddress::parse(&item.record) {
            Ok(record) => record,
            Err(error) => {
                log::info!("a radio address record was refused: {error}");
                return;
            }
        };
        let Some(contact) = self.chat(&item.chat_id).map(|chat| chat.contact.public_key) else {
            return;
        };
        match self.inner.radio_store.set_contact_radio(&contact, record) {
            Ok(true) => {
                self.contact_meshes_changed(&contact);
                self.reach_changed();
            }
            Ok(false) => {}
            Err(error) => log::warn!("could not keep a contact's radio: {error}"),
        }
    }

    /// Reads the radio address records the contact sent in the chat `chat_id`, for those that
    /// came while the app was closed or the chat was a request.
    pub(super) fn read_radio_addresses(&self, chat_id: &str) {
        let Some(account) = self.state().account.clone() else {
            return;
        };
        match self
            .inner
            .messaging
            .radio_addresses(&account.public_key, chat_id)
        {
            Ok(items) => {
                for item in items {
                    self.radio_address_received(&item);
                }
            }
            Err(error) => log::info!("could not read a chat's radio addresses: {error}"),
        }
    }

    /// A chat is new to the snapshot: the contact's records go into the address book, and the
    /// chat gets this phone's record if it has a radio.
    pub(super) fn radio_chat_added(&self, chat_id: &str) {
        self.read_radio_addresses(chat_id);
        self.reach_changed();
        if self.attached().is_some() {
            let core = self.clone();
            let chat_id = chat_id.to_owned();
            tokio::spawn(async move { core.share_radio_address(Some(&chat_id)).await });
        }
    }

    /// The meshes the contact's radios run, as their records say.
    pub(super) fn contact_meshes(&self, contact: &str) -> Vec<Mesh> {
        self.inner
            .radio_store
            .contact_radio(contact)
            .map(|_| vec![Mesh::MeshCore])
            .unwrap_or_default()
    }

    /// Gives the contact's chats and invitations their meshes, and emits
    /// [`CoreEvent::ContactChanged`] if that changed anything.
    fn contact_meshes_changed(&self, contact: &str) {
        let meshes = self.contact_meshes(contact);
        let changed = {
            let mut state = self.state();
            let state = &mut *state;
            let mut changed = None;
            let contacts = state.chats.iter_mut().map(|chat| &mut chat.contact).chain(
                state
                    .invitations
                    .iter_mut()
                    .map(|invitation| &mut invitation.from),
            );
            for known in contacts.filter(|known| known.public_key == contact) {
                if known.meshes != meshes {
                    known.meshes = meshes.clone();
                    changed = Some(known.clone());
                }
            }
            changed
        };
        if let Some(contact) = changed {
            self.emit(CoreEvent::ContactChanged { contact });
        }
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
        contact_radio: &RadioAddress,
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
            .send_text(&contact_radio.key, &format::to_text_message(&packet))
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
                if let Some(radio) = &from {
                    self.hear(radio);
                }
                self.receive_by_radio(from.as_deref(), &text).await;
            }
            RadioEvent::Delivered { ack } => self.radio_confirmed(&ack),
            RadioEvent::ContactHeard { public_key } => self.hear(&public_key),
            RadioEvent::Disconnected => {
                if let Some(attached) = self.attached().as_mut() {
                    attached.connected = false;
                }
                self.radio_changed();
                self.reach_changed();
            }
        }
    }

    /// The radio `radio` was heard just now (#44).
    fn hear(&self, radio: &str) {
        self.heard().hear(radio, tokio::time::Instant::now());
        self.reach_changed();
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
        // The contact's radio confirmed it, so it was heard.
        let radio = self.chat(&confirmed.chat_id).and_then(|chat| {
            self.inner
                .radio_store
                .contact_radio(&chat.contact.public_key)
        });
        if let Some(radio) = radio {
            self.hear(&radio.key);
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
        self.heard().clear();
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

    pub(super) fn heard(&self) -> MutexGuard<'_, Heard> {
        self.inner
            .radio
            .heard
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
                core.share_radio_address(None).await;
            }
        }
    }
}

//! Chat text over radio (#40): sending, receiving and confirming through the radio the core
//! uses.
//!
//! - **Sending.** MDK gets the message first. It keeps it and publishes it once a relay can be
//!   reached: that copy is the permanent record (#47). The radio carries a packet with the same
//!   text and time, so the other phone rebuilds the same id (`docs/radio-format-v0.md`).
//! - **Receiving.** A packet goes through the [`RadioInbox`] (#42). What passes it is kept in the
//!   radio store and shown, marked as radio. When the relay copy arrives later, the chat keeps one
//!   message.
//! - **Confirming (#41).** When the other radio confirms a message, it shows as confirmed. With
//!   no confirmation in time, the message goes once more, while the chat is still reachable by
//!   radio and the hour's airtime allows; with none after that either, it shows as timed out.
//!   Its relay copy goes as for any radio message, so it still arrives once a relay is back.
//! - **Addresses (#34).** With a radio, the core shares its radio address record in each direct
//!   chat, and keeps the records its contacts share as their radios.
//!
//! Pairing a radio, and opening it again after its link drops, is in `pairing.rs` (#24).

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use tokio::sync::broadcast::{self, error::RecvError};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::{Core, Inner, failed, now_ms, upgrade};
use crate::interface::{
    Account, CommandError, CoreEvent, Mesh, Message, PairedRadio, RadioConnection, RadioIssue,
    RadioPreset, RadioStatus, Reach,
};
use crate::messaging::{RadioAddressItem, TextMessage};
use crate::radio::airtime;
use crate::radio::format::{self, MAX_RADIO_TEXT_BYTES};
use crate::radio::{
    EXPECTED_PRESET, Radio, RadioAddress, RadioEvent, RadioInbox, RadioInfo, RadioPorts,
    ReceivedText, Refusal,
};
use crate::radio_store::{RadioMessage, RadioStore, Transmission};
use crate::reach::{Heard, radio_reach};

/// How often the core takes each chat's current radio keys while it has a radio. Keys can't be
/// had while a chat waits for a relay to confirm a change, so the newest are kept beforehand.
const KEY_REFRESH: Duration = Duration::from_secs(60);
/// What the core adds to the radio's own estimate of how long a confirmation may take: the
/// confirmation still has to come over the link from the radio (#41). Provisional, until
/// hardware day measures it (#36).
const CONFIRM_MARGIN: Duration = Duration::from_secs(1);
/// How many times the core sends a radio message again when no confirmation comes: once, as
/// each try costs shared airtime (#41).
const RADIO_RETRIES: u32 = 1;
/// How many confirmations the core keeps for messages it hasn't kept yet.
const EARLY_CONFIRMATIONS: usize = 16;

/// The radio the core uses, and the watcher that follows its events.
pub(super) struct Attached {
    pub(super) radio: Arc<dyn Radio>,
    pub(super) info: RadioInfo,
    pub(super) connected: bool,
    watcher: JoinHandle<()>,
}

impl Drop for Attached {
    fn drop(&mut self) {
        self.watcher.abort();
    }
}

/// What a radio send needs: the radio, the contact's radio, and the chat's newest keys.
pub(super) struct RadioReady {
    radio: Arc<dyn Radio>,
    contact_radio: RadioAddress,
    epoch: u64,
    key: crate::radio::RadioKey,
    /// The packet's size and time on air (#51).
    bytes: usize,
    airtime: Duration,
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
    /// The airtime the app may use in an hour, when a test sets it (#51).
    pub(super) airtime_cap: std::sync::Mutex<Option<Duration>>,
    /// How long to wait for the other radio's confirmation, when a test sets it (#41).
    pub(super) confirm_wait: std::sync::Mutex<Option<Duration>>,
    /// Confirmations of no message the store holds: the radio can confirm a message before
    /// `send_text` returns and the message is kept. Held while keeping a sent message or a
    /// confirmation, so that each sees the other.
    pub(super) early_confirmations: std::sync::Mutex<VecDeque<String>>,
    /// Where radios are plugged in, on a device that pairs them over a cable (#24).
    pub(super) ports: std::sync::Mutex<Option<Arc<dyn RadioPorts>>>,
    /// Looks for the paired radio while its link is down (#24).
    pub(super) reconnect: std::sync::Mutex<Option<JoinHandle<()>>>,
    /// How long to wait between looks, when a test sets it (#24).
    pub(super) reconnect_wait: std::sync::Mutex<Option<Duration>>,
    /// Pairing, forgetting and opening the paired radio again happen one at a time (#24).
    pub(super) pairing: tokio::sync::Mutex<()>,
}

/// A radio message whose confirmation the core awaits (#41).
struct Awaited {
    id: String,
    chat_id: String,
    /// How long until its confirmation is due.
    wait: Duration,
    /// What a retry needs. None after a restart: retries don't outlive the app.
    retry: Option<Retry>,
}

struct Retry {
    /// The text message that carried the packet: a retry sends it as it was, so a copy that got
    /// through already is dropped as a replay on the other phone (#42).
    text: String,
    bytes: usize,
    airtime: Duration,
}

impl Core {
    /// Uses `radio` for radio messages from now on, in place of any radio used before. Emits
    /// [`CoreEvent::RadioChanged`].
    ///
    /// Pairing (#24) opens a radio and hands it over here. Tests hand over mock radios directly
    /// too; those aren't remembered.
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

    /// The radio as an interface shows it: the one in use, or else the paired one while the core
    /// looks for it (#24).
    pub(super) fn radio_status(&self) -> RadioStatus {
        let pairing = self.inner.radio_store.pairing();
        let looking = self.reconnecting();
        let can_pair = self.radio_ports_slot().is_some();
        let unless_connected = if looking {
            RadioConnection::Connecting
        } else {
            RadioConnection::Disconnected
        };
        let paired = match self.attached().as_ref() {
            Some(attached) => {
                let record = RadioAddress::new(&attached.info, 0);
                Some(PairedRadio {
                    name: attached.info.name.clone(),
                    public_key: attached.info.public_key.clone(),
                    connection: if attached.connected {
                        RadioConnection::Connected
                    } else {
                        unless_connected
                    },
                    port: pairing
                        .filter(|pairing| pairing.radio.key == record.key)
                        .map(|pairing| pairing.port),
                    preset: preset_view(&record),
                })
            }
            None => pairing.map(|pairing| PairedRadio {
                name: pairing.name,
                public_key: pairing.radio.key.clone(),
                connection: unless_connected,
                port: Some(pairing.port),
                preset: preset_view(&pairing.radio),
            }),
        };
        RadioStatus {
            paired,
            forced: self.inner.radio_store.forced(),
            can_pair,
        }
    }

    /// Puts the radio's status into the snapshot, and emits it if it changed.
    pub(super) fn radio_changed(&self) -> RadioStatus {
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
        ready: RadioReady,
    ) -> Result<Message, CommandError> {
        self.radio_sending().insert(chat_id.to_owned());
        let sent = self.radio_send(account, chat_id, text, ready).await;
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

    /// What a radio send needs, or why the radio can't take `text` in the chat `chat_id`.
    pub(super) async fn radio_ready(
        &self,
        chat_id: &str,
        text: &str,
    ) -> Result<RadioReady, CommandError> {
        if !format::fits(text) {
            return Err(CommandError::new(format!(
                "That is too long for radio. Keep it to {MAX_RADIO_TEXT_BYTES} bytes."
            )));
        }
        let chat = self
            .chat(chat_id)
            .ok_or_else(|| CommandError::new("This chat no longer exists."))?;
        let in_use = self
            .attached()
            .as_ref()
            .map(|a| (Arc::clone(&a.radio), a.connected));
        let radio = match in_use {
            Some((radio, true)) => radio,
            // Paired, but its link is down: the core is looking for it (#24).
            Some((_, false)) => return Err(CommandError::new("Your radio isn't connected.")),
            None if self.inner.radio_store.pairing().is_some() => {
                return Err(CommandError::new("Your radio isn't connected."));
            }
            None => return Err(CommandError::new("Pair a radio first.")),
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
        // Within the hour's airtime (#51).
        let Some(settings) = self.attached().as_ref().map(|a| a.info.settings) else {
            return Err(CommandError::new("Pair a radio first."));
        };
        let packet = format::OVERHEAD_BYTES + text.len();
        let bytes = airtime::meshcore_text_bytes((4 * packet).div_ceil(3));
        let airtime = airtime::time_on_air(&settings, bytes);
        let (used, _) = self.inner.radio_store.airtime_used(now_ms());
        if Duration::from_millis(used) + airtime > self.airtime_cap() {
            return Err(CommandError::new(
                "Your radio has used its airtime for this hour.",
            ));
        }
        Ok(RadioReady {
            radio,
            contact_radio,
            epoch,
            key,
            bytes,
            airtime,
        })
    }

    /// Sets the airtime the app may use in an hour, in place of
    /// [`airtime::HOURLY_AIRTIME`]: for tests (#51).
    pub fn set_radio_hourly_airtime(&self, cap: Duration) {
        *self
            .inner
            .radio
            .airtime_cap
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(cap);
        self.reach_changed();
    }

    fn airtime_cap(&self) -> Duration {
        self.inner
            .radio
            .airtime_cap
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .unwrap_or(airtime::HOURLY_AIRTIME)
    }

    /// How long the core waits for the other radio to confirm each try, in place of the radio's
    /// own estimate: for tests, and for tuning on hardware day (#41).
    pub fn set_radio_confirm_wait(&self, wait: Duration) {
        *self
            .inner
            .radio
            .confirm_wait
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(wait);
    }

    /// How long to wait for a confirmation the radio estimates may take `estimate`.
    fn confirm_wait(&self, estimate: Duration) -> Duration {
        self.inner
            .radio
            .confirm_wait
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .unwrap_or(estimate + CONFIRM_MARGIN)
    }

    /// Whether the radio has airtime left this hour for the shortest message, and if not, when
    /// some comes free, in milliseconds since the Unix epoch (#51).
    pub(super) fn airtime_left(&self) -> (bool, Option<u64>) {
        let Some(settings) = self.attached().as_ref().map(|a| a.info.settings) else {
            return (true, None);
        };
        let shortest = airtime::time_on_air(
            &settings,
            airtime::meshcore_text_bytes((4 * (format::OVERHEAD_BYTES + 1)).div_ceil(3)),
        );
        let (used, frees_at) = self.inner.radio_store.airtime_used(now_ms());
        if Duration::from_millis(used) + shortest <= self.airtime_cap() {
            (true, None)
        } else {
            (false, frees_at)
        }
    }

    /// Every transmission the app caused in the last two days, oldest first (#52).
    pub fn radio_transmissions(&self) -> Vec<Transmission> {
        self.inner.radio_store.transmissions_since(0)
    }

    /// Whether the radio of the contact `contact` can be reached now, or why not (#44).
    pub(super) fn contact_radio_reach(&self, contact: &str) -> Result<(), RadioIssue> {
        let own = self.own_radio_record();
        let contact = self.inner.radio_store.contact_radio(contact);
        let (airtime_left, _) = self.airtime_left();
        radio_reach(
            own.as_ref(),
            contact.as_ref(),
            &self.heard(),
            airtime_left,
            tokio::time::Instant::now(),
        )
    }

    /// Hands `text` to MDK, then to the radio. Returns the message as MDK keeps it.
    async fn radio_send(
        &self,
        account: &Account,
        chat_id: &str,
        text: &str,
        ready: RadioReady,
    ) -> Result<TextMessage, CommandError> {
        let RadioReady {
            radio,
            contact_radio,
            epoch,
            key,
            bytes,
            airtime,
        } = ready;
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
        let packet =
            format::to_text_message(&format::seal_text(&key, epoch, counter, sent_at, text));
        match radio.send_text(&contact_radio.key, &packet).await {
            Ok(sent) => {
                // Logged with its airtime: the hourly cap counts it, and the log shows that only
                // what the user sent went out (#51, #52).
                self.log_radio_transmission(bytes, airtime, "message");
                let wait = self.confirm_wait(sent.timeout);
                let kept = RadioMessage {
                    chat_id: chat_id.to_owned(),
                    id: message_id.clone(),
                    outgoing: true,
                    text: text.to_owned(),
                    sent_at: message.sent_at,
                    ack: Some(sent.ack.clone()),
                    retry_acks: Vec::new(),
                    confirmed: false,
                    confirm_by: Some(due(wait)),
                    timed_out: false,
                    kept_at: now_ms(),
                };
                self.keep_sent(&sent.ack, |store| store.add_message(kept).map(|_| ()));
                self.await_confirmation(Awaited {
                    id: message_id,
                    chat_id: chat_id.to_owned(),
                    wait,
                    retry: Some(Retry {
                        text: packet,
                        bytes,
                        airtime,
                    }),
                });
            }
            Err(error) => log::warn!("a message waits for the internet: the radio failed: {error}"),
        }
        Ok(message)
    }

    /// Logs a transmission the app caused (#52), with its airtime for the hourly cap (#51).
    fn log_radio_transmission(&self, bytes: usize, airtime: Duration, reason: &str) {
        let now = now_ms();
        let logged = self.inner.radio_store.log_transmission(
            Transmission {
                at: now,
                bytes,
                airtime_ms: airtime.as_millis().try_into().unwrap_or(u64::MAX),
                reason: reason.to_owned(),
            },
            now,
        );
        if let Err(error) = logged {
            log::warn!("could not log a radio transmission: {error}");
        }
    }

    /// Keeps what was sent with the confirmation code `ack`, by `keep`, then takes a
    /// confirmation that came for it already.
    fn keep_sent(&self, ack: &str, keep: impl FnOnce(&RadioStore) -> std::io::Result<()>) {
        let early = {
            let mut early = self.early_confirmations();
            if let Err(error) = keep(&self.inner.radio_store) {
                log::warn!("could not keep a message sent by radio: {error}");
            }
            early
                .iter()
                .position(|code| code == ack)
                .and_then(|at| early.remove(at))
        };
        if early.is_some() {
            self.radio_confirmed(ack);
        }
    }

    /// Waits for the confirmation of `awaited` (#41).
    fn await_confirmation(&self, awaited: Awaited) {
        tokio::spawn(await_confirmation(Arc::downgrade(&self.inner), awaited));
    }

    /// After a restart: waits for the confirmations still due, without retries.
    pub(super) fn await_after_restart(&self, awaited: Vec<RadioMessage>) {
        let now = now_ms();
        for message in awaited {
            let due = message.confirm_by.unwrap_or(now);
            self.await_confirmation(Awaited {
                id: message.id,
                chat_id: message.chat_id,
                wait: Duration::from_millis(due.saturating_sub(now)),
                retry: None,
            });
        }
    }

    /// Sends the message of `awaited` again, while the chat is still reachable by radio, the
    /// internet doesn't carry it instead, and the hour's airtime allows. Returns how long to wait
    /// for this try's confirmation.
    async fn retry_by_radio(&self, awaited: &Awaited, retry: &Retry) -> Option<Duration> {
        let chat = self.chat(&awaited.chat_id)?;
        if let Err(issue) = self.contact_radio_reach(&chat.contact.public_key) {
            log::info!("no radio retry: {issue:?}");
            return None;
        }
        // By the rules of #45: once a relay is back, the internet carries the message.
        let relays_up = self.reach().reach(&awaited.chat_id, Instant::now()) == Reach::Internet;
        if relays_up && !self.inner.radio_store.forced() {
            return None;
        }
        let (used, _) = self.inner.radio_store.airtime_used(now_ms());
        if Duration::from_millis(used) + retry.airtime > self.airtime_cap() {
            log::info!("no radio retry: the hour's airtime is used");
            return None;
        }
        let radio = self
            .attached()
            .as_ref()
            .map(|attached| Arc::clone(&attached.radio))?;
        let contact_radio = self
            .inner
            .radio_store
            .contact_radio(&chat.contact.public_key)?;
        let sent = match radio.send_text(&contact_radio.key, &retry.text).await {
            Ok(sent) => sent,
            Err(error) => {
                log::warn!("a radio retry failed: {error}");
                return None;
            }
        };
        self.log_radio_transmission(retry.bytes, retry.airtime, "retry");
        let wait = self.confirm_wait(sent.timeout);
        let ack = sent.ack.clone();
        self.keep_sent(&sent.ack, |store| {
            store.retried(&awaited.id, ack, due(wait)).map(|_| ())
        });
        Some(wait)
    }

    /// No confirmation came in time for the message `id`, after any retries.
    fn radio_timed_out(&self, id: &str) {
        let timed_out = match self.inner.radio_store.time_out(id) {
            Ok(Some(message)) => message,
            Ok(None) => return,
            Err(error) => {
                log::warn!("could not note a radio message that timed out: {error}");
                return;
            }
        };
        log::info!("a radio message timed out");
        self.show_sent(&timed_out);
    }

    /// Shows the sent radio message `radio` as the store has it now.
    fn show_sent(&self, radio: &RadioMessage) {
        let Some(account) = self.state().account.clone() else {
            return;
        };
        if let Some(message) =
            self.inner
                .messaging
                .chat_message(&account.public_key, &radio.chat_id, &radio.id)
        {
            self.apply_message(&message);
        }
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
                let key = self.attached().as_mut().map(|attached| {
                    attached.connected = false;
                    attached.info.public_key.to_ascii_lowercase()
                });
                // The paired radio restarted, or its cable was pulled: look for it (#24).
                let paired = self.inner.radio_store.pairing().map(|p| p.radio.key);
                if key.is_some() && key == paired {
                    self.paired_radio_lost();
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
            retry_acks: Vec::new(),
            confirmed: false,
            confirm_by: None,
            timed_out: false,
            kept_at: now_ms(),
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

    /// The other radio confirmed the message sent with `ack`: its first try or a retry, also
    /// after it timed out.
    fn radio_confirmed(&self, ack: &str) {
        let confirmed = {
            let mut early = self.early_confirmations();
            match self.inner.radio_store.confirm(ack) {
                Ok(Some(message)) => message,
                Ok(None) => {
                    // The radio may confirm before the message is kept: `keep_sent` looks here.
                    if early.len() >= EARLY_CONFIRMATIONS {
                        early.pop_front();
                    }
                    early.push_back(ack.to_owned());
                    return;
                }
                Err(error) => {
                    log::warn!("could not keep a radio confirmation: {error}");
                    return;
                }
            }
        };
        self.show_sent(&confirmed);
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

    /// Stops following the radio and looking for it, as when the core closes.
    pub(super) fn stop_radio(&self) {
        self.stop_reconnect();
        drop(self.attached().take());
    }

    /// Forgets the radio's keys and the paired radio, and stops using it, as when the account is
    /// deleted.
    pub(super) fn clear_radio(&self) {
        self.stop_radio();
        self.inbox().clear();
        self.heard().clear();
        self.early_confirmations().clear();
        if let Err(error) = self.inner.radio_store.clear() {
            log::warn!("could not remove the radio store: {error}");
        }
    }

    pub(super) fn attached(&self) -> MutexGuard<'_, Option<Attached>> {
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

    fn early_confirmations(&self) -> MutexGuard<'_, VecDeque<String>> {
        self.inner
            .radio
            .early_confirmations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// A radio's settings as an interface shows them, and whether they are the preset the app
/// expects (#24).
fn preset_view(record: &RadioAddress) -> RadioPreset {
    RadioPreset {
        frequency_khz: record.freq,
        bandwidth_hz: record.bw,
        spreading_factor: record.sf,
        coding_rate: record.cr,
        expected: record.preset() == EXPECTED_PRESET,
    }
}

/// When a confirmation awaited for `wait` from now is due, in milliseconds since the Unix epoch.
fn due(wait: Duration) -> u64 {
    now_ms().saturating_add(wait.as_millis().try_into().unwrap_or(u64::MAX))
}

/// Waits for the confirmation of `awaited`. Without one in time, sends the message again, up
/// to [`RADIO_RETRIES`] times while it may, then marks it timed out (#41).
async fn await_confirmation(core: Weak<Inner>, awaited: Awaited) {
    let mut wait = awaited.wait;
    let mut retries = 0;
    loop {
        tokio::time::sleep(wait).await;
        let Some(core) = upgrade(&core) else { return };
        let awaiting = core
            .inner
            .radio_store
            .message(&awaited.id)
            .is_some_and(|message| !message.confirmed && !message.timed_out);
        if !awaiting {
            return;
        }
        if retries < RADIO_RETRIES
            && let Some(retry) = &awaited.retry
            && let Some(next) = core.retry_by_radio(&awaited, retry).await
        {
            retries += 1;
            wait = next;
            continue;
        }
        core.radio_timed_out(&awaited.id);
        return;
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

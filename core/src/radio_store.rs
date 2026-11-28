//! What the core keeps about the radio (#40), in `radio.json` in the data folder:
//!
//! - **The paired radio** (#24): which radio it is and where it was plugged in, to open it again.
//! - **The switch that forces radio**, for testing until the core chooses the transport (#45).
//! - **The counters** for what this phone sends: each chat's next counter in each epoch. A
//!   counter is stored before its packet goes out, so that it is never used twice with one key
//!   (`docs/radio-format-v0.md`).
//! - **The contacts' radios:** the address book, which their radio address records fill (#34),
//!   and which record this phone shared in each chat.
//! - **Every message that travelled by radio,** sent or received, with its id and chat. The ids
//!   catch a replayed packet after a restart (#42). A sent one keeps the codes its confirmation
//!   may carry, and whether one came in time (#41). A received one is in no other store until its
//!   relay copy arrives, so until then it keeps its text, encrypted (#127). A sent one never
//!   does: MDK has it from the start.
//! - **Every transmission the app caused** in the last two days, with its airtime and reason: the
//!   hourly airtime cap counts them (#51), and they show that the app sends nothing by itself
//!   (#52).
//!
//! The file is small, written whole on every change, and readable by its owner only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

use crate::private_file;
use crate::radio::RadioAddress;
use crate::vault::Sealed;

/// A message that travelled by radio.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RadioMessage {
    pub chat_id: String,
    /// MDK's id for the message, which its relay copy has too.
    pub id: String,
    pub outgoing: bool,
    /// Incoming, while MDK doesn't have the message: its text, encrypted (#127).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed: Option<Sealed>,
    /// The text of a message kept before #127, until it is sealed or dropped.
    #[serde(default, rename = "text", skip_serializing_if = "Option::is_none")]
    pub plain: Option<String>,
    /// While the text is kept, in a chat whose messages disappear: when this one does, in
    /// milliseconds since the Unix epoch (#124).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// When the sender sent it, in milliseconds since the Unix epoch (whole seconds).
    pub sent_at: u64,
    /// Outgoing: the code that the other radio's confirmation carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack: Option<String>,
    /// Outgoing: the codes of the retries' confirmations. Each try has its own (#41).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retry_acks: Vec<String>,
    /// Outgoing: the other radio confirmed it.
    #[serde(default)]
    pub confirmed: bool,
    /// Outgoing, while the other radio's confirmation is awaited: when it is due, in
    /// milliseconds since the Unix epoch (#41).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_by: Option<u64>,
    /// Outgoing: no confirmation came in time, also after a retry (#41). One that comes later
    /// still counts.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub timed_out: bool,
    /// When this phone kept it, in milliseconds since the Unix epoch. It orders messages sent
    /// within the same second.
    #[serde(default)]
    pub kept_at: u64,
}

impl RadioMessage {
    /// Whether the store still keeps the message's text.
    pub fn holds_text(&self) -> bool {
        self.sealed.is_some() || self.plain.is_some()
    }
}

/// The radio paired over a cable (#24).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pairing {
    /// The port the radio was on last.
    pub port: String,
    /// The USB device's serial number, which goes with the radio when it is plugged in
    /// elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_number: Option<String>,
    /// The name the radio announced last.
    pub name: String,
    /// Which radio it is, and its settings when it was last connected.
    pub radio: RadioAddress,
}

/// A transmission the app caused (#52).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transmission {
    /// Milliseconds since the Unix epoch.
    pub at: u64,
    /// The packet's size on air, in bytes.
    pub bytes: usize,
    /// Its time on air, in milliseconds.
    pub airtime_ms: u64,
    /// Why the app sent it: `message` for a message the user sent, or one that waited; `retry`
    /// for one sent again because no confirmation came (#41).
    pub reason: String,
}

/// How long the transmission log keeps an entry: long enough for a 24-hour idle run (#52).
const KEEP_TRANSMISSIONS_MS: u64 = 48 * 60 * 60 * 1000;
/// The window the airtime cap counts over (#51).
pub const AIRTIME_WINDOW_MS: u64 = 60 * 60 * 1000;

/// Why no counter can be had.
#[derive(Debug, thiserror::Error)]
pub enum CounterError {
    /// The file was damaged, so the counters it held are lost. The chat sends again from its
    /// next epoch, whose key has never been used.
    #[error("the radio counters were lost; the chat sends again from epoch {0}")]
    Lost(u64),
    #[error("could not store the counter: {0}")]
    Store(#[from] std::io::Error),
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Data {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pairing: Option<Pairing>,
    #[serde(default)]
    forced: bool,
    /// Chat → epoch → the next counter.
    #[serde(default)]
    counters: BTreeMap<String, BTreeMap<u64, u64>>,
    /// Set once a damaged file was found: a chat may then send only in epochs from
    /// `send_from`, which it gets on its first try.
    #[serde(default)]
    counters_lost: bool,
    #[serde(default)]
    send_from: BTreeMap<String, u64>,
    /// Contact's account key → their radio's newest record.
    #[serde(default)]
    contacts: BTreeMap<String, RadioAddress>,
    /// This phone's record for the radio it uses: kept, so that the record of the same radio
    /// keeps the time it was first made.
    #[serde(default)]
    own: Option<RadioAddress>,
    /// Chat → the record of this phone's radio that was shared in it last.
    #[serde(default)]
    shared: BTreeMap<String, RadioAddress>,
    /// Oldest first.
    #[serde(default)]
    messages: Vec<RadioMessage>,
    /// Oldest first, for the last two days.
    #[serde(default)]
    transmissions: Vec<Transmission>,
}

pub struct RadioStore {
    path: PathBuf,
    data: Mutex<Data>,
}

impl RadioStore {
    /// The store in `data_dir`. A damaged file is kept aside as `radio.json.damaged`, unless it
    /// holds message text from before it was encrypted (#127): then it is removed. What it held
    /// is lost, and so are the counters: no chat sends again in an epoch it may have sent in
    /// before.
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("radio.json");
        let aside = path.with_extension("json.damaged");
        // Left over from a crash.
        if let Err(error) = private_file::remove(&path.with_extension("json.tmp")) {
            log::warn!("could not remove a leftover of the radio store: {error}");
        }
        private_file::restrict(&path);
        if std::fs::read(&aside).is_ok_and(|bytes| has_plain_text(&bytes)) {
            if let Err(error) = private_file::remove(&aside) {
                log::warn!("could not remove an old damaged radio store: {error}");
            }
        } else {
            private_file::restrict(&aside);
        }
        let data = match std::fs::read(&path) {
            Ok(json) => serde_json::from_slice(&json).unwrap_or_else(|error| {
                log::error!("the radio store is damaged, and its counters are lost: {error}");
                let put_away = if has_plain_text(&json) {
                    private_file::remove(&path)
                } else {
                    std::fs::rename(&path, &aside)
                };
                if let Err(error) = put_away {
                    log::warn!("could not put the damaged radio store away: {error}");
                }
                Data {
                    counters_lost: true,
                    ..Data::default()
                }
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Data::default(),
            Err(error) => {
                // Unreadable is as bad as damaged for the counters.
                log::error!("could not read the radio store, and its counters are lost: {error}");
                Data {
                    counters_lost: true,
                    ..Data::default()
                }
            }
        };
        let lost = data.counters_lost;
        let store = Self {
            path,
            data: Mutex::new(data),
        };
        if lost && let Err(error) = store.save(&store.lock()) {
            log::warn!("could not note that the radio counters are lost: {error}");
        }
        store
    }

    /// The radio paired over a cable, if any (#24).
    pub fn pairing(&self) -> Option<Pairing> {
        self.lock().pairing.clone()
    }

    /// Remembers `pairing` as the paired radio, or forgets it with `None`.
    pub fn set_pairing(&self, pairing: Option<Pairing>) -> std::io::Result<()> {
        let mut data = self.lock();
        if data.pairing == pairing {
            return Ok(());
        }
        data.pairing = pairing;
        self.save(&data)
    }

    pub fn forced(&self) -> bool {
        self.lock().forced
    }

    pub fn set_forced(&self, forced: bool) -> std::io::Result<()> {
        let mut data = self.lock();
        data.forced = forced;
        self.save(&data)
    }

    /// The counter for the next packet in the chat `chat_id` in `epoch`, stored as used before
    /// this returns.
    pub fn take_counter(&self, chat_id: &str, epoch: u64) -> Result<u64, CounterError> {
        let mut data = self.lock();
        if data.counters_lost {
            let from = *data
                .send_from
                .entry(chat_id.to_owned())
                .or_insert(epoch + 1);
            if epoch < from {
                self.save(&data)?;
                return Err(CounterError::Lost(from));
            }
        }
        let next = data
            .counters
            .entry(chat_id.to_owned())
            .or_default()
            .entry(epoch)
            .or_insert(0);
        let counter = *next;
        *next += 1;
        self.save(&data)?;
        Ok(counter)
    }

    /// The radio of the contact whose account key is `contact`.
    pub fn contact_radio(&self, contact: &str) -> Option<RadioAddress> {
        self.lock().contacts.get(contact).cloned()
    }

    /// The contact whose radio has the public key `radio`.
    pub fn contact_with_radio(&self, radio: &str) -> Option<String> {
        self.lock()
            .contacts
            .iter()
            .find(|(_, known)| known.key == radio)
            .map(|(contact, _)| contact.clone())
    }

    /// Keeps `record` as the radio of the contact `contact`, unless the contact has a newer
    /// record, or this one already. Returns whether it was kept. Of two records made at the same
    /// time, the one that came last counts.
    pub fn set_contact_radio(&self, contact: &str, record: RadioAddress) -> std::io::Result<bool> {
        let mut data = self.lock();
        if data
            .contacts
            .get(contact)
            .is_some_and(|known| known.at > record.at || *known == record)
        {
            return Ok(false);
        }
        data.contacts.insert(contact.to_owned(), record);
        self.save(&data)?;
        Ok(true)
    }

    /// This phone's record for the radio `info` describes: the one kept, if it is for the same
    /// radio with the same settings, or a new one made at `now`.
    pub fn own_record(
        &self,
        info: &crate::radio::RadioInfo,
        now: u64,
    ) -> std::io::Result<RadioAddress> {
        let mut data = self.lock();
        let current = RadioAddress::new(info, now);
        if let Some(own) = &data.own
            && own.same_radio(&current)
        {
            return Ok(own.clone());
        }
        data.own = Some(current.clone());
        self.save(&data)?;
        Ok(current)
    }

    /// Whether `record` is what was shared in the chat `chat_id` last.
    pub fn is_shared(&self, chat_id: &str, record: &RadioAddress) -> bool {
        self.lock().shared.get(chat_id) == Some(record)
    }

    pub fn set_shared(&self, chat_id: &str, record: RadioAddress) -> std::io::Result<()> {
        let mut data = self.lock();
        data.shared.insert(chat_id.to_owned(), record);
        self.save(&data)
    }

    /// Keeps `message`. Returns false, and keeps nothing, if a message with its id is kept
    /// already.
    pub fn add_message(&self, message: RadioMessage) -> std::io::Result<bool> {
        let mut data = self.lock();
        if data.messages.iter().any(|known| known.id == message.id) {
            return Ok(false);
        }
        data.messages.push(message);
        self.save(&data)?;
        Ok(true)
    }

    pub fn message(&self, id: &str) -> Option<RadioMessage> {
        self.lock()
            .messages
            .iter()
            .find(|known| known.id == id)
            .cloned()
    }

    /// The messages whose text is still kept.
    pub fn messages_with_text(&self) -> Vec<RadioMessage> {
        self.lock()
            .messages
            .iter()
            .filter(|message| message.holds_text())
            .cloned()
            .collect()
    }

    /// Drops the text of the message `id`, and keeps what replay checks and confirmations need
    /// (#127). Returns whether it had any.
    pub fn forget_text(&self, id: &str) -> std::io::Result<bool> {
        let mut data = self.lock();
        let Some(message) = data
            .messages
            .iter_mut()
            .find(|message| message.id == id && message.holds_text())
        else {
            return Ok(false);
        };
        message.sealed = None;
        message.plain = None;
        message.expires_at = None;
        self.save(&data)?;
        Ok(true)
    }

    /// Takes the plain text kept before #127 out of the file: a sent message's is dropped, and a
    /// received one's is encrypted by `seal`, given the text and the message's id. What `seal`
    /// can't do now stays, for the next start.
    pub fn seal_plain(
        &self,
        seal: impl Fn(&str, &str) -> std::io::Result<Sealed>,
    ) -> std::io::Result<()> {
        let mut data = self.lock();
        let mut changed = false;
        for message in &mut data.messages {
            let Some(plain) = &message.plain else {
                continue;
            };
            if !message.outgoing {
                message.sealed = Some(seal(plain, &message.id)?);
            }
            message.plain = None;
            changed = true;
        }
        if changed {
            self.save(&data)?;
        }
        Ok(())
    }

    /// The chat's messages that travelled by radio, oldest first.
    pub fn messages_in(&self, chat_id: &str) -> Vec<RadioMessage> {
        self.lock()
            .messages
            .iter()
            .filter(|message| message.chat_id == chat_id)
            .cloned()
            .collect()
    }

    /// Marks the sent message whose confirmation carries `ack` as confirmed, and returns it: the
    /// code of its first try or of a retry, also after it timed out. Nothing for a code that
    /// isn't known, or a message that was confirmed already.
    pub fn confirm(&self, ack: &str) -> std::io::Result<Option<RadioMessage>> {
        let mut data = self.lock();
        let Some(message) = data.messages.iter_mut().rev().find(|message| {
            !message.confirmed
                && (message.ack.as_deref() == Some(ack)
                    || message.retry_acks.iter().any(|code| code == ack))
        }) else {
            return Ok(None);
        };
        message.confirmed = true;
        message.confirm_by = None;
        let confirmed = message.clone();
        self.save(&data)?;
        Ok(Some(confirmed))
    }

    /// Notes a retry of the sent message `id`: its confirmation carries `ack`, and is due by
    /// `confirm_by`. Returns false, and notes nothing, for a message that is confirmed or timed
    /// out already, or unknown.
    pub fn retried(&self, id: &str, ack: String, confirm_by: u64) -> std::io::Result<bool> {
        let mut data = self.lock();
        let Some(message) = data.messages.iter_mut().find(|message| message.id == id) else {
            return Ok(false);
        };
        if !awaits_confirmation(message) {
            return Ok(false);
        }
        message.retry_acks.push(ack);
        message.confirm_by = Some(confirm_by);
        self.save(&data)?;
        Ok(true)
    }

    /// Marks the sent message `id` as timed out, and returns it. Nothing for a message that is
    /// confirmed or timed out already, or unknown.
    pub fn time_out(&self, id: &str) -> std::io::Result<Option<RadioMessage>> {
        let mut data = self.lock();
        let Some(message) = data.messages.iter_mut().find(|message| message.id == id) else {
            return Ok(None);
        };
        if !awaits_confirmation(message) {
            return Ok(None);
        }
        message.timed_out = true;
        message.confirm_by = None;
        let timed_out = message.clone();
        self.save(&data)?;
        Ok(Some(timed_out))
    }

    /// After a restart: marks the sent messages whose confirmation was due by `now_ms` as timed
    /// out, and returns those still awaited. Retries don't outlive the app.
    pub fn after_restart(&self, now_ms: u64) -> std::io::Result<Vec<RadioMessage>> {
        let mut data = self.lock();
        let mut changed = false;
        let mut awaited = Vec::new();
        for message in data.messages.iter_mut().filter(|m| awaits_confirmation(m)) {
            match message.confirm_by {
                Some(due) if due > now_ms => awaited.push(message.clone()),
                // Overdue, or sent before the core knew when a confirmation is due.
                _ => {
                    message.timed_out = true;
                    message.confirm_by = None;
                    changed = true;
                }
            }
        }
        if changed {
            self.save(&data)?;
        }
        Ok(awaited)
    }

    /// Logs a transmission the app caused at `now_ms`, and forgets those older than two days.
    pub fn log_transmission(&self, transmission: Transmission, now_ms: u64) -> std::io::Result<()> {
        let mut data = self.lock();
        data.transmissions
            .retain(|known| known.at + KEEP_TRANSMISSIONS_MS > now_ms);
        data.transmissions.push(transmission);
        self.save(&data)
    }

    /// The transmissions the app caused since `since_ms`, oldest first.
    pub fn transmissions_since(&self, since_ms: u64) -> Vec<Transmission> {
        self.lock()
            .transmissions
            .iter()
            .filter(|transmission| transmission.at >= since_ms)
            .cloned()
            .collect()
    }

    /// The airtime the app used in the hour before `now_ms`, in milliseconds, and when the
    /// oldest of it drops out of the hour.
    pub fn airtime_used(&self, now_ms: u64) -> (u64, Option<u64>) {
        let data = self.lock();
        let in_hour: Vec<&Transmission> = data
            .transmissions
            .iter()
            .filter(|transmission| transmission.at + AIRTIME_WINDOW_MS > now_ms)
            .collect();
        let used = in_hour
            .iter()
            .map(|transmission| transmission.airtime_ms)
            .sum();
        let frees_at = in_hour
            .first()
            .map(|transmission| transmission.at + AIRTIME_WINDOW_MS);
        (used, frees_at)
    }

    /// Forgets everything, as when the account is deleted.
    pub fn clear(&self) -> std::io::Result<()> {
        *self.lock() = Data::default();
        private_file::remove(&self.path.with_extension("json.tmp"))?;
        private_file::remove(&self.path.with_extension("json.damaged"))?;
        private_file::remove(&self.path)
    }

    fn save(&self, data: &Data) -> std::io::Result<()> {
        let json = serde_json::to_vec(data).map_err(std::io::Error::other)?;
        // A crash leaves the old file or the new one: a counter that went out is never
        // forgotten.
        private_file::replace(&self.path, &json)
    }

    fn lock(&self) -> MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Whether a store that can't be parsed holds message text from before #127.
fn has_plain_text(json: &[u8]) -> bool {
    json.windows(7).any(|window| window == b"\"text\":")
}

/// Whether `message` is one this phone sent by radio whose confirmation is still awaited.
fn awaits_confirmation(message: &RadioMessage) -> bool {
    message.outgoing && !message.confirmed && !message.timed_out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(id: &str, ack: &str) -> RadioMessage {
        RadioMessage {
            chat_id: "chat".to_owned(),
            id: id.to_owned(),
            outgoing: true,
            sealed: None,
            plain: None,
            expires_at: None,
            sent_at: 1_795_300_000_000,
            ack: Some(ack.to_owned()),
            retry_acks: Vec::new(),
            confirmed: false,
            confirm_by: None,
            timed_out: false,
            kept_at: 1_795_300_000_100,
        }
    }

    #[test]
    fn a_retry_or_a_late_confirmation_still_confirms() {
        let data = tempfile::tempdir().unwrap();
        let store = RadioStore::open(data.path());
        store
            .add_message(RadioMessage {
                confirm_by: Some(1_000),
                ..sent("m1", "0001")
            })
            .unwrap();
        assert!(store.retried("m1", "0002".to_owned(), 2_000).unwrap());
        let retried = store.message("m1").unwrap();
        assert_eq!(retried.retry_acks, vec!["0002".to_owned()]);
        assert_eq!(retried.confirm_by, Some(2_000));
        // The retry's code confirms it.
        assert!(store.confirm("0002").unwrap().unwrap().confirmed);
        assert_eq!(store.message("m1").unwrap().confirm_by, None);
        // Nothing more happens to a confirmed message.
        assert!(!store.retried("m1", "0003".to_owned(), 3_000).unwrap());
        assert_eq!(store.time_out("m1").unwrap(), None);

        // A message that timed out stays so across a restart, and its confirmation still counts
        // when it comes late.
        store.add_message(sent("m2", "0004")).unwrap();
        let timed_out = store.time_out("m2").unwrap().unwrap();
        assert!(timed_out.timed_out && !timed_out.confirmed);
        assert_eq!(store.time_out("m2").unwrap(), None);
        assert!(!store.retried("m2", "0005".to_owned(), 3_000).unwrap());
        let reopened = RadioStore::open(data.path());
        assert!(reopened.message("m2").unwrap().timed_out);
        let late = reopened.confirm("0004").unwrap().unwrap();
        assert!(late.confirmed && late.timed_out);

        // Unknown messages change nothing.
        assert!(!reopened.retried("m9", "0006".to_owned(), 3_000).unwrap());
        assert_eq!(reopened.time_out("m9").unwrap(), None);
    }

    #[test]
    fn after_a_restart_overdue_confirmations_time_out() {
        let data = tempfile::tempdir().unwrap();
        let store = RadioStore::open(data.path());
        let due = |id: &str, ack: &str, confirm_by: Option<u64>| RadioMessage {
            confirm_by,
            ..sent(id, ack)
        };
        store
            .add_message(due("overdue", "0001", Some(1_000)))
            .unwrap();
        store
            .add_message(due("awaited", "0002", Some(5_000)))
            .unwrap();
        // Sent before the core knew when a confirmation is due.
        store.add_message(due("older", "0003", None)).unwrap();
        store.add_message(due("confirmed", "0004", None)).unwrap();
        store.confirm("0004").unwrap();
        store
            .add_message(RadioMessage {
                outgoing: false,
                ack: None,
                ..sent("received", "")
            })
            .unwrap();

        let awaited = store.after_restart(2_000).unwrap();
        assert_eq!(
            awaited.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["awaited"]
        );
        let reopened = RadioStore::open(data.path());
        let timed_out = |id: &str| reopened.message(id).unwrap().timed_out;
        assert!(timed_out("overdue") && timed_out("older"));
        assert!(!timed_out("awaited") && !timed_out("confirmed") && !timed_out("received"));
    }

    #[test]
    fn issue_127_dropped_text_leaves_what_replays_and_confirmations_need() {
        let data = tempfile::tempdir().unwrap();
        let vault = crate::vault::test_vault(data.path());
        let store = RadioStore::open(data.path());
        store
            .add_message(RadioMessage {
                outgoing: false,
                sealed: Some(vault.seal("Hello", "in").unwrap()),
                expires_at: Some(5_000),
                ..sent("in", "")
            })
            .unwrap();
        store.add_message(sent("out", "0001")).unwrap();
        assert_eq!(store.messages_with_text().len(), 1);

        assert!(store.forget_text("in").unwrap());
        assert!(!store.forget_text("in").unwrap());
        assert!(!store.forget_text("out").unwrap());
        let reopened = RadioStore::open(data.path());
        assert!(reopened.messages_with_text().is_empty());
        let kept = reopened.message("in").unwrap();
        assert_eq!(
            (kept.chat_id.as_str(), kept.sealed, kept.expires_at),
            ("chat", None, None)
        );
        assert!(reopened.confirm("0001").unwrap().is_some());
    }

    #[test]
    fn issue_127_text_kept_by_an_earlier_version_leaves_the_file() {
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("radio.json");
        let aside = data.path().join("radio.json.damaged");
        std::fs::write(
            &path,
            br#"{"messages":[
                {"chat_id":"c","id":"in","outgoing":false,"text":"Received secret","sent_at":1},
                {"chat_id":"c","id":"out","outgoing":true,"text":"Sent secret","sent_at":2}
            ]}"#,
        )
        .unwrap();
        std::fs::write(&aside, br#"{"messages":[{"text":"Old secret""#).unwrap();
        let vault = crate::vault::test_vault(data.path());
        let store = RadioStore::open(data.path());
        assert!(!aside.exists());
        store.seal_plain(|text, id| vault.seal(text, id)).unwrap();
        let file = std::fs::read_to_string(&path).unwrap();
        assert!(!file.contains("secret"), "{file}");
        let received = store.message("in").unwrap().sealed.unwrap();
        assert_eq!(
            vault.open(&received, "in").as_deref(),
            Some("Received secret")
        );
        assert!(!store.message("out").unwrap().holds_text());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // A damaged file with text from before is removed rather than kept aside.
        std::fs::write(&path, br#"{"messages":[{"text":"Torn secret""#).unwrap();
        RadioStore::open(data.path());
        assert!(!aside.exists());
        let file = std::fs::read_to_string(&path).unwrap();
        assert!(!file.contains("secret"), "{file}");
    }

    #[test]
    fn counters_go_up_per_chat_and_epoch_and_survive_a_restart() {
        let data = tempfile::tempdir().unwrap();
        let store = RadioStore::open(data.path());
        assert_eq!(store.take_counter("a", 7).unwrap(), 0);
        assert_eq!(store.take_counter("a", 7).unwrap(), 1);
        assert_eq!(store.take_counter("a", 8).unwrap(), 0);
        assert_eq!(store.take_counter("b", 7).unwrap(), 0);

        let reopened = RadioStore::open(data.path());
        assert_eq!(reopened.take_counter("a", 7).unwrap(), 2);
        assert_eq!(reopened.take_counter("a", 8).unwrap(), 1);
    }

    #[test]
    fn a_damaged_file_stops_sending_until_the_next_epoch() {
        let data = tempfile::tempdir().unwrap();
        std::fs::write(data.path().join("radio.json"), b"{not json").unwrap();
        let store = RadioStore::open(data.path());
        assert!(data.path().join("radio.json.damaged").exists());
        // Epoch 7 may have been used before the damage, with counters now lost.
        assert!(matches!(
            store.take_counter("a", 7),
            Err(CounterError::Lost(8))
        ));
        assert!(matches!(
            store.take_counter("a", 7),
            Err(CounterError::Lost(8))
        ));
        assert_eq!(store.take_counter("a", 8).unwrap(), 0);
        // Each chat on its own: b's first try is in epoch 3.
        assert!(matches!(
            store.take_counter("b", 3),
            Err(CounterError::Lost(4))
        ));
        // Also after a restart.
        let reopened = RadioStore::open(data.path());
        assert!(matches!(
            reopened.take_counter("a", 7),
            Err(CounterError::Lost(8))
        ));
        assert_eq!(reopened.take_counter("a", 8).unwrap(), 1);
    }

    fn record(key: &str, at: u64) -> RadioAddress {
        RadioAddress::parse(&format!(
            r#"{{"v":0,"mesh":"meshcore","key":"{key}","freq":869618,"bw":62500,"sf":8,"cr":8,"at":{at}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn keeps_the_switch_the_contacts_and_the_messages() {
        let data = tempfile::tempdir().unwrap();
        let store = RadioStore::open(data.path());
        assert!(!store.forced());
        store.set_forced(true).unwrap();
        let radio = record(&"be".repeat(32), 10);
        assert!(store.set_contact_radio("bea", radio.clone()).unwrap());
        // An older record, or the same again, doesn't replace it.
        assert!(
            !store
                .set_contact_radio("bea", record(&"cd".repeat(32), 9))
                .unwrap()
        );
        assert!(!store.set_contact_radio("bea", radio.clone()).unwrap());
        assert!(store.add_message(sent("m1", "0001")).unwrap());
        assert!(!store.add_message(sent("m1", "0002")).unwrap());

        let reopened = RadioStore::open(data.path());
        assert!(reopened.forced());
        assert_eq!(reopened.contact_radio("bea"), Some(radio));
        assert_eq!(
            reopened.contact_with_radio(&"be".repeat(32)).as_deref(),
            Some("bea")
        );
        assert_eq!(reopened.contact_with_radio(&"cd".repeat(32)), None);
        assert_eq!(reopened.messages_in("chat"), vec![sent("m1", "0001")]);
        assert!(reopened.messages_in("other").is_empty());

        // A confirmation counts once.
        assert_eq!(reopened.confirm("0002").unwrap(), None);
        assert!(reopened.confirm("0001").unwrap().unwrap().confirmed);
        assert_eq!(reopened.confirm("0001").unwrap(), None);
        assert!(
            RadioStore::open(data.path())
                .message("m1")
                .unwrap()
                .confirmed
        );

        // A newer record does, and so does another made at the same time.
        assert!(
            reopened
                .set_contact_radio("bea", record(&"cd".repeat(32), 11))
                .unwrap()
        );
        assert_eq!(reopened.contact_radio("bea").unwrap().at, 11);
        assert!(
            reopened
                .set_contact_radio("bea", record(&"ef".repeat(32), 11))
                .unwrap()
        );
        assert_eq!(reopened.contact_radio("bea").unwrap().key, "ef".repeat(32));

        reopened.clear().unwrap();
        reopened.clear().unwrap();
        let cleared = RadioStore::open(data.path());
        assert!(!cleared.forced());
        assert!(cleared.message("m1").is_none());
        assert_eq!(cleared.take_counter("chat", 1).unwrap(), 0);
    }

    #[test]
    fn remembers_the_paired_radio_until_it_is_forgotten() {
        let data = tempfile::tempdir().unwrap();
        let store = RadioStore::open(data.path());
        assert_eq!(store.pairing(), None);
        let pairing = Pairing {
            port: "/dev/cu.usbmodem1101".to_owned(),
            serial_number: Some("A1B2".to_owned()),
            name: "Ada's radio".to_owned(),
            radio: record(&"ab".repeat(32), 10),
        };
        store.set_pairing(Some(pairing.clone())).unwrap();
        let reopened = RadioStore::open(data.path());
        assert_eq!(reopened.pairing(), Some(pairing));
        reopened.set_pairing(None).unwrap();
        assert_eq!(RadioStore::open(data.path()).pairing(), None);

        // Deleting the account forgets it too.
        let store = RadioStore::open(data.path());
        store
            .set_pairing(Some(Pairing {
                port: "/dev/ttyACM0".to_owned(),
                serial_number: None,
                name: "Bea's radio".to_owned(),
                radio: record(&"be".repeat(32), 11),
            }))
            .unwrap();
        store.clear().unwrap();
        assert_eq!(RadioStore::open(data.path()).pairing(), None);
    }

    #[test]
    fn the_own_record_keeps_its_time_until_the_radio_changes() {
        use crate::radio::{Mesh, RadioInfo, RadioSettings};
        let data = tempfile::tempdir().unwrap();
        let store = RadioStore::open(data.path());
        let mut info = RadioInfo {
            mesh: Mesh::MeshCore,
            public_key: "ab".repeat(32),
            name: "Ada's radio".to_owned(),
            settings: RadioSettings {
                frequency_mhz: 869.618,
                bandwidth_khz: 62.5,
                spreading_factor: 8,
                coding_rate: 8,
                tx_power_dbm: 22,
            },
            firmware: None,
            model: None,
        };
        let first = store.own_record(&info, 100).unwrap();
        assert_eq!(first.at, 100);
        assert_eq!(store.own_record(&info, 200).unwrap(), first);
        assert!(!store.is_shared("chat", &first));
        store.set_shared("chat", first.clone()).unwrap();
        assert!(RadioStore::open(data.path()).is_shared("chat", &first));

        info.settings.spreading_factor = 7;
        let changed = store.own_record(&info, 300).unwrap();
        assert_eq!(changed.at, 300);
        assert!(!store.is_shared("chat", &changed));
    }

    #[test]
    fn logs_transmissions_and_counts_the_last_hour_of_airtime() {
        const HOUR: u64 = 60 * 60 * 1000;
        let data = tempfile::tempdir().unwrap();
        let store = RadioStore::open(data.path());
        let sent = |at: u64, airtime_ms: u64| Transmission {
            at,
            bytes: 70,
            airtime_ms,
            reason: "message".to_owned(),
        };
        assert_eq!(store.airtime_used(HOUR), (0, None));
        store.log_transmission(sent(HOUR, 700), HOUR).unwrap();
        store
            .log_transmission(sent(HOUR + 1_000, 1_600), HOUR + 1_000)
            .unwrap();
        assert_eq!(store.airtime_used(HOUR + 2_000), (2_300, Some(2 * HOUR)));
        // An hour on, the first drops out of the count, and frees its airtime.
        assert_eq!(
            store.airtime_used(2 * HOUR),
            (1_600, Some(2 * HOUR + 1_000))
        );
        assert_eq!(store.airtime_used(2 * HOUR + 1_000), (0, None));

        // The log stays across a restart, for two days.
        let reopened = RadioStore::open(data.path());
        assert_eq!(reopened.transmissions_since(0).len(), 2);
        assert_eq!(reopened.transmissions_since(HOUR + 1).len(), 1);
        reopened
            .log_transmission(sent(50 * HOUR, 700), 50 * HOUR)
            .unwrap();
        assert_eq!(reopened.transmissions_since(0), vec![sent(50 * HOUR, 700)]);
    }
}

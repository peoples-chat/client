//! What the core keeps about the radio (#40), in `radio.json` in the data folder:
//!
//! - **The paired radio** (#24): which radio it is and where it was plugged in, to open it again.
//! - **The switch that forces radio**, for testing until the core chooses the transport (#45).
//! - **The radio keys** (#128): this phone's current key for each chat and mesh, with its next
//!   counter, and the keys the contacts sent. A counter is stored before its packet goes out, so
//!   that it is never used twice with one key (`docs/radio-format-v0.md`). A damaged file loses
//!   the keys with their counters: this phone then makes new keys.
//! - **The contacts' radios:** the address book, which their radio address records fill (#34),
//!   and which record this phone shared in each chat.
//! - **Every message that travelled by radio,** sent or received, with its id. A received one is
//!   in no other store until its relay copy arrives, and the ids catch a replayed packet after a
//!   restart (#42). A sent one keeps the codes its confirmation may carry, and whether one came
//!   in time (#41).
//! - **Every transmission the app caused** in the last two days, with its airtime and reason: the
//!   hourly airtime cap counts them (#51), and they show that the app sends nothing by itself
//!   (#52).
//!
//! The file is small, and written whole on every change.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

use crate::radio::{InboxKey, RadioAddress, RadioKey, RadioKeyRecord, new_radio_key};

/// How long a contact's key still opens packets once a newer one has come: long enough for a
/// packet, and its retry (#41), already on the way.
pub const OLD_KEY_GRACE_MS: u64 = 10 * 60 * 1000;
/// How many of a contact's keys a chat keeps for one mesh, the current one included.
const KEPT_KEYS: usize = 4;
/// How many meshes a chat keeps a contact's keys for, known and unknown.
const KEPT_MESHES: usize = 8;

/// This phone's current key for one chat and mesh (#128).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnKey {
    pub record: RadioKeyRecord,
    /// The chat's members, and their device counts, when the key was made: a change to them
    /// calls for a new key.
    pub members: Vec<(String, u32)>,
    /// The counter of the next packet sealed with the key.
    next_counter: u64,
    /// The record has been handed to the chat, so the key may seal packets.
    pub shared: bool,
}

/// What one packet is sealed with.
pub struct SendingKey {
    pub key: RadioKey,
    pub number: u64,
    /// Stored as used already.
    pub counter: u64,
}

/// A key that the contact sent in a chat.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ContactKey {
    record: RadioKeyRecord,
    /// Once a newer key for the mesh has come: when the contact made it, by their clock, and
    /// when this phone got it, both in milliseconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    replaced: Option<(u64, u64)>,
}

/// A message that travelled by radio.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadioMessage {
    pub chat_id: String,
    /// MDK's id for the message, which its relay copy has too.
    pub id: String,
    pub outgoing: bool,
    pub text: String,
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

#[derive(Debug, Default, Serialize, Deserialize)]
struct Data {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pairing: Option<Pairing>,
    #[serde(default)]
    forced: bool,
    /// Chat → mesh → this phone's current key.
    #[serde(default)]
    own_keys: BTreeMap<String, BTreeMap<String, OwnKey>>,
    /// Chat → the keys its contact sent, oldest first within each mesh.
    #[serde(default)]
    contact_keys: BTreeMap<String, Vec<ContactKey>>,
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
    /// The store in `data_dir`. A damaged file is kept aside as `radio.json.damaged`, and what it
    /// held is lost.
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("radio.json");
        let data = match std::fs::read(&path) {
            Ok(json) => serde_json::from_slice(&json).unwrap_or_else(|error| {
                // Only where it went wrong: the message could quote a key.
                log::error!(
                    "the radio store is damaged at line {}, column {}",
                    error.line(),
                    error.column()
                );
                if let Err(error) = std::fs::rename(&path, path.with_extension("json.damaged")) {
                    log::warn!("could not keep the damaged radio store aside: {error}");
                }
                Data::default()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Data::default(),
            Err(error) => {
                log::error!("could not read the radio store: {error}");
                Data::default()
            }
        };
        Self {
            path,
            data: Mutex::new(data),
        }
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

    /// This phone's current key for the chat `chat_id` and `mesh`.
    pub fn own_key(&self, chat_id: &str, mesh: &str) -> Option<OwnKey> {
        self.lock().own_keys.get(chat_id)?.get(mesh).cloned()
    }

    /// This phone's key for the chat `chat_id` and `mesh`, whose members are `members` now: the
    /// one kept, or a new random one if there is none or the members have changed since it was
    /// made. Kept before this returns.
    pub fn own_key_for(
        &self,
        chat_id: &str,
        mesh: &str,
        members: &[(String, u32)],
        now_ms: u64,
    ) -> std::io::Result<OwnKey> {
        let mut data = self.lock();
        let kept = data.own_keys.get(chat_id).and_then(|keys| keys.get(mesh));
        if let Some(kept) = kept
            && kept.members == members
        {
            return Ok(kept.clone());
        }
        // Later than the key before it, also if the clock went back.
        let (number, at) = kept.map_or((1, now_ms), |kept| {
            (kept.record.n + 1, now_ms.max(kept.record.at + 1))
        });
        let made = OwnKey {
            record: RadioKeyRecord::new(mesh, &new_radio_key(), number, at),
            members: members.to_vec(),
            next_counter: 0,
            shared: false,
        };
        data.own_keys
            .entry(chat_id.to_owned())
            .or_default()
            .insert(mesh.to_owned(), made.clone());
        self.save(&data)?;
        Ok(made)
    }

    /// Notes that the record of the key `number` has been handed to the chat.
    pub fn set_key_shared(&self, chat_id: &str, mesh: &str, number: u64) -> std::io::Result<()> {
        let mut data = self.lock();
        let Some(key) = data
            .own_keys
            .get_mut(chat_id)
            .and_then(|keys| keys.get_mut(mesh))
            .filter(|key| key.record.n == number)
        else {
            return Ok(());
        };
        key.shared = true;
        self.save(&data)
    }

    /// The key and counter for the next packet in the chat `chat_id` over `mesh`, the counter
    /// stored as used before this returns. Nothing while the current key hasn't been shared.
    pub fn take_counter(&self, chat_id: &str, mesh: &str) -> std::io::Result<Option<SendingKey>> {
        let mut data = self.lock();
        let Some(own) = data
            .own_keys
            .get_mut(chat_id)
            .and_then(|keys| keys.get_mut(mesh))
            .filter(|own| own.shared)
        else {
            return Ok(None);
        };
        let Some(key) = own.record.radio_key() else {
            return Ok(None);
        };
        let sending = SendingKey {
            key,
            number: own.record.n,
            counter: own.next_counter,
        };
        own.next_counter += 1;
        self.save(&data)?;
        Ok(Some(sending))
    }

    /// Keeps `record`, a key the contact sent in the chat `chat_id`. The newest by the contact's
    /// clock is current for its mesh; the ones before it are replaced from `now_ms`. Returns
    /// whether it was kept: not a key kept already, nor one older than all those kept.
    pub fn add_contact_key(
        &self,
        chat_id: &str,
        record: RadioKeyRecord,
        now_ms: u64,
    ) -> std::io::Result<bool> {
        let mut data = self.lock();
        let keys = data.contact_keys.entry(chat_id.to_owned()).or_default();
        let known = keys
            .iter()
            .any(|kept| kept.record.mesh == record.mesh && kept.record.key == record.key);
        let meshes: BTreeSet<&str> = keys.iter().map(|kept| kept.record.mesh.as_str()).collect();
        if known || (meshes.len() >= KEPT_MESHES && !meshes.contains(record.mesh.as_str())) {
            return Ok(false);
        }
        let mesh = record.mesh.clone();
        let made = (record.at, record.n);
        keys.push(ContactKey {
            record,
            replaced: None,
        });
        keys.sort_by_key(|kept| (kept.record.at, kept.record.n));
        // Each key but the newest of the mesh was replaced by the one after it.
        let mut newer: Option<u64> = None;
        let mut of_mesh = 0;
        let mut dropped = Vec::new();
        for (index, kept) in keys.iter_mut().enumerate().rev() {
            if kept.record.mesh != mesh {
                continue;
            }
            of_mesh += 1;
            if let Some(newer) = newer {
                let since = kept.replaced.map_or(now_ms, |(_, since)| since);
                kept.replaced = Some((newer, since));
            }
            newer = Some(kept.record.at);
            if of_mesh > KEPT_KEYS {
                dropped.push(index);
            }
        }
        let was_dropped = dropped
            .iter()
            .any(|index| (keys[*index].record.at, keys[*index].record.n) == made);
        // From the back, so that the places stay right.
        for index in dropped {
            keys.remove(index);
        }
        self.save(&data)?;
        Ok(!was_dropped)
    }

    /// Every key the contact sent in the chat `chat_id` that is kept, whatever its mesh.
    pub fn contact_key_records(&self, chat_id: &str) -> Vec<RadioKeyRecord> {
        self.lock()
            .contact_keys
            .get(chat_id)
            .map_or_else(Vec::new, |keys| {
                keys.iter().map(|kept| kept.record.clone()).collect()
            })
    }

    /// The contact's keys that open packets in the chat `chat_id` over `mesh` at `now_ms`: the
    /// current one, and those replaced less than [`OLD_KEY_GRACE_MS`] ago.
    pub fn receiving_keys(&self, chat_id: &str, mesh: &str, now_ms: u64) -> Vec<InboxKey> {
        let data = self.lock();
        data.contact_keys
            .get(chat_id)
            .into_iter()
            .flatten()
            .filter(|kept| kept.record.mesh == mesh)
            .filter(|kept| {
                kept.replaced
                    .is_none_or(|(_, since)| now_ms < since.saturating_add(OLD_KEY_GRACE_MS))
            })
            .filter_map(|kept| {
                Some(InboxKey {
                    number: kept.record.n,
                    key: kept.record.radio_key()?,
                    replaced_at: kept
                        .replaced
                        .map(|(at, _)| u32::try_from(at / 1000).unwrap_or(u32::MAX)),
                })
            })
            .collect()
    }

    /// Whether `key`, in hex, is one of this phone's own current keys.
    pub fn is_own_key(&self, key: &str) -> bool {
        self.lock()
            .own_keys
            .values()
            .flat_map(BTreeMap::values)
            .any(|own| own.record.key == key)
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
        match std::fs::remove_file(&self.path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    fn save(&self, data: &Data) -> std::io::Result<()> {
        if let Some(folder) = self.path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let json = serde_json::to_vec(data).map_err(std::io::Error::other)?;
        // Written whole and synced, then moved into place, so that a crash leaves the old file
        // or the new one: a counter that went out is never forgotten.
        let temporary = self.path.with_extension("json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        // It holds the radio keys: for this user only.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temporary)?;
        std::io::Write::write_all(&mut file, &json)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &self.path)
    }

    fn lock(&self) -> MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(PoisonError::into_inner)
    }
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
            text: "Hello".to_owned(),
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

    fn members(contact_devices: u32) -> Vec<(String, u32)> {
        vec![("ada".to_owned(), 1), ("bea".to_owned(), contact_devices)]
    }

    /// The counter of the next packet in the chat `a`, and its key.
    fn take(store: &RadioStore) -> (u64, [u8; 32], u64) {
        let sending = store.take_counter("a", "meshcore").unwrap().unwrap();
        (sending.number, *sending.key, sending.counter)
    }

    #[test]
    fn issue_128_own_keys_keep_their_counters_across_restarts_and_change_with_the_members() {
        let data = tempfile::tempdir().unwrap();
        let store = RadioStore::open(data.path());
        let first = store
            .own_key_for("a", "meshcore", &members(1), 1_000)
            .unwrap();
        assert_eq!(first.record.n, 1);
        // Nothing is sealed with a key the chat doesn't have.
        assert!(store.take_counter("a", "meshcore").unwrap().is_none());
        store.set_key_shared("a", "meshcore", 1).unwrap();
        let (number, key, counter) = take(&store);
        assert_eq!((number, counter), (1, 0));
        assert_eq!(take(&store), (1, key, 1));

        // Another chat, and another mesh, have keys of their own.
        let other_chat = store
            .own_key_for("b", "meshcore", &members(1), 1_000)
            .unwrap();
        let other_mesh = store
            .own_key_for("a", "meshtastic", &members(1), 1_000)
            .unwrap();
        assert_ne!(other_chat.record.key, first.record.key);
        assert_ne!(other_mesh.record.key, first.record.key);

        // The same key and the next counter after a restart.
        let reopened = RadioStore::open(data.path());
        let kept = reopened
            .own_key_for("a", "meshcore", &members(1), 2_000)
            .unwrap();
        assert_eq!(kept.record, first.record);
        assert_eq!(take(&reopened), (1, key, 2));

        // The members change: a new key, made later even with the clock gone back.
        let next = reopened
            .own_key_for("a", "meshcore", &members(2), 500)
            .unwrap();
        assert_eq!((next.record.n, next.record.at), (2, 1_001));
        assert_ne!(next.record.key, first.record.key);
        assert!(!next.shared);
        assert!(reopened.take_counter("a", "meshcore").unwrap().is_none());
        // A late note about the old key doesn't count for the new one.
        reopened.set_key_shared("a", "meshcore", 1).unwrap();
        assert!(reopened.take_counter("a", "meshcore").unwrap().is_none());
        reopened.set_key_shared("a", "meshcore", 2).unwrap();
        assert_eq!(take(&reopened).2, 0);
    }

    #[test]
    fn issue_128_a_damaged_file_means_new_keys() {
        let data = tempfile::tempdir().unwrap();
        let store = RadioStore::open(data.path());
        let first = store
            .own_key_for("a", "meshcore", &members(1), 1_000)
            .unwrap();
        std::fs::write(data.path().join("radio.json"), b"{not json").unwrap();
        let reopened = RadioStore::open(data.path());
        assert!(data.path().join("radio.json.damaged").exists());
        // The counters are lost, and the key with them, so no nonce repeats.
        assert_eq!(reopened.own_key("a", "meshcore"), None);
        let next = reopened
            .own_key_for("a", "meshcore", &members(1), 2_000)
            .unwrap();
        assert_ne!(next.record.key, first.record.key);
    }

    fn contact_key(mesh: &str, number: u64) -> RadioKeyRecord {
        let key = new_radio_key();
        RadioKeyRecord::new(mesh, &key, number, number * 1_000_000)
    }

    fn numbers(store: &RadioStore, mesh: &str, now_ms: u64) -> Vec<(u64, Option<u32>)> {
        store
            .receiving_keys("a", mesh, now_ms)
            .iter()
            .map(|key| (key.number, key.replaced_at))
            .collect()
    }

    #[test]
    fn issue_128_a_replaced_key_is_kept_for_packets_on_the_way_then_not_used() {
        let data = tempfile::tempdir().unwrap();
        let store = RadioStore::open(data.path());
        let first = contact_key("meshcore", 1);
        assert!(store.add_contact_key("a", first.clone(), 100).unwrap());
        assert!(!store.add_contact_key("a", first.clone(), 100).unwrap());
        assert!(
            store
                .add_contact_key("a", contact_key("meshcore", 2), 5_000)
                .unwrap()
        );

        let reopened = RadioStore::open(data.path());
        // Just replaced: it still opens what was sent before the newer key was made.
        assert_eq!(
            numbers(&reopened, "meshcore", 5_000),
            vec![(1, Some(2_000)), (2, None)]
        );
        // Later it opens nothing, and reading the chat's history again changes nothing.
        let later = 5_000 + OLD_KEY_GRACE_MS;
        assert!(!reopened.add_contact_key("a", first, later).unwrap());
        assert_eq!(numbers(&reopened, "meshcore", later), vec![(2, None)]);

        // Only the newest few are kept; an old one that comes again stays out.
        for number in 3..=6 {
            reopened
                .add_contact_key("a", contact_key("meshcore", number), later)
                .unwrap();
        }
        let kept: Vec<u64> = reopened
            .contact_key_records("a")
            .iter()
            .map(|record| record.n)
            .collect();
        assert_eq!(kept, vec![3, 4, 5, 6]);
        assert!(
            !reopened
                .add_contact_key("a", contact_key("meshcore", 1), later)
                .unwrap()
        );
    }

    #[test]
    fn issue_128_each_mesh_has_its_own_current_key() {
        let data = tempfile::tempdir().unwrap();
        let store = RadioStore::open(data.path());
        for (mesh, number) in [("meshcore", 1), ("meshtastic", 2), ("future-mesh", 3)] {
            assert!(
                store
                    .add_contact_key("a", contact_key(mesh, number), 100)
                    .unwrap()
            );
        }
        // None replaced another.
        for (mesh, number) in [("meshcore", 1), ("meshtastic", 2)] {
            assert_eq!(numbers(&store, mesh, u64::MAX), vec![(number, None)]);
        }
        assert_eq!(store.contact_key_records("a").len(), 3);
        // A chat doesn't keep keys for ever more meshes.
        for number in 0..20 {
            store
                .add_contact_key("a", contact_key(&format!("mesh-{number}"), 4), 100)
                .unwrap();
        }
        assert_eq!(store.contact_key_records("a").len(), KEPT_MESHES);
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
        assert!(cleared.contact_key_records("chat").is_empty());
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

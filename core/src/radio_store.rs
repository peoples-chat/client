//! What the core keeps about the radio (#40), in `radio.json` in the data folder:
//!
//! - **The paired radio** (#24): which radio it is and where it was plugged in, to open it again.
//! - **The switch that forces radio**, for testing until the core chooses the transport (#45).
//! - **This phone's radio records** (#128), one for each chat and mesh, each with its radio key
//!   and the key's next counter. A counter is stored before its packet goes out, so that it is
//!   never used twice with one key (`docs/radio-format-v0.md`). A damaged file loses the keys
//!   with their counters: this phone then makes new keys.
//! - **The contacts' records,** as the chats' state has them, and the keys they have held; and
//!   the address book that version 0 records fill (#34).
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
use serde_json::Value;
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::radio::{
    InboxKey, Mesh, RadioAddress, RadioInfo, RadioKey, RadioRecord, encode_component, new_radio_key,
};

/// How long a contact's key still opens packets once a newer one has come: long enough for a
/// packet, and its retry (#41), already on the way.
pub const OLD_KEY_GRACE_MS: u64 = 10 * 60 * 1000;
/// How many of a contact's keys a chat keeps for one mesh, the current one included.
const KEPT_KEYS: usize = 4;

/// This phone's current record for one chat and mesh (#128).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnRecord {
    pub record: RadioRecord,
    /// The chat's members, and their device counts, when the key was made: a change to them
    /// calls for a new key.
    pub members: Vec<(String, u32)>,
    /// The counter of the next packet sealed with the key.
    next_counter: u64,
    /// The chat's state has had the record, so the key may seal packets.
    pub published: bool,
}

/// What one packet is sealed with.
pub struct SendingKey {
    pub key: RadioKey,
    pub number: u64,
    /// Stored as used already.
    pub counter: u64,
}

/// A contact's records, as the state of one chat has them. They hold keys, so no `Debug`.
#[derive(Serialize, Deserialize)]
struct ContactRecords {
    contact: String,
    /// As they came, those the app can't use among them.
    records: Vec<Value>,
}

/// A key that a contact's record has held. No `Debug`, and wiped when dropped.
#[derive(Serialize, Deserialize)]
struct ContactKey {
    mesh: String,
    /// The key, in hex.
    key: String,
    number: u64,
    /// Once the record no longer held it: when the contact made the record after it, by their
    /// clock, and when this phone got that, both in milliseconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    replaced: Option<(u64, u64)>,
}

impl Drop for ContactKey {
    fn drop(&mut self) {
        self.key.zeroize();
    }
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
    /// When it disappears, in milliseconds since the Unix epoch, if the chat's messages
    /// disappeared when it was sent (#124). Its text is dropped then; its id stays, for the
    /// replay check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// Its time was up, and its text is gone (#124).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub expired: bool,
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

#[derive(Default, Serialize, Deserialize)]
struct Data {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pairing: Option<Pairing>,
    #[serde(default)]
    forced: bool,
    /// Chat → mesh → this phone's current record.
    #[serde(default)]
    own_records: BTreeMap<String, BTreeMap<String, OwnRecord>>,
    /// Chat → its contact's records.
    #[serde(default)]
    contact_records: BTreeMap<String, ContactRecords>,
    /// Chat → the keys its contact's records have held, oldest first.
    #[serde(default)]
    contact_keys: BTreeMap<String, Vec<ContactKey>>,
    /// Contact's account key → their radio, from their newest version 0 record.
    #[serde(default)]
    contacts: BTreeMap<String, RadioAddress>,
    /// Oldest first.
    #[serde(default)]
    messages: Vec<RadioMessage>,
    /// Oldest first, for the last two days.
    #[serde(default)]
    transmissions: Vec<Transmission>,
    /// The SHA-256, in hex, of every radio key that someone outside its chat could read (#163).
    /// Such a key opens and seals nothing any more.
    #[serde(default)]
    exposed: BTreeSet<String>,
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

    /// This phone's current record for the chat `chat_id` and `mesh`.
    pub fn own_record(&self, chat_id: &str, mesh: &str) -> Option<OwnRecord> {
        self.lock().own_records.get(chat_id)?.get(mesh).cloned()
    }

    /// Whether this phone has a record for the chat `chat_id`, on any mesh.
    pub fn has_own_records(&self, chat_id: &str) -> bool {
        self.lock().own_records.contains_key(chat_id)
    }

    /// Brings this phone's records for the chat `chat_id` up to date, and keeps them before this
    /// returns. A record gets a new random key when the chat's `members` have changed since its
    /// key was made, on every mesh, and when the radio `info` describes, or its name or
    /// settings, differ from the record of its mesh. A mesh without a record gets one. `None`
    /// for `members` while they can't be read: those kept stay, and a mesh without a record
    /// gets none.
    pub fn keep_own_records(
        &self,
        chat_id: &str,
        info: Option<&RadioInfo>,
        members: Option<&[(String, u32)]>,
        now_ms: u64,
    ) -> std::io::Result<()> {
        let mut data = self.lock();
        let mut records = data.own_records.get(chat_id).cloned().unwrap_or_default();
        let mut changed = false;
        if let Some(members) = members {
            for own in records.values_mut().filter(|own| own.members != members) {
                let mut record = own.record.clone();
                record.rk = hex::encode(new_radio_key().as_slice());
                *own = successor(&own.record, record, members.to_vec(), now_ms);
                changed = true;
            }
        }
        if let Some(info) = info {
            let current = RadioRecord::new(info, &new_radio_key(), 1, now_ms);
            let made = match records.get(info.mesh.name()) {
                Some(own) if own.record.same_radio(&current) => None,
                Some(own) => Some(successor(&own.record, current, own.members.clone(), now_ms)),
                None => members.map(|members| OwnRecord {
                    record: current,
                    members: members.to_vec(),
                    next_counter: 0,
                    published: false,
                }),
            };
            if let Some(made) = made {
                records.insert(info.mesh.name().to_owned(), made);
                changed = true;
            }
        }
        if !changed {
            return Ok(());
        }
        data.own_records.insert(chat_id.to_owned(), records);
        self.save(&data)
    }

    /// What this phone's component of the chat `chat_id` should hold, if it has a record there.
    pub fn own_component(&self, chat_id: &str) -> Option<Vec<u8>> {
        let data = self.lock();
        let records = data.own_records.get(chat_id)?;
        Some(encode_component(records.values().map(|own| &own.record)))
    }

    /// Notes that the chat's state has `component`: if that is what this phone's records of the
    /// chat make, their keys may seal packets.
    pub fn set_published(&self, chat_id: &str, component: &[u8]) -> std::io::Result<()> {
        let mut data = self.lock();
        let Some(records) = data.own_records.get_mut(chat_id) else {
            return Ok(());
        };
        if records.values().all(|own| own.published)
            || encode_component(records.values().map(|own| &own.record)) != component
        {
            return Ok(());
        }
        for own in records.values_mut() {
            own.published = true;
        }
        self.save(&data)
    }

    /// The key and counter for the next packet in the chat `chat_id` over `mesh`, the counter
    /// stored as used before this returns. Nothing while the chat's state hasn't had the current
    /// key.
    pub fn take_counter(&self, chat_id: &str, mesh: &str) -> std::io::Result<Option<SendingKey>> {
        let mut data = self.lock();
        let Some(own) = data
            .own_records
            .get_mut(chat_id)
            .and_then(|records| records.get_mut(mesh))
            .filter(|own| own.published)
        else {
            return Ok(None);
        };
        let Some(key) = own.record.radio_key() else {
            return Ok(None);
        };
        let sending = SendingKey {
            key,
            number: own.record.rkn,
            counter: own.next_counter,
        };
        own.next_counter += 1;
        self.save(&data)?;
        Ok(Some(sending))
    }

    /// Keeps `records` as what the state of the chat `chat_id` has for its contact `contact`.
    /// The key of a record is the contact's current one for its mesh; a key no record holds any
    /// more is replaced from `now_ms`. Returns whether anything changed.
    pub fn set_contact_records(
        &self,
        chat_id: &str,
        contact: &str,
        records: Vec<Value>,
        now_ms: u64,
    ) -> std::io::Result<bool> {
        let mut data = self.lock();
        if data
            .contact_records
            .get(chat_id)
            .is_some_and(|known| known.contact == contact && known.records == records)
        {
            return Ok(false);
        }
        let usable: Vec<RadioRecord> = records
            .iter()
            .filter_map(RadioRecord::from_value)
            // A key of this phone's own, sent back to it, would open its own packets.
            .filter(|record| !is_own_key(&data, &record.rk))
            // One that someone outside the chat could read is never taken again (#163).
            .filter(|record| !data.exposed.contains(&digest(&record.rk)))
            .collect();
        let keys = data.contact_keys.entry(chat_id.to_owned()).or_default();
        for mesh in [Mesh::MeshCore.name(), Mesh::Meshtastic.name()] {
            let record = usable.iter().find(|record| record.mesh == mesh);
            let held = record.map(|record| record.rk.as_str());
            let made = record.map_or(now_ms, |record| record.at);
            for kept in keys.iter_mut().filter(|kept| kept.mesh == mesh) {
                if Some(kept.key.as_str()) != held && kept.replaced.is_none() {
                    kept.replaced = Some((made, now_ms));
                }
            }
            let Some(record) = record else { continue };
            if !keys
                .iter()
                .any(|kept| kept.mesh == mesh && kept.key == record.rk)
            {
                keys.push(ContactKey {
                    mesh: mesh.to_owned(),
                    key: record.rk.clone(),
                    number: record.rkn,
                    replaced: None,
                });
            }
            let of_mesh = keys.iter().filter(|kept| kept.mesh == mesh).count();
            let mut excess = of_mesh.saturating_sub(KEPT_KEYS);
            // The oldest replaced ones go.
            keys.retain(|kept| {
                let drop = excess > 0 && kept.mesh == mesh && kept.replaced.is_some();
                excess -= usize::from(drop);
                !drop
            });
        }
        data.contact_records.insert(
            chat_id.to_owned(),
            ContactRecords {
                contact: contact.to_owned(),
                records,
            },
        );
        self.save(&data)?;
        Ok(true)
    }

    /// Someone who is neither the account nor its contact is in the chat `chat_id`, so they could
    /// read the radio keys in its state (#163): those of `in_state`, in hex, this phone's keys
    /// the state has had, and the contact's keys it has held.
    ///
    /// If any of them is new here: none of them is used or accepted again, the contact's records
    /// of the chat are forgotten, and this phone gets new keys in this chat and in `others`, the
    /// contact's direct chats, which are then due to be written. Returns whether that happened.
    pub fn expose(
        &self,
        chat_id: &str,
        in_state: &[String],
        others: &[String],
        now_ms: u64,
    ) -> std::io::Result<bool> {
        let mut data = self.lock();
        let own = data.own_records.get(chat_id).into_iter().flatten();
        let held = data.contact_keys.get(chat_id).into_iter().flatten();
        let new: Vec<String> = own
            .filter(|(_, own)| own.published)
            .map(|(_, own)| own.record.rk.as_str())
            .chain(held.map(|kept| kept.key.as_str()))
            .chain(in_state.iter().map(String::as_str))
            .map(digest)
            .filter(|key| !data.exposed.contains(key))
            .collect();
        if new.is_empty() {
            return Ok(false);
        }
        data.exposed.extend(new);
        data.contact_records.remove(chat_id);
        data.contact_keys.remove(chat_id);
        for chat_id in std::iter::once(chat_id).chain(others.iter().map(String::as_str)) {
            for own in data.own_records.get_mut(chat_id).into_iter().flatten() {
                let mut record = own.1.record.clone();
                record.rk = hex::encode(new_radio_key().as_slice());
                *own.1 = successor(&own.1.record, record, own.1.members.clone(), now_ms);
            }
        }
        self.save(&data)?;
        Ok(true)
    }

    /// The contact's records in the chat `chat_id`, as they came.
    pub fn contact_records(&self, chat_id: &str) -> Vec<Value> {
        self.lock()
            .contact_records
            .get(chat_id)
            .map_or_else(Vec::new, |known| known.records.clone())
    }

    /// The contact's keys that open packets in the chat `chat_id` over `mesh` at `now_ms`: the
    /// current one, and those replaced less than [`OLD_KEY_GRACE_MS`] ago.
    pub fn receiving_keys(&self, chat_id: &str, mesh: &str, now_ms: u64) -> Vec<InboxKey> {
        let data = self.lock();
        data.contact_keys
            .get(chat_id)
            .into_iter()
            .flatten()
            .filter(|kept| kept.mesh == mesh)
            .filter(|kept| {
                kept.replaced
                    .is_none_or(|(_, since)| now_ms < since.saturating_add(OLD_KEY_GRACE_MS))
            })
            .filter_map(|kept| {
                let mut key = RadioKey::default();
                hex::decode_to_slice(&kept.key, key.as_mut()).ok()?;
                Some(InboxKey {
                    number: kept.number,
                    key,
                    replaced_at: kept
                        .replaced
                        .map(|(at, _)| u32::try_from(at / 1000).unwrap_or(u32::MAX)),
                })
            })
            .collect()
    }

    /// The radio of the contact whose account key is `contact`: their MeshCore radio, the mesh
    /// that carries messages, or else another of theirs.
    pub fn contact_radio(&self, contact: &str) -> Option<RadioAddress> {
        let mut radios = contact_radios(&self.lock(), contact);
        radios.sort_by_key(|radio| radio.mesh != Mesh::MeshCore.name());
        radios.into_iter().next()
    }

    /// The contact whose radio has the public key `radio`.
    pub fn contact_with_radio(&self, radio: &str) -> Option<String> {
        let data = self.lock();
        data.contacts
            .keys()
            .chain(data.contact_records.values().map(|known| &known.contact))
            .find(|contact| {
                contact_radios(&data, contact)
                    .iter()
                    .any(|known| known.key == radio)
            })
            .cloned()
    }

    /// Keeps `record`, a version 0 record, as the radio of the contact `contact`, unless the
    /// contact has a newer one, or this one already. Returns whether it was kept. Of two records
    /// made at the same time, the one that came last counts.
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

    /// Drops the text of the messages whose time is up at `now_ms`, and of those in `ids`,
    /// which MDK deleted (#124). Their ids, counters and confirmation codes stay, for the
    /// replay check and late confirmations. Returns the messages it changed.
    pub fn expire(&self, now_ms: u64, ids: &[String]) -> std::io::Result<Vec<RadioMessage>> {
        let mut data = self.lock();
        let mut changed = Vec::new();
        for message in &mut data.messages {
            let due =
                message.expires_at.is_some_and(|at| at <= now_ms) || ids.contains(&message.id);
            if due && !message.expired {
                message.expired = true;
                message.text.clear();
                changed.push(message.clone());
            }
        }
        if !changed.is_empty() {
            self.save(&data)?;
        }
        Ok(changed)
    }

    /// When the next kept message's time is up, in milliseconds since the Unix epoch (#124).
    pub fn next_expiry(&self) -> Option<u64> {
        self.lock()
            .messages
            .iter()
            .filter(|message| !message.expired)
            .filter_map(|message| message.expires_at)
            .min()
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

/// The record that comes after `before`: `record`, with the next number, made later even if the
/// clock went back.
fn successor(
    before: &RadioRecord,
    mut record: RadioRecord,
    members: Vec<(String, u32)>,
    now_ms: u64,
) -> OwnRecord {
    record.rkn = before.rkn + 1;
    record.at = now_ms.max(before.at + 1);
    OwnRecord {
        record,
        members,
        next_counter: 0,
        published: false,
    }
}

/// What [`Data::exposed`] keeps of `key`, in hex.
fn digest(key: &str) -> String {
    hex::encode(Sha256::digest(key.to_ascii_lowercase()))
}

/// Whether `key`, in hex, is one of this phone's own current keys.
fn is_own_key(data: &Data, key: &str) -> bool {
    data.own_records
        .values()
        .flat_map(BTreeMap::values)
        .any(|own| own.record.rk == key)
}

/// The radios of the contact `contact`, one for each mesh: from the records in their chats'
/// state, or from a version 0 record where that is newer.
fn contact_radios(data: &Data, contact: &str) -> Vec<RadioAddress> {
    let mut radios: BTreeMap<String, RadioAddress> = BTreeMap::new();
    let from_state = data
        .contact_records
        .values()
        .filter(|known| known.contact == contact)
        .flat_map(|known| &known.records)
        .filter_map(RadioRecord::from_value)
        .map(|record| record.address());
    for radio in from_state.chain(data.contacts.get(contact).cloned()) {
        if radios
            .get(&radio.mesh)
            .is_none_or(|known| radio.at > known.at)
        {
            radios.insert(radio.mesh.clone(), radio);
        }
    }
    radios.into_values().collect()
}

/// Whether `message` is one this phone sent by radio whose confirmation is still awaited.
fn awaits_confirmation(message: &RadioMessage) -> bool {
    message.outgoing && !message.confirmed && !message.timed_out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radio::{MOCK_SETTINGS, MeshtasticInfo};

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
            expires_at: None,
            expired: false,
        }
    }

    /// #124: a radio message's text goes when its time is up, or when MDK deleted its relay
    /// copy, also across a restart. Its id stays, so a replayed packet is still caught.
    #[test]
    fn issue_124_radio_messages_lose_their_text_when_they_expire() {
        let data = tempfile::tempdir().unwrap();
        let store = RadioStore::open(data.path());
        store
            .add_message(RadioMessage {
                expires_at: Some(2_000),
                ..sent("timed", "0001")
            })
            .unwrap();
        store.add_message(sent("pruned", "0002")).unwrap();
        store.add_message(sent("kept", "0003")).unwrap();

        // Not yet.
        assert!(store.expire(1_999, &[]).unwrap().is_empty());
        assert_eq!(store.message("timed").unwrap().text, "Hello");

        let expired = store.expire(2_000, &["pruned".to_owned()]).unwrap();
        let ids: Vec<&str> = expired.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["timed", "pruned"]);
        // Once only.
        assert!(
            store
                .expire(3_000, &["pruned".to_owned()])
                .unwrap()
                .is_empty()
        );

        let reopened = RadioStore::open(data.path());
        for id in ["timed", "pruned"] {
            let message = reopened.message(id).unwrap();
            assert!(message.expired && message.text.is_empty(), "{id}");
            assert!(message.ack.is_some(), "{id}");
        }
        assert_eq!(reopened.message("kept").unwrap().text, "Hello");
        // The text is gone from the file too.
        let json = std::fs::read_to_string(data.path().join("radio.json")).unwrap();
        assert_eq!(json.matches("Hello").count(), 1);
        // The id still catches a replay.
        assert!(!reopened.add_message(sent("timed", "0009")).unwrap());
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

    fn radio(mesh: Mesh) -> RadioInfo {
        RadioInfo {
            mesh,
            public_key: "ab".repeat(32),
            name: "Ada's radio".to_owned(),
            settings: MOCK_SETTINGS,
            firmware: None,
            model: None,
            meshtastic: (mesh == Mesh::Meshtastic).then_some(MeshtasticInfo {
                node_num: 7,
                preset: None,
                region: None,
            }),
        }
    }

    /// Brings the chat `a`'s records up to date and puts them in its state.
    fn publish(store: &RadioStore, info: &RadioInfo, devices: u32, now_ms: u64) -> OwnRecord {
        store
            .keep_own_records("a", Some(info), Some(&members(devices)), now_ms)
            .unwrap();
        let component = store.own_component("a").unwrap();
        store.set_published("a", &component).unwrap();
        store.own_record("a", info.mesh.name()).unwrap()
    }

    /// The number, key and counter of the next packet in the chat `a`.
    fn take(store: &RadioStore) -> (u64, [u8; 32], u64) {
        let sending = store.take_counter("a", "meshcore").unwrap().unwrap();
        (sending.number, *sending.key, sending.counter)
    }

    #[test]
    fn issue_128_own_keys_survive_restarts_and_change_with_the_members_and_the_radio() {
        let data = tempfile::tempdir().unwrap();
        let store = RadioStore::open(data.path());
        let info = radio(Mesh::MeshCore);
        store
            .keep_own_records("a", Some(&info), Some(&members(1)), 1_000)
            .unwrap();
        let first = store.own_record("a", "meshcore").unwrap();
        assert_eq!((first.record.rkn, first.record.at), (1, 1_000));
        // Nothing is sealed with a key the chat's state doesn't have.
        assert!(store.take_counter("a", "meshcore").unwrap().is_none());
        store.set_published("a", b"[]").unwrap();
        assert!(store.take_counter("a", "meshcore").unwrap().is_none());
        publish(&store, &info, 1, 1_000);
        let (number, key, counter) = take(&store);
        assert_eq!((number, counter), (1, 0));
        assert_eq!(take(&store), (1, key, 1));

        // The same key and the next counter after a restart.
        let reopened = RadioStore::open(data.path());
        assert_eq!(publish(&reopened, &info, 1, 2_000).record, first.record);
        assert_eq!(take(&reopened), (1, key, 2));

        // Another mesh has a record and key of its own, and leaves this one as it was.
        let other = publish(&reopened, &radio(Mesh::Meshtastic), 1, 2_000);
        assert_ne!(other.record.rk, first.record.rk);
        assert_eq!(
            reopened.own_record("a", "meshcore").unwrap().record,
            first.record
        );

        // The members change: a new key on every mesh, made later even with the clock gone back.
        reopened
            .keep_own_records("a", None, Some(&members(2)), 500)
            .unwrap();
        for before in [&first, &other] {
            let after = reopened.own_record("a", &before.record.mesh).unwrap();
            assert_eq!(after.record.rkn, 2);
            assert_eq!(after.record.at, before.record.at + 1);
            assert_ne!(after.record.rk, before.record.rk);
            assert!(after.record.same_radio(&before.record));
            assert!(!after.published);
        }
        assert!(reopened.take_counter("a", "meshcore").unwrap().is_none());
        publish(&reopened, &info, 2, 3_000);
        assert_eq!(take(&reopened).2, 0);

        // So does a change to the radio's settings, or another radio, also while the members
        // can't be read.
        let mut changed = info.clone();
        changed.settings.spreading_factor = 7;
        reopened
            .keep_own_records("a", Some(&changed), None, 4_000)
            .unwrap();
        let after = reopened.own_record("a", "meshcore").unwrap();
        assert_eq!((after.record.rkn, after.record.sf), (3, 7));
        changed.public_key = "cd".repeat(32);
        assert_eq!(publish(&reopened, &changed, 2, 5_000).record.rkn, 4);
        // A chat whose members can't be read gets no first record.
        reopened
            .keep_own_records("b", Some(&info), None, 6_000)
            .unwrap();
        assert!(!reopened.has_own_records("b"));
    }

    #[test]
    fn issue_128_a_damaged_file_means_new_keys() {
        let data = tempfile::tempdir().unwrap();
        let store = RadioStore::open(data.path());
        let info = radio(Mesh::MeshCore);
        let first = publish(&store, &info, 1, 1_000);
        std::fs::write(data.path().join("radio.json"), b"{not json").unwrap();
        let reopened = RadioStore::open(data.path());
        assert!(data.path().join("radio.json.damaged").exists());
        // The counters are lost, and the key with them, so no nonce repeats.
        assert_eq!(reopened.own_record("a", "meshcore"), None);
        let next = publish(&reopened, &info, 1, 2_000);
        assert_ne!(next.record.rk, first.record.rk);
    }

    /// A contact's record on `mesh` with a new key, number `number`.
    fn contact_record(mesh: Mesh, number: u64) -> Value {
        let record = RadioRecord::new(&radio(mesh), &new_radio_key(), number, number * 1_000_000);
        serde_json::to_value(record).unwrap()
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
        let set = |store: &RadioStore, record: &Value, now_ms| {
            store
                .set_contact_records("a", "bea", vec![record.clone()], now_ms)
                .unwrap()
        };
        let first = contact_record(Mesh::MeshCore, 1);
        assert!(set(&store, &first, 100));
        assert!(!set(&store, &first, 100));
        assert!(set(&store, &contact_record(Mesh::MeshCore, 2), 5_000));

        let reopened = RadioStore::open(data.path());
        // Just replaced: it still opens what was sent before the newer key was made.
        assert_eq!(
            numbers(&reopened, "meshcore", 5_000),
            vec![(1, Some(2_000)), (2, None)]
        );
        let later = 5_000 + OLD_KEY_GRACE_MS;
        assert_eq!(numbers(&reopened, "meshcore", later), vec![(2, None)]);

        // Only the newest few are kept.
        for number in 3..=6 {
            set(&reopened, &contact_record(Mesh::MeshCore, number), later);
        }
        let kept: Vec<u64> = numbers(&reopened, "meshcore", later)
            .into_iter()
            .map(|(number, _)| number)
            .collect();
        assert_eq!(kept, vec![3, 4, 5, 6]);
    }

    #[test]
    fn issue_128_each_mesh_has_its_own_key_and_radio() {
        let data = tempfile::tempdir().unwrap();
        let store = RadioStore::open(data.path());
        let records = vec![
            contact_record(Mesh::Meshtastic, 2),
            serde_json::json!({"v": 1, "mesh": "a-mesh-of-the-future"}),
            contact_record(Mesh::MeshCore, 1),
        ];
        store
            .set_contact_records("a", "bea", records.clone(), 100)
            .unwrap();
        for (mesh, number) in [("meshcore", 1), ("meshtastic", 2)] {
            assert_eq!(numbers(&store, mesh, u64::MAX), vec![(number, None)]);
        }
        assert!(numbers(&store, "a-mesh-of-the-future", 100).is_empty());
        assert_eq!(store.contact_records("a"), records);
        // The MeshCore radio is the one messages can go to.
        assert_eq!(store.contact_radio("bea").unwrap().mesh, "meshcore");
        assert_eq!(
            store.contact_with_radio(&"ab".repeat(32)).as_deref(),
            Some("bea")
        );

        // A key that is this phone's own is never taken as the contact's.
        let own = publish(&store, &radio(Mesh::MeshCore), 1, 1_000);
        let reflected = serde_json::to_value(own.record).unwrap();
        store
            .set_contact_records("b", "eve", vec![reflected], 100)
            .unwrap();
        assert!(store.receiving_keys("b", "meshcore", 100).is_empty());
    }

    #[test]
    fn issue_163_an_exposed_key_is_never_used_or_accepted_again() {
        let data = tempfile::tempdir().unwrap();
        let store = RadioStore::open(data.path());
        let info = radio(Mesh::MeshCore);
        // Own records in two chats with the contact, `a` and `b`, both written.
        for chat in ["a", "b"] {
            store
                .keep_own_records(chat, Some(&info), Some(&members(1)), 1_000)
                .unwrap();
            let component = store.own_component(chat).unwrap();
            store.set_published(chat, &component).unwrap();
        }
        let own = |store: &RadioStore, chat: &str| store.own_record(chat, "meshcore").unwrap();
        let (in_a, in_b) = (own(&store, "a"), own(&store, "b"));
        let contacts = contact_record(Mesh::MeshCore, 1);
        store
            .set_contact_records("a", "bea", vec![contacts.clone()], 100)
            .unwrap();
        let in_state = vec![contacts["rk"].as_str().unwrap().to_owned()];

        let others = ["b".to_owned()];
        assert!(store.expose("a", &in_state, &others, 2_000).unwrap());
        for (chat, before) in [("a", &in_a), ("b", &in_b)] {
            let after = own(&store, chat);
            assert_ne!(after.record.rk, before.record.rk, "{chat}");
            assert_eq!(after.record.rkn, before.record.rkn + 1, "{chat}");
            assert!(!after.published, "{chat}");
        }
        assert!(store.receiving_keys("a", "meshcore", 2_000).is_empty());
        assert!(store.contact_records("a").is_empty());

        // Once only, also after a restart and with the old keys still in the chat's state.
        let reopened = RadioStore::open(data.path());
        let rotated = own(&reopened, "b");
        assert!(!reopened.expose("a", &in_state, &others, 3_000).unwrap());
        assert_eq!(own(&reopened, "b"), rotated);

        // The exposed key is refused wherever it comes back; a new one is taken.
        let fresh = contact_record(Mesh::MeshCore, 2);
        for chat in ["a", "b"] {
            reopened
                .set_contact_records(chat, "bea", vec![contacts.clone()], 4_000)
                .unwrap();
            assert!(reopened.receiving_keys(chat, "meshcore", 4_000).is_empty());
        }
        reopened
            .set_contact_records("a", "bea", vec![fresh], 4_000)
            .unwrap();
        assert_eq!(numbers(&reopened, "meshcore", 4_000), vec![(2, None)]);
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
        assert_eq!(cleared.contact_radio("bea"), None);
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

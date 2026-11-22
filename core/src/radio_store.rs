//! What the core keeps about the radio (#40), in `radio.json` in the data folder:
//!
//! - **The switch that forces radio**, for testing until the core chooses the transport (#45).
//! - **The counters** for what this phone sends: each chat's next counter in each epoch. A
//!   counter is stored before its packet goes out, so that it is never used twice with one key
//!   (`docs/radio-format-v0.md`).
//! - **The contacts' radios:** the address book, which the radio address records fill (#34).
//! - **Every message that travelled by radio,** sent or received, with its id. A received one is
//!   in no other store until its relay copy arrives, and the ids catch a replayed packet after a
//!   restart (#42).
//!
//! The file is small, and written whole on every change.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

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
    /// Outgoing: the other radio confirmed it.
    #[serde(default)]
    pub confirmed: bool,
    /// When this phone kept it, in milliseconds since the Unix epoch. It orders messages sent
    /// within the same second.
    #[serde(default)]
    pub kept_at: u64,
}

/// A contact's radio, as the address book holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactRadio {
    /// The radio's public key, in hex: its address on the mesh.
    pub public_key: String,
}

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
    /// Contact's account key → their radio.
    #[serde(default)]
    contacts: BTreeMap<String, ContactRadio>,
    /// Oldest first.
    #[serde(default)]
    messages: Vec<RadioMessage>,
}

pub struct RadioStore {
    path: PathBuf,
    data: Mutex<Data>,
}

impl RadioStore {
    /// The store in `data_dir`. A damaged file is kept aside as `radio.json.damaged`. What it
    /// held is lost, and so are the counters: no chat sends again in an epoch it may have sent
    /// in before.
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("radio.json");
        let data = match std::fs::read(&path) {
            Ok(json) => serde_json::from_slice(&json).unwrap_or_else(|error| {
                log::error!("the radio store is damaged, and its counters are lost: {error}");
                if let Err(error) = std::fs::rename(&path, path.with_extension("json.damaged")) {
                    log::warn!("could not keep the damaged radio store aside: {error}");
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
    pub fn contact_radio(&self, contact: &str) -> Option<ContactRadio> {
        self.lock().contacts.get(contact).cloned()
    }

    /// The contact whose radio has the public key `radio`.
    pub fn contact_with_radio(&self, radio: &str) -> Option<String> {
        self.lock()
            .contacts
            .iter()
            .find(|(_, known)| known.public_key == radio)
            .map(|(contact, _)| contact.clone())
    }

    pub fn set_contact_radio(&self, contact: &str, radio: ContactRadio) -> std::io::Result<()> {
        let mut data = self.lock();
        data.contacts.insert(contact.to_owned(), radio);
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

    /// Marks the sent message whose confirmation carries `ack` as confirmed, and returns it.
    /// Nothing for a code that isn't known, or a message that was confirmed already.
    pub fn confirm(&self, ack: &str) -> std::io::Result<Option<RadioMessage>> {
        let mut data = self.lock();
        let Some(message) = data
            .messages
            .iter_mut()
            .rev()
            .find(|message| message.ack.as_deref() == Some(ack) && !message.confirmed)
        else {
            return Ok(None);
        };
        message.confirmed = true;
        let confirmed = message.clone();
        self.save(&data)?;
        Ok(Some(confirmed))
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
        let mut file = std::fs::File::create(&temporary)?;
        std::io::Write::write_all(&mut file, &json)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &self.path)
    }

    fn lock(&self) -> MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(PoisonError::into_inner)
    }
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
            confirmed: false,
            kept_at: 1_795_300_000_100,
        }
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

    #[test]
    fn keeps_the_switch_the_contacts_and_the_messages() {
        let data = tempfile::tempdir().unwrap();
        let store = RadioStore::open(data.path());
        assert!(!store.forced());
        store.set_forced(true).unwrap();
        let radio = ContactRadio {
            public_key: "beef".to_owned(),
        };
        store.set_contact_radio("bea", radio.clone()).unwrap();
        assert!(store.add_message(sent("m1", "0001")).unwrap());
        assert!(!store.add_message(sent("m1", "0002")).unwrap());

        let reopened = RadioStore::open(data.path());
        assert!(reopened.forced());
        assert_eq!(reopened.contact_radio("bea"), Some(radio));
        assert_eq!(reopened.contact_with_radio("beef").as_deref(), Some("bea"));
        assert_eq!(reopened.contact_with_radio("cafe"), None);
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

        reopened.clear().unwrap();
        reopened.clear().unwrap();
        let cleared = RadioStore::open(data.path());
        assert!(!cleared.forced());
        assert!(cleared.message("m1").is_none());
        assert_eq!(cleared.take_counter("chat", 1).unwrap(), 0);
    }
}

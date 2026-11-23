//! Messages with no path yet (#46): written while no relay could be reached and the contact's
//! radio couldn't be either, or too long for the radio that could. The core holds them itself,
//! in `outbox.json` in the data folder, until a path returns: once MDK has a message, nothing
//! takes it back, and these can still be cancelled.
//!
//! Each has a provisional id until it goes. MDK's id for the message replaces it then.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use rand::RngCore;
use serde::{Deserialize, Serialize};

/// A message waiting for a path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queued {
    /// The provisional id, `queued-` and 16 hex digits.
    pub id: String,
    pub chat_id: String,
    pub text: String,
    /// When the user wrote it, in milliseconds since the Unix epoch.
    pub written_at: u64,
}

pub struct Outbox {
    path: PathBuf,
    /// Oldest first.
    list: Mutex<Vec<Queued>>,
}

impl Outbox {
    /// The outbox in `data_dir`. A file that can't be read is logged, and counts as empty.
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("outbox.json");
        let list = match std::fs::read(&path) {
            Ok(json) => serde_json::from_slice(&json).unwrap_or_else(|error| {
                log::warn!("could not read the waiting messages: {error}");
                Vec::new()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                log::warn!("could not read the waiting messages: {error}");
                Vec::new()
            }
        };
        Self {
            path,
            list: Mutex::new(list),
        }
    }

    /// Keeps `text` for the chat `chat_id`, written at `now_ms`.
    pub fn add(&self, chat_id: &str, text: &str, now_ms: u64) -> std::io::Result<Queued> {
        let mut bytes = [0u8; 8];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let queued = Queued {
            id: format!("queued-{}", hex::encode(bytes)),
            chat_id: chat_id.to_owned(),
            text: text.to_owned(),
            written_at: now_ms,
        };
        let mut list = self.lock();
        list.push(queued.clone());
        self.save(&list)?;
        Ok(queued)
    }

    /// The chat's waiting messages, oldest first.
    pub fn in_chat(&self, chat_id: &str) -> Vec<Queued> {
        self.lock()
            .iter()
            .filter(|queued| queued.chat_id == chat_id)
            .cloned()
            .collect()
    }

    pub fn has(&self, chat_id: &str) -> bool {
        self.lock().iter().any(|queued| queued.chat_id == chat_id)
    }

    /// Takes the message `id` out, as it goes or is cancelled. `None` if it isn't waiting.
    pub fn take(&self, id: &str) -> std::io::Result<Option<Queued>> {
        let mut list = self.lock();
        let Some(index) = list.iter().position(|queued| queued.id == id) else {
            return Ok(None);
        };
        let taken = list.remove(index);
        self.save(&list)?;
        Ok(Some(taken))
    }

    /// Puts a message back that couldn't go after all, in its place.
    pub fn put_back(&self, queued: Queued) -> std::io::Result<()> {
        let mut list = self.lock();
        let index = list
            .iter()
            .position(|known| known.written_at > queued.written_at)
            .unwrap_or(list.len());
        list.insert(index, queued);
        self.save(&list)
    }

    /// Forgets every waiting message, as when the account is deleted.
    pub fn clear(&self) -> std::io::Result<()> {
        self.lock().clear();
        match std::fs::remove_file(&self.path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    fn save(&self, list: &[Queued]) -> std::io::Result<()> {
        if let Some(folder) = self.path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let json = serde_json::to_vec(list).map_err(std::io::Error::other)?;
        // Written whole, then moved into place, so that a crash leaves the old file or the new.
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, json)?;
        std::fs::rename(&temporary, &self.path)
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Queued>> {
        self.list.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_order_across_a_restart_and_lets_messages_go_once() {
        let data = tempfile::tempdir().unwrap();
        let outbox = Outbox::open(data.path());
        let first = outbox.add("a", "First", 1).unwrap();
        let other = outbox.add("b", "Elsewhere", 2).unwrap();
        let second = outbox.add("a", "Second", 3).unwrap();
        assert!(first.id.starts_with("queued-") && first.id.len() == 23);
        assert_ne!(first.id, second.id);
        assert!(outbox.has("a") && outbox.has("b") && !outbox.has("c"));

        let reopened = Outbox::open(data.path());
        assert_eq!(reopened.in_chat("a"), vec![first.clone(), second.clone()]);
        assert_eq!(reopened.in_chat("b"), vec![other]);

        // Taken once, as it goes or is cancelled.
        assert_eq!(reopened.take(&first.id).unwrap(), Some(first.clone()));
        assert_eq!(reopened.take(&first.id).unwrap(), None);
        assert_eq!(reopened.in_chat("a"), vec![second.clone()]);
        // One that couldn't go after all goes back in its place.
        reopened.put_back(first.clone()).unwrap();
        assert_eq!(Outbox::open(data.path()).in_chat("a"), vec![first, second]);

        reopened.clear().unwrap();
        reopened.clear().unwrap();
        assert!(!Outbox::open(data.path()).has("a"));
    }

    #[test]
    fn a_damaged_file_counts_as_empty() {
        let data = tempfile::tempdir().unwrap();
        std::fs::write(data.path().join("outbox.json"), b"[{").unwrap();
        assert!(Outbox::open(data.path()).in_chat("a").is_empty());
    }
}

//! Messages with no path yet (#46): written while no relay could be reached and the contact's
//! radio couldn't be either, or too long for the radio that could. The core holds them itself,
//! in `outbox.json` in the data folder, until a path returns: once MDK has a message, nothing
//! takes it back, and these can still be cancelled.
//!
//! Their text is encrypted with the account's data key, and the file is readable by its owner
//! only (#127). A message leaves the file as it goes or is cancelled.
//!
//! Each has a provisional id until it goes. MDK's id for the message replaces it then.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::private_file;
use crate::vault::{Sealed, Vault};

/// A message waiting for a path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Queued {
    /// The provisional id, `queued-` and 16 hex digits.
    pub id: String,
    pub chat_id: String,
    pub text: String,
    /// When the user wrote it, in milliseconds since the Unix epoch.
    pub written_at: u64,
}

/// A waiting message as the file keeps it.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Kept {
    id: String,
    chat_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sealed: Option<Sealed>,
    /// The text of a message queued before #127, until it is sealed.
    #[serde(default, rename = "text", skip_serializing_if = "Option::is_none")]
    plain: Option<String>,
    written_at: u64,
}

pub struct Outbox {
    path: PathBuf,
    vault: Arc<Vault>,
    /// Oldest first.
    list: Mutex<Vec<Kept>>,
}

impl Outbox {
    /// The outbox in `data_dir`. A file that can't be read is logged, and counts as empty. A
    /// damaged one is removed: it may hold text from before it was encrypted.
    pub(crate) fn open(data_dir: &Path, vault: Arc<Vault>) -> Self {
        let path = data_dir.join("outbox.json");
        // Left over from a crash.
        if let Err(error) = private_file::remove(&path.with_extension("json.tmp")) {
            log::warn!("could not remove a leftover of the waiting messages: {error}");
        }
        private_file::restrict(&path);
        let list = match std::fs::read(&path) {
            Ok(json) => serde_json::from_slice(&json).unwrap_or_else(|error| {
                log::warn!("could not read the waiting messages: {error}");
                if let Err(error) = private_file::remove(&path) {
                    log::warn!("could not remove the damaged waiting messages: {error}");
                }
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
            vault,
            list: Mutex::new(list),
        }
    }

    /// Encrypts the text of messages queued before it was kept encrypted. What can't be
    /// encrypted now stays as it is, for the next start.
    pub fn seal_plain(&self) -> std::io::Result<()> {
        let mut list = self.lock();
        let mut changed = false;
        for kept in list.iter_mut() {
            let Some(plain) = &kept.plain else { continue };
            kept.sealed = Some(self.vault.seal(plain, &kept.id)?);
            kept.plain = None;
            changed = true;
        }
        if changed {
            self.save(&list)?;
        }
        Ok(())
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
        let kept = self.seal(&queued)?;
        let mut list = self.lock();
        list.push(kept);
        self.save(&list)?;
        Ok(queued)
    }

    /// The chat's waiting messages, oldest first. One whose text can't be decrypted now is left
    /// out, and stays in the file.
    pub fn in_chat(&self, chat_id: &str) -> Vec<Queued> {
        self.lock()
            .iter()
            .filter(|kept| kept.chat_id == chat_id)
            .filter_map(|kept| self.open_kept(kept))
            .collect()
    }

    pub fn has(&self, chat_id: &str) -> bool {
        self.lock().iter().any(|kept| kept.chat_id == chat_id)
    }

    /// Takes the message `id` out, as it goes or is cancelled. `None` if it isn't waiting.
    pub fn take(&self, id: &str) -> std::io::Result<Option<Queued>> {
        let mut list = self.lock();
        let Some((index, taken)) = list
            .iter()
            .enumerate()
            .find(|(_, kept)| kept.id == id)
            .and_then(|(index, kept)| Some((index, self.open_kept(kept)?)))
        else {
            return Ok(None);
        };
        list.remove(index);
        self.save(&list)?;
        Ok(Some(taken))
    }

    /// Puts a message back that couldn't go after all, in its place.
    pub fn put_back(&self, queued: Queued) -> std::io::Result<()> {
        let kept = self.seal(&queued)?;
        let mut list = self.lock();
        let index = list
            .iter()
            .position(|known| known.written_at > kept.written_at)
            .unwrap_or(list.len());
        list.insert(index, kept);
        self.save(&list)
    }

    /// Forgets every waiting message, as when the account is deleted.
    pub fn clear(&self) -> std::io::Result<()> {
        self.lock().clear();
        private_file::remove(&self.path.with_extension("json.tmp"))?;
        private_file::remove(&self.path)
    }

    fn seal(&self, queued: &Queued) -> std::io::Result<Kept> {
        Ok(Kept {
            id: queued.id.clone(),
            chat_id: queued.chat_id.clone(),
            sealed: Some(self.vault.seal(&queued.text, &queued.id)?),
            plain: None,
            written_at: queued.written_at,
        })
    }

    fn open_kept(&self, kept: &Kept) -> Option<Queued> {
        let text = match (&kept.sealed, &kept.plain) {
            (Some(sealed), _) => self.vault.open(sealed, &kept.id)?,
            (None, plain) => plain.clone()?,
        };
        Some(Queued {
            id: kept.id.clone(),
            chat_id: kept.chat_id.clone(),
            text,
            written_at: kept.written_at,
        })
    }

    fn save(&self, list: &[Kept]) -> std::io::Result<()> {
        let json = serde_json::to_vec(list).map_err(std::io::Error::other)?;
        private_file::replace(&self.path, &json)
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Kept>> {
        self.list.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::test_vault;

    fn open(folder: &Path) -> Outbox {
        Outbox::open(folder, test_vault(folder))
    }

    #[test]
    fn keeps_the_order_across_a_restart_and_lets_messages_go_once() {
        let data = tempfile::tempdir().unwrap();
        let outbox = open(data.path());
        let first = outbox.add("a", "First", 1).unwrap();
        let other = outbox.add("b", "Elsewhere", 2).unwrap();
        let second = outbox.add("a", "Second", 3).unwrap();
        assert!(first.id.starts_with("queued-") && first.id.len() == 23);
        assert_ne!(first.id, second.id);
        assert!(outbox.has("a") && outbox.has("b") && !outbox.has("c"));

        let reopened = open(data.path());
        assert_eq!(reopened.in_chat("a"), vec![first.clone(), second.clone()]);
        assert_eq!(reopened.in_chat("b"), vec![other]);

        // Taken once, as it goes or is cancelled.
        assert_eq!(reopened.take(&first.id).unwrap(), Some(first.clone()));
        assert_eq!(reopened.take(&first.id).unwrap(), None);
        assert_eq!(reopened.in_chat("a"), vec![second.clone()]);
        // One that couldn't go after all goes back in its place.
        reopened.put_back(first.clone()).unwrap();
        assert_eq!(open(data.path()).in_chat("a"), vec![first, second]);

        reopened.clear().unwrap();
        reopened.clear().unwrap();
        assert!(!open(data.path()).has("a"));
    }

    #[test]
    fn issue_127_waiting_text_is_not_in_the_file_in_plain() {
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("outbox.json");
        // As an earlier version kept it.
        std::fs::write(
            &path,
            br#"[{"id":"queued-1","chat_id":"a","text":"Old secret","written_at":1}]"#,
        )
        .unwrap();
        let outbox = open(data.path());
        outbox.seal_plain().unwrap();
        outbox.add("a", "New secret", 2).unwrap();
        let file = std::fs::read_to_string(&path).unwrap();
        assert!(!file.contains("secret"), "{file}");
        let texts: Vec<String> = open(data.path())
            .in_chat("a")
            .into_iter()
            .map(|queued| queued.text)
            .collect();
        assert_eq!(texts, ["Old secret", "New secret"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_damaged_file_counts_as_empty() {
        let data = tempfile::tempdir().unwrap();
        std::fs::write(data.path().join("outbox.json"), b"[{").unwrap();
        assert!(open(data.path()).in_chat("a").is_empty());
        assert!(!data.path().join("outbox.json").exists());
    }
}

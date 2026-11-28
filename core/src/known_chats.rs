//! The direct chats the core has shown, and with whom (#125): a chat id and the contact's public
//! key, nothing more, in `chats.json` in the data folder.
//!
//! MDK knows a direct chat by its state: no name and two accounts. Once the contact leaves, or
//! the chat gets a name or a third member, MDK no longer says who the chat was with. The core
//! keeps following the chat by its id, and after a restart this file says which of the account's
//! chats were direct ones, and with whom.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct KnownChats {
    path: PathBuf,
    /// Chat id to the contact's public key, both in hex.
    chats: Mutex<BTreeMap<String, String>>,
}

impl KnownChats {
    /// The chats kept in `data_dir`. A file that can't be read is logged, and counts as none.
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("chats.json");
        let chats = match std::fs::read(&path) {
            Ok(json) => serde_json::from_slice(&json).unwrap_or_else(|error| {
                log::warn!("could not read the known chats: {error}");
                BTreeMap::new()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => {
                log::warn!("could not read the known chats: {error}");
                BTreeMap::new()
            }
        };
        Self {
            path,
            chats: Mutex::new(chats),
        }
    }

    /// The contact of the chat `chat_id`, if the core has shown it as a direct chat.
    pub fn contact(&self, chat_id: &str) -> Option<String> {
        self.lock().get(chat_id).cloned()
    }

    /// Remembers that `chat_id` is a direct chat with `contact`. Kept at once if it is new.
    pub fn remember(&self, chat_id: &str, contact: &str) {
        let mut chats = self.lock();
        if chats.get(chat_id).is_some_and(|known| known == contact) {
            return;
        }
        chats.insert(chat_id.to_owned(), contact.to_owned());
        if let Err(error) = self.save(&chats) {
            log::warn!("could not keep a chat: {error}");
        }
    }

    /// Forgets every chat, as when the account is deleted.
    pub fn clear(&self) -> std::io::Result<()> {
        self.lock().clear();
        match std::fs::remove_file(&self.path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    fn save(&self, chats: &BTreeMap<String, String>) -> std::io::Result<()> {
        if let Some(folder) = self.path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let json = serde_json::to_vec_pretty(chats).map_err(std::io::Error::other)?;
        // Written whole, then moved into place, so that a crash leaves the old file or the new.
        // Readable by the owner only, as MDK's own files are.
        let temporary = self.path.with_extension("json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options.open(&temporary)?.write_all(&json)?;
        std::fs::rename(&temporary, &self.path)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, String>> {
        self.chats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_125_known_chats_stay_across_a_restart() {
        let data = tempfile::tempdir().unwrap();
        let chats = KnownChats::open(data.path());
        assert_eq!(chats.contact("c1"), None);
        chats.remember("c1", "k1");
        chats.remember("c2", "k2");
        chats.remember("c1", "k1");

        let reopened = KnownChats::open(data.path());
        assert_eq!(reopened.contact("c1").as_deref(), Some("k1"));
        assert_eq!(reopened.contact("c2").as_deref(), Some("k2"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(data.path().join("chats.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        reopened.clear().unwrap();
        reopened.clear().unwrap();
        assert_eq!(KnownChats::open(data.path()).contact("c1"), None);
    }

    #[test]
    fn a_damaged_file_counts_as_no_chats() {
        let data = tempfile::tempdir().unwrap();
        std::fs::write(data.path().join("chats.json"), b"not json").unwrap();
        assert_eq!(KnownChats::open(data.path()).contact("c1"), None);
    }
}

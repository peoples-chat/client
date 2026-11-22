//! The invitations the user sent (#95): a personal link per person, which works once and for 7
//! days. They live on this device only, in `invites.json` in the data folder; no server knows
//! about them. The link carries a token, which the invited person's app sends back in the new
//! chat. The token then opens that chat at once, without a request.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rand::RngCore;
use serde::{Deserialize, Serialize};

/// How long an invitation works.
pub const INVITE_LIFETIME: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// An invitation the user sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteRecord {
    /// The token in the link, and the invitation's id.
    pub token: String,
    /// Whom it is for. Only the user sees it.
    pub name: String,
    /// Milliseconds since the Unix epoch.
    pub created_at: u64,
    pub expires_at: u64,
    /// The person who used it, once they have.
    #[serde(default)]
    pub joined: Option<Joined>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Joined {
    pub public_key: String,
    pub chat_id: String,
    pub at: u64,
}

pub struct Invites {
    path: PathBuf,
    list: Mutex<Vec<InviteRecord>>,
}

impl Invites {
    /// The invitations kept in `data_dir`. A file that can't be read is logged, and counts as
    /// none.
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("invites.json");
        let list = match std::fs::read(&path) {
            Ok(json) => serde_json::from_slice(&json).unwrap_or_else(|error| {
                log::warn!("could not read the invitations: {error}");
                Vec::new()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                log::warn!("could not read the invitations: {error}");
                Vec::new()
            }
        };
        Self {
            path,
            list: Mutex::new(list),
        }
    }

    pub fn all(&self) -> Vec<InviteRecord> {
        self.lock().clone()
    }

    /// A new invitation for `name`, with a fresh token, kept at once.
    pub fn create(&self, name: &str, now_ms: u64) -> std::io::Result<InviteRecord> {
        let mut bytes = [0u8; 10];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let invite = InviteRecord {
            token: hex::encode(bytes),
            name: name.to_owned(),
            created_at: now_ms,
            expires_at: now_ms + INVITE_LIFETIME.as_millis() as u64,
            joined: None,
        };
        let mut list = self.lock();
        list.push(invite.clone());
        self.save(&list)?;
        Ok(invite)
    }

    /// Takes a token that came back in the chat `chat_id` from `public_key`. Returns the
    /// invitation it opens: one that nobody has used yet and that hasn't expired. A token that
    /// doesn't open one changes nothing; that chat stays an ordinary request.
    pub fn redeem(
        &self,
        token: &str,
        public_key: &str,
        chat_id: &str,
        now_ms: u64,
    ) -> Option<InviteRecord> {
        let mut list = self.lock();
        let invite = list.iter_mut().find(|invite| invite.token == token)?;
        if invite.joined.is_some() || now_ms >= invite.expires_at {
            return None;
        }
        invite.joined = Some(Joined {
            public_key: public_key.to_owned(),
            chat_id: chat_id.to_owned(),
            at: now_ms,
        });
        let redeemed = invite.clone();
        if let Err(error) = self.save(&list) {
            log::warn!("could not keep an invitation's use: {error}");
        }
        Some(redeemed)
    }

    /// Forgets every invitation, as when the account is deleted.
    pub fn clear(&self) -> std::io::Result<()> {
        self.lock().clear();
        match std::fs::remove_file(&self.path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    fn save(&self, list: &[InviteRecord]) -> std::io::Result<()> {
        if let Some(folder) = self.path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let json = serde_json::to_vec_pretty(list).map_err(std::io::Error::other)?;
        // Written whole, then moved into place, so that a crash leaves the old file or the new.
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, json)?;
        std::fs::rename(&temporary, &self.path)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<InviteRecord>> {
        self.list
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 24 * 60 * 60 * 1000;

    #[test]
    fn an_invitation_works_once_and_for_seven_days() {
        let data = tempfile::tempdir().unwrap();
        let invites = Invites::open(data.path());
        let sofie = invites.create("Sofie", 1_000).unwrap();
        let jonas = invites.create("Jonas", 1_000).unwrap();
        assert_ne!(sofie.token, jonas.token);
        assert_eq!(sofie.token.len(), 20);
        assert_eq!(sofie.expires_at, 1_000 + 7 * DAY);

        // An unknown token opens nothing.
        assert_eq!(invites.redeem("0000000000", "k1", "c1", 2_000), None);
        // Sofie's opens her invitation, once.
        let used = invites.redeem(&sofie.token, "k1", "c1", 2_000).unwrap();
        assert_eq!(used.joined.unwrap().chat_id, "c1");
        assert_eq!(invites.redeem(&sofie.token, "k2", "c2", 3_000), None);
        // Jonas's has expired by the time it comes back.
        assert_eq!(
            invites.redeem(&jonas.token, "k3", "c3", 1_000 + 7 * DAY),
            None
        );

        // All of it stays across a restart.
        let reopened = Invites::open(data.path());
        assert_eq!(reopened.all(), invites.all());
        assert!(reopened.all()[0].joined.is_some());
        assert!(reopened.all()[1].joined.is_none());

        reopened.clear().unwrap();
        reopened.clear().unwrap();
        assert!(Invites::open(data.path()).all().is_empty());
    }

    #[test]
    fn a_damaged_file_counts_as_no_invitations() {
        let data = tempfile::tempdir().unwrap();
        std::fs::write(data.path().join("invites.json"), b"not json").unwrap();
        assert!(Invites::open(data.path()).all().is_empty());
    }
}

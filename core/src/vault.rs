//! Encryption for the message text the core keeps outside MDK (#127): a received radio message
//! until MDK has its relay copy, and messages that wait for a path.
//!
//! The data key is random, one per account, and lives where the account key does (#115): the
//! platform's key store, or a file in the data folder in development builds on computers. It has
//! nothing to do with the account key. MDK's store trait keeps secp256k1 keys, so the data key is
//! made as one, and the cipher's key is derived from it. Deleting the account removes it.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use marmot_account::{AccountSecretStore, AccountSummary};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroizing;

const NONCE_BYTES: usize = 12;
/// Separates the cipher's key from any other use of the stored secret.
const KEY_INFO: &[u8] = b"peoples.chat local data v1";

/// Text as it is kept on disk: a random nonce and the encrypted text, in base64.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Sealed(String);

type DataKey = Zeroizing<[u8; 32]>;

struct Current {
    account_hex: String,
    /// Kept once loaded, so that the key store is asked once.
    key: Option<DataKey>,
}

/// Seals and opens text with the data key of the account on this device.
pub(crate) struct Vault {
    secrets: Arc<dyn AccountSecretStore>,
    current: Mutex<Option<Current>>,
}

impl Vault {
    pub(crate) fn new(secrets: Arc<dyn AccountSecretStore>) -> Self {
        Self {
            secrets,
            current: Mutex::new(None),
        }
    }

    /// The account whose data key to use from now on.
    pub(crate) fn use_account(&self, account_hex: &str) {
        *self.current() = Some(Current {
            account_hex: account_hex.to_owned(),
            key: None,
        });
    }

    /// Encrypts `text`. `context` names where it belongs, such as the message's id: it opens only
    /// with the same. Makes the account's data key the first time.
    pub(crate) fn seal(&self, text: &str, context: &str) -> std::io::Result<Sealed> {
        let key = self
            .key(true)?
            .ok_or_else(|| std::io::Error::other("there is no data key"))?;
        let mut nonce = [0u8; NONCE_BYTES];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let encrypted = cipher(&key)
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: text.as_bytes(),
                    aad: context.as_bytes(),
                },
            )
            .map_err(|_| std::io::Error::other("could not encrypt"))?;
        Ok(Sealed(BASE64.encode([&nonce[..], &encrypted].concat())))
    }

    /// The text in `sealed`, or `None` if it can't be had: the key is gone or can't be read, or
    /// the text was changed.
    pub(crate) fn open(&self, sealed: &Sealed, context: &str) -> Option<String> {
        let key = match self.key(false) {
            Ok(Some(key)) => key,
            Ok(None) => {
                log::warn!("a kept message can't be read: its key is gone");
                return None;
            }
            Err(error) => {
                log::warn!("could not read the data key: {error}");
                return None;
            }
        };
        let bytes = BASE64.decode(&sealed.0).ok()?;
        let (nonce, encrypted) = bytes.split_at_checked(NONCE_BYTES)?;
        let text = cipher(&key)
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: encrypted,
                    aad: context.as_bytes(),
                },
            )
            .ok()?;
        String::from_utf8(text).ok()
    }

    /// Removes the account's data key, as when the account is deleted.
    pub(crate) fn forget(&self) -> std::io::Result<()> {
        let Some(current) = self.current().take() else {
            return Ok(());
        };
        let entry = entry(&current.account_hex);
        if has(self.secrets.as_ref(), &entry)? {
            self.secrets
                .remove_secret(&entry)
                .map_err(std::io::Error::other)?;
        }
        Ok(())
    }

    /// The account's data key. With `create`, one is made if there is none. A key that is there
    /// but can't be read is an error, never replaced.
    fn key(&self, create: bool) -> std::io::Result<Option<DataKey>> {
        let mut current = self.current();
        let Some(current) = current.as_mut() else {
            return Ok(None);
        };
        if let Some(key) = &current.key {
            return Ok(Some(key.clone()));
        }
        let entry = entry(&current.account_hex);
        let secret = if has(self.secrets.as_ref(), &entry)? {
            self.secrets
                .load_secret(&entry)
                .map_err(std::io::Error::other)?
        } else if create {
            let secret = nostr::prelude::Keys::generate();
            self.secrets
                .write_secret(&entry, &secret)
                .map_err(std::io::Error::other)?;
            secret
        } else {
            return Ok(None);
        };
        let mut key = Zeroizing::new([0u8; 32]);
        Hkdf::<Sha256>::new(None, secret.secret_key().as_secret_bytes())
            .expand(KEY_INFO, key.as_mut_slice())
            .map_err(|_| std::io::Error::other("could not derive the data key"))?;
        current.key = Some(key.clone());
        Ok(Some(key))
    }

    fn current(&self) -> MutexGuard<'_, Option<Current>> {
        self.current.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn cipher(key: &DataKey) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new(Key::from_slice(key.as_slice()))
}

/// Where the data key of the account `account_hex` is kept. MDK's stores name an entry after an
/// account: this one is no account's, so MDK never lists it.
fn entry(account_hex: &str) -> AccountSummary {
    let name = format!("local-data-{account_hex}");
    AccountSummary {
        label: name.clone(),
        account_id_hex: name,
        local_signing: false,
        external_signing: false,
        signed_out: false,
        unlinked: false,
    }
}

/// Whether `store` has `entry`. The file store goes by the label, the key store by the id.
fn has(store: &dyn AccountSecretStore, entry: &AccountSummary) -> std::io::Result<bool> {
    Ok(store
        .has_secret_for_label(&entry.label)
        .map_err(std::io::Error::other)?
        || store
            .has_secret_for_account_id(&entry.account_id_hex)
            .map_err(std::io::Error::other)?)
}

/// Moves the data key of the account `account_hex` into `to`, if it is in `from` and not yet in
/// `to` (#115). It is removed from `from` only once `to` reads it back.
pub(crate) fn move_data_key(
    account_hex: &str,
    from: &dyn AccountSecretStore,
    to: &dyn AccountSecretStore,
) -> std::io::Result<()> {
    let entry = entry(account_hex);
    if has(to, &entry)? || !has(from, &entry)? {
        return Ok(());
    }
    let secret = from.load_secret(&entry).map_err(std::io::Error::other)?;
    to.write_secret(&entry, &secret)
        .map_err(std::io::Error::other)?;
    let moved = to.load_secret(&entry).map_err(std::io::Error::other)?;
    if moved.public_key() != secret.public_key() {
        return Err(std::io::Error::other(
            "the moved key reads back differently",
        ));
    }
    from.remove_secret(&entry).map_err(std::io::Error::other)
}

/// A vault for tests, with its key in a file under `folder`.
#[cfg(test)]
pub(crate) fn test_vault(folder: &std::path::Path) -> Arc<Vault> {
    let secrets = Arc::new(marmot_account::LocalFileSecretStore::new(folder));
    let vault = Arc::new(Vault::new(secrets));
    vault.use_account(&"ab".repeat(32));
    vault
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_127_sealed_text_opens_only_with_its_key_and_context() {
        let data = tempfile::tempdir().unwrap();
        let vault = test_vault(data.path());
        let sealed = vault.seal("Meet at the mill", "m1").unwrap();
        assert!(!serde_json::to_string(&sealed).unwrap().contains("mill"));
        assert_ne!(sealed, vault.seal("Meet at the mill", "m1").unwrap());
        assert_eq!(
            vault.open(&sealed, "m1").as_deref(),
            Some("Meet at the mill")
        );
        assert_eq!(vault.open(&sealed, "m2"), None);

        // The key stays across a restart.
        let restarted = test_vault(data.path());
        assert_eq!(
            restarted.open(&sealed, "m1").as_deref(),
            Some("Meet at the mill")
        );

        // Once forgotten, as with the account, nothing opens it, and the next key is another.
        restarted.forget().unwrap();
        let after = test_vault(data.path());
        assert_eq!(after.open(&sealed, "m1"), None);
        after.seal("Something else", "m3").unwrap();
        assert_eq!(after.open(&sealed, "m1"), None);
    }

    #[test]
    fn issue_127_the_data_key_moves_with_the_account_key() {
        let one = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let account = "ab".repeat(32);
        let sealed = test_vault(one.path()).seal("Hello", "m1").unwrap();
        let from = marmot_account::LocalFileSecretStore::new(one.path());
        let to = marmot_account::LocalFileSecretStore::new(other.path());
        move_data_key(&account, &from, &to).unwrap();
        // Again changes nothing.
        move_data_key(&account, &from, &to).unwrap();
        assert_eq!(
            test_vault(other.path()).open(&sealed, "m1").as_deref(),
            Some("Hello")
        );
        assert_eq!(test_vault(one.path()).open(&sealed, "m1"), None);
    }
}

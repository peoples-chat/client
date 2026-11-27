//! Keys for radio messages in a direct chat (#37), derived from the chat's MLS state so that no
//! extra key exchange is needed.
//!
//! Both members hold the same exporter secret in the same epoch ([`crate::Messaging::chat_secret`]).
//! Each derives one key per direction from it, with HKDF-SHA256 and a label of the app's own:
//!
//! ```text
//! key(sender) = HKDF-SHA256(salt = none, IKM = exporter secret, info = LABEL || sender's public key)
//! ```
//!
//! A key changes with every epoch. Separate keys for the two directions keep the two senders'
//! nonces apart, and a packet reflected back to its sender fails to decrypt. The finding behind
//! this, with the epoch and nonce rules: `docs/spikes/37-radio-keys.md`.

use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

/// The HKDF label. Provisional and unregistered: Marmot assigns its exporter contexts only when a
/// spec is adopted, and this one has no spec yet.
pub const RADIO_KEY_LABEL: &[u8] = b"peoples.chat/radio/v0";

/// Key length in bytes, for a 256-bit AEAD such as ChaCha20-Poly1305.
pub const RADIO_KEY_BYTES: usize = 32;

/// A key for one direction of a chat in one epoch. Wiped when dropped.
pub type RadioKey = Zeroizing<[u8; RADIO_KEY_BYTES]>;

/// The radio keys of one direct chat in one MLS epoch.
pub struct RadioKeys {
    pub epoch: u64,
    /// Encrypts what this phone sends.
    pub send: RadioKey,
    /// Decrypts what the contact sends.
    pub receive: RadioKey,
}

impl std::fmt::Debug for RadioKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RadioKeys")
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum RadioKeyError {
    #[error("the chat's secret is {0} bytes, fewer than {RADIO_KEY_BYTES}")]
    ShortSecret(usize),
    #[error("{0:?} is not a public key")]
    InvalidPublicKey(String),
    #[error("a chat with oneself has no direction")]
    SameKey,
}

impl RadioKeys {
    /// The keys for the chat whose exporter secret in `epoch` is `chat_secret`, between the
    /// account `own_public_key` and the contact `contact_public_key` (hex, 32 bytes).
    pub fn derive(
        chat_secret: &[u8],
        epoch: u64,
        own_public_key: &str,
        contact_public_key: &str,
    ) -> Result<Self, RadioKeyError> {
        if chat_secret.len() < RADIO_KEY_BYTES {
            return Err(RadioKeyError::ShortSecret(chat_secret.len()));
        }
        let own = public_key(own_public_key)?;
        let contact = public_key(contact_public_key)?;
        if own == contact {
            return Err(RadioKeyError::SameKey);
        }
        Ok(Self {
            epoch,
            send: direction_key(chat_secret, &own),
            receive: direction_key(chat_secret, &contact),
        })
    }
}

/// The key for what `sender` sends.
fn direction_key(chat_secret: &[u8], sender: &[u8; 32]) -> RadioKey {
    let hkdf = Hkdf::<Sha256>::new(None, chat_secret);
    let mut info = [0; RADIO_KEY_LABEL.len() + 32];
    info[..RADIO_KEY_LABEL.len()].copy_from_slice(RADIO_KEY_LABEL);
    info[RADIO_KEY_LABEL.len()..].copy_from_slice(sender);
    let mut key = Zeroizing::new([0; RADIO_KEY_BYTES]);
    hkdf.expand(&info, key.as_mut())
        .expect("32 bytes is within HKDF-SHA256's output limit");
    key
}

fn public_key(hex_key: &str) -> Result<[u8; 32], RadioKeyError> {
    let mut key = [0; 32];
    hex::decode_to_slice(hex_key, &mut key)
        .map_err(|_| RadioKeyError::InvalidPublicKey(hex_key.to_owned()))?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const BEA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn secret() -> Vec<u8> {
        (0..32).collect()
    }

    /// Computed separately, with Python's `hmac` and `hashlib` (RFC 5869), from the secret
    /// 00 01 … 1f, the label and the sender's key.
    #[test]
    fn matches_the_test_vector() {
        let keys = RadioKeys::derive(&secret(), 7, ADA, BEA).unwrap();
        assert_eq!(keys.epoch, 7);
        assert_eq!(
            hex::encode(*keys.send),
            "805f65bf8627f9d3a3849c2e330d9c6f9f5936495b5212c5597526bcda48ead7"
        );
        assert_eq!(
            hex::encode(*keys.receive),
            "05e43f1c7156b5f6b36d01742b86cf32a9753e57767d2e1bbc6ee99158d62d5d"
        );
    }

    #[test]
    fn each_side_receives_with_the_key_the_other_sends_with() {
        let ada = RadioKeys::derive(&secret(), 7, ADA, BEA).unwrap();
        let bea = RadioKeys::derive(&secret(), 7, BEA, ADA).unwrap();
        assert_eq!(*ada.send, *bea.receive);
        assert_eq!(*ada.receive, *bea.send);
        assert_ne!(*ada.send, *ada.receive);
    }

    #[test]
    fn another_epoch_s_secret_gives_other_keys() {
        let mut next = secret();
        next[0] ^= 1;
        let now = RadioKeys::derive(&secret(), 7, ADA, BEA).unwrap();
        let later = RadioKeys::derive(&next, 8, ADA, BEA).unwrap();
        assert_ne!(*now.send, *later.send);
        assert_ne!(*now.receive, *later.receive);
    }

    #[test]
    fn says_what_is_wrong() {
        assert_eq!(
            RadioKeys::derive(&[0; 16], 7, ADA, BEA).unwrap_err(),
            RadioKeyError::ShortSecret(16)
        );
        assert_eq!(
            RadioKeys::derive(&secret(), 7, "npub1…", BEA).unwrap_err(),
            RadioKeyError::InvalidPublicKey("npub1…".to_owned())
        );
        assert_eq!(
            RadioKeys::derive(&secret(), 7, ADA, ADA).unwrap_err(),
            RadioKeyError::SameKey
        );
    }

    #[test]
    fn keys_stay_out_of_logs() {
        let keys = RadioKeys::derive(&secret(), 7, ADA, BEA).unwrap();
        let printed = format!("{keys:?}");
        assert!(!printed.contains("805f65"), "{printed}");
        assert!(printed.contains("epoch: 7"), "{printed}");
    }
}

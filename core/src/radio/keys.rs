//! Keys for radio messages in a direct chat. Each member's app makes a random key of its own for
//! the chat and sends it there, inside the chat's MLS messages, as an app event of the app's own
//! kind ([`crate::messaging::RADIO_KEY_KIND`]). The key encrypts what that member sends by radio,
//! and nothing else. The decision (#128): <https://github.com/peoples-chat/client/issues/128>.
//!
//! The event's content is a small, versioned record:
//!
//! ```json
//! {"v":0,"mesh":"meshcore","key":"…64 hex…","n":1,"at":1795300000000}
//! ```
//!
//! - `mesh` is the radio protocol the key is for, spelled as in the radio address record (#34):
//!   `meshcore` or `meshtastic`. A record with a mesh the app doesn't know is kept and not used.
//! - `n` numbers the sender's keys for the chat and mesh. A packet carries its lowest 8 bits, to
//!   say which key sealed it.
//! - `at` is when the sender made the key, in milliseconds since the Unix epoch, by the sender's
//!   clock. The newest key is the member's current one for that mesh.
//!
//! Fields a later version adds are ignored; a record of another version is refused. The rules for
//! packets are in `docs/radio-format-v0.md`, "Where the keys come from".

use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

/// The record's version.
pub const RADIO_KEY_VERSION: u8 = 0;
/// Key length in bytes, for a 256-bit AEAD such as ChaCha20-Poly1305.
pub const RADIO_KEY_BYTES: usize = 32;
/// The longest record taken, in bytes. A record is about 130.
const MAX_RECORD_BYTES: usize = 1024;
/// The longest mesh name taken, in bytes.
const MAX_MESH_BYTES: usize = 32;

/// A key for what one member sends in one chat over one mesh. Wiped when dropped.
pub type RadioKey = Zeroizing<[u8; RADIO_KEY_BYTES]>;

/// A new random key.
pub fn new_radio_key() -> RadioKey {
    let mut key = Zeroizing::new([0; RADIO_KEY_BYTES]);
    OsRng.fill_bytes(key.as_mut());
    key
}

/// A member's radio key, as the chat carries it and the radio store keeps it. `Debug` leaves the
/// key out.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadioKeyRecord {
    pub v: u8,
    /// The radio protocol the key is for. Any name is kept; only known ones are used.
    pub mesh: String,
    /// The key, 64 hex digits.
    pub key: String,
    /// The key's number among the sender's keys for the chat and mesh.
    pub n: u64,
    /// When the key was made, in milliseconds since the Unix epoch.
    pub at: u64,
}

impl std::fmt::Debug for RadioKeyRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RadioKeyRecord")
            .field("v", &self.v)
            .field("mesh", &self.mesh)
            .field("n", &self.n)
            .field("at", &self.at)
            .finish_non_exhaustive()
    }
}

impl Drop for RadioKeyRecord {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

/// Why a record was refused. None of these holds any of the record's text: it holds a key.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum RadioKeyError {
    #[error("the record is not JSON of the known shape")]
    Malformed,
    #[error("record version {0} is unknown")]
    UnknownVersion(u8),
    #[error("the mesh's name is empty or too long")]
    InvalidMesh,
    #[error("the key is not 64 hex digits")]
    InvalidKey,
}

impl RadioKeyRecord {
    /// The record of `key`, number `n` for `mesh`, made at `at`.
    pub fn new(mesh: &str, key: &RadioKey, n: u64, at: u64) -> Self {
        Self {
            v: RADIO_KEY_VERSION,
            mesh: mesh.to_owned(),
            key: hex::encode(key.as_slice()),
            n,
            at,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a record always serializes")
    }

    /// Reads a record that came in a chat. A mesh the app doesn't know is not an error.
    pub fn parse(json: &str) -> Result<Self, RadioKeyError> {
        if json.len() > MAX_RECORD_BYTES {
            return Err(RadioKeyError::Malformed);
        }
        // The version first, so that a later version isn't reported as malformed.
        #[derive(Deserialize)]
        struct Version {
            v: u8,
        }
        let version: Version = serde_json::from_str(json).map_err(|_| RadioKeyError::Malformed)?;
        if version.v != RADIO_KEY_VERSION {
            return Err(RadioKeyError::UnknownVersion(version.v));
        }
        let mut record: Self = serde_json::from_str(json).map_err(|_| RadioKeyError::Malformed)?;
        if record.mesh.is_empty() || record.mesh.len() > MAX_MESH_BYTES {
            return Err(RadioKeyError::InvalidMesh);
        }
        if record.radio_key().is_none() {
            return Err(RadioKeyError::InvalidKey);
        }
        record.key.make_ascii_lowercase();
        Ok(record)
    }

    /// The key itself, if the record's is one.
    pub fn radio_key(&self) -> Option<RadioKey> {
        let mut key = Zeroizing::new([0; RADIO_KEY_BYTES]);
        hex::decode_to_slice(&self.key, key.as_mut()).ok()?;
        Some(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> RadioKey {
        Zeroizing::new(std::array::from_fn(|byte| byte as u8))
    }

    const HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    #[test]
    fn a_record_is_small_and_reads_back() {
        let record = RadioKeyRecord::new("meshcore", &key(), 1, 1_795_300_000_000);
        let json = record.to_json();
        assert_eq!(
            json,
            format!(r#"{{"v":0,"mesh":"meshcore","key":"{HEX}","n":1,"at":1795300000000}}"#)
        );
        let read = RadioKeyRecord::parse(&json).unwrap();
        assert_eq!(read, record);
        assert_eq!(*read.radio_key().unwrap(), *key());
    }

    #[test]
    fn takes_meshes_it_doesnt_know_and_refuses_what_it_must() {
        let json = RadioKeyRecord::new("meshcore", &key(), 1, 1).to_json();
        for mesh in ["meshtastic", "a-mesh-of-the-future"] {
            let read = RadioKeyRecord::parse(&json.replace("meshcore", mesh)).unwrap();
            assert_eq!(read.mesh, mesh);
        }
        // A field from a later version of the same shape.
        assert!(RadioKeyRecord::parse(&json.replace(r#""at":"#, r#""x":2,"at":"#)).is_ok());
        for (json, error) in [
            (
                json.replace(r#""v":0"#, r#""v":1"#),
                RadioKeyError::UnknownVersion(1),
            ),
            (json.replace("meshcore", ""), RadioKeyError::InvalidMesh),
            (
                json.replace("meshcore", &"m".repeat(33)),
                RadioKeyError::InvalidMesh,
            ),
            (json.replace(HEX, "abc"), RadioKeyError::InvalidKey),
            (
                json.replace(HEX, &"g".repeat(64)),
                RadioKeyError::InvalidKey,
            ),
            ("not json".to_owned(), RadioKeyError::Malformed),
            (r#"{"v":0}"#.to_owned(), RadioKeyError::Malformed),
            ("x".repeat(2000), RadioKeyError::Malformed),
        ] {
            assert_eq!(RadioKeyRecord::parse(&json).unwrap_err(), error, "{json}");
        }
    }

    #[test]
    fn new_keys_differ_and_stay_out_of_logs() {
        assert_ne!(*new_radio_key(), *new_radio_key());
        let record = RadioKeyRecord::new("meshcore", &key(), 1, 1);
        let printed = format!("{record:?}");
        assert!(!printed.contains("000102"), "{printed}");
        assert!(printed.contains("meshcore"), "{printed}");
    }
}

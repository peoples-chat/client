//! Keys for radio messages in a direct chat. Each member's app makes a random key of its own for
//! the chat and mesh, and puts it in its radio address record ([`super::RadioRecord`]), which
//! lives in the chat's state (#128). The key encrypts what that member sends by radio, and
//! nothing else. The rules for packets are in `docs/radio-format-v0.md`, "Where the keys come
//! from".

use rand::RngCore;
use rand::rngs::OsRng;
use zeroize::Zeroizing;

/// Key length in bytes, for a 256-bit AEAD such as ChaCha20-Poly1305.
pub const RADIO_KEY_BYTES: usize = 32;

/// A key for what one member sends in one chat over one mesh. Wiped when dropped.
pub type RadioKey = Zeroizing<[u8; RADIO_KEY_BYTES]>;

/// A new random key.
pub fn new_radio_key() -> RadioKey {
    let mut key = Zeroizing::new([0; RADIO_KEY_BYTES]);
    OsRng.fill_bytes(key.as_mut());
    key
}

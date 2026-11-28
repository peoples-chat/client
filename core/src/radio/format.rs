//! The radio message format v0 (#38, #39): one chat message in one radio packet.
//!
//! `docs/radio-format-v0.md` describes the format. These are its pure functions, checked
//! against that document's test vectors, which `scripts/radio_vectors.py` generates apart from
//! this code. Nothing here sends, stores or remembers: keeping counters, epochs and the replay
//! window is the receiving side's job (#40, #42).
//!
//! Every function that reads a packet takes whatever arrived over the air and returns an error
//! for anything malformed. None of them panics; `core/fuzz` checks that.

use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use cgka_traits::app_event::{MARMOT_APP_EVENT_KIND_CHAT, canonical_event_id};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

use super::keys::RadioKey;

/// The format's version, in the first byte's high 4 bits.
pub const FORMAT_VERSION: u8 = 0;
/// Version and type, key hint, the counter's low 16 bits, and the send time.
pub const HEADER_BYTES: usize = 8;
/// The Poly1305 tag.
pub const TAG_BYTES: usize = 16;
/// What a packet adds to its text.
pub const OVERHEAD_BYTES: usize = HEADER_BYTES + TAG_BYTES;
/// The most a MeshCore text message holds ([`super::MAX_TEXT_BYTES`]); the packet travels in
/// one as base64.
const TEXT_MESSAGE_BYTES: usize = super::MAX_TEXT_BYTES;
/// The longest text that fits in one packet in one MeshCore text message: 96 bytes.
pub const MAX_RADIO_TEXT_BYTES: usize = TEXT_MESSAGE_BYTES * 3 / 4 - OVERHEAD_BYTES;

/// What a packet carries. Only text exists in v0; delivery confirmations (#41), reactions and
/// locations will take the next values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PacketKind {
    Text,
}

impl PacketKind {
    fn code(self) -> u8 {
        match self {
            Self::Text => 0,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Text),
            _ => None,
        }
    }
}

/// The 8 bytes in front of a packet's ciphertext. They are authenticated, not encrypted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub kind: PacketKind,
    /// The number of the sender's key that sealed it, modulo 256.
    pub key_hint: u8,
    /// The low 16 bits of the sender's counter. [`full_counter`] rebuilds the rest.
    pub counter_low: u16,
    /// When the message was sent, in seconds since the Unix epoch, as in its Nostr event.
    pub sent_at: u32,
}

impl Header {
    /// Reads the header of `packet`, which must also hold a tag.
    pub fn parse(packet: &[u8]) -> Result<Self, PacketError> {
        if packet.len() < OVERHEAD_BYTES {
            return Err(PacketError::TooShort(packet.len()));
        }
        let version = packet[0] >> 4;
        if version != FORMAT_VERSION {
            return Err(PacketError::UnknownVersion(version));
        }
        let kind = PacketKind::from_code(packet[0] & 0x0F)
            .ok_or(PacketError::UnknownKind(packet[0] & 0x0F))?;
        Ok(Self {
            kind,
            key_hint: packet[1],
            counter_low: u16::from_be_bytes([packet[2], packet[3]]),
            sent_at: u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]),
        })
    }

    fn to_bytes(self) -> [u8; HEADER_BYTES] {
        let mut bytes = [0; HEADER_BYTES];
        bytes[0] = FORMAT_VERSION << 4 | self.kind.code();
        bytes[1] = self.key_hint;
        bytes[2..4].copy_from_slice(&self.counter_low.to_be_bytes());
        bytes[4..8].copy_from_slice(&self.sent_at.to_be_bytes());
        bytes
    }
}

/// Why a packet was refused. Radio input is untrusted: a receiver drops such a packet silently.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PacketError {
    #[error("the text message is not a radio packet")]
    NotAPacket,
    #[error("{0} bytes are too few for a packet")]
    TooShort(usize),
    #[error("format version {0} is unknown")]
    UnknownVersion(u8),
    #[error("packet type {0} is unknown")]
    UnknownKind(u8),
    #[error("the packet is not a text")]
    NotText,
    #[error("the counter doesn't match the packet's")]
    CounterMismatch,
    #[error("the packet doesn't open with this key")]
    NotAuthentic,
    #[error("the text is not UTF-8")]
    NotUtf8,
}

/// A text as it came out of a packet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenedText {
    pub text: String,
    /// Seconds since the Unix epoch.
    pub sent_at: u32,
}

/// Seals `text` into a packet with the sender's key number `key_number`. `counter` must never
/// have been used with this key before: the sender stores the next one before the packet goes out.
pub fn seal_text(
    key: &RadioKey,
    key_number: u64,
    counter: u64,
    sent_at: u32,
    text: &str,
) -> Vec<u8> {
    let header = Header {
        kind: PacketKind::Text,
        key_hint: key_number as u8,
        counter_low: counter as u16,
        sent_at,
    }
    .to_bytes();
    let sealed = cipher(key)
        .encrypt(
            &nonce(counter),
            Payload {
                msg: text.as_bytes(),
                aad: &header,
            },
        )
        .expect("ChaCha20-Poly1305 seals any text that fits in memory");
    let mut packet = Vec::with_capacity(HEADER_BYTES + sealed.len());
    packet.extend_from_slice(&header);
    packet.extend_from_slice(&sealed);
    packet
}

/// Opens a text packet with the sender's key. `counter` is the full counter, which
/// [`full_counter`] rebuilds from the header.
pub fn open_text(key: &RadioKey, packet: &[u8], counter: u64) -> Result<OpenedText, PacketError> {
    let header = Header::parse(packet)?;
    if header.kind != PacketKind::Text {
        return Err(PacketError::NotText);
    }
    if header.counter_low != counter as u16 {
        return Err(PacketError::CounterMismatch);
    }
    let (header_bytes, sealed) = packet.split_at(HEADER_BYTES);
    let text = cipher(key)
        .decrypt(
            &nonce(counter),
            Payload {
                msg: sealed,
                aad: header_bytes,
            },
        )
        .map_err(|_| PacketError::NotAuthentic)?;
    Ok(OpenedText {
        text: String::from_utf8(text).map_err(|_| PacketError::NotUtf8)?,
        sent_at: header.sent_at,
    })
}

/// The full counter whose low 16 bits are `low`: the one closest to the counter after the
/// highest seen so far, or to 0 before any.
pub fn full_counter(highest: Option<u64>, low: u16) -> u64 {
    const SPAN: u64 = 1 << 16;
    let expected = highest.map_or(0, |highest| highest.saturating_add(1));
    let candidate = (expected & !(SPAN - 1)) | u64::from(low);
    if candidate > expected && candidate - expected > SPAN / 2 {
        candidate.checked_sub(SPAN).unwrap_or(candidate)
    } else if expected > candidate && expected - candidate > SPAN / 2 {
        candidate.checked_add(SPAN).unwrap_or(candidate)
    } else {
        candidate
    }
}

/// How a packet travels in a MeshCore text message: base64, standard alphabet, no padding.
pub fn to_text_message(packet: &[u8]) -> String {
    STANDARD_NO_PAD.encode(packet)
}

/// The packet in a MeshCore text message. Anything but unpadded standard base64 is refused,
/// which includes ordinary text from people on stock MeshCore apps, mostly.
pub fn from_text_message(text: &str) -> Result<Vec<u8>, PacketError> {
    STANDARD_NO_PAD
        .decode(text)
        .map_err(|_| PacketError::NotAPacket)
}

/// Whether `text` fits in one packet in one MeshCore text message.
pub fn fits(text: &str) -> bool {
    text.len() <= MAX_RADIO_TEXT_BYTES
}

/// The id MDK gives the message: the id of the kind-9 event that `sender` (an account's public
/// key, in hex) signs for `text` at `sent_at`. It holds for texts without mentions, which MDK
/// would tag; the sender checks that before it sends.
pub fn message_id(sender: &str, sent_at: u32, text: &str) -> String {
    canonical_event_id(
        sender,
        u64::from(sent_at),
        MARMOT_APP_EVENT_KIND_CHAT,
        &[],
        text,
    )
}

fn cipher(key: &RadioKey) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new(Key::from_slice(key.as_slice()))
}

/// Four zero bytes, then the full counter.
fn nonce(counter: u64) -> Nonce {
    let mut nonce = [0; 12];
    nonce[4..].copy_from_slice(&counter.to_be_bytes());
    Nonce::from(nonce)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeroize::Zeroizing;

    const ADA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const BEA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// The vectors' keys: Ada's and Bea's.
    fn vector_key(hex_key: &str) -> RadioKey {
        let mut key = Zeroizing::new([0; 32]);
        hex::decode_to_slice(hex_key, key.as_mut()).unwrap();
        key
    }

    fn ada_key() -> RadioKey {
        vector_key("805f65bf8627f9d3a3849c2e330d9c6f9f5936495b5212c5597526bcda48ead7")
    }

    fn bea_key() -> RadioKey {
        vector_key("05e43f1c7156b5f6b36d01742b86cf32a9753e57767d2e1bbc6ee99158d62d5d")
    }

    struct Vector {
        from_ada: bool,
        key_number: u64,
        counter: u64,
        sent_at: u32,
        text: &'static str,
        packet: &'static str,
        text_message: &'static str,
        id: &'static str,
    }

    /// From `docs/radio-format-v0.md`, "Test vectors".
    const VECTORS: [Vector; 3] = [
        Vector {
            from_ada: true,
            key_number: 7,
            counter: 0,
            sent_at: 1795300000,
            text: "Hello Bea",
            packet: "000700006b021aa0b14ef70f73b5a3c2c4d71c6b846d9efa1210e79830b8d95141",
            text_message: "AAcAAGsCGqCxTvcPc7WjwsTXHGuEbZ76EhDnmDC42VFB",
            id: "28161bd53ad40bf5d4a26ce489ac8b255465a3f30086800ecc5065810a5571b7",
        },
        Vector {
            from_ada: false,
            key_number: 7,
            counter: 300,
            sent_at: 1795300060,
            text: "Hej! Ses vi i morgen? ☔",
            packet: "0007012c6b021adc1b142e9f31cf994773465c9e70b4ba8dff2182baec5a16e5174dbb0e77c2285e85735ea23fec51f202",
            text_message: "AAcBLGsCGtwbFC6fMc+ZR3NGXJ5wtLqN/yGCuuxaFuUXTbsOd8IoXoVzXqI/7FHyAg",
            id: "9a67ab054fd3b8c27baf5875d0b6a9c864ee1ad534ca392b0dc5fc954b585cb1",
        },
        Vector {
            from_ada: true,
            key_number: 263,
            counter: 70000,
            sent_at: 1795300120,
            text: "Still here.",
            packet: "000711706b021b1827e411bf2afcd0b96c7b419f566072e3b1a3c0672b382ed2617fb2",
            text_message: "AAcRcGsCGxgn5BG/KvzQuWx7QZ9WYHLjsaPAZys4LtJhf7I",
            id: "c8f4ca5abdd524ab60265ebbae320ca082726a820b2f56cf6fe68cd045844e0e",
        },
    ];

    #[test]
    fn matches_the_test_vectors() {
        for vector in VECTORS {
            let (key, sender) = if vector.from_ada {
                (&ada_key(), ADA)
            } else {
                (&bea_key(), BEA)
            };
            let packet = seal_text(
                key,
                vector.key_number,
                vector.counter,
                vector.sent_at,
                vector.text,
            );
            assert_eq!(hex::encode(&packet), vector.packet, "{}", vector.text);
            assert_eq!(to_text_message(&packet), vector.text_message);
            assert_eq!(from_text_message(vector.text_message).unwrap(), packet);
            assert_eq!(
                open_text(key, &packet, vector.counter).unwrap(),
                OpenedText {
                    text: vector.text.to_owned(),
                    sent_at: vector.sent_at,
                }
            );
            assert_eq!(message_id(sender, vector.sent_at, vector.text), vector.id);
        }
    }

    #[test]
    fn the_longest_text_fills_one_text_message() {
        assert_eq!(MAX_RADIO_TEXT_BYTES, 96);
        let key = ada_key();
        let longest = "x".repeat(MAX_RADIO_TEXT_BYTES);
        assert!(fits(&longest));
        let packet = seal_text(&key, 7, 1, 1795300180, &longest);
        assert_eq!(to_text_message(&packet).len(), 160);
        let longer = longest + "x";
        assert!(!fits(&longer));
        assert!(to_text_message(&seal_text(&key, 7, 1, 1795300180, &longer)).len() > 160);
        // Bytes count, not characters.
        assert!(!fits(&"☔".repeat(33)));
    }

    #[test]
    fn refuses_what_it_must() {
        let ada = ada_key();
        let packet = seal_text(&ada, 7, 0, 1795300000, "Hello Bea");

        // Opened with another key, as when a packet is reflected back to its sender.
        assert_eq!(
            open_text(&bea_key(), &packet, 0),
            Err(PacketError::NotAuthentic)
        );
        // Any changed byte, in the header too.
        for index in 0..packet.len() {
            let mut changed = packet.clone();
            changed[index] ^= 1;
            assert!(open_text(&ada, &changed, 0).is_err(), "byte {index}");
        }
        // The right low bits with the wrong full counter.
        assert_eq!(
            open_text(&ada, &packet, 1 << 16),
            Err(PacketError::NotAuthentic)
        );
        assert_eq!(
            open_text(&ada, &packet, 1),
            Err(PacketError::CounterMismatch)
        );
        assert_eq!(
            open_text(&ada, &packet[..OVERHEAD_BYTES - 1], 0),
            Err(PacketError::TooShort(OVERHEAD_BYTES - 1))
        );

        let mut other_version = packet.clone();
        other_version[0] = 0x10;
        assert_eq!(
            Header::parse(&other_version),
            Err(PacketError::UnknownVersion(1))
        );
        let mut other_kind = packet;
        other_kind[0] = 0x01;
        assert_eq!(Header::parse(&other_kind), Err(PacketError::UnknownKind(1)));
    }

    #[test]
    fn refuses_text_messages_that_are_not_packets() {
        for (text, refused) in [
            ("Hello from a stock MeshCore app", PacketError::NotAPacket),
            // Padded, and with bits left over: not how packets travel.
            (
                "AAcAAGsCGqCxTvcPc7WjwsTXHGuEbZ76EhDnmDC42VFB=",
                PacketError::NotAPacket,
            ),
            ("AB", PacketError::NotAPacket),
            ("", PacketError::TooShort(0)),
            ("AAAA", PacketError::TooShort(3)),
        ] {
            let error = match from_text_message(text) {
                Err(error) => error,
                Ok(bytes) => Header::parse(&bytes).unwrap_err(),
            };
            assert_eq!(error, refused, "{text:?}");
        }
    }

    #[test]
    fn rebuilds_the_counter_from_its_low_bits() {
        for (highest, low, full) in [
            (None, 0, 0),
            (None, 5, 5),
            (Some(0), 1, 1),
            (Some(65530), 3, 65539),
            (Some(70000), 4464, 70000),
            (Some(70000), 4000, 69536),
            (Some(0), 65535, 65535),
            // At the very end it can't go up, so it stays below.
            (Some(u64::MAX), 7, 0xFFFF_FFFF_FFFF_0007),
        ] {
            assert_eq!(full_counter(highest, low), full, "{highest:?} {low}");
        }
    }

    /// The fuzz target in `core/fuzz` does this at length; this keeps a short version in CI.
    #[test]
    fn malformed_input_never_panics() {
        let key = ada_key();
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let valid = seal_text(&key, 7, 0, 1795300000, "Hello Bea");
        for round in 0..20_000 {
            let length = (next() % 200) as usize;
            let mut input: Vec<u8> = (0..length).map(|_| next() as u8).collect();
            if round % 2 == 0 && input.len() >= HEADER_BYTES {
                // Keep a valid header now and then, so that decryption runs.
                input[..HEADER_BYTES].copy_from_slice(&valid[..HEADER_BYTES]);
            }
            let _ = from_text_message(&String::from_utf8_lossy(&input));
            if let Ok(header) = Header::parse(&input) {
                let counter = full_counter(Some(next()), header.counter_low);
                assert!(open_text(&key, &input, counter).is_err());
            }
        }
    }
}

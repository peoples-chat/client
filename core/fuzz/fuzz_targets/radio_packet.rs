//! Whatever arrives over the air must be refused or opened, never panic (#39). This feeds
//! arbitrary input to the radio format's decoder, as raw packets and as MeshCore text messages.
//!
//! The keys are those of the format's test vectors, so the seeds in `seeds/radio_packet` open,
//! and their mutations reach every check after the header.

#![no_main]

use libfuzzer_sys::fuzz_target;
use peoples_chat_core::radio::RadioKey;
use peoples_chat_core::radio::format::{self, Header};

/// The keys of the format's test vectors.
const VECTOR_KEYS: [&str; 2] = [
    "805f65bf8627f9d3a3849c2e330d9c6f9f5936495b5212c5597526bcda48ead7",
    "05e43f1c7156b5f6b36d01742b86cf32a9753e57767d2e1bbc6ee99158d62d5d",
];

fuzz_target!(|data: &[u8]| {
    let keys = VECTOR_KEYS.map(|hex_key| {
        let mut key = RadioKey::new([0; 32]);
        hex::decode_to_slice(hex_key, key.as_mut()).expect("the vectors' keys");
        key
    });
    if let Ok(text) = std::str::from_utf8(data)
        && let Ok(packet) = format::from_text_message(text)
    {
        open(&keys, &packet);
    }
    open(&keys, data);
});

fn open(keys: &[RadioKey; 2], packet: &[u8]) {
    let Ok(header) = Header::parse(packet) else {
        return;
    };
    // Any highest counter so far, including none and the largest.
    let highest = match packet.get(HIGHEST_AT) {
        None => None,
        Some(0) => Some(u64::MAX),
        Some(byte) => Some(u64::from(*byte) << 15),
    };
    let counter = format::full_counter(highest, header.counter_low);
    for key in keys {
        // A packet that opens seals back to the same bytes.
        if let Ok(opened) = format::open_text(key, packet, counter) {
            let again = format::seal_text(
                key,
                u64::from(header.key_hint),
                counter,
                opened.sent_at,
                &opened.text,
            );
            assert_eq!(again, packet);
        }
    }
}

/// A byte past the header that picks the highest counter seen.
const HIGHEST_AT: usize = format::HEADER_BYTES;

//! Whatever arrives over the air must be refused or opened, never panic (#39). This feeds
//! arbitrary input to the radio format's decoder, as raw packets and as MeshCore text messages.
//!
//! The keys are those of the format's test vectors, so the seeds in `seeds/radio_packet` open,
//! and their mutations reach every check after the header.

#![no_main]

use libfuzzer_sys::fuzz_target;
use peoples_chat_core::radio::RadioKeys;
use peoples_chat_core::radio::format::{self, Header};

fuzz_target!(|data: &[u8]| {
    let keys = RadioKeys::derive(
        &(0..32).collect::<Vec<u8>>(),
        7,
        &"aa".repeat(32),
        &"bb".repeat(32),
    )
    .expect("the vectors' keys");
    if let Ok(text) = std::str::from_utf8(data)
        && let Ok(packet) = format::from_text_message(text)
    {
        open(&keys, &packet);
    }
    open(&keys, data);
});

fn open(keys: &RadioKeys, packet: &[u8]) {
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
    for key in [&keys.send, &keys.receive] {
        // A packet that opens seals back to the same bytes.
        if let Ok(opened) = format::open_text(key, packet, counter) {
            let again = format::seal_text(
                key,
                u64::from(header.epoch_hint),
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

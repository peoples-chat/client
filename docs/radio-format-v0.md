# Radio message format v0

The app's compact format for a chat message in one radio packet (#38). It is a proof-of-concept design: the spec
requires an independent security review before public release.

**In short:**

- **The packet:** an 8-byte header, the text encrypted with ChaCha20-Poly1305 under a key from the chat's MLS state,
  and a 16-byte tag. That is 24 bytes of overhead.
- **The budget:** in a MeshCore text message the packet travels as base64, which leaves 96 bytes for the text.
- **The message id:** the receiver rebuilds the message's id from the packet. The copy the sender later publishes to
  the relays has the same id, so receivers drop the duplicate.

## Assumptions

- **The path is a MeshCore text message**, until #36 measures the options. Text-encoded data in a plain message is the
  one option that works with meshcore-rs 0.2.0 today, and the firmware encrypts and confirms it
  ([radio.md](radio.md#meshcore-facts-that-shape-the-poc)). Another path changes only the carrier and the budget.
- **Direct chats only:** a chat with exactly two members.
- **Keys come from the chat's MLS state:** one per direction, per epoch
  ([spikes/37-radio-keys.md](spikes/37-radio-keys.md)).

## The packet

Numbers are big-endian.

| Offset | Bytes | Field | Value |
| --- | --- | --- | --- |
| 0 | 1 | Version and type | Version in the high 4 bits (0); type in the low 4 bits (0: text) |
| 1 | 1 | Epoch hint | The chat's MLS epoch, modulo 256 |
| 2 | 2 | Counter | The low 16 bits of the sender's counter for this chat and epoch |
| 4 | 4 | Sent at | Seconds since the Unix epoch, as in the message's event |
| 8 | n | Ciphertext | The text, UTF-8, encrypted |
| 8 + n | 16 | Tag | Poly1305 |

**Types.** Only text (0) is defined. Delivery confirmations (#41), reactions and locations come later, as types 1 to 3.
A receiver drops a version or type it doesn't know.

## Encryption

- **Key:** key(e, sender), the sender's key in epoch e ([spike #37](spikes/37-radio-keys.md#the-keys)).
- **AEAD:** ChaCha20-Poly1305, RFC 8439.
- **Nonce:** four zero bytes, then the full counter as a 64-bit number.
- **Associated data:** the 8-byte header, so changing any header field makes the packet fail.

**The tag is 16 bytes.** That is the standard tag, which every library checks. Truncating it to 8 bytes would give 8
more bytes of text, with a 2^-64 chance per try of a forgery getting through; on radio every try costs airtime. It can
be reconsidered with #36's measurements.

**What opening proves.** A packet that opens with key(e, contact) comes from someone who holds the chat's secret in
epoch e: a member. In a two-person chat that is the contact, since a packet reflected back to its sender doesn't open
with the other direction's key. This is the member-level authentication the spec promises, and no more.

## The message id

The receiver rebuilds the id that MDK gives the message, from the sender's account key, the time and the text:

```text
id = sha256 of the NIP-01 serialization of [0, sender's account key, sent at, 9, [], text]
```

This is `cgka_traits::app_event::canonical_event_id`, the function MDK uses itself.

- **It works because** MDK signs a text as a kind-9 event whose only tags are for mentions (`nostr:npub…` in the
  text).
- **So a text with mentions doesn't go by radio in v0.** Before sending, the sender rebuilds the id and compares it
  with MDK's id for the message; if they differ, the message waits for the internet.
- **The relay copy shares the id.** When the internet returns, MDK publishes the same message, and receivers keep one
  copy (#47).

## Which chat

The packet doesn't name the chat, which saves bytes:

- The receiver knows the radio that sent the packet. The radio address record (#34) leads from the radio to the
  contact, and from the contact to the direct chat.
- Without a record, the receiver tries each direct chat's keys. Only the right one opens the packet.

## Epochs

These come from [spike #37](spikes/37-radio-keys.md#epochs):

- **The sender** uses the newest epoch it holds keys for.
- **The receiver** keeps the keys of the last four epochs, and opens the packet with the key whose epoch matches the
  hint.
- **A receiver that is behind**, with no key for the sender's epoch, drops the packet. The relay copy follows.

## The counter and the replay window

- **Each counter** belongs to one chat, epoch and direction, and starts at 0.
- **The sender stores** the next value before a packet goes out, so a value is never used twice with one key.
- **The receiver rebuilds the full counter** from the low 16 bits: the value closest to the one after the highest it
  has seen, as DTLS 1.3 does with record numbers.
- **The replay window.** The receiver accepts a counter higher than any it has seen, or one of the 64 below the highest
  that it hasn't seen yet. It drops everything else.
- **Drops are silent.** A packet that doesn't open, names an epoch with no kept key, or falls outside the window never
  reaches the interface and never causes a transmission (#42). Messages are also dropped as duplicates by id.

## The text budget

**Overhead:** 24 bytes, the 8-byte header and the 16-byte tag.

**The carrier.** In a MeshCore text message, the packet travels as base64 (RFC 4648, standard alphabet, no padding).
The firmware takes at most 160 bytes of text without NUL, so the packet can be at most 120 bytes. That leaves **96
bytes of text**.

That is just under the spec's target of 100 to 150 bytes. #36's measurements decide which lever to pull:

| Change | Text bytes |
| --- | --- |
| v0: text message, base64, 16-byte tag | 96 |
| A denser text encoding (5 characters per 4 bytes) | 104 |
| An 8-byte tag | 104 |
| Both | 112 |
| Raw data (`SEND_RAW_DATA`, about 170 bytes), 16-byte tag | About 146 |

**One packet per message.** v0 sends no message in more than one packet. A longer text waits for the internet. The
composer shows the limit in radio mode (#48), and the core enforces it (#51).

## Test vectors

`scripts/radio_vectors.py` generates these with nothing but Python's standard library, apart from the core's code.
Before it prints, it checks its ChaCha20-Poly1305 against RFC 8439, its HKDF against RFC 5869, and its event id against
MDK's own NIP-01 test. The ids below also match `canonical_event_id`. Run it with `python3 -I
scripts/radio_vectors.py`.

All vectors use the chat secret `00 01 … 1f` in epoch 7. Ada's account key is `aa…aa`, and Bea's is `bb…bb`, 32
bytes each.

**Keys:**

- Ada sends with `805f65bf8627f9d3a3849c2e330d9c6f9f5936495b5212c5597526bcda48ead7`
- Bea sends with `05e43f1c7156b5f6b36d01742b86cf32a9753e57767d2e1bbc6ee99158d62d5d`

**Ada to Bea:** "Hello Bea" (9 bytes), epoch 7, counter 0, sent at 1795300000.

- Nonce: `000000000000000000000000`
- Packet (33 bytes): `000700006b021aa0b14ef70f73b5a3c2c4d71c6b846d9efa1210e79830b8d95141`
- Text message (44 characters): `AAcAAGsCGqCxTvcPc7WjwsTXHGuEbZ76EhDnmDC42VFB`
- Message id: `28161bd53ad40bf5d4a26ce489ac8b255465a3f30086800ecc5065810a5571b7`

**Bea to Ada:** "Hej! Ses vi i morgen? ☔" (25 bytes), epoch 7, counter 300, sent at 1795300060.

- Nonce: `00000000000000000000012c`
- Packet (49 bytes): `0007012c6b021adc1b142e9f31cf994773465c9e70b4ba8dff2182baec5a16e5174dbb0e77c2285e85735ea23fec51f202`
- Text message (66 characters): `AAcBLGsCGtwbFC6fMc+ZR3NGXJ5wtLqN/yGCuuxaFuUXTbsOd8IoXoVzXqI/7FHyAg`
- Message id: `9a67ab054fd3b8c27baf5875d0b6a9c864ee1ad534ca392b0dc5fc954b585cb1`

**Ada to Bea, epoch 263, after 70,000 messages:** "Still here." (11 bytes), counter 70000, sent at 1795300120,
with the same keys as epoch 7, to test the hint and the counter's low bits.

- Nonce: `000000000000000000011170`
- Packet (35 bytes): `000711706b021b1827e411bf2afcd0b96c7b419f566072e3b1a3c0672b382ed2617fb2`
- Text message (47 characters): `AAcRcGsCGxgn5BG/KvzQuWx7QZ9WYHLjsaPAZys4LtJhf7I`
- Message id: `c8f4ca5abdd524ab60265ebbae320ca082726a820b2f56cf6fe68cd045844e0e`

**The budget.** A 96-byte text makes a 120-byte packet, and 160 characters. One more byte doesn't fit.

**Refused:**

- the first packet, opened with Bea's sending key, as if reflected back to Ada;
- the first packet with the tag's last bit flipped.

**Rebuilding the counter:**

| Highest seen | Low 16 bits | Full counter |
| --- | --- | --- |
| 0 | 1 | 1 |
| 65530 | 3 | 65539 |
| 70000 | 4464 | 70000, the highest itself: a replay |
| 70000 | 4000 | 69536, below the window: dropped |

## In the core

`core/src/radio/format.rs` implements the format as pure functions (#39):

- `seal_text`, `open_text`, `Header::parse`, `full_counter`;
- `to_text_message` and `from_text_message`, the base64 carrier;
- `message_id`, the id rebuild.

Its unit tests check it against the vectors above, against changed and reflected packets, and against 20,000 random
inputs in CI. Keeping counters, kept epochs and the replay window is up to the receiving side (#40, #42).

**Fuzzing.** `core/fuzz` holds a cargo-fuzz target. It feeds arbitrary input to the decoder, as raw packets and as text
messages, and checks two things: nothing panics, and a packet that opens seals back to the same bytes. It needs cargo-fuzz and a
nightly toolchain:

```sh
cargo install cargo-fuzz
cd core && cargo +nightly fuzz run radio_packet --sanitizer none fuzz/corpus/radio_packet fuzz/seeds/radio_packet
```

The seeds are the vectors above, so mutations start from packets that open. `--sanitizer none`: the core is safe Rust,
so the fuzzer looks for panics, and a sanitizer would only slow it down and reach into MDK's C and Odin code.

## Not in v0

- groups: only chats with two members;
- messages in more than one packet;
- texts with mentions;
- delivery confirmations, reactions and locations, which have their types reserved;
- an independent security review.

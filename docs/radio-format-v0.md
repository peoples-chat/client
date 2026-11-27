# Radio message format v0

The app's compact format for a chat message in one radio packet (#38). It is a proof-of-concept design: the spec
requires an independent security review before public release.

**In short:**

- **The packet:** an 8-byte header, the text encrypted with ChaCha20-Poly1305 under the sender's radio key for the
  chat, and a 16-byte tag. That is 24 bytes of overhead.
- **The keys:** each member's app makes a random one and puts it in its radio address record, which lives in the
  chat's state ([Where the keys come from](#where-the-keys-come-from)).
- **The budget:** in a MeshCore text message the packet travels as base64, which leaves 96 bytes for the text.
- **The message id:** the receiver rebuilds the message's id from the packet. The copy the sender later publishes to
  the relays has the same id, so receivers drop the duplicate.

## Assumptions

- **The path is a MeshCore text message**, until #36 measures the options. Text-encoded data in a plain message is the
  one option that works with meshcore-rs 0.2.0 today, and the firmware encrypts and confirms it
  ([radio.md](radio.md#meshcore-facts-that-shape-the-poc)). Another path changes only the carrier and the budget.
- **Direct chats only:** a chat with exactly two members.
- **Keys are in the chat's state:** one per member, chat and mesh.

## Where the keys come from

**Decisions (2026-11-27, [#128](https://github.com/peoples-chat/client/issues/128)).**

1. A radio key is app data of our own, inside the chat. Nothing is derived from the chat's MLS exporter secrets. In
   particular `MLS-Exporter("marmot", "group-event", 32)`, which radio used before, is reserved by Marmot for the outer
   encryption of kind-445 events, and "MUST NOT be reused for app payloads"
   ([transports/nostr.md](https://github.com/marmot-protocol/marmot/blob/677a4b5/transports/nostr.md)).
2. **One record, not two.** The key is part of the radio address record (#34), which is now version 1: one record per
   radio protocol, with the radio, its settings and the app's radio key.
3. **Both MeshCore and Meshtastic.** The record names its mesh, and its fields cover both.
4. **The record lives in the chat's state, not in a message** (after spike
   [#117](https://github.com/peoples-chat/client/blob/117-chat-state-spike/docs/spikes/117-chat-state.md)). It is
   written as MLS app components in the direct chat's GroupContext, so no client shows anything, and the current
   records come with the chat to new devices. Marmot recommends app components for app-owned data
   ([foundation/mls-protocol.md](https://github.com/marmot-protocol/marmot/blob/677a4b5/foundation/mls-protocol.md),
   App components).

**The components.** The account whose public key (hex) sorts first writes component **`0xf910`**, the other
**`0xf911`**, each only its own. One shared component would lose a record when both write at once. The ids are
provisional until registered ([#145](https://github.com/peoples-chat/client/issues/145)). A component holds a JSON array
of records, at most one per mesh. Each write is an MLS commit, so it needs a relay and starts a new epoch. Only an
admin of the chat can write; MDK makes both members of the direct chats it starts admins.

**The record, version 1,** is canonical JSON: members sorted by name, no spaces. A MeshCore record is about 250 bytes,
a Meshtastic one about 320.

```json
{"addr":"…64 hex…","at":1795300000000,"bw":62500,"cr":8,"freq":869618,"mesh":"meshcore","name":"Ada's radio","rk":"…64 hex…","rkn":1,"sf":8,"v":1}
```

| Member | Meaning |
| --- | --- |
| `v` | `1` |
| `mesh` | `meshcore` or `meshtastic` |
| `name` | The radio's advertised name, at most 40 bytes |
| `addr` | The radio's address on the mesh. MeshCore: its public key, 64 hex digits. Meshtastic: `{"key":"…64 hex…","num":123}`, the node's public key and number |
| `freq`, `bw`, `sf`, `cr` | LoRa settings: frequency in kHz, bandwidth in Hz, spreading factor, the 5 of 4/5 |
| `preset`, `region` | Optional, for a mesh that names them, such as `LONG_FAST` and `EU_868` |
| `rk` | The app's radio key for this chat and mesh: 32 random bytes, in hex |
| `rkn` | The key's number among the sender's keys for the chat and mesh, from 1 |
| `at` | When the record was made, in milliseconds since the Unix epoch, by the sender's clock |

The Meshtastic fields are described from Meshtastic's published protobufs (`NodeInfo.num`, `User.public_key`,
`Config.LoRaConfig`'s `modem_preset` and `region`); they have not been tried against a Meshtastic radio.

**Reading.** A record of a mesh the app doesn't know, or of another version, or with a field missing or out of range,
is kept and not used. Members added later are ignored. Nothing in a component makes the app reject the chat's change.

**Version 0** is the address and settings alone, which older builds send as an app event of kind 9911. The app still
reads it, and takes no key from it. It no longer sends it.

**Who writes it, and when.**

- Only in a direct chat: the account, its contact and nobody else. Never in a group, and never outside the chat.
- Each member's app makes its key for a chat and mesh when it first has a radio of that mesh, writes the record, and
  keeps the key across restarts, in the radio store.
- **A new key, and the record rewritten:** when the chat's membership changes (its member accounts, or an account's
  number of devices in it), on every mesh; and when the radio, its name or its settings change, on that mesh. A key
  update alone, which keeps the members, keeps the radio key.
- The app looks when a radio is attached, when the chat moves to a new epoch or changes, before each radio send, and
  every minute while it has a radio. It writes only while the chat is settled; MDK sends a waiting change by itself.
- A key seals nothing until the chat's state has had it.

**Which key opens a packet.** The receiver picks by sender and mesh: of the keys that the contact's records have held
for the mesh of the radio in use, those whose number matches the packet's key hint. The key in the record now is the
current one. Only MeshCore sends radio packets today. A Meshtastic key is stored and selected the same way.

**Old keys.** A key that the record no longer holds still opens packets for 10 minutes after the app saw it replaced,
enough for a packet and its retry on the way, and only packets sent no later than the newer record was made (the
packet's send time against that record's `at`, both by the sender's clock). After that it opens nothing. A chat keeps
at most four keys per mesh.

**Behind on the chat's state.** The records travel over the relays only. A receiver that hasn't fetched the sender's
current record drops the packet, and the relay copy of the message follows.

## The packet

Numbers are big-endian.

| Offset | Bytes | Field | Value |
| --- | --- | --- | --- |
| 0 | 1 | Version and type | Version in the high 4 bits (0); type in the low 4 bits (0: text) |
| 1 | 1 | Key hint | The number (`n`) of the sender's key that sealed the packet, modulo 256 |
| 2 | 2 | Counter | The low 16 bits of the sender's counter for this key |
| 4 | 4 | Sent at | Seconds since the Unix epoch, as in the message's event |
| 8 | n | Ciphertext | The text, UTF-8, encrypted |
| 8 + n | 16 | Tag | Poly1305 |

**Types.** Only text (0) is defined. Delivery confirmations (#41), reactions and locations come later, as types 1 to 3.
A receiver drops a version or type it doesn't know.

## Encryption

- **Key:** the sender's current radio key for the chat and mesh ([above](#where-the-keys-come-from)).
- **AEAD:** ChaCha20-Poly1305, RFC 8439.
- **Nonce:** four zero bytes, then the full counter as a 64-bit number.
- **Associated data:** the 8-byte header, so changing any header field makes the packet fail.

**The tag is 16 bytes.** That is the standard tag, which every library checks. Truncating it to 8 bytes would give 8
more bytes of text, with a 2^-64 chance per try of a forgery getting through; on radio every try costs airtime. It can
be reconsidered with #36's measurements.

**What opening proves.** A packet that opens with a key from the contact's record comes from someone who holds that key:
someone who was a member of the chat when it was sent. A packet reflected back to its sender doesn't open, since the
sender never takes its own key as a contact's. This is the member-level authentication the spec promises, and no more.

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

## The counter and the replay window

- **Each counter** belongs to one key, and starts at 0. The sender keeps it with the key, so that losing one loses
  both.
- **The sender stores** the next value before a packet goes out, so a value is never used twice with one key.
- **The receiver rebuilds the full counter** from the low 16 bits: the value closest to the one after the highest it
  has seen, as DTLS 1.3 does with record numbers.
- **The replay window.** The receiver accepts a counter higher than any it has seen, or one of the 64 below the highest
  that it hasn't seen yet. It drops everything else.
- **Drops are silent.** A packet that doesn't open, names a key number with no kept key, was sent with a replaced key
  after the newer one was made, or falls outside the window never reaches the interface and never causes a transmission
  (#42). Messages are also dropped as duplicates by id.

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

Ada's account key is `aa…aa`, and Bea's is `bb…bb`, 32 bytes each. The vectors' radio keys are fixed values; a real
key is random. (The script makes them with HKDF, as radio keys were made before #128, which is why it checks HKDF.)

**Keys,** both number 7 unless a vector says otherwise:

- Ada's: `805f65bf8627f9d3a3849c2e330d9c6f9f5936495b5212c5597526bcda48ead7`
- Bea's: `05e43f1c7156b5f6b36d01742b86cf32a9753e57767d2e1bbc6ee99158d62d5d`

**Ada to Bea:** "Hello Bea" (9 bytes), key number 7, counter 0, sent at 1795300000.

- Nonce: `000000000000000000000000`
- Packet (33 bytes): `000700006b021aa0b14ef70f73b5a3c2c4d71c6b846d9efa1210e79830b8d95141`
- Text message (44 characters): `AAcAAGsCGqCxTvcPc7WjwsTXHGuEbZ76EhDnmDC42VFB`
- Message id: `28161bd53ad40bf5d4a26ce489ac8b255465a3f30086800ecc5065810a5571b7`

**Bea to Ada:** "Hej! Ses vi i morgen? ☔" (25 bytes), key number 7, counter 300, sent at 1795300060.

- Nonce: `00000000000000000000012c`
- Packet (49 bytes): `0007012c6b021adc1b142e9f31cf994773465c9e70b4ba8dff2182baec5a16e5174dbb0e77c2285e85735ea23fec51f202`
- Text message (66 characters): `AAcBLGsCGtwbFC6fMc+ZR3NGXJ5wtLqN/yGCuuxaFuUXTbsOd8IoXoVzXqI/7FHyAg`
- Message id: `9a67ab054fd3b8c27baf5875d0b6a9c864ee1ad534ca392b0dc5fc954b585cb1`

**Ada to Bea, key number 263, after 70,000 messages:** "Still here." (11 bytes), counter 70000, sent at 1795300120,
with the same key bytes, to test the hint and the counter's low bits.

- Nonce: `000000000000000000011170`
- Packet (35 bytes): `000711706b021b1827e411bf2afcd0b96c7b419f566072e3b1a3c0672b382ed2617fb2`
- Text message (47 characters): `AAcRcGsCGxgn5BG/KvzQuWx7QZ9WYHLjsaPAZys4LtJhf7I`
- Message id: `c8f4ca5abdd524ab60265ebbae320ca082726a820b2f56cf6fe68cd045844e0e`

**The budget.** A 96-byte text makes a 120-byte packet, and 160 characters. One more byte doesn't fit.

**Refused:**

- the first packet, opened with Bea's key, as if reflected back to Ada;
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
inputs in CI.

**Receiving.** `RadioInbox` in `core/src/radio/inbox.rs` applies the rules above to every incoming packet (#42):

- It takes each direct chat's keys from the contact's records, for the mesh of the radio in use.
- It finds the chat by the sending radio, or by trying each chat's keys.
- It opens the packet and checks its counter against a 64-wide replay window per key.
- It refuses a message whose id the chat has already.

Only an authentic packet counts against the window, so a forgery can't use up a counter. The windows live in memory; after a
restart, a replayed packet is caught by its message id, because received radio messages are stored with it (#40). A
refusal returns nothing to show and nothing to send.

**On the radio** (#40, `core/src/app/radio.rs`). The core uses the radio the user paired (#24); the tests hand it mock
radios. When `send_message` chooses radio (#45), or the switch that forces radio is on, it sends like this:

1. MDK takes the message and keeps it until a relay can be reached. MDK's id is the id above.
2. The core rebuilds that id from the account key, the time and the text. If it differs, as for a text with mentions,
   the message waits for the internet.
3. It looks at the chat's membership, and makes and writes a new record if it changed. While the chat waits for a
   relay to confirm a change, its members can't be read and the key stays. A key the chat's state hasn't had seals
   nothing.
4. It stores the next counter for the key, then seals the packet and hands it to the radio, addressed to the contact's
   MeshCore radio from their record (#34).

Every packet the radio receives goes through the inbox. Before giving up on one, the core reads the contacts' records
from the chats' state once more and tries again.

`core/src/radio_store.rs` keeps, in `radio.json`:

- **this phone's record** for each chat and mesh, with its key's next counter, written and synced before the packet
  goes out. A damaged file loses keys and counters together: the app then makes new keys.
- **the contacts' records and keys.** The file holds keys in the clear, as it holds message texts. On Unix systems,
  macOS, iOS and Android included, only its owner can read it.
- **every message that went or came by radio,** with its id. A received one is shown from there until its relay copy
  arrives, and is marked as radio after that too.
- **the contacts' radios,** and the switch.

Two cores with mock radios exchange text this way with the relay blocked (`core/tests/radio_chat.rs`).

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

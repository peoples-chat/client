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
  ([spikes/37-radio-keys.md](spikes/37-radio-keys.md)). v0 takes them from a secret that Marmot reserves for
  another use: see [Where the keys come from](#where-the-keys-come-from).

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

## Where the keys come from

**Decision (#128): v0 keeps the group-event secret, as a known break of a Marmot rule. The format can't be proposed
to Marmot, or released, with it.**

```text
secret(e)   = MLS-Exporter("marmot", "group-event", 32) in epoch e
key(e, S)   = HKDF-SHA256(salt: none, IKM: secret(e), info: "peoples.chat/radio/v0" || S's public key), 32 bytes
```

**The rule.** Marmot registers that secret for one use, the outer encryption of the chat's kind-445 events on the
relays: "It MUST NOT be reused for app payloads, media, stream records, or other feature keys"
([transports/nostr.md](https://github.com/marmot-protocol/marmot/blob/36757fed3c5053aa3c194cc6c26f6985b344af51/transports/nostr.md#L119-L120)).
A feature that needs an epoch secret must use an exporter label and context of its own
([foundation/mls-protocol.md](https://github.com/marmot-protocol/marmot/blob/36757fed3c5053aa3c194cc6c26f6985b344af51/foundation/mls-protocol.md#L93-L103)).
The radio keys are a feature key, so v0 breaks the rule. The HKDF label of our own doesn't mend it: it keeps the radio
keys apart from MDK's key, but both still hang on the one secret.

**What the break costs.** Marmot's threat model has an attacker who gets a chat's outer key. That attacker can remove
the outer layer of the chat's relay events, and still can't read its messages, because MLS encrypts them again
underneath
([SC-033](https://github.com/marmot-protocol/marmot/blob/36757fed3c5053aa3c194cc6c26f6985b344af51/security-considerations.md#L989-L1000)).
A radio packet has no second layer. With the group-event secret of epoch e, that attacker can derive both radio keys
of epoch e, and so can:

- read every radio packet of the chat in that epoch;
- forge packets in either direction, since the key is all that a packet proves
  ([What opening proves](#encryption)).

Other epochs, and the messages that went over the internet, stay as safe as Marmot keeps them.

**Why v0 keeps it anyway:**

- **Nothing else works with MDK v0.18.0** without a change to MDK or to the chat (the table below).
- **The proof of concept doesn't depend on the difference.** It is one direct chat between two phones of our own.
  White Noise never sees a radio packet, and internet chats are unchanged.
- **The swap is small.** Only `secret(e)` changes. The packet, the HKDF step, the counters and the epoch rules stay,
  and `Messaging::chat_secret` is the one place in the core that reads the secret.

**The options that were weighed:**

| Option | Verdict | Why |
| --- | --- | --- |
| Keep the group-event secret | **v0 only** | It works today. It breaks the rule above, at the cost above |
| An exporter of our own: `MLS-Exporter` with a label and context for radio | **The target** | It is what Marmot's rule asks for, and it needs nothing from the other member's app. MDK computes three fixed exporters and refuses other labels, so MDK must first export a label the app chooses (#58) |
| MDK's safe exporter (`safe_export_secret`), for an app component of ours | No | It works only for a component the chat requires, and every member's app must then support it. White Noise doesn't |
| No derived key: the message, or the key, travels as an ordinary MLS message of a new kind | No | See below |

**Why not an ordinary MLS message.** A comment on #128 asks why radio needs keys of its own at all. There are two ways
to read that:

- **The radio message as an MLS message.** It doesn't fit. An MLS message carries its framing and a signature, around
  a signed event, and that leaves no room for text in a 120-byte packet ([The text budget](#the-text-budget)). This is
  why the spec asks for a compact format with keys from the chat's MLS state.
- **A random radio key, sent in the chat as a message of a new kind.** It avoids the exporter, but it brings back the
  key exchange that deriving the keys avoids. The key must reach the contact over the relays before the outage, a new
  one must follow each epoch change to keep MLS's key renewal, and the two phones must agree on which of them sends
  it. The new kind is also an identifier for Marmot to assign, as an exporter context is, so it needs a proposal just
  the same.

**Before the radio transport proposal (#8) or a public release:**

1. MDK exports a secret under a label and context that the app chooses (#58).
2. `secret(e)` becomes that exporter, under a provisional radio context until Marmot assigns one.
3. The format's version goes up, since v0 packets no longer open, and the test vectors are made again.

`core/tests/issue_128_radio_key_source.rs` keeps this section and the code in step. It fails if the core reads
another secret or uses another label than the ones named here.

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
inputs in CI.

**Receiving.** `RadioInbox` in `core/src/radio/inbox.rs` applies the rules above to every incoming packet (#42):

- It keeps each direct chat's keys for four epochs.
- It finds the chat by the sending radio, or by trying each chat's keys.
- It opens the packet and checks its counter against a 64-wide replay window per epoch.
- It refuses a message whose id the chat has already.

Only an authentic packet counts against the window, so a forgery can't use up a counter. The windows live in memory; after a
restart, a replayed packet is caught by its message id, because received radio messages are stored with it (#40). A
refusal returns nothing to show and nothing to send.

**On the radio** (#40, `core/src/app/radio.rs`). The core uses the radio the user paired (#24); the tests hand it mock
radios. When `send_message` chooses radio (#45), or the switch that forces radio is on, it sends like this:

1. MDK takes the message and keeps it until a relay can be reached. MDK's id is the id above.
2. The core rebuilds that id from the account key, the time and the text. If it differs, as for a text with mentions,
   the message waits for the internet.
3. It takes the chat's newest kept keys: the current ones if the chat is settled, else the last it took. A chat can't
   give keys while it waits for a relay to confirm a change, so the core takes them every minute while it has a radio.
4. It stores the next counter for the chat and epoch, then seals the packet and hands it to the radio, addressed to
   the contact's radio from the address book (#34).

Every packet the radio receives goes through the inbox. Before giving up on one, the core takes the chats' current
keys once and tries again, in case a chat has moved to a new epoch.

`core/src/radio_store.rs` keeps, in `radio.json`:

- **the counters,** written and synced before the packet goes out. A damaged file means lost counters: each chat then
  sends again only from its next epoch, whose key has never been used.
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
- keys from an exporter of radio's own: v0 reuses the group-event secret
  ([Where the keys come from](#where-the-keys-come-from));
- an independent security review.

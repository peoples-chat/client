# Spike #37: radio keys from the chat's MLS state

> **Replaced on 27 November 2026 ([#128](https://github.com/peoples-chat/client/issues/128)).** Radio keys no longer
> come from the chat's MLS state. Each member's app makes a random key and sends it inside the chat's MLS messages, as
> an app event of our own kind, 9912: [radio-format-v0.md, "Where the keys come
> from"](../radio-format-v0.md#where-the-keys-come-from). The secret this spike used,
> `MLS-Exporter("marmot", "group-event", 32)`, is reserved by Marmot for the outer encryption of kind-445 events. What
> follows is the spike as it was, kept for what it found about MDK. The code and the tests it names are gone.

**Result: works, without changes to MDK.** Tested 22 November 2026 with MDK v0.18.0.

Both members of a direct chat derive the same radio keys from the chat's MLS state, so radio needs no key exchange of
its own:

- The core reads an MLS exporter secret that MDK already computes for every epoch.
- It derives one key per direction from it, with HKDF and a label of the app's own.
- `core/tests/radio_keys.rs` has two cores derive matching keys, keep them across a restart, and move to new ones when
  the epoch changes.

The rules for epochs and nonces below are what the radio format (#38) and its implementation (#39) build on.

## What MDK lets the app export

These calls are public in `marmot-app`:

| Call | What it gives | Usable here |
| --- | --- | --- |
| `runtime.accounts().exporter_secret(account, group, label, length)` | One of the three exporter secrets MDK computes for each epoch. The current epoch only. | Yes |
| `runtime.safe_export_secret(account, group, component)` | The MLS safe exporter of an app component (MLS extensions draft) | No |
| `runtime.group_mls_state(account, group)` | The epoch, and whether a change is in flight (`lifecycle_state`) | Yes |

**The three exporter secrets** are all `MLS-Exporter("marmot", context, 32)`:

| MDK's name | Context | MDK uses it for |
| --- | --- | --- |
| `marmot/group-event` | `group-event` | The key for the chat's events on the relays |
| The encrypted-media key | `encrypted-media` | Encrypted media |
| `marmot/agent-text-stream-quic` | `agent-text-stream-quic` | Agent text streams |

Any other label fails with "missing exporter secret for label". MDK computes exactly these three whenever it is asked,
so an app can't ask for an exporter of its own.

**The safe exporter** works only for an app component that the group lists as required. Direct chats that MDK starts
don't list ours. Adding one would be a change to the group that every member's app has to accept, White Noise
included.

**Past epochs.** MDK keeps no past epoch's exporter secret that the app can read. It keeps some for encrypted media,
internally.

## The keys

```text
secret(e)   = MLS-Exporter("marmot", "group-event", 32) in epoch e
key(e, S)   = HKDF-SHA256(salt: none, IKM: secret(e), info: "peoples.chat/radio/v0" || S's public key), 32 bytes
```

**One key per direction.** S is the sender: Ada sends with key(e, Ada) and receives with key(e, Bea), and Bea the other
way round. This keeps the two senders' nonces apart, and a packet reflected back to its sender fails to decrypt.

**The label is provisional and unregistered.** Marmot assigns exporter contexts only when a spec is adopted.

- A registered context, or the safe exporter, can replace the group-event secret later.
- Proposing one belongs with the radio format proposal (#56, #58).

**Why the spike thought the group-event secret safe to build on** (wrong: see the note at the top):

- HKDF with a label of our own keeps the radio keys separate from MDK's key for relay events. Knowing a radio key
  reveals neither the secret nor that key.
- Anyone who has the secret is a member of the chat, or got it from one: the same boundary the chat already has.
- In a two-person chat, being a member means being one of the two. This is the member-level authentication the spec
  asks for.

**Code:**

- `RadioKeys::derive` in `core/src/radio/keys.rs`, with a test vector computed separately, with Python's `hmac`.
- `Messaging::chat_secret` reads the secret; `Core::radio_keys` puts the two together.
- The keys never leave the core, and they show as redacted in logs.

## Epochs

The keys change with every MLS epoch: when a member joins or leaves, or updates their own key.

**What we saw** (`radio_keys_change_with_the_epoch`, an ignored test because it is slow):

- The chat starts in epoch 0. Adding the contact starts epoch 1.
- After joining, MDK updates the new member's key by itself: epoch 2, 100 seconds later.
- MDK updates each member's key every 24 to 36 days.
- An update asked for now (`Core::update_own_key`) waits for a minute without changes to the chat, plus up to 30
  seconds. It took 99 seconds.

**Changes need the relays.** A commit takes effect only once a relay has confirmed it, and nothing about a chat's
membership or keys travels by radio. During an outage both phones stay in the epoch of their last sync.

**Changes in flight.** While the account's own change waits for the relays, MDK hands out the secret of the epoch that
change starts, but still reports the epoch before. `Messaging::chat_secret` therefore reads only while the chat is
settled (`Stable`). Otherwise it fails: "the chat is changing". By MDK's code, a key update that falls due during an
outage stays in flight until the relays are back (not tried here). The core then can't read the current epoch's
secret during the outage.

**Rules for v0:**

1. **The core keeps keys.** It keeps the radio keys of the chat's last four epochs, derived whenever it sees the chat
   settled in a new epoch. MDK keeps no past secrets for the app, and during an outage the current one may be out of
   reach (above).
2. **The sender** uses the newest epoch it holds keys for, and puts the epoch's lowest 8 bits in the packet as a hint.
3. **The receiver** decrypts with the kept key whose epoch matches the hint.
4. **A receiver that is behind** has no key for the sender's epoch: the sender confirmed a change that the receiver
   never fetched. It drops the packet. The sender publishes the same message to the relays when the internet returns
   (spec: Message format on radio), and that copy reaches it.

Not built in this spike: the key store (#39).

## Nonces

A key stays the same for the whole epoch, which can be weeks and several outages long. A nonce must never repeat under
one key.

**Rules for v0:**

1. **The AEAD is ChaCha20-Poly1305** (RFC 8439): a 96-bit nonce, and a 256-bit key. MDK's dependencies already bring it
   in. The tag length is #38's choice.
2. **The nonce is a counter:** the sender's message counter for the chat and epoch, big-endian, padded to 96 bits. Each
   epoch's keys start at 0. Per-direction keys mean the two senders can't collide.
3. **The sender stores the counter before it sends.** The core stores the next counter for each chat and epoch before
   a packet goes out, so a crash can't reuse one. Losing that store while MDK keeps the chat would allow reuse. #39
   keeps both in the same data folder.
4. **The packet carries only the counter's low bits.** The receiver rebuilds the full counter from the highest it has
   seen, as DTLS does with record numbers. How many bits is #38's choice. Radio sends are capped per hour (#51), so 16
   bits leave a wide margin within an epoch.
5. **The counter is also the replay window** (#42).

A random 64-bit nonce would need no stored state, but costs 8 bytes per packet instead of about 2. It remains the
fallback if storing the counter proves fragile.

## Answers to the issue

- **Does MDK let the app run an MLS exporter?** Partly. The app can read three fixed exporter secrets of the current
  epoch, but can't choose its own label. That is enough: a key derived from the group-event secret.
- **The smallest change, if needed.** None for the PoC. Later, ask Marmot for a registered radio context or for safe
  exporter support in direct chats (#58).
- **A provisional context.** `peoples.chat/radio/v0`, as the HKDF label, marked unregistered.
- **Which epoch, when the phones last synced at different epochs.** The sender's newest kept epoch, with an 8-bit hint.
  The receiver keeps four epochs and drops what it has no key for. The relay copy follows.
- **The nonce or counter scheme.** A per-direction counter, stored before sending, with its low bits on the wire.

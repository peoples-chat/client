# Radio transports in the core

The core talks to a paired LoRa radio through a radio transport (#60). Each mesh is a separate transport: radios of
different meshes can't talk to each other, and a radio runs one mesh at a time. MeshCore comes first, through
[meshcore-rs](https://github.com/andrewdavidmackenzie/meshcore-rs). Meshtastic follows after the MVP.

The core sends and receives chat text through a radio it is handed (#40), while the switch that forces radio is on.
Pairing a real radio comes with #24, and the path for the packets with #36; until then the tests use mock radios.
[radio-format-v0.md](radio-format-v0.md#in-the-core) describes how the core sends and receives.

**Addresses.** Each app learns its contacts' radios from their radio address records (#34), with nobody doing
anything:

- **The record** (`core/src/radio/address.rs`) is small, versioned JSON: the mesh, the radio's public key, its LoRa
  settings (frequency in kHz, bandwidth in Hz, spreading factor, coding rate) and when it was made, in milliseconds.

  ```json
  {"v":0,"mesh":"meshcore","key":"…64 hex…","freq":869618,"bw":62500,"sf":8,"cr":8,"at":1795300000000}
  ```

- **It travels** as an app event of kind 9911 inside the direct chat ([mdk.md](mdk.md#app-events)): never in a group,
  never published.
- **Sending.** With a radio, the core sends its record in every direct chat that hasn't had this one: when the radio
  is attached, when a chat starts, and every minute for chats it couldn't reach. A different radio, or the same one with
  other settings, makes a new record; the same radio keeps the time of its first record, so it isn't sent again.
- **Receiving.** The newest record of a chat's contact becomes their radio in `radio.json`. Records that came while the
  app was closed are read when it starts, and those in a request when it is accepted. A record of another version or
  mesh, or with a malformed key or settings out of LoRa's range, changes nothing.
- **What shows.** A contact with a radio has `meshes: ["meshCore"]`, and Settings → Radio counts them under "Your
  contacts by mesh".

**Keys.** Radio messages are encrypted with keys derived from the chat's MLS state (#37), with one key per direction.
`core/src/radio/keys.rs` derives them, and [spikes/37-radio-keys.md](spikes/37-radio-keys.md) has the finding,
including the rules for epochs and nonces.

**The format.** [radio-format-v0.md](radio-format-v0.md) describes a chat message in one packet (#38), with test
vectors from `scripts/radio_vectors.py`. `core/src/radio/format.rs` implements it (#39), and `core/fuzz` fuzzes its
decoder. `RadioInbox` decides which incoming packets to accept: authentic, new to the replay window, and not known
already (#42).

## The interface

`Radio` in `core/src/radio/mod.rs`:

| Method | What it does |
| --- | --- |
| `mesh()` | Which mesh the radio runs |
| `info()` | Public key, name, LoRa settings (frequency, bandwidth, spreading factor, coding rate, power), firmware and model |
| `contacts()` | The contacts the radio holds: public key, name, kind (person, repeater, room, sensor), when last heard, whether a route is known |
| `send_text(to, text)` | Sends text to a contact. Returns the code that the delivery confirmation will carry, and how long it may take |
| `events()` | Text received, delivered, contact heard, disconnected |
| `disconnect()` | Ends the link |

- **Connecting** depends on the mesh and the link, so each implementation has its own constructors.
- **Keys** are hex strings, as elsewhere in the core.
- **The trait is object-safe** (`async-trait`), so the core can hold radios of different meshes side by side.

**Implementations:**

- **`MeshCoreRadio`** (`core/src/radio/meshcore.rs`):
  - `connect_serial(port)` connects over USB serial on desktops.
  - `over(commands, dispatcher)` works over meshcore-rs's command handler and event dispatcher. That is for a link
    the app runs itself, such as Bluetooth on phones (#23), and for tests.
- **`MockMesh` and `MockRadio`** (`core/src/radio/mock.rs`): radios on a mesh in memory. They hear each other, confirm
  delivery, and can be taken out of range. They're for tests and for interface work without hardware.

**Text** is at most 160 bytes of UTF-8 and has no NUL character. `send_text` refuses anything else before it reaches
the radio. Shorter limits for the mesh's sake come with #51.

## Trying a radio

Plug a MeshCore radio with companion firmware for USB into the computer and run:

```sh
cargo run -p peoples-chat-core --example radio_info                          # lists serial ports
cargo run -p peoples-chat-core --example radio_info -- /dev/cu.usbmodem1101  # describes the radio on that port
```

It prints the radio's public key, name, LoRa settings, firmware and contacts. It only reads: it changes nothing on the
radio. Waiting for radios: #12.

## meshcore-rs

**Version and licence.**

- 0.2.0 (16 August 2026), pinned exactly in `core/Cargo.toml`. The licence is MIT. It is a port of the project's
  Python client, `meshcore_py`.
- Everything it brings in is MIT, Apache or BSD, except `serialport` (MPL-2.0), which comes with USB serial. MPL-2.0
  applies per file: our code can take any licence, and changes to `serialport`'s own files would have to stay open.
  This matters for the licence decision (#9).

**Platforms.**

| Platform | Link | Features |
| --- | --- | --- |
| Desktop | USB serial | `serial` |
| iOS, Android | Bluetooth through the app (#23), passed in with `MeshCoreRadio::over` | none |

- With no feature, the protocol code still builds, and `cargo check` passes for iOS and Android with every feature.
- **iOS** apps can't open serial ports.
- **Android** needs `UsbManager` for USB. meshcore-rs's own Bluetooth (`btleplug`) needs extra Java setup there, and
  on Apple it can connect only by name.

**What it covers.** At MeshCore companion firmware v1.18.0 (15 October 2026, firmware code 14):

| The PoC needs | Companion protocol | meshcore-rs 0.2.0 |
| --- | --- | --- |
| Identity and settings | `APP_START` → `SELF_INFO`, `DEVICE_QUERY` → `DEVICE_INFO` | Yes |
| Contacts | `GET_CONTACTS`, `ADD_UPDATE_CONTACT`, `REMOVE_CONTACT`, `IMPORT_CONTACT` | Yes. `EXPORT_CONTACT` returns garbled text |
| Finding a contact by key | `GET_CONTACT_BY_KEY` (30) | No |
| Sending text | `SEND_TXT_MSG` → `SENT` with the ACK code and a timeout | Yes, first attempt only |
| Delivery confirmation | push `SEND_CONFIRMED` | Yes (the round-trip time is dropped) |
| Receiving text | push `MSG_WAITING`, then `SYNC_NEXT_MESSAGE` until none are left | Yes, both message formats |
| Hearing contacts | pushes `ADVERT`, `NEW_ADVERT` | `ADVERT` keeps only the key's prefix; `NEW_ADVERT` is right |
| Raw data | `SEND_RAW_DATA` (25), push `RAW_DATA` | Receiving only |
| Channel data | `SEND_CHANNEL_DATA` (62), `CHANNEL_DATA_RECV` (27), since v1.15 | No. Its main branch adds `send_channel_data()`, unreleased as of 22 November 2026 |
| Resetting a route | `RESET_PATH` (13) | No |
| Radio settings | `SET_RADIO_PARAMS` (11) | No. The app never changes them without asking anyway |
| Path discovery, trace, control data, statistics, CLI | 52, 36, 55, 56, 66 | No, or parsed wrongly |

Any command can still be sent as hand-built bytes through meshcore-rs's `send`.

**Problems found, and what the adapter does about them:**

- **The radio's errors arrive as timeouts.** Only two commands look for the radio's ERR frame. The adapter checks the
  contact and the text first, so the common errors get a real message.
- **`ADVERT` and `PATH_UPDATED` are parsed in an old layout.** Only the key's first six bytes are right. The adapter
  finds the contact by that prefix, reading the contacts again for one the radio has just added.
- **Unknown frames.** A queued channel-data frame (27) is an unknown frame to meshcore-rs. Collecting messages then
  waits 5 seconds before it moves on.
- **Replies are matched by event type only.** A radio that answers instantly can be missed, because meshcore-rs
  starts listening after it writes. Real links are slow enough; the test radio waits 20 ms.
- **A dropped `MeshCore` keeps running.** Its tasks and the serial port stay open, so call `disconnect()`. There is
  no reconnect.
- **Others:** the `STATUS_RESPONSE`, `TELEMETRY_RESPONSE`, `TRACE_DATA` and `CONTROL_DATA` layouts are wrong.
  `max_contacts` is capped at 255. The doc comments give the frequency in the wrong unit: the radio reports kHz, and
  the bandwidth in Hz.

None of this blocks the PoC. Raising it upstream belongs to #58.

## MeshCore facts that shape the PoC

These come from the firmware source at `companion-v1.18.0`.

**Text messages:**

- At most 160 bytes. A NUL character ends the text.
- The recipient's radio confirms on receipt with a 4-byte code, which `SENT` announced. Delivered means it reached
  the radio, not that someone read it.
- **The firmware never retries, and never reports a timeout.** An app retries by sending again with the same
  timestamp and the attempt number raised; meshcore-rs always sends attempt 0. `meshcore_py` tries three times, and
  falls back to flooding after two direct attempts.
- **Retries that get through arrive twice.** Drop duplicates by sender, timestamp and text.

**The radio's queue:**

- It keeps 16 messages for the app. When it is full, a new direct message is dropped, so the adapter collects
  messages as soon as the radio says some wait.
- Collecting takes messages off the radio, so the adapter waits until something listens (`events()`).
- **Every link sees the same replies.** A phone app on Bluetooth and our app on USB, connected at once, share one
  queue. Collecting from one takes the messages from the other.

**Binary data for #36.** Companions can't send each other binary direct messages at v1.18.0. The options:

| Option | Encrypted | Confirmed | Limit | Notes |
| --- | --- | --- | --- | --- |
| Text-encoded data in a plain message | Yes | Yes | 160 bytes, no NUL | Works with meshcore-rs today |
| `SEND_RAW_DATA` | No | No | About 170 bytes | Direct or one hop only; reaches only an app that is connected at the time |
| Channel data on a private channel (62) | By the channel key | No | 165 bytes | No sender identity; not in meshcore-rs 0.2.0 |

## Tests

`core/tests/radio.rs`:

- **Mock radios:** they exchange text and confirm delivery; a radio out of range loses messages; contacts heard; the
  errors.
- **`MeshCoreRadio` against a fake companion radio:** the fake answers meshcore-rs's commands with frames laid out as
  firmware v1.18.0 sends them. The test covers:
  - the identity and LoRa settings, with their units;
  - firmware and model;
  - contacts;
  - sending, with the ACK code and the confirmation;
  - receiving a message with its SNR;
  - adverts;
  - refusing unknown contacts and long text;
  - disconnecting;
  - keeping messages that waited until the first listener.

# Radio transports in the core

The core talks to a paired LoRa radio through a radio transport (#60). Each mesh is a separate transport: radios of
different meshes can't talk to each other, and a radio runs one mesh at a time. MeshCore comes first, through
[meshcore-rs](https://github.com/andrewdavidmackenzie/meshcore-rs). Meshtastic follows after the MVP.

The core sends and receives chat text through the radio the user paired (#40). It sends by radio when no relay is
reachable and the contact's radio has been heard (#44, #45), or while the switch that forces radio is on. Computers pair
a radio over USB ([Pairing](#pairing), #24); phones will over Bluetooth (#23). The path for the packets comes with #36.
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

**Airtime** (#51, `core/src/radio/airtime.rs`). The core counts the time each packet occupies the channel and keeps
what the user sends within an hourly budget:

- **Time on air** follows Semtech's LoRa formula for an explicit header and a CRC, with a 16-symbol preamble to err
  on the long side. A MeshCore text message on air is 6 bytes plus the encrypted timestamp, flags and text in 16-byte
  blocks. In the lab, the lengths on air were the worked-out ones; the time on air itself wasn't measured
  ([radio-paths.md](radio-paths.md#measured-in-the-lab)).
- **In the EU/UK narrow preset** (SF8, 62.5 kHz, 4/8), the longest message the app sends (160 characters of base64)
  takes about 1.6 seconds, and a short one about 0.6 seconds.
- **The cap** is 36 seconds an hour (`HOURLY_AIRTIME`): a tenth of the 10% duty cycle that the EU allows in 869.4 to
  869.65 MHz at 500 mW e.r.p. ([CEPT ERC Recommendation 70-03](https://docdb.cept.org/download/2369), Annex 1; check
  the current edition's footnotes, which also allow listen-before-talk with adaptive frequency agility instead). The
  rest is left for the radio's own traffic, its acknowledgements and the shared mesh. That is about 22 of the longest
  messages an hour, or over 40 short ones.
- **The core enforces it.** A message that doesn't fit in what is left of the hour waits (#46). Once even the shortest
  wouldn't fit, the chat is `unreachable` with `radioIssue: airtimeUsed`, and its notice says "Your radio has used its
  airtime for this hour." When airtime comes free, the waiting messages go.

**The transmission log** (#52). Every packet the app makes the radio send is logged in `radio.json` for two days, with
its time, size, airtime and reason (`message`: one the user sent, or one that waited; `retry`: one sent again for want
of a confirmation). `Core::radio_transmissions` returns it. The cap counts it, and the tests compare it with what the
user sent. The firmware's own adverts and acknowledgements aren't the app's, and aren't in it.

## Confirmation

The other radio confirms a text message on receipt (#41). That means it reached the radio, not that anyone read it,
and it is all the app claims: a message is `confirmed`, or `timedOut` when no confirmation came, never more.

- **How long the core waits** for each try: the radio's own estimate, which `SENT` carries, plus a second for the
  confirmation to come over the link from the radio (`CONFIRM_MARGIN`). Companion v1.18.0 estimates 0.5 s plus
  16 times the packet's airtime for a flood, and for a direct message 0.5 s plus 6 times the airtime and 0.25 s for
  each link on the path: about 10 s to 27 s for the longest message in the EU/UK narrow preset. In the lab, on its
  faster private preset and a desk, confirmations took 0.65 s to 1.2 s, far less than the estimate
  ([radio-paths.md](radio-paths.md#measured-in-the-lab)). `Core::set_radio_confirm_wait` replaces the estimate, for
  tests and tuning.
- **One retry** (`RADIO_RETRIES`), because each try costs shared airtime. It goes only while the chat is still
  reachable by radio (the contact's radio heard, the same preset, airtime left), the hour's airtime has room for it,
  and no relay carries the message instead, unless radio is forced. It is logged with the reason `retry`.
- **The retry sends the same packet again.** meshcore-rs gives it a new timestamp, so the mesh doesn't drop it as a
  repeat, and the firmware gives it its own confirmation code; either code confirms the message. If the first try got
  through after all, the other phone drops the second copy as a replay (#42).
- **Without a confirmation after the retry**, the message is `timedOut`. Its relay copy still goes once a relay is
  back, as for every radio message (#47), and then it is `sent`. A confirmation that comes later still counts.
- **Across a restart**, retries stop. A message whose confirmation was due meanwhile is `timedOut`; one still due is
  waited for until then.
- **The radio can confirm before `send_text` returns**, as the mock radios always do. The core keeps such a
  confirmation until the message it belongs to is kept.

`core/tests/radio_confirmation.rs` checks a confirmed message, a retry that gets through, a message to a radio that is
switched off, a restart, and the relays' return; and that no retry goes once the contact's radio isn't heard anymore,
or the hour's airtime has no room for it.

## Pairing

A computer pairs a MeshCore companion radio over USB (#24): the user plugs it in and picks its port in Settings →
Radio → Pair a radio. The radio's public key becomes the user's address on the mesh, which the core shares with every
contact as its radio address record (#34). [core-interface.md](core-interface.md#radio) has the commands and what the
interface shows.

- **The ports** (`RadioPorts` in `core/src/radio/ports.rs`). `SerialPorts` lists the computer's USB serial ports.
  A Mac lists each one twice, as `/dev/cu.…` and `/dev/tty.…`; only the `cu` one is kept, because the `tty` one waits
  for a carrier when it is opened. Bluetooth ports and debug consoles are left out. Linux without udev can't say what
  is behind a port, so its USB serial ports are known by their names (`ttyACM`, `ttyUSB`).
- **Pairing** opens the radio on the chosen port with `MeshCoreRadio::connect_serial`, reads its key, name and
  settings, and keeps them in `radio.json` with the port and the USB device's serial number. A device that doesn't
  answer as a companion radio within meshcore-rs's timeout isn't paired, and its port is let go of at once.
- **The preset.** The interface shows the radio's frequency, bandwidth, spreading factor and coding rate, and warns
  when they aren't MeshCore's EU/UK (Narrow) preset: 869.618 MHz, 62.5 kHz, spreading factor 8, coding rate 4/8
  (`EXPECTED_PRESET`). It compares the same four numbers as reach does (#44).
- **Read only.** Pairing and opening the radio again send the companion protocol's greeting, the device query and
  the contact list request, and nothing that changes a setting. Changing the radio's frequency or region is left to
  the MeshCore app.
- **Opening it again.** The core opens the paired radio at start, and when its link drops, because the radio restarted
  or its cable was pulled. It looks on the radio's own port, or on another port with its USB serial number, so it is
  found when it is plugged in elsewhere. It waits a second before the first look after a drop, then twice as long each
  time, up to 10 seconds. A look only lists the ports until the radio's port is there, so it costs little. Another
  radio on that port is let go of again: the radio must have the paired key.
- **While the app runs it holds the radio's port.** Quit it to use the radio with another app over USB.
- **Forgetting** closes the link and forgets the radio. Deleting the account forgets it too.

`core/tests/radio_pairing.rs` checks this with mock radios on mock ports (`MockPorts`): listing, pairing, a device that
isn't a radio, a port that went, the preset and its warning, remembering the radio across a restart, opening it again
after it restarts or moves to another port, not taking another radio for it, and forgetting it, also with the
account. Trying it on a real radio waits for hardware day (#12).

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
  - `connect_serial(port)` connects over USB serial on desktops. A radio that doesn't answer its greeting, or that is
    dropped, lets go of the port, so that it can be opened again (#24).
  - `over(commands, dispatcher)` works over meshcore-rs's command handler and event dispatcher. That is for a link
    the app runs itself, such as Bluetooth on phones (#23), and for tests.
- **`MockMesh` and `MockRadio`** (`core/src/radio/mock.rs`): radios on a mesh in memory. They hear each other, confirm
  delivery, and can be taken out of range. They're for tests and for interface work without hardware. `MockPorts`
  plugs them into mock USB ports, for pairing (#24).

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

In the app on the computer (`pnpm tauri dev`), Settings → Radio → Pair a radio lists the same ports and pairs the
radio ([Pairing](#pairing)). Quit `radio_info` first: only one program can hold the port.

With two radios, `path_spike` measures the three paths for the app's packets (#36). Steps:
[radio-paths.md](radio-paths.md#running-the-spike). What it measured in the lab:
[radio-paths.md](radio-paths.md#measured-in-the-lab).

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
| `SEND_RAW_DATA` | No | No | 172 bytes | Along a path the app gives, never a flood; reaches only an app that is connected at the time |
| Channel data on a private channel (62) | By the channel key | No | 165 bytes | No sender identity; not in meshcore-rs 0.2.0 |

[radio-paths.md](radio-paths.md) compares the three in detail, from the firmware source and from measurements between
two radios, and recommends text messages.

## Fake radios over TCP

`fake-radio/` (`peoples-chat-fake-radio`, #132) is a companion radio in software, for testing radio code without
hardware. meshcore-rs connects to it with `MeshCore::tcp` (its `tcp` feature), and the core's radio then works over that
link:

```rust
let (ada, bea) = pair(Config::default(), Config::default()).await?;
let addr = ada.addr();
let link = MeshCore::tcp(&addr.ip().to_string(), addr.port()).await?;
let radio = MeshCoreRadio::over(link.commands().clone(), link.dispatcher()).await?; // keep `link`
```

- **Ports.** Each fake listens on 127.0.0.1, on a free port unless `Config::port` names one, so tests run in parallel.
- **Commands.** App start, device query, contacts, sending a direct message and collecting received ones, with the
  message-waiting and send-confirmed pushes, reading and setting the clock (never backwards), and sending an advert.
  An advert is stamped with the sender's clock, and a radio that holds the sender as a contact reports it only if it
  is later than the last one it took. Any other command gets the "unsupported" error. `set_clock` puts a clock off,
  and `lose_next_messages` loses a chosen number of messages.
- **Pairs.** `pair` starts two radios that hear each other and hold each other as contacts; `link` does so for two that
  are already running. `add_contact` adds more.
- **Knobs** (`Config`, per radio):

  | Field | Effect |
  | --- | --- |
  | `loss` | Chance that a packet this radio sends over the air is lost: its messages, and its confirmations of what it received |
  | `duplication` | Chance that such a packet arrives twice. The app hears of a confirmation once |
  | `delay` | How long such a packet takes to arrive |
  | `max_contacts` | Size of the contact table. Reported halved, as firmware does |
  | `seed` | Repeats the same choice of what is lost or duplicated |
  | `reply_latency` | How long the radio takes to answer its app, 20 ms by default (see [meshcore-rs](#meshcore-rs)) |

- **Fixtures.** `Fixture::load` reads captured frames and `FakeRadio::replay` answers with them. A fixture is a text
  file with one frame per line, in order: `app` or `radio`, a space, then the payload in hex without the stream
  framing. `#` starts a comment line. The app's commands are compared by their first byte only; `replay_mismatches` and
  `replay_remaining` tell whether the app followed the capture.
- **What it isn't.** One hop, no routes, new contacts from adverts, channels or airtime. The signal strength is always 7.25 dB. Its
  frames follow the layouts above, not a comparison with a real radio, and the fixture in `fake-radio/tests/fixtures`
  is built by hand until real captures exist (#133).

## Tests

`fake-radio/tests/issue_132_fake_radio.rs` runs meshcore-rs against the fake: each command, a pair, each knob and the
replay. `core/tests/issue_132_radio_over_tcp.rs` runs `MeshCoreRadio` over a pair.

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

# Radio paths for the app's packets (#36)

Which MeshCore mechanism should carry the app's radio packets between two companion radios? This page compares the
candidates as the firmware source describes them, and explains how to measure them with the `path_spike` example.

**Status: prepared on 23 November 2026, before the radios arrived. Nothing here is measured yet.** Hardware day fills in
the blanks under [What hardware day fills in](#what-hardware-day-fills-in). The finding and the recommendation follow
from those measurements.

## The candidates

At companion firmware v1.18.0, an app can send another companion radio:

1. **A text message to a contact** (`SEND_TXT_MSG`). Format v0 ([radio-format-v0.md](radio-format-v0.md)) uses this,
   with the packet as base64.
2. **Raw data** (`SEND_RAW_DATA`): a `RAW_CUSTOM` packet holding any bytes.
3. **Channel data** (`SEND_CHANNEL_DATA`, since v1.15): a datagram on a group channel. Here that is a private channel
   that only the two radios hold.

#36 also lists the binary request command (`SEND_BINARY_REQ`). It can't carry app data between two companions. The
receiving companion answers telemetry requests itself, and passes any other request to a listener inside the firmware,
never to the connected app (`MyMesh::onContactRequest`). The spike leaves it out.

## What the firmware source says

From the companion firmware at `companion-v1.18.0`.

| | Text message | Raw data | Channel data |
| --- | --- | --- | --- |
| Addressed to | One contact: a 1-byte hash of each key | Nobody: every companion in range passes it to its app | Every radio holding the channel: a 1-byte hash of its secret |
| MeshCore's encryption | Per contact: AES-128 with a 2-byte MAC | None | The channel's key: AES-128 with a 2-byte MAC |
| Routing | Floods until the radio knows a route, then direct. The radio learns the route itself | Direct only, along a path the app gives. Zero hop reaches only the radios in range. A flood is refused | A flood, or direct along a path the app gives |
| Confirmation | Yes. `SENT` gives the code and a timeout, then `SEND_CONFIRMED` arrives | None: OK means it is queued to send | None: OK means it is queued to send |
| Retries | None by the firmware. The app sends again with the attempt raised | None | None |
| Longest payload | 160 bytes of text, with no NUL | 172 bytes | 165 bytes |
| Chat text in a format v0 packet | 96 bytes: a 120-byte packet is 160 characters of base64 | 148 bytes | 141 bytes |
| On air, zero hop, for an n-byte payload | 6 + 16 × ⌈(5 + n) / 16⌉ | 2 + n | 5 + 16 × ⌈(3 + n) / 16⌉ |
| When the receiving app isn't connected | Kept in the radio's queue of 16 frames | Lost: it is pushed once | Kept in the queue, but the first to go when it is full |
| Who sent it | The sender's key prefix, from the contact list | Not said | Not said |
| meshcore-rs 0.2.0 | Sends the first attempt, with refusals reported as timeouts. Receives | Receives only | Neither |
| meshcore-rs master (`407b647`, 22 November) | Also reads the radio's refusals | Receives only | Sends floods of up to 163 bytes. Receives |

**On-air sizes.**

- **Text message:** 6 bytes are the header, the path byte, the two hashes and the MAC. The 5 bytes are the timestamp
  and flags, encrypted with the text in 16-byte blocks.
- **Channel data:** 5 bytes are the header, the path byte, the channel's hash and the MAC. The 3 bytes are the data
  type and length, encrypted with the data.
- **Repeaters and scopes:** each repeater on a path adds its hash (1 byte by default). A radio with a flood scope adds
  4 bytes of transport codes to its floods.

**Each path's limit, in detail.**

- **Text.** `MAX_TEXT_LEN` is 160. Longer text fails to compose, and the radio answers `TABLE_FULL`.
- **Raw data.**
  - The serial link carries frames of up to 176 bytes (`MAX_FRAME_SIZE`). The radio cuts a longer command frame to 176
    without an error, so a zero-hop frame holds at most 174 bytes of payload.
  - The receiving radio puts 4 bytes in front of the payload in its push, and drops a push frame over 176 bytes. So 172
    bytes arrive, while 173 or 174 bytes go on air and vanish.
- **Channel data.** `MAX_GROUP_DATA_LENGTH` is 165 (184 − 16 − 3). Longer data is refused with `ILLEGAL_ARG`.
  meshcore-rs's master uses 163, from an older frame limit.

**Other facts that matter here.**

- **Raw data reaches everyone in range.** Zero hop, every companion in range whose app is connected receives it. Only
  the format's key decides who can open it.
- **Confirmation comes from the radio.** The receiving radio confirms a text message on receipt, whether or not its app
  is connected.
- **A text flood is answered with a flood.** The receiving radio floods its confirmation together with the route back.
- **Over USB the app always counts as connected.** `ArduinoSerialInterface::isConnected()` always returns true, so
  pushes go out even when nothing has the port open. Over Bluetooth, connected means a Bluetooth connection.
- **The radio logs what it hears.** It reports every packet it hears, up to 173 bytes, as `LOG_RX_DATA`. The spike
  uses this to check the worked-out on-air sizes.
- **The radio has its own airtime budget.** Its default airtime factor of 1.0 allows half the time on air. The spike
  stays far below that.

## What hardware day fills in

| Question | Text message | Raw data | Channel data | From |
| --- | --- | --- | --- | --- |
| Delivered intact, zero hop, at each size | | | | the table |
| Median time to arrive | | | | the table |
| Median time to the confirmation | | | | the table |
| The longest chat text that always arrived | | | | the summary under the table |
| Logged on-air sizes match the worked-out ones | | | | `logged` against `on air` |
| Copies of a flood heard: repeaters in range | | refused | | `copies` |
| Receiver's link closed | | | | `--offline` |
| Receiver switched off | | | | `--radio-off` |
| Through one repeater | | | | `--via` |
| What a stock MeshCore phone app shows on radio B | | | | by eye |
| Whether opening the serial port restarts the radio, which empties its queue | | | | `--offline` |

## Choosing the path

1. **Rule out** a path that doesn't deliver the v0 packet intact, zero hop, nearly every time at the sizes the app
   sends.
2. **Among the rest, prefer in this order:**
   1. it waits on the radio when the receiving app isn't connected, because phones suspend apps;
   2. it is confirmed;
   3. it works without the app knowing a route, so it can flood;
   4. its text budget comes closer to the 100 to 150 bytes the spec aims for;
   5. it takes less airtime per message.
3. **What the source predicts:**
   - **Text messages** come out ahead on waiting, confirmation and routing. The price is 96 bytes of text, and base64
     adds a third to the time on air.
   - **Channel data** waits and floods. But it has no confirmation, which an app-level one would have to add at its own
     airtime (#41), and it doesn't say who sent it.
   - **Raw data** needs a route that the app has to learn, and is lost when the app isn't listening.
4. **A proposal to MeshCore.** If text messages win, none is needed. If the budget matters more, a binary direct
   message would need one: for example, a data type for text messages, or raw data that floods and waits on the radio.
   That belongs with #58.

## Running the spike

You need two radios with companion firmware for USB, on the same preset (EU/UK narrow), plugged into one desktop. The
expectations come from firmware v1.18.0, and the tool says so when a radio runs another version. Check each radio first
with `radio_info` ([radio.md](radio.md#trying-a-radio)).

```sh
cargo run -p peoples-chat-core --example path_spike                     # lists the serial ports
cargo run -p peoples-chat-core --example path_spike -- --plan           # what a run sends, without radios
cargo run -p peoples-chat-core --example path_spike -- PORT_A PORT_B    # measures, from radio A to radio B
```

1. **Look at the plan.** `--plan` lists every send with its packet, payload and on-air sizes, its airtime, and what the
   source expects. The default run makes 78 sends. Of those, 66 go on air, including 12 floods, for about 77 seconds on
   air over about 13 minutes.
2. **Run the main measurement.** On a desk, lower the power so that floods reach fewer repeaters, for example
   `--tx-power 2`. The tool sets the power back at the end.
3. **Check what happens when the receiver is away:** `--offline --radio-off`.
   - `--offline` closes radio B's link while radio A sends, then opens it again.
   - `--radio-off` asks you to switch radio B off. Unplugging isn't enough if it has a battery.
4. **Go through one repeater:** `--via HASH`.
   - Find the repeater with `radio_info`: it is a contact of kind Repeater, and its hash is the first byte of its key,
     in hex.
   - The repeater must hear radio A. Radio B may hear radio A directly too, but it ignores a direct packet whose next hop
     is the repeater.
   - A run with `--via` sends only direct packets.
5. **Measure the other direction** by swapping the ports.
6. **Look at a stock phone app.** Connect the MeshCore phone app to radio B over Bluetooth, run
   `--paths raw,channel --texts 48 --repeat 1 --flood-repeat 0`, and note what the app shows.
   - The phone app and the tool then share radio B's queue, so the tool may miss channel data.

**What a run changes on the radios.**

- **Contacts:** for text messages, each radio gets the other as a contact. They stay.
- **Test channel:** both radios hold a private channel, `pc-spike`, with a random secret. It goes in a free slot: by
  default the last slot both radios have, or `--channel-index`. The tool refuses a slot in use, and empties it at the
  end.
- **Power:** with `--tx-power`, both radios use that transmit power until the end.
- **Route:** with `--via`, radio A's route to radio B goes through the repeater. At the end the tool forgets it, and the
  radios learn the route again.
- **Stopping:** Ctrl-C stops after the current send, and the tool still puts everything back and writes the results.
  Ctrl-C a second time ends the tool at once.

**How it measures.**

- **Packets.** It sends format v0 packets with chat text of the app's sizes. By default these are 10, 48 and 96 bytes,
  and sizes on each side of each path's limit. A text message carries the packet as base64; raw data and channel data
  carry its bytes. Floods are sent only at sizes up to 96 bytes, because every repeater in range passes them on.
- **Order and pace.** It sends round by round, every path in each round, so that changes over time spread across the
  paths. It keeps radio A on air for at most 10% of the time, the EU limit for 869.4 to 869.65 MHz (`--duty-cycle`
  lowers it). A send the radio refuses isn't repeated.
- **What it records for each send:**
  - the radio's answer;
  - the confirmation, for text messages;
  - when radio B passed the packet on, and the route it came by;
  - whether it arrived intact and opened with the key;
  - SNR and RSSI;
  - its length on air as radio B logged it, and how many copies radio B heard;
  - what the source expects.

  One clock times both ends, because both radios are on the same desktop.
- **Output.** It prints a table by path, route and size, and each path's longest chat text that always arrived. Every
  send goes to a CSV file (`--out`, or `path-spike-<unix time>.csv`).

## The code and its tests

`core/examples/path_spike/`:

| File | What it does |
| --- | --- |
| `frames.rs` | The companion frames meshcore-rs 0.2.0 lacks, the limits and on-air sizes from the firmware source |
| `plan.rs` | Options, the schedule, the packets and the pace |
| `link.rs` | One radio's link: everything it reports, with when it arrived. It collects the radio's queue |
| `report.rs` | The table, the budgets and the CSV |
| `fake.rs` | Two fake radios for the tests |

It reads the airtime formula from `core/src/radio/airtime.rs` (#51).

`cargo test -p peoples-chat-core --example path_spike` runs its tests, and so does `cargo test -p peoples-chat-core`. They
never open a serial port. They cover:

- **The frames,** checked against the firmware's layouts, including a channel datagram as the firmware queues it.
- **The source's numbers:** each path's limit, the packet and on-air sizes, and the lengths in the radio's log.
- **Arguments, the schedule and the pace.**
- **Whole runs against two fake radios.** The fakes answer as firmware v1.18.0 does, and the runs include a channel
  slot in use and a route through a repeater.

**Sources.** In the firmware at `companion-v1.18.0`:

- `examples/companion_radio/MyMesh.cpp` and `MyMesh.h`: the frames, the queue, confirmations and the radio's log;
- `src/Mesh.cpp` and `src/Packet.cpp`: routing, paths and packets;
- `src/helpers/BaseChatMesh.cpp`: text messages, acknowledgements, routes and channel data;
- `src/helpers/ArduinoSerialInterface.cpp`: the serial link;
- `src/MeshCore.h`, `src/helpers/BaseChatMesh.h` and `src/helpers/BaseSerialInterface.h`: the limits.

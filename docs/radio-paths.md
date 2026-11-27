# Radio paths for the app's packets (#36)

Which MeshCore mechanism should carry the app's radio packets between two companion radios? This page compares the
candidates as the firmware source describes them, gives what was [measured](#measured-in-the-lab) between two radios,
[recommends](#recommendation) one, and explains how to measure again with the `path_spike` example.

**Status: measured on 27 November 2026 (#140)** between the lab radios R1 and R2, on a desk, zero hop. Both are Seeed
Wio Tracker L1 with companion firmware v1.18.0-81dbae5.

**Every measured number here is on the private lab preset, 869.850 MHz, 62.5 kHz, SF 7, CR 5, 2 dBm, not EU/UK Narrow**
(869.618 MHz, SF 8, CR 8). Times on air and times to arrive are longer on EU/UK Narrow. Nothing here says anything about
range. A repeater hop and a stock MeshCore app are still to be tried (#36).

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

## Measured in the lab

All of this is on the private lab preset, **869.850 MHz, 62.5 kHz, SF 7, CR 5, 2 dBm, not EU/UK Narrow**, between two
radios on a desk, with no repeater. Chat text is the text inside a format v0 packet.

The captures, from `pnpm verify:radio --path-spike` ([radio-lab.md](radio-lab.md#measurements)):

- **Every send:** [R1 to R2](radio-paths/lab-r1-to-r2.csv) (10 direct and 3 flood rounds, and the receiver away) and
  [R2 to R1](radio-paths/lab-r2-to-r1.csv) (5 direct and 2 flood rounds).
- **Every frame** of the R1 to R2 run, as fixtures for the fake radio (#132):
  `fake-radio/tests/fixtures/lab-path-spike-R1.frames` and `…-R2.frames`.

### Payload limit

Lab preset (869.850 MHz, 62.5 kHz, SF 7, CR 5, 2 dBm). Each limit was met from both sides, and matches the source.

| | Longest that arrived | One step longer | Chat text at the limit |
| --- | --- | --- | --- |
| Text message | 160 characters | 162: refused, `TABLE_FULL` | 96 B |
| Raw data | 172 bytes | 173: sent on air, never passed on | 148 B |
| Channel data | 165 bytes | 166: refused, `ILLEGAL_ARG` | 141 B |

### Delivery, time to arrive and confirmation

Lab preset (869.850 MHz, 62.5 kHz, SF 7, CR 5, 2 dBm). Each cell is delivered intact of sent, then the median time from
handing the packet to radio A until radio B's app had it, then (`ack`) the median time until radio A reported the
confirmation. Only text messages are confirmed. A refused send never went on air.

**R1 to R2:**

| Route | Chat text | Text message | Raw data | Channel data |
| --- | --- | --- | --- | --- |
| zero hop | 10 B | 10/10, 324 ms, ack 653 ms | 10/10, 218 ms | 10/10, 273 ms |
| zero hop | 48 B | 10/10, 468 ms, ack 798 ms | 10/10, 332 ms | 10/10, 365 ms |
| zero hop | 96 B | 10/10, 656 ms, ack 985 ms | 10/10, 476 ms | 10/10, 510 ms |
| zero hop | 97 B | refused | 10/10, 477 ms | 10/10, 510 ms |
| zero hop | 126 B | refused | 10/10, 560 ms | 10/10, 604 ms |
| zero hop | 141 B | refused | 10/10, 612 ms | 10/10, 656 ms |
| zero hop | 142 B | refused | 10/10, 612 ms | refused |
| zero hop | 148 B | refused | 10/10, 632 ms | refused |
| zero hop | 149 B | refused | 0/10 | refused |
| flood | 10 B | 3/3, 324 ms, ack 695 ms | refused | 3/3, 271 ms |
| flood | 48 B | 3/3, 468 ms, ack 841 ms | refused | 3/3, 365 ms |
| flood | 96 B | 3/3, 655 ms, ack 1026 ms | refused | 3/3, 511 ms |

**R2 to R1:**

| Route | Chat text | Text message | Raw data | Channel data |
| --- | --- | --- | --- | --- |
| zero hop | 10 B | 5/5, 323 ms, ack 653 ms | 5/5, 218 ms | 5/5, 272 ms |
| zero hop | 48 B | 5/5, 469 ms, ack 799 ms | 5/5, 332 ms | 5/5, 365 ms |
| zero hop | 96 B | 5/5, 656 ms, ack 985 ms | 5/5, 476 ms | 5/5, 510 ms |
| zero hop | 97 B | refused | 5/5, 477 ms | 5/5, 511 ms |
| zero hop | 126 B | refused | 5/5, 560 ms | 5/5, 604 ms |
| zero hop | 141 B | refused | 5/5, 611 ms | 5/5, 655 ms |
| zero hop | 142 B | refused | 5/5, 611 ms | refused |
| zero hop | 148 B | refused | 5/5, 632 ms | refused |
| zero hop | 149 B | refused | 0/5 | refused |
| flood | 10 B | 2/2, 322 ms, ack 694 ms | refused | 2/2, 272 ms |
| flood | 48 B | 2/2, 467 ms, ack 839 ms | refused | 2/2, 365 ms |
| flood | 96 B | 2/2, 655 ms, ack 1026 ms | refused | 2/2, 511 ms |

- Within its limit, every packet arrived intact and opened with the key. This is a strong link on a desk: it shows that
  the paths work, not how often they deliver at a distance.
- Every text message that was delivered was also confirmed.
- Zero hop, R1 to R2, time to arrive was the worked-out airtime plus 46 to 62 ms.
- The radio's own estimate of how long a confirmation may take was far longer than it took: 2.6 s to 4.6 s direct and
  5.5 s to 10.8 s for a flood.
- Each flood short enough to be logged was heard once: there is no repeater in range.

### On-air length against the worked-out size

Lab preset (869.850 MHz, 62.5 kHz, SF 7, CR 5, 2 dBm), zero hop, R1 to R2. "Logged" is the packet's length as radio B
reported hearing it. A radio logs packets up to 173 bytes only. Every logged length, in both directions, was the
worked-out one. The airtime is worked out from the length (#51), not measured.

| Path | Chat text | Payload | Worked out | Logged | Airtime |
| --- | --- | --- | --- | --- | --- |
| text | 10 B | 46 B | 70 B | 70 B | 272.9 ms |
| text | 48 B | 96 B | 118 B | 118 B | 416.3 ms |
| text | 96 B | 160 B | 182 B | not logged | 600.6 ms |
| raw | 10 B | 34 B | 36 B | 36 B | 170.5 ms |
| raw | 48 B | 72 B | 74 B | 74 B | 283.1 ms |
| raw | 96 B | 120 B | 122 B | 122 B | 426.5 ms |
| raw | 97 B | 121 B | 123 B | 123 B | 426.5 ms |
| raw | 126 B | 150 B | 152 B | 152 B | 508.4 ms |
| raw | 141 B | 165 B | 167 B | 167 B | 559.6 ms |
| raw | 142 B | 166 B | 168 B | 168 B | 559.6 ms |
| raw | 148 B | 172 B | 174 B | not logged | 580.1 ms |
| raw | 149 B | 173 B | 175 B | not logged | 580.1 ms |
| channel | 10 B | 34 B | 53 B | 53 B | 221.7 ms |
| channel | 48 B | 72 B | 85 B | 85 B | 313.9 ms |
| channel | 96 B | 120 B | 133 B | 133 B | 457.2 ms |
| channel | 97 B | 121 B | 133 B | 133 B | 457.2 ms |
| channel | 126 B | 150 B | 165 B | 165 B | 549.4 ms |
| channel | 141 B | 165 B | 181 B | not logged | 600.6 ms |

### SNR and RSSI

Lab preset (869.850 MHz, 62.5 kHz, SF 7, CR 5, 2 dBm). As radio R2 logged R1's packets, which it does up to 173 bytes.
The app itself gets SNR for text and channel data, and SNR and RSSI for raw data.

| Path | Packets logged | SNR | RSSI |
| --- | --- | --- | --- |
| text | 26 | 11.5 to 12.75 dB | -59 to -58 dBm |
| raw | 70 | 11.25 to 12.5 dB | -59 to -58 dBm |
| channel | 59 | 11.25 to 13 dB | -59 to -58 dBm |

### The receiver away

Lab preset (869.850 MHz, 62.5 kHz, SF 7, CR 5, 2 dBm). One 48-byte chat text on each path while radio B's USB link was
closed, then the link opened again.

| Path | What happened | Confirmation |
| --- | --- | --- |
| text | queued on the radio | 798 ms |
| raw | lost | none |
| channel | queued on the radio | none |

Opening the serial port didn't restart the radio: what it had queued was still there.

### Not tried

| Question | Why |
| --- | --- |
| Receiver switched off (`--radio-off`) | The lab has no per-port power switch |
| Through one repeater (`--via`) | The lab has no repeater. It stays on #36 |
| What a stock MeshCore phone app shows | It stays on #36 |
| EU/UK Narrow, other power, distance | The lab radios stay on the lab preset |

## The text budget against 100 to 150 bytes

The longest chat text that arrived intact every time, zero hop, on the lab preset (869.850 MHz, 62.5 kHz, SF 7, CR 5,
2 dBm). The limits are in bytes, so they are the same on any preset.

| Path | Chat text | Against the target |
| --- | --- | --- |
| text | 96 B | 4 B short of 100 |
| raw | 148 B | inside 100 to 150 |
| channel | 141 B | inside 100 to 150 |

## Recommendation

**Use text messages** (`SEND_TXT_MSG`), as format v0 already does.

- All three paths delivered everything within their limits, so delivery rules none out here.
- Only text messages had all of: waiting on the radio while the app was away, a confirmation, and flooding until the
  radio knows a route.
- **The price is the budget: 96 bytes, 4 short of the 100 to 150 the spec aims for.** It is also the most airtime: for
  48 bytes of chat text 416 ms, against 283 ms for raw data and 314 ms for channel data, before the confirmation's own
  packet (lab preset: 869.850 MHz, 62.5 kHz, SF 7, CR 5, 2 dBm; worked out).
- **Channel data** is the runner-up, with 141 bytes. It has no confirmation and doesn't say who sent it, the radio drops
  it first when its queue is full, and meshcore-rs 0.2.0 can't send or read it.
- **Raw data** has the largest budget, 148 bytes, but was lost when the app was away, can't flood and isn't confirmed.

**No proposal to MeshCore is needed for the PoC.** Text messages work with the firmware and meshcore-rs as they are. If
100 bytes or more is a firm requirement, that changes: a binary direct message that is queued and confirmed like text
would need a proposal (#58). The other way to gain the 4 bytes is a smaller overhead in the format, which this
measurement doesn't look into.

Posted on #36: COMMENT_LINK

## Adverts the radios send on their own

ADVERTS

## Choosing the path

The rule behind the recommendation, set before the measurements:

1. **Rule out** a path that doesn't deliver the v0 packet intact, zero hop, nearly every time at the sizes the app
   sends.
2. **Among the rest, prefer in this order:**
   1. it waits on the radio when the receiving app isn't connected, because phones suspend apps;
   2. it is confirmed;
   3. it works without the app knowing a route, so it can flood;
   4. its text budget comes closer to the 100 to 150 bytes the spec aims for;
   5. it takes less airtime per message.

## Running the spike

In the radio lab, run it through the harness, which takes the lab lock, finds the radios and logs every frame:
`pnpm verify:radio --path-spike --offline` ([radio-lab.md](radio-lab.md#measurements)). Elsewhere, run it alone, as
follows.

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

# The radio lab harness

`pnpm verify:radio` (#133) runs the hardware checks against the radios in the lab. It is separate from
`pnpm verify`, which needs no hardware.

```sh
pnpm verify:radio             # checks every USB radio, then tests the first two on the air
pnpm verify:radio --dry-run   # lists the radios it found and their firmware
```

`npm run verify:radio` runs the same script. The code is in `core/examples/verify_radio/`.

## What a run does

1. **Reads the lab file**, `~/factory/radios.toml`. Without it, the run stops before touching anything.
2. **Takes the lab lock**, `~/factory/locks/radio-lab`, and holds it until the end.
3. **Finds each USB radio** by its USB serial number. Port names such as `/dev/cu.usbmodem1101` change, and are
   never used to tell radios apart.
4. **Checks each radio** before anything else happens: its firmware version and public key against its entry, its
   frequency, bandwidth, spreading factor and coding rate against `[lab]`, and its transmit power against
   `max_tx_power_dbm`. All radios are checked, every difference is reported, and then the run fails.
5. **Checks that each radio still responds**, by asking for its contacts. A dry run stops after step 4.

A run that isn't dry then goes on to the [over-the-air tests](#the-over-the-air-tests). Bluetooth comes with #135: a
radio without a `usb_serial` is listed as not checked.

It exits with 0 when all passed, 1 when something failed, 2 for a wrong argument and 130 when interrupted.

## The over-the-air tests

After the checks, the first two USB radios in the file are tested on the air (#134, `messages.rs`). Two apps, each with
its own core and an empty data folder, become contacts through a relay inside the harness, which also carries their
radio address records. The relay is then shut, so only the radios can carry what follows. Each core pairs its radio
over USB as the app does.

| Check | Passes when |
| --- | --- |
| Pairing (#24) | Each app pairs its radio, and warns about the preset unless it is EU/UK (Narrow) |
| Address records (#34) | Each app stores the other radio's key |
| Reachability (#44) | Neither contact is reachable by radio until their radio's advert is heard |
| Messages (#40, #41) | A message each way arrives and is acknowledged within 60 s, with at most one retry |
| Reconnecting (#24) | The second radio's serial port is closed for 5 s; the app opens it again by itself, and the next message arrives |

The times to arrive and to be acknowledged are printed, with the radio's own estimate.

**What goes on the air:** one zero-hop advert from each radio (sent again up to twice if not heard), three short
messages (four when an advert isn't reported), any retry, and the radios' own acknowledgements.

**When an advert isn't reported.** A radio ignores an advert that isn't stamped later than the last one it took from
that contact, so a radio whose clock is behind goes unheard. Neither the app nor the harness sets a radio's clock. The
run then prints a `WARNING` with both times, and each app hears the other by a message and its acknowledgement instead,
sent with the app's force-radio switch. The run still passes: read the warnings.

**Closing the port isn't a power cycle.** The radio keeps running. The harness closes the port, tells the app the link
ended, and hides the port until it "plugs it in" again.

`pnpm verify` runs the same tests against two fake radios (`hardware_tests.rs`).

## The lab file

```toml
[lab]
preset = "private-test"
frequency_mhz = 869.850
bandwidth_khz = 62.5
spreading_factor = 7
coding_rate = 5
max_tx_power_dbm = 2

[[radio]]
label = "R1"
usb_serial = "0123456789ABCDEF"
firmware = "v1.18.0-81dbae5"   # exactly as the radio reports it
public_key = "abcd…"           # optional; all 64 hex digits, checked when given
hub_port = "1-1.4:2"           # optional: <hub location>:<port>, as uhubctl names them
```

Other fields are ignored. A `hub_port` in angle brackets, such as `"<none: …>"`, means there is none.

| Environment variable | Default |
| --- | --- |
| `RADIOS_TOML` | `~/factory/radios.toml` |
| `RADIO_LAB_LOCK` | `~/factory/locks/radio-lab` |
| `RADIO_LOG_DIR` | `.factory/radio/` in the repository |
| `UHUBCTL` | `uhubctl` on the `PATH` |

## The lock

The lock is a file that exists while a run holds it. It names the process and when it started. It is created in one
step, so two runs can't both get it, and it is removed after success, failure, a panic, Ctrl-C, SIGTERM and SIGHUP.

A process that is killed outright (SIGKILL, power loss) leaves it behind. The next run doesn't take it over: it fails,
says that the lock is stale, and a person removes the file once sure that no one is using the radios.

Anything else that uses the radios must take the same lock the same way: create the file, and fail if it exists.

## A radio that stops responding

A radio that doesn't answer, or is no longer on USB, gets its power cycled once:
`uhubctl -l <location> -p <port> -a cycle -d 3`. The run then waits up to 60 seconds for it, and checks it again as in
step 4. Without a `hub_port`, the run fails and says to restart the radio by hand. A dry run never cycles power. A
radio with the wrong firmware or settings, or a port that another program holds, is never cycled either.

Cycling a hub port restarts only a radio that has no battery of its own.

## What the harness never does

It never changes a radio's firmware, frequency, preset or power. Every frame goes through the harness's own serial
link (`link.rs`), which lets only these companion commands through: app start (1), send text (2), contacts (4), time
(5), advert (7), add or update a contact (9), next message (10), reset path (13), battery (20) and device query (22).
Anything else is refused and logged as `blocked`.

## The frame log

Each run writes `frames-<unix ms>-<pid>.jsonl`: one line for every frame, in order.

```json
{"at_ms":1795700000000,"dir":"to_radio","frame":"16…","radio":"R1"}
```

- `dir` is `to_radio`, `from_radio`, or `blocked` for a command the harness refused to send.
- `frame` is the companion frame in hex, without the serial link's start byte and length. A `from_radio` frame can be
  handed to meshcore-rs's `MessageReader::handle_rx` as it is, so the log serves as fixtures for a fake radio.
- The radio's Bluetooth PIN and every position (the radio's own, its contacts' and those in adverts heard) are zeroed,
  because fixtures end up in the repository. Lengths don't change.

Next to the log, each run saves one fixture per radio, `frames-…-R1.frames`, in the format the fake radio's
`Fixture::load` reads ([radio.md](radio.md#fake-radios-over-tcp)). They are saved also when a test fails.

A log and its fixtures still hold public keys, contact names and message packets. Look at them before committing or
publishing them.

## Adding a hardware test

`Lab::run` in `lab.rs` has every checked radio as a `Station`, with its `MeshCoreRadio`: the same adapter the app
uses. A test added there runs under the lock, after the checks, with its frames logged.

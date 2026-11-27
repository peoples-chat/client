# The radio lab harness

`pnpm verify:radio` (#133) runs the hardware checks against the radios in the lab. It is separate from
`pnpm verify`, which needs no hardware.

```sh
pnpm verify:radio             # checks every USB radio, then that each still responds
pnpm verify:radio --dry-run   # lists the radios it found and their firmware
pnpm verify:radio --path-spike --offline   # measures the MeshCore paths from R1 to R2 (#140)
pnpm verify:radio --adverts 75             # listens for the radios' own adverts for 75 minutes (#140)
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

The checks send nothing over the air. The message tests come with #134, and Bluetooth with #135: a radio without a
`usb_serial` is listed as not checked.

It exits with 0 when all passed, 1 when something failed, 2 for a wrong argument and 130 when interrupted.

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

The path spike's links also let through raw data (25), reading and setting a channel slot (31, 32) and channel data
(62). It puts its test channel in a free slot and empties it at the end, and leaves each radio holding the other as a
contact. It refuses `--tx-power` and `--radio-off`.

## Measurements

Both run after the checks above, under the lock, on the first two USB radios of the lab file.

- **`--path-spike`** runs [path_spike](radio-paths.md#running-the-spike) from the first radio to the second, or back
  with `--reverse`. It takes the spike's other options. The CSV goes beside the frame log, or to `--out`. Interrupting
  ends at once: the next run finds its test channel (`pc-spike`) and empties it.
- **`--adverts MINUTES`** keeps both radios open, sends nothing, and reports every advert each one hears. At the end it
  has each radio send one zero-hop advert, and fails unless the other hears it.
- **`--fixtures FRAME_LOG FOLDER NAME`** uses no radios: it writes a frame log as fixtures for the fake radio (#132),
  one file per radio.

## The frame log

Each run writes `frames-<unix ms>-<pid>.jsonl`: one line for every frame, in order.

```json
{"at_ms":1795700000000,"dir":"to_radio","frame":"16…","radio":"R1"}
```

- `dir` is `to_radio`, `from_radio`, or `blocked` for a command the harness refused to send.
- `frame` is the companion frame in hex, without the serial link's start byte and length. A `from_radio` frame can be
  handed to meshcore-rs's `MessageReader::handle_rx` as it is, so the log serves as fixtures for a fake radio.
- The radio's Bluetooth PIN, channel secrets and every position (the radio's own, its contacts', and those in logged
  adverts) are zeroed, because fixtures end up in the repository. Lengths don't change.

A log still holds public keys, contact names and, in later tests, message packets. Look at one before committing it.

## Adding a hardware test

`Lab::stations` in `lab.rs` has every checked radio as a `Station`, with its `MeshCoreRadio`: the same adapter the app
uses. A test added there runs under the lock, after the checks, with its frames logged.

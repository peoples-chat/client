# The radio lab harness

`pnpm verify:radio` (#133) runs the hardware checks against the radios in the lab. It is separate from
`pnpm verify`, which needs no hardware.

```sh
pnpm verify:radio                     # checks every radio, then runs the hardware tests
pnpm verify:radio --dry-run           # lists the radios it found and their firmware; sends nothing
pnpm verify:radio --usb-only          # leaves the radios on Bluetooth alone
pnpm verify:radio --allow-bluetooth   # once, by a person at the Mac: see Bluetooth below
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
5. **Checks that each radio still responds**, by asking for its contacts. A dry run skips this.
6. **Runs the Bluetooth test** for each radio with a `ble_name` and no `usb_serial` ([Bluetooth](#bluetooth)). A dry
   run finds and checks the radio over Bluetooth, and stops there.

Up to step 5 the harness sends nothing over the air. The message tests between USB radios come with #134. With
`--usb-only`, and on computers other than Macs, a radio without a `usb_serial` is listed as not checked.

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

[[radio]]
label = "R3"
ble_name = "MeshCore-0123456789AB"   # the name the radio advertises over Bluetooth
firmware = "v1.18.0-81dbae5"
public_key = "abcd…"
reboot_to_drop_link = true           # optional, off by default: see Bluetooth below
```

Other fields are ignored. A `hub_port` in angle brackets, such as `"<none: …>"`, means there is none.

Two switches let the Bluetooth test do what it otherwise refuses to. Both are off unless the file turns them on, so
that a person decides:

| Switch | Where | What it allows |
| --- | --- | --- |
| `exchange_adverts = true` | `[lab]` | Two radios that don't hold each other as contacts announce themselves once (command 7). A radio that hears an announcement adds its sender, which changes what the radios store |
| `reboot_to_drop_link = true` | the Bluetooth radio's `[[radio]]` | The harness restarts that radio (command 19) to drop its Bluetooth link. A restart changes no firmware, frequency or preset |

| Environment variable | Default |
| --- | --- |
| `RADIOS_TOML` | `~/factory/radios.toml` |
| `RADIO_LAB_LOCK` | `~/factory/locks/radio-lab` |
| `RADIO_LOG_DIR` | `.factory/radio/` in the repository |
| `UHUBCTL` | `uhubctl` on the `PATH` |
| `VERIFY_RADIO_BLE_PROGRAM` | the harness's own binary: the program that holds the Bluetooth link |

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

## Bluetooth

The Bluetooth test (#135) reaches a radio through meshcore-rs's own Bluetooth link, the code under
`MeshCoreRadio::connect_ble` ([radio.md](radio.md#the-interface)). The radio must already be paired with the Mac.

**Two processes.** macOS decides about Bluetooth by the program *responsible* for the one that uses it. For anything
started from a terminal, an agent or cargo, that is the terminal or the agent: macOS ends the program, or asks on the
screen. So the side that holds the Bluetooth link runs as its own launchd job
(`chat.peoples.factory.verify-radio-ble`): the harness's own binary, started with `--ble-child` through
`launchctl submit`, with the reason it uses Bluetooth built in (`Info.plist`). The run keeps the lab lock and the USB
radios, and tells that side what to do over a socket on the same computer. The job is removed when the run ends,
however it ends. Its output is next to the frame log, as `frames-….bluetooth.log`.

A small Bluetooth-to-TCP bridge behind launchd would have been simpler to keep allowed, but it would test the
bridge's Bluetooth code and skip meshcore-rs's, which is what the app is to use.

**What a run does**, for the Bluetooth radio (R3) and the first USB radio (R1):

1. Finds R3 by its advertised name, connects, and checks its firmware, key and settings against the lab file, exactly
   as for a USB radio. On any difference the run stops before anything is sent to R3 or through it.
2. Makes sure R1 and R3 hold each other as contacts. If they don't, the run fails, unless `exchange_adverts` allows
   them to announce themselves.
3. Sends a message from R1, which must arrive on R3 over Bluetooth, and one from R3 over Bluetooth, which must arrive
   on R1.
4. Restarts R3, if `reboot_to_drop_link` allows it, which drops the link from the radio's end. The link's own checks
   must notice. The same process must then find the radio and connect again, and a message from R1 must arrive over
   the new link. Without the switch the run fails here and says so: it doesn't pass with the drop untested.

**No prompt.** The Bluetooth side first asks macOS whether it is allowed (`CBManager.authorization`), which uses no
Bluetooth and shows nothing. Unless the answer is "allowed", it never touches Bluetooth, and the run fails with what
to do. Nothing a run does can put a dialog on the screen.

**Allowing it** takes a person at the Mac, once:

```sh
pnpm verify:radio --allow-bluetooth
```

This is a dry run that may ask. macOS shows its dialog for "verify:radio"; click Allow. If macOS asks to pair, the
radio isn't paired with this Mac yet.

The harness isn't signed with an identity, so macOS should tie the permission to the exact binary, and **a rebuilt
harness must then be allowed again**: after any change to the harness or the core. (Expected from how macOS records
permissions; not yet tried in the lab.) To keep one allowed binary across rebuilds, copy
`target/debug/examples/verify_radio` somewhere stable, set `VERIFY_RADIO_BLE_PROGRAM` to the copy, and allow that
one with `--allow-bluetooth`. The copy then holds the Bluetooth link for every run, with the Bluetooth code it was
built with, until the socket's protocol changes; the run says when it does. A binary signed with a real identity
would stay allowed across rebuilds. Whether the lab gets one is a person's decision, not the harness's.

**Not through the harness's serial link.** meshcore-rs writes to the radio itself over Bluetooth, so these frames
are not in the frame log and no list of commands filters them. The Bluetooth side can only call `MeshCoreRadio`,
which sends app start (1), send text (2), contacts (4), next message (10), battery (20) and device query (22), and,
when the lab file allows it, advert (7) and reboot (19).

## What the harness never does

It never changes a radio's firmware, frequency, preset or power. Every frame to a USB radio goes through the
harness's own serial link (`link.rs`), which lets only these companion commands through: app start (1), send text (2),
contacts (4), time (5), advert (7), add or update a contact (9), next message (10), reset path (13), battery (20) and
device query (22). Anything else is refused and logged as `blocked`. Over USB that includes reboot (19); the one
radio the harness may restart is a Bluetooth radio whose entry says `reboot_to_drop_link = true`
([Bluetooth](#bluetooth)).

## The frame log

Each run writes `frames-<unix ms>-<pid>.jsonl`: one line for every frame, in order.

```json
{"at_ms":1795700000000,"dir":"to_radio","frame":"16…","radio":"R1"}
```

- `dir` is `to_radio`, `from_radio`, or `blocked` for a command the harness refused to send.
- `frame` is the companion frame in hex, without the serial link's start byte and length. A `from_radio` frame can be
  handed to meshcore-rs's `MessageReader::handle_rx` as it is, so the log serves as fixtures for a fake radio.
- The radio's Bluetooth PIN and every position (the radio's own and its contacts') are zeroed, because fixtures end
  up in the repository. Lengths don't change.

A log still holds public keys, contact names and, in later tests, message packets. Look at one before committing it.

## Adding a hardware test

`Lab::run` in `lab.rs` has every checked radio as a `Station`, with its `MeshCoreRadio`: the same adapter the app
uses. A test added there runs under the lock, after the checks, with its frames logged.

The Bluetooth test is in `ble.rs`: `run` is the run's side and `serve` the side that holds the link. `ble_tests.rs`
runs both against fake radios, with a task in place of the launchd job.

# The radio lab harness

`pnpm verify:radio` (#133) runs the hardware checks against the radios in the lab. It is separate from
`pnpm verify`, which needs no hardware.

```sh
pnpm verify:radio              # checks every radio, then tests them on the air
pnpm verify:radio --dry-run    # lists the radios it found and their firmware; sends nothing
pnpm verify:radio --usb-only   # leaves the radios on Bluetooth alone
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
5. **Checks that each radio still responds**, by asking for its contacts. A dry run skips this and step 6.
6. **Sets each radio's clock** to the computer's time (`set_time`, command 6), and prints what it read before and
   after. This happens once every radio has passed the checks, and before anything goes on the air. A radio ignores an
   advert that isn't stamped later than the last one it took from that contact, so one whose clock is behind goes
   unheard. The clock isn't a protected setting; the MeshCore app sets it on every connect. A radio doesn't move its
   clock back: one that is ahead keeps its time, and the run says so and goes on.
7. **Runs the Bluetooth test** for each radio with a `ble_name` and no `usb_serial`, through the
   [bridge](#the-bluetooth-bridge). A dry run connects and checks the radio, and stops there. Without a running bridge
   the radio is skipped, the run says `SKIPPED`, and it can still pass. A Bluetooth radio's clock isn't set. With
   `--usb-only`, a radio without a `usb_serial` is listed as not checked.

A run that isn't dry then goes on to the [over-the-air tests](#the-over-the-air-tests) between the first two USB radios.
A failed Bluetooth test ends the run before them.

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

**What goes on the air** in these tests (the [Bluetooth test](#run-the-test) adds its own): one zero-hop advert from each radio (sent again up to twice if not heard), three short
messages, any retry, and the radios' own acknowledgements.

**When an advert isn't reported** after three tries, the run fails, and says what the sender's clock reads, the advert
time the other radio holds for it, and the computer's time. No message is sent instead. If the held time is in the
future, setting the clocks can't help: that contact's stored time has to go first.

The app itself doesn't set a radio's clock; only the harness does.

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

[[radio]]
label = "R3"
ble_name = "MeshCore-0123456789AB"   # the name the radio advertises over Bluetooth
firmware = "v1.18.0-81dbae5"
public_key = "abcd…"
bridge_port = 5535                   # optional: where the bridge passes frames, on 127.0.0.1
bridge_control_port = 5536           # optional: the port after bridge_port
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

## The Bluetooth bridge

macOS asks for Bluetooth permission per program, and an agent can't click Allow. So everything that uses Bluetooth for
the lab is in one small program, `ble-bridge` (#135, Swift with CoreBluetooth, `tools/ble-bridge/`). It is built once,
installed at a fixed path, run as its own launchd job and allowed once by a person. The harness never touches
Bluetooth: the core reaches the radio over TCP on this computer, with meshcore-rs's TCP link
(`MeshCoreRadio::connect_tcp`). meshcore-rs's own Bluetooth code is therefore not under test.

### What it does

- Finds the radio that advertises `ble_name`, connects over the Nordic UART service and passes companion frames both
  ways. The radio must already be paired with the Mac.
- **Frames**, on `127.0.0.1:<bridge_port>`: MeshCore's stream framing, as over serial. One client at a time; a new one
  replaces the old. While there is no Bluetooth link, a client is turned away, and when the link is lost the client's
  connection is closed.
- **Control**, on `127.0.0.1:<bridge_control_port>`: one command per line. `status` answers `connected` or
  `disconnected`. `drop` disconnects the Bluetooth link and answers `dropped`.
- Looks for the radio again, 2 seconds after any lost link.
- **Never raises a prompt by itself.** Before anything touches Bluetooth it reads `CBManager.authorization`. Unless
  the answer is "allowed", it says so and exits with code 77.

Anything on this computer can connect to the two ports while the bridge runs, and so use the radio. The lab Mac is
for this only; stop the bridge when that matters.

### Install it (a person, at the Mac)

Needs Xcode's command line tools (`xcrun swiftc`). From the repository:

```sh
tools/ble-bridge/install.sh                     # builds ~/factory/bin/ble-bridge and its launchd job
tools/ble-bridge/install.sh --allow-bluetooth   # once: macOS asks on the screen, click Allow
```

The first command stops with a note that Bluetooth isn't allowed yet. The second runs the bridge once as a launchd job
that may ask, and after Allow starts the real job. The job is `chat.peoples.factory.ble-bridge`
(`~/Library/LaunchAgents/chat.peoples.factory.ble-bridge.plist`), started at login, and restarted if it crashes. Its
log is `~/factory/logs/ble-bridge.log`. It reads the radio's name and the ports from `radios.toml` when it starts, so
run `install.sh` again after changing them.

The program is built only when it isn't there, because macOS ties the permission to the exact program: it isn't signed
with an identity. `--rebuild` builds it again, and then `--allow-bluetooth` is needed again. (Expected from how macOS
records permissions; not yet tried in the lab.) `--uninstall` stops the job and removes both files.

`BLE_BRIDGE_BIN`, `RADIOS_TOML` and `BLE_BRIDGE_LOG` change the paths. `pnpm verify` never installs or starts the
bridge. Its tests build their own copy under `target/` and run it only with a fake radio.

### Run the test

```sh
pnpm verify:radio --dry-run   # connects to R3 through the bridge and checks it; sends nothing
pnpm verify:radio             # the whole run
```

For the Bluetooth radio (R3) and the first USB radio (R1), a run:

1. Asks the bridge's control port. If nothing listens, R3 is skipped.
2. Waits up to 45 seconds for the bridge to have its link, connects, and checks R3's firmware, key and settings against
   the lab file, exactly as for a USB radio. On any difference the run fails before anything is sent to R3 or through
   it.
3. Makes sure R1 and R3 hold each other as contacts. If they don't, both announce themselves once (command 7), and a
   radio that hears an announcement adds its sender.
4. Sends a message from R1, which must arrive on R3, and one from R3, which must arrive on R1.
5. Has the bridge drop the Bluetooth link. The core must notice. The same process then connects again, checks R3 again,
   and a message from R1 must arrive over the new link.

A bridge that runs but can't reach the radio fails the run; it isn't skipped.

## What the harness never does

It never changes a radio's firmware, frequency, preset or power. Every frame to a USB radio goes through the
harness's own serial link (`link.rs`), which lets only these companion commands through: app start (1), send text (2),
contacts (4), time (5), set time (6), advert (7), add or update a contact (9), next message (10), reset path (13),
battery (20) and device query (22). Anything else is refused and logged as `blocked`.

Frames to the Bluetooth radio go through meshcore-rs's TCP link and the bridge, so they are **not in the frame log and
no list of commands filters them**. The harness reaches that radio only through `MeshCoreRadio`, which sends app start
(1), send text (2), contacts (4), advert (7), next message (10), battery (20) and device query (22).

## The frame log

Each run writes `frames-<unix ms>-<pid>.jsonl`: one line for every frame to or from a USB radio, in order.

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

The Bluetooth test is in `ble.rs`. `ble_tests.rs` runs it with the real bridge program and fake radios, and
`core/tests/issue_135_ble_bridge.rs` tests the bridge itself the same way. Both need a Mac.

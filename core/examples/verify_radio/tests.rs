//! The harness against fake radios on fake USB ports: no hardware, no serial port. The fakes
//! speak the serial framing and answer as companion firmware v1.18.0 does.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use meshcore_rs::events::EventDispatcher;
use meshcore_rs::reader::MessageReader;
use peoples_chat_core::radio::Port;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

use crate::config::{self, Config, HubPort};
use crate::lab::{Lab, Timing, Usb};
use crate::link::{FrameLog, Io, Link};
use crate::lock::LabLock;
use crate::power::{Power, Uhubctl};
use crate::{End, Mode, Paths, locked, verify};

const FIRMWARE: &str = "v1.18.0-81dbae5";

const LAB: &str = r#"
[lab]
preset = "private-test"
frequency_mhz = 869.850
bandwidth_khz = 62.5
spreading_factor = 7
coding_rate = 5
max_tx_power_dbm = 2

[[radio]]
label = "R1"
role = "usb-a"
usb_serial = "AAAA1111"
firmware = "v1.18.0-81dbae5"
hub_port = "1-1.4:2"

[[radio]]
label = "R2"
usb_serial = "BBBB2222"
firmware = "v1.18.0-81dbae5"
radio_read = { frequency_mhz = 869.85, tx_power_dbm = 2 }
hub_port = "<none: no per-port power switching>"

[[radio]]
label = "R3"
ble_name = "MeshCore-TEST"
firmware = "v1.18.0-81dbae5"
"#;

// The fake radios.

#[derive(Clone)]
struct Radio {
    serial: &'static str,
    path: &'static str,
    plugged_in: bool,
    answers: bool,
    firmware: &'static str,
    /// kHz.
    frequency: u32,
    tx_power: u8,
    /// Stops answering after this many commands.
    hangs_after: Option<usize>,
    clock: u32,
    /// The codes of the commands that reached it.
    commands: Vec<u8>,
}

impl Radio {
    fn new(serial: &'static str, path: &'static str) -> Self {
        Self {
            serial,
            path,
            plugged_in: true,
            answers: true,
            firmware: FIRMWARE,
            frequency: 869_850,
            tx_power: 2,
            hangs_after: None,
            clock: 1_715_824_653,
            commands: Vec::new(),
        }
    }
}

#[derive(Clone)]
struct Bench(Arc<Mutex<Vec<Radio>>>);

impl Bench {
    /// R1 and R2, on port names that say nothing about which is which.
    fn new() -> Self {
        Self(Arc::new(Mutex::new(vec![
            Radio::new("AAAA1111", "/dev/cu.usbmodem9901"),
            Radio::new("BBBB2222", "/dev/cu.usbmodem1101"),
        ])))
    }

    fn change(&self, radio: usize, change: impl FnOnce(&mut Radio)) {
        change(&mut self.0.lock().unwrap()[radio]);
    }

    fn radio(&self, radio: usize) -> Radio {
        self.0.lock().unwrap()[radio].clone()
    }
}

#[async_trait]
impl Usb for Bench {
    async fn list(&self) -> Result<Vec<Port>, String> {
        let radios = self.0.lock().unwrap();
        Ok(radios
            .iter()
            .filter(|radio| radio.plugged_in)
            .map(|radio| Port {
                path: radio.path.to_owned(),
                description: None,
                serial_number: Some(radio.serial.to_owned()),
            })
            .collect())
    }

    fn open(&self, path: &str) -> Result<Box<dyn Io>, String> {
        let radio = self
            .0
            .lock()
            .unwrap()
            .iter()
            .position(|radio| radio.path == path)
            .ok_or("no such port")?;
        let (app, device) = tokio::io::duplex(4096);
        tokio::spawn(serve(self.clone(), radio, device));
        Ok(Box::new(app))
    }
}

async fn serve(bench: Bench, radio: usize, mut port: DuplexStream) {
    loop {
        let mut header = [0; 3];
        if port.read_exact(&mut header).await.is_err() {
            return;
        }
        assert_eq!(header[0], b'<');
        let mut command = vec![0; usize::from(u16::from_le_bytes([header[1], header[2]]))];
        if port.read_exact(&mut command).await.is_err() {
            return;
        }
        let state = {
            let mut radios = bench.0.lock().unwrap();
            let state = &mut radios[radio];
            state.commands.push(command[0]);
            if state
                .hangs_after
                .is_some_and(|after| state.commands.len() > after)
            {
                state.answers = false;
                state.hangs_after = None;
            }
            state.clone()
        };
        if !state.answers {
            continue;
        }
        // meshcore-rs listens for the answer only after it has written the command.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let replies = match command[0] {
            1 => vec![self_info(&state)],
            22 => vec![device_info(state.firmware)],
            4 => vec![vec![2, 0, 0, 0, 0], vec![4, 0, 0, 0, 0]],
            // A clock that takes whatever it is set to.
            5 => vec![[&[9][..], &state.clock.to_le_bytes()].concat()],
            6 => {
                let time = u32::from_le_bytes(command[1..5].try_into().unwrap());
                bench.change(radio, |radio| radio.clock = time);
                vec![vec![0]]
            }
            _ => vec![vec![1, 1]],
        };
        for reply in replies {
            let mut framed = vec![b'>'];
            framed.extend_from_slice(&(reply.len() as u16).to_le_bytes());
            framed.extend_from_slice(&reply);
            if port.write_all(&framed).await.is_err() {
                return;
            }
        }
    }
}

fn self_info(radio: &Radio) -> Vec<u8> {
    let mut frame = vec![5, 1, radio.tx_power, 22];
    frame.extend_from_slice(&[0xad; 32]);
    frame.extend_from_slice(&55_000_000i32.to_le_bytes()); // lat
    frame.extend_from_slice(&12_000_000i32.to_le_bytes()); // lon
    frame.extend_from_slice(&[0; 4]);
    frame.extend_from_slice(&radio.frequency.to_le_bytes());
    frame.extend_from_slice(&62_500u32.to_le_bytes());
    frame.extend_from_slice(&[7, 5]);
    frame.extend_from_slice(b"Lab radio");
    frame
}

fn device_info(firmware: &str) -> Vec<u8> {
    let padded = |text: &str, length| {
        let mut bytes = text.as_bytes().to_vec();
        bytes.resize(length, 0);
        bytes
    };
    let mut frame = vec![13, 14, 175, 40];
    frame.extend_from_slice(&123_456u32.to_le_bytes()); // BLE PIN
    frame.extend(padded("26 Nov 2026", 12));
    frame.extend(padded("Seeed Wio Tracker L1", 40));
    frame.extend(padded(firmware, 20));
    frame.extend_from_slice(&[0, 0]);
    frame
}

/// Counts power cycles. When it `mends`, the cycled radio is back and answers.
struct Switch {
    bench: Bench,
    mends: bool,
    cycled: Mutex<Vec<HubPort>>,
}

impl Switch {
    fn new(bench: &Bench, mends: bool) -> Self {
        Self {
            bench: bench.clone(),
            mends,
            cycled: Mutex::new(Vec::new()),
        }
    }

    fn count(&self) -> usize {
        self.cycled.lock().unwrap().len()
    }
}

#[async_trait]
impl Power for Switch {
    async fn cycle(&self, hub: &HubPort) -> Result<(), String> {
        self.cycled.lock().unwrap().push(hub.clone());
        if self.mends {
            self.bench.change(0, |radio| {
                radio.plugged_in = true;
                radio.answers = true;
            });
        }
        Ok(())
    }
}

fn timing() -> Timing {
    Timing {
        reply: Duration::from_millis(500),
        back: Duration::from_secs(2),
        poll: Duration::from_millis(10),
        ble_connect: Duration::from_secs(3),
        watch: Duration::from_millis(100),
        lost: Duration::from_secs(5),
        message: Duration::from_secs(3),
        contacts: Duration::from_secs(2),
    }
}

/// One run against the bench: how it ended, what it printed and its frame log.
async fn run(
    config: &Config,
    bench: &Bench,
    switch: &Switch,
    dry_run: bool,
) -> (Result<(), String>, String, String) {
    let folder = TempDir::new().unwrap();
    let log = FrameLog::create(folder.path()).unwrap();
    let lab = Lab {
        config,
        usb: Arc::new(bench.clone()),
        power: switch,
        log: log.clone(),
        timing: timing(),
        bluetooth: false,
    };
    let mut out = Vec::new();
    let result = lab.run(dry_run, &mut out).await;
    let frames = std::fs::read_to_string(log.path()).unwrap();
    (result, String::from_utf8(out).unwrap(), frames)
}

fn lab() -> Config {
    config::parse(LAB).unwrap()
}

// The command and the lab file.

#[test]
fn issue_133_package_json_has_the_verify_radio_script() {
    let package: serde_json::Value =
        serde_json::from_str(include_str!("../../../package.json")).unwrap();
    let script = package["scripts"]["verify:radio"].as_str().unwrap();
    assert!(script.contains("--example verify_radio"), "{script}");
}

#[test]
fn issue_133_the_lab_file_is_in_the_factory_folder_unless_the_environment_says() {
    let home = |name: &str| (name == "HOME").then(|| "/home/lab".into());
    let paths = Paths::from_env(home).unwrap();
    assert_eq!(paths.config, Path::new("/home/lab/factory/radios.toml"));
    assert_eq!(paths.lock, Path::new("/home/lab/factory/locks/radio-lab"));

    let paths = Paths::from_env(|name| match name {
        "RADIOS_TOML" => Some("/elsewhere/lab.toml".into()),
        other => home(other),
    })
    .unwrap();
    assert_eq!(paths.config, Path::new("/elsewhere/lab.toml"));
}

#[test]
fn issue_133_a_missing_lab_file_fails_cleanly_and_takes_no_lock() {
    let folder = TempDir::new().unwrap();
    let paths = Paths {
        config: folder.path().join("radios.toml"),
        lock: folder.path().join("locks/radio-lab"),
        logs: folder.path().join("logs"),
        uhubctl: "uhubctl".into(),
    };
    let mut out = Vec::new();
    let End::Failed(error) = verify(&paths, Mode::default(), &mut out) else {
        panic!("a run without a lab file passed");
    };
    assert!(error.contains("No radio lab file at"), "{error}");
    assert!(error.contains("RADIOS_TOML"), "{error}");
    assert!(!paths.lock.exists());
    assert!(!paths.logs.exists());
}

#[test]
fn issue_133_a_lab_file_that_is_not_filled_in_is_refused() {
    let placeholder = LAB.replace("\"AAAA1111\"", "\"<serial>\"");
    let error = config::parse(&placeholder).unwrap_err();
    assert!(error.contains("R1: usb_serial isn't filled in"), "{error}");
    let option = LAB.replace("1-1.4:2", "-a:2");
    let error = config::parse(&option).unwrap_err();
    assert!(error.contains("hub_port"), "{error}");
    let twice = LAB.replace("BBBB2222", "aaaa1111");
    assert!(config::parse(&twice).is_err());
    assert!(config::parse("[lab]").is_err());
}

// Finding and checking the radios.

#[tokio::test]
async fn issue_133_a_dry_run_finds_radios_by_serial_number_and_lists_their_firmware() {
    let bench = Bench::new();
    let switch = Switch::new(&bench, true);
    let (result, out, _) = run(&lab(), &bench, &switch, true).await;
    result.unwrap();
    let r1 = out.lines().find(|line| line.starts_with("R1:")).unwrap();
    assert!(r1.contains("/dev/cu.usbmodem9901 (USB AAAA1111)"), "{r1}");
    assert!(r1.contains("firmware v1.18.0-81dbae5"), "{r1}");
    let r2 = out.lines().find(|line| line.starts_with("R2:")).unwrap();
    assert!(r2.contains("/dev/cu.usbmodem1101 (USB BBBB2222)"), "{r2}");
    assert!(
        out.contains("R3: not on USB (MeshCore-TEST), not checked"),
        "{out}"
    );
    assert!(out.contains("2 USB radios match"), "{out}");

    // Plugged in elsewhere, under each other's names.
    bench.change(0, |radio| radio.path = "/dev/cu.usbmodem1101");
    bench.change(1, |radio| radio.path = "/dev/cu.usbmodem9901");
    let (result, out, _) = run(&lab(), &bench, &switch, true).await;
    result.unwrap();
    assert!(
        out.contains("R1: /dev/cu.usbmodem1101 (USB AAAA1111)"),
        "{out}"
    );
}

#[tokio::test]
async fn issue_133_wrong_firmware_or_preset_fails_before_anything_runs() {
    type Change = fn(&mut Radio);
    let cases: [(Change, &str); 3] = [
        (
            |radio| radio.firmware = "v1.17.0",
            "firmware is v1.17.0, radios.toml says v1.18.0-81dbae5",
        ),
        (
            |radio| radio.frequency = 869_618,
            "frequency is 869.618 MHz, the lab preset is 869.85 MHz",
        ),
        (
            |radio| radio.tx_power = 22,
            "transmit power is 22 dBm, the lab allows at most 2 dBm",
        ),
    ];
    for (change, expected) in cases {
        let bench = Bench::new();
        bench.change(1, change);
        let switch = Switch::new(&bench, true);
        let (result, out, _) = run(&lab(), &bench, &switch, false).await;
        let error = result.unwrap_err();
        assert!(error.contains("nothing was run"), "{error}");
        assert!(error.contains(&format!("R2: {expected}")), "{error}");
        assert!(!out.contains("responds"), "{out}");
        // Power cycling wouldn't mend it.
        assert_eq!(switch.count(), 0);
    }
}

#[tokio::test]
async fn issue_133_a_public_key_other_than_the_lab_files_is_refused() {
    let config = config::parse(&LAB.replace("role = \"usb-a\"", "public_key = \"beef\"")).unwrap();
    let bench = Bench::new();
    let (result, _, _) = run(&config, &bench, &Switch::new(&bench, true), true).await;
    assert!(result.unwrap_err().contains("R1: public key is adadad"));
}

// The Bluetooth radio without a bridge (#135). The runs with one are in `ble_tests.rs`.

#[test]
fn issue_135_the_bridges_ports_come_from_the_lab_file() {
    let r3 = lab().radios.remove(2);
    assert_eq!((r3.bridge_port(), r3.bridge_control_port()), (5535, 5536));
    let named = LAB.replace("MeshCore-TEST\"", "MeshCore-TEST\"\nbridge_port = 6000");
    let r3 = config::parse(&named).unwrap().radios.remove(2);
    assert_eq!((r3.bridge_port(), r3.bridge_control_port()), (6000, 6001));
    let both = named.replace("6000", "6000\nbridge_control_port = 7000");
    let r3 = config::parse(&both).unwrap().radios.remove(2);
    assert_eq!((r3.bridge_port(), r3.bridge_control_port()), (6000, 7000));

    let unnamed = LAB.replace("\"MeshCore-TEST\"", "\"<name>\"");
    let error = config::parse(&unnamed).unwrap_err();
    assert!(error.contains("R3: ble_name isn't filled in"), "{error}");
}

#[tokio::test]
async fn issue_135_the_bluetooth_radio_is_skipped_when_the_bridge_is_not_running() {
    // A port nothing listens on.
    let port = std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = config::parse(&LAB.replace(
        "MeshCore-TEST\"",
        &format!("MeshCore-TEST\"\nbridge_control_port = {port}"),
    ))
    .unwrap();
    let bench = Bench::new();
    let folder = TempDir::new().unwrap();
    let switch = Switch::new(&bench, true);
    for dry_run in [true, false] {
        let lab = Lab {
            config: &config,
            usb: Arc::new(bench.clone()),
            power: &switch,
            log: FrameLog::create(folder.path()).unwrap(),
            timing: timing(),
            bluetooth: true,
        };
        let mut out = Vec::new();
        // The run passes: the radio is left out, and says so.
        lab.run(dry_run, &mut out).await.unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.contains(&format!(
                "R3: SKIPPED, not checked: the Bluetooth bridge isn't running (nothing listens \
                 on 127.0.0.1:{port})"
            )),
            "{out}"
        );
        assert!(out.contains("tools/ble-bridge/install.sh"), "{out}");
    }
}

// The lock.

fn lock_in(folder: &TempDir) -> std::path::PathBuf {
    folder.path().join("locks/radio-lab")
}

#[test]
fn issue_133_a_stale_lock_fails_cleanly_and_is_left_alone() {
    let folder = TempDir::new().unwrap();
    let path = lock_in(&folder);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // A process that has ended.
    let mut child = Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    let stale = format!("pid={pid}\n");
    std::fs::write(&path, &stale).unwrap();

    let end = locked(&path, || async { panic!("ran while the lab was locked") });
    let End::Failed(error) = end else {
        panic!("a stale lock didn't fail the run: {end:?}");
    };
    assert!(error.contains("Stale lab lock"), "{error}");
    assert!(error.contains(&pid.to_string()), "{error}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), stale);

    std::fs::write(&path, "").unwrap();
    let error = LabLock::acquire(&path).unwrap_err();
    assert!(error.contains("names no process"), "{error}");
}

#[test]
fn issue_133_the_lock_is_held_for_the_whole_run_and_released_after_failure() {
    let folder = TempDir::new().unwrap();
    let path = lock_in(&folder);
    let end = locked(&path, || async {
        assert!(path.exists());
        // A second run meanwhile is turned away.
        let error = LabLock::acquire(&path).unwrap_err();
        assert!(error.contains("in use"), "{error}");
        Err("the test failed".to_owned())
    });
    assert_eq!(end, End::Failed("the test failed".to_owned()));
    assert!(!path.exists());

    assert_eq!(locked(&path, || async { Ok(()) }), End::Passed);
    assert!(!path.exists());

    let panicked = std::panic::catch_unwind(|| {
        locked(&path, || async { panic!("a test panicked") });
    });
    assert!(panicked.is_err());
    assert!(!path.exists());
}

/// The run that [`issue_133_an_interrupt_releases_the_lock`] starts as its own process.
#[test]
#[ignore = "a helper: runs only as a child of the interrupt test"]
fn holds_the_lock_until_interrupted() {
    let Some(path) = std::env::var_os("VERIFY_RADIO_TEST_LOCK") else {
        return;
    };
    let end = locked(Path::new(&path), || async {
        std::future::pending::<Result<(), String>>().await
    });
    assert_eq!(end, End::Interrupted);
}

#[test]
fn issue_133_an_interrupt_releases_the_lock() {
    for signal in ["-INT", "-TERM", "-HUP"] {
        let folder = TempDir::new().unwrap();
        let path = lock_in(&folder);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "tests::holds_the_lock_until_interrupted",
            ])
            .env("VERIFY_RADIO_TEST_LOCK", &path)
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !path.exists() {
            assert!(Instant::now() < deadline, "the child never took the lock");
            std::thread::sleep(Duration::from_millis(20));
        }
        let sent = Command::new("kill")
            .args([signal, &child.id().to_string()])
            .status()
            .unwrap();
        assert!(sent.success());
        let status = child.wait().unwrap();
        // It ended by its own choice, having seen the signal, not because the signal killed it.
        assert!(status.success(), "{signal}: {status}");
        assert!(!path.exists(), "{signal} left the lock behind");
    }
}

// Power cycling.

#[tokio::test]
async fn issue_133_a_silent_or_vanished_radio_is_power_cycled_and_waited_for() {
    let changes: [fn(&mut Radio); 2] = [
        |radio| radio.answers = false,
        |radio| radio.plugged_in = false,
    ];
    for change in changes {
        let bench = Bench::new();
        bench.change(0, change);
        let switch = Switch::new(&bench, true);
        let (result, out, _) = run(&lab(), &bench, &switch, false).await;
        result.unwrap();
        assert_eq!(
            *switch.cycled.lock().unwrap(),
            [HubPort {
                location: "1-1.4".to_owned(),
                port: 2
            }]
        );
        assert!(out.contains("R1: cycling its power"), "{out}");
        assert!(out.contains("R1: back on"), "{out}");
        assert!(out.contains("ok R1: responds"), "{out}");
    }
}

#[tokio::test]
async fn issue_133_a_radio_that_stops_responding_mid_run_is_power_cycled() {
    let bench = Bench::new();
    // Answers the greeting and the first description, then hangs.
    let bench_probe = Bench::new();
    let (result, _, _) = run(&lab(), &bench_probe, &Switch::new(&bench_probe, true), true).await;
    result.unwrap();
    let to_connect = bench_probe.radio(0).commands.len();
    bench.change(0, |radio| radio.hangs_after = Some(to_connect));

    let switch = Switch::new(&bench, true);
    let (result, out, _) = run(&lab(), &bench, &switch, false).await;
    result.unwrap();
    assert!(out.contains("R1: stopped responding"), "{out}");
    assert!(out.contains("R1: back on"), "{out}");
    assert_eq!(switch.count(), 1);
}

#[tokio::test]
async fn issue_133_a_radio_that_does_not_come_back_fails_the_run() {
    let bench = Bench::new();
    bench.change(0, |radio| radio.answers = false);
    let switch = Switch::new(&bench, false);
    let (result, _, _) = run(&lab(), &bench, &switch, false).await;
    let error = result.unwrap_err();
    assert!(error.contains("R1 didn't come back within 2 s"), "{error}");
    assert_eq!(switch.count(), 1);
}

#[tokio::test]
async fn issue_133_without_a_hub_port_the_run_says_to_restart_the_radio_by_hand() {
    let bench = Bench::new();
    bench.change(1, |radio| radio.answers = false);
    let switch = Switch::new(&bench, true);
    let (result, _, _) = run(&lab(), &bench, &switch, false).await;
    let error = result.unwrap_err();
    assert!(error.contains("R2 stopped responding"), "{error}");
    assert!(error.contains("no hub_port"), "{error}");
    assert_eq!(switch.count(), 0);
}

#[tokio::test]
async fn issue_133_a_dry_run_never_cycles_power() {
    let bench = Bench::new();
    bench.change(0, |radio| radio.answers = false);
    let switch = Switch::new(&bench, true);
    let (result, out, _) = run(&lab(), &bench, &switch, true).await;
    assert!(result.unwrap_err().contains("R1: the radio on"));
    assert!(out.contains("R2: /dev/cu.usbmodem1101"), "{out}");
    assert_eq!(switch.count(), 0);
}

#[tokio::test]
async fn issue_133_uhubctl_cycles_the_hub_port() {
    let folder = TempDir::new().unwrap();
    let program = folder.path().join("uhubctl");
    let asked = folder.path().join("asked");
    std::fs::write(
        &program,
        format!("#!/bin/sh\necho \"$@\" > '{}'\n", asked.display()),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let hub = HubPort {
        location: "1-1.4".to_owned(),
        port: 2,
    };
    Uhubctl { program }.cycle(&hub).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(asked).unwrap(),
        "-l 1-1.4 -p 2 -a cycle -d 3\n"
    );

    let error = Uhubctl {
        program: "false".into(),
    }
    .cycle(&hub)
    .await
    .unwrap_err();
    assert!(error.contains("uhubctl failed"), "{error}");
}

// The frame log, and what the harness never does.

#[tokio::test]
async fn issue_133_every_frame_is_logged_and_replays_as_a_fixture() {
    let bench = Bench::new();
    let (result, _, frames) = run(&lab(), &bench, &Switch::new(&bench, true), false).await;
    result.unwrap();
    let frames: Vec<serde_json::Value> = frames
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let of = |radio: &str, direction: &str| -> Vec<Vec<u8>> {
        frames
            .iter()
            .filter(|frame| frame["radio"] == radio && frame["dir"] == direction)
            .map(|frame| hex::decode(frame["frame"].as_str().unwrap()).unwrap())
            .collect()
    };
    for (radio, state) in [("R1", bench.radio(0)), ("R2", bench.radio(1))] {
        // Everything that reached the radio is in the log, in order.
        let sent: Vec<u8> = of(radio, "to_radio").iter().map(|frame| frame[0]).collect();
        assert_eq!(sent, state.commands);
        // So is everything it answered, and meshcore-rs reads it back.
        let received = of(radio, "from_radio");
        for code in [5, 13, 2, 4] {
            assert!(received.iter().any(|frame| frame[0] == code), "{code}");
        }
        let reader = MessageReader::new(Arc::new(EventDispatcher::new()));
        for frame in received {
            reader.handle_rx(frame).await.unwrap();
        }
    }
    assert!(frames.iter().all(|frame| frame["at_ms"].as_u64().is_some()));
    // Neither the Bluetooth PIN nor the position is kept.
    assert_eq!(
        of("R1", "from_radio").iter().find(|f| f[0] == 13).unwrap()[4..8],
        [0; 4]
    );
    assert_eq!(
        of("R1", "from_radio").iter().find(|f| f[0] == 5).unwrap()[36..44],
        [0; 8]
    );
}

#[tokio::test]
async fn issue_133_commands_that_change_firmware_or_settings_never_reach_a_radio() {
    let bench = Bench::new();
    let folder = TempDir::new().unwrap();
    let log = FrameLog::create(folder.path()).unwrap();
    let stream = bench.open("/dev/cu.usbmodem9901").unwrap();
    let link = Link::open("R1", stream, log.clone(), Duration::from_millis(100));
    // Set radio parameters, set transmit power, reboot, export and import the private key,
    // factory reset.
    let forbidden = [11, 12, 19, 23, 24, 51];
    for command in forbidden {
        let sent = link
            .commands
            .lock()
            .await
            .send_multi(
                &[command, 0, 0],
                &[meshcore_rs::EventType::Ok],
                Duration::from_millis(50),
            )
            .await;
        assert!(sent.is_err());
    }
    // An allowed one after them gets through.
    link.commands.lock().await.send_appstart().await.unwrap();
    assert_eq!(bench.radio(0).commands, [1]);
    let frames = std::fs::read_to_string(log.path()).unwrap();
    assert_eq!(frames.matches("\"blocked\"").count(), forbidden.len());
    link.close().await;
}

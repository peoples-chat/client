//! The Bluetooth run (#135) against fake radios: R1 on a fake USB port, and R3 behind the real
//! bridge program, which has a fake companion radio over TCP in place of the Bluetooth one. The
//! same against the lab's radios is `pnpm verify:radio`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use peoples_chat_core::radio::{MeshCoreRadio, Port, Radio};
use peoples_chat_fake_radio::{self as fake, FakeRadio};
use tempfile::TempDir;
use tokio::net::TcpStream;

use crate::checks_and_tests;
use crate::config::{self, Config};
use crate::lab::{Lab, Timing, Usb};
use crate::link::{FrameLog, Io};
use crate::messages::Waits;
use crate::power::Power;
use crate::support;

#[path = "../../tests/support/bridge.rs"]
mod bridge;

use self::bridge::Bridge;

/// A lab on the preset and firmware the fake radios report.
fn config(bridge: &Bridge) -> Config {
    config::parse(&format!(
        r#"
[lab]
preset = "fake"
frequency_mhz = 869.618
bandwidth_khz = 62.5
spreading_factor = 8
coding_rate = 8
max_tx_power_dbm = 22

[[radio]]
label = "R1"
usb_serial = "AAAA1111"
firmware = "v1.18.0"

[[radio]]
label = "R3"
ble_name = "MeshCore-FAKE"
firmware = "v1.18.0"
bridge_port = {}
bridge_control_port = {}
"#,
        bridge.port, bridge.control_port
    ))
    .unwrap()
}

fn timing() -> Timing {
    Timing {
        reply: std::time::Duration::from_secs(1),
        poll: std::time::Duration::from_millis(50),
        ble_connect: std::time::Duration::from_secs(10),
        watch: std::time::Duration::from_millis(200),
        lost: std::time::Duration::from_secs(5),
        message: std::time::Duration::from_secs(5),
        contacts: std::time::Duration::from_secs(5),
        ..Timing::default()
    }
}

/// The computer's USB ports: TCP links to fakes, by serial number.
struct Cables(Vec<(&'static str, SocketAddr)>);

#[async_trait]
impl Usb for Cables {
    async fn list(&self) -> Result<Vec<Port>, String> {
        Ok(self
            .0
            .iter()
            .map(|(serial, _)| Port {
                path: format!("/dev/cu.fake-{serial}"),
                description: None,
                serial_number: Some((*serial).to_owned()),
            })
            .collect())
    }

    fn open(&self, path: &str) -> Result<Box<dyn Io>, String> {
        let (_, addr) = self
            .0
            .iter()
            .find(|(serial, _)| path.ends_with(serial))
            .ok_or("no such port")?;
        let stream = std::net::TcpStream::connect(addr).map_err(|error| error.to_string())?;
        stream
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let stream = TcpStream::from_std(stream).map_err(|error| error.to_string())?;
        Ok(Box::new(stream))
    }
}

/// Never asked: the fakes have no hub.
struct NoPower;

#[async_trait]
impl Power for NoPower {
    async fn cycle(&self, _hub: &config::HubPort) -> Result<(), String> {
        Err("no hub".to_owned())
    }
}

/// Two radios in range that aren't contacts yet, R3 behind the bridge.
struct Bench {
    r1: FakeRadio,
    r3: FakeRadio,
    bridge: Bridge,
}

impl Bench {
    async fn new(r3: fake::Config) -> Self {
        let r1 = FakeRadio::start(fake::Config::default()).await.unwrap();
        let r3 = FakeRadio::start(r3).await.unwrap();
        fake::put_in_range(&r1, &r3);
        let bridge = Bridge::start(r3.addr());
        bridge.until("connected").await;
        Self { r1, r3, bridge }
    }

    /// One run: how it ended and what it printed.
    async fn run(&self, config: &Config, dry_run: bool) -> (Result<(), String>, String) {
        let folder = TempDir::new().unwrap();
        let lab = Lab {
            config,
            usb: Arc::new(Cables(vec![("AAAA1111", self.r1.addr())])),
            power: &NoPower,
            log: FrameLog::create(folder.path()).unwrap(),
            timing: timing(),
            bluetooth: true,
        };
        let mut out = Vec::new();
        let result = lab.run(dry_run, &mut out).await;
        (result, String::from_utf8(out).unwrap())
    }

    /// How many contacts R1 holds: none until someone announced itself to it.
    async fn r1_contacts(&self) -> usize {
        let radio = MeshCoreRadio::connect_tcp("127.0.0.1", self.r1.addr().port())
            .await
            .unwrap();
        radio.contacts().await.unwrap().len()
    }
}

/// Where in `out` the line that starts with `start` is.
fn line(out: &str, start: &str) -> usize {
    out.lines()
        .position(|line| line.starts_with(start))
        .unwrap_or_else(|| panic!("no line {start:?} in\n{out}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_135_a_dry_run_checks_the_radio_through_the_bridge_and_sends_nothing() {
    let bench = Bench::new(fake::Config::default()).await;
    let (result, out) = bench.run(&config(&bench.bridge), true).await;
    result.unwrap_or_else(|error| panic!("{error}\n{out}"));
    let key = hex::encode(bench.r3.public_key());
    assert!(
        out.contains(&format!(
            "R3: MeshCore-FAKE over Bluetooth, firmware v1.18.0, key {}…, 869.618 MHz",
            &key[..12]
        )),
        "{out}"
    );
    line(&out, "ok R3: connected through the bridge on 127.0.0.1:");
    assert!(
        !out.contains("arrived") && !out.contains("announce"),
        "{out}"
    );
    assert_eq!(bench.r1_contacts().await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_135_a_radio_off_the_lab_preset_is_refused_before_anything_is_transmitted() {
    let bench = Bench::new(fake::Config {
        frequency_khz: 869_525,
        ..fake::Config::default()
    })
    .await;
    let (result, out) = bench.run(&config(&bench.bridge), false).await;
    let error = result.unwrap_err();
    assert!(
        error.contains("R3: frequency is 869.525 MHz, the lab preset is 869.618 MHz"),
        "{error}"
    );
    assert!(
        !out.contains("arrived") && !out.contains("announce"),
        "{out}"
    );
    // Neither radio announced itself, so neither came to hold the other.
    assert_eq!(bench.r1_contacts().await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_135_messages_pass_both_ways_and_a_dropped_link_comes_back() {
    let bench = Bench::new(fake::Config::default()).await;
    let (result, out) = bench.run(&config(&bench.bridge), false).await;
    result.unwrap_or_else(|error| panic!("{error}\n{out}"));
    let connected = line(&out, "ok R3: connected through the bridge");
    let contacts = line(&out, "R1 and R3 don't hold each other as a contact");
    let to_ble = line(&out, "ok R1 (USB) to R3 (Bluetooth): \"verify:radio ");
    let to_usb = line(&out, "ok R3 (Bluetooth) to R1 (USB): \"verify:radio ");
    let dropped = line(
        &out,
        "ok R3: the bridge dropped the Bluetooth link, the core noticed after",
    );
    assert!(
        connected < contacts && contacts < to_ble && to_ble < to_usb && to_usb < dropped,
        "{out}"
    );
    let dropped = out.lines().nth(dropped).unwrap();
    assert!(
        dropped.contains(&format!("in the same process (pid {})", std::process::id())),
        "{dropped}"
    );
    assert!(dropped.contains("arrived over the new link"), "{dropped}");
}

/// The whole lab at once, as on the Mac: R1 and R2 on USB (#134), R3 behind the bridge.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_135_one_run_covers_the_bluetooth_radio_and_the_two_usb_radios() {
    let _turn = support::take_turn().await;
    let bench = Bench::new(fake::Config::default()).await;
    let r2 = FakeRadio::start(fake::Config::default()).await.unwrap();
    fake::link(&bench.r1, &r2).unwrap();
    // R1's clock is behind, as the lab's were.
    bench.r1.set_clock(1_715_824_653);
    let mut config = config(&bench.bridge);
    let mut second = config.radios[0].clone();
    second.label = "R2".to_owned();
    second.usb_serial = Some("BBBB2222".to_owned());
    config.radios.insert(1, second);
    let folder = TempDir::new().unwrap();
    let lab = Lab {
        config: &config,
        usb: Arc::new(Cables(vec![
            ("AAAA1111", bench.r1.addr()),
            ("BBBB2222", r2.addr()),
        ])),
        power: &NoPower,
        log: FrameLog::create(folder.path()).unwrap(),
        timing: Timing {
            reply: Timing::default().reply,
            ..timing()
        },
        bluetooth: true,
    };
    let waits = Waits {
        delivery: Duration::from_secs(30),
        advert: Duration::from_secs(5),
        app: Duration::from_secs(60),
        closed: Duration::from_secs(2),
        reconnect: Duration::from_secs(30),
    };
    let mut out = Vec::new();
    let result = checks_and_tests(&lab, waits, false, &mut out).await;
    let out = String::from_utf8(out).unwrap();
    result.unwrap_or_else(|error| panic!("{error}\n{out}"));
    let mut rest = out.as_str();
    for expected in [
        "ok R1: clock set",
        "ok R2: clock ",
        "ok R1 (USB) to R3 (Bluetooth)",
        "ok R3 (Bluetooth) to R1 (USB)",
        "ok R3: the bridge dropped the Bluetooth link",
        "Over the air: R1 and R2",
        "ok R1: heard R2's advert",
        "ok \"Lab message 2\" from R1 to R2",
    ] {
        let at = rest
            .find(expected)
            .unwrap_or_else(|| panic!("missing {expected:?} in:\n{out}"));
        rest = &rest[at + expected.len()..];
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_135_a_running_bridge_without_its_radio_fails_the_run() {
    let bench = Bench::new(fake::Config::default()).await;
    // A bridge that looks for a radio that isn't there.
    let gone = FakeRadio::start(fake::Config::default()).await.unwrap();
    let nowhere = gone.addr();
    drop(gone);
    let bridge = Bridge::start(nowhere);
    let config = config(&bridge);
    let (result, out) = bench.run(&config, false).await;
    let error = result.unwrap_err();
    assert!(
        error.contains("R3: the bridge has no Bluetooth link to MeshCore-FAKE"),
        "{error}"
    );
    assert!(!out.contains("SKIPPED"), "{out}");
}

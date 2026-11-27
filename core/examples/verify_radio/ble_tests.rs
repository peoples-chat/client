//! The Bluetooth run (#135) against fake radios: R1 on a fake USB port and R3 where the side
//! that holds the Bluetooth link looks for it. Both are fake companion radios over TCP that hear
//! each other. R3's link can go quiet without closing, as a Bluetooth link does.
//!
//! The run itself, the socket between its two sides and the side that holds the link are the
//! real ones. Only launchd, macOS's permission and the radio link under meshcore-rs are stood
//! in for. The same against the lab's radios is `pnpm verify:radio`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use meshcore_rs::MeshCore;
use peoples_chat_core::radio::{MeshCoreRadio, Port, RadioError};
use peoples_chat_fake_radio::{self as fake, FakeRadio};
use tempfile::TempDir;
use tokio::net::TcpStream;

use crate::ble::{Bluetooth, Launcher, Opened, Permission, serve};
use crate::config::{self, Config};
use crate::lab::{Lab, Timing, Usb};
use crate::link::{FrameLog, Io};
use crate::power::Power;

const BLE_NAME: &str = "MeshCore-FAKE";

/// A lab on the preset and firmware the fake radios report.
const LAB: &str = r#"
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
reboot_to_drop_link = true
"#;

fn timing() -> Timing {
    Timing {
        reply: Duration::from_millis(500),
        back: Duration::from_secs(2),
        poll: Duration::from_millis(20),
        ble_start: Duration::from_secs(2),
        ble_connect: Duration::from_secs(3),
        ble_lost: Duration::from_secs(5),
        message: Duration::from_secs(3),
        contacts: Duration::from_secs(2),
    }
}

/// R1's USB port: a TCP link to its fake.
struct Cable(SocketAddr);

#[async_trait]
impl Usb for Cable {
    async fn list(&self) -> Result<Vec<Port>, String> {
        Ok(vec![Port {
            path: "/dev/cu.usbmodem1101".to_owned(),
            description: None,
            serial_number: Some("AAAA1111".to_owned()),
        }])
    }

    fn open(&self, _path: &str) -> Result<Box<dyn Io>, String> {
        let stream = std::net::TcpStream::connect(self.0).map_err(|error| error.to_string())?;
        stream
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let stream = TcpStream::from_std(stream).map_err(|error| error.to_string())?;
        Ok(Box::new(stream))
    }
}

/// Bluetooth with one radio in range: R3's fake, under its advertised name.
struct Air {
    permission: Permission,
    radio: SocketAddr,
    /// How often a link to the radio was opened.
    opened: AtomicUsize,
}

#[async_trait]
impl Bluetooth for Air {
    fn permission(&self) -> Permission {
        self.permission
    }

    async fn open(&self, name: &str, _within: Duration) -> Result<Opened, RadioError> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        if name != BLE_NAME {
            return Err(RadioError::Link(format!("no Bluetooth link to {name}")));
        }
        let failed = |error: meshcore_rs::Error| RadioError::Link(error.to_string());
        let link = MeshCore::tcp(&self.radio.ip().to_string(), self.radio.port())
            .await
            .map_err(failed)?;
        link.set_default_timeout(Duration::from_millis(300)).await;
        let radio = MeshCoreRadio::over(link.commands().clone(), link.dispatcher()).await?;
        // As `MeshCoreRadio::connect_ble` does, only faster.
        radio.watch_link(Duration::from_millis(100));
        Ok(Opened {
            radio,
            _link: Some(Box::new(link)),
        })
    }
}

/// Runs the side that holds the link as a task of the test, in place of a launchd job.
struct InProcess(Arc<Air>);

struct Stop(tokio::task::JoinHandle<()>);

impl Drop for Stop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[async_trait]
impl Launcher for InProcess {
    async fn launch(
        &self,
        control: SocketAddr,
        name: &str,
        may_ask: bool,
    ) -> Result<Box<dyn Send>, String> {
        let air = self.0.clone();
        let name = name.to_owned();
        Ok(Box::new(Stop(tokio::spawn(async move {
            let stream = TcpStream::connect(control).await.unwrap();
            serve(stream, &name, may_ask, air.as_ref(), timing()).await;
        }))))
    }

    fn hint(&self) -> String {
        "It is a task of the test.".to_owned()
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

struct Bench {
    r1: FakeRadio,
    r3: FakeRadio,
    air: Arc<Air>,
}

impl Bench {
    /// R1 and R3 in range of each other. `contacts`: whether they hold each other already.
    async fn new(contacts: bool, permission: Permission, r3: fake::Config) -> Self {
        let r1 = FakeRadio::start(fake::Config::default()).await.unwrap();
        let r3 = FakeRadio::start(r3).await.unwrap();
        if contacts {
            fake::link(&r1, &r3).unwrap();
        } else {
            fake::put_in_range(&r1, &r3);
        }
        let air = Arc::new(Air {
            permission,
            radio: r3.addr(),
            opened: AtomicUsize::new(0),
        });
        Self { r1, r3, air }
    }

    async fn ready() -> Self {
        Self::new(true, Permission::Allowed, fake::Config::default()).await
    }

    fn opened(&self) -> usize {
        self.air.opened.load(Ordering::SeqCst)
    }

    /// One run: how it ended and what it printed.
    async fn run(
        &self,
        config: &Config,
        dry_run: bool,
        may_ask: bool,
    ) -> (Result<(), String>, String) {
        let folder = TempDir::new().unwrap();
        let launcher = InProcess(self.air.clone());
        let lab = Lab {
            config,
            usb: &Cable(self.r1.addr()),
            power: &NoPower,
            log: FrameLog::create(folder.path()).unwrap(),
            timing: timing(),
            ble: Some(&launcher),
            may_ask,
        };
        let mut out = Vec::new();
        let result = lab.run(dry_run, &mut out).await;
        (result, String::from_utf8(out).unwrap())
    }
}

fn lab() -> Config {
    config::parse(LAB).unwrap()
}

/// The lines of `out` that report a passed step.
fn passed(out: &str) -> Vec<&str> {
    out.lines().filter(|line| line.starts_with("ok ")).collect()
}

// Criterion 1: found by its advertised name, and connected.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_135_a_dry_run_finds_the_bluetooth_radio_by_name_and_sends_nothing() {
    let bench = Bench::ready().await;
    let (result, out) = bench.run(&lab(), true, false).await;
    result.unwrap();
    let key = hex::encode(bench.r3.public_key());
    assert!(
        out.contains(&format!(
            "R3: MeshCore-FAKE over Bluetooth, firmware v1.18.0, key {}…, 869.618 MHz",
            &key[..12]
        )),
        "{out}"
    );
    assert!(
        out.contains("ok R3: found by its advertised name and connected"),
        "{out}"
    );
    assert_eq!(bench.opened(), 1);
    // Nothing went over the air, and nothing was restarted.
    assert!(
        !out.contains("arrived") && !out.contains("dropped"),
        "{out}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_135_a_radio_that_advertises_another_name_is_not_taken_for_it() {
    let bench = Bench::ready().await;
    let other = config::parse(&LAB.replace("MeshCore-FAKE", "MeshCore-OTHER")).unwrap();
    let (result, out) = bench.run(&other, false, false).await;
    let error = result.unwrap_err();
    assert!(
        error.contains("R3: no Bluetooth link to MeshCore-OTHER"),
        "{error}"
    );
    assert!(!out.contains("arrived"), "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_135_a_bluetooth_radio_off_the_lab_preset_is_refused_before_anything_is_sent() {
    let off_preset = fake::Config {
        frequency_khz: 869_525,
        ..fake::Config::default()
    };
    let bench = Bench::new(true, Permission::Allowed, off_preset).await;
    let (result, out) = bench.run(&lab(), false, false).await;
    let error = result.unwrap_err();
    assert!(
        error.contains("R3: frequency is 869.525 MHz, the lab preset is 869.618 MHz"),
        "{error}"
    );
    assert!(
        passed(&out).iter().all(|line| line.starts_with("ok R1")),
        "{out}"
    );

    // The same for the firmware and the key the lab file expects.
    let bench = Bench::ready().await;
    let other = config::parse(&LAB.replace(
        "ble_name = \"MeshCore-FAKE\"\nfirmware = \"v1.18.0\"",
        "ble_name = \"MeshCore-FAKE\"\nfirmware = \"v1.17.0\"\npublic_key = \"beef\"",
    ))
    .unwrap();
    let error = bench.run(&other, false, false).await.0.unwrap_err();
    assert!(
        error.contains("R3: firmware is v1.18.0, radios.toml says v1.17.0; public key is"),
        "{error}"
    );
}

// Criteria 2 and 3: a message each way, then the link drops and comes back.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_135_messages_pass_both_ways_and_a_dropped_link_comes_back_in_the_same_process() {
    let bench = Bench::ready().await;
    let (result, out) = bench.run(&lab(), false, false).await;
    result.unwrap_or_else(|error| panic!("{error}\n{out}"));
    let passed = passed(&out);
    let step = |start: &str| {
        passed
            .iter()
            .position(|line| line.starts_with(start))
            .unwrap_or_else(|| panic!("no step {start:?} in\n{out}"))
    };
    let found = step("ok R3: found by its advertised name and connected");
    let to_ble = step("ok R1 (USB) to R3 (Bluetooth): \"verify:radio ");
    let to_usb = step("ok R3 (Bluetooth) to R1 (USB): \"verify:radio ");
    let dropped =
        step("ok R3: its Bluetooth link was dropped (the radio restarted), noticed after");
    assert!(
        found < to_ble && to_ble < to_usb && to_usb < dropped,
        "{out}"
    );
    let dropped = passed[dropped];
    assert!(
        dropped.contains(&format!("by the same process (pid {})", std::process::id())),
        "{dropped}"
    );
    assert!(dropped.contains("arrived over the new link"), "{dropped}");
    // Opened once at the start, and once more after the drop.
    assert_eq!(bench.opened(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_135_the_radio_is_restarted_only_when_the_lab_file_allows_it() {
    let bench = Bench::ready().await;
    let config = config::parse(&LAB.replace("reboot_to_drop_link = true", "")).unwrap();
    let (result, out) = bench.run(&config, false, false).await;
    let error = result.unwrap_err();
    assert!(
        error.contains("R3: the dropped link wasn't tested"),
        "{error}"
    );
    assert!(error.contains("reboot_to_drop_link = true"), "{error}");
    // The messages had passed by then, and the link was never dropped.
    assert_eq!(
        passed(&out)
            .iter()
            .filter(|line| line.contains("arrived"))
            .count(),
        2,
        "{out}"
    );
    assert_eq!(bench.opened(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_135_radios_that_are_not_contacts_announce_themselves_only_when_allowed() {
    let strangers = || Bench::new(false, Permission::Allowed, fake::Config::default());
    let bench = strangers().await;
    let (result, out) = bench.run(&lab(), false, false).await;
    let error = result.unwrap_err();
    assert!(
        error.contains("R1 and R3 don't hold each other as a contact"),
        "{error}"
    );
    assert!(error.contains("exchange_adverts = true"), "{error}");
    assert!(!out.contains("arrived"), "{out}");

    let bench = strangers().await;
    let allowed = LAB.replace("preset = \"fake\"", "exchange_adverts = true");
    let (result, out) = bench
        .run(&config::parse(&allowed).unwrap(), false, false)
        .await;
    result.unwrap_or_else(|error| panic!("{error}\n{out}"));
    assert!(out.contains("both announce themselves"), "{out}");
    assert!(out.contains("ok R3 (Bluetooth) to R1 (USB)"), "{out}");
}

// Criterion 4: headless, with no macOS permission prompt.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_135_a_run_never_touches_bluetooth_before_macos_allows_it() {
    for permission in [
        Permission::NotAsked,
        Permission::Denied,
        Permission::Restricted,
    ] {
        let bench = Bench::new(true, permission, fake::Config::default()).await;
        let (result, out) = bench.run(&lab(), false, false).await;
        let error = result.unwrap_err();
        assert!(
            error.contains(&format!(
                "R3: macOS hasn't allowed the harness to use Bluetooth (it says: {})",
                permission.name()
            )),
            "{error}"
        );
        assert!(
            error.contains("pnpm verify:radio --allow-bluetooth"),
            "{error}"
        );
        // Using Bluetooth is what makes macOS ask. It wasn't used.
        assert_eq!(bench.opened(), 0, "{out}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_135_only_a_person_who_asks_for_it_gets_the_prompt() {
    // `--allow-bluetooth`: a dry run that may ask.
    let bench = Bench::new(true, Permission::NotAsked, fake::Config::default()).await;
    let (result, out) = bench.run(&lab(), true, true).await;
    result.unwrap_or_else(|error| panic!("{error}\n{out}"));
    assert!(out.contains("Click Allow in the dialog"), "{out}");
    assert_eq!(bench.opened(), 1);

    // It can't ask past a refusal.
    let bench = Bench::new(true, Permission::Denied, fake::Config::default()).await;
    assert!(bench.run(&lab(), true, true).await.0.is_err());
    assert_eq!(bench.opened(), 0);
}

// The lab file and the command.

#[test]
fn issue_135_the_lab_file_must_describe_a_bluetooth_radio_and_allows_nothing_by_default() {
    let config = config::parse(crate::tests::LAB).unwrap();
    assert!(!config.lab.exchange_adverts);
    assert!(config.radios.iter().all(|radio| !radio.reboot_to_drop_link));
    let config = lab();
    assert!(config.radios[1].reboot_to_drop_link);

    let unnamed = LAB.replace("\"MeshCore-FAKE\"", "\"<name>\"");
    let error = config::parse(&unnamed).unwrap_err();
    assert!(error.contains("R3: ble_name isn't filled in"), "{error}");
}

#[test]
fn issue_135_without_a_launcher_bluetooth_radios_are_listed_and_left_alone() {
    // What `--usb-only` and computers other than Macs get.
    let paths = crate::Paths {
        config: "radios.toml".into(),
        lock: "lock".into(),
        logs: "logs".into(),
        uhubctl: "uhubctl".into(),
        ble_program: None,
    };
    let usb_only = crate::Mode {
        usb_only: true,
        ..crate::Mode::default()
    };
    let folder = TempDir::new().unwrap();
    let log = FrameLog::create(folder.path()).unwrap();
    assert!(crate::bluetooth(&paths, usb_only, &log).unwrap().is_none());
}

#[cfg(target_os = "macos")]
#[test]
fn issue_135_the_binary_tells_macos_why_it_uses_bluetooth() {
    // Without the usage description in the binary, macOS ends a program that touches Bluetooth.
    let binary = std::fs::read(std::env::current_exe().unwrap()).unwrap();
    let needle = b"NSBluetoothAlwaysUsageDescription";
    assert!(binary.windows(needle.len()).any(|window| window == needle));
}

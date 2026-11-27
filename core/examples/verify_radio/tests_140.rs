//! Issue #140: the path spike and the advert listening as lab tests, against the spike's fake
//! radios behind the harness's serial link.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use peoples_chat_core::radio::Port;
use peoples_chat_fake_radio::{Config as FakeConfig, FakeRadio, Fixture, Sender};
use tempfile::TempDir;

use crate::config::{self, Config};
use crate::lab::{Lab, Timing, Usb};
use crate::link::{FrameLog, Io};
use crate::lock::LabLock;
use crate::path_spike::fake::FakeMesh;
use crate::path_spike::plan::Options;
use crate::power::Uhubctl;
use crate::{End, Paths, Task, spike, task, verify};

/// The fake mesh's radios are on EU/UK narrow, firmware v1.18.0.
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
label = "R2"
usb_serial = "BBBB2222"
firmware = "v1.18.0"
"#;

/// The fake mesh on USB, on port names that say nothing about which radio is which.
struct MeshUsb(FakeMesh);

const PORTS: [(&str, &str); 2] = [
    ("/dev/cu.usbmodem9901", "AAAA1111"),
    ("/dev/cu.usbmodem1101", "BBBB2222"),
];

#[async_trait]
impl Usb for MeshUsb {
    async fn list(&self) -> Result<Vec<Port>, String> {
        Ok(PORTS
            .iter()
            .rev()
            .map(|(path, serial)| Port {
                path: (*path).to_owned(),
                description: None,
                serial_number: Some((*serial).to_owned()),
            })
            .collect())
    }

    fn open(&self, path: &str) -> Result<Box<dyn Io>, String> {
        let radio = PORTS
            .iter()
            .position(|(port, _)| *port == path)
            .ok_or("no such port")?;
        Ok(Box::new(self.0.serial(radio)))
    }
}

struct Bench {
    usb: MeshUsb,
    config: Config,
    power: Uhubctl,
    log: Arc<FrameLog>,
    folder: TempDir,
}

impl Bench {
    fn new() -> Self {
        let folder = TempDir::new().unwrap();
        Self {
            usb: MeshUsb(FakeMesh::new()),
            config: config::parse(LAB).unwrap(),
            power: Uhubctl {
                program: "false".into(),
            },
            log: FrameLog::create(folder.path()).unwrap(),
            folder,
        }
    }

    fn lab(&self) -> Lab<'_> {
        Lab {
            config: &self.config,
            usb: &self.usb,
            power: &self.power,
            log: self.log.clone(),
            timing: Timing::default(),
        }
    }

    /// The logged frames: radio, direction and bytes.
    fn frames(&self) -> Vec<(String, String, Vec<u8>)> {
        std::fs::read_to_string(self.log.path())
            .unwrap()
            .lines()
            .map(|line| {
                let frame: serde_json::Value = serde_json::from_str(line).unwrap();
                let text = |name: &str| frame[name].as_str().unwrap().to_owned();
                (
                    text("radio"),
                    text("dir"),
                    hex::decode(text("frame")).unwrap(),
                )
            })
            .collect()
    }
}

fn args(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_owned).collect()
}

fn small_run() -> Options {
    spike::options(&args("--texts 10,96 --repeat 1 --flood-repeat 1 --offline")).unwrap()
}

#[test]
fn issue_140_a_frame_log_becomes_one_fixture_per_radio() {
    let log = [
        r#"{"at_ms":1,"radio":"R1","dir":"to_radio","frame":"0a"}"#,
        r#"{"at_ms":2,"radio":"R2","dir":"from_radio","frame":"83"}"#,
        r#"{"at_ms":3,"radio":"R1","dir":"blocked","frame":"0c02"}"#,
        r#"{"at_ms":4,"radio":"R1","dir":"from_radio","frame":"0a"}"#,
    ]
    .join("\n");
    let radios = crate::fixtures::from_log(&log).unwrap();
    assert_eq!(radios["R1"], "app 0a\nradio 0a\n");
    assert_eq!(radios["R2"], "radio 83\n");
    assert!(crate::fixtures::from_log("not a frame").is_err());
}

/// The captures of the lab run, which the fake radio replays.
#[tokio::test]
async fn issue_140_the_lab_captures_replay_on_the_fake_radio() {
    let folder =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fake-radio/tests/fixtures");
    for (radio, key) in [("R1", "32981804a143"), ("R2", "3d0e1c634782")] {
        let path = folder.join(format!("lab-path-spike-{radio}.frames"));
        let fixture = Fixture::load(&path).unwrap();
        // The log's redactions hold: no channel secret was kept.
        for frame in &fixture.frames {
            if frame.sender == Sender::App && frame.payload[0] == 32 {
                assert_eq!(frame.payload[34..50], [0; 16], "{radio}");
            }
        }
        let fake = FakeRadio::replay(FakeConfig::default(), fixture)
            .await
            .unwrap();
        let addr = fake.addr();
        let link = meshcore_rs::MeshCore::tcp(&addr.ip().to_string(), addr.port())
            .await
            .unwrap();
        let me = link.commands().lock().await.send_appstart().await.unwrap();
        assert!(hex::encode(me.public_key).starts_with(key), "{radio}");
        assert_eq!(
            (me.radio_freq, me.sf, me.cr, me.tx_power),
            (869_850, 7, 5, 2)
        );
        assert!(fake.replay_mismatches().is_empty(), "{radio}");
        link.disconnect().await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn issue_140_the_spike_runs_through_the_harness_and_logs_every_frame() {
    let bench = Bench::new();
    let mut out = Vec::new();
    let rows = bench
        .lab()
        .path_spike(small_run(), false, bench.folder.path(), &mut out)
        .await
        .unwrap();
    let out = String::from_utf8(out).unwrap();
    // R1 and R2 are found by serial number, whatever their ports are called.
    assert!(
        out.contains("R1: /dev/cu.usbmodem9901 (USB AAAA1111)"),
        "{out}"
    );
    assert!(out.contains("radio A is R1, radio B is R2"), "{out}");
    let unexpected: Vec<_> = rows.iter().filter(|row| !row.as_expected).collect();
    assert!(unexpected.is_empty(), "{unexpected:#?}");
    assert!(rows.iter().any(|row| row.phase == "receiver away"));
    let csv = std::fs::read_dir(bench.folder.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|extension| extension == "csv"))
        .expect("the run wrote its CSV");
    assert_eq!(
        std::fs::read_to_string(csv).unwrap().lines().count(),
        rows.len() + 1
    );

    // The spike's frames passed through the harness, which refused none of them.
    let frames = bench.frames();
    assert!(
        frames
            .iter()
            .all(|(_, direction, _)| direction != "blocked")
    );
    for command in [2, 25, 31, 32, 62] {
        assert!(
            frames.iter().any(|(radio, direction, frame)| radio == "R1"
                && direction == "to_radio"
                && frame[0] == command),
            "{command}"
        );
    }
    // The test channel's secret isn't kept, and both slots are empty again.
    for (_, direction, frame) in &frames {
        if direction == "to_radio" && frame[0] == 32 {
            assert_eq!(frame[34..50], [0; 16]);
        }
    }
    assert_eq!(bench.usb.0.channel_slot(0, 7), "");
    assert_eq!(bench.usb.0.channel_slot(1, 7), "");
}

#[tokio::test(start_paused = true)]
async fn issue_140_the_spike_can_measure_from_r2_to_r1() {
    let bench = Bench::new();
    let options = spike::options(&args("--paths raw --texts 10 --repeat 1")).unwrap();
    let mut out = Vec::new();
    bench
        .lab()
        .path_spike(options, true, bench.folder.path(), &mut out)
        .await
        .unwrap();
    assert!(bench.frames().iter().any(|(radio, direction, frame)| {
        radio == "R2" && direction == "to_radio" && frame[0] == 25
    }));
}

#[test]
fn issue_140_a_spike_waits_for_the_lab_lock() {
    let folder = TempDir::new().unwrap();
    let config = folder.path().join("radios.toml");
    std::fs::write(&config, LAB).unwrap();
    let paths = Paths {
        config,
        lock: folder.path().join("locks/radio-lab"),
        logs: folder.path().join("logs"),
        uhubctl: "uhubctl".into(),
    };
    let held = LabLock::acquire(&paths.lock).unwrap();
    let run = Task::PathSpike {
        options: small_run(),
        reverse: false,
    };
    let End::Failed(error) = verify(&paths, run, &mut Vec::new()) else {
        panic!("a spike ran while the lab was in use");
    };
    assert!(error.contains("in use"), "{error}");
    assert!(!paths.logs.exists());
    drop(held);
}

#[test]
fn issue_140_the_command_line_never_lets_the_spike_change_power_or_pick_ports() {
    assert_eq!(
        task(&args("-- --path-spike --reverse --offline")),
        Ok(Task::PathSpike {
            options: spike::options(&args("--offline")).unwrap(),
            reverse: true,
        })
    );
    assert_eq!(
        task(&args("--adverts 75")),
        Ok(Task::Adverts {
            period: Duration::from_secs(75 * 60)
        })
    );
    assert_eq!(task(&[]), Ok(Task::Check { dry_run: false }));
    for (line, says) in [
        ("--path-spike --tx-power 2", "never changes a radio's power"),
        ("--path-spike --radio-off", "can't switch a radio off"),
        ("--path-spike /dev/a /dev/b", "two serial ports"),
        ("--path-spike --plan", "no ports"),
        ("--adverts 0", "number of minutes"),
        ("--fast", "Unknown argument"),
    ] {
        let error = task(&args(line)).unwrap_err();
        assert!(error.contains(says), "{line}: {error}");
    }
}

#[tokio::test(start_paused = true)]
async fn issue_140_a_quiet_hour_has_no_adverts_and_prompted_ones_are_heard() {
    let bench = Bench::new();
    let heard = bench
        .lab()
        .adverts(Duration::from_secs(3600), &mut Vec::new())
        .await
        .unwrap();
    assert_eq!(heard.len(), 2);
    for radio in &heard {
        assert!(radio.unprompted.is_empty(), "{radio:?}");
        assert!(radio.control_heard, "{radio:?}");
    }
    // Only the two prompted adverts were sent.
    let adverts = bench
        .frames()
        .iter()
        .filter(|(_, direction, frame)| direction == "to_radio" && frame[0] == 7)
        .count();
    assert_eq!(adverts, 2);
}

#[tokio::test(start_paused = true)]
async fn issue_140_an_advert_a_radio_sends_by_itself_is_counted() {
    let bench = Bench::new();
    let lab = bench.lab();
    let mut out = Vec::new();
    let (heard, ()) = tokio::join!(lab.adverts(Duration::from_secs(3600), &mut out), async {
        tokio::time::sleep(Duration::from_secs(1800)).await;
        bench.usb.0.advertise(0).await;
    });
    let heard = heard.unwrap();
    // R1 advertised, so R2 heard it.
    assert!(heard[0].unprompted.is_empty(), "{heard:?}");
    let r2 = &heard[1];
    assert_eq!(r2.label, "R2");
    assert_eq!(r2.unprompted.len(), 1);
    assert_eq!(r2.unprompted[0].from, r2.other_key);
    assert!(r2.unprompted[0].after >= Duration::from_secs(1790));
}

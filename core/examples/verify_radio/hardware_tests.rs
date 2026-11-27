//! The over-the-air tests (#134) against two fake radios on fake USB ports, so that `pnpm
//! verify` checks what `pnpm verify:radio` does on real ones.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use peoples_chat_core::radio::Port;
use peoples_chat_fake_radio::{Config as Fake, FakeRadio, Fixture, Sender, pair};
use tempfile::TempDir;

use crate::config::{self, Config};
use crate::fixtures;
use crate::lab::{Timing, Usb};
use crate::link::{FrameLog, Io};
use crate::messages::{self, Bench, Waits};
use crate::support;

/// The fake radios' preset and firmware.
const LAB: &str = r#"
[lab]
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

struct Plugged([(&'static str, &'static str, SocketAddr); 2]);

#[async_trait]
impl Usb for Plugged {
    async fn list(&self) -> Result<Vec<Port>, String> {
        Ok(self
            .0
            .iter()
            .map(|(serial, path, _)| Port {
                path: (*path).to_owned(),
                description: None,
                serial_number: Some((*serial).to_owned()),
            })
            .collect())
    }

    fn open(&self, path: &str) -> Result<Box<dyn Io>, String> {
        let (_, _, addr) = self
            .0
            .iter()
            .find(|(_, known, _)| *known == path)
            .ok_or("no such port")?;
        let stream = std::net::TcpStream::connect(addr).map_err(|error| error.to_string())?;
        stream
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let stream = tokio::net::TcpStream::from_std(stream).map_err(|error| error.to_string())?;
        Ok(Box::new(stream))
    }
}

fn lab() -> Config {
    config::parse(LAB).unwrap()
}

fn waits() -> Waits {
    Waits {
        delivery: Duration::from_secs(30),
        advert: Duration::from_secs(5),
        app: Duration::from_secs(60),
        closed: Duration::from_secs(2),
        reconnect: Duration::from_secs(30),
    }
}

/// One run over two fake radios: how it ended, what it printed, and its frame log's folder.
async fn run(
    radios: &(FakeRadio, FakeRadio),
) -> (Result<(), String>, String, TempDir, Arc<FrameLog>) {
    let folder = TempDir::new().unwrap();
    let log = FrameLog::create(folder.path()).unwrap();
    let bench = Bench {
        config: lab(),
        usb: Arc::new(Plugged([
            ("AAAA1111", "/dev/cu.fake1", radios.0.addr()),
            ("BBBB2222", "/dev/cu.fake2", radios.1.addr()),
        ])),
        log: log.clone(),
        timing: Timing::default(),
        waits: waits(),
    };
    let mut out = Vec::new();
    let result = messages::run(&bench, &mut out).await;
    (result, String::from_utf8(out).unwrap(), folder, log)
}

async fn radios() -> (FakeRadio, FakeRadio) {
    pair(Fake::default(), Fake::default()).await.unwrap()
}

/// The whole run, with the first message lost once: every criterion of the issue, in the order
/// the run checks them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_134_two_apps_pass_every_check_over_two_radios() {
    let _turn = support::take_turn().await;
    let radios = radios().await;
    radios.0.lose_next_messages(1);
    let (result, out, _folder, log) = run(&radios).await;
    result.unwrap_or_else(|error| panic!("{error}\n{out}"));
    let mut rest = out.as_str();
    for expected in [
        // The address records, on both sides.
        "ok R1: stored R2's radio address record",
        "ok R2: stored R1's radio address record",
        // Reachable only after the radio has been heard.
        "ok R1: contact not reachable, their radio not heard yet",
        "ok R2: contact not reachable, their radio not heard yet",
        "ok R1: heard R2's advert",
        "ok R2: heard R1's advert",
        // Sent, arrived and acknowledged, with the one retry.
        "ok \"Lab message 1\" from R1 to R2",
        "1 retries",
        "ok \"Lab reply\" from R2 to R1",
        // The port closes, the app reconnects, the next message arrives.
        "ok R2: serial port closed",
        "ok R2: reconnected by itself",
        "ok \"Lab message 2\" from R1 to R2",
    ] {
        let at = rest
            .find(expected)
            .unwrap_or_else(|| panic!("missing {expected:?} in:\n{out}"));
        rest = &rest[at + expected.len()..];
    }

    // The frames of the run, as fixtures the fake radio loads, one per radio.
    let saved = fixtures::save(log.path()).unwrap();
    assert_eq!(saved.len(), 2, "{saved:?}");
    for (path, radio) in saved.iter().zip(["R1", "R2"]) {
        assert!(
            path.to_str()
                .unwrap()
                .ends_with(&format!("-{radio}.frames"))
        );
        let fixture = Fixture::load(path).unwrap();
        let has = |sender, code| {
            fixture
                .frames
                .iter()
                .any(|frame| frame.sender == sender && frame.payload[0] == code)
        };
        // A message and an advert went out; the confirmation or the message came in.
        assert!(has(Sender::App, 2) && has(Sender::App, 7), "{radio}");
        assert!(has(Sender::Radio, 0x80), "{radio}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_134_a_message_lost_twice_fails_the_run() {
    let _turn = support::take_turn().await;
    let radios = radios().await;
    radios.0.lose_next_messages(2);
    let (result, out, _folder, _) = run(&radios).await;
    let error = result.unwrap_err();
    assert!(
        error.contains("timed out after 1 retries: it didn't arrive"),
        "{error}\n{out}"
    );
    assert!(!out.contains("serial port closed"), "{out}");
}

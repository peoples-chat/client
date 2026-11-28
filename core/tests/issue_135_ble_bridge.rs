//! Issue #135: the Bluetooth bridge (`tools/ble-bridge/`). The real program is built here and
//! run with a fake companion radio over TCP (#132) in place of the Bluetooth one, so its TCP
//! side, its control port and how the core lives with it are covered without hardware. No test
//! here reaches Bluetooth.
//!
//! The same against the lab's radio is `pnpm verify:radio` with the bridge installed
//! (`docs/radio-lab.md`).

// The bridge is a Mac program.
#![cfg(target_os = "macos")]

#[path = "support/bridge.rs"]
mod bridge;
mod support;

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use peoples_chat_core::Core;
use peoples_chat_core::interface::RadioConnection;
use peoples_chat_core::radio::{MeshCoreRadio, Port, Radio, RadioError, RadioEvent, RadioPorts};
use peoples_chat_fake_radio::{self as fake, Config, FakeRadio};
use support::{Phone, TestRelay, until};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::broadcast;

use crate::bridge::{Bridge, program, repository};

const HOST: &str = "127.0.0.1";

async fn next_event(events: &mut broadcast::Receiver<RadioEvent>) -> RadioEvent {
    tokio::time::timeout(Duration::from_secs(15), events.recv())
        .await
        .expect("no radio event in time")
        .unwrap()
}

/// Waits for the message `expected`, past other events.
async fn arrives(events: &mut broadcast::Receiver<RadioEvent>, expected: &str) {
    loop {
        match next_event(events).await {
            RadioEvent::TextReceived { text, .. } => return assert_eq!(text, expected),
            RadioEvent::Disconnected => panic!("the link was lost before {expected:?} arrived"),
            _ => {}
        }
    }
}

/// R1 reached directly, as over USB, and R3 behind the bridge.
async fn two_radios() -> (FakeRadio, FakeRadio, Bridge) {
    let (r1, r3) = fake::pair(Config::default(), Config::default())
        .await
        .unwrap();
    let bridge = Bridge::start(r3.addr());
    bridge.until("connected").await;
    (r1, r3, bridge)
}

async fn direct(radio: &FakeRadio) -> MeshCoreRadio {
    MeshCoreRadio::connect_tcp(HOST, radio.addr().port())
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_135_messages_pass_through_the_bridge_both_ways() {
    let (r1, r3, bridge) = two_radios().await;
    let usb = direct(&r1).await;
    let ble = MeshCoreRadio::connect_tcp(HOST, bridge.port).await.unwrap();
    assert_eq!(
        ble.info().await.unwrap().public_key,
        hex::encode(r3.public_key())
    );

    let mut at_ble = ble.events();
    usb.send_text(&hex::encode(r3.public_key()), "To Bluetooth")
        .await
        .unwrap();
    arrives(&mut at_ble, "To Bluetooth").await;

    let mut at_usb = usb.events();
    ble.send_text(&hex::encode(r1.public_key()), "From Bluetooth")
        .await
        .unwrap();
    arrives(&mut at_usb, "From Bluetooth").await;
}

/// The app's APP_START frame as it travels over a stream.
fn app_start() -> Vec<u8> {
    let payload = [&[1u8, 3, 0, 0, 0, 0, 0, 0][..], b"test"].concat();
    [&[b'<', payload.len() as u8, 0][..], &payload].concat()
}

/// Reads one frame from the radio's side: `>`, a length, the payload.
async fn frame(stream: &mut TcpStream) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(5), async {
        assert_eq!(stream.read_u8().await.unwrap(), b'>');
        let mut payload = vec![0; usize::from(stream.read_u16_le().await.unwrap())];
        stream.read_exact(&mut payload).await.unwrap();
        payload
    })
    .await
    .expect("no frame in time")
}

/// Whether the other end closes the connection.
async fn closed(stream: &mut TcpStream) -> bool {
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut rest))
        .await
        .is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_135_the_bridge_keeps_frames_whole_and_serves_one_client() {
    let r3 = FakeRadio::start(Config::default()).await.unwrap();
    let bridge = Bridge::start(r3.addr());
    bridge.until("connected").await;

    // A frame that arrives in pieces, after bytes that belong to none.
    let mut first = TcpStream::connect((HOST, bridge.port)).await.unwrap();
    let sent = [&b"noise"[..], &app_start()].concat();
    for piece in sent.chunks(3) {
        first.write_all(piece).await.unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let self_info = frame(&mut first).await;
    assert_eq!(self_info[0], 5);
    assert_eq!(self_info[4..36], r3.public_key());

    // A second client takes over; the first is let go.
    let mut second = TcpStream::connect((HOST, bridge.port)).await.unwrap();
    second.write_all(&app_start()).await.unwrap();
    assert_eq!(frame(&mut second).await[0], 5);
    assert!(closed(&mut first).await);

    // A length no frame has ends the connection; the bridge carries on.
    second.write_all(&[b'<', 0xff, 0xff]).await.unwrap();
    assert!(closed(&mut second).await);
    assert_eq!(bridge.ask("status"), "connected");
    assert_eq!(bridge.ask("reboot"), "error: unknown command");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_135_a_drop_on_request_ends_the_link_and_the_bridge_reconnects() {
    let (r1, r3, bridge) = two_radios().await;
    let ble = MeshCoreRadio::connect_tcp(HOST, bridge.port).await.unwrap();
    let mut events = ble.events();

    assert_eq!(bridge.ask("drop"), "dropped");
    assert_eq!(bridge.ask("status"), "disconnected");
    // There is nothing to drop, and nothing to serve, while it is away.
    assert_eq!(bridge.ask("drop"), "error: the radio isn't connected");
    let mut early = TcpStream::connect((HOST, bridge.port)).await.unwrap();
    assert!(closed(&mut early).await);
    // The core notices.
    assert_eq!(next_event(&mut events).await, RadioEvent::Disconnected);

    // The bridge reconnects by itself, and a new link works.
    bridge.until("connected").await;
    let again = MeshCoreRadio::connect_tcp(HOST, bridge.port).await.unwrap();
    let mut events = again.events();
    direct(&r1)
        .await
        .send_text(&hex::encode(r3.public_key()), "Back again")
        .await
        .unwrap();
    arrives(&mut events, "Back again").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_135_the_bridge_reconnects_when_the_radio_goes_away_by_itself() {
    let r3 = FakeRadio::start(Config::default()).await.unwrap();
    let port = r3.addr().port();
    let bridge = Bridge::start(r3.addr());
    bridge.until("connected").await;
    let mut client = TcpStream::connect((HOST, bridge.port)).await.unwrap();
    client.write_all(&app_start()).await.unwrap();
    frame(&mut client).await;

    // The radio is gone: the client is told by its connection ending.
    drop(r3);
    assert!(closed(&mut client).await);
    bridge.until("disconnected").await;

    // It comes back, and the bridge finds it.
    let _back = loop {
        let config = Config {
            port,
            ..Config::default()
        };
        match FakeRadio::start(config).await {
            Ok(radio) => break radio,
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    };
    bridge.until("connected").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_135_a_radio_that_goes_quiet_behind_the_bridge_is_noticed() {
    let (_r1, r3, bridge) = two_radios().await;
    let ble = MeshCoreRadio::connect_tcp(HOST, bridge.port).await.unwrap();
    // The app checks every 10 seconds.
    ble.watch_link(Duration::from_millis(100));
    let mut events = ble.events();
    // The bridge still has its link and the connection stays open: only the checks can tell.
    r3.go_quiet();
    assert_eq!(next_event(&mut events).await, RadioEvent::Disconnected);
}

/// Where the core looks for radios: the one behind the bridge.
struct ThroughBridge(u16);

#[async_trait]
impl RadioPorts for ThroughBridge {
    async fn list(&self) -> Result<Vec<Port>, RadioError> {
        Ok(vec![Port {
            path: "bridge".to_owned(),
            description: None,
            serial_number: None,
        }])
    }

    async fn open(&self, _path: &str) -> Result<Arc<dyn Radio>, RadioError> {
        Ok(Arc::new(MeshCoreRadio::connect_tcp(HOST, self.0).await?))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_135_the_core_connects_again_after_the_bridge_dropped_the_link() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let phone = Phone::new("bridge-ada", &relay.url());
    let core: Core = phone.start().await;
    core.set_radio_reconnect_wait(Duration::from_millis(50));
    let r3 = FakeRadio::start(Config::default()).await.unwrap();
    // Away long enough for the interface to show it.
    let bridge = Bridge::pausing(r3.addr(), Duration::from_secs(2));
    bridge.until("connected").await;
    core.use_radio_ports(Arc::new(ThroughBridge(bridge.port)));
    let connection = |core: &Core| core.snapshot().radio.paired.map(|radio| radio.connection);

    let paired = core.pair_radio("bridge").await.unwrap().paired.unwrap();
    assert_eq!(paired.connection, RadioConnection::Connected);

    assert_eq!(bridge.ask("drop"), "dropped");
    until("the core noticing the dropped link", || {
        (connection(&core) == Some(RadioConnection::Connecting)).then_some(())
    })
    .await;
    // The same core, still running, has the radio back.
    until("the radio back in use", || {
        (connection(&core) == Some(RadioConnection::Connected)).then_some(())
    })
    .await;
    core.close().await.unwrap();
}

// Bluetooth permission. Only refusals can be assumed, so these never reach Bluetooth.

#[test]
fn issue_135_the_bridge_stops_without_using_bluetooth_unless_macos_allows_it() {
    for (state, says) in [
        ("notDetermined", "not asked yet"),
        ("denied", "denied"),
        ("restricted", "restricted"),
    ] {
        let output = Command::new(program())
            .args(["--name", "MeshCore-NONE", "--assume-authorization", state])
            .output()
            .unwrap();
        let error = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(77), "{error}");
        assert!(
            error.contains(&format!(
                "macOS hasn't allowed this program to use Bluetooth (it says: {says})"
            )),
            "{error}"
        );
        assert!(error.contains("install.sh --allow-bluetooth"), "{error}");
        // It stopped before it listened or looked for anything.
        assert!(output.stdout.is_empty());
    }
    // An approval can't be assumed: that would skip the check.
    let output = Command::new(program())
        .args([
            "--name",
            "MeshCore-NONE",
            "--assume-authorization",
            "allowed",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

// The build, the lab file and the launchd job.

#[test]
fn issue_135_the_built_bridge_carries_its_identifier_and_why_it_uses_bluetooth() {
    // Without the usage description inside, macOS ends a program that touches Bluetooth.
    let binary = std::fs::read(program()).unwrap();
    for needle in [
        "chat.peoples.ble-bridge",
        "NSBluetoothAlwaysUsageDescription",
    ] {
        assert!(
            binary
                .windows(needle.len())
                .any(|window| window == needle.as_bytes()),
            "{needle}"
        );
    }
    let signature = Command::new("codesign")
        .args(["--display", "--verbose=2"])
        .arg(program())
        .output()
        .unwrap();
    let signature = String::from_utf8_lossy(&signature.stderr);
    assert!(
        signature.contains("Identifier=chat.peoples.ble-bridge"),
        "{signature}"
    );
}

/// A port nothing listens on right now.
fn free_port() -> u16 {
    std::net::TcpListener::bind((HOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_135_the_bridge_takes_its_ports_from_the_lab_file() {
    let r3 = FakeRadio::start(Config::default()).await.unwrap();
    let (port, control) = (free_port(), free_port());
    let folder = TempDir::new().unwrap();
    let lab = folder.path().join("radios.toml");
    std::fs::write(
        &lab,
        format!(
            "[lab]\nfrequency_mhz = 869.85\n\n\
             [[radio]]\nlabel = \"R1\"\nusb_serial = \"AAAA\"\nbridge_port = 1\n\n\
             # The Bluetooth one.\n\
             [[radio]]\nlabel = \"R3\"\nble_name = \"MeshCore-FAKE\"  # advertised\n\
             bridge_port = {port}\nbridge_control_port = {control}\n"
        ),
    )
    .unwrap();
    let bridge = Bridge::with(&[
        "--fake-radio",
        &r3.addr().to_string(),
        "--lab",
        lab.to_str().unwrap(),
    ]);
    assert_eq!((bridge.port, bridge.control_port), (port, control));
    bridge.until("connected").await;
}

#[test]
fn issue_135_the_launchd_job_runs_the_installed_bridge_and_verify_never_installs_it() {
    let folder = TempDir::new().unwrap();
    let write = |keep: &str| -> serde_json::Value {
        let plist = folder.path().join("job.plist");
        let status = Command::new("bash")
            .arg(repository().join("tools/ble-bridge/job.sh"))
            .arg(&plist)
            .args(["chat.peoples.factory.ble-bridge", "/tmp/out.log", keep])
            .args(["/opt/a & b/ble-bridge", "--lab", "/tmp/radios.toml"])
            .status()
            .unwrap();
        assert!(status.success());
        let json = Command::new("plutil")
            .args(["-convert", "json", "-o", "-"])
            .arg(&plist)
            .output()
            .unwrap();
        serde_json::from_slice(&json.stdout).unwrap()
    };
    // The job that asks for permission runs once: with any `KeepAlive`, launchd would start it
    // again.
    assert_eq!(write("false").get("KeepAlive"), None);
    let job = write("true");
    assert_eq!(job["Label"], "chat.peoples.factory.ble-bridge");
    assert_eq!(
        job["ProgramArguments"],
        serde_json::json!(["/opt/a & b/ble-bridge", "--lab", "/tmp/radios.toml"])
    );
    assert_eq!(job["RunAtLoad"], true);
    assert_eq!(job["KeepAlive"]["Crashed"], true);

    // The install script parses; nothing runs it.
    let install = repository().join("tools/ble-bridge/install.sh");
    let parsed = Command::new("bash")
        .arg("-n")
        .arg(&install)
        .status()
        .unwrap();
    assert!(parsed.success());
    let text = std::fs::read_to_string(install).unwrap();
    assert!(text.contains("$HOME/factory/bin/ble-bridge"));
    for runs_in_verify in ["scripts/verify.sh", "package.json"] {
        let text = std::fs::read_to_string(repository().join(runs_in_verify)).unwrap();
        assert!(!text.contains("install.sh --") && !text.contains("ble-bridge"));
    }
}

//! Pairing a radio over USB (#24), with mock radios on mock ports: listing the ports, pairing,
//! the preset and its warning, remembering the radio across a restart, opening it again after it
//! restarts or is plugged in elsewhere, and forgetting it. No real serial port is opened.

mod support;

use std::sync::Arc;
use std::time::Duration;

use peoples_chat_core::Core;
use peoples_chat_core::interface::{
    CoreEvent, PairedRadio, RadioConnection, RadioPort, RadioPreset,
};
use peoples_chat_core::radio::{MockMesh, MockPorts, RadioSettings};
use support::{Phone, TestRelay, next_event, until};

const RADIO_PORT: &str = "/dev/cu.usbmodem1101";
const MOVED_PORT: &str = "/dev/cu.usbmodem2101";
const OTHER_PORT: &str = "/dev/cu.usbserial-0001";

/// MeshCore's EU/UK (Narrow) preset, as the mock radios have it.
const EU_NARROW: RadioPreset = RadioPreset {
    frequency_khz: 869_618,
    bandwidth_hz: 62_500,
    spreading_factor: 8,
    coding_rate: 8,
    expected: true,
};

fn paired(core: &Core) -> Option<PairedRadio> {
    core.snapshot().radio.paired
}

async fn connection_is(core: &Core, connection: RadioConnection) -> PairedRadio {
    until(&format!("the radio {connection:?}"), || {
        paired(core).filter(|radio| radio.connection == connection)
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pairs_a_radio_remembers_it_and_forgets_it() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let phone = Phone::new("pairing-ada", &relay.url());
    let core = phone.start().await;
    let mesh = MockMesh::new();
    let radio = mesh.radio("Ada's radio");
    let ports = MockPorts::new();
    ports.plug(RADIO_PORT, &radio);
    ports.plug_other(OTHER_PORT, "CP2102 USB to UART");

    // Without ports, as on a phone, nothing can be paired.
    assert!(!core.snapshot().radio.can_pair);
    assert_eq!(
        core.radio_ports().await.unwrap_err().message,
        "This device can't pair a radio yet."
    );
    let mut events = core.subscribe();
    core.use_radio_ports(Arc::new(ports.clone()));
    match next_event(&mut events).await {
        CoreEvent::RadioChanged { radio } => assert!(radio.can_pair && radio.paired.is_none()),
        other => panic!("expected the radio, got {other:?}"),
    }
    assert_eq!(
        core.radio_ports().await.unwrap(),
        vec![
            RadioPort {
                path: RADIO_PORT.to_owned(),
                description: Some("Mock radio".to_owned()),
            },
            RadioPort {
                path: OTHER_PORT.to_owned(),
                description: Some("CP2102 USB to UART".to_owned()),
            },
        ]
    );

    // A device that isn't a radio doesn't answer, and a port that went can't be paired.
    assert_eq!(
        core.pair_radio(OTHER_PORT).await.unwrap_err().message,
        "The device on /dev/cu.usbserial-0001 didn't answer. Is it a MeshCore radio with \
         companion firmware for USB?"
    );
    assert_eq!(
        core.pair_radio("/dev/cu.gone").await.unwrap_err().message,
        "That port is gone. Plug the radio in and look again."
    );
    assert_eq!(paired(&core), None);

    // The radio: its key, name, port and preset.
    let expected = PairedRadio {
        name: "Ada's radio".to_owned(),
        public_key: radio.public_key().to_owned(),
        connection: RadioConnection::Connected,
        port: Some(RADIO_PORT.to_owned()),
        preset: EU_NARROW,
    };
    let status = core.pair_radio(RADIO_PORT).await.unwrap();
    assert_eq!(status.paired.as_ref(), Some(&expected));
    assert_eq!(paired(&core).as_ref(), Some(&expected));
    let announced = until("the paired radio announced", || match events.try_recv() {
        Ok(CoreEvent::RadioChanged { radio }) => radio.paired.filter(|p| p.port.is_some()),
        _ => None,
    })
    .await;
    assert_eq!(announced, expected);
    // Pairing it again changes nothing.
    let opened = ports.opened().len();
    assert_eq!(
        core.pair_radio(RADIO_PORT).await.unwrap().paired,
        Some(expected.clone())
    );
    assert_eq!(ports.opened().len(), opened);

    // After a restart the core remembers it, and opens it again once it has its ports.
    core.close().await.unwrap();
    let core = phone.start().await;
    let remembered = paired(&core).unwrap();
    assert_eq!(
        (
            remembered.name.as_str(),
            remembered.port.as_deref(),
            remembered.connection
        ),
        (
            "Ada's radio",
            Some(RADIO_PORT),
            RadioConnection::Disconnected
        )
    );
    core.use_radio_ports(Arc::new(ports.clone()));
    assert_eq!(
        connection_is(&core, RadioConnection::Connected).await,
        expected
    );

    // Forgotten: the core stops using it, and stays so after a restart.
    let status = core.forget_radio().await.unwrap();
    assert_eq!(status.paired, None);
    assert!(status.can_pair);
    core.close().await.unwrap();
    let core = phone.start().await;
    core.use_radio_ports(Arc::new(ports.clone()));
    assert_eq!(paired(&core), None);
    let opened = ports.opened().len();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(ports.opened().len(), opened, "nothing looks for it");
    core.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn opens_the_paired_radio_again_when_it_comes_back() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let phone = Phone::new("pairing-bea", &relay.url());
    let core = phone.start().await;
    core.set_radio_reconnect_wait(Duration::from_millis(50));
    let mesh = MockMesh::new();
    let radio = mesh.radio("Bea's radio");
    let stranger = mesh.radio("Someone else's radio");
    let ports = MockPorts::new();
    ports.plug(RADIO_PORT, &radio);
    core.use_radio_ports(Arc::new(ports.clone()));
    core.pair_radio(RADIO_PORT).await.unwrap();

    // The radio restarts: its link drops, and the core looks for it until it is back.
    ports.unplug(RADIO_PORT);
    let looking = connection_is(&core, RadioConnection::Connecting).await;
    assert_eq!(
        (looking.public_key.as_str(), looking.port.as_deref()),
        (radio.public_key(), Some(RADIO_PORT))
    );
    ports.plug(RADIO_PORT, &radio);
    connection_is(&core, RadioConnection::Connected).await;

    // Plugged in elsewhere, it is found by its USB serial number.
    ports.unplug(RADIO_PORT);
    connection_is(&core, RadioConnection::Connecting).await;
    ports.plug(MOVED_PORT, &radio);
    let moved = connection_is(&core, RadioConnection::Connected).await;
    assert_eq!(moved.port.as_deref(), Some(MOVED_PORT));

    // Another radio on its port isn't taken for it.
    ports.unplug(MOVED_PORT);
    connection_is(&core, RadioConnection::Connecting).await;
    ports.plug(MOVED_PORT, &stranger);
    until("a look at the other radio", || {
        (ports
            .opened()
            .iter()
            .filter(|path| *path == MOVED_PORT)
            .count()
            >= 3)
            .then_some(())
    })
    .await;
    let still = paired(&core).unwrap();
    assert_eq!(
        (still.public_key.as_str(), still.connection),
        (radio.public_key(), RadioConnection::Connecting)
    );
    ports.unplug(MOVED_PORT);
    ports.plug(RADIO_PORT, &radio);
    let back = connection_is(&core, RadioConnection::Connected).await;
    assert_eq!(back.port.as_deref(), Some(RADIO_PORT));
    core.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn warns_about_another_preset_and_forgets_the_radio_with_the_account() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let phone = Phone::new("pairing-cleo", &relay.url());
    let core = phone.start().await;
    core.create_account("Cleo Dahl").await.unwrap();
    let mesh = MockMesh::new();
    let far = mesh.radio_with(
        "Cleo's radio",
        RadioSettings {
            frequency_mhz: 869.525,
            bandwidth_khz: 250.0,
            spreading_factor: 11,
            coding_rate: 5,
            tx_power_dbm: 22,
        },
    );
    let ports = MockPorts::new();
    ports.plug(RADIO_PORT, &far);
    core.use_radio_ports(Arc::new(ports.clone()));
    let status = core.pair_radio(RADIO_PORT).await.unwrap();
    assert_eq!(
        status.paired.unwrap().preset,
        RadioPreset {
            frequency_khz: 869_525,
            bandwidth_hz: 250_000,
            spreading_factor: 11,
            coding_rate: 5,
            expected: false,
        }
    );

    // The paired radio goes with the account; pairing another stays possible.
    let mut events = core.subscribe();
    core.delete_account().await.unwrap();
    assert_eq!(next_event(&mut events).await, CoreEvent::AccountDeleted);
    match next_event(&mut events).await {
        CoreEvent::RadioChanged { radio } => assert!(radio.can_pair && radio.paired.is_none()),
        other => panic!("expected the radio, got {other:?}"),
    }
    core.close().await.unwrap();
    let core = phone.start().await;
    assert_eq!(paired(&core), None);
    core.close().await.unwrap();
}

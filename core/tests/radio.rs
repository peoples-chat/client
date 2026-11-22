//! The radio transport interface (#60): mock radios on a mock mesh, and the MeshCore adapter
//! against a fake companion radio that answers in MeshCore firmware v1.18.0's frame layouts.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meshcore_rs::commands::CommandHandler;
use meshcore_rs::events::EventDispatcher;
use meshcore_rs::reader::MessageReader;
use peoples_chat_core::radio::{
    ContactKind, MAX_TEXT_BYTES, MeshCoreRadio, MockMesh, Radio, RadioError, RadioEvent, Sent,
};
use tokio::sync::{broadcast, mpsc};

const WAIT: Duration = Duration::from_secs(10);

async fn next_event(events: &mut broadcast::Receiver<RadioEvent>) -> RadioEvent {
    tokio::time::timeout(WAIT, events.recv())
        .await
        .expect("no radio event in time")
        .expect("the radio stopped sending events")
}

// The mock mesh.

#[tokio::test]
async fn mock_radios_exchange_text_and_confirm_delivery() {
    let mesh = MockMesh::new();
    let ada = mesh.radio("Ada");
    let bea = mesh.radio("Bea");
    let mut ada_events = ada.events();
    let mut bea_events = bea.events();

    let info = ada.info().await.unwrap();
    assert_eq!(info.public_key, ada.public_key());
    assert_eq!(info.name, "Ada");
    let contacts = ada.contacts().await.unwrap();
    assert_eq!(contacts.len(), 1);
    assert_eq!(contacts[0].public_key, bea.public_key());
    assert_eq!(contacts[0].name, "Bea");
    assert_eq!(contacts[0].kind, ContactKind::Chat);

    let sent = ada
        .send_text(bea.public_key(), "Storm tonight")
        .await
        .unwrap();
    match next_event(&mut bea_events).await {
        RadioEvent::TextReceived {
            from_prefix,
            from,
            text,
            ..
        } => {
            assert_eq!(from_prefix, &ada.public_key()[..12]);
            assert_eq!(from.as_deref(), Some(ada.public_key()));
            assert_eq!(text, "Storm tonight");
        }
        other => panic!("expected a message, got {other:?}"),
    }
    assert_eq!(
        next_event(&mut ada_events).await,
        RadioEvent::Delivered { ack: sent.ack }
    );

    // Out of range, a message goes out but never arrives.
    mesh.set_in_range(bea.public_key(), false);
    ada.send_text(bea.public_key(), "Are you there?")
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(200), bea_events.recv())
            .await
            .is_err()
    );
    mesh.set_in_range(bea.public_key(), true);
    bea.advertise();
    assert_eq!(
        next_event(&mut ada_events).await,
        RadioEvent::ContactHeard {
            public_key: bea.public_key().to_owned()
        }
    );
}

#[tokio::test]
async fn mock_radios_say_what_is_wrong() {
    let mesh = MockMesh::new();
    let ada = mesh.radio("Ada");
    let bea = mesh.radio("Bea");
    let mut events = ada.events();

    assert!(matches!(
        ada.send_text("00ff", "Hi").await,
        Err(RadioError::UnknownContact(_))
    ));
    let long = "x".repeat(MAX_TEXT_BYTES + 1);
    assert!(matches!(
        ada.send_text(bea.public_key(), &long).await,
        Err(RadioError::TooLong {
            max: MAX_TEXT_BYTES
        })
    ));
    assert!(matches!(
        ada.send_text(bea.public_key(), "a\0b").await,
        Err(RadioError::InvalidText)
    ));

    ada.disconnect().await.unwrap();
    assert_eq!(next_event(&mut events).await, RadioEvent::Disconnected);
    assert!(matches!(ada.info().await, Err(RadioError::NotConnected)));
    assert!(matches!(
        ada.send_text(bea.public_key(), "Hi").await,
        Err(RadioError::NotConnected)
    ));
}

// MeshCore, against a fake companion radio.

/// Bea's radio: a contact of the fake radio, with a known route.
const BEA_KEY: [u8; 32] = [
    0xbe, 0xa0, 0x11, 0x22, 0x33, 0x44, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
    21, 22, 23, 24, 25, 26, 27, 28, 29, 30,
];
/// The fake radio's own key.
const OWN_KEY: [u8; 32] = [0xad; 32];
/// The code the fake radio expects to confirm its sent message with.
const EXPECTED_ACK: [u8; 4] = [0xa1, 0xb2, 0xc3, 0xd4];

/// A companion radio in a test: it answers meshcore-rs's commands with frames laid out as
/// MeshCore firmware v1.18.0 sends them (`examples/companion_radio/MyMesh.cpp`).
struct FakeRadio {
    reader: Arc<MessageReader>,
    /// Message frames waiting for the app to collect them.
    waiting: Mutex<VecDeque<Vec<u8>>>,
}

impl FakeRadio {
    /// Starts a fake radio, and returns it with the handler and dispatcher an app talks to.
    fn start() -> (
        Arc<Self>,
        Arc<tokio::sync::Mutex<CommandHandler>>,
        Arc<EventDispatcher>,
    ) {
        let (to_radio, mut from_app) = mpsc::channel::<Vec<u8>>(64);
        let dispatcher = Arc::new(EventDispatcher::new());
        let reader = Arc::new(MessageReader::new(dispatcher.clone()));
        let commands = Arc::new(tokio::sync::Mutex::new(CommandHandler::new(
            to_radio,
            dispatcher.clone(),
            reader.clone(),
        )));
        let radio = Arc::new(Self {
            reader,
            waiting: Mutex::new(VecDeque::new()),
        });
        let answering = radio.clone();
        tokio::spawn(async move {
            while let Some(command) = from_app.recv().await {
                // meshcore-rs listens for the answer only after it has written the command, so
                // a radio that answers at once would be missed.
                tokio::time::sleep(Duration::from_millis(20)).await;
                for frame in answering.answer(&command) {
                    answering.push(frame).await;
                }
            }
        });
        (radio, commands, dispatcher)
    }

    /// Sends a frame to the app, as the radio would over its link.
    async fn push(&self, frame: Vec<u8>) {
        self.reader.handle_rx(frame).await.unwrap();
    }

    fn answer(&self, command: &[u8]) -> Vec<Vec<u8>> {
        match command[0] {
            1 => vec![self_info()],
            22 => vec![device_info()],
            4 => vec![
                [&[2u8][..], &1u32.to_le_bytes()].concat(),
                contact(),
                [&[4u8][..], &1_795_300_000u32.to_le_bytes()].concat(),
            ],
            2 => {
                // 02 | txt_type | attempt | timestamp | 6-byte recipient prefix | text
                if command[7..13] == BEA_KEY[..6] {
                    // SENT: direct, expected ACK, suggested timeout 3000 ms.
                    vec![[&[6u8, 0][..], &EXPECTED_ACK, &3000u32.to_le_bytes()].concat()]
                } else {
                    vec![vec![1, 2]] // ERR NOT_FOUND
                }
            }
            10 => vec![
                self.waiting
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| vec![10]), // NO_MORE_MESSAGES
            ],
            _ => vec![vec![1, 1]], // ERR UNSUPPORTED_CMD
        }
    }
}

/// SELF_INFO: Ada's radio on MeshCore's EU/UK narrow preset (869.618 MHz, 62.5 kHz, SF 8, CR 8).
fn self_info() -> Vec<u8> {
    let mut frame = vec![5, 1, 22, 22];
    frame.extend_from_slice(&OWN_KEY);
    frame.extend_from_slice(&0i32.to_le_bytes()); // lat
    frame.extend_from_slice(&0i32.to_le_bytes()); // lon
    frame.extend_from_slice(&[0, 0, 0, 0]); // multi_acks, loc policy, telemetry, manual add
    frame.extend_from_slice(&869_618u32.to_le_bytes()); // kHz
    frame.extend_from_slice(&62_500u32.to_le_bytes()); // Hz
    frame.extend_from_slice(&[8, 8]);
    frame.extend_from_slice(b"Ada radio");
    frame
}

/// DEVICE_INFO: firmware code 14 (v1.18.0), a Heltec V3.
fn device_info() -> Vec<u8> {
    let mut frame = vec![13, 14, 175, 40];
    frame.extend_from_slice(&0u32.to_le_bytes()); // BLE PIN
    frame.extend_from_slice(&padded(b"15 Oct 2026", 12));
    frame.extend_from_slice(&padded(b"Heltec V3", 40));
    frame.extend_from_slice(&padded(b"v1.18.0", 20));
    frame.extend_from_slice(&[0, 0]); // client repeat, path hash mode
    frame
}

/// CONTACT: Bea's radio, a chat node with a one-hop route.
fn contact() -> Vec<u8> {
    let mut frame = vec![3];
    frame.extend_from_slice(&BEA_KEY);
    frame.extend_from_slice(&[1, 0, 1]); // type chat, flags, out_path_len
    frame.extend_from_slice(&padded(&[0x42], 64));
    frame.extend_from_slice(&padded(b"Bea", 32));
    frame.extend_from_slice(&1_795_299_000u32.to_le_bytes()); // last advert
    frame.extend_from_slice(&0i32.to_le_bytes());
    frame.extend_from_slice(&0i32.to_le_bytes());
    frame.extend_from_slice(&1_795_300_000u32.to_le_bytes()); // lastmod
    frame
}

/// CONTACT_MSG_RECV_V3 from Bea: SNR 7.25 dB, sent at 1795300100.
fn message_from_bea(text: &str) -> Vec<u8> {
    let mut frame = vec![16, 29, 0, 0]; // snr x4, two reserved bytes
    frame.extend_from_slice(&BEA_KEY[..6]);
    frame.extend_from_slice(&[0xff, 0]); // path_len: arrived direct; txt_type plain
    frame.extend_from_slice(&1_795_300_100u32.to_le_bytes());
    frame.extend_from_slice(text.as_bytes());
    frame
}

fn padded(bytes: &[u8], len: usize) -> Vec<u8> {
    let mut padded = bytes.to_vec();
    padded.resize(len, 0);
    padded
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn meshcore_radio_reads_the_companion_protocol() {
    let (fake, commands, dispatcher) = FakeRadio::start();
    let radio = MeshCoreRadio::over(commands, &dispatcher).await.unwrap();
    let mut events = radio.events();

    let info = radio.info().await.unwrap();
    assert_eq!(info.public_key, hex::encode(OWN_KEY));
    assert_eq!(info.name, "Ada radio");
    assert_eq!(info.settings.frequency_mhz, 869.618);
    assert_eq!(info.settings.bandwidth_khz, 62.5);
    assert_eq!(
        (info.settings.spreading_factor, info.settings.coding_rate),
        (8, 8)
    );
    assert_eq!(info.settings.tx_power_dbm, 22);
    assert_eq!(info.firmware.as_deref(), Some("v1.18.0"));
    assert_eq!(info.model.as_deref(), Some("Heltec V3"));

    let contacts = radio.contacts().await.unwrap();
    assert_eq!(contacts.len(), 1);
    assert_eq!(contacts[0].public_key, hex::encode(BEA_KEY));
    assert_eq!(contacts[0].name, "Bea");
    assert_eq!(contacts[0].kind, ContactKind::Chat);
    assert_eq!(contacts[0].last_heard, Some(1_795_299_000));
    assert!(contacts[0].has_path);

    // Sending: the radio tells which code its confirmation will carry.
    let sent = radio
        .send_text(&hex::encode(BEA_KEY), "Storm tonight")
        .await
        .unwrap();
    assert_eq!(
        sent,
        Sent {
            ack: hex::encode(EXPECTED_ACK),
            timeout: Duration::from_millis(3000),
        }
    );
    // SEND_CONFIRMED: the ACK code and the round trip in ms.
    fake.push([&[0x82u8][..], &EXPECTED_ACK, &1200u32.to_le_bytes()].concat())
        .await;
    assert_eq!(
        next_event(&mut events).await,
        RadioEvent::Delivered {
            ack: hex::encode(EXPECTED_ACK)
        }
    );

    // Receiving: the radio says a message waits, and the radio collects it.
    fake.waiting
        .lock()
        .unwrap()
        .push_back(message_from_bea("Coming at three"));
    fake.push(vec![0x83]).await; // MSG_WAITING
    assert_eq!(
        next_event(&mut events).await,
        RadioEvent::TextReceived {
            from_prefix: hex::encode(&BEA_KEY[..6]),
            from: Some(hex::encode(BEA_KEY)),
            text: "Coming at three".to_owned(),
            sent_at: 1_795_300_100_000,
            snr: Some(7.25),
        }
    );

    // ADVERT carries the whole key; the radio names the contact it knows.
    fake.push([&[0x80u8][..], &BEA_KEY].concat()).await;
    assert_eq!(
        next_event(&mut events).await,
        RadioEvent::ContactHeard {
            public_key: hex::encode(BEA_KEY)
        }
    );

    // What the radio would refuse is refused before it is sent.
    let stranger = hex::encode([0x51; 32]);
    assert!(matches!(
        radio.send_text(&stranger, "Hi").await,
        Err(RadioError::UnknownContact(_))
    ));
    let long = "x".repeat(MAX_TEXT_BYTES + 1);
    assert!(matches!(
        radio.send_text(&hex::encode(BEA_KEY), &long).await,
        Err(RadioError::TooLong { .. })
    ));

    radio.disconnect().await.unwrap();
    assert_eq!(next_event(&mut events).await, RadioEvent::Disconnected);
    assert!(matches!(radio.info().await, Err(RadioError::NotConnected)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn meshcore_radio_keeps_waiting_messages_for_its_first_listener() {
    let (fake, commands, dispatcher) = FakeRadio::start();
    fake.waiting
        .lock()
        .unwrap()
        .extend([message_from_bea("First"), message_from_bea("Second")]);
    let radio = MeshCoreRadio::over(commands, &dispatcher).await.unwrap();

    // Collecting takes messages off the radio, so nothing goes before someone listens.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(fake.waiting.lock().unwrap().len(), 2);

    let mut events = radio.events();
    for expected in ["First", "Second"] {
        match next_event(&mut events).await {
            RadioEvent::TextReceived { text, from, .. } => {
                assert_eq!(text, expected);
                assert_eq!(from, Some(hex::encode(BEA_KEY)));
            }
            other => panic!("expected a message, got {other:?}"),
        }
    }
    assert!(fake.waiting.lock().unwrap().is_empty());
}

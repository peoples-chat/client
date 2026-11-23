//! Two companion radios in memory, for the spike's tests. They answer the spike's commands with
//! frames laid out as firmware v1.18.0 sends them, and pass packets to each other as its source
//! says they do (`frames::expected`). They have no air: every packet arrives.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::Duration;

use meshcore_rs::commands::CommandHandler;
use meshcore_rs::events::EventDispatcher;
use meshcore_rs::reader::MessageReader;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::frames::{CHANNEL_DATA_RECV, MAX_CHANNEL_DATA, MAX_FRAME, MAX_TEXT};
use crate::link::Link;

/// Channel slots per radio.
const CHANNELS: u8 = 8;
/// 7.25 dB, as the radio reports it: times 4.
const SNR: u8 = 29;
const RSSI: u8 = -40i8 as u8;
/// What a radio answers when it takes a command.
const OK: [u8; 1] = [0];

pub struct FakeMesh {
    radios: [Arc<FakeRadio>; 2],
}

impl FakeMesh {
    pub fn new() -> Self {
        let radios = [
            FakeRadio::new([0xa1; 32], "Ada radio"),
            FakeRadio::new([0xb2; 32], "Bea radio"),
        ];
        let _ = radios[0].peer.set(Arc::downgrade(&radios[1]));
        let _ = radios[1].peer.set(Arc::downgrade(&radios[0]));
        Self { radios }
    }

    pub fn link_a(&self) -> Link {
        self.radios[0].link("A")
    }

    pub fn link_b(&self) -> Link {
        self.radios[1].link("B")
    }

    /// The name in a channel slot of radio 0 (A) or 1 (B).
    pub fn channel_slot(&self, radio: usize, index: u8) -> String {
        self.radios[radio]
            .state()
            .channels
            .get(&index)
            .map(|(name, _)| name.clone())
            .unwrap_or_default()
    }

    pub fn set_channel_slot(&self, radio: usize, index: u8, name: &str) {
        self.radios[radio]
            .state()
            .channels
            .insert(index, (name.to_owned(), [1; 16]));
    }

    /// The route radio 0 (A) or 1 (B) knows to the other, as hop hashes.
    pub fn route_to_peer(&self, radio: usize) -> Option<Vec<u8>> {
        self.radios[radio].state().route.clone()
    }
}

/// What one radio sends the other.
enum Air {
    Text {
        timestamp: [u8; 4],
        text: Vec<u8>,
        flood: bool,
        ack: [u8; 4],
    },
    Raw {
        payload: Vec<u8>,
    },
    Channel {
        secret: [u8; 16],
        flood: bool,
        data: Vec<u8>,
    },
}

struct FakeRadio {
    key: [u8; 32],
    name: &'static str,
    /// Where frames for the app go.
    app: OnceLock<Arc<MessageReader>>,
    peer: OnceLock<Weak<FakeRadio>>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    knows_peer: bool,
    /// The route to the peer, once known.
    route: Option<Vec<u8>>,
    channels: BTreeMap<u8, (String, [u8; 16])>,
    /// Frames waiting for the app to collect them.
    queue: VecDeque<Vec<u8>>,
    acks: u32,
}

impl FakeRadio {
    fn new(key: [u8; 32], name: &'static str) -> Arc<Self> {
        Arc::new(Self {
            key,
            name,
            app: OnceLock::new(),
            peer: OnceLock::new(),
            state: Mutex::new(State::default()),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn peer(&self) -> Arc<FakeRadio> {
        self.peer
            .get()
            .and_then(Weak::upgrade)
            .expect("the mesh has two radios")
    }

    /// The app's link to this radio.
    fn link(self: &Arc<Self>, label: &'static str) -> Link {
        let (to_radio, mut from_app) = mpsc::channel::<Vec<u8>>(64);
        let dispatcher = Arc::new(EventDispatcher::new());
        let reader = Arc::new(MessageReader::new(dispatcher.clone()));
        let commands = Arc::new(tokio::sync::Mutex::new(CommandHandler::new(
            to_radio,
            dispatcher.clone(),
            reader.clone(),
        )));
        let _ = self.app.set(reader);
        let radio = self.clone();
        tokio::spawn(async move {
            while let Some(mut command) = from_app.recv().await {
                // meshcore-rs listens for the answer only after it has written the command.
                tokio::time::sleep(Duration::from_millis(20)).await;
                // The radio's serial reader cuts a longer frame.
                command.truncate(MAX_FRAME);
                let (replies, air) = radio.answer(&command);
                for reply in replies {
                    radio.to_app(reply).await;
                }
                if let Some(air) = air {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    radio.peer().hear(&radio, air).await;
                }
            }
        });
        Link::over(label, commands, &dispatcher)
    }

    async fn to_app(&self, frame: Vec<u8>) {
        let app = self.app.get().expect("the radio has a link");
        app.handle_rx(frame)
            .await
            .expect("the reader takes any frame");
    }

    fn answer(&self, command: &[u8]) -> (Vec<Vec<u8>>, Option<Air>) {
        let peer = self.peer();
        let mut state = self.state();
        let refuse = |code: u8| (vec![vec![1, code]], None);
        match command[0] {
            1 => (vec![self.self_info()], None),
            22 => (vec![device_info()], None),
            12 => (vec![OK.to_vec()], None),
            4 => {
                let mut frames =
                    vec![[&[2u8][..], &u32::from(state.knows_peer).to_le_bytes()].concat()];
                if state.knows_peer {
                    frames.push(contact(&peer, state.route.as_deref()));
                }
                frames.push([&[4u8][..], &1_795_300_000u32.to_le_bytes()].concat());
                (frames, None)
            }
            // 9 | key | type | flags | path byte | path (64) | name (32) | timestamp | lat | lon
            9 if command[1..33] == peer.key => {
                state.knows_peer = true;
                state.route = route(command[35], &command[36..100]);
                (vec![OK.to_vec()], None)
            }
            13 if state.knows_peer && command[1..33] == peer.key => {
                state.route = None;
                (vec![OK.to_vec()], None)
            }
            31 if command[1] < CHANNELS => {
                let (name, secret) = state.channels.get(&command[1]).cloned().unwrap_or_default();
                let mut frame = vec![18, command[1]];
                frame.extend_from_slice(&padded(name.as_bytes(), 32));
                frame.extend_from_slice(&secret);
                (vec![frame], None)
            }
            32 if command[1] < CHANNELS && command.len() == 50 => {
                let name = String::from_utf8_lossy(&command[2..34])
                    .trim_end_matches('\0')
                    .to_owned();
                let secret: [u8; 16] = command[34..50].try_into().expect("16 bytes");
                state.channels.insert(command[1], (name, secret));
                (vec![OK.to_vec()], None)
            }
            10 => (
                vec![state.queue.pop_front().unwrap_or_else(|| vec![10])],
                None,
            ),
            // 2 | type | attempt | timestamp | the recipient's key prefix | text
            2 => {
                if !state.knows_peer || command[7..13] != peer.key[..6] {
                    return refuse(2);
                }
                let text = command[13..].to_vec();
                if text.len() > MAX_TEXT {
                    return refuse(3);
                }
                state.acks += 1;
                let ack = state.acks.to_le_bytes();
                let flood = state.route.is_none();
                let sent = [&[6, u8::from(flood)][..], &ack, &3000u32.to_le_bytes()].concat();
                let timestamp = command[3..7].try_into().expect("4 bytes");
                (
                    vec![sent],
                    Some(Air::Text {
                        timestamp,
                        text,
                        flood,
                        ack,
                    }),
                )
            }
            // 25 | path byte | path | payload
            25 => {
                if command[1] == 0xff {
                    return refuse(1);
                }
                let payload = command[2 + path_bytes(command[1])..].to_vec();
                if payload.len() < 4 {
                    return refuse(6);
                }
                (vec![OK.to_vec()], Some(Air::Raw { payload }))
            }
            // 62 | channel | path byte | path | data type | payload
            62 => {
                let Some((_, secret)) = state.channels.get(&command[1]).cloned() else {
                    return refuse(2);
                };
                let flood = command[2] == 0xff;
                let start = 3 + if flood { 0 } else { path_bytes(command[2]) };
                let data = command[start + 2..].to_vec();
                if command[start..start + 2] == [0u8, 0] || data.len() > MAX_CHANNEL_DATA {
                    return refuse(6);
                }
                (
                    vec![OK.to_vec()],
                    Some(Air::Channel {
                        secret,
                        flood,
                        data,
                    }),
                )
            }
            _ => refuse(1),
        }
    }

    /// Takes a packet `from` the other radio, as the firmware does.
    async fn hear(&self, from: &FakeRadio, air: Air) {
        match air {
            Air::Text {
                timestamp,
                text,
                flood,
                ack,
            } => {
                let sealed = (5 + text.len()).div_ceil(16) * 16;
                let payload =
                    [&[self.key[0], from.key[0], 0, 0][..], &vec![0; sealed][..]].concat();
                self.log_on_air(2, flood, &payload).await;
                // 16 | SNR | 0 | 0 | the sender's key prefix | path byte | type | timestamp | text
                let mut frame = vec![16, SNR, 0, 0];
                frame.extend_from_slice(&from.key[..6]);
                frame.extend_from_slice(&[if flood { 0 } else { 0xff }, 0]);
                frame.extend_from_slice(&timestamp);
                frame.extend_from_slice(&text);
                self.queue(frame).await;
                // The confirmation, and for a flood the route back.
                tokio::time::sleep(Duration::from_millis(50)).await;
                from.to_app([&[0x82][..], &ack, &120u32.to_le_bytes()].concat())
                    .await;
                if flood {
                    from.state().route = Some(Vec::new());
                    from.to_app([&[0x81][..], &self.key].concat()).await;
                }
            }
            Air::Raw { payload } => {
                self.log_on_air(0x0f, false, &payload).await;
                // The push holds 4 bytes and the payload, and a longer frame is dropped.
                if 4 + payload.len() <= MAX_FRAME {
                    self.to_app([&[0x84, SNR, RSSI, 0xff][..], &payload[..]].concat())
                        .await;
                }
            }
            Air::Channel {
                secret,
                flood,
                data,
            } => {
                let index = self
                    .state()
                    .channels
                    .iter()
                    .find(|(_, (_, held))| *held == secret)
                    .map(|(index, _)| *index);
                let Some(index) = index else {
                    return;
                };
                let sealed = (3 + data.len()).div_ceil(16) * 16;
                let hash = Sha256::digest(secret)[0];
                let payload = [&[hash, 0, 0][..], &vec![0; sealed][..]].concat();
                self.log_on_air(6, flood, &payload).await;
                let mut frame = vec![CHANNEL_DATA_RECV, SNR, 0, 0, index];
                frame.extend_from_slice(&[if flood { 0 } else { 0xff }, 0xff, 0xff]);
                frame.push(data.len() as u8);
                frame.extend_from_slice(&data);
                self.queue(frame).await;
            }
        }
    }

    /// The packet as the radio logs what it hears: header, path byte (no hops), payload.
    async fn log_on_air(&self, payload_type: u8, flood: bool, payload: &[u8]) {
        let header = (payload_type << 2) | if flood { 1 } else { 2 };
        let packet = [&[header, 0][..], payload].concat();
        if packet.len() + 3 <= MAX_FRAME {
            self.to_app([&[0x88, SNR, RSSI][..], &packet[..]].concat())
                .await;
        }
    }

    async fn queue(&self, frame: Vec<u8>) {
        self.state().queue.push_back(frame);
        self.to_app(vec![0x83]).await;
    }

    /// SELF_INFO on MeshCore's EU/UK narrow preset (869.618 MHz, 62.5 kHz, SF 8, CR 8).
    fn self_info(&self) -> Vec<u8> {
        let mut frame = vec![5, 1, 22, 22];
        frame.extend_from_slice(&self.key);
        frame.extend_from_slice(&[0; 8]); // lat, lon
        frame.extend_from_slice(&[0; 4]); // multi-acks, location policy, telemetry, manual add
        frame.extend_from_slice(&869_618u32.to_le_bytes()); // kHz
        frame.extend_from_slice(&62_500u32.to_le_bytes()); // Hz
        frame.extend_from_slice(&[8, 8]);
        frame.extend_from_slice(self.name.as_bytes());
        frame
    }
}

/// DEVICE_INFO: firmware code 14 (v1.18.0), with eight channel slots.
fn device_info() -> Vec<u8> {
    let mut frame = vec![13, 14, 50, CHANNELS];
    frame.extend_from_slice(&0u32.to_le_bytes()); // BLE PIN
    frame.extend_from_slice(&padded(b"15 Oct 2026", 12));
    frame.extend_from_slice(&padded(b"Fake radio", 40));
    frame.extend_from_slice(&padded(b"v1.18.0", 20));
    frame.extend_from_slice(&[0, 0]); // client repeat, path hash mode
    frame
}

/// CONTACT: `radio`, a person's radio, with its route.
fn contact(radio: &FakeRadio, route: Option<&[u8]>) -> Vec<u8> {
    let mut frame = vec![3];
    frame.extend_from_slice(&radio.key);
    frame.extend_from_slice(&[1, 0, route.map_or(0xff, |route| route.len() as u8)]);
    frame.extend_from_slice(&padded(route.unwrap_or_default(), 64));
    frame.extend_from_slice(&padded(radio.name.as_bytes(), 32));
    frame.extend_from_slice(&1_795_299_000u32.to_le_bytes()); // last advert
    frame.extend_from_slice(&[0; 8]); // lat, lon
    frame.extend_from_slice(&1_795_300_000u32.to_le_bytes()); // last change
    frame
}

/// The route a path byte and path describe; 0xFF for none.
fn route(path_byte: u8, path: &[u8]) -> Option<Vec<u8>> {
    (path_byte != 0xff).then(|| path[..path_bytes(path_byte)].to_vec())
}

/// How many bytes of hop hashes a path byte announces.
fn path_bytes(path_byte: u8) -> usize {
    usize::from(path_byte & 0x3f) * (usize::from(path_byte >> 6) + 1)
}

fn padded(bytes: &[u8], len: usize) -> Vec<u8> {
    let mut padded = bytes.to_vec();
    padded.resize(len, 0);
    padded
}

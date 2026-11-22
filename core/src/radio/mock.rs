//! Mock radios on a mock mesh: the [`Radio`] interface without hardware, for tests and for
//! interface work (#60).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::sync::broadcast;

use super::{
    ContactKind, EVENT_BUFFER, Mesh, Radio, RadioContact, RadioError, RadioEvent, RadioInfo,
    RadioSettings, Sent, check_text, key_prefix,
};

/// The settings a mock radio reports unless it is given others: MeshCore's EU/UK narrow preset.
pub const MOCK_SETTINGS: RadioSettings = RadioSettings {
    frequency_mhz: 869.618,
    bandwidth_khz: 62.5,
    spreading_factor: 8,
    coding_rate: 8,
    tx_power_dbm: 22,
};
/// How long a mock radio says a confirmation may take.
const ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// A mesh of mock radios. Every radio on it hears every other one in range, and holds the others
/// as contacts. Clones share one mesh.
#[derive(Clone, Default)]
pub struct MockMesh {
    state: Arc<Mutex<MeshState>>,
}

#[derive(Default)]
struct MeshState {
    radios: BTreeMap<String, Node>,
    /// Radios whose messages are lost, both ways.
    out_of_range: BTreeSet<String>,
    next_ack: u32,
}

struct Node {
    name: String,
    settings: RadioSettings,
    events: broadcast::Sender<RadioEvent>,
    connected: bool,
}

impl MockMesh {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a connected radio called `name`, with [`MOCK_SETTINGS`]. Its key is made from the
    /// name and its place on the mesh, so a test can predict nothing else about it.
    pub fn radio(&self, name: &str) -> MockRadio {
        self.radio_with(name, MOCK_SETTINGS)
    }

    /// Adds a connected radio called `name` that reports `settings`. The mock mesh carries its
    /// messages whatever the settings; telling which radios can hear each other is the core's
    /// job.
    pub fn radio_with(&self, name: &str, settings: RadioSettings) -> MockRadio {
        let mut state = self.state();
        let mut key = [0u8; 32];
        let number = u64::try_from(state.radios.len()).unwrap_or(u64::MAX) + 1;
        key[..8].copy_from_slice(&number.to_be_bytes());
        for (slot, byte) in key[8..].iter_mut().zip(name.bytes()) {
            *slot = byte;
        }
        let public_key = hex::encode(key);
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        state.radios.insert(
            public_key.clone(),
            Node {
                name: name.to_owned(),
                settings,
                events,
                connected: true,
            },
        );
        MockRadio {
            mesh: self.clone(),
            public_key,
        }
    }

    /// Takes the radio with `public_key` out of range, or brings it back. Messages to and from a
    /// radio out of range are sent but never arrive.
    pub fn set_in_range(&self, public_key: &str, in_range: bool) {
        let mut state = self.state();
        if in_range {
            state.out_of_range.remove(public_key);
        } else {
            state.out_of_range.insert(public_key.to_owned());
        }
    }

    fn state(&self) -> MutexGuard<'_, MeshState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A radio on a [`MockMesh`].
pub struct MockRadio {
    mesh: MockMesh,
    public_key: String,
}

impl MockRadio {
    /// The radio's public key, in hex.
    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    /// Makes the radio announce itself: every other connected radio hears it.
    pub fn advertise(&self) {
        let state = self.mesh.state();
        if state.out_of_range.contains(&self.public_key) {
            return;
        }
        for (key, node) in &state.radios {
            if key != &self.public_key && node.connected {
                let _ = node.events.send(RadioEvent::ContactHeard {
                    public_key: self.public_key.clone(),
                });
            }
        }
    }

    fn connected(&self, state: &MeshState) -> Result<(), RadioError> {
        match state.radios.get(&self.public_key) {
            Some(node) if node.connected => Ok(()),
            _ => Err(RadioError::NotConnected),
        }
    }
}

#[async_trait]
impl Radio for MockRadio {
    fn mesh(&self) -> Mesh {
        Mesh::MeshCore
    }

    async fn info(&self) -> Result<RadioInfo, RadioError> {
        let state = self.mesh.state();
        self.connected(&state)?;
        let node = &state.radios[&self.public_key];
        Ok(RadioInfo {
            mesh: Mesh::MeshCore,
            public_key: self.public_key.clone(),
            name: node.name.clone(),
            settings: node.settings,
            firmware: Some("mock".to_owned()),
            model: Some("Mock radio".to_owned()),
        })
    }

    async fn contacts(&self) -> Result<Vec<RadioContact>, RadioError> {
        let state = self.mesh.state();
        self.connected(&state)?;
        Ok(state
            .radios
            .iter()
            .filter(|(key, _)| **key != self.public_key)
            .map(|(key, node)| RadioContact {
                public_key: key.clone(),
                name: node.name.clone(),
                kind: ContactKind::Chat,
                last_heard: None,
                has_path: false,
            })
            .collect())
    }

    async fn send_text(&self, to: &str, text: &str) -> Result<Sent, RadioError> {
        check_text(text)?;
        let mut state = self.mesh.state();
        self.connected(&state)?;
        let Some(recipient) = state.radios.get(to) else {
            return Err(RadioError::UnknownContact(to.to_owned()));
        };
        let recipient_events = recipient.events.clone();
        let reaches = recipient.connected
            && !state.out_of_range.contains(to)
            && !state.out_of_range.contains(&self.public_key);
        state.next_ack += 1;
        let ack = format!("{:08x}", state.next_ack);

        if reaches {
            let _ = recipient_events.send(RadioEvent::TextReceived {
                from_prefix: key_prefix(&self.public_key),
                from: Some(self.public_key.clone()),
                text: text.to_owned(),
                sent_at: now_ms() / 1000 * 1000,
                snr: Some(9.5),
            });
            let _ = state.radios[&self.public_key]
                .events
                .send(RadioEvent::Delivered { ack: ack.clone() });
        }
        Ok(Sent {
            ack,
            timeout: ACK_TIMEOUT,
        })
    }

    fn events(&self) -> broadcast::Receiver<RadioEvent> {
        self.mesh.state().radios[&self.public_key]
            .events
            .subscribe()
    }

    async fn disconnect(&self) -> Result<(), RadioError> {
        let mut state = self.mesh.state();
        let node = state
            .radios
            .get_mut(&self.public_key)
            .ok_or(RadioError::NotConnected)?;
        if node.connected {
            node.connected = false;
            let _ = node.events.send(RadioEvent::Disconnected);
        }
        Ok(())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis().try_into().unwrap_or(u64::MAX))
}

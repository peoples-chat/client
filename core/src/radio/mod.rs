//! Radio transports (#60): the meshes the app talks to through a paired radio.
//!
//! Each mesh is a separate transport behind [`Radio`]. Radios of different meshes can't talk to
//! each other, and a radio runs one mesh at a time, so a user with two meshes pairs two radios.
//! MeshCore comes first ([`MeshCoreRadio`], through meshcore-rs); Meshtastic follows after the
//! MVP. [`MockMesh`] puts mock radios behind the same interface, for tests and for interface
//! work without hardware.
//!
//! Connecting is specific to a mesh and a link (USB serial, Bluetooth), so each implementation
//! has its own constructors; everything after that goes through [`Radio`]. [`RadioPorts`] finds
//! and opens radios plugged in over USB, for pairing (#24). Public keys are hex strings, as
//! elsewhere in the core. Details: `docs/radio.md`.

mod address;
pub mod airtime;
pub mod format;
mod inbox;
mod keys;
mod meshcore;
mod mock;
mod ports;

use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::broadcast;

pub use self::address::{AddressError, EXPECTED_PRESET, RADIO_ADDRESS_VERSION, RadioAddress};
pub use self::inbox::{KEPT_EPOCHS, REPLAY_WINDOW, RadioInbox, ReceivedText, Refusal};
pub use self::keys::{RADIO_KEY_BYTES, RADIO_KEY_LABEL, RadioKey, RadioKeyError, RadioKeys};
pub use self::meshcore::{MAX_TEXT_BYTES, MeshCoreRadio};
pub use self::mock::{MOCK_SETTINGS, MockMesh, MockPorts, MockRadio};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use self::ports::SerialPorts;
pub use self::ports::{Port, RadioPorts};

/// How many events a listener may fall behind before it misses some.
const EVENT_BUFFER: usize = 256;

/// A mesh network that a radio runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Mesh {
    MeshCore,
}

/// The radio's identity and settings, as it reports them.
#[derive(Clone, Debug, PartialEq)]
pub struct RadioInfo {
    pub mesh: Mesh,
    /// The radio's public key, in hex: its address on the mesh.
    pub public_key: String,
    /// The name the radio announces.
    pub name: String,
    pub settings: RadioSettings,
    /// The firmware version, such as `v1.18.0`, when the radio tells.
    pub firmware: Option<String>,
    /// The hardware model, when the radio tells.
    pub model: Option<String>,
}

/// LoRa settings. Radios hear each other only with the same frequency, bandwidth, spreading
/// factor and coding rate: together, the region and preset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RadioSettings {
    pub frequency_mhz: f64,
    pub bandwidth_khz: f64,
    pub spreading_factor: u8,
    pub coding_rate: u8,
    pub tx_power_dbm: u8,
}

/// What a contact on the mesh is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContactKind {
    /// A person's radio.
    Chat,
    Repeater,
    Room,
    Sensor,
    Other(u8),
}

/// A contact the radio knows. The radio holds only a working set; the phone holds the rest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RadioContact {
    /// The contact's public key, in hex.
    pub public_key: String,
    pub name: String,
    pub kind: ContactKind,
    /// When the contact last announced itself, by its own clock, in seconds since the Unix epoch.
    pub last_heard: Option<u64>,
    /// The radio knows a route to it. Without one, messages flood the mesh.
    pub has_path: bool,
}

/// A text message that the radio has sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sent {
    /// The code that [`RadioEvent::Delivered`] carries when the other radio confirms it, in hex.
    pub ack: String,
    /// How long the confirmation may take, as the radio estimates it.
    pub timeout: Duration,
}

/// Something the radio reports.
#[derive(Clone, Debug, PartialEq)]
pub enum RadioEvent {
    /// A contact sent a text message.
    TextReceived {
        /// The first six bytes of the sender's public key, in hex: what the mesh carries.
        from_prefix: String,
        /// The sender's full public key, when the radio knows the sender.
        from: Option<String>,
        text: String,
        /// When the sender sent it, by the sender's clock, in milliseconds since the Unix epoch.
        sent_at: u64,
        /// Signal-to-noise ratio of the last hop, in dB, when the radio tells.
        snr: Option<f32>,
    },
    /// The other radio confirmed a message: the [`Sent::ack`] it was sent with.
    Delivered { ack: String },
    /// The radio heard a contact announce itself.
    ContactHeard { public_key: String },
    /// The link to the radio ended.
    Disconnected,
}

/// Why a radio command failed.
#[derive(Debug, thiserror::Error)]
pub enum RadioError {
    #[error("the radio is not connected")]
    NotConnected,
    #[error("the radio doesn't know the contact {0}")]
    UnknownContact(String),
    #[error("the radio didn't answer in time")]
    Timeout,
    #[error("the radio refused: {0}")]
    Refused(String),
    #[error("the text is longer than the {max} bytes a radio message holds")]
    TooLong { max: usize },
    #[error("the text holds a character the radio can't send")]
    InvalidText,
    #[error("{0}")]
    Link(String),
}

/// A connected radio of one mesh.
#[async_trait]
pub trait Radio: Send + Sync {
    fn mesh(&self) -> Mesh;

    /// The radio's identity and settings.
    async fn info(&self) -> Result<RadioInfo, RadioError>;

    /// The contacts the radio holds.
    async fn contacts(&self) -> Result<Vec<RadioContact>, RadioError>;

    /// Sends `text` to the contact `to` (a public key in hex). Returns once the radio has sent
    /// it; [`RadioEvent::Delivered`] follows if the other radio confirms it.
    async fn send_text(&self, to: &str, text: &str) -> Result<Sent, RadioError>;

    /// Follows the radio's events from now on. Messages that waited on the radio for the app go
    /// to the first listener.
    fn events(&self) -> broadcast::Receiver<RadioEvent>;

    /// Ends the link to the radio. Emits [`RadioEvent::Disconnected`].
    async fn disconnect(&self) -> Result<(), RadioError>;
}

/// The first six bytes of a public key, in hex: how MeshCore names a message's sender.
pub(crate) fn key_prefix(public_key_hex: &str) -> String {
    public_key_hex.chars().take(12).collect()
}

/// Checks that `text` fits in one radio message.
pub(crate) fn check_text(text: &str) -> Result<(), RadioError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(RadioError::TooLong {
            max: MAX_TEXT_BYTES,
        });
    }
    // The radio ends the text at a NUL byte.
    if text.contains('\0') {
        return Err(RadioError::InvalidText);
    }
    Ok(())
}

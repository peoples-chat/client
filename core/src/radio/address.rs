//! The radio address record (#34): which radio a person has, on which mesh, with which LoRa
//! settings. Two contacts' apps exchange it inside their direct chat, as an app event of the
//! app's own kind, so that each knows where to send radio messages without anyone doing
//! anything. It travels only inside the encrypted chat, never in a group and never published.
//!
//! It is JSON, small and versioned:
//!
//! ```json
//! {"v":0,"mesh":"meshcore","key":"…64 hex…","freq":869618,"bw":62500,"sf":8,"cr":8,"at":1795300000000}
//! ```
//!
//! `freq` is in kHz and `bw` in Hz, so that no number has a fraction. `at` is when the record was
//! made, in milliseconds since the Unix epoch: a newer record replaces an older one. Fields a later
//! version adds are ignored here; a record of another version is refused, and the contact keeps
//! the record they had.

use serde::{Deserialize, Serialize};

use super::{Mesh, RadioInfo, RadioSettings};

/// The record's version.
pub const RADIO_ADDRESS_VERSION: u8 = 0;
/// The longest record taken, in bytes. A record is about 130.
const MAX_RECORD_BYTES: usize = 1024;

/// A radio, as its owner's app describes it to a contact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadioAddress {
    pub v: u8,
    /// The mesh the radio runs: `meshcore`.
    pub mesh: String,
    /// The radio's public key, in hex: its address on the mesh.
    pub key: String,
    /// Frequency, in kHz.
    pub freq: u32,
    /// Bandwidth, in Hz.
    pub bw: u32,
    /// Spreading factor.
    pub sf: u8,
    /// Coding rate, as the 5 of 4/5.
    pub cr: u8,
    /// When the record was made, in milliseconds since the Unix epoch.
    pub at: u64,
}

/// Why a record was refused.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum AddressError {
    #[error("the record is not JSON of the known shape: {0}")]
    Malformed(String),
    #[error("record version {0} is unknown")]
    UnknownVersion(u8),
    #[error("the mesh {0:?} is unknown")]
    UnknownMesh(String),
    #[error("the radio's key is not 64 hex digits")]
    InvalidKey,
    #[error("the LoRa settings are out of range")]
    InvalidSettings,
}

impl RadioAddress {
    /// The record for the radio that `info` describes, made at `at`.
    pub fn new(info: &RadioInfo, at: u64) -> Self {
        let settings = &info.settings;
        Self {
            v: RADIO_ADDRESS_VERSION,
            mesh: mesh_name(info.mesh).to_owned(),
            key: info.public_key.to_ascii_lowercase(),
            freq: (settings.frequency_mhz * 1000.0).round() as u32,
            bw: (settings.bandwidth_khz * 1000.0).round() as u32,
            sf: settings.spreading_factor,
            cr: settings.coding_rate,
            at,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a record always serializes")
    }

    /// Reads a record that came in a chat. Anything malformed or out of range is refused.
    pub fn parse(json: &str) -> Result<Self, AddressError> {
        if json.len() > MAX_RECORD_BYTES {
            return Err(AddressError::Malformed("too long".to_owned()));
        }
        // The version first, so that a later version isn't reported as malformed.
        #[derive(Deserialize)]
        struct Version {
            v: u8,
        }
        let version: Version = serde_json::from_str(json)
            .map_err(|error| AddressError::Malformed(error.to_string()))?;
        if version.v != RADIO_ADDRESS_VERSION {
            return Err(AddressError::UnknownVersion(version.v));
        }
        let mut record: Self = serde_json::from_str(json)
            .map_err(|error| AddressError::Malformed(error.to_string()))?;
        if record.mesh != mesh_name(Mesh::MeshCore) {
            return Err(AddressError::UnknownMesh(record.mesh));
        }
        if record.key.len() != 64 || !record.key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AddressError::InvalidKey);
        }
        record.key.make_ascii_lowercase();
        // LoRa runs from 137 MHz to 1020 MHz, at 7.8 kHz to 500 kHz, with SF 5 to 12 and CR 5 to 8.
        let settings_valid = (137_000..=1_020_000).contains(&record.freq)
            && (7_800..=500_000).contains(&record.bw)
            && (5..=12).contains(&record.sf)
            && (5..=8).contains(&record.cr);
        if !settings_valid {
            return Err(AddressError::InvalidSettings);
        }
        Ok(record)
    }

    /// The same radio with the same settings, whenever each record was made.
    pub fn same_radio(&self, other: &Self) -> bool {
        Self {
            at: 0,
            ..self.clone()
        } == Self {
            at: 0,
            ..other.clone()
        }
    }

    /// The mesh the radio runs.
    pub fn mesh(&self) -> Mesh {
        Mesh::MeshCore
    }

    /// The LoRa settings, as a radio reports them. Transmit power is the owner's own business,
    /// and not in the record.
    pub fn settings(&self) -> RadioSettings {
        RadioSettings {
            frequency_mhz: f64::from(self.freq) / 1000.0,
            bandwidth_khz: f64::from(self.bw) / 1000.0,
            spreading_factor: self.sf,
            coding_rate: self.cr,
            tx_power_dbm: 0,
        }
    }
}

fn mesh_name(mesh: Mesh) -> &'static str {
    match mesh {
        Mesh::MeshCore => "meshcore",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "7d1c4a3be0f2a9c85e6b1f40d3a29e7c1b8f05d64e2a97c3b51f80e6d4c2a913";

    fn info() -> RadioInfo {
        RadioInfo {
            mesh: Mesh::MeshCore,
            public_key: KEY.to_uppercase(),
            name: "Ada's radio".to_owned(),
            settings: RadioSettings {
                frequency_mhz: 869.618,
                bandwidth_khz: 62.5,
                spreading_factor: 8,
                coding_rate: 8,
                tx_power_dbm: 22,
            },
            firmware: None,
            model: None,
        }
    }

    #[test]
    fn a_record_is_small_and_reads_back() {
        let record = RadioAddress::new(&info(), 1_795_300_000_000);
        let json = record.to_json();
        assert_eq!(
            json,
            format!(
                r#"{{"v":0,"mesh":"meshcore","key":"{KEY}","freq":869618,"bw":62500,"sf":8,"cr":8,"at":1795300000000}}"#
            )
        );
        assert!(json.len() < 160, "{} bytes", json.len());
        assert_eq!(RadioAddress::parse(&json).unwrap(), record);
        assert_eq!(record.settings().frequency_mhz, 869.618);
        assert_eq!(record.settings().bandwidth_khz, 62.5);
    }

    #[test]
    fn the_same_radio_is_the_same_whenever_it_was_made() {
        let first = RadioAddress::new(&info(), 1);
        let later = RadioAddress::new(&info(), 2);
        assert!(first.same_radio(&later));
        let mut other = info();
        other.settings.spreading_factor = 7;
        assert!(!first.same_radio(&RadioAddress::new(&other, 1)));
    }

    #[test]
    fn takes_fields_it_doesnt_know_and_refuses_what_it_must() {
        let record = RadioAddress::new(&info(), 1_795_300_000_000);
        let json = record.to_json();
        // A field from a later version of the same shape.
        let more = json.replace(r#""at":"#, r#""name":"Ada","at":"#);
        assert_eq!(RadioAddress::parse(&more).unwrap(), record);

        let refused = [
            (
                json.replace(r#""v":0"#, r#""v":1"#),
                AddressError::UnknownVersion(1),
            ),
            (
                json.replace("meshcore", "meshtastic"),
                AddressError::UnknownMesh("meshtastic".to_owned()),
            ),
            (json.replace(KEY, "abc"), AddressError::InvalidKey),
            (json.replace(KEY, &"g".repeat(64)), AddressError::InvalidKey),
            (
                json.replace("869618", "2400000"),
                AddressError::InvalidSettings,
            ),
            (
                json.replace(r#""sf":8"#, r#""sf":13"#),
                AddressError::InvalidSettings,
            ),
        ];
        for (json, error) in refused {
            assert_eq!(RadioAddress::parse(&json).unwrap_err(), error, "{json}");
        }
        for json in ["", "not json", "{}", r#"{"v":0}"#, &"x".repeat(2000)] {
            assert!(
                matches!(RadioAddress::parse(json), Err(AddressError::Malformed(_))),
                "{json}"
            );
        }
    }
}

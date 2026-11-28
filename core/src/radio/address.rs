//! The radio address record (#34, #128): which radio a person has, on which mesh, with which
//! settings, and the key their app encrypts radio messages with. Each member's records live in
//! the direct chat's state, as an MLS app component in its GroupContext: nothing shows in either
//! chat, and they come with the chat to a new device. The decisions:
//! <https://github.com/peoples-chat/client/issues/128>.
//!
//! A member's component is a JSON array of [`RadioRecord`]s, version 1, at most one per mesh.
//! The account whose public key (hex) sorts first writes [`RADIO_COMPONENT_ID`], the other the
//! id after it ([`radio_component_id`]). A record is canonical JSON, with sorted members and no
//! spaces:
//!
//! ```json
//! {"addr":"…64 hex…","at":1795300000000,"bw":62500,"cr":8,"freq":869618,"mesh":"meshcore",
//!  "name":"Ada's radio","rk":"…64 hex…","rkn":1,"sf":8,"v":1}
//! ```
//!
//! `freq` is in kHz and `bw` in Hz, so that no number has a fraction.
//! `docs/radio-format-v0.md`, "Where the keys come from", has every field.
//!
//! Version 0 ([`RadioAddress`]) is the address and settings alone. Older builds send it as an app
//! event of kind 9911, which is still read. The core also keeps radios in that shape itself.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::{Zeroize, Zeroizing};

use super::keys::{RADIO_KEY_BYTES, RadioKey};
use super::{Mesh, RadioInfo, RadioSettings};

/// The version of the address alone, as older builds send it.
pub const RADIO_ADDRESS_VERSION: u8 = 0;
/// The version of the record kept in the chat's state.
pub const RADIO_RECORD_VERSION: u8 = 1;
/// The app component of the account whose public key sorts first; the other's is the next.
/// Provisional until registered (#145).
pub const RADIO_COMPONENT_ID: u16 = 0xf910;
/// The LoRa settings the app expects (#24): MeshCore's EU/UK (Narrow) preset, at 869.618 MHz,
/// 62.5 kHz, spreading factor 8 and coding rate 4/8. Frequency in kHz and bandwidth in Hz, as in
/// the record.
pub const EXPECTED_PRESET: (u32, u32, u8, u8) = (869_618, 62_500, 8, 8);
/// The longest version 0 record taken, in bytes. One is about 130.
const MAX_RECORD_BYTES: usize = 1024;
/// The most a component holds (MDK's limit).
const MAX_COMPONENT_BYTES: usize = 4096;
/// The most records taken from one component.
const MAX_RECORDS: usize = 8;
/// The longest radio name a record carries, in bytes. MeshCore's names are at most 32, and
/// Meshtastic's long names 39.
const MAX_NAME_BYTES: usize = 40;
/// The longest mesh, preset or region name taken, in bytes.
const MAX_LABEL_BYTES: usize = 32;

/// A radio and its settings: a version 0 record, and what a version 1 record says without its
/// key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadioAddress {
    pub v: u8,
    /// The mesh the radio runs: `meshcore` or `meshtastic`.
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

/// Why a version 0 record was refused.
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
    /// The radio that `info` describes, at `at`.
    pub fn new(info: &RadioInfo, at: u64) -> Self {
        let settings = &info.settings;
        Self {
            v: RADIO_ADDRESS_VERSION,
            mesh: info.mesh.name().to_owned(),
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

    /// Reads a version 0 record that came in a chat. Anything malformed or out of range is
    /// refused. Version 0 knows MeshCore alone.
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
        if record.mesh != Mesh::MeshCore.name() {
            return Err(AddressError::UnknownMesh(record.mesh));
        }
        if !is_hex_key(&record.key) {
            return Err(AddressError::InvalidKey);
        }
        record.key.make_ascii_lowercase();
        if !settings_valid(record.freq, record.bw, record.sf, record.cr) {
            return Err(AddressError::InvalidSettings);
        }
        Ok(record)
    }

    /// The region and preset: frequency in kHz, bandwidth in Hz, spreading factor and coding
    /// rate. Radios hear each other only when these are the same.
    pub fn preset(&self) -> (u32, u32, u8, u8) {
        (self.freq, self.bw, self.sf, self.cr)
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

fn is_hex_key(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Whether LoRa settings are in range: 137 MHz to 1020 MHz, at 7.8 kHz to 500 kHz, with SF 5 to
/// 12 and CR 5 to 8.
fn settings_valid(freq: u32, bw: u32, sf: u8, cr: u8) -> bool {
    (137_000..=1_020_000).contains(&freq)
        && (7_800..=500_000).contains(&bw)
        && (5..=12).contains(&sf)
        && (5..=8).contains(&cr)
}

/// The component that holds `account`'s records in its direct chat with `peer`.
pub fn radio_component_id(account: &str, peer: &str) -> u16 {
    if account < peer {
        RADIO_COMPONENT_ID
    } else {
        RADIO_COMPONENT_ID + 1
    }
}

/// A radio's address on its mesh.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MeshAddress {
    /// MeshCore: the radio's public key, 64 hex digits.
    MeshCore(String),
    /// Meshtastic: the node's number and public key (`NodeInfo.num` and `User.public_key` in
    /// its protobufs), the key as 64 hex digits.
    Meshtastic { key: String, num: u32 },
}

impl MeshAddress {
    /// The radio's public key, in hex.
    pub fn key(&self) -> &str {
        match self {
            Self::MeshCore(key) | Self::Meshtastic { key, .. } => key,
        }
    }
}

/// A member's radio on one mesh, and the key for what they send over it. Version 1.
///
/// The fields are in alphabetical order, so that the JSON is canonical. `Debug` leaves the key
/// out.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadioRecord {
    pub addr: MeshAddress,
    /// When the record was made, in milliseconds since the Unix epoch.
    pub at: u64,
    /// Bandwidth, in Hz.
    pub bw: u32,
    /// Coding rate, as the 5 of 4/5.
    pub cr: u8,
    /// Frequency, in kHz.
    pub freq: u32,
    /// `meshcore` or `meshtastic`.
    pub mesh: String,
    /// The radio's advertised name.
    pub name: String,
    /// The modem preset, on a mesh that names them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// The region, on a mesh that names them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// The app's radio key for this chat and mesh, 64 hex digits.
    pub rk: String,
    /// The key's number.
    pub rkn: u64,
    /// Spreading factor.
    pub sf: u8,
    pub v: u8,
}

impl std::fmt::Debug for RadioRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RadioRecord")
            .field("mesh", &self.mesh)
            .field("name", &self.name)
            .field("rkn", &self.rkn)
            .field("at", &self.at)
            .finish_non_exhaustive()
    }
}

impl Drop for RadioRecord {
    fn drop(&mut self) {
        self.rk.zeroize();
    }
}

impl RadioRecord {
    /// The record of the radio that `info` describes, with the key `key`, number `rkn`, made at
    /// `at`.
    pub fn new(info: &RadioInfo, key: &RadioKey, rkn: u64, at: u64) -> Self {
        let radio = RadioAddress::new(info, at);
        let meshtastic = info.meshtastic.as_ref();
        let mut end = info.name.len().min(MAX_NAME_BYTES);
        while !info.name.is_char_boundary(end) {
            end -= 1;
        }
        Self {
            addr: match meshtastic {
                Some(node) => MeshAddress::Meshtastic {
                    key: radio.key,
                    num: node.node_num,
                },
                None => MeshAddress::MeshCore(radio.key),
            },
            at,
            bw: radio.bw,
            cr: radio.cr,
            freq: radio.freq,
            mesh: radio.mesh,
            name: info.name[..end].to_owned(),
            preset: meshtastic.and_then(|node| node.preset.clone()),
            region: meshtastic.and_then(|node| node.region.clone()),
            rk: hex::encode(key.as_slice()),
            rkn,
            sf: radio.sf,
            v: RADIO_RECORD_VERSION,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a record always serializes")
    }

    /// Reads one record of a component. `None` for one the app can't use: another version, a
    /// mesh it doesn't know, or a field that is missing or out of range. Fields a later version
    /// adds are ignored.
    pub fn from_value(value: &Value) -> Option<Self> {
        let mut record = Self::deserialize(value).ok()?;
        let address_fits = matches!(
            (Mesh::from_name(&record.mesh)?, &record.addr),
            (Mesh::MeshCore, MeshAddress::MeshCore(_))
                | (Mesh::Meshtastic, MeshAddress::Meshtastic { .. })
        );
        let labels_fit = [&record.preset, &record.region]
            .into_iter()
            .flatten()
            .all(|label| label.len() <= MAX_LABEL_BYTES);
        if record.v != RADIO_RECORD_VERSION
            || !address_fits
            || !is_hex_key(record.addr.key())
            || !is_hex_key(&record.rk)
            || record.name.len() > MAX_NAME_BYTES
            || !labels_fit
            || !settings_valid(record.freq, record.bw, record.sf, record.cr)
        {
            return None;
        }
        match &mut record.addr {
            MeshAddress::MeshCore(key) | MeshAddress::Meshtastic { key, .. } => {
                key.make_ascii_lowercase();
            }
        }
        record.rk.make_ascii_lowercase();
        Some(record)
    }

    /// The key itself, if the record's is one.
    pub fn radio_key(&self) -> Option<RadioKey> {
        let mut key = Zeroizing::new([0; RADIO_KEY_BYTES]);
        hex::decode_to_slice(&self.rk, key.as_mut()).ok()?;
        Some(key)
    }

    /// The same radio with the same name and settings, whatever the key.
    pub fn same_radio(&self, other: &Self) -> bool {
        (&self.addr, &self.mesh, &self.name) == (&other.addr, &other.mesh, &other.name)
            && (&self.preset, &self.region) == (&other.preset, &other.region)
            && (self.freq, self.bw, self.sf, self.cr) == (other.freq, other.bw, other.sf, other.cr)
    }

    /// The radio and its settings, without the key.
    pub fn address(&self) -> RadioAddress {
        RadioAddress {
            v: RADIO_ADDRESS_VERSION,
            mesh: self.mesh.clone(),
            key: self.addr.key().to_owned(),
            freq: self.freq,
            bw: self.bw,
            sf: self.sf,
            cr: self.cr,
            at: self.at,
        }
    }
}

/// A member's component, as it is written: `records` as a JSON array.
pub fn encode_component<'a>(records: impl IntoIterator<Item = &'a RadioRecord>) -> Vec<u8> {
    let records: Vec<&RadioRecord> = records.into_iter().collect();
    serde_json::to_vec(&records).expect("records always serialize")
}

/// The records in a component as they came, those the app can't use among them: one per mesh,
/// the first of each. Nothing for data that isn't a list.
pub fn decode_component(data: &[u8]) -> Vec<Value> {
    if data.len() > MAX_COMPONENT_BYTES {
        return Vec::new();
    }
    let Ok(values) = serde_json::from_slice::<Vec<Value>>(data) else {
        return Vec::new();
    };
    let mut meshes: Vec<String> = Vec::new();
    values
        .into_iter()
        .filter(|value| {
            let Some(mesh) = value.get("mesh").and_then(Value::as_str) else {
                return false;
            };
            let new = mesh.len() <= MAX_LABEL_BYTES && !meshes.iter().any(|known| known == mesh);
            if new {
                meshes.push(mesh.to_owned());
            }
            new
        })
        .take(MAX_RECORDS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radio::MeshtasticInfo;

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
            meshtastic: None,
        }
    }

    fn meshtastic_info() -> RadioInfo {
        RadioInfo {
            mesh: Mesh::Meshtastic,
            meshtastic: Some(MeshtasticInfo {
                node_num: u32::MAX,
                preset: Some("LONG_FAST".to_owned()),
                region: Some("EU_868".to_owned()),
            }),
            ..info()
        }
    }

    fn radio_key() -> RadioKey {
        Zeroizing::new([7; RADIO_KEY_BYTES])
    }

    fn value(json: &str) -> Value {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn issue_128_a_version_1_record_is_canonical_small_and_reads_back() {
        let record = RadioRecord::new(&info(), &radio_key(), 3, 1_795_300_000_000);
        let json = record.to_json();
        let rk = "07".repeat(32);
        assert_eq!(
            json,
            format!(
                r#"{{"addr":"{KEY}","at":1795300000000,"bw":62500,"cr":8,"freq":869618,"mesh":"meshcore","name":"Ada's radio","rk":"{rk}","rkn":3,"sf":8,"v":1}}"#
            )
        );
        assert!(json.len() < 300, "{} bytes", json.len());
        assert_eq!(RadioRecord::from_value(&value(&json)).unwrap(), record);
        assert_eq!(*record.radio_key().unwrap(), *radio_key());
        assert!(!format!("{record:?}").contains(&rk));

        let meshtastic = RadioRecord::new(&meshtastic_info(), &radio_key(), 1, 1);
        let json = meshtastic.to_json();
        assert!(
            json.starts_with(&format!(
                r#"{{"addr":{{"key":"{KEY}","num":4294967295}},"at":1,"#
            )),
            "{json}"
        );
        assert!(json.contains(r#""preset":"LONG_FAST","region":"EU_868""#));
        // Meshtastic's two more address and setting fields take it a little further.
        assert!(json.len() < 340, "{} bytes", json.len());
        assert_eq!(RadioRecord::from_value(&value(&json)).unwrap(), meshtastic);
    }

    #[test]
    fn issue_128_a_component_keeps_what_it_cant_use() {
        let meshcore = RadioRecord::new(&info(), &radio_key(), 1, 1);
        let meshtastic = RadioRecord::new(&meshtastic_info(), &radio_key(), 1, 1);
        let data = encode_component([&meshcore, &meshtastic]);
        let read: Vec<RadioRecord> = decode_component(&data)
            .iter()
            .filter_map(RadioRecord::from_value)
            .collect();
        assert_eq!(read, vec![meshcore.clone(), meshtastic]);

        // A mesh nobody knows is kept and not used; a second record of a mesh is not kept.
        let future = r#"{"v":1,"mesh":"a-mesh-of-the-future","addr":[1,2]}"#;
        let json = meshcore.to_json();
        let kept = decode_component(format!("[{future},{json},{json},7]").as_bytes());
        assert_eq!(kept, vec![value(future), value(&json)]);
        assert_eq!(RadioRecord::from_value(&kept[0]), None);

        let rk = "07".repeat(32);
        for unusable in [
            json.replace(r#""v":1"#, r#""v":2"#),
            json.replace("meshcore", "meshtastic"),
            json.replace(KEY, "abc"),
            json.replace(&rk, "abc"),
            json.replace("869618", "2400000"),
            json.replace("Ada's radio", &"n".repeat(41)),
        ] {
            assert_eq!(
                RadioRecord::from_value(&value(&unusable)),
                None,
                "{unusable}"
            );
        }
        // A field from a later version of the same shape.
        let more = json.replace(r#""at":"#, r#""x":2,"at":"#);
        assert_eq!(RadioRecord::from_value(&value(&more)), Some(meshcore));
        let long = format!("[{}]", " ".repeat(5000));
        for data in ["", "{}", "not json", &long] {
            assert!(decode_component(data.as_bytes()).is_empty(), "{data}");
        }
    }

    #[test]
    fn issue_128_each_member_has_a_component_of_their_own() {
        assert_eq!(radio_component_id("aa", "bb"), 0xf910);
        assert_eq!(radio_component_id("bb", "aa"), 0xf911);
    }

    #[test]
    fn a_version_0_record_is_small_and_reads_back() {
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

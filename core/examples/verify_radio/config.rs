//! The lab as `radios.toml` describes it: the preset every radio must be on, and the radios.
//!
//! The harness only reads this file. Fields it doesn't need are ignored.

use std::path::Path;

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub lab: Preset,
    #[serde(rename = "radio", default)]
    pub radios: Vec<RadioEntry>,
}

/// What every radio in the lab must be set to before anything is sent.
#[derive(Clone, Debug, Deserialize)]
pub struct Preset {
    #[serde(default)]
    pub preset: String,
    pub frequency_mhz: f64,
    pub bandwidth_khz: f64,
    pub spreading_factor: u8,
    pub coding_rate: u8,
    pub max_tx_power_dbm: u8,
    /// Whether the Bluetooth run (#135) may have two radios announce themselves, so that they
    /// add each other as contacts. Off unless the file says so: it adds to what the radios
    /// store.
    #[serde(default)]
    pub exchange_adverts: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RadioEntry {
    pub label: String,
    /// A USB radio's serial number. Without one, the radio isn't on USB.
    pub usb_serial: Option<String>,
    /// A Bluetooth radio's advertised name.
    pub ble_name: Option<String>,
    /// The firmware version the radio must report.
    pub firmware: String,
    /// The radio's public key in hex, checked when given.
    pub public_key: Option<String>,
    /// Where `uhubctl` switches the radio's power: `<hub location>:<port>`. A value in angle
    /// brackets says why there is none.
    pub hub_port: Option<String>,
    /// Whether the Bluetooth run (#135) may restart this radio to drop its Bluetooth link. Off
    /// unless the file says so.
    #[serde(default)]
    pub reboot_to_drop_link: bool,
}

/// A port of a hub that `uhubctl` can switch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubPort {
    pub location: String,
    pub port: u8,
}

impl RadioEntry {
    /// Where the radio's power is switched, if anywhere.
    pub fn hub(&self) -> Result<Option<HubPort>, String> {
        let Some(text) = self.hub_port.as_deref().map(str::trim) else {
            return Ok(None);
        };
        if text.is_empty() || placeholder(text) {
            return Ok(None);
        }
        text.rsplit_once(':')
            .and_then(|(location, port)| {
                // uhubctl names hubs like `1-1.4`. A leading `-` would read as an option.
                let named = location.starts_with(|c: char| c.is_ascii_digit())
                    && location
                        .chars()
                        .all(|c| c.is_ascii_digit() || c == '-' || c == '.');
                let port = port.parse().ok().filter(|&port| port > 0)?;
                named.then(|| HubPort {
                    location: location.to_owned(),
                    port,
                })
            })
            .map(Some)
            .ok_or_else(|| {
                format!(
                    "{}: hub_port {text:?} isn't <hub location>:<port>, such as \"1-1.4:2\"",
                    self.label
                )
            })
    }
}

fn placeholder(text: &str) -> bool {
    text.starts_with('<')
}

/// Reads and checks the lab file.
pub fn load(path: &Path) -> Result<Config, String> {
    let text = std::fs::read_to_string(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => format!(
            "No radio lab file at {}. Set RADIOS_TOML to the lab's radios.toml.",
            path.display()
        ),
        _ => format!("Could not read {}: {error}", path.display()),
    })?;
    parse(&text).map_err(|error| format!("{}: {error}", path.display()))
}

pub fn parse(text: &str) -> Result<Config, String> {
    let config: Config = toml::from_str(text).map_err(|error| error.to_string())?;
    if !config.radios.iter().any(|radio| radio.usb_serial.is_some()) {
        return Err("no [[radio]] has a usb_serial, so there is no USB radio to test".to_owned());
    }
    let mut labels = Vec::new();
    let mut serials = Vec::new();
    for radio in &config.radios {
        if labels.contains(&&radio.label) {
            return Err(format!("two radios are labelled {}", radio.label));
        }
        labels.push(&radio.label);
        let Some(serial) = &radio.usb_serial else {
            // A Bluetooth radio is checked like the others before anything is sent, so its entry
            // must say what to expect.
            if let Some(name) = &radio.ble_name {
                for (field, value) in [("ble_name", name), ("firmware", &radio.firmware)] {
                    if value.trim().is_empty() || placeholder(value) {
                        return Err(format!("{}: {field} isn't filled in", radio.label));
                    }
                }
            }
            continue;
        };
        for (field, value) in [("usb_serial", serial), ("firmware", &radio.firmware)] {
            if value.trim().is_empty() || placeholder(value) {
                return Err(format!("{}: {field} isn't filled in", radio.label));
            }
        }
        let serial = serial.to_ascii_uppercase();
        if serials.contains(&serial) {
            return Err(format!("{}: another radio has its usb_serial", radio.label));
        }
        serials.push(serial);
        radio.hub()?;
    }
    Ok(config)
}

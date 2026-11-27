//! A run against the lab: find each USB radio by its serial number, check it against
//! `radios.toml`, and bring a radio that stops responding back by cycling its power.

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use peoples_chat_core::radio::{
    MeshCoreRadio, Port, Radio, RadioError, RadioInfo, RadioPorts, SerialPorts,
};
use tokio::time::Instant;
use tokio_serial::SerialPortBuilderExt;

use crate::config::{Config, Preset, RadioEntry};
use crate::link::{FrameLog, Io, Link};
use crate::power::Power;

/// MeshCore companion radios talk at this rate over USB serial.
const BAUD_RATE: u32 = 115_200;

/// The computer's USB ports.
#[async_trait]
pub trait Usb: Send + Sync {
    async fn list(&self) -> Result<Vec<Port>, String>;
    fn open(&self, path: &str) -> Result<Box<dyn Io>, String>;
}

pub struct Computer;

#[async_trait]
impl Usb for Computer {
    async fn list(&self) -> Result<Vec<Port>, String> {
        SerialPorts.list().await.map_err(|error| error.to_string())
    }

    fn open(&self, path: &str) -> Result<Box<dyn Io>, String> {
        let port = tokio_serial::new(path, BAUD_RATE)
            .open_native_async()
            .map_err(|error| error.to_string())?;
        Ok(Box::new(port))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// How long a radio may take to answer a command.
    pub reply: Duration,
    /// How long a radio may take to come back after its power was cycled.
    pub back: Duration,
    /// How often to look for it meanwhile.
    pub poll: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            reply: Duration::from_secs(5),
            back: Duration::from_secs(60),
            poll: Duration::from_secs(1),
        }
    }
}

/// Why a radio can't be used.
#[derive(Debug, PartialEq, Eq)]
pub enum Trouble {
    /// No USB port has its serial number. Power cycling may bring it back.
    Missing(String),
    /// It is there and doesn't answer. Power cycling may bring it back.
    Silent(String),
    /// Anything cycling its power wouldn't mend: a busy port, or the wrong firmware or settings.
    Wrong(String),
}

impl Trouble {
    fn stopped_responding(&self) -> bool {
        !matches!(self, Trouble::Wrong(_))
    }

    pub fn message(&self) -> &str {
        match self {
            Trouble::Missing(message) | Trouble::Silent(message) | Trouble::Wrong(message) => {
                message
            }
        }
    }
}

/// A radio that answered and matches the lab.
pub struct Station {
    pub entry: RadioEntry,
    pub port: String,
    pub info: RadioInfo,
    pub radio: MeshCoreRadio,
    pub link: Link,
}

impl std::fmt::Debug for Station {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} on {}", self.entry.label, self.port)
    }
}

impl Station {
    async fn close(self) {
        let _ = self.radio.disconnect().await;
        drop(self.radio);
        self.link.close().await;
    }
}

pub struct Lab<'a> {
    pub config: &'a Config,
    pub usb: Arc<dyn Usb>,
    pub power: &'a dyn Power,
    pub log: Arc<FrameLog>,
    pub timing: Timing,
}

impl Lab<'_> {
    /// Opens the radio with the entry's USB serial number and checks it, without touching its
    /// power.
    pub async fn connect(&self, entry: &RadioEntry) -> Result<Station, Trouble> {
        let label = &entry.label;
        let serial = entry.usb_serial.as_deref().unwrap_or_default();
        let ports =
            self.usb.list().await.map_err(|error| {
                Trouble::Wrong(format!("{label}: could not list ports: {error}"))
            })?;
        let port = find_port(&ports, serial).map_err(|found| match found {
            0 => Trouble::Missing(format!(
                "{label}: no USB port has serial number {serial}. Is it plugged in?"
            )),
            _ => Trouble::Wrong(format!(
                "{label}: {found} USB ports have serial number {serial}"
            )),
        })?;
        let stream = self.usb.open(&port).map_err(|error| {
            Trouble::Wrong(format!(
                "{label}: could not open {port}: {error}. Is another program using it?"
            ))
        })?;
        let link = Link::open(label, stream, self.log.clone(), self.timing.reply);
        let silent = |error: RadioError| {
            Trouble::Silent(format!(
                "{label}: the radio on {port} doesn't respond: {error}"
            ))
        };
        let radio = MeshCoreRadio::over(link.commands.clone(), &link.dispatcher)
            .await
            .map_err(silent)?;
        let info = radio.info().await.map_err(silent)?;
        let station = Station {
            entry: entry.clone(),
            port: port.clone(),
            info,
            radio,
            link,
        };
        let wrong = mismatches(&self.config.lab, entry, &station.info);
        if wrong.is_empty() {
            Ok(station)
        } else {
            let message = format!("{label}: {}", wrong.join("; "));
            station.close().await;
            Err(Trouble::Wrong(message))
        }
    }

    /// Like [`Lab::connect`], and cycles the radio's power once if it doesn't respond.
    pub async fn connect_or_recover(
        &self,
        entry: &RadioEntry,
        out: &mut (dyn Write + Send),
    ) -> Result<Station, String> {
        match self.connect(entry).await {
            Ok(station) => Ok(station),
            Err(trouble) if trouble.stopped_responding() => {
                let _ = writeln!(out, "{}", trouble.message());
                self.power_cycle(entry, out).await
            }
            Err(trouble) => Err(trouble.message().to_owned()),
        }
    }

    /// Cycles the radio's power, and waits until it is back and checked again.
    async fn power_cycle(
        &self,
        entry: &RadioEntry,
        out: &mut (dyn Write + Send),
    ) -> Result<Station, String> {
        let label = &entry.label;
        let Some(hub) = entry.hub()? else {
            return Err(format!(
                "{label} stopped responding, and radios.toml gives no hub_port for uhubctl. \
                 Replug or restart it by hand."
            ));
        };
        let _ = writeln!(
            out,
            "{label}: cycling its power (hub {}, port {})",
            hub.location, hub.port
        );
        self.power
            .cycle(&hub)
            .await
            .map_err(|error| format!("{label}: {error}"))?;
        let deadline = Instant::now() + self.timing.back;
        loop {
            tokio::time::sleep(self.timing.poll).await;
            match self.connect(entry).await {
                Ok(station) => {
                    let _ = writeln!(out, "{label}: back on {}", station.port);
                    return Ok(station);
                }
                // A port that isn't ready yet can refuse to open: keep looking. A radio that
                // answers with the wrong settings is reported when the time is up.
                Err(trouble) if Instant::now() >= deadline => {
                    return Err(format!(
                        "{label} didn't come back within {} s of cycling its power. {}",
                        self.timing.back.as_secs(),
                        trouble.message()
                    ));
                }
                Err(_) => {}
            }
        }
    }

    /// Asks a station for its contacts: the check that it still responds. A radio that doesn't
    /// is power-cycled, and asked again.
    pub async fn check_responding(
        &self,
        station: Station,
        out: &mut (dyn Write + Send),
    ) -> Result<(Station, usize), String> {
        let label = station.entry.label.clone();
        match station.radio.contacts().await {
            Ok(contacts) => Ok((station, contacts.len())),
            Err(error) => {
                let _ = writeln!(out, "{label}: stopped responding: {error}");
                let entry = station.entry.clone();
                station.close().await;
                let station = self.power_cycle(&entry, out).await?;
                let contacts = station
                    .radio
                    .contacts()
                    .await
                    .map_err(|error| format!("{label}: {error}"))?;
                Ok((station, contacts.len()))
            }
        }
    }

    /// The whole run. Every USB radio is checked before any test starts. A dry run lists what it
    /// finds, and never touches a radio's power.
    pub async fn run(&self, dry_run: bool, out: &mut (dyn Write + Send)) -> Result<(), String> {
        let preset = &self.config.lab;
        let _ = writeln!(
            out,
            "Lab preset {}: {} MHz, {} kHz, SF {}, CR {}, at most {} dBm",
            preset.preset,
            preset.frequency_mhz,
            preset.bandwidth_khz,
            preset.spreading_factor,
            preset.coding_rate,
            preset.max_tx_power_dbm
        );
        let mut stations = Vec::new();
        let mut problems = Vec::new();
        for entry in &self.config.radios {
            if entry.usb_serial.is_none() {
                let _ = writeln!(
                    out,
                    "{}: not on USB ({}), not checked",
                    entry.label,
                    entry.ble_name.as_deref().unwrap_or("no usb_serial")
                );
                continue;
            }
            let station = if dry_run {
                self.connect(entry)
                    .await
                    .map_err(|trouble| trouble.message().to_owned())
            } else {
                self.connect_or_recover(entry, out).await
            };
            match station {
                Ok(station) => {
                    let _ = writeln!(out, "{}", describe(&station));
                    stations.push(station);
                }
                Err(problem) => {
                    let _ = writeln!(out, "FAIL {problem}");
                    problems.push(problem);
                }
            }
        }
        if !problems.is_empty() {
            for station in stations {
                station.close().await;
            }
            return Err(format!(
                "The lab isn't ready, so nothing was run:\n  {}",
                problems.join("\n  ")
            ));
        }
        if dry_run {
            let _ = writeln!(
                out,
                "Dry run: {} USB radios match radios.toml.",
                stations.len()
            );
        } else {
            let mut checked = Vec::new();
            for station in stations {
                let (station, contacts) = self.check_responding(station, out).await?;
                let _ = writeln!(
                    out,
                    "ok {}: responds, {contacts} contacts",
                    station.entry.label
                );
                checked.push(station);
            }
            stations = checked;
        }
        for station in stations {
            station.close().await;
        }
        Ok(())
    }
}

/// The path of the one port with this USB serial number, or how many ports have it.
pub fn find_port(ports: &[Port], serial: &str) -> Result<String, usize> {
    let found: Vec<&Port> = ports
        .iter()
        .filter(|port| {
            port.serial_number
                .as_deref()
                .is_some_and(|found| found.eq_ignore_ascii_case(serial))
        })
        .collect();
    match found[..] {
        [port] => Ok(port.path.clone()),
        _ => Err(found.len()),
    }
}

/// How what a radio reports differs from what the lab file says it must be.
pub fn mismatches(preset: &Preset, entry: &RadioEntry, info: &RadioInfo) -> Vec<String> {
    let mut wrong = Vec::new();
    let firmware = info.firmware.as_deref().map(str::trim);
    if firmware != Some(entry.firmware.trim()) {
        wrong.push(format!(
            "firmware is {}, radios.toml says {}",
            firmware.unwrap_or("not reported"),
            entry.firmware
        ));
    }
    if let Some(key) = &entry.public_key
        && !key.eq_ignore_ascii_case(&info.public_key)
    {
        wrong.push(format!(
            "public key is {}, radios.toml says {key}",
            info.public_key
        ));
    }
    let settings = info.settings;
    // The radio reports whole kHz and Hz.
    if (settings.frequency_mhz - preset.frequency_mhz).abs() >= 0.0005 {
        wrong.push(format!(
            "frequency is {} MHz, the lab preset is {} MHz",
            settings.frequency_mhz, preset.frequency_mhz
        ));
    }
    if (settings.bandwidth_khz - preset.bandwidth_khz).abs() >= 0.0005 {
        wrong.push(format!(
            "bandwidth is {} kHz, the lab preset is {} kHz",
            settings.bandwidth_khz, preset.bandwidth_khz
        ));
    }
    if settings.spreading_factor != preset.spreading_factor {
        wrong.push(format!(
            "spreading factor is {}, the lab preset is {}",
            settings.spreading_factor, preset.spreading_factor
        ));
    }
    if settings.coding_rate != preset.coding_rate {
        wrong.push(format!(
            "coding rate is {}, the lab preset is {}",
            settings.coding_rate, preset.coding_rate
        ));
    }
    if settings.tx_power_dbm > preset.max_tx_power_dbm {
        wrong.push(format!(
            "transmit power is {} dBm, the lab allows at most {} dBm",
            settings.tx_power_dbm, preset.max_tx_power_dbm
        ));
    }
    wrong
}

fn describe(station: &Station) -> String {
    let info = &station.info;
    let settings = info.settings;
    format!(
        "{}: {} (USB {}), firmware {}, key {}…, {} MHz, {} kHz, SF {}, CR {}, {} dBm",
        station.entry.label,
        station.port,
        station.entry.usb_serial.as_deref().unwrap_or_default(),
        info.firmware.as_deref().unwrap_or("unknown"),
        info.public_key.get(..12).unwrap_or(&info.public_key),
        settings.frequency_mhz,
        settings.bandwidth_khz,
        settings.spreading_factor,
        settings.coding_rate,
        settings.tx_power_dbm
    )
}

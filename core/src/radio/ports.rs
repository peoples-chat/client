//! Where a radio is plugged in: USB serial ports on computers (#24).
//!
//! Pairing over a cable lists the ports, opens the radio on the one the user picks, and opens it
//! again after the radio restarts or is plugged in again. [`SerialPorts`] does that with the
//! computer's own ports; [`super::MockPorts`] with mock radios, for tests and interface work
//! without hardware. Phones reach radios over Bluetooth instead (#23).

// Phones have no serial ports: only the tests use the listing there.
#![cfg_attr(any(target_os = "android", target_os = "ios"), allow(dead_code))]

use std::sync::Arc;

use async_trait::async_trait;

use super::{Radio, RadioError};

/// A port a radio may be plugged into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Port {
    /// Where it is: `/dev/cu.usbmodem1101` on a Mac, `/dev/ttyACM0` on Linux, `COM3` on Windows.
    pub path: String,
    /// What the device on it says it is: its USB product name, or its maker.
    pub description: Option<String>,
    /// The USB device's serial number. It goes with the radio when it is plugged in elsewhere.
    pub serial_number: Option<String>,
}

/// The ports radios can be plugged into, and a way to open the radio on one.
#[async_trait]
pub trait RadioPorts: Send + Sync {
    /// The ports a radio may be on now.
    async fn list(&self) -> Result<Vec<Port>, RadioError>;

    /// Opens the radio on the port at `path`. Fails with [`RadioError::Timeout`] when the device
    /// there doesn't answer as a radio.
    async fn open(&self, path: &str) -> Result<Arc<dyn Radio>, RadioError>;
}

/// The computer's USB serial ports, with MeshCore companion radios on them.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[derive(Clone, Copy, Debug, Default)]
pub struct SerialPorts;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[async_trait]
impl RadioPorts for SerialPorts {
    async fn list(&self) -> Result<Vec<Port>, RadioError> {
        let found = tokio::task::spawn_blocking(tokio_serial::available_ports)
            .await
            .map_err(|error| RadioError::Link(error.to_string()))?
            .map_err(|error| RadioError::Link(error.to_string()))?;
        Ok(usb_ports(found.into_iter().map(found_port).collect()))
    }

    async fn open(&self, path: &str) -> Result<Arc<dyn Radio>, RadioError> {
        Ok(Arc::new(super::MeshCoreRadio::connect_serial(path).await?))
    }
}

/// A port as the system lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Found {
    path: String,
    /// What a USB device says about itself; `None` for a port that isn't USB, or of unknown
    /// kind.
    usb: Option<Usb>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Usb {
    product: Option<String>,
    manufacturer: Option<String>,
    serial_number: Option<String>,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn found_port(info: tokio_serial::SerialPortInfo) -> Found {
    let usb = match info.port_type {
        tokio_serial::SerialPortType::UsbPort(usb) => Some(Usb {
            product: usb.product,
            manufacturer: usb.manufacturer,
            serial_number: usb.serial_number,
        }),
        _ => None,
    };
    Found {
        path: info.port_name,
        usb,
    }
}

/// The ports a companion radio may be on: USB serial ports, sorted by path.
///
/// - A Mac lists each one twice, as `/dev/cu.…` and `/dev/tty.…`. The `tty` one waits for a
///   carrier when opened, so only the `cu` one is kept.
/// - Linux without udev can't tell what is behind a port, so its USB serial ports are known by
///   their names.
/// - Anything else, such as Bluetooth ports and debug consoles, is left out.
fn usb_ports(found: Vec<Found>) -> Vec<Port> {
    let paths: Vec<String> = found.iter().map(|port| port.path.clone()).collect();
    let mut ports: Vec<Port> = found
        .into_iter()
        .filter(|port| {
            port.path
                .strip_prefix("/dev/tty.")
                .is_none_or(|name| !paths.contains(&format!("/dev/cu.{name}")))
        })
        .filter_map(|port| match port.usb {
            Some(usb) => Some(Port {
                description: named(usb.product).or_else(|| named(usb.manufacturer)),
                serial_number: named(usb.serial_number),
                path: port.path,
            }),
            None if usb_name(&port.path) => Some(Port {
                path: port.path,
                description: None,
                serial_number: None,
            }),
            None => None,
        })
        .collect();
    ports.sort_by(|a, b| a.path.cmp(&b.path));
    ports
}

/// The names USB serial ports have where the system can't say more.
fn usb_name(path: &str) -> bool {
    ["ttyACM", "ttyUSB", "usbmodem", "usbserial"]
        .iter()
        .any(|name| path.contains(name))
}

fn named(text: Option<String>) -> Option<String> {
    text.map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usb(path: &str, product: Option<&str>, serial: Option<&str>) -> Found {
        Found {
            path: path.to_owned(),
            usb: Some(Usb {
                product: product.map(str::to_owned),
                manufacturer: Some("Seeed".to_owned()),
                serial_number: serial.map(str::to_owned),
            }),
        }
    }

    fn other(path: &str) -> Found {
        Found {
            path: path.to_owned(),
            usb: None,
        }
    }

    #[test]
    fn lists_usb_serial_ports_once_each() {
        let ports = usb_ports(vec![
            usb("/dev/tty.usbmodem1101", Some("T1000-E"), Some("A1B2")),
            other("/dev/cu.Bluetooth-Incoming-Port"),
            usb("/dev/cu.usbmodem1101", Some("T1000-E"), Some("A1B2")),
            usb("/dev/cu.usbserial-0001", Some("  "), None),
            other("/dev/ttyACM0"),
            other("/dev/ttyS0"),
        ]);
        assert_eq!(
            ports,
            vec![
                Port {
                    path: "/dev/cu.usbmodem1101".to_owned(),
                    description: Some("T1000-E".to_owned()),
                    serial_number: Some("A1B2".to_owned()),
                },
                // No product name: the maker's.
                Port {
                    path: "/dev/cu.usbserial-0001".to_owned(),
                    description: Some("Seeed".to_owned()),
                    serial_number: None,
                },
                // Linux without udev: known by its name.
                Port {
                    path: "/dev/ttyACM0".to_owned(),
                    description: None,
                    serial_number: None,
                },
            ]
        );
    }

    #[test]
    fn keeps_a_tty_port_that_has_no_cu_twin() {
        let ports = usb_ports(vec![usb("/dev/tty.usbmodem2101", None, None)]);
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].path, "/dev/tty.usbmodem2101");
    }
}

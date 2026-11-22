//! Prints what a MeshCore companion radio plugged in over USB says about itself (#60): its
//! public key, name, radio settings, firmware and contacts.
//!
//! ```sh
//! cargo run -p peoples-chat-core --example radio_info -- /dev/cu.usbmodem1101
//! ```
//!
//! Without a port, it lists the serial ports it sees. It only reads from the radio.

use peoples_chat_core::radio::{MeshCoreRadio, Radio, RadioError};

#[tokio::main]
async fn main() {
    let Some(port) = std::env::args().nth(1) else {
        list_ports();
        return;
    };
    if let Err(error) = describe(&port).await {
        eprintln!("{port}: {error}");
        if matches!(error, RadioError::Timeout) {
            eprintln!("Is it a MeshCore radio with companion firmware for USB?");
        }
        std::process::exit(1);
    }
}

async fn describe(port: &str) -> Result<(), RadioError> {
    let radio = MeshCoreRadio::connect_serial(port).await?;
    let info = radio.info().await?;
    let settings = info.settings;
    println!("Name:        {}", info.name);
    println!("Public key:  {}", info.public_key);
    println!(
        "Firmware:    {} ({})",
        info.firmware.as_deref().unwrap_or("unknown"),
        info.model.as_deref().unwrap_or("unknown model")
    );
    println!(
        "Radio:       {} MHz, {} kHz bandwidth, spreading factor {}, coding rate {}, {} dBm",
        settings.frequency_mhz,
        settings.bandwidth_khz,
        settings.spreading_factor,
        settings.coding_rate,
        settings.tx_power_dbm
    );
    let contacts = radio.contacts().await?;
    println!("Contacts:    {}", contacts.len());
    for contact in contacts {
        println!(
            "  {} {:?} {}",
            &contact.public_key[..12],
            contact.kind,
            contact.name
        );
    }
    radio.disconnect().await
}

fn list_ports() {
    match tokio_serial::available_ports() {
        Ok(ports) if !ports.is_empty() => {
            println!("Serial ports:");
            for port in ports {
                let detail = match port.port_type {
                    tokio_serial::SerialPortType::UsbPort(usb) => format!(
                        "USB {:04x}:{:04x} {}",
                        usb.vid,
                        usb.pid,
                        usb.product.unwrap_or_default()
                    ),
                    _ => String::new(),
                };
                println!("  {} {detail}", port.port_name);
            }
            println!("Run again with the radio's port.");
        }
        Ok(_) => println!("No serial ports found. Plug in a MeshCore radio over USB."),
        Err(error) => eprintln!("Could not list serial ports: {error}"),
    }
}

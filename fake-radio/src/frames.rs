//! The companion protocol's frames, laid out as MeshCore firmware v1.18.0 sends them
//! (`examples/companion_radio/MyMesh.cpp`), and the framing they travel in over a stream.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::Contact;

// Commands, app to radio.
pub const CMD_APP_START: u8 = 1;
pub const CMD_SEND_TXT_MSG: u8 = 2;
pub const CMD_GET_CONTACTS: u8 = 4;
pub const CMD_SEND_SELF_ADVERT: u8 = 7;
pub const CMD_SYNC_NEXT_MESSAGE: u8 = 10;
pub const CMD_REBOOT: u8 = 19;
pub const CMD_GET_BATT_AND_STORAGE: u8 = 20;
pub const CMD_DEVICE_QUERY: u8 = 22;

// Error codes.
pub const ERR_UNSUPPORTED: u8 = 1;
pub const ERR_NOT_FOUND: u8 = 2;
pub const ERR_ILLEGAL_ARG: u8 = 6;

/// The most a frame from the app may hold. Real frames stay under 200 bytes.
const MAX_FRAME: usize = 1024;

/// Reads one frame the app sent: `<`, the length as a little-endian u16, the payload.
pub async fn read(stream: &mut (impl AsyncRead + Unpin)) -> io::Result<Vec<u8>> {
    // Bytes before a frame's start are skipped, as the firmware does.
    while stream.read_u8().await? != b'<' {}
    let len = usize::from(stream.read_u16_le().await?);
    if len == 0 || len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad frame length",
        ));
    }
    let mut payload = vec![0; len];
    stream.read_exact(&mut payload).await?;
    Ok(payload)
}

/// Writes one frame to the app: `>`, the length as a little-endian u16, the payload.
pub async fn write(stream: &mut (impl AsyncWrite + Unpin), payload: &[u8]) -> io::Result<()> {
    let len = u16::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame too long"))?;
    let mut frame = vec![b'>'];
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(payload);
    stream.write_all(&frame).await
}

pub fn ok() -> Vec<u8> {
    vec![0]
}

pub fn error(code: u8) -> Vec<u8> {
    vec![1, code]
}

/// BATT_AND_STORAGE: 4.1 V, and 120 of 1024 kB of storage used.
pub fn battery() -> Vec<u8> {
    let mut frame = vec![12];
    frame.extend_from_slice(&4100u16.to_le_bytes());
    frame.extend_from_slice(&120u32.to_le_bytes());
    frame.extend_from_slice(&1024u32.to_le_bytes());
    frame
}

pub fn contacts_start(count: usize) -> Vec<u8> {
    [&[2u8][..], &(count as u32).to_le_bytes()].concat()
}

pub fn contacts_end(last_modified: u32) -> Vec<u8> {
    [&[4u8][..], &last_modified.to_le_bytes()].concat()
}

pub fn contact(contact: &Contact, last_modified: u32) -> Vec<u8> {
    let mut frame = vec![3];
    frame.extend_from_slice(&contact.public_key);
    // Type, flags, and a route of length 0: the contact is in direct range.
    frame.extend_from_slice(&[contact.kind, 0, 0]);
    frame.extend_from_slice(&[0; 64]);
    frame.extend_from_slice(&padded(contact.name.as_bytes(), 32));
    frame.extend_from_slice(&contact.last_advert.to_le_bytes());
    frame.extend_from_slice(&[0; 8]); // latitude, longitude
    frame.extend_from_slice(&last_modified.to_le_bytes());
    frame
}

/// SELF_INFO on MeshCore's EU/UK narrow preset: 869.618 MHz, 62.5 kHz, SF 8, CR 8.
pub fn self_info(public_key: &[u8; 32], name: &str) -> Vec<u8> {
    let mut frame = vec![5, 1, 22, 22]; // chat node, TX power, its maximum
    frame.extend_from_slice(public_key);
    frame.extend_from_slice(&[0; 8]); // latitude, longitude
    frame.extend_from_slice(&[0; 4]); // multi ACKs, location policy, telemetry, manual add
    frame.extend_from_slice(&869_618u32.to_le_bytes()); // kHz
    frame.extend_from_slice(&62_500u32.to_le_bytes()); // Hz
    frame.extend_from_slice(&[8, 8]);
    frame.extend_from_slice(name.as_bytes());
    frame
}

/// DEVICE_INFO: firmware code 14 (v1.18.0). The table's size travels halved.
pub fn device_info(max_contacts: usize) -> Vec<u8> {
    let halved = u8::try_from(max_contacts / 2).unwrap_or(u8::MAX);
    let mut frame = vec![13, 14, halved, 40];
    frame.extend_from_slice(&[0; 4]); // BLE PIN
    frame.extend_from_slice(&padded(b"15 Oct 2026", 12));
    frame.extend_from_slice(&padded(b"Fake radio", 40));
    frame.extend_from_slice(&padded(b"v1.18.0", 20));
    frame.extend_from_slice(&[0, 0]); // client repeat, path hash mode
    frame
}

/// SENT: by a direct route, the code the confirmation will carry, and how long that may take.
pub fn sent(ack: [u8; 4], timeout_ms: u32) -> Vec<u8> {
    [&[6u8, 0][..], &ack, &timeout_ms.to_le_bytes()].concat()
}

/// A received direct message, in the format with the signal strength (CONTACT_MSG_RECV_V3).
pub fn message(from: &[u8; 32], sent_at: u32, text: &[u8]) -> Vec<u8> {
    let mut frame = vec![16, 29, 0, 0]; // SNR 7.25 dB in quarters, two reserved bytes
    frame.extend_from_slice(&from[..6]);
    frame.extend_from_slice(&[0xff, 0]); // arrived direct, plain text
    frame.extend_from_slice(&sent_at.to_le_bytes());
    frame.extend_from_slice(text);
    frame
}

pub fn no_more_messages() -> Vec<u8> {
    vec![10]
}

/// SEND_CONFIRMED push: the ACK code and the round trip.
pub fn send_confirmed(ack: [u8; 4], round_trip_ms: u32) -> Vec<u8> {
    [&[0x82u8][..], &ack, &round_trip_ms.to_le_bytes()].concat()
}

/// MSG_WAITING push.
pub fn message_waiting() -> Vec<u8> {
    vec![0x83]
}

fn padded(bytes: &[u8], len: usize) -> Vec<u8> {
    let mut padded = bytes.to_vec();
    // The last byte stays zero, so a name that is too long still ends.
    padded.truncate(len - 1);
    padded.resize(len, 0);
    padded
}

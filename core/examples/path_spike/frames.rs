//! The companion protocol as the spike speaks it, laid out as MeshCore companion firmware
//! v1.18.0 does: `examples/companion_radio/MyMesh.cpp`, with paths from `src/Packet.cpp` and the
//! limits from `src/MeshCore.h`, `src/helpers/BaseChatMesh.h` and
//! `src/helpers/BaseSerialInterface.h`.
//!
//! meshcore-rs 0.2.0 can't send raw data or channel data, can't read channel data, and reports
//! the radio's refusals only as timeouts, so the spike builds these frames itself and sends them
//! through meshcore-rs's `send_multi`. Details: `docs/radio-paths.md`.

use meshcore_rs::events::LogData;

use crate::airtime::meshcore_text_bytes;

/// `CMD_SEND_TXT_MSG`.
const SEND_TXT_MSG: u8 = 2;
/// `CMD_SYNC_NEXT_MESSAGE`.
pub const SYNC_NEXT_MESSAGE: u8 = 10;
/// `CMD_RESET_PATH`.
const RESET_PATH: u8 = 13;
/// `CMD_SEND_RAW_DATA`.
const SEND_RAW_DATA: u8 = 25;
/// `CMD_SEND_CHANNEL_DATA`.
const SEND_CHANNEL_DATA: u8 = 62;
/// `RESP_CODE_CHANNEL_DATA_RECV`: a channel datagram collected from the radio's queue.
pub const CHANNEL_DATA_RECV: u8 = 27;
/// `DATA_TYPE_DEV`: the data type the firmware keeps for experiments.
pub const DATA_TYPE_DEV: u16 = 0xFFFF;
/// `ERR_CODE_UNSUPPORTED_CMD`, also the answer to raw data with no path.
pub const UNSUPPORTED_CMD: u8 = 1;
/// `ERR_CODE_TABLE_FULL`, also the answer to text over [`MAX_TEXT`] bytes.
pub const TABLE_FULL: u8 = 3;
/// `ERR_CODE_ILLEGAL_ARG`.
pub const ILLEGAL_ARG: u8 = 6;

/// The path byte of a flood, and of a direct packet in what a radio reports (`OUT_PATH_UNKNOWN`).
const NO_PATH: u8 = 0xFF;
/// The most a path holds, in bytes (`MAX_PATH_SIZE`).
const MAX_PATH_BYTES: usize = 64;
/// The most hops a path byte can count: its low 6 bits.
const MAX_HOPS: usize = 63;

/// The longest frame either way over the serial link (`MAX_FRAME_SIZE`). The radio cuts a longer
/// command frame to this, and drops a longer reply or push.
pub const MAX_FRAME: usize = 176;
/// The longest text in a text message (`MAX_TEXT_LEN`).
pub const MAX_TEXT: usize = 160;
/// The longest channel datagram (`MAX_GROUP_DATA_LENGTH`).
pub const MAX_CHANNEL_DATA: usize = 165;
/// The shortest raw data payload the radio takes.
const MIN_RAW_DATA: usize = 4;
/// The bytes in front of a raw payload in the receiving radio's push: code, SNR, RSSI, reserved.
const RAW_DATA_PUSH_HEADER: usize = 4;
/// The bytes in front of the text in a `SEND_TXT_MSG` frame.
const TEXT_FRAME_HEADER: usize = 13;
/// The bytes in front of the path in a `SEND_RAW_DATA` frame.
const RAW_FRAME_HEADER: usize = 2;
/// The bytes in a `SEND_CHANNEL_DATA` frame besides the path and the payload.
const CHANNEL_FRAME_HEADER: usize = 5;

/// The three candidate paths for the app's packets between two companion radios.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// A text message to a contact, carrying the packet as base64: what format v0 uses.
    Text,
    /// `SEND_RAW_DATA`: the packet's bytes as they are.
    Raw,
    /// `SEND_CHANNEL_DATA` on a private channel that only the two test radios hold.
    Channel,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Text, Kind::Raw, Kind::Channel];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Raw => "raw",
            Kind::Channel => "channel",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

/// How a packet travels.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Route {
    /// Every repeater in range passes it on.
    Flood,
    /// Along a path of repeaters, named by the first `hash_size` bytes of their keys. With no
    /// hops it reaches only the radios in range: zero hop.
    Direct { hash_size: u8, hops: Vec<u8> },
}

impl Route {
    pub fn zero_hop() -> Self {
        Self::Direct {
            hash_size: 1,
            hops: Vec::new(),
        }
    }

    /// Reads `--via`: the repeaters to go through, as their hashes in hex, `hash_size` bytes each.
    pub fn via(hex: &str, hash_size: u8) -> Result<Self, String> {
        if !(1..=3).contains(&hash_size) {
            return Err(format!("a hop's hash is 1 to 3 bytes, not {hash_size}"));
        }
        let hops = hex::decode(hex).map_err(|_| format!("{hex:?} is not hex"))?;
        let size = usize::from(hash_size);
        if hops.is_empty() || hops.len() % size != 0 {
            return Err(format!(
                "{hex:?} is not a whole number of {size}-byte hashes"
            ));
        }
        if hops.len() > MAX_PATH_BYTES || hops.len() / size > MAX_HOPS {
            return Err(format!("{hex:?} is longer than a path can be"));
        }
        Ok(Self::Direct { hash_size, hops })
    }

    /// The path byte: the hash size less one in the top 2 bits, the number of hops in the rest.
    pub fn path_byte(&self) -> u8 {
        match self {
            Self::Flood => NO_PATH,
            Self::Direct { hash_size, hops } => {
                let count = hops.len() / usize::from(*hash_size);
                ((hash_size - 1) << 6) | count as u8
            }
        }
    }

    /// The hashes of the hops, one after another.
    pub fn path(&self) -> &[u8] {
        match self {
            Self::Flood => &[],
            Self::Direct { hops, .. } => hops,
        }
    }

    pub fn is_flood(&self) -> bool {
        *self == Self::Flood
    }

    pub fn label(&self) -> String {
        match self {
            Self::Flood => "flood".to_owned(),
            Self::Direct { hops, .. } if hops.is_empty() => "zero hop".to_owned(),
            Self::Direct { hops, .. } => format!("via {}", hex::encode(hops)),
        }
    }
}

/// `SEND_TXT_MSG`: a plain text message, first attempt, to the contact whose key starts with
/// `prefix`. The radio sends it along the contact's route, or floods it when it knows none.
pub fn text_message(prefix: &[u8; 6], timestamp: u32, text: &[u8]) -> Vec<u8> {
    // 2 | text type: plain | attempt | timestamp (LE) | the key's first 6 bytes | text
    let mut frame = vec![SEND_TXT_MSG, 0, 0];
    frame.extend_from_slice(&timestamp.to_le_bytes());
    frame.extend_from_slice(prefix);
    frame.extend_from_slice(text);
    frame
}

/// `SEND_RAW_DATA`: `payload` as it is, along `route`. The radio refuses a flood.
pub fn raw_data(route: &Route, payload: &[u8]) -> Vec<u8> {
    // 25 | path byte | path | payload
    let mut frame = vec![SEND_RAW_DATA, route.path_byte()];
    frame.extend_from_slice(route.path());
    frame.extend_from_slice(payload);
    frame
}

/// `SEND_CHANNEL_DATA`: `payload` on channel `index`, with the experiments' data type.
pub fn channel_data(index: u8, route: &Route, payload: &[u8]) -> Vec<u8> {
    // 62 | channel | path byte | path (none for a flood) | data type (LE) | payload
    let mut frame = vec![SEND_CHANNEL_DATA, index, route.path_byte()];
    frame.extend_from_slice(route.path());
    frame.extend_from_slice(&DATA_TYPE_DEV.to_le_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// `RESET_PATH`: forget the route to a contact, so that the next text message floods.
pub fn reset_path(public_key: &[u8; 32]) -> Vec<u8> {
    [&[RESET_PATH][..], public_key].concat()
}

/// A channel datagram collected from the radio, which meshcore-rs 0.2.0 passes on as an unknown
/// frame: `27 | SNR×4 | 0 | 0 | channel | path byte | data type (LE) | length | data`.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelDatagram {
    pub snr: f32,
    pub channel: u8,
    pub path_byte: u8,
    pub data_type: u16,
    pub data: Vec<u8>,
}

impl ChannelDatagram {
    pub fn parse(frame: &[u8]) -> Option<Self> {
        if frame.len() < 9 || frame[0] != CHANNEL_DATA_RECV {
            return None;
        }
        let data = frame.get(9..9 + usize::from(frame[8]))?.to_vec();
        Some(Self {
            snr: f32::from(frame[1] as i8) / 4.0,
            channel: frame[4],
            path_byte: frame[5],
            data_type: u16::from_le_bytes([frame[6], frame[7]]),
            data,
        })
    }
}

/// The code in the radio's ERR frame. meshcore-rs passes the frame's bytes after the ERR code on
/// as lossy UTF-8, so a frame `1 | code` arrives as a string of that one byte.
pub fn error_code(message: &str) -> Option<u8> {
    (message.len() == 1).then(|| message.as_bytes()[0])
}

/// The firmware's name for a refusal code (`ERR_CODE_*`).
pub fn error_name(code: u8) -> &'static str {
    match code {
        1 => "UNSUPPORTED_CMD",
        2 => "NOT_FOUND",
        3 => "TABLE_FULL",
        4 => "BAD_STATE",
        5 => "FILE_IO_ERROR",
        6 => "ILLEGAL_ARG",
        _ => "an unknown code",
    }
}

/// How a received packet came, from the path byte the receiving radio reports with it: 0xFF for
/// a direct packet, otherwise the hops a flood took.
pub fn received_route(path_byte: u8) -> String {
    if path_byte == NO_PATH {
        "direct".to_owned()
    } else {
        format!("flood, {} hops", path_byte & 0x3F)
    }
}

/// The bytes a packet takes on air: header, path byte, path and payload. A radio with a flood
/// scope adds 4 bytes of transport codes to its floods; this assumes none.
pub fn on_air_bytes(kind: Kind, route: &Route, payload: usize) -> usize {
    let path = route.path().len();
    match kind {
        // The two hashes, the MAC, and the encrypted timestamp, flags and text in 16-byte blocks.
        Kind::Text => meshcore_text_bytes(payload) + path,
        Kind::Raw => 2 + path + payload,
        // The channel's hash, the MAC, and the encrypted data type, length and data in blocks.
        Kind::Channel => 2 + path + 1 + 2 + (3 + payload).div_ceil(16) * 16,
    }
}

/// The length of a packet as the receiving radio logged it on air.
pub fn logged_bytes(log: &LogData) -> Option<usize> {
    let header = log.header.as_ref()?;
    let transport = if header.transport_code.is_some() {
        4
    } else {
        0
    };
    Some(2 + transport + header.path.len() + log.payload.len())
}

/// What the firmware source says happens to a payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expected {
    Delivered,
    /// The sending radio refuses it with this code.
    Refused(u8),
    /// The sending radio cuts it short, and the receiving radio passes on what is left.
    Cut,
    /// The sending radio sends it, and the receiving radio doesn't pass it on.
    Lost,
}

impl Expected {
    pub fn describe(self) -> String {
        match self {
            Self::Delivered => "delivered".to_owned(),
            Self::Refused(code) => format!("refused: {}", error_name(code)),
            Self::Cut => "cut short".to_owned(),
            Self::Lost => "lost".to_owned(),
        }
    }
}

/// What the firmware source says happens to a payload of `len` bytes on `kind` along `route`.
pub fn expected(kind: Kind, route: &Route, len: usize) -> Expected {
    let path = route.path().len();
    match kind {
        // A longer frame is cut to MAX_FRAME, and text over MAX_TEXT fails to compose: the radio
        // reports that as TABLE_FULL. The route comes from the contact, not the frame.
        Kind::Text => {
            let text = len.min(MAX_FRAME - TEXT_FRAME_HEADER);
            if text > MAX_TEXT {
                Expected::Refused(TABLE_FULL)
            } else if text < len {
                Expected::Cut
            } else {
                Expected::Delivered
            }
        }
        Kind::Raw => {
            if route.is_flood() {
                // "flood, not supported (yet)"
                return Expected::Refused(UNSUPPORTED_CMD);
            }
            let sent = len.min(MAX_FRAME.saturating_sub(RAW_FRAME_HEADER + path));
            if sent < MIN_RAW_DATA {
                Expected::Refused(ILLEGAL_ARG)
            } else if sent + RAW_DATA_PUSH_HEADER > MAX_FRAME {
                // The receiving radio's push frame would be too long, and it drops it.
                Expected::Lost
            } else if sent < len {
                Expected::Cut
            } else {
                Expected::Delivered
            }
        }
        Kind::Channel => {
            let sent = len.min(MAX_FRAME.saturating_sub(CHANNEL_FRAME_HEADER + path));
            if sent > MAX_CHANNEL_DATA {
                Expected::Refused(ILLEGAL_ARG)
            } else if sent < len {
                Expected::Cut
            } else {
                Expected::Delivered
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use meshcore_rs::events::MeshPacketHeader;
    use meshcore_rs::{PayloadType, RouteType};

    use super::*;

    const KEY: [u8; 32] = [0xb3; 32];

    #[test]
    fn the_text_frame_is_laid_out_as_the_firmware_reads_it() {
        let frame = text_message(&[1, 2, 3, 4, 5, 6], 0x6543_2100, b"AAEC");
        assert_eq!(
            frame,
            [
                2, 0, 0, 0x00, 0x21, 0x43, 0x65, 1, 2, 3, 4, 5, 6, b'A', b'A', b'E', b'C'
            ]
        );
        // The firmware takes the text from byte 13 on.
        assert_eq!(frame.len() - TEXT_FRAME_HEADER, 4);
    }

    #[test]
    fn raw_data_frames_carry_the_path_byte_and_path() {
        assert_eq!(raw_data(&Route::zero_hop(), &[9; 4]), [25, 0, 9, 9, 9, 9]);
        let via = Route::via("a3f0", 1).unwrap();
        assert_eq!(raw_data(&via, &[9; 4]), [25, 2, 0xa3, 0xf0, 9, 9, 9, 9]);
        // Two hops of 2-byte hashes: hash size less one in the top bits.
        let wide = Route::via("a3f00b17", 2).unwrap();
        assert_eq!(wide.path_byte(), 0b0100_0010);
        // A flood has no path bytes; the radio refuses it.
        assert_eq!(raw_data(&Route::Flood, &[9; 4]), [25, 0xff, 9, 9, 9, 9]);
    }

    #[test]
    fn channel_data_frames_carry_the_experiments_data_type() {
        assert_eq!(
            channel_data(7, &Route::Flood, &[1, 2]),
            [62, 7, 0xff, 0xff, 0xff, 1, 2]
        );
        assert_eq!(
            channel_data(7, &Route::zero_hop(), &[1, 2]),
            [62, 7, 0, 0xff, 0xff, 1, 2]
        );
        assert_eq!(
            channel_data(7, &Route::via("a3", 1).unwrap(), &[1, 2]),
            [62, 7, 1, 0xa3, 0xff, 0xff, 1, 2]
        );
    }

    #[test]
    fn reset_path_names_the_contact_by_its_whole_key() {
        let frame = reset_path(&KEY);
        assert_eq!(frame.len(), 33);
        assert_eq!(frame[0], 13);
        assert_eq!(frame[1..], KEY);
    }

    #[test]
    fn via_refuses_what_is_not_a_path() {
        assert!(Route::via("a3", 0).is_err());
        assert!(Route::via("a3", 4).is_err());
        assert!(Route::via("a3f", 1).is_err());
        assert!(Route::via("", 1).is_err());
        assert!(Route::via("a3f0", 3).is_err());
        assert!(Route::via(&"ab".repeat(64), 1).is_err());
        assert!(Route::via(&"ab".repeat(63), 1).is_ok());
        assert!(Route::via("zz", 1).is_err());
        assert_eq!(Route::via("A3", 1).unwrap().label(), "via a3");
        assert_eq!(Route::zero_hop().label(), "zero hop");
        assert_eq!(Route::Flood.label(), "flood");
    }

    #[test]
    fn channel_datagrams_are_read_as_the_firmware_queues_them() {
        // SNR 7.25 dB, channel 7, direct, data type 0xFFFF, three bytes.
        let frame = [27, 29, 0, 0, 7, 0xff, 0xff, 0xff, 3, 0xaa, 0xbb, 0xcc];
        let datagram = ChannelDatagram::parse(&frame).unwrap();
        assert_eq!(datagram.snr, 7.25);
        assert_eq!(datagram.channel, 7);
        assert_eq!(received_route(datagram.path_byte), "direct");
        assert_eq!(datagram.data_type, DATA_TYPE_DEV);
        assert_eq!(datagram.data, [0xaa, 0xbb, 0xcc]);
        // A flood that took two hops, with a negative SNR.
        let flood = [27, 0xf8, 0, 0, 7, 2, 0xff, 0xff, 1, 0xaa];
        let datagram = ChannelDatagram::parse(&flood).unwrap();
        assert_eq!(datagram.snr, -2.0);
        assert_eq!(received_route(datagram.path_byte), "flood, 2 hops");
        // Too short for its length byte, or another frame.
        assert!(ChannelDatagram::parse(&frame[..11]).is_none());
        assert!(ChannelDatagram::parse(&[16, 29, 0, 0, 7, 0xff, 0xff, 0xff, 0]).is_none());
    }

    #[test]
    fn refusal_codes_come_through_meshcore_rs_as_one_byte() {
        // meshcore-rs reads ERR | 6 as the text "\u{6}".
        assert_eq!(error_code("\u{6}"), Some(ILLEGAL_ARG));
        assert_eq!(error_code("Unknown error"), None);
        assert_eq!(error_name(UNSUPPORTED_CMD), "UNSUPPORTED_CMD");
        assert_eq!(error_name(TABLE_FULL), "TABLE_FULL");
        assert_eq!(error_name(ILLEGAL_ARG), "ILLEGAL_ARG");
    }

    #[test]
    fn on_air_sizes_follow_the_firmware_packets() {
        let zero = Route::zero_hop();
        // The longest base64 text: 6 bytes before 176 bytes of encrypted text.
        assert_eq!(on_air_bytes(Kind::Text, &zero, 160), 182);
        assert_eq!(
            on_air_bytes(Kind::Text, &Route::via("a3", 1).unwrap(), 160),
            183
        );
        // Raw data: header, path byte and the payload as it is.
        assert_eq!(on_air_bytes(Kind::Raw, &zero, 172), 174);
        // Channel data: header, path byte, channel hash, MAC, 168 bytes encrypted in 176.
        assert_eq!(on_air_bytes(Kind::Channel, &zero, 165), 181);
        assert_eq!(on_air_bytes(Kind::Channel, &Route::Flood, 13), 21);
    }

    #[test]
    fn logged_packets_are_measured_with_their_header_and_path() {
        let header = |transport_code, path: Vec<u8>| MeshPacketHeader {
            route_type: RouteType::Direct,
            payload_type: PayloadType::RawCustom,
            payload_version: 0,
            transport_code,
            path_len: path.len() as u8,
            path_hash_size: 1,
            path,
        };
        let log = |header| LogData {
            snr: 0.0,
            rssi: -40,
            header,
            advertisement: None,
            payload: vec![0; 20],
        };
        assert_eq!(logged_bytes(&log(Some(header(None, vec![])))), Some(22));
        assert_eq!(logged_bytes(&log(Some(header(None, vec![0xa3])))), Some(23));
        assert_eq!(
            logged_bytes(&log(Some(header(Some([1, 2, 3, 4]), vec![])))),
            Some(26)
        );
        assert_eq!(logged_bytes(&log(None)), None);
    }

    #[test]
    fn the_source_sets_each_paths_limit() {
        let zero = Route::zero_hop();
        // Text: 160 bytes, refused as TABLE_FULL above that, also when the frame is cut.
        assert_eq!(expected(Kind::Text, &zero, 160), Expected::Delivered);
        assert_eq!(
            expected(Kind::Text, &zero, 161),
            Expected::Refused(TABLE_FULL)
        );
        assert_eq!(
            expected(Kind::Text, &zero, 200),
            Expected::Refused(TABLE_FULL)
        );
        assert_eq!(expected(Kind::Text, &Route::Flood, 46), Expected::Delivered);
        // Raw data: the receiving radio's push holds at most 172 bytes.
        assert_eq!(expected(Kind::Raw, &zero, 172), Expected::Delivered);
        assert_eq!(expected(Kind::Raw, &zero, 173), Expected::Lost);
        assert_eq!(expected(Kind::Raw, &zero, 174), Expected::Lost);
        assert_eq!(
            expected(Kind::Raw, &zero, 3),
            Expected::Refused(ILLEGAL_ARG)
        );
        assert_eq!(
            expected(Kind::Raw, &Route::Flood, 34),
            Expected::Refused(UNSUPPORTED_CMD)
        );
        // A long path leaves less room in the frame: 172 bytes along 3 hops are cut to 171.
        let three = Route::via("a3b4c5", 1).unwrap();
        assert_eq!(expected(Kind::Raw, &three, 171), Expected::Delivered);
        assert_eq!(expected(Kind::Raw, &three, 172), Expected::Cut);
        // Channel data: 165 bytes.
        assert_eq!(expected(Kind::Channel, &zero, 165), Expected::Delivered);
        assert_eq!(
            expected(Kind::Channel, &Route::Flood, 165),
            Expected::Delivered
        );
        assert_eq!(
            expected(Kind::Channel, &zero, 166),
            Expected::Refused(ILLEGAL_ARG)
        );
        assert_eq!(
            expected(
                Kind::Channel,
                &Route::via(&"ab".repeat(10), 1).unwrap(),
                165
            ),
            Expected::Cut
        );
    }

    #[test]
    fn kinds_have_names() {
        for kind in Kind::ALL {
            assert_eq!(Kind::parse(kind.name()), Some(kind));
        }
        assert_eq!(Kind::parse("binary"), None);
    }
}

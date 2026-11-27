//! The harness's own link to a companion radio, so that every frame passes through it.
//!
//! Over serial, a frame is a start byte (`<` to the radio, `>` from it), its length in two
//! little-endian bytes, and the payload. meshcore-rs speaks the protocol; this link frames what
//! it writes and unframes what the radio sends, writes both to the frame log, and refuses any
//! command that isn't on the list of ones that can't change firmware or radio settings.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use meshcore_rs::commands::CommandHandler;
use meshcore_rs::events::{EventDispatcher, EventPayload};
use meshcore_rs::reader::MessageReader;
use meshcore_rs::{EventType, MeshCoreEvent};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const TO_RADIO: u8 = b'<';
const FROM_RADIO: u8 = b'>';
/// Longer than any frame the firmware sends (176 bytes): a length past this is noise.
const LONGEST_FRAME: usize = 1024;

/// The companion commands the harness lets through. Everything else is refused, among it the
/// ones that set the frequency and preset (11), the transmit power (12), reboot (19), touch the
/// private key (23, 24) or reset the radio (51).
const ALLOWED_COMMANDS: &[u8] = &[
    1,  // APP_START
    2,  // SEND_TXT_MSG
    4,  // GET_CONTACTS
    5,  // GET_DEVICE_TIME
    7,  // SEND_SELF_ADVERT
    9,  // ADD_UPDATE_CONTACT
    10, // SYNC_NEXT_MESSAGE
    13, // RESET_PATH
    20, // GET_BATT_AND_STORAGE
    22, // DEVICE_QUERY
];

/// What the path spike (#140) needs besides: sends on its two other paths, and a private
/// channel in a free slot, which it empties at the end. Still nothing that changes firmware,
/// frequency, preset or power.
pub const SPIKE_COMMANDS: &[u8] = &[
    25, // SEND_RAW_DATA
    31, // GET_CHANNEL
    32, // SET_CHANNEL
    62, // SEND_CHANNEL_DATA
];

/// A byte stream to a radio.
pub trait Io: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Io for T {}

/// Every frame to and from the radios, one JSON object per line.
pub struct FrameLog {
    path: PathBuf,
    file: Mutex<File>,
    failed: AtomicBool,
}

impl FrameLog {
    pub fn create(folder: &Path) -> Result<Arc<Self>, String> {
        let fail = |error: std::io::Error| {
            format!(
                "Could not create a frame log in {}: {error}",
                folder.display()
            )
        };
        std::fs::create_dir_all(folder).map_err(fail)?;
        let path = folder.join(format!("frames-{}-{}.jsonl", now_ms(), std::process::id()));
        let file = File::create_new(&path).map_err(fail)?;
        Ok(Arc::new(Self {
            path,
            file: Mutex::new(file),
            failed: AtomicBool::new(false),
        }))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether any frame could not be written.
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }

    fn record(&self, radio: &str, direction: &str, frame: &[u8]) {
        let line = serde_json::json!({
            "at_ms": now_ms(),
            "radio": radio,
            "dir": direction,
            "frame": hex::encode(redacted(direction, frame)),
        });
        let mut file = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        if writeln!(file, "{line}").is_err() {
            self.failed.store(true, Ordering::SeqCst);
        }
    }
}

/// A frame as the log keeps it. Logs become test fixtures in the repository, so what a radio
/// says about its Bluetooth PIN and anyone's position is zeroed. Lengths stay the same.
fn redacted(direction: &str, frame: &[u8]) -> Vec<u8> {
    let mut frame = frame.to_vec();
    if direction == "from_radio"
        && let Some(position) = logged_advert_position(&frame)
    {
        frame[position].fill(0);
        return frame;
    }
    let secret = match (direction, frame.first()) {
        // CHANNEL_INFO and SET_CHANNEL: the channel's secret.
        ("from_radio", Some(18)) | ("to_radio" | "blocked", Some(32)) => 34..50,
        // DEVICE_INFO: the Bluetooth PIN.
        ("from_radio", Some(13)) => 4..8,
        // SELF_INFO: latitude and longitude.
        ("from_radio", Some(5)) => 36..44,
        // CONTACT and NEW_ADVERT: latitude and longitude.
        ("from_radio", Some(3 | 0x8a)) => 136..144,
        // ADD_UPDATE_CONTACT: the same.
        ("to_radio" | "blocked", Some(9)) => 136..144,
        _ => return frame,
    };
    let end = secret.end.min(frame.len());
    if secret.start < end {
        frame[secret.start..end].fill(0);
    }
    frame
}

/// Where the position sits in LOG_RX_DATA of an advert, if the advert gives one. The frame is
/// `0x88 | SNR | RSSI | packet`, the packet `header | [transport codes] | path byte | path |
/// payload`, and an advert's payload `key (32) | timestamp (4) | signature (64) | flags |
/// [latitude, longitude] | …`.
fn logged_advert_position(frame: &[u8]) -> Option<std::ops::Range<usize>> {
    const ADVERT: u8 = 4;
    const HAS_POSITION: u8 = 0x10;
    if frame.first() != Some(&0x88) {
        return None;
    }
    let header = *frame.get(3)?;
    if (header >> 2) & 0x0f != ADVERT {
        return None;
    }
    // Route types 0 and 3 carry 4 bytes of transport codes.
    let transport = if matches!(header & 3, 0 | 3) { 4 } else { 0 };
    let path_at = 4 + transport;
    let path_byte = *frame.get(path_at)?;
    let path = usize::from(path_byte & 0x3f) * (usize::from(path_byte >> 6) + 1);
    let flags_at = path_at + 1 + path + 32 + 4 + 64;
    if frame.get(flags_at)? & HAS_POSITION == 0 {
        return None;
    }
    let end = frame.len().min(flags_at + 9);
    Some(flags_at + 1..end)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// An open link. Dropping it lets go of the port.
pub struct Link {
    pub commands: Arc<tokio::sync::Mutex<CommandHandler>>,
    pub dispatcher: Arc<EventDispatcher>,
    tasks: Vec<JoinHandle<()>>,
}

impl Link {
    pub fn open(
        label: &str,
        stream: Box<dyn Io>,
        log: Arc<FrameLog>,
        reply_timeout: Duration,
    ) -> Self {
        Self::open_allowing(label, stream, log, reply_timeout, &[])
    }

    /// Like [`Link::open`], letting the commands in `also` through as well.
    pub fn open_allowing(
        label: &str,
        stream: Box<dyn Io>,
        log: Arc<FrameLog>,
        reply_timeout: Duration,
        also: &'static [u8],
    ) -> Self {
        let (to_radio, from_app) = mpsc::channel::<Vec<u8>>(64);
        let dispatcher = Arc::new(EventDispatcher::new());
        let reader = Arc::new(MessageReader::new(dispatcher.clone()));
        let mut handler = CommandHandler::new(to_radio, dispatcher.clone(), reader.clone());
        handler.set_default_timeout(reply_timeout);
        let (read, write) = tokio::io::split(stream);
        let tasks = vec![
            tokio::spawn(write_frames(
                label.to_owned(),
                from_app,
                write,
                log.clone(),
                also,
            )),
            tokio::spawn(read_frames(
                label.to_owned(),
                read,
                reader,
                dispatcher.clone(),
                log,
            )),
        ];
        Self {
            commands: Arc::new(tokio::sync::Mutex::new(handler)),
            dispatcher,
            tasks,
        }
    }

    /// Ends the link, and returns once the port is free.
    pub async fn close(mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

async fn write_frames(
    label: String,
    mut frames: mpsc::Receiver<Vec<u8>>,
    mut port: impl AsyncWrite + Unpin,
    log: Arc<FrameLog>,
    also: &'static [u8],
) {
    while let Some(frame) = frames.recv().await {
        if !frame
            .first()
            .is_some_and(|command| ALLOWED_COMMANDS.contains(command) || also.contains(command))
        {
            log.record(&label, "blocked", &frame);
            eprintln!(
                "{label}: refused to send command {:?}: the harness never changes a radio's \
                 firmware or settings.",
                frame.first()
            );
            continue;
        }
        let Ok(length) = u16::try_from(frame.len()) else {
            continue;
        };
        let mut framed = vec![TO_RADIO];
        framed.extend_from_slice(&length.to_le_bytes());
        framed.extend_from_slice(&frame);
        log.record(&label, "to_radio", &frame);
        if port.write_all(&framed).await.is_err() || port.flush().await.is_err() {
            return;
        }
    }
}

async fn read_frames(
    label: String,
    mut port: impl AsyncRead + Unpin,
    reader: Arc<MessageReader>,
    dispatcher: Arc<EventDispatcher>,
    log: Arc<FrameLog>,
) {
    let mut buffer = Vec::new();
    let mut chunk = [0; 1024];
    loop {
        match port.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
        while let Some(frame) = next_frame(&mut buffer) {
            log.record(&label, "from_radio", &frame);
            if let Err(error) = reader.handle_rx(frame).await {
                eprintln!("{label}: could not read a frame: {error}");
            }
        }
    }
    dispatcher
        .emit(MeshCoreEvent::new(
            EventType::Disconnected,
            EventPayload::None,
        ))
        .await;
}

/// Takes the next whole frame off `buffer`, skipping anything that isn't one.
fn next_frame(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    loop {
        let start = buffer.iter().position(|&byte| byte == FROM_RADIO);
        buffer.drain(..start.unwrap_or(buffer.len()));
        if buffer.len() < 3 {
            return None;
        }
        let length = usize::from(u16::from_le_bytes([buffer[1], buffer[2]]));
        if length == 0 || length > LONGEST_FRAME {
            buffer.remove(0);
            continue;
        }
        if buffer.len() < 3 + length {
            return None;
        }
        let frame = buffer[3..3 + length].to_vec();
        buffer.drain(..3 + length);
        return Some(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_133_frames_are_found_among_noise_and_in_pieces() {
        let mut buffer = b"boot\r\n>\xff\xff>\x02\x00".to_vec();
        assert_eq!(next_frame(&mut buffer), None);
        buffer.extend_from_slice(&[7, 8, b'>', 1, 0, 9]);
        assert_eq!(next_frame(&mut buffer), Some(vec![7, 8]));
        assert_eq!(next_frame(&mut buffer), Some(vec![9]));
        assert_eq!(next_frame(&mut buffer), None);
    }

    #[test]
    fn issue_140_logged_frames_hide_channel_secrets_and_advertised_positions() {
        assert_eq!(redacted("from_radio", &[18; 50])[34..], [0; 16]);
        assert_eq!(redacted("to_radio", &[32; 50])[34..], [0; 16]);
        // LOG_RX_DATA of a zero-hop advert with a position: 0x88, SNR, RSSI, header, path byte,
        // key, timestamp, signature, flags, latitude, longitude, name.
        let mut advert = vec![0x88, 29, 0xd8, (4 << 2) | 2, 0];
        advert.extend_from_slice(&[7; 100]);
        advert.push(0x91);
        advert.extend_from_slice(&[9; 8]);
        advert.extend_from_slice(b"R1");
        let logged = redacted("from_radio", &advert);
        let position = 106..114;
        assert_eq!(logged[position], [0; 8]);
        assert_eq!(logged[..106], advert[..106]);
        assert_eq!(logged[114..], *b"R1");
        // Without a position, and for other packets, nothing changes.
        advert[105] = 0x81;
        assert_eq!(redacted("from_radio", &advert), advert);
        advert[3] = (2 << 2) | 2;
        advert[105] = 0x91;
        assert_eq!(redacted("from_radio", &advert), advert);
    }

    #[test]
    fn issue_133_logged_frames_hide_the_pin_and_positions() {
        let device_info = redacted("from_radio", &[13; 12]);
        assert_eq!(device_info, [13, 13, 13, 13, 0, 0, 0, 0, 13, 13, 13, 13]);
        let me = redacted("from_radio", &[5; 50]);
        assert_eq!(me[36..44], [0; 8]);
        assert_eq!((me[35], me[44], me.len()), (5, 5, 50));
        let contact = redacted("from_radio", &[3; 148]);
        assert_eq!(contact[136..144], [0; 8]);
        // A short frame, and a command that only shares a code with a reply, stay as they are.
        assert_eq!(redacted("from_radio", &[5, 1]), [5, 1]);
        assert_eq!(redacted("to_radio", &[5; 50]), [5; 50]);
    }
}

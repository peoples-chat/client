//! Fixtures: the frames that passed between an app and a radio, as a text file.
//!
//! One frame per line, in the order they passed: who sent it, `app` or `radio`, then the
//! frame's payload in hex, without the stream framing. Blank lines and lines starting with `#`
//! are ignored.
//!
//! ```text
//! # The app asks for the next message, and there is none.
//! app 0a
//! radio 0a
//! ```

use std::path::Path;
use std::str::FromStr;
use std::{fs, io};

/// Who sent a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sender {
    App,
    Radio,
}

/// One captured frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub sender: Sender,
    pub payload: Vec<u8>,
}

/// Captured frames, in the order they passed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fixture {
    pub frames: Vec<Frame>,
}

impl Fixture {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        fs::read_to_string(path)?.parse()
    }
}

impl FromStr for Fixture {
    type Err = io::Error;

    fn from_str(text: &str) -> io::Result<Self> {
        let mut frames = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let bad = |what: &str| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("fixture line {}: {what}", number + 1),
                )
            };
            let (sender, payload) = line.split_once(' ').ok_or_else(|| bad("no frame"))?;
            let sender = match sender {
                "app" => Sender::App,
                "radio" => Sender::Radio,
                _ => return Err(bad("the sender is neither `app` nor `radio`")),
            };
            let payload = hex::decode(payload.trim()).map_err(|_| bad("not hex"))?;
            if payload.is_empty() {
                return Err(bad("no frame"));
            }
            frames.push(Frame { sender, payload });
        }
        Ok(Self { frames })
    }
}

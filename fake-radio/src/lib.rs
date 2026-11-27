//! A fake MeshCore companion radio over TCP, so that radio code can be tested without
//! hardware (#132). meshcore-rs connects to it as to any radio: `MeshCore::tcp`.
//!
//! [`pair`] starts two radios that hear each other. What each one sends over their pretend air
//! can be delayed, lost or duplicated ([`Config`]). [`FakeRadio::replay`] instead answers with
//! the frames of a [`Fixture`].
//!
//! Notes: `docs/radio.md`.

mod fixture;
mod frames;

use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::Instant;

pub use fixture::{Fixture, Frame, Sender};

/// The most text one message holds, in bytes.
const MAX_TEXT_BYTES: usize = 160;
/// How many messages a radio keeps for its app. When full, new ones are dropped.
const QUEUE_SIZE: usize = 16;

/// How one fake radio behaves. Set what a test needs and take the rest from `default()`.
#[derive(Debug, Clone)]
pub struct Config {
    /// The TCP port on 127.0.0.1. 0, the default, takes a free one, so tests can run in
    /// parallel.
    pub port: u16,
    pub name: String,
    /// The chance, from 0 to 1, that a packet this radio puts on the air is lost. Packets are
    /// its messages and its confirmations of the messages it received.
    pub loss: f64,
    /// The chance, from 0 to 1, that a packet this radio puts on the air arrives twice.
    pub duplication: f64,
    /// How long a packet this radio puts on the air takes to arrive.
    pub delay: Duration,
    /// How many contacts the radio can hold. It reports this halved, as firmware does, so an
    /// odd size reads one smaller.
    pub max_contacts: usize,
    /// Seeds the choice of what is lost or duplicated, so a run can be repeated.
    pub seed: u64,
    /// How long the radio takes to answer its app. meshcore-rs listens for an answer only
    /// after it has written the command, so an instant answer can be missed.
    pub reply_latency: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 0,
            name: "Fake radio".to_owned(),
            loss: 0.0,
            duplication: 0.0,
            delay: Duration::ZERO,
            max_contacts: 350,
            seed: 0,
            reply_latency: Duration::from_millis(20),
        }
    }
}

/// A contact in a radio's table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    pub public_key: [u8; 32],
    pub name: String,
    /// MeshCore's contact type: 1 a chat node, 2 a repeater, 3 a room, 4 a sensor.
    pub kind: u8,
    /// When the contact was last heard, in seconds since 1970.
    pub last_advert: u32,
}

/// The contact table has no room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableFull;

impl std::fmt::Display for TableFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the radio's contact table is full")
    }
}

impl std::error::Error for TableFull {}

/// A running fake radio. Dropping it closes its port and its link to the app.
pub struct FakeRadio {
    addr: SocketAddr,
    shared: Arc<Shared>,
    server: JoinHandle<()>,
}

struct Shared {
    config: Config,
    public_key: [u8; 32],
    state: Mutex<State>,
}

struct State {
    /// Frames for the app that is connected, if one is.
    app: Option<mpsc::UnboundedSender<Vec<u8>>>,
    contacts: Vec<(Contact, u32)>,
    /// Message frames waiting for the app to collect them.
    waiting: VecDeque<Waiting>,
    /// Confirmations still due, by their code, with when the message went.
    unconfirmed: HashMap<[u8; 4], Instant>,
    /// The radios in range.
    peers: Vec<Weak<Shared>>,
    rng: StdRng,
    replay: Option<Replay>,
    /// How many of the next messages are lost on the air.
    lose_texts: usize,
}

struct Waiting {
    from: [u8; 32],
    sent_at: u32,
    text: Vec<u8>,
}

/// A fixture being replayed.
struct Replay {
    left: VecDeque<Frame>,
    mismatches: Vec<Vec<u8>>,
}

impl Replay {
    /// The radio's frames up to the app's next one.
    fn radio_frames(&mut self) -> Vec<Vec<u8>> {
        let mut frames = Vec::new();
        while self
            .left
            .front()
            .is_some_and(|frame| frame.sender == Sender::Radio)
        {
            frames.extend(self.left.pop_front().map(|frame| frame.payload));
        }
        frames
    }

    /// The captured answer to a command. Commands are compared by their code only, because the
    /// rest holds things like the time.
    fn answer(&mut self, command: &[u8]) -> Vec<Vec<u8>> {
        let expected = self.left.front().map(|frame| frame.payload.first());
        if expected == Some(command.first()) {
            self.left.pop_front();
            self.radio_frames()
        } else {
            self.mismatches.push(command.to_vec());
            vec![frames::error(frames::ERR_UNSUPPORTED)]
        }
    }
}

/// What a radio puts on the air.
#[derive(Clone)]
enum Packet {
    Text {
        from: [u8; 32],
        to_prefix: [u8; 6],
        sent_at: u32,
        text: Vec<u8>,
        ack: [u8; 4],
    },
    Ack {
        to: [u8; 32],
        ack: [u8; 4],
    },
    Advert {
        from: [u8; 32],
    },
}

impl FakeRadio {
    /// Starts a fake radio with a new random key. It is in range of no other until [`link`].
    pub async fn start(config: Config) -> io::Result<Self> {
        Self::serve(config, None).await
    }

    /// Starts a radio that answers with a fixture's frames instead of its own. It sends the
    /// radio's frames that come first when the app connects, then those that follow each of
    /// the app's. Only `port` and `reply_latency` of the config apply.
    pub async fn replay(config: Config, fixture: Fixture) -> io::Result<Self> {
        let replay = Replay {
            left: fixture.frames.into(),
            mismatches: Vec::new(),
        };
        Self::serve(config, Some(replay)).await
    }

    async fn serve(config: Config, replay: Option<Replay>) -> io::Result<Self> {
        for chance in [config.loss, config.duplication] {
            if !(0.0..=1.0).contains(&chance) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "loss and duplication are chances from 0 to 1",
                ));
            }
        }
        let listener = TcpListener::bind(("127.0.0.1", config.port)).await?;
        let addr = listener.local_addr()?;
        let state = State {
            app: None,
            contacts: Vec::new(),
            waiting: VecDeque::new(),
            unconfirmed: HashMap::new(),
            peers: Vec::new(),
            rng: StdRng::seed_from_u64(config.seed),
            replay,
            lose_texts: 0,
        };
        let shared = Arc::new(Shared {
            config,
            public_key: rand::random(),
            state: Mutex::new(state),
        });
        let server = tokio::spawn(accept(listener, shared.clone()));
        Ok(Self {
            addr,
            shared,
            server,
        })
    }

    /// Where the radio listens.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.shared.public_key
    }

    /// Adds a contact, or updates the one with its key.
    pub fn add_contact(&self, contact: Contact) -> Result<(), TableFull> {
        self.shared.add_contact(contact)
    }

    /// Loses the next `count` messages this radio puts on the air, whatever
    /// [`Config::loss`] says: for tests that need a certain loss.
    pub fn lose_next_messages(&self, count: usize) {
        self.shared.state().lose_texts = count;
    }

    /// The app's commands that didn't follow the fixture being replayed.
    pub fn replay_mismatches(&self) -> Vec<Vec<u8>> {
        let state = self.shared.state();
        state
            .replay
            .as_ref()
            .map_or_else(Vec::new, |replay| replay.mismatches.clone())
    }

    /// How many of the fixture's frames haven't passed yet.
    pub fn replay_remaining(&self) -> usize {
        let state = self.shared.state();
        state.replay.as_ref().map_or(0, |replay| replay.left.len())
    }
}

impl Drop for FakeRadio {
    fn drop(&mut self) {
        // Ends the listener and, with it, the connection it serves.
        self.server.abort();
    }
}

/// Puts two radios in range of each other, and adds each to the other's contacts. A radio
/// whose table is full still hears the other, but doesn't hold it: it can't send to it, and
/// drops its messages, which it couldn't decrypt.
pub fn link(a: &FakeRadio, b: &FakeRadio) -> Result<(), TableFull> {
    let mut result = Ok(());
    for (one, other) in [(a, b), (b, a)] {
        one.shared.state().peers.push(Arc::downgrade(&other.shared));
        let added = one.add_contact(Contact {
            public_key: other.public_key(),
            name: other.shared.config.name.clone(),
            kind: 1,
            last_advert: now(),
        });
        result = result.and(added);
    }
    result
}

/// Starts two linked radios. Fails if either has no room for the other; start them and
/// [`link`] them yourself to test that.
pub async fn pair(a: Config, b: Config) -> io::Result<(FakeRadio, FakeRadio)> {
    let a = FakeRadio::start(a).await?;
    let b = FakeRadio::start(b).await?;
    link(&a, &b).map_err(io::Error::other)?;
    Ok((a, b))
}

fn now() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as u32)
}

/// Serves one app at a time, as a radio's link does: a new connection replaces the old one.
async fn accept(listener: TcpListener, shared: Arc<Shared>) {
    // Dropped with this task, which ends the connection.
    let mut connection = JoinSet::new();
    while let Ok((stream, _)) = listener.accept().await {
        connection.shutdown().await;
        connection.spawn(converse(stream, shared.clone()));
    }
}

async fn converse(stream: TcpStream, shared: Arc<Shared>) {
    let (mut from_app, mut to_app) = stream.into_split();
    let (sender, mut outgoing) = mpsc::unbounded_channel::<Vec<u8>>();
    let greeting = {
        let mut state = shared.state();
        state.app = Some(sender);
        state.replay.as_mut().map(Replay::radio_frames)
    };
    let write = async {
        while let Some(frame) = outgoing.recv().await {
            if frames::write(&mut to_app, &frame).await.is_err() {
                break;
            }
        }
        let _ = to_app.shutdown().await;
    };
    let read = async {
        if let Some(greeting) = greeting {
            tokio::time::sleep(shared.config.reply_latency).await;
            shared.to_app(greeting);
        }
        while let Ok(command) = frames::read(&mut from_app).await {
            tokio::time::sleep(shared.config.reply_latency).await;
            let (answer, packet) = shared.answer(&command);
            // The answer goes first, so a confirmation can't overtake its SENT.
            shared.to_app(answer);
            if let Some(packet) = packet {
                shared.transmit(packet);
            }
        }
    };
    tokio::select! {
        () = write => {}
        () = read => {}
    }
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn to_app(&self, frames: Vec<Vec<u8>>) {
        if let Some(app) = &self.state().app {
            for frame in frames {
                let _ = app.send(frame);
            }
        }
    }

    fn add_contact(&self, contact: Contact) -> Result<(), TableFull> {
        let mut state = self.state();
        let modified = now();
        let held = state
            .contacts
            .iter()
            .position(|(held, _)| held.public_key == contact.public_key);
        match held {
            Some(index) => state.contacts[index] = (contact, modified),
            None if state.contacts.len() < self.config.max_contacts => {
                state.contacts.push((contact, modified));
            }
            None => return Err(TableFull),
        }
        Ok(())
    }

    /// The answer to one command, and what it puts on the air.
    fn answer(&self, command: &[u8]) -> (Vec<Vec<u8>>, Option<Packet>) {
        if let Some(replay) = &mut self.state().replay {
            return (replay.answer(command), None);
        }
        let Some((&code, body)) = command.split_first() else {
            return (vec![frames::error(frames::ERR_ILLEGAL_ARG)], None);
        };
        let answer = match code {
            frames::CMD_APP_START => frames::self_info(&self.public_key, &self.config.name),
            frames::CMD_DEVICE_QUERY => frames::device_info(self.config.max_contacts),
            frames::CMD_GET_CONTACTS => return (self.contacts(body), None),
            frames::CMD_SEND_TXT_MSG => {
                let (answer, packet) = self.send_text(body);
                return (vec![answer], packet);
            }
            frames::CMD_SEND_SELF_ADVERT => {
                let advert = Packet::Advert {
                    from: self.public_key,
                };
                return (vec![frames::ok()], Some(advert));
            }
            frames::CMD_SYNC_NEXT_MESSAGE => self
                .state()
                .waiting
                .pop_front()
                .map_or_else(frames::no_more_messages, |m| {
                    frames::message(&m.from, m.sent_at, &m.text)
                }),
            _ => frames::error(frames::ERR_UNSUPPORTED),
        };
        (vec![answer], None)
    }

    /// GET_CONTACTS, with an optional time: only contacts changed since then.
    fn contacts(&self, body: &[u8]) -> Vec<Vec<u8>> {
        let since = body
            .first_chunk::<4>()
            .map_or(0, |since| u32::from_le_bytes(*since));
        let state = self.state();
        let mut answer = vec![frames::contacts_start(state.contacts.len())];
        let changed = state.contacts.iter().filter(|(_, at)| *at > since);
        answer.extend(changed.map(|(contact, at)| frames::contact(contact, *at)));
        let latest = state.contacts.iter().map(|(_, at)| *at).max();
        answer.push(frames::contacts_end(latest.unwrap_or(0)));
        answer
    }

    /// SEND_TXT_MSG: text type, attempt, time, 6 bytes of the recipient's key, text.
    fn send_text(&self, body: &[u8]) -> (Vec<u8>, Option<Packet>) {
        let refuse = |code| (frames::error(code), None);
        let Some((header, text)) = body.split_first_chunk::<12>() else {
            return refuse(frames::ERR_ILLEGAL_ARG);
        };
        let [kind, attempt, t0, t1, t2, t3, to_prefix @ ..] = *header;
        // Only plain text.
        if kind != 0 || text.len() > MAX_TEXT_BYTES {
            return refuse(frames::ERR_ILLEGAL_ARG);
        }
        let mut state = self.state();
        if !state
            .contacts
            .iter()
            .any(|(contact, _)| contact.public_key[..6] == to_prefix)
        {
            return refuse(frames::ERR_NOT_FOUND);
        }
        // As the firmware computes it: over the time, the attempt, the text and the sender.
        let digest = Sha256::new()
            .chain_update([t0, t1, t2, t3, attempt & 3])
            .chain_update(text)
            .chain_update(self.public_key)
            .finalize();
        let ack = [digest[0], digest[1], digest[2], digest[3]];
        state.unconfirmed.insert(ack, Instant::now());
        let packet = Packet::Text {
            from: self.public_key,
            to_prefix,
            sent_at: u32::from_le_bytes([t0, t1, t2, t3]),
            text: text.to_vec(),
            ack,
        };
        // Time for the message and its confirmation, and some to spare.
        let timeout = Duration::from_secs(3) + self.config.delay * 2;
        let timeout = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        (frames::sent(ack, timeout), Some(packet))
    }

    /// Puts a packet on the air, where it may be delayed, lost or duplicated.
    fn transmit(&self, packet: Packet) {
        let (copies, peers) = {
            let mut state = self.state();
            let mut lost = state.rng.gen_bool(self.config.loss);
            if matches!(packet, Packet::Text { .. }) && state.lose_texts > 0 {
                state.lose_texts -= 1;
                lost = true;
            }
            let twice = state.rng.gen_bool(self.config.duplication);
            let copies = if lost { 0 } else { 1 + usize::from(twice) };
            (copies, state.peers.clone())
        };
        if copies == 0 {
            return;
        }
        let delay = self.config.delay;
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            for _ in 0..copies {
                for peer in peers.iter().filter_map(Weak::upgrade) {
                    peer.receive(packet.clone());
                }
            }
        });
    }

    fn receive(&self, packet: Packet) {
        match packet {
            Packet::Text {
                from,
                to_prefix,
                sent_at,
                text,
                ack,
            } => {
                {
                    let mut state = self.state();
                    // Only the addressee, and only from a contact: others can't decrypt it.
                    let known = state.contacts.iter().any(|(c, _)| c.public_key == from);
                    if self.public_key[..6] != to_prefix
                        || !known
                        || state.waiting.len() >= QUEUE_SIZE
                    {
                        return;
                    }
                    state.waiting.push_back(Waiting {
                        from,
                        sent_at,
                        text,
                    });
                }
                self.to_app(vec![frames::message_waiting()]);
                self.transmit(Packet::Ack { to: from, ack });
            }
            Packet::Advert { from } => {
                // Only contacts: the fake adds none by itself.
                let known = self
                    .state()
                    .contacts
                    .iter()
                    .any(|(c, _)| c.public_key == from);
                if known {
                    self.to_app(vec![frames::advert(&from)]);
                }
            }
            Packet::Ack { to, ack } => {
                if to != self.public_key {
                    return;
                }
                // A confirmation counts once.
                let sent = self.state().unconfirmed.remove(&ack);
                if let Some(sent) = sent {
                    let round_trip = u32::try_from(sent.elapsed().as_millis()).unwrap_or(u32::MAX);
                    self.to_app(vec![frames::send_confirmed(ack, round_trip)]);
                }
            }
        }
    }
}

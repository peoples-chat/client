//! The over-the-air tests (#134): two apps, each with its own core, on the lab's first two USB
//! radios.
//!
//! The phones become contacts through a relay inside this process, which also carries their
//! radio address records. The relay is then shut, so that only the radios can carry what
//! follows. Both cores pair their radio as the app does, through ports that open it with the
//! harness's own link: the radio is checked at every open, and every frame is logged.
//!
//! This is the only part of the harness that puts anything on the air: adverts from each radio
//! (zero hop, so no repeater passes them on) and a few short messages, plus the retries.

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use async_trait::async_trait;
use peoples_chat_core::Core;
use peoples_chat_core::interface::{
    Chat, DeliveryState, Direction, Mesh, RadioConnection, RadioIssue, Reach, Transport,
};
use peoples_chat_core::radio::{
    EXPECTED_PRESET, MeshCoreRadio, Port, Radio, RadioContact, RadioError, RadioEvent, RadioInfo,
    RadioPorts, Sent,
};
use tokio::sync::broadcast;
use tokio::time::Instant;

use crate::config::{Config, HubPort, RadioEntry};
use crate::lab::{Lab, Timing, Usb, find_port};
use crate::link::{FrameLog, Link};
use crate::power::Power;
use crate::support::{self, Phone, TestRelay};

/// How long each step may take.
#[derive(Clone, Copy, Debug)]
pub struct Waits {
    /// A message to arrive and be confirmed, with its one retry: tens of seconds.
    pub delivery: Duration,
    /// An advert to be heard, before it is sent again.
    pub advert: Duration,
    /// Anything the apps do among themselves: the relay counting as gone, an address record.
    pub app: Duration,
    /// How long the second radio's port stays closed.
    pub closed: Duration,
    /// The app to find its radio again once the port is back.
    pub reconnect: Duration,
}

impl Default for Waits {
    fn default() -> Self {
        Self {
            delivery: Duration::from_secs(60),
            advert: Duration::from_secs(15),
            app: Duration::from_secs(60),
            closed: Duration::from_secs(5),
            reconnect: Duration::from_secs(60),
        }
    }
}

/// How often an advert is sent before giving up: a single packet on the air can be lost.
const ADVERT_TRIES: usize = 3;

/// What the tests need from the harness.
pub struct Bench {
    pub config: Config,
    pub usb: Arc<dyn Usb>,
    pub log: Arc<FrameLog>,
    pub timing: Timing,
    pub waits: Waits,
}

/// Ports never touch a radio's power.
struct NoPower;

#[async_trait]
impl Power for NoPower {
    async fn cycle(&self, _: &HubPort) -> Result<(), String> {
        Err("the message tests never cycle a radio's power".to_owned())
    }
}

/// A radio a core has open, over the harness's link.
struct LabRadio {
    radio: MeshCoreRadio,
    link: tokio::sync::Mutex<Option<Link>>,
    /// How long the radio said the last message's confirmation may take.
    estimate: Arc<Mutex<Option<Duration>>>,
}

impl LabRadio {
    /// Closes the serial port under the app, and tells it the link ended, as an unplugged
    /// radio would.
    async fn cut(&self) {
        if let Some(link) = self.link.lock().await.take() {
            let dispatcher = link.dispatcher.clone();
            link.close().await;
            dispatcher
                .emit(meshcore_rs::MeshCoreEvent::new(
                    meshcore_rs::EventType::Disconnected,
                    meshcore_rs::events::EventPayload::None,
                ))
                .await;
        }
    }

    /// Announces the radio to those in direct range.
    async fn advertise(&self) -> Result<(), String> {
        let link = self.link.lock().await;
        let link = link.as_ref().ok_or("the radio's port is closed")?;
        let commands = link.commands.lock().await;
        commands
            .send_advert(false)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// The radio's own clock, in seconds since the Unix epoch. Reading it changes nothing.
    async fn clock(&self) -> Option<u32> {
        let link = self.link.lock().await;
        let commands = link.as_ref()?.commands.lock().await;
        commands.get_time().await.ok()
    }
}

#[async_trait]
impl Radio for LabRadio {
    fn mesh(&self) -> peoples_chat_core::radio::Mesh {
        self.radio.mesh()
    }

    async fn info(&self) -> Result<RadioInfo, RadioError> {
        self.radio.info().await
    }

    async fn contacts(&self) -> Result<Vec<RadioContact>, RadioError> {
        self.radio.contacts().await
    }

    async fn send_text(&self, to: &str, text: &str) -> Result<Sent, RadioError> {
        let sent = self.radio.send_text(to, text).await?;
        *self.estimate.lock().unwrap_or_else(PoisonError::into_inner) = Some(sent.timeout);
        Ok(sent)
    }

    fn events(&self) -> broadcast::Receiver<RadioEvent> {
        self.radio.events()
    }

    async fn disconnect(&self) -> Result<(), RadioError> {
        let ended = self.radio.disconnect().await;
        if let Some(link) = self.link.lock().await.take() {
            link.close().await;
        }
        ended
    }
}

/// The one USB port a phone's core sees: its own radio's.
struct LabPorts {
    entry: RadioEntry,
    config: Config,
    usb: Arc<dyn Usb>,
    log: Arc<FrameLog>,
    timing: Timing,
    /// The port is held back, as when the radio is unplugged.
    unplugged: AtomicBool,
    open: Mutex<Weak<LabRadio>>,
    estimate: Arc<Mutex<Option<Duration>>>,
}

impl LabPorts {
    fn new(bench: &Bench, entry: &RadioEntry) -> Arc<Self> {
        Arc::new(Self {
            entry: entry.clone(),
            config: bench.config.clone(),
            usb: bench.usb.clone(),
            log: bench.log.clone(),
            timing: bench.timing,
            unplugged: AtomicBool::new(false),
            open: Mutex::new(Weak::new()),
            estimate: Arc::new(Mutex::new(None)),
        })
    }

    fn serial(&self) -> &str {
        self.entry.usb_serial.as_deref().unwrap_or_default()
    }

    fn radio(&self) -> Result<Arc<LabRadio>, String> {
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .upgrade()
            .ok_or_else(|| format!("{}: the app doesn't have it open", self.entry.label))
    }

    async fn unplug(&self) -> Result<(), String> {
        self.unplugged.store(true, Ordering::SeqCst);
        self.radio()?.cut().await;
        Ok(())
    }

    fn plug_in(&self) {
        self.unplugged.store(false, Ordering::SeqCst);
    }

    async fn path(&self) -> Result<String, String> {
        let ports = self.list().await.map_err(|error| error.to_string())?;
        find_port(&ports, self.serial())
            .map_err(|_| format!("{}: its USB port is gone", self.entry.label))
    }
}

#[async_trait]
impl RadioPorts for LabPorts {
    async fn list(&self) -> Result<Vec<Port>, RadioError> {
        if self.unplugged.load(Ordering::SeqCst) {
            return Ok(Vec::new());
        }
        let ports = self.usb.list().await.map_err(RadioError::Link)?;
        Ok(ports
            .into_iter()
            .filter(|port| {
                port.serial_number
                    .as_deref()
                    .is_some_and(|serial| serial.eq_ignore_ascii_case(self.serial()))
            })
            .collect())
    }

    async fn open(&self, _path: &str) -> Result<Arc<dyn Radio>, RadioError> {
        let lab = Lab {
            config: &self.config,
            usb: self.usb.clone(),
            power: &NoPower,
            log: self.log.clone(),
            timing: self.timing,
        };
        // Finds the port by serial number again, and checks firmware, preset and power.
        let station = lab
            .connect(&self.entry)
            .await
            .map_err(|trouble| RadioError::Link(trouble.message().to_owned()))?;
        let radio = Arc::new(LabRadio {
            radio: station.radio,
            link: tokio::sync::Mutex::new(Some(station.link)),
            estimate: self.estimate.clone(),
        });
        *self.open.lock().unwrap_or_else(PoisonError::into_inner) = Arc::downgrade(&radio);
        Ok(radio)
    }
}

/// One side of the test: a phone, its core and its radio.
struct Side {
    label: String,
    phone: Phone,
    core: Core,
    ports: Arc<LabPorts>,
}

impl Side {
    async fn start(bench: &Bench, entry: &RadioEntry, name: &str, relay: &str) -> Self {
        let phone = Phone::new(&format!("lab-{}", entry.label), relay);
        let core = phone.start().await;
        let ports = LabPorts::new(bench, entry);
        core.set_radio_reconnect_wait(Duration::from_secs(1));
        if let Err(error) = core.create_account(name).await {
            panic!("could not create {name}'s account: {}", error.message);
        }
        Self {
            label: entry.label.clone(),
            phone,
            core,
            ports,
        }
    }

    fn account_key(&self) -> String {
        self.core
            .snapshot()
            .account
            .map(|account| account.public_key)
            .unwrap_or_default()
    }

    fn chat(&self, chat_id: &str) -> Option<Chat> {
        self.core
            .snapshot()
            .chats
            .into_iter()
            .find(|chat| chat.id == chat_id)
    }

    fn connection(&self) -> Option<RadioConnection> {
        self.core
            .snapshot()
            .radio
            .paired
            .map(|paired| paired.connection)
    }

    /// The radio key this phone has stored for the contact `account`, from `radio.json`.
    fn stored_radio_of(&self, account: &str) -> Option<String> {
        stored_radio(self.phone.data_dir(), account)
    }

    fn retries(&self) -> usize {
        self.core
            .radio_transmissions()
            .iter()
            .filter(|sent| sent.reason == "retry")
            .count()
    }
}

fn stored_radio(data_dir: &Path, account: &str) -> Option<String> {
    let text = std::fs::read_to_string(data_dir.join("radio.json")).ok()?;
    let store: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(store["contacts"][account]["key"].as_str()?.to_owned())
}

/// Waits up to `wait` until `done` returns something.
async fn within<T>(
    wait: Duration,
    what: &str,
    mut done: impl FnMut() -> Option<T>,
) -> Result<T, String> {
    let deadline = Instant::now() + wait;
    loop {
        if let Some(value) = done() {
            return Ok(value);
        }
        if Instant::now() >= deadline {
            return Err(format!("{what}: not within {} s", wait.as_secs()));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Runs the over-the-air tests on the lab's first two USB radios.
pub async fn run(bench: &Bench, out: &mut (dyn Write + Send)) -> Result<(), String> {
    let usb: Vec<&RadioEntry> = bench
        .config
        .radios
        .iter()
        .filter(|radio| radio.usb_serial.is_some())
        .collect();
    let [first, second, ..] = usb[..] else {
        let _ = writeln!(out, "Over the air: needs two USB radios, not run");
        return Ok(());
    };
    let _ = writeln!(out, "Over the air: {} and {}", first.label, second.label);

    let relay = TestRelay::start().await;
    let ada = Side::start(bench, first, "Lab Ada", &relay.url()).await;
    let bea = Side::start(bench, second, "Lab Bea", &relay.url()).await;
    let result = scenario(bench, &relay, &ada, &bea, out).await;
    for side in [ada, bea] {
        // Lets go of the port before the lab lock is released.
        let _ = side.core.forget_radio().await;
        let _ = side.core.close().await;
    }
    result
}

async fn scenario(
    bench: &Bench,
    relay: &TestRelay,
    ada: &Side,
    bea: &Side,
    out: &mut (dyn Write + Send),
) -> Result<(), String> {
    let waits = bench.waits;
    let chat_id = support::make_contacts(&ada.core, &bea.core).await;
    within(waits.app, "the second phone's chat", || {
        bea.chat(&chat_id).map(|_| ())
    })
    .await?;
    settled(waits.app, &ada.core, &bea.core, &chat_id).await?;

    // Pairing over USB, as the app does it (#24).
    for side in [ada, bea] {
        side.core.use_radio_ports(side.ports.clone());
        let path = side.ports.path().await?;
        let status = side
            .core
            .pair_radio(&path)
            .await
            .map_err(|error| format!("{}: could not pair: {}", side.label, error.message))?;
        let paired = status.paired.ok_or("no radio after pairing")?;
        let preset = paired.preset;
        let expected = (
            preset.frequency_khz,
            preset.bandwidth_hz,
            preset.spreading_factor,
            preset.coding_rate,
        ) == EXPECTED_PRESET;
        if paired.connection != RadioConnection::Connected || preset.expected != expected {
            return Err(format!("{}: paired wrongly: {paired:?}", side.label));
        }
        let warning = if preset.expected {
            "no preset warning"
        } else {
            "the app warns that it isn't on EU/UK (Narrow)"
        };
        let _ = writeln!(out, "ok {}: paired on {path}, {warning}", side.label);
    }

    // Address records (#34).
    let radio_keys = [ada, bea].map(|side| {
        side.core
            .snapshot()
            .radio
            .paired
            .map(|paired| paired.public_key.to_ascii_lowercase())
    });
    for (side, other, other_radio) in [(ada, bea, &radio_keys[1]), (bea, ada, &radio_keys[0])] {
        let account = other.account_key();
        within(
            waits.app,
            &format!("{}: {}'s address record", side.label, other.label),
            || {
                let meshes = side.chat(&chat_id)?.contact.meshes;
                (meshes == [Mesh::MeshCore] && side.stored_radio_of(&account) == *other_radio)
                    .then_some(())
            },
        )
        .await?;
        let _ = writeln!(
            out,
            "ok {}: stored {}'s radio address record",
            side.label, other.label
        );
    }

    // Only the radios from here on.
    relay.block_messages();

    // Reachable only after the contact's radio has been heard (#44).
    for side in [ada, bea] {
        not_heard(waits.app, side, &chat_id).await?;
        let _ = writeln!(
            out,
            "ok {}: contact not reachable, their radio not heard yet",
            side.label
        );
    }
    let first = heard_after_advert(bench, bea, ada, &chat_id, out).await?;
    // The first radio hasn't announced itself, so the second still can't reach it.
    not_heard(waits.app, bea, &chat_id).await?;
    let second = heard_after_advert(bench, ada, bea, &chat_id, out).await?;
    if !(first && second) {
        // A message and its acknowledgement are heard too (#44). The app's switch for testing
        // sends one although the contact isn't reachable yet.
        heard_by_message(waits.delivery, ada, bea, &chat_id, out).await?;
    }
    for (side, other) in [(ada, bea), (bea, ada)] {
        within(waits.app, "the contact is reachable by radio", || {
            side.chat(&chat_id)
                .filter(|chat| chat.reach == Reach::Radio)
                .map(|_| ())
        })
        .await?;
        let _ = writeln!(
            out,
            "ok {}: {}'s radio was heard, contact reachable by radio",
            side.label, other.label
        );
    }

    // A message each way (#40, #41).
    exchange(waits.delivery, ada, bea, &chat_id, "Lab message 1", out).await?;
    exchange(waits.delivery, bea, ada, &chat_id, "Lab reply", out).await?;

    // The second radio goes away and comes back.
    bea.ports.unplug().await?;
    within(waits.app, "the app notices its radio is gone", || {
        (bea.connection() != Some(RadioConnection::Connected)).then_some(())
    })
    .await?;
    let _ = writeln!(
        out,
        "ok {}: serial port closed, the app shows its radio as gone",
        bea.label
    );
    tokio::time::sleep(waits.closed).await;
    let back = Instant::now();
    bea.ports.plug_in();
    within(waits.reconnect, "the app reconnects to its radio", || {
        (bea.connection() == Some(RadioConnection::Connected)).then_some(())
    })
    .await?;
    let _ = writeln!(
        out,
        "ok {}: reconnected by itself {:.1} s after the port came back",
        bea.label,
        back.elapsed().as_secs_f32()
    );
    exchange(waits.delivery, ada, bea, &chat_id, "Lab message 2", out).await?;
    if !(first && second) {
        let _ = writeln!(
            out,
            "WARNING: an advert wasn't reported (see above). Reachability was shown with a \
             message and its acknowledgement only."
        );
    }
    Ok(())
}

/// Waits until both phones have the chat's radio keys in the same epoch.
async fn settled(wait: Duration, ada: &Core, bea: &Core, chat_id: &str) -> Result<(), String> {
    let deadline = Instant::now() + wait;
    loop {
        if let (Ok(a), Ok(b)) = (ada.radio_keys(chat_id).await, bea.radio_keys(chat_id).await)
            && a.epoch == b.epoch
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("the chat didn't settle on both phones".to_owned());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Waits until the relay counts as gone, and checks that the contact then isn't reachable
/// because their radio hasn't been heard.
async fn not_heard(wait: Duration, side: &Side, chat_id: &str) -> Result<(), String> {
    let chat = within(wait, "the relay counts as gone", || {
        side.chat(chat_id)
            .filter(|chat| chat.reach != Reach::Internet)
    })
    .await?;
    if chat.reach == Reach::Unreachable && chat.radio_issue == Some(RadioIssue::NotHeard) {
        Ok(())
    } else {
        Err(format!(
            "{}: before the contact's radio was heard, the chat is {:?} ({:?})",
            side.label, chat.reach, chat.radio_issue
        ))
    }
}

/// `from`'s radio announces itself, and `to`'s app then counts the contact as reachable. When
/// `to`'s radio never reports it, says what the clocks show and returns false.
async fn heard_after_advert(
    bench: &Bench,
    from: &Side,
    to: &Side,
    chat_id: &str,
    out: &mut (dyn Write + Send),
) -> Result<bool, String> {
    for attempt in 1..=ADVERT_TRIES {
        from.ports.radio()?.advertise().await?;
        let heard = within(bench.waits.advert, "the advert", || {
            to.chat(chat_id)
                .filter(|chat| chat.reach == Reach::Radio)
                .map(|_| ())
        })
        .await;
        if heard.is_ok() {
            let _ = writeln!(
                out,
                "ok {}: heard {}'s advert (try {attempt}), contact reachable by radio",
                to.label, from.label
            );
            return Ok(true);
        }
    }
    let seconds = |time: Option<u64>| time.map_or("unknown".to_owned(), |time| time.to_string());
    let clock = from.ports.radio()?.clock().await;
    let contacts = to
        .ports
        .radio()?
        .contacts()
        .await
        .map_err(|error| error.to_string())?;
    let own = from
        .core
        .snapshot()
        .radio
        .paired
        .map(|paired| paired.public_key)
        .unwrap_or_default();
    let held = contacts
        .iter()
        .find(|contact| contact.public_key.eq_ignore_ascii_case(&own));
    let _ = writeln!(
        out,
        "WARNING {}: reported none of {}'s {ADVERT_TRIES} adverts. {}'s clock: {}; the newest \
         advert time {} holds for it: {}; this computer: {} (Unix seconds). A radio ignores an \
         advert that isn't newer than the last.",
        to.label,
        from.label,
        from.label,
        seconds(clock.map(u64::from)),
        to.label,
        held.map_or("not a contact".to_owned(), |held| seconds(held.last_heard)),
        support::now_ms() / 1000
    );
    Ok(false)
}

/// One message sent with the app's force-radio switch, so that each app hears the other's radio:
/// the receiver by the message, the sender by its acknowledgement.
async fn heard_by_message(
    wait: Duration,
    from: &Side,
    to: &Side,
    chat_id: &str,
    out: &mut (dyn Write + Send),
) -> Result<(), String> {
    let force = |on| from.core.set_force_radio(on).map_err(|error| error.message);
    force(true)?;
    let result = exchange(wait, from, to, chat_id, "Lab hello", out).await;
    force(false)?;
    result
}

/// `from` sends `text`: it must arrive on `to` and be confirmed within `wait`, with at most one
/// retry.
async fn exchange(
    wait: Duration,
    from: &Side,
    to: &Side,
    chat_id: &str,
    text: &str,
    out: &mut (dyn Write + Send),
) -> Result<(), String> {
    let what = format!("{text:?} from {} to {}", from.label, to.label);
    let retries_before = from.retries();
    let started = Instant::now();
    let sent = from
        .core
        .send_message(chat_id, text)
        .await
        .map_err(|error| format!("{what}: {}", error.message))?;
    if sent.transport != Transport::Radio {
        return Err(format!("{what}: it didn't go by radio"));
    }
    let mut arrived = None;
    let mut confirmed = None;
    let done = within(wait, &what, || {
        if arrived.is_none() {
            let received = to.chat(chat_id)?.messages.into_iter().any(|message| {
                message.id == sent.id
                    && message.direction == Direction::In
                    && message.transport == Transport::Radio
                    && message.text == text
            });
            arrived = received.then(|| started.elapsed());
        }
        let delivery = from
            .chat(chat_id)?
            .messages
            .into_iter()
            .find(|message| message.id == sent.id)?
            .delivery;
        if delivery == Some(DeliveryState::TimedOut) {
            return Some(false);
        }
        if confirmed.is_none() && delivery == Some(DeliveryState::Confirmed) {
            confirmed = Some(started.elapsed());
        }
        (arrived.is_some() && confirmed.is_some()).then_some(true)
    })
    .await;
    let retries = from.retries() - retries_before;
    let progress = match (arrived, confirmed) {
        (Some(_), _) => "it arrived, but no acknowledgement came",
        (None, Some(_)) => "it was acknowledged, but the other app didn't show it",
        (None, None) => "it didn't arrive",
    };
    match (done, arrived, confirmed) {
        (Ok(true), Some(arrived), Some(confirmed)) if retries <= 1 => {
            let estimate = from
                .ports
                .estimate
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "ok {what}: arrived after {:.1} s, acknowledged after {:.1} s, {retries} \
                 retries (the radio estimated {:.1} s)",
                arrived.as_secs_f32(),
                confirmed.as_secs_f32(),
                estimate.as_secs_f32()
            );
            Ok(())
        }
        (Ok(true), ..) => Err(format!("{what}: {retries} retries, more than one")),
        (Ok(false), ..) => Err(format!(
            "{what}: timed out after {retries} retries: {progress}"
        )),
        (Err(error), ..) => Err(format!("{error}: {progress}")),
    }
}

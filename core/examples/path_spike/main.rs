//! The path spike (#36): how the app's radio packets travel between two MeshCore companion
//! radios on one desktop, over each candidate path: text messages, raw data and channel data.
//! Both radios are on USB serial, so one clock times both ends.
//!
//! ```sh
//! cargo run -p peoples-chat-core --example path_spike -- --plan
//! cargo run -p peoples-chat-core --example path_spike -- /dev/cu.usbmodem1101 /dev/cu.usbmodem1201
//! ```
//!
//! It sends format v0 packets of the app's sizes from radio A to radio B, round by round, within
//! radio A's duty cycle. For each send it records the radio's answer, the confirmation, when and
//! how the packet arrived, whether it arrived intact, how long it was on air, and what the
//! firmware source says should happen. It prints a table, writes every send to a CSV file, and
//! puts the radios back as it found them. Steps and the comparison: `docs/radio-paths.md`.

// The core's airtime module (#51), read from its file: the core declares it only once #109 is
// merged, and this example works either way.
#[path = "../../src/radio/airtime.rs"]
mod airtime;
#[cfg(test)]
mod fake;
mod frames;
mod link;
mod plan;
mod report;

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chacha20poly1305::aead::OsRng;
use chacha20poly1305::aead::rand_core::RngCore;
use meshcore_rs::events::{Contact, EventPayload, LogData, SelfInfo};
use meshcore_rs::{EventType, PayloadType};
use peoples_chat_core::radio::RadioSettings;
use peoples_chat_core::radio::format::from_text_message;
use sha2::{Digest, Sha256};
use tokio::time::Instant;

use crate::airtime::{HOURLY_AIRTIME, time_on_air};
use crate::frames::{ChannelDatagram, DATA_TYPE_DEV, Expected, Kind, Route};
use crate::link::{Link, Logged};
use crate::plan::{CHECK_TEXT, Command, Keys, Options, Step, Variant};
use crate::report::{ROUNDS, Row, millis};

/// The name of the private channel the spike sets up on both radios.
const CHANNEL_NAME: &str = "pc-spike";
/// The channel slot when the radio doesn't say how many it has.
const FALLBACK_CHANNEL_INDEX: u8 = 7;
/// MeshCore's contact type for a person's radio.
const CHAT_CONTACT: u8 = 1;
/// The firmware whose source the expectations come from.
const SOURCE_FIRMWARE: &str = "v1.18.0";
const AWAY: &str = "receiver away";
const OFF: &str = "radio off";
/// A received packet shorter than this isn't taken for the start of one that was cut short.
const SHORTEST_CUT: usize = 16;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match plan::parse_args(&args) {
        Ok(Command::ListPorts) => {
            list_ports();
            println!("\n{}", plan::USAGE);
        }
        Ok(Command::Help) => println!("{}", plan::USAGE),
        Ok(Command::Plan(options)) => print_plan(&options),
        Ok(Command::Run(options)) => {
            if let Err(error) = run(options).await {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("{error}\n\n{}", plan::USAGE);
            std::process::exit(2);
        }
    }
}

async fn run(options: Options) -> Result<(), String> {
    let (port_a, port_b) = options.ports.clone().ok_or("a run needs two ports")?;
    let a = Link::serial("A", &port_a).await?;
    let b = Link::serial("B", &port_b).await?;
    let mut spike = Spike::setup(a, b, &options).await?;
    let outline = plan::outline(&options, &spike.settings);
    println!(
        "\n{} sends, {} of them on air ({} floods), {:.0} s on air in all: about {} minutes at \
         {}% of the time. Ctrl-C stops after the current send and still writes the results.\n",
        outline.steps,
        outline.transmissions,
        outline.floods,
        outline.airtime.as_secs_f64(),
        outline.duration.as_secs().div_ceil(60),
        options.duty_cycle
    );
    let stop = stop_on_ctrl_c();
    let mut rows = spike.rounds(&options, &stop).await;
    if options.offline && !stop.load(Ordering::SeqCst) {
        match spike.receiver_away(&port_b, &options).await {
            Ok(more) => rows.extend(more),
            Err(error) => eprintln!("The receiver-away check stopped: {error}"),
        }
    }
    spike.put_back_b().await;
    if options.radio_off && !stop.load(Ordering::SeqCst) {
        rows.extend(spike.radio_off(&options).await);
    }
    spike.put_back_a().await;
    report(&rows, &spike, &options);
    spike.a.close().await;
    spike.b.close().await;
    Ok(())
}

/// The first Ctrl-C asks the run to stop after its current send; a second ends it at once.
fn stop_on_ctrl_c() -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            println!("\nStopping after this send. Ctrl-C again ends at once.");
            flag.store(true, Ordering::SeqCst);
            if tokio::signal::ctrl_c().await.is_ok() {
                std::process::exit(130);
            }
        }
    });
    stop
}

/// The private channel both radios hold for the run.
struct TestChannel {
    index: u8,
    /// The first byte of the secret's SHA-256: how packets on air name the channel.
    hash: u8,
}

/// A packet radio A has handed over.
struct Sent {
    started: Instant,
    counter: u64,
    payload: Vec<u8>,
    airtime: Duration,
    expected: Expected,
    /// The code the confirmation will carry: text messages only.
    ack: Option<[u8; 4]>,
    /// The radio took it to send.
    on_air: bool,
}

/// A packet as radio B passed it on.
struct Receipt {
    at: Instant,
    bytes: Vec<u8>,
    path_byte: Option<u8>,
    snr: Option<f32>,
    rssi: Option<i16>,
}

struct Spike {
    a: Link,
    b: Link,
    key_a: [u8; 32],
    key_b: [u8; 32],
    settings: RadioSettings,
    keys: Keys,
    channel: Option<TestChannel>,
    /// Each radio's transmit power before the run, when the run changed it.
    tx_power: Option<(u8, u8)>,
    /// The repeaters radio A's text messages go through, when the run set its route.
    text_via: Option<Route>,
    duty_cycle: f64,
    counter: u64,
    /// Sends the radio refused, which later rounds skip.
    refused: HashSet<(Variant, usize)>,
}

impl Spike {
    /// Introduces the radios to each other and sets up the test channel, after checking that
    /// they can hear each other and that the channel slot is free.
    async fn setup(a: Link, b: Link, options: &Options) -> Result<Self, String> {
        let (me_a, device_a) = a.hello().await?;
        let (me_b, device_b) = b.hello().await?;
        for (label, me, device) in [("A", &me_a, &device_a), ("B", &me_b, &device_b)] {
            let version = device.as_ref().and_then(|device| device.version.clone());
            println!(
                "Radio {label}: {}, key {}…, firmware {} ({})",
                me.name,
                &hex::encode(me.public_key)[..12],
                version.as_deref().unwrap_or("unknown"),
                device
                    .as_ref()
                    .and_then(|device| device.model.clone())
                    .unwrap_or_else(|| "unknown model".to_owned())
            );
            if !version.is_some_and(|version| version.contains(&SOURCE_FIRMWARE[1..])) {
                println!(
                    "  The expectations come from firmware {SOURCE_FIRMWARE}'s source; this one \
                     may differ."
                );
            }
        }
        let settings = settings(&me_a);
        if (me_a.radio_freq, me_a.radio_bw, me_a.sf, me_a.cr)
            != (me_b.radio_freq, me_b.radio_bw, me_b.sf, me_b.cr)
        {
            return Err(format!(
                "The radios can't hear each other: A is on {}, B on {}. Give both the same preset.",
                describe(&settings),
                describe(&self::settings(&me_b))
            ));
        }
        println!("Both on {}.", describe(&settings));

        // The last slot both radios have: the least likely to be in use.
        let slots = [&device_a, &device_b]
            .iter()
            .filter_map(|device| device.as_ref().and_then(|device| device.max_channels))
            .min();
        let index = options
            .channel_index
            .or(slots.and_then(|count| count.checked_sub(1)))
            .unwrap_or(FALLBACK_CHANNEL_INDEX);
        let wants_channel = options.kinds.contains(&Kind::Channel);
        if wants_channel {
            for link in [&a, &b] {
                free_channel(link, index).await?;
            }
        }

        let mut spike = Self {
            a,
            b,
            key_a: me_a.public_key,
            key_b: me_b.public_key,
            settings,
            keys: Keys::new(),
            channel: None,
            tx_power: None,
            text_via: options
                .via
                .clone()
                .filter(|_| options.kinds.contains(&Kind::Text)),
            duty_cycle: options.duty_cycle,
            counter: 0,
            refused: HashSet::new(),
        };
        if let Some(power) = options.tx_power {
            spike.tx_power = Some((me_a.tx_power, me_b.tx_power));
            for link in [&spike.a, &spike.b] {
                link.commands()
                    .await
                    .set_tx_power(power)
                    .await
                    .map_err(|error| format!("radio {}'s power: {error}", link.label))?;
            }
            println!("Both transmit at {power} dBm for the run.");
        }
        if options.kinds.contains(&Kind::Text) {
            spike.know_each_other(&me_a, &me_b).await?;
        }
        if wants_channel {
            spike.channel = Some(spike.open_channel(index).await?);
        }
        spike.a.collect().await;
        spike.b.collect().await;
        Ok(spike)
    }

    /// Each radio needs the other as a contact for text messages. Through repeaters, radio A's
    /// route to radio B goes through them.
    async fn know_each_other(&self, me_a: &SelfInfo, me_b: &SelfInfo) -> Result<(), String> {
        for (link, other, label) in [(&self.a, me_b, "B"), (&self.b, me_a, "A")] {
            let contacts = link
                .commands()
                .await
                .get_contacts(0)
                .await
                .map_err(|error| format!("radio {}'s contacts: {error}", link.label))?;
            let known = contacts
                .into_iter()
                .find(|contact| contact.public_key == other.public_key);
            let route = if link.label == "A" {
                self.text_via.as_ref()
            } else {
                None
            };
            if known.is_some() && route.is_none() {
                continue;
            }
            let mut contact = known.unwrap_or_else(|| Contact {
                public_key: other.public_key,
                contact_type: CHAT_CONTACT,
                flags: 0,
                path_len: -1,
                out_path: Vec::new(),
                adv_name: other.name.clone(),
                last_advert: unix_now(),
                adv_lat: 0,
                adv_lon: 0,
                last_modification_timestamp: 0,
            });
            if let Some(route) = route {
                contact.path_len = route.path_byte() as i8;
                contact.out_path = route.path().to_vec();
            }
            link.commands()
                .await
                .add_contact(&contact)
                .await
                .map_err(|error| format!("radio {}'s contacts: {error}", link.label))?;
            match route {
                Some(route) => println!("Radio A reaches radio B {}.", route.label()),
                None => println!("Radio {} now has radio {label} as a contact.", link.label),
            }
        }
        Ok(())
    }

    /// Puts a channel with a new random secret in the free slot `index` of both radios.
    async fn open_channel(&self, index: u8) -> Result<TestChannel, String> {
        let mut secret = [0; 16];
        OsRng.fill_bytes(&mut secret);
        for link in [&self.a, &self.b] {
            link.commands()
                .await
                .set_channel(index, CHANNEL_NAME, &secret)
                .await
                .map_err(|error| format!("radio {}'s channel {index}: {error}", link.label))?;
        }
        println!("Both hold the test channel {CHANNEL_NAME:?} in slot {index} for the run.");
        Ok(TestChannel {
            index,
            hash: Sha256::digest(secret)[0],
        })
    }

    /// The main measurement: every send of the schedule, within radio A's duty cycle.
    async fn rounds(&mut self, options: &Options, stop: &AtomicBool) -> Vec<Row> {
        let steps = plan::schedule(options);
        let mut rows = Vec::new();
        for (number, step) in steps.iter().enumerate() {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            if self
                .refused
                .contains(&(step.variant.clone(), step.text_bytes))
            {
                continue;
            }
            let (mut row, sent) = self.transmit(ROUNDS, step).await;
            if sent.on_air {
                self.watch(&mut row, &sent, &step.variant.route).await;
                pause_until(
                    sent.started + plan::spacing(sent.airtime, self.duty_cycle),
                    stop,
                )
                .await;
                self.heard(&mut row, &sent);
            } else if row.outcome == "refused" {
                self.refused.insert((step.variant.clone(), step.text_bytes));
            }
            println!("{:>4}/{} {}", number + 1, steps.len(), line(&row));
            rows.push(row);
        }
        rows
    }

    /// Hands radio A one packet for radio B.
    async fn transmit(&mut self, phase: &str, step: &Step) -> (Row, Sent) {
        let Variant { kind, route } = &step.variant;
        let kind = *kind;
        self.counter += 1;
        let sent_at = unix_now();
        let packet = self.keys.seal(self.counter, step.text_bytes, sent_at);
        let payload = plan::payload(kind, &packet);
        let on_air_bytes = frames::on_air_bytes(kind, route, payload.len());
        let airtime = time_on_air(&self.settings, on_air_bytes);
        let expected = frames::expected(kind, route, payload.len());
        let mut row = Row {
            phase: phase.to_owned(),
            round: step.round,
            kind: Some(kind),
            route: route.label(),
            text_bytes: step.text_bytes,
            packet_bytes: packet.len(),
            payload_bytes: payload.len(),
            on_air_bytes,
            airtime_ms: millis(airtime),
            expected: expected.describe(),
            ..Row::default()
        };
        // A text message takes the contact's route. Without one, the radio floods it.
        if kind == Kind::Text
            && route.is_flood()
            && let Err(error) = self.a.command(&frames::reset_path(&self.key_b)).await
        {
            row.note = format!("forgetting the route: {error}");
        }
        let frame = match kind {
            Kind::Text => frames::text_message(&prefix(&self.key_b), sent_at, &payload),
            Kind::Raw => frames::raw_data(route, &payload),
            Kind::Channel => {
                let index = self.channel.as_ref().map_or(0, |channel| channel.index);
                frames::channel_data(index, route, &payload)
            }
        };
        let replies = match kind {
            Kind::Text => [EventType::MsgSent, EventType::Error],
            Kind::Raw | Kind::Channel => [EventType::Ok, EventType::Error],
        };
        let started = Instant::now();
        let reply = self.a.request(&frame, &replies).await;
        row.reply_ms = Some(millis(started.elapsed()));
        let mut sent = Sent {
            started,
            counter: self.counter,
            payload,
            airtime,
            expected,
            ack: None,
            on_air: false,
        };
        match reply {
            Err(error) => {
                row.reply = "no answer".to_owned();
                row.outcome = "no answer".to_owned();
                row.note = error.to_string();
            }
            Ok(event) => match event.payload {
                EventPayload::String(message) if event.event_type == EventType::Error => {
                    let code = frames::error_code(&message);
                    row.reply = format!("refused: {}", code.map_or("no code", frames::error_name));
                    row.outcome = "refused".to_owned();
                    row.as_expected = code.is_some_and(|code| expected == Expected::Refused(code));
                }
                EventPayload::MsgSent(info) => {
                    row.reply = if info.message_type == 1 {
                        "sent flood"
                    } else {
                        "sent direct"
                    }
                    .to_owned();
                    row.radio_timeout_ms = Some(u64::from(info.suggested_timeout));
                    sent.ack = Some(info.expected_ack);
                    sent.on_air = true;
                }
                _ => {
                    row.reply = "ok".to_owned();
                    sent.on_air = true;
                }
            },
        }
        (row, sent)
    }

    /// Waits for radio B to pass the packet on, and for a text message's confirmation.
    async fn watch(&self, row: &mut Row, sent: &Sent, route: &Route) {
        let kind = row.kind.expect("every send has a path");
        let estimate = row.radio_timeout_ms.map(Duration::from_millis);
        let until = sent.started + plan::patience(route, sent.airtime, estimate);
        let receipt = self.b.log.wait_for(sent.started, until, |logged| {
            self.receipt(kind, &sent.payload, logged)
        });
        let confirmation = self.confirmation(sent, until);
        let (receipt, confirmed) = tokio::join!(receipt, confirmation);
        row.ack_ms = confirmed.map(|at| millis(at - sent.started));
        match receipt {
            Some(receipt) => {
                self.received(row, sent, &receipt);
                row.as_expected = match sent.expected {
                    // A text message is also confirmed.
                    Expected::Delivered => {
                        row.intact == Some(true) && (sent.ack.is_none() || confirmed.is_some())
                    }
                    Expected::Cut => row.intact == Some(false),
                    Expected::Lost | Expected::Refused(_) => false,
                };
            }
            None => {
                row.outcome = "lost".to_owned();
                row.as_expected = sent.expected == Expected::Lost;
            }
        }
        if kind == Kind::Text
            && route.is_flood()
            && self
                .a
                .log
                .since(sent.started)
                .iter()
                .any(|logged| logged.event.event_type == EventType::PathUpdate)
        {
            row.note = join_note(&row.note, "radio A learned the route");
        }
    }

    /// When radio A reported the confirmation of `sent`, if it did by `until`.
    async fn confirmation(&self, sent: &Sent, until: Instant) -> Option<Instant> {
        let tag = sent.ack?;
        self.a
            .log
            .wait_for(sent.started, until, |logged| match &logged.event.payload {
                EventPayload::Ack { tag: got } if *got == tag => Some(logged.at),
                _ => None,
            })
            .await
    }

    /// Fills in what radio B passed on.
    fn received(&self, row: &mut Row, sent: &Sent, receipt: &Receipt) {
        let kind = row.kind.expect("every send has a path");
        row.delivery_ms = Some(millis(receipt.at - sent.started));
        row.received_route = receipt
            .path_byte
            .map(frames::received_route)
            .unwrap_or_default();
        row.snr = receipt.snr;
        row.rssi = receipt.rssi;
        let packet = match kind {
            Kind::Text => std::str::from_utf8(&receipt.bytes)
                .ok()
                .and_then(|text| from_text_message(text).ok()),
            Kind::Raw | Kind::Channel => Some(receipt.bytes.clone()),
        };
        let intact = receipt.bytes == sent.payload
            && packet.is_some_and(|packet| self.keys.opens(&packet, sent.counter, row.text_bytes));
        row.intact = Some(intact);
        row.outcome = if intact { "delivered" } else { "cut short" }.to_owned();
    }

    /// `logged` as the packet `payload` arriving at radio B, or the start of it if the sending
    /// radio cut it short.
    fn receipt(&self, kind: Kind, payload: &[u8], logged: &Logged) -> Option<Receipt> {
        let (bytes, path_byte, snr, rssi) = match (&logged.event.payload, kind) {
            (EventPayload::ContactMessage(message), Kind::Text)
                if message.sender_prefix[..] == self.key_a[..6] =>
            {
                (
                    message.text.clone().into_bytes(),
                    Some(message.path_len),
                    message.snr,
                    None,
                )
            }
            (EventPayload::RawData(raw), Kind::Raw) => {
                (raw.payload.clone(), None, Some(raw.snr), Some(raw.rssi))
            }
            (EventPayload::Bytes(frame), Kind::Channel) => {
                let datagram = ChannelDatagram::parse(frame)?;
                let index = self.channel.as_ref()?.index;
                if datagram.channel != index || datagram.data_type != DATA_TYPE_DEV {
                    return None;
                }
                (
                    datagram.data,
                    Some(datagram.path_byte),
                    Some(datagram.snr),
                    None,
                )
            }
            _ => return None,
        };
        let same = bytes == payload || (bytes.len() >= SHORTEST_CUT && payload.starts_with(&bytes));
        same.then_some(Receipt {
            at: logged.at,
            bytes,
            path_byte,
            snr,
            rssi,
        })
    }

    /// Counts the copies of the packet that radio B heard on air since it went, and takes the
    /// first one's length. A radio logs every packet it hears, up to 173 bytes.
    fn heard(&self, row: &mut Row, sent: &Sent) {
        let kind = row.kind.expect("every send has a path");
        let ours = |log: &LogData| {
            let Some(header) = &log.header else {
                return false;
            };
            match kind {
                // Addressed from radio A to radio B: the first byte of each key.
                Kind::Text => {
                    header.payload_type == PayloadType::TextMsg
                        && log.payload.get(..2) == Some(&[self.key_b[0], self.key_a[0]])
                }
                Kind::Raw => {
                    header.payload_type == PayloadType::RawCustom && log.payload == sent.payload
                }
                Kind::Channel => {
                    header.payload_type == PayloadType::GroupData
                        && log.payload.first() == self.channel.as_ref().map(|channel| &channel.hash)
                }
            }
        };
        let copies: Vec<usize> = self
            .b
            .log
            .since(sent.started)
            .iter()
            .filter_map(|logged| match &logged.event.payload {
                EventPayload::LogData(log) if ours(log) => Some(frames::logged_bytes(log)),
                _ => None,
            })
            .flatten()
            .collect();
        row.copies_heard = copies.len();
        row.logged_bytes = copies.first().copied();
    }

    /// The check with radio B's link closed: its radio keeps running, and what it queued
    /// arrives when the link opens again.
    async fn receiver_away(&mut self, port_b: &str, options: &Options) -> Result<Vec<Row>, String> {
        println!("\nClosing radio B's link; the radio keeps running.");
        self.b.close().await;
        let mut sends = Vec::new();
        for step in self.checks(options) {
            let (mut row, sent) = self.transmit(AWAY, &step).await;
            if sent.on_air {
                let until = sent.started
                    + plan::patience(
                        &step.variant.route,
                        sent.airtime,
                        row.radio_timeout_ms.map(Duration::from_millis),
                    );
                row.ack_ms = self
                    .confirmation(&sent, until)
                    .await
                    .map(|at| millis(at - sent.started));
                tokio::time::sleep_until(
                    sent.started + plan::spacing(sent.airtime, self.duty_cycle),
                )
                .await;
            }
            sends.push((row, sent));
        }
        println!("Opening radio B's link again.");
        self.b = Link::serial("B", port_b).await?;
        self.b.hello().await?;
        let opened = Instant::now();
        self.b.collect().await;
        let mut rows = Vec::new();
        for (mut row, sent) in sends {
            let kind = row.kind.expect("every send has a path");
            // USB counts as connected even when the port is closed: raw data is pushed once, to
            // nobody. Text and channel datagrams wait in the radio's queue.
            let queued = kind != Kind::Raw;
            row.expected = if queued {
                "queued on the radio"
            } else {
                "lost"
            }
            .to_owned();
            if sent.on_air {
                let receipt = self
                    .b
                    .log
                    .wait_for(opened, opened + Duration::from_secs(3), |logged| {
                        self.receipt(kind, &sent.payload, logged)
                    })
                    .await;
                match receipt {
                    Some(receipt) => {
                        self.received(&mut row, &sent, &receipt);
                        row.outcome = "queued on the radio".to_owned();
                    }
                    None => row.outcome = "lost".to_owned(),
                }
                row.as_expected = row.delivered() == queued;
            }
            println!("     {}", line(&row));
            rows.push(row);
        }
        Ok(rows)
    }

    /// The check with radio B switched off: what radio A's side can tell.
    async fn radio_off(&mut self, options: &Options) -> Vec<Row> {
        prompt(
            "\nSwitch radio B off now: unplugging it is not enough if it has a battery. Press \
             Enter when it is off.",
        )
        .await;
        let mut rows = Vec::new();
        for step in self.checks(options) {
            let (mut row, sent) = self.transmit(OFF, &step).await;
            let kind = row.kind.expect("every send has a path");
            row.expected = match kind {
                Kind::Text => "no confirmation",
                Kind::Raw | Kind::Channel => "taken, with no sign of the loss",
            }
            .to_owned();
            if sent.on_air {
                let estimate = row.radio_timeout_ms.map(Duration::from_millis);
                let patience = plan::patience(&step.variant.route, sent.airtime, estimate)
                    .max(Duration::from_secs(10));
                let confirmed = self.confirmation(&sent, sent.started + patience).await;
                row.ack_ms = confirmed.map(|at| millis(at - sent.started));
                row.outcome = match (kind, confirmed) {
                    (Kind::Text, None) => "no confirmation",
                    (Kind::Text, Some(_)) => "confirmed",
                    _ => "taken",
                }
                .to_owned();
                row.as_expected = confirmed.is_none();
                tokio::time::sleep_until(
                    sent.started + plan::spacing(sent.airtime, self.duty_cycle),
                )
                .await;
            }
            println!("     {}", line(&row));
            rows.push(row);
        }
        println!("You can switch radio B on again.");
        rows
    }

    /// One medium-sized direct send on each path, for the receiver-away and radio-off checks.
    fn checks(&self, options: &Options) -> Vec<Step> {
        plan::variants(options)
            .into_iter()
            .filter(|variant| !variant.route.is_flood())
            .map(|variant| Step {
                round: 0,
                variant,
                text_bytes: CHECK_TEXT,
            })
            .collect()
    }

    async fn put_back_b(&self) {
        self.put_back(&self.b, self.tx_power.map(|(_, b)| b)).await;
    }

    /// Puts radio A back, and lets it find its own route to radio B again.
    async fn put_back_a(&self) {
        self.put_back(&self.a, self.tx_power.map(|(a, _)| a)).await;
        if self.text_via.is_some()
            && let Err(error) = self.a.command(&frames::reset_path(&self.key_b)).await
        {
            eprintln!("Radio A still routes to radio B through the repeaters: {error}");
        }
    }

    /// Empties the test channel's slot, and sets the transmit power back.
    async fn put_back(&self, link: &Link, power: Option<u8>) {
        if let Some(channel) = &self.channel
            && let Err(error) = link
                .commands()
                .await
                .set_channel(channel.index, "", &[0; 16])
                .await
        {
            eprintln!(
                "Radio {} still holds the test channel in slot {}: {error}",
                link.label, channel.index
            );
        }
        if let Some(power) = power
            && let Err(error) = link.commands().await.set_tx_power(power).await
        {
            eprintln!(
                "Radio {} still transmits at the run's power: {error}",
                link.label
            );
        }
    }
}

/// Fails unless channel slot `index` is empty, or holds a test channel from an earlier run.
async fn free_channel(link: &Link, index: u8) -> Result<(), String> {
    let slot = link
        .commands()
        .await
        .get_channel(index)
        .await
        .map_err(|error| format!("radio {} has no channel slot {index}: {error}", link.label))?;
    if slot.name == CHANNEL_NAME || (slot.name.is_empty() && slot.secret == [0; 16]) {
        Ok(())
    } else {
        Err(format!(
            "Channel slot {index} on radio {} holds {:?}. Pick a free one with --channel-index.",
            link.label, slot.name
        ))
    }
}

/// Sleeps until `deadline`, or until the run is asked to stop.
async fn pause_until(deadline: Instant, stop: &AtomicBool) {
    while Instant::now() < deadline && !stop.load(Ordering::SeqCst) {
        let step = deadline.min(Instant::now() + Duration::from_millis(250));
        tokio::time::sleep_until(step).await;
    }
}

async fn prompt(message: &str) {
    println!("{message}");
    let _ = tokio::task::spawn_blocking(|| std::io::stdin().read_line(&mut String::new())).await;
}

fn report(rows: &[Row], spike: &Spike, options: &Options) {
    println!("\n{}", report::table(rows));
    println!(
        "Longest chat text that arrived intact every time, direct (the spec aims for 100 to 150 bytes):"
    );
    for (kind, budget) in report::budgets(rows) {
        let budget = budget.map_or("none".to_owned(), |bytes| format!("{bytes} bytes"));
        println!("  {:<8} {budget}", kind.name());
    }
    let on_air: f64 = rows
        .iter()
        .filter(|row| row.went_on_air())
        .map(|row| row.airtime_ms)
        .sum();
    println!(
        "Radio A was on air for {:.0} s, worked out from the packets. The app allows itself {} s an \
         hour (#51).",
        on_air / 1000.0,
        HOURLY_AIRTIME.as_secs()
    );
    let differ = rows.iter().filter(|row| !row.as_expected).count();
    if differ > 0 {
        println!("{differ} sends went otherwise than the firmware source says: see the table.");
    }
    let missed = spike.a.log.missed() + spike.b.log.missed();
    if missed > 0 {
        println!("The logs fell behind and missed {missed} events: the results may be short.");
    }
    let out = options
        .out
        .clone()
        .unwrap_or_else(|| format!("path-spike-{}.csv", unix_now()).into());
    match std::fs::write(&out, report::csv(rows, missed)) {
        Ok(()) => println!("Every send: {}", out.display()),
        Err(error) => eprintln!("Could not write {}: {error}", out.display()),
    }
}

/// One line about a send, for following the run.
fn line(row: &Row) -> String {
    let mut line = format!(
        "{} {}, {} B of text: {}",
        row.path(),
        row.route,
        row.text_bytes,
        row.outcome
    );
    if let Some(ms) = row.delivery_ms {
        line.push_str(&format!(" in {ms:.0} ms"));
    }
    if let Some(ms) = row.ack_ms {
        line.push_str(&format!(", confirmed in {ms:.0} ms"));
    }
    if row.outcome == "refused" {
        line.push_str(&format!(" ({})", row.reply));
    }
    if !row.as_expected {
        line.push_str(&format!(" [the source says: {}]", row.expected));
    }
    line
}

fn join_note(note: &str, more: &str) -> String {
    if note.is_empty() {
        more.to_owned()
    } else {
        format!("{note}; {more}")
    }
}

/// What `--plan` shows: every distinct send, and the totals.
fn print_plan(options: &Options) {
    let settings = plan::EU_UK_NARROW;
    println!(
        "Assuming MeshCore's EU/UK narrow preset, {}. A run uses the radios' own settings.\n",
        describe(&settings)
    );
    let mut rounds: BTreeMap<(Kind, String, usize), (Step, usize)> = BTreeMap::new();
    for step in plan::schedule(options) {
        let key = (
            step.variant.kind,
            step.variant.route.label(),
            step.text_bytes,
        );
        rounds.entry(key).or_insert((step, 0)).1 += 1;
    }
    let mut rows = vec![
        [
            "path",
            "route",
            "text",
            "packet",
            "payload",
            "on air",
            "airtime",
            "rounds",
            "the source says",
        ]
        .map(str::to_owned),
    ];
    for ((kind, route, text_bytes), (step, count)) in rounds {
        let payload = plan::payload_bytes(kind, text_bytes);
        let expected = frames::expected(kind, &step.variant.route, payload);
        // A refused send isn't repeated, and never goes on air.
        let (on_air, airtime, count) = if matches!(expected, Expected::Refused(_)) {
            ("-".to_owned(), "-".to_owned(), 1)
        } else {
            (
                format!(
                    "{} B",
                    frames::on_air_bytes(kind, &step.variant.route, payload)
                ),
                format!("{:.0} ms", millis(plan::airtime(&settings, &step))),
                count,
            )
        };
        rows.push([
            kind.name().to_owned(),
            route,
            format!("{text_bytes} B"),
            format!("{} B", plan::packet_bytes(text_bytes)),
            format!("{payload} B"),
            on_air,
            airtime,
            count.to_string(),
            expected.describe(),
        ]);
    }
    let widths: Vec<usize> = (0..rows[0].len())
        .map(|column| rows.iter().map(|row| row[column].len()).max().unwrap_or(0))
        .collect();
    for row in &rows {
        let line: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(field, width)| format!("{field:<width$}"))
            .collect();
        println!("{}", line.join("  ").trim_end());
    }
    let direct = options.via.clone().unwrap_or_else(Route::zero_hop);
    let budgets: Vec<String> = options
        .kinds
        .iter()
        .map(|&kind| {
            let budget = options
                .texts
                .iter()
                .filter(|&&text| {
                    let payload = plan::payload_bytes(kind, text);
                    frames::expected(kind, &direct, payload) == Expected::Delivered
                })
                .max();
            let budget = budget.map_or("none".to_owned(), |bytes| format!("{bytes} bytes"));
            format!("{} {budget}", kind.name())
        })
        .collect();
    println!(
        "\nBy the source, the longest of these chat texts each path delivers: {}.",
        budgets.join(", ")
    );
    let outline = plan::outline(options, &settings);
    println!(
        "{} sends, {} of them on air ({} floods): {:.0} s on air, about {} minutes at {}% of the \
         time.",
        outline.steps,
        outline.transmissions,
        outline.floods,
        outline.airtime.as_secs_f64(),
        outline.duration.as_secs().div_ceil(60),
        options.duty_cycle
    );
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
        }
        Ok(_) => println!("No serial ports found. Plug in two MeshCore radios over USB."),
        Err(error) => eprintln!("Could not list serial ports: {error}"),
    }
}

/// The radio reports the frequency in kHz and the bandwidth in Hz.
fn settings(me: &SelfInfo) -> RadioSettings {
    RadioSettings {
        frequency_mhz: f64::from(me.radio_freq) / 1000.0,
        bandwidth_khz: f64::from(me.radio_bw) / 1000.0,
        spreading_factor: me.sf,
        coding_rate: me.cr,
        tx_power_dbm: me.tx_power,
    }
}

fn describe(settings: &RadioSettings) -> String {
    format!(
        "{} MHz, {} kHz, spreading factor {}, coding rate 4/{}",
        settings.frequency_mhz,
        settings.bandwidth_khz,
        settings.spreading_factor,
        settings.coding_rate
    )
}

fn prefix(key: &[u8; 32]) -> [u8; 6] {
    let mut prefix = [0; 6];
    prefix.copy_from_slice(&key[..6]);
    prefix
}

fn unix_now() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeMesh;
    use crate::report::budgets;

    fn options() -> Options {
        Options {
            texts: vec![10, 96, 97, 141, 142, 148, 149],
            repeat: 2,
            flood_repeat: 1,
            ..Options::default()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn against_radios_that_follow_the_source_every_send_goes_as_expected() {
        let mesh = FakeMesh::new();
        let options = options();
        let mut spike = Spike::setup(mesh.link_a(), mesh.link_b(), &options)
            .await
            .unwrap();
        assert_eq!(mesh.channel_slot(0, 7), CHANNEL_NAME);
        let started = Instant::now();
        let rows = spike.rounds(&options, &AtomicBool::new(false)).await;
        let unexpected: Vec<_> = rows.iter().filter(|row| !row.as_expected).collect();
        assert!(unexpected.is_empty(), "{unexpected:#?}");
        // The budgets the source gives each path.
        assert_eq!(
            budgets(&rows),
            [
                (Kind::Text, Some(96)),
                (Kind::Raw, Some(148)),
                (Kind::Channel, Some(141))
            ]
        );
        // The first text floods and teaches radio A the route; the others go direct.
        let texts: Vec<_> = rows
            .iter()
            .filter(|row| row.kind == Some(Kind::Text))
            .collect();
        assert_eq!(texts[0].reply, "sent flood");
        assert!(
            texts[0].note.contains("learned the route"),
            "{:?}",
            texts[0].note
        );
        assert_eq!(texts[0].received_route, "flood, 0 hops");
        let direct = texts.iter().find(|row| row.route == "zero hop").unwrap();
        assert_eq!(direct.reply, "sent direct");
        assert_eq!(direct.received_route, "direct");
        assert!(direct.ack_ms.is_some());
        // Refused sends aren't repeated: text of 97 bytes is refused once, not twice.
        let refused = rows
            .iter()
            .filter(|row| row.kind == Some(Kind::Text) && row.text_bytes == 97)
            .count();
        assert_eq!(refused, 1);
        // Raw data of 149 bytes goes on air and never arrives.
        let lost = rows
            .iter()
            .find(|row| row.kind == Some(Kind::Raw) && row.text_bytes == 149)
            .unwrap();
        assert_eq!((lost.reply.as_str(), lost.outcome.as_str()), ("ok", "lost"));
        // The receiving radio's log agrees with the worked-out sizes, where it logs them.
        let logged: Vec<_> = rows
            .iter()
            .filter(|row| row.logged_bytes.is_some())
            .collect();
        assert!(!logged.is_empty());
        assert!(
            logged
                .iter()
                .all(|row| row.logged_bytes == Some(row.on_air_bytes)),
            "{logged:#?}"
        );
        // Radio A kept to 10% of the time on air.
        let on_air: f64 = rows
            .iter()
            .filter(|row| row.outcome != "refused")
            .map(|row| row.airtime_ms)
            .sum();
        assert!(millis(started.elapsed()) >= on_air * 10.0 * 0.95);

        spike.put_back_b().await;
        spike.put_back_a().await;
        assert_eq!(mesh.channel_slot(0, 7), "");
        assert_eq!(mesh.channel_slot(1, 7), "");
        let table = report::table(&rows);
        assert!(table.lines().count() > 10, "{table}");
        assert_eq!(report::csv(&rows, 0).lines().count(), rows.len() + 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_used_channel_slot_stops_the_setup() {
        let mesh = FakeMesh::new();
        mesh.set_channel_slot(1, 7, "Friends");
        let error = Spike::setup(mesh.link_a(), mesh.link_b(), &options())
            .await
            .err()
            .unwrap();
        assert!(error.contains("holds \"Friends\""), "{error}");
        // Nothing was changed on radio A.
        assert_eq!(mesh.channel_slot(0, 7), "");
    }

    #[tokio::test(start_paused = true)]
    async fn through_a_repeater_radio_a_routes_by_the_path_and_forgets_it_after() {
        let mesh = FakeMesh::new();
        let options = Options {
            via: Some(Route::via("c3", 1).unwrap()),
            texts: vec![10],
            repeat: 1,
            ..Options::default()
        };
        let mut spike = Spike::setup(mesh.link_a(), mesh.link_b(), &options)
            .await
            .unwrap();
        assert_eq!(mesh.route_to_peer(0), Some(vec![0xc3]));
        let rows = spike.rounds(&options, &AtomicBool::new(false)).await;
        assert_eq!(rows.len(), 3);
        assert!(
            rows.iter()
                .all(|row| row.route == "via c3" && row.as_expected),
            "{rows:#?}"
        );
        assert_eq!(rows[0].reply, "sent direct");
        spike.put_back_a().await;
        assert_eq!(mesh.route_to_peer(0), None);
    }
}

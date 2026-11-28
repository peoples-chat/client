//! How often the lab radios advertise on their own (#140, for the one-hour window of #44).
//!
//! Both radios stay open and nothing is sent: each reports every packet it hears
//! (`LOG_RX_DATA`), the other radio's adverts among them. At the end each radio is told to send
//! one zero-hop advert, which shows that the listening would have caught one.

use std::io::Write;
use std::time::Duration;

use meshcore_rs::events::{EventPayload, MeshCoreEvent};
use meshcore_rs::{EventType, PayloadType};
use tokio::sync::broadcast::{Receiver, error::RecvError};
use tokio::time::Instant;

use crate::lab::Lab;
use crate::link::Link;

/// How long to wait for the advert each radio is told to send at the end.
const CONTROL_WAIT: Duration = Duration::from_secs(5);

/// One advert a radio heard.
#[derive(Debug, PartialEq, Eq)]
pub struct Heard {
    /// After the listening started.
    pub after: Duration,
    /// The advertiser's public key, in hex.
    pub from: String,
}

/// What one radio heard.
#[derive(Debug)]
#[allow(dead_code)] // The run prints these as it goes; the tests read them.
pub struct Listened {
    pub label: String,
    /// The other lab radio's public key, in hex.
    pub other_key: String,
    /// Adverts heard before any radio was told to send one.
    pub unprompted: Vec<Heard>,
    /// The other radio's advert arrived when it was told to send it.
    pub control_heard: bool,
}

struct Ear {
    label: String,
    other_key: String,
    link: Link,
    events: Receiver<MeshCoreEvent>,
}

/// The advertiser of an advert in the radio's log.
fn advertiser(event: &MeshCoreEvent) -> Option<String> {
    let EventPayload::LogData(log) = &event.payload else {
        return None;
    };
    let advert = log.header.as_ref()?.payload_type == PayloadType::Advert;
    (advert && log.payload.len() >= 32).then(|| hex::encode(&log.payload[..32]))
}

/// The adverts on `events` until `until`.
async fn listen(
    events: &mut Receiver<MeshCoreEvent>,
    label: &str,
    started: Instant,
    until: Instant,
) -> Result<Vec<Heard>, String> {
    let mut heard = Vec::new();
    loop {
        match tokio::time::timeout_at(until, events.recv()).await {
            Err(_) => return Ok(heard),
            Ok(Ok(event)) if event.event_type == EventType::Disconnected => {
                return Err(format!("{label}: the link dropped while listening"));
            }
            Ok(Ok(event)) => {
                if let Some(from) = advertiser(&event) {
                    let after = started.elapsed();
                    println!(
                        "{label} heard an advert from {}… after {} s",
                        &from[..12],
                        after.as_secs()
                    );
                    heard.push(Heard { after, from });
                }
            }
            Ok(Err(RecvError::Lagged(missed))) => {
                return Err(format!("{label}: missed {missed} events while listening"));
            }
            Ok(Err(RecvError::Closed)) => return Err(format!("{label}: the link closed")),
        }
    }
}

impl Lab<'_> {
    /// Checks the lab, then listens on its first two USB radios for `period`.
    pub async fn adverts(
        &self,
        period: Duration,
        out: &mut (dyn Write + Send),
    ) -> Result<Vec<Listened>, String> {
        let mut found = Vec::new();
        for station in self.stations(false, out).await? {
            found.push((
                station.entry.label.clone(),
                station.port.clone(),
                station.info.public_key.to_lowercase(),
            ));
            station.close().await;
        }
        let [first, second, ..] = &found[..] else {
            return Err("listening needs two USB radios in radios.toml".to_owned());
        };
        let mut ears = Vec::new();
        for ((label, port, _), (_, _, other_key)) in [(first, second), (second, first)] {
            let stream = self
                .usb
                .open(port)
                .map_err(|error| format!("{label}: could not open {port}: {error}"))?;
            let link = Link::open(label, stream, self.log.clone(), self.timing.reply);
            let events = link.dispatcher.receiver();
            {
                // As companion apps greet a radio: it then reports what it hears.
                let commands = link.commands.lock().await;
                commands
                    .send_appstart()
                    .await
                    .map_err(|error| format!("{label}: {error}"))?;
                commands
                    .send_device_query()
                    .await
                    .map_err(|error| format!("{label}: {error}"))?;
            }
            ears.push(Ear {
                label: label.clone(),
                other_key: other_key.clone(),
                link,
                events,
            });
        }
        let _ = writeln!(
            out,
            "Listening for adverts for {} s, sending nothing.",
            period.as_secs()
        );
        let started = Instant::now();
        let until = started + period;
        let [a, b] = &mut ears[..] else {
            return Err("listening needs two radios".to_owned());
        };
        let (quiet_a, quiet_b) = tokio::join!(
            listen(&mut a.events, &a.label, started, until),
            listen(&mut b.events, &b.label, started, until),
        );
        let quiet = [quiet_a?, quiet_b?];

        let _ = writeln!(out, "Telling each radio to send one zero-hop advert.");
        let mut controls = [false; 2];
        // Radio `sender` advertises, and the other one listens.
        for sender in [0, 1] {
            let hearer = 1 - sender;
            ears[sender]
                .link
                .commands
                .lock()
                .await
                .send_advert(false)
                .await
                .map_err(|error| format!("{}: {error}", ears[sender].label))?;
            let ear = &mut ears[hearer];
            let until = Instant::now() + CONTROL_WAIT;
            let heard = listen(&mut ear.events, &ear.label, started, until).await?;
            controls[hearer] = heard.iter().any(|heard| heard.from == ear.other_key);
        }

        let mut listened = Vec::new();
        for ((ear, unprompted), control_heard) in ears.into_iter().zip(quiet).zip(controls) {
            let from_other = unprompted
                .iter()
                .filter(|heard| heard.from == ear.other_key)
                .count();
            let _ = writeln!(
                out,
                "{}: in {} s, {from_other} adverts from the other lab radio and {} from others; \
                 its prompted advert {}",
                ear.label,
                period.as_secs(),
                unprompted.len() - from_other,
                if control_heard {
                    "was heard"
                } else {
                    "was NOT heard"
                }
            );
            ear.link.close().await;
            listened.push(Listened {
                label: ear.label,
                other_key: ear.other_key,
                unprompted,
                control_heard,
            });
        }
        if listened.iter().any(|radio| !radio.control_heard) {
            return Err(
                "a prompted advert wasn't heard, so the silence before it proves nothing"
                    .to_owned(),
            );
        }
        Ok(listened)
    }
}

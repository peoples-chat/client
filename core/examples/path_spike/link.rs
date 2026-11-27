//! One radio on its link: everything it reports, with when it arrived, and its queue collected
//! as soon as it says that messages wait.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use meshcore_rs::commands::CommandHandler;
use meshcore_rs::events::{DeviceInfoData, EventDispatcher, EventPayload, SelfInfo};
use meshcore_rs::{EventType, MeshCore, MeshCoreEvent};
use tokio::sync::{Notify, broadcast};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::frames::{self, SYNC_NEXT_MESSAGE};

/// MeshCore companion radios talk at this rate over USB serial.
const BAUD_RATE: u32 = 115_200;
/// How long the radio may take to answer a command.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// The radio's queue holds 16 frames; collecting more than this in a row means something loops.
const MOST_COLLECTED: usize = 64;

/// What a radio reported, with when it arrived.
#[derive(Clone, Debug)]
pub struct Logged {
    pub at: Instant,
    pub event: MeshCoreEvent,
}

/// Everything a radio reported since its link opened.
#[derive(Default)]
pub struct EventLog {
    events: Mutex<Vec<Logged>>,
    added: Notify,
    missed: Mutex<u64>,
}

impl EventLog {
    fn record(&self, event: MeshCoreEvent) {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Logged {
                at: Instant::now(),
                event,
            });
        self.added.notify_waiters();
    }

    /// The events that arrived from `from` on.
    pub fn since(&self, from: Instant) -> Vec<Logged> {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|logged| logged.at >= from)
            .cloned()
            .collect()
    }

    /// The first event from `from` on that `find` picks, waiting for it until `until`.
    pub async fn wait_for<T>(
        &self,
        from: Instant,
        until: Instant,
        mut find: impl FnMut(&Logged) -> Option<T>,
    ) -> Option<T> {
        loop {
            let added = self.added.notified();
            tokio::pin!(added);
            // Listen before looking, so that an event between the two isn't missed.
            added.as_mut().enable();
            if let Some(found) = self.since(from).iter().find_map(&mut find) {
                return Some(found);
            }
            if tokio::time::timeout_at(until, added).await.is_err() {
                return None;
            }
        }
    }

    /// How many events the log fell too far behind to see.
    pub fn missed(&self) -> u64 {
        *self.missed.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub struct Link {
    pub label: &'static str,
    commands: Arc<tokio::sync::Mutex<CommandHandler>>,
    pub log: Arc<EventLog>,
    tasks: Vec<JoinHandle<()>>,
    /// The serial link, when this link opened it.
    serial: Option<MeshCore>,
    /// Closes a link run elsewhere, such as the lab harness's. Dropping it unpolled drops that
    /// link too.
    closer: Option<Closer>,
}

/// What ends a link run elsewhere.
pub type Closer = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

impl Link {
    /// Opens a companion radio on a USB serial port.
    pub async fn serial(label: &'static str, port: &str) -> Result<Self, String> {
        let serial = MeshCore::serial(port, BAUD_RATE)
            .await
            .map_err(|error| format!("radio {label} on {port}: {error}"))?;
        let commands = serial.commands().clone();
        let dispatcher = serial.dispatcher().clone();
        let mut link = Self::over(label, commands, &dispatcher);
        link.serial = Some(serial);
        Ok(link)
    }

    /// A radio behind meshcore-rs's command handler and event dispatcher, for a link run
    /// elsewhere: the test's fake radios.
    pub fn over(
        label: &'static str,
        commands: Arc<tokio::sync::Mutex<CommandHandler>>,
        dispatcher: &Arc<EventDispatcher>,
    ) -> Self {
        let log = Arc::new(EventLog::default());
        let recorder = tokio::spawn(record(dispatcher.receiver(), log.clone()));
        let collector = tokio::spawn(collect_on_notice(dispatcher.receiver(), commands.clone()));
        Self {
            label,
            commands,
            log,
            tasks: vec![recorder, collector],
            serial: None,
            closer: None,
        }
    }

    /// Has [`Link::close`] run `closer` too: it closes the link this one runs over.
    #[allow(dead_code)] // The lab harness (`verify:radio`) uses it; the example alone doesn't.
    pub fn closing(mut self, closer: Closer) -> Self {
        self.closer = Some(closer);
        self
    }

    /// Greets the radio as companion apps do, and asks what it is. Asking for the device
    /// information also makes the radio send messages with their signal strength.
    pub async fn hello(&self) -> Result<(SelfInfo, Option<DeviceInfoData>), String> {
        let commands = self.commands.lock().await;
        let me = commands
            .send_appstart()
            .await
            .map_err(|error| format!("radio {} didn't answer: {error}", self.label))?;
        Ok((me, commands.send_device_query().await.ok()))
    }

    /// Sends a frame the spike built, and returns the radio's answer: the first of `replies`.
    pub async fn request(
        &self,
        frame: &[u8],
        replies: &[EventType],
    ) -> Result<MeshCoreEvent, meshcore_rs::Error> {
        self.commands
            .lock()
            .await
            .send_multi(frame, replies, REPLY_TIMEOUT)
            .await
    }

    /// Sends a frame the spike built that the radio answers with OK, unless it refuses.
    pub async fn command(&self, frame: &[u8]) -> Result<(), String> {
        let reply = self
            .request(frame, &[EventType::Ok, EventType::Error])
            .await
            .map_err(|error| error.to_string())?;
        match reply.payload {
            EventPayload::String(message) if reply.event_type == EventType::Error => {
                let code = frames::error_code(&message);
                Err(format!(
                    "refused: {}",
                    code.map_or("no code", frames::error_name)
                ))
            }
            _ => Ok(()),
        }
    }

    /// meshcore-rs's own commands, for what it does right.
    pub async fn commands(&self) -> tokio::sync::MutexGuard<'_, CommandHandler> {
        self.commands.lock().await
    }

    /// Collects what waits on the radio. The radio says when messages arrive, but not about
    /// those that waited before the link opened.
    pub async fn collect(&self) {
        collect(&self.commands).await;
    }

    /// Closes the link. The radio keeps running.
    pub async fn close(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
        if let Some(serial) = self.serial.take()
            && let Err(error) = serial.disconnect().await
        {
            eprintln!("closing radio {}'s link: {error}", self.label);
        }
        if let Some(closer) = self.closer.take() {
            closer.await;
        }
    }
}

async fn record(mut events: broadcast::Receiver<MeshCoreEvent>, log: Arc<EventLog>) {
    loop {
        match events.recv().await {
            Ok(event) => log.record(event),
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                *log.missed.lock().unwrap_or_else(PoisonError::into_inner) += missed;
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Collects the radio's queue whenever it says that messages wait. It holds 16 frames, and a
/// full queue drops new direct messages.
async fn collect_on_notice(
    mut events: broadcast::Receiver<MeshCoreEvent>,
    commands: Arc<tokio::sync::Mutex<CommandHandler>>,
) {
    loop {
        match events.recv().await {
            Ok(event) if event.event_type == EventType::MessagesWaiting => collect(&commands).await,
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(_)) => collect(&commands).await,
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

async fn collect(commands: &tokio::sync::Mutex<CommandHandler>) {
    // meshcore-rs's own `get_msg` doesn't know channel datagrams, and waits for its timeout on
    // each. They arrive as unknown frames; the log keeps them, and this moves on.
    let replies = [
        EventType::ContactMsgRecv,
        EventType::ChannelMsgRecv,
        EventType::NoMoreMessages,
        EventType::Error,
        EventType::Unknown,
    ];
    for _ in 0..MOST_COLLECTED {
        let reply = commands
            .lock()
            .await
            .send_multi(&[SYNC_NEXT_MESSAGE], &replies, REPLY_TIMEOUT)
            .await;
        match reply {
            Ok(event)
                if matches!(
                    event.event_type,
                    EventType::NoMoreMessages | EventType::Error
                ) =>
            {
                return;
            }
            Ok(_) => {}
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn the_log_finds_what_arrives_while_it_waits() {
        let log = Arc::new(EventLog::default());
        let start = Instant::now();
        let later = log.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            later.record(MeshCoreEvent::ok());
        });
        let found = log
            .wait_for(start, start + Duration::from_secs(1), |logged| {
                (logged.event.event_type == EventType::Ok).then_some(logged.at)
            })
            .await;
        assert_eq!(found, Some(start + Duration::from_millis(300)));
        // Nothing else arrives: it gives up at the deadline.
        let none = log
            .wait_for(start, start + Duration::from_secs(2), |logged| {
                (logged.event.event_type == EventType::Error).then_some(())
            })
            .await;
        assert_eq!(none, None);
        assert_eq!(Instant::now(), start + Duration::from_secs(2));
        // Events before `from` don't count.
        assert!(log.since(start + Duration::from_millis(301)).is_empty());
    }
}

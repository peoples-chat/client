//! MeshCore through meshcore-rs (#60): a companion radio, reached over USB serial on desktops.
//!
//! meshcore-rs speaks the MeshCore companion protocol. This adapter turns its commands and
//! events into [`Radio`]. On phones the crate is built without a link of its own, and a link the
//! app runs itself, such as Bluetooth (#23), uses [`MeshCoreRadio::over`].
//!
//! Notes on meshcore-rs 0.2.0 that this code works around are in `docs/radio.md`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use meshcore_rs::commands::{CommandHandler, Destination};
use meshcore_rs::events::{Contact, EventDispatcher, EventPayload, EventType, MeshCoreEvent};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use super::{
    ContactKind, EVENT_BUFFER, Mesh, Radio, RadioContact, RadioError, RadioEvent, RadioInfo,
    RadioSettings, Sent, check_text, key_prefix,
};

/// The most text one MeshCore message holds, in bytes of UTF-8 (firmware v1.18.0).
pub const MAX_TEXT_BYTES: usize = 160;
/// MeshCore companion radios talk at this rate over USB serial.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
const BAUD_RATE: u32 = 115_200;
/// MeshCore's text type for plain messages. CLI replies from repeaters and signed room posts use
/// others.
const TXT_PLAIN: u8 = 0;

/// A MeshCore companion radio.
pub struct MeshCoreRadio {
    commands: Arc<tokio::sync::Mutex<CommandHandler>>,
    state: Arc<Shared>,
    /// The link, when this radio opened it. Its tasks read and write the serial port.
    link: Option<meshcore_rs::MeshCore>,
    /// Turns meshcore-rs's events into [`RadioEvent`]s.
    pump: JoinHandle<()>,
}

struct Shared {
    events: broadcast::Sender<RadioEvent>,
    connected: AtomicBool,
    /// Messages may wait on the radio, and nobody listened when it said so. Collecting them
    /// takes them off the radio, so it waits for a listener.
    uncollected: AtomicBool,
    /// The contacts the radio holds, by the prefix messages name them with.
    contacts: Mutex<HashMap<String, String>>,
}

impl Shared {
    fn contacts(&self) -> MutexGuard<'_, HashMap<String, String>> {
        self.contacts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn disconnected(&self) {
        if self.connected.swap(false, Ordering::SeqCst) {
            let _ = self.events.send(RadioEvent::Disconnected);
        }
    }
}

impl MeshCoreRadio {
    /// Connects to a companion radio on a USB serial port, such as `/dev/cu.usbmodem1101` on a
    /// Mac or `/dev/ttyACM0` on Linux. Desktop only: phones reach radios over Bluetooth (#23).
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub async fn connect_serial(port: &str) -> Result<Self, RadioError> {
        let link = meshcore_rs::MeshCore::serial(port, BAUD_RATE)
            .await
            .map_err(|error| RadioError::Link(error.to_string()))?;
        let commands = link.commands().clone();
        let dispatcher = link.dispatcher().clone();
        Self::start(commands, &dispatcher, Some(link)).await
    }

    /// A radio over meshcore-rs's command handler and event dispatcher, for a link the app runs
    /// itself: it writes the handler's frames to the radio and passes the radio's frames to the
    /// dispatcher's reader. Tests use it with a fake radio.
    pub async fn over(
        commands: Arc<tokio::sync::Mutex<CommandHandler>>,
        dispatcher: &Arc<EventDispatcher>,
    ) -> Result<Self, RadioError> {
        Self::start(commands, dispatcher, None).await
    }

    /// Starts following the radio's events, then greets it the way companion apps do. Messages
    /// that waited on the radio go to the first listener of [`Radio::events`].
    async fn start(
        commands: Arc<tokio::sync::Mutex<CommandHandler>>,
        dispatcher: &Arc<EventDispatcher>,
        link: Option<meshcore_rs::MeshCore>,
    ) -> Result<Self, RadioError> {
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let state = Arc::new(Shared {
            events,
            connected: AtomicBool::new(true),
            uncollected: AtomicBool::new(true),
            contacts: Mutex::new(HashMap::new()),
        });
        let pump = tokio::spawn(pump(dispatcher.receiver(), commands.clone(), state.clone()));
        let radio = Self {
            commands,
            state,
            link,
            pump,
        };
        if let Err(error) = radio.greet().await {
            // Lets go of the port, so that it can be opened again (#24).
            let _ = radio.disconnect().await;
            return Err(error);
        }
        Ok(radio)
    }

    /// Introduces the app to the radio, and reads its contacts.
    async fn greet(&self) -> Result<(), RadioError> {
        {
            let commands = self.commands.lock().await;
            // The radio expects an app to introduce itself before it pushes anything.
            commands.send_appstart().await.map_err(command_error)?;
            // Asking for the device information also makes the radio send messages with their
            // signal strength. Older firmware may not answer.
            if let Err(error) = commands.send_device_query().await {
                log::info!("the radio didn't describe itself: {error}");
            }
        }
        refresh_contacts(&self.commands, &self.state).await?;
        Ok(())
    }

    fn ensure_connected(&self) -> Result<(), RadioError> {
        if self.state.connected.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(RadioError::NotConnected)
        }
    }
}

impl Drop for MeshCoreRadio {
    fn drop(&mut self) {
        self.pump.abort();
        // A link this radio opened holds the serial port until it ends (#24).
        if let Some(link) = self.link.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(async move {
                let _ = link.disconnect().await;
            });
        }
    }
}

#[async_trait]
impl Radio for MeshCoreRadio {
    fn mesh(&self) -> Mesh {
        Mesh::MeshCore
    }

    async fn info(&self) -> Result<RadioInfo, RadioError> {
        self.ensure_connected()?;
        let commands = self.commands.lock().await;
        let me = commands.send_appstart().await.map_err(command_error)?;
        // Older firmware may not answer; the rest is still worth having.
        let device = commands.send_device_query().await.ok();
        Ok(RadioInfo {
            mesh: Mesh::MeshCore,
            public_key: hex::encode(me.public_key),
            name: me.name,
            settings: RadioSettings {
                // The radio reports the frequency in kHz and the bandwidth in Hz.
                frequency_mhz: f64::from(me.radio_freq) / 1000.0,
                bandwidth_khz: f64::from(me.radio_bw) / 1000.0,
                spreading_factor: me.sf,
                coding_rate: me.cr,
                tx_power_dbm: me.tx_power,
            },
            firmware: device.as_ref().and_then(|device| device.version.clone()),
            model: device.and_then(|device| device.model),
        })
    }

    async fn contacts(&self) -> Result<Vec<RadioContact>, RadioError> {
        self.ensure_connected()?;
        let contacts = refresh_contacts(&self.commands, &self.state).await?;
        Ok(contacts.iter().map(contact_view).collect())
    }

    async fn send_text(&self, to: &str, text: &str) -> Result<Sent, RadioError> {
        self.ensure_connected()?;
        // The radio answers both of these with an error that meshcore-rs reports only as a
        // timeout, so check first: the text must fit, and the radio must hold the contact.
        check_text(text)?;
        let key = hex::decode(to)
            .ok()
            .filter(|key| key.len() == 32)
            .ok_or_else(|| RadioError::UnknownContact(to.to_owned()))?;
        let known = |state: &Shared| state.contacts().values().any(|known| known == to);
        if !known(&self.state) {
            refresh_contacts(&self.commands, &self.state).await?;
            if !known(&self.state) {
                return Err(RadioError::UnknownContact(to.to_owned()));
            }
        }
        let sent = self
            .commands
            .lock()
            .await
            .send_msg(Destination::Bytes(key), text, None)
            .await
            .map_err(command_error)?;
        Ok(Sent {
            ack: hex::encode(sent.expected_ack),
            timeout: Duration::from_millis(u64::from(sent.suggested_timeout)),
        })
    }

    fn events(&self) -> broadcast::Receiver<RadioEvent> {
        let events = self.state.events.subscribe();
        // Someone listens now: collect what waited for a listener.
        if self.state.uncollected.swap(false, Ordering::SeqCst) {
            match tokio::runtime::Handle::try_current() {
                Ok(runtime) => {
                    runtime.spawn(collect_waiting(self.commands.clone(), self.state.clone()));
                }
                Err(_) => self.state.uncollected.store(true, Ordering::SeqCst),
            }
        }
        events
    }

    async fn disconnect(&self) -> Result<(), RadioError> {
        if let Some(link) = &self.link {
            link.disconnect()
                .await
                .map_err(|error| RadioError::Link(error.to_string()))?;
        }
        self.state.disconnected();
        Ok(())
    }
}

/// Passes meshcore-rs's events on as [`RadioEvent`]s, and collects messages when the radio says
/// some are waiting.
async fn pump(
    mut events: broadcast::Receiver<MeshCoreEvent>,
    commands: Arc<tokio::sync::Mutex<CommandHandler>>,
    state: Arc<Shared>,
) {
    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                log::warn!("missed {missed} radio events");
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => {
                state.disconnected();
                return;
            }
        };
        match (event.event_type, event.payload) {
            (EventType::ContactMsgRecv, EventPayload::ContactMessage(message))
                if message.txt_type == TXT_PLAIN =>
            {
                let from_prefix = hex::encode(message.sender_prefix);
                let from = state.contacts().get(&from_prefix).cloned();
                let _ = state.events.send(RadioEvent::TextReceived {
                    from_prefix,
                    from,
                    text: message.text,
                    sent_at: u64::from(message.sender_timestamp) * 1000,
                    snr: message.snr,
                });
            }
            (EventType::Ack, EventPayload::Ack { tag }) => {
                let _ = state.events.send(RadioEvent::Delivered {
                    ack: hex::encode(tag),
                });
            }
            (EventType::MessagesWaiting, _) => {
                if state.events.receiver_count() > 0 {
                    collect_waiting(commands.clone(), state.clone()).await;
                } else {
                    state.uncollected.store(true, Ordering::SeqCst);
                }
            }
            (EventType::NewContact, EventPayload::Contact(contact)) => {
                let public_key = contact.public_key_hex();
                state
                    .contacts()
                    .insert(key_prefix(&public_key), public_key.clone());
                let _ = state.events.send(RadioEvent::ContactHeard { public_key });
            }
            // The firmware sends the whole key here, but meshcore-rs keeps only its prefix. A
            // contact the radio has just added isn't known yet: read the contacts again.
            (EventType::Advertisement, EventPayload::Advertisement(advert)) => {
                let prefix = hex::encode(advert.prefix);
                let mut known = state.contacts().get(&prefix).cloned();
                if known.is_none() && refresh_contacts(&commands, &state).await.is_ok() {
                    known = state.contacts().get(&prefix).cloned();
                }
                if let Some(public_key) = known {
                    let _ = state.events.send(RadioEvent::ContactHeard { public_key });
                }
            }
            (EventType::Disconnected, _) => state.disconnected(),
            _ => {}
        }
    }
}

/// Reads the radio's contacts, and remembers them by the prefix messages name senders with.
async fn refresh_contacts(
    commands: &Arc<tokio::sync::Mutex<CommandHandler>>,
    state: &Shared,
) -> Result<Vec<Contact>, RadioError> {
    let contacts = commands
        .lock()
        .await
        .get_contacts(0)
        .await
        .map_err(command_error)?;
    let mut known = state.contacts();
    known.clear();
    for contact in &contacts {
        known.insert(contact.prefix_hex(), contact.public_key_hex());
    }
    Ok(contacts)
}

/// Collects the messages waiting on the radio. Each arrives as its own event, which [`pump`]
/// passes on. The radio keeps only 16, so this runs as soon as it says some are waiting and
/// someone listens.
async fn collect_waiting(commands: Arc<tokio::sync::Mutex<CommandHandler>>, state: Arc<Shared>) {
    loop {
        match commands.lock().await.get_msg().await {
            Ok(Some(_)) => {}
            Ok(None) => return,
            Err(error) => {
                log::warn!("could not collect a waiting radio message: {error}");
                // Try again when the radio next says messages wait.
                state.uncollected.store(true, Ordering::SeqCst);
                return;
            }
        }
    }
}

fn contact_view(contact: &Contact) -> RadioContact {
    RadioContact {
        public_key: contact.public_key_hex(),
        name: contact.adv_name.clone(),
        kind: match contact.contact_type {
            1 => ContactKind::Chat,
            2 => ContactKind::Repeater,
            3 => ContactKind::Room,
            4 => ContactKind::Sensor,
            other => ContactKind::Other(other),
        },
        last_heard: (contact.last_advert > 0).then(|| u64::from(contact.last_advert)),
        // MeshCore marks a contact without a known route with a path length of -1.
        has_path: contact.path_len >= 0,
    }
}

fn command_error(error: meshcore_rs::Error) -> RadioError {
    match error {
        meshcore_rs::Error::Timeout(_) => RadioError::Timeout,
        meshcore_rs::Error::Device(reason) | meshcore_rs::Error::Disabled(reason) => {
            RadioError::Refused(reason)
        }
        // The link's writer is gone.
        meshcore_rs::Error::NotConnected | meshcore_rs::Error::Channel(_) => {
            RadioError::NotConnected
        }
        other => RadioError::Link(other.to_string()),
    }
}

//! The Bluetooth run (#135): a radio reached through meshcore-rs's own Bluetooth link.
//!
//! macOS lets a program use Bluetooth only when the program *responsible* for it has permission.
//! For anything started from a terminal, an agent or cargo, that is the terminal or the agent,
//! and macOS ends the program or asks on the screen. So the side that holds the Bluetooth link
//! runs as its own launchd job ([`crate::launchd`]): the same binary, started with
//! `--ble-child`. The run ([`run`]) holds the lab lock and the USB radios, and tells the
//! Bluetooth side ([`serve`]) what to do over a socket on this computer, one JSON object per
//! line each way.
//!
//! The Bluetooth side never touches Bluetooth before macOS has allowed it, so that a run can't
//! put a prompt on the screen. Only `--allow-bluetooth`, which a person runs once, may ask.

use std::any::Any;
use std::io::Write;
use std::net::SocketAddr;
use std::time::Duration;

use async_trait::async_trait;
use peoples_chat_core::radio::{
    Mesh, MeshCoreRadio, Radio, RadioError, RadioEvent, RadioInfo, RadioSettings,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio::time::Instant;

use crate::config::RadioEntry;
use crate::lab::{Lab, Station, Timing, describe_info, mismatches};

/// Both sides must speak the same version: the Bluetooth side can be an older copy of the
/// program (`VERIFY_RADIO_BLE_PROGRAM`).
const PROTOCOL: u64 = 1;

/// What macOS says about this program using Bluetooth (`CBManager.authorization`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    /// Nobody has been asked yet: using Bluetooth now would put a prompt on the screen.
    NotAsked,
    Restricted,
    Denied,
    Allowed,
}

impl Permission {
    pub fn name(self) -> &'static str {
        match self {
            Permission::NotAsked => "not asked yet",
            Permission::Restricted => "restricted",
            Permission::Denied => "denied",
            Permission::Allowed => "allowed",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        [
            Permission::NotAsked,
            Permission::Restricted,
            Permission::Denied,
            Permission::Allowed,
        ]
        .into_iter()
        .find(|permission| permission.name() == name)
    }
}

/// Starts the side that holds the Bluetooth link.
#[async_trait]
pub trait Launcher: Send + Sync {
    /// Starts it. It connects to `control` and serves the radio named `name`. With `may_ask`
    /// it may put the macOS permission prompt on the screen. Dropping what this returns stops
    /// it.
    async fn launch(
        &self,
        control: SocketAddr,
        name: &str,
        may_ask: bool,
    ) -> Result<Box<dyn Send>, String>;

    /// Where to look when it doesn't start.
    fn hint(&self) -> String;
}

/// Bluetooth as the side that holds the link sees it.
#[async_trait]
pub trait Bluetooth: Send + Sync {
    fn permission(&self) -> Permission;

    /// Finds the radio that advertises `name` and connects to it, with its link watched.
    async fn open(&self, name: &str, within: Duration) -> Result<Opened, RadioError>;
}

/// A connected radio, and whatever else must live as long as its link.
pub struct Opened {
    pub radio: MeshCoreRadio,
    pub _link: Option<Box<dyn Any + Send + Sync>>,
}

/// One end of the socket between the run and the Bluetooth side.
struct Control {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

impl Control {
    fn new(stream: TcpStream) -> Self {
        let (reader, writer) = stream.into_split();
        Self {
            reader: BufReader::new(reader),
            writer,
        }
    }

    async fn send(&mut self, message: &Value) -> Result<(), String> {
        let line = format!("{message}\n");
        self.writer
            .write_all(line.as_bytes())
            .await
            .map_err(|error| error.to_string())
    }

    /// The next message, or `None` when the other side is gone.
    async fn next(&mut self) -> Option<Value> {
        let mut line = String::new();
        match self.reader.read_line(&mut line).await {
            Ok(read) if read > 0 => serde_json::from_str(&line).ok(),
            _ => None,
        }
    }
}

fn millis(since: Instant) -> u64 {
    since.elapsed().as_millis() as u64
}

fn seconds(millis: u64) -> String {
    format!("{:.1} s", millis as f64 / 1000.0)
}

fn info_json(info: &RadioInfo) -> Value {
    json!({
        "public_key": info.public_key,
        "name": info.name,
        "firmware": info.firmware,
        "model": info.model,
        "frequency_mhz": info.settings.frequency_mhz,
        "bandwidth_khz": info.settings.bandwidth_khz,
        "spreading_factor": info.settings.spreading_factor,
        "coding_rate": info.settings.coding_rate,
        "tx_power_dbm": info.settings.tx_power_dbm,
    })
}

fn info_from(value: &Value) -> Option<RadioInfo> {
    let text = |field: &str| value[field].as_str().map(str::to_owned);
    let small = |field: &str| value[field].as_u64().and_then(|n| u8::try_from(n).ok());
    Some(RadioInfo {
        mesh: Mesh::MeshCore,
        public_key: text("public_key")?,
        name: text("name")?,
        settings: RadioSettings {
            frequency_mhz: value["frequency_mhz"].as_f64()?,
            bandwidth_khz: value["bandwidth_khz"].as_f64()?,
            spreading_factor: small("spreading_factor")?,
            coding_rate: small("coding_rate")?,
            tx_power_dbm: small("tx_power_dbm")?,
        },
        firmware: text("firmware"),
        model: text("model"),
    })
}

// The side that holds the Bluetooth link.

const NOT_ALLOWED: &str = "macOS hasn't allowed this program to use Bluetooth";

/// Serves the run on `stream` until it says goodbye or goes away.
pub async fn serve(
    stream: TcpStream,
    name: &str,
    may_ask: bool,
    bluetooth: &dyn Bluetooth,
    timing: Timing,
) {
    let mut control = Control::new(stream);
    let permission = bluetooth.permission();
    let hello = json!({
        "protocol": PROTOCOL,
        "pid": std::process::id(),
        "bluetooth": permission.name(),
    });
    if control.send(&hello).await.is_err() {
        return;
    }
    let mut side = Side {
        name,
        bluetooth,
        timing,
        may_use: permission == Permission::Allowed
            || (may_ask && permission == Permission::NotAsked),
        opened: None,
        events: None,
    };
    while let Some(request) = control.next().await {
        let what = request["do"].as_str().unwrap_or_default().to_owned();
        if what == "bye" {
            break;
        }
        let reply = match side.handle(&what, &request).await {
            Ok(mut reply) => {
                reply["ok"] = Value::Bool(true);
                reply
            }
            Err(error) => json!({ "ok": false, "error": error }),
        };
        if control.send(&reply).await.is_err() {
            break;
        }
    }
    side.close().await;
}

struct Side<'a> {
    name: &'a str,
    bluetooth: &'a dyn Bluetooth,
    timing: Timing,
    /// Whether Bluetooth may be touched at all.
    may_use: bool,
    opened: Option<Opened>,
    /// The radio's events since it was opened, so that a message that arrives early waits.
    events: Option<broadcast::Receiver<RadioEvent>>,
}

impl Side<'_> {
    fn radio(&self) -> Result<&MeshCoreRadio, String> {
        self.opened
            .as_ref()
            .map(|opened| &opened.radio)
            .ok_or_else(|| "the radio isn't connected".to_owned())
    }

    async fn open(&mut self) -> Result<RadioInfo, String> {
        self.close().await;
        let opened = self
            .bluetooth
            .open(self.name, self.timing.ble_connect)
            .await
            .map_err(|error| error.to_string())?;
        let info = opened.radio.info().await.map_err(|e| e.to_string())?;
        self.events = Some(opened.radio.events());
        self.opened = Some(opened);
        Ok(info)
    }

    async fn close(&mut self) {
        self.events = None;
        if let Some(opened) = self.opened.take() {
            let _ = opened.radio.disconnect().await;
        }
    }

    /// Waits for an event that `wanted` accepts.
    async fn wait_for<T>(
        &mut self,
        within: Duration,
        what: &str,
        mut wanted: impl FnMut(&RadioEvent) -> Option<T>,
    ) -> Result<T, String> {
        let events = self
            .events
            .as_mut()
            .ok_or_else(|| "the radio isn't connected".to_owned())?;
        let deadline = Instant::now() + within;
        loop {
            match tokio::time::timeout_at(deadline, events.recv()).await {
                Err(_) => return Err(format!("{what} within {} s", within.as_secs())),
                Ok(Err(broadcast::error::RecvError::Closed)) => {
                    return Err(format!("{what}: the radio is gone"));
                }
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => {}
                Ok(Ok(event)) => {
                    if let Some(found) = wanted(&event) {
                        return Ok(found);
                    }
                    if event == RadioEvent::Disconnected {
                        return Err(format!("{what}: the Bluetooth link was lost"));
                    }
                }
            }
        }
    }

    async fn handle(&mut self, what: &str, request: &Value) -> Result<Value, String> {
        let text = |field: &str| {
            request[field]
                .as_str()
                .ok_or_else(|| format!("the request has no {field}"))
        };
        let failed = |error: RadioError| error.to_string();
        match what {
            "connect" => {
                if !self.may_use {
                    return Err(NOT_ALLOWED.to_owned());
                }
                let started = Instant::now();
                let info = self.open().await?;
                Ok(json!({ "info": info_json(&info), "took_ms": millis(started) }))
            }
            "contacts" => {
                let contacts = self.radio()?.contacts().await.map_err(failed)?;
                let keys: Vec<String> = contacts.into_iter().map(|c| c.public_key).collect();
                Ok(json!({ "keys": keys }))
            }
            "advertise" => {
                self.radio()?.advertise().await.map_err(failed)?;
                Ok(json!({}))
            }
            "send" => {
                let sent = self
                    .radio()?
                    .send_text(text("to")?, text("text")?)
                    .await
                    .map_err(failed)?;
                Ok(json!({ "ack": sent.ack }))
            }
            "expect_text" => {
                let expected = text("text")?.to_owned();
                let from = self
                    .wait_for(
                        self.timing.message,
                        &format!("no message {expected:?} arrived"),
                        |event| match event {
                            RadioEvent::TextReceived { text, from, .. } if *text == expected => {
                                Some(from.clone())
                            }
                            _ => None,
                        },
                    )
                    .await?;
                Ok(json!({ "from": from }))
            }
            // The radio restarts, which drops the link from the radio's end. The link's own
            // checks must notice; then the same process finds the radio and connects again.
            "drop" => {
                let started = Instant::now();
                self.radio()?.restart().await.map_err(failed)?;
                self.wait_for(
                    self.timing.ble_lost,
                    "the lost link wasn't noticed",
                    |event| (*event == RadioEvent::Disconnected).then_some(()),
                )
                .await?;
                let lost_after = millis(started);
                let info = self.open().await?;
                Ok(json!({
                    "lost_after_ms": lost_after,
                    "back_after_ms": millis(started),
                    "info": info_json(&info),
                    "pid": std::process::id(),
                }))
            }
            other => Err(format!("unknown request {other:?}")),
        }
    }
}

// The run.

/// The run's end of the socket.
struct Child {
    label: String,
    control: Control,
    pid: u64,
}

impl Child {
    async fn ask(&mut self, request: Value, within: Duration) -> Result<Value, String> {
        let label = &self.label;
        let what = request["do"].as_str().unwrap_or_default().to_owned();
        self.control
            .send(&request)
            .await
            .map_err(|error| format!("{label}: the Bluetooth side is gone: {error}"))?;
        let reply = tokio::time::timeout(within, self.control.next())
            .await
            .map_err(|_| {
                format!(
                    "{label}: the Bluetooth side didn't answer {what:?} within {} s",
                    within.as_secs()
                )
            })?
            .ok_or_else(|| format!("{label}: the Bluetooth side went away during {what:?}"))?;
        if reply["ok"].as_bool() == Some(true) {
            Ok(reply)
        } else {
            Err(format!(
                "{label}: {}",
                reply["error"]
                    .as_str()
                    .unwrap_or("the Bluetooth side failed")
            ))
        }
    }

    async fn contacts(&mut self, within: Duration) -> Result<Vec<String>, String> {
        let reply = self.ask(json!({ "do": "contacts" }), within).await?;
        Ok(reply["keys"]
            .as_array()
            .map(|keys| {
                keys.iter()
                    .filter_map(|key| key.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default())
    }
}

/// The Bluetooth run for one radio. Without `usb`, a dry run: it connects and checks the radio,
/// and sends nothing. With `usb`, it then passes a message each way between the two radios,
/// drops the Bluetooth link and has it come back.
pub async fn run(
    lab: &Lab<'_>,
    launcher: &dyn Launcher,
    entry: &RadioEntry,
    usb: Option<&Station>,
    may_ask: bool,
    out: &mut (dyn Write + Send),
) -> Result<(), String> {
    let label = &entry.label;
    let timing = lab.timing;
    let name = entry.ble_name.as_deref().unwrap_or_default();
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|error| format!("{label}: could not open the control socket: {error}"))?;
    let control = listener
        .local_addr()
        .map_err(|error| format!("{label}: {error}"))?;
    // Stops the Bluetooth side when this returns, however it does.
    let _job = launcher
        .launch(control, name, may_ask)
        .await
        .map_err(|error| format!("{label}: could not start the Bluetooth side: {error}"))?;
    let (stream, _) = tokio::time::timeout(timing.ble_start, listener.accept())
        .await
        .map_err(|_| {
            format!(
                "{label}: the Bluetooth side didn't start within {} s. {}",
                timing.ble_start.as_secs(),
                launcher.hint()
            )
        })?
        .map_err(|error| format!("{label}: {error}"))?;
    let mut control = Control::new(stream);
    let hello = tokio::time::timeout(timing.ble_start, control.next())
        .await
        .ok()
        .flatten()
        .ok_or_else(|| {
            format!(
                "{label}: the Bluetooth side said nothing. {}",
                launcher.hint()
            )
        })?;
    if hello["protocol"].as_u64() != Some(PROTOCOL) {
        return Err(format!(
            "{label}: the Bluetooth side is another version of the harness. Build it again \
             (see VERIFY_RADIO_BLE_PROGRAM in docs/radio-lab.md)."
        ));
    }
    let permission = hello["bluetooth"].as_str().and_then(Permission::parse);
    let allowed = permission == Some(Permission::Allowed);
    if !allowed && !(may_ask && permission == Some(Permission::NotAsked)) {
        return Err(format!(
            "{label}: macOS hasn't allowed the harness to use Bluetooth (it says: {}), and a run \
             never asks, so nothing was sent to the radio. A person allows it once, at the Mac: \
             run `pnpm verify:radio --allow-bluetooth` and click Allow in the dialog. macOS asks \
             again after the harness is rebuilt (docs/radio-lab.md).",
            permission.map_or("nothing", Permission::name)
        ));
    }
    if !allowed {
        let _ = writeln!(
            out,
            "{label}: asking macOS for Bluetooth. Click Allow in the dialog on the Mac's screen \
             within {} s.",
            timing.ble_connect.as_secs()
        );
    }
    let mut child = Child {
        label: label.clone(),
        control,
        pid: hello["pid"].as_u64().unwrap_or_default(),
    };

    // Found by its advertised name, connected, and checked against the lab before anything is
    // sent to it or through it.
    let connected = child
        .ask(json!({ "do": "connect" }), timing.ble_connect * 2)
        .await?;
    let info = info_from(&connected["info"])
        .ok_or_else(|| format!("{label}: the Bluetooth side didn't describe the radio"))?;
    let _ = writeln!(
        out,
        "{label}: {name} over Bluetooth, {}",
        describe_info(&info)
    );
    let wrong = mismatches(&lab.config.lab, entry, &info);
    if !wrong.is_empty() {
        return Err(format!("{label}: {}", wrong.join("; ")));
    }
    let _ = writeln!(
        out,
        "ok {label}: found by its advertised name and connected through meshcore-rs's Bluetooth \
         link in {}, macOS permission: {}, no prompt",
        seconds(connected["took_ms"].as_u64().unwrap_or_default()),
        if allowed { "allowed" } else { "just asked" },
    );
    let Some(usb) = usb else {
        let _ = child.control.send(&json!({ "do": "bye" })).await;
        return Ok(());
    };

    let usb_label = &usb.entry.label;
    let usb_key = usb.info.public_key.clone();
    let ble_key = info.public_key.clone();
    make_contacts(lab, &mut child, usb, &ble_key, out).await?;

    // A message each way.
    let run_id = std::process::id();
    let mut usb_events = usb.radio.events();
    let to_ble = format!("verify:radio {run_id} {usb_label} to {label}");
    pass_to_ble(&mut child, usb, &ble_key, &to_ble, timing).await?;
    let _ = writeln!(
        out,
        "ok {usb_label} (USB) to {label} (Bluetooth): {to_ble:?} arrived"
    );
    let to_usb = format!("verify:radio {run_id} {label} to {usb_label}");
    let started = Instant::now();
    child
        .ask(
            json!({ "do": "send", "to": usb_key, "text": to_usb }),
            timing.message,
        )
        .await?;
    wait_for_text(&mut usb_events, &to_usb, timing.message)
        .await
        .map_err(|error| format!("{usb_label}: {error}"))?;
    let _ = writeln!(
        out,
        "ok {label} (Bluetooth) to {usb_label} (USB): {to_usb:?} arrived in {}",
        seconds(millis(started))
    );

    // The link drops, is noticed, and comes back in the same process.
    if !entry.reboot_to_drop_link {
        return Err(format!(
            "{label}: the dropped link wasn't tested. The harness can only drop a Bluetooth \
             link by restarting the radio (command 19), and does that only when radios.toml \
             says `reboot_to_drop_link = true` for {label}. Restarting changes no firmware, \
             frequency or preset."
        ));
    }
    let dropped = child
        .ask(
            json!({ "do": "drop" }),
            timing.ble_lost + timing.ble_connect * 2,
        )
        .await?;
    let again = info_from(&dropped["info"]).filter(|again| again.public_key == ble_key);
    if again.is_none() {
        return Err(format!("{label}: another radio answered after the drop"));
    }
    if dropped["pid"].as_u64() != Some(child.pid) {
        return Err(format!("{label}: the Bluetooth side was restarted"));
    }
    let after = format!("verify:radio {run_id} {usb_label} to {label} again");
    pass_to_ble(&mut child, usb, &ble_key, &after, timing).await?;
    let _ = writeln!(
        out,
        "ok {label}: its Bluetooth link was dropped (the radio restarted), noticed after {}, \
         and connected again after {} by the same process (pid {}); {after:?} arrived over the \
         new link",
        seconds(dropped["lost_after_ms"].as_u64().unwrap_or_default()),
        seconds(dropped["back_after_ms"].as_u64().unwrap_or_default()),
        child.pid,
    );
    let _ = child.control.send(&json!({ "do": "bye" })).await;
    Ok(())
}

/// Sends `text` from the USB radio and waits until the Bluetooth side has it.
async fn pass_to_ble(
    child: &mut Child,
    usb: &Station,
    ble_key: &str,
    text: &str,
    timing: Timing,
) -> Result<(), String> {
    usb.radio
        .send_text(ble_key, text)
        .await
        .map_err(|error| format!("{}: could not send: {error}", usb.entry.label))?;
    child
        .ask(
            json!({ "do": "expect_text", "text": text }),
            timing.message + timing.reply,
        )
        .await?;
    Ok(())
}

async fn wait_for_text(
    events: &mut broadcast::Receiver<RadioEvent>,
    expected: &str,
    within: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + within;
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Err(_) => {
                return Err(format!(
                    "no message {expected:?} arrived within {} s",
                    within.as_secs()
                ));
            }
            Ok(Ok(RadioEvent::TextReceived { text, .. })) if text == expected => return Ok(()),
            Ok(Ok(RadioEvent::Disconnected) | Err(broadcast::error::RecvError::Closed)) => {
                return Err("its link was lost".to_owned());
            }
            Ok(_) => {}
        }
    }
}

/// Makes sure each radio holds the other as a contact: without that a radio refuses to send.
/// Radios add each other when they hear each other's announcement, and the harness has them
/// announce themselves only when the lab file allows it.
async fn make_contacts(
    lab: &Lab<'_>,
    child: &mut Child,
    usb: &Station,
    ble_key: &str,
    out: &mut (dyn Write + Send),
) -> Result<(), String> {
    let label = child.label.clone();
    let usb_label = &usb.entry.label;
    let timing = lab.timing;
    let usb_key = &usb.info.public_key;
    let mut announced = false;
    let deadline = Instant::now() + timing.contacts;
    loop {
        let usb_holds = usb
            .radio
            .contacts()
            .await
            .map_err(|error| format!("{usb_label}: {error}"))?
            .iter()
            .any(|contact| contact.public_key.eq_ignore_ascii_case(ble_key));
        let ble_holds = child
            .contacts(timing.ble_connect)
            .await?
            .iter()
            .any(|key| key.eq_ignore_ascii_case(usb_key));
        if usb_holds && ble_holds {
            return Ok(());
        }
        let missing = match (usb_holds, ble_holds) {
            (false, false) => format!("{usb_label} and {label} don't hold each other"),
            (false, true) => format!("{usb_label} doesn't hold {label}"),
            _ => format!("{label} doesn't hold {usb_label}"),
        };
        if !lab.config.lab.exchange_adverts {
            return Err(format!(
                "{missing} as a contact, so no message can pass. Add them to each other, or \
                 set `exchange_adverts = true` under [lab] in radios.toml: the harness then has \
                 both announce themselves once (command 7), and radios that hear an \
                 announcement add its sender."
            ));
        }
        if announced && Instant::now() >= deadline {
            return Err(format!(
                "{missing} as a contact {} s after both announced themselves. Does the radio \
                 add contacts by itself?",
                timing.contacts.as_secs()
            ));
        }
        if !announced {
            let _ = writeln!(
                out,
                "{missing} as a contact: both announce themselves (radios.toml allows it)"
            );
            usb.radio
                .advertise()
                .await
                .map_err(|error| format!("{usb_label}: could not announce itself: {error}"))?;
            child
                .ask(json!({ "do": "advertise" }), timing.reply * 2)
                .await?;
            announced = true;
        }
        tokio::time::sleep(timing.poll).await;
    }
}

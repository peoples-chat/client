//! The Bluetooth run (#135): a radio reached through the bridge (`tools/ble-bridge/`).
//!
//! The harness never touches Bluetooth. The bridge, a program of its own that a person installed
//! and allowed once, holds the Bluetooth link and passes the radio's companion frames to TCP on
//! this computer. The core connects there ([`MeshCoreRadio::connect_tcp`]), and a second port of
//! the bridge takes commands about the link. Without a bridge, the radio is skipped.

use std::io::Write;
use std::time::Duration;

use peoples_chat_core::radio::{MeshCoreRadio, Radio, RadioEvent, RadioInfo};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio::time::Instant;

use crate::config::RadioEntry;
use crate::lab::{Lab, Station, describe_info, mismatches};

const HOST: &str = "127.0.0.1";

/// The bridge's control port: one command per line, one line back.
struct Control {
    stream: BufReader<TcpStream>,
    within: Duration,
}

impl Control {
    /// `None` when nothing listens there: the bridge isn't running.
    async fn open(port: u16, within: Duration) -> Option<Self> {
        let stream = tokio::time::timeout(within, TcpStream::connect((HOST, port)))
            .await
            .ok()?
            .ok()?;
        Some(Self {
            stream: BufReader::new(stream),
            within,
        })
    }

    async fn ask(&mut self, command: &str) -> Result<String, String> {
        let failed = |why: String| format!("the bridge didn't answer {command:?}: {why}");
        let reply = tokio::time::timeout(self.within, async {
            self.stream
                .write_all(format!("{command}\n").as_bytes())
                .await?;
            let mut reply = String::new();
            self.stream.read_line(&mut reply).await?;
            Ok::<_, std::io::Error>(reply)
        })
        .await
        .map_err(|_| failed("no answer in time".to_owned()))?
        .map_err(|error| failed(error.to_string()))?;
        match reply.trim() {
            "" => Err(failed("it closed the connection".to_owned())),
            reply => Ok(reply.to_owned()),
        }
    }
}

/// A radio behind the bridge, connected and checked against the lab.
struct Linked {
    radio: MeshCoreRadio,
    info: RadioInfo,
}

/// Waits for the bridge to have its radio, then connects the core to it and checks it against
/// the lab file. Nothing is sent over the air, and a radio that differs is let go of.
async fn connect(
    lab: &Lab<'_>,
    entry: &RadioEntry,
    control: &mut Control,
) -> Result<Linked, String> {
    let label = &entry.label;
    let name = entry.ble_name.as_deref().unwrap_or_default();
    let deadline = Instant::now() + lab.timing.ble_connect;
    let radio = loop {
        let problem = match control.ask("status").await {
            Ok(status) if status == "connected" => {
                match MeshCoreRadio::connect_tcp(HOST, entry.bridge_port()).await {
                    Ok(radio) => break radio,
                    Err(error) => error.to_string(),
                }
            }
            Ok(_) => format!("the bridge has no Bluetooth link to {name}"),
            Err(error) => error,
        };
        if Instant::now() >= deadline {
            return Err(format!(
                "{label}: {problem} after {} s. Is the radio on, in range and not connected to \
                 a phone? The bridge's log says more.",
                lab.timing.ble_connect.as_secs()
            ));
        }
        tokio::time::sleep(lab.timing.poll).await;
    };
    radio.watch_link(lab.timing.watch);
    let checked = radio
        .info()
        .await
        .map_err(|e| e.to_string())
        .and_then(|info| {
            let wrong = mismatches(&lab.config.lab, entry, &info);
            if wrong.is_empty() {
                Ok(info)
            } else {
                Err(wrong.join("; "))
            }
        });
    match checked {
        Ok(info) => Ok(Linked { radio, info }),
        Err(problem) => {
            let _ = radio.disconnect().await;
            Err(format!("{label}: {problem}"))
        }
    }
}

/// The Bluetooth run for one radio. Without `usb`, a dry run: it connects and checks the radio,
/// and sends nothing. With `usb`, it then passes a message each way between the two radios, has
/// the bridge drop the Bluetooth link, and has the core connect again.
pub async fn run(
    lab: &Lab<'_>,
    entry: &RadioEntry,
    usb: Option<&Station>,
    out: &mut (dyn Write + Send),
) -> Result<(), String> {
    let label = &entry.label;
    let timing = lab.timing;
    let name = entry.ble_name.as_deref().unwrap_or_default();
    let control_port = entry.bridge_control_port();
    let Some(mut control) = Control::open(control_port, timing.reply).await else {
        let _ = writeln!(
            out,
            "{label}: SKIPPED, not checked: the Bluetooth bridge isn't running (nothing listens \
             on {HOST}:{control_port}). A person installs and starts it with \
             tools/ble-bridge/install.sh (docs/radio-lab.md)."
        );
        return Ok(());
    };
    let started = Instant::now();
    let linked = connect(lab, entry, &mut control).await?;
    let _ = writeln!(
        out,
        "{label}: {name} over Bluetooth, {}",
        describe_info(&linked.info)
    );
    let _ = writeln!(
        out,
        "ok {label}: connected through the bridge on {HOST}:{} in {}, on the lab preset",
        entry.bridge_port(),
        seconds(started)
    );
    let Some(usb) = usb else {
        let _ = linked.radio.disconnect().await;
        return Ok(());
    };
    let result = pass_messages(lab, entry, &mut control, linked, usb, out).await;
    result.map_err(|error| format!("{label}: {error}"))
}

/// Messages both ways, then the dropped link. `linked` is checked, as `usb` is.
async fn pass_messages(
    lab: &Lab<'_>,
    entry: &RadioEntry,
    control: &mut Control,
    linked: Linked,
    usb: &Station,
    out: &mut (dyn Write + Send),
) -> Result<(), String> {
    let label = &entry.label;
    let usb_label = &usb.entry.label;
    let timing = lab.timing;
    let Linked { radio, info } = linked;
    make_contacts(lab, &radio, label, usb, &info.public_key, out).await?;

    // A message each way. Listening starts before anything is sent.
    let run_id = std::process::id();
    let mut ble_events = radio.events();
    let mut usb_events = usb.radio.events();
    let to_ble = format!("verify:radio {run_id} {usb_label} to {label}");
    let started = Instant::now();
    pass(
        &usb.radio,
        &info.public_key,
        &to_ble,
        &mut ble_events,
        timing.message,
    )
    .await?;
    let _ = writeln!(
        out,
        "ok {usb_label} (USB) to {label} (Bluetooth): {to_ble:?} arrived in {}",
        seconds(started)
    );
    let to_usb = format!("verify:radio {run_id} {label} to {usb_label}");
    let started = Instant::now();
    pass(
        &radio,
        &usb.info.public_key,
        &to_usb,
        &mut usb_events,
        timing.message,
    )
    .await?;
    let _ = writeln!(
        out,
        "ok {label} (Bluetooth) to {usb_label} (USB): {to_usb:?} arrived in {}",
        seconds(started)
    );

    // The bridge drops the link. The core must notice, and the same process connects again.
    let mut events = radio.events();
    let started = Instant::now();
    let reply = control.ask("drop").await?;
    if reply != "dropped" {
        return Err(format!("the bridge didn't drop the link: {reply}"));
    }
    wait_for(&mut events, timing.lost, |event| {
        *event == RadioEvent::Disconnected
    })
    .await
    .map_err(|_| {
        format!(
            "the core didn't notice the dropped link within {} s",
            timing.lost.as_secs()
        )
    })?;
    let noticed = seconds(started);
    let _ = radio.disconnect().await;
    drop(radio);

    let again = connect(lab, entry, control)
        .await
        .map_err(|error| format!("the link didn't come back after it was dropped. {error}"))?;
    if again.info.public_key != info.public_key {
        return Err("another radio answered after the drop".to_owned());
    }
    let back = seconds(started);
    let mut ble_events = again.radio.events();
    let after = format!("verify:radio {run_id} {usb_label} to {label} again");
    pass(
        &usb.radio,
        &info.public_key,
        &after,
        &mut ble_events,
        timing.message,
    )
    .await?;
    let _ = writeln!(
        out,
        "ok {label}: the bridge dropped the Bluetooth link, the core noticed after {noticed} and \
         was connected again after {back} in the same process (pid {run_id}); {after:?} arrived \
         over the new link"
    );
    let _ = again.radio.disconnect().await;
    Ok(())
}

fn seconds(since: Instant) -> String {
    format!("{:.1} s", since.elapsed().as_secs_f64())
}

/// Sends `text` from one radio, and waits until the other, whose events these are, has it.
async fn pass(
    from: &MeshCoreRadio,
    to: &str,
    text: &str,
    arriving: &mut broadcast::Receiver<RadioEvent>,
    within: Duration,
) -> Result<(), String> {
    from.send_text(to, text)
        .await
        .map_err(|error| format!("could not send {text:?}: {error}"))?;
    wait_for(
        arriving,
        within,
        |event| matches!(event, RadioEvent::TextReceived { text: got, .. } if got == text),
    )
    .await
    .map_err(|why| format!("{text:?} didn't arrive: {why}"))
}

/// Waits for an event that `wanted` accepts.
async fn wait_for(
    events: &mut broadcast::Receiver<RadioEvent>,
    within: Duration,
    wanted: impl Fn(&RadioEvent) -> bool,
) -> Result<(), String> {
    let deadline = Instant::now() + within;
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Err(_) => return Err(format!("nothing within {} s", within.as_secs())),
            Ok(Ok(event)) if wanted(&event) => return Ok(()),
            Ok(Ok(RadioEvent::Disconnected) | Err(broadcast::error::RecvError::Closed)) => {
                return Err("the radio's link was lost".to_owned());
            }
            Ok(_) => {}
        }
    }
}

/// Makes sure each radio holds the other as a contact: without that a radio refuses to send. A
/// radio adds another when it hears it announce itself, so both announce themselves. That
/// changes no firmware, frequency, preset or power.
async fn make_contacts(
    lab: &Lab<'_>,
    ble: &MeshCoreRadio,
    label: &str,
    usb: &Station,
    ble_key: &str,
    out: &mut (dyn Write + Send),
) -> Result<(), String> {
    let usb_label = &usb.entry.label;
    let timing = lab.timing;
    let mut announced = false;
    let deadline = Instant::now() + timing.contacts;
    loop {
        let holds = |contacts: Vec<peoples_chat_core::radio::RadioContact>, key: &str| {
            contacts
                .iter()
                .any(|contact| contact.public_key.eq_ignore_ascii_case(key))
        };
        let usb_holds = holds(
            usb.radio
                .contacts()
                .await
                .map_err(|error| format!("{usb_label}'s contacts: {error}"))?,
            ble_key,
        );
        let ble_holds = holds(
            ble.contacts()
                .await
                .map_err(|error| format!("{label}'s contacts: {error}"))?,
            &usb.info.public_key,
        );
        if usb_holds && ble_holds {
            return Ok(());
        }
        let missing = match (usb_holds, ble_holds) {
            (false, false) => format!("{usb_label} and {label} don't hold each other"),
            (false, true) => format!("{usb_label} doesn't hold {label}"),
            _ => format!("{label} doesn't hold {usb_label}"),
        };
        if !announced {
            let _ = writeln!(out, "{missing} as a contact: both announce themselves");
            usb.radio
                .advertise()
                .await
                .map_err(|error| format!("{usb_label} could not announce itself: {error}"))?;
            ble.advertise()
                .await
                .map_err(|error| format!("{label} could not announce itself: {error}"))?;
            announced = true;
        } else if Instant::now() >= deadline {
            return Err(format!(
                "{missing} as a contact {} s after both announced themselves. Does the radio \
                 add contacts by itself?",
                timing.contacts.as_secs()
            ));
        }
        tokio::time::sleep(timing.poll).await;
    }
}

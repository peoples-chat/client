//! Pairing a radio over a cable (#24). The user picks the port their radio is plugged into, and
//! the core takes the radio's key as their address on the mesh: it shares it with every contact
//! (#34).
//!
//! The core remembers the radio, and opens it again by itself: at start, and after the radio
//! restarts or is plugged in again, on the same port or on another one with the same USB serial
//! number. It only reads the radio's identity and settings, and never changes any of them.

use std::sync::{Arc, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use tokio::task::JoinHandle;

use super::{Core, Inner, failed, upgrade};
use crate::interface::{CommandError, RadioPort, RadioStatus};
use crate::radio::{Port, Radio, RadioAddress, RadioError, RadioInfo, RadioPorts};
use crate::radio_store::Pairing;

/// How long the core waits before it looks for the paired radio again after its link dropped.
/// The wait doubles after each look, up to [`LONGEST_RECONNECT_WAIT`].
const FIRST_RECONNECT_WAIT: Duration = Duration::from_secs(1);
/// The longest wait between looks. A look only lists the ports until the radio's own is there,
/// so it costs little.
const LONGEST_RECONNECT_WAIT: Duration = Duration::from_secs(10);

impl Core {
    /// Pairs radios on `ports` from now on, and opens the paired radio there (#24). The shell
    /// gives the core the computer's USB serial ports; tests give it mock ones. Emits
    /// [`crate::interface::CoreEvent::RadioChanged`].
    pub fn use_radio_ports(&self, ports: Arc<dyn RadioPorts>) {
        *self.radio_ports_slot() = Some(ports);
        if self.inner.radio_store.pairing().is_some() && !self.paired_radio_connected() {
            self.start_reconnect(Duration::ZERO);
        }
        self.radio_changed();
    }

    /// The ports a radio may be plugged into now (#24).
    pub async fn radio_ports(&self) -> Result<Vec<RadioPort>, CommandError> {
        let ports = self.ports()?;
        let listed = ports
            .list()
            .await
            .map_err(|error| failed("list the USB ports", error))?;
        Ok(listed
            .into_iter()
            .map(|port| RadioPort {
                path: port.path,
                description: port.description,
            })
            .collect())
    }

    /// Pairs the radio on the port at `path` (#24): opens it, reads its key, name and settings,
    /// remembers it, and uses it in place of any radio before. Emits
    /// [`crate::interface::CoreEvent::RadioChanged`].
    pub async fn pair_radio(&self, path: &str) -> Result<RadioStatus, CommandError> {
        let ports = self.ports()?;
        self.stop_reconnect();
        let _one_at_a_time = self.inner.radio.pairing.lock().await;
        let same_port = self
            .inner
            .radio_store
            .pairing()
            .is_some_and(|pairing| pairing.port == path);
        if same_port && self.paired_radio_connected() {
            return Ok(self.radio_status());
        }
        let listed = ports
            .list()
            .await
            .map_err(|error| failed("list the USB ports", error))?;
        let Some(port) = listed.into_iter().find(|port| port.path == path) else {
            self.reconnect_if_paired();
            return Err(CommandError::new(
                "That port is gone. Plug the radio in and look again.",
            ));
        };
        // The paired radio's link may still hold the port.
        if same_port {
            self.let_go_of_radio().await;
        }
        let (radio, info) = match open_radio(ports.as_ref(), path).await {
            Ok(opened) => opened,
            Err(error) => {
                self.reconnect_if_paired();
                self.radio_changed();
                return Err(pairing_error(path, &error));
            }
        };
        // The radio used before, on another port, goes.
        self.let_go_of_radio().await;
        if let Err(error) = self.remember(&port, &info) {
            let _ = radio.disconnect().await;
            return Err(error);
        }
        if let Err(error) = self.attach_radio(radio.clone()).await {
            // It went away between reading it and using it.
            let _ = radio.disconnect().await;
            self.reconnect_if_paired();
            self.radio_changed();
            return Err(error);
        }
        log::info!("a radio was paired");
        Ok(self.radio_status())
    }

    /// Stops using the paired radio, closes its link, and forgets it (#24). Nothing on the radio
    /// changes. Emits [`crate::interface::CoreEvent::RadioChanged`].
    pub async fn forget_radio(&self) -> Result<RadioStatus, CommandError> {
        self.stop_reconnect();
        let _one_at_a_time = self.inner.radio.pairing.lock().await;
        self.let_go_of_radio().await;
        self.inner
            .radio_store
            .set_pairing(None)
            .map_err(|error| failed("forget the radio", error))?;
        log::info!("the paired radio was forgotten");
        Ok(self.radio_changed())
    }

    /// Waits `wait` between looks for the paired radio, in place of the growing wait: for tests
    /// (#24).
    pub fn set_radio_reconnect_wait(&self, wait: Duration) {
        *self
            .inner
            .radio
            .reconnect_wait
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(wait);
    }

    /// The paired radio's link dropped: looks for it, soon. The caller emits the status.
    pub(super) fn paired_radio_lost(&self) {
        self.start_reconnect(self.next_reconnect_wait(Duration::ZERO));
    }

    /// Stops looking for the paired radio.
    pub(super) fn stop_reconnect(&self) {
        if let Some(task) = self.reconnect_task().take() {
            task.abort();
        }
    }

    /// Whether the core is looking for the paired radio.
    pub(super) fn reconnecting(&self) -> bool {
        self.reconnect_task()
            .as_ref()
            .is_some_and(|task| !task.is_finished())
    }

    pub(super) fn radio_ports_slot(&self) -> MutexGuard<'_, Option<Arc<dyn RadioPorts>>> {
        self.inner
            .radio
            .ports
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts looking for the paired radio, after `first`, unless the core looks already.
    fn start_reconnect(&self, first: Duration) {
        if self.radio_ports_slot().is_none() {
            return;
        }
        let mut task = self.reconnect_task();
        if task.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        *task = Some(tokio::spawn(reconnect(Arc::downgrade(&self.inner), first)));
    }

    /// Looks for the paired radio again, if there is one that isn't in use: after a pairing
    /// that didn't work.
    fn reconnect_if_paired(&self) {
        if self.inner.radio_store.pairing().is_some() && !self.paired_radio_connected() {
            self.paired_radio_lost();
        }
    }

    /// Opens the paired radio where it is plugged in now, and uses it. Done once it is in use, or
    /// when no radio is paired any more.
    async fn open_paired(&self) -> Result<(), String> {
        let _one_at_a_time = self.inner.radio.pairing.lock().await;
        let Some(pairing) = self.inner.radio_store.pairing() else {
            return Ok(());
        };
        if self.paired_radio_connected() {
            return Ok(());
        }
        let ports = self.ports().map_err(|error| error.message)?;
        let listed = ports.list().await.map_err(|error| error.to_string())?;
        let port = find_paired(&listed, &pairing)
            .cloned()
            .ok_or_else(|| "it isn't plugged in".to_owned())?;
        // The link that dropped may still hold the port.
        self.let_go_of_radio().await;
        let (radio, info) = open_radio(ports.as_ref(), &port.path)
            .await
            .map_err(|error| format!("{}: {error}", port.path))?;
        if !info.public_key.eq_ignore_ascii_case(&pairing.radio.key) {
            let _ = radio.disconnect().await;
            return Err(format!("another radio is on {}", port.path));
        }
        self.remember(&port, &info).map_err(|error| error.message)?;
        self.attach_radio(radio)
            .await
            .map_err(|error| error.message)?;
        log::info!("the paired radio is back");
        Ok(())
    }

    /// Remembers the radio `info` describes, on `port`, as the paired radio.
    fn remember(&self, port: &Port, info: &RadioInfo) -> Result<(), CommandError> {
        self.inner
            .radio_store
            .set_pairing(Some(Pairing {
                port: port.path.clone(),
                serial_number: port.serial_number.clone(),
                name: info.name.clone(),
                radio: RadioAddress::new(info, 0),
            }))
            .map_err(|error| failed("remember the radio", error))
    }

    /// Stops using the radio in use, and closes its link so that its port is free.
    async fn let_go_of_radio(&self) {
        let previous = self.attached().take();
        let Some(previous) = previous else {
            return;
        };
        let radio = Arc::clone(&previous.radio);
        drop(previous);
        if let Err(error) = radio.disconnect().await {
            log::info!("could not close the radio's link: {error}");
        }
        self.radio_changed();
        self.reach_changed();
    }

    /// Whether the radio in use is the paired one, with its link open.
    fn paired_radio_connected(&self) -> bool {
        let Some(paired) = self.inner.radio_store.pairing() else {
            return false;
        };
        self.attached().as_ref().is_some_and(|attached| {
            attached.connected
                && attached
                    .info
                    .public_key
                    .eq_ignore_ascii_case(&paired.radio.key)
        })
    }

    /// How long to wait after a look that waited `previous`: a test's wait, or the growing one.
    fn next_reconnect_wait(&self, previous: Duration) -> Duration {
        let set = *self
            .inner
            .radio
            .reconnect_wait
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        set.unwrap_or_else(|| next_wait(previous))
    }

    fn ports(&self) -> Result<Arc<dyn RadioPorts>, CommandError> {
        self.radio_ports_slot()
            .clone()
            .ok_or_else(|| CommandError::new("This device can't pair a radio yet."))
    }

    fn reconnect_task(&self) -> MutexGuard<'_, Option<JoinHandle<()>>> {
        self.inner
            .radio
            .reconnect
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Looks for the paired radio until it is in use again: first after `first`, then waiting
/// longer between looks.
async fn reconnect(core: Weak<Inner>, first: Duration) {
    let mut wait = first;
    loop {
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        let Some(core) = upgrade(&core) else { return };
        match core.open_paired().await {
            Ok(()) => return,
            Err(why) => log::info!("the paired radio isn't back yet: {why}"),
        }
        wait = core.next_reconnect_wait(wait);
    }
}

/// The wait after one of `previous`: twice as long, from [`FIRST_RECONNECT_WAIT`] up to
/// [`LONGEST_RECONNECT_WAIT`].
fn next_wait(previous: Duration) -> Duration {
    (previous * 2).clamp(FIRST_RECONNECT_WAIT, LONGEST_RECONNECT_WAIT)
}

/// The listed port the paired radio is on: its own, or another with its USB serial number.
fn find_paired<'a>(listed: &'a [Port], pairing: &Pairing) -> Option<&'a Port> {
    listed
        .iter()
        .find(|port| port.path == pairing.port)
        .or_else(|| {
            let serial = pairing.serial_number.as_ref()?;
            listed
                .iter()
                .find(|port| port.serial_number.as_ref() == Some(serial))
        })
}

/// Opens the radio on `path` and reads what it says about itself. A radio that doesn't answer
/// is let go of, so that its port is free again.
async fn open_radio(
    ports: &dyn RadioPorts,
    path: &str,
) -> Result<(Arc<dyn Radio>, RadioInfo), RadioError> {
    let radio = ports.open(path).await?;
    match radio.info().await {
        Ok(info) => Ok((radio, info)),
        Err(error) => {
            let _ = radio.disconnect().await;
            Err(error)
        }
    }
}

/// What to tell the user when the radio on `path` couldn't be paired.
fn pairing_error(path: &str, error: &RadioError) -> CommandError {
    log::warn!("could not pair the radio on {path}: {error}");
    CommandError::new(match error {
        RadioError::Timeout => format!(
            "The device on {path} didn't answer. Is it a MeshCore radio with companion firmware \
             for USB?"
        ),
        RadioError::Link(_) | RadioError::NotConnected => {
            format!("Could not open {path}. Another app may be using it.")
        }
        _ => "Could not pair the radio. Try again.".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_longer_between_looks_up_to_ten_seconds() {
        let mut wait = Duration::ZERO;
        let mut waits = Vec::new();
        for _ in 0..7 {
            wait = next_wait(wait);
            waits.push(wait.as_secs());
        }
        assert_eq!(waits, [1, 2, 4, 8, 10, 10, 10]);
    }

    #[test]
    fn finds_the_paired_radio_on_its_port_or_by_its_serial_number() {
        let port = |path: &str, serial: Option<&str>| Port {
            path: path.to_owned(),
            description: None,
            serial_number: serial.map(str::to_owned),
        };
        let pairing = Pairing {
            port: "/dev/cu.usbmodem1101".to_owned(),
            serial_number: Some("A1B2".to_owned()),
            name: "Ada's radio".to_owned(),
            radio: RadioAddress::parse(&format!(
                r#"{{"v":0,"mesh":"meshcore","key":"{}","freq":869618,"bw":62500,"sf":8,"cr":8,"at":0}}"#,
                "ab".repeat(32)
            ))
            .unwrap(),
        };
        let own = [
            port("/dev/cu.usbmodem1101", None),
            port("/dev/cu.other", Some("A1B2")),
        ];
        assert_eq!(
            find_paired(&own, &pairing).unwrap().path,
            "/dev/cu.usbmodem1101"
        );
        let moved = [
            port("/dev/cu.usbserial-1", None),
            port("/dev/cu.usbmodem2101", Some("A1B2")),
        ];
        assert_eq!(
            find_paired(&moved, &pairing).unwrap().path,
            "/dev/cu.usbmodem2101"
        );
        let gone = [port("/dev/cu.usbmodem2101", Some("C3D4"))];
        assert_eq!(find_paired(&gone, &pairing), None);
    }
}

//! `pnpm verify:radio` (#133): runs the hardware checks against the radio lab, safely.
//!
//! ```sh
//! pnpm verify:radio              # checks every radio, then tests them on the air
//! pnpm verify:radio --dry-run    # lists the radios found and their firmware
//! pnpm verify:radio --usb-only   # leaves the radios on Bluetooth alone
//! ```
//!
//! It holds the lab lock for the whole run, finds radios by their USB serial numbers, refuses
//! to go on when a radio's firmware or settings differ from `radios.toml`, and saves every USB
//! radio's frames. It never changes a radio's firmware, frequency, preset or power; it does set
//! each USB radio's clock, except in a dry run. The over-the-air tests between USB radios (#134)
//! are in `messages.rs`. A radio on Bluetooth (#135, `ble.rs`) is reached over TCP through the
//! bridge in `tools/ble-bridge/`, and skipped when the bridge isn't running: the harness itself
//! never touches Bluetooth. Details: `docs/radio-lab.md`.

mod ble;
#[cfg(all(test, target_os = "macos"))]
mod ble_tests;
mod config;
mod fixtures;
#[cfg(test)]
mod hardware_tests;
mod lab;
mod link;
mod lock;
mod messages;
mod power;
/// The two-phone tests' own phones and relay.
#[path = "../../tests/support/mod.rs"]
mod support;
#[cfg(test)]
mod tests;

use std::future::Future;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::signal::unix::{SignalKind, signal};

use crate::lab::{Computer, Lab, Timing};
use crate::link::FrameLog;
use crate::lock::LabLock;
use crate::messages::{Bench, Waits};
use crate::power::Uhubctl;

const USAGE: &str = "Usage: pnpm verify:radio [--dry-run] [--usb-only]

  --dry-run    lists the radios and checks them against the lab; sends nothing
  --usb-only   leaves the radios on Bluetooth alone

Environment:
  RADIOS_TOML     the lab file (default: ~/factory/radios.toml)
  RADIO_LAB_LOCK  the lock (default: ~/factory/locks/radio-lab)
  RADIO_LOG_DIR   where frame logs go (default: .factory/radio in the repository)
  UHUBCTL         the uhubctl program (default: uhubctl)";

/// Where a run reads and writes.
struct Paths {
    config: PathBuf,
    lock: PathBuf,
    logs: PathBuf,
    uhubctl: PathBuf,
}

/// What a run may do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Mode {
    /// Lists and checks the radios, and sends nothing.
    dry_run: bool,
    /// Leaves Bluetooth radios alone.
    usb_only: bool,
}

impl Paths {
    fn from_env(var: impl Fn(&str) -> Option<PathBuf>) -> Result<Self, String> {
        let set = |name| var(name).filter(|path| !path.as_os_str().is_empty());
        let in_factory = |name, file: &str| {
            set(name)
                .or_else(|| set("HOME").map(|home| home.join("factory").join(file)))
                .ok_or_else(|| format!("Neither {name} nor HOME is set."))
        };
        Ok(Self {
            config: in_factory("RADIOS_TOML", "radios.toml")?,
            lock: in_factory("RADIO_LAB_LOCK", "locks/radio-lab")?,
            logs: set("RADIO_LOG_DIR").unwrap_or_else(|| {
                let core = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
                core.parent().unwrap_or(core).join(".factory/radio")
            }),
            uhubctl: set("UHUBCTL").unwrap_or_else(|| "uhubctl".into()),
        })
    }
}

/// How a run ended.
#[derive(Debug, PartialEq, Eq)]
enum End {
    Passed,
    Failed(String),
    Interrupted,
}

/// Runs `work` while holding the lab lock. The lock is released when this returns: after
/// success, failure, a panic, or Ctrl-C, SIGTERM or SIGHUP.
fn locked<F>(lock: &std::path::Path, work: impl FnOnce() -> F) -> End
where
    F: Future<Output = Result<(), String>>,
{
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return End::Failed(error.to_string()),
    };
    let mut held = None;
    let end = runtime.block_on(async {
        // Listen before taking the lock, so that no signal can end the process while it's held.
        let signals = (
            signal(SignalKind::interrupt()),
            signal(SignalKind::terminate()),
            signal(SignalKind::hangup()),
        );
        let (Ok(mut interrupt), Ok(mut terminate), Ok(mut hangup)) = signals else {
            return End::Failed("Could not listen for signals.".to_owned());
        };
        match LabLock::acquire(lock) {
            Ok(lock) => held = Some(lock),
            Err(error) => return End::Failed(error),
        }
        tokio::select! {
            result = work() => match result {
                Ok(()) => End::Passed,
                Err(error) => End::Failed(error),
            },
            _ = interrupt.recv() => End::Interrupted,
            _ = terminate.recv() => End::Interrupted,
            _ = hangup.recv() => End::Interrupted,
        }
    });
    // Waits for a power cycle still under way before the lab is free for the next run.
    drop(runtime);
    drop(held);
    end
}

fn verify(paths: &Paths, mode: Mode, out: &mut (dyn Write + Send)) -> End {
    // Before the lock: a missing lab file needs no radios.
    let config = match config::load(&paths.config) {
        Ok(config) => config,
        Err(error) => return End::Failed(error),
    };
    let _ = writeln!(out, "Radio lab: {}", paths.config.display());
    locked(&paths.lock, move || async move {
        let log = FrameLog::create(&paths.logs)?;
        let _ = writeln!(out, "Frame log: {}", log.path().display());
        let power = Uhubctl {
            program: paths.uhubctl.clone(),
        };
        let lab = Lab {
            config: &config,
            usb: Arc::new(Computer),
            power: &power,
            log: log.clone(),
            timing: Timing::default(),
            bluetooth: !mode.usb_only,
        };
        let result = checks_and_tests(&lab, Waits::default(), mode.dry_run, out).await;
        // Whether the tests passed or not: a failed run's frames are worth having too.
        for fixture in fixtures::save(log.path())? {
            let _ = writeln!(out, "Fixture: {}", fixture.display());
        }
        result?;
        if log.failed() {
            return Err(format!(
                "Some frames could not be written to {}.",
                log.path().display()
            ));
        }
        Ok(())
    })
}

/// Checks every radio and sets its clock, then, unless it is a dry run, tests over the air.
async fn checks_and_tests(
    lab: &Lab<'_>,
    waits: Waits,
    dry_run: bool,
    out: &mut (dyn Write + Send),
) -> Result<(), String> {
    lab.run(dry_run, out).await?;
    if dry_run {
        return Ok(());
    }
    let bench = Bench {
        config: lab.config.clone(),
        usb: lab.usb.clone(),
        log: lab.log.clone(),
        timing: lab.timing,
        waits,
    };
    messages::run(&bench, out).await
}

fn main() {
    let mut mode = Mode::default();
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--dry-run" => mode.dry_run = true,
            "--usb-only" => mode.usb_only = true,
            // pnpm passes the separator on.
            "--" => {}
            "--help" | "-h" => {
                println!("{USAGE}");
                return;
            }
            other => {
                eprintln!("Unknown argument {other:?}\n\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    let end = match Paths::from_env(|name| std::env::var_os(name).map(PathBuf::from)) {
        Ok(paths) => verify(&paths, mode, &mut std::io::stdout()),
        Err(error) => End::Failed(error),
    };
    match end {
        End::Passed => println!("verify:radio passed"),
        End::Failed(error) => {
            eprintln!("verify:radio failed: {error}");
            std::process::exit(1);
        }
        End::Interrupted => {
            eprintln!("verify:radio interrupted; the lab lock is released");
            std::process::exit(130);
        }
    }
}

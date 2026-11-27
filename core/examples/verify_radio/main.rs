//! `pnpm verify:radio` (#133): runs the hardware checks against the radio lab, safely.
//!
//! ```sh
//! pnpm verify:radio              # checks every USB radio, then that each still responds
//! pnpm verify:radio --dry-run    # lists the radios found and their firmware
//! pnpm verify:radio --allow-bluetooth   # once, by a person: lets macOS ask for Bluetooth
//! ```
//!
//! It holds the lab lock for the whole run, finds radios by their USB serial numbers, refuses
//! to go on when a radio's firmware or settings differ from `radios.toml`, and saves every frame.
//! It never changes a radio's firmware or settings. A radio on Bluetooth (#135) is reached
//! through meshcore-rs's Bluetooth link, from a launchd job of its own. Details:
//! `docs/radio-lab.md`.

mod ble;
#[cfg(test)]
mod ble_tests;
mod config;
mod lab;
#[cfg(target_os = "macos")]
mod launchd;
mod link;
mod lock;
mod power;
#[cfg(test)]
mod tests;

use std::future::Future;
use std::io::Write;
use std::path::PathBuf;

use tokio::signal::unix::{SignalKind, signal};

use crate::lab::{Computer, Lab, Timing};
use crate::link::FrameLog;
use crate::lock::LabLock;
use crate::power::Uhubctl;

const USAGE: &str = "Usage: pnpm verify:radio [--dry-run | --allow-bluetooth] [--usb-only]

  --dry-run             lists the radios and checks them against the lab; sends nothing
  --usb-only            leaves the radios on Bluetooth alone
  --allow-bluetooth     a dry run that may put the macOS Bluetooth prompt on the screen.
                        A person runs it once, at the Mac, and clicks Allow.

Environment:
  RADIOS_TOML               the lab file (default: ~/factory/radios.toml)
  RADIO_LAB_LOCK            the lock (default: ~/factory/locks/radio-lab)
  RADIO_LOG_DIR             where frame logs go (default: .factory/radio in the repository)
  UHUBCTL                   the uhubctl program (default: uhubctl)
  VERIFY_RADIO_BLE_PROGRAM  a copy of this program to hold the Bluetooth link (default: itself)";

/// Where a run reads and writes.
struct Paths {
    config: PathBuf,
    lock: PathBuf,
    logs: PathBuf,
    uhubctl: PathBuf,
    /// The program that holds the Bluetooth link, when it isn't this one.
    ble_program: Option<PathBuf>,
}

/// What a run may do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Mode {
    /// Lists and checks the radios, and sends nothing.
    dry_run: bool,
    /// May put the macOS Bluetooth prompt on the screen.
    may_ask: bool,
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
            ble_program: set("VERIFY_RADIO_BLE_PROGRAM"),
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

/// What starts the side that holds a Bluetooth link: a launchd job on a Mac, nothing elsewhere.
#[cfg(target_os = "macos")]
fn bluetooth(
    paths: &Paths,
    mode: Mode,
    log: &FrameLog,
) -> Result<Option<Box<dyn ble::Launcher>>, String> {
    if mode.usb_only {
        return Ok(None);
    }
    let program = match &paths.ble_program {
        Some(program) => program.clone(),
        None => std::env::current_exe().map_err(|error| error.to_string())?,
    };
    // Next to the frame log, and named after it.
    let output = log.path().with_extension("bluetooth.log");
    Ok(Some(Box::new(launchd::Launchd { program, output })))
}

#[cfg(not(target_os = "macos"))]
fn bluetooth(_: &Paths, _: Mode, _: &FrameLog) -> Result<Option<Box<dyn ble::Launcher>>, String> {
    Ok(None)
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
        let bluetooth = bluetooth(paths, mode, &log)?;
        let lab = Lab {
            config: &config,
            usb: &Computer,
            power: &power,
            log: log.clone(),
            timing: Timing::default(),
            ble: bluetooth.as_deref(),
            may_ask: mode.may_ask,
        };
        lab.run(mode.dry_run, out).await?;
        if log.failed() {
            return Err(format!(
                "Some frames could not be written to {}.",
                log.path().display()
            ));
        }
        Ok(())
    })
}

/// `--ble-child <port> <name> [--may-ask]`.
fn ble_child(arguments: &[String]) -> i32 {
    let (Some(port), Some(name)) = (
        arguments.first().and_then(|port| port.parse::<u16>().ok()),
        arguments.get(1),
    ) else {
        eprintln!("--ble-child is started by a run, with a port and a radio's name");
        return 2;
    };
    let may_ask = arguments.get(2).map(String::as_str) == Some("--may-ask");
    #[cfg(target_os = "macos")]
    return launchd::child(port, name, may_ask);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (port, name, may_ask);
        eprintln!("The Bluetooth run needs a Mac.");
        2
    }
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    // The side that holds the Bluetooth link: launchd starts it for a run (launchd.rs).
    if arguments.first().map(String::as_str) == Some("--ble-child") {
        std::process::exit(ble_child(&arguments[1..]));
    }
    let mut mode = Mode::default();
    for argument in arguments {
        match argument.as_str() {
            "--dry-run" => mode.dry_run = true,
            "--usb-only" => mode.usb_only = true,
            "--allow-bluetooth" => {
                mode.dry_run = true;
                mode.may_ask = true;
            }
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

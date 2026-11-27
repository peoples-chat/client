//! `pnpm verify:radio` (#133): runs the hardware checks against the radio lab, safely.
//!
//! ```sh
//! pnpm verify:radio              # checks every USB radio, then that each still responds
//! pnpm verify:radio --dry-run    # lists the radios found and their firmware
//! pnpm verify:radio --path-spike --offline   # the path spike between R1 and R2 (#140)
//! pnpm verify:radio --adverts 75             # how often they advertise on their own (#140)
//! ```
//!
//! It holds the lab lock for the whole run, finds radios by their USB serial numbers, refuses
//! to go on when a radio's firmware or settings differ from `radios.toml`, and saves every frame.
//! It never changes a radio's firmware or settings. Details: `docs/radio-lab.md`.

mod adverts;
mod config;
mod fixtures;
mod lab;
mod link;
mod lock;
// The path spike (#36), which also runs on its own as an example. Parts of its command line
// aren't used here.
#[path = "../path_spike/main.rs"]
#[allow(dead_code)]
mod path_spike;
mod power;
mod spike;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_140;

use std::future::Future;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use tokio::signal::unix::{SignalKind, signal};

use crate::lab::{Computer, Lab, Timing};
use crate::link::FrameLog;
use crate::lock::LabLock;
use crate::power::Uhubctl;

const USAGE: &str = "Usage: pnpm verify:radio [--dry-run]
       pnpm verify:radio --path-spike [--reverse] [path_spike options]
       pnpm verify:radio --adverts MINUTES
       pnpm verify:radio --fixtures FRAME_LOG FOLDER NAME   (no radios)

path_spike options: --paths, --texts, --repeat, --flood-repeat, --via, --offline, --out, …

Environment:
  RADIOS_TOML           the lab file (default: ~/factory/radios.toml)
  RADIO_LAB_LOCK        the lock (default: ~/factory/locks/radio-lab)
  RADIO_LOG_DIR         where frame logs go (default: .factory/radio in the repository)
  UHUBCTL               the uhubctl program (default: uhubctl)";

/// Where a run reads and writes.
struct Paths {
    config: PathBuf,
    lock: PathBuf,
    logs: PathBuf,
    uhubctl: PathBuf,
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

/// What a run does once it holds the lab.
#[derive(Debug, PartialEq)]
enum Task {
    /// The checks only.
    Check { dry_run: bool },
    /// The path spike between the first two USB radios.
    PathSpike {
        options: path_spike::plan::Options,
        reverse: bool,
    },
    /// Listen for the radios' own adverts.
    Adverts { period: Duration },
}

/// Reads the command line of a run on the radios.
fn task(args: &[String]) -> Result<Task, String> {
    // pnpm passes the separator on.
    let args: Vec<String> = args.iter().filter(|arg| *arg != "--").cloned().collect();
    match args.split_first() {
        None => Ok(Task::Check { dry_run: false }),
        Some((first, [])) if first == "--dry-run" => Ok(Task::Check { dry_run: true }),
        Some((first, rest)) if first == "--path-spike" => {
            let reverse = rest.iter().any(|arg| arg == "--reverse");
            let rest: Vec<String> = rest
                .iter()
                .filter(|arg| *arg != "--reverse")
                .cloned()
                .collect();
            Ok(Task::PathSpike {
                options: spike::options(&rest)?,
                reverse,
            })
        }
        Some((first, [minutes])) if first == "--adverts" => {
            let minutes: u64 = minutes
                .parse()
                .ok()
                .filter(|minutes| (1..=24 * 60).contains(minutes))
                .ok_or("--adverts takes a number of minutes")?;
            Ok(Task::Adverts {
                period: Duration::from_secs(minutes * 60),
            })
        }
        Some((other, _)) => Err(format!("Unknown argument {other:?}")),
    }
}

fn verify(paths: &Paths, task: Task, out: &mut (dyn Write + Send)) -> End {
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
            usb: &Computer,
            power: &power,
            log: log.clone(),
            timing: Timing::default(),
        };
        match task {
            Task::Check { dry_run } => lab.run(dry_run, out).await?,
            Task::PathSpike { options, reverse } => {
                lab.path_spike(options, reverse, &paths.logs, out).await?;
            }
            Task::Adverts { period } => {
                lab.adverts(period, out).await?;
            }
        }
        if log.failed() {
            return Err(format!(
                "Some frames could not be written to {}.",
                log.path().display()
            ));
        }
        Ok(())
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).filter(|arg| arg != "--").collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return;
    }
    if let [first, log, folder, name] = &args[..]
        && first == "--fixtures"
    {
        let heading = "Captured from a real radio in the lab (#140), on the private lab preset:\n\
                       869.850 MHz, 62.5 kHz, SF 7, CR 5, 2 dBm. The format: fake-radio/src/fixture.rs.";
        match fixtures::write(log.as_ref(), folder.as_ref(), name, heading) {
            Ok(written) => written
                .iter()
                .for_each(|path| println!("{}", path.display())),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    let task = match task(&args) {
        Ok(task) => task,
        Err(error) => {
            eprintln!("{error}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    let end = match Paths::from_env(|name| std::env::var_os(name).map(PathBuf::from)) {
        Ok(paths) => verify(&paths, task, &mut std::io::stdout()),
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

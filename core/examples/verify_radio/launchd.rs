//! The side that holds the Bluetooth link, as its own launchd job on a Mac (#135).
//!
//! `launchctl submit` starts this same binary with `--ble-child`. As a launchd job it is
//! responsible for its own use of Bluetooth: macOS reads the usage description built into the
//! binary (`Info.plist`), and its permission is its own, not that of the terminal or agent
//! that started the run.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use async_trait::async_trait;
use peoples_chat_core::radio::{MeshCoreRadio, RadioError};
use tokio::net::TcpStream;

use crate::ble::{Bluetooth, Launcher, Opened, Permission, serve};
use crate::lab::Timing;

/// The launchd job. There is one at a time: the lab lock sees to that.
const LABEL: &str = "chat.peoples.factory.verify-radio-ble";
const LAUNCHCTL: &str = "/bin/launchctl";

/// What macOS shows in the Bluetooth prompt, and reads before it lets the program near
/// Bluetooth at all. Without it in the binary, macOS ends the program.
#[used]
#[unsafe(link_section = "__TEXT,__info_plist")]
static INFO_PLIST: [u8; include_bytes!("Info.plist").len()] = *include_bytes!("Info.plist");

/// Starts the Bluetooth side with `launchctl submit`.
pub struct Launchd {
    /// The program to start: this one, or the copy that `VERIFY_RADIO_BLE_PROGRAM` names.
    pub program: PathBuf,
    /// Where the job's output goes.
    pub output: PathBuf,
}

/// Removes the job when dropped.
struct Job;

impl Drop for Job {
    fn drop(&mut self) {
        remove_job();
    }
}

fn remove_job() {
    let _ = Command::new(LAUNCHCTL)
        .args(["remove", LABEL])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[async_trait]
impl Launcher for Launchd {
    async fn launch(
        &self,
        control: SocketAddr,
        name: &str,
        may_ask: bool,
    ) -> Result<Box<dyn Send>, String> {
        // A run that was killed outright leaves its job behind.
        remove_job();
        let mut submit = Command::new(LAUNCHCTL);
        submit
            .args(["submit", "-l", LABEL, "-o"])
            .arg(&self.output)
            .arg("-e")
            .arg(&self.output)
            .arg("--")
            .arg(&self.program)
            .arg("--ble-child")
            .arg(control.port().to_string())
            .arg(name);
        if may_ask {
            submit.arg("--may-ask");
        }
        let job = Job;
        let status = submit
            .status()
            .map_err(|error| format!("{LAUNCHCTL}: {error}"))?;
        if !status.success() {
            return Err(format!("launchctl submit: {status}"));
        }
        Ok(Box::new(job))
    }

    fn hint(&self) -> String {
        format!(
            "Its output is in {}. It runs {} as the launchd job {LABEL}.",
            self.output.display(),
            self.program.display()
        )
    }
}

/// This Mac's Bluetooth, through meshcore-rs.
struct Mac;

#[async_trait]
impl Bluetooth for Mac {
    fn permission(&self) -> Permission {
        // SAFETY: `+[CBManager authorization]` takes no arguments and returns an NSInteger.
        // CoreBluetooth is linked in through meshcore-rs's Bluetooth link. Reading it uses no
        // Bluetooth and shows no prompt.
        let authorization: isize =
            unsafe { objc2::msg_send![objc2::class!(CBManager), authorization] };
        match authorization {
            3 => Permission::Allowed,
            2 => Permission::Denied,
            1 => Permission::Restricted,
            _ => Permission::NotAsked,
        }
    }

    async fn open(&self, name: &str, within: Duration) -> Result<Opened, RadioError> {
        Ok(Opened {
            radio: MeshCoreRadio::connect_ble(name, within).await?,
            _link: None,
        })
    }
}

/// Writes what the core logs to the job's output.
struct Output;

impl log::Log for Output {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!("{}: {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

/// `verify_radio --ble-child <port> <name> [--may-ask]`: serves the run that listens on
/// `port`, then removes its own job, so that launchd doesn't start it again.
pub fn child(port: u16, name: &str, may_ask: bool) -> i32 {
    let _ = log::set_logger(&Output);
    log::set_max_level(log::LevelFilter::Info);
    let code = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(async {
            match TcpStream::connect(("127.0.0.1", port)).await {
                Ok(stream) => {
                    eprintln!("serving the run on port {port} for {name}");
                    serve(stream, name, may_ask, &Mac, Timing::default()).await;
                    eprintln!("the run is over");
                    0
                }
                Err(error) => {
                    eprintln!("no run listens on port {port}: {error}");
                    1
                }
            }
        }),
        Err(error) => {
            eprintln!("{error}");
            1
        }
    };
    remove_job();
    code
}

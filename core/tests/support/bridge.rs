//! The Bluetooth bridge (#135), built from `tools/ble-bridge/` and run against a fake radio
//! over TCP. Run that way it never touches Bluetooth. Shared by the tests of the bridge and of
//! `verify:radio`.

// Each test binary uses its own part of this.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub fn repository() -> PathBuf {
    let core = Path::new(env!("CARGO_MANIFEST_DIR"));
    core.parent().unwrap().to_owned()
}

/// The bridge, built once per test program with the script that `install.sh` uses.
pub fn program() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        let folder = repository().join("target/ble-bridge-test");
        std::fs::create_dir_all(&folder).unwrap();
        // Built beside, then moved into place, so that no one runs half a program.
        let building = folder.join(format!("building-{}", std::process::id()));
        let status = Command::new("bash")
            .arg(repository().join("tools/ble-bridge/build.sh"))
            .arg(&building)
            .status()
            .unwrap();
        assert!(status.success(), "the bridge didn't build");
        let built = folder.join("ble-bridge");
        std::fs::rename(building, &built).unwrap();
        built
    })
}

/// A running bridge. Dropping it ends the process.
pub struct Bridge {
    process: Child,
    pub port: u16,
    pub control_port: u16,
}

impl Bridge {
    /// With a fake radio in place of the Bluetooth one, on free ports.
    pub fn start(fake: SocketAddr) -> Self {
        Self::pausing(fake, Duration::from_millis(300))
    }

    /// `pause`: how long it waits before it looks for a lost radio again.
    pub fn pausing(fake: SocketAddr, pause: Duration) -> Self {
        Self::with(&[
            "--fake-radio",
            &fake.to_string(),
            "--port",
            "0",
            "--control-port",
            "0",
            "--reconnect-ms",
            &pause.as_millis().to_string(),
        ])
    }

    /// With these arguments, which must name a fake radio: nothing in the tests may reach
    /// Bluetooth.
    pub fn with(arguments: &[&str]) -> Self {
        assert!(arguments.contains(&"--fake-radio"));
        let mut process = Command::new(program())
            .args(arguments)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(process.stdout.take().unwrap());
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        let ports: Vec<u16> = line
            .split_whitespace()
            .filter_map(|word| word.strip_prefix("127.0.0.1:")?.parse().ok())
            .collect();
        assert!(
            line.contains("listening") && ports.len() == 2,
            "the bridge said {line:?}"
        );
        // Keeps reading, so that the bridge can keep writing.
        std::thread::spawn(move || {
            for line in output.lines().map_while(Result::ok) {
                println!("{line}");
            }
        });
        Self {
            process,
            port: ports[0],
            control_port: ports[1],
        }
    }

    /// One command on the control port, and its answer.
    pub fn ask(&self, command: &str) -> String {
        let stream = TcpStream::connect(("127.0.0.1", self.control_port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        (&stream)
            .write_all(format!("{command}\n").as_bytes())
            .unwrap();
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply).unwrap();
        reply.trim().to_owned()
    }

    /// Waits until `status` answers `wanted`.
    pub async fn until(&self, wanted: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.ask("status") != wanted {
            assert!(Instant::now() < deadline, "the bridge never got {wanted}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

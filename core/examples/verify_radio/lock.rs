//! The lab lock: one run at a time uses the radios.
//!
//! The lock is a file that exists while a run holds it, created in one step so that two runs
//! can't both get it. A lock left behind (the process was killed outright) is never taken over:
//! the run fails and says who held it, and a person removes the file.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub struct LabLock {
    path: PathBuf,
    /// What this run wrote, so that it only ever removes its own lock.
    content: String,
}

impl LabLock {
    pub fn acquire(path: &Path) -> Result<Self, String> {
        let fail = |error: std::io::Error| {
            format!("Could not take the lab lock {}: {error}", path.display())
        };
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(fail)?;
        }
        let since = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let content = format!("pid={}\nsince_unix_ns={since}\n", std::process::id());
        let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(held(path));
            }
            Err(error) => return Err(fail(error)),
        };
        let lock = Self {
            path: path.to_owned(),
            content,
        };
        if let Err(error) = file.write_all(lock.content.as_bytes()) {
            let _ = std::fs::remove_file(path);
            return Err(fail(error));
        }
        Ok(lock)
    }
}

impl Drop for LabLock {
    fn drop(&mut self) {
        if std::fs::read_to_string(&self.path).is_ok_and(|found| found == self.content)
            && let Err(error) = std::fs::remove_file(&self.path)
        {
            eprintln!(
                "Could not release the lab lock {}: {error}",
                self.path.display()
            );
        }
    }
}

/// Why the lock at `path` can't be had.
fn held(path: &Path) -> String {
    let holder = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("pid="))
                .and_then(|pid| pid.trim().parse::<u32>().ok())
        })
        .filter(|&pid| pid > 0);
    let path = path.display();
    match holder {
        Some(pid) if running(pid) => format!(
            "The radio lab is in use: process {pid} holds {path}. Wait for that run to finish."
        ),
        Some(pid) => format!(
            "Stale lab lock: {path} names process {pid}, which isn't running. Check that no one \
             is using the radios, then remove the file."
        ),
        None => format!(
            "The radio lab is locked: {path} exists and names no process. Check that no one is \
             using the radios, then remove it."
        ),
    }
}

/// Whether a process of this user with this id is running.
fn running(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

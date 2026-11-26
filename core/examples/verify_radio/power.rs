//! Switching a radio's power off and on again through `uhubctl`.

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use async_trait::async_trait;

use crate::config::HubPort;

/// Seconds the port stays off.
const OFF_SECONDS: &str = "3";

#[async_trait]
pub trait Power: Send + Sync {
    /// Turns the hub port off and on again.
    async fn cycle(&self, hub: &HubPort) -> Result<(), String>;
}

pub struct Uhubctl {
    pub program: PathBuf,
}

#[async_trait]
impl Power for Uhubctl {
    async fn cycle(&self, hub: &HubPort) -> Result<(), String> {
        let mut command = Command::new(&self.program);
        command.args(["-l", &hub.location, "-p", &hub.port.to_string()]);
        command.args(["-a", "cycle", "-d", OFF_SECONDS]);
        // In its own process group, so that Ctrl-C can't stop it with the port still off.
        command.process_group(0);
        let output = tokio::task::spawn_blocking(move || command.output())
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| format!("could not run {}: {error}", self.program.display()))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "uhubctl failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
}

//! Turns a run's frame log into fixtures for the fake radio (#132): one file per radio, a frame
//! a line, `app <hex>` or `radio <hex>`. The log has already hidden what a radio says in private.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The fixtures in `log`, by radio label. Frames the harness refused to send never passed.
pub fn from_log(log: &str) -> Result<BTreeMap<String, String>, String> {
    let mut radios: BTreeMap<String, String> = BTreeMap::new();
    for (number, line) in log.lines().enumerate() {
        let bad = || format!("frame log line {} isn't a frame", number + 1);
        let frame: serde_json::Value = serde_json::from_str(line).map_err(|_| bad())?;
        let (Some(radio), Some(direction), Some(hex)) = (
            frame["radio"].as_str(),
            frame["dir"].as_str(),
            frame["frame"].as_str(),
        ) else {
            return Err(bad());
        };
        let sender = match direction {
            "to_radio" => "app",
            "from_radio" => "radio",
            _ => continue,
        };
        let text = radios.entry(radio.to_owned()).or_default();
        text.push_str(sender);
        text.push(' ');
        text.push_str(hex);
        text.push('\n');
    }
    Ok(radios)
}

/// Writes `<name>-<radio>.frames` into `folder` for each radio in the log at `log`, with
/// `heading` as comment lines at the top.
pub fn write(log: &Path, folder: &Path, name: &str, heading: &str) -> Result<Vec<PathBuf>, String> {
    let text = std::fs::read_to_string(log)
        .map_err(|error| format!("Could not read {}: {error}", log.display()))?;
    std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    let mut written = Vec::new();
    for (radio, frames) in from_log(&text)? {
        let path = folder.join(format!("{name}-{radio}.frames"));
        let comments: String = heading.lines().map(|line| format!("# {line}\n")).collect();
        std::fs::write(&path, format!("{comments}# Radio {radio}.\n\n{frames}"))
            .map_err(|error| format!("Could not write {}: {error}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

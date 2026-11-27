//! Each run's frames as fixtures for the fake radio (#134): next to the frame log, one file per
//! radio in the format `peoples_chat_fake_radio::Fixture` loads.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};

/// Writes the fixtures of the frame log at `log`, and returns their paths.
pub fn save(log: &Path) -> Result<Vec<PathBuf>, String> {
    let fail =
        |error: std::io::Error| format!("Could not save fixtures from {}: {error}", log.display());
    let text = std::fs::read_to_string(log).map_err(fail)?;
    let mut radios: BTreeMap<String, String> = BTreeMap::new();
    for line in text.lines() {
        let Ok(line) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let sender = match line["dir"].as_str() {
            Some("to_radio") => "app",
            Some("from_radio") => "radio",
            // A blocked command never passed.
            _ => continue,
        };
        let (Some(radio), Some(frame)) = (line["radio"].as_str(), line["frame"].as_str()) else {
            continue;
        };
        let fixture = radios.entry(radio.to_owned()).or_insert_with(|| {
            format!("# {radio}, captured by verify:radio. The format: fake-radio/src/fixture.rs.\n")
        });
        let _ = writeln!(fixture, "{sender} {frame}");
    }
    let stem = log
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("frames");
    radios
        .into_iter()
        .map(|(radio, fixture)| {
            let name: String = radio.chars().filter(char::is_ascii_alphanumeric).collect();
            let path = log.with_file_name(format!("{stem}-{name}.frames"));
            std::fs::write(&path, fixture).map_err(fail)?;
            Ok(path)
        })
        .collect()
}

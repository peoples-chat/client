//! Issue #140: the numbers in `docs/radio-paths.md` are the ones in the lab run's CSV files and
//! frame logs. Each test works its table rows out from the captures and looks for them in the
//! page. `cargo test -p peoples-chat-core --example verify_radio -- results_140 --nocapture`
//! prints them.

use std::collections::BTreeMap;
use std::path::Path;

const PATHS: [&str; 3] = ["text", "raw", "channel"];

fn repository(file: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{file}: {error}"))
}

fn page() -> String {
    repository("docs/radio-paths.md")
}

type Row = BTreeMap<String, String>;

/// The sends of a run. No field of these files holds a line break.
fn run(file: &str) -> Vec<Row> {
    let text = repository(&format!("docs/radio-paths/{file}"));
    let mut lines = text.lines().map(fields);
    let header = lines.next().unwrap();
    lines
        .map(|line| header.iter().cloned().zip(line).collect())
        .collect()
}

fn fields(line: &str) -> Vec<String> {
    let mut fields = vec![String::new()];
    let mut quoted = false;
    for character in line.chars() {
        match character {
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(String::new()),
            other => fields.last_mut().unwrap().push(other),
        }
    }
    fields
}

fn select<'a>(rows: &'a [Row], phase: &str, path: &str, route: &str) -> Vec<&'a Row> {
    rows.iter()
        .filter(|row| row["phase"] == phase && row["path"] == path && row["route"] == route)
        .collect()
}

fn of_size<'a>(rows: &[&'a Row], text_bytes: usize) -> Vec<&'a Row> {
    rows.iter()
        .copied()
        .filter(|row| row["text_bytes"] == text_bytes.to_string())
        .collect()
}

fn median(rows: &[&Row], column: &str) -> Option<f64> {
    let mut values: Vec<f64> = rows
        .iter()
        .filter_map(|row| row[column].parse().ok())
        .collect();
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    match values.len() {
        0 => None,
        even if even % 2 == 0 => Some((values[middle - 1] + values[middle]) / 2.0),
        _ => Some(values[middle]),
    }
}

/// One cell: delivered of sent, the median time to arrive and, for text, to the confirmation.
fn cell(rows: &[&Row]) -> String {
    if rows.is_empty() {
        return "not sent".to_owned();
    }
    if rows.iter().all(|row| row["outcome"] == "refused") {
        return "refused".to_owned();
    }
    let delivered = rows.iter().filter(|row| row["intact"] == "true").count();
    let mut cell = format!("{delivered}/{}", rows.len());
    if let Some(ms) = median(rows, "delivery_ms") {
        cell.push_str(&format!(", {ms:.0} ms"));
    }
    if let Some(ms) = median(rows, "ack_ms") {
        cell.push_str(&format!(", ack {ms:.0} ms"));
    }
    cell
}

fn sizes(rows: &[Row]) -> Vec<usize> {
    let mut sizes: Vec<usize> = rows
        .iter()
        .map(|row| row["text_bytes"].parse().unwrap())
        .collect();
    sizes.sort_unstable();
    sizes.dedup();
    sizes
}

fn assert_in_page(lines: &[String]) {
    println!("{}", lines.join("\n"));
    let page = page();
    for line in lines {
        assert!(page.contains(line), "docs/radio-paths.md lacks: {line}");
    }
}

#[test]
fn issue_140_the_page_gives_each_directions_delivery_and_times() {
    for file in ["lab-r1-to-r2.csv", "lab-r2-to-r1.csv"] {
        let rows = run(file);
        let mut lines = Vec::new();
        for route in ["zero hop", "flood"] {
            for size in sizes(&rows) {
                let cells: Vec<String> = PATHS
                    .iter()
                    .map(|path| cell(&of_size(&select(&rows, "rounds", path, route), size)))
                    .collect();
                if route == "flood" && size > 96 {
                    continue;
                }
                lines.push(format!("| {route} | {size} B | {} |", cells.join(" | ")));
            }
        }
        assert_in_page(&lines);
        // Everything went as the firmware source says, and no event was missed.
        assert!(rows.iter().all(|row| row["as_expected"] == "true"));
        assert!(rows.iter().all(|row| row["lagged"] == "0"));
        // Every length the receiving radio logged is the one worked out.
        for row in rows.iter().filter(|row| !row["logged_bytes"].is_empty()) {
            assert_eq!(row["logged_bytes"], row["on_air_bytes"], "{row:?}");
        }
    }
}

#[test]
fn issue_140_the_page_gives_sizes_on_air_as_logged_and_worked_out() {
    let rows = run("lab-r1-to-r2.csv");
    let mut lines = Vec::new();
    for path in PATHS {
        for size in sizes(&rows) {
            let sent = of_size(&select(&rows, "rounds", path, "zero hop"), size);
            let Some(row) = sent.iter().find(|row| row["outcome"] != "refused") else {
                continue;
            };
            let logged = sent
                .iter()
                .map(|row| row["logged_bytes"].as_str())
                .find(|bytes| !bytes.is_empty())
                .map_or("not logged".to_owned(), |bytes| format!("{bytes} B"));
            lines.push(format!(
                "| {path} | {size} B | {} B | {} B | {logged} | {} ms |",
                row["payload_bytes"], row["on_air_bytes"], row["airtime_ms"]
            ));
        }
    }
    assert_in_page(&lines);
}

#[test]
fn issue_140_the_page_says_how_arrival_follows_the_worked_out_airtime() {
    let rows = run("lab-r1-to-r2.csv");
    let over: Vec<f64> = rows
        .iter()
        .filter(|row| row["phase"] == "rounds" && row["route"] == "zero hop")
        .filter_map(|row| {
            let delivery: f64 = row["delivery_ms"].parse().ok()?;
            Some(delivery - row["airtime_ms"].parse::<f64>().ok()?)
        })
        .collect();
    let least = over.iter().copied().fold(f64::MAX, f64::min);
    let most = over.iter().copied().fold(f64::MIN, f64::max);
    assert_in_page(&[format!(
        "the worked-out airtime plus {least:.0} to {most:.0} ms"
    )]);
}

#[test]
fn issue_140_the_page_compares_each_budget_with_the_target() {
    let rows = run("lab-r1-to-r2.csv");
    let mut lines = Vec::new();
    for path in PATHS {
        let sent = select(&rows, "rounds", path, "zero hop");
        let budget = sizes(&rows)
            .into_iter()
            .filter(|&size| {
                let sent = of_size(&sent, size);
                !sent.is_empty() && sent.iter().all(|row| row["intact"] == "true")
            })
            .max()
            .unwrap();
        let against = match budget {
            ..100 => format!("{} B short of 100", 100 - budget),
            100..=150 => "inside 100 to 150".to_owned(),
            _ => "above 150".to_owned(),
        };
        lines.push(format!("| {path} | {budget} B | {against} |"));
    }
    assert_in_page(&lines);
}

/// SNR and RSSI of the packets radio R2 logged from R1, from its capture: `0x88 | SNR×4 |
/// RSSI | header | …`, with the payload type in the header.
#[test]
fn issue_140_the_page_gives_signal_strength_for_each_path() {
    let capture = repository("fake-radio/tests/fixtures/lab-path-spike-R2.frames");
    let mut lines = Vec::new();
    for (path, payload_type) in [("text", 2), ("raw", 15), ("channel", 6)] {
        let heard: Vec<(f64, i8)> = capture
            .lines()
            .filter_map(|line| line.strip_prefix("radio 88"))
            .map(|hex| hex::decode(hex).unwrap())
            .filter(|frame| (frame[2] >> 2) & 0x0f == payload_type)
            .map(|frame| (f64::from(frame[0] as i8) / 4.0, frame[1] as i8))
            .collect();
        let snr = heard.iter().map(|(snr, _)| *snr);
        let rssi = heard.iter().map(|(_, rssi)| *rssi);
        lines.push(format!(
            "| {path} | {} | {} to {} dB | {} to {} dBm |",
            heard.len(),
            snr.clone().fold(f64::MAX, f64::min),
            snr.fold(f64::MIN, f64::max),
            rssi.clone().min().unwrap(),
            rssi.max().unwrap(),
        ));
    }
    assert_in_page(&lines);
}

#[test]
fn issue_140_the_page_says_what_arrived_while_the_receiver_was_away() {
    let rows = run("lab-r1-to-r2.csv");
    let lines: Vec<String> = PATHS
        .iter()
        .map(|path| {
            let row = select(&rows, "receiver away", path, "zero hop")[0];
            let confirmed = row["ack_ms"]
                .parse::<f64>()
                .map_or("none".to_owned(), |ms| format!("{ms:.0} ms"));
            format!("| {path} | {} | {confirmed} |", row["outcome"])
        })
        .collect();
    assert_in_page(&lines);
}

#[test]
fn issue_140_the_page_names_the_lab_preset_recommends_a_path_and_notes_adverts() {
    let page = page();
    for says in [
        "869.850 MHz, 62.5 kHz, SF 7, CR 5, 2 dBm",
        "not EU/UK Narrow",
        "## Recommendation",
        "**Use text messages**",
        "**No proposal to MeshCore is needed",
        "## Adverts the radios send on their own",
        "#44",
        "issues/36#issuecomment-",
    ] {
        assert!(page.contains(says), "docs/radio-paths.md lacks: {says}");
    }
}

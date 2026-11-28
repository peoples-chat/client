//! What a run found: one row per send, written as CSV, and a table of the results by path and
//! size.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use super::frames::Kind;

/// What happened to one packet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    /// The part of the run: `rounds`, `receiver away` or `radio off`.
    pub phase: String,
    pub round: usize,
    pub kind: Option<Kind>,
    /// `zero hop`, `flood` or `via …`.
    pub route: String,
    pub text_bytes: usize,
    /// The format v0 packet.
    pub packet_bytes: usize,
    /// What went to the radio: base64 for a text message.
    pub payload_bytes: usize,
    /// Worked out from the firmware's packet layout.
    pub on_air_bytes: usize,
    pub airtime_ms: f64,
    /// What the firmware source says happens.
    pub expected: String,
    /// The radio's answer: `ok`, `sent direct`, `sent flood`, `refused: …` or `no answer`.
    pub reply: String,
    pub reply_ms: Option<f64>,
    /// The radio's estimate of how long the confirmation may take. Text messages only.
    pub radio_timeout_ms: Option<u64>,
    pub ack_ms: Option<f64>,
    pub delivery_ms: Option<f64>,
    /// The packet arrived as sent, and opened with the receiving side's key.
    pub intact: Option<bool>,
    /// `direct`, or `flood, N hops`.
    pub received_route: String,
    pub snr: Option<f32>,
    pub rssi: Option<i16>,
    /// The packet's length on air as the receiving radio logged it. The radio logs packets up to
    /// 173 bytes.
    pub logged_bytes: Option<usize>,
    /// How many copies of the packet the receiving radio heard: repeaters pass floods on.
    pub copies_heard: usize,
    /// `delivered`, `cut short`, `refused`, `lost`, `queued on the radio`, …
    pub outcome: String,
    /// The outcome is what the firmware source says.
    pub as_expected: bool,
    pub note: String,
}

impl Row {
    pub fn delivered(&self) -> bool {
        self.delivery_ms.is_some()
    }

    /// The radio took the packet to send. The on-air figures of one it refused are what it
    /// would have taken.
    pub fn went_on_air(&self) -> bool {
        !self.reply.starts_with("refused") && self.reply != "no answer"
    }

    pub fn path(&self) -> &'static str {
        self.kind.map_or("", Kind::name)
    }
}

const HEADER: [&str; 26] = [
    "phase",
    "round",
    "path",
    "route",
    "text_bytes",
    "packet_bytes",
    "payload_bytes",
    "on_air_bytes",
    "airtime_ms",
    "expected",
    "reply",
    "reply_ms",
    "radio_timeout_ms",
    "ack_ms",
    "delivered",
    "delivery_ms",
    "intact",
    "received_route",
    "snr",
    "rssi",
    "logged_bytes",
    "copies_heard",
    "outcome",
    "as_expected",
    "note",
    "lagged",
];

/// The rows as CSV, with a header line. `missed` counts events the logs fell behind on.
pub fn csv(rows: &[Row], missed: u64) -> String {
    let mut out = HEADER.join(",");
    out.push('\n');
    for row in rows {
        let fields = [
            row.phase.clone(),
            row.round.to_string(),
            row.path().to_owned(),
            row.route.clone(),
            row.text_bytes.to_string(),
            row.packet_bytes.to_string(),
            row.payload_bytes.to_string(),
            row.on_air_bytes.to_string(),
            format!("{:.1}", row.airtime_ms),
            row.expected.clone(),
            row.reply.clone(),
            optional(row.reply_ms.map(|ms| format!("{ms:.1}"))),
            optional(row.radio_timeout_ms.map(|ms| ms.to_string())),
            optional(row.ack_ms.map(|ms| format!("{ms:.1}"))),
            row.delivered().to_string(),
            optional(row.delivery_ms.map(|ms| format!("{ms:.1}"))),
            optional(row.intact.map(|intact| intact.to_string())),
            row.received_route.clone(),
            optional(row.snr.map(|snr| snr.to_string())),
            optional(row.rssi.map(|rssi| rssi.to_string())),
            optional(row.logged_bytes.map(|bytes| bytes.to_string())),
            row.copies_heard.to_string(),
            row.outcome.clone(),
            row.as_expected.to_string(),
            row.note.clone(),
            missed.to_string(),
        ];
        let line: Vec<_> = fields.iter().map(|field| quoted(field)).collect();
        out.push_str(&line.join(","));
        out.push('\n');
    }
    out
}

fn optional(value: Option<String>) -> String {
    value.unwrap_or_default()
}

fn quoted(field: &str) -> String {
    if field.contains([',', '"', '\n']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_owned()
    }
}

/// The results by phase, path, route and size, as a table for the terminal.
pub fn table(rows: &[Row]) -> String {
    let mut groups: BTreeMap<(String, Option<Kind>, String, usize), Vec<&Row>> = BTreeMap::new();
    for row in rows {
        groups
            .entry((
                row.phase.clone(),
                row.kind,
                row.route.clone(),
                row.text_bytes,
            ))
            .or_default()
            .push(row);
    }
    let mut lines = vec![
        [
            "phase",
            "path",
            "route",
            "text",
            "payload",
            "on air",
            "logged",
            "airtime",
            "sent",
            "delivered",
            "delivery",
            "ack",
            "copies",
            "outcome",
            "source",
        ]
        .map(str::to_owned),
    ];
    for ((phase, kind, route, text_bytes), rows) in &groups {
        let first = rows[0];
        let sent = rows.iter().filter(|row| row.went_on_air()).count();
        let delivered = rows.iter().filter(|row| row.delivered()).count();
        let mut outcomes: Vec<&str> = rows.iter().map(|row| row.outcome.as_str()).collect();
        outcomes.dedup();
        let logged = rows.iter().find_map(|row| row.logged_bytes);
        lines.push([
            phase.clone(),
            kind.map_or("", Kind::name).to_owned(),
            route.clone(),
            format!("{text_bytes} B"),
            format!("{} B", first.payload_bytes),
            if first.went_on_air() {
                format!("{} B", first.on_air_bytes)
            } else {
                "-".to_owned()
            },
            logged.map_or("-".to_owned(), |bytes| format!("{bytes} B")),
            if first.went_on_air() {
                format!("{:.0} ms", first.airtime_ms)
            } else {
                "-".to_owned()
            },
            sent.to_string(),
            format!("{delivered}/{}", rows.len()),
            median_ms(rows.iter().filter_map(|row| row.delivery_ms)),
            median_ms(rows.iter().filter_map(|row| row.ack_ms)),
            rows.iter()
                .map(|row| row.copies_heard)
                .max()
                .unwrap_or(0)
                .to_string(),
            outcomes.join(", "),
            if rows.iter().all(|row| row.as_expected) {
                "as expected".to_owned()
            } else {
                format!("differs: {}", first.expected)
            },
        ]);
    }
    let widths: Vec<usize> = (0..lines[0].len())
        .map(|column| {
            lines
                .iter()
                .map(|line| line[column].len())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = String::new();
    for line in &lines {
        for (field, width) in line.iter().zip(&widths) {
            let _ = write!(out, "{field:<width$}  ");
        }
        out.truncate(out.trim_end().len());
        out.push('\n');
    }
    out
}

fn median_ms(values: impl Iterator<Item = f64>) -> String {
    let mut values: Vec<f64> = values.collect();
    if values.is_empty() {
        return "-".to_owned();
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    let median = if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    };
    format!("{median:.0} ms")
}

/// For each path, the longest chat text that arrived intact in every round of the main run:
/// the path's text budget, against the 100 to 150 bytes the spec aims for.
pub fn budgets(rows: &[Row]) -> Vec<(Kind, Option<usize>)> {
    let mut sizes: BTreeMap<(Kind, usize), bool> = BTreeMap::new();
    for row in rows
        .iter()
        .filter(|row| row.phase == ROUNDS && row.route != "flood")
    {
        if let Some(kind) = row.kind {
            let fine = row.intact == Some(true);
            sizes
                .entry((kind, row.text_bytes))
                .and_modify(|all| *all &= fine)
                .or_insert(fine);
        }
    }
    let mut budgets: Vec<(Kind, Option<usize>)> = Vec::new();
    for ((kind, text_bytes), fine) in sizes {
        if budgets.last().is_none_or(|(last, _)| *last != kind) {
            budgets.push((kind, None));
        }
        if fine && let Some(budget) = budgets.last_mut() {
            budget.1 = Some(text_bytes);
        }
    }
    budgets
}

/// The phase of the main measurement.
pub const ROUNDS: &str = "rounds";

pub fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(kind: Kind, text_bytes: usize, intact: Option<bool>) -> Row {
        Row {
            phase: ROUNDS.to_owned(),
            kind: Some(kind),
            route: "zero hop".to_owned(),
            text_bytes,
            delivery_ms: intact.map(|_| 500.0),
            intact,
            outcome: if intact.is_some() {
                "delivered"
            } else {
                "lost"
            }
            .to_owned(),
            as_expected: true,
            ..Row::default()
        }
    }

    #[test]
    fn csv_has_a_header_and_quotes_what_needs_it() {
        let mut first = row(Kind::Raw, 10, Some(true));
        first.note = "path learned, \"twice\"".to_owned();
        let out = csv(&[first], 0);
        let lines: Vec<_> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].split(',').count(), HEADER.len());
        assert!(lines[1].starts_with("rounds,0,raw,zero hop,10,"));
        assert!(lines[1].contains(",500.0,true,"));
        assert!(lines[1].ends_with(",\"path learned, \"\"twice\"\"\",0"));
    }

    #[test]
    fn the_table_groups_by_path_route_and_size() {
        let rows = [
            row(Kind::Text, 10, Some(true)),
            row(Kind::Text, 10, Some(true)),
            row(Kind::Text, 10, None),
        ];
        let table = table(&rows);
        let lines: Vec<_> = table.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[1].contains("2/3"), "{table}");
        assert!(lines[1].contains("500 ms"), "{table}");
        assert!(lines[1].contains("delivered, lost"), "{table}");
    }

    #[test]
    fn a_budget_is_the_longest_text_that_always_arrived() {
        let rows = [
            row(Kind::Text, 10, Some(true)),
            row(Kind::Text, 96, Some(true)),
            row(Kind::Text, 97, None),
            row(Kind::Raw, 141, Some(true)),
            row(Kind::Raw, 148, Some(true)),
            row(Kind::Raw, 148, None),
            row(Kind::Channel, 10, None),
        ];
        assert_eq!(
            budgets(&rows),
            [
                (Kind::Text, Some(96)),
                (Kind::Raw, Some(141)),
                (Kind::Channel, None)
            ]
        );
    }

    #[test]
    fn medians() {
        assert_eq!(median_ms([3.0, 1.0, 2.0].into_iter()), "2 ms");
        assert_eq!(median_ms([4.0, 1.0, 2.0, 6.0].into_iter()), "3 ms");
        assert_eq!(median_ms(std::iter::empty()), "-");
    }
}

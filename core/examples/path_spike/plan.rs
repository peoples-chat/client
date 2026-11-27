//! What a run sends: its options, the order of the sends, the packets, and the pace.

use std::path::PathBuf;
use std::time::Duration;

use peoples_chat_core::radio::format::{
    MAX_RADIO_TEXT_BYTES, OVERHEAD_BYTES, open_text, seal_text, to_text_message,
};
use peoples_chat_core::radio::{RadioKeys, RadioSettings};

use super::airtime::time_on_air;
use super::frames::{self, Expected, Kind, Route};

pub const USAGE: &str = "\
Measures the three MeshCore paths for the app's radio packets between two companion radios on
USB serial (#36): text messages, raw data and channel data. docs/radio-paths.md has the steps.

Usage:
  path_spike                              lists the serial ports
  path_spike <port A> <port B> [options]  measures, sending from A to B
  path_spike --plan [options]             shows what a run sends, without radios

Options:
  --paths text,raw,channel  the paths to measure (default: all three)
  --texts 10,48,96          chat text sizes in bytes (default: 10,48,96,97,126,141,142,148,149)
  --repeat N                rounds of direct sends (default: 3)
  --flood-repeat N          rounds of floods, at sizes up to 96 bytes (default: 2; 0 for none)
  --via HEX                 send direct through these repeaters, by key hash, instead of zero
                            hop; no floods then
  --hash-size N             bytes per repeater hash in --via (default: 1)
  --channel-index N         the channel slot for the test channel (default: the radio's last)
  --tx-power DBM            transmit power for the run, put back afterwards
  --duty-cycle PERCENT      how much of the time radio A may transmit (default and most: 10)
  --offline                 also send while radio B's link is closed
  --radio-off               also send while radio B is switched off; asks you to switch it
  --out FILE                the CSV file (default: path-spike-<unix time>.csv)";

/// The chat text sizes a run sends by default, in bytes: short, medium and the longest that
/// format v0 sends in a text message (96), one byte more, a 150-byte packet, then each side of
/// the limits of channel data (141 and 142) and raw data (148 and 149).
pub const DEFAULT_TEXTS: [usize; 9] = [10, 48, 96, 97, 126, 141, 142, 148, 149];
/// The chat text size for the receiver-away and radio-off checks.
pub const CHECK_TEXT: usize = 48;
/// The most of the time a radio may transmit in 869.4 to 869.65 MHz (CEPT ERC Recommendation
/// 70-03, Annex 1), in percent.
const MOST_DUTY_CYCLE: f64 = 10.0;
/// Chat text sizes past this are no packet the app would make.
const LONGEST_TEXT: usize = 255;

/// MeshCore's EU/UK narrow preset, which `--plan` assumes. A run uses the radios' own settings.
pub const EU_UK_NARROW: RadioSettings = RadioSettings {
    frequency_mhz: 869.618,
    bandwidth_khz: 62.5,
    spreading_factor: 8,
    coding_rate: 8,
    tx_power_dbm: 22,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// Radio A's and radio B's serial ports. `--plan` needs none.
    pub ports: Option<(String, String)>,
    pub kinds: Vec<Kind>,
    pub texts: Vec<usize>,
    pub repeat: usize,
    pub flood_repeat: usize,
    pub via: Option<Route>,
    pub channel_index: Option<u8>,
    pub tx_power: Option<u8>,
    /// In percent.
    pub duty_cycle: f64,
    pub offline: bool,
    pub radio_off: bool,
    pub out: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            ports: None,
            kinds: Kind::ALL.to_vec(),
            texts: DEFAULT_TEXTS.to_vec(),
            repeat: 3,
            flood_repeat: 2,
            via: None,
            channel_index: None,
            tx_power: None,
            duty_cycle: MOST_DUTY_CYCLE,
            offline: false,
            radio_off: false,
            out: None,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Command {
    ListPorts,
    Help,
    Plan(Options),
    Run(Options),
}

/// Reads the command line, without the program's name.
pub fn parse_args(args: &[String]) -> Result<Command, String> {
    if args.is_empty() {
        return Ok(Command::ListPorts);
    }
    let mut options = Options::default();
    let mut ports = Vec::new();
    let mut plan = false;
    let mut via = None;
    let mut hash_size = 1;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .map(String::as_str)
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "--plan" => plan = true,
            "--offline" => options.offline = true,
            "--radio-off" => options.radio_off = true,
            "--paths" => {
                let mut kinds = Vec::new();
                for name in value()?.split(',') {
                    let kind = Kind::parse(name.trim())
                        .ok_or_else(|| format!("{name:?} is not a path: text, raw or channel"))?;
                    if !kinds.contains(&kind) {
                        kinds.push(kind);
                    }
                }
                options.kinds = kinds;
            }
            "--texts" => {
                options.texts = value()?
                    .split(',')
                    .map(|size| number(arg, size.trim()))
                    .collect::<Result<_, _>>()?;
                if let Some(size) = options.texts.iter().find(|&&size| size > LONGEST_TEXT) {
                    return Err(format!(
                        "{size} bytes is longer than any chat text the app sends"
                    ));
                }
            }
            "--repeat" => options.repeat = number(arg, value()?)?,
            "--flood-repeat" => options.flood_repeat = number(arg, value()?)?,
            "--via" => via = Some(value()?.to_owned()),
            "--hash-size" => hash_size = number(arg, value()?)?,
            "--channel-index" => options.channel_index = Some(number(arg, value()?)?),
            "--tx-power" => options.tx_power = Some(number(arg, value()?)?),
            "--duty-cycle" => {
                options.duty_cycle = value()?
                    .parse()
                    .ok()
                    .filter(|share| *share > 0.0 && *share <= MOST_DUTY_CYCLE)
                    .ok_or_else(|| {
                        format!(
                            "--duty-cycle is a percentage above 0 and at most {MOST_DUTY_CYCLE}"
                        )
                    })?;
            }
            "--out" => options.out = Some(PathBuf::from(value()?)),
            flag if flag.starts_with('-') => return Err(format!("{flag} is not an option")),
            port => ports.push(port.to_owned()),
        }
    }
    if let Some(hex) = via {
        options.via = Some(Route::via(&hex, hash_size)?);
    }
    if options.repeat == 0 {
        return Err("--repeat must be at least 1".to_owned());
    }
    if options.texts.is_empty() || options.kinds.is_empty() {
        return Err("nothing to send".to_owned());
    }
    match (ports.len(), plan) {
        (0, true) => Ok(Command::Plan(options)),
        (2, _) => {
            options.ports = Some((ports[0].clone(), ports[1].clone()));
            Ok(if plan {
                Command::Plan(options)
            } else {
                Command::Run(options)
            })
        }
        (count, _) => Err(format!(
            "give two serial ports, radio A's and radio B's, not {count}"
        )),
    }
}

fn number<T: std::str::FromStr>(flag: &str, text: &str) -> Result<T, String> {
    text.parse()
        .map_err(|_| format!("{flag} takes a number, not {text:?}"))
}

/// A path along a route.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Variant {
    pub kind: Kind,
    pub route: Route,
}

/// One send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub round: usize,
    pub variant: Variant,
    pub text_bytes: usize,
}

/// The paths and routes a run measures, in the order each round sends them.
pub fn variants(options: &Options) -> Vec<Variant> {
    let direct = options.via.clone().unwrap_or_else(Route::zero_hop);
    // Through repeaters, only direct sends: the floods are the same as without.
    let floods = options.via.is_none();
    let mut variants = Vec::new();
    for &kind in &options.kinds {
        let flood = Variant {
            kind,
            route: Route::Flood,
        };
        // The text flood goes first: it also teaches radio A the route the direct ones take.
        if floods && kind == Kind::Text {
            variants.push(flood.clone());
        }
        variants.push(Variant {
            kind,
            route: direct.clone(),
        });
        if floods && kind != Kind::Text {
            variants.push(flood);
        }
    }
    variants
}

/// Every send of a run, round by round, so that changes over time spread across the paths.
pub fn schedule(options: &Options) -> Vec<Step> {
    let mut steps = Vec::new();
    for round in 0..options.repeat.max(options.flood_repeat) {
        for variant in variants(options) {
            let flood = variant.route.is_flood();
            if round
                >= if flood {
                    options.flood_repeat
                } else {
                    options.repeat
                }
            {
                continue;
            }
            for &text_bytes in &options.texts {
                // Every repeater in range passes a flood on, so floods stay at the sizes that
                // format v0 sends.
                if flood && text_bytes > MAX_RADIO_TEXT_BYTES {
                    continue;
                }
                steps.push(Step {
                    round,
                    variant: variant.clone(),
                    text_bytes,
                });
            }
        }
    }
    steps
}

/// The made-up chat secret and accounts the spike seals packets with: the radios aren't paired
/// with accounts, and only the packets' sizes and their arrival intact matter here.
const CHAT_SECRET: [u8; 32] = [0x36; 32];
const ACCOUNT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ACCOUNT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const EPOCH: u64 = 1;

/// Radio A's sending key, and the same key as radio B's side opens packets with.
pub struct Keys {
    pub send: RadioKeys,
    pub receive: RadioKeys,
}

impl Keys {
    pub fn new() -> Self {
        let derive = |own, contact| {
            RadioKeys::derive(&CHAT_SECRET, EPOCH, own, contact).expect("the test keys are valid")
        };
        Self {
            send: derive(ACCOUNT_A, ACCOUNT_B),
            receive: derive(ACCOUNT_B, ACCOUNT_A),
        }
    }

    /// A format v0 packet with `text_bytes` of chat text, as radio A's side seals it.
    pub fn seal(&self, counter: u64, text_bytes: usize, sent_at: u32) -> Vec<u8> {
        seal_text(
            &self.send.send,
            EPOCH,
            counter,
            sent_at,
            &chat_text(counter, text_bytes),
        )
    }

    /// Whether `packet` opens on radio B's side to the chat text it was sealed with.
    pub fn opens(&self, packet: &[u8], counter: u64, text_bytes: usize) -> bool {
        open_text(&self.receive.receive, packet, counter)
            .is_ok_and(|opened| opened.text == chat_text(counter, text_bytes))
    }
}

/// Chat text of exactly `len` bytes for packet `counter`. ASCII, so bytes and characters agree.
pub fn chat_text(counter: u64, len: usize) -> String {
    let mut text = format!("#{counter} ");
    while text.len() < len {
        text.push_str("the quick brown fox jumps over the lazy dog ");
    }
    text.truncate(len);
    text
}

/// The size of the format v0 packet for `text_bytes` of chat text.
pub fn packet_bytes(text_bytes: usize) -> usize {
    OVERHEAD_BYTES + text_bytes
}

/// What goes to the radio for a packet: base64 text for a text message, the bytes otherwise.
pub fn payload(kind: Kind, packet: &[u8]) -> Vec<u8> {
    match kind {
        Kind::Text => to_text_message(packet).into_bytes(),
        Kind::Raw | Kind::Channel => packet.to_vec(),
    }
}

/// The payload's length for `text_bytes` of chat text: base64 grows the packet by a third.
pub fn payload_bytes(kind: Kind, text_bytes: usize) -> usize {
    let packet = packet_bytes(text_bytes);
    match kind {
        Kind::Text => (packet * 4).div_ceil(3),
        Kind::Raw | Kind::Channel => packet,
    }
}

/// The time on air of a step's packet with `settings`.
pub fn airtime(settings: &RadioSettings, step: &Step) -> Duration {
    let payload = payload_bytes(step.variant.kind, step.text_bytes);
    time_on_air(
        settings,
        frames::on_air_bytes(step.variant.kind, &step.variant.route, payload),
    )
}

/// How long after a packet's start the next may go, for radio A to stay within `duty_cycle`
/// percent of the time on air.
pub fn spacing(airtime: Duration, duty_cycle: f64) -> Duration {
    airtime.mul_f64(100.0 / duty_cycle)
}

/// How long to wait for a packet to arrive before calling it lost. A text message also waits
/// for the radio's own estimate of its confirmation.
pub fn patience(route: &Route, airtime: Duration, radio_estimate: Option<Duration>) -> Duration {
    let own = if route.is_flood() {
        Duration::from_secs(5) + airtime * 8
    } else {
        Duration::from_secs(3) + airtime * 4
    };
    radio_estimate.map_or(own, |estimate| own.max(estimate + Duration::from_secs(1)))
}

/// What `--plan` shows about a run.
pub struct Outline {
    pub steps: usize,
    pub transmissions: usize,
    pub floods: usize,
    pub airtime: Duration,
    pub duration: Duration,
}

/// Totals for a run, assuming that the radios behave as the firmware source says: refused sends
/// happen once, and lost packets are waited for.
pub fn outline(options: &Options, settings: &RadioSettings) -> Outline {
    let mut outline = Outline {
        steps: 0,
        transmissions: 0,
        floods: 0,
        airtime: Duration::ZERO,
        duration: Duration::ZERO,
    };
    for step in schedule(options) {
        let payload = payload_bytes(step.variant.kind, step.text_bytes);
        let expected = frames::expected(step.variant.kind, &step.variant.route, payload);
        if matches!(expected, Expected::Refused(_)) {
            if step.round == 0 {
                outline.steps += 1;
                outline.duration += Duration::from_millis(500);
            }
            continue;
        }
        let airtime = airtime(settings, &step);
        outline.steps += 1;
        outline.transmissions += 1;
        outline.floods += usize::from(step.variant.route.is_flood());
        outline.airtime += airtime;
        let wait = if expected == Expected::Lost {
            patience(&step.variant.route, airtime, None)
        } else {
            airtime * 2 + Duration::from_millis(500)
        };
        outline.duration += wait.max(spacing(airtime, options.duty_cycle));
    }
    outline
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    fn run(line: &str) -> Options {
        match parse_args(&args(line)) {
            Ok(Command::Run(options)) => options,
            other => panic!("{line}: {other:?}"),
        }
    }

    #[test]
    fn no_arguments_list_the_ports() {
        assert_eq!(parse_args(&[]), Ok(Command::ListPorts));
        assert_eq!(parse_args(&args("--help")), Ok(Command::Help));
    }

    #[test]
    fn two_ports_run_with_the_defaults() {
        let options = run("/dev/cu.usbmodem1101 /dev/cu.usbmodem1201");
        assert_eq!(
            options.ports,
            Some((
                "/dev/cu.usbmodem1101".to_owned(),
                "/dev/cu.usbmodem1201".to_owned()
            ))
        );
        assert_eq!(options.kinds, Kind::ALL);
        assert_eq!(options.texts, DEFAULT_TEXTS);
        assert_eq!((options.repeat, options.flood_repeat), (3, 2));
        assert_eq!(options.duty_cycle, 10.0);
        assert!(options.via.is_none() && !options.offline && !options.radio_off);
    }

    #[test]
    fn options_are_read() {
        let options = run(
            "A B --paths raw,text,raw --texts 10,96 --repeat 5 --flood-repeat 0 --via a3f0 \
             --channel-index 3 --tx-power 2 --duty-cycle 1.5 --offline --radio-off --out x.csv",
        );
        assert_eq!(options.kinds, [Kind::Raw, Kind::Text]);
        assert_eq!(options.texts, [10, 96]);
        assert_eq!((options.repeat, options.flood_repeat), (5, 0));
        assert_eq!(options.via, Some(Route::via("a3f0", 1).unwrap()));
        assert_eq!(options.channel_index, Some(3));
        assert_eq!(options.tx_power, Some(2));
        assert_eq!(options.duty_cycle, 1.5);
        assert!(options.offline && options.radio_off);
        assert_eq!(options.out, Some(PathBuf::from("x.csv")));
        let wide = run("A B --via a3f00b17 --hash-size 2");
        assert_eq!(wide.via.unwrap().path_byte(), 0b0100_0010);
    }

    #[test]
    fn the_plan_needs_no_radios() {
        assert!(matches!(parse_args(&args("--plan")), Ok(Command::Plan(_))));
        assert!(matches!(
            parse_args(&args("--plan A B --repeat 1")),
            Ok(Command::Plan(Options {
                ports: Some(_),
                repeat: 1,
                ..
            }))
        ));
    }

    #[test]
    fn mistakes_are_explained() {
        for (line, says) in [
            ("A", "two serial ports"),
            ("A B C", "two serial ports"),
            ("--repeat 3", "two serial ports"),
            ("A B --paths binary", "not a path"),
            ("A B --texts 10,x", "takes a number"),
            ("A B --texts 300", "longer than any chat text"),
            ("A B --repeat 0", "at least 1"),
            ("A B --repeat", "needs a value"),
            ("A B --duty-cycle 25", "at most 10"),
            ("A B --duty-cycle 0", "above 0"),
            ("A B --via a3f", "not hex"),
            ("A B --via a3f0b1 --hash-size 2", "whole number"),
            ("A B --via a3 --hash-size 4", "1 to 3 bytes"),
            ("A B --fast", "not an option"),
        ] {
            match parse_args(&args(line)) {
                Err(message) => assert!(message.contains(says), "{line}: {message}"),
                other => panic!("{line}: {other:?}"),
            }
        }
    }

    #[test]
    fn rounds_send_every_path_and_floods_stay_small() {
        let options = Options {
            texts: vec![10, 96, 141],
            repeat: 2,
            flood_repeat: 1,
            ..Options::default()
        };
        let labels: Vec<_> = variants(&options)
            .iter()
            .map(|variant| format!("{} {}", variant.kind.name(), variant.route.label()))
            .collect();
        assert_eq!(
            labels,
            [
                "text flood",
                "text zero hop",
                "raw zero hop",
                "raw flood",
                "channel zero hop",
                "channel flood"
            ]
        );
        let steps = schedule(&options);
        let floods = steps.iter().filter(|step| step.variant.route.is_flood());
        assert!(
            floods
                .clone()
                .all(|step| step.round == 0 && step.text_bytes <= 96)
        );
        assert_eq!(floods.count(), 3 * 2);
        assert_eq!(steps.len() - 6, 3 * 3 * 2);
        // The first send teaches radio A the route.
        assert_eq!(steps[0].variant.route, Route::Flood);
        assert_eq!(steps[0].variant.kind, Kind::Text);
    }

    #[test]
    fn through_repeaters_only_direct_sends() {
        let options = Options {
            via: Some(Route::via("a3", 1).unwrap()),
            ..Options::default()
        };
        assert!(
            variants(&options)
                .iter()
                .all(|variant| !variant.route.is_flood())
        );
        assert_eq!(variants(&options).len(), 3);
    }

    #[test]
    fn packets_have_the_apps_sizes() {
        let keys = Keys::new();
        for text_bytes in [0, 10, 96, 149] {
            assert_eq!(chat_text(7, text_bytes).len(), text_bytes);
            let packet = keys.seal(7, text_bytes, 1_795_300_000);
            assert_eq!(packet.len(), packet_bytes(text_bytes));
            assert!(keys.opens(&packet, 7, text_bytes));
            assert!(!keys.opens(&packet, 8, text_bytes));
            for kind in Kind::ALL {
                assert_eq!(
                    payload(kind, &packet).len(),
                    payload_bytes(kind, text_bytes)
                );
            }
        }
        // The longest chat text in format v0 fills a text message.
        assert_eq!(
            payload_bytes(Kind::Text, MAX_RADIO_TEXT_BYTES),
            frames::MAX_TEXT
        );
        assert!(payload_bytes(Kind::Text, MAX_RADIO_TEXT_BYTES + 1) > frames::MAX_TEXT);
        // Each path's largest chat text by the source: 141 bytes for channel data, 148 for raw.
        let zero = Route::zero_hop();
        assert_eq!(
            frames::expected(Kind::Channel, &zero, payload_bytes(Kind::Channel, 141)),
            Expected::Delivered
        );
        assert_ne!(
            frames::expected(Kind::Channel, &zero, payload_bytes(Kind::Channel, 142)),
            Expected::Delivered
        );
        assert_eq!(
            frames::expected(Kind::Raw, &zero, payload_bytes(Kind::Raw, 148)),
            Expected::Delivered
        );
        assert_ne!(
            frames::expected(Kind::Raw, &zero, payload_bytes(Kind::Raw, 149)),
            Expected::Delivered
        );
    }

    #[test]
    fn the_pace_keeps_radio_a_within_its_duty_cycle() {
        let airtime = Duration::from_millis(1623);
        assert_eq!(spacing(airtime, 10.0), Duration::from_millis(16_230));
        assert_eq!(spacing(airtime, 1.0), Duration::from_millis(162_300));
        // Waiting before calling a packet lost: longer for floods, and at least the radio's
        // own estimate for a text message.
        let zero = Route::zero_hop();
        assert_eq!(
            patience(&zero, Duration::from_secs(1), None),
            Duration::from_secs(7)
        );
        assert_eq!(
            patience(&Route::Flood, Duration::from_secs(1), None),
            Duration::from_secs(13)
        );
        assert_eq!(
            patience(&zero, Duration::from_secs(1), Some(Duration::from_secs(10))),
            Duration::from_secs(11)
        );
    }

    #[test]
    fn the_default_run_fits_in_half_an_hour() {
        let outline = outline(&Options::default(), &EU_UK_NARROW);
        assert!(outline.transmissions > 50, "{}", outline.transmissions);
        assert_eq!(outline.floods, 2 * 3 + 2 * 3);
        // At most 10% of the time on air, with the waits for lost packets.
        assert!(outline.duration >= outline.airtime * 10);
        assert!(
            outline.duration < Duration::from_secs(30 * 60),
            "{:?}",
            outline.duration
        );
    }
}

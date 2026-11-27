//! The path spike (#36, #140) as a lab test: it runs under the lock, between the first two USB
//! radios of the lab file after their checks, with every frame logged. The measuring itself is
//! `core/examples/path_spike`.

use std::io::Write;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::lab::{Lab, Station};
use crate::link::{Link, SPIKE_COMMANDS};
use crate::path_spike::link::Link as SpikeLink;
use crate::path_spike::plan::{self, Command, Options};
use crate::path_spike::report::Row;

/// Reads the spike's own options. The harness finds the ports, and refuses what the lab can't or
/// mustn't do.
pub fn options(args: &[String]) -> Result<Options, String> {
    let ports = ["A".to_owned(), "B".to_owned()];
    let all: Vec<String> = ports.into_iter().chain(args.iter().cloned()).collect();
    let options = match plan::parse_args(&all)? {
        Command::Run(options) => options,
        _ => return Err("--path-spike takes the spike's options only, and no ports".to_owned()),
    };
    if options.tx_power.is_some() {
        return Err("the lab never changes a radio's power: leave out --tx-power".to_owned());
    }
    if options.radio_off {
        return Err("the lab can't switch a radio off: use --offline only".to_owned());
    }
    Ok(options)
}

/// Where a radio of the run is.
struct End {
    label: String,
    port: String,
}

impl Lab<'_> {
    /// Opens a spike link to a checked radio, through the harness's own link.
    fn spike_link(&self, name: &'static str, end: &End) -> Result<SpikeLink, String> {
        let stream = self
            .usb
            .open(&end.port)
            .map_err(|error| format!("{}: could not open {}: {error}", end.label, end.port))?;
        let link = Link::open_allowing(
            &end.label,
            stream,
            self.log.clone(),
            self.timing.reply,
            SPIKE_COMMANDS,
        );
        let spike = SpikeLink::over(name, link.commands.clone(), &link.dispatcher);
        Ok(spike.closing(Box::pin(link.close())))
    }

    /// Checks the lab, then measures from the first USB radio to the second, or the other way
    /// round with `reverse`. The CSV goes to `options.out`, or into `folder`.
    pub async fn path_spike(
        &self,
        mut options: Options,
        reverse: bool,
        folder: &Path,
        out: &mut (dyn Write + Send),
    ) -> Result<Vec<Row>, String> {
        let mut ends = Vec::new();
        for station in self.stations(false, out).await? {
            ends.push(end_of(&station));
            station.close().await;
        }
        if ends.len() < 2 {
            return Err("the path spike needs two USB radios in radios.toml".to_owned());
        }
        ends.truncate(2);
        if reverse {
            ends.reverse();
        }
        let _ = writeln!(
            out,
            "Path spike: radio A is {}, radio B is {}.",
            ends[0].label, ends[1].label
        );
        options.out.get_or_insert_with(|| {
            let name = format!(
                "path-spike-{}-{}.csv",
                ends[0].label,
                std::time::UNIX_EPOCH
                    .elapsed()
                    .map_or(0, |since| since.as_secs())
            );
            folder.join(name)
        });
        let a = self.spike_link("A", &ends[0])?;
        let b = self.spike_link("B", &ends[1])?;
        let never = AtomicBool::new(false);
        let rows = crate::path_spike::run_over(a, b, &options, &never, || async {
            self.spike_link("B", &ends[1])
        })
        .await?;
        if rows.is_empty() {
            return Err("the path spike sent nothing".to_owned());
        }
        Ok(rows)
    }
}

fn end_of(station: &Station) -> End {
    End {
        label: station.entry.label.clone(),
        port: station.port.clone(),
    }
}

//! `kwinfo-cli`: inspect what KWinfo's backends see, without the GUI, and capture
//! sensor snapshots for fixtures and bug reports.

use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use kwinfo_core::recording::{RECORDING, Recorder};
use kwinfo_core::snapshot::{self, MANIFEST};
use kwinfo_core::{
    BackendChoice, BackendSet, Sampler, SamplerConfig, Snapshot, SysRoot, TemperatureUnit,
    format_value,
};
use tracing_subscriber::EnvFilter;

/// Time between the two readings `list` takes: rates and usage need two.
const LIST_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Parser, Debug)]
#[command(
    name = "kwinfo-cli",
    version,
    about = "Command line companion to KWinfo"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List every device and channel with its current value
    List {
        /// Use the synthetic mock backend instead of real hardware
        #[arg(long, conflicts_with = "replay")]
        mock: bool,
        /// Run the real backends against a captured tree instead of / (the output of
        /// `kwinfo-cli snapshot`, or a test fixture)
        #[arg(long, value_name = "DIR")]
        replay: Option<PathBuf>,
        /// Backend selection through the environment (`KWINFO_BACKEND=mock`)
        #[arg(long, env = "KWINFO_BACKEND", hide = true)]
        backend: Option<String>,
        /// Show temperatures in Fahrenheit
        #[arg(long)]
        fahrenheit: bool,
        /// Show the stable id of every device and channel
        #[arg(long)]
        ids: bool,
    },
    /// Copy the sensor-relevant parts of /sys and /proc into a directory, with serial
    /// numbers, WWNs and MAC addresses scrubbed, for test fixtures and bug reports
    Snapshot {
        /// Directory to write; must not exist yet or be empty
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
        /// Capture from this tree instead of / (e.g. to re-scrub an older capture)
        #[arg(long, value_name = "DIR", default_value = "/")]
        root: PathBuf,
    },
    /// Take a snapshot and then record every value that changes, frame by frame, so that
    /// `--replay` shows usage, rates and temperatures moving as they did
    Record {
        /// Directory to write; must not exist yet or be empty
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
        /// How long to record: 90, 90s, 2m or 500ms
        #[arg(long, default_value = "60s", value_parser = parse_duration)]
        duration: Duration,
        /// Time between frames in milliseconds
        #[arg(
            long,
            default_value_t = 1000,
            value_name = "MS",
            value_parser = clap::value_parser!(u64).range(50..=60_000)
        )]
        interval: u64,
        /// Record from this tree instead of /
        #[arg(long, value_name = "DIR", default_value = "/")]
        root: PathBuf,
    },
}

/// `90` and `90s` are seconds; `2m` minutes; `500ms` milliseconds.
fn parse_duration(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let n: u64 = number
        .parse()
        .map_err(|_| format!("`{text}` is not a duration such as 90s, 2m or 500ms"))?;
    let duration = match unit.trim() {
        "" | "s" => Duration::from_secs(n),
        "m" | "min" => Duration::from_secs(n.saturating_mul(60)),
        "ms" => Duration::from_millis(n),
        other => return Err(format!("unknown unit `{other}`; use s, m or ms")),
    };
    Ok(duration)
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("KWINFO_LOG").unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    match args.command {
        Command::List {
            mock,
            replay,
            backend,
            fahrenheit,
            ids,
        } => {
            let choice = match replay {
                Some(dir) => BackendChoice::Replay(dir),
                None if mock || backend.as_deref() == Some("mock") => BackendChoice::Mock,
                None => BackendChoice::Auto,
            };
            let temperature = if fahrenheit {
                TemperatureUnit::Fahrenheit
            } else {
                TemperatureUnit::Celsius
            };
            list(choice, temperature, ids)
        }
        Command::Snapshot { out, root } => take_snapshot(&root, &out),
        Command::Record {
            out,
            duration,
            interval,
            root,
        } => record(&root, &out, duration, Duration::from_millis(interval)),
    }
}

/// Runs the backends through the real sampler for two ticks, so that devices several
/// backends contribute to are merged and ordered exactly as in the GUI.
fn sample_twice(choice: BackendChoice) -> anyhow::Result<std::sync::Arc<Snapshot>> {
    let set = BackendSet::build(choice);
    for note in &set.notes {
        eprintln!("note: {note}");
    }
    let sampler = Sampler::start(
        set.backends,
        SamplerConfig {
            interval: LIST_INTERVAL,
            history_capacity: 1,
        },
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let snapshot = sampler.snapshot();
        if snapshot.tick >= 2 {
            return Ok(snapshot);
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "timed out waiting for sensor readings"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

struct Line {
    sensor: String,
    value: String,
    kind: &'static str,
    id: String,
}

fn list(choice: BackendChoice, temperature: TemperatureUnit, ids: bool) -> anyhow::Result<()> {
    let snapshot = sample_twice(choice)?;
    if snapshot.devices.is_empty() {
        println!("No devices found.");
        return Ok(());
    }

    let groups: Vec<(String, Vec<Line>)> = snapshot
        .devices
        .iter()
        .map(|device| {
            let mut header = device.device.label.clone();
            if ids {
                header = format!("{header}  [{}]", device.device.id);
            }
            let lines = device
                .channels
                .iter()
                .map(|entry| {
                    let c = &entry.channel;
                    Line {
                        sensor: c.label.clone(),
                        value: format_value(entry.stats.current(), c.unit, c.decimals, temperature),
                        kind: c.kind.label(),
                        id: c.id.key().to_string(),
                    }
                })
                .collect();
            (header, lines)
        })
        .collect();

    let lines = || groups.iter().flat_map(|(_, lines)| lines.iter());
    let sensor_width = lines()
        .map(|l| l.sensor.chars().count())
        .max()
        .unwrap_or(0)
        .max(6);
    let value_width = lines()
        .map(|l| l.value.chars().count())
        .max()
        .unwrap_or(0)
        .max(7);
    let kind_width = lines().map(|l| l.kind.len()).max().unwrap_or(0);
    for (device, lines) in &groups {
        println!("{device}");
        for line in lines {
            if ids {
                println!(
                    "  {:<sensor_width$}  {:>value_width$}  {:<kind_width$}  {}",
                    line.sensor, line.value, line.kind, line.id
                );
            } else {
                println!(
                    "  {:<sensor_width$}  {:>value_width$}  {}",
                    line.sensor, line.value, line.kind
                );
            }
        }
        println!();
    }
    println!(
        "{} devices, {} channels",
        groups.len(),
        snapshot.channel_count()
    );
    Ok(())
}

fn take_snapshot(root: &Path, out: &Path) -> anyhow::Result<()> {
    let source = SysRoot::new(root);
    let report = snapshot::capture(&source, out)?;
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    println!(
        "Wrote {} files and {} symlinks to {}.",
        report.files,
        report.links,
        out.display()
    );
    println!(
        "Scrubbed {} files holding serial numbers, WWNs or MAC addresses.",
        report.scrubbed.len()
    );
    if !report.unreadable.is_empty() {
        println!(
            "{} files could not be read and were left out.",
            report.unreadable.len()
        );
    }
    println!("Details: {}", out.join(MANIFEST).display());
    println!("Check it: kwinfo-cli list --replay {}", out.display());
    Ok(())
}

fn record(root: &Path, out: &Path, duration: Duration, interval: Duration) -> anyhow::Result<()> {
    let frames = usize::try_from(duration.as_millis() / interval.as_millis().max(1))
        .unwrap_or(usize::MAX)
        .max(1);
    let mut recorder = Recorder::start(&SysRoot::new(root), out, interval)?;
    eprintln!(
        "Recording {frames} more frames, one every {} ms; Ctrl-C stops early and keeps what \
         was recorded.",
        interval.as_millis()
    );
    let started = recorder.started();
    let progress = std::io::stderr().is_terminal();
    for frame in 1..=frames {
        let due = started + interval * u32::try_from(frame).unwrap_or(u32::MAX);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
        recorder.capture_frame(started.elapsed())?;
        if progress {
            eprint!("\r  frame {frame}/{frames}");
        }
    }
    if progress {
        eprintln!();
    }
    let report = recorder.finish()?;
    for warning in &report.snapshot.warnings {
        eprintln!("warning: {warning}");
    }
    println!(
        "Recorded {} frames of {} values ({} changes, {} KiB) to {}.",
        report.frames,
        report.tracked,
        report.changes,
        report.bytes.div_ceil(1024),
        out.join(RECORDING).display()
    );
    println!(
        "The snapshot under it has {} files; {} were scrubbed. Details: {}",
        report.snapshot.files,
        report.snapshot.scrubbed.len(),
        out.join(MANIFEST).display()
    );
    println!("Replay it: kwinfo --replay {}", out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_take_seconds_minutes_and_milliseconds() {
        assert_eq!(parse_duration("90"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("120s"), Ok(Duration::from_secs(120)));
        assert_eq!(parse_duration("2m"), Ok(Duration::from_secs(120)));
        assert_eq!(parse_duration("500ms"), Ok(Duration::from_millis(500)));
        assert!(parse_duration("2h").is_err());
        assert!(parse_duration("soon").is_err());
    }
}

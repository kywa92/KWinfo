//! `kwinfo-cli`: inspect what KWinfo's backends see, without the GUI.

use std::collections::HashMap;
use std::thread;
use std::time::Duration;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use kwinfo_core::{BackendChoice, BackendSet, ChannelId, TemperatureUnit, format_value};
use tracing_subscriber::EnvFilter;

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
        #[arg(long)]
        mock: bool,
        /// Backend selection through the environment (`KWINFO_BACKEND=mock`)
        #[arg(long, env = "KWINFO_BACKEND", hide = true)]
        backend: Option<String>,
        /// Show temperatures in Fahrenheit
        #[arg(long)]
        fahrenheit: bool,
    },
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
            backend,
            fahrenheit,
        } => {
            let choice = if mock || backend.as_deref() == Some("mock") {
                BackendChoice::Mock
            } else {
                BackendChoice::Auto
            };
            let temperature = if fahrenheit {
                TemperatureUnit::Fahrenheit
            } else {
                TemperatureUnit::Celsius
            };
            list(choice, temperature)
        }
    }
}

struct Line {
    sensor: String,
    value: String,
    kind: &'static str,
}

fn list(choice: BackendChoice, temperature: TemperatureUnit) -> anyhow::Result<()> {
    let set = BackendSet::build(choice);
    for note in &set.notes {
        eprintln!("note: {note}");
    }

    let mut groups: Vec<(String, Vec<Line>)> = Vec::new();
    for mut backend in set.backends {
        let name = backend.name();
        let devices = backend
            .enumerate()
            .with_context(|| format!("backend {name} failed to enumerate"))?;
        // Sample twice: rate-type channels need two readings before they have a value.
        let mut samples = Vec::new();
        backend
            .sample(&mut samples)
            .with_context(|| format!("backend {name} failed to sample"))?;
        thread::sleep(Duration::from_millis(250));
        samples.clear();
        backend
            .sample(&mut samples)
            .with_context(|| format!("backend {name} failed to sample"))?;
        let values: HashMap<ChannelId, Option<f64>> =
            samples.into_iter().map(|s| (s.channel, s.value)).collect();

        for (device, channels) in devices {
            let lines = channels
                .iter()
                .map(|c| Line {
                    sensor: c.label.clone(),
                    value: format_value(
                        values.get(&c.id).copied().flatten(),
                        c.unit,
                        c.decimals,
                        temperature,
                    ),
                    kind: c.kind.label(),
                })
                .collect();
            groups.push((device.label, lines));
        }
    }

    if groups.is_empty() {
        println!("No devices found.");
        return Ok(());
    }

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
    for (device, lines) in &groups {
        println!("{device}");
        for line in lines {
            println!(
                "  {:<sensor_width$}  {:>value_width$}  {}",
                line.sensor, line.value, line.kind
            );
        }
        println!();
    }
    let channel_count: usize = groups.iter().map(|(_, lines)| lines.len()).sum();
    println!("{} devices, {channel_count} channels", groups.len());
    Ok(())
}

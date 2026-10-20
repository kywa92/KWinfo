//! The `kwinfo-cli` command line. It is kept apart from `main.rs` so that `cargo xtask man`
//! can render it into the kwinfo-cli(1) manual page; it may use only `clap` and `std`.

use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "kwinfo-cli",
    version,
    about = "Command line companion to KWinfo"
)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
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
    /// Show what the privileged helper reads: through the system service, or with
    /// --session from a helper started with pkexec for this command
    Helper {
        /// Start the helper with pkexec (asks for an administrator password) instead of
        /// using the system service
        #[arg(long)]
        session: bool,
        /// Show temperatures in Fahrenheit
        #[arg(long)]
        fahrenheit: bool,
    },
    /// Bundle the newest diagnostic logs, a machine summary, the configuration and the
    /// sensor inventory into one text file to attach to a bug report
    Report {
        /// File to write (default: a timestamped file next to the logs)
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// Take the inventory from the mock backend
        #[arg(long, conflicts_with = "replay")]
        mock: bool,
        /// Take the inventory from a captured tree
        #[arg(long, value_name = "DIR")]
        replay: Option<PathBuf>,
        /// Leave the sensor inventory out (no hardware access, faster)
        #[arg(long)]
        no_inventory: bool,
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
pub fn parse_duration(text: &str) -> Result<Duration, String> {
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

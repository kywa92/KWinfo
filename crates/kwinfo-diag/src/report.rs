//! Bundles the newest logs, a machine summary, the configuration and a sensor inventory
//! into one text file to attach to a bug report.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use kwinfo_core::{
    BackendChoice, BackendSet, Sampler, SamplerConfig, TemperatureUnit, format_value,
};

use crate::civil::Utc;
use crate::{logfile, sysinfo};

/// Where the sensor inventory section comes from.
pub enum Inventory {
    /// Leave the section out.
    None,
    /// Run the backends briefly and list every device and channel.
    Collect(BackendChoice),
    /// Text the caller already has, e.g. the GUI's current snapshot.
    Text(String),
}

pub struct ReportOptions {
    /// Output file; default: `kwinfo-report-<host>-<utc>.txt` in the log directory.
    pub out: Option<PathBuf>,
    /// Log directory to read; default: the same one [`crate::init`] writes to.
    pub log_dir: Option<PathBuf>,
    pub inventory: Inventory,
    /// Config file to include; default: `~/.config/kwinfo/config.toml` if present.
    pub config_path: Option<PathBuf>,
    /// How many of the newest log files to include, and how much of each (from the end).
    pub max_log_files: usize,
    pub max_bytes_per_log: u64,
}

impl Default for ReportOptions {
    fn default() -> Self {
        Self {
            out: None,
            log_dir: None,
            inventory: Inventory::None,
            config_path: None,
            max_log_files: 3,
            max_bytes_per_log: 2 * 1024 * 1024,
        }
    }
}

/// Writes the report and returns its path.
pub fn write(options: ReportOptions) -> anyhow::Result<PathBuf> {
    let now = Utc::now();
    let system = sysinfo::collect();
    let hostname = system
        .iter()
        .find(|(k, _)| k == "hostname")
        .map(|(_, v)| v.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "_"))
        .unwrap_or_else(|| "host".into());
    let log_dir = logfile::log_dir(options.log_dir.as_deref());
    let out = options
        .out
        .unwrap_or_else(|| log_dir.join(format!("kwinfo-report-{hostname}-{}.txt", now.compact())));

    let mut text = String::new();
    let _ = writeln!(text, "# KWinfo diagnostic report");
    let _ = writeln!(text, "generated: {} (UTC)", now.iso());
    let _ = writeln!(text, "kwinfo_version: {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(
        text,
        "profile: {}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    let _ = writeln!(text);

    let _ = writeln!(text, "## System");
    text.push_str(&sysinfo::render(&system));
    let _ = writeln!(text);

    let _ = writeln!(text, "## Configuration");
    match options.config_path.or_else(default_config_path) {
        Some(path) if path.exists() => {
            let _ = writeln!(text, "file: {}", path.display());
            match std::fs::read_to_string(&path) {
                Ok(config) => text.push_str(&config),
                Err(err) => {
                    let _ = writeln!(text, "(unreadable: {err})");
                }
            }
        }
        Some(path) => {
            let _ = writeln!(text, "(no file at {})", path.display());
        }
        None => {
            let _ = writeln!(text, "(no configuration directory on this platform)");
        }
    }
    let _ = writeln!(text);

    match options.inventory {
        Inventory::None => {}
        Inventory::Text(inventory) => {
            let _ = writeln!(text, "## Sensor inventory");
            text.push_str(&inventory);
            let _ = writeln!(text);
        }
        Inventory::Collect(choice) => {
            let _ = writeln!(text, "## Sensor inventory");
            match inventory(choice) {
                Ok(inventory) => text.push_str(&inventory),
                Err(err) => {
                    let _ = writeln!(text, "(failed: {err:#})");
                }
            }
            let _ = writeln!(text);
        }
    }

    let logs = logfile::list(&log_dir);
    let _ = writeln!(text, "## Log files in {}", log_dir.display());
    if logs.is_empty() {
        let _ = writeln!(text, "(none)");
    }
    for (path, _, size) in &logs {
        let _ = writeln!(
            text,
            "{} ({size} bytes)",
            path.file_name().unwrap_or_default().to_string_lossy()
        );
    }
    let _ = writeln!(text);

    for (path, _, size) in logs.iter().take(options.max_log_files) {
        let _ = writeln!(
            text,
            "## Log: {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        );
        match std::fs::read_to_string(path) {
            Ok(content) => {
                if *size > options.max_bytes_per_log {
                    let keep = options.max_bytes_per_log as usize;
                    let mut start = content.len().saturating_sub(keep);
                    while !content.is_char_boundary(start) {
                        start += 1;
                    }
                    let _ = writeln!(text, "(first {} bytes omitted)", start);
                    text.push_str(&content[start..]);
                } else {
                    text.push_str(&content);
                }
            }
            Err(err) => {
                let _ = writeln!(text, "(unreadable: {err})");
            }
        }
        if !text.ends_with('\n') {
            text.push('\n');
        }
        let _ = writeln!(text);
    }

    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&out, text).with_context(|| format!("writing {}", out.display()))?;
    tracing::info!(target: "kwinfo::session", path = %out.display(), "diagnostic report written");
    Ok(out)
}

/// `~/.config/kwinfo/config.toml` on Linux, the platform equivalent elsewhere.
pub fn default_config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("io.github", "kywa92", "kwinfo")
        .map(|dirs| dirs.config_dir().join("config.toml"))
}

/// Runs the backends for two ticks and lists every device and channel with its current
/// value and stable id.
pub fn inventory(choice: BackendChoice) -> anyhow::Result<String> {
    let set = BackendSet::build(choice);
    let mut text = String::new();
    for note in &set.notes {
        let _ = writeln!(text, "note: {note}");
    }
    let sampler = Sampler::start(
        set.backends,
        SamplerConfig {
            interval: Duration::from_millis(500),
            history_capacity: 1,
        },
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    let snapshot = loop {
        let snapshot = sampler.snapshot();
        if snapshot.tick >= 2 {
            break snapshot;
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "timed out waiting for sensor readings"
        );
        thread::sleep(Duration::from_millis(10));
    };
    for device in &snapshot.devices {
        let _ = writeln!(text, "{}  [{}]", device.device.label, device.device.id);
        for entry in &device.channels {
            let c = &entry.channel;
            let _ = writeln!(
                text,
                "  {:<34} {:>16}  {:<12} {}",
                c.label,
                format_value(
                    entry.stats.current(),
                    c.unit,
                    c.decimals,
                    TemperatureUnit::Celsius
                ),
                c.kind.label(),
                c.id.key()
            );
        }
    }
    let _ = writeln!(
        text,
        "{} devices, {} channels",
        snapshot.devices.len(),
        snapshot.channel_count()
    );
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_bundles_system_logs_and_inventory() {
        let dir = std::env::temp_dir().join(format!("kwinfo-diag-report-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("kwinfo-20260101-000000-1.log"),
            "# fake log\nsession start\n",
        )
        .unwrap();
        let config = dir.join("config.toml");
        std::fs::write(&config, "[general]\npolling_ms = 1000\n").unwrap();

        let path = write(ReportOptions {
            log_dir: Some(dir.clone()),
            config_path: Some(config),
            inventory: Inventory::Collect(BackendChoice::Mock),
            ..ReportOptions::default()
        })
        .unwrap();
        assert!(path.starts_with(&dir));
        let text = std::fs::read_to_string(&path).unwrap();
        for needle in [
            "# KWinfo diagnostic report",
            "## System",
            "os: ",
            "## Configuration",
            "polling_ms = 1000",
            "## Sensor inventory",
            "CPU [#0]: Mock Hexa-Core 6000X",
            "## Log files in",
            "## Log: kwinfo-20260101-000000-1.log",
            "# fake log",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn long_logs_are_tailed() {
        let dir = std::env::temp_dir().join(format!("kwinfo-diag-tail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let body: String = (0..200).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.join("kwinfo-20260101-000000-1.log"), &body).unwrap();
        let path = write(ReportOptions {
            log_dir: Some(dir.clone()),
            config_path: Some(dir.join("missing.toml")),
            max_bytes_per_log: 100,
            ..ReportOptions::default()
        })
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("bytes omitted"));
        assert!(text.contains("line 199"));
        assert!(!text.contains("line 0\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

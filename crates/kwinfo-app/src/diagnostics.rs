//! "Save diagnostic report…" in Settings → General: a `kwinfo_diag` report whose sensor
//! inventory is what the window shows right now, written on a background thread.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use kwinfo_core::{Config, Snapshot, TemperatureUnit, format_value};
use kwinfo_diag::report::{Inventory, ReportOptions};

/// The sensor inventory section of a report: every device with its id, then one line per
/// channel with its name, current value, kind and key, in the layout `kwinfo-cli report`
/// uses. Renamed, hidden and disabled sensors are marked, since they explain what the
/// user saw. Values are in °C whatever the display setting, like the CLI's.
/// The privileged helper's state for the report: one line, and the helper's notes on
/// sources it could not open.
pub fn helper_text(state: &kwinfo_ipc::client::HelperState) -> String {
    let mut text = format!("helper: {}\n", state.describe());
    if let kwinfo_ipc::client::HelperState::Running { capabilities, .. } = state {
        for note in &capabilities.notes {
            let _ = writeln!(text, "helper note: {note}");
        }
    }
    text
}

pub fn inventory_text(snapshot: &Snapshot, config: &Config) -> String {
    let mut text = String::new();
    for device in &snapshot.devices {
        let id = &device.device.id;
        let _ = write!(text, "{}  [{id}]", device.device.label);
        if config.is_device_disabled(id) {
            text.push_str("  (monitoring disabled)");
        }
        text.push('\n');
        for entry in &device.channels {
            let c = &entry.channel;
            let label = config.label_of(c);
            let value = format_value(
                entry.stats.current(),
                c.unit,
                c.decimals,
                TemperatureUnit::Celsius,
            );
            let _ = write!(
                text,
                "  {label:<34} {value:>16}  {:<12} {}",
                c.kind.label(),
                c.id.key()
            );
            let mut notes = Vec::new();
            if label != c.label {
                notes.push(format!("renamed from {:?}", c.label));
            }
            if config.is_hidden(&c.id) {
                notes.push("hidden".to_string());
            }
            if entry.disabled || config.is_disabled(c) {
                notes.push("disabled".to_string());
            }
            if !notes.is_empty() {
                let _ = write!(text, "  ({})", notes.join(", "));
            }
            text.push('\n');
        }
    }
    let _ = writeln!(
        text,
        "{} devices, {} channels",
        snapshot.devices.len(),
        snapshot.channel_count()
    );
    text
}

/// A report being written.
pub struct ReportJob {
    rx: Receiver<Result<PathBuf, String>>,
}

impl ReportJob {
    /// Starts writing a report with `inventory` as its sensor section. `out` overrides
    /// the file (default: next to the log files); `config_path` is the settings file in
    /// use. `ctx` is asked to repaint when the report is done.
    pub fn start(
        inventory: String,
        out: Option<PathBuf>,
        config_path: Option<PathBuf>,
        ctx: egui::Context,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let options = ReportOptions {
            out,
            config_path,
            inventory: Inventory::Text(inventory),
            ..ReportOptions::default()
        };
        let worker_tx = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("kwinfo-report".into())
            .spawn(move || {
                let result = kwinfo_diag::report::write(options).map_err(|err| format!("{err:#}"));
                let _ = worker_tx.send(result);
                ctx.request_repaint();
            });
        if let Err(err) = spawned {
            let _ = tx.send(Err(format!("could not start writing the report: {err}")));
        }
        Self { rx }
    }

    /// The outcome, once the report is written (or failed).
    pub fn poll(&self) -> Option<Result<PathBuf, String>> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err("the report writer stopped unexpectedly".to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use kwinfo_core::{
        Category, Channel, ChannelId, ChannelSnapshot, Device, DeviceSnapshot, Kind, Stats,
    };

    use super::*;

    fn snapshot() -> Snapshot {
        let device = Device::new("cpu:0", "CPU [#0]: Test", Category::Cpu);
        let channels = ["package", "core0", "core1"].map(|key| {
            let mut stats = Stats::new();
            stats.update(Some(42.25));
            ChannelSnapshot {
                channel: Arc::new(Channel::new(
                    &device.id,
                    key,
                    format!("Temp {key}"),
                    Kind::Temperature,
                )),
                stats,
                disabled: false,
            }
        });
        let now = Instant::now();
        Snapshot {
            tick: 2,
            started_at: now,
            taken_at: now,
            stats_since: now,
            interval: Duration::from_secs(1),
            tick_cost: Duration::ZERO,
            devices: vec![DeviceSnapshot {
                device: Arc::new(device),
                channels: channels.into(),
            }],
        }
    }

    #[test]
    fn the_inventory_lists_ids_values_and_what_the_user_changed() {
        let mut config = Config::default();
        config.general.temperature_unit = TemperatureUnit::Fahrenheit;
        let id = |key: &str| ChannelId::from_raw(format!("cpu:0/{key}"));
        config.update_channel(&id("package"), |c| c.label = Some("Die".into()));
        config.update_channel(&id("core0"), |c| {
            c.hidden = true;
            c.disabled = true;
        });
        let text = inventory_text(&snapshot(), &config);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "CPU [#0]: Test  [cpu:0]");
        assert!(lines[1].starts_with("  Die "), "{}", lines[1]);
        assert!(
            lines[1].contains("42.2 °C"),
            "Celsius whatever the setting: {}",
            lines[1]
        );
        assert!(lines[1].contains("Temperature"), "{}", lines[1]);
        assert!(
            lines[1].ends_with("package  (renamed from \"Temp package\")"),
            "{}",
            lines[1]
        );
        assert!(
            lines[2].ends_with("core0  (hidden, disabled)"),
            "{}",
            lines[2]
        );
        assert!(lines[3].ends_with("core1"), "{}", lines[3]);
        assert_eq!(lines[4], "1 devices, 3 channels");
    }

    #[test]
    fn reports_are_written_in_the_background() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("report.txt");
        let config = dir.path().join("config.toml");
        std::fs::write(&config, "[polling]\ninterval_ms = 500\n").unwrap();
        let job = ReportJob::start(
            "CPU  [cpu:0]\n".into(),
            Some(out.clone()),
            Some(config),
            egui::Context::default(),
        );
        let deadline = Instant::now() + Duration::from_secs(20);
        let result = loop {
            if let Some(result) = job.poll() {
                break result;
            }
            assert!(Instant::now() < deadline, "the report never finished");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(result, Ok(out.clone()));
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("## Sensor inventory\nCPU  [cpu:0]"), "{text}");
        assert!(
            text.contains("interval_ms = 500"),
            "the settings file in use"
        );
    }
}

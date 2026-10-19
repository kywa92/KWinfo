//! S.M.A.R.T. data through `smartctl` (smartmontools 7.0 or later, JSON output).
//!
//! `smartctl --scan -j` lists the drives; every minute each one is queried with
//! `smartctl -j -i -H -A -n standby -d <type> <device>`, which leaves sleeping disks
//! asleep (they are skipped until they wake up). The rows follow HWiNFO's: temperature,
//! remaining life, failure and warning flags, power-on hours, host reads and writes,
//! and the error counters that matter (reallocated, pending and uncorrectable sectors on
//! ATA; media errors, spare and unsafe shutdowns on NVMe).
//!
//! Serial numbers are read to tell drives apart but never leave the helper: device ids
//! carry a hash of them, as the unprivileged `block` backend does.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use kwinfo_core::backends::shared::{short_hash, slug};
use kwinfo_core::{Category, Channel, ChannelId, Device, Kind, Sample, Source};
use serde_json::Value;

use super::{Provider, Status};

/// How often the drives are queried.
pub const REFRESH: Duration = Duration::from_secs(60);

/// Where `smartctl` is looked for. Fixed paths rather than `PATH`, which a privileged
/// process should not trust.
const SMARTCTL: [&str; 4] = [
    "/usr/sbin/smartctl",
    "/usr/bin/smartctl",
    "/usr/local/sbin/smartctl",
    "/sbin/smartctl",
];

/// Runs (or replays) `smartctl`.
pub trait SmartTool: Send {
    /// `smartctl --scan -j`.
    fn scan(&mut self) -> io::Result<Vec<u8>>;
    /// `smartctl -j -i -H -A -n standby -d <kind> <name>`.
    fn query(&mut self, name: &str, kind: &str) -> io::Result<Vec<u8>>;
    /// Whether there is a `smartctl` at all.
    fn present(&self) -> bool {
        true
    }
}

/// The real `smartctl`.
pub struct Smartctl {
    path: PathBuf,
}

impl Smartctl {
    pub fn find() -> Option<Self> {
        SMARTCTL
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
            .map(|path| Self { path })
    }

    fn run(&self, args: &[&str]) -> io::Result<Vec<u8>> {
        // smartctl reports problems through its exit status bits while still printing
        // JSON, so the status is not an error here; the JSON says what happened.
        Ok(Command::new(&self.path).args(args).output()?.stdout)
    }
}

impl SmartTool for Smartctl {
    fn scan(&mut self) -> io::Result<Vec<u8>> {
        self.run(&["--scan", "-j"])
    }

    fn query(&mut self, name: &str, kind: &str) -> io::Result<Vec<u8>> {
        self.run(&["-j", "-i", "-H", "-A", "-n", "standby", "-d", kind, name])
    }
}

/// No `smartctl` installed.
pub struct NoSmartctl;

impl SmartTool for NoSmartctl {
    fn scan(&mut self) -> io::Result<Vec<u8>> {
        Err(io::Error::from(io::ErrorKind::NotFound))
    }

    fn query(&mut self, _: &str, _: &str) -> io::Result<Vec<u8>> {
        Err(io::Error::from(io::ErrorKind::NotFound))
    }

    fn present(&self) -> bool {
        false
    }
}

/// Captured output: `scan.json`, and `<device basename>.json` per drive (`nvme0.json`
/// for `/dev/nvme0`). For tests and `kwinfo-helper stdio --smartctl-dir`.
pub struct SmartctlDir {
    dir: PathBuf,
}

impl SmartctlDir {
    pub fn new(dir: impl AsRef<Path>) -> Self {
        Self {
            dir: dir.as_ref().to_path_buf(),
        }
    }
}

impl SmartTool for SmartctlDir {
    fn scan(&mut self) -> io::Result<Vec<u8>> {
        std::fs::read(self.dir.join("scan.json"))
    }

    fn query(&mut self, name: &str, _: &str) -> io::Result<Vec<u8>> {
        let base = Path::new(name).file_name().unwrap_or_default();
        std::fs::read(self.dir.join(base).with_extension("json"))
    }
}

/// A drive in `smartctl --scan` output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanEntry {
    /// `/dev/sda`, `/dev/nvme0`.
    pub name: String,
    /// smartctl's `-d` type: `sat`, `nvme`, `scsi`, ...
    pub kind: String,
}

pub fn parse_scan(json: &[u8]) -> Vec<ScanEntry> {
    let Ok(value) = serde_json::from_slice::<Value>(json) else {
        return Vec::new();
    };
    value["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            Some(ScanEntry {
                name: d["name"].as_str()?.to_string(),
                kind: d["type"].as_str().unwrap_or("auto").to_string(),
            })
        })
        .collect()
}

/// The rows a drive can have, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reading {
    Temperature,
    RemainingLife,
    AvailableSpare,
    Failure,
    Warning,
    PowerOnHours,
    PowerCycles,
    UnsafeShutdowns,
    HostReads,
    HostWrites,
    Reallocated,
    Pending,
    Uncorrectable,
    MediaErrors,
}

impl Reading {
    pub fn key(self) -> &'static str {
        match self {
            Reading::Temperature => "smart.temperature",
            Reading::RemainingLife => "smart.life",
            Reading::AvailableSpare => "smart.spare",
            Reading::Failure => "smart.failure",
            Reading::Warning => "smart.warning",
            Reading::PowerOnHours => "smart.power_on_hours",
            Reading::PowerCycles => "smart.power_cycles",
            Reading::UnsafeShutdowns => "smart.unsafe_shutdowns",
            Reading::HostReads => "smart.host_reads",
            Reading::HostWrites => "smart.host_writes",
            Reading::Reallocated => "smart.reallocated",
            Reading::Pending => "smart.pending",
            Reading::Uncorrectable => "smart.uncorrectable",
            Reading::MediaErrors => "smart.media_errors",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Reading::Temperature => "Drive Temperature",
            Reading::RemainingLife => "Drive Remaining Life",
            Reading::AvailableSpare => "Available Spare",
            Reading::Failure => "Drive Failure",
            Reading::Warning => "Drive Warning",
            Reading::PowerOnHours => "Power-On Hours",
            Reading::PowerCycles => "Power Cycles",
            Reading::UnsafeShutdowns => "Unsafe Shutdowns",
            Reading::HostReads => "Total Host Reads",
            Reading::HostWrites => "Total Host Writes",
            Reading::Reallocated => "Reallocated Sectors",
            Reading::Pending => "Pending Sectors",
            Reading::Uncorrectable => "Uncorrectable Sectors",
            Reading::MediaErrors => "Media Errors",
        }
    }

    pub fn kind(self) -> Kind {
        match self {
            Reading::Temperature => Kind::Temperature,
            Reading::RemainingLife | Reading::AvailableSpare => Kind::Usage,
            Reading::Failure | Reading::Warning => Kind::Boolean,
            Reading::HostReads | Reading::HostWrites => Kind::Capacity,
            _ => Kind::Count,
        }
    }
}

/// What one query of a drive said.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub model: String,
    pub serial: Option<String>,
    pub readings: Vec<(Reading, f64)>,
}

fn put(readings: &mut Vec<(Reading, f64)>, reading: Reading, value: Option<f64>) {
    if let Some(value) = value.filter(|v| v.is_finite()) {
        readings.push((reading, value));
    }
}

/// Parses a drive query. `None` when the drive is asleep or could not be opened.
pub fn parse_report(json: &[u8]) -> Option<Report> {
    let v: Value = serde_json::from_slice(json).ok()?;
    let model = v["model_name"]
        .as_str()
        .or_else(|| v["scsi_model_name"].as_str())?
        .trim()
        .to_string();
    let serial = v["serial_number"].as_str().map(str::to_string);
    let mut readings = Vec::new();
    let number = |v: &Value| v.as_f64();

    put(
        &mut readings,
        Reading::Temperature,
        number(&v["temperature"]["current"]),
    );
    put(
        &mut readings,
        Reading::PowerOnHours,
        number(&v["power_on_time"]["hours"]),
    );
    put(
        &mut readings,
        Reading::PowerCycles,
        number(&v["power_cycle_count"]),
    );
    let passed = v["smart_status"]["passed"].as_bool();
    put(
        &mut readings,
        Reading::Failure,
        passed.map(|p| if p { 0.0 } else { 1.0 }),
    );

    let nvme = &v["nvme_smart_health_information_log"];
    let warning = if nvme.is_object() {
        // NVMe data units are 1000 blocks of 512 bytes.
        let units = |field: &str| number(&nvme[field]).map(|u| u * 512_000.0);
        put(
            &mut readings,
            Reading::RemainingLife,
            number(&nvme["percentage_used"]).map(|used| (100.0 - used).max(0.0)),
        );
        put(
            &mut readings,
            Reading::AvailableSpare,
            number(&nvme["available_spare"]),
        );
        put(
            &mut readings,
            Reading::UnsafeShutdowns,
            number(&nvme["unsafe_shutdowns"]),
        );
        put(&mut readings, Reading::HostReads, units("data_units_read"));
        put(
            &mut readings,
            Reading::HostWrites,
            units("data_units_written"),
        );
        put(
            &mut readings,
            Reading::MediaErrors,
            number(&nvme["media_errors"]),
        );
        let critical = number(&nvme["critical_warning"]).unwrap_or(0.0) != 0.0;
        let low_spare = match (
            number(&nvme["available_spare"]),
            number(&nvme["available_spare_threshold"]),
        ) {
            (Some(spare), Some(threshold)) => spare < threshold,
            _ => false,
        };
        critical || low_spare
    } else {
        let table: Vec<&Value> = v["ata_smart_attributes"]["table"]
            .as_array()
            .into_iter()
            .flatten()
            .collect();
        let attribute = |id: u64| table.iter().find(|a| a["id"].as_u64() == Some(id));
        let raw = |id: u64| attribute(id).and_then(|a| number(&a["raw"]["value"]));
        let block = number(&v["logical_block_size"]).unwrap_or(512.0);
        // Life left, as the normalised value of the first attribute a vendor uses for it.
        let life = [231, 233, 177, 202, 169]
            .into_iter()
            .find_map(|id| attribute(id).and_then(|a| number(&a["value"])));
        put(
            &mut readings,
            Reading::RemainingLife,
            life.map(|l| l.clamp(0.0, 100.0)),
        );
        let bytes = |id: u64| {
            let a = attribute(id)?;
            let name = a["name"].as_str().unwrap_or("");
            let raw = number(&a["raw"]["value"])?;
            let scale = if name.contains("LBA") {
                block
            } else if name.contains("32MiB") {
                32.0 * 1024.0 * 1024.0
            } else if name.contains("GiB") || name.contains("GB") {
                1024.0 * 1024.0 * 1024.0
            } else {
                return None;
            };
            Some(raw * scale)
        };
        put(&mut readings, Reading::HostReads, bytes(242));
        put(&mut readings, Reading::HostWrites, bytes(241));
        put(&mut readings, Reading::Reallocated, raw(5));
        put(&mut readings, Reading::Pending, raw(197));
        put(&mut readings, Reading::Uncorrectable, raw(198));
        let failing = table.iter().any(|a| {
            let (value, threshold) = (number(&a["value"]), number(&a["thresh"]));
            matches!((value, threshold), (Some(v), Some(t)) if t > 0.0 && v <= t)
        });
        failing || raw(197).unwrap_or(0.0) > 0.0 || raw(198).unwrap_or(0.0) > 0.0
    };
    if passed.is_some() || !readings.is_empty() {
        put(
            &mut readings,
            Reading::Warning,
            Some(if warning { 1.0 } else { 0.0 }),
        );
    }
    readings.sort_by_key(|&(reading, _)| reading);
    Some(Report {
        model,
        serial,
        readings,
    })
}

#[derive(Debug)]
struct Drive {
    entry: ScanEntry,
    channels: Vec<(Reading, ChannelId)>,
    values: Vec<(Reading, f64)>,
}

/// See the module documentation.
pub struct Smart {
    tool: Box<dyn SmartTool>,
    drives: Vec<Drive>,
    refreshed: Option<Instant>,
    asleep: usize,
    status: Status,
    /// Query again every [`REFRESH`] while sampling (off in tests, which drive it).
    refresh_while_sampling: bool,
}

impl Smart {
    pub fn new(tool: Box<dyn SmartTool>, refresh_while_sampling: bool) -> Self {
        Self {
            tool,
            drives: Vec::new(),
            refreshed: None,
            asleep: 0,
            status: Status::missing("S.M.A.R.T. has not been read yet"),
            refresh_while_sampling,
        }
    }

    /// Queries every known drive again.
    pub fn refresh(&mut self, now: Instant) {
        for drive in &mut self.drives {
            if let Some(report) = self
                .tool
                .query(&drive.entry.name, &drive.entry.kind)
                .ok()
                .and_then(|json| parse_report(&json))
            {
                drive.values = report.readings;
            }
        }
        self.refreshed = Some(now);
    }
}

impl Provider for Smart {
    fn name(&self) -> &'static str {
        "smart"
    }

    fn enumerate(&mut self) -> Vec<(Device, Vec<Channel>)> {
        self.drives.clear();
        self.asleep = 0;
        if !self.tool.present() {
            self.status = Status::missing(
                "smartctl is not installed, so there are no S.M.A.R.T. readings: install \
                 smartmontools (sudo apt install smartmontools).",
            );
            return Vec::new();
        }
        let scan = match self.tool.scan() {
            Ok(json) => parse_scan(&json),
            Err(err) => {
                self.status = Status::missing(format!("smartctl --scan failed: {err}."));
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        for (order, entry) in scan.into_iter().enumerate() {
            let Some(report) = self
                .tool
                .query(&entry.name, &entry.kind)
                .ok()
                .and_then(|json| parse_report(&json))
            else {
                self.asleep += 1;
                continue;
            };
            let identity = report.serial.as_deref().unwrap_or(&entry.name);
            let node = Path::new(&entry.name)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&entry.name)
                .to_string();
            let device = Device::new(
                format!("smart:{}:{}", slug(&report.model), short_hash(identity)),
                format!("S.M.A.R.T.: {} ({node})", report.model),
                Category::Smart,
            )
            .with_order(u16::try_from(order).unwrap_or(u16::MAX));
            let channels: Vec<Channel> = report
                .readings
                .iter()
                .map(|&(reading, _)| {
                    let channel =
                        Channel::new(&device.id, reading.key(), reading.label(), reading.kind())
                            .with_source(Source::Helper);
                    if reading == Reading::Temperature {
                        channel.with_decimals(0)
                    } else {
                        channel
                    }
                })
                .collect();
            self.drives.push(Drive {
                entry,
                channels: report
                    .readings
                    .iter()
                    .zip(&channels)
                    .map(|(&(reading, _), c)| (reading, c.id.clone()))
                    .collect(),
                values: report.readings,
            });
            out.push((device, channels));
        }
        self.refreshed = Some(Instant::now());
        self.status = match (self.drives.len(), self.asleep) {
            (0, 0) => Status::missing("smartctl found no drives."),
            (0, n) => Status::missing(format!(
                "{n} drive(s) did not answer (asleep, or not readable); they are tried again \
                 every minute."
            )),
            (_, 0) => Status::working(),
            (_, n) => Status {
                available: true,
                note: Some(format!(
                    "{n} drive(s) are asleep and are tried again every minute."
                )),
            },
        };
        out
    }

    fn sample(&mut self, now: Instant, out: &mut Vec<Sample>) {
        if self.refresh_while_sampling
            && self
                .refreshed
                .is_none_or(|t| now.saturating_duration_since(t) >= REFRESH)
        {
            self.refresh(now);
        }
        for drive in &self.drives {
            for (reading, channel) in &drive.channels {
                let value = drive
                    .values
                    .iter()
                    .find(|(r, _)| r == reading)
                    .map(|&(_, v)| v);
                out.push(Sample::new(channel.clone(), value));
            }
        }
    }

    fn status(&self) -> Status {
        self.status.clone()
    }

    /// Drives that were asleep at the last look show up once they are awake.
    fn rescan_every(&self) -> Duration {
        if self.asleep > 0 {
            REFRESH
        } else {
            Duration::from_secs(600)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/smartctl")
    }

    fn read(name: &str) -> Vec<u8> {
        std::fs::read(fixtures().join(name)).unwrap()
    }

    fn value(report: &Report, reading: Reading) -> Option<f64> {
        report
            .readings
            .iter()
            .find(|(r, _)| *r == reading)
            .map(|&(_, v)| v)
    }

    #[test]
    fn the_scan_lists_device_and_type() {
        let scan = parse_scan(&read("scan.json"));
        assert_eq!(
            scan,
            [
                ScanEntry {
                    name: "/dev/sda".into(),
                    kind: "sat".into()
                },
                ScanEntry {
                    name: "/dev/sdb".into(),
                    kind: "sat".into()
                },
                ScanEntry {
                    name: "/dev/nvme0".into(),
                    kind: "nvme".into()
                },
            ]
        );
        assert!(parse_scan(b"not json").is_empty());
    }

    #[test]
    fn nvme_health_becomes_hwinfo_rows() {
        let report = parse_report(&read("nvme0.json")).unwrap();
        assert_eq!(report.model, "Samsung SSD 970 EVO Plus 1TB");
        assert_eq!(value(&report, Reading::Temperature), Some(38.0));
        assert_eq!(value(&report, Reading::RemainingLife), Some(97.0));
        assert_eq!(value(&report, Reading::AvailableSpare), Some(100.0));
        assert_eq!(value(&report, Reading::Failure), Some(0.0));
        assert_eq!(value(&report, Reading::Warning), Some(0.0));
        assert_eq!(value(&report, Reading::PowerOnHours), Some(4120.0));
        assert_eq!(
            value(&report, Reading::HostWrites),
            Some(41_927_313.0 * 512_000.0)
        );
        assert_eq!(value(&report, Reading::MediaErrors), Some(0.0));
        assert_eq!(value(&report, Reading::Reallocated), None, "ATA only");
        let order: Vec<Reading> = report.readings.iter().map(|&(r, _)| r).collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(order, sorted);
    }

    #[test]
    fn ata_attributes_become_hwinfo_rows() {
        let report = parse_report(&read("sda.json")).unwrap();
        assert_eq!(report.model, "Samsung SSD 860 EVO 500GB");
        assert_eq!(value(&report, Reading::Temperature), Some(33.0));
        assert_eq!(
            value(&report, Reading::RemainingLife),
            Some(91.0),
            "attribute 177"
        );
        assert_eq!(
            value(&report, Reading::HostWrites),
            Some(29_830_104_123.0 * 512.0)
        );
        assert_eq!(value(&report, Reading::Reallocated), Some(0.0));
        assert_eq!(value(&report, Reading::Pending), Some(2.0));
        assert_eq!(
            value(&report, Reading::Warning),
            Some(1.0),
            "pending sectors are a warning"
        );
        assert_eq!(value(&report, Reading::Failure), Some(0.0));
    }

    #[test]
    fn sleeping_drives_are_skipped() {
        assert_eq!(parse_report(&read("sdb.json")), None);
    }

    #[test]
    fn drives_become_devices_without_their_serial_numbers() {
        let mut smart = Smart::new(Box::new(SmartctlDir::new(fixtures())), false);
        let devices = smart.enumerate();
        assert_eq!(devices.len(), 2, "the sleeping /dev/sdb is left out");
        let (sata, nvme) = (&devices[0], &devices[1]);
        assert_eq!(sata.0.label, "S.M.A.R.T.: Samsung SSD 860 EVO 500GB (sda)");
        assert_eq!(
            nvme.0.label,
            "S.M.A.R.T.: Samsung SSD 970 EVO Plus 1TB (nvme0)"
        );
        assert!(
            nvme.0
                .id
                .as_str()
                .starts_with("smart:samsung-ssd-970-evo-plus-1tb:")
        );
        assert!(!nvme.0.id.as_str().contains("SCRUBBED"));
        assert_eq!(nvme.0.category, Category::Smart);
        assert!(nvme.1.iter().all(|c| c.source == Source::Helper));
        let status = smart.status();
        assert!(status.available);
        assert!(status.note.unwrap().contains("asleep"));
        assert_eq!(smart.rescan_every(), REFRESH);

        let mut out = Vec::new();
        smart.sample(Instant::now(), &mut out);
        let temp = out
            .iter()
            .find(|s| s.channel == ChannelId::new(&nvme.0.id, "smart.temperature"))
            .unwrap();
        assert_eq!(temp.value, Some(38.0));
    }

    #[test]
    fn without_smartctl_the_note_says_what_to_install() {
        let mut smart = Smart::new(Box::new(NoSmartctl), false);
        assert!(smart.enumerate().is_empty());
        assert!(smart.status().note.unwrap().contains("smartmontools"));
    }
}

//! CSV logging of sensor readings (PLAN.md section 2, item 9). The sampler thread calls
//! [`CsvLogger::record`] right after every tick, so a row holds the readings of one tick.
//!
//! A file starts with a header row: `time_utc`, `elapsed_s`, then one column per sensor
//! named `<device> / <sensor> [<unit>]`. Each row has the UTC time (ISO 8601, with
//! milliseconds), the seconds since the file began, and the readings in each sensor's
//! base unit (°C, V, W, bytes, ...) with a `.` decimal point; booleans are 0 or 1 and a
//! missing reading is an empty field. A new file begins whenever the set of columns
//! changes (a sensor appears, is hidden, or is marked for the log) and, if asked, every
//! day at 00:00 UTC, so that every file is a plain rectangular table.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tracing::{info, warn};

use crate::model::{Channel, ChannelId, Unit};
use crate::sampler::Snapshot;

/// Which sensors get a column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogSelection {
    /// Every sensor except these (the hidden ones). Sensors that are not polled are
    /// always left out.
    AllExcept(HashSet<ChannelId>),
    /// Only these.
    Only(HashSet<ChannelId>),
}

impl LogSelection {
    fn includes(&self, id: &ChannelId) -> bool {
        match self {
            LogSelection::AllExcept(excluded) => !excluded.contains(id),
            LogSelection::Only(included) => included.contains(id),
        }
    }
}

/// What to log, and where.
#[derive(Clone, Debug, PartialEq)]
pub struct LogSpec {
    pub folder: PathBuf,
    /// Time between rows; zero logs every tick.
    pub interval: Duration,
    pub selection: LogSelection,
    /// Start a new file at 00:00 UTC.
    pub rotate_daily: bool,
    /// Column names for sensors the user renamed, instead of the backends' labels.
    pub names: HashMap<ChannelId, String>,
}

/// How logging is going, for the UI.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogStatus {
    pub active: bool,
    /// The file being written (or last written).
    pub path: Option<PathBuf>,
    /// Rows written since logging started.
    pub rows: u64,
    pub columns: usize,
    /// Files started since logging started.
    pub files: u32,
    /// Why logging stopped, when it stopped by itself.
    pub error: Option<String>,
}

struct OpenFile {
    writer: csv::Writer<File>,
    columns: Vec<ChannelId>,
    day: i64,
    started: Instant,
}

/// See the module documentation.
pub struct CsvLogger {
    spec: LogSpec,
    file: Option<OpenFile>,
    last_row: Option<Instant>,
    status: LogStatus,
}

impl CsvLogger {
    pub fn new(spec: LogSpec) -> Self {
        Self {
            spec,
            file: None,
            last_row: None,
            status: LogStatus {
                active: true,
                ..LogStatus::default()
            },
        }
    }

    pub fn spec(&self) -> &LogSpec {
        &self.spec
    }

    pub fn status(&self) -> &LogStatus {
        &self.status
    }

    /// Writes a row for `snapshot` when one is due. `now` is the tick's time, `wall` the
    /// time to stamp the row with. An I/O error stops logging and is kept in the status.
    pub fn record(&mut self, snapshot: &Snapshot, now: Instant, wall: SystemTime) {
        if !self.status.active || snapshot.tick == 0 {
            return;
        }
        // Half a polling period of slack, so that jitter does not skip a row.
        if let Some(last) = self.last_row
            && now.saturating_duration_since(last) + snapshot.interval / 2 < self.spec.interval
        {
            return;
        }
        if let Err(err) = self.write_row(snapshot, now, wall) {
            warn!("CSV logging stopped: {err}");
            self.status.error = Some(err.to_string());
            self.status.active = false;
            self.file = None;
            return;
        }
        self.last_row = Some(now);
    }

    fn write_row(&mut self, snapshot: &Snapshot, now: Instant, wall: SystemTime) -> io::Result<()> {
        let mut columns: Vec<(&str, &Channel, Option<f64>)> = Vec::new();
        for device in &snapshot.devices {
            for entry in &device.channels {
                if !entry.disabled && self.spec.selection.includes(&entry.channel.id) {
                    columns.push((&device.device.label, &entry.channel, entry.stats.current()));
                }
            }
        }
        let ids: Vec<ChannelId> = columns.iter().map(|(_, c, _)| c.id.clone()).collect();
        let day = utc_day(wall);
        let fresh = match &self.file {
            None => true,
            Some(file) => file.columns != ids || (self.spec.rotate_daily && file.day != day),
        };
        if fresh {
            self.file = None;
            let names: Vec<String> = columns
                .iter()
                .map(|(device, channel, _)| {
                    let label = self.spec.names.get(&channel.id).unwrap_or(&channel.label);
                    column_name(device, label, channel.unit)
                })
                .collect();
            self.open(names, ids, wall, day, now)?;
        }
        let Some(file) = self.file.as_mut() else {
            return Ok(());
        };
        let mut record = Vec::with_capacity(columns.len() + 2);
        record.push(iso_utc(wall));
        record.push(format!(
            "{:.3}",
            now.saturating_duration_since(file.started).as_secs_f64()
        ));
        for (_, channel, value) in &columns {
            record.push(cell(*value, channel.unit, channel.decimals));
        }
        file.writer.write_record(&record)?;
        file.writer.flush()?;
        self.status.rows += 1;
        Ok(())
    }

    fn open(
        &mut self,
        names: Vec<String>,
        columns: Vec<ChannelId>,
        wall: SystemTime,
        day: i64,
        now: Instant,
    ) -> io::Result<()> {
        fs::create_dir_all(&self.spec.folder)?;
        let (path, file) = create_unique(&self.spec.folder, &file_stamp(wall))?;
        let mut writer = csv::Writer::from_writer(file);
        let mut header = vec!["time_utc".to_string(), "elapsed_s".to_string()];
        header.extend(names);
        writer.write_record(&header)?;
        writer.flush()?;
        info!(path = %path.display(), columns = columns.len(), "CSV log file started");
        self.status.path = Some(path);
        self.status.columns = columns.len();
        self.status.files += 1;
        self.file = Some(OpenFile {
            writer,
            columns,
            day,
            started: now,
        });
        Ok(())
    }
}

/// `CPU [#0]: Intel Core i7-8700K / CPU Package [°C]`.
pub fn column_name(device: &str, label: &str, unit: Unit) -> String {
    match unit.symbol() {
        "" => format!("{device} / {label}"),
        symbol => format!("{device} / {label} [{symbol}]"),
    }
}

/// A reading as a CSV field.
pub fn cell(value: Option<f64>, unit: Unit, decimals: u8) -> String {
    match value.filter(|v| v.is_finite()) {
        None => String::new(),
        Some(v) => match unit {
            Unit::Bool => (if v != 0.0 { "1" } else { "0" }).to_string(),
            Unit::Bytes | Unit::BytesPerSecond | Unit::Count => format!("{v:.0}"),
            _ => format!("{:.*}", usize::from(decimals), v),
        },
    }
}

/// `kwinfo-<stamp>.csv` in `folder`, or `kwinfo-<stamp>-2.csv` and so on if taken.
fn create_unique(folder: &Path, stamp: &str) -> io::Result<(PathBuf, File)> {
    for n in 1..1000 {
        let name = if n == 1 {
            format!("kwinfo-{stamp}.csv")
        } else {
            format!("kwinfo-{stamp}-{n}.csv")
        };
        let path = folder.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "too many log files with the same name",
    ))
}

fn since_epoch(t: SystemTime) -> Duration {
    t.duration_since(UNIX_EPOCH).unwrap_or_default()
}

/// Days since 1970-01-01, UTC.
fn utc_day(t: SystemTime) -> i64 {
    (since_epoch(t).as_secs() / 86_400) as i64
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn parts(t: SystemTime) -> ((i64, u32, u32), (u64, u64, u64), u32) {
    let since = since_epoch(t);
    let secs = since.as_secs();
    let date = civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    (
        date,
        (rem / 3600, (rem / 60) % 60, rem % 60),
        since.subsec_millis(),
    )
}

/// `2026-10-20T14:03:05.123Z`.
pub fn iso_utc(t: SystemTime) -> String {
    let ((y, mo, d), (h, mi, s), ms) = parts(t);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{ms:03}Z")
}

/// `20261020-140305`, for file names.
fn file_stamp(t: SystemTime) -> String {
    let ((y, mo, d), (h, mi, s), _) = parts(t);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Category, Device, Kind};
    use crate::sampler::{ChannelSnapshot, DeviceSnapshot};
    use crate::stats::Stats;
    use std::sync::Arc;

    /// 2026-10-20T23:59:58Z.
    fn late_evening() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(20_746 * 86_400 + 86_398)
    }

    fn channel(
        device: &Device,
        key: &str,
        label: &str,
        kind: Kind,
        value: Option<f64>,
        disabled: bool,
    ) -> ChannelSnapshot {
        let mut stats = Stats::new();
        stats.update(value);
        ChannelSnapshot {
            channel: Arc::new(Channel::new(&device.id, key, label, kind)),
            stats,
            disabled,
        }
    }

    fn snapshot(tick: u64, temp: f64, extra: bool) -> Snapshot {
        let cpu = Device::new("cpu:0", "CPU [#0]: Intel Core i7-8700K", Category::Cpu);
        let mut channels = vec![
            channel(
                &cpu,
                "temp",
                "CPU Package",
                Kind::Temperature,
                Some(temp),
                false,
            ),
            channel(&cpu, "vcore", "Vcore", Kind::Voltage, Some(1.2345), false),
            channel(
                &cpu,
                "throttle",
                "Thermal Throttling",
                Kind::Boolean,
                Some(1.0),
                false,
            ),
            channel(&cpu, "fan", "CPU Fan", Kind::Fan, None, false),
            channel(&cpu, "off", "Disabled Sensor", Kind::Power, None, true),
        ];
        if extra {
            channels.push(channel(
                &cpu,
                "new",
                "New, Sensor",
                Kind::Clock,
                Some(4300.0),
                false,
            ));
        }
        let now = Instant::now();
        Snapshot {
            tick,
            started_at: now,
            taken_at: now,
            stats_since: now,
            interval: Duration::from_secs(1),
            tick_cost: Duration::ZERO,
            devices: vec![DeviceSnapshot {
                device: Arc::new(cpu),
                channels,
            }],
        }
    }

    fn spec(folder: &Path, interval: Duration) -> LogSpec {
        LogSpec {
            folder: folder.to_path_buf(),
            interval,
            selection: LogSelection::AllExcept(HashSet::from([ChannelId::from_raw("cpu:0/vcore")])),
            rotate_daily: true,
            names: HashMap::from([(ChannelId::from_raw("cpu:0/temp"), "Package".to_string())]),
        }
    }

    fn files(dir: &Path) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        files
    }

    #[test]
    fn writes_a_header_and_one_row_per_due_tick() {
        let dir = tempfile::tempdir().unwrap();
        let mut logger = CsvLogger::new(spec(dir.path(), Duration::from_secs(2)));
        let t0 = Instant::now();
        let wall = UNIX_EPOCH + Duration::from_millis(1_792_500_000_250);
        for tick in 1..=5u32 {
            let s = snapshot(u64::from(tick), 48.0 + f64::from(tick), false);
            logger.record(
                &s,
                t0 + Duration::from_secs(u64::from(tick)),
                wall + Duration::from_secs(u64::from(tick)),
            );
        }
        let status = logger.status().clone();
        assert!(status.active);
        assert_eq!(status.rows, 3, "ticks 1, 3 and 5 at a 2 s interval");
        assert_eq!(
            status.columns, 3,
            "Vcore excluded, the disabled sensor left out"
        );
        let path = status.path.unwrap();
        assert!(
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("kwinfo-2026")
        );
        let text = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[0],
            "time_utc,elapsed_s,CPU [#0]: Intel Core i7-8700K / Package [°C],\
             CPU [#0]: Intel Core i7-8700K / Thermal Throttling,CPU [#0]: Intel Core i7-8700K / CPU Fan [RPM]"
        );
        assert_eq!(lines.len(), 4);
        assert!(
            lines[1].starts_with("2026-10-20T12:40:01.250Z,"),
            "{}",
            lines[1]
        );
        assert!(lines[1].ends_with(",0.000,49.0,1,"), "{}", lines[1]);
        assert!(lines[3].ends_with(",4.000,53.0,1,"), "{}", lines[3]);
    }

    #[test]
    fn a_new_sensor_starts_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut logger = CsvLogger::new(spec(dir.path(), Duration::ZERO));
        let t0 = Instant::now();
        let wall = UNIX_EPOCH + Duration::from_secs(1_792_500_000);
        logger.record(&snapshot(1, 50.0, false), t0, wall);
        logger.record(
            &snapshot(2, 51.0, true),
            t0 + Duration::from_secs(1),
            wall + Duration::from_secs(1),
        );
        let files = files(dir.path());
        assert_eq!(files.len(), 2, "{files:?}");
        let second = fs::read_to_string(&files[1]).unwrap();
        assert!(
            second
                .lines()
                .next()
                .unwrap()
                .ends_with(",\"CPU [#0]: Intel Core i7-8700K / New, Sensor [MHz]\""),
            "{second}"
        );
        assert!(second.lines().nth(1).unwrap().ends_with(",4300.0"));
        assert_eq!(logger.status().files, 2);
    }

    #[test]
    fn daily_rotation_starts_a_file_at_midnight_utc() {
        let dir = tempfile::tempdir().unwrap();
        let mut logger = CsvLogger::new(spec(dir.path(), Duration::ZERO));
        let t0 = Instant::now();
        for i in 0..4u64 {
            logger.record(
                &snapshot(i + 1, 50.0, false),
                t0 + Duration::from_secs(i),
                late_evening() + Duration::from_secs(i),
            );
        }
        let files = files(dir.path());
        assert_eq!(files.len(), 2);
        assert!(
            files[1]
                .to_str()
                .unwrap()
                .contains("kwinfo-20261021-000000"),
            "{files:?}"
        );
        assert_eq!(
            fs::read_to_string(&files[1]).unwrap().lines().count(),
            3,
            "header, 00:00:00, 00:00:01"
        );
    }

    #[test]
    fn marked_sensors_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut only = spec(dir.path(), Duration::ZERO);
        only.selection = LogSelection::Only(HashSet::from([ChannelId::from_raw("cpu:0/vcore")]));
        let mut logger = CsvLogger::new(only);
        logger.record(&snapshot(1, 50.0, false), Instant::now(), SystemTime::now());
        let text = fs::read_to_string(logger.status().path.clone().unwrap()).unwrap();
        assert_eq!(
            text.lines().next().unwrap(),
            "time_utc,elapsed_s,CPU [#0]: Intel Core i7-8700K / Vcore [V]"
        );
        // 1.2345 is 1.23449… in binary; the sensor's 3 decimals give 1.234.
        assert!(text.lines().nth(1).unwrap().ends_with(",1.234"), "{text}");
    }

    #[test]
    fn an_unwritable_folder_stops_logging_with_the_reason() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("not-a-folder");
        fs::write(&blocker, "").unwrap();
        let mut logger = CsvLogger::new(spec(&blocker, Duration::ZERO));
        logger.record(&snapshot(1, 50.0, false), Instant::now(), SystemTime::now());
        let status = logger.status();
        assert!(!status.active);
        assert!(status.error.is_some());
        assert_eq!(status.rows, 0);
    }

    #[test]
    fn cells_and_times() {
        assert_eq!(cell(Some(48.04), Unit::Celsius, 1), "48.0");
        assert_eq!(cell(Some(0.0), Unit::Bool, 0), "0");
        assert_eq!(cell(Some(1536.4), Unit::Bytes, 1), "1536");
        assert_eq!(cell(Some(f64::NAN), Unit::Volt, 3), "");
        assert_eq!(cell(None, Unit::Volt, 3), "");
        assert_eq!(
            iso_utc(late_evening() + Duration::from_millis(5)),
            "2026-10-20T23:59:58.005Z"
        );
        assert_eq!(file_stamp(late_evening()), "20261020-235958");
        assert_eq!(
            column_name("Memory", "Load", Unit::Percent),
            "Memory / Load [%]"
        );
    }
}

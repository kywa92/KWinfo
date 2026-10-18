//! `kwinfo-cli record`: a snapshot plus every value that changes, frame by frame, so that
//! `--replay` shows usage, rates and temperatures moving the way they did on the recorded
//! machine.
//!
//! A recording is a snapshot directory (see [`crate::snapshot`]) with one more file,
//! `kwinfo-recording.txt`:
//!
//! ```text
//! kwinfo-recording 1
//! interval-us 1000000
//! frame 0 0
//! 2971 proc/stat
//! <2971 bytes>
//! 6 sys/devices/platform/coretemp.0/hwmon/hwmon1/temp1_input
//! <6 bytes>
//! frame 1 1000213
//! ...
//! ```
//!
//! A frame starts with its number and the time since frame 0 in microseconds. Frame 0
//! holds every tracked file; later frames hold only the files that changed. An entry is
//! the byte length and the path (relative to the root, no symlinks) on one line, then
//! exactly that many bytes and a newline. Tracked files are the snapshot's files whose
//! contents move ([`is_tracked`]): `/proc/stat`, `/proc/diskstats`, `/proc/meminfo`, hwmon
//! readings, clocks, idle-state times, throttle counters, disk and network counters and
//! battery values. They carry no identifiers, and pass through the snapshot's scrubber
//! anyway.
//!
//! Replay copies the snapshot to a private temporary directory and, before every sample,
//! writes the next frame's files into the copy; the backends read it as they read any
//! tree. The copy's [`SysRoot::now`] returns the recorded time, so rates come out as
//! recorded whatever the polling interval of the replay. After the last frame the
//! recording starts over from frame 0.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write as _};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};

use crate::backend::Backend;
use crate::backends::hwmon;
use crate::model::{Channel, Device, Sample};
use crate::snapshot::{self, Scrubbed, Scrubber, SnapshotReport};
use crate::sysroot::{ReplayClock, SysRoot};

/// The file that turns a snapshot into a recording.
pub const RECORDING: &str = "kwinfo-recording.txt";

const MAGIC: &str = "kwinfo-recording 1";

/// No sysfs or procfs file KWinfo reads comes anywhere near this; a bigger entry means
/// the recording is corrupt.
const MAX_ENTRY: usize = 4 << 20;

/// Battery and UPS attributes that change while the machine runs.
const SUPPLY_VALUES: &[&str] = &[
    "status",
    "capacity",
    "capacity_level",
    "energy_now",
    "energy_full",
    "charge_now",
    "charge_full",
    "power_now",
    "current_now",
    "voltage_now",
    "temp",
    "online",
    "present",
    "cycle_count",
    "time_to_empty_now",
    "time_to_full_now",
];

/// Whether the snapshot file at `rel` (relative to the root, symlinks resolved) holds a
/// value that changes while the machine runs, and so is re-read for every frame.
pub(crate) fn is_tracked(rel: &Path) -> bool {
    let Some(name) = rel.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let path = rel.to_string_lossy();
    if let Some(rest) = path.strip_prefix("proc/") {
        return matches!(rest, "stat" | "diskstats" | "meminfo");
    }
    if path.contains("/power_supply/") {
        return SUPPLY_VALUES.contains(&name);
    }
    if path.contains("/cpuidle/") {
        // Only the time is read; the entry counts would double the size of every frame.
        return name == "time";
    }
    let parent = rel
        .parent()
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    matches!(parent, "statistics" | "thermal_throttle")
        || matches!(name, "stat" | "scaling_cur_freq" | "operstate" | "carrier")
        || name.ends_with("_freq_mhz")
        || hwmon::is_reading_attribute(name)
}

/// What a finished recording contains.
#[derive(Debug)]
pub struct RecordReport {
    pub snapshot: SnapshotReport,
    /// Frames written, frame 0 included.
    pub frames: usize,
    /// Files re-read for every frame.
    pub tracked: usize,
    /// Entries written over all frames.
    pub changes: usize,
    /// Size of the recording file.
    pub bytes: u64,
}

/// Writes a recording: [`Recorder::start`] takes the snapshot and frame 0, each
/// [`Recorder::capture_frame`] appends a frame, [`Recorder::finish`] reports.
///
/// The caller decides when frames are taken; `kwinfo-cli record` waits for the interval,
/// tests change a fixture tree between frames.
pub struct Recorder {
    source: SysRoot,
    out: PathBuf,
    tracked: Vec<PathBuf>,
    last: HashMap<PathBuf, Vec<u8>>,
    scrubber: Scrubber,
    writer: BufWriter<File>,
    snapshot: SnapshotReport,
    started: Instant,
    frames: usize,
    changes: usize,
}

impl Recorder {
    /// Takes a snapshot of `source` into `out` (which must not exist yet or be empty) and
    /// records frame 0. `interval` is the intended time between frames; replay uses it
    /// as the pause between the last frame and the first when it starts over.
    pub fn start(source: &SysRoot, out: &Path, interval: Duration) -> anyhow::Result<Self> {
        let captured = snapshot::capture_tracked(source, out)?;
        let tracked: Vec<PathBuf> = captured
            .files
            .into_iter()
            .filter(|rel| rel.to_str().is_some() && is_tracked(rel))
            .collect();
        let path = out.join(RECORDING);
        let file = File::create(&path).with_context(|| format!("creating {}", path.display()))?;
        let mut writer = BufWriter::new(file);
        writeln!(writer, "{MAGIC}")?;
        writeln!(writer, "interval-us {}", interval.as_micros())?;
        let mut recorder = Self {
            source: source.clone(),
            out: out.to_path_buf(),
            tracked,
            last: HashMap::new(),
            scrubber: captured.scrubber,
            writer,
            snapshot: captured.report,
            started: Instant::now(),
            frames: 0,
            changes: 0,
        };
        recorder.started = Instant::now();
        recorder.capture_frame(Duration::ZERO)?;
        Ok(recorder)
    }

    /// When frame 0 was read. Later frames are timed from here.
    pub fn started(&self) -> Instant {
        self.started
    }

    /// Reads the tracked files again and appends the ones that changed, as a frame taken
    /// `elapsed` after frame 0. Returns how many files changed. Files that cannot be read
    /// this time keep their previous contents.
    pub fn capture_frame(&mut self, elapsed: Duration) -> anyhow::Result<usize> {
        writeln!(self.writer, "frame {} {}", self.frames, elapsed.as_micros())?;
        let mut changed = 0;
        for rel in &self.tracked {
            let Ok(data) = fs::read(self.source.path(rel)) else {
                continue;
            };
            let data = match self.scrubber.scrub(rel, data) {
                Scrubbed::Kept(data) | Scrubbed::Changed(data) => data,
                Scrubbed::Dropped => continue,
            };
            if data.len() > MAX_ENTRY || self.last.get(rel) == Some(&data) {
                continue;
            }
            let path = rel.to_str().expect("tracked paths are UTF-8");
            writeln!(self.writer, "{} {path}", data.len())?;
            self.writer.write_all(&data)?;
            self.writer.write_all(b"\n")?;
            self.last.insert(rel.clone(), data);
            changed += 1;
        }
        // A recording cut short (Ctrl-C) keeps every frame written so far.
        self.writer.flush()?;
        self.frames += 1;
        self.changes += changed;
        Ok(changed)
    }

    pub fn finish(mut self) -> anyhow::Result<RecordReport> {
        self.writer.flush()?;
        let bytes = fs::metadata(self.out.join(RECORDING))?.len();
        Ok(RecordReport {
            snapshot: self.snapshot,
            frames: self.frames,
            tracked: self.tracked.len(),
            changes: self.changes,
            bytes,
        })
    }
}

/// Whether `dir` holds a recording rather than a plain snapshot.
pub fn is_recording(dir: &Path) -> bool {
    dir.join(RECORDING).is_file()
}

#[derive(Debug, Default)]
struct Recording {
    interval: Duration,
    frames: Vec<Frame>,
}

#[derive(Debug)]
struct Frame {
    elapsed: Duration,
    files: Vec<(PathBuf, Vec<u8>)>,
}

/// A relative path made only of plain components, so that it cannot leave the tree.
fn plain_relative(path: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    let plain = !path.as_os_str().is_empty()
        && path.components().all(|c| matches!(c, Component::Normal(_)));
    plain.then(|| path.to_path_buf())
}

/// Reads a recording file front to back.
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    /// The next line without its newline; `None` at the end or in a cut-off last line.
    fn line(&mut self) -> Option<&'a [u8]> {
        let rest = self.data.get(self.pos..)?;
        let end = rest.iter().position(|&b| b == b'\n')?;
        self.pos += end + 1;
        Some(&rest[..end])
    }

    /// The next `len` bytes and the newline after them; `None` if the file ends first.
    fn take(&mut self, len: usize) -> anyhow::Result<Option<&'a [u8]>> {
        let Some(contents) = self.data.get(self.pos..self.pos + len) else {
            return Ok(None);
        };
        if self.data.get(self.pos + len).is_some_and(|&b| b != b'\n') {
            bail!("an entry is longer than its length says");
        }
        self.pos += len + 1;
        Ok(Some(contents))
    }
}

/// Parses a recording file. An entry cut off by the end of the file (a recording that was
/// interrupted mid-write) ends the recording; anything else malformed is an error.
fn parse(data: &[u8]) -> anyhow::Result<Recording> {
    let mut cursor = Cursor { data, pos: 0 };
    if cursor.line() != Some(MAGIC.as_bytes()) {
        bail!("not a KWinfo recording (the first line is not `{MAGIC}`)");
    }
    let mut recording = Recording {
        interval: Duration::from_secs(1),
        frames: Vec::new(),
    };
    while let Some(raw) = cursor.line() {
        let text = std::str::from_utf8(raw).context("a header line is not UTF-8")?;
        let (head, rest) = text.split_once(' ').unwrap_or((text, ""));
        match head {
            "interval-us" if recording.frames.is_empty() => {
                let micros: u64 = rest.parse().context("bad interval")?;
                recording.interval = Duration::from_micros(micros);
            }
            "frame" => {
                let micros = rest
                    .split_whitespace()
                    .nth(1)
                    .and_then(|t| t.parse::<u64>().ok())
                    .with_context(|| format!("bad frame line `{text}`"))?;
                recording.frames.push(Frame {
                    elapsed: Duration::from_micros(micros),
                    files: Vec::new(),
                });
            }
            _ if head.bytes().all(|b| b.is_ascii_digit()) && !head.is_empty() => {
                let len: usize = head.parse().context("bad entry length")?;
                if len > MAX_ENTRY {
                    bail!("entry `{rest}` is {len} bytes long");
                }
                let rel = plain_relative(rest)
                    .with_context(|| format!("entry path `{rest}` leaves the tree"))?;
                let Some(frame) = recording.frames.last_mut() else {
                    bail!("entry `{rest}` before the first frame");
                };
                let Some(contents) = cursor
                    .take(len)
                    .with_context(|| format!("entry `{rest}`"))?
                else {
                    break;
                };
                frame.files.push((rel, contents.to_vec()));
            }
            // Header fields from newer versions.
            _ if recording.frames.is_empty() => {}
            _ => bail!("unexpected line `{text}`"),
        }
    }
    Ok(recording)
}

/// Copies a captured tree, recreating symlinks with their original targets. Files are
/// written fresh so that the copy is writable whatever the permissions of the original.
pub(crate) fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let dest = to.join(entry.file_name());
        if kind.is_symlink() {
            let target = fs::read_link(entry.path())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &dest)?;
            #[cfg(not(unix))]
            return Err(io::Error::other(format!(
                "cannot recreate the link to {}",
                target.display()
            )));
        } else if kind.is_dir() {
            copy_tree(&entry.path(), &dest)?;
        } else if kind.is_file() {
            fs::write(&dest, fs::read(entry.path())?)?;
        }
    }
    Ok(())
}

/// Plays a recording back. It is a backend that reports nothing itself: placed first in
/// the backend list, its [`Backend::sample`] writes the next frame into the working copy
/// and sets the clock, and the real backends then sample that copy.
pub(crate) struct Playback {
    /// Removed when the playback is dropped, i.e. when the sampler stops.
    work: tempfile::TempDir,
    /// The recording's frames, with each file resolved to its place in the working copy.
    frames: Vec<Frame>,
    next: usize,
    /// Added to the recorded times on every pass, so the clock never runs backwards.
    pass_offset: Duration,
    /// The length of one pass: the last frame's time plus one interval.
    period: Duration,
    clock: Arc<ReplayClock>,
}

impl Playback {
    /// Reads the recording in `dir` and prepares a working copy of its snapshot. Returns
    /// the playback and the root the backends must read.
    pub(crate) fn open(dir: &Path) -> anyhow::Result<(Self, SysRoot)> {
        let path = dir.join(RECORDING);
        let data = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let recording = parse(&data).with_context(|| format!("parsing {}", path.display()))?;

        let work = tempfile::Builder::new()
            .prefix("kwinfo-replay-")
            .tempdir()
            .context("creating the replay's working copy")?;
        for top in ["sys", "proc", "usr"] {
            let from = dir.join(top);
            if from.is_dir() {
                copy_tree(&from, &work.path().join(top))
                    .with_context(|| format!("copying {}", from.display()))?;
            }
        }
        let base = fs::canonicalize(work.path())?;

        // Resolve every destination once. Entries that would land outside the working
        // copy (through a symlink in a hostile recording) or on anything but a file of
        // the snapshot are dropped.
        let mut targets: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
        let mut frames = Vec::with_capacity(recording.frames.len());
        for frame in recording.frames {
            let files: Vec<(PathBuf, Vec<u8>)> = frame
                .files
                .into_iter()
                .filter_map(|(rel, contents)| {
                    let target = targets.entry(rel).or_insert_with_key(|rel| {
                        let resolved = fs::canonicalize(base.join(rel)).ok()?;
                        (resolved.starts_with(&base) && resolved.is_file()).then_some(resolved)
                    });
                    Some((target.clone()?, contents))
                })
                .collect();
            frames.push(Frame {
                elapsed: frame.elapsed,
                files,
            });
        }
        let period = frames.last().map_or(Duration::ZERO, |f| f.elapsed) + recording.interval;
        let clock = Arc::new(ReplayClock::new());
        let root = SysRoot::new(&base).with_clock(Arc::clone(&clock));
        let playback = Self {
            work,
            frames,
            next: 0,
            pass_offset: Duration::ZERO,
            period,
            clock,
        };
        Ok((playback, root))
    }

    pub(crate) fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Writes the next frame into the working copy and moves the clock to its time.
    fn advance(&mut self) {
        if self.frames.is_empty() {
            return;
        }
        if self.next == self.frames.len() {
            self.next = 0;
            self.pass_offset += self.period;
        }
        let frame = &self.frames[self.next];
        for (target, contents) in &frame.files {
            if let Err(err) = fs::write(target, contents) {
                tracing::debug!("replay could not write {}: {err}", target.display());
            }
        }
        self.clock.set(self.pass_offset + frame.elapsed);
        self.next += 1;
    }
}

impl Backend for Playback {
    fn name(&self) -> &'static str {
        "replay"
    }

    fn enumerate(&mut self) -> anyhow::Result<Vec<(Device, Vec<Channel>)>> {
        Ok(Vec::new())
    }

    fn sample(&mut self, _out: &mut Vec<Sample>) -> anyhow::Result<()> {
        self.advance();
        Ok(())
    }

    fn rescan_every(&self) -> Option<Duration> {
        None
    }
}

impl std::fmt::Debug for Playback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Playback")
            .field("work", &self.work.path())
            .field("frames", &self.frames.len())
            .field("next", &self.next)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::test_support::*;
    use crate::{BackendChoice, BackendSet, ChannelId};

    /// Adds `by` to the integer in the file at `rel`.
    fn bump(root: &SysRoot, rel: &str, by: u64) {
        let value: u64 = fs::read_to_string(root.path(rel))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        write(root, rel, &format!("{}\n", value + by));
    }

    /// One second of activity on a fixture copy: `busy` percent on cpu0 and every other
    /// thread idle, `rx` bytes downloaded, `sectors` read from the NVMe drive, `c6_us` in
    /// C6 on every thread, and the CPU package at `package_mc` m°C.
    fn one_second(root: &SysRoot, busy: u64, rx: u64, sectors: u64, c6_us: u64, package_mc: u32) {
        let stat = fs::read_to_string(root.proc("stat")).unwrap();
        let lines: Vec<String> = stat
            .lines()
            .map(|line| {
                let mut fields: Vec<String> = line.split_whitespace().map(str::to_string).collect();
                if let Some(cpu) = fields[0].strip_prefix("cpu").filter(|n| !n.is_empty()) {
                    let (user, idle) = if cpu == "0" {
                        (busy, 100 - busy)
                    } else {
                        (0, 100)
                    };
                    let add = |v: &str, by: u64| (v.parse::<u64>().unwrap() + by).to_string();
                    fields[1] = add(&fields[1], user);
                    fields[4] = add(&fields[4], idle);
                }
                fields.join(" ")
            })
            .collect();
        write(root, "proc/stat", &(lines.join("\n") + "\n"));

        let diskstats = fs::read_to_string(root.proc("diskstats")).unwrap();
        let lines: Vec<String> = diskstats
            .lines()
            .map(|line| {
                let mut fields: Vec<String> = line.split_whitespace().map(str::to_string).collect();
                if fields[2] == "nvme0n1" {
                    fields[5] = (fields[5].parse::<u64>().unwrap() + sectors).to_string();
                }
                fields.join(" ")
            })
            .collect();
        write(root, "proc/diskstats", &(lines.join("\n") + "\n"));

        bump(
            root,
            "sys/devices/pci0000:00/0000:00:1f.6/net/enp0s31f6/statistics/rx_bytes",
            rx,
        );
        for cpu in 0..12 {
            bump(
                root,
                &format!("sys/devices/system/cpu/cpu{cpu}/cpuidle/state4/time"),
                c6_us,
            );
        }
        write(
            root,
            "sys/devices/platform/coretemp.0/hwmon/hwmon1/temp1_input",
            &format!("{package_mc}\n"),
        );
    }

    #[test]
    fn a_recording_replays_usage_rates_and_temperatures() {
        let (_source_dir, source) = fixture_copy();
        let out_dir = tempfile::tempdir().unwrap();
        let out = out_dir.path().join("recording");
        let mut recorder = Recorder::start(&source, &out, Duration::from_secs(1)).unwrap();
        one_second(&source, 50, 1 << 20, 2048, 600_000, 45_000);
        assert!(recorder.capture_frame(Duration::from_secs(1)).unwrap() > 0);
        one_second(&source, 25, 2 << 20, 4096, 300_000, 47_000);
        recorder.capture_frame(Duration::from_secs(2)).unwrap();
        let report = recorder.finish().unwrap();
        assert_eq!(report.frames, 3);
        assert!(is_recording(&out));

        let set = BackendSet::build(BackendChoice::Replay(out.clone()));
        assert_eq!(set.backends.len(), 8, "the player and the seven backends");
        assert!(
            set.notes
                .iter()
                .any(|n| n.contains("recording of 3 frames")),
            "{:?}",
            set.notes
        );
        let mut backends = set.backends;
        let devices: Vec<(Device, Vec<Channel>)> = backends
            .iter_mut()
            .flat_map(|b| b.enumerate().unwrap())
            .collect();
        let nvme = &devices
            .iter()
            .find(|(d, _)| d.label == "Drive: Samsung SSD 970 EVO Plus 1TB")
            .expect("the NVMe drive is replayed")
            .0
            .id;
        let read_rate = ChannelId::new(nvme, "read_rate");
        let id = ChannelId::from_raw;
        let mut tick = || {
            let mut out = Vec::new();
            for backend in &mut backends {
                backend.sample(&mut out).unwrap();
            }
            sample_map(&out)
        };

        // Frame 0: nothing to compare with yet.
        let first = tick();
        assert_eq!(first[&id("cpu:0/usage.cpu0")], None);
        assert_close(first[&id("cpu:0/coretemp.temp1_input")], 38.0);

        // Frame 1, one recorded second later, whatever the time between the two calls.
        let second = tick();
        assert_close(second[&id("cpu:0/usage.cpu0")], 50.0);
        assert_close(second[&id("cpu:0/usage.cpu6")], 0.0);
        assert_close(second[&id("cpu:0/usage.total")], 50.0 / 12.0);
        assert_close(second[&id("net:enp0s31f6/download")], 1_048_576.0);
        assert_close(second[&read_rate], 1_048_576.0);
        assert_close(second[&id("cpu:0/cstate.c6")], 60.0);
        assert_close(second[&id("cpu:0/coretemp.temp1_input")], 45.0);

        let third = tick();
        assert_close(third[&id("cpu:0/usage.cpu0")], 25.0);
        assert_close(third[&id("net:enp0s31f6/download")], 2.0 * 1_048_576.0);
        assert_close(third[&read_rate], 2.0 * 1_048_576.0);
        assert_close(third[&id("cpu:0/cstate.c6")], 30.0);
        assert_close(third[&id("cpu:0/coretemp.temp1_input")], 47.0);

        // Starting over, the counters jump back: one tick without rates, then the first
        // second again.
        let restart = tick();
        assert_eq!(restart[&id("net:enp0s31f6/download")], None);
        assert_eq!(restart[&id("cpu:0/usage.cpu0")], None);
        assert_close(restart[&id("cpu:0/coretemp.temp1_input")], 38.0);
        let again = tick();
        assert_close(again[&id("cpu:0/usage.cpu0")], 50.0);
        assert_close(again[&id("net:enp0s31f6/download")], 1_048_576.0);
    }

    #[test]
    fn frames_hold_values_only_and_only_when_they_change() {
        let out_dir = tempfile::tempdir().unwrap();
        let out = out_dir.path().join("idle");
        let mut recorder =
            Recorder::start(&fixture_root(), &out, Duration::from_millis(500)).unwrap();
        assert_eq!(
            recorder.capture_frame(Duration::from_millis(500)).unwrap(),
            0
        );
        let report = recorder.finish().unwrap();
        assert_eq!(report.frames, 2);
        assert_eq!(report.changes, report.tracked, "only frame 0 has entries");

        let recording = parse(&fs::read(out.join(RECORDING)).unwrap()).unwrap();
        assert_eq!(recording.interval, Duration::from_millis(500));
        assert_eq!(recording.frames.len(), 2);
        assert!(recording.frames[1].files.is_empty());
        let paths: Vec<String> = recording.frames[0]
            .files
            .iter()
            .map(|(p, _)| p.display().to_string())
            .collect();
        for wanted in [
            "proc/stat",
            "proc/meminfo",
            "hwmon2/fan2_input",
            "hwmon2/pwm1",
            "hwmon2/in2_alarm",
            "cpufreq/policy0/scaling_cur_freq",
            "cpu0/cpuidle/state4/time",
            "cpu0/thermal_throttle/package_throttle_total_time_ms",
            "nvme0n1/stat",
            "enp0s31f6/statistics/rx_bytes",
            "gt/gt0/rps_act_freq_mhz",
            "power_supply/BAT0/energy_now",
        ] {
            assert!(
                paths.iter().any(|p| p.ends_with(wanted)),
                "{wanted} is tracked"
            );
        }
        for unwanted in [
            "_label",
            "_max",
            "_crit",
            "serial",
            "serial_number",
            "model",
            "wwid",
            "cpuidle/state4/usage",
        ] {
            assert!(
                !paths.iter().any(|p| p.ends_with(unwanted)),
                "{unwanted} is not tracked"
            );
        }
        assert!(
            !paths
                .iter()
                .any(|p| p.contains("dmi") || p.contains("cpuinfo"))
        );
    }

    #[test]
    fn malformed_recordings_are_refused_and_cut_off_ones_kept() {
        assert!(parse(b"not a recording\n").is_err());
        for hostile in ["5 ../x", "5 /etc/x", "5 sys/../../x", "5 ./x"] {
            let data = format!("kwinfo-recording 1\nframe 0 0\n{hostile}\nhello\n");
            assert!(parse(data.as_bytes()).is_err(), "{hostile}");
        }
        assert!(
            parse(b"kwinfo-recording 1\n5 proc/x\nhello\n").is_err(),
            "an entry before the first frame"
        );
        assert!(
            parse(b"kwinfo-recording 1\nframe 0 0\n3 proc/x\nhello\n").is_err(),
            "a length that does not match"
        );

        let cut = parse(
            b"kwinfo-recording 1\ninterval-us 250000\nnew-field 7\nframe 0 0\n5 proc/x\nhello\n\
              frame 1 250000\n9 proc/x\nhel",
        )
        .unwrap();
        assert_eq!(cut.interval, Duration::from_millis(250));
        assert_eq!(cut.frames.len(), 2);
        assert_eq!(
            cut.frames[0].files,
            vec![(PathBuf::from("proc/x"), b"hello".to_vec())]
        );
        assert!(cut.frames[1].files.is_empty());
    }

    #[test]
    fn frames_never_write_outside_the_working_copy() {
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("victim");
        fs::write(&victim, "untouched\n").unwrap();

        // A hostile recording: a link out of the tree, and a frame writing through it.
        let recording = tempfile::tempdir().unwrap();
        fs::create_dir_all(recording.path().join("proc")).unwrap();
        fs::write(recording.path().join("proc/stat"), "cpu0 1 0 1 1\n").unwrap();
        std::os::unix::fs::symlink(outside.path(), recording.path().join("proc/escape")).unwrap();
        fs::write(
            recording.path().join(RECORDING),
            "kwinfo-recording 1\nframe 0 0\n6 proc/escape/victim\npwned\n\n7 proc/stat\ncpu0 2\n\n",
        )
        .unwrap();

        let (mut playback, root) = Playback::open(recording.path()).unwrap();
        playback.advance();
        assert_eq!(fs::read_to_string(&victim).unwrap(), "untouched\n");
        assert_eq!(fs::read_to_string(root.proc("stat")).unwrap(), "cpu0 2\n");
        assert_eq!(
            fs::read_to_string(recording.path().join("proc/stat")).unwrap(),
            "cpu0 1 0 1 1\n",
            "the recording itself is never modified"
        );

        let work = root.root().to_path_buf();
        assert!(work.exists());
        drop(playback);
        assert!(!work.exists(), "the working copy goes with the playback");
    }
}

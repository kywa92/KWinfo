//! `kwinfo-cli snapshot`: copies the parts of `/sys` and `/proc` that the backends read
//! into a directory, so that the tree can be committed as a test fixture, attached to a
//! bug report, and run with `--replay`.
//!
//! Only files on an allow-list are read: sysfs has attributes that are slow, huge,
//! binary or have side effects when read, and the snapshot must not carry personal
//! data. Serial numbers, WWNs/EUIs, NVMe qualified names and MAC addresses are replaced
//! by placeholders (the same original always maps to the same placeholder); the VPD
//! pages that carry serials are dropped, and the ATA IDENTIFY data in VPD page 0x89 keeps
//! its model string but loses its serial number and WWN. DMI serial numbers and UUIDs
//! are never copied. Symlinks are recreated with their original relative targets, so the
//! copy resolves exactly like the live tree.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, bail};

use crate::backends::block::VPD89_IDENTIFY;
use crate::backends::{PCI_IDS_PATHS, is_pci_address};
use crate::sysroot::{SysRoot, lexical_join, list_dir, parse_hex, read_string};

/// What a capture did, for the CLI to print and for the manifest file.
#[derive(Debug, Default)]
pub struct SnapshotReport {
    pub files: usize,
    pub links: usize,
    /// Files whose contents were replaced or redacted, relative to the root.
    pub scrubbed: Vec<String>,
    /// Allow-listed files that could not be read (permissions, driver errors).
    pub unreadable: Vec<(String, String)>,
    pub warnings: Vec<String>,
}

/// The manifest written next to `sys/` and `proc/`.
pub const MANIFEST: &str = "kwinfo-snapshot.txt";

const CPUFREQ_FILES: &[&str] = &[
    "scaling_cur_freq",
    "scaling_min_freq",
    "scaling_max_freq",
    "cpuinfo_min_freq",
    "cpuinfo_max_freq",
    "base_frequency",
    "scaling_driver",
    "scaling_governor",
    "energy_performance_preference",
    "affected_cpus",
    "related_cpus",
];
const CPUIDLE_FILES: &[&str] = &[
    "name",
    "desc",
    "latency",
    "residency",
    "time",
    "usage",
    "disable",
];
const BLOCK_FILES: &[&str] = &[
    "size",
    "removable",
    "ro",
    "dev",
    "stat",
    "wwid",
    "nsid",
    "ext_range",
];
const QUEUE_FILES: &[&str] = &[
    "rotational",
    "logical_block_size",
    "physical_block_size",
    "hw_sector_size",
];
const DISK_DEVICE_FILES: &[&str] = &[
    "model",
    "vendor",
    "rev",
    "firmware_rev",
    "serial",
    "wwid",
    "state",
    "type",
    "name",
    "vpd_pg89",
];
const NVME_FILES: &[&str] = &[
    "model",
    "serial",
    "firmware_rev",
    "transport",
    "address",
    "state",
    "cntlid",
    "subsysnqn",
    "numa_node",
];
const NET_FILES: &[&str] = &[
    "type",
    "flags",
    "operstate",
    "carrier",
    "mtu",
    "speed",
    "duplex",
    "address",
    "addr_assign_type",
    "ifindex",
    "iflink",
];
const NET_STAT_FILES: &[&str] = &[
    "rx_bytes",
    "tx_bytes",
    "rx_packets",
    "tx_packets",
    "rx_errors",
    "tx_errors",
    "rx_dropped",
    "tx_dropped",
];
const PCI_FILES: &[&str] = &[
    "vendor",
    "device",
    "subsystem_vendor",
    "subsystem_device",
    "class",
    "revision",
];
/// DMI fields that identify the model, never the unit: no serials, UUID or asset tags.
const DMI_FILES: &[&str] = &[
    "board_vendor",
    "board_name",
    "board_version",
    "sys_vendor",
    "product_name",
    "product_version",
    "product_family",
    "bios_vendor",
    "bios_version",
    "bios_date",
    "bios_release",
    "chassis_type",
    "chassis_vendor",
];

/// Copies the sensor-relevant parts of `source` into `out`, which must not exist yet or
/// be empty, and writes a manifest describing what was scrubbed and what could not be
/// read.
pub fn capture(source: &SysRoot, out: &Path) -> anyhow::Result<SnapshotReport> {
    if !source.sys("").is_dir() || !source.proc("").is_dir() {
        bail!(
            "{} has no sys/ and proc/ directories; snapshots are taken on Linux",
            source.root().display()
        );
    }
    if out.exists()
        && fs::read_dir(out)
            .with_context(|| format!("reading {}", out.display()))?
            .next()
            .is_some()
    {
        bail!("{} exists and is not empty", out.display());
    }
    fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;

    let mut mirror = Mirror {
        src: source.root().to_path_buf(),
        out: out.to_path_buf(),
        report: SnapshotReport::default(),
        scrubber: Scrubber::default(),
        made: HashSet::new(),
        pci_ids: BTreeSet::new(),
    };
    mirror.capture_all(source);
    mirror.write_pci_ids(source)?;
    let report = mirror.report;
    write_manifest(source, out, &report)?;
    Ok(report)
}

struct Mirror {
    src: PathBuf,
    out: PathBuf,
    report: SnapshotReport,
    scrubber: Scrubber,
    /// Output paths already written, files and links alike.
    made: HashSet<PathBuf>,
    /// `(vendor, device)` of every PCI device copied, for the pci.ids excerpt.
    pci_ids: BTreeSet<(u32, u32)>,
}

impl Mirror {
    fn capture_all(&mut self, source: &SysRoot) {
        for file in ["stat", "meminfo", "diskstats", "cpuinfo"] {
            self.copy(&Path::new("proc").join(file));
        }

        // hwmon: every attribute of every chip, and the device link the ids come from.
        for name in list_dir(&source.sys("class/hwmon")) {
            let chip = PathBuf::from("sys/class/hwmon").join(&name);
            self.copy_files(&chip, |f| f != "uevent");
            if self.walk(&chip.join("device"), true, 0).is_some()
                && !source.sys("class/hwmon").join(&name).join("name").exists()
            {
                // Drivers with the pre-3.x layout keep their attributes on the device.
                self.copy_files(&chip.join("device"), |f| f != "uevent" && f != "modalias");
            }
        }

        // CPU topology, clocks, idle states and throttling counters.
        let cpu = PathBuf::from("sys/devices/system/cpu");
        for file in ["online", "possible", "present", "kernel_max"] {
            self.copy(&cpu.join(file));
        }
        for name in list_dir(&source.sys("devices/system/cpu")) {
            let is_cpu = name
                .strip_prefix("cpu")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
            if !is_cpu {
                continue;
            }
            let dir = cpu.join(&name);
            self.copy(&dir.join("online"));
            self.copy_files(&dir.join("topology"), |_| true);
            self.copy_files(&dir.join("cpufreq"), |f| CPUFREQ_FILES.contains(&f));
            self.copy_files(&dir.join("thermal_throttle"), |_| true);
            for state in list_dir(&source.path(dir.join("cpuidle"))) {
                if state.starts_with("state") {
                    self.copy_files(&dir.join("cpuidle").join(state), |f| {
                        CPUIDLE_FILES.contains(&f)
                    });
                }
            }
        }

        // GPUs: clocks and PCI identity.
        for name in list_dir(&source.sys("class/drm")) {
            let is_card = name
                .strip_prefix("card")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
            if !is_card {
                continue;
            }
            let card = PathBuf::from("sys/class/drm").join(&name);
            self.copy_files(&card, |f| {
                (f.starts_with("gt_") && f.ends_with("_freq_mhz")) || f == "dev"
            });
            for gt in list_dir(&source.path(card.join("gt"))) {
                self.copy_files(&card.join("gt").join(gt), |f| {
                    f.starts_with("rps_") || f.starts_with("throttle_reason") || f == "id"
                });
            }
            self.copy_pci_device(source, &card.join("device"));
        }

        // Disks: the physical ones only (loop and zram devices can number in dozens).
        for name in list_dir(&source.sys("block")) {
            let disk = PathBuf::from("sys/block").join(&name);
            let resolved = source.resolve(&source.path(&disk));
            let virtual_disk = resolved.is_some_and(|p| {
                let p = p.to_string_lossy().into_owned();
                p.contains("/devices/virtual/") && !p.contains("/nvme-subsystem/")
            });
            if virtual_disk {
                continue;
            }
            self.copy_files(&disk, |f| BLOCK_FILES.contains(&f));
            self.copy_files(&disk.join("queue"), |f| QUEUE_FILES.contains(&f));
            self.copy_files(&disk.join("device"), |f| DISK_DEVICE_FILES.contains(&f));
        }
        for name in list_dir(&source.sys("class/nvme")) {
            let controller = PathBuf::from("sys/class/nvme").join(&name);
            self.copy_files(&controller, |f| NVME_FILES.contains(&f));
            self.copy_pci_device(source, &controller.join("device"));
        }

        // Network interfaces; virtual ones other than loopback are left out.
        for name in list_dir(&source.sys("class/net")) {
            let iface = PathBuf::from("sys/class/net").join(&name);
            let is_virtual = source
                .resolve(&source.path(&iface))
                .is_some_and(|p| p.to_string_lossy().contains("/devices/virtual/"));
            if is_virtual && name != "lo" {
                continue;
            }
            if name.starts_with("enx") || name.starts_with("wlx") {
                self.report.warnings.push(format!(
                    "interface {name} is named after its MAC address; rename it in the \
                     snapshot before sharing"
                ));
            }
            self.copy_files(&iface, |f| NET_FILES.contains(&f));
            self.copy_files(&iface.join("statistics"), |f| NET_STAT_FILES.contains(&f));
            self.copy_pci_device(source, &iface.join("device"));
        }

        // Batteries, UPSes and adapters. `uevent` repeats the serial number.
        for name in list_dir(&source.sys("class/power_supply")) {
            let supply = PathBuf::from("sys/class/power_supply").join(&name);
            self.copy_files(&supply, |f| f != "uevent");
            self.walk(&supply.join("device"), true, 0);
        }

        let dmi = PathBuf::from("sys/class/dmi/id");
        for file in DMI_FILES {
            self.copy(&dmi.join(file));
        }
    }

    /// Walks `rel` component by component, recreating directories and symlinks in the
    /// output, and returns the symlink-free path it leads to in the source. With
    /// `follow_last` false a final symlink is recreated but not followed.
    fn walk(&mut self, rel: &Path, follow_last: bool, depth: u32) -> Option<PathBuf> {
        if depth > 32 {
            return None;
        }
        let parts: Vec<&std::ffi::OsStr> = rel
            .components()
            .filter_map(|c| match c {
                Component::Normal(part) => Some(part),
                _ => None,
            })
            .collect();
        let mut real = PathBuf::new();
        for (i, part) in parts.iter().enumerate() {
            let candidate = real.join(part);
            let meta = fs::symlink_metadata(self.src.join(&candidate)).ok()?;
            if meta.file_type().is_symlink() {
                let target = fs::read_link(self.src.join(&candidate)).ok()?;
                self.make_link(&candidate, &target);
                if i + 1 == parts.len() && !follow_last {
                    return Some(candidate);
                }
                // `real` has no symlinks in it, so `..` in the target resolves lexically.
                let resolved = match target.strip_prefix("/") {
                    Ok(absolute) => lexical_join(Path::new(""), absolute),
                    Err(_) => lexical_join(&real, &target),
                };
                let rest: PathBuf = parts[i + 1..].iter().collect();
                return self.walk(&resolved.join(rest), follow_last, depth + 1);
            }
            if meta.is_dir() && fs::create_dir_all(self.out.join(&candidate)).is_err() {
                return None;
            }
            real = candidate;
        }
        Some(real)
    }

    fn make_link(&mut self, rel: &Path, target: &Path) {
        let dest = self.out.join(rel);
        if !self.made.insert(dest.clone()) {
            return;
        }
        if let Some(parent) = dest.parent() {
            let _ = fs::create_dir_all(parent);
        }
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(target, &dest);
        #[cfg(not(unix))]
        let made: std::io::Result<()> = Err(std::io::Error::other("symlinks need unix"));
        match made {
            Ok(()) => self.report.links += 1,
            Err(err) => self
                .report
                .warnings
                .push(format!("could not recreate link {}: {err}", rel.display())),
        }
    }

    /// Copies one file, scrubbing it if it carries an identifier.
    fn copy(&mut self, rel: &Path) {
        let Some(real) = self.walk(rel, true, 0) else {
            return;
        };
        let dest = self.out.join(&real);
        if !self.made.insert(dest.clone()) {
            return;
        }
        let shown = real.display().to_string();
        let data = match fs::read(self.src.join(&real)) {
            Ok(data) => data,
            Err(err) => {
                self.report.unreadable.push((shown, err.to_string()));
                return;
            }
        };
        let data = match self.scrubber.scrub(&real, data) {
            Scrubbed::Kept(data) => data,
            Scrubbed::Changed(data) => {
                self.report.scrubbed.push(shown);
                data
            }
            Scrubbed::Dropped => return,
        };
        if let Some(parent) = dest.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match fs::write(&dest, data) {
            Ok(()) => self.report.files += 1,
            Err(err) => self
                .report
                .warnings
                .push(format!("could not write {}: {err}", dest.display())),
        }
    }

    /// Copies the regular files of a directory that `keep` accepts.
    fn copy_files(&mut self, rel_dir: &Path, keep: impl Fn(&str) -> bool) {
        let Some(real) = self.walk(rel_dir, true, 0) else {
            return;
        };
        let Ok(entries) = fs::read_dir(self.src.join(&real)) else {
            return;
        };
        let mut names: Vec<String> = entries
            .filter_map(|e| {
                let e = e.ok()?;
                e.file_type().ok()?.is_file().then_some(())?;
                e.file_name().into_string().ok()
            })
            .filter(|name| keep(name))
            .collect();
        names.sort();
        for name in names {
            self.copy(&real.join(name));
        }
    }

    /// Follows a `device` link and, when it leads to a PCI device, copies the device's
    /// ids and its `driver` link.
    fn copy_pci_device(&mut self, source: &SysRoot, rel_link: &Path) {
        let Some(real) = self.walk(rel_link, true, 0) else {
            return;
        };
        let is_pci = real
            .file_name()
            .is_some_and(|n| is_pci_address(&n.to_string_lossy()));
        if !is_pci {
            return;
        }
        for file in PCI_FILES {
            self.copy(&real.join(file));
        }
        self.walk(&real.join("driver"), false, 0);
        let id =
            |file: &str| read_string(&source.path(real.join(file))).and_then(|v| parse_hex(&v));
        if let (Some(vendor), Some(device)) = (id("vendor"), id("device")) {
            self.pci_ids.insert((vendor, device));
        }
    }

    /// Writes the pci.ids lines for the copied PCI devices, so that GPU names resolve
    /// in replay without the whole database.
    fn write_pci_ids(&mut self, source: &SysRoot) -> anyhow::Result<()> {
        if self.pci_ids.is_empty() {
            return Ok(());
        }
        let Some(file) = PCI_IDS_PATHS
            .iter()
            .find_map(|rel| fs::File::open(source.path(rel)).ok())
        else {
            self.report
                .warnings
                .push("no pci.ids found; GPU names will show PCI ids in replay".into());
            return Ok(());
        };
        let vendors: BTreeSet<u32> = self.pci_ids.iter().map(|(v, _)| *v).collect();
        let mut text = String::from(
            "# Excerpt of pci.ids (GPL-2.0-or-later OR BSD-3-Clause) written by kwinfo-cli snapshot\n",
        );
        let mut vendor: Option<u32> = None;
        for line in BufReader::new(file).split(b'\n') {
            let Ok(line) = line else { break };
            let line = String::from_utf8_lossy(&line).into_owned();
            if line.starts_with("C ") {
                break;
            }
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix('\t') {
                let Some(v) = vendor else { continue };
                if rest.starts_with('\t') {
                    continue;
                }
                let device = rest.get(..4).and_then(|d| u32::from_str_radix(d, 16).ok());
                if device.is_some_and(|d| self.pci_ids.contains(&(v, d))) {
                    text.push_str(&line);
                    text.push('\n');
                }
            } else {
                vendor = line
                    .get(..4)
                    .and_then(|v| u32::from_str_radix(v, 16).ok())
                    .filter(|v| vendors.contains(v));
                if vendor.is_some() {
                    text.push_str(&line);
                    text.push('\n');
                }
            }
        }
        let dest = self.out.join(PCI_IDS_PATHS[0]);
        fs::create_dir_all(dest.parent().expect("pci.ids path has a parent"))?;
        fs::write(&dest, text)?;
        self.report.files += 1;
        Ok(())
    }
}

enum Scrubbed {
    Kept(Vec<u8>),
    Changed(Vec<u8>),
    Dropped,
}

/// Replaces identifiers with placeholders, consistently across files.
#[derive(Default)]
struct Scrubber {
    placeholders: HashMap<String, String>,
}

impl Scrubber {
    fn placeholder(&mut self, original: &str, make: impl FnOnce(usize) -> String) -> String {
        let next = self.placeholders.len() + 1;
        self.placeholders
            .entry(original.to_string())
            .or_insert_with(|| make(next))
            .clone()
    }

    fn scrub(&mut self, rel: &Path, data: Vec<u8>) -> Scrubbed {
        let name = rel
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let path = rel.to_string_lossy();
        let text = || String::from_utf8_lossy(&data).trim().to_string();
        match name.as_str() {
            "serial" | "serial_number" => {
                let value = text();
                if value.is_empty() {
                    return Scrubbed::Kept(data);
                }
                let replaced = self.placeholder(&value, |n| format!("SCRUBBED{n:04}"));
                Scrubbed::Changed(format!("{replaced}\n").into_bytes())
            }
            "wwid" | "eui" | "nguid" | "uuid" | "subsysnqn" => {
                let value = text();
                if value.is_empty() {
                    return Scrubbed::Kept(data);
                }
                // Keep the designator type (`eui.`, `naa.`, `nvme.`, `t10.`).
                let prefix = value
                    .split_once('.')
                    .filter(|(p, _)| p.len() <= 4)
                    .map_or(String::new(), |(p, _)| format!("{p}."));
                let replaced = self.placeholder(&value, |n| format!("{prefix}scrubbed{n:04}"));
                Scrubbed::Changed(format!("{replaced}\n").into_bytes())
            }
            "address" if path.contains("/net/") => {
                let value = text();
                if value.is_empty() || value.bytes().all(|b| b == b'0' || b == b':') {
                    return Scrubbed::Kept(data);
                }
                // Locally administered unicast addresses, numbered.
                let replaced = self.placeholder(&value, |n| {
                    format!("02:00:00:00:{:02x}:{:02x}", (n >> 8) & 0xff, n & 0xff)
                });
                Scrubbed::Changed(format!("{replaced}\n").into_bytes())
            }
            "vpd_pg80" | "vpd_pg83" => Scrubbed::Dropped,
            "vpd_pg89" => {
                let mut data = data;
                let identify = VPD89_IDENTIFY;
                if data.len() >= identify + 224 {
                    // IDENTIFY words 10–19 (serial number) and 108–111 (WWN).
                    data[identify + 20..identify + 40].fill(b' ');
                    data[identify + 216..identify + 224].fill(0);
                    Scrubbed::Changed(data)
                } else {
                    Scrubbed::Dropped
                }
            }
            "cpuinfo" => {
                let text = String::from_utf8_lossy(&data);
                if !text.lines().any(|l| l.starts_with("Serial")) {
                    return Scrubbed::Kept(data);
                }
                let kept: Vec<&str> = text.lines().filter(|l| !l.starts_with("Serial")).collect();
                Scrubbed::Changed((kept.join("\n") + "\n").into_bytes())
            }
            _ => Scrubbed::Kept(data),
        }
    }
}

fn write_manifest(source: &SysRoot, out: &Path, report: &SnapshotReport) -> anyhow::Result<()> {
    let captured = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let kernel =
        read_string(&source.proc("sys/kernel/osrelease")).unwrap_or_else(|| "unknown".into());
    let mut text = format!(
        "KWinfo sensor snapshot\n\
         kwinfo version: {}\n\
         captured (unix time): {captured}\n\
         kernel: {kernel}\n\
         files: {}, symlinks: {}\n",
        env!("CARGO_PKG_VERSION"),
        report.files,
        report.links,
    );
    text.push_str("\nScrubbed (identifiers replaced by placeholders):\n");
    for path in &report.scrubbed {
        text.push_str(&format!("  {path}\n"));
    }
    text.push_str("\nUnreadable (left out):\n");
    for (path, err) in &report.unreadable {
        text.push_str(&format!("  {path}: {err}\n"));
    }
    if !report.warnings.is_empty() {
        text.push_str("\nWarnings:\n");
        for warning in &report.warnings {
            text.push_str(&format!("  {warning}\n"));
        }
    }
    fs::write(out.join(MANIFEST), text).with_context(|| format!("writing {MANIFEST}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::linux_backends;
    use crate::backends::test_support::fixture_root;

    fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                files_under(&entry.path(), out);
            } else if kind.is_file() {
                out.push(entry.path());
            }
        }
    }

    fn snapshot_of_fixture() -> (tempfile::TempDir, PathBuf, SnapshotReport) {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("snapshot");
        let report = capture(&fixture_root(), &dest).expect("capture succeeds");
        (dir, dest, report)
    }

    #[test]
    fn identifiers_are_scrubbed() {
        let (_dir, dest, report) = snapshot_of_fixture();
        let mut files = Vec::new();
        files_under(&dest, &mut files);
        let secrets: [&[u8]; 7] = [
            b"S4EWNX0R123456A",
            b"S3Z1NB0K123456X",
            b"a8:5e:45:12:34:56",
            b"02:42:ac:11:00:01",
            b"0025385b71b02c5f",
            b"5002538e40e1d0e9",
            &[0x50, 0x02, 0x53, 0x8e, 0x40, 0xe1, 0xd0, 0xe9],
        ];
        for file in &files {
            let data = fs::read(file).unwrap();
            for secret in secrets {
                assert!(
                    !data.windows(secret.len()).any(|w| w == secret),
                    "{} still contains {}",
                    file.display(),
                    String::from_utf8_lossy(secret)
                );
            }
        }
        let read = |rel: &str| fs::read_to_string(dest.join(rel)).unwrap();
        let battery = read("sys/class/power_supply/BAT0/serial_number");
        let nvme = read("sys/class/nvme/nvme0/serial");
        assert!(battery.starts_with("SCRUBBED") && nvme.starts_with("SCRUBBED"));
        assert_ne!(
            battery, nvme,
            "different originals get different placeholders"
        );
        assert!(read("sys/block/nvme0n1/wwid").starts_with("eui.scrubbed"));
        assert!(read("sys/block/sda/device/wwid").starts_with("naa.scrubbed"));
        assert!(read("sys/class/net/enp0s31f6/address").starts_with("02:00:00:00:"));
        assert_eq!(read("sys/class/net/lo/address"), "00:00:00:00:00:00\n");

        // The battery's uevent repeats the serial; VPD pages 0x80/0x83 are nothing else.
        let gone = |rel: &str| fs::symlink_metadata(dest.join(rel)).is_err();
        assert!(gone("sys/class/power_supply/BAT0/uevent"));
        assert!(gone("sys/block/sda/device/vpd_pg80"));
        assert!(gone("sys/block/sda/device/vpd_pg83"));
        // Virtual block devices and network interfaces other than lo are left out.
        assert!(gone("sys/block/loop0"));
        assert!(gone("sys/class/net/docker0"));
        assert!(!gone("sys/class/net/lo"));
        // DMI serials are never copied (the fixture has none, but the allow-list says so).
        assert!(
            !DMI_FILES
                .iter()
                .any(|f| f.contains("serial") || f.contains("uuid"))
        );

        assert!(report.files > 900, "copied {} files", report.files);
        // The fixture's 46 links, minus loop0, docker0, the DP connector and render node
        // (and their `device` links) and the AHCI controller's `driver` link.
        assert_eq!(report.links, 39);
        assert!(report.unreadable.is_empty(), "{:?}", report.unreadable);
        let manifest = read(MANIFEST);
        assert!(manifest.contains("nvme/nvme0/serial"), "{manifest}");
        assert!(manifest.contains("vpd_pg89"), "{manifest}");
    }

    #[test]
    fn ata_identify_data_keeps_the_model_but_not_the_serial() {
        let (_dir, dest, _) = snapshot_of_fixture();
        let page = fs::read(dest.join("sys/block/sda/device/vpd_pg89")).unwrap();
        assert_eq!(
            crate::backends::block::ata_model_from_vpd89(&page).as_deref(),
            Some("Samsung SSD 860 EVO 500GB")
        );
        let identify = &page[VPD89_IDENTIFY..];
        assert!(
            identify[20..40].iter().all(|&b| b == b' '),
            "serial blanked"
        );
        assert!(identify[216..224].iter().all(|&b| b == 0), "WWN zeroed");
    }

    /// Everything the backends show, minus drive ids (which hash the scrubbed WWIDs).
    fn listing(root: &SysRoot) -> Vec<String> {
        let mut lines = Vec::new();
        for mut backend in linux_backends(root, None) {
            let devices = backend.enumerate().unwrap();
            let mut samples = Vec::new();
            backend.sample(&mut samples).unwrap();
            for (device, channels) in devices {
                for channel in channels {
                    let value = samples
                        .iter()
                        .find(|s| s.channel == channel.id)
                        .and_then(|s| s.value);
                    lines.push(format!(
                        "{} | {} | {} | {:?} | {:?} | {:?}",
                        device.label,
                        channel.id.key(),
                        channel.label,
                        channel.kind,
                        channel.unit,
                        value
                    ));
                }
            }
        }
        lines
    }

    #[test]
    fn a_snapshot_replays_like_the_original() {
        let (_dir, dest, _) = snapshot_of_fixture();
        let original = listing(&fixture_root());
        assert_eq!(original.len(), 137);
        assert_eq!(listing(&SysRoot::new(&dest)), original);
    }

    #[test]
    fn refuses_to_overwrite_or_to_run_without_sys() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("keep.txt"), "mine").unwrap();
        let err = capture(&fixture_root(), dir.path()).unwrap_err();
        assert!(err.to_string().contains("not empty"), "{err}");

        let empty = tempfile::tempdir().unwrap();
        let err = capture(&SysRoot::new(empty.path()), &dir.path().join("x")).unwrap_err();
        assert!(err.to_string().contains("no sys/"), "{err}");
    }
}

//! The `block` backend: activity, read and write rates and since-boot totals of every
//! physical disk, in a "Drive: <model>" group.
//!
//! Files read: `/sys/block/*` to find whole physical disks (`size`, the resolved path,
//! which is under `devices/virtual` for loop, zram, device-mapper and md devices),
//! `/sys/block/<disk>/wwid`, `/sys/block/<disk>/device/{model,vendor,wwid,vpd_pg89}`,
//! `/sys/class/nvme/nvmeN/{model,serial}` for NVMe identity, and every tick
//! `/proc/diskstats` (or `/sys/block/<disk>/stat` for disks missing from it).
//!
//! Drive temperatures come from the `nvme` and `drivetemp` hwmon chips, which the
//! `hwmon` backend files under the same device using [`drives`]. S.M.A.R.T. data needs the
//! privileged helper.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::backend::Backend;
use crate::model::{Category, Channel, ChannelId, Device, Kind, Sample};
use crate::sysroot::{SysRoot, list_dir, read_into, read_string, read_u64, trim_attr};

use super::util::{Counter, short_hash, slug};

/// diskstats counts 512-byte sectors whatever the device's block size.
const SECTOR: f64 = 512.0;

/// A physical disk and the device its rows belong to.
#[derive(Debug)]
pub(crate) struct Drive {
    /// Kernel name, e.g. `nvme0n1` or `sda`.
    pub name: String,
    /// The hardware the disk belongs to, resolved: the NVMe controller or the SCSI
    /// device. Drive-temperature hwmon chips hang off the same directory.
    pub hw_path: Option<PathBuf>,
    pub device: Device,
}

/// Every whole physical disk, in natural name order.
pub(crate) fn drives(root: &SysRoot) -> Vec<Drive> {
    struct Found {
        name: String,
        hw_path: Option<PathBuf>,
        model: Option<String>,
        identity: Option<String>,
    }
    let mut found = Vec::new();
    let block = root.sys("block");
    for name in list_dir(&block) {
        let Some(dir) = root.resolve(&block.join(&name)) else {
            continue;
        };
        if !is_physical_disk(&name, &dir) {
            continue;
        }
        let device_dir = root.resolve(&dir.join("device"));
        let (hw_path, model, identity) = if name.starts_with("nvme") {
            let controller = nvme_controller(root, &name, device_dir.as_deref());
            let model = controller
                .as_deref()
                .and_then(|c| read_string(&c.join("model")));
            let identity = read_string(&dir.join("wwid")).or_else(|| {
                let serial = read_string(&controller.as_deref()?.join("serial"))?;
                let nsid = read_string(&dir.join("nsid")).unwrap_or_else(|| "1".into());
                Some(format!("{serial}:{nsid}"))
            });
            (controller, model, identity)
        } else {
            let model = device_dir.as_deref().and_then(scsi_model);
            let identity = device_dir.as_deref().and_then(|d| {
                read_string(&d.join("wwid"))
                    .or_else(|| read_string(&d.join("serial")))
                    .or_else(|| vpd_serial(&d.join("vpd_pg80")))
            });
            let identity = identity.or_else(|| read_string(&dir.join("serial")));
            (device_dir, model, identity)
        };
        found.push(Found {
            name,
            hw_path,
            model,
            identity,
        });
    }

    let label = |f: &Found| match &f.model {
        Some(model) => format!("Drive: {model}"),
        None => format!("Drive: {}", f.name),
    };
    let labels: Vec<String> = found.iter().map(label).collect();
    found
        .iter()
        .enumerate()
        .map(|(index, f)| {
            let id = match (&f.model, &f.identity) {
                (Some(model), Some(identity)) => {
                    format!("block:{}:{}", slug(model), short_hash(identity))
                }
                (Some(model), None) => format!("block:{}:{}", slug(model), f.name),
                (None, _) => format!("block:{}", f.name),
            };
            // Two identical drives get their kernel names to tell them apart.
            let mut label = labels[index].clone();
            if labels.iter().filter(|l| **l == label).count() > 1 {
                label = format!("{label} [{}]", f.name);
            }
            Drive {
                name: f.name.clone(),
                hw_path: f.hw_path.clone(),
                device: Device::new(id, label, Category::Drive)
                    .with_order(u16::try_from(index).unwrap_or(u16::MAX)),
            }
        })
        .collect()
}

/// Whole disks backed by hardware: not partitions (which `/sys/block` does not list),
/// not loop/zram/dm/md devices (under `devices/virtual`), not empty card readers or
/// optical drives, not the hidden per-path nodes of NVMe multipath.
fn is_physical_disk(name: &str, dir: &Path) -> bool {
    let path = dir.to_string_lossy();
    if path.contains("/devices/virtual/") && !path.contains("/nvme-subsystem/") {
        return false;
    }
    if name.starts_with("sr") || name.starts_with("fd") || is_nvme_path_node(name) {
        return false;
    }
    read_u64(&dir.join("size")).unwrap_or(0) > 0
}

/// `nvme0c1n1`: one path of a multipath namespace, hidden from users.
fn is_nvme_path_node(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("nvme") else {
        return false;
    };
    let digits = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    digits.starts_with('c')
}

/// The NVMe controller of namespace `name`: the disk's `device` link when it points at a
/// controller, else `/sys/class/nvme/nvmeX` from the name (multipath head disks hang off
/// the subsystem instead).
fn nvme_controller(root: &SysRoot, name: &str, device_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = device_dir {
        let base = dir.file_name()?.to_string_lossy();
        if base.starts_with("nvme") && !base.starts_with("nvme-subsys") {
            return Some(dir.to_path_buf());
        }
    }
    let rest = name.strip_prefix("nvme")?;
    let number: String = rest.chars().take_while(char::is_ascii_digit).collect();
    root.resolve(&root.sys(format!("class/nvme/nvme{number}")))
}

/// The model of a SCSI/ATA disk: the full ATA string from VPD page 0x89 when the kernel
/// provides it (the SCSI `model` attribute stops at 16 characters), else `model`
/// prefixed with a meaningful `vendor`.
fn scsi_model(device_dir: &Path) -> Option<String> {
    if let Some(model) = std::fs::read(device_dir.join("vpd_pg89"))
        .ok()
        .and_then(|page| ata_model_from_vpd89(&page))
    {
        return Some(model);
    }
    let model =
        read_string(&device_dir.join("model")).or_else(|| read_string(&device_dir.join("name")))?;
    match read_string(&device_dir.join("vendor")) {
        Some(vendor) if vendor != "ATA" && !model.starts_with(&vendor) => {
            Some(format!("{vendor} {model}"))
        }
        _ => Some(model),
    }
}

/// Offset of the 512-byte IDENTIFY DEVICE block in the ATA Information VPD page: a
/// 4-byte header plus 56 bytes of SAT layer information.
pub(crate) const VPD89_IDENTIFY: usize = 60;

/// The full model from an ATA Information VPD page (0x89): IDENTIFY words 27–46.
pub(crate) fn ata_model_from_vpd89(page: &[u8]) -> Option<String> {
    if page.len() < VPD89_IDENTIFY + 94 || page[1] != 0x89 {
        return None;
    }
    ata_string(&page[VPD89_IDENTIFY + 54..VPD89_IDENTIFY + 94])
}

/// ATA strings store two characters per little-endian word, first character high.
fn ata_string(raw: &[u8]) -> Option<String> {
    let swapped: Vec<u8> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .flat_map(|&[a, b]| [b, a])
        .collect();
    let text = String::from_utf8_lossy(&swapped);
    let text = trim_attr(&text);
    (!text.is_empty()).then(|| text.to_string())
}

/// The serial number from a Unit Serial Number VPD page (0x80).
fn vpd_serial(path: &Path) -> Option<String> {
    let page = std::fs::read(path).ok()?;
    if page.len() < 4 || page[1] != 0x80 {
        return None;
    }
    let len = usize::from(page[3]).min(page.len() - 4);
    let text = String::from_utf8_lossy(&page[4..4 + len]);
    let text = trim_attr(&text);
    (!text.is_empty()).then(|| text.to_string())
}

/// The counters of one disk that the rows are computed from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DiskStats {
    pub sectors_read: u64,
    pub sectors_written: u64,
    /// Milliseconds during which the disk had I/O in flight (`io_ticks`).
    pub io_ms: u64,
}

impl DiskStats {
    /// Parses the counter fields as they appear in `/sys/block/*/stat`, or in a
    /// `/proc/diskstats` line after major, minor and name.
    pub fn parse<'a>(mut fields: impl Iterator<Item = &'a str>) -> Option<Self> {
        let mut values = [0u64; 10];
        for value in &mut values {
            *value = fields.next()?.parse().ok()?;
        }
        Some(Self {
            sectors_read: values[2],
            sectors_written: values[6],
            io_ms: values[9],
        })
    }
}

#[derive(Debug, Default)]
struct DiskCounters {
    read: Counter,
    written: Counter,
    busy: Counter,
}

#[derive(Debug)]
struct DiskPlan {
    name: String,
    activity: ChannelId,
    read_rate: ChannelId,
    write_rate: ChannelId,
    read_total: ChannelId,
    write_total: ChannelId,
}

/// See the module documentation.
pub struct BlockBackend {
    root: SysRoot,
    disks: Vec<DiskPlan>,
    counters: HashMap<String, DiskCounters>,
    buf: Vec<u8>,
}

impl BlockBackend {
    pub fn new(root: SysRoot) -> Self {
        Self {
            root,
            disks: Vec::new(),
            counters: HashMap::new(),
            buf: Vec::with_capacity(4096),
        }
    }

    /// One sampling pass at an explicit time; [`Backend::sample`] passes `Instant::now()`.
    pub fn sample_at(&mut self, now: Instant, out: &mut Vec<Sample>) {
        let mut stats: HashMap<&str, DiskStats> = HashMap::new();
        let have_diskstats = read_into(&self.root.proc("diskstats"), &mut self.buf).is_ok();
        let text = if have_diskstats {
            String::from_utf8_lossy(&self.buf).into_owned()
        } else {
            String::new()
        };
        for line in text.lines() {
            let mut fields = line.split_ascii_whitespace();
            let (Some(_major), Some(_minor), Some(name)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            if let Some(disk) = self.disks.iter().find(|d| d.name == name)
                && let Some(parsed) = DiskStats::parse(fields)
            {
                stats.insert(disk.name.as_str(), parsed);
            }
        }

        for disk in &self.disks {
            let current = stats.get(disk.name.as_str()).copied().or_else(|| {
                let text = read_string(&self.root.sys("block").join(&disk.name).join("stat"))?;
                DiskStats::parse(text.split_ascii_whitespace())
            });
            let counters = self.counters.entry(disk.name.clone()).or_default();
            let (activity, read_rate, write_rate) = match current {
                Some(s) => (
                    counters
                        .busy
                        .rate(s.io_ms, now)
                        .map(|ms_per_s| (ms_per_s / 10.0).clamp(0.0, 100.0)),
                    counters.read.rate(s.sectors_read, now).map(|r| r * SECTOR),
                    counters
                        .written
                        .rate(s.sectors_written, now)
                        .map(|r| r * SECTOR),
                ),
                None => (None, None, None),
            };
            out.push(Sample::new(disk.activity.clone(), activity));
            out.push(Sample::new(disk.read_rate.clone(), read_rate));
            out.push(Sample::new(disk.write_rate.clone(), write_rate));
            out.push(Sample::new(
                disk.read_total.clone(),
                current.map(|s| s.sectors_read as f64 * SECTOR),
            ));
            out.push(Sample::new(
                disk.write_total.clone(),
                current.map(|s| s.sectors_written as f64 * SECTOR),
            ));
        }
    }
}

impl Backend for BlockBackend {
    fn name(&self) -> &'static str {
        "block"
    }

    fn enumerate(&mut self) -> anyhow::Result<Vec<(Device, Vec<Channel>)>> {
        self.disks.clear();
        let mut result = Vec::new();
        for drive in drives(&self.root) {
            let id = drive.device.id.clone();
            let channels = vec![
                Channel::new(&id, "activity", "Total Activity", Kind::Usage),
                Channel::new(&id, "read_rate", "Read Rate", Kind::Throughput),
                Channel::new(&id, "write_rate", "Write Rate", Kind::Throughput),
                Channel::new(&id, "read_total", "Read Total", Kind::Capacity),
                Channel::new(&id, "write_total", "Write Total", Kind::Capacity),
            ];
            self.disks.push(DiskPlan {
                name: drive.name,
                activity: channels[0].id.clone(),
                read_rate: channels[1].id.clone(),
                write_rate: channels[2].id.clone(),
                read_total: channels[3].id.clone(),
                write_total: channels[4].id.clone(),
            });
            result.push((drive.device, channels));
        }
        Ok(result)
    }

    fn sample(&mut self, out: &mut Vec<Sample>) -> anyhow::Result<()> {
        self.sample_at(Instant::now(), out);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::backends::test_support::*;
    use crate::model::Unit;

    #[test]
    fn finds_physical_disks_with_full_models() {
        let drives = drives(&fixture_root());
        let names: Vec<&str> = drives.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["nvme0n1", "sda"], "loop devices are skipped");

        let nvme = &drives[0];
        assert_eq!(nvme.device.label, "Drive: Samsung SSD 970 EVO Plus 1TB");
        assert_eq!(nvme.device.category, Category::Drive);
        // The id carries a hash of the namespace's WWID, never the serial itself.
        assert_eq!(
            nvme.device.id.as_str(),
            format!(
                "block:samsung-ssd-970-evo-plus-1tb:{}",
                short_hash("eui.0025385b71b02c5f")
            )
        );
        assert!(nvme.hw_path.as_ref().unwrap().ends_with("nvme/nvme0"));

        // The SCSI model attribute says "Samsung SSD 860 "; VPD page 0x89 has it all.
        let sata = &drives[1];
        assert_eq!(sata.device.label, "Drive: Samsung SSD 860 EVO 500GB");
        assert_eq!(
            sata.device.id.as_str(),
            format!(
                "block:samsung-ssd-860-evo-500gb:{}",
                short_hash("naa.5002538e40e1d0e9")
            )
        );
        assert!(sata.hw_path.as_ref().unwrap().ends_with("0:0:0:0"));
    }

    #[test]
    fn diskstats_fields_map_to_sectors_and_busy_time() {
        let stats = DiskStats::parse(
            "152345 1234 9876543 45678 98765 4321 7654321 87654 0 76543 133332 0 0 0 0"
                .split_ascii_whitespace(),
        )
        .unwrap();
        assert_eq!(
            stats,
            DiskStats {
                sectors_read: 9_876_543,
                sectors_written: 7_654_321,
                io_ms: 76_543,
            }
        );
        assert_eq!(DiskStats::parse("1 2 3".split_ascii_whitespace()), None);
    }

    #[test]
    fn rows_and_totals() {
        let mut backend = BlockBackend::new(fixture_root());
        let (devices, samples) = enumerate_and_sample(&mut backend);
        assert_eq!(devices.len(), 2);
        let (device, channels) = &devices[0];
        assert_eq!(
            keys_and_labels(channels),
            [
                ("activity", "Total Activity"),
                ("read_rate", "Read Rate"),
                ("write_rate", "Write Rate"),
                ("read_total", "Read Total"),
                ("write_total", "Write Total"),
            ]
        );
        assert_eq!(channel(channels, "read_rate").unit, Unit::BytesPerSecond);
        assert_eq!(channel(channels, "write_total").unit, Unit::Bytes);
        let values = sample_map(&samples);
        let get = |key: &str| values[&ChannelId::new(&device.id, key)];
        assert_close(get("read_total"), 9_876_543.0 * 512.0);
        assert_close(get("write_total"), 7_654_321.0 * 512.0);
        assert_eq!(get("read_rate"), None, "rates need two readings");
    }

    #[test]
    fn rates_and_activity_follow_diskstats() {
        let (_dir, root) = fixture_copy();
        let mut backend = BlockBackend::new(root.clone());
        let devices = backend.enumerate().unwrap();
        let (t0, t1) = instants(Duration::from_secs(2));
        backend.sample_at(t0, &mut Vec::new());

        // Two seconds later nvme0n1 read 8 MiB, wrote 2 MiB and was busy 500 ms.
        let diskstats = std::fs::read_to_string(root.proc("diskstats")).unwrap();
        let bumped: Vec<String> = diskstats
            .lines()
            .map(|line| {
                let mut f: Vec<String> = line.split_whitespace().map(str::to_string).collect();
                if f[2] == "nvme0n1" {
                    let add = |v: &str, n: u64| (v.parse::<u64>().unwrap() + n).to_string();
                    f[5] = add(&f[5], 16_384);
                    f[9] = add(&f[9], 4_096);
                    f[12] = add(&f[12], 500);
                }
                f.join(" ")
            })
            .collect();
        write(&root, "proc/diskstats", &(bumped.join("\n") + "\n"));

        let mut samples = Vec::new();
        backend.sample_at(t1, &mut samples);
        let values = sample_map(&samples);
        let nvme = &devices[0].0.id;
        let get = |key: &str| values[&ChannelId::new(nvme, key)];
        assert_close(get("read_rate"), 4.0 * 1024.0 * 1024.0);
        assert_close(get("write_rate"), 1024.0 * 1024.0);
        assert_close(get("activity"), 25.0);
        let sda = &devices[1].0.id;
        assert_close(values[&ChannelId::new(sda, "read_rate")], 0.0);
    }

    #[test]
    fn disks_missing_from_diskstats_use_their_stat_file() {
        let (_dir, root) = fixture_copy();
        write(&root, "proc/diskstats", "");
        let mut backend = BlockBackend::new(root);
        let (devices, samples) = enumerate_and_sample(&mut backend);
        let values = sample_map(&samples);
        let sda = &devices[1].0.id;
        assert_close(
            values[&ChannelId::new(sda, "read_total")],
            1_876_543.0 * 512.0,
        );
    }
}

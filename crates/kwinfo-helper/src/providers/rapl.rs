//! CPU, GT, DRAM and platform power from the RAPL energy counters under
//! `/sys/class/powercap/intel-rapl:*` (Intel, and AMD Zen 2 and later through the same
//! driver). The counters are root-only since CVE-2020-8694; the helper turns them into
//! power averaged over at least [`MIN_WINDOW`] and never passes the raw counters on.
//!
//! Files read per domain directory `intel-rapl:<package>[:<subdomain>]`: `name`
//! (`package-0`, `core`, `uncore`, `dram`, `psys`), `energy_uj` and `max_energy_range_uj`.
//! The `intel-rapl-mmio:*` domains repeat the package domain and are skipped.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use kwinfo_core::backends::shared::{package_device, read_cpuinfo};
use kwinfo_core::sysroot::{list_dir, read_string, read_u64};
use kwinfo_core::{Channel, ChannelId, Device, Kind, Sample, Source, SysRoot};

use super::{Provider, Status};

/// Power is averaged over at least this long: shorter windows would be noisy, and
/// coarse power is what blunts the side channel that made the counters root-only.
pub const MIN_WINDOW: Duration = Duration::from_millis(250);
/// More than this is a counter glitch, not a reading.
const MAX_PLAUSIBLE_W: f64 = 5_000.0;

#[derive(Debug)]
struct Domain {
    channel: ChannelId,
    energy: PathBuf,
    /// The counter wraps to 0 after this many microjoules.
    range: u64,
    prev: Option<(u64, Instant)>,
    power: Option<f64>,
}

impl Domain {
    /// The power since the previous reading in watts, or the previous power when less
    /// than [`MIN_WINDOW`] has passed. `None` for the first reading.
    fn update(&mut self, energy: u64, now: Instant) -> Option<f64> {
        let Some((prev, then)) = self.prev else {
            self.prev = Some((energy, now));
            return None;
        };
        let dt = now.saturating_duration_since(then);
        if dt < MIN_WINDOW {
            return self.power;
        }
        let delta = if energy >= prev {
            energy - prev
        } else {
            self.range.saturating_sub(prev).saturating_add(energy)
        };
        self.prev = Some((energy, now));
        let watts = delta as f64 / 1e6 / dt.as_secs_f64();
        self.power = (watts.is_finite() && watts < MAX_PLAUSIBLE_W).then_some(watts);
        self.power
    }
}

/// The row of a RAPL domain: its channel key and label.
fn row(name: &str, intel: bool) -> (String, String) {
    let (key, label) = match name {
        n if n.starts_with("package") => ("package", "CPU Package Power"),
        "core" if intel => ("cores", "IA Cores Power"),
        "core" => ("cores", "CPU Cores Power"),
        "uncore" => ("uncore", "GT Cores Power"),
        "dram" => ("dram", "DRAM Power"),
        "psys" => ("psys", "Platform Power"),
        other => {
            let key: String = other
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            return (format!("rapl.{key}"), format!("{other} Power"));
        }
    };
    (format!("rapl.{key}"), label.to_string())
}

/// See the module documentation.
pub struct Rapl {
    root: SysRoot,
    domains: Vec<Domain>,
    status: Status,
}

impl Rapl {
    pub fn new(root: SysRoot) -> Self {
        Self {
            root,
            domains: Vec::new(),
            status: Status::missing("RAPL has not been read yet"),
        }
    }

    /// [`Provider::sample`] at an explicit time, for tests.
    pub fn sample_at(&mut self, now: Instant, out: &mut Vec<Sample>) {
        for domain in &mut self.domains {
            let value = read_u64(&domain.energy).and_then(|e| domain.update(e, now));
            out.push(Sample::new(domain.channel.clone(), value));
        }
    }
}

impl Provider for Rapl {
    fn name(&self) -> &'static str {
        "rapl"
    }

    fn enumerate(&mut self) -> Vec<(Device, Vec<Channel>)> {
        let class = self.root.sys("class/powercap");
        let intel = read_cpuinfo(&self.root).vendor.as_deref() == Some("GenuineIntel");
        let mut devices: BTreeMap<u32, (Device, Vec<Channel>)> = BTreeMap::new();
        let mut domains = Vec::new();
        let mut denied = false;
        for name in list_dir(&class) {
            // `intel-rapl` itself is the control type; `intel-rapl-mmio:*` repeats the
            // package domain through MMIO.
            let Some(rest) = name.strip_prefix("intel-rapl:") else {
                continue;
            };
            let Some(Ok(mut package)) = rest.split(':').next().map(str::parse::<u32>) else {
                continue;
            };
            let dir = class.join(&name);
            let Some(kind) = read_string(&dir.join("name")) else {
                continue;
            };
            if let Some(n) = kind.strip_prefix("package-").and_then(|n| n.parse().ok()) {
                package = n;
            }
            // The platform domain is not a package's; it joins the first package.
            if kind == "psys" {
                package = 0;
            }
            let energy = dir.join("energy_uj");
            match std::fs::read_to_string(&energy) {
                Ok(_) => {}
                Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
                    denied = true;
                    continue;
                }
                Err(_) => continue,
            }
            let (key, label) = row(&kind, intel);
            let (device, channels) = devices
                .entry(package)
                .or_insert_with(|| (package_device(&self.root, package), Vec::new()));
            if channels.iter().any(|c| c.id.key() == key) {
                continue;
            }
            let channel =
                Channel::new(&device.id, &key, label, Kind::Power).with_source(Source::Helper);
            domains.push(Domain {
                channel: channel.id.clone(),
                energy,
                range: read_u64(&dir.join("max_energy_range_uj")).unwrap_or(u64::MAX),
                prev: None,
                power: None,
            });
            channels.push(channel);
        }
        self.domains = domains;
        self.status = if !self.domains.is_empty() {
            Status::working()
        } else if denied {
            Status::missing("The RAPL energy counters are root-only; the helper must run as root.")
        } else if class.is_dir() {
            Status::missing(
                "This CPU exposes no RAPL energy counters (no intel-rapl domains in \
                 /sys/class/powercap; the intel_rapl_msr module may not be loaded).",
            )
        } else {
            Status::missing("This system has no /sys/class/powercap, so no CPU power readings.")
        };
        devices.into_values().collect()
    }

    fn sample(&mut self, now: Instant, out: &mut Vec<Sample>) {
        self.sample_at(now, out);
    }

    fn status(&self) -> Status {
        self.status.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn fixture() -> (tempfile::TempDir, SysRoot) {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/synthetic-8700k");
        let dir = tempfile::tempdir().unwrap();
        copy_tree(&source, dir.path());
        let root = SysRoot::new(dir.path());
        (dir, root)
    }

    pub(crate) fn copy_tree(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                copy_tree(&entry.path(), &target);
            } else if kind.is_symlink() {
                let link = fs::read_link(entry.path()).unwrap();
                std::os::unix::fs::symlink(link, &target).unwrap();
            } else {
                fs::copy(entry.path(), &target).unwrap();
            }
        }
    }

    fn set_energy(root: &SysRoot, domain: &str, uj: u64) {
        fs::write(
            root.sys(format!("class/powercap/{domain}/energy_uj")),
            format!("{uj}\n"),
        )
        .unwrap();
    }

    #[test]
    fn finds_the_8700k_domains_and_names_them_like_hwinfo() {
        let (_dir, root) = fixture();
        let mut rapl = Rapl::new(root);
        let devices = rapl.enumerate();
        assert_eq!(devices.len(), 1);
        let (device, channels) = &devices[0];
        assert_eq!(device.id.as_str(), "cpu:0");
        assert_eq!(device.label, "CPU [#0]: Intel Core i7-8700K");
        let rows: Vec<(&str, &str)> = channels
            .iter()
            .map(|c| (c.id.key(), c.label.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                ("rapl.package", "CPU Package Power"),
                ("rapl.cores", "IA Cores Power"),
                ("rapl.uncore", "GT Cores Power"),
                ("rapl.dram", "DRAM Power"),
            ],
            "the MMIO copy of the package domain is skipped"
        );
        assert!(
            channels
                .iter()
                .all(|c| c.source == Source::Helper && c.kind == Kind::Power)
        );
        assert_eq!(rapl.status(), Status::working());
    }

    #[test]
    fn power_is_the_energy_per_time_with_wrap_around() {
        let (_dir, root) = fixture();
        let mut rapl = Rapl::new(root.clone());
        rapl.enumerate();
        let t0 = Instant::now();
        let mut out = Vec::new();
        rapl.sample_at(t0, &mut out);
        assert!(
            out.iter().all(|s| s.value.is_none()),
            "the first reading has no power yet"
        );
        let watts = |out: &[Sample], key: &str| {
            out.iter()
                .find(|s| s.channel.key() == key)
                .and_then(|s| s.value)
                .unwrap()
        };

        // 35 J in 1 s on the package. The core counter wraps: it was 24 J below its
        // range a second ago and reads 4 J now, so 28 J went by.
        let package: u64 = fs::read_to_string(root.sys("class/powercap/intel-rapl:0/energy_uj"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        set_energy(&root, "intel-rapl:0", package + 35_000_000);
        let range: u64 = 262_143_328_850;
        rapl.domains[1].prev = Some((range - 24_000_000, t0));
        set_energy(&root, "intel-rapl:0:0", 4_000_000);
        out.clear();
        rapl.sample_at(t0 + Duration::from_secs(1), &mut out);
        assert!((watts(&out, "rapl.package") - 35.0).abs() < 1e-9);
        assert!(
            (watts(&out, "rapl.cores") - 28.0).abs() < 1e-9,
            "{}",
            watts(&out, "rapl.cores")
        );

        // Asked again within 250 ms: the same power, and the window keeps running.
        set_energy(&root, "intel-rapl:0", package + 36_000_000);
        out.clear();
        rapl.sample_at(t0 + Duration::from_millis(1100), &mut out);
        assert!((watts(&out, "rapl.package") - 35.0).abs() < 1e-9);
    }

    #[test]
    fn without_powercap_the_note_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let mut rapl = Rapl::new(SysRoot::new(dir.path()));
        assert!(rapl.enumerate().is_empty());
        let status = rapl.status();
        assert!(!status.available);
        assert!(status.note.unwrap().contains("powercap"));
    }

    #[test]
    fn labels_for_other_vendors_and_unknown_domains() {
        assert_eq!(row("core", false).1, "CPU Cores Power");
        assert_eq!(row("package-1", true).0, "rapl.package");
        assert_eq!(
            row("weird name", true),
            (
                "rapl.weird_name".to_string(),
                "weird name Power".to_string()
            )
        );
    }
}

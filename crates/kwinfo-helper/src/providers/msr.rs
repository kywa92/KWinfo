//! Intel model-specific registers through `/dev/cpu/<cpu>/msr` (the `msr` module, root
//! and `CAP_SYS_RAWIO`): what sysfs does not show.
//!
//! - `IA32_PERF_STATUS` (0x198) bits 47:32: the core voltage requested (VID), in units
//!   of 1/8192 V, per core: "Core #N VID".
//! - `IA32_THERM_STATUS` (0x19C) bit 10: the core runs below the requested frequency
//!   because of a power limit: "Core #N Power Limit Exceeded".
//! - `IA32_PACKAGE_THERM_STATUS` (0x1B1) bit 10: the same for the package.
//! - `MSR_PKG_Cx_RESIDENCY` (0x60D, 0x3F8-0x3FA, 0x630-0x632) against the time-stamp
//!   counter (0x10): package C-state residency in percent.
//!
//! Thermal throttling is not repeated here: the `cpu` backend reads it unprivileged from
//! the kernel's `thermal_throttle` counters. AMD CPUs have different registers and are
//! left for later (PLAN.md 6.2).

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io;
use std::time::Instant;

use kwinfo_core::backends::shared::{package_device, read_cpuinfo, read_topology};
use kwinfo_core::{Channel, ChannelId, Device, Kind, Sample, Source, SysRoot};

use super::{Provider, Status};

pub const IA32_TIME_STAMP_COUNTER: u32 = 0x10;
pub const IA32_PERF_STATUS: u32 = 0x198;
pub const IA32_THERM_STATUS: u32 = 0x19C;
pub const IA32_PACKAGE_THERM_STATUS: u32 = 0x1B1;
/// Bit 10 of both thermal status registers: power limitation status.
const POWER_LIMIT_BIT: u64 = 1 << 10;
/// The package C-state residency counters, as far as the CPU has them.
pub const PACKAGE_CSTATES: [(u32, &str); 7] = [
    (0x60D, "C2"),
    (0x3F8, "C3"),
    (0x3F9, "C6"),
    (0x3FA, "C7"),
    (0x630, "C8"),
    (0x631, "C9"),
    (0x632, "C10"),
];

/// Reads model-specific registers.
pub trait MsrDevice: Send {
    /// Register `msr` of logical CPU `cpu`.
    fn read(&mut self, cpu: u32, msr: u32) -> io::Result<u64>;
}

/// `/dev/cpu/<cpu>/msr` under a root, each kept open once opened.
pub struct DevMsr {
    root: SysRoot,
    files: HashMap<u32, File>,
}

impl DevMsr {
    pub fn new(root: SysRoot) -> Self {
        Self {
            root,
            files: HashMap::new(),
        }
    }
}

impl MsrDevice for DevMsr {
    fn read(&mut self, cpu: u32, msr: u32) -> io::Result<u64> {
        use std::os::unix::fs::FileExt as _;
        let file = match self.files.entry(cpu) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(File::open(self.root.path(format!("dev/cpu/{cpu}/msr")))?)
            }
        };
        let mut bytes = [0u8; 8];
        file.read_exact_at(&mut bytes, u64::from(msr))?;
        Ok(u64::from_le_bytes(bytes))
    }
}

/// The core voltage in `IA32_PERF_STATUS`, in volts.
pub fn vid_volts(perf_status: u64) -> f64 {
    ((perf_status >> 32) & 0xFFFF) as f64 / 8192.0
}

/// What reading MSRs failed with, for the user.
fn failure_note(err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::NotFound => "The msr kernel module is not loaded, so core VIDs, power \
             limits and package C-states are missing: sudo modprobe msr (kwinfo-helper \
             install loads it at every boot)."
            .to_string(),
        io::ErrorKind::PermissionDenied => {
            "Reading MSRs needs root and CAP_SYS_RAWIO; the helper is not running with them."
                .to_string()
        }
        _ => format!("Reading MSRs failed: {err}."),
    }
}

#[derive(Debug)]
struct CoreRegs {
    cpu: u32,
    vid: Option<ChannelId>,
    limit: Option<ChannelId>,
}

#[derive(Debug)]
struct CState {
    msr: u32,
    channel: ChannelId,
    prev: Option<u64>,
}

#[derive(Debug)]
struct PackageRegs {
    cpu: u32,
    limit: Option<ChannelId>,
    cstates: Vec<CState>,
    prev_tsc: Option<u64>,
}

/// See the module documentation.
pub struct Msr<D> {
    root: SysRoot,
    device: D,
    cores: Vec<CoreRegs>,
    packages: Vec<PackageRegs>,
    status: Status,
}

impl<D: MsrDevice> Msr<D> {
    pub fn new(root: SysRoot, device: D) -> Self {
        Self {
            root,
            device,
            cores: Vec::new(),
            packages: Vec::new(),
            status: Status::missing("MSRs have not been read yet"),
        }
    }
}

impl<D: MsrDevice> Provider for Msr<D> {
    fn name(&self) -> &'static str {
        "msr"
    }

    fn enumerate(&mut self) -> Vec<(Device, Vec<Channel>)> {
        self.cores.clear();
        self.packages.clear();
        if read_cpuinfo(&self.root).vendor.as_deref() != Some("GenuineIntel") {
            self.status = Status::missing("MSR sensors are only read on Intel CPUs so far.");
            return Vec::new();
        }
        let threads = read_topology(&self.root);
        let Some(first) = threads.first() else {
            self.status = Status::missing("The CPU topology could not be read.");
            return Vec::new();
        };
        if let Err(err) = self.device.read(first.cpu, IA32_PERF_STATUS) {
            self.status = Status::missing(failure_note(&err));
            return Vec::new();
        }
        // The first thread of every core and of every package reads for all of them.
        let mut packages: BTreeMap<u32, Vec<(u32, u32)>> = BTreeMap::new();
        for thread in &threads {
            let cores = packages.entry(thread.package).or_default();
            if !cores.iter().any(|&(core, _)| core == thread.core) {
                cores.push((thread.core, thread.cpu));
            }
        }
        let mut out = Vec::new();
        for (package, cores) in packages {
            let device = package_device(&self.root, package);
            let mut vids = Vec::new();
            let mut limits = Vec::new();
            for &(core, cpu) in &cores {
                // Some CPUs (and virtual machines) leave the voltage field at zero.
                let vid = self
                    .device
                    .read(cpu, IA32_PERF_STATUS)
                    .is_ok_and(|v| vid_volts(v) > 0.0)
                    .then(|| {
                        Channel::new(
                            &device.id,
                            &format!("msr.vid.core{core}"),
                            format!("Core #{core} VID"),
                            Kind::Voltage,
                        )
                        .with_group("Core VIDs")
                        .with_source(Source::Helper)
                    });
                let limit = self.device.read(cpu, IA32_THERM_STATUS).is_ok().then(|| {
                    Channel::new(
                        &device.id,
                        &format!("msr.plimit.core{core}"),
                        format!("Core #{core} Power Limit Exceeded"),
                        Kind::Boolean,
                    )
                    .with_group("Power Limit Exceeded")
                    .with_source(Source::Helper)
                });
                self.cores.push(CoreRegs {
                    cpu,
                    vid: vid.as_ref().map(|c| c.id.clone()),
                    limit: limit.as_ref().map(|c| c.id.clone()),
                });
                vids.extend(vid);
                limits.extend(limit);
            }
            let cpu = cores[0].1;
            let mut channels = vids;
            channels.extend(limits);
            let limit = self
                .device
                .read(cpu, IA32_PACKAGE_THERM_STATUS)
                .is_ok()
                .then(|| {
                    Channel::new(
                        &device.id,
                        "msr.plimit.package",
                        "Package Power Limit Exceeded",
                        Kind::Boolean,
                    )
                    .with_source(Source::Helper)
                });
            let mut cstates = Vec::new();
            if self.device.read(cpu, IA32_TIME_STAMP_COUNTER).is_ok() {
                for (msr, name) in PACKAGE_CSTATES {
                    if self.device.read(cpu, msr).is_err() {
                        continue;
                    }
                    let channel = Channel::new(
                        &device.id,
                        &format!("msr.pkg_{}", name.to_ascii_lowercase()),
                        format!("Package {name} Residency"),
                        Kind::Usage,
                    )
                    .with_group("Package C-States")
                    .with_source(Source::Helper);
                    cstates.push(CState {
                        msr,
                        channel: channel.id.clone(),
                        prev: None,
                    });
                    channels.push(channel);
                }
            }
            self.packages.push(PackageRegs {
                cpu,
                limit: limit.as_ref().map(|c| c.id.clone()),
                cstates,
                prev_tsc: None,
            });
            channels.extend(limit);
            out.push((device, channels));
        }
        self.status = Status::working();
        out
    }

    fn sample(&mut self, _now: Instant, out: &mut Vec<Sample>) {
        let device = &mut self.device;
        for core in &self.cores {
            if let Some(vid) = &core.vid {
                let value = device.read(core.cpu, IA32_PERF_STATUS).ok().map(vid_volts);
                out.push(Sample::new(vid.clone(), value));
            }
            if let Some(limit) = &core.limit {
                let value = device
                    .read(core.cpu, IA32_THERM_STATUS)
                    .ok()
                    .map(|v| f64::from(u8::from(v & POWER_LIMIT_BIT != 0)));
                out.push(Sample::new(limit.clone(), value));
            }
        }
        for package in &mut self.packages {
            if let Some(limit) = &package.limit {
                let value = device
                    .read(package.cpu, IA32_PACKAGE_THERM_STATUS)
                    .ok()
                    .map(|v| f64::from(u8::from(v & POWER_LIMIT_BIT != 0)));
                out.push(Sample::new(limit.clone(), value));
            }
            if package.cstates.is_empty() {
                continue;
            }
            let tsc = device.read(package.cpu, IA32_TIME_STAMP_COUNTER).ok();
            let elapsed = match (package.prev_tsc, tsc) {
                (Some(before), Some(now)) if now > before => Some(now - before),
                _ => None,
            };
            package.prev_tsc = tsc;
            for cstate in &mut package.cstates {
                let counter = device.read(package.cpu, cstate.msr).ok();
                let value = match (elapsed, cstate.prev, counter) {
                    (Some(elapsed), Some(before), Some(now)) if now >= before => {
                        Some(((now - before) as f64 / elapsed as f64 * 100.0).clamp(0.0, 100.0))
                    }
                    _ => None,
                };
                cstate.prev = counter;
                out.push(Sample::new(cstate.channel.clone(), value));
            }
        }
    }

    fn status(&self) -> Status {
        self.status.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::Path;

    /// Registers per `(cpu, msr)`; a missing one fails with `EIO` like a real unsupported
    /// register. `ticks` advances the time-stamp and residency counters per read of the TSC.
    #[derive(Default)]
    struct FakeMsr {
        regs: HashMap<(u32, u32), u64>,
        fail_all: Option<io::ErrorKind>,
    }

    impl MsrDevice for FakeMsr {
        fn read(&mut self, cpu: u32, msr: u32) -> io::Result<u64> {
            if let Some(kind) = self.fail_all {
                return Err(io::Error::from(kind));
            }
            self.regs
                .get(&(cpu, msr))
                .copied()
                .ok_or_else(|| io::Error::from_raw_os_error(5))
        }
    }

    fn root() -> SysRoot {
        SysRoot::new(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/synthetic-8700k"),
        )
    }

    /// An 8700K: cores 0-5 on CPUs 0-5 (their second threads are CPUs 6-11), 1.2 V,
    /// core 3 power-limited, package C2/C3/C6/C7 residency counters.
    fn coffee_lake() -> FakeMsr {
        let mut fake = FakeMsr::default();
        for cpu in 0..12 {
            fake.regs
                .insert((cpu, IA32_PERF_STATUS), (9830u64 << 32) | 0x2B00);
            let therm = if cpu == 3 { POWER_LIMIT_BIT } else { 0 };
            fake.regs
                .insert((cpu, IA32_THERM_STATUS), therm | 0x8800_0000);
        }
        fake.regs.insert((0, IA32_PACKAGE_THERM_STATUS), 0);
        fake.regs.insert((0, IA32_TIME_STAMP_COUNTER), 1_000_000);
        for (msr, _) in &PACKAGE_CSTATES[..4] {
            fake.regs.insert((0, *msr), 0);
        }
        fake
    }

    #[test]
    fn decodes_the_voltage_field() {
        assert!((vid_volts(9830u64 << 32) - 1.199_951).abs() < 1e-6);
        assert_eq!(vid_volts(0x2B00), 0.0);
    }

    #[test]
    fn finds_vids_limits_and_the_package_c_states_the_cpu_has() {
        let mut msr = Msr::new(root(), coffee_lake());
        let devices = msr.enumerate();
        assert_eq!(msr.status(), Status::working());
        assert_eq!(devices.len(), 1);
        let (device, channels) = &devices[0];
        assert_eq!(device.id.as_str(), "cpu:0");
        let labels: Vec<&str> = channels.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(&labels[..2], ["Core #0 VID", "Core #1 VID"]);
        assert_eq!(labels[6], "Core #0 Power Limit Exceeded");
        assert_eq!(
            &labels[12..],
            [
                "Package C2 Residency",
                "Package C3 Residency",
                "Package C6 Residency",
                "Package C7 Residency",
                "Package Power Limit Exceeded",
            ],
            "C8-C10 are not on a desktop part"
        );
        assert!(channels.iter().all(|c| c.source == Source::Helper));
        assert_eq!(channels[0].group.as_deref(), Some("Core VIDs"));
    }

    #[test]
    fn samples_voltages_flags_and_residency_percentages() {
        let mut msr = Msr::new(root(), coffee_lake());
        msr.enumerate();
        let mut out = Vec::new();
        msr.sample(Instant::now(), &mut out);
        let value =
            |out: &[Sample], key: &str| out.iter().find(|s| s.channel.key() == key).unwrap().value;
        assert!((value(&out, "msr.vid.core0").unwrap() - 1.199_951).abs() < 1e-6);
        assert_eq!(value(&out, "msr.plimit.core3"), Some(1.0));
        assert_eq!(value(&out, "msr.plimit.core0"), Some(0.0));
        assert_eq!(
            value(&out, "msr.pkg_c6"),
            None,
            "residency needs two readings"
        );

        // 1,000,000 TSC ticks later the package spent 25 % in C6 and 5 % in C2.
        msr.device
            .regs
            .insert((0, IA32_TIME_STAMP_COUNTER), 2_000_000);
        msr.device.regs.insert((0, 0x3F9), 250_000);
        msr.device.regs.insert((0, 0x60D), 50_000);
        out.clear();
        msr.sample(Instant::now(), &mut out);
        assert_eq!(value(&out, "msr.pkg_c6"), Some(25.0));
        assert_eq!(value(&out, "msr.pkg_c2"), Some(5.0));
        assert_eq!(value(&out, "msr.pkg_c7"), Some(0.0));
    }

    #[test]
    fn a_missing_msr_module_is_explained() {
        let fake = FakeMsr {
            fail_all: Some(io::ErrorKind::NotFound),
            ..FakeMsr::default()
        };
        let mut msr = Msr::new(root(), fake);
        assert!(msr.enumerate().is_empty());
        let status = msr.status();
        assert!(!status.available);
        assert!(status.note.unwrap().contains("modprobe msr"));
    }

    #[test]
    fn dev_msr_reads_eight_bytes_at_the_register_offset() {
        let dir = tempfile::tempdir().unwrap();
        let cpu0 = dir.path().join("dev/cpu/0");
        std::fs::create_dir_all(&cpu0).unwrap();
        let mut image = vec![0u8; 0x200];
        image[0x198..0x1A0].copy_from_slice(&0x0000_2666_0000_2B00u64.to_le_bytes());
        std::fs::write(cpu0.join("msr"), image).unwrap();
        let mut device = DevMsr::new(SysRoot::new(dir.path()));
        assert_eq!(
            device.read(0, IA32_PERF_STATUS).unwrap(),
            0x0000_2666_0000_2B00
        );
        assert_eq!(
            device.read(1, IA32_PERF_STATUS).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(
            device.read(0, 0x632).is_err(),
            "past the end, like an unsupported register"
        );
    }
}

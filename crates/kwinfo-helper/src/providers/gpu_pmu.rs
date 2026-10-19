//! Intel GPU engine load and RC6 residency from the i915 performance counters (PMU),
//! which need `CAP_PERFMON`: what `intel_gpu_top` shows.
//!
//! The PMU is `/sys/bus/event_source/devices/i915` (or `i915_<PCI address>` on kernels
//! that support several GPUs): `type` is the perf type, `cpumask` the CPU to count on,
//! and `events/<name>` holds `config=0x...` for each event. Engine `busy` events and
//! `rc6-residency` count nanoseconds, so load is their increase over elapsed time.
//! The `xe` driver's PMU differs and is left for later.

use std::io;
use std::time::Instant;

use kwinfo_core::backends::shared::gpus;
use kwinfo_core::sysroot::{list_dir, parse_cpu_list, read_int, read_string};
use kwinfo_core::{Category, Channel, ChannelId, Device, Kind, Sample, Source, SysRoot};

use super::{Provider, Status};

/// The events read, the channel key and the row label.
const EVENTS: [(&str, &str, &str); 6] = [
    ("rcs0-busy", "pmu.render", "GPU Core Load"),
    ("vcs0-busy", "pmu.video", "GPU Video Engine Load"),
    ("vecs0-busy", "pmu.video_enhance", "GPU Video Enhance Load"),
    ("bcs0-busy", "pmu.copy", "GPU Copy Engine Load"),
    ("ccs0-busy", "pmu.compute", "GPU Compute Load"),
    ("rc6-residency", "pmu.rc6", "GPU RC6 Residency"),
];

/// Opens and reads performance counters.
pub trait PerfCounters: Send {
    /// A system-wide counter of `config` on PMU `pmu`, counting on `cpu`; returns a handle.
    fn open(&mut self, pmu: u32, config: u64, cpu: i32) -> io::Result<usize>;
    fn read(&mut self, counter: usize) -> io::Result<u64>;
}

/// No performance counters (not Linux, or a captured tree).
pub struct NoCounters;

impl PerfCounters for NoCounters {
    fn open(&mut self, _: u32, _: u64, _: i32) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }

    fn read(&mut self, _: usize) -> io::Result<u64> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

/// The counters for `root`: the kernel's on the live Linux system, none otherwise.
pub fn host_counters(root: &SysRoot) -> Box<dyn PerfCounters> {
    #[cfg(target_os = "linux")]
    if root.is_host() {
        return Box::new(perf::Perf::default());
    }
    let _ = root;
    Box::new(NoCounters)
}

/// `config=0x30002` → `0x30002`.
pub fn parse_event(text: &str) -> Option<u64> {
    text.split(',').find_map(|term| {
        let value = term.trim().strip_prefix("config=")?;
        match value.strip_prefix("0x") {
            Some(hex) => u64::from_str_radix(hex, 16).ok(),
            None => value.parse().ok(),
        }
    })
}

#[derive(Debug)]
struct Engine {
    channel: ChannelId,
    counter: usize,
    prev: Option<(u64, Instant)>,
}

/// See the module documentation.
pub struct GpuPmu {
    root: SysRoot,
    counters: Box<dyn PerfCounters>,
    engines: Vec<Engine>,
    status: Status,
}

impl GpuPmu {
    pub fn new(root: SysRoot, counters: Box<dyn PerfCounters>) -> Self {
        Self {
            root,
            counters,
            engines: Vec::new(),
            status: Status::missing("The GPU counters have not been read yet"),
        }
    }

    /// The GPU device a PMU belongs to: the i915 card for `i915`, the card at the
    /// address for `i915_0000_00_02.0`.
    fn device_of(&self, pmu: &str) -> Device {
        let gpus = gpus(&self.root);
        let address = pmu.strip_prefix("i915_").map(|a| a.replace('_', ":"));
        let found = match &address {
            Some(address) => gpus.iter().find(|g| &g.address == address),
            None => gpus.iter().find(|g| g.driver.as_deref() == Some("i915")),
        };
        match (found, address) {
            (Some(gpu), _) => gpu.device.clone(),
            (None, Some(address)) => Device::new(format!("drm:{address}"), "GPU", Category::Gpu),
            (None, None) => Device::new("drm:i915", "GPU: Intel", Category::Gpu),
        }
    }
}

impl Provider for GpuPmu {
    fn name(&self) -> &'static str {
        "gpu_pmu"
    }

    fn enumerate(&mut self) -> Vec<(Device, Vec<Channel>)> {
        self.engines.clear();
        let base = self.root.sys("bus/event_source/devices");
        let mut out = Vec::new();
        let mut failure = None;
        for pmu in list_dir(&base) {
            if pmu != "i915" && !pmu.starts_with("i915_") {
                continue;
            }
            let dir = base.join(&pmu);
            let Ok(kind) = read_int(&dir.join("type")) else {
                continue;
            };
            let cpu = read_string(&dir.join("cpumask"))
                .and_then(|mask| parse_cpu_list(&mask).first().copied())
                .unwrap_or(0);
            let device = self.device_of(&pmu);
            let mut channels = Vec::new();
            for (event, key, label) in EVENTS {
                let Some(config) =
                    read_string(&dir.join("events").join(event)).and_then(|t| parse_event(&t))
                else {
                    continue;
                };
                let counter = match self.counters.open(kind as u32, config, cpu as i32) {
                    Ok(counter) => counter,
                    Err(err) => {
                        failure.get_or_insert(err);
                        continue;
                    }
                };
                let channel =
                    Channel::new(&device.id, key, label, Kind::Usage).with_source(Source::Helper);
                self.engines.push(Engine {
                    channel: channel.id.clone(),
                    counter,
                    prev: None,
                });
                channels.push(channel);
            }
            if !channels.is_empty() {
                out.push((device, channels));
            }
        }
        self.status = match (self.engines.is_empty(), failure) {
            (false, _) => Status::working(),
            (true, Some(err)) if err.kind() == io::ErrorKind::Unsupported => {
                Status::missing("GPU performance counters are only read on the live Linux system.")
            }
            (true, Some(err)) => Status::missing(format!(
                "The i915 performance counters could not be opened: {err}."
            )),
            (true, None) => Status::missing("There is no i915 GPU with performance counters."),
        };
        out
    }

    fn sample(&mut self, now: Instant, out: &mut Vec<Sample>) {
        for engine in &mut self.engines {
            let value = match self.counters.read(engine.counter) {
                Ok(ns) => {
                    let load = engine.prev.and_then(|(before, then)| {
                        let elapsed = now.checked_duration_since(then)?.as_nanos() as f64;
                        (elapsed > 0.0 && ns >= before)
                            .then(|| ((ns - before) as f64 / elapsed * 100.0).clamp(0.0, 100.0))
                    });
                    engine.prev = Some((ns, now));
                    load
                }
                Err(_) => None,
            };
            out.push(Sample::new(engine.channel.clone(), value));
        }
    }

    fn status(&self) -> Status {
        self.status.clone()
    }
}

#[cfg(target_os = "linux")]
mod perf {
    //! `perf_event_open(2)` for system-wide counting events.
    #![allow(unsafe_code)]

    use std::fs::File;
    use std::io::{self, Read as _};
    use std::os::fd::{FromRawFd as _, RawFd};

    use super::PerfCounters;

    /// The first published version of `struct perf_event_attr` (`PERF_ATTR_SIZE_VER0`,
    /// 64 bytes), which every kernel accepts. All flags off: the counter runs at once.
    #[repr(C)]
    #[derive(Default)]
    struct PerfEventAttr {
        kind: u32,
        size: u32,
        config: u64,
        sample_period: u64,
        sample_type: u64,
        read_format: u64,
        flags: u64,
        wakeup_events: u32,
        bp_type: u32,
        config1: u64,
    }

    const PERF_FLAG_FD_CLOEXEC: libc::c_ulong = 8;

    #[derive(Default)]
    pub(super) struct Perf {
        files: Vec<File>,
    }

    impl PerfCounters for Perf {
        fn open(&mut self, pmu: u32, config: u64, cpu: i32) -> io::Result<usize> {
            let attr = PerfEventAttr {
                kind: pmu,
                size: std::mem::size_of::<PerfEventAttr>() as u32,
                config,
                ..PerfEventAttr::default()
            };
            // SAFETY: `attr` is a fully initialised perf_event_attr whose `size` field
            // matches its layout; the kernel only reads it. pid -1 with a CPU counts
            // system-wide, group fd -1 starts a new group.
            let fd = unsafe {
                libc::syscall(
                    libc::SYS_perf_event_open,
                    &attr as *const PerfEventAttr,
                    -1 as libc::pid_t,
                    cpu as libc::c_int,
                    -1 as libc::c_int,
                    PERF_FLAG_FD_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: the kernel just returned this descriptor; nothing else owns it.
            let file = unsafe { File::from_raw_fd(fd as RawFd) };
            self.files.push(file);
            Ok(self.files.len() - 1)
        }

        fn read(&mut self, counter: usize) -> io::Result<u64> {
            let file = self
                .files
                .get_mut(counter)
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
            let mut bytes = [0u8; 8];
            file.read_exact(&mut bytes)?;
            Ok(u64::from_ne_bytes(bytes))
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn the_attribute_struct_has_the_kernels_first_layout() {
            assert_eq!(std::mem::size_of::<super::PerfEventAttr>(), 64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::Path;
    use std::time::Duration;

    /// Counters whose values the test sets; opened events are recorded by config.
    #[derive(Default)]
    struct FakeCounters {
        opened: Vec<(u32, u64, i32)>,
        values: HashMap<usize, u64>,
    }

    impl PerfCounters for std::sync::Arc<std::sync::Mutex<FakeCounters>> {
        fn open(&mut self, pmu: u32, config: u64, cpu: i32) -> io::Result<usize> {
            let mut fake = self.lock().unwrap();
            fake.opened.push((pmu, config, cpu));
            Ok(fake.opened.len() - 1)
        }

        fn read(&mut self, counter: usize) -> io::Result<u64> {
            Ok(self
                .lock()
                .unwrap()
                .values
                .get(&counter)
                .copied()
                .unwrap_or(0))
        }
    }

    fn root() -> SysRoot {
        SysRoot::new(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/synthetic-8700k"),
        )
    }

    #[test]
    fn events_parse_from_their_config_terms() {
        assert_eq!(parse_event("config=0x30002\n"), Some(0x30002));
        assert_eq!(parse_event("config=12"), Some(12));
        assert_eq!(parse_event("event=0x1,config=0x0"), Some(0));
        assert_eq!(parse_event("event=0x1"), None);
    }

    #[test]
    fn the_uhd_630_gets_engine_load_rows_in_its_gpu_group() {
        let fake = std::sync::Arc::new(std::sync::Mutex::new(FakeCounters::default()));
        let mut pmu = GpuPmu::new(root(), Box::new(fake.clone()));
        let devices = pmu.enumerate();
        assert_eq!(pmu.status(), Status::working());
        assert_eq!(devices.len(), 1);
        let (device, channels) = &devices[0];
        assert_eq!(device.id.as_str(), "drm:0000:00:02.0");
        let labels: Vec<&str> = channels.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "GPU Core Load",
                "GPU Video Engine Load",
                "GPU Video Enhance Load",
                "GPU Copy Engine Load",
                "GPU RC6 Residency",
            ]
        );
        let opened = fake.lock().unwrap().opened.clone();
        assert_eq!(opened[0], (17, 0x0, 0));
        assert_eq!(opened[4], (17, 0x10_0003, 0), "rc6-residency");

        let t0 = Instant::now();
        let mut out = Vec::new();
        pmu.sample(t0, &mut out);
        assert!(out.iter().all(|s| s.value.is_none()));
        {
            let mut fake = fake.lock().unwrap();
            fake.values.insert(0, 400_000_000); // render busy 0.4 s of 1 s
            fake.values.insert(4, 900_000_000); // RC6 0.9 s
        }
        out.clear();
        pmu.sample(t0 + Duration::from_secs(1), &mut out);
        assert_eq!(out[0].value, Some(40.0));
        assert_eq!(out[1].value, Some(0.0));
        assert_eq!(out[4].value, Some(90.0));
    }

    #[test]
    fn without_counters_the_note_says_why() {
        let mut pmu = GpuPmu::new(root(), Box::new(NoCounters));
        assert!(pmu.enumerate().is_empty());
        assert!(pmu.status().note.unwrap().contains("live Linux"));

        let dir = tempfile::tempdir().unwrap();
        let mut none = GpuPmu::new(SysRoot::new(dir.path()), Box::new(NoCounters));
        assert!(none.enumerate().is_empty());
        assert!(none.status().note.unwrap().contains("no i915"));
    }
}

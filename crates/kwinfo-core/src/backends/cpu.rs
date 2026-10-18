//! The `cpu` backend: usage from `/proc/stat`, clocks from cpufreq, C-state residency
//! from cpuidle and thermal throttling from the `thermal_throttle` counters, named after
//! the topology the way HWiNFO does ("Core #2 Thread #1 Usage").
//!
//! Files read, per online logical CPU `N` under `/sys/devices/system/cpu/cpuN`:
//! `topology/{physical_package_id,core_id}`, `cpufreq/scaling_cur_freq` (kHz),
//! `cpuidle/stateK/{name,time}` (µs), `thermal_throttle/{core,package}_throttle_count`
//! and `..._total_time_ms`; plus `/proc/stat` and `/proc/cpuinfo`.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Instant;

use crate::backend::Backend;
use crate::model::{Category, Channel, ChannelId, Device, Kind, Sample, Unit};
use crate::sysroot::{SysRoot, list_dir, read_int, read_into, read_string};

use super::util::{Counter, key_part, squash_spaces};

/// HWiNFO's ratio rows divide by the base clock, which is 100 MHz on every Intel CPU
/// since Sandy Bridge and every AMD Zen.
const BUS_CLOCK_MHZ: f64 = 100.0;

/// The device every CPU-package row belongs to, shared with the `hwmon` backend so that
/// `coretemp` temperatures land in the same group: id `cpu:<package>`, label
/// `CPU [#0]: Intel Core i7-8700K`.
pub(crate) fn package_device(root: &SysRoot, package: u32) -> Device {
    let info = read_cpuinfo(root);
    let name = info
        .models
        .get(&package)
        .or_else(|| info.models.values().next());
    let label = match name {
        Some(name) => format!("CPU [#{package}]: {}", pretty_cpu_name(name)),
        None => format!("CPU [#{package}]"),
    };
    Device::new(format!("cpu:{package}"), label, Category::Cpu).with_order(package as u16)
}

/// What `/proc/cpuinfo` says about the packages: model names and the vendor.
#[derive(Debug, Default)]
pub(crate) struct CpuInfo {
    pub vendor: Option<String>,
    pub models: BTreeMap<u32, String>,
}

pub(crate) fn read_cpuinfo(root: &SysRoot) -> CpuInfo {
    std::fs::read_to_string(root.proc("cpuinfo"))
        .map(|text| parse_cpuinfo(&text))
        .unwrap_or_default()
}

pub(crate) fn parse_cpuinfo(text: &str) -> CpuInfo {
    let mut info = CpuInfo::default();
    for block in text.split("\n\n") {
        let mut package = 0;
        let mut model = None;
        for line in block.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "vendor_id" if info.vendor.is_none() => info.vendor = Some(value.to_string()),
                "physical id" => package = value.parse().unwrap_or(0),
                "model name" if !value.is_empty() => model = Some(value.to_string()),
                _ => {}
            }
        }
        if let Some(model) = model {
            info.models.entry(package).or_insert(model);
        }
    }
    info
}

/// Strips the marketing noise from a `/proc/cpuinfo` model name:
/// `Intel(R) Core(TM) i7-8700K CPU @ 3.70GHz` → `Intel Core i7-8700K`,
/// `AMD Ryzen 7 9800X3D 8-Core Processor` → `AMD Ryzen 7 9800X3D`.
pub(crate) fn pretty_cpu_name(raw: &str) -> String {
    let mut name = raw.to_string();
    for noise in ["(R)", "(r)", "(TM)", "(tm)"] {
        name = name.replace(noise, "");
    }
    if let Some(at) = name.find(" @ ") {
        name.truncate(at);
    }
    for suffix in [" with Radeon Graphics", " with Radeon Vega Mobile Gfx"] {
        if let Some(at) = name.find(suffix) {
            name.truncate(at);
        }
    }
    let mut words: Vec<&str> = name.split_whitespace().collect();
    // "12th Gen Intel Core i7-12700K": HWiNFO leaves the generation out.
    let ordinal = |w: &str| {
        ["st", "nd", "rd", "th"]
            .iter()
            .any(|s| w.strip_suffix(s).is_some_and(|n| n.parse::<u32>().is_ok()))
    };
    if words.len() > 2 && ordinal(words[0]) && words[1] == "Gen" {
        words.drain(..2);
    }
    words.retain(|w| {
        let core_count = w
            .strip_suffix("-Core")
            .is_some_and(|n| n.parse::<u32>().is_ok());
        *w != "CPU" && *w != "Processor" && !core_count
    });
    squash_spaces(&words.join(" "))
}

/// Jiffies a logical CPU spent busy and in total, from one `cpuN` line of `/proc/stat`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CpuTimes {
    pub busy: u64,
    pub total: u64,
}

impl CpuTimes {
    /// Usage in percent between `self` (earlier) and `now`; `None` if no time passed.
    pub fn usage_since(self, now: CpuTimes) -> Option<f64> {
        let total = now.total.checked_sub(self.total)?;
        let busy = now.busy.checked_sub(self.busy)?;
        (total > 0).then(|| (busy.min(total) as f64 / total as f64) * 100.0)
    }
}

/// Parses the per-CPU lines of `/proc/stat`
/// (`cpuN user nice system idle iowait irq softirq steal guest guest_nice`). Guest time is
/// already included in user and nice, so it is not added again.
pub(crate) fn parse_proc_stat(text: &str, out: &mut Vec<(u32, CpuTimes)>) {
    out.clear();
    for line in text.lines() {
        // `cpuN ...`; the aggregate `cpu  ...` line has a space where the number would be.
        let Some(rest) = line
            .strip_prefix("cpu")
            .filter(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        else {
            continue;
        };
        let mut fields = rest.split_ascii_whitespace();
        let Some(Ok(cpu)) = fields.next().map(str::parse::<u32>) else {
            continue;
        };
        let values: Vec<u64> = fields.take(8).map_while(|f| f.parse().ok()).collect();
        if values.len() < 4 {
            continue;
        }
        let total: u64 = values.iter().sum();
        let idle = values[3] + values.get(4).copied().unwrap_or(0);
        out.push((
            cpu,
            CpuTimes {
                busy: total - idle,
                total,
            },
        ));
    }
}

/// Where a logical CPU sits: its package, core and position among the core's threads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Thread {
    pub cpu: u32,
    pub package: u32,
    pub core: u32,
    pub index: u32,
}

/// The online logical CPUs with their topology, ordered by package, core and thread.
pub(crate) fn read_topology(root: &SysRoot) -> Vec<Thread> {
    let base = root.sys("devices/system/cpu");
    let mut found = Vec::new();
    for name in list_dir(&base) {
        let Some(Ok(cpu)) = name.strip_prefix("cpu").map(str::parse::<u32>) else {
            continue;
        };
        let dir = base.join(&name);
        if read_string(&dir.join("online")).as_deref() == Some("0") {
            continue;
        }
        let topology = dir.join("topology");
        let Some(core) = read_int(&topology.join("core_id")).ok() else {
            continue;
        };
        let package = read_int(&topology.join("physical_package_id")).unwrap_or(0);
        found.push((
            u32::try_from(package).unwrap_or(0),
            u32::try_from(core).unwrap_or(0),
            cpu,
        ));
    }
    found.sort_unstable();
    let mut threads: Vec<Thread> = Vec::with_capacity(found.len());
    for (package, core, cpu) in found {
        let index = match threads.last() {
            Some(last) if last.package == package && last.core == core => last.index + 1,
            _ => 0,
        };
        threads.push(Thread {
            cpu,
            package,
            core,
            index,
        });
    }
    threads
}

/// Turns a cpuidle state name into a row label: `C1E` → `C1E`, `C6-SKL` → `C6`,
/// `C1_ACPI` → `C1`.
fn cstate_label(name: &str) -> String {
    let name = name.strip_suffix("_ACPI").unwrap_or(name);
    let name = match name.split_once('-') {
        Some((head, tail)) if tail.chars().all(|c| c.is_ascii_uppercase()) => head,
        _ => name,
    };
    format!("{name} Residency")
}

#[derive(Debug)]
struct ThreadPlan {
    cpu: u32,
    usage: ChannelId,
}

#[derive(Debug)]
struct ThrottlePlan {
    channel: ChannelId,
    /// `..._total_time_ms` where the kernel has it (5.18+), else `..._count`.
    counter: PathBuf,
}

#[derive(Debug)]
struct CorePlan {
    clock: ChannelId,
    ratio: Option<ChannelId>,
    freq_files: Vec<PathBuf>,
}

#[derive(Debug)]
struct CStatePlan {
    channel: ChannelId,
    /// `(cpu, path of cpuidle/stateK/time)` for every thread of the package.
    times: Vec<(u32, PathBuf)>,
}

#[derive(Debug)]
struct PackagePlan {
    threads: Vec<ThreadPlan>,
    cores: Vec<CorePlan>,
    total: ChannelId,
    max: ChannelId,
    throttles: Vec<ThrottlePlan>,
    cstates: Vec<CStatePlan>,
}

/// See the module documentation.
pub struct CpuBackend {
    root: SysRoot,
    packages: Vec<PackagePlan>,
    /// Previous readings, keyed by things that stay put across rescans.
    prev_times: HashMap<u32, CpuTimes>,
    throttle_counters: HashMap<PathBuf, Counter>,
    idle_prev: HashMap<PathBuf, u64>,
    idle_prev_at: Option<Instant>,
    stat_buf: Vec<u8>,
    stat_parsed: Vec<(u32, CpuTimes)>,
}

impl CpuBackend {
    pub fn new(root: SysRoot) -> Self {
        Self {
            root,
            packages: Vec::new(),
            prev_times: HashMap::new(),
            throttle_counters: HashMap::new(),
            idle_prev: HashMap::new(),
            idle_prev_at: None,
            stat_buf: Vec::with_capacity(16 * 1024),
            stat_parsed: Vec::new(),
        }
    }

    /// One sampling pass at an explicit time; [`Backend::sample`] passes the root's clock
    /// ([`SysRoot::now`]), which is the recorded time when a recording is replayed.
    /// Tests use it to control the interval between two readings.
    pub fn sample_at(&mut self, now: Instant, out: &mut Vec<Sample>) {
        let stat_ok = read_into(&self.root.proc("stat"), &mut self.stat_buf).is_ok();
        if stat_ok {
            let text = String::from_utf8_lossy(&self.stat_buf);
            parse_proc_stat(&text, &mut self.stat_parsed);
        } else {
            self.stat_parsed.clear();
        }
        let current: HashMap<u32, CpuTimes> = self.stat_parsed.iter().copied().collect();
        let idle_dt_us = self
            .idle_prev_at
            .and_then(|then| now.checked_duration_since(then))
            .map(|d| d.as_secs_f64() * 1e6);

        for package in &self.packages {
            // Clocks and ratios.
            for core in &package.cores {
                let khz = core
                    .freq_files
                    .iter()
                    .filter_map(|path| read_int(path).ok())
                    .max();
                let mhz = khz.map(|khz| khz as f64 / 1000.0);
                out.push(Sample::new(core.clock.clone(), mhz));
                if let Some(ratio) = &core.ratio {
                    out.push(Sample::new(ratio.clone(), mhz.map(|m| m / BUS_CLOCK_MHZ)));
                }
            }

            // Usage per thread, the package total and the busiest thread.
            let (mut busy, mut total) = (0u64, 0u64);
            let mut max: Option<f64> = None;
            for thread in &package.threads {
                let usage = match (self.prev_times.get(&thread.cpu), current.get(&thread.cpu)) {
                    (Some(&prev), Some(&now_times)) => {
                        busy += now_times.busy.saturating_sub(prev.busy);
                        total += now_times.total.saturating_sub(prev.total);
                        prev.usage_since(now_times)
                    }
                    _ => None,
                };
                if let Some(u) = usage {
                    max = Some(max.map_or(u, |m: f64| m.max(u)));
                }
                out.push(Sample::new(thread.usage.clone(), usage));
            }
            out.push(Sample::new(package.max.clone(), max));
            let total_usage = (total > 0).then(|| busy.min(total) as f64 / total as f64 * 100.0);
            out.push(Sample::new(package.total.clone(), total_usage));

            // Throttling: did the counter move since the last tick?
            for throttle in &package.throttles {
                let value = read_int(&throttle.counter)
                    .ok()
                    .and_then(|v| u64::try_from(v).ok());
                let moved = value.and_then(|v| {
                    self.throttle_counters
                        .entry(throttle.counter.clone())
                        .or_default()
                        .delta(v, now)
                });
                out.push(Sample::new(
                    throttle.channel.clone(),
                    moved.map(|d| if d > 0 { 1.0 } else { 0.0 }),
                ));
            }

            // Idle-state residency, averaged over the package's threads.
            for cstate in &package.cstates {
                let mut spent = 0.0;
                let mut threads = 0usize;
                for (_, path) in &cstate.times {
                    let Some(time) = read_int(path).ok().and_then(|v| u64::try_from(v).ok()) else {
                        continue;
                    };
                    if let Some(prev) = self.idle_prev.insert(path.clone(), time)
                        && time >= prev
                    {
                        spent += (time - prev) as f64;
                        threads += 1;
                    }
                }
                let residency = match idle_dt_us {
                    Some(dt) if dt > 0.0 && threads > 0 => {
                        Some((spent / (dt * threads as f64) * 100.0).clamp(0.0, 100.0))
                    }
                    _ => None,
                };
                out.push(Sample::new(cstate.channel.clone(), residency));
            }
        }

        if stat_ok {
            self.prev_times = current;
        }
        self.idle_prev_at = Some(now);
    }
}

impl Backend for CpuBackend {
    fn name(&self) -> &'static str {
        "cpu"
    }

    fn enumerate(&mut self) -> anyhow::Result<Vec<(Device, Vec<Channel>)>> {
        let threads = read_topology(&self.root);
        let info = read_cpuinfo(&self.root);
        let has_ratio = matches!(
            info.vendor.as_deref(),
            Some("GenuineIntel" | "AuthenticAMD" | "HygonGenuine")
        );
        let cpu_dir = self.root.sys("devices/system/cpu");

        let mut packages: BTreeMap<u32, Vec<Thread>> = BTreeMap::new();
        for thread in threads {
            packages.entry(thread.package).or_default().push(thread);
        }

        let mut result = Vec::new();
        self.packages.clear();
        for (package, threads) in packages {
            let device = package_device(&self.root, package);
            let id = device.id.clone();
            let mut channels = Vec::new();
            let mut cores: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
            for thread in &threads {
                cores.entry(thread.core).or_default().push(thread.cpu);
            }

            // Core clocks.
            let mut core_plans = Vec::new();
            let mut ratios = Vec::new();
            for (core, cpus) in &cores {
                let freq_files: Vec<PathBuf> = cpus
                    .iter()
                    .map(|cpu| cpu_dir.join(format!("cpu{cpu}/cpufreq/scaling_cur_freq")))
                    .filter(|path| path.exists())
                    .collect();
                if freq_files.is_empty() {
                    continue;
                }
                let clock = Channel::new(
                    &id,
                    &format!("clock.core{core}"),
                    format!("Core #{core} Clock"),
                    Kind::Clock,
                );
                let ratio = has_ratio.then(|| {
                    Channel::new(
                        &id,
                        &format!("ratio.core{core}"),
                        format!("Core #{core} Ratio"),
                        Kind::Other,
                    )
                    .with_unit(Unit::Multiplier)
                    .with_decimals(1)
                });
                core_plans.push(CorePlan {
                    clock: clock.id.clone(),
                    ratio: ratio.as_ref().map(|r| r.id.clone()),
                    freq_files,
                });
                channels.push(clock);
                ratios.extend(ratio);
            }

            // Usage.
            let mut thread_plans = Vec::new();
            for thread in &threads {
                let channel = Channel::new(
                    &id,
                    &format!("usage.cpu{}", thread.cpu),
                    format!("Core #{} Thread #{} Usage", thread.core, thread.index),
                    Kind::Usage,
                );
                thread_plans.push(ThreadPlan {
                    cpu: thread.cpu,
                    usage: channel.id.clone(),
                });
                channels.push(channel);
            }
            let max = Channel::new(&id, "usage.max", "Max CPU/Thread Usage", Kind::Usage);
            let total = Channel::new(&id, "usage.total", "Total CPU Usage", Kind::Usage);
            let (max_id, total_id) = (max.id.clone(), total.id.clone());
            channels.push(max);
            channels.push(total);

            // Thermal throttling, per core (first thread) and for the package.
            let mut throttles = Vec::new();
            let throttle_counter = |cpu: u32, scope: &str| {
                let dir = cpu_dir.join(format!("cpu{cpu}/thermal_throttle"));
                [
                    format!("{scope}_throttle_total_time_ms"),
                    format!("{scope}_throttle_count"),
                ]
                .into_iter()
                .map(|file| dir.join(file))
                .find(|path| path.exists())
            };
            for (core, cpus) in &cores {
                if let Some(counter) = throttle_counter(cpus[0], "core") {
                    let channel = Channel::new(
                        &id,
                        &format!("throttle.core{core}"),
                        format!("Core #{core} Thermal Throttling"),
                        Kind::Boolean,
                    );
                    throttles.push(ThrottlePlan {
                        channel: channel.id.clone(),
                        counter,
                    });
                    channels.push(channel);
                }
            }
            if let Some(counter) = throttle_counter(threads[0].cpu, "package") {
                let channel = Channel::new(
                    &id,
                    "throttle.package",
                    "Package/Ring Thermal Throttling",
                    Kind::Boolean,
                );
                throttles.push(ThrottlePlan {
                    channel: channel.id.clone(),
                    counter,
                });
                channels.push(channel);
            }

            channels.extend(ratios);

            // Idle states, named after the first thread's cpuidle states.
            let mut cstates = Vec::new();
            let first_idle = cpu_dir.join(format!("cpu{}/cpuidle", threads[0].cpu));
            for state in list_dir(&first_idle) {
                if !state.starts_with("state") {
                    continue;
                }
                let Some(name) = read_string(&first_idle.join(&state).join("name")) else {
                    continue;
                };
                if name == "POLL" {
                    continue;
                }
                let times: Vec<(u32, PathBuf)> = threads
                    .iter()
                    .map(|t| {
                        (
                            t.cpu,
                            cpu_dir.join(format!("cpu{}/cpuidle/{state}/time", t.cpu)),
                        )
                    })
                    .filter(|(_, path)| path.exists())
                    .collect();
                let channel = Channel::new(
                    &id,
                    &format!("cstate.{}", key_part(&name.to_lowercase())),
                    cstate_label(&name),
                    Kind::Usage,
                );
                cstates.push(CStatePlan {
                    channel: channel.id.clone(),
                    times,
                });
                channels.push(channel);
            }

            self.packages.push(PackagePlan {
                threads: thread_plans,
                cores: core_plans,
                total: total_id,
                max: max_id,
                throttles,
                cstates,
            });
            result.push((device, channels));
        }
        Ok(result)
    }

    fn sample(&mut self, out: &mut Vec<Sample>) -> anyhow::Result<()> {
        self.sample_at(self.root.now(), out);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::backends::test_support::*;

    #[test]
    fn cpu_names_lose_their_marketing_noise() {
        let cases = [
            (
                "Intel(R) Core(TM) i7-8700K CPU @ 3.70GHz",
                "Intel Core i7-8700K",
            ),
            (
                "AMD Ryzen 7 9800X3D 8-Core Processor",
                "AMD Ryzen 7 9800X3D",
            ),
            (
                "AMD Ryzen 5 5600G with Radeon Graphics",
                "AMD Ryzen 5 5600G",
            ),
            (
                "12th Gen Intel(R) Core(TM) i7-12700K",
                "Intel Core i7-12700K",
            ),
            (
                "Intel(R) Xeon(R) CPU E5-2680 v4 @ 2.40GHz",
                "Intel Xeon E5-2680 v4",
            ),
            ("Intel(R) Core(TM) Ultra 7 155H", "Intel Core Ultra 7 155H"),
        ];
        for (raw, pretty) in cases {
            assert_eq!(pretty_cpu_name(raw), pretty, "{raw}");
        }
    }

    #[test]
    fn proc_stat_lines_become_busy_and_total_jiffies() {
        let mut out = Vec::new();
        parse_proc_stat(
            "cpu  10 0 10 80 0 0 0 0 0 0\ncpu0 1 2 3 4 5 6 7 8 9 10\nintr 1 2 3\ncpu1 5 0 5 90\n",
            &mut out,
        );
        assert_eq!(
            out,
            vec![
                (
                    0,
                    CpuTimes {
                        busy: 1 + 2 + 3 + 6 + 7 + 8,
                        total: 36
                    }
                ),
                (
                    1,
                    CpuTimes {
                        busy: 10,
                        total: 100
                    }
                ),
            ]
        );
        let earlier = CpuTimes {
            busy: 10,
            total: 100,
        };
        let later = CpuTimes {
            busy: 35,
            total: 200,
        };
        assert_eq!(earlier.usage_since(later), Some(25.0));
        assert_eq!(later.usage_since(later), None);
    }

    #[test]
    fn topology_numbers_threads_within_cores() {
        let threads = read_topology(&fixture_root());
        assert_eq!(threads.len(), 12);
        let names: Vec<(u32, u32, u32)> =
            threads.iter().map(|t| (t.cpu, t.core, t.index)).collect();
        assert_eq!(names[0], (0, 0, 0));
        assert_eq!(names[1], (6, 0, 1));
        assert_eq!(names[11], (11, 5, 1));
    }

    #[test]
    fn enumerates_the_8700k_rows_in_hwinfo_order() {
        let mut backend = CpuBackend::new(fixture_root());
        let devices = backend.enumerate().unwrap();
        assert_eq!(devices.len(), 1);
        let (device, channels) = &devices[0];
        assert_eq!(device.id.as_str(), "cpu:0");
        assert_eq!(device.label, "CPU [#0]: Intel Core i7-8700K");
        assert_eq!(device.category, Category::Cpu);

        let labels: Vec<&str> = channels.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(&labels[..2], ["Core #0 Clock", "Core #1 Clock"]);
        assert_eq!(labels[6], "Core #0 Thread #0 Usage");
        assert_eq!(labels[7], "Core #0 Thread #1 Usage");
        assert_eq!(labels[17], "Core #5 Thread #1 Usage");
        assert_eq!(labels[18], "Max CPU/Thread Usage");
        assert_eq!(labels[19], "Total CPU Usage");
        assert_eq!(labels[20], "Core #0 Thermal Throttling");
        assert_eq!(labels[26], "Package/Ring Thermal Throttling");
        assert_eq!(labels[27], "Core #0 Ratio");
        assert_eq!(
            &labels[33..],
            [
                "C1 Residency",
                "C1E Residency",
                "C3 Residency",
                "C6 Residency",
                "C7s Residency",
                "C8 Residency",
                "C9 Residency",
                "C10 Residency",
            ]
        );

        let usage = channel(channels, "usage.cpu6");
        assert_eq!(usage.label, "Core #0 Thread #1 Usage");
        assert_eq!((usage.kind, usage.unit), (Kind::Usage, Unit::Percent));
        let ratio = channel(channels, "ratio.core0");
        assert_eq!((ratio.unit, ratio.decimals), (Unit::Multiplier, 1));
        assert_eq!(channel(channels, "cstate.c7s").kind, Kind::Usage);
        assert_eq!(
            channel(channels, "throttle.package").id.as_str(),
            "cpu:0/throttle.package"
        );
    }

    #[test]
    fn clocks_come_from_the_fastest_thread_of_each_core() {
        let mut backend = CpuBackend::new(fixture_root());
        let (_, samples) = enumerate_and_sample(&mut backend);
        let values = sample_map(&samples);
        let get = |key: &str| values[&ChannelId::from_raw(format!("cpu:0/{key}"))];
        // cpu0 runs at 4,300,000 kHz and its sibling cpu6 at 4,299,876 kHz.
        assert_close(get("clock.core0"), 4300.0);
        assert_close(get("ratio.core0"), 43.0);
        assert_close(get("clock.core5"), 800.012);
        // The first sample has nothing to compare against.
        assert_eq!(get("usage.total"), None);
        assert_eq!(get("throttle.core0"), None);
        assert_eq!(get("cstate.c6"), None);
    }

    #[test]
    fn usage_throttling_and_residency_follow_the_counters() {
        let (_dir, root) = fixture_copy();
        let mut backend = CpuBackend::new(root.clone());
        backend.enumerate().unwrap();
        let (t0, t1) = instants(Duration::from_secs(1));
        let mut first = Vec::new();
        backend.sample_at(t0, &mut first);

        // One second later: cpu0 was 50 % busy, cpu6 25 %, every other thread idle.
        let stat = std::fs::read_to_string(root.proc("stat")).unwrap();
        let mut lines: Vec<String> = stat.lines().map(str::to_string).collect();
        for line in &mut lines {
            let bump = |line: &str, user: u64, idle: u64| {
                let mut f: Vec<u64> = line
                    .split_whitespace()
                    .skip(1)
                    .map(|v| v.parse().unwrap())
                    .collect();
                f[0] += user;
                f[3] += idle;
                let name = line.split_whitespace().next().unwrap().to_string();
                format!(
                    "{name} {}",
                    f.iter().map(u64::to_string).collect::<Vec<_>>().join(" ")
                )
            };
            if line.starts_with("cpu0 ") {
                *line = bump(line, 50, 50);
            } else if line.starts_with("cpu6 ") {
                *line = bump(line, 25, 75);
            } else if line.starts_with("cpu") && !line.starts_with("cpu ") {
                *line = bump(line, 0, 100);
            }
        }
        write(&root, "proc/stat", &(lines.join("\n") + "\n"));
        // The package throttled once; core 0 did not.
        write(
            &root,
            "sys/devices/system/cpu/cpu0/thermal_throttle/package_throttle_total_time_ms",
            "160\n",
        );
        // Every thread spent 0.6 s of the second in C6.
        for cpu in 0..12 {
            let path = root.sys(format!("devices/system/cpu/cpu{cpu}/cpuidle/state4/time"));
            let before: u64 = std::fs::read_to_string(&path)
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            std::fs::write(&path, format!("{}\n", before + 600_000)).unwrap();
        }

        let mut second = Vec::new();
        backend.sample_at(t1, &mut second);
        let values = sample_map(&second);
        let get = |key: &str| values[&ChannelId::from_raw(format!("cpu:0/{key}"))];
        assert_close(get("usage.cpu0"), 50.0);
        assert_close(get("usage.cpu6"), 25.0);
        assert_close(get("usage.cpu1"), 0.0);
        assert_close(get("usage.max"), 50.0);
        assert_close(get("usage.total"), 75.0 / 12.0);
        assert_close(get("throttle.package"), 1.0);
        assert_close(get("throttle.core0"), 0.0);
        assert_close(get("cstate.c6"), 60.0);
        assert_close(get("cstate.c1e"), 0.0);
    }
}

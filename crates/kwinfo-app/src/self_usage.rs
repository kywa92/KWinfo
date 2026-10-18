//! KWinfo's own CPU use, shown in the window title like HWiNFO's "(0.2%)": the share of
//! the whole machine's CPU time the process used over the last second or so.
//!
//! Linux only, from `/proc/self/stat`; elsewhere (the Mac development build) the title
//! simply goes without it.

use std::time::{Duration, Instant};

/// Shortest time between two measurements.
const EVERY: Duration = Duration::from_secs(1);

pub struct SelfUsage {
    last: Option<(Instant, Duration)>,
    percent: Option<f64>,
    cpus: f64,
}

impl Default for SelfUsage {
    fn default() -> Self {
        Self::new()
    }
}

impl SelfUsage {
    pub fn new() -> Self {
        let cpus = std::thread::available_parallelism().map_or(1, usize::from);
        Self {
            last: None,
            percent: None,
            cpus: cpus as f64,
        }
    }

    /// Measures again when at least a second has passed since the last measurement.
    pub fn update(&mut self, now: Instant) {
        self.record(now, process_cpu_time());
    }

    fn record(&mut self, now: Instant, cpu: Option<Duration>) {
        if self
            .last
            .is_some_and(|(then, _)| now.saturating_duration_since(then) < EVERY)
        {
            return;
        }
        let Some(cpu) = cpu else {
            return;
        };
        if let Some((then, before)) = self.last {
            let wall = now.saturating_duration_since(then).as_secs_f64();
            if wall > 0.0 {
                let used = cpu.saturating_sub(before).as_secs_f64();
                self.percent = Some(used / wall / self.cpus * 100.0);
            }
        }
        self.last = Some((now, cpu));
    }

    /// The last measurement, in percent of the machine's total CPU capacity.
    pub fn percent(&self) -> Option<f64> {
        self.percent
    }
}

#[cfg(target_os = "linux")]
fn process_cpu_time() -> Option<Duration> {
    parse_stat(&std::fs::read_to_string("/proc/self/stat").ok()?)
}

#[cfg(not(target_os = "linux"))]
fn process_cpu_time() -> Option<Duration> {
    None
}

/// User plus system time from a `/proc/<pid>/stat` line (fields 14 and 15), counted in
/// clock ticks of 1/100 s (`USER_HZ`, which is 100 on every architecture KWinfo targets).
#[cfg(any(target_os = "linux", test))]
fn parse_stat(stat: &str) -> Option<Duration> {
    // The command name (field 2) is in parentheses and may contain spaces or ')'.
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // `fields[0]` is field 3 (the state), so utime (14) and stime (15) follow at 11, 12.
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some(Duration::from_millis((utime + stime) * 10))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_lines_give_user_plus_system_time() {
        let line = "4242 (kwinfo (main)) S 1 4242 4242 0 -1 4194560 2471 0 0 0 250 50 0 0 20 0 \
                    9 0 123 456 789";
        assert_eq!(parse_stat(line), Some(Duration::from_secs(3)));
        assert_eq!(parse_stat("garbage"), None);
        assert_eq!(parse_stat("1 (x) S 1 2"), None);
    }

    #[test]
    fn usage_is_a_share_of_all_cpus() {
        let mut usage = SelfUsage::new();
        usage.cpus = 4.0;
        let t0 = Instant::now();
        usage.record(t0, Some(Duration::from_millis(100)));
        assert_eq!(usage.percent(), None, "one reading is not a rate");
        usage.record(
            t0 + Duration::from_millis(500),
            Some(Duration::from_millis(900)),
        );
        assert_eq!(usage.percent(), None, "too soon");
        usage.record(
            t0 + Duration::from_secs(2),
            Some(Duration::from_millis(180)),
        );
        let percent = usage.percent().unwrap();
        // 80 ms of CPU in 2 s on 4 CPUs: 1 %.
        assert!((percent - 1.0).abs() < 1e-9, "{percent}");
    }
}

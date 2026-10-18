//! Runs every Linux backend against `tests/fixtures/synthetic-8700k` through the real
//! sampler and compares the complete listing (ids, labels, kinds, units, decimals,
//! scaled values, limits) with `expected/synthetic-8700k.txt`.
//!
//! After an intended change, regenerate the expectation and review the diff:
//!
//! ```bash
//! UPDATE_EXPECT=1 cargo test -p kwinfo-core --test synthetic_8700k
//! ```

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use kwinfo_core::{
    BackendChoice, BackendSet, Category, Sampler, SamplerConfig, Snapshot, TemperatureUnit,
    format_value,
};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn replay(dir: &Path) -> std::sync::Arc<Snapshot> {
    let set = BackendSet::build(BackendChoice::Replay(dir.to_path_buf()));
    assert_eq!(set.backends.len(), 7);
    let sampler = Sampler::start(
        set.backends,
        SamplerConfig {
            interval: Duration::from_millis(50),
            history_capacity: 4,
        },
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let snapshot = sampler.snapshot();
        if snapshot.tick >= 2 {
            return snapshot;
        }
        assert!(Instant::now() < deadline, "the sampler did not tick");
        thread::sleep(Duration::from_millis(5));
    }
}

fn render(snapshot: &Snapshot) -> String {
    let mut out = String::new();
    for device in &snapshot.devices {
        let d = &device.device;
        writeln!(
            out,
            "{}  [{}]  {:?} #{}",
            d.label, d.id, d.category, d.order
        )
        .unwrap();
        for entry in &device.channels {
            let c = &entry.channel;
            let value = format_value(
                entry.stats.current(),
                c.unit,
                c.decimals,
                TemperatureUnit::Celsius,
            );
            write!(
                out,
                "  {} | {} | {:?} | {:?}/{} | {}",
                c.id.key(),
                c.label,
                c.kind,
                c.unit,
                c.decimals,
                value
            )
            .unwrap();
            if let Some(max) = c.limits.max {
                write!(out, " | max {max}").unwrap();
            }
            if let Some(crit) = c.limits.crit {
                write!(out, " | crit {crit}").unwrap();
            }
            out.push('\n');
        }
    }
    out
}

#[test]
fn the_synthetic_8700k_lists_exactly_as_expected() {
    let snapshot = replay(&workspace().join("tests/fixtures/synthetic-8700k"));
    let actual = render(&snapshot);
    let expected_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/expected/synthetic-8700k.txt");
    if std::env::var_os("UPDATE_EXPECT").is_some() {
        std::fs::create_dir_all(expected_path.parent().unwrap()).unwrap();
        std::fs::write(&expected_path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&expected_path)
        .expect("expected listing exists; run with UPDATE_EXPECT=1 to create it");
    for (n, (a, e)) in actual.lines().zip(expected.lines()).enumerate() {
        assert_eq!(a, e, "line {} differs", n + 1);
    }
    assert_eq!(
        actual.lines().count(),
        expected.lines().count(),
        "different number of lines"
    );
}

#[test]
fn groups_come_in_hwinfo_order_with_merged_components() {
    let snapshot = replay(&workspace().join("tests/fixtures/synthetic-8700k"));
    let groups: Vec<(Category, &str)> = snapshot
        .devices
        .iter()
        .map(|d| (d.device.category, d.device.label.as_str()))
        .collect();
    assert_eq!(
        groups,
        [
            (Category::Cpu, "CPU [#0]: Intel Core i7-8700K"),
            (
                Category::Motherboard,
                "ASUS PRIME Z370-A (Nuvoton NCT6793D)"
            ),
            (Category::Memory, "Memory"),
            (Category::Gpu, "GPU [#0]: Intel UHD Graphics 630"),
            (Category::Drive, "Drive: Samsung SSD 970 EVO Plus 1TB"),
            (Category::Drive, "Drive: Samsung SSD 860 EVO 500GB"),
            (Category::Network, "Network: enp0s31f6"),
            (Category::Battery, "Battery: SMP 5B10W13930"),
            (Category::Other, "ACPI Thermal Zone"),
        ]
    );
    // coretemp (hwmon) and the cpu backend share one group, temperatures first.
    let cpu = &snapshot.devices[0];
    assert_eq!(cpu.channels[0].channel.label, "CPU Package");
    assert!(
        cpu.channels
            .iter()
            .any(|c| c.channel.label == "Total CPU Usage")
    );
    // The NVMe hwmon chip joins its drive's group.
    let nvme = &snapshot.devices[4];
    assert_eq!(nvme.channels[0].channel.label, "Drive Temperature");
    assert_eq!(snapshot.channel_count(), 137);
}

#[test]
fn replaying_a_missing_directory_says_so() {
    let set = BackendSet::build(BackendChoice::Replay(PathBuf::from("/nonexistent/kwinfo")));
    assert!(
        set.notes
            .iter()
            .any(|n| n.contains("neither sys/ nor proc/"))
    );
}

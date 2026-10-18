//! Runs the `kwinfo-cli` binary against the synthetic fixture: `list --replay` and a
//! `snapshot` round trip, as a user would on the 8700K.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/synthetic-8700k")
}

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kwinfo-cli"))
        .args(args)
        .output()
        .expect("kwinfo-cli runs")
}

fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "kwinfo-cli failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

/// The device header lines of a `list` output.
fn groups(listing: &str) -> Vec<&str> {
    listing
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with(' ') && !l.contains(" channels"))
        .collect()
}

#[test]
fn list_replays_the_fixture() {
    let out = stdout(&cli(&["list", "--replay", fixture().to_str().unwrap()]));
    assert_eq!(
        groups(&out),
        [
            "CPU [#0]: Intel Core i7-8700K",
            "ASUS PRIME Z370-A (Nuvoton NCT6793D)",
            "Memory",
            "GPU [#0]: Intel UHD Graphics 630",
            "Drive: Samsung SSD 970 EVO Plus 1TB",
            "Drive: Samsung SSD 860 EVO 500GB",
            "Network: enp0s31f6",
            "Battery: SMP 5B10W13930",
            "ACPI Thermal Zone",
        ]
    );
    assert!(out.contains("9 devices, 137 channels"), "{out}");
    assert!(out.contains("CPU Package"), "{out}");

    let fahrenheit = stdout(&cli(&[
        "list",
        "--replay",
        fixture().to_str().unwrap(),
        "--fahrenheit",
        "--ids",
    ]));
    assert!(
        fahrenheit.contains("100.4 °F"),
        "38 °C package: {fahrenheit}"
    );
    assert!(
        fahrenheit.contains("[hwmon:nct6793:nct6775.656]"),
        "{fahrenheit}"
    );
    assert!(fahrenheit.contains("coretemp.temp1_input"), "{fahrenheit}");
}

#[test]
fn snapshot_round_trips_and_refuses_to_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("snap");
    let report = stdout(&cli(&[
        "snapshot",
        "--root",
        fixture().to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]));
    assert!(report.contains("Scrubbed"), "{report}");
    assert!(out.join("kwinfo-snapshot.txt").is_file());

    let original = stdout(&cli(&["list", "--replay", fixture().to_str().unwrap()]));
    let replayed = stdout(&cli(&["list", "--replay", out.to_str().unwrap()]));
    assert_eq!(groups(&replayed), groups(&original));

    let again = cli(&[
        "snapshot",
        "--root",
        fixture().to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("not empty"));
}

#[test]
fn record_writes_a_replayable_recording() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("recording");
    let report = stdout(&cli(&[
        "record",
        "--root",
        fixture().to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--duration",
        "1s",
        "--interval",
        "250",
    ]));
    // Frame 0 and one frame per 250 ms.
    assert!(report.contains("Recorded 5 frames"), "{report}");
    let recording = std::fs::read_to_string(out.join("kwinfo-recording.txt")).unwrap();
    assert!(recording.starts_with("kwinfo-recording 1\ninterval-us 250000\n"));
    assert!(recording.contains("\nframe 4 "), "{recording}");

    let replayed = stdout(&cli(&["list", "--replay", out.to_str().unwrap()]));
    assert!(replayed.contains("9 devices, 137 channels"), "{replayed}");

    let bad = cli(&["record", "--out", "x", "--interval", "5"]);
    assert!(!bad.status.success(), "intervals below 50 ms are refused");
}

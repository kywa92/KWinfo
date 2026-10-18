# Changelog

All notable changes to KWinfo are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Bootstrap the Cargo workspace: `kwinfo-core`, `kwinfo-ipc`, `kwinfo-helper`,
  `kwinfo-app`, `kwinfo-cli` (milestone M0).
- Sensor data model, sampler thread and a mock backend for development on any OS.
- eframe application shell with the Sensor Status table, dark and light palettes and
  system theme detection.
- `kwinfo-cli list` to print devices and channels as a table.
- GitHub Actions CI: rustfmt, clippy, tests on Ubuntu 22.04 and macOS, cargo-deny.
- Headless `egui_kittest` snapshot tests of the table in both palettes, and a preview
  test that renders the whole window to `target/preview/`.
- Unprivileged Linux backends (milestone M1), all reading through a `SysRoot` so that
  they build and are tested on any OS: `hwmon` (every `/sys/class/hwmon` chip, scaled,
  labelled, with limits and alarm rows; `coretemp`, `nvme`/`drivetemp` and GPU chips join
  their CPU, drive or GPU group), `cpu` (per-thread and total usage, core clocks and
  ratios, C-state residency, thermal throttling), `intel_gpu` (actual, requested, max and
  boost clocks), `block` (activity, rates and totals per physical disk, full ATA models),
  `memory`, `network` (rates and totals; loopback and virtual interfaces hidden) and
  `power_supply` (batteries and UPSes).
- Stable device ids from driver names and bus paths (`hwmon:nct6793:nct6775.656`,
  `cpu:0`, `drm:0000:00:02.0`); drive ids carry a hash of the WWID or serial number
  instead of the number itself.
- `libsensors` cargo feature (default on Linux, built on the `lm-sensors` crate): labels,
  `ignore` and `compute` statements from the lm-sensors configuration.
- `kwinfo-cli snapshot --out <dir>`: captures the sensor-relevant parts of `/sys` and
  `/proc` with serial numbers, WWNs and MAC addresses scrubbed.
- `--replay <dir>` for `kwinfo` and `kwinfo-cli list`: runs the real backends against a
  captured tree. `kwinfo-cli list --ids` shows device and channel ids.
- `kwinfo-cli record --duration <time> --interval <ms> --out <dir>`: a snapshot plus every
  changing value frame by frame (`kwinfo-recording.txt`). `--replay` plays recordings
  back in a loop with the recorded timing, so usage and rates move as they did.
- Hand-built `tests/fixtures/synthetic-8700k` fixture with unit tests and a full expected
  listing.

### Changed

- `kwinfo-cli list` samples through the sampler thread, so devices that several backends
  contribute to are merged as in the GUI.
- The sampler schedules its first tick after the initial enumeration, so the second tick
  (the first with rates) comes a full interval later.
- New units `Wh` and `x` (multiplier); channels carry optional `max`/`crit` limits.
- Rate-based backends (`cpu`, `block`, `network`) take the time from their `SysRoot`, which
  is the wall clock except while a recording is replayed.

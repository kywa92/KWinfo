# Changelog

All notable changes to KWinfo are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Diagnostic logging (`kwinfo-diag`): a log file per run with a machine header, panic
  backtraces, user-action and sampler-health lines, and `kwinfo-cli report` bundling the
  logs, configuration and sensor inventory into one text file (docs/diagnostics.md).

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
- The complete Sensor Status window (milestone M2): runs of similar rows nest in
  collapsible sub-groups (Core Clocks, Thread Usage, C-State Residency, Distance to TjMAX,
  Alarms, …) whose row shows the members' lowest minimum and highest maximum; painted
  per-kind row icons and per-category device icons (no `egui-phosphor` release supports
  egui 0.37); context menus for sensors (Show Graph, which logs until graph windows land
  in M3, Rename, Hide/Unhide, Custom colour, Disable/Enable monitoring, Reset this sensor,
  Copy value), devices (collapse, reset, disable/enable monitoring, copy values),
  sub-groups and the header (column visibility, reset widths); double-click reserved for
  Show Graph; a Ctrl+F quick filter; `[N value(s) hidden]` and, on Linux, KWinfo's own CPU
  use in the window title; a "Show hidden sensors" setting that lists hidden rows greyed.
- Toolbar with painted icons: expand/collapse all, reset, logging (M5), elapsed time,
  polling period, a helper-status placeholder (M4), filter, settings and quit.
- Settings dialog tabs: General, Theme, Columns and Hidden sensors (with Unhide and Enable
  buttons); the Logging, Helper and Alerts tabs say which milestone fills them.
- Persistent settings in `~/.config/kwinfo/config.toml` (`kwinfo_core::config`): theme,
  temperature unit, polling period, column visibility and widths, window size, per-device
  collapsed and disabled state and collapsed sub-groups, and per-sensor hidden, name,
  colour and disabled state, keyed by the stable ids. Saved a moment after each change and
  on quit; an unreadable file is kept as `config.toml.bad`. `--config <file>` (or
  `KWINFO_CONFIG`) selects another file.
- "Disable monitoring" for sensors and whole devices: the sampler ignores them, clears
  their statistics and history, and tells the backends through the new
  `Backend::set_disabled`, so `hwmon`, `cpu`, `intel_gpu`, `block`, `network`,
  `power_supply` and the mock backend skip the reads (keeping inputs that enabled
  derived rows such as Core Max need).
- Custom row colours keep their hue and get their lightness clamped per theme to reach
  4.5:1 contrast on every row background; kind icons are clamped to 3:1. An "Other
  colour…" dialog previews a colour in both themes.
- User actions in the window are logged with `kwinfo_diag::action!`: hide, rename,
  colour, disable/enable, reset, copy, expand/collapse, filter and settings open/close,
  settings changes, Show Graph and quit.
- "Save diagnostic report…" in Settings → General writes a `kwinfo_diag` report whose
  sensor inventory is the window's current view, on a background thread, and shows the
  file's path.
- The sampler logs a `kwinfo::health` line every 60 ticks: average and maximum tick cost,
  device and channel counts, unreadable and disabled channels, failing backends.
- Headless interaction tests of the window that run with `cargo test`, and snapshot
  baselines for a customised table and an open context menu in both themes.

### Changed

- `kwinfo-cli list` samples through the sampler thread, so devices that several backends
  contribute to are merged as in the GUI.
- The sampler schedules its first tick after the initial enumeration, so the second tick
  (the first with rates) comes a full interval later.
- New units `Wh` and `x` (multiplier); channels carry optional `max`/`crit` limits.
- Rate-based backends (`cpu`, `block`, `network`) take the time from their `SysRoot`, which
  is the wall clock except while a recording is replayed.
- Channels carry an optional `group` (sub-group name); the expected 8700K listing shows it.
  Snapshots flag disabled channels (`ChannelSnapshot::disabled`), and the sampler can
  reset several channels at once.
- `--interval` and `--theme` of `kwinfo` now override the saved settings for one run
  instead of always applying their defaults. `ThemeMode` moved to `kwinfo_core::config`.
- Labels no longer take clicks for text selection, so a click anywhere on a row reaches
  the row.

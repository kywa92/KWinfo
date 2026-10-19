# KWinfo

A Linux hardware monitor in the spirit of HWiNFO64's *Sensor Status* window and its
floating sensor graphs. Written in Rust with [egui](https://github.com/emilk/egui), shipped
as a `.deb` and an AppImage, with dark and light themes.

**Status:** milestone M4. The unprivileged Linux backends (`hwmon`, `cpu`, `intel_gpu`,
`block`, `memory`, `network`, `power_supply`, plus lm-sensors labels) read real hardware on
Linux; the Sensor Status window is complete (a tree-table with sub-groups, per-kind
icons, context menus, a quick filter, a toolbar and a settings dialog, with every setting
remembered between runs); every sensor has a live graph, in its own window or in a panel
of the main window; and the privileged helper adds the root-only sensors (RAPL power,
core VIDs and package C-states from MSRs, S.M.A.R.T., GPU engine load). Logging, tray
values and alerts arrive in M5, packages in M6. See [PLAN.md](PLAN.md) for the full design
and the milestone list.

## Building

Requires a stable Rust toolchain (`rustup`), and on Linux the packages
`libsensors-dev clang pkg-config libxkbcommon-dev libgl1-mesa-dev libwayland-dev`.

```bash
cargo run -p kwinfo-app -- --mock
```

```bash
cargo run -p kwinfo-cli -- list --mock
```

`--mock` (or `KWINFO_BACKEND=mock`) selects the synthetic backend, which is also what the
app uses on macOS by default. On Linux both binaries read the real hardware:

```bash
cargo run -p kwinfo-cli -- list --ids
```

The `libsensors` cargo feature (on by default, Linux only) applies the labels, `ignore`
and `compute` statements of `/etc/sensors3.conf` and `/etc/sensors.d/`. Build with
`--no-default-features` to leave it out; nothing else needs `libsensors-dev` or `clang`.

## The Sensor Status window

- Devices are group rows; click one to collapse or expand it. Runs of similar rows
  (Core Clocks, Thread Usage, C-State Residency, Distance to TjMAX, …) are nested in
  sub-groups whose row shows the lowest minimum and highest maximum of its members.
- Right-click a sensor for Show Graph (a double-click does the same), Rename, Hide, Custom colour, Disable monitoring, Reset this sensor and Copy
  value. Right-click a device to collapse it, reset its values, disable monitoring of the
  whole device or copy its values as text; right-click the header to choose columns.
- Hidden sensors are counted in the title, `[N value(s) hidden]`. Settings → General (or
  Hidden sensors) → "Show hidden sensors" lists them greyed out so they can be un-hidden.
- Disabled sensors are not polled at all: the backends skip their files and their
  statistics are cleared until they are enabled again.
- Ctrl+F opens a quick filter over sensor, group and device names; Esc closes it.
- Column widths (drag the header dividers), visible columns, collapsed groups, names,
  colours, the theme, the temperature unit, the polling period and the window size are
  all remembered. Custom colours are stored once; each theme adjusts their lightness so
  they stay readable on its background.

The row and toolbar icons are painted vector shapes rather than Phosphor glyphs:
`egui-phosphor` has no release for egui 0.37 yet.

## Graphs

- Double-click a sensor (or right-click it, Show Graph) for a live graph like HWiNFO's:
  the readings as a filled area under a line on a grid, and beside it the top of the
  range, the current value, Auto Fit, Reset and the bottom of the range. Rows with an
  open graph show a small graph mark.
- Temperatures and loads start on a fixed 0 to 100 range; everything else starts with
  Auto Fit, which follows the readings with some headroom. Typing a value into the top or
  bottom field fixes the range (`1.5 GB`, `250M` and `48.5 °C` all work); Reset clears the
  sensor's graph, minimum, maximum and average. Hover the graph to read any point.
- Right-click a graph to choose how much of it to show (1 minute to 2 hours, within the
  history kept) or to close it. Settings → Graphs sets the history length (10 minutes
  to 2 hours; at most 14,400 readings per sensor).
- Graphs open in their own windows, which come back at their size and place after a
  restart. On Wayland the desktop decides where windows open, so the Automatic setting
  shows the graphs in a panel on the right of the main window instead; the toolbar's
  Graphs menu and Settings → Graphs switch between the two. The same menu tiles the
  graph windows beside the main window and closes them all.

## Settings

Settings live in `~/.config/kwinfo/config.toml` (`$XDG_CONFIG_HOME/kwinfo/` if set; on
macOS `~/Library/Application Support/io.github.kywa92.kwinfo/`). KWinfo writes the
file a moment after every change and on quit; it is plain TOML keyed by the stable device
and channel ids that `kwinfo-cli list --ids` prints:

```toml
[general]
temperature_unit = "Celsius"
show_hidden = false

[theme]
mode = "auto"

[polling]
interval_ms = 1000

[channels."cpu:0/clock.core0"]
label = "P-core 0"
color = "#ff8c00"
```

A file that cannot be read is kept as `config.toml.bad` and the defaults apply. For a
throwaway run, point KWinfo at another file; `--interval` and `--theme` override the
saved values for one run without changing them:

```bash
cargo run -p kwinfo-app -- --mock --config /tmp/kwinfo-test.toml --theme light
```

## Snapshots and replay

`kwinfo-cli snapshot` copies the parts of `/sys` and `/proc` the backends read into a
directory, with serial numbers, WWNs and MAC addresses replaced by placeholders:

```bash
cargo run --release -p kwinfo-cli --no-default-features -- snapshot --out my-machine
```

`--replay` runs the real Linux backends against such a tree on any OS, including macOS:

```bash
cargo run -p kwinfo-app -- --replay tests/fixtures/synthetic-8700k
```

```bash
cargo run -p kwinfo-cli -- list --replay tests/fixtures/synthetic-8700k
```

A snapshot is static, so usage and rate rows stay empty or at zero; temperatures,
voltages, fans and clocks show their captured values. `kwinfo-cli record` takes a
snapshot and then records every value that changes, frame by frame:

```bash
cargo run --release -p kwinfo-cli --no-default-features -- record --duration 2m --out my-machine
```

`--replay` plays a recording back one frame per polling tick and starts over at the end.
Rates and usage come out as recorded whatever the polling interval, because the backends
take the recorded time of each frame rather than the wall clock.

## Root-only sensors: the helper

CPU package power, core voltages, package C-states, S.M.A.R.T. data and GPU load need
root. `kwinfo-helper` reads them and nothing else, and never writes to the hardware.
KWinfo uses it as a D-Bus system service (installed by the packages, or from Settings →
Helper → Install helper…, which runs `pkexec kwinfo-helper install`), or, without an
install, started through `pkexec` for one session. The toolbar's Helper button shows its
state. To see what it reads on a machine:

```bash
sudo ./target/release/kwinfo-helper probe
```

See [docs/helper.md](docs/helper.md) for what each source needs, the files an install
writes, security notes and troubleshooting.

## Diagnostics and logs

Every KWinfo binary writes a log file per run (on Linux under `~/.local/state/kwinfo/logs/`)
that starts with a machine summary and records backend activity, user actions and panics.
To collect everything from a test machine in one file to attach to a bug report:

```bash
kwinfo-cli report
```

In the window, Settings → General → "Save diagnostic report…" writes the same report, with
the sensors as the window shows them (renamed, hidden and disabled ones marked). See
[docs/diagnostics.md](docs/diagnostics.md) for the locations, the environment variables
and what the report contains.

## Testing

```bash
cargo test --workspace
```

This includes headless interaction tests of the window (`crates/kwinfo-app/tests/
interaction.rs`): context menus, the rename and colour dialogs, the filter, columns,
collapsing, disabling monitoring, and a restart that must restore everything from
`config.toml`. kittest checks after every step that each control has an accessible name.

The backend tests parse the fixture trees in `tests/fixtures/`; the complete listing of
the synthetic i7-8700K fixture is compared with
`crates/kwinfo-core/tests/expected/synthetic-8700k.txt` (regenerate it with
`UPDATE_EXPECT=1 cargo test -p kwinfo-core --test synthetic_8700k` and review the diff).

Linux-only code can be type-checked from macOS once the target is installed
(`rustup target add x86_64-unknown-linux-gnu`):

```bash
cargo check --workspace --target x86_64-unknown-linux-gnu --no-default-features
```

Two GPU-backed test suites are ignored by default because CI runners have no GPU:

```bash
cargo test -p kwinfo-app --test snapshots -- --ignored
```

compares the Sensor Status table (default, customised with hidden, renamed, recoloured and
disabled rows, and with a context menu open, each in both themes) against the committed
pictures in `crates/kwinfo-app/tests/snapshots/` (set `UPDATE_SNAPSHOTS=true` to accept
changes), and

```bash
cargo test -p kwinfo-app --test preview -- --ignored
```

renders the whole window into `target/preview/*.png` for a quick look: both themes, the
settings tabs, the context menu, the filter, the dialogs and the replayed 8700K fixture.

## Layout

| Path | Contents |
|---|---|
| `crates/kwinfo-core` | sensor model, backends, sampler, statistics, history, settings |
| `crates/kwinfo-ipc` | the helper protocol and the client the app and CLI use |
| `crates/kwinfo-helper` | the privileged helper: providers, stdio and D-Bus service, installer |
| `crates/kwinfo-app` | the eframe GUI, binary `kwinfo` |
| `crates/kwinfo-cli` | the command line tool, binary `kwinfo-cli` |
| `crates/kwinfo-diag` | per-run log files and `kwinfo-cli report` bundles |
| `packaging/` | the helper's systemd unit, D-Bus and polkit files (desktop file and AppImage script in M6) |
| `tests/fixtures/` | sysfs/procfs trees used by tests and `--replay` |

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

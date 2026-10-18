# KWinfo — Implementation Plan

A Linux hardware monitor in the spirit of HWiNFO64's **Sensor Status** window and its
floating sensor graphs. Written in Rust, shipped as a `.deb` and an AppImage, with
first-class dark and light themes.

Plan date: 2026-10-18. Crate versions in section 11 were checked on crates.io that day.

## 0. Decisions and assumptions

Decisions taken from your answers:

| Topic | Decision |
|---|---|
| GUI toolkit | egui / eframe 0.37 (pure Rust, immediate mode) |
| Root-only data | Separate `kwinfo-helper` service: systemd + D-Bus + polkit. `pkexec` fallback for the AppImage |
| v1 scope | Sensor Status window + graph windows only. No System Summary, no hardware tree |
| First test machine | Intel Core i7-8700K + Intel UHD 630 iGPU. Later: AMD CPU + NVIDIA GPU, then Intel CPU + NVIDIA GPU |

Assumptions I made. Change any of them and the plan adjusts, nothing below is locked to them:

- **Distros:** Ubuntu 22.04+ / Debian 12+ for the `.deb`. Any glibc ≥ 2.35 distro for the AppImage. Kernel ≥ 5.15, 6.x recommended.
- **Desktops:** GNOME and KDE, on both X11 and Wayland. Tray icons on GNOME need the AppIndicator extension.
- **License:** MIT OR Apache-2.0 for KWinfo code (the Rust norm). Every dependency below is compatible.
- **Identity:** app ID `io.github.kylanwalters.KWinfo`; binaries `kwinfo`, `kwinfo-helper`, `kwinfo-cli`.
- **Development:** you work on the Mac against a mock backend and captured sensor snapshots; real sensor testing happens on the 8700K machine.
- **Default polling:** 1000 ms (HWiNFO defaults to 2000 ms). S.M.A.R.T. polled separately every 60 s.

## 1. What the reference screenshots tell us

| File | What it shows | What we take from it |
|---|---|---|
| `reference1.png` | HWiNFO v8.40, dark. Sensor Status with collapsible device groups (System, CPU, Memory Timings, Nuvoton NCT6687D, chipset, DIMMs, S.M.A.R.T., drives, GPU, UPS) and six floating graph windows | The dark palette, group rows, per-row type icons, the graph window anatomy: filled area on black with a grey grid, right-hand column with y-max, current value, Auto Fit, Reset, y-min. Bottom icon toolbar with elapsed timer |
| `reference2.png` | Dark, GPU group expanded with nested sub-groups (Rail Voltages, Rail Powers, D3D Usages, Performance Limiters), Average column, orange-highlighted Current cells, selected row outlined in blue | Nested groups, the Average column, user-set highlight colours, Yes/No boolean rows |
| `reference3.webp` | HWiNFO v6, light. Sensor Status with Average column, "0:02:10" elapsed timer | The light palette, column layout, bottom toolbar |
| `reference4.png` | HWiNFO v4, light. Full desktop incl. the right-click menu (Show Graph, Add to tray, Remove from tray, Disable/Enable monitoring, Rename, Hide), graph windows with "Fit y"/"Reset", bottom controls (Celsius/Fahrenheit, Reset Values, Logging Start, Configure, Minimize All, Save & Quit) | The context menu and the toolbar command set |
| `reference5.png` | Light, ten graph windows tiled in two columns beside the sensor list, "[35 value(s) hidden]" in the title | Tiling many graphs, hidden-values counter |
| `reference6.png` | HWiNFO v5, light. Per-core graphs, CPU Package Power graph, rows for Core #N Thread #N Usage, thermal throttling Yes/No, ratios, Distance to TjMAX, Package/IA/GT power, GPU clock | The exact CPU row set we want on the 8700K |

The System Summary and hardware tree visible in references 4 and 6 are out of scope by your decision.

## 2. v1 feature list

### Sensor Status window
1. Tree-table: device groups (collapsible, state remembered) containing channels; nested sub-groups where a device has many similar rows (e.g. "Core Clocks", "Core Usage").
2. Columns: Sensor, Current, Minimum, Maximum, Average (all since the last reset). Column widths and visibility persist.
3. Type icon per row, per-kind number formatting and units, °C/°F toggle.
4. Row interactions: select, double-click to open a graph, right-click menu with Show Graph, Add to tray / Remove from tray, Rename, Hide, Custom colour, Disable monitoring / Enable monitoring (per channel and per device), Reset this sensor, Copy value.
5. "[N value(s) hidden]" in the title; a "Show hidden" switch in settings to reveal and un-hide rows.
6. Bottom toolbar: Expand/Collapse all, Reset Min/Max/Avg, Logging start/stop, elapsed timer, polling period, Settings, Minimise to tray, Quit. Title also shows KWinfo's own CPU cost like HWiNFO's "(0.2%)".
7. Helper status indicator with a one-click fix (install / start helper) when root-only sensors are unavailable.
8. Quick filter (Ctrl+F) over sensor names. Nice to have.
9. CSV logging: ISO timestamp column, one column per selected channel, interval ≥ polling period, daily rotation optional.
10. Tray values: one StatusNotifierItem per chosen sensor, rendering the value as text with its colour, tooltip with the label, menu with Show/Hide window and Quit.
11. Alerts: per-sensor threshold with desktop notification and row highlight. Ships in v1 if time allows, else v1.1.
12. Settings: polling period, S.M.A.R.T. period, theme (Auto/Dark/Light), temperature unit, start minimised, autostart, remember layout, logging defaults, helper management, hidden/disabled sensor lists.

### Graph windows
1. One per channel, opened from a row. Native floating windows (egui viewports). Title is the sensor label.
2. Content: filled area plus line, horizontal grid, time axis showing the last N minutes. Right column: y-max (editable), current value, Auto Fit toggle, Reset, y-min (editable). Hover shows value and time.
3. History length configurable, default one hour at the polling rate.
4. Size always persisted; position persisted where the platform allows (X11). "Tile graph windows" and "Close all graphs" commands.
5. "Graphs panel" mode: the same graphs as internal windows inside the main window, freely positioned and tileable. This is the answer for Wayland and tiling window managers.
6. Default colour per sensor kind, per-graph override, both theme-aware.

## 3. Architecture

### 3.1 Workspace layout

```
KWinfo/
├─ Cargo.toml                      workspace
├─ rust-toolchain.toml             pinned stable
├─ crates/
│  ├─ kwinfo-core/                 sensor model, backends, sampler, stats, history, config, CSV logging
│  ├─ kwinfo-ipc/                  wire types shared by app and helper (serde)
│  ├─ kwinfo-helper/               privileged helper: D-Bus service, --stdio, --install
│  ├─ kwinfo-app/                  eframe GUI, binary `kwinfo`
│  └─ kwinfo-cli/                  binary `kwinfo-cli`: list, watch, json, snapshot, doctor
├─ packaging/
│  ├─ appimage/                    build.sh, AppRun, icon
│  ├─ systemd/kwinfo-helper.service
│  ├─ dbus/                        io.github.kylanwalters.KWinfo.Helper.conf and .service
│  ├─ polkit/                      io.github.kylanwalters.KWinfo.policy
│  └─ desktop/                     .desktop, icons/hicolor/..., metainfo.xml
├─ tests/fixtures/                 captured sysfs snapshots (8700K first), used by tests and the replay backend
├─ docs/                           sensors-detect, kernel module hints, troubleshooting, man page
└─ .github/workflows/              ci.yml, release.yml
```

### 3.2 Sensor data model (kwinfo-core)

```rust
pub struct DeviceId(String);   // stable across reboots, e.g. "cpu:0", "hwmon:nct6775:nct6775.656",
                               // "drm:0000:00:02.0", "block:<model>:<hash of WWID or serial>"
pub struct ChannelId(String);  // "<device>/<channel>", e.g. "hwmon:coretemp:coretemp.0/temp1_input"

pub enum Kind { Temperature, Voltage, Current, Power, Energy, Fan, Pwm, Clock, Usage,
                Throughput, Capacity, Count, Boolean, Other }
pub enum Category { Cpu, Motherboard, Gpu, Drive, Smart, Memory, Network, Battery, Ups,
                    Psu, Cooler, Other }

pub struct Device  { id: DeviceId, label: String, category: Category, order: u16 }
pub struct Channel { id: ChannelId, device: DeviceId, label: String, kind: Kind,
                     unit: Unit, decimals: u8, source: Source /* Unprivileged | Helper */ }
pub struct Sample  { channel: ChannelId, value: Option<f64>, at: Instant }
pub struct Stats   { current: Option<f64>, min: f64, max: f64, sum: f64, n: u64 }  // reset-able
pub struct History { ring: VecDeque<(f64 /* secs since start */, f32)>, cap: usize }
```

Stable IDs matter because hwmon numbering changes between boots. The ID is built from the
driver name and the device's bus path (the `hwmon*/device` symlink target), never from `hwmonN`.
All per-sensor user settings (hidden, renamed, colour, tray, disabled, alert, graph layout) are
keyed by `ChannelId`.

Backend trait:

```rust
pub trait Backend: Send {
    fn name(&self) -> &'static str;
    fn enumerate(&mut self) -> Result<Vec<(Device, Vec<Channel>)>>;  // at start and on rescan
    fn sample(&mut self, out: &mut Vec<Sample>) -> Result<()>;      // every tick, must be cheap
    fn rescan_every(&self) -> Option<Duration> { Some(Duration::from_secs(60)) }
}
```

### 3.3 Sampling pipeline

- **Sampler thread** owns all backends. Every tick it calls `sample()` on each, folds the
  samples into `Stats` and `History`, then publishes an immutable `Snapshot` through
  `arc_swap::ArcSwap` and calls `egui::Context::request_repaint()`.
- Enumeration happens once at start and on a slow rescan (60 s, or on udev events later) so
  the per-tick work is only reading known files. sysfs reads are a few microseconds each.
- **Helper client** is just another backend. It subscribes over D-Bus and receives pushed
  readings; the sampler merges the latest set each tick.
- **Logger** runs in the sampler thread: on each logging interval it appends one CSV row.
- **UI thread** only reads the current `Snapshot`. Repaints are driven by ticks, not by a
  timer, so an idle KWinfo costs almost nothing.
- Backends degrade gracefully: unreadable channels show "n/a", errors are logged once.
- Budget: enumerate ≤ 200 ms, tick ≤ 5 ms for ~500 channels, idle UI frame ≤ 2 ms,
  < 1 % CPU at 1 s polling, < 100 MB RAM with 20 graphs open.

### 3.4 Unprivileged backends

| Backend | Source | Provides | Notes |
|---|---|---|---|
| `hwmon` | `/sys/class/hwmon/*` | Everything the kernel hwmon drivers expose: Super I/O chips (`nct6775`, `nct6683`, `it87`), `coretemp`, `k10temp`, `acpitz`, `nvme`, `drivetemp`, `amdgpu`, `i915`/`xe`, `corsair-psu`, `nzxt-*`, `asus-ec-sensors`, `jc42`/`spd5118` DIMM temps, `iwlwifi`, batteries | Reads `temp*/in*/curr*/power*/fan*/pwm*/energy*/freq*_input` plus `_label`, `_crit`, `_max`, `_alarm`. Scales milli/micro units. Needs the right modules loaded (section 3.6) |
| `cpu` | `/proc/stat`, `/proc/cpuinfo`, `/sys/devices/system/cpu` | Total CPU usage, max thread usage, per-thread usage, per-core clock from `cpufreq/scaling_cur_freq` (APERF/MPERF-based on `intel_pstate`), ratio, C-state residency from `cpuidle/state*/time`, throttle counts from `thermal_throttle/*` as Yes/No | Topology from `topology/core_id` gives HWiNFO-style "Core #N Thread #M" names |
| `intel_gpu` | `/sys/class/drm/card*/gt_*_freq_mhz` (newer kernels: `gt/gt0/rps_*`), `i915` hwmon where present | GPU clock (actual and requested), min/max/boost clocks | Busy % and GT power need the helper (section 3.5) |
| `block` | `/sys/class/nvme`, `/sys/block/*/stat`, `/proc/diskstats` | Drive identity, read/write rate, activity % | NVMe temps arrive via `hwmon`; S.M.A.R.T. via the helper |
| `memory` | `/proc/meminfo` | Physical memory used/available, load %, swap | |
| `network` | `/sys/class/net/*/statistics` | Download/upload rate and totals per interface | Loopback and virtual interfaces hidden by default |
| `power_supply` | `/sys/class/power_supply` | Batteries, AC presence, USB HID UPS devices the kernel exposes as type UPS | NUT integration is post-v1 |
| `libsensors` overlay | libsensors via the `lm-sensors` crate | Labels and `compute` rules from `/etc/sensors3.conf` and `/etc/sensors.d` | Cargo feature, on by default on Linux. Section 3.6 |
| `mock` / `replay` | synthetic or captured fixture tree | Development on macOS, CI, unit tests | Section 8 |

All Linux-only code is behind `cfg(target_os = "linux")` so the app builds and runs on the
Mac with the mock backend.

### 3.5 The privileged helper (`kwinfo-helper`)

Why it exists: since 2020 the kernel makes RAPL energy counters root-only (CVE-2020-8694),
MSR access needs `CAP_SYS_RAWIO`, `smartctl` needs raw device access, and the i915
performance counters need `CAP_PERFMON`. The GUI stays an ordinary user process.

**Providers inside the helper (Intel first):**

- **RAPL** from `/sys/class/powercap/intel-rapl:*` (`energy_uj`, `name`,
  `max_energy_range_uj` for wrap-around). Power = Δenergy / Δtime. On the 8700K the domains
  map to HWiNFO's rows: `package-0` → CPU Package Power, `core` → IA Cores Power,
  `uncore` → GT Cores Power, `dram` → DRAM Power, `psys` if present. The same driver covers
  AMD Zen 2+ later.
- **MSR** via `/dev/cpu/N/msr` (helper loads the `msr` module): `IA32_PERF_STATUS` → per-core
  VID, `IA32_THERM_STATUS` → PROCHOT / thermal status, package C-state residency counters.
- **S.M.A.R.T.** via `smartctl -j` (JSON, smartmontools ≥ 7.0): temperature, remaining life /
  percentage used, power-on hours, host reads/writes, reallocated and pending sectors,
  critical warnings, giving HWiNFO-style "Drive Failure" / "Drive Warning" rows. Uses
  `-n standby` so spun-down disks are not woken. Polled on its own slow interval.
- **i915 PMU** via `perf_event_open` on the `i915` event source: render/video/blitter
  busy %, RC6 residency, actual and requested frequency → "GPU Core Load" and friends.
  The `xe` driver for newer Intel GPUs has an equivalent PMU.

**Interface:** D-Bus system bus, name `io.github.kylanwalters.KWinfo.Helper`, interface
`io.github.kylanwalters.KWinfo.Helper1`. Methods `Capabilities()`, `Enumerate() → s` (JSON),
`Subscribe(interval_ms, smart_interval_s)`, `Unsubscribe()`, signal `Samples(s)` (JSON).
JSON payloads keep the exact same message types usable over the stdio transport. `zbus` 6
on both sides. The helper exits after two minutes without subscribers and is D-Bus activated
(`SystemdService=`), so it costs nothing while KWinfo is not running.

**Authorization:** polkit action `io.github.kylanwalters.KWinfo.read-sensors` with
`allow_active=yes`, `allow_inactive=no`, `allow_any=no`: a logged-in local user gets the data
with no password, remote or inactive sessions do not. Checked once per caller and cached until
that bus name disappears. The helper never writes to hardware.

**Hardening:** runs as root but with systemd sandboxing: `NoNewPrivileges`,
`ProtectSystem=strict`, `ProtectHome`, `PrivateTmp`, `RestrictAddressFamilies=AF_UNIX`,
`SystemCallFilter=@system-service`, a capability bounding set limited to what `msr`,
`smartctl` and `perf_event_open` need. Energy counters are never exported raw; the helper
reports averaged power at ≥ 250 ms granularity, which blunts the side channel that made the
kernel restrict them.

**AppImage path (no system service installed):**

1. The app detects the missing D-Bus service and shows a Setup dialog.
2. "Install helper (recommended)" runs `pkexec kwinfo-helper --install` once: copies the
   binary to `/usr/local/libexec/kwinfo/`, writes the unit, D-Bus and polkit files, runs
   `systemctl daemon-reload`. From then on the AppImage behaves like the `.deb`.
   `--uninstall` reverses it.
3. "Run helper for this session" runs the helper via `pkexec` in `--stdio` mode
   (newline-delimited JSON over pipes), one password prompt per launch. Because root cannot
   read a user's FUSE-mounted AppImage, the app first copies the helper to
   `$XDG_RUNTIME_DIR/kwinfo/` and the pkexec'd shell installs it to a root-owned location
   before executing it.

### 3.6 Using lm-sensors

lm-sensors is two things and KWinfo uses both:

- **Kernel modules are the real source of motherboard sensors.** The app and `kwinfo-cli doctor`
  detect "no Super I/O chip found" and point at `sudo sensors-detect` plus
  `/etc/modules-load.d/`. Chip guidance to document: Nuvoton NCT67xx → `nct6775` (in-tree);
  NCT6687D / NCT6686D on MSI boards → `nct6683` with `force=1` (in-tree, read-only, tested
  board list in the kernel docs); ITE IT86xx/IT87xx (many Gigabyte boards) → out-of-tree
  `it87` via DKMS, blocked by Secure Boot unless signed; ASUS → `asus-ec-sensors` /
  `asus_wmi_sensors` (in-tree); DDR4 DIMM temps → `jc42` + `i2c-i801`; DDR5 → `spd5118`
  (kernel 6.11+); SATA drive temps → `drivetemp`.
- **libsensors for labels and compute rules.** At enumeration the `libsensors` feature
  iterates chips through the `lm-sensors` crate, matches each to a hwmon device by sysfs path,
  takes `sensors_get_label()` as the row label and uses `sensors_get_value()` for channels
  that have a `compute` rule (voltage dividers such as +12V). Everything else reads raw sysfs.
  This honours `/etc/sensors3.conf`, `/etc/sensors.d/*.conf` and `ignore` lines, so a user's
  existing lm-sensors setup carries straight over.

Packaging consequences: the `.deb` depends on `libsensors5` (already on every Mesa-based
desktop) and recommends `lm-sensors` and `smartmontools`; the AppImage bundles `libsensors.so.5`
(LGPL-2.1, tiny) or is built with `LMSENSORS_STATIC=1`. Building needs `libsensors-dev` and
`clang` on Linux; on macOS the feature is compiled out.

### 3.7 Persistence

`~/.config/kwinfo/config.toml` (XDG), logs default to `~/.local/share/kwinfo/logs/`.
Sections: `general`, `theme`, `polling`, `columns`, `devices` (collapsed state, order,
disabled), `channels` (per `ChannelId`: hidden, label, colour, tray, disabled, alert),
`graphs` (channel, size, position, y-range, autofit, panel-or-window). Saved debounced on
change and on quit ("Save & Quit").

## 4. UI design

### 4.1 Sensor Status window

```
┌ KWinfo – Sensor Status [3 value(s) hidden] (0.3%) ────────────────────── – □ × ┐
│ Sensor                               │ Current   │ Minimum  │ Maximum  │ Average │
│ ▾ ▣ CPU [#0]: Intel Core i7-8700K    │           │          │          │         │
│     🌡 CPU Package                    │  48.0 °C  │ 43.0 °C  │ 70.0 °C  │ 47.2 °C │
│     🌡 Core #0 … Core #5              │  46.0 °C  │ …        │          │         │
│     🌡 Core Max / Distance to TjMAX   │           │          │          │         │
│     ⚡ CPU Package Power   (helper)    │  2.645 W  │ 2.607 W  │ 12.834 W │ 3.1 W   │
│     ◔ Total CPU Usage                 │  17.1 %   │          │          │         │
│     ⏱ Core #0 Clock                   │ 4300.0 MHz│          │          │         │
│ ▸ ▤ ASUS PRIME Z370-A (Nuvoton NCT6793D)                                        │
│ ▸ ▦ GPU [#0]: Intel UHD Graphics 630                                            │
│ ▸ ▬ Drive: Samsung SSD 970 EVO 1TB                                              │
│ ▸ ▬ S.M.A.R.T.: Samsung SSD 970 EVO 1TB                                         │
│ ▸ ▥ Memory      ▸ ⇅ Network: enp3s0                                             │
├──────────────────────────────────────────────────────────────────────────────────┤
│ [⇕] [↺ Reset] [▤ Log]  0:09:30  [⏱ 1000 ms] [⚙]                      [▁ Tray] [×]│
└──────────────────────────────────────────────────────────────────────────────────┘
```

- Built with `egui_extras::TableBuilder`: striped, resizable columns, sticky header, and
  virtualised rows so thousands of rows paint only what is visible.
- egui has no tree-table widget, so the tree is a flattened list of visible nodes computed
  from the collapse state each frame; rows get an indent and a chevron. Cheap and simple.
- Right-click opens an egui popup menu with the commands from section 2.
- Row icons are Phosphor glyphs (`egui-phosphor`), tinted by `Kind`: thermometer, lightning,
  fan, gauge, clock, hard drive, CPU, graphics card, memory, battery, plug, network.
- Numbers are right-aligned with fixed decimals per kind (temps 1, V and W 3, MHz 1, % 1,
  RPM 0) and tabular figures.
- The elapsed timer counts since start or last reset, like HWiNFO.

### 4.2 Graph windows

```
┌ CPU Package ──────────────────────────────── × ┐
│ ┌────────────────────────────────┐  [ 100.0  ] │
│ │ grid · filled area · 1.5 px line│  48.0 °C   │
│ │                                │  [Auto Fit] │
│ │                                │  [ Reset  ] │
│ └────────────────────────────────┘  [  0.0   ] │
└────────────────────────────────────────────────┘
```

- Each graph is an eframe deferred viewport (`show_viewport_deferred`), so it is a real
  native window the window manager can move, tile and close. Deferred viewports repaint on
  their own; they read the shared `Snapshot` and are told to repaint on each tick.
- `egui_plot::Plot` with a `Line` and `fill`, horizontal grid, x axis in seconds ago, no
  legend. Auto Fit on: bounds follow the data. Auto Fit off: the two edit fields set the y
  range. Reset clears the history and the min/max markers for that sensor.
- Wayland does not let applications choose window positions. Sizes are always saved;
  positions are restored on X11, and on Wayland the compositor's own placement rules apply.
  The Graphs panel mode (graphs as `egui::Window`s inside the main viewport) gives full
  control everywhere and is the default when the session is Wayland.
- Tile command arranges windows in an N-column grid like `reference5.png`.

### 4.3 Tray

`ksni` (pure Rust StatusNotifierItem over D-Bus; works on KDE, GNOME with AppIndicator,
Cinnamon, XFCE). For each tray sensor the app rasterises the value text into a small RGBA
bitmap (`ab_glyph`) whenever the text changes and publishes it as the icon. Tooltip shows the
full label; the menu has Show/Hide, Remove from tray, Quit. Closing the main window while
tray icons exist minimises to the tray instead of quitting.

### 4.4 Settings and setup dialogs

Modal `egui::Window`s with tabs: General, Theme, Columns, Logging, Helper, Hidden sensors,
Alerts. The Setup dialog (section 3.5) appears automatically when sensors are missing and
lists concrete fixes: load a module, run `sensors-detect`, install `smartmontools`, install
the helper.

## 5. Dark and light mode

### 5.1 Behaviour

- Setting `Theme = Auto | Dark | Light`, default Auto.
- Auto follows the desktop through `dark-light` 3.0, which reads the
  `org.freedesktop.appearance.color-scheme` key from xdg-desktop-portal and can subscribe to
  changes, so switching the desktop theme switches KWinfo live, including open graph windows
  and tray bitmaps. Fallback order when no portal is running: eframe's `system_theme()`,
  then Dark.
- One `Theme` struct of semantic colour tokens, two constants `DARK` and `LIGHT` modelled on
  the references. Tokens map into `egui::Visuals` (window and panel fill, faint and extreme
  backgrounds, widget fills and strokes, selection, striped rows) plus KWinfo's own table and
  graph colours. No widget hard-codes a colour.
- Per-kind graph colours exist in both palettes. A user's custom colour is stored once and
  its lightness is clamped per theme so it stays readable on both backgrounds; a per-theme
  override is available in the colour picker.
- Fonts: the system UI font through eframe's `system_fonts` feature with bundled Noto Sans as
  fallback; 13 px table text, 12 px toolbar.

### 5.2 Palette tokens (starting values, to be tuned against the screenshots)

| Token | Dark (ref 1, 2) | Light (ref 3–6) |
|---|---|---|
| window background | `#1b1b1b` | `#f0f0f0` |
| table background | `#202020` | `#ffffff` |
| group header row | `#2a2a2a` | `#f3f3f3` |
| alternate row | `#242424` | `#f8f8f8` |
| hover row | `#2f2f2f` | `#eaf2fb` |
| selection | `#1f3a5f` fill, `#3b82f6` outline | `#cce8ff` fill, `#0a64c2` outline |
| text primary / secondary | `#e6e6e6` / `#a0a0a0` | `#1a1a1a` / `#5c5c5c` |
| grid lines and borders | `#3a3a3a` | `#d0d0d0` |
| Current column tint | `#243040` | `#eef4fb` |
| accent | `#4da3ff` | `#0a64c2` |
| highlight / alert | `#ff7a1a` | `#ff8c00` |
| graph background / grid | `#0d0d0d` / `#333333` | `#e9f3fb` / `#a9d0ec` |
| graph: temperature | `#b22222` | `#e57373` |
| graph: voltage | `#d4a017` | `#c9a227` |
| graph: power | `#1e90ff` | `#7986cb` |
| graph: fan | `#22a06b` | `#66bb6a` |
| graph: usage | `#2ea8a0` | `#8fb98f` |
| graph: clock | `#8a7dd6` | `#9b8fd8` |
| graph: other | `#5b7fb8` | `#90a4ae` |

Graph fills use the series colour at 35 % alpha under a 1.5 px line at full colour. Every
text/background pair must reach 4.5:1 contrast in both palettes.

### 5.3 Verification

- `egui_kittest` snapshot tests render the Sensor Status table and one graph from the replay
  backend in both themes; PNGs live in `tests/snapshots/{dark,light}/` and CI diffs them.
- Manual checklist per release: both themes on GNOME and KDE, Wayland and X11, live theme
  switch with graphs open, tray bitmaps on light and dark panels.

## 6. Hardware support

### 6.1 First target: i7-8700K + UHD 630 (Coffee Lake)

| Group | Rows | Source |
|---|---|---|
| CPU [#0]: Intel Core i7-8700K | CPU Package, Core #0–#5, Core Max, Distance to TjMAX | `coretemp` hwmon |
| | Total CPU Usage, Max CPU/Thread Usage, Core #N Thread #M Usage ×12, Core #N Clock, Core #N Ratio, C-state residency per state, Core/Package Thermal Throttling (Yes/No) | `cpu` backend |
| | CPU Package Power, IA Cores Power, GT Cores Power, DRAM Power | helper (RAPL) |
| | Core #N VID, PROCHOT, Package C-state residency | helper (MSR) |
| Motherboard (board name from `/sys/class/dmi/id/board_name`, readable unprivileged) | Vcore, +12V, +5V, +3.3V, VBAT, …; CPU/System/PCH/VRM temps; CPU/Chassis/Pump fans; intrusion | Super I/O hwmon driver + libsensors labels |
| GPU [#0]: Intel UHD Graphics 630 | GPU Clock (actual), GPU Requested Clock, Max/Boost clock | `intel_gpu` backend |
| | GPU Core Load, Video Engine Load, RC6 residency, GT Cores Power | helper (i915 PMU, RAPL) |
| Drive: <model> | Read rate, Write rate, Total Activity %, NVMe temperature | `block` + `hwmon` |
| S.M.A.R.T.: <model> | Temperature, Remaining Life, Power-On Hours, Host Writes/Reads, Reallocated/Pending Sectors, Drive Failure, Drive Warning | helper (`smartctl -j`) |
| Memory | Physical Memory Used / Available / Load, Swap; DIMM temps if `jc42` is loaded | `memory`, `hwmon` |
| Network: <iface> | Download rate, Upload rate, totals | `network` |
| Other | `acpitz` and PCH temps, Wi-Fi card temp, PSU or AIO rows if an in-tree driver exists | `hwmon` |

A Z370 board with a Nuvoton chip works out of the box with `nct6775`. If the board has an
ITE chip, only `acpitz` temperatures appear until the `it87` DKMS module is installed; the
Setup dialog says so.

### 6.2 Later targets

- **AMD Ryzen:** `k10temp` (Tctl, Tdie, Tccd*), `amd_pstate` clocks, RAPL via the same
  powercap driver on Zen 2+ (helper), MSR effective clocks. Optional out-of-tree
  `zenpower3` (Zen 1–3 voltages/currents) and `ryzen_smu` (Zen 2–5 per-core power and VID,
  HWiNFO's "Enhanced" rows) behind a feature that activates when the module is present.
- **NVIDIA:** `nvml-wrapper` 0.13 loads `libnvidia-ml` at runtime, so there is no hard
  dependency and it works in the AppImage. Temps (GPU, memory junction where supported),
  fan % and RPM, power draw and limit, graphics/memory/video/SM clocks, GPU/memory/encoder/
  decoder utilisation, VRAM used/total, PCIe throughput, performance limiters from the clock
  throttle reasons. The GPU section in `reference2.png` maps almost one-to-one onto NVML.
  Nouveau shows hwmon only.
- **AMD Radeon:** `amdgpu` hwmon (edge/junction/memory temps, fan, power, VDDGFX) plus sysfs
  (`gpu_busy_percent`, `mem_busy_percent`, VRAM used/total, `pp_dpm_sclk/mclk`) and the
  binary `gpu_metrics` table for per-rail currents, effective clocks and throttle status.
  Parse `gpu_metrics` ourselves or use the MIT-licensed `libdrm_amdgpu_sys`; the `amdgpu-sysfs`
  crate is LGPL-3 and would complicate static linking.
- **Intel Arc:** `xe` hwmon (power, energy, voltage, temps) plus the `xe` PMU.

### 6.3 Honest limits versus HWiNFO on Windows

HWiNFO ships its own drivers; KWinfo depends on kernel drivers. Say so in the README.
Partial or unavailable: memory timings (only SPD-declared values via `ee1004`/`spd5118`),
AMD "Enhanced" sensors without `ryzen_smu`, NVIDIA per-rail voltages (NVML does not expose
them), Super I/O chips with no in-tree driver, chipset temperatures on boards without a
vendor driver, UPS data beyond what USB HID exposes (NUT backend planned for v1.x), USB-only
coolers not covered by in-tree hwmon drivers (`liquidctl` bridge planned later).

## 7. Packaging

### 7.1 `.deb` with cargo-deb 3.8

Metadata lives in `crates/kwinfo-app/Cargo.toml`:

```toml
[package.metadata.deb]
name = "kwinfo"
section = "utils"
depends = "$auto, libsensors5, polkitd | policykit-1, dbus"
recommends = "lm-sensors, smartmontools"
assets = [
  ["target/release/kwinfo",        "usr/bin/",            "755"],
  ["target/release/kwinfo-cli",    "usr/bin/",            "755"],
  ["target/release/kwinfo-helper", "usr/libexec/kwinfo/", "755"],
  ["packaging/desktop/io.github.kylanwalters.KWinfo.desktop",      "usr/share/applications/", "644"],
  ["packaging/desktop/io.github.kylanwalters.KWinfo.metainfo.xml", "usr/share/metainfo/",     "644"],
  ["packaging/desktop/icons/hicolor/scalable/apps/io.github.kylanwalters.KWinfo.svg", "usr/share/icons/hicolor/scalable/apps/", "644"],
  ["packaging/dbus/io.github.kylanwalters.KWinfo.Helper.conf",    "usr/share/dbus-1/system.d/",         "644"],
  ["packaging/dbus/io.github.kylanwalters.KWinfo.Helper.service", "usr/share/dbus-1/system-services/",  "644"],
  ["packaging/polkit/io.github.kylanwalters.KWinfo.policy",       "usr/share/polkit-1/actions/",        "644"],
  ["docs/kwinfo.1", "usr/share/man/man1/", "644"],
]
[package.metadata.deb.systemd-units]
unit-scripts = "packaging/systemd"   # kwinfo-helper.service; D-Bus activated, so enable/start are optional
```

Build all crates first (`cargo build --release --workspace`), then
`cargo deb -p kwinfo-app --no-build`. `$auto` fills in the ldd-derived dependencies
(`libxkbcommon0`, `libgl1`, …). Run `lintian` in CI. Build on Ubuntu 22.04 so the package
installs on 22.04+ and Debian 12+.

### 7.2 AppImage

`packaging/appimage/build.sh`: release build → assemble `AppDir` (`usr/bin/kwinfo`,
`usr/bin/kwinfo-helper`, `usr/bin/kwinfo-cli`, desktop file, icon) → `linuxdeploy` with the
AppImage output plugin. linuxdeploy copies non-system libraries (libsensors) and excludes
GL, X11, Wayland and glibc per the AppImage exclude list, which is exactly right for eframe.
Output `KWinfo-x86_64.AppImage` with zsync update information pointing at GitHub releases.
Built on Ubuntu 22.04 (glibc 2.35 floor). `cargo-appimage` 2.4 stays as a one-command local
alternative. The app detects `$APPIMAGE` to enable the helper install flow from section 3.5.

### 7.3 CI/CD (GitHub Actions)

- `ci.yml` on push and PR: `cargo fmt --check`, `cargo clippy --workspace -D warnings`,
  `cargo test --workspace` on `ubuntu-22.04` (installs `libsensors-dev clang`), the mock and
  replay tests also on `macos-latest` so the Mac development path never rots, `cargo deny`
  for licences and advisories.
- `release.yml` on tags `v*`: `.deb`, AppImage, a `kwinfo-cli` tarball, `SHA256SUMS`, GitHub
  release with notes from `CHANGELOG.md`.

## 8. Development and testing strategy

- **On the Mac:** `cargo run -p kwinfo-app -- --mock` runs the full GUI against synthetic
  devices (a 6-core CPU, a board with voltages and fans, a GPU, two drives, a NIC) whose values
  follow smooth noise so graphs look alive. `--replay <fixture>` plays back captured data.
- **Fixtures:** `kwinfo-cli snapshot` on the 8700K copies the relevant parts of `/sys` and
  `/proc` plus `smartctl -j` output (serials scrubbed) into `tests/fixtures/8700k-z370/`.
  `kwinfo-cli record --duration 120s` captures a time series. Unit tests parse every fixture;
  bug reporters can attach snapshots.
- **A Linux VM on the Mac** (UTM, Lima or OrbStack) validates packaging, D-Bus, polkit,
  systemd and theming only; VMs expose no hwmon chips.
- **On the 8700K:** compare `kwinfo-cli list` with `sensors`, `turbostat`, `intel_gpu_top`
  and `smartctl`; measure KWinfo's CPU and memory against the budgets in 3.3.
- **GUI tests:** `egui_kittest` snapshots for both themes, interaction tests for the context
  menu, hide, rename and reset.
- **Tooling from day one:** `rustfmt`, `clippy -D warnings`, `cargo deny`, `rust-toolchain.toml`,
  `CHANGELOG.md`, conventional commits.

## 9. Milestones

Each milestone ends in a runnable state. Sizes are rough solo-developer estimates.

| # | Milestone | Deliverable | Size |
|---|---|---|---|
| M0 | Bootstrap | `git init`, workspace, CI, lint config, licence, README, eframe shell with theme switch and an empty table, mock backend, `kwinfo-cli list` | ~1 week |
| M1 | Core + unprivileged backends | Model, sampler, stats, history, `hwmon`, `cpu`, `intel_gpu`, `block`, `memory`, `network`, `power_supply`, libsensors overlay, snapshot/replay, fixtures from the 8700K, unit tests | ~2 weeks |
| M2 | Sensor Status window | Tree-table, columns, icons, formatting, °C/°F, collapse state, context menu, hidden counter, toolbar, persistence | ~2 weeks |
| M3 | Graph windows | Viewports, plot, Auto Fit/Reset, y fields, history length, colours, layout persistence, tile and close-all, Graphs panel mode | ~1.5 weeks |
| M4 | Helper | `kwinfo-ipc`, D-Bus service with polkit, RAPL/MSR/S.M.A.R.T./i915 PMU providers, client backend, `--stdio` and `--install`, Setup dialog, systemd hardening | ~2 weeks |
| M5 | Logging, tray, alerts | CSV logger, `ksni` tray with value bitmaps, alerts and notifications, autostart, start minimised | ~1.5 weeks |
| M6 | Packaging and 0.1.0 | cargo-deb, AppImage, release workflow, docs for modules/sensors-detect/it87/nct6683, first release | ~1 week |

Post-v1, in order: AMD CPU, NVIDIA GPU, AMD GPU, NUT for UPS, `liquidctl` bridge, Flatpak,
on-screen overlay, remote JSON API.

## 10. Risks and mitigations

- **Wayland window placement** → Graphs panel mode as the Wayland default; native windows remain available.
- **`egui_dock` lags egui** (0.21 targets egui 0.36) → do not depend on it in v1; the panel mode uses plain `egui::Window`s.
- **No in-tree driver for the board's Super I/O chip** → Setup dialog with `sensors-detect` and DKMS instructions; everything else keeps working.
- **Root-only sensors** → the helper; polkit grants active local sessions without a password; AppImage users get one-time `--install`.
- **i915 busy % needs CAP_PERFMON** → helper provider; the row is simply absent without it.
- **libsensors build dependencies** → Cargo feature, compiled out on macOS, CI installs them.
- **glibc floor for the AppImage** → build on Ubuntu 22.04; `cargo zigbuild` can lower it further if needed.
- **Licences** → stay with MIT/Apache crates; libsensors is LGPL-2.1 and dynamically linked or bundled; avoid statically linking LGPL-3 crates.
- **Secure Boot** blocks unsigned DKMS modules (`it87`, `zenpower3`, `ryzen_smu`) → document signing or the trade-off.
- **GNOME tray needs an extension** → documented; KWinfo works without a tray.
- **Scope creep** → HWiNFO's long tail of per-sensor options stays out of v1; the list in section 2 is the contract.

Open questions, none blocking (assumed answers in section 0): licence choice, app ID and GitHub
location, default polling period, minimum kernel, whether alerts land in v1 or v1.1.

## 11. Crates (versions checked on crates.io, 2026-10-18)

| Crate | Version | Role |
|---|---|---|
| `egui`, `eframe`, `egui_extras`, `egui_plot` | 0.37.0 | GUI, table, plots, native viewports |
| `egui-phosphor` | 0.14 | icon glyphs |
| `egui_kittest` | 0.37 (ships with egui) | GUI snapshot and interaction tests |
| `lm-sensors` (+ `sensors-sys`) | 0.5.1 | libsensors labels and compute rules (feature) |
| `zbus` | 6.0 | D-Bus for the helper service and client |
| `ksni` | 0.3.6 | StatusNotifierItem tray |
| `dark-light` | 3.0 | desktop theme detection and change subscription |
| `notify-rust` | 4.18 | alert notifications |
| `nvml-wrapper` | 0.13 | NVIDIA (post-v1) |
| `serde`, `toml`, `csv`, `directories` | 1.x / 1.1 / 1.4 / 6.0 | config, logging, XDG paths |
| `raw-cpuid` | 11.6 | CPU identification |
| `tokio`, `arc-swap`, `crossbeam-channel`, `parking_lot` | current | runtime and concurrency |
| `ab_glyph` | current | tray value bitmaps |
| `thiserror`, `anyhow`, `tracing`, `tracing-subscriber`, `libc`, `nix` | current | infrastructure |
| `cargo-deb` | 3.8 | `.deb` |
| `linuxdeploy` + AppImage plugin, `cargo-appimage` | continuous / 2.4 | AppImage |

Deliberately not used: `sysinfo` (too coarse for HWiNFO-grade rows), `egui_dock` (version lag),
`tray-icon` (pulls GTK or libappindicator unless its `ksni` feature is used; using `ksni`
directly is simpler), `amdgpu-sysfs` (LGPL-3).

## 12. Next step

Start M0: initialise the repository, create the workspace and CI, and get the eframe shell
running on the Mac with the mock backend and both themes.

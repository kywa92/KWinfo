# Sensor fixtures

Trees of `/sys` and `/proc` (plus a `pci.ids` excerpt) that the backend tests parse and
that `--replay` runs the real Linux backends against, on any OS. Symlinks keep their
original relative targets, so a fixture resolves exactly like the machine it came from.

## `synthetic-8700k/`

Hand-built, because the first test machine was not reachable when milestone M1 was
written. It models the PLAN.md section 6.1 machine as the kernel would expose it:

| Part | What the fixture contains |
|---|---|
| CPU | Intel Core i7-8700K, 6 cores / 12 threads: `/proc/cpuinfo`, `/proc/stat`, topology, `intel_pstate` cpufreq policies, `intel_idle` C-states (POLL, C1, C1E, C3, C6, C7s, C8, C9, C10), `thermal_throttle` counters |
| `coretemp` | `hwmon1`: Package id 0 and Core 0–5, high 80 °C, crit (TjMAX) 100 °C |
| Super I/O | `hwmon2`: `nct6793` at `nct6775.656` with in0–in14, ten labelled temperatures (SYSTIN, CPUTIN, AUXTIN0–3, PECI Agent 0, PCH_*), fan1–6, pwm1–6, intrusion0/1, and the min/max/alarm files the driver creates |
| Board | DMI for an ASUS PRIME Z370-A with ASUS's usual product-name placeholders |
| iGPU | `card0` at `0000:00:02.0` (8086:3e92, `i915`) with both `gt_*_freq_mhz` and `gt/gt0/rps_*_freq_mhz`, a DP connector and a render node |
| NVMe | Samsung 970 EVO Plus: controller `nvme0`, namespace `nvme0n1`, `nvme` hwmon chip (Composite, Sensor 1, Sensor 2) |
| SATA | Samsung 860 EVO `sda` with the 16-character SCSI model, `wwid`, and VPD pages 0x80, 0x83 and 0x89 (binary, with the full ATA model) |
| Other | `loop0` (virtual, skipped), `acpitz` thermal zone, `/proc/meminfo` (32 GB, 2 GB swap), `/proc/diskstats` |
| Network | `enp0s31f6` (I219-V) plus `lo` and `docker0`, which the backend hides |
| Battery | `BAT0` and an `AC` adapter. A desktop has none; they are there so that the `power_supply` backend has something to read |

The serial numbers, WWNs and MAC addresses in it are made up; they exist so that the
snapshot tests can check that they are scrubbed.

## `8700k-z370/` (to be captured)

The real i7-8700K machine, captured with `kwinfo-cli snapshot`. On that machine, as the
normal desktop user (not root), from a checkout of this repository:

```bash
cargo run --release -p kwinfo-cli --no-default-features -- snapshot --out tests/fixtures/8700k-z370
```

The command reads only an allow-list of files, replaces serial numbers, WWNs, NVMe
qualified names and MAC addresses with placeholders, drops VPD pages 0x80/0x83, blanks the
serial number and WWN inside VPD page 0x89, never copies DMI serials or UUIDs, and writes
`kwinfo-snapshot.txt` listing what it scrubbed and what it could not read. Check the
result, then commit it:

```bash
cargo run -p kwinfo-cli -- list --replay tests/fixtures/8700k-z370
```

Before capturing, make sure the Super I/O driver is loaded (`sensors` lists an `nct67xx`
or `it87xx` chip; if not, run `sudo sensors-detect`). Saving `sensors -u` output next to
the snapshot (`sensors -u > tests/fixtures/8700k-z370/sensors-u.txt`) records libsensors'
own view of the chips for later comparison.

## Recordings

`kwinfo-cli record` writes a snapshot plus `kwinfo-recording.txt`, the values that change
over time (frame 0 holds all of them, later frames only the ones that moved). Replaying it
shows live-looking usage, rates and temperatures. Two minutes at one frame per second come
to a few megabytes on a desktop, mostly idle-state times, hwmon readings and `/proc/stat`:

```bash
cargo run --release -p kwinfo-cli --no-default-features -- record --duration 2m --out ~/kwinfo-8700k-load
```

Recordings are for looking at the GUI and for bug reports; tests build their own small
recordings from `synthetic-8700k`.

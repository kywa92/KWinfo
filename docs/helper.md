# The privileged helper

Some readings are root-only on Linux: the RAPL energy counters behind CPU power (since
CVE-2020-8694), model-specific registers, S.M.A.R.T. data and the GPU's performance
counters. KWinfo itself never runs as root. `kwinfo-helper` reads those sources, and
nothing else, for it; it never writes to the hardware.

## What it reads

| Source | Rows | Needs |
|---|---|---|
| RAPL (`/sys/class/powercap/intel-rapl:*`) | CPU Package Power, IA Cores Power, GT Cores Power, DRAM Power, Platform Power | Intel, or AMD Zen 2 and later; the `intel_rapl_msr` driver |
| MSRs (`/dev/cpu/N/msr`) | Core #N VID, Core #N Power Limit Exceeded, Package Power Limit Exceeded, Package C2-C10 Residency | Intel; the `msr` module (an install loads it at every boot) |
| S.M.A.R.T. (`smartctl -j`) | a "S.M.A.R.T.: <model>" group per drive: temperature, remaining life, failure and warning, power-on hours, power cycles, host reads and writes, reallocated, pending and uncorrectable sectors (ATA), available spare, media errors and unsafe shutdowns (NVMe) | smartmontools 7.0 or later |
| i915 PMU (`perf_event_open`) | GPU Core Load, Video Engine Load, Video Enhance Load, Copy Engine Load, RC6 Residency | an Intel GPU on the `i915` driver |

RAPL and MSR rows join the CPU group, and PMU rows the GPU group, of the unprivileged
backends. Power is averaged over at least 250 ms and raw energy counters never leave the
helper. Drives are told apart by a hash of their serial number, which is never shown or
logged. Sleeping disks are queried with `-n standby`, so they stay asleep (and are tried
again every minute). Thermal throttling is not repeated: the `cpu` backend reads it from
the kernel's counters without root.

## How KWinfo reaches it

1. **System service** (recommended). The packages install it; for the AppImage or a build
   from source, Settings → Helper → *Install helper…* runs `pkexec kwinfo-helper install`
   once. D-Bus activation then starts the helper as root on KWinfo's first call, and it
   exits after two minutes without calls. polkit allows it for the active local session
   without a password (action `io.github.kywa92.KWinfo.read-sensors`); SSH and inactive
   sessions are refused.
2. **Session helper.** Settings → Helper → *Run for this session* runs
   `pkexec kwinfo-helper stdio`, which asks for an administrator password and lasts until
   KWinfo quits.

The toolbar's Helper button shows the state: green running, blue starting, grey not
installed (or not used, with the mock backend or `--replay`), orange a problem. Hover it
for details; click it for the Helper settings.

## What an install writes

`kwinfo-helper install` copies itself to `/usr/local/libexec/kwinfo/kwinfo-helper` and
writes:

| File | Purpose |
|---|---|
| `/etc/systemd/system/kwinfo-helper.service` | the service, sandboxed (read-only file system, no network, only `CAP_SYS_RAWIO`, `CAP_SYS_ADMIN` and `CAP_PERFMON`) |
| `/usr/share/dbus-1/system-services/io.github.kywa92.KWinfo.Helper.service` | D-Bus activation |
| `/etc/dbus-1/system.d/io.github.kywa92.KWinfo.Helper.conf` | only root may own the name |
| `/usr/share/polkit-1/actions/io.github.kywa92.KWinfo.policy` | who may read |
| `/etc/modules-load.d/kwinfo.conf` | loads `msr` at boot |

The list goes to `/usr/local/libexec/kwinfo/installed-files`;
`pkexec /usr/local/libexec/kwinfo/kwinfo-helper uninstall` (or *Uninstall helper…*)
removes exactly those files. An install refuses to shadow a packaged helper
(`/usr/libexec/kwinfo/kwinfo-helper`).

## Testing it on a machine

From a release build on the machine (`cargo build --release`):

```bash
sudo ./target/release/kwinfo-helper probe
```

prints every source the helper could open, why the others are missing, and two rounds
of readings. Through KWinfo's client, as a session helper (asks for a password):

```bash
./target/release/kwinfo-cli helper --session
```

Install the service, then check it the way KWinfo uses it:

```bash
pkexec "$PWD/target/release/kwinfo-helper" install
```

```bash
./target/release/kwinfo-cli helper
```

```bash
journalctl -b -u kwinfo-helper.service
```

`kwinfo-cli report` and Settings → General → *Save diagnostic report…* record the
helper's state and notes. `sudo kwinfo-cli snapshot --out <dir>` also captures the RAPL
counters and the i915 PMU's events for a fixture.

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| "The msr kernel module is not loaded" | `sudo modprobe msr`; an install adds it to `/etc/modules-load.d`. |
| "smartctl is not installed" | `sudo apt install smartmontools` (or your distribution's package). |
| "polkit did not allow it (is a polkit authentication agent running?)" | `pkexec` needs an agent to ask for the password; GNOME and KDE have one, minimal window managers may not (start e.g. `lxpolkit`). |
| "The helper was refused" over SSH | polkit allows only the active local session, on purpose. |
| No RAPL rows | the CPU or kernel has no RAPL domains in `/sys/class/powercap`. |
| No GPU load rows | not an i915 GPU (the `xe` driver and other vendors come later), or `perf_event_open` failed; see the note. |

## Security notes

- The helper only reads files and registers and runs `smartctl`; it takes no paths or
  commands from its callers, and the requests are four fixed messages.
- polkit decides per caller; D-Bus answers go to the caller only (the helper sends no
  broadcast signals, which every local user could receive).
- A session helper started from an AppImage is first copied to `$XDG_RUNTIME_DIR/kwinfo/`
  because root cannot read the user's FUSE mount; another process of the same user could
  swap that copy before root runs it. The installed service has no such window.

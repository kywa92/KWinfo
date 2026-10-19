# Diagnostics and logs

KWinfo keeps a detailed log of every run so that problems seen on one machine can be
analysed on another. The `kwinfo-diag` crate provides it; every binary (`kwinfo`,
`kwinfo-cli`, `kwinfo-helper`) initialises it at startup.

## Where the logs are

| Platform | Directory |
|---|---|
| Linux | `$XDG_STATE_HOME/kwinfo/logs/`, normally `~/.local/state/kwinfo/logs/` |
| macOS (development) | `~/Library/Application Support/io.github.kywa92.kwinfo/logs/` |

One file per run, named `<binary>-<UTC date>-<time>-<pid>.log`, for example
`kwinfo-20261018-094351-74375.log`. The 30 newest files are kept, within a 50 MB budget;
older ones are deleted when the next run starts.

## What a log contains

1. A banner line with the binary, its version and the start time.
2. A session header: command line, OS and kernel, distribution, hostname, CPU, memory,
   DMI system/board/BIOS strings, desktop and session type (Wayland or X11), AppImage
   path, locale, user id, executable path, the `hwmon` chips the kernel exposes, which
   sensor-related kernel modules are loaded, whether `sensors`, `smartctl`, `pkexec` and
   `dmidecode` are on `PATH`, and every `KWINFO_*` environment variable.
3. Every tracing event: `debug` and above from KWinfo's own crates (backend enumeration,
   what each backend found or failed to read, sampler recoveries), `info` and above from
   dependencies, with graphics-stack chatter limited to warnings.
4. User actions, on the `kwinfo::action` target: `action="settings.theme" mode=Dark`.
5. A `kwinfo::health` line every 60 ticks: average and maximum tick cost, device and
   channel counts, channels without a reading, sensors whose monitoring the user
   disabled, failing backends.
6. Panics with a backtrace (`kwinfo::panic`), and a "session end" line with the uptime.

## Sending everything in one file

On the machine that showed the problem, run:

```bash
kwinfo-cli report
```

It writes `kwinfo-report-<hostname>-<UTC time>.txt` next to the logs and prints the path.
The GUI writes the same report from Settings → General → "Save diagnostic report…", taking
the sensor inventory from what the window shows (renamed, hidden and disabled sensors are
marked) and the configuration from the settings file in use, and shows the path.
The report contains the machine summary, the configuration file, a sensor inventory (every
device and channel with its current value and stable id, taken live from the hardware), the
list of log files, and the newest three logs (the last 2 MB of each). Options:

| Flag | Effect |
|---|---|
| `--out FILE` | write the report somewhere else |
| `--no-inventory` | skip the live inventory (no hardware access, faster) |
| `--mock` / `--replay DIR` | take the inventory from the mock backend or a captured tree |

The report and the logs contain the hostname, user names that appear in paths, and the
configuration file. They contain no drive serial numbers or MAC addresses.

## Environment variables

| Variable | Effect | Default |
|---|---|---|
| `KWINFO_LOG` | what goes to stderr, in `tracing` filter syntax | `info` for `kwinfo`, `warn` for `kwinfo-cli` |
| `KWINFO_FILE_LOG` | what goes to the file | `debug` for KWinfo crates, `info` for dependencies |
| `KWINFO_LOG_DIR` | directory for the log files | the platform directory above |
| `KWINFO_NO_FILE_LOG` | anything but `0` disables the file | unset |

Examples: `KWINFO_FILE_LOG=trace kwinfo` records everything, including every sysfs read;
`KWINFO_LOG=debug kwinfo-cli list` mirrors the file's detail on the terminal.

## Logging from code

```rust
let _diag = kwinfo_diag::init(kwinfo_diag::DiagConfig::new("kwinfo", env!("CARGO_PKG_VERSION")));
kwinfo_diag::action!("sensor.hide", channel = %id);
tracing::warn!(target: "kwinfo_core::backends::hwmon", "could not read {}: {err}", path.display());
```

Use `tracing::warn!` or `error!` for anything a user should know about, `debug!` for the
detail that only matters when something goes wrong, and the `action!` macro for things the
user did.

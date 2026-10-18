# KWinfo

A Linux hardware monitor in the spirit of HWiNFO64's *Sensor Status* window and its
floating sensor graphs. Written in Rust with [egui](https://github.com/emilk/egui), shipped
as a `.deb` and an AppImage, with dark and light themes.

**Status:** milestone M1. The unprivileged Linux backends (`hwmon`, `cpu`, `intel_gpu`,
`block`, `memory`, `network`, `power_supply`, plus lm-sensors labels) read real hardware on
Linux; the window is still the M0 table. Root-only sensors (RAPL power, MSRs, S.M.A.R.T.,
GPU busy) arrive with the helper in M4. See [PLAN.md](PLAN.md) for the full design and the
milestone list.

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

A replayed tree is static, so usage and rate rows stay empty or at zero; temperatures,
voltages, fans and clocks show their captured values.

## Testing

```bash
cargo test --workspace
```

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

compares the Sensor Status table against the committed pictures in
`crates/kwinfo-app/tests/snapshots/` (set `UPDATE_SNAPSHOTS=true` to accept changes), and

```bash
cargo test -p kwinfo-app --test preview -- --ignored
```

renders the whole window in both themes into `target/preview/*.png` for a quick look.

## Layout

| Path | Contents |
|---|---|
| `crates/kwinfo-core` | sensor model, backends, sampler, statistics, history |
| `crates/kwinfo-ipc` | message types shared by the app and the privileged helper |
| `crates/kwinfo-helper` | the privileged helper service (stub in M0) |
| `crates/kwinfo-app` | the eframe GUI, binary `kwinfo` |
| `crates/kwinfo-cli` | the command line tool, binary `kwinfo-cli` |
| `packaging/` | desktop file, systemd/D-Bus/polkit files, AppImage script (later milestones) |
| `tests/fixtures/` | sysfs/procfs trees used by tests and `--replay` |

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

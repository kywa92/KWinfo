# KWinfo

A Linux hardware monitor in the spirit of HWiNFO64's *Sensor Status* window and its
floating sensor graphs. Written in Rust with [egui](https://github.com/emilk/egui), shipped
as a `.deb` and an AppImage, with dark and light themes.

**Status:** milestone M0 (bootstrap). Nothing reads real hardware yet; the app runs against
a mock backend on any OS. See [PLAN.md](PLAN.md) for the full design and the milestone list.

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
app uses on macOS during development. Real backends arrive in milestone M1.

## Testing

```bash
cargo test --workspace
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
| `tests/fixtures/` | captured sysfs snapshots used by tests |

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

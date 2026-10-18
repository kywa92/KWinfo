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

//! Visual regression snapshots of the Sensor Status table in both palettes.
//!
//! They need a GPU (wgpu), so they are ignored by default and CI skips them:
//!
//! ```bash
//! cargo test -p kwinfo-app --test snapshots -- --ignored
//! ```
//!
//! Baselines live in `tests/snapshots/*.png`; a failing comparison writes `*.new.png`
//! and `*.diff.png` next to them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use kwinfo_app::table::{TableState, show};
use kwinfo_app::theme::{self, Palette, ThemeTokens};
use kwinfo_core::mock::MockBackend;
use kwinfo_core::{
    Backend, ChannelId, ChannelSnapshot, DeviceSnapshot, Snapshot, Stats, TemperatureUnit,
};

/// A snapshot with fixed, seed-determined values, so the picture is reproducible.
fn fixed_snapshot() -> Snapshot {
    let mut mock = MockBackend::new(1);
    let devices = mock.enumerate().unwrap();
    let mut stats: HashMap<ChannelId, Stats> = HashMap::new();
    for t in [10.0, 20.0, 30.0] {
        let mut out = Vec::new();
        mock.sample_at(t, &mut out);
        for sample in out {
            stats
                .entry(sample.channel)
                .or_default()
                .update(sample.value);
        }
    }
    let now = Instant::now();
    Snapshot {
        tick: 3,
        started_at: now,
        taken_at: now,
        stats_since: now,
        interval: Duration::from_secs(1),
        tick_cost: Duration::ZERO,
        devices: devices
            .into_iter()
            .map(|(device, channels)| DeviceSnapshot {
                device: Arc::new(device),
                channels: channels
                    .into_iter()
                    .map(|channel| ChannelSnapshot {
                        stats: stats[&channel.id],
                        channel: Arc::new(channel),
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn snapshot_table(palette: Palette, name: &str) {
    let snapshot = fixed_snapshot();
    let mut state = TableState::default();
    let tokens: &ThemeTokens = ThemeTokens::for_palette(palette);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(780.0, 600.0))
        .with_pixels_per_point(1.0)
        .with_theme(palette.egui_theme())
        .with_context_setup(theme::install)
        .wgpu()
        .build_ui(|ui| {
            egui::Frame::new().fill(tokens.table_bg).show(ui, |ui| {
                show(ui, &snapshot, &mut state, tokens, TemperatureUnit::Celsius);
            });
        });
    harness.run();
    harness.snapshot(name);
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn sensor_table_dark() {
    snapshot_table(Palette::Dark, "sensor_table_dark");
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn sensor_table_light() {
    snapshot_table(Palette::Light, "sensor_table_light");
}

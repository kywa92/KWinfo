//! Visual regression snapshots of the Sensor Status table in both palettes: the default
//! view, a customised view (hidden rows shown greyed, a renamed and a recoloured sensor,
//! disabled monitoring, collapsed groups) and an open context menu.
//!
//! They need a GPU (wgpu), so they are ignored by default and CI skips them:
//!
//! ```bash
//! cargo test -p kwinfo-app --test snapshots -- --ignored
//! ```
//!
//! Baselines live in `tests/snapshots/*.png`; a failing comparison writes `*.new.png`
//! and `*.diff.png` next to them. `UPDATE_SNAPSHOTS=true` accepts the new pictures.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use kwinfo_app::table::{self, TableState, TableView};
use kwinfo_app::theme::{self, Palette, ThemeTokens};
use kwinfo_core::mock::MockBackend;
use kwinfo_core::{
    Backend, ChannelId, ChannelSnapshot, Config, DeviceId, DeviceSnapshot, Rgb, Snapshot, Stats,
};

/// A snapshot with fixed, seed-determined values, so the picture is reproducible.
/// Channels in `disabled` look as the sampler leaves them: flagged, with no statistics.
fn fixed_snapshot(disabled: &HashSet<ChannelId>) -> Snapshot {
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
                    .map(|channel| {
                        let off = disabled.contains(&channel.id);
                        ChannelSnapshot {
                            stats: if off {
                                Stats::new()
                            } else {
                                stats[&channel.id]
                            },
                            disabled: off,
                            channel: Arc::new(channel),
                        }
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn harness(palette: Palette, config: Config) -> Harness<'static, (Config, TableState)> {
    let disabled = config.disabled_channels();
    let snapshot = fixed_snapshot(&disabled);
    let tokens: &'static ThemeTokens = ThemeTokens::for_palette(palette);
    Harness::builder()
        .with_size(egui::vec2(780.0, 600.0))
        .with_pixels_per_point(1.0)
        .with_theme(palette.egui_theme())
        .with_context_setup(theme::install)
        .wgpu()
        .build_ui_state(
            move |ui, (config, state): &mut (Config, TableState)| {
                egui::Frame::new().fill(tokens.table_bg).show(ui, |ui| {
                    let view = TableView {
                        snapshot: &snapshot,
                        config,
                        tokens,
                    };
                    table::show(ui, view, state);
                });
            },
            (config, TableState::default()),
        )
}

/// Hidden rows shown greyed, a renamed and a recoloured sensor, a disabled sensor, a
/// collapsed sub-group, and a disabled, collapsed device.
fn customised() -> Config {
    let mut config = Config::default();
    let id = |raw: &str| ChannelId::from_raw(raw);
    config.general.show_hidden = true;
    for hidden in [
        "cpu:mock/temp.core1",
        "cpu:mock/temp.core3",
        "cpu:mock/usage.max",
    ] {
        config.update_channel(&id(hidden), |c| c.hidden = true);
    }
    config.update_channel(&id("cpu:mock/temp.package"), |c| {
        c.label = Some("CPU Die (renamed)".into());
    });
    config.update_channel(&id("cpu:mock/temp.coremax"), |c| {
        c.color = Some(Rgb::new(0xff, 0x8c, 0x00));
    });
    config.update_channel(&id("cpu:mock/usage.total"), |c| {
        c.color = Some(Rgb::new(0x1e, 0x88, 0xe5));
    });
    config.update_channel(&id("cpu:mock/power.gt"), |c| c.disabled = true);
    let cpu = DeviceId::new("cpu:mock");
    config.set_group_collapsed(&cpu, "Core Usage", true);
    let board = DeviceId::new("hwmon:mock:nct6793d");
    config.update_device(&board, |d| {
        d.disabled = true;
        d.collapsed = true;
    });
    config.set_collapsed(&DeviceId::new("mem:mock"), true);
    config
}

fn default_table(palette: Palette, name: &str) {
    let mut harness = harness(palette, Config::default());
    harness.run();
    harness.snapshot(name);
}

fn customised_table(palette: Palette, name: &str) {
    let mut harness = harness(palette, customised());
    harness.run();
    // Hidden rows are listed (greyed) because show_hidden is on; the rename shows.
    assert!(harness.query_by_label("Core #1").is_some());
    assert!(harness.query_by_label("CPU Die (renamed)").is_some());
    assert!(harness.query_by_label("CPU Package").is_none());
    harness.snapshot(name);
}

fn context_menu(palette: Palette, name: &str) {
    let mut harness = harness(palette, Config::default());
    harness.run();
    harness.get_by_label("Core #2").click_secondary();
    harness.run();
    assert!(harness.query_by_label("Disable monitoring").is_some());
    harness.snapshot(name);
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn sensor_table_dark() {
    default_table(Palette::Dark, "sensor_table_dark");
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn sensor_table_light() {
    default_table(Palette::Light, "sensor_table_light");
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn sensor_table_customised_dark() {
    customised_table(Palette::Dark, "sensor_table_customised_dark");
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn sensor_table_customised_light() {
    customised_table(Palette::Light, "sensor_table_customised_light");
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn sensor_table_menu_dark() {
    context_menu(Palette::Dark, "sensor_table_menu_dark");
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn sensor_table_menu_light() {
    context_menu(Palette::Light, "sensor_table_menu_light");
}

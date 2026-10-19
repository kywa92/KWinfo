//! Visual regression snapshots of the sensor graph in both palettes: a temperature on
//! its fixed 0 to 100 °C range, with a spike and a gap in the readings, and a power
//! reading under Auto Fit.
//!
//! They need a GPU (wgpu), so they are ignored by default and CI skips them:
//!
//! ```bash
//! cargo test -p kwinfo-app --test graph_snapshots -- --ignored
//! ```
//!
//! Baselines live in `tests/snapshots/*.png`; `UPDATE_SNAPSHOTS=true` accepts changes.

use egui::{Color32, Frame, Margin};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use kwinfo_app::graph::{self, GraphInput, new_entry};
use kwinfo_app::theme::{self, Palette, ThemeTokens};
use kwinfo_core::config::GraphEntry;
use kwinfo_core::{ChannelId, Kind, TemperatureUnit, Unit};

/// Ten minutes of readings, one a second: a slow wave, a spike at five minutes and a
/// ten-second gap at about seven.
fn readings(base: f32, swing: f32, spike: f32) -> Vec<(f64, f32)> {
    (0..=600)
        .map(|i| {
            let t = f64::from(i);
            let v = match i {
                400..410 => f32::NAN,
                300 => base + spike,
                _ => base + swing * ((t / 47.0).sin() as f32) + 0.4 * ((t / 3.0).cos() as f32),
            };
            (t, v)
        })
        .collect()
}

struct Case {
    name: &'static str,
    label: &'static str,
    kind: Kind,
    unit: Unit,
    decimals: u8,
    points: Vec<(f64, f32)>,
    entry: GraphEntry,
}

fn temperature() -> Case {
    Case {
        name: "graph_temperature",
        label: "CPU Package",
        kind: Kind::Temperature,
        unit: Unit::Celsius,
        decimals: 1,
        points: readings(52.0, 9.0, 30.0),
        entry: new_entry(ChannelId::from_raw("cpu:0/temp1"), Kind::Temperature, 3600),
    }
}

fn power() -> Case {
    Case {
        name: "graph_power",
        label: "CPU Package Power",
        kind: Kind::Power,
        unit: Unit::Watt,
        decimals: 3,
        points: readings(38.0, 14.0, 60.0),
        entry: new_entry(ChannelId::from_raw("cpu:0/power"), Kind::Power, 3600),
    }
}

fn snapshot(case: Case, palette: Palette) {
    let tokens: &'static ThemeTokens = ThemeTokens::for_palette(palette);
    let color: Color32 = tokens.graph_color(case.kind);
    let current = case.points.last().map(|&(_, v)| f64::from(v));
    let name = format!(
        "{}_{}",
        case.name,
        if palette == Palette::Dark {
            "dark"
        } else {
            "light"
        }
    );
    let label = case.label;
    let mut harness = Harness::builder()
        .with_size(egui::vec2(520.0, 190.0))
        .with_pixels_per_point(1.0)
        .with_theme(palette.egui_theme())
        .with_context_setup(theme::install)
        .wgpu()
        .build_ui(move |ui| {
            Frame::new()
                .fill(tokens.window_bg)
                .inner_margin(Margin::same(6))
                .show(ui, |ui| {
                    let input = GraphInput {
                        id: ui.scope_id().with("graph"),
                        name: case.label,
                        points: &case.points,
                        now: 600.0,
                        current,
                        kind: case.kind,
                        unit: case.unit,
                        decimals: case.decimals,
                        temperature: TemperatureUnit::Celsius,
                        color,
                        history_s: 3600,
                        disabled: false,
                    };
                    graph::show(ui, &input, &case.entry, tokens);
                });
        });
    harness.run();
    assert!(
        harness
            .query_by_label(&format!("Graph of {label}"))
            .is_some()
    );
    assert!(
        harness
            .query_by_label(&format!("Maximum of {label}"))
            .is_some()
    );
    harness.snapshot(&name);
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn graph_temperature_dark() {
    snapshot(temperature(), Palette::Dark);
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn graph_temperature_light() {
    snapshot(temperature(), Palette::Light);
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn graph_power_dark() {
    snapshot(power(), Palette::Dark);
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn graph_power_light() {
    snapshot(power(), Palette::Light);
}

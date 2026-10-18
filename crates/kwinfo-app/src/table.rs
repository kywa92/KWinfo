//! The Sensor Status tree-table: device groups with their channels, and the Current,
//! Minimum, Maximum and Average columns.

use std::collections::HashSet;

use egui::{Align, Color32, CornerRadius, Layout, RichText, Sense, Ui, pos2, vec2};
use egui_extras::{Column, TableBuilder, TableRow};
use kwinfo_core::{ChannelId, DeviceId, Snapshot, TemperatureUnit, Unit, format_value};

use crate::theme::ThemeTokens;

pub const ROW_HEIGHT: f32 = 20.0;
pub const HEADER_HEIGHT: f32 = 22.0;

/// What the table remembers between frames.
#[derive(Debug, Default)]
pub struct TableState {
    pub collapsed: HashSet<DeviceId>,
    pub selected: Option<ChannelId>,
}

impl TableState {
    pub fn all_collapsed(&self, snapshot: &Snapshot) -> bool {
        !snapshot.devices.is_empty()
            && snapshot
                .devices
                .iter()
                .all(|d| self.collapsed.contains(&d.device.id))
    }

    pub fn set_all_collapsed(&mut self, snapshot: &Snapshot, collapsed: bool) {
        if collapsed {
            self.collapsed
                .extend(snapshot.devices.iter().map(|d| d.device.id.clone()));
        } else {
            self.collapsed.clear();
        }
    }
}

#[derive(Clone, Copy)]
enum Row {
    Device(usize),
    Channel(usize, usize),
}

/// egui has no tree-table, so the tree is flattened into the visible rows each frame.
fn flatten(snapshot: &Snapshot, state: &TableState) -> Vec<Row> {
    let mut rows = Vec::with_capacity(snapshot.channel_count() + snapshot.devices.len());
    for (d, device) in snapshot.devices.iter().enumerate() {
        rows.push(Row::Device(d));
        if !state.collapsed.contains(&device.device.id) {
            rows.extend((0..device.channels.len()).map(|c| Row::Channel(d, c)));
        }
    }
    rows
}

pub fn show(
    ui: &mut Ui,
    snapshot: &Snapshot,
    state: &mut TableState,
    tokens: &ThemeTokens,
    temperature: TemperatureUnit,
) {
    let rows = flatten(snapshot, state);
    let height = ui.available_height();
    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .sense(Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::initial(330.0).at_least(160.0).clip(true))
        .column(Column::initial(110.0).at_least(72.0))
        .column(Column::initial(100.0).at_least(72.0))
        .column(Column::initial(100.0).at_least(72.0))
        .column(Column::remainder().at_least(90.0))
        .min_scrolled_height(0.0)
        .max_scroll_height(height)
        .header(HEADER_HEIGHT, |mut header| {
            header.col(|ui| {
                ui.add_space(4.0);
                ui.strong("Sensor");
            });
            for name in ["Current", "Minimum", "Maximum", "Average"] {
                header.col(|ui| right_aligned(ui, RichText::new(name).strong()));
            }
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, rows.len(), |mut row| match rows[row.index()] {
                Row::Device(d) => device_row(&mut row, snapshot, state, tokens, d),
                Row::Channel(d, c) => {
                    channel_row(&mut row, snapshot, state, tokens, temperature, d, c)
                }
            });
        });
}

fn device_row(
    row: &mut TableRow<'_, '_>,
    snapshot: &Snapshot,
    state: &mut TableState,
    tokens: &ThemeTokens,
    d: usize,
) {
    let device = &snapshot.devices[d].device;
    let collapsed = state.collapsed.contains(&device.id);
    row.col(|ui| {
        fill(ui, tokens.group_row);
        ui.add_space(4.0);
        chevron(ui, collapsed, tokens.text_secondary);
        ui.label(RichText::new(&device.label).strong());
    });
    for _ in 0..4 {
        row.col(|ui| fill(ui, tokens.group_row));
    }
    if row.response().clicked() {
        if collapsed {
            state.collapsed.remove(&device.id);
        } else {
            state.collapsed.insert(device.id.clone());
        }
    }
}

fn channel_row(
    row: &mut TableRow<'_, '_>,
    snapshot: &Snapshot,
    state: &mut TableState,
    tokens: &ThemeTokens,
    temperature: TemperatureUnit,
    d: usize,
    c: usize,
) {
    let entry = &snapshot.devices[d].channels[c];
    let channel = &entry.channel;
    let selected = state.selected.as_ref() == Some(&channel.id);
    row.set_selected(selected);
    row.col(|ui| {
        ui.add_space(24.0);
        kind_marker(ui, tokens.graph_color(channel.kind));
        ui.label(&channel.label);
    });
    let stats = entry.stats;
    let (unit, decimals) = (channel.unit, channel.decimals);
    row.col(|ui| {
        if !selected {
            fill(ui, tokens.current_tint);
        }
        value(ui, stats.current(), unit, decimals, temperature);
    });
    row.col(|ui| value(ui, stats.min(), unit, decimals, temperature));
    row.col(|ui| value(ui, stats.max(), unit, decimals, temperature));
    row.col(|ui| value(ui, stats.average(), unit, decimals, temperature));
    if row.response().clicked() {
        state.selected = Some(channel.id.clone());
    }
}

fn value(ui: &mut Ui, v: Option<f64>, unit: Unit, decimals: u8, temperature: TemperatureUnit) {
    right_aligned(
        ui,
        RichText::new(format_value(v, unit, decimals, temperature)),
    );
}

fn right_aligned(ui: &mut Ui, text: RichText) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.add_space(4.0);
        ui.label(text);
    });
}

fn fill(ui: &mut Ui, color: Color32) {
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, CornerRadius::ZERO, color);
}

/// A small coloured dot standing in for the per-kind icons that milestone M2 adds.
fn kind_marker(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(12.0, ROW_HEIGHT), Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
}

/// The expand/collapse indicator of a device row, drawn rather than typed so it does not
/// depend on which fonts are installed.
fn chevron(ui: &mut Ui, collapsed: bool, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(12.0, ROW_HEIGHT), Sense::hover());
    let c = rect.center();
    let points = if collapsed {
        vec![
            pos2(c.x - 2.5, c.y - 4.0),
            pos2(c.x + 3.0, c.y),
            pos2(c.x - 2.5, c.y + 4.0),
        ]
    } else {
        vec![
            pos2(c.x - 4.0, c.y - 2.5),
            pos2(c.x + 4.0, c.y - 2.5),
            pos2(c.x, c.y + 3.0),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        color,
        egui::Stroke::NONE,
    ));
}

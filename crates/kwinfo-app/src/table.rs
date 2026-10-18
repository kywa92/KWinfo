//! The Sensor Status tree-table: device rows, sub-group rows and sensor rows under the
//! Sensor, Current, Minimum, Maximum and Average columns.
//!
//! egui has no tree-table, so every frame the snapshot is flattened into the visible
//! rows ([`layout`]), taking collapse state, hidden sensors and the quick filter into
//! account, and `egui_extras` paints only the rows that are on screen.

use egui::{Align, Color32, CornerRadius, Layout as EguiLayout, RichText, Sense, Ui, pos2, vec2};
use egui_extras::{Column as TableColumn, TableBuilder, TableRow};
use kwinfo_core::config::{Column, Config};
use kwinfo_core::{
    ChannelId, DeviceSnapshot, Snapshot, Stats, TemperatureUnit, Unit, format_value,
};

use kwinfo_diag::action;

use crate::icons::{self, Icon};
use crate::menu;
use crate::theme::ThemeTokens;

pub const ROW_HEIGHT: f32 = 20.0;
pub const HEADER_HEIGHT: f32 = 22.0;
/// A sub-group needs at least this many visible rows to get a row of its own; smaller
/// groups are listed flat.
pub const MIN_GROUP_ROWS: usize = 3;
/// Horizontal step between tree levels: a chevron and its gap, so that a level's icons
/// line up under its parent's icon, as in HWiNFO.
const INDENT: f32 = 16.0;
const CHEVRON: f32 = 12.0;

/// What the table keeps between frames. Everything that survives a restart (collapse
/// state, hidden rows, names, colours, columns) lives in the configuration instead.
#[derive(Debug, Default)]
pub struct TableState {
    pub selected: Option<ChannelId>,
    /// The quick filter (Ctrl+F); only applies while the filter bar is open.
    pub filter: String,
    pub filter_open: bool,
    /// Set to move the keyboard focus into the filter field on the next frame.
    pub focus_filter: bool,
    /// Bumped to make the table forget the column widths egui remembers.
    widths_generation: u64,
    last_widths: Vec<f32>,
    resizing: bool,
}

impl TableState {
    pub fn open_filter(&mut self) {
        self.filter_open = true;
        self.focus_filter = true;
    }

    pub fn close_filter(&mut self) {
        self.filter_open = false;
        self.filter.clear();
    }

    /// The filter in effect: the typed text while the bar is open, else nothing.
    pub fn active_filter(&self) -> &str {
        if self.filter_open {
            self.filter.trim()
        } else {
            ""
        }
    }

    /// Starts again from the configured (or default) column widths.
    pub fn reset_widths(&mut self) {
        self.widths_generation += 1;
        self.last_widths.clear();
        self.resizing = false;
    }

    /// Whether every device row is collapsed.
    pub fn all_collapsed(snapshot: &Snapshot, config: &Config) -> bool {
        !snapshot.devices.is_empty()
            && snapshot
                .devices
                .iter()
                .all(|d| config.is_collapsed(&d.device.id))
    }

    pub fn set_all_collapsed(snapshot: &Snapshot, config: &mut Config, collapsed: bool) {
        for device in &snapshot.devices {
            config.set_collapsed(&device.device.id, collapsed);
        }
    }
}

/// Something only the application can do, asked for by a row or its menu.
#[derive(Clone, Debug, PartialEq)]
pub enum TableAction {
    /// Open a sensor's graph (double-click or menu). Graph windows arrive in M3.
    ShowGraph(ChannelId),
    /// Ask the user for a new name for a sensor.
    Rename(ChannelId),
    /// Ask the user for a custom colour for a sensor.
    PickColor(ChannelId),
    /// Reset minimum, maximum and average of these sensors.
    Reset(Vec<ChannelId>),
    /// Text went to the clipboard; the application confirms it briefly.
    Copied(String),
    /// The column widths were reset from the header menu.
    ResetWidths,
}

/// One visible row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// A device (group) row: index into `snapshot.devices`.
    Device(usize),
    /// A sub-group row: index into [`Layout::groups`].
    Group(usize),
    /// A sensor: device index, channel index, and depth in the tree (1 directly under
    /// the device, 2 inside a sub-group).
    Channel { d: usize, c: usize, depth: u8 },
}

/// A sub-group with a row of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group<'a> {
    pub d: usize,
    pub name: &'a str,
    /// Indices of the member channels within the device, in order.
    pub members: Vec<usize>,
}

/// The flattened tree for one frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout<'a> {
    pub rows: Vec<Row>,
    pub groups: Vec<Group<'a>>,
    /// Sensors that pass the filter and the hidden setting, collapsed or not.
    pub shown: usize,
    /// Whether a filter was applied.
    pub filtered: bool,
}

/// Flattens the snapshot into visible rows.
///
/// - Hidden sensors are left out unless `config.general.show_hidden`.
/// - Sensors sharing a [`kwinfo_core::Channel::group`] are gathered under one sub-group
///   row at the position of the first of them, when there are at least
///   [`MIN_GROUP_ROWS`]; otherwise they stay in place, flat.
/// - A non-empty `filter` keeps sensors whose name (the user's or the original), group
///   or device matches it, case-insensitively, and expands everything that matches.
pub fn layout<'a>(snapshot: &'a Snapshot, config: &Config, filter: &str) -> Layout<'a> {
    enum Item {
        Channel(usize),
        Group(usize),
    }
    let needle = filter.trim().to_lowercase();
    let filtering = !needle.is_empty();
    let matches = |text: &str| text.to_lowercase().contains(&needle);
    let mut out = Layout {
        filtered: filtering,
        ..Layout::default()
    };
    for (d, device) in snapshot.devices.iter().enumerate() {
        let device_matches = filtering && matches(&device.device.label);
        let mut items = Vec::new();
        let mut groups: Vec<(&'a str, Vec<usize>)> = Vec::new();
        for (c, entry) in device.channels.iter().enumerate() {
            let channel = &entry.channel;
            if config.is_hidden(&channel.id) && !config.general.show_hidden {
                continue;
            }
            if filtering
                && !device_matches
                && !matches(config.label_of(channel))
                && !matches(&channel.label)
                && !channel.group.as_deref().is_some_and(matches)
            {
                continue;
            }
            out.shown += 1;
            match channel.group.as_deref() {
                Some(name) => match groups.iter().position(|(n, _)| *n == name) {
                    Some(g) => groups[g].1.push(c),
                    None => {
                        items.push(Item::Group(groups.len()));
                        groups.push((name, vec![c]));
                    }
                },
                None => items.push(Item::Channel(c)),
            }
        }
        if filtering && items.is_empty() && !device_matches {
            continue;
        }
        out.rows.push(Row::Device(d));
        if !filtering && config.is_collapsed(&device.device.id) {
            continue;
        }
        for item in items {
            match item {
                Item::Channel(c) => out.rows.push(Row::Channel { d, c, depth: 1 }),
                Item::Group(g) => {
                    let (name, members) = (groups[g].0, std::mem::take(&mut groups[g].1));
                    if members.len() < MIN_GROUP_ROWS {
                        out.rows
                            .extend(members.iter().map(|&c| Row::Channel { d, c, depth: 1 }));
                        continue;
                    }
                    let collapsed =
                        !filtering && config.is_group_collapsed(&device.device.id, name);
                    out.rows.push(Row::Group(out.groups.len()));
                    if !collapsed {
                        out.rows
                            .extend(members.iter().map(|&c| Row::Channel { d, c, depth: 2 }));
                    }
                    out.groups.push(Group { d, name, members });
                }
            }
        }
    }
    out
}

/// Number of hidden sensors present in the snapshot, for the window title.
pub fn hidden_count(snapshot: &Snapshot, config: &Config) -> usize {
    snapshot
        .channels()
        .filter(|c| config.is_hidden(&c.channel.id))
        .count()
}

/// Everything the table reads, and the settings it may change, in one frame.
pub struct TableView<'a> {
    pub snapshot: &'a Snapshot,
    pub config: &'a mut Config,
    pub tokens: &'a ThemeTokens,
}

/// Shows the table and returns what the application should do.
pub fn show(ui: &mut Ui, view: TableView<'_>, state: &mut TableState) -> Vec<TableAction> {
    let TableView {
        snapshot,
        config,
        tokens,
    } = view;
    let layout = layout(snapshot, config, state.active_filter());
    let columns: Vec<Column> = config.columns.visible().collect();
    let temperature = config.general.temperature_unit;
    let mut actions = Vec::new();
    let mut widths = Vec::new();

    let height = ui.available_height();
    let mut builder = TableBuilder::new(ui)
        .id_salt(("sensor-table", &columns, state.widths_generation))
        .striped(true)
        .resizable(true)
        .sense(Sense::click())
        .cell_layout(EguiLayout::left_to_right(Align::Center))
        .min_scrolled_height(0.0)
        .max_scroll_height(height);
    for (i, &column) in columns.iter().enumerate() {
        builder = builder.column(table_column(column, config, i + 1 == columns.len()));
    }
    let selected = state.selected.clone();
    let mut clicked: Option<ChannelId> = None;
    builder
        .header(HEADER_HEIGHT, |mut header| {
            for &column in &columns {
                let (_, response) = header.col(|ui| header_cell(ui, column));
                response.context_menu(|ui| menu::columns(ui, config, &mut actions));
            }
        })
        .body(|body| {
            widths = body.widths().to_vec();
            body.rows(ROW_HEIGHT, layout.rows.len(), |mut row| {
                let mut cx = RowContext {
                    snapshot,
                    config: &mut *config,
                    tokens,
                    columns: &columns,
                    temperature,
                    filtering: layout.filtered,
                    actions: &mut actions,
                };
                match layout.rows[row.index()] {
                    Row::Device(d) => device_row(&mut row, &mut cx, d),
                    Row::Group(g) => group_row(&mut row, &mut cx, &layout.groups[g]),
                    Row::Channel { d, c, depth } => {
                        if let Some(id) =
                            channel_row(&mut row, &mut cx, d, c, depth, selected.as_ref())
                        {
                            clicked = Some(id);
                        }
                    }
                }
            });
        });
    if let Some(id) = clicked {
        state.selected = Some(id);
    }
    if actions.contains(&TableAction::ResetWidths) {
        state.reset_widths();
    } else {
        remember_widths(ui, state, config, &columns, widths);
    }
    actions
}

/// Writes column widths the user dragged into the configuration. Widths egui changes on
/// its own (the last column filling the window, the first frame's layout) are not
/// stored: only changes that happen while, or right after, a mouse button is held.
fn remember_widths(
    ui: &Ui,
    state: &mut TableState,
    config: &mut Config,
    columns: &[Column],
    widths: Vec<f32>,
) {
    let pointer_down = ui.input(|i| i.pointer.primary_down());
    if widths == state.last_widths {
        if !pointer_down {
            state.resizing = false;
        }
        return;
    }
    let first_frame = state.last_widths.is_empty();
    if !first_frame && (pointer_down || state.resizing) {
        state.resizing = true;
        // The last column takes whatever room is left, so its width is not a setting.
        let fixed = columns.len().saturating_sub(1);
        for (&column, &width) in columns.iter().zip(&widths).take(fixed) {
            let width = width.round();
            let is_default = (width - default_width(column)).abs() < 0.5;
            config.columns.get_mut(column).width = (!is_default).then_some(width);
        }
    }
    state.last_widths = widths;
}

fn default_width(column: Column) -> f32 {
    match column {
        Column::Sensor => 330.0,
        Column::Current => 110.0,
        Column::Minimum | Column::Maximum | Column::Average => 100.0,
    }
}

fn table_column(column: Column, config: &Config, last: bool) -> TableColumn {
    let minimum = if column == Column::Sensor {
        160.0
    } else {
        72.0
    };
    let base = if last {
        TableColumn::remainder()
    } else {
        let width = config
            .columns
            .get(column)
            .width
            .unwrap_or_else(|| default_width(column));
        TableColumn::initial(width)
    };
    // Names are clipped; values grow their column rather than lose digits.
    base.at_least(minimum).clip(column == Column::Sensor)
}

fn header_cell(ui: &mut Ui, column: Column) {
    if column == Column::Sensor {
        ui.add_space(4.0);
        ui.strong(column.label());
    } else {
        right_aligned(ui, RichText::new(column.label()).strong());
    }
}

/// What every row needs.
struct RowContext<'a, 'b> {
    snapshot: &'a Snapshot,
    config: &'b mut Config,
    tokens: &'a ThemeTokens,
    columns: &'b [Column],
    temperature: TemperatureUnit,
    filtering: bool,
    actions: &'b mut Vec<TableAction>,
}

fn device_row(row: &mut TableRow<'_, '_>, cx: &mut RowContext<'_, '_>, d: usize) {
    let (tokens, snapshot) = (cx.tokens, cx.snapshot);
    let device = &snapshot.devices[d];
    let id = &device.device.id;
    let collapsed = !cx.filtering && cx.config.is_collapsed(id);
    let disabled = cx.config.is_device_disabled(id);
    for &column in cx.columns {
        row.col(|ui| {
            fill(ui, tokens.group_row);
            match column {
                Column::Sensor => {
                    tree_prefix(ui, 0, Some(collapsed), tokens.text_secondary);
                    icons::icon(
                        ui,
                        Icon::for_category(device.device.category),
                        tokens.text_secondary,
                    );
                    ui.add_space(4.0);
                    let color = if disabled {
                        tokens.text_secondary
                    } else {
                        tokens.text_primary
                    };
                    ui.label(RichText::new(&device.device.label).strong().color(color));
                }
                Column::Current if disabled => right_aligned(
                    ui,
                    RichText::new("Disabled")
                        .italics()
                        .color(tokens.text_secondary),
                ),
                _ => {}
            }
        });
    }
    let response = row.response();
    if response.clicked() && !cx.filtering {
        action!("device.collapse", device = %id, collapsed = !collapsed);
        cx.config.set_collapsed(id, !collapsed);
    }
    response.context_menu(|ui| {
        menu::device(ui, device, collapsed, cx.temperature, cx.config, cx.actions);
    });
}

fn group_row(row: &mut TableRow<'_, '_>, cx: &mut RowContext<'_, '_>, group: &Group<'_>) {
    let (tokens, snapshot) = (cx.tokens, cx.snapshot);
    let device = &snapshot.devices[group.d];
    let collapsed = !cx.filtering && cx.config.is_group_collapsed(&device.device.id, group.name);
    let first = &device.channels[group.members[0]].channel;
    let summary = summarise(device, &group.members);
    let all_disabled = group
        .members
        .iter()
        .all(|&c| cx.config.is_disabled(&device.channels[c].channel));
    let text = if all_disabled {
        tokens.text_secondary
    } else {
        tokens.text_primary
    };
    for &column in cx.columns {
        row.col(|ui| match column {
            Column::Sensor => {
                tree_prefix(ui, 1, Some(collapsed), tokens.text_secondary);
                let icon_color = if all_disabled {
                    tokens.text_secondary
                } else {
                    tokens.icon_color(first.kind)
                };
                icons::icon(ui, Icon::for_channel(first.kind, first.unit), icon_color);
                ui.add_space(4.0);
                ui.label(RichText::new(group.name).color(text));
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!("({})", group.members.len()))
                        .color(tokens.text_secondary),
                );
            }
            Column::Current => {
                if let Some(s) = summary.filter(|s| s.unit == Unit::Bool) {
                    s.show(ui, s.current, cx.temperature, text);
                }
            }
            Column::Minimum => {
                if let Some(s) = summary {
                    s.show(ui, s.min, cx.temperature, text);
                }
            }
            Column::Maximum => {
                if let Some(s) = summary {
                    s.show(ui, s.max, cx.temperature, text);
                }
            }
            Column::Average => {}
        });
    }
    let response = row.response();
    if response.clicked() && !cx.filtering {
        action!(
            "group.collapse",
            device = %device.device.id,
            group = group.name,
            collapsed = !collapsed
        );
        cx.config
            .set_group_collapsed(&device.device.id, group.name, !collapsed);
    }
    response.context_menu(|ui| {
        menu::group(
            ui,
            device,
            group.name,
            &group.members,
            collapsed,
            cx.config,
            cx.actions,
        );
    });
}

/// Returns the channel's id when the row was clicked, to select it.
fn channel_row(
    row: &mut TableRow<'_, '_>,
    cx: &mut RowContext<'_, '_>,
    d: usize,
    c: usize,
    depth: u8,
    selected: Option<&ChannelId>,
) -> Option<ChannelId> {
    let (tokens, snapshot) = (cx.tokens, cx.snapshot);
    let entry = &snapshot.devices[d].channels[c];
    let channel = &entry.channel;
    let settings = cx.config.channel(&channel.id);
    let hidden = settings.is_some_and(|s| s.hidden);
    let disabled = entry.disabled || cx.config.is_disabled(channel);
    let custom = settings
        .and_then(|s| s.color)
        .map(|rgb| tokens.custom_text(rgb));
    let muted = hidden || disabled;
    let label = cx.config.label_of(channel).to_string();
    let (label_color, icon_color, value_color) = if muted {
        (
            tokens.text_secondary,
            tokens.text_secondary,
            tokens.text_secondary,
        )
    } else {
        (
            custom.unwrap_or(tokens.text_primary),
            custom.unwrap_or_else(|| tokens.icon_color(channel.kind)),
            tokens.text_primary,
        )
    };
    let is_selected = selected == Some(&channel.id);
    row.set_selected(is_selected);
    let stats: Stats = entry.stats;
    let (unit, decimals, temperature) = (channel.unit, channel.decimals, cx.temperature);
    for &column in cx.columns {
        row.col(|ui| match column {
            Column::Sensor => {
                tree_prefix(ui, depth, None, tokens.text_secondary);
                icons::icon(
                    ui,
                    Icon::for_channel(channel.kind, channel.unit),
                    icon_color,
                );
                ui.add_space(4.0);
                let mut text = RichText::new(label.as_str()).color(label_color);
                if hidden {
                    text = text.italics();
                }
                ui.label(text);
            }
            Column::Current => {
                if !is_selected {
                    fill(ui, tokens.current_tint);
                }
                if disabled {
                    right_aligned(
                        ui,
                        RichText::new("Disabled")
                            .italics()
                            .color(tokens.text_secondary),
                    );
                } else {
                    let color = if muted {
                        value_color
                    } else {
                        custom.unwrap_or(value_color)
                    };
                    value(ui, stats.current(), unit, decimals, temperature, color);
                }
            }
            Column::Minimum => value(ui, stats.min(), unit, decimals, temperature, value_color),
            Column::Maximum => value(ui, stats.max(), unit, decimals, temperature, value_color),
            Column::Average => {
                value(
                    ui,
                    stats.average(),
                    unit,
                    decimals,
                    temperature,
                    value_color,
                );
            }
        });
    }
    let response = row.response();
    if response.double_clicked() {
        cx.actions.push(TableAction::ShowGraph(channel.id.clone()));
    }
    response.context_menu(|ui| {
        menu::channel(ui, entry, cx.temperature, tokens, cx.config, cx.actions);
    });
    response.clicked().then(|| channel.id.clone())
}

/// What a sub-group row shows: the lowest minimum and the highest maximum of its
/// members and, for Yes/No rows, whether any member currently says Yes. Only when all
/// members share a unit.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Summary {
    current: Option<f64>,
    min: Option<f64>,
    max: Option<f64>,
    unit: Unit,
    decimals: u8,
}

impl Summary {
    fn show(&self, ui: &mut Ui, v: Option<f64>, temperature: TemperatureUnit, color: Color32) {
        value(ui, v, self.unit, self.decimals, temperature, color);
    }
}

fn summarise(device: &DeviceSnapshot, members: &[usize]) -> Option<Summary> {
    let first = &device.channels[*members.first()?].channel;
    let entries = || members.iter().map(|&c| &device.channels[c]);
    if entries().any(|e| e.channel.unit != first.unit) {
        return None;
    }
    let fold = |f: fn(&Stats) -> Option<f64>, pick: fn(f64, f64) -> f64| {
        entries().filter_map(|e| f(&e.stats)).reduce(pick)
    };
    Some(Summary {
        current: fold(Stats::current, f64::max),
        min: fold(Stats::min, f64::min),
        max: fold(Stats::max, f64::max),
        unit: first.unit,
        decimals: first.decimals,
    })
}

/// A right-aligned value cell.
fn value(
    ui: &mut Ui,
    v: Option<f64>,
    unit: Unit,
    decimals: u8,
    temperature: TemperatureUnit,
    color: Color32,
) {
    let text = format_value(v, unit, decimals, temperature);
    right_aligned(ui, RichText::new(text).color(color));
}

fn right_aligned(ui: &mut Ui, text: RichText) {
    ui.with_layout(EguiLayout::right_to_left(Align::Center), |ui| {
        ui.add_space(4.0);
        ui.label(text);
    });
}

fn fill(ui: &mut Ui, color: Color32) {
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, CornerRadius::ZERO, color);
}

/// Indents the Sensor cell for `depth` and, for device and sub-group rows, draws the
/// expand/collapse chevron.
fn tree_prefix(ui: &mut Ui, depth: u8, chevron: Option<bool>, color: Color32) {
    ui.spacing_mut().item_spacing.x = 0.0;
    ui.add_space(4.0 + f32::from(depth) * INDENT);
    if let Some(collapsed) = chevron {
        self::chevron(ui, collapsed, color);
        ui.add_space(INDENT - CHEVRON);
    }
}

/// The expand/collapse indicator, drawn rather than typed so it does not depend on
/// which fonts are installed.
fn chevron(ui: &mut Ui, collapsed: bool, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(CHEVRON, ROW_HEIGHT), Sense::hover());
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use kwinfo_core::{Category, Channel, ChannelSnapshot, Device, DeviceId, Kind};

    use super::*;

    fn snapshot() -> Snapshot {
        let cpu = Device::new("cpu:0", "CPU [#0]: Test", Category::Cpu);
        let mut channels = vec![Channel::new(
            &cpu.id,
            "package",
            "CPU Package",
            Kind::Temperature,
        )];
        for core in 0..4 {
            channels.push(
                Channel::new(
                    &cpu.id,
                    &format!("clock.core{core}"),
                    format!("Core #{core} Clock"),
                    Kind::Clock,
                )
                .with_group("Core Clocks"),
            );
        }
        channels.push(Channel::new(
            &cpu.id,
            "usage.total",
            "Total CPU Usage",
            Kind::Usage,
        ));
        for t in 0..2 {
            channels.push(
                Channel::new(
                    &cpu.id,
                    &format!("throttle{t}"),
                    format!("Throttle {t}"),
                    Kind::Boolean,
                )
                .with_group("Thermal Throttling"),
            );
        }
        let gpu = Device::new("gpu:0", "GPU [#0]: Test", Category::Gpu);
        let gpu_channels = vec![Channel::new(&gpu.id, "clock", "GPU Clock", Kind::Clock)];
        let entries = |channels: Vec<Channel>| {
            channels
                .into_iter()
                .enumerate()
                .map(|(i, channel)| {
                    let mut stats = Stats::new();
                    stats.update(Some(i as f64 * 10.0));
                    stats.update(Some(i as f64 * 10.0 + 5.0));
                    ChannelSnapshot {
                        channel: Arc::new(channel),
                        stats,
                        disabled: false,
                    }
                })
                .collect()
        };
        let now = Instant::now();
        Snapshot {
            tick: 2,
            started_at: now,
            taken_at: now,
            stats_since: now,
            interval: Duration::from_secs(1),
            tick_cost: Duration::ZERO,
            devices: vec![
                DeviceSnapshot {
                    device: Arc::new(cpu),
                    channels: entries(channels),
                },
                DeviceSnapshot {
                    device: Arc::new(gpu),
                    channels: entries(gpu_channels),
                },
            ],
        }
    }

    fn id(raw: &str) -> ChannelId {
        ChannelId::from_raw(raw)
    }

    #[test]
    fn groups_nest_when_large_and_stay_flat_when_small() {
        let snapshot = snapshot();
        let config = Config::default();
        let layout = layout(&snapshot, &config, "");
        assert_eq!(
            layout.rows,
            vec![
                Row::Device(0),
                Row::Channel {
                    d: 0,
                    c: 0,
                    depth: 1
                },
                Row::Group(0),
                Row::Channel {
                    d: 0,
                    c: 1,
                    depth: 2
                },
                Row::Channel {
                    d: 0,
                    c: 2,
                    depth: 2
                },
                Row::Channel {
                    d: 0,
                    c: 3,
                    depth: 2
                },
                Row::Channel {
                    d: 0,
                    c: 4,
                    depth: 2
                },
                Row::Channel {
                    d: 0,
                    c: 5,
                    depth: 1
                },
                // Two throttle rows are too few for a sub-group.
                Row::Channel {
                    d: 0,
                    c: 6,
                    depth: 1
                },
                Row::Channel {
                    d: 0,
                    c: 7,
                    depth: 1
                },
                Row::Device(1),
                Row::Channel {
                    d: 1,
                    c: 0,
                    depth: 1
                },
            ]
        );
        assert_eq!(layout.groups[0].name, "Core Clocks");
        assert_eq!(layout.groups[0].members, vec![1, 2, 3, 4]);
        assert_eq!(layout.shown, 9);
    }

    #[test]
    fn collapse_state_hides_rows() {
        let snapshot = snapshot();
        let mut config = Config::default();
        config.set_group_collapsed(&DeviceId::new("cpu:0"), "Core Clocks", true);
        config.set_collapsed(&DeviceId::new("gpu:0"), true);
        let rows = layout(&snapshot, &config, "").rows;
        assert_eq!(rows.len(), 12 - 4 - 1);
        assert!(rows.contains(&Row::Group(0)));
        assert_eq!(rows.last(), Some(&Row::Device(1)));
    }

    #[test]
    fn hidden_rows_are_left_out_unless_shown_and_are_counted() {
        let snapshot = snapshot();
        let mut config = Config::default();
        for key in ["cpu:0/clock.core0", "cpu:0/clock.core1", "gpu:0/clock"] {
            config.update_channel(&id(key), |c| c.hidden = true);
        }
        // Not present on this machine: not counted in the title.
        config.update_channel(&id("cpu:9/gone"), |c| c.hidden = true);
        assert_eq!(hidden_count(&snapshot, &config), 3);

        let layout_hidden = layout(&snapshot, &config, "");
        // Two clocks left: below the threshold, so no sub-group row.
        assert!(layout_hidden.groups.is_empty());
        assert!(!layout_hidden.rows.contains(&Row::Channel {
            d: 0,
            c: 1,
            depth: 1
        }));
        assert_eq!(layout_hidden.rows.last(), Some(&Row::Device(1)));

        config.general.show_hidden = true;
        let shown = layout(&snapshot, &config, "");
        assert_eq!(shown.groups.len(), 1);
        assert_eq!(
            shown.rows.last(),
            Some(&Row::Channel {
                d: 1,
                c: 0,
                depth: 1
            })
        );
    }

    #[test]
    fn the_filter_matches_names_groups_and_devices_and_expands_matches() {
        let snapshot = snapshot();
        let mut config = Config::default();
        config.set_collapsed(&DeviceId::new("cpu:0"), true);
        config.set_group_collapsed(&DeviceId::new("cpu:0"), "Core Clocks", true);
        config.update_channel(&id("cpu:0/usage.total"), |c| c.label = Some("Busy".into()));

        let by_name = layout(&snapshot, &config, "core #2");
        assert!(by_name.filtered);
        assert_eq!(by_name.shown, 1);
        assert_eq!(
            by_name.rows,
            vec![
                Row::Device(0),
                Row::Channel {
                    d: 0,
                    c: 3,
                    depth: 1
                }
            ]
        );

        let by_group = layout(&snapshot, &config, "CLOCKS");
        assert_eq!(by_group.shown, 4);
        assert_eq!(by_group.groups.len(), 1, "expanded although collapsed");
        assert_eq!(by_group.rows.len(), 1 + 1 + 4);

        let by_new_name = layout(&snapshot, &config, "busy");
        assert_eq!(
            by_new_name.rows[1],
            Row::Channel {
                d: 0,
                c: 5,
                depth: 1
            }
        );
        let by_old_name = layout(&snapshot, &config, "total cpu");
        assert_eq!(by_old_name.shown, 1);

        let by_device = layout(&snapshot, &config, "gpu [#0]");
        assert_eq!(
            by_device.rows,
            vec![
                Row::Device(1),
                Row::Channel {
                    d: 1,
                    c: 0,
                    depth: 1
                }
            ]
        );

        assert!(
            layout(&snapshot, &config, "nothing like this")
                .rows
                .is_empty()
        );
    }

    #[test]
    fn group_rows_summarise_their_members() {
        let snapshot = snapshot();
        let device = &snapshot.devices[0];
        let clocks = summarise(device, &[1, 2, 3, 4]).unwrap();
        assert_eq!(clocks.min, Some(10.0));
        assert_eq!(clocks.max, Some(45.0));
        assert_eq!(clocks.current, Some(45.0));
        assert!(
            summarise(device, &[0, 1]).is_none(),
            "°C and MHz do not mix"
        );
    }

    #[test]
    fn collapse_all_round_trips() {
        let snapshot = snapshot();
        let mut config = Config::default();
        assert!(!TableState::all_collapsed(&snapshot, &config));
        TableState::set_all_collapsed(&snapshot, &mut config, true);
        assert!(TableState::all_collapsed(&snapshot, &config));
        TableState::set_all_collapsed(&snapshot, &mut config, false);
        assert!(config.devices.is_empty());
    }
}

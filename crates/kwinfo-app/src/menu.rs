//! The table's context menus: sensor rows, device rows, sub-group rows and the header.

use egui::{Button, Checkbox, Color32, Response, RichText, Role, Sense, Stroke, StrokeKind, Ui};
use egui::{WidgetInfo, vec2};
use kwinfo_core::config::{Column, Config};
use kwinfo_core::{ChannelId, ChannelSnapshot, DeviceSnapshot, Rgb, TemperatureUnit, format_value};

use kwinfo_diag::action;

use crate::table::TableAction;
use crate::theme::ThemeTokens;

/// Quick picks of the Custom colour menu. A colour is stored as picked; each theme
/// adjusts its lightness for contrast ([`ThemeTokens::custom_text`]).
pub const SWATCHES: [(&str, Rgb); 8] = [
    ("Red", Rgb::new(0xe5, 0x39, 0x35)),
    ("Orange", Rgb::new(0xff, 0x8c, 0x00)),
    ("Yellow", Rgb::new(0xfd, 0xd8, 0x35)),
    ("Green", Rgb::new(0x43, 0xa0, 0x47)),
    ("Teal", Rgb::new(0x00, 0x89, 0x7b)),
    ("Blue", Rgb::new(0x1e, 0x88, 0xe5)),
    ("Purple", Rgb::new(0x8e, 0x24, 0xaa)),
    ("Pink", Rgb::new(0xd8, 0x1b, 0x60)),
];

const MENU_WIDTH: f32 = 200.0;

/// The ids of some of a device's channels, by index.
fn channel_ids<'a>(
    device: &'a DeviceSnapshot,
    members: impl IntoIterator<Item = usize> + 'a,
) -> impl Iterator<Item = ChannelId> + 'a {
    members
        .into_iter()
        .map(move |c| device.channels[c].channel.id.clone())
}

/// The menu of a sensor row.
pub fn channel(
    ui: &mut Ui,
    entry: &ChannelSnapshot,
    temperature: TemperatureUnit,
    tokens: &ThemeTokens,
    config: &mut Config,
    actions: &mut Vec<TableAction>,
) {
    let channel = &entry.channel;
    let id = &channel.id;
    let settings = config.channel(id).cloned().unwrap_or_default();
    ui.set_min_width(MENU_WIDTH);

    if ui
        .button(RichText::new("Show Graph").strong())
        .on_hover_text("Graph windows arrive in milestone M3. Double-clicking a row does the same.")
        .clicked()
    {
        actions.push(TableAction::ShowGraph(id.clone()));
    }
    ui.separator();
    ui.add_enabled(false, Button::new("Add to tray"))
        .on_disabled_hover_text("Tray values arrive in milestone M5");
    ui.separator();
    if ui.button("Rename…").clicked() {
        actions.push(TableAction::Rename(id.clone()));
    }
    let hide = if settings.hidden { "Unhide" } else { "Hide" };
    if ui.button(hide).clicked() {
        action!("sensor.hide", channel = %id, hidden = !settings.hidden);
        config.update_channel(id, |c| c.hidden = !settings.hidden);
    }
    ui.menu_button("Custom colour", |ui| {
        color_menu(ui, id, settings.color, tokens, config, actions);
    });
    ui.separator();
    if config.is_device_disabled(&channel.device) {
        if ui
            .button("Enable monitoring of the device")
            .on_hover_text("Monitoring is disabled for the whole device")
            .clicked()
        {
            action!("device.monitoring", device = %channel.device, enabled = true);
            config.update_device(&channel.device, |d| d.disabled = false);
        }
    } else {
        let (label, hint) = if settings.disabled {
            ("Enable monitoring", "Poll this sensor again")
        } else {
            (
                "Disable monitoring",
                "Stop polling this sensor; its values are cleared",
            )
        };
        if ui.button(label).on_hover_text(hint).clicked() {
            action!("sensor.monitoring", channel = %id, enabled = settings.disabled);
            config.update_channel(id, |c| c.disabled = !settings.disabled);
        }
    }
    ui.separator();
    if ui
        .button("Reset this sensor")
        .on_hover_text("Reset minimum, maximum and average of this sensor")
        .clicked()
    {
        actions.push(TableAction::Reset(vec![id.clone()]));
    }
    if ui.button("Copy value").clicked() {
        let text = format_value(
            entry.stats.current(),
            channel.unit,
            channel.decimals,
            temperature,
        );
        action!("sensor.copy", channel = %id);
        ui.ctx().copy_text(text.clone());
        actions.push(TableAction::Copied(text));
    }
}

fn color_menu(
    ui: &mut Ui,
    id: &ChannelId,
    current: Option<Rgb>,
    tokens: &ThemeTokens,
    config: &mut Config,
    actions: &mut Vec<TableAction>,
) {
    egui::Grid::new("colour-swatches")
        .spacing([2.0, 2.0])
        .show(ui, |ui| {
            for (i, &(name, rgb)) in SWATCHES.iter().enumerate() {
                if swatch(
                    ui,
                    tokens.custom_text(rgb),
                    name,
                    current == Some(rgb),
                    tokens,
                )
                .clicked()
                {
                    action!("sensor.colour", channel = %id, colour = %rgb);
                    config.update_channel(id, |c| c.color = Some(rgb));
                }
                if i % 4 == 3 {
                    ui.end_row();
                }
            }
        });
    ui.separator();
    if ui.button("Other colour…").clicked() {
        actions.push(TableAction::PickColor(id.clone()));
    }
    if ui
        .add_enabled(current.is_some(), Button::new("Default colour"))
        .clicked()
    {
        action!("sensor.colour", channel = %id, colour = "default");
        config.update_channel(id, |c| c.color = None);
    }
}

/// A clickable colour square, named for screen readers.
fn swatch(
    ui: &mut Ui,
    color: Color32,
    name: &str,
    selected: bool,
    tokens: &ThemeTokens,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
    response.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, selected, name));
    if ui.is_rect_visible(rect) {
        let stroke = if selected {
            Stroke::new(2.0, tokens.selection_outline)
        } else if response.hovered() {
            Stroke::new(1.0, tokens.accent)
        } else {
            Stroke::new(1.0, tokens.grid)
        };
        ui.painter().rect_filled(rect.shrink(4.0), 2.0, color);
        ui.painter()
            .rect_stroke(rect.shrink(1.5), 3.0, stroke, StrokeKind::Middle);
    }
    response.on_hover_text(name)
}

/// The menu of a device row.
pub fn device(
    ui: &mut Ui,
    device: &DeviceSnapshot,
    collapsed: bool,
    temperature: TemperatureUnit,
    config: &mut Config,
    actions: &mut Vec<TableAction>,
) {
    let id = &device.device.id;
    ui.set_min_width(MENU_WIDTH);
    if ui
        .button(if collapsed { "Expand" } else { "Collapse" })
        .clicked()
    {
        action!("device.collapse", device = %id, collapsed = !collapsed);
        config.set_collapsed(id, !collapsed);
    }
    ui.separator();
    if ui
        .button("Reset values")
        .on_hover_text("Reset minimum, maximum and average of every sensor of this device")
        .clicked()
    {
        actions.push(TableAction::Reset(
            channel_ids(device, 0..device.channels.len()).collect(),
        ));
    }
    let disabled = config.is_device_disabled(id);
    let (label, hint) = if disabled {
        ("Enable monitoring", "Poll this device's sensors again")
    } else {
        (
            "Disable monitoring",
            "Stop polling every sensor of this device",
        )
    };
    if ui.button(label).on_hover_text(hint).clicked() {
        action!("device.monitoring", device = %id, enabled = disabled);
        config.update_device(id, |d| d.disabled = !disabled);
    }
    ui.separator();
    if ui
        .button("Copy values")
        .on_hover_text("Copy the device's sensors and values as tab-separated text")
        .clicked()
    {
        action!("device.copy", device = %id);
        let text = device_text(device, temperature, config);
        ui.ctx().copy_text(text);
        actions.push(TableAction::Copied(format!(
            "the values of {}",
            device.device.label
        )));
    }
}

/// The visible sensors of a device as tab-separated lines, for the clipboard.
pub fn device_text(
    device: &DeviceSnapshot,
    temperature: TemperatureUnit,
    config: &Config,
) -> String {
    let mut text = format!(
        "{}\nSensor\tCurrent\tMinimum\tMaximum\tAverage\n",
        device.device.label
    );
    for entry in &device.channels {
        let channel = &entry.channel;
        if config.is_hidden(&channel.id) {
            continue;
        }
        let f = |v| format_value(v, channel.unit, channel.decimals, temperature);
        let current = if entry.disabled || config.is_disabled(channel) {
            "Disabled".to_string()
        } else {
            f(entry.stats.current())
        };
        text.push_str(&format!(
            "{}\t{current}\t{}\t{}\t{}\n",
            config.label_of(channel),
            f(entry.stats.min()),
            f(entry.stats.max()),
            f(entry.stats.average()),
        ));
    }
    text
}

/// The menu of a sub-group row.
pub fn group(
    ui: &mut Ui,
    device: &DeviceSnapshot,
    name: &str,
    members: &[usize],
    collapsed: bool,
    config: &mut Config,
    actions: &mut Vec<TableAction>,
) {
    let id = &device.device.id;
    ui.set_min_width(MENU_WIDTH);
    if ui
        .button(if collapsed { "Expand" } else { "Collapse" })
        .clicked()
    {
        action!("group.collapse", device = %id, group = name, collapsed = !collapsed);
        config.set_group_collapsed(id, name, !collapsed);
    }
    ui.separator();
    if ui
        .button("Reset values")
        .on_hover_text("Reset minimum, maximum and average of every sensor in this group")
        .clicked()
    {
        actions.push(TableAction::Reset(
            channel_ids(device, members.iter().copied()).collect(),
        ));
    }
    let ids: Vec<ChannelId> = channel_ids(device, members.iter().copied()).collect();
    let all_hidden = ids.iter().all(|c| config.is_hidden(c));
    let label = if all_hidden { "Unhide all" } else { "Hide all" };
    if ui.button(label).clicked() {
        action!("group.hide", device = %id, group = name, hidden = !all_hidden);
        for c in &ids {
            config.update_channel(c, |s| s.hidden = !all_hidden);
        }
    }
}

/// The menu of the header: which columns to show.
pub fn columns(ui: &mut Ui, config: &mut Config, actions: &mut Vec<TableAction>) {
    ui.set_min_width(170.0);
    for column in Column::ALL {
        let mut visible = config.columns.is_visible(column);
        if ui
            .add_enabled(
                column.can_hide(),
                Checkbox::new(&mut visible, column.label()),
            )
            .changed()
        {
            action!("columns.visible", column = column.label(), visible);
            config.columns.get_mut(column).visible = visible;
        }
    }
    ui.separator();
    if ui.button("Reset column widths").clicked() {
        action!("columns.reset_widths");
        config.columns.reset_widths();
        actions.push(TableAction::ResetWidths);
    }
}

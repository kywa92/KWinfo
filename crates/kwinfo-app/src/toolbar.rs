//! The bottom toolbar: expand/collapse, reset, logging, elapsed time, polling period,
//! graphs, helper status, filter, settings and quit.

use egui::{Align, Button, ComboBox, Layout, Popup, RichText, Role, Ui, WidgetInfo, pos2};
use kwinfo_core::{GraphMode, Snapshot};

use kwinfo_diag::action;

use crate::app::{KwinfoApp, POLLING_CHOICES_MS, format_elapsed};
use crate::graphs::Placement;
use crate::icons::{self, Icon};
use crate::settings::SettingsTab;
use crate::table::TableState;

impl KwinfoApp {
    pub(crate) fn toolbar(&mut self, ui: &mut Ui, snapshot: &Snapshot) {
        let tokens = self.tokens();
        ui.horizontal(|ui| {
            let all_collapsed = TableState::all_collapsed(snapshot, &self.config);
            let (icon, name) = if all_collapsed {
                (Icon::ExpandAll, "Expand all")
            } else {
                (Icon::CollapseAll, "Collapse all")
            };
            if icons::button(ui, icon, None, name)
                .on_hover_text(name)
                .clicked()
            {
                action!("table.collapse_all", collapsed = !all_collapsed);
                TableState::set_all_collapsed(snapshot, &mut self.config, !all_collapsed);
            }
            if icons::button(ui, Icon::Reset, Some("Reset"), "Reset")
                .on_hover_text("Reset minimum, maximum and average of every sensor")
                .clicked()
            {
                action!("stats.reset_all");
                self.sampler.reset_all();
            }
            ui.add_enabled_ui(false, |ui| {
                icons::button(ui, Icon::Document, None, "Logging")
                    .on_disabled_hover_text("CSV logging arrives in milestone M5");
            });
            ui.separator();
            icons::icon(ui, Icon::Stopwatch, tokens.text_secondary);
            ui.monospace(format_elapsed(snapshot.elapsed()))
                .on_hover_text("Time since the statistics were last reset");
            ui.separator();
            icons::icon(ui, Icon::Gauge, tokens.text_secondary);
            self.polling_combo(ui, "toolbar-polling", None);
            ui.separator();
            self.graphs_button(ui);

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icons::button(ui, Icon::Power, None, "Quit")
                    .on_hover_text("Save the settings and quit")
                    .clicked()
                {
                    self.quit(ui.ctx());
                }
                if icons::button(ui, Icon::Gear, Some("Settings"), "Settings").clicked() {
                    self.set_settings_open(!self.settings.open);
                }
                if icons::button(ui, Icon::Search, None, "Filter")
                    .on_hover_text("Filter sensors by name (Ctrl+F)")
                    .clicked()
                {
                    let open = !self.table.filter_open;
                    self.set_filter_open(open, "toolbar");
                }
                self.helper_status(ui);
                ui.add_space(4.0);
                let status = match &self.flash {
                    Some((text, _)) => RichText::new(text).color(tokens.text_primary),
                    None => RichText::new(format!(
                        "{} sensors · {:.1} ms",
                        snapshot.channel_count(),
                        snapshot.tick_cost.as_secs_f64() * 1000.0
                    ))
                    .color(tokens.text_secondary),
                };
                ui.add(egui::Label::new(status).truncate())
                    .on_hover_text("Sensors listed, and how long the last poll took");
            });
        });
    }

    /// The Graphs menu: tile, close all, and where graphs are shown. Shows how many
    /// graphs are open.
    fn graphs_button(&mut self, ui: &mut Ui) {
        let count = self.graphs.len();
        let text = (count > 0).then(|| count.to_string());
        let response = icons::button(ui, Icon::Graph, text.as_deref(), "Graphs").on_hover_text(
            "Graphs: tile or close them, and choose whether they open in their own windows. \
             Open one by double-clicking a sensor.",
        );
        let _ = Popup::menu(&response).show(|ui| self.graphs_menu(ui));
    }

    fn graphs_menu(&mut self, ui: &mut Ui) {
        let tokens = self.tokens();
        let ctx = ui.ctx().clone();
        ui.set_min_width(230.0);
        let count = self.graphs.len();
        let windows = self.graph_placement(&ctx) == Placement::Windows;
        let why_not = if count == 0 {
            "No graph is open. Double-click a sensor to open one."
        } else {
            "The graphs are inside the main window, where they are always tiled."
        };
        if ui
            .add_enabled(count > 0 && windows, Button::new("Tile graph windows"))
            .on_disabled_hover_text(why_not)
            .clicked()
        {
            self.tile_graphs(&ctx);
        }
        if ui
            .add_enabled(count > 0, Button::new("Close all graphs"))
            .clicked()
        {
            self.close_all_graphs();
        }
        ui.separator();
        ui.label(
            RichText::new("Show graphs")
                .small()
                .color(tokens.text_secondary),
        );
        for mode in GraphMode::ALL {
            if ui
                .radio(self.config.graphs.mode == mode, mode.label())
                .clicked()
            {
                self.set_graph_mode(mode);
            }
        }
    }

    /// The polling-period selector. Named by `label` (a visible label's id) or, without
    /// one, by its own accessible name, so screen readers know what the box is for.
    pub(crate) fn polling_combo(&mut self, ui: &mut Ui, id: &str, label: Option<egui::Id>) {
        let mut chosen = None;
        let response = ComboBox::from_id_salt(id)
            .width(84.0)
            .selected_text(format!("{} ms", self.interval_ms))
            .show_ui(ui, |ui| {
                for ms in POLLING_CHOICES_MS {
                    if ui
                        .selectable_label(self.interval_ms == ms, format!("{ms} ms"))
                        .clicked()
                    {
                        chosen = Some(ms);
                    }
                }
            })
            .response;
        if let Some(ms) = chosen {
            self.set_interval(ms);
        }
        match label {
            Some(label) => {
                response.labelled_by(label);
            }
            None => {
                let value = format!("{} ms", self.interval_ms);
                response.widget_info(|| {
                    let mut info = WidgetInfo::labeled(Role::ComboBox, true, "Polling period");
                    info.current_text_value = Some(value.clone());
                    info
                });
                response.on_hover_text("Polling period: how often the sensors are read");
            }
        }
    }

    /// Placeholder for the privileged helper's status (milestone M4): a grey dot means
    /// not running. Opens the Helper tab of the settings.
    fn helper_status(&mut self, ui: &mut Ui) {
        let tokens = self.tokens();
        let response = icons::button(ui, Icon::Shield, Some("Helper"), "Helper: not running")
            .on_hover_text(
                "The privileged helper is not running. Root-only sensors (CPU package \
                 power, S.M.A.R.T., GPU load) arrive with it in milestone M4.",
            );
        // The status dot sits on the shield's lower right corner.
        let padding = ui.spacing().button_padding.x;
        let dot = pos2(
            response.rect.left() + padding + icons::ICON_SIZE - 1.0,
            response.rect.center().y + 4.0,
        );
        ui.painter().circle_filled(dot, 3.2, tokens.text_secondary);
        ui.painter()
            .circle_stroke(dot, 3.2, egui::Stroke::new(1.5, tokens.window_bg));
        if response.clicked() {
            self.settings.tab = SettingsTab::Helper;
            self.set_settings_open(true);
        }
    }
}

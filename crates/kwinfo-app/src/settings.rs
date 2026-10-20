//! The settings window (PLAN.md 4.4): General, Theme, Columns, Graphs, Hidden sensors,
//! Logging, Helper and Alerts.

use egui::{Align, Button, Checkbox, ComboBox, Layout, RichText, ScrollArea, Spinner, Ui};
use kwinfo_core::config::{Column, HISTORY_MINUTES_CHOICES};
use kwinfo_core::{ChannelId, DeviceId, GraphMode, Snapshot, TemperatureUnit, ThemeMode};

use kwinfo_diag::action;

use crate::app::KwinfoApp;
use crate::graph::span_label;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsTab {
    #[default]
    General,
    Theme,
    Columns,
    Graphs,
    Hidden,
    Logging,
    Helper,
    Alerts,
}

impl SettingsTab {
    pub const ALL: [SettingsTab; 8] = [
        SettingsTab::General,
        SettingsTab::Theme,
        SettingsTab::Columns,
        SettingsTab::Graphs,
        SettingsTab::Hidden,
        SettingsTab::Logging,
        SettingsTab::Helper,
        SettingsTab::Alerts,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingsTab::General => "General",
            SettingsTab::Theme => "Theme",
            SettingsTab::Columns => "Columns",
            SettingsTab::Graphs => "Graphs",
            SettingsTab::Hidden => "Hidden sensors",
            SettingsTab::Logging => "Logging",
            SettingsTab::Helper => "Helper",
            SettingsTab::Alerts => "Alerts",
        }
    }
}

#[derive(Debug, Default)]
pub struct SettingsState {
    pub open: bool,
    pub tab: SettingsTab,
}

impl KwinfoApp {
    pub(crate) fn settings_window(&mut self, ctx: &egui::Context, snapshot: &Snapshot) {
        let was_open = self.settings.open;
        let mut open = was_open;
        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(470.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for tab in SettingsTab::ALL {
                        if ui
                            .selectable_value(&mut self.settings.tab, tab, tab.label())
                            .clicked()
                        {
                            action!("settings.tab", tab = tab.label());
                        }
                    }
                });
                ui.separator();
                ui.add_space(4.0);
                match self.settings.tab {
                    SettingsTab::General => self.general_tab(ui),
                    SettingsTab::Theme => self.theme_tab(ui),
                    SettingsTab::Columns => self.columns_tab(ui),
                    SettingsTab::Graphs => self.graphs_tab(ui),
                    SettingsTab::Hidden => self.hidden_tab(ui, snapshot),
                    SettingsTab::Logging => self.logging_tab(ui),
                    SettingsTab::Helper => self.helper_tab(ui),
                    SettingsTab::Alerts => self.alerts_tab(ui, snapshot),
                }
            });
        if open != was_open {
            // The window's own close button.
            self.set_settings_open(open);
        }
    }

    fn general_tab(&mut self, ui: &mut Ui) {
        let tokens = self.tokens();
        egui::Grid::new("settings-general")
            .num_columns(2)
            .striped(false)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label("Temperature");
                ui.horizontal(|ui| {
                    let unit = &mut self.config.general.temperature_unit;
                    let before = *unit;
                    ui.radio_value(unit, TemperatureUnit::Celsius, "Celsius");
                    ui.radio_value(unit, TemperatureUnit::Fahrenheit, "Fahrenheit");
                    if *unit != before {
                        action!("settings.temperature_unit", unit = ?*unit);
                    }
                });
                ui.end_row();

                let polling = ui.label("Polling period");
                self.polling_combo(ui, "settings-polling", Some(polling.id));
                ui.end_row();

                ui.label("Hidden sensors");
                if ui
                    .checkbox(
                        &mut self.config.general.show_hidden,
                        "Show them, greyed out, to un-hide them",
                    )
                    .changed()
                {
                    action!(
                        "settings.show_hidden",
                        show = self.config.general.show_hidden
                    );
                }
                ui.end_row();

                ui.label("Diagnostics");
                if ui
                    .add_enabled(
                        !self.report_running(),
                        Button::new("Save diagnostic report…"),
                    )
                    .on_hover_text(
                        "Write the logs, a machine summary, the settings and the sensors \
                         shown here into one text file to attach to a bug report",
                    )
                    .clicked()
                {
                    self.save_report(ui.ctx());
                }
                ui.end_row();
            });
        if self.report_running() {
            ui.horizontal(|ui| {
                ui.add(Spinner::new());
                ui.label("Writing the report…");
            });
        } else if let Some(result) = &self.report_result {
            match result {
                Ok(path) => {
                    let path = path.display().to_string();
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Report saved to");
                        ui.label(RichText::new(&path).monospace());
                        if ui.small_button("Copy path").clicked() {
                            ui.ctx().copy_text(path.clone());
                        }
                    });
                }
                Err(err) => {
                    ui.label(
                        RichText::new(format!("The report could not be written: {err}"))
                            .color(tokens.text_primary),
                    );
                }
            }
        }
        ui.add_space(8.0);
        let saved_to = match self.persistence.path() {
            Some(path) => format!("Settings are saved to {}", path.display()),
            None => "Settings are kept for this session only.".to_string(),
        };
        ui.label(RichText::new(saved_to).small().color(tokens.text_secondary));
    }

    fn graphs_tab(&mut self, ui: &mut Ui) {
        let tokens = self.tokens();
        egui::Grid::new("settings-graphs")
            .num_columns(2)
            .striped(false)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label("Show graphs");
                ui.vertical(|ui| {
                    for mode in GraphMode::ALL {
                        if ui
                            .radio(self.config.graphs.mode == mode, mode.label())
                            .clicked()
                        {
                            self.set_graph_mode(mode);
                        }
                    }
                });
                ui.end_row();

                let label = ui.label("History");
                let minutes = self.config.graphs.history_minutes;
                let mut chosen = None;
                ComboBox::from_id_salt("settings-history")
                    .width(84.0)
                    .selected_text(span_label(minutes * 60))
                    .show_ui(ui, |ui| {
                        for choice in HISTORY_MINUTES_CHOICES {
                            if ui
                                .selectable_label(minutes == choice, span_label(choice * 60))
                                .clicked()
                            {
                                chosen = Some(choice);
                            }
                        }
                    })
                    .response
                    .labelled_by(label.id);
                if let Some(choice) = chosen {
                    self.set_history_minutes(choice);
                }
                ui.end_row();
            });
        ui.add_space(6.0);
        let points = self.config.graphs.history_points(self.interval_ms);
        let notes = [
            format!(
                "Every sensor keeps up to {points} readings for its graph at the current \
                 polling period of {} ms. Right-click a graph to choose how much of it to show.",
                self.interval_ms
            ),
            if self.wayland {
                "This is a Wayland session: the desktop decides where windows open, so \
                 Automatic shows the graphs inside the main window, and separate graph \
                 windows can be neither tiled nor reopened where they were."
                    .to_string()
            } else {
                "Automatic opens every graph in its own window, except on Wayland, where the \
                 desktop decides where windows go and the graphs go inside the main window."
                    .to_string()
            },
        ];
        for note in notes {
            ui.label(RichText::new(note).small().color(tokens.text_secondary));
        }
    }

    fn theme_tab(&mut self, ui: &mut Ui) {
        let tokens = self.tokens();
        ui.horizontal(|ui| {
            for mode in ThemeMode::ALL {
                if ui.radio(self.theme_mode == mode, mode.label()).clicked() {
                    self.set_theme_mode(mode);
                }
            }
        });
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "Follow system switches between the dark and the light palette with the \
                 desktop, live.",
            )
            .color(tokens.text_secondary),
        );
    }

    fn columns_tab(&mut self, ui: &mut Ui) {
        let tokens = self.tokens();
        ui.label("Show these columns:");
        ui.horizontal_wrapped(|ui| {
            for column in Column::ALL {
                let mut visible = self.config.columns.is_visible(column);
                if ui
                    .add_enabled(
                        column.can_hide(),
                        Checkbox::new(&mut visible, column.label()),
                    )
                    .changed()
                {
                    action!("columns.visible", column = column.label(), visible);
                    self.config.columns.get_mut(column).visible = visible;
                }
            }
        });
        ui.add_space(6.0);
        if ui.button("Reset column widths").clicked() {
            action!("columns.reset_widths");
            self.config.columns.reset_widths();
            self.table.reset_widths();
        }
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "Drag the dividers in the header to change widths; right-click the header \
                 for the same choices. Both are remembered.",
            )
            .color(tokens.text_secondary),
        );
    }

    fn hidden_tab(&mut self, ui: &mut Ui, snapshot: &Snapshot) {
        let tokens = self.tokens();
        if ui
            .checkbox(
                &mut self.config.general.show_hidden,
                "Show hidden sensors in the table, greyed out",
            )
            .changed()
        {
            action!(
                "settings.show_hidden",
                show = self.config.general.show_hidden
            );
        }
        ui.add_space(6.0);

        let describe = |id: &ChannelId| match snapshot.find(id) {
            Some(entry) => {
                let device = snapshot
                    .devices
                    .iter()
                    .find(|d| d.device.id == entry.channel.device)
                    .map_or("", |d| d.device.label.as_str());
                format!("{} — {device}", self.config.label_of(&entry.channel))
            }
            None => format!("{id} (not present)"),
        };
        let hidden: Vec<(ChannelId, String)> = self
            .config
            .channels
            .iter()
            .filter(|(_, c)| c.hidden)
            .map(|(id, _)| (id.clone(), describe(id)))
            .collect();
        let disabled: Vec<(ChannelId, String)> = self
            .config
            .channels
            .iter()
            .filter(|(_, c)| c.disabled)
            .map(|(id, _)| (id.clone(), describe(id)))
            .collect();
        let disabled_devices: Vec<(DeviceId, String)> = self
            .config
            .devices
            .iter()
            .filter(|(_, d)| d.disabled)
            .map(|(id, _)| {
                let label = snapshot
                    .devices
                    .iter()
                    .find(|d| &d.device.id == id)
                    .map_or_else(|| format!("{id} (not present)"), |d| d.device.label.clone());
                (id.clone(), label)
            })
            .collect();

        ui.horizontal(|ui| {
            ui.strong(format!("Hidden ({})", hidden.len()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(!hidden.is_empty(), Button::new("Unhide all"))
                    .clicked()
                {
                    action!("sensor.unhide_all", count = hidden.len());
                    for (id, _) in &hidden {
                        self.config.update_channel(id, |c| c.hidden = false);
                    }
                }
            });
        });
        ScrollArea::vertical()
            .id_salt("hidden-list")
            .max_height(150.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if hidden.is_empty() {
                    ui.label(
                        RichText::new("Nothing is hidden. Right-click a sensor and choose Hide.")
                            .color(tokens.text_secondary),
                    );
                }
                for (id, text) in &hidden {
                    ui.horizontal(|ui| {
                        if ui.button("Unhide").clicked() {
                            action!("sensor.hide", channel = %id, hidden = false);
                            self.config.update_channel(id, |c| c.hidden = false);
                        }
                        ui.label(text);
                    });
                }
            });

        ui.add_space(8.0);
        ui.strong(format!(
            "Monitoring disabled ({})",
            disabled.len() + disabled_devices.len()
        ));
        ScrollArea::vertical()
            .id_salt("disabled-list")
            .max_height(150.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if disabled.is_empty() && disabled_devices.is_empty() {
                    ui.label(
                        RichText::new(
                            "Every sensor is polled. Right-click a sensor or a device and \
                             choose Disable monitoring to stop polling it.",
                        )
                        .color(tokens.text_secondary),
                    );
                }
                for (id, text) in &disabled_devices {
                    ui.horizontal(|ui| {
                        if ui.button("Enable").clicked() {
                            action!("device.monitoring", device = %id, enabled = true);
                            self.config.update_device(id, |d| d.disabled = false);
                        }
                        ui.label(format!("{text} (whole device)"));
                    });
                }
                for (id, text) in &disabled {
                    ui.horizontal(|ui| {
                        if ui.button("Enable").clicked() {
                            action!("sensor.monitoring", channel = %id, enabled = true);
                            self.config.update_channel(id, |c| c.disabled = false);
                        }
                        ui.label(text);
                    });
                }
            });
    }
}

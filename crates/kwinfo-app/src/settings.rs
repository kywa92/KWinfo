//! The settings window (PLAN.md 4.4): General, Theme, Columns and Hidden sensors, plus
//! the Logging, Helper and Alerts tabs that later milestones fill in.

use egui::{Align, Button, Checkbox, Layout, RichText, ScrollArea, Ui};
use kwinfo_core::config::Column;
use kwinfo_core::{ChannelId, DeviceId, Snapshot, TemperatureUnit, ThemeMode};

use kwinfo_diag::action;

use crate::app::KwinfoApp;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsTab {
    #[default]
    General,
    Theme,
    Columns,
    Hidden,
    Logging,
    Helper,
    Alerts,
}

impl SettingsTab {
    pub const ALL: [SettingsTab; 7] = [
        SettingsTab::General,
        SettingsTab::Theme,
        SettingsTab::Columns,
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
        let mut open = self.settings.open;
        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(470.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for tab in SettingsTab::ALL {
                        ui.selectable_value(&mut self.settings.tab, tab, tab.label());
                    }
                });
                ui.separator();
                ui.add_space(4.0);
                match self.settings.tab {
                    SettingsTab::General => self.general_tab(ui),
                    SettingsTab::Theme => self.theme_tab(ui),
                    SettingsTab::Columns => self.columns_tab(ui),
                    SettingsTab::Hidden => self.hidden_tab(ui, snapshot),
                    SettingsTab::Logging => self.later(
                        ui,
                        "CSV logging (start and stop, interval, choice of sensors, daily files) \
                         arrives in milestone M5.",
                    ),
                    SettingsTab::Helper => self.later(
                        ui,
                        "The privileged helper (RAPL power, MSRs, S.M.A.R.T., GPU load) and \
                         the buttons to install or start it arrive in milestone M4.",
                    ),
                    SettingsTab::Alerts => self.later(
                        ui,
                        "Per-sensor alerts with desktop notifications arrive in milestone M5.",
                    ),
                }
            });
        self.settings.open = open;
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
            });
        ui.add_space(8.0);
        let saved_to = match self.persistence.path() {
            Some(path) => format!("Settings are saved to {}", path.display()),
            None => "Settings are kept for this session only.".to_string(),
        };
        ui.label(RichText::new(saved_to).small().color(tokens.text_secondary));
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

    fn later(&self, ui: &mut Ui, text: &str) {
        ui.add_space(6.0);
        ui.label(RichText::new(text).color(self.tokens().text_secondary));
        ui.add_space(6.0);
    }
}

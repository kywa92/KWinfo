//! The main window: the Sensor Status table, the bottom toolbar and the settings dialog.

use std::time::Duration;

use egui::{Align, Button, ComboBox, Frame, Layout, Margin, RichText};
use kwinfo_core::{BackendChoice, BackendSet, Sampler, SamplerConfig, Snapshot, TemperatureUnit};

use crate::system_theme::SystemTheme;
use crate::table::{self, TableState};
use crate::theme::{self, Palette, ThemeMode, ThemeTokens};

pub const TITLE: &str = "KWinfo – Sensor Status";
pub const APP_ID: &str = "io.github.kylanwalters.KWinfo";

const POLLING_CHOICES_MS: [u64; 5] = [250, 500, 1000, 2000, 5000];

pub struct KwinfoApp {
    sampler: Sampler,
    notes: Vec<String>,
    theme_mode: ThemeMode,
    applied: Option<Palette>,
    system_theme: SystemTheme,
    temperature: TemperatureUnit,
    table: TableState,
    show_settings: bool,
    interval_ms: u64,
}

impl KwinfoApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        choice: BackendChoice,
        interval_ms: u64,
        theme_mode: ThemeMode,
    ) -> Self {
        let set = BackendSet::build(choice);
        for note in &set.notes {
            tracing::info!("{note}");
        }
        let sampler = Sampler::start(
            set.backends,
            SamplerConfig {
                interval: Duration::from_millis(interval_ms),
                ..SamplerConfig::default()
            },
        );
        let ctx = cc.egui_ctx.clone();
        sampler.set_on_tick(move || ctx.request_repaint());
        theme::install(&cc.egui_ctx);
        let system_theme = SystemTheme::start(cc.egui_ctx.clone());
        Self {
            sampler,
            notes: set.notes,
            theme_mode,
            applied: None,
            system_theme,
            temperature: TemperatureUnit::Celsius,
            table: TableState::default(),
            show_settings: false,
            interval_ms,
        }
    }

    fn effective_palette(&self, ctx: &egui::Context) -> Palette {
        match self.theme_mode {
            ThemeMode::Dark => Palette::Dark,
            ThemeMode::Light => Palette::Light,
            ThemeMode::Auto => self
                .system_theme
                .current()
                .or_else(|| ctx.system_theme().map(Palette::from_theme))
                .unwrap_or(Palette::Dark),
        }
    }

    fn tokens(&self) -> &'static ThemeTokens {
        ThemeTokens::for_palette(self.applied.unwrap_or(Palette::Dark))
    }

    /// The polling-period selector. `label` is the id of the text that names it, so
    /// screen readers (and the test harness) know what the box is for.
    fn polling_combo(&mut self, ui: &mut egui::Ui, id: &str, label: egui::Id) {
        ComboBox::from_id_salt(id)
            .width(90.0)
            .selected_text(format!("{} ms", self.interval_ms))
            .show_ui(ui, |ui| {
                for ms in POLLING_CHOICES_MS {
                    if ui
                        .selectable_value(&mut self.interval_ms, ms, format!("{ms} ms"))
                        .clicked()
                    {
                        self.sampler.set_interval(Duration::from_millis(ms));
                    }
                }
            })
            .response
            .labelled_by(label);
    }

    /// Opens or closes the settings window.
    pub fn set_settings_open(&mut self, open: bool) {
        self.show_settings = open;
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        ui.horizontal(|ui| {
            let all_collapsed = self.table.all_collapsed(snapshot);
            let label = if all_collapsed {
                "Expand all"
            } else {
                "Collapse all"
            };
            if ui.button(label).clicked() {
                self.table.set_all_collapsed(snapshot, !all_collapsed);
            }
            if ui
                .button("Reset")
                .on_hover_text("Reset minimum, maximum and average of every sensor")
                .clicked()
            {
                self.sampler.reset_all();
            }
            ui.add_enabled(false, Button::new("Logging"))
                .on_disabled_hover_text("CSV logging arrives in milestone M5");
            ui.separator();
            ui.monospace(format_elapsed(snapshot.elapsed()))
                .on_hover_text("Time since the statistics were last reset");
            ui.separator();
            let poll_label = ui.label("Poll:");
            self.polling_combo(ui, "toolbar-polling", poll_label.id);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Settings").clicked() {
                    self.show_settings = !self.show_settings;
                }
                let status = format!(
                    "{} sensors · tick {:.1} ms",
                    snapshot.channel_count(),
                    snapshot.tick_cost.as_secs_f64() * 1000.0
                );
                ui.label(RichText::new(status).color(self.tokens().text_secondary));
            });
        });
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                egui::Grid::new("settings-grid")
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        let theme_label = ui.label("Theme");
                        ComboBox::from_id_salt("settings-theme")
                            .selected_text(self.theme_mode.label())
                            .show_ui(ui, |ui| {
                                for mode in [ThemeMode::Auto, ThemeMode::Dark, ThemeMode::Light] {
                                    ui.selectable_value(&mut self.theme_mode, mode, mode.label());
                                }
                            })
                            .response
                            .labelled_by(theme_label.id);
                        ui.end_row();

                        ui.label("Temperature");
                        ui.horizontal(|ui| {
                            ui.radio_value(
                                &mut self.temperature,
                                TemperatureUnit::Celsius,
                                "Celsius",
                            );
                            ui.radio_value(
                                &mut self.temperature,
                                TemperatureUnit::Fahrenheit,
                                "Fahrenheit",
                            );
                        });
                        ui.end_row();

                        let polling_label = ui.label("Polling");
                        self.polling_combo(ui, "settings-polling", polling_label.id);
                        ui.end_row();
                    });
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Settings are not saved between runs yet (milestone M2).")
                        .small()
                        .color(self.tokens().text_secondary),
                );
            });
        self.show_settings = open;
    }

    fn empty_state(&self, ui: &mut egui::Ui) {
        let tokens = self.tokens();
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            ui.label(RichText::new("No sensors to show").size(18.0));
            ui.add_space(6.0);
            for note in &self.notes {
                ui.label(RichText::new(note).color(tokens.text_secondary));
            }
            if self.notes.is_empty() {
                ui.label(
                    RichText::new("Waiting for the first sampling tick…")
                        .color(tokens.text_secondary),
                );
            }
        });
    }
}

impl eframe::App for KwinfoApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let palette = self.effective_palette(ctx);
        if self.applied != Some(palette) {
            ctx.set_theme(palette.preference());
            self.applied = Some(palette);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let tokens = self.tokens();
        let snapshot = self.sampler.snapshot();
        egui::Panel::bottom("toolbar")
            .frame(
                Frame::new()
                    .fill(tokens.window_bg)
                    .inner_margin(Margin::symmetric(8, 6)),
            )
            .show(ui, |ui| self.toolbar(ui, &snapshot));
        egui::CentralPanel::default()
            .frame(Frame::new().fill(tokens.table_bg))
            .show(ui, |ui| {
                if snapshot.devices.is_empty() {
                    self.empty_state(ui);
                } else {
                    table::show(ui, &snapshot, &mut self.table, tokens, self.temperature);
                }
            });
        self.settings_window(ui.ctx());
    }
}

/// `H:MM:SS`, like HWiNFO's elapsed-time display.
pub fn format_elapsed(elapsed: Duration) -> String {
    let total = elapsed.as_secs();
    format!(
        "{}:{:02}:{:02}",
        total / 3600,
        (total / 60) % 60,
        total % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_is_h_mm_ss() {
        assert_eq!(format_elapsed(Duration::from_secs(0)), "0:00:00");
        assert_eq!(format_elapsed(Duration::from_secs(570)), "0:09:30");
        assert_eq!(
            format_elapsed(Duration::from_secs(3600 * 25 + 61)),
            "25:01:01"
        );
    }
}

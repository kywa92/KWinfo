//! CSV logging in the UI (PLAN.md section 2, item 9): the toolbar's Logging button,
//! Settings → Logging, and keeping the sampler's logger in step with the settings. The
//! logging itself runs on the sampler thread (`kwinfo_core::logging`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use egui::{ComboBox, RichText, TextEdit, Ui, pos2};
use kwinfo_core::config::{LOG_INTERVAL_CHOICES_MS, LogSensors, default_log_folder};
use kwinfo_core::logging::{LogSelection, LogSpec};
use kwinfo_diag::action;

use crate::app::KwinfoApp;
use crate::icons::{self, Icon};

/// `Every reading`, `5 s`, `1 min`.
pub fn interval_label(ms: u64) -> String {
    match ms {
        0 => "Every reading".to_string(),
        ms if ms >= 60_000 && ms % 60_000 == 0 => format!("{} min", ms / 60_000),
        ms if ms % 1000 == 0 => format!("{} s", ms / 1000),
        ms => format!("{ms} ms"),
    }
}

impl KwinfoApp {
    /// The log the settings describe.
    pub(crate) fn log_spec(&self) -> LogSpec {
        let settings = &self.config.logging;
        let selection = match settings.sensors {
            LogSensors::Visible => LogSelection::AllExcept(self.config.hidden_channels()),
            LogSensors::Marked => LogSelection::Only(self.config.logged_channels()),
        };
        let names: HashMap<_, _> = self
            .config
            .channels
            .iter()
            .filter_map(|(id, c)| Some((id.clone(), c.label.clone()?)))
            .collect();
        LogSpec {
            folder: settings.folder(),
            interval: Duration::from_millis(settings.interval_ms),
            selection,
            rotate_daily: settings.rotate_daily,
            names,
        }
    }

    /// Tells the sampler to start, stop or change the log when the wish or the settings
    /// changed. A changed log starts a new file.
    pub(crate) fn sync_logging(&mut self) {
        let wanted = self.logging_on.then(|| self.log_spec());
        if wanted != self.sent_log_spec {
            self.sampler.set_logging(wanted.clone());
            self.sent_log_spec = wanted;
        }
    }

    /// Starts or stops logging.
    pub fn set_logging(&mut self, on: bool) {
        if on {
            action!("logging.start", folder = %self.config.logging.folder().display());
            // Always a fresh logger, also after one stopped on an error.
            self.sent_log_spec = None;
        } else {
            action!("logging.stop", rows = self.sampler.logging().rows);
        }
        self.logging_on = on;
        self.sync_logging();
    }

    /// Whether logging is on, as the user asked.
    pub fn logging_on(&self) -> bool {
        self.logging_on
    }

    /// How the sampler's logging is going.
    pub fn logging_status(&self) -> kwinfo_core::logging::LogStatus {
        self.sampler.logging()
    }

    /// The toolbar button: a document, with a red dot while a log is being written.
    pub(crate) fn logging_button(&mut self, ui: &mut Ui) {
        let tokens = self.tokens();
        let status = self.sampler.logging();
        let failed = self.logging_on && !status.active && status.error.is_some();
        let (name, hover) = if failed {
            (
                "Logging stopped",
                format!(
                    "Logging stopped: {}. Click to start again.",
                    status.error.as_deref().unwrap_or("an error")
                ),
            )
        } else if self.logging_on {
            let file = status
                .path
                .as_ref()
                .map_or_else(|| "starting…".to_string(), |p| p.display().to_string());
            (
                "Stop logging",
                format!("Logging to {file} ({} rows). Click to stop.", status.rows),
            )
        } else {
            (
                "Start logging",
                "Log the sensors to a CSV file (Settings → Logging)".to_string(),
            )
        };
        let response = icons::button(ui, Icon::Document, None, name).on_hover_text(hover);
        if self.logging_on {
            let padding = ui.spacing().button_padding.y + 2.0;
            let dot = pos2(
                response.rect.left() + padding + icons::ICON_SIZE - 1.0,
                response.rect.center().y + 4.0,
            );
            let color = if failed {
                tokens.highlight
            } else {
                egui::Color32::from_rgb(0xe5, 0x39, 0x35)
            };
            ui.painter().circle_filled(dot, 3.2, color);
            ui.painter()
                .circle_stroke(dot, 3.2, egui::Stroke::new(1.5, tokens.window_bg));
        }
        if response.clicked() {
            self.set_logging(failed || !self.logging_on);
        }
    }

    pub(crate) fn logging_tab(&mut self, ui: &mut Ui) {
        let tokens = self.tokens();
        let default_folder = default_log_folder()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        egui::Grid::new("settings-logging")
            .num_columns(2)
            .striped(false)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                let label = ui.label("Folder");
                let edit = ui
                    .add(
                        TextEdit::singleline(&mut self.log_folder_text)
                            .hint_text(default_folder.as_str())
                            .desired_width(300.0),
                    )
                    .labelled_by(label.id);
                if edit.lost_focus() {
                    let text = self.log_folder_text.trim();
                    let folder = (!text.is_empty()).then(|| PathBuf::from(text));
                    if folder != self.config.logging.folder {
                        action!("settings.log_folder", folder = ?folder);
                        self.config.logging.folder = folder;
                    }
                }
                ui.end_row();

                let label = ui.label("Interval");
                let current = self.config.logging.interval_ms;
                let mut chosen = None;
                ComboBox::from_id_salt("settings-log-interval")
                    .width(120.0)
                    .selected_text(interval_label(current))
                    .show_ui(ui, |ui| {
                        for ms in LOG_INTERVAL_CHOICES_MS {
                            if ui
                                .selectable_label(current == ms, interval_label(ms))
                                .clicked()
                            {
                                chosen = Some(ms);
                            }
                        }
                    })
                    .response
                    .labelled_by(label.id);
                if let Some(ms) = chosen {
                    action!("settings.log_interval", interval_ms = ms);
                    self.config.logging.interval_ms = ms;
                }
                ui.end_row();

                ui.label("Sensors");
                ui.vertical(|ui| {
                    let marked = self.config.logged_channels().len();
                    for sensors in LogSensors::ALL {
                        let text = match sensors {
                            LogSensors::Marked => format!("{} ({marked} marked)", sensors.label()),
                            LogSensors::Visible => sensors.label().to_string(),
                        };
                        if ui
                            .radio(self.config.logging.sensors == sensors, text)
                            .clicked()
                        {
                            action!("settings.log_sensors", sensors = ?sensors);
                            self.config.logging.sensors = sensors;
                        }
                    }
                });
                ui.end_row();

                ui.label("Files");
                ui.vertical(|ui| {
                    if ui
                        .checkbox(
                            &mut self.config.logging.rotate_daily,
                            "Start a new file every day (UTC)",
                        )
                        .changed()
                    {
                        action!(
                            "settings.log_rotate",
                            daily = self.config.logging.rotate_daily
                        );
                    }
                    if ui
                        .checkbox(
                            &mut self.config.logging.autostart,
                            "Start logging when KWinfo starts",
                        )
                        .changed()
                    {
                        action!("settings.log_autostart", on = self.config.logging.autostart);
                    }
                });
                ui.end_row();
            });
        ui.add_space(6.0);
        let status = self.sampler.logging();
        ui.horizontal(|ui| {
            let text = if self.logging_on {
                "Stop logging"
            } else {
                "Start logging"
            };
            if ui.button(text).clicked() {
                self.set_logging(!self.logging_on);
            }
            if let Some(path) = &status.path
                && ui.small_button("Copy file path").clicked()
            {
                ui.ctx().copy_text(path.display().to_string());
            }
        });
        let state = match (&status.error, status.active, &status.path) {
            (Some(error), false, _) if self.logging_on => format!("Logging stopped: {error}"),
            (_, true, Some(path)) => format!(
                "Writing {} · {} rows · {} sensors",
                path.display(),
                status.rows,
                status.columns
            ),
            (_, true, None) => "Starting…".to_string(),
            (_, false, Some(path)) => format!("Last file: {}", path.display()),
            _ => "Not logging.".to_string(),
        };
        ui.label(RichText::new(state).small().color(tokens.text_secondary));
        ui.label(
            RichText::new(
                "One column per sensor, in its base unit (°C, V, W, bytes), with a UTC time \
                 stamp. A new file starts whenever the logged sensors change. Mark sensors \
                 with \"Log this sensor\" in their menu.",
            )
            .small()
            .color(tokens.text_secondary),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_have_short_names() {
        assert_eq!(interval_label(0), "Every reading");
        assert_eq!(interval_label(1000), "1 s");
        assert_eq!(interval_label(60_000), "1 min");
        assert_eq!(interval_label(1500), "1500 ms");
    }
}

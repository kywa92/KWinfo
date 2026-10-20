//! Alerts in the UI (PLAN.md section 2, item 11): checking the thresholds once per
//! sampling tick, highlighting the sensors in alert, desktop notifications, the alert
//! dialog and Settings → Alerts. The threshold logic is `kwinfo_core::alerts`.

use std::collections::HashSet;

use egui::{RichText, Ui};
use kwinfo_core::alerts::{AlertEvent, Breach};
use kwinfo_core::{ChannelId, Snapshot, format_value};
use kwinfo_diag::action;

use crate::app::KwinfoApp;
use crate::dialogs::{AlertDialog, Outcome};

impl KwinfoApp {
    /// Checks the alerts against the snapshot when it holds a new tick.
    pub(crate) fn check_alerts(&mut self, snapshot: &Snapshot) {
        if snapshot.tick == self.alert_tick {
            return;
        }
        self.alert_tick = snapshot.tick;
        let mut events = Vec::new();
        for entry in snapshot.channels() {
            let id = &entry.channel.id;
            if let Some(alert) = self.config.alert_of(id).copied() {
                events.extend(self.alerts.update(
                    id,
                    entry.stats.current(),
                    &alert,
                    entry.channel.decimals,
                ));
            }
        }
        let config = &self.config;
        events.extend(self.alerts.retain(|id| config.alert_of(id).is_some()));
        for event in events {
            self.announce(event, snapshot);
        }
    }

    fn announce(&mut self, event: AlertEvent, snapshot: &Snapshot) {
        match event {
            AlertEvent::Started {
                channel,
                breach,
                value,
                threshold,
            } => {
                let Some((device, entry)) = snapshot.devices.iter().find_map(|d| {
                    d.channels
                        .iter()
                        .find(|c| c.channel.id == channel)
                        .map(|c| (d, c))
                }) else {
                    return;
                };
                let c = &entry.channel;
                let temperature = self.config.general.temperature_unit;
                let show = |v: f64| format_value(Some(v), c.unit, c.decimals, temperature);
                let word = match breach {
                    Breach::Above => "above",
                    Breach::Below => "below",
                };
                let summary = format!("{} {word} {}", self.config.label_of(c), show(threshold));
                let body = format!("Now {}. {}", show(value), device.device.label);
                action!("alert.started", channel = %channel, value, threshold);
                self.flash(format!("Alert: {summary}"));
                if self.config.alert_of(&channel).is_some_and(|a| a.notify) {
                    notify::send(&summary, &body);
                }
            }
            AlertEvent::Ended { channel } => action!("alert.ended", channel = %channel),
        }
    }

    /// Checks the alerts again at the next frame rather than the next tick, after a change.
    pub(crate) fn recheck_alerts(&mut self) {
        self.alert_tick = 0;
    }

    /// The sensors in alert now, for the table.
    pub(crate) fn alerting(&self) -> HashSet<ChannelId> {
        self.alerts.active().map(|(id, _)| id.clone()).collect()
    }

    /// Whether `id` is in alert now.
    pub fn in_alert(&self, id: &ChannelId) -> bool {
        self.alerts.is_active(id)
    }

    pub(crate) fn open_alert_dialog(&mut self, id: &ChannelId) {
        let Some(entry) = self.snapshot.find(id) else {
            return;
        };
        let c = &entry.channel;
        self.alert_dialog = Some(AlertDialog::new(
            id.clone(),
            self.config.label_of(c),
            (c.unit, c.decimals, self.config.general.temperature_unit),
            entry.stats.current(),
            self.config.alert_of(id),
        ));
    }

    pub(crate) fn alert_dialog_frame(&mut self, ctx: &egui::Context) {
        let tokens = self.tokens();
        let Some(dialog) = &mut self.alert_dialog else {
            return;
        };
        let Some(outcome) = dialog.show(ctx, tokens) else {
            return;
        };
        if let Outcome::Apply(alert) = outcome {
            action!(
                "sensor.alert",
                channel = %dialog.channel,
                above = ?alert.and_then(|a| a.above),
                below = ?alert.and_then(|a| a.below)
            );
            self.config
                .update_channel(&dialog.channel, |c| c.alert = alert);
            self.recheck_alerts();
        }
        self.alert_dialog = None;
    }

    pub(crate) fn alerts_tab(&mut self, ui: &mut Ui, snapshot: &Snapshot) {
        let tokens = self.tokens();
        let temperature = self.config.general.temperature_unit;
        struct Row {
            id: ChannelId,
            label: String,
            above: String,
            below: String,
            notify: bool,
            active: bool,
        }
        let mut rows: Vec<Row> = self
            .config
            .alerts()
            .map(|(id, alert)| {
                let channel = snapshot.find(id).map(|e| &e.channel);
                let show = |v: Option<f64>| match (v, channel) {
                    (Some(v), Some(c)) => format_value(Some(v), c.unit, c.decimals, temperature),
                    (Some(v), None) => format!("{v}"),
                    (None, _) => "–".to_string(),
                };
                Row {
                    id: id.clone(),
                    label: channel
                        .map_or_else(|| id.to_string(), |c| self.config.label_of(c).to_string()),
                    above: show(alert.above),
                    below: show(alert.below),
                    notify: alert.notify,
                    active: self.alerts.is_active(id),
                }
            })
            .collect();
        rows.sort_by(|a, b| a.label.cmp(&b.label));
        if rows.is_empty() {
            ui.label("No alerts yet.");
            ui.label(
                RichText::new(
                    "Right-click a sensor and choose \"Set alert…\" to be told when it goes \
                     above or below a limit.",
                )
                .color(tokens.text_secondary),
            );
            return;
        }
        egui::Grid::new("settings-alerts")
            .num_columns(6)
            .striped(true)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for heading in ["Sensor", "Above", "Below", "Notify", "", ""] {
                    ui.label(RichText::new(heading).strong());
                }
                ui.end_row();
                for row in rows {
                    let mut label = RichText::new(&row.label);
                    if row.active {
                        label = label.strong().color(tokens.highlight);
                    }
                    ui.label(label)
                        .on_hover_text(if row.active { "In alert now" } else { "" });
                    ui.label(&row.above);
                    ui.label(&row.below);
                    let mut notify = row.notify;
                    let check = ui.checkbox(&mut notify, "");
                    check.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::Role::CheckBox,
                            true,
                            notify,
                            format!("Notify for {}", row.label),
                        )
                    });
                    if check.changed() {
                        action!("sensor.alert_notify", channel = %row.id, notify);
                        self.config.update_channel(&row.id, |c| {
                            if let Some(alert) = &mut c.alert {
                                alert.notify = notify;
                            }
                        });
                    }
                    if ui.small_button("Edit…").clicked() {
                        self.open_alert_dialog(&row.id);
                    }
                    if ui.small_button("Remove").clicked() {
                        action!("sensor.alert", channel = %row.id, removed = true);
                        self.config.update_channel(&row.id, |c| c.alert = None);
                        self.recheck_alerts();
                    }
                    ui.end_row();
                }
            });
    }
}

mod notify {
    //! Desktop notifications: through the freedesktop notification service on Linux,
    //! on a thread of their own since the D-Bus call may take a moment.

    #[cfg(target_os = "linux")]
    pub fn send(summary: &str, body: &str) {
        let (summary, body) = (summary.to_string(), body.to_string());
        let _ = std::thread::Builder::new()
            .name("kwinfo-notify".into())
            .spawn(move || {
                let shown = notify_rust::Notification::new()
                    .appname("KWinfo")
                    .summary(&summary)
                    .body(&body)
                    .icon("dialog-warning")
                    .show();
                if let Err(err) = shown {
                    tracing::warn!("the alert notification could not be shown: {err}");
                }
            });
    }

    #[cfg(not(target_os = "linux"))]
    pub fn send(summary: &str, _body: &str) {
        tracing::info!("alert (no desktop notifications on this system): {summary}");
    }
}

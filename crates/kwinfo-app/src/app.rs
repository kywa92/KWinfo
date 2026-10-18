//! The main window: the Sensor Status table, the quick filter, the bottom toolbar, the
//! settings dialog, and the glue to the sampler and the configuration file.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Frame, Key, KeyboardShortcut, Margin, Modifiers, RichText, TextEdit, ViewportCommand};
use kwinfo_core::{
    BackendChoice, BackendSet, ChannelId, Config, ConfigFile, DeviceId, Sampler, SamplerConfig,
    Snapshot, ThemeMode,
};

use kwinfo_diag::action;

use crate::diagnostics::{self, ReportJob};
use crate::dialogs::{ColorDialog, Outcome, RenameDialog};
use crate::icons::{self, Icon};
use crate::persist::Persistence;
use crate::self_usage::SelfUsage;
use crate::settings::{SettingsState, SettingsTab};
use crate::system_theme::SystemTheme;
use crate::table::{self, TableAction, TableState, TableView};
use crate::theme::{self, Palette, ThemeTokens};

pub const TITLE: &str = "KWinfo – Sensor Status";
pub const APP_ID: &str = "io.github.kylanwalters.KWinfo";

/// The polling periods offered in the toolbar and the settings.
pub const POLLING_CHOICES_MS: [u64; 6] = [250, 500, 1000, 2000, 3000, 5000];
/// The window size until the user resizes it.
pub const DEFAULT_WINDOW_SIZE: [f32; 2] = [780.0, 720.0];
/// Opens the quick filter: Ctrl+F (Cmd+F on macOS).
pub const FILTER_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::F);
/// How long a confirmation such as "Copied 48.0 °C" stays in the toolbar.
const FLASH_FOR: Duration = Duration::from_secs(3);

/// How the application starts.
pub struct Startup {
    pub backend: BackendChoice,
    /// The settings in effect at start, usually loaded from `config_file`.
    pub config: Config,
    /// Where settings are saved; `None` keeps them in memory (tests, previews).
    pub config_file: Option<ConfigFile>,
    /// Problems to show the user, e.g. why the settings file could not be used.
    pub warnings: Vec<String>,
    /// Command line override of the polling period, for this run only.
    pub interval_ms: Option<u64>,
    /// Command line override of the theme, for this run only.
    pub theme: Option<ThemeMode>,
    /// Where "Save diagnostic report…" writes; `None` puts the report next to the log
    /// files, as `kwinfo-cli report` does.
    pub report_out: Option<PathBuf>,
}

impl Startup {
    /// Default settings, kept in memory only.
    pub fn in_memory(backend: BackendChoice) -> Self {
        Self {
            backend,
            config: Config::default(),
            config_file: None,
            warnings: Vec::new(),
            interval_ms: None,
            theme: None,
            report_out: None,
        }
    }
}

pub struct KwinfoApp {
    pub(crate) sampler: Sampler,
    /// Notes from the backends, shown when there is nothing else to show.
    notes: Vec<String>,
    /// Problems the user should know about (settings file); dismissable.
    warnings: Vec<String>,
    pub(crate) config: Config,
    pub(crate) persistence: Persistence,
    /// The theme in effect: the command line's choice until the user picks one.
    pub(crate) theme_mode: ThemeMode,
    /// The polling period in effect, likewise.
    pub(crate) interval_ms: u64,
    applied: Option<Palette>,
    system_theme: SystemTheme,
    pub(crate) table: TableState,
    pub(crate) settings: SettingsState,
    rename: Option<RenameDialog>,
    color: Option<ColorDialog>,
    sent_disabled: Option<(HashSet<ChannelId>, HashSet<DeviceId>)>,
    title: String,
    self_usage: SelfUsage,
    pub(crate) flash: Option<(String, Instant)>,
    snapshot: Arc<Snapshot>,
    report_out: Option<PathBuf>,
    report_job: Option<ReportJob>,
    /// The last diagnostic report's file, or why it could not be written.
    pub(crate) report_result: Option<Result<PathBuf, String>>,
}

impl KwinfoApp {
    pub fn new(cc: &eframe::CreationContext<'_>, startup: Startup) -> Self {
        let Startup {
            backend,
            config,
            config_file,
            warnings,
            interval_ms,
            theme,
            report_out,
        } = startup;
        let set = BackendSet::build(backend);
        for note in &set.notes {
            tracing::info!("{note}");
        }
        let interval_ms = interval_ms.unwrap_or(config.polling.interval_ms);
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
        let snapshot = sampler.snapshot();
        let mut app = Self {
            sampler,
            notes: set.notes,
            warnings,
            persistence: Persistence::new(config_file, &config),
            theme_mode: theme.unwrap_or(config.theme.mode),
            interval_ms,
            config,
            applied: None,
            system_theme,
            table: TableState::default(),
            settings: SettingsState::default(),
            rename: None,
            color: None,
            sent_disabled: None,
            title: TITLE.to_string(),
            self_usage: SelfUsage::new(),
            flash: None,
            snapshot,
            report_out,
            report_job: None,
            report_result: None,
        };
        // Before the sampler's first tick, so disabled sensors are never read.
        app.sync_disabled();
        app
    }

    /// The settings as they are now (saved shortly after every change).
    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut Config {
        &mut self.config
    }

    pub fn table_state(&self) -> &TableState {
        &self.table
    }

    pub fn table_state_mut(&mut self) -> &mut TableState {
        &mut self.table
    }

    /// The sampler's latest snapshot.
    pub fn latest_snapshot(&self) -> Arc<Snapshot> {
        self.sampler.snapshot()
    }

    /// Opens or closes the settings window.
    pub fn set_settings_open(&mut self, open: bool) {
        if open != self.settings.open {
            if open {
                action!("settings.open", tab = self.settings.tab.label());
            } else {
                action!("settings.close");
            }
        }
        self.settings.open = open;
    }

    /// Opens or closes the quick filter; `via` says how, for the log.
    pub(crate) fn set_filter_open(&mut self, open: bool, via: &str) {
        if open == self.table.filter_open {
            if open {
                self.table.focus_filter = true;
            }
            return;
        }
        if open {
            action!("filter.open", via);
            self.table.open_filter();
        } else {
            action!("filter.close", via, filter = %self.table.filter);
            self.table.close_filter();
        }
    }

    /// The diagnostic report being written, or the outcome of the last one.
    pub fn report_result(&self) -> Option<&Result<PathBuf, String>> {
        self.report_result.as_ref()
    }

    /// Whether a diagnostic report is being written.
    pub fn report_running(&self) -> bool {
        self.report_job.is_some()
    }

    /// Starts writing a diagnostic report whose inventory is the current snapshot.
    pub(crate) fn save_report(&mut self, ctx: &egui::Context) {
        if self.report_job.is_some() {
            return;
        }
        action!("diagnostics.report");
        let inventory = diagnostics::inventory_text(&self.snapshot, &self.config);
        self.report_result = None;
        self.report_job = Some(ReportJob::start(
            inventory,
            self.report_out.clone(),
            self.persistence.path().map(PathBuf::from),
            ctx.clone(),
        ));
    }

    fn poll_report(&mut self) {
        let Some(result) = self.report_job.as_ref().and_then(ReportJob::poll) else {
            return;
        };
        match &result {
            Ok(path) => action!("diagnostics.report_saved", path = %path.display()),
            Err(err) => tracing::warn!("the diagnostic report could not be written: {err}"),
        }
        self.report_job = None;
        self.report_result = Some(result);
    }

    pub fn set_settings_tab(&mut self, tab: SettingsTab) {
        self.settings.tab = tab;
    }

    /// The window title: hidden-value count, and on Linux KWinfo's own CPU use.
    pub fn window_title(&self) -> String {
        let mut title = TITLE.to_string();
        let hidden = table::hidden_count(&self.snapshot, &self.config);
        if hidden > 0 {
            title.push_str(&format!(" [{hidden} value(s) hidden]"));
        }
        if let Some(percent) = self.self_usage.percent() {
            title.push_str(&format!(" ({percent:.1}%)"));
        }
        title
    }

    pub(crate) fn tokens(&self) -> &'static ThemeTokens {
        ThemeTokens::for_palette(self.applied.unwrap_or(Palette::Dark))
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

    /// The user picked a theme: it applies now and is saved.
    pub(crate) fn set_theme_mode(&mut self, mode: ThemeMode) {
        action!("settings.theme", mode = ?mode);
        self.theme_mode = mode;
        self.config.theme.mode = mode;
    }

    /// The user picked a polling period: it applies now and is saved.
    pub(crate) fn set_interval(&mut self, ms: u64) {
        action!("settings.polling", interval_ms = ms);
        self.interval_ms = ms;
        self.config.polling.interval_ms = ms;
        self.sampler.set_interval(Duration::from_millis(ms));
    }

    /// Shows `text` in the toolbar for a few seconds.
    pub(crate) fn flash(&mut self, text: impl Into<String>) {
        self.flash = Some((text.into(), Instant::now()));
    }

    /// Saves and closes the window.
    pub(crate) fn quit(&mut self, ctx: &egui::Context) {
        action!("app.quit");
        self.persistence.flush(&self.config);
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    /// Tells the sampler which sensors not to poll, when that changed.
    fn sync_disabled(&mut self) {
        let wanted = (
            self.config.disabled_channels(),
            self.config.disabled_devices(),
        );
        if self.sent_disabled.as_ref() != Some(&wanted) {
            self.sampler
                .set_disabled(wanted.0.clone(), wanted.1.clone());
            self.sent_disabled = Some(wanted);
        }
    }

    fn remember_window_size(&mut self, ctx: &egui::Context) {
        let (rect, maximized, fullscreen) = ctx.input(|i| {
            let viewport = i.viewport();
            (viewport.inner_rect, viewport.maximized, viewport.fullscreen)
        });
        if maximized == Some(true) || fullscreen == Some(true) {
            return;
        }
        if let Some(rect) = rect.filter(|r| r.is_positive()) {
            let size = rect.size().round();
            if self.config.window.size() != Some([size.x, size.y]) {
                self.config.window.width = Some(size.x);
                self.config.window.height = Some(size.y);
            }
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let dialog_open = self.rename.is_some() || self.color.is_some();
        if !dialog_open && ctx.input_mut(|i| i.consume_shortcut(&FILTER_SHORTCUT)) {
            self.set_filter_open(true, "shortcut");
        }
        if self.table.filter_open && !dialog_open && ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.set_filter_open(false, "escape");
        }
    }

    fn apply(&mut self, action: TableAction, snapshot: &Snapshot) {
        match action {
            TableAction::ShowGraph(id) => {
                let label = snapshot
                    .find(&id)
                    .map_or(id.as_str(), |c| self.config.label_of(&c.channel))
                    .to_string();
                action!("sensor.show_graph", channel = %id, note = "graph windows arrive in M3");
                self.flash(format!("Graph of {label}: arrives in M3"));
            }
            TableAction::Rename(id) => {
                if let Some(entry) = snapshot.find(&id) {
                    let current = self.config.label_of(&entry.channel).to_string();
                    self.rename = Some(RenameDialog::new(id, &entry.channel.label, &current));
                }
            }
            TableAction::PickColor(id) => {
                if let Some(entry) = snapshot.find(&id) {
                    let tokens = self.tokens();
                    let start = self.config.color_of(&id).map_or_else(
                        || tokens.icon_color(entry.channel.kind),
                        |rgb| egui::Color32::from_rgb(rgb.r, rgb.g, rgb.b),
                    );
                    let label = self.config.label_of(&entry.channel).to_string();
                    self.color = Some(ColorDialog::new(id, &label, start));
                }
            }
            TableAction::Reset(ids) => {
                action!("sensor.reset", count = ids.len());
                self.sampler.reset_channels(ids);
            }
            TableAction::Copied(text) => self.flash(format!("Copied {text}")),
            TableAction::ResetWidths => {}
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let tokens = self.tokens();
        if let Some(dialog) = &mut self.rename
            && let Some(outcome) = dialog.show(ctx, tokens)
        {
            if let Outcome::Apply(label) = outcome {
                action!("sensor.rename", channel = %dialog.channel, label = ?label);
                self.config
                    .update_channel(&dialog.channel, |c| c.label = label);
            }
            self.rename = None;
        }
        if let Some(dialog) = &mut self.color
            && let Some(outcome) = dialog.show(ctx, tokens)
        {
            if let Outcome::Apply(color) = outcome {
                let shown = color.map_or_else(|| "default".to_string(), |c| c.to_string());
                action!("sensor.colour", channel = %dialog.channel, colour = %shown);
                self.config
                    .update_channel(&dialog.channel, |c| c.color = color);
            }
            self.color = None;
        }
    }

    fn filter_bar(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        let tokens = self.tokens();
        ui.horizontal(|ui| {
            icons::icon(ui, Icon::Search, tokens.text_secondary);
            let label = ui.label("Filter");
            let edit = ui
                .add(
                    TextEdit::singleline(&mut self.table.filter)
                        .hint_text("Sensor, group or device name")
                        .desired_width(260.0),
                )
                .labelled_by(label.id);
            if std::mem::take(&mut self.table.focus_filter) {
                edit.request_focus();
            }
            if !self.table.active_filter().is_empty() {
                let shown = table::layout(snapshot, &self.config, self.table.active_filter()).shown;
                let text = match shown {
                    0 => "No sensor matches".to_string(),
                    1 => "1 sensor".to_string(),
                    n => format!("{n} sensors"),
                };
                ui.label(RichText::new(text).color(tokens.text_secondary));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::button(ui, Icon::Close, None, "Close filter")
                    .on_hover_text("Close the filter (Esc)")
                    .clicked()
                {
                    self.set_filter_open(false, "close button");
                }
            });
        });
    }

    fn warnings_bar(&mut self, ui: &mut egui::Ui) {
        let tokens = self.tokens();
        let mut dismissed = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Note:").strong().color(tokens.text_primary));
            ui.vertical(|ui| {
                for warning in &self.warnings {
                    ui.label(warning);
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                dismissed = ui.button("Dismiss").clicked();
            });
        });
        if dismissed {
            self.warnings.clear();
        }
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
        self.remember_window_size(ctx);
        if ctx.input(|i| i.viewport().close_requested()) {
            self.persistence.flush(&self.config);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let tokens = self.tokens();
        let snapshot = self.sampler.snapshot();
        self.snapshot = Arc::clone(&snapshot);
        self.shortcuts(&ctx);
        self.poll_report();

        egui::Panel::bottom("toolbar")
            .frame(
                Frame::new()
                    .fill(tokens.window_bg)
                    .inner_margin(Margin::symmetric(8, 6)),
            )
            .show(ui, |ui| self.toolbar(ui, &snapshot));
        if let Some(error) = self.persistence.error() {
            let note = format!("The settings could not be saved: {error}");
            if !self.warnings.contains(&note) {
                self.warnings.push(note);
            }
        }
        if !self.warnings.is_empty() {
            egui::Panel::top("warnings")
                .frame(
                    Frame::new()
                        .fill(tokens.group_row)
                        .inner_margin(Margin::symmetric(8, 6)),
                )
                .show(ui, |ui| self.warnings_bar(ui));
        }
        if self.table.filter_open {
            egui::Panel::top("filter")
                .frame(
                    Frame::new()
                        .fill(tokens.window_bg)
                        .inner_margin(Margin::symmetric(8, 5)),
                )
                .show(ui, |ui| self.filter_bar(ui, &snapshot));
        }
        let mut actions = Vec::new();
        egui::CentralPanel::default()
            .frame(Frame::new().fill(tokens.table_bg))
            .show(ui, |ui| {
                if snapshot.devices.is_empty() {
                    self.empty_state(ui);
                } else {
                    let view = TableView {
                        snapshot: &snapshot,
                        config: &mut self.config,
                        tokens,
                    };
                    actions = table::show(ui, view, &mut self.table);
                }
            });
        self.settings_window(&ctx, &snapshot);
        self.dialogs(&ctx);
        for action in actions {
            self.apply(action, &snapshot);
        }
        self.sync_disabled();

        self.self_usage.update(Instant::now());
        let title = self.window_title();
        if title != self.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        if let Some(wait) = self.persistence.update(&self.config, Instant::now()) {
            ctx.request_repaint_after(wait);
        }
        if let Some((_, since)) = &self.flash {
            match FLASH_FOR.checked_sub(since.elapsed()) {
                Some(left) => ctx.request_repaint_after(left),
                None => self.flash = None,
            }
        }
    }
}

impl Drop for KwinfoApp {
    /// Last chance to save changes made just before quitting.
    fn drop(&mut self) {
        if self.persistence.is_dirty(&self.config) {
            self.persistence.flush(&self.config);
        }
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

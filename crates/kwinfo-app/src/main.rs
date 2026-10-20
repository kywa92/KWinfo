//! Entry point of the `kwinfo` binary.

mod args;

use args::{Args, ThemeArg};
use clap::Parser as _;
use kwinfo_app::app::{self, KwinfoApp, Startup};
use kwinfo_core::{BackendChoice, ConfigFile, ThemeMode};

impl From<ThemeArg> for ThemeMode {
    fn from(arg: ThemeArg) -> Self {
        match arg {
            ThemeArg::Auto => ThemeMode::Auto,
            ThemeArg::Dark => ThemeMode::Dark,
            ThemeArg::Light => ThemeMode::Light,
        }
    }
}

fn main() -> eframe::Result {
    // Stderr plus a per-run log file; see the kwinfo-diag crate. Kept alive until exit.
    let _diag = kwinfo_diag::init(kwinfo_diag::DiagConfig::new(
        "kwinfo",
        env!("CARGO_PKG_VERSION"),
    ));

    let args = Args::parse();
    let backend = match args.replay {
        Some(dir) => BackendChoice::Replay(dir),
        None if args.mock || args.backend.as_deref() == Some("mock") => BackendChoice::Mock,
        None => BackendChoice::Auto,
    };
    let config_file = args
        .config
        .map(ConfigFile::new)
        .or_else(ConfigFile::default_location);
    let (config, warnings) = match &config_file {
        Some(file) => {
            tracing::info!("settings: {}", file.path().display());
            let (config, note) = file.load_or_default();
            (config, note.into_iter().collect())
        }
        None => (
            kwinfo_core::Config::default(),
            vec!["No home directory, so the settings will not be saved.".to_string()],
        ),
    };
    let size = config.window.size().unwrap_or(app::DEFAULT_WINDOW_SIZE);
    let startup = Startup {
        backend,
        config,
        config_file,
        warnings,
        interval_ms: args.interval,
        theme: args.theme.map(ThemeMode::from),
        report_out: None,
        helper: None,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(app::TITLE)
            .with_app_id(app::APP_ID)
            .with_inner_size(size)
            .with_min_inner_size([520.0, 360.0]),
        ..Default::default()
    };
    eframe::run_native(
        app::TITLE,
        options,
        Box::new(move |cc| Ok(Box::new(KwinfoApp::new(cc, startup)))),
    )
}

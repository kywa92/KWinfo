//! Entry point of the `kwinfo` binary.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use kwinfo_app::app::{self, KwinfoApp, Startup};
use kwinfo_core::{BackendChoice, ConfigFile, ThemeMode};

#[derive(Parser, Debug)]
#[command(
    name = "kwinfo",
    version,
    about = "Hardware monitor in the spirit of HWiNFO64"
)]
struct Args {
    /// Use the synthetic mock backend instead of real hardware
    #[arg(long, conflicts_with = "replay")]
    mock: bool,
    /// Run the real backends against a captured tree instead of / (the output of
    /// `kwinfo-cli snapshot`, or a test fixture)
    #[arg(long, value_name = "DIR")]
    replay: Option<PathBuf>,
    /// Backend selection through the environment (`KWINFO_BACKEND=mock`)
    #[arg(long, env = "KWINFO_BACKEND", hide = true)]
    backend: Option<String>,
    /// Polling interval in milliseconds for this run (default: the saved setting)
    #[arg(long, value_name = "MS")]
    interval: Option<u64>,
    /// Theme for this run (default: the saved setting)
    #[arg(long, value_enum)]
    theme: Option<ThemeArg>,
    /// Settings file to use instead of ~/.config/kwinfo/config.toml
    #[arg(long, value_name = "FILE", env = "KWINFO_CONFIG")]
    config: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ThemeArg {
    Auto,
    Dark,
    Light,
}

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

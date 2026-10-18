//! Entry point of the `kwinfo` binary.

use clap::{Parser, ValueEnum};
use kwinfo_app::app::{self, KwinfoApp};
use kwinfo_app::theme::ThemeMode;
use kwinfo_core::BackendChoice;

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
    replay: Option<std::path::PathBuf>,
    /// Backend selection through the environment (`KWINFO_BACKEND=mock`)
    #[arg(long, env = "KWINFO_BACKEND", hide = true)]
    backend: Option<String>,
    /// Polling interval in milliseconds
    #[arg(long, default_value_t = 1000, value_name = "MS")]
    interval: u64,
    /// Theme to start with
    #[arg(long, value_enum, default_value_t = ThemeArg::Auto)]
    theme: ThemeArg,
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
    let choice = match args.replay {
        Some(dir) => BackendChoice::Replay(dir),
        None if args.mock || args.backend.as_deref() == Some("mock") => BackendChoice::Mock,
        None => BackendChoice::Auto,
    };
    let theme_mode = ThemeMode::from(args.theme);
    let interval = args.interval;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(app::TITLE)
            .with_app_id(app::APP_ID)
            .with_inner_size([780.0, 720.0])
            .with_min_inner_size([520.0, 360.0]),
        ..Default::default()
    };
    eframe::run_native(
        app::TITLE,
        options,
        Box::new(move |cc| Ok(Box::new(KwinfoApp::new(cc, choice, interval, theme_mode)))),
    )
}

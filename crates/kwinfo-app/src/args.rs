//! The `kwinfo` command line. It is kept apart from `main.rs` so that `cargo xtask man`
//! can render it into the kwinfo(1) manual page; it may use only `clap` and `std`.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "kwinfo",
    version,
    about = "Hardware monitor in the spirit of HWiNFO64"
)]
pub struct Args {
    /// Use the synthetic mock backend instead of real hardware
    #[arg(long, conflicts_with = "replay")]
    pub mock: bool,
    /// Run the real backends against a captured tree instead of / (the output of
    /// `kwinfo-cli snapshot`, or a test fixture)
    #[arg(long, value_name = "DIR")]
    pub replay: Option<PathBuf>,
    /// Backend selection through the environment (`KWINFO_BACKEND=mock`)
    #[arg(long, env = "KWINFO_BACKEND", hide = true)]
    pub backend: Option<String>,
    /// Polling interval in milliseconds for this run (default: the saved setting)
    #[arg(long, value_name = "MS")]
    pub interval: Option<u64>,
    /// Theme for this run (default: the saved setting)
    #[arg(long, value_enum)]
    pub theme: Option<ThemeArg>,
    /// Settings file to use instead of ~/.config/kwinfo/config.toml
    #[arg(long, value_name = "FILE", env = "KWINFO_CONFIG")]
    pub config: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ThemeArg {
    Auto,
    Dark,
    Light,
}

//! The `kwinfo-helper` command line. It is kept apart from `main.rs` so that
//! `cargo xtask man` can render it into the kwinfo-helper(8) manual page; it may use only
//! `clap` and `std`.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "kwinfo-helper",
    version,
    about = "Privileged sensor helper for KWinfo: RAPL power, MSRs, S.M.A.R.T. and GPU load"
)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Serve the D-Bus system service (D-Bus activation starts this as root)
    Serve,
    /// Answer requests on standard input and output, one JSON message per line (KWinfo
    /// starts this through pkexec when no service is installed)
    Stdio(SourceArgs),
    /// Show what the helper can read on this machine; run it as root
    Probe(SourceArgs),
    /// Install the helper, its systemd unit, D-Bus files and polkit action (as root)
    Install {
        /// Install into this staging directory instead of / (packaging, tests)
        #[arg(long, hide = true, value_name = "DIR")]
        root: Option<PathBuf>,
        /// The helper's path as the installed files name it
        #[arg(long, hide = true, value_name = "PATH")]
        helper_path: Option<String>,
        /// Do not copy the helper itself (it is already in place)
        #[arg(long, hide = true)]
        no_copy: bool,
    },
    /// Remove what install added (as root)
    Uninstall {
        /// Uninstall from this staging directory instead of /
        #[arg(long, hide = true, value_name = "DIR")]
        root: Option<PathBuf>,
    },
}

#[derive(clap::Args, Debug)]
pub struct SourceArgs {
    /// Read a captured tree instead of / (tests and demonstrations)
    #[arg(long, hide = true, value_name = "DIR")]
    pub root: Option<PathBuf>,
    /// Use captured smartctl output from this directory instead of running smartctl
    #[arg(long, hide = true, value_name = "DIR")]
    pub smartctl_dir: Option<PathBuf>,
}

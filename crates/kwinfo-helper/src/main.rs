//! The privileged helper. Milestone M4 implements the D-Bus service, the stdio mode and
//! the installer; M0 only reserves the command line so packaging can refer to it.

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "kwinfo-helper",
    version,
    about = "Privileged sensor helper for KWinfo"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run as the D-Bus system service (systemd activates this)
    Serve,
    /// Speak newline-delimited JSON on stdin/stdout (used through pkexec by the AppImage)
    Stdio,
    /// Install the systemd unit, D-Bus policy and polkit action system-wide
    Install,
    /// Remove what `install` added
    Uninstall,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let what = match args.command {
        Command::Serve => "serve",
        Command::Stdio => "stdio",
        Command::Install => "install",
        Command::Uninstall => "uninstall",
    };
    eprintln!(
        "kwinfo-helper {what}: not implemented yet (planned for milestone M4; protocol v{})",
        kwinfo_ipc::PROTOCOL_VERSION
    );
    ExitCode::from(2)
}

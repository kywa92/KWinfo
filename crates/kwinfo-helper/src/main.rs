//! `kwinfo-helper`: KWinfo's privileged sensor helper. See the library documentation and
//! docs/helper.md.

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::bail;
use clap::{Parser, Subcommand};
use kwinfo_core::{SysRoot, TemperatureUnit, format_value};
use kwinfo_helper::install::{self, Layout};
use kwinfo_helper::providers::Sources;
use kwinfo_helper::{Helper, stdio};
use kwinfo_ipc::{LOCAL_HELPER_PATH, PROTOCOL_VERSION, Request, Response};

#[derive(Parser, Debug)]
#[command(
    name = "kwinfo-helper",
    version,
    about = "Privileged sensor helper for KWinfo: RAPL power, MSRs, S.M.A.R.T. and GPU load"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
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
struct SourceArgs {
    /// Read a captured tree instead of / (tests and demonstrations)
    #[arg(long, hide = true, value_name = "DIR")]
    root: Option<PathBuf>,
    /// Use captured smartctl output from this directory instead of running smartctl
    #[arg(long, hide = true, value_name = "DIR")]
    smartctl_dir: Option<PathBuf>,
}

impl SourceArgs {
    fn sources(&self) -> Sources {
        Sources {
            root: self.root.as_ref().map_or_else(SysRoot::host, SysRoot::new),
            smartctl_dir: self.smartctl_dir.clone(),
        }
    }
}

fn main() -> ExitCode {
    let args = Args::parse();
    // Logs go to standard error only: the journal for the service, KWinfo's own log for
    // a session helper. A file under root's home would help nobody.
    let quiet = !matches!(args.command, Command::Serve | Command::Stdio(_));
    let _diag = kwinfo_diag::init(
        kwinfo_diag::DiagConfig::new("kwinfo-helper", env!("CARGO_PKG_VERSION"))
            .stderr_default(if quiet { "warn" } else { "info" })
            .without_file(),
    );
    match run(args.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("kwinfo-helper: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command) -> anyhow::Result<()> {
    match command {
        Command::Serve => serve(),
        Command::Stdio(args) => {
            let mut helper = Helper::with_sources(&args.sources());
            stdio::serve(&mut helper, io::stdin().lock(), io::stdout().lock())?;
            Ok(())
        }
        Command::Probe(args) => probe(&args),
        Command::Install {
            root,
            helper_path,
            no_copy,
        } => install_command(root, helper_path, no_copy),
        Command::Uninstall { root } => uninstall_command(root),
    }
}

#[cfg(unix)]
fn serve() -> anyhow::Result<()> {
    kwinfo_helper::dbus::serve(Helper::with_sources(&Sources::host()))
}

#[cfg(not(unix))]
fn serve() -> anyhow::Result<()> {
    bail!("the D-Bus service only runs on Linux")
}

/// Prints the capabilities and two rounds of readings, one second apart.
fn probe(args: &SourceArgs) -> anyhow::Result<()> {
    let live = args.root.is_none();
    if live && !install::is_root() {
        eprintln!(
            "note: not running as root, so most sources will be missing (sudo kwinfo-helper probe)"
        );
    }
    let mut helper = Helper::with_sources(&args.sources());
    if let Response::Error { message } = helper.handle(&Request::Hello {
        protocol: PROTOCOL_VERSION,
    }) {
        bail!(message);
    }
    let Response::Capabilities(caps) = helper.handle(&Request::Capabilities) else {
        bail!("no capabilities");
    };
    println!(
        "KWinfo helper {} (protocol {PROTOCOL_VERSION})",
        env!("CARGO_PKG_VERSION")
    );
    println!("Reads: {}", caps.summary());
    for note in &caps.notes {
        println!("Note: {note}");
    }
    let Response::Enumerate { devices } = helper.handle(&Request::Enumerate) else {
        bail!("no devices");
    };
    helper.handle(&Request::Sample);
    std::thread::sleep(Duration::from_secs(1));
    let Response::Samples { samples } = helper.handle(&Request::Sample) else {
        bail!("no readings");
    };
    for entry in &devices {
        println!();
        println!("{}  [{}]", entry.device.label, entry.device.id);
        for channel in &entry.channels {
            let value = samples
                .iter()
                .find(|s| s.channel == channel.id)
                .and_then(|s| s.value);
            println!(
                "  {:<34} {:>16}  {}",
                channel.label,
                format_value(
                    value,
                    channel.unit,
                    channel.decimals,
                    TemperatureUnit::Celsius
                ),
                channel.id.key()
            );
        }
    }
    let count: usize = devices.iter().map(|d| d.channels.len()).sum();
    println!();
    println!("{} devices, {count} channels", devices.len());
    Ok(())
}

fn install_command(
    root: Option<PathBuf>,
    helper_path: Option<String>,
    no_copy: bool,
) -> anyhow::Result<()> {
    let live = root.is_none();
    if live && !install::is_root() {
        bail!(
            "installing needs root: pkexec {} install",
            std::env::current_exe()?.display()
        );
    }
    let layout = Layout::new(root.unwrap_or_else(|| PathBuf::from("/")))
        .with_helper_path(helper_path.unwrap_or_else(|| LOCAL_HELPER_PATH.to_string()));
    let binary = if no_copy {
        None
    } else {
        Some(std::env::current_exe()?)
    };
    for path in install::install(&layout, binary.as_deref())? {
        println!("installed {path}");
    }
    if live {
        for note in install::activate() {
            println!("note: {note}");
        }
    }
    println!("The helper is installed; KWinfo uses it from now on.");
    Ok(())
}

fn uninstall_command(root: Option<PathBuf>) -> anyhow::Result<()> {
    let live = root.is_none();
    if live && !install::is_root() {
        bail!(
            "uninstalling needs root: pkexec {} uninstall",
            std::env::current_exe()?.display()
        );
    }
    let layout = Layout::new(root.unwrap_or_else(|| PathBuf::from("/")));
    let mut notes = if live {
        install::deactivate()
    } else {
        Vec::new()
    };
    for path in install::uninstall(&layout)? {
        println!("removed {path}");
    }
    if live {
        notes.extend(install::reload());
    }
    for note in notes {
        println!("note: {note}");
    }
    println!("The helper is uninstalled.");
    Ok(())
}

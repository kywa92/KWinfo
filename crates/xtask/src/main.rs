//! `cargo xtask`: KWinfo's development tasks (the alias is in `.cargo/config.toml`).
//!
//! `cargo xtask man` writes the manual pages in `docs/man/`. Their synopses, options and
//! commands come from the binaries' own clap definitions (each binary's `args.rs`,
//! compiled into this program as a plain module); the prose comes from [`pages`].
//! `cargo xtask man --check` writes nothing and fails when a committed page differs from
//! what would be written, so CI notices an option that changed without its page.

#[allow(dead_code)]
#[path = "../../kwinfo-app/src/args.rs"]
mod kwinfo_args;
#[allow(dead_code)]
#[path = "../../kwinfo-cli/src/args.rs"]
mod kwinfo_cli_args;
#[allow(dead_code)]
#[path = "../../kwinfo-helper/src/args.rs"]
mod kwinfo_helper_args;

use std::fs;
use std::io;
use std::path::Path;
use std::process::ExitCode;

use anyhow::Context as _;
use clap::{CommandFactory as _, Parser, Subcommand};
use clap_mangen::Man;
use clap_mangen::roff::{Inline, Roff, bold, italic, roman};

#[derive(Parser, Debug)]
#[command(name = "cargo xtask", about = "KWinfo's development tasks")]
struct Args {
    #[command(subcommand)]
    task: Task,
}

#[derive(Subcommand, Debug)]
enum Task {
    /// Write the manual pages in docs/man/ from the binaries' command lines
    Man {
        /// Write nothing; fail if a committed page is out of date
        #[arg(long)]
        check: bool,
    },
}

fn main() -> ExitCode {
    let result = match Args::parse().task {
        Task::Man { check } => man(check),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("xtask: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// Writes (or with `check`, compares) every page; `Ok(false)` when a page is stale.
fn man(check: bool) -> anyhow::Result<bool> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/man");
    let mut fresh = true;
    for page in pages() {
        let name = page.file_name();
        let path = dir.join(&name);
        let text = page.render().with_context(|| format!("rendering {name}"))?;
        let current = fs::read_to_string(&path).ok();
        if current.as_deref() == Some(text.as_str()) {
            continue;
        }
        if check {
            eprintln!("docs/man/{name} is out of date; run `cargo xtask man` and commit it");
            fresh = false;
        } else {
            fs::create_dir_all(&dir)?;
            fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
            println!("wrote docs/man/{name}");
        }
    }
    Ok(fresh)
}

/// One manual page: a binary's command line plus the prose around it.
struct Page {
    command: clap::Command,
    section: &'static str,
    /// The paragraphs of DESCRIPTION.
    description: &'static [&'static str],
    /// Sections after OPTIONS and COMMANDS, in order.
    sections: Vec<(&'static str, Vec<Block>)>,
}

/// Prose. `**bold**` and `` `bold` `` mark commands and options, `*italic*` file names
/// and other text to fill in, as man-pages(7) asks.
enum Block {
    Para(&'static str),
    /// A tagged paragraph: the tag, then its text.
    Item(&'static str, &'static str),
}

use Block::{Item, Para};

/// What `Roff::render` puts in front of every rendering; a page needs it once.
const PREAMBLE: &str = ".ie \\n(.g .ds Aq \\(aq\n.el .ds Aq '\n";

impl Page {
    fn name(&self) -> &str {
        self.command.get_name()
    }

    fn file_name(&self) -> String {
        format!("{}.{}", self.name(), self.section)
    }

    fn render(&self) -> io::Result<String> {
        let mut command = self.command.clone();
        if command.has_subcommands() {
            command = command
                .subcommand_value_name("command")
                .disable_help_subcommand(true);
        }
        let man = Man::new(command.clone()).section(self.section);

        // The title line is written here: clap_mangen drops an empty date, which shifts
        // the source into the date's place. No date, so the pages only change with
        // their content or the version.
        let mut title = Roff::new();
        title.control(
            "TH",
            [
                self.name().to_uppercase().as_str(),
                self.section,
                "\"\"",
                &format!("KWinfo {}", env!("CARGO_PKG_VERSION")),
            ],
        );
        let mut out = String::from(PREAMBLE);
        out += &strip(title.render());
        out += &part(|w| man.render_name_section(w))?;
        out += &part(|w| man.render_synopsis_section(w))?;
        out += &section("DESCRIPTION", &paras(self.description));
        out += &part(|w| man.render_options_section(w))?;

        let subcommands: Vec<_> = command
            .get_subcommands()
            .filter(|s| !s.is_hide_set())
            .cloned()
            .collect();
        if !subcommands.is_empty() {
            out += &section("COMMANDS", &[]);
            for sub in subcommands {
                out += &self.render_subcommand(sub)?;
            }
        }
        for (title, blocks) in &self.sections {
            out += &section(title, blocks);
        }
        Ok(tidy(&out))
    }

    /// A subsection per command: its synopsis, what it does and its options.
    fn render_subcommand(&self, sub: clap::Command) -> io::Result<String> {
        let name = sub.get_name().to_string();
        let about = sub
            .get_long_about()
            .or_else(|| sub.get_about())
            .map(|text| text.to_string());
        let has_options = sub.get_arguments().any(|a| !a.is_hide_set());
        let man = Man::new(
            sub.disable_help_flag(true)
                .bin_name(format!("{} {name}", self.name())),
        );

        let mut roff = Roff::new();
        roff.control("SS", [name.as_str()]);
        let mut out = strip(roff.render());
        out += &without_heading(&part(|w| man.render_synopsis_section(w))?);
        if let Some(about) = about {
            let mut roff = Roff::new();
            roff.control("PP", []);
            roff.text(inlines(&about));
            out += &strip(roff.render());
        }
        if has_options {
            out += &without_heading(&part(|w| man.render_options_section(w))?);
        }
        Ok(out)
    }
}

/// What one of clap_mangen's renderers writes, without the preamble.
fn part(render: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> io::Result<String> {
    let mut buf = Vec::new();
    render(&mut buf)?;
    String::from_utf8(buf).map(strip).map_err(io::Error::other)
}

fn strip(text: String) -> String {
    match text.strip_prefix(PREAMBLE) {
        Some(rest) => rest.to_string(),
        None => text,
    }
}

/// Drops the `.SH` line a renderer starts with, for use under a `.SS`.
fn without_heading(text: &str) -> String {
    match text.split_once('\n') {
        Some((first, rest)) if first.starts_with(".SH ") => rest.to_string(),
        _ => text.to_string(),
    }
}

fn section(title: &str, blocks: &[Block]) -> String {
    let mut roff = Roff::new();
    roff.control("SH", [title]);
    for (i, block) in blocks.iter().enumerate() {
        match block {
            Para(text) => {
                // A paragraph straight after the heading needs no .PP.
                if i > 0 {
                    roff.control("PP", []);
                }
                roff.text(inlines(text));
            }
            Item(tag, text) => {
                roff.control("TP", []);
                roff.text(inlines(tag));
                roff.text(inlines(text));
            }
        }
    }
    strip(roff.render())
}

fn paras(texts: &[&'static str]) -> Vec<Block> {
    texts.iter().map(|text| Para(text)).collect()
}

/// `**bold**`, `` `bold` `` and `*italic*` into roff inlines.
fn inlines(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    for (i, part) in text.split("**").enumerate() {
        if i % 2 == 1 {
            out.push(bold(part));
            continue;
        }
        for (j, piece) in part.split('`').enumerate() {
            if j % 2 == 1 {
                out.push(bold(piece));
                continue;
            }
            for (k, word) in piece.split('*').enumerate() {
                if !word.is_empty() {
                    out.push(if k % 2 == 1 {
                        italic(word)
                    } else {
                        roman(word)
                    });
                }
            }
        }
    }
    out
}

/// Final touches: clap's help texts quote commands in backticks, which become bold, and
/// clap_mangen leaves spaces at the ends of synopsis lines.
fn tidy(roff: &str) -> String {
    let mut out = String::with_capacity(roff.len());
    for line in roff.lines().map(str::trim_end) {
        let pieces: Vec<&str> = line.split('`').collect();
        if line.starts_with('.') || pieces.len() < 3 || pieces.len().is_multiple_of(2) {
            out.push_str(line);
        } else {
            for (i, piece) in pieces.iter().enumerate() {
                if i % 2 == 1 {
                    out.push_str("\\fB");
                    out.push_str(piece);
                    out.push_str("\\fR");
                } else {
                    out.push_str(piece);
                }
            }
        }
        out.push('\n');
    }
    out
}

/// Environment variables every binary reads through kwinfo-diag (docs/diagnostics.md).
fn logging_environment(stderr_default: &'static str) -> Vec<Block> {
    vec![
        Item("**KWINFO_LOG**", stderr_default),
        Item(
            "**KWINFO_FILE_LOG**",
            "What goes to the log file (default: debug for KWinfo, info for the libraries).",
        ),
        Item("**KWINFO_LOG_DIR**", "Directory for the log files."),
        Item(
            "**KWINFO_NO_FILE_LOG**",
            "Anything but 0 turns the log file off.",
        ),
    ]
}

fn pages() -> Vec<Page> {
    let mut kwinfo_environment = vec![
        Item(
            "**KWINFO_CONFIG**",
            "The settings file, as with **--config**.",
        ),
        Item(
            "**KWINFO_BACKEND**",
            "*mock* selects the synthetic sensors, as **--mock** does.",
        ),
    ];
    kwinfo_environment.extend(logging_environment(
        "What goes to standard error, in the filter syntax of the Rust tracing library \
         (default: info).",
    ));
    let mut cli_environment = vec![Item(
        "**KWINFO_BACKEND**",
        "*mock* makes **list** use the synthetic sensors, as **--mock** does.",
    )];
    cli_environment.extend(logging_environment(
        "What goes to standard error, in the filter syntax of the Rust tracing library \
         (default: warn).",
    ));

    vec![
        Page {
            command: kwinfo_args::Args::command(),
            section: "1",
            description: &[
                "**kwinfo** shows the hardware sensors of a Linux machine in one window \
                 modelled on the Sensor Status window of HWiNFO64: temperatures, voltages, \
                 currents, fan speeds, clocks, power, usage, drive and network rates and \
                 S.M.A.R.T. data, grouped by device, each with its current value and the \
                 minimum, maximum and average since the last reset.",
                "Double-click a sensor for a live graph. Right-click it to rename, hide, \
                 recolour or stop monitoring it, to set an alert, or to show its value in \
                 the system tray. The toolbar logs sensors to CSV files. Every setting is \
                 kept between runs.",
                "Most sensors come from the kernel's hwmon drivers through */sys*, with \
                 the labels and compute rules of the lm-sensors configuration. Motherboard \
                 sensors need the kernel module for the board's Super I/O chip; \
                 **sensors-detect**(8) finds it, and the installation guide lists the usual \
                 ones. Sensors that only root may read (CPU power from RAPL, core voltages, \
                 package C-states, S.M.A.R.T. data, GPU engine load) come from \
                 **kwinfo-helper**(8), a small system service that KWinfo starts through \
                 D-Bus.",
            ],
            sections: vec![
                ("ENVIRONMENT", kwinfo_environment),
                (
                    "FILES",
                    vec![
                        Item(
                            "*~/.config/kwinfo/config.toml*",
                            "The settings: theme, columns, polling period, graphs, and \
                             per-sensor names, colours, hidden and disabled state, alerts \
                             and tray icons. *$XDG_CONFIG_HOME* replaces *~/.config* when \
                             set.",
                        ),
                        Item(
                            "*~/.local/share/kwinfo/logs/*",
                            "The default folder for CSV logs.",
                        ),
                        Item(
                            "*~/.local/state/kwinfo/logs/*",
                            "A diagnostic log per run; **kwinfo-cli report** bundles the \
                             newest ones for a bug report.",
                        ),
                        Item(
                            "*/usr/share/doc/kwinfo/*",
                            "The installation guide (kernel modules, sensors-detect), the \
                             helper and the diagnostics guides, when installed from the \
                             Debian package.",
                        ),
                    ],
                ),
                (
                    "EXAMPLES",
                    vec![
                        Item(
                            "**kwinfo --mock**",
                            "Synthetic sensors, for a look at KWinfo without reading any \
                             hardware.",
                        ),
                        Item(
                            "**kwinfo --replay** *my-machine*",
                            "Shows what **kwinfo-cli snapshot** or **kwinfo-cli record** \
                             captured into *my-machine*, on any computer.",
                        ),
                        Item(
                            "**kwinfo --theme light --interval 2000**",
                            "The light theme and a reading every two seconds, for this run \
                             only.",
                        ),
                    ],
                ),
                ("BUGS", vec![Para(BUGS)]),
                (
                    "SEE ALSO",
                    vec![Para(
                        "**kwinfo-cli**(1), **kwinfo-helper**(8), **sensors**(1), \
                         **sensors-detect**(8), **smartctl**(8)",
                    )],
                ),
            ],
        },
        Page {
            command: kwinfo_cli_args::Args::command(),
            section: "1",
            description: &[
                "**kwinfo-cli** runs the sensor backends of **kwinfo**(1) without a window. \
                 It lists what they read, shows what the privileged helper adds, bundles \
                 diagnostic reports, and captures snapshots and recordings of the sensor \
                 files that **kwinfo --replay** plays back on any computer: for \
                 comparing with **sensors**(1), for bug reports and for test fixtures.",
            ],
            sections: vec![
                (
                    "EXIT STATUS",
                    vec![Para(
                        "0 on success; 1 after an error, which is described on standard \
                         error; 2 for a usage error.",
                    )],
                ),
                ("ENVIRONMENT", cli_environment),
                (
                    "EXAMPLES",
                    vec![
                        Item(
                            "**kwinfo-cli list --ids**",
                            "Every sensor with its value and the stable id that the \
                             settings file uses.",
                        ),
                        Item(
                            "**kwinfo-cli helper --session**",
                            "Starts the helper through **pkexec**(1) for this command and \
                             shows what it reads.",
                        ),
                        Item(
                            "**sudo kwinfo-cli snapshot --out** *my-machine*",
                            "Captures the sensor files, including the RAPL counters only \
                             root may read, with serial numbers and MAC addresses scrubbed.",
                        ),
                        Item(
                            "**kwinfo-cli record --duration 2m --out** *my-machine*",
                            "Two minutes of readings for **kwinfo --replay** *my-machine*.",
                        ),
                        Item(
                            "**kwinfo-cli report**",
                            "One text file to attach to a bug report.",
                        ),
                    ],
                ),
                ("BUGS", vec![Para(BUGS)]),
                (
                    "SEE ALSO",
                    vec![Para(
                        "**kwinfo**(1), **kwinfo-helper**(8), **sensors**(1), \
                         **smartctl**(8)",
                    )],
                ),
            ],
        },
        Page {
            command: kwinfo_helper_args::Args::command(),
            section: "8",
            description: &[
                "**kwinfo-helper** reads, as root, the sensors the kernel keeps from other \
                 users, for **kwinfo**(1) and **kwinfo-cli**(1): CPU package, core, GT, \
                 DRAM and platform power from the RAPL energy counters; core voltages, \
                 power-limit status and package C-state residency from model-specific \
                 registers; S.M.A.R.T. data through **smartctl**(8); and the engine load of \
                 Intel GPUs from the i915 performance counters. It never writes to the \
                 hardware and takes no paths or commands from its callers.",
                "Nobody needs to run it by hand. As a D-Bus system service (**serve**) it \
                 is started through systemd by D-Bus activation on the first call to \
                 *io.github.kywa92.KWinfo.Helper*, and exits after two minutes without \
                 calls. polkit grants the action *io.github.kywa92.KWinfo.read-sensors* to \
                 the active local session without a password and refuses remote and \
                 inactive sessions. Without the service, KWinfo can start the helper for \
                 one session through **pkexec**(1) (**stdio**).",
                "The Debian package installs the service, with the helper in \
                 */usr/libexec/kwinfo*. For the AppImage or a build from source, **install** \
                 copies the helper to */usr/local/libexec/kwinfo* and writes the same \
                 files; **uninstall** removes exactly what it wrote.",
            ],
            sections: vec![
                (
                    "FILES",
                    vec![
                        Item(
                            "*/usr/libexec/kwinfo/kwinfo-helper*",
                            "The helper, as the package installs it.",
                        ),
                        Item(
                            "*/usr/local/libexec/kwinfo/kwinfo-helper*",
                            "The helper, as **install** copies it; *installed-files* beside \
                             it lists everything **install** wrote.",
                        ),
                        Item(
                            "*kwinfo-helper.service*",
                            "The systemd unit, sandboxed and without network access: in \
                             */usr/lib/systemd/system* from the package, in \
                             */etc/systemd/system* from **install**.",
                        ),
                        Item(
                            "*/usr/share/dbus-1/system-services/io.github.kywa92.KWinfo.Helper.service*",
                            "D-Bus activation.",
                        ),
                        Item(
                            "*io.github.kywa92.KWinfo.Helper.conf*",
                            "The D-Bus policy (only root may own the name): in \
                             */usr/share/dbus-1/system.d* from the package, in \
                             */etc/dbus-1/system.d* from **install**.",
                        ),
                        Item(
                            "*/usr/share/polkit-1/actions/io.github.kywa92.KWinfo.policy*",
                            "The polkit action.",
                        ),
                        Item(
                            "*kwinfo.conf*",
                            "Loads the msr module at boot: in */usr/lib/modules-load.d* \
                             from the package, in */etc/modules-load.d* from **install**.",
                        ),
                    ],
                ),
                (
                    "EXAMPLES",
                    vec![
                        Item(
                            "**sudo kwinfo-helper probe**",
                            "Every source the helper can open, why the others are missing, \
                             and two rounds of readings.",
                        ),
                        Item(
                            "**kwinfo-cli helper**",
                            "What the service reads, asked the way KWinfo asks.",
                        ),
                        Item(
                            "**journalctl -b -u kwinfo-helper.service**",
                            "The service's log.",
                        ),
                    ],
                ),
                ("BUGS", vec![Para(BUGS)]),
                (
                    "SEE ALSO",
                    vec![Para(
                        "**kwinfo**(1), **kwinfo-cli**(1), **pkexec**(1), **polkit**(8), \
                         **smartctl**(8), **systemd.exec**(5)",
                    )],
                ),
            ],
        },
    ]
}

const BUGS: &str = "Report problems at *https://github.com/kywa92/KWinfo/issues* and attach \
                    the file that **kwinfo-cli report** writes.";

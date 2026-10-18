//! Diagnostic logging for every KWinfo binary.
//!
//! Each run writes its own log file (by default under `~/.local/state/kwinfo/logs/` on
//! Linux) that starts with a header describing the machine, then records every tracing
//! event: `debug` and above for KWinfo's own crates, `info` and above for dependencies,
//! panics with a backtrace, the sampler's periodic health line, and user actions logged
//! with the [`action!`] macro. Old files are pruned so the directory never grows past a
//! few dozen runs. The [`report`] module bundles the newest logs with a system summary,
//! the configuration and a sensor inventory into one text file to attach to a bug report.
//!
//! Environment variables:
//!
//! | Variable | Effect |
//! |---|---|
//! | `KWINFO_LOG` | filter for what goes to stderr (default depends on the binary) |
//! | `KWINFO_FILE_LOG` | filter for what goes to the file (default: `debug` for KWinfo crates) |
//! | `KWINFO_LOG_DIR` | directory for the log files |
//! | `KWINFO_NO_FILE_LOG` | set to anything but `0` to disable the file entirely |

mod civil;
mod logfile;
pub mod report;
pub mod sysinfo;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, Layer as _};

/// Re-exported for the [`action!`] macro.
#[doc(hidden)]
pub use tracing;

pub use logfile::{default_log_dir, log_dir};

/// What the file receives unless `KWINFO_FILE_LOG` says otherwise: everything KWinfo
/// itself says at `debug`, dependencies at `info`, and the chattiest graphics crates at
/// `warn`.
pub const DEFAULT_FILE_FILTER: &str = "info,kwinfo=debug,kwinfo_core=debug,kwinfo_app=debug,\
kwinfo_cli=debug,kwinfo_helper=debug,kwinfo_ipc=debug,kwinfo_diag=debug,\
wgpu_core=warn,wgpu_hal=warn,naga=warn,winit=warn,calloop=warn,zbus=warn";

/// Logs a user action, e.g. `action!("settings.theme", mode = ?mode)`. Actions go to the
/// `kwinfo::action` target so they are easy to grep for in a log file.
#[macro_export]
macro_rules! action {
    ($name:expr) => {
        $crate::tracing::info!(target: "kwinfo::action", action = $name)
    };
    ($name:expr, $($field:tt)+) => {
        $crate::tracing::info!(target: "kwinfo::action", action = $name, $($field)+)
    };
}

#[derive(Clone, Debug)]
pub struct DiagConfig {
    /// Binary name, used in the log file name and the header.
    pub app: &'static str,
    pub version: &'static str,
    /// Default stderr filter when `KWINFO_LOG` is unset.
    pub stderr_default: &'static str,
    /// Where to write log files; `None` means `KWINFO_LOG_DIR` or the platform default.
    pub log_dir: Option<PathBuf>,
}

impl DiagConfig {
    pub fn new(app: &'static str, version: &'static str) -> Self {
        Self {
            app,
            version,
            stderr_default: "info",
            log_dir: None,
        }
    }

    pub fn stderr_default(mut self, filter: &'static str) -> Self {
        self.stderr_default = filter;
        self
    }

    pub fn log_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.log_dir = Some(dir.into());
        self
    }
}

/// Keeps the logging session alive; dropping it writes the "session end" line.
pub struct DiagGuard {
    log_path: Option<PathBuf>,
    started: Instant,
}

impl DiagGuard {
    /// The file this run logs to, if file logging is active.
    pub fn log_path(&self) -> Option<&Path> {
        self.log_path.as_deref()
    }
}

impl Drop for DiagGuard {
    fn drop(&mut self) {
        tracing::info!(
            target: "kwinfo::session",
            uptime_s = self.started.elapsed().as_secs(),
            "session end"
        );
    }
}

/// Installs the stderr and file loggers and the panic hook, then writes the session
/// header. Call once at the top of `main` and keep the guard alive until the end.
///
/// If a global tracing subscriber is already installed (tests do that), the existing one
/// is kept and only the header is written through it.
pub fn init(config: DiagConfig) -> DiagGuard {
    let stderr_filter = EnvFilter::try_from_env("KWINFO_LOG")
        .unwrap_or_else(|_| EnvFilter::new(config.stderr_default));
    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_filter(stderr_filter);
    let registry = tracing_subscriber::registry().with(stderr_layer);

    let disabled = std::env::var_os("KWINFO_NO_FILE_LOG").is_some_and(|v| v != "0");
    let opened = if disabled {
        None
    } else {
        Some(logfile::open(
            config.log_dir.as_deref(),
            config.app,
            config.version,
        ))
    };

    let mut log_path = None;
    let mut open_error = None;
    let init_result = match opened {
        Some(Ok(log)) => {
            log_path = Some(log.path);
            let file_filter = EnvFilter::try_from_env("KWINFO_FILE_LOG")
                .unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILE_FILTER));
            let file_layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_target(true)
                .with_thread_names(true)
                .with_writer(Mutex::new(log.file))
                .with_filter(file_filter);
            registry.with(file_layer).try_init()
        }
        Some(Err(err)) => {
            open_error = Some(err);
            registry.try_init()
        }
        None => registry.try_init(),
    };
    let already_installed = init_result.is_err();

    install_panic_hook();
    write_session_header(&config, log_path.as_deref());
    if let Some(err) = open_error {
        tracing::warn!(target: "kwinfo::session", "file logging disabled: {err:#}");
    }
    if already_installed {
        tracing::debug!(target: "kwinfo::session", "a tracing subscriber was already installed; keeping it");
    }
    DiagGuard {
        log_path,
        started: Instant::now(),
    }
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!(target: "kwinfo::panic", "{info}\n{backtrace}");
        previous(info);
    }));
}

fn write_session_header(config: &DiagConfig, log_path: Option<&Path>) {
    let args: Vec<String> = std::env::args().collect();
    tracing::info!(
        target: "kwinfo::session",
        app = config.app,
        version = config.version,
        profile = if cfg!(debug_assertions) { "debug" } else { "release" },
        pid = std::process::id(),
        log = log_path.map(|p| p.display().to_string()).unwrap_or_else(|| "(stderr only)".into()),
        "session start"
    );
    tracing::info!(target: "kwinfo::session", command_line = ?args);
    for (key, value) in sysinfo::collect() {
        tracing::info!(target: "kwinfo::session", "{key}: {value}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("kwinfo-diag-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The one test that installs the global subscriber; the others stay away from it.
    #[test]
    fn init_writes_banner_header_and_events() {
        let dir = temp_dir("init");
        let guard = init(DiagConfig::new("kwinfo-test", "0.0.0").log_dir(&dir));
        let path = guard.log_path().expect("file logging active").to_path_buf();
        tracing::info!("hello from the test");
        action!("test.action", answer = 42);
        drop(guard);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with("# kwinfo-test 0.0.0 diagnostic log"),
            "{text}"
        );
        assert!(text.contains("session start"), "{text}");
        assert!(text.contains("hello from the test"), "{text}");
        assert!(
            text.contains("kwinfo::action") && text.contains("answer=42"),
            "{text}"
        );
        assert!(text.contains("session end"), "{text}");
        assert!(text.contains("os: "), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

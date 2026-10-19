//! The sources the helper reads. Each one reads through a [`SysRoot`] (or a trait over
//! the device it talks to), so that it is unit-tested on any OS against fixtures.

pub mod gpu_pmu;
pub mod msr;
pub mod rapl;
pub mod smart;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use kwinfo_core::{Channel, Device, Sample, SysRoot};

/// A privileged source of readings.
pub trait Provider: Send {
    /// `rapl`, `msr`, `smart` or `gpu_pmu`: also the name of its [`kwinfo_ipc::Capabilities`]
    /// flag.
    fn name(&self) -> &'static str;

    /// Finds the devices and channels, replacing what an earlier call found. Called at
    /// start and then every [`Provider::rescan_every`].
    fn enumerate(&mut self) -> Vec<(Device, Vec<Channel>)>;

    /// Reads every channel the last [`Provider::enumerate`] found. Rates and powers are
    /// over the time since the previous call; the first call gives `None` for them.
    fn sample(&mut self, now: Instant, out: &mut Vec<Sample>);

    /// Whether the source works here and, if not, what the user can do about it.
    fn status(&self) -> Status;

    /// How often to look for new devices. Most sources are fixed at boot.
    fn rescan_every(&self) -> Duration {
        Duration::from_secs(600)
    }
}

/// Whether a source works, and a sentence for the user when it does not (or works
/// only in part).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub available: bool,
    pub note: Option<String>,
}

impl Status {
    pub fn working() -> Self {
        Self {
            available: true,
            note: None,
        }
    }

    pub fn missing(note: impl Into<String>) -> Self {
        Self {
            available: false,
            note: Some(note.into()),
        }
    }
}

/// What the helper reads from: the live system, or a captured tree for tests and
/// demonstrations (`kwinfo-helper stdio --root <dir>`).
#[derive(Clone, Debug)]
pub struct Sources {
    pub root: SysRoot,
    /// Captured `smartctl` output to use instead of running `smartctl`.
    pub smartctl_dir: Option<PathBuf>,
}

impl Sources {
    pub fn host() -> Self {
        Self {
            root: SysRoot::host(),
            smartctl_dir: None,
        }
    }
}

/// Every provider, reading `sources`.
pub fn all(sources: &Sources) -> Vec<Box<dyn Provider>> {
    let root = sources.root.clone();
    let smart: Box<dyn smart::SmartTool> = match &sources.smartctl_dir {
        Some(dir) => Box::new(smart::SmartctlDir::new(dir)),
        None if root.is_host() => match smart::Smartctl::find() {
            Some(tool) => Box::new(tool),
            None => Box::new(smart::NoSmartctl),
        },
        None => Box::new(smart::NoSmartctl),
    };
    vec![
        Box::new(rapl::Rapl::new(root.clone())),
        Box::new(msr::Msr::new(root.clone(), msr::DevMsr::new(root.clone()))),
        Box::new(smart::Smart::new(smart, root.is_host())),
        Box::new(gpu_pmu::GpuPmu::new(
            root.clone(),
            gpu_pmu::host_counters(&root),
        )),
    ]
}

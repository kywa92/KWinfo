//! Chooses which backends to run, and the Linux backends themselves.
//!
//! The Linux backends read sysfs and procfs through a [`SysRoot`], so they compile on
//! every OS and are unit-tested on macOS against the fixture trees in `tests/fixtures/`.
//! Only the pieces that need the live system (the libsensors overlay) are Linux-only.

pub mod block;
pub mod cpu;
mod dmi;
mod drm;
pub mod hwmon;
pub mod intel_gpu;
#[cfg(all(feature = "libsensors", target_os = "linux"))]
pub mod libsensors;
pub mod memory;
pub mod network;
mod pci;
pub mod power_supply;
mod util;

use std::path::PathBuf;

pub(crate) use pci::{PCI_IDS_PATHS, is_pci_address};

/// What the privileged helper (`kwinfo-helper`) shares with the backends here, so that
/// its rows join the same devices: the CPU packages (`cpu:<package>`) and their
/// topology, the GPUs (`drm:<PCI address>`), and the hashing that keeps serial numbers
/// out of device ids.
pub mod shared {
    pub use super::cpu::{
        CpuInfo, Thread, package_device, parse_cpuinfo, read_cpuinfo, read_topology,
    };
    pub use super::util::{short_hash, slug};

    use crate::model::Device;
    use crate::sysroot::SysRoot;

    /// A GPU as the DRM subsystem lists it.
    #[derive(Clone, Debug)]
    pub struct GpuInfo {
        /// PCI address, e.g. `0000:00:02.0`.
        pub address: String,
        /// The device its rows belong to: `drm:<address>`, `GPU [#0]: ...`.
        pub device: Device,
        /// The kernel driver bound to it, e.g. `i915`.
        pub driver: Option<String>,
    }

    /// Every PCI GPU with a DRM card, in the order and with the labels the backends use.
    pub fn gpus(root: &SysRoot) -> Vec<GpuInfo> {
        let mut names = super::pci::PciNames::default();
        super::drm::gpus(root, &mut names)
            .into_iter()
            .filter_map(|gpu| {
                let address = gpu.pci_dir.file_name()?.to_str()?.to_string();
                Some(GpuInfo {
                    address,
                    device: gpu.device,
                    driver: gpu.driver,
                })
            })
            .collect()
    }
}

use crate::backend::Backend;
use crate::mock::MockBackend;
use crate::recording;
use crate::sysroot::SysRoot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendChoice {
    /// Real hardware where available.
    Auto,
    /// The synthetic backend, for development and tests.
    Mock,
    /// The real Linux backends, reading a captured tree (`kwinfo-cli snapshot` output or a
    /// test fixture) instead of `/`, or playing a `kwinfo-cli record` recording back.
    /// Works on every OS.
    Replay(PathBuf),
}

pub struct BackendSet {
    pub backends: Vec<Box<dyn Backend>>,
    /// Human-readable notes about what was (not) selected, for the UI status line.
    pub notes: Vec<String>,
}

impl BackendSet {
    pub fn build(choice: BackendChoice) -> Self {
        let mut notes = Vec::new();
        let backends: Vec<Box<dyn Backend>> = match choice {
            BackendChoice::Mock => vec![Box::new(MockBackend::new(0x5eed))],
            BackendChoice::Replay(dir) if recording::is_recording(&dir) => {
                match recording::Playback::open(&dir) {
                    Ok((playback, root)) => {
                        notes.push(format!(
                            "Replaying a recording of {} frames from {}.",
                            playback.frame_count(),
                            dir.display()
                        ));
                        // First, so that every tick's frame is in place before the
                        // backends read it.
                        let mut backends: Vec<Box<dyn Backend>> = vec![Box::new(playback)];
                        backends.extend(linux_backends(&root, None));
                        backends
                    }
                    Err(err) => {
                        notes.push(format!(
                            "Cannot replay the recording in {}: {err:#}",
                            dir.display()
                        ));
                        Vec::new()
                    }
                }
            }
            BackendChoice::Replay(dir) => {
                let root = SysRoot::new(&dir);
                if !root.sys("").is_dir() && !root.proc("").is_dir() {
                    notes.push(format!(
                        "{} has neither sys/ nor proc/; is it a kwinfo-cli snapshot?",
                        dir.display()
                    ));
                }
                notes.push(format!("Replaying the captured tree {}.", dir.display()));
                linux_backends(&root, None)
            }
            BackendChoice::Auto => {
                #[cfg(target_os = "linux")]
                {
                    let root = SysRoot::host();
                    linux_backends(&root, host_overlay())
                }
                #[cfg(not(target_os = "linux"))]
                {
                    notes.push(
                        "Hardware backends only exist on Linux; showing the mock backend. \
                         Use --replay <dir> to run them against a captured tree."
                            .to_string(),
                    );
                    vec![Box::new(MockBackend::new(0x5eed))]
                }
            }
        };
        Self { backends, notes }
    }
}

/// The unprivileged Linux backends reading `root`, in the order their rows should appear
/// inside devices that several backends contribute to (temperatures first, then clocks
/// and usage for a CPU; temperatures, then rates for a drive).
pub fn linux_backends(
    root: &SysRoot,
    overlay: Option<Box<dyn hwmon::ChipOverlay>>,
) -> Vec<Box<dyn Backend>> {
    vec![
        Box::new(hwmon::HwmonBackend::new(root.clone()).with_overlay(overlay)),
        Box::new(cpu::CpuBackend::new(root.clone())),
        Box::new(intel_gpu::IntelGpuBackend::new(root.clone())),
        Box::new(memory::MemoryBackend::new(root.clone())),
        Box::new(block::BlockBackend::new(root.clone())),
        Box::new(network::NetworkBackend::new(root.clone())),
        Box::new(power_supply::PowerSupplyBackend::new(root.clone())),
    ]
}

/// libsensors labels and compute rules for the live system, when the feature is built in
/// and libsensors initialises.
#[cfg(target_os = "linux")]
fn host_overlay() -> Option<Box<dyn hwmon::ChipOverlay>> {
    #[cfg(feature = "libsensors")]
    {
        libsensors::LibSensorsOverlay::new().map(|o| Box::new(o) as Box<dyn hwmon::ChipOverlay>)
    }
    #[cfg(not(feature = "libsensors"))]
    {
        None
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Helpers for tests that run the backends against the fixture trees.

    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use crate::backend::Backend;
    use crate::model::{Channel, ChannelId, Device, Sample};
    use crate::sysroot::SysRoot;

    /// `tests/fixtures/synthetic-8700k` in the workspace.
    pub fn synthetic_8700k() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/synthetic-8700k")
    }

    pub fn fixture_root() -> SysRoot {
        SysRoot::new(synthetic_8700k())
    }

    /// A private, writable copy of the synthetic fixture, with symlinks preserved, for
    /// tests that change counters between two samples.
    pub fn fixture_copy() -> (tempfile::TempDir, SysRoot) {
        let dir = tempfile::tempdir().expect("temporary directory");
        crate::recording::copy_tree(&synthetic_8700k(), dir.path()).expect("copying the fixture");
        let root = SysRoot::new(dir.path());
        (dir, root)
    }

    pub fn write(root: &SysRoot, rel: &str, contents: &str) {
        fs::write(root.path(rel), contents).unwrap_or_else(|e| panic!("writing {rel}: {e}"));
    }

    /// Finds a device by id in an enumeration result.
    pub fn device<'a>(
        devices: &'a [(Device, Vec<Channel>)],
        id: &str,
    ) -> &'a (Device, Vec<Channel>) {
        devices
            .iter()
            .find(|(d, _)| d.id.as_str() == id)
            .unwrap_or_else(|| {
                let ids: Vec<&str> = devices.iter().map(|(d, _)| d.id.as_str()).collect();
                panic!("no device {id}; have {ids:?}")
            })
    }

    /// `(key, label)` of every channel, in order.
    pub fn keys_and_labels(channels: &[Channel]) -> Vec<(&str, &str)> {
        channels
            .iter()
            .map(|c| (c.id.key(), c.label.as_str()))
            .collect()
    }

    pub fn channel<'a>(channels: &'a [Channel], key: &str) -> &'a Channel {
        channels
            .iter()
            .find(|c| c.id.key() == key)
            .unwrap_or_else(|| panic!("no channel {key}"))
    }

    /// One sample of every channel, as a lookup by channel key within `device`.
    pub fn sample_map(samples: &[Sample]) -> std::collections::HashMap<ChannelId, Option<f64>> {
        samples
            .iter()
            .map(|s| (s.channel.clone(), s.value))
            .collect()
    }

    /// Two instants a fixed distance apart, for rate tests.
    pub fn instants(apart: Duration) -> (Instant, Instant) {
        let t0 = Instant::now();
        (t0, t0 + apart)
    }

    /// Enumerates then samples a backend once.
    pub fn enumerate_and_sample(
        backend: &mut dyn Backend,
    ) -> (Vec<(Device, Vec<Channel>)>, Vec<Sample>) {
        let devices = backend.enumerate().expect("enumeration succeeds");
        let mut samples = Vec::new();
        backend.sample(&mut samples).expect("sampling succeeds");
        (devices, samples)
    }

    /// Asserts a float equals the expected value to within 1e-9.
    #[track_caller]
    pub fn assert_close(actual: Option<f64>, expected: f64) {
        let actual = actual.unwrap_or_else(|| panic!("expected {expected}, got no value"));
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }
}

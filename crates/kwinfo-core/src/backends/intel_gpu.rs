//! The `intel_gpu` backend: clocks of Intel GPUs driven by `i915`.
//!
//! Files read under `/sys/class/drm/cardN`: `gt/gtM/rps_{act,cur,max,boost}_freq_mhz` on
//! kernels that have per-GT directories (5.18+), otherwise the older card-level
//! `gt_{act,cur,max,boost}_freq_mhz`. Busy percentages and GT power need the i915 PMU and
//! RAPL, which only the privileged helper can read (milestone M4).

use std::path::PathBuf;

use crate::backend::Backend;
use crate::model::{Channel, ChannelId, Device, Kind, Sample};
use crate::sysroot::{SysRoot, list_dir, read_int};

use super::drm;
use super::pci::PciNames;

/// The frequency files and the rows they become, in display order.
const CLOCKS: [(&str, &str); 4] = [
    ("act", "GPU Clock"),
    ("cur", "GPU Requested Clock"),
    ("max", "GPU Max Clock"),
    ("boost", "GPU Boost Clock"),
];

/// See the module documentation.
pub struct IntelGpuBackend {
    root: SysRoot,
    names: PciNames,
    readers: Vec<(ChannelId, PathBuf)>,
}

impl IntelGpuBackend {
    pub fn new(root: SysRoot) -> Self {
        Self {
            root,
            names: PciNames::default(),
            readers: Vec::new(),
        }
    }
}

/// `(gt number, [(file key, path)])` for every GT of the card that exposes clocks.
fn frequency_files(card_dir: &std::path::Path) -> Vec<(u32, Vec<(&'static str, PathBuf)>)> {
    let mut gts = Vec::new();
    let gt_root = card_dir.join("gt");
    for name in list_dir(&gt_root) {
        let Some(Ok(n)) = name.strip_prefix("gt").map(str::parse::<u32>) else {
            continue;
        };
        let files: Vec<(&str, PathBuf)> = CLOCKS
            .iter()
            .map(|(key, _)| {
                (
                    *key,
                    gt_root.join(&name).join(format!("rps_{key}_freq_mhz")),
                )
            })
            .filter(|(_, path)| path.exists())
            .collect();
        if !files.is_empty() {
            gts.push((n, files));
        }
    }
    if gts.is_empty() {
        let files: Vec<(&str, PathBuf)> = CLOCKS
            .iter()
            .map(|(key, _)| (*key, card_dir.join(format!("gt_{key}_freq_mhz"))))
            .filter(|(_, path)| path.exists())
            .collect();
        if !files.is_empty() {
            gts.push((0, files));
        }
    }
    gts
}

impl Backend for IntelGpuBackend {
    fn name(&self) -> &'static str {
        "intel_gpu"
    }

    fn enumerate(&mut self) -> anyhow::Result<Vec<(Device, Vec<Channel>)>> {
        self.readers.clear();
        let mut result = Vec::new();
        for gpu in drm::gpus(&self.root, &mut self.names) {
            if gpu.vendor != 0x8086 {
                continue;
            }
            let id = gpu.device.id.clone();
            let mut channels = Vec::new();
            for (gt, files) in frequency_files(&gpu.card_dir) {
                for (key, path) in files {
                    let label = CLOCKS
                        .iter()
                        .find(|(k, _)| *k == key)
                        .map_or("GPU Clock", |(_, label)| label);
                    let label = if gt == 0 {
                        label.to_string()
                    } else {
                        format!("{label} (GT{gt})")
                    };
                    let channel =
                        Channel::new(&id, &format!("gt{gt}.{key}_freq"), label, Kind::Clock);
                    self.readers.push((channel.id.clone(), path));
                    channels.push(channel);
                }
            }
            if channels.is_empty() {
                tracing::debug!(
                    gpu = %gpu.device.label,
                    driver = gpu.driver.as_deref().unwrap_or("none"),
                    "no i915 frequency files; GPU clocks unavailable"
                );
            } else {
                result.push((gpu.device, channels));
            }
        }
        Ok(result)
    }

    fn sample(&mut self, out: &mut Vec<Sample>) -> anyhow::Result<()> {
        for (channel, path) in &self.readers {
            let mhz = read_int(path).ok().map(|v| v as f64);
            out.push(Sample::new(channel.clone(), mhz));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::test_support::*;
    use crate::model::Unit;

    #[test]
    fn reads_per_gt_clocks() {
        let mut backend = IntelGpuBackend::new(fixture_root());
        let (devices, samples) = enumerate_and_sample(&mut backend);
        let (device, channels) = device(&devices, "drm:0000:00:02.0");
        assert_eq!(device.label, "GPU [#0]: Intel UHD Graphics 630");
        assert_eq!(
            keys_and_labels(channels),
            [
                ("gt0.act_freq", "GPU Clock"),
                ("gt0.cur_freq", "GPU Requested Clock"),
                ("gt0.max_freq", "GPU Max Clock"),
                ("gt0.boost_freq", "GPU Boost Clock"),
            ]
        );
        assert!(
            channels
                .iter()
                .all(|c| c.unit == Unit::MegaHertz && c.kind == Kind::Clock)
        );
        let values = sample_map(&samples);
        let get = |key: &str| values[&ChannelId::from_raw(format!("drm:0000:00:02.0/{key}"))];
        assert_close(get("gt0.act_freq"), 350.0);
        assert_close(get("gt0.cur_freq"), 350.0);
        assert_close(get("gt0.max_freq"), 1200.0);
        assert_close(get("gt0.boost_freq"), 1200.0);
    }

    #[test]
    fn older_kernels_use_card_level_files_under_the_same_keys() {
        let (_dir, root) = fixture_copy();
        let card = root.sys("devices/pci0000:00/0000:00:02.0/drm/card0");
        std::fs::remove_dir_all(card.join("gt")).unwrap();
        std::fs::write(card.join("gt_act_freq_mhz"), "1150\n").unwrap();
        let mut backend = IntelGpuBackend::new(root);
        let (devices, samples) = enumerate_and_sample(&mut backend);
        let (_, channels) = device(&devices, "drm:0000:00:02.0");
        assert_eq!(channels.len(), 4);
        let values = sample_map(&samples);
        assert_close(
            values[&ChannelId::from_raw("drm:0000:00:02.0/gt0.act_freq")],
            1150.0,
        );
    }
}

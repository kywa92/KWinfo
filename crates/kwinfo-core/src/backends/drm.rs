//! GPUs as the DRM subsystem lists them (`/sys/class/drm/cardN`). Shared by the
//! `intel_gpu` backend and the `hwmon` backend (for `amdgpu`, `i915` and `xe` sensor
//! chips), so that both put their rows into the same "GPU [#0]: ..." group, whose id is
//! `drm:<PCI address>`.

use std::path::PathBuf;

use crate::model::{Category, Device};
use crate::sysroot::{SysRoot, link_name, list_dir, read_hex};

use super::pci::{PciNames, is_pci_address, marketing_name, vendor_short_name};

#[derive(Debug)]
pub(crate) struct Gpu {
    /// The card's own directory, e.g. `.../0000:00:02.0/drm/card0`, resolved.
    pub card_dir: PathBuf,
    /// The PCI device directory, resolved.
    pub pci_dir: PathBuf,
    pub vendor: u32,
    pub driver: Option<String>,
    pub device: Device,
}

/// Every PCI GPU with a DRM card, numbered `GPU [#0]`, `GPU [#1]`, ... in PCI address
/// order. Connectors (`card0-DP-1`), render nodes and non-PCI framebuffers are skipped.
pub(crate) fn gpus(root: &SysRoot, names: &mut PciNames) -> Vec<Gpu> {
    let class = root.sys("class/drm");
    let mut found: Vec<(String, PathBuf, PathBuf)> = Vec::new();
    for name in list_dir(&class) {
        let Some(number) = name.strip_prefix("card") else {
            continue;
        };
        if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let link = class.join(&name);
        let (Some(card_dir), Some(pci_dir)) =
            (root.resolve(&link), root.resolve(&link.join("device")))
        else {
            continue;
        };
        let Some(address) = pci_dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !is_pci_address(address) || found.iter().any(|(a, _, _)| a == address) {
            continue;
        }
        found.push((address.to_string(), card_dir, pci_dir));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
        .into_iter()
        .enumerate()
        .map(|(index, (address, card_dir, pci_dir))| {
            let vendor = read_hex(&pci_dir.join("vendor")).unwrap_or(0);
            let device_id = read_hex(&pci_dir.join("device")).unwrap_or(0);
            let driver = link_name(&pci_dir.join("driver"));
            let name = gpu_name(root, names, vendor, device_id);
            let device = Device::new(
                format!("drm:{address}"),
                format!("GPU [#{index}]: {name}"),
                Category::Gpu,
            )
            .with_order(u16::try_from(index).unwrap_or(u16::MAX));
            Gpu {
                card_dir,
                pci_dir,
                vendor,
                driver,
                device,
            }
        })
        .collect()
}

/// "Intel UHD Graphics 630" from pci.ids, or "Intel Graphics [8086:3e92]" without it.
fn gpu_name(root: &SysRoot, names: &mut PciNames, vendor: u32, device: u32) -> String {
    let short = vendor_short_name(vendor);
    match names.device_name(root, vendor, device) {
        Some(raw) => {
            let name = marketing_name(&raw);
            match short {
                Some(v) if !name.starts_with(v) => format!("{v} {name}"),
                _ => name.to_string(),
            }
        }
        None => format!(
            "{} Graphics [{vendor:04x}:{device:04x}]",
            short.unwrap_or("Unknown")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::test_support::{fixture_copy, fixture_root};

    #[test]
    fn lists_the_igpu_with_its_marketing_name() {
        let gpus = gpus(&fixture_root(), &mut PciNames::default());
        assert_eq!(gpus.len(), 1, "connectors and render nodes are not GPUs");
        let gpu = &gpus[0];
        assert_eq!(gpu.device.id.as_str(), "drm:0000:00:02.0");
        assert_eq!(gpu.device.label, "GPU [#0]: Intel UHD Graphics 630");
        assert_eq!(gpu.device.category, Category::Gpu);
        assert_eq!(gpu.vendor, 0x8086);
        assert_eq!(gpu.driver.as_deref(), Some("i915"));
        assert!(gpu.card_dir.ends_with("0000:00:02.0/drm/card0"));
    }

    #[test]
    fn falls_back_to_pci_ids_numbers_without_a_name_database() {
        let (_dir, root) = fixture_copy();
        std::fs::remove_file(root.path("usr/share/hwdata/pci.ids")).unwrap();
        let gpus = gpus(&root, &mut PciNames::default());
        assert_eq!(gpus[0].device.label, "GPU [#0]: Intel Graphics [8086:3e92]");
    }
}

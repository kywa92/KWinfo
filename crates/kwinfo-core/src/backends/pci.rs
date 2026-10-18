//! PCI addresses and device names from the `pci.ids` database that `hwdata` (or the
//! `pci.ids` package) installs. Used to turn `8086:3e92` into "Intel UHD Graphics 630".

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};

use crate::sysroot::SysRoot;

/// Where distributions install pci.ids, relative to the root, in order of preference.
/// `kwinfo-cli snapshot` writes the subset a machine needs to the first of these.
pub(crate) const PCI_IDS_PATHS: [&str; 3] = [
    "usr/share/hwdata/pci.ids",
    "usr/share/misc/pci.ids",
    "usr/share/pci.ids",
];

/// Whether `s` is a PCI address such as `0000:00:02.0` (domain, bus, device, function).
pub(crate) fn is_pci_address(s: &str) -> bool {
    let mut parts = s.split(':');
    let (Some(domain), Some(bus), Some(slot), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let Some((device, function)) = slot.split_once('.') else {
        return false;
    };
    let hex = |p: &str, len: usize| p.len() == len && p.bytes().all(|b| b.is_ascii_hexdigit());
    domain.len() >= 4
        && domain.bytes().all(|b| b.is_ascii_hexdigit())
        && hex(bus, 2)
        && hex(device, 2)
        && function.len() == 1
        && matches!(function.as_bytes()[0], b'0'..=b'7')
}

/// Short vendor names as tools such as HWiNFO print them.
pub(crate) fn vendor_short_name(vendor: u32) -> Option<&'static str> {
    match vendor {
        0x8086 => Some("Intel"),
        0x1002 | 0x1022 => Some("AMD"),
        0x10de => Some("NVIDIA"),
        0x144d => Some("Samsung"),
        0x15b7 => Some("Sandisk"),
        _ => None,
    }
}

/// The marketing name inside pci.ids' brackets, or the whole name when it has none:
/// `CoffeeLake-S GT2 [UHD Graphics 630]` → `UHD Graphics 630`.
pub(crate) fn marketing_name(raw: &str) -> &str {
    match (raw.rfind('['), raw.rfind(']')) {
        (Some(open), Some(close)) if open < close => raw[open + 1..close].trim(),
        _ => raw.trim(),
    }
}

/// Looks up and caches device names from pci.ids. The file is a megabyte or more, so
/// each backend keeps one of these across rescans.
#[derive(Debug, Default)]
pub(crate) struct PciNames {
    cache: HashMap<(u32, u32), Option<String>>,
}

impl PciNames {
    /// The pci.ids name of `vendor:device`, e.g. `CoffeeLake-S GT2 [UHD Graphics 630]`.
    pub fn device_name(&mut self, root: &SysRoot, vendor: u32, device: u32) -> Option<String> {
        self.cache
            .entry((vendor, device))
            .or_insert_with(|| {
                PCI_IDS_PATHS.iter().find_map(|rel| {
                    let file = File::open(root.path(rel)).ok()?;
                    find_device(BufReader::new(file), vendor, device)
                })
            })
            .clone()
    }
}

/// Scans pci.ids for a device line under the vendor's section.
pub(crate) fn find_device(reader: impl BufRead, vendor: u32, device: u32) -> Option<String> {
    let vendor_hex = format!("{vendor:04x}");
    let device_hex = format!("{device:04x}");
    let mut in_vendor = false;
    for line in reader.split(b'\n') {
        let line = line.ok()?;
        let line = String::from_utf8_lossy(&line);
        let line = line.trim_end_matches('\r');
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if line.starts_with("C ") {
            // Device classes follow the vendor list.
            break;
        }
        if let Some(rest) = line.strip_prefix('\t') {
            if in_vendor
                && !rest.starts_with('\t')
                && rest
                    .get(..4)
                    .is_some_and(|d| d.eq_ignore_ascii_case(&device_hex))
            {
                return Some(rest[4..].trim().to_string());
            }
        } else {
            if in_vendor {
                return None;
            }
            in_vendor = line
                .get(..4)
                .is_some_and(|v| v.eq_ignore_ascii_case(&vendor_hex));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# pci.ids excerpt
1002  Advanced Micro Devices, Inc. [AMD/ATI]
\t73bf  Navi 21 [Radeon RX 6800/6800 XT / 6900 XT]
8086  Intel Corporation
\t15b8  Ethernet Connection (2) I219-V
\t3e92  CoffeeLake-S GT2 [UHD Graphics 630]
\t\t1043 8694  PRIME Z370-A
\t5912  HD Graphics 630
C 00  Unclassified device
\t3e92  not a device
";

    #[test]
    fn finds_devices_under_their_vendor() {
        let find = |v, d| find_device(SAMPLE.as_bytes(), v, d);
        assert_eq!(
            find(0x8086, 0x3e92).as_deref(),
            Some("CoffeeLake-S GT2 [UHD Graphics 630]")
        );
        assert_eq!(
            find(0x1002, 0x73bf).as_deref(),
            Some("Navi 21 [Radeon RX 6800/6800 XT / 6900 XT]")
        );
        assert_eq!(find(0x8086, 0x9999), None);
        assert_eq!(find(0x10de, 0x3e92), None);
    }

    #[test]
    fn marketing_names_come_from_brackets() {
        assert_eq!(
            marketing_name("CoffeeLake-S GT2 [UHD Graphics 630]"),
            "UHD Graphics 630"
        );
        assert_eq!(marketing_name("HD Graphics 630"), "HD Graphics 630");
    }

    #[test]
    fn recognises_pci_addresses() {
        assert!(is_pci_address("0000:00:02.0"));
        assert!(is_pci_address("10000:e1:00.7"));
        assert!(!is_pci_address("0000:00:02.8"));
        assert!(!is_pci_address("nvme0"));
        assert!(!is_pci_address("coretemp.0"));
        assert!(!is_pci_address("0:0:0:0"));
    }
}

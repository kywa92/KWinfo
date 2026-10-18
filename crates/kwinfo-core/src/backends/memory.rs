//! The `memory` backend: physical memory and swap use from `/proc/meminfo`.
//!
//! "Used" is `MemTotal − MemAvailable`, the memory that cannot be reclaimed without
//! swapping, which is what `free` reports as used since procps-ng 4.

use std::collections::HashMap;

use crate::backend::Backend;
use crate::model::{Category, Channel, ChannelId, Device, DeviceId, Kind, Sample};
use crate::sysroot::{SysRoot, read_into};

/// `/proc/meminfo` in bytes, keyed by field name.
pub(crate) fn parse_meminfo(text: &str) -> HashMap<&str, u64> {
    text.lines()
        .filter_map(|line| {
            let (key, rest) = line.split_once(':')?;
            let mut parts = rest.split_ascii_whitespace();
            let value: u64 = parts.next()?.parse().ok()?;
            let bytes = match parts.next() {
                Some("kB") => value * 1024,
                _ => value,
            };
            Some((key.trim(), bytes))
        })
        .collect()
}

#[derive(Debug)]
struct Plan {
    used: ChannelId,
    available: ChannelId,
    load: ChannelId,
    swap: Option<(ChannelId, ChannelId)>,
}

/// See the module documentation.
pub struct MemoryBackend {
    root: SysRoot,
    plan: Option<Plan>,
    buf: Vec<u8>,
}

impl MemoryBackend {
    pub fn new(root: SysRoot) -> Self {
        Self {
            root,
            plan: None,
            buf: Vec::with_capacity(4096),
        }
    }
}

fn read_meminfo<'b>(root: &SysRoot, buf: &'b mut Vec<u8>) -> Option<HashMap<&'b str, u64>> {
    read_into(&root.proc("meminfo"), buf).ok()?;
    let text = std::str::from_utf8(buf).ok()?;
    Some(parse_meminfo(text))
}

impl Backend for MemoryBackend {
    fn name(&self) -> &'static str {
        "memory"
    }

    fn enumerate(&mut self) -> anyhow::Result<Vec<(Device, Vec<Channel>)>> {
        self.plan = None;
        let Some(info) = read_meminfo(&self.root, &mut self.buf) else {
            return Ok(Vec::new());
        };
        if !info.contains_key("MemTotal") {
            return Ok(Vec::new());
        }
        let has_swap = info.get("SwapTotal").copied().unwrap_or(0) > 0;
        let device = Device::new("memory", "Memory", Category::Memory);
        let id: DeviceId = device.id.clone();
        let mut channels = vec![
            Channel::new(&id, "used", "Physical Memory Used", Kind::Capacity),
            Channel::new(
                &id,
                "available",
                "Physical Memory Available",
                Kind::Capacity,
            ),
            Channel::new(&id, "load", "Physical Memory Load", Kind::Usage),
        ];
        let swap = has_swap.then(|| {
            channels.push(Channel::new(&id, "swap_used", "Swap Used", Kind::Capacity));
            channels.push(Channel::new(&id, "swap_load", "Swap Load", Kind::Usage));
            (channels[3].id.clone(), channels[4].id.clone())
        });
        self.plan = Some(Plan {
            used: channels[0].id.clone(),
            available: channels[1].id.clone(),
            load: channels[2].id.clone(),
            swap,
        });
        Ok(vec![(device, channels)])
    }

    fn sample(&mut self, out: &mut Vec<Sample>) -> anyhow::Result<()> {
        let Some(plan) = &self.plan else {
            return Ok(());
        };
        let info = read_meminfo(&self.root, &mut self.buf);
        let get = |key: &str| info.as_ref().and_then(|i| i.get(key).copied());
        let total = get("MemTotal").filter(|&t| t > 0);
        let available = get("MemAvailable").or_else(|| {
            // Kernels before 3.14 lack MemAvailable; approximate it like procps does.
            Some(get("MemFree")? + get("Buffers").unwrap_or(0) + get("Cached").unwrap_or(0))
        });
        let used = total.zip(available).map(|(t, a)| t.saturating_sub(a));
        out.push(Sample::new(plan.used.clone(), used.map(|u| u as f64)));
        out.push(Sample::new(
            plan.available.clone(),
            available.map(|a| a as f64),
        ));
        out.push(Sample::new(
            plan.load.clone(),
            total.zip(used).map(|(t, u)| u as f64 / t as f64 * 100.0),
        ));
        if let Some((swap_used, swap_load)) = &plan.swap {
            let swap_total = get("SwapTotal").filter(|&t| t > 0);
            let used = swap_total
                .zip(get("SwapFree"))
                .map(|(t, f)| t.saturating_sub(f));
            out.push(Sample::new(swap_used.clone(), used.map(|u| u as f64)));
            out.push(Sample::new(
                swap_load.clone(),
                swap_total
                    .zip(used)
                    .map(|(t, u)| u as f64 / t as f64 * 100.0),
            ));
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
    fn meminfo_values_are_bytes() {
        let info = parse_meminfo("MemTotal:       32803620 kB\nHugePages_Total:       0\n");
        assert_eq!(info["MemTotal"], 32_803_620 * 1024);
        assert_eq!(info["HugePages_Total"], 0);
    }

    #[test]
    fn used_available_load_and_swap() {
        let mut backend = MemoryBackend::new(fixture_root());
        let (devices, samples) = enumerate_and_sample(&mut backend);
        let (device, channels) = device(&devices, "memory");
        assert_eq!(device.label, "Memory");
        assert_eq!(device.category, Category::Memory);
        assert_eq!(
            keys_and_labels(channels),
            [
                ("used", "Physical Memory Used"),
                ("available", "Physical Memory Available"),
                ("load", "Physical Memory Load"),
                ("swap_used", "Swap Used"),
                ("swap_load", "Swap Load"),
            ]
        );
        assert_eq!(channel(channels, "used").unit, Unit::Bytes);
        let values = sample_map(&samples);
        let get = |key: &str| values[&ChannelId::new(&device.id, key)];
        let total = 32_803_620.0 * 1024.0;
        let available = 26_543_210.0 * 1024.0;
        assert_close(get("available"), available);
        assert_close(get("used"), total - available);
        assert_close(get("load"), (total - available) / total * 100.0);
        assert_close(get("swap_used"), (2_097_148.0 - 1_835_004.0) * 1024.0);
        assert_close(get("swap_load"), 262_144.0 / 2_097_148.0 * 100.0);
    }

    #[test]
    fn swap_rows_need_swap() {
        let (_dir, root) = fixture_copy();
        let meminfo = std::fs::read_to_string(root.proc("meminfo"))
            .unwrap()
            .replace("SwapTotal:       2097148 kB", "SwapTotal:             0 kB");
        write(&root, "proc/meminfo", &meminfo);
        let mut backend = MemoryBackend::new(root);
        let devices = backend.enumerate().unwrap();
        assert_eq!(devices[0].1.len(), 3);
    }
}

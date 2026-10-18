//! The `network` backend: download and upload rates and since-boot totals per interface.
//!
//! Files read under `/sys/class/net/<iface>`: `type` (to skip loopback) and the resolved
//! path (virtual interfaces such as bridges, veth pairs, tun/tap, WireGuard or Docker's
//! live under `devices/virtual`), then every tick `statistics/{rx,tx}_bytes`. Loopback and
//! virtual interfaces are hidden unless [`NetworkBackend::with_virtual`] asks for them.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use crate::backend::Backend;
use crate::model::{Category, Channel, ChannelId, Device, Kind, Sample};
use crate::sysroot::{SysRoot, list_dir, read_u64};

use super::util::{Counter, key_part};

/// `ARPHRD_LOOPBACK` from `<linux/if_arp.h>`.
const ARPHRD_LOOPBACK: u64 = 772;

#[derive(Debug)]
struct InterfacePlan {
    name: String,
    rx_bytes: PathBuf,
    tx_bytes: PathBuf,
    download: ChannelId,
    upload: ChannelId,
    rx_total: ChannelId,
    tx_total: ChannelId,
    /// `rx_bytes` feeds only disabled channels and is not read.
    skip_rx: bool,
    /// Likewise for `tx_bytes`.
    skip_tx: bool,
}

/// See the module documentation.
pub struct NetworkBackend {
    root: SysRoot,
    include_virtual: bool,
    interfaces: Vec<InterfacePlan>,
    counters: HashMap<(String, bool), Counter>,
}

impl NetworkBackend {
    pub fn new(root: SysRoot) -> Self {
        Self {
            root,
            include_virtual: false,
            interfaces: Vec::new(),
            counters: HashMap::new(),
        }
    }

    /// Also reports loopback and virtual interfaces.
    pub fn with_virtual(mut self, include: bool) -> Self {
        self.include_virtual = include;
        self
    }

    /// One sampling pass at an explicit time; [`Backend::sample`] passes the root's clock
    /// ([`SysRoot::now`]), which is the recorded time when a recording is replayed.
    pub fn sample_at(&mut self, now: Instant, out: &mut Vec<Sample>) {
        for iface in &self.interfaces {
            for (path, rate_channel, total_channel, is_rx, skip) in [
                (
                    &iface.rx_bytes,
                    &iface.download,
                    &iface.rx_total,
                    true,
                    iface.skip_rx,
                ),
                (
                    &iface.tx_bytes,
                    &iface.upload,
                    &iface.tx_total,
                    false,
                    iface.skip_tx,
                ),
            ] {
                if skip {
                    continue;
                }
                let bytes = read_u64(path);
                let rate = bytes.and_then(|b| {
                    self.counters
                        .entry((iface.name.clone(), is_rx))
                        .or_default()
                        .rate(b, now)
                });
                out.push(Sample::new(rate_channel.clone(), rate));
                out.push(Sample::new(total_channel.clone(), bytes.map(|b| b as f64)));
            }
        }
    }
}

impl Backend for NetworkBackend {
    fn name(&self) -> &'static str {
        "network"
    }

    fn enumerate(&mut self) -> anyhow::Result<Vec<(Device, Vec<Channel>)>> {
        self.interfaces.clear();
        let class = self.root.sys("class/net");
        let mut result = Vec::new();
        for name in list_dir(&class) {
            let Some(dir) = self.root.resolve(&class.join(&name)) else {
                continue;
            };
            let stats = dir.join("statistics");
            if !stats.join("rx_bytes").exists() {
                continue;
            }
            let is_virtual = dir.to_string_lossy().contains("/devices/virtual/")
                || read_u64(&dir.join("type")) == Some(ARPHRD_LOOPBACK);
            if is_virtual && !self.include_virtual {
                continue;
            }
            let order = u16::try_from(result.len()).unwrap_or(u16::MAX);
            let device = Device::new(
                format!("net:{}", key_part(&name)),
                format!("Network: {name}"),
                Category::Network,
            )
            .with_order(order);
            let id = device.id.clone();
            let channels = vec![
                Channel::new(&id, "download", "Download Rate", Kind::Throughput),
                Channel::new(&id, "upload", "Upload Rate", Kind::Throughput),
                Channel::new(&id, "total_down", "Total Downloaded", Kind::Capacity),
                Channel::new(&id, "total_up", "Total Uploaded", Kind::Capacity),
            ];
            self.interfaces.push(InterfacePlan {
                name,
                rx_bytes: stats.join("rx_bytes"),
                tx_bytes: stats.join("tx_bytes"),
                download: channels[0].id.clone(),
                upload: channels[1].id.clone(),
                rx_total: channels[2].id.clone(),
                tx_total: channels[3].id.clone(),
                skip_rx: false,
                skip_tx: false,
            });
            result.push((device, channels));
        }
        Ok(result)
    }

    fn sample(&mut self, out: &mut Vec<Sample>) -> anyhow::Result<()> {
        self.sample_at(self.root.now(), out);
        Ok(())
    }

    /// A byte counter is skipped when both rows it feeds (the rate and the total) are
    /// disabled; its rate starts afresh when one of them is enabled again.
    fn set_disabled(&mut self, disabled: &HashSet<ChannelId>) {
        for iface in &mut self.interfaces {
            iface.skip_rx =
                disabled.contains(&iface.download) && disabled.contains(&iface.rx_total);
            iface.skip_tx = disabled.contains(&iface.upload) && disabled.contains(&iface.tx_total);
            for (skip, is_rx) in [(iface.skip_rx, true), (iface.skip_tx, false)] {
                if skip {
                    self.counters.remove(&(iface.name.clone(), is_rx));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::backends::test_support::*;
    use crate::model::Unit;

    #[test]
    fn hides_loopback_and_virtual_interfaces() {
        let mut backend = NetworkBackend::new(fixture_root());
        let devices = backend.enumerate().unwrap();
        let ids: Vec<&str> = devices.iter().map(|(d, _)| d.id.as_str()).collect();
        assert_eq!(ids, ["net:enp0s31f6"]);
        let (device, channels) = &devices[0];
        assert_eq!(device.label, "Network: enp0s31f6");
        assert_eq!(device.category, Category::Network);
        assert_eq!(
            keys_and_labels(channels),
            [
                ("download", "Download Rate"),
                ("upload", "Upload Rate"),
                ("total_down", "Total Downloaded"),
                ("total_up", "Total Uploaded"),
            ]
        );
        assert_eq!(channel(channels, "download").unit, Unit::BytesPerSecond);

        let mut all = NetworkBackend::new(fixture_root()).with_virtual(true);
        let ids: Vec<String> = all
            .enumerate()
            .unwrap()
            .into_iter()
            .map(|(d, _)| d.id.to_string())
            .collect();
        assert_eq!(ids, ["net:docker0", "net:enp0s31f6", "net:lo"]);
    }

    #[test]
    fn rates_and_totals_follow_the_byte_counters() {
        let (_dir, root) = fixture_copy();
        let mut backend = NetworkBackend::new(root.clone());
        backend.enumerate().unwrap();
        let (t0, t1) = instants(Duration::from_millis(500));
        let mut first = Vec::new();
        backend.sample_at(t0, &mut first);
        let values = sample_map(&first);
        let get = |values: &HashMap<ChannelId, Option<f64>>, key: &str| {
            values[&ChannelId::from_raw(format!("net:enp0s31f6/{key}"))]
        };
        assert_eq!(get(&values, "download"), None);
        assert_close(get(&values, "total_down"), 8_123_456_789.0);
        assert_close(get(&values, "total_up"), 912_345_678.0);

        let stats = "sys/devices/pci0000:00/0000:00:1f.6/net/enp0s31f6/statistics";
        write(&root, &format!("{stats}/rx_bytes"), "8124505365\n");
        write(&root, &format!("{stats}/tx_bytes"), "912345678\n");
        let mut second = Vec::new();
        backend.sample_at(t1, &mut second);
        let values = sample_map(&second);
        // 1,048,576 bytes in half a second.
        assert_close(get(&values, "download"), 2.0 * 1_048_576.0);
        assert_close(get(&values, "upload"), 0.0);
    }

    #[test]
    fn a_counter_is_read_while_any_row_it_feeds_is_enabled() {
        let mut backend = NetworkBackend::new(fixture_root());
        backend.enumerate().unwrap();
        let id = |key: &str| ChannelId::from_raw(format!("net:enp0s31f6/{key}"));
        backend.set_disabled(&HashSet::from([
            id("download"),
            id("total_down"),
            id("upload"),
        ]));
        let mut out = Vec::new();
        backend.sample_at(Instant::now(), &mut out);
        let values = sample_map(&out);
        assert!(!values.contains_key(&id("download")));
        assert!(!values.contains_key(&id("total_down")));
        // tx_bytes still feeds the enabled total.
        assert_close(values[&id("total_up")], 912_345_678.0);
    }
}

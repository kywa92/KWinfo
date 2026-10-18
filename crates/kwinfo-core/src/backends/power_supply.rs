//! The `power_supply` backend: batteries and UPSes from `/sys/class/power_supply`.
//!
//! Files read under `/sys/class/power_supply/<name>`: `type`, `present`, `scope`,
//! `status`, `capacity`, `voltage_now`, `current_now`, `power_now`,
//! `energy_{now,full,full_design}` (or `charge_*` with `voltage_min_design`),
//! `cycle_count`, `temp`, `manufacturer`, `model_name`; and `online` of the mains
//! adapter for the "AC Power" row. Mains and USB supplies get no group of their own.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::backend::Backend;
use crate::model::{Category, Channel, ChannelId, Device, Kind, Sample, Unit};
use crate::sysroot::{SysRoot, list_dir, read_i64, read_string};

use super::util::{key_part, squash_spaces};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    ChargeLevel,
    Voltage,
    Current,
    ChargeRate,
    Remaining,
    FullCapacity,
    Wear,
    Cycles,
    Temperature,
    Charging,
    Discharging,
    AcPower,
}

#[derive(Debug)]
struct SupplyPlan {
    dir: PathBuf,
    rows: Vec<(ChannelId, Row)>,
}

/// See the module documentation.
pub struct PowerSupplyBackend {
    root: SysRoot,
    supplies: Vec<SupplyPlan>,
    mains: Option<PathBuf>,
    /// Rows disabled by the user, which are not read.
    disabled: HashSet<ChannelId>,
}

impl PowerSupplyBackend {
    pub fn new(root: SysRoot) -> Self {
        Self {
            root,
            supplies: Vec::new(),
            mains: None,
            disabled: HashSet::new(),
        }
    }
}

/// Micro-units (µV, µA, µW, µWh) to base units.
fn micro(dir: &Path, file: &str) -> Option<f64> {
    read_i64(&dir.join(file)).map(|v| v as f64 / 1e6)
}

/// Energy in Wh from `energy_<what>` (µWh), or from `charge_<what>` (µAh) times the
/// design voltage for batteries that report charge.
fn energy_wh(dir: &Path, what: &str) -> Option<f64> {
    micro(dir, &format!("energy_{what}")).or_else(|| {
        let charge = micro(dir, &format!("charge_{what}"))?;
        let volts = micro(dir, "voltage_min_design").or_else(|| micro(dir, "voltage_now"))?;
        Some(charge * volts)
    })
}

fn has(dir: &Path, file: &str) -> bool {
    dir.join(file).exists()
}

impl Backend for PowerSupplyBackend {
    fn name(&self) -> &'static str {
        "power_supply"
    }

    fn enumerate(&mut self) -> anyhow::Result<Vec<(Device, Vec<Channel>)>> {
        self.supplies.clear();
        self.mains = None;
        let class = self.root.sys("class/power_supply");
        let mut found = Vec::new();
        for name in list_dir(&class) {
            let Some(dir) = self.root.resolve(&class.join(&name)) else {
                continue;
            };
            match read_string(&dir.join("type")).as_deref() {
                Some("Mains") if self.mains.is_none() => self.mains = Some(dir),
                Some(kind @ ("Battery" | "UPS")) => {
                    let is_ups = kind == "UPS";
                    found.push((name, dir, is_ups));
                }
                _ => {}
            }
        }

        let mut result = Vec::new();
        for (index, (name, dir, is_ups)) in found.into_iter().enumerate() {
            if read_string(&dir.join("present")).as_deref() == Some("0") {
                continue;
            }
            let model = squash_spaces(&format!(
                "{} {}",
                read_string(&dir.join("manufacturer")).unwrap_or_default(),
                read_string(&dir.join("model_name")).unwrap_or_default()
            ));
            let model = if model.is_empty() {
                name.clone()
            } else {
                model
            };
            let (category, prefix) = if is_ups {
                (Category::Ups, "UPS")
            } else {
                (Category::Battery, "Battery")
            };
            // Peripheral batteries (a wireless mouse) sort after the system's own.
            let peripheral = read_string(&dir.join("scope")).as_deref() == Some("Device");
            let order = u16::try_from(index).unwrap_or(0) + if peripheral { 100 } else { 0 };
            let device = Device::new(
                format!("power_supply:{}", key_part(&name)),
                format!("{prefix}: {model}"),
                category,
            )
            .with_order(order);
            let id = device.id.clone();

            let energy = has(&dir, "energy_now") || has(&dir, "charge_now");
            let full = has(&dir, "energy_full") || has(&dir, "charge_full");
            let design = has(&dir, "energy_full_design") || has(&dir, "charge_full_design");
            let candidates: [(bool, &str, &str, Kind, Unit, Row); 12] = [
                (
                    has(&dir, "capacity"),
                    "charge_level",
                    "Charge Level",
                    Kind::Usage,
                    Unit::Percent,
                    Row::ChargeLevel,
                ),
                (
                    has(&dir, "voltage_now"),
                    "voltage",
                    "Voltage",
                    Kind::Voltage,
                    Unit::Volt,
                    Row::Voltage,
                ),
                (
                    has(&dir, "current_now"),
                    "current",
                    "Current",
                    Kind::Current,
                    Unit::Ampere,
                    Row::Current,
                ),
                (
                    has(&dir, "power_now")
                        || (has(&dir, "voltage_now") && has(&dir, "current_now")),
                    "charge_rate",
                    "Charge Rate",
                    Kind::Power,
                    Unit::Watt,
                    Row::ChargeRate,
                ),
                (
                    energy,
                    "remaining",
                    "Remaining Capacity",
                    Kind::Energy,
                    Unit::WattHour,
                    Row::Remaining,
                ),
                (
                    full,
                    "full_capacity",
                    "Full Charge Capacity",
                    Kind::Energy,
                    Unit::WattHour,
                    Row::FullCapacity,
                ),
                (
                    full && design,
                    "wear",
                    "Wear Level",
                    Kind::Usage,
                    Unit::Percent,
                    Row::Wear,
                ),
                (
                    has(&dir, "cycle_count"),
                    "cycle_count",
                    "Cycle Count",
                    Kind::Count,
                    Unit::Count,
                    Row::Cycles,
                ),
                (
                    has(&dir, "temp"),
                    "temperature",
                    "Temperature",
                    Kind::Temperature,
                    Unit::Celsius,
                    Row::Temperature,
                ),
                (
                    has(&dir, "status"),
                    "charging",
                    "Charging",
                    Kind::Boolean,
                    Unit::Bool,
                    Row::Charging,
                ),
                (
                    has(&dir, "status"),
                    "discharging",
                    "Discharging",
                    Kind::Boolean,
                    Unit::Bool,
                    Row::Discharging,
                ),
                (
                    self.mains.is_some() && !peripheral,
                    "ac_power",
                    "AC Power",
                    Kind::Boolean,
                    Unit::Bool,
                    Row::AcPower,
                ),
            ];
            let mut channels = Vec::new();
            let mut rows = Vec::new();
            for (present, key, label, kind, unit, row) in candidates {
                if !present {
                    continue;
                }
                let channel = Channel::new(&id, key, label, kind).with_unit(unit);
                rows.push((channel.id.clone(), row));
                channels.push(channel);
            }
            self.supplies.push(SupplyPlan { dir, rows });
            result.push((device, channels));
        }
        Ok(result)
    }

    fn sample(&mut self, out: &mut Vec<Sample>) -> anyhow::Result<()> {
        let ac_online = self
            .mains
            .as_ref()
            .and_then(|dir| read_string(&dir.join("online")))
            .map(|v| v == "1");
        for supply in &self.supplies {
            let dir = &supply.dir;
            let status = read_string(&dir.join("status"));
            // Drivers disagree on the sign of current and power; take it from the status.
            let sign = if status.as_deref() == Some("Discharging") {
                -1.0
            } else {
                1.0
            };
            for (channel, row) in &supply.rows {
                if self.disabled.contains(channel) {
                    continue;
                }
                let value = match row {
                    Row::ChargeLevel => read_i64(&dir.join("capacity")).map(|v| v as f64),
                    Row::Voltage => micro(dir, "voltage_now"),
                    Row::Current => micro(dir, "current_now").map(|a| sign * a.abs()),
                    Row::ChargeRate => micro(dir, "power_now")
                        .or_else(|| Some(micro(dir, "voltage_now")? * micro(dir, "current_now")?))
                        .map(|w| sign * w.abs()),
                    Row::Remaining => energy_wh(dir, "now"),
                    Row::FullCapacity => energy_wh(dir, "full"),
                    Row::Wear => energy_wh(dir, "full")
                        .zip(energy_wh(dir, "full_design"))
                        .filter(|(_, design)| *design > 0.0)
                        .map(|(full, design)| ((1.0 - full / design) * 100.0).max(0.0)),
                    Row::Cycles => read_i64(&dir.join("cycle_count")).map(|v| v as f64),
                    Row::Temperature => read_i64(&dir.join("temp")).map(|v| v as f64 / 10.0),
                    Row::Charging => status
                        .as_ref()
                        .map(|s| f64::from(u8::from(s == "Charging"))),
                    Row::Discharging => status
                        .as_ref()
                        .map(|s| f64::from(u8::from(s == "Discharging"))),
                    Row::AcPower => ac_online.map(|on| f64::from(u8::from(on))),
                };
                out.push(Sample::new(channel.clone(), value));
            }
        }
        Ok(())
    }

    fn set_disabled(&mut self, disabled: &HashSet<ChannelId>) {
        self.disabled.clone_from(disabled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::test_support::*;

    #[test]
    fn battery_rows_and_values() {
        let mut backend = PowerSupplyBackend::new(fixture_root());
        let (devices, samples) = enumerate_and_sample(&mut backend);
        assert_eq!(devices.len(), 1, "the AC adapter has no group of its own");
        let (device, channels) = device(&devices, "power_supply:BAT0");
        assert_eq!(device.label, "Battery: SMP 5B10W13930");
        assert_eq!(device.category, Category::Battery);
        assert_eq!(
            keys_and_labels(channels),
            [
                ("charge_level", "Charge Level"),
                ("voltage", "Voltage"),
                ("charge_rate", "Charge Rate"),
                ("remaining", "Remaining Capacity"),
                ("full_capacity", "Full Charge Capacity"),
                ("wear", "Wear Level"),
                ("cycle_count", "Cycle Count"),
                ("charging", "Charging"),
                ("discharging", "Discharging"),
                ("ac_power", "AC Power"),
            ]
        );
        assert_eq!(channel(channels, "remaining").unit, Unit::WattHour);
        let values = sample_map(&samples);
        let get = |key: &str| values[&ChannelId::new(&device.id, key)];
        assert_close(get("charge_level"), 87.0);
        assert_close(get("voltage"), 12.15);
        assert_close(get("charge_rate"), -8.64);
        assert_close(get("remaining"), 43.21);
        assert_close(get("full_capacity"), 49.68);
        assert_close(get("wear"), (1.0 - 49.68 / 57.0) * 100.0);
        assert_close(get("cycle_count"), 123.0);
        assert_close(get("charging"), 0.0);
        assert_close(get("discharging"), 1.0);
        assert_close(get("ac_power"), 0.0);
    }

    #[test]
    fn charge_based_batteries_convert_to_watt_hours() {
        let (_dir, root) = fixture_copy();
        let bat = "sys/devices/LNXSYSTM:00/LNXSYBUS:00/PNP0C0A:00/power_supply/BAT0";
        for file in [
            "energy_now",
            "energy_full",
            "energy_full_design",
            "power_now",
        ] {
            std::fs::remove_file(root.path(format!("{bat}/{file}"))).unwrap();
        }
        write(&root, &format!("{bat}/charge_now"), "3750000\n");
        write(&root, &format!("{bat}/charge_full"), "4000000\n");
        write(&root, &format!("{bat}/charge_full_design"), "5000000\n");
        write(&root, &format!("{bat}/current_now"), "-500000\n");
        write(&root, &format!("{bat}/status"), "Charging\n");
        let mut backend = PowerSupplyBackend::new(root);
        let (devices, samples) = enumerate_and_sample(&mut backend);
        let id = &devices[0].0.id;
        let values = sample_map(&samples);
        let get = |key: &str| values[&ChannelId::new(id, key)];
        // 3.75 Ah at the 11.52 V design voltage.
        assert_close(get("remaining"), 3.75 * 11.52);
        assert_close(get("wear"), 20.0);
        // Charging: positive whatever sign the driver used.
        assert_close(get("current"), 0.5);
        assert_close(get("charge_rate"), 12.15 * 0.5);
        assert_close(get("charging"), 1.0);
    }
}

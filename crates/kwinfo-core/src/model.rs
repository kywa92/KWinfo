//! The sensor data model shared by every backend and every front end.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Identifies a device (a sensor chip, a CPU, a GPU, a drive, ...) stably across reboots.
///
/// Backends build it from the driver name and the device's bus path, never from a kernel
/// enumeration number such as `hwmon3`, so that per-sensor user settings stay attached to
/// the right hardware. Examples: `cpu:0`, `hwmon:nct6775:nct6775.656`, `drm:0000:00:02.0`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(Arc<str>);

impl DeviceId {
    pub fn new(id: impl AsRef<str>) -> Self {
        Self(Arc::from(id.as_ref()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for DeviceId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

/// Identifies one reading of a device: `<device id>/<channel key>`.
///
/// The channel key is the backend's own stable name for the reading, for example the
/// hwmon attribute (`temp1_input`) or a synthetic key (`usage.core3`). Keys must not
/// contain `/`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChannelId(Arc<str>);

impl ChannelId {
    pub fn new(device: &DeviceId, key: impl AsRef<str>) -> Self {
        debug_assert!(
            !key.as_ref().contains('/'),
            "channel keys must not contain '/'"
        );
        Self(Arc::from(
            format!("{}/{}", device.as_str(), key.as_ref()).as_str(),
        ))
    }

    /// Wraps an already formatted `<device>/<key>` string, for example from a config file.
    pub fn from_raw(raw: impl AsRef<str>) -> Self {
        Self(Arc::from(raw.as_ref()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The device part of the id.
    pub fn device(&self) -> &str {
        self.0
            .rsplit_once('/')
            .map_or(&*self.0, |(device, _)| device)
    }

    /// The channel key part of the id.
    pub fn key(&self) -> &str {
        self.0.rsplit_once('/').map_or("", |(_, key)| key)
    }
}

impl fmt::Display for ChannelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a channel measures. Drives the row icon, the default unit and decimals, and the
/// default graph colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    Temperature,
    Voltage,
    Current,
    Power,
    Energy,
    Fan,
    Pwm,
    Clock,
    Usage,
    Throughput,
    Capacity,
    Count,
    Boolean,
    Other,
}

impl Kind {
    pub fn default_unit(self) -> Unit {
        match self {
            Kind::Temperature => Unit::Celsius,
            Kind::Voltage => Unit::Volt,
            Kind::Current => Unit::Ampere,
            Kind::Power => Unit::Watt,
            Kind::Energy => Unit::Joule,
            Kind::Fan => Unit::Rpm,
            Kind::Pwm | Kind::Usage => Unit::Percent,
            Kind::Clock => Unit::MegaHertz,
            Kind::Throughput => Unit::BytesPerSecond,
            Kind::Capacity => Unit::Bytes,
            Kind::Count => Unit::Count,
            Kind::Boolean => Unit::Bool,
            Kind::Other => Unit::None,
        }
    }

    /// Decimal places HWiNFO users expect for this kind of reading.
    pub fn default_decimals(self) -> u8 {
        match self {
            Kind::Temperature | Kind::Clock | Kind::Usage | Kind::Pwm => 1,
            Kind::Voltage | Kind::Current | Kind::Power | Kind::Energy => 3,
            Kind::Fan | Kind::Count | Kind::Boolean | Kind::Other => 0,
            Kind::Throughput | Kind::Capacity => 1,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Temperature => "Temperature",
            Kind::Voltage => "Voltage",
            Kind::Current => "Current",
            Kind::Power => "Power",
            Kind::Energy => "Energy",
            Kind::Fan => "Fan",
            Kind::Pwm => "PWM",
            Kind::Clock => "Clock",
            Kind::Usage => "Usage",
            Kind::Throughput => "Throughput",
            Kind::Capacity => "Capacity",
            Kind::Count => "Count",
            Kind::Boolean => "Boolean",
            Kind::Other => "Other",
        }
    }
}

/// The unit a channel's values are expressed in. Values are always stored in the base
/// unit (volts, watts, bytes, ...); display scaling happens in [`crate::format`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Unit {
    Celsius,
    Volt,
    Ampere,
    Watt,
    Joule,
    Rpm,
    Percent,
    MegaHertz,
    BytesPerSecond,
    Bytes,
    Count,
    Bool,
    None,
}

impl Unit {
    pub fn symbol(self) -> &'static str {
        match self {
            Unit::Celsius => "°C",
            Unit::Volt => "V",
            Unit::Ampere => "A",
            Unit::Watt => "W",
            Unit::Joule => "J",
            Unit::Rpm => "RPM",
            Unit::Percent => "%",
            Unit::MegaHertz => "MHz",
            Unit::BytesPerSecond => "B/s",
            Unit::Bytes => "B",
            Unit::Count | Unit::Bool | Unit::None => "",
        }
    }
}

/// The group a device is shown under. Also fixes the order of groups in the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Category {
    Cpu,
    Motherboard,
    Memory,
    Gpu,
    Drive,
    Smart,
    Network,
    Battery,
    Ups,
    Psu,
    Cooler,
    Other,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::Cpu => "CPU",
            Category::Motherboard => "Motherboard",
            Category::Memory => "Memory",
            Category::Gpu => "GPU",
            Category::Drive => "Drive",
            Category::Smart => "S.M.A.R.T.",
            Category::Network => "Network",
            Category::Battery => "Battery",
            Category::Ups => "UPS",
            Category::Psu => "PSU",
            Category::Cooler => "Cooler",
            Category::Other => "Other",
        }
    }
}

/// Where a reading comes from. Helper-sourced channels are absent when the privileged
/// helper is not running, and the UI can say so.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Source {
    Unprivileged,
    Helper,
    Mock,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: DeviceId,
    /// Display name, e.g. `CPU [#0]: Intel Core i7-8700K`.
    pub label: String,
    pub category: Category,
    /// Order among devices of the same category.
    pub order: u16,
}

impl Device {
    pub fn new(id: impl AsRef<str>, label: impl Into<String>, category: Category) -> Self {
        Self {
            id: DeviceId::new(id),
            label: label.into(),
            category,
            order: 0,
        }
    }

    pub fn with_order(mut self, order: u16) -> Self {
        self.order = order;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub id: ChannelId,
    pub device: DeviceId,
    /// Display name, e.g. `Core #0` or `CPU Package Power`.
    pub label: String,
    pub kind: Kind,
    pub unit: Unit,
    pub decimals: u8,
    pub source: Source,
}

impl Channel {
    /// Creates a channel with the unit and decimals that are conventional for `kind`.
    pub fn new(device: &DeviceId, key: &str, label: impl Into<String>, kind: Kind) -> Self {
        Self {
            id: ChannelId::new(device, key),
            device: device.clone(),
            label: label.into(),
            kind,
            unit: kind.default_unit(),
            decimals: kind.default_decimals(),
            source: Source::Unprivileged,
        }
    }

    pub fn with_unit(mut self, unit: Unit) -> Self {
        self.unit = unit;
        self
    }

    pub fn with_decimals(mut self, decimals: u8) -> Self {
        self.decimals = decimals;
        self
    }

    pub fn with_source(mut self, source: Source) -> Self {
        self.source = source;
        self
    }
}

/// One reading produced by a backend during a sampling tick. `None` means the channel
/// exists but could not be read this tick. All samples of a tick share the tick's time.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub channel: ChannelId,
    pub value: Option<f64>,
}

impl Sample {
    pub fn new(channel: ChannelId, value: impl Into<Option<f64>>) -> Self {
        Self {
            channel,
            value: value.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_id_splits_into_device_and_key() {
        let device = DeviceId::new("hwmon:coretemp:coretemp.0");
        let id = ChannelId::new(&device, "temp1_input");
        assert_eq!(id.as_str(), "hwmon:coretemp:coretemp.0/temp1_input");
        assert_eq!(id.device(), "hwmon:coretemp:coretemp.0");
        assert_eq!(id.key(), "temp1_input");
    }

    #[test]
    fn ids_round_trip_through_serde() {
        let device = DeviceId::new("cpu:0");
        let id = ChannelId::new(&device, "package_power");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"cpu:0/package_power\"");
        let back: ChannelId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn channel_defaults_follow_kind() {
        let device = DeviceId::new("cpu:0");
        let channel = Channel::new(&device, "vcore", "Vcore", Kind::Voltage);
        assert_eq!(channel.unit, Unit::Volt);
        assert_eq!(channel.decimals, 3);
        assert_eq!(channel.source, Source::Unprivileged);
    }
}

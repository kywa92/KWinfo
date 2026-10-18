//! Messages exchanged between the GUI and the privileged helper (`kwinfo-helper`).
//!
//! Both transports, the D-Bus system service and the `--stdio` pipe used from the
//! AppImage, carry these types as JSON, so the protocol is defined exactly once here.
//! Milestone M4 implements the helper; M0 only fixes the vocabulary.

use kwinfo_core::{Channel, ChannelId, Device};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

pub const DBUS_NAME: &str = "io.github.kylanwalters.KWinfo.Helper";
pub const DBUS_PATH: &str = "/io/github/kylanwalters/KWinfo/Helper";
pub const DBUS_INTERFACE: &str = "io.github.kylanwalters.KWinfo.Helper1";
pub const POLKIT_ACTION: &str = "io.github.kylanwalters.KWinfo.read-sensors";

/// Which privileged data sources the helper could open on this machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Energy counters under `/sys/class/powercap`.
    pub rapl: bool,
    /// Model-specific registers through `/dev/cpu/*/msr`.
    pub msr: bool,
    /// `smartctl` is installed and drives answer.
    pub smart: bool,
    /// The i915/xe performance counters.
    pub gpu_pmu: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Hello {
        protocol: u32,
    },
    Capabilities,
    Enumerate,
    Subscribe {
        interval_ms: u32,
        smart_interval_s: u32,
    },
    Unsubscribe,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Hello {
        protocol: u32,
        version: String,
    },
    Capabilities(Capabilities),
    Enumerate {
        devices: Vec<DeviceEntry>,
    },
    Samples {
        tick_ms: u64,
        samples: Vec<SampleEntry>,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceEntry {
    pub device: Device,
    pub channels: Vec<Channel>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SampleEntry {
    pub channel: ChannelId,
    pub value: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use kwinfo_core::{Category, Kind, Source};

    #[test]
    fn messages_round_trip_as_json() {
        let device = Device::new("cpu:0", "CPU [#0]: Intel Core i7-8700K", Category::Cpu);
        let channel = Channel::new(
            &device.id,
            "power.package",
            "CPU Package Power",
            Kind::Power,
        )
        .with_source(Source::Helper);
        let response = Response::Enumerate {
            devices: vec![DeviceEntry {
                device,
                channels: vec![channel],
            }],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"type\":\"enumerate\""));
        let back: Response = serde_json::from_str(&json).unwrap();
        assert_eq!(back, response);

        let request = Request::Subscribe {
            interval_ms: 1000,
            smart_interval_s: 60,
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            json,
            r#"{"type":"subscribe","interval_ms":1000,"smart_interval_s":60}"#
        );
    }
}

//! Talking to the privileged helper (`kwinfo-helper`): the protocol, defined once for
//! both transports, and the client that the application and the command line tool use.
//!
//! The helper reads what only root may read (RAPL energy counters, model-specific
//! registers, S.M.A.R.T. data, GPU performance counters) and answers four requests:
//! [`Request::Hello`], [`Request::Capabilities`], [`Request::Enumerate`] and
//! [`Request::Sample`]. They travel as JSON, one message per line over the `stdio`
//! transport (the helper started through `pkexec` for one session), or as the string
//! result of the D-Bus method of the same name on the system service.
//!
//! The client pulls readings (one `Sample` per sampling tick) rather than subscribing to
//! signals: D-Bus signals on the system bus can be received by every local user, which
//! would hand out the power readings the kernel restricts to root.

pub mod client;
pub mod framing;

use kwinfo_core::{Channel, ChannelId, Device};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

pub const DBUS_NAME: &str = "io.github.kywa92.KWinfo.Helper";
pub const DBUS_PATH: &str = "/io/github/kywa92/KWinfo/Helper";
pub const DBUS_INTERFACE: &str = "io.github.kywa92.KWinfo.Helper1";
pub const POLKIT_ACTION: &str = "io.github.kywa92.KWinfo.read-sensors";

/// Where a local install (`kwinfo-helper install`) puts the helper. Packages use
/// `/usr/libexec/kwinfo/kwinfo-helper` instead.
pub const LOCAL_HELPER_PATH: &str = "/usr/local/libexec/kwinfo/kwinfo-helper";
/// The list of files a local install wrote, which `uninstall` removes.
pub const LOCAL_MANIFEST_PATH: &str = "/usr/local/libexec/kwinfo/installed-files";
/// Where packages install the helper.
pub const PACKAGED_HELPER_PATH: &str = "/usr/libexec/kwinfo/kwinfo-helper";

/// Which privileged sources the helper could open on this machine, and why the others
/// are missing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Energy counters under `/sys/class/powercap` (package, core, GT, DRAM power).
    pub rapl: bool,
    /// Model-specific registers through `/dev/cpu/*/msr` (VID, limits, package C-states).
    pub msr: bool,
    /// `smartctl` is installed and at least one drive answered.
    pub smart: bool,
    /// The i915 performance counters (GPU engine load, RC6).
    pub gpu_pmu: bool,
    /// One sentence per missing source on what to do about it, e.g. loading `msr`.
    #[serde(default)]
    pub notes: Vec<String>,
}

impl Capabilities {
    /// `"RAPL, MSR, S.M.A.R.T."`: the sources that work, for status lines.
    pub fn summary(&self) -> String {
        let names: Vec<&str> = [
            (self.rapl, "RAPL power"),
            (self.msr, "MSRs"),
            (self.smart, "S.M.A.R.T."),
            (self.gpu_pmu, "GPU load"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
        if names.is_empty() {
            "nothing".to_string()
        } else {
            names.join(", ")
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// The first request on a connection; the helper refuses other protocol versions.
    Hello {
        protocol: u32,
    },
    Capabilities,
    /// The devices and channels the helper provides.
    Enumerate,
    /// The current readings. The helper reads its sources at most every 250 ms, and
    /// answers more frequent requests with the previous readings.
    Sample,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Hello { protocol: u32, version: String },
    Capabilities(Capabilities),
    Enumerate { devices: Vec<DeviceEntry> },
    Samples { samples: Vec<SampleEntry> },
    Error { message: String },
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
        let channel = Channel::new(&device.id, "rapl.package", "CPU Package Power", Kind::Power)
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

        assert_eq!(
            serde_json::to_string(&Request::Sample).unwrap(),
            r#"{"type":"sample"}"#
        );
        assert_eq!(
            serde_json::to_string(&Request::Hello { protocol: 1 }).unwrap(),
            r#"{"type":"hello","protocol":1}"#
        );
    }

    #[test]
    fn capabilities_without_notes_still_parse() {
        let caps: Capabilities =
            serde_json::from_str(r#"{"rapl":true,"msr":false,"smart":true,"gpu_pmu":false}"#)
                .unwrap();
        assert!(caps.notes.is_empty());
        assert_eq!(caps.summary(), "RAPL power, S.M.A.R.T.");
        assert_eq!(Capabilities::default().summary(), "nothing");
    }
}

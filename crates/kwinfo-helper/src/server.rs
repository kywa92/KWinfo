//! Answering requests, whatever the transport: [`Helper::handle`] turns a
//! [`Request`] into a [`Response`].

use std::time::{Duration, Instant};

use kwinfo_core::{Channel, Device};
use kwinfo_ipc::{Capabilities, DeviceEntry, PROTOCOL_VERSION, Request, Response, SampleEntry};
use tracing::{debug, info};

use crate::providers::{self, Provider, Sources};

/// Requests closer together than this get the previous readings: every reading is an
/// average over at least this long (see `providers::rapl::MIN_WINDOW`).
pub const MIN_SAMPLE_INTERVAL: Duration = Duration::from_millis(250);

struct Slot {
    provider: Box<dyn Provider>,
    devices: Vec<(Device, Vec<Channel>)>,
    scanned: Option<Instant>,
}

/// The helper's state: its providers, what they found, and the last readings.
pub struct Helper {
    slots: Vec<Slot>,
    last: Option<(Instant, Vec<SampleEntry>)>,
    clock: Box<dyn Fn() -> Instant + Send>,
}

impl Helper {
    pub fn new(providers: Vec<Box<dyn Provider>>) -> Self {
        Self {
            slots: providers
                .into_iter()
                .map(|provider| Slot {
                    provider,
                    devices: Vec::new(),
                    scanned: None,
                })
                .collect(),
            last: None,
            clock: Box::new(Instant::now),
        }
    }

    /// Every provider, reading `sources`.
    pub fn with_sources(sources: &Sources) -> Self {
        Self::new(providers::all(sources))
    }

    /// Replaces the clock, for tests.
    pub fn with_clock(mut self, clock: impl Fn() -> Instant + Send + 'static) -> Self {
        self.clock = Box::new(clock);
        self
    }

    pub fn handle(&mut self, request: &Request) -> Response {
        let now = (self.clock)();
        match request {
            Request::Hello { protocol } if *protocol == PROTOCOL_VERSION => Response::Hello {
                protocol: PROTOCOL_VERSION,
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            Request::Hello { protocol } => Response::Error {
                message: format!(
                    "this helper speaks protocol {PROTOCOL_VERSION} and KWinfo speaks {protocol}; \
                     install the helper of the same KWinfo version"
                ),
            },
            Request::Capabilities => {
                self.scan_due(now);
                Response::Capabilities(self.capabilities())
            }
            Request::Enumerate => {
                self.scan_due(now);
                Response::Enumerate {
                    devices: self.devices(),
                }
            }
            Request::Sample => {
                self.scan_due(now);
                Response::Samples {
                    samples: self.sample(now),
                }
            }
        }
    }

    /// Re-enumerates every provider whose rescan is due.
    fn scan_due(&mut self, now: Instant) {
        for slot in &mut self.slots {
            let due = slot.scanned.is_none_or(|then| {
                now.saturating_duration_since(then) >= slot.provider.rescan_every()
            });
            if !due {
                continue;
            }
            slot.devices = slot.provider.enumerate();
            slot.scanned = Some(now);
            let channels: usize = slot.devices.iter().map(|(_, c)| c.len()).sum();
            let status = slot.provider.status();
            if status.available {
                info!(provider = slot.provider.name(), channels, note = ?status.note, "provider ready");
            } else {
                info!(provider = slot.provider.name(), note = ?status.note, "provider unavailable");
            }
            // New channels mean new readings: the next sample is taken fresh.
            self.last = None;
        }
    }

    /// The providers' devices, merged by id in the order the providers found them: RAPL
    /// and MSR rows share the CPU package device.
    fn devices(&self) -> Vec<DeviceEntry> {
        let mut out: Vec<DeviceEntry> = Vec::new();
        for (device, channels) in self.slots.iter().flat_map(|s| s.devices.iter()) {
            match out.iter_mut().find(|e| e.device.id == device.id) {
                Some(entry) => entry.channels.extend(channels.iter().cloned()),
                None => out.push(DeviceEntry {
                    device: device.clone(),
                    channels: channels.clone(),
                }),
            }
        }
        out
    }

    fn sample(&mut self, now: Instant) -> Vec<SampleEntry> {
        if let Some((then, samples)) = &self.last
            && now.saturating_duration_since(*then) < MIN_SAMPLE_INTERVAL
        {
            debug!("sample requested again within {MIN_SAMPLE_INTERVAL:?}; repeating the last");
            return samples.clone();
        }
        let mut raw = Vec::new();
        for slot in &mut self.slots {
            slot.provider.sample(now, &mut raw);
        }
        let samples: Vec<SampleEntry> = raw
            .into_iter()
            .map(|s| SampleEntry {
                channel: s.channel,
                value: s.value,
            })
            .collect();
        self.last = Some((now, samples.clone()));
        samples
    }

    fn capabilities(&self) -> Capabilities {
        let mut caps = Capabilities::default();
        for slot in &self.slots {
            let status = slot.provider.status();
            let flag = match slot.provider.name() {
                "rapl" => &mut caps.rapl,
                "msr" => &mut caps.msr,
                "smart" => &mut caps.smart,
                "gpu_pmu" => &mut caps.gpu_pmu,
                _ => continue,
            };
            *flag = status.available;
            caps.notes.extend(status.note);
        }
        caps
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::Status;
    use kwinfo_core::{Category, ChannelId, Kind, Sample, Source};
    use std::sync::{Arc, Mutex};

    /// A provider counting its enumerations and samples.
    struct Counting {
        name: &'static str,
        device: &'static str,
        key: &'static str,
        samples: Arc<Mutex<u32>>,
        scans: Arc<Mutex<u32>>,
        status: Status,
    }

    impl Provider for Counting {
        fn name(&self) -> &'static str {
            self.name
        }

        fn enumerate(&mut self) -> Vec<(Device, Vec<Channel>)> {
            *self.scans.lock().unwrap() += 1;
            let device = Device::new(self.device, "CPU [#0]: Test", Category::Cpu);
            let channel = Channel::new(&device.id, self.key, self.key, Kind::Power)
                .with_source(Source::Helper);
            vec![(device, vec![channel])]
        }

        fn sample(&mut self, _: Instant, out: &mut Vec<Sample>) {
            let mut n = self.samples.lock().unwrap();
            *n += 1;
            out.push(Sample::new(
                ChannelId::from_raw(format!("{}/{}", self.device, self.key)),
                f64::from(*n),
            ));
        }

        fn status(&self) -> Status {
            self.status.clone()
        }
    }

    fn helper(now: Arc<Mutex<Instant>>) -> (Helper, Arc<Mutex<u32>>, Arc<Mutex<u32>>) {
        let samples = Arc::new(Mutex::new(0));
        let scans = Arc::new(Mutex::new(0));
        let rapl = Counting {
            name: "rapl",
            device: "cpu:0",
            key: "rapl.package",
            samples: samples.clone(),
            scans: scans.clone(),
            status: Status::working(),
        };
        let msr = Counting {
            name: "msr",
            device: "cpu:0",
            key: "msr.vid.core0",
            samples: Arc::new(Mutex::new(0)),
            scans: Arc::new(Mutex::new(0)),
            status: Status::missing("The msr kernel module is not loaded"),
        };
        let helper = Helper::new(vec![Box::new(rapl), Box::new(msr)])
            .with_clock(move || *now.lock().unwrap());
        (helper, samples, scans)
    }

    #[test]
    fn hello_checks_the_protocol_version() {
        let (mut helper, _, _) = helper(Arc::new(Mutex::new(Instant::now())));
        assert!(matches!(
            helper.handle(&Request::Hello {
                protocol: PROTOCOL_VERSION
            }),
            Response::Hello {
                protocol: PROTOCOL_VERSION,
                ..
            }
        ));
        let Response::Error { message } = helper.handle(&Request::Hello { protocol: 99 }) else {
            panic!("a newer client is refused");
        };
        assert!(message.contains("protocol 1"));
    }

    #[test]
    fn devices_merge_and_capabilities_explain_what_is_missing() {
        let (mut helper, _, _) = helper(Arc::new(Mutex::new(Instant::now())));
        let Response::Enumerate { devices } = helper.handle(&Request::Enumerate) else {
            panic!();
        };
        assert_eq!(devices.len(), 1, "both providers' rows join cpu:0");
        assert_eq!(devices[0].channels.len(), 2);
        let Response::Capabilities(caps) = helper.handle(&Request::Capabilities) else {
            panic!();
        };
        assert!(caps.rapl);
        assert!(!caps.msr);
        assert_eq!(caps.notes, ["The msr kernel module is not loaded"]);
    }

    #[test]
    fn samples_are_rate_limited_and_providers_rescanned_on_schedule() {
        let now = Arc::new(Mutex::new(Instant::now()));
        let (mut helper, samples, scans) = helper(now.clone());
        let value = |response: Response| match response {
            Response::Samples { samples } => samples[0].value,
            other => panic!("{other:?}"),
        };
        assert_eq!(value(helper.handle(&Request::Sample)), Some(1.0));
        *now.lock().unwrap() += Duration::from_millis(100);
        assert_eq!(
            value(helper.handle(&Request::Sample)),
            Some(1.0),
            "repeated within 250 ms"
        );
        *now.lock().unwrap() += Duration::from_millis(200);
        assert_eq!(value(helper.handle(&Request::Sample)), Some(2.0));
        assert_eq!(*samples.lock().unwrap(), 2);
        assert_eq!(*scans.lock().unwrap(), 1);
        *now.lock().unwrap() += Duration::from_secs(601);
        helper.handle(&Request::Sample);
        assert_eq!(*scans.lock().unwrap(), 2, "rescanned after ten minutes");
    }
}

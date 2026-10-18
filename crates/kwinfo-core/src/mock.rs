//! A synthetic backend: realistic-looking devices whose values drift smoothly.
//!
//! It runs on any operating system, which is how the GUI is developed on macOS, and it
//! is deterministic for a given seed and sample time, which is how snapshot tests get
//! stable pictures.

use std::f64::consts::TAU;
use std::time::{Duration, Instant};

use crate::backend::Backend;
use crate::model::{Category, Channel, ChannelId, Device, DeviceId, Kind, Sample, Source};

const MIB: f64 = 1024.0 * 1024.0;
const GIB: f64 = 1024.0 * MIB;

/// xorshift64*: tiny, deterministic, good enough for noise.
#[derive(Clone, Debug)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Parameters of a smoothly varying analog signal.
#[derive(Clone, Copy, Debug)]
struct Analog {
    base: f64,
    amplitude: f64,
    period: f64,
    jitter: f64,
    min: f64,
    max: f64,
}

#[derive(Clone, Debug)]
enum Shape {
    /// Base plus two sines, a slow random walk and jitter, clamped to a range.
    Analog { cfg: Analog, phase: f64, walk: f64 },
    /// Mostly constant, flips rarely.
    Boolean { value: bool, flip_chance: f64 },
    /// Grows steadily, like a byte counter.
    Monotonic { base: f64, rate: f64 },
}

#[derive(Clone, Debug)]
struct Signal {
    channel: ChannelId,
    shape: Shape,
}

impl Signal {
    fn value(&mut self, t: f64, rng: &mut Rng) -> f64 {
        match &mut self.shape {
            Shape::Analog { cfg, phase, walk } => {
                *walk += (rng.unit() - 0.5) * cfg.amplitude * 0.05;
                *walk = walk.clamp(-cfg.amplitude * 0.5, cfg.amplitude * 0.5);
                let slow = (TAU * t / cfg.period + *phase).sin();
                let fast = (TAU * t / (cfg.period * 0.23) + *phase * 2.0).sin();
                let v = cfg.base
                    + cfg.amplitude * (0.7 * slow + 0.3 * fast)
                    + *walk
                    + (rng.unit() - 0.5) * cfg.jitter;
                v.clamp(cfg.min, cfg.max)
            }
            Shape::Boolean { value, flip_chance } => {
                if rng.unit() < *flip_chance {
                    *value = !*value;
                }
                if *value { 1.0 } else { 0.0 }
            }
            Shape::Monotonic { base, rate } => *base + *rate * t,
        }
    }
}

struct Builder {
    rng: Rng,
    devices: Vec<(Device, Vec<Channel>)>,
    signals: Vec<Signal>,
}

impl Builder {
    fn device(&mut self, device: Device) -> usize {
        self.devices.push((device, Vec::new()));
        self.devices.len() - 1
    }

    fn add(&mut self, device: usize, channel: Channel, shape: Shape) {
        self.signals.push(Signal {
            channel: channel.id.clone(),
            shape,
        });
        self.devices[device].1.push(channel);
    }

    fn analog(&mut self, device: usize, channel: Channel, cfg: Analog) {
        let phase = self.rng.unit() * TAU;
        self.add(
            device,
            channel,
            Shape::Analog {
                cfg,
                phase,
                walk: 0.0,
            },
        );
    }

    fn boolean(&mut self, device: usize, channel: Channel, value: bool, flip_chance: f64) {
        self.add(device, channel, Shape::Boolean { value, flip_chance });
    }

    fn monotonic(&mut self, device: usize, channel: Channel, base: f64, rate: f64) {
        self.add(device, channel, Shape::Monotonic { base, rate });
    }

    fn cpu(&mut self) {
        let id = DeviceId::new("cpu:mock");
        let d = self.device(Device::new(
            id.as_str(),
            "CPU [#0]: Mock Hexa-Core 6000X",
            Category::Cpu,
        ));
        let temp = |base: f64, amplitude: f64, period: f64| Analog {
            base,
            amplitude,
            period,
            jitter: 0.5,
            min: 20.0,
            max: 100.0,
        };
        self.analog(
            d,
            Channel::new(&id, "temp.package", "CPU Package", Kind::Temperature),
            temp(48.0, 8.0, 90.0),
        );
        for core in 0..6 {
            let key = format!("temp.core{core}");
            let channel = Channel::new(&id, &key, format!("Core #{core}"), Kind::Temperature);
            self.analog(
                d,
                channel,
                temp(
                    45.0 + f64::from(core) * 0.7,
                    7.0,
                    60.0 + f64::from(core) * 9.0,
                ),
            );
        }
        self.analog(
            d,
            Channel::new(&id, "temp.coremax", "Core Max", Kind::Temperature),
            temp(52.0, 8.0, 70.0),
        );
        self.analog(
            d,
            Channel::new(
                &id,
                "temp.tjmax_distance",
                "Distance to TjMAX",
                Kind::Temperature,
            ),
            temp(48.0, 8.0, 70.0),
        );

        let usage = |base: f64, amplitude: f64, period: f64| Analog {
            base,
            amplitude,
            period,
            jitter: 2.0,
            min: 0.0,
            max: 100.0,
        };
        self.analog(
            d,
            Channel::new(&id, "usage.total", "Total CPU Usage", Kind::Usage),
            usage(18.0, 15.0, 45.0),
        );
        self.analog(
            d,
            Channel::new(&id, "usage.max", "Max CPU/Thread Usage", Kind::Usage),
            usage(45.0, 30.0, 25.0),
        );
        for core in 0..6 {
            let key = format!("usage.core{core}");
            let channel = Channel::new(&id, &key, format!("Core #{core} Usage"), Kind::Usage);
            self.analog(
                d,
                channel,
                usage(
                    12.0 + f64::from(core) * 3.0,
                    12.0,
                    20.0 + f64::from(core) * 5.0,
                ),
            );
        }
        for core in 0..6 {
            let key = format!("clock.core{core}");
            let channel = Channel::new(&id, &key, format!("Core #{core} Clock"), Kind::Clock);
            let cfg = Analog {
                base: 4300.0,
                amplitude: 300.0,
                period: 30.0 + f64::from(core) * 4.0,
                jitter: 25.0,
                min: 800.0,
                max: 4700.0,
            };
            self.analog(d, channel, cfg);
        }
        let power = |base: f64, amplitude: f64, max: f64| Analog {
            base,
            amplitude,
            period: 50.0,
            jitter: 0.4,
            min: 0.1,
            max,
        };
        self.analog(
            d,
            Channel::new(&id, "power.package", "CPU Package Power", Kind::Power)
                .with_source(Source::Helper),
            power(35.0, 20.0, 150.0),
        );
        self.analog(
            d,
            Channel::new(&id, "power.cores", "IA Cores Power", Kind::Power)
                .with_source(Source::Helper),
            power(28.0, 18.0, 140.0),
        );
        self.analog(
            d,
            Channel::new(&id, "power.gt", "GT Cores Power", Kind::Power)
                .with_source(Source::Helper),
            Analog {
                base: 0.3,
                amplitude: 0.25,
                period: 40.0,
                jitter: 0.02,
                min: 0.0,
                max: 15.0,
            },
        );
        self.analog(
            d,
            Channel::new(&id, "voltage.core0_vid", "Core #0 VID", Kind::Voltage)
                .with_source(Source::Helper),
            Analog {
                base: 1.15,
                amplitude: 0.08,
                period: 20.0,
                jitter: 0.005,
                min: 0.6,
                max: 1.4,
            },
        );
        self.boolean(
            d,
            Channel::new(
                &id,
                "bool.throttle",
                "Core Thermal Throttling",
                Kind::Boolean,
            ),
            false,
            0.002,
        );
    }

    fn motherboard(&mut self) {
        let id = DeviceId::new("hwmon:mock:nct6793d");
        let d = self.device(Device::new(
            id.as_str(),
            "Mock Z370 Board (Nuvoton NCT6793D)",
            Category::Motherboard,
        ));
        let volt = |base: f64, amplitude: f64| Analog {
            base,
            amplitude,
            period: 35.0,
            jitter: amplitude * 0.2,
            min: 0.0,
            max: 15.0,
        };
        for (key, label, base, amp) in [
            ("in0", "Vcore", 1.2, 0.08),
            ("in1", "+12V", 12.1, 0.05),
            ("in2", "+5V", 5.04, 0.02),
            ("in3", "+3.3V", 3.31, 0.01),
            ("in4", "VBAT", 3.1, 0.005),
            ("in5", "AVCC", 3.36, 0.01),
            ("in6", "3VSB", 3.39, 0.01),
        ] {
            self.analog(
                d,
                Channel::new(&id, key, label, Kind::Voltage),
                volt(base, amp),
            );
        }
        let temp = |base: f64, amplitude: f64| Analog {
            base,
            amplitude,
            period: 80.0,
            jitter: 0.3,
            min: 10.0,
            max: 110.0,
        };
        for (key, label, base, amp) in [
            ("temp1", "CPU", 42.0, 6.0),
            ("temp2", "System", 31.0, 2.0),
            ("temp3", "PCH", 46.0, 3.0),
            ("temp4", "VRM", 50.0, 8.0),
        ] {
            self.analog(
                d,
                Channel::new(&id, key, label, Kind::Temperature),
                temp(base, amp),
            );
        }
        let fan = |base: f64, amplitude: f64| Analog {
            base,
            amplitude,
            period: 60.0,
            jitter: 15.0,
            min: 0.0,
            max: 3000.0,
        };
        for (key, label, base, amp) in [
            ("fan1", "CPU Fan", 1100.0, 250.0),
            ("fan2", "Chassis Fan #1", 800.0, 100.0),
            ("fan3", "Chassis Fan #2", 760.0, 90.0),
            ("fan4", "Pump", 2400.0, 120.0),
        ] {
            self.analog(d, Channel::new(&id, key, label, Kind::Fan), fan(base, amp));
        }
        self.boolean(
            d,
            Channel::new(&id, "intrusion0", "Chassis Intrusion", Kind::Boolean),
            false,
            0.0,
        );
    }

    fn memory(&mut self) {
        let id = DeviceId::new("mem:mock");
        let d = self.device(Device::new(id.as_str(), "Memory", Category::Memory));
        self.analog(
            d,
            Channel::new(&id, "used", "Physical Memory Used", Kind::Capacity),
            Analog {
                base: 12.4 * GIB,
                amplitude: 1.5 * GIB,
                period: 120.0,
                jitter: 20.0 * MIB,
                min: 2.0 * GIB,
                max: 31.0 * GIB,
            },
        );
        self.analog(
            d,
            Channel::new(
                &id,
                "available",
                "Physical Memory Available",
                Kind::Capacity,
            ),
            Analog {
                base: 19.0 * GIB,
                amplitude: 1.5 * GIB,
                period: 120.0,
                jitter: 20.0 * MIB,
                min: 1.0 * GIB,
                max: 30.0 * GIB,
            },
        );
        self.analog(
            d,
            Channel::new(&id, "load", "Physical Memory Load", Kind::Usage),
            Analog {
                base: 39.0,
                amplitude: 5.0,
                period: 120.0,
                jitter: 0.2,
                min: 0.0,
                max: 100.0,
            },
        );
        self.analog(
            d,
            Channel::new(&id, "swap_used", "Swap Used", Kind::Capacity),
            Analog {
                base: 0.8 * GIB,
                amplitude: 0.2 * GIB,
                period: 300.0,
                jitter: 1.0 * MIB,
                min: 0.0,
                max: 8.0 * GIB,
            },
        );
    }

    fn gpu(&mut self) {
        let id = DeviceId::new("drm:mock:gpu0");
        let d = self.device(Device::new(
            id.as_str(),
            "GPU [#0]: Mock Graphics 630",
            Category::Gpu,
        ));
        self.analog(
            d,
            Channel::new(&id, "temp", "GPU Temperature", Kind::Temperature),
            Analog {
                base: 55.0,
                amplitude: 8.0,
                period: 70.0,
                jitter: 0.5,
                min: 20.0,
                max: 105.0,
            },
        );
        self.analog(
            d,
            Channel::new(&id, "clock.core", "GPU Clock", Kind::Clock),
            Analog {
                base: 1150.0,
                amplitude: 250.0,
                period: 25.0,
                jitter: 10.0,
                min: 300.0,
                max: 1450.0,
            },
        );
        self.analog(
            d,
            Channel::new(&id, "clock.memory", "GPU Memory Clock", Kind::Clock),
            Analog {
                base: 2133.0,
                amplitude: 0.0,
                period: 1.0,
                jitter: 0.0,
                min: 2133.0,
                max: 2133.0,
            },
        );
        self.analog(
            d,
            Channel::new(&id, "usage.core", "GPU Core Load", Kind::Usage),
            Analog {
                base: 35.0,
                amplitude: 30.0,
                period: 18.0,
                jitter: 3.0,
                min: 0.0,
                max: 100.0,
            },
        );
        self.analog(
            d,
            Channel::new(&id, "usage.video", "GPU Video Engine Load", Kind::Usage),
            Analog {
                base: 5.0,
                amplitude: 5.0,
                period: 40.0,
                jitter: 1.0,
                min: 0.0,
                max: 100.0,
            },
        );
        self.analog(
            d,
            Channel::new(&id, "power", "GPU Power", Kind::Power).with_source(Source::Helper),
            Analog {
                base: 12.0,
                amplitude: 8.0,
                period: 30.0,
                jitter: 0.3,
                min: 0.5,
                max: 45.0,
            },
        );
    }

    fn drives(&mut self) {
        let drives = [
            (
                "nvme0n1",
                "Mock NVMe SSD 1TB",
                38.0,
                25.0 * MIB,
                3500.0 * MIB,
                4120.0,
                97.0,
            ),
            (
                "sda",
                "Mock SATA SSD 500GB",
                33.0,
                6.0 * MIB,
                550.0 * MIB,
                11_870.0,
                91.0,
            ),
        ];
        for (name, model, temp, rate, max_rate, hours, life) in drives {
            let id = DeviceId::new(format!("block:mock:{name}"));
            let d = self.device(Device::new(
                id.as_str(),
                format!("Drive: {model}"),
                Category::Drive,
            ));
            self.analog(
                d,
                Channel::new(&id, "temp", "Drive Temperature", Kind::Temperature),
                Analog {
                    base: temp,
                    amplitude: 4.0,
                    period: 200.0,
                    jitter: 0.2,
                    min: 10.0,
                    max: 90.0,
                },
            );
            self.analog(
                d,
                Channel::new(&id, "read_rate", "Read Rate", Kind::Throughput),
                Analog {
                    base: rate,
                    amplitude: rate,
                    period: 15.0,
                    jitter: rate * 0.2,
                    min: 0.0,
                    max: max_rate,
                },
            );
            self.analog(
                d,
                Channel::new(&id, "write_rate", "Write Rate", Kind::Throughput),
                Analog {
                    base: rate * 0.3,
                    amplitude: rate * 0.3,
                    period: 22.0,
                    jitter: rate * 0.1,
                    min: 0.0,
                    max: max_rate,
                },
            );
            self.analog(
                d,
                Channel::new(&id, "activity", "Total Activity", Kind::Usage),
                Analog {
                    base: 6.0,
                    amplitude: 6.0,
                    period: 15.0,
                    jitter: 1.0,
                    min: 0.0,
                    max: 100.0,
                },
            );
            self.monotonic(
                d,
                Channel::new(&id, "read_total", "Total Host Reads", Kind::Capacity),
                1.2e12,
                rate,
            );
            self.monotonic(
                d,
                Channel::new(&id, "write_total", "Total Host Writes", Kind::Capacity),
                0.9e12,
                rate * 0.3,
            );

            let smart_id = DeviceId::new(format!("smart:mock:{name}"));
            let s = self.device(Device::new(
                smart_id.as_str(),
                format!("S.M.A.R.T.: {model}"),
                Category::Smart,
            ));
            self.analog(
                s,
                Channel::new(&smart_id, "temp", "Drive Temperature", Kind::Temperature)
                    .with_source(Source::Helper),
                Analog {
                    base: temp,
                    amplitude: 4.0,
                    period: 200.0,
                    jitter: 0.0,
                    min: 10.0,
                    max: 90.0,
                },
            );
            self.analog(
                s,
                Channel::new(&smart_id, "life", "Drive Remaining Life", Kind::Usage)
                    .with_source(Source::Helper),
                Analog {
                    base: life,
                    amplitude: 0.0,
                    period: 1.0,
                    jitter: 0.0,
                    min: 0.0,
                    max: 100.0,
                },
            );
            self.monotonic(
                s,
                Channel::new(&smart_id, "power_on_hours", "Power-On Hours", Kind::Count)
                    .with_source(Source::Helper),
                hours,
                1.0 / 3600.0,
            );
            self.boolean(
                s,
                Channel::new(&smart_id, "failure", "Drive Failure", Kind::Boolean)
                    .with_source(Source::Helper),
                false,
                0.0,
            );
            self.boolean(
                s,
                Channel::new(&smart_id, "warning", "Drive Warning", Kind::Boolean)
                    .with_source(Source::Helper),
                false,
                0.0,
            );
        }
    }

    fn network(&mut self) {
        let id = DeviceId::new("net:mock:eth0");
        let d = self.device(Device::new(id.as_str(), "Network: eth0", Category::Network));
        self.analog(
            d,
            Channel::new(&id, "download", "Download Rate", Kind::Throughput),
            Analog {
                base: 1.2 * MIB,
                amplitude: 1.2 * MIB,
                period: 20.0,
                jitter: 0.2 * MIB,
                min: 0.0,
                max: 118.0 * MIB,
            },
        );
        self.analog(
            d,
            Channel::new(&id, "upload", "Upload Rate", Kind::Throughput),
            Analog {
                base: 0.2 * MIB,
                amplitude: 0.2 * MIB,
                period: 33.0,
                jitter: 0.05 * MIB,
                min: 0.0,
                max: 118.0 * MIB,
            },
        );
        self.monotonic(
            d,
            Channel::new(&id, "total_down", "Total Downloaded", Kind::Capacity),
            2.1 * GIB,
            1.2 * MIB,
        );
        self.monotonic(
            d,
            Channel::new(&id, "total_up", "Total Uploaded", Kind::Capacity),
            0.4 * GIB,
            0.2 * MIB,
        );
    }
}

/// The synthetic backend. See the module documentation.
pub struct MockBackend {
    started: Instant,
    rng: Rng,
    devices: Vec<(Device, Vec<Channel>)>,
    signals: Vec<Signal>,
}

impl MockBackend {
    pub fn new(seed: u64) -> Self {
        let mut builder = Builder {
            rng: Rng::new(seed),
            devices: Vec::new(),
            signals: Vec::new(),
        };
        builder.cpu();
        builder.motherboard();
        builder.memory();
        builder.gpu();
        builder.drives();
        builder.network();
        Self {
            started: Instant::now(),
            rng: builder.rng,
            devices: builder.devices,
            signals: builder.signals,
        }
    }

    /// Generates one sample per channel for an explicit time (seconds). Deterministic
    /// for a given seed and call sequence, which tests rely on.
    pub fn sample_at(&mut self, t: f64, out: &mut Vec<Sample>) {
        for signal in &mut self.signals {
            let value = signal.value(t, &mut self.rng);
            out.push(Sample::new(signal.channel.clone(), Some(value)));
        }
    }
}

impl Backend for MockBackend {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn enumerate(&mut self) -> anyhow::Result<Vec<(Device, Vec<Channel>)>> {
        Ok(self.devices.clone())
    }

    fn sample(&mut self, out: &mut Vec<Sample>) -> anyhow::Result<()> {
        let t = self.started.elapsed().as_secs_f64();
        self.sample_at(t, out);
        Ok(())
    }

    fn rescan_every(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerates_a_realistic_set_of_devices() {
        let mut mock = MockBackend::new(7);
        let devices = mock.enumerate().unwrap();
        assert_eq!(devices.len(), 9);
        let channels: usize = devices.iter().map(|(_, c)| c.len()).sum();
        assert!(channels > 60, "expected a rich channel set, got {channels}");
        let cpu = &devices[0].0;
        assert_eq!(cpu.category, Category::Cpu);
        assert!(devices.iter().any(|(d, _)| d.category == Category::Smart));
    }

    #[test]
    fn produces_one_sample_per_channel_within_range() {
        let mut mock = MockBackend::new(7);
        let devices = mock.enumerate().unwrap();
        let channels: usize = devices.iter().map(|(_, c)| c.len()).sum();
        let mut out = Vec::new();
        mock.sample_at(12.5, &mut out);
        assert_eq!(out.len(), channels);
        for sample in &out {
            let v = sample.value.unwrap();
            assert!(v.is_finite(), "{} produced {v}", sample.channel);
            if sample.channel.key().starts_with("usage") {
                assert!((0.0..=100.0).contains(&v), "{} = {v}", sample.channel);
            }
        }
    }

    #[test]
    fn is_deterministic_for_a_seed() {
        let mut a = MockBackend::new(42);
        let mut b = MockBackend::new(42);
        let (mut out_a, mut out_b) = (Vec::new(), Vec::new());
        for t in [1.0, 2.0, 3.0] {
            a.sample_at(t, &mut out_a);
            b.sample_at(t, &mut out_b);
        }
        assert_eq!(out_a, out_b);
    }
}

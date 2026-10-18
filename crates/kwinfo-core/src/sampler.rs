//! The sampler thread: polls backends on a fixed interval, keeps statistics and history
//! per channel, and publishes immutable snapshots for the front ends.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use parking_lot::Mutex;
use tracing::{debug, info, warn};

use crate::backend::Backend;
use crate::history::History;
use crate::model::{Channel, ChannelId, Device, DeviceId, Sample};
use crate::stats::Stats;

/// The shortest polling interval the sampler accepts.
pub const MIN_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug)]
pub struct SamplerConfig {
    /// Time between two sampling ticks.
    pub interval: Duration,
    /// Number of points kept per channel for graphs.
    pub history_capacity: usize,
}

impl Default for SamplerConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_millis(1000),
            history_capacity: 3600,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChannelSnapshot {
    pub channel: Arc<Channel>,
    pub stats: Stats,
}

#[derive(Clone, Debug)]
pub struct DeviceSnapshot {
    pub device: Arc<Device>,
    pub channels: Vec<ChannelSnapshot>,
}

/// An immutable view of every device, channel and statistic at one tick. Devices are
/// ordered by category, then by the backend's order, then by label.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub tick: u64,
    pub started_at: Instant,
    pub taken_at: Instant,
    /// When statistics were last reset (or the start).
    pub stats_since: Instant,
    pub interval: Duration,
    /// How long the backends took during this tick.
    pub tick_cost: Duration,
    pub devices: Vec<DeviceSnapshot>,
}

impl Snapshot {
    fn empty(now: Instant, interval: Duration) -> Self {
        Self {
            tick: 0,
            started_at: now,
            taken_at: now,
            stats_since: now,
            interval,
            tick_cost: Duration::ZERO,
            devices: Vec::new(),
        }
    }

    pub fn channel_count(&self) -> usize {
        self.devices.iter().map(|d| d.channels.len()).sum()
    }

    /// Time since the statistics were last reset.
    pub fn elapsed(&self) -> Duration {
        self.taken_at.saturating_duration_since(self.stats_since)
    }

    pub fn channels(&self) -> impl Iterator<Item = &ChannelSnapshot> {
        self.devices.iter().flat_map(|d| d.channels.iter())
    }

    pub fn find(&self, id: &ChannelId) -> Option<&ChannelSnapshot> {
        self.channels().find(|c| &c.channel.id == id)
    }
}

enum Command {
    SetInterval(Duration),
    ResetAll,
    Reset(ChannelId),
    OnTick(Box<dyn Fn() + Send>),
    Stop,
}

struct Shared {
    snapshot: ArcSwap<Snapshot>,
    history: Mutex<HashMap<ChannelId, History>>,
}

/// Handle to the sampler thread. Dropping it stops the thread.
pub struct Sampler {
    shared: Arc<Shared>,
    tx: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Sampler {
    /// Spawns the sampler thread, which enumerates the backends and starts ticking at
    /// once. The first snapshot is published as soon as enumeration finishes.
    pub fn start(backends: Vec<Box<dyn Backend>>, config: SamplerConfig) -> Self {
        let now = Instant::now();
        let shared = Arc::new(Shared {
            snapshot: ArcSwap::from_pointee(Snapshot::empty(now, config.interval)),
            history: Mutex::new(HashMap::new()),
        });
        let (tx, rx) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("kwinfo-sampler".into())
            .spawn(move || Worker::new(backends, config, worker_shared).run(rx))
            .expect("failed to spawn the sampler thread");
        Self {
            shared,
            tx,
            thread: Some(thread),
        }
    }

    /// The latest snapshot. Cheap: an atomic pointer load.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.shared.snapshot.load_full()
    }

    pub fn set_interval(&self, interval: Duration) {
        self.send(Command::SetInterval(interval));
    }

    /// Resets minimum, maximum and average of every channel.
    pub fn reset_all(&self) {
        self.send(Command::ResetAll);
    }

    /// Resets one channel's statistics and clears its history.
    pub fn reset_channel(&self, id: ChannelId) {
        self.send(Command::Reset(id));
    }

    /// Registers a callback that runs on the sampler thread after every tick, for
    /// example to request a repaint of the UI.
    pub fn set_on_tick(&self, on_tick: impl Fn() + Send + 'static) {
        self.send(Command::OnTick(Box::new(on_tick)));
    }

    /// Runs `f` with the history of `id`, if the channel is known. Holds a lock, so keep
    /// `f` short (copy the points out).
    pub fn with_history<R>(&self, id: &ChannelId, f: impl FnOnce(&History) -> R) -> Option<R> {
        let history = self.shared.history.lock();
        history.get(id).map(f)
    }

    fn send(&self, command: Command) {
        if self.tx.send(command).is_err() {
            warn!("the sampler thread is gone; command dropped");
        }
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct BackendSlot {
    backend: Box<dyn Backend>,
    last_scan: Instant,
    failing: bool,
}

struct ChannelState {
    channel: Arc<Channel>,
    stats: Stats,
}

struct DeviceState {
    device: Arc<Device>,
    channels: Vec<usize>,
}

struct Worker {
    backends: Vec<BackendSlot>,
    devices: Vec<DeviceState>,
    device_index: HashMap<DeviceId, usize>,
    channels: Vec<ChannelState>,
    channel_index: HashMap<ChannelId, usize>,
    publish_order: Vec<usize>,
    shared: Arc<Shared>,
    interval: Duration,
    history_capacity: usize,
    started_at: Instant,
    stats_since: Instant,
    next_tick: Instant,
    tick: u64,
    on_tick: Option<Box<dyn Fn() + Send>>,
    scratch: Vec<Sample>,
}

impl Worker {
    fn new(backends: Vec<Box<dyn Backend>>, config: SamplerConfig, shared: Arc<Shared>) -> Self {
        let now = Instant::now();
        Self {
            backends: backends
                .into_iter()
                .map(|backend| BackendSlot {
                    backend,
                    last_scan: now,
                    failing: false,
                })
                .collect(),
            devices: Vec::new(),
            device_index: HashMap::new(),
            channels: Vec::new(),
            channel_index: HashMap::new(),
            publish_order: Vec::new(),
            shared,
            interval: config.interval.max(MIN_INTERVAL),
            history_capacity: config.history_capacity.max(1),
            started_at: now,
            stats_since: now,
            next_tick: now,
            tick: 0,
            on_tick: None,
            scratch: Vec::new(),
        }
    }

    fn run(mut self, rx: Receiver<Command>) {
        self.rescan(true);
        self.publish(Duration::ZERO);
        // Enumeration can take a while on a real machine; start the tick schedule after
        // it, so that the second tick (the first with rates) comes a full interval later.
        self.next_tick = Instant::now();
        loop {
            if Instant::now() >= self.next_tick {
                self.tick();
                self.next_tick += self.interval;
                let now = Instant::now();
                if self.next_tick < now {
                    // The tick overran the interval; resume from now instead of bursting.
                    self.next_tick = now;
                }
            }
            let timeout = self.next_tick.saturating_duration_since(Instant::now());
            match rx.recv_timeout(timeout) {
                Ok(command) => {
                    if !self.handle(command) {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        debug!("sampler thread stopped after {} ticks", self.tick);
    }

    /// Returns `false` when the thread should stop.
    fn handle(&mut self, command: Command) -> bool {
        match command {
            Command::SetInterval(interval) => {
                self.interval = interval.max(MIN_INTERVAL);
                self.next_tick = Instant::now();
            }
            Command::ResetAll => {
                for channel in &mut self.channels {
                    channel.stats.reset();
                }
                self.stats_since = Instant::now();
                self.publish(Duration::ZERO);
            }
            Command::Reset(id) => {
                if let Some(&index) = self.channel_index.get(&id) {
                    self.channels[index].stats.reset();
                    if let Some(history) = self.shared.history.lock().get_mut(&id) {
                        history.clear();
                    }
                    self.publish(Duration::ZERO);
                }
            }
            Command::OnTick(on_tick) => self.on_tick = Some(on_tick),
            Command::Stop => return false,
        }
        true
    }

    /// Re-enumerates every backend that is due (or all of them when `force`).
    fn rescan(&mut self, force: bool) {
        let now = Instant::now();
        let mut changed = false;
        for i in 0..self.backends.len() {
            let due = force
                || self.backends[i]
                    .backend
                    .rescan_every()
                    .is_some_and(|every| now.duration_since(self.backends[i].last_scan) >= every);
            if !due {
                continue;
            }
            self.backends[i].last_scan = now;
            let name = self.backends[i].backend.name();
            match self.backends[i].backend.enumerate() {
                Ok(found) => {
                    for (device, channels) in found {
                        changed |= self.add_device(device, channels);
                    }
                }
                Err(err) => warn!(backend = name, "enumeration failed: {err:#}"),
            }
        }
        if changed {
            self.recompute_order();
            info!(
                devices = self.devices.len(),
                channels = self.channels.len(),
                "sensor enumeration updated"
            );
        }
    }

    /// Adds unknown devices and channels. Returns whether anything new appeared.
    fn add_device(&mut self, device: Device, channels: Vec<Channel>) -> bool {
        let mut changed = false;
        let d = match self.device_index.get(&device.id) {
            Some(&d) => d,
            None => {
                let d = self.devices.len();
                self.device_index.insert(device.id.clone(), d);
                self.devices.push(DeviceState {
                    device: Arc::new(device),
                    channels: Vec::new(),
                });
                changed = true;
                d
            }
        };
        let mut history = self.shared.history.lock();
        for channel in channels {
            if self.channel_index.contains_key(&channel.id) {
                continue;
            }
            let c = self.channels.len();
            self.channel_index.insert(channel.id.clone(), c);
            history.insert(channel.id.clone(), History::new(self.history_capacity));
            self.channels.push(ChannelState {
                channel: Arc::new(channel),
                stats: Stats::new(),
            });
            self.devices[d].channels.push(c);
            changed = true;
        }
        changed
    }

    fn recompute_order(&mut self) {
        let mut order: Vec<usize> = (0..self.devices.len()).collect();
        order.sort_by(|&a, &b| {
            let (da, db) = (&self.devices[a].device, &self.devices[b].device);
            (da.category, da.order, da.label.as_str()).cmp(&(
                db.category,
                db.order,
                db.label.as_str(),
            ))
        });
        self.publish_order = order;
    }

    fn tick(&mut self) {
        let start = Instant::now();
        self.rescan(false);
        let t = start.duration_since(self.started_at).as_secs_f64();
        let shared = Arc::clone(&self.shared);
        let mut history = shared.history.lock();
        let mut scratch = std::mem::take(&mut self.scratch);
        for i in 0..self.backends.len() {
            scratch.clear();
            let name = self.backends[i].backend.name();
            match self.backends[i].backend.sample(&mut scratch) {
                Ok(()) => {
                    if self.backends[i].failing {
                        info!(backend = name, "sampling recovered");
                        self.backends[i].failing = false;
                    }
                    for sample in &scratch {
                        match self.channel_index.get(&sample.channel) {
                            Some(&c) => {
                                self.channels[c].stats.update(sample.value);
                                if let Some(h) = history.get_mut(&sample.channel) {
                                    h.push(t, sample.value);
                                }
                            }
                            None => debug!(
                                backend = name,
                                channel = %sample.channel,
                                "sample for an unknown channel ignored"
                            ),
                        }
                    }
                }
                Err(err) => {
                    if !self.backends[i].failing {
                        warn!(backend = name, "sampling failed: {err:#}");
                        self.backends[i].failing = true;
                    }
                }
            }
        }
        self.scratch = scratch;
        drop(history);
        self.tick += 1;
        self.publish(start.elapsed());
        if let Some(on_tick) = &self.on_tick {
            on_tick();
        }
    }

    fn publish(&self, tick_cost: Duration) {
        let devices = self
            .publish_order
            .iter()
            .map(|&d| {
                let state = &self.devices[d];
                DeviceSnapshot {
                    device: Arc::clone(&state.device),
                    channels: state
                        .channels
                        .iter()
                        .map(|&c| {
                            let cs = &self.channels[c];
                            ChannelSnapshot {
                                channel: Arc::clone(&cs.channel),
                                stats: cs.stats,
                            }
                        })
                        .collect(),
                }
            })
            .collect();
        let snapshot = Snapshot {
            tick: self.tick,
            started_at: self.started_at,
            taken_at: Instant::now(),
            stats_since: self.stats_since,
            interval: self.interval,
            tick_cost,
            devices,
        };
        self.shared.snapshot.store(Arc::new(snapshot));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Category, Kind};

    /// A backend whose single channel counts the samples taken.
    struct Counter {
        device: Device,
        channel: Channel,
        n: f64,
    }

    impl Counter {
        fn new() -> Self {
            let device = Device::new("test:counter", "Counter", Category::Other);
            let channel = Channel::new(&device.id, "n", "Count", Kind::Count);
            Self {
                device,
                channel,
                n: 0.0,
            }
        }
    }

    impl Backend for Counter {
        fn name(&self) -> &'static str {
            "counter"
        }

        fn enumerate(&mut self) -> anyhow::Result<Vec<(Device, Vec<Channel>)>> {
            Ok(vec![(self.device.clone(), vec![self.channel.clone()])])
        }

        fn sample(&mut self, out: &mut Vec<Sample>) -> anyhow::Result<()> {
            self.n += 1.0;
            out.push(Sample::new(self.channel.id.clone(), Some(self.n)));
            Ok(())
        }

        fn rescan_every(&self) -> Option<Duration> {
            None
        }
    }

    fn wait_until(sampler: &Sampler, pred: impl Fn(&Snapshot) -> bool) -> Arc<Snapshot> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let snapshot = sampler.snapshot();
            if pred(&snapshot) {
                return snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the sampler"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn fast(history_capacity: usize) -> SamplerConfig {
        SamplerConfig {
            interval: Duration::from_millis(5),
            history_capacity,
        }
    }

    #[test]
    fn ticks_accumulate_statistics_and_history() {
        let sampler = Sampler::start(vec![Box::new(Counter::new())], fast(4));
        let snapshot = wait_until(&sampler, |s| s.tick >= 5);
        let id = ChannelId::from_raw("test:counter/n");
        let entry = snapshot
            .find(&id)
            .expect("the counter channel is published");
        assert_eq!(entry.stats.min(), Some(1.0));
        let max = entry.stats.max().unwrap();
        assert!(max >= 5.0);
        assert_eq!(entry.stats.current(), Some(max));
        assert_eq!(snapshot.devices[0].device.label, "Counter");
        let len = sampler.with_history(&id, History::len).unwrap();
        assert_eq!(len, 4, "history is capped at its capacity");
    }

    #[test]
    fn reset_restarts_statistics_while_sampling_continues() {
        let sampler = Sampler::start(vec![Box::new(Counter::new())], fast(16));
        let before = wait_until(&sampler, |s| s.tick >= 3);
        let id = ChannelId::from_raw("test:counter/n");
        let max_before = before.find(&id).unwrap().stats.max().unwrap();

        sampler.reset_all();
        let after = wait_until(&sampler, |s| {
            s.stats_since > before.stats_since && s.tick >= before.tick + 2
        });
        let stats = after.find(&id).unwrap().stats;
        assert!(
            stats.min().unwrap() > max_before,
            "the minimum restarted after the reset"
        );
        assert!(stats.count() <= after.tick - before.tick + 1);
        assert!(after.elapsed() < before.elapsed() + Duration::from_secs(1));
    }

    #[test]
    fn interval_changes_are_published() {
        let sampler = Sampler::start(Vec::new(), SamplerConfig::default());
        sampler.set_interval(Duration::from_millis(250));
        let snapshot = wait_until(&sampler, |s| s.interval == Duration::from_millis(250));
        assert!(snapshot.devices.is_empty());
        assert_eq!(snapshot.channel_count(), 0);
    }

    #[test]
    fn intervals_below_the_minimum_are_clamped() {
        let sampler = Sampler::start(
            Vec::new(),
            SamplerConfig {
                interval: Duration::ZERO,
                history_capacity: 0,
            },
        );
        let snapshot = wait_until(&sampler, |s| s.tick >= 1);
        assert_eq!(snapshot.interval, MIN_INTERVAL);
    }
}

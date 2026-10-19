//! The sampler thread: polls backends on a fixed interval, keeps statistics and history
//! per channel, and publishes immutable snapshots for the front ends.

use std::collections::{HashMap, HashSet};
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

/// Ticks between two `kwinfo::health` log lines.
pub const HEALTH_EVERY: u64 = 60;

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
    /// The channel is not polled ([`Sampler::set_disabled`]); its statistics are empty.
    pub disabled: bool,
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
    SetHistoryCapacity(usize),
    SetDisabled {
        channels: HashSet<ChannelId>,
        devices: HashSet<DeviceId>,
    },
    ResetAll,
    Reset(Vec<ChannelId>),
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

    /// Changes how many points each channel's history keeps; shrinking drops the oldest.
    pub fn set_history_capacity(&self, capacity: usize) {
        self.send(Command::SetHistoryCapacity(capacity));
    }

    /// Resets minimum, maximum and average of every channel.
    pub fn reset_all(&self) {
        self.send(Command::ResetAll);
    }

    /// Resets one channel's statistics and clears its history.
    pub fn reset_channel(&self, id: ChannelId) {
        self.send(Command::Reset(vec![id]));
    }

    /// Resets the statistics and history of several channels, e.g. of one device.
    pub fn reset_channels(&self, ids: impl IntoIterator<Item = ChannelId>) {
        self.send(Command::Reset(ids.into_iter().collect()));
    }

    /// Stops polling `channels` and every channel of `devices` ("Disable monitoring"),
    /// replacing the previous set. Their statistics and history are cleared and stay
    /// empty until they are enabled again; the backends are told so they can skip the
    /// reads. Channels that appear later on a disabled device are disabled too.
    pub fn set_disabled(&self, channels: HashSet<ChannelId>, devices: HashSet<DeviceId>) {
        self.send(Command::SetDisabled { channels, devices });
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
    disabled: bool,
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
    /// How long the backends took during the last tick.
    tick_cost: Duration,
    /// Tick costs since the last health line.
    health: TickCosts,
    on_tick: Option<Box<dyn Fn() + Send>>,
    scratch: Vec<Sample>,
    disabled_channels: HashSet<ChannelId>,
    disabled_devices: HashSet<DeviceId>,
    /// Every channel not polled, from both sets above; what the backends are told.
    disabled: HashSet<ChannelId>,
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
            tick_cost: Duration::ZERO,
            health: TickCosts::default(),
            on_tick: None,
            scratch: Vec::new(),
            disabled_channels: HashSet::new(),
            disabled_devices: HashSet::new(),
            disabled: HashSet::new(),
        }
    }

    fn run(mut self, rx: Receiver<Command>) {
        self.rescan(true);
        // Commands sent right after start (typically the disabled set) apply before the
        // first tick reads anything.
        while let Ok(command) = rx.try_recv() {
            if !self.handle(command) {
                return;
            }
        }
        self.publish();
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
            Command::SetHistoryCapacity(capacity) => {
                self.history_capacity = capacity.max(1);
                for history in self.shared.history.lock().values_mut() {
                    history.set_capacity(self.history_capacity);
                }
            }
            Command::SetInterval(interval) => {
                self.interval = interval.max(MIN_INTERVAL);
                self.next_tick = Instant::now();
            }
            Command::SetDisabled { channels, devices } => {
                self.disabled_channels = channels;
                self.disabled_devices = devices;
                if self.apply_disabled() {
                    self.tell_backends(None);
                }
                self.publish();
            }
            Command::ResetAll => {
                for channel in &mut self.channels {
                    channel.stats.reset();
                }
                self.stats_since = Instant::now();
                self.publish();
            }
            Command::Reset(ids) => {
                let mut history = self.shared.history.lock();
                let mut any = false;
                for id in &ids {
                    if let Some(&index) = self.channel_index.get(id) {
                        self.channels[index].stats.reset();
                        if let Some(points) = history.get_mut(id) {
                            points.clear();
                        }
                        any = true;
                    }
                }
                drop(history);
                if any {
                    self.publish();
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
            // Enumeration rebuilds the backend's read plan; tell it again what to skip.
            if changed && self.apply_disabled() {
                self.tell_backends(None);
            } else {
                self.tell_backends(Some(i));
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
                disabled: false,
            });
            self.devices[d].channels.push(c);
            changed = true;
        }
        changed
    }

    /// Recomputes which channels are not polled from the user's channel and device sets.
    /// A channel that gets disabled loses its statistics and history: they would describe
    /// a past the user chose to stop watching. Returns whether the set changed.
    fn apply_disabled(&mut self) -> bool {
        let mut disabled = HashSet::new();
        let mut history = self.shared.history.lock();
        for state in &mut self.channels {
            let now_disabled = self.disabled_channels.contains(&state.channel.id)
                || self.disabled_devices.contains(&state.channel.device);
            if now_disabled && !state.disabled {
                state.stats = Stats::new();
                if let Some(points) = history.get_mut(&state.channel.id) {
                    points.clear();
                }
            }
            state.disabled = now_disabled;
            if now_disabled {
                disabled.insert(state.channel.id.clone());
            }
        }
        drop(history);
        let changed = disabled != self.disabled;
        self.disabled = disabled;
        changed
    }

    /// Passes the disabled set to one backend, or to all of them.
    fn tell_backends(&mut self, only: Option<usize>) {
        for (i, slot) in self.backends.iter_mut().enumerate() {
            if only.is_none_or(|o| o == i) {
                slot.backend.set_disabled(&self.disabled);
            }
        }
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
                            Some(&c) if self.channels[c].disabled => {}
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
        self.tick_cost = start.elapsed();
        self.publish();
        if let Some(line) = self.health_line() {
            line.log();
        }
        if let Some(on_tick) = &self.on_tick {
            on_tick();
        }
    }

    /// Every [`HEALTH_EVERY`] ticks, what the `kwinfo::health` line reports; the tick
    /// costs then start accumulating afresh.
    fn health_line(&mut self) -> Option<HealthLine> {
        self.health.record(self.tick_cost);
        if !self.tick.is_multiple_of(HEALTH_EVERY) {
            return None;
        }
        let costs = std::mem::take(&mut self.health);
        Some(HealthLine {
            tick: self.tick,
            interval: self.interval,
            avg_tick: costs.average(),
            max_tick: costs.max,
            devices: self.devices.len(),
            channels: self.channels.len(),
            // Disabled channels have no reading on purpose; they are counted apart.
            unreadable: self
                .channels
                .iter()
                .filter(|c| !c.disabled && c.stats.current().is_none())
                .count(),
            disabled: self.channels.iter().filter(|c| c.disabled).count(),
            failing_backends: self
                .backends
                .iter()
                .filter(|slot| slot.failing)
                .map(|slot| slot.backend.name())
                .collect(),
        })
    }

    fn publish(&self) {
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
                                disabled: cs.disabled,
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
            tick_cost: self.tick_cost,
            devices,
        };
        self.shared.snapshot.store(Arc::new(snapshot));
    }
}

/// Tick costs accumulated between two health lines.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct TickCosts {
    ticks: u32,
    total: Duration,
    max: Duration,
}

impl TickCosts {
    fn record(&mut self, cost: Duration) {
        self.ticks += 1;
        self.total += cost;
        self.max = self.max.max(cost);
    }

    fn average(&self) -> Duration {
        self.total.checked_div(self.ticks).unwrap_or_default()
    }
}

/// The periodic `kwinfo::health` line: how expensive the ticks were and how many
/// channels or backends are in trouble.
#[derive(Clone, Debug, PartialEq)]
struct HealthLine {
    tick: u64,
    interval: Duration,
    avg_tick: Duration,
    max_tick: Duration,
    devices: usize,
    channels: usize,
    /// Enabled channels whose current value is missing.
    unreadable: usize,
    disabled: usize,
    failing_backends: Vec<&'static str>,
}

impl HealthLine {
    fn log(&self) {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        info!(
            target: "kwinfo::health",
            tick = self.tick,
            interval_ms = u64::try_from(self.interval.as_millis()).unwrap_or(u64::MAX),
            avg_tick_ms = ms(self.avg_tick),
            max_tick_ms = ms(self.max_tick),
            devices = self.devices,
            channels = self.channels,
            unreadable = self.unreadable,
            disabled = self.disabled,
            failing_backends = %self.failing_backends.join(","),
            "sampler health"
        );
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

    /// Two counting channels on one device; remembers every disabled set it is given.
    struct Pair {
        device: Device,
        channels: [Channel; 2],
        n: f64,
        told: Arc<Mutex<Vec<HashSet<ChannelId>>>>,
    }

    impl Pair {
        fn new(told: Arc<Mutex<Vec<HashSet<ChannelId>>>>) -> Self {
            let device = Device::new("test:pair", "Pair", Category::Other);
            let channels = [
                Channel::new(&device.id, "a", "A", Kind::Count),
                Channel::new(&device.id, "b", "B", Kind::Count),
            ];
            Self {
                device,
                channels,
                n: 0.0,
                told,
            }
        }
    }

    impl Backend for Pair {
        fn name(&self) -> &'static str {
            "pair"
        }

        fn enumerate(&mut self) -> anyhow::Result<Vec<(Device, Vec<Channel>)>> {
            Ok(vec![(self.device.clone(), self.channels.to_vec())])
        }

        fn sample(&mut self, out: &mut Vec<Sample>) -> anyhow::Result<()> {
            self.n += 1.0;
            for channel in &self.channels {
                out.push(Sample::new(channel.id.clone(), Some(self.n)));
            }
            Ok(())
        }

        fn rescan_every(&self) -> Option<Duration> {
            None
        }

        fn set_disabled(&mut self, disabled: &HashSet<ChannelId>) {
            self.told.lock().push(disabled.clone());
        }
    }

    #[test]
    fn disabled_channels_are_not_polled_until_enabled_again() {
        let told = Arc::new(Mutex::new(Vec::new()));
        let sampler = Sampler::start(vec![Box::new(Pair::new(Arc::clone(&told)))], fast(64));
        let (a, b) = (
            ChannelId::from_raw("test:pair/a"),
            ChannelId::from_raw("test:pair/b"),
        );
        let before = wait_until(&sampler, |s| s.tick >= 3);
        assert!(before.find(&b).unwrap().stats.count() >= 3);
        assert_eq!(
            told.lock().first(),
            Some(&HashSet::new()),
            "told after enumeration"
        );

        sampler.set_disabled(HashSet::from([b.clone()]), HashSet::new());
        let first = wait_until(&sampler, |s| s.find(&b).is_some_and(|c| c.disabled));
        let later = wait_until(&sampler, |s| s.tick >= first.tick + 3);
        let entry = later.find(&b).unwrap();
        assert!(entry.disabled);
        assert_eq!(
            entry.stats.count(),
            0,
            "statistics are cleared and stay empty"
        );
        assert_eq!(entry.stats.current(), None);
        assert_eq!(sampler.with_history(&b, History::len), Some(0));
        assert!(!later.find(&a).unwrap().disabled);
        assert!(later.find(&a).unwrap().stats.count() > before.find(&a).unwrap().stats.count());
        assert_eq!(told.lock().last(), Some(&HashSet::from([b.clone()])));

        sampler.set_disabled(HashSet::new(), HashSet::new());
        let enabled = wait_until(&sampler, |s| {
            s.find(&b)
                .is_some_and(|c| !c.disabled && c.stats.count() >= 2)
        });
        let stats = enabled.find(&b).unwrap().stats;
        assert!(
            stats.min().unwrap() > later.tick as f64 - 1.0,
            "statistics restart from the first sample after enabling"
        );
        assert_eq!(told.lock().last(), Some(&HashSet::new()));
    }

    #[test]
    fn disabling_a_device_disables_all_its_channels() {
        let told = Arc::new(Mutex::new(Vec::new()));
        let sampler = Sampler::start(vec![Box::new(Pair::new(Arc::clone(&told)))], fast(8));
        wait_until(&sampler, |s| s.tick >= 1);
        sampler.set_disabled(HashSet::new(), HashSet::from([DeviceId::new("test:pair")]));
        let snapshot = wait_until(&sampler, |s| s.channels().all(|c| c.disabled));
        let later = wait_until(&sampler, |s| s.tick >= snapshot.tick + 2);
        assert!(later.channels().all(|c| c.stats.count() == 0));
        assert_eq!(
            told.lock().last(),
            Some(&HashSet::from([
                ChannelId::from_raw("test:pair/a"),
                ChannelId::from_raw("test:pair/b"),
            ]))
        );
    }

    /// A backend that enumerates one device whose channel never has a reading and
    /// whose sampling always fails.
    struct Broken;

    impl Backend for Broken {
        fn name(&self) -> &'static str {
            "broken"
        }

        fn enumerate(&mut self) -> anyhow::Result<Vec<(Device, Vec<Channel>)>> {
            let device = Device::new("test:broken", "Broken", Category::Other);
            let channel = Channel::new(&device.id, "x", "X", Kind::Count);
            Ok(vec![(device, vec![channel])])
        }

        fn sample(&mut self, _out: &mut Vec<Sample>) -> anyhow::Result<()> {
            anyhow::bail!("no such file")
        }

        fn rescan_every(&self) -> Option<Duration> {
            None
        }
    }

    #[test]
    fn a_health_line_comes_every_sixty_ticks() {
        let told = Arc::new(Mutex::new(Vec::new()));
        let shared = Arc::new(Shared {
            snapshot: ArcSwap::from_pointee(Snapshot::empty(Instant::now(), MIN_INTERVAL)),
            history: Mutex::new(HashMap::new()),
        });
        let backends: Vec<Box<dyn Backend>> = vec![
            Box::new(Pair::new(told)),
            Box::new(Counter::new()),
            Box::new(Broken),
        ];
        let mut worker = Worker::new(backends, fast(4), shared);
        worker.rescan(true);
        worker.disabled_channels = HashSet::from([ChannelId::from_raw("test:pair/b")]);
        worker.apply_disabled();

        let mut lines = Vec::new();
        for _ in 0..(2 * HEALTH_EVERY) {
            worker.tick();
            lines.extend(worker.health.ticks.eq(&0).then_some(worker.tick));
        }
        assert_eq!(
            lines,
            vec![HEALTH_EVERY, 2 * HEALTH_EVERY],
            "costs reset after each line"
        );

        // One more round, checking the line itself.
        let mut line = None;
        for _ in 0..HEALTH_EVERY {
            worker.tick += 1;
            worker.tick_cost = Duration::from_millis(worker.tick % 4);
            line = worker.health_line().or(line);
        }
        let line = line.expect("a line after sixty ticks");
        assert_eq!(line.tick, 3 * HEALTH_EVERY);
        assert_eq!((line.devices, line.channels), (3, 4));
        assert_eq!(
            line.unreadable, 1,
            "the broken channel; not the disabled one"
        );
        assert_eq!(line.disabled, 1);
        assert_eq!(line.failing_backends, vec!["broken"]);
        assert_eq!(line.max_tick, Duration::from_millis(3));
        assert_eq!(line.avg_tick, Duration::from_micros(1500));
        line.log();
    }

    #[test]
    fn resetting_several_channels_clears_each() {
        let told = Arc::new(Mutex::new(Vec::new()));
        let sampler = Sampler::start(vec![Box::new(Pair::new(told))], fast(8));
        let before = wait_until(&sampler, |s| s.tick >= 4);
        sampler.reset_channels([
            ChannelId::from_raw("test:pair/a"),
            ChannelId::from_raw("test:pair/b"),
        ]);
        let after = wait_until(&sampler, |s| {
            s.channels()
                .all(|c| c.stats.min().is_some_and(|m| m > before.tick as f64))
        });
        assert!(after.channels().all(|c| c.stats.count() < after.tick));
    }

    #[test]
    fn history_capacity_changes_apply_to_existing_channels() {
        let sampler = Sampler::start(vec![Box::new(Counter::new())], fast(64));
        wait_until(&sampler, |s| s.tick >= 6);
        let id = ChannelId::from_raw("test:counter/n");
        sampler.set_history_capacity(3);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (len, capacity) = sampler
                .with_history(&id, |h| (h.len(), h.capacity()))
                .unwrap();
            if capacity == 3 {
                assert!(len <= 3);
                break;
            }
            assert!(Instant::now() < deadline, "the capacity never changed");
            thread::sleep(Duration::from_millis(2));
        }
    }
}

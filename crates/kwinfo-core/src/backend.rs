//! The interface every data source implements.

use std::collections::HashSet;
use std::time::Duration;

use crate::model::{Channel, ChannelId, Device, Sample};

/// A source of devices and readings: a sysfs tree, the privileged helper, NVML, or the
/// mock generator. Backends are owned by the sampler thread and never touch the UI.
pub trait Backend: Send {
    /// Short machine name used in logs and diagnostics, e.g. `hwmon`.
    fn name(&self) -> &'static str;

    /// Discovers devices and their channels. Called once at start and again every
    /// [`Backend::rescan_every`] so hot-plugged hardware shows up. Must be idempotent:
    /// returning a device or channel that was already reported is fine.
    fn enumerate(&mut self) -> anyhow::Result<Vec<(Device, Vec<Channel>)>>;

    /// Reads the current value of every channel into `out`. Called every tick; must be
    /// cheap (read known files, no discovery). Channels missing from `out` keep their
    /// previous statistics and get no history point for the tick.
    fn sample(&mut self, out: &mut Vec<Sample>) -> anyhow::Result<()>;

    /// How often to call [`Backend::enumerate`] again. `None` disables rescans.
    fn rescan_every(&self) -> Option<Duration> {
        Some(Duration::from_secs(60))
    }

    /// The channels the user switched off ("Disable monitoring"). The sampler ignores
    /// their samples whatever the backend does, so this is purely an optimisation: a
    /// backend can stop reading them, but must keep reading anything an enabled channel
    /// is computed from. Called after every [`Backend::enumerate`] and whenever the set
    /// changes. The default does nothing.
    fn set_disabled(&mut self, disabled: &HashSet<ChannelId>) {
        let _ = disabled;
    }
}

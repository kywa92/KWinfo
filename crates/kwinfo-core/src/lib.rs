//! Core of KWinfo: the sensor data model, the backend trait, the Linux backends, the
//! sampler thread that turns backend readings into statistics and history, and the mock
//! backend used for development on machines without Linux hardware access.
//!
//! The GUI (`kwinfo-app`) and the command line tool (`kwinfo-cli`) only ever talk to
//! this crate; they never read hardware themselves.

pub mod alerts;
pub mod backend;
pub mod backends;
pub mod config;
pub mod format;
pub mod history;
pub mod logging;
pub mod mock;
pub mod model;
pub mod recording;
pub mod sampler;
pub mod snapshot;
pub mod stats;
pub mod sysroot;

pub use backend::Backend;
pub use backends::{BackendChoice, BackendSet};
pub use config::{Config, ConfigFile, GraphEntry, GraphMode, Graphs, Rgb, ThemeMode};
pub use format::{
    TemperatureUnit, format_input, format_value, from_display, parse_input, to_display,
};
pub use history::History;
pub use model::{
    Category, Channel, ChannelId, Device, DeviceId, Kind, Limits, Sample, Source, Unit,
};
pub use sampler::{ChannelSnapshot, DeviceSnapshot, Sampler, SamplerConfig, Snapshot};
pub use stats::Stats;
pub use sysroot::SysRoot;

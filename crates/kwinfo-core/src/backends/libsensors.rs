//! The libsensors overlay for the `hwmon` backend (cargo feature `libsensors`, Linux).
//!
//! libsensors reads `/etc/sensors3.conf` and `/etc/sensors.d/*.conf`, so a user's
//! existing lm-sensors setup carries over: `label` statements rename rows, `ignore`
//! statements hide them, and `compute` statements (voltage dividers such as +12V) are
//! applied by reading those features through `sensors_get_value()`. libsensors offers no
//! way to ask whether a feature has a compute rule, so every feature of a chip that
//! libsensors knows is read through it; everything else (pwm, freq, chips libsensors
//! does not list) is read from sysfs by the backend itself.

use std::path::{Path, PathBuf};

use lm_sensors::feature::Kind as FeatureKind;
use lm_sensors::value::Kind as ValueKind;
use lm_sensors::{FeatureRef, Initializer, LMSensors};

use super::hwmon::{ChipOverlay, OverlayChip};

/// Routes libsensors configuration errors into KWinfo's log.
#[derive(Debug)]
struct LogListener;

impl lm_sensors::errors::Listener for LogListener {
    fn on_lm_sensors_config_error(
        &self,
        error: &str,
        file_name: Option<&Path>,
        line_number: usize,
    ) {
        let file = file_name.map_or_else(|| "<config>".into(), |f| f.display().to_string());
        tracing::warn!("lm-sensors configuration: {error} at {file}:{line_number}");
    }

    fn on_lm_sensors_fatal_error(&self, error: &str, procedure: &str) {
        tracing::error!("lm-sensors: fatal error {error} in {procedure}");
    }
}

/// The libsensors handle, moved into the sampler thread together with the backend.
struct Handle(LMSensors);

// SAFETY: `LMSensors` is not `Send` only because it keeps the previously installed error
// listener as a raw pointer, to restore it on drop. libsensors has no thread-local
// state, every call the crate makes into it holds the crate's global API lock, and the
// listener installed here is a stateless unit struct. The handle is owned by exactly one
// backend and only used from whichever thread owns that backend, so moving it between
// threads is sound.
#[allow(unsafe_code)]
unsafe impl Send for Handle {}

/// See the module documentation.
pub struct LibSensorsOverlay {
    sensors: Handle,
}

impl LibSensorsOverlay {
    /// Initialises libsensors with the system configuration. `None` (with a log line) if
    /// that fails, for example because another part of the process already did.
    pub fn new() -> Option<Self> {
        match Initializer::default()
            .error_listener(Box::new(LogListener))
            .initialize()
        {
            Ok(sensors) => {
                tracing::debug!(version = sensors.version(), "libsensors initialised");
                Some(Self {
                    sensors: Handle(sensors),
                })
            }
            Err(err) => {
                tracing::info!("libsensors unavailable, using sysfs labels only: {err}");
                None
            }
        }
    }
}

/// The input sub-feature that holds a feature's reading.
fn input_kind(kind: FeatureKind) -> Option<ValueKind> {
    Some(match kind {
        FeatureKind::Voltage => ValueKind::VoltageInput,
        FeatureKind::Fan => ValueKind::FanInput,
        FeatureKind::Temperature => ValueKind::TemperatureInput,
        FeatureKind::Power => ValueKind::PowerInput,
        FeatureKind::Energy => ValueKind::EnergyInput,
        FeatureKind::Current => ValueKind::CurrentInput,
        FeatureKind::Intrusion => ValueKind::IntrusionAlarm,
        _ => return None,
    })
}

fn read_feature(feature: FeatureRef<'_>) -> Option<f64> {
    let kind = input_kind(feature.kind()?)?;
    let sub_feature = feature.sub_feature_by_kind(kind).ok().or_else(|| {
        // Some power meters only provide an average.
        (kind == ValueKind::PowerInput)
            .then(|| feature.sub_feature_by_kind(ValueKind::PowerAverage).ok())
            .flatten()
    })?;
    sub_feature.raw_value().ok()
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

impl ChipOverlay for LibSensorsOverlay {
    fn chip(&mut self, attr_dir: &Path) -> Option<OverlayChip> {
        let wanted = canonical(attr_dir);
        let (handle, chip) = self
            .sensors
            .0
            .chip_iter(None)
            .enumerate()
            .find(|(_, chip)| chip.path().is_some_and(|p| canonical(p) == wanted))?;
        let mut labels = std::collections::HashMap::new();
        for feature in chip.feature_iter() {
            let Some(Ok(name)) = feature.name() else {
                continue;
            };
            if feature.kind().and_then(input_kind).is_none() {
                continue;
            }
            let label = feature.label().unwrap_or_else(|_| name.to_string());
            labels.insert(name.to_string(), label);
        }
        Some(OverlayChip { handle, labels })
    }

    fn read(&mut self, chip: usize, feature: &str) -> Option<f64> {
        let chip = self.sensors.0.chip_iter(None).nth(chip)?;
        let feature = chip
            .feature_iter()
            .find(|f| matches!(f.name(), Some(Ok(name)) if name == feature))?;
        read_feature(feature)
    }
}

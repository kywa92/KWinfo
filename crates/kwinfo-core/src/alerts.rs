//! Alert thresholds with hysteresis (PLAN.md section 2, item 11). An alert starts when a
//! reading crosses a threshold and ends only once the reading is back past the threshold
//! by a margin ([`hysteresis`]), so a reading hovering at the threshold does not start
//! and end an alert on every tick. A missing reading changes nothing.

use std::collections::HashMap;

use crate::config::AlertSettings;
use crate::model::ChannelId;

/// Which threshold a reading crossed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Breach {
    Above,
    Below,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AlertEvent {
    Started {
        channel: ChannelId,
        breach: Breach,
        value: f64,
        threshold: f64,
    },
    Ended {
        channel: ChannelId,
    },
}

/// How far back past its threshold a reading must come for an alert to end: 2 % of the
/// threshold, and at least one step of the sensor's precision (`decimals`).
pub fn hysteresis(threshold: f64, decimals: u8) -> f64 {
    (threshold.abs() * 0.02).max(10f64.powi(-i32::from(decimals)))
}

/// The alerts in progress.
#[derive(Debug, Default)]
pub struct AlertTracker {
    active: HashMap<ChannelId, Breach>,
}

impl AlertTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one reading of `channel`; returns what started or ended.
    pub fn update(
        &mut self,
        channel: &ChannelId,
        value: Option<f64>,
        alert: &AlertSettings,
        decimals: u8,
    ) -> Option<AlertEvent> {
        let value = value.filter(|v| v.is_finite())?;
        match self.active.get(channel).copied() {
            None => {
                let (breach, threshold) = match (alert.above, alert.below) {
                    (Some(above), _) if value > above => (Breach::Above, above),
                    (_, Some(below)) if value < below => (Breach::Below, below),
                    _ => return None,
                };
                self.active.insert(channel.clone(), breach);
                Some(AlertEvent::Started {
                    channel: channel.clone(),
                    breach,
                    value,
                    threshold,
                })
            }
            Some(Breach::Above) => match alert.above {
                Some(above) if value > above - hysteresis(above, decimals) => None,
                _ => self.end(channel),
            },
            Some(Breach::Below) => match alert.below {
                Some(below) if value < below + hysteresis(below, decimals) => None,
                _ => self.end(channel),
            },
        }
    }

    /// Ends the alert of `channel`, if there is one.
    pub fn end(&mut self, channel: &ChannelId) -> Option<AlertEvent> {
        self.active.remove(channel).map(|_| AlertEvent::Ended {
            channel: channel.clone(),
        })
    }

    /// Ends the alerts of channels `keep` rejects (their settings are gone).
    pub fn retain(&mut self, keep: impl Fn(&ChannelId) -> bool) -> Vec<AlertEvent> {
        let gone: Vec<ChannelId> = self.active.keys().filter(|c| !keep(c)).cloned().collect();
        gone.iter().filter_map(|c| self.end(c)).collect()
    }

    pub fn is_active(&self, channel: &ChannelId) -> bool {
        self.active.contains_key(channel)
    }

    pub fn active(&self) -> impl Iterator<Item = (&ChannelId, Breach)> {
        self.active.iter().map(|(c, b)| (c, *b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn above(threshold: f64) -> AlertSettings {
        AlertSettings {
            above: Some(threshold),
            below: None,
            notify: true,
        }
    }

    #[test]
    fn a_reading_hovering_at_the_threshold_does_not_flap() {
        let id = ChannelId::from_raw("cpu:0/temp");
        let alert = above(90.0);
        let mut tracker = AlertTracker::new();
        assert_eq!(tracker.update(&id, Some(89.9), &alert, 1), None);
        assert!(matches!(
            tracker.update(&id, Some(90.5), &alert, 1),
            Some(AlertEvent::Started { breach: Breach::Above, threshold, .. }) if threshold == 90.0
        ));
        assert!(tracker.is_active(&id));
        // 2 % of 90 °C is 1.8 °C: dips to 89.0 and back up change nothing.
        for value in [89.5, 90.4, 89.0, 90.2, 88.3] {
            assert_eq!(tracker.update(&id, Some(value), &alert, 1), None, "{value}");
        }
        assert_eq!(
            tracker.update(&id, None, &alert, 1),
            None,
            "a missing reading"
        );
        assert_eq!(
            tracker.update(&id, Some(88.1), &alert, 1),
            Some(AlertEvent::Ended {
                channel: id.clone()
            })
        );
        assert!(!tracker.is_active(&id));
    }

    #[test]
    fn below_thresholds_and_removed_settings() {
        let id = ChannelId::from_raw("hwmon:x/in1");
        let alert = AlertSettings {
            above: None,
            below: Some(11.4),
            notify: false,
        };
        let mut tracker = AlertTracker::new();
        assert!(matches!(
            tracker.update(&id, Some(11.2), &alert, 3),
            Some(AlertEvent::Started {
                breach: Breach::Below,
                ..
            })
        ));
        assert_eq!(
            tracker.update(&id, Some(11.5), &alert, 3),
            None,
            "within 2 % of 11.4 V"
        );
        assert_eq!(
            tracker.update(&id, Some(11.7), &alert, 3),
            Some(AlertEvent::Ended {
                channel: id.clone()
            })
        );

        tracker.update(&id, Some(11.0), &alert, 3);
        assert_eq!(
            tracker.update(&id, Some(11.0), &AlertSettings::default(), 3),
            Some(AlertEvent::Ended {
                channel: id.clone()
            }),
            "the threshold went away"
        );
        tracker.update(&id, Some(11.0), &alert, 3);
        assert_eq!(
            tracker.retain(|_| false),
            vec![AlertEvent::Ended {
                channel: id.clone()
            }]
        );
        assert_eq!(tracker.active().count(), 0);
    }

    #[test]
    fn hysteresis_has_a_floor() {
        assert!((hysteresis(90.0, 1) - 1.8).abs() < 1e-12);
        assert_eq!(hysteresis(0.0, 1), 0.1);
        assert_eq!(hysteresis(0.5, 0), 1.0);
    }
}

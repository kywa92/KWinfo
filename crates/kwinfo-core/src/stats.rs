//! Running statistics for one channel: current, minimum, maximum and average since the
//! last reset.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    current: Option<f64>,
    min: Option<f64>,
    max: Option<f64>,
    sum: f64,
    count: u64,
}

impl Stats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a reading. `None` (unreadable) and non-finite values update `current`
    /// but never the minimum, maximum or average.
    pub fn update(&mut self, value: Option<f64>) {
        self.current = value;
        let Some(v) = value else { return };
        if !v.is_finite() {
            return;
        }
        self.min = Some(self.min.map_or(v, |m| m.min(v)));
        self.max = Some(self.max.map_or(v, |m| m.max(v)));
        self.sum += v;
        self.count += 1;
    }

    /// Clears minimum, maximum and average. The current value is kept; the statistics
    /// start again from the next reading.
    pub fn reset(&mut self) {
        self.min = None;
        self.max = None;
        self.sum = 0.0;
        self.count = 0;
    }

    pub fn current(&self) -> Option<f64> {
        self.current
    }

    pub fn min(&self) -> Option<f64> {
        self.min
    }

    pub fn max(&self) -> Option<f64> {
        self.max
    }

    pub fn average(&self) -> Option<f64> {
        (self.count > 0).then(|| self.sum / self.count as f64)
    }

    /// Number of readings that went into the minimum, maximum and average.
    pub fn count(&self) -> u64 {
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_min_max_and_average() {
        let mut s = Stats::new();
        assert_eq!(s.current(), None);
        assert_eq!(s.average(), None);

        s.update(Some(40.0));
        s.update(Some(50.0));
        s.update(Some(30.0));

        assert_eq!(s.current(), Some(30.0));
        assert_eq!(s.min(), Some(30.0));
        assert_eq!(s.max(), Some(50.0));
        assert_eq!(s.average(), Some(40.0));
        assert_eq!(s.count(), 3);
    }

    #[test]
    fn unreadable_and_non_finite_values_only_touch_current() {
        let mut s = Stats::new();
        s.update(Some(10.0));
        s.update(None);
        assert_eq!(s.current(), None);
        assert_eq!(s.min(), Some(10.0));
        assert_eq!(s.max(), Some(10.0));
        assert_eq!(s.count(), 1);

        s.update(Some(f64::NAN));
        assert!(s.current().unwrap().is_nan());
        assert_eq!(s.count(), 1);
        assert_eq!(s.average(), Some(10.0));
    }

    #[test]
    fn reset_keeps_current_and_restarts_statistics() {
        let mut s = Stats::new();
        s.update(Some(1.0));
        s.update(Some(9.0));
        s.reset();

        assert_eq!(s.current(), Some(9.0));
        assert_eq!(s.min(), None);
        assert_eq!(s.max(), None);
        assert_eq!(s.average(), None);

        s.update(Some(4.0));
        assert_eq!(s.min(), Some(4.0));
        assert_eq!(s.max(), Some(4.0));
        assert_eq!(s.average(), Some(4.0));
    }
}

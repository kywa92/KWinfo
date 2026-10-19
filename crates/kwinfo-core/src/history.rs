//! A bounded ring buffer of `(time, value)` points that feeds the graph windows.

use std::collections::VecDeque;

/// Time is seconds since the sampler started; values are `f32` to halve memory use. An
/// unreadable sample is stored as `NaN` so that graphs can show a gap.
#[derive(Clone, Debug)]
pub struct History {
    capacity: usize,
    points: VecDeque<(f64, f32)>,
}

impl History {
    /// `capacity` is clamped to at least one point. Memory grows with the points actually
    /// recorded, not with the capacity, so a long history costs nothing until it fills.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            points: VecDeque::new(),
        }
    }

    pub fn push(&mut self, time: f64, value: Option<f64>) {
        if self.points.len() == self.capacity {
            self.points.pop_front();
        }
        let v = value.map_or(f32::NAN, |v| v as f32);
        self.points.push_back((time, v));
    }

    /// Changes the capacity, dropping the oldest points if the buffer shrinks.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(1);
        while self.points.len() > self.capacity {
            self.points.pop_front();
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn clear(&mut self) {
        self.points.clear();
    }

    /// Points from oldest to newest.
    pub fn iter(&self) -> impl Iterator<Item = (f64, f32)> + '_ {
        self.points.iter().copied()
    }

    /// The points at or after `start` (seconds), oldest first, plus the last point
    /// before it, so that a graph's line can enter from its left edge.
    pub fn points_since(&self, start: f64) -> Vec<(f64, f32)> {
        let first = self.points.partition_point(|&(t, _)| t < start);
        self.points
            .range(first.saturating_sub(1)..)
            .copied()
            .collect()
    }

    pub fn latest(&self) -> Option<(f64, f32)> {
        self.points.back().copied()
    }

    /// Minimum and maximum of the readable (non-NaN) values, if any.
    pub fn min_max(&self) -> Option<(f32, f32)> {
        self.points
            .iter()
            .map(|&(_, v)| v)
            .filter(|v| v.is_finite())
            .fold(None, |acc, v| match acc {
                None => Some((v, v)),
                Some((lo, hi)) => Some((lo.min(v), hi.max(v))),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_oldest_when_full() {
        let mut h = History::new(3);
        for i in 0..5 {
            h.push(f64::from(i), Some(f64::from(i) * 10.0));
        }
        assert_eq!(h.len(), 3);
        let times: Vec<f64> = h.iter().map(|(t, _)| t).collect();
        assert_eq!(times, vec![2.0, 3.0, 4.0]);
        assert_eq!(h.latest(), Some((4.0, 40.0)));
    }

    #[test]
    fn min_max_ignores_gaps() {
        let mut h = History::new(10);
        h.push(0.0, Some(5.0));
        h.push(1.0, None);
        h.push(2.0, Some(-2.5));
        h.push(3.0, Some(7.0));
        assert_eq!(h.min_max(), Some((-2.5, 7.0)));

        let mut gaps_only = History::new(2);
        gaps_only.push(0.0, None);
        assert_eq!(gaps_only.min_max(), None);
    }

    #[test]
    fn shrinking_capacity_drops_oldest_points() {
        let mut h = History::new(5);
        for i in 0..5 {
            h.push(f64::from(i), Some(1.0));
        }
        h.set_capacity(2);
        assert_eq!(h.len(), 2);
        assert_eq!(h.iter().next().map(|(t, _)| t), Some(3.0));
        assert_eq!(h.capacity(), 2);

        h.set_capacity(0);
        assert_eq!(h.capacity(), 1);
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn points_since_includes_the_point_before_the_window() {
        let mut h = History::new(10);
        for i in 0..6 {
            h.push(f64::from(i), Some(f64::from(i) * 2.0));
        }
        let times = |points: Vec<(f64, f32)>| points.iter().map(|&(t, _)| t).collect::<Vec<_>>();
        assert_eq!(times(h.points_since(3.5)), vec![3.0, 4.0, 5.0]);
        assert_eq!(times(h.points_since(3.0)), vec![2.0, 3.0, 4.0, 5.0]);
        assert_eq!(
            times(h.points_since(-10.0)),
            vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0]
        );
        assert_eq!(times(h.points_since(99.0)), vec![5.0]);
        assert!(History::new(3).points_since(0.0).is_empty());
    }
}

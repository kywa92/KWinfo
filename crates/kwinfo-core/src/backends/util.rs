//! Small pieces shared by the Linux backends.

use std::time::Instant;

/// Turns a monotonically increasing counter (bytes, sectors, milliseconds) into a rate
/// per second.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Counter {
    prev: Option<(u64, Instant)>,
}

impl Counter {
    /// The rate since the previous reading. `None` for the first reading, when no time
    /// has passed, or when the counter went backwards (a driver reload or a re-created
    /// interface); the next reading then starts afresh from this one.
    pub fn rate(&mut self, value: u64, now: Instant) -> Option<f64> {
        let (old, then) = self.prev.replace((value, now))?;
        let dt = now.checked_duration_since(then)?.as_secs_f64();
        if dt <= 0.0 || value < old {
            return None;
        }
        Some((value - old) as f64 / dt)
    }

    /// The increase since the previous reading, without dividing by time.
    pub fn delta(&mut self, value: u64, now: Instant) -> Option<u64> {
        let (old, _) = self.prev.replace((value, now))?;
        value.checked_sub(old)
    }
}

/// A short, stable, non-reversible tag for an identifier such as a drive's serial number
/// or WWN. Device ids use it so that they stay unique per drive without writing serial
/// numbers into config files, logs or bug reports.
pub(crate) fn short_hash(value: &str) -> String {
    // FNV-1a (64 bit); the low 32 bits are plenty to tell one machine's drives apart.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", hash & 0xffff_ffff)
}

/// Lower-case ASCII words joined by `-`, for readable id parts:
/// `Samsung SSD 970 EVO Plus 1TB` → `samsung-ssd-970-evo-plus-1tb`.
pub(crate) fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Makes a string safe as part of a channel key or device id: no `/`, no whitespace.
pub(crate) fn key_part(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c == '/' || c.is_whitespace() {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// Collapses runs of whitespace into single spaces.
pub(crate) fn squash_spaces(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn counters_give_rates_and_survive_resets() {
        let mut counter = Counter::default();
        let t0 = Instant::now();
        assert_eq!(counter.rate(1_000, t0), None);
        assert_eq!(
            counter.rate(3_000, t0 + Duration::from_secs(2)),
            Some(1_000.0)
        );
        // The counter went backwards: no rate, but the next reading works again.
        assert_eq!(counter.rate(10, t0 + Duration::from_secs(3)), None);
        assert_eq!(
            counter.rate(510, t0 + Duration::from_millis(3_500)),
            Some(1_000.0)
        );
        // No time passed.
        assert_eq!(counter.rate(600, t0 + Duration::from_millis(3_500)), None);
    }

    #[test]
    fn ids_are_slugged_and_hashed() {
        assert_eq!(
            slug("Samsung SSD 970 EVO Plus 1TB"),
            "samsung-ssd-970-evo-plus-1tb"
        );
        assert_eq!(slug("  WDC  WDS480G2G0C-00AJM0 "), "wdc-wds480g2g0c-00ajm0");
        assert_eq!(short_hash("S4EWNX0R123456A"), short_hash("S4EWNX0R123456A"));
        assert_ne!(short_hash("S4EWNX0R123456A"), short_hash("S4EWNX0R123456B"));
        assert_eq!(short_hash("").len(), 8);
        assert_eq!(key_part("a b/c"), "a_b_c");
        assert_eq!(squash_spaces(" Intel  Core   i7 "), "Intel Core i7");
    }
}

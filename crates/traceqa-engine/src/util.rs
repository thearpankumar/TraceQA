//! Small helpers shared by the engine's modules.

use std::time::Duration;

/// Whole milliseconds in `d`, rounded down. A duration too long for a `u64` (about 584 million
/// years) saturates instead of wrapping.
pub(crate) fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ms_saturates_and_rounds_down() {
        assert_eq!(ms(Duration::ZERO), 0);
        assert_eq!(ms(Duration::from_micros(1999)), 1);
        assert_eq!(ms(Duration::from_millis(1500)), 1500);
        // u64::MAX seconds is far more milliseconds than a u64 holds.
        let huge = Duration::new(u64::MAX, 999_999_999);
        assert_eq!(ms(huge), u64::MAX);
    }
}

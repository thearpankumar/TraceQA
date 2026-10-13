//! Timestamps and run ids, without a date library.

use std::sync::atomic::{AtomicU32, Ordering};

use crate::unix_time;

/// The current time as an RFC 3339 UTC timestamp, to the second.
pub fn now_rfc3339() -> String {
    let (date, time) = split(unix_time().0);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        date.0, date.1, date.2, time.0, time.1, time.2
    )
}

/// A run id that sorts by start time: `r_YYYYMMDD_HHMMSS_xxxx`.
pub fn new_run_id() -> String {
    // Runs started in the same instant by one process must still differ.
    static STARTED: AtomicU32 = AtomicU32::new(0);
    let (seconds, nanos) = unix_time();
    let (date, time) = split(seconds);
    let salt = (nanos / 1_000)
        ^ std::process::id().rotate_left(8)
        ^ STARTED.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9e37);
    let salt = salt & 0xffff;
    format!(
        "r_{:04}{:02}{:02}_{:02}{:02}{:02}_{salt:04x}",
        date.0, date.1, date.2, time.0, time.1, time.2
    )
}

type Date = (i64, i64, i64);
type Time = (u64, u64, u64);

fn split(unix_seconds: u64) -> (Date, Time) {
    let seconds_of_day = unix_seconds % 86_400;
    let date = civil_from_days((unix_seconds / 86_400) as i64);
    (
        date,
        (seconds_of_day / 3_600, seconds_of_day % 3_600 / 60, seconds_of_day % 60),
    )
}

/// Days since 1970-01-01 to (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(days: i64) -> Date {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_instants() {
        assert_eq!(split(0), ((1970, 1, 1), (0, 0, 0)));
        assert_eq!(split(951_782_400), ((2000, 2, 29), (0, 0, 0)));
        assert_eq!(split(1_700_000_000), ((2023, 11, 14), (22, 13, 20)));
        assert_eq!(split(1_735_689_599), ((2024, 12, 31), (23, 59, 59)));
    }

    #[test]
    fn run_ids_have_the_documented_shape() {
        let id = new_run_id();
        assert_eq!(id.len(), "r_20261013_210455_a1b2".len());
        assert!(id.starts_with("r_20"));
    }
}

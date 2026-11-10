//! Times as tay records them, in whole seconds since 1970, and writes them,
//! in UTC.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The time now, in seconds since 1970.
pub fn now() -> u64 {
    seconds(SystemTime::now())
}

/// `time` in seconds since 1970; a time before then is 1970.
pub fn seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

/// `seconds` since 1970 as a person reads it: `2026-11-09 22:41:03 UTC`.
pub fn utc(seconds: u64) -> String {
    let (days, rest) = (seconds / 86400, seconds % 86400);
    let (year, month, day) = civil(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The calendar date `days` after 1970-01-01, by the proleptic Gregorian
/// calendar, as Howard Hinnant's `civil_from_days` works it out.
fn civil(days: u64) -> (u64, u64, u64) {
    let days = days + 719_468;
    let era = days / 146_097;
    let day_of_era = days % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_written_in_utc() {
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00:00 UTC");
        assert_eq!(utc(1_794_292_971), "2026-11-10 06:42:51 UTC");
    }
}

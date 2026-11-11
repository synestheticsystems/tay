//! Times as tay records them, in seconds or milliseconds since 1970, and
//! writes them, in UTC; and how long something took, in seconds.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The time now, in seconds since 1970.
pub fn now() -> u64 {
    seconds(SystemTime::now())
}

/// `time` in seconds since 1970; a time before then is 1970.
pub fn seconds(time: SystemTime) -> u64 {
    since_1970(time).as_secs()
}

/// The time now, in milliseconds since 1970.
pub fn now_millis() -> u64 {
    u64::try_from(since_1970(SystemTime::now()).as_millis()).unwrap_or(u64::MAX)
}

fn since_1970(time: SystemTime) -> Duration {
    time.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO)
}

/// How long `length` is, in seconds to the millisecond, as tay says how
/// long something took.
pub fn in_seconds(length: Duration) -> f64 {
    (length.as_secs_f64() * 1000.0).round() / 1000.0
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

/// `millis` since 1970 as a line of a unit's log begins with it:
/// `2026-11-09T22:41:03.250Z`. Every such time is as long as any other, so
/// that the times of lines sort as their text does.
pub fn stamp(millis: u64) -> String {
    let seconds = millis / 1000;
    let (days, rest) = (seconds / 86400, seconds % 86400);
    let (year, month, day) = civil(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60,
        millis % 1000
    )
}

/// A time written in UTC as `2026-11-10T11:56:38.480328673Z`, to the
/// second or to any fraction of it, as Docker writes the times it keeps, in
/// nanoseconds since 1970; `None` for text that is not such a time, or one
/// before 1970.
pub fn nanos_of(text: &str) -> Option<u64> {
    let (date, time) = text.strip_suffix('Z')?.split_once('T')?;
    let number = |text: &str, digits: usize| {
        (text.len() == digits && text.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| text.parse::<u64>().ok())
            .flatten()
    };
    let mut date = date.split('-');
    let (year, month, day) = (
        number(date.next()?, 4)?,
        number(date.next()?, 2)?,
        number(date.next()?, 2)?,
    );
    if date.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (clock, fraction) = time.split_once('.').unwrap_or((time, ""));
    let mut clock = clock.split(':');
    let (hours, minutes, seconds) = (
        number(clock.next()?, 2)?,
        number(clock.next()?, 2)?,
        number(clock.next()?, 2)?,
    );
    if clock.next().is_some() || hours > 23 || minutes > 59 || seconds > 60 {
        return None;
    }
    if fraction.len() > 9 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let nanos = format!("{fraction:0<9}").parse::<u64>().ok()?;
    let days = days_from_civil(year, month, day)?;
    let seconds = days * 86400 + hours * 3600 + minutes * 60 + seconds;
    seconds.checked_mul(1_000_000_000)?.checked_add(nanos)
}

/// The days from 1970-01-01 to a calendar date, by the proleptic Gregorian
/// calendar, as Howard Hinnant's `days_from_civil` works them out; `None`
/// before 1970.
fn days_from_civil(year: u64, month: u64, day: u64) -> Option<u64> {
    let year = if month <= 2 {
        year.checked_sub(1)?
    } else {
        year
    };
    let era = year / 400;
    let year_of_era = year % 400;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    (era * 146_097 + day_of_era).checked_sub(719_468)
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
        assert_eq!(stamp(1_794_292_971_005), "2026-11-10T06:42:51.005Z");
    }

    #[test]
    fn a_time_docker_writes_is_read_to_the_nanosecond() {
        assert_eq!(
            nanos_of("2026-11-10T06:42:51.005123456Z"),
            Some(1_794_292_971_005_123_456)
        );
        assert_eq!(
            nanos_of("2000-02-29T00:00:00Z"),
            Some(951_782_400_000_000_000)
        );
        assert_eq!(nanos_of("1970-01-01T00:00:00.5Z"), Some(500_000_000));
        // What it writes is what the log's own times say, to the
        // millisecond.
        let read = nanos_of(&stamp(1_794_292_971_005)).unwrap();
        assert_eq!(read / 1_000_000, 1_794_292_971_005);
        for not in [
            "2026-11-10 06:42:51Z",
            "2026-13-10T06:42:51Z",
            "1969-12-31T23:59:59Z",
            "2026-11-10T06:42:51.0000000001Z",
            "2026-11-10T06:42:51+01:00",
        ] {
            assert_eq!(nanos_of(not), None, "{not}");
        }
    }
}

//! Turning a moment into text.
//!
//! Two callers want the same arithmetic in different shapes: the log wants
//! `20260904 21:14:03` on every line, and a backup wants a folder name that
//! sorts and reads. Neither is worth a calendar library — this is one function
//! and a pair of formats — but two copies of a civil-calendar conversion would
//! be two chances to get a leap year wrong.
//!
//! Everything here is UTC. A log read next to a `ls -l` should agree with it,
//! and a local time that shifts twice a year is a log whose lines can appear to
//! go backwards.

/// Seconds since the epoch, now.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A moment, taken apart.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Parts {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// Split seconds since the epoch into a date and a time.
///
/// Howard Hinnant's civil-from-days, which is exact for every year this will
/// ever be handed and is shorter than the comment explaining why a calendar
/// dependency would not be.
pub fn parts(seconds: u64) -> Parts {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { year + 1 } else { year };

    Parts {
        year,
        month,
        day,
        hour: (rest / 3_600) as u32,
        minute: ((rest / 60) % 60) as u32,
        second: (rest % 60) as u32,
    }
}

/// `20260904 21:14:03` — what every log line starts with.
pub fn stamp(seconds: u64) -> String {
    let at = parts(seconds);
    format!(
        "{:04}{:02}{:02} {:02}:{:02}:{:02}",
        at.year, at.month, at.day, at.hour, at.minute, at.second
    )
}

/// `2026-09-04 211403` — a folder name for a moment: sortable, and readable
/// without a tool.
pub fn folder(seconds: u64) -> String {
    let at = parts(seconds);
    format!(
        "{:04}-{:02}-{:02} {:02}{:02}{:02}",
        at.year, at.month, at.day, at.hour, at.minute, at.second
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_is_where_it_should_be() {
        assert_eq!(stamp(0), "19700101 00:00:00");
        assert_eq!(folder(0), "1970-01-01 000000");
    }

    #[test]
    fn a_known_moment_comes_out_right() {
        // 2026-09-04T21:14:03Z, checked against `date -u -d @1788556443`.
        assert_eq!(stamp(1_788_556_443), "20260904 21:14:03");
    }

    #[test]
    fn a_leap_day_is_a_day() {
        // 2024-02-29T00:00:00Z.
        assert_eq!(stamp(1_709_164_800), "20240229 00:00:00");
        // And the day after it is the first of March.
        assert_eq!(stamp(1_709_164_800 + 86_400), "20240301 00:00:00");
    }

    #[test]
    fn a_century_that_is_not_a_leap_year_is_not_one() {
        // 2100-02-28T00:00:00Z, then the next day, which is March in a
        // calendar that knows 2100 is not a leap year.
        let feb_28 = 4_107_456_000;
        assert_eq!(stamp(feb_28), "21000228 00:00:00");
        assert_eq!(stamp(feb_28 + 86_400), "21000301 00:00:00");
    }

    #[test]
    fn the_time_of_day_is_carried_through() {
        assert_eq!(stamp(86_399), "19700101 23:59:59");
        assert_eq!(parts(3_723).hour, 1);
        assert_eq!(parts(3_723).minute, 2);
        assert_eq!(parts(3_723).second, 3);
    }
}

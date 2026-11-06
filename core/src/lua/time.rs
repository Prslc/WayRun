use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// A UTC civil date, the fields `wayrun.date` hands a script.
pub(super) struct Date {
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
    pub second: i64,
}

/// Unix seconds, the host clock a signature or a cache TTL needs.
pub(super) fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// UTC civil fields from Unix seconds; the sandbox has no timezone database.
pub(super) fn date(seconds: i64) -> Date {
    let days = seconds.div_euclid(86_400);
    let secs = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    Date {
        year,
        month,
        day,
        hour: secs / 3_600,
        minute: secs % 3_600 / 60,
        second: secs % 60,
    }
}

#[cfg(test)]
mod tests {
    use super::date;

    #[test]
    fn unix_seconds_convert_to_utc_civil_fields() {
        let epoch = date(0);
        assert_eq!(
            (epoch.year, epoch.month, epoch.day, epoch.hour),
            (1970, 1, 1, 0)
        );
        assert_eq!((epoch.minute, epoch.second), (0, 0));

        let before = date(-1);
        assert_eq!((before.year, before.month, before.day), (1969, 12, 31));
        assert_eq!((before.hour, before.minute, before.second), (23, 59, 59));

        let leap = date(1_582_934_400);
        assert_eq!((leap.year, leap.month, leap.day), (2020, 2, 29));

        let known = date(1_600_000_000);
        assert_eq!(
            (
                known.year,
                known.month,
                known.day,
                known.hour,
                known.minute,
                known.second,
            ),
            (2020, 9, 13, 12, 26, 40)
        );
    }
}

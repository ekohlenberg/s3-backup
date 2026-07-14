//! Minimal UTC date/time formatting with no external date/time crate.
//!
//! We only need two things anywhere in this program: an ISO 8601 timestamp
//! for the bucket manifest / metadata, and an AWS SigV4-style
//! `YYYYMMDDTHHMMSSZ` / `YYYYMMDD` pair for request signing. Both are derived
//! from a Unix timestamp using Howard Hinnant's well-known civil-from-days
//! algorithm (public domain), which keeps this dependency-free per the
//! project's "minimize dependencies" goal.

use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Civil {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// Days since the Unix epoch -> (year, month, day). See
/// http://howardhinnant.github.io/date_algorithms.html#civil_from_days
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

pub fn civil_from_unix(secs: i64) -> Civil {
    let days = secs.div_euclid(86400);
    let mut rem = secs.rem_euclid(86400);
    let hour = (rem / 3600) as u32;
    rem -= (hour as i64) * 3600;
    let minute = (rem / 60) as u32;
    let second = (rem % 60) as u32;
    let (year, month, day) = civil_from_days(days);
    Civil {
        year,
        month,
        day,
        hour,
        minute,
        second,
    }
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before 1970-01-01")
        .as_secs() as i64
}

/// `2026-07-13T18:42:00Z`
pub fn iso8601_now() -> String {
    iso8601(now_unix())
}

pub fn iso8601(unix_secs: i64) -> String {
    let c = civil_from_unix(unix_secs);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        c.year, c.month, c.day, c.hour, c.minute, c.second
    )
}

/// `(amz_date, date_stamp)` = (`20260713T184200Z`, `20260713`), as used by
/// SigV4's `x-amz-date` header and credential scope respectively.
pub fn amz_date_now() -> (String, String) {
    amz_date(now_unix())
}

pub fn amz_date(unix_secs: i64) -> (String, String) {
    let c = civil_from_unix(unix_secs);
    let amz = format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        c.year, c.month, c.day, c.hour, c.minute, c.second
    );
    let date_stamp = format!("{:04}{:02}{:02}", c.year, c.month, c.day);
    (amz, date_stamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_zero_is_1970_01_01() {
        let c = civil_from_unix(0);
        assert_eq!((c.year, c.month, c.day, c.hour, c.minute, c.second), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn known_timestamp_matches() {
        // 2026-07-13T18:42:00Z, from the migration notes' example manifest.
        // Verified against `date -u -d "2026-07-13T18:42:00Z" +%s`.
        let unix = 1783968120i64;
        assert_eq!(iso8601(unix), "2026-07-13T18:42:00Z");
        let (amz, ds) = amz_date(unix);
        assert_eq!(amz, "20260713T184200Z");
        assert_eq!(ds, "20260713");
    }

    #[test]
    fn leap_day_2024() {
        // 2024-02-29T00:00:00Z = unix 1709164800
        let c = civil_from_unix(1709164800);
        assert_eq!((c.year, c.month, c.day), (2024, 2, 29));
    }

    #[test]
    fn round_trip_is_monotonic() {
        let mut prev = civil_from_unix(0);
        for t in (0..10_000_000i64).step_by(3_600) {
            let c = civil_from_unix(t);
            assert!(
                (c.year, c.month, c.day, c.hour, c.minute, c.second)
                    >= (prev.year, prev.month, prev.day, prev.hour, prev.minute, prev.second)
            );
            prev = c;
        }
    }
}

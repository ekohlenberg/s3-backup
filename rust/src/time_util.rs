//! Minimal UTC + local date/time formatting with no external date/time crate.
//!
//! We only need three things anywhere in this program: an ISO 8601 UTC
//! timestamp for the bucket manifest / object metadata, an AWS SigV4-style
//! `YYYYMMDDTHHMMSSZ` / `YYYYMMDD` pair for request signing, and a
//! local-time ISO 8601 timestamp for log lines (`logging.rs`). All three are
//! derived from a Unix timestamp using Howard Hinnant's well-known
//! civil-from-days algorithm (public domain), which keeps this
//! dependency-free per the project's "minimize dependencies" goal -- no
//! `chrono`/`time` crate, and no hand-rolled libc/WinAPI FFI either.
//!
//! Object metadata and the bucket manifest deliberately keep UTC
//! (`iso8601_now`/`iso8601`): backups can run from hosts in different time
//! zones sharing one bucket, and UTC keeps those timestamps directly
//! comparable. Only the log timestamps (`local_iso8601_now`) use local time,
//! for the person reading the log.

use std::sync::OnceLock;
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

/// Local-time ISO 8601 with a UTC offset suffix, e.g.
/// `2026-07-23T14:32:10-07:00`. Used only for log lines (`logging.rs`) --
/// see the module-level doc comment for why object metadata/the manifest
/// stay on UTC instead.
pub fn local_iso8601_now() -> String {
    format_with_offset(now_unix(), local_utc_offset_seconds())
}

/// Formats `unix_secs` as if it were UTC but `offset_seconds` east of it,
/// appending that offset as the ISO 8601 suffix. Splitting the pure
/// date-math from the "what's my machine's current offset" lookup below
/// keeps this half unit-testable without depending on the test runner's own
/// time zone.
fn format_with_offset(unix_secs: i64, offset_seconds: i64) -> String {
    let c = civil_from_unix(unix_secs + offset_seconds);
    let (sign, magnitude) = if offset_seconds >= 0 {
        ('+', offset_seconds)
    } else {
        ('-', -offset_seconds)
    };
    let offset_hours = magnitude / 3600;
    let offset_minutes = (magnitude % 3600) / 60;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{sign}{:02}:{:02}",
        c.year, c.month, c.day, c.hour, c.minute, c.second, offset_hours, offset_minutes
    )
}

/// Best-effort local UTC offset in seconds (positive east of UTC), cached
/// after the first call -- a CLI run is short enough that the offset isn't
/// expected to change mid-run, and re-deriving it on every log line would
/// mean a process spawn per line. Falls back to 0 (i.e. UTC) if it can't be
/// determined, same "degrade gracefully rather than fail the run" posture
/// as `config::hostname_fallback`.
///
/// std has no portable "local UTC offset" API and this project avoids
/// pulling in a date/time crate (see the module doc comment) or hand-rolling
/// libc/WinAPI FFI bindings just for this, so -- mirroring
/// `config::hostname_fallback`'s use of the `hostname` command -- this
/// shells out to a platform utility that already knows the answer.
fn local_utc_offset_seconds() -> i64 {
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(detect_local_utc_offset_seconds)
}

/// `date +%z` prints the local zone's current UTC offset as e.g. `-0700` or
/// `+0000` -- supported by both the macOS/BSD and GNU/Linux `date` builds.
#[cfg(unix)]
fn detect_local_utc_offset_seconds() -> i64 {
    std::process::Command::new("date")
        .arg("+%z")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| parse_offset_hhmm(s.trim()))
        .unwrap_or(0)
}

/// Parses the `date +%z` format (`[+-]HHMM`) into signed seconds.
#[cfg(unix)]
fn parse_offset_hhmm(s: &str) -> Option<i64> {
    if s.len() != 5 {
        return None;
    }
    let sign = match s.as_bytes()[0] {
        b'+' => 1i64,
        b'-' => -1i64,
        _ => return None,
    };
    let hours: i64 = s.get(1..3)?.parse().ok()?;
    let minutes: i64 = s.get(3..5)?.parse().ok()?;
    Some(sign * (hours * 3600 + minutes * 60))
}

/// PowerShell's `[TimeZoneInfo]::Local.GetUtcOffset(...)` -- unlike
/// `.BaseUtcOffset`, this reflects whatever DST is in effect right now --
/// prints as a `TimeSpan`, e.g. `-07:00:00` or `05:30:00` (no leading `+`
/// for a non-negative span).
#[cfg(windows)]
fn detect_local_utc_offset_seconds() -> i64 {
    std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "[System.TimeZoneInfo]::Local.GetUtcOffset([System.DateTime]::Now).ToString()",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| parse_offset_hhmmss(s.trim()))
        .unwrap_or(0)
}

/// Parses a .NET `TimeSpan.ToString()` of the form `[-]HH:MM:SS` (no `+` for
/// non-negative) into signed seconds. The seconds component is parsed but
/// ignored -- UTC offsets are only ever whole minutes.
#[cfg(windows)]
fn parse_offset_hhmmss(s: &str) -> Option<i64> {
    let (sign, rest) = match s.strip_prefix('-') {
        Some(rest) => (-1i64, rest),
        None => (1i64, s),
    };
    let mut parts = rest.split(':');
    let hours: i64 = parts.next()?.parse().ok()?;
    let minutes: i64 = parts.next()?.parse().ok()?;
    Some(sign * (hours * 3600 + minutes * 60))
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

    #[test]
    fn format_with_offset_shifts_the_clock_and_appends_the_suffix() {
        // Same 2026-07-13T18:42:00Z instant as known_timestamp_matches,
        // reformatted at a few different offsets. This exercises the pure
        // date math directly rather than local_iso8601_now(), since the
        // latter depends on whatever zone the machine running the test
        // happens to be in.
        let unix = 1783968120i64;
        assert_eq!(format_with_offset(unix, 0), "2026-07-13T18:42:00+00:00");
        assert_eq!(format_with_offset(unix, -7 * 3600), "2026-07-13T11:42:00-07:00");
        // Positive, non-whole-hour offset, and one that crosses into the
        // next day.
        assert_eq!(
            format_with_offset(unix, 5 * 3600 + 30 * 60),
            "2026-07-14T00:12:00+05:30"
        );
    }

    #[cfg(unix)]
    #[test]
    fn parse_offset_hhmm_handles_sign_zero_and_garbage() {
        assert_eq!(parse_offset_hhmm("+0000"), Some(0));
        assert_eq!(parse_offset_hhmm("-0700"), Some(-7 * 3600));
        assert_eq!(parse_offset_hhmm("+0530"), Some(5 * 3600 + 30 * 60));
        assert_eq!(parse_offset_hhmm("garbage"), None);
        assert_eq!(parse_offset_hhmm(""), None);
    }

    #[cfg(windows)]
    #[test]
    fn parse_offset_hhmmss_handles_sign_zero_and_garbage() {
        assert_eq!(parse_offset_hhmmss("00:00:00"), Some(0));
        assert_eq!(parse_offset_hhmmss("-07:00:00"), Some(-7 * 3600));
        assert_eq!(parse_offset_hhmmss("05:30:00"), Some(5 * 3600 + 30 * 60));
        assert_eq!(parse_offset_hhmmss("garbage"), None);
        assert_eq!(parse_offset_hhmmss(""), None);
    }
}

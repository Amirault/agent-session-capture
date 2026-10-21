//! The `Date` behaviour the tool relies on: `Date.parse` of ISO-8601 text,
//! `new Date(ms).toISOString()` and `Date.now()`.

use std::time::{SystemTime, UNIX_EPOCH};

const MS_PER_DAY: f64 = 86_400_000.0;
/// ECMAScript TimeClip bound.
const MAX_TIME: f64 = 8.64e15;

/// `Date.now()`.
pub fn now_ms() -> f64 {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    elapsed.as_millis() as f64
}

/// Days since 1970-01-01 of a proleptic Gregorian civil date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// ECMAScript TimeClip: the time value `new Date(ms)` holds (NaN when out of range).
pub fn time_clip(ms: f64) -> f64 {
    if !ms.is_finite() || ms.abs() > MAX_TIME {
        return f64::NAN;
    }
    ms.trunc() + 0.0
}

/// `new Date(ms).toISOString()`; `None` where JavaScript throws
/// `RangeError: Invalid time value`.
pub fn to_iso_string(ms: f64) -> Option<String> {
    let t = time_clip(ms);
    if t.is_nan() {
        return None;
    }
    let t = t as i64;
    let days = t.div_euclid(MS_PER_DAY as i64);
    let in_day = t.rem_euclid(MS_PER_DAY as i64);
    let (year, month, day) = civil_from_days(days);
    let (h, m, s, ms) = (
        in_day / 3_600_000,
        in_day / 60_000 % 60,
        in_day / 1000 % 60,
        in_day % 1000,
    );
    let year_text = if (0..=9999).contains(&year) {
        format!("{year:04}")
    } else if year < 0 {
        format!("-{:06}", -year)
    } else {
        format!("+{year:06}")
    };
    Some(format!("{year_text}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}.{ms:03}Z"))
}

/// The ISO-8601 forms V8's `Date.parse` accepts: `YYYY`, `YYYY-MM`,
/// `YYYY-MM-DD` (or a signed 6-digit year), optionally followed by
/// `THH:mm[:ss[.fff…]]` and `Z` or a `±HH:mm` / `±HHmm` offset; `T` and `Z`
/// in either case. A date alone is UTC; a date-time without a zone is local
/// time in JavaScript and is read as UTC here (the tool never builds one).
pub fn parse(text: &str) -> f64 {
    parse_iso(text.as_bytes()).unwrap_or(f64::NAN)
}

fn digits(s: &[u8], at: usize, n: usize) -> Option<i64> {
    let slice = s.get(at..at + n)?;
    if !slice.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(slice.iter().fold(0i64, |acc, d| acc * 10 + i64::from(d - b'0')))
}

fn parse_iso(s: &[u8]) -> Option<f64> {
    let mut i;
    let year = match s.first()? {
        b'+' | b'-' => {
            let y = digits(s, 1, 6)?;
            if s[0] == b'-' && y == 0 {
                return None;
            }
            i = 7;
            if s[0] == b'-' { -y } else { y }
        }
        _ => {
            i = 4;
            digits(s, 0, 4)?
        }
    };
    let (mut month, mut day) = (1, 1);
    if s.get(i) == Some(&b'-') {
        month = digits(s, i + 1, 2)?;
        i += 3;
        if s.get(i) == Some(&b'-') {
            day = digits(s, i + 1, 2)?;
            i += 3;
        }
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let date_days = days_from_civil(year, month, day) as f64;
    if i == s.len() {
        return Some(time_clip(date_days * MS_PER_DAY)).filter(|t| !t.is_nan());
    }
    if !matches!(s.get(i), Some(b'T' | b't')) {
        return None;
    }
    let hour = digits(s, i + 1, 2)?;
    if s.get(i + 3) != Some(&b':') {
        return None;
    }
    let minute = digits(s, i + 4, 2)?;
    i += 6;
    let (mut second, mut millis) = (0, 0);
    if s.get(i) == Some(&b':') {
        second = digits(s, i + 1, 2)?;
        i += 3;
        if s.get(i) == Some(&b'.') {
            let start = i + 1;
            let mut end = start;
            while s.get(end).is_some_and(u8::is_ascii_digit) {
                end += 1;
            }
            if end == start {
                return None;
            }
            let fraction = &s[start..end.min(start + 3)];
            millis = fraction.iter().fold(0i64, |acc, d| acc * 10 + i64::from(d - b'0'))
                * 10i64.pow(3 - fraction.len() as u32);
            i = end;
        }
    }
    if hour > 24 || minute > 59 || second > 59 || (hour == 24 && (minute, second, millis) != (0, 0, 0)) {
        return None;
    }
    let mut offset_minutes = 0;
    match s.get(i) {
        None => {}
        Some(b'Z' | b'z') => {
            i += 1;
        }
        Some(&sign @ (b'+' | b'-')) => {
            let oh = digits(s, i + 1, 2)?;
            let om = if s.get(i + 3) == Some(&b':') {
                let v = digits(s, i + 4, 2)?;
                i += 6;
                v
            } else {
                let v = digits(s, i + 3, 2)?;
                i += 5;
                v
            };
            if oh > 23 || om > 59 {
                return None;
            }
            offset_minutes = (oh * 60 + om) * if sign == b'+' { 1 } else { -1 };
        }
        Some(_) => return None,
    }
    if i != s.len() {
        return None;
    }
    let ms = date_days * MS_PER_DAY + (hour * 3_600_000 + minute * 60_000 + second * 1000 + millis) as f64
        - (offset_minutes * 60_000) as f64;
    Some(time_clip(ms)).filter(|t| !t.is_nan())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_like_v8() {
        let cases = [
            ("2026-07-01T10:00:00Z", 1782900000000.0),
            ("2026-07-01T10:00Z", 1782900000000.0),
            ("2026-07-01T10:00:00+0200", 1782892800000.0),
            ("2026-07-01T10:00:00.12345678901Z", 1782900000123.0),
            ("2026-02-31T00:00:00Z", 1772496000000.0),
            ("2026-07-01T24:00:00Z", 1782950400000.0),
            ("2026-07-01T10:00:00+23:59", 1782813660000.0),
            ("2026-07-01T10:00:00-0530", 1782919800000.0),
            ("0000-01-01T00:00:00Z", -62167219200000.0),
            ("2026-02-30", 1772409600000.0),
            ("2026-07-01t10:00:00z", 1782900000000.0),
        ];
        for (text, expected) in cases {
            assert_eq!(parse(text), expected, "{text}");
        }
        for bad in [
            "2026-07-01T10Z",
            "2026-07-01T10:00:00+02",
            "2026-02-32T00:00:00Z",
            "2026-07-01T24:00:01Z",
            "2026-07-01T23:59:60Z",
            "2026-07-01T10:00:00+24:00",
            "2026-07-01T10:00:00ZZ",
            "2026-07-01T10:00:00.Z",
            "2026-13-01",
            "bad date",
            "",
        ] {
            assert!(parse(bad).is_nan(), "{bad}");
        }
    }

    #[test]
    fn prints_iso_strings() {
        assert_eq!(to_iso_string(1792600450355.0).unwrap(), "2026-10-21T16:34:10.355Z");
        assert_eq!(to_iso_string(-62167219200000.0).unwrap(), "0000-01-01T00:00:00.000Z");
        assert_eq!(to_iso_string(1784000000123.456).unwrap(), "2026-07-14T03:33:20.123Z");
        assert!(to_iso_string(f64::NAN).is_none());
    }
}

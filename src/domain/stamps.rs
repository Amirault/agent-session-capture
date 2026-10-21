//! Pure timestamp parsing shared by the session-mode adapters and the CLI.
//!
//! Two formats matter:
//! - source-native stamps (Warp/SQLite: UTC "YYYY-MM-DD HH:MM:SS(.ffffff)"),
//!   compared for skip-unchanged as RAW strings (no parsing) but parsed as UTC
//!   for the `--since` cutoff;
//! - `--since` values (ISO-8601 date or datetime), validated by the CLI (exit 2
//!   when unparseable) and parsed to the same UTC epoch scale.

use crate::js::date;
use crate::js::string::js_trim;

/// Parse a SQLite-style UTC stamp ("2026-06-30 12:00:00", fraction optional,
/// `T` or space) as epoch milliseconds; `None` when the shape does not match.
/// Fractional seconds beyond milliseconds are truncated.
pub fn parse_stamp_utc(stamp: &str) -> Option<f64> {
    let s = js_trim(stamp).as_bytes();
    let digit = |i: usize| s.get(i).is_some_and(u8::is_ascii_digit);
    let shape = s.len() >= 19
        && (0..4).all(digit)
        && s[4] == b'-'
        && digit(5)
        && digit(6)
        && s[7] == b'-'
        && digit(8)
        && digit(9)
        && (s[10] == b'T' || s[10] == b' ')
        && digit(11)
        && digit(12)
        && s[13] == b':'
        && digit(14)
        && digit(15)
        && s[16] == b':'
        && digit(17)
        && digit(18);
    if !shape {
        return None;
    }
    let fraction = if s.len() == 19 {
        ""
    } else if s[19] == b'.' && s.len() > 20 && s[20..].iter().all(u8::is_ascii_digit) {
        std::str::from_utf8(&s[20..]).ok()?
    } else {
        return None;
    };
    let mut millis: String = fraction.chars().take(3).collect();
    while millis.len() < 3 {
        millis.push('0');
    }
    let text = format!(
        "{}T{}.{}Z",
        std::str::from_utf8(&s[..10]).ok()?,
        std::str::from_utf8(&s[11..19]).ok()?,
        millis
    );
    let epoch = date::parse(&text);
    if epoch.is_nan() { None } else { Some(epoch) }
}

fn is_date_only(b: &[u8]) -> bool {
    b.len() == 10
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit)
}

fn has_zone_suffix(value: &str) -> bool {
    let b = value.as_bytes();
    if value.ends_with('Z') {
        return true;
    }
    let n = b.len();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    // [+-]\d{2}:?\d{2}$
    (n >= 6 && (b[n - 6] == b'+' || b[n - 6] == b'-') && digits(n - 5..n - 3) && b[n - 3] == b':' && digits(n - 2..n))
        || (n >= 5 && (b[n - 5] == b'+' || b[n - 5] == b'-') && digits(n - 4..n))
}

/// Parse a `--since` value (ISO-8601 date or datetime) as UTC epoch
/// milliseconds; `None` when unparseable (a CLI usage error).
///
/// A bare date means midnight UTC — and, as in the original tool, a bare date
/// that is not a real date (`2026-13-01`) is not rejected but yields `NaN`,
/// which no stamp passes. A datetime without a zone suffix is read as UTC;
/// `Z` and `±HH:MM` suffixes are honored.
pub fn parse_since_utc(value: &str) -> Option<f64> {
    if is_date_only(value.as_bytes()) {
        return Some(date::parse(&format!("{value}T00:00:00.000Z")));
    }
    let b = value.as_bytes();
    if !(b.len() >= 11 && is_date_only(&b[..10]) && b[10] == b'T') {
        return None;
    }
    let with_zone = if has_zone_suffix(value) {
        value.to_string()
    } else {
        format!("{value}Z")
    };
    let epoch = date::parse(&with_zone);
    if epoch.is_nan() { None } else { Some(epoch) }
}

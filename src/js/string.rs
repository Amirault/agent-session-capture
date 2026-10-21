//! JavaScript string semantics the output depends on.
//!
//! JS strings are UTF-16: `length`, `slice` and `<` work on code units, and a
//! slice can split a surrogate pair. Rust strings are UTF-8 and cannot hold a
//! lone surrogate, so one is encoded as two private-use characters
//! ([`LONE_SURROGATE_TAG`] then `U+F0000 + (unit - 0xD800)`); the JSON
//! serializer turns that pair back into the `\udXXX` escape `JSON.stringify`
//! writes.

use std::cmp::Ordering;

/// Leading character of an encoded lone surrogate.
pub const LONE_SURROGATE_TAG: char = '\u{10FFFD}';
const LONE_SURROGATE_BASE: u32 = 0xF0000;

/// Encode one lone UTF-16 surrogate (0xD800..=0xDFFF) into `out`.
pub fn push_lone_surrogate(out: &mut String, unit: u16) {
    out.push(LONE_SURROGATE_TAG);
    out.push(char::from_u32(LONE_SURROGATE_BASE + u32::from(unit - 0xD800)).expect("plane 15 code point"));
}

/// Decode the payload character of an encoded lone surrogate.
pub fn lone_surrogate_unit(payload: char) -> Option<u16> {
    let code = payload as u32;
    if (LONE_SURROGATE_BASE..LONE_SURROGATE_BASE + 0x800).contains(&code) {
        Some((code - LONE_SURROGATE_BASE) as u16 + 0xD800)
    } else {
        None
    }
}

/// The UTF-16 code units of `s`, encoded lone surrogates decoded back.
pub fn utf16_units(s: &str) -> Vec<u16> {
    let mut units = Vec::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == LONE_SURROGATE_TAG
            && let Some(unit) = chars.peek().copied().and_then(lone_surrogate_unit)
        {
            chars.next();
            units.push(unit);
            continue;
        }
        let mut buf = [0u16; 2];
        units.extend_from_slice(c.encode_utf16(&mut buf));
    }
    units
}

/// Rebuild a string from UTF-16 code units, encoding lone surrogates.
pub fn from_utf16_units(units: &[u16]) -> String {
    let mut out = String::with_capacity(units.len());
    for decoded in char::decode_utf16(units.iter().copied()) {
        match decoded {
            Ok(c) => out.push(c),
            Err(e) => push_lone_surrogate(&mut out, e.unpaired_surrogate()),
        }
    }
    out
}

/// Text as Node writes it to a UTF-8 stream: a lone surrogate becomes U+FFFD.
pub fn to_output_text(s: &str) -> String {
    if !s.contains(LONE_SURROGATE_TAG) {
        return s.to_string();
    }
    String::from_utf16_lossy(&utf16_units(s))
}

/// `s.length`.
pub fn utf16_len(s: &str) -> usize {
    if s.is_ascii() {
        return s.len();
    }
    utf16_units(s).len()
}

/// `s.slice(start, end)` with non-negative, already clamped indexes.
pub fn utf16_slice(s: &str, start: usize, end: usize) -> String {
    if s.is_ascii() {
        return s[start.min(s.len())..end.min(s.len())].to_string();
    }
    let units = utf16_units(s);
    let end = end.min(units.len());
    let start = start.min(end);
    from_utf16_units(&units[start..end])
}

/// `String.prototype.trim` whitespace: WhiteSpace and LineTerminator.
pub fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

/// `s.trim()`.
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// `a < b ? -1 : a > b ? 1 : 0` on JS strings (UTF-16 code unit order).
pub fn js_cmp(a: &str, b: &str) -> Ordering {
    if a.is_ascii() && b.is_ascii() {
        return a.as_bytes().cmp(b.as_bytes());
    }
    let mut left = a.encode_utf16();
    let mut right = b.encode_utf16();
    // Encoded lone surrogates sort as their private-use encoding; such strings
    // never reach an ordering that matters for the output.
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x != y => return x.cmp(&y),
            _ => {}
        }
    }
}

/// Primary collation weight of an ASCII character in the CLDR root order
/// (`localeCompare` with no locale): control characters are ignorable,
/// then whitespace, punctuation and symbols, digits, letters.
fn primary_weight(c: u8) -> Option<u16> {
    const ORDER: &[u8] = b"\t\n\x0b\x0c\r _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$0123456789abcdefghijklmnopqrstuvwxyz";
    let lower = c.to_ascii_lowercase();
    ORDER.iter().position(|&o| o == lower).map(|p| p as u16 + 1)
}

/// `a.localeCompare(b)` for the ids this tool sorts.
///
/// ASCII strings follow the CLDR root collation (three levels: base
/// character, then lowercase before uppercase). Anything else falls back to
/// code unit order, which ICU agrees with for the identifiers seen in
/// practice (UUIDs, slugs, numbers).
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    if !(a.is_ascii() && b.is_ascii()) {
        return js_cmp(a, b);
    }
    let weights = |s: &str| -> Vec<u16> { s.bytes().filter_map(primary_weight).collect() };
    let primary = weights(a).cmp(&weights(b));
    if primary != Ordering::Equal {
        return primary;
    }
    let tertiary = |s: &str| -> Vec<u8> {
        s.bytes()
            .filter(|&c| primary_weight(c).is_some())
            .map(|c| u8::from(c.is_ascii_uppercase()))
            .collect()
    };
    tertiary(a).cmp(&tertiary(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slicing_a_surrogate_pair_keeps_each_half() {
        let s = "a🚀b";
        assert_eq!(utf16_len(s), 4);
        let head = utf16_slice(s, 0, 2);
        assert_eq!(utf16_len(&head), 2);
        assert_eq!(utf16_units(&head), vec![0x61, 0xD83D]);
        let rejoined = format!("{}{}", head, utf16_slice(s, 2, 4));
        assert_eq!(utf16_units(&rejoined), utf16_units(s));
    }

    #[test]
    fn locale_compare_puts_lowercase_first_and_ignores_case_at_primary_level() {
        assert_eq!(locale_compare("a", "B"), Ordering::Less);
        assert_eq!(locale_compare("a", "A"), Ordering::Less);
        assert_eq!(locale_compare("sess-A", "sess-B"), Ordering::Less);
        assert_eq!(locale_compare("a-b", "a_b"), Ordering::Greater);
        assert_eq!(locale_compare("t1", "T2"), Ordering::Less);
    }
}

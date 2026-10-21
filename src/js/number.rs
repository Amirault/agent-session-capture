//! `Number.prototype.toString()` (radix 10), as `JSON.stringify` and template
//! literals print numbers.

/// Format `value` exactly as JavaScript prints a number.
pub fn number_to_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value == 0.0 {
        return "0".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if value < 0.0 {
        return format!("-{}", number_to_string(-value));
    }
    // Shortest round-trip digits: Rust's `{:e}` prints d.ddddde<exp>.
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific.split_once('e').expect("scientific notation");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    // value = 0.d1d2..dk * 10^n
    let n = exponent.parse::<i32>().expect("exponent") + 1;

    if k <= n && n <= 21 {
        let mut out = digits;
        out.extend(std::iter::repeat_n('0', (n - k) as usize));
        out
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{}", "0".repeat((-n) as usize), digits)
    } else {
        let e = n - 1;
        let sign = if e < 0 { '-' } else { '+' };
        if k == 1 {
            format!("{digits}e{sign}{}", e.abs())
        } else {
            format!("{}.{}e{sign}{}", &digits[..1], &digits[1..], e.abs())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::number_to_string;

    #[test]
    fn prints_like_javascript() {
        let cases = [
            (1.0, "1"),
            (1.5, "1.5"),
            (-0.0, "0"),
            (100.0, "100"),
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (1.2345e-7, "1.2345e-7"),
            (0.000001, "0.000001"),
            (1e-7, "1e-7"),
            (123456789012345680000000.0, "1.2345678901234569e+23"),
            (12345678901234567890.0, "12345678901234567000"),
            (0.1, "0.1"),
            (1784000000.123456, "1784000000.123456"),
            (-2.5e-10, "-2.5e-10"),
        ];
        for (value, expected) in cases {
            assert_eq!(number_to_string(value), expected, "{value}");
        }
    }
}

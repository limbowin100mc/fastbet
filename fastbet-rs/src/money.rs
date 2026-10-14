//! Money handling. Every amount is an integer in the currency's smallest unit
//! (value * 10^decimals), exactly like the casino APIs send it. `i128` holds
//! 18-decimal coins with room to spare, so nothing is ever rounded.

use serde_json::Value;

pub type Units = i128;

/// "0.001" with 8 decimals -> 100000. Accepts a comma as the decimal separator.
pub fn parse_units(s: &str, dec: u32) -> Option<Units> {
    let s = s.trim().replace(',', ".");
    if s.is_empty() || s == "." {
        return None;
    }
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        None => (s.as_str(), ""),
    };
    if !int.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let dec = dec as usize;
    let mut digits = String::with_capacity(int.len() + dec);
    digits.push_str(if int.is_empty() { "0" } else { int });
    let frac: String = frac.chars().take(dec).collect();
    digits.push_str(&frac);
    for _ in frac.len()..dec {
        digits.push('0');
    }
    digits.parse::<Units>().ok()
}

/// 100000 with 8 decimals -> "0.001". Trailing zeros are trimmed, `max_frac` caps the fraction.
pub fn format_units(v: Units, dec: u32, max_frac: u32) -> String {
    let neg = v < 0;
    let s = v.unsigned_abs().to_string();
    let dec = dec as usize;
    let s = if s.len() <= dec { format!("{}{}", "0".repeat(dec + 1 - s.len()), s) } else { s };
    let (int, frac) = s.split_at(s.len() - dec);
    let frac: String = frac.chars().take(max_frac as usize).collect();
    let frac = frac.trim_end_matches('0');
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    out.push_str(int);
    if !frac.is_empty() {
        out.push('.');
        out.push_str(frac);
    }
    out
}

/// Base units -> f64 coin value (display only, never for betting math).
pub fn to_f64(v: Units, dec: u32) -> f64 {
    v as f64 / 10f64.powi(dec as i32)
}

/// v * (1 + pct/100), rounded down. Used for on-win / on-loss stake changes.
pub fn scale_pct(v: Units, pct: f64) -> Units {
    let factor = ((100.0 + pct) * 1e6).round() as i128;
    (v * factor) / 100_000_000
}

/// v * m with 6 decimals of precision on `m`, rounded down. Used for payouts.
pub fn mul_f(v: Units, m: f64) -> Units {
    let factor = (m * 1e6).floor() as i128;
    (v * factor) / 1_000_000
}

/// Integer amounts arrive as strings ("100000") or numbers.
pub fn units_from_json(v: &Value) -> Option<Units> {
    match v {
        Value::String(s) => s.trim().parse::<Units>().ok(),
        Value::Number(n) => n.as_i64().map(|n| n as Units).or_else(|| {
            let f = n.as_f64()?;
            (f.fract() == 0.0).then_some(f as Units)
        }),
        _ => None,
    }
}

pub fn f64_from_json(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Parses a user-typed number, accepting a comma decimal separator.
pub fn num(s: &str) -> Option<f64> {
    let t = s.trim().replace(',', ".");
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>().ok().filter(|f| f.is_finite())
}

/// 1234567.891 -> "1,234,567.891"
pub fn group_thousands(s: &str) -> String {
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    if neg {
        format!("-{out}")
    } else {
        out
    }
}

pub fn fmt_int(n: impl Into<i128>) -> String {
    group_thousands(&n.into().to_string())
}

/// "$1,234.56", "-$0.0042"
pub fn fmt_usd(usd: f64) -> String {
    fmt_fiat(usd, "usd")
}

/// How a fiat is written in front of an amount, where it has a well-known sign.
pub fn fiat_sign(code: &str) -> Option<&'static str> {
    Some(match code {
        "usd" => "$",
        "eur" => "€",
        "gbp" => "£",
        "jpy" => "¥",
        "cny" => "CN¥",
        "inr" => "₹",
        "krw" => "₩",
        "rub" => "₽",
        "try" => "₺",
        "php" => "₱",
        "vnd" => "₫",
        "ngn" => "₦",
        "ils" => "₪",
        "uah" => "₴",
        "kzt" => "₸",
        "brl" => "R$",
        "cad" => "CA$",
        "aud" => "A$",
        "nzd" => "NZ$",
        "mxn" => "MX$",
        "hkd" => "HK$",
        "twd" => "NT$",
        _ => return None,
    })
}

/// "€1,234.56", "-¥0.0042", "PLN 12.50": an amount in a fiat currency (code in any case).
pub fn fmt_fiat(v: f64, code: &str) -> String {
    let abs = v.abs();
    let d = if abs != 0.0 && abs < 0.01 { 4 } else { 2 };
    let body = group_thousands(&format!("{abs:.d$}"));
    let minus = if v < 0.0 { "-" } else { "" };
    let code = code.to_ascii_lowercase();
    match fiat_sign(&code) {
        Some(sign) => format!("{minus}{sign}{body}"),
        None => format!("{minus}{} {body}", code.to_uppercase()),
    }
}

/// 2 -> "2.00×", 12345.6 -> "12,345×"
pub fn fmt_multi(m: f64) -> String {
    if !m.is_finite() {
        return "?".into();
    }
    if m >= 1000.0 {
        format!("{}×", group_thousands(&(m.floor() as i128).to_string()))
    } else {
        format!("{m:.2}×")
    }
}

/// 3723000 ms -> "01:02:03"
pub fn fmt_time(ms: u128) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// Latency in ms, compact.
pub fn fmt_ms(ms: f64) -> String {
    if ms <= 0.0 {
        "–".into()
    } else {
        format!("{}ms", ms.round() as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_format_roundtrip() {
        assert_eq!(parse_units("0.001", 8), Some(100_000));
        assert_eq!(parse_units("1", 18), Some(1_000_000_000_000_000_000));
        assert_eq!(parse_units("0,5", 2), Some(50));
        assert_eq!(parse_units("", 8), None);
        assert_eq!(parse_units("abc", 8), None);
        assert_eq!(format_units(100_000, 8, 8), "0.001");
        assert_eq!(format_units(-150_000_000, 8, 8), "-1.5");
        assert_eq!(format_units(0, 8, 8), "0");
        assert_eq!(format_units(123_456_789_012, 8, 4), "1234.5678");
    }

    #[test]
    fn scaling() {
        assert_eq!(scale_pct(1000, 100.0), 2000);
        assert_eq!(scale_pct(1000, 50.0), 1500);
        assert_eq!(scale_pct(1000, -50.0), 500);
        assert_eq!(mul_f(1000, 2.0), 2000);
        assert_eq!(mul_f(1000, 1.98), 1980);
    }

    #[test]
    fn formatting() {
        assert_eq!(group_thousands("1234567.89"), "1,234,567.89");
        assert_eq!(fmt_usd(1234.5), "$1,234.50");
        assert_eq!(fmt_fiat(1234.5, "EUR"), "€1,234.50");
        assert_eq!(fmt_fiat(-0.004, "jpy"), "-¥0.0040");
        assert_eq!(fmt_fiat(12.5, "pln"), "PLN 12.50");
        assert_eq!(fmt_fiat(-3.0, "brl"), "-R$3.00");
        assert_eq!(fmt_usd(-0.0042), "-$0.0042");
        assert_eq!(fmt_multi(2.0), "2.00×");
        assert_eq!(fmt_multi(12345.6), "12,345×");
        assert_eq!(fmt_time(3_723_000), "01:02:03");
    }
}

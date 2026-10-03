//! Small encoding and formatting helpers.

use std::time::Duration;

const BASE32_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// RFC 4648 base32, lowercase, without padding.
pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 8 / 5 + 1);
    let (mut buf, mut bits) = (0u32, 0u32);
    for &byte in data {
        buf = (buf << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32_ALPHABET[((buf >> bits) & 31) as usize] as char);
        }
        buf &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(BASE32_ALPHABET[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// Inverse of [`base32_encode`]; case-insensitive. Returns `None` on invalid input.
pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 5 / 8);
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let value = BASE32_ALPHABET
            .iter()
            .position(|&a| a == c.to_ascii_lowercase())?;
        buf = (buf << 5) | value as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

pub use crate::crypto::random;

pub fn hex_encode(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Parse durations like `90`, `30s`, `10m`, `2h` or `1d`.
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (number, unit) = s.split_at(split);
    let n: u64 = number.parse().ok()?;
    let multiplier = match unit {
        "" | "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => return None,
    };
    let secs = n.checked_mul(multiplier)?;
    (secs > 0).then(|| Duration::from_secs(secs))
}

pub fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {}s", s / 60, s % 60),
        3600..=86_399 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        _ => format!("{}d {}h", s / 86_400, s % 86_400 / 3600),
    }
}

pub fn format_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base32_round_trips() {
        for len in 0..40 {
            let data: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(base32_decode(&base32_encode(&data)).unwrap(), data);
        }
    }

    #[test]
    fn base32_matches_rfc4648() {
        assert_eq!(base32_encode(b"foobar"), "mzxw6ytboi");
        assert_eq!(base32_decode("MZXW6YTBOI").unwrap(), b"foobar");
        assert!(base32_decode("not base32!").is_none());
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(hex_encode(&[0, 1, 0xab, 0xff]), "0001abff");
        assert_eq!(hex_decode("0001abff").unwrap(), vec![0, 1, 0xab, 0xff]);
        assert!(hex_decode("abc").is_none());
        assert!(hex_decode("zz").is_none());
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("90"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("10m"), Some(Duration::from_secs(600)));
        assert_eq!(parse_duration("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_duration("1d"), Some(Duration::from_secs(86_400)));
        assert_eq!(parse_duration("0s"), None);
        assert_eq!(parse_duration("5x"), None);
        assert_eq!(parse_duration(""), None);
        assert_eq!(format_duration(Duration::from_secs(3725)), "1h 2m");
    }

    #[test]
    fn bytes() {
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1_500_000), "1.5 MB");
    }
}

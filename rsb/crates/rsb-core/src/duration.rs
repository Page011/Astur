//! Durations as users write them: a number of seconds (`2`, `0.5`) or a
//! string with a unit (`"500ms"`, `"2s"`, `"1m"`, `"1h"`). Same rules in
//! `bar.toml`, CSS (`ms`, `s` only) and Luau.

use std::time::Duration;

/// Longest duration accepted anywhere.
pub const MAX_DURATION: Duration = Duration::from_secs(24 * 60 * 60);

const EXPECTED: &str = "expected a duration like \"500ms\", \"2s\", \"1m\" or \"1h\"";

/// Parses `"500ms"`, `"2s"`, `"1.5s"`, `"1m"`, `"1h"`. No spaces, no
/// negative values, at most 24 h. The error says what was expected.
pub fn parse_duration(text: &str) -> Result<Duration, String> {
    let split = text.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    if number.is_empty() || number.starts_with('.') || number.ends_with('.') {
        return Err(EXPECTED.into());
    }
    let n: f64 = number.parse().map_err(|_| EXPECTED.to_string())?;
    let secs = match unit {
        "ms" => n / 1000.0,
        "s" => n,
        "m" => n * 60.0,
        "h" => n * 3600.0,
        "" => return Err(format!("'{text}' has no unit; {EXPECTED}")),
        _ => return Err(format!("unknown unit '{unit}'; {EXPECTED}")),
    };
    from_seconds(secs)
}

/// A number of seconds from Luau or TOML. Same limits as `parse_duration`.
pub fn from_seconds(secs: f64) -> Result<Duration, String> {
    if !secs.is_finite() || secs < 0.0 {
        return Err("a duration must be 0 or more".into());
    }
    if secs > MAX_DURATION.as_secs_f64() {
        return Err("a duration must be at most 24h".into());
    }
    Ok(Duration::from_secs_f64(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units() {
        assert_eq!(parse_duration("500ms"), Ok(Duration::from_millis(500)));
        assert_eq!(parse_duration("2s"), Ok(Duration::from_secs(2)));
        assert_eq!(parse_duration("1.5s"), Ok(Duration::from_millis(1500)));
        assert_eq!(parse_duration("1m"), Ok(Duration::from_secs(60)));
        assert_eq!(parse_duration("1h"), Ok(Duration::from_secs(3600)));
        assert_eq!(parse_duration("24h"), Ok(MAX_DURATION));
        assert_eq!(parse_duration("0s"), Ok(Duration::ZERO));
    }

    #[test]
    fn refusals() {
        for bad in ["", "2", "s", "-1s", "1 s", " 1s", "1.s", ".5s", "1d", "25h", "1e3s", "1..2s", "2S"] {
            assert!(parse_duration(bad).is_err(), "{bad:?}");
        }
        assert!(parse_duration("2").unwrap_err().contains("no unit"));
        assert!(parse_duration("2d").unwrap_err().contains("unknown unit 'd'"));
    }

    #[test]
    fn seconds() {
        assert_eq!(from_seconds(0.25), Ok(Duration::from_millis(250)));
        assert!(from_seconds(-0.1).is_err());
        assert!(from_seconds(f64::NAN).is_err());
        assert!(from_seconds(86_401.0).is_err());
    }
}

//! Wall-clock time, time zones and the clock format syntax.
//!
//! Format tokens (case-sensitive; anything else is copied; text in single
//! quotes is copied as is, `''` is a quote):
//!
//! | Token | Meaning | Example |
//! |---|---|---|
//! | `yyyy` / `yy` | year | 2026 / 26 |
//! | `MMMM` / `MMM` / `MM` / `M` | month name / short / 2-digit / number | November / Nov / 11 / 11 |
//! | `dddd` / `ddd` / `dd` / `d` | weekday / short weekday / 2-digit day / day | Tuesday / Tue / 04 / 4 |
//! | `HH` / `H` | hour 0-23, 2-digit / plain | 09 / 9 |
//! | `hh` / `h` | hour 1-12, 2-digit / plain | 09 / 9 |
//! | `mm` / `m` | minute | 05 / 5 |
//! | `ss` / `s` | second | 07 / 7 |
//! | `tt` / `t` | AM/PM / A/P | PM / P |
//!
//! Longer runs of one letter repeat the longest token (`HHH` = `HH` `H`).
//! Names are English in this version. `precision_of(format)` tells the clock
//! how often it must tick: a format without `s` ticks once a minute, on the
//! minute; one without hours or minutes ticks once an hour.

use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    /// 1..=12
    pub month: u8,
    /// 1..=31
    pub day: u8,
    /// 0 = Sunday .. 6 = Saturday
    pub weekday: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub millis: u16,
    /// Offset from UTC in minutes for this zone at this moment.
    pub utc_offset_min: i32,
}

impl LocalTime {
    /// The time at `epoch_ms` (milliseconds since 1970-01-01 UTC) seen at a
    /// fixed offset. Weekday is computed.
    pub fn from_epoch_ms(epoch_ms: i64, utc_offset_min: i32) -> LocalTime {
        let local = epoch_ms + utc_offset_min as i64 * 60_000;
        let days = local.div_euclid(86_400_000);
        let ms_of_day = local.rem_euclid(86_400_000);
        let (year, month, day) = civil_from_days(days);
        LocalTime {
            year,
            month,
            day,
            weekday: (days + 4).rem_euclid(7) as u8,
            hour: (ms_of_day / 3_600_000) as u8,
            minute: (ms_of_day / 60_000 % 60) as u8,
            second: (ms_of_day / 1000 % 60) as u8,
            millis: (ms_of_day % 1000) as u16,
            utc_offset_min,
        }
    }

    /// Milliseconds since 1970-01-01 UTC. Ignores `weekday`.
    pub fn to_epoch_ms(&self) -> i64 {
        let days = days_from_civil(self.year, self.month, self.day);
        let ms =
            self.hour as i64 * 3_600_000 + self.minute as i64 * 60_000 + self.second as i64 * 1000 + self.millis as i64;
        days * 86_400_000 + ms - self.utc_offset_min as i64 * 60_000
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
fn days_from_civil(y: i32, m: u8, d: u8) -> i64 {
    let y = y as i64 - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
pub fn civil_from_days(z: i64) -> (i32, u8, u8) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y as i32, m, d)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimeZone {
    #[default]
    Local,
    Utc,
    /// Fixed offset in minutes, from `"+10:00"` / `"-05:30"`.
    Offset(i32),
}

impl TimeZone {
    /// `"local"`, `"utc"`, `"+10:00"`, `"-0530"`, `"+10"` (case-insensitive
    /// names; offsets within 14 hours).
    pub fn parse(text: &str) -> Result<TimeZone, String> {
        let expected = || format!("bad time zone '{text}': use \"local\", \"utc\" or an offset like \"+10:00\"");
        match text.to_ascii_lowercase().as_str() {
            "local" => return Ok(TimeZone::Local),
            "utc" | "z" => return Ok(TimeZone::Utc),
            _ => {}
        }
        let (sign, rest) = match text.as_bytes().first() {
            Some(b'+') => (1, &text[1..]),
            Some(b'-') => (-1, &text[1..]),
            _ => return Err(expected()),
        };
        let digits: String = rest.chars().filter(|c| *c != ':').collect();
        let colon_ok = match rest.find(':') {
            None => true,
            Some(i) => i == 2 && rest.len() == 5,
        };
        if !colon_ok || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(expected());
        }
        let (h, m) = match digits.len() {
            1 | 2 => (digits.parse::<i32>().map_err(|_| expected())?, 0),
            4 => (
                digits[..2].parse::<i32>().map_err(|_| expected())?,
                digits[2..].parse::<i32>().map_err(|_| expected())?,
            ),
            _ => return Err(expected()),
        };
        if m >= 60 || h * 60 + m > 14 * 60 {
            return Err(format!("time zone '{text}' is out of range (-14:00 to +14:00)"));
        }
        Ok(TimeZone::Offset(sign * (h * 60 + m)))
    }
}

/// How often a display must change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Precision {
    Second,
    Minute,
    Hour,
}

impl Precision {
    pub fn name(self) -> &'static str {
        match self {
            Precision::Second => "second",
            Precision::Minute => "minute",
            Precision::Hour => "hour",
        }
    }
}

/// Source of time. The real one lives in rsb-services; tests use
/// `ManualClock`.
pub trait Clock: Send + Sync {
    /// Monotonic time for timers.
    fn mono(&self) -> Instant;
    fn now(&self, zone: TimeZone) -> LocalTime;
}

/// CPU time used so far by the calling thread. The script runtime uses it to
/// tell a slow callback from one that was only waiting for the CPU (a
/// preempted thread uses wall time but no CPU time). rsb-services has the
/// Windows one.
pub trait ThreadTime: Send + Sync {
    fn now(&self) -> Duration;
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

#[derive(Debug, PartialEq)]
enum Token<'a> {
    Text(std::borrow::Cow<'a, str>),
    Field(char, usize),
}

/// Splits a format into literal text and fields (letter, run length).
fn tokens(format: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let chars: Vec<(usize, char)> = format.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (pos, c) = chars[i];
        if c == '\'' {
            // '' outside quotes is a quote; otherwise copy up to the closing quote.
            if chars.get(i + 1).map(|x| x.1) == Some('\'') {
                out.push(Token::Text("'".into()));
                i += 2;
                continue;
            }
            let mut text = String::new();
            i += 1;
            while i < chars.len() {
                if chars[i].1 == '\'' {
                    if chars.get(i + 1).map(|x| x.1) == Some('\'') {
                        text.push('\'');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                text.push(chars[i].1);
                i += 1;
            }
            out.push(Token::Text(text.into()));
            continue;
        }
        let max = match c {
            'y' | 'M' | 'd' => 4,
            'H' | 'h' | 'm' | 's' | 't' => 2,
            _ => 0,
        };
        if max == 0 {
            let end = chars.get(i + 1).map_or(format.len(), |x| x.0);
            out.push(Token::Text(format[pos..end].into()));
            i += 1;
            continue;
        }
        let mut n = 1;
        while n < max && chars.get(i + n).map(|x| x.1) == Some(c) {
            n += 1;
        }
        out.push(Token::Field(c, n));
        i += n;
    }
    out
}

/// Formats `t` with the token table above. Unknown letters are copied.
pub fn format_time(format: &str, t: &LocalTime) -> String {
    let mut s = String::new();
    let h12 = match t.hour % 12 {
        0 => 12,
        h => h,
    };
    for tok in tokens(format) {
        match tok {
            Token::Text(x) => s.push_str(&x),
            Token::Field(c, n) => {
                let v = match (c, n) {
                    ('y', 2) => format!("{:02}", t.year.rem_euclid(100)),
                    ('y', 4) => format!("{:04}", t.year),
                    ('y', _) => t.year.to_string(),
                    ('M', 1) => t.month.to_string(),
                    ('M', 2) => format!("{:02}", t.month),
                    ('M', 3) => MONTHS[(t.month.clamp(1, 12) - 1) as usize][..3].to_string(),
                    ('M', _) => MONTHS[(t.month.clamp(1, 12) - 1) as usize].to_string(),
                    ('d', 1) => t.day.to_string(),
                    ('d', 2) => format!("{:02}", t.day),
                    ('d', 3) => WEEKDAYS[(t.weekday % 7) as usize][..3].to_string(),
                    ('d', _) => WEEKDAYS[(t.weekday % 7) as usize].to_string(),
                    ('H', 1) => t.hour.to_string(),
                    ('H', _) => format!("{:02}", t.hour),
                    ('h', 1) => h12.to_string(),
                    ('h', _) => format!("{h12:02}"),
                    ('m', 1) => t.minute.to_string(),
                    ('m', _) => format!("{:02}", t.minute),
                    ('s', 1) => t.second.to_string(),
                    ('s', _) => format!("{:02}", t.second),
                    ('t', 1) => (if t.hour < 12 { "A" } else { "P" }).to_string(),
                    _ => (if t.hour < 12 { "AM" } else { "PM" }).to_string(),
                };
                s.push_str(&v);
            }
        }
    }
    s
}

/// The finest unit `format` shows.
pub fn precision_of(format: &str) -> Precision {
    let mut p = Precision::Hour;
    for tok in tokens(format) {
        match tok {
            Token::Field('s', _) => return Precision::Second,
            Token::Field('m', _) => p = Precision::Minute,
            _ => {}
        }
    }
    p
}

/// Time from `t` to the next whole second / minute / hour boundary, so
/// ticks land on the boundary (a minute clock changes at :00 seconds).
/// Exactly on a boundary, the next one is a whole unit away.
pub fn until_boundary(t: &LocalTime, precision: Precision) -> Duration {
    let ms_in = t.millis as u64;
    let s = t.second.min(59) as u64;
    let m = t.minute.min(59) as u64;
    let (unit, into) = match precision {
        Precision::Second => (1000, ms_in),
        Precision::Minute => (60_000, s * 1000 + ms_in),
        Precision::Hour => (3_600_000, m * 60_000 + s * 1000 + ms_in),
    };
    Duration::from_millis(unit - into.min(unit - 1))
}

/// A clock tests move by hand. `advance` moves both the monotonic and the
/// wall time; `set_wall` moves only the wall time (a clock change, or a
/// resume from sleep where the monotonic wait did not count the sleep).
pub struct ManualClock {
    state: std::sync::Mutex<(Instant, LocalTime)>,
}

impl ManualClock {
    pub fn new(start: LocalTime) -> ManualClock {
        ManualClock { state: std::sync::Mutex::new((Instant::now(), start)) }
    }

    pub fn advance(&self, by: Duration) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.0 += by;
        let ms = st.1.to_epoch_ms() + by.as_millis() as i64;
        st.1 = LocalTime::from_epoch_ms(ms, st.1.utc_offset_min);
    }

    pub fn set_wall(&self, to: LocalTime) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.1 = LocalTime::from_epoch_ms(to.to_epoch_ms(), to.utc_offset_min);
    }
}

impl Clock for ManualClock {
    fn mono(&self) -> Instant {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).0
    }

    fn now(&self, zone: TimeZone) -> LocalTime {
        let local = self.state.lock().unwrap_or_else(|e| e.into_inner()).1;
        let offset = match zone {
            TimeZone::Local => return local,
            TimeZone::Utc => 0,
            TimeZone::Offset(m) => m,
        };
        LocalTime::from_epoch_ms(local.to_epoch_ms(), offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tuesday 24 November 2026, 09:05:07.250 at +10:00.
    fn sample() -> LocalTime {
        LocalTime {
            year: 2026,
            month: 11,
            day: 24,
            weekday: 2,
            hour: 9,
            minute: 5,
            second: 7,
            millis: 250,
            utc_offset_min: 600,
        }
    }

    #[test]
    fn epoch_round_trip_and_weekday() {
        let t = sample();
        let ms = t.to_epoch_ms();
        assert_eq!(LocalTime::from_epoch_ms(ms, 600), t);
        assert_eq!(LocalTime::from_epoch_ms(0, 0).weekday, 4, "1970-01-01 was a Thursday");
        let leap = LocalTime::from_epoch_ms(LocalTime { year: 2024, month: 2, day: 29, ..t }.to_epoch_ms(), 600);
        assert_eq!((leap.year, leap.month, leap.day, leap.weekday), (2024, 2, 29, 4));
        let before = LocalTime::from_epoch_ms(-1, 0);
        assert_eq!((before.year, before.month, before.day, before.hour, before.millis), (1969, 12, 31, 23, 999));
    }

    #[test]
    fn every_token() {
        let t = sample();
        let f = |s: &str| format_time(s, &t);
        assert_eq!(f("yyyy yy"), "2026 26");
        assert_eq!(f("MMMM MMM MM M"), "November Nov 11 11");
        assert_eq!(f("dddd ddd dd d"), "Tuesday Tue 24 24");
        assert_eq!(f("HH H hh h"), "09 9 09 9");
        assert_eq!(f("mm m ss s"), "05 5 07 7");
        assert_eq!(f("tt t"), "AM A");
        assert_eq!(f("HH:mm 'UTC'"), "09:05 UTC");
        assert_eq!(f("'it''s' HH''"), "it's 09'");
        assert_eq!(f("HHH"), "099");
        assert_eq!(f("x-z!"), "x-z!");
        let pm = LocalTime { hour: 0, ..t };
        assert_eq!(format_time("h tt", &pm), "12 AM");
        let pm = LocalTime { hour: 13, day: 4, month: 3, ..t };
        assert_eq!(format_time("hh t d/M dd", &pm), "01 P 4/3 04");
    }

    #[test]
    fn precision() {
        assert_eq!(precision_of("HH:mm:ss"), Precision::Second);
        assert_eq!(precision_of("HH:mm"), Precision::Minute);
        assert_eq!(precision_of("ddd d MMM"), Precision::Hour);
        assert_eq!(precision_of("HH 'mm ss'"), Precision::Hour);
        assert_eq!(precision_of("h tt"), Precision::Hour);
    }

    #[test]
    fn boundaries() {
        let t = sample();
        assert_eq!(until_boundary(&t, Precision::Second), Duration::from_millis(750));
        assert_eq!(until_boundary(&t, Precision::Minute), Duration::from_millis(52_750));
        assert_eq!(until_boundary(&t, Precision::Hour), Duration::from_millis(3_600_000 - 307_250));
        let exact = LocalTime { second: 0, millis: 0, ..t };
        assert_eq!(until_boundary(&exact, Precision::Minute), Duration::from_secs(60));
        // A minute clock started at t lands on :00.
        let clock = ManualClock::new(t);
        clock.advance(until_boundary(&t, Precision::Minute));
        let next = clock.now(TimeZone::Local);
        assert_eq!((next.minute, next.second, next.millis), (6, 0, 0));
    }

    #[test]
    fn zones() {
        assert_eq!(TimeZone::parse("local"), Ok(TimeZone::Local));
        assert_eq!(TimeZone::parse("UTC"), Ok(TimeZone::Utc));
        assert_eq!(TimeZone::parse("+10:00"), Ok(TimeZone::Offset(600)));
        assert_eq!(TimeZone::parse("-0530"), Ok(TimeZone::Offset(-330)));
        assert_eq!(TimeZone::parse("+10"), Ok(TimeZone::Offset(600)));
        assert_eq!(TimeZone::parse("+14:00"), Ok(TimeZone::Offset(840)));
        for bad in ["", "+", "10:00", "+1:00", "+10:0", "+10:60", "+15:00", "+1a", "+100", "Sydney"] {
            assert!(TimeZone::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn manual_clock_zones_and_jumps() {
        let clock = ManualClock::new(sample());
        let mono = clock.mono();
        let utc = clock.now(TimeZone::Utc);
        assert_eq!((utc.day, utc.hour, utc.minute, utc.utc_offset_min), (23, 23, 5, 0));
        let ny = clock.now(TimeZone::Offset(-300));
        assert_eq!((ny.day, ny.hour), (23, 18));
        clock.advance(Duration::from_secs(3600 * 24));
        assert_eq!(clock.now(TimeZone::Local).day, 25);
        assert_eq!(clock.mono() - mono, Duration::from_secs(86_400));
        clock.set_wall(LocalTime { hour: 23, ..sample() });
        assert_eq!(clock.now(TimeZone::Local).hour, 23);
        assert_eq!(clock.mono() - mono, Duration::from_secs(86_400), "a wall jump leaves timers alone");
    }
}

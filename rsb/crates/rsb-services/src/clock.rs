//! The real clock: `GetSystemTimePreciseAsFileTime` and the current time
//! zone rules (`SystemTimeToTzSpecificLocalTime`, asked afresh at every
//! reading so daylight-saving switches show at once).

use std::time::{Duration, Instant};

use rsb_core::time::{Clock, LocalTime, ThreadTime, TimeZone};
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows::Win32::System::Threading::GetCurrentThread;
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows::Win32::System::WindowsProgramming::QueryThreadCycleTime;

pub struct SystemClock;

/// Milliseconds between 1601-01-01 (FILETIME epoch) and 1970-01-01.
const FILETIME_TO_UNIX_MS: i64 = 11_644_473_600_000;

fn utc_epoch_ms() -> i64 {
    // SAFETY: no arguments; returns the time by value.
    let ft: FILETIME = unsafe { GetSystemTimePreciseAsFileTime() };
    let ticks = (i64::from(ft.dwHighDateTime) << 32) | i64::from(ft.dwLowDateTime);
    ticks / 10_000 - FILETIME_TO_UNIX_MS
}

/// Offset in minutes of the local zone at the instant `utc_ms`.
fn local_offset_min(utc_ms: i64) -> i32 {
    let ticks = (utc_ms + FILETIME_TO_UNIX_MS) * 10_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    // SAFETY: live in/out pointers; a null zone means the current time zone.
    let ok = unsafe {
        FileTimeToSystemTime(&ft, &mut utc).is_ok() && SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_ok()
    };
    if !ok {
        return 0;
    }
    let as_utc = LocalTime {
        year: i32::from(local.wYear),
        month: local.wMonth as u8,
        day: local.wDay as u8,
        weekday: 0,
        hour: local.wHour as u8,
        minute: local.wMinute as u8,
        second: local.wSecond as u8,
        millis: local.wMilliseconds,
        utc_offset_min: 0,
    };
    // Whole minutes: the zone offset never has seconds, and `utc_ms` carries sub-second noise.
    let diff = as_utc.to_epoch_ms() - (utc_ms - utc_ms.rem_euclid(1000) + i64::from(local.wMilliseconds));
    (diff as f64 / 60_000.0).round() as i32
}

impl Clock for SystemClock {
    fn mono(&self) -> Instant {
        Instant::now()
    }

    fn now(&self, zone: TimeZone) -> LocalTime {
        let utc = utc_epoch_ms();
        let offset = match zone {
            TimeZone::Local => local_offset_min(utc),
            TimeZone::Utc => 0,
            TimeZone::Offset(m) => m,
        };
        LocalTime::from_epoch_ms(utc, offset)
    }
}

/// CPU time of the calling thread from `QueryThreadCycleTime`, converted
/// with a cycles-per-nanosecond rate measured once at `new` (about 20 ms
/// against `QueryPerformanceCounter`). `GetThreadTimes` is not used: it only
/// moves in 15.6 ms steps, longer than the script budget.
pub struct ThreadCycleTime {
    cycles_per_ns: f64,
}

fn thread_cycles() -> u64 {
    let mut c = 0u64;
    // SAFETY: the pseudo handle of the calling thread is always valid; `c` is a live out pointer.
    let _ = unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut c) };
    c
}

impl ThreadCycleTime {
    /// Spins the calling thread for about 20 ms (three 7 ms rounds; the
    /// fastest rate counts, because a preempted round counts fewer cycles
    /// than the wall time it took).
    pub fn new() -> ThreadCycleTime {
        let mut best = 0.0f64;
        for _ in 0..3 {
            let (c0, t0) = (thread_cycles(), Instant::now());
            while t0.elapsed() < Duration::from_millis(7) {
                std::hint::spin_loop();
            }
            let (c1, ns) = (thread_cycles(), t0.elapsed().as_nanos() as f64);
            best = best.max(c1.saturating_sub(c0) as f64 / ns.max(1.0));
        }
        ThreadCycleTime { cycles_per_ns: if best > 0.0 { best } else { 1.0 } }
    }

    /// Cycles per nanosecond measured at `new` (the clock rate while it ran).
    pub fn cycles_per_ns(&self) -> f64 {
        self.cycles_per_ns
    }
}

impl ThreadTime for ThreadCycleTime {
    fn now(&self) -> Duration {
        Duration::from_nanos((thread_cycles() as f64 / self.cycles_per_ns) as u64)
    }
}

impl Default for ThreadCycleTime {
    fn default() -> Self {
        ThreadCycleTime::new()
    }
}

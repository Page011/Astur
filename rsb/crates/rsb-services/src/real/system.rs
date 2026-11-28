//! CPU load and memory. No thread: the hub's timer calls `tick`, and each
//! sample is two cheap kernel calls.

use std::time::Duration;

use rsb_core::services::{ServiceCommand, ServiceId};
use rsb_core::value::Value;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;

use crate::util::{map, round1};
use crate::{DEFAULT_SAMPLE_INTERVAL, Publisher, Service};

/// The first value comes this soon after start: CPU load is a difference
/// between two reads, so one is taken at start and the next here.
const FIRST_SAMPLE: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub(crate) struct CpuTimes {
    pub idle: u64,
    /// Kernel time includes idle time.
    pub kernel: u64,
    pub user: u64,
}

fn ticks(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

pub(crate) fn read_times() -> Option<CpuTimes> {
    let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: three live out pointers.
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
    Some(CpuTimes { idle: ticks(idle), kernel: ticks(kernel), user: ticks(user) })
}

/// Load in percent between two reads.
pub(crate) fn cpu_percent(prev: CpuTimes, now: CpuTimes) -> f64 {
    let total = now.kernel.saturating_sub(prev.kernel) + now.user.saturating_sub(prev.user);
    let idle = now.idle.saturating_sub(prev.idle);
    if total == 0 { 0.0 } else { (100.0 * (1.0 - idle as f64 / total as f64)).clamp(0.0, 100.0) }
}

/// (percent in use, bytes in use, bytes total).
pub(crate) fn read_memory() -> Option<(f64, u64, u64)> {
    let mut m = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: `m` is initialised with its size as the call requires.
    unsafe { GlobalMemoryStatusEx(&mut m) }.ok()?;
    let used = m.ullTotalPhys.saturating_sub(m.ullAvailPhys);
    let percent = if m.ullTotalPhys == 0 { 0.0 } else { 100.0 * used as f64 / m.ullTotalPhys as f64 };
    Some((percent, used, m.ullTotalPhys))
}

pub(crate) struct SystemService {
    out: Option<Publisher>,
    prev: Option<CpuTimes>,
    interval: Duration,
    next: Option<Duration>,
}

impl SystemService {
    pub(crate) fn new() -> SystemService {
        SystemService { out: None, prev: None, interval: DEFAULT_SAMPLE_INTERVAL, next: None }
    }
}

impl Service for SystemService {
    fn id(&self) -> ServiceId {
        ServiceId::System
    }

    fn start(&mut self, out: Publisher) -> Result<(), String> {
        self.prev = Some(read_times().ok_or("GetSystemTimes failed")?);
        self.out = Some(out);
        self.next = Some(FIRST_SAMPLE);
        Ok(())
    }

    fn set_interval(&mut self, interval: Duration) {
        self.interval = interval;
    }

    fn command(&mut self, _command: ServiceCommand) -> Result<(), String> {
        Err("the system service has no commands".into())
    }

    fn stop(&mut self) {
        self.out = None;
        self.prev = None;
        self.next = None;
    }

    fn stale_after(&self) -> Duration {
        (self.interval * 5).max(Duration::from_secs(10))
    }

    fn tick_after(&self) -> Option<Duration> {
        self.out.as_ref().map(|_| self.next.unwrap_or(self.interval))
    }

    fn tick(&mut self) {
        self.next = None;
        let (Some(out), Some(prev)) = (&self.out, self.prev) else { return };
        let Some(now) = read_times() else { return };
        self.prev = Some(now);
        let Some((memory, used, total)) = read_memory() else { return };
        out.publish(map([
            ("cpu", Value::Num(round1(cpu_percent(prev, now)))),
            ("memory", Value::Num(round1(memory))),
            ("memory_used", Value::Int(used as i64)),
            ("memory_total", Value::Int(total as i64)),
        ]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_is_the_busy_share_of_the_interval() {
        let a = CpuTimes { idle: 1000, kernel: 2000, user: 1000 };
        // 1000 ticks passed in kernel (400 of them idle) and 500 in user: 1100 busy of 1500.
        let b = CpuTimes { idle: 1400, kernel: 3000, user: 1500 };
        assert!((cpu_percent(a, b) - 100.0 * 1100.0 / 1500.0).abs() < 1e-9);
        assert_eq!(cpu_percent(a, a), 0.0, "no time passed");
        let fully_idle = CpuTimes { idle: 2000, kernel: 3000, user: 1000 };
        assert_eq!(cpu_percent(a, fully_idle), 0.0);
    }

    #[test]
    fn a_counter_that_goes_backwards_never_gives_a_negative_or_huge_load() {
        let a = CpuTimes { idle: 100, kernel: 200, user: 100 };
        let b = CpuTimes { idle: 50, kernel: 100, user: 50 };
        let load = cpu_percent(a, b);
        assert!((0.0..=100.0).contains(&load), "{load}");
    }

    #[test]
    fn the_real_reads_are_in_range() {
        let (percent, used, total) = read_memory().expect("memory status");
        assert!(total > 0 && used <= total && (0.0..=100.0).contains(&percent));
        let t = read_times().expect("system times");
        assert!(t.kernel >= t.idle, "kernel time includes idle time");
    }

    #[test]
    fn stale_time_follows_the_interval_with_a_floor() {
        let mut s = SystemService::new();
        assert_eq!(s.stale_after(), Duration::from_secs(10), "5 x 2 s");
        s.set_interval(Duration::from_millis(500));
        assert_eq!(s.stale_after(), Duration::from_secs(10), "floor of 10 s");
        s.set_interval(Duration::from_secs(5));
        assert_eq!(s.stale_after(), Duration::from_secs(25));
    }
}

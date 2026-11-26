//! Frame pacing.
//!
//! A free-running 16.7 ms timer drifts against a display refreshing at 59.94,
//! 60 or 144 Hz and so doubles or drops frames. Frame deadlines here are
//! moved to the compositor's next frame start at or after the requested time
//! (period and phase from `DwmGetCompositionTimingInfo`) and fired by a
//! high-resolution waitable timer the message loop waits on next to its
//! queue, so one animation frame lands in one display frame.
//!
//! The compositor's timing is read once per animation burst (a gap since the
//! last deadline) and again when a frame landed late. Without timing (some
//! remote sessions have none) the deadline is used as asked.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE, HWND, WAIT_OBJECT_0};
use windows::Win32::Graphics::Dwm::{DWM_TIMING_INFO, DwmGetCompositionTimingInfo};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CancelWaitableTimer, CreateWaitableTimerExW, SetWaitableTimer,
    TIMER_ALL_ACCESS, WaitForSingleObject,
};
use windows::core::PCWSTR;

use crate::WinError;

/// A deadline set after this long without one starts a new burst: the
/// compositor timing is read again.
const BURST_GAP: Duration = Duration::from_millis(150);
/// After a failed read, do not ask again for this long.
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(1);

/// The compositor's timing at one moment, in performance-counter ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Timing {
    pub period: i64,
    pub vblank: i64,
}

/// The first frame start at or after `target`: `vblank + n * period`.
pub(crate) fn align_to_frame(target: i64, timing: Timing) -> i64 {
    if timing.period <= 0 {
        return target;
    }
    let n = (target - timing.vblank).div_euclid(timing.period);
    let mut t = timing.vblank + n * timing.period;
    if t < target {
        t += timing.period;
    }
    t
}

fn qpc() -> i64 {
    let mut v = 0i64;
    // SAFETY: a live out pointer.
    unsafe {
        let _ = QueryPerformanceCounter(&mut v);
    }
    v
}

fn read_timing() -> Option<(Timing, u64)> {
    let mut info = DWM_TIMING_INFO { cbSize: size_of::<DWM_TIMING_INFO>() as u32, ..Default::default() };
    // SAFETY: `info` is a live DWM_TIMING_INFO with its size set; the null
    // window asks for the composition's own timing.
    unsafe { DwmGetCompositionTimingInfo(HWND::default(), &mut info) }.ok()?;
    if info.qpcRefreshPeriod == 0 {
        return None;
    }
    Some((Timing { period: info.qpcRefreshPeriod as i64, vblank: info.qpcVBlank as i64 }, info.cRefresh))
}

/// The compositor's refresh period.
pub(crate) fn refresh_period() -> Option<Duration> {
    let (timing, _) = read_timing()?;
    let mut freq = 0i64;
    // SAFETY: a live out pointer.
    unsafe {
        let _ = QueryPerformanceFrequency(&mut freq);
    }
    (freq > 0).then(|| Duration::from_nanos((timing.period as i128 * 1_000_000_000 / freq as i128) as u64))
}

/// The compositor's refresh counter now.
pub(crate) fn compositor_frame() -> Option<u64> {
    read_timing().map(|(_, c)| c)
}

pub(crate) struct Pacer {
    timer: HANDLE,
    freq: i64,
    timing: Option<Timing>,
    /// Do not read the timing before this moment (after a failed read).
    no_read_until: Option<Instant>,
    last_arm: Option<Instant>,
    /// The instant the timer was last armed for (aligned).
    armed_for: Option<Instant>,
    /// A frame landed late: read the timing again at the next deadline.
    stale: bool,
    /// How many times the timing was read, for tests and stats.
    pub(crate) timing_reads: u32,
    /// Tests: ignore the compositor and fire at the requested time.
    pub(crate) align: bool,
}

impl Pacer {
    pub(crate) fn new() -> Result<Pacer, WinError> {
        let mut freq = 0i64;
        // SAFETY: a live out pointer.
        unsafe {
            let _ = QueryPerformanceFrequency(&mut freq);
        }
        // SAFETY: no name, no security attributes.
        let timer = unsafe {
            CreateWaitableTimerExW(None, PCWSTR::null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS.0)
        }
        // Windows before the high-resolution flag (10 1803): the plain one.
        .or_else(|_| {
            // SAFETY: as above.
            unsafe { CreateWaitableTimerExW(None, PCWSTR::null(), 0, TIMER_ALL_ACCESS.0) }
        })
        .map_err(|e| crate::sys::call_error("CreateWaitableTimerExW", &e))?;
        Ok(Pacer {
            timer,
            freq: freq.max(1),
            timing: None,
            no_read_until: None,
            last_arm: None,
            armed_for: None,
            stale: false,
            timing_reads: 0,
            align: true,
        })
    }

    /// The handle the loop waits on.
    pub(crate) fn handle(&self) -> HANDLE {
        self.timer
    }

    /// Polls the timer: true once when it was signalled (a successful wait
    /// resets it, so the loop's own wait on the handle is reported by the
    /// caller and this covers the rest).
    pub(crate) fn take_signal(&mut self) -> bool {
        // SAFETY: our own timer handle; a zero wait only polls.
        let r = unsafe { WaitForSingleObject(self.timer, 0) };
        r == WAIT_OBJECT_0
    }

    /// The timer fired: `note_fired` for its lateness, and nothing is armed.
    pub(crate) fn fired(&mut self, now: Instant) {
        let armed = self.armed_for.take();
        self.note_fired(now, armed);
    }

    pub(crate) fn cancel(&mut self) {
        // SAFETY: our own timer handle.
        unsafe {
            let _ = CancelWaitableTimer(self.timer);
        }
        self.armed_for = None;
    }

    /// A frame fired `late` after the instant it was armed for: more than a
    /// refresh period means the phase is out of date.
    fn note_fired(&mut self, now: Instant, armed_for: Option<Instant>) {
        if let (Some(t), Some(timing)) = (armed_for, self.timing) {
            let late = now.saturating_duration_since(t);
            let period = Duration::from_nanos((timing.period as i128 * 1_000_000_000 / self.freq as i128) as u64);
            if late > period {
                self.stale = true;
            }
        }
    }

    fn refresh_timing(&mut self, now: Instant) {
        let burst = self.last_arm.is_none_or(|t| now.saturating_duration_since(t) > BURST_GAP);
        if !(burst || self.stale || self.timing.is_none()) {
            return;
        }
        if self.timing.is_none() && self.no_read_until.is_some_and(|t| now < t) {
            return;
        }
        self.timing_reads += 1;
        self.stale = false;
        match read_timing() {
            Some((t, _)) => {
                self.timing = Some(t);
                self.no_read_until = None;
            }
            None => {
                self.timing = None;
                self.no_read_until = Some(now + RETRY_AFTER_FAILURE);
            }
        }
    }

    /// Arms the timer for the frame start at or after `at`. Returns the
    /// instant it will fire.
    pub(crate) fn arm(&mut self, at: Instant, now: Instant) -> Instant {
        if self.align {
            self.refresh_timing(now);
        }
        self.last_arm = Some(now);
        let wait = at.saturating_duration_since(now);
        let now_q = qpc();
        let want_q = now_q + (wait.as_nanos() as i128 * self.freq as i128 / 1_000_000_000) as i64;
        let fire_q = match self.timing {
            Some(t) if self.align => align_to_frame(want_q, t),
            _ => want_q,
        };
        let ticks = (fire_q - now_q).max(0);
        let hundred_ns = (ticks as i128 * 10_000_000 / self.freq as i128) as i64;
        // Negative = relative to now, in 100 ns units; zero would mean "at
        // the epoch", so a due-now request waits one unit.
        let due = -(hundred_ns.max(1));
        // SAFETY: our own timer handle; `due` outlives the call; no completion routine.
        let set = unsafe { SetWaitableTimer(self.timer, &due, 0, None, None, false) };
        if let Err(e) = set {
            log::warn!("frame timer not armed: {e}");
        }
        let fires = now + Duration::from_nanos((hundred_ns as u64).saturating_mul(100));
        self.armed_for = Some(fires);
        fires
    }
}

impl Drop for Pacer {
    fn drop(&mut self) {
        // SAFETY: the handle was created in `new` and is closed once.
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.timer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Timing = Timing { period: 160_000, vblank: 1_000_000 };

    #[test]
    fn a_target_on_a_frame_start_stays() {
        assert_eq!(align_to_frame(1_000_000, T), 1_000_000);
        assert_eq!(align_to_frame(1_160_000, T), 1_160_000);
    }

    #[test]
    fn a_target_between_frames_moves_to_the_next_start() {
        assert_eq!(align_to_frame(1_000_001, T), 1_160_000);
        assert_eq!(align_to_frame(1_159_999, T), 1_160_000);
        assert_eq!(align_to_frame(1_320_001, T), 1_480_000);
    }

    #[test]
    fn a_target_before_the_reference_vblank_aligns_too() {
        assert_eq!(align_to_frame(900_000, T), 1_000_000);
        assert_eq!(align_to_frame(839_999, T), 840_000);
        assert_eq!(align_to_frame(-1, T), 1_000_000 - 160_000 * 6);
    }

    #[test]
    fn the_result_is_never_before_the_target_and_less_than_a_period_after_it() {
        for target in (-500_000i64..3_000_000).step_by(7_919) {
            let r = align_to_frame(target, T);
            assert!(r >= target, "{target} -> {r}");
            assert!(r - target < T.period, "{target} -> {r}");
            assert_eq!((r - T.vblank).rem_euclid(T.period), 0, "{target} -> {r}");
        }
    }

    #[test]
    fn a_zero_period_leaves_the_target_alone() {
        assert_eq!(align_to_frame(123, Timing { period: 0, vblank: 5 }), 123);
    }

    #[test]
    fn successive_one_frame_deadlines_land_in_successive_frames() {
        // A 16.7 ms animation step on a ~31 ms (32 Hz) display: each deadline
        // set at a frame start lands at the next frame start, not the same one.
        let period = 3_125_000i64; // 31.25 ms at 100 ns units
        let t = Timing { period, vblank: 0 };
        let step = 1_667_000i64;
        let mut now = 0i64;
        let mut frames = Vec::new();
        for _ in 0..10 {
            let fire = align_to_frame(now + step, t);
            frames.push(fire / period);
            now = fire;
        }
        assert!(frames.windows(2).all(|w| w[1] == w[0] + 1), "{frames:?}");
    }
}

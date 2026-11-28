//! UI-thread responsiveness probe: a thread sends `WM_NULL` to the hidden
//! top-level window with `SendMessageTimeoutW` every `period` and records how
//! long each round trip took. The worst gap says how long the UI thread was
//! unable to answer input.
//!
//! A sent message is answered from inside the target thread's message wait, so
//! with the probe on the UI thread wakes about 100 times a second: idle CPU
//! and wake counts taken with the probe running are not the idle numbers. The
//! native menu's modal loop answers sent messages too, so a menu being open
//! does not show as a gap, but the loop's own handler is not called while it
//! is open: frames and wakes wait for it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_NULL};

use crate::sys;

/// Resolution of the histogram: 0.05 ms bins up to 100 ms.
const BIN_MS: f64 = 0.05;
const BINS: usize = 2000;
/// Round trips at or over 100 ms are kept exactly, up to this many.
const BIG_CAP: usize = 4096;
const SEND_TIMEOUT_MS: u32 = 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProbeStats {
    pub samples: u64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    /// The worst round trip of the whole run, start-up included.
    pub worst_ms: f64,
    /// The worst round trip after the first bar frame was drawn: what the
    /// running bar does, without the one-off cost of building it.
    pub worst_after_first_frame_ms: f64,
    /// Sends that hit the 1 s timeout.
    pub timeouts: u64,
}

/// Round trips seen so far, as a histogram (a long run must not grow memory).
pub(crate) struct Histogram {
    bins: Vec<u32>,
    big: Vec<f64>,
    samples: u64,
    worst: f64,
    worst_after_first: f64,
    after_first: bool,
    timeouts: u64,
}

impl Histogram {
    pub(crate) fn new() -> Histogram {
        Histogram {
            bins: vec![0; BINS],
            big: Vec::new(),
            samples: 0,
            worst: 0.0,
            worst_after_first: 0.0,
            after_first: false,
            timeouts: 0,
        }
    }

    /// The first bar frame is on the screen: from here the worst gap is kept apart.
    pub(crate) fn first_frame_drawn(&mut self) {
        self.after_first = true;
    }

    pub(crate) fn record(&mut self, ms: f64, timed_out: bool) {
        self.samples += 1;
        if timed_out {
            self.timeouts += 1;
        }
        self.worst = self.worst.max(ms);
        if self.after_first {
            self.worst_after_first = self.worst_after_first.max(ms);
        }
        let bin = (ms / BIN_MS) as usize;
        if bin < BINS {
            self.bins[bin] += 1;
        } else if self.big.len() < BIG_CAP {
            self.big.push(ms);
        } else {
            // Over the cap, keep the count right and the value capped at the
            // worst seen: only the tail percentile is affected.
            self.big[BIG_CAP - 1] = self.worst;
        }
    }

    /// The value below which `q` (0..1) of the samples fall.
    fn quantile(&self, q: f64) -> f64 {
        if self.samples == 0 {
            return 0.0;
        }
        let target = ((self.samples as f64) * q).ceil().max(1.0) as u64;
        let mut seen = 0u64;
        for (i, n) in self.bins.iter().enumerate() {
            seen += u64::from(*n);
            if seen >= target {
                return (i as f64 + 1.0) * BIN_MS;
            }
        }
        let mut big = self.big.clone();
        big.sort_by(|a, b| a.total_cmp(b));
        let idx = (target - seen).saturating_sub(1) as usize;
        big.get(idx).copied().unwrap_or(self.worst)
    }

    pub(crate) fn stats(&self) -> ProbeStats {
        ProbeStats {
            samples: self.samples,
            p50_ms: self.quantile(0.50),
            p95_ms: self.quantile(0.95),
            worst_ms: self.worst,
            worst_after_first_frame_ms: self.worst_after_first,
            timeouts: self.timeouts,
        }
    }
}

pub struct UiProbe {
    hist: Arc<Mutex<Histogram>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl UiProbe {
    /// Started by `Ui` when `UiOptions::probe` is set; `period` 10 ms.
    pub fn period() -> Duration {
        Duration::from_millis(10)
    }

    /// Starts probing the window with this raw handle value. Stops when the
    /// probe is dropped.
    pub fn start(target: isize) -> UiProbe {
        let hist = Arc::new(Mutex::new(Histogram::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (h2, s2) = (Arc::clone(&hist), Arc::clone(&stop));
        let thread = std::thread::Builder::new()
            .name("rsb-ui-probe".into())
            .spawn(move || {
                while !s2.load(Ordering::Acquire) {
                    let t = Instant::now();
                    let mut result = 0usize;
                    // SAFETY: plain value arguments and a live out pointer; a
                    // destroyed window only makes the call fail.
                    let r = unsafe {
                        SendMessageTimeoutW(
                            sys::hwnd(target),
                            WM_NULL,
                            WPARAM(0),
                            LPARAM(0),
                            SMTO_ABORTIFHUNG,
                            SEND_TIMEOUT_MS,
                            Some(&mut result),
                        )
                    };
                    let ms = t.elapsed().as_secs_f64() * 1000.0;
                    if s2.load(Ordering::Acquire) {
                        break;
                    }
                    h2.lock().unwrap_or_else(|e| e.into_inner()).record(ms, r.0 == 0);
                    std::thread::sleep(UiProbe::period());
                }
            })
            .ok();
        UiProbe { hist, stop, thread }
    }

    pub fn stats(&self) -> ProbeStats {
        self.hist.lock().unwrap_or_else(|e| e.into_inner()).stats()
    }

    /// The first bar frame was drawn.
    pub fn first_frame_drawn(&self) {
        self.hist.lock().unwrap_or_else(|e| e.into_inner()).first_frame_drawn();
    }
}

impl Drop for UiProbe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_histogram_is_zero() {
        assert_eq!(Histogram::new().stats(), ProbeStats::default());
    }

    #[test]
    fn percentiles_of_a_known_set() {
        let mut h = Histogram::new();
        // 1 ms .. 100 ms excluded edge: 90 samples at 1 ms, 9 at 10 ms, 1 at 30 ms.
        for _ in 0..90 {
            h.record(1.0, false);
        }
        for _ in 0..9 {
            h.record(10.0, false);
        }
        h.record(30.0, false);
        let s = h.stats();
        assert_eq!(s.samples, 100);
        assert!((s.p50_ms - 1.0).abs() <= 0.1, "p50 {}", s.p50_ms);
        assert!((s.p95_ms - 10.0).abs() <= 0.1, "p95 {}", s.p95_ms);
        assert_eq!(s.worst_ms, 30.0);
        assert_eq!(s.timeouts, 0);
    }

    #[test]
    fn the_worst_gap_after_the_first_frame_is_kept_apart_from_the_start_up() {
        let mut h = Histogram::new();
        h.record(40.0, false); // building the first frame
        h.record(1.0, false);
        h.first_frame_drawn();
        h.record(6.0, false);
        h.record(2.0, false);
        let s = h.stats();
        assert_eq!((s.worst_ms, s.worst_after_first_frame_ms), (40.0, 6.0));
        assert_eq!(Histogram::new().stats().worst_after_first_frame_ms, 0.0);
    }

    #[test]
    fn large_gaps_and_timeouts_are_kept() {
        let mut h = Histogram::new();
        for _ in 0..19 {
            h.record(0.5, false);
        }
        h.record(1000.0, true);
        let s = h.stats();
        assert_eq!(s.timeouts, 1);
        assert_eq!(s.worst_ms, 1000.0);
        assert!(s.p95_ms <= 1.0);
        let mut all_big = Histogram::new();
        all_big.record(150.0, false);
        all_big.record(250.0, false);
        assert_eq!(all_big.stats().p50_ms, 150.0);
    }

    #[test]
    fn the_big_list_stops_growing() {
        let mut h = Histogram::new();
        for i in 0..(BIG_CAP + 100) {
            h.record(100.0 + i as f64, false);
        }
        assert_eq!(h.big.len(), BIG_CAP);
        assert_eq!(h.stats().samples, (BIG_CAP + 100) as u64);
    }
}

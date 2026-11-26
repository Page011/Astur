//! The watchdog: a thread that wakes once a second and looks at the script
//! thread's heartbeat. A call that has run for 2 s is something the
//! interrupt cannot see (a long call inside a library function or a Rust
//! callback): the UI is told, shows a bar plate and logs it once; when the
//! call returns the plate clears. Cost: 60 wakes a minute and nothing else.
//!
//! While an overlay (the full-screen power menu) is shown it also watches
//! the UI thread: a turn that has run for more than 2 s hides every overlay
//! window, so a hung topmost window never traps the person at the screen.
//! It wakes four times a second only while an overlay is shown.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rsb_core::msg::Heartbeat;
use rsb_core::wake::Wake;
use serde_json::json;

use crate::msg::UiMsg;
use crate::popups::OverlayWatch;
use crate::stats::Stats;

pub const STALL_AFTER: Duration = Duration::from_secs(2);
const PERIOD: Duration = Duration::from_secs(1);
/// How often it looks while an overlay is shown.
const OVERLAY_PERIOD: Duration = Duration::from_millis(250);

/// What the overlay check needs: the flag the UI sets while an overlay is
/// shown, how long the UI thread's current turn has run, and the call that
/// hides the overlay windows (it must not wait for the UI thread).
pub struct OverlayGuard {
    pub watch: Arc<OverlayWatch>,
    pub busy_for: Box<dyn Fn() -> Duration + Send>,
    pub hide: Box<dyn Fn() -> usize + Send>,
    pub stats: Arc<Stats>,
}

pub struct Watchdog {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
    wakes: Arc<AtomicU64>,
}

impl Watchdog {
    pub fn start(
        heartbeat: Arc<Heartbeat>,
        tx: Sender<UiMsg>,
        waker: Arc<dyn Wake>,
        overlays: Option<OverlayGuard>,
    ) -> Watchdog {
        Watchdog::start_guarded(heartbeat, tx, waker, PERIOD, STALL_AFTER, overlays)
    }

    /// The same with the timings chosen (tests shorten them).
    pub fn start_with(
        heartbeat: Arc<Heartbeat>,
        tx: Sender<UiMsg>,
        waker: Arc<dyn Wake>,
        period: Duration,
        stall_after: Duration,
    ) -> Watchdog {
        Watchdog::start_guarded(heartbeat, tx, waker, period, stall_after, None)
    }

    /// The same with the overlay check, and the timings chosen.
    pub fn start_guarded(
        heartbeat: Arc<Heartbeat>,
        tx: Sender<UiMsg>,
        waker: Arc<dyn Wake>,
        period: Duration,
        stall_after: Duration,
        overlays: Option<OverlayGuard>,
    ) -> Watchdog {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let wakes = Arc::new(AtomicU64::new(0));
        let (thread_stop, thread_wakes) = (Arc::clone(&stop), Arc::clone(&wakes));
        let thread = std::thread::Builder::new()
            .name("rsb-watchdog".into())
            .spawn(move || {
                let mut reported: Option<u64> = None;
                loop {
                    let (lock, cond) = &*thread_stop;
                    let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                    let overlay_up = overlays.as_ref().is_some_and(|g| g.watch.shown.load(Ordering::Acquire));
                    let wait = if overlay_up { OVERLAY_PERIOD.min(period) } else { period };
                    let (guard, _) = cond.wait_timeout_while(guard, wait, |s| !*s).unwrap_or_else(|e| e.into_inner());
                    thread_wakes.fetch_add(1, Ordering::Relaxed);
                    if *guard {
                        break;
                    }
                    drop(guard);
                    if let Some(g) = &overlays
                        && g.watch.shown.load(Ordering::Acquire)
                        && (g.busy_for)() >= STALL_AFTER
                    {
                        let windows = (g.hide)();
                        if windows > 0 {
                            let total = g.watch.hidden.fetch_add(1, Ordering::AcqRel) + 1;
                            g.watch.shown.store(false, Ordering::Release);
                            g.stats.emit("watchdog", json!({"overlays_hidden": total, "windows": windows}));
                            log::error!("the UI thread stopped answering; the overlay was hidden");
                        }
                    }
                    match heartbeat.busy(Instant::now()) {
                        Some((instance, ran, call)) if ran >= stall_after => {
                            if reported != Some(call) {
                                reported = Some(call);
                                let _ = tx.send(UiMsg::Stalled { instance, for_ms: ran.as_millis() as u64 });
                                waker.wake();
                            }
                        }
                        Some((_, _, call)) if reported == Some(call) => {}
                        _ => {
                            if reported.take().is_some() {
                                let _ = tx.send(UiMsg::StalledCleared);
                                waker.wake();
                            }
                        }
                    }
                }
            })
            .expect("the watchdog thread starts");
        Watchdog { stop, thread: Some(thread), wakes }
    }

    /// Times the thread woke (for `wakes.watchdog`).
    pub fn wakes(&self) -> u64 {
        self.wakes.load(Ordering::Relaxed)
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let (lock, cond) = &*self.stop;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cond.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsb_core::element::InstanceId;
    use std::sync::mpsc::channel;

    struct NoWake;
    impl Wake for NoWake {
        fn wake(&self) {}
    }

    const PERIOD_TEST: Duration = Duration::from_millis(40);
    const STALL_TEST: Duration = Duration::from_millis(200);

    #[test]
    fn a_stalled_call_is_reported_once_and_clears_when_it_returns() {
        let hb = Arc::new(Heartbeat::new());
        let (tx, rx) = channel();
        let dog = Watchdog::start_with(hb.clone(), tx, Arc::new(NoWake), PERIOD_TEST, STALL_TEST);
        hb.begin(InstanceId(7), Instant::now());
        let first = rx.recv_timeout(Duration::from_secs(2)).expect("reported");
        assert!(
            matches!(first, UiMsg::Stalled { instance: InstanceId(7), for_ms } if for_ms >= 200),
            "{}",
            match &first {
                UiMsg::Stalled { for_ms, .. } => format!("{for_ms} ms"),
                _ => "another message".into(),
            }
        );
        assert!(rx.recv_timeout(Duration::from_millis(400)).is_err(), "reported once for one call");
        hb.end();
        assert!(matches!(rx.recv_timeout(Duration::from_secs(2)), Ok(UiMsg::StalledCleared)));
        assert!(dog.wakes() >= 3);
        dog.stop();
    }

    #[test]
    fn short_calls_and_an_idle_script_thread_report_nothing() {
        let hb = Arc::new(Heartbeat::new());
        let (tx, rx) = channel();
        let dog = Watchdog::start_with(hb.clone(), tx, Arc::new(NoWake), PERIOD_TEST, Duration::from_secs(5));
        hb.begin(InstanceId(1), Instant::now());
        assert!(rx.recv_timeout(Duration::from_millis(400)).is_err());
        hb.end();
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        dog.stop();
    }

    #[test]
    fn a_stalled_ui_thread_hides_the_overlays_once_and_only_while_one_is_shown() {
        use std::sync::atomic::AtomicUsize;
        let hb = Arc::new(Heartbeat::new());
        let (tx, _rx) = channel();
        let watch = Arc::new(OverlayWatch::default());
        let busy_ms = Arc::new(AtomicU64::new(0));
        let hidden_calls = Arc::new(AtomicUsize::new(0));
        let (b, h) = (Arc::clone(&busy_ms), Arc::clone(&hidden_calls));
        let guard = OverlayGuard {
            watch: Arc::clone(&watch),
            busy_for: Box::new(move || Duration::from_millis(b.load(Ordering::Relaxed))),
            hide: Box::new(move || {
                h.fetch_add(1, Ordering::Relaxed);
                2
            }),
            stats: Arc::new(Stats::off()),
        };
        let dog = Watchdog::start_guarded(hb, tx, Arc::new(NoWake), PERIOD_TEST, Duration::from_secs(5), Some(guard));
        // A long stall with no overlay up hides nothing.
        busy_ms.store(3000, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(hidden_calls.load(Ordering::Relaxed), 0);
        // Overlay up, UI turn shorter than the limit: nothing.
        watch.shown.store(true, Ordering::Release);
        busy_ms.store(1999, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(hidden_calls.load(Ordering::Relaxed), 0);
        // Over 2 s: hidden, once, and the counter the UI reads moves.
        busy_ms.store(2001, Ordering::Relaxed);
        let until = Instant::now() + Duration::from_secs(3);
        while hidden_calls.load(Ordering::Relaxed) == 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(hidden_calls.load(Ordering::Relaxed), 1);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(hidden_calls.load(Ordering::Relaxed), 1, "the flag was cleared: no second hide for one stall");
        assert_eq!(watch.hidden.load(Ordering::Acquire), 1);
        assert!(!watch.shown.load(Ordering::Acquire));
        dog.stop();
    }

    #[test]
    fn the_real_timings_are_one_second_and_two() {
        assert_eq!((PERIOD, STALL_AFTER), (Duration::from_secs(1), Duration::from_secs(2)));
        assert_eq!(OVERLAY_PERIOD, Duration::from_millis(250));
    }
}

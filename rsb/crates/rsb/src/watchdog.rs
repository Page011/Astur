//! The watchdog: a thread that wakes once a second and looks at the script
//! thread's heartbeat. A call that has run for 2 s is something the
//! interrupt cannot see (a long call inside a library function or a Rust
//! callback): the UI is told, shows a bar plate and logs it once; when the
//! call returns the plate clears. Cost: 60 wakes a minute and nothing else.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rsb_core::msg::Heartbeat;
use rsb_core::wake::Wake;

use crate::msg::UiMsg;

pub const STALL_AFTER: Duration = Duration::from_secs(2);
const PERIOD: Duration = Duration::from_secs(1);

pub struct Watchdog {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
    wakes: Arc<AtomicU64>,
}

impl Watchdog {
    pub fn start(heartbeat: Arc<Heartbeat>, tx: Sender<UiMsg>, waker: Arc<dyn Wake>) -> Watchdog {
        Watchdog::start_with(heartbeat, tx, waker, PERIOD, STALL_AFTER)
    }

    /// The same with the timings chosen (tests shorten them).
    pub fn start_with(
        heartbeat: Arc<Heartbeat>,
        tx: Sender<UiMsg>,
        waker: Arc<dyn Wake>,
        period: Duration,
        stall_after: Duration,
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
                    let (guard, _) = cond.wait_timeout_while(guard, period, |s| !*s).unwrap_or_else(|e| e.into_inner());
                    thread_wakes.fetch_add(1, Ordering::Relaxed);
                    if *guard {
                        break;
                    }
                    drop(guard);
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
    fn the_real_timings_are_one_second_and_two() {
        assert_eq!((PERIOD, STALL_AFTER), (Duration::from_secs(1), Duration::from_secs(2)));
    }
}

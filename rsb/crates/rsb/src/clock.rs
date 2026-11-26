//! The UI thread's idea of "now". A running bar reads the system's monotonic
//! clock; the headless timeline renderer replaces it with a virtual time it
//! moves by hand, so a frame drawn at 400 ms is the same frame in every run.
//!
//! Only the UI thread's scheduling goes through here (frames, gestures, tooltip
//! delays). Elapsed-time measurements of real work (`render_us`) still read the
//! system clock.

use std::cell::Cell;
use std::time::Instant;

thread_local! {
    static VIRTUAL: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// The current time: the virtual one when this thread has one, else the system's.
pub fn now() -> Instant {
    VIRTUAL.with(Cell::get).unwrap_or_else(Instant::now)
}

/// Sets this thread's virtual time; `None` goes back to the system clock.
pub fn set_virtual(at: Option<Instant>) {
    VIRTUAL.with(|v| v.set(at));
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn the_virtual_time_replaces_the_system_clock_on_this_thread_only() {
        let base = Instant::now() + Duration::from_secs(3600);
        set_virtual(Some(base));
        assert_eq!(now(), base);
        assert_eq!(now(), base, "it does not move by itself");
        let other = std::thread::spawn(|| now() < Instant::now() + Duration::from_secs(60)).join().unwrap();
        assert!(other, "another thread still reads the system clock");
        set_virtual(None);
        assert!(now() < base);
    }
}

//! The console: rsb is a console-subsystem program so `%ERRORLEVEL%` in
//! cmd.exe is its own exit code. Run mode lets go of a console that was made
//! just for it, and turns console close, Ctrl+C, logoff and shutdown into the
//! normal stop order.

use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

use windows::Win32::System::Console::{
    CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT, FreeConsole,
    GetConsoleProcessList, SetConsoleCtrlHandler,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETHUNGAPPTIMEOUT, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};
use windows::core::BOOL;

/// The longest a console event waits for the stop order. Windows ends the
/// process at the hung-app timeout after a close event (5 s unless the user
/// lowered it), so the fallback that gives the screen space back has to run
/// well before that: see `stop_wait`.
const STOP_WAIT: Duration = Duration::from_secs(3);

/// Time to leave the fallback between the end of the wait and the hung-app
/// timeout, so removing the appbars finishes before the process is ended.
const FALLBACK_ROOM: Duration = Duration::from_millis(1500);

/// The shortest wait: a normal stop order finishes well inside it.
const STOP_WAIT_FLOOR: Duration = Duration::from_millis(500);

/// What Windows allows a close handler before it ends the process.
#[cfg(test)]
const CLOSE_EVENT_LIMIT: Duration = Duration::from_secs(5);

type Hook = Box<dyn Fn() + Send + Sync>;

static HOOK: OnceLock<Hook> = OnceLock::new();
static STOPPED: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// `GetConsoleProcessList` returns 1 when this process is alone on its
/// console: it was started from Explorer or a shortcut and the console only
/// exists for it, so let go of it (a brief flash). Started from cmd.exe the
/// list holds the shell too and the console stays. No console: nothing.
pub fn detach_if_own_console() {
    let mut list = [0u32; 2];
    // SAFETY: `list` is a live buffer of the length passed.
    let count = unsafe { GetConsoleProcessList(&mut list) };
    if count == 1 {
        // SAFETY: no arguments; failing just leaves the console attached.
        let _ = unsafe { FreeConsole() };
    }
}

unsafe extern "system" fn on_ctrl(kind: u32) -> BOOL {
    match kind {
        CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => {
            if let Some(hook) = HOOK.get() {
                hook();
            }
            // The UI thread is busy (a native menu is open, for one): give the
            // screen space back from here, as the process ends right after.
            wait_for_stop(&STOPPED, stop_wait(hung_app_timeout()), || {
                log::warn!("the stop order did not finish in time; releasing the screen space from the console thread");
                rsb_win::remove_all_appbars();
            });
            BOOL(1)
        }
        _ => BOOL(0),
    }
}

/// The hung-app timeout in milliseconds, `None` when Windows will not say.
fn hung_app_timeout() -> Option<u32> {
    let mut ms = 0u32;
    // SAFETY: `ms` is a live u32, which is what SPI_GETHUNGAPPTIMEOUT writes.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETHUNGAPPTIMEOUT,
            0,
            Some(std::ptr::from_mut(&mut ms).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    read.ok().map(|()| ms)
}

/// How long to wait for the stop order given the hung-app timeout: `STOP_WAIT`,
/// or less when the timeout is short enough that a longer wait would leave the
/// fallback no time, but never below `STOP_WAIT_FLOOR`.
fn stop_wait(hung_app_timeout_ms: Option<u32>) -> Duration {
    let Some(ms) = hung_app_timeout_ms else {
        return STOP_WAIT;
    };
    let room = Duration::from_millis(u64::from(ms)).saturating_sub(FALLBACK_ROOM);
    room.min(STOP_WAIT).max(STOP_WAIT_FLOOR)
}

/// Waits until `state` says the stop order has finished; `on_timeout` runs
/// when it has not after `timeout`. Returns whether it finished.
fn wait_for_stop(state: &(Mutex<bool>, Condvar), timeout: Duration, on_timeout: impl FnOnce()) -> bool {
    let (lock, cond) = state;
    let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let (guard, _) = cond.wait_timeout_while(guard, timeout, |done| !*done).unwrap_or_else(|e| e.into_inner());
    let done = *guard;
    drop(guard);
    if !done {
        on_timeout();
    }
    done
}

/// `request_stop` asks the UI thread to quit (it only posts a message). The
/// handler then waits up to `STOP_WAIT` for `mark_stopped`, so the process is not
/// ended before the work area is restored.
pub fn install_ctrl_handler(request_stop: impl Fn() + Send + Sync + 'static) {
    let _ = HOOK.set(Box::new(request_stop));
    // SAFETY: `on_ctrl` matches the handler signature and lives for the whole process.
    if unsafe { SetConsoleCtrlHandler(Some(on_ctrl), true) }.is_err() {
        log::warn!("the console close handler could not be installed");
    }
}

/// The stop order has finished: let a waiting console event return.
pub fn mark_stopped() {
    let (lock, cond) = &STOPPED;
    *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
    cond.notify_all();
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[test]
    fn the_wait_ends_well_before_windows_ends_the_process() {
        assert!(STOP_WAIT + Duration::from_secs(1) <= CLOSE_EVENT_LIMIT, "the fallback needs time to run");
    }

    #[test]
    fn the_wait_follows_a_lowered_hung_app_timeout() {
        let ms = |v: u32| stop_wait(Some(v));
        assert_eq!(ms(5000), STOP_WAIT, "the default timeout keeps the full wait");
        assert_eq!(ms(4500), STOP_WAIT);
        assert_eq!(ms(4000), Duration::from_millis(2500));
        assert_eq!(ms(3000), Duration::from_millis(1500));
        assert_eq!(ms(2000), Duration::from_millis(500));
        assert_eq!(ms(1000), STOP_WAIT_FLOOR, "never below the floor");
        assert_eq!(ms(0), STOP_WAIT_FLOOR);
        assert_eq!(stop_wait(None), STOP_WAIT, "an unreadable setting keeps the default");
        // A timeout of 2 s or more always leaves the fallback its room.
        for v in (2000..=20_000).step_by(250) {
            assert!(ms(v) + FALLBACK_ROOM <= Duration::from_millis(u64::from(v)), "{v} ms");
        }
    }

    #[test]
    fn the_hung_app_timeout_can_be_read() {
        let ms = hung_app_timeout().expect("the setting is readable");
        assert!(ms > 0, "{ms}");
    }

    #[test]
    fn a_stop_order_that_never_finishes_runs_the_timeout_action() {
        let state = (Mutex::new(false), Condvar::new());
        let ran = AtomicBool::new(false);
        let done = wait_for_stop(&state, Duration::from_millis(50), || ran.store(true, Ordering::Relaxed));
        assert!(!done);
        assert!(ran.load(Ordering::Relaxed), "the screen space is released from here");
    }

    #[test]
    fn a_finished_stop_order_does_not_run_it() {
        let state = (Mutex::new(false), Condvar::new());
        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(30));
                *state.0.lock().unwrap() = true;
                state.1.notify_all();
            });
            let done = wait_for_stop(&state, Duration::from_secs(10), || panic!("it finished in time"));
            assert!(done);
        });
        let late = (Mutex::new(true), Condvar::new());
        assert!(wait_for_stop(&late, Duration::from_millis(1), || panic!("already finished")));
    }
}

//! The one way every service waits for an asynchronous WinRT operation: poll its status against a
//! deadline.
//!
//! A completion handler registered on the operation runs on a thread-pool thread that can still be unwinding
//! when the waiting thread leaves its apartment with `RoUninitialize`; that crashed test binaries
//! about once in two runs. Polling keeps every object on the waiting thread, and a timed-out
//! operation is cancelled with no callback left behind to run into freed state.

use std::time::{Duration, Instant};

use windows::core::{Error, HRESULT};

/// `HRESULT_FROM_WIN32(ERROR_TIMEOUT)`.
pub(crate) const TIMED_OUT: HRESULT = HRESULT(0x8007_05B4_u32 as i32);
/// Gap between status reads. Operations here finish in a few milliseconds when they finish at all.
const POLL: Duration = Duration::from_millis(5);

/// What one status read says about the operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Progress {
    Running,
    /// Finished, however it ended: the result call reports success or the error.
    Finished,
}

/// Reads `status` every few milliseconds until the operation is not running, for at most `wait`.
/// On timeout `cancel` is called once and the error is `TIMED_OUT`. A status that cannot be read
/// cancels as well: the operation is of no further use.
pub(crate) fn poll_until(
    mut status: impl FnMut() -> windows::core::Result<Progress>,
    cancel: impl FnOnce(),
    wait: Duration,
) -> windows::core::Result<()> {
    let end = Instant::now() + wait;
    loop {
        match status() {
            Ok(Progress::Finished) => return Ok(()),
            Ok(Progress::Running) => {}
            Err(e) => {
                cancel();
                return Err(e);
            }
        }
        if Instant::now() >= end {
            cancel();
            return Err(Error::from(TIMED_OUT));
        }
        std::thread::sleep(POLL);
    }
}

/// The result of an async operation, waiting at most `$wait` for it; a timed-out operation is
/// cancelled and gives `TIMED_OUT`. A macro because the operation types (`IAsyncOperation<T>`,
/// `DataReaderLoadOperation`, ...) share no trait that is exported by the `windows` crate.
macro_rules! await_op {
    ($op:expr, $wait:expr) => {{
        let op = &$op;
        $crate::real::winrt::poll_until(
            || {
                // `AsyncStatus::Started` is 0.
                Ok(if op.Status()?.0 == 0 {
                    $crate::real::winrt::Progress::Running
                } else {
                    $crate::real::winrt::Progress::Finished
                })
            },
            || drop(op.Cancel()),
            $wait,
        )
        .and_then(|()| op.GetResults())
    }};
}
pub(crate) use await_op;

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn an_operation_that_never_finishes_times_out_and_is_cancelled_once() {
        let cancels = Cell::new(0);
        let reads = Cell::new(0);
        let t = Instant::now();
        let e = poll_until(
            || {
                reads.set(reads.get() + 1);
                Ok(Progress::Running)
            },
            || cancels.set(cancels.get() + 1),
            Duration::from_millis(200),
        )
        .expect_err("still running");
        assert_eq!(e.code(), TIMED_OUT);
        assert_eq!(cancels.get(), 1, "a timed-out operation is cancelled exactly once");
        assert!(t.elapsed() >= Duration::from_millis(200) && t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        assert!(reads.get() >= 5, "it polls, not once: {} reads", reads.get());
    }

    #[test]
    fn a_finished_operation_returns_at_once_and_is_not_cancelled() {
        let t = Instant::now();
        let reads = Cell::new(0);
        let done = poll_until(
            || {
                reads.set(reads.get() + 1);
                Ok(if reads.get() < 3 { Progress::Running } else { Progress::Finished })
            },
            || panic!("a finished operation is not cancelled"),
            Duration::from_secs(5),
        );
        assert!(done.is_ok());
        assert_eq!(reads.get(), 3);
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
    }

    #[test]
    fn a_status_that_cannot_be_read_is_an_error_and_cancels() {
        let cancels = Cell::new(0);
        let e = poll_until(
            || Err(Error::from(HRESULT(0x8000_4005_u32 as i32))),
            || cancels.set(cancels.get() + 1),
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert_eq!(e.code().0, 0x8000_4005_u32 as i32);
        assert_eq!(cancels.get(), 1);
    }

    #[test]
    fn a_zero_wait_still_reads_the_status_once() {
        let reads = Cell::new(0);
        let done = poll_until(
            || {
                reads.set(reads.get() + 1);
                Ok(Progress::Finished)
            },
            || {},
            Duration::ZERO,
        );
        assert!(done.is_ok() && reads.get() == 1);
    }

    #[test]
    fn a_real_operation_that_is_already_done_is_read_through_the_macro() {
        // SAFETY: initialises the WinRT apartment of this test thread; the thread ends with the test.
        let _ = unsafe {
            windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED)
        };
        // A radio list needs no radio: the operation finishes with an empty list.
        let op = windows::Devices::Radios::Radio::GetRadiosAsync().expect("the radio list can be asked for");
        let list = await_op!(op, Duration::from_secs(5)).expect("the radio list finishes");
        println!("radio list through the helper: {} entries", list.Size().unwrap_or(0));
    }
}

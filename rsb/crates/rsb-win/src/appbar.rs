//! `SHAppBarMessage` helpers. The appbar protocol: register (`ABM_NEW`), ask
//! where the shell would put us (`ABM_QUERYPOS`), claim that rectangle
//! (`ABM_SETPOS`), remove on exit (`ABM_REMOVE`). The shell sends
//! `ABN_POSCHANGED` for our own `SETPOS` too, so callers skip an unchanged
//! rectangle or the two would answer each other forever.

use std::sync::Mutex;

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::UI::Shell::{APPBARDATA, SHAppBarMessage};

use crate::sys;

const ABM_NEW: u32 = 0;
const ABM_REMOVE: u32 = 1;
const ABM_QUERYPOS: u32 = 2;
const ABM_SETPOS: u32 = 3;

pub(crate) const ABE_TOP: u32 = 1;
pub(crate) const ABE_BOTTOM: u32 = 3;

pub(crate) const ABN_POSCHANGED: usize = 1;
pub(crate) const ABN_FULLSCREENAPP: usize = 2;

/// Callback message the shell posts our notifications on (`WM_APP + 1`).
pub(crate) const CALLBACK_MSG: u32 = 0x8001;

fn message(msg: u32, hwnd: HWND, edge: u32, rc: RECT) -> (usize, RECT) {
    let mut d = APPBARDATA {
        cbSize: size_of::<APPBARDATA>() as u32,
        hWnd: hwnd,
        uCallbackMessage: CALLBACK_MSG,
        uEdge: edge,
        rc,
        lParam: LPARAM(0),
    };
    // SAFETY: `d` is a fully initialised APPBARDATA with the right cbSize.
    let r = unsafe { SHAppBarMessage(msg, &mut d) };
    (r, d.rc)
}

/// Windows with a live registration, for `remove_all` when the process cannot
/// shut down in order (a panic): the shell keeps a dead appbar's space until
/// told otherwise.
static LIVE: Mutex<Vec<isize>> = Mutex::new(Vec::new());

fn live() -> std::sync::MutexGuard<'static, Vec<isize>> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Makes the next `n` registrations fail, as the shell does before it is up.
#[cfg(test)]
pub(crate) static REFUSE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// False when the shell refuses (already registered, or no shell).
pub(crate) fn register(hwnd: isize) -> bool {
    #[cfg(test)]
    {
        use std::sync::atomic::Ordering::{AcqRel, Acquire};
        let left = REFUSE.load(Acquire);
        if left > 0 && REFUSE.compare_exchange(left, left - 1, AcqRel, Acquire).is_ok() {
            return false;
        }
    }
    let ok = message(ABM_NEW, sys::hwnd(hwnd), ABE_TOP, RECT::default()).0 != 0;
    if ok {
        live().push(hwnd);
    }
    ok
}

pub(crate) fn remove(hwnd: isize) {
    live().retain(|h| *h != hwnd);
    message(ABM_REMOVE, sys::hwnd(hwnd), ABE_TOP, RECT::default());
}

/// Removes every registration this process holds, from any thread. For the panic hook and for a console
/// close the window loop cannot answer.
pub(crate) fn remove_all() {
    let all: Vec<isize> = std::mem::take(&mut *live());
    for hwnd in all {
        message(ABM_REMOVE, sys::hwnd(hwnd), ABE_TOP, RECT::default());
    }
}

/// The rectangle the shell would grant for `rc` on `edge`.
pub(crate) fn query(hwnd: isize, edge: u32, rc: RECT) -> RECT {
    message(ABM_QUERYPOS, sys::hwnd(hwnd), edge, rc).1
}

pub(crate) fn set_pos(hwnd: isize, edge: u32, rc: RECT) {
    message(ABM_SETPOS, sys::hwnd(hwnd), edge, rc);
}

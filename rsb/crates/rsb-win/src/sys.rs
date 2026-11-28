//! Small wrappers over Win32 calls the rest of the crate makes in several
//! places: posting, timers, error text, the registry read for the theme.

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, PostMessageW, SendMessageW, SetTimer};
use windows::core::w;

use crate::WinError;

pub(crate) fn hwnd(h: isize) -> HWND {
    HWND(h as *mut c_void)
}

pub(crate) fn raw(h: HWND) -> isize {
    h.0 as isize
}

/// `PostMessageW`; false when the window is gone or the queue is full.
pub(crate) fn post(h: isize, msg: u32, wp: usize, lp: isize) -> bool {
    // SAFETY: plain value arguments; a stale handle only makes the call fail.
    unsafe { PostMessageW(Some(hwnd(h)), msg, WPARAM(wp), LPARAM(lp)) }.is_ok()
}

/// `SendMessageW` to a window of this thread: its procedure runs before this
/// returns. Used for the messages Windows refuses to post (system messages
/// with a pointer or structure argument fail with `ERROR_MESSAGE_SYNC_ONLY`).
/// Only for windows this process created; the procedures here only record.
pub(crate) fn send(h: isize, msg: u32, wp: usize, lp: isize) {
    // SAFETY: plain value arguments to one of our own windows.
    unsafe {
        SendMessageW(hwnd(h), msg, Some(WPARAM(wp)), Some(LPARAM(lp)));
    }
}

pub(crate) fn set_timer(h: isize, id: usize, ms: u32) -> bool {
    // SAFETY: plain value arguments; no callback, the timer posts WM_TIMER.
    unsafe { SetTimer(Some(hwnd(h)), id, ms, None) != 0 }
}

pub(crate) fn kill_timer(h: isize, id: usize) {
    // SAFETY: plain value arguments; failure (no such timer) is fine.
    unsafe {
        let _ = KillTimer(Some(hwnd(h)), id);
    }
}

/// The calling thread's last Win32 error as a `WinError`.
pub(crate) fn last_error(call: &'static str) -> WinError {
    WinError::Call { call, message: windows::core::Error::from_thread().message() }
}

pub(crate) fn call_error(call: &'static str, e: &windows::core::Error) -> WinError {
    WinError::Call { call, message: e.message() }
}

/// Windows' "app mode" is dark: `AppsUseLightTheme` is 0. Read only. A missing
/// value (older Windows) means light.
pub(crate) fn read_apps_dark() -> bool {
    let mut data: u32 = 1;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: `data` and `size` are live locals of the sizes passed; the key
    // and value names are 'static wide literals.
    let err = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    err.0 == 0 && data == 0
}

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

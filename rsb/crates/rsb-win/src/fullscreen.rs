//! A plain fullscreen window of our own, for checking how bars react to a
//! fullscreen app.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG, MWMO_INPUTAVAILABLE,
    MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, QS_ALLINPUT, RegisterClassExW, SW_SHOW, SetForegroundWindow,
    ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WNDCLASSEXW, WS_POPUP, WS_VISIBLE,
};
use windows::core::w;

use crate::{WinError, monitor, sys};

/// The window takes the foreground while it is open, which is what Windows
/// needs before it sends `ABN_FULLSCREENAPP`, and the user may be using the
/// PC: never longer than this.
const MAX_OPEN: Duration = Duration::from_secs(1);

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: forwarding the arguments Windows gave us.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// Opens a borderless window over the whole monitor on its own thread and
/// closes it after `for_time` (1 s at most). Returns once the window exists.
pub fn spawn_fullscreen_test_window(monitor: &str, for_time: Duration) -> Result<(), WinError> {
    let mon = monitor::find(monitor).ok_or_else(|| WinError::NoSuchMonitor(monitor.to_string()))?;
    let open_for = for_time.min(MAX_OPEN);
    let (tx, rx) = mpsc::channel::<Result<(), WinError>>();
    std::thread::Builder::new()
        .name("rsb-fullscreen-test".into())
        .spawn(move || {
            let made = make_window(&mon.rect);
            let hwnd = match made {
                Ok(h) => {
                    let _ = tx.send(Ok(()));
                    h
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            take_foreground(hwnd);
            let end = Instant::now() + open_for;
            loop {
                pump();
                let left = end.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                // SAFETY: no handles; only waits for input or the timeout.
                unsafe {
                    MsgWaitForMultipleObjectsEx(None, left.as_millis().max(1) as u32, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
                }
            }
            // SAFETY: the window belongs to this thread.
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        })
        .map_err(|e| WinError::Call { call: "thread spawn", message: e.to_string() })?;
    rx.recv_timeout(Duration::from_secs(2)).unwrap_or_else(|_| {
        Err(WinError::Call { call: "CreateWindowExW", message: "no answer from the window thread".into() })
    })
}

/// Shows the window and asks for the foreground. Windows refuses that to a
/// process that is not the foreground process (and in a disconnected session
/// there is no foreground at all): then no fullscreen notice follows, and the
/// caller sees it as a missing `Fullscreen` event. Nothing is forced.
fn take_foreground(hwnd: HWND) {
    // SAFETY: `hwnd` is the window this thread just made.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = BringWindowToTop(hwnd);
        if !SetForegroundWindow(hwnd).as_bool() {
            log::debug!("the fullscreen test window was not given the foreground");
        }
    }
}

fn make_window(rect: &rsb_core::geom::RectPx) -> Result<HWND, WinError> {
    // SAFETY: the class name and procedure are 'static; the handles are None
    // or ones Windows just returned.
    unsafe {
        let hinst = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
        let class = w!("RsbFullscreenTest");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc_),
            hInstance: hinst,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc); // fails harmlessly when the class already exists
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("RSB fullscreen test"),
            WS_POPUP | WS_VISIBLE,
            rect.x0,
            rect.y0,
            rect.width(),
            rect.height(),
            None,
            None,
            Some(hinst),
            None,
        )
    }
    .map_err(|e| sys::call_error("CreateWindowExW", &e))
}

fn pump() {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid out pointer; messages go to this thread's own
    // window procedure.
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Whether a window of process `window_pid` belongs to us (`own_pid`). No
/// process id (no such window, or one we cannot see) is not ours.
pub(crate) fn is_own_process(window_pid: Option<u32>, own_pid: u32) -> bool {
    window_pid == Some(own_pid)
}

/// The class of the stand-in window `spawn_fullscreen_test_window` opens: it
/// plays another app's fullscreen window, so it does not count as ours even
/// though this process made it.
const STAND_IN_CLASS: &str = "RsbFullscreenTest";

/// The foreground window is one of ours (an overlay, a popup): the shell's
/// fullscreen notice then describes our own window, not another app.
pub(crate) fn foreground_is_ours() -> bool {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId};
    // SAFETY: no arguments; the handle may be null (no foreground window).
    let fg = unsafe { GetForegroundWindow() };
    if fg.0.is_null() {
        return false;
    }
    let mut pid = 0u32;
    // SAFETY: a live out pointer; a stale handle only fails the call.
    let thread = unsafe { GetWindowThreadProcessId(fg, Some(&mut pid)) };
    // SAFETY: no arguments.
    let own = unsafe { GetCurrentProcessId() };
    if !is_own_process((thread != 0).then_some(pid), own) {
        return false;
    }
    let mut name = [0u16; 64];
    // SAFETY: the buffer length is the slice length.
    let len = unsafe { GetClassNameW(fg, &mut name) }.max(0) as usize;
    String::from_utf16_lossy(&name[..len.min(name.len())]) != STAND_IN_CLASS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_is_ours_only_when_its_process_id_is_ours() {
        assert!(is_own_process(Some(4242), 4242));
        assert!(!is_own_process(Some(4243), 4242));
        assert!(!is_own_process(None, 4242));
        assert!(!is_own_process(Some(0), 4242));
        assert!(is_own_process(Some(0), 0), "equal ids are equal, whatever they are");
    }
}

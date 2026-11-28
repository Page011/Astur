//! The foreground window: its title and its program's file name.
//!
//! A WinEvent hook (out of context, so no DLL is injected anywhere) reports
//! foreground changes and title changes on a thread that runs a message
//! loop. Titles come from `InternalGetWindowText`, which never sends a
//! message to the window, so a hung program cannot hang the bar. Name
//! changes are published at most 4 times a second. RSB's own windows and
//! the shell's desktop and taskbar count as "no window". Titles are values
//! for widgets and are never logged.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rsb_core::services::{ServiceCommand, ServiceId};
use rsb_core::value::Value;
use windows::Win32::Foundation::{CloseHandle, HWND, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EVENT_OBJECT_NAMECHANGE, EVENT_SYSTEM_FOREGROUND, GetClassNameW, GetForegroundWindow,
    GetWindowThreadProcessId, InternalGetWindowText, IsWindowVisible, MSG, MWMO_INPUTAVAILABLE,
    MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, QS_ALLINPUT, TranslateMessage, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS,
};
use windows::core::PWSTR;

use super::Worker;
use crate::util::{cut_chars, map, text};
use crate::win::Owned;
use crate::{Publisher, Service};

const REREAD: Duration = Duration::from_secs(60);
/// Title changes are published at most this often (4 a second).
const NAME_THROTTLE: Duration = Duration::from_millis(250);
const MAX_TITLE: usize = 256;

/// Window classes of the shell's own surfaces.
const SHELL_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "NotifyIconOverflowWindow",
    "Windows.UI.Core.CoreWindow",
];

/// Title and program file name of a window, with no visibility or ownership
/// rules applied.
pub(crate) fn read_window(hwnd: HWND) -> Option<(String, String)> {
    let mut buf = [0u16; 512];
    // SAFETY: `buf` is a writable slice; InternalGetWindowText sends no message to the window.
    let n = unsafe { InternalGetWindowText(hwnd, &mut buf) }.max(0) as usize;
    let title = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
    let mut pid = 0u32;
    // SAFETY: a live out pointer.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }
    Some((title, process_file_name(pid).unwrap_or_default()))
}

/// The file name (no folder) of a process's program.
pub(crate) fn process_file_name(pid: u32) -> Option<String> {
    // SAFETY: plain flag and id; the handle is closed below.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` and `len` describe a writable buffer.
    let r = unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) };
    // SAFETY: closes the handle opened above.
    let _ = unsafe { CloseHandle(h) };
    r.ok()?;
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    path.rsplit(['\\', '/']).next().map(str::to_string)
}

fn none_value() -> Value {
    map([("has_window", Value::Bool(false)), ("title", text("")), ("process", text(""))])
}

/// The value for the window `hwnd` as a widget sees it.
fn foreground_value(hwnd: HWND) -> Value {
    if hwnd.0.is_null() {
        return none_value();
    }
    // SAFETY: plain window-handle queries; a stale handle just fails.
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return none_value();
    }
    let mut pid = 0u32;
    // SAFETY: a live out pointer.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    // SAFETY: no arguments.
    if pid == 0 || pid == unsafe { GetCurrentProcessId() } {
        return none_value();
    }
    let mut class = [0u16; 128];
    // SAFETY: `class` is a writable slice.
    let n = unsafe { GetClassNameW(hwnd, &mut class) }.max(0) as usize;
    if SHELL_CLASSES.contains(&String::from_utf16_lossy(&class[..n]).as_str()) {
        return none_value();
    }
    match read_window(hwnd) {
        Some((title, process)) => map([
            ("has_window", Value::Bool(true)),
            ("title", Value::Str(cut_chars(&title, MAX_TITLE))),
            ("process", Value::Str(process)),
        ]),
        None => none_value(),
    }
}

struct HookState {
    out: Publisher,
    foreground: isize,
    last_name_publish: Instant,
    pending_name: bool,
}

thread_local! {
    /// The hook callback runs on the thread that installed it, inside its message loop.
    static STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

unsafe extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(st) = s.as_mut() else { return };
        if event == EVENT_SYSTEM_FOREGROUND {
            st.foreground = hwnd.0 as isize;
            st.out.publish(foreground_value(hwnd));
            st.last_name_publish = Instant::now();
            st.pending_name = false;
        } else if event == EVENT_OBJECT_NAMECHANGE
            && id_object == 0
            && id_child == 0
            && hwnd.0 as isize == st.foreground
        {
            if st.last_name_publish.elapsed() >= NAME_THROTTLE {
                st.out.publish(foreground_value(hwnd));
                st.last_name_publish = Instant::now();
                st.pending_name = false;
            } else {
                st.pending_name = true;
            }
        }
    });
}

fn run(out: Publisher, stop: Arc<Owned>) {
    // SAFETY: no arguments.
    let fg = unsafe { GetForegroundWindow() };
    out.publish(foreground_value(fg));
    STATE.with(|s| {
        *s.borrow_mut() = Some(HookState {
            out: out.clone(),
            foreground: fg.0 as isize,
            last_name_publish: Instant::now(),
            pending_name: false,
        });
    });
    let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
    // SAFETY: the callback is a plain function; out-of-context hooks need no module.
    let hooks = [
        unsafe { SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(on_event), 0, 0, flags) },
        unsafe { SetWinEventHook(EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_NAMECHANGE, None, Some(on_event), 0, 0, flags) },
    ];
    if hooks.iter().any(|h| h.is_invalid()) {
        out.status(rsb_core::services::ServiceStatus::Error, Some("the window hooks could not be installed".into()));
    }
    loop {
        let pending = STATE.with(|s| s.borrow().as_ref().is_some_and(|st| st.pending_name));
        let wait = if pending { NAME_THROTTLE } else { REREAD };
        crate::count_source_wake();
        // SAFETY: the stop event is live; messages (hook callbacks arrive as messages) wake the wait.
        let waited = unsafe {
            MsgWaitForMultipleObjectsEx(Some(&[stop.get()]), wait.as_millis() as u32, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        };
        if waited == WAIT_OBJECT_0 {
            break;
        }
        if waited == WAIT_TIMEOUT {
            // Settled name changes, or the minute check (an unchanged value still counts as alive).
            // SAFETY: no arguments.
            let fg = unsafe { GetForegroundWindow() };
            STATE.with(|s| {
                if let Some(st) = s.borrow_mut().as_mut() {
                    st.foreground = fg.0 as isize;
                    st.out.publish(foreground_value(fg));
                    st.last_name_publish = Instant::now();
                    st.pending_name = false;
                }
            });
            continue;
        }
        let mut msg = MSG::default();
        // SAFETY: standard message pump; the hooks' callbacks run inside DispatchMessageW / PeekMessageW.
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            // SAFETY: `msg` was filled by PeekMessageW.
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    for h in hooks {
        if !h.is_invalid() {
            // SAFETY: a hook handle from SetWinEventHook, unhooked once on the installing thread.
            let _ = unsafe { UnhookWinEvent(h) };
        }
    }
    STATE.with(|s| *s.borrow_mut() = None);
}

pub(crate) struct WindowService {
    worker: Option<Worker>,
}

impl WindowService {
    pub(crate) fn new() -> WindowService {
        WindowService { worker: None }
    }
}

impl Service for WindowService {
    fn id(&self) -> ServiceId {
        ServiceId::Window
    }

    fn start(&mut self, out: Publisher) -> Result<(), String> {
        self.worker = Some(Worker::spawn("rsb-window", move |stop| run(out, stop))?);
        Ok(())
    }

    fn command(&mut self, _command: ServiceCommand) -> Result<(), String> {
        Err("the window service has no commands".into())
    }

    fn stop(&mut self) {
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
    }

    fn stale_after(&self) -> Duration {
        Duration::from_secs(180)
    }
}

#[cfg(test)]
mod tests {
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP};
    use windows::core::w;

    use super::*;

    /// A never-shown window of this process with a known title.
    fn own_window() -> HWND {
        // SAFETY: the STATIC class is built in; the window is hidden and destroyed by the test.
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("title-for-the-test"),
                WS_POPUP,
                0,
                0,
                10,
                10,
                None,
                None,
                None,
                None,
            )
        }
        .expect("a hidden window")
    }

    #[test]
    fn the_program_name_of_a_window_is_the_file_name_of_its_process() {
        let hwnd = own_window();
        let (title, process) = read_window(hwnd).expect("readable");
        // SAFETY: destroys the window made above, on the same thread.
        let _ = unsafe { DestroyWindow(hwnd) };
        let exe = std::env::current_exe().unwrap();
        let want = exe.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(title, "title-for-the-test");
        assert!(process.eq_ignore_ascii_case(&want), "file name only: {} vs {}", process.len(), want.len());
        assert!(!process.contains(['\\', '/']));
    }

    #[test]
    fn our_own_windows_and_nothing_count_as_no_window() {
        let hwnd = own_window();
        let own = foreground_value(hwnd);
        // SAFETY: as above.
        let _ = unsafe { DestroyWindow(hwnd) };
        assert_eq!(own.get("has_window"), Some(&Value::Bool(false)), "our own process never counts");
        assert_eq!(foreground_value(HWND::default()).get("has_window"), Some(&Value::Bool(false)));
        assert_eq!(rsb_core::services::check_value(ServiceId::Window, &none_value()), Ok(()));
    }

    #[test]
    fn a_process_that_cannot_be_opened_has_no_name() {
        assert_eq!(process_file_name(0), None);
        assert!(process_file_name(std::process::id()).is_some());
    }

    #[test]
    fn the_shell_surfaces_are_listed() {
        for class in ["Progman", "WorkerW", "Shell_TrayWnd"] {
            assert!(SHELL_CLASSES.contains(&class));
        }
    }
}

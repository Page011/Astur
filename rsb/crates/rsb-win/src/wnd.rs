//! Window classes, window creation and the two window procedures.
//!
//! Both procedures only record: they push `Raw` events and update the small
//! tables in `Shared`. Nothing here calls the handler or waits on anything.

use std::ffi::c_void;
use std::sync::Once;
use std::time::Instant;

use rsb_core::hit::{Button, PointerEvent, PointerKind};
use rsb_core::style::Cursor;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, GWLP_USERDATA, GetWindowLongPtrW, HTCLIENT, IDC_ARROW, IDC_HAND,
    LoadCursorW, MA_NOACTIVATE, RegisterClassExW, RegisterWindowMessageW, SetCursor, SetWindowLongPtrW,
    WINDOW_EX_STYLE, WM_APP, WM_CAPTURECHANGED, WM_CLOSE, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ENDSESSION,
    WM_ERASEBKGND, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_NCCREATE, WM_NCDESTROY, WM_POWERBROADCAST, WM_QUERYENDSESSION, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SETCURSOR, WM_SETTINGCHANGE, WM_TIMECHANGE, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::appbar::{self, ABN_FULLSCREENAPP, ABN_POSCHANGED, CALLBACK_MSG};
use crate::shared::{Raw, Shared};
use crate::{WinError, sys};

/// Posted by `UiWaker::wake`.
pub(crate) const WM_WAKE: u32 = WM_APP + 0x10;
/// Posted when a frame deadline is already due, instead of a 10 ms timer.
pub(crate) const WM_FRAME_NOW: u32 = WM_APP + 0x11;

pub(crate) const TIMER_FRAME: usize = 1;
const TIMER_DISPLAY: usize = 2;
/// `WM_DISPLAYCHANGE` bursts (a resolution change sends several) collapse into
/// one `MonitorsChanged` this long after the last.
pub(crate) const DISPLAY_DEBOUNCE_MS: u32 = 250;

/// Windows timers never go below this.
const USER_TIMER_MINIMUM: u32 = 10;

/// Bit set in the low word of an injected mouse message's `wParam` (a flag no
/// real message uses). Injected messages must not arm `TrackMouseEvent` (the
/// real pointer is elsewhere, so Windows would answer with an immediate
/// leave) or capture the mouse (that would take it from whatever the owner is
/// really using).
pub(crate) const INJECTED: usize = 0x8000;

const PBT_APMRESUMEAUTOMATIC: usize = 0x12;
const SPI_NONE: usize = 0;

static CLASSES: Once = Once::new();
static CLASS_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

const BAR_CLASS: PCWSTR = w!("RsbBarWindow");
const HIDDEN_CLASS: PCWSTR = w!("RsbHiddenWindow");

fn instance() -> windows::Win32::Foundation::HINSTANCE {
    // SAFETY: no arguments that can dangle; null asks for this module.
    unsafe { GetModuleHandleW(None) }.map(Into::into).unwrap_or_default()
}

pub(crate) fn register_classes() -> Result<(), WinError> {
    CLASSES.call_once(|| {
        let mut err = None;
        for (name, proc_) in [
            (BAR_CLASS, bar_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT),
            (HIDDEN_CLASS, hidden_proc),
        ] {
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(proc_),
                hInstance: instance(),
                lpszClassName: name,
                ..Default::default()
            };
            // SAFETY: the class name and procedure are 'static.
            if unsafe { RegisterClassExW(&wc) } == 0 {
                err = Some(sys::last_error("RegisterClassExW").to_string());
            }
        }
        *CLASS_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = err;
    });
    match CLASS_ERROR.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        None => Ok(()),
        Some(message) => Err(WinError::Call { call: "RegisterClassExW", message }),
    }
}

pub(crate) fn taskbar_created_message() -> u32 {
    // SAFETY: the string is a 'static wide literal.
    unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) }
}

/// The never-shown top-level window that receives wakes and the broadcasts.
/// Top-level, not message-only: message-only windows get no broadcasts.
pub(crate) fn create_hidden(shared: *const Shared) -> Result<HWND, WinError> {
    // SAFETY: the class is registered; `shared` outlives the window (Ui
    // destroys every window before it drops the Rc).
    unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            HIDDEN_CLASS,
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance()),
            Some(shared as *const c_void),
        )
    }
    .map_err(|e| sys::call_error("CreateWindowExW", &e))
}

/// A layered, topmost, never-activated tool window (no taskbar button),
/// created hidden: it is shown by its first present.
pub(crate) fn create_bar(shared: *const Shared, x: i32, y: i32, w: i32, h: i32) -> Result<HWND, WinError> {
    let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST;
    // SAFETY: as for `create_hidden`.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(ex.0),
            BAR_CLASS,
            w!("RSB"),
            WS_POPUP,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(instance()),
            Some(shared as *const c_void),
        )
    }
    .map_err(|e| sys::call_error("CreateWindowExW", &e))
}

/// # Safety
/// `hwnd` must be a window made by this module whose `Shared` is still alive
/// (the returned lifetime is only as long as the message being handled).
unsafe fn shared_of<'a>(hwnd: HWND) -> Option<&'a Shared> {
    // SAFETY: reading our own window's user data.
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Shared;
    // SAFETY: the pointer was stored by `attach` from a live Rc and is cleared
    // at WM_NCDESTROY, before Ui drops it.
    unsafe { p.as_ref() }
}

fn attach(hwnd: HWND, lp: LPARAM) {
    // SAFETY: for WM_NCCREATE, lParam points at the CREATESTRUCTW.
    let cs = unsafe { &*(lp.0 as *const CREATESTRUCTW) };
    // SAFETY: storing a value in our own window's user data.
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
    }
}

fn detach(hwnd: HWND) {
    // SAFETY: clearing our own window's user data.
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
    }
}

fn default_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: forwarding the arguments Windows gave us.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

unsafe extern "system" fn hidden_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        attach(hwnd, lp);
    }
    // SAFETY: `hwnd` is one of ours.
    if let Some(sh) = unsafe { shared_of(hwnd) }
        && let Some(r) = hidden_message(sh, hwnd, msg, wp, lp)
    {
        return r;
    }
    if msg == WM_NCDESTROY {
        detach(hwnd);
    }
    default_proc(hwnd, msg, wp, lp)
}

fn hidden_message(sh: &Shared, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    let me = sys::raw(hwnd);
    match msg {
        WM_WAKE => {
            sh.wake_pending.set(true);
            Some(LRESULT(0))
        }
        WM_FRAME_NOW => {
            frame_check(sh, me);
            Some(LRESULT(0))
        }
        WM_TIMER if wp.0 == TIMER_FRAME => {
            sys::kill_timer(me, TIMER_FRAME);
            frame_check(sh, me);
            Some(LRESULT(0))
        }
        WM_TIMER if wp.0 == TIMER_DISPLAY => {
            sys::kill_timer(me, TIMER_DISPLAY);
            sh.push(Raw::Display);
            Some(LRESULT(0))
        }
        WM_DISPLAYCHANGE => {
            // Setting a timer that exists restarts it: a burst becomes one.
            sys::set_timer(me, TIMER_DISPLAY, DISPLAY_DEBOUNCE_MS);
            Some(LRESULT(0))
        }
        WM_SETTINGCHANGE => {
            // Named settings come with wParam 0 and a string in lParam; for the
            // numbered ones lParam is not necessarily a pointer, so it is only
            // read in the named case.
            if wp.0 == SPI_NONE && lp.0 != 0 && is_immersive_color_set(lp.0 as *const u16) {
                let dark = sh.theme_override.take().unwrap_or_else(sys::read_apps_dark);
                sh.push(Raw::Theme { dark });
            }
            Some(LRESULT(0))
        }
        WM_POWERBROADCAST => {
            if wp.0 == PBT_APMRESUMEAUTOMATIC {
                sh.push(Raw::Clock);
            }
            Some(LRESULT(1))
        }
        WM_TIMECHANGE => {
            sh.push(Raw::Clock);
            Some(LRESULT(0))
        }
        WM_QUERYENDSESSION => Some(LRESULT(1)),
        WM_ENDSESSION => {
            if wp.0 != 0 {
                // The session is ending: give the screen space back inside the
                // message, because the process may be gone before the loop
                // runs again.
                let all: Vec<isize> = sh.registered.borrow().iter().copied().collect();
                for h in all {
                    appbar::remove(h);
                }
                sh.registered.borrow_mut().clear();
                sh.push(Raw::EndSession);
            }
            Some(LRESULT(0))
        }
        WM_CLOSE => {
            sh.push(Raw::Close);
            Some(LRESULT(0))
        }
        m if m != 0 && m == sh.taskbar_created.get() => {
            sh.push(Raw::TaskbarCreated);
            Some(LRESULT(0))
        }
        _ => None,
    }
}

/// Compares the NUL-terminated string at `p` with `ImmersiveColorSet`, reading
/// no further than the first difference.
fn is_immersive_color_set(p: *const u16) -> bool {
    for (i, c) in "ImmersiveColorSet".encode_utf16().chain(std::iter::once(0)).enumerate() {
        // SAFETY: `p` is the string WM_SETTINGCHANGE carries; the loop stops at
        // the first character that differs, which is at the latest the
        // string's terminating NUL.
        if unsafe { *p.add(i) } != c {
            return false;
        }
    }
    true
}

/// Hands out `FrameDue` when the deadline has passed, re-arms when a timer
/// fired a little early, ignores a cancelled one.
fn frame_check(sh: &Shared, hidden: isize) {
    let Some(at) = sh.deadline.get() else { return };
    let now = Instant::now();
    if now >= at {
        sh.deadline.set(None);
        sh.push(Raw::FrameDue);
    } else {
        arm_frame_timer(hidden, at);
    }
}

/// One-shot timer for `at`; a deadline that is already due is posted instead.
pub(crate) fn arm_frame_timer(hidden: isize, at: Instant) {
    let left = at.saturating_duration_since(Instant::now());
    if left.is_zero() {
        sys::kill_timer(hidden, TIMER_FRAME);
        sys::post(hidden, WM_FRAME_NOW, 0, 0);
        return;
    }
    let ms = (left.as_micros().div_ceil(1000)).min(u32::MAX as u128 / 2) as u32;
    sys::set_timer(hidden, TIMER_FRAME, ms.max(USER_TIMER_MINIMUM));
}

unsafe extern "system" fn bar_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        attach(hwnd, lp);
    }
    // SAFETY: `hwnd` is one of ours.
    if let Some(sh) = unsafe { shared_of(hwnd) }
        && let Some(r) = bar_message(sh, hwnd, msg, wp, lp)
    {
        return r;
    }
    if msg == WM_NCDESTROY {
        detach(hwnd);
    }
    default_proc(hwnd, msg, wp, lp)
}

fn xy(lp: LPARAM) -> (f32, f32) {
    // Client coordinates are signed 16-bit values in the low and high words.
    (((lp.0 & 0xFFFF) as u16 as i16) as f32, (((lp.0 >> 16) & 0xFFFF) as u16 as i16) as f32)
}

fn bar_message(sh: &Shared, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    let key = sys::raw(hwnd);
    match msg {
        WM_MOUSEACTIVATE => Some(LRESULT(MA_NOACTIVATE as isize)),
        // The renderer paints every pixel; nothing to erase.
        WM_ERASEBKGND => Some(LRESULT(1)),
        WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
            let cursor = sh.wins.borrow().get(&key).map(|w| w.cursor)?;
            apply_cursor(cursor);
            Some(LRESULT(1))
        }
        WM_MOUSEMOVE => {
            let (x, y) = xy(lp);
            let injected = wp.0 & INJECTED != 0;
            let (bar, arm) = {
                let mut wins = sh.wins.borrow_mut();
                let w = wins.get_mut(&key)?;
                w.last = (x, y);
                let mut arm = false;
                if !injected {
                    w.hovered = true;
                    if !w.tracking {
                        w.tracking = true;
                        arm = true;
                    }
                }
                (w.bar, arm)
            };
            if arm
                && !track_leave(hwnd)
                && let Some(w) = sh.wins.borrow_mut().get_mut(&key)
            {
                w.tracking = false;
            }
            sh.push(Raw::Pointer { bar, event: PointerEvent { kind: PointerKind::Move, x, y } });
            Some(LRESULT(0))
        }
        WM_MOUSELEAVE => {
            let (bar, x, y) = {
                let mut wins = sh.wins.borrow_mut();
                let w = wins.get_mut(&key)?;
                w.tracking = false;
                w.hovered = false;
                (w.bar, w.last.0, w.last.1)
            };
            sh.push(Raw::Pointer { bar, event: PointerEvent { kind: PointerKind::Leave, x, y } });
            Some(LRESULT(0))
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            button(sh, hwnd, msg, wp, lp, true);
            Some(LRESULT(0))
        }
        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
            button(sh, hwnd, msg, wp, lp, false);
            Some(LRESULT(0))
        }
        WM_CAPTURECHANGED => {
            if let Some(w) = sh.wins.borrow_mut().get_mut(&key) {
                w.captured = 0;
            }
            Some(LRESULT(0))
        }
        WM_MOUSEWHEEL => {
            // Wheel coordinates are screen coordinates.
            let mut pt =
                POINT { x: (lp.0 & 0xFFFF) as u16 as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32 };
            // SAFETY: `pt` is a live POINT and `hwnd` is our window.
            unsafe {
                let _ = ScreenToClient(hwnd, &mut pt);
            }
            let notches = (((wp.0 >> 16) & 0xFFFF) as u16 as i16) as f32 / 120.0;
            let bar = {
                let mut wins = sh.wins.borrow_mut();
                let w = wins.get_mut(&key)?;
                w.last = (pt.x as f32, pt.y as f32);
                w.bar
            };
            sh.push(Raw::Pointer {
                bar,
                event: PointerEvent { kind: PointerKind::Wheel(notches), x: pt.x as f32, y: pt.y as f32 },
            });
            Some(LRESULT(0))
        }
        CALLBACK_MSG => {
            let bar = sh.bar_of(key)?;
            match wp.0 {
                ABN_POSCHANGED => sh.push(Raw::PosChanged { bar }),
                ABN_FULLSCREENAPP => sh.push(Raw::Fullscreen { bar, active: lp.0 != 0 }),
                _ => {}
            }
            Some(LRESULT(0))
        }
        WM_DPICHANGED => {
            // lParam would carry Windows' suggested rectangle; the window layer
            // places the bar from its own spec instead (and an injected
            // message has no rectangle), so it is not read.
            let bar = sh.bar_of(key)?;
            sh.push(Raw::Dpi { bar, dpi: (wp.0 & 0xFFFF) as u32 });
            Some(LRESULT(0))
        }
        WM_CLOSE => {
            sh.push(Raw::Close);
            Some(LRESULT(0))
        }
        _ => None,
    }
}

fn button(sh: &Shared, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, down: bool) {
    let key = sys::raw(hwnd);
    let injected = wp.0 & INJECTED != 0;
    let which = match msg {
        WM_LBUTTONDOWN | WM_LBUTTONUP => Button::Left,
        WM_RBUTTONDOWN | WM_RBUTTONUP => Button::Right,
        _ => Button::Middle,
    };
    let (x, y) = xy(lp);
    let (bar, capture, release) = {
        let mut wins = sh.wins.borrow_mut();
        let Some(w) = wins.get_mut(&key) else { return };
        w.last = (x, y);
        let mut capture = false;
        let mut release = false;
        if !injected {
            if down {
                capture = w.captured == 0;
                w.captured += 1;
            } else if w.captured > 0 {
                w.captured -= 1;
                release = w.captured == 0;
            }
        }
        (w.bar, capture, release)
    };
    if capture {
        // Keeps the matching button-up coming to us if the pointer leaves.
        // SAFETY: `hwnd` is our window.
        unsafe {
            SetCapture(hwnd);
        }
    }
    if release {
        // SAFETY: no arguments.
        unsafe {
            let _ = ReleaseCapture();
        }
    }
    let kind = if down { PointerKind::Down(which) } else { PointerKind::Up(which) };
    sh.push(Raw::Pointer { bar, event: PointerEvent { kind, x, y } });
}

fn track_leave(hwnd: HWND) -> bool {
    let mut t = TRACKMOUSEEVENT {
        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    // SAFETY: `t` is a fully initialised TRACKMOUSEEVENT.
    unsafe { TrackMouseEvent(&mut t) }.is_ok()
}

/// Sets the thread's cursor to the arrow or the hand (shared system cursors:
/// nothing to free).
pub(crate) fn apply_cursor(c: Cursor) {
    let id = match c {
        Cursor::Default => IDC_ARROW,
        Cursor::Pointer => IDC_HAND,
    };
    // SAFETY: loads a shared system cursor; SetCursor takes the handle as is.
    unsafe {
        if let Ok(h) = LoadCursorW(None, id) {
            SetCursor(Some(h));
        }
    }
}

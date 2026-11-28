//! Window classes, window creation and the two window procedures.
//!
//! Both procedures only record: they push `Raw` events and update the small
//! tables in `Shared`. Nothing here calls the handler or waits on anything.

use std::ffi::c_void;
use std::sync::Once;

use rsb_core::geom::RectPx;
use rsb_core::hit::{Button, PointerEvent, PointerKind};
use rsb_core::key::{Key, KeyEvent, Modifiers, TextInput};
use rsb_core::style::Cursor;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BLACK_BRUSH, BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, GetStockObject, HBRUSH, PAINTSTRUCT,
    ScreenToClient,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::Ime::{IMECHARPOSITION, IMR_QUERYCHARPOSITION};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_F4, VK_PROCESSKEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, GWLP_USERDATA, GetClientRect, GetWindowLongPtrW,
    GetWindowThreadProcessId, HTCLIENT, IDC_ARROW, IDC_HAND, IDC_IBEAM, LoadCursorW, MA_NOACTIVATE, RegisterClassExW,
    RegisterWindowMessageW, SetCursor, SetWindowLongPtrW, WA_INACTIVE, WINDOW_EX_STYLE, WM_ACTIVATE, WM_APP,
    WM_CAPTURECHANGED, WM_CHAR, WM_CLOSE, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ENDSESSION, WM_ERASEBKGND,
    WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION, WM_IME_REQUEST, WM_IME_SETCONTEXT, WM_IME_STARTCOMPOSITION, WM_KEYDOWN,
    WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEHWHEEL,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WM_POWERBROADCAST, WM_QUERYENDSESSION,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR, WM_SETTINGCHANGE, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMECHANGE, WM_TIMER,
    WM_WTSSESSION_CHANGE, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::appbar::{self, ABN_FULLSCREENAPP, ABN_POSCHANGED, CALLBACK_MSG};
use crate::keys;
use crate::shared::{Raw, Shared, Target};
use crate::{OutsidePress, WinError, sys};

/// Posted by `UiWaker::wake`.
pub(crate) const WM_WAKE: u32 = WM_APP + 0x10;
/// Posted by the clipboard worker when it finished a request.
pub(crate) const WM_CLIP_READY: u32 = WM_APP + 0x13;

/// Runs out when the oldest clipboard request must be answered `None`.
pub(crate) const TIMER_CLIP: usize = 3;
const TIMER_DISPLAY: usize = 2;
/// `WM_DISPLAYCHANGE` bursts (a resolution change sends several) collapse into
/// one `MonitorsChanged` this long after the last.
pub(crate) const DISPLAY_DEBOUNCE_MS: u32 = 250;

/// Bit set in the low word of an injected mouse message's `wParam` (a flag no
/// real message uses). Injected messages must not arm `TrackMouseEvent` (the
/// real pointer is elsewhere, so Windows would answer with an immediate
/// leave) or capture the mouse (that would take it from whatever the user is
/// really using).
pub(crate) const INJECTED: usize = 0x8000;

const PBT_APMRESUMEAUTOMATIC: usize = 0x12;
const SPI_NONE: usize = 0;
const WTS_SESSION_LOCK: usize = 7;
const WTS_SESSION_UNLOCK: usize = 8;
const VK_PROCESSKEY_CODE: usize = VK_PROCESSKEY.0 as usize;

static CLASSES: Once = Once::new();
static CLASS_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

const BAR_CLASS: PCWSTR = w!("RsbBarWindow");
const HIDDEN_CLASS: PCWSTR = w!("RsbHiddenWindow");
const POPUP_CLASS: PCWSTR = w!("RsbPopupWindow");
const DIM_CLASS: PCWSTR = w!("RsbDimWindow");
const BACKDROP_CLASS: PCWSTR = w!("RsbBackdropWindow");

fn instance() -> windows::Win32::Foundation::HINSTANCE {
    // SAFETY: no arguments that can dangle; null asks for this module.
    unsafe { GetModuleHandleW(None) }.map(Into::into).unwrap_or_default()
}

pub(crate) fn register_classes() -> Result<(), WinError> {
    CLASSES.call_once(|| {
        let mut err = None;
        // The backdrop window's client area is black: with the frame
        // extended over it, the system draws its effect in place of black.
        // SAFETY: a stock object needs no cleanup.
        let black = HBRUSH(unsafe { GetStockObject(BLACK_BRUSH) }.0);
        for (name, proc_, brush) in [
            (BAR_CLASS, bar_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT, HBRUSH::default()),
            (HIDDEN_CLASS, hidden_proc, HBRUSH::default()),
            (POPUP_CLASS, popup_proc, HBRUSH::default()),
            (DIM_CLASS, dim_proc, HBRUSH::default()),
            (BACKDROP_CLASS, backdrop_proc, black),
        ] {
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(proc_),
                hInstance: instance(),
                lpszClassName: name,
                hbrBackground: brush,
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
        WM_CLIP_READY => {
            sh.push(Raw::ClipboardReady);
            Some(LRESULT(0))
        }
        WM_TIMER if wp.0 == TIMER_CLIP => {
            sys::kill_timer(me, TIMER_CLIP);
            sh.push(Raw::ClipboardReady);
            Some(LRESULT(0))
        }
        crate::hook::WM_OUTSIDE_PRESS => {
            // The coordinates travel as two 32-bit values: the hook
            // procedure posts them without allocating anything.
            sh.push(Raw::OutsidePress(OutsidePress { x: wp.0 as u32 as i32, y: lp.0 as u32 as i32 }));
            Some(LRESULT(0))
        }
        WM_WTSSESSION_CHANGE => {
            match wp.0 {
                WTS_SESSION_LOCK => sh.push(Raw::SessionLock { locked: true }),
                WTS_SESSION_UNLOCK => sh.push(Raw::SessionLock { locked: false }),
                _ => {}
            }
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
        WM_MOUSEMOVE | WM_MOUSELEAVE | WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_LBUTTONUP
        | WM_RBUTTONUP | WM_MBUTTONUP | WM_CAPTURECHANGED | WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            mouse_message(sh, hwnd, msg, wp, lp)
        }
        CALLBACK_MSG => {
            let bar = sh.bar_of(key)?;
            match wp.0 {
                ABN_POSCHANGED => sh.push(Raw::PosChanged { bar }),
                // Our own overlay covers the monitor too: it is not a
                // fullscreen app, and must not hide or lower its own bar.
                ABN_FULLSCREENAPP => {
                    let active = lp.0 != 0;
                    if !(active && crate::fullscreen::foreground_is_ours()) {
                        sh.push(Raw::Fullscreen { bar, active });
                    }
                }
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

/// What a pointer event on a window of this kind becomes.
fn pointer_raw(target: Target, event: PointerEvent) -> Raw {
    match target {
        Target::Bar(bar) => Raw::Pointer { bar, event },
        Target::Popup(popup) => Raw::PopupPointer { popup, event },
    }
}

/// The mouse messages, for bars, popups and dim windows alike.
fn mouse_message(sh: &Shared, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    let key = sys::raw(hwnd);
    match msg {
        WM_MOUSEMOVE => {
            let (x, y) = xy(lp);
            let injected = wp.0 & INJECTED != 0;
            let (target, arm) = {
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
                (w.target, arm)
            };
            if arm
                && !track_leave(hwnd)
                && let Some(w) = sh.wins.borrow_mut().get_mut(&key)
            {
                w.tracking = false;
            }
            sh.push(pointer_raw(target, PointerEvent { kind: PointerKind::Move, x, y }));
            Some(LRESULT(0))
        }
        WM_MOUSELEAVE => {
            let (target, x, y) = {
                let mut wins = sh.wins.borrow_mut();
                let w = wins.get_mut(&key)?;
                w.tracking = false;
                w.hovered = false;
                (w.target, w.last.0, w.last.1)
            };
            sh.push(pointer_raw(target, PointerEvent { kind: PointerKind::Leave, x, y }));
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
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            // Wheel coordinates are screen coordinates.
            let mut pt =
                POINT { x: (lp.0 & 0xFFFF) as u16 as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32 };
            // SAFETY: `pt` is a live POINT and `hwnd` is our window.
            unsafe {
                let _ = ScreenToClient(hwnd, &mut pt);
            }
            let notches = (((wp.0 >> 16) & 0xFFFF) as u16 as i16) as f32 / 120.0;
            // Shift held (MK_SHIFT in the low word) turns the vertical wheel sideways, down = right.
            let kind = match msg {
                WM_MOUSEHWHEEL => PointerKind::HWheel(notches),
                _ if wp.0 & 0x4 != 0 => PointerKind::HWheel(-notches),
                _ => PointerKind::Wheel(notches),
            };
            let target = {
                let mut wins = sh.wins.borrow_mut();
                let w = wins.get_mut(&key)?;
                w.last = (pt.x as f32, pt.y as f32);
                w.target
            };
            sh.push(pointer_raw(target, PointerEvent { kind, x: pt.x as f32, y: pt.y as f32 }));
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
    let (target, capture, release) = {
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
        (w.target, capture, release)
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
    sh.push(pointer_raw(target, PointerEvent { kind, x, y }));
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

fn create_with(
    class: PCWSTR,
    ex: windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE,
    rect: &RectPx,
    shared: *const Shared,
) -> Result<HWND, WinError> {
    // SAFETY: as for `create_hidden`.
    unsafe {
        CreateWindowExW(
            ex,
            class,
            w!("RSB"),
            WS_POPUP,
            rect.x0,
            rect.y0,
            rect.width(),
            rect.height(),
            None,
            None,
            Some(instance()),
            Some(shared as *const c_void),
        )
    }
    .map_err(|e| sys::call_error("CreateWindowExW", &e))
}

/// A popup, menu, tooltip or overlay content window; `ex` carries the
/// styles that differ by role. Created hidden.
pub(crate) fn create_popup(
    shared: *const Shared,
    ex: windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE,
    rect: &RectPx,
) -> Result<HWND, WinError> {
    create_with(POPUP_CLASS, ex, rect, shared)
}

/// The dim window of an overlay: layered with a constant alpha, so a fade
/// needs no repaint. Created hidden.
pub(crate) fn create_dim(shared: *const Shared, rect: &RectPx) -> Result<HWND, WinError> {
    let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST;
    create_with(DIM_CLASS, WINDOW_EX_STYLE(ex.0), rect, shared)
}

/// The window the system draws its backdrop effect into: not layered (a
/// layered window has no redirection surface for the effect), never
/// activated. Created hidden.
pub(crate) fn create_backdrop(shared: *const Shared, rect: &RectPx) -> Result<HWND, WinError> {
    let ex = WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST;
    create_with(BACKDROP_CLASS, WINDOW_EX_STYLE(ex.0), rect, shared)
}

unsafe extern "system" fn popup_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        attach(hwnd, lp);
    }
    // SAFETY: `hwnd` is one of ours.
    if let Some(sh) = unsafe { shared_of(hwnd) }
        && let Some(r) = popup_message(sh, hwnd, msg, wp, lp)
    {
        return r;
    }
    if msg == WM_NCDESTROY {
        detach(hwnd);
    }
    default_proc(hwnd, msg, wp, lp)
}

unsafe extern "system" fn dim_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        attach(hwnd, lp);
    }
    // SAFETY: `hwnd` is one of ours.
    if let Some(sh) = unsafe { shared_of(hwnd) }
        && let Some(r) = dim_message(sh, hwnd, msg, wp, lp)
    {
        return r;
    }
    if msg == WM_NCDESTROY {
        detach(hwnd);
    }
    default_proc(hwnd, msg, wp, lp)
}

unsafe extern "system" fn backdrop_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_CLOSE => LRESULT(0),
        _ => default_proc(hwnd, msg, wp, lp),
    }
}

fn popup_of(sh: &Shared, key: isize) -> Option<crate::PopupId> {
    match sh.wins.borrow().get(&key)?.target {
        Target::Popup(p) => Some(p),
        Target::Bar(_) => None,
    }
}

fn set_window_cursor(sh: &Shared, key: isize) -> Option<LRESULT> {
    let cursor = sh.wins.borrow().get(&key).map(|w| w.cursor)?;
    apply_cursor(cursor);
    Some(LRESULT(1))
}

fn popup_message(sh: &Shared, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    let key = sys::raw(hwnd);
    match msg {
        // Only `Ui::activate_popup` gives a popup focus: a click does not.
        WM_MOUSEACTIVATE => Some(LRESULT(MA_NOACTIVATE as isize)),
        WM_ERASEBKGND => Some(LRESULT(1)),
        WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => set_window_cursor(sh, key),
        WM_MOUSEMOVE | WM_MOUSELEAVE | WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_LBUTTONUP
        | WM_RBUTTONUP | WM_MBUTTONUP | WM_CAPTURECHANGED | WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            mouse_message(sh, hwnd, msg, wp, lp)
        }
        WM_ACTIVATE => {
            let popup = popup_of(sh, key)?;
            if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE {
                // Focus moving to another of our own windows is the popup
                // manager's business; only a window of another process
                // (or none we can see) ends a focus-holding popup.
                let other = HWND(lp.0 as *mut c_void);
                let mut pid = 0u32;
                // SAFETY: a live out pointer; a null or stale handle only fails the call.
                let known = !other.0.is_null() && unsafe { GetWindowThreadProcessId(other, Some(&mut pid)) } != 0;
                // SAFETY: no arguments.
                let own = unsafe { GetCurrentProcessId() };
                if !crate::fullscreen::is_own_process(known.then_some(pid), own) {
                    sh.push(Raw::PopupFocus { popup, focused: false });
                }
            } else {
                sh.push(Raw::PopupFocus { popup, focused: true });
            }
            // The default handler gives the window the keyboard focus.
            None
        }
        WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP => {
            let popup = popup_of(sh, key)?;
            // The IME used this key: an Escape that cancels a composition
            // must not close the popup.
            if wp.0 == VK_PROCESSKEY_CODE {
                return Some(LRESULT(0));
            }
            let vk = (wp.0 & 0xFFFF) as u16;
            let system = msg == WM_SYSKEYDOWN || msg == WM_SYSKEYUP;
            if system && vk == VK_F4.0 {
                // Alt+F4 closes the popup the way Escape does. Windows would turn it into a close
                // request only for a window with a system menu, which a popup has not.
                if msg == WM_SYSKEYDOWN && !keys::is_repeat(lp.0) {
                    let plain = Modifiers::default();
                    let escape = KeyEvent { key: Key::Escape, mods: plain, down: true, repeat: false };
                    sh.push(Raw::Key { popup, event: escape });
                }
                return Some(LRESULT(0));
            }
            let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
            let mods = sh.inject_mods.get().unwrap_or_else(keys::real_modifiers);
            let repeat = down && keys::is_repeat(lp.0);
            sh.push(Raw::Key { popup, event: KeyEvent { key: keys::map_vk(vk), mods, down, repeat } });
            Some(LRESULT(0))
        }
        WM_CHAR => {
            let popup = popup_of(sh, key)?;
            let joined = sh.wins.borrow_mut().get_mut(&key)?.joiner.push((wp.0 & 0xFFFF) as u16);
            if let Some(c) = joined.and_then(keys::text_char) {
                sh.push(Raw::Text { popup, input: TextInput::Chars(c.to_string()) });
            }
            Some(LRESULT(0))
        }
        // An IME asking where the character being composed is (the candidate list goes under it).
        WM_IME_REQUEST if wp.0 as u32 == IMR_QUERYCHARPOSITION => {
            popup_of(sh, key)?;
            let (l, t, _, b) = sh.wins.borrow().get(&key)?.ime_caret?;
            if lp.0 == 0 {
                return None;
            }
            // SAFETY: for IMR_QUERYCHARPOSITION lParam points at an IMECHARPOSITION whose dwSize
            // the IME set; nothing is written when it is smaller than the structure.
            unsafe {
                let info = &mut *(lp.0 as *mut IMECHARPOSITION);
                if (info.dwSize as usize) < std::mem::size_of::<IMECHARPOSITION>() {
                    return None;
                }
                info.pt = POINT { x: l, y: t };
                info.cLineHeight = (b - t).max(1) as u32;
                info.rcDocument = RECT { left: l, top: t, right: l, bottom: b };
            }
            Some(LRESULT(1))
        }
        // We draw the composition ourselves; the candidate list stays the
        // IME's.
        WM_IME_SETCONTEXT => {
            let flags = LPARAM(keys::without_composition_window(lp.0));
            Some(default_proc(hwnd, msg, wp, flags))
        }
        // Not passed on: the default handler would turn the result into
        // WM_CHAR and draw its own composition window.
        WM_IME_STARTCOMPOSITION => {
            let popup = popup_of(sh, key)?;
            sh.push(Raw::Text { popup, input: TextInput::CompositionStart });
            Some(LRESULT(0))
        }
        WM_IME_ENDCOMPOSITION => {
            let popup = popup_of(sh, key)?;
            sh.push(Raw::Text { popup, input: TextInput::CompositionEnd });
            Some(LRESULT(0))
        }
        WM_IME_COMPOSITION => {
            let popup = popup_of(sh, key)?;
            let flags = lp.0 as u32;
            let injected = sh.inject_comp.borrow_mut().take();
            let mut comp = injected.unwrap_or_else(|| keys::read_composition(hwnd, flags));
            // No string flag at all: the composition was cancelled.
            if flags & (keys::GCS_COMPSTR_BIT | keys::GCS_RESULTSTR_BIT) == 0 && comp.update.is_none() {
                comp.update = Some((String::new(), 0));
            }
            if let Some(text) = comp.result.filter(|t| !t.is_empty()) {
                sh.push(Raw::Text { popup, input: TextInput::CompositionCommit(text) });
            }
            if let Some((text, cursor)) = comp.update {
                sh.push(Raw::Text { popup, input: TextInput::CompositionUpdate { text, cursor } });
            }
            Some(LRESULT(0))
        }
        // The window stays as it is; the popup manager closes the popup.
        WM_DPICHANGED => {
            let popup = popup_of(sh, key)?;
            sh.push(Raw::PopupDpi { popup, dpi: (wp.0 & 0xFFFF) as u32 });
            Some(LRESULT(0))
        }
        // A request to close the window (the taskbar, another program) is an
        // Escape too: the manager closes the popup the way it closes one for
        // that key, and a popup that forbids Escape stays.
        WM_CLOSE => {
            let popup = popup_of(sh, key)?;
            let plain = Modifiers::default();
            sh.push(Raw::Key { popup, event: KeyEvent { key: Key::Escape, mods: plain, down: true, repeat: false } });
            Some(LRESULT(0))
        }
        _ => None,
    }
}

fn dim_message(sh: &Shared, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    let key = sys::raw(hwnd);
    match msg {
        WM_MOUSEACTIVATE => Some(LRESULT(MA_NOACTIVATE as isize)),
        WM_ERASEBKGND => Some(LRESULT(1)),
        WM_PAINT => {
            let rgb = {
                let mut wins = sh.wins.borrow_mut();
                let w = wins.get_mut(&key)?;
                w.paints += 1;
                w.dim_rgb
            };
            let mut ps = PAINTSTRUCT::default();
            // SAFETY: the window is ours; the paint struct, rectangle and
            // brush are live locals, and the brush is deleted after use.
            unsafe {
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let brush = CreateSolidBrush(COLORREF(rgb));
                FillRect(hdc, &rc, brush);
                let _ = DeleteObject(brush.into());
                let _ = EndPaint(hwnd, &ps);
            }
            Some(LRESULT(0))
        }
        WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
            apply_cursor(Cursor::Default);
            Some(LRESULT(1))
        }
        WM_MOUSEMOVE | WM_MOUSELEAVE | WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_LBUTTONUP
        | WM_RBUTTONUP | WM_MBUTTONUP | WM_CAPTURECHANGED | WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            mouse_message(sh, hwnd, msg, wp, lp)
        }
        WM_CLOSE => Some(LRESULT(0)),
        _ => None,
    }
}

/// Sets the thread's cursor to the arrow or the hand (shared system cursors:
/// nothing to free).
pub(crate) fn apply_cursor(c: Cursor) {
    let id = match c {
        Cursor::Default => IDC_ARROW,
        Cursor::Pointer => IDC_HAND,
        Cursor::Text => IDC_IBEAM,
    };
    // SAFETY: loads a shared system cursor; SetCursor takes the handle as is.
    unsafe {
        if let Ok(h) = LoadCursorW(None, id) {
            SetCursor(Some(h));
        }
    }
}

// hyprwin — Alt-drag move/resize for Windows (Hyprland-style mouse binds)
//
// Hold LEFT ALT, then:
//   Left-drag   -> move the window under the cursor
//   Right-drag  -> resize from the corner nearest the cursor; a red marker
//                  shows which corner is being dragged
//
// LEFT ALT is reserved as suprland's modifier: a low-level keyboard hook blocks
// it from every application so it never triggers app menus or Alt shortcuts.
// Alt+Tab is preserved by synthesizing an injected Alt+Tab for the system.
// RIGHT ALT is untouched, so use it for normal Alt behavior.
//
// Both hooks run on this process's message-loop thread, so all drag state lives
// behind a single Mutex with effectively zero contention.

// Uncomment to run without a console window (release builds):
// #![windows_subsystem = "windows"]

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Condvar, Mutex};

use windows::core::w;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::Foundation::{
    BOOL, COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SYSTEMTIME, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CombineRgn, CreateFontW, CreateRectRgn, CreateSolidBrush, DeleteObject, DrawTextW,
    EndPaint, EnumDisplayMonitors, FillRect, GetMonitorInfoW, GetStockObject, InvalidateRect,
    MonitorFromPoint, MonitorFromWindow, SelectObject, SetBkMode, SetTextColor, SetWindowRgn,
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_GUI_FONT, DT_CENTER,
    DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, HDC, HGDIOBJ,
    HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST, OUT_DEFAULT_PRECIS, PAINTSTRUCT, RGN_OR,
    TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_LBUTTON, VK_LMENU, VK_MENU, VK_RBUTTON,
    VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{                  
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetAncestor,
    GetDesktopWindow, GetMessageW, GetShellWindow, GetWindowRect, IsZoomed, RegisterClassW,
    SetLayeredWindowAttributes, SetWindowPos, SetWindowsHookExW, ShowWindow,
    SetCursorPos,
    TranslateMessage,
    UnhookWindowsHookEx, WindowFromPoint, GA_ROOT, HC_ACTION, HWND_TOPMOST, KBDLLHOOKSTRUCT,
    LLKHF_INJECTED, LWA_ALPHA, MSG, MSLLHOOKSTRUCT, SWP_NOACTIVATE, SWP_NOSENDCHANGING, SWP_NOSIZE,
    SWP_NOZORDER,
    SWP_SHOWWINDOW, SW_HIDE, SW_RESTORE, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN,
    WM_SYSKEYUP, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};

// --- tiling additions -----------------------------------------------------
use std::collections::{HashMap, VecDeque};
use core::ffi::c_void;
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CLOAKED,
    DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::Accessibility::SetWinEventHook;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_SHIFT;
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows,
    GetClassNameW, GetForegroundWindow, GetWindow, GetWindowLongW, GetWindowTextLengthW,
    GetClientRect, GetCursorPos, GetWindowLongPtrW, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, IsWindow, IsWindowVisible, PostMessageW, SetWindowLongPtrW, GWLP_USERDATA,
    SetForegroundWindow, SetTimer, SetWindowLongW, SystemParametersInfoW, EVENT_OBJECT_DESTROY,
    EVENT_OBJECT_HIDE, EVENT_OBJECT_SHOW, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND,
    EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MOVESIZEEND, GWL_EXSTYLE, GWL_STYLE, GW_OWNER,
    SPI_SETFOREGROUNDLOCKTIMEOUT,
    SW_SHOW, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
    WM_CLOSE, WM_DISPLAYCHANGE, WM_PAINT, WM_TIMER, WM_USER, WS_CHILD,
};

// --- tunables -------------------------------------------------------------
const MIN_W: i32 = 120;
const MIN_H: i32 = 80;
// When grabbing a maximized window, shrink it to this fraction of the monitor
// work area (in each dimension) and center it on the cursor.
const RESTORE_NUM: i32 = 1;
const RESTORE_DEN: i32 = 2;
// Red L-shaped corner bracket shown while resizing: total arm length and the
// thickness of each arm (px).
const MARK_LEN: i32 = 28;
const MARK_THICK: i32 = 4;
// Top corners sit on the very top edge; lift the bracket up slightly so it reads
// as hugging the corner instead of sitting inside the title bar.
const MARK_TOP_LIFT: i32 = 8;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    None,
    Move,
    Resize,
}

struct Drag {
    mode: Mode,
    hwnd: isize,
    // cursor position when the drag began (screen coords)
    origin_x: i32,
    origin_y: i32,
    // window rect when the drag began
    win_x: i32,
    win_y: i32,
    win_w: i32,
    win_h: i32,
    // for resize: which corner is being dragged
    left: bool,
    top: bool,
}

impl Drag {
    const fn new() -> Self {
        Drag {
            mode: Mode::None,
            hwnd: 0,
            origin_x: 0,
            origin_y: 0,
            win_x: 0,
            win_y: 0,
            win_w: 0,
            win_h: 0,
            left: false,
            top: false,
        }
    }
}

static STATE: Mutex<Drag> = Mutex::new(Drag::new());

/// Latest desired window placement. A dedicated worker thread applies it so the
/// input hook never blocks on a slow app's SetWindowPos; intermediate updates
/// are coalesced and only the most recent target is applied.
#[derive(Clone, Copy)]
struct Target {
    hwnd: isize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    resize: bool,
}
static TARGET: Mutex<Option<Target>> = Mutex::new(None);
static TARGET_CV: Condvar = Condvar::new();

/// Queue the newest placement for the worker thread and wake it.
fn set_target(hwnd: isize, x: i32, y: i32, w: i32, h: i32, resize: bool) {
    {
        let mut t = TARGET.lock().unwrap();
        *t = Some(Target { hwnd, x, y, w, h, resize });
    }
    TARGET_CV.notify_one();
}

/// Worker loop: wait for a target, apply the newest, repeat. Runs SetWindowPos
/// off the input thread so a busy app can't stutter the cursor, and drops stale
/// intermediate positions so the window always converges to the latest one.
fn position_worker() {
    loop {
        let target = {
            let mut t = TARGET.lock().unwrap();
            loop {
                if let Some(target) = t.take() {
                    break target;
                }
                t = TARGET_CV.wait(t).unwrap();
            }
        };
        unsafe {
            let hwnd = hwnd_from(target.hwnd);
            if target.resize {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    target.x,
                    target.y,
                    target.w,
                    target.h,
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSENDCHANGING,
                );
            } else {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    target.x,
                    target.y,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSENDCHANGING,
                );
            }
        }
    }
}

// Set by the keyboard hook while physical Left Alt is held (Alt is blocked from
// apps and reserved as suprland's modifier).
static ALT_DOWN: AtomicBool = AtomicBool::new(false);
// True while we are feeding the system a synthetic Alt so Alt+Tab keeps working
// despite the physical Alt being blocked from everything.
static FAKE_ALT: AtomicBool = AtomicBool::new(false);
// Handle of the red corner-marker overlay window.
static MARKER_HWND: AtomicIsize = AtomicIsize::new(0);
// True only while a move/resize drag is in progress. Lets the global mouse hook
// skip the STATE mutex on every mouse-move when nothing is being dragged — and
// system-wide mouse-move is the single hottest path through this process.
static ANY_DRAG: AtomicBool = AtomicBool::new(false);

#[inline]
unsafe fn vk_down(vk: VIRTUAL_KEY) -> bool {
    (GetAsyncKeyState(vk.0 as i32) as u16 & 0x8000) != 0
}

#[inline]
unsafe fn left_alt_down() -> bool {
    // Trust the hook flag, but fall back to the live key state so a missed
    // key-down (e.g. Alt held before the hook saw it) can't wedge the modifier.
    ALT_DOWN.load(Ordering::Relaxed) || vk_down(VK_LMENU)
}

#[inline]
fn drag_active() -> bool {
    STATE.lock().unwrap().mode != Mode::None
}

/// WndProc for the marker window: nothing custom, the class brush paints it red.
unsafe extern "system" fn marker_wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_DISPLAYCHANGE {
        // Reposition/create bars for the new monitor layout, then retile.
        ensure_bars();
        push_cmd(Cmd::RefreshMonitors);
    } else if msg == WM_RELOAD {
        // Config changed: rebuild font + bars (must happen on this thread so it
        // can't race a paint).
        make_bar_font(
            BAR_HEIGHT.load(Ordering::Relaxed) as i32,
            BAR_FONT_SIZE.load(Ordering::Relaxed) as i32,
        );
        if BAR_HEIGHT.load(Ordering::Relaxed) > 0 {
            ensure_bars();
        } else {
            for b in BARS.lock().unwrap().iter() {
                let _ = ShowWindow(hwnd_from(b.hwnd), SW_HIDE);
            }
        }
    }
    DefWindowProcW(h, msg, w, l)
}

/// Inject one synthetic key event. Used to feed the system a real Alt (and Tab)
/// for the Alt+Tab passthrough while the physical Left Alt is blocked from apps.
unsafe fn inject_key(vk: VIRTUAL_KEY, up: bool) {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    SendInput(&[input], core::mem::size_of::<INPUT>() as i32);
}

/// Low-level keyboard hook. Left Alt is reserved as suprland's modifier: it is
/// blocked from every application so it never triggers menus or Alt shortcuts.
/// Alt+Tab is preserved by synthesizing an injected Alt+Tab for the system while
/// swallowing the physical keys.
unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        // Let our own synthetic events through — this is how Alt+Tab reaches the
        // system despite the physical Alt being blocked.
        let injected = (kb.flags.0 & LLKHF_INJECTED.0) != 0;
        if !injected {
            let msg = wparam.0 as u32;
            let down = matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN);
            let up = matches!(msg, WM_KEYUP | WM_SYSKEYUP);

            // Clear the auto-repeat guard on release.
            if up && (kb.vkCode as usize) < 256 {
                PRESSED.lock().unwrap()[kb.vkCode as usize] = false;
            }

            if kb.vkCode == VK_LMENU.0 as u32 {
                if down {
                    ALT_DOWN.store(true, Ordering::Relaxed);
                } else if up {
                    ALT_DOWN.store(false, Ordering::Relaxed);
                    // Release the synthetic Alt so the task switcher commits.
                    if FAKE_ALT.swap(false, Ordering::Relaxed) {
                        inject_key(VK_MENU, true);
                    }
                }
                return LRESULT(1); // never let apps see Left Alt
            }

            // Alt+Tab (and Alt+Shift+Tab): drive the switcher with injected keys
            // and swallow the physical Tab so it isn't counted twice.
            if kb.vkCode == VK_TAB.0 as u32 && ALT_DOWN.load(Ordering::Relaxed) {
                if down {
                    if !FAKE_ALT.swap(true, Ordering::Relaxed) {
                        inject_key(VK_MENU, false);
                    }
                    inject_key(VK_TAB, false);
                    inject_key(VK_TAB, true);
                }
                return LRESULT(1);
            }

            // Tiling hotkeys: Alt + key. Swallowed from apps (Alt is reserved).
            if down && ALT_DOWN.load(Ordering::Relaxed) {
                let shift = vk_down(VK_SHIFT);
                if let Some(cmd) = resolve_hotkey(kb.vkCode, shift) {
                    let vk = kb.vkCode as usize;
                    let mut p = PRESSED.lock().unwrap();
                    if vk < 256 && !p[vk] {
                        p[vk] = true;
                        drop(p);
                        push_cmd(cmd);
                    }
                    return LRESULT(1);
                }
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

/// Shape the marker window into an L-bracket hugging the given corner.
unsafe fn set_marker_shape(left: bool, top: bool) {
    let raw = MARKER_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        return;
    }
    let s = MARK_LEN;
    let t = MARK_THICK;
    // Horizontal arm hugs the top or bottom edge; vertical arm the left/right.
    let (hy0, hy1) = if top { (0, t) } else { (s - t, s) };
    let (vx0, vx1) = if left { (0, t) } else { (s - t, s) };
    let horiz = CreateRectRgn(0, hy0, s, hy1);
    let vert = CreateRectRgn(vx0, 0, vx1, s);
    let region = CreateRectRgn(0, 0, 0, 0);
    CombineRgn(region, horiz, vert, RGN_OR);
    let _ = DeleteObject(HGDIOBJ(horiz.0));
    let _ = DeleteObject(HGDIOBJ(vert.0));
    // The window takes ownership of `region`; the system frees it later.
    SetWindowRgn(hwnd_from(raw), region, BOOL(1));
}

/// Position the L-bracket so its corner sits exactly on the dragged corner.
unsafe fn show_marker(corner_x: i32, corner_y: i32, left: bool, top: bool) {
    let raw = MARKER_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        return;
    }
    let x = if left { corner_x } else { corner_x - MARK_LEN };
    let y = if top { corner_y - MARK_TOP_LIFT } else { corner_y - MARK_LEN };
    let _ = SetWindowPos(
        hwnd_from(raw),
        HWND_TOPMOST,
        x,
        y,
        MARK_LEN,
        MARK_LEN,
        SWP_NOACTIVATE | SWP_SHOWWINDOW,
    );
}

unsafe fn hide_marker() {
    let raw = MARKER_HWND.load(Ordering::Relaxed);
    if raw != 0 {
        let _ = ShowWindow(hwnd_from(raw), SW_HIDE);
    }
}

#[inline]
fn hwnd_from(raw: isize) -> HWND {
    HWND(raw as *mut core::ffi::c_void)
}

/// Resolve the top-level window under a screen point, ignoring desktop/shell.
unsafe fn root_window_at(pt: POINT) -> Option<HWND> {
    let h = WindowFromPoint(pt);
    if h.0.is_null() {
        return None;
    }
    let root = GetAncestor(h, GA_ROOT);
    if root.0.is_null() || root == GetDesktopWindow() || root == GetShellWindow() {
        return None;
    }
    Some(root)
}

/// Work area (excludes taskbar) of the monitor under a screen point.
unsafe fn work_area_at(pt: POINT) -> RECT {
    let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut mi = MONITORINFO {
        cbSize: core::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(mon, &mut mi).as_bool() {
        mi.rcWork
    } else {
        RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        }
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
    let pt = info.pt;
    let msg = wparam.0 as u32;
    let suppress = LRESULT(1);

    match msg {
        WM_LBUTTONDOWN if left_alt_down() && !drag_active() => {
            if let Some(hwnd) = root_window_at(pt) {
                let mut rect = RECT::default();
                if IsZoomed(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                    // Shrink to a small floating window centered on the cursor.
                    let work = work_area_at(pt);
                    let w = ((work.right - work.left) * RESTORE_NUM / RESTORE_DEN).max(MIN_W);
                    let h = ((work.bottom - work.top) * RESTORE_NUM / RESTORE_DEN).max(MIN_H);
                    let mut x = pt.x - w / 2;
                    let mut y = pt.y - h / 2;
                    x = x.clamp(work.left, (work.right - w).max(work.left));
                    y = y.clamp(work.top, (work.bottom - h).max(work.top));
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        x,
                        y,
                        w,
                        h,
                        SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSENDCHANGING,
                    );
                    let mut s = STATE.lock().unwrap();
                    s.mode = Mode::Move;
                    s.hwnd = hwnd.0 as isize;
                    s.origin_x = pt.x;
                    s.origin_y = pt.y;
                    s.win_x = x;
                    s.win_y = y;
                    s.win_w = w;
                    s.win_h = h;
                    ANY_DRAG.store(true, Ordering::Relaxed);
                    return suppress;
                } else if GetWindowRect(hwnd, &mut rect).is_ok() {
                    let mut s = STATE.lock().unwrap();
                    s.mode = Mode::Move;
                    s.hwnd = hwnd.0 as isize;
                    s.origin_x = pt.x;
                    s.origin_y = pt.y;
                    s.win_x = rect.left;
                    s.win_y = rect.top;
                    s.win_w = rect.right - rect.left;
                    s.win_h = rect.bottom - rect.top;
                    ANY_DRAG.store(true, Ordering::Relaxed);
                    return suppress;
                }
            }
        }
        WM_RBUTTONDOWN if left_alt_down() && !drag_active() => {
            if let Some(hwnd) = root_window_at(pt) {
                let mut rect = RECT::default();
                if GetWindowRect(hwnd, &mut rect).is_ok() {
                    let cx = (rect.left + rect.right) / 2;
                    let cy = (rect.top + rect.bottom) / 2;
                    let left = pt.x < cx;
                    let top = pt.y < cy;
                    let corner_x = if left { rect.left } else { rect.right };
                    let corner_y = if top { rect.top } else { rect.bottom };
                    set_marker_shape(left, top);
                    show_marker(corner_x, corner_y, left, top);
                    let mut s = STATE.lock().unwrap();
                    s.mode = Mode::Resize;
                    s.hwnd = hwnd.0 as isize;
                    s.origin_x = pt.x;
                    s.origin_y = pt.y;
                    s.win_x = rect.left;
                    s.win_y = rect.top;
                    s.win_w = rect.right - rect.left;
                    s.win_h = rect.bottom - rect.top;
                    s.left = left;
                    s.top = top;
                    ANY_DRAG.store(true, Ordering::Relaxed);
                    return suppress;
                }
            }
        }
        WM_MOUSEMOVE if ANY_DRAG.load(Ordering::Relaxed) => {
            // NOTE: do NOT suppress mouse-move events. Returning 1 here would
            // freeze the physical cursor, so `pt` never advances and the window
            // can't follow. We reposition the window and let the move pass through.
            //
            // We also can't trust GetAsyncKeyState for the drag button here: the
            // button-down was suppressed, so the OS thinks it's up. The drag is
            // ended only by the matching button-up event below.
            //
            // The ANY_DRAG guard keeps every other process's mouse-move off the
            // STATE mutex entirely — only an active drag reaches this lock.
            let s = STATE.lock().unwrap();
            match s.mode {
                Mode::Move => {
                    let dx = pt.x - s.origin_x;
                    let dy = pt.y - s.origin_y;
                    set_target(s.hwnd, s.win_x + dx, s.win_y + dy, 0, 0, false);
                }
                Mode::Resize => {
                    // Drag the nearest corner; the opposite corner stays fixed.
                    let dx = pt.x - s.origin_x;
                    let dy = pt.y - s.origin_y;
                    let mut x = s.win_x;
                    let mut y = s.win_y;
                    let mut w;
                    let mut h;
                    if s.left {
                        x = s.win_x + dx;
                        w = s.win_w - dx;
                    } else {
                        w = s.win_w + dx;
                    }
                    if s.top {
                        y = s.win_y + dy;
                        h = s.win_h - dy;
                    } else {
                        h = s.win_h + dy;
                    }
                    if w < MIN_W {
                        if s.left {
                            x = s.win_x + (s.win_w - MIN_W);
                        }
                        w = MIN_W;
                    }
                    if h < MIN_H {
                        if s.top {
                            y = s.win_y + (s.win_h - MIN_H);
                        }
                        h = MIN_H;
                    }
                    set_target(s.hwnd, x, y, w, h, true);
                    let corner_x = if s.left { x } else { x + w };
                    let corner_y = if s.top { y } else { y + h };
                    show_marker(corner_x, corner_y, s.left, s.top);
                }
                Mode::None => {}
            }
        }
        WM_LBUTTONUP => {
            let mut s = STATE.lock().unwrap();
            if s.mode == Mode::Move {
                let h = s.hwnd;
                s.mode = Mode::None;
                ANY_DRAG.store(false, Ordering::Relaxed);
                drop(s);
                // Re-integrate the dropped window into the tiling layout.
                push_cmd(Cmd::DragMoved(h, pt.x, pt.y));
                return suppress;
            }
        }
        WM_RBUTTONUP => {
            let mut s = STATE.lock().unwrap();
            if s.mode == Mode::Resize {
                let h = s.hwnd;
                s.mode = Mode::None;
                ANY_DRAG.store(false, Ordering::Relaxed);
                drop(s);
                hide_marker();
                // Apply the new size to the layout (master ratio) or snap back.
                push_cmd(Cmd::DragResized(h));
                return suppress;
            }
        }
        _ => {}
    }

    CallNextHookEx(None, code, wparam, lparam)
}

// =========================================================================
// Tiling window manager
//
// A dedicated manager thread owns all monitor/workspace state; the input/event
// hooks only push lightweight commands onto a queue and return immediately, so
// the low-level hooks never block on SetWindowPos/EnumWindows.
//
// Each monitor owns its own set of workspaces (GlazeWM / Hyprland style) and is
// tiled independently on its own work area. Windows are positioned with
// individual SetWindowPos calls (restore-then-place) — a robust approach used
// by komorebi; a single DeferWindowPos batch can fail wholesale if one window
// misbehaves, leaving everything un-tiled.
// =========================================================================

/// Runtime configuration, loaded from suprland.conf at startup.
#[derive(Clone)]
struct Config {
    per_monitor: bool,          // true: Alt+1..9 switches focused monitor only
    start_tiled: bool,          // tile automatically on launch
    outer_gap: i32,             // gap between windows and screen edge
    inner_gap: i32,             // gap between adjacent windows
    master_ratio: f32,          // fraction of width given to the master window
    workspaces: usize,          // workspaces per monitor (1..10)
    workspace_keys: Vec<u32>,   // VK code per workspace; Alt+key switches, +Shift moves
    layout: String,            // "dwindle" (spiral into a corner) or "master"
    terminal: String,           // command launched by Alt+Enter
    browser: String,            // Alt+Shift+Enter; empty = default browser
    unfocused_opacity: f32,     // 0.0-1.0 alpha for unfocused windows (1.0 = off)
    border_enabled: bool,       // draw coloured DWM borders (Windows 11)
    focused_border: u32,        // COLORREF for the focused window border
    unfocused_border: u32,      // COLORREF for unfocused window borders
    cursor_follows_focus: bool, // warp the mouse to the focused window
    focus_follows_mouse: bool,  // hovering a window focuses it (Hyprland follow_mouse)
    bar_enabled: bool,          // draw the status bar (waybar-style) on every monitor
    bar_height: i32,            // bar thickness in px (work area is reserved for it)
    bar_bottom: bool,           // dock the bar at the bottom instead of the top
    bar_font_size: i32,         // text height in px; 0 = auto from bar_height
    bar_show_title: bool,       // show the focused window title
    bar_show_clock: bool,       // show the clock
    bar_clock_24h: bool,        // 24-hour clock (false = 12-hour with am/pm)
    bar_show_layout: bool,      // show layout + tiling/floating state on the right
    bar_bg: u32,                // COLORREF bar background
    bar_fg: u32,                // COLORREF bar text
    bar_accent: u32,            // COLORREF active-workspace highlight
    bar_inactive: u32,          // COLORREF empty-workspace text
    ignore_classes: Vec<String>, // window classes never tiled/managed
    float_classes: Vec<String>,  // window classes managed but auto-floated
    key_focus_next: u32,      // Alt+<key> focus next window in the stack (default J)
    key_focus_prev: u32,      // Alt+<key> focus previous window in the stack (default K)
    key_shrink_master: u32,   // Alt+<key> shrink the master area (default H)
    key_grow_master: u32,     // Alt+<key> grow the master area (default L)
    key_promote_master: u32,  // Alt+<key> promote focused window to master (default M)
    key_toggle_tiling: u32,   // Alt+<key> toggle tiling on/off (default T)
    key_toggle_float: u32,    // Alt+<key> toggle floating for focused window (default F)
    key_close_window: u32,    // Alt+<key> close the focused window (default W)
}

impl Config {
    fn defaults() -> Self {
        Config {
            per_monitor: false,
            start_tiled: true,
            outer_gap: 8,
            inner_gap: 8,
            master_ratio: 0.55,
            workspaces: 9,
            workspace_keys: parse_keys("1 2 3 4 5 6 7 8 9 0"),
            layout: "dwindle".to_string(),
            terminal: "wt.exe".to_string(),
            browser: String::new(),
            unfocused_opacity: 0.8,
            border_enabled: true,
            focused_border: parse_color("#66AAFF", 0x00FFAA66),
            unfocused_border: parse_color("#223A5E", 0x005E3A22),
            cursor_follows_focus: true,
            focus_follows_mouse: false,
            bar_enabled: true,
            bar_height: 28,
            bar_bottom: false,
            bar_font_size: 0,
            bar_show_title: true,
            bar_show_clock: true,
            bar_clock_24h: true,
            bar_show_layout: true,
            bar_bg: parse_color("#1A1B26", 0x00261B1A),
            bar_fg: parse_color("#C0CAF5", 0x00F5CAC0),
            bar_accent: parse_color("#66AAFF", 0x00FFAA66),
            bar_inactive: parse_color("#565F89", 0x00895F56),
            ignore_classes: Vec::new(),
            float_classes: Vec::new(),
            key_focus_next: 0x4A,     // J
            key_focus_prev: 0x4B,     // K
            key_shrink_master: 0x48,  // H
            key_grow_master: 0x4C,    // L
            key_promote_master: 0x4D, // M
            key_toggle_tiling: 0x54,  // T
            key_toggle_float: 0x46,   // F
            key_close_window: 0x57,   // W
        }
    }
}

const DEFAULT_CONFIG: &str = "\
# ============================================================================
# suprland configuration  (window manager)
# ============================================================================
# Location : %USERPROFILE%\\.suprland\\suprland.conf
#            (override with the SUPRLAND_CONFIG environment variable)
# The status bar is configured separately in navbar.conf (same folder).
# Apply    : edit this file, then restart suprland.
# Regen    : delete this file and relaunch to get a fresh, fully-commented copy.
#
# Syntax   : one  key = value  per line. '#' starts a comment. Blank lines and
#            surrounding whitespace are ignored. Unknown keys are ignored, so a
#            typo silently falls back to the default below rather than erroring.
#
# Value types:
#   bool   : true / false   (also accepts yes/no, 1/0, on/off; anything else
#            counts as false)
#   int    : whole number; clamped to the stated range
#   float  : decimal; clamped to the stated range
#   colour : #RRGGBB hex (e.g. #66AAFF). Malformed values keep the default.
#   text   : literal string (program name / command line)
#   keys   : space/comma list of key names: 0-9, A-Z, F1-F24.
#
# Every key below is shown set to its built-in DEFAULT.
# ============================================================================

# ---------------------------------------------------------------------------
# Workspaces & layout
# ---------------------------------------------------------------------------

# How workspaces map to monitors.
#   shared      = workspaces are numbered globally, starting from your MAIN
#                 (primary) monitor and rotating outward. With 3 monitors,
#                 ws1 = main monitor, ws2 = next, ws3 = next, ws4 = main (its
#                 2nd workspace), and so on. The workspace key jumps focus to
#                 whichever monitor owns that workspace.
#   per_monitor = each monitor has its own independent workspaces 1..N
#                 (GlazeWM style). The workspace key switches the workspace on
#                 the monitor that currently has focus.
# values: shared | per_monitor
workspace_mode = shared

# Number of workspaces.  int 1 - 10
#   shared mode      = TOTAL across all monitors (distributed from the main
#                      monitor outward).
#   per_monitor mode = workspaces per monitor.
workspaces = 10

# Keys (with LEFT ALT) that select workspaces 1, 2, 3, ... in order. Add Shift
# to MOVE the focused window to that workspace instead. Avoid the fixed binds
# (J K H L M T F W and arrows/Enter). Example for Alt+Q/W/E = ws 1/2/3:
#   workspace_keys = Q W E R T Y
# keys
workspace_keys = 1 2 3 4 5 6 7 8 9 0

# Tile windows automatically on launch (Alt+T toggles at runtime).  bool
start_tiled = true

# Tiling layout.
#   dwindle = each new window splits the remaining space, spiralling into the
#             bottom corner (Hyprland / omarchy default).
#   master  = one large master column on the left, the rest stacked on the right.
# values: dwindle | master
layout = dwindle

# Fraction of the width given to the master window (master layout, and the
# master split that Alt+H / Alt+L adjust).  float 0.10 - 0.90
master_ratio = 0.55

# ---------------------------------------------------------------------------
# Gaps
# ---------------------------------------------------------------------------

# Gap in pixels between windows and the screen edge.  int (>= 0)
outer_gap = 8
# Gap in pixels between adjacent windows.  int (>= 0)
inner_gap = 8

# ---------------------------------------------------------------------------
# Focus & cursor behaviour
# ---------------------------------------------------------------------------

# Warp the mouse cursor onto the window that gains focus via Alt+arrows and on
# workspace switches.  bool
cursor_follows_focus = true

# Focus follows mouse: hovering a window with the cursor focuses it, like
# Hyprland's follow_mouse. Off by default (Windows focus-steal is more abrupt
# than on Wayland); set true for the omarchy/Hyprland feel.  bool
focus_follows_mouse = false

# ---------------------------------------------------------------------------
# Appearance: window borders & dimming
# ---------------------------------------------------------------------------

# Dim unfocused windows to this opacity.  float 0.10 - 1.00  (1.0 = disabled)
unfocused_opacity = 0.8

# Coloured window borders. Requires Windows 11 (no effect on Windows 10).  bool
border_enabled = true
# Border colour of the focused window.    colour
focused_border = #66AAFF
# Border colour of unfocused windows.     colour
unfocused_border = #223A5E

# ---------------------------------------------------------------------------
# Window rules
# ---------------------------------------------------------------------------
# Comma-separated lists of window CLASS names (not titles). Use a tool like
# AutoHotkey's Window Spy, or Spy++, to find a window's class.

# Never tile/manage these (in addition to the built-in shell/tooltip/lock-screen
# filtering). Example: ignore_classes = TaskManagerWindow, MyOverlayClass
ignore_classes =
# Manage but always float these (let the app place them; don't tile).
# Example: float_classes = #32770, MsiDialogCloseClass
float_classes =

# ---------------------------------------------------------------------------
# Launchers
# ---------------------------------------------------------------------------

# Program launched by Alt+Enter. Resolved via the shell, so PATH and App
# Execution Aliases (e.g. wt.exe) work.  text
terminal = wt.exe
# Program launched by Alt+Shift+Enter. Leave EMPTY to open the system default
# browser.  text
browser =

# ============================================================================
# Hotkeys (LEFT ALT is the modifier)
# ============================================================================
#   Alt + left-drag      move window under cursor (drops back into the tiling)
#   Alt + right-drag     resize nearest corner (red bracket marker)
#   Alt+T                toggle tiling on/off (floating mode; workspaces kept)
#   Alt+J / Alt+K        focus next / previous window in the stack
#   Alt+Shift+J / K      swap window order in the stack
#   Alt+arrows           focus window by direction (cursor follows)
#   Alt+Shift+arrows     move window by direction (across monitors)
#   Alt+M                promote focused window to master
#   Alt+H / Alt+L        shrink / grow the master area
#   Alt+F                toggle floating for the focused window
#   Alt+W                close the focused window
#   Alt+Enter            launch terminal
#   Alt+Shift+Enter      launch browser
#   Alt+<workspace_key>  switch to that workspace (see workspace_keys above)
#   Alt+Shift+<ws key>   move focused window to that workspace (and follow it)
#   Alt+Tab              normal task switcher (still works)
#   RIGHT ALT            normal Alt behaviour (LEFT ALT is reserved by suprland)
#
# The letter keys above (J K H L M T F W) are rebindable. Each takes a single
# key name (see the 'keys' type at the top of this file). Arrows and Enter
# are fixed.  key
key_focus_next = J
key_focus_prev = K
key_shrink_master = H
key_grow_master = L
key_promote_master = M
key_toggle_tiling = T
key_toggle_float = F
key_close_window = W
# ============================================================================
";

const DEFAULT_NAVBAR: &str = "\
# ============================================================================
# suprland navbar configuration  (status bar)
# ============================================================================
# Location : %USERPROFILE%\\.suprland\\navbar.conf
#            (override with the SUPRLAND_NAVBAR environment variable)
# Window-manager settings live separately in suprland.conf (same folder).
# Apply    : edit this file, then restart suprland.
#
# One bar is drawn on EVERY monitor. Each shows that monitor's workspaces and
# focused window. The tiling work area is reserved so windows never sit under a
# bar. Click a workspace pill to switch to it.
#
# Value types: bool, int, colour (#RRGGBB) -- see suprland.conf for details.
# ============================================================================

# Show the bars.  bool   (set false to disable entirely)
enabled = true
# Bar thickness in pixels.  int 0 - 200  (0 also disables it)
height = 28
# Dock the bars at the bottom of each screen instead of the top.  bool
bottom = false
# Text height in px. 0 = auto (about half the bar height).  int 0 - 100
font_size = 0
# Show the focused window title.  bool
show_title = true
# Show the clock.  bool
show_clock = true
# 24-hour clock; false = 12-hour with am/pm.  bool
clock_24h = true
# Show the layout name + tiling/floating state on the right.  bool
show_layout = true

# Colours (#RRGGBB).
bg = #1A1B26
fg = #C0CAF5
# Active-workspace pill highlight.
accent = #66AAFF
# Empty workspaces / layout text.
inactive = #565F89
";

fn parse_bool(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "1" | "on")
}

/// Parse a comma/semicolon-separated list of window-class names, trimmed.
fn parse_list(v: &str) -> Vec<String> {
    v.split([',', ';'])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Map a key name ("1", "Q", "F3") to its Win32 virtual-key code.
fn key_to_vk(name: &str) -> Option<u32> {
    let n = name.trim().to_ascii_uppercase();
    let b = n.as_bytes();
    if b.len() == 1 {
        let c = b[0];
        if c.is_ascii_digit() || c.is_ascii_uppercase() {
            return Some(c as u32); // ASCII '0'-'9'/'A'-'Z' == their VK codes
        }
    }
    if let Some(num) = n.strip_prefix('F') {
        if let Ok(k) = num.parse::<u32>() {
            if (1..=24).contains(&k) {
                return Some(0x70 + k - 1); // VK_F1 = 0x70
            }
        }
    }
    None
}

/// Parse a space/comma-separated list of key names into VK codes.
fn parse_keys(v: &str) -> Vec<u32> {
    v.split([',', ' ', '\t'])
        .filter_map(|s| {
            let s = s.trim();
            if s.is_empty() {
                None
            } else {
                key_to_vk(s)
            }
        })
        .collect()
}

/// Parse a #RRGGBB hex string into a Win32 COLORREF (0x00BBGGRR). Falls back to
/// `fallback` on any malformed input.
fn parse_color(v: &str, fallback: u32) -> u32 {
    let s = v.trim().trim_start_matches('#');
    if s.len() == 6 {
        if let Ok(rgb) = u32::from_str_radix(s, 16) {
            let r = (rgb >> 16) & 0xFF;
            let g = (rgb >> 8) & 0xFF;
            let b = rgb & 0xFF;
            return (b << 16) | (g << 8) | r;
        }
    }
    fallback
}

/// Resolve a config file path: env override, else %USERPROFILE%\.suprland\<name>.
fn config_path(env: &str, name: &str) -> std::path::PathBuf {
    if let Ok(p) = std::env::var(env) {
        return std::path::PathBuf::from(p);
    }
    let mut dir = std::env::var("USERPROFILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    dir.push(".suprland");
    dir.push(name);
    dir
}

/// Read a config file, writing `default` the first time it is missing.
fn read_or_create(path: &std::path::Path, default: &str) -> String {
    match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(path, default);
            println!("wrote default config: {}", path.display());
            default.to_string()
        }
    }
}

/// Load settings from suprland.conf (window manager) and navbar.conf (status
/// bar), creating each with documented defaults when missing.
fn load_config() -> Config {
    let mut c = Config::defaults();
    let wm = config_path("SUPRLAND_CONFIG", "suprland.conf");
    parse_into(&mut c, &read_or_create(&wm, DEFAULT_CONFIG));
    let nav = config_path("SUPRLAND_NAVBAR", "navbar.conf");
    parse_into(&mut c, &read_or_create(&nav, DEFAULT_NAVBAR));
    c
}

/// Apply `key = value` lines from `text` onto `c`. Unknown keys are ignored.
/// Recognises both the window-manager keys (suprland.conf) and the navbar keys
/// (navbar.conf, unprefixed) so either file may set either, and old configs that
/// used the `bar_*` names keep working.
fn parse_into(c: &mut Config, text: &str) {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        match k {
            // ---- window manager (suprland.conf) ----
            "workspace_mode" => c.per_monitor = v.eq_ignore_ascii_case("per_monitor"),
            "start_tiled" => c.start_tiled = parse_bool(v),
            "outer_gap" => {
                if let Ok(n) = v.parse() {
                    c.outer_gap = n;
                }
            }
            "inner_gap" => {
                if let Ok(n) = v.parse() {
                    c.inner_gap = n;
                }
            }
            "master_ratio" => {
                if let Ok(n) = v.parse::<f32>() {
                    c.master_ratio = n.clamp(0.1, 0.9);
                }
            }
            "workspaces" => {
                if let Ok(n) = v.parse::<usize>() {
                    c.workspaces = n.clamp(1, 10);
                }
            }
            "workspace_keys" => {
                let keys = parse_keys(v);
                if !keys.is_empty() {
                    c.workspace_keys = keys;
                }
            }
            "layout" => c.layout = v.to_ascii_lowercase(),
            "terminal" => c.terminal = v.to_string(),
            "browser" => c.browser = v.to_string(),
            "unfocused_opacity" => {
                if let Ok(n) = v.parse::<f32>() {
                    c.unfocused_opacity = n.clamp(0.1, 1.0);
                }
            }
            "border_enabled" => c.border_enabled = parse_bool(v),
            "focused_border" => c.focused_border = parse_color(v, c.focused_border),
            "unfocused_border" => c.unfocused_border = parse_color(v, c.unfocused_border),
            "cursor_follows_focus" => c.cursor_follows_focus = parse_bool(v),
            "focus_follows_mouse" => c.focus_follows_mouse = parse_bool(v),
            "ignore_classes" => c.ignore_classes = parse_list(v),
            "float_classes" => c.float_classes = parse_list(v),
            "key_focus_next" => {
                if let Some(k) = key_to_vk(v) {
                    c.key_focus_next = k;
                }
            }
            "key_focus_prev" => {
                if let Some(k) = key_to_vk(v) {
                    c.key_focus_prev = k;
                }
            }
            "key_shrink_master" => {
                if let Some(k) = key_to_vk(v) {
                    c.key_shrink_master = k;
                }
            }
            "key_grow_master" => {
                if let Some(k) = key_to_vk(v) {
                    c.key_grow_master = k;
                }
            }
            "key_promote_master" => {
                if let Some(k) = key_to_vk(v) {
                    c.key_promote_master = k;
                }
            }
            "key_toggle_tiling" => {
                if let Some(k) = key_to_vk(v) {
                    c.key_toggle_tiling = k;
                }
            }
            "key_toggle_float" => {
                if let Some(k) = key_to_vk(v) {
                    c.key_toggle_float = k;
                }
            }
            "key_close_window" => {
                if let Some(k) = key_to_vk(v) {
                    c.key_close_window = k;
                }
            }
            // ---- navbar (navbar.conf, unprefixed) and legacy bar_* aliases ----
            "enabled" | "bar_enabled" => c.bar_enabled = parse_bool(v),
            "height" | "bar_height" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.bar_height = n.clamp(0, 200);
                }
            }
            "bottom" | "bar_bottom" => c.bar_bottom = parse_bool(v),
            "font_size" | "bar_font_size" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.bar_font_size = n.clamp(0, 100);
                }
            }
            "show_title" | "bar_show_title" => c.bar_show_title = parse_bool(v),
            "show_clock" | "bar_show_clock" => c.bar_show_clock = parse_bool(v),
            "clock_24h" | "bar_clock_24h" => c.bar_clock_24h = parse_bool(v),
            "show_layout" | "bar_show_layout" => c.bar_show_layout = parse_bool(v),
            "bg" | "bar_bg" => c.bar_bg = parse_color(v, c.bar_bg),
            "fg" | "bar_fg" => c.bar_fg = parse_color(v, c.bar_fg),
            "accent" | "bar_accent" => c.bar_accent = parse_color(v, c.bar_accent),
            "inactive" | "bar_inactive" => c.bar_inactive = parse_color(v, c.bar_inactive),
            _ => {}
        }
    }
}

/// A spatial direction for arrow-key focus/move.
#[derive(Clone, Copy)]
enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Commands sent from the hooks to the manager thread.
enum Cmd {
    Add(isize),
    Remove(isize),
    Focused(isize),
    FocusDir(i32),
    SwapDir(i32),
    PromoteMaster,
    ResizeMaster(f32),
    Switch(usize),
    MoveToWs(usize),
    ToggleTiling,
    ToggleFloat,
    CloseFocused,
    Retile,
    RefreshMonitors,
    DragMoved(isize, i32, i32), // window dropped after an Alt+left-drag (hwnd, x, y)
    DragResized(isize),         // window released after an Alt+right-drag resize
    LaunchTerminal,             // Alt+Enter
    LaunchBrowser,              // Alt+Shift+Enter
    FocusGeo(Dir),              // Alt+arrow: focus the window in a direction
    MoveGeo(Dir),               // Alt+Shift+arrow: move the window in a direction
    FocusMouse(isize),          // focus-follows-mouse: cursor hovered this window
    BarClick(isize, usize),     // bar pill clicked: (monitor hmon, local workspace)
    Reload(Box<Config>),        // config file changed on disk; apply live
}

static CMDQ: Mutex<VecDeque<Cmd>> = Mutex::new(VecDeque::new());
static CMDCV: Condvar = Condvar::new();
// While true, programmatic show/hide must not be mistaken for app events.
static SUPPRESS: AtomicBool = AtomicBool::new(false);
// De-duplicates auto-repeat key-downs for our hotkeys.
static PRESSED: Mutex<[bool; 256]> = Mutex::new([false; 256]);
// Every window the manager currently tracks (across all monitors/workspaces).
// Kept in sync by the manager so the shutdown handler can reveal them all.
static MANAGED: Mutex<Vec<isize>> = Mutex::new(Vec::new());
// O(1) window -> (monitor, workspace) lookup, rebuilt by sync_managed once per
// command (it already walks every window, so this is free). `locate` reads it.
static INDEX: Mutex<Option<HashMap<isize, (usize, usize)>>> = Mutex::new(None);
// Mirror of cfg.focus_follows_mouse readable by the poll thread without the cfg.
static FOLLOW_MOUSE: AtomicBool = AtomicBool::new(false);
// Last window seen as foreground, to collapse duplicate foreground events.
static LAST_FG: AtomicIsize = AtomicIsize::new(0);
// Config-driven window-class filters, populated once at startup so the hooks and
// is_manageable can read them without threading the whole Config through.
static IGNORE_CLASSES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static FLOAT_CLASSES: Mutex<Vec<String>> = Mutex::new(Vec::new());
// VK code per workspace (index = workspace), read by the keyboard hook.
static WORKSPACE_KEYS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Rebindable single-letter hotkeys (config keys `key_*`); defaults match the
/// historical hardcoded J/K/H/L/M/T/F/W binds.
struct HotkeyBinds {
    focus_next: u32,
    focus_prev: u32,
    shrink_master: u32,
    grow_master: u32,
    promote_master: u32,
    toggle_tiling: u32,
    toggle_float: u32,
    close_window: u32,
}
static HOTKEYS: Mutex<HotkeyBinds> = Mutex::new(HotkeyBinds {
    focus_next: 0x4A,
    focus_prev: 0x4B,
    shrink_master: 0x48,
    grow_master: 0x4C,
    promote_master: 0x4D,
    toggle_tiling: 0x54,
    toggle_float: 0x46,
    close_window: 0x57,
});

// ---- status bar (one per monitor) ----
/// A bar window bound to one monitor.
#[derive(Clone, Copy)]
struct BarWin {
    hwnd: isize,
    hmon: isize,
}
static BARS: Mutex<Vec<BarWin>> = Mutex::new(Vec::new());
// HINSTANCE stashed so the display-change handler can create bars for new monitors.
static BAR_HINST: AtomicIsize = AtomicIsize::new(0);
// Bar geometry, set at startup so ensure_bars works without a Config in hand.
static BAR_HEIGHT: AtomicIsize = AtomicIsize::new(0); // 0 = bar disabled
static BAR_BOTTOM: AtomicBool = AtomicBool::new(false);
static BAR_FONT_SIZE: AtomicIsize = AtomicIsize::new(0); // 0 = auto from height
// Width of each workspace pill in px, and the bar text height, set from config.
static BAR_CELL: AtomicIsize = AtomicIsize::new(34);
// Shared font handle for all bars (created once).
static BAR_FONT: AtomicIsize = AtomicIsize::new(0);

/// Per-monitor paint data. `labels` are the workspace numbers to print (which
/// differ between shared and per_monitor modes); `active`/`occupied` index into
/// that same list, so the click handler can map a pill straight to a workspace.
#[derive(Clone, PartialEq)]
struct MonBar {
    hmon: isize,
    labels: Vec<usize>,
    active: usize,
    occupied: u64,
    title: String,
}

/// Everything the bars paint. Replaced wholesale by the manager each update.
#[derive(Clone)]
struct BarData {
    bg: u32,
    fg: u32,
    accent: u32,
    inactive: u32,
    show_title: bool,
    show_clock: bool,
    clock_24h: bool,
    show_layout: bool,
    layout: String,
    tiling: bool,
    mons: Vec<MonBar>,
}

impl BarData {
    const fn new() -> Self {
        BarData {
            bg: 0x00261B1A,
            fg: 0x00F5CAC0,
            accent: 0x00FFAA66,
            inactive: 0x00895F56,
            show_title: true,
            show_clock: true,
            clock_24h: true,
            show_layout: true,
            layout: String::new(),
            tiling: true,
            mons: Vec::new(),
        }
    }
}

static BAR: Mutex<BarData> = Mutex::new(BarData::new());
// Custom message: manager asks a bar to repaint.
const WM_BAR_REFRESH: u32 = WM_USER + 1;
// Custom message (to the marker window): config changed, rebuild bars on the
// main thread.
const WM_RELOAD: u32 = WM_USER + 2;
// SetTimer id for the bar clock tick.
const BAR_TIMER_ID: usize = 1;

fn push_cmd(c: Cmd) {
    CMDQ.lock().unwrap().push_back(c);
    CMDCV.notify_one();
}

struct Workspace {
    windows: Vec<isize>,  // all managed windows in this workspace (tiled order)
    floating: Vec<isize>, // subset of `windows` excluded from tiling
    focused: isize,       // last-focused window handle (0 = none)
    // Per-split size ratios for the dwindle layout (index = split level, i.e.
    // tiled-window index). Each is the fraction the window at that level takes of
    // its split; missing/extra entries default to 0.5. Edited by resizing.
    splits: Vec<f32>,
}

impl Workspace {
    fn new() -> Self {
        Workspace {
            windows: Vec::new(),
            floating: Vec::new(),
            focused: 0,
            splits: Vec::new(),
        }
    }
}

/// One physical display: its own workspaces, tiled on its own work area.
struct Monitor {
    hmon: isize,        // HMONITOR (raw) — identity across enumerations
    base_work: RECT,    // taskbar-excluded area, before the bar is subtracted
    work_area: RECT,    // tiling area (base_work minus the status bar)
    workspaces: Vec<Workspace>,
    active: usize,      // index of the currently-shown workspace
}

impl Monitor {
    fn new(hmon: isize, work_area: RECT, count: usize) -> Self {
        let mut workspaces = Vec::with_capacity(count);
        for _ in 0..count {
            workspaces.push(Workspace::new());
        }
        Monitor {
            hmon,
            base_work: work_area,
            work_area,
            workspaces,
            active: 0,
        }
    }
}

struct Manager {
    monitors: Vec<Monitor>,
    focused_mon: usize,
    primary: usize, // index of the main monitor; workspace 1 starts here
    tiling: bool,
    cfg: Config,
}

impl Manager {
    fn mon_by_hmon(&self, raw: isize) -> Option<usize> {
        self.monitors.iter().position(|m| m.hmon == raw)
    }

    /// Map a global (shared-mode) workspace index to (monitor, local workspace).
    /// Numbering starts at the primary monitor and rotates outward, so ws1 is
    /// always on the user's main screen. In per_monitor mode it targets the
    /// currently-focused monitor.
    fn global_to_ml(&self, i: usize) -> (usize, usize) {
        if self.cfg.per_monitor {
            (self.focused_mon.min(self.monitors.len().saturating_sub(1)), i)
        } else {
            let n = self.monitors.len().max(1);
            ((self.primary + (i % n)) % n, i / n)
        }
    }

    /// Inverse of `global_to_ml` for shared mode: the global workspace number a
    /// monitor's local workspace belongs to.
    fn ml_to_global(&self, mi: usize, local: usize) -> usize {
        if self.cfg.per_monitor {
            local
        } else {
            let n = self.monitors.len().max(1);
            let off = (mi + n - self.primary % n) % n;
            local * n + off
        }
    }

    /// Locate a tracked window as (monitor index, workspace index).
    ///
    /// O(1) via the INDEX snapshot (rebuilt by sync_managed after every command);
    /// falls back to a linear scan for handles added within the current command,
    /// before the next reindex, so it can never miss a live window.
    fn locate(&self, h: isize) -> Option<(usize, usize)> {
        if let Some(map) = INDEX.lock().unwrap().as_ref() {
            if let Some(&p) = map.get(&h) {
                // Guard against a stale entry from a since-moved window.
                if self
                    .monitors
                    .get(p.0)
                    .and_then(|m| m.workspaces.get(p.1))
                    .is_some_and(|ws| ws.windows.contains(&h))
                {
                    return Some(p);
                }
            }
        }
        for (mi, m) in self.monitors.iter().enumerate() {
            for (wi, ws) in m.workspaces.iter().enumerate() {
                if ws.windows.contains(&h) {
                    return Some((mi, wi));
                }
            }
        }
        None
    }
}

/// Read a window's class name.
unsafe fn window_class(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = GetClassNameW(hwnd, &mut buf);
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Shell/system window classes that must never be tiled. Tooltips, the lock
/// screen, the task-view/alt-tab surfaces, and various invisible UWP host and
/// IME windows all show up as top-level windows and would otherwise be grabbed.
const BLOCK_CLASSES: &[&str] = &[
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Progman",
    "WorkerW",
    "Windows.UI.Core.CoreWindow",
    "Windows.UI.Composition.DesktopWindowContentBridge",
    "Windows.Internal.Shell.TabProxyWindow",
    "ForegroundStaging",
    "MultitaskingViewFrame",
    "XamlExplorerHostIslandWindow",
    "ShellExperienceHost",
    "tooltips_class32",                // generic Win32 tooltips
    "LockScreenBackstopFrame",         // lock screen
    "LockApp",
    "WinUIDesktopWin32WindowClass",    // some transient WinUI shells
    "EdgeUiInputTopWndClass",
    "Windows.UI.Input.InputSite.WindowClass",
    "IME",
    "MSCTFIME UI",
    "Default IME",
    "hyprwin_marker",
    "suprland_bar",
];

/// Is this a normal top-level application window we should tile?
unsafe fn is_manageable(hwnd: HWND) -> bool {
    if hwnd.0.is_null() || !IsWindowVisible(hwnd).as_bool() {
        return false;
    }
    // Never manage our own windows (console, marker, bars).
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == GetCurrentProcessId() {
        return false;
    }
    // Only true top-level roots, no owned tool/dialog windows.
    if GetAncestor(hwnd, GA_ROOT) != hwnd {
        return false;
    }
    if let Ok(owner) = GetWindow(hwnd, GW_OWNER) {
        if !owner.0.is_null() {
            return false;
        }
    }
    let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    // Child windows, tool windows, and non-activatable windows (tooltips, OSDs,
    // the lock-screen overlay, IME candidates) are never real app windows.
    if style & WS_CHILD.0 != 0
        || ex & WS_EX_TOOLWINDOW.0 != 0
        || ex & WS_EX_NOACTIVATE.0 != 0
    {
        return false;
    }
    if GetWindowTextLengthW(hwnd) == 0 {
        return false;
    }
    // Skip cloaked windows (e.g. UWP ghost windows on other virtual desktops).
    let mut cloaked = 0u32;
    let _ = DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        &mut cloaked as *mut _ as *mut c_void,
        core::mem::size_of::<u32>() as u32,
    );
    if cloaked != 0 {
        return false;
    }
    // Reject known shell/desktop classes and any user-configured ignore list.
    let class = window_class(hwnd);
    if BLOCK_CLASSES.contains(&class.as_str()) {
        return false;
    }
    if IGNORE_CLASSES
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.eq_ignore_ascii_case(&class))
    {
        return false;
    }
    true
}

/// Should a freshly-managed window of this class start floating?
unsafe fn should_float(hwnd: HWND) -> bool {
    let class = window_class(hwnd);
    FLOAT_CLASSES
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.eq_ignore_ascii_case(&class))
}

/// Compute the visible-frame correction: Win32 GetWindowRect includes an
/// invisible DWM shadow border, so we expand the target by that padding to make
/// the *visible* edges line up flush, giving even gaps.
unsafe fn adjust_for_border(hwnd: HWND, target: RECT) -> RECT {
    let mut wr = RECT::default();
    if GetWindowRect(hwnd, &mut wr).is_err() {
        return target;
    }
    let mut fr = RECT::default();
    let ok = DwmGetWindowAttribute(
        hwnd,
        DWMWA_EXTENDED_FRAME_BOUNDS,
        &mut fr as *mut _ as *mut c_void,
        core::mem::size_of::<RECT>() as u32,
    )
    .is_ok();
    if !ok {
        return target;
    }
    let lp = fr.left - wr.left;
    let tp = fr.top - wr.top;
    let rp = wr.right - fr.right;
    let bp = wr.bottom - fr.bottom;
    RECT {
        left: target.left - lp,
        top: target.top - tp,
        right: target.right + rp,
        bottom: target.bottom + bp,
    }
}

/// Master-stack layout (Hyprland dwindle-lite): one master column on the left,
/// the remaining windows stacked vertically on the right.
fn master_stack(area: RECT, n: usize, ratio: f32, outer: i32, inner: i32) -> Vec<RECT> {
    let mut out = Vec::with_capacity(n);
    let x0 = area.left + outer;
    let y0 = area.top + outer;
    let w = area.right - area.left - 2 * outer;
    let h = area.bottom - area.top - 2 * outer;
    if n == 0 || w <= 0 || h <= 0 {
        return out;
    }
    if n == 1 {
        out.push(RECT {
            left: x0,
            top: y0,
            right: x0 + w,
            bottom: y0 + h,
        });
        return out;
    }
    let master_w = ((w - inner) as f32 * ratio) as i32;
    let stack_w = (w - inner) - master_w;
    out.push(RECT {
        left: x0,
        top: y0,
        right: x0 + master_w,
        bottom: y0 + h,
    });
    let sx = x0 + master_w + inner;
    let sc = (n - 1) as i32;
    let each = (h - (sc - 1) * inner) / sc;
    for i in 0..sc {
        let sy = y0 + i * (each + inner);
        let bottom = if i == sc - 1 { y0 + h } else { sy + each };
        out.push(RECT {
            left: sx,
            top: sy,
            right: sx + stack_w,
            bottom,
        });
    }
    out
}

/// The split ratio for level `i`, defaulting to 0.5 and clamped to a sane range.
fn split_ratio(splits: &[f32], i: usize) -> f32 {
    splits.get(i).copied().unwrap_or(0.5).clamp(0.05, 0.95)
}

/// Dwindle/spiral layout (Hyprland / omarchy default): each window takes a
/// fraction (`splits[i]`, default half) of the remaining space, alternating the
/// split along the longer side, so windows spiral toward the bottom corner.
/// Resizing a window edits the relevant `splits` entry (see `resize_dwindle`).
fn dwindle_layout(area: RECT, n: usize, outer: i32, inner: i32, splits: &[f32]) -> Vec<RECT> {
    let mut out = Vec::with_capacity(n);
    if n == 0 {
        return out;
    }
    let mut cur = RECT {
        left: area.left + outer,
        top: area.top + outer,
        right: area.right - outer,
        bottom: area.bottom - outer,
    };
    if cur.right <= cur.left || cur.bottom <= cur.top {
        return out;
    }
    for i in 0..n {
        if i == n - 1 {
            out.push(cur);
            break;
        }
        let w = cur.right - cur.left;
        let h = cur.bottom - cur.top;
        let r = split_ratio(splits, i);
        if w >= h {
            let half = ((w - inner) as f32 * r) as i32;
            out.push(RECT {
                left: cur.left,
                top: cur.top,
                right: cur.left + half,
                bottom: cur.bottom,
            });
            cur.left += half + inner;
        } else {
            let half = ((h - inner) as f32 * r) as i32;
            out.push(RECT {
                left: cur.left,
                top: cur.top,
                right: cur.right,
                bottom: cur.top + half,
            });
            cur.top += half + inner;
        }
    }
    out
}

/// Update `splits` so the dwindle window at tiled index `idx` matches the size
/// the user dragged it to (`new`). Replays the cascade to find that window's
/// split level + axis, then back-computes the ratio. Neighbours reflow to fill.
fn resize_dwindle(
    splits: &mut Vec<f32>,
    area: RECT,
    n: usize,
    outer: i32,
    inner: i32,
    idx: usize,
    new: RECT,
) {
    if n < 2 {
        return;
    }
    // The window at idx owns split level idx (it takes the first part); the very
    // last window instead shares level n-2 (it is that split's remainder).
    let (level, is_remainder) = if idx < n - 1 {
        (idx, false)
    } else {
        (n - 2, true)
    };
    if splits.len() < n - 1 {
        splits.resize(n - 1, 0.5);
    }
    // Replay the cascade up to `level` to find that split's available rect.
    let mut cur = RECT {
        left: area.left + outer,
        top: area.top + outer,
        right: area.right - outer,
        bottom: area.bottom - outer,
    };
    for i in 0..level {
        let w = cur.right - cur.left;
        let h = cur.bottom - cur.top;
        let r = split_ratio(splits, i);
        if w >= h {
            let half = ((w - inner) as f32 * r) as i32;
            cur.left += half + inner;
        } else {
            let half = ((h - inner) as f32 * r) as i32;
            cur.top += half + inner;
        }
    }
    let w = cur.right - cur.left;
    let h = cur.bottom - cur.top;
    let vertical = w >= h;
    let avail = (if vertical { w } else { h } - inner).max(1) as f32;
    let new_size = if vertical {
        new.right - new.left
    } else {
        new.bottom - new.top
    } as f32;
    // First-half window: ratio = its size / available. Remainder window: it gets
    // (1 - ratio), so ratio = 1 - its size / available.
    let ratio = if is_remainder {
        1.0 - new_size / avail
    } else {
        new_size / avail
    };
    splits[level] = ratio.clamp(0.05, 0.95);
}

/// Enumerate physical monitors, sorted left-to-right (0 = leftmost), each with
/// its own fresh set of workspaces.
unsafe extern "system" fn monitor_enum_proc(
    hmon: HMONITOR,
    _hdc: HDC,
    _rc: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let v = &mut *(lparam.0 as *mut Vec<(isize, i32, RECT)>);
    let mut mi = MONITORINFO {
        cbSize: core::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(hmon, &mut mi).as_bool() {
        v.push((hmon.0 as isize, mi.rcMonitor.left, mi.rcWork));
    }
    BOOL(1)
}

unsafe fn enumerate_monitors() -> Vec<Monitor> {
    let mut raw: Vec<(isize, i32, RECT)> = Vec::new();
    let _ = EnumDisplayMonitors(
        None,
        None,
        Some(monitor_enum_proc),
        LPARAM(&mut raw as *mut _ as isize),
    );
    if raw.is_empty() {
        raw.push((0, 0, work_area_at(POINT { x: 0, y: 0 })));
    }
    raw.sort_by_key(|m| m.1); // left-to-right
    // One placeholder workspace each; distribute_workspaces sets the real counts.
    raw.into_iter()
        .map(|(h, _, wa)| Monitor::new(h, wa, 1))
        .collect()
}

/// Index of the primary (main) monitor — the one containing the origin (0,0).
unsafe fn primary_index(monitors: &[Monitor]) -> usize {
    let hmon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTONEAREST).0 as isize;
    monitors.iter().position(|m| m.hmon == hmon).unwrap_or(0)
}

/// Set each monitor's workspace count. In `per_monitor` mode every monitor gets
/// `total` workspaces; in shared mode `total` is the GLOBAL number, distributed
/// round-robin from the primary monitor outward (so it's a total, not per-screen).
/// Existing workspaces (and their windows) are preserved.
fn distribute_workspaces(monitors: &mut [Monitor], primary: usize, total: usize, per_monitor: bool) {
    let n = monitors.len().max(1);
    for (idx, m) in monitors.iter_mut().enumerate() {
        let count = if per_monitor {
            total
        } else {
            let off = (idx + n - primary % n) % n;
            if off >= total {
                0
            } else {
                (total - 1 - off) / n + 1
            }
        }
        .max(1);
        while m.workspaces.len() < count {
            m.workspaces.push(Workspace::new());
        }
        // Shrinking: don't lose windows on removed workspaces — fold them into
        // the first workspace so they stay managed.
        while m.workspaces.len() > count {
            let extra = m.workspaces.pop().unwrap();
            m.workspaces[0].windows.extend(extra.windows);
            m.workspaces[0].floating.extend(extra.floating);
        }
        if m.active >= m.workspaces.len() {
            m.active = 0;
        }
    }
}

/// Recompute every monitor's tiling work area from its base (taskbar-excluded)
/// area, leaving room for the status bar so tiled windows never sit under it.
/// Idempotent — safe to call again on config reload.
unsafe fn reserve_bar(monitors: &mut [Monitor], cfg: &Config) {
    for m in monitors.iter_mut() {
        m.work_area = m.base_work;
        if cfg.bar_enabled && cfg.bar_height > 0 {
            if cfg.bar_bottom {
                m.work_area.bottom -= cfg.bar_height;
            } else {
                m.work_area.top += cfg.bar_height;
            }
        }
    }
}

/// Resolve which managed monitor a window currently sits on.
unsafe fn monitor_index_for_window(mgr: &Manager, hwnd: HWND) -> usize {
    let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST).0 as isize;
    mgr.mon_by_hmon(hmon)
        .unwrap_or_else(|| mgr.focused_mon.min(mgr.monitors.len().saturating_sub(1)))
}

/// Resolve which managed monitor contains a screen point.
unsafe fn monitor_index_for_point(mgr: &Manager, pt: POINT) -> usize {
    let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST).0 as isize;
    mgr.mon_by_hmon(hmon)
        .unwrap_or_else(|| mgr.focused_mon.min(mgr.monitors.len().saturating_sub(1)))
}

/// The tiled (non-floating) window on monitor `mi`'s active workspace whose
/// current rectangle contains `pt`, ignoring `exclude`.
unsafe fn window_under_point(mgr: &Manager, mi: usize, pt: POINT, exclude: isize) -> Option<isize> {
    let a = mgr.monitors[mi].active;
    let ws = &mgr.monitors[mi].workspaces[a];
    for &w in &ws.windows {
        if w == exclude || ws.floating.contains(&w) {
            continue;
        }
        let mut r = RECT::default();
        if GetWindowRect(hwnd_from(w), &mut r).is_ok()
            && pt.x >= r.left
            && pt.x < r.right
            && pt.y >= r.top
            && pt.y < r.bottom
        {
            return Some(w);
        }
    }
    None
}

/// Launch an external program detached. Routed through `cmd /C start` so PATH
/// and App Execution Aliases (e.g. wt.exe) resolve like they do from the shell.
fn launch(cmd: &str) {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return;
    }
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", cmd])
        .spawn();
}

/// Reveal every tracked window (so nothing is left hidden on another workspace)
/// and undo suprland's styling — but leave every window exactly where it is, so
/// quitting doesn't disturb the current layout.
unsafe fn restore_all_windows() {
    SUPPRESS.store(true, Ordering::Relaxed);
    let list = MANAGED.lock().unwrap().clone();
    for h in list {
        let hwnd = hwnd_from(h);
        if !IsWindow(hwnd).as_bool() {
            continue;
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        // Undo any dimming and restore the default border. Positions untouched.
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        let def: u32 = 0xFFFFFFFF; // DWMWA_COLOR_DEFAULT
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &def as *const _ as *const c_void,
            core::mem::size_of::<u32>() as u32,
        );
    }
    SUPPRESS.store(false, Ordering::Relaxed);
}

/// Console control handler: on Ctrl+C / window-close / logoff, un-hide every
/// managed window before the process dies so the user never loses them.
unsafe extern "system" fn console_handler(_ctrl_type: u32) -> BOOL {
    restore_all_windows();
    BOOL(0) // not fully handled — let the default handler terminate us
}

/// Move one window to a target rect. Restores minimized/maximized windows first
/// (they can't be repositioned otherwise) and compensates the DWM shadow border
/// so the visible edges sit flush. Individual SetWindowPos — robust per komorebi.
unsafe fn position_window(hwnd: HWND, target: RECT) {
    if IsIconic(hwnd).as_bool() || IsZoomed(hwnd).as_bool() {
        let _ = ShowWindow(hwnd, SW_RESTORE);
    }
    let r = adjust_for_border(hwnd, target);
    let _ = SetWindowPos(
        hwnd,
        None,
        r.left,
        r.top,
        r.right - r.left,
        r.bottom - r.top,
        SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSENDCHANGING,
    );
}

/// Tile a single monitor's active workspace on that monitor's work area.
unsafe fn retile_monitor(mgr: &Manager, mi: usize) {
    if !mgr.tiling || mi >= mgr.monitors.len() {
        return;
    }
    let mon = &mgr.monitors[mi];
    let ws = &mon.workspaces[mon.active];
    let tiled: Vec<isize> = ws
        .windows
        .iter()
        .copied()
        .filter(|h| !ws.floating.contains(h) && !IsIconic(hwnd_from(*h)).as_bool())
        .collect();
    let n = tiled.len();
    if n == 0 {
        return;
    }
    let rects = if mgr.cfg.layout == "master" {
        master_stack(
            mon.work_area,
            n,
            mgr.cfg.master_ratio,
            mgr.cfg.outer_gap,
            mgr.cfg.inner_gap,
        )
    } else {
        dwindle_layout(
            mon.work_area,
            n,
            mgr.cfg.outer_gap,
            mgr.cfg.inner_gap,
            &ws.splits,
        )
    };
    if rects.len() < n {
        return;
    }
    SUPPRESS.store(true, Ordering::Relaxed);
    for (i, h) in tiled.iter().enumerate() {
        position_window(hwnd_from(*h), rects[i]);
    }
    SUPPRESS.store(false, Ordering::Relaxed);
}

/// Tile every monitor's active workspace.
unsafe fn retile_all(mgr: &Manager) {
    for mi in 0..mgr.monitors.len() {
        retile_monitor(mgr, mi);
    }
}

/// Apply opacity + border colour to a single window based on focus state.
unsafe fn style_window(hwnd: HWND, focused: bool, cfg: &Config) {
    if cfg.unfocused_opacity < 0.999 {
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex & WS_EX_LAYERED.0 == 0 {
            SetWindowLongW(hwnd, GWL_EXSTYLE, (ex | WS_EX_LAYERED.0) as i32);
        }
        let alpha = if focused {
            255
        } else {
            (cfg.unfocused_opacity * 255.0) as u8
        };
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA);
    }
    if cfg.border_enabled {
        let color = COLORREF(if focused {
            cfg.focused_border
        } else {
            cfg.unfocused_border
        });
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &color as *const _ as *const c_void,
            core::mem::size_of::<COLORREF>() as u32,
        );
    }
}

/// The window currently styled as focused, so a focus change only has to touch
/// the two windows whose state actually flipped instead of every window.
static STYLED_FOCUS: AtomicIsize = AtomicIsize::new(0);

/// Compute the globally-focused window handle (0 if none).
fn global_focus(mgr: &Manager) -> isize {
    if mgr.monitors.is_empty() {
        return 0;
    }
    let fm = mgr.focused_mon.min(mgr.monitors.len() - 1);
    let fa = mgr.monitors[fm].active;
    mgr.monitors[fm].workspaces[fa].focused
}

/// Style every managed window from scratch — used once at startup. After that
/// `apply_styles` keeps things current by touching only what changed.
unsafe fn style_all(mgr: &Manager) {
    let focused_h = global_focus(mgr);
    STYLED_FOCUS.store(focused_h, Ordering::Relaxed);
    for m in &mgr.monitors {
        for ws in &m.workspaces {
            for &h in &ws.windows {
                style_window(hwnd_from(h), h != 0 && h == focused_h, &mgr.cfg);
            }
        }
    }
}

/// Keep focus highlighting current. `style_window` makes cross-process DWM
/// border + layered-alpha calls, so doing it for every window after every
/// command was the dominant cost. Focus highlight only changes for at most two
/// windows (the one losing focus and the one gaining it), so restyle exactly
/// those. Newly-added windows always become the focused one (see Cmd::Add), so
/// they get styled here too — nothing is left unstyled.
unsafe fn apply_styles(mgr: &Manager) {
    let focused_h = global_focus(mgr);
    let prev = STYLED_FOCUS.swap(focused_h, Ordering::Relaxed);
    if prev == focused_h {
        return;
    }
    if prev != 0 && IsWindow(hwnd_from(prev)).as_bool() {
        style_window(hwnd_from(prev), false, &mgr.cfg);
    }
    if focused_h != 0 {
        style_window(hwnd_from(focused_h), true, &mgr.cfg);
    }
}

/// Warp the mouse cursor to the centre of a window.
unsafe fn center_cursor_on(h: isize) {
    let mut r = RECT::default();
    if GetWindowRect(hwnd_from(h), &mut r).is_ok() {
        let _ = SetCursorPos((r.left + r.right) / 2, (r.top + r.bottom) / 2);
    }
}

#[inline]
fn rect_center(r: RECT) -> (i32, i32) {
    ((r.left + r.right) / 2, (r.top + r.bottom) / 2)
}

/// From `items[from]`, pick the nearest other window lying in direction `dir`.
fn pick_directional(items: &[(isize, RECT)], from: usize, dir: Dir) -> Option<usize> {
    let (cx, cy) = rect_center(items[from].1);
    let mut best = None;
    let mut best_score = i64::MAX;
    for (i, (_, r)) in items.iter().enumerate() {
        if i == from {
            continue;
        }
        let (ox, oy) = rect_center(*r);
        let (primary, secondary, valid) = match dir {
            Dir::Left => ((cx - ox) as i64, (cy - oy).unsigned_abs() as i64, ox < cx),
            Dir::Right => ((ox - cx) as i64, (cy - oy).unsigned_abs() as i64, ox > cx),
            Dir::Up => ((cy - oy) as i64, (cx - ox).unsigned_abs() as i64, oy < cy),
            Dir::Down => ((oy - cy) as i64, (cx - ox).unsigned_abs() as i64, oy > cy),
        };
        if !valid || primary <= 0 {
            continue;
        }
        let score = primary + secondary * 2;
        if score < best_score {
            best_score = score;
            best = Some(i);
        }
    }
    best
}

/// Collect the active workspace's windows with their current rectangles.
unsafe fn active_window_rects(mgr: &Manager, mi: usize) -> Vec<(isize, RECT)> {
    let a = mgr.monitors[mi].active;
    let mut items = Vec::new();
    for &h in &mgr.monitors[mi].workspaces[a].windows {
        let mut r = RECT::default();
        if GetWindowRect(hwnd_from(h), &mut r).is_ok() {
            items.push((h, r));
        }
    }
    items
}

/// The monitor to the left/right of `mi` (monitors are ordered left-to-right).
/// Vertical directions have no neighbour in this layout.
fn adjacent_monitor(mgr: &Manager, mi: usize, dir: Dir) -> Option<usize> {
    match dir {
        Dir::Left if mi > 0 => Some(mi - 1),
        Dir::Right if mi + 1 < mgr.monitors.len() => Some(mi + 1),
        _ => None,
    }
}

/// Best-effort focus that defeats the Windows foreground lock by briefly
/// attaching to the current foreground thread's input queue.
unsafe fn focus_window(h: isize) {
    if h == 0 {
        return;
    }
    let hwnd = hwnd_from(h);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let fg = GetForegroundWindow();
    let cur = GetCurrentThreadId();
    let fgt = GetWindowThreadProcessId(fg, None);
    if fgt != 0 && fgt != cur {
        let _ = AttachThreadInput(cur, fgt, BOOL(1));
        let _ = SetForegroundWindow(hwnd);
        let _ = BringWindowToTop(hwnd);
        let _ = AttachThreadInput(cur, fgt, BOOL(0));
    } else {
        let _ = SetForegroundWindow(hwnd);
        let _ = BringWindowToTop(hwnd);
    }
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let v = &mut *(lparam.0 as *mut Vec<isize>);
    if is_manageable(hwnd) {
        v.push(hwnd.0 as isize);
    }
    BOOL(1)
}

/// Add every currently-manageable window to its monitor's active workspace.
unsafe fn assign_existing_windows(mgr: &mut Manager) {
    let mut v: Vec<isize> = Vec::new();
    let _ = EnumWindows(Some(enum_proc), LPARAM(&mut v as *mut Vec<isize> as isize));
    for h in v {
        if mgr.locate(h).is_some() {
            continue;
        }
        let mi = monitor_index_for_window(mgr, hwnd_from(h));
        let a = mgr.monitors[mi].active;
        mgr.monitors[mi].workspaces[a].windows.push(h);
        if should_float(hwnd_from(h)) {
            mgr.monitors[mi].workspaces[a].floating.push(h);
        }
        mgr.monitors[mi].workspaces[a].focused = h;
    }
}

/// Switch one monitor to workspace `n`: hide the old set, reveal the new,
/// retile, then focus. Workspaces are never cleared — only shown/hidden.
unsafe fn switch_monitor_workspace(mgr: &mut Manager, mi: usize, n: usize) {
    if mi >= mgr.monitors.len() {
        return;
    }
    let old = mgr.monitors[mi].active;
    if n == old || n >= mgr.monitors[mi].workspaces.len() {
        return;
    }
    SUPPRESS.store(true, Ordering::Relaxed);
    for h in mgr.monitors[mi].workspaces[old].windows.clone() {
        let _ = ShowWindow(hwnd_from(h), SW_HIDE);
    }
    mgr.monitors[mi].active = n;
    for h in mgr.monitors[mi].workspaces[n].windows.clone() {
        let _ = ShowWindow(hwnd_from(h), SW_SHOW);
    }
    SUPPRESS.store(false, Ordering::Relaxed);
    retile_monitor(mgr, mi);
    let f = mgr.monitors[mi].workspaces[n].focused;
    let f = if f != 0 {
        f
    } else {
        mgr.monitors[mi].workspaces[n]
            .windows
            .first()
            .copied()
            .unwrap_or(0)
    };
    mgr.monitors[mi].workspaces[n].focused = f;
    if f != 0 {
        focus_window(f);
        if mgr.cfg.cursor_follows_focus {
            center_cursor_on(f);
        }
    } else if mgr.cfg.cursor_follows_focus {
        // Empty workspace: park the cursor on that monitor so focus is there.
        let wa = mgr.monitors[mi].work_area;
        let _ = SetCursorPos((wa.left + wa.right) / 2, (wa.top + wa.bottom) / 2);
    }
}

/// Re-enumerate monitors after a display change. Preserves each surviving
/// monitor's active workspace and re-homes tracked windows, keeping their
/// workspace index when the monitor still exists.
unsafe fn refresh_monitors(mgr: &mut Manager) {
    let mut tracked: Vec<(isize, usize, isize)> = Vec::new(); // (old hmon, wi, hwnd)
    let mut old_active: Vec<(isize, usize)> = Vec::new();
    for mon in &mgr.monitors {
        old_active.push((mon.hmon, mon.active));
        for (wi, ws) in mon.workspaces.iter().enumerate() {
            for &h in &ws.windows {
                tracked.push((mon.hmon, wi, h));
            }
        }
    }
    let mut fresh = enumerate_monitors();
    let primary = primary_index(&fresh);
    distribute_workspaces(&mut fresh, primary, mgr.cfg.workspaces, mgr.cfg.per_monitor);
    for mon in fresh.iter_mut() {
        if let Some((_, a)) = old_active.iter().find(|(hm, _)| *hm == mon.hmon) {
            if *a < mon.workspaces.len() {
                mon.active = *a;
            }
        }
    }
    reserve_bar(&mut fresh, &mgr.cfg);
    mgr.monitors = fresh;
    mgr.primary = primary;
    for (old_hmon, wi, h) in tracked {
        if !is_manageable(hwnd_from(h)) {
            continue;
        }
        let mi = mgr
            .mon_by_hmon(old_hmon)
            .unwrap_or_else(|| monitor_index_for_window(mgr, hwnd_from(h)));
        let target_wi = if old_hmon == mgr.monitors[mi].hmon {
            wi.min(mgr.monitors[mi].workspaces.len() - 1)
        } else {
            mgr.monitors[mi].active
        };
        if !mgr.monitors[mi].workspaces[target_wi].windows.contains(&h) {
            mgr.monitors[mi].workspaces[target_wi].windows.push(h);
            if mgr.monitors[mi].workspaces[target_wi].focused == 0 {
                mgr.monitors[mi].workspaces[target_wi].focused = h;
            }
        }
    }
    if mgr.focused_mon >= mgr.monitors.len() {
        mgr.focused_mon = 0;
    }
    retile_all(mgr);
}

fn focused_index(ws: &Workspace) -> Option<usize> {
    if ws.windows.is_empty() {
        return None;
    }
    ws.windows
        .iter()
        .position(|&h| h == ws.focused)
        .or(Some(0))
}

unsafe fn process(mgr: &mut Manager, cmd: Cmd) {
    match cmd {
        Cmd::Add(h) => {
            if mgr.locate(h).is_none() && is_manageable(hwnd_from(h)) {
                let mi = monitor_index_for_window(mgr, hwnd_from(h));
                let a = mgr.monitors[mi].active;
                mgr.monitors[mi].workspaces[a].windows.push(h);
                if should_float(hwnd_from(h)) {
                    mgr.monitors[mi].workspaces[a].floating.push(h);
                }
                mgr.monitors[mi].workspaces[a].focused = h;
                mgr.focused_mon = mi;
                retile_monitor(mgr, mi);
            }
        }
        Cmd::Remove(h) => {
            if let Some((mi, wi)) = mgr.locate(h) {
                let ws = &mut mgr.monitors[mi].workspaces[wi];
                ws.windows.retain(|&x| x != h);
                ws.floating.retain(|&x| x != h);
                if ws.focused == h {
                    ws.focused = ws.windows.first().copied().unwrap_or(0);
                }
                if wi == mgr.monitors[mi].active {
                    retile_monitor(mgr, mi);
                }
            }
        }
        Cmd::Focused(h) => {
            if let Some((mi, wi)) = mgr.locate(h) {
                mgr.focused_mon = mi;
                if wi == mgr.monitors[mi].active {
                    mgr.monitors[mi].workspaces[wi].focused = h;
                }
            }
        }
        Cmd::FocusMouse(h) => {
            // Focus-follows-mouse: only act on a tracked window on a visible
            // workspace that isn't already the focused one.
            if let Some((mi, wi)) = mgr.locate(h) {
                if wi == mgr.monitors[mi].active
                    && !(mgr.focused_mon == mi && mgr.monitors[mi].workspaces[wi].focused == h)
                {
                    mgr.focused_mon = mi;
                    mgr.monitors[mi].workspaces[wi].focused = h;
                    focus_window(h);
                }
            }
        }
        Cmd::BarClick(hmon, local) => {
            if let Some(mi) = mgr.mon_by_hmon(hmon) {
                if local < mgr.monitors[mi].workspaces.len() {
                    mgr.focused_mon = mi;
                    if local != mgr.monitors[mi].active {
                        switch_monitor_workspace(mgr, mi, local);
                    } else {
                        let f = mgr.monitors[mi].workspaces[local].focused;
                        if f != 0 {
                            focus_window(f);
                        }
                    }
                }
            }
        }
        Cmd::Reload(cfg) => {
            mgr.cfg = *cfg;
            // Apply new workspace counts / mode, then recompute work areas for
            // the (possibly changed) bar height. Bars themselves are recreated
            // on the main thread (WM_RELOAD -> ensure_bars).
            distribute_workspaces(
                &mut mgr.monitors,
                mgr.primary,
                mgr.cfg.workspaces,
                mgr.cfg.per_monitor,
            );
            reserve_bar(&mut mgr.monitors, &mgr.cfg);
            // Reset every window's styling so disabling opacity/borders takes
            // effect, then re-apply from scratch.
            SUPPRESS.store(true, Ordering::Relaxed);
            for m in &mgr.monitors {
                for ws in &m.workspaces {
                    for &h in &ws.windows {
                        let hwnd = hwnd_from(h);
                        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
                        let def: u32 = 0xFFFFFFFF; // DWMWA_COLOR_DEFAULT
                        let _ = DwmSetWindowAttribute(
                            hwnd,
                            DWMWA_BORDER_COLOR,
                            &def as *const _ as *const c_void,
                            core::mem::size_of::<u32>() as u32,
                        );
                    }
                }
            }
            SUPPRESS.store(false, Ordering::Relaxed);
            STYLED_FOCUS.store(0, Ordering::Relaxed);
            retile_all(mgr);
            style_all(mgr);
        }
        Cmd::FocusDir(d) => {
            if !mgr.tiling {
                return;
            }
            let mi = mgr.focused_mon;
            let a = mgr.monitors[mi].active;
            if let Some(idx) = focused_index(&mgr.monitors[mi].workspaces[a]) {
                let ws = &mgr.monitors[mi].workspaces[a];
                let len = ws.windows.len() as i32;
                let ni = (idx as i32 + d).rem_euclid(len) as usize;
                let target = ws.windows[ni];
                mgr.monitors[mi].workspaces[a].focused = target;
                focus_window(target);
            }
        }
        Cmd::SwapDir(d) => {
            if !mgr.tiling {
                return;
            }
            let mi = mgr.focused_mon;
            let a = mgr.monitors[mi].active;
            let len = mgr.monitors[mi].workspaces[a].windows.len();
            if let Some(idx) = focused_index(&mgr.monitors[mi].workspaces[a]) {
                if len > 1 {
                    let ni = (idx as i32 + d).rem_euclid(len as i32) as usize;
                    mgr.monitors[mi].workspaces[a].windows.swap(idx, ni);
                    retile_monitor(mgr, mi);
                }
            }
        }
        Cmd::PromoteMaster => {
            if !mgr.tiling {
                return;
            }
            let mi = mgr.focused_mon;
            let a = mgr.monitors[mi].active;
            if let Some(idx) = focused_index(&mgr.monitors[mi].workspaces[a]) {
                if idx != 0 {
                    mgr.monitors[mi].workspaces[a].windows.swap(0, idx);
                    retile_monitor(mgr, mi);
                }
            }
        }
        Cmd::ResizeMaster(delta) => {
            if !mgr.tiling {
                return;
            }
            mgr.cfg.master_ratio = (mgr.cfg.master_ratio + delta).clamp(0.15, 0.85);
            retile_monitor(mgr, mgr.focused_mon);
        }
        Cmd::Switch(i) => {
            if i >= mgr.cfg.workspaces || mgr.monitors.is_empty() {
                return;
            }
            let (mi, local) = mgr.global_to_ml(i);
            if mi >= mgr.monitors.len() || local >= mgr.monitors[mi].workspaces.len() {
                return;
            }
            mgr.focused_mon = mi;
            if local != mgr.monitors[mi].active {
                // Shows the workspace, retiles, focuses + warps the cursor.
                switch_monitor_workspace(mgr, mi, local);
            } else {
                // Already showing it: move focus (and cursor) to that monitor.
                let f = mgr.monitors[mi].workspaces[local].focused;
                if f != 0 {
                    focus_window(f);
                    if mgr.cfg.cursor_follows_focus {
                        center_cursor_on(f);
                    }
                } else if mgr.cfg.cursor_follows_focus {
                    let wa = mgr.monitors[mi].work_area;
                    let _ = SetCursorPos((wa.left + wa.right) / 2, (wa.top + wa.bottom) / 2);
                }
            }
        }
        Cmd::MoveToWs(i) => {
            if i >= mgr.cfg.workspaces || !mgr.tiling || mgr.monitors.is_empty() {
                return;
            }
            let from_mi = mgr.focused_mon;
            let from_a = mgr.monitors[from_mi].active;
            let h = mgr.monitors[from_mi].workspaces[from_a].focused;
            if h == 0 {
                return;
            }
            let (to_mi, to_local) = mgr.global_to_ml(i);
            if to_mi >= mgr.monitors.len() || to_local >= mgr.monitors[to_mi].workspaces.len() {
                return;
            }
            if to_mi == from_mi && to_local == from_a {
                return;
            }
            {
                let ws = &mut mgr.monitors[from_mi].workspaces[from_a];
                ws.windows.retain(|&x| x != h);
                ws.floating.retain(|&x| x != h);
                ws.focused = ws.windows.first().copied().unwrap_or(0);
            }
            mgr.monitors[to_mi].workspaces[to_local].windows.push(h);
            mgr.monitors[to_mi].workspaces[to_local].focused = h;
            retile_monitor(mgr, from_mi);
            // Follow the window: show its destination workspace, focus it, warp.
            mgr.focused_mon = to_mi;
            if to_local != mgr.monitors[to_mi].active {
                switch_monitor_workspace(mgr, to_mi, to_local);
            } else {
                retile_monitor(mgr, to_mi);
                focus_window(h);
                if mgr.cfg.cursor_follows_focus {
                    center_cursor_on(h);
                }
            }
        }
        Cmd::ToggleTiling => {
            // Flip tiling only. Workspaces stay intact so Alt+1..9 keeps working
            // whether tiling is on or off; turning it back on re-applies layout.
            mgr.tiling = !mgr.tiling;
            if mgr.tiling {
                retile_all(mgr);
                let mi = mgr.focused_mon;
                let a = mgr.monitors[mi].active;
                let f = mgr.monitors[mi].workspaces[a].focused;
                if f != 0 {
                    focus_window(f);
                }
            }
        }
        Cmd::ToggleFloat => {
            if !mgr.tiling {
                return;
            }
            let mi = mgr.focused_mon;
            let a = mgr.monitors[mi].active;
            let h = mgr.monitors[mi].workspaces[a].focused;
            if h == 0 {
                return;
            }
            let ws = &mut mgr.monitors[mi].workspaces[a];
            if let Some(p) = ws.floating.iter().position(|&x| x == h) {
                ws.floating.remove(p);
            } else {
                ws.floating.push(h);
            }
            retile_monitor(mgr, mi);
        }
        Cmd::CloseFocused => {
            let mi = mgr.focused_mon;
            let a = mgr.monitors[mi].active;
            let h = mgr.monitors[mi].workspaces[a].focused;
            if h != 0 {
                let _ = PostMessageW(hwnd_from(h), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        Cmd::Retile => retile_all(mgr),
        Cmd::RefreshMonitors => refresh_monitors(mgr),
        Cmd::DragMoved(h, x, y) => {
            if !mgr.tiling {
                return;
            }
            let Some((from_mi, from_wi)) = mgr.locate(h) else {
                return;
            };
            // Floating windows are left wherever the user dropped them.
            if mgr.monitors[from_mi].workspaces[from_wi].floating.contains(&h) {
                return;
            }
            let from_a = mgr.monitors[from_mi].active;
            if from_wi != from_a {
                return;
            }
            let pt = POINT { x, y };
            let to_mi = monitor_index_for_point(mgr, pt);
            let target = window_under_point(mgr, to_mi, pt, h);
            if to_mi == from_mi {
                // Reorder within the same monitor: swap with the window dropped onto.
                if let Some(t) = target {
                    let ws = &mut mgr.monitors[to_mi].workspaces[from_a];
                    let ia = ws.windows.iter().position(|&w| w == h);
                    let ib = ws.windows.iter().position(|&w| w == t);
                    if let (Some(ia), Some(ib)) = (ia, ib) {
                        ws.windows.swap(ia, ib);
                    }
                }
                mgr.monitors[from_mi].workspaces[from_a].focused = h;
                retile_monitor(mgr, from_mi);
            } else {
                // Move the window to the monitor it was dropped on.
                {
                    let ws = &mut mgr.monitors[from_mi].workspaces[from_a];
                    ws.windows.retain(|&w| w != h);
                    ws.floating.retain(|&w| w != h);
                    ws.focused = ws.windows.first().copied().unwrap_or(0);
                }
                let to_a = mgr.monitors[to_mi].active;
                let ws = &mut mgr.monitors[to_mi].workspaces[to_a];
                match target.and_then(|t| ws.windows.iter().position(|&w| w == t)) {
                    Some(pos) => ws.windows.insert(pos, h),
                    None => ws.windows.push(h),
                }
                ws.focused = h;
                mgr.focused_mon = to_mi;
                retile_monitor(mgr, from_mi);
                retile_monitor(mgr, to_mi);
            }
            focus_window(h);
        }
        Cmd::DragResized(h) => {
            if !mgr.tiling {
                return;
            }
            let Some((mi, wi)) = mgr.locate(h) else {
                return;
            };
            if mgr.monitors[mi].workspaces[wi].floating.contains(&h)
                || wi != mgr.monitors[mi].active
            {
                return;
            }
            let mut r = RECT::default();
            if GetWindowRect(hwnd_from(h), &mut r).is_err() {
                retile_monitor(mgr, mi);
                return;
            }
            let wa = mgr.monitors[mi].work_area;
            // Tiled order must match what retile_monitor / dwindle_layout use.
            let tiled: Vec<isize> = mgr.monitors[mi].workspaces[wi]
                .windows
                .iter()
                .copied()
                .filter(|w| {
                    !mgr.monitors[mi].workspaces[wi].floating.contains(w)
                        && !IsIconic(hwnd_from(*w)).as_bool()
                })
                .collect();
            let n = tiled.len();
            if mgr.cfg.layout == "master" {
                // Master width sets the ratio; stack windows snap back.
                if tiled.first() == Some(&h) {
                    let total =
                        (wa.right - wa.left - 2 * mgr.cfg.outer_gap - mgr.cfg.inner_gap).max(1);
                    let mw = (r.right - r.left).max(1);
                    mgr.cfg.master_ratio = (mw as f32 / total as f32).clamp(0.15, 0.85);
                }
            } else if let Some(idx) = tiled.iter().position(|&w| w == h) {
                // Dwindle: edit the split ratio so neighbours reflow to fill.
                resize_dwindle(
                    &mut mgr.monitors[mi].workspaces[wi].splits,
                    wa,
                    n,
                    mgr.cfg.outer_gap,
                    mgr.cfg.inner_gap,
                    idx,
                    r,
                );
            }
            retile_monitor(mgr, mi);
        }
        Cmd::LaunchTerminal => launch(&mgr.cfg.terminal),
        Cmd::LaunchBrowser => {
            // Empty browser config = open the system default browser via http.
            if mgr.cfg.browser.trim().is_empty() {
                launch("http://");
            } else {
                launch(&mgr.cfg.browser);
            }
        }
        Cmd::FocusGeo(dir) => {
            if !mgr.tiling || mgr.monitors.is_empty() {
                return;
            }
            let mi = mgr.focused_mon;
            let a = mgr.monitors[mi].active;
            let cur = mgr.monitors[mi].workspaces[a].focused;
            let items = active_window_rects(mgr, mi);
            let from = items.iter().position(|(h, _)| *h == cur).unwrap_or(0);
            let picked = if items.is_empty() {
                None
            } else {
                pick_directional(&items, from, dir)
            };
            if let Some(ti) = picked {
                let target = items[ti].0;
                mgr.monitors[mi].workspaces[a].focused = target;
                focus_window(target);
                if mgr.cfg.cursor_follows_focus {
                    center_cursor_on(target);
                }
            } else if let Some(to_mi) = adjacent_monitor(mgr, mi, dir) {
                // No neighbour this way: jump focus to the adjacent monitor.
                mgr.focused_mon = to_mi;
                let ta = mgr.monitors[to_mi].active;
                let f = mgr.monitors[to_mi].workspaces[ta].focused;
                let f = if f != 0 {
                    f
                } else {
                    mgr.monitors[to_mi].workspaces[ta]
                        .windows
                        .first()
                        .copied()
                        .unwrap_or(0)
                };
                if f != 0 {
                    mgr.monitors[to_mi].workspaces[ta].focused = f;
                    focus_window(f);
                    if mgr.cfg.cursor_follows_focus {
                        center_cursor_on(f);
                    }
                }
            }
        }
        Cmd::MoveGeo(dir) => {
            if !mgr.tiling || mgr.monitors.is_empty() {
                return;
            }
            let mi = mgr.focused_mon;
            let a = mgr.monitors[mi].active;
            let h = mgr.monitors[mi].workspaces[a].focused;
            if h == 0 {
                return;
            }
            let items = active_window_rects(mgr, mi);
            let from = items.iter().position(|(w, _)| *w == h).unwrap_or(0);
            let picked = if items.is_empty() {
                None
            } else {
                pick_directional(&items, from, dir)
            };
            if let Some(ti) = picked {
                // Swap order with the neighbour in that direction.
                let target = items[ti].0;
                let ws = &mut mgr.monitors[mi].workspaces[a];
                let ia = ws.windows.iter().position(|&w| w == h);
                let ib = ws.windows.iter().position(|&w| w == target);
                if let (Some(ia), Some(ib)) = (ia, ib) {
                    ws.windows.swap(ia, ib);
                }
                retile_monitor(mgr, mi);
                if mgr.cfg.cursor_follows_focus {
                    center_cursor_on(h);
                }
            } else if let Some(to_mi) = adjacent_monitor(mgr, mi, dir) {
                // Move the window to the adjacent monitor's active workspace.
                {
                    let ws = &mut mgr.monitors[mi].workspaces[a];
                    ws.windows.retain(|&w| w != h);
                    ws.floating.retain(|&w| w != h);
                    ws.focused = ws.windows.first().copied().unwrap_or(0);
                }
                let ta = mgr.monitors[to_mi].active;
                mgr.monitors[to_mi].workspaces[ta].windows.push(h);
                mgr.monitors[to_mi].workspaces[ta].focused = h;
                mgr.focused_mon = to_mi;
                retile_monitor(mgr, mi);
                retile_monitor(mgr, to_mi);
                focus_window(h);
                if mgr.cfg.cursor_follows_focus {
                    center_cursor_on(h);
                }
            }
        }
    }
}

// =========================================================================
// Status bar (waybar-style): workspace pills + focused title + clock.
// =========================================================================

/// Read a window's title into a String.
unsafe fn window_title(h: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = GetWindowTextW(h, &mut buf);
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// EnumDisplayMonitors callback collecting (HMONITOR, full monitor rect).
unsafe extern "system" fn bar_mon_enum(
    hmon: HMONITOR,
    _hdc: HDC,
    _rc: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let v = &mut *(lparam.0 as *mut Vec<(isize, RECT)>);
    let mut mi = MONITORINFO {
        cbSize: core::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(hmon, &mut mi).as_bool() {
        v.push((hmon.0 as isize, mi.rcMonitor));
    }
    BOOL(1)
}

/// Build the shared bar font and pill-cell width. Call only on the main thread
/// (the bars' paint thread) so deleting the old font can't race a paint.
unsafe fn make_bar_font(height: i32, font_size: i32) {
    let size = if font_size > 0 {
        font_size
    } else {
        ((height as f32) * 0.5) as i32
    }
    .max(8);
    let f = CreateFontW(
        -size, // negative = character height (matches point-style sizing)
        0,
        0,
        0,
        600, // semi-bold
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        0, // DEFAULT_PITCH | FF_DONTCARE
        w!("Segoe UI"),
    );
    let prev = BAR_FONT.swap(f.0 as isize, Ordering::Relaxed);
    if prev != 0 {
        let _ = DeleteObject(HGDIOBJ(prev as *mut c_void));
    }
    BAR_CELL.store((height.max(8) as f32 * 1.25) as isize, Ordering::Relaxed);
}

/// Create or reposition one bar window per monitor. Safe to call repeatedly
/// (startup and on display changes); runs only on the main thread because the
/// bars' message loop is the main thread.
unsafe fn ensure_bars() {
    let height = BAR_HEIGHT.load(Ordering::Relaxed) as i32;
    if height <= 0 {
        return;
    }
    let bottom = BAR_BOTTOM.load(Ordering::Relaxed);
    let hinst = HINSTANCE(BAR_HINST.load(Ordering::Relaxed) as *mut c_void);

    let mut raw: Vec<(isize, RECT)> = Vec::new();
    let _ = EnumDisplayMonitors(
        None,
        None,
        Some(bar_mon_enum),
        LPARAM(&mut raw as *mut _ as isize),
    );

    let mut bars = BARS.lock().unwrap();
    for &(hmon, rcm) in &raw {
        let x = rcm.left;
        let y = if bottom { rcm.bottom - height } else { rcm.top };
        let w = rcm.right - rcm.left;
        if let Some(b) = bars.iter().find(|b| b.hmon == hmon) {
            let _ = SetWindowPos(
                hwnd_from(b.hwnd),
                HWND_TOPMOST,
                x,
                y,
                w,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        } else {
            let hb = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                w!("suprland_bar"),
                w!(""),
                WS_POPUP,
                x,
                y,
                w,
                height,
                None,
                None,
                hinst,
                None,
            )
            .expect("bar window failed");
            SetWindowLongPtrW(hb, GWLP_USERDATA, hmon);
            let _ = ShowWindow(hb, SW_SHOW);
            SetTimer(hb, BAR_TIMER_ID, 1000, None);
            bars.push(BarWin {
                hwnd: hb.0 as isize,
                hmon,
            });
        }
    }
    // Hide bars whose monitor disappeared.
    let present: Vec<isize> = raw.iter().map(|(h, _)| *h).collect();
    for b in bars.iter() {
        if !present.contains(&b.hmon) {
            let _ = ShowWindow(hwnd_from(b.hwnd), SW_HIDE);
        }
    }
}

/// Convert a 24-hour hour to (12-hour, "am"/"pm").
fn to_12h(h: u16) -> (u16, &'static str) {
    let ap = if h < 12 { "am" } else { "pm" };
    let mut h12 = h % 12;
    if h12 == 0 {
        h12 = 12;
    }
    (h12, ap)
}

/// Rebuild the per-monitor bar snapshot and repaint only the bars that changed.
/// The clock is refreshed separately by each bar's 1s timer, so an idle desktop
/// causes no repaints from here.
unsafe fn update_bar(mgr: &Manager) {
    if BARS.lock().unwrap().is_empty() {
        return;
    }
    let count = mgr.cfg.workspaces;
    let mut mons = Vec::with_capacity(mgr.monitors.len());
    for (mi, m) in mgr.monitors.iter().enumerate() {
        // Pill numbers: per_monitor shows 1..count; shared shows this monitor's
        // slice of the global numbering, which starts at the primary monitor.
        let labels: Vec<usize> = (0..count)
            .map(|local| {
                if mgr.cfg.per_monitor {
                    local + 1
                } else {
                    mgr.ml_to_global(mi, local) + 1
                }
            })
            .collect();
        let mut occupied: u64 = 0;
        for local in 0..count {
            if m
                .workspaces
                .get(local)
                .is_some_and(|ws| !ws.windows.is_empty())
            {
                occupied |= 1 << local;
            }
        }
        let active = m.active.min(count.saturating_sub(1));
        let fh = m.workspaces.get(m.active).map(|ws| ws.focused).unwrap_or(0);
        let title = if fh != 0 {
            window_title(hwnd_from(fh))
        } else {
            String::new()
        };
        mons.push(MonBar {
            hmon: m.hmon,
            labels,
            active,
            occupied,
            title,
        });
    }
    let new = BarData {
        bg: mgr.cfg.bar_bg,
        fg: mgr.cfg.bar_fg,
        accent: mgr.cfg.bar_accent,
        inactive: mgr.cfg.bar_inactive,
        show_title: mgr.cfg.bar_show_title,
        show_clock: mgr.cfg.bar_show_clock,
        clock_24h: mgr.cfg.bar_clock_24h,
        show_layout: mgr.cfg.bar_show_layout,
        layout: mgr.cfg.layout.clone(),
        tiling: mgr.tiling,
        mons,
    };

    // Diff against the previous snapshot so only changed monitors repaint.
    let mut changed: Vec<isize> = Vec::new();
    {
        let old = BAR.lock().unwrap();
        let global_changed = old.bg != new.bg
            || old.fg != new.fg
            || old.accent != new.accent
            || old.inactive != new.inactive
            || old.show_title != new.show_title
            || old.show_clock != new.show_clock
            || old.clock_24h != new.clock_24h
            || old.show_layout != new.show_layout
            || old.layout != new.layout
            || old.tiling != new.tiling
            || old.mons.len() != new.mons.len();
        for nm in &new.mons {
            let diff = match old.mons.iter().find(|om| om.hmon == nm.hmon) {
                Some(om) => om != nm,
                None => true,
            };
            if global_changed || diff {
                changed.push(nm.hmon);
            }
        }
    }
    *BAR.lock().unwrap() = new;
    if changed.is_empty() {
        return;
    }
    let bars = BARS.lock().unwrap().clone();
    for b in bars {
        if changed.contains(&b.hmon) {
            let _ = PostMessageW(hwnd_from(b.hwnd), WM_BAR_REFRESH, WPARAM(0), LPARAM(0));
        }
    }
}

/// Paint one monitor's bar: workspace pills (left), focused title (centre),
/// layout/tiling state + clock (right). The owning monitor's HMONITOR is stored
/// in GWLP_USERDATA so each bar paints its own data.
unsafe fn paint_bar(h: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(h, &mut ps);
    let hmon = GetWindowLongPtrW(h, GWLP_USERDATA);
    let data = BAR.lock().unwrap().clone();

    let mut rc = RECT::default();
    let _ = GetClientRect(h, &mut rc);
    let h_px = rc.bottom - rc.top;

    let bg_brush = CreateSolidBrush(COLORREF(data.bg));
    FillRect(hdc, &rc, bg_brush);
    let _ = DeleteObject(HGDIOBJ(bg_brush.0));

    let font_raw = BAR_FONT.load(Ordering::Relaxed);
    let old_font = if font_raw != 0 {
        Some(SelectObject(hdc, HGDIOBJ(font_raw as *mut c_void)))
    } else {
        Some(SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT)))
    };
    SetBkMode(hdc, TRANSPARENT);

    let cell = BAR_CELL.load(Ordering::Relaxed) as i32;
    let mut right_edge = rc.right;

    // Clock (rightmost).
    if data.show_clock {
        let st: SYSTEMTIME = GetLocalTime();
        let clock = if data.clock_24h {
            format!("{:02}:{:02}", st.wHour, st.wMinute)
        } else {
            let (h12, ap) = to_12h(st.wHour);
            format!("{}:{:02} {}", h12, st.wMinute, ap)
        };
        let mut cs: Vec<u16> = clock.encode_utf16().collect();
        let mut clk = RECT {
            left: right_edge - 96,
            top: 0,
            right: right_edge - 12,
            bottom: h_px,
        };
        SetTextColor(hdc, COLORREF(data.fg));
        DrawTextW(
            hdc,
            &mut cs,
            &mut clk,
            DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        right_edge -= 100;
    }

    // Layout / tiling state.
    if data.show_layout {
        let s = if data.tiling {
            format!("[{}]", data.layout)
        } else {
            "[float]".to_string()
        };
        let mut sv: Vec<u16> = s.encode_utf16().collect();
        let mut lr = RECT {
            left: right_edge - 116,
            top: 0,
            right: right_edge - 8,
            bottom: h_px,
        };
        SetTextColor(hdc, COLORREF(data.inactive));
        DrawTextW(
            hdc,
            &mut sv,
            &mut lr,
            DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        right_edge -= 124;
    }

    if let Some(mb) = data.mons.iter().find(|m| m.hmon == hmon) {
        // Workspace pills.
        for (i, label) in mb.labels.iter().enumerate() {
            let x0 = i as i32 * cell;
            let mut cr = RECT {
                left: x0,
                top: 0,
                right: x0 + cell,
                bottom: h_px,
            };
            let occ = mb.occupied & (1 << i) != 0;
            if i == mb.active {
                // Inset fill so the highlight reads as a pill, not a full block.
                let pad = (h_px / 6).clamp(2, 6);
                let pill = RECT {
                    left: x0 + 3,
                    top: pad,
                    right: x0 + cell - 3,
                    bottom: h_px - pad,
                };
                let ab = CreateSolidBrush(COLORREF(data.accent));
                FillRect(hdc, &pill, ab);
                let _ = DeleteObject(HGDIOBJ(ab.0));
                SetTextColor(hdc, COLORREF(data.bg));
            } else {
                SetTextColor(hdc, COLORREF(if occ { data.fg } else { data.inactive }));
            }
            let mut s: Vec<u16> = format!("{}", label).encode_utf16().collect();
            DrawTextW(
                hdc,
                &mut s,
                &mut cr,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        }

        // Focused window title (between pills and the right cluster).
        if data.show_title && !mb.title.is_empty() {
            let tx = mb.labels.len() as i32 * cell + 14;
            let r = right_edge - 8;
            if r > tx {
                let mut tr = RECT {
                    left: tx,
                    top: 0,
                    right: r,
                    bottom: h_px,
                };
                SetTextColor(hdc, COLORREF(data.fg));
                let mut s: Vec<u16> = mb.title.encode_utf16().collect();
                DrawTextW(
                    hdc,
                    &mut s,
                    &mut tr,
                    DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
                );
            }
        }
    }

    if let Some(of) = old_font {
        SelectObject(hdc, of);
    }
    let _ = EndPaint(h, &ps);
}

/// Bar WndProc: paints on demand, ticks the clock, and switches that monitor's
/// workspace when a pill is clicked.
unsafe extern "system" fn bar_wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint_bar(h);
            LRESULT(0)
        }
        WM_BAR_REFRESH | WM_TIMER => {
            let _ = InvalidateRect(h, None, BOOL(0));
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (l.0 as u32 & 0xFFFF) as i16 as i32;
            let cell = BAR_CELL.load(Ordering::Relaxed) as i32;
            let hmon = GetWindowLongPtrW(h, GWLP_USERDATA);
            if cell > 0 {
                let i = (x / cell) as usize;
                let count = BAR
                    .lock()
                    .unwrap()
                    .mons
                    .iter()
                    .find(|m| m.hmon == hmon)
                    .map(|m| m.labels.len())
                    .unwrap_or(0);
                if i < count {
                    push_cmd(Cmd::BarClick(hmon, i));
                }
            }
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            push_cmd(Cmd::RefreshMonitors);
            DefWindowProcW(h, msg, w, l)
        }
        _ => DefWindowProcW(h, msg, w, l),
    }
}

/// Focus-follows-mouse poll loop. Polls the cursor instead of running in the
/// low-level mouse hook so it never adds latency to the global input path. Only
/// active while `focus_follows_mouse` is enabled and no drag/Alt/button is busy.
fn focus_follow_worker() {
    let mut last: isize = 0;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(80));
        if !FOLLOW_MOUSE.load(Ordering::Relaxed) {
            last = 0;
            continue;
        }
        unsafe {
            if ANY_DRAG.load(Ordering::Relaxed) || left_alt_down() {
                continue;
            }
            // Don't refocus mid-click (e.g. dragging a selection across windows).
            if vk_down(VK_LBUTTON) || vk_down(VK_RBUTTON) {
                continue;
            }
            let mut pt = POINT::default();
            if GetCursorPos(&mut pt).is_err() {
                continue;
            }
            let Some(hwnd) = root_window_at(pt) else {
                continue;
            };
            let h = hwnd.0 as isize;
            if h == last {
                continue;
            }
            last = h;
            // Only tracked windows; never fight non-managed/shell windows.
            if !MANAGED.lock().unwrap().contains(&h) {
                continue;
            }
            if GetForegroundWindow().0 as isize == h {
                continue;
            }
            push_cmd(Cmd::FocusMouse(h));
        }
    }
}

/// Watch the two config files and apply changes live, so editing + saving a
/// config takes effect without restarting suprland.
fn config_watcher() {
    use std::time::SystemTime;
    let wm = config_path("SUPRLAND_CONFIG", "suprland.conf");
    let nav = config_path("SUPRLAND_NAVBAR", "navbar.conf");
    let mtime = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut last: (Option<SystemTime>, Option<SystemTime>) = (mtime(&wm), mtime(&nav));
    loop {
        std::thread::sleep(std::time::Duration::from_millis(1000));
        let now = (mtime(&wm), mtime(&nav));
        if now == last {
            continue;
        }
        last = now;
        let cfg = load_config();
        // Statics the hooks/workers read directly.
        FOLLOW_MOUSE.store(cfg.focus_follows_mouse, Ordering::Relaxed);
        *IGNORE_CLASSES.lock().unwrap() = cfg.ignore_classes.clone();
        *FLOAT_CLASSES.lock().unwrap() = cfg.float_classes.clone();
        *WORKSPACE_KEYS.lock().unwrap() = cfg.workspace_keys.clone();
        {
            let mut hk = HOTKEYS.lock().unwrap();
            hk.focus_next = cfg.key_focus_next;
            hk.focus_prev = cfg.key_focus_prev;
            hk.shrink_master = cfg.key_shrink_master;
            hk.grow_master = cfg.key_grow_master;
            hk.promote_master = cfg.key_promote_master;
            hk.toggle_tiling = cfg.key_toggle_tiling;
            hk.toggle_float = cfg.key_toggle_float;
            hk.close_window = cfg.key_close_window;
        }
        BAR_BOTTOM.store(cfg.bar_bottom, Ordering::Relaxed);
        BAR_FONT_SIZE.store(cfg.bar_font_size as isize, Ordering::Relaxed);
        BAR_HEIGHT.store(
            if cfg.bar_enabled {
                cfg.bar_height as isize
            } else {
                0
            },
            Ordering::Relaxed,
        );
        // Manager applies the rest; the marker (main thread) rebuilds the bars.
        push_cmd(Cmd::Reload(Box::new(cfg)));
        let marker = MARKER_HWND.load(Ordering::Relaxed);
        if marker != 0 {
            unsafe {
                let _ = PostMessageW(hwnd_from(marker), WM_RELOAD, WPARAM(0), LPARAM(0));
            }
        }
    }
}

fn manager_loop(cfg: Config) {
    let mut mgr = unsafe {
        let mut monitors = enumerate_monitors();
        // The main monitor (contains the origin 0,0) owns workspace 1 and gets
        // initial focus.
        let primary = primary_index(&monitors);
        distribute_workspaces(&mut monitors, primary, cfg.workspaces, cfg.per_monitor);
        reserve_bar(&mut monitors, &cfg);
        let mut m = Manager {
            monitors,
            focused_mon: primary,
            primary,
            tiling: cfg.start_tiled,
            cfg,
        };
        assign_existing_windows(&mut m);
        if m.tiling {
            retile_all(&m);
        }
        style_all(&m);
        m
    };
    sync_managed(&mgr);
    unsafe {
        update_bar(&mgr);
    }
    loop {
        let cmd = {
            let mut q = CMDQ.lock().unwrap();
            loop {
                if let Some(c) = q.pop_front() {
                    break c;
                }
                q = CMDCV.wait(q).unwrap();
            }
        };
        unsafe {
            process(&mut mgr, cmd);
            apply_styles(&mgr);
            update_bar(&mgr);
        }
        sync_managed(&mgr);
    }
}

/// Refresh the shutdown registry and the O(1) locate index from current manager
/// state. One walk feeds both, so the index costs nothing extra.
fn sync_managed(mgr: &Manager) {
    let mut all = MANAGED.lock().unwrap();
    all.clear();
    let mut map: HashMap<isize, (usize, usize)> = HashMap::new();
    for (mi, m) in mgr.monitors.iter().enumerate() {
        for (wi, ws) in m.workspaces.iter().enumerate() {
            for &h in &ws.windows {
                all.push(h);
                map.insert(h, (mi, wi));
            }
        }
    }
    *INDEX.lock().unwrap() = Some(map);
}

/// WinEvent callback: translate OS window lifecycle/focus events into manager
/// commands. Runs on the main thread's message loop.
unsafe extern "system" fn win_event_proc(
    _hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if id_object != 0 || id_child != 0 || hwnd.0.is_null() {
        return;
    }
    match event {
        EVENT_OBJECT_SHOW => {
            if !SUPPRESS.load(Ordering::Relaxed) {
                push_cmd(Cmd::Add(hwnd.0 as isize));
            }
        }
        EVENT_SYSTEM_FOREGROUND => {
            // Foreground events refire for the same window; collapse repeats so
            // the manager doesn't re-run locate + styling for no change.
            let h = hwnd.0 as isize;
            if LAST_FG.swap(h, Ordering::Relaxed) == h {
                return;
            }
            push_cmd(Cmd::Focused(h));
            if !SUPPRESS.load(Ordering::Relaxed) {
                push_cmd(Cmd::Add(h));
            }
        }
        EVENT_OBJECT_HIDE | EVENT_OBJECT_DESTROY => {
            if !SUPPRESS.load(Ordering::Relaxed) {
                push_cmd(Cmd::Remove(hwnd.0 as isize));
            }
        }
        EVENT_SYSTEM_MINIMIZESTART | EVENT_SYSTEM_MINIMIZEEND => {
            push_cmd(Cmd::Retile);
        }
        // User finished a native (non-Alt) move/resize. Re-integrate the window
        // into the tiling: master keeps its new width as the ratio, everything
        // else snaps back so windows never overlap.
        EVENT_SYSTEM_MOVESIZEEND if !SUPPRESS.load(Ordering::Relaxed) => {
            push_cmd(Cmd::DragResized(hwnd.0 as isize));
        }
        _ => {}
    }
}

/// Map an Alt+key (with optional Shift) hotkey to a manager command. The
/// letter binds are rebindable via config (see `HOTKEYS`); arrows and Enter
/// are fixed.
fn map_hotkey(vk: u32, shift: bool) -> Option<Cmd> {
    {
        let hk = HOTKEYS.lock().unwrap();
        if vk == hk.focus_next {
            return Some(if shift { Cmd::SwapDir(1) } else { Cmd::FocusDir(1) });
        }
        if vk == hk.focus_prev {
            return Some(if shift { Cmd::SwapDir(-1) } else { Cmd::FocusDir(-1) });
        }
        if vk == hk.shrink_master {
            return Some(Cmd::ResizeMaster(-0.05));
        }
        if vk == hk.grow_master {
            return Some(Cmd::ResizeMaster(0.05));
        }
        if vk == hk.promote_master {
            return Some(Cmd::PromoteMaster);
        }
        if vk == hk.toggle_tiling {
            return Some(Cmd::ToggleTiling);
        }
        if vk == hk.toggle_float {
            return Some(Cmd::ToggleFloat);
        }
        if vk == hk.close_window {
            return Some(Cmd::CloseFocused);
        }
    }
    match vk {
        0x0D => Some(if shift { Cmd::LaunchBrowser } else { Cmd::LaunchTerminal }), // Enter
        0x25 => Some(if shift { Cmd::MoveGeo(Dir::Left) } else { Cmd::FocusGeo(Dir::Left) }), // Left
        0x26 => Some(if shift { Cmd::MoveGeo(Dir::Up) } else { Cmd::FocusGeo(Dir::Up) }),     // Up
        0x27 => Some(if shift { Cmd::MoveGeo(Dir::Right) } else { Cmd::FocusGeo(Dir::Right) }), // Right
        0x28 => Some(if shift { Cmd::MoveGeo(Dir::Down) } else { Cmd::FocusGeo(Dir::Down) }), // Down
        _ => None,
    }
}

/// Resolve a hotkey to a command: fixed binds first, then the configurable
/// workspace keys (Alt = switch, Alt+Shift = move focused window there).
fn resolve_hotkey(vk: u32, shift: bool) -> Option<Cmd> {
    if let Some(c) = map_hotkey(vk, shift) {
        return Some(c);
    }
    let keys = WORKSPACE_KEYS.lock().unwrap();
    if let Some(i) = keys.iter().position(|&k| k == vk) {
        return Some(if shift {
            Cmd::MoveToWs(i)
        } else {
            Cmd::Switch(i)
        });
    }
    None
}

fn main() {
    unsafe {
        let hmod = GetModuleHandleW(None).expect("GetModuleHandleW failed");
        let hinst = HINSTANCE(hmod.0);

        // Load config once here so the bars (main thread) and the manager thread
        // share the exact same settings.
        let cfg = load_config();
        FOLLOW_MOUSE.store(cfg.focus_follows_mouse, Ordering::Relaxed);
        *IGNORE_CLASSES.lock().unwrap() = cfg.ignore_classes.clone();
        *FLOAT_CLASSES.lock().unwrap() = cfg.float_classes.clone();
        *WORKSPACE_KEYS.lock().unwrap() = cfg.workspace_keys.clone();
        {
            let mut hk = HOTKEYS.lock().unwrap();
            hk.focus_next = cfg.key_focus_next;
            hk.focus_prev = cfg.key_focus_prev;
            hk.shrink_master = cfg.key_shrink_master;
            hk.grow_master = cfg.key_grow_master;
            hk.promote_master = cfg.key_promote_master;
            hk.toggle_tiling = cfg.key_toggle_tiling;
            hk.toggle_float = cfg.key_toggle_float;
            hk.close_window = cfg.key_close_window;
        }
        BAR_HINST.store(hinst.0 as isize, Ordering::Relaxed);
        if cfg.bar_enabled {
            BAR_HEIGHT.store(cfg.bar_height as isize, Ordering::Relaxed);
        }
        BAR_BOTTOM.store(cfg.bar_bottom, Ordering::Relaxed);
        BAR_FONT_SIZE.store(cfg.bar_font_size as isize, Ordering::Relaxed);

        // Red, click-through, topmost corner-marker overlay.
        let brush = CreateSolidBrush(COLORREF(0x000000FF)); // 0x00BBGGRR -> red
        let wc = WNDCLASSW {
            lpfnWndProc: Some(marker_wndproc),
            hInstance: hinst,
            hbrBackground: brush,
            lpszClassName: w!("hyprwin_marker"),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let marker = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("hyprwin_marker"),
            w!(""),
            WS_POPUP,
            0,
            0,
            MARK_LEN,
            MARK_LEN,
            None,
            None,
            hinst,
            None,
        )
        .expect("CreateWindowExW failed");
        let _ = SetLayeredWindowAttributes(marker, COLORREF(0), 200, LWA_ALPHA);
        MARKER_HWND.store(marker.0 as isize, Ordering::Relaxed);

        // Status bar on every monitor (waybar-style). Register the class once,
        // build the font, then create a bar window per monitor.
        if cfg.bar_enabled && cfg.bar_height > 0 {
            let bar_brush = CreateSolidBrush(COLORREF(cfg.bar_bg));
            let bwc = WNDCLASSW {
                lpfnWndProc: Some(bar_wndproc),
                hInstance: hinst,
                hbrBackground: bar_brush,
                lpszClassName: w!("suprland_bar"),
                ..Default::default()
            };
            RegisterClassW(&bwc);
            make_bar_font(cfg.bar_height, cfg.bar_font_size);
            ensure_bars();
        }

        let mouse_hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hinst, 0)
            .expect("mouse hook failed");
        let kbd_hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), hinst, 0)
            .expect("keyboard hook failed");

        // Reveal all managed windows on Ctrl+C / console close so none are left
        // hidden on another workspace when suprland exits.
        let _ = SetConsoleCtrlHandler(Some(console_handler), BOOL(1));

        // Reduce the foreground lock so the manager can focus windows reliably.
        let _ = SystemParametersInfoW(
            SPI_SETFOREGROUNDLOCKTIMEOUT,
            0,
            Some(core::ptr::null_mut()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );

        // React to windows opening/closing/focusing for tiling. Out-of-context
        // callbacks run on this thread's message loop; own-process events skipped.
        let _ = SetWinEventHook(
            EVENT_OBJECT_DESTROY,
            EVENT_OBJECT_HIDE,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        let _ = SetWinEventHook(
            EVENT_OBJECT_SHOW,
            EVENT_OBJECT_SHOW,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        let _ = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        let _ = SetWinEventHook(
            EVENT_SYSTEM_MINIMIZESTART,
            EVENT_SYSTEM_MINIMIZEEND,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        // Native (non-Alt) move/resize finished: re-tile so windows never overlap.
        let _ = SetWinEventHook(
            EVENT_SYSTEM_MOVESIZEEND,
            EVENT_SYSTEM_MOVESIZEEND,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );

        // Apply window moves/resizes off the input thread for smoothness.
        std::thread::spawn(position_worker);
        // Focus-follows-mouse poll loop (no-op unless enabled in config).
        std::thread::spawn(focus_follow_worker);
        // Hot-reload config files on save.
        std::thread::spawn(config_watcher);
        // Owns all tiling/workspace state; hooks only enqueue commands to it.
        std::thread::spawn(move || manager_loop(cfg));

        println!("suprland running.");
        println!("  LEFT ALT + left-drag  = move window (drops back into the tiling)");
        println!("  LEFT ALT + right-drag = resize nearest corner (red bracket)");
        println!("  --- tiling (LEFT ALT is the modifier) ---");
        println!("  Alt+T          = toggle tiling on/off (keeps workspaces)");
        println!("  Alt+J / Alt+K  = focus next / previous window");
        println!("  Alt+Shift+J/K  = swap window order in the stack");
        println!("  Alt+arrows     = focus window by direction (cursor follows)");
        println!("  Alt+Shift+arr  = move window by direction (across monitors)");
        println!("  Alt+M          = promote focused window to master");
        println!("  Alt+H / Alt+L  = shrink / grow the master area");
        println!("  Alt+F          = toggle float for focused window");
        println!("  Alt+W          = close focused window");
        println!("  Alt+Enter      = launch terminal");
        println!("  Alt+Shift+Enter= launch default browser");
        println!("  Alt+1..9       = switch workspace (or click a bar pill)");
        println!("  Alt+Shift+1..9 = move focused window to workspace");
        println!("  Per-monitor status bars, focus-follows-mouse, window rules:");
        println!("  all configurable in suprland.conf (see comments in that file).");
        println!("  Alt+Tab still works. Use RIGHT ALT for normal Alt behavior.");
        println!("  --- config ---");
        println!("  Default 'shared' mode spreads workspaces across monitors:");
        println!("  ws1=mon1, ws2=mon2, ws3=mon3, ws4=mon1 (2nd), and so on.");
        println!("  Edit %USERPROFILE%\\.suprland\\suprland.conf then restart.");
        println!("  workspace_mode = shared | per_monitor; set terminal/browser too.");
        println!("Press Ctrl+C in this window to quit (windows are restored).");

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let _ = UnhookWindowsHookEx(kbd_hook);
        let _ = UnhookWindowsHookEx(mouse_hook);
    }
}

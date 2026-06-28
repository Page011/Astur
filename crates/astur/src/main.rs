// astur — Alt-drag move/resize for Windows
//
// Hold LEFT ALT, then:
//   Left-drag   -> move the window under the cursor
//   Right-drag  -> resize from the corner nearest the cursor; a red marker
//                  shows which corner is being dragged
//
// LEFT ALT is reserved as Astur's modifier: a low-level keyboard hook blocks
// it from every application so it never triggers app menus or Alt shortcuts.
// Alt+Tab is preserved by synthesizing an injected Alt+Tab for the system.
// RIGHT ALT is untouched, so use it for normal Alt behavior.
//
// Both hooks run on this process's message-loop thread, so all drag state lives
// behind a single Mutex with effectively zero contention.

// Astur Full ships without a console window — the tray icon is the control surface
// (Settings / Quit). Release only, so debug builds keep the console for development.
// (Astur Lite, the `lite` branch, keeps its console.)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Instant;

mod layout;
// Config now lives in the shared `astur-config` crate (the settings GUI parses the
// same model). Aliased to `config` so the rest of this file is unchanged.
use astur_config as config;
use config::{config_path, load_config, Config};
use layout::{dwindle_layout, master_stack, resize_dwindle, split_ratio};

use windows::core::{w, PCWSTR};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::Foundation::{
    BOOL, BOOLEAN, CloseHandle, COLORREF, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, LUID, POINT,
    RECT, SIZE, SYSTEMTIME, WPARAM,
};
use windows::Win32::System::Shutdown::{
    ExitWindowsEx, LockWorkStation, EWX_FORCEIFHUNG, EWX_LOGOFF, EWX_REBOOT, EWX_SHUTDOWN,
    SHUTDOWN_REASON,
};
use windows::Win32::System::Power::SetSuspendState;
use windows::Win32::Security::{
    AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
    SE_SHUTDOWN_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::Graphics::Gdi::{
    AlphaBlend, BITMAP, BLENDFUNCTION, StretchBlt, SetStretchBltMode, HALFTONE,
    BeginPaint, BitBlt, CombineRgn, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW,
    CreateRectRgn, CreateSolidBrush, CreatePen, DeleteDC, DeleteObject, DrawTextW, EndPaint,
    GetObjectW, RoundRect, PS_SOLID, UpdateWindow,
    EnumDisplayMonitors, FillRect, GetDC, GetMonitorInfoW, GetStockObject, InvalidateRect,
    MonitorFromPoint, MonitorFromWindow, ReleaseDC, SelectObject, SetBkMode, SetTextColor,
    SetWindowRgn, CAPTUREBLT, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_GUI_FONT, DT_CALCRECT, DT_CENTER, DT_END_ELLIPSIS, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE,
    DT_VCENTER, HDC, HGDIOBJ, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, RGN_OR, SRCCOPY, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, MAPVK_VK_TO_CHAR, VIRTUAL_KEY, VK_BACK, VK_DOWN, VK_ESCAPE,
    VK_LBUTTON, VK_LEFT, VK_LMENU, VK_MENU, VK_RBUTTON, VK_RETURN, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::Shell::{
    ShellExecuteW, SHCreateItemFromParsingName, IShellItem, IEnumShellItems,
    IShellItemImageFactory, BHID_EnumItems, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING,
    SIIGBF_ICONONLY, Shell_NotifyIconW, NOTIFYICONDATAW, NIM_ADD, NIM_DELETE, NIF_ICON,
    NIF_MESSAGE, NIF_TIP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, DestroyMenu, LoadIconW, PostQuitMessage,
    TrackPopupMenu, HICON, IDI_APPLICATION, LR_DEFAULTCOLOR, MF_STRING, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, WM_LBUTTONDBLCLK,
};
use std::os::windows::ffi::OsStrExt;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Search::{
    IDataInitialize, IDBInitialize, IDBCreateSession, IDBCreateCommand, ICommandText, ICommand,
    IRowset, IAccessor, DBBINDING, HACCESSOR, MSDAINITIALIZE, DBPART_VALUE, DBPART_STATUS,
    DBMEMOWNER_PROVIDEROWNED, DBPARAMIO_NOTPARAM, DBTYPE_WSTR, DBTYPE_BYREF, DBTYPE_I8, DBTYPE_DATE,
    DBACCESSOR_ROWDATA, DBSTATUS_S_OK,
};
use windows::core::{GUID, IUnknown, Interface};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetAncestor,
    GetDesktopWindow, GetMessageW, GetShellWindow, GetWindowRect, IsZoomed, RegisterClassW,
    SetLayeredWindowAttributes, SetWindowPos, SetWindowsHookExW, ShowWindow,
    SetCursorPos,
    TranslateMessage,
    UnhookWindowsHookEx, WindowFromPoint, GA_ROOT, HC_ACTION, HWND_TOPMOST, KBDLLHOOKSTRUCT,
    LLKHF_INJECTED, LWA_ALPHA, MSG, MSLLHOOKSTRUCT, SWP_NOACTIVATE, SWP_NOSENDCHANGING, SWP_NOSIZE,
    SWP_NOZORDER,
    SWP_SHOWWINDOW, SW_HIDE, SW_RESTORE, SW_SHOWNA, DestroyWindow, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN,
    WM_SYSKEYUP, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};

// --- tiling additions -----------------------------------------------------
use std::collections::{HashMap, VecDeque};
use core::ffi::c_void;
use windows::Win32::Graphics::Dwm::{
    DwmFlush, DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CLOAKED,
    DWMWA_EXTENDED_FRAME_BOUNDS, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, OpenProcessToken,
};
use windows::Win32::UI::Accessibility::SetWinEventHook;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_SHIFT;
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, FindWindowExW, FindWindowW, SendMessageTimeoutW, SMTO_ABORTIFHUNG,
    GetClassNameW, GetForegroundWindow, GetWindow, GetWindowLongW, GetWindowTextLengthW,
    GetClientRect, GetCursorPos, GetWindowLongPtrW, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, IsWindow, IsWindowVisible, PeekMessageW, PostMessageW, SetWindowLongPtrW, GWLP_USERDATA, PM_REMOVE,
    KillTimer, PW_RENDERFULLCONTENT, SetForegroundWindow, SetTimer, SetWindowLongW, SystemParametersInfoW, EVENT_OBJECT_DESTROY,
    EVENT_OBJECT_HIDE, EVENT_OBJECT_SHOW, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND,
    EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MOVESIZEEND, GWL_EXSTYLE, GWL_STYLE, GW_OWNER,
    SPI_SETFOREGROUNDLOCKTIMEOUT,
    SW_SHOW, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
    WM_CLOSE, WM_DISPLAYCHANGE, WM_ERASEBKGND, WM_PAINT, WM_TIMER, WM_USER, WS_CHILD,
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
// Window class for the transient workspace-slide overlay.
const SLIDE_CLASS: PCWSTR = w!("astur_slide");

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

// =========================================================================
// Tile placement is instant: one SetWindowPos per window. Astur renders no
// window pixels (DWM does), so the only positional "animation" possible was
// interpolating SetWindowPos over time — it landed windows unreliably across
// apps and cost a per-frame cross-process DWM round-trip, so it was removed in
// favour of going straight to the target. The workspace-switch slide (DWM
// thumbnails, see run_transition) is a separate GPU-composited effect and is
// kept; ease_in_out_cubic below paces it.
// =========================================================================
/// Symmetric ease: slow start, fast middle, slow stop. Avoids the big first-frame
/// leap an ease-OUT gives a slide (which read as "jumpy").
#[inline]
fn ease_in_out_cubic(t: f64) -> f64 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let u = -2.0 * t + 2.0;
        1.0 - (u * u * u) / 2.0
    }
}

/// Overshoot ease: passes the target then settles back to it — the "spring"
/// feel. The back-ease is front-loaded (fast throw) and already lands with zero
/// velocity at t=1 (its derivative there is 0), so the settle is inherently
/// soft — no extra smoothing needed. `C1` sets overshoot strength (1.70158 =
/// classic back-ease; 1.10 was too timid to read as a spring). Lands EXACTLY on
/// the target at t=1 — required, or the final frame misaligns with the real
/// windows and the reveal pops. Returns values >1.0 around the tail, so callers
/// must have headroom past the target (the wallpaper backdrop covers the sliver
/// exposed past the edge at peak overshoot).
#[inline]
fn ease_out_back(t: f64) -> f64 {
    const C1: f64 = 1.40; // ~13% overshoot — a confident spring, not cartoonish
    const C3: f64 = C1 + 1.0;
    let u = t - 1.0;
    1.0 + C3 * u * u * u + C1 * u * u
}

/// Fade alpha ramp: 0→1, fast-out so the incoming workspace reads quickly.
#[inline]
fn ease_out_cubic(t: f64) -> f64 {
    let u = 1.0 - t;
    1.0 - u * u * u
}

/// Workspace-switch animation style. Parsed once per switch from the config
/// string; cheap enough not to cache.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WsAnim {
    Off,
    Slide,
    Spring,
    Fade,
}

impl WsAnim {
    fn from_cfg(cfg: &Config) -> WsAnim {
        // Back-compat: workspace_slide = false forces off regardless of the style.
        if !cfg.workspace_slide {
            return WsAnim::Off;
        }
        match cfg.workspace_anim.as_str() {
            "off" => WsAnim::Off,
            "spring" => WsAnim::Spring,
            "fade" => WsAnim::Fade,
            _ => WsAnim::Slide,
        }
    }
}

/// Move a window with no activation/zorder side effects (instant tile placement
/// and the workspace-slide reveal).
unsafe fn set_pos_raw(h: isize, r: RECT) {
    let _ = SetWindowPos(
        hwnd_from(h),
        None,
        r.left,
        r.top,
        r.right - r.left,
        r.bottom - r.top,
        SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSENDCHANGING,
    );
}

// Set by the keyboard hook while physical Left Alt is held (Alt is blocked from
// apps and reserved as Astur's modifier).
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

/// Low-level keyboard hook. Left Alt is reserved as Astur's modifier: it is
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
                PRESSED[kb.vkCode as usize].store(false, Ordering::Relaxed);
            }

            // System-menu capture mode: route nav keys to the power menu while open.
            if SYSMENU_OPEN.load(Ordering::Relaxed) {
                let vk = kb.vkCode;
                let is_mod = vk == VK_LMENU.0 as u32
                    || vk == VK_MENU.0 as u32
                    || vk == VK_SHIFT.0 as u32;
                if !is_mod {
                    if down {
                        let hs = SYSMENU_HWND.load(Ordering::Relaxed);
                        if hs != 0 {
                            let hwnd = hwnd_from(hs);
                            let post = |a: usize| {
                                let _ = PostMessageW(hwnd, WM_SYSMENU, WPARAM(a), LPARAM(0));
                            };
                            if vk == VK_ESCAPE.0 as u32 {
                                post(SM_CLOSE);
                            } else if vk == VK_RETURN.0 as u32 {
                                post(SM_ACTIVATE);
                            } else if vk == VK_UP.0 as u32 {
                                post(SM_UP);
                            } else if vk == VK_DOWN.0 as u32 {
                                post(SM_DOWN);
                            } else if vk == VK_LEFT.0 as u32 || vk == VK_BACK.0 as u32 {
                                post(SM_BACK);
                            }
                        }
                    }
                    return LRESULT(1); // swallow all non-modifier keys while open
                }
            }

            // Launcher capture mode: while the picker is open, route keys to it
            // and swallow them from the system. Modifiers fall through so Left
            // Alt's own bookkeeping (ALT_DOWN / FAKE_ALT) still runs.
            if LAUNCHER_OPEN.load(Ordering::Relaxed) {
                let vk = kb.vkCode;
                let is_mod = vk == VK_LMENU.0 as u32
                    || vk == VK_MENU.0 as u32
                    || vk == VK_SHIFT.0 as u32;
                if !is_mod {
                    if down {
                        let hl = LAUNCHER_HWND.load(Ordering::Relaxed);
                        if hl != 0 {
                            let hwnd = hwnd_from(hl);
                            let post = |a: usize, d: isize| {
                                let _ = PostMessageW(hwnd, WM_LAUNCHER, WPARAM(a), LPARAM(d));
                            };
                            if vk == VK_ESCAPE.0 as u32 {
                                post(LA_CLOSE, 0);
                            } else if vk == VK_RETURN.0 as u32 {
                                // Shift+Enter on a file opens its containing folder.
                                if vk_down(VK_SHIFT) {
                                    post(LA_ACTIVATE_ALT, 0);
                                } else {
                                    post(LA_ACTIVATE, 0);
                                }
                            } else if vk == VK_TAB.0 as u32 {
                                post(LA_TAB, 0); // toggle the file detail footer
                            } else if vk == VK_BACK.0 as u32 {
                                post(LA_BACK, 0);
                            } else if vk == VK_UP.0 as u32 {
                                post(LA_UP, 0);
                            } else if vk == VK_DOWN.0 as u32 {
                                post(LA_DOWN, 0);
                            } else if vk == VK_SPACE.0 as u32 {
                                post(LA_CHAR, ' ' as isize);
                            } else {
                                let c = MapVirtualKeyW(vk, MAPVK_VK_TO_CHAR) & 0x7FFF;
                                if c >= 0x20 {
                                    if let Some(ch) = char::from_u32(c) {
                                        post(LA_CHAR, ch.to_ascii_lowercase() as isize);
                                    }
                                }
                            }
                        }
                    }
                    return LRESULT(1); // swallow all non-modifier keys while open
                }
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

            // Alt+Shift+Space: system/power menu. Checked BEFORE the launcher so the
            // shift variant doesn't open the app picker.
            if down
                && ALT_DOWN.load(Ordering::Relaxed)
                && kb.vkCode == VK_SPACE.0 as u32
                && vk_down(VK_SHIFT)
                && !SYSMENU_OPEN.load(Ordering::Relaxed)
                && !LAUNCHER_OPEN.load(Ordering::Relaxed)
            {
                SYSMENU_OPEN.store(true, Ordering::Relaxed);
                let hs = SYSMENU_HWND.load(Ordering::Relaxed);
                if hs != 0 {
                    let _ = PostMessageW(hwnd_from(hs), WM_SYSMENU, WPARAM(SM_OPEN), LPARAM(0));
                }
                return LRESULT(1);
            }

            // Alt+Space: open the app launcher (no Shift — Shift is the system menu).
            // Not Win+Space — that's the system layout toggle. Left Alt is already
            // Astur's reserved modifier, so this never reaches apps.
            if down
                && ALT_DOWN.load(Ordering::Relaxed)
                && kb.vkCode == VK_SPACE.0 as u32
                && !vk_down(VK_SHIFT)
                && !LAUNCHER_OPEN.load(Ordering::Relaxed)
                && !SYSMENU_OPEN.load(Ordering::Relaxed)
            {
                LAUNCHER_OPEN.store(true, Ordering::Relaxed);
                let hl = LAUNCHER_HWND.load(Ordering::Relaxed);
                if hl != 0 {
                    let _ =
                        PostMessageW(hwnd_from(hl), WM_LAUNCHER, WPARAM(LA_OPEN), LPARAM(0));
                }
                return LRESULT(1);
            }

            // Tiling hotkeys: Alt + key. Swallowed from apps (Alt is reserved).
            if down && ALT_DOWN.load(Ordering::Relaxed) {
                let shift = vk_down(VK_SHIFT);
                if let Some(cmd) = resolve_hotkey(kb.vkCode, shift) {
                    let vk = kb.vkCode as usize;
                    // swap(true): push only on the first down (debounce auto-repeat),
                    // re-armed by the key-up store above. Lockless on the hot path.
                    if vk < 256 && !PRESSED[vk].swap(true, Ordering::Relaxed) {
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

    // Click outside the open launcher dismisses it. The picker is NOACTIVATE (never
    // focused), so the global hook is the only place that sees the click. Cheap: one
    // atomic when the launcher's closed (the common case), a rect compare only while
    // it's open AND the event is a button-down.
    if LAUNCHER_OPEN.load(Ordering::Relaxed) && matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN) {
        let inside = pt.x >= LAUNCHER_RECT_L.load(Ordering::Relaxed)
            && pt.x < LAUNCHER_RECT_R.load(Ordering::Relaxed)
            && pt.y >= LAUNCHER_RECT_T.load(Ordering::Relaxed)
            && pt.y < LAUNCHER_RECT_B.load(Ordering::Relaxed);
        if !inside {
            let hl = LAUNCHER_HWND.load(Ordering::Relaxed);
            if hl != 0 {
                let _ = PostMessageW(hwnd_from(hl), WM_LAUNCHER, WPARAM(LA_CLOSE), LPARAM(0));
            }
            return suppress; // eat the dismissing click so it doesn't also act
        }
    }

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
// Each monitor owns its own set of workspaces (GlazeWM style) and is
// tiled independently on its own work area. Windows are positioned with
// individual SetWindowPos calls (restore-then-place) — a robust approach used
// by komorebi; a single DeferWindowPos batch can fail wholesale if one window
// misbehaves, leaving everything un-tiled.
// =========================================================================

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
// Per-VK auto-repeat guard. Atomic (not a Mutex) so the keyboard hook — on the
// OS-wide input path — never takes a lock to debounce a held hotkey.
static PRESSED: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];
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
// Font family name, read on the main thread when (re)building the font.
static BAR_FONT_NAME: Mutex<String> = Mutex::new(String::new());
// Horizontal padding from each screen edge (px), read at paint time.
static BAR_PADDING: AtomicIsize = AtomicIsize::new(8);
// Live system stats (percent 0..100, or -1 = unavailable), filled by stats_worker
// and read at paint time. Gated by STATS_ON so the worker idles when no stat
// widget is enabled.
static STATS_ON: AtomicBool = AtomicBool::new(false);
static STAT_CPU: AtomicIsize = AtomicIsize::new(-1);
static STAT_MEM: AtomicIsize = AtomicIsize::new(-1);
static STAT_BAT: AtomicIsize = AtomicIsize::new(-1);

/// Sliding workspace-pill highlight. While an entry is present for a monitor,
/// paint_bar draws the accent pill at an interpolated x between the old and new
/// pill instead of snapping. Keyed by HMONITOR, driven by a fast WM_TIMER on the
/// bar window.
struct PillAnim {
    from_x: i32,
    to_x: i32,
    start: Instant,
}
static PILL_ANIM: Mutex<Option<HashMap<isize, PillAnim>>> = Mutex::new(None);
const PILL_ANIM_MS: f64 = 160.0;

fn pill_anim_set(hmon: isize, from_x: i32, to_x: i32) {
    PILL_ANIM.lock().unwrap().get_or_insert_with(HashMap::new).insert(
        hmon,
        PillAnim {
            from_x,
            to_x,
            start: Instant::now(),
        },
    );
}

fn pill_anim_clear(hmon: isize) {
    if let Some(m) = PILL_ANIM.lock().unwrap().as_mut() {
        m.remove(&hmon);
    }
}

/// Current highlight left-x for a monitor's pill animation and whether it's done.
/// None = no animation running for this monitor (paint at the static active pill).
fn pill_anim_x(hmon: isize) -> Option<(i32, bool)> {
    let g = PILL_ANIM.lock().unwrap();
    let a = g.as_ref()?.get(&hmon)?;
    let t = (a.start.elapsed().as_secs_f64() * 1000.0 / PILL_ANIM_MS).min(1.0);
    let x = (a.from_x as f64 + (a.to_x - a.from_x) as f64 * ease_in_out_cubic(t)).round() as i32;
    Some((x, t >= 1.0))
}

/// Per-monitor paint data. One entry per drawn pill: `slots[i]` is the local
/// workspace index that pill maps to (so a click resolves straight to a
/// workspace even when empty pills are hidden), `labels[i]` is the number to
/// print, `occupied` bit i marks a pill whose workspace has windows, and
/// `active` is the pill index of the shown workspace (usize::MAX if none).
#[derive(Clone, PartialEq)]
struct MonBar {
    hmon: isize,
    slots: Vec<usize>,
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
    show_date: bool,
    date_format: String,
    show_cpu: bool,
    show_mem: bool,
    show_battery: bool,
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
            show_date: false,
            date_format: String::new(),
            show_cpu: false,
            show_mem: false,
            show_battery: false,
            layout: String::new(),
            tiling: true,
            mons: Vec::new(),
        }
    }
}

static BAR: Mutex<BarData> = Mutex::new(BarData::new());
// Custom message: manager asks a bar to repaint.
const WM_BAR_REFRESH: u32 = WM_USER + 1;
// Custom message: manager seeds a pill-highlight slide (wparam=from_x, lparam=to_x).
const WM_PILL_ANIM: u32 = WM_USER + 3;
// SetTimer id for the pill-slide animation (distinct from the clock tick).
const PILL_TIMER_ID: usize = 2;
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
    // HMONITOR a launched terminal/browser should land on (the cursor's monitor at
    // launch time); consumed by the next Add. 0 = none.
    pending_launch_mon: isize,
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
    "astur_marker",
    "astur_bar",
    "astur_slide",
];

/// Is an already-tracked handle still worth re-homing on a display change?
/// Deliberately NOT `is_manageable`: that rejects `SW_HIDE`'d windows (every
/// window on an inactive workspace), which would silently drop and orphan them
/// when monitors are added/removed. A tracked window only stops being ours when
/// its window is actually destroyed.
unsafe fn tracked_window_alive(hwnd: HWND) -> bool {
    !hwnd.0.is_null() && IsWindow(hwnd).as_bool()
}

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

/// HMONITOR currently under the cursor, or 0 if it can't be read. Used to land a
/// launched terminal/browser on the workspace the cursor is on.
unsafe fn cursor_hmon() -> isize {
    let mut pt = POINT::default();
    if GetCursorPos(&mut pt).is_err() {
        return 0;
    }
    MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST).0 as isize
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
/// and undo Astur's styling — but leave every window exactly where it is, so
/// quitting doesn't disturb the current layout.
/// Reveal + un-style a specific list of window handles. Takes the list by ref so
/// callers control how they acquire it (the panic path must not re-lock a mutex
/// it may already hold — see `restore_on_panic`).
unsafe fn restore_windows(list: &[isize]) {
    SUPPRESS.store(true, Ordering::Relaxed);
    for &h in list {
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

unsafe fn restore_all_windows() {
    let list = MANAGED.lock().unwrap().clone();
    restore_windows(&list);
}

/// Panic-path restore: a thread panic with `panic = "abort"` runs the panic hook
/// but then aborts, skipping the console handler — so reveal managed windows here
/// or a window hidden on an inactive workspace is orphaned. Uses `try_lock`: the
/// panic may have fired while this thread already held MANAGED, and std mutexes
/// are not reentrant, so a blocking lock would deadlock instead of aborting.
fn restore_on_panic() {
    let list = MANAGED.try_lock().map(|g| g.clone()).unwrap_or_default();
    unsafe { restore_windows(&list) };
}

/// Console control handler: on Ctrl+C / window-close / logoff, un-hide every
/// managed window before the process dies so the user never loses them.
unsafe extern "system" fn console_handler(_ctrl_type: u32) -> BOOL {
    restore_all_windows();
    BOOL(0) // not fully handled — let the default handler terminate us
}

/// Place a window at `target` immediately. Restores minimised/maximised windows
/// first and border-corrects the resting rect so the visible edges sit flush.
/// (Named `animate_to` for historical reasons; placement is now always instant.)
unsafe fn animate_to(hwnd: HWND, target: RECT) {
    if IsIconic(hwnd).as_bool() || IsZoomed(hwnd).as_bool() {
        let _ = ShowWindow(hwnd, SW_RESTORE);
    }
    let to = adjust_for_border(hwnd, target);
    set_pos_raw(hwnd.0 as isize, to);
}

/// Compute the tiled (hwnd, screen-rect) targets for one workspace, in tiling
/// order — shared by retiling and the slide compositor. Rects are raw layout
/// rects (not yet border-corrected); callers adjust as needed.
unsafe fn workspace_layout(mgr: &Manager, mi: usize, wi: usize) -> Vec<(isize, RECT)> {
    if mi >= mgr.monitors.len() {
        return Vec::new();
    }
    let mon = &mgr.monitors[mi];
    let Some(ws) = mon.workspaces.get(wi) else {
        return Vec::new();
    };
    let tiled: Vec<isize> = ws
        .windows
        .iter()
        .copied()
        .filter(|h| !ws.floating.contains(h) && !IsIconic(hwnd_from(*h)).as_bool())
        .collect();
    let n = tiled.len();
    if n == 0 {
        return Vec::new();
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
        return Vec::new();
    }
    tiled.into_iter().zip(rects).collect()
}

/// Tile a single monitor's active workspace on that monitor's work area,
/// animating windows to their targets (glide) when animations are on.
unsafe fn retile_monitor(mgr: &Manager, mi: usize) {
    if !mgr.tiling {
        return;
    }
    let rects = workspace_layout(mgr, mi, mgr.monitors.get(mi).map(|m| m.active).unwrap_or(0));
    if rects.is_empty() {
        return;
    }

    // Glide path: animate windows from their current position to the new tile
    // slot via a cosmetic overlay (the real placement is still instant, done
    // underneath). Only when enabled, idle, and the layout actually changed —
    // a no-op retile (e.g. refocus) must not raise an overlay.
    let want_glide = mgr.cfg.animations
        && mgr.cfg.animation_ms > 0
        && mgr.cfg.window_anim == "glide"
        && !GLIDE_BUSY.load(Ordering::Relaxed);
    if want_glide {
        let full = mgr.monitors[mi].work_area;
        let mut items = Vec::with_capacity(rects.len());
        let mut changed = false;
        let mut ok = true;
        for (h, target) in &rects {
            let hwnd = hwnd_from(*h);
            let mut cur = RECT::default();
            if GetWindowRect(hwnd, &mut cur).is_err() {
                ok = false;
                break;
            }
            let to = adjust_for_border(hwnd, *target);
            let old = RECT {
                left: cur.left - full.left,
                top: cur.top - full.top,
                right: cur.right - full.left,
                bottom: cur.bottom - full.top,
            };
            let new = RECT {
                left: to.left - full.left,
                top: to.top - full.top,
                right: to.right - full.left,
                bottom: to.bottom - full.top,
            };
            // Treat a few-px difference as unchanged so DWM shadow/rounding jitter
            // doesn't trigger a glide on an effectively-static window.
            if (old.left - new.left).abs() > 2
                || (old.top - new.top).abs() > 2
                || (old.right - new.right).abs() > 2
                || (old.bottom - new.bottom).abs() > 2
            {
                changed = true;
            }
            items.push(GlideItem { old, new });
        }
        if ok && changed {
            let out = capture_monitor(full);
            if out != 0 {
                GLIDE_BUSY.store(true, Ordering::Relaxed);
                dispatch_glide(GlideReq {
                    out_bmp: out,
                    rect: full,
                    items,
                    dur_ms: mgr.cfg.animation_ms.max(1) as u64,
                });
                // Wait until the overlay covers the monitor, then place the real
                // windows underneath it (hidden), exactly like the workspace slide.
                wait_glide_overlay_up();
                SUPPRESS.store(true, Ordering::Relaxed);
                for (h, target) in rects {
                    animate_to(hwnd_from(h), target);
                }
                SUPPRESS.store(false, Ordering::Relaxed);
                return;
            }
        }
    }

    // Instant path (glide off, busy, capture failed, or nothing moved).
    SUPPRESS.store(true, Ordering::Relaxed);
    for (h, target) in rects {
        animate_to(hwnd_from(h), target);
    }
    SUPPRESS.store(false, Ordering::Relaxed);
}

/// Place the active workspace's windows at their targets INSTANTLY (no glide).
/// Used on workspace switch: the windows were just revealed from a hidden state,
/// so gliding them from a stale position would look like a jump.
unsafe fn place_active_instant(mgr: &Manager, mi: usize) {
    if !mgr.tiling {
        return;
    }
    let rects = workspace_layout(mgr, mi, mgr.monitors.get(mi).map(|m| m.active).unwrap_or(0));
    SUPPRESS.store(true, Ordering::Relaxed);
    for (h, target) in rects {
        let hwnd = hwnd_from(h);
        if IsIconic(hwnd).as_bool() || IsZoomed(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        set_pos_raw(h, adjust_for_border(hwnd, target));
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

/// Monotonic millisecond clock anchored at first use. Used for short-lived
/// timing guards (e.g. the focus-follow settle window) where a stored deadline
/// is needed and `Instant` can't live in an atomic.
fn now_ms() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Deadline (in `now_ms()`) before which focus-follows-mouse stays quiet. Set
/// whenever the manager moves focus programmatically (keyboard focus, workspace
/// switch) so the fast hover poll can't immediately yank focus back to whatever
/// window the cursor happens to be sitting over. A genuine cursor move after the
/// window expires still focuses normally.
static FOLLOW_SETTLE_MS: AtomicU64 = AtomicU64::new(0);
const FOLLOW_SETTLE_GUARD_MS: u64 = 200;

/// Suppress focus-follows-mouse for a short settle window after a programmatic
/// focus change. Cheap; called from the manager thread only.
fn bump_follow_settle() {
    FOLLOW_SETTLE_MS.store(now_ms() + FOLLOW_SETTLE_GUARD_MS, Ordering::Relaxed);
}

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

/// Style every window of a monitor's active workspace to its final opacity +
/// border immediately (focused vs dimmed). Called on workspace switch so the
/// revealed windows are already at their resting opacity — otherwise they pop in
/// at 100% and visibly dim a frame later.
unsafe fn style_active(mgr: &Manager, mi: usize) {
    let a = mgr.monitors[mi].active;
    let f = mgr.monitors[mi].workspaces[a].focused;
    for &h in &mgr.monitors[mi].workspaces[a].windows {
        style_window(hwnd_from(h), h != 0 && h == f, &mgr.cfg);
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

/// Collect the active workspace's windows with rectangles for directional nav.
/// Tiled windows use their LAYOUT TARGET rect (stable even while a glide is in
/// flight — live GetWindowRect would return transient mid-animation positions
/// and make Alt+arrow / Alt+Shift+arrow pick the wrong neighbour). Floating /
/// untiled windows fall back to their live rect.
unsafe fn active_window_rects(mgr: &Manager, mi: usize) -> Vec<(isize, RECT)> {
    let a = mgr.monitors[mi].active;
    let mut items: Vec<(isize, RECT)> = if mgr.tiling {
        workspace_layout(mgr, mi, a)
    } else {
        Vec::new()
    };
    for &h in &mgr.monitors[mi].workspaces[a].windows {
        if items.iter().any(|(w, _)| *w == h) {
            continue;
        }
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

/// A cosmetic workspace-slide request handed from the manager to the transition
/// thread. The manager has already performed the real (instant) switch; this is
/// purely a visual overlay, so losing or dropping it never affects windows.
struct SlideReq {
    out_bmp: isize, // HBITMAP: frozen outgoing workspace (worker owns + frees)
    in_bmp: isize,  // HBITMAP: frozen incoming workspace (worker owns + frees); 0 = first
                    // visit, no snapshot — worker holds the outgoing frame then reveals
    out_rects: Vec<RECT>, // work-area-local rects of the outgoing windows
    in_rects: Vec<RECT>,  // work-area-local rects of the incoming windows
    rect: RECT,     // work-area rect (overlay geometry)
    dir: i32,       // +1 = new ws came from the right, -1 from the left
    dur_ms: u64,
    mode: WsAnim,   // slide / spring / fade (off never reaches the worker)
}
static SLIDE_REQ: Mutex<Option<SlideReq>> = Mutex::new(None);
static SLIDE_CV: Condvar = Condvar::new();
// Handshake: the worker sets this true once the overlay is up and showing the
// outgoing image, so the manager can do the (now hidden) switch underneath it
// without the destination workspace flashing first.
static SLIDE_READY: Mutex<bool> = Mutex::new(false);
static SLIDE_READY_CV: Condvar = Condvar::new();

/// Block (bounded) until the transition worker has the overlay up and covering
/// the monitor, or the timeout elapses (overlay failed — proceed anyway).
fn wait_slide_overlay_up() {
    let guard = SLIDE_READY.lock().unwrap();
    let _ = SLIDE_READY_CV
        .wait_timeout_while(guard, std::time::Duration::from_millis(250), |up| !*up)
        .unwrap();
}

/// Worker → manager: overlay is up.
fn signal_slide_overlay_up() {
    *SLIDE_READY.lock().unwrap() = true;
    SLIDE_READY_CV.notify_one();
}

/// Per-(monitor, workspace) frozen snapshot of how that workspace last looked
/// when it was left: the work-area image plus the work-area-local rects of its
/// tiled windows (so the slide can move only the windows and leave the wallpaper
/// in the gaps still). Populated for free from the outgoing capture on every
/// switch. HBITMAPs are GPU-backed DDBs (~no process RAM). Touched only on the
/// manager thread — the worker gets private copies, so no cross-thread sharing.
struct Snap {
    bmp: isize,
    rects: Vec<RECT>,
}
static SNAP: Mutex<Option<HashMap<(isize, usize), Snap>>> = Mutex::new(None);

/// Store the snapshot for (hmon, ws), freeing any previous one.
unsafe fn snap_store(hmon: isize, ws: usize, bmp: isize, rects: Vec<RECT>) {
    if bmp == 0 {
        return;
    }
    let mut g = SNAP.lock().unwrap();
    let map = g.get_or_insert_with(HashMap::new);
    if let Some(old) = map.insert((hmon, ws), Snap { bmp, rects }) {
        let _ = DeleteObject(HGDIOBJ(old.bmp as *mut c_void));
    }
}

/// Current snapshot (bmp, window rects) for (hmon, ws), or None if not cached.
fn snap_get(hmon: isize, ws: usize) -> Option<(isize, Vec<RECT>)> {
    SNAP.lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(&(hmon, ws)))
        .map(|s| (s.bmp, s.rects.clone()))
}

/// Drop every cached snapshot (resolution/style no longer valid). Call on display
/// change and config reload.
unsafe fn snap_clear() {
    if let Some(map) = SNAP.lock().unwrap().take() {
        for (_, s) in map {
            let _ = DeleteObject(HGDIOBJ(s.bmp as *mut c_void));
        }
    }
}

// One-shot guard so the wallpaper-source diagnostic prints once, not every switch.
static WP_DIAG: AtomicBool = AtomicBool::new(false);

/// Find the desktop window that paints the wallpaper. On Win10/11 it's usually a
/// WorkerW spawned behind the icon host (SHELLDLL_DefView); on some configs the
/// wallpaper is on Progman itself, which is the fallback. Returns null if neither.
unsafe fn wallpaper_window() -> HWND {
    let progman = FindWindowW(w!("Progman"), PCWSTR::null()).unwrap_or(HWND(std::ptr::null_mut()));
    if !progman.0.is_null() {
        // Nudge Progman to spawn the wallpaper WorkerW (no-op if already present).
        let mut res: usize = 0;
        let _ = SendMessageTimeoutW(
            progman,
            0x052C,
            WPARAM(0),
            LPARAM(0),
            SMTO_ABORTIFHUNG,
            1000,
            Some(&mut res as *mut usize),
        );
    }
    let mut found: isize = 0;
    let _ = EnumWindows(Some(wp_enum), LPARAM(&mut found as *mut isize as isize));
    if found != 0 {
        return HWND(found as *mut c_void);
    }
    // No separate WorkerW — wallpaper is painted directly on Progman.
    progman
}

/// EnumWindows callback: the wallpaper WorkerW is the top-level WorkerW that sits
/// directly behind the WorkerW hosting SHELLDLL_DefView.
unsafe extern "system" fn wp_enum(top: HWND, lp: LPARAM) -> BOOL {
    let out = &mut *(lp.0 as *mut isize);
    let defview = FindWindowExW(top, None, w!("SHELLDLL_DefView"), PCWSTR::null());
    if matches!(defview, Ok(dv) if !dv.0.is_null()) {
        if let Ok(worker) = FindWindowExW(None, top, w!("WorkerW"), PCWSTR::null()) {
            if !worker.0.is_null() {
                *out = worker.0 as isize;
                return BOOL(0); // stop
            }
        }
    }
    BOOL(1)
}

/// Capture the wallpaper under `work_area` into a GPU-backed DDB, or 0 on failure
/// (caller then falls back to a flat slide). Captured fresh every slide (on the
/// worker thread) so it's always the CURRENT wallpaper — no cache to go stale when
/// the user changes it.
unsafe fn capture_wallpaper(work_area: RECT) -> isize {
    let w = work_area.right - work_area.left;
    let h = work_area.bottom - work_area.top;
    if w <= 0 || h <= 0 {
        return 0;
    }
    let src = wallpaper_window();
    if src.0.is_null() {
        if !WP_DIAG.swap(true, Ordering::Relaxed) {
            eprintln!("[Astur] wallpaper: no Progman/WorkerW found -> flat slide");
        }
        return 0;
    }
    let mut wr = RECT::default();
    if GetWindowRect(src, &mut wr).is_err() {
        return 0;
    }
    let (ww, wh) = (wr.right - wr.left, wr.bottom - wr.top);
    if ww <= 0 || wh <= 0 {
        return 0;
    }
    let screen = GetDC(None);
    if screen.0.is_null() {
        return 0;
    }
    // Render the WHOLE wallpaper window with PrintWindow + PW_RENDERFULLCONTENT
    // (BitBlt of a DWM-composited desktop window comes back black), then crop the
    // work-area region out of it.
    let fulldc = CreateCompatibleDC(screen);
    let fullbmp = CreateCompatibleBitmap(screen, ww, wh);
    let resdc = CreateCompatibleDC(screen);
    let resbmp = CreateCompatibleBitmap(screen, w, h);
    let ofb = SelectObject(fulldc, HGDIOBJ(fullbmp.0));
    let orb = SelectObject(resdc, HGDIOBJ(resbmp.0));
    let printed = PrintWindow(src, fulldc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();
    let ok = printed
        && BitBlt(
            resdc,
            0,
            0,
            w,
            h,
            fulldc,
            work_area.left - wr.left,
            work_area.top - wr.top,
            SRCCOPY,
        )
        .is_ok();
    SelectObject(fulldc, ofb);
    SelectObject(resdc, orb);
    let _ = DeleteObject(HGDIOBJ(fullbmp.0));
    let _ = DeleteDC(fulldc);
    let _ = DeleteDC(resdc);
    let _ = ReleaseDC(None, screen);
    if !WP_DIAG.swap(true, Ordering::Relaxed) {
        let mut buf = [0u16; 64];
        let n = GetClassNameW(src, &mut buf);
        let class = String::from_utf16_lossy(&buf[..n as usize]);
        eprintln!("[Astur] wallpaper source class '{class}', PrintWindow={printed}, ok={ok}");
    }
    if !ok {
        let _ = DeleteObject(HGDIOBJ(resbmp.0));
        return 0;
    }
    resbmp.0 as isize
}

/// Duplicate a DDB into a fresh GPU-backed bitmap the caller owns. Used to hand
/// the transition worker its own copies so the cache is never touched off-thread.
unsafe fn dup_ddb(src: isize, w: i32, h: i32) -> isize {
    if src == 0 || w <= 0 || h <= 0 {
        return 0;
    }
    let screen = GetDC(None);
    if screen.0.is_null() {
        return 0;
    }
    let dst = CreateCompatibleBitmap(screen, w, h);
    if dst.0.is_null() {
        let _ = ReleaseDC(None, screen);
        return 0;
    }
    let sdc = CreateCompatibleDC(screen);
    let ddc = CreateCompatibleDC(screen);
    let so = SelectObject(sdc, HGDIOBJ(src as *mut c_void));
    let do_ = SelectObject(ddc, HGDIOBJ(dst.0));
    let _ = BitBlt(ddc, 0, 0, w, h, sdc, 0, 0, SRCCOPY);
    SelectObject(sdc, so);
    SelectObject(ddc, do_);
    let _ = DeleteDC(sdc);
    let _ = DeleteDC(ddc);
    let _ = ReleaseDC(None, screen);
    dst.0 as isize
}

/// Hand a slide to the transition thread, replacing (and freeing) any request it
/// hasn't picked up yet so a burst of switches can't leak frozen bitmaps.
fn dispatch_slide(req: SlideReq) {
    *SLIDE_READY.lock().unwrap() = false;
    {
        let mut slot = SLIDE_REQ.lock().unwrap();
        if let Some(old) = slot.take() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(old.out_bmp as *mut c_void));
                let _ = DeleteObject(HGDIOBJ(old.in_bmp as *mut c_void));
            }
        }
        *slot = Some(req);
    }
    SLIDE_CV.notify_one();
}

// =========================================================================
// Per-window glide: window move / open / close / re-tile animation.
//
// Reuses the workspace-overlay trick instead of the (removed, jittery)
// per-frame real-window SetWindowPos. On a layout change the manager freezes
// the work area to one bitmap, the worker raises a topmost overlay showing
// frame 0 (== current screen, no flash) and signals back; the manager then
// places the REAL windows at their targets instantly UNDER the overlay; the
// worker glides each window's frozen image from its old rect to its new rect
// over a wallpaper backdrop, then tears the overlay down to reveal the already
// correct windows. A black/failed wallpaper capture degrades to instant.
// =========================================================================

/// One window's travel for a glide, in work-area-local coordinates.
struct GlideItem {
    old: RECT,
    new: RECT,
}

/// A cosmetic window-glide request handed from the manager to the glide worker.
/// The worker owns and frees `out_bmp`.
struct GlideReq {
    out_bmp: isize,    // HBITMAP: frozen work area before placement (worker frees)
    rect: RECT,        // work area (overlay geometry)
    items: Vec<GlideItem>, // per-window old->new travel, work-area-local
    dur_ms: u64,
}
static GLIDE_REQ: Mutex<Option<GlideReq>> = Mutex::new(None);
static GLIDE_CV: Condvar = Condvar::new();
static GLIDE_READY: Mutex<bool> = Mutex::new(false);
static GLIDE_READY_CV: Condvar = Condvar::new();
// True from dispatch until the overlay tears down. Lets the manager skip
// stacking a second glide over a running one (it places instantly instead).
static GLIDE_BUSY: AtomicBool = AtomicBool::new(false);

fn wait_glide_overlay_up() {
    let guard = GLIDE_READY.lock().unwrap();
    let _ = GLIDE_READY_CV
        .wait_timeout_while(guard, std::time::Duration::from_millis(250), |up| !*up)
        .unwrap();
}

fn signal_glide_overlay_up() {
    *GLIDE_READY.lock().unwrap() = true;
    GLIDE_READY_CV.notify_one();
}

/// Hand a glide to its worker, freeing any request it hasn't picked up yet.
fn dispatch_glide(req: GlideReq) {
    *GLIDE_READY.lock().unwrap() = false;
    {
        let mut slot = GLIDE_REQ.lock().unwrap();
        if let Some(old) = slot.take() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(old.out_bmp as *mut c_void));
            }
        }
        *slot = Some(req);
    }
    GLIDE_CV.notify_one();
}

/// Glide thread: owns its own overlay + message pump, idles on the condvar.
fn glide_worker() {
    loop {
        let req = {
            let mut slot = GLIDE_REQ.lock().unwrap();
            loop {
                if let Some(r) = slot.take() {
                    break r;
                }
                slot = GLIDE_CV.wait(slot).unwrap();
            }
        };
        unsafe { run_window_glide(req) };
        GLIDE_BUSY.store(false, Ordering::Relaxed);
    }
}

/// Composite a window glide: wallpaper backdrop + each window's frozen image
/// blitted from its old rect to an eased-interpolated rect (StretchBlt covers
/// resizes). Worker owns and frees `out_bmp`.
unsafe fn run_window_glide(req: GlideReq) {
    let full = req.rect;
    let w = full.right - full.left;
    let h = full.bottom - full.top;
    let free_out = || {
        let _ = DeleteObject(HGDIOBJ(req.out_bmp as *mut c_void));
    };
    if w <= 0 || h <= 0 || req.out_bmp == 0 || req.items.is_empty() {
        free_out();
        signal_glide_overlay_up();
        return;
    }
    // Need the still wallpaper to fill vacated areas. If we can't get it, degrade
    // to an instant switch (no overlay): signal and bail, the manager places the
    // real windows with no animation.
    let wp = capture_wallpaper(full);
    if wp == 0 {
        free_out();
        signal_glide_overlay_up();
        return;
    }
    let hinst = HINSTANCE(BAR_HINST.load(Ordering::Relaxed) as *mut c_void);
    let overlay = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
        SLIDE_CLASS,
        w!(""),
        WS_POPUP,
        full.left,
        full.top,
        w,
        h,
        None,
        None,
        hinst,
        None,
    );
    let Ok(overlay) = overlay else {
        let _ = DeleteObject(HGDIOBJ(wp as *mut c_void));
        free_out();
        signal_glide_overlay_up();
        return;
    };

    let odc = GetDC(overlay);
    let backdc = CreateCompatibleDC(odc);
    let back = CreateCompatibleBitmap(odc, w, h);
    let srcdc = CreateCompatibleDC(odc); // frozen before-frame
    let wpdc = CreateCompatibleDC(odc); // wallpaper backdrop
    let ob = SelectObject(backdc, HGDIOBJ(back.0));
    let os = SelectObject(srcdc, HGDIOBJ(req.out_bmp as *mut c_void));
    let owp = SelectObject(wpdc, HGDIOBJ(wp as *mut c_void));
    // Smooth scaling for the resize case (HALFTONE), harmless for pure moves.
    SetStretchBltMode(backdc, HALFTONE);

    // Compose one frame at eased progress `e` (0..=1). At e=0 every window sits
    // at its old rect over the still wallpaper == current screen (no flash). At
    // e=1 every window is at its new rect, pixel-aligned with the real windows
    // placed underneath, so the reveal is seamless.
    let compose = |e: f64| {
        let _ = BitBlt(backdc, 0, 0, w, h, wpdc, 0, 0, SRCCOPY);
        for it in &req.items {
            let lerp = |a: i32, b: i32| (a as f64 + (b - a) as f64 * e).round() as i32;
            let dl = lerp(it.old.left, it.new.left);
            let dt = lerp(it.old.top, it.new.top);
            let dw = lerp(it.old.right, it.new.right) - dl;
            let dh = lerp(it.old.bottom, it.new.bottom) - dt;
            let (sw, sh) = (it.old.right - it.old.left, it.old.bottom - it.old.top);
            if dw > 0 && dh > 0 && sw > 0 && sh > 0 {
                let _ = StretchBlt(
                    backdc, dl, dt, dw, dh, srcdc, it.old.left, it.old.top, sw, sh, SRCCOPY,
                );
            }
        }
    };

    // Frame 0 must be pixel-identical to the live screen (exact capture via srcdc,
    // not the wallpaper-composited compose(0.0)). CRITICAL ORDER: show the overlay
    // FIRST, THEN present — a blit to a still-hidden window's DC is clipped away and
    // lost, leaving the overlay empty so the wallpaper flashes through (see the full
    // note in run_transition). Show, present, settle, flush, then signal.
    let _ = BitBlt(backdc, 0, 0, w, h, srcdc, 0, 0, SRCCOPY);
    let _ = ShowWindow(overlay, SW_SHOWNA);
    let _ = BitBlt(odc, 0, 0, w, h, backdc, 0, 0, SRCCOPY);
    let _ = UpdateWindow(overlay);
    let _ = DwmFlush();
    signal_glide_overlay_up();

    let dur = req.dur_ms.max(1) as f64;
    let frame_dur = std::time::Duration::from_micros(8_333); // ~120 Hz
    let start = Instant::now();
    let mut next = start;
    let mut msg = MSG::default();
    loop {
        while PeekMessageW(&mut msg, overlay, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let el = start.elapsed().as_secs_f64() * 1000.0;
        compose(ease_out_cubic((el / dur).min(1.0)));
        let _ = BitBlt(odc, 0, 0, w, h, backdc, 0, 0, SRCCOPY);
        if el >= dur {
            break;
        }
        next += frame_dur;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }

    SelectObject(backdc, ob);
    SelectObject(srcdc, os);
    SelectObject(wpdc, owp);
    let _ = DeleteObject(HGDIOBJ(back.0));
    let _ = DeleteObject(HGDIOBJ(wp as *mut c_void));
    let _ = DeleteDC(backdc);
    let _ = DeleteDC(srcdc);
    let _ = DeleteDC(wpdc);
    ReleaseDC(overlay, odc);
    free_out();
    // Sync teardown to the next DWM frame so the real (already-placed) windows
    // are composited before the overlay disappears — no flash on the reveal.
    let _ = DwmFlush();
    let _ = DestroyWindow(overlay);
}

/// Transition thread: owns the slide overlay and pumps its own message loop, so
/// the overlay is a well-behaved window (never the "not responding" ghost a
/// pump-less window becomes). Blocks on the condvar when idle.
fn transition_worker() {
    loop {
        let req = {
            let mut slot = SLIDE_REQ.lock().unwrap();
            loop {
                if let Some(r) = slot.take() {
                    break r;
                }
                slot = SLIDE_CV.wait(slot).unwrap();
            }
        };
        unsafe { run_transition(req) };
    }
}

/// How long the switch overlay holds the outgoing frame on a FIRST visit (no
/// cached incoming snapshot) before revealing — long enough for the destination's
/// first paint to land underneath, short enough to read as instant. Without this
/// hold a freshly-shown window (whose DWM surface was discarded by SW_HIDE) would
/// flash its background through before it repaints.
const COVER_HOLD_MS: u64 = 48;

/// Render one push: a FIXED, monitor-bounded topmost overlay whose surface is a
/// two-image filmstrip — the frozen OUTGOING workspace and the frozen INCOMING
/// workspace, side by side — scrolled together so the old slides off one edge as
/// the new slides in from the other. The overlay never moves, so it cannot bleed
/// onto an adjacent monitor; everything is GDI blits the eye sees as one motion.
/// Both snapshots are screen BitBlts (gaps/dimming baked in) so the reveal at the
/// end is pixel-identical to the real windows already placed underneath. The
/// worker owns and frees both request bitmaps. When `in_bmp == 0` (first visit to
/// the destination, no cached snapshot) the overlay instead HOLDS the outgoing
/// frame for `COVER_HOLD_MS` to cover the switch + first paint, then reveals.
unsafe fn run_transition(req: SlideReq) {
    let full = req.rect;
    let w = full.right - full.left;
    let h = full.bottom - full.top;
    let free_in = || {
        let _ = DeleteObject(HGDIOBJ(req.out_bmp as *mut c_void));
        let _ = DeleteObject(HGDIOBJ(req.in_bmp as *mut c_void));
    };
    if w <= 0 || h <= 0 || req.out_bmp == 0 {
        free_in();
        signal_slide_overlay_up(); // unblock the manager (no overlay this time)
        return;
    }
    // No incoming image == first visit to the destination workspace (no cached
    // snapshot). We still raise the overlay and HOLD the outgoing frame so the real
    // switch + the destination's first paint happen underneath it, hidden, then
    // reveal — killing the "background flashes through the windows" pop a
    // freshly-shown (surface-discarded) window makes before it repaints.
    let have_incoming = req.in_bmp != 0;
    // Capture the CURRENT wallpaper here on the worker (not cached), so it's always
    // up to date and the manager isn't blocked by the PrintWindow. 0 = flat slide.
    let wp = capture_wallpaper(full);
    let hinst = HINSTANCE(BAR_HINST.load(Ordering::Relaxed) as *mut c_void);
    let overlay = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
        SLIDE_CLASS,
        w!(""),
        WS_POPUP,
        full.left,
        full.top,
        w,
        h,
        None,
        None,
        hinst,
        None,
    );
    let Ok(overlay) = overlay else {
        free_in();
        signal_slide_overlay_up();
        return;
    };

    // One reused back buffer + source DCs; compose into the back buffer then
    // present in a single blit per frame (no flicker, no per-frame allocation).
    let odc = GetDC(overlay);
    let backdc = CreateCompatibleDC(odc);
    let back = CreateCompatibleBitmap(odc, w, h);
    let outdc = CreateCompatibleDC(odc);
    let indc = CreateCompatibleDC(odc);
    let wpdc = CreateCompatibleDC(odc);
    let ob = SelectObject(backdc, HGDIOBJ(back.0));
    let oo = SelectObject(outdc, HGDIOBJ(req.out_bmp as *mut c_void));
    let oi = if req.in_bmp != 0 {
        SelectObject(indc, HGDIOBJ(req.in_bmp as *mut c_void))
    } else {
        HGDIOBJ::default()
    };
    let owp = if wp != 0 {
        Some(SelectObject(wpdc, HGDIOBJ(wp as *mut c_void)))
    } else {
        None
    };

    // Compose one frame into the back buffer at horizontal offset `off`. With a
    // wallpaper backdrop, the still wallpaper is laid down first and only the
    // window rects are blitted on top (sliding), so the gaps stay put. Without
    // one (capture failed) it falls back to a flat full-frame filmstrip.
    let compose = |off: i32| {
        if wp != 0 {
            let _ = BitBlt(backdc, 0, 0, w, h, wpdc, 0, 0, SRCCOPY);
            for r in &req.out_rects {
                let (rw, rh) = (r.right - r.left, r.bottom - r.top);
                let _ = BitBlt(backdc, r.left + off, r.top, rw, rh, outdc, r.left, r.top, SRCCOPY);
            }
            for r in &req.in_rects {
                let (rw, rh) = (r.right - r.left, r.bottom - r.top);
                let _ = BitBlt(
                    backdc,
                    r.left + off + req.dir * w,
                    r.top,
                    rw,
                    rh,
                    indc,
                    r.left,
                    r.top,
                    SRCCOPY,
                );
            }
        } else {
            let _ = BitBlt(backdc, off, 0, w, h, outdc, 0, 0, SRCCOPY);
            let _ = BitBlt(backdc, off + req.dir * w, 0, w, h, indc, 0, 0, SRCCOPY);
        }
    };

    // Paint frame 0 BEFORE showing the overlay so raising it causes no flash.
    // CRITICAL: frame 0 must be pixel-identical to what's already on screen, or
    // the instant the overlay is raised it pops (the "flash before the slide").
    // `compose(0)` rebuilds the frame from the PrintWindow wallpaper capture +
    // window rects; if that wallpaper differs even slightly from the live
    // DWM-composited desktop (acrylic/transparency, sub-pixel crop), the gaps
    // flash on raise. So for frame 0 we blit the EXACT live screen capture
    // (`out_bmp`, grabbed by `capture_monitor` a moment ago) straight through —
    // a guaranteed match. The wallpaper-composited path only kicks in once the
    // windows actually start moving (off != 0), where a sub-pixel gap diff is
    // invisible under motion.
    let _ = BitBlt(backdc, 0, 0, w, h, outdc, 0, 0, SRCCOPY);
    // CRITICAL ORDER — show the overlay FIRST, then present frame 0 to its DC.
    // Blitting to the window DC while the overlay is still HIDDEN is clipped to its
    // (empty) visible region and silently lost; the overlay then comes up empty and
    // DWM shows the wallpaper underneath until the animation loop's first frame
    // lands a few ms later. That is exactly the "windows flash hidden (wallpaper),
    // then reappear and slide" the user reported. Showing first makes the present
    // land on the now-visible window; `UpdateWindow` settles any pending paint onto
    // our pixels (erase is suppressed in `slide_wndproc`); `DwmFlush` blocks until
    // frame 0 is genuinely on the glass. Only THEN signal the manager to do the
    // real switch underneath the (now actually covering) overlay.
    let _ = ShowWindow(overlay, SW_SHOWNA);
    let _ = BitBlt(odc, 0, 0, w, h, backdc, 0, 0, SRCCOPY);
    let _ = UpdateWindow(overlay);
    let _ = DwmFlush();
    signal_slide_overlay_up();

    // The new ws came from the `dir` side, so the outgoing leaves the opposite
    // way; the incoming sits in the adjacent filmstrip slot (off + dir*w) and is
    // contiguous with it (no seam).
    let target = -req.dir * w;
    let dur = req.dur_ms.max(1) as f64;
    let has_wp = wp != 0;
    let frame_dur = std::time::Duration::from_micros(8_333); // ~120 Hz back-buffer
    let start = Instant::now();
    let mut next = start;
    let mut msg = MSG::default();
    // Whole-frame constant-alpha blend descriptor, reused for the fade mode.
    // BlendOp 0 == AC_SRC_OVER; AlphaFormat 0 == ignore per-pixel alpha (the
    // captured DDBs have no alpha channel), so SourceConstantAlpha drives it.
    let mut blend = BLENDFUNCTION {
        BlendOp: 0,
        BlendFlags: 0,
        SourceConstantAlpha: 0,
        AlphaFormat: 0,
    };
    loop {
        while PeekMessageW(&mut msg, overlay, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if !have_incoming {
            // First visit: hold frame 0 (already on screen) for the cover window,
            // then break to the synced reveal. Deliberately NO recompose — blitting
            // the (window-less) incoming would slide the outgoing off to bare
            // wallpaper. We just wait while the switch + first paint land beneath.
            if start.elapsed() >= std::time::Duration::from_millis(COVER_HOLD_MS) {
                break;
            }
            next += frame_dur;
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else {
                next = now;
            }
            continue;
        }
        let el = start.elapsed().as_secs_f64() * 1000.0;
        let t = (el / dur).min(1.0);
        match req.mode {
            WsAnim::Fade => {
                // Crossfade whole frames: outgoing underneath, incoming alpha
                // ramped on top. Both DDBs already bake in wallpaper + gaps, so
                // the still regions stay rock-steady and only the windows fade.
                let _ = BitBlt(backdc, 0, 0, w, h, outdc, 0, 0, SRCCOPY);
                blend.SourceConstantAlpha =
                    (255.0 * ease_out_cubic(t)).round().clamp(0.0, 255.0) as u8;
                let _ = AlphaBlend(backdc, 0, 0, w, h, indc, 0, 0, w, h, blend);
            }
            WsAnim::Spring if has_wp => {
                // Overshoot past the target then settle. Needs a wallpaper
                // backdrop: at peak overshoot a thin band past the edge is
                // exposed and must show the still wallpaper, not black.
                let off = (target as f64 * ease_out_back(t)).round() as i32;
                compose(off);
            }
            _ => {
                // Slide (and spring with no wallpaper backdrop — fall back to the
                // symmetric ease so the overshoot can't expose a black sliver).
                let off = (target as f64 * ease_in_out_cubic(t)).round() as i32;
                compose(off);
            }
        }
        let _ = BitBlt(odc, 0, 0, w, h, backdc, 0, 0, SRCCOPY);
        if el >= dur {
            break;
        }
        next += frame_dur;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }

    SelectObject(backdc, ob);
    SelectObject(outdc, oo);
    SelectObject(indc, oi);
    if let Some(owp) = owp {
        SelectObject(wpdc, owp);
    }
    let _ = DeleteObject(HGDIOBJ(back.0));
    let _ = DeleteObject(HGDIOBJ(wp as *mut c_void));
    let _ = DeleteDC(backdc);
    let _ = DeleteDC(outdc);
    let _ = DeleteDC(indc);
    let _ = DeleteDC(wpdc);
    ReleaseDC(overlay, odc);
    free_in();
    // Sync the reveal to a DWM composition pass. The real windows were placed
    // (and styled) under the overlay long ago, but tearing the overlay down
    // off-vblank can expose a frame before DWM has recomposited them — the
    // "flash" where the snapshot vanishes a beat before the live window paints.
    // Block until the next composed frame so the overlay's last (target-aligned)
    // pixels and the live windows hand off on the same vblank: a clean reveal.
    let _ = DwmFlush();
    let _ = DestroyWindow(overlay);
}

/// WndProc for the slide overlay: swallow background erase (the GDI blits own
/// every pixel; letting DefWindowProc erase with the class brush would flash
/// black before the first frame).
unsafe extern "system" fn slide_wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_ERASEBKGND {
        return LRESULT(1);
    }
    DefWindowProcW(h, msg, w, l)
}

/// Instant workspace switch: hide the old set, reveal + tile the new. Used when
/// the slide compositor is disabled or not applicable.
unsafe fn switch_plain(mgr: &mut Manager, mi: usize, old: usize, n: usize) {
    SUPPRESS.store(true, Ordering::Relaxed);
    // Iterate by index (no Vec clone per switch): the manager owns `mgr` on this
    // thread and ShowWindow touches no Astur state, so the borrow is safe to hold.
    {
        let ws = &mgr.monitors[mi].workspaces[old].windows;
        for i in 0..ws.len() {
            let _ = ShowWindow(hwnd_from(ws[i]), SW_HIDE);
        }
    }
    mgr.monitors[mi].active = n;
    {
        let ws = &mgr.monitors[mi].workspaces[n].windows;
        for i in 0..ws.len() {
            let _ = ShowWindow(hwnd_from(ws[i]), SW_SHOW);
        }
    }
    SUPPRESS.store(false, Ordering::Relaxed);
    // Instant placement — these windows were just unhidden; gliding them from a
    // stale position would jump.
    place_active_instant(mgr, mi);
}

/// Capture a monitor's current pixels into a GPU-backed off-screen bitmap (DDB,
/// not a DIB — so ~no process RAM). Returns the HBITMAP as an isize, or 0 on
/// failure. The caller hands it to the transition thread, which frees it.
unsafe fn capture_monitor(full: RECT) -> isize {
    let w = full.right - full.left;
    let h = full.bottom - full.top;
    if w <= 0 || h <= 0 {
        return 0;
    }
    let screen = GetDC(None);
    if screen.0.is_null() {
        return 0;
    }
    let mem = CreateCompatibleDC(screen);
    let bmp = CreateCompatibleBitmap(screen, w, h);
    if bmp.0.is_null() {
        let _ = DeleteDC(mem);
        let _ = ReleaseDC(None, screen);
        return 0;
    }
    let old = SelectObject(mem, HGDIOBJ(bmp.0));
    let _ = BitBlt(mem, 0, 0, w, h, screen, full.left, full.top, SRCCOPY | CAPTUREBLT);
    SelectObject(mem, old);
    let _ = DeleteDC(mem);
    let _ = ReleaseDC(None, screen);
    bmp.0 as isize
}

/// Work-area-local rects of every window on (mi, wsi), read from their real
/// positions — so the slide moves floating windows (and float mode) too, not just
/// the tiled layout.
unsafe fn ws_window_rects(mgr: &Manager, mi: usize, wsi: usize, origin: RECT) -> Vec<RECT> {
    mgr.monitors[mi].workspaces[wsi]
        .windows
        .iter()
        .filter_map(|&hwin| {
            let mut r = RECT::default();
            GetWindowRect(hwnd_from(hwin), &mut r).ok().map(|_| RECT {
                left: r.left - origin.left,
                top: r.top - origin.top,
                right: r.right - origin.left,
                bottom: r.bottom - origin.top,
            })
        })
        .collect()
}

/// Switch one monitor to workspace `n`, then focus. Workspaces are never cleared
/// — only shown/hidden. When the slide compositor is enabled the switch is still
/// done instantly and correctly here (so window management can never break);
/// only a cosmetic snapshot is handed to the transition thread to slide over it.
unsafe fn switch_monitor_workspace(mgr: &mut Manager, mi: usize, n: usize) {
    if mi >= mgr.monitors.len() {
        return;
    }
    let old = mgr.monitors[mi].active;
    if n == old || n >= mgr.monitors[mi].workspaces.len() {
        return;
    }
    // Not gated on tiling: the transition is cosmetic and works in float mode too.
    let mode = WsAnim::from_cfg(&mgr.cfg);
    let want_slide = mgr.cfg.animations && mgr.cfg.animation_ms > 0 && mode != WsAnim::Off;
    let dir = if n > old { 1 } else { -1 };
    let hmon = mgr.monitors[mi].hmon;
    // Slide region = the tiling work area, NOT the full monitor. This excludes the
    // navbar, so the bar stays pinned above the slide instead of moving with it.
    let full = mgr.monitors[mi].work_area;
    let (w, h) = (full.right - full.left, full.bottom - full.top);

    // Freeze the outgoing workspace BEFORE the switch, while it's still on screen,
    // along with the work-area-local rects of its tiled windows (so only the
    // windows slide and the wallpaper in the gaps stays put).
    let out = if want_slide { capture_monitor(full) } else { 0 };
    let out_rects: Vec<RECT> = if out != 0 {
        ws_window_rects(mgr, mi, old, full)
    } else {
        Vec::new()
    };

    // Push: the worker raises an overlay showing the outgoing image (frame 0 ==
    // current screen, so no visible change) and signals back once it covers the
    // monitor. We then do the real switch UNDERNEATH it — that's what stops the
    // destination workspace flashing before the animation. Incoming image is the
    // snapshot from the last time we left `n`; the worker gets private copies.
    // Always raise the overlay when we have an outgoing capture — even on the FIRST
    // visit to `n`, where there's no cached snapshot to slide in. With an incoming
    // image the worker animates (slide/spring/fade); without one it briefly holds
    // the outgoing frame to cover the switch + first paint, then reveals. Either
    // way the destination never flashes its background before it repaints.
    if out != 0 {
        let (in_bmp, in_rects) = match snap_get(hmon, n) {
            Some((b, r)) => (dup_ddb(b, w, h), r),
            None => (0, Vec::new()), // first visit: cover-and-reveal, no slide image
        };
        // Worker captures the still wallpaper backdrop itself (always current).
        dispatch_slide(SlideReq {
            out_bmp: dup_ddb(out, w, h),
            in_bmp,
            out_rects: out_rects.clone(),
            in_rects,
            rect: full,
            dir,
            // Floor the duration so a full-monitor push is never too steppy. Fade
            // has no positional steppiness, so it can use the raw configured ms.
            dur_ms: if mode == WsAnim::Fade {
                mgr.cfg.animation_ms.max(1) as u64
            } else {
                mgr.cfg.animation_ms.max(200) as u64
            },
            mode,
        });
        wait_slide_overlay_up();
    }

    // The real, correct switch — instant placement, on this thread. Cannot fail.
    // Now hidden under the overlay (if sliding).
    switch_plain(mgr, mi, old, n);

    // Cache the fresh outgoing as `old`'s snapshot for next time (takes ownership
    // of `out`, freeing any previous snapshot of that ws). First visit to a ws
    // has no snapshot, so its first entry is an instant switch.
    if out != 0 {
        snap_store(hmon, old, out, out_rects);
    }

    // Resolve the new workspace's focus, then style every window to its resting
    // opacity/border NOW. This is what stops the reveal from popping in at 100%
    // and dimming a frame later; it happens under the overlay, so it's invisible.
    let f = {
        let ws = &mut mgr.monitors[mi].workspaces[n];
        let f = if ws.focused != 0 {
            ws.focused
        } else {
            ws.windows.first().copied().unwrap_or(0)
        };
        ws.focused = f;
        f
    };
    style_active(mgr, mi);
    STYLED_FOCUS.store(f, Ordering::Relaxed);

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
    // Hold focus-follows-mouse off for a beat: the cursor may still be sitting
    // over a window on another monitor, and the fast hover poll would otherwise
    // yank focus straight back off the workspace we just switched to.
    bump_follow_settle();
}

/// Re-enumerate monitors after a display change. Preserves each surviving
/// monitor's active workspace and re-homes tracked windows, keeping their
/// workspace index when the monitor still exists.
unsafe fn refresh_monitors(mgr: &mut Manager) {
    // Cached workspace snapshots are tied to the old monitor handles/resolution
    // and are invalid after a display change — drop them all.
    snap_clear();
    // Snapshot tracked windows BEFORE the rebuild. Each window remembers the
    // GLOBAL workspace number it lived on (computed against the OLD layout), so
    // when a monitor is unplugged its windows keep their workspace identity and
    // collate onto a surviving monitor instead of all collapsing onto that
    // monitor's active workspace.
    let old_n = mgr.monitors.len().max(1);
    let old_primary = mgr.primary;
    let per_monitor = mgr.cfg.per_monitor;
    // Remember which physical monitor was focused — its index shifts when a
    // monitor to its left is removed, so a bare range-clamp would leave focus
    // (and the per-monitor gone-window fallback) pointing at the wrong screen.
    let old_focused_hmon = mgr
        .monitors
        .get(mgr.focused_mon)
        .map(|m| m.hmon)
        .unwrap_or(0);
    // (old hmon, old local wi, old global ws, hwnd, floating?)
    let mut tracked: Vec<(isize, usize, usize, isize, bool)> = Vec::new();
    let mut old_active: Vec<(isize, usize)> = Vec::new();
    for (mi, mon) in mgr.monitors.iter().enumerate() {
        old_active.push((mon.hmon, mon.active));
        for (wi, ws) in mon.workspaces.iter().enumerate() {
            let global = if per_monitor {
                wi
            } else {
                let off = (mi + old_n - old_primary % old_n) % old_n;
                wi * old_n + off
            };
            for &h in &ws.windows {
                let floating = ws.floating.contains(&h);
                tracked.push((mon.hmon, wi, global, h, floating));
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
    // Re-resolve focus to the same physical monitor (its index may have moved);
    // fall back to primary if that screen is gone. Must run before any
    // global_to_ml below — it reads focused_mon in per_monitor mode.
    mgr.focused_mon = mgr
        .mon_by_hmon(old_focused_hmon)
        .unwrap_or(primary)
        .min(mgr.monitors.len().saturating_sub(1));
    for (old_hmon, wi, global, h, floating) in tracked {
        if !tracked_window_alive(hwnd_from(h)) {
            continue;
        }
        let (mi, target_wi) = if per_monitor {
            // Per-monitor: workspaces are independent per screen. A surviving
            // monitor keeps its exact local workspace; a window from a gone
            // monitor falls to the focused monitor's same-numbered workspace.
            if let Some(mi) = mgr.mon_by_hmon(old_hmon) {
                (mi, wi.min(mgr.monitors[mi].workspaces.len() - 1))
            } else {
                let (mi, local) = mgr.global_to_ml(global);
                (mi, local.min(mgr.monitors[mi].workspaces.len() - 1))
            }
        } else {
            // Shared mode: the global workspace number is the invariant, not the
            // physical monitor. Re-map EVERY window through its saved global —
            // when primary/monitor-count changes, a surviving monitor's local
            // index no longer equals the old global number, so keeping `wi`
            // would misplace windows.
            let (mi, local) = mgr.global_to_ml(global);
            (mi, local.min(mgr.monitors[mi].workspaces.len() - 1))
        };
        let ws = &mut mgr.monitors[mi].workspaces[target_wi];
        if !ws.windows.contains(&h) {
            ws.windows.push(h);
            if floating && !ws.floating.contains(&h) {
                ws.floating.push(h);
            }
            if ws.focused == 0 {
                ws.focused = h;
            }
        }
    }
    // Normalize visibility: windows re-homed from a hidden (inactive) workspace
    // onto a now-active one must be re-shown, and vice versa. Without this they
    // stay SW_HIDE'd and appear to vanish.
    SUPPRESS.store(true, Ordering::Relaxed);
    for mon in &mgr.monitors {
        let active = mon.active;
        for (wi, ws) in mon.workspaces.iter().enumerate() {
            let cmd = if wi == active { SW_SHOWNA } else { SW_HIDE };
            for &h in &ws.windows {
                let _ = ShowWindow(hwnd_from(h), cmd);
            }
        }
    }
    SUPPRESS.store(false, Ordering::Relaxed);
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
                // A terminal/browser we just launched lands on the cursor's monitor
                // (consumed once); everything else goes by its spawn position.
                let pending = std::mem::replace(&mut mgr.pending_launch_mon, 0);
                let mi = mgr
                    .mon_by_hmon(pending)
                    .unwrap_or_else(|| monitor_index_for_window(mgr, hwnd_from(h)));
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
            // Gaps/opacity may have changed — cached snapshots are now stale.
            snap_clear();
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
                bump_follow_settle();
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
            let mi = mgr.focused_mon;
            if mgr.cfg.layout == "master" {
                // Master layout: one global master width.
                mgr.cfg.master_ratio = (mgr.cfg.master_ratio + delta).clamp(0.15, 0.85);
            } else {
                // Dwindle: grow/shrink the focused window's own split so H/L do
                // something useful here too (master_ratio is unused by dwindle).
                let a = mgr.monitors[mi].active;
                let ws = &mgr.monitors[mi].workspaces[a];
                let tiled: Vec<isize> = ws
                    .windows
                    .iter()
                    .copied()
                    .filter(|h| !ws.floating.contains(h) && !IsIconic(hwnd_from(*h)).as_bool())
                    .collect();
                let n = tiled.len();
                if n >= 2 {
                    if let Some(idx) = tiled.iter().position(|&h| h == ws.focused) {
                        // The window at idx owns split level idx (first part); the
                        // last window is the remainder of level n-2 (gets 1-ratio).
                        let (level, remainder) = if idx < n - 1 {
                            (idx, false)
                        } else {
                            (n - 2, true)
                        };
                        let splits = &mut mgr.monitors[mi].workspaces[a].splits;
                        if splits.len() < n - 1 {
                            splits.resize(n - 1, 0.5);
                        }
                        let cur = split_ratio(splits, level);
                        // Positive delta always grows the focused window.
                        let nr = if remainder { cur - delta } else { cur + delta };
                        splits[level] = nr.clamp(0.05, 0.95);
                    }
                }
            }
            retile_monitor(mgr, mi);
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
        Cmd::LaunchTerminal => {
            // Land the new window on the workspace the cursor is on, not wherever
            // the OS opens it (usually the primary monitor).
            mgr.pending_launch_mon = cursor_hmon();
            launch(&mgr.cfg.terminal);
        }
        Cmd::LaunchBrowser => {
            mgr.pending_launch_mon = cursor_hmon();
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
            bump_follow_settle();
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
    // Null-terminated face name; kept alive for the duration of the call.
    let name = {
        let n = BAR_FONT_NAME.lock().unwrap().clone();
        if n.trim().is_empty() {
            "Segoe UI".to_string()
        } else {
            n
        }
    };
    let mut wname: Vec<u16> = name.encode_utf16().collect();
    wname.push(0);
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
        PCWSTR(wname.as_ptr()),
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
                w!("astur_bar"),
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

/// Render a date from a SYSTEMTIME using a small token language:
///   yyyy/yy = year, MMM/MM = month (name/number), ddd/dd = weekday/day-of-month.
/// Any other characters are copied verbatim. Char-based so a non-ASCII format
/// string can't split a UTF-8 boundary.
fn format_date(fmt: &str, st: &SYSTEMTIME) -> String {
    const WD: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MO: [&str; 13] = [
        "", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let chars: Vec<char> = fmt.chars().collect();
    let at = |i: usize, tok: &str| -> bool {
        let t: Vec<char> = tok.chars().collect();
        i + t.len() <= chars.len() && chars[i..i + t.len()] == t[..]
    };
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if at(i, "yyyy") {
            out.push_str(&format!("{:04}", st.wYear));
            i += 4;
        } else if at(i, "yy") {
            out.push_str(&format!("{:02}", st.wYear % 100));
            i += 2;
        } else if at(i, "MMM") {
            out.push_str(MO.get(st.wMonth as usize).copied().unwrap_or(""));
            i += 3;
        } else if at(i, "MM") {
            out.push_str(&format!("{:02}", st.wMonth));
            i += 2;
        } else if at(i, "ddd") {
            out.push_str(WD.get(st.wDayOfWeek as usize).copied().unwrap_or(""));
            i += 3;
        } else if at(i, "dd") {
            out.push_str(&format!("{:02}", st.wDay));
            i += 2;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Poll CPU / RAM / battery into the STAT_* atomics every ~2s for the bar's
/// stats widgets. Idles cheaply while no stat widget is enabled (STATS_ON). Runs
/// off the input/manager threads so it can never add latency to either.
fn stats_worker() {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows::Win32::System::Threading::GetSystemTimes;
    let ticks = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    let mut prev_idle = 0u64;
    let mut prev_total = 0u64;
    loop {
        if !STATS_ON.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(500));
            continue;
        }
        unsafe {
            // CPU: kernel time already includes idle, so total = kernel + user
            // and busy = total - idle. Percentage is over the interval delta.
            let mut idle = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            if GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).is_ok() {
                let idle_t = ticks(idle);
                let total_t = ticks(kernel) + ticks(user);
                let didle = idle_t.saturating_sub(prev_idle);
                let dtotal = total_t.saturating_sub(prev_total);
                if prev_total != 0 && dtotal > 0 {
                    let used = dtotal.saturating_sub(didle);
                    let pct = (used as f64 / dtotal as f64 * 100.0).round() as isize;
                    STAT_CPU.store(pct.clamp(0, 100), Ordering::Relaxed);
                }
                prev_idle = idle_t;
                prev_total = total_t;
            }
            // RAM: dwMemoryLoad is already a 0..100 percentage.
            let mut ms = MEMORYSTATUSEX {
                dwLength: core::mem::size_of::<MEMORYSTATUSEX>() as u32,
                ..Default::default()
            };
            if GlobalMemoryStatusEx(&mut ms).is_ok() {
                STAT_MEM.store(ms.dwMemoryLoad as isize, Ordering::Relaxed);
            }
            // Battery: 0..100, or 255 = unknown / no battery present.
            let mut ps = SYSTEM_POWER_STATUS::default();
            if GetSystemPowerStatus(&mut ps).is_ok() && ps.BatteryLifePercent <= 100 {
                STAT_BAT.store(ps.BatteryLifePercent as isize, Ordering::Relaxed);
            } else {
                STAT_BAT.store(-1, Ordering::Relaxed);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(2000));
    }
}

/// Rebuild the per-monitor bar snapshot and repaint only the bars that changed.
/// The clock is refreshed separately by each bar's 1s timer, so an idle desktop
/// causes no repaints from here.
unsafe fn update_bar(mgr: &Manager) {
    if BARS.lock().unwrap().is_empty() {
        return;
    }
    let hide_empty = mgr.cfg.bar_hide_empty;
    let mut mons = Vec::with_capacity(mgr.monitors.len());
    for (mi, m) in mgr.monitors.iter().enumerate() {
        // Pills are this monitor's OWN workspaces only. In shared mode each
        // monitor owns a slice of the global numbering (so labels like 1,4,7,10
        // on the primary, 2,5,8 on the next), and every label is reachable by a
        // workspace key. Iterating cfg.workspaces here instead would invent local
        // indices the monitor doesn't have and balloon shared-mode labels past
        // the 10 reachable keys (the old "workspace 30" bug).
        let count = m.workspaces.len();
        // Which local workspaces get a pill. The active one is always shown;
        // empties are dropped only when hide_empty_workspaces is set.
        let mut slots: Vec<usize> = Vec::with_capacity(count);
        for local in 0..count {
            let occ = m
                .workspaces
                .get(local)
                .is_some_and(|ws| !ws.windows.is_empty());
            if !hide_empty || occ || local == m.active {
                slots.push(local);
            }
        }
        // Pill numbers: per_monitor shows 1..count; shared shows this monitor's
        // slice of the global numbering, which starts at the primary monitor.
        let labels: Vec<usize> = slots
            .iter()
            .map(|&local| {
                if mgr.cfg.per_monitor {
                    local + 1
                } else {
                    mgr.ml_to_global(mi, local) + 1
                }
            })
            .collect();
        let mut occupied: u64 = 0;
        for (pill, &local) in slots.iter().enumerate().take(64) {
            if m
                .workspaces
                .get(local)
                .is_some_and(|ws| !ws.windows.is_empty())
            {
                occupied |= 1 << pill;
            }
        }
        let active = slots
            .iter()
            .position(|&l| l == m.active)
            .unwrap_or(usize::MAX);
        let fh = m.workspaces.get(m.active).map(|ws| ws.focused).unwrap_or(0);
        let title = if fh != 0 {
            window_title(hwnd_from(fh))
        } else {
            String::new()
        };
        mons.push(MonBar {
            hmon: m.hmon,
            slots,
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
        show_date: mgr.cfg.bar_show_date,
        date_format: mgr.cfg.bar_date_format.clone(),
        show_cpu: mgr.cfg.bar_show_cpu,
        show_mem: mgr.cfg.bar_show_mem,
        show_battery: mgr.cfg.bar_show_battery,
        layout: mgr.cfg.layout.clone(),
        tiling: mgr.tiling,
        mons,
    };

    // Diff against the previous snapshot so only changed monitors repaint, and
    // seed a pill-highlight slide on any monitor whose active workspace moved.
    let animate_pills = mgr.cfg.animations;
    let cell = BAR_CELL.load(Ordering::Relaxed) as i32;
    let pad = BAR_PADDING.load(Ordering::Relaxed) as i32;
    let mut changed: Vec<isize> = Vec::new();
    let mut anim_seeds: Vec<(isize, i32, i32)> = Vec::new();
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
            || old.show_date != new.show_date
            || old.date_format != new.date_format
            || old.show_cpu != new.show_cpu
            || old.show_mem != new.show_mem
            || old.show_battery != new.show_battery
            || old.layout != new.layout
            || old.tiling != new.tiling
            || old.mons.len() != new.mons.len();
        for nm in &new.mons {
            let om = old.mons.iter().find(|om| om.hmon == nm.hmon);
            let diff = match om {
                Some(om) => om != nm,
                None => true,
            };
            if global_changed || diff {
                changed.push(nm.hmon);
            }
            // Animate only when the pill layout is unchanged (so indices are
            // comparable) and a different, real pill became active.
            if animate_pills {
                if let Some(om) = om {
                    if om.slots == nm.slots
                        && om.active != usize::MAX
                        && nm.active != usize::MAX
                        && om.active != nm.active
                    {
                        anim_seeds.push((
                            nm.hmon,
                            pad + om.active as i32 * cell,
                            pad + nm.active as i32 * cell,
                        ));
                    }
                }
            }
        }
    }
    *BAR.lock().unwrap() = new;
    if changed.is_empty() && anim_seeds.is_empty() {
        return;
    }
    let bars = BARS.lock().unwrap().clone();
    for b in bars {
        if changed.contains(&b.hmon) {
            let _ = PostMessageW(hwnd_from(b.hwnd), WM_BAR_REFRESH, WPARAM(0), LPARAM(0));
        }
        if let Some(&(_, fx, tx)) = anim_seeds.iter().find(|s| s.0 == b.hmon) {
            let _ = PostMessageW(
                hwnd_from(b.hwnd),
                WM_PILL_ANIM,
                WPARAM(fx as usize),
                LPARAM(tx as isize),
            );
        }
    }
}

/// Measure the pixel width of a string in the current DC font.
unsafe fn text_width(hdc: HDC, s: &str) -> i32 {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    if v.is_empty() {
        return 0;
    }
    let mut r = RECT::default();
    DrawTextW(
        hdc,
        &mut v,
        &mut r,
        DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
    );
    r.right - r.left
}

/// Draw one right-cluster widget flush against the running `right` edge, then
/// move the edge left by the text width plus a gap. Skips empty strings so a
/// disabled / unavailable widget leaves no hole.
const BAR_WIDGET_GAP: i32 = 16;
unsafe fn draw_right(hdc: HDC, right: &mut i32, h_px: i32, s: &str, color: u32) {
    let w = text_width(hdc, s);
    if w <= 0 {
        return;
    }
    let mut r = RECT {
        left: *right - w,
        top: 0,
        right: *right,
        bottom: h_px,
    };
    SetTextColor(hdc, COLORREF(color));
    let mut v: Vec<u16> = s.encode_utf16().collect();
    DrawTextW(
        hdc,
        &mut v,
        &mut r,
        DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
    *right -= w + BAR_WIDGET_GAP;
}

/// Paint one monitor's bar in three clusters: workspace pills (left), focused
/// title (centre), and the widget cluster (right): clock, date, battery, mem,
/// cpu, layout — drawn right-to-left and measured so each only takes the room it
/// needs. The owning monitor's HMONITOR is in GWLP_USERDATA so each bar paints
/// its own data.
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
    let pad = BAR_PADDING.load(Ordering::Relaxed) as i32;
    let mut right_edge = rc.right - pad;

    // ---- right cluster (right-to-left): clock, date, battery, mem, cpu, layout
    if data.show_clock {
        let st: SYSTEMTIME = GetLocalTime();
        let clock = if data.clock_24h {
            format!("{:02}:{:02}", st.wHour, st.wMinute)
        } else {
            let (h12, ap) = to_12h(st.wHour);
            format!("{}:{:02} {}", h12, st.wMinute, ap)
        };
        draw_right(hdc, &mut right_edge, h_px, &clock, data.fg);
    }
    if data.show_date {
        let st: SYSTEMTIME = GetLocalTime();
        let date = format_date(&data.date_format, &st);
        draw_right(hdc, &mut right_edge, h_px, &date, data.fg);
    }
    if data.show_battery {
        let b = STAT_BAT.load(Ordering::Relaxed);
        if b >= 0 {
            draw_right(hdc, &mut right_edge, h_px, &format!("BAT {}%", b), data.fg);
        }
    }
    if data.show_mem {
        let v = STAT_MEM.load(Ordering::Relaxed);
        if v >= 0 {
            draw_right(hdc, &mut right_edge, h_px, &format!("RAM {}%", v), data.fg);
        }
    }
    if data.show_cpu {
        let v = STAT_CPU.load(Ordering::Relaxed);
        if v >= 0 {
            draw_right(hdc, &mut right_edge, h_px, &format!("CPU {}%", v), data.fg);
        }
    }
    if data.show_layout {
        let s = if data.tiling {
            format!("[{}]", data.layout)
        } else {
            "[float]".to_string()
        };
        draw_right(hdc, &mut right_edge, h_px, &s, data.inactive);
    }

    if let Some(mb) = data.mons.iter().find(|m| m.hmon == hmon) {
        // ---- left cluster: workspace pills, offset by the edge padding.
        // Numbers first, all in their resting colours...
        for (i, label) in mb.labels.iter().enumerate() {
            let x0 = pad + i as i32 * cell;
            let mut cr = RECT {
                left: x0,
                top: 0,
                right: x0 + cell,
                bottom: h_px,
            };
            let occ = mb.occupied & (1 << i) != 0;
            SetTextColor(hdc, COLORREF(if occ { data.fg } else { data.inactive }));
            let mut s: Vec<u16> = format!("{}", label).encode_utf16().collect();
            DrawTextW(
                hdc,
                &mut s,
                &mut cr,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        }
        // ...then the accent highlight on top, at the animated x while a slide is
        // in flight, otherwise snapped to the active pill. The number it sits over
        // is redrawn in the bg colour so it reads through the fill.
        let hl = match pill_anim_x(hmon) {
            Some((x, _)) => Some(x),
            None if mb.active != usize::MAX => Some(pad + mb.active as i32 * cell),
            None => None,
        };
        if let (Some(x), true) = (hl, !mb.labels.is_empty() && cell > 0) {
            let ipad = (h_px / 6).clamp(2, 6);
            let pill = RECT {
                left: x + 3,
                top: ipad,
                right: x + cell - 3,
                bottom: h_px - ipad,
            };
            let ab = CreateSolidBrush(COLORREF(data.accent));
            FillRect(hdc, &pill, ab);
            let _ = DeleteObject(HGDIOBJ(ab.0));
            let nearest = (((x - pad) as f32 / cell as f32).round() as i32)
                .clamp(0, mb.labels.len() as i32 - 1) as usize;
            let mut cr = RECT {
                left: x,
                top: 0,
                right: x + cell,
                bottom: h_px,
            };
            SetTextColor(hdc, COLORREF(data.bg));
            let mut s: Vec<u16> = format!("{}", mb.labels[nearest]).encode_utf16().collect();
            DrawTextW(
                hdc,
                &mut s,
                &mut cr,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        }

        // ---- centre cluster: focused window title, centred in the gap between
        // the pills and the right cluster (ellipsised if it doesn't fit).
        if data.show_title && !mb.title.is_empty() {
            let left = pad + mb.labels.len() as i32 * cell + 14;
            let right = right_edge - 8;
            if right > left {
                let mut tr = RECT {
                    left,
                    top: 0,
                    right,
                    bottom: h_px,
                };
                SetTextColor(hdc, COLORREF(data.fg));
                let mut s: Vec<u16> = mb.title.encode_utf16().collect();
                DrawTextW(
                    hdc,
                    &mut s,
                    &mut tr,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
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
        WM_PILL_ANIM => {
            let hmon = GetWindowLongPtrW(h, GWLP_USERDATA);
            pill_anim_set(hmon, w.0 as i32, l.0 as i32);
            // ~120 Hz repaint while the highlight slides.
            SetTimer(h, PILL_TIMER_ID, 8, None);
            let _ = InvalidateRect(h, None, BOOL(0));
            LRESULT(0)
        }
        WM_TIMER => {
            if w.0 == PILL_TIMER_ID {
                let hmon = GetWindowLongPtrW(h, GWLP_USERDATA);
                // Stop the fast timer once the slide finishes (or vanished).
                if pill_anim_x(hmon).map(|(_, done)| done).unwrap_or(true) {
                    let _ = KillTimer(h, PILL_TIMER_ID);
                    pill_anim_clear(hmon);
                }
            }
            let _ = InvalidateRect(h, None, BOOL(0));
            LRESULT(0)
        }
        WM_BAR_REFRESH => {
            let _ = InvalidateRect(h, None, BOOL(0));
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (l.0 as u32 & 0xFFFF) as i16 as i32;
            let cell = BAR_CELL.load(Ordering::Relaxed) as i32;
            let pad = BAR_PADDING.load(Ordering::Relaxed) as i32;
            let hmon = GetWindowLongPtrW(h, GWLP_USERDATA);
            if cell > 0 && x >= pad {
                let pill = ((x - pad) / cell) as usize;
                // Map the clicked pill back to its real local workspace via slots
                // (pills and workspaces diverge when empty pills are hidden).
                let local = BAR
                    .lock()
                    .unwrap()
                    .mons
                    .iter()
                    .find(|m| m.hmon == hmon)
                    .and_then(|m| m.slots.get(pill).copied());
                if let Some(local) = local {
                    push_cmd(Cmd::BarClick(hmon, local));
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
    // Last cursor position we evaluated. Poll fast (~1 frame) for a snappy hover,
    // but only run the expensive WindowFromPoint + MANAGED lock when the cursor
    // actually moved — a still cursor costs one GetCursorPos per tick and bails.
    let mut last_pt = POINT { x: i32::MIN, y: i32::MIN };
    loop {
        std::thread::sleep(std::time::Duration::from_millis(16));
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
            // Inside the post-switch / post-keyboard-focus settle window: don't
            // fight the programmatic focus. Sync last_pt so that once the guard
            // expires only a genuine cursor move (not this stale position) fires.
            if now_ms() < FOLLOW_SETTLE_MS.load(Ordering::Relaxed) {
                last_pt = pt;
                continue;
            }
            // Cursor hasn't moved since the last tick — nothing to resolve.
            if pt.x == last_pt.x && pt.y == last_pt.y {
                continue;
            }
            last_pt = pt;
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

/// Push the config values the bar paint path and stats worker read from atomics
/// (so they need no Config in hand). Call at startup and on every reload.
fn apply_bar_statics(cfg: &Config) {
    BAR_HEIGHT.store(
        if cfg.bar_enabled {
            cfg.bar_height as isize
        } else {
            0
        },
        Ordering::Relaxed,
    );
    BAR_BOTTOM.store(cfg.bar_bottom, Ordering::Relaxed);
    BAR_FONT_SIZE.store(cfg.bar_font_size as isize, Ordering::Relaxed);
    BAR_PADDING.store(cfg.bar_padding as isize, Ordering::Relaxed);
    *BAR_FONT_NAME.lock().unwrap() = cfg.bar_font_name.clone();
    STATS_ON.store(
        cfg.bar_show_cpu || cfg.bar_show_mem || cfg.bar_show_battery,
        Ordering::Relaxed,
    );
}

/// Watch the two config files and apply changes live, so editing + saving a
/// config takes effect without restarting Astur.
fn config_watcher() {
    use std::time::SystemTime;
    let wm = config_path("ASTUR_CONFIG", "astur.conf");
    let nav = config_path("ASTUR_NAVBAR", "navbar.conf");
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
        apply_bar_statics(&cfg);
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
            pending_launch_mon: 0,
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

// =========================================================================
// App launcher (Alt+Space): omarchy/rofi-style centered picker.
//
// Driven entirely through the LL keyboard hook, so it never needs foreground
// focus (no foreground-lock dance): the hook posts intents to the launcher
// window, whose wndproc owns all state and repaints. v1 source is Start Menu
// .lnk/.url shortcuts; file search (Windows Search index) is planned — see
// plan/launcher.md.
// =========================================================================

// Custom message: wParam = action (LA_*), lParam = char (for LA_CHAR).
const WM_LAUNCHER: u32 = WM_USER + 10;
const LA_OPEN: usize = 0;
const LA_CHAR: usize = 1;
const LA_BACK: usize = 2;
const LA_UP: usize = 3;
const LA_DOWN: usize = 4;
const LA_ACTIVATE: usize = 5;
const LA_CLOSE: usize = 6;
const LA_TAB: usize = 7; // toggle the file detail footer
const LA_ACTIVATE_ALT: usize = 8; // Shift+Enter: open a file's containing folder

// Theme (COLORREF is 0x00BBGGRR). Forte blue #366382 accent on a dark surface;
// minimal chrome (thin frame, subtle divider) for a clean omarchy/rofi look.
const LAUNCHER_BG: u32 = 0x0016_1616;
const LAUNCHER_FG: u32 = 0x00E6_E6E6;
const LAUNCHER_DIM: u32 = 0x0089_8989;
const LAUNCHER_SELBG: u32 = 0x0082_6333; // #366382
const LAUNCHER_SELFG: u32 = 0x00FF_FFFF;
const LAUNCHER_FRAME: u32 = 0x0033_2A26; // subtle blue-tinted 1px frame
const LAUNCHER_DIVIDER: u32 = 0x0029_2929; // muted divider under the query row
const LAUNCHER_W: i32 = 660;
const LAUNCHER_H: i32 = 452;
const LAUNCHER_ROW_H: i32 = 40;
const LAUNCHER_PAD: i32 = 16;
const LAUNCHER_HEADER: i32 = 54; // query row height
const LAUNCHER_ICON_PX: i32 = 28; // per-row app icon box
const LAUNCHER_SEL_RADIUS: i32 = 12; // rounded selection pill

static LAUNCHER_OPEN: AtomicBool = AtomicBool::new(false);
static LAUNCHER_HWND: AtomicIsize = AtomicIsize::new(0);
static LAUNCHER_FONT: AtomicIsize = AtomicIsize::new(0);

// Launcher window bounds (screen coords), published on show so the global mouse
// hook can detect a click OUTSIDE the picker and dismiss it without a focus grab.
static LAUNCHER_RECT_L: AtomicI32 = AtomicI32::new(0);
static LAUNCHER_RECT_T: AtomicI32 = AtomicI32::new(0);
static LAUNCHER_RECT_R: AtomicI32 = AtomicI32::new(0);
static LAUNCHER_RECT_B: AtomicI32 = AtomicI32::new(0);

// Lazy icon loader: paint enqueues visible app indices needing an icon; the icon
// worker resolves the shell icon to an HBITMAP off the UI thread and repaints.
static ICON_QUEUE: Mutex<VecDeque<usize>> = Mutex::new(VecDeque::new());
static ICON_CV: Condvar = Condvar::new();

struct AppEntry {
    name: String,
    name_lc: String,
    path: String,    // .lnk/.url file path, or `shell:AppsFolder\<id>` for UWP/system apps
    icon: isize,     // 0 = not yet loaded, -1 = none/failed, else an HBITMAP (32bpp ARGB)
}
/// One file/folder result from the Windows Search index (Phase 3).
struct FileHit {
    name: String,
    path: String,
    size: i64,  // bytes (-1 = unknown / folder)
    date: f64,  // OLE automation date (days since 1899-12-30); 0 = unknown
}
/// A visible result row: an app (index into `all`) or a file (index into `files`).
#[derive(Clone, Copy)]
enum Hit {
    App(usize),
    File(usize),
}
struct LauncherState {
    query: String,
    all: Vec<AppEntry>,
    files: Vec<FileHit>,   // current file-search results (top-N, replaced per query)
    filtered: Vec<Hit>,    // merged app + file rows, best first
    sel: usize,
    loaded: bool,
    detail: bool,          // Tab: show the expanded detail footer for a file row
    search_gen: u64,       // generation of `files` (drops stale async results)
}
static LAUNCHER_STATE: Mutex<LauncherState> = Mutex::new(LauncherState {
    query: String::new(),
    all: Vec::new(),
    files: Vec::new(),
    filtered: Vec::new(),
    sel: 0,
    loaded: false,
    detail: false,
    search_gen: 0,
});

// File-search request hand-off to `filesearch_worker` (debounced + cancellable).
static SEARCH_REQ: Mutex<Option<(u64, String)>> = Mutex::new(None);
static SEARCH_CV: Condvar = Condvar::new();
static SEARCH_GEN: AtomicU64 = AtomicU64::new(0);

/// Recursively collect `*.lnk` / `*.url` under a Start Menu root into `out`,
/// keyed by lowercased display name so per-user shadows all-users duplicates.
fn collect_shortcuts(dir: &std::path::Path, out: &mut std::collections::HashMap<String, AppEntry>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for ent in rd.flatten() {
        let p = ent.path();
        if p.is_dir() {
            collect_shortcuts(&p, out);
        } else if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
            let ext = ext.to_ascii_lowercase();
            if ext == "lnk" || ext == "url" {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    let name = stem.to_string();
                    let key = name.to_ascii_lowercase();
                    out.entry(key.clone()).or_insert(AppEntry {
                        name,
                        name_lc: key,
                        path: p.to_string_lossy().into_owned(),
                        icon: 0,
                    });
                }
            }
        }
    }
}

/// Read one `SIGDN` display string from a shell item, freeing the COM buffer.
unsafe fn sigdn(item: &IShellItem, kind: windows::Win32::UI::Shell::SIGDN) -> String {
    match item.GetDisplayName(kind) {
        Ok(p) => {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const c_void));
            s
        }
        Err(_) => String::new(),
    }
}

/// Enumerate the shell `AppsFolder` — the "All apps" list Start shows — into `out`,
/// keyed by lowercased display name. This is what pulls in UWP/system apps that
/// have no Start Menu `.lnk` (Notepad, Calculator, Settings, Store apps, …), so the
/// picker can replace pressing Start and typing an app name. Each entry launches
/// via `shell:AppsFolder\<id>` (works for Win32 and UWP through `ShellExecuteW`).
/// `.lnk` entries are inserted first and win the dedup (their launch is rock-solid),
/// so AppsFolder only fills the gaps. Requires COM initialised on this thread.
unsafe fn enumerate_appsfolder(out: &mut std::collections::HashMap<String, AppEntry>) {
    let parsing: Vec<u16> = "shell:AppsFolder"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let folder: windows::core::Result<IShellItem> =
        SHCreateItemFromParsingName(PCWSTR(parsing.as_ptr()), None);
    let Ok(folder) = folder else { return };
    let en: windows::core::Result<IEnumShellItems> = folder.BindToHandler(None, &BHID_EnumItems);
    let Ok(en) = en else { return };
    loop {
        let mut arr: [Option<IShellItem>; 1] = [None];
        let mut fetched = 0u32;
        if en.Next(&mut arr, Some(&mut fetched)).is_err() || fetched == 0 {
            break;
        }
        let Some(item) = arr[0].take() else { break };
        let name = sigdn(&item, SIGDN_NORMALDISPLAY);
        if name.is_empty() {
            continue;
        }
        let child = sigdn(&item, SIGDN_PARENTRELATIVEPARSING);
        if child.is_empty() {
            continue;
        }
        let key = name.to_ascii_lowercase();
        if !out.contains_key(&key) {
            // The AppsFolder id is usually an AUMID (UWP, e.g. `Microsoft.Windows
            // Notepad_...!App`) — launch via `shell:AppsFolder\<aumid>`. Some Win32
            // entries expose a real exe path as their id instead; launch that
            // directly (ShellExecute on the file is the robust path).
            let path = if child.contains(":\\") && std::path::Path::new(&child).exists() {
                child.clone()
            } else {
                format!(r"shell:AppsFolder\{child}")
            };
            out.insert(
                key.clone(),
                AppEntry {
                    name,
                    name_lc: key,
                    path,
                    icon: 0,
                },
            );
        }
    }
}

/// Enumerate installed apps: Start Menu `.lnk`/`.url` first (reliable launch), then
/// the AppsFolder for everything else (UWP/system apps). Sorted by display name.
fn launcher_enumerate() -> Vec<AppEntry> {
    let mut map = std::collections::HashMap::new();
    // Per-user first so it wins the dedup, then all-users.
    if let Ok(appdata) = std::env::var("APPDATA") {
        let mut p = std::path::PathBuf::from(appdata);
        p.push(r"Microsoft\Windows\Start Menu\Programs");
        collect_shortcuts(&p, &mut map);
    }
    if let Ok(pd) = std::env::var("ProgramData") {
        let mut p = std::path::PathBuf::from(pd);
        p.push(r"Microsoft\Windows\Start Menu\Programs");
        collect_shortcuts(&p, &mut map);
    }
    unsafe { enumerate_appsfolder(&mut map) };
    let mut v: Vec<AppEntry> = map.into_values().collect();
    v.sort_by(|a, b| a.name_lc.cmp(&b.name_lc));
    v
}

/// Resolve a shell item's icon to a 32bpp ARGB HBITMAP at `px` square. Returns the
/// HBITMAP as an isize, or -1 on failure. Runs on the icon worker (slow shell call
/// off the UI thread). Requires COM initialised on the calling thread.
unsafe fn load_icon(path: &str, px: i32) -> isize {
    let mut w: Vec<u16> = path.encode_utf16().collect();
    w.push(0);
    let factory: windows::core::Result<IShellItemImageFactory> =
        SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None);
    let Ok(factory) = factory else { return -1 };
    match factory.GetImage(SIZE { cx: px, cy: px }, SIIGBF_ICONONLY) {
        Ok(hb) => hb.0 as isize,
        Err(_) => -1,
    }
}

/// Icon worker: drains `ICON_QUEUE`, resolves each app's shell icon to an HBITMAP,
/// stores it on the entry, and repaints. Off the UI thread so a slow icon (UWP
/// logo, network path) never stalls typing. One apartment for its lifetime.
fn icon_worker() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        loop {
            let idx = {
                let mut q = ICON_QUEUE.lock().unwrap();
                loop {
                    if let Some(i) = q.pop_front() {
                        break i;
                    }
                    q = ICON_CV.wait(q).unwrap();
                }
            };
            // Short lock: grab the path only if this entry still needs an icon.
            let path = {
                let st = LAUNCHER_STATE.lock().unwrap();
                match st.all.get(idx) {
                    Some(e) if e.icon == 0 => e.path.clone(),
                    _ => continue,
                }
            };
            let hbmp = load_icon(&path, LAUNCHER_ICON_PX);
            {
                let mut st = LAUNCHER_STATE.lock().unwrap();
                if let Some(e) = st.all.get_mut(idx) {
                    e.icon = hbmp;
                }
            }
            let hl = LAUNCHER_HWND.load(Ordering::Relaxed);
            if hl != 0 {
                let _ = InvalidateRect(hwnd_from(hl), None, BOOL(0));
            }
        }
    }
}

/// Fuzzy subsequence score for `query` against `cand` (both lowercase). None if
/// not all query chars appear in order. Higher = better: contiguous runs,
/// word-boundary starts, and earlier/shorter matches score up.
fn fuzzy_score(query: &str, cand: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let cb = cand.as_bytes();
    let mut qi = query.chars();
    let mut want = qi.next();
    let mut score = 0i32;
    let mut run = 0i32;
    let mut matched_first = false;
    for (i, &c) in cb.iter().enumerate() {
        let Some(w) = want else { break };
        let is_boundary = i == 0 || cb[i - 1] == b' ' || cb[i - 1] == b'-' || cb[i - 1] == b'_';
        if (c as char).eq_ignore_ascii_case(&w) {
            if i == 0 {
                matched_first = true;
            }
            run += 1;
            score += 8 + run * 4; // reward contiguous runs
            if is_boundary {
                score += 12; // reward start-of-word matches
            }
            score -= (i as i32) / 4; // earlier matches slightly better
            want = qi.next();
        } else {
            run = 0;
        }
    }
    if want.is_some() {
        return None; // ran out of candidate before matching all query chars
    }
    score -= cand.len() as i32 / 8; // shorter targets slightly better
    if matched_first {
        score += 10;
    }
    Some(score)
}

/// Recompute `filtered` (and clamp `sel`) for the current query.
fn launcher_refilter(st: &mut LauncherState) {
    let q = st.query.to_ascii_lowercase();
    let mut scored: Vec<(i32, usize)> = st
        .all
        .iter()
        .enumerate()
        .filter_map(|(i, e)| fuzzy_score(&q, &e.name_lc).map(|s| (s, i)))
        .collect();
    // Best score first; ties keep alphabetical order (all is pre-sorted, stable).
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    let mut filtered: Vec<Hit> = scored.into_iter().map(|(_, i)| Hit::App(i)).collect();
    // File results (from the async index worker, already query-filtered) after apps —
    // apps are instant and the common case, so they never wait on the index.
    for i in 0..st.files.len() {
        filtered.push(Hit::File(i));
    }
    st.filtered = filtered;
    if st.sel >= st.filtered.len() {
        st.sel = st.filtered.len().saturating_sub(1);
    }
}

/// Bump the search generation and hand the current query to `filesearch_worker`.
/// Cheap; the worker debounces + drops stale generations.
fn launcher_dispatch_search(query: &str) {
    let gen = SEARCH_GEN.fetch_add(1, Ordering::Relaxed) + 1;
    *SEARCH_REQ.lock().unwrap() = Some((gen, query.to_string()));
    SEARCH_CV.notify_one();
}

// ----- file search (Windows Search index via OLE DB Search.CollatorDSO) --------

/// Mixed-type OLE DB row buffer: path (WSTR|BYREF provider ptr), size (I8), date
/// (automation DATE f64), each with a DBSTATUS. `repr(C)` so the binding offsets
/// below are exact.
#[repr(C)]
struct SearchRow {
    s_path: u32,
    _p0: u32,
    path: *mut u16, // @8
    s_size: u32,
    _p1: u32,
    size: i64, // @24
    s_date: u32,
    _p2: u32,
    date: f64, // @40
}

unsafe fn read_wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0;
    while *p.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
}

/// Keep only real filesystem paths (`X:\…` or UNC). The index also returns Outlook
/// items as `/account@dom/Folder/Subject` — not launchable as files.
fn is_fs_path(p: &str) -> bool {
    let b = p.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\')
        || p.starts_with("\\\\")
}

/// Build a full-text `CONTAINS` argument from the query — each ≥2-char word becomes a
/// prefix term (`"word*"`) and they're AND-ed, so "annual report" matches files whose
/// name contains words starting "annual" AND "report". `CONTAINS` hits the full-text
/// index (~100ms) vs a leading-wildcard `LIKE '%q%'` which scans the whole index
/// (~900ms). Returns None if there's no usable term. Words are stripped of `"`/`'`
/// (phrase/SQL hazards) so the resulting `'…'` literal is safe.
fn build_contains(query: &str) -> Option<String> {
    let words: Vec<String> = query
        .split_whitespace()
        .map(|w| w.chars().filter(|c| *c != '"' && *c != '\'').collect::<String>())
        .filter(|w| w.chars().count() >= 2)
        .map(|w| format!("\"{w}*\""))
        .collect();
    if words.is_empty() {
        None
    } else {
        Some(words.join(" AND "))
    }
}

fn fmt_size(bytes: i64) -> String {
    if bytes < 0 {
        return String::new();
    }
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", b / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", b / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// OLE automation date (days since 1899-12-30) → `YYYY-MM-DD HH:MM`.
fn fmt_oadate(d: f64) -> String {
    if d <= 0.0 {
        return String::new();
    }
    let unix_days = d.trunc() as i64 - 25569; // 1899-12-30 → 1970-01-01 offset
    let frac = d - d.trunc();
    let secs = (frac * 86400.0).round() as i64;
    let (y, m, day) = civil_from_days(unix_days);
    format!("{y:04}-{m:02}-{day:02} {:02}:{:02}", secs / 3600, (secs % 3600) / 60)
}

/// A live connection to the Windows Search index. Created once on the worker
/// thread; each query reuses the session (a fresh command per query).
struct FileSearch {
    _dbinit: IDBInitialize, // held to keep the data source initialised
    create_cmd: IDBCreateCommand,
}
impl FileSearch {
    unsafe fn new() -> Option<FileSearch> {
        let connstr = "Provider=Search.CollatorDSO;Extended Properties='Application=Windows'";
        let mut cs: Vec<u16> = connstr.encode_utf16().chain(std::iter::once(0)).collect();
        let init: IDataInitialize =
            CoCreateInstance(&MSDAINITIALIZE, None, CLSCTX_INPROC_SERVER).ok()?;
        let mut ds: Option<IUnknown> = None;
        init.GetDataSource(
            None,
            CLSCTX_INPROC_SERVER.0 as u32,
            PCWSTR(cs.as_mut_ptr()),
            &IDBInitialize::IID,
            &mut ds,
        )
        .ok()?;
        let dbinit: IDBInitialize = ds?.cast().ok()?;
        dbinit.Initialize().ok()?;
        let session: IDBCreateSession = dbinit.cast().ok()?;
        let sess_unk: IUnknown = session.CreateSession(None, &IDBCreateCommand::IID).ok()?;
        let create_cmd: IDBCreateCommand = sess_unk.cast().ok()?;
        Some(FileSearch {
            _dbinit: dbinit,
            create_cmd,
        })
    }

    unsafe fn run(&self, query: &str) -> Vec<FileHit> {
        let mut out = Vec::new();
        let Some(contains) = build_contains(query) else {
            return out; // no ≥2-char term — would match almost everything
        };
        // CONTAINS = full-text index (fast). LIKE '%q%' scans the index (~900ms, too
        // slow). Word-prefix match, not pure substring — see plan/launcher.md / the
        // MFT path for true Everything-style substring.
        let sql = format!(
            "SELECT TOP 40 System.ItemPathDisplay, System.Size, System.DateModified \
             FROM SYSTEMINDEX WHERE CONTAINS(System.FileName, '{contains}') \
             ORDER BY System.DateModified DESC"
        );
        let _ = self.exec(&sql, &mut out);
        out
    }

    unsafe fn exec(&self, sql: &str, out: &mut Vec<FileHit>) -> windows::core::Result<()> {
        let cmd_unk: IUnknown = self.create_cmd.CreateCommand(None, &ICommandText::IID)?;
        let cmd_text: ICommandText = cmd_unk.cast()?;
        let dbguid_default = GUID::from_u128(0xC8B521FB_5CF3_11CE_ADE5_00AA0044773D);
        let mut sqlw: Vec<u16> = sql.encode_utf16().chain(std::iter::once(0)).collect();
        cmd_text.SetCommandText(&dbguid_default, PCWSTR(sqlw.as_mut_ptr()))?;
        let cmd: ICommand = cmd_text.cast()?;
        let mut rowset_unk: Option<IUnknown> = None;
        cmd.Execute(None, &IRowset::IID, None, None, Some(&mut rowset_unk))?;
        let rowset: IRowset = rowset_unk.unwrap().cast()?;
        let accessor: IAccessor = rowset.cast()?;

        let mk = |ord: usize, obs: usize, obv: usize, wt: u16, mo: u32, cb: usize| DBBINDING {
            iOrdinal: ord,
            obValue: obv,
            obLength: 0,
            obStatus: obs,
            pTypeInfo: core::mem::ManuallyDrop::new(None),
            pObject: std::ptr::null_mut(),
            pBindExt: std::ptr::null_mut(),
            dwPart: (DBPART_VALUE.0 | DBPART_STATUS.0) as u32,
            dwMemOwner: mo,
            eParamIO: DBPARAMIO_NOTPARAM.0 as u32,
            cbMaxLen: cb,
            dwFlags: 0,
            wType: wt,
            bPrecision: 0,
            bScale: 0,
        };
        let prov = DBMEMOWNER_PROVIDEROWNED.0 as u32;
        let bindings = [
            mk(1, 0, 8, (DBTYPE_WSTR.0 | DBTYPE_BYREF.0) as u16, prov, 0),
            mk(2, 16, 24, DBTYPE_I8.0 as u16, 0, 8),
            mk(3, 32, 40, DBTYPE_DATE.0 as u16, 0, 8),
        ];
        let mut hacc = HACCESSOR::default();
        accessor.CreateAccessor(
            DBACCESSOR_ROWDATA.0 as u32,
            bindings.len(),
            bindings.as_ptr(),
            std::mem::size_of::<SearchRow>(),
            &mut hacc,
            None,
        )?;

        loop {
            let mut rows: [*mut usize; 1] = [std::ptr::null_mut()];
            let mut obtained: usize = 0;
            if rowset.GetNextRows(0, 0, &mut obtained, &mut rows).is_err() || obtained == 0 {
                break;
            }
            let hrow_arr = rows[0];
            let hrow = *hrow_arr;
            let mut row = SearchRow {
                s_path: 0, _p0: 0, path: std::ptr::null_mut(),
                s_size: 0, _p1: 0, size: 0,
                s_date: 0, _p2: 0, date: 0.0,
            };
            if rowset
                .GetData(hrow, hacc, &mut row as *mut SearchRow as *mut c_void)
                .is_ok()
            {
                let ok = DBSTATUS_S_OK.0 as u32;
                let path = if row.s_path == ok { read_wide(row.path) } else { String::new() };
                if is_fs_path(&path) {
                    let size = if row.s_size == ok { row.size } else { -1 };
                    let date = if row.s_date == ok { row.date } else { 0.0 };
                    let name = std::path::Path::new(&path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(&path)
                        .to_string();
                    out.push(FileHit { name, path, size, date });
                }
            }
            let _ = rowset.ReleaseRows(obtained, hrow_arr as *const usize, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut());
            CoTaskMemFree(Some(hrow_arr as *const c_void));
            if out.len() >= 40 {
                break;
            }
        }
        let _ = accessor.ReleaseAccessor(hacc, None);
        Ok(())
    }
}

/// File-search worker: own COM STA + one persistent index connection. Drains the
/// debounced request slot, drops stale generations, writes results + repaints.
/// If the index can't be opened, file search is silently disabled (apps still work).
fn filesearch_worker() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let search = FileSearch::new();
        loop {
            let (gen, q) = {
                let mut slot = SEARCH_REQ.lock().unwrap();
                loop {
                    if let Some(r) = slot.take() {
                        break r;
                    }
                    slot = SEARCH_CV.wait(slot).unwrap();
                }
            };
            // Short debounce to coalesce bursts; CONTAINS is fast (~100ms) so this can
            // be tight without spamming the index.
            std::thread::sleep(std::time::Duration::from_millis(45));
            if SEARCH_GEN.load(Ordering::Relaxed) != gen {
                continue;
            }
            let Some(search) = search.as_ref() else { continue };
            let hits = search.run(&q);
            if SEARCH_GEN.load(Ordering::Relaxed) != gen {
                continue; // superseded while the index query ran
            }
            {
                let mut st = LAUNCHER_STATE.lock().unwrap();
                st.files = hits;
                st.search_gen = gen;
                launcher_refilter(&mut st);
            }
            let hl = LAUNCHER_HWND.load(Ordering::Relaxed);
            if hl != 0 {
                let _ = InvalidateRect(hwnd_from(hl), None, BOOL(0));
            }
        }
    }
}

/// Build the launcher font once (semibold, ~18px).
unsafe fn make_launcher_font() {
    if LAUNCHER_FONT.load(Ordering::Relaxed) != 0 {
        return;
    }
    let mut wname: Vec<u16> = "Segoe UI".encode_utf16().collect();
    wname.push(0);
    let f = CreateFontW(
        -19,
        0,
        0,
        0,
        600,
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        0,
        PCWSTR(wname.as_ptr()),
    );
    LAUNCHER_FONT.store(f.0 as isize, Ordering::Relaxed);
}

/// Center the launcher on the monitor under the cursor and show it (no-activate
/// — we drive it via the keyboard hook, so it must not steal focus).
unsafe fn launcher_show(h: HWND) {
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let wa = work_area_at(pt);
    let x = (wa.left + wa.right) / 2 - LAUNCHER_W / 2;
    let y = (wa.top + wa.bottom) / 2 - LAUNCHER_H / 2;
    let _ = SetWindowPos(h, HWND_TOPMOST, x, y, LAUNCHER_W, LAUNCHER_H, SWP_NOACTIVATE);
    // Publish bounds for the mouse hook's click-outside-to-dismiss check.
    LAUNCHER_RECT_L.store(x, Ordering::Relaxed);
    LAUNCHER_RECT_T.store(y, Ordering::Relaxed);
    LAUNCHER_RECT_R.store(x + LAUNCHER_W, Ordering::Relaxed);
    LAUNCHER_RECT_B.store(y + LAUNCHER_H, Ordering::Relaxed);
    let _ = ShowWindow(h, SW_SHOWNA);
    let _ = InvalidateRect(h, None, BOOL(0));
}

/// Hide the launcher and reset transient state.
unsafe fn launcher_close(h: HWND) {
    let _ = ShowWindow(h, SW_HIDE);
    LAUNCHER_OPEN.store(false, Ordering::Relaxed);
    let mut st = LAUNCHER_STATE.lock().unwrap();
    st.query.clear();
    st.sel = 0;
    st.files.clear();
    st.detail = false;
}

/// Launch the selected shortcut/app/file via the shell (resolves target/args/dir).
unsafe fn launcher_launch(path: &str) {
    let mut wpath: Vec<u16> = path.encode_utf16().collect();
    wpath.push(0);
    let mut op: Vec<u16> = "open".encode_utf16().collect();
    op.push(0);
    ShellExecuteW(
        HWND(std::ptr::null_mut()),
        PCWSTR(op.as_ptr()),
        PCWSTR(wpath.as_ptr()),
        PCWSTR::null(),
        PCWSTR::null(),
        SW_SHOW,
    );
}

/// Open Explorer with the file selected (Shift+Enter on a file result).
unsafe fn launcher_reveal_in_folder(path: &str) {
    let file: Vec<u16> = "explorer.exe".encode_utf16().chain(std::iter::once(0)).collect();
    let params = format!("/select,\"{path}\"");
    let pw: Vec<u16> = params.encode_utf16().chain(std::iter::once(0)).collect();
    let op: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    ShellExecuteW(
        HWND(std::ptr::null_mut()),
        PCWSTR(op.as_ptr()),
        PCWSTR(file.as_ptr()),
        PCWSTR(pw.as_ptr()),
        PCWSTR::null(),
        SW_SHOW,
    );
}

unsafe fn launcher_paint(h: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(h, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(h, &mut rc);
    let w = rc.right - rc.left;
    let ht = rc.bottom - rc.top;

    // Thin 1px frame, then the dark surface inset inside it (DWM rounds the outer
    // corners, so this reads as a clean bordered card).
    let frame = CreateSolidBrush(COLORREF(LAUNCHER_FRAME));
    FillRect(hdc, &rc, frame);
    let _ = DeleteObject(HGDIOBJ(frame.0));
    let inner = RECT {
        left: rc.left + 1,
        top: rc.top + 1,
        right: rc.right - 1,
        bottom: rc.bottom - 1,
    };
    let bg = CreateSolidBrush(COLORREF(LAUNCHER_BG));
    FillRect(hdc, &inner, bg);
    let _ = DeleteObject(HGDIOBJ(bg.0));

    let font_raw = LAUNCHER_FONT.load(Ordering::Relaxed);
    let old_font = if font_raw != 0 {
        Some(SelectObject(hdc, HGDIOBJ(font_raw as *mut c_void)))
    } else {
        Some(SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT)))
    };
    SetBkMode(hdc, TRANSPARENT);

    let st = LAUNCHER_STATE.lock().unwrap();

    // Query row.
    let mut qr = RECT {
        left: LAUNCHER_PAD,
        top: 0,
        right: w - LAUNCHER_PAD,
        bottom: LAUNCHER_HEADER,
    };
    if st.query.is_empty() {
        SetTextColor(hdc, COLORREF(LAUNCHER_DIM));
        let mut v: Vec<u16> = "Search apps and files…".encode_utf16().collect();
        DrawTextW(hdc, &mut v, &mut qr, DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    } else {
        SetTextColor(hdc, COLORREF(LAUNCHER_FG));
        // Trailing caret marks the input (the picker is owner-drawn, no edit ctrl).
        let mut v: Vec<u16> = format!("{}\u{258f}", st.query).encode_utf16().collect();
        DrawTextW(hdc, &mut v, &mut qr, DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    }
    // Divider under the query row.
    let div = RECT {
        left: LAUNCHER_PAD,
        top: LAUNCHER_HEADER,
        right: w - LAUNCHER_PAD,
        bottom: LAUNCHER_HEADER + 1,
    };
    let dbrush = CreateSolidBrush(COLORREF(LAUNCHER_DIVIDER));
    FillRect(hdc, &div, dbrush);
    let _ = DeleteObject(HGDIOBJ(dbrush.0));

    // Result rows, scrolled so the selection is always visible.
    let list_top = LAUNCHER_HEADER + 6;
    // Tab detail footer: when on and the selection is a file, reserve a panel at the
    // bottom (path / modified / size) and shrink the list to fit.
    let sel_file = match st.filtered.get(st.sel) {
        Some(Hit::File(i)) => st.files.get(*i),
        _ => None,
    };
    let footer_h = if st.detail && sel_file.is_some() { 80 } else { 0 };
    let list_bottom = ht - footer_h - 4;
    let rows = ((list_bottom - list_top) / LAUNCHER_ROW_H).max(1) as usize;
    let scroll = if st.sel >= rows { st.sel - rows + 1 } else { 0 };
    let icon_dc = CreateCompatibleDC(hdc);
    // AC_SRC_OVER + AC_SRC_ALPHA: blend the 32bpp icon by its own per-pixel alpha.
    let icon_blend = BLENDFUNCTION {
        BlendOp: 0,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: 1,
    };
    let text_left = LAUNCHER_PAD + 6 + LAUNCHER_ICON_PX + 10;
    let mut want: Vec<usize> = Vec::new(); // app indices whose icon isn't loaded yet
    for vis in 0..rows {
        let idx = scroll + vis;
        if idx >= st.filtered.len() {
            break;
        }
        let hit = st.filtered[idx];
        let top = list_top + vis as i32 * LAUNCHER_ROW_H;
        let row = RECT {
            left: LAUNCHER_PAD,
            top,
            right: w - LAUNCHER_PAD,
            bottom: top + LAUNCHER_ROW_H,
        };
        if idx == st.sel {
            // Rounded accent pill, inset from the row edges (omarchy-style).
            let sel = CreateSolidBrush(COLORREF(LAUNCHER_SELBG));
            let pen = CreatePen(PS_SOLID, 1, COLORREF(LAUNCHER_SELBG));
            let ob = SelectObject(hdc, HGDIOBJ(sel.0));
            let op = SelectObject(hdc, HGDIOBJ(pen.0));
            let _ = RoundRect(
                hdc,
                row.left + 4,
                top + 3,
                row.right - 4,
                top + LAUNCHER_ROW_H - 3,
                LAUNCHER_SEL_RADIUS,
                LAUNCHER_SEL_RADIUS,
            );
            SelectObject(hdc, ob);
            SelectObject(hdc, op);
            let _ = DeleteObject(HGDIOBJ(sel.0));
            let _ = DeleteObject(HGDIOBJ(pen.0));
            SetTextColor(hdc, COLORREF(LAUNCHER_SELFG));
        } else {
            SetTextColor(hdc, COLORREF(LAUNCHER_FG));
        }
        // Resolve name + (apps only) lazy icon.
        let name: &str = match hit {
            Hit::App(i) => {
                let e = &st.all[i];
                // App icon, loaded lazily off the UI thread; missing ones queue + pop in.
                if e.icon > 1 {
                    let hb = e.icon as *mut c_void;
                    let prev = SelectObject(icon_dc, HGDIOBJ(hb));
                    let mut bm = BITMAP::default();
                    GetObjectW(
                        HGDIOBJ(hb),
                        std::mem::size_of::<BITMAP>() as i32,
                        Some(&mut bm as *mut _ as *mut c_void),
                    );
                    let iy = top + (LAUNCHER_ROW_H - LAUNCHER_ICON_PX) / 2;
                    let _ = AlphaBlend(
                        hdc, row.left + 6, iy, LAUNCHER_ICON_PX, LAUNCHER_ICON_PX, icon_dc, 0, 0,
                        bm.bmWidth.max(1), bm.bmHeight.max(1), icon_blend,
                    );
                    SelectObject(icon_dc, prev);
                } else if e.icon == 0 {
                    want.push(i);
                }
                e.name.as_str()
            }
            Hit::File(i) => {
                // File rows: a small dim square marks them (no shell icon yet); the
                // Tab footer shows path/size/date.
                if idx != st.sel {
                    let mk = CreateSolidBrush(COLORREF(LAUNCHER_DIM));
                    let g = RECT {
                        left: row.left + 6 + (LAUNCHER_ICON_PX - 14) / 2,
                        top: top + (LAUNCHER_ROW_H - 14) / 2,
                        right: row.left + 6 + (LAUNCHER_ICON_PX - 14) / 2 + 14,
                        bottom: top + (LAUNCHER_ROW_H - 14) / 2 + 14,
                    };
                    FillRect(hdc, &g, mk);
                    let _ = DeleteObject(HGDIOBJ(mk.0));
                }
                st.files[i].name.as_str()
            }
        };
        let mut tr = RECT {
            left: text_left,
            ..row
        };
        let mut v: Vec<u16> = name.encode_utf16().collect();
        DrawTextW(
            hdc,
            &mut v,
            &mut tr,
            DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
    }
    let _ = DeleteDC(icon_dc);

    // Detail footer for the selected file (Tab toggles it).
    if footer_h > 0 {
        if let Some(f) = sel_file {
            let fy = ht - footer_h;
            let divf = RECT { left: LAUNCHER_PAD, top: fy, right: w - LAUNCHER_PAD, bottom: fy + 1 };
            let db = CreateSolidBrush(COLORREF(LAUNCHER_DIVIDER));
            FillRect(hdc, &divf, db);
            let _ = DeleteObject(HGDIOBJ(db.0));
            let line = |n: i32, label: &str, color: u32| {
                SetTextColor(hdc, COLORREF(color));
                let mut r = RECT {
                    left: LAUNCHER_PAD + 2,
                    top: fy + 8 + n * 22,
                    right: w - LAUNCHER_PAD,
                    bottom: fy + 8 + n * 22 + 22,
                };
                let mut v: Vec<u16> = label.encode_utf16().collect();
                DrawTextW(hdc, &mut v, &mut r, DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
            };
            line(0, &f.path, LAUNCHER_FG);
            let meta = format!("Modified {}    Size {}", fmt_oadate(f.date), fmt_size(f.size));
            line(1, &meta, LAUNCHER_DIM);
            line(2, "Enter: open    Shift+Enter: open folder", LAUNCHER_DIM);
        }
    }

    if let Some(of) = old_font {
        SelectObject(hdc, of);
    }
    drop(st);
    // Queue any visible rows still missing an icon; the icon worker resolves them.
    if !want.is_empty() {
        let mut q = ICON_QUEUE.lock().unwrap();
        for idx in want {
            if !q.contains(&idx) {
                q.push_back(idx);
            }
        }
        drop(q);
        ICON_CV.notify_all();
    }
    let _ = EndPaint(h, &ps);
}

unsafe extern "system" fn launcher_wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_LAUNCHER => {
            match w.0 {
                LA_OPEN => {
                    {
                        let mut st = LAUNCHER_STATE.lock().unwrap();
                        if !st.loaded {
                            st.all = launcher_enumerate();
                            st.loaded = true;
                        }
                        st.query.clear();
                        st.sel = 0;
                        st.files.clear();
                        st.detail = false;
                        launcher_refilter(&mut st);
                    }
                    launcher_show(h);
                }
                LA_CHAR => {
                    let q = {
                        let mut st = LAUNCHER_STATE.lock().unwrap();
                        if let Some(c) = char::from_u32(l.0 as u32) {
                            st.query.push(c);
                        }
                        st.sel = 0;
                        st.files.clear(); // stale results vanish until the new query returns
                        launcher_refilter(&mut st);
                        st.query.clone()
                    };
                    launcher_dispatch_search(&q);
                    let _ = InvalidateRect(h, None, BOOL(0));
                }
                LA_BACK => {
                    let q = {
                        let mut st = LAUNCHER_STATE.lock().unwrap();
                        st.query.pop();
                        st.sel = 0;
                        st.files.clear();
                        launcher_refilter(&mut st);
                        st.query.clone()
                    };
                    launcher_dispatch_search(&q);
                    let _ = InvalidateRect(h, None, BOOL(0));
                }
                LA_UP => {
                    {
                        let mut st = LAUNCHER_STATE.lock().unwrap();
                        if st.sel > 0 {
                            st.sel -= 1;
                        }
                    }
                    let _ = InvalidateRect(h, None, BOOL(0));
                }
                LA_DOWN => {
                    {
                        let mut st = LAUNCHER_STATE.lock().unwrap();
                        if st.sel + 1 < st.filtered.len() {
                            st.sel += 1;
                        }
                    }
                    let _ = InvalidateRect(h, None, BOOL(0));
                }
                LA_ACTIVATE => {
                    // Enter: launch the selected app, or open the selected file.
                    let action = {
                        let st = LAUNCHER_STATE.lock().unwrap();
                        match st.filtered.get(st.sel) {
                            Some(Hit::App(i)) => st.all.get(*i).map(|e| e.path.clone()),
                            Some(Hit::File(i)) => st.files.get(*i).map(|f| f.path.clone()),
                            None => None,
                        }
                    };
                    launcher_close(h);
                    if let Some(p) = action {
                        launcher_launch(&p);
                    }
                }
                LA_ACTIVATE_ALT => {
                    // Shift+Enter on a file: open its containing folder (file selected).
                    let path = {
                        let st = LAUNCHER_STATE.lock().unwrap();
                        match st.filtered.get(st.sel) {
                            Some(Hit::File(i)) => st.files.get(*i).map(|f| f.path.clone()),
                            _ => None,
                        }
                    };
                    if let Some(p) = path {
                        launcher_close(h);
                        launcher_reveal_in_folder(&p);
                    }
                }
                LA_TAB => {
                    {
                        let mut st = LAUNCHER_STATE.lock().unwrap();
                        st.detail = !st.detail;
                    }
                    let _ = InvalidateRect(h, None, BOOL(0));
                }
                LA_CLOSE => launcher_close(h),
                _ => {}
            }
            LRESULT(0)
        }
        WM_PAINT => {
            launcher_paint(h);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        _ => DefWindowProcW(h, msg, w, l),
    }
}

/// Launcher thread: registers its class, creates the (hidden) picker window, and
/// pumps its own message loop. Idle until the hook posts `WM_LAUNCHER`.
fn launcher_thread() {
    unsafe {
        let hinst = HINSTANCE(BAR_HINST.load(Ordering::Relaxed) as *mut c_void);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(launcher_wndproc),
            hInstance: hinst,
            hbrBackground: CreateSolidBrush(COLORREF(LAUNCHER_BG)),
            lpszClassName: w!("astur_launcher"),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            w!("astur_launcher"),
            w!(""),
            WS_POPUP,
            0,
            0,
            LAUNCHER_W,
            LAUNCHER_H,
            None,
            None,
            hinst,
            None,
        );
        let Ok(hwnd) = hwnd else {
            return;
        };
        make_launcher_font();
        LAUNCHER_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
        // Modern rounded corners on the picker card (Win11; no-op pre-22000).
        let pref = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const c_void,
            std::mem::size_of_val(&pref) as u32,
        );
        // COM for the shell enumeration + icon resolution this thread does.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        // Enumerate apps now, in the idle window before the first Alt+Space, so the
        // first open is instant (AppsFolder enumeration can take a beat).
        {
            let apps = launcher_enumerate();
            let n = apps.len();
            {
                let mut st = LAUNCHER_STATE.lock().unwrap();
                st.all = apps;
                st.loaded = true;
                launcher_refilter(&mut st);
            }
            // Preload every app's icon in the background so the list is fully
            // iconned before the picker is opened (the parallel icon workers chew
            // through these while Astur sits idle).
            let mut q = ICON_QUEUE.lock().unwrap();
            for i in 0..n {
                q.push_back(i);
            }
            drop(q);
            ICON_CV.notify_all();
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

// =========================================================================
// System / power menu (Alt+Shift+Space): omarchy-style power actions, same
// hook-driven no-focus model as the launcher. See plan/system-menu.md.
// =========================================================================

const WM_SYSMENU: u32 = WM_USER + 11;
const SM_OPEN: usize = 0;
const SM_UP: usize = 1;
const SM_DOWN: usize = 2;
const SM_ACTIVATE: usize = 3;
const SM_CLOSE: usize = 4;
const SM_BACK: usize = 5; // up one level (submenu -> root), or close from root

const SYSMENU_W: i32 = 380;
const SYSMENU_HEADER: i32 = 44;
const SYSMENU_FOOTER: i32 = 34; // hint / confirm banner

static SYSMENU_OPEN: AtomicBool = AtomicBool::new(false);
static SYSMENU_HWND: AtomicIsize = AtomicIsize::new(0);

#[derive(Clone, Copy, PartialEq)]
enum SysAct {
    Lock,
    Sleep,
    SignOut,
    Restart,
    Shutdown,
    OpenConfig,
}

// A row is either a category (drills into a submenu) or an action (the bool = needs a
// confirm press). omarchy-style categorised menu; data-driven so mods can add rows.
enum SysKind {
    Category(&'static [SysItem]),
    Action(SysAct, bool),
}
struct SysItem {
    label: &'static str,
    kind: SysKind,
}

const POWER: &[SysItem] = &[
    SysItem { label: "Lock", kind: SysKind::Action(SysAct::Lock, false) },
    SysItem { label: "Sleep", kind: SysKind::Action(SysAct::Sleep, false) },
    SysItem { label: "Sign out", kind: SysKind::Action(SysAct::SignOut, true) },
    SysItem { label: "Restart", kind: SysKind::Action(SysAct::Restart, true) },
    SysItem { label: "Shut down", kind: SysKind::Action(SysAct::Shutdown, true) },
];
const SETUP: &[SysItem] = &[
    SysItem { label: "Open config folder", kind: SysKind::Action(SysAct::OpenConfig, false) },
];
const SYS_ROOT: &[SysItem] = &[
    SysItem { label: "Power", kind: SysKind::Category(POWER) },
    SysItem { label: "Setup", kind: SysKind::Category(SETUP) },
    // Theme / Appearance lands here once theming + wallpaper exist (see roadmap-v2.md).
];

struct SysMenuState {
    items: &'static [SysItem], // current level
    title: &'static str,
    sel: usize,
    confirm: bool,
    at_root: bool,
}
static SYSMENU_STATE: Mutex<SysMenuState> = Mutex::new(SysMenuState {
    items: SYS_ROOT,
    title: "System",
    sel: 0,
    confirm: false,
    at_root: true,
});

/// Enable SeShutdownPrivilege on our token (required by ExitWindowsEx for reboot/
/// shutdown). Lazy — only when a power action fires, never at startup.
unsafe fn enable_shutdown_priv() {
    let mut tok = HANDLE::default();
    if OpenProcessToken(
        GetCurrentProcess(),
        TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
        &mut tok,
    )
    .is_err()
    {
        return;
    }
    let mut luid = LUID::default();
    if LookupPrivilegeValueW(PCWSTR::null(), SE_SHUTDOWN_NAME, &mut luid).is_ok() {
        let tp = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        let _ = AdjustTokenPrivileges(tok, BOOL(0), Some(&tp), 0, None, None);
    }
    let _ = CloseHandle(tok);
}

unsafe fn sysmenu_exec(act: SysAct) {
    match act {
        SysAct::Lock => {
            let _ = LockWorkStation();
        }
        SysAct::Sleep => {
            let _ = SetSuspendState(BOOLEAN(0), BOOLEAN(0), BOOLEAN(0));
        }
        SysAct::SignOut => {
            let _ = ExitWindowsEx(EWX_LOGOFF | EWX_FORCEIFHUNG, SHUTDOWN_REASON(0));
        }
        SysAct::Restart => {
            enable_shutdown_priv();
            let _ = ExitWindowsEx(EWX_REBOOT | EWX_FORCEIFHUNG, SHUTDOWN_REASON(0));
        }
        SysAct::Shutdown => {
            enable_shutdown_priv();
            let _ = ExitWindowsEx(EWX_SHUTDOWN | EWX_FORCEIFHUNG, SHUTDOWN_REASON(0));
        }
        SysAct::OpenConfig => {
            if let Ok(home) = std::env::var("USERPROFILE") {
                let dir = format!(r"{home}\.astur");
                let wp: Vec<u16> = dir.encode_utf16().chain(std::iter::once(0)).collect();
                let op: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
                ShellExecuteW(
                    HWND(std::ptr::null_mut()),
                    PCWSTR(op.as_ptr()),
                    PCWSTR(wp.as_ptr()),
                    PCWSTR::null(),
                    PCWSTR::null(),
                    SW_SHOW,
                );
            }
        }
    }
}

/// Size + center the menu to the current level's row count, then repaint.
unsafe fn sysmenu_layout(h: HWND) {
    let n = SYSMENU_STATE.lock().unwrap().items.len() as i32;
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let wa = work_area_at(pt);
    let hgt = SYSMENU_HEADER + 6 + n * LAUNCHER_ROW_H + SYSMENU_FOOTER + 6;
    let x = (wa.left + wa.right) / 2 - SYSMENU_W / 2;
    let y = (wa.top + wa.bottom) / 2 - hgt / 2;
    let _ = SetWindowPos(h, HWND_TOPMOST, x, y, SYSMENU_W, hgt, SWP_NOACTIVATE);
    let _ = InvalidateRect(h, None, BOOL(0));
}

unsafe fn sysmenu_show(h: HWND) {
    {
        let mut st = SYSMENU_STATE.lock().unwrap();
        st.items = SYS_ROOT;
        st.title = "System";
        st.sel = 0;
        st.confirm = false;
        st.at_root = true;
    }
    sysmenu_layout(h);
    let _ = ShowWindow(h, SW_SHOWNA);
}

unsafe fn sysmenu_close(h: HWND) {
    let _ = ShowWindow(h, SW_HIDE);
    SYSMENU_OPEN.store(false, Ordering::Relaxed);
    let mut st = SYSMENU_STATE.lock().unwrap();
    st.items = SYS_ROOT;
    st.title = "System";
    st.sel = 0;
    st.confirm = false;
    st.at_root = true;
}

unsafe fn sysmenu_paint(h: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(h, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(h, &mut rc);
    let w = rc.right - rc.left;

    let frame = CreateSolidBrush(COLORREF(LAUNCHER_FRAME));
    FillRect(hdc, &rc, frame);
    let _ = DeleteObject(HGDIOBJ(frame.0));
    let inner = RECT {
        left: rc.left + 1,
        top: rc.top + 1,
        right: rc.right - 1,
        bottom: rc.bottom - 1,
    };
    let bg = CreateSolidBrush(COLORREF(LAUNCHER_BG));
    FillRect(hdc, &inner, bg);
    let _ = DeleteObject(HGDIOBJ(bg.0));

    let font_raw = LAUNCHER_FONT.load(Ordering::Relaxed);
    let old_font = if font_raw != 0 {
        Some(SelectObject(hdc, HGDIOBJ(font_raw as *mut c_void)))
    } else {
        None
    };
    SetBkMode(hdc, TRANSPARENT);

    let st = SYSMENU_STATE.lock().unwrap();
    SetTextColor(hdc, COLORREF(LAUNCHER_DIM));
    let mut tr = RECT {
        left: LAUNCHER_PAD,
        top: 0,
        right: w - LAUNCHER_PAD,
        bottom: SYSMENU_HEADER,
    };
    let mut tv: Vec<u16> = st.title.encode_utf16().collect();
    DrawTextW(hdc, &mut tv, &mut tr, DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    let div = RECT {
        left: LAUNCHER_PAD,
        top: SYSMENU_HEADER,
        right: w - LAUNCHER_PAD,
        bottom: SYSMENU_HEADER + 1,
    };
    let db = CreateSolidBrush(COLORREF(LAUNCHER_DIVIDER));
    FillRect(hdc, &div, db);
    let _ = DeleteObject(HGDIOBJ(db.0));

    for (i, item) in st.items.iter().enumerate() {
        let top = SYSMENU_HEADER + 6 + i as i32 * LAUNCHER_ROW_H;
        let row = RECT {
            left: LAUNCHER_PAD,
            top,
            right: w - LAUNCHER_PAD,
            bottom: top + LAUNCHER_ROW_H,
        };
        if i == st.sel {
            let sel = CreateSolidBrush(COLORREF(LAUNCHER_SELBG));
            let pen = CreatePen(PS_SOLID, 1, COLORREF(LAUNCHER_SELBG));
            let ob = SelectObject(hdc, HGDIOBJ(sel.0));
            let op = SelectObject(hdc, HGDIOBJ(pen.0));
            let _ = RoundRect(
                hdc,
                row.left + 4,
                top + 3,
                row.right - 4,
                top + LAUNCHER_ROW_H - 3,
                LAUNCHER_SEL_RADIUS,
                LAUNCHER_SEL_RADIUS,
            );
            SelectObject(hdc, ob);
            SelectObject(hdc, op);
            let _ = DeleteObject(HGDIOBJ(sel.0));
            let _ = DeleteObject(HGDIOBJ(pen.0));
            SetTextColor(hdc, COLORREF(LAUNCHER_SELFG));
        } else {
            SetTextColor(hdc, COLORREF(LAUNCHER_FG));
        }
        let mut r = RECT {
            left: row.left + 14,
            ..row
        };
        let mut v: Vec<u16> = item.label.encode_utf16().collect();
        DrawTextW(hdc, &mut v, &mut r, DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        // Chevron marks a category (drills into a submenu).
        if let SysKind::Category(_) = item.kind {
            let mut cr = RECT {
                left: row.left,
                top,
                right: row.right - 12,
                bottom: top + LAUNCHER_ROW_H,
            };
            let mut cv: Vec<u16> = "\u{203a}".encode_utf16().collect();
            DrawTextW(hdc, &mut cv, &mut cr, DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_RIGHT);
        }
    }

    let fy = rc.bottom - SYSMENU_FOOTER;
    let label = if st.confirm {
        format!(
            "Press Enter again to {}  \u{2022}  Esc cancels",
            st.items[st.sel].label.to_ascii_lowercase()
        )
    } else if st.at_root {
        "Up/Down  \u{2022}  Enter open  \u{2022}  Esc close".to_string()
    } else {
        "Up/Down  \u{2022}  Enter run  \u{2022}  \u{2190}/Esc back".to_string()
    };
    SetTextColor(hdc, COLORREF(if st.confirm { LAUNCHER_SELBG } else { LAUNCHER_DIM }));
    let mut fr = RECT {
        left: LAUNCHER_PAD,
        top: fy,
        right: w - LAUNCHER_PAD,
        bottom: rc.bottom,
    };
    let mut fv: Vec<u16> = label.encode_utf16().collect();
    DrawTextW(hdc, &mut fv, &mut fr, DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);

    if let Some(of) = old_font {
        SelectObject(hdc, of);
    }
    drop(st);
    let _ = EndPaint(h, &ps);
}

unsafe extern "system" fn sysmenu_wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_SYSMENU => {
            match w.0 {
                SM_OPEN => sysmenu_show(h),
                SM_UP => {
                    {
                        let mut st = SYSMENU_STATE.lock().unwrap();
                        st.confirm = false;
                        if st.sel > 0 {
                            st.sel -= 1;
                        }
                    }
                    let _ = InvalidateRect(h, None, BOOL(0));
                }
                SM_DOWN => {
                    {
                        let mut st = SYSMENU_STATE.lock().unwrap();
                        st.confirm = false;
                        let n = st.items.len();
                        if st.sel + 1 < n {
                            st.sel += 1;
                        }
                    }
                    let _ = InvalidateRect(h, None, BOOL(0));
                }
                SM_ACTIVATE => {
                    // Category drills into its submenu; an action runs (confirm-gated
                    // ones arm on the first Enter, run on the second).
                    enum Nav {
                        Drill,
                        Confirm,
                        Run(SysAct),
                    }
                    let nav = {
                        let mut st = SYSMENU_STATE.lock().unwrap();
                        let sel = st.sel;
                        let (sub_opt, label, act_opt): (
                            Option<&'static [SysItem]>,
                            &'static str,
                            Option<(SysAct, bool)>,
                        ) = match &st.items[sel].kind {
                            SysKind::Category(sub) => (Some(*sub), st.items[sel].label, None),
                            SysKind::Action(a, n) => (None, st.items[sel].label, Some((*a, *n))),
                        };
                        if let Some(sub) = sub_opt {
                            st.items = sub;
                            st.title = label;
                            st.sel = 0;
                            st.confirm = false;
                            st.at_root = false;
                            Nav::Drill
                        } else {
                            let (act, needs) = act_opt.unwrap();
                            if needs && !st.confirm {
                                st.confirm = true;
                                Nav::Confirm
                            } else {
                                Nav::Run(act)
                            }
                        }
                    };
                    match nav {
                        Nav::Drill => sysmenu_layout(h),
                        Nav::Confirm => {
                            let _ = InvalidateRect(h, None, BOOL(0));
                        }
                        Nav::Run(a) => {
                            sysmenu_close(h);
                            sysmenu_exec(a);
                        }
                    }
                }
                SM_BACK => {
                    enum B {
                        Repaint,
                        Layout,
                        Close,
                    }
                    let b = {
                        let mut st = SYSMENU_STATE.lock().unwrap();
                        if st.confirm {
                            st.confirm = false;
                            B::Repaint
                        } else if !st.at_root {
                            st.items = SYS_ROOT;
                            st.title = "System";
                            st.sel = 0;
                            st.at_root = true;
                            B::Layout
                        } else {
                            B::Close
                        }
                    };
                    match b {
                        B::Repaint => {
                            let _ = InvalidateRect(h, None, BOOL(0));
                        }
                        B::Layout => sysmenu_layout(h),
                        B::Close => sysmenu_close(h),
                    }
                }
                SM_CLOSE => {
                    let cancel_only = {
                        let mut st = SYSMENU_STATE.lock().unwrap();
                        if st.confirm {
                            st.confirm = false;
                            true
                        } else {
                            false
                        }
                    };
                    if cancel_only {
                        let _ = InvalidateRect(h, None, BOOL(0));
                    } else {
                        sysmenu_close(h);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_PAINT => {
            sysmenu_paint(h);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        _ => DefWindowProcW(h, msg, w, l),
    }
}

/// System-menu thread: registers its class, creates the hidden popup, pumps its own
/// message loop. Idle until the keyboard hook posts `WM_SYSMENU`.
fn sysmenu_thread() {
    unsafe {
        let hinst = HINSTANCE(BAR_HINST.load(Ordering::Relaxed) as *mut c_void);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(sysmenu_wndproc),
            hInstance: hinst,
            hbrBackground: CreateSolidBrush(COLORREF(LAUNCHER_BG)),
            lpszClassName: w!("astur_sysmenu"),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            w!("astur_sysmenu"),
            w!(""),
            WS_POPUP,
            0,
            0,
            SYSMENU_W,
            400,
            None,
            None,
            hinst,
            None,
        );
        let Ok(hwnd) = hwnd else {
            return;
        };
        make_launcher_font();
        SYSMENU_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
        let pref = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const c_void,
            std::mem::size_of_val(&pref) as u32,
        );
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

// =========================================================================
// System tray icon (Astur Full): the control surface when there's no console.
// Left/double-click -> Settings; right-click -> Settings / Quit. Quit restores
// all managed windows then exits. See plan/editions.md.
// =========================================================================

const WM_TRAY: u32 = WM_USER + 20;
const TRAY_SETTINGS: usize = 1;
const TRAY_QUIT: usize = 2;

// The Astur logo (site favicon, 32x32 transparent), embedded so the tray icon needs
// no external file or resource compiler.
const TRAY_ICON_PNG: &[u8] = include_bytes!("../assets/tray-icon.png");

/// Build the tray HICON from the embedded PNG (Win10/11 accept PNG icon bits).
/// Falls back to the stock application icon if creation fails.
unsafe fn tray_icon() -> HICON {
    CreateIconFromResourceEx(TRAY_ICON_PNG, BOOL(1), 0x0003_0000, 0, 0, LR_DEFAULTCOLOR)
        .unwrap_or_else(|_| LoadIconW(None, IDI_APPLICATION).unwrap_or_default())
}

unsafe fn tray_add(hwnd: HWND) {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: tray_icon(),
        ..Default::default()
    };
    for (i, c) in "Astur".encode_utf16().enumerate().take(127) {
        nid.szTip[i] = c;
    }
    let _ = Shell_NotifyIconW(NIM_ADD, &nid);
}

unsafe fn tray_remove(hwnd: HWND) {
    let nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    };
    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
}

/// Launch the sibling settings GUI (`astur-settings.exe` next to this exe).
unsafe fn tray_open_settings() {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(dir) = exe.parent() else { return };
    let path = dir.join("astur-settings.exe");
    let p: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let op: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    ShellExecuteW(
        HWND(std::ptr::null_mut()),
        PCWSTR(op.as_ptr()),
        PCWSTR(p.as_ptr()),
        PCWSTR::null(),
        PCWSTR::null(),
        SW_SHOW,
    );
}

unsafe extern "system" fn tray_wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_TRAY {
        // Classic NOTIFYICON callback: lParam low word = the mouse message.
        let event = (l.0 as u32) & 0xFFFF;
        if event == WM_LBUTTONUP || event == WM_LBUTTONDBLCLK {
            tray_open_settings();
        } else if event == WM_RBUTTONUP {
            if let Ok(menu) = CreatePopupMenu() {
                let s1: Vec<u16> = "Settings\0".encode_utf16().collect();
                let s2: Vec<u16> = "Quit\0".encode_utf16().collect();
                let _ = AppendMenuW(menu, MF_STRING, TRAY_SETTINGS, PCWSTR(s1.as_ptr()));
                let _ = AppendMenuW(menu, MF_STRING, TRAY_QUIT, PCWSTR(s2.as_ptr()));
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                // Required so the menu dismisses when you click elsewhere.
                let _ = SetForegroundWindow(h);
                let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, h, None);
                let _ = DestroyMenu(menu);
                match cmd.0 as usize {
                    TRAY_SETTINGS => tray_open_settings(),
                    TRAY_QUIT => {
                        tray_remove(h);
                        restore_all_windows();
                        PostQuitMessage(0);
                    }
                    _ => {}
                }
            }
        }
        return LRESULT(0);
    }
    DefWindowProcW(h, msg, w, l)
}

/// Register + create the hidden tray window and add the tray icon. Returns its HWND.
unsafe fn setup_tray(hinst: HINSTANCE) -> Option<HWND> {
    let wc = WNDCLASSW {
        lpfnWndProc: Some(tray_wndproc),
        hInstance: hinst,
        lpszClassName: w!("astur_tray"),
        ..Default::default()
    };
    RegisterClassW(&wc);
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW,
        w!("astur_tray"),
        w!("Astur"),
        WS_POPUP,
        0,
        0,
        0,
        0,
        None,
        None,
        hinst,
        None,
    )
    .ok()?;
    tray_add(hwnd);
    Some(hwnd)
}

fn main() {
    // Reveal every managed window if any thread panics. `panic = "abort"` skips
    // destructors and a process kill skips the console handler, so without this a
    // window hidden on an inactive workspace would be left invisible. The hook
    // runs before the abort.
    std::panic::set_hook(Box::new(|info| {
        restore_on_panic();
        eprintln!("Astur: panic — managed windows restored. {info}");
    }));
    unsafe {
        // 1ms timer resolution so the animation worker's frame sleeps are precise
        // (the default ~15.6ms granularity is the main cause of choppy motion).
        let _ = windows::Win32::Media::timeBeginPeriod(1);

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
        apply_bar_statics(&cfg);

        // Red, click-through, topmost corner-marker overlay.
        let brush = CreateSolidBrush(COLORREF(0x000000FF)); // 0x00BBGGRR -> red
        let wc = WNDCLASSW {
            lpfnWndProc: Some(marker_wndproc),
            hInstance: hinst,
            hbrBackground: brush,
            lpszClassName: w!("astur_marker"),
            ..Default::default()
        };
        RegisterClassW(&wc);

        // Workspace-slide overlay class (black background; the slide paints the
        // captured screen onto it via GDI, DWM thumbnails composite over that).
        let slide_wc = WNDCLASSW {
            lpfnWndProc: Some(slide_wndproc),
            hInstance: hinst,
            hbrBackground: CreateSolidBrush(COLORREF(0)),
            lpszClassName: SLIDE_CLASS,
            ..Default::default()
        };
        RegisterClassW(&slide_wc);

        let marker = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("astur_marker"),
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
                lpszClassName: w!("astur_bar"),
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
        // hidden on another workspace when Astur exits.
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

        // System tray icon — the control surface for Astur Full (no console in
        // release): left/double-click opens Settings, right-click menu = Settings/Quit.
        let _tray = setup_tray(hinst);

        // Apply window moves/resizes off the input thread for smoothness.
        std::thread::spawn(position_worker);
        // Focus-follows-mouse poll loop (no-op unless enabled in config).
        std::thread::spawn(focus_follow_worker);
        // CPU/RAM/battery poll loop (idles unless a stats widget is enabled).
        std::thread::spawn(stats_worker);
        // Workspace-slide compositor (owns its overlay + message pump; idle on a
        // condvar until the manager dispatches a slide).
        std::thread::spawn(transition_worker);
        // Per-window glide compositor (move/open/close/re-tile). Own overlay +
        // pump; idle on a condvar until the manager dispatches a glide.
        std::thread::spawn(glide_worker);
        // App launcher (Alt+Space): owns its picker window + message pump, idle
        // until the keyboard hook posts an open/key message.
        std::thread::spawn(launcher_thread);
        // Resolve launcher app icons to HBITMAPs off the UI thread, in parallel so
        // the whole list is iconned fast (each worker is a COM STA; they idle on a
        // condvar once the queue drains). Count is a speed/RAM trade — see
        // plan/optimization.md.
        for _ in 0..3 {
            std::thread::spawn(icon_worker);
        }
        // File search against the Windows Search index (debounced, own COM STA).
        std::thread::spawn(filesearch_worker);
        // System / power menu (Alt+Shift+Space): owns its popup + message pump.
        std::thread::spawn(sysmenu_thread);
        // Hot-reload config files on save.
        std::thread::spawn(config_watcher);
        // Owns all tiling/workspace state; hooks only enqueue commands to it.
        std::thread::spawn(move || manager_loop(cfg));

        println!("Astur running.");
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
        println!("  Alt+Space      = app launcher (type to filter, Enter to run)");
        println!("  Alt+Enter      = launch terminal");
        println!("  Alt+Shift+Enter= launch default browser");
        println!("  Alt+1..9,0     = switch workspace (or click a bar pill)");
        println!("  Alt+Shift+1..0 = move focused window to workspace");
        println!("  Per-monitor status bars, focus-follows-mouse, window rules:");
        println!("  all configurable in astur.conf (see comments in that file).");
        println!("  Alt+Tab still works. Use RIGHT ALT for normal Alt behavior.");
        println!("  --- config ---");
        println!("  Default 'shared' mode spreads workspaces across monitors:");
        println!("  ws1=mon1, ws2=mon2, ws3=mon3, ws4=mon1 (2nd), and so on.");
        println!("  Edit %USERPROFILE%\\.astur\\astur.conf then restart.");
        println!("  workspace_mode = shared | per_monitor; set terminal/browser too.");
        println!("Press Ctrl+C in this window to quit (windows are restored).");

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let _ = UnhookWindowsHookEx(kbd_hook);
        let _ = UnhookWindowsHookEx(mouse_hook);
        let _ = windows::Win32::Media::timeEndPeriod(1);
    }
}

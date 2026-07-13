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
    AlphaBlend, BLENDFUNCTION, StretchBlt, SetStretchBltMode, HALFTONE,
    BeginPaint, BitBlt, CombineRgn, CreateBitmap, CreateCompatibleBitmap, CreateCompatibleDC,
    CreateFontW,
    CreateRectRgn, CreateRoundRectRgn, CreateSolidBrush, CreatePen, DeleteDC, DeleteObject,
    DrawTextW, EndPaint,
    RoundRect, PS_SOLID, UpdateWindow,
    EnumDisplayMonitors, FillRect, GetDC, GetMonitorInfoW, GetStockObject, InvalidateRect,
    MonitorFromPoint, MonitorFromWindow, ReleaseDC, SelectObject, SetBkMode, SetTextColor,
    SetWindowRgn, CAPTUREBLT, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_GUI_FONT, DT_CALCRECT, DT_CENTER, DT_END_ELLIPSIS, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE,
    DT_VCENTER, DRAW_TEXT_FORMAT, HDC, HGDIOBJ, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, RGN_DIFF, RGN_OR, SRCCOPY, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, SendInput, ToUnicode, INPUT, INPUT_0,
    INPUT_KEYBOARD, KEYBDINPUT, VK_CAPITAL,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DOWN,
    VK_ESCAPE, VK_LBUTTON, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_MENU, VK_RBUTTON,
    VK_RCONTROL, VK_RETURN, VK_RMENU, VK_RSHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::Shell::{
    ShellExecuteW, SHCreateItemFromParsingName, IShellItem, IEnumShellItems,
    IShellItemImageFactory, BHID_EnumItems, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING,
    SIIGBF_ICONONLY, Shell_NotifyIconW, NOTIFYICONDATAW, NIM_ADD, NIM_DELETE, NIF_ICON,
    NIF_MESSAGE, NIF_TIP, SHGetFileInfoW, SHFILEINFOW, SHGFI_FLAGS, SHGFI_SYSICONINDEX,
    SHGFI_USEFILEATTRIBUTES, SHGetImageList, SHIL_LARGE,
};
use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconFromResourceEx, CreateIconIndirect, CreatePopupMenu, DestroyMenu,
    DrawIconEx, LoadIconW, PostQuitMessage, TrackPopupMenu, DI_NORMAL, HICON, ICONINFO,
    IDI_APPLICATION, LR_DEFAULTCOLOR, MF_STRING, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_LBUTTONDBLCLK,
};
use std::os::windows::ffi::OsStrExt;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::Media::Audio::{
    eConsole, eRender, Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator, MMDeviceEnumerator,
};
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
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
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SYSKEYDOWN,
    WM_SYSKEYUP, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};

// --- tiling additions -----------------------------------------------------
use std::collections::{HashMap, VecDeque};
use core::ffi::c_void;
use windows::Win32::Graphics::Dwm::{
    DwmFlush, DwmGetWindowAttribute, DwmRegisterThumbnail, DwmSetWindowAttribute,
    DwmUnregisterThumbnail, DwmUpdateThumbnailProperties, DWMWA_BORDER_COLOR, DWMWA_CLOAKED,
    DWMWA_EXTENDED_FRAME_BOUNDS, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_VISIBLE,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, OpenProcess,
    OpenProcessToken, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::core::s;
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
    WM_CLOSE, WM_DISPLAYCHANGE, WM_ENDSESSION, WM_ERASEBKGND, WM_PAINT, WM_QUERYENDSESSION,
    WM_TIMER, WM_USER, WS_CHILD,
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
    // latest previewed rect shown by the drag outline; committed to the real
    // window once on release, so there is no per-frame cross-process SetWindowPos.
    cur_x: i32,
    cur_y: i32,
    cur_w: i32,
    cur_h: i32,
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
            cur_x: 0,
            cur_y: 0,
            cur_w: 0,
            cur_h: 0,
        }
    }
}

static STATE: Mutex<Drag> = Mutex::new(Drag::new());

/// Drag previews never touch the real window per frame. Moving/resizing a foreign
/// window live means a cross-process SetWindowPos per mouse event, which stalls on
/// the target app's own repaint (a browser re-layouts per pixel — the "resizing is
/// slow" complaint). The primary preview is a live DWM thumbnail (below); this
/// outline frame is the fallback when a thumbnail can't register. Either way the
/// final rect is committed to the real window ONCE on release, by the manager.
static OUTLINE_HWND: AtomicIsize = AtomicIsize::new(0);
const OUTLINE_THICK: i32 = 3;

/// Show the drag outline as a hollow rectangle at (x, y, w, h): region-shaped to a
/// frame so only the border paints. Layered / click-through / topmost overlay.
unsafe fn show_outline(x: i32, y: i32, w: i32, h: i32) {
    let raw = OUTLINE_HWND.load(Ordering::Relaxed);
    if raw == 0 || w <= 0 || h <= 0 {
        return;
    }
    let hwnd = hwnd_from(raw);
    let t = OUTLINE_THICK;
    let region = CreateRectRgn(0, 0, w, h);
    if w > 2 * t && h > 2 * t {
        let inner = CreateRectRgn(t, t, w - t, h - t);
        CombineRgn(region, region, inner, RGN_DIFF);
        let _ = DeleteObject(HGDIOBJ(inner.0));
    }
    // The window takes ownership of `region`; the system frees the previous one.
    SetWindowRgn(hwnd, region, BOOL(1));
    let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
}

unsafe fn hide_outline() {
    let raw = OUTLINE_HWND.load(Ordering::Relaxed);
    if raw != 0 {
        let _ = ShowWindow(hwnd_from(raw), SW_HIDE);
    }
}

/// Trivial WndProc for the outline / thumbnail overlays. Must be its OWN proc (not
/// the marker's, which handles WM_DISPLAYCHANGE/WM_RELOAD and would double-fire the
/// bar rebuild).
unsafe extern "system" fn outline_wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    DefWindowProcW(h, msg, w, l)
}

// --- Live DWM-thumbnail drag preview (move + resize) -----------------------
// The dragged window is mirrored live with a DWM thumbnail (GPU-composited — works
// even on Chrome, where PrintWindow returns black). The manager parks the real
// window off-screen for the duration (Cmd::DragPark) so only the mirror is visible,
// and commits the final rect on release (Cmd::DragMoved/DragResized) — the hook
// itself never does a cross-process SetWindowPos. Thumbnails preserve the source
// aspect ratio, so a resize letterboxes while the aspect changes (accepted for live
// content); registration failure falls back to the outline (and no park).
static THUMB_HWND: AtomicIsize = AtomicIsize::new(0); // overlay DWM renders into
static THUMB_ID: AtomicIsize = AtomicIsize::new(0); // HTHUMBNAIL (0 = none active)
static DRAG_THUMB: AtomicBool = AtomicBool::new(false); // this drag uses the thumbnail

unsafe fn thumb_props(id: isize, w: i32, h: i32) {
    let props = DWM_THUMBNAIL_PROPERTIES {
        dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY,
        rcDestination: RECT { left: 0, top: 0, right: w, bottom: h },
        opacity: 255,
        fVisible: BOOL(1),
        fSourceClientAreaOnly: BOOL(0),
        ..Default::default()
    };
    let _ = DwmUpdateThumbnailProperties(id, &props);
}

/// Begin a live thumbnail preview of `src` at (x, y, w, h). Returns false if the
/// thumbnail can't be registered (caller falls back to the outline).
unsafe fn thumb_begin(src: isize, x: i32, y: i32, w: i32, h: i32) -> bool {
    let ov = THUMB_HWND.load(Ordering::Relaxed);
    if ov == 0 || w <= 0 || h <= 0 {
        return false;
    }
    let id = match DwmRegisterThumbnail(hwnd_from(ov), hwnd_from(src)) {
        Ok(id) => id,
        Err(_) => return false,
    };
    THUMB_ID.store(id, Ordering::Relaxed);
    let _ = SetWindowPos(hwnd_from(ov), HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    thumb_props(id, w, h);
    let _ = src; // parked by the manager (Cmd::DragPark) — never from the hook
    true
}

unsafe fn thumb_update(x: i32, y: i32, w: i32, h: i32) {
    let ov = THUMB_HWND.load(Ordering::Relaxed);
    let id = THUMB_ID.load(Ordering::Relaxed);
    if ov == 0 || id == 0 || w <= 0 || h <= 0 {
        return;
    }
    let _ = SetWindowPos(hwnd_from(ov), HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE);
    thumb_props(id, w, h);
}

unsafe fn thumb_end() {
    let id = THUMB_ID.load(Ordering::Relaxed);
    if id != 0 {
        let _ = DwmUnregisterThumbnail(id);
        THUMB_ID.store(0, Ordering::Relaxed);
    }
    let ov = THUMB_HWND.load(Ordering::Relaxed);
    if ov != 0 {
        let _ = ShowWindow(hwnd_from(ov), SW_HIDE);
    }
}

// Drag preview: a live thumbnail when it registers, else the outline frame.
unsafe fn drag_preview_begin(src: isize, x: i32, y: i32, w: i32, h: i32) {
    if thumb_begin(src, x, y, w, h) {
        DRAG_THUMB.store(true, Ordering::Relaxed);
        // The mirror overlay is up (frame 0 == the window's own pixels). Now ask
        // the manager to park the real window off-screen so the user sees only the
        // thumbnail — via the queue, because the hook must never do a cross-process
        // SetWindowPos. The park lands under/behind the already-covering overlay.
        push_cmd(Cmd::DragPark(src));
    } else {
        DRAG_THUMB.store(false, Ordering::Relaxed);
        show_outline(x, y, w, h);
    }
}
unsafe fn drag_preview_update(x: i32, y: i32, w: i32, h: i32) {
    if DRAG_THUMB.load(Ordering::Relaxed) {
        thumb_update(x, y, w, h);
    } else {
        show_outline(x, y, w, h);
    }
}
unsafe fn drag_preview_end() {
    if DRAG_THUMB.load(Ordering::Relaxed) {
        thumb_end();
    } else {
        hide_outline();
    }
}

/// Commit a previewed rect to the real window in one synchronous SetWindowPos.
/// Runs on the MANAGER thread (DragMoved/DragResized/DragPark handlers), never on a
/// hook. Handles floating windows (which keep this dropped rect) and tiled ones
/// (which retile over it) alike.
unsafe fn commit_rect(hwnd: isize, x: i32, y: i32, w: i32, h: i32) {
    let _ = SetWindowPos(
        hwnd_from(hwnd),
        None,
        x,
        y,
        w,
        h,
        SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSENDCHANGING,
    );
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

/// True for any modifier key's virtual-key code. The low-level keyboard hook
/// reports the SPECIFIC left/right codes (`VK_LSHIFT`/`VK_RSHIFT`, `VK_LMENU`,
/// `VK_LCONTROL`…), never the generic aggregate (`VK_SHIFT` etc.). Capture modes
/// (launcher / system menu) MUST let these fall through to the system: swallowing
/// a modifier key-up while a menu is open leaves the global async key state (what
/// `GetAsyncKeyState` reads) reporting that modifier stuck down — the "phantom
/// Shift" bug when a menu is opened with Alt+Shift+Space and Shift is released
/// before the menu closes. Includes the generic codes for injected events too.
#[inline]
fn is_modifier_vk(vk: u32) -> bool {
    vk == VK_SHIFT.0 as u32
        || vk == VK_LSHIFT.0 as u32
        || vk == VK_RSHIFT.0 as u32
        || vk == VK_MENU.0 as u32
        || vk == VK_LMENU.0 as u32
        || vk == VK_RMENU.0 as u32
        || vk == VK_CONTROL.0 as u32
        || vk == VK_LCONTROL.0 as u32
        || vk == VK_RCONTROL.0 as u32
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
    if msg == WM_CLOSE || msg == WM_QUERYENDSESSION || msg == WM_ENDSESSION {
        // Graceful teardown paths for the no-console (windows-subsystem) build:
        // Task Manager "End task" sends WM_CLOSE; logoff/shutdown sends
        // WM_QUERYENDSESSION/WM_ENDSESSION. Reveal every managed window before
        // the process dies so none stay hidden. (Hard kills skip all of this —
        // the crash-rescue file covers those on the next launch.)
        restore_all_windows();
        if msg == WM_CLOSE {
            PostQuitMessage(0);
            return LRESULT(0);
        }
        return DefWindowProcW(h, msg, w, l);
    }
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
                if !is_modifier_vk(vk) {
                    if down {
                        let hs = SYSMENU_HWND.load(Ordering::Relaxed);
                        if hs != 0 {
                            let hwnd = hwnd_from(hs);
                            let post = |a: usize| {
                                let _ = PostMessageW(hwnd, WM_SYSMENU, WPARAM(a), LPARAM(0));
                            };
                            if vk == VK_ESCAPE.0 as u32 {
                                // Esc steps back one level (cancel confirm -> back to
                                // root -> close from root), same as Left/Backspace —
                                // so Esc in a submenu returns to the menu, not exit.
                                post(SM_BACK);
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
                if !is_modifier_vk(vk) {
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
                                post(LA_TAB, 0); // toggle the wide column view
                            } else if vk == VK_BACK.0 as u32 {
                                post(LA_BACK, 0);
                            } else if vk == VK_UP.0 as u32 {
                                post(LA_UP, 0);
                            } else if vk == VK_DOWN.0 as u32 {
                                post(LA_DOWN, 0);
                            } else if vk == VK_SPACE.0 as u32 {
                                post(LA_CHAR, ' ' as isize);
                            } else {
                                // Pack vk + scancode + Shift/CapsLock; the launcher
                                // thread runs ToUnicode (honours Shift — capitals and
                                // calculator symbols like + * ( ) — which
                                // MAPVK_VK_TO_CHAR did not). No conversion on the hook.
                                let shift = vk_down(VK_SHIFT);
                                let caps = (GetKeyState(VK_CAPITAL.0 as i32) & 1) != 0;
                                let packed = (vk as isize & 0xFFFF)
                                    | ((kb.scanCode as isize & 0xFFFF) << 16)
                                    | ((shift as isize) << 32)
                                    | ((caps as isize) << 33);
                                post(LA_KEY, packed);
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

    // Popup mouse routing (launcher + system menu). Closed = one atomic load, the
    // common case. Open: a click OUTSIDE dismisses (eaten, so it doesn't also act
    // on whatever is underneath); the WHEEL inside scrolls the list (eaten, so the
    // app under the popup doesn't scroll — wheel routing to unfocused windows is a
    // user setting, the hook is deterministic). Clicks INSIDE fall through: the
    // popups are NOACTIVATE but still receive mouse messages directly, and their
    // wndprocs handle hover-select and click-activate.
    if LAUNCHER_OPEN.load(Ordering::Relaxed) {
        let inside = pt.x >= LAUNCHER_RECT_L.load(Ordering::Relaxed)
            && pt.x < LAUNCHER_RECT_R.load(Ordering::Relaxed)
            && pt.y >= LAUNCHER_RECT_T.load(Ordering::Relaxed)
            && pt.y < LAUNCHER_RECT_B.load(Ordering::Relaxed);
        let hl = LAUNCHER_HWND.load(Ordering::Relaxed);
        if hl != 0 {
            if matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN) && !inside {
                let _ = PostMessageW(hwnd_from(hl), WM_LAUNCHER, WPARAM(LA_CLOSE), LPARAM(0));
                return suppress; // eat the dismissing click so it doesn't also act
            }
            if msg == WM_MOUSEWHEEL && inside {
                // Wheel delta rides the high word of mouseData (signed, ±120/notch).
                let delta = ((info.mouseData >> 16) as u16 as i16) as isize;
                let step: isize = if delta > 0 { 1 } else { -1 };
                let _ = PostMessageW(hwnd_from(hl), WM_LAUNCHER, WPARAM(LA_SCROLL), LPARAM(step));
                return suppress;
            }
        }
    }
    if SYSMENU_OPEN.load(Ordering::Relaxed) {
        let inside = pt.x >= SYSMENU_RECT_L.load(Ordering::Relaxed)
            && pt.x < SYSMENU_RECT_R.load(Ordering::Relaxed)
            && pt.y >= SYSMENU_RECT_T.load(Ordering::Relaxed)
            && pt.y < SYSMENU_RECT_B.load(Ordering::Relaxed);
        let hs = SYSMENU_HWND.load(Ordering::Relaxed);
        if hs != 0 {
            if matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN) && !inside {
                let _ = PostMessageW(hwnd_from(hs), WM_SYSMENU, WPARAM(SM_CLOSE), LPARAM(0));
                return suppress;
            }
            if msg == WM_MOUSEWHEEL && inside {
                let delta = ((info.mouseData >> 16) as u16 as i16) as isize;
                let act = if delta > 0 { SM_UP } else { SM_DOWN };
                let _ = PostMessageW(hwnd_from(hs), WM_SYSMENU, WPARAM(act), LPARAM(0));
                return suppress;
            }
        }
    }
    // Wheel over a status bar: route to that bar (volume widget / workspace
    // cycle). The bar is NOACTIVATE so the wheel would otherwise go to the
    // focused app. Idle cost: one atomic load; per-slot checks are plain loads
    // (the hook may not lock). Eaten so the app underneath doesn't also scroll.
    if msg == WM_MOUSEWHEEL && BARS_HOT.load(Ordering::Relaxed) {
        for i in 0..MAX_BARS {
            let hb = BARHIT_HWND[i].load(Ordering::Relaxed);
            if hb == 0 {
                continue;
            }
            if pt.x >= BARHIT_L[i].load(Ordering::Relaxed)
                && pt.x < BARHIT_R[i].load(Ordering::Relaxed)
                && pt.y >= BARHIT_T[i].load(Ordering::Relaxed)
                && pt.y < BARHIT_B[i].load(Ordering::Relaxed)
            {
                let delta = ((info.mouseData >> 16) as u16 as i16) as i32;
                let up = (delta > 0) as usize;
                let _ = PostMessageW(
                    hwnd_from(hb),
                    WM_BAR_WHEEL,
                    WPARAM(up),
                    LPARAM(pt.x as isize),
                );
                return suppress;
            }
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
                    s.cur_x = x;
                    s.cur_y = y;
                    s.cur_w = w;
                    s.cur_h = h;
                    let src = s.hwnd;
                    ANY_DRAG.store(true, Ordering::Relaxed);
                    drop(s);
                    drag_preview_begin(src, x, y, w, h);
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
                    s.cur_x = rect.left;
                    s.cur_y = rect.top;
                    s.cur_w = rect.right - rect.left;
                    s.cur_h = rect.bottom - rect.top;
                    let src = s.hwnd;
                    ANY_DRAG.store(true, Ordering::Relaxed);
                    drop(s);
                    drag_preview_begin(
                        src,
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                    );
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
                    s.cur_x = rect.left;
                    s.cur_y = rect.top;
                    s.cur_w = rect.right - rect.left;
                    s.cur_h = rect.bottom - rect.top;
                    let src = s.hwnd;
                    ANY_DRAG.store(true, Ordering::Relaxed);
                    drop(s);
                    drag_preview_begin(
                        src,
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                    );
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
            let mut s = STATE.lock().unwrap();
            match s.mode {
                Mode::Move => {
                    let nx = s.win_x + (pt.x - s.origin_x);
                    let ny = s.win_y + (pt.y - s.origin_y);
                    s.cur_x = nx;
                    s.cur_y = ny;
                    s.cur_w = s.win_w;
                    s.cur_h = s.win_h;
                    drag_preview_update(nx, ny, s.win_w, s.win_h);
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
                    s.cur_x = x;
                    s.cur_y = y;
                    s.cur_w = w;
                    s.cur_h = h;
                    drag_preview_update(x, y, w, h);
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
                let (cx, cy, cw, ch) = (s.cur_x, s.cur_y, s.cur_w, s.cur_h);
                s.mode = Mode::None;
                ANY_DRAG.store(false, Ordering::Relaxed);
                drop(s);
                // Push first so the manager can commit the previewed rect (and
                // restore a parked window) at the earliest; then drop the preview.
                push_cmd(Cmd::DragMoved(
                    h,
                    pt.x,
                    pt.y,
                    RECT { left: cx, top: cy, right: cx + cw, bottom: cy + ch },
                ));
                drag_preview_end();
                return suppress;
            }
        }
        WM_RBUTTONUP => {
            let mut s = STATE.lock().unwrap();
            if s.mode == Mode::Resize {
                let h = s.hwnd;
                let (cx, cy, cw, ch) = (s.cur_x, s.cur_y, s.cur_w, s.cur_h);
                s.mode = Mode::None;
                ANY_DRAG.store(false, Ordering::Relaxed);
                drop(s);
                // Push first (manager commits the previewed rect + restores a parked
                // window), then tear the preview down.
                push_cmd(Cmd::DragResized(
                    h,
                    Some(RECT { left: cx, top: cy, right: cx + cw, bottom: cy + ch }),
                ));
                hide_marker();
                drag_preview_end();
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
    // Alt-drag lifecycle. The hook never touches the real window (a cross-process
    // SetWindowPos can stall on a busy app) — it previews with an overlay and
    // pushes these; the manager parks/commits the real window.
    DragPark(isize),                   // thumbnail drag began: park the window off-screen
    DragMoved(isize, i32, i32, RECT),  // dropped after Alt+left-drag: (hwnd, x, y, final rect)
    DragResized(isize, Option<RECT>),  // released after resize; None = read the live rect
    LaunchTerminal,             // Alt+Enter
    LaunchBrowser,              // Alt+Shift+Enter
    FocusGeo(Dir),              // Alt+arrow: focus the window in a direction
    MoveGeo(Dir),               // Alt+Shift+arrow: move the window in a direction
    FocusMouse(isize),          // focus-follows-mouse: cursor hovered this window
    BarClick(isize, usize),     // bar pill clicked: (monitor hmon, local workspace)
    BarFocus(isize),            // bar app-button clicked: focus this window
    BarCycle(isize, i32),       // bar wheel: (monitor hmon, +1 next / -1 prev workspace)
    Reload(Box<Config>),        // config file changed on disk; apply live
}

static CMDQ: Mutex<VecDeque<Cmd>> = Mutex::new(VecDeque::new());
static CMDCV: Condvar = Condvar::new();
// While true, programmatic show/hide must not be mistaken for app events.
static SUPPRESS: AtomicBool = AtomicBool::new(false);
// Windows Astur itself hid for a workspace switch. SUPPRESS alone is NOT enough
// to filter their EVENT_OBJECT_HIDE: WinEvents are out-of-context (queued to the
// main thread), so the tail of a hide batch can arrive AFTER the manager cleared
// SUPPRESS — Cmd::Remove then untracked live windows, leaving them hidden and
// orphaned ("windows on other workspaces died"). Membership here says "this hide
// was ours — ignore it". Not touched by the LL input hooks, so a lock is fine.
static HIDDEN_BY_US: Mutex<Option<std::collections::HashSet<isize>>> = Mutex::new(None);

fn mark_hidden_by_us(h: isize) {
    HIDDEN_BY_US
        .lock()
        .unwrap()
        .get_or_insert_with(Default::default)
        .insert(h);
}

fn unmark_hidden_by_us(h: isize) {
    if let Some(s) = HIDDEN_BY_US.lock().unwrap().as_mut() {
        s.remove(&h);
    }
}

fn was_hidden_by_us(h: isize) -> bool {
    HIDDEN_BY_US
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|s| s.contains(&h))
}
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
// Network rates in bytes/s (-1 = unavailable) and speaker volume (0..100 / -1),
// polled by stats_worker; volume also updates instantly on a bar wheel/click.
static NET_ON: AtomicBool = AtomicBool::new(false);
static VOL_ON: AtomicBool = AtomicBool::new(false);
static STAT_NET_D: AtomicIsize = AtomicIsize::new(-1);
static STAT_NET_U: AtomicIsize = AtomicIsize::new(-1);
static STAT_VOL: AtomicIsize = AtomicIsize::new(-1);
static STAT_MUTE: AtomicBool = AtomicBool::new(false);

// ---- bar v2 style/behaviour (ensure_bars + the mouse hook read these) ----
static BAR_FLOATING: AtomicBool = AtomicBool::new(false);
static BAR_MARGIN: AtomicIsize = AtomicIsize::new(8);
static BAR_RADIUS: AtomicIsize = AtomicIsize::new(12);
static BAR_AUTOHIDE: AtomicBool = AtomicBool::new(false);
static BAR_WHEEL_WS: AtomicBool = AtomicBool::new(true);

// Hook-visible bar hit rects, lock-free (the mouse hook may not take locks).
// Slot i is bar i's on-screen rect while it accepts wheel input; hwnd 0 = empty.
// BARS_HOT short-circuits the whole check to one atomic load when idle.
const MAX_BARS: usize = 8;
static BARS_HOT: AtomicBool = AtomicBool::new(false);
static BARHIT_HWND: [AtomicIsize; MAX_BARS] = [const { AtomicIsize::new(0) }; MAX_BARS];
static BARHIT_L: [AtomicI32; MAX_BARS] = [const { AtomicI32::new(0) }; MAX_BARS];
static BARHIT_T: [AtomicI32; MAX_BARS] = [const { AtomicI32::new(0) }; MAX_BARS];
static BARHIT_R: [AtomicI32; MAX_BARS] = [const { AtomicI32::new(0) }; MAX_BARS];
static BARHIT_B: [AtomicI32; MAX_BARS] = [const { AtomicI32::new(0) }; MAX_BARS];

/// Publish (or clear, with w=0 rects) a bar's wheel hit rect for the hook.
fn barhit_publish(hwnd: isize, r: Option<RECT>) {
    // Reuse the slot already holding this hwnd, else the first empty one.
    let slot = (0..MAX_BARS)
        .find(|&i| BARHIT_HWND[i].load(Ordering::Relaxed) == hwnd)
        .or_else(|| (0..MAX_BARS).find(|&i| BARHIT_HWND[i].load(Ordering::Relaxed) == 0));
    let Some(i) = slot else { return };
    match r {
        Some(r) => {
            BARHIT_L[i].store(r.left, Ordering::Relaxed);
            BARHIT_T[i].store(r.top, Ordering::Relaxed);
            BARHIT_R[i].store(r.right, Ordering::Relaxed);
            BARHIT_B[i].store(r.bottom, Ordering::Relaxed);
            BARHIT_HWND[i].store(hwnd, Ordering::Relaxed);
        }
        None => {
            BARHIT_HWND[i].store(0, Ordering::Relaxed);
        }
    }
}

/// Per-bar paint layout published for same-thread mouse hit-testing (pill /
/// app-button / volume-widget ranges move with the configurable zones).
#[derive(Default, Clone)]
struct BarLayout {
    pills_x0: i32,
    cell: i32,
    npills: usize,
    apps: Vec<(i32, i32, isize)>, // (x0, x1, hwnd)
    vol: (i32, i32),              // volume widget x-range (0,0 = not shown)
}
static BAR_LAYOUTS: Mutex<Option<HashMap<isize, BarLayout>>> = Mutex::new(None);

/// Auto-hide runtime state per bar window (bar/main thread only). `y_cur` eases
/// toward shown/hidden each AH_TIMER tick, so the bar slides rather than pops.
/// `strip` is the reveal band on the bar's docked screen edge.
struct AhBar {
    x: i32,
    w: i32,
    h: i32,
    y_shown: i32,
    y_hidden: i32,
    y_cur: f64,
    shown: bool,
    strip: RECT,
}
static AH_BARS: Mutex<Option<HashMap<isize, AhBar>>> = Mutex::new(None);
const AH_TIMER_ID: usize = 4;

/// Sliding workspace-pill highlight. While an entry is present for a monitor,
/// paint_bar draws the accent pill at an interpolated position between the old
/// and new pill INDEX instead of snapping (indices, not x's: with configurable
/// zones the pills' origin is only known at paint time). Keyed by HMONITOR,
/// driven by a fast WM_TIMER on the bar window.
struct PillAnim {
    from_i: i32,
    to_i: i32,
    start: Instant,
}
static PILL_ANIM: Mutex<Option<HashMap<isize, PillAnim>>> = Mutex::new(None);
const PILL_ANIM_MS: f64 = 160.0;

fn pill_anim_set(hmon: isize, from_i: i32, to_i: i32) {
    PILL_ANIM.lock().unwrap().get_or_insert_with(HashMap::new).insert(
        hmon,
        PillAnim {
            from_i,
            to_i,
            start: Instant::now(),
        },
    );
}

fn pill_anim_clear(hmon: isize) {
    if let Some(m) = PILL_ANIM.lock().unwrap().as_mut() {
        m.remove(&hmon);
    }
}

/// Current highlight position (in pill units) for a monitor's pill animation and
/// whether it's done. None = no animation running (paint at the active pill).
fn pill_anim_pos(hmon: isize) -> Option<(f64, bool)> {
    let g = PILL_ANIM.lock().unwrap();
    let a = g.as_ref()?.get(&hmon)?;
    let t = (a.start.elapsed().as_secs_f64() * 1000.0 / PILL_ANIM_MS).min(1.0);
    let pos = a.from_i as f64 + (a.to_i - a.from_i) as f64 * ease_in_out_cubic(t);
    Some((pos, t >= 1.0))
}

/// Per-monitor paint data. One entry per drawn pill: `slots[i]` is the local
/// workspace index that pill maps to (so a click resolves straight to a
/// workspace even when empty pills are hidden), `labels[i]` is the number to
/// print, `occupied` bit i marks a pill whose workspace has windows, and
/// `active` is the pill index of the shown workspace (usize::MAX if none).
/// `apps` lists the active workspace's windows (hwnd + cached exe HICON) for
/// the app-buttons widget.
#[derive(Clone, PartialEq)]
struct MonBar {
    hmon: isize,
    slots: Vec<usize>,
    labels: Vec<usize>,
    active: usize,
    occupied: u64,
    title: String,
    apps: Vec<(isize, isize)>,
}

/// One bar widget slot; the navbar zone lists resolve to these at update time.
#[derive(Clone, Copy, PartialEq)]
enum BarWidget {
    Workspaces,
    Apps,
    Title,
    Layout,
    Cpu,
    Mem,
    Net,
    Volume,
    Battery,
    Date,
    Clock,
}

/// Resolve one configured zone: widget names -> widgets, honouring the show_*
/// toggles (a widget must be listed AND enabled to draw).
fn zone_widgets(names: &[String], cfg: &Config) -> Vec<BarWidget> {
    names
        .iter()
        .filter_map(|n| match n.as_str() {
            "workspaces" => Some(BarWidget::Workspaces),
            "apps" if cfg.bar_show_apps => Some(BarWidget::Apps),
            "title" if cfg.bar_show_title => Some(BarWidget::Title),
            "layout" if cfg.bar_show_layout => Some(BarWidget::Layout),
            "cpu" if cfg.bar_show_cpu => Some(BarWidget::Cpu),
            "mem" if cfg.bar_show_mem => Some(BarWidget::Mem),
            "net" if cfg.bar_show_net => Some(BarWidget::Net),
            "volume" if cfg.bar_show_volume => Some(BarWidget::Volume),
            "battery" if cfg.bar_show_battery => Some(BarWidget::Battery),
            "date" if cfg.bar_show_date => Some(BarWidget::Date),
            "clock" if cfg.bar_show_clock => Some(BarWidget::Clock),
            _ => None,
        })
        .collect()
}

/// The four bar colours with the theme applied. Each colour is independently
/// `auto` (None — resolves to the shared dark/light preset in `astur-config`)
/// or an explicit user COLORREF that always wins. Explicit tri-state replaced
/// two failed heuristics: per-field default-matching mixed presets with custom
/// colours (black on black), and all-or-nothing froze the bar dark forever the
/// moment ANY colour had ever been touched.
fn themed_bar_colors(cfg: &Config) -> (u32, u32, u32, u32) {
    let preset = if THEME_LIGHT.load(Ordering::Relaxed) {
        config::BAR_LIGHT
    } else {
        config::BAR_DARK
    };
    (
        cfg.bar_bg.unwrap_or(preset[0]),
        cfg.bar_fg.unwrap_or(preset[1]),
        cfg.bar_accent.unwrap_or(preset[2]),
        cfg.bar_inactive.unwrap_or(preset[3]),
    )
}

/// Everything the bars paint. Replaced wholesale by the manager each update.
#[derive(Clone)]
struct BarData {
    bg: u32,
    fg: u32,
    accent: u32,
    inactive: u32,
    clock_24h: bool,
    date_format: String,
    layout: String,
    tiling: bool,
    left: Vec<BarWidget>,
    center: Vec<BarWidget>,
    right: Vec<BarWidget>,
    mons: Vec<MonBar>,
}

impl BarData {
    const fn new() -> Self {
        BarData {
            bg: 0x00261B1A,
            fg: 0x00F5CAC0,
            accent: 0x00FFAA66,
            inactive: 0x00895F56,
            clock_24h: true,
            date_format: String::new(),
            layout: String::new(),
            tiling: true,
            left: Vec::new(),
            center: Vec::new(),
            right: Vec::new(),
            mons: Vec::new(),
        }
    }
}

static BAR: Mutex<BarData> = Mutex::new(BarData::new());
// Custom message: manager asks a bar to repaint.
const WM_BAR_REFRESH: u32 = WM_USER + 1;
// Custom message: manager seeds a pill-highlight slide (wparam=from pill index,
// lparam=to pill index — paint resolves indices to x's, zones move the origin).
const WM_PILL_ANIM: u32 = WM_USER + 3;
// Custom message from the LL mouse hook: wheel over this bar (wparam: 1=up,
// 0=down; lparam = screen x of the cursor).
const WM_BAR_WHEEL: u32 = WM_USER + 4;
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
        // Auto-hide bars reserve nothing (they overlay on reveal). A floating
        // bar reserves its height plus the margin on both sides so tiles clear
        // the detached pill.
        if cfg.bar_enabled && cfg.bar_height > 0 && !cfg.bar_autohide {
            let extra = if cfg.bar_floating { cfg.bar_margin * 2 } else { 0 };
            if cfg.bar_bottom {
                m.work_area.bottom -= cfg.bar_height + extra;
            } else {
                m.work_area.top += cfg.bar_height + extra;
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
        unmark_hidden_by_us(h);
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
    // Everything is visible again — nothing left for the crash-rescue pass.
    let _ = std::fs::remove_file(rescue_file());
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
        // Skip dead HWNDs: if a window was destroyed but its EVENT_OBJECT_DESTROY was
        // missed (WinEvent hooks can drop events under load), a stale entry would
        // otherwise reserve an empty tile — the "ghost window taking a tile" bug.
        .filter(|h| {
            IsWindow(hwnd_from(*h)).as_bool()
                && !ws.floating.contains(h)
                && !IsIconic(hwnd_from(*h)).as_bool()
        })
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
    // Every hide is marked in HIDDEN_BY_US BEFORE the ShowWindow so the async
    // EVENT_OBJECT_HIDE can never race the marker (see the static's comment).
    {
        let ws = &mgr.monitors[mi].workspaces[old].windows;
        for i in 0..ws.len() {
            mark_hidden_by_us(ws[i]);
            let _ = ShowWindow(hwnd_from(ws[i]), SW_HIDE);
        }
    }
    mgr.monitors[mi].active = n;
    {
        let ws = &mgr.monitors[mi].workspaces[n].windows;
        for i in 0..ws.len() {
            unmark_hidden_by_us(ws[i]);
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
            let show = wi == active;
            for &h in &ws.windows {
                if show {
                    unmark_hidden_by_us(h);
                } else {
                    mark_hidden_by_us(h);
                }
                let _ = ShowWindow(hwnd_from(h), if show { SW_SHOWNA } else { SW_HIDE });
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
            match mgr.locate(h) {
                Some((mi, wi)) => {
                    // Already tracked. If an app just surfaced it on a HIDDEN
                    // workspace (link click opening the browser, taskbar
                    // activation, …), FOLLOW it: switch to its workspace. Never
                    // pull the window out of its workspace — that half-shows it
                    // over the active tiling. Foreground check keeps background
                    // self-shows (toasts, splash refreshes) from yanking the
                    // workspace.
                    if wi != mgr.monitors[mi].active
                        && IsWindowVisible(hwnd_from(h)).as_bool()
                        && GetForegroundWindow() == hwnd_from(h)
                    {
                        mgr.monitors[mi].workspaces[wi].focused = h;
                        mgr.focused_mon = mi;
                        switch_monitor_workspace(mgr, mi, wi);
                    }
                }
                None if is_manageable(hwnd_from(h)) => {
                    // A terminal/browser we just launched lands on the cursor's
                    // monitor (consumed once); everything else goes by its spawn
                    // position.
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
                None => {}
            }
        }
        Cmd::Remove(h) => {
            unmark_hidden_by_us(h); // untracked -> marker would only go stale
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
                } else {
                    // The OS foregrounded a window on a hidden workspace (an app
                    // activated it — link opened in the browser, taskbar click).
                    // Follow it there; pulling it out would break both layouts.
                    mgr.monitors[mi].workspaces[wi].focused = h;
                    switch_monitor_workspace(mgr, mi, wi);
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
        Cmd::BarFocus(h) => {
            // App button clicked: focus that window (same effect as clicking it).
            if IsWindow(hwnd_from(h)).as_bool() {
                focus_window(h);
            }
        }
        Cmd::BarCycle(hmon, dir) => {
            // Wheel over the bar: previous/next workspace on that monitor (wraps).
            if let Some(mi) = mgr.mon_by_hmon(hmon) {
                let count = mgr.monitors[mi].workspaces.len();
                if count > 1 {
                    let cur = mgr.monitors[mi].active as i32;
                    let next = (cur + dir).rem_euclid(count as i32) as usize;
                    mgr.focused_mon = mi;
                    switch_monitor_workspace(mgr, mi, next);
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
        Cmd::DragPark(h) => {
            // Thumbnail drag began: park the real window far off-screen (size kept)
            // so the user sees only the live DWM mirror. Off-screen, NOT SW_HIDE — a
            // hidden window blanks its thumbnail. The drop (DragMoved/DragResized)
            // commits the final rect, which restores it on-screen.
            if IsWindow(hwnd_from(h)).as_bool() {
                let _ = SetWindowPos(
                    hwnd_from(h),
                    None,
                    -32000,
                    -32000,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSENDCHANGING,
                );
            }
        }
        Cmd::DragMoved(h, x, y, r) => {
            // Land the previewed rect FIRST — the real window never moved during the
            // drag (the thumbnail path even parked it off-screen), so this single
            // SetWindowPos is the actual move. It must precede every early-out:
            // floating, unmanaged, and tiling-off windows keep exactly this rect.
            commit_rect(h, r.left, r.top, r.right - r.left, r.bottom - r.top);
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
        Cmd::DragResized(h, rect) => {
            // Alt-resize carries the previewed rect (commit before any early-out so
            // floating/unmanaged windows land too); the native MOVESIZEEND path
            // passes None and the window already sits at its final rect.
            if let Some(r) = rect {
                commit_rect(h, r.left, r.top, r.right - r.left, r.bottom - r.top);
            }
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
            let r = match rect {
                Some(r) => r,
                None => {
                    let mut r = RECT::default();
                    if GetWindowRect(hwnd_from(h), &mut r).is_err() {
                        retile_monitor(mgr, mi);
                        return;
                    }
                    r
                }
            };
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
/// One AH_TIMER tick (~30ms): decide shown/hidden from the cursor and ease the
/// bar's y toward the target (slide-in/out). Runs on the bar's own thread and
/// only ever moves the bar window itself — never a managed window.
unsafe fn bar_autohide_tick(h: HWND) {
    let key = h.0 as isize;
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let mut g = AH_BARS.lock().unwrap();
    let Some(ab) = g.as_mut().and_then(|m| m.get_mut(&key)) else { return };
    let yc = ab.y_cur as i32;
    let over_bar = pt.x >= ab.x
        && pt.x < ab.x + ab.w
        && pt.y >= yc - 8
        && pt.y < yc + ab.h + 8;
    let in_strip = pt.x >= ab.strip.left
        && pt.x < ab.strip.right
        && pt.y >= ab.strip.top
        && pt.y < ab.strip.bottom;
    let want = over_bar || in_strip;
    if want != ab.shown {
        ab.shown = want;
        // Wheel routing only while the bar is on screen.
        if want {
            barhit_publish(
                key,
                Some(RECT {
                    left: ab.x,
                    top: ab.y_shown,
                    right: ab.x + ab.w,
                    bottom: ab.y_shown + ab.h,
                }),
            );
        } else {
            barhit_publish(key, None);
        }
    }
    let target = if ab.shown { ab.y_shown } else { ab.y_hidden } as f64;
    if (ab.y_cur - target).abs() > 0.5 {
        ab.y_cur += (target - ab.y_cur) * 0.35;
        if (ab.y_cur - target).abs() <= 0.5 {
            ab.y_cur = target;
        }
        let x = ab.x;
        let y = ab.y_cur.round() as i32;
        drop(g); // release before the (same-process) window move
        let _ = SetWindowPos(
            h,
            HWND_TOPMOST,
            x,
            y,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOSIZE,
        );
    }
}

unsafe fn ensure_bars() {
    let height = BAR_HEIGHT.load(Ordering::Relaxed) as i32;
    if height <= 0 {
        // Bar disabled: silence the hook's wheel routing.
        for i in 0..MAX_BARS {
            BARHIT_HWND[i].store(0, Ordering::Relaxed);
        }
        BARS_HOT.store(false, Ordering::Relaxed);
        return;
    }
    let bottom = BAR_BOTTOM.load(Ordering::Relaxed);
    let floating = BAR_FLOATING.load(Ordering::Relaxed);
    let margin = if floating {
        BAR_MARGIN.load(Ordering::Relaxed) as i32
    } else {
        0
    };
    let radius = BAR_RADIUS.load(Ordering::Relaxed) as i32;
    let autohide = BAR_AUTOHIDE.load(Ordering::Relaxed);
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
        let x = rcm.left + margin;
        let w = (rcm.right - rcm.left) - margin * 2;
        let y = if bottom {
            rcm.bottom - height - margin
        } else {
            rcm.top + margin
        };
        let hb = if let Some(b) = bars.iter().find(|b| b.hmon == hmon) {
            let hb = hwnd_from(b.hwnd);
            let _ = SetWindowPos(
                hb,
                HWND_TOPMOST,
                x,
                y,
                w,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            hb
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
            hb
        };
        // Floating bars get rounded corners via a window region (works on
        // Windows 10 and 11 alike). Classic bars clear any leftover region.
        if floating && radius > 0 {
            let rgn = CreateRoundRectRgn(0, 0, w + 1, height + 1, radius * 2, radius * 2);
            let _ = SetWindowRgn(hb, rgn, true); // system owns the region now
        } else {
            let _ = SetWindowRgn(hb, None, true);
        }
        // Publish the wheel hit rect for the LL mouse hook.
        barhit_publish(
            hb.0 as isize,
            Some(RECT {
                left: x,
                top: y,
                right: x + w,
                bottom: y + height,
            }),
        );
        // Auto-hide state: reveal band on the docked screen edge.
        if autohide {
            let strip = if bottom {
                RECT {
                    left: rcm.left,
                    top: rcm.bottom - 2,
                    right: rcm.right,
                    bottom: rcm.bottom,
                }
            } else {
                RECT {
                    left: rcm.left,
                    top: rcm.top,
                    right: rcm.right,
                    bottom: rcm.top + 2,
                }
            };
            let y_hidden = if bottom {
                rcm.bottom + 2
            } else {
                rcm.top - height - 2
            };
            AH_BARS
                .lock()
                .unwrap()
                .get_or_insert_with(HashMap::new)
                .insert(
                    hb.0 as isize,
                    AhBar {
                        x,
                        w,
                        h: height,
                        y_shown: y,
                        y_hidden,
                        y_cur: y as f64,
                        shown: true,
                        strip,
                    },
                );
            SetTimer(hb, AH_TIMER_ID, 30, None);
        } else {
            if let Some(m) = AH_BARS.lock().unwrap().as_mut() {
                m.remove(&(hb.0 as isize));
            }
            let _ = KillTimer(hb, AH_TIMER_ID);
        }
    }
    // Hide bars whose monitor disappeared (and stop routing wheel to them).
    let present: Vec<isize> = raw.iter().map(|(h, _)| *h).collect();
    for b in bars.iter() {
        if !present.contains(&b.hmon) {
            let _ = ShowWindow(hwnd_from(b.hwnd), SW_HIDE);
            barhit_publish(b.hwnd, None);
        }
    }
    BARS_HOT.store(!bars.is_empty(), Ordering::Relaxed);
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

// ---- bar app-button icons ----------------------------------------------------
// Cached per exe path (loaded once via the launcher's HQ shell-icon pipeline at
// exactly the drawn size, then reused for every window of that app).
const BAR_ICON_PX: i32 = 20;
static BAR_ICONS: Mutex<Option<HashMap<String, isize>>> = Mutex::new(None);

/// Full exe path of a window's process (for the app-buttons icon cache key).
unsafe fn window_exe(hwnd: HWND) -> Option<String> {
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == 0 {
        return None;
    }
    let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
    let mut buf = [0u16; 512];
    let mut len = buf.len() as u32;
    let ok = QueryFullProcessImageNameW(
        proc,
        PROCESS_NAME_WIN32,
        windows::core::PWSTR(buf.as_mut_ptr()),
        &mut len,
    );
    let _ = CloseHandle(proc);
    ok.ok()?;
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

/// HICON for a window's exe at BAR_ICON_PX (cached; -1 = none). Manager thread.
unsafe fn bar_app_icon(hwnd: HWND) -> isize {
    let Some(path) = window_exe(hwnd) else { return -1 };
    {
        let cache = BAR_ICONS.lock().unwrap();
        if let Some(m) = cache.as_ref() {
            if let Some(&ic) = m.get(&path) {
                return ic;
            }
        }
    }
    let ic = shell_item_hicon(&path, BAR_ICON_PX)
        .map(|h| h.0 as isize)
        .unwrap_or(-1);
    BAR_ICONS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(path, ic);
    ic
}

/// Compact bytes/s for the net widget: 0K / 340K / 1.2M.
fn fmt_rate(bps: isize) -> String {
    if bps < 0 {
        return String::new();
    }
    let k = bps as f64 / 1024.0;
    if k < 1000.0 {
        format!("{:.0}K", k)
    } else {
        format!("{:.1}M", k / 1024.0)
    }
}

// ---- speaker volume (bar widget) ----------------------------------------------

/// Default render endpoint's volume interface. Created per call — cheap COM
/// activation, and it always tracks the CURRENT default device.
unsafe fn endpoint_volume() -> Option<IAudioEndpointVolume> {
    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
    let dev = en.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
    dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok()
}

unsafe fn volume_poll() {
    match endpoint_volume() {
        Some(v) => {
            if let Ok(s) = v.GetMasterVolumeLevelScalar() {
                STAT_VOL.store((s * 100.0).round() as isize, Ordering::Relaxed);
            }
            if let Ok(m) = v.GetMute() {
                STAT_MUTE.store(m.as_bool(), Ordering::Relaxed);
            }
        }
        None => STAT_VOL.store(-1, Ordering::Relaxed),
    }
}

/// Nudge the master volume (wheel over the volume widget). Updates the cached
/// stat immediately so the bar repaint shows the new value without waiting for
/// the 2s poll.
unsafe fn volume_adjust(delta: f32) {
    if let Some(v) = endpoint_volume() {
        if let Ok(s) = v.GetMasterVolumeLevelScalar() {
            let ns = (s + delta).clamp(0.0, 1.0);
            let _ = v.SetMasterVolumeLevelScalar(ns, std::ptr::null());
            STAT_VOL.store((ns * 100.0).round() as isize, Ordering::Relaxed);
        }
    }
}

unsafe fn volume_toggle_mute() {
    if let Some(v) = endpoint_volume() {
        if let Ok(m) = v.GetMute() {
            let nm = !m.as_bool();
            let _ = v.SetMute(nm, std::ptr::null());
            STAT_MUTE.store(nm, Ordering::Relaxed);
        }
    }
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
    let mut prev_net: Option<(u64, u64, Instant)> = None;
    loop {
        if !STATS_ON.load(Ordering::Relaxed) {
            prev_net = None;
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
            // Network: total octets across up ethernet/wifi interfaces; the rate
            // is the delta over the poll interval.
            if NET_ON.load(Ordering::Relaxed) {
                let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
                if GetIfTable2(&mut table).is_ok() && !table.is_null() {
                    let t = &*table;
                    let rows =
                        std::slice::from_raw_parts(t.Table.as_ptr(), t.NumEntries as usize);
                    let mut tin: u64 = 0;
                    let mut tout: u64 = 0;
                    for r in rows {
                        // 6 = ethernet, 71 = 802.11; OperStatus 1 = up.
                        if r.OperStatus.0 == 1 && (r.Type == 6 || r.Type == 71) {
                            tin = tin.saturating_add(r.InOctets);
                            tout = tout.saturating_add(r.OutOctets);
                        }
                    }
                    FreeMibTable(table as *const c_void);
                    let now = Instant::now();
                    if let Some((pin, pout, pt)) = prev_net {
                        let dt = now.duration_since(pt).as_secs_f64().max(0.1);
                        STAT_NET_D.store(
                            (tin.saturating_sub(pin) as f64 / dt) as isize,
                            Ordering::Relaxed,
                        );
                        STAT_NET_U.store(
                            (tout.saturating_sub(pout) as f64 / dt) as isize,
                            Ordering::Relaxed,
                        );
                    }
                    prev_net = Some((tin, tout, now));
                }
            } else {
                prev_net = None;
            }
            // Speaker volume + mute for the volume widget.
            if VOL_ON.load(Ordering::Relaxed) {
                volume_poll();
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
        // App buttons: the active workspace's windows with their exe icons
        // (cached per exe, so this is a HashMap hit after the first sighting).
        let apps: Vec<(isize, isize)> = if mgr.cfg.bar_show_apps {
            m.workspaces
                .get(m.active)
                .map(|ws| {
                    ws.windows
                        .iter()
                        .filter(|h| IsWindow(hwnd_from(**h)).as_bool())
                        .map(|&h| (h, bar_app_icon(hwnd_from(h))))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        mons.push(MonBar {
            hmon: m.hmon,
            slots,
            labels,
            active,
            occupied,
            title,
            apps,
        });
    }
    let (bg, fg, accent, inactive) = themed_bar_colors(&mgr.cfg);
    let new = BarData {
        bg,
        fg,
        accent,
        inactive,
        clock_24h: mgr.cfg.bar_clock_24h,
        date_format: mgr.cfg.bar_date_format.clone(),
        layout: mgr.cfg.layout.clone(),
        tiling: mgr.tiling,
        left: zone_widgets(&mgr.cfg.bar_left, &mgr.cfg),
        center: zone_widgets(&mgr.cfg.bar_center, &mgr.cfg),
        right: zone_widgets(&mgr.cfg.bar_right, &mgr.cfg),
        mons,
    };

    // Diff against the previous snapshot so only changed monitors repaint, and
    // seed a pill-highlight slide on any monitor whose active workspace moved.
    let animate_pills = mgr.cfg.animations;
    let mut changed: Vec<isize> = Vec::new();
    let mut anim_seeds: Vec<(isize, i32, i32)> = Vec::new();
    {
        let old = BAR.lock().unwrap();
        let global_changed = old.bg != new.bg
            || old.fg != new.fg
            || old.accent != new.accent
            || old.inactive != new.inactive
            || old.clock_24h != new.clock_24h
            || old.date_format != new.date_format
            || old.layout != new.layout
            || old.tiling != new.tiling
            || old.left != new.left
            || old.center != new.center
            || old.right != new.right
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
            // comparable) and a different, real pill became active. Seeds are
            // pill INDICES — paint knows the pills' x origin, update_bar doesn't
            // (it moves with the configurable zones).
            if animate_pills {
                if let Some(om) = om {
                    if om.slots == nm.slots
                        && om.active != usize::MAX
                        && nm.active != usize::MAX
                        && om.active != nm.active
                    {
                        anim_seeds.push((nm.hmon, om.active as i32, nm.active as i32));
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

const BAR_WIDGET_GAP: i32 = 16;
const BAR_APP_BTN_W: i32 = BAR_ICON_PX + 10; // app-button cell (icon + breathing room)

/// Text + colour-class for the simple text widgets (None = widget renders
/// nothing right now, e.g. no battery present). bool = draw dim.
unsafe fn bar_widget_text(wgt: BarWidget, data: &BarData, mb: Option<&MonBar>) -> Option<(String, bool)> {
    match wgt {
        BarWidget::Clock => {
            let st: SYSTEMTIME = GetLocalTime();
            let s = if data.clock_24h {
                format!("{:02}:{:02}", st.wHour, st.wMinute)
            } else {
                let (h12, ap) = to_12h(st.wHour);
                format!("{}:{:02} {}", h12, st.wMinute, ap)
            };
            Some((s, false))
        }
        BarWidget::Date => {
            let st: SYSTEMTIME = GetLocalTime();
            Some((format_date(&data.date_format, &st), false))
        }
        BarWidget::Battery => {
            let b = STAT_BAT.load(Ordering::Relaxed);
            (b >= 0).then(|| (format!("BAT {}%", b), false))
        }
        BarWidget::Mem => {
            let v = STAT_MEM.load(Ordering::Relaxed);
            (v >= 0).then(|| (format!("RAM {}%", v), false))
        }
        BarWidget::Cpu => {
            let v = STAT_CPU.load(Ordering::Relaxed);
            (v >= 0).then(|| (format!("CPU {}%", v), false))
        }
        BarWidget::Net => {
            let d = STAT_NET_D.load(Ordering::Relaxed);
            let u = STAT_NET_U.load(Ordering::Relaxed);
            (d >= 0 && u >= 0)
                .then(|| (format!("\u{2193}{} \u{2191}{}", fmt_rate(d), fmt_rate(u)), false))
        }
        BarWidget::Volume => {
            let v = STAT_VOL.load(Ordering::Relaxed);
            if v < 0 {
                return None;
            }
            if STAT_MUTE.load(Ordering::Relaxed) {
                Some(("MUTE".to_string(), true))
            } else {
                Some((format!("VOL {}%", v), false))
            }
        }
        BarWidget::Layout => {
            let s = if data.tiling {
                format!("[{}]", data.layout)
            } else {
                "[float]".to_string()
            };
            Some((s, true))
        }
        BarWidget::Title => {
            let t = mb.map(|m| m.title.as_str()).unwrap_or("");
            (!t.is_empty()).then(|| (t.to_string(), false))
        }
        BarWidget::Workspaces | BarWidget::Apps => None, // composite, drawn separately
    }
}

/// Width one widget will occupy (0 = skipped). `avail` caps the flexible title.
unsafe fn bar_widget_width(
    hdc: HDC,
    wgt: BarWidget,
    data: &BarData,
    mb: Option<&MonBar>,
    cell: i32,
    avail: i32,
) -> i32 {
    match wgt {
        BarWidget::Workspaces => mb.map(|m| m.labels.len() as i32 * cell).unwrap_or(0),
        BarWidget::Apps => mb.map(|m| m.apps.len() as i32 * BAR_APP_BTN_W).unwrap_or(0),
        _ => match bar_widget_text(wgt, data, mb) {
            Some((s, _)) => text_width(hdc, &s).min(avail.max(0)),
            None => 0,
        },
    }
}

/// Paint one widget with its left edge at `x`; returns the width consumed.
/// Records hit ranges (pills / app buttons / volume) into `lay` for the
/// wndproc's mouse handling.
unsafe fn bar_widget_draw(
    hdc: HDC,
    wgt: BarWidget,
    x: i32,
    h_px: i32,
    avail: i32,
    data: &BarData,
    mb: Option<&MonBar>,
    lay: &mut BarLayout,
    cell: i32,
) -> i32 {
    match wgt {
        BarWidget::Workspaces => {
            let Some(mb) = mb else { return 0 };
            let n = mb.labels.len() as i32;
            if n == 0 || cell <= 0 {
                return 0;
            }
            lay.pills_x0 = x;
            lay.npills = mb.labels.len();
            // Numbers first, in their resting colours...
            for (i, label) in mb.labels.iter().enumerate() {
                let x0 = x + i as i32 * cell;
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
            // ...then the accent highlight, at the animated position while a
            // slide is in flight, otherwise snapped to the active pill.
            let hl = match pill_anim_pos(mb.hmon) {
                Some((pos, _)) => Some(x + (pos * cell as f64).round() as i32),
                None if mb.active != usize::MAX => Some(x + mb.active as i32 * cell),
                None => None,
            };
            if let Some(hx) = hl {
                let ipad = (h_px / 6).clamp(2, 6);
                let pill = RECT {
                    left: hx + 3,
                    top: ipad,
                    right: hx + cell - 3,
                    bottom: h_px - ipad,
                };
                let ab = CreateSolidBrush(COLORREF(data.accent));
                FillRect(hdc, &pill, ab);
                let _ = DeleteObject(HGDIOBJ(ab.0));
                let nearest = (((hx - x) as f32 / cell as f32).round() as i32)
                    .clamp(0, n - 1) as usize;
                let mut cr = RECT {
                    left: hx,
                    top: 0,
                    right: hx + cell,
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
            n * cell
        }
        BarWidget::Apps => {
            let Some(mb) = mb else { return 0 };
            if mb.apps.is_empty() {
                return 0;
            }
            let iy = (h_px - BAR_ICON_PX) / 2;
            for (i, &(hwnd, icon)) in mb.apps.iter().enumerate() {
                let bx = x + i as i32 * BAR_APP_BTN_W;
                if icon > 0 {
                    let _ = DrawIconEx(
                        hdc,
                        bx + (BAR_APP_BTN_W - BAR_ICON_PX) / 2,
                        iy,
                        HICON(icon as *mut c_void),
                        BAR_ICON_PX,
                        BAR_ICON_PX,
                        0,
                        None,
                        DI_NORMAL,
                    );
                } else {
                    // No icon resolved: a dim placeholder square.
                    let mk = CreateSolidBrush(COLORREF(data.inactive));
                    let g = RECT {
                        left: bx + (BAR_APP_BTN_W - 10) / 2,
                        top: (h_px - 10) / 2,
                        right: bx + (BAR_APP_BTN_W - 10) / 2 + 10,
                        bottom: (h_px - 10) / 2 + 10,
                    };
                    FillRect(hdc, &g, mk);
                    let _ = DeleteObject(HGDIOBJ(mk.0));
                }
                lay.apps.push((bx, bx + BAR_APP_BTN_W, hwnd));
            }
            mb.apps.len() as i32 * BAR_APP_BTN_W
        }
        _ => {
            let Some((s, dim)) = bar_widget_text(wgt, data, mb) else { return 0 };
            let tw = text_width(hdc, &s).min(avail.max(0));
            if tw <= 0 {
                return 0;
            }
            let mut r = RECT {
                left: x,
                top: 0,
                right: x + tw,
                bottom: h_px,
            };
            SetTextColor(hdc, COLORREF(if dim { data.inactive } else { data.fg }));
            let mut v: Vec<u16> = s.encode_utf16().collect();
            DrawTextW(
                hdc,
                &mut v,
                &mut r,
                DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
            if wgt == BarWidget::Volume {
                lay.vol = (x, x + tw);
            }
            tw
        }
    }
}

/// Paint one monitor's bar from the three configurable zones (navbar.conf
/// `left` / `center` / `right`): the left zone flows left-to-right, the right
/// zone hugs the right edge (listed order still reads left-to-right), and the
/// center zone is centred in the remaining gap (the title flexes to fill).
/// The owning monitor's HMONITOR is in GWLP_USERDATA so each bar paints its own
/// data; the hit ranges land in BAR_LAYOUTS for the mouse handlers.
unsafe fn paint_bar(h: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let win_hdc = BeginPaint(h, &mut ps);
    let hmon = GetWindowLongPtrW(h, GWLP_USERDATA);
    let data = BAR.lock().unwrap().clone();

    let mut rc = RECT::default();
    let _ = GetClientRect(h, &mut rc);
    let h_px = rc.bottom - rc.top;
    let w = rc.right - rc.left;
    // Double buffer: the pill slide repaints at ~120Hz; direct painting flickers.
    let bb = backbuf_begin(win_hdc, w, h_px);
    let hdc = bb.as_ref().map(|b| b.dc).unwrap_or(win_hdc);

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
    let mb = data.mons.iter().find(|m| m.hmon == hmon);
    let mut lay = BarLayout {
        cell,
        ..Default::default()
    };

    // ---- left zone: flows left-to-right from the padding.
    let mut x = pad;
    for wgt in &data.left {
        let drew = bar_widget_draw(hdc, *wgt, x, h_px, w, &data, mb, &mut lay, cell);
        if drew > 0 {
            x += drew + BAR_WIDGET_GAP;
        }
    }
    let left_end = x;

    // ---- right zone: anchored to the right edge; iterate reversed so the
    // configured order reads left-to-right on screen.
    let mut right = w - pad;
    for wgt in data.right.iter().rev() {
        let ww = bar_widget_width(hdc, *wgt, &data, mb, cell, w);
        if ww <= 0 {
            continue;
        }
        let wx = right - ww;
        let _ = bar_widget_draw(hdc, *wgt, wx, h_px, ww, &data, mb, &mut lay, cell);
        right = wx - BAR_WIDGET_GAP;
    }

    // ---- center zone: centred in the remaining gap; the title flexes.
    let gap_l = left_end;
    let gap_r = right;
    if gap_r > gap_l && !data.center.is_empty() {
        let avail = gap_r - gap_l;
        let mut widths: Vec<i32> = Vec::with_capacity(data.center.len());
        let mut total = 0;
        for wgt in &data.center {
            let ww = bar_widget_width(hdc, *wgt, &data, mb, cell, avail - total);
            widths.push(ww);
            if ww > 0 {
                total += ww + BAR_WIDGET_GAP;
            }
        }
        if total > 0 {
            total -= BAR_WIDGET_GAP;
        }
        let mut cx = gap_l + ((avail - total).max(0)) / 2;
        for (wgt, ww) in data.center.iter().zip(widths) {
            if ww <= 0 {
                continue;
            }
            let _ = bar_widget_draw(hdc, *wgt, cx, h_px, ww, &data, mb, &mut lay, cell);
            cx += ww + BAR_WIDGET_GAP;
        }
    }

    if let Some(of) = old_font {
        SelectObject(hdc, of);
    }
    if let Some(b) = bb {
        backbuf_end(win_hdc, b);
    }
    // Publish this bar's hit ranges for the wndproc mouse handlers.
    BAR_LAYOUTS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(h.0 as isize, lay);
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
        WM_TIMER if w.0 == AH_TIMER_ID => {
            bar_autohide_tick(h);
            LRESULT(0)
        }
        WM_TIMER => {
            if w.0 == PILL_TIMER_ID {
                let hmon = GetWindowLongPtrW(h, GWLP_USERDATA);
                // Stop the fast timer once the slide finishes (or vanished).
                if pill_anim_pos(hmon).map(|(_, done)| done).unwrap_or(true) {
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
        WM_BAR_WHEEL => {
            // Routed from the LL mouse hook (the bar is NOACTIVATE, so the wheel
            // never reaches it natively). wparam: 1 = up, 0 = down; lparam =
            // screen x. Over the volume widget the wheel adjusts volume;
            // anywhere else it cycles workspaces (if enabled).
            let up = w.0 == 1;
            let mut wr = RECT::default();
            let _ = GetWindowRect(h, &mut wr);
            let cx = l.0 as i32 - wr.left;
            let lay = BAR_LAYOUTS
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|m| m.get(&(h.0 as isize)).cloned())
                .unwrap_or_default();
            if lay.vol.1 > lay.vol.0 && cx >= lay.vol.0 && cx < lay.vol.1 {
                volume_adjust(if up { 0.02 } else { -0.02 });
                let _ = InvalidateRect(h, None, BOOL(0));
            } else if BAR_WHEEL_WS.load(Ordering::Relaxed) {
                let hmon = GetWindowLongPtrW(h, GWLP_USERDATA);
                push_cmd(Cmd::BarCycle(hmon, if up { -1 } else { 1 }));
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            // Hit-test against the painted layout: workspace pills switch, app
            // buttons focus, the volume widget toggles mute.
            let x = (l.0 as u32 & 0xFFFF) as i16 as i32;
            let hmon = GetWindowLongPtrW(h, GWLP_USERDATA);
            let lay = BAR_LAYOUTS
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|m| m.get(&(h.0 as isize)).cloned())
                .unwrap_or_default();
            if lay.npills > 0
                && lay.cell > 0
                && x >= lay.pills_x0
                && x < lay.pills_x0 + lay.npills as i32 * lay.cell
            {
                let pill = ((x - lay.pills_x0) / lay.cell) as usize;
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
            } else if let Some(&(_, _, hw)) =
                lay.apps.iter().find(|&&(x0, x1, _)| x >= x0 && x < x1)
            {
                push_cmd(Cmd::BarFocus(hw));
            } else if lay.vol.1 > lay.vol.0 && x >= lay.vol.0 && x < lay.vol.1 {
                volume_toggle_mute();
                let _ = InvalidateRect(h, None, BOOL(0));
            }
            LRESULT(0)
        }
        // Paint is double-buffered; a background erase would only add flicker.
        WM_ERASEBKGND => LRESULT(1),
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
    BAR_FLOATING.store(cfg.bar_floating, Ordering::Relaxed);
    BAR_MARGIN.store(cfg.bar_margin as isize, Ordering::Relaxed);
    BAR_RADIUS.store(cfg.bar_radius as isize, Ordering::Relaxed);
    BAR_AUTOHIDE.store(cfg.bar_autohide, Ordering::Relaxed);
    BAR_WHEEL_WS.store(cfg.bar_wheel_ws, Ordering::Relaxed);
    NET_ON.store(cfg.bar_show_net, Ordering::Relaxed);
    VOL_ON.store(cfg.bar_show_volume, Ordering::Relaxed);
    STATS_ON.store(
        cfg.bar_show_cpu
            || cfg.bar_show_mem
            || cfg.bar_show_battery
            || cfg.bar_show_net
            || cfg.bar_show_volume,
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
        apply_theme(&cfg);
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
    drop(all);
    persist_hidden(mgr);
}

// ---- crash rescue -------------------------------------------------------------
// Astur hides inactive-workspace windows with SW_HIDE. Graceful exits restore
// them, but a hard kill (taskkill /F, Task Manager End task, a crash that skips
// the panic hook) cannot — the windows would stay hidden ("died"). So the
// manager persists the CURRENTLY HIDDEN set to ~/.astur/rescue.lst whenever it
// changes, and the next launch un-hides any verified survivors before adopting
// windows. A graceful restore deletes the file.
static LAST_RESCUE_HASH: AtomicU64 = AtomicU64::new(0);

fn rescue_file() -> std::path::PathBuf {
    config_path("ASTUR_RESCUE", "rescue.lst")
}

/// Write (or clear) the hidden-window rescue list. Cheap: hashes the hidden set
/// and returns without touching the disk when nothing changed (the common case —
/// it only actually writes on workspace switches and window moves).
fn persist_hidden(mgr: &Manager) {
    let mut hidden: Vec<isize> = Vec::new();
    for m in &mgr.monitors {
        for (wi, ws) in m.workspaces.iter().enumerate() {
            if wi != m.active {
                hidden.extend(ws.windows.iter().copied());
            }
        }
    }
    let mut hash: u64 = 0x9E37_79B9_7F4A_7C15 ^ hidden.len() as u64;
    for &h in &hidden {
        hash = hash.rotate_left(9) ^ (h as u64).wrapping_mul(0x0100_0000_01B3);
    }
    if LAST_RESCUE_HASH.swap(hash, Ordering::Relaxed) == hash {
        return;
    }
    let path = rescue_file();
    if hidden.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    let mut out = String::new();
    for &h in &hidden {
        unsafe {
            let hw = hwnd_from(h);
            let mut pid = 0u32;
            GetWindowThreadProcessId(hw, Some(&mut pid));
            let mut cls = [0u16; 64];
            let n = GetClassNameW(hw, &mut cls) as usize;
            // hwnd pid class — class may contain spaces, so it goes last.
            out.push_str(&format!(
                "{} {} {}\n",
                h,
                pid,
                String::from_utf16_lossy(&cls[..n])
            ));
        }
    }
    if let Some(p) = path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let _ = std::fs::write(&path, out);
}

/// Un-hide windows a previous Astur instance hid and then failed to restore.
/// Each entry is verified (same hwnd AND pid AND class) so a recycled HWND can
/// never make us show a window some other app deliberately hid. Runs once at
/// startup, before window adoption — rescued windows are then adopted normally
/// onto the active workspace of their monitor.
unsafe fn rescue_orphans() {
    let path = rescue_file();
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let mut n = 0u32;
    for line in text.lines() {
        let mut it = line.splitn(3, ' ');
        let (Some(hs), Some(ps), Some(cls)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let (Ok(h), Ok(pid)) = (hs.parse::<isize>(), ps.parse::<u32>()) else {
            continue;
        };
        let hw = hwnd_from(h);
        if !IsWindow(hw).as_bool() || IsWindowVisible(hw).as_bool() {
            continue;
        }
        let mut p = 0u32;
        GetWindowThreadProcessId(hw, Some(&mut p));
        let mut c = [0u16; 64];
        let cn = GetClassNameW(hw, &mut c) as usize;
        if p == pid && String::from_utf16_lossy(&c[..cn]) == cls {
            let _ = ShowWindow(hw, SW_SHOWNA);
            n += 1;
        }
    }
    let _ = std::fs::remove_file(&path);
    if n > 0 {
        println!("rescued {n} window(s) hidden by a previous session");
    }
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
            let h = hwnd.0 as isize;
            // Someone made it visible — whoever hid it, the marker is stale now
            // (and a later app-driven hide must untrack it again).
            unmark_hidden_by_us(h);
            if !SUPPRESS.load(Ordering::Relaxed) {
                push_cmd(Cmd::Add(h));
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
        EVENT_OBJECT_HIDE => {
            // Untrack only hides the APP performed (close-to-tray etc.). Hides
            // Astur performed for a workspace switch are marked in HIDDEN_BY_US;
            // SUPPRESS alone misses the tail of the batch (async delivery), and
            // untracking those orphaned live windows on hidden workspaces.
            let h = hwnd.0 as isize;
            if !SUPPRESS.load(Ordering::Relaxed) && !was_hidden_by_us(h) {
                push_cmd(Cmd::Remove(h));
            }
        }
        EVENT_OBJECT_DESTROY => {
            // A destroyed window is gone for real — always untrack (a Remove for
            // an untracked hwnd is a no-op, so this is safe even mid-switch).
            let h = hwnd.0 as isize;
            unmark_hidden_by_us(h);
            push_cmd(Cmd::Remove(h));
        }
        EVENT_SYSTEM_MINIMIZESTART | EVENT_SYSTEM_MINIMIZEEND => {
            push_cmd(Cmd::Retile);
        }
        // User finished a native (non-Alt) move/resize. Re-integrate the window
        // into the tiling: master keeps its new width as the ratio, everything
        // else snaps back so windows never overlap.
        EVENT_SYSTEM_MOVESIZEEND if !SUPPRESS.load(Ordering::Relaxed) => {
            // No preview rect here — the window is already where the user put it;
            // the manager reads the live rect (None).
            push_cmd(Cmd::DragResized(hwnd.0 as isize, None));
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
const LA_TAB: usize = 7; // toggle the wide column view (modified / size / path)
const LA_ACTIVATE_ALT: usize = 8; // Shift+Enter: open a file's containing folder
const LA_SCROLL: usize = 9; // mouse wheel: lParam = +1 (up) / -1 (down)
const LA_KEY: usize = 10; // raw key: lParam = vk | scan<<16 | shift<<32 | caps<<33

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
const LAUNCHER_WIDE_W: i32 = 1060; // Tab column view (clamped to the work area)
const LAUNCHER_H: i32 = 452;
const LAUNCHER_COLHDR: i32 = 22; // wide-mode column-header row height
const COL_DATE_W: i32 = 150; // "Modified" column
const COL_SIZE_W: i32 = 90; // "Size" column (right-aligned)
const LAUNCHER_ROW_H: i32 = 40;
const LAUNCHER_PAD: i32 = 16;
const LAUNCHER_HEADER: i32 = 54; // query row height
const LAUNCHER_ICON_PX: i32 = 32; // per-row app icon box (Start-Menu-ish size)
const LAUNCHER_SEL_RADIUS: i32 = 12; // rounded selection pill

// ---- popup theme (dark / light / auto) -------------------------------------
// The popups (launcher + system menu) read their palette at paint time, so a
// theme change in astur.conf hot-reloads without touching the windows.
struct Pal {
    bg: u32,
    fg: u32,
    dim: u32,
    selbg: u32,
    selfg: u32,
    frame: u32,
    divider: u32,
}
const PAL_DARK: Pal = Pal {
    bg: LAUNCHER_BG,
    fg: LAUNCHER_FG,
    dim: LAUNCHER_DIM,
    selbg: LAUNCHER_SELBG,
    selfg: LAUNCHER_SELFG,
    frame: LAUNCHER_FRAME,
    divider: LAUNCHER_DIVIDER,
};
const PAL_LIGHT: Pal = Pal {
    bg: 0x00F7_F4F2,      // #F2F4F7 — soft cool grey-white surface
    fg: 0x001A_1614,      // #14161A near-black text (strong contrast)
    dim: 0x0068_615C,     // #5C6168 readable muted grey
    selbg: LAUNCHER_SELBG, // same Forte-blue accent both themes
    selfg: 0x00FF_FFFF,
    frame: 0x00D4_CCC6,   // #C6CCD4 cool border
    divider: 0x00E6_E1DD, // #DDE1E6
};
static THEME_LIGHT: AtomicBool = AtomicBool::new(false);
fn pal() -> &'static Pal {
    if THEME_LIGHT.load(Ordering::Relaxed) {
        &PAL_LIGHT
    } else {
        &PAL_DARK
    }
}

/// Windows "apps use light theme" flag (Settings > Personalisation > Colours).
fn windows_apps_light() -> bool {
    unsafe {
        let sub: Vec<u16> = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let val: Vec<u16> = "AppsUseLightTheme"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut data: u32 = 0;
        let mut cb: u32 = core::mem::size_of::<u32>() as u32;
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(sub.as_ptr()),
            PCWSTR(val.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut c_void),
            Some(&mut cb),
        )
        .is_ok()
            && data == 1
    }
}

/// Resolve `theme = dark|light|auto` into THEME_LIGHT (startup + hot-reload).
fn apply_theme(cfg: &Config) {
    let light = match cfg.theme.as_str() {
        "light" => true,
        "auto" => windows_apps_light(),
        _ => false,
    };
    THEME_LIGHT.store(light, Ordering::Relaxed);
    ACRYLIC_ON.store(cfg.acrylic, Ordering::Relaxed);
}

// ---- acrylic backdrop (experimental) ---------------------------------------
// Undocumented user32!SetWindowCompositionAttribute with ACCENT_ENABLE_
// ACRYLICBLURBEHIND. The popup also gets whole-window alpha (layered) so the
// blur reads through the GDI-painted surface. Config-gated, default off.
static ACRYLIC_ON: AtomicBool = AtomicBool::new(false);
#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    gradient: u32, // AABBGGRR tint
    anim: u32,
}
#[repr(C)]
struct CompAttrData {
    attr: u32,
    pdata: *mut c_void,
    cb: u32,
}

/// Apply (or remove) the acrylic accent + layered alpha on a popup window.
/// Safe to call on every show — cheap, idempotent.
unsafe fn apply_acrylic(h: HWND, on: bool) {
    type SetWca = unsafe extern "system" fn(HWND, *mut CompAttrData) -> i32;
    let Ok(user32) = GetModuleHandleW(w!("user32.dll")) else { return };
    let Some(f) = GetProcAddress(user32, s!("SetWindowCompositionAttribute")) else { return };
    let f: SetWca = core::mem::transmute(f);
    let dark = !THEME_LIGHT.load(Ordering::Relaxed);
    let mut ap = AccentPolicy {
        state: if on { 4 } else { 0 }, // 4 = ACCENT_ENABLE_ACRYLICBLURBEHIND
        flags: 2,
        gradient: if dark { 0x99_10_10_10 } else { 0xCC_F2_EE_EC }, // AABBGGRR tint
        anim: 0,
    };
    let mut d = CompAttrData {
        attr: 19, // WCA_ACCENT_POLICY
        pdata: &mut ap as *mut _ as *mut c_void,
        cb: core::mem::size_of::<AccentPolicy>() as u32,
    };
    let _ = f(h, &mut d);
    // Slightly transparent window so the blur shows through the opaque GDI fill —
    // DARK theme only. In light mode the fade washes the light surface into
    // whatever light window sits underneath (text became unreadable), so the
    // popup stays fully opaque there and the accent is effectively cosmetic.
    let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
    let alpha = if on && dark { 236 } else { 255 };
    if on {
        SetWindowLongPtrW(h, GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as isize);
        let _ = SetLayeredWindowAttributes(h, COLORREF(0), alpha, LWA_ALPHA);
    } else if ex & WS_EX_LAYERED.0 as isize != 0 {
        let _ = SetLayeredWindowAttributes(h, COLORREF(0), 255, LWA_ALPHA);
    }
}

// ---- GDI back buffer --------------------------------------------------------
// All owner-drawn surfaces (launcher, system menu, bar) render into a memory DC
// and blit once. Painting straight to the window DC flashes: the bg fill wipes
// the previous frame on screen before the content lands (the launcher icons
// visibly blinked on every wheel scroll).
struct BackBuf {
    dc: HDC,
    bmp: windows::Win32::Graphics::Gdi::HBITMAP,
    old: HGDIOBJ,
    w: i32,
    h: i32,
}

unsafe fn backbuf_begin(win: HDC, w: i32, h: i32) -> Option<BackBuf> {
    let dc = CreateCompatibleDC(win);
    if dc.0.is_null() {
        return None;
    }
    let bmp = CreateCompatibleBitmap(win, w.max(1), h.max(1));
    if bmp.0.is_null() {
        let _ = DeleteDC(dc);
        return None;
    }
    let old = SelectObject(dc, HGDIOBJ(bmp.0));
    Some(BackBuf { dc, bmp, old, w, h })
}

unsafe fn backbuf_end(win: HDC, b: BackBuf) {
    let _ = BitBlt(win, 0, 0, b.w, b.h, b.dc, 0, 0, SRCCOPY);
    SelectObject(b.dc, b.old);
    let _ = DeleteObject(HGDIOBJ(b.bmp.0));
    let _ = DeleteDC(b.dc);
}

// ---- clipboard --------------------------------------------------------------

/// Put UTF-16 text on the clipboard (calculator result copy).
unsafe fn clipboard_set_text(h: HWND, s: &str) {
    let wide: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    if OpenClipboard(h).is_err() {
        return;
    }
    let _ = EmptyClipboard();
    let bytes = wide.len() * 2;
    if let Ok(hg) = GlobalAlloc(GMEM_MOVEABLE, bytes) {
        let p = GlobalLock(hg) as *mut u16;
        if !p.is_null() {
            std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
            let _ = GlobalUnlock(hg);
            // 13 = CF_UNICODETEXT. On success the system owns the memory.
            if SetClipboardData(13, HANDLE(hg.0)).is_err() {
                let _ = windows::Win32::Foundation::GlobalFree(hg);
            }
        } else {
            let _ = windows::Win32::Foundation::GlobalFree(hg);
        }
    }
    let _ = CloseClipboard();
}

// ---- inline calculator --------------------------------------------------------
// Tiny recursive-descent evaluator: + - * / % ^ parentheses, unary minus,
// decimals. Returns None on any parse error, so a non-maths query never shows
// a calc row.

struct CalcParser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> CalcParser<'a> {
    fn skip(&mut self) {
        while self.i < self.b.len() && self.b[self.i] == b' ' {
            self.i += 1;
        }
    }
    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        loop {
            self.skip();
            match self.b.get(self.i) {
                Some(b'+') => {
                    self.i += 1;
                    v += self.term()?;
                }
                Some(b'-') => {
                    self.i += 1;
                    v -= self.term()?;
                }
                _ => return Some(v),
            }
        }
    }
    fn term(&mut self) -> Option<f64> {
        let mut v = self.pow()?;
        loop {
            self.skip();
            match self.b.get(self.i) {
                Some(b'*') => {
                    self.i += 1;
                    v *= self.pow()?;
                }
                Some(b'/') => {
                    self.i += 1;
                    let d = self.pow()?;
                    if d == 0.0 {
                        return None;
                    }
                    v /= d;
                }
                Some(b'%') => {
                    self.i += 1;
                    let d = self.pow()?;
                    if d == 0.0 {
                        return None;
                    }
                    v %= d;
                }
                _ => return Some(v),
            }
        }
    }
    fn pow(&mut self) -> Option<f64> {
        let base = self.unary()?;
        self.skip();
        if self.b.get(self.i) == Some(&b'^') {
            self.i += 1;
            let e = self.pow()?; // right-associative
            return Some(base.powf(e));
        }
        Some(base)
    }
    fn unary(&mut self) -> Option<f64> {
        self.skip();
        if self.b.get(self.i) == Some(&b'-') {
            self.i += 1;
            return Some(-self.unary()?);
        }
        self.atom()
    }
    fn atom(&mut self) -> Option<f64> {
        self.skip();
        if self.b.get(self.i) == Some(&b'(') {
            self.i += 1;
            let v = self.expr()?;
            self.skip();
            if self.b.get(self.i) != Some(&b')') {
                return None;
            }
            self.i += 1;
            return Some(v);
        }
        let start = self.i;
        while self.b.get(self.i).is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
            self.i += 1;
        }
        if self.i == start {
            return None;
        }
        std::str::from_utf8(&self.b[start..self.i]).ok()?.parse().ok()
    }
}

/// Evaluate a maths query. Only fires when the text looks like an expression
/// (calc characters only, at least one operator, at least one digit) so app
/// names never trigger it.
fn calc_eval(q: &str) -> Option<f64> {
    let t = q.trim();
    if t.is_empty()
        || !t.bytes().all(|c| c.is_ascii_digit() || b"+-*/%^(). ".contains(&c))
        || !t.bytes().any(|c| c.is_ascii_digit())
        || !t.bytes().any(|c| b"+-*/%^".contains(&c))
    {
        return None;
    }
    let mut p = CalcParser { b: t.as_bytes(), i: 0 };
    let v = p.expr()?;
    p.skip();
    if p.i != p.b.len() || !v.is_finite() {
        return None;
    }
    Some(v)
}

/// Format a calc result: integers plainly, otherwise up to 10 significant
/// decimals with trailing zeros trimmed.
fn calc_fmt(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v:.10}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Open the default browser on a web search for `q`.
unsafe fn launcher_web_search(q: &str) {
    let mut url = String::from("https://www.google.com/search?q=");
    for b in q.trim().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                url.push(b as char)
            }
            b' ' => url.push('+'),
            _ => url.push_str(&format!("%{b:02X}")),
        }
    }
    launcher_launch(&url);
}

static LAUNCHER_OPEN: AtomicBool = AtomicBool::new(false);
static LAUNCHER_HWND: AtomicIsize = AtomicIsize::new(0);
static LAUNCHER_FONT: AtomicIsize = AtomicIsize::new(0);

// Launcher window bounds (screen coords), published on show so the global mouse
// hook can detect a click OUTSIDE the picker and dismiss it without a focus grab.
static LAUNCHER_RECT_L: AtomicI32 = AtomicI32::new(0);
static LAUNCHER_RECT_T: AtomicI32 = AtomicI32::new(0);
static LAUNCHER_RECT_R: AtomicI32 = AtomicI32::new(0);
static LAUNCHER_RECT_B: AtomicI32 = AtomicI32::new(0);
// Last screen-space cursor position the launcher evaluated for hover-select.
// Seeded on open so a popup appearing UNDER a still cursor can't steal selection;
// only a genuine move afterwards hovers.
static LAUNCHER_LAST_MX: AtomicI32 = AtomicI32::new(i32::MIN);
static LAUNCHER_LAST_MY: AtomicI32 = AtomicI32::new(i32::MIN);

// Lazy icon loader: paint enqueues visible app indices needing an icon; the icon
// worker resolves the shell icon to an HBITMAP off the UI thread and repaints.
static ICON_QUEUE: Mutex<VecDeque<usize>> = Mutex::new(VecDeque::new());
static ICON_CV: Condvar = Condvar::new();

struct AppEntry {
    name: String,
    name_lc: String,
    path: String,    // .lnk/.url file path, or `shell:AppsFolder\<id>` for UWP/system apps
    icon: isize,     // 0 = not yet loaded, -1 = none/failed, else an HICON (owned)
}
/// One file/folder result from the Windows Search index (Phase 3).
struct FileHit {
    name: String,
    path: String,
    size: i64,  // bytes (-1 = unknown / folder)
    date: f64,  // OLE automation date (days since 1899-12-30); 0 = unknown
}
/// A visible result row: an app (index into `all`), a file (index into `files`),
/// the inline calculator result, or the web-search fallback.
#[derive(Clone, Copy)]
enum Hit {
    App(usize),
    File(usize),
    Calc, // pinned first row when the query evaluates as maths (Enter copies)
    Web,  // "Search the web" fallback when nothing else matched
}
struct LauncherState {
    query: String,
    all: Vec<AppEntry>,
    files: Vec<FileHit>,   // current file-search results (top-N, replaced per query)
    filtered: Vec<Hit>,    // merged app + file rows, best first
    calc: Option<String>,  // formatted calculator result for the current query
    sel: usize,
    scroll: usize,         // first visible row (wheel scrolls; keyboard keeps sel visible)
    loaded: bool,
    wide: bool,            // Tab: wide column view (modified / size / path)
    search_gen: u64,       // generation of `files` (drops stale async results)
}
static LAUNCHER_STATE: Mutex<LauncherState> = Mutex::new(LauncherState {
    query: String::new(),
    all: Vec::new(),
    files: Vec::new(),
    filtered: Vec::new(),
    calc: None,
    sel: 0,
    scroll: 0,
    loaded: false,
    wide: false,
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

/// Resolve an app's icon to an HICON. Returns the HICON as an isize, or -1 on
/// failure. Runs on the icon worker (slow shell calls off the UI thread). Requires
/// COM initialised on the calling thread.
///
/// Primary = the system image list at JUMBO (256px) via `SHGetFileInfo` — the same
/// source Explorer/Start use, so file-backed apps (.lnk/.exe) get crisp, correctly
/// alpha'd icons (this is how "Start-Menu-quality" launchers do it). Fallback =
/// `IShellItemImageFactory` (handles UWP / `shell:AppsFolder` parsing names), whose
/// HBITMAP is wrapped into an HICON so the paint path is uniform (`DrawIconEx`).
unsafe fn load_icon(path: &str, px: i32) -> isize {
    // 1) Shell item image at EXACTLY the display size: the shell picks the best
    //    native frame and scales it high-quality, and it handles .lnk, .exe AND
    //    UWP (`shell:AppsFolder\…`) parsing names. Do NOT use SHIL_JUMBO here:
    //    icons with no 256px frame come back as a tiny 32px sprite in the CORNER
    //    of the 256px cell, and DrawIconEx's 256→32 downscale is low-quality —
    //    that combination was the "icon quality died" regression.
    if let Some(hicon) = shell_item_hicon(path, px) {
        return hicon.0 as isize;
    }
    // 2) System image list at native 32px (SHIL_LARGE == the display box, 1:1) —
    //    robust for odd .lnk/.exe paths where the item factory fails.
    if let Some(hicon) = sys_list_icon(path) {
        return hicon.0 as isize;
    }
    // 3) Generic executable icon so a row never renders blank.
    if let Some(hicon) = generic_app_icon() {
        return hicon.0 as isize;
    }
    -1
}

/// System image-list icon (SHIL_LARGE, native 32px) for a file-backed shell path.
unsafe fn sys_list_icon(path: &str) -> Option<HICON> {
    let mut w: Vec<u16> = path.encode_utf16().collect();
    w.push(0);
    let mut shfi = SHFILEINFOW::default();
    let r = SHGetFileInfoW(
        PCWSTR(w.as_ptr()),
        FILE_FLAGS_AND_ATTRIBUTES(0),
        Some(&mut shfi),
        std::mem::size_of::<SHFILEINFOW>() as u32,
        SHGFI_SYSICONINDEX,
    );
    if r == 0 {
        return None;
    }
    let il: IImageList = SHGetImageList(SHIL_LARGE as i32).ok()?;
    let hicon = il.GetIcon(shfi.iIcon, ILD_TRANSPARENT.0).ok()?;
    (!hicon.0.is_null()).then_some(hicon)
}

/// Cached generic "application" icon (the shell's default .exe icon), used when
/// both real resolvers fail so the row still shows something. 0 = not yet
/// resolved, -1 = resolution failed, else an HICON we own for the process life.
static GENERIC_APP_ICON: AtomicIsize = AtomicIsize::new(0);

unsafe fn generic_app_icon() -> Option<HICON> {
    let cached = GENERIC_APP_ICON.load(Ordering::Relaxed);
    if cached == -1 {
        return None;
    }
    if cached != 0 {
        return Some(HICON(cached as *mut c_void));
    }
    // SHGFI_USEFILEATTRIBUTES: resolve by name+attributes only — the file need
    // not exist, we just want the shell's stock icon for "an .exe".
    let name: Vec<u16> = "app.exe".encode_utf16().chain(std::iter::once(0)).collect();
    let mut shfi = SHFILEINFOW::default();
    let r = SHGetFileInfoW(
        PCWSTR(name.as_ptr()),
        FILE_ATTRIBUTE_NORMAL,
        Some(&mut shfi),
        std::mem::size_of::<SHFILEINFOW>() as u32,
        SHGFI_FLAGS(SHGFI_SYSICONINDEX.0 | SHGFI_USEFILEATTRIBUTES.0),
    );
    let hicon = if r != 0 {
        SHGetImageList::<IImageList>(SHIL_LARGE as i32)
            .ok()
            .and_then(|il| il.GetIcon(shfi.iIcon, ILD_TRANSPARENT.0).ok())
            .filter(|h| !h.0.is_null())
    } else {
        None
    };
    GENERIC_APP_ICON.store(hicon.map_or(-1, |h| h.0 as isize), Ordering::Relaxed);
    hicon
}

/// Primary resolver: an `IShellItemImageFactory` image at `px` square, wrapped into
/// an HICON so the paint path is uniform (`DrawIconEx`). The factory handles .lnk,
/// .exe and UWP (`shell:AppsFolder\…`) parsing names, and scales from the icon's
/// best native frame with high quality — request the EXACT display size and blit 1:1.
unsafe fn shell_item_hicon(path: &str, px: i32) -> Option<HICON> {
    let mut w: Vec<u16> = path.encode_utf16().collect();
    w.push(0);
    let factory: IShellItemImageFactory =
        SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).ok()?;
    let hb = factory.GetImage(SIZE { cx: px, cy: px }, SIIGBF_ICONONLY).ok()?;
    // Monochrome AND-mask, zeroed: with a 32bpp colour bitmap the per-pixel alpha
    // drives transparency, so an all-0 mask is correct. CreateIconIndirect requires one.
    let stride = (((px + 15) & !15) / 8) as usize;
    let mask_bits = vec![0u8; stride * px as usize];
    let mask = CreateBitmap(px, px, 1, 1, Some(mask_bits.as_ptr() as *const c_void));
    let ii = ICONINFO {
        fIcon: BOOL(1),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: hb,
    };
    let hicon = CreateIconIndirect(&ii).ok();
    // CreateIconIndirect copies the bitmaps; free the sources.
    let _ = DeleteObject(HGDIOBJ(mask.0));
    let _ = DeleteObject(HGDIOBJ(hb.0));
    hicon.filter(|h| !h.0.is_null())
}

/// Icon worker: drains `ICON_QUEUE`, resolves each app's shell icon to an HICON,
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
            let hicon = load_icon(&path, LAUNCHER_ICON_PX);
            {
                let mut st = LAUNCHER_STATE.lock().unwrap();
                if let Some(e) = st.all.get_mut(idx) {
                    e.icon = hicon;
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
    // Inline calculator: a maths-looking query pins its result as the first row.
    st.calc = calc_eval(&st.query).map(calc_fmt);
    let mut filtered: Vec<Hit> = Vec::new();
    if st.calc.is_some() {
        filtered.push(Hit::Calc);
    }
    let mut scored: Vec<(i32, usize)> = st
        .all
        .iter()
        .enumerate()
        .filter_map(|(i, e)| fuzzy_score(&q, &e.name_lc).map(|s| (s, i)))
        .collect();
    // Best score first; ties keep alphabetical order (all is pre-sorted, stable).
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    filtered.extend(scored.into_iter().map(|(_, i)| Hit::App(i)));
    // File results (from the async index worker, already query-filtered) after apps —
    // apps are instant and the common case, so they never wait on the index.
    for i in 0..st.files.len() {
        filtered.push(Hit::File(i));
    }
    // Nothing matched a non-empty query: offer a web search as the only row.
    if filtered.is_empty() && !st.query.trim().is_empty() {
        filtered.push(Hit::Web);
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
/// Size + center the picker on `wa`, publish its bounds for the mouse hook
/// (click-outside dismiss + wheel routing), and repaint. `wide` = the Tab column
/// view; the width is clamped to the work area on small screens.
unsafe fn launcher_place(h: HWND, wa: RECT, wide: bool) {
    let want = if wide { LAUNCHER_WIDE_W } else { LAUNCHER_W };
    let win_w = want.min(wa.right - wa.left - 48).max(320);
    let x = (wa.left + wa.right) / 2 - win_w / 2;
    let y = (wa.top + wa.bottom) / 2 - LAUNCHER_H / 2;
    let _ = SetWindowPos(h, HWND_TOPMOST, x, y, win_w, LAUNCHER_H, SWP_NOACTIVATE);
    LAUNCHER_RECT_L.store(x, Ordering::Relaxed);
    LAUNCHER_RECT_T.store(y, Ordering::Relaxed);
    LAUNCHER_RECT_R.store(x + win_w, Ordering::Relaxed);
    LAUNCHER_RECT_B.store(y + LAUNCHER_H, Ordering::Relaxed);
    let _ = InvalidateRect(h, None, BOOL(0));
}

unsafe fn launcher_show(h: HWND) {
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    launcher_place(h, work_area_at(pt), false);
    // Hover baseline: the popup opens under a possibly-still cursor; only a real
    // move after this may hover-select.
    LAUNCHER_LAST_MX.store(pt.x, Ordering::Relaxed);
    LAUNCHER_LAST_MY.store(pt.y, Ordering::Relaxed);
    apply_acrylic(h, ACRYLIC_ON.load(Ordering::Relaxed));
    let _ = ShowWindow(h, SW_SHOWNA);
}

/// Hide the launcher and reset transient state.
unsafe fn launcher_close(h: HWND) {
    let _ = ShowWindow(h, SW_HIDE);
    LAUNCHER_OPEN.store(false, Ordering::Relaxed);
    let mut st = LAUNCHER_STATE.lock().unwrap();
    st.query.clear();
    st.sel = 0;
    st.scroll = 0;
    st.files.clear();
    st.calc = None;
    st.wide = false;
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

// --- Launcher list geometry (paint + mouse hit-testing share these) ---------

/// Top of the result list in client coords (below the query row, and below the
/// column-header row in wide mode).
fn launcher_list_top(st: &LauncherState) -> i32 {
    LAUNCHER_HEADER + 6 + if st.wide { LAUNCHER_COLHDR } else { 0 }
}

/// Visible list rows for the current mode + client height.
fn launcher_rows(st: &LauncherState, ht: i32) -> usize {
    (((ht - 4) - launcher_list_top(st)) / LAUNCHER_ROW_H).max(1) as usize
}

/// Stored scroll clamped so the viewport never runs past the end of the list.
fn launcher_scroll(st: &LauncherState, rows: usize) -> usize {
    st.scroll.min(st.filtered.len().saturating_sub(rows))
}

/// Result-row index under a client-space `y`, or None on chrome/padding/empties.
fn launcher_row_hit(st: &LauncherState, ht: i32, y: i32) -> Option<usize> {
    let list_top = launcher_list_top(st);
    if y < list_top || y >= ht - 4 {
        return None;
    }
    let vis = ((y - list_top) / LAUNCHER_ROW_H) as usize;
    let rows = launcher_rows(st, ht);
    if vis >= rows {
        return None;
    }
    let idx = launcher_scroll(st, rows) + vis;
    (idx < st.filtered.len()).then_some(idx)
}

unsafe fn launcher_paint(h: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let win_hdc = BeginPaint(h, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(h, &mut rc);
    let w = rc.right - rc.left;
    let ht = rc.bottom - rc.top;
    // Double buffer: render off-screen, blit once (no bg-wipe flash on scroll).
    let bb = backbuf_begin(win_hdc, w, ht);
    let hdc = bb.as_ref().map(|b| b.dc).unwrap_or(win_hdc);
    let p = pal();

    // Thin 1px frame, then the surface inset inside it (DWM rounds the outer
    // corners, so this reads as a clean bordered card).
    let frame = CreateSolidBrush(COLORREF(p.frame));
    FillRect(hdc, &rc, frame);
    let _ = DeleteObject(HGDIOBJ(frame.0));
    let inner = RECT {
        left: rc.left + 1,
        top: rc.top + 1,
        right: rc.right - 1,
        bottom: rc.bottom - 1,
    };
    let bg = CreateSolidBrush(COLORREF(p.bg));
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
        SetTextColor(hdc, COLORREF(p.dim));
        let mut v: Vec<u16> = "Search apps and files…".encode_utf16().collect();
        DrawTextW(hdc, &mut v, &mut qr, DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    } else {
        SetTextColor(hdc, COLORREF(p.fg));
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
    let dbrush = CreateSolidBrush(COLORREF(p.divider));
    FillRect(hdc, &div, dbrush);
    let _ = DeleteObject(HGDIOBJ(dbrush.0));

    // Result rows: st.scroll drives the viewport (wheel scrolls it; the keyboard
    // arms keep the selection visible). Wide (Tab) adds Modified/Size/Path columns.
    let list_top = launcher_list_top(&st);
    let rows = launcher_rows(&st, ht);
    let scroll = launcher_scroll(&st, rows);
    let text_left = LAUNCHER_PAD + 6 + LAUNCHER_ICON_PX + 10;
    // Wide-mode column x's, anchored off the right edge; path gets the big share.
    let col_path_w = (w as f64 * 0.40) as i32;
    let path_x = w - LAUNCHER_PAD - 6 - col_path_w;
    let size_x = path_x - COL_SIZE_W;
    let date_x = size_x - COL_DATE_W;
    if st.wide {
        // Dim column headers in the band under the query divider.
        SetTextColor(hdc, COLORREF(p.dim));
        let hdr = |x0: i32, x1: i32, label: &str, extra: DRAW_TEXT_FORMAT| {
            let mut r = RECT {
                left: x0,
                top: LAUNCHER_HEADER + 2,
                right: x1,
                bottom: LAUNCHER_HEADER + 2 + LAUNCHER_COLHDR,
            };
            let mut v: Vec<u16> = label.encode_utf16().collect();
            DrawTextW(hdc, &mut v, &mut r, DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | extra);
        };
        hdr(text_left, date_x - 10, "Name", DRAW_TEXT_FORMAT(0));
        hdr(date_x, size_x - 8, "Modified", DRAW_TEXT_FORMAT(0));
        hdr(size_x, size_x + COL_SIZE_W - 16, "Size", DT_RIGHT);
        hdr(path_x, w - LAUNCHER_PAD, "Path", DRAW_TEXT_FORMAT(0));
    }
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
            let sel = CreateSolidBrush(COLORREF(p.selbg));
            let pen = CreatePen(PS_SOLID, 1, COLORREF(p.selbg));
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
            SetTextColor(hdc, COLORREF(p.selfg));
        } else {
            SetTextColor(hdc, COLORREF(p.fg));
        }
        // Calculator / web-search rows: a marker glyph in the icon box + one line
        // of text; no wide-mode meta cells.
        match hit {
            Hit::Calc | Hit::Web => {
                let glyph = if matches!(hit, Hit::Calc) { "=" } else { "\u{2192}" };
                let mut gr = RECT {
                    left: row.left + 6,
                    top,
                    right: row.left + 6 + LAUNCHER_ICON_PX,
                    bottom: top + LAUNCHER_ROW_H,
                };
                let keep = if idx == st.sel { p.selfg } else { p.dim };
                SetTextColor(hdc, COLORREF(keep));
                let mut gv: Vec<u16> = glyph.encode_utf16().collect();
                DrawTextW(hdc, &mut gv, &mut gr, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
                let text = if matches!(hit, Hit::Calc) {
                    format!(
                        "{}   (Enter copies)",
                        st.calc.as_deref().unwrap_or("")
                    )
                } else {
                    format!("Search the web for \u{201c}{}\u{201d}", st.query.trim())
                };
                SetTextColor(hdc, COLORREF(if idx == st.sel { p.selfg } else { p.fg }));
                let mut tr = RECT { left: text_left, ..row };
                let mut v: Vec<u16> = text.encode_utf16().collect();
                DrawTextW(
                    hdc,
                    &mut v,
                    &mut tr,
                    DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
                );
                continue;
            }
            _ => {}
        }
        // Resolve name + wide-mode meta cells (+ apps' lazy icon).
        let (name, date_s, size_s, path_s): (&str, String, String, &str) = match hit {
            Hit::App(i) => {
                let e = &st.all[i];
                // App icon, loaded lazily off the UI thread; missing ones queue + pop in.
                if e.icon > 1 {
                    // The HICON was resolved at exactly LAUNCHER_ICON_PX, so this is
                    // a 1:1 draw (no scaling blur); DrawIconEx composites the icon's
                    // own straight alpha — no premultiply, no halo.
                    let hicon = HICON(e.icon as *mut c_void);
                    let iy = top + (LAUNCHER_ROW_H - LAUNCHER_ICON_PX) / 2;
                    let _ = DrawIconEx(
                        hdc,
                        row.left + 6,
                        iy,
                        hicon,
                        LAUNCHER_ICON_PX,
                        LAUNCHER_ICON_PX,
                        0,
                        None,
                        DI_NORMAL,
                    );
                } else if e.icon == 0 {
                    want.push(i);
                }
                (e.name.as_str(), String::new(), String::new(), e.path.as_str())
            }
            Hit::File(i) => {
                // File rows: a small dim square marks them (no shell icon yet); the
                // wide view carries date/size/path in columns.
                if idx != st.sel {
                    let mk = CreateSolidBrush(COLORREF(p.dim));
                    let g = RECT {
                        left: row.left + 6 + (LAUNCHER_ICON_PX - 14) / 2,
                        top: top + (LAUNCHER_ROW_H - 14) / 2,
                        right: row.left + 6 + (LAUNCHER_ICON_PX - 14) / 2 + 14,
                        bottom: top + (LAUNCHER_ROW_H - 14) / 2 + 14,
                    };
                    FillRect(hdc, &g, mk);
                    let _ = DeleteObject(HGDIOBJ(mk.0));
                }
                let f = &st.files[i];
                (
                    f.name.as_str(),
                    if f.date > 0.0 { fmt_oadate(f.date) } else { String::new() },
                    fmt_size(f.size),
                    f.path.as_str(),
                )
            }
            // Drawn above (with an early continue).
            Hit::Calc | Hit::Web => unreachable!(),
        };
        let mut tr = RECT {
            left: text_left,
            right: if st.wide { date_x - 10 } else { row.right },
            ..row
        };
        let mut v: Vec<u16> = name.encode_utf16().collect();
        DrawTextW(
            hdc,
            &mut v,
            &mut tr,
            DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        if st.wide {
            // Meta cells: dim normally, selection-white on the accent pill.
            SetTextColor(
                hdc,
                COLORREF(if idx == st.sel { p.selfg } else { p.dim }),
            );
            let cell = |x0: i32, x1: i32, s: &str, extra: DRAW_TEXT_FORMAT| {
                if s.is_empty() {
                    return;
                }
                let mut r = RECT { left: x0, right: x1, ..row };
                let mut v: Vec<u16> = s.encode_utf16().collect();
                DrawTextW(
                    hdc,
                    &mut v,
                    &mut r,
                    DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS | extra,
                );
            };
            cell(date_x, size_x - 8, &date_s, DRAW_TEXT_FORMAT(0));
            cell(size_x, size_x + COL_SIZE_W - 16, &size_s, DT_RIGHT);
            cell(path_x, w - LAUNCHER_PAD, path_s, DRAW_TEXT_FORMAT(0));
        }
    }

    if let Some(of) = old_font {
        SelectObject(hdc, of);
    }
    drop(st);
    if let Some(b) = bb {
        backbuf_end(win_hdc, b);
    }
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
                        st.scroll = 0;
                        st.files.clear();
                        st.wide = false;
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
                        st.scroll = 0;
                        st.files.clear(); // stale results vanish until the new query returns
                        launcher_refilter(&mut st);
                        st.query.clone()
                    };
                    launcher_dispatch_search(&q);
                    let _ = InvalidateRect(h, None, BOOL(0));
                }
                LA_KEY => {
                    // Raw key from the hook: vk | scan<<16 | shift<<32 | caps<<33.
                    // ToUnicode here (off the hook thread) with a synthetic key
                    // state, so Shift and CapsLock produce the right character —
                    // capitals, and the calculator's + * ( ) ^ % symbols.
                    let vk = (l.0 & 0xFFFF) as u32;
                    let scan = ((l.0 >> 16) & 0xFFFF) as u32;
                    let shift = (l.0 >> 32) & 1 != 0;
                    let caps = (l.0 >> 33) & 1 != 0;
                    let mut state = [0u8; 256];
                    if shift {
                        state[VK_SHIFT.0 as usize] = 0x80;
                    }
                    if caps {
                        state[VK_CAPITAL.0 as usize] = 0x01;
                    }
                    let mut buf = [0u16; 8];
                    let n = ToUnicode(vk, scan, Some(&state), &mut buf, 0);
                    if n >= 1 {
                        if let Some(c) = char::decode_utf16(buf[..n as usize].iter().copied())
                            .next()
                            .and_then(|r| r.ok())
                            .filter(|c| *c >= ' ')
                        {
                            let _ = PostMessageW(
                                h,
                                WM_LAUNCHER,
                                WPARAM(LA_CHAR),
                                LPARAM(c as isize),
                            );
                        }
                    }
                }
                LA_BACK => {
                    let q = {
                        let mut st = LAUNCHER_STATE.lock().unwrap();
                        st.query.pop();
                        st.sel = 0;
                        st.scroll = 0;
                        st.files.clear();
                        launcher_refilter(&mut st);
                        st.query.clone()
                    };
                    launcher_dispatch_search(&q);
                    let _ = InvalidateRect(h, None, BOOL(0));
                }
                LA_UP => {
                    let mut rc = RECT::default();
                    let _ = GetClientRect(h, &mut rc);
                    {
                        let mut st = LAUNCHER_STATE.lock().unwrap();
                        if st.sel > 0 {
                            st.sel -= 1;
                        }
                        // Keep the keyboard selection visible in the scrolled viewport.
                        let rows = launcher_rows(&st, rc.bottom);
                        if st.sel < launcher_scroll(&st, rows) {
                            st.scroll = st.sel;
                        }
                    }
                    let _ = InvalidateRect(h, None, BOOL(0));
                }
                LA_DOWN => {
                    let mut rc = RECT::default();
                    let _ = GetClientRect(h, &mut rc);
                    {
                        let mut st = LAUNCHER_STATE.lock().unwrap();
                        if st.sel + 1 < st.filtered.len() {
                            st.sel += 1;
                        }
                        let rows = launcher_rows(&st, rc.bottom);
                        if st.sel >= launcher_scroll(&st, rows) + rows {
                            st.scroll = st.sel + 1 - rows;
                        }
                    }
                    let _ = InvalidateRect(h, None, BOOL(0));
                }
                LA_ACTIVATE => {
                    // Enter: launch the app / open the file / copy the calc result /
                    // run the web-search fallback.
                    enum Act {
                        Open(String),
                        Copy(String),
                        Web(String),
                        None,
                    }
                    let action = {
                        let st = LAUNCHER_STATE.lock().unwrap();
                        match st.filtered.get(st.sel) {
                            Some(Hit::App(i)) => {
                                st.all.get(*i).map(|e| Act::Open(e.path.clone())).unwrap_or(Act::None)
                            }
                            Some(Hit::File(i)) => {
                                st.files.get(*i).map(|f| Act::Open(f.path.clone())).unwrap_or(Act::None)
                            }
                            Some(Hit::Calc) => {
                                st.calc.clone().map(Act::Copy).unwrap_or(Act::None)
                            }
                            Some(Hit::Web) => Act::Web(st.query.trim().to_string()),
                            None => Act::None,
                        }
                    };
                    // Copy needs the window alive as the clipboard owner; do it
                    // before closing.
                    if let Act::Copy(s) = &action {
                        clipboard_set_text(h, s);
                    }
                    launcher_close(h);
                    match action {
                        Act::Open(p) => launcher_launch(&p),
                        Act::Web(q) => launcher_web_search(&q),
                        _ => {}
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
                    // Tab toggles the wide column view; resize + recenter in place
                    // (on the monitor the picker is on) and republish the bounds.
                    let wide = {
                        let mut st = LAUNCHER_STATE.lock().unwrap();
                        st.wide = !st.wide;
                        st.wide
                    };
                    let mon = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
                    let mut mi = MONITORINFO {
                        cbSize: core::mem::size_of::<MONITORINFO>() as u32,
                        ..Default::default()
                    };
                    let wa = if GetMonitorInfoW(mon, &mut mi).as_bool() {
                        mi.rcWork
                    } else {
                        RECT { left: 0, top: 0, right: 1920, bottom: 1080 }
                    };
                    launcher_place(h, wa, wide);
                }
                LA_SCROLL => {
                    // Mouse wheel: scroll the viewport; drag the selection along so
                    // Enter always acts on a visible row. Skip the repaint entirely
                    // when nothing changed (short list, or already at either end).
                    let mut rc = RECT::default();
                    let _ = GetClientRect(h, &mut rc);
                    let changed = {
                        let mut st = LAUNCHER_STATE.lock().unwrap();
                        let rows = launcher_rows(&st, rc.bottom);
                        let maxs = st.filtered.len().saturating_sub(rows);
                        let cur = launcher_scroll(&st, rows);
                        let next = if l.0 > 0 { cur.saturating_sub(1) } else { (cur + 1).min(maxs) };
                        let old_sel = st.sel;
                        st.scroll = next;
                        if !st.filtered.is_empty() {
                            let last = st.filtered.len() - 1;
                            st.sel = st.sel.clamp(next, (next + rows - 1).min(last));
                        }
                        next != cur || st.sel != old_sel
                    };
                    if changed {
                        let _ = InvalidateRect(h, None, BOOL(0));
                    }
                }
                LA_CLOSE => launcher_close(h),
                _ => {}
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            // Hover-select. Screen-space move guard: the popup can open (or resize)
            // under a still cursor, and the synthetic WM_MOUSEMOVE that generates
            // must not steal the keyboard selection.
            let mx = (l.0 & 0xFFFF) as i16 as i32;
            let my = ((l.0 >> 16) & 0xFFFF) as i16 as i32;
            let sx = LAUNCHER_RECT_L.load(Ordering::Relaxed) + mx;
            let sy = LAUNCHER_RECT_T.load(Ordering::Relaxed) + my;
            if sx == LAUNCHER_LAST_MX.load(Ordering::Relaxed)
                && sy == LAUNCHER_LAST_MY.load(Ordering::Relaxed)
            {
                return LRESULT(0);
            }
            LAUNCHER_LAST_MX.store(sx, Ordering::Relaxed);
            LAUNCHER_LAST_MY.store(sy, Ordering::Relaxed);
            let mut rc = RECT::default();
            let _ = GetClientRect(h, &mut rc);
            let repaint = {
                let mut st = LAUNCHER_STATE.lock().unwrap();
                match launcher_row_hit(&st, rc.bottom, my) {
                    Some(idx) if idx != st.sel => {
                        st.sel = idx;
                        true
                    }
                    _ => false,
                }
            };
            if repaint {
                let _ = InvalidateRect(h, None, BOOL(0));
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            // Click activates the row under the cursor (select, then the same code
            // path as Enter). Clicks on chrome/padding do nothing.
            let my = ((l.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut rc = RECT::default();
            let _ = GetClientRect(h, &mut rc);
            let hit = {
                let mut st = LAUNCHER_STATE.lock().unwrap();
                match launcher_row_hit(&st, rc.bottom, my) {
                    Some(idx) => {
                        st.sel = idx;
                        true
                    }
                    None => false,
                }
            };
            if hit {
                let _ = PostMessageW(h, WM_LAUNCHER, WPARAM(LA_ACTIVATE), LPARAM(0));
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
// Menu bounds (screen coords), published by sysmenu_layout for the mouse hook's
// click-outside-dismiss + wheel routing (same scheme as the launcher).
static SYSMENU_RECT_L: AtomicI32 = AtomicI32::new(0);
static SYSMENU_RECT_T: AtomicI32 = AtomicI32::new(0);
static SYSMENU_RECT_R: AtomicI32 = AtomicI32::new(0);
static SYSMENU_RECT_B: AtomicI32 = AtomicI32::new(0);
// Hover-select move baseline (see LAUNCHER_LAST_MX).
static SYSMENU_LAST_MX: AtomicI32 = AtomicI32::new(i32::MIN);
static SYSMENU_LAST_MY: AtomicI32 = AtomicI32::new(i32::MIN);

#[derive(Clone, Copy, PartialEq)]
enum SysAct {
    Lock,
    Sleep,
    SignOut,
    Restart,
    Shutdown,
    OpenConfig,
    OpenSettings,
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
    SysItem { label: "Settings", kind: SysKind::Action(SysAct::OpenSettings, false) },
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
        SysAct::OpenSettings => tray_open_settings(),
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
    // Publish bounds for the hook's click-outside dismiss + wheel routing, and
    // re-seed the hover baseline (the menu just moved/resized under the cursor).
    SYSMENU_RECT_L.store(x, Ordering::Relaxed);
    SYSMENU_RECT_T.store(y, Ordering::Relaxed);
    SYSMENU_RECT_R.store(x + SYSMENU_W, Ordering::Relaxed);
    SYSMENU_RECT_B.store(y + hgt, Ordering::Relaxed);
    SYSMENU_LAST_MX.store(pt.x, Ordering::Relaxed);
    SYSMENU_LAST_MY.store(pt.y, Ordering::Relaxed);
    let _ = InvalidateRect(h, None, BOOL(0));
}

/// Menu-row index under a client-space `y` (rows sit under the title, fixed pitch).
fn sysmenu_row_hit(n: usize, y: i32) -> Option<usize> {
    let top = SYSMENU_HEADER + 6;
    if y < top {
        return None;
    }
    let i = ((y - top) / LAUNCHER_ROW_H) as usize;
    (i < n).then_some(i)
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
    apply_acrylic(h, ACRYLIC_ON.load(Ordering::Relaxed));
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
    let win_hdc = BeginPaint(h, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(h, &mut rc);
    let w = rc.right - rc.left;
    // Double buffer (see launcher_paint) — no bg-wipe flash on wheel/hover.
    let bb = backbuf_begin(win_hdc, w, rc.bottom - rc.top);
    let hdc = bb.as_ref().map(|b| b.dc).unwrap_or(win_hdc);
    let p = pal();

    let frame = CreateSolidBrush(COLORREF(p.frame));
    FillRect(hdc, &rc, frame);
    let _ = DeleteObject(HGDIOBJ(frame.0));
    let inner = RECT {
        left: rc.left + 1,
        top: rc.top + 1,
        right: rc.right - 1,
        bottom: rc.bottom - 1,
    };
    let bg = CreateSolidBrush(COLORREF(p.bg));
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
    SetTextColor(hdc, COLORREF(p.dim));
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
    let db = CreateSolidBrush(COLORREF(p.divider));
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
            let sel = CreateSolidBrush(COLORREF(p.selbg));
            let pen = CreatePen(PS_SOLID, 1, COLORREF(p.selbg));
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
            SetTextColor(hdc, COLORREF(p.selfg));
        } else {
            SetTextColor(hdc, COLORREF(p.fg));
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
    SetTextColor(hdc, COLORREF(if st.confirm { p.selbg } else { p.dim }));
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
    if let Some(b) = bb {
        backbuf_end(win_hdc, b);
    }
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
        WM_MOUSEMOVE => {
            // Hover-select (same move guard as the launcher). A selection change
            // also disarms a pending confirm — confirm belongs to the armed row.
            let mx = (l.0 & 0xFFFF) as i16 as i32;
            let my = ((l.0 >> 16) & 0xFFFF) as i16 as i32;
            let sx = SYSMENU_RECT_L.load(Ordering::Relaxed) + mx;
            let sy = SYSMENU_RECT_T.load(Ordering::Relaxed) + my;
            if sx == SYSMENU_LAST_MX.load(Ordering::Relaxed)
                && sy == SYSMENU_LAST_MY.load(Ordering::Relaxed)
            {
                return LRESULT(0);
            }
            SYSMENU_LAST_MX.store(sx, Ordering::Relaxed);
            SYSMENU_LAST_MY.store(sy, Ordering::Relaxed);
            let repaint = {
                let mut st = SYSMENU_STATE.lock().unwrap();
                match sysmenu_row_hit(st.items.len(), my) {
                    Some(i) if i != st.sel => {
                        st.sel = i;
                        st.confirm = false;
                        true
                    }
                    _ => false,
                }
            };
            if repaint {
                let _ = InvalidateRect(h, None, BOOL(0));
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            // Click = select + the same activate path as Enter (drill a category,
            // arm/execute a confirm-gated action, run a plain action).
            let my = ((l.0 >> 16) & 0xFFFF) as i16 as i32;
            let hit = {
                let mut st = SYSMENU_STATE.lock().unwrap();
                match sysmenu_row_hit(st.items.len(), my) {
                    Some(i) => {
                        if i != st.sel {
                            st.sel = i;
                            st.confirm = false;
                        }
                        true
                    }
                    None => false,
                }
            };
            if hit {
                let _ = PostMessageW(h, WM_SYSMENU, WPARAM(SM_ACTIVATE), LPARAM(0));
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
        apply_theme(&cfg);

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

        // Drag-outline overlay: an accent-coloured hollow frame previewing the
        // move/resize target. Region-shaped per drag; layered + click-through so it
        // never eats input. A plain DefWindowProc window — it must NOT share
        // marker_wndproc (that handles WM_DISPLAYCHANGE/WM_RELOAD, which would then
        // double-fire the bar/monitor rebuild).
        let outline_brush = CreateSolidBrush(COLORREF(LAUNCHER_SELBG)); // #366382 accent
        let outline_wc = WNDCLASSW {
            lpfnWndProc: Some(outline_wndproc),
            hInstance: hinst,
            hbrBackground: outline_brush,
            lpszClassName: w!("astur_outline"),
            ..Default::default()
        };
        RegisterClassW(&outline_wc);
        let outline = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("astur_outline"),
            w!(""),
            WS_POPUP,
            0,
            0,
            10,
            10,
            None,
            None,
            hinst,
            None,
        )
        .expect("CreateWindowExW failed");
        let _ = SetLayeredWindowAttributes(outline, COLORREF(0), 220, LWA_ALPHA);
        OUTLINE_HWND.store(outline.0 as isize, Ordering::Relaxed);

        // Thumbnail overlay: a plain (non-layered) topmost tool window DWM renders
        // the live window mirror into during a move-drag. Black background is never
        // seen — the thumbnail fills the whole client.
        let thumb_wc = WNDCLASSW {
            lpfnWndProc: Some(outline_wndproc),
            hInstance: hinst,
            hbrBackground: CreateSolidBrush(COLORREF(0)),
            lpszClassName: w!("astur_thumb"),
            ..Default::default()
        };
        RegisterClassW(&thumb_wc);
        let thumb = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            w!("astur_thumb"),
            w!(""),
            WS_POPUP,
            0,
            0,
            10,
            10,
            None,
            None,
            hinst,
            None,
        )
        .expect("CreateWindowExW failed");
        THUMB_HWND.store(thumb.0 as isize, Ordering::Relaxed);

        // Status bar on every monitor (waybar-style). Register the class once,
        // build the font, then create a bar window per monitor.
        if cfg.bar_enabled && cfg.bar_height > 0 {
            // Class brush is a first-frame fallback only (paint is buffered).
            let bar_brush = CreateSolidBrush(COLORREF(themed_bar_colors(&cfg).0));
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
        // Crash rescue: un-hide anything a previous (killed) instance left hidden
        // BEFORE the manager adopts windows, so they're adopted visible.
        rescue_orphans();
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

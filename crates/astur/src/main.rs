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

use std::sync::atomic::{
    AtomicBool, AtomicI32, AtomicIsize, AtomicU32, AtomicU64, AtomicU8, Ordering,
};
use std::sync::{Condvar, LazyLock, Mutex, OnceLock};
use std::time::Instant;

mod layout;
// Config now lives in the shared `astur-config` crate (the settings GUI parses the
// same model). Aliased to `config` so the rest of this file is unchanged.
use astur_config as config;
use config::{config_path, load_config, Config, HotkeyDef, WindowRule};
use layout::{
    columns_layout, dwindle_layout, grid_layout, master_stack, monocle_layout, resize_dwindle,
    split_ratio,
};

use windows::core::{w, PCWSTR};
use windows::core::{IUnknown, Interface, GUID};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, BOOL, BOOLEAN, COLORREF, HANDLE, HINSTANCE, HLOCAL, HWND,
    INVALID_HANDLE_VALUE, LPARAM, LRESULT, LUID, POINT, RECT, SIZE, SYSTEMTIME, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    AlphaBlend, BeginPaint, BitBlt, CombineRgn, CreateBitmap, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateFontW, CreatePen, CreateRectRgn, CreateRoundRectRgn,
    CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, Ellipse, EndPaint, EnumDisplayMonitors,
    ExtCreatePen, FillRect, GdiFlush, GetDC, GetMonitorInfoW, GetStockObject, InvalidateRect,
    LineTo, MonitorFromPoint, MonitorFromRect, MonitorFromWindow, MoveToEx, PolyBezier, ReleaseDC,
    RoundRect, SelectObject, SetBkMode, SetStretchBltMode, SetTextColor, SetWindowRgn, StretchBlt,
    UpdateWindow, BLENDFUNCTION, BS_SOLID, CAPTUREBLT, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    COLORONCOLOR, DEFAULT_CHARSET, DEFAULT_GUI_FONT, DRAW_TEXT_FORMAT, DT_CALCRECT, DT_CENTER,
    DT_END_ELLIPSIS, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, HDC, HGDIOBJ, HMONITOR,
    LOGBRUSH, MONITORINFO, MONITOR_DEFAULTTONEAREST, NULL_BRUSH, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    PS_GEOMETRIC, PS_SOLID, RGN_DIFF, RGN_OR, SRCCOPY, TRANSPARENT,
};
use windows::Win32::Media::Audio::{
    eConsole, eRender, Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator, MMDeviceEnumerator,
};
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    AdjustTokenPrivileges, GetTokenInformation, LookupPrivilegeValueW, TokenElevation, TokenUser,
    LUID_AND_ATTRIBUTES, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
    SE_SHUTDOWN_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_ELEVATION, TOKEN_PRIVILEGES, TOKEN_QUERY,
    TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{
    ReadFile, WriteFile, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Console::{
    AttachConsole, GetStdHandle, SetConsoleCtrlHandler, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE,
};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Power::SetSuspendState;
use windows::Win32::System::Registry::{
    RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows::Win32::System::Search::{
    IAccessor, ICommand, ICommandText, IDBCreateCommand, IDBCreateSession, IDBInitialize,
    IDataInitialize, IRowset, DBACCESSOR_ROWDATA, DBBINDING, DBMEMOWNER_PROVIDEROWNED,
    DBPARAMIO_NOTPARAM, DBPART_STATUS, DBPART_VALUE, DBSTATUS_S_OK, DBTYPE_BYREF, DBTYPE_DATE,
    DBTYPE_I8, DBTYPE_WSTR, HACCESSOR, MSDAINITIALIZE,
};
use windows::Win32::System::Shutdown::{
    ExitWindowsEx, LockWorkStation, EWX_FORCEIFHUNG, EWX_LOGOFF, EWX_REBOOT, EWX_SHUTDOWN,
    SHUTDOWN_REASON,
};
use windows::Win32::System::SystemInformation::{GetLocalTime, GetTickCount, GetTickCount64};
use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT, WM_MOUSELEAVE};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, GetLastInputInfo, SendInput, ToUnicode, TrackMouseEvent, INPUT,
    INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, LASTINPUTINFO,
    TME_LEAVE, TRACKMOUSEEVENT, VIRTUAL_KEY, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DOWN, VK_ESCAPE,
    VK_LBUTTON, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_MENU, VK_RBUTTON, VK_RCONTROL,
    VK_RETURN, VK_RMENU, VK_RSHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, IEnumShellItems, IShellItem, IShellItemImageFactory,
    SHCreateItemFromParsingName, SHGetFileInfoW, SHGetImageList, ShellExecuteW, Shell_NotifyIconW,
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, SHFILEINFOW, SHGFI_FLAGS,
    SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES, SHIL_LARGE, SIGDN_NORMALDISPLAY,
    SIGDN_PARENTRELATIVEPARSING, SIIGBF_ICONONLY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CopyIcon, CreateIconFromResourceEx, CreateIconIndirect, CreatePopupMenu,
    DestroyIcon, DestroyMenu, DrawIconEx, LoadIconW, PostQuitMessage, TrackPopupMenu, DI_NORMAL,
    HICON, ICONINFO, IDI_APPLICATION, LR_DEFAULTCOLOR, MF_STRING, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    WM_LBUTTONDBLCLK,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetAncestor,
    GetDesktopWindow, GetLayeredWindowAttributes, GetMessageW, GetShellWindow, GetWindowRect,
    IsZoomed, RegisterClassW, SetCursorPos, SetLayeredWindowAttributes, SetWindowPos,
    SetWindowsHookExW, ShowWindow, TranslateMessage, UnhookWindowsHookEx, WindowFromPoint, GA_ROOT,
    GA_ROOTOWNER, HC_ACTION, HHOOK, HWND_TOPMOST, KBDLLHOOKSTRUCT, LAYERED_WINDOW_ATTRIBUTES_FLAGS,
    LLKHF_INJECTED, LWA_ALPHA, MSG, MSLLHOOKSTRUCT, SET_WINDOW_POS_FLAGS, SWP_ASYNCWINDOWPOS,
    SWP_NOACTIVATE, SWP_NOSENDCHANGING, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_HIDE,
    SW_RESTORE, SW_SHOWNA, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN,
    WM_SYSKEYUP, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};

// --- tiling additions -----------------------------------------------------
use core::ffi::c_void;
use std::collections::{HashMap, VecDeque};
use windows::core::s;
use windows::Win32::Graphics::Dwm::{
    DwmFlush, DwmGetWindowAttribute, DwmRegisterThumbnail, DwmSetWindowAttribute,
    DwmUnregisterThumbnail, DwmUpdateThumbnailProperties, DWMWA_BORDER_COLOR, DWMWA_CLOAKED,
    DWMWA_EXTENDED_FRAME_BOUNDS, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_VISIBLE,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::Threading::{
    AttachThreadInput, CreateMutexW, GetCurrentProcess, GetCurrentProcessId, GetCurrentThread,
    GetCurrentThreadId, GetGuiResources, OpenMutexW, OpenProcess, OpenProcessToken,
    QueryFullProcessImageNameW, SetThreadInformation, SetThreadPriority, ThreadPowerThrottling,
    WaitForSingleObject, GR_GDIOBJECTS, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, SYNCHRONIZATION_ACCESS_RIGHTS, THREAD_POWER_THROTTLING_CURRENT_VERSION,
    THREAD_POWER_THROTTLING_EXECUTION_SPEED, THREAD_POWER_THROTTLING_STATE, THREAD_PRIORITY,
    THREAD_PRIORITY_ABOVE_NORMAL, THREAD_PRIORITY_HIGHEST,
};
use windows::Win32::UI::Accessibility::SetWinEventHook;
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, GetDpiForWindow, SetProcessDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_SHIFT;
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, ChangeWindowMessageFilterEx, EnumWindows, FindWindowExW, FindWindowW,
    GetClassNameW, GetClientRect, GetCursorPos, GetForegroundWindow, GetSystemMetrics, GetWindow,
    GetWindowLongPtrW, GetWindowLongW, GetWindowPlacement, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsHungAppWindow, IsIconic, IsWindow, IsWindowVisible, KillTimer,
    MessageBoxW, PeekMessageW, PostMessageW, RegisterWindowMessageW, SendMessageTimeoutW,
    SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowLongW, SetWindowPlacement,
    SystemParametersInfoW, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE, EVENT_OBJECT_LOCATIONCHANGE,
    EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_SHOW, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND,
    EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MOVESIZEEND, GWLP_USERDATA, GWL_EXSTYLE, GWL_STYLE,
    GW_OWNER, MB_ICONERROR, MB_OK, MSGFLT_ALLOW, OBJID_CURSOR, PM_REMOVE, PW_RENDERFULLCONTENT,
    SMTO_ABORTIFHUNG, SM_CXSCREEN, SM_CYSCREEN, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE,
    SPI_GETFOREGROUNDLOCKTIMEOUT, SPI_GETWORKAREA, SPI_SETDESKWALLPAPER,
    SPI_SETFOREGROUNDLOCKTIMEOUT, SPI_SETWORKAREA, SW_SHOW, SW_SHOWNORMAL,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOWPLACEMENT, WINDOW_EX_STYLE, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS, WM_CLIPBOARDUPDATE, WM_CLOSE, WM_DISPLAYCHANGE, WM_DPICHANGED,
    WM_ENDSESSION, WM_ERASEBKGND, WM_PAINT, WM_QUERYENDSESSION, WM_SETTINGCHANGE, WM_TIMER,
    WM_USER, WS_CHILD,
};

// =========================================================================
// Diagnostics log
// =========================================================================
// Release builds are `windows_subsystem = "windows"`, so every `println!` below
// writes to a console that does not exist, and almost every Win32 call is
// `let _ = ...`. Without a file log, nothing Astur does — a hook that failed to
// install, a config line it could not parse, a compositor that fell back — is
// visible to anyone, and a bug report can only ever be a video.
//
// Rules:
//   * NEVER log from `mouse_proc` / `keyboard_proc`. These take a lock and
//     allocate; the hooks are on the OS-wide input path (bar-to-hold #2). Hook
//     health travels through atomics and is logged by the watchdog thread.
//   * Every macro early-outs on one relaxed atomic load before formatting, so a
//     `debug!` site costs a load when the level is `error` (the default).
//   * The queue is bounded and drops oldest-first: a stuck disk must never
//     block the manager thread.

const LOG_OFF: u8 = 0;
const LOG_ERROR: u8 = 1;
const LOG_INFO: u8 = 2;
const LOG_DEBUG: u8 = 3;

static LOG_LEVEL: AtomicU8 = AtomicU8::new(LOG_OFF);
static LOGQ: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
static LOGCV: Condvar = Condvar::new();
static LOG_DROPPED: AtomicU64 = AtomicU64::new(0);
static LOG_WORKER: OnceLock<()> = OnceLock::new();
const LOG_QUEUE_MAX: usize = 1024;
/// Rotate at 1 MiB into `astur.log.old`. Two files is the whole retention story.
const LOG_MAX_BYTES: u64 = 1024 * 1024;

fn log_level_from_str(s: &str) -> u8 {
    match s {
        "debug" => LOG_DEBUG,
        "info" => LOG_INFO,
        "error" => LOG_ERROR,
        _ => LOG_OFF,
    }
}

fn log_level_name(level: u8) -> &'static str {
    match level {
        LOG_DEBUG => "debug",
        LOG_INFO => "info",
        LOG_ERROR => "error",
        _ => "off",
    }
}

fn log_path() -> std::path::PathBuf {
    config_path("ASTUR_LOG", "astur.log")
}

#[inline]
fn log_on(level: u8) -> bool {
    LOG_LEVEL.load(Ordering::Relaxed) >= level
}

fn log_stamp() -> String {
    let t = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    )
}

/// Queue one line for the writer thread. Called by the manager, the workers and
/// the main thread — never by a hook.
fn log_push(level: u8, msg: &str) {
    if !log_on(level) {
        return;
    }
    let tag = match level {
        LOG_ERROR => "ERROR",
        LOG_INFO => "INFO ",
        _ => "DEBUG",
    };
    let line = format!("{} {} {}\r\n", log_stamp(), tag, msg);
    {
        let mut q = LOGQ.lock().unwrap_or_else(|p| p.into_inner());
        if q.len() >= LOG_QUEUE_MAX {
            q.pop_front();
            LOG_DROPPED.fetch_add(1, Ordering::Relaxed);
        }
        q.push_back(line);
    }
    // Spawned on the first line that is actually kept, so `log_level = off`
    // costs no thread at all.
    LOG_WORKER.get_or_init(|| {
        spawn_named("log", log_worker);
    });
    LOGCV.notify_one();
}

/// Write one line straight to the log file, bypassing the queue. For the panic
/// hook only: `panic = "abort"` means the worker thread never runs again.
fn log_sync(msg: &str) {
    use std::io::Write;
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = write!(f, "{} ERROR {}\r\n", log_stamp(), msg);
        let _ = f.flush();
    }
}

/// Sole writer. Blocks on the condvar; batches whatever accumulated.
fn log_worker() {
    use std::io::Write;
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    loop {
        let batch: Vec<String> = {
            let mut q = LOGQ.lock().unwrap_or_else(|p| p.into_inner());
            while q.is_empty() {
                q = LOGCV.wait(q).unwrap_or_else(|p| p.into_inner());
            }
            q.drain(..).collect()
        };
        let dropped = LOG_DROPPED.swap(0, Ordering::Relaxed);
        if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > LOG_MAX_BYTES {
            let _ = std::fs::rename(&path, path.with_extension("log.old"));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            if dropped > 0 {
                let _ = write!(
                    f,
                    "{} ERROR log queue full: {dropped} lines dropped\r\n",
                    log_stamp()
                );
            }
            for line in batch {
                let _ = f.write_all(line.as_bytes());
            }
        }
    }
}

macro_rules! log_error {
    ($($arg:tt)*) => {
        if log_on(LOG_ERROR) { log_push(LOG_ERROR, &format!($($arg)*)) }
    };
}
macro_rules! log_info {
    ($($arg:tt)*) => {
        if log_on(LOG_INFO) { log_push(LOG_INFO, &format!($($arg)*)) }
    };
}
macro_rules! log_debug {
    ($($arg:tt)*) => {
        if log_on(LOG_DEBUG) { log_push(LOG_DEBUG, &format!($($arg)*)) }
    };
}

// =========================================================================
// Latency probes and counters
// =========================================================================
// "Faster" is a claim until it has a number. These are the numbers: stage
// timers on the switch / glide / retile / manager paths, a SetWindowPos count,
// WinEvent intake counters and the LL-hook delivery delay.
//
// Rules:
//   * Stage timers exist only at log_level = debug. `Probe::start` returns an
//     inert probe otherwise, so a disabled probe costs one relaxed load and a
//     branch per mark: no clock read, no formatting, no allocation.
//   * One log line per operation, through the normal async queue, so a probe
//     never adds disk I/O to the path it measures.
//   * Never a Probe on `mouse_proc` / `keyboard_proc`. The hooks only do a
//     relaxed fetch_max on HOOK_DELAY_MAX, and only at debug level.

/// Stage stopwatch for one operation. Each `mark` records microseconds since
/// `start`; the line is logged when the probe drops, so an early return still
/// reports how far it got.
struct Probe(Option<(Instant, String)>);

impl Probe {
    fn start(name: &str) -> Probe {
        Probe(log_on(LOG_DEBUG).then(|| (Instant::now(), name.to_string())))
    }

    fn on(&self) -> bool {
        self.0.is_some()
    }

    fn mark(&mut self, stage: &str) {
        if let Some((t0, line)) = &mut self.0 {
            use std::fmt::Write;
            let _ = write!(line, " {stage}=+{}us", t0.elapsed().as_micros());
        }
    }

    /// Free-form context (`format_args!` so nothing is formatted when off).
    fn note(&mut self, text: std::fmt::Arguments) {
        if let Some((_, line)) = &mut self.0 {
            use std::fmt::Write;
            let _ = write!(line, " {text}");
        }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        if let Some((t0, line)) = self.0.take() {
            log_push(
                LOG_DEBUG,
                &format!("probe {line} total={}us", t0.elapsed().as_micros()),
            );
        }
    }
}

/// A timestamp only when probes are on (for hand-offs like dispatch -> pickup).
fn probe_now() -> Option<Instant> {
    log_on(LOG_DEBUG).then(Instant::now)
}

/// Upper median and maximum of per-frame timings, for the compositor probes.
/// Sorts in place; (0, 0) when there were no frames.
fn p50_max(v: &mut [u32]) -> (u32, u32) {
    if v.is_empty() {
        return (0, 0);
    }
    v.sort_unstable();
    (v[v.len() / 2], v[v.len() - 1])
}

/// Every SetWindowPos Astur issues on a real window (set_pos_raw, commit_rect,
/// the drag park). Diagnostics, and the per-Cmd probe line reports the delta.
static SWP_CALLS: AtomicU64 = AtomicU64::new(0);

/// WinEvent intake counters, bumped on the main thread by `win_event_proc`
/// (and the manager, for Add). Relaxed adds; totals since start.
const EVC_SHOW: usize = 0;
const EVC_HIDE: usize = 1;
const EVC_DESTROY: usize = 2;
const EVC_FOREGROUND: usize = 3;
const EVC_NAMECHANGE: usize = 4;
/// Every LOCATIONCHANGE the hook delivers, counted BEFORE the object filter.
const EVC_LOCATION: usize = 5;
/// The subset of those that are the cursor (id_object == OBJID_CURSOR, -9).
const EVC_LOCATION_CURSOR: usize = 6;
/// Cmd::Add commands the manager processed.
const EVC_ADD: usize = 7;
/// SHOWs the style prefilter dropped (child / tool / no-activate windows).
const EVC_SHOW_STYLE: usize = 8;
/// NAMECHANGE refreshes folded into one already queued (BAR-10).
const EVC_BAR_REFRESH_FOLDED: usize = 9;
const EVC_NAMES: [&str; 10] = [
    "show",
    "hide",
    "destroy",
    "foreground",
    "namechange",
    "location",
    "location_cursor",
    "add",
    "show_style_skipped",
    "bar_refresh_folded",
];
static EV_COUNTS: [AtomicU64; 10] = [const { AtomicU64::new(0) }; 10];

#[inline]
fn ev_count(slot: usize) {
    EV_COUNTS[slot].fetch_add(1, Ordering::Relaxed);
}

/// Worst LL-hook delivery delay (ms) since the watchdog last read it: the OS
/// input timestamp against GetTickCount on arrival. Written by the hooks with a
/// relaxed fetch_max, only at debug level.
static HOOK_DELAY_MAX: AtomicU32 = AtomicU32::new(0);

/// Hook side of HOOK_DELAY_MAX. Hook-legal: one tick read and one relaxed
/// atomic, no lock, no allocation.
#[inline]
fn hook_delay_note(event_time: u32) {
    // Signed: an event stamped a tick after our read must not wrap to 49 days.
    let late = unsafe { GetTickCount() }.wrapping_sub(event_time) as i32;
    if late > 0 {
        HOOK_DELAY_MAX.fetch_max(late as u32, Ordering::Relaxed);
    }
}

/// One line of counters, for diagnostics and the watchdog's debug tick.
fn counters_line() -> String {
    use std::fmt::Write;
    let mut s = String::new();
    for (name, n) in EVC_NAMES.iter().zip(EV_COUNTS.iter()) {
        let _ = write!(s, "{name}={} ", n.load(Ordering::Relaxed));
    }
    let gdi = unsafe { GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS) };
    // A flag stuck at true is a bar title frozen for good with no error
    // anywhere (BAR-10), so it is shown next to the queue it gates.
    let _ = write!(
        s,
        "swp={} gdi={gdi} cmdq={} bar_refresh_queued={} focus_refused={} focus_missed={} winevent_failed={:#x} {}",
        SWP_CALLS.load(Ordering::Relaxed),
        CMDQ.lock().unwrap().len(),
        BAR_REFRESH_QUEUED.load(Ordering::Relaxed),
        FOCUS_REFUSED.load(Ordering::Relaxed),
        FOCUS_MISSED.load(Ordering::Relaxed),
        WINEVENT_HOOKS_FAILED.load(Ordering::Relaxed),
        wp_counters()
    );
    s
}

// =========================================================================
// Thread scheduling
// =========================================================================
// Every Astur window is WS_EX_NOACTIVATE, so the process is never in focus:
// Windows gives it at best Medium QoS (Low when no bar is visible), and every
// thread runs at NORMAL. Under load a hook callback, a command wake-up or an
// animation frame then queues behind the user's own busy threads, and on a
// hybrid CPU on battery it can land on an efficiency core (EVENTS-12). So the
// threads a user waits on are raised a little and opted into HighQoS. Not
// measured here; the expected gain is ~0 on an idle desktop on AC.
//
// Hard rules (each one is a way to break the hooks or starve the desktop):
//   * Nothing below THREAD_PRIORITY_NORMAL. The icon, filesearch, stats,
//     wallpaper, state, mru, config-watcher, ipc and capture threads all hold
//     std Mutexes (SRW locks: no priority inheritance) that the main, manager
//     and launcher threads also take. A starved holder can keep the main
//     thread blocked past LowLevelHooksTimeout, and Windows then removes the
//     hooks without a word.
//   * Manager >= compositors, so a compositor can never outrun the placement
//     it is covering and reveal it early.
//   * The main thread (hooks, WinEvents, bars, tray, marker) at most HIGHEST,
//     never TIME_CRITICAL: bar GDI paints would then preempt the apps.
//   * No MMCSS: its DisplayPostProcessing class is scheduled above DWM and
//     audio. Plain SetThreadPriority in the normal class tops out at 15,
//     below MMCSS's 16+, so it cannot starve either.
//   * No process-wide IGNORE_TIMER_RESOLUTION and no priority-class change:
//     std::thread::sleep already uses a high-resolution waitable timer, and
//     whether process-level power state reaches the apps Astur launches is
//     undocumented. Thread-level settings sidestep that question.
//   * None of these threads busy-waits: each blocks on a condvar or a message
//     loop when idle, so raising them costs nothing at rest.

/// The threads Astur raises: the ones between an input and what it causes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThreadRole {
    /// LL hooks, WinEvents, bars, tray and marker (the process main thread).
    Main,
    /// Owns all window state and runs every command.
    Manager,
    /// glide_worker and transition_worker: the cosmetic overlays.
    Compositor,
    /// The Alt+Space picker: keystroke to repaint.
    Launcher,
}

fn role_priority(role: ThreadRole) -> THREAD_PRIORITY {
    match role {
        ThreadRole::Manager => THREAD_PRIORITY_HIGHEST,
        ThreadRole::Main | ThreadRole::Compositor | ThreadRole::Launcher => {
            THREAD_PRIORITY_ABOVE_NORMAL
        }
    }
}

/// Raise the calling thread for `role` and opt it into HighQoS. Called first
/// thing in each such thread: a new thread starts at NORMAL whatever its
/// parent runs at. A refusal (HighQoS needs Windows 10 1709+) is logged and
/// the thread carries on at whatever it got.
fn raise_current_thread(role: ThreadRole) {
    unsafe {
        if let Err(e) = SetThreadPriority(GetCurrentThread(), role_priority(role)) {
            log_info!("{role:?} thread: priority unchanged ({e})");
        }
        // ControlMask EXECUTION_SPEED with StateMask 0 = "never throttle this
        // thread's execution speed", i.e. explicit HighQoS.
        let state = THREAD_POWER_THROTTLING_STATE {
            Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: 0,
        };
        if let Err(e) = SetThreadInformation(
            GetCurrentThread(),
            ThreadPowerThrottling,
            &state as *const _ as *const c_void,
            core::mem::size_of::<THREAD_POWER_THROTTLING_STATE>() as u32,
        ) {
            log_info!("{role:?} thread: HighQoS not set ({e})");
        }
    }
}

/// std::thread::spawn with a name (std sets it through SetThreadDescription,
/// resolved at run time), so WPA, a debugger and a crash dump can tell the
/// threads apart. Fails exactly like std::thread::spawn: a panic.
fn spawn_named<F>(name: &str, f: F)
where
    F: FnOnce() + Send + 'static,
{
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(f)
        .expect("failed to spawn thread");
}

// =========================================================================
// DPI
// =========================================================================
// Astur declares per-monitor-v2 awareness in `main()`, so every rect it reads
// from Win32 and every rect it hands back is in PHYSICAL pixels on the monitor
// concerned. Before that (<= 2.1.2) Windows virtualised the whole desktop to
// 96 DPI and tiles landed in the top-left 1/scale of a scaled screen — GitHub
// issue #5.
//
// The consequence is that every pixel in the config is a LOGICAL pixel at 100%
// and has to be scaled by the DPI of the monitor the chrome is drawn on before
// it becomes a real pixel. Tiling geometry needs no scaling at all: the work
// area already arrives in physical pixels.
//
// The scaling is applied inside the `bar_*` / `la_*` accessor functions rather
// than at 60-odd call sites, so a call site cannot forget. Each accessor reads
// one atomic holding the DPI of the surface currently being drawn:
//   * `BAR_PAINT_DPI` — set at the top of `paint_bar` from that bar's own
//     window DPI. All bar painting is on the main thread, one bar at a time.
//   * `UI_DPI` — set when the launcher or the system menu places itself. Both
//     are single popups that live on one monitor at a time.

const DPI_BASE: u32 = 96;

/// DPI of the bar currently being painted (96 until the first paint).
static BAR_PAINT_DPI: AtomicU32 = AtomicU32::new(DPI_BASE);
/// DPI of the monitor the launcher / system menu was last placed on.
static UI_DPI: AtomicU32 = AtomicU32::new(DPI_BASE);

/// Scale a configured (logical, 100%) pixel value to physical pixels.
#[inline]
fn dpi_px(px: i32, dpi: u32) -> i32 {
    if dpi == DPI_BASE {
        return px;
    }
    ((px as i64 * dpi as i64) / DPI_BASE as i64) as i32
}

#[inline]
fn bar_dpi() -> u32 {
    BAR_PAINT_DPI.load(Ordering::Relaxed)
}

#[inline]
fn ui_dpi() -> u32 {
    UI_DPI.load(Ordering::Relaxed)
}

/// Effective DPI of one monitor (96 = 100%). Per-monitor, so a mixed-DPI desk
/// is handled correctly. Falls back to 96 when the query fails.
unsafe fn monitor_dpi(hmon: isize) -> u32 {
    let (mut x, mut y) = (DPI_BASE, DPI_BASE);
    if GetDpiForMonitor(
        HMONITOR(hmon as *mut c_void),
        MDT_EFFECTIVE_DPI,
        &mut x,
        &mut y,
    )
    .is_err()
    {
        return DPI_BASE;
    }
    x.max(DPI_BASE)
}

/// DPI of the monitor a window is on, via the window itself (correct even
/// mid-move between two monitors of different scale).
unsafe fn window_dpi(h: HWND) -> u32 {
    let d = GetDpiForWindow(h);
    if d == 0 {
        DPI_BASE
    } else {
        d.max(DPI_BASE)
    }
}

/// DPI of the monitor under a screen point.
unsafe fn dpi_at(pt: POINT) -> u32 {
    monitor_dpi(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST).0 as isize)
}

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
/// DPI of the monitor the current drag started on. Sampled once at button-down
/// (never per WM_MOUSEMOVE — the hook is on the OS-wide input path), so the
/// bracket and the drag outline are the same physical size at every scale.
static DRAG_DPI: AtomicU32 = AtomicU32::new(DPI_BASE);
#[inline]
fn drag_dpi() -> u32 {
    DRAG_DPI.load(Ordering::Relaxed)
}
// Top corners sit on the very top edge; lift the bracket up slightly so it reads
// as hugging the corner instead of sitting inside the title bar.
const MARK_TOP_LIFT: i32 = 8;
// Window class for the transient workspace-slide overlay.
const SLIDE_CLASS: PCWSTR = w!("astur_slide");

/// Longest configured animation_ms whose slide / glide overlays let input
/// through (SWITCH-14).
const CLICK_THROUGH_MAX_MS: i32 = 250;

/// Extended style for the slide and glide overlays. Up to
/// CLICK_THROUGH_MAX_MS they are layered + transparent, so clicks, the wheel
/// and Alt+drag go straight to the real windows, which are already at their
/// final place underneath; before, the overlay ate every work-area click for
/// the whole animation (140-245 ms at the defaults), and an Alt+drag started
/// in that window grabbed the overlay itself (WindowFromPoint skips only
/// layered + transparent windows). The trade-off, accepted: during the
/// animation a click acts on the final layout while old or in-between frames
/// still show. At the default 140 ms that is under reaction time, like the
/// animations-off path; the config allows up to 2000 ms, where a misdirected
/// click would be noticeable, so longer animations keep blocking input.
fn overlay_ex_style(animation_ms: i32) -> WINDOW_EX_STYLE {
    let base = WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE;
    if animation_ms <= CLICK_THROUGH_MAX_MS {
        base | WS_EX_LAYERED | WS_EX_TRANSPARENT
    } else {
        base
    }
}

/// A layered overlay shows nothing until it has attributes, so a failed SLWA
/// would leave the switch running uncovered with frame 0 never on screen.
/// Makes it opaque (alpha 255; never UpdateLayeredWindow, which fails after
/// SLWA and would break the GetDC blits). false = drop this overlay: the
/// caller treats it like a failed CreateWindowExW. Logged once.
unsafe fn overlay_make_visible(overlay: HWND, ex_style: WINDOW_EX_STYLE) -> bool {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if !ex_style.contains(WS_EX_LAYERED) {
        return true;
    }
    if SetLayeredWindowAttributes(overlay, COLORREF(0), 255, LWA_ALPHA).is_ok() {
        return true;
    }
    if !LOGGED.swap(true, Ordering::Relaxed) {
        log_error!("overlay SetLayeredWindowAttributes failed; that switch or glide ran uncovered");
    }
    false
}

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
    let t = dpi_px(OUTLINE_THICK, drag_dpi()).max(1);
    let region = CreateRectRgn(0, 0, w, h);
    if w > 2 * t && h > 2 * t {
        let inner = CreateRectRgn(t, t, w - t, h - t);
        CombineRgn(region, region, inner, RGN_DIFF);
        let _ = DeleteObject(HGDIOBJ(inner.0));
    }
    // The window takes ownership of `region`; the system frees the previous one.
    SetWindowRgn(hwnd, region, BOOL(1));
    let _ = SetWindowPos(
        hwnd,
        HWND_TOPMOST,
        x,
        y,
        w,
        h,
        SWP_NOACTIVATE | SWP_SHOWWINDOW,
    );
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
// and puts it back on release (Cmd::DragMoved/DragResized: the final rect, or for a
// tiled resize drop placed instantly an un-park plus its tile) — the hook
// itself never does a cross-process SetWindowPos. Thumbnails preserve the source
// aspect ratio, so a resize letterboxes while the aspect changes (accepted for live
// content); registration failure falls back to the outline (and no park).
static THUMB_HWND: AtomicIsize = AtomicIsize::new(0); // overlay DWM renders into
static THUMB_ID: AtomicIsize = AtomicIsize::new(0); // HTHUMBNAIL (0 = none active)
static DRAG_THUMB: AtomicBool = AtomicBool::new(false); // this drag uses the thumbnail

unsafe fn thumb_props(id: isize, w: i32, h: i32) {
    let props = DWM_THUMBNAIL_PROPERTIES {
        dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY,
        rcDestination: RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        },
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
    let _ = SetWindowPos(
        hwnd_from(ov),
        HWND_TOPMOST,
        x,
        y,
        w,
        h,
        SWP_NOACTIVATE | SWP_SHOWWINDOW,
    );
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

/// Commit a previewed rect to the real window in one SetWindowPos (posted, not
/// waited on, with async placement: see `foreign_swp_flags`). Runs on the
/// MANAGER thread (DragMoved/DragResized/DragUnmaximize handlers), never on a
/// hook. Handles floating windows (which keep this dropped rect) and tiled ones
/// (which retile over it) alike.
unsafe fn commit_rect(hwnd: isize, x: i32, y: i32, w: i32, h: i32) {
    SWP_CALLS.fetch_add(1, Ordering::Relaxed);
    let _ = SetWindowPos(
        hwnd_from(hwnd),
        None,
        x,
        y,
        w,
        h,
        foreign_swp_flags(
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSENDCHANGING,
            ASYNC_WINDOW_POS.load(Ordering::Relaxed),
        ),
    );
}

/// Post, rather than wait for, every SetWindowPos on an app's window (config
/// `async_window_pos`, default on; TILE-1). A synchronous cross-process
/// SetWindowPos returns only after that app's thread has handled the move
/// (WM_NCCALCSIZE, WM_WINDOWPOSCHANGED, its WM_SIZE relayout), so a retile
/// cost the SUM of every changed app's handling, serially on the manager
/// thread, and one hung app stalled it without limit. Posted, each app handles
/// its own request in parallel: the layout is done after the slowest app, and
/// the manager is free for the next command at once.
///
/// This does NOT make the manager hang-proof. These stay synchronous and can
/// still block on a hung app: switch_plain's ShowWindow hide/show, the
/// refresh_monitors re-show, Cmd::Add's hide, the scratchpad show/hide,
/// SW_RESTORE, BringWindowToTop and SetWindowLongW. Only commands that just
/// place windows stop waiting.
///
/// Consequences handled elsewhere, because a SetWindowPos no longer means the
/// window is there yet: the cursor warp centres on the requested tile, not the
/// live rect (`center_cursor_on`); a drop waits (bounded) for its own commit
/// to land before a glide captures the screen (`drop_retile_force_instant`);
/// the border correction rejects a read that straddles a landing
/// (`adjust_for_border`). Set by apply_hook_config; a reload that turns it off
/// while a posted move is still queued can land that older rect after a newer
/// synchronous one, once, until the next retile.
static ASYNC_WINDOW_POS: AtomicBool = AtomicBool::new(true);

/// Flags for a SetWindowPos on an app's (foreign) window: `base`, plus
/// SWP_ASYNCWINDOWPOS when async placement is on. Every such call goes through
/// here (set_pos_raw, commit_rect, the drag park and un-park) so the requests
/// one window gets are all posted, FIFO in its queue, or all synchronous:
/// mixing the two could land an older rect after a newer one. ShowWindow and
/// SetWindowPlacement stay synchronous; each is followed, in the same call, by
/// a posted SetWindowPos, so the newest rect still lands last.
fn foreign_swp_flags(base: SET_WINDOW_POS_FLAGS, async_on: bool) -> SET_WINDOW_POS_FLAGS {
    if async_on {
        base | SWP_ASYNCWINDOWPOS
    } else {
        base
    }
}

/// What an Alt-resize drop does before its retile (INPUT-5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResizeDrop {
    /// Land the preview rect first, as every drop used to: untiled windows
    /// keep it, and a glide captures the window there.
    Commit,
    /// Tiled, instant, parked: move back to where the park took it from,
    /// position only, and let the retile size it once.
    UnparkOrigin,
    /// Tiled, instant, never parked (the outline preview): the retile's one
    /// SetWindowPos is all it needs.
    NoUnpark,
}

/// `tiled`: the retile will place the window (see `tile_target`), and a
/// parked one has a recorded origin. `will_glide`: the glide can run now.
fn resize_drop_plan(tiled: bool, parked: bool, will_glide: bool) -> ResizeDrop {
    if !tiled || will_glide {
        ResizeDrop::Commit
    } else if parked {
        ResizeDrop::UnparkOrigin
    } else {
        ResizeDrop::NoUnpark
    }
}

/// Move a parked window back to `origin` without resizing it: a pure move
/// costs the app no relayout, and it brings the window back on the monitor
/// (and DPI) it was parked from, so the retile's resize is a same-monitor one.
unsafe fn unpark_to(h: isize, origin: POINT) {
    SWP_CALLS.fetch_add(1, Ordering::Relaxed);
    let _ = SetWindowPos(
        hwnd_from(h),
        None,
        origin.x,
        origin.y,
        0,
        0,
        foreign_swp_flags(
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSENDCHANGING,
            ASYNC_WINDOW_POS.load(Ordering::Relaxed),
        ),
    );
}

/// A screen rect as WINDOWPLACEMENT's rcNormalPosition. For a top-level window
/// without WS_EX_TOOLWINDOW that is workspace coordinates: screen coordinates
/// shifted by where the work area starts inside its monitor (a top or left
/// taskbar), `work_off` = rcWork origin minus rcMonitor origin. NOT minus the
/// work area's own screen origin: on a secondary monitor that would restore the
/// window onto the primary one. Tool windows use screen coordinates as is.
fn placement_to_workspace(r: RECT, work_off: (i32, i32), toolwindow: bool) -> RECT {
    if toolwindow {
        return r;
    }
    RECT {
        left: r.left - work_off.0,
        top: r.top - work_off.1,
        right: r.right - work_off.0,
        bottom: r.bottom - work_off.1,
    }
}

/// Un-maximize straight to `r` (screen coordinates) in one synchronous
/// SetWindowPlacement: one app relayout, to the right size. Only showCmd and
/// the normal rect change; flags and the min/max positions are the window's
/// own. SW_SHOWNORMAL activates like the SW_RESTORE it replaces (a floating
/// drop never gets a focus_window of its own). False when it did not take, or
/// the window is somehow still maximised: the caller falls back to SW_RESTORE.
unsafe fn unmaximize_to(hwnd: HWND, r: RECT) -> bool {
    let mut wp = WINDOWPLACEMENT {
        length: core::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    if GetWindowPlacement(hwnd, &mut wp).is_err() {
        return false;
    }
    let mut mi = MONITORINFO {
        cbSize: core::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !GetMonitorInfoW(MonitorFromRect(&r, MONITOR_DEFAULTTONEAREST), &mut mi).as_bool() {
        return false;
    }
    let work_off = (
        mi.rcWork.left - mi.rcMonitor.left,
        mi.rcWork.top - mi.rcMonitor.top,
    );
    let tool = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0;
    wp.rcNormalPosition = placement_to_workspace(r, work_off, tool);
    wp.showCmd = SW_SHOWNORMAL.0 as u32;
    SetWindowPlacement(hwnd, &wp).is_ok() && !IsZoomed(hwnd).as_bool()
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

/// The incoming image's opacity at fade progress `t`: exactly 0 (the outgoing
/// frame, what frame 0 shows) for t <= 0, exactly 255 (the incoming snapshot,
/// pixel-aligned with the real windows underneath) for t >= 1.
#[inline]
fn fade_alpha(t: f64) -> u8 {
    (255.0 * ease_out_cubic(t.clamp(0.0, 1.0))).round() as u8
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
/// and the workspace-slide reveal). Posted when async placement is on.
unsafe fn set_pos_raw(h: isize, r: RECT) {
    SWP_CALLS.fetch_add(1, Ordering::Relaxed);
    let _ = SetWindowPos(
        hwnd_from(h),
        None,
        r.left,
        r.top,
        r.right - r.left,
        r.bottom - r.top,
        foreign_swp_flags(
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSENDCHANGING,
            ASYNC_WINDOW_POS.load(Ordering::Relaxed),
        ),
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
    if msg == WM_DISPLAYCHANGE || msg == WM_DPICHANGED {
        // Do NOTHING synchronously here. Resolution, monitor add/remove and
        // scale changes all arrive as these two messages, often several in a
        // row, and the rebuild moves windows — which delivers more of them.
        // See `request_bar_rebuild`. The wallpaper crops are the wrong size
        // from this moment, not from when the manager gets to RefreshMonitors:
        // `wp_invalidate` is an atomic bump plus a condvar poke, nothing that
        // moves a window or sends a message.
        wp_invalidate();
        request_bar_rebuild(true);
        return LRESULT(0);
    } else if msg == WM_SETTINGCHANGE && w.0 == SPI_SETDESKWALLPAPER.0 as usize {
        // The wallpaper changed (us, Settings, a slideshow app): every cached
        // crop now shows the old one. Falls through to DefWindowProc.
        wp_invalidate();
    } else if msg == WM_SETTINGCHANGE && w.0 == SPI_SETWORKAREA.0 as usize {
        // A work area moved with no display change of its own: the taskbar
        // Explorer puts on a newly attached monitor can land after the
        // WM_DISPLAYCHANGE refresh, and auto-hide or an appbar changes it too.
        // Work areas are read only by enumerate_monitors, so without this the
        // tiles sit under that taskbar until the next display change
        // (TILE-11). Same coalesced request, nothing synchronous here (see
        // `request_bar_rebuild`), but only the work areas are re-read unless a
        // display change is folded into it. Falls through to DefWindowProc.
        request_bar_rebuild(false);
    } else if msg != 0 && msg == TASKBAR_CREATED_MSG.load(Ordering::Relaxed) {
        // Explorer (re)started: the wallpaper window is a new one.
        wp_explorer_restarted();
    } else if msg == WM_REBUILD_BARS {
        // The deferred rebuild, running on a clean stack.
        BARS_REBUILD_PENDING.store(false, Ordering::Relaxed);
        if !REBUILD_DISPLAY.swap(false, Ordering::Relaxed) {
            // Work area only (SPI_SETWORKAREA). Bars sit on the monitor rect,
            // not the work area, so nothing here moves; the manager re-reads
            // the work areas and does nothing if none changed.
            push_cmd(Cmd::RefreshWorkAreas);
            return LRESULT(0);
        }
        seed_fullscreen_windows();
        // Snapshots and per-DPI fonts were sized for the old scale.
        bar_fonts_clear();
        bar_icons_retire();
        bar_icons_sweep();
        ensure_bars();
        push_cmd(Cmd::RefreshMonitors);
        return LRESULT(0);
    } else if msg == WM_RELOAD {
        // Config changed: drop the per-DPI fonts and rebuild bars (must happen
        // on this thread so it can't race a paint; they are rebuilt lazily on
        // the next paint, one per monitor DPI). Bar icons are NOT cleared (a
        // reload used to re-extract every one, LAUNCH-13): only a size no bar
        // uses now is retired, and freed once no snapshot can hold it.
        bar_fonts_clear();
        bar_icons_retire();
        bar_icons_sweep();
        if BAR_HEIGHT.load(Ordering::Relaxed) > 0 {
            ensure_bars();
        } else {
            for b in BARS.lock().unwrap().iter() {
                let _ = ShowWindow(hwnd_from(b.hwnd), SW_HIDE);
            }
        }
    } else if msg == WM_REARM_HOOKS {
        // Watchdog says the OS dropped our hooks. Re-install on this thread —
        // low-level hooks belong to the thread that pumps their messages.
        let hinst = HINSTANCE(
            GetModuleHandleW(None)
                .map(|m| m.0)
                .unwrap_or(core::ptr::null_mut()),
        );
        if install_hooks(hinst) {
            let n = HOOK_REARMS.fetch_add(1, Ordering::Relaxed) + 1;
            // First per watchdog episode at ERROR, then debug (see hook_watchdog).
            if !REARM_LOGGED.swap(true, Ordering::Relaxed) {
                log_error!("input hooks re-armed (re-arm #{n})");
            } else {
                log_debug!("input hooks re-armed (re-arm #{n})");
            }
        } else {
            log_error!("input hooks re-arm FAILED; Astur is deaf until restart");
        }
    } else if msg == WM_BAR_MODE_CHANGED {
        // A maximized/fullscreen app entered or left one monitor. Rebuild only
        // bar runtime geometry; manager/window layout must not disturb the app.
        ensure_bars();
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
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
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
    hook_alive_stamp();
    if code == HC_ACTION as i32 {
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        if log_on(LOG_DEBUG) {
            hook_delay_note(kb.time);
        }
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
                                if ALT_SWITCHER_MODE.load(Ordering::Relaxed) {
                                    post(if vk_down(VK_SHIFT) { LA_UP } else { LA_DOWN }, 0);
                                } else {
                                    post(LA_TAB, 0); // toggle the wide column view
                                }
                            } else if vk == 0x74 {
                                // VK_F5
                                post(LA_REFRESH, 0);
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
                    if ALT_SWITCHER_MODE.swap(false, Ordering::Relaxed) {
                        let h = LAUNCHER_HWND.load(Ordering::Relaxed);
                        if h != 0 {
                            let _ = PostMessageW(
                                hwnd_from(h),
                                WM_LAUNCHER,
                                WPARAM(LA_ACTIVATE),
                                LPARAM(0),
                            );
                        }
                    }
                    // Release the synthetic Alt so the system task switcher commits.
                    if FAKE_ALT.swap(false, Ordering::Relaxed) {
                        inject_key(VK_MENU, true);
                    }
                }
                return LRESULT(1); // never let apps see Left Alt
            }

            // Alt+Tab (and Alt+Shift+Tab): drive the switcher with injected keys
            // and swallow the physical Tab so it isn't counted twice.
            if kb.vkCode == VK_TAB.0 as u32 && ALT_DOWN.load(Ordering::Relaxed) {
                if ALT_TAB_REPLACE.load(Ordering::Relaxed) {
                    if down && !LAUNCHER_OPEN.swap(true, Ordering::Relaxed) {
                        ALT_SWITCHER_MODE.store(true, Ordering::Relaxed);
                        let h = LAUNCHER_HWND.load(Ordering::Relaxed);
                        if h != 0 {
                            let _ = PostMessageW(
                                hwnd_from(h),
                                WM_LAUNCHER,
                                WPARAM(LA_OPEN_SWITCHER),
                                LPARAM(0),
                            );
                        } else {
                            LAUNCHER_OPEN.store(false, Ordering::Relaxed);
                            ALT_SWITCHER_MODE.store(false, Ordering::Relaxed);
                        }
                    }
                } else if down {
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
                && SYSMENU_ENABLED.load(Ordering::Relaxed)
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
                && LAUNCHER_ENABLED.load(Ordering::Relaxed)
                && !LAUNCHER_OPEN.load(Ordering::Relaxed)
                && !SYSMENU_OPEN.load(Ordering::Relaxed)
            {
                LAUNCHER_OPEN.store(true, Ordering::Relaxed);
                let hl = LAUNCHER_HWND.load(Ordering::Relaxed);
                if hl != 0 {
                    let _ = PostMessageW(hwnd_from(hl), WM_LAUNCHER, WPARAM(LA_OPEN), LPARAM(0));
                }
                return LRESULT(1);
            }

            // Tiling hotkeys: Alt + key. Swallowed from apps (Alt is reserved).
            if down && ALT_DOWN.load(Ordering::Relaxed) {
                let shift = vk_down(VK_SHIFT);
                if let Some(cmd) = resolve_hotkey(kb.vkCode, shift, vk_down(VK_CONTROL)) {
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
    let s = dpi_px(MARK_LEN, drag_dpi());
    let t = dpi_px(MARK_THICK, drag_dpi()).max(1);
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
    let len = dpi_px(MARK_LEN, drag_dpi());
    let x = if left { corner_x } else { corner_x - len };
    let y = if top {
        corner_y - dpi_px(MARK_TOP_LIFT, drag_dpi())
    } else {
        corner_y - len
    };
    let _ = SetWindowPos(
        hwnd_from(raw),
        HWND_TOPMOST,
        x,
        y,
        len,
        len,
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
        return mi.rcWork;
    }
    // Fallback: the real primary work area. A hardcoded 1920x1080 was tolerable
    // while the process was DPI-unaware and every desktop looked like a 96-DPI
    // one; now that rects are physical it would be actively wrong on a 4K or
    // scaled screen.
    let mut wa = RECT::default();
    if SystemParametersInfoW(
        SPI_GETWORKAREA,
        0,
        Some(&mut wa as *mut RECT as *mut c_void),
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    )
    .is_ok()
        && wa.right > wa.left
        && wa.bottom > wa.top
    {
        return wa;
    }
    RECT {
        left: 0,
        top: 0,
        right: GetSystemMetrics(SM_CXSCREEN).max(640),
        bottom: GetSystemMetrics(SM_CYSCREEN).max(480),
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Proof of life for the watchdog. One relaxed store; hook-legal.
    hook_alive_stamp();
    if code != HC_ACTION as i32 {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
    if log_on(LOG_DEBUG) {
        hook_delay_note(info.time);
    }
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
                // The signed delta itself, not just its sign: the bar turns
                // partial notches into whole steps. Still one post, no lock.
                let delta = ((info.mouseData >> 16) as u16 as i16) as isize;
                let _ = PostMessageW(
                    hwnd_from(hb),
                    WM_BAR_WHEEL,
                    WPARAM(delta as usize),
                    LPARAM(pt.x as isize),
                );
                return suppress;
            }
        }
    }

    match msg {
        WM_LBUTTONDOWN if left_alt_down() && !drag_active() => {
            // Sample the scale once per drag; the overlay sizes below read it.
            DRAG_DPI.store(dpi_at(pt), Ordering::Relaxed);
            if let Some(hwnd) = root_window_at(pt) {
                let mut rect = RECT::default();
                if IsZoomed(hwnd).as_bool() {
                    // Un-maximize + place is the MANAGER's job. ShowWindow on a
                    // foreign window drives that process's message loop, and a
                    // busy app would blow the LowLevelHooksTimeout — at which
                    // point Windows silently unhooks Astur and every hotkey,
                    // the launcher and Alt-drag die with no error (review B-02;
                    // this had regressed after the 2026-07-10 clean-up).
                    // The hook only predicts the rect (pure arithmetic) and
                    // seeds the drag from it, so the preview follows the cursor
                    // from the first WM_MOUSEMOVE.
                    let work = work_area_at(pt);
                    let w = ((work.right - work.left) * RESTORE_NUM / RESTORE_DEN).max(MIN_W);
                    let h = ((work.bottom - work.top) * RESTORE_NUM / RESTORE_DEN).max(MIN_H);
                    let mut x = pt.x - w / 2;
                    let mut y = pt.y - h / 2;
                    x = x.clamp(work.left, (work.right - w).max(work.left));
                    y = y.clamp(work.top, (work.bottom - h).max(work.top));
                    push_cmd(Cmd::DragUnmaximize(
                        hwnd.0 as isize,
                        RECT {
                            left: x,
                            top: y,
                            right: x + w,
                            bottom: y + h,
                        },
                    ));
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
            DRAG_DPI.store(dpi_at(pt), Ordering::Relaxed);
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
        // ANY_DRAG guard: every button-up system-wide lands here, and the hook
        // may not take STATE without an atomic saying there is a drag to end
        // (bar-to-hold #2). It is stored on this thread right after every mode
        // change, so it equals `mode != None` wherever the hook reads it.
        WM_LBUTTONUP if ANY_DRAG.load(Ordering::Relaxed) => {
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
                    RECT {
                        left: cx,
                        top: cy,
                        right: cx + cw,
                        bottom: cy + ch,
                    },
                ));
                drag_preview_end();
                return suppress;
            }
        }
        WM_RBUTTONUP if ANY_DRAG.load(Ordering::Relaxed) => {
            let mut s = STATE.lock().unwrap();
            if s.mode == Mode::Resize {
                let h = s.hwnd;
                let (cx, cy, cw, ch) = (s.cur_x, s.cur_y, s.cur_w, s.cur_h);
                s.mode = Mode::None;
                ANY_DRAG.store(false, Ordering::Relaxed);
                drop(s);
                // Push first (the manager brings a parked window back: to the
                // previewed rect, or, for a tiled window placed instantly,
                // straight to its new tile), then tear the preview down.
                push_cmd(Cmd::DragResized(
                    h,
                    Some(RECT {
                        left: cx,
                        top: cy,
                        right: cx + cw,
                        bottom: cy + ch,
                    }),
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
    /// Adopt (or follow) a window. The u32 is the WinEvent that asked, so a
    /// rejection can say what triggered it.
    Add(isize, u32),
    Remove(isize),
    /// App-driven hide: untrack unless the window is visible again by then.
    RemoveHidden(isize),
    Focused(isize),
    ActivateWindow(isize),
    FocusDir(i32),
    SwapDir(i32),
    PromoteMaster,
    ResizeMaster(f32),
    Switch(usize),
    MoveToWs(usize),
    ToggleTiling,
    ToggleFloat,
    CloseFocused,
    /// This window was minimized or restored: re-tile its own monitor, if it
    /// is tiled there (`retile_for_target`, resolved when processed).
    RetileFor(isize),
    RefreshMonitors,
    /// A work area may have moved with no display change (SPI_SETWORKAREA).
    RefreshWorkAreas,
    // Alt-drag lifecycle. The hook never touches the real window (a cross-process
    // SetWindowPos can stall on a busy app) — it previews with an overlay and
    // pushes these; the manager parks/commits the real window.
    DragPark(isize), // thumbnail drag began: park the window off-screen
    /// Alt+left-drag started on a MAXIMIZED window: un-maximize it and put it
    /// at the predicted restored rect. `ShowWindow(SW_RESTORE)` drives the
    /// target's own message loop, so it must never run on the hook.
    DragUnmaximize(isize, RECT),
    DragMoved(isize, i32, i32, RECT), // dropped after Alt+left-drag: (hwnd, x, y, final rect)
    DragResized(isize, Option<RECT>), // released after resize; None = read the live rect
    LaunchTerminal,                   // Alt+Enter
    LaunchBrowser,                    // Alt+Shift+Enter
    FocusGeo(Dir),                    // Alt+arrow: focus the window in a direction
    MoveGeo(Dir),                     // Alt+Shift+arrow: move the window in a direction
    FocusMouse(isize),                // focus-follows-mouse: cursor hovered this window
    BarClick(isize, usize),           // bar pill clicked: (monitor hmon, local workspace)
    BarFocus(isize),                  // bar app-button clicked: focus this window
    BarCycle(isize, i32),             // bar wheel: (monitor hmon, +1 next / -1 prev workspace)
    Extra(usize),                     // compiled extra-hotkey index; strings stay off the hook path
    SetLayout(String),
    ToggleScratchpad,
    /// Config changed; apply live. true = full (an explicit reload redoes
    /// everything), false = only the groups that differ from mgr.cfg.
    Reload(Box<Config>, bool),
    /// The focused window renamed itself (browser tab, editor file, download
    /// progress). Nothing to re-tile — the manager loop repaints the bar after
    /// every command, and `update_bar` only repaints monitors whose data
    /// changed. That diff saves the repaint, not the tick (update_bar's title
    /// reads plus sync_managed), so at most one is ever queued: see
    /// `bar_refresh_gate`. Why that matters is an unmeasured assumption: at
    /// normal title rates the queue never grows; it bounds kHz title spam and
    /// renames piling up behind a long command (BAR-10).
    BarRefresh,
}

impl Cmd {
    /// Variant name for the manager's per-command probe line.
    fn name(&self) -> &'static str {
        match self {
            Cmd::Add(..) => "Add",
            Cmd::Remove(_) => "Remove",
            Cmd::RemoveHidden(_) => "RemoveHidden",
            Cmd::Focused(_) => "Focused",
            Cmd::ActivateWindow(_) => "ActivateWindow",
            Cmd::FocusDir(_) => "FocusDir",
            Cmd::SwapDir(_) => "SwapDir",
            Cmd::PromoteMaster => "PromoteMaster",
            Cmd::ResizeMaster(_) => "ResizeMaster",
            Cmd::Switch(_) => "Switch",
            Cmd::MoveToWs(_) => "MoveToWs",
            Cmd::ToggleTiling => "ToggleTiling",
            Cmd::ToggleFloat => "ToggleFloat",
            Cmd::CloseFocused => "CloseFocused",
            Cmd::RetileFor(_) => "RetileFor",
            Cmd::RefreshMonitors => "RefreshMonitors",
            Cmd::RefreshWorkAreas => "RefreshWorkAreas",
            Cmd::DragPark(_) => "DragPark",
            Cmd::DragUnmaximize(..) => "DragUnmaximize",
            Cmd::DragMoved(..) => "DragMoved",
            Cmd::DragResized(..) => "DragResized",
            Cmd::LaunchTerminal => "LaunchTerminal",
            Cmd::LaunchBrowser => "LaunchBrowser",
            Cmd::FocusGeo(_) => "FocusGeo",
            Cmd::MoveGeo(_) => "MoveGeo",
            Cmd::FocusMouse(_) => "FocusMouse",
            Cmd::BarClick(..) => "BarClick",
            Cmd::BarFocus(_) => "BarFocus",
            Cmd::BarCycle(..) => "BarCycle",
            Cmd::Extra(_) => "Extra",
            Cmd::SetLayout(_) => "SetLayout",
            Cmd::ToggleScratchpad => "ToggleScratchpad",
            Cmd::Reload(..) => "Reload",
            Cmd::BarRefresh => "BarRefresh",
        }
    }
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
static WINDOW_RULES: Mutex<Vec<WindowRule>> = Mutex::new(Vec::new());
static SCRATCHPAD_HWND: AtomicIsize = AtomicIsize::new(0);
static SCRATCHPAD_PENDING_AT: AtomicU64 = AtomicU64::new(0);
static SCRATCHPAD_HIDDEN: AtomicBool = AtomicBool::new(false);
static WINDOW_MRU: Mutex<VecDeque<isize>> = Mutex::new(VecDeque::new());
static WALLPAPER_REQ: Mutex<Option<String>> = Mutex::new(None);
static WALLPAPER_CV: Condvar = Condvar::new();
static WALLPAPER_LAST: Mutex<String> = Mutex::new(String::new());
static STATE_REQ: Mutex<Option<String>> = Mutex::new(None);
static STATE_CV: Condvar = Condvar::new();
static MRU_REQ: Mutex<Option<String>> = Mutex::new(None);
static MRU_CV: Condvar = Condvar::new();
static LAUNCHER_MRU: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);
static MRU_TICK: AtomicU64 = AtomicU64::new(0);
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

#[derive(Clone, Copy)]
struct ExtraBind {
    vk: u32,
    shift: bool,
    ctrl: bool,
    index: usize,
}
static EXTRA_HOTKEYS: Mutex<Vec<ExtraBind>> = Mutex::new(Vec::new());

fn chord_vk(name: &str) -> Option<u32> {
    config::key_to_vk(name).or_else(|| match name.trim().to_ascii_uppercase().as_str() {
        "SPACE" => Some(0x20),
        "TAB" => Some(0x09),
        "ENTER" | "RETURN" => Some(0x0D),
        "BACKSPACE" | "BACK" => Some(0x08),
        "GRAVE" | "BACKTICK" | "OEM_3" => Some(0xC0),
        "MINUS" | "OEM_MINUS" => Some(0xBD),
        "EQUAL" | "OEM_PLUS" => Some(0xBB),
        _ => None,
    })
}

fn compile_extra_hotkeys(cfg: &Config) -> Vec<ExtraBind> {
    cfg.extra_hotkeys
        .iter()
        .enumerate()
        .filter_map(|(index, def)| {
            if def.action == "scratchpad" && !cfg.scratchpad_enabled {
                return None;
            }
            let parts: Vec<&str> = def.chord.split('+').collect();
            let has_alt = parts.iter().any(|p| p.eq_ignore_ascii_case("ALT"));
            let key = parts.iter().rev().find_map(|p| chord_vk(p))?;
            has_alt.then_some(ExtraBind {
                vk: key,
                shift: parts.iter().any(|p| p.eq_ignore_ascii_case("SHIFT")),
                ctrl: parts.iter().any(|p| p.eq_ignore_ascii_case("CTRL")),
                index,
            })
        })
        .collect()
}

fn apply_hook_config(cfg: &Config) {
    // Every startup and every reload comes through here, so this is the one
    // place the log level has to be applied.
    LOG_LEVEL.store(log_level_from_str(&cfg.log_level), Ordering::Relaxed);
    // Logged at ERROR — the default level — because a key Astur did not
    // understand is a setting the user believes is in effect and is not.
    for key in &cfg.unknown_keys {
        log_error!("config line not understood (ignored): {key}");
    }
    FOLLOW_MOUSE.store(cfg.focus_follows_mouse, Ordering::Relaxed);
    // Logged at startup and on each reload: which placement mode ran is the
    // first question for a misplaced-window report.
    ASYNC_WINDOW_POS.store(cfg.async_window_pos, Ordering::Relaxed);
    log_info!(
        "window placement: {} (async_window_pos = {})",
        if cfg.async_window_pos {
            "posted"
        } else {
            "synchronous"
        },
        cfg.async_window_pos
    );
    *IGNORE_CLASSES.lock().unwrap() = cfg.ignore_classes.clone();
    *FLOAT_CLASSES.lock().unwrap() = cfg.float_classes.clone();
    *WINDOW_RULES.lock().unwrap() = cfg.window_rules.clone();
    *WORKSPACE_KEYS.lock().unwrap() = cfg.workspace_keys.clone();
    *EXTRA_HOTKEYS.lock().unwrap() = compile_extra_hotkeys(cfg);
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
static MEDIA_TEXT: Mutex<String> = Mutex::new(String::new());
static MEDIA_ON: AtomicBool = AtomicBool::new(false);

// ---- bar v2 style/behaviour (ensure_bars + the mouse hook read these) ----
static BAR_FLOATING: AtomicBool = AtomicBool::new(false);
static BAR_MARGIN: AtomicIsize = AtomicIsize::new(8);
static BAR_RADIUS: AtomicIsize = AtomicIsize::new(12);
static BAR_AUTOHIDE: AtomicBool = AtomicBool::new(false);
static BAR_WHEEL_WS: AtomicBool = AtomicBool::new(true);
// Top-level app HWND -> HMONITOR for every visible maximized/fullscreen app.
// Main-thread WinEvents maintain this map. Bars only read it while rebuilding,
// never from hook or paint hot paths.
static FULLSCREEN_WINDOWS: Mutex<Option<HashMap<isize, isize>>> = Mutex::new(None);

// Hook-visible bar hit rects, lock-free (the mouse hook may not take locks).
// Slot i is bar i's on-screen rect while it accepts wheel input; hwnd 0 = empty.
// BARS_HOT short-circuits the whole check to one atomic load when idle.
const MAX_BARS: usize = 8;
static BARS_HOT: AtomicBool = AtomicBool::new(false);
/// One-shot guard so an overflowing bar array reports itself exactly once.
static BARHIT_FULL_LOGGED: AtomicBool = AtomicBool::new(false);
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
    let Some(i) = slot else {
        // More than MAX_BARS monitors: this bar silently loses wheel routing.
        // Rare, but it used to be invisible. Log it once — the fixed array has
        // to stay (the hook reads it lock-free).
        if !BARHIT_FULL_LOGGED.swap(true, Ordering::Relaxed) {
            log_error!("more than {MAX_BARS} bars: wheel routing dropped for bar {hwnd:#x}");
        }
        return;
    };
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
static BAR_HOVER_HWND: AtomicIsize = AtomicIsize::new(0);
static BAR_HOVER_APP: AtomicIsize = AtomicIsize::new(0);
/// The bar holding a TME_LEAVE request (0 = none). Main thread only (bar_wndproc).
static BAR_LEAVE_ARMED: AtomicIsize = AtomicIsize::new(0);
/// Partial wheel delta per bar (keyed by its monitor), towards the next whole
/// workspace step. Main thread only (bar_wndproc).
static BAR_WHEEL_ACC: Mutex<Option<HashMap<isize, i32>>> = Mutex::new(None);

/// Whole workspace steps in one wheel event, carrying partial notches in
/// `acc` (> 0 = up). The remainder resets when the direction flips, so a
/// half-notch the other way never counts towards this one.
fn wheel_steps(acc: &mut i32, delta: i32) -> i32 {
    const WHEEL_DELTA: i32 = 120;
    if *acc != 0 && (*acc > 0) != (delta > 0) {
        *acc = 0;
    }
    *acc += delta;
    let steps = *acc / WHEEL_DELTA;
    *acc -= steps * WHEEL_DELTA;
    steps
}

/// Auto-hide runtime state per bar window (bar/main thread only). `y_cur` eases
/// toward shown/hidden each AH_TIMER tick, so the bar slides rather than pops.
/// `strip` is the reveal band on the bar's docked screen edge.
#[derive(Clone, Copy)]
struct AhBar {
    x: i32,
    w: i32,
    h: i32,
    y_shown: i32,
    y_hidden: i32,
    y_cur: f64,
    shown: bool,
    strip: RECT,
    /// Cursor grab tolerance around the bar, physical px on ITS monitor.
    tol: i32,
}
static AH_BARS: Mutex<Option<HashMap<isize, AhBar>>> = Mutex::new(None);
const AH_TIMER_ID: usize = 4;

/// Sliding workspace-pill highlight. While an entry is present for a monitor,
/// paint_bar draws the accent pill at an interpolated position between the old
/// and new pill INDEX instead of snapping (indices, not x's: with configurable
/// zones the pills' origin is only known at paint time). Keyed by HMONITOR,
/// driven by a fast WM_TIMER on the bar window.
struct PillAnim {
    from: f64, // pill units; fractional when a slide was retargeted mid-way
    to_i: i32,
    start: Instant,
}
static PILL_ANIM: Mutex<Option<HashMap<isize, PillAnim>>> = Mutex::new(None);
const PILL_ANIM_MS: f64 = 160.0;

fn pill_anim_set(hmon: isize, from_i: i32, to_i: i32) {
    let mut guard = PILL_ANIM.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    // Read the in-flight position under the same lock as the insert, so no
    // paint can see a position between the two.
    let next = pill_retarget(map.get(&hmon), from_i, to_i, Instant::now());
    map.insert(hmon, next);
}

/// A new slide to `to_i`. Mid-slide it starts from where the highlight IS:
/// seeding from `from_i` (update_bar's previous target) made a second switch
/// jump back to that target first. Easing, duration and start are unchanged.
fn pill_retarget(cur: Option<&PillAnim>, from_i: i32, to_i: i32, now: Instant) -> PillAnim {
    let from = match cur.map(|a| pill_pos(a, now)) {
        Some((pos, false)) => pos,
        _ => from_i as f64,
    };
    PillAnim {
        from,
        to_i,
        start: now,
    }
}

/// Where `a` puts the highlight at `now` (pill units), and whether it arrived.
fn pill_pos(a: &PillAnim, now: Instant) -> (f64, bool) {
    let t = (now.saturating_duration_since(a.start).as_secs_f64() * 1000.0 / PILL_ANIM_MS).min(1.0);
    (
        a.from + (a.to_i as f64 - a.from) * ease_in_out_cubic(t),
        t >= 1.0,
    )
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
    Some(pill_pos(a, Instant::now()))
}

/// Per-monitor paint data. One entry per drawn pill: `slots[i]` is the local
/// workspace index that pill maps to (so a click resolves straight to a
/// workspace even when empty pills are hidden), `labels[i]` is the number to
/// print, `occupied` bit i marks a pill whose workspace has windows, and
/// `active` is the pill index of the shown workspace (usize::MAX if none).
/// `apps` lists the active workspace's windows (hwnd + cached exe HICON) for
/// the app-buttons widget.
#[derive(Clone, PartialEq)]
struct BarApp {
    hwnd: isize,
    icon: isize,
    label: String,
}

#[derive(Clone, PartialEq)]
struct MonBar {
    hmon: isize,
    slots: Vec<usize>,
    labels: Vec<String>,
    active: usize,
    occupied: u64,
    title: String,
    apps: Vec<BarApp>,
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
    Media,
    Separator,
    Spacer,
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
            "media" if cfg.bar_show_media => Some(BarWidget::Media),
            "separator" => Some(BarWidget::Separator),
            "spacer" => Some(BarWidget::Spacer),
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
    bar_colors(cfg, THEME_LIGHT.load(Ordering::Relaxed))
}

/// `themed_bar_colors` for an explicit theme.
fn bar_colors(cfg: &Config, light: bool) -> (u32, u32, u32, u32) {
    let preset = if light {
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
    clock_format: String,
    icon_mode: String,
    show_app_labels: bool,
    show_tooltips: bool,
    cpu_format: String,
    mem_format: String,
    battery_format: String,
    net_format: String,
    volume_format: String,
    icon_cpu: String,
    icon_mem: String,
    icon_battery: String,
    icon_net: String,
    icon_volume: String,
    layout: String,
    tiling: bool,
    left: Vec<BarWidget>,
    center: Vec<BarWidget>,
    right: Vec<BarWidget>,
    mons: Vec<MonBar>,
}

impl BarData {
    fn new() -> Self {
        BarData {
            bg: 0x00261B1A,
            fg: 0x00F5CAC0,
            accent: 0x00FFAA66,
            inactive: 0x00895F56,
            clock_24h: true,
            date_format: String::new(),
            clock_format: "HH:mm".to_string(),
            icon_mode: "both".to_string(),
            show_app_labels: false,
            show_tooltips: true,
            cpu_format: "{value}%".to_string(),
            mem_format: "RAM {value}%".to_string(),
            battery_format: "BAT {value}%".to_string(),
            net_format: "{down} {up}".to_string(),
            volume_format: "VOL {value}%".to_string(),
            icon_cpu: String::new(),
            icon_mem: String::new(),
            icon_battery: String::new(),
            icon_net: String::new(),
            icon_volume: String::new(),
            layout: String::new(),
            tiling: true,
            left: Vec::new(),
            center: Vec::new(),
            right: Vec::new(),
            mons: Vec::new(),
        }
    }
}

static BAR: LazyLock<Mutex<BarData>> = LazyLock::new(|| Mutex::new(BarData::new()));
// Custom message: manager asks a bar to repaint.
const WM_BAR_REFRESH: u32 = WM_USER + 1;
// Custom message: manager seeds a pill-highlight slide (wparam=from pill index,
// lparam=to pill index — paint resolves indices to x's, zones move the origin).
const WM_PILL_ANIM: u32 = WM_USER + 3;
// Custom message from the LL mouse hook: wheel over this bar (wparam: 1=up,
// 0=down; lparam = screen x of the cursor).
const WM_BAR_WHEEL: u32 = WM_USER + 4;
// Custom message to the marker window: per-monitor fullscreen bar mode changed.
const WM_BAR_MODE_CHANGED: u32 = WM_USER + 5;
// SetTimer id for the pill-slide animation (distinct from the clock tick).
const PILL_TIMER_ID: usize = 2;
// Custom message (to the marker window): config changed, rebuild bars on the
// main thread.
const WM_RELOAD: u32 = WM_USER + 2;
// Custom message (to the marker window): the watchdog believes the low-level
// hooks are gone. Re-arming must happen on the thread that owns them.
const WM_REARM_HOOKS: u32 = WM_USER + 6;
// Custom message (to the marker window): rebuild bar geometry, once, OUTSIDE
// the message that asked for it. See `request_bar_rebuild`.
const WM_REBUILD_BARS: u32 = WM_USER + 7;
// SetTimer id for the bar clock tick.
const BAR_TIMER_ID: usize = 1;

// =========================================================================
// Hook watchdog
// =========================================================================
// Windows silently removes a low-level hook whose proc overruns
// HKEY_CURRENT_USER\Control Panel\Desktop\LowLevelHooksTimeout (300 ms by
// default). There is no message, no error and no callback: Astur simply
// becomes a running process that does nothing — Alt-drag, every hotkey, the
// launcher and the system menu all stop, with no way for the user to tell why.
// Before this watchdog existed the only recovery was for the user to guess and
// restart the app (review B-03).
//
// The detector is deliberately dumb: the hooks stamp an atomic (a relaxed store
// of the tick count — no lock, no allocation, hook-legal), and this thread asks
// the OS when input last happened. Input recently, no callback for a while, and
// the hooks are gone.

/// Last time either hook proc ran (GetTickCount64 ms).
static HOOK_TICK: AtomicU64 = AtomicU64::new(0);
static MOUSE_HOOK_H: AtomicIsize = AtomicIsize::new(0);
static KBD_HOOK_H: AtomicIsize = AtomicIsize::new(0);
/// How many times the watchdog has had to put the hooks back. Nonzero here is
/// the single most useful number in a bug report.
static HOOK_REARMS: AtomicU32 = AtomicU32::new(0);

/// Input seen this recently counts as "the user is using the machine".
const WATCHDOG_INPUT_WINDOW_MS: u32 = 1_000;
/// No hook callback for this long, while input is happening, means unhooked.
/// With the 1 s base poll a dead hook is caught 2-3 s after it dies (it was
/// 5-10 s: 5 s poll and silence). The cost: a main thread stalled past 2 s
/// now logs "not pumping" sooner.
const WATCHDOG_SILENCE_MS: u64 = 2_000;
/// Watchdog poll while healthy; doubles per failed re-arm, up to a minute.
const WATCHDOG_POLL_MS: u64 = 1_000;
/// The main thread's "re-armed" line was logged at ERROR this episode; later
/// re-arms log at debug. Cleared by the watchdog when hooks are healthy again.
static REARM_LOGGED: AtomicBool = AtomicBool::new(false);

#[inline]
fn hook_alive_stamp() {
    // Cheap enough for the input path: GetTickCount64 reads the shared user
    // data page, and the store is relaxed.
    HOOK_TICK.store(unsafe { GetTickCount64() }, Ordering::Relaxed);
}

/// Install both low-level hooks, replacing any existing ones. Must run on the
/// thread that pumps messages for them (the main thread).
unsafe fn install_hooks(hinst: HINSTANCE) -> bool {
    for slot in [&MOUSE_HOOK_H, &KBD_HOOK_H] {
        let old = slot.swap(0, Ordering::Relaxed);
        if old != 0 {
            let _ = UnhookWindowsHookEx(HHOOK(old as *mut c_void));
        }
    }
    let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hinst, 0);
    let kbd = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), hinst, 0);
    match (mouse, kbd) {
        (Ok(m), Ok(k)) => {
            MOUSE_HOOK_H.store(m.0 as isize, Ordering::Relaxed);
            KBD_HOOK_H.store(k.0 as isize, Ordering::Relaxed);
            hook_alive_stamp();
            true
        }
        (m, k) => {
            // Partial success leaves a half-working WM; drop both.
            if let Ok(m) = m {
                let _ = UnhookWindowsHookEx(m);
            }
            if let Ok(k) = k {
                let _ = UnhookWindowsHookEx(k);
            }
            false
        }
    }
}

/// Whether hook silence means the hooks are gone. Input they could never see
/// is not evidence: UIPI keeps a non-elevated process's LL hooks from input
/// to an elevated foreground window, and hooks see only their own desktop
/// (never the secure one: UAC prompt, Ctrl+Alt+Del, lock screen). With the
/// shorter thresholds, typing into an elevated window would otherwise re-arm
/// every 2 s, two ERROR lines each, and rotate the 1 MiB log in hours.
fn hooks_look_dead(
    silence_ms: u64,
    idle_ms: u32,
    fg_elevated: bool,
    self_elevated: bool,
    other_desktop: bool,
) -> bool {
    idle_ms <= WATCHDOG_INPUT_WINDOW_MS
        && silence_ms >= WATCHDOG_SILENCE_MS
        && !other_desktop
        && !(fg_elevated && !self_elevated)
}

/// Is the foreground window's process elevated? A process we cannot open or
/// whose token we cannot read counts as elevated: that is what denies us, and
/// the cost of a wrong guess is one skipped check. Watchdog thread, called
/// only when the hooks already look silent, so it is never on a hot path.
unsafe fn foreground_elevated() -> bool {
    let fg = GetForegroundWindow();
    if fg.0.is_null() {
        return false;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(fg, Some(&mut pid));
    if pid == 0 || pid == GetCurrentProcessId() {
        return false;
    }
    let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
        return true;
    };
    let elevated = token_elevated(process).unwrap_or(true);
    let _ = CloseHandle(process);
    elevated
}

/// A desktop's name (UOI_NAME, e.g. "Default"), or None if it can't be read.
unsafe fn desktop_name(desk: windows::Win32::System::StationsAndDesktops::HDESK) -> Option<String> {
    use windows::Win32::System::StationsAndDesktops::{GetUserObjectInformationW, UOI_NAME};
    let mut name = [0u16; 64];
    GetUserObjectInformationW(
        HANDLE(desk.0),
        UOI_NAME,
        Some(name.as_mut_ptr() as *mut c_void),
        (name.len() * 2) as u32,
        None,
    )
    .ok()?;
    let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    Some(String::from_utf16_lossy(&name[..len]))
}

/// Is input going to a desktop other than `own`, the one the hooks were
/// installed on? They see input to that desktop only: not the secure desktop
/// (UAC prompt, Ctrl+Alt+Del, lock screen; OpenInputDesktop is denied there),
/// a screen saver's, or another program's private desktop.
unsafe fn input_desktop_foreign(own: &str) -> bool {
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, OpenInputDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS,
    };
    let Ok(desk) = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) else {
        return true;
    };
    let name = desktop_name(desk);
    let _ = CloseDesktop(desk);
    name.is_some_and(|n| !n.eq_ignore_ascii_case(own))
}

/// Watchdog loop. Cheap: one GetLastInputInfo a second, no locks.
fn hook_watchdog() {
    // Re-arms are attempted with a backoff. The first version logged and posted
    // every 5 s forever, and stamped HOOK_TICK itself so it "would not re-fire
    // too soon" — which reset the very measurement it was reporting, so every
    // line after the first read exactly 5000 ms and the real, growing outage was
    // invisible. Don't do that: never let a detector write to the thing it
    // detects on.
    let mut backoff_ms = WATCHDOG_POLL_MS;
    let mut attempts = 0u32;
    // An episode runs from the first silent check to a healthy check with no
    // re-arm in flight. Not the first healthy check: install_hooks stamps
    // HOOK_TICK, so the check right after any re-arm always reads healthy,
    // and an episode ending there would log both ERROR lines every cycle.
    let mut episode = false;
    let self_elevated = unsafe { process_elevated() };
    // This thread's desktop is the process's, the one the main thread
    // installed the hooks on. The thread's own handle: never closed.
    let own_desktop = unsafe {
        windows::Win32::System::StationsAndDesktops::GetThreadDesktop(GetCurrentThreadId())
            .ok()
            .and_then(|d| desktop_name(d))
    }
    .unwrap_or_else(|| "Default".to_string());
    let mut reported_wedged = false;
    // Re-arm count at the moment we last posted, so "did the main thread act on
    // it?" stays a correct question after the first successful re-arm.
    let mut posted_at: Option<u32> = None;
    // Time slept towards the next health check. At log_level = debug the thread
    // also wakes once a second to report the hook delivery delay; otherwise it
    // sleeps straight through to the check, exactly as before.
    let mut waited_ms = 0u64;
    let mut debug_ticks = 0u32;
    let mut last_counters = String::new();
    loop {
        let due_ms = backoff_ms.min(60_000);
        let debug = log_on(LOG_DEBUG);
        let step_ms = if debug {
            1_000.min(due_ms - waited_ms)
        } else {
            due_ms - waited_ms
        };
        std::thread::sleep(std::time::Duration::from_millis(step_ms));
        waited_ms += step_ms;
        if debug {
            // GetTickCount moves in ~15.6 ms steps (timeBeginPeriod does not
            // change that; measured 15/16 ms steps with it set), so a one-step
            // reading can be a sub-millisecond delay straddling a tick. Two steps
            // or more guarantees a real delay of at least one tick.
            let late = HOOK_DELAY_MAX.swap(0, Ordering::Relaxed);
            if late > 16 {
                log_debug!("hook delay: an input event reached the LL hooks {late} ms late");
            }
            // Intake counters every 10 s, only when something moved.
            debug_ticks += 1;
            if debug_ticks.is_multiple_of(10) {
                let line = counters_line();
                if line != last_counters {
                    log_debug!("counters {line}");
                    last_counters = line;
                }
            }
        }
        if waited_ms < due_ms {
            continue;
        }
        waited_ms = 0;
        unsafe {
            let mut lii = LASTINPUTINFO {
                cbSize: core::mem::size_of::<LASTINPUTINFO>() as u32,
                dwTime: 0,
            };
            if !GetLastInputInfo(&mut lii).as_bool() {
                continue;
            }
            let idle_ms = GetTickCount().wrapping_sub(lii.dwTime);
            let silence_ms = GetTickCount64().saturating_sub(HOOK_TICK.load(Ordering::Relaxed));
            // The two syscall checks run only once the hooks already look
            // silent, so a healthy poll stays one GetLastInputInfo.
            let dead = hooks_look_dead(silence_ms, idle_ms, false, self_elevated, false)
                && hooks_look_dead(
                    silence_ms,
                    idle_ms,
                    foreground_elevated(),
                    self_elevated,
                    input_desktop_foreign(&own_desktop),
                );
            if !dead {
                // Healthy, or silent only because the input is out of the
                // hooks' reach: forget any previous trouble.
                if posted_at.is_none() && episode {
                    episode = false;
                    REARM_LOGGED.store(false, Ordering::Relaxed);
                }
                backoff_ms = WATCHDOG_POLL_MS;
                attempts = 0;
                posted_at = None;
                reported_wedged = false;
                continue;
            }
            let marker = MARKER_HWND.load(Ordering::Relaxed);
            if marker == 0 {
                continue;
            }
            // A re-arm can only happen on the thread that owns the hooks, so if
            // that thread is not pumping messages, posting is pointless. Tell
            // the two failures apart — they have completely different causes.
            if posted_at.is_some_and(|n| HOOK_REARMS.load(Ordering::Relaxed) == n) {
                if !reported_wedged {
                    reported_wedged = true;
                    log_error!(
                        "hooks silent {silence_ms} ms and {attempts} re-arm request(s) went                          unprocessed — the main thread is not pumping messages. Astur cannot                          recover from here; this is a deadlock or a re-entrant wndproc, not a                          dropped hook."
                    );
                }
                backoff_ms = (backoff_ms * 2).min(60_000);
                continue;
            }
            attempts += 1;
            posted_at = Some(HOOK_REARMS.load(Ordering::Relaxed));
            if !episode {
                episode = true;
                log_error!(
                    "hooks silent for {silence_ms} ms with input {idle_ms} ms ago — re-arming"
                );
            } else {
                log_debug!(
                    "hooks silent for {silence_ms} ms with input {idle_ms} ms ago — re-arming"
                );
            }
            let _ = PostMessageW(hwnd_from(marker), WM_REARM_HOOKS, WPARAM(0), LPARAM(0));
            backoff_ms = (backoff_ms * 2).min(60_000);
        }
    }
}

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
    hmon: isize,     // HMONITOR (raw) — identity across enumerations
    base_work: RECT, // taskbar-excluded area, before the bar is subtracted
    work_area: RECT, // tiling area (base_work minus the status bar)
    workspaces: Vec<Workspace>,
    active: usize, // index of the currently-shown workspace
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

/// One queued switch command, resolved against a burst's virtual state.
#[derive(Debug, PartialEq)]
enum SwitchStep {
    /// Would show (monitor, local workspace).
    To(usize, usize),
    /// Changes nothing today: `process` returns before any effect.
    NoOp,
    /// Not foldable: another command, or a switch for another monitor.
    Stop,
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
    // Where Cmd::DragPark took a window from (hwnd, top-left before the park),
    // so a tiled resize drop can move it back position-only, onto its own
    // monitor and DPI, before the retile sizes it (INPUT-5). Taken by the drop.
    park_origin: Option<(isize, POINT)>,
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
        self.global_to_ml_at(i, self.focused_mon)
    }

    /// `global_to_ml` with `focus` standing in for the focused monitor, so a
    /// burst of queued switches can be resolved against where the earlier
    /// ones in it would have left focus (see `fold_switches`).
    fn global_to_ml_at(&self, i: usize, focus: usize) -> (usize, usize) {
        if self.cfg.per_monitor {
            (focus.min(self.monitors.len().saturating_sub(1)), i)
        } else {
            let n = self.monitors.len().max(1);
            ((self.primary + (i % n)) % n, i / n)
        }
    }

    /// Where `cmd` would switch, resolved exactly as `process` resolves a
    /// Switch / BarCycle, but against a virtual focused monitor `focus` and,
    /// once a burst is under way, its monitor and virtual active workspace
    /// `on`. Pure: reads the model only.
    fn switch_step(&self, cmd: &Cmd, focus: usize, on: Option<(usize, usize)>) -> SwitchStep {
        let (mi, ws) = match *cmd {
            Cmd::Switch(i) => {
                if i >= self.cfg.workspaces || self.monitors.is_empty() {
                    return SwitchStep::NoOp;
                }
                let (mi, local) = self.global_to_ml_at(i, focus);
                if mi >= self.monitors.len() || local >= self.monitors[mi].workspaces.len() {
                    return SwitchStep::NoOp;
                }
                (mi, local)
            }
            Cmd::BarCycle(hmon, d) => {
                let Some(mi) = self.mon_by_hmon(hmon) else {
                    return SwitchStep::NoOp;
                };
                let count = self.monitors[mi].workspaces.len();
                if count <= 1 {
                    return SwitchStep::NoOp;
                }
                // A delta applies to wherever the burst has got to, not to the
                // real active workspace: [Switch(a), BarCycle(+1)] ends on a+1.
                let cur = match on {
                    Some((m, a)) if m == mi => a,
                    _ => self.monitors[mi].active,
                };
                (mi, (cur as i32 + d).rem_euclid(count as i32) as usize)
            }
            _ => return SwitchStep::Stop,
        };
        match on {
            Some((m, _)) if m != mi => SwitchStep::Stop,
            _ => SwitchStep::To(mi, ws),
        }
    }

    /// Fold `first` (a Switch or BarCycle just popped) and the Switch /
    /// BarCycle commands queued directly behind it for the same monitor into
    /// the one workspace the run ends on (SWITCH-17). Commands are applied in
    /// order against the virtual state, so a later Switch overrides earlier
    /// deltas and a no-op (bad index, unknown monitor) never becomes the
    /// target. Pops what it folds from `q`, and stops without popping at any
    /// other command (Focused, Add, MoveToWs, Extra: never looked inside) and
    /// at a switch for another monitor, so ordering against everything else
    /// is kept. Pure: the caller holds CMDQ, which the LL keyboard hook pushes
    /// through, so no Win32 call and no switch may happen in here.
    /// Returns (monitor, workspace) or None when all were no-ops, and how many
    /// commands it popped.
    fn fold_switches(&self, first: &Cmd, q: &mut VecDeque<Cmd>) -> (Option<(usize, usize)>, usize) {
        let mut focus = self.focused_mon;
        let mut on: Option<(usize, usize)> = None;
        let apply = |cmd: &Cmd, focus: &mut usize, on: &mut Option<(usize, usize)>| {
            match self.switch_step(cmd, *focus, *on) {
                SwitchStep::To(mi, ws) => {
                    // Every effective Switch / BarCycle sets focused_mon.
                    *focus = mi;
                    *on = Some((mi, ws));
                    true
                }
                SwitchStep::NoOp => true,
                SwitchStep::Stop => false,
            }
        };
        apply(first, &mut focus, &mut on);
        let mut popped = 0;
        while let Some(next) = q.front() {
            if !apply(next, &mut focus, &mut on) {
                break;
            }
            q.pop_front();
            popped += 1;
        }
        (on, popped)
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

    /// Remove `h` from whichever workspace owns it. Returns where it was and
    /// whether it was FLOATING there, and repairs that workspace's focus.
    ///
    /// This exists because the same three lines were open-coded at five call
    /// sites, and two of them forgot `floating` — so `Alt+Shift+3` on a floating
    /// window silently re-tiled it (review B-07). Membership changes go through
    /// here or through `move_window`; nothing else touches `windows`/`floating`.
    fn detach_window(&mut self, h: isize) -> Option<(usize, usize, bool)> {
        let (mi, wi) = self.locate(h)?;
        let ws = self.monitors.get_mut(mi)?.workspaces.get_mut(wi)?;
        let was_floating = ws.floating.contains(&h);
        ws.windows.retain(|&x| x != h);
        ws.floating.retain(|&x| x != h);
        if ws.focused == h {
            ws.focused = ws.windows.first().copied().unwrap_or(0);
        }
        Some((mi, wi, was_floating))
    }

    /// Move `h` to (`to_mi`, `to_wi`), CARRYING its floating flag, and give it
    /// that workspace's focus. `at` inserts before that tiled index (used when a
    /// drag is dropped onto a specific window); `None` appends.
    ///
    /// Returns false when the window is untracked or the destination does not
    /// exist — in which case nothing is changed, so a caller can never lose a
    /// window by moving it somewhere invalid.
    fn move_window(&mut self, h: isize, to_mi: usize, to_wi: usize, at: Option<usize>) -> bool {
        if self
            .monitors
            .get(to_mi)
            .and_then(|m| m.workspaces.get(to_wi))
            .is_none()
        {
            return false;
        }
        let Some((_, _, was_floating)) = self.detach_window(h) else {
            return false;
        };
        let ws = &mut self.monitors[to_mi].workspaces[to_wi];
        match at.filter(|&i| i <= ws.windows.len()) {
            Some(i) => ws.windows.insert(i, h),
            None => ws.windows.push(h),
        }
        if was_floating {
            ws.floating.push(h);
        }
        ws.focused = h;
        true
    }

    /// The focused window of the focused monitor's active workspace (0 = none),
    /// with its (monitor, workspace) — the `mi`/`a`/`focused` dance that was
    /// repeated ~20 times.
    fn focused(&self) -> (usize, usize, isize) {
        let mi = self.focused_mon.min(self.monitors.len().saturating_sub(1));
        let a = self.monitors.get(mi).map(|m| m.active).unwrap_or(0);
        let h = self
            .monitors
            .get(mi)
            .and_then(|m| m.workspaces.get(a))
            .map(|ws| ws.focused)
            .unwrap_or(0);
        (mi, a, h)
    }
}

/// Read a window's class name.
unsafe fn window_class(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = GetClassNameW(hwnd, &mut buf);
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

#[derive(Clone, Copy, PartialEq)]
enum RuleAction {
    Tile,
    Float,
    Ignore,
}

#[derive(Clone, Copy)]
struct RulePlacement {
    action: RuleAction,
    workspace: Option<usize>,
    monitor: Option<usize>,
}

fn glob_match(pattern: &str, value: &str) -> bool {
    let p = pattern.to_ascii_lowercase().into_bytes();
    let v = value.to_ascii_lowercase().into_bytes();
    let (mut pi, mut vi, mut star, mut mark) = (0usize, 0usize, None, 0usize);
    while vi < v.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == v[vi]) {
            pi += 1;
            vi += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            pi += 1;
            mark = vi;
        } else if let Some(si) = star {
            pi = si + 1;
            mark += 1;
            vi = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

fn rule_field(pattern: &str, value: &str, contains: bool) -> bool {
    if pattern.is_empty() {
        true
    } else if pattern.contains('*') || pattern.contains('?') {
        glob_match(pattern, value)
    } else if contains {
        value
            .to_ascii_lowercase()
            .contains(&pattern.to_ascii_lowercase())
    } else {
        pattern.eq_ignore_ascii_case(value)
    }
}

unsafe fn match_window_rule(hwnd: HWND) -> Option<RulePlacement> {
    let class = window_class(hwnd);
    let title = window_title(hwnd);
    let exe_path = window_exe(hwnd).unwrap_or_default();
    let exe_name = std::path::Path::new(&exe_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&exe_path);
    WINDOW_RULES.lock().unwrap().iter().find_map(|rule| {
        let exe_value = if rule.exe.contains('\\') || rule.exe.contains('/') {
            exe_path.as_str()
        } else {
            exe_name
        };
        if !rule_field(&rule.exe, exe_value, false)
            || !rule_field(&rule.class, &class, false)
            || !rule_field(&rule.title, &title, true)
        {
            return None;
        }
        let action = match rule.action.as_str() {
            "tile" => RuleAction::Tile,
            "float" => RuleAction::Float,
            "ignore" => RuleAction::Ignore,
            _ => return None,
        };
        Some(RulePlacement {
            action,
            workspace: rule.workspace,
            monitor: rule.monitor,
        })
    })
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
    "tooltips_class32",        // generic Win32 tooltips
    "LockScreenBackstopFrame", // lock screen
    "LockApp",
    "WinUIDesktopWin32WindowClass", // some transient WinUI shells
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

/// Can a window with this style never be an app window? Exactly the three
/// bits `app_surface_reject` refuses on: WS_CHILD, WS_EX_TOOLWINDOW,
/// WS_EX_NOACTIVATE. Also the EVENT_OBJECT_SHOW prefilter (EVENTS-1): menus,
/// tooltips and child controls stop costing a fullscreen probe and a Cmd::Add.
fn show_rejected_by_style(style: u32, exstyle: u32) -> bool {
    style & WS_CHILD.0 != 0 || exstyle & (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) != 0
}

/// Is this a visible top-level app surface (including owned presentation/game
/// popups with no title)? Used by fullscreen detection, not tiling adoption.
unsafe fn is_app_surface(hwnd: HWND) -> bool {
    app_surface_reject(hwnd).is_none()
}

/// `is_app_surface`, saying why not: None = it is one, else the first failed
/// check. The reasons feed the Cmd::Add rejection probe; one chain, so the
/// logged reason can never drift from the real decision.
unsafe fn app_surface_reject(hwnd: HWND) -> Option<&'static str> {
    if hwnd.0.is_null() || !IsWindowVisible(hwnd).as_bool() {
        return Some("invisible");
    }
    // Never treat our own windows (console, marker, bars) as app surfaces.
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == GetCurrentProcessId() {
        return Some("own-process");
    }
    // Only true top-level roots. Owned presentation popups remain eligible.
    if GetAncestor(hwnd, GA_ROOT) != hwnd {
        return Some("not-root");
    }
    let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    // Child windows, tool windows, and non-activatable windows (tooltips, OSDs,
    // the lock-screen overlay, IME candidates) are never real app windows.
    // The same test the SHOW prefilter applies, so the two cannot drift.
    if show_rejected_by_style(style, ex) {
        return Some("style");
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
        return Some("cloaked");
    }
    // Reject known shell/desktop classes.
    let class = window_class(hwnd);
    if BLOCK_CLASSES.contains(&class.as_str()) {
        return Some("class");
    }
    None
}

/// Is this a normal top-level application window (not shell/Astur chrome)?
/// None = yes, else why not (see `app_surface_reject`). Ignored/floating rules
/// are deliberately not checked: ignored games still need their monitor's
/// navbar to auto-hide while fullscreen.
unsafe fn app_window_reject(hwnd: HWND) -> Option<&'static str> {
    if let Some(why) = app_surface_reject(hwnd) {
        return Some(why);
    }
    // Tiling adopts only unowned, titled main windows. Fullscreen detection uses
    // is_app_surface directly so owned/no-title presentation windows still count.
    if let Ok(owner) = GetWindow(hwnd, GW_OWNER) {
        if !owner.0.is_null() {
            return Some("owned");
        }
    }
    (GetWindowTextLengthW(hwnd) <= 0).then_some("untitled")
}

/// Is this a normal top-level application window we should tile?
unsafe fn is_manageable(hwnd: HWND) -> bool {
    manage_reject(hwnd).is_none()
}

/// `is_manageable` with the reason (see `app_surface_reject`).
unsafe fn manage_reject(hwnd: HWND) -> Option<&'static str> {
    if let Some(why) = app_window_reject(hwnd) {
        return Some(why);
    }
    let class = window_class(hwnd);
    if IGNORE_CLASSES
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.eq_ignore_ascii_case(&class))
    {
        return Some("ignore_classes");
    }
    match_window_rule(hwnd)
        .is_some_and(|r| r.action == RuleAction::Ignore)
        .then_some("rule")
}

const FULLSCREEN_EDGE_TOLERANCE: i32 = 2;

/// Borderless fullscreen windows normally match rcMonitor exactly; tolerate a
/// tiny DWM border discrepancy. Maximized windows are detected separately via
/// IsZoomed because their rect stops at the Windows taskbar work area.
fn rect_covers_monitor(window: RECT, monitor: RECT) -> bool {
    window.left <= monitor.left + FULLSCREEN_EDGE_TOLERANCE
        && window.top <= monitor.top + FULLSCREEN_EDGE_TOLERANCE
        && window.right >= monitor.right - FULLSCREEN_EDGE_TOLERANCE
        && window.bottom >= monitor.bottom - FULLSCREEN_EDGE_TOLERANCE
}

/// Monitor occupied by this visible maximized/fullscreen app, if any.
unsafe fn fullscreen_monitor(hwnd: HWND) -> Option<isize> {
    if !is_app_surface(hwnd) || IsIconic(hwnd).as_bool() {
        return None;
    }
    let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
    let mut mi = MONITORINFO {
        cbSize: core::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !GetMonitorInfoW(hmon, &mut mi).as_bool() {
        return None;
    }
    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_err() {
        return None;
    }
    (IsZoomed(hwnd).as_bool() || rect_covers_monitor(rect, mi.rcMonitor)).then_some(hmon.0 as isize)
}

fn fullscreen_window_tracked(hwnd: isize) -> bool {
    FULLSCREEN_WINDOWS
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|m| m.contains_key(&hwnd))
}

/// Refresh one app's fullscreen membership. True only on an actual transition,
/// so resize WinEvents never flood navbar rebuild messages.
unsafe fn refresh_fullscreen_window(hwnd: HWND) -> bool {
    let h = hwnd.0 as isize;
    let monitor = fullscreen_monitor(hwnd);
    let mut guard = FULLSCREEN_WINDOWS.lock().unwrap();
    let windows = guard.get_or_insert_with(HashMap::new);
    if windows.get(&h).copied() == monitor {
        return false;
    }
    windows.remove(&h);
    if let Some(hmon) = monitor {
        windows.insert(h, hmon);
    }
    true
}

fn remove_fullscreen_window(hwnd: isize) -> bool {
    FULLSCREEN_WINDOWS
        .lock()
        .unwrap()
        .as_mut()
        .and_then(|m| m.remove(&hwnd))
        .is_some()
}

unsafe extern "system" fn fullscreen_enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let windows = &mut *(lparam.0 as *mut HashMap<isize, isize>);
    if let Some(hmon) = fullscreen_monitor(hwnd) {
        windows.insert(hwnd.0 as isize, hmon);
    }
    BOOL(1)
}

/// One-time/display-change reconciliation catches fullscreen apps already open
/// before Astur starts and drops stale monitor handles after topology changes.
unsafe fn seed_fullscreen_windows() {
    let mut windows = HashMap::new();
    let _ = EnumWindows(
        Some(fullscreen_enum_proc),
        LPARAM(&mut windows as *mut HashMap<isize, isize> as isize),
    );
    *FULLSCREEN_WINDOWS.lock().unwrap() = Some(windows);
}

fn monitor_has_fullscreen(hmon: isize) -> bool {
    FULLSCREEN_WINDOWS
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|m| m.values().any(|&monitor| monitor == hmon))
}

unsafe fn request_bar_mode_refresh() {
    let marker = MARKER_HWND.load(Ordering::Relaxed);
    if marker != 0 {
        let _ = PostMessageW(hwnd_from(marker), WM_BAR_MODE_CHANGED, WPARAM(0), LPARAM(0));
    }
}

/// Should a freshly-managed window start floating? Rich rules take precedence
/// over legacy class-only lists, so a tile rule can override a broad float list.
unsafe fn should_float(hwnd: HWND, rule: Option<RulePlacement>) -> bool {
    if let Some(rule) = rule {
        return rule.action == RuleAction::Float;
    }
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
    // The insets are the difference of two reads, GetWindowRect and the DWM
    // frame bounds. With posted placement (TILE-1) this window's own earlier
    // SetWindowPos can land between them (back-to-back retiles in an Add
    // burst, a drop's commit then its retile), and the "insets" are then the
    // move distance: the window would sit that far off its tile until the next
    // retile. Such a pair is re-read once (the move has landed by then);
    // still implausible, the target is used uncorrected, a few px off at worst.
    let max = dpi_px(BORDER_INSET_MAX, border_dpi(hwnd));
    for _ in 0..2 {
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
        if let Some((lp, tp, rp, bp)) = bounded_insets(
            fr.left - wr.left,
            fr.top - wr.top,
            wr.right - fr.right,
            wr.bottom - fr.bottom,
            max,
        ) {
            return RECT {
                left: target.left - lp,
                top: target.top - tp,
                right: target.right + rp,
                bottom: target.bottom + bp,
            };
        }
    }
    if log_on(LOG_DEBUG) {
        // Once per window: one with a genuinely odd frame would log on every
        // retile otherwise.
        let first = {
            let mut seen = BORDER_REJECTED.lock().unwrap();
            let seen = seen.get_or_insert_with(Default::default);
            // Bounded: HWNDs are recycled, and a long debug session would
            // otherwise grow this forever and mute a reused handle.
            if seen.len() >= BORDER_REJECTED_CAP {
                seen.clear();
            }
            seen.insert(hwnd.0 as isize)
        };
        if first {
            log_debug!(
                "border insets of {:#x} out of 0..={max} px; placed uncorrected",
                hwnd.0 as isize
            );
        }
    }
    target
}

/// Largest invisible-border inset (logical px) the border correction trusts.
/// Real frames are about 7-8 px at 100% on Windows 10 and 11.
const BORDER_INSET_MAX: i32 = 16;

/// Windows `adjust_for_border` already logged as rejected (debug only).
static BORDER_REJECTED: Mutex<Option<std::collections::HashSet<isize>>> = Mutex::new(None);
const BORDER_REJECTED_CAP: usize = 1024;

/// DPI to scale BORDER_INSET_MAX by. GetDpiForWindow is 96 for a DPI-unaware
/// app on any monitor, while its border is drawn scaled to the monitor's DPI in
/// the physical pixels this process reads, so take the larger of the two.
unsafe fn border_dpi(hwnd: HWND) -> u32 {
    let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
    window_dpi(hwnd).max(monitor_dpi(mon.0 as isize))
}

/// The border insets (left, top, right, bottom) when every one is a plausible
/// frame width, in 0..=max px; None for a read that cannot be a frame (see
/// `adjust_for_border`).
fn bounded_insets(l: i32, t: i32, r: i32, b: i32, max: i32) -> Option<(i32, i32, i32, i32)> {
    let ok = |v: i32| (0..=max).contains(&v);
    (ok(l) && ok(t) && ok(r) && ok(b)).then_some((l, t, r, b))
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
fn distribute_workspaces(
    monitors: &mut [Monitor],
    primary: usize,
    total: usize,
    per_monitor: bool,
) {
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
            // Carry the focus too when workspace 0 has none, or the folded
            // windows arrive with focus pointing at whatever happened to be
            // first (review B-14). `splits` is deliberately NOT carried: those
            // ratios describe a different set of tiled windows and would place
            // the merged set wrongly.
            if m.workspaces[0].focused == 0 {
                m.workspaces[0].focused = extra.focused;
            }
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
            // Physical px on THIS monitor. Must match what ensure_bars actually
            // places, or every tile on a scaled screen is offset by the
            // difference — hence the shared helper.
            let reserved = bar_reserved_px(
                cfg.bar_height,
                cfg.bar_floating,
                cfg.bar_margin,
                monitor_dpi(m.hmon),
            );
            if cfg.bar_bottom {
                m.work_area.bottom -= reserved;
            } else {
                m.work_area.top += reserved;
            }
        }
    }
}

/// Vertical space one bar occupies on a monitor of `dpi`, in physical pixels.
/// The single source of truth shared by `reserve_bar` (what tiling leaves free)
/// and `ensure_bars` (where the window is actually put).
fn bar_reserved_px(height_logical: i32, floating: bool, margin_logical: i32, dpi: u32) -> i32 {
    dpi_px(height_logical, dpi)
        + if floating {
            dpi_px(margin_logical, dpi) * 2
        } else {
            0
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
/// Spawned from a short-lived thread: CreateProcess used to block the manager
/// (an estimated 2-10 ms per launch, not measured) on the thread every command
/// waits for. The caller still sets `pending_launch_mon` first.
fn launch(cmd: &str) {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return;
    }
    let owned = cmd.to_string();
    let spawned = std::thread::Builder::new()
        .name("launch".to_string())
        .spawn(move || launch_now(&owned));
    if spawned.is_err() {
        launch_now(cmd);
    }
}

/// `launch`'s body. CREATE_NO_WINDOW: Astur has no console in release, so
/// cmd.exe used to open a console window of its own for the moment `start`
/// ran, which could flash and be adopted then retiled away. `start` still
/// gives a console target (cmd, pwsh) its own new console window.
fn launch_now(cmd: &str) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    if let Err(e) = std::process::Command::new("cmd")
        .args(["/C", "start", "", cmd])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    {
        log_error!("launch failed ({cmd}): {e}");
    }
}

fn queue_wallpaper(path: &str) {
    let path = path.trim();
    if path.is_empty() || *WALLPAPER_LAST.lock().unwrap() == path {
        return;
    }
    *WALLPAPER_REQ.lock().unwrap() = Some(path.to_string());
    WALLPAPER_CV.notify_one();
}

fn queue_workspace_wallpaper(mgr: &Manager, mi: usize, wi: usize) {
    let index = if mgr.cfg.per_monitor {
        wi
    } else {
        mgr.ml_to_global(mi, wi)
    };
    if let Some(path) = mgr.cfg.workspace_wallpapers.get(index) {
        queue_wallpaper(path);
    }
}

fn wallpaper_worker() {
    loop {
        let path = {
            let mut slot = WALLPAPER_REQ.lock().unwrap();
            loop {
                if let Some(path) = slot.take() {
                    break path;
                }
                slot = WALLPAPER_CV.wait(slot).unwrap();
            }
        };
        let mut wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let applied = unsafe {
            SystemParametersInfoW(
                SPI_SETDESKWALLPAPER,
                0,
                Some(wide.as_mut_ptr() as *mut c_void),
                SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
            )
            .is_ok()
        };
        if applied {
            *WALLPAPER_LAST.lock().unwrap() = path;
            // Per-workspace wallpapers land here on nearly every switch; the
            // cached crops show the previous one from now on.
            wp_invalidate();
        }
    }
}

fn hex_encode(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn hex_decode(value: &str) -> Option<String> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}

fn load_active_state() -> Vec<usize> {
    let path = config_path("ASTUR_STATE", "state.conf");
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .find_map(|line| line.strip_prefix("active="))
        .map(|values| {
            values
                .split(',')
                .filter_map(|value| value.trim().parse::<usize>().ok())
                .collect()
        })
        .unwrap_or_default()
}

fn queue_manager_state(mgr: &Manager) {
    if !mgr.cfg.persist_state {
        return;
    }
    let active = mgr
        .monitors
        .iter()
        .map(|monitor| monitor.active.to_string())
        .collect::<Vec<_>>()
        .join(",");
    *STATE_REQ.lock().unwrap() = Some(format!("version=1\nactive={active}\n"));
    STATE_CV.notify_one();
}

fn state_worker() {
    loop {
        let text = {
            let mut slot = STATE_REQ.lock().unwrap();
            loop {
                if let Some(text) = slot.take() {
                    break text;
                }
                slot = STATE_CV.wait(slot).unwrap();
            }
        };
        let path = config_path("ASTUR_STATE", "state.conf");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, text);
    }
}

fn load_launcher_mru() {
    let path = config_path("ASTUR_MRU", "launcher-mru.conf");
    let mut map = HashMap::new();
    let mut max_tick = 0u64;
    if let Ok(text) = std::fs::read_to_string(path) {
        for line in text.lines() {
            let Some((tick, key)) = line.split_once('|') else {
                continue;
            };
            let Some(key) = hex_decode(key) else { continue };
            let Ok(tick) = tick.parse::<u64>() else {
                continue;
            };
            max_tick = max_tick.max(tick);
            map.insert(key, tick);
        }
    }
    MRU_TICK.store(max_tick, Ordering::Relaxed);
    *LAUNCHER_MRU.lock().unwrap() = Some(map);
}

fn touch_window_mru(hwnd: isize) {
    let mut order = WINDOW_MRU.lock().unwrap();
    order.retain(|item| *item != hwnd);
    order.push_front(hwnd);
    order.truncate(100);
}

fn launcher_mru_score(key: &str) -> i32 {
    let tick = LAUNCHER_MRU
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|map| map.get(key))
        .copied()
        .unwrap_or(0);
    if tick == 0 {
        0
    } else {
        let age = MRU_TICK.load(Ordering::Relaxed).saturating_sub(tick);
        (30i32 - age.min(25) as i32).max(5)
    }
}

fn launcher_mru_bump(key: &str) {
    let cfg = UI_CFG
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(Config::defaults);
    if !cfg.launcher_mru {
        return;
    }
    let tick = MRU_TICK.fetch_add(1, Ordering::Relaxed) + 1;
    let text = {
        let mut state = LAUNCHER_MRU.lock().unwrap();
        let map = state.get_or_insert_with(HashMap::new);
        map.insert(key.to_string(), tick);
        if !cfg.persist_state {
            return;
        }
        let mut rows: Vec<(u64, String)> = map
            .iter()
            .map(|(key, tick)| (*tick, hex_encode(key)))
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.0));
        rows.truncate(200);
        rows.into_iter()
            .map(|(tick, key)| format!("{tick}|{key}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    };
    *MRU_REQ.lock().unwrap() = Some(text);
    MRU_CV.notify_one();
}

fn mru_worker() {
    loop {
        let text = {
            let mut slot = MRU_REQ.lock().unwrap();
            loop {
                if let Some(text) = slot.take() {
                    break text;
                }
                slot = MRU_CV.wait(slot).unwrap();
            }
        };
        let path = config_path("ASTUR_MRU", "launcher-mru.conf");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, text);
    }
}
/// Is this process elevated (admin token)? False if the token can't be read.
unsafe fn process_elevated() -> bool {
    token_elevated(GetCurrentProcess()).unwrap_or(false)
}

/// Whether `process`'s token is elevated. None if the token can't be read.
unsafe fn token_elevated(process: HANDLE) -> Option<bool> {
    let mut token = HANDLE::default();
    if OpenProcessToken(process, TOKEN_QUERY, &mut token).is_err() {
        return None;
    }
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut len = 0u32;
    let ok = GetTokenInformation(
        token,
        TokenElevation,
        Some(&mut elevation as *mut TOKEN_ELEVATION as *mut c_void),
        core::mem::size_of::<TOKEN_ELEVATION>() as u32,
        &mut len,
    )
    .is_ok();
    let _ = CloseHandle(token);
    ok.then_some(elevation.TokenIsElevated != 0)
}

/// A security descriptor granting full access to the current user's SID and
/// nobody else, for the IPC pipe. Leaked deliberately: it lives for the process
/// lifetime and is handed to CreateNamedPipeW on every accept loop iteration.
/// Returns None if anything fails, in which case the caller falls back to the
/// default DACL (which is what shipped before).
unsafe fn owner_only_security_descriptor() -> Option<*mut c_void> {
    static SD: OnceLock<usize> = OnceLock::new();
    let value = *SD.get_or_init(|| {
        // Own SID, as a string, straight from the process token.
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return 0;
        }
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let ok = len > 0
            && GetTokenInformation(
                token,
                TokenUser,
                Some(buf.as_mut_ptr() as *mut c_void),
                len,
                &mut len,
            )
            .is_ok();
        let sid_string = ok
            .then(|| {
                let user = &*(buf.as_ptr() as *const TOKEN_USER);
                let mut out = windows::core::PWSTR::null();
                ConvertSidToStringSidW(user.User.Sid, &mut out)
                    .ok()
                    .map(|_| {
                        let s = out.to_string().unwrap_or_default();
                        let _ = LocalFree(HLOCAL(out.0 as *mut c_void));
                        s
                    })
            })
            .flatten();
        let _ = CloseHandle(token);
        let Some(sid) = sid_string.filter(|s| !s.is_empty()) else {
            return 0;
        };
        // D: = DACL, A = allow, GA = generic all, for that SID only.
        let sddl: Vec<u16> = format!("D:(A;;GA;;;{sid})")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut psd = PSECURITY_DESCRIPTOR::default();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut psd,
            None,
        )
        .is_err()
        {
            return 0;
        }
        psd.0 as usize
    });
    (value != 0).then_some(value as *mut c_void)
}

unsafe fn ipc_dispatch(line: &str) -> String {
    let line = line.trim();
    let mut parts = line.split_whitespace();
    let command = parts.next().unwrap_or("").to_ascii_lowercase();
    let argument = parts.collect::<Vec<_>>().join(" ");
    let one_based = || {
        argument
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
    };
    match command.as_str() {
        "switch" => match one_based() {
            Some(index) => push_cmd(Cmd::Switch(index)),
            None => return "error expected: switch <workspace>\n".to_string(),
        },
        "move" => match one_based() {
            Some(index) => push_cmd(Cmd::MoveToWs(index)),
            None => return "error expected: move <workspace>\n".to_string(),
        },
        "focus_next" => push_cmd(Cmd::FocusDir(1)),
        "focus_prev" => push_cmd(Cmd::FocusDir(-1)),
        "toggle_tiling" => push_cmd(Cmd::ToggleTiling),
        "toggle_float" => push_cmd(Cmd::ToggleFloat),
        "close" => push_cmd(Cmd::CloseFocused),
        "layout"
            if matches!(
                argument.as_str(),
                "dwindle" | "master" | "columns" | "grid" | "monocle"
            ) =>
        {
            push_cmd(Cmd::SetLayout(argument));
        }
        "scratchpad" => push_cmd(Cmd::ToggleScratchpad),
        "terminal" => push_cmd(Cmd::LaunchTerminal),
        "browser" => push_cmd(Cmd::LaunchBrowser),
        "launcher" => open_launcher_popup(),
        "system_menu" => open_system_popup(),
        "reload" => reload_config_now(),
        // Arbitrary exec is opt-in. Astur can otherwise be used as a
        // convenient parent process by anything already running as the user
        // (review S-02). Window-management verbs above are always available.
        "launch" if !argument.is_empty() => {
            if !UI_CFG
                .lock()
                .unwrap()
                .as_ref()
                .map(|c| c.ipc_allow_launch)
                .unwrap_or(false)
            {
                log_error!("IPC launch refused (ipc_allow_launch = false): {argument}");
                return "error launch is disabled (set ipc_allow_launch = true)
"
                .to_string();
            }
            log_info!("IPC launch: {argument}");
            launch(&argument);
        }
        "status" => {
            return format!(
                "ok windows={} launcher={} system_menu={}\n",
                MANAGED.lock().unwrap().len(),
                LAUNCHER_OPEN.load(Ordering::Relaxed),
                SYSMENU_OPEN.load(Ordering::Relaxed)
            );
        }
        "help" => {
            return "ok switch move focus_next focus_prev toggle_tiling toggle_float close layout scratchpad terminal browser launcher system_menu reload launch status\n".to_string();
        }
        _ => return "error unknown command; send help\n".to_string(),
    }
    "ok\n".to_string()
}

fn ipc_worker() {
    loop {
        // Cheap check first. IPC is off by default, and this used to deep-clone
        // the whole Config every 500 ms forever just to read one bool — pure
        // waste on an idle desktop (review P-07).
        let enabled = UI_CFG
            .lock()
            .unwrap()
            .as_ref()
            .map(|c| c.ipc_enabled)
            .unwrap_or(false);
        if !enabled {
            // Sleep on the reload condvar instead of polling: a config change
            // wakes us immediately, and an idle desktop costs nothing at all.
            let guard = IPC_WAKE.0.lock().unwrap();
            let _ = IPC_WAKE
                .1
                .wait_timeout(guard, std::time::Duration::from_secs(30));
            continue;
        }
        let cfg = UI_CFG
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(Config::defaults);
        let clean: String = cfg
            .ipc_pipe
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
            .collect();
        let name = if clean.is_empty() {
            "astur"
        } else {
            clean.as_str()
        };
        let path = format!(r"\\.\pipe\{name}");
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            // Explicit DACL: only this user account. The default (NULL) SD is
            // already restrictive in practice, but "in practice" is not a
            // security property — say what is allowed (review S-02).
            let sd = owner_only_security_descriptor();
            let sa = sd.map(|sd| SECURITY_ATTRIBUTES {
                nLength: core::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd,
                bInheritHandle: BOOL(0),
            });
            let pipe = CreateNamedPipeW(
                PCWSTR(wide.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                4096,
                4096,
                0,
                sa.as_ref().map(|sa| sa as *const SECURITY_ATTRIBUTES),
            );
            if pipe == INVALID_HANDLE_VALUE {
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
            let _ = ConnectNamedPipe(pipe, None);
            let mut buffer = [0u8; 4096];
            let mut read = 0u32;
            if ReadFile(pipe, Some(&mut buffer), Some(&mut read), None).is_ok() && read > 0 {
                let input = String::from_utf8_lossy(&buffer[..read as usize]);
                let mut output = String::new();
                for line in input.lines() {
                    output.push_str(&ipc_dispatch(line));
                }
                let mut written = 0u32;
                let _ = WriteFile(pipe, Some(output.as_bytes()), Some(&mut written), None);
            }
            let _ = DisconnectNamedPipe(pipe);
            let _ = CloseHandle(pipe);
        }
    }
}
/// Reveal every tracked window (so nothing is left hidden on another workspace)
/// and undo Astur's styling — but leave every window exactly where it is, so
/// quitting doesn't disturb the current layout.
/// Reveal + un-style a specific list of window handles. Takes the list by ref so
/// callers control how they acquire it (the panic path must not re-lock a mutex
/// it may already hold — see `restore_on_panic`).
/// Undo Astur's per-window styling: full opacity, default border, and — the bit
/// that used to be missed — REMOVE the `WS_EX_LAYERED` bit we added.
///
/// Setting alpha back to 255 is not enough. A layered window stays layered for
/// the rest of its life, on a separate composition path, even after Astur exits
/// (review B-16). `unfocused_opacity` defaults to 0.8, so this applied to every
/// window Astur ever dimmed.
unsafe fn unstyle_window(hwnd: HWND) {
    if !IsWindow(hwnd).as_bool() {
        return;
    }
    let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if ex & WS_EX_LAYERED.0 != 0 {
        SetWindowLongW(hwnd, GWL_EXSTYLE, (ex & !WS_EX_LAYERED.0) as i32);
    }
    let def: u32 = 0xFFFFFFFF; // DWMWA_COLOR_DEFAULT
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_BORDER_COLOR,
        &def as *const _ as *const c_void,
        core::mem::size_of::<u32>() as u32,
    );
}

unsafe fn restore_windows(list: &[isize]) {
    SUPPRESS.store(true, Ordering::Relaxed);
    for &h in list {
        let hwnd = hwnd_from(h);
        if !IsWindow(hwnd).as_bool() {
            continue;
        }
        unmark_hidden_by_us(h);
        let _ = ShowWindow(hwnd, SW_SHOW);
        // Undo dimming, the layered style and the border. Positions untouched.
        unstyle_window(hwnd);
    }
    SUPPRESS.store(false, Ordering::Relaxed);
}

unsafe fn restore_all_windows() {
    let list = MANAGED.lock().unwrap().clone();
    restore_windows(&list);
    // Everything is visible again — nothing left for the crash-rescue pass.
    let _ = std::fs::remove_file(rescue_file());
    // Every graceful exit route funnels through here (tray Quit, Ctrl+C, End
    // task, logoff, the panic hook), so it is also where the system-wide
    // foreground-lock setting goes back to what the user had.
    restore_foreground_lock();
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
    let rects = match mgr.cfg.layout.as_str() {
        "master" => master_stack(
            mon.work_area,
            n,
            mgr.cfg.master_ratio,
            mgr.cfg.outer_gap,
            mgr.cfg.inner_gap,
        ),
        "columns" => columns_layout(mon.work_area, n, mgr.cfg.outer_gap, mgr.cfg.inner_gap),
        "grid" => grid_layout(mon.work_area, n, mgr.cfg.outer_gap, mgr.cfg.inner_gap),
        "monocle" => monocle_layout(mon.work_area, n, mgr.cfg.outer_gap),
        _ => dwindle_layout(
            mon.work_area,
            n,
            mgr.cfg.outer_gap,
            mgr.cfg.inner_gap,
            &ws.splits,
        ),
    };
    if rects.len() < n {
        return Vec::new();
    }
    tiled.into_iter().zip(rects).collect()
}

/// Tile a single monitor's active workspace on that monitor's work area,
/// animating windows to their targets (glide) when animations are on.
unsafe fn retile_monitor(mgr: &Manager, mi: usize) {
    retile_monitor_opts(mgr, mi, false);
}

/// `retile_monitor`, with the glide ruled out by the caller. A drop decides
/// glide or instant once, up front (`drop_retile_force_instant`), and passes
/// that decision here: re-reading GLIDE_BUSY instead could see the worker go
/// idle in between and glide a window from a rect it is not at.
unsafe fn retile_monitor_opts(mgr: &Manager, mi: usize, force_instant: bool) {
    if !mgr.tiling {
        return;
    }
    let rects = workspace_layout(mgr, mi, mgr.monitors.get(mi).map(|m| m.active).unwrap_or(0));
    if rects.is_empty() {
        return;
    }
    let mut probe = Probe::start("retile");
    probe.note(format_args!("mon={mi} windows={}", rects.len()));

    // Glide path: animate windows from their current position to the new tile
    // slot via a cosmetic overlay (the real placement is still instant, done
    // underneath). Only when enabled, idle, and the layout actually changed —
    // a no-op retile (e.g. refocus) must not raise an overlay.
    let want_glide =
        !force_instant && glide_enabled(&mgr.cfg) && !GLIDE_BUSY.load(Ordering::Relaxed);
    // The glide composes over the capture thread's wallpaper crop and bails
    // to instant without a current one. Check that lock-free BEFORE paying the
    // ~18 ms capture_monitor and the worker round trip for nothing: no Explorer,
    // a replacement shell, the warm-up not done yet, or a wallpaper that just
    // changed and is still being re-rendered.
    let wp_ok = want_glide && wp_ready();
    if want_glide && !wp_ok {
        probe.note(format_args!("wp_not_ready=instant"));
        // A failed render has no pending retry of its own; this is it
        // (at most once per WP_TTL, never while Explorer is known absent).
        wp_ttl_hint();
    }
    if wp_ok {
        let full = mgr.monitors[mi].work_area;
        let mut items = Vec::with_capacity(rects.len());
        let mut changed = false;
        let mut ok = true;
        for (h, target) in &rects {
            let hwnd = hwnd_from(*h);
            let mut cur = RECT::default();
            // A parked window (a drag's DragPark, or a drop whose posted
            // commit has not landed yet) is not in the capture: its glide item
            // would sample off-bitmap and the window would vanish for the
            // whole glide. Place instantly instead.
            if GetWindowRect(hwnd, &mut cur).is_err() || rect_parked(&cur) {
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
            if !glide_still(&old, &new) {
                changed = true;
            }
            items.push(GlideItem { old, new });
        }
        // Capture and cover only what moves (glide_damage), grown by the DWM
        // shadow reach at this monitor's scale.
        let hmon = mgr.monitors[mi].hmon;
        let damage = if ok && changed {
            let margin = dpi_px(GLIDE_SHADOW_PX, monitor_dpi(hmon));
            glide_damage(
                &items,
                margin,
                full.right - full.left,
                full.bottom - full.top,
            )
        } else {
            None
        };
        if let Some(d) = damage {
            let area = RECT {
                left: full.left + d.left,
                top: full.top + d.top,
                right: full.left + d.right,
                bottom: full.top + d.bottom,
            };
            // Into `area` coordinates, the capture's. An item entirely outside
            // it is a live window that stays (<= GLIDE_STILL_PX), not drawn;
            // one crossing its edge is drawn 1:1 from the capture, which
            // matches the live part outside.
            items.retain_mut(|it| {
                if !rects_overlap(&it.old, &d) && !rects_overlap(&it.new, &d) {
                    return false;
                }
                for r in [&mut it.old, &mut it.new] {
                    r.left -= d.left;
                    r.right -= d.left;
                    r.top -= d.top;
                    r.bottom -= d.top;
                }
                true
            });
            probe.note(format_args!(
                "damage={}x{} of {}x{}",
                d.right - d.left,
                d.bottom - d.top,
                full.right - full.left,
                full.bottom - full.top
            ));
            let out = capture_monitor(area);
            probe.mark("capture");
            if out != 0 {
                GLIDE_HMON.store(hmon, Ordering::Relaxed);
                GLIDE_BUSY.store(true, Ordering::Relaxed);
                // `out` is selected on the glide thread next: flush this
                // thread's GDI batch before the hand-off.
                let _ = GdiFlush();
                dispatch_glide(GlideReq {
                    out_bmp: out,
                    hmon,
                    rect: full,
                    area,
                    items,
                    dur_ms: mgr.cfg.animation_ms.max(1) as u64,
                    queued: probe_now(),
                    ex_style: overlay_ex_style(mgr.cfg.animation_ms),
                });
                // Wait until the overlay covers the monitor, then place the real
                // windows underneath it (hidden), exactly like the workspace slide.
                let up = wait_glide_overlay_up();
                probe.mark(if up { "glide_up" } else { "glide_TIMEOUT" });
                place_tiles(rects, &mut probe);
                return;
            }
        }
    }

    // Instant path (glide off, busy, capture failed, or nothing moved).
    place_tiles(rects, &mut probe);
}

/// The placement loop of `retile_monitor`: every window straight to its tile
/// under SUPPRESS. With probes on it also times each window, because one slow
/// app's synchronous SetWindowPos is what stretches a whole retile.
unsafe fn place_tiles(rects: Vec<(isize, RECT)>, probe: &mut Probe) {
    let timed = probe.on();
    let mut slowest = (0u128, 0isize);
    SUPPRESS.store(true, Ordering::Relaxed);
    for (h, target) in rects {
        let t = timed.then(Instant::now);
        animate_to(hwnd_from(h), target);
        if let Some(t) = t {
            slowest = slowest.max((t.elapsed().as_micros(), h));
        }
    }
    SUPPRESS.store(false, Ordering::Relaxed);
    probe.mark("placed");
    probe.note(format_args!("slowest={}us@{:#x}", slowest.0, slowest.1));
}

/// Is the window glide configured on? (Whether one can run right now also
/// needs the worker idle and a current wallpaper crop.)
fn glide_enabled(cfg: &Config) -> bool {
    cfg.animations && cfg.animation_ms > 0 && cfg.window_anim == "glide"
}

/// Off-screen where Cmd::DragPark puts a window (-32000, which is also where
/// Windows keeps minimised ones). No monitor sits that far out.
fn rect_parked(r: &RECT) -> bool {
    r.left <= -30000
}

/// Has a drop's own posted commit landed? `before` is the window's rect read
/// just before the commit was posted, `live` its rect now, `committed` the rect
/// asked for. Landed = at the committed rect, or moved off `before` to anywhere
/// but the park (the app may round or constrain the rect; a park posted
/// earlier and still pending must not count as the commit).
fn swp_landed(before: RECT, live: RECT, committed: RECT) -> bool {
    !rect_parked(&live) && (live == committed || live != before)
}

/// How a tiled drop's retile may run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DropRetile {
    /// The glide may run: the window is on screen at its dropped rect, so the
    /// capture and the glide's start rect agree.
    Glide,
    /// Place instantly: the glide was off or busy when decided, or the window
    /// is still parked or not yet landed.
    Instant,
}

fn drop_glide_plan(glide_wanted: bool, landed: bool) -> DropRetile {
    if glide_wanted && landed {
        DropRetile::Glide
    } else {
        DropRetile::Instant
    }
}

/// Could a glide start right now? Configured on, the worker idle, and a
/// current wallpaper crop. Only the manager ever sets GLIDE_BUSY, so a true
/// here stays true until this thread dispatches something itself.
fn glide_can_run(cfg: &Config) -> bool {
    glide_enabled(cfg) && !GLIDE_BUSY.load(Ordering::Relaxed) && wp_ready()
}

/// Must the retile after a drop of `h` skip the glide (TILE-1 G2)?
/// `glide_wanted` is `glide_can_run`, sampled once by the caller: re-reading
/// GLIDE_BUSY later could see the worker go idle and glide a window that is
/// not where the glide thinks. The drop's commit is posted, so a glide started
/// straight after it reads h's rect, and captures the screen, while h may still
/// be parked off-screen: h would vanish for the whole glide and pop back. So
/// wait for the commit to land (at most DROP_LAND_WAIT_MS, in 1 ms steps: only
/// a busy or hung app takes that long) and go instant if it did not. With
/// synchronous placement the commit has landed already.
unsafe fn drop_retile_force_instant(
    glide_wanted: bool,
    h: isize,
    before: RECT,
    committed: RECT,
) -> bool {
    if !glide_wanted {
        return true;
    }
    if !ASYNC_WINDOW_POS.load(Ordering::Relaxed) {
        return false;
    }
    let t0 = Instant::now();
    let deadline = std::time::Duration::from_millis(DROP_LAND_WAIT_MS);
    let landed = loop {
        if swp_landed(before, window_rect_of(h), committed) {
            break true;
        }
        if t0.elapsed() >= deadline {
            break false;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    let plan = drop_glide_plan(glide_wanted, landed);
    log_debug!(
        "drop {h:#x}: commit landed={landed} after {}us -> {plan:?}",
        t0.elapsed().as_micros()
    );
    plan == DropRetile::Instant
}

/// Longest a drop waits for its own posted commit before placing instantly.
const DROP_LAND_WAIT_MS: u64 = 32;

/// GetWindowRect, or an empty rect when the window is gone.
unsafe fn window_rect_of(h: isize) -> RECT {
    let mut r = RECT::default();
    let _ = GetWindowRect(hwnd_from(h), &mut r);
    r
}

/// Place the active workspace's windows at their targets INSTANTLY (no glide).
/// Used on workspace switch: the windows were just revealed from a hidden state,
/// so gliding them from a stale position would look like a jump.
unsafe fn place_active_instant(mgr: &Manager, mi: usize) {
    if !mgr.tiling {
        return;
    }
    let rects = workspace_layout(mgr, mi, mgr.monitors.get(mi).map(|m| m.active).unwrap_or(0));
    // Debug only: a window that did not land where it was put. Windows are
    // also laid out while hidden (place_hidden_workspace), and a placement
    // that silently failed there would otherwise only show as a wrong tile.
    // Synchronous placement only: a posted one has not landed yet when read
    // back here, so every window would be reported.
    let check = log_on(LOG_DEBUG) && !ASYNC_WINDOW_POS.load(Ordering::Relaxed);
    SUPPRESS.store(true, Ordering::Relaxed);
    for (h, target) in rects {
        let hwnd = hwnd_from(h);
        if IsIconic(hwnd).as_bool() || IsZoomed(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let to = adjust_for_border(hwnd, target);
        set_pos_raw(h, to);
        let mut got = RECT::default();
        if check && GetWindowRect(hwnd, &mut got).is_ok() && got != to {
            log_debug!(
                "placed {h:#x} at {},{} {}x{}, wanted {},{} {}x{}",
                got.left,
                got.top,
                got.right - got.left,
                got.bottom - got.top,
                to.left,
                to.top,
                to.right - to.left,
                to.bottom - to.top
            );
        }
    }
    SUPPRESS.store(false, Ordering::Relaxed);
}

/// Must Cmd::MoveToWs retile the source workspace itself? Only when the
/// destination is on another monitor: then the source stays on screen. On the
/// same monitor the follow-switch hides it, and `place_hidden_workspace` lays
/// it out afterwards, unseen.
fn move_needs_source_retile(to_mi: usize, from_mi: usize) -> bool {
    to_mi != from_mi
}

/// Lay out a HIDDEN workspace's tiled windows at their slots, so its next
/// reveal (place_active_instant) finds them already there. No glide, no
/// SUPPRESS: a SetWindowPos without SWP_SHOWWINDOW on a hidden window raises
/// no show or hide event. The border correction reads DWM frame bounds, which
/// stay valid while hidden (measured: 12/12 windows, WINQUERY-18). Maximised
/// windows are skipped, since SW_RESTORE would show them; the reveal fixes
/// those. Minimised ones are not in the layout at all.
unsafe fn place_hidden_workspace(mgr: &Manager, mi: usize, wi: usize) {
    if !mgr.tiling || mgr.monitors.get(mi).is_none_or(|m| m.active == wi) {
        return;
    }
    for (h, target) in workspace_layout(mgr, mi, wi) {
        let hwnd = hwnd_from(h);
        if IsZoomed(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            continue;
        }
        set_pos_raw(h, adjust_for_border(hwnd, target));
    }
}

/// Tile every monitor's active workspace.
unsafe fn retile_all(mgr: &Manager) {
    for mi in 0..mgr.monitors.len() {
        retile_monitor(mgr, mi);
    }
}

/// Does a layered window's alpha still need setting? `read_ok`, `flags` and
/// `cur` are what GetLayeredWindowAttributes returned. Only a successful read
/// of exactly LWA_ALPHA at the target value is a skip: a failed read (never set
/// through SLWA, or UpdateLayeredWindow), a colour key, or another value is set
/// as before.
fn alpha_set_needed(read_ok: bool, flags: u32, cur: u8, target: u8) -> bool {
    !read_ok || flags != LWA_ALPHA.0 || cur != target
}

/// Apply opacity + border colour to a single window based on focus state.
unsafe fn style_window(hwnd: HWND, focused: bool, cfg: &Config) {
    if cfg.unfocused_opacity < 0.999 {
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let was_layered = ex & WS_EX_LAYERED.0 != 0;
        if !was_layered {
            SetWindowLongW(hwnd, GWL_EXSTYLE, (ex | WS_EX_LAYERED.0) as i32);
        }
        let alpha = if focused {
            255
        } else {
            (cfg.unfocused_opacity * 255.0) as u8
        };
        // Skip a SetLayeredWindowAttributes that would re-send the alpha the
        // window already has (SWITCH-18): every switch restyles each incoming
        // window, and hidden windows keep their alpha, so nearly all of those
        // are repeats. SLWA measured ~250 us per call (0.7-1.3 ms under load),
        // on the manager thread before focus lands. The read is the live
        // value, not a cache, so an app that changed its own opacity, or a
        // cancelled shutdown that un-styled everything, is still corrected. A
        // freshly-layered window has no alpha to read and is always set.
        let needed = !was_layered || {
            let mut cur = 0u8;
            let mut flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS(0);
            let ok = GetLayeredWindowAttributes(
                hwnd,
                None,
                Some(&mut cur as *mut u8),
                Some(&mut flags as *mut _),
            )
            .is_ok();
            alpha_set_needed(ok, flags.0, cur, alpha)
        };
        if needed {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA);
        }
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

/// May a Cmd::FocusMouse act now, given the settle deadline?
fn focus_mouse_allowed(now: u64, settle_deadline: u64) -> bool {
    now >= settle_deadline
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

/// Warp the mouse cursor to the centre of a window. A tiled window is centred
/// on the tile it was just given, not on its live rect: placement is posted
/// (TILE-1), so straight after a retile the live rect can still be the old one,
/// or a neighbour can still sit under the new centre. Floating and untiled
/// windows use their live rect.
unsafe fn center_cursor_on(mgr: &Manager, h: isize) {
    let mut r = RECT::default();
    let r = match tile_target(mgr, h) {
        Some(t) => t,
        None if GetWindowRect(hwnd_from(h), &mut r).is_ok() => r,
        None => return,
    };
    let _ = SetCursorPos((r.left + r.right) / 2, (r.top + r.bottom) / 2);
}

/// The tile a retile of h's monitor puts h in (raw layout rect), or None when
/// that retile would not place it: tiling off, untracked, on a hidden
/// workspace, floating, minimised, dead, or a layout that came up short. The
/// same `workspace_layout` the retile itself runs.
unsafe fn tile_target(mgr: &Manager, h: isize) -> Option<RECT> {
    if !mgr.tiling {
        return None;
    }
    let (mi, wi) = mgr
        .locate(h)
        .filter(|&(mi, wi)| wi == mgr.monitors[mi].active)?;
    workspace_layout(mgr, mi, wi)
        .into_iter()
        .find(|&(w, _)| w == h)
        .map(|(_, r)| r)
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
    // AttachThreadInput joins two threads' input queues and can block when the
    // other thread is not pumping messages — with no timeout. This runs on the
    // manager thread, which owns ALL window state, so one hung app would stall
    // every workspace switch, hotkey and retile behind it (review B-12).
    // Skipping the attach costs at worst a focus that does not take; blocking
    // costs the whole WM.
    let took = if !fg.0.is_null() && IsHungAppWindow(fg).as_bool() {
        log_error!(
            "skipped focus attach: foreground window {:#x} is not responding",
            fg.0 as isize
        );
        let took = SetForegroundWindow(hwnd);
        let _ = BringWindowToTop(hwnd);
        took
    } else if fgt != 0 && fgt != cur {
        let _ = AttachThreadInput(cur, fgt, BOOL(1));
        let took = SetForegroundWindow(hwnd);
        let _ = BringWindowToTop(hwnd);
        let _ = AttachThreadInput(cur, fgt, BOOL(0));
        took
    } else {
        let took = SetForegroundWindow(hwnd);
        let _ = BringWindowToTop(hwnd);
        took
    };
    if !took.as_bool() {
        let n = FOCUS_REFUSED.fetch_add(1, Ordering::Relaxed) + 1;
        if focus_fail_log_due() {
            log_error!("SetForegroundWindow refused {h:#x} (refused={n} since start)");
        }
    }
}

/// Focus that did not land. The workspace reveal no longer activates each
/// window it shows (SW_SHOWNA, SWITCH-4), so focus_window is the only
/// activation on a switch: if it fails, keystrokes go to a window that is
/// now hidden, with nothing on screen to say so. Counted always (diagnostics
/// prints them); logged at most once per FOCUS_FAIL_LOG_GAP_MS, since a
/// foreground lock can refuse every call.
static FOCUS_REFUSED: AtomicU64 = AtomicU64::new(0);
static FOCUS_MISSED: AtomicU64 = AtomicU64::new(0);
static FOCUS_FAIL_LOGGED_MS: AtomicU64 = AtomicU64::new(0);
const FOCUS_FAIL_LOG_GAP_MS: u64 = 30_000;

fn focus_fail_log_due() -> bool {
    let now = now_ms().max(1);
    let last = FOCUS_FAIL_LOGGED_MS.load(Ordering::Relaxed);
    if last != 0 && now < last + FOCUS_FAIL_LOG_GAP_MS {
        return false;
    }
    FOCUS_FAIL_LOGGED_MS.store(now, Ordering::Relaxed);
    true
}

/// After a switch's focus_window(f): count (and rate-limit log) a foreground
/// that ended up on some other app. Foreground changes land asynchronously on
/// the target's thread, so "no foreground yet" and "another window of f's own
/// thread" (its previous active window, before it processes the activation)
/// are in flight, not failures.
unsafe fn check_focus_landed(f: isize) {
    let fg = GetForegroundWindow();
    if fg.0.is_null() || GetAncestor(fg, GA_ROOTOWNER) == hwnd_from(f) {
        return;
    }
    if GetWindowThreadProcessId(fg, None) == GetWindowThreadProcessId(hwnd_from(f), None) {
        return;
    }
    let n = FOCUS_MISSED.fetch_add(1, Ordering::Relaxed) + 1;
    if focus_fail_log_due() {
        log_error!(
            "switch focus did not land: foreground is {:#x}, not {f:#x} (missed={n} since start)",
            fg.0 as isize
        );
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
        let hwnd = hwnd_from(h);
        let mi = monitor_index_for_window(mgr, hwnd);
        let a = mgr.monitors[mi].active;
        mgr.monitors[mi].workspaces[a].windows.push(h);
        if should_float(hwnd, match_window_rule(hwnd)) {
            mgr.monitors[mi].workspaces[a].floating.push(h);
        }
        mgr.monitors[mi].workspaces[a].focused = h;
    }
}

/// A cosmetic workspace-slide request handed from the manager to the transition
/// thread. The manager has already performed the real (instant) switch; this is
/// purely a visual overlay, so losing or dropping it never affects windows.
struct SlideReq {
    // Frozen outgoing workspace, handed over uncopied: the worker hands it back
    // (SNAP_RETURNS) as `old_ws`'s snapshot when done with it.
    out_bmp: Bmp,
    // Frozen incoming workspace (worker frees); None = first visit, no
    // snapshot: the worker holds the outgoing frame, then reveals.
    in_bmp: Option<Bmp>,
    out_rects: Vec<RECT>, // work-area-local rects of the outgoing windows
    in_rects: Vec<RECT>,  // work-area-local rects of the incoming windows
    hmon: isize,          // monitor, with `rect` the wallpaper-crop key
    old_ws: usize,        // workspace `out_bmp` shows: its snapshot key, with hmon
    rect: RECT,           // work-area rect (overlay geometry)
    dir: i32,             // +1 = new ws came from the right, -1 from the left
    dur_ms: u64,
    mode: WsAnim,              // slide / spring / fade (off never reaches the worker)
    queued: Option<Instant>,   // dispatch time, for the pickup probe (debug only)
    gen: u64,                  // SLIDE_GEN of this request (see the overlay state notes)
    ex_style: WINDOW_EX_STYLE, // overlay_ex_style of the configured animation_ms
}
static SLIDE_REQ: Mutex<Option<SlideReq>> = Mutex::new(None);
static SLIDE_CV: Condvar = Condvar::new();
// Handshake: the worker stores (its request's gen, overlay up?) once the
// overlay covers the monitor showing the outgoing image, or once it knows
// there will be none, so the manager can do the (now hidden) switch
// underneath without the destination workspace flashing first. Keyed by gen:
// a signal meant for another request never releases this one's wait.
static SLIDE_READY: Mutex<(u64, bool)> = Mutex::new((0, false));
static SLIDE_READY_CV: Condvar = Condvar::new();

// ---- slide overlay state (SWITCH-2) ----------------------------------------
// A switch that landed while the previous slide was still on the glass used to
// capture the screen (the old overlay's mid-slide frame), wait up to 250 ms for
// the one transition worker to finish that slide and raise a new overlay, then
// slide the stale frame again over a switch that had already happened: a jump
// back in time, with that frame cached as the snapshot of the workspace just
// left. Now such a switch runs UNDER the overlay already up: it "holds" it (the
// worker freezes the frame on the glass) and the worker reveals once the
// manager releases it. The real switch runs on every path; only the cosmetic
// overlay branches.
//
// Every request carries a generation `k` (SLIDE_GEN). The rules:
//   * The manager waits for k's own signal. On a timeout it marks k aborted
//     (SLIDE_ABORT) under the handshake lock, so the worker, after signalling,
//     knows whether anyone was still waiting.
//   * The worker never shows an overlay for a request that is aborted or no
//     longer the newest: shown late, it would paint the pre-switch frame over a
//     switch that already finished. A stale abort cannot touch the next gen.
//   * GLASS = k << 16 | holds: k's overlay is on the glass. The worker sets it
//     right after the signal and clears it by CAS on its own gen on every exit
//     path (GlassGuard), never "after run returns" like GLIDE_BUSY: the worker
//     picks the next request up at once, and would clear ITS flag. The manager
//     holds only by CAS from the value it read, so it can never hold an
//     overlay that is already leaving; the worker leaves only by CAS from the
//     value it last saw, so it can never leave under a hold it has not seen.
//   * SLIDE_RELEASED = the GLASS value of the hold the manager is done with,
//     stored only after switch_plain (and the styling) returned. A release for
//     another gen or an older hold never matches.
//   * No lock is held across switch_plain, and the worker caps every hold
//     (HOLD_CAP_MS): the overlay is topmost, so an unbounded hold would pin a
//     stale frame over the desktop while the manager is stuck on a hung app
//     (review B-12).
static SLIDE_GEN: AtomicU64 = AtomicU64::new(0);
static SLIDE_ABORT: AtomicU64 = AtomicU64::new(0);
static GLASS: AtomicU64 = AtomicU64::new(0);
static GLASS_HMON: AtomicIsize = AtomicIsize::new(0);
/// now_ms() at which the on-glass overlay's own animation ends.
static GLASS_END_MS: AtomicU64 = AtomicU64::new(0);
static SLIDE_RELEASED: AtomicU64 = AtomicU64::new(0);
const GLASS_HOLDS: u64 = 0xFFFF;
/// A hold the worker has seen and that is not released within this long is
/// torn down anyway (the switch underneath then finishes uncovered).
const HOLD_CAP_MS: u64 = 250;
/// The manager holds an overlay only until this long past its animation end;
/// a later switch takes a fresh overlay, so a stream of switches cannot pin
/// one. The longest legitimate on-glass time is therefore about the animation
/// + HOLD_LATE_MS + HOLD_CAP_MS + COVER_HOLD_MS (~550 ms past its end) ...
const HOLD_LATE_MS: u64 = 250;
/// ... and GLASS still set this long past the end is a bug, not a hold: log
/// it and reset, or every later switch would skip its slide for good.
const GLASS_STUCK_MS: u64 = 1_000;
/// How long a switch waits for this monitor's leaving overlay (a released
/// hold's COVER_HOLD_MS plus the teardown's DwmFlush, ~65 ms) before it
/// captures anyway and does not keep the capture.
const SLIDE_LEAVE_WAIT_MS: u64 = 100;

/// Is request `k` still worth showing? Not once the manager gave up on it
/// (`aborted` == k) or dispatched a newer one.
fn slide_gen_live(k: u64, newest: u64, aborted: u64) -> bool {
    k == newest && k != aborted
}

/// The GLASS value a manager hold moves `glass` to. None = nothing on the
/// glass (or the hold counter is full).
fn glass_hold(glass: u64) -> Option<u64> {
    (glass != 0 && glass & GLASS_HOLDS != GLASS_HOLDS).then_some(glass + 1)
}

/// Has the manager released the hold that `glass` records?
fn glass_released(glass: u64, released: u64) -> bool {
    glass & GLASS_HOLDS != 0 && released == glass
}

/// Manager: run this switch under the slide overlay already on `hmon`'s
/// glass, if there is one to hold. Some(v) = held; release `v` after the
/// switch. Lock-free, never waits.
fn slide_hold(hmon: isize) -> Option<u64> {
    let g = GLASS.load(Ordering::SeqCst);
    if g == 0 || GLASS_HMON.load(Ordering::SeqCst) != hmon {
        return None;
    }
    let late = now_ms().saturating_sub(GLASS_END_MS.load(Ordering::SeqCst));
    if late > GLASS_STUCK_MS {
        if GLASS
            .compare_exchange(g, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            log_error!("slide overlay state stuck {late} ms past its end (glass={g:#x}); reset");
        }
        return None;
    }
    if late > HOLD_LATE_MS {
        return None;
    }
    let v = glass_hold(g)?;
    GLASS
        .compare_exchange(g, v, Ordering::SeqCst, Ordering::SeqCst)
        .ok()
        .map(|_| v)
}

/// Worker: request `k`'s overlay is on the glass over `hmon`.
fn glass_on(k: u64, hmon: isize, end_ms: u64) {
    GLASS_HMON.store(hmon, Ordering::SeqCst);
    GLASS_END_MS.store(end_ms, Ordering::SeqCst);
    GLASS.store(k << 16, Ordering::SeqCst);
}

/// Worker: take the overlay off the glass, unless a hold newer than `seen`
/// started meanwhile. true = off.
fn glass_leave(seen: u64) -> bool {
    GLASS
        .compare_exchange(seen, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

/// Worker: take `k`'s overlay off the glass whatever holds it (cap hit, or
/// any exit). Leaves another gen's state alone.
fn glass_clear(k: u64) {
    loop {
        let g = GLASS.load(Ordering::SeqCst);
        if g >> 16 != k
            || GLASS
                .compare_exchange(g, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            return;
        }
    }
}

/// Clears GLASS for its gen when dropped: every exit path of run_transition.
struct GlassGuard(u64);
impl Drop for GlassGuard {
    fn drop(&mut self) {
        glass_clear(self.0);
    }
}

/// Hands the outgoing capture back to the manager when dropped: every exit
/// path of run_transition, early returns included, where it is still a valid
/// capture of the workspace just left. Every path deselects it first.
struct SnapHome(Option<SnapReturn>);
impl SnapHome {
    fn raw(&self) -> isize {
        self.0.as_ref().map_or(0, |r| r.snap.bmp.raw())
    }
    fn rects(&self) -> &[RECT] {
        self.0.as_ref().map_or(&[], |r| &r.snap.rects)
    }
}
impl Drop for SnapHome {
    fn drop(&mut self) {
        if let Some(r) = self.0.take() {
            snap_return(r);
        }
    }
}

/// Block (bounded) until the transition worker has request `k`'s overlay up
/// and covering the monitor, or has said there will be none. Returns true only
/// for k's own overlay-up; false on a no-overlay signal, and on the timeout,
/// which also marks k aborted (under the handshake lock, see the notes above)
/// so a late overlay is never shown.
fn wait_slide_overlay_up(k: u64) -> bool {
    let guard = SLIDE_READY.lock().unwrap();
    let (guard, res) = SLIDE_READY_CV
        .wait_timeout_while(guard, std::time::Duration::from_millis(250), |s| s.0 != k)
        .unwrap();
    if res.timed_out() {
        SLIDE_ABORT.store(k, Ordering::SeqCst);
        return false;
    }
    guard.1
}

/// Worker → manager: request `k`'s overlay is up (`up`), or there is none.
/// Requests run one at a time, so gens only grow; never step backwards.
fn signal_slide_overlay_up(k: u64, up: bool) {
    {
        let mut s = SLIDE_READY.lock().unwrap();
        if k >= s.0 {
            *s = (k, up);
        }
    }
    SLIDE_READY_CV.notify_one();
}

/// An owned GDI bitmap (HBITMAP), never 0: exactly one owner at a time, freed
/// exactly once, when that owner drops it. Deliberately neither Clone nor Copy,
/// so a second owner of one handle is a compile error rather than a second
/// DeleteObject: by then the handle value can already name an unrelated live
/// GDI object (a bar font or brush), which would be deleted silently.
struct Bmp(isize);

impl Bmp {
    /// Take ownership of a fresh handle; None for a failed (0) one.
    fn new(h: isize) -> Option<Bmp> {
        // Lazy `then`: `then_some(Bmp(h))` would build, and drop (delete), a
        // Bmp(0) on the failure path.
        (h != 0).then(|| Bmp(h))
    }

    /// The raw handle, to select it for a while. Ownership stays here: the
    /// caller deselects it before this moves on or drops (a selected bitmap
    /// cannot be deleted, and is never selected on two threads).
    fn raw(&self) -> isize {
        self.0
    }
}

impl Drop for Bmp {
    fn drop(&mut self) {
        bmp_delete(self.0);
    }
}

#[cfg(not(test))]
fn bmp_delete(h: isize) {
    // SAFETY: called only from Bmp::drop. Bmp is the handle's sole owner and
    // is going away, so nothing can still select, use or free it.
    unsafe {
        let _ = DeleteObject(HGDIOBJ(h as *mut c_void));
    }
}

// Tests swap the deleter for a recorder, so ownership (freed exactly once) is
// checked without GDI.
#[cfg(test)]
thread_local! {
    static BMP_FREED: std::cell::RefCell<Vec<isize>> = const { std::cell::RefCell::new(Vec::new()) };
}
#[cfg(test)]
fn bmp_delete(h: isize) {
    BMP_FREED.with(|f| f.borrow_mut().push(h));
}

/// Per-(monitor, workspace) frozen snapshot of how that workspace last looked
/// when it was left: the work-area image plus the work-area-local rects of its
/// tiled windows (so the slide can move only the windows and leave the wallpaper
/// in the gaps still). Populated for free from the outgoing capture on every
/// switch. HBITMAPs are GPU-backed DDBs (~no process RAM). The map is touched
/// only on the manager thread, and every bitmap moves by single-owner
/// hand-off, never shared (`Bmp`): the incoming image leaves the map through
/// `snap_take` and the SlideReq carries it to the worker, which frees it; the
/// outgoing capture goes to the worker uncopied (no dup_ddb: a full-frame
/// copy, ~7 ms at 1080p per the ANIM audit, on the manager's pre-switch path)
/// and comes back through SNAP_RETURNS, which the manager drains into the map.
/// Neither SNAP nor SNAP_RETURNS is ever held across a GDI or window call, and
/// bitmaps are dropped (DeleteObject) only after the lock is released.
struct Snap {
    bmp: Bmp,
    rects: Vec<RECT>,
    // Size of `bmp`: a take for a different work-area size is rejected, so a
    // wrong-size image can never reach the worker.
    w: i32,
    h: i32,
}
type SnapMap = HashMap<(isize, usize), Snap>;

#[derive(Default)]
struct SnapCache {
    map: SnapMap,
    /// Per (hmon, ws): the SLIDE_GEN of the one outgoing capture of that
    /// workspace still out with the worker that should become its snapshot.
    /// Set when the manager leaves the workspace with a keepable capture;
    /// removed when it leaves without one, or the snapshot is dropped. A
    /// returning capture is stored only if it matches, so a late or
    /// superseded one (a newer departure since, an overlay in the capture, a
    /// reload or display change: snap_clear empties this) can never land as
    /// a stale snapshot after the fact.
    newest: HashMap<(isize, usize), u64>,
}
static SNAP: Mutex<Option<SnapCache>> = Mutex::new(None);

/// An outgoing capture handed back to the manager: by the worker once its
/// slide is done with it (every exit path, see SnapHome), or by the manager
/// itself for a request that never reached the worker.
struct SnapReturn {
    key: (isize, usize),
    gen: u64,
    snap: Snap,
}
/// Worker -> manager hand-back queue. The worker only ever pushes; only the
/// manager drains it (snap_drain, snap_clear), so the map stays manager-only.
static SNAP_RETURNS: Mutex<Vec<SnapReturn>> = Mutex::new(Vec::new());

/// Remove `key`'s snapshot from `map` and return it if it is still `w` x `h`.
/// A wrong-size one is removed too and its bitmap handed to `reject` (the
/// caller drops it once the lock is released). Either way the entry is gone:
/// a snapshot is taken at most once.
fn snap_take_from(
    map: &mut SnapMap,
    key: (isize, usize),
    w: i32,
    h: i32,
    reject: impl FnOnce(Bmp),
) -> Option<Snap> {
    let s = map.remove(&key)?;
    if s.w == w && s.h == h {
        Some(s)
    } else {
        reject(s.bmp);
        None
    }
}

/// Take (hmon, ws)'s snapshot (bmp, window rects) for a slide INTO that
/// workspace: ownership moves to the caller, no copy. Only called for a
/// workspace about to become active, whose snapshot is re-created from the
/// outgoing capture when it is next left.
unsafe fn snap_take(hmon: isize, ws: usize, w: i32, h: i32) -> Option<(Bmp, Vec<RECT>)> {
    let mut wrong_size = None;
    let got = SNAP
        .lock()
        .unwrap()
        .as_mut()
        .and_then(|c| snap_take_from(&mut c.map, (hmon, ws), w, h, |b| wrong_size = Some(b)));
    drop(wrong_size); // freed after the lock is released
    got.map(|s| (s.bmp, s.rects))
}

/// Remove `key`'s snapshot from `map`, handing its bitmap to `free` (the caller
/// drops it once the lock is released). Returns whether there was one.
fn snap_remove_from(map: &mut SnapMap, key: (isize, usize), free: impl FnOnce(Bmp)) -> bool {
    match map.remove(&key) {
        Some(s) => {
            free(s.bmp);
            true
        }
        None => false,
    }
}

/// Drop (hmon, ws)'s snapshot: it no longer shows that workspace (the visit
/// changed it without a fresh capture, or the capture was of an overlay) and
/// would slide in a stale image on the next visit, which now gets the
/// first-visit cover-hold instead. Simply not storing is not enough: an older
/// entry would survive, and so would a capture of it still out with the
/// worker, which is disowned here too. Returns whether there was an entry.
unsafe fn snap_remove(hmon: isize, ws: usize) -> bool {
    let mut bmp = None;
    let had = SNAP.lock().unwrap().as_mut().is_some_and(|c| {
        c.newest.remove(&(hmon, ws));
        snap_remove_from(&mut c.map, (hmon, ws), |b| bmp = Some(b))
    });
    drop(bmp); // freed after the lock is released
    had
}

/// The manager is leaving (hmon, ws) with outgoing capture `gen`, handed to
/// the worker: that capture, once back, is ws's snapshot. Any entry stored
/// before this departure predates the visit just ending, so it goes now; an
/// older capture still out with the worker no longer matches `newest`.
unsafe fn snap_keep(hmon: isize, ws: usize, gen: u64) {
    let old = {
        // get_or_insert, not as_mut: the first switch after startup or a
        // snap_clear finds no cache yet, and must still record its capture.
        let mut guard = SNAP.lock().unwrap();
        let c = guard.get_or_insert_with(SnapCache::default);
        c.newest.insert((hmon, ws), gen);
        c.map.remove(&(hmon, ws))
    };
    drop(old); // freed after the lock is released
}

/// Store the returned captures that are still wanted (see `newest`), in
/// order, and move everything else, including any entry a store replaces, to
/// `dead`, which the caller drops once the lock is released.
fn snap_apply_returns(cache: &mut SnapCache, rets: Vec<SnapReturn>, dead: &mut Vec<Snap>) {
    for r in rets {
        if cache.newest.get(&r.key) == Some(&r.gen) {
            cache.newest.remove(&r.key);
            dead.extend(cache.map.insert(r.key, r.snap));
        } else {
            dead.push(r.snap);
        }
    }
}

/// Hand an outgoing capture back to the manager. The bitmap must already be
/// deselected; the flush then makes the GDI batch that drew it visible to the
/// thread that selects it next (GdiFlush docs, objects shared across threads).
fn snap_return(ret: SnapReturn) {
    // SAFETY: GdiFlush takes no arguments and only flushes this thread's batch.
    unsafe {
        let _ = GdiFlush();
    }
    SNAP_RETURNS.lock().unwrap().push(ret);
}

/// Manager: move the captures the worker has handed back into the map.
unsafe fn snap_drain() {
    let rets = std::mem::take(&mut *SNAP_RETURNS.lock().unwrap());
    if rets.is_empty() {
        return;
    }
    let mut dead = Vec::new();
    {
        let mut guard = SNAP.lock().unwrap();
        let cache = guard.get_or_insert_with(SnapCache::default);
        snap_apply_returns(cache, rets, &mut dead);
    }
    drop(dead); // freed after the lock is released
}

/// Drop every cached snapshot (resolution/style no longer valid), and every
/// capture handed back but not yet drained. Call on display change and config
/// reload. Captures still out with the worker no longer match `newest` (now
/// empty), so the drain after their return frees them.
unsafe fn snap_clear() {
    // Take each in its own statement so the guard drops before the frees.
    let cache = SNAP.lock().unwrap().take();
    let rets = std::mem::take(&mut *SNAP_RETURNS.lock().unwrap());
    drop(cache);
    drop(rets);
}

// =========================================================================
// Wallpaper cache: one capture thread, a private crop per compositor
// =========================================================================
// The slide and the glide fill the gaps that open up as windows move with the
// still wallpaper, rendered by PrintWindow(PW_RENDERFULLCONTENT) of Explorer's
// wallpaper window. That render costs 56-110 ms (56 ms median measured in
// review on the owner's 3-monitor machine, 96-108 ms per the ANIM audit) and
// used to run on the compositor BEFORE its overlay signal, so on every animated
// switch and retile the manager sat blocked on it (ANIM-1). It also has no
// timeout: the call is processed by Explorer's desktop thread.
//
// This reverses the old "captured fresh every slide, no cache to go stale"
// choice, and these rules are why that is safe:
//   * Only `wallpaper_capture_worker` calls `wallpaper_window` or PrintWindow.
//     Never a compositor: with its overlay up, a PrintWindow blocked on a busy
//     Explorer would pin a frozen, click-eating topmost frame over windows that
//     are already placed underneath it.
//   * WALLPAPER_GEN says which crops are current. Bump sites only fetch_add
//     (`wp_invalidate`, no GDI); a crop stamped with an older gen is NEVER
//     blitted. An older gen means a different wallpaper (per-workspace
//     wallpapers change on nearly every switch; on a sparse workspace the gaps
//     are most of the screen) or a different monitor geometry, not a sub-pixel
//     gap difference. A miss costs a flat slide or an instant glide, nothing
//     worse, and frame 0 still comes from the exact capture_monitor grab.
//   * Crops are keyed by hmon AND the exact work-area RECT, so a wrong-size
//     bitmap can never be blitted after a display or bar change.
//   * Each compositor gets its own copy through its WP_SLOTS slot (a bitmap is
//     selectable into one DC at a time, and a slide and a glide can overlap).
//     One owner at a time, and only the owner DeleteObjects: the capture thread
//     for untaken slot entries, the compositor for the copies it took.
//   * Render only at startup, WP_DEBOUNCE after a gen bump (so Explorer's own
//     wallpaper fade has finished), and after a teardown once the last try is
//     older than WP_TTL (the backstop for changes nobody announces:
//     slideshows, or WM_SETTINGCHANGE not reaching an elevated Astur). Never on
//     a timer, never while an animation runs, and not at all when no
//     configured animation uses the wallpaper.
// Memory: one DDB per monitor per compositor (~8 MB at 1080p), by design.

// One-shot guard so the wallpaper-source diagnostic prints once, not every
// render. Re-armed when Explorer restarts.
static WP_DIAG: AtomicBool = AtomicBool::new(false);

/// Which wallpaper crops are current. Starts at 1 so it never equals the
/// "nothing published yet" WP_READY_GEN of 0.
static WALLPAPER_GEN: AtomicU64 = AtomicU64::new(1);
/// Gen of the crops last published for the glide (0 = none). The manager's
/// lock-free "can a glide compose at all?" check (`wp_ready`).
static WP_READY_GEN: AtomicU64 = AtomicU64::new(0);
/// Cached wallpaper source (WorkerW or Progman); 0 = discover on next use.
static WP_SOURCE: AtomicIsize = AtomicIsize::new(0);
/// Negative cache: the last render found no wallpaper source (no Explorer, a
/// replacement shell). Stops TTL retries; a gen bump or TaskbarCreated retries.
static WP_NO_SOURCE: AtomicBool = AtomicBool::new(false);
/// Does any configured animation read the wallpaper? Set by the manager.
static WP_WANTED: AtomicBool = AtomicBool::new(false);
/// What to crop for: (hmon, work area) per monitor, published by the manager.
static WP_TARGETS: Mutex<Vec<(isize, RECT)>> = Mutex::new(Vec::new());
/// Registered "TaskbarCreated" message (Explorer broadcasts it on (re)start).
static TASKBAR_CREATED_MSG: AtomicU32 = AtomicU32::new(0);

/// One cropped wallpaper DDB for one monitor's work area.
struct WpEntry {
    hmon: isize,
    rect: RECT,
    gen: u64,
    bmp: isize,
}
const WP_SLIDE: usize = 0;
const WP_GLIDE: usize = 1;
/// Per-compositor hand-off slots: the capture thread puts a fresh set in, the
/// compositor takes the whole set. Untaken entries still belong to the
/// capture thread, which frees them when it replaces the set.
static WP_SLOTS: [Mutex<Vec<WpEntry>>; 2] = [const { Mutex::new(Vec::new()) }; 2];

/// When the capture thread should next render. `ttl_check` = an animation
/// finished (or a glide was skipped): render if the last try is older than
/// WP_TTL.
struct WpSched {
    due: Option<Instant>,
    ttl_check: bool,
}
static WP_SCHED: Mutex<WpSched> = Mutex::new(WpSched {
    due: None,
    ttl_check: false,
});
static WP_SCHED_CV: Condvar = Condvar::new();
const WP_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(1_500);
const WP_TTL: std::time::Duration = std::time::Duration::from_secs(60);
const WP_BUSY_RETRY: std::time::Duration = std::time::Duration::from_millis(250);
// True while the transition worker runs a slide (GLIDE_BUSY's twin), so a
// render never loads DWM mid-animation.
static SLIDE_BUSY: AtomicBool = AtomicBool::new(false);
// The monitor that slide is on, stored before SLIDE_BUSY (see `overlay_up_on`).
static SLIDE_HMON: AtomicIsize = AtomicIsize::new(0);

// Silent-failure counters: a cache that always misses or is always stale
// would otherwise just quietly turn every slide flat and every glide instant.
static WP_HIT: AtomicU64 = AtomicU64::new(0);
static WP_MISS: AtomicU64 = AtomicU64::new(0);
static WP_STALE: AtomicU64 = AtomicU64::new(0);
static WP_NOSRC: AtomicU64 = AtomicU64::new(0);
static WP_RENDERS: AtomicU64 = AtomicU64::new(0);
static WP_RENDER_MS: AtomicU32 = AtomicU32::new(0);

/// Does any configured animation composite over the wallpaper? Moving slide
/// and spring frames do, and so does the glide. Fade crossfades two whole
/// captures and never reads it. `animations` = animations on AND a non-zero
/// duration.
fn wallpaper_needed(animations: bool, ws: WsAnim, window_anim: &str) -> bool {
    animations && (matches!(ws, WsAnim::Slide | WsAnim::Spring) || window_anim == "glide")
}

/// Does this slide read the wallpaper? Only moving slide/spring frames: a
/// first visit (no incoming image) holds frame 0, and fade blends captures.
fn slide_wants_wallpaper(mode: WsAnim, have_incoming: bool) -> bool {
    have_incoming && matches!(mode, WsAnim::Slide | WsAnim::Spring)
}

/// May a compositor blit this crop? Only a current-gen crop made for exactly
/// this monitor and work area.
fn wp_entry_usable(
    entry_gen: u64,
    cur_gen: u64,
    entry_hmon: isize,
    entry_rect: RECT,
    hmon: isize,
    work_area: RECT,
) -> bool {
    entry_gen == cur_gen && entry_hmon == hmon && entry_rect == work_area
}

/// Can a glide compose right now? Lock-free, for the manager's retile path.
fn wp_ready() -> bool {
    WP_READY_GEN.load(Ordering::Acquire) == WALLPAPER_GEN.load(Ordering::Acquire)
}

/// Ask the capture thread to render `delay` from now. Each request replaces
/// the due time, so a burst of bumps renders once, after the last. WP_SCHED
/// is a leaf lock, never held across a render or any call into another
/// thread, so this is safe even re-entered inside a SetWindowPos (the
/// WM_DPICHANGED path that deadlocked in e0eea54).
fn wp_schedule(delay: std::time::Duration) {
    WP_SCHED.lock().unwrap().due = Some(Instant::now() + delay);
    WP_SCHED_CV.notify_one();
}

/// The wallpaper, a monitor or the work areas changed: every crop is stale
/// from now on. An atomic bump plus a condvar poke (no GDI, no messages), so
/// it is fine from any thread, the marker wndproc included.
fn wp_invalidate() {
    WALLPAPER_GEN.fetch_add(1, Ordering::AcqRel);
    wp_schedule(WP_DEBOUNCE);
}

/// An animation finished (or a glide was skipped for want of a crop): let the
/// capture thread re-render if its last try is older than WP_TTL.
fn wp_ttl_hint() {
    WP_SCHED.lock().unwrap().ttl_check = true;
    WP_SCHED_CV.notify_one();
}

/// Explorer (re)started: its wallpaper window is a new one. Forget the cached
/// source and any "no source" verdict, then re-render.
fn wp_explorer_restarted() {
    WP_SOURCE.store(0, Ordering::Relaxed);
    WP_NO_SOURCE.store(false, Ordering::Relaxed);
    WP_DIAG.store(false, Ordering::Relaxed);
    wp_invalidate();
}

/// Manager: publish what to crop for and whether to bother, then mark every
/// crop stale. Call wherever monitors, work areas or animation settings may
/// have changed (startup, reload, display change), after `reserve_bar`.
fn wp_publish(monitors: &[Monitor], cfg: &Config, delay: std::time::Duration) {
    WP_WANTED.store(
        wallpaper_needed(
            cfg.animations && cfg.animation_ms > 0,
            WsAnim::from_cfg(cfg),
            &cfg.window_anim,
        ),
        Ordering::Relaxed,
    );
    *WP_TARGETS.lock().unwrap() = monitors.iter().map(|m| (m.hmon, m.work_area)).collect();
    WALLPAPER_GEN.fetch_add(1, Ordering::AcqRel);
    wp_schedule(delay);
}

/// Free crops this thread owns (untaken slot entries, or a compositor's own).
unsafe fn wp_free(entries: Vec<WpEntry>) {
    for e in entries {
        let _ = DeleteObject(HGDIOBJ(e.bmp as *mut c_void));
    }
}

/// Wallpaper cache counters, for the counters line and the render log.
fn wp_counters() -> String {
    format!(
        "wp_hit={} wp_miss={} wp_stale={} wp_nosrc={} wp_renders={} wp_render_ms={}",
        WP_HIT.load(Ordering::Relaxed),
        WP_MISS.load(Ordering::Relaxed),
        WP_STALE.load(Ordering::Relaxed),
        WP_NOSRC.load(Ordering::Relaxed),
        WP_RENDERS.load(Ordering::Relaxed),
        WP_RENDER_MS.load(Ordering::Relaxed),
    )
}

/// A compositor's own wallpaper crops, taken from its WP_SLOTS slot. Lives on
/// that compositor thread, which is their only owner and only deleter.
struct WpCache {
    slot: usize,
    own: Vec<WpEntry>,
}

impl WpCache {
    fn new(slot: usize) -> WpCache {
        WpCache {
            slot,
            own: Vec::new(),
        }
    }

    /// The current crop for (hmon, exact work area), or 0 on a miss. Never
    /// renders: a miss means a flat slide or an instant glide, not a wait on
    /// Explorer while the manager is blocked on our overlay signal. The
    /// returned bitmap stays owned here; the caller only selects it for the
    /// duration of one animation.
    unsafe fn get(&mut self, hmon: isize, work_area: RECT) -> isize {
        // Take whatever the capture thread published since last time; the
        // swap is the only work under the lock, DeleteObject runs after it.
        let fresh = std::mem::take(&mut *WP_SLOTS[self.slot].lock().unwrap());
        if !fresh.is_empty() {
            wp_free(std::mem::replace(&mut self.own, fresh));
        }
        // The gen only grows, so a stale crop can never become usable again:
        // free it now rather than carry megabytes of dead bitmap around.
        let cur = WALLPAPER_GEN.load(Ordering::Acquire);
        let mut stale = false;
        self.own.retain(|e| {
            if e.gen == cur {
                return true;
            }
            stale |= e.hmon == hmon && e.rect == work_area;
            let _ = DeleteObject(HGDIOBJ(e.bmp as *mut c_void));
            false
        });
        let hit = self
            .own
            .iter()
            .find(|e| wp_entry_usable(e.gen, cur, e.hmon, e.rect, hmon, work_area));
        match hit {
            Some(e) => {
                WP_HIT.fetch_add(1, Ordering::Relaxed);
                e.bmp
            }
            None => {
                let counter = if stale { &WP_STALE } else { &WP_MISS };
                counter.fetch_add(1, Ordering::Relaxed);
                log_debug!(
                    "wallpaper: {} for {:#x} (slot {}, gen {cur})",
                    if stale { "stale crop" } else { "no crop" },
                    hmon,
                    self.slot
                );
                0
            }
        }
    }
}

/// Class check for the cached wallpaper source: after an Explorer restart the
/// old HWND value can belong to an unrelated window.
unsafe fn is_wallpaper_class(h: HWND) -> bool {
    let mut buf = [0u16; 16];
    let n = (GetClassNameW(h, &mut buf).max(0) as usize).min(buf.len());
    let class = String::from_utf16_lossy(&buf[..n]);
    class == "WorkerW" || class == "Progman"
}

/// The wallpaper source window, cached. Validated on every use (IsWindow +
/// WorkerW/Progman class); the 0x052C nudge and the EnumWindows walk run only
/// on a miss. Capture thread only.
unsafe fn wp_source() -> HWND {
    let cached = hwnd_from(WP_SOURCE.load(Ordering::Relaxed));
    if !cached.0.is_null() && IsWindow(cached).as_bool() && is_wallpaper_class(cached) {
        return cached;
    }
    let src = wallpaper_window();
    WP_SOURCE.store(src.0 as isize, Ordering::Relaxed);
    src
}

/// Find the desktop window that paints the wallpaper. On Win10/11 it's usually a
/// WorkerW spawned behind the icon host (SHELLDLL_DefView); on some configs the
/// wallpaper is on Progman itself, which is the fallback. Returns null if neither.
/// Capture thread only (it sends to Explorer; see the cache notes above).
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

enum WpOutcome {
    Published,
    Skipped,
    NoSource,
    Failed,
}

/// One render on the capture thread: a single PrintWindow of the wallpaper
/// window, cropped once per (monitor, work area) per compositor and handed to
/// the WP_SLOTS.
unsafe fn wp_render() -> WpOutcome {
    if !WP_WANTED.load(Ordering::Relaxed) {
        // No animation reads the wallpaper: no Explorer round trip at all (not
        // even the 0x052C nudge), and drop anything nobody will take.
        for slot in &WP_SLOTS {
            let old = std::mem::take(&mut *slot.lock().unwrap());
            wp_free(old);
        }
        return WpOutcome::Skipped;
    }
    // Stamp with the gen read BEFORE rendering: a bump during the render makes
    // these crops stale at once, and that bump already scheduled a re-render.
    // Read it before the targets too: `wp_publish` stores new targets and only
    // then bumps, so a gen read here can never be stamped on the old targets.
    let gen = WALLPAPER_GEN.load(Ordering::Acquire);
    let targets = WP_TARGETS.lock().unwrap().clone();
    if targets.is_empty() {
        return WpOutcome::Skipped;
    }
    let t0 = Instant::now();
    let src = wp_source();
    if src.0.is_null() {
        WP_NOSRC.fetch_add(1, Ordering::Relaxed);
        if !WP_DIAG.swap(true, Ordering::Relaxed) {
            log_info!("wallpaper: no Progman/WorkerW found -> flat slide, instant glide");
        }
        return WpOutcome::NoSource;
    }
    let mut wr = RECT::default();
    if GetWindowRect(src, &mut wr).is_err() {
        WP_SOURCE.store(0, Ordering::Relaxed);
        return WpOutcome::Failed;
    }
    let (ww, wh) = (wr.right - wr.left, wr.bottom - wr.top);
    if ww <= 0 || wh <= 0 {
        return WpOutcome::Failed;
    }
    let screen = GetDC(None);
    if screen.0.is_null() {
        return WpOutcome::Failed;
    }
    // Render the WHOLE wallpaper window with PrintWindow + PW_RENDERFULLCONTENT
    // (BitBlt of a DWM-composited desktop window comes back black), then crop
    // every work area out of that one render.
    let fulldc = CreateCompatibleDC(screen);
    let fullbmp = CreateCompatibleBitmap(screen, ww, wh);
    let ofb = SelectObject(fulldc, HGDIOBJ(fullbmp.0));
    let printed = !fullbmp.is_invalid()
        && PrintWindow(src, fulldc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();
    let mut fresh: [Vec<WpEntry>; 2] = [Vec::new(), Vec::new()];
    if printed {
        let cropdc = CreateCompatibleDC(screen);
        for &(hmon, rect) in &targets {
            let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
            if w <= 0 || h <= 0 {
                continue;
            }
            for set in fresh.iter_mut() {
                let bmp = CreateCompatibleBitmap(screen, w, h);
                if bmp.is_invalid() {
                    continue;
                }
                let ocb = SelectObject(cropdc, HGDIOBJ(bmp.0));
                let ok = BitBlt(
                    cropdc,
                    0,
                    0,
                    w,
                    h,
                    fulldc,
                    rect.left - wr.left,
                    rect.top - wr.top,
                    SRCCOPY,
                )
                .is_ok();
                SelectObject(cropdc, ocb);
                if ok {
                    set.push(WpEntry {
                        hmon,
                        rect,
                        gen,
                        bmp: bmp.0 as isize,
                    });
                } else {
                    let _ = DeleteObject(HGDIOBJ(bmp.0));
                }
            }
        }
        let _ = DeleteDC(cropdc);
    }
    SelectObject(fulldc, ofb);
    let _ = DeleteObject(HGDIOBJ(fullbmp.0));
    let _ = DeleteDC(fulldc);
    let _ = ReleaseDC(None, screen);
    // The crops are selected on the compositor threads next: flush this
    // thread's GDI batch before handing them over (GdiFlush docs, GDI objects
    // shared between threads).
    let _ = GdiFlush();
    let ms = t0.elapsed().as_millis() as u32;
    if !WP_DIAG.swap(true, Ordering::Relaxed) {
        let mut buf = [0u16; 64];
        let n = GetClassNameW(src, &mut buf);
        let class = String::from_utf16_lossy(&buf[..n as usize]);
        log_info!("wallpaper source class '{class}', PrintWindow={printed}, render={ms}ms");
    }
    if !printed {
        // A failed print can mean a dead or wrong source: rediscover next time.
        WP_SOURCE.store(0, Ordering::Relaxed);
        return WpOutcome::Failed;
    }
    let glide_ready = !fresh[WP_GLIDE].is_empty();
    for (slot, set) in WP_SLOTS.iter().zip(fresh) {
        let old = std::mem::replace(&mut *slot.lock().unwrap(), set);
        wp_free(old); // never taken, so still this thread's to free
    }
    if glide_ready {
        WP_READY_GEN.store(gen, Ordering::Release);
    }
    WP_RENDERS.fetch_add(1, Ordering::Relaxed);
    WP_RENDER_MS.store(ms, Ordering::Relaxed);
    log_debug!(
        "wallpaper: rendered gen {gen} for {} monitor(s) in {ms} ms ({})",
        targets.len(),
        wp_counters()
    );
    WpOutcome::Published
}

/// The wallpaper capture thread: the only caller of `wallpaper_window` and
/// PrintWindow. Normal priority; sleeps on WP_SCHED_CV between renders.
fn wallpaper_capture_worker() {
    // Last render attempt, successful or not, for the TTL backstop: a failing
    // render retries at most once per WP_TTL rather than after every animation.
    let mut last_try: Option<Instant> = None;
    loop {
        {
            let mut s = WP_SCHED.lock().unwrap();
            loop {
                if std::mem::take(&mut s.ttl_check)
                    && s.due.is_none()
                    && !WP_NO_SOURCE.load(Ordering::Relaxed)
                    && last_try.is_none_or(|t| t.elapsed() >= WP_TTL)
                {
                    s.due = Some(Instant::now() + WP_DEBOUNCE);
                }
                match s.due {
                    Some(due) => {
                        let now = Instant::now();
                        if now >= due {
                            s.due = None;
                            break;
                        }
                        s = WP_SCHED_CV.wait_timeout(s, due - now).unwrap().0;
                    }
                    None => s = WP_SCHED_CV.wait(s).unwrap(),
                }
            }
        }
        // Never load Explorer and DWM with a render mid-animation. Retry soon,
        // but never earlier than a bump that arrived meanwhile asked for (that
        // one is still waiting out Explorer's wallpaper fade).
        if GLIDE_BUSY.load(Ordering::Relaxed) || SLIDE_BUSY.load(Ordering::Relaxed) {
            let retry = Instant::now() + WP_BUSY_RETRY;
            let mut s = WP_SCHED.lock().unwrap();
            s.due = Some(s.due.map_or(retry, |d| d.max(retry)));
            continue;
        }
        let outcome = unsafe { wp_render() };
        if !matches!(outcome, WpOutcome::Skipped) {
            last_try = Some(Instant::now());
        }
        WP_NO_SOURCE.store(matches!(outcome, WpOutcome::NoSource), Ordering::Relaxed);
    }
}

/// A request that will never play: its outgoing capture still goes back to the
/// manager, which keeps it as that workspace's snapshot if nothing newer
/// replaced it (freeing it instead would leave a burst's visits with the
/// cover-hold and no slide). Its incoming snapshot is dropped: that workspace
/// became active with the switch, and its snapshot is re-created from the
/// outgoing capture when it is next left. Manager thread, no GDI on the
/// bitmaps here, so nothing to flush.
fn slide_rehome(req: SlideReq) {
    let w = req.rect.right - req.rect.left;
    let h = req.rect.bottom - req.rect.top;
    SNAP_RETURNS.lock().unwrap().push(SnapReturn {
        key: (req.hmon, req.old_ws),
        gen: req.gen,
        snap: Snap {
            bmp: req.out_bmp,
            rects: req.out_rects,
            w,
            h,
        },
    });
    // req.in_bmp drops here, after the lock.
}

/// Hand a slide to the transition thread, replacing any request it hasn't
/// picked up yet (`slide_rehome`), so a burst of switches can't leak frozen
/// bitmaps or lose snapshots.
fn dispatch_slide(req: SlideReq) {
    // No handshake reset: the wait is keyed by req.gen, so an earlier
    // request's signal can never release it.
    let old = SLIDE_REQ.lock().unwrap().replace(req);
    SLIDE_CV.notify_one();
    if let Some(old) = old {
        slide_rehome(old); // outside SLIDE_REQ: no nested locks
    }
}

/// Take back a slide the worker has not picked up yet (left behind by a
/// handshake timeout). A switch that raises no overlay of its own calls this,
/// or that older slide would play later over the finished switch. Returns
/// whether there was one.
fn slide_cancel_pending() -> bool {
    let old = SLIDE_REQ.lock().unwrap().take();
    match old {
        Some(old) => {
            slide_rehome(old);
            true
        }
        None => false,
    }
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
// correct windows. No current wallpaper crop (see the wallpaper cache) degrades
// to instant.
// =========================================================================

/// One window's travel for a glide, in work-area-local coordinates.
struct GlideItem {
    old: RECT,
    new: RECT,
}

/// Edge jitter (px) under which a window counts as not moving: DWM shadow and
/// rounding noise. Shared by the "did the layout change?" test and the 1:1 draw.
const GLIDE_STILL_PX: i32 = 2;

/// Is every edge of `new` within GLIDE_STILL_PX of `old`?
fn glide_still(old: &RECT, new: &RECT) -> bool {
    (old.left - new.left).abs() <= GLIDE_STILL_PX
        && (old.top - new.top).abs() <= GLIDE_STILL_PX
        && (old.right - new.right).abs() <= GLIDE_STILL_PX
        && (old.bottom - new.bottom).abs() <= GLIDE_STILL_PX
}

/// DWM shadow reach (logical px) past a window's rect. The glide's damage box
/// grows by this much: cut tighter, the moved windows' live shadows would show
/// at the overlay's edge from frame 0, and the old positions' shadows would
/// vanish outside it.
const GLIDE_SHADOW_PX: i32 = 32;

/// Damage covering at least this share (percent) of the work area glides the
/// whole work area, as before: a near-full capture saves nothing, and the
/// full path has fewer edge cases.
const GLIDE_DAMAGE_FULL_PCT: i64 = 90;

/// The work-area-local rect a glide has to cover (ANIM-10): the union of the
/// old and new rects of every item that actually moves (not glide_still),
/// grown by `margin` and clamped to the `w` x `h` work area; the whole work
/// area when that is GLIDE_DAMAGE_FULL_PCT or more of it. None = nothing
/// visibly moves. Everything outside it is a live window that stays put (or
/// moves <= GLIDE_STILL_PX and is placed live), so the capture, the overlay
/// and every frame shrink to it: capture cost is ~5 ms fixed plus area
/// (re-measured in review: 960x540 10.1 ms, 1920x1080 25.0 ms), and it runs on
/// the manager before placement.
fn glide_damage(items: &[GlideItem], margin: i32, w: i32, h: i32) -> Option<RECT> {
    let mut u: Option<RECT> = None;
    for it in items.iter().filter(|it| !glide_still(&it.old, &it.new)) {
        for r in [it.old, it.new] {
            u = Some(match u {
                None => r,
                Some(a) => RECT {
                    left: a.left.min(r.left),
                    top: a.top.min(r.top),
                    right: a.right.max(r.right),
                    bottom: a.bottom.max(r.bottom),
                },
            });
        }
    }
    let u = u?;
    let d = RECT {
        left: (u.left - margin).clamp(0, w),
        top: (u.top - margin).clamp(0, h),
        right: (u.right + margin).clamp(0, w),
        bottom: (u.bottom + margin).clamp(0, h),
    };
    if d.right <= d.left || d.bottom <= d.top {
        return None; // every move lies outside the work area: nothing to show
    }
    let area = (d.right - d.left) as i64 * (d.bottom - d.top) as i64;
    if area * 100 >= w as i64 * h as i64 * GLIDE_DAMAGE_FULL_PCT {
        return Some(RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        });
    }
    Some(d)
}

/// Do `a` and `b` overlap (non-empty intersection)?
fn rects_overlap(a: &RECT, b: &RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

/// How one glide item is drawn this frame.
#[derive(Debug, PartialEq, Eq)]
enum GlideBlit {
    /// Same size as the source: a plain BitBlt, no scaling path at all.
    Blit,
    /// Scaled: StretchBlt (COLORONCOLOR).
    Stretch,
    /// Degenerate source or destination: draw nothing.
    Skip,
}

fn glide_blit_kind(dw: i32, dh: i32, sw: i32, sh: i32) -> GlideBlit {
    if dw <= 0 || dh <= 0 || sw <= 0 || sh <= 0 {
        GlideBlit::Skip
    } else if dw == sw && dh == sh {
        GlideBlit::Blit
    } else {
        GlideBlit::Stretch
    }
}

/// A cosmetic window-glide request handed from the manager to the glide worker.
/// The worker owns and frees `out_bmp`.
struct GlideReq {
    out_bmp: isize,        // HBITMAP: frozen `area` before placement (worker frees)
    hmon: isize,           // monitor, with `rect` the wallpaper-crop key
    rect: RECT,            // work area (the wallpaper crop's extent)
    area: RECT,            // screen rect glided (glide_damage): capture + overlay geometry
    items: Vec<GlideItem>, // per-window old->new travel, `area`-local
    dur_ms: u64,
    queued: Option<Instant>, // dispatch time, for the pickup probe (debug only)
    ex_style: WINDOW_EX_STYLE, // overlay_ex_style of the configured animation_ms
}
static GLIDE_REQ: Mutex<Option<GlideReq>> = Mutex::new(None);
static GLIDE_CV: Condvar = Condvar::new();
static GLIDE_READY: Mutex<bool> = Mutex::new(false);
static GLIDE_READY_CV: Condvar = Condvar::new();
// True from dispatch until the overlay tears down. Lets the manager skip
// stacking a second glide over a running one (it places instantly instead).
static GLIDE_BUSY: AtomicBool = AtomicBool::new(false);
// The monitor that glide is on, stored with GLIDE_BUSY: a glide on one monitor
// must not count as an overlay over another (see `overlay_up_on`).
static GLIDE_HMON: AtomicIsize = AtomicIsize::new(0);

/// Could a capture of `hmon` right now include one of Astur's own overlays (a
/// slide or glide running, or tearing down, on that monitor)? Sampled BEFORE
/// the capture: overlays are only ever started by the manager, so none can
/// appear during it. Such a capture is fine as a slide's frame 0 (it is what
/// is on screen) but is never cached as a workspace snapshot.
fn overlay_up_on(hmon: isize) -> bool {
    (SLIDE_BUSY.load(Ordering::SeqCst) && SLIDE_HMON.load(Ordering::SeqCst) == hmon)
        || (GLIDE_BUSY.load(Ordering::SeqCst) && GLIDE_HMON.load(Ordering::SeqCst) == hmon)
}

/// Glide twin of `wait_slide_overlay_up`; false on timeout.
fn wait_glide_overlay_up() -> bool {
    let guard = GLIDE_READY.lock().unwrap();
    let (_guard, res) = GLIDE_READY_CV
        .wait_timeout_while(guard, std::time::Duration::from_millis(250), |up| !*up)
        .unwrap();
    !res.timed_out()
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
    raise_current_thread(ThreadRole::Compositor);
    let mut wp = WpCache::new(WP_GLIDE);
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
        unsafe { run_window_glide(req, &mut wp) };
        GLIDE_BUSY.store(false, Ordering::Relaxed);
        wp_ttl_hint();
    }
}

/// Composite a window glide: wallpaper backdrop + each window's frozen image
/// blitted from its old rect to an eased-interpolated rect (StretchBlt covers
/// resizes). Worker owns and frees `out_bmp`; the wallpaper crop stays in `wpc`.
unsafe fn run_window_glide(req: GlideReq, wpc: &mut WpCache) {
    let mut probe = Probe::start("glide");
    probe.note(format_args!("items={}", req.items.len()));
    if let Some(q) = req.queued {
        probe.note(format_args!("pickup={}us", q.elapsed().as_micros()));
    }
    // The overlay covers only the glided area (glide_damage); the wallpaper
    // crop is the whole work area's, read from the matching sub-rect.
    let full = req.area;
    let w = full.right - full.left;
    let h = full.bottom - full.top;
    let (wp_x, wp_y) = (full.left - req.rect.left, full.top - req.rect.top);
    probe.note(format_args!(
        "area={w}x{h}@{wp_x},{wp_y} of {}x{}",
        req.rect.right - req.rect.left,
        req.rect.bottom - req.rect.top
    ));
    let free_out = || {
        let _ = DeleteObject(HGDIOBJ(req.out_bmp as *mut c_void));
    };
    if w <= 0 || h <= 0 || req.out_bmp == 0 || req.items.is_empty() {
        free_out();
        signal_glide_overlay_up();
        return;
    }
    // Need the still wallpaper to fill vacated areas. It comes ready-made from
    // the capture thread (never rendered here: see the wallpaper cache notes).
    // With no current crop for this exact monitor + work area, degrade to an
    // instant switch (no overlay): signal and bail, the manager places the
    // real windows with no animation.
    let wp = wpc.get(req.hmon, req.rect);
    probe.mark("wallpaper");
    if wp == 0 {
        free_out();
        signal_glide_overlay_up();
        probe.note(format_args!("no-wallpaper=instant"));
        return;
    }
    let hinst = HINSTANCE(BAR_HINST.load(Ordering::Relaxed) as *mut c_void);
    // Click-through for short animations (see overlay_ex_style).
    let overlay = CreateWindowExW(
        req.ex_style,
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
    probe.mark("window");
    let Ok(overlay) = overlay else {
        free_out();
        signal_glide_overlay_up();
        return;
    };
    if !overlay_make_visible(overlay, req.ex_style) {
        let _ = DestroyWindow(overlay);
        free_out();
        signal_glide_overlay_up();
        return;
    }

    let odc = GetDC(overlay);
    let srcdc = CreateCompatibleDC(odc); // frozen before-frame
    let os = SelectObject(srcdc, HGDIOBJ(req.out_bmp as *mut c_void));

    // Frame 0 must be pixel-identical to the live screen: the exact capture,
    // presented straight from srcdc (not the wallpaper-composited compose(0.0),
    // and no back-buffer hop, which was a second full-frame blit before the
    // signal). CRITICAL ORDER: show the overlay FIRST, THEN present — a blit to a
    // still-hidden window's DC is clipped away and lost, leaving the overlay
    // empty so the wallpaper flashes through (see the full note in
    // run_transition). Show, present, settle, flush, then signal.
    let _ = ShowWindow(overlay, SW_SHOWNA);
    let _ = BitBlt(odc, 0, 0, w, h, srcdc, 0, 0, SRCCOPY);
    let _ = UpdateWindow(overlay);
    let _ = DwmFlush();
    signal_glide_overlay_up();
    probe.mark("signal");

    // Back buffer and wallpaper DC only now: nothing before the signal reads
    // them, and allocating a fresh w x h bitmap there was manager wait time.
    // Every compose starts with a full wallpaper blit, so the buffer's initial
    // contents are never shown. (A retarget mid-glide would have to seed it
    // from srcdc here first.)
    let backdc = CreateCompatibleDC(odc);
    let back = CreateCompatibleBitmap(odc, w, h);
    let wpdc = CreateCompatibleDC(odc); // wallpaper backdrop
    let ob = SelectObject(backdc, HGDIOBJ(back.0));
    let owp = SelectObject(wpdc, HGDIOBJ(wp as *mut c_void));
    // Nearest-neighbour, explicitly. HALFTONE was the frame budget (ANIM-7,
    // measured in review at 950x1060, DDB, GdiFlush-synced): 6.4 ms per
    // equal-size item and 15 ms scaled, against 0.83 / 1.5 ms for COLORONCOLOR.
    // From those parts a 3-window frame ran an estimated 16-35 ms against an
    // 8.3 ms budget, so the teardown landed late. Not the default BLACKONWHITE:
    // it ANDs pixels together when shrinking. Accepted cost: slight aliasing on
    // scaled mid-glide frames only; frame 0 is the exact capture and the
    // reveal is the real window.
    SetStretchBltMode(backdc, COLORONCOLOR);

    // Compose one frame at eased progress `e` (0..=1). At e=0 every window sits
    // at its old rect over the still wallpaper == current screen (no flash). At
    // e=1 every window is at its new rect, pixel-aligned with the real windows
    // placed underneath, so the reveal is seamless.
    let compose = |e: f64| {
        let _ = BitBlt(backdc, 0, 0, w, h, wpdc, wp_x, wp_y, SRCCOPY);
        for it in &req.items {
            let lerp = |a: i32, b: i32| (a as f64 + (b - a) as f64 * e).round() as i32;
            let dl = lerp(it.old.left, it.new.left);
            let dt = lerp(it.old.top, it.new.top);
            let (sw, sh) = (it.old.right - it.old.left, it.old.bottom - it.old.top);
            // A window that barely changes (within the jitter the manager
            // already ignores) is drawn 1:1 at its gliding origin: stretching
            // it by a pixel or two nearest-neighbour would shimmer its text for
            // the whole glide.
            let (dw, dh) = if glide_still(&it.old, &it.new) {
                (sw, sh)
            } else {
                (
                    lerp(it.old.right, it.new.right) - dl,
                    lerp(it.old.bottom, it.new.bottom) - dt,
                )
            };
            match glide_blit_kind(dw, dh, sw, sh) {
                // Pure moves keep their exact size every frame (the lerped
                // edges move together), so this is the common case.
                GlideBlit::Blit => {
                    let _ = BitBlt(
                        backdc,
                        dl,
                        dt,
                        dw,
                        dh,
                        srcdc,
                        it.old.left,
                        it.old.top,
                        SRCCOPY,
                    );
                }
                GlideBlit::Stretch => {
                    let _ = StretchBlt(
                        backdc,
                        dl,
                        dt,
                        dw,
                        dh,
                        srcdc,
                        it.old.left,
                        it.old.top,
                        sw,
                        sh,
                        SRCCOPY,
                    );
                }
                GlideBlit::Skip => {}
            }
        }
    };

    let dur = req.dur_ms.max(1) as f64;
    let frame_dur = std::time::Duration::from_micros(8_333); // ~120 Hz
    let start = Instant::now();
    let mut next = start;
    let mut msg = MSG::default();
    // compose+present time per frame (us), probes only.
    let mut frame_us: Vec<u32> = Vec::new();
    loop {
        while PeekMessageW(&mut msg, overlay, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if back.is_invalid() {
            // No back buffer (GDI quota): presenting it would show garbage.
            // Frame 0 is on screen, the windows are placed under it; reveal.
            break;
        }
        let el = start.elapsed().as_secs_f64() * 1000.0;
        let tf = probe.on().then(Instant::now);
        compose(ease_out_cubic((el / dur).min(1.0)));
        let _ = BitBlt(odc, 0, 0, w, h, backdc, 0, 0, SRCCOPY);
        if let Some(tf) = tf {
            if frame_us.is_empty() {
                probe.mark("first_frame");
            }
            frame_us.push(tf.elapsed().as_micros() as u32);
        }
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
    SelectObject(wpdc, owp); // deselected: the crop goes back to `wpc` for reuse
    let _ = DeleteObject(HGDIOBJ(back.0));
    let _ = DeleteDC(backdc);
    let _ = DeleteDC(srcdc);
    let _ = DeleteDC(wpdc);
    ReleaseDC(overlay, odc);
    free_out();
    // Sync teardown to the next DWM frame so the real (already-placed) windows
    // are composited before the overlay disappears — no flash on the reveal.
    let _ = DwmFlush();
    let _ = DestroyWindow(overlay);
    probe.mark("teardown");
    let (p50, max) = p50_max(&mut frame_us);
    probe.note(format_args!(
        "frames={} frame_p50={p50}us frame_max={max}us",
        frame_us.len()
    ));
}

/// Transition thread: owns the slide overlay and pumps its own message loop, so
/// the overlay is a well-behaved window (never the "not responding" ghost a
/// pump-less window becomes). Blocks on the condvar when idle.
fn transition_worker() {
    raise_current_thread(ThreadRole::Compositor);
    let mut wp = WpCache::new(WP_SLIDE);
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
        // Release: the manager reads BUSY then HMON on another thread, so a
        // BUSY=true it sees must carry this HMON (Relaxed gave no such order).
        SLIDE_HMON.store(req.hmon, Ordering::Relaxed);
        SLIDE_BUSY.store(true, Ordering::Release);
        unsafe { run_transition(req, &mut wp) };
        SLIDE_BUSY.store(false, Ordering::Release);
        wp_ttl_hint();
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
/// worker frees the incoming bitmap and hands the outgoing one back to the
/// manager (SnapHome). When `in_bmp` is None (first visit to the destination,
/// no cached snapshot) the overlay instead HOLDS the outgoing frame for
/// `COVER_HOLD_MS` to cover the switch + first paint, then reveals. The
/// wallpaper crop, when one is used, stays owned by `wpc`.
unsafe fn run_transition(req: SlideReq, wpc: &mut WpCache) {
    let mut probe = Probe::start("slide");
    probe.note(format_args!("first_visit={}", req.in_bmp.is_none()));
    if let Some(q) = req.queued {
        probe.note(format_args!("pickup={}us", q.elapsed().as_micros()));
    }
    let full = req.rect;
    let w = full.right - full.left;
    let h = full.bottom - full.top;
    let k = req.gen;
    // From here on, every return hands `out` back; `req.in_bmp` drops with
    // `req`. Both are deselected on every path before that.
    let home = SnapHome(Some(SnapReturn {
        key: (req.hmon, req.old_ws),
        gen: k,
        snap: Snap {
            bmp: req.out_bmp,
            rects: req.out_rects,
            w,
            h,
        },
    }));
    let out_raw = home.raw();
    let out_rects = home.rects();
    let in_raw = req.in_bmp.as_ref().map_or(0, Bmp::raw);
    if w <= 0 || h <= 0 {
        signal_slide_overlay_up(k, false); // unblock the manager (no overlay this time)
        return;
    }
    // Picked up after the manager gave up on it (handshake timeout): that
    // switch is long done, uncovered. Nothing to show.
    if !slide_gen_live(
        k,
        SLIDE_GEN.load(Ordering::SeqCst),
        SLIDE_ABORT.load(Ordering::SeqCst),
    ) {
        probe.note(format_args!("stale=dropped"));
        return;
    }
    // No incoming image == first visit to the destination workspace (no cached
    // snapshot). We still raise the overlay and HOLD the outgoing frame so the real
    // switch + the destination's first paint happen underneath it, hidden, then
    // reveal — killing the "background flashes through the windows" pop a
    // freshly-shown (surface-discarded) window makes before it repaints.
    let have_incoming = in_raw != 0;
    // The still wallpaper backdrop, only when a frame will read it: moving
    // slide/spring frames. A first visit holds frame 0 and fade blends whole
    // captures, so neither touches it. It comes ready-made from the capture
    // thread, never rendered here: this used to be a PrintWindow right here,
    // squarely on the manager's critical path, since the manager waits for our
    // signal before it switches. Stale or missing = the flat filmstrip (wp == 0).
    let wants_wp = slide_wants_wallpaper(req.mode, have_incoming);
    let wp = if wants_wp { wpc.get(req.hmon, full) } else { 0 };
    probe.mark("wallpaper");
    if wants_wp && wp == 0 {
        probe.note(format_args!("no-wallpaper=flat"));
    }
    let hinst = HINSTANCE(BAR_HINST.load(Ordering::Relaxed) as *mut c_void);
    // Click-through for short animations (see overlay_ex_style).
    let overlay = CreateWindowExW(
        req.ex_style,
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
    probe.mark("window");
    let Ok(overlay) = overlay else {
        signal_slide_overlay_up(k, false);
        return;
    };
    if !overlay_make_visible(overlay, req.ex_style) {
        let _ = DestroyWindow(overlay);
        signal_slide_overlay_up(k, false);
        return;
    }

    // Before the signal, only what frame 0 needs: the window DC and the
    // outgoing capture (plus the incoming one, selected here so no GDI setup is
    // left between the signal and the first moving frame's inputs).
    let odc = GetDC(overlay);
    let outdc = CreateCompatibleDC(odc);
    let oo = SelectObject(outdc, HGDIOBJ(out_raw as *mut c_void));
    let (indc, oi) = if have_incoming {
        let dc = CreateCompatibleDC(odc);
        (dc, SelectObject(dc, HGDIOBJ(in_raw as *mut c_void)))
    } else {
        (HDC::default(), HGDIOBJ::default())
    };

    // Frame 0. CRITICAL: it must be pixel-identical to what's already on
    // screen, or the instant the overlay is raised it pops (the "flash before
    // the slide"). `compose(0)` rebuilds the frame from the PrintWindow
    // wallpaper capture + window rects; if that wallpaper differs even slightly
    // from the live DWM-composited desktop (acrylic/transparency, sub-pixel
    // crop), the gaps flash on raise. So frame 0 is the EXACT live screen
    // capture (`out_bmp`, grabbed by `capture_monitor` a moment ago), presented
    // straight from outdc to the window: a guaranteed match, and one full-frame
    // blit before the signal instead of two (there is no back-buffer hop; the
    // back buffer does not exist yet). The wallpaper-composited path only
    // kicks in once the windows actually start moving (off != 0), where a
    // sub-pixel gap diff is invisible under motion.
    //
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
    //
    // Last check before anything reaches the screen: if the manager gave up
    // on this request meanwhile, or a newer one exists, the switch it was for
    // already happened uncovered, and showing frame 0 now would paint the
    // pre-switch screen over it. Never shown, so no DwmFlush is needed.
    if !slide_gen_live(
        k,
        SLIDE_GEN.load(Ordering::SeqCst),
        SLIDE_ABORT.load(Ordering::SeqCst),
    ) {
        SelectObject(outdc, oo);
        let _ = DeleteDC(outdc);
        if have_incoming {
            SelectObject(indc, oi);
            let _ = DeleteDC(indc);
        }
        ReleaseDC(overlay, odc);
        let _ = DestroyWindow(overlay);
        signal_slide_overlay_up(k, false);
        probe.note(format_args!("stale=never-shown"));
        return;
    }
    let _ = ShowWindow(overlay, SW_SHOWNA);
    let _ = BitBlt(odc, 0, 0, w, h, outdc, 0, 0, SRCCOPY);
    let _ = UpdateWindow(overlay);
    let _ = DwmFlush();
    signal_slide_overlay_up(k, true);
    probe.mark("signal");
    // The handshake lock orders this read after a timeout's abort mark: true
    // means the manager stopped waiting before our signal and switched
    // uncovered, so frame 0 is already stale. Take it straight down.
    let aborted = SLIDE_ABORT.load(Ordering::SeqCst) == k;
    let _glass = GlassGuard(k);
    if aborted {
        probe.note(format_args!("late=torn-down"));
    } else {
        let anim_ms = if have_incoming {
            req.dur_ms
        } else {
            COVER_HOLD_MS
        };
        glass_on(k, req.hmon, now_ms() + anim_ms);
    }
    // The animation clock starts at the signal, before the fade's top overlay
    // is set up: that setup ends in a DwmFlush (~1 frame, measured 17 ms on
    // the bench desktop) and would otherwise be added to every fade's end.
    let start = Instant::now();
    // Fade: the incoming image on a second, layered overlay above this one,
    // and DWM does the blend (ANIM-9). None = the GDI AlphaBlend loop below.
    let fade_top = if req.mode == WsAnim::Fade && have_incoming && !aborted {
        fade_overlay_up(full, req.ex_style, hinst, indc)
    } else {
        None
    };
    if fade_top.is_some() {
        probe.mark("fade_top");
    }

    // One reused back buffer + wallpaper DC, created only now: nothing before
    // the signal reads them, and allocating (and first-touching) a fresh w x h
    // bitmap there was manager wait time. Its initial contents are never shown:
    // every compose fills it completely (the wallpaper path starts with a full
    // blit, the flat filmstrip's two blits cover [0, w) for any off in
    // [target, 0], fade starts with a full blit), and the first-visit hold never
    // reads it. If a retarget mid-slide is ever added, seed it from outdc here.
    let backdc = CreateCompatibleDC(odc);
    let back = CreateCompatibleBitmap(odc, w, h);
    let wpdc = CreateCompatibleDC(odc);
    let ob = SelectObject(backdc, HGDIOBJ(back.0));
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
            for r in out_rects {
                let (rw, rh) = (r.right - r.left, r.bottom - r.top);
                let _ = BitBlt(
                    backdc,
                    r.left + off,
                    r.top,
                    rw,
                    rh,
                    outdc,
                    r.left,
                    r.top,
                    SRCCOPY,
                );
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

    // The new ws came from the `dir` side, so the outgoing leaves the opposite
    // way; the incoming sits in the adjacent filmstrip slot (off + dir*w) and is
    // contiguous with it (no seam).
    let target = -req.dir * w;
    let dur = req.dur_ms.max(1) as f64;
    let has_wp = wp != 0;
    let frame_dur = std::time::Duration::from_micros(8_333); // ~120 Hz back-buffer
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
    // compose+present time per moving frame (us), probes only.
    let mut frame_us: Vec<u32> = Vec::new();
    let cover = std::time::Duration::from_millis(COVER_HOLD_MS);
    let hold_cap = std::time::Duration::from_millis(HOLD_CAP_MS);
    // The manager hold being honoured (its GLASS value), when this thread first
    // saw it, and when the manager released it.
    let mut hold_seen = 0u64;
    let mut hold_at = start;
    let mut released_at: Option<Instant> = None;
    let mut holds = 0u32;
    loop {
        while PeekMessageW(&mut msg, overlay, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let g = if aborted {
            0
        } else {
            GLASS.load(Ordering::SeqCst)
        };
        if g >> 16 != k {
            // Never on the glass (late), or reset as stuck: reveal now.
            break;
        }
        if g & GLASS_HOLDS != 0 {
            // A switch is running under this overlay. Stop advancing (the frame
            // on the glass stays), keep pumping, and reveal COVER_HOLD_MS after
            // the manager releases it, which covers the incoming windows' first
            // paint exactly like a first visit. A newer hold restarts the wait.
            if g != hold_seen {
                hold_seen = g;
                hold_at = Instant::now();
                released_at = None;
                holds += 1;
            }
            if released_at.is_none() && glass_released(g, SLIDE_RELEASED.load(Ordering::SeqCst)) {
                released_at = Some(Instant::now());
            }
            match released_at {
                Some(t) if t.elapsed() >= cover => {
                    if glass_leave(g) {
                        break;
                    }
                    continue; // held again meanwhile
                }
                None if hold_at.elapsed() >= hold_cap => {
                    log_info!("slide hold not released within {HOLD_CAP_MS} ms; revealing");
                    break;
                }
                _ => {}
            }
        } else {
            let done = if !have_incoming {
                // First visit: hold frame 0 (already on screen) for the cover
                // window, then the synced reveal. Deliberately NO recompose —
                // blitting the (window-less) incoming would slide the outgoing
                // off to bare wallpaper. We just wait while the switch + first
                // paint land beneath.
                start.elapsed() >= cover
            } else if let Some(top) = fade_top {
                // The whole frame is one attribute call: DWM blends the two
                // overlays. The GDI crossfade below (now only the fallback) is
                // a full-frame BitBlt + AlphaBlend + present per frame (~15 ms
                // at 1080p, re-measured in review), so a 140 ms fade showed
                // 7-9 frames and ended late. A hold simply stops calling this:
                // the alpha freezes.
                let el = start.elapsed().as_secs_f64() * 1000.0;
                let tf = probe.on().then(Instant::now);
                let _ =
                    SetLayeredWindowAttributes(top, COLORREF(0), fade_alpha(el / dur), LWA_ALPHA);
                if let Some(tf) = tf {
                    if frame_us.is_empty() {
                        probe.mark("first_frame");
                    }
                    frame_us.push(tf.elapsed().as_micros() as u32);
                }
                el >= dur
            } else if back.is_invalid() {
                // No back buffer (GDI quota): presenting it would show garbage.
                // Frame 0 is on screen and the switch is done under it; reveal.
                true
            } else {
                let el = start.elapsed().as_secs_f64() * 1000.0;
                let t = (el / dur).min(1.0);
                let tf = probe.on().then(Instant::now);
                match req.mode {
                    WsAnim::Fade => {
                        // Fallback crossfade (no layered top overlay): whole
                        // frames, outgoing underneath, incoming alpha ramped on
                        // top. Both DDBs already bake in wallpaper + gaps, so
                        // the still regions stay rock-steady and only the
                        // windows fade.
                        let _ = BitBlt(backdc, 0, 0, w, h, outdc, 0, 0, SRCCOPY);
                        blend.SourceConstantAlpha = fade_alpha(t);
                        let _ = AlphaBlend(backdc, 0, 0, w, h, indc, 0, 0, w, h, blend);
                    }
                    WsAnim::Spring if has_wp => {
                        // Overshoot past the target then settle. Needs a
                        // wallpaper backdrop: at peak overshoot a thin band past
                        // the edge is exposed and must show the still
                        // wallpaper, not black.
                        let off = (target as f64 * ease_out_back(t)).round() as i32;
                        compose(off);
                    }
                    _ => {
                        // Slide (and spring with no wallpaper backdrop — fall
                        // back to the symmetric ease so the overshoot can't
                        // expose a black sliver).
                        let off = (target as f64 * ease_in_out_cubic(t)).round() as i32;
                        compose(off);
                    }
                }
                let _ = BitBlt(odc, 0, 0, w, h, backdc, 0, 0, SRCCOPY);
                if let Some(tf) = tf {
                    if frame_us.is_empty() {
                        probe.mark("first_frame");
                    }
                    frame_us.push(tf.elapsed().as_micros() as u32);
                }
                el >= dur
            };
            if done {
                // Off the glass BEFORE the teardown, so a switch arriving now
                // takes a fresh overlay instead of holding one that is leaving.
                if glass_leave(g) {
                    break;
                }
                continue; // a hold landed first: honour it
            }
        }
        next += frame_dur;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    glass_clear(k); // cap hit or reset: off the glass before the teardown too
    if holds > 0 {
        probe.note(format_args!("holds={holds}"));
    }

    SelectObject(backdc, ob);
    SelectObject(outdc, oo);
    if have_incoming {
        SelectObject(indc, oi);
        let _ = DeleteDC(indc);
    }
    if let Some(owp) = owp {
        SelectObject(wpdc, owp); // deselected: the crop goes back to `wpc` for reuse
    }
    let _ = DeleteObject(HGDIOBJ(back.0));
    let _ = DeleteDC(backdc);
    let _ = DeleteDC(outdc);
    let _ = DeleteDC(wpdc);
    ReleaseDC(overlay, odc);
    // Both deselected: `out` back to the manager now rather than after the
    // DwmFlush, `in` freed (it was the destination's snapshot, re-created
    // from the outgoing capture when that workspace is next left).
    drop(home);
    drop(req.in_bmp);
    // Sync the reveal to a DWM composition pass. The real windows were placed
    // (and styled) under the overlay long ago, but tearing the overlay down
    // off-vblank can expose a frame before DWM has recomposited them — the
    // "flash" where the snapshot vanishes a beat before the live window paints.
    // Block until the next composed frame so the overlay's last (target-aligned)
    // pixels and the live windows hand off on the same vblank: a clean reveal.
    match fade_top {
        // Fade: the bottom overlay (outgoing) goes first, under the top one,
        // which is opaque at the end, so nothing changes on screen; then the
        // same synced reveal of the top one. The other order would flash the
        // old workspace for a frame. (After a capped or frozen hold the top
        // one may be partly transparent: the live windows then show through
        // it for that one frame, which sits between the frozen frame and
        // the reveal.)
        Some(top) => {
            let _ = DestroyWindow(overlay);
            let _ = DwmFlush();
            let _ = DestroyWindow(top);
        }
        None => {
            let _ = DwmFlush();
            let _ = DestroyWindow(overlay);
        }
    }
    probe.mark("teardown");
    let (p50, max) = p50_max(&mut frame_us);
    probe.note(format_args!(
        "frames={} frame_p50={p50}us frame_max={max}us",
        frame_us.len()
    ));
}

/// Fade's top overlay (ANIM-9): the incoming snapshot on a second, layered
/// popup at `full`, above the outgoing overlay, shown at alpha 0 (so the screen
/// is still exactly the outgoing frame) and then painted. The fade then only
/// sets its alpha each frame. Run after the signal, so the manager's wait does
/// not grow. None on any failure, with nothing of it ever visible: the caller
/// falls back to the GDI crossfade.
unsafe fn fade_overlay_up(
    full: RECT,
    ex_style: WINDOW_EX_STYLE,
    hinst: HINSTANCE,
    indc: HDC,
) -> Option<HWND> {
    let (w, h) = (full.right - full.left, full.bottom - full.top);
    // Layered whatever the click-through setting (the alpha is SLWA), and
    // transparent to input exactly when the outgoing overlay is. Created
    // last, so it sits above that one: the system puts a new window at the
    // top of the z-order of its kind (both are topmost).
    let top = CreateWindowExW(
        ex_style | WS_EX_LAYERED,
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
    )
    .ok()?;
    // Alpha 0 BEFORE the first show: a layered window without attributes is
    // not drawn at all, and any alpha above 0 would show an unpainted surface.
    if SetLayeredWindowAttributes(top, COLORREF(0), 0, LWA_ALPHA).is_err() {
        let _ = DestroyWindow(top);
        return None;
    }
    let _ = ShowWindow(top, SW_SHOWNA);
    // Paint only now that it is shown: a blit to a hidden window's DC is lost
    // (the frame-0 trap in run_transition). Alpha 0 is still shown.
    let dc = GetDC(top);
    let painted = !dc.0.is_null() && BitBlt(dc, 0, 0, w, h, indc, 0, 0, SRCCOPY).is_ok();
    if !dc.0.is_null() {
        ReleaseDC(top, dc);
    }
    if !painted {
        let _ = DestroyWindow(top);
        return None;
    }
    let _ = UpdateWindow(top);
    // Its pixels composed before any alpha above 0 can reveal them.
    let _ = DwmFlush();
    Some(top)
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

/// The outgoing hide order: list order, except that the real OS foreground,
/// when it is one of them, goes last. SW_HIDE on the foreground makes Windows
/// activate a replacement, and in list order that could be another outgoing
/// window, hidden next, and so on: a chain of cross-process activations and
/// FOREGROUND events, each of which could bounce the switch back through
/// Cmd::Focused. Last, every other outgoing window is already hidden, so none
/// of them can be picked. Keyed on GetForegroundWindow, never ws.focused: the
/// foreground may be on another monitor or be an owned dialog, and then list
/// order is kept. No allocation.
fn hide_order(outgoing: &[isize], fg: isize) -> impl Iterator<Item = isize> + '_ {
    let last = (fg != 0 && outgoing.contains(&fg)).then_some(fg);
    outgoing
        .iter()
        .copied()
        .filter(move |&h| Some(h) != last)
        .chain(last)
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
        let fg = GetForegroundWindow().0 as isize;
        for h in hide_order(ws, fg) {
            mark_hidden_by_us(h);
            let _ = ShowWindow(hwnd_from(h), SW_HIDE);
        }
    }
    mgr.monitors[mi].active = n;
    {
        let ws = &mgr.monitors[mi].workspaces[n].windows;
        for &h in ws.iter() {
            if h == SCRATCHPAD_HWND.load(Ordering::Relaxed)
                && SCRATCHPAD_HIDDEN.load(Ordering::Relaxed)
            {
                mark_hidden_by_us(h);
                continue;
            }
            unmark_hidden_by_us(h);
            // SHOWNA: reveal without activating. SW_SHOW activated every
            // window it showed (one cross-process activate/deactivate chain
            // and FOREGROUND event each), only for focus_window(f) to pick the
            // real target right after (SWITCH-4). Not SetWindowPos with
            // SWP_SHOWWINDOW: the hide above is ShowWindow, which sends
            // WM_SHOWWINDOW(FALSE), and only ShowWindow sends the matching
            // TRUE that frameworks tracking visibility through it wait for.
            let _ = ShowWindow(hwnd_from(h), SW_SHOWNA);
        }
    }
    SUPPRESS.store(false, Ordering::Relaxed);
    // Instant placement — these windows were just unhidden; gliding them from a
    // stale position would jump.
    place_active_instant(mgr, mi);
}

/// Capture a monitor's current pixels into a GPU-backed off-screen bitmap (DDB,
/// not a DIB — so ~no process RAM). Returns the HBITMAP as an isize, or 0 on
/// failure. The caller hands it to a compositor thread: the glide worker frees
/// it; the transition worker hands it back as the left workspace's snapshot.
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
    let _ = BitBlt(
        mem,
        0,
        0,
        w,
        h,
        screen,
        full.left,
        full.top,
        SRCCOPY | CAPTUREBLT,
    );
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
    let mut probe = Probe::start("switch");
    probe.note(format_args!("mon={mi} ws={old}->{n}"));
    // Settle focus-follows-mouse from the START too, not only at the end: with
    // a click-through overlay the hover poll can see a window of the new
    // workspace while this switch is still running, and its FocusMouse,
    // processed after, would override the focus chosen below (the 2026-06-26
    // "FFM fought keyboard switches" fix). Cmd::FocusMouse re-checks it.
    bump_follow_settle();
    // Not gated on tiling: the transition is cosmetic and works in float mode too.
    let mode = WsAnim::from_cfg(&mgr.cfg);
    let mut want_slide = mgr.cfg.animations && mgr.cfg.animation_ms > 0 && mode != WsAnim::Off;
    let dir = if n > old { 1 } else { -1 };
    let hmon = mgr.monitors[mi].hmon;
    // Slide region = the tiling work area, NOT the full monitor. This excludes the
    // navbar, so the bar stays pinned above the slide instead of moving with it.
    let full = mgr.monitors[mi].work_area;
    let (w, h) = (full.right - full.left, full.bottom - full.top);

    // Empty to empty: nothing but wallpaper would move, so no capture, no
    // overlay handshake and no 200 ms of click-eating overlay (SWITCH-17).
    // "Empty" is `windows`, so floating, minimised and scratchpad windows all
    // count as content. The old workspace's snapshot is from when it last had
    // windows and would ghost them into its next slide: drop it. A slide left
    // pending by a handshake timeout would otherwise play later over this
    // switch.
    if want_slide
        && mgr.monitors[mi].workspaces[old].windows.is_empty()
        && mgr.monitors[mi].workspaces[n].windows.is_empty()
    {
        want_slide = false;
        let dropped = snap_remove(hmon, old);
        let cancelled = slide_cancel_pending();
        probe.note(format_args!(
            "empty=skip snap_removed={dropped} pending_cancelled={cancelled}"
        ));
    }
    // A slide already on this monitor's glass: run this switch under it (see
    // the overlay state notes). No capture of that half-slid frame, no wait
    // for the one worker to finish it, no second overlay replaying it.
    let held = if want_slide { slide_hold(hmon) } else { None };
    if let Some(v) = held {
        probe.note(format_args!("held={v:#x}"));
    }
    // Not holdable but still up on this monitor: it is on its way out (its
    // animation done, or its hold released) or past its hold window. Let its
    // teardown finish before capturing. A capture of it is the frozen frame,
    // not the windows now under it: the next slide's frame 0 would replay it
    // after the reveal (a jump back), and it could not be kept as a snapshot.
    // The one worker has to finish that teardown before it can pick up our
    // request anyway, so this costs at most the capture time.
    let slide_here =
        || SLIDE_BUSY.load(Ordering::SeqCst) && SLIDE_HMON.load(Ordering::SeqCst) == hmon;
    if want_slide && held.is_none() && slide_here() {
        let t = Instant::now();
        while slide_here() && t.elapsed() < std::time::Duration::from_millis(SLIDE_LEAVE_WAIT_MS) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        probe.mark("leave_wait");
    }
    // Store the captures the worker has handed back since the last switch.
    // Here, after the leave wait, so the return of a slide that wait just saw
    // finish is included: its capture may be `n`'s snapshot.
    snap_drain();
    // Sampled before the capture: whether it can include an overlay of ours.
    let busy = want_slide && held.is_none() && overlay_up_on(hmon);

    // Freeze the outgoing workspace BEFORE the switch, while it's still on screen,
    // along with the work-area-local rects of its tiled windows (so only the
    // windows slide and the wallpaper in the gaps stays put).
    let out = if want_slide && held.is_none() {
        Bmp::new(capture_monitor(full))
    } else {
        None
    };
    probe.mark("capture");

    // Push: the worker raises an overlay showing the outgoing image (frame 0 ==
    // current screen, so no visible change) and signals back once it covers the
    // monitor. We then do the real switch UNDERNEATH it — that's what stops the
    // destination workspace flashing before the animation. Both images move by
    // single-owner hand-off, never shared: the incoming one is the snapshot
    // from the last time we left `n`, taken out of the cache; the outgoing one
    // goes over uncopied and comes back through SNAP_RETURNS as `old`'s
    // snapshot. Always raise the overlay when we have an outgoing capture
    // — even on the FIRST visit to `n`, where there's no cached snapshot to slide
    // in. With an incoming image the worker animates (slide/spring/fade); without
    // one it briefly holds the outgoing frame to cover the switch + first paint,
    // then reveals. Either way the destination never flashes its background
    // before it repaints.
    if let Some(out) = out {
        let out_rects = ws_window_rects(mgr, mi, old, full);
        // TAKE, not copy (and `out` goes over uncopied too): each full-frame
        // dup_ddb (7 ms at 1080p per the ANIM audit) came off the manager's
        // pre-switch path. `n` becomes active now
        // and its snapshot is re-created from the outgoing capture when it is
        // next left, so removing it loses nothing. The same holds if
        // dispatch_slide supersedes this request and frees it unplayed.
        let (in_bmp, in_rects) = match snap_take(hmon, n, w, h) {
            Some((b, r)) => (Some(b), r),
            None => (None, Vec::new()), // first visit: cover-and-reveal, no slide image
        };
        // Both bitmaps are selected on the worker thread next: flush this
        // thread's GDI batch first (GdiFlush docs, objects shared across threads).
        let _ = GdiFlush();
        // The worker takes its still wallpaper backdrop from the capture
        // thread's cache (flat filmstrip when none is current).
        let k = SLIDE_GEN.fetch_add(1, Ordering::SeqCst) + 1;
        // Decide `old`'s snapshot before the capture can come back. Never an
        // image that may hold one of our overlays (a mid-slide or mid-glide
        // frame): it would slide in as `old` next time, so that drops the
        // previous one instead of keeping this.
        if busy {
            let dropped = snap_remove(hmon, old);
            probe.note(format_args!("nostore=overlay_busy snap_removed={dropped}"));
        } else {
            snap_keep(hmon, old, k);
        }
        dispatch_slide(SlideReq {
            out_bmp: out,
            in_bmp,
            out_rects,
            in_rects,
            hmon,
            old_ws: old,
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
            queued: probe_now(),
            gen: k,
            ex_style: overlay_ex_style(mgr.cfg.animation_ms),
        });
        probe.mark("dispatch");
        let up = wait_slide_overlay_up(k);
        probe.mark(if up {
            "overlay_up"
        } else if SLIDE_ABORT.load(Ordering::SeqCst) == k {
            "overlay_TIMEOUT"
        } else {
            "overlay_none"
        });
    }

    // The real, correct switch — instant placement, on this thread. Cannot fail.
    // Now hidden under the overlay (if sliding).
    switch_plain(mgr, mi, old, n);
    probe.mark("plain");
    queue_workspace_wallpaper(mgr, mi, n);
    queue_manager_state(mgr);

    // A captured switch settled `old`'s snapshot before its dispatch (the
    // capture comes back from the worker and is kept then). First visit to a
    // ws has no snapshot, so its first entry is a cover-hold, not a slide. A
    // held switch captured nothing, so `old`'s previous snapshot predates this
    // visit, which may have changed it: drop it.
    if held.is_some() {
        let dropped = snap_remove(hmon, old);
        probe.note(format_args!("nostore=held snap_removed={dropped}"));
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
    // Swap, not store: a switch on this monitor while focus was on ANOTHER
    // monitor used to overwrite STYLED_FOCUS without un-focusing that window,
    // and apply_styles then saw no change, so two windows looked focused. A
    // hidden previous focus is left alone: style_active restyles its whole
    // workspace when it is next shown.
    let prev = STYLED_FOCUS.swap(f, Ordering::Relaxed);
    if prev != 0
        && prev != f
        && IsWindow(hwnd_from(prev)).as_bool()
        && IsWindowVisible(hwnd_from(prev)).as_bool()
    {
        style_window(hwnd_from(prev), false, &mgr.cfg);
    }
    probe.mark("style");
    // Only now, with the switch done and styled underneath: the worker then
    // holds COVER_HOLD_MS more for the first paint and reveals.
    if let Some(v) = held {
        SLIDE_RELEASED.store(v, Ordering::SeqCst);
    }

    if f != 0 {
        focus_window(f);
        probe.mark("focus");
        check_focus_landed(f);
        if mgr.cfg.cursor_follows_focus {
            center_cursor_on(mgr, f);
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
    // New monitors / work areas: new crop targets, every old crop stale.
    wp_publish(&mgr.monitors, &mgr.cfg, WP_DEBOUNCE);
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
                // A scratchpad toggled away stays away, as on a switch: shown
                // here it popped back while SCRATCHPAD_HIDDEN stayed true, so
                // the next toggle press only re-hid it.
                if h == SCRATCHPAD_HWND.load(Ordering::Relaxed)
                    && SCRATCHPAD_HIDDEN.load(Ordering::Relaxed)
                {
                    mark_hidden_by_us(h);
                    continue;
                }
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

/// SPI_SETWORKAREA with no display change folded in: re-read the work areas
/// only. Appbars (YASB, docks), taskbar auto-hide and Explorer broadcast it,
/// often for a change that moves nothing, and `refresh_monitors` rebuilds
/// every workspace from scratch. A different monitor set means a display
/// change is under way: take the full path then.
unsafe fn refresh_work_areas(mgr: &mut Manager) {
    let fresh = enumerate_monitors();
    if fresh.len() != mgr.monitors.len()
        || fresh
            .iter()
            .zip(&mgr.monitors)
            .any(|(f, m)| f.hmon != m.hmon)
    {
        refresh_monitors(mgr);
        return;
    }
    let mut changed = false;
    for (f, m) in fresh.iter().zip(mgr.monitors.iter_mut()) {
        if f.base_work != m.base_work {
            m.base_work = f.base_work;
            changed = true;
        }
    }
    if !changed {
        log_debug!("work-area change broadcast: no monitor's work area moved");
        return;
    }
    // Cached snapshots show the old layout.
    snap_clear();
    reserve_bar(&mut mgr.monitors, &mgr.cfg);
    wp_publish(&mgr.monitors, &mgr.cfg, WP_DEBOUNCE);
    retile_all(mgr);
}

fn focused_index(ws: &Workspace) -> Option<usize> {
    if ws.windows.is_empty() {
        return None;
    }
    ws.windows.iter().position(|&h| h == ws.focused).or(Some(0))
}

unsafe fn toggle_scratchpad(mgr: &mut Manager) {
    if !mgr.cfg.scratchpad_enabled {
        return;
    }
    let h = SCRATCHPAD_HWND.load(Ordering::Relaxed);
    if h == 0 || !IsWindow(hwnd_from(h)).as_bool() {
        SCRATCHPAD_HWND.store(0, Ordering::Relaxed);
        SCRATCHPAD_HIDDEN.store(false, Ordering::Relaxed);
        SCRATCHPAD_PENDING_AT.store(GetTickCount64(), Ordering::Relaxed);
        mgr.pending_launch_mon = cursor_hmon();
        launch(&mgr.cfg.scratchpad_command);
        return;
    }
    if !SCRATCHPAD_HIDDEN.load(Ordering::Relaxed) && IsWindowVisible(hwnd_from(h)).as_bool() {
        SCRATCHPAD_HIDDEN.store(true, Ordering::Relaxed);
        mark_hidden_by_us(h);
        SUPPRESS.store(true, Ordering::Relaxed);
        let _ = ShowWindow(hwnd_from(h), SW_HIDE);
        SUPPRESS.store(false, Ordering::Relaxed);
        if let Some((mi, wi)) = mgr.locate(h) {
            let ws = &mut mgr.monitors[mi].workspaces[wi];
            if ws.focused == h {
                ws.focused = ws.windows.iter().copied().find(|x| *x != h).unwrap_or(0);
            }
            if wi == mgr.monitors[mi].active {
                retile_monitor(mgr, mi);
            }
        }
        return;
    }

    let to_mi = mgr.focused_mon;
    let to_wi = mgr.monitors[to_mi].active;
    if let Some((from_mi, from_wi)) = mgr.locate(h) {
        if (from_mi, from_wi) != (to_mi, to_wi) {
            let old = &mut mgr.monitors[from_mi].workspaces[from_wi];
            old.windows.retain(|x| *x != h);
            old.floating.retain(|x| *x != h);
            if old.focused == h {
                old.focused = old.windows.first().copied().unwrap_or(0);
            }
            let target = &mut mgr.monitors[to_mi].workspaces[to_wi];
            target.windows.push(h);
            target.floating.push(h);
            if from_wi == mgr.monitors[from_mi].active {
                retile_monitor(mgr, from_mi);
            }
        }
    }
    let target = &mut mgr.monitors[to_mi].workspaces[to_wi];
    if !target.windows.contains(&h) {
        target.windows.push(h);
    }
    if !target.floating.contains(&h) {
        target.floating.push(h);
    }
    target.focused = h;
    SCRATCHPAD_HIDDEN.store(false, Ordering::Relaxed);
    unmark_hidden_by_us(h);
    SUPPRESS.store(true, Ordering::Relaxed);
    let _ = ShowWindow(hwnd_from(h), SW_SHOWNA);
    SUPPRESS.store(false, Ordering::Relaxed);
    focus_window(h);
    retile_monitor(mgr, to_mi);
}

unsafe fn open_launcher_popup() {
    if !LAUNCHER_ENABLED.load(Ordering::Relaxed)
        || LAUNCHER_OPEN.swap(true, Ordering::Relaxed)
        || SYSMENU_OPEN.load(Ordering::Relaxed)
    {
        return;
    }
    let h = LAUNCHER_HWND.load(Ordering::Relaxed);
    if h != 0 {
        let _ = PostMessageW(hwnd_from(h), WM_LAUNCHER, WPARAM(LA_OPEN), LPARAM(0));
    } else {
        LAUNCHER_OPEN.store(false, Ordering::Relaxed);
    }
}

unsafe fn open_system_popup() {
    if !SYSMENU_ENABLED.load(Ordering::Relaxed)
        || SYSMENU_OPEN.swap(true, Ordering::Relaxed)
        || LAUNCHER_OPEN.load(Ordering::Relaxed)
    {
        return;
    }
    let h = SYSMENU_HWND.load(Ordering::Relaxed);
    if h != 0 {
        let _ = PostMessageW(hwnd_from(h), WM_SYSMENU, WPARAM(SM_OPEN), LPARAM(0));
    } else {
        SYSMENU_OPEN.store(false, Ordering::Relaxed);
    }
}

unsafe fn process_extra(mgr: &mut Manager, index: usize) {
    let Some(HotkeyDef {
        action, argument, ..
    }) = mgr.cfg.extra_hotkeys.get(index).cloned()
    else {
        return;
    };
    let core = match action.as_str() {
        "focus_next" => Some(Cmd::FocusDir(1)),
        "focus_prev" => Some(Cmd::FocusDir(-1)),
        "swap_next" => Some(Cmd::SwapDir(1)),
        "swap_prev" => Some(Cmd::SwapDir(-1)),
        "promote_master" => Some(Cmd::PromoteMaster),
        "shrink_master" => Some(Cmd::ResizeMaster(-0.05)),
        "grow_master" => Some(Cmd::ResizeMaster(0.05)),
        "toggle_tiling" => Some(Cmd::ToggleTiling),
        "toggle_float" => Some(Cmd::ToggleFloat),
        "close" | "close_window" => Some(Cmd::CloseFocused),
        "terminal" => Some(Cmd::LaunchTerminal),
        "browser" => Some(Cmd::LaunchBrowser),
        "switch_workspace" => argument
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .map(Cmd::Switch),
        "move_to_workspace" => argument
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .map(Cmd::MoveToWs),
        _ => None,
    };
    if let Some(cmd) = core {
        process(mgr, cmd);
        return;
    }
    match action.as_str() {
        "layout"
            if matches!(
                argument.as_str(),
                "dwindle" | "master" | "columns" | "grid" | "monocle"
            ) =>
        {
            mgr.cfg.layout = argument;
            retile_all(mgr);
        }
        "launch" | "command" => {
            mgr.pending_launch_mon = cursor_hmon();
            launch(&argument);
        }
        "scratchpad" => toggle_scratchpad(mgr),
        "launcher" => open_launcher_popup(),
        "system_menu" => open_system_popup(),
        "reload" => reload_config_now(),
        _ => {}
    }
}

/// Show local workspace `ws` on monitor `mi` and focus it: the tail of
/// Cmd::Switch and Cmd::BarCycle, and of a folded burst of them. Already
/// showing = move focus (and the cursor) to that monitor, which is also what a
/// burst that ends where it started does: focused_mon never changes without
/// focus moving with it.
unsafe fn show_workspace(mgr: &mut Manager, mi: usize, ws: usize) {
    mgr.focused_mon = mi;
    if ws != mgr.monitors[mi].active {
        // Shows the workspace, retiles, focuses + warps the cursor.
        switch_monitor_workspace(mgr, mi, ws);
    } else {
        let f = mgr.monitors[mi].workspaces[ws].focused;
        if f != 0 {
            focus_window(f);
            if mgr.cfg.cursor_follows_focus {
                center_cursor_on(mgr, f);
            }
        } else if mgr.cfg.cursor_follows_focus {
            let wa = mgr.monitors[mi].work_area;
            let _ = SetCursorPos((wa.left + wa.right) / 2, (wa.top + wa.bottom) / 2);
        }
    }
}

/// Drop `h` from whatever workspace holds it and re-tile if that workspace is
/// on screen. A no-op for an untracked handle.
unsafe fn untrack_window(mgr: &mut Manager, h: isize) {
    if SCRATCHPAD_HWND.load(Ordering::Relaxed) == h {
        SCRATCHPAD_HWND.store(0, Ordering::Relaxed);
        SCRATCHPAD_HIDDEN.store(false, Ordering::Relaxed);
    }
    unmark_hidden_by_us(h); // untracked -> marker would only go stale
    if let Some((mi, wi)) = mgr.locate(h) {
        // Out of MANAGED now, not at the sync after this command: the retile
        // below raises SUPPRESS, and an app re-showing `h` meanwhile would
        // pass for our own echo (SUPPRESS && is_managed) and be dropped,
        // leaving a visible window unmanaged with nothing logged.
        MANAGED.lock().unwrap().retain(|&x| x != h);
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

unsafe fn process(mgr: &mut Manager, cmd: Cmd) {
    match cmd {
        Cmd::Add(h, event) => {
            ev_count(EVC_ADD);
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
                    let hwnd = hwnd_from(h);
                    let rule = match_window_rule(hwnd);
                    let pending = std::mem::replace(&mut mgr.pending_launch_mon, 0);
                    let mut mi = mgr
                        .mon_by_hmon(pending)
                        .unwrap_or_else(|| monitor_index_for_window(mgr, hwnd));
                    let mut wi = mgr.monitors[mi].active;
                    if let Some(rule) = rule {
                        if let Some(rule_mon) = rule.monitor {
                            mi = rule_mon.min(mgr.monitors.len().saturating_sub(1));
                            wi = mgr.monitors[mi].active;
                        }
                        if let Some(rule_ws) = rule.workspace {
                            if rule.monitor.is_some() || mgr.cfg.per_monitor {
                                wi = rule_ws
                                    .min(mgr.monitors[mi].workspaces.len().saturating_sub(1));
                            } else {
                                (mi, wi) = mgr.global_to_ml(rule_ws);
                            }
                        }
                    }
                    let pending_at = SCRATCHPAD_PENDING_AT.load(Ordering::Relaxed);
                    let pending =
                        pending_at != 0 && GetTickCount64().saturating_sub(pending_at) <= 5_000;
                    if pending_at != 0 && !pending {
                        SCRATCHPAD_PENDING_AT.store(0, Ordering::Relaxed);
                    }
                    let class_matches = mgr.cfg.scratchpad_class.is_empty()
                        || rule_field(&mgr.cfg.scratchpad_class, &window_class(hwnd), false);
                    let scratch = mgr.cfg.scratchpad_enabled
                        && SCRATCHPAD_HWND.load(Ordering::Relaxed) == 0
                        && pending
                        && class_matches;
                    if scratch {
                        SCRATCHPAD_PENDING_AT.store(0, Ordering::Relaxed);
                        mi = mgr.focused_mon;
                        wi = mgr.monitors[mi].active;
                        SCRATCHPAD_HWND.store(h, Ordering::Relaxed);
                    }
                    let visible = wi == mgr.monitors[mi].active;
                    let ws = &mut mgr.monitors[mi].workspaces[wi];
                    ws.windows.push(h);
                    if scratch || should_float(hwnd, rule) {
                        ws.floating.push(h);
                    }
                    ws.focused = h;
                    if visible {
                        mgr.focused_mon = mi;
                        retile_monitor(mgr, mi);
                    } else {
                        mark_hidden_by_us(h);
                        SUPPRESS.store(true, Ordering::Relaxed);
                        let _ = ShowWindow(hwnd, SW_HIDE);
                        SUPPRESS.store(false, Ordering::Relaxed);
                    }
                }
                None => {
                    // Rejection probe: which check refused the window and which
                    // event asked, so the Add rate can be attributed. Re-walks
                    // the checks (debug only); the window may have changed since
                    // the guard ran, hence "now-passes".
                    if log_on(LOG_DEBUG) {
                        let hwnd = hwnd_from(h);
                        let trigger = match event {
                            EVENT_OBJECT_SHOW => "show",
                            EVENT_SYSTEM_FOREGROUND => "foreground",
                            _ => "other",
                        };
                        log_debug!(
                            "add rejected {h:#x} reason={} class='{}' event={trigger}",
                            manage_reject(hwnd).unwrap_or("now-passes"),
                            window_class(hwnd)
                        );
                    }
                }
            }
        }
        Cmd::Remove(h) => untrack_window(mgr, h),
        Cmd::RemoveHidden(h) => {
            // An app-driven hide (close-to-tray etc.), queued at event time. The
            // app may have shown the window again before this runs, and that
            // SHOW's Add is skipped as our own echo when SUPPRESS is set and
            // MANAGED still lists the window (it is only synced after the
            // batch). Untracking it then left a VISIBLE window unmanaged for
            // good (bench: 7 of 8 managed, once). Visible now = keep its slot.
            // The other order, a re-show landing during this untrack's own
            // retile, is covered in untrack_window.
            if !IsWindowVisible(hwnd_from(h)).as_bool() {
                untrack_window(mgr, h);
            }
        }
        Cmd::Focused(h) => {
            if let Some((mi, wi)) = mgr.locate(h) {
                // Act only if `h` still owns the foreground now, at processing
                // time. A queued FOREGROUND event can be stale by the time it
                // is processed: an echo of a switch's own focus_window that
                // queued behind the next Switch, or a click on an outgoing
                // window mid-switch. Following one of those bounced the user
                // back to a workspace they had just left, and two could keep a
                // monitor flipping. The whole command is a no-op then (no MRU
                // touch, no focused_mon), or a stale event would still point
                // focused_mon at the wrong monitor for MoveToWs / FocusDir.
                //
                // This NARROWS the 2026-07-13 decision (5477d14) to leave this
                // follow unguarded; it does not reverse it. The check is
                // deliberately neither IsWindowVisible nor was_hidden_by_us:
                // a Notepad++ second instance calls SetForegroundWindow on its
                // still-hidden window without showing it, and must still pull
                // the user to its workspace, which only this branch does
                // (Cmd::Add's follow needs a visible window). Root owner, so a
                // dialog the app raised right after still counts. Residual:
                // after a switch to an empty workspace, or a focus_window that
                // failed, a stale event for the old foreground still passes.
                let fg_root = GetAncestor(GetForegroundWindow(), GA_ROOTOWNER).0 as isize;
                if !focused_follow_allowed(fg_root, h) {
                    return;
                }
                touch_window_mru(h);
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
        Cmd::ActivateWindow(h) => {
            if let Some((mi, wi)) = mgr.locate(h) {
                mgr.focused_mon = mi;
                mgr.monitors[mi].workspaces[wi].focused = h;
                if wi != mgr.monitors[mi].active {
                    switch_monitor_workspace(mgr, mi, wi);
                }
                touch_window_mru(h);
                focus_window(h);
            }
        }
        Cmd::FocusMouse(h) => {
            // Queued before a programmatic focus change (a switch, keyboard
            // focus) but processed after it: the poll's own settle check ran
            // too early to see it. Drop it rather than yank focus back.
            if !focus_mouse_allowed(now_ms(), FOLLOW_SETTLE_MS.load(Ordering::Relaxed)) {
                return;
            }
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
        Cmd::Extra(index) => process_extra(mgr, index),
        Cmd::SetLayout(layout) => {
            if matches!(
                layout.as_str(),
                "dwindle" | "master" | "columns" | "grid" | "monocle"
            ) {
                mgr.cfg.layout = layout;
                retile_all(mgr);
            }
        }
        Cmd::ToggleScratchpad => toggle_scratchpad(mgr),
        Cmd::BarCycle(hmon, dir) => {
            // Wheel over the bar: previous/next workspace on that monitor (wraps).
            if let Some(mi) = mgr.mon_by_hmon(hmon) {
                let count = mgr.monitors[mi].workspaces.len();
                if count > 1 {
                    let cur = mgr.monitors[mi].active as i32;
                    let next = (cur + dir).rem_euclid(count as i32) as usize;
                    show_workspace(mgr, mi, next);
                }
            }
        }
        Cmd::Reload(cfg, full) => {
            // Redo only what the new values change: a colour-only save used to
            // unstyle, re-tile and restyle every window (a visible flash and a
            // full layout pass). Diffed here against mgr.cfg, not
            // in the watcher: apply_theme has already replaced UI_CFG, and
            // mgr.cfg also holds runtime changes the file resets (Alt+H/L
            // master_ratio, layout switches), which must still re-tile.
            let g = if full {
                config::ReloadGroups::full()
            } else {
                config::reload_groups(&mgr.cfg, &cfg)
            };
            mgr.cfg = *cfg;
            if g.wm || g.style || g.anim {
                // Gaps/opacity may have changed — cached snapshots are now stale.
                // Animations too: a switch with the slide off neither captures
                // nor drops the workspace it leaves, so a snapshot from before
                // an animations-off spell would slide in its old layout.
                snap_clear();
            }
            if g.wm {
                // Apply new workspace counts / mode, then recompute work areas
                // for the (possibly changed) bar height. Bars themselves are
                // recreated on the main thread (WM_RELOAD -> ensure_bars).
                distribute_workspaces(
                    &mut mgr.monitors,
                    mgr.primary,
                    mgr.cfg.workspaces,
                    mgr.cfg.per_monitor,
                );
                reserve_bar(&mut mgr.monitors, &mgr.cfg);
            }
            if g.wm || g.anim {
                // Bar height and the animation settings may have changed: new
                // crop targets (and whether to render at all), every old crop
                // stale.
                wp_publish(&mgr.monitors, &mgr.cfg, WP_DEBOUNCE);
            }
            if g.style {
                // Reset every window's styling so disabling opacity/borders
                // takes effect, then re-apply from scratch.
                SUPPRESS.store(true, Ordering::Relaxed);
                for m in &mgr.monitors {
                    for ws in &m.workspaces {
                        for &h in &ws.windows {
                            unstyle_window(hwnd_from(h));
                        }
                    }
                }
                SUPPRESS.store(false, Ordering::Relaxed);
                STYLED_FOCUS.store(0, Ordering::Relaxed);
            }
            if g.wm {
                retile_all(mgr);
            }
            if g.wm || g.style {
                style_all(mgr);
            }
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
            show_workspace(mgr, mi, local);
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
            // Carries the floating flag: sending a floated window to another
            // workspace used to silently re-tile it (review B-07).
            if !mgr.move_window(h, to_mi, to_local, None) {
                return;
            }
            // On the same monitor the source workspace is about to be hidden by
            // the switch below (the early return above rules out staying).
            // Reflowing it first only ran a glide handshake (capture, overlay,
            // DwmFlush, up to 250 ms) that the slide then covered and cut back
            // from, before the switch could even start (SWITCH-3). Its windows
            // are placed while hidden instead, after the switch. Another monitor
            // keeps the source workspace on screen, so it retiles as before.
            let reflow_now = move_needs_source_retile(to_mi, from_mi);
            if reflow_now {
                retile_monitor(mgr, from_mi);
            }
            // Follow the window: show its destination workspace, focus it, warp.
            mgr.focused_mon = to_mi;
            if to_local != mgr.monitors[to_mi].active {
                switch_monitor_workspace(mgr, to_mi, to_local);
                if !reflow_now {
                    // The capture the switch just took of from_a (its snapshot
                    // once the worker hands it back) still shows h in its old
                    // slot: never slide that in. Then lay out what is left.
                    snap_remove(mgr.monitors[from_mi].hmon, from_a);
                    place_hidden_workspace(mgr, from_mi, from_a);
                }
            } else {
                retile_monitor(mgr, to_mi);
                focus_window(h);
                if mgr.cfg.cursor_follows_focus {
                    center_cursor_on(mgr, h);
                }
                // The retile is posted: for a moment the cursor can sit over
                // the tile's old occupant, and the hover poll would focus it.
                bump_follow_settle();
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
            let (mi, a, h) = mgr.focused();
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
            let (_, _, h) = mgr.focused();
            if h != 0 {
                let _ = PostMessageW(hwnd_from(h), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        // The loop's update_bar does the work (and clears the flag again at
        // its top). Cleared here too, before any title is read, so a rename
        // from now on queues exactly one more refresh.
        Cmd::BarRefresh => bar_refresh_clear(&BAR_REFRESH_QUEUED),
        Cmd::RetileFor(h) => {
            // Resolved now, not when the event fired: commands queued ahead
            // of this one (MoveToWs, Remove, a switch) can move h.
            let target = retile_for_target(
                mgr.locate(h),
                |mi| mgr.monitors[mi].active,
                |mi, wi| mgr.monitors[mi].workspaces[wi].floating.contains(&h),
            );
            if let Some(mi) = target {
                retile_monitor(mgr, mi);
            }
        }
        Cmd::RefreshMonitors => refresh_monitors(mgr),
        Cmd::RefreshWorkAreas => refresh_work_areas(mgr),
        Cmd::DragUnmaximize(h, r) => {
            // The hook predicted this rect; do the parts that can block here.
            let hwnd = hwnd_from(h);
            if IsWindow(hwnd).as_bool() {
                // Straight to the predicted rect in one restore (INPUT-9): an
                // SW_RESTORE went to the old normal rect first, a full app
                // relayout, and commit_rect then resized it again. Should the
                // placement not take, SW_RESTORE as before: commit_rect on a
                // window still WS_MAXIMIZE would corrupt its restore state.
                let direct = unmaximize_to(hwnd, r);
                if !direct {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                // Exact-position fixup, and the whole move when the placement
                // fell back. After a direct restore the size already matches,
                // so this is at most a move.
                commit_rect(h, r.left, r.top, r.right - r.left, r.bottom - r.top);
                log_debug!(
                    "DragUnmaximize {h:#x} -> {},{} {}x{} direct={direct}",
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top
                );
            }
        }
        Cmd::DragPark(h) => {
            // Thumbnail drag began: park the real window far off-screen (size kept)
            // so the user sees only the live DWM mirror. Off-screen, NOT SW_HIDE — a
            // hidden window blanks its thumbnail. The drop (DragMoved/DragResized)
            // brings it back on-screen: the committed rect, or an un-park and
            // its tile (a tiled resize drop placed instantly).
            if IsWindow(hwnd_from(h)).as_bool() {
                // Where it came from, for a tiled resize drop's un-park. Read
                // before the park is posted; an already-parked window keeps
                // the origin it has.
                let from = window_rect_of(h);
                if !rect_parked(&from) {
                    mgr.park_origin = Some((
                        h,
                        POINT {
                            x: from.left,
                            y: from.top,
                        },
                    ));
                }
                // Mixed-DPI check (INPUT-5 C2): compare with the dpi the drop logs.
                log_debug!(
                    "DragPark {h:#x} from {},{} dpi={}",
                    from.left,
                    from.top,
                    window_dpi(hwnd_from(h))
                );
                SWP_CALLS.fetch_add(1, Ordering::Relaxed);
                let _ = SetWindowPos(
                    hwnd_from(h),
                    None,
                    -32000,
                    -32000,
                    0,
                    0,
                    // Same posting mode as the drop's commit, so the park can
                    // never land after it (FIFO in the app's queue).
                    foreign_swp_flags(
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSENDCHANGING,
                        ASYNC_WINDOW_POS.load(Ordering::Relaxed),
                    ),
                );
            }
        }
        Cmd::DragMoved(h, x, y, r) => {
            // Land the previewed rect FIRST — the real window never moved during the
            // drag (the thumbnail path even parked it off-screen), so this single
            // SetWindowPos is the actual move. It must precede every early-out:
            // floating, unmanaged, and tiling-off windows keep exactly this rect.
            // `before`: to tell when the (posted) commit has landed.
            let before = window_rect_of(h);
            commit_rect(h, r.left, r.top, r.right - r.left, r.bottom - r.top);
            if !mgr.tiling {
                return;
            }
            let Some((from_mi, from_wi)) = mgr.locate(h) else {
                return;
            };
            // Floating windows keep the rect the user dropped them at — but if
            // that was on ANOTHER monitor they must change owner, or switching
            // workspaces on the old monitor SW_HIDEs a window the user is
            // looking at on the new one (review B-06).
            if mgr.monitors[from_mi].workspaces[from_wi]
                .floating
                .contains(&h)
            {
                // Grabbed while its workspace was being switched away (the
                // overlay lets input through, so the drag can start in the gap
                // before the hide) and now SW_HIDE'd there. Re-homing it would
                // put a hidden window into a visible workspace, where nothing
                // shows it again: tracked but invisible, lost to the user.
                if from_wi != mgr.monitors[from_mi].active {
                    return;
                }
                let to_mi = monitor_index_for_point(mgr, POINT { x, y });
                if to_mi != from_mi {
                    let to_a = mgr.monitors[to_mi].active;
                    if mgr.move_window(h, to_mi, to_a, None) {
                        mgr.focused_mon = to_mi;
                        log_debug!("floating window {h:#x} re-homed {from_mi} -> {to_mi}");
                    }
                }
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
                let force = drop_retile_force_instant(glide_can_run(&mgr.cfg), h, before, r);
                retile_monitor_opts(mgr, from_mi, force);
            } else {
                // Move the window to the monitor it was dropped on, landing it
                // where it was dropped in the tiled order.
                let to_a = mgr.monitors[to_mi].active;
                let at = target.and_then(|t| {
                    mgr.monitors[to_mi].workspaces[to_a]
                        .windows
                        .iter()
                        .position(|&w| w == t)
                });
                if !mgr.move_window(h, to_mi, to_a, at) {
                    return;
                }
                mgr.focused_mon = to_mi;
                retile_monitor(mgr, from_mi);
                // Decided after the source retile: a glide started there makes
                // this one instant anyway, with no landing wait.
                let force = drop_retile_force_instant(glide_can_run(&mgr.cfg), h, before, r);
                retile_monitor_opts(mgr, to_mi, force);
            }
            focus_window(h);
        }
        Cmd::DragResized(h, rect) => {
            // Alt-resize carries the previewed rect; the native MOVESIZEEND path
            // passes None and the window already sits at its final rect.
            //
            // A tiled resize drop that will be placed instantly skips the commit
            // of the preview rect (INPUT-5): the retile's one SetWindowPos sizes
            // it into its slot, so the app relayouts once instead of twice
            // (preview, then slot). A parked window first moves back, position
            // only (no relayout), to where it was parked from, so it returns on
            // its own monitor and DPI. This is the one exception to "commit
            // before every early-out": every other drop still commits first
            // (floating, unmanaged and tiling-off windows keep that rect, and a
            // glide needs the window on screen at it for its capture).
            let before = window_rect_of(h);
            let origin = mgr
                .park_origin
                .take()
                .filter(|&(w, _)| w == h)
                .map(|(_, p)| p);
            // Glide or instant is decided here, once, and handed to the retile.
            let glide_wanted = glide_can_run(&mgr.cfg);
            let plan = rect.map(|_| {
                let parked = rect_parked(&before);
                // Parked with no recorded origin: commit, as before.
                let tiled = tile_target(mgr, h).is_some() && (!parked || origin.is_some());
                resize_drop_plan(tiled, parked, glide_wanted)
            });
            match (rect, plan, origin) {
                (Some(r), Some(ResizeDrop::Commit), _) => {
                    commit_rect(h, r.left, r.top, r.right - r.left, r.bottom - r.top);
                }
                (_, Some(ResizeDrop::UnparkOrigin), Some(o)) => {
                    log_debug!(
                        "DragResized {h:#x} un-park to {},{} dpi={}",
                        o.x,
                        o.y,
                        window_dpi(hwnd_from(h))
                    );
                    unpark_to(h, o);
                }
                _ => {}
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
            let force = match plan {
                Some(ResizeDrop::Commit) => drop_retile_force_instant(glide_wanted, h, before, r),
                // Skipped commit: the glide was ruled out when the plan was made.
                Some(_) => true,
                // MOVESIZEEND: nothing committed, nothing in flight.
                None => false,
            };
            retile_monitor_opts(mgr, mi, force);
            // A skipped commit must never leave the window parked. The plan
            // only skips it for a window this retile's layout contains (see
            // tile_target), so it has been sent its slot. With synchronous
            // placement that has also landed and is checked here; a posted one
            // cannot be read back yet, and re-committing the preview after it
            // would land the preview rect last, off the tile.
            if matches!(plan, Some(ResizeDrop::UnparkOrigin | ResizeDrop::NoUnpark))
                && !ASYNC_WINDOW_POS.load(Ordering::Relaxed)
                && rect_parked(&window_rect_of(h))
            {
                log_error!("resize drop left {h:#x} off-screen; committing the preview rect");
                commit_rect(h, r.left, r.top, r.right - r.left, r.bottom - r.top);
            }
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
                    center_cursor_on(mgr, target);
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
                        center_cursor_on(mgr, f);
                    }
                }
            }
            bump_follow_settle();
        }
        Cmd::MoveGeo(dir) => {
            if !mgr.tiling || mgr.monitors.is_empty() {
                return;
            }
            let (mi, a, h) = mgr.focused();
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
                    center_cursor_on(mgr, h);
                }
                // Posted retile: the neighbour may still be under the cursor.
                bump_follow_settle();
            } else if let Some(to_mi) = adjacent_monitor(mgr, mi, dir) {
                // Move the window to the adjacent monitor's active workspace.
                let ta = mgr.monitors[to_mi].active;
                if !mgr.move_window(h, to_mi, ta, None) {
                    return;
                }
                mgr.focused_mon = to_mi;
                retile_monitor(mgr, mi);
                retile_monitor(mgr, to_mi);
                focus_window(h);
                if mgr.cfg.cursor_follows_focus {
                    center_cursor_on(mgr, h);
                }
                bump_follow_settle();
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

/// Bar fonts, one per distinct monitor DPI. A single shared HFONT cannot serve
/// a mixed-DPI desk: the same `bar_font_size` has to become more physical
/// pixels on the 150% screen than on the 100% one, and GDI does not rescale a
/// font for us.
static BAR_FONTS: Mutex<Option<HashMap<u32, isize>>> = Mutex::new(None);

/// Drop every cached bar font. Main thread only (the bars' paint thread), so
/// deleting cannot race a paint. Call whenever the font config changes.
unsafe fn bar_fonts_clear() {
    if let Some(map) = BAR_FONTS.lock().unwrap().take() {
        for (_, f) in map {
            let _ = DeleteObject(HGDIOBJ(f as *mut c_void));
        }
    }
}

/// The bar font for one monitor DPI, built on first use. Main thread only.
unsafe fn bar_font_for(dpi: u32) -> isize {
    if let Some(f) = BAR_FONTS
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(&dpi).copied())
    {
        return f;
    }
    let f = make_bar_font(
        BAR_HEIGHT.load(Ordering::Relaxed) as i32,
        BAR_FONT_SIZE.load(Ordering::Relaxed) as i32,
        dpi,
    );
    BAR_FONTS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(dpi, f);
    f
}

/// Build one bar font at `dpi`. `height`/`font_size` are logical (100%) px.
unsafe fn make_bar_font(height: i32, font_size: i32, dpi: u32) -> isize {
    let size = dpi_px(
        if font_size > 0 {
            font_size
        } else {
            ((height as f32) * 0.5) as i32
        },
        dpi,
    )
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
    // NOTE: this used to also store BAR_CELL = height * 1.25, which ran after
    // apply_bar_statics on every startup and reload and therefore silently
    // discarded the documented `workspace_width` setting. BAR_CELL is config,
    // not a font metric — leave it alone.
    f.0 as isize
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
    let Some(ab) = g.as_mut().and_then(|m| m.get_mut(&key)) else {
        return;
    };
    let want = bar_cursor_over(pt, ab.x, ab.w, ab.y_cur as i32, ab.h, ab.tol, ab.strip);
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
        let _ = SetWindowPos(h, HWND_TOPMOST, x, y, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE);
    }
}

/// Is `pt` on the bar at (x, y, w, h), with `tol` grab slack (physical px on
/// its monitor) above and below, or in its reveal `strip`? The auto-hide
/// timer's test, shared with ensure_bars' snap decision.
fn bar_cursor_over(pt: POINT, x: i32, w: i32, y: i32, h: i32, tol: i32, strip: RECT) -> bool {
    let over_bar = pt.x >= x && pt.x < x + w && pt.y >= y - tol && pt.y < y + h + tol;
    let in_strip =
        pt.x >= strip.left && pt.x < strip.right && pt.y >= strip.top && pt.y < strip.bottom;
    over_bar || in_strip
}

/// May a bar that is going auto-hide jump straight to hidden (BAR-19)? Only
/// when a fullscreen app is the cause: it used to slide off over the app's
/// content for ~180-210 ms. Configured auto-hide keeps its slide, and a bar
/// the pointer is on (or reaching for, in the strip) is left to the timer.
fn bar_snap_hide(fullscreen: bool, autohide_cfg: bool, cursor_over: bool) -> bool {
    fullscreen && !autohide_cfg && !cursor_over
}

/// Fit bar `key`'s auto-hide state to new geometry `geo`, creating it (shown)
/// if new, and return (the y the bar sits at now, whether it is shown).
/// `snap` = straight to hidden. Otherwise slide progress is preserved, so
/// another monitor changing mode or a config/display rebuild never makes a
/// hidden or mid-slide bar jump.
fn ah_bar_update(key: isize, geo: &AhBar, snap: bool) -> (i32, bool) {
    let mut guard = AH_BARS.lock().unwrap();
    let state = guard
        .get_or_insert_with(HashMap::new)
        .entry(key)
        .or_insert(*geo);
    if snap {
        state.shown = false;
        state.y_cur = state.y_hidden as f64; // progress 1 below
    }
    let old_span = state.y_hidden - state.y_shown;
    let progress = if old_span == 0 {
        0.0
    } else {
        ((state.y_cur - state.y_shown as f64) / old_span as f64).clamp(0.0, 1.0)
    };
    *state = AhBar {
        y_cur: geo.y_shown as f64 + progress * (geo.y_hidden - geo.y_shown) as f64,
        shown: state.shown,
        ..*geo
    };
    (state.y_cur.round() as i32, state.shown)
}

/// True while `ensure_bars` is running; set again if something asks for another
/// pass while one is in flight. Plain atomics rather than a Mutex on purpose:
/// this guards RE-ENTRANCY on one thread, not access from several. Every caller
/// is on the main thread.
static ENSURE_BARS_BUSY: AtomicBool = AtomicBool::new(false);
static ENSURE_BARS_AGAIN: AtomicBool = AtomicBool::new(false);

/// Ask for a bar rebuild without doing it here.
///
/// This exists because `ensure_bars` calls `SetWindowPos` on the bars, and
/// Windows delivers `WM_DPICHANGED` **synchronously** to a window whose DPI
/// changes as a result. Calling `ensure_bars` straight out of a `WM_DPICHANGED`
/// handler therefore re-enters it, from inside its own `SetWindowPos`, once per
/// bar, per monitor — which wedged the main thread solid on a scale change.
/// A wedged main thread is not a cosmetic bug: the low-level hooks are
/// dispatched on it, so Windows drops them past `LowLevelHooksTimeout` and the
/// whole WM goes deaf. (2026-08-26; the watchdog reported it and could not fix
/// it, because the thread it needed to re-arm on was the wedged one.)
///
/// So: coalesce into one posted message and let the pump deliver it after the
/// DPI change has finished unwinding.
///
/// `display`: a display or DPI change (full bar rebuild + RefreshMonitors).
/// false = SPI_SETWORKAREA alone, which appbars and taskbar auto-hide
/// broadcast often, usually moving nothing: RefreshWorkAreas re-reads only the
/// work areas, where RefreshMonitors rebuilds every workspace (dwindle splits,
/// focus, snapshots, a ShowWindow per managed window). Either kind folded into
/// a pending request upgrades it to a display one if either asked.
unsafe fn request_bar_rebuild(display: bool) {
    if display {
        REBUILD_DISPLAY.store(true, Ordering::Relaxed);
    }
    if BARS_REBUILD_PENDING.swap(true, Ordering::Relaxed) {
        return; // one is already queued; N windows x N monitors collapse to one
    }
    let marker = MARKER_HWND.load(Ordering::Relaxed);
    if marker == 0 {
        BARS_REBUILD_PENDING.store(false, Ordering::Relaxed);
        return;
    }
    if PostMessageW(hwnd_from(marker), WM_REBUILD_BARS, WPARAM(0), LPARAM(0)).is_err() {
        BARS_REBUILD_PENDING.store(false, Ordering::Relaxed);
        // This post is the only way a display change reaches the manager (the
        // bars no longer queue RefreshMonitors themselves, TILE-11), so a
        // lost one would leave every tile on the old geometry, silently.
        log_error!("display change dropped: could not post the bar rebuild / monitor refresh");
    }
}
static BARS_REBUILD_PENDING: AtomicBool = AtomicBool::new(false);
/// The pending rebuild includes a display/DPI change. Main thread only, like
/// BARS_REBUILD_PENDING (every requester and WM_REBUILD_BARS run there).
static REBUILD_DISPLAY: AtomicBool = AtomicBool::new(false);

/// Re-entrancy-safe wrapper. Never called recursively: an inner request is
/// folded into one more pass by the outer call instead.
unsafe fn ensure_bars() {
    if ENSURE_BARS_BUSY.swap(true, Ordering::Relaxed) {
        ENSURE_BARS_AGAIN.store(true, Ordering::Relaxed);
        return;
    }
    // Bounded: geometry converges in one pass normally, two if a bar crossed a
    // DPI boundary while being placed. The cap makes a livelock impossible.
    for _ in 0..4 {
        ENSURE_BARS_AGAIN.store(false, Ordering::Relaxed);
        ensure_bars_inner();
        if !ENSURE_BARS_AGAIN.load(Ordering::Relaxed) {
            break;
        }
    }
    ENSURE_BARS_BUSY.store(false, Ordering::Relaxed);
}

/// DANGER, and the reason `ensure_bars` has a re-entrancy guard: this function
/// holds `BARS.lock()` across `SetWindowPos` and `CreateWindowExW`, both of
/// which dispatch messages to our own wndprocs SYNCHRONOUSLY. If one of those
/// handlers calls back in here, `BARS.lock()` deadlocks against itself — a
/// std Mutex is not reentrant — and the main thread stops pumping forever. That
/// is not a cosmetic failure: the low-level hooks are dispatched on this thread,
/// so Windows drops them and the whole WM goes deaf with no way back.
///
/// It happened, on 2026-08-26, the first time a user changed display scale.
/// Never call this directly from a wndproc; use `request_bar_rebuild`.
unsafe fn ensure_bars_inner() {
    // Logical (100%) values straight from the config; each monitor scales them
    // by its own DPI below, so a mixed-DPI desk gets a correctly sized bar on
    // both screens.
    let height_logical = BAR_HEIGHT.load(Ordering::Relaxed) as i32;
    if height_logical <= 0 {
        // Bar disabled: silence the hook's wheel routing.
        for slot in BARHIT_HWND.iter() {
            slot.store(0, Ordering::Relaxed);
        }
        BARS_HOT.store(false, Ordering::Relaxed);
        return;
    }
    let bottom = BAR_BOTTOM.load(Ordering::Relaxed);
    let floating = BAR_FLOATING.load(Ordering::Relaxed);
    let margin_logical = if floating {
        BAR_MARGIN.load(Ordering::Relaxed) as i32
    } else {
        0
    };
    let radius_logical = BAR_RADIUS.load(Ordering::Relaxed) as i32;
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
        // Configured auto-hide stays global. Fullscreen override is per monitor.
        let autohide_cfg = BAR_AUTOHIDE.load(Ordering::Relaxed);
        let fullscreen = monitor_has_fullscreen(hmon);
        let autohide = autohide_cfg || fullscreen;
        // Physical px for THIS monitor. reserve_bar computes the same number
        // from the same inputs; if these two ever diverge, every tile on a
        // scaled screen is offset by the difference.
        let dpi = monitor_dpi(hmon);
        let height = dpi_px(height_logical, dpi);
        let margin = dpi_px(margin_logical, dpi);
        let radius = dpi_px(radius_logical, dpi);
        let edge = dpi_px(2, dpi).max(1); // auto-hide reveal band / hidden overshoot
        let x = rcm.left + margin;
        let w = (rcm.right - rcm.left) - margin * 2;
        let y = if bottom {
            rcm.bottom - height - margin
        } else {
            rcm.top + margin
        };
        // Auto-hide geometry: the reveal band on the docked screen edge, and
        // where the bar parks while hidden.
        let strip = if bottom {
            RECT {
                left: rcm.left,
                top: rcm.bottom - edge,
                right: rcm.right,
                bottom: rcm.bottom,
            }
        } else {
            RECT {
                left: rcm.left,
                top: rcm.top,
                right: rcm.right,
                bottom: rcm.top + edge,
            }
        };
        let y_hidden = if bottom {
            rcm.bottom + edge
        } else {
            rcm.top - height - edge
        };
        let tol = dpi_px(8, dpi);
        let snap = autohide && {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            bar_snap_hide(
                fullscreen,
                autohide_cfg,
                bar_cursor_over(pt, x, w, y, height, tol, strip),
            )
        };
        let geo = AhBar {
            x,
            w,
            h: height,
            y_shown: y,
            y_hidden,
            y_cur: y as f64,
            shown: true,
            strip,
            tol,
        };
        // Auto-hide state BEFORE the first placement, so the bar lands at its
        // final y in one move. It used to be placed shown (SWP_SHOWWINDOW) and
        // then moved to its hidden/mid-slide y: one frame of bar over a
        // fullscreen app, and over content on any rebuild while hidden.
        let existing = bars.iter().find(|b| b.hmon == hmon).map(|b| b.hwnd);
        let mut ah = match existing {
            Some(key) if autohide => Some(ah_bar_update(key, &geo, snap)),
            _ => None,
        };
        let y_now = match ah {
            Some((y_cur, _)) => y_cur,
            // A new bar has no state yet: it starts shown unless it snaps.
            None if snap => y_hidden,
            None => y,
        };
        let hb = if let Some(key) = existing {
            let hb = hwnd_from(key);
            let _ = SetWindowPos(
                hb,
                HWND_TOPMOST,
                x,
                y_now,
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
                y_now,
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
        if autohide && ah.is_none() {
            // The same answer as y_now: a new key starts from `geo`.
            ah = Some(ah_bar_update(hb.0 as isize, &geo, snap));
        }
        // Floating bars get rounded corners via a window region (works on
        // Windows 10 and 11 alike). Classic bars clear any leftover region.
        if floating && radius > 0 {
            let rgn = CreateRoundRectRgn(0, 0, w + 1, height + 1, radius * 2, radius * 2);
            let _ = SetWindowRgn(hb, rgn, true); // system owns the region now
        } else {
            let _ = SetWindowRgn(hb, None, true);
        }
        // Publish the wheel hit rect for the LL mouse hook, only while the bar
        // is on screen (the auto-hide timer republishes as it shows / hides).
        let shown = ah.is_none_or(|(_, shown)| shown);
        barhit_publish(
            hb.0 as isize,
            shown.then_some(RECT {
                left: x,
                top: y,
                right: x + w,
                bottom: y + height,
            }),
        );
        if autohide {
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
const DEFAULT_BAR_ICON_PX: i32 = 20;
static BAR_ICON_PX_CFG: AtomicI32 = AtomicI32::new(DEFAULT_BAR_ICON_PX);
static BAR_WIDGET_GAP_CFG: AtomicI32 = AtomicI32::new(16);
// Bar icons live in BAR_ICON_CACHE, keyed on (exe path, pixel size). Keying on
// the path alone meant a size change in the settings GUI kept the old icons
// until restart, and — once per-monitor DPI arrived — that a 100% and a 150%
// monitor would share one bitmap (see review B-09). A size change resolves new
// keys; the old size is retired and freed only after the manager has published
// a snapshot without it (bar_icons_retire / bar_icons_sweep), so no paint can
// draw a destroyed handle and no size tried in the settings GUI stays behind.

/// Bumped by `update_bar` before it looks up any bar icon; BAR_SHOWN_GEN is
/// the value the snapshot now in BAR was built under. A handle retired at gen
/// g is in no snapshot once BAR_SHOWN_GEN > g: that build started after it
/// left the cache, so its lookups could not return it.
static BAR_ICON_GEN: AtomicU64 = AtomicU64::new(0);
static BAR_SHOWN_GEN: AtomicU64 = AtomicU64::new(0);
/// Bar icons dropped from BAR_ICON_CACHE, with the BAR_ICON_GEN they were
/// dropped at. Main thread only.
static BAR_ICON_RETIRED: Mutex<Vec<(u64, isize)>> = Mutex::new(Vec::new());

/// Retire every bar icon at a size no connected monitor's bar uses now
/// (bar_icon_size changed, a monitor left or changed scale). Main thread.
unsafe fn bar_icons_retire() {
    let sizes = icon_sizes(BAR_ICON_PX_CFG.load(Ordering::Relaxed));
    if sizes.is_empty() {
        return; // no monitor enumerated: keep everything rather than guess
    }
    let mut out = Vec::new();
    {
        let mut cache = BAR_ICON_CACHE.lock().unwrap();
        // Read under the cache lock, after the drops: see BAR_ICON_GEN.
        let gen = BAR_ICON_GEN.load(Ordering::SeqCst);
        cache.retain_live(
            |_, px, _| sizes.contains(&px),
            |icon| {
                if icon > 1 {
                    out.push((gen, icon));
                }
            },
        );
    }
    BAR_ICON_RETIRED.lock().unwrap().extend(out);
}

/// Free the retired bar icons no snapshot can hold any more. Main thread,
/// never inside a paint: `paint_bar` draws a clone of BAR, and only this
/// thread paints bars, so between paints no clone is in use.
unsafe fn bar_icons_sweep() {
    let shown = BAR_SHOWN_GEN.load(Ordering::SeqCst);
    BAR_ICON_RETIRED.lock().unwrap().retain(|&(gen, icon)| {
        if gen < shown {
            release_launcher_icon(icon);
        }
        gen >= shown
    });
}

/// Icon box for the bar currently being painted, in physical px.
#[inline]
fn bar_icon_px() -> i32 {
    dpi_px(BAR_ICON_PX_CFG.load(Ordering::Relaxed), bar_dpi())
}
#[inline]
fn bar_widget_gap() -> i32 {
    dpi_px(BAR_WIDGET_GAP_CFG.load(Ordering::Relaxed), bar_dpi())
}

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

/// Return cached app icon without shell work on manager thread. Cache misses are
/// queued to icon workers and temporarily render a placeholder.
/// `px` is the physical size the caller will draw at — icons are resolved at
/// exactly that size and never rescaled in DrawIconEx (see
/// plan/known-issues.md 2026-07-10), so it is part of the cache key.
/// Stamp 0: this runs per window on every bar update, too hot for a metadata
/// read, so an app updated mid-session keeps its old bar icon until restart.
unsafe fn bar_app_icon(hwnd: HWND, px: i32) -> isize {
    let Some(path) = window_exe(hwnd) else {
        return -1;
    };
    {
        let mut cache = BAR_ICON_CACHE.lock().unwrap();
        if let Some(icon) = cache.get(&path, px, 0) {
            return icon;
        }
        // 0 = queued: later lookups wait for the worker instead of re-queueing.
        cache.insert(&path, px, 0, 0, 0);
    }
    let job = IconJob::Bar(path, px);
    let mut q = ICON_QUEUE.lock().unwrap();
    if !q.contains(&job) {
        q.push_back(job);
        ICON_CV.notify_one();
    }
    0
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

// Bar wheel / click -> stats worker. The bar's window procedure runs on the
// main thread, which pumps the LL hooks, and each notch used to run COM
// activation plus audio-service RPCs there (an estimated 0.3-3 ms, more while
// MMDevAPI first loads; not measured). The bar now only adds to these, shows
// the change optimistically and wakes the worker, which applies it.
static VOL_DELTA: AtomicIsize = AtomicIsize::new(0); // pending change, whole percent
static MUTE_TOGGLES: AtomicU32 = AtomicU32::new(0); // pending mute clicks
static STATS_WAKE: LazyLock<(Mutex<bool>, Condvar)> =
    LazyLock::new(|| (Mutex::new(false), Condvar::new()));

fn stats_wake() {
    *STATS_WAKE.0.lock().unwrap() = true;
    STATS_WAKE.1.notify_one();
}

/// Sleep up to `timeout`, or until the bar queues a volume change.
fn stats_wait(timeout: std::time::Duration) {
    let guard = STATS_WAKE.0.lock().unwrap();
    let (mut pending, _) = STATS_WAKE
        .1
        .wait_timeout_while(guard, timeout, |p| !*p)
        .unwrap();
    *pending = false;
}

/// Queued wheel steps (whole percent) and mute clicks as (level change,
/// toggle once). An even number of clicks cancels out.
fn volume_drain(delta_pct: isize, toggles: u32) -> (f32, bool) {
    (delta_pct as f32 / 100.0, toggles % 2 == 1)
}

/// Stats worker: apply what the bar queued. The endpoint is resolved fresh,
/// as the poll does, rather than cached: a cached one would keep adjusting
/// the previous device after the default output changes. A failed call just
/// leaves the optimistic value for the next poll to correct.
unsafe fn volume_apply_pending() {
    let (step, toggle) = volume_drain(
        VOL_DELTA.swap(0, Ordering::Relaxed),
        MUTE_TOGGLES.swap(0, Ordering::Relaxed),
    );
    if step == 0.0 && !toggle {
        return;
    }
    let Some(v) = endpoint_volume() else {
        return;
    };
    if step != 0.0 {
        if let Ok(s) = v.GetMasterVolumeLevelScalar() {
            let ns = (s + step).clamp(0.0, 1.0);
            let _ = v.SetMasterVolumeLevelScalar(ns, std::ptr::null());
            STAT_VOL.store((ns * 100.0).round() as isize, Ordering::Relaxed);
        }
    }
    if toggle {
        if let Ok(m) = v.GetMute() {
            let nm = !m.as_bool();
            let _ = v.SetMute(nm, std::ptr::null());
            STAT_MUTE.store(nm, Ordering::Relaxed);
        }
    }
}

unsafe fn media_poll() {
    const PLAYERS: &[&str] = &[
        "spotify.exe",
        "vlc.exe",
        "music.ui.exe",
        "wmplayer.exe",
        "foobar2000.exe",
        "musicbee.exe",
    ];
    let handles = MANAGED.lock().unwrap().clone();
    let mut found = String::new();
    for h in handles {
        let hwnd = hwnd_from(h);
        let Some(exe) = window_exe(hwnd) else {
            continue;
        };
        let name = std::path::Path::new(&exe)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if !PLAYERS
            .iter()
            .any(|player| player.eq_ignore_ascii_case(name))
        {
            continue;
        }
        let mut title = window_title(hwnd);
        for suffix in [" - VLC media player", " - Windows Media Player"] {
            if let Some(value) = title.strip_suffix(suffix) {
                title = value.to_string();
            }
        }
        if !title.is_empty()
            && !title.eq_ignore_ascii_case("spotify")
            && !title.eq_ignore_ascii_case("spotify premium")
        {
            found = title;
            break;
        }
    }
    *MEDIA_TEXT.lock().unwrap() = found;
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
    let mut next_poll = Instant::now();
    loop {
        // Before the STATS_ON gate: a notch over a painted volume widget must
        // apply even while a reload is flipping the gate.
        unsafe { volume_apply_pending() };
        let now = Instant::now();
        if now < next_poll {
            // Woken early by the bar: back to sleep for the rest of the
            // interval, without re-running the polls.
            stats_wait(next_poll - now);
            continue;
        }
        if !STATS_ON.load(Ordering::Relaxed) {
            prev_net = None;
            next_poll = now + std::time::Duration::from_millis(500);
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
                    let rows = std::slice::from_raw_parts(t.Table.as_ptr(), t.NumEntries as usize);
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
            if MEDIA_ON.load(Ordering::Relaxed) {
                media_poll();
            } else {
                MEDIA_TEXT.lock().unwrap().clear();
            }
        }
        next_poll = Instant::now() + std::time::Duration::from_millis(2000);
    }
}

/// Rebuild the per-monitor bar snapshot and repaint only the bars that changed.
/// The clock is refreshed separately by each bar's 1s timer, so an idle desktop
/// causes no repaints from here.
unsafe fn update_bar(mgr: &Manager) {
    // Before the early return and before any title read: whatever path
    // consumes a BarRefresh, the next rename must be able to queue one, or the
    // title freezes with nothing logged (BAR-10).
    bar_refresh_clear(&BAR_REFRESH_QUEUED);
    if BARS.lock().unwrap().is_empty() {
        return;
    }
    // Before the first bar_app_icon below (see BAR_ICON_GEN).
    let icon_gen = BAR_ICON_GEN.fetch_add(1, Ordering::SeqCst) + 1;
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
        let labels: Vec<String> = slots
            .iter()
            .map(|&local| {
                let global = if mgr.cfg.per_monitor {
                    local
                } else {
                    mgr.ml_to_global(mi, local)
                };
                mgr.cfg
                    .workspace_icons
                    .get(global)
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        mgr.cfg
                            .workspace_names
                            .get(global)
                            .filter(|s| !s.is_empty())
                    })
                    .cloned()
                    .unwrap_or_else(|| (global + 1).to_string())
            })
            .collect();
        let mut occupied: u64 = 0;
        for (pill, &local) in slots.iter().enumerate().take(64) {
            if m.workspaces
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
        if let Some(slot) = BAR_TITLE_HWND.get(mi) {
            slot.store(fh, Ordering::Relaxed);
        }
        let title = if fh != 0 {
            window_title(hwnd_from(fh))
        } else {
            String::new()
        };
        // App buttons: the active workspace's windows with their exe icons
        // (cached per (exe, size), so this is a HashMap hit after the first
        // sighting). Resolved at THIS monitor's physical icon size — the
        // manager thread cannot read the paint-time DPI.
        let icon_px = dpi_px(BAR_ICON_PX_CFG.load(Ordering::Relaxed), monitor_dpi(m.hmon));
        let apps: Vec<BarApp> = if mgr.cfg.bar_show_apps {
            m.workspaces
                .get(m.active)
                .map(|ws| {
                    ws.windows
                        .iter()
                        .filter(|h| IsWindow(hwnd_from(**h)).as_bool())
                        .map(|&h| {
                            let hwnd = hwnd_from(h);
                            let label = window_exe(hwnd)
                                .and_then(|path| {
                                    std::path::Path::new(&path)
                                        .file_stem()
                                        .and_then(|s| s.to_str())
                                        .map(str::to_string)
                                })
                                .unwrap_or_else(|| window_title(hwnd));
                            BarApp {
                                hwnd: h,
                                icon: bar_app_icon(hwnd, icon_px),
                                label,
                            }
                        })
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
    for slot in BAR_TITLE_HWND.iter().skip(mgr.monitors.len()) {
        slot.store(0, Ordering::Relaxed);
    }
    let new = bar_data_from(
        &mgr.cfg,
        THEME_LIGHT.load(Ordering::Relaxed),
        mgr.tiling,
        mons,
    );

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
            || old.clock_format != new.clock_format
            || old.icon_mode != new.icon_mode
            || old.show_app_labels != new.show_app_labels
            || old.show_tooltips != new.show_tooltips
            || old.cpu_format != new.cpu_format
            || old.mem_format != new.mem_format
            || old.battery_format != new.battery_format
            || old.net_format != new.net_format
            || old.volume_format != new.volume_format
            || old.icon_cpu != new.icon_cpu
            || old.icon_mem != new.icon_mem
            || old.icon_battery != new.icon_battery
            || old.icon_net != new.icon_net
            || old.icon_volume != new.icon_volume
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
    BAR_SHOWN_GEN.store(icon_gen, Ordering::SeqCst);
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

/// A bar snapshot: everything config-derived, around the manager's `mons`.
/// update_bar and the startup seed both build through here, so the seed can
/// never disagree with the manager's first snapshot on anything but `mons`.
fn bar_data_from(cfg: &Config, light: bool, tiling: bool, mons: Vec<MonBar>) -> BarData {
    let (bg, fg, accent, inactive) = bar_colors(cfg, light);
    BarData {
        bg,
        fg,
        accent,
        inactive,
        clock_24h: cfg.bar_clock_24h,
        date_format: cfg.bar_date_format.clone(),
        clock_format: cfg.bar_clock_format.clone(),
        icon_mode: cfg.bar_icon_mode.clone(),
        show_app_labels: cfg.bar_show_app_labels,
        show_tooltips: cfg.bar_show_tooltips,
        cpu_format: cfg.bar_cpu_format.clone(),
        mem_format: cfg.bar_mem_format.clone(),
        battery_format: cfg.bar_battery_format.clone(),
        net_format: cfg.bar_net_format.clone(),
        volume_format: cfg.bar_volume_format.clone(),
        icon_cpu: cfg.bar_icon_cpu.clone(),
        icon_mem: cfg.bar_icon_mem.clone(),
        icon_battery: cfg.bar_icon_battery.clone(),
        icon_net: cfg.bar_icon_net.clone(),
        icon_volume: cfg.bar_icon_volume.clone(),
        layout: cfg.layout.clone(),
        tiling,
        left: zone_widgets(&cfg.bar_left, cfg),
        center: zone_widgets(&cfg.bar_center, cfg),
        right: zone_widgets(&cfg.bar_right, cfg),
        mons,
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

// app-button cell (icon + breathing room)

fn widget_format(template: &str, values: &[(&str, String)]) -> String {
    let mut out = template.to_string();
    for (key, value) in values {
        out = out.replace(&format!("{{{key}}}"), value);
    }
    out
}

fn widget_icon_text(mode: &str, icon: &str, text: String) -> String {
    match mode {
        "icon" if !icon.is_empty() => icon.to_string(),
        "both" if !icon.is_empty() && !text.is_empty() => format!("{icon} {text}"),
        _ => text,
    }
}

fn format_clock_widget(data: &BarData, st: &SYSTEMTIME) -> String {
    let (h12, ap) = to_12h(st.wHour);
    let mut fmt = data.clock_format.clone();
    if fmt.is_empty() {
        fmt = if data.clock_24h { "HH:mm" } else { "h:mm tt" }.to_string();
    }
    // Longest tokens first so HH is not partially consumed as H.
    fmt.replace("HH", &format!("{:02}", st.wHour))
        .replace("hh", &format!("{:02}", h12))
        .replace("mm", &format!("{:02}", st.wMinute))
        .replace("tt", ap)
        .replace('H', &st.wHour.to_string())
        .replace('h', &h12.to_string())
}

/// Text + colour-class for simple widgets. Formatting and text/icon composition
/// are config-driven; separators/spacers and composite widgets draw elsewhere.
unsafe fn bar_widget_text(
    wgt: BarWidget,
    data: &BarData,
    mb: Option<&MonBar>,
) -> Option<(String, bool)> {
    match wgt {
        BarWidget::Clock => Some((format_clock_widget(data, &GetLocalTime()), false)),
        BarWidget::Date => Some((format_date(&data.date_format, &GetLocalTime()), false)),
        BarWidget::Battery => {
            let value = STAT_BAT.load(Ordering::Relaxed);
            (value >= 0).then(|| {
                let text = widget_format(&data.battery_format, &[("value", value.to_string())]);
                (
                    widget_icon_text(&data.icon_mode, &data.icon_battery, text),
                    false,
                )
            })
        }
        BarWidget::Mem => {
            let value = STAT_MEM.load(Ordering::Relaxed);
            (value >= 0).then(|| {
                let text = widget_format(&data.mem_format, &[("value", value.to_string())]);
                (
                    widget_icon_text(&data.icon_mode, &data.icon_mem, text),
                    false,
                )
            })
        }
        BarWidget::Cpu => {
            let value = STAT_CPU.load(Ordering::Relaxed);
            (value >= 0).then(|| {
                let text = widget_format(&data.cpu_format, &[("value", value.to_string())]);
                (
                    widget_icon_text(&data.icon_mode, &data.icon_cpu, text),
                    false,
                )
            })
        }
        BarWidget::Net => {
            let down = STAT_NET_D.load(Ordering::Relaxed);
            let up = STAT_NET_U.load(Ordering::Relaxed);
            (down >= 0 && up >= 0).then(|| {
                let text = widget_format(
                    &data.net_format,
                    &[("down", fmt_rate(down)), ("up", fmt_rate(up))],
                );
                (
                    widget_icon_text(&data.icon_mode, &data.icon_net, text),
                    false,
                )
            })
        }
        BarWidget::Volume => {
            let value = STAT_VOL.load(Ordering::Relaxed);
            if value < 0 {
                return None;
            }
            if STAT_MUTE.load(Ordering::Relaxed) {
                Some((
                    widget_icon_text(&data.icon_mode, &data.icon_volume, "MUTE".to_string()),
                    true,
                ))
            } else {
                let text = widget_format(&data.volume_format, &[("value", value.to_string())]);
                Some((
                    widget_icon_text(&data.icon_mode, &data.icon_volume, text),
                    false,
                ))
            }
        }
        BarWidget::Media => {
            let text = MEDIA_TEXT.lock().unwrap().clone();
            (!text.is_empty()).then_some((text, false))
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
        BarWidget::Workspaces | BarWidget::Apps | BarWidget::Separator | BarWidget::Spacer => None,
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
        BarWidget::Apps => mb
            .map(|m| {
                m.apps
                    .iter()
                    .map(|app| {
                        bar_icon_px()
                            + 10
                            + if data.show_app_labels {
                                text_width(hdc, &app.label) + 8
                            } else {
                                0
                            }
                    })
                    .sum()
            })
            .unwrap_or(0),
        BarWidget::Spacer => bar_widget_gap() * 2,
        BarWidget::Separator => 1,
        _ => match bar_widget_text(wgt, data, mb) {
            Some((s, _)) => text_width(hdc, &s).min(avail.max(0)),
            None => 0,
        },
    }
}

/// Paint one widget with its left edge at `x`; returns the width consumed.
/// Records hit ranges (pills / app buttons / volume) into `lay` for the
/// wndproc's mouse handling.
#[allow(clippy::too_many_arguments)]
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
                let mut s: Vec<u16> = label.encode_utf16().collect();
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
                let nearest =
                    (((hx - x) as f32 / cell as f32).round() as i32).clamp(0, n - 1) as usize;
                let mut cr = RECT {
                    left: hx,
                    top: 0,
                    right: hx + cell,
                    bottom: h_px,
                };
                SetTextColor(hdc, COLORREF(data.bg));
                let mut s: Vec<u16> = mb.labels[nearest].encode_utf16().collect();
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
            let iy = (h_px - bar_icon_px()) / 2;
            let mut used = 0;
            for app in &mb.apps {
                let label_w = if data.show_app_labels {
                    text_width(hdc, &app.label) + 8
                } else {
                    0
                };
                let width = bar_icon_px() + 10 + label_w;
                let bx = x + used;
                if app.icon > 0 {
                    let _ = DrawIconEx(
                        hdc,
                        bx + 5,
                        iy,
                        HICON(app.icon as *mut c_void),
                        bar_icon_px(),
                        bar_icon_px(),
                        0,
                        None,
                        DI_NORMAL,
                    );
                } else {
                    draw_builtin_icon(hdc, "command", bx + 5, iy, bar_icon_px(), data.inactive);
                }
                if data.show_app_labels {
                    SetTextColor(hdc, COLORREF(data.fg));
                    let mut rect = RECT {
                        left: bx + bar_icon_px() + 12,
                        top: 0,
                        right: bx + width,
                        bottom: h_px,
                    };
                    let mut label: Vec<u16> = app.label.encode_utf16().collect();
                    DrawTextW(
                        hdc,
                        &mut label,
                        &mut rect,
                        DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
                    );
                }
                lay.apps.push((bx, bx + width, app.hwnd));
                used += width;
            }
            used
        }
        BarWidget::Spacer => bar_widget_gap() * 2,
        BarWidget::Separator => {
            let top = (h_px / 4).max(1);
            let line = RECT {
                left: x,
                top,
                right: x + 1,
                bottom: h_px - top,
            };
            let brush = CreateSolidBrush(COLORREF(data.inactive));
            FillRect(hdc, &line, brush);
            let _ = DeleteObject(HGDIOBJ(brush.0));
            1
        }
        _ => {
            let Some((s, dim)) = bar_widget_text(wgt, data, mb) else {
                return 0;
            };
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
/// Per-bar signature of everything the 1 s tick can reveal. Returns true when it
/// differs from the last tick for this bar (and records the new value), so an
/// idle desktop repaints roughly once a minute per monitor instead of once a
/// second. Per bar, not global: each bar has its own timer, and a shared
/// signature would let whichever fired first starve the others.
///
/// Main thread only — every bar timer runs there.
unsafe fn bar_tick_changed(h: HWND) -> bool {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    {
        let data = BAR.lock().unwrap();
        format_clock_widget(&data, &GetLocalTime()).hash(&mut hasher);
        format_date(&data.date_format, &GetLocalTime()).hash(&mut hasher);
    }
    for stat in [
        &STAT_CPU,
        &STAT_MEM,
        &STAT_BAT,
        &STAT_NET_D,
        &STAT_NET_U,
        &STAT_VOL,
    ] {
        stat.load(Ordering::Relaxed).hash(&mut hasher);
    }
    STAT_MUTE.load(Ordering::Relaxed).hash(&mut hasher);
    MEDIA_TEXT.lock().unwrap().hash(&mut hasher);
    let now = hasher.finish();
    let mut seen = BAR_TICK_SIG.lock().unwrap();
    seen.get_or_insert_with(HashMap::new)
        .insert(h.0 as isize, now)
        != Some(now)
}

static BAR_TICK_SIG: Mutex<Option<HashMap<isize, u64>>> = Mutex::new(None);

unsafe fn paint_bar(h: HWND) {
    // Publish this bar's DPI for the whole paint. Every `bar_icon_px()` /
    // `bar_widget_gap()` call below it reads this, so no call site has to
    // remember to scale. Safe as a plain static: all bar painting happens on
    // the main thread, one bar at a time.
    let dpi = window_dpi(h);
    BAR_PAINT_DPI.store(dpi, Ordering::Relaxed);
    // Before the clone below: the previous paint's clone is gone and this one
    // is not taken yet, the one point where no bar holds an icon handle.
    bar_icons_sweep();
    let mut ps = PAINTSTRUCT::default();
    let win_hdc = BeginPaint(h, &mut ps);
    let hmon = GetWindowLongPtrW(h, GWLP_USERDATA);
    let data = BAR.lock().unwrap().clone();

    let mut rc = RECT::default();
    let _ = GetClientRect(h, &mut rc);
    let h_px = rc.bottom - rc.top;
    let w = rc.right - rc.left;
    // Double buffer: the pill slide repaints at up to 100 Hz (a 10 ms timer);
    // direct painting flickers.
    let bb = backbuf_begin(win_hdc, w, h_px);
    let hdc = bb.as_ref().map(|b| b.dc).unwrap_or(win_hdc);

    let bg_brush = CreateSolidBrush(COLORREF(data.bg));
    FillRect(hdc, &rc, bg_brush);
    let _ = DeleteObject(HGDIOBJ(bg_brush.0));

    let font_raw = bar_font_for(dpi);
    let old_font = if font_raw != 0 {
        Some(SelectObject(hdc, HGDIOBJ(font_raw as *mut c_void)))
    } else {
        Some(SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT)))
    };
    SetBkMode(hdc, TRANSPARENT);

    let cell = dpi_px(BAR_CELL.load(Ordering::Relaxed) as i32, dpi);
    let pad = dpi_px(BAR_PADDING.load(Ordering::Relaxed) as i32, dpi);
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
            x += drew + bar_widget_gap();
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
        right = wx - bar_widget_gap();
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
                total += ww + bar_widget_gap();
            }
        }
        if total > 0 {
            total -= bar_widget_gap();
        }
        let mut cx = gap_l + ((avail - total).max(0)) / 2;
        for (wgt, ww) in data.center.iter().zip(widths) {
            if ww <= 0 {
                continue;
            }
            let _ = bar_widget_draw(hdc, *wgt, cx, h_px, ww, &data, mb, &mut lay, cell);
            cx += ww + bar_widget_gap();
        }
    }

    let pointer_over_bar = {
        let mut cursor = POINT::default();
        let mut window = RECT::default();
        GetCursorPos(&mut cursor).is_ok()
            && GetWindowRect(h, &mut window).is_ok()
            && cursor.x >= window.left
            && cursor.x < window.right
            && cursor.y >= window.top
            && cursor.y < window.bottom
    };
    if data.show_tooltips
        && !data.show_app_labels
        && pointer_over_bar
        && BAR_HOVER_HWND.load(Ordering::Relaxed) == h.0 as isize
    {
        let hovered = BAR_HOVER_APP.load(Ordering::Relaxed);
        if let Some((_, x1, _)) = lay.apps.iter().find(|(_, _, hwnd)| *hwnd == hovered) {
            if let Some(app) = mb.and_then(|m| m.apps.iter().find(|app| app.hwnd == hovered)) {
                let tw = text_width(hdc, &app.label);
                let left = (*x1 + 4).min((rc.right - tw - 16).max(0));
                let tip = RECT {
                    left,
                    top: 2,
                    right: (left + tw + 12).min(rc.right),
                    bottom: h_px - 2,
                };
                let brush = CreateSolidBrush(COLORREF(data.accent));
                let pen = CreatePen(PS_SOLID, 1, COLORREF(data.accent));
                let old_b = SelectObject(hdc, HGDIOBJ(brush.0));
                let old_p = SelectObject(hdc, HGDIOBJ(pen.0));
                let _ = RoundRect(hdc, tip.left, tip.top, tip.right, tip.bottom, 8, 8);
                SelectObject(hdc, old_p);
                SelectObject(hdc, old_b);
                let _ = DeleteObject(HGDIOBJ(pen.0));
                let _ = DeleteObject(HGDIOBJ(brush.0));
                SetTextColor(hdc, COLORREF(data.bg));
                let mut text_rect = RECT {
                    left: tip.left + 6,
                    right: tip.right - 6,
                    ..tip
                };
                let mut label: Vec<u16> = app.label.encode_utf16().collect();
                DrawTextW(
                    hdc,
                    &mut label,
                    &mut text_rect,
                    DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
                );
            }
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
            // Fast repaint while the highlight slides: 8 ms asked, but SetTimer
            // clamps to USER_TIMER_MINIMUM (10 ms), so at most 100 Hz.
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
            } else if w.0 == BAR_TIMER_ID && !bar_tick_changed(h) {
                // The 1 s tick exists for the clock and the stats. Astur's clock
                // format has no seconds token, so on an idle desktop this used
                // to repaint every bar every second to draw the identical
                // pixels — and `paint_bar` deep-clones the whole BarData each
                // time (review P-04). Skip when nothing it shows has changed.
                return LRESULT(0);
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
            // never reaches it natively). wparam: the signed wheel delta
            // (WHEEL_DELTA = 120 a notch, > 0 = up); lparam = screen x. Over
            // the volume widget the wheel adjusts volume; anywhere else it
            // cycles workspaces (if enabled).
            let delta = w.0 as isize as i32;
            let up = delta > 0;
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
                // Queued for the stats worker (no audio COM on this thread);
                // shown at once, corrected by the worker's real read.
                let step: isize = if up { 2 } else { -2 };
                VOL_DELTA.fetch_add(step, Ordering::Relaxed);
                let cur = STAT_VOL.load(Ordering::Relaxed);
                if cur >= 0 {
                    STAT_VOL.store((cur + step).clamp(0, 100), Ordering::Relaxed);
                }
                stats_wake();
                let _ = InvalidateRect(h, None, BOOL(0));
            } else if BAR_WHEEL_WS.load(Ordering::Relaxed) {
                let hmon = GetWindowLongPtrW(h, GWLP_USERDATA);
                // One workspace per full notch, not per event: a high-resolution
                // wheel or a touchpad sends many small deltas per notch, and
                // each used to be a whole switch (SWITCH-17).
                let steps = {
                    let mut acc = BAR_WHEEL_ACC.lock().unwrap();
                    let acc = acc
                        .get_or_insert_with(HashMap::new)
                        .entry(hmon)
                        .or_insert(0);
                    wheel_steps(acc, delta)
                };
                if steps != 0 {
                    // Up = previous workspace.
                    push_cmd(Cmd::BarCycle(hmon, -steps));
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (l.0 as u32 & 0xFFFF) as i16 as i32;
            let app = BAR_LAYOUTS
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|m| m.get(&(h.0 as isize)))
                .and_then(|lay| {
                    lay.apps
                        .iter()
                        .find(|(x0, x1, _)| x >= *x0 && x < *x1)
                        .map(|(_, _, hwnd)| *hwnd)
                })
                .unwrap_or(0);
            let changed = BAR_HOVER_HWND.swap(h.0 as isize, Ordering::Relaxed) != h.0 as isize
                || BAR_HOVER_APP.swap(app, Ordering::Relaxed) != app;
            if changed {
                let _ = InvalidateRect(h, None, BOOL(0));
            }
            // Ask for a WM_MOUSELEAVE once per hover. Without one the tooltip and
            // hover state stayed painted after the pointer left, until the next
            // repaint for any other reason (with idle tick skipping, up to a
            // minute). One pointer, so one tracked bar; TME_LEAVE is one-shot and
            // re-armed by the next move after each leave.
            if BAR_LEAVE_ARMED.swap(h.0 as isize, Ordering::Relaxed) != h.0 as isize {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: core::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: h,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut tme);
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            let me = h.0 as isize;
            let _ = BAR_LEAVE_ARMED.compare_exchange(me, 0, Ordering::Relaxed, Ordering::Relaxed);
            // Only if the hover is still ours: the pointer may already be on
            // another monitor's bar, whose move got here first.
            if BAR_HOVER_HWND
                .compare_exchange(me, 0, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                BAR_HOVER_APP.store(0, Ordering::Relaxed);
                let _ = InvalidateRect(h, None, BOOL(0));
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
                // Queued like the wheel above.
                MUTE_TOGGLES.fetch_add(1, Ordering::Relaxed);
                STAT_MUTE.fetch_xor(true, Ordering::Relaxed);
                stats_wake();
                let _ = InvalidateRect(h, None, BOOL(0));
            }
            LRESULT(0)
        }
        // Paint is double-buffered; a background erase would only add flicker.
        WM_ERASEBKGND => LRESULT(1),
        // No RefreshMonitors from here (TILE-11). Every bar gets this broadcast,
        // hidden bars of unplugged monitors too, so each used to queue its own
        // full refresh, the first ones before the bars were even rebuilt. The
        // marker gets the same broadcast and queues exactly one, after the
        // rebuild (WM_REBUILD_BARS).
        WM_DISPLAYCHANGE => DefWindowProcW(h, msg, w, l),
        // The scale of the monitor this bar sits on changed (Settings ->
        // Display -> Scale, or a dock/undock). ensure_bars re-derives the
        // height/margin/radius from the new DPI, and RefreshMonitors re-reserves
        // the work area and clears the stale-scale snapshots. The font cache is
        // keyed by DPI, so the next paint builds the new one by itself.
        // The scale of the monitor this bar sits on changed. Rebuilding here
        // would re-enter this handler from inside its own SetWindowPos — see
        // `request_bar_rebuild`. Ask, repaint, return.
        WM_DPICHANGED => {
            request_bar_rebuild(true);
            let _ = InvalidateRect(h, None, BOOL(0));
            LRESULT(0)
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
    let mut last_pt = POINT {
        x: i32::MIN,
        y: i32::MIN,
    };
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
    BAR_CELL.store(cfg.bar_workspace_width as isize, Ordering::Relaxed);
    BAR_ICON_PX_CFG.store(cfg.bar_icon_size, Ordering::Relaxed);
    BAR_WIDGET_GAP_CFG.store(cfg.bar_widget_gap, Ordering::Relaxed);
    *BAR_FONT_NAME.lock().unwrap() = cfg.bar_font_name.clone();
    BAR_FLOATING.store(cfg.bar_floating, Ordering::Relaxed);
    BAR_MARGIN.store(cfg.bar_margin as isize, Ordering::Relaxed);
    BAR_RADIUS.store(cfg.bar_radius as isize, Ordering::Relaxed);
    BAR_AUTOHIDE.store(cfg.bar_autohide, Ordering::Relaxed);
    BAR_WHEEL_WS.store(cfg.bar_wheel_ws, Ordering::Relaxed);
    NET_ON.store(cfg.bar_show_net, Ordering::Relaxed);
    VOL_ON.store(cfg.bar_show_volume, Ordering::Relaxed);
    MEDIA_ON.store(cfg.media_enabled && cfg.bar_show_media, Ordering::Relaxed);
    STATS_ON.store(
        cfg.bar_show_cpu
            || cfg.bar_show_mem
            || cfg.bar_show_battery
            || cfg.bar_show_net
            || cfg.bar_show_volume
            || (cfg.media_enabled && cfg.bar_show_media),
        Ordering::Relaxed,
    );
}

/// Startup config load. A file that exists but cannot be read is usually an
/// editor mid-save holding it open, which clears in milliseconds, so retry
/// briefly; after that run on the built-in defaults WITHOUT writing anything
/// (the user's file is still there) and hand the error back to be logged once
/// the log level is known. `config_watcher(true)` then keeps retrying it.
fn load_config_for_startup() -> (Config, Option<config::ConfigReadError>) {
    let mut last_err = None;
    for attempt in 0..4 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        match load_config() {
            Ok(cfg) => return (cfg, None),
            Err(e) => last_err = Some(e),
        }
    }
    (config::default_config(), last_err)
}

/// Quiet time after the last change to a config file before it is read. It
/// must span the settings GUI's back-to-back writes of both files (each a
/// truncate then a write): see review B-10 in `config_watcher`.
const CONFIG_QUIET: std::time::Duration = std::time::Duration::from_millis(120);
/// With change notifications working, still compare mtimes this often, for a
/// notification lost to a buffer overflow or never sent by a network share.
const CONFIG_BACKSTOP: std::time::Duration = std::time::Duration::from_secs(10);
/// The old fixed poll, kept for when notifications are unavailable and while a
/// read keeps failing (an editor holding the file mid-save clears in ms).
const CONFIG_POLL: std::time::Duration = std::time::Duration::from_millis(1000);

/// Notifier threads -> config watcher: count of matching directory changes.
static CONFIG_EVENTS: LazyLock<(Mutex<u32>, Condvar)> =
    LazyLock::new(|| (Mutex::new(0), Condvar::new()));
/// Cleared when a config directory cannot be watched; the watcher then polls.
static CONFIG_NOTIFY_OK: AtomicBool = AtomicBool::new(true);

fn config_event_wake() {
    *CONFIG_EVENTS.0.lock().unwrap() += 1;
    CONFIG_EVENTS.1.notify_one();
}

/// Wait up to `timeout` for a config-file change. True = one arrived.
fn config_event_wait(timeout: std::time::Duration) -> bool {
    let guard = CONFIG_EVENTS.0.lock().unwrap();
    let (mut n, _) = CONFIG_EVENTS
        .1
        .wait_timeout_while(guard, timeout, |n| *n == 0)
        .unwrap();
    std::mem::take(&mut *n) > 0
}

/// Trailing debounce: every event moves the deadline to event + quiet, so a
/// burst of writes produces one read, `quiet` after the last of them.
struct Debounce {
    quiet: std::time::Duration,
    due: Option<Instant>,
}

impl Debounce {
    fn event(&mut self, now: Instant) {
        self.due = Some(now + self.quiet);
    }

    /// How long to wait for the next event: until the deadline, or `idle`
    /// when nothing is pending.
    fn wait(&self, now: Instant, idle: std::time::Duration) -> std::time::Duration {
        self.due
            .map_or(idle, |due| due.saturating_duration_since(now))
    }
}

/// The file names in one `ReadDirectoryChangesW` result: a chain of
/// FILE_NOTIFY_INFORMATION records (u32 NextEntryOffset, u32 Action, u32
/// FileNameLength in bytes, then that many bytes of UTF-16). Bounds-checked,
/// so a short or corrupt buffer yields what parsed rather than a bad read.
fn notify_file_names(buf: &[u8]) -> Vec<String> {
    let u32_at = |o: usize| {
        buf.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let mut out = Vec::new();
    let mut at = 0usize;
    while let (Some(next), Some(len)) = (u32_at(at), u32_at(at + 8)) {
        let start = at + 12;
        let Some(bytes) = buf.get(start..start + len as usize) else {
            break;
        };
        let wide: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        out.push(String::from_utf16_lossy(&wide));
        if next == 0 {
            break;
        }
        at += next as usize;
    }
    out
}

/// Block on change notifications for `dir` and wake the config watcher when
/// one of `names` (lowercase file names) is written, resized, created or
/// renamed into place. Filtered by name: astur.log, state.conf, rescue.lst and
/// launcher-mru.conf live in the same directory and change far more often.
/// Returns if the directory cannot be watched; the watcher then polls.
fn config_dir_notifier(dir: std::path::PathBuf, names: Vec<String>) {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY,
        FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    let wide: Vec<u16> = dir
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call. Share-delete so
    // holding the handle never stops the user renaming or removing the folder.
    let handle = match unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    } {
        Ok(h) => h,
        Err(e) => {
            log_error!(
                "config watch: cannot open {} ({e}); polling every second",
                dir.display()
            );
            CONFIG_NOTIFY_OK.store(false, Ordering::Relaxed);
            config_event_wake();
            return;
        }
    };
    // DWORD-aligned, as ReadDirectoryChangesW requires.
    let mut buf = vec![0u32; 2048];
    loop {
        let mut got = 0u32;
        // SAFETY: synchronous call (no OVERLAPPED); `buf` is a live, aligned
        // buffer of exactly the length passed, and `got` outlives the call.
        let r = unsafe {
            ReadDirectoryChangesW(
                handle,
                buf.as_mut_ptr() as *mut c_void,
                (buf.len() * 4) as u32,
                BOOL(0),
                FILE_NOTIFY_CHANGE_LAST_WRITE
                    | FILE_NOTIFY_CHANGE_SIZE
                    | FILE_NOTIFY_CHANGE_FILE_NAME,
                Some(&mut got),
                None,
                None,
            )
        };
        if let Err(e) = r {
            log_error!(
                "config watch: {} stopped reporting changes ({e}); polling every second",
                dir.display()
            );
            CONFIG_NOTIFY_OK.store(false, Ordering::Relaxed);
            config_event_wake();
            break;
        }
        // SAFETY: the kernel wrote `got` bytes (at most the buffer's length)
        // into `buf`; viewing initialised u32s as bytes is always valid.
        let bytes = unsafe {
            std::slice::from_raw_parts(buf.as_ptr() as *const u8, (got as usize).min(buf.len() * 4))
        };
        // 0 bytes = the change list overflowed and the names were dropped:
        // one of ours may be among them, so let the mtime check decide.
        if got == 0
            || notify_file_names(bytes)
                .iter()
                .any(|n| names.contains(&n.to_lowercase()))
        {
            config_event_wake();
        }
    }
    // SAFETY: opened above and used by nothing else.
    unsafe {
        let _ = CloseHandle(handle);
    }
}

/// Watch the two config files and apply changes live, so editing + saving a
/// config takes effect without restarting Astur. `unread` = startup could not
/// read them and is running on defaults: treat the files as changed so the
/// first check tries again instead of waiting for the user's next save.
fn config_watcher(unread: bool) {
    use std::time::SystemTime;
    let wm = config_path("ASTUR_CONFIG", "astur.conf");
    let nav = config_path("ASTUR_NAVBAR", "navbar.conf");
    let mtime = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut last: (Option<SystemTime>, Option<SystemTime>) = if unread {
        (None, None)
    } else {
        (mtime(&wm), mtime(&nav))
    };
    // On-disk version whose read last failed, so a file that stays unreadable
    // (a UTF-16 save) logs once rather than every check.
    let mut failed: Option<(Option<SystemTime>, Option<SystemTime>)> = None;
    // One notifier per distinct directory (two when ASTUR_CONFIG/ASTUR_NAVBAR
    // point elsewhere), each blocked in the kernel until something changes.
    // This replaced a 1 s mtime poll: a save now applies ~CONFIG_QUIET after
    // it lands instead of 300-1300 ms later, and idle costs no wakeups.
    let mut dirs: Vec<(std::path::PathBuf, Vec<String>)> = Vec::new();
    for path in [&wm, &nav] {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
            CONFIG_NOTIFY_OK.store(false, Ordering::Relaxed);
            continue;
        };
        let name = name.to_string_lossy().to_lowercase();
        match dirs.iter_mut().find(|(d, _)| d == dir) {
            Some((_, names)) => names.push(name),
            None => dirs.push((dir.to_path_buf(), vec![name])),
        }
    }
    for (i, (dir, names)) in dirs.into_iter().enumerate() {
        spawn_named(&format!("config-notify-{i}"), move || {
            config_dir_notifier(dir, names)
        });
    }
    let mut debounce = Debounce {
        quiet: CONFIG_QUIET,
        due: None,
    };
    if unread {
        debounce.event(Instant::now());
    }
    loop {
        let idle = if failed.is_some() || !CONFIG_NOTIFY_OK.load(Ordering::Relaxed) {
            CONFIG_POLL
        } else {
            CONFIG_BACKSTOP
        };
        if config_event_wait(debounce.wait(Instant::now(), idle)) {
            debounce.event(Instant::now());
            continue;
        }
        // Quiet for CONFIG_QUIET since the last change, or the backstop /
        // poll came round: the mtimes decide whether anything changed.
        let settled = debounce.due.take().is_some();
        let seen = (mtime(&wm), mtime(&nav));
        if seen == last {
            continue;
        }
        if !settled {
            // Found by the backstop or the poll, not a notification: the
            // write may still be in progress, so settle first all the same.
            debounce.event(Instant::now());
            continue;
        }
        // Settled before reading. The settings GUI writes astur.conf and
        // navbar.conf in a loop; a read landing between the two used to apply
        // a MISMATCHED pair and then reload a second time — two full retiles,
        // two snapshot clears, a visible double flash (review B-10). The fixed
        // 300 ms settle is now the trailing CONFIG_QUIET debounce above, which
        // restarts on every write to either file and so spans both.
        let cfg = match load_config() {
            Ok(cfg) => cfg,
            Err(e) => {
                // Keep running on the current config and leave `last` alone, so
                // the next check (CONFIG_POLL while failing) retries: a sharing
                // violation mid editor-save clears in milliseconds, and giving
                // up here would drop the user's save for good.
                if failed != Some(seen) {
                    failed = Some(seen);
                    log_error!("config reload skipped, current settings kept: {e}");
                }
                continue;
            }
        };
        last = seen;
        failed = None;
        // Diff before apply_theme overwrites UI_CFG (the last applied config),
        // so only the subsystems a save actually touched are redone: a colour
        // edit no longer re-enumerates the launcher or rebuilds the bars. The
        // manager diffs its own groups against mgr.cfg in Cmd::Reload.
        let groups = UI_CFG
            .lock()
            .unwrap()
            .as_ref()
            .map_or_else(config::ReloadGroups::full, |old| {
                config::reload_groups(old, &cfg)
            });
        if groups.is_empty() {
            log_debug!("config file rewritten with identical settings — nothing to reload");
            continue;
        }
        log_info!("config changed on disk — reloading {groups:?}");
        // Statics the hooks/workers read directly.
        apply_hook_config(&cfg);
        apply_bar_statics(&cfg);
        apply_theme(&cfg);
        let launcher = LAUNCHER_HWND.load(Ordering::Relaxed);
        if launcher != 0 && groups.launcher {
            unsafe {
                let _ = PostMessageW(
                    hwnd_from(launcher),
                    WM_LAUNCHER,
                    WPARAM(LA_REFRESH),
                    LPARAM(0),
                );
            }
        }
        // Manager applies the rest; the marker (main thread) rebuilds the bars.
        push_cmd(Cmd::Reload(Box::new(cfg), false));
        let marker = MARKER_HWND.load(Ordering::Relaxed);
        if marker != 0 && groups.bar {
            unsafe {
                let _ = PostMessageW(hwnd_from(marker), WM_RELOAD, WPARAM(0), LPARAM(0));
            }
        }
    }
}

fn manager_loop(cfg: Config) {
    raise_current_thread(ThreadRole::Manager);
    let mut mgr = unsafe {
        let mut monitors = enumerate_monitors();
        // The main monitor (contains the origin 0,0) owns workspace 1 and gets
        // initial focus.
        let primary = primary_index(&monitors);
        distribute_workspaces(&mut monitors, primary, cfg.workspaces, cfg.per_monitor);
        if cfg.persist_state {
            for (monitor, active) in monitors.iter_mut().zip(load_active_state()) {
                monitor.active = active.min(monitor.workspaces.len().saturating_sub(1));
            }
        }
        reserve_bar(&mut monitors, &cfg);
        let mut m = Manager {
            monitors,
            focused_mon: primary,
            primary,
            tiling: cfg.start_tiled,
            cfg,
            pending_launch_mon: 0,
            park_origin: None,
        };
        assign_existing_windows(&mut m);
        // Warm the wallpaper cache now, not on the first animation. Until it
        // lands (one render, ~60-110 ms) glides place instantly.
        wp_publish(&m.monitors, &m.cfg, std::time::Duration::ZERO);
        queue_workspace_wallpaper(&m, primary, m.monitors[primary].active);
        if m.tiling {
            // Instant, never the glide (ANIM-16): the glide handshake (capture,
            // overlay up, DwmFlush; bounded by the 250 ms wait) held back the
            // first placement, the first bar update and the first command.
            // Owner-visible: monitor 0's startup resize now shows directly
            // instead of under an overlay, as monitors 1..n (GLIDE_BUSY already
            // set) always did. Reload and ToggleTiling keep their animation.
            for mi in 0..m.monitors.len() {
                place_active_instant(&m, mi);
            }
        }
        style_all(&m);
        m
    };
    sync_managed(&mgr);
    unsafe {
        update_bar(&mgr);
    }
    loop {
        let (cmd, depth, burst) = {
            let mut q = CMDQ.lock().unwrap();
            let c = loop {
                if let Some(c) = q.pop_front() {
                    break c;
                }
                q = CMDCV.wait(q).unwrap();
            };
            // Switches that queued up while this thread was busy (a wheel
            // flick over the bar, fast key taps, an IPC batch) only matter for
            // where they end: fold them to that one workspace now, instead of
            // N full switches each with a capture, an overlay handshake and
            // two workspaces of cross-process show/hide (SWITCH-17). Model
            // reads only while CMDQ is held; the switch runs after unlocking.
            let burst = if matches!(c, Cmd::Switch(_) | Cmd::BarCycle(..)) {
                let (target, folded) = mgr.fold_switches(&c, &mut q);
                (folded > 0).then_some((target, folded))
            } else {
                None
            };
            (c, q.len(), burst)
        };
        // Per-command probe: time in `process` and in the fixed tail (styles,
        // bar, index sync), SetWindowPos calls issued, queue depth left behind.
        // Everything here is skipped unless log_level = debug.
        let probe = probe_now().map(|t0| (t0, cmd.name(), SWP_CALLS.load(Ordering::Relaxed)));
        unsafe {
            match burst {
                Some((target, folded)) => {
                    log_debug!("switch burst: {} commands -> {target:?}", folded + 1);
                    if let Some((mi, ws)) = target {
                        show_workspace(&mut mgr, mi, ws);
                    }
                }
                None => process(&mut mgr, cmd),
            }
        }
        let t1 = probe.map(|_| Instant::now());
        unsafe {
            apply_styles(&mgr);
            update_bar(&mgr);
        }
        sync_managed(&mgr);
        if let (Some((t0, name, swp0)), Some(t1)) = (probe, t1) {
            log_debug!(
                "cmd {name} process={}us tail={}us swp={} queued={depth}",
                (t1 - t0).as_micros(),
                t1.elapsed().as_micros(),
                SWP_CALLS.load(Ordering::Relaxed) - swp0
            );
        }
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
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
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
        log_info!("rescued {n} window(s) hidden by a previous session");
    }
}

/// Whether the manager tracks `h` as of its last command batch.
fn is_managed(h: isize) -> bool {
    MANAGED.lock().unwrap().contains(&h)
}

/// Should a show/foreground event queue `Cmd::Add`? SUPPRESS is one global
/// flag the manager thread holds across a whole retile, while these events
/// arrive on the main thread. Gating every window on it dropped any window an
/// app opened mid-retile, for good (4 windows shown together: 1 tiled, 3 never
/// managed; measured with the hidden-desktop bench). Only a window we already
/// track can be the echo of our own show, so only those are skipped.
fn show_needs_add(suppressed: bool, tracked: bool) -> bool {
    !(suppressed && tracked)
}

/// A Cmd::BarRefresh is queued and not yet consumed (BAR-10). Set by the
/// NAMECHANGE arm of win_event_proc and by the icon worker when a bar icon
/// resolves (BAR-13); cleared by the manager before it reads any title (top
/// of update_bar, and the BarRefresh arm).
static BAR_REFRESH_QUEUED: AtomicBool = AtomicBool::new(false);

/// The window whose title each monitor's bar shows (slot = monitor index; 0 =
/// none), published by update_bar and read by the NAMECHANGE arm (BAR-9).
static BAR_TITLE_HWND: [AtomicIsize; MAX_BARS] = [const { AtomicIsize::new(0) }; MAX_BARS];

/// Does a rename of `h` need a bar refresh? When a bar shows its title: any
/// monitor's displayed window, not only the foreground one. The foreground-
/// only filter left a secondary monitor's title stale until some unrelated
/// command ran; indefinitely at idle. The foreground check stays as the
/// fallback, and is the only one for monitors past MAX_BARS. Relaxed loads:
/// a stale slot costs at most one extra refresh, and the next update_bar
/// republishes.
fn namechange_forward(h: isize, fg: impl FnOnce() -> isize, slots: &[AtomicIsize]) -> bool {
    h != 0 && (slots.iter().any(|s| s.load(Ordering::Relaxed) == h) || h == fg())
}

/// May a title change queue a Cmd::BarRefresh? At most one is pending: the
/// manager clears the flag before it reads titles, so a rename landing after
/// the clear queues exactly one more and none is lost. Gates ONLY BarRefresh;
/// Add, Remove, Focused and Reload must never sit behind it, or a title storm
/// would swallow a window. AcqRel on both sides: "clear, then read the title"
/// is a store-then-load that Relaxed does not order.
fn bar_refresh_gate(queued: &AtomicBool) -> bool {
    !queued.swap(true, Ordering::AcqRel)
}

/// Manager side of `bar_refresh_gate`: the pending refresh is being consumed.
fn bar_refresh_clear(queued: &AtomicBool) {
    queued.swap(false, Ordering::AcqRel);
}

/// The monitor a minimize or restore of a window re-tiles (TILE-3): its own,
/// and only while the window is tiled on that monitor's visible workspace.
/// Layouts are per monitor, so no other monitor depends on it, and an untracked
/// or floating window changes no layout at all. `loc` is locate(h); `active`
/// and `floating` read the manager for that monitor / workspace. Deliberately
/// no IsIconic test: the event can arrive before the state settles, and the
/// layout reads IsIconic itself.
fn retile_for_target(
    loc: Option<(usize, usize)>,
    active: impl Fn(usize) -> usize,
    floating: impl Fn(usize, usize) -> bool,
) -> Option<usize> {
    let (mi, wi) = loc?;
    (wi == active(mi) && !floating(mi, wi)).then_some(mi)
}

/// The WinEvent ranges Astur listens to, one SetWinEventHook each (EVENTS-1).
/// Exact on purpose. SHOW (0x8002) sits inside DESTROY..HIDE, and a second,
/// SHOW-only hook used to deliver every show twice: two Cmd::Add, two manager
/// ticks. Never widen a range over CREATE (0x8000) or REORDER (0x8004): both
/// fire constantly and nothing here handles them.
const WINEVENT_RANGES: [(u32, u32, &str); 5] = [
    (EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE, "destroy..hide"),
    // F11/borderless fullscreen and maximize/restore change top-level
    // geometry; title changes keep the bar's title widget current. The
    // callback filters both noisy events before doing any work.
    (
        EVENT_OBJECT_LOCATIONCHANGE,
        EVENT_OBJECT_NAMECHANGE,
        "locationchange..namechange",
    ),
    (
        EVENT_SYSTEM_FOREGROUND,
        EVENT_SYSTEM_FOREGROUND,
        "foreground",
    ),
    (
        EVENT_SYSTEM_MINIMIZESTART,
        EVENT_SYSTEM_MINIMIZEEND,
        "minimize",
    ),
    // Native (non-Alt) move/resize finished: re-tile so windows never overlap.
    (
        EVENT_SYSTEM_MOVESIZEEND,
        EVENT_SYSTEM_MOVESIZEEND,
        "movesizeend",
    ),
];

/// Bit i set = WINEVENT_RANGES[i] failed to register. Nonzero means Astur is
/// partly blind to window lifecycle; shown in the counters line.
static WINEVENT_HOOKS_FAILED: AtomicU32 = AtomicU32::new(0);

/// Does a queued Cmd::Focused(h) still describe the foreground? `fg_root` is
/// GetAncestor(GetForegroundWindow(), GA_ROOTOWNER) at processing time.
fn focused_follow_allowed(fg_root: isize, h: isize) -> bool {
    h != 0 && fg_root == h
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
    // Counted before the filter: the raw LOCATIONCHANGE rate (cursor moves
    // included) is the cost of having that hook installed at all.
    if event == EVENT_OBJECT_LOCATIONCHANGE {
        ev_count(EVC_LOCATION);
        if id_object == OBJID_CURSOR.0 {
            ev_count(EVC_LOCATION_CURSOR);
        }
    }
    if id_object != 0 || id_child != 0 || hwnd.0.is_null() {
        return;
    }
    match event {
        EVENT_OBJECT_SHOW => ev_count(EVC_SHOW),
        EVENT_OBJECT_HIDE => ev_count(EVC_HIDE),
        EVENT_OBJECT_DESTROY => ev_count(EVC_DESTROY),
        EVENT_SYSTEM_FOREGROUND => ev_count(EVC_FOREGROUND),
        EVENT_OBJECT_NAMECHANGE => ev_count(EVC_NAMECHANGE),
        _ => {}
    }
    let h = hwnd.0 as isize;
    // SHOW prefilter (EVENTS-1): a child, tool or no-activate window can never
    // be adopted (app_surface_reject refuses the same bits), and every menu,
    // tooltip and child control fires SHOW. GetWindowLongW reads the window
    // without sending it a message, so this is safe on a hung app. Residual,
    // accepted: a window that clears those bits right after its SHOW is
    // adopted on its next FOREGROUND instead.
    let show_junk = event == EVENT_OBJECT_SHOW
        && show_rejected_by_style(
            GetWindowLongW(hwnd, GWL_STYLE) as u32,
            GetWindowLongW(hwnd, GWL_EXSTYLE) as u32,
        );
    let tracked_fullscreen = fullscreen_window_tracked(h);
    let fullscreen_changed = match event {
        // Skips the fullscreen probe, but not the stale-entry removal it would
        // have done (such a window is never fullscreen): a recycled HWND must
        // not pin a monitor's bar in fullscreen mode.
        EVENT_OBJECT_SHOW if show_junk => tracked_fullscreen && remove_fullscreen_window(h),
        // Window is definitely leaving visible fullscreen state. Remove directly:
        // EVENT callbacks may run before IsIconic/IsWindowVisible settles.
        EVENT_OBJECT_HIDE | EVENT_OBJECT_DESTROY | EVENT_SYSTEM_MINIMIZESTART
            if tracked_fullscreen =>
        {
            remove_fullscreen_window(h)
        }
        // F11/maximize geometry changes arrive here. Ignore background resize
        // noise unless window is foreground or already tracked fullscreen.
        EVENT_OBJECT_LOCATIONCHANGE if GetForegroundWindow() == hwnd || tracked_fullscreen => {
            refresh_fullscreen_window(hwnd)
        }
        EVENT_OBJECT_SHOW | EVENT_SYSTEM_FOREGROUND | EVENT_SYSTEM_MINIMIZEEND => {
            refresh_fullscreen_window(hwnd)
        }
        _ => false,
    };
    if fullscreen_changed {
        request_bar_mode_refresh();
    }
    match event {
        EVENT_OBJECT_SHOW => {
            // Someone made it visible — whoever hid it, the marker is stale now
            // (and a later app-driven hide must untrack it again).
            // Unconditional and first, before the prefilter below.
            unmark_hidden_by_us(h);
            if show_junk {
                ev_count(EVC_SHOW_STYLE);
            } else if show_needs_add(SUPPRESS.load(Ordering::Relaxed), is_managed(h)) {
                push_cmd(Cmd::Add(h, event));
            }
        }
        EVENT_OBJECT_NAMECHANGE => {
            // The bar's title widget used to freeze between manager commands:
            // `update_bar` ran only after a Cmd, and the bar's own 1 s repaint
            // draws from the cached snapshot (review B-08). Switching a browser
            // tab or opening another file left a stale title on screen, which
            // reads as "the bar is frozen".
            // Only displayed windows' titles are shown (see namechange_forward),
            // so filter here and keep everything else off the queue. No bar,
            // nothing to refresh.
            if BAR_HEIGHT.load(Ordering::Relaxed) > 0
                && namechange_forward(
                    hwnd.0 as isize,
                    || GetForegroundWindow().0 as isize,
                    &BAR_TITLE_HWND,
                )
            {
                if bar_refresh_gate(&BAR_REFRESH_QUEUED) {
                    push_cmd(Cmd::BarRefresh);
                } else {
                    ev_count(EVC_BAR_REFRESH_FOLDED);
                }
            }
        }
        EVENT_SYSTEM_FOREGROUND => {
            // Foreground events refire for the same window; collapse repeats so
            // the manager doesn't re-run locate + styling for no change.
            if LAST_FG.swap(h, Ordering::Relaxed) == h {
                return;
            }
            push_cmd(Cmd::Focused(h));
            if show_needs_add(SUPPRESS.load(Ordering::Relaxed), is_managed(h)) {
                push_cmd(Cmd::Add(h, event));
            }
        }
        EVENT_OBJECT_HIDE => {
            // Untrack only hides the APP performed (close-to-tray etc.). Hides
            // Astur performed for a workspace switch are marked in HIDDEN_BY_US;
            // SUPPRESS alone misses the tail of the batch (async delivery), and
            // untracking those orphaned live windows on hidden workspaces.
            if !SUPPRESS.load(Ordering::Relaxed) && !was_hidden_by_us(h) {
                push_cmd(Cmd::RemoveHidden(h));
            }
        }
        EVENT_OBJECT_DESTROY => {
            // A destroyed window is gone for real — always untrack (a Remove for
            // an untracked hwnd is a no-op, so this is safe even mid-switch).
            unmark_hidden_by_us(h);
            push_cmd(Cmd::Remove(h));
        }
        EVENT_SYSTEM_MINIMIZESTART | EVENT_SYSTEM_MINIMIZEEND => {
            // Only h's own monitor can change (TILE-3). This used to re-place
            // every window on every monitor for any minimize anywhere, which
            // also un-maximized and re-snapped windows elsewhere as a side
            // effect; nothing relied on that.
            push_cmd(Cmd::RetileFor(h));
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
            return Some(if shift {
                Cmd::SwapDir(1)
            } else {
                Cmd::FocusDir(1)
            });
        }
        if vk == hk.focus_prev {
            return Some(if shift {
                Cmd::SwapDir(-1)
            } else {
                Cmd::FocusDir(-1)
            });
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
        0x0D => Some(if shift {
            Cmd::LaunchBrowser
        } else {
            Cmd::LaunchTerminal
        }), // Enter
        0x25 => Some(if shift {
            Cmd::MoveGeo(Dir::Left)
        } else {
            Cmd::FocusGeo(Dir::Left)
        }), // Left
        0x26 => Some(if shift {
            Cmd::MoveGeo(Dir::Up)
        } else {
            Cmd::FocusGeo(Dir::Up)
        }), // Up
        0x27 => Some(if shift {
            Cmd::MoveGeo(Dir::Right)
        } else {
            Cmd::FocusGeo(Dir::Right)
        }), // Right
        0x28 => Some(if shift {
            Cmd::MoveGeo(Dir::Down)
        } else {
            Cmd::FocusGeo(Dir::Down)
        }), // Down
        _ => None,
    }
}

/// Resolve a hotkey to a command. User-defined bindings override built-ins.
/// Resolver returns only small command/index values: no allocation on hook path.
fn resolve_hotkey(vk: u32, shift: bool, ctrl: bool) -> Option<Cmd> {
    if let Some(binding) = EXTRA_HOTKEYS
        .lock()
        .unwrap()
        .iter()
        .find(|b| b.vk == vk && b.shift == shift && b.ctrl == ctrl)
        .copied()
    {
        return Some(Cmd::Extra(binding.index));
    }
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
const LA_REFRESH: usize = 11; // F5: rebuild installed/custom app list
const LA_OPEN_SWITCHER: usize = 12; // Alt+Tab replacement: window-only mode

// Theme (COLORREF is 0x00BBGGRR). Forte blue #366382 accent on a dark surface;
// minimal chrome (thin frame, subtle divider) for a clean omarchy/rofi look.
const LAUNCHER_BG: u32 = 0x0016_1616;
const LAUNCHER_FG: u32 = 0x00E6_E6E6;
const LAUNCHER_DIM: u32 = 0x0089_8989;
const LAUNCHER_SELBG: u32 = 0x0082_6333; // #366382
const LAUNCHER_SELFG: u32 = 0x00FF_FFFF;
const LAUNCHER_FRAME: u32 = 0x0033_2A26; // subtle blue-tinted 1px frame
const LAUNCHER_DIVIDER: u32 = 0x0029_2929; // muted divider under the query row
const DEFAULT_LAUNCHER_W: i32 = 660;
const DEFAULT_LAUNCHER_WIDE_W: i32 = 1060; // Tab column view (clamped to the work area)
const DEFAULT_LAUNCHER_H: i32 = 452;
const LAUNCHER_COLHDR: i32 = 22; // wide-mode column-header row height
const COL_DATE_W: i32 = 150; // "Modified" column
const COL_SIZE_W: i32 = 90; // "Size" column (right-aligned)
const DEFAULT_LAUNCHER_ROW_H: i32 = 40;
const DEFAULT_LAUNCHER_PAD: i32 = 16;
const LAUNCHER_HEADER: i32 = 54; // query row height
const DEFAULT_LAUNCHER_ICON_PX: i32 = 32; // per-row app icon box (Start-Menu-ish size)
const DEFAULT_LAUNCHER_SEL_RADIUS: i32 = 12; // rounded selection pill

// Hot-reloaded popup geometry. Hook-visible enable flags stay atomic; richer
// config is read only by popup/menu threads through UI_CFG.
static UI_CFG: Mutex<Option<Config>> = Mutex::new(None);
static LAUNCHER_ENABLED: AtomicBool = AtomicBool::new(true);
/// Woken when the config changes so the (disabled) IPC worker can re-check
/// `ipc_enabled` immediately instead of polling for it. The 30 s timeout is a
/// backstop, not the mechanism.
static IPC_WAKE: LazyLock<(Mutex<()>, Condvar)> =
    LazyLock::new(|| (Mutex::new(()), Condvar::new()));

/// Set while a popup is repositioning itself, so a `WM_DPICHANGED` raised BY
/// that reposition does not recurse into it. The launcher and the system menu
/// are never open at the same time, so one flag covers both.
static POPUP_PLACING: AtomicBool = AtomicBool::new(false);

/// Window that had the foreground when the picker opened — the paste target.
static LAUNCHER_PREV_FG: AtomicIsize = AtomicIsize::new(0);
static SYSMENU_ENABLED: AtomicBool = AtomicBool::new(true);
static ALT_TAB_REPLACE: AtomicBool = AtomicBool::new(false);
static ALT_SWITCHER_MODE: AtomicBool = AtomicBool::new(false);
static LA_W_CFG: AtomicI32 = AtomicI32::new(DEFAULT_LAUNCHER_W);
static LA_WIDE_W_CFG: AtomicI32 = AtomicI32::new(DEFAULT_LAUNCHER_WIDE_W);
static LA_H_CFG: AtomicI32 = AtomicI32::new(DEFAULT_LAUNCHER_H);
static LA_ROW_H_CFG: AtomicI32 = AtomicI32::new(DEFAULT_LAUNCHER_ROW_H);
static LA_PAD_CFG: AtomicI32 = AtomicI32::new(DEFAULT_LAUNCHER_PAD);
static LA_ICON_CFG: AtomicI32 = AtomicI32::new(DEFAULT_LAUNCHER_ICON_PX);
static LA_SEL_RADIUS_CFG: AtomicI32 = AtomicI32::new(DEFAULT_LAUNCHER_SEL_RADIUS);
static SM_W_CFG: AtomicI32 = AtomicI32::new(380);
static POPUP_OPACITY_CFG: AtomicI32 = AtomicI32::new(100);
static POPUP_RADIUS_CFG: AtomicI32 = AtomicI32::new(16);
static POPUP_BORDER_CFG: AtomicI32 = AtomicI32::new(1);
static POPUP_FONT_DIRTY: AtomicBool = AtomicBool::new(true);

// Every popup metric is a LOGICAL (100%) pixel in the config and comes back
// here as a PHYSICAL pixel for the monitor the popup is on. `UI_DPI` is set by
// launcher_place / sysmenu_layout before anything is measured or drawn, so no
// call site has to remember to scale. Both popups are single windows that live
// on one monitor at a time, which is what makes one global sound.
#[inline]
fn la_w() -> i32 {
    dpi_px(LA_W_CFG.load(Ordering::Relaxed), ui_dpi())
}
#[inline]
fn la_wide_w() -> i32 {
    dpi_px(LA_WIDE_W_CFG.load(Ordering::Relaxed), ui_dpi())
}
#[inline]
fn la_h() -> i32 {
    dpi_px(LA_H_CFG.load(Ordering::Relaxed), ui_dpi())
}
#[inline]
fn la_row_h() -> i32 {
    dpi_px(LA_ROW_H_CFG.load(Ordering::Relaxed), ui_dpi())
}
#[inline]
fn la_pad() -> i32 {
    dpi_px(LA_PAD_CFG.load(Ordering::Relaxed), ui_dpi())
}
#[inline]
fn la_icon_px() -> i32 {
    dpi_px(LA_ICON_CFG.load(Ordering::Relaxed), ui_dpi())
}
#[inline]
fn la_sel_radius() -> i32 {
    dpi_px(LA_SEL_RADIUS_CFG.load(Ordering::Relaxed), ui_dpi())
}
#[inline]
fn popup_radius() -> i32 {
    dpi_px(POPUP_RADIUS_CFG.load(Ordering::Relaxed), ui_dpi())
}
#[inline]
fn popup_border() -> i32 {
    dpi_px(POPUP_BORDER_CFG.load(Ordering::Relaxed), ui_dpi())
}
/// Query-row height (logical 54).
#[inline]
fn la_header() -> i32 {
    dpi_px(LAUNCHER_HEADER, ui_dpi())
}
/// Wide-mode column-header row height (logical 22).
#[inline]
fn la_colhdr() -> i32 {
    dpi_px(LAUNCHER_COLHDR, ui_dpi())
}
#[inline]
fn col_date_w() -> i32 {
    dpi_px(COL_DATE_W, ui_dpi())
}
#[inline]
fn col_size_w() -> i32 {
    dpi_px(COL_SIZE_W, ui_dpi())
}

/// Adopt the DPI of the monitor a popup is about to appear on. Must run before
/// any la_* metric is read for that appearance; the popup font is rebuilt when
/// the scale actually changes.
unsafe fn set_ui_dpi(dpi: u32) {
    if UI_DPI.swap(dpi, Ordering::Relaxed) != dpi {
        POPUP_FONT_DIRTY.store(true, Ordering::Release);
    }
}

unsafe fn shape_popup(hwnd: HWND, width: i32, height: i32) {
    let radius = popup_radius().max(1);
    let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, radius * 2, radius * 2);
    let _ = SetWindowRgn(hwnd, region, BOOL(1));
}

// ---- popup theme (dark / light / auto) -------------------------------------
// The popups (launcher + system menu) read their palette at paint time, so a
// theme change in astur.conf hot-reloads without touching the windows.
#[derive(Clone, Copy)]
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
    bg: 0x00F7_F4F2,       // #F2F4F7 — soft cool grey-white surface
    fg: 0x001A_1614,       // #14161A near-black text (strong contrast)
    dim: 0x0068_615C,      // #5C6168 readable muted grey
    selbg: LAUNCHER_SELBG, // same Forte-blue accent both themes
    selfg: 0x00FF_FFFF,
    frame: 0x00D4_CCC6,   // #C6CCD4 cool border
    divider: 0x00E6_E1DD, // #DDE1E6
};
static THEME_LIGHT: AtomicBool = AtomicBool::new(false);
fn pal() -> Pal {
    let base = if THEME_LIGHT.load(Ordering::Relaxed) {
        PAL_LIGHT
    } else {
        PAL_DARK
    };
    let cfg = UI_CFG.lock().unwrap();
    let Some(cfg) = cfg.as_ref() else { return base };
    Pal {
        bg: cfg.popup_bg.unwrap_or(base.bg),
        fg: cfg.popup_fg.unwrap_or(base.fg),
        dim: cfg.popup_muted.unwrap_or(base.dim),
        selbg: cfg.popup_accent.unwrap_or(base.selbg),
        selfg: cfg.popup_accent_fg.unwrap_or(base.selfg),
        frame: cfg.popup_border.unwrap_or(base.frame),
        divider: cfg.popup_border.unwrap_or(base.divider),
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
    LAUNCHER_ENABLED.store(cfg.launcher_enabled, Ordering::Relaxed);
    SYSMENU_ENABLED.store(cfg.system_menu_enabled, Ordering::Relaxed);
    ALT_TAB_REPLACE.store(cfg.alt_tab_replacement, Ordering::Relaxed);
    LA_W_CFG.store(cfg.launcher_width, Ordering::Relaxed);
    LA_WIDE_W_CFG.store(cfg.launcher_wide_width, Ordering::Relaxed);
    LA_H_CFG.store(cfg.launcher_height, Ordering::Relaxed);
    LA_ROW_H_CFG.store(cfg.launcher_row_height, Ordering::Relaxed);
    LA_PAD_CFG.store(cfg.launcher_padding, Ordering::Relaxed);
    LA_ICON_CFG.store(cfg.launcher_icon_size, Ordering::Relaxed);
    LA_SEL_RADIUS_CFG.store(cfg.launcher_selection_radius, Ordering::Relaxed);
    SM_W_CFG.store(cfg.system_menu_width, Ordering::Relaxed);
    POPUP_OPACITY_CFG.store(cfg.popup_opacity, Ordering::Relaxed);
    POPUP_RADIUS_CFG.store(cfg.popup_radius, Ordering::Relaxed);
    POPUP_BORDER_CFG.store(cfg.popup_border_width, Ordering::Relaxed);
    let font_changed = UI_CFG.lock().unwrap().as_ref().is_none_or(|old| {
        old.popup_font_name != cfg.popup_font_name
            || old.popup_font_size != cfg.popup_font_size
            || old.popup_font_weight != cfg.popup_font_weight
    });
    *UI_CFG.lock().unwrap() = Some(cfg.clone());
    if font_changed {
        POPUP_FONT_DIRTY.store(true, Ordering::Release);
    }
    // Let a sleeping IPC worker notice ipc_enabled without waiting out its
    // backstop timeout.
    IPC_WAKE.1.notify_all();
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
    let Ok(user32) = GetModuleHandleW(w!("user32.dll")) else {
        return;
    };
    let Some(f) = GetProcAddress(user32, s!("SetWindowCompositionAttribute")) else {
        return;
    };
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
    let configured = (POPUP_OPACITY_CFG.load(Ordering::Relaxed).clamp(20, 100) * 255 / 100) as u8;
    let alpha = if on && dark {
        configured.min(236)
    } else {
        configured
    };
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

static CLIPBOARD_ITEMS: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

unsafe fn clipboard_get_text(h: HWND) -> Option<String> {
    const CF_UNICODETEXT: u32 = 13;
    if IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() || OpenClipboard(h).is_err() {
        return None;
    }
    let result = (|| {
        let data = GetClipboardData(CF_UNICODETEXT).ok()?;
        let global = windows::Win32::Foundation::HGLOBAL(data.0);
        let ptr = GlobalLock(global) as *const u16;
        if ptr.is_null() {
            return None;
        }
        let mut len = 0usize;
        while len < 32_768 && *ptr.add(len) != 0 {
            len += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
        let _ = GlobalUnlock(global);
        let text = text.trim().to_string();
        (!text.is_empty()).then_some(text)
    })();
    let _ = CloseClipboard();
    result
}

/// True when the clipboard owner has asked history tools to leave this copy
/// alone. Password managers, banking sites and terminals set one of these
/// formats; Windows' own clipboard history, Ditto and ClipClip all honour them,
/// and a clipboard history that does not is a way to leak a master password
/// onto the screen (review S-01).
///
/// Two conventions are in play. `ExcludeClipboardContentFromMonitorProcessing`
/// and `Clipboard Viewer Ignore` mean "skip this entirely" by their presence.
/// `CanIncludeInClipboardHistory` and `CanUploadToCloudClipboard` are DWORD
/// opt-outs: present and zero means no. Formats are registered once — the
/// registration is process-wide and the ids never change.
unsafe fn clipboard_is_sensitive(h: HWND) -> bool {
    static FORMATS: OnceLock<[u32; 4]> = OnceLock::new();
    let ids = *FORMATS.get_or_init(|| {
        let reg = |name: PCWSTR| RegisterClipboardFormatW(name);
        [
            reg(w!("ExcludeClipboardContentFromMonitorProcessing")),
            reg(w!("Clipboard Viewer Ignore")),
            reg(w!("CanIncludeInClipboardHistory")),
            reg(w!("CanUploadToCloudClipboard")),
        ]
    });
    // Presence alone is the signal for the first two.
    for id in ids.iter().take(2) {
        if *id != 0 && IsClipboardFormatAvailable(*id).is_ok() {
            return true;
        }
    }
    // The last two carry a DWORD; 0 = "don't". Reading needs the clipboard open.
    for id in ids.iter().skip(2) {
        if *id == 0 || IsClipboardFormatAvailable(*id).is_err() {
            continue;
        }
        if OpenClipboard(h).is_err() {
            // Cannot check: treat as sensitive. Missing one copy in the history
            // is a far smaller cost than capturing a password.
            return true;
        }
        let deny = (|| {
            let data = GetClipboardData(*id).ok()?;
            let global = windows::Win32::Foundation::HGLOBAL(data.0);
            let ptr = GlobalLock(global) as *const u32;
            let value = (!ptr.is_null()).then(|| *ptr);
            let _ = GlobalUnlock(global);
            Some(value? == 0)
        })()
        .unwrap_or(true);
        let _ = CloseClipboard();
        if deny {
            return true;
        }
    }
    false
}

unsafe fn clipboard_capture(h: HWND) {
    let cfg = UI_CFG
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(Config::defaults);
    if !cfg.clipboard_history {
        return;
    }
    if clipboard_is_sensitive(h) {
        log_debug!("clipboard capture skipped: owner marked the content sensitive");
        return;
    }
    let Some(text) = clipboard_get_text(h) else {
        return;
    };
    let mut items = CLIPBOARD_ITEMS.lock().unwrap();
    items.retain(|item| item != &text);
    items.push_front(text);
    items.truncate(cfg.clipboard_limit);
}

/// Type text into whatever the user was working in. Restores foreground to the
/// window that had it when the picker opened and WAITS for it, because Ctrl+V
/// goes wherever the foreground is at the moment it is injected.
unsafe fn paste_text(h: HWND, text: &str) {
    clipboard_set_text(h, text);
    let target = LAUNCHER_PREV_FG.swap(0, Ordering::Relaxed);
    if target != 0 && IsWindow(hwnd_from(target)).as_bool() {
        focus_window(target);
        // Up to ~100 ms; foreground changes are asynchronous and the window
        // may have to repaint first. Bounded so a stuck app cannot hang the
        // launcher thread.
        for _ in 0..20 {
            if GetForegroundWindow().0 as isize == target {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if GetForegroundWindow().0 as isize != target {
            log_error!("paste target {target:#x} never regained focus; pasting anyway");
        }
    }
    inject_key(VK_CONTROL, false);
    inject_key(VIRTUAL_KEY(0x56), false);
    inject_key(VIRTUAL_KEY(0x56), true);
    inject_key(VK_CONTROL, true);
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
        while self
            .b
            .get(self.i)
            .is_some_and(|c| c.is_ascii_digit() || *c == b'.')
        {
            self.i += 1;
        }
        if self.i == start {
            return None;
        }
        std::str::from_utf8(&self.b[start..self.i])
            .ok()?
            .parse()
            .ok()
    }
}

/// Evaluate a maths query. Only fires when the text looks like an expression
/// (calc characters only, at least one operator, at least one digit) so app
/// names never trigger it.
fn calc_eval(q: &str) -> Option<f64> {
    let t = q.trim();
    if t.is_empty()
        || !t
            .bytes()
            .all(|c| c.is_ascii_digit() || b"+-*/%^(). ".contains(&c))
        || !t.bytes().any(|c| c.is_ascii_digit())
        || !t.bytes().any(|c| b"+-*/%^".contains(&c))
    {
        return None;
    }
    let mut p = CalcParser {
        b: t.as_bytes(),
        i: 0,
    };
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

fn url_encode(q: &str) -> String {
    let mut out = String::new();
    for b in q.trim().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Open configured web-search template. `{query}` receives URL-encoded text.
unsafe fn launcher_web_search(q: &str) {
    let template = UI_CFG
        .lock()
        .unwrap()
        .as_ref()
        .map(|c| c.launcher_web_url.clone())
        .unwrap_or_else(|| "https://www.google.com/search?q={query}".to_string());
    launcher_launch(&template.replace("{query}", &url_encode(q)));
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

// Lazy icon loader: paint enqueues visible app/file rows; workers resolve shell
// icons off the UI thread into ICON_CACHE / BAR_ICON_CACHE / FILE_ICON_CACHE.
// Every job carries
// the physical px it is for (a worker must not read UI_DPI, which follows
// whichever monitor a popup last opened on). File jobs carry the search
// generation so rows of a superseded result are not resolved.
#[derive(Clone, PartialEq, Eq)]
enum IconJob {
    /// (index into LauncherState::all, px)
    App(usize, i32),
    /// (search generation, index into LauncherState::files, px)
    File(u64, usize, i32),
    /// (exe path, physical pixel size to resolve at), into BAR_ICON_CACHE
    Bar(String, i32),
    /// (exe path, px): a switcher Window row, into ICON_CACHE
    Exe(String, i32),
}
static ICON_QUEUE: Mutex<VecDeque<IconJob>> = Mutex::new(VecDeque::new());
static ICON_CV: Condvar = Condvar::new();

/// One resolved size of one icon source.
#[derive(Clone, Copy)]
struct IconSlot<H> {
    px: i32,
    stamp: u64,
    icon: H,
    used: u64, // cache clock at the last lookup (LRU order)
}

/// Shell icons keyed by (source, physical px, stamp): the source (a path or a
/// shell parsing name), the size it was resolved at, and a stamp (the source
/// file's mtime, 0 when there is none to read), so a changed source is a miss
/// rather than a stale hit. The cache owns every handle it holds and hands a
/// handle to `delete` exactly once, when it drops it; rows and bar buttons
/// only look up. `cap` None = no LRU: entries go only through `retain_live`.
///
/// Why it exists (LAUNCH-13): icons used to be owned per list row, so every
/// config reload destroyed and re-extracted hundreds of them, the startup
/// preload ran at 96 DPI so 125%/150% monitors drew them upscaled from 32 px,
/// and a reload on the main thread destroyed bar icons the launcher thread
/// could be drawing at that moment.
struct IconCache<H> {
    map: HashMap<String, Vec<IconSlot<H>>>,
    len: usize,
    clock: u64,
    cap: Option<usize>,
}

impl<H: Copy + PartialEq> IconCache<H> {
    fn new(cap: Option<usize>) -> Self {
        IconCache {
            map: HashMap::new(),
            len: 0,
            clock: 0,
            cap,
        }
    }

    /// The icon for exactly this source, size and stamp.
    fn get(&mut self, source: &str, px: i32, stamp: u64) -> Option<H> {
        self.clock += 1;
        let clock = self.clock;
        let slot = self
            .map
            .get_mut(source)?
            .iter_mut()
            .find(|s| s.px == px && s.stamp == stamp)?;
        slot.used = clock;
        Some(slot.icon)
    }

    /// The closest other size of `source` whose icon passes `usable`: what a
    /// paint draws (scaled, briefly) until the exact size resolves.
    fn nearest(&self, source: &str, px: i32, stamp: u64, usable: impl Fn(H) -> bool) -> Option<H> {
        self.map
            .get(source)?
            .iter()
            .filter(|s| s.stamp == stamp && usable(s.icon))
            .min_by_key(|s| (s.px - px).abs())
            .map(|s| s.icon)
    }

    /// Store `icon`, replacing a `pending` placeholder. If the key already
    /// holds anything else (two workers resolved the same key), the existing
    /// entry wins and `icon` comes back for the caller to free.
    fn insert(&mut self, source: &str, px: i32, stamp: u64, icon: H, pending: H) -> Option<H> {
        self.clock += 1;
        let clock = self.clock;
        let slots = self.map.entry(source.to_string()).or_default();
        match slots.iter_mut().find(|s| s.px == px && s.stamp == stamp) {
            Some(s) if s.icon == pending => {
                s.icon = icon;
                s.used = clock;
                None
            }
            Some(_) => Some(icon),
            None => {
                slots.push(IconSlot {
                    px,
                    stamp,
                    icon,
                    used: clock,
                });
                self.len += 1;
                None
            }
        }
    }

    fn over_cap(&self) -> bool {
        self.cap.is_some_and(|cap| self.len > cap)
    }

    /// Drop every entry `live(source, px, stamp)` rejects, whatever the cap,
    /// handing each dropped handle to `dead` once: a size no monitor uses any
    /// more or a superseded stamp, which no lookup can ask for again. The
    /// caller decides when a dropped handle can be destroyed.
    fn retain_live(&mut self, live: impl Fn(&str, i32, u64) -> bool, mut dead: impl FnMut(H)) {
        let mut dropped = 0;
        self.map.retain(|src, slots| {
            slots.retain(|s| {
                let keep = live(src, s.px, s.stamp);
                if !keep {
                    dead(s.icon);
                    dropped += 1;
                }
                keep
            });
            !slots.is_empty()
        });
        self.len -= dropped;
    }

    /// Over `cap`: drop least-recently-used entries until back under it,
    /// skipping every source `keep` claims (the rows on screen draw those),
    /// and hand each dropped handle to `delete` once. An uncapped cache never
    /// drops anything here.
    fn evict(&mut self, keep: impl Fn(&str) -> bool, mut delete: impl FnMut(H)) {
        let Some(cap) = self.cap else {
            return;
        };
        if self.len <= cap {
            return;
        }
        let mut order: Vec<(u64, String, i32, u64)> = self
            .map
            .iter()
            .filter(|(src, _)| !keep(src))
            .flat_map(|(src, slots)| slots.iter().map(|s| (s.used, src.clone(), s.px, s.stamp)))
            .collect();
        order.sort_unstable_by_key(|o| o.0);
        for (_, src, px, stamp) in order {
            if self.len <= cap {
                break;
            }
            let Some(slots) = self.map.get_mut(&src) else {
                continue;
            };
            if let Some(i) = slots.iter().position(|s| s.px == px && s.stamp == stamp) {
                delete(slots.swap_remove(i).icon);
                self.len -= 1;
            }
            if slots.is_empty() {
                self.map.remove(&src);
            }
        }
    }
}

/// Launcher app rows and switcher Window rows. Values: an HICON, or -1 =
/// resolving failed (not retried). Only the launcher thread draws these, so it
/// is also the one that frees them: LA_REFRESH drops sizes no monitor uses and
/// superseded stamps, between paints, with no handle in flight anywhere else.
/// Never evicted otherwise, so a reload or F5 re-extracts nothing it had.
/// (Kept apart from bar and system-menu icons for exactly that: shared with
/// the bar snapshot and the sysmenu thread, nothing could be freed safely, and
/// every launcher_icon_size tried kept a full set per DPI until exit.)
static ICON_CACHE: LazyLock<Mutex<IconCache<isize>>> =
    LazyLock::new(|| Mutex::new(IconCache::new(None)));
/// Bar app-button icons, keyed (exe path, px, 0). Values as ICON_CACHE, plus
/// 0 = a bar job is queued for it. The main thread draws them, from the
/// manager's BAR snapshot, so a handle dropped from here may still be in the
/// published snapshot or in the clone a paint is drawing: `bar_icons_retire`
/// parks it and `bar_icons_sweep` frees it once a snapshot built after that is
/// published, on the main thread between paints.
static BAR_ICON_CACHE: LazyLock<Mutex<IconCache<isize>>> =
    LazyLock::new(|| Mutex::new(IconCache::new(None)));
/// System-menu custom icons. Resolved and drawn only on the sysmenu thread,
/// which frees an entry the moment it supersedes it.
static SYSMENU_ICON_CACHE: LazyLock<Mutex<IconCache<isize>>> =
    LazyLock::new(|| Mutex::new(IconCache::new(None)));
/// File-result icons are unbounded (every search brings new paths), so LRU:
/// each HICON is a USER object plus two GDI bitmaps, and running into the 10k
/// per-process USER/GDI quota would break all GDI, snapshot overlays included.
/// Evicted only on the launcher thread (end of paint), the only thread that
/// draws file icons, and never for a row currently listed.
static FILE_ICON_CACHE: LazyLock<Mutex<IconCache<isize>>> =
    LazyLock::new(|| Mutex::new(IconCache::new(Some(FILE_ICON_CAP))));
const FILE_ICON_CAP: usize = 512;

/// Stamp for an icon source: its mtime, so an app update or a rewritten
/// shortcut resolves anew; 0 for shell parsing names (UWP), which keep their
/// first icon until Astur restarts.
fn icon_stamp(source: &str) -> u64 {
    if source.starts_with("shell:") {
        return 0;
    }
    std::fs::metadata(source)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as u64)
}

struct AppEntry {
    name: String,
    name_lc: String,
    path: String,      // launch target: shortcut, app shell id, URL, file, or command
    icon_path: String, // icon source; defaults to path, custom entries may override it
    stamp: u64,        // icon_stamp(icon_path) at enumeration: part of the icon key
}
/// One file/folder result from the Windows Search index (Phase 3). Its icon
/// lives in FILE_ICON_CACHE, keyed by path, px and `date`.
#[derive(Clone)]
struct FileHit {
    name: String,
    path: String,
    size: i64, // bytes (-1 = unknown / folder)
    date: f64, // OLE automation date (days since 1899-12-30); 0 = unknown
}

impl FileHit {
    /// Icon-key stamp: the indexed modified date, free with the result.
    fn stamp(&self) -> u64 {
        self.date.to_bits()
    }
}
struct WindowHit {
    hwnd: isize,
    title: String,
    title_lc: String,
    exe: String,
}
struct EmojiHit {
    text: String,
    name: String,
    name_lc: String,
}
/// A visible result row from any enabled launcher provider.
#[derive(Clone, Copy)]
enum Hit {
    App(usize),
    File(usize),
    Window(usize),
    Clipboard(usize),
    Emoji(usize),
    Calc,
    Web,
}
struct LauncherState {
    query: String,
    all: Vec<AppEntry>,
    files: Vec<FileHit>, // current file-search results (top-N, replaced per query)
    windows: Vec<WindowHit>,
    clipboard: Vec<String>,
    emoji: Vec<EmojiHit>,
    filtered: Vec<Hit>,   // merged app + file rows, best first
    calc: Option<String>, // formatted calculator result for the current query
    sel: usize,
    scroll: usize, // first visible row (wheel scrolls; keyboard keeps sel visible)
    loaded: bool,
    wide: bool,        // Tab: wide column view (modified / size / path)
    window_only: bool, // Alt+Tab replacement mode
    search_gen: u64,   // generation of `files` (drops stale async results)
    // Recent file-search results, newest first, keyed by `file_search_key`.
    // Backspace to a query seen moments ago shows its rows at once instead of
    // an empty file section for the ~145 ms round trip.
    file_cache: VecDeque<(String, Vec<FileHit>)>,
}
static LAUNCHER_STATE: Mutex<LauncherState> = Mutex::new(LauncherState {
    query: String::new(),
    all: Vec::new(),
    files: Vec::new(),
    windows: Vec::new(),
    clipboard: Vec::new(),
    emoji: Vec::new(),
    filtered: Vec::new(),
    calc: None,
    sel: 0,
    scroll: 0,
    loaded: false,
    wide: false,
    window_only: false,
    search_gen: 0,
    file_cache: VecDeque::new(),
});

// File-search request hand-off to `filesearch_worker` (debounced + cancellable).
static SEARCH_REQ: Mutex<Option<(u64, String)>> = Mutex::new(None);
static SEARCH_CV: Condvar = Condvar::new();
static SEARCH_GEN: AtomicU64 = AtomicU64::new(0);
/// Query -> result pairs kept in `LauncherState::file_cache`.
const FILE_CACHE_CAP: usize = 32;

/// Destroy an HICON the icon caches own (or a duplicate nobody stored).
unsafe fn release_launcher_icon(raw: isize) {
    if raw > 1 {
        let _ = DestroyIcon(HICON(raw as *mut c_void));
    }
}

/// Move `key` to the front of the result cache with `hits`, dropping the
/// least-recent entry past `cap`.
fn file_cache_put<T>(cache: &mut VecDeque<(String, T)>, key: String, hits: T, cap: usize) {
    cache.retain(|(k, _)| *k != key);
    cache.push_front((key, hits));
    cache.truncate(cap);
}

/// Look `key` up in the result cache, marking it most recent.
fn file_cache_get<'a, T>(cache: &'a mut VecDeque<(String, T)>, key: &str) -> Option<&'a T> {
    let pos = cache.iter().position(|(k, _)| k == key)?;
    let entry = cache.remove(pos)?;
    cache.push_front(entry);
    cache.front().map(|(_, v)| v)
}

/// Stand-in file rows for a query just edited, shown until the index answers:
/// the cached result for exactly this query if there is one, else the current
/// rows the new query still matches. Before this the file section emptied on
/// every keystroke and came back ~145 ms later (45 ms debounce + the query).
/// Rows can reorder when the real TOP-N lands; Enter on a stand-in row opens
/// a real file that matches.
/// Their icons come from FILE_ICON_CACHE by path, so a kept row keeps its icon.
fn launcher_provisional_files(st: &mut LauncherState, cfg: &Config) {
    let Some(key) = file_search_key(&st.query, cfg) else {
        st.files.clear(); // no search runs for this query
        return;
    };
    st.files = match file_cache_get(&mut st.file_cache, &key) {
        Some(hits) => hits.clone(),
        None => provisional_hits(st.files.iter().map(|f| f.name.as_str()), &st.query)
            .into_iter()
            .map(|i| st.files[i].clone())
            .collect(),
    };
}

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
                        icon_path: p.to_string_lossy().into_owned(),
                        stamp: 0,
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
                    icon_path: path.clone(),
                    path,
                    stamp: 0,
                },
            );
        }
    }
}

/// Enumerate installed apps: Start Menu `.lnk`/`.url` first (reliable launch), then
/// the AppsFolder for everything else (UWP/system apps). Sorted by display name.
fn launcher_enumerate() -> Vec<AppEntry> {
    let cfg = UI_CFG
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(Config::defaults);
    let mut map = std::collections::HashMap::new();
    if cfg.launcher_source_apps {
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
    }
    for entry in cfg.launcher_entries {
        let key = entry.label.to_ascii_lowercase();
        let icon_path = if entry.icon.is_empty() || entry.icon.eq_ignore_ascii_case("auto") {
            entry.target.clone()
        } else {
            entry.icon
        };
        map.insert(
            key.clone(),
            AppEntry {
                name: entry.label,
                name_lc: key,
                path: entry.target,
                icon_path,
                stamp: 0,
            },
        );
    }
    let mut v: Vec<AppEntry> = map.into_values().collect();
    v.sort_by(|a, b| a.name_lc.cmp(&b.name_lc));
    // Once per enumeration (startup, F5, a launcher-list reload), never per
    // paint: one metadata read per entry.
    for e in &mut v {
        e.stamp = icon_stamp(&e.icon_path);
    }
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
    // 3) Generic executable icon so a row never renders blank. Copy the cached
    // base handle because the icon caches own and may destroy what they hold.
    if let Some(hicon) = generic_app_icon() {
        if let Ok(copy) = CopyIcon(hicon) {
            return copy.0 as isize;
        }
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
    let hb = factory
        .GetImage(SIZE { cx: px, cy: px }, SIIGBF_ICONONLY)
        .ok()?;
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
            let job = {
                let mut q = ICON_QUEUE.lock().unwrap();
                loop {
                    if let Some(job) = q.pop_front() {
                        break job;
                    }
                    q = ICON_CV.wait(q).unwrap();
                }
            };
            // Short lock: copy the source only if the row still belongs to
            // the current model, and skip what the cache already holds (a
            // reload or F5 re-queues every app; nearly all are hits).
            let (path, px, stamp) = {
                let st = LAUNCHER_STATE.lock().unwrap();
                match &job {
                    IconJob::App(i, px) => match st.all.get(*i) {
                        Some(e) if !is_builtin_icon(&e.icon_path) => {
                            (e.icon_path.clone(), *px, e.stamp)
                        }
                        _ => continue,
                    },
                    IconJob::File(gen, i, px) if st.search_gen == *gen => match st.files.get(*i) {
                        Some(f) => (f.path.clone(), *px, f.stamp()),
                        _ => continue,
                    },
                    IconJob::File(..) => continue,
                    IconJob::Bar(path, px) | IconJob::Exe(path, px) => (path.clone(), *px, 0),
                }
            };
            let cache = match &job {
                IconJob::File(..) => &FILE_ICON_CACHE,
                IconJob::Bar(..) => &BAR_ICON_CACHE,
                IconJob::App(..) | IconJob::Exe(..) => &ICON_CACHE,
            };
            // A Bar job's key holds the 0 placeholder bar_app_icon left there.
            if !matches!(job, IconJob::Bar(..))
                && cache.lock().unwrap().get(&path, px, stamp).is_some()
            {
                continue;
            }
            // Every job carries the exact size it was queued for; the worker
            // must not re-read a global, which on a mixed-DPI desk would be
            // whichever monitor painted last.
            let hicon = load_icon(&path, px);
            // Keyed by source, so an icon can never land on the wrong row
            // however the list moved while it resolved.
            let dup = cache.lock().unwrap().insert(&path, px, stamp, hicon, 0);
            if let Some(dup) = dup {
                release_launcher_icon(dup); // another worker got there first
            }
            if matches!(job, IconJob::Bar(..)) {
                // Paint draws the manager's snapshot, which still holds the 0
                // placeholder, so a repaint alone showed nothing until some
                // unrelated command ran (BAR-13). Have update_bar publish the
                // new handle: through the BarRefresh gate, so a startup or
                // reload burst of N icons queues one refresh, not N ahead of
                // the user's commands.
                if bar_refresh_gate(&BAR_REFRESH_QUEUED) {
                    push_cmd(Cmd::BarRefresh);
                }
            }
            // Launcher rows (app, file and switcher Window rows) repaint only
            // on this.
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
unsafe fn launcher_windows() -> Vec<WindowHit> {
    let mut out = Vec::new();
    // A copy: the loop does OpenProcess and title reads per window, and the
    // main (hook) thread takes MANAGED on every show and foreground event.
    let managed = MANAGED.lock().unwrap().clone();
    for h in managed {
        let hwnd = hwnd_from(h);
        if (h == SCRATCHPAD_HWND.load(Ordering::Relaxed)
            && SCRATCHPAD_HIDDEN.load(Ordering::Relaxed))
            || !IsWindow(hwnd).as_bool()
        {
            continue;
        }
        let title = window_title(hwnd);
        if title.is_empty() {
            continue;
        }
        let exe = window_exe(hwnd).unwrap_or_default();
        out.push(WindowHit {
            hwnd: h,
            title_lc: format!(
                "{} {}",
                title.to_ascii_lowercase(),
                exe.to_ascii_lowercase()
            ),
            title,
            exe,
        });
    }
    let order = WINDOW_MRU.lock().unwrap().clone();
    out.sort_by(|a, b| {
        let ar = order
            .iter()
            .position(|item| *item == a.hwnd)
            .unwrap_or(usize::MAX);
        let br = order
            .iter()
            .position(|item| *item == b.hwnd)
            .unwrap_or(usize::MAX);
        ar.cmp(&br).then_with(|| {
            a.title
                .to_ascii_lowercase()
                .cmp(&b.title.to_ascii_lowercase())
        })
    });
    let foreground = GetForegroundWindow().0 as isize;
    if let Some(index) = out.iter().position(|win| win.hwnd == foreground) {
        out.rotate_left(index);
    }
    out
}

fn emoji_catalog() -> Vec<EmojiHit> {
    const ITEMS: &[(u32, &str)] = &[
        (0x1F600, "grinning face"),
        (0x1F602, "face tears joy laugh"),
        (0x1F603, "smiling face"),
        (0x1F609, "wink face"),
        (0x1F60D, "heart eyes face"),
        (0x1F914, "thinking face"),
        (0x1F642, "slight smile"),
        (0x1F643, "upside down face"),
        (0x1F44D, "thumbs up approve"),
        (0x1F44E, "thumbs down reject"),
        (0x1F44F, "clap hands"),
        (0x1F64F, "folded hands thanks"),
        (0x1F4AA, "strong flex"),
        (0x1F91D, "handshake"),
        (0x1F44B, "wave hello goodbye"),
        (0x2764, "heart love"),
        (0x1F494, "broken heart"),
        (0x1F525, "fire hot"),
        (0x2728, "sparkles"),
        (0x2B50, "star favourite"),
        (0x2705, "check mark done"),
        (0x274C, "cross mark no"),
        (0x26A0, "warning"),
        (0x2139, "information"),
        (0x1F4A1, "light bulb idea"),
        (0x1F680, "rocket launch"),
        (0x1F389, "party celebration"),
        (0x1F381, "gift present"),
        (0x1F4CC, "pin"),
        (0x1F4C5, "calendar"),
        (0x1F4E7, "email"),
        (0x1F4DE, "phone"),
        (0x1F4BB, "computer laptop"),
        (0x1F527, "wrench tool"),
        (0x2699, "gear settings"),
        (0x1F512, "lock secure"),
        (0x1F513, "unlock"),
        (0x1F50D, "search magnifier"),
        (0x1F4C1, "folder"),
        (0x1F4C4, "document file"),
    ];
    ITEMS
        .iter()
        .filter_map(|(code, name)| {
            char::from_u32(*code).map(|glyph| EmojiHit {
                text: glyph.to_string(),
                name: (*name).to_string(),
                name_lc: (*name).to_string(),
            })
        })
        .collect()
}

fn prefixed_query<'a>(query: &'a str, prefix: &str) -> Option<&'a str> {
    (!prefix.is_empty())
        .then(|| query.strip_prefix(prefix))
        .flatten()
        .map(str::trim)
}

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
    let cfg = UI_CFG
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(Config::defaults);
    let query = st.query.trim();
    if st.window_only {
        st.calc = None;
        let mut windows: Vec<(i32, usize)> = st
            .windows
            .iter()
            .enumerate()
            .filter_map(|(i, win)| fuzzy_score(query, &win.title_lc).map(|score| (score, i)))
            .collect();
        windows.sort_by_key(|r| std::cmp::Reverse(r.0));
        st.filtered = windows
            .into_iter()
            .map(|(_, i)| Hit::Window(i))
            .take(cfg.launcher_max_results)
            .collect();
        if st.sel >= st.filtered.len() {
            st.sel = st.filtered.len().saturating_sub(1);
        }
        return;
    }
    let clip_q = (cfg.launcher_source_clipboard && cfg.clipboard_history)
        .then(|| prefixed_query(query, &cfg.clipboard_prefix))
        .flatten();
    let emoji_q = (cfg.launcher_source_emoji && cfg.emoji_picker)
        .then(|| prefixed_query(query, &cfg.emoji_prefix))
        .flatten();
    let mut filtered: Vec<Hit> = Vec::new();

    if let Some(q) = clip_q {
        let q = q.to_ascii_lowercase();
        let mut scored: Vec<(i32, usize)> = st
            .clipboard
            .iter()
            .enumerate()
            .filter_map(|(i, text)| {
                fuzzy_score(&q, &text.replace(['\r', '\n'], " ").to_ascii_lowercase())
                    .map(|score| (score, i))
            })
            .collect();
        scored.sort_by_key(|r| std::cmp::Reverse(r.0));
        filtered.extend(scored.into_iter().map(|(_, i)| Hit::Clipboard(i)));
        st.calc = None;
    } else if let Some(q) = emoji_q {
        let q = q.to_ascii_lowercase();
        let mut scored: Vec<(i32, usize)> = st
            .emoji
            .iter()
            .enumerate()
            .filter_map(|(i, item)| fuzzy_score(&q, &item.name_lc).map(|score| (score, i)))
            .collect();
        scored.sort_by_key(|r| std::cmp::Reverse(r.0));
        filtered.extend(scored.into_iter().map(|(_, i)| Hit::Emoji(i)));
        st.calc = None;
    } else {
        let q = query.to_ascii_lowercase();
        st.calc = cfg
            .launcher_source_calc
            .then(|| calc_eval(query))
            .flatten()
            .map(calc_fmt);
        if st.calc.is_some() {
            filtered.push(Hit::Calc);
        }
        let mut scored: Vec<(i32, usize)> = st
            .all
            .iter()
            .enumerate()
            .filter_map(|(i, entry)| {
                fuzzy_score(&q, &entry.name_lc).map(|score| {
                    let boost = if cfg.launcher_mru {
                        launcher_mru_score(&entry.path)
                    } else {
                        0
                    };
                    (score + boost, i)
                })
            })
            .collect();
        scored.sort_by_key(|r| std::cmp::Reverse(r.0));
        filtered.extend(scored.into_iter().map(|(_, i)| Hit::App(i)));

        if cfg.launcher_source_windows {
            let mut windows: Vec<(i32, usize)> = st
                .windows
                .iter()
                .enumerate()
                .filter_map(|(i, win)| {
                    fuzzy_score(&q, &win.title_lc).map(|score| {
                        let boost = if cfg.launcher_mru {
                            launcher_mru_score(&win.exe)
                        } else {
                            0
                        };
                        (score + boost, i)
                    })
                })
                .collect();
            windows.sort_by_key(|r| std::cmp::Reverse(r.0));
            filtered.extend(windows.into_iter().map(|(_, i)| Hit::Window(i)));
        }
        if cfg.launcher_source_files {
            filtered.extend((0..st.files.len()).map(Hit::File));
        }
        if cfg.launcher_source_web && filtered.is_empty() && !query.is_empty() {
            filtered.push(Hit::Web);
        }
    }
    filtered.truncate(cfg.launcher_max_results);
    st.filtered = filtered;
    if st.sel >= st.filtered.len() {
        st.sel = st.filtered.len().saturating_sub(1);
    }
}
/// Apply one typed edit to the query: reset the selection, put up provisional
/// file rows, refilter, start the file search and repaint. Launcher thread.
unsafe fn launcher_edit_query(h: HWND, edit: impl FnOnce(&mut String)) {
    let cfg = UI_CFG
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(Config::defaults);
    let q = {
        let mut st = LAUNCHER_STATE.lock().unwrap();
        edit(&mut st.query);
        st.sel = 0;
        st.scroll = 0;
        launcher_provisional_files(&mut st, &cfg);
        launcher_refilter(&mut st);
        st.query.clone()
    };
    launcher_dispatch_search(&q);
    let _ = InvalidateRect(h, None, BOOL(0));
}

/// Bump the search generation and hand the current query to `filesearch_worker`.
/// Cheap; the worker debounces + drops stale generations.
fn launcher_dispatch_search(query: &str) {
    let cfg = UI_CFG
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(Config::defaults);
    if !file_search_wanted(query, &cfg) {
        launcher_cancel_search();
        return;
    }
    let gen = SEARCH_GEN.fetch_add(1, Ordering::Relaxed) + 1;
    *SEARCH_REQ.lock().unwrap() = Some((gen, query.to_string()));
    SEARCH_CV.notify_one();
}

/// Supersede any queued or in-flight file search, so its result is dropped.
fn launcher_cancel_search() {
    SEARCH_GEN.fetch_add(1, Ordering::Relaxed);
    *SEARCH_REQ.lock().unwrap() = None;
}

/// Whether `query` runs a file search at all: files are on and the query is
/// not a clipboard/emoji provider query.
fn file_search_wanted(query: &str, cfg: &Config) -> bool {
    let provider_only = (cfg.launcher_source_clipboard
        && cfg.clipboard_history
        && prefixed_query(query.trim(), &cfg.clipboard_prefix).is_some())
        || (cfg.launcher_source_emoji
            && cfg.emoji_picker
            && prefixed_query(query.trim(), &cfg.emoji_prefix).is_some());
    cfg.launcher_source_files && !provider_only
}

/// Result-cache key for `query`: its CONTAINS terms plus every setting that
/// shapes the result (scope, excludes, row cap), so a cached result can never
/// outlive a config change that would alter it. None when no search runs.
fn file_search_key(query: &str, cfg: &Config) -> Option<String> {
    if !file_search_wanted(query, cfg) {
        return None;
    }
    let contains = build_contains(query)?;
    Some(format!(
        "{contains}\u{1}{}\u{1}{:?}\u{1}{}",
        cfg.launcher_file_scope.trim(),
        cfg.launcher_file_exclude,
        cfg.launcher_max_results
    ))
}

/// Whether a finished search for `gen` may replace the file rows: only the
/// newest generation, and only forward. With two workers the older query can
/// finish last; checked and stored under one LAUNCHER_STATE lock, so a stale
/// result never overwrites a newer one (no later result would repair it).
fn should_store_search(gen: u64, cur_gen: u64, st_gen: u64) -> bool {
    gen == cur_gen && gen > st_gen
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
    let words: Vec<String> = contains_terms(query)
        .into_iter()
        .map(|w| format!("\"{w}*\""))
        .collect();
    if words.is_empty() {
        None
    } else {
        Some(words.join(" AND "))
    }
}

/// The query words `build_contains` sends as prefix terms. Shared with
/// `provisional_hits` so the local preview and the index query cannot drift.
fn contains_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| *c != '"' && *c != '\'')
                .collect::<String>()
        })
        .filter(|w| w.chars().count() >= 2)
        .collect()
}

/// Indices of the file `names` that `query` still matches, mirroring
/// `CONTAINS(System.FileName, '"w1*" AND "w2*"')`: every term must prefix a
/// word of the name, case-insensitively. Empty when the query has no term,
/// exactly as `FileSearch::run` returns nothing then. A preview: the indexer's
/// word breaker is not a plain split on non-alphanumerics, so the real result
/// may differ by a row when it lands.
fn provisional_hits<'a>(names: impl Iterator<Item = &'a str>, query: &str) -> Vec<usize> {
    let terms: Vec<String> = contains_terms(query)
        .iter()
        .map(|t| t.to_lowercase())
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }
    names
        .enumerate()
        .filter(|(_, name)| {
            let name = name.to_lowercase();
            let words: Vec<&str> = name
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .collect();
            terms
                .iter()
                .all(|t| words.iter().any(|w| w.starts_with(t.as_str())))
        })
        .map(|(i, _)| i)
        .collect()
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
    format!(
        "{y:04}-{m:02}-{day:02} {:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60
    )
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
            CLSCTX_INPROC_SERVER.0,
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

    unsafe fn run(&self, query: &str, cfg: &Config) -> Vec<FileHit> {
        let mut out = Vec::new();
        let Some(contains) = build_contains(query) else {
            return out; // no ≥2-char term — would match almost everything
        };
        let configured = cfg.launcher_file_scope.trim();
        let scope = if configured.is_empty() {
            std::env::var("USERPROFILE").unwrap_or_default()
        } else {
            configured.to_string()
        };
        let scope_clause = if scope.is_empty() || scope == "*" || scope.eq_ignore_ascii_case("all")
        {
            String::new()
        } else {
            format!(" AND SCOPE='file:{}'", scope.replace('\'', "''"))
        };
        let top = cfg.launcher_max_results.clamp(5, 500);
        let sql = format!(
            "SELECT TOP {top} System.ItemPathDisplay, System.Size, System.DateModified \
             FROM SYSTEMINDEX WHERE CONTAINS(System.FileName, '{contains}'){scope_clause} \
             ORDER BY System.DateModified DESC"
        );
        let _ = self.exec(&sql, &mut out);
        if !cfg.launcher_file_exclude.is_empty() {
            let excludes: Vec<String> = cfg
                .launcher_file_exclude
                .iter()
                .map(|s| s.to_ascii_lowercase())
                .collect();
            out.retain(|hit| {
                let path = hit.path.to_ascii_lowercase();
                !excludes
                    .iter()
                    .any(|part| !part.is_empty() && path.contains(part))
            });
        }
        out.truncate(cfg.launcher_max_results);
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
                s_path: 0,
                _p0: 0,
                path: std::ptr::null_mut(),
                s_size: 0,
                _p1: 0,
                size: 0,
                s_date: 0,
                _p2: 0,
                date: 0.0,
            };
            if rowset
                .GetData(hrow, hacc, &mut row as *mut SearchRow as *mut c_void)
                .is_ok()
            {
                let ok = DBSTATUS_S_OK.0 as u32;
                let path = if row.s_path == ok {
                    read_wide(row.path)
                } else {
                    String::new()
                };
                if is_fs_path(&path) {
                    let size = if row.s_size == ok { row.size } else { -1 };
                    let date = if row.s_date == ok { row.date } else { 0.0 };
                    let name = std::path::Path::new(&path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(&path)
                        .to_string();
                    out.push(FileHit {
                        name,
                        path,
                        size,
                        date,
                    });
                }
            }
            let _ = rowset.ReleaseRows(
                obtained,
                hrow_arr as *const usize,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            CoTaskMemFree(Some(hrow_arr as *const c_void));
        }
        let _ = accessor.ReleaseAccessor(hacc, None);
        Ok(())
    }
}

/// File-search worker: own COM STA + one persistent index connection. Drains the
/// debounced request slot, drops stale generations, writes results + repaints.
/// Two run side by side on the one request slot, so the newest query starts at
/// once instead of queueing behind a superseded one still in the index (a
/// query cannot be abandoned mid-Execute). If the index can't be opened, file
/// search is silently disabled (apps still work).
fn filesearch_worker() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        // A worker with no session must not take requests: with two workers, a
        // generation it swallowed would never be answered and the provisional
        // rows for it would stay up.
        let Some(search) = FileSearch::new() else {
            log_debug!("file search: Windows Search index unavailable on this worker");
            return;
        };
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
            let cfg = UI_CFG
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(Config::defaults);
            let hits = search.run(&q, &cfg);
            {
                let mut st = LAUNCHER_STATE.lock().unwrap();
                // Cache even a superseded result: it is still the true answer
                // for `q`, which is what Backspace comes back to.
                if let Some(key) = file_search_key(&q, &cfg) {
                    file_cache_put(&mut st.file_cache, key, hits.clone(), FILE_CACHE_CAP);
                }
                let cur = SEARCH_GEN.load(Ordering::Relaxed);
                if !should_store_search(gen, cur, st.search_gen) {
                    continue; // superseded while the index query ran
                }
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

/// Build/rebuild shared popup font after a live configuration change.
unsafe fn make_launcher_font() {
    let current = LAUNCHER_FONT.load(Ordering::Acquire);
    if current != 0 && !POPUP_FONT_DIRTY.swap(false, Ordering::AcqRel) {
        return;
    }
    let (name, size, weight) = UI_CFG
        .lock()
        .unwrap()
        .as_ref()
        .map(|c| {
            (
                c.popup_font_name.clone(),
                c.popup_font_size,
                c.popup_font_weight,
            )
        })
        .unwrap_or_else(|| ("Segoe UI".to_string(), 19, 600));
    let mut wname: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // Logical point-ish size from the config, scaled to the popup's monitor.
    let size = dpi_px(size, ui_dpi()).max(8);
    let f = CreateFontW(
        -size,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        0,
        PCWSTR(wname.as_mut_ptr()),
    );
    if !f.0.is_null() {
        let old = LAUNCHER_FONT.swap(f.0 as isize, Ordering::AcqRel);
        if old != 0 {
            let _ = DeleteObject(HGDIOBJ(old as *mut c_void));
        }
    }
}

/// Center the launcher on the monitor under the cursor and show it (no-activate
/// — we drive it via the keyboard hook, so it must not steal focus).
/// Size + center the picker on `wa`, publish its bounds for the mouse hook
/// (click-outside dismiss + wheel routing), and repaint. `wide` = the Tab column
/// view; the width is clamped to the work area on small screens.
unsafe fn launcher_place(h: HWND, wa: RECT, wide: bool) {
    // Adopt the target monitor's scale BEFORE reading any la_* metric — they
    // all resolve against it.
    set_ui_dpi(dpi_at(POINT {
        x: (wa.left + wa.right) / 2,
        y: (wa.top + wa.bottom) / 2,
    }));
    let want = if wide { la_wide_w() } else { la_w() };
    let win_w = want.min(wa.right - wa.left - 48).max(320);
    let x = (wa.left + wa.right) / 2 - win_w / 2;
    let y = (wa.top + wa.bottom) / 2 - la_h() / 2;
    let _ = SetWindowPos(h, HWND_TOPMOST, x, y, win_w, la_h(), SWP_NOACTIVATE);
    shape_popup(h, win_w, la_h());
    LAUNCHER_RECT_L.store(x, Ordering::Relaxed);
    LAUNCHER_RECT_T.store(y, Ordering::Relaxed);
    LAUNCHER_RECT_R.store(x + win_w, Ordering::Relaxed);
    LAUNCHER_RECT_B.store(y + la_h(), Ordering::Relaxed);
    let _ = InvalidateRect(h, None, BOOL(0));
}

unsafe fn launcher_target_work_area() -> RECT {
    let cfg = UI_CFG
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(Config::defaults);
    let mut cursor = POINT::default();
    let _ = GetCursorPos(&mut cursor);
    let monitor = match cfg.launcher_placement.as_str() {
        "primary_monitor" => MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTONEAREST),
        "focused_monitor" => {
            let foreground = GetForegroundWindow();
            if foreground.0.is_null() {
                MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST)
            } else {
                MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST)
            }
        }
        _ => MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST),
    };
    let mut info = MONITORINFO {
        cbSize: core::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(monitor, &mut info).as_bool() {
        info.rcWork
    } else {
        work_area_at(cursor)
    }
}

unsafe fn launcher_show(h: HWND) {
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    launcher_place(h, launcher_target_work_area(), false);
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
    // Recent results are for backspacing within one session: kept across a
    // close they could list, and let Enter open, a file deleted since.
    st.file_cache.clear();
    st.calc = None;
    st.wide = false;
    st.window_only = false;
    ALT_SWITCHER_MODE.store(false, Ordering::Relaxed);
}

/// Launch the selected shortcut/app/file via the shell (resolves target/args/dir).
unsafe fn launcher_launch(path: &str) {
    if let Some(command) = path.strip_prefix("cmd:") {
        launch(command);
        return;
    }
    let path = path.strip_prefix("url:").unwrap_or(path);
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
    let file: Vec<u16> = "explorer.exe"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
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
    la_header() + 6 + if st.wide { la_colhdr() } else { 0 }
}

/// Visible list rows for the current mode + client height.
fn launcher_rows(st: &LauncherState, ht: i32) -> usize {
    (((ht - 4) - launcher_list_top(st)) / la_row_h()).max(1) as usize
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
    let vis = ((y - list_top) / la_row_h()) as usize;
    let rows = launcher_rows(st, ht);
    if vis >= rows {
        return None;
    }
    let idx = launcher_scroll(st, rows) + vis;
    (idx < st.filtered.len()).then_some(idx)
}

fn is_builtin_icon(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "app"
            | "browser"
            | "calculator"
            | "clipboard"
            | "command"
            | "file"
            | "folder"
            | "grid"
            | "lock"
            | "media"
            | "power"
            | "power-circle"
            | "reload"
            | "restart"
            | "screenshot"
            | "search"
            | "settings"
            | "signout"
            | "sleep"
            | "setup"
            | "terminal"
            | "wallpaper"
            | "web"
            | "window"
    )
}

#[inline]
fn icon_coord(origin: i32, size: i32, value: i32) -> i32 {
    origin + (value * size + 12) / 24
}

unsafe fn icon_path(hdc: HDC, x: i32, y: i32, size: i32, points: &[(i32, i32)]) {
    let Some(&(x0, y0)) = points.first() else {
        return;
    };
    let _ = MoveToEx(hdc, icon_coord(x, size, x0), icon_coord(y, size, y0), None);
    for &(px, py) in &points[1..] {
        let _ = LineTo(hdc, icon_coord(x, size, px), icon_coord(y, size, py));
    }
}

/// Draw one or more cubic Bezier segments. Lucide paths used here need at most
/// four segments, so fixed stack storage avoids paint-time allocation.
unsafe fn icon_bezier(hdc: HDC, x: i32, y: i32, size: i32, points: &[(i32, i32)]) {
    const MAX_POINTS: usize = 13;
    let mut scaled = [POINT { x: 0, y: 0 }; MAX_POINTS];
    if points.len() < 4 || points.len() > MAX_POINTS || !(points.len() - 1).is_multiple_of(3) {
        return;
    }
    for (dst, &(px, py)) in scaled.iter_mut().zip(points) {
        dst.x = icon_coord(x, size, px);
        dst.y = icon_coord(y, size, py);
    }
    let _ = PolyBezier(hdc, &scaled[..points.len()]);
}

unsafe fn icon_circle(hdc: HDC, x: i32, y: i32, size: i32, cx: i32, cy: i32, radius: i32) {
    let _ = Ellipse(
        hdc,
        icon_coord(x, size, cx - radius),
        icon_coord(y, size, cy - radius),
        icon_coord(x, size, cx + radius),
        icon_coord(y, size, cy + radius),
    );
}

unsafe fn icon_round_rect(
    hdc: HDC,
    x: i32,
    y: i32,
    size: i32,
    rect: (i32, i32, i32, i32),
    radius: i32,
) {
    let diameter = icon_coord(0, size, radius * 2);
    let _ = RoundRect(
        hdc,
        icon_coord(x, size, rect.0),
        icon_coord(y, size, rect.1),
        icon_coord(x, size, rect.2),
        icon_coord(y, size, rect.3),
        diameter,
        diameter,
    );
}

/// Lucide 24x24 icon geometry adapted to allocation-free GDI primitives.
/// Geometric pens preserve Lucide's round caps/joins at small popup sizes.
unsafe fn draw_builtin_icon(hdc: HDC, name: &str, x: i32, y: i32, size: i32, color: u32) {
    let s = size.max(12);
    let pen_width = ((s * 2 + 12) / 24).max(1);
    let brush = LOGBRUSH {
        lbStyle: BS_SOLID,
        lbColor: COLORREF(color),
        lbHatch: 0,
    };
    let pen = ExtCreatePen(PS_GEOMETRIC | PS_SOLID, pen_width as u32, &brush, None);
    let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
    let old_brush = SelectObject(hdc, GetStockObject(NULL_BRUSH));
    match name.trim().to_ascii_lowercase().as_str() {
        "power-circle" => {
            icon_circle(hdc, x, y, s, 12, 12, 10);
            icon_path(hdc, x, y, s, &[(12, 7), (12, 11)]);
            icon_bezier(
                hdc,
                x,
                y,
                s,
                &[
                    (8, 9),
                    (5, 12),
                    (7, 17),
                    (10, 18),
                    (13, 20),
                    (19, 15),
                    (16, 9),
                ],
            );
        }
        "power" => {
            icon_path(hdc, x, y, s, &[(12, 2), (12, 12)]);
            icon_bezier(
                hdc,
                x,
                y,
                s,
                &[
                    (18, 7),
                    (22, 11),
                    (21, 17),
                    (17, 20),
                    (12, 23),
                    (4, 21),
                    (3, 15),
                    (2, 11),
                    (4, 8),
                    (6, 7),
                ],
            );
        }
        "lock" => {
            icon_round_rect(hdc, x, y, s, (3, 11, 21, 22), 2);
            icon_bezier(
                hdc,
                x,
                y,
                s,
                &[(7, 11), (7, 5), (9, 2), (12, 2), (15, 2), (17, 5), (17, 11)],
            );
        }
        "sleep" => {
            icon_bezier(
                hdc,
                x,
                y,
                s,
                &[
                    (21, 13),
                    (19, 20),
                    (12, 23),
                    (6, 19),
                    (0, 15),
                    (3, 5),
                    (12, 3),
                    (8, 9),
                    (13, 15),
                    (21, 13),
                ],
            );
        }
        "signout" => {
            icon_path(hdc, x, y, s, &[(16, 17), (21, 12), (16, 7)]);
            icon_path(hdc, x, y, s, &[(21, 12), (9, 12)]);
            icon_path(
                hdc,
                x,
                y,
                s,
                &[(9, 21), (5, 21), (3, 19), (3, 5), (5, 3), (9, 3)],
            );
        }
        "restart" => {
            icon_bezier(
                hdc,
                x,
                y,
                s,
                &[
                    (21, 12),
                    (21, 17),
                    (17, 21),
                    (12, 21),
                    (7, 21),
                    (3, 17),
                    (3, 12),
                    (3, 7),
                    (7, 3),
                    (12, 3),
                    (15, 3),
                    (17, 4),
                    (19, 6),
                ],
            );
            icon_path(hdc, x, y, s, &[(19, 6), (21, 8)]);
            icon_path(hdc, x, y, s, &[(21, 3), (21, 8), (16, 8)]);
        }
        "reload" => {
            icon_bezier(
                hdc,
                x,
                y,
                s,
                &[(3, 12), (3, 7), (7, 3), (12, 3), (15, 3), (17, 4), (19, 6)],
            );
            icon_path(hdc, x, y, s, &[(19, 6), (21, 8)]);
            icon_path(hdc, x, y, s, &[(21, 3), (21, 8), (16, 8)]);
            icon_bezier(
                hdc,
                x,
                y,
                s,
                &[
                    (21, 12),
                    (21, 17),
                    (17, 21),
                    (12, 21),
                    (9, 21),
                    (7, 20),
                    (5, 18),
                ],
            );
            icon_path(hdc, x, y, s, &[(5, 18), (3, 16)]);
            icon_path(hdc, x, y, s, &[(8, 16), (3, 16), (3, 21)]);
        }
        "settings" => {
            icon_path(hdc, x, y, s, &[(14, 17), (5, 17)]);
            icon_path(hdc, x, y, s, &[(19, 7), (10, 7)]);
            icon_circle(hdc, x, y, s, 17, 17, 3);
            icon_circle(hdc, x, y, s, 7, 7, 3);
        }
        "setup" => {
            icon_path(hdc, x, y, s, &[(10, 5), (3, 5)]);
            icon_path(hdc, x, y, s, &[(21, 5), (14, 5)]);
            icon_path(hdc, x, y, s, &[(14, 3), (14, 7)]);
            icon_path(hdc, x, y, s, &[(8, 12), (3, 12)]);
            icon_path(hdc, x, y, s, &[(21, 12), (12, 12)]);
            icon_path(hdc, x, y, s, &[(8, 10), (8, 14)]);
            icon_path(hdc, x, y, s, &[(12, 19), (3, 19)]);
            icon_path(hdc, x, y, s, &[(21, 19), (16, 19)]);
            icon_path(hdc, x, y, s, &[(16, 17), (16, 21)]);
        }
        "folder" => {
            icon_path(
                hdc,
                x,
                y,
                s,
                &[
                    (2, 14),
                    (2, 5),
                    (4, 3),
                    (8, 3),
                    (11, 6),
                    (18, 6),
                    (20, 8),
                    (20, 10),
                ],
            );
            icon_path(
                hdc,
                x,
                y,
                s,
                &[
                    (2, 14),
                    (6, 14),
                    (8, 10),
                    (20, 10),
                    (22, 12),
                    (20, 19),
                    (18, 21),
                    (4, 21),
                    (2, 19),
                    (2, 14),
                ],
            );
        }
        "screenshot" => {
            icon_path(hdc, x, y, s, &[(3, 7), (3, 5), (5, 3), (7, 3)]);
            icon_path(hdc, x, y, s, &[(17, 3), (19, 3), (21, 5), (21, 7)]);
            icon_path(hdc, x, y, s, &[(21, 17), (21, 19), (19, 21), (17, 21)]);
            icon_path(hdc, x, y, s, &[(7, 21), (5, 21), (3, 19), (3, 17)]);
            icon_path(hdc, x, y, s, &[(7, 12), (17, 12)]);
        }
        "wallpaper" => {
            icon_round_rect(hdc, x, y, s, (3, 3, 21, 21), 2);
            icon_circle(hdc, x, y, s, 9, 9, 2);
            icon_path(hdc, x, y, s, &[(6, 21), (15, 12), (17, 12), (21, 16)]);
        }
        "command" | "terminal" => {
            icon_round_rect(hdc, x, y, s, (2, 3, 22, 21), 2);
            icon_path(hdc, x, y, s, &[(6, 8), (10, 12), (6, 16)]);
            icon_path(hdc, x, y, s, &[(13, 16), (18, 16)]);
        }
        "clipboard" => {
            icon_round_rect(hdc, x, y, s, (5, 4, 19, 22), 2);
            icon_round_rect(hdc, x, y, s, (8, 2, 16, 6), 1);
        }
        "web" | "browser" => {
            icon_circle(hdc, x, y, s, 12, 12, 10);
            icon_bezier(
                hdc,
                x,
                y,
                s,
                &[
                    (12, 2),
                    (7, 6),
                    (7, 18),
                    (12, 22),
                    (17, 18),
                    (17, 6),
                    (12, 2),
                ],
            );
            icon_path(hdc, x, y, s, &[(3, 9), (21, 9)]);
            icon_path(hdc, x, y, s, &[(3, 15), (21, 15)]);
        }
        "calculator" => {
            icon_round_rect(hdc, x, y, s, (4, 2, 20, 22), 2);
            icon_path(hdc, x, y, s, &[(4, 8), (20, 8)]);
            icon_path(hdc, x, y, s, &[(8, 12), (8, 18)]);
            icon_path(hdc, x, y, s, &[(6, 15), (10, 15)]);
            icon_path(hdc, x, y, s, &[(14, 13), (18, 17)]);
            icon_path(hdc, x, y, s, &[(18, 13), (14, 17)]);
        }
        "file" => {
            icon_path(hdc, x, y, s, &[(14, 2), (14, 8), (20, 8)]);
            icon_path(
                hdc,
                x,
                y,
                s,
                &[
                    (14, 2),
                    (6, 2),
                    (4, 4),
                    (4, 20),
                    (6, 22),
                    (18, 22),
                    (20, 20),
                    (20, 8),
                    (14, 2),
                ],
            );
        }
        "window" => {
            icon_round_rect(hdc, x, y, s, (2, 4, 22, 20), 2);
            icon_path(hdc, x, y, s, &[(2, 9), (22, 9)]);
        }
        "media" => {
            icon_path(hdc, x, y, s, &[(6, 3), (20, 12), (6, 21), (6, 3)]);
        }
        "search" => {
            icon_circle(hdc, x, y, s, 11, 11, 8);
            icon_path(hdc, x, y, s, &[(17, 17), (22, 22)]);
        }
        "app" | "grid" => {
            icon_round_rect(hdc, x, y, s, (3, 3, 9, 9), 1);
            icon_round_rect(hdc, x, y, s, (15, 3, 21, 9), 1);
            icon_round_rect(hdc, x, y, s, (3, 15, 9, 21), 1);
            icon_round_rect(hdc, x, y, s, (15, 15, 21, 21), 1);
        }
        _ => {
            icon_circle(hdc, x, y, s, 12, 12, 8);
        }
    }
    SelectObject(hdc, old_brush);
    SelectObject(hdc, old_pen);
    let _ = DeleteObject(HGDIOBJ(pen.0));
}

/// Physical px of a `logical`-size icon on each connected monitor, deduped.
unsafe fn icon_sizes(logical: i32) -> Vec<i32> {
    let mut mons: Vec<(isize, RECT)> = Vec::new();
    let _ = EnumDisplayMonitors(
        None,
        None,
        Some(bar_mon_enum),
        LPARAM(&mut mons as *mut _ as isize),
    );
    let mut sizes: Vec<i32> = mons
        .iter()
        .map(|&(hmon, _)| dpi_px(logical, monitor_dpi(hmon)))
        .collect();
    sizes.sort_unstable();
    sizes.dedup();
    sizes
}

/// Every launcher icon size a connected monitor needs (the popup's own if
/// none enumerated).
unsafe fn launcher_icon_sizes() -> Vec<i32> {
    let sizes = icon_sizes(LA_ICON_CFG.load(Ordering::Relaxed));
    if sizes.is_empty() {
        vec![la_icon_px()]
    } else {
        sizes
    }
}

/// Replace any queued app-icon jobs with one per app at every size in
/// `sizes` (launcher_icon_sizes). At the popup's own DPI only, the startup
/// preload ran at 96 (UI_DPI is set only once the popup is placed), so every
/// icon on a 125%/150% monitor was drawn upscaled from 32 px.
unsafe fn launcher_queue_app_icons(n: usize, sizes: &[i32]) {
    let mut q = ICON_QUEUE.lock().unwrap();
    q.retain(|job| !matches!(job, IconJob::App(..)));
    for &px in sizes {
        q.extend((0..n).map(|i| IconJob::App(i, px)));
    }
    drop(q);
    ICON_CV.notify_all();
}

/// Draw `source`'s icon from `cache` into a `px` box at (x, y). The exact size
/// is a 1:1 draw (no scaling blur); DrawIconEx composites the icon's own
/// straight alpha — no premultiply, no halo. Until that size has resolved,
/// the nearest other cached size is drawn scaled rather than leaving a gap
/// (a new monitor DPI, a changed icon size), and true is returned so the
/// caller queues the exact size; the scaled draw lasts one resolve.
unsafe fn draw_cached_icon(
    hdc: HDC,
    cache: &Mutex<IconCache<isize>>,
    source: &str,
    stamp: u64,
    x: i32,
    y: i32,
    px: i32,
) -> bool {
    let (icon, missing) = {
        let mut c = cache.lock().unwrap();
        match c.get(source, px, stamp) {
            Some(icon) => (icon, false),
            None => (
                c.nearest(source, px, stamp, |icon| icon > 1).unwrap_or(0),
                true,
            ),
        }
    };
    if icon > 1 {
        let _ = DrawIconEx(
            hdc,
            x,
            y,
            HICON(icon as *mut c_void),
            px,
            px,
            0,
            None,
            DI_NORMAL,
        );
    }
    missing
}

unsafe fn launcher_paint(h: HWND) {
    make_launcher_font();
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
    let border = popup_border();
    let inner = RECT {
        left: rc.left + border,
        top: rc.top + border,
        right: rc.right - border,
        bottom: rc.bottom - border,
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
        left: la_pad(),
        top: 0,
        right: w - la_pad(),
        bottom: la_header(),
    };
    if st.query.is_empty() {
        SetTextColor(hdc, COLORREF(p.dim));
        let prompt = if st.window_only {
            "Switch windows"
        } else {
            "Search apps, files and commands…"
        };
        let mut v: Vec<u16> = prompt.encode_utf16().collect();
        DrawTextW(
            hdc,
            &mut v,
            &mut qr,
            DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
    } else {
        SetTextColor(hdc, COLORREF(p.fg));
        // Trailing caret marks the input (the picker is owner-drawn, no edit ctrl).
        let mut v: Vec<u16> = format!("{}\u{258f}", st.query).encode_utf16().collect();
        DrawTextW(
            hdc,
            &mut v,
            &mut qr,
            DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
    }
    // Divider under the query row.
    let div = RECT {
        left: la_pad(),
        top: la_header(),
        right: w - la_pad(),
        bottom: la_header() + 1,
    };
    let dbrush = CreateSolidBrush(COLORREF(p.divider));
    FillRect(hdc, &div, dbrush);
    let _ = DeleteObject(HGDIOBJ(dbrush.0));

    // Result rows: st.scroll drives the viewport (wheel scrolls it; the keyboard
    // arms keep the selection visible). Wide (Tab) adds Modified/Size/Path columns.
    let list_top = launcher_list_top(&st);
    let rows = launcher_rows(&st, ht);
    let scroll = launcher_scroll(&st, rows);
    let text_left = la_pad() + 6 + la_icon_px() + 10;
    // Wide-mode column x's, anchored off the right edge; path gets the big share.
    let col_path_w = (w as f64 * 0.40) as i32;
    let path_x = w - la_pad() - 6 - col_path_w;
    let size_x = path_x - col_size_w();
    let date_x = size_x - col_date_w();
    if st.wide {
        // Dim column headers in the band under the query divider.
        SetTextColor(hdc, COLORREF(p.dim));
        let hdr = |x0: i32, x1: i32, label: &str, extra: DRAW_TEXT_FORMAT| {
            let mut r = RECT {
                left: x0,
                top: la_header() + 2,
                right: x1,
                bottom: la_header() + 2 + la_colhdr(),
            };
            let mut v: Vec<u16> = label.encode_utf16().collect();
            DrawTextW(
                hdc,
                &mut v,
                &mut r,
                DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | extra,
            );
        };
        hdr(text_left, date_x - 10, "Name", DRAW_TEXT_FORMAT(0));
        hdr(date_x, size_x - 8, "Modified", DRAW_TEXT_FORMAT(0));
        hdr(size_x, size_x + col_size_w() - 16, "Size", DT_RIGHT);
        hdr(path_x, w - la_pad(), "Path", DRAW_TEXT_FORMAT(0));
    }
    let mut want: Vec<IconJob> = Vec::new(); // visible app/file icons still missing
    for vis in 0..rows {
        let idx = scroll + vis;
        if idx >= st.filtered.len() {
            break;
        }
        let hit = st.filtered[idx];
        let top = list_top + vis as i32 * la_row_h();
        let row = RECT {
            left: la_pad(),
            top,
            right: w - la_pad(),
            bottom: top + la_row_h(),
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
                top + la_row_h() - 3,
                la_sel_radius(),
                la_sel_radius(),
            );
            SelectObject(hdc, ob);
            SelectObject(hdc, op);
            let _ = DeleteObject(HGDIOBJ(sel.0));
            let _ = DeleteObject(HGDIOBJ(pen.0));
            SetTextColor(hdc, COLORREF(p.selfg));
        } else {
            SetTextColor(hdc, COLORREF(p.fg));
        }
        // Provider-only rows: compact marker/glyph plus one line, no metadata.
        match hit {
            Hit::Calc | Hit::Web | Hit::Clipboard(_) | Hit::Emoji(_) => {
                let keep = if idx == st.sel { p.selfg } else { p.dim };
                let icon_x = row.left + 6;
                let icon_y = top + (la_row_h() - la_icon_px()) / 2;
                if let Hit::Emoji(i) = hit {
                    let mut gr = RECT {
                        left: icon_x,
                        top,
                        right: icon_x + la_icon_px(),
                        bottom: top + la_row_h(),
                    };
                    SetTextColor(hdc, COLORREF(keep));
                    let mut glyph: Vec<u16> = st.emoji[i].text.encode_utf16().collect();
                    DrawTextW(
                        hdc,
                        &mut glyph,
                        &mut gr,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );
                } else {
                    let icon = match hit {
                        Hit::Calc => "calculator",
                        Hit::Web => "web",
                        Hit::Clipboard(_) => "clipboard",
                        _ => "command",
                    };
                    draw_builtin_icon(hdc, icon, icon_x, icon_y, la_icon_px(), keep);
                }
                let text = match hit {
                    Hit::Calc => format!("{}   (Enter copies)", st.calc.as_deref().unwrap_or("")),
                    Hit::Web => format!("Search the web for \u{201c}{}\u{201d}", st.query.trim()),
                    Hit::Clipboard(i) => st.clipboard[i].replace(['\r', '\n'], " "),
                    Hit::Emoji(i) => st.emoji[i].name.clone(),
                    _ => String::new(),
                };
                SetTextColor(hdc, COLORREF(if idx == st.sel { p.selfg } else { p.fg }));
                let mut tr = RECT {
                    left: text_left,
                    ..row
                };
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
                if is_builtin_icon(&e.icon_path) {
                    let iy = top + (la_row_h() - la_icon_px()) / 2;
                    draw_builtin_icon(
                        hdc,
                        &e.icon_path,
                        row.left + 6,
                        iy,
                        la_icon_px(),
                        if idx == st.sel { p.selfg } else { p.dim },
                    );
                } else {
                    let iy = top + (la_row_h() - la_icon_px()) / 2;
                    let px = la_icon_px();
                    if draw_cached_icon(
                        hdc,
                        &ICON_CACHE,
                        &e.icon_path,
                        e.stamp,
                        row.left + 6,
                        iy,
                        px,
                    ) {
                        want.push(IconJob::App(i, px));
                    }
                }
                (
                    e.name.as_str(),
                    String::new(),
                    String::new(),
                    e.path.as_str(),
                )
            }
            Hit::Window(i) => {
                let win = &st.windows[i];
                // ICON_CACHE by the exe read when the list was built: this
                // thread is the only one that draws it (so LA_REFRESH can free
                // it), and a paint no longer opens each row's process.
                if !win.exe.is_empty() {
                    let iy = top + (la_row_h() - la_icon_px()) / 2;
                    let px = la_icon_px();
                    if draw_cached_icon(hdc, &ICON_CACHE, &win.exe, 0, row.left + 6, iy, px) {
                        want.push(IconJob::Exe(win.exe.clone(), px));
                    }
                }
                (
                    win.title.as_str(),
                    String::new(),
                    String::new(),
                    win.exe.as_str(),
                )
            }
            Hit::File(i) => {
                let f = &st.files[i];
                let iy = top + (la_row_h() - la_icon_px()) / 2;
                let px = la_icon_px();
                if draw_cached_icon(
                    hdc,
                    &FILE_ICON_CACHE,
                    &f.path,
                    f.stamp(),
                    row.left + 6,
                    iy,
                    px,
                ) {
                    want.push(IconJob::File(st.search_gen, i, px));
                }
                (
                    f.name.as_str(),
                    if f.date > 0.0 {
                        fmt_oadate(f.date)
                    } else {
                        String::new()
                    },
                    fmt_size(f.size),
                    f.path.as_str(),
                )
            }
            // Drawn above (with an early continue).
            Hit::Calc | Hit::Web | Hit::Clipboard(_) | Hit::Emoji(_) => unreachable!(),
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
            SetTextColor(hdc, COLORREF(if idx == st.sel { p.selfg } else { p.dim }));
            let cell = |x0: i32, x1: i32, s: &str, extra: DRAW_TEXT_FORMAT| {
                if s.is_empty() {
                    return;
                }
                let mut r = RECT {
                    left: x0,
                    right: x1,
                    ..row
                };
                let mut v: Vec<u16> = s.encode_utf16().collect();
                DrawTextW(
                    hdc,
                    &mut v,
                    &mut r,
                    DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS | extra,
                );
            };
            cell(date_x, size_x - 8, &date_s, DRAW_TEXT_FORMAT(0));
            cell(size_x, size_x + col_size_w() - 16, &size_s, DT_RIGHT);
            cell(path_x, w - la_pad(), path_s, DRAW_TEXT_FORMAT(0));
        }
    }

    if let Some(of) = old_font {
        SelectObject(hdc, of);
    }
    // Trim the file-icon LRU here: this is the only thread that draws file
    // icons and this paint is done with them, and a row still listed (even
    // scrolled out of view) keeps its icon.
    {
        let mut cache = FILE_ICON_CACHE.lock().unwrap();
        if cache.over_cap() {
            let listed: std::collections::HashSet<&str> =
                st.files.iter().map(|f| f.path.as_str()).collect();
            cache.evict(
                |src| listed.contains(src),
                |icon| release_launcher_icon(icon),
            );
        }
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
                    // Remember what had focus. The picker is NOACTIVATE, but
                    // hiding it does not synchronously hand foreground back, so
                    // a paste fired straight after the hide raced focus
                    // restoration and could land in the wrong window (B-13).
                    LAUNCHER_PREV_FG.store(GetForegroundWindow().0 as isize, Ordering::Relaxed);
                    {
                        let mut st = LAUNCHER_STATE.lock().unwrap();
                        if !st.loaded {
                            st.all = launcher_enumerate();
                            st.loaded = true;
                        }
                        st.query.clear();
                        st.sel = 0;
                        st.scroll = 0;
                        // A search still in flight from the last session must
                        // not land under the fresh, empty query.
                        launcher_cancel_search();
                        st.files.clear();
                        let cfg = UI_CFG
                            .lock()
                            .unwrap()
                            .clone()
                            .unwrap_or_else(Config::defaults);
                        st.windows = if cfg.launcher_source_windows {
                            launcher_windows()
                        } else {
                            Vec::new()
                        };
                        st.clipboard = if cfg.launcher_source_clipboard && cfg.clipboard_history {
                            CLIPBOARD_ITEMS.lock().unwrap().iter().cloned().collect()
                        } else {
                            Vec::new()
                        };
                        st.emoji = if cfg.launcher_source_emoji && cfg.emoji_picker {
                            emoji_catalog()
                        } else {
                            Vec::new()
                        };
                        st.wide = false;
                        st.window_only = false;
                        launcher_refilter(&mut st);
                    }
                    launcher_show(h);
                }
                LA_OPEN_SWITCHER => {
                    {
                        let mut st = LAUNCHER_STATE.lock().unwrap();
                        st.query.clear();
                        launcher_cancel_search();
                        st.files.clear();
                        st.windows = launcher_windows();
                        st.clipboard.clear();
                        st.emoji.clear();
                        st.wide = false;
                        st.window_only = true;
                        st.scroll = 0;
                        st.sel = 0;
                        launcher_refilter(&mut st);
                        if st.filtered.len() > 1 {
                            st.sel = 1;
                        }
                    }
                    launcher_show(h);
                }
                LA_REFRESH => {
                    let apps = launcher_enumerate();
                    let n = apps.len();
                    let sizes = launcher_icon_sizes();
                    {
                        let mut st = LAUNCHER_STATE.lock().unwrap();
                        st.all = apps;
                        st.loaded = true;
                        st.sel = 0;
                        st.scroll = 0;
                        launcher_refilter(&mut st);
                        // Free what no row can ask for again: a size no
                        // monitor uses (launcher_icon_size changed) and an
                        // app's superseded or removed stamp. Safe here: only
                        // this thread draws ICON_CACHE, and it is not painting.
                        // Stamp 0 (UWP ids, switcher exes) goes by size only.
                        let live: std::collections::HashSet<(&str, u64)> = st
                            .all
                            .iter()
                            .map(|e| (e.icon_path.as_str(), e.stamp))
                            .collect();
                        ICON_CACHE.lock().unwrap().retain_live(
                            |src, px, stamp| {
                                sizes.contains(&px) && (stamp == 0 || live.contains(&(src, stamp)))
                            },
                            |icon| release_launcher_icon(icon),
                        );
                    }
                    // Cached icons are reused (the worker skips hits), so a
                    // refresh re-extracts only new or changed entries.
                    launcher_queue_app_icons(n, &sizes);
                    let _ = InvalidateRect(h, None, BOOL(0));
                }
                LA_CHAR => {
                    // Only the hook's Space arrives here; LA_KEY edits inline.
                    launcher_edit_query(h, |q| {
                        if let Some(c) = char::from_u32(l.0 as u32) {
                            q.push(c);
                        }
                    });
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
                    // wFlags = 0, not 0x4 (no state change): 0 keeps a pending
                    // dead key in this thread's buffer, so ´ then e composes é
                    // on US-Intl / German / French layouts.
                    let mut buf = [0u16; 8];
                    let n = ToUnicode(vk, scan, Some(&state), &mut buf, 0);
                    if n >= 1 {
                        if let Some(c) = char::decode_utf16(buf[..n as usize].iter().copied())
                            .next()
                            .and_then(|r| r.ok())
                            .filter(|c| *c >= ' ')
                        {
                            // Inline, not re-posted as LA_CHAR: a re-post lands
                            // behind the Space / Backspace / Enter the hook
                            // already queued, so whenever this thread lagged,
                            // "cod"+Enter launched the pick for "co".
                            launcher_edit_query(h, |q| q.push(c));
                        }
                    }
                }
                LA_BACK => {
                    launcher_edit_query(h, |q| {
                        q.pop();
                    });
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
                        Paste(String),
                        Focus(isize),
                        Web(String),
                        None,
                    }
                    let action = {
                        let st = LAUNCHER_STATE.lock().unwrap();
                        match st.filtered.get(st.sel) {
                            Some(Hit::App(i)) => st
                                .all
                                .get(*i)
                                .map(|e| {
                                    launcher_mru_bump(&e.path);
                                    Act::Open(e.path.clone())
                                })
                                .unwrap_or(Act::None),
                            Some(Hit::File(i)) => st
                                .files
                                .get(*i)
                                .map(|f| {
                                    launcher_mru_bump(&f.path);
                                    Act::Open(f.path.clone())
                                })
                                .unwrap_or(Act::None),
                            Some(Hit::Window(i)) => st
                                .windows
                                .get(*i)
                                .map(|win| {
                                    launcher_mru_bump(&win.exe);
                                    Act::Focus(win.hwnd)
                                })
                                .unwrap_or(Act::None),
                            Some(Hit::Clipboard(i)) => st
                                .clipboard
                                .get(*i)
                                .cloned()
                                .map(Act::Paste)
                                .unwrap_or(Act::None),
                            Some(Hit::Emoji(i)) => st
                                .emoji
                                .get(*i)
                                .map(|item| Act::Paste(item.text.clone()))
                                .unwrap_or(Act::None),
                            Some(Hit::Calc) => st.calc.clone().map(Act::Copy).unwrap_or(Act::None),
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
                        Act::Paste(s) => paste_text(h, &s),
                        Act::Focus(hwnd) => push_cmd(Cmd::ActivateWindow(hwnd)),
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
                        RECT {
                            left: 0,
                            top: 0,
                            right: 1920,
                            bottom: 1080,
                        }
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
                        let next = if l.0 > 0 {
                            cur.saturating_sub(1)
                        } else {
                            (cur + 1).min(maxs)
                        };
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
        WM_CLIPBOARDUPDATE => {
            clipboard_capture(h);
            LRESULT(0)
        }
        WM_PAINT => {
            launcher_paint(h);
            LRESULT(0)
        }
        // Scale changed under an open popup: re-place it (which re-reads
        // UI_DPI) and let the next paint rebuild the font.
        WM_DPICHANGED => {
            // Two traps here, both hit by the same re-entrancy:
            //  1. `LAUNCHER_STATE.lock().unwrap().wide` passed as an ARGUMENT
            //     keeps the guard alive for the whole call. launcher_place ->
            //     SetWindowPos -> a synchronous WM_DPICHANGED back into this
            //     arm -> lock() on a std Mutex we already hold = deadlock, on
            //     the launcher thread, permanently.
            //  2. Even without that, re-placing from inside the message that
            //     announced the move recurses. See `request_bar_rebuild` for
            //     the same problem on the bars.
            let wide = LAUNCHER_STATE.lock().unwrap().wide;
            if !POPUP_PLACING.swap(true, Ordering::Relaxed) {
                launcher_place(h, launcher_target_work_area(), wide);
                POPUP_PLACING.store(false, Ordering::Relaxed);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        _ => DefWindowProcW(h, msg, w, l),
    }
}

/// Launcher thread: registers its class, creates the (hidden) picker window, and
/// pumps its own message loop. Idle until the hook posts `WM_LAUNCHER`.
fn launcher_thread() {
    raise_current_thread(ThreadRole::Launcher);
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
            la_w(),
            la_h(),
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
        let _ = AddClipboardFormatListener(hwnd);
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
            launcher_queue_app_icons(n, &launcher_icon_sizes());
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

#[inline]
fn sm_w() -> i32 {
    dpi_px(SM_W_CFG.load(Ordering::Relaxed), ui_dpi())
}
const SYSMENU_HEADER: i32 = 44;
const SYSMENU_FOOTER: i32 = 34; // hint / confirm banner
/// Header/footer bands, scaled to the monitor the menu is on.
#[inline]
fn sm_header() -> i32 {
    dpi_px(SYSMENU_HEADER, ui_dpi())
}
#[inline]
fn sm_footer() -> i32 {
    dpi_px(SYSMENU_FOOTER, ui_dpi())
}

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

#[derive(Clone, PartialEq)]
enum SysAct {
    Lock,
    Sleep,
    Hibernate,
    SignOut,
    Restart,
    Shutdown,
    OpenConfig,
    OpenSettings,
    Reload,
    RestartAstur,
    Screenshot,
    SetWallpaper(String),
    Command(String),
}

#[derive(Clone)]
enum SysKind {
    Category(Vec<SysItem>),
    Action(SysAct, bool),
}

#[derive(Clone)]
struct SysItem {
    label: String,
    icon: String,
    kind: SysKind,
}

fn sys_action(label: &str, icon: &str, act: SysAct, confirm: bool) -> SysItem {
    SysItem {
        label: label.to_string(),
        icon: icon.to_string(),
        kind: SysKind::Action(act, confirm),
    }
}

fn builtin_sys_item(id: &str, wallpaper_dir: &str) -> Option<SysItem> {
    match id.trim().to_ascii_lowercase().as_str() {
        "lock" => Some(sys_action("Lock", "lock", SysAct::Lock, false)),
        "sleep" => Some(sys_action("Sleep", "sleep", SysAct::Sleep, false)),
        "hibernate" => Some(sys_action("Hibernate", "sleep", SysAct::Hibernate, false)),
        "sign_out" | "signout" => Some(sys_action("Sign out", "signout", SysAct::SignOut, true)),
        "restart" => Some(sys_action("Restart", "restart", SysAct::Restart, true)),
        "shutdown" | "shut_down" => Some(sys_action("Shut down", "power", SysAct::Shutdown, true)),
        "settings" => Some(sys_action(
            "Settings",
            "settings",
            SysAct::OpenSettings,
            false,
        )),
        "open_config" => Some(sys_action(
            "Open config folder",
            "folder",
            SysAct::OpenConfig,
            false,
        )),
        "reload" => Some(sys_action(
            "Reload configuration",
            "reload",
            SysAct::Reload,
            false,
        )),
        "restart_astur" => Some(sys_action(
            "Restart Astur",
            "restart",
            SysAct::RestartAstur,
            true,
        )),
        "screenshot" => Some(sys_action(
            "Screenshot",
            "screenshot",
            SysAct::Screenshot,
            false,
        )),
        "wallpapers" | "wallpaper" => {
            let items = wallpaper_items(wallpaper_dir);
            (!items.is_empty()).then(|| SysItem {
                label: "Wallpaper".to_string(),
                icon: "wallpaper".to_string(),
                kind: SysKind::Category(items),
            })
        }
        _ => None,
    }
}

fn wallpaper_items(configured: &str) -> Vec<SysItem> {
    let dir = if configured.trim().is_empty() {
        config_path("ASTUR_WALLPAPERS", "wallpapers")
    } else {
        std::path::PathBuf::from(configured)
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<SysItem> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let ext = path.extension()?.to_str()?.to_ascii_lowercase();
            if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "bmp") {
                return None;
            }
            let label = path.file_stem()?.to_string_lossy().into_owned();
            Some(SysItem {
                label,
                icon: path.to_string_lossy().into_owned(),
                kind: SysKind::Action(
                    SysAct::SetWallpaper(path.to_string_lossy().into_owned()),
                    false,
                ),
            })
        })
        .collect();
    out.sort_by(|a, b| {
        a.label
            .to_ascii_lowercase()
            .cmp(&b.label.to_ascii_lowercase())
    });
    out
}

fn build_system_root() -> Vec<SysItem> {
    let cfg = UI_CFG
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(Config::defaults);
    let mut power: Vec<SysItem> = cfg
        .system_power_items
        .iter()
        .filter_map(|id| builtin_sys_item(id, &cfg.wallpaper_dir))
        .collect();
    let mut setup: Vec<SysItem> = cfg
        .system_setup_items
        .iter()
        .filter_map(|id| builtin_sys_item(id, &cfg.wallpaper_dir))
        .collect();
    let mut extras: Vec<(String, Vec<SysItem>)> = Vec::new();
    for action in cfg.system_actions {
        let category = action.category.clone();
        let item = SysItem {
            label: action.label,
            icon: action.icon,
            kind: SysKind::Action(SysAct::Command(action.target), action.confirm),
        };
        if category.eq_ignore_ascii_case("power") {
            power.push(item);
        } else if category.eq_ignore_ascii_case("setup") {
            setup.push(item);
        } else if let Some((_, items)) = extras
            .iter_mut()
            .find(|(name, _)| name.eq_ignore_ascii_case(&category))
        {
            items.push(item);
        } else {
            extras.push((category, vec![item]));
        }
    }
    let mut root = Vec::new();
    if !power.is_empty() {
        root.push(SysItem {
            label: "Power".to_string(),
            icon: "power-circle".to_string(),
            kind: SysKind::Category(power),
        });
    }
    if !setup.is_empty() {
        root.push(SysItem {
            label: "Setup".to_string(),
            icon: "setup".to_string(),
            kind: SysKind::Category(setup),
        });
    }
    for (label, items) in extras {
        root.push(SysItem {
            label,
            icon: "command".to_string(),
            kind: SysKind::Category(items),
        });
    }
    root
}

struct SysMenuState {
    items: Vec<SysItem>,
    title: String,
    sel: usize,
    confirm: bool,
    stack: Vec<(String, Vec<SysItem>)>,
}
static SYSMENU_STATE: Mutex<SysMenuState> = Mutex::new(SysMenuState {
    items: Vec::new(),
    title: String::new(),
    sel: 0,
    confirm: false,
    stack: Vec::new(),
});
/// A custom system-menu icon, from SYSMENU_ICON_CACHE keyed by (source, px,
/// stamp). Keyed on the source alone it kept the first size it resolved at,
/// so the menu opened on a monitor of another scale drew it rescaled.
unsafe fn sysmenu_custom_icon(source: &str) -> isize {
    if source.is_empty()
        || (!std::path::Path::new(source).exists() && !source.starts_with("shell:"))
    {
        return 0;
    }
    let px = la_icon_px();
    let stamp = icon_stamp(source);
    if let Some(icon) = SYSMENU_ICON_CACHE.lock().unwrap().get(source, px, stamp) {
        return icon;
    }
    // Resolved on this thread, outside the lock, as before.
    let icon = load_icon(source, px);
    let sizes = launcher_icon_sizes();
    // One guard for the rest: re-locking inside a `match` on a lock()
    // temporary self-deadlocks, since the temporary lives to the match's end.
    let mut cache = SYSMENU_ICON_CACHE.lock().unwrap();
    // This source at a size no monitor uses or an older stamp is never asked
    // for again. Free now: only this thread draws these, and DrawIconEx has
    // finished with any it drew earlier in this paint.
    cache.retain_live(
        |src, p, s| src != source || (s == stamp && sizes.contains(&p)),
        |old| release_launcher_icon(old),
    );
    match cache.insert(source, px, stamp, icon, 0) {
        Some(dup) => {
            release_launcher_icon(dup);
            cache.get(source, px, stamp).unwrap_or(0)
        }
        None => icon,
    }
}
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

unsafe fn reload_config_now() {
    let cfg = match load_config() {
        Ok(cfg) => cfg,
        Err(e) => {
            log_error!("config reload skipped, current settings kept: {e}");
            return;
        }
    };
    apply_hook_config(&cfg);
    apply_theme(&cfg);
    apply_bar_statics(&cfg);
    let launcher = LAUNCHER_HWND.load(Ordering::Relaxed);
    if launcher != 0 {
        let _ = PostMessageW(
            hwnd_from(launcher),
            WM_LAUNCHER,
            WPARAM(LA_REFRESH),
            LPARAM(0),
        );
    }
    // Full, never diffed: an explicit reload (IPC, system menu, extra hotkey)
    // is the user's recovery tool for windows left mis-styled.
    push_cmd(Cmd::Reload(Box::new(cfg), true));
    let hm = MARKER_HWND.load(Ordering::Relaxed);
    if hm != 0 {
        let _ = PostMessageW(hwnd_from(hm), WM_RELOAD, WPARAM(0), LPARAM(0));
    }
}

unsafe fn sysmenu_exec(act: SysAct) {
    match act {
        SysAct::Lock => {
            let _ = LockWorkStation();
        }
        SysAct::Sleep => {
            let _ = SetSuspendState(BOOLEAN(0), BOOLEAN(0), BOOLEAN(0));
        }
        SysAct::Hibernate => {
            let _ = SetSuspendState(BOOLEAN(1), BOOLEAN(0), BOOLEAN(0));
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
            if let Some(dir) = config_path("ASTUR_CONFIG", "astur.conf").parent() {
                launcher_launch(&dir.to_string_lossy());
            }
        }
        SysAct::Reload => reload_config_now(),
        SysAct::RestartAstur => {
            if let Ok(exe) = std::env::current_exe() {
                restore_all_windows();
                // Explicit hand-off: the replacement waits for this PID to exit
                // before claiming the single-instance lock, so the two never
                // manage the same windows at once.
                let _ = std::process::Command::new(exe)
                    .arg("--wait-for-pid")
                    .arg(std::process::id().to_string())
                    .spawn();
                std::process::exit(0);
            }
        }
        SysAct::Screenshot => launcher_launch("ms-screenclip:"),
        SysAct::SetWallpaper(path) => queue_wallpaper(&path),
        SysAct::Command(target) => launcher_launch(&target),
    }
}
/// Size + center the menu to the current level's row count, then repaint.
unsafe fn sysmenu_layout(h: HWND) {
    let n = SYSMENU_STATE.lock().unwrap().items.len() as i32;
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    // Same rule as the launcher: adopt the monitor's scale before reading any
    // metric.
    set_ui_dpi(dpi_at(pt));
    let wa = work_area_at(pt);
    let hgt = sm_header() + 6 + n * la_row_h() + sm_footer() + 6;
    let x = (wa.left + wa.right) / 2 - sm_w() / 2;
    let y = (wa.top + wa.bottom) / 2 - hgt / 2;
    let _ = SetWindowPos(h, HWND_TOPMOST, x, y, sm_w(), hgt, SWP_NOACTIVATE);
    shape_popup(h, sm_w(), hgt);
    // Publish bounds for the hook's click-outside dismiss + wheel routing, and
    // re-seed the hover baseline (the menu just moved/resized under the cursor).
    SYSMENU_RECT_L.store(x, Ordering::Relaxed);
    SYSMENU_RECT_T.store(y, Ordering::Relaxed);
    SYSMENU_RECT_R.store(x + sm_w(), Ordering::Relaxed);
    SYSMENU_RECT_B.store(y + hgt, Ordering::Relaxed);
    SYSMENU_LAST_MX.store(pt.x, Ordering::Relaxed);
    SYSMENU_LAST_MY.store(pt.y, Ordering::Relaxed);
    let _ = InvalidateRect(h, None, BOOL(0));
}

/// Menu-row index under a client-space `y` (rows sit under the title, fixed pitch).
fn sysmenu_row_hit(n: usize, y: i32) -> Option<usize> {
    let top = sm_header() + 6;
    if y < top {
        return None;
    }
    let i = ((y - top) / la_row_h()) as usize;
    (i < n).then_some(i)
}

unsafe fn sysmenu_show(h: HWND) {
    {
        let mut st = SYSMENU_STATE.lock().unwrap();
        st.items = build_system_root();
        if st.items.is_empty() {
            st.items.push(sys_action(
                "Settings",
                "settings",
                SysAct::OpenSettings,
                false,
            ));
        }
        st.title = "System".to_string();
        st.sel = 0;
        st.confirm = false;
        st.stack.clear();
    }
    sysmenu_layout(h);
    apply_acrylic(h, ACRYLIC_ON.load(Ordering::Relaxed));
    let _ = ShowWindow(h, SW_SHOWNA);
}

unsafe fn sysmenu_close(h: HWND) {
    let _ = ShowWindow(h, SW_HIDE);
    SYSMENU_OPEN.store(false, Ordering::Relaxed);
    let mut st = SYSMENU_STATE.lock().unwrap();
    st.items.clear();
    st.title.clear();
    st.sel = 0;
    st.confirm = false;
    st.stack.clear();
}

unsafe fn sysmenu_paint(h: HWND) {
    make_launcher_font();
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
    let border = popup_border();
    let inner = RECT {
        left: rc.left + border,
        top: rc.top + border,
        right: rc.right - border,
        bottom: rc.bottom - border,
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
        left: la_pad(),
        top: 0,
        right: w - la_pad(),
        bottom: sm_header(),
    };
    let mut tv: Vec<u16> = st.title.encode_utf16().collect();
    DrawTextW(
        hdc,
        &mut tv,
        &mut tr,
        DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
    let div = RECT {
        left: la_pad(),
        top: sm_header(),
        right: w - la_pad(),
        bottom: sm_header() + 1,
    };
    let db = CreateSolidBrush(COLORREF(p.divider));
    FillRect(hdc, &div, db);
    let _ = DeleteObject(HGDIOBJ(db.0));

    for (i, item) in st.items.iter().enumerate() {
        let top = sm_header() + 6 + i as i32 * la_row_h();
        let row = RECT {
            left: la_pad(),
            top,
            right: w - la_pad(),
            bottom: top + la_row_h(),
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
                top + la_row_h() - 3,
                la_sel_radius(),
                la_sel_radius(),
            );
            SelectObject(hdc, ob);
            SelectObject(hdc, op);
            let _ = DeleteObject(HGDIOBJ(sel.0));
            let _ = DeleteObject(HGDIOBJ(pen.0));
            SetTextColor(hdc, COLORREF(p.selfg));
        } else {
            SetTextColor(hdc, COLORREF(p.fg));
        }
        let icon_x = row.left + 10;
        let icon_y = top + (la_row_h() - la_icon_px()) / 2;
        let icon_color = if i == st.sel { p.selfg } else { p.dim };
        let custom = sysmenu_custom_icon(&item.icon);
        if custom > 1 {
            let _ = DrawIconEx(
                hdc,
                icon_x,
                icon_y,
                HICON(custom as *mut c_void),
                la_icon_px(),
                la_icon_px(),
                0,
                None,
                DI_NORMAL,
            );
        } else {
            draw_builtin_icon(hdc, &item.icon, icon_x, icon_y, la_icon_px(), icon_color);
        }
        let mut r = RECT {
            left: row.left + 20 + la_icon_px(),
            ..row
        };
        let mut v: Vec<u16> = item.label.encode_utf16().collect();
        DrawTextW(
            hdc,
            &mut v,
            &mut r,
            DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        // Chevron marks a category (drills into a submenu).
        if matches!(&item.kind, SysKind::Category(_)) {
            let mut cr = RECT {
                left: row.left,
                top,
                right: row.right - 12,
                bottom: top + la_row_h(),
            };
            let mut cv: Vec<u16> = "\u{203a}".encode_utf16().collect();
            DrawTextW(
                hdc,
                &mut cv,
                &mut cr,
                DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_RIGHT,
            );
        }
    }

    let fy = rc.bottom - sm_footer();
    let label = if st.confirm {
        format!(
            "Press Enter again to {}  \u{2022}  Esc cancels",
            st.items[st.sel].label.to_ascii_lowercase()
        )
    } else if st.stack.is_empty() {
        "Up/Down  \u{2022}  Enter open  \u{2022}  Esc close".to_string()
    } else {
        "Up/Down  \u{2022}  Enter run  \u{2022}  \u{2190}/Esc back".to_string()
    };
    SetTextColor(hdc, COLORREF(if st.confirm { p.selbg } else { p.dim }));
    let mut fr = RECT {
        left: la_pad(),
        top: fy,
        right: w - la_pad(),
        bottom: rc.bottom,
    };
    let mut fv: Vec<u16> = label.encode_utf16().collect();
    DrawTextW(
        hdc,
        &mut fv,
        &mut fr,
        DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
    );

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
                    enum Nav {
                        Drill,
                        Confirm,
                        Run(SysAct),
                    }
                    let nav = {
                        let mut st = SYSMENU_STATE.lock().unwrap();
                        let Some(item) = st.items.get(st.sel).cloned() else {
                            return LRESULT(0);
                        };
                        match item.kind {
                            SysKind::Category(sub) => {
                                let old_title = std::mem::replace(&mut st.title, item.label);
                                let old_items = std::mem::replace(&mut st.items, sub);
                                st.stack.push((old_title, old_items));
                                st.sel = 0;
                                st.confirm = false;
                                Nav::Drill
                            }
                            SysKind::Action(act, needs_confirm) => {
                                if needs_confirm && !st.confirm {
                                    st.confirm = true;
                                    Nav::Confirm
                                } else {
                                    Nav::Run(act)
                                }
                            }
                        }
                    };
                    match nav {
                        Nav::Drill => sysmenu_layout(h),
                        Nav::Confirm => {
                            let _ = InvalidateRect(h, None, BOOL(0));
                        }
                        Nav::Run(action) => {
                            sysmenu_close(h);
                            sysmenu_exec(action);
                        }
                    }
                }
                SM_BACK => {
                    enum Back {
                        Repaint,
                        Layout,
                        Close,
                    }
                    let action = {
                        let mut st = SYSMENU_STATE.lock().unwrap();
                        if st.confirm {
                            st.confirm = false;
                            Back::Repaint
                        } else if let Some((title, items)) = st.stack.pop() {
                            st.title = title;
                            st.items = items;
                            st.sel = 0;
                            Back::Layout
                        } else {
                            Back::Close
                        }
                    };
                    match action {
                        Back::Repaint => {
                            let _ = InvalidateRect(h, None, BOOL(0));
                        }
                        Back::Layout => sysmenu_layout(h),
                        Back::Close => sysmenu_close(h),
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
        WM_DPICHANGED => {
            // Same re-entrancy guard as the launcher: sysmenu_layout moves the
            // window, which can deliver this message again from inside it.
            if !POPUP_PLACING.swap(true, Ordering::Relaxed) {
                sysmenu_layout(h);
                POPUP_PLACING.store(false, Ordering::Relaxed);
            }
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
            sm_w(),
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
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
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
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent() else { return };
    let path = dir.join("astur-settings.exe");
    let Err(error) = std::process::Command::new(&path).spawn() else {
        return;
    };
    let message = format!(
        "Could not open settings GUI.\r\n\r\nExpected:\r\n{}\r\n\r\n{}\r\n\r\nSource build:\r\ncargo build --release",
        path.display(),
        error
    );
    let text: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    let _ = MessageBoxW(
        None,
        PCWSTR(text.as_ptr()),
        w!("Astur settings"),
        MB_OK | MB_ICONERROR,
    );
}

/// `tray_open_settings` on its own thread. The tray window lives on the main
/// thread, which pumps the low-level hooks: process creation there (image
/// load, AV scan; an estimated 5-50 ms, not measured) stalled every mouse and
/// key event for its duration. The error MessageBox runs its own modal loop,
/// so it is fine on the worker.
unsafe fn tray_open_settings_async() {
    let spawned = std::thread::Builder::new()
        .name("tray-settings".to_string())
        .spawn(|| unsafe { tray_open_settings() });
    if spawned.is_err() {
        tray_open_settings();
    }
}

unsafe extern "system" fn tray_wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_TRAY {
        // Classic NOTIFYICON callback: lParam low word = the mouse message.
        let event = (l.0 as u32) & 0xFFFF;
        if event == WM_LBUTTONUP || event == WM_LBUTTONDBLCLK {
            tray_open_settings_async();
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
                let cmd = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_RIGHTBUTTON,
                    pt.x,
                    pt.y,
                    0,
                    h,
                    None,
                );
                let _ = DestroyMenu(menu);
                match cmd.0 as usize {
                    TRAY_SETTINGS => tray_open_settings_async(),
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

// =========================================================================
// Command line
// =========================================================================
// Astur is a GUI-subsystem process, so these answer through the console that
// launched them (when there is one) and, for `--check`, a file as well.

const CLI_HELP: &str = r"Astur - tiling window manager for Windows 10/11

Usage: astur.exe [option]

  (no option)          run the window manager
  --check              print a diagnostics report (version, DPI awareness,
                       monitors + their DPI, config paths, log path) and
                       save it next to the config
  --version            print the version
  --help               print this
  --wait-for-pid <pid> wait for that process to exit, then run (used by the
                       tray's Restart so two instances never overlap)

Config: %USERPROFILE%\.astur\astur.conf and navbar.conf
Log:    %USERPROFILE%\.astur\astur.log (set log_level in astur.conf)
";

enum CliAction {
    Run,
    Check,
    Version,
    Help,
    WaitForPid(u32),
}

fn parse_args() -> CliAction {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--check" | "-check" | "/check") => CliAction::Check,
        Some("--version" | "-V") => CliAction::Version,
        Some("--help" | "-h" | "-?" | "/?") => CliAction::Help,
        Some("--wait-for-pid") => match args.next().and_then(|v| v.parse().ok()) {
            Some(pid) => CliAction::WaitForPid(pid),
            None => CliAction::Run,
        },
        _ => CliAction::Run,
    }
}

unsafe fn message_box(message: &str) {
    let text: Vec<u16> = message
        .replace('\n', "\r\n")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let _ = MessageBoxW(
        None,
        PCWSTR(text.as_ptr()),
        w!("Astur"),
        MB_OK | MB_ICONERROR,
    );
}

// =========================================================================
// Single-instance guard
// =========================================================================
// Astur ships as a portable exe with no installer, so double-launching it is a
// normal accident — and two managers is not a degraded experience, it is a
// broken one: two LL hook chains (every Cmd pushed twice, Alt suppressed
// twice), two sets of per-monitor bars stacked on each other, two managers
// issuing conflicting SetWindowPos/SW_HIDE for the same HWNDs, and two crash-
// rescue files racing on the same path.
//
// A named mutex in the Local\ namespace scopes this per user session, which is
// what we want: two people fast-user-switched on one machine each get their
// own Astur.

const INSTANCE_MUTEX: PCWSTR = w!(r"Local\astur.instance");

/// Held for the process lifetime by the owning instance. Never released
/// explicitly — the kernel drops it when the process exits, including on a
/// crash or a kill, which is exactly the behaviour we want.
static INSTANCE_LOCK: AtomicIsize = AtomicIsize::new(0);

/// Take the single-instance lock. `false` = another Astur already owns it.
unsafe fn claim_single_instance() -> bool {
    let Ok(handle) = CreateMutexW(None, true, INSTANCE_MUTEX) else {
        return true; // cannot create the mutex: do not block the user's WM
    };
    // CreateMutexW succeeds either way; ERROR_ALREADY_EXISTS is how it says
    // somebody else owns it.
    if windows::Win32::Foundation::GetLastError()
        == windows::Win32::Foundation::ERROR_ALREADY_EXISTS
    {
        let _ = CloseHandle(handle);
        return false;
    }
    INSTANCE_LOCK.store(handle.0 as isize, Ordering::Relaxed);
    true
}

/// Probe without taking it (used by `--check`).
unsafe fn instance_already_running() -> bool {
    // If WE hold it, the answer is no. Without this the running WM's own
    // startup report says "a second Astur is running" and points at itself —
    // a false alarm in exactly the diagnostic people would paste into a bug
    // report.
    if INSTANCE_LOCK.load(Ordering::Relaxed) != 0 {
        return false;
    }
    match OpenMutexW(
        SYNCHRONIZATION_ACCESS_RIGHTS(PROCESS_SYNCHRONIZE.0),
        false,
        INSTANCE_MUTEX,
    ) {
        Ok(h) => {
            let _ = CloseHandle(h);
            true
        }
        Err(_) => false,
    }
}

/// Restart hand-off: the replacement waits for the old process to exit before
/// claiming the instance lock, so the two never overlap. Bounded so a wedged
/// predecessor cannot stop the restart entirely.
unsafe fn wait_for_predecessor(pid: u32) {
    let Ok(handle) = OpenProcess(
        PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
        false,
        pid,
    ) else {
        return; // already gone
    };
    let _ = WaitForSingleObject(handle, 10_000);
    let _ = CloseHandle(handle);
}

// =========================================================================
// Diagnostics report  (`astur.exe --check`, and the startup log line)
// =========================================================================
// The answer to "how would we find out this is broken, if nobody told us?" for
// the whole DPI/monitor surface: one paste-ready dump the reporter can attach
// instead of a video.

unsafe extern "system" fn diag_mon_enum(
    hmon: HMONITOR,
    _hdc: HDC,
    _rc: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let v = &mut *(lparam.0 as *mut Vec<(isize, RECT, RECT, bool, u32)>);
    let mut mi = MONITORINFO {
        cbSize: core::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(hmon, &mut mi).as_bool() {
        v.push((
            hmon.0 as isize,
            mi.rcMonitor,
            mi.rcWork,
            mi.dwFlags & 1 != 0, // MONITORINFOF_PRIMARY
            monitor_dpi(hmon.0 as isize),
        ));
    }
    BOOL(1)
}

/// One line per monitor: handle, full rect, work area, DPI and scale.
unsafe fn diag_monitors() -> Vec<String> {
    let mut raw: Vec<(isize, RECT, RECT, bool, u32)> = Vec::new();
    let _ = EnumDisplayMonitors(
        None,
        None,
        Some(diag_mon_enum),
        LPARAM(&mut raw as *mut _ as isize),
    );
    raw.sort_by_key(|m| m.1.left);
    raw.iter()
        .enumerate()
        .map(|(i, (hmon, rc, wa, primary, dpi))| {
            format!(
                "  [{i}] hmon=0x{hmon:x}{} rect={},{} {}x{} work={},{} {}x{} dpi={dpi} ({}%)",
                if *primary { " PRIMARY" } else { "" },
                rc.left,
                rc.top,
                rc.right - rc.left,
                rc.bottom - rc.top,
                wa.left,
                wa.top,
                wa.right - wa.left,
                wa.bottom - wa.top,
                dpi * 100 / DPI_BASE,
            )
        })
        .collect()
}

/// Windows build string, straight out of the registry (no deprecated
/// GetVersionEx shimming).
unsafe fn windows_build() -> String {
    let key = w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let read_sz = |name: PCWSTR| -> Option<String> {
        let mut buf = [0u16; 128];
        let mut cb = (buf.len() * 2) as u32;
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            name,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut c_void),
            Some(&mut cb),
        )
        .is_ok()
        .then(|| {
            let n = (cb as usize / 2).saturating_sub(1);
            String::from_utf16_lossy(&buf[..n])
        })
    };
    let read_dword = |name: PCWSTR| -> Option<u32> {
        let mut v = 0u32;
        let mut cb = 4u32;
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            name,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut c_void),
            Some(&mut cb),
        )
        .is_ok()
        .then_some(v)
    };
    let build: u32 = read_sz(w!("CurrentBuild"))
        .and_then(|b| b.parse().ok())
        .unwrap_or(0);
    // The registry still says "Windows 10 Pro" on Windows 11; the build number
    // is the only honest discriminator (11 starts at 22000).
    let name = read_sz(w!("ProductName")).unwrap_or_else(|| "Windows".into());
    let name = if build >= 22000 {
        name.replace("Windows 10", "Windows 11")
    } else {
        name
    };
    match read_dword(w!("UBR")) {
        Some(ubr) => format!("{name} build {build}.{ubr}"),
        None => format!("{name} build {build}"),
    }
}

/// The report shared by `--check` and (at `info`) the startup log.
/// `live` = produced inside the running WM, so its counters mean something; a
/// `--check` process has its own, all zero, and prints none.
unsafe fn diagnostics_report(dpi_aware: bool, live: bool) -> String {
    let mut out = String::new();
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "?".into());
    out.push_str(&format!("Astur {}\n", env!("CARGO_PKG_VERSION")));
    out.push_str(&format!("  exe            : {exe}\n"));
    out.push_str(&format!("  os             : {}\n", windows_build()));
    out.push_str(&format!(
        "  dpi awareness  : {}\n",
        if dpi_aware {
            "per-monitor-v2"
        } else {
            "FAILED TO SET (tiling will be wrong on scaled displays)"
        }
    ));
    out.push_str(&format!(
        "  system dpi     : {}\n",
        dpi_at(POINT { x: 0, y: 0 })
    ));
    out.push_str("  monitors       :\n");
    for line in diag_monitors() {
        out.push_str(&line);
        out.push('\n');
    }
    for (env, name) in [
        ("ASTUR_CONFIG", "astur.conf"),
        ("ASTUR_NAVBAR", "navbar.conf"),
    ] {
        let path = config_path(env, name);
        let meta = std::fs::metadata(&path);
        out.push_str(&format!(
            "  {name:<14} : {} ({})\n",
            path.display(),
            match meta {
                Ok(m) => format!("{} bytes", m.len()),
                Err(_) => "missing — defaults in use".to_string(),
            }
        ));
    }
    out.push_str(&format!(
        "  log            : {} (log_level = {})\n",
        log_path().display(),
        log_level_name(LOG_LEVEL.load(Ordering::Relaxed)),
    ));
    match load_config() {
        Err(e) => {
            out.push_str(&format!(
                "  config keys    : UNREADABLE, file left untouched: {e}\n"
            ));
        }
        Ok(cfg) if cfg.unknown_keys.is_empty() => {
            out.push_str("  config keys    : all understood\n");
        }
        Ok(cfg) => {
            out.push_str(&format!(
                "  config keys    : {} NOT understood (ignored):\n",
                cfg.unknown_keys.len()
            ));
            for key in &cfg.unknown_keys {
                out.push_str(&format!("      {key}\n"));
            }
        }
    }
    out.push_str(&format!(
        "  hook re-arms   : {}\n",
        HOOK_REARMS.load(Ordering::Relaxed)
    ));
    if live {
        out.push_str(&format!(
            "  placement      : {}\n",
            if ASYNC_WINDOW_POS.load(Ordering::Relaxed) {
                "posted (async_window_pos = true)"
            } else {
                "synchronous (async_window_pos = false)"
            }
        ));
        out.push_str(&format!("  counters       : {}\n", counters_line()));
    }
    out.push_str(&format!(
        "  other instance : {}\n",
        if instance_already_running() {
            "YES — a second Astur is running; they will fight over your windows"
        } else {
            "no"
        }
    ));
    out
}

/// Log the environment once at startup. This is the line that turns "it looks
/// wrong on my laptop" into a diagnosable report.
unsafe fn log_startup_environment(dpi_aware: bool) {
    if !dpi_aware {
        log_error!("SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2) failed; tiling will be wrong on scaled displays");
    }
    if !log_on(LOG_INFO) {
        return;
    }
    for line in diagnostics_report(dpi_aware, true).lines() {
        log_info!("{}", line.trim_end());
    }
}

/// Write `--check` output to the parent console when there is one, and always
/// to a file, so a GUI-subsystem process can still be asked what it sees.
unsafe fn run_check() -> i32 {
    let report = diagnostics_report(true, false);
    let path = config_path("ASTUR_CHECK", "astur-check.txt");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let saved = std::fs::write(&path, report.replace('\n', "\r\n")).is_ok();
    console_write(&report);
    if saved {
        console_write(&format!("\nSaved to {}\n", path.display()));
    }
    0
}

/// Write to stdout without `println!`. Release builds are the "windows"
/// subsystem, so stdout may not exist at all; `println!` panics in that case
/// and this must not.
unsafe fn console_write(text: &str) {
    let handle = match GetStdHandle(STD_OUTPUT_HANDLE) {
        Ok(h) if !h.is_invalid() && !h.0.is_null() => h,
        _ => return,
    };
    let bytes = text.as_bytes();
    let mut written = 0u32;
    let _ = WriteFile(handle, Some(bytes), Some(&mut written), None);
}

/// Attach to the console that launched us, if any, so `--check` / `--version`
/// can answer in the terminal the user typed them into. No AllocConsole: a
/// double-clicked exe should not flash a window that vanishes on exit.
unsafe fn attach_parent_console() {
    let already =
        matches!(GetStdHandle(STD_OUTPUT_HANDLE), Ok(h) if !h.is_invalid() && !h.0.is_null());
    if already {
        return; // redirected to a file or pipe: leave it alone
    }
    let _ = AttachConsole(ATTACH_PARENT_PROCESS);
}

// --- foreground lock (system-wide setting; saved and restored) --------------

/// Previous SPI_SETFOREGROUNDLOCKTIMEOUT value, +1 so that 0 means "we never
/// changed it" and the restore path is a no-op on every other exit route.
static FOREGROUND_LOCK_PREV: AtomicU32 = AtomicU32::new(0);

unsafe fn disable_foreground_lock() {
    let mut prev: u32 = 0;
    let read = SystemParametersInfoW(
        SPI_GETFOREGROUNDLOCKTIMEOUT,
        0,
        Some(&mut prev as *mut u32 as *mut c_void),
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    )
    .is_ok();
    if read && prev == 0 {
        return; // already off; nothing to change and nothing to restore
    }
    // pvParam carries the new timeout BY VALUE (a UINT in a PVOID), and the
    // call wants SPIF_SENDCHANGE — without it Windows can report failure even
    // though the docs read as if uiParam alone were enough. The old code passed
    // a null pvParam with no flags and had its result discarded, so this had
    // most likely never worked; nobody could tell, which is the point of the
    // logging work.
    let ok = SystemParametersInfoW(
        SPI_SETFOREGROUNDLOCKTIMEOUT,
        0,
        Some(core::ptr::null_mut()),
        SPIF_SENDCHANGE,
    )
    .is_ok();
    if !ok {
        // Advisory, not fatal: focus changes still work, they can occasionally
        // flash in the taskbar instead of taking. Not an ERROR.
        log_info!("foreground lock timeout unchanged (SystemParametersInfoW refused it)");
        return;
    }
    if read {
        FOREGROUND_LOCK_PREV.store(prev.saturating_add(1), Ordering::Relaxed);
        log_info!("foreground lock timeout {prev} -> 0 (restored on exit)");
    }
}

/// Put the system setting back. Safe to call more than once and from any exit
/// path; a hard kill obviously cannot run it, which is why the value is only
/// ever set to what the user already had.
unsafe fn restore_foreground_lock() {
    let saved = FOREGROUND_LOCK_PREV.swap(0, Ordering::Relaxed);
    if saved == 0 {
        return;
    }
    let value = saved - 1;
    // By value, like the disable path: pvParam IS the timeout. This used to pass
    // a pointer to `value`, which set the timeout to the low bits of a stack
    // address on every graceful exit instead of the user's original value.
    let _ = SystemParametersInfoW(
        SPI_SETFOREGROUNDLOCKTIMEOUT,
        0,
        Some(value as usize as *mut c_void),
        SPIF_SENDCHANGE,
    );
    let mut now: u32 = 0;
    let read = SystemParametersInfoW(
        SPI_GETFOREGROUNDLOCKTIMEOUT,
        0,
        Some(&mut now as *mut u32 as *mut c_void),
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    )
    .is_ok();
    if read && now != value {
        log_error!("foreground lock timeout restore failed: wanted {value}, system has {now}");
    }
}

fn main() {
    // Reveal every managed window if any thread panics. `panic = "abort"` skips
    // destructors and a process kill skips the console handler, so without this a
    // window hidden on an inactive workspace would be left invisible. The hook
    // runs before the abort.
    std::panic::set_hook(Box::new(|info| {
        restore_on_panic();
        // Written synchronously: `panic = "abort"` gives the log worker no
        // chance to drain its queue, and a panic is the one event that must
        // never be lost.
        log_sync(&format!("PANIC {info}"));
    }));
    unsafe {
        // MUST be the first Win32 call: it has to happen before any window, DC
        // or monitor query, and it cannot be changed afterwards. From here on
        // GetMonitorInfoW returns physical pixels and SetWindowPos takes them,
        // on every monitor at every scale. Without it Windows virtualises the
        // desktop to 96 DPI and tiles land in the top-left 1/scale of a scaled
        // screen (GitHub #5) — 80% at 125%, 66% at 150%.
        let dpi_aware = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        // Command line, before anything is created. `--check`/`--version`/
        // `--help` answer and exit; `--wait-for-pid` is the restart hand-off.
        match parse_args() {
            CliAction::Check => {
                attach_parent_console();
                // An unreadable file is reported by the check itself.
                let cfg = load_config().unwrap_or_else(|_| config::default_config());
                LOG_LEVEL.store(log_level_from_str(&cfg.log_level), Ordering::Relaxed);
                std::process::exit(run_check());
            }
            CliAction::Version => {
                attach_parent_console();
                console_write(&format!("Astur {}\n", env!("CARGO_PKG_VERSION")));
                std::process::exit(0);
            }
            CliAction::Help => {
                attach_parent_console();
                console_write(CLI_HELP);
                std::process::exit(0);
            }
            CliAction::WaitForPid(pid) => wait_for_predecessor(pid),
            CliAction::Run => {}
        }

        // One manager per session. Two would fight over the same windows.
        if !claim_single_instance() {
            attach_parent_console();
            console_write("Astur is already running.\n");
            message_box(
                "Astur is already running.\n\nUse the tray icon to open Settings or quit it.",
            );
            std::process::exit(0);
        }

        // 1ms timer resolution so the animation worker's frame sleeps are precise
        // (the default ~15.6ms granularity is the main cause of choppy motion).
        let _ = windows::Win32::Media::timeBeginPeriod(1);

        let hmod = GetModuleHandleW(None).expect("GetModuleHandleW failed");
        let hinst = HINSTANCE(hmod.0);

        // Load config once here so the bars (main thread) and the manager thread
        // share the exact same settings.
        let (cfg, cfg_err) = load_config_for_startup();
        let cfg_unread = cfg_err.is_some();
        apply_hook_config(&cfg); // also applies log_level, so log after this
        if let Some(e) = cfg_err {
            log_error!("running on built-in defaults, config file left untouched: {e}");
        }
        log_startup_environment(dpi_aware.is_ok());
        if cfg.persist_state {
            load_launcher_mru();
        }
        BAR_HINST.store(hinst.0 as isize, Ordering::Relaxed);
        apply_bar_statics(&cfg);
        apply_theme(&cfg);
        // Seed the bar snapshot from the config before any bar exists (BAR-23):
        // the bars used to paint BarData::new()'s hard-coded dark defaults with
        // no widgets until the manager had adopted, retiled and styled every
        // window, a dark flash for light-theme users. Here, before the manager
        // thread is spawned, so it can never overwrite the manager's first
        // snapshot. `mons` stays empty: seeded pill slots or an active index
        // that the first real update then changes would start a spurious pill
        // slide or flash a label.
        *BAR.lock().unwrap() = bar_data_from(
            &cfg,
            THEME_LIGHT.load(Ordering::Relaxed),
            cfg.start_tiled,
            Vec::new(),
        );

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
        // Explorer broadcasts "TaskbarCreated" when it (re)starts; the marker
        // resets the wallpaper source cache on it. It is a registered message
        // above WM_USER, so UIPI drops it from medium-IL Explorer to an
        // elevated Astur unless explicitly allowed (ChangeWindowMessageFilter
        // remarks). Allowed only when elevated: that is the only case needing it.
        let taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));
        TASKBAR_CREATED_MSG.store(taskbar_created, Ordering::Relaxed);
        if taskbar_created != 0 && process_elevated() {
            let _ = ChangeWindowMessageFilterEx(marker, taskbar_created, MSGFLT_ALLOW, None);
        }

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

        // Seed per-monitor fullscreen state before first bar placement so apps
        // already maximized/fullscreen when Astur starts never get covered.
        seed_fullscreen_windows();

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
            // Fonts are built lazily per monitor DPI on first paint.
            ensure_bars();
        }

        // This thread runs the LL hooks and WinEvents from here on: raise it
        // before they exist (see the thread scheduling notes).
        raise_current_thread(ThreadRole::Main);
        // Without these Astur is inert, but `panic = "abort"` would turn an
        // .expect() here into a silent process death with no window and no
        // message (review W-03). Say why, then leave cleanly.
        if !install_hooks(hinst) {
            log_error!("SetWindowsHookExW failed; cannot run");
            message_box(
                "Astur could not install its keyboard/mouse hooks.

                 Another program may be blocking them, or Astur may need to be 
                 run at the same privilege level as the apps you want to manage.",
            );
            restore_all_windows();
            std::process::exit(1);
        }

        // Reveal all managed windows on Ctrl+C / console close so none are left
        // hidden on another workspace when Astur exits.
        let _ = SetConsoleCtrlHandler(Some(console_handler), BOOL(1));

        // Reduce the foreground lock so the manager can focus windows reliably.
        // This is SYSTEM-WIDE, affecting every application, so remember the old
        // value and put it back on a graceful exit (review S-03) — leaving the
        // machine in a changed state after quitting is not ours to do.
        if cfg.foreground_lock_disable {
            disable_foreground_lock();
        }

        // React to windows opening/closing/focusing for tiling. Out-of-context
        // callbacks run on this thread's message loop; own-process events
        // skipped. One hook per range in WINEVENT_RANGES, each checked: every
        // result used to be dropped, so a failed one failed silently.
        for (i, &(min, max, name)) in WINEVENT_RANGES.iter().enumerate() {
            let hook = SetWinEventHook(
                min,
                max,
                None,
                Some(win_event_proc),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            );
            if hook.0.is_null() {
                WINEVENT_HOOKS_FAILED.fetch_or(1 << i, Ordering::Relaxed);
                log_error!("SetWinEventHook({name}) failed; those window events are not seen");
            }
        }
        if WINEVENT_HOOKS_FAILED.load(Ordering::Relaxed) & 1 != 0 {
            // Without DESTROY..HIDE Astur neither adopts shown windows nor
            // untracks closed ones, and nothing on screen would say why.
            message_box(
                "Astur could not subscribe to window show / hide / close events.\n\n\
                 New windows will not be tiled and closed windows will leave gaps \
                 until Astur is restarted.",
            );
        }

        // System tray icon — the control surface for Astur Full (no console in
        // release): left/double-click opens Settings, right-click menu = Settings/Quit.
        let _tray = setup_tray(hinst);

        // Focus-follows-mouse poll loop (no-op unless enabled in config).
        spawn_named("focus-follow", focus_follow_worker);
        // CPU/RAM/battery poll loop (idles unless a stats widget is enabled).
        spawn_named("stats", stats_worker);
        // Wallpaper/state writes can involve disk/shell I/O; keep them off manager/hooks.
        spawn_named("wallpaper", wallpaper_worker);
        spawn_named("state", state_worker);
        spawn_named("mru", mru_worker);
        // Workspace-slide compositor (owns its overlay + message pump; idle on a
        // condvar until the manager dispatches a slide).
        spawn_named("transition", transition_worker);
        // Per-window glide compositor (move/open/close/re-tile). Own overlay +
        // pump; idle on a condvar until the manager dispatches a glide.
        spawn_named("glide", glide_worker);
        // Wallpaper renders for both compositors (the only PrintWindow caller;
        // see the wallpaper cache notes). Normal priority, idle on a condvar.
        spawn_named("wallpaper-capture", wallpaper_capture_worker);
        // App launcher (Alt+Space): owns its picker window + message pump, idle
        // until the keyboard hook posts an open/key message.
        spawn_named("launcher", launcher_thread);
        // Resolve launcher app icons to HBITMAPs off the UI thread, in parallel so
        // the whole list is iconned fast (each worker is a COM STA; they idle on a
        // condvar once the queue drains). Count is a speed/RAM trade — see
        // plan/optimization.md. RAM is no longer the constraint, so one per core
        // from 3 up to 8: preload and new-DPI bursts finish sooner, unless the
        // shell serialises extraction internally (not measured).
        let icon_workers = std::thread::available_parallelism()
            .map_or(3, |n| n.get())
            .clamp(3, 8);
        for i in 0..icon_workers {
            spawn_named(&format!("icon-{i}"), icon_worker);
        }
        // File search against the Windows Search index (debounced, own COM STA
        // each). Two, so the newest query never waits on a superseded one.
        for i in 0..2 {
            spawn_named(&format!("filesearch-{i}"), filesearch_worker);
        }
        // System / power menu (Alt+Shift+Space): owns its popup + message pump.
        spawn_named("sysmenu", sysmenu_thread);
        // Hot-reload config files on save.
        spawn_named("config-watcher", move || config_watcher(cfg_unread));
        // Put the input hooks back if Windows silently drops them.
        spawn_named("hook-watchdog", hook_watchdog);
        // Optional local-only named-pipe command API; blocks on its own worker.
        spawn_named("ipc", ipc_worker);
        // Crash rescue: un-hide anything a previous (killed) instance left hidden
        // BEFORE the manager adopts windows, so they're adopted visible.
        rescue_orphans();
        // Owns all tiling/workspace state; hooks only enqueue commands to it.
        spawn_named("manager", move || manager_loop(cfg));

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

        for slot in [&MOUSE_HOOK_H, &KBD_HOOK_H] {
            let h = slot.swap(0, Ordering::Relaxed);
            if h != 0 {
                let _ = UnhookWindowsHookEx(HHOOK(h as *mut c_void));
            }
        }
        let _ = windows::Win32::Media::timeEndPeriod(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_delay_keeps_the_worst_and_ignores_future_stamps() {
        // The only test that touches HOOK_DELAY_MAX.
        HOOK_DELAY_MAX.store(0, Ordering::Relaxed);
        let now = unsafe { GetTickCount() };
        hook_delay_note(now.wrapping_sub(40));
        hook_delay_note(now.wrapping_sub(20));
        let worst = HOOK_DELAY_MAX.load(Ordering::Relaxed);
        assert!((40..1_000).contains(&worst), "worst = {worst}");
        // A stamp ahead of our tick read must not wrap to ~49 days.
        hook_delay_note(now.wrapping_add(10_000));
        assert_eq!(HOOK_DELAY_MAX.load(Ordering::Relaxed), worst);
    }

    #[test]
    fn frame_stats_are_upper_median_and_max() {
        assert_eq!(p50_max(&mut []), (0, 0));
        assert_eq!(p50_max(&mut [7]), (7, 7));
        assert_eq!(p50_max(&mut [9, 1, 5]), (5, 9));
        // Even count: the upper of the two middle values.
        assert_eq!(p50_max(&mut [4, 1, 3, 2]), (3, 4));
    }

    #[test]
    fn show_event_adds_untracked_windows_even_mid_retile() {
        // An app opening a window while the manager holds SUPPRESS for a
        // retile must still be adopted (the 1-of-4 burst bug).
        assert!(show_needs_add(true, false));
        assert!(show_needs_add(false, false));
        // Tracked + not ours: Add runs the follow-to-workspace logic.
        assert!(show_needs_add(false, true));
        // Tracked + SUPPRESS: echo of our own show, skip.
        assert!(!show_needs_add(true, true));
    }

    // ---- drag drops (batch 17, INPUT-9 / INPUT-5) -----------------------------

    #[test]
    fn a_resize_drop_skips_its_commit_only_when_tiled_and_instant() {
        use ResizeDrop::*;
        // Untiled windows, and any glide, commit the preview as before.
        assert_eq!(resize_drop_plan(false, false, false), Commit);
        assert_eq!(resize_drop_plan(false, true, false), Commit);
        assert_eq!(resize_drop_plan(true, true, true), Commit);
        assert_eq!(resize_drop_plan(true, false, true), Commit);
        // Tiled + instant: un-park if parked, else straight to the retile.
        assert_eq!(resize_drop_plan(true, true, false), UnparkOrigin);
        assert_eq!(resize_drop_plan(true, false, false), NoUnpark);
    }

    #[test]
    fn placement_rects_shift_by_the_work_area_inset_not_its_origin() {
        let r = RECT {
            left: 2100,
            top: 60,
            right: 2740,
            bottom: 540,
        };
        // A secondary monitor with no toolbar: workspace == screen.
        assert_eq!(placement_to_workspace(r, (0, 0), false), r);
        // A top taskbar 48 px tall on that monitor.
        let w = placement_to_workspace(r, (0, 48), false);
        assert_eq!((w.left, w.top, w.right, w.bottom), (2100, 12, 2740, 492));
        // Tool windows use screen coordinates.
        assert_eq!(placement_to_workspace(r, (0, 48), true), r);
    }

    // ---- thread scheduling (batch 11, EVENTS-12) ------------------------------

    #[test]
    fn raised_threads_never_go_below_normal_and_the_manager_leads_the_compositors() {
        use windows::Win32::System::Threading::{
            THREAD_PRIORITY_NORMAL, THREAD_PRIORITY_TIME_CRITICAL,
        };
        let roles = [
            ThreadRole::Main,
            ThreadRole::Manager,
            ThreadRole::Compositor,
            ThreadRole::Launcher,
        ];
        for r in roles {
            let p = role_priority(r).0;
            assert!(p >= THREAD_PRIORITY_NORMAL.0, "{r:?} below normal");
            assert!(p < THREAD_PRIORITY_TIME_CRITICAL.0, "{r:?} time-critical");
        }
        assert!(role_priority(ThreadRole::Manager).0 >= role_priority(ThreadRole::Compositor).0);
        assert!(role_priority(ThreadRole::Main).0 <= THREAD_PRIORITY_HIGHEST.0);
    }

    // ---- posted placement (batch 10, TILE-1) ----------------------------------

    #[test]
    fn foreign_swp_flags_adds_only_the_async_bit_and_only_when_on() {
        let base = SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSENDCHANGING;
        assert_eq!(foreign_swp_flags(base, false), base);
        assert_eq!(foreign_swp_flags(base, true), base | SWP_ASYNCWINDOWPOS);
        let park = SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSENDCHANGING;
        assert_eq!(
            foreign_swp_flags(park, true).0 & !SWP_ASYNCWINDOWPOS.0,
            park.0
        );
    }

    #[test]
    fn border_insets_outside_a_plausible_frame_are_rejected() {
        assert_eq!(bounded_insets(7, 0, 7, 7, 16), Some((7, 0, 7, 7)));
        assert_eq!(bounded_insets(0, 0, 0, 0, 16), Some((0, 0, 0, 0)));
        assert_eq!(bounded_insets(16, 16, 16, 16, 16), Some((16, 16, 16, 16)));
        // A pair read across a landing: the "inset" is the move distance.
        assert_eq!(bounded_insets(412, 7, -405, 7, 16), None);
        assert_eq!(bounded_insets(7, -1, 7, 7, 16), None);
        assert_eq!(bounded_insets(7, 0, 17, 7, 16), None);
    }

    #[test]
    fn a_drop_commit_counts_as_landed_only_off_the_park() {
        let rc = |l, t, r, b| RECT {
            left: l,
            top: t,
            right: r,
            bottom: b,
        };
        let parked = rc(-32000, -32000, -31360, -31520);
        let drop = rc(100, 100, 740, 580);
        // Parked during the drag, commit not processed yet.
        assert!(!swp_landed(parked, parked, drop));
        // Landed exactly, or where the app rounded it to.
        assert!(swp_landed(parked, drop, drop));
        assert!(swp_landed(parked, rc(100, 100, 740, 582), drop));
        // Outline drag (never parked), dropped where it already was.
        assert!(swp_landed(drop, drop, drop));
        // The park itself was still queued at the drop and has just landed:
        // that is not the commit.
        let orig = rc(0, 0, 640, 480);
        assert!(!swp_landed(orig, parked, drop));
        assert!(!swp_landed(orig, orig, drop));
    }

    #[test]
    fn a_drop_glides_only_when_wanted_and_landed() {
        assert_eq!(drop_glide_plan(true, true), DropRetile::Glide);
        assert_eq!(drop_glide_plan(true, false), DropRetile::Instant);
        assert_eq!(drop_glide_plan(false, true), DropRetile::Instant);
        assert_eq!(drop_glide_plan(false, false), DropRetile::Instant);
        assert!(rect_parked(&RECT {
            left: -32000,
            top: -32000,
            right: 0,
            bottom: 0
        }));
        assert!(!rect_parked(&RECT {
            left: -3840,
            top: 0,
            right: 0,
            bottom: 2160
        }));
    }

    // ---- event intake (batch 8) ---------------------------------------------

    #[test]
    fn a_minimize_retiles_only_its_own_tiled_monitor() {
        let active = |mi: usize| [0usize, 2][mi];
        let never = |_: usize, _: usize| false;
        assert_eq!(retile_for_target(Some((1, 2)), active, never), Some(1));
        assert_eq!(retile_for_target(Some((0, 0)), active, never), Some(0));
        // Untracked, on a hidden workspace, or floating: nothing to re-tile.
        assert_eq!(retile_for_target(None, active, never), None);
        assert_eq!(retile_for_target(Some((1, 0)), active, never), None);
        assert_eq!(retile_for_target(Some((1, 2)), active, |_, _| true), None);
    }

    #[test]
    fn the_show_prefilter_rejects_exactly_the_app_surface_style_bits() {
        let (child, tool, noact) = (WS_CHILD.0, WS_EX_TOOLWINDOW.0, WS_EX_NOACTIVATE.0);
        assert!(show_rejected_by_style(child, 0));
        assert!(show_rejected_by_style(0, tool));
        assert!(show_rejected_by_style(0, noact));
        // An ordinary app window: overlapped, with the usual extended bits.
        let overlapped = 0x00CF_0000; // WS_OVERLAPPEDWINDOW
        let ex_app = 0x0000_0100 | 0x0000_0200 | WS_EX_LAYERED.0; // WINDOWEDGE | CLIENTEDGE
        assert!(!show_rejected_by_style(overlapped, ex_app));
        assert!(!show_rejected_by_style(
            0,
            WS_EX_TOPMOST.0 | WS_EX_TRANSPARENT.0
        ));
    }

    #[test]
    fn winevent_ranges_cover_show_once_and_never_create_or_reorder() {
        let covers = |e: u32| {
            WINEVENT_RANGES
                .iter()
                .filter(|&&(lo, hi, _)| (lo..=hi).contains(&e))
                .count()
        };
        assert_eq!(covers(EVENT_OBJECT_SHOW), 1);
        assert_eq!(covers(0x8000), 0, "EVENT_OBJECT_CREATE");
        assert_eq!(covers(0x8004), 0, "EVENT_OBJECT_REORDER");
        for e in [
            EVENT_OBJECT_DESTROY,
            EVENT_OBJECT_HIDE,
            EVENT_OBJECT_LOCATIONCHANGE,
            EVENT_OBJECT_NAMECHANGE,
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_MINIMIZESTART,
            EVENT_SYSTEM_MINIMIZEEND,
            EVENT_SYSTEM_MOVESIZEEND,
        ] {
            assert_eq!(covers(e), 1, "event {e:#x}");
        }
        // The failure mask has a bit per range.
        assert!(WINEVENT_RANGES.len() <= 32);
    }

    #[test]
    fn at_most_one_bar_refresh_is_queued_until_the_manager_takes_it() {
        let q = AtomicBool::new(false);
        assert!(bar_refresh_gate(&q));
        assert!(!bar_refresh_gate(&q));
        assert!(!bar_refresh_gate(&q));
        bar_refresh_clear(&q);
        assert!(bar_refresh_gate(&q), "a clear re-enables the next push");
        assert!(!bar_refresh_gate(&q));
    }

    // ---- switch commit ------------------------------------------------------

    #[test]
    fn the_foreground_is_hidden_last_and_only_when_it_is_outgoing() {
        let order = |ws: &[isize], fg: isize| hide_order(ws, fg).collect::<Vec<_>>();
        assert_eq!(order(&[1, 2, 3], 1), vec![2, 3, 1]);
        assert_eq!(order(&[1, 2, 3], 2), vec![1, 3, 2]);
        assert_eq!(order(&[1, 2, 3], 3), vec![1, 2, 3]);
        // Foreground elsewhere (another monitor, an owned dialog, nothing):
        // list order, nothing dropped or added.
        assert_eq!(order(&[1, 2, 3], 9), vec![1, 2, 3]);
        assert_eq!(order(&[1, 2, 3], 0), vec![1, 2, 3]);
        assert!(order(&[], 1).is_empty());
    }

    #[test]
    fn a_focus_event_is_followed_only_while_it_owns_the_foreground() {
        assert!(focused_follow_allowed(0xA, 0xA));
        // Stale: the foreground moved on (to the switch's own target).
        assert!(!focused_follow_allowed(0xB, 0xA));
        // No foreground at all is not ownership.
        assert!(!focused_follow_allowed(0, 0xA));
        assert!(!focused_follow_allowed(0, 0));
    }

    #[test]
    fn alpha_is_resent_unless_the_window_already_has_it() {
        let alpha = LWA_ALPHA.0;
        assert!(!alpha_set_needed(true, alpha, 204, 204));
        assert!(alpha_set_needed(true, alpha, 255, 204));
        // Failed read (never set through SLWA): set it.
        assert!(alpha_set_needed(false, alpha, 204, 204));
        // Colour key or no flags: not ours, set it.
        assert!(alpha_set_needed(true, 0, 204, 204));
        assert!(alpha_set_needed(true, alpha | 1, 204, 204));
    }

    #[test]
    fn short_animations_let_clicks_through_their_overlays() {
        let blocking = WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE;
        let through = blocking | WS_EX_LAYERED | WS_EX_TRANSPARENT;
        assert_eq!(overlay_ex_style(140), through); // the default
        assert_eq!(overlay_ex_style(0), through);
        assert_eq!(overlay_ex_style(CLICK_THROUGH_MAX_MS), through);
        // Long animations keep blocking input: a click would land on a layout
        // the user has not seen yet.
        assert_eq!(overlay_ex_style(CLICK_THROUGH_MAX_MS + 1), blocking);
        assert_eq!(overlay_ex_style(2000), blocking);
    }

    #[test]
    fn focus_follows_mouse_waits_out_the_settle_window() {
        assert!(!focus_mouse_allowed(100, 300));
        assert!(!focus_mouse_allowed(299, 300));
        assert!(focus_mouse_allowed(300, 300));
        assert!(focus_mouse_allowed(301, 300));
        assert!(focus_mouse_allowed(5, 0), "never settled");
    }

    // ---- wallpaper cache ----------------------------------------------------

    #[test]
    fn wallpaper_is_rendered_only_when_an_animation_reads_it() {
        // Animations off: never, whatever the styles say.
        assert!(!wallpaper_needed(false, WsAnim::Slide, "glide"));
        // Fade and off never read it; only a glide would.
        assert!(!wallpaper_needed(true, WsAnim::Fade, "off"));
        assert!(!wallpaper_needed(true, WsAnim::Off, "off"));
        assert!(wallpaper_needed(true, WsAnim::Fade, "glide"));
        assert!(wallpaper_needed(true, WsAnim::Off, "glide"));
        // Moving slide / spring frames read it with or without the glide.
        assert!(wallpaper_needed(true, WsAnim::Slide, "off"));
        assert!(wallpaper_needed(true, WsAnim::Spring, "off"));
    }

    #[test]
    fn only_moving_slides_take_a_wallpaper() {
        assert!(slide_wants_wallpaper(WsAnim::Slide, true));
        assert!(slide_wants_wallpaper(WsAnim::Spring, true));
        // Fade blends whole captures; a first visit holds frame 0.
        assert!(!slide_wants_wallpaper(WsAnim::Fade, true));
        assert!(!slide_wants_wallpaper(WsAnim::Slide, false));
        assert!(!slide_wants_wallpaper(WsAnim::Spring, false));
        assert!(!slide_wants_wallpaper(WsAnim::Fade, false));
    }

    #[test]
    fn a_wallpaper_crop_is_used_only_on_an_exact_match() {
        let wa = RECT {
            left: 0,
            top: 40,
            right: 1920,
            bottom: 1080,
        };
        assert!(wp_entry_usable(7, 7, 0x10, wa, 0x10, wa));
        // Stale gen: a different wallpaper, never blitted.
        assert!(!wp_entry_usable(6, 7, 0x10, wa, 0x10, wa));
        // Another monitor.
        assert!(!wp_entry_usable(7, 7, 0x11, wa, 0x10, wa));
        // Same monitor, different work area (bar or resolution change).
        let taller = RECT { top: 0, ..wa };
        assert!(!wp_entry_usable(7, 7, 0x10, taller, 0x10, wa));
        let wider = RECT { right: 1921, ..wa };
        assert!(!wp_entry_usable(7, 7, 0x10, wa, 0x10, wider));
    }

    // ---- glide compose ------------------------------------------------------

    #[test]
    fn glide_blits_equal_sizes_and_stretches_the_rest() {
        assert_eq!(glide_blit_kind(950, 1060, 950, 1060), GlideBlit::Blit);
        // Off by one either way is a real scale.
        assert_eq!(glide_blit_kind(951, 1060, 950, 1060), GlideBlit::Stretch);
        assert_eq!(glide_blit_kind(950, 1059, 950, 1060), GlideBlit::Stretch);
        // Any non-positive size draws nothing.
        assert_eq!(glide_blit_kind(0, 1060, 950, 1060), GlideBlit::Skip);
        assert_eq!(glide_blit_kind(950, -3, 950, 1060), GlideBlit::Skip);
        assert_eq!(glide_blit_kind(950, 1060, 0, 1060), GlideBlit::Skip);
        assert_eq!(glide_blit_kind(950, 1060, 950, -1), GlideBlit::Skip);
    }

    #[test]
    fn glide_still_is_the_two_pixel_jitter_band() {
        let a = RECT {
            left: 10,
            top: 10,
            right: 500,
            bottom: 400,
        };
        assert!(glide_still(&a, &a));
        let jitter = RECT {
            left: 12,
            top: 8,
            right: 502,
            bottom: 398,
        };
        assert!(glide_still(&a, &jitter));
        assert!(!glide_still(&a, &RECT { left: 13, ..a }));
        assert!(!glide_still(&a, &RECT { bottom: 403, ..a }));
    }

    fn rc(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn glide_damage_covers_only_what_moves_plus_the_shadow() {
        // 1920x1040 work area. Left window static (1 px jitter), right column
        // split: the top one grows down over a closed sibling's slot.
        let items = [
            GlideItem {
                old: rc(8, 8, 956, 1032),
                new: rc(9, 8, 956, 1032),
            },
            GlideItem {
                old: rc(964, 8, 1912, 516),
                new: rc(964, 8, 1912, 1032),
            },
        ];
        // Union over the moving item only, grown by 32 and clamped.
        assert_eq!(
            glide_damage(&items, 32, 1920, 1040),
            Some(rc(932, 0, 1920, 1040))
        );
        // No margin: the exact union.
        assert_eq!(
            glide_damage(&items, 0, 1920, 1040),
            Some(rc(964, 8, 1912, 1032))
        );
        // Nothing moves beyond the jitter band: no glide at all.
        assert_eq!(glide_damage(&items[..1], 32, 1920, 1040), None);
        assert_eq!(glide_damage(&[], 32, 1920, 1040), None);
    }

    #[test]
    fn glide_damage_near_the_whole_area_takes_the_full_path() {
        let full = Some(rc(0, 0, 1920, 1040));
        // A layout change moving everything: the whole work area.
        let items = [
            GlideItem {
                old: rc(8, 8, 956, 1032),
                new: rc(8, 8, 1280, 1032),
            },
            GlideItem {
                old: rc(964, 8, 1912, 1032),
                new: rc(1288, 8, 1912, 1032),
            },
        ];
        assert_eq!(glide_damage(&items, 32, 1920, 1040), full);
        // Just under the threshold stays a sub-rect, at it goes full: 90% of
        // 1000x1000 is 900000 = 900x1000.
        let near = |right: i32| {
            [GlideItem {
                old: rc(0, 0, 10, 1000),
                new: rc(0, 0, right, 1000),
            }]
        };
        assert_eq!(
            glide_damage(&near(899), 0, 1000, 1000),
            Some(rc(0, 0, 899, 1000))
        );
        assert_eq!(
            glide_damage(&near(900), 0, 1000, 1000),
            Some(rc(0, 0, 1000, 1000))
        );
        // A move entirely off the work area (another monitor) shows nothing.
        let off = [GlideItem {
            old: rc(-900, 0, -500, 400),
            new: rc(-800, 0, -400, 400),
        }];
        assert_eq!(glide_damage(&off, 32, 1000, 1000), None);
    }

    #[test]
    fn fade_alpha_runs_from_the_outgoing_to_the_incoming_frame() {
        // Endpoints match what frame 0 and the reveal show, even past them.
        assert_eq!(fade_alpha(-0.5), 0);
        assert_eq!(fade_alpha(0.0), 0);
        assert_eq!(fade_alpha(1.0), 255);
        assert_eq!(fade_alpha(7.0), 255);
        // Never steps back in between.
        let mut last = 0u8;
        for i in 0..=1000 {
            let a = fade_alpha(i as f64 / 1000.0);
            assert!(a >= last, "alpha fell at t={}", i as f64 / 1000.0);
            last = a;
        }
    }

    #[test]
    fn rects_overlap_needs_a_shared_pixel() {
        let a = rc(0, 0, 10, 10);
        assert!(rects_overlap(&a, &rc(9, 9, 20, 20)));
        assert!(
            !rects_overlap(&a, &rc(10, 0, 20, 10)),
            "touching edges share none"
        );
        assert!(!rects_overlap(&a, &rc(0, 10, 10, 20)));
        assert!(rects_overlap(&a, &rc(-5, -5, 50, 50)));
    }

    // ---- workspace snapshots ------------------------------------------------

    fn snap(bmp: isize, w: i32, h: i32) -> Snap {
        Snap {
            bmp: Bmp::new(bmp).unwrap(),
            rects: vec![RECT {
                left: 0,
                top: 0,
                right: 10,
                bottom: 10,
            }],
            w,
            h,
        }
    }

    /// Handles the test deleter has freed on this thread so far, cleared.
    fn freed() -> Vec<isize> {
        BMP_FREED.with(|f| std::mem::take(&mut *f.borrow_mut()))
    }

    #[test]
    fn a_snapshot_is_taken_once_and_handed_over_whole() {
        let mut map = SnapMap::new();
        map.insert((0x10, 2), snap(0xB1, 1920, 1040));
        map.insert((0x10, 3), snap(0xB2, 1920, 1040));
        let mut rejected = Vec::new();
        let got = snap_take_from(&mut map, (0x10, 2), 1920, 1040, |b| rejected.push(b.raw()));
        let got = got.expect("first take hands the snapshot over");
        assert_eq!((got.bmp.raw(), got.rects.len()), (0xB1, 1));
        // Gone from the cache: a second take finds nothing, and nothing else
        // was touched.
        assert!(
            snap_take_from(&mut map, (0x10, 2), 1920, 1040, |b| rejected.push(b.raw())).is_none()
        );
        assert!(map.contains_key(&(0x10, 3)));
        assert!(rejected.is_empty());
    }

    #[test]
    fn a_wrong_size_snapshot_is_removed_and_freed_not_handed_over() {
        let mut map = SnapMap::new();
        map.insert((0x10, 2), snap(0xB1, 1920, 1040));
        let mut rejected = Vec::new();
        // The work area shrank (bar height, resolution) since it was stored.
        let got = snap_take_from(&mut map, (0x10, 2), 1920, 1000, |b| rejected.push(b.raw()));
        assert!(got.is_none());
        assert_eq!(rejected, vec![0xB1]);
        assert!(map.is_empty());
        // A missing key rejects nothing.
        assert!(
            snap_take_from(&mut map, (0x11, 0), 1920, 1000, |b| rejected.push(b.raw())).is_none()
        );
        assert_eq!(rejected, vec![0xB1]);
    }

    #[test]
    fn a_removed_snapshot_is_freed_exactly_once() {
        let _ = freed();
        let mut map = SnapMap::new();
        map.insert((0x10, 2), snap(0xB1, 1920, 1040));
        map.insert((0x10, 3), snap(0xB2, 1920, 1040));
        let mut dead = Vec::new();
        assert!(snap_remove_from(&mut map, (0x10, 2), |b| dead.push(b)));
        assert!(
            freed().is_empty(),
            "not freed while the caller still holds it"
        );
        drop(dead);
        assert_eq!(freed(), vec![0xB1]);
        assert!(!map.contains_key(&(0x10, 2)));
        assert!(map.contains_key(&(0x10, 3)), "only that entry");
        // Nothing left to remove: nothing freed twice.
        assert!(!snap_remove_from(&mut map, (0x10, 2), drop)); // was |b| drop(b) ));
        assert!(freed().is_empty());
    }

    #[test]
    fn an_owned_bitmap_is_freed_exactly_once_and_cannot_be_cloned() {
        let _ = freed();
        // A failed capture is no bitmap at all: nothing to own, nothing freed.
        assert!(Bmp::new(0).is_none());
        let b = Bmp::new(0xC1).unwrap();
        // Moved through a hand-off (a request, the return queue): still one owner.
        let moved = Some(b);
        assert!(freed().is_empty());
        drop(moved);
        assert_eq!(freed(), vec![0xC1]);
        // Compile-time: Bmp must not be Clone (and so not Copy). If it were,
        // both impls below would apply and the `_` would be ambiguous.
        trait AmbiguousIfClone<A> {
            fn check() {}
        }
        impl<T: ?Sized> AmbiguousIfClone<()> for T {}
        impl<T: Clone> AmbiguousIfClone<u8> for T {}
        <Bmp as AmbiguousIfClone<_>>::check();
    }

    fn ret(key: (isize, usize), gen: u64, bmp: isize) -> SnapReturn {
        SnapReturn {
            key,
            gen,
            snap: snap(bmp, 1920, 1040),
        }
    }

    #[test]
    fn the_first_capture_after_a_clear_comes_back_as_the_snapshot() {
        // The only test touching the SNAP / SNAP_RETURNS statics. No real
        // GDI object is involved (the test deleter records; GdiFlush is a
        // no-op without a batch).
        unsafe {
            snap_clear(); // no cache at all: the state after startup or a reload
            let _ = freed();
            snap_keep(0x20, 1, 5);
            snap_return(ret((0x20, 1), 5, 0xD1));
            // A stale capture of another key, handed back meanwhile, is freed.
            snap_return(ret((0x20, 2), 4, 0xD2));
            snap_drain();
            assert_eq!(freed(), vec![0xD2]);
            let (bmp, rects) = snap_take(0x20, 1, 1920, 1040).expect("kept on its return");
            assert_eq!((bmp.raw(), rects.len()), (0xD1, 1));
            drop(bmp);
            // Left again without a keepable capture: the next one is disowned.
            snap_keep(0x20, 1, 6);
            assert!(!snap_remove(0x20, 1));
            snap_return(ret((0x20, 1), 6, 0xD3));
            snap_drain();
            assert!(snap_take(0x20, 1, 1920, 1040).is_none());
            let mut f = freed();
            f.sort_unstable();
            assert_eq!(f, vec![0xD1, 0xD3]);
            snap_clear();
        }
    }

    #[test]
    fn a_returned_capture_is_kept_only_if_it_is_the_newest_wanted() {
        let _ = freed();
        let mut cache = SnapCache::default();
        // (m, 1): left with capture 7, whose capture is wanted; an entry
        // already sits there (stored by some earlier drain).
        cache.map.insert((0x10, 1), snap(0xA0, 1920, 1040));
        cache.newest.insert((0x10, 1), 7);
        // (m, 2): left again since (newest 9), so capture 8 is stale.
        cache.newest.insert((0x10, 2), 9);
        // (m, 3): left with an overlay in the capture, or a reload since:
        // no newest entry at all.
        let rets = vec![
            ret((0x10, 1), 7, 0xB7),
            ret((0x10, 2), 8, 0xB8),
            ret((0x10, 3), 5, 0xB5),
        ];
        let mut dead = Vec::new();
        snap_apply_returns(&mut cache, rets, &mut dead);
        // Kept: 7, replacing (and freeing, once) the older entry.
        assert_eq!(cache.map.get(&(0x10, 1)).map(|s| s.bmp.raw()), Some(0xB7));
        assert!(
            !cache.newest.contains_key(&(0x10, 1)),
            "a gen is stored at most once"
        );
        // Stale and unwanted ones never land, and 9 is still awaited.
        assert!(!cache.map.contains_key(&(0x10, 2)));
        assert!(!cache.map.contains_key(&(0x10, 3)));
        assert_eq!(cache.newest.get(&(0x10, 2)), Some(&9));
        // Nothing freed under the (would-be) lock; the caller drops `dead`.
        assert!(freed().is_empty());
        drop(dead);
        let mut f = freed();
        f.sort_unstable();
        assert_eq!(f, vec![0xA0, 0xB5, 0xB8]);
        // The same capture returned twice (a bug) is not stored twice.
        let mut dead = Vec::new();
        snap_apply_returns(&mut cache, vec![ret((0x10, 1), 7, 0xB9)], &mut dead);
        assert_eq!(cache.map.get(&(0x10, 1)).map(|s| s.bmp.raw()), Some(0xB7));
        drop(dead);
        assert_eq!(freed(), vec![0xB9]);
    }

    #[test]
    fn only_a_cross_monitor_move_reflows_the_source_at_once() {
        assert!(!move_needs_source_retile(0, 0));
        assert!(!move_needs_source_retile(2, 2));
        assert!(move_needs_source_retile(1, 0));
        assert!(move_needs_source_retile(0, 1));
    }

    #[test]
    fn a_pure_move_keeps_its_width_every_frame() {
        // The compose lerp: the Blit fast path only fires if moving both edges
        // by the same delta never changes the rounded width.
        let lerp = |a: i32, b: i32, e: f64| (a as f64 + (b - a) as f64 * e).round() as i32;
        for step in 0..=100 {
            let e = step as f64 / 100.0;
            let l = lerp(10, 973, e);
            let r = lerp(960, 1923, e);
            assert_eq!(r - l, 950, "e = {e}");
        }
    }

    // ---- workspace model --------------------------------------------------
    // `Manager` owns window membership. These tests exercise it directly (no
    // Win32, no real windows), which is the whole point of routing every
    // membership change through `move_window`/`detach_window`: the bug class
    // they replaced — a window dropped from `floating`, or left owned by the
    // monitor it is no longer on — is now checkable in CI.

    fn test_manager(monitors: usize, workspaces: usize) -> Manager {
        let mons = (0..monitors)
            .map(|i| {
                Monitor::new(
                    0x1000 + i as isize,
                    RECT {
                        left: i as i32 * 1920,
                        top: 0,
                        right: (i as i32 + 1) * 1920,
                        bottom: 1080,
                    },
                    workspaces,
                )
            })
            .collect();
        // INDEX is a global mirror of membership; tests must not inherit a
        // previous test's snapshot, and `locate` falls back to a linear scan.
        *INDEX.lock().unwrap() = None;
        Manager {
            monitors: mons,
            focused_mon: 0,
            primary: 0,
            tiling: true,
            cfg: Config::defaults(),
            pending_launch_mon: 0,
            park_origin: None,
        }
    }

    /// Every tracked window appears exactly once across every workspace, and
    /// `floating` is always a subset of `windows`.
    fn assert_model_sound(mgr: &Manager, expect: &[isize]) {
        let mut seen: Vec<isize> = Vec::new();
        for m in &mgr.monitors {
            for ws in &m.workspaces {
                for &h in &ws.windows {
                    assert!(!seen.contains(&h), "window {h:#x} is in two workspaces");
                    seen.push(h);
                }
                for &f in &ws.floating {
                    assert!(
                        ws.windows.contains(&f),
                        "floating {f:#x} is not in its workspace's windows"
                    );
                }
                assert!(
                    ws.focused == 0 || ws.windows.contains(&ws.focused),
                    "workspace focus {:#x} is not one of its own windows",
                    ws.focused
                );
            }
        }
        seen.sort_unstable();
        let mut want = expect.to_vec();
        want.sort_unstable();
        assert_eq!(seen, want, "windows were lost or duplicated");
    }

    fn add(mgr: &mut Manager, mi: usize, wi: usize, h: isize, floating: bool) {
        let ws = &mut mgr.monitors[mi].workspaces[wi];
        ws.windows.push(h);
        if floating {
            ws.floating.push(h);
        }
        ws.focused = h;
    }

    #[test]
    fn move_window_carries_the_floating_flag() {
        // B-07: Alt+Shift+<n> on a floated window silently re-tiled it.
        let mut mgr = test_manager(1, 3);
        add(&mut mgr, 0, 0, 0xA, true);
        assert!(mgr.move_window(0xA, 0, 2, None));
        assert!(mgr.monitors[0].workspaces[2].floating.contains(&0xA));
        assert!(mgr.monitors[0].workspaces[0].floating.is_empty());
        assert_model_sound(&mgr, &[0xA]);
    }

    #[test]
    fn move_window_across_monitors_changes_owner() {
        // B-06: the window stayed owned by the monitor it had left, so
        // switching workspaces there hid a window visible on the other screen.
        let mut mgr = test_manager(2, 2);
        add(&mut mgr, 0, 0, 0xA, true);
        add(&mut mgr, 0, 0, 0xB, false);
        assert!(mgr.move_window(0xA, 1, 0, None));
        assert_eq!(mgr.locate(0xA), Some((1, 0)));
        assert!(mgr.monitors[1].workspaces[0].floating.contains(&0xA));
        // The source workspace repaired its own focus rather than pointing at a
        // window it no longer owns.
        assert_eq!(mgr.monitors[0].workspaces[0].focused, 0xB);
        assert_model_sound(&mgr, &[0xA, 0xB]);
    }

    #[test]
    fn move_window_to_a_missing_destination_changes_nothing() {
        let mut mgr = test_manager(1, 2);
        add(&mut mgr, 0, 0, 0xA, false);
        assert!(!mgr.move_window(0xA, 5, 0, None), "bogus monitor");
        assert!(!mgr.move_window(0xA, 0, 9, None), "bogus workspace");
        assert_eq!(mgr.locate(0xA), Some((0, 0)));
        assert_model_sound(&mgr, &[0xA]);
    }

    #[test]
    fn move_window_honours_the_drop_position() {
        let mut mgr = test_manager(2, 1);
        add(&mut mgr, 1, 0, 0xB, false);
        add(&mut mgr, 1, 0, 0xC, false);
        add(&mut mgr, 0, 0, 0xA, false);
        assert!(mgr.move_window(0xA, 1, 0, Some(1)));
        assert_eq!(mgr.monitors[1].workspaces[0].windows, vec![0xB, 0xA, 0xC]);
        assert_model_sound(&mgr, &[0xA, 0xB, 0xC]);
    }

    #[test]
    fn a_window_is_never_lost_by_any_sequence_of_moves() {
        let mut mgr = test_manager(3, 4);
        let all: Vec<isize> = (1..=9).collect();
        for (i, &h) in all.iter().enumerate() {
            add(&mut mgr, i % 3, i % 4, h, i % 2 == 0);
        }
        assert_model_sound(&mgr, &all);
        // Deterministic shuffle: every window visits every monitor/workspace.
        for round in 0..7usize {
            for (i, &h) in all.iter().enumerate() {
                let to_mi = (i + round) % 3;
                let to_wi = (i * 2 + round) % 4;
                assert!(mgr.move_window(h, to_mi, to_wi, None));
                assert_model_sound(&mgr, &all);
            }
        }
    }

    #[test]
    fn detach_repairs_focus_and_reports_floating() {
        let mut mgr = test_manager(1, 1);
        add(&mut mgr, 0, 0, 0xA, false);
        add(&mut mgr, 0, 0, 0xB, true);
        assert_eq!(mgr.detach_window(0xB), Some((0, 0, true)));
        assert_eq!(mgr.monitors[0].workspaces[0].focused, 0xA);
        assert_eq!(mgr.detach_window(0xB), None, "already gone");
        assert_model_sound(&mgr, &[0xA]);
    }

    #[test]
    fn focused_never_indexes_out_of_range() {
        let mut mgr = test_manager(1, 1);
        add(&mut mgr, 0, 0, 0xA, false);
        // A stale focused_mon (monitor unplugged mid-command) must not panic —
        // `panic = "abort"` would take the WM down and strand hidden windows.
        mgr.focused_mon = 7;
        let (mi, _, _) = mgr.focused();
        assert_eq!(mi, 0);
        mgr.monitors.clear();
        assert_eq!(mgr.focused(), (0, 0, 0));
    }

    #[test]
    fn shrinking_the_workspace_count_keeps_windows_and_focus() {
        // B-14: the folded workspace's `focused` was dropped on the floor.
        let mut mgr = test_manager(1, 3);
        add(&mut mgr, 0, 2, 0xA, false);
        add(&mut mgr, 0, 2, 0xB, true);
        mgr.monitors[0].workspaces[2].focused = 0xB;
        distribute_workspaces(&mut mgr.monitors, 0, 1, true);
        assert_eq!(mgr.monitors[0].workspaces.len(), 1);
        assert_eq!(mgr.locate(0xA), Some((0, 0)));
        assert!(
            mgr.monitors[0].workspaces[0].floating.contains(&0xB),
            "folding a workspace must not re-tile its floating windows"
        );
        assert_eq!(mgr.monitors[0].workspaces[0].focused, 0xB);
        assert_model_sound(&mgr, &[0xA, 0xB]);
    }

    // ---- switch bursts ------------------------------------------------------

    fn queue(cmds: Vec<Cmd>) -> VecDeque<Cmd> {
        cmds.into()
    }

    fn five_workspaces() -> Manager {
        let mut mgr = test_manager(1, 5);
        mgr.cfg.workspaces = 5;
        mgr
    }

    #[test]
    fn a_later_switch_overrides_earlier_wheel_steps() {
        let mgr = five_workspaces();
        let hm = mgr.monitors[0].hmon;
        let mut rest = queue(vec![Cmd::Switch(3)]);
        let got = mgr.fold_switches(&Cmd::BarCycle(hm, 1), &mut rest);
        assert_eq!(got, (Some((0, 3)), 1));
        assert!(rest.is_empty());
    }

    #[test]
    fn a_wheel_step_applies_to_where_the_burst_has_got_to() {
        let mgr = five_workspaces();
        let hm = mgr.monitors[0].hmon;
        // [Switch(5th), BarCycle(+1)] wraps to the 1st, not active + 1.
        let mut rest = queue(vec![Cmd::BarCycle(hm, 1)]);
        assert_eq!(
            mgr.fold_switches(&Cmd::Switch(4), &mut rest),
            (Some((0, 0)), 1)
        );
        let mut rest = queue(vec![Cmd::BarCycle(hm, 1), Cmd::BarCycle(hm, 1)]);
        assert_eq!(
            mgr.fold_switches(&Cmd::BarCycle(hm, 1), &mut rest),
            (Some((0, 3)), 2)
        );
    }

    #[test]
    fn a_no_op_switch_never_becomes_the_target() {
        let mgr = five_workspaces();
        // Alt+2 then Alt+7 with 5 workspaces: today that ends on ws2.
        let mut rest = queue(vec![Cmd::Switch(6)]);
        assert_eq!(
            mgr.fold_switches(&Cmd::Switch(1), &mut rest),
            (Some((0, 1)), 1)
        );
        // Every command a no-op (bad index, unknown bar): nothing to do.
        let mut rest = queue(vec![Cmd::Switch(99), Cmd::BarCycle(0xDEAD, 1)]);
        assert_eq!(mgr.fold_switches(&Cmd::Switch(9), &mut rest), (None, 2));
        // One workspace: the wheel cannot cycle.
        let one = test_manager(1, 1);
        let hm = one.monitors[0].hmon;
        assert_eq!(
            one.fold_switches(&Cmd::BarCycle(hm, 1), &mut VecDeque::new()),
            (None, 0)
        );
    }

    #[test]
    fn a_burst_stops_at_any_other_command() {
        let mgr = five_workspaces();
        let mut rest = queue(vec![Cmd::Switch(2), Cmd::Focused(0xA), Cmd::Switch(3)]);
        assert_eq!(
            mgr.fold_switches(&Cmd::Switch(1), &mut rest),
            (Some((0, 2)), 1)
        );
        assert!(matches!(rest.front(), Some(Cmd::Focused(0xA))));
        assert_eq!(rest.len(), 2);
        // Extra may switch internally: never looked inside, never crossed.
        let mut rest = queue(vec![Cmd::Extra(0), Cmd::Switch(3)]);
        assert_eq!(
            mgr.fold_switches(&Cmd::Switch(1), &mut rest),
            (Some((0, 1)), 0)
        );
        assert_eq!(rest.len(), 2);
    }

    #[test]
    fn a_burst_stops_at_a_switch_for_another_monitor() {
        // Shared mode, primary 0: even globals on monitor 0, odd on monitor 1.
        let mgr = test_manager(2, 3);
        let mut rest = queue(vec![Cmd::Switch(2), Cmd::Switch(1), Cmd::Switch(4)]);
        assert_eq!(
            mgr.fold_switches(&Cmd::Switch(0), &mut rest),
            (Some((0, 1)), 1)
        );
        assert_eq!(rest.len(), 2);
        let hm1 = mgr.monitors[1].hmon;
        let mut rest = queue(vec![Cmd::BarCycle(hm1, 1)]);
        assert_eq!(
            mgr.fold_switches(&Cmd::Switch(2), &mut rest),
            (Some((0, 1)), 0)
        );
        assert_eq!(rest.len(), 1);
    }

    #[test]
    fn per_monitor_switches_go_where_a_wheel_step_moved_focus() {
        let mut mgr = test_manager(2, 3);
        mgr.cfg.per_monitor = true;
        mgr.cfg.workspaces = 3;
        let hm1 = mgr.monitors[1].hmon;
        // The wheel over monitor 1's bar focuses monitor 1, so the Switch
        // behind it lands there too, as it does unfolded.
        let mut rest = queue(vec![Cmd::Switch(2)]);
        assert_eq!(
            mgr.fold_switches(&Cmd::BarCycle(hm1, 1), &mut rest),
            (Some((1, 2)), 1)
        );
    }

    #[test]
    fn a_burst_that_ends_where_it_started_targets_the_active_workspace() {
        let mut mgr = five_workspaces();
        mgr.monitors[0].active = 2;
        let hm = mgr.monitors[0].hmon;
        let mut rest = queue(vec![Cmd::BarCycle(hm, -1)]);
        // == active: show_workspace then takes the already-showing branch.
        assert_eq!(
            mgr.fold_switches(&Cmd::BarCycle(hm, 1), &mut rest),
            (Some((0, 2)), 1)
        );
    }

    #[test]
    fn the_wheel_steps_once_per_full_notch() {
        let mut acc = 0;
        assert_eq!([40, 40, 40].map(|d| wheel_steps(&mut acc, d)), [0, 0, 1]);
        assert_eq!(acc, 0);
        // A flip drops the partial notch rather than cancelling against it.
        let mut acc = 0;
        assert_eq!(wheel_steps(&mut acc, 80), 0);
        assert_eq!(wheel_steps(&mut acc, -80), 0);
        assert_eq!(acc, -80);
        // Several notches in one event (a fast spin), both ways.
        let mut acc = 0;
        assert_eq!(wheel_steps(&mut acc, 240), 2);
        assert_eq!(wheel_steps(&mut acc, -360), -3);
    }

    #[test]
    fn slide_generations_and_holds_never_cross() {
        // An abort for k-1 cannot stop k; k's own abort, or a newer request, does.
        assert!(slide_gen_live(5, 5, 4));
        assert!(!slide_gen_live(5, 5, 5));
        assert!(!slide_gen_live(5, 6, 0));
        // Only an overlay on the glass can be held; every hold is a new value.
        assert_eq!(glass_hold(0), None);
        let on = 5u64 << 16;
        assert_eq!(glass_hold(on), Some(on + 1));
        assert_eq!(glass_hold(on + 1), Some(on + 2));
        assert_eq!(glass_hold(on | GLASS_HOLDS), None);
        // A release matches only that exact hold: not an older one, not another
        // gen's, and never an overlay nobody holds.
        assert!(glass_released(on + 2, on + 2));
        assert!(!glass_released(on + 2, on + 1));
        assert!(!glass_released(on + 1, (4u64 << 16) + 1));
        assert!(!glass_released(on, on));
    }

    #[test]
    fn monitor_cover_requires_all_four_edges() {
        let monitor = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        let exact = monitor;
        let dwm_tolerance = RECT {
            left: -1918,
            top: 2,
            right: -2,
            bottom: 1078,
        };
        let navbar_reserved = RECT {
            left: -1920,
            top: 32,
            right: 0,
            bottom: 1080,
        };
        let taskbar_reserved = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1040,
        };

        assert!(rect_covers_monitor(exact, monitor));
        assert!(rect_covers_monitor(dwm_tolerance, monitor));
        assert!(!rect_covers_monitor(navbar_reserved, monitor));
        assert!(!rect_covers_monitor(taskbar_reserved, monitor));
    }

    // ---- launcher file search (batch 9, LAUNCH-10 / LAUNCH-11) ------------------

    #[test]
    fn provisional_rows_are_the_names_every_term_still_prefixes() {
        let names = ["Annual Report 2024.pdf", "report-draft.docx", "Budget.xlsx"];
        let keep = |q: &str| provisional_hits(names.iter().copied(), q);
        assert_eq!(keep("rep"), vec![0, 1]);
        // Case-insensitive, and every term must match (the AND of the query).
        assert_eq!(keep("ANN rep"), vec![0]);
        // A term prefixes a word; it does not match mid-word.
        assert_eq!(keep("eport"), Vec::<usize>::new());
        // Refinement only narrows: the result is a subset of the wider query's.
        let wide = keep("re");
        assert!(keep("repo").iter().all(|i| wide.contains(i)));
        // No 2+ char term: the index query returns nothing, and so does this.
        assert!(build_contains("r").is_none());
        assert_eq!(keep("r"), Vec::<usize>::new());
        assert_eq!(keep("   "), Vec::<usize>::new());
        // Quotes are stripped exactly as build_contains strips them.
        assert_eq!(keep("\"bud"), vec![2]);
    }

    #[test]
    fn a_search_result_is_stored_only_for_the_newest_generation_and_forward() {
        assert!(should_store_search(5, 5, 4));
        // Superseded while it ran.
        assert!(!should_store_search(5, 6, 4));
        // A newer generation already landed (the two-worker race).
        assert!(!should_store_search(5, 5, 5));
        assert!(!should_store_search(5, 5, 6));
    }

    #[test]
    fn the_file_result_cache_keeps_the_most_recent_queries() {
        let mut cache: VecDeque<(String, u32)> = VecDeque::new();
        for i in 0..3 {
            file_cache_put(&mut cache, format!("q{i}"), i, 2);
        }
        // Capacity 2: the oldest went first.
        assert!(file_cache_get(&mut cache, "q0").is_none());
        assert_eq!(file_cache_get(&mut cache, "q1"), Some(&1));
        // q1 is now the most recent, so a new entry pushes out q2.
        file_cache_put(&mut cache, "q3".into(), 3, 2);
        assert!(file_cache_get(&mut cache, "q2").is_none());
        assert_eq!(file_cache_get(&mut cache, "q1"), Some(&1));
        // Re-putting a key replaces it rather than duplicating it.
        file_cache_put(&mut cache, "q1".into(), 9, 2);
        assert_eq!(cache.len(), 2);
        assert_eq!(file_cache_get(&mut cache, "q1"), Some(&9));
    }

    // ---- config watcher (batch 12, LAUNCH-18) ------------------------------------

    #[test]
    fn two_writes_40ms_apart_read_once_120ms_after_the_last() {
        use std::time::Duration;
        let t0 = Instant::now();
        let idle = Duration::from_secs(10);
        let mut d = Debounce {
            quiet: Duration::from_millis(120),
            due: None,
        };
        assert_eq!(d.wait(t0, idle), idle, "nothing pending: the backstop");
        d.event(t0);
        d.event(t0 + Duration::from_millis(40));
        // The first write's deadline (t0 + 120) no longer fires.
        assert_eq!(
            d.wait(t0 + Duration::from_millis(120), idle),
            Duration::from_millis(40)
        );
        assert_eq!(
            d.wait(t0 + Duration::from_millis(160), idle),
            Duration::ZERO
        );
    }

    #[test]
    fn notify_records_yield_every_file_name_and_stop_at_a_short_buffer() {
        fn record(name: &str, last: bool) -> Vec<u8> {
            let wide: Vec<u8> = name.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
            // Records are DWORD-aligned; the offset includes that padding.
            let len = 12 + wide.len();
            let padded = (len + 3) & !3;
            let mut r = Vec::new();
            r.extend_from_slice(&(if last { 0 } else { padded as u32 }).to_le_bytes());
            r.extend_from_slice(&3u32.to_le_bytes()); // FILE_ACTION_MODIFIED
            r.extend_from_slice(&(wide.len() as u32).to_le_bytes());
            r.extend_from_slice(&wide);
            r.resize(padded, 0);
            r
        }
        let mut buf = record("astur.log", false);
        buf.extend(record("navbar.conf", true));
        assert_eq!(notify_file_names(&buf), vec!["astur.log", "navbar.conf"]);
        // A name running past the end is dropped, not read out of bounds.
        let cut = &buf[..buf.len() - 8];
        assert_eq!(notify_file_names(cut), vec!["astur.log"]);
        assert!(notify_file_names(&[]).is_empty());
    }

    // ---- blocking work off the hook thread (batch 13, BAR-15 / INPUT-12) ------

    #[test]
    fn queued_volume_steps_sum_and_mute_clicks_cancel_in_pairs() {
        assert_eq!(volume_drain(0, 0), (0.0, false));
        assert_eq!(volume_drain(6, 0), (0.06, false));
        assert_eq!(volume_drain(-2, 1), (-0.02, true));
        // Two clicks before the worker ran: muted and unmuted again.
        assert_eq!(volume_drain(0, 2), (0.0, false));
        assert_eq!(volume_drain(0, 3), (0.0, true));
    }

    #[test]
    fn the_watchdog_re_arms_only_for_input_the_hooks_could_have_seen() {
        let (silent, typing) = (WATCHDOG_SILENCE_MS, 0u32);
        // Plain dead hooks: input, no callbacks.
        assert!(hooks_look_dead(silent, typing, false, false, false));
        // Not silent long enough, or nobody is using the machine.
        assert!(!hooks_look_dead(silent - 1, typing, false, false, false));
        assert!(!hooks_look_dead(
            silent,
            WATCHDOG_INPUT_WINDOW_MS + 1,
            false,
            false,
            false
        ));
        // UIPI: an elevated foreground is invisible to a non-elevated Astur...
        assert!(!hooks_look_dead(silent, typing, true, false, false));
        // ...but not to an elevated one.
        assert!(hooks_look_dead(silent, typing, true, true, false));
        // The secure desktop is invisible to everyone.
        assert!(!hooks_look_dead(silent, typing, false, false, true));
        assert!(!hooks_look_dead(silent, typing, false, true, true));
    }

    // ---- icon cache (batch 14, LAUNCH-13) ----------------------------------------

    #[test]
    fn an_icon_is_keyed_by_source_size_and_stamp() {
        let mut c: IconCache<i64> = IconCache::new(None);
        assert_eq!(c.insert("app.lnk", 32, 7, 100, 0), None);
        assert_eq!(c.get("app.lnk", 32, 7), Some(100));
        // px is part of the key: a 150% monitor does not get the 100% bitmap.
        assert_eq!(c.get("app.lnk", 48, 7), None);
        // A changed source (new mtime) is a miss, not a stale hit.
        assert_eq!(c.get("app.lnk", 32, 8), None);
        // ...but the paint may draw the nearest size of the same version.
        c.insert("app.lnk", 40, 7, 101, 0);
        assert_eq!(c.nearest("app.lnk", 48, 7, |h| h > 1), Some(101));
        assert_eq!(c.nearest("app.lnk", 48, 8, |h| h > 1), None);
    }

    #[test]
    fn a_placeholder_is_replaced_and_a_raced_duplicate_comes_back_to_free() {
        let mut c: IconCache<i64> = IconCache::new(None);
        c.insert("x.exe", 20, 0, 0, 0); // queued placeholder
        assert_eq!(c.insert("x.exe", 20, 0, 55, 0), None);
        assert_eq!(c.get("x.exe", 20, 0), Some(55));
        // A second worker resolved the same key: the stored one wins.
        assert_eq!(c.insert("x.exe", 20, 0, 56, 0), Some(56));
        assert_eq!(c.get("x.exe", 20, 0), Some(55));
    }

    #[test]
    fn the_lifetime_cache_never_deletes() {
        let mut c: IconCache<i64> = IconCache::new(None);
        for i in 0..100 {
            c.insert(&format!("app{i}"), 32, 0, i + 2, 0);
        }
        let mut deleted = Vec::new();
        c.evict(|_| false, |h| deleted.push(h));
        assert!(
            deleted.is_empty(),
            "app and bar icons are never LRU-evicted (only retain_live drops them)"
        );
        assert!(!c.over_cap());
    }

    #[test]
    fn retain_live_frees_dead_sizes_and_stamps_once_and_keeps_the_rest() {
        let mut c: IconCache<i64> = IconCache::new(None);
        c.insert("a.lnk", 32, 7, 10, 0);
        c.insert("a.lnk", 48, 7, 11, 0); // a size no monitor uses now
        c.insert("a.lnk", 32, 6, 12, 0); // superseded stamp
        c.insert("b.exe", 32, 0, 13, 0);
        let mut dead = Vec::new();
        c.retain_live(
            |src, px, stamp| px == 32 && (stamp == 0 || (src, stamp) == ("a.lnk", 7)),
            |h| dead.push(h),
        );
        dead.sort();
        assert_eq!(dead, vec![11, 12]);
        assert_eq!(c.get("a.lnk", 32, 7), Some(10));
        assert_eq!(c.get("b.exe", 32, 0), Some(13));
        assert_eq!(c.get("a.lnk", 48, 7), None);
        // The count follows, so a capped cache's over_cap stays true to it.
        let mut again = Vec::new();
        c.retain_live(|_, _, _| false, |h| again.push(h));
        again.sort();
        assert_eq!(again, vec![10, 13]);
        assert_eq!(c.len, 0);
        assert!(c.map.is_empty());
    }

    #[test]
    fn the_file_lru_frees_each_evicted_icon_once_and_never_a_listed_one() {
        let mut c: IconCache<i64> = IconCache::new(Some(3));
        for i in 0..6 {
            c.insert(&format!("f{i}"), 32, 0, 10 + i, 0);
        }
        // f0 is oldest but on screen; f4 was just drawn.
        c.get("f4", 32, 0);
        let mut deleted = Vec::new();
        c.evict(|src| src == "f0", |h| deleted.push(h));
        deleted.sort();
        // Three go (6 -> cap 3): the oldest unlisted ones, f1 f2 f3.
        assert_eq!(deleted, vec![11, 12, 13]);
        assert_eq!(c.get("f0", 32, 0), Some(10));
        assert_eq!(c.get("f4", 32, 0), Some(14));
        assert_eq!(c.get("f1", 32, 0), None);
        // Under cap now: a second pass frees nothing more.
        let mut again = Vec::new();
        c.evict(|_| false, |h| again.push(h));
        assert!(again.is_empty());
        // Every row listed: over cap, but nothing may go.
        for i in 6..9 {
            c.insert(&format!("f{i}"), 32, 0, 10 + i, 0);
        }
        let mut none = Vec::new();
        c.evict(|_| true, |h| none.push(h));
        assert!(none.is_empty());
        assert!(c.over_cap());
    }

    // ---- startup bar seed (batch 15, BAR-23) --------------------------------------

    #[test]
    fn the_startup_bar_seed_is_themed_and_carries_no_monitors() {
        let mut cfg = Config::defaults();
        cfg.start_tiled = false;
        let seed = bar_data_from(&cfg, true, cfg.start_tiled, Vec::new());
        assert!(!seed.tiling, "tiling follows start_tiled");
        assert!(
            seed.mons.is_empty(),
            "no pills until the first manager update"
        );
        assert_eq!(
            (seed.bg, seed.fg, seed.accent, seed.inactive),
            (
                config::BAR_LIGHT[0],
                config::BAR_LIGHT[1],
                config::BAR_LIGHT[2],
                config::BAR_LIGHT[3]
            )
        );
        assert_eq!(seed.layout, cfg.layout);
        assert!(seed.left == zone_widgets(&cfg.bar_left, &cfg));
        // An explicit colour still wins over the theme preset.
        cfg.bar_bg = Some(0x0012_3456);
        assert_eq!(bar_data_from(&cfg, true, true, Vec::new()).bg, 0x0012_3456);
    }

    // ---- bar visuals (batch 20, BAR-3 / BAR-9 / BAR-19) -------------------------

    #[test]
    fn a_retargeted_pill_slide_starts_where_the_highlight_is() {
        use std::time::Duration;
        let t0 = Instant::now();
        let a = PillAnim {
            from: 0.0,
            to_i: 4,
            start: t0,
        };
        let mid = t0 + Duration::from_millis(80); // half of PILL_ANIM_MS
        let (before, arrived) = pill_pos(&a, mid);
        assert!(!arrived && before > 0.0 && before < 4.0);
        // Alt+5 then Alt+2 mid-slide: the new slide begins exactly where the
        // highlight was, not back at update_bar's previous target (4).
        let b = pill_retarget(Some(&a), 4, 1, mid);
        assert_eq!(b.from, before);
        assert_eq!(pill_pos(&b, mid), (before, false));
        // Same curve and duration, ending on the new target.
        let (end, done) = pill_pos(&b, mid + Duration::from_millis(160));
        assert!(done && (end - 1.0).abs() < 1e-9);
        // A finished slide, or none, starts from the previous target as before.
        let late = pill_retarget(Some(&a), 4, 2, t0 + Duration::from_millis(200));
        assert_eq!(late.from, 4.0);
        assert_eq!(pill_retarget(None, 3, 5, t0).from, 3.0);
    }

    #[test]
    fn a_rename_refreshes_the_bar_of_any_monitor_showing_that_window() {
        let slots = [
            AtomicIsize::new(0x10),
            AtomicIsize::new(0x20),
            AtomicIsize::new(0),
        ];
        // Monitor 2's title while the user types on monitor 1: the case the
        // foreground-only filter left stale. A slot hit never asks for fg.
        assert!(namechange_forward(
            0x20,
            || unreachable!("slot matched"),
            &slots
        ));
        // The foreground window still counts (the only test past MAX_BARS).
        assert!(namechange_forward(0x30, || 0x30, &slots));
        // Shown nowhere and not foreground: kept off the manager queue.
        assert!(!namechange_forward(0x40, || 0x10, &slots));
        // An empty slot (0) never matches a null handle.
        assert!(!namechange_forward(0, || 0, &slots));
    }

    #[test]
    fn only_a_fullscreen_app_snaps_the_bar_hidden_and_never_under_the_pointer() {
        assert!(bar_snap_hide(true, false, false));
        // The pointer is on the bar or its reveal strip: the timer decides.
        assert!(!bar_snap_hide(true, false, true));
        // Configured auto-hide keeps its slide; no auto-hide, nothing to do.
        assert!(!bar_snap_hide(true, true, false));
        assert!(!bar_snap_hide(false, true, false));
        assert!(!bar_snap_hide(false, false, false));

        let strip = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 2,
        };
        let over = |x, y| bar_cursor_over(POINT { x, y }, 0, 1920, 100, 30, 8, strip);
        assert!(over(10, 110), "on the bar");
        assert!(over(10, 92) && over(10, 137), "inside the grab slack");
        assert!(!over(10, 91) && !over(10, 138));
        assert!(!over(1920, 110), "right edge is exclusive");
        assert!(over(1919, 1), "in the reveal strip");

        // Snap parks a new bar hidden in one step, and later rebuilds keep it
        // there (progress 1) through a geometry change.
        let geo = AhBar {
            x: 0,
            w: 1920,
            h: 30,
            y_shown: 0,
            y_hidden: -32,
            y_cur: 0.0,
            shown: true,
            strip,
            tol: 8,
        };
        let (snapped, fresh) = (-0x5eed_0001, -0x5eed_0002); // never a real bar hwnd
        assert_eq!(ah_bar_update(snapped, &geo, true), (-32, false));
        assert_eq!(ah_bar_update(snapped, &geo, false), (-32, false));
        let taller = AhBar {
            h: 40,
            y_hidden: -42,
            ..geo
        };
        assert_eq!(ah_bar_update(snapped, &taller, false), (-42, false));
        // Without a snap a new bar starts shown, as it always did.
        assert_eq!(ah_bar_update(fresh, &geo, false), (0, true));
        if let Some(m) = AH_BARS.lock().unwrap().as_mut() {
            m.remove(&snapped);
            m.remove(&fresh);
        }
    }
}

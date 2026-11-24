//! The UI thread.
//!
//! `Ui` owns every window RSB creates and runs the Win32 message loop. It is
//! not `Send`: create it on the thread that will run the loop. Window
//! procedures only record what happened; the loop then hands each event to
//! the `UiHandler`, which may call back into `Ui` (present a frame, change
//! the cursor, open the menu). Other threads reach the UI thread only through
//! `UiWaker` plus their own channel.
//!
//! Bar windows are layered (`WS_EX_LAYERED | WS_EX_TOOLWINDOW |
//! WS_EX_NOACTIVATE | WS_EX_TOPMOST`), presented with
//! `UpdateLayeredWindowIndirect` and a dirty rectangle from a top-down 32-bit
//! DIB section the renderer draws into directly. They never take focus and
//! have no taskbar button.
//!
//! Broadcasts (`TaskbarCreated`, `WM_SETTINGCHANGE`, `WM_DISPLAYCHANGE`,
//! `WM_POWERBROADCAST`, `WM_TIMECHANGE`) reach only top-level windows, so the
//! hidden window that receives them and the wake messages is a top-level
//! `WS_POPUP` with `WS_EX_TOOLWINDOW` that is never shown, not a
//! message-only (`HWND_MESSAGE`) window. Tests check its parent is the
//! desktop window; they never broadcast.
//!
//! The menu is never opened by this crate on its own: the binary decides
//! (a right click nothing else handled) and calls `Ui::show_menu`.
//! `TrackPopupMenu` runs a modal loop: frames and wakes wait until the menu
//! closes (a user action, so acceptable).

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rsb_core::config::{FullscreenMode, Position};
use rsb_core::geom::RectPx;
use rsb_core::hit::{Button, PointerEvent, PointerKind};
use rsb_core::pixels::PixelsMut;
use rsb_core::scene::BarGeometry;
use rsb_core::style::{Backdrop, Cursor};
use rsb_core::wake::Wake;
use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DWM_SYSTEMBACKDROP_TYPE, DWMSBT_MAINWINDOW, DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DwmExtendFrameIntoClientArea,
    DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION};
use windows::Win32::UI::Controls::{MARGINS, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, DispatchMessageW, HWND_BOTTOM, HWND_TOPMOST, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx,
    PM_REMOVE, PeekMessageW, QS_ALLINPUT, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SetWindowPos, ShowWindow, TranslateMessage, ULW_ALPHA, UPDATELAYEREDWINDOWINFO,
    UpdateLayeredWindowIndirect, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
    WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_POWERBROADCAST, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SETTINGCHANGE, WM_TIMECHANGE,
};

mod appbar;
mod bar;
mod dib;
mod fullscreen;
mod menu;
mod monitor;
pub mod probe;
mod shared;
mod shell;
mod sys;
#[cfg(test)]
mod tests;
mod wnd;

pub use fullscreen::spawn_fullscreen_test_window;

/// Gives back the screen space of every bar this process reserved, from any
/// thread, without the window loop. For the panic hook, which aborts, and for a console close that the
/// window loop is too busy to answer.
pub fn remove_all_appbars() {
    appbar::remove_all();
}

use bar::{BarState, Metrics, window_in_grant};
use dib::Dib;
use shared::{Raw, Shared, WinState};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WinError {
    /// A Win32 call failed: the call's name and the system message.
    Call {
        call: &'static str,
        message: String,
    },
    NoSuchBar(BarId),
    NoSuchMonitor(String),
}

impl std::fmt::Display for WinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WinError::Call { call, message } => write!(f, "{call}: {message}"),
            WinError::NoSuchBar(id) => write!(f, "no bar window {}", id.0),
            WinError::NoSuchMonitor(m) => write!(f, "no monitor {m}"),
        }
    }
}

impl std::error::Error for WinError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BarId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonitorInfo {
    /// Windows' enumeration order (`EnumDisplayMonitors`).
    pub index: u32,
    /// HMONITOR value (stable while the monitor stays connected).
    pub handle: u64,
    /// `\\.\DISPLAY1`
    pub device: String,
    pub primary: bool,
    /// Physical px, virtual-screen coordinates.
    pub rect: RectPx,
    pub work: RectPx,
    pub dpi: u32,
}

/// Where and how a bar window sits. Sizes are logical px; the window layer
/// scales them by the monitor's DPI and reports the result as
/// `UiEvent::Geometry`.
#[derive(Clone, Debug, PartialEq)]
pub struct BarWindowSpec {
    /// Bar name, for logs.
    pub name: String,
    /// `MonitorInfo::device`.
    pub monitor: String,
    pub position: Position,
    pub height: u32,
    pub floating: bool,
    pub margin: u32,
    /// Register an appbar (work area shrinks by the bar's height, plus the
    /// margin when floating).
    pub reserve: bool,
    pub fullscreen: FullscreenMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    Reload,
    OpenConfig,
    OpenLog,
    Quit,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    /// Another thread called `UiWaker::wake`: drain your channels.
    Wake,
    /// The deadline set with `set_frame_deadline` passed.
    FrameDue,
    Pointer {
        bar: BarId,
        event: PointerEvent,
    },
    /// A bar's surface size or scale is new (after create, DPI change,
    /// monitor change). Draw a full frame.
    Geometry {
        bar: BarId,
        geometry: BarGeometry,
    },
    /// Monitors were added, removed or changed (`WM_DISPLAYCHANGE`, debounced
    /// 250 ms). Rebuild bars from the selectors.
    MonitorsChanged,
    /// Windows' app theme changed (`WM_SETTINGCHANGE` "ImmersiveColorSet").
    ThemeChanged {
        dark: bool,
    },
    /// A fullscreen app came to the front of this bar's monitor or left.
    /// The window layer already applied the bar's `FullscreenMode`.
    Fullscreen {
        bar: BarId,
        active: bool,
    },
    /// Explorer restarted; appbars were registered again already.
    TaskbarCreated,
    /// The menu opened by `show_menu` closed with this item picked (nothing
    /// is sent when it was dismissed).
    Menu {
        bar: BarId,
        item: MenuItem,
    },
    /// The wall clock jumped: resume from sleep (`WM_POWERBROADCAST`
    /// `PBT_APMRESUMEAUTOMATIC`) or a time or time zone change
    /// (`WM_TIMECHANGE`). Forward as `ScriptIn::ClockChanged`.
    ClockChanged,
    /// WM_CLOSE / WM_ENDSESSION: shut down.
    CloseRequested,
}

pub trait UiHandler {
    fn handle(&mut self, ui: &mut Ui, event: UiEvent);
}

/// Events the test flags inject. Each is posted as the real message to a
/// window this process created, so the same code path runs.
#[derive(Clone, Debug, PartialEq)]
pub enum TestInput {
    TaskbarCreated,
    /// `WM_DPICHANGED` with this DPI and the rect Windows would suggest.
    Dpi {
        bar: BarId,
        dpi: u32,
    },
    DisplayChange,
    Theme {
        dark: bool,
    },
    /// The appbar's `ABN_FULLSCREENAPP` callback.
    FullscreenNotice {
        bar: BarId,
        active: bool,
    },
    /// Mouse messages (move, buttons, wheel, leave) to the bar window.
    Pointer {
        bar: BarId,
        event: PointerEvent,
    },
    /// `WM_TIMECHANGE` to the hidden window.
    TimeChange,
    /// `WM_POWERBROADCAST` / `PBT_APMRESUMEAUTOMATIC` to the hidden window.
    Resume,
}

/// Cheap, cloneable, `Send` handle that wakes the UI thread. Many wakes
/// before the loop runs collapse into one `UiEvent::Wake`: an `AtomicBool`
/// set by `wake` (posting only when it was clear) and cleared by the loop
/// before it hands out `Wake`, so a wake that lands while the handler drains
/// its channels is never lost.
#[derive(Clone)]
pub struct UiWaker {
    inner: Arc<shared::WakeInner>,
}

impl Wake for UiWaker {
    fn wake(&self) {
        self.inner.wake(wnd::WM_WAKE);
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UiOptions {
    /// Run `probe::UiProbe` from the start.
    pub probe: bool,
}

pub struct Ui {
    shared: Rc<Shared>,
    hidden: windows::Win32::Foundation::HWND,
    bars: HashMap<BarId, BarState>,
    next_bar: u32,
    /// Events produced by `Ui`'s own calls and by translating `Raw` ones,
    /// handed to the handler in order.
    pending: VecDeque<UiEvent>,
    dark: bool,
    offscreen: bool,
    quit: bool,
    running: bool,
    loop_wakes: u64,
    probe: Option<probe::UiProbe>,
    // Not Send: holds HWNDs and runs on its creating thread.
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Ui {
    /// Sets Per-Monitor-V2 DPI awareness, registers the window classes and
    /// creates the hidden top-level window used for wakes, `TaskbarCreated`,
    /// display, theme, power and time changes.
    pub fn new(options: UiOptions) -> Result<Ui, WinError> {
        // SAFETY: no pointers; failure only means awareness was already set
        // (by a manifest or an earlier call), which is what we want anyway.
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        wnd::register_classes()?;
        let shared = Rc::new(Shared::new());
        let hidden = wnd::create_hidden(Rc::as_ptr(&shared))?;
        let raw = sys::raw(hidden);
        shared.hidden.set(raw);
        shared.taskbar_created.set(wnd::taskbar_created_message());
        shared.wake.hwnd.store(raw, std::sync::atomic::Ordering::Release);
        let probe = options.probe.then(|| probe::UiProbe::start(raw));
        Ok(Ui {
            shared,
            hidden,
            bars: HashMap::new(),
            next_bar: 1,
            pending: VecDeque::new(),
            dark: sys::read_apps_dark(),
            offscreen: false,
            quit: false,
            running: false,
            loop_wakes: 0,
            probe,
            _not_send: std::marker::PhantomData,
        })
    }

    pub fn waker(&self) -> UiWaker {
        UiWaker { inner: Arc::clone(&self.shared.wake) }
    }

    pub fn monitors(&self) -> Vec<MonitorInfo> {
        monitor::enumerate()
    }

    /// Windows' "app mode" is dark.
    pub fn system_dark(&self) -> bool {
        self.dark
    }

    /// Creates, places and (when `reserve`) registers the bar; the window
    /// stays hidden until the first `present`. A `Geometry` event follows.
    pub fn create_bar(&mut self, spec: &BarWindowSpec) -> Result<BarId, WinError> {
        let mon = monitor::find(&spec.monitor).ok_or_else(|| WinError::NoSuchMonitor(spec.monitor.clone()))?;
        let dpi = mon.dpi.clamp(48, 960);
        let m = Metrics::new(spec, dpi);
        let grant = edge_strip(spec, &mon.rect, m);
        let win = window_in_grant(spec, m, grant);
        let hwnd = wnd::create_bar(Rc::as_ptr(&self.shared), win.x0, win.y0, win.width(), win.height())?;
        let dib = match Dib::new(win.width() as u32, win.height() as u32) {
            Ok(d) => d,
            Err(e) => {
                // SAFETY: the window was just created by this thread.
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                return Err(e);
            }
        };
        let id = BarId(self.next_bar);
        self.next_bar += 1;
        let key = sys::raw(hwnd);
        self.shared.wins.borrow_mut().insert(
            key,
            WinState {
                bar: id,
                cursor: Cursor::Default,
                tracking: false,
                hovered: false,
                last: (0.0, 0.0),
                captured: 0,
            },
        );
        let mut state = BarState {
            hwnd,
            spec: spec.clone(),
            dpi,
            monitor_dpi: mon.dpi,
            scale: dpi as f32 / 96.0,
            window: win,
            dib,
            needs_full: true,
            presented: false,
            want_visible: true,
            shown: false,
            fs_active: false,
            last_grant: None,
            setpos_calls: 0,
            registrations: 0,
            recent_setpos: VecDeque::new(),
            loop_warned: false,
            backdrop: Backdrop::None,
        };
        disable_dwm_rounding(&state);
        if spec.reserve && !self.offscreen {
            if appbar::register(key) {
                self.shared.registered.borrow_mut().insert(key);
                state.registrations += 1;
            } else {
                log::warn!("bar {}: the shell refused the appbar registration", spec.name);
            }
        }
        self.bars.insert(id, state);
        if let Err(e) = self.place(id, true) {
            self.destroy_bar(id);
            return Err(e);
        }
        if let Some(geometry) = self.geometry(id) {
            self.pending.push_back(UiEvent::Geometry { bar: id, geometry });
        }
        Ok(id)
    }

    /// Unregisters the appbar (work area restored exactly) and destroys it.
    pub fn destroy_bar(&mut self, bar: BarId) {
        let Some(state) = self.bars.remove(&bar) else { return };
        let key = sys::raw(state.hwnd);
        if self.shared.registered.borrow_mut().remove(&key) {
            appbar::remove(key);
        }
        // SAFETY: the window belongs to this thread.
        unsafe {
            let _ = DestroyWindow(state.hwnd);
        }
        self.shared.wins.borrow_mut().remove(&key);
        // The DIB goes with `state`, after the window that used it.
    }

    pub fn geometry(&self, bar: BarId) -> Option<BarGeometry> {
        let b = self.bars.get(&bar)?;
        Some(BarGeometry { width_px: b.dib.width, height_px: b.dib.height, scale: b.scale })
    }

    /// Lets `draw` write into the bar's DIB section and presents the
    /// rectangle it returns (None = nothing to present). Shows the window on
    /// its first present.
    pub fn present(
        &mut self,
        bar: BarId,
        draw: &mut dyn FnMut(&mut PixelsMut<'_>) -> Option<RectPx>,
    ) -> Result<(), WinError> {
        let b = self.bars.get_mut(&bar).ok_or(WinError::NoSuchBar(bar))?;
        let (w, h) = (b.dib.width as i32, b.dib.height as i32);
        let dirty = {
            let mut px = b.dib.pixels();
            draw(&mut px)
        };
        let Some(dirty) = dirty.and_then(|r| r.intersect(&RectPx::new(0, 0, w, h))) else { return Ok(()) };
        // The first update, and the first after a resize, must cover the whole
        // window: the layered surface has nothing valid to keep.
        let whole = b.needs_full || dirty == RectPx::new(0, 0, w, h);
        let size = windows::Win32::Foundation::SIZE { cx: w, cy: h };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let rc = RECT { left: dirty.x0, top: dirty.y0, right: dirty.x1, bottom: dirty.y1 };
        let info = UPDATELAYEREDWINDOWINFO {
            cbSize: size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
            hdcDst: Default::default(),
            pptDst: std::ptr::null(),
            psize: &size,
            hdcSrc: b.dib.dc,
            pptSrc: &src,
            crKey: Default::default(),
            pblend: &blend,
            dwFlags: ULW_ALPHA,
            prcDirty: if whole { std::ptr::null() } else { &rc },
        };
        // SAFETY: every pointer in `info` refers to a local that outlives the
        // call, and the source DC holds the DIB section drawn into above.
        if !unsafe { UpdateLayeredWindowIndirect(b.hwnd, &info) }.as_bool() {
            return Err(sys::last_error("UpdateLayeredWindowIndirect"));
        }
        b.needs_full = false;
        b.presented = true;
        self.apply_visibility(bar);
        Ok(())
    }

    /// Opens the native menu (Reload, Open config folder, Open log, Quit) at
    /// bar-relative physical px. Our own bar window is made foreground for
    /// the menu's lifetime only, so a click elsewhere closes it. The pick
    /// arrives as `UiEvent::Menu`.
    pub fn show_menu(&mut self, bar: BarId, x: i32, y: i32) {
        let Some(b) = self.bars.get(&bar) else { return };
        let at = POINT { x: b.window.x0 + x, y: b.window.y0 + y };
        let above = b.spec.position == Position::Bottom;
        if let Some(item) = menu::track(b.hwnd, at, above) {
            self.pending.push_back(UiEvent::Menu { bar, item });
        }
    }

    pub fn set_cursor(&mut self, bar: BarId, cursor: Cursor) {
        let Some(b) = self.bars.get(&bar) else { return };
        let hovered = {
            let mut wins = self.shared.wins.borrow_mut();
            let Some(w) = wins.get_mut(&sys::raw(b.hwnd)) else { return };
            w.cursor = cursor;
            w.hovered
        };
        // WM_SETCURSOR only comes with mouse movement: apply it now when the
        // pointer is already over the bar.
        if hovered {
            wnd::apply_cursor(cursor);
        }
    }

    /// `DWMWA_SYSTEMBACKDROP_TYPE` (Windows 11 22621+; ignored before).
    pub fn set_backdrop(&mut self, bar: BarId, backdrop: Backdrop) {
        let dark = self.dark;
        let Some(b) = self.bars.get_mut(&bar) else { return };
        b.backdrop = backdrop;
        apply_backdrop(b, dark);
    }

    /// Hide or show (`rsb toggle`). A hidden bar keeps its appbar
    /// registration only while shown.
    pub fn set_visible(&mut self, bar: BarId, visible: bool) {
        let offscreen = self.offscreen;
        let Some(b) = self.bars.get_mut(&bar) else { return };
        b.want_visible = visible;
        let key = sys::raw(b.hwnd);
        let reserve = b.spec.reserve && !offscreen;
        if !visible {
            if self.shared.registered.borrow_mut().remove(&key) {
                appbar::remove(key);
            }
        } else if reserve && !self.shared.is_registered(key) {
            if appbar::register(key) {
                self.shared.registered.borrow_mut().insert(key);
                b.registrations += 1;
            }
            if let Err(e) = self.place(bar, true) {
                log::warn!("bar placement failed: {e}");
            }
        }
        self.apply_visibility(bar);
    }

    pub fn is_visible(&self, bar: BarId) -> bool {
        self.bars.get(&bar).is_some_and(|b| b.want_visible)
    }

    /// One `FrameDue` at or after `at`; None cancels. Timer resolution is
    /// the system's (about 15.6 ms unless raised by another process).
    pub fn set_frame_deadline(&mut self, at: Option<Instant>) {
        self.shared.deadline.set(at);
        let hidden = self.shared.hidden.get();
        match at {
            None => sys::kill_timer(hidden, wnd::TIMER_FRAME),
            Some(t) => wnd::arm_frame_timer(hidden, t),
        }
    }

    /// Opens a file or folder with the shell on a short-lived worker thread
    /// (ShellExecute can block on other processes).
    pub fn open_path(&self, path: &Path) {
        shell::open_on_worker(path);
    }

    /// Posts a test input to our own windows (hidden test flags).
    pub fn inject(&self, input: TestInput) {
        let hidden = self.shared.hidden.get();
        let sent = match input {
            TestInput::TaskbarCreated => sys::post(hidden, self.shared.taskbar_created.get(), 0, 0),
            TestInput::Dpi { bar, dpi } => self.bars.get(&bar).is_some_and(|b| {
                // Windows refuses to post this message (it normally carries a
                // rectangle pointer), so it is sent to our own window. The
                // suggested rectangle is left out: the handler ignores it.
                sys::send(sys::raw(b.hwnd), WM_DPICHANGED, (dpi as usize) << 16 | dpi as usize, 0);
                true
            }),
            TestInput::DisplayChange => {
                let (w, h) = self
                    .monitors()
                    .iter()
                    .find(|m| m.primary)
                    .map(|m| (m.rect.width(), m.rect.height()))
                    .unwrap_or((1920, 1080));
                sys::post(hidden, WM_DISPLAYCHANGE, 32, pack(w, h))
            }
            TestInput::Theme { dark } => {
                self.shared.theme_override.set(Some(dark));
                // The message carries a string pointer, so it cannot be posted.
                sys::send(hidden, WM_SETTINGCHANGE, 0, self.shared.immersive.as_ptr() as isize);
                true
            }
            TestInput::FullscreenNotice { bar, active } => self.bars.get(&bar).is_some_and(|b| {
                sys::post(sys::raw(b.hwnd), appbar::CALLBACK_MSG, appbar::ABN_FULLSCREENAPP, active as isize)
            }),
            TestInput::Pointer { bar, event } => self.bars.get(&bar).is_some_and(|b| inject_pointer(b, event)),
            TestInput::TimeChange => sys::post(hidden, WM_TIMECHANGE, 0, 0),
            TestInput::Resume => sys::post(hidden, WM_POWERBROADCAST, 0x12, 0),
        };
        if !sent {
            log::warn!("test input was not delivered: {input:?}");
        }
    }

    /// Probe numbers so far (None without `UiOptions::probe`).
    pub fn probe_stats(&self) -> Option<probe::ProbeStats> {
        self.probe.as_ref().map(|p| p.stats())
    }

    /// The monitor's work area now, for checks.
    pub fn work_area(&self, monitor: &str) -> Option<RectPx> {
        monitor::find(monitor).map(|m| m.work)
    }

    /// Runs the message loop until `quit`. Returns after every bar was
    /// destroyed and every appbar removed. `WM_ENDSESSION` removes the
    /// appbars before it returns, then raises `CloseRequested`.
    pub fn run(&mut self, handler: &mut dyn UiHandler) -> Result<(), WinError> {
        if self.running {
            return Err(WinError::Call { call: "Ui::run", message: "the message loop is already running".into() });
        }
        self.running = true;
        'outer: loop {
            if self.quit {
                break;
            }
            self.pump_messages();
            while let Some(event) = self.next_event() {
                handler.handle(self, event);
                if self.quit {
                    break 'outer;
                }
            }
            if self.quit {
                break;
            }
            // SAFETY: no handles; waits for any queued message and nothing else.
            unsafe {
                MsgWaitForMultipleObjectsEx(None, u32::MAX, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            }
            self.loop_wakes += 1;
        }
        self.destroy_all();
        // Let the destroy messages run before the caller moves on.
        self.pump_messages();
        self.shared.queue.borrow_mut().clear();
        self.pending.clear();
        self.quit = false;
        self.running = false;
        Ok(())
    }

    pub fn quit(&mut self) {
        self.quit = true;
    }

    /// How many times the loop woke from its wait (a message arrived), for
    /// wake-count budgets.
    pub fn loop_wakes(&self) -> u64 {
        self.loop_wakes
    }

    /// Test runs on the owner's PC: place every bar window beyond the right
    /// edge of the virtual screen and never register an appbar, so nothing
    /// covers the desktop and no work area moves. Call before `create_bar`.
    pub fn set_offscreen(&mut self, on: bool) {
        self.offscreen = on;
    }

    /// Whether the bar's window currently holds an appbar registration.
    pub fn is_reserved(&self, bar: BarId) -> bool {
        self.bars.get(&bar).is_some_and(|b| self.shared.is_registered(sys::raw(b.hwnd)))
    }

    /// The bar window's rectangle (physical px, virtual-screen coordinates).
    pub fn window_rect(&self, bar: BarId) -> Option<RectPx> {
        self.bars.get(&bar).map(|b| b.window)
    }

    /// Times the shell was told `ABM_NEW` for this bar (more than one after a
    /// `TaskbarCreated`).
    pub fn appbar_registrations(&self, bar: BarId) -> u32 {
        self.bars.get(&bar).map_or(0, |b| b.registrations)
    }

    /// Times the bar sent `ABM_SETPOS`.
    pub fn appbar_setpos_calls(&self, bar: BarId) -> u32 {
        self.bars.get(&bar).map_or(0, |b| b.setpos_calls)
    }

    // ---- loop internals ----

    fn pump_messages(&mut self) {
        let mut msg = MSG::default();
        // SAFETY: `msg` is a valid out pointer; messages go to window
        // procedures of this crate.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    self.quit = true;
                    continue;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    fn next_event(&mut self) -> Option<UiEvent> {
        loop {
            if let Some(e) = self.pending.pop_front() {
                return Some(e);
            }
            if let Some(raw) = self.shared.pop() {
                self.translate(raw);
                continue;
            }
            if self.shared.wake_pending.get() {
                self.shared.wake_pending.set(false);
                // Cleared before the event goes out, so a wake that arrives
                // while the handler drains its channels posts a new message.
                self.shared.wake.posted.store(false, std::sync::atomic::Ordering::Release);
                return Some(UiEvent::Wake);
            }
            return None;
        }
    }

    fn translate(&mut self, raw: Raw) {
        match raw {
            Raw::Pointer { bar, event } => self.pending.push_back(UiEvent::Pointer { bar, event }),
            Raw::FrameDue => self.pending.push_back(UiEvent::FrameDue),
            Raw::Clock => self.pending.push_back(UiEvent::ClockChanged),
            Raw::Close | Raw::EndSession => self.pending.push_back(UiEvent::CloseRequested),
            Raw::Theme { dark } => {
                self.dark = dark;
                for b in self.bars.values() {
                    if b.backdrop != Backdrop::None {
                        apply_backdrop(b, dark);
                    }
                }
                self.pending.push_back(UiEvent::ThemeChanged { dark });
            }
            Raw::Fullscreen { bar, active } => {
                let Some(b) = self.bars.get_mut(&bar) else { return };
                if b.fs_active == active {
                    return;
                }
                b.fs_active = active;
                match b.spec.fullscreen {
                    FullscreenMode::Below => {
                        // Losing topmost puts the bar under the fullscreen window.
                        let after = if active { HWND_BOTTOM } else { HWND_TOPMOST };
                        // SAFETY: the window is ours.
                        unsafe {
                            let _ =
                                SetWindowPos(b.hwnd, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                        }
                    }
                    FullscreenMode::Hide => self.apply_visibility(bar),
                    FullscreenMode::Stay => {}
                }
                self.pending.push_back(UiEvent::Fullscreen { bar, active });
            }
            Raw::PosChanged { bar } => {
                if !self.bars.get(&bar).is_some_and(|b| self.shared.is_registered(sys::raw(b.hwnd))) {
                    return;
                }
                self.replace(bar, false);
            }
            Raw::Dpi { bar, dpi } => {
                let Some(b) = self.bars.get_mut(&bar) else { return };
                b.dpi = dpi.clamp(48, 960);
                self.replace(bar, true);
            }
            Raw::Display => {
                let ids: Vec<BarId> = self.bars.keys().copied().collect();
                for id in ids {
                    if let Some(b) = self.bars.get_mut(&id)
                        && let Some(m) = monitor::find(&b.spec.monitor)
                        && m.dpi != b.monitor_dpi
                    {
                        b.monitor_dpi = m.dpi;
                        b.dpi = m.dpi.clamp(48, 960);
                    }
                    self.replace(id, true);
                }
                self.pending.push_back(UiEvent::MonitorsChanged);
            }
            Raw::TaskbarCreated => {
                let ids: Vec<BarId> = self.bars.keys().copied().collect();
                for id in ids {
                    let offscreen = self.offscreen;
                    let Some(b) = self.bars.get_mut(&id) else { continue };
                    let key = sys::raw(b.hwnd);
                    // A bar whose first registration was refused (the shell
                    // was not up yet at logon) gets its chance now.
                    let wanted = b.spec.reserve && b.want_visible && !offscreen;
                    if !self.shared.is_registered(key) && !wanted {
                        continue;
                    }
                    // The new shell has an empty appbar list. An injected
                    // notice finds the old registration still in place, so it
                    // is removed first: the shell refuses a second ABM_NEW.
                    if self.shared.is_registered(key) {
                        appbar::remove(key);
                        self.shared.registered.borrow_mut().remove(&key);
                    }
                    if appbar::register(key) {
                        self.shared.registered.borrow_mut().insert(key);
                        b.registrations += 1;
                        b.last_grant = None;
                    } else {
                        log::warn!("bar {}: could not register again after the shell restarted", b.spec.name);
                    }
                    self.replace(id, true);
                }
                self.pending.push_back(UiEvent::TaskbarCreated);
            }
        }
    }

    /// Re-places a bar and reports new geometry if its surface changed.
    fn replace(&mut self, bar: BarId, force: bool) {
        match self.place(bar, force) {
            Ok(true) => {
                if let Some(geometry) = self.geometry(bar) {
                    self.pending.push_back(UiEvent::Geometry { bar, geometry });
                }
            }
            Ok(false) => {}
            Err(e) => log::warn!("bar placement failed: {e}"),
        }
    }

    /// Asks the shell for the bar's rectangle (when registered), claims it,
    /// moves the window there and swaps the surface when its size changed.
    /// Returns whether the surface size or scale changed.
    fn place(&mut self, bar: BarId, force: bool) -> Result<bool, WinError> {
        let shared = &self.shared;
        let offscreen_x =
            self.offscreen.then(|| monitor::enumerate().iter().map(|m| m.rect.x1).max().unwrap_or(0) + 200);
        let b = self.bars.get_mut(&bar).ok_or(WinError::NoSuchBar(bar))?;
        let mon = monitor::find(&b.spec.monitor).ok_or_else(|| WinError::NoSuchMonitor(b.spec.monitor.clone()))?;
        let m = Metrics::new(&b.spec, b.dpi);
        let key = sys::raw(b.hwnd);
        let mut grant = edge_strip(&b.spec, &mon.rect, m);
        if shared.is_registered(key) {
            let (edge, reserved) = (edge_of(&b.spec), m.reserved());
            let mut q =
                appbar::query(key, edge, RECT { left: grant.x0, top: grant.y0, right: grant.x1, bottom: grant.y1 });
            // The shell adjusts the side facing the other appbars; the depth
            // stays ours.
            match b.spec.position {
                Position::Top => q.bottom = q.top + reserved,
                Position::Bottom => q.top = q.bottom - reserved,
            }
            grant = RectPx::new(q.left, q.top, q.right, q.bottom);
            // The shell reports our own SETPOS back as ABN_POSCHANGED: an
            // unchanged rectangle is not submitted again, or the two would
            // answer each other forever.
            if !force && b.last_grant == Some(q) {
                return Ok(false);
            }
            let now = Instant::now();
            b.recent_setpos.retain(|t| now.duration_since(*t) < Duration::from_secs(2));
            if !force && b.recent_setpos.len() >= 30 {
                if !b.loop_warned {
                    b.loop_warned = true;
                    log::warn!("bar {}: appbar position keeps changing; ignoring notices for now", b.spec.name);
                }
                return Ok(false);
            }
            b.recent_setpos.push_back(now);
            appbar::set_pos(key, edge, q);
            b.last_grant = Some(q);
            b.setpos_calls += 1;
        } else {
            b.last_grant = None;
        }
        let mut win = window_in_grant(&b.spec, m, grant);
        if let Some(off) = offscreen_x {
            let dx = off - mon.rect.x0;
            win = RectPx::new(win.x0 + dx, win.y0, win.x1 + dx, win.y1);
        }
        // SAFETY: the window is ours.
        unsafe {
            let _ =
                SetWindowPos(b.hwnd, None, win.x0, win.y0, win.width(), win.height(), SWP_NOZORDER | SWP_NOACTIVATE);
        }
        b.window = win;
        let scale = b.dpi as f32 / 96.0;
        let resized = win.width() as u32 != b.dib.width || win.height() as u32 != b.dib.height;
        let changed = resized || scale != b.scale;
        if resized {
            b.dib = Dib::new(win.width() as u32, win.height() as u32)?;
        }
        if changed {
            b.needs_full = true;
        }
        b.scale = scale;
        Ok(changed)
    }

    /// Shows or hides the window from the user's toggle, the first present and
    /// a fullscreen app in `Hide` mode.
    fn apply_visibility(&mut self, bar: BarId) {
        let Some(b) = self.bars.get_mut(&bar) else { return };
        let want = b.want_visible && b.presented && !(b.fs_active && b.spec.fullscreen == FullscreenMode::Hide);
        if want != b.shown {
            // SAFETY: the window is ours.
            unsafe {
                let _ = ShowWindow(b.hwnd, if want { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
            b.shown = want;
        }
    }

    fn destroy_all(&mut self) {
        let ids: Vec<BarId> = self.bars.keys().copied().collect();
        for id in ids {
            self.destroy_bar(id);
        }
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        // The probe sends to the hidden window: stop it first.
        self.probe = None;
        self.destroy_all();
        self.shared.wake.hwnd.store(0, std::sync::atomic::Ordering::Release);
        sys::kill_timer(sys::raw(self.hidden), wnd::TIMER_FRAME);
        // SAFETY: the window belongs to this thread.
        unsafe {
            let _ = DestroyWindow(self.hidden);
        }
    }
}

/// The strip a bar claims at its monitor's edge before the shell is asked:
/// full width, bar height (plus the margin when floating).
fn edge_strip(spec: &BarWindowSpec, mon: &RectPx, m: Metrics) -> RectPx {
    match spec.position {
        Position::Top => RectPx::new(mon.x0, mon.y0, mon.x1, mon.y0 + m.reserved()),
        Position::Bottom => RectPx::new(mon.x0, mon.y1 - m.reserved(), mon.x1, mon.y1),
    }
}

fn edge_of(spec: &BarWindowSpec) -> u32 {
    match spec.position {
        Position::Top => appbar::ABE_TOP,
        Position::Bottom => appbar::ABE_BOTTOM,
    }
}

fn pack(lo: i32, hi: i32) -> isize {
    ((hi as i16 as u16 as isize) << 16) | (lo as i16 as u16 as isize)
}

/// The real mouse message for `event`, posted to the bar window and marked as
/// injected (see `wnd::INJECTED`).
fn inject_pointer(b: &BarState, event: PointerEvent) -> bool {
    let (x, y) = (event.x.round() as i32, event.y.round() as i32);
    let hwnd = sys::raw(b.hwnd);
    let down = |button: Button| match button {
        Button::Left => WM_LBUTTONDOWN,
        Button::Right => WM_RBUTTONDOWN,
        Button::Middle => WM_MBUTTONDOWN,
    };
    let up = |button: Button| match button {
        Button::Left => WM_LBUTTONUP,
        Button::Right => WM_RBUTTONUP,
        Button::Middle => WM_MBUTTONUP,
    };
    match event.kind {
        PointerKind::Move => sys::post(hwnd, WM_MOUSEMOVE, wnd::INJECTED, pack(x, y)),
        PointerKind::Leave => sys::post(hwnd, WM_MOUSELEAVE, 0, 0),
        PointerKind::Down(button) => sys::post(hwnd, down(button), wnd::INJECTED, pack(x, y)),
        PointerKind::Up(button) => sys::post(hwnd, up(button), wnd::INJECTED, pack(x, y)),
        PointerKind::Wheel(notches) => {
            let delta = (notches * 120.0).round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            // The wheel message carries screen coordinates.
            let screen = pack(b.window.x0 + x, b.window.y0 + y);
            sys::post(hwnd, WM_MOUSEWHEEL, ((delta as u16 as usize) << 16) | wnd::INJECTED, screen)
        }
    }
}

fn disable_dwm_rounding(b: &BarState) {
    // The renderer draws its own corner radius; DWM rounding would clip a
    // second time.
    let pref = DWMWCP_DONOTROUND;
    // SAFETY: the attribute pointer refers to a live local of the size passed.
    let r = unsafe {
        DwmSetWindowAttribute(b.hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &pref as *const _ as *const std::ffi::c_void, 4)
    };
    if let Err(e) = r {
        log::debug!("corner preference not set: {e}");
    }
}

fn apply_backdrop(b: &BarState, dark: bool) {
    let kind: DWM_SYSTEMBACKDROP_TYPE = match b.backdrop {
        Backdrop::None => DWMSBT_NONE,
        Backdrop::Acrylic => DWMSBT_TRANSIENTWINDOW,
        Backdrop::Mica => DWMSBT_MAINWINDOW,
    };
    let margins = if b.backdrop == Backdrop::None {
        MARGINS::default()
    } else {
        // -1 extends the frame over the whole client area, which is what lets
        // the backdrop show through the transparent pixels.
        MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 }
    };
    let dark_flag: i32 = dark as i32;
    // SAFETY: each attribute pointer refers to a live local of the size passed.
    unsafe {
        if let Err(e) = DwmExtendFrameIntoClientArea(b.hwnd, &margins) {
            log::debug!("backdrop frame not extended: {e}");
        }
        let _ = DwmSetWindowAttribute(
            b.hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark_flag as *const _ as *const std::ffi::c_void,
            4,
        );
        if let Err(e) =
            DwmSetWindowAttribute(b.hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &kind as *const _ as *const std::ffi::c_void, 4)
        {
            // Windows 10 and early Windows 11 builds do not know the attribute.
            log::debug!("system backdrop not set: {e}");
        }
    }
}

//! Tests for the window layer.
//!
//! The plain tests create hidden windows only: nothing is shown and no screen
//! space is registered. The `#[ignore]` ones show a window or register an
//! appbar and run one at a time through the desktop lock; they print the work
//! area before and after. One of them takes the foreground and only runs with
//! `RSB_TEST_FOREGROUND=1`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rsb_core::config::{FullscreenMode, Position};
use rsb_core::geom::RectPx;
use rsb_core::hit::{Button, PointerEvent, PointerKind};
use rsb_core::style::Cursor;
use rsb_core::wake::Wake;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GA_PARENT, GWL_EXSTYLE, GWL_STYLE, GetAncestor, GetCursor, GetDesktopWindow, GetForegroundWindow,
    GetWindowLongPtrW, IDC_ARROW, IDC_HAND, IsWindow, IsWindowVisible, LoadCursorW, MA_NOACTIVATE, SendMessageW,
    WM_ENDSESSION, WM_MOUSEACTIVATE, WM_SETCURSOR, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

use crate::bar::Metrics;
use crate::{BarId, BarWindowSpec, MonitorInfo, TestInput, Ui, UiEvent, UiHandler, UiOptions, monitor, sys};

mod popups;

// ---- helpers ----

/// "rdp" in a Remote Desktop session, else "console": the label on every timing a test prints.
fn session_name() -> &'static str {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_REMOTESESSION};
    // SAFETY: a plain query with no pointers.
    if unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0 { "rdp" } else { "console" }
}

struct Driver<F> {
    f: F,
    events: Vec<UiEvent>,
    deadline: Instant,
    timed_out: bool,
}

impl<F: FnMut(&mut Ui, &UiEvent) -> bool> UiHandler for Driver<F> {
    fn handle(&mut self, ui: &mut Ui, event: UiEvent) {
        let is_wake = matches!(event, UiEvent::Wake);
        let stop = (self.f)(ui, &event);
        if !is_wake {
            self.events.push(event);
        }
        if stop {
            ui.quit();
        } else if is_wake && Instant::now() >= self.deadline {
            self.timed_out = true;
            ui.quit();
        }
    }
}

/// Runs the loop's own steps (pump messages, hand out events) until `f`
/// returns true or `limit` passes, without `run`, so bars are still there
/// afterwards. `f` also gets a `Wake` as a tick about every 15 ms; ticks are not
/// recorded. Returns the recorded events and whether the limit was hit.
fn drive(ui: &mut Ui, limit: Duration, mut f: impl FnMut(&mut Ui, &UiEvent) -> bool) -> (Vec<UiEvent>, bool) {
    let end = Instant::now() + limit;
    let mut events = Vec::new();
    loop {
        ui.pump_messages();
        while let Some(e) = ui.next_event() {
            if matches!(e, UiEvent::Wake) {
                continue;
            }
            let stop = f(ui, &e);
            events.push(e);
            if stop {
                return (events, false);
            }
        }
        if f(ui, &UiEvent::Wake) {
            return (events, false);
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return (events, true);
        }
        let wait = left.min(Duration::from_millis(15)).as_millis().max(1) as u32;
        ui.wait(wait);
    }
}

/// Runs the real `Ui::run` until `f` returns true or `limit` passes (which
/// destroys every bar when it returns). A helper thread
/// wakes the loop every 15 ms, so `f` also runs on a steady beat (`Wake`
/// events are passed to `f` but not recorded). Returns the other events and
/// whether the limit was hit.
fn drive_run(ui: &mut Ui, limit: Duration, f: impl FnMut(&mut Ui, &UiEvent) -> bool) -> (Vec<UiEvent>, bool) {
    let stop = Arc::new(AtomicBool::new(false));
    let waker = ui.waker();
    let s2 = Arc::clone(&stop);
    let ticker = std::thread::spawn(move || {
        while !s2.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(15));
            waker.wake();
        }
    });
    let mut d = Driver { f, events: Vec::new(), deadline: Instant::now() + limit, timed_out: false };
    ui.run(&mut d).expect("run");
    stop.store(true, Ordering::Relaxed);
    ticker.join().expect("ticker");
    (d.events, d.timed_out)
}

fn primary(ui: &Ui) -> MonitorInfo {
    let all = ui.monitors();
    all.iter().find(|m| m.primary).or(all.first()).cloned().expect("a monitor")
}

fn spec(mon: &MonitorInfo) -> BarWindowSpec {
    BarWindowSpec {
        name: "test".into(),
        monitor: mon.device.clone(),
        position: Position::Top,
        height: 32,
        floating: false,
        margin: 8,
        reserve: false,
        fullscreen: FullscreenMode::Below,
    }
}

fn hwnd(ui: &Ui, bar: BarId) -> HWND {
    ui.bars[&bar].hwnd
}

fn ex_style(h: HWND) -> u32 {
    // SAFETY: reading a window's style bits.
    unsafe { GetWindowLongPtrW(h, GWL_EXSTYLE) as u32 }
}

fn visible(h: HWND) -> bool {
    // SAFETY: plain handle argument.
    unsafe { IsWindowVisible(h) }.as_bool()
}

fn geometries(events: &[UiEvent], bar: BarId) -> Vec<(u32, u32, f32)> {
    events
        .iter()
        .filter_map(|e| match e {
            UiEvent::Geometry { bar: b, geometry } if *b == bar => {
                Some((geometry.width_px, geometry.height_px, geometry.scale))
            }
            _ => None,
        })
        .collect()
}

fn count(events: &[UiEvent], want: &UiEvent) -> usize {
    events.iter().filter(|e| *e == want).count()
}

fn work(ui: &Ui, mon: &MonitorInfo) -> RectPx {
    ui.work_area(&mon.device).expect("work area")
}

/// Polls (answering messages) until the primary work area equals `want`.
fn wait_work(ui: &mut Ui, mon: &MonitorInfo, limit: Duration, want: impl Fn(RectPx) -> bool) -> RectPx {
    let t = Instant::now();
    let mut last = work(ui, mon);
    drive(ui, limit, |ui, _| {
        last = work(ui, mon);
        want(last) || t.elapsed() > limit
    });
    last
}

fn fill(px: &mut rsb_core::pixels::PixelsMut<'_>, color: [u8; 4]) {
    for p in px.data.as_chunks_mut::<4>().0 {
        *p = color;
    }
}

fn full(px: &rsb_core::pixels::PixelsMut<'_>) -> RectPx {
    RectPx::new(0, 0, px.width as i32, px.height as i32)
}

// ---- hidden window, wake, loop ----

#[test]
fn hidden_window_is_a_top_level_window_never_shown() {
    let ui = Ui::new(UiOptions::default()).unwrap();
    // SAFETY: plain handle arguments.
    unsafe {
        assert_eq!(
            GetAncestor(ui.hidden, GA_PARENT),
            GetDesktopWindow(),
            "parent is the desktop, not the message-only root"
        );
        assert!(!IsWindowVisible(ui.hidden).as_bool());
    }
    assert_ne!(ex_style(ui.hidden) & WS_EX_TOOLWINDOW.0, 0);
    assert_ne!(ui.shared.taskbar_created.get(), 0, "TaskbarCreated is registered");
}

#[test]
fn repeated_wakes_collapse_into_one_event() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let waker = ui.waker();
    for _ in 0..1000 {
        waker.wake();
    }
    let mut wakes = 0;
    let (_, timed_out) = drive_plain(&mut ui, Duration::from_millis(300), |ui, e| {
        if matches!(e, UiEvent::Wake) {
            wakes += 1;
            ui.set_frame_deadline(Some(Instant::now() + Duration::from_millis(100)));
        }
        matches!(e, UiEvent::FrameDue)
    });
    assert!(!timed_out);
    assert_eq!(wakes, 1, "1000 wakes before the loop ran");
}

/// Like `drive` but without the wake ticker, for tests that count wakes.
fn drive_plain(ui: &mut Ui, limit: Duration, f: impl FnMut(&mut Ui, &UiEvent) -> bool) -> (Vec<UiEvent>, bool) {
    struct Plain<F> {
        f: F,
        deadline: Instant,
        timed_out: bool,
    }
    impl<F: FnMut(&mut Ui, &UiEvent) -> bool> UiHandler for Plain<F> {
        fn handle(&mut self, ui: &mut Ui, event: UiEvent) {
            if (self.f)(ui, &event) {
                ui.quit();
            } else if Instant::now() >= self.deadline {
                self.timed_out = true;
                ui.quit();
            }
        }
    }
    // The deadline is checked on events; a frame timer guarantees one.
    ui.set_frame_deadline(Some(Instant::now() + limit + Duration::from_millis(50)));
    let mut p = Plain { f, deadline: Instant::now() + limit, timed_out: false };
    ui.run(&mut p).expect("run");
    (Vec::new(), p.timed_out)
}

#[test]
fn a_wake_during_the_drain_is_not_lost() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let waker = ui.waker();
    waker.wake();
    let mut wakes = 0;
    let w2 = waker.clone();
    let (_, timed_out) = drive_plain(&mut ui, Duration::from_millis(500), |_, e| {
        if matches!(e, UiEvent::Wake) {
            wakes += 1;
            if wakes == 1 {
                // A producer sending while the handler drains its channels.
                w2.wake();
            }
        }
        wakes == 2
    });
    assert!(!timed_out, "the second wake never arrived");
    assert_eq!(wakes, 2);
}

#[test]
fn many_threads_waking_never_lose_the_last_one() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let waker = ui.waker();
    let produced = Arc::new(AtomicU64::new(0));
    let threads = 4u64;
    let each = 5000u64;
    let total = threads * each;
    let hs: Vec<_> = (0..threads)
        .map(|_| {
            let (w, p) = (waker.clone(), Arc::clone(&produced));
            std::thread::spawn(move || {
                for _ in 0..each {
                    p.fetch_add(1, Ordering::SeqCst);
                    w.wake();
                }
            })
        })
        .collect();
    let mut seen = 0u64;
    let mut wakes = 0u64;
    let started = Instant::now();
    let (_, timed_out) = drive_plain(&mut ui, Duration::from_secs(10), |_, e| {
        if matches!(e, UiEvent::Wake) {
            wakes += 1;
            seen = produced.load(Ordering::SeqCst);
        }
        seen == total
    });
    for h in hs {
        h.join().unwrap();
    }
    assert!(!timed_out, "stuck at {seen} of {total}: a wake was lost");
    eprintln!("{total} wakes from {threads} threads gave {wakes} Wake events in {:?}", started.elapsed());
    assert!(wakes < total, "no collapsing at all");
}

#[test]
fn wake_does_not_block_and_survives_the_ui() {
    let ui = Ui::new(UiOptions::default()).unwrap();
    let waker = ui.waker();
    let t = Instant::now();
    for _ in 0..100_000 {
        waker.wake();
    }
    let took = t.elapsed();
    eprintln!("100000 wakes with no loop running: {took:?}");
    assert!(took < Duration::from_secs(1));
    drop(ui);
    waker.wake(); // the window is gone: a no-op, not a panic or a hang
}

#[test]
fn waker_is_send_and_sync() {
    fn check<T: Send + Sync + Clone + Wake>() {}
    check::<crate::UiWaker>();
}

#[test]
fn quit_before_run_ends_the_loop_at_once_and_the_loop_can_run_again() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    ui.quit();
    let (_, timed_out) = drive_plain(&mut ui, Duration::from_millis(200), |_, _| false);
    assert!(!timed_out);
    let (_, timed_out) = drive_plain(&mut ui, Duration::from_millis(100), |_, e| matches!(e, UiEvent::FrameDue));
    assert!(!timed_out, "second run");
}

// ---- frame deadline ----

#[test]
fn frame_deadline_fires_once_at_or_after_the_time() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let t0 = Instant::now();
    ui.set_frame_deadline(Some(t0 + Duration::from_millis(80)));
    let mut at = None;
    let (events, timed_out) = drive(&mut ui, Duration::from_millis(500), |_, e| {
        if matches!(e, UiEvent::FrameDue) {
            at = Some(t0.elapsed());
        }
        false
    });
    assert!(timed_out);
    assert_eq!(count(&events, &UiEvent::FrameDue), 1, "one-shot");
    let at = at.expect("FrameDue");
    eprintln!("frame deadline 80 ms fired at {at:?}");
    assert!(at >= Duration::from_millis(79), "early: {at:?}");
    assert!(at < Duration::from_millis(300), "late: {at:?}");
}

#[test]
fn frame_deadline_in_the_past_fires_promptly_and_cancel_stops_it() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    ui.set_frame_deadline(Some(Instant::now() - Duration::from_millis(5)));
    let (events, _) = drive(&mut ui, Duration::from_millis(150), |_, _| false);
    assert_eq!(count(&events, &UiEvent::FrameDue), 1);

    ui.set_frame_deadline(Some(Instant::now() + Duration::from_millis(60)));
    ui.set_frame_deadline(None);
    let (events, _) = drive(&mut ui, Duration::from_millis(250), |_, _| false);
    assert_eq!(count(&events, &UiEvent::FrameDue), 0, "cancelled");

    // A later deadline replaces an earlier one.
    ui.set_frame_deadline(Some(Instant::now() + Duration::from_millis(20)));
    ui.set_frame_deadline(Some(Instant::now() + Duration::from_millis(200)));
    let t = Instant::now();
    let mut at = Duration::ZERO;
    let (events, _) = drive(&mut ui, Duration::from_millis(450), |_, e| {
        if matches!(e, UiEvent::FrameDue) {
            at = t.elapsed();
        }
        false
    });
    assert_eq!(count(&events, &UiEvent::FrameDue), 1);
    assert!(at >= Duration::from_millis(150), "the first deadline was not replaced: {at:?}");
}

// ---- monitors, theme, clock, taskbar, display ----

#[test]
fn monitors_are_enumerated_consistently() {
    let ui = Ui::new(UiOptions::default()).unwrap();
    let ms = ui.monitors();
    assert!(!ms.is_empty());
    assert_eq!(ms.iter().filter(|m| m.primary).count(), 1, "exactly one primary");
    for (i, m) in ms.iter().enumerate() {
        assert_eq!(m.index as usize, i);
        assert!(!m.device.is_empty());
        assert!(m.rect.width() > 0 && m.rect.height() > 0);
        assert!(m.work.x0 >= m.rect.x0 && m.work.y0 >= m.rect.y0 && m.work.x1 <= m.rect.x1 && m.work.y1 <= m.rect.y1);
        assert!(m.dpi >= 96, "dpi {}", m.dpi);
        assert_eq!(ui.work_area(&m.device), Some(m.work));
        assert_eq!(monitor::find(&m.device.to_lowercase()).map(|f| f.handle), Some(m.handle));
    }
    assert_eq!(ui.work_area("\\\\.\\DISPLAY99"), None);
    eprintln!("monitors: {}", ms.len());
}

#[test]
fn theme_is_read_from_the_registry_and_injected_values_are_reported() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let real = sys::read_apps_dark();
    assert_eq!(ui.system_dark(), real);
    ui.inject(TestInput::Theme { dark: !real });
    let (events, timed_out) = drive(&mut ui, Duration::from_secs(2), |_, e| matches!(e, UiEvent::ThemeChanged { .. }));
    assert!(!timed_out);
    assert_eq!(events, vec![UiEvent::ThemeChanged { dark: !real }]);
    assert_eq!(ui.system_dark(), !real);
    ui.inject(TestInput::Theme { dark: real });
    let (events, _) = drive(&mut ui, Duration::from_secs(2), |_, e| matches!(e, UiEvent::ThemeChanged { .. }));
    assert_eq!(events, vec![UiEvent::ThemeChanged { dark: real }]);
}

#[test]
fn a_settings_change_for_something_else_is_not_a_theme_change() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    // wParam 0 with a string that is not ImmersiveColorSet.
    let other: Vec<u16> = sys::wide("Environment");
    let h = sys::raw(ui.hidden);
    sys::send(h, windows::Win32::UI::WindowsAndMessaging::WM_SETTINGCHANGE, 0, other.as_ptr() as isize);
    // wParam of a numbered setting with no string at all.
    sys::send(h, windows::Win32::UI::WindowsAndMessaging::WM_SETTINGCHANGE, 0x2F, 0);
    let (events, _) = drive(&mut ui, Duration::from_millis(200), |_, _| false);
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn time_change_and_resume_report_clock_changed() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    ui.inject(TestInput::TimeChange);
    ui.inject(TestInput::Resume);
    let (events, _) = drive(&mut ui, Duration::from_millis(300), |_, _| false);
    assert_eq!(count(&events, &UiEvent::ClockChanged), 2, "{events:?}");
    // Other power broadcasts are not clock changes.
    assert!(sys::post(sys::raw(ui.hidden), windows::Win32::UI::WindowsAndMessaging::WM_POWERBROADCAST, 0x4, 0));
    let (events, _) = drive(&mut ui, Duration::from_millis(150), |_, _| false);
    assert!(events.is_empty());
}

#[test]
fn taskbar_created_arrives_as_an_event() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    ui.inject(TestInput::TaskbarCreated);
    let (events, timed_out) = drive(&mut ui, Duration::from_secs(1), |_, e| matches!(e, UiEvent::TaskbarCreated));
    assert!(!timed_out);
    assert_eq!(events, vec![UiEvent::TaskbarCreated]);
}

#[test]
fn display_changes_are_debounced_into_one_event() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let t0 = Instant::now();
    let mut sent = 0u32;
    let mut last_sent = t0;
    let mut got_at = None;
    let (events, _) = drive(&mut ui, Duration::from_millis(900), |ui, e| {
        // Four messages 50 ms apart: each restarts the 250 ms timer.
        if sent < 4 && t0.elapsed() >= Duration::from_millis(50 * sent as u64) {
            ui.inject(TestInput::DisplayChange);
            sent += 1;
            last_sent = Instant::now();
        }
        if matches!(e, UiEvent::MonitorsChanged) {
            got_at = Some(last_sent.elapsed());
        }
        false
    });
    assert_eq!(count(&events, &UiEvent::MonitorsChanged), 1, "{events:?}");
    let after_last = got_at.unwrap();
    eprintln!("MonitorsChanged {after_last:?} after the last of 4 display messages");
    assert!(after_last >= Duration::from_millis(235), "not debounced: {after_last:?}");
    assert!(after_last < Duration::from_millis(500));
    // monitors() reads the live set on every call.
    assert_eq!(ui.monitors(), monitor::enumerate());
}

#[test]
fn close_messages_raise_close_requested() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    use windows::Win32::UI::WindowsAndMessaging::WM_CLOSE;
    assert!(sys::post(sys::raw(ui.hidden), WM_CLOSE, 0, 0));
    assert!(sys::post(sys::raw(ui.hidden), WM_ENDSESSION, 1, 0));
    // An ENDSESSION that says the session was not ended changes nothing.
    assert!(sys::post(sys::raw(ui.hidden), WM_ENDSESSION, 0, 0));
    let (events, _) = drive(&mut ui, Duration::from_millis(250), |_, _| false);
    assert_eq!(count(&events, &UiEvent::CloseRequested), 2, "{events:?}");
}

// ---- bar windows (hidden, nothing registered) ----

#[test]
fn a_bar_window_never_activates_has_no_taskbar_button_and_starts_hidden() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    ui.set_offscreen(true);
    let mon = primary(&ui);
    let bar = ui.create_bar(&spec(&mon)).unwrap();
    let h = hwnd(&ui, bar);
    let ex = ex_style(h);
    for (name, bit) in [
        ("layered", WS_EX_LAYERED),
        ("toolwindow", WS_EX_TOOLWINDOW),
        ("noactivate", WS_EX_NOACTIVATE),
        ("topmost", WS_EX_TOPMOST),
    ] {
        assert_ne!(ex & bit.0, 0, "{name}");
    }
    assert_eq!(ex & WS_EX_APPWINDOW.0, 0, "no taskbar button");
    // SAFETY: reading the style of a window of ours; sending to our own window.
    unsafe {
        assert_ne!(GetWindowLongPtrW(h, GWL_STYLE) as u32 & WS_POPUP.0, 0);
        let r = SendMessageW(h, WM_MOUSEACTIVATE, None, None);
        assert_eq!(r.0, MA_NOACTIVATE as isize);
    }
    assert!(!visible(h), "hidden until the first present");
    assert!(ui.is_visible(bar));
    assert!(!ui.is_reserved(bar));
    ui.destroy_bar(bar);
    // SAFETY: plain handle argument.
    assert!(!unsafe { IsWindow(Some(h)) }.as_bool());
}

#[test]
fn create_reports_geometry_and_places_the_window_at_the_edge() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let mon = primary(&ui);
    let m = Metrics::new(&spec(&mon), mon.dpi);
    let bar = ui.create_bar(&spec(&mon)).unwrap();
    let g = ui.geometry(bar).unwrap();
    assert_eq!(g.height_px as i32, m.height);
    assert_eq!(g.width_px as i32, mon.rect.width());
    assert_eq!(g.scale, mon.dpi as f32 / 96.0);
    assert_eq!(ui.window_rect(bar), Some(RectPx::new(mon.rect.x0, mon.rect.y0, mon.rect.x1, mon.rect.y0 + m.height)));
    let (events, _) = drive(&mut ui, Duration::from_millis(100), |_, _| false);
    assert_eq!(geometries(&events, bar), vec![(g.width_px, g.height_px, g.scale)]);

    let mut bottom = spec(&mon);
    bottom.position = Position::Bottom;
    let b2 = ui.create_bar(&bottom).unwrap();
    assert_eq!(ui.window_rect(b2), Some(RectPx::new(mon.rect.x0, mon.rect.y1 - m.height, mon.rect.x1, mon.rect.y1)));

    let mut floating = spec(&mon);
    floating.floating = true;
    let b3 = ui.create_bar(&floating).unwrap();
    let mg = Metrics::new(&floating, mon.dpi).margin;
    assert_eq!(
        ui.window_rect(b3),
        Some(RectPx::new(mon.rect.x0 + mg, mon.rect.y0 + mg, mon.rect.x1 - mg, mon.rect.y0 + mg + m.height))
    );
    assert!(!ui.is_reserved(b3), "reserve is off");
}

#[test]
fn offscreen_bars_sit_beyond_the_virtual_screen_and_never_register() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    ui.set_offscreen(true);
    let mon = primary(&ui);
    let mut s = spec(&mon);
    s.reserve = true;
    let bar = ui.create_bar(&s).unwrap();
    let right = ui.monitors().iter().map(|m| m.rect.x1).max().unwrap();
    let r = ui.window_rect(bar).unwrap();
    assert!(r.x0 >= right + 200, "{r:?} vs {right}");
    assert!(!ui.is_reserved(bar));
    assert_eq!(ui.appbar_registrations(bar), 0);
    assert_eq!(ui.work_area(&mon.device), Some(mon.work), "work area untouched");
}

#[test]
fn unknown_monitor_and_bar_are_errors() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let mon = primary(&ui);
    let mut s = spec(&mon);
    s.monitor = "\\\\.\\DISPLAY99".into();
    assert_eq!(ui.create_bar(&s), Err(crate::WinError::NoSuchMonitor("\\\\.\\DISPLAY99".into())));
    assert_eq!(ui.present(BarId(77), &mut |_| None), Err(crate::WinError::NoSuchBar(BarId(77))));
    assert_eq!(ui.geometry(BarId(77)), None);
    ui.destroy_bar(BarId(77));
    ui.set_cursor(BarId(77), Cursor::Pointer);
    ui.inject(TestInput::Dpi { bar: BarId(77), dpi: 120 });
    assert!(!ui.is_visible(BarId(77)));
}

#[test]
fn dpi_change_relayouts_96_120_96() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let mon = primary(&ui);
    let s = spec(&mon);
    let bar = ui.create_bar(&s).unwrap();
    // Whatever the monitor's own scale is, the sequence under test is 96, 120, 96.
    for dpi in [96, 120, 96] {
        ui.inject(TestInput::Dpi { bar, dpi });
    }
    let (events, _) = drive(&mut ui, Duration::from_millis(250), |_, _| false);
    let g = geometries(&events, bar);
    let (small, big) = (Metrics::new(&s, 96).height as u32, Metrics::new(&s, 120).height as u32);
    eprintln!("monitor {} dpi: bar heights seen {:?}", mon.dpi, g.iter().map(|x| x.1).collect::<Vec<_>>());
    assert_eq!((small, big), (32, 40));
    // The first report is the create (at the monitor's own scale); a request
    // for the scale already in use is not reported again.
    let tail = &g[g.len() - 3..];
    let width = ui.geometry(bar).unwrap().width_px;
    assert_eq!(tail, [(width, small, 1.0), (width, big, 1.25), (width, small, 1.0)]);
    assert_eq!(ui.window_rect(bar).unwrap().height() as u32, small);
}

#[test]
fn pointer_messages_become_pointer_events_in_physical_bar_px() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let mon = primary(&ui);
    let bar = ui.create_bar(&spec(&mon)).unwrap();
    let ev = |kind, x: f32, y: f32| UiEvent::Pointer { bar, event: PointerEvent { kind, x, y } };
    let input = [
        (PointerKind::Move, 10.0, 5.0),
        (PointerKind::Down(Button::Left), 10.0, 5.0),
        (PointerKind::Up(Button::Left), 10.0, 5.0),
        (PointerKind::Down(Button::Right), 300.0, 20.0),
        (PointerKind::Up(Button::Right), 300.0, 20.0),
        (PointerKind::Down(Button::Middle), 11.0, 6.0),
        (PointerKind::Up(Button::Middle), 11.0, 6.0),
        (PointerKind::Wheel(2.0), 120.0, 10.0),
        (PointerKind::Wheel(-0.5), 121.0, 11.0),
        (PointerKind::Leave, 121.0, 11.0),
    ];
    for (kind, x, y) in input {
        ui.inject(TestInput::Pointer { bar, event: PointerEvent { kind, x, y } });
    }
    let (events, _) = drive(&mut ui, Duration::from_millis(300), |_, _| false);
    let got: Vec<UiEvent> = events.into_iter().filter(|e| matches!(e, UiEvent::Pointer { .. })).collect();
    let want: Vec<UiEvent> = input.iter().map(|(k, x, y)| ev(*k, *x, *y)).collect();
    // The leave carries the last position the window saw.
    assert_eq!(got, want);
    // Injected input never arms mouse tracking or capture: no extra leave.
    assert!(!ui.shared.wins.borrow()[&sys::raw(hwnd(&ui, bar))].tracking);
}

#[test]
fn the_wheel_with_shift_held_and_the_tilt_wheel_arrive_as_a_sideways_wheel() {
    // MK_SHIFT is 0x4 in the low word of wParam; the high word is the delta (120 = one notch up).
    const MK_SHIFT: usize = 0x4;
    let wheel = |delta: i16| ((delta as u16 as usize) << 16) | MK_SHIFT;
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let mon = primary(&ui);
    let bar = ui.create_bar(&spec(&mon)).unwrap();
    let at = ui.window_rect(bar).unwrap();
    // The message carries screen coordinates; the event comes back in bar px.
    let lp = ((at.y0 + 5) as u16 as isize) << 16 | (at.x0 + 10) as u16 as isize;
    let h = sys::raw(hwnd(&ui, bar));
    assert!(sys::post(h, windows::Win32::UI::WindowsAndMessaging::WM_MOUSEWHEEL, wheel(-120), lp));
    assert!(sys::post(h, windows::Win32::UI::WindowsAndMessaging::WM_MOUSEWHEEL, wheel(240), lp));
    // No Shift: an ordinary wheel. The tilt wheel: positive is right.
    assert!(sys::post(h, windows::Win32::UI::WindowsAndMessaging::WM_MOUSEWHEEL, (120usize) << 16, lp));
    assert!(sys::post(h, windows::Win32::UI::WindowsAndMessaging::WM_MOUSEHWHEEL, (120usize) << 16, lp));
    let (events, _) = drive(&mut ui, Duration::from_millis(300), |_, _| false);
    let kinds: Vec<PointerKind> = events
        .into_iter()
        .filter_map(|e| match e {
            UiEvent::Pointer { event, .. } => Some(event.kind),
            _ => None,
        })
        .collect();
    assert_eq!(
        kinds,
        [PointerKind::HWheel(1.0), PointerKind::HWheel(-2.0), PointerKind::Wheel(1.0), PointerKind::HWheel(1.0)],
        "Shift with the wheel down goes right"
    );
}

#[test]
fn a_real_mouse_move_arms_leave_tracking_and_reports_the_leave() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let mon = primary(&ui);
    let bar = ui.create_bar(&spec(&mon)).unwrap();
    let h = sys::raw(hwnd(&ui, bar));
    // A move without the injected marker takes the real path: Windows is asked
    // to report the leave. The real pointer is not over this hidden window, so
    // it answers with WM_MOUSELEAVE at once.
    assert!(sys::post(h, windows::Win32::UI::WindowsAndMessaging::WM_MOUSEMOVE, 0, super::pack(7, 3)));
    let (events, timed_out) = drive(&mut ui, Duration::from_secs(1), |_, e| {
        matches!(e, UiEvent::Pointer { event: PointerEvent { kind: PointerKind::Leave, .. }, .. })
    });
    assert!(!timed_out, "no leave arrived: {events:?}");
    let kinds: Vec<PointerKind> = events
        .iter()
        .filter_map(|e| match e {
            UiEvent::Pointer { event, .. } => Some(event.kind),
            _ => None,
        })
        .collect();
    assert_eq!(kinds, vec![PointerKind::Move, PointerKind::Leave]);
    let wins = ui.shared.wins.borrow();
    assert!(!wins[&h].tracking && !wins[&h].hovered);
}

#[test]
fn set_cursor_picks_the_cursor_for_wm_setcursor() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let mon = primary(&ui);
    let bar = ui.create_bar(&spec(&mon)).unwrap();
    let h = hwnd(&ui, bar);
    // SAFETY: loading shared system cursors, sending to our own window.
    unsafe {
        let hand = LoadCursorW(None, IDC_HAND).unwrap();
        let arrow = LoadCursorW(None, IDC_ARROW).unwrap();
        ui.set_cursor(bar, Cursor::Pointer);
        let r = SendMessageW(h, WM_SETCURSOR, None, Some(windows::Win32::Foundation::LPARAM(1)));
        assert_eq!(r.0, 1);
        assert_eq!(GetCursor(), hand);
        ui.set_cursor(bar, Cursor::Default);
        SendMessageW(h, WM_SETCURSOR, None, Some(windows::Win32::Foundation::LPARAM(1)));
        assert_eq!(GetCursor(), arrow);
    }
}

#[test]
fn set_frame_deadline_and_probe_see_a_stalled_ui_thread() {
    let mut ui = Ui::new(UiOptions { probe: true, ..UiOptions::default() }).unwrap();
    assert!(ui.probe_stats().is_some());
    ui.set_frame_deadline(Some(Instant::now() + Duration::from_millis(200)));
    let (_, timed_out) = drive(&mut ui, Duration::from_millis(900), |_, e| {
        if matches!(e, UiEvent::FrameDue) {
            // The UI thread is busy and cannot answer for 150 ms.
            std::thread::sleep(Duration::from_millis(150));
            return true;
        }
        false
    });
    assert!(!timed_out);
    // The probe thread records the stalled round trip once the UI thread
    // answers again: keep pumping for a moment before reading.
    drive(&mut ui, Duration::from_millis(120), |_, _| false);
    let s = ui.probe_stats().unwrap();
    eprintln!(
        "probe ({}): {} samples, p50 {:.2} ms, p95 {:.2} ms, worst {:.2} ms, {} timeouts",
        session_name(),
        s.samples,
        s.p50_ms,
        s.p95_ms,
        s.worst_ms,
        s.timeouts
    );
    assert!(s.samples >= 10, "{s:?}");
    assert_eq!(s.timeouts, 0);
    assert!(s.worst_ms >= 100.0, "the 150 ms stall was not seen: {s:?}");
    assert!(s.p50_ms < 100.0);
    assert!(Ui::new(UiOptions::default()).unwrap().probe_stats().is_none());
}

#[test]
fn probe_on_an_idle_loop_reports_small_gaps() {
    let mut ui = Ui::new(UiOptions { probe: true, ..UiOptions::default() }).unwrap();
    drive(&mut ui, Duration::from_millis(600), |_, _| false);
    let s = ui.probe_stats().unwrap();
    eprintln!(
        "idle probe ({}): {} samples, p50 {:.2} ms, p95 {:.2} ms, worst {:.2} ms",
        session_name(),
        s.samples,
        s.p50_ms,
        s.p95_ms,
        s.worst_ms
    );
    assert!(s.samples >= 20, "{s:?}");
    assert_eq!(s.timeouts, 0);
    assert!(s.p50_ms < 20.0, "{s:?}");
}

#[test]
fn shell_open_reports_a_missing_file_without_any_window() {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
    let t = std::thread::spawn(|| {
        // SAFETY: balanced below on the same thread.
        let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let r = crate::shell::shell_open(std::path::Path::new("C:\\rsb-no-such-folder\\missing.file"));
        if com.is_ok() {
            // SAFETY: the initialise above succeeded.
            unsafe { CoUninitialize() };
        }
        r
    });
    let r = t.join().unwrap();
    assert!(r.is_err(), "a missing file must fail, not open anything");
}

// ---- tests that show a window or register screen space ----

fn print_work(label: &str, mon: &MonitorInfo, w: RectPx) {
    eprintln!("work area {label} ({}): {},{},{},{}", mon.device, w.x0, w.y0, w.x1, w.y1);
}

/// Prints the work area before and after and fails if it did not come back.
struct WorkGuard {
    device: String,
    before: RectPx,
}

impl WorkGuard {
    fn new(ui: &Ui, mon: &MonitorInfo) -> WorkGuard {
        let before = work(ui, mon);
        print_work("before", mon, before);
        WorkGuard { device: mon.device.clone(), before }
    }

    fn check(&self, ui: &mut Ui, mon: &MonitorInfo) {
        let after = wait_work(ui, mon, Duration::from_secs(2), |w| w == self.before);
        print_work("after", mon, after);
        assert_eq!(after, self.before, "{}: the work area was not restored exactly", self.device);
    }
}

#[test]
#[ignore = "shows a window or registers screen space: run through the desktop lock"]
fn present_shows_the_window_without_focus_and_hide_works() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    ui.set_offscreen(true); // shown, but beyond the visible desktop
    let mon = primary(&ui);
    let bar = ui.create_bar(&spec(&mon)).unwrap();
    let h = hwnd(&ui, bar);
    assert!(!visible(h));
    // SAFETY: plain call.
    let fg_before = unsafe { GetForegroundWindow() };
    // First present: a partial dirty rect is widened to the whole window.
    ui.present(bar, &mut |px| {
        fill(px, [60, 40, 20, 255]);
        Some(RectPx::new(1, 1, 5, 5))
    })
    .unwrap();
    assert!(visible(h), "shown by its first present");
    // SAFETY: plain call.
    assert_eq!(unsafe { GetForegroundWindow() }, fg_before, "the bar must not take the foreground");
    // A partial update, one with nothing to present, and a whole-surface one.
    ui.present(bar, &mut |px| {
        let r = RectPx::new(10, 2, 40, 20).intersect(&full(px));
        for y in 2..20usize {
            for x in 10..40usize {
                let i = (y * px.width as usize + x) * 4;
                px.data[i..i + 4].copy_from_slice(&[0, 0, 255, 255]);
            }
        }
        r
    })
    .unwrap();
    ui.present(bar, &mut |_| None).unwrap();
    ui.present(bar, &mut |px| Some(full(px))).unwrap();
    // A rectangle past the surface is cut to it, not an error.
    ui.present(bar, &mut |_| Some(RectPx::new(-50, -50, 99999, 99999))).unwrap();
    ui.set_visible(bar, false);
    assert!(!visible(h) && !ui.is_visible(bar));
    ui.set_visible(bar, true);
    assert!(visible(h) && ui.is_visible(bar));
    ui.destroy_bar(bar);
    // SAFETY: plain call.
    assert!(!unsafe { IsWindow(Some(h)) }.as_bool());
}

fn reserved_spec(mon: &MonitorInfo) -> BarWindowSpec {
    let mut s = spec(mon);
    s.reserve = true;
    s
}

#[test]
#[ignore = "shows a window or registers screen space: run through the desktop lock"]
fn appbar_moves_the_work_area_by_exactly_the_bar_height_and_restores_it() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let mon = primary(&ui);
    let guard = WorkGuard::new(&ui, &mon);
    let s = reserved_spec(&mon);
    let h = Metrics::new(&s, mon.dpi).height;
    let bar = ui.create_bar(&s).unwrap();
    assert!(ui.is_reserved(bar));
    let w = wait_work(&mut ui, &mon, Duration::from_secs(2), |w| w != guard.before);
    print_work("while registered", &mon, w);
    assert_eq!(w.y0 - guard.before.y0, h, "top moved down by the bar height");
    assert_eq!((w.x0, w.x1, w.y1), (guard.before.x0, guard.before.x1, guard.before.y1), "other edges untouched");
    assert_eq!(ui.window_rect(bar).unwrap().y1, w.y0, "the window sits exactly in the reserved strip");
    ui.destroy_bar(bar);
    guard.check(&mut ui, &mon);
}

#[test]
#[ignore = "shows a window or registers screen space: run through the desktop lock"]
fn appbar_at_the_bottom_and_floating_reserve_the_right_amount() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let mon = primary(&ui);
    let guard = WorkGuard::new(&ui, &mon);

    let mut flo = reserved_spec(&mon);
    flo.floating = true;
    let m = Metrics::new(&flo, mon.dpi);
    let bar = ui.create_bar(&flo).unwrap();
    let w = wait_work(&mut ui, &mon, Duration::from_secs(2), |w| w != guard.before);
    print_work("floating top", &mon, w);
    assert_eq!(w.y0 - guard.before.y0, m.reserved(), "bar height plus margin");
    let win = ui.window_rect(bar).unwrap();
    assert_eq!((win.y0, win.y1), (guard.before.y0 + m.margin, guard.before.y0 + m.margin + m.height));
    ui.destroy_bar(bar);
    guard.check(&mut ui, &mon);

    let mut bot = reserved_spec(&mon);
    bot.position = Position::Bottom;
    let bar = ui.create_bar(&bot).unwrap();
    let w = wait_work(&mut ui, &mon, Duration::from_secs(2), |w| w != guard.before);
    print_work("bottom", &mon, w);
    let h = Metrics::new(&bot, mon.dpi).height;
    assert_eq!(guard.before.y1 - w.y1, h, "bottom moved up by the bar height");
    assert_eq!(w.y0, guard.before.y0);
    ui.destroy_bar(bar);
    guard.check(&mut ui, &mon);
}

#[test]
#[ignore = "shows a window or registers screen space: run through the desktop lock"]
fn unchanged_position_notices_do_not_loop() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let mon = primary(&ui);
    let guard = WorkGuard::new(&ui, &mon);
    let bar = ui.create_bar(&reserved_spec(&mon)).unwrap();
    assert_eq!(ui.appbar_setpos_calls(bar), 1);
    let h = sys::raw(hwnd(&ui, bar));
    // Let the shell's own notices for our SETPOS arrive, then add 40 of ours.
    let mut sent = false;
    drive(&mut ui, Duration::from_millis(700), |_, _| {
        if !sent {
            sent = true;
            for _ in 0..40 {
                assert!(sys::post(h, crate::appbar::CALLBACK_MSG, crate::appbar::ABN_POSCHANGED, 0));
            }
        }
        false
    });
    eprintln!("SETPOS calls after 40 extra position notices: {}", ui.appbar_setpos_calls(bar));
    assert_eq!(ui.appbar_setpos_calls(bar), 1, "an unchanged grant is not submitted again");
    ui.destroy_bar(bar);
    guard.check(&mut ui, &mon);
}

#[test]
#[ignore = "shows a window or registers screen space: run through the desktop lock"]
fn taskbar_created_registers_the_bar_again() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let mon = primary(&ui);
    let guard = WorkGuard::new(&ui, &mon);
    let s = reserved_spec(&mon);
    let h = Metrics::new(&s, mon.dpi).height;
    let bar = ui.create_bar(&s).unwrap();
    let w = wait_work(&mut ui, &mon, Duration::from_secs(2), |w| w != guard.before);
    assert_eq!(ui.appbar_registrations(bar), 1);
    ui.inject(TestInput::TaskbarCreated);
    let (events, timed_out) = drive(&mut ui, Duration::from_secs(2), |_, e| matches!(e, UiEvent::TaskbarCreated));
    assert!(!timed_out);
    assert_eq!(count(&events, &UiEvent::TaskbarCreated), 1);
    assert_eq!(ui.appbar_registrations(bar), 2, "registered again");
    assert!(ui.is_reserved(bar));
    let after = wait_work(&mut ui, &mon, Duration::from_millis(500), |_| false);
    print_work("after TaskbarCreated", &mon, after);
    assert_eq!(after, w, "the same space is held");
    assert_eq!(after.y0 - guard.before.y0, h);
    ui.destroy_bar(bar);
    guard.check(&mut ui, &mon);
}

#[test]
#[ignore = "shows a window or registers screen space: run through the desktop lock"]
fn a_bar_the_shell_refused_at_start_registers_when_the_shell_announces_itself() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let mon = primary(&ui);
    let guard = WorkGuard::new(&ui, &mon);
    let s = reserved_spec(&mon);
    let h = Metrics::new(&s, mon.dpi).height;
    crate::appbar::REFUSE.store(1, std::sync::atomic::Ordering::Release);
    let bar = ui.create_bar(&s).unwrap();
    assert_eq!(ui.appbar_registrations(bar), 0, "the first registration was refused");
    assert!(!ui.is_reserved(bar));
    ui.inject(TestInput::TaskbarCreated);
    let (_, timed_out) = drive(&mut ui, Duration::from_secs(2), |_, e| matches!(e, UiEvent::TaskbarCreated));
    assert!(!timed_out);
    assert_eq!(ui.appbar_registrations(bar), 1, "registered when the shell came up");
    assert!(ui.is_reserved(bar));
    let after = wait_work(&mut ui, &mon, Duration::from_secs(2), |w| w != guard.before);
    assert_eq!(after.y0 - guard.before.y0, h);
    ui.destroy_bar(bar);
    guard.check(&mut ui, &mon);
}

#[test]
#[ignore = "shows a window or registers screen space: run through the desktop lock"]
fn removing_every_appbar_gives_the_space_back_without_the_window_loop() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let mon = primary(&ui);
    let guard = WorkGuard::new(&ui, &mon);
    let s = reserved_spec(&mon);
    let bar = ui.create_bar(&s).unwrap();
    wait_work(&mut ui, &mon, Duration::from_secs(2), |w| w != guard.before);
    // What the panic hook does, from another thread.
    std::thread::spawn(crate::remove_all_appbars).join().unwrap();
    let after = wait_work(&mut ui, &mon, Duration::from_secs(2), |w| w == guard.before);
    assert_eq!(after, guard.before, "the reserved space is released");
    ui.destroy_bar(bar);
    guard.check(&mut ui, &mon);
}

#[test]
#[ignore = "shows a window or registers screen space: run through the desktop lock"]
fn end_of_session_removes_the_appbar_inside_the_message() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let mon = primary(&ui);
    let guard = WorkGuard::new(&ui, &mon);
    let bar = ui.create_bar(&reserved_spec(&mon)).unwrap();
    wait_work(&mut ui, &mon, Duration::from_secs(2), |w| w != guard.before);
    assert!(sys::post(sys::raw(ui.hidden), WM_ENDSESSION, 1, 0));
    let mut at_event = None;
    let (events, timed_out) = drive(&mut ui, Duration::from_secs(2), |ui, e| {
        if matches!(e, UiEvent::CloseRequested) {
            at_event = Some(work(ui, &mon));
            return true;
        }
        false
    });
    assert!(!timed_out);
    assert_eq!(count(&events, &UiEvent::CloseRequested), 1);
    assert!(!ui.is_reserved(bar), "no registration left");
    // The shell already restored the space when the event was handed out.
    let at = at_event.unwrap();
    print_work("when CloseRequested arrived", &mon, at);
    assert_eq!(at, guard.before);
    ui.destroy_bar(bar);
    guard.check(&mut ui, &mon);
}

#[test]
#[ignore = "shows a window or registers screen space: run through the desktop lock"]
fn dpi_change_with_an_appbar_moves_the_reserved_space() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let mon = primary(&ui);
    let guard = WorkGuard::new(&ui, &mon);
    let s = reserved_spec(&mon);
    let small = Metrics::new(&s, mon.dpi).height;
    let big = Metrics::new(&s, 120).height;
    let bar = ui.create_bar(&s).unwrap();
    wait_work(&mut ui, &mon, Duration::from_secs(2), |w| w != guard.before);
    ui.inject(TestInput::Dpi { bar, dpi: 120 });
    let w = wait_work(&mut ui, &mon, Duration::from_secs(2), |w| w.y0 - guard.before.y0 == big);
    print_work("at 120 dpi", &mon, w);
    assert_eq!(w.y0 - guard.before.y0, big);
    ui.inject(TestInput::Dpi { bar, dpi: mon.dpi });
    let w = wait_work(&mut ui, &mon, Duration::from_secs(2), |w| w.y0 - guard.before.y0 == small);
    assert_eq!(w.y0 - guard.before.y0, small);
    ui.destroy_bar(bar);
    guard.check(&mut ui, &mon);
}

#[test]
#[ignore = "shows a window or registers screen space: run through the desktop lock"]
fn two_bars_on_one_edge_stack_in_creation_order() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let mon = primary(&ui);
    let guard = WorkGuard::new(&ui, &mon);
    let s = reserved_spec(&mon);
    let h = Metrics::new(&s, mon.dpi).height;
    let a = ui.create_bar(&s).unwrap();
    let b = ui.create_bar(&s).unwrap();
    let w = wait_work(&mut ui, &mon, Duration::from_secs(2), |w| w.y0 - guard.before.y0 == 2 * h);
    print_work("two top bars", &mon, w);
    assert_eq!(w.y0 - guard.before.y0, 2 * h);
    let (ra, rb) = (ui.window_rect(a).unwrap(), ui.window_rect(b).unwrap());
    assert_eq!(rb.y0, ra.y1, "second bar sits under the first");
    ui.destroy_bar(b);
    ui.destroy_bar(a);
    guard.check(&mut ui, &mon);
}

#[test]
#[ignore = "shows a window or registers screen space: run through the desktop lock"]
fn fullscreen_notices_apply_below_hide_and_stay() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    ui.set_offscreen(true);
    let mon = primary(&ui);
    let mk = |ui: &mut Ui, mode| {
        let mut s = spec(&mon);
        s.fullscreen = mode;
        let b = ui.create_bar(&s).unwrap();
        ui.present(b, &mut |px| {
            fill(px, [10, 10, 10, 255]);
            Some(full(px))
        })
        .unwrap();
        b
    };
    let below = mk(&mut ui, FullscreenMode::Below);
    let hide = mk(&mut ui, FullscreenMode::Hide);
    let stay = mk(&mut ui, FullscreenMode::Stay);
    let topmost = |ui: &Ui, b| ex_style(hwnd(ui, b)) & WS_EX_TOPMOST.0 != 0;
    assert!(topmost(&ui, below) && visible(hwnd(&ui, hide)) && visible(hwnd(&ui, stay)));

    for b in [below, hide, stay] {
        ui.inject(TestInput::FullscreenNotice { bar: b, active: true });
    }
    let (events, _) = drive(&mut ui, Duration::from_millis(300), |_, _| false);
    for b in [below, hide, stay] {
        assert_eq!(count(&events, &UiEvent::Fullscreen { bar: b, active: true }), 1);
    }
    assert!(!topmost(&ui, below), "Below drops topmost");
    assert!(visible(hwnd(&ui, below)), "Below stays visible");
    assert!(!visible(hwnd(&ui, hide)), "Hide hides");
    assert!(ui.is_visible(hide), "the user's toggle is unchanged");
    assert!(topmost(&ui, stay) && visible(hwnd(&ui, stay)), "Stay changes nothing");

    // A repeated notice is not a second event.
    ui.inject(TestInput::FullscreenNotice { bar: below, active: true });
    let (events, _) = drive(&mut ui, Duration::from_millis(150), |_, _| false);
    assert!(events.is_empty());

    for b in [below, hide, stay] {
        ui.inject(TestInput::FullscreenNotice { bar: b, active: false });
    }
    let (events, _) = drive(&mut ui, Duration::from_millis(300), |_, _| false);
    for b in [below, hide, stay] {
        assert_eq!(count(&events, &UiEvent::Fullscreen { bar: b, active: false }), 1);
    }
    assert!(topmost(&ui, below) && visible(hwnd(&ui, hide)), "restored when it ends");

    // A user toggle wins over the end of a fullscreen app.
    ui.set_visible(hide, false);
    ui.inject(TestInput::FullscreenNotice { bar: hide, active: true });
    ui.inject(TestInput::FullscreenNotice { bar: hide, active: false });
    drive(&mut ui, Duration::from_millis(200), |_, _| false);
    assert!(!visible(hwnd(&ui, hide)));
}

#[test]
#[ignore = "shows a window or registers screen space: run through the desktop lock"]
fn a_real_fullscreen_window_brings_the_shell_notice() {
    // The test window has to take the foreground, which interrupts whoever is
    // using the PC: it runs only when asked for.
    if std::env::var("RSB_TEST_FOREGROUND").as_deref() != Ok("1") {
        eprintln!("SKIPPED: set RSB_TEST_FOREGROUND=1 to let a test window take the foreground");
        return;
    }
    // SAFETY: plain call.
    if unsafe { GetForegroundWindow() }.0.is_null() {
        // A disconnected session has no foreground window to hand over, so
        // Windows cannot send the notice: nothing to measure.
        eprintln!("SKIPPED: this session has no foreground window");
        return;
    }
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let mon = primary(&ui);
    let guard = WorkGuard::new(&ui, &mon);
    let bar = ui.create_bar(&reserved_spec(&mon)).unwrap();
    wait_work(&mut ui, &mon, Duration::from_secs(2), |w| w != guard.before);
    let t0 = Instant::now();
    crate::spawn_fullscreen_test_window(&mon.device, Duration::from_millis(900)).unwrap();
    let (mut on, mut off) = (None, None);
    drive(&mut ui, Duration::from_secs(4), |_, e| {
        match e {
            UiEvent::Fullscreen { active: true, .. } => on = Some(t0.elapsed()),
            UiEvent::Fullscreen { active: false, .. } => off = Some(t0.elapsed()),
            _ => {}
        }
        off.is_some()
    });
    eprintln!("ABN_FULLSCREENAPP on after {on:?}, off after {off:?}");
    assert!(on.is_some(), "no fullscreen notice: the test window did not get the foreground");
    assert!(off.is_some());
    ui.destroy_bar(bar);
    guard.check(&mut ui, &mon);
}

#[test]
#[ignore = "shows a window or registers screen space: run through the desktop lock"]
fn run_ends_with_no_windows_and_the_work_area_restored() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let mon = primary(&ui);
    let guard = WorkGuard::new(&ui, &mon);
    let mut top = reserved_spec(&mon);
    let mut bot = reserved_spec(&mon);
    top.name = "top".into();
    bot.name = "bottom".into();
    bot.position = Position::Bottom;
    let a = ui.create_bar(&top).unwrap();
    let b = ui.create_bar(&bot).unwrap();
    let (ha, hb) = (hwnd(&ui, a), hwnd(&ui, b));
    let hidden = ui.hidden;
    ui.set_frame_deadline(Some(Instant::now() + Duration::from_millis(400)));
    let mut during = None;
    let (_, timed_out) = drive_run(&mut ui, Duration::from_secs(3), |ui, e| {
        if matches!(e, UiEvent::FrameDue) {
            during = Some(work(ui, &mon));
            return true;
        }
        false
    });
    assert!(!timed_out);
    print_work("during", &mon, during.unwrap());
    assert_ne!(during.unwrap(), guard.before, "the bars held space while running");
    // run() has returned: every bar is gone and the space is back.
    // SAFETY: plain handle arguments.
    unsafe {
        assert!(!IsWindow(Some(ha)).as_bool() && !IsWindow(Some(hb)).as_bool());
        assert!(IsWindow(Some(hidden)).as_bool(), "the hidden window lives as long as Ui");
    }
    assert!(ui.bars.is_empty());
    guard.check(&mut ui, &mon);
    drop(ui);
    // SAFETY: plain handle argument.
    assert!(!unsafe { IsWindow(Some(hidden)) }.as_bool(), "no window left after drop");
}

#[test]
#[ignore = "shows a window or registers screen space: run through the desktop lock"]
fn hiding_a_bar_releases_its_space_and_showing_takes_it_again() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let mon = primary(&ui);
    let guard = WorkGuard::new(&ui, &mon);
    let s = reserved_spec(&mon);
    let h = Metrics::new(&s, mon.dpi).height;
    let bar = ui.create_bar(&s).unwrap();
    wait_work(&mut ui, &mon, Duration::from_secs(2), |w| w != guard.before);
    ui.set_visible(bar, false);
    assert!(!ui.is_reserved(bar));
    wait_work(&mut ui, &mon, Duration::from_secs(2), |w| w == guard.before);
    ui.set_visible(bar, true);
    let w = wait_work(&mut ui, &mon, Duration::from_secs(2), |w| w != guard.before);
    assert_eq!(w.y0 - guard.before.y0, h);
    ui.destroy_bar(bar);
    guard.check(&mut ui, &mon);
}

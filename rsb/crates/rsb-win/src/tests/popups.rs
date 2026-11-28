//! Tests for popup, menu, tooltip and overlay windows, their input, the
//! outside-press watch, the clipboard and frame pacing.
//!
//! The plain tests create hidden windows beyond every monitor and only send
//! messages to windows this process made. The `#[ignore]` ones show windows
//! (still beyond every monitor) or install the mouse hook and run through the
//! desktop lock, one at a time (the ten-second mouse watch also needs
//! `RSB_TEST_REAL_HOOK=1`). No test activates a window or sends input to
//! anything it did not create.

use std::sync::{Condvar, Mutex};

use rsb_core::color::Color;
use rsb_core::key::{Key, KeyEvent, Modifiers, TextInput};
use windows::Win32::UI::WindowsAndMessaging::{
    GW_HWNDPREV, GetWindow, GetWindowRect, WM_ACTIVATE, WM_CHAR, WM_KEYDOWN, WS_EX_TRANSPARENT,
};

use super::*;
use crate::clip::{ClipApi, ClipClient};
use crate::{
    DimSpec, HitRegion, OutsidePress, PopupId, PopupWindowSpec, StallGuard, SurfaceRole, WinError, hook, keys,
};

// ---- helpers ----

fn offscreen_ui() -> Ui {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    ui.set_offscreen(true);
    ui
}

fn far_rect(ui: &Ui, w: i32, h: i32) -> RectPx {
    let x = ui.offscreen_x() + 100;
    RectPx::new(x, 40, x + w, 40 + h)
}

fn popup_spec(ui: &Ui, role: SurfaceRole, w: i32, h: i32, focusable: bool) -> PopupWindowSpec {
    PopupWindowSpec { role, rect: far_rect(ui, w, h), monitor: primary(ui).device, owner: None, focusable }
}

fn make(ui: &mut Ui, role: SurfaceRole, focusable: bool) -> PopupId {
    let spec = popup_spec(ui, role, 120, 80, focusable);
    ui.create_popup(&spec).expect("create_popup")
}

/// Every event that is ready now (messages pumped first).
fn collect(ui: &mut Ui) -> Vec<UiEvent> {
    ui.pump_messages();
    let mut out = Vec::new();
    while let Some(e) = ui.next_event() {
        if !matches!(e, UiEvent::Wake) {
            out.push(e);
        }
    }
    out
}

fn keys_of(events: &[UiEvent], id: PopupId) -> Vec<KeyEvent> {
    events
        .iter()
        .filter_map(|e| match e {
            UiEvent::Key { popup, event } if *popup == id => Some(*event),
            _ => None,
        })
        .collect()
}

fn texts_of(events: &[UiEvent], id: PopupId) -> Vec<TextInput> {
    events
        .iter()
        .filter_map(|e| match e {
            UiEvent::Text { popup, input } if *popup == id => Some(input.clone()),
            _ => None,
        })
        .collect()
}

fn window_rect(h: HWND) -> RectPx {
    let mut r = windows::Win32::Foundation::RECT::default();
    // SAFETY: a live out pointer and one of our windows.
    unsafe {
        let _ = GetWindowRect(h, &mut r);
    }
    RectPx::new(r.left, r.top, r.right, r.bottom)
}

/// `upper` is above `lower` in the z-order (walking up from `lower`).
fn is_above(upper: HWND, lower: HWND) -> bool {
    let mut cur = lower;
    for _ in 0..10_000 {
        // SAFETY: plain handle arguments.
        match unsafe { GetWindow(cur, GW_HWNDPREV) } {
            Ok(h) if !h.0.is_null() => {
                if h == upper {
                    return true;
                }
                cur = h;
            }
            _ => return false,
        }
    }
    false
}

fn paint_full(px: &mut rsb_core::pixels::PixelsMut<'_>) -> Option<RectPx> {
    fill(px, [40, 20, 10, 255]);
    Some(full(px))
}

fn on_real_screen(r: RectPx) -> bool {
    monitor::enumerate().iter().any(|m| m.rect.intersect(&r).is_some())
}

// ---- window styles and lifecycle (hidden windows) ----

#[test]
fn popup_windows_are_layered_tool_windows_that_never_activate_unless_focusable() {
    let mut ui = offscreen_ui();
    let plain = make(&mut ui, SurfaceRole::Popup, false);
    let focus = make(&mut ui, SurfaceRole::Popup, true);
    let tip = make(&mut ui, SurfaceRole::Tooltip, true);
    let (hp, hf, ht) = (ui.popup_hwnd(plain).unwrap(), ui.popup_hwnd(focus).unwrap(), ui.popup_hwnd(tip).unwrap());
    for h in [hp, hf, ht] {
        let ex = ex_style(h);
        assert_ne!(ex & WS_EX_LAYERED.0, 0);
        assert_ne!(ex & WS_EX_TOOLWINDOW.0, 0, "no taskbar button, not in Alt+Tab");
        assert_ne!(ex & WS_EX_TOPMOST.0, 0);
        assert!(!visible(h), "hidden until the first present");
    }
    assert_ne!(ex_style(hp) & WS_EX_NOACTIVATE.0, 0);
    assert_eq!(ex_style(hf) & WS_EX_NOACTIVATE.0, 0, "a focusable popup may be activated when asked");
    assert_ne!(ex_style(ht) & WS_EX_NOACTIVATE.0, 0, "a tooltip is never focusable");
    assert_ne!(ex_style(ht) & WS_EX_TRANSPARENT.0, 0, "a tooltip lets the pointer through");
    assert_eq!(ex_style(hp) & WS_EX_TRANSPARENT.0, 0);
    assert_eq!(ui.activation_calls(), 0);
}

#[test]
fn focusable_follows_the_content_and_a_tooltip_stays_unfocusable() {
    let mut ui = offscreen_ui();
    let id = make(&mut ui, SurfaceRole::Popup, false);
    let h = ui.popup_hwnd(id).unwrap();
    ui.set_popup_focusable(id, true);
    assert_eq!(ex_style(h) & WS_EX_NOACTIVATE.0, 0);
    ui.set_popup_focusable(id, false);
    assert_ne!(ex_style(h) & WS_EX_NOACTIVATE.0, 0);
    let tip = make(&mut ui, SurfaceRole::Tooltip, false);
    ui.set_popup_focusable(tip, true);
    assert_ne!(ex_style(ui.popup_hwnd(tip).unwrap()) & WS_EX_NOACTIVATE.0, 0);
    assert_eq!(ui.activation_calls(), 0, "changing the style never activates");
}

#[test]
fn activating_is_refused_without_a_call_unless_the_popup_may_take_focus_and_is_shown() {
    let mut ui = offscreen_ui();
    let plain = make(&mut ui, SurfaceRole::Popup, false);
    let focus_hidden = make(&mut ui, SurfaceRole::Popup, true);
    let tip = make(&mut ui, SurfaceRole::Tooltip, true);
    assert!(!ui.activate_popup(plain));
    assert!(!ui.activate_popup(focus_hidden), "not shown yet");
    assert!(!ui.activate_popup(tip));
    assert!(!ui.activate_popup(PopupId(999)));
    assert_eq!(ui.activation_calls(), 0, "no SetForegroundWindow was made");
}

#[test]
fn a_popup_reports_geometry_is_resized_by_move_and_destroyed() {
    let mut ui = offscreen_ui();
    let id = make(&mut ui, SurfaceRole::Menu, false);
    let events = collect(&mut ui);
    let geo = events.iter().find_map(|e| match e {
        UiEvent::PopupGeometry { popup, geometry } if *popup == id => Some(*geometry),
        _ => None,
    });
    let geo = geo.expect("PopupGeometry after create");
    assert_eq!((geo.width_px, geo.height_px), (120, 80));
    assert!(geo.scale >= 0.5);
    assert_eq!(ui.popup_geometry(id), Some(geo));

    let h = ui.popup_hwnd(id).unwrap();
    let to = RectPx::new(ui.offscreen_x() + 300, 100, ui.offscreen_x() + 460, 260);
    ui.move_popup(id, to).unwrap();
    assert_eq!(window_rect(h), to);
    let g2 = ui.popup_geometry(id).unwrap();
    assert_eq!((g2.width_px, g2.height_px), (160, 160));
    let events = collect(&mut ui);
    assert!(events.iter().any(|e| matches!(e, UiEvent::PopupGeometry { popup, .. } if *popup == id)));

    // Same size: moved, no new surface, no event.
    let moved = RectPx::new(to.x0 + 50, to.y0, to.x1 + 50, to.y1);
    ui.move_popup(id, moved).unwrap();
    assert_eq!(window_rect(h), moved);
    assert!(collect(&mut ui).iter().all(|e| !matches!(e, UiEvent::PopupGeometry { .. })));

    ui.destroy_popup(id);
    // SAFETY: plain handle argument.
    assert!(!unsafe { IsWindow(Some(h)) }.as_bool());
    assert!(ui.popup_geometry(id).is_none());
    assert!(ui.move_popup(id, moved).is_err());
    assert!(ui.present_popup(id, &mut |_| None).is_err());
    ui.destroy_popup(id); // twice is fine
}

#[test]
fn an_offscreen_run_never_places_a_popup_on_a_real_screen() {
    let mut ui = offscreen_ui();
    let mon = primary(&ui);
    let spec = PopupWindowSpec {
        role: SurfaceRole::Popup,
        rect: RectPx::new(mon.rect.x0 + 100, mon.rect.y0 + 100, mon.rect.x0 + 300, mon.rect.y0 + 260),
        monitor: mon.device.clone(),
        owner: None,
        focusable: false,
    };
    let id = ui.create_popup(&spec).unwrap();
    let r = ui.popup_rect(id).unwrap();
    assert!(!on_real_screen(r), "{r:?}");
    assert_eq!((r.width(), r.height()), (200, 160));
    let moved = ui.move_popup(id, spec.rect);
    assert!(moved.is_ok());
    assert!(!on_real_screen(ui.popup_rect(id).unwrap()));
    // A live (not offscreen) Ui keeps the rectangle it is given.
    let mut live = Ui::new(UiOptions::default()).unwrap();
    let id2 = live.create_popup(&spec).unwrap();
    assert_eq!(live.popup_rect(id2), Some(spec.rect));
}

#[test]
fn an_unknown_monitor_is_an_error() {
    let mut ui = offscreen_ui();
    let mut spec = popup_spec(&ui, SurfaceRole::Popup, 10, 10, false);
    spec.monitor = "\\\\.\\NOPE".into();
    assert!(matches!(ui.create_popup(&spec), Err(WinError::NoSuchMonitor(_))));
    let dim = DimSpec { rect: spec.rect, monitor: "\\\\.\\NOPE".into(), owner: None, backdrop: false };
    assert!(matches!(ui.create_dim(&dim), Err(WinError::NoSuchMonitor(_))));
}

#[test]
fn popup_pointer_messages_become_popup_pointer_events_in_popup_px() {
    let mut ui = offscreen_ui();
    let id = make(&mut ui, SurfaceRole::Popup, false);
    collect(&mut ui);
    let at = |kind| PointerEvent { kind, x: 10.0, y: 7.0 };
    for kind in [
        PointerKind::Move,
        PointerKind::Down(Button::Left),
        PointerKind::Up(Button::Left),
        PointerKind::Down(Button::Right),
        PointerKind::Up(Button::Right),
    ] {
        ui.inject(TestInput::PopupPointer { popup: id, event: at(kind) });
    }
    ui.inject(TestInput::PopupPointer { popup: id, event: at(PointerKind::Wheel(-1.0)) });
    let got: Vec<PointerEvent> = {
        let (events, _) = drive(&mut ui, Duration::from_millis(400), |_, _| false);
        events
            .into_iter()
            .filter_map(|e| match e {
                UiEvent::PopupPointer { popup, event } if popup == id => Some(event),
                _ => None,
            })
            .collect()
    };
    assert_eq!(got.len(), 6, "{got:?}");
    assert!(got.iter().all(|e| (e.x, e.y) == (10.0, 7.0)), "{got:?}");
    assert_eq!(got[1].kind, PointerKind::Down(Button::Left));
    assert_eq!(got[4].kind, PointerKind::Up(Button::Right));
    assert!(matches!(got[5].kind, PointerKind::Wheel(n) if n == -1.0));
}

#[test]
fn the_popup_cursor_is_kept_per_window_including_the_text_cursor() {
    let mut ui = offscreen_ui();
    let id = make(&mut ui, SurfaceRole::Popup, false);
    ui.set_popup_cursor(id, Cursor::Text);
    let h = ui.popup_hwnd(id).unwrap();
    let key = sys::raw(h);
    assert_eq!(ui.shared.wins.borrow().get(&key).map(|w| w.cursor), Some(Cursor::Text));
    // WM_SETCURSOR over the client area answers "handled" and sets the I-beam.
    // SAFETY: a message to one of our windows.
    let handled = unsafe { SendMessageW(h, WM_SETCURSOR, None, Some(windows::Win32::Foundation::LPARAM(1))) };
    assert_eq!(handled.0, 1);
    // SAFETY: reads the thread's cursor handle and a shared system cursor.
    unsafe {
        assert_eq!(GetCursor(), LoadCursorW(None, windows::Win32::UI::WindowsAndMessaging::IDC_IBEAM).unwrap());
    }
}

// ---- keyboard, text, IME ----

#[test]
fn key_events_reach_the_popup_with_their_modifiers_and_repeat() {
    let mut ui = offscreen_ui();
    let id = make(&mut ui, SurfaceRole::Popup, true);
    collect(&mut ui);
    let ctrl = Modifiers { ctrl: true, ..Modifiers::default() };
    let down = KeyEvent { key: Key::Char('c'), mods: ctrl, down: true, repeat: false };
    let again = KeyEvent { repeat: true, ..down };
    let up = KeyEvent { down: false, ..down };
    for event in [down, again, up] {
        ui.inject(TestInput::Key { popup: id, event });
    }
    assert_eq!(keys_of(&collect(&mut ui), id), vec![down, again, up]);

    // Alt goes through the system-key messages and keeps its meaning.
    let alt = KeyEvent {
        key: Key::ArrowLeft,
        mods: Modifiers { alt: true, ..Modifiers::default() },
        down: true,
        repeat: false,
    };
    ui.inject(TestInput::Key { popup: id, event: alt });
    // Named keys, function keys, digits.
    let plain = Modifiers::default();
    let list =
        [Key::Escape, Key::Enter, Key::Tab, Key::Backspace, Key::F(5), Key::Char('7'), Key::Space, Key::PageDown];
    for key in list {
        ui.inject(TestInput::Key { popup: id, event: KeyEvent { key, mods: plain, down: true, repeat: false } });
    }
    let got = keys_of(&collect(&mut ui), id);
    assert_eq!(got[0], alt);
    let names: Vec<Key> = got[1..].iter().map(|e| e.key).collect();
    assert_eq!(names, list);
    assert!(got[1..].iter().all(|e| e.mods == plain && e.down && !e.repeat));
}

#[test]
fn a_key_the_ime_used_is_not_a_key() {
    let mut ui = offscreen_ui();
    let id = make(&mut ui, SurfaceRole::Popup, true);
    collect(&mut ui);
    let h = sys::raw(ui.popup_hwnd(id).unwrap());
    // Escape that cancels a composition arrives as VK_PROCESSKEY.
    sys::send(h, WM_KEYDOWN, 0xE5, keys::key_lparam(true, false, false));
    sys::send(h, windows::Win32::UI::WindowsAndMessaging::WM_KEYUP, 0xE5, keys::key_lparam(false, false, false));
    assert!(keys_of(&collect(&mut ui), id).is_empty());
    // A real Escape afterwards still is one.
    ui.inject(TestInput::Key {
        popup: id,
        event: KeyEvent { key: Key::Escape, mods: Modifiers::default(), down: true, repeat: false },
    });
    assert_eq!(keys_of(&collect(&mut ui), id).len(), 1);
}

#[test]
fn alt_f4_on_a_popup_arrives_as_an_escape_and_leaves_the_window_alone() {
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_F4;
    use windows::Win32::UI::WindowsAndMessaging::{WM_CLOSE, WM_SYSKEYDOWN};
    let mut ui = offscreen_ui();
    let id = make(&mut ui, SurfaceRole::Popup, true);
    collect(&mut ui);
    let h = sys::raw(ui.popup_hwnd(id).unwrap());
    // The key is taken by the popup itself: a popup has no system menu to turn it into a close request.
    sys::send(h, WM_SYSKEYDOWN, VK_F4.0 as usize, keys::key_lparam(true, false, true));
    let got = keys_of(&collect(&mut ui), id);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!((got[0].key, got[0].down, got[0].repeat), (Key::Escape, true, false));
    assert!(ui.popup_hwnd(id).is_some(), "the window is the manager's to close");
    // A close request sent straight to the window does the same.
    sys::send(h, WM_CLOSE, 0, 0);
    assert_eq!(keys_of(&collect(&mut ui), id).len(), 1);
}

#[test]
fn an_ime_that_asks_where_the_character_is_gets_the_caret_in_screen_px() {
    use windows::Win32::UI::Input::Ime::{IMECHARPOSITION, IMR_QUERYCHARPOSITION};
    use windows::Win32::UI::WindowsAndMessaging::WM_IME_REQUEST;
    let mut ui = offscreen_ui();
    let id = make(&mut ui, SurfaceRole::Popup, true);
    collect(&mut ui);
    let h = sys::raw(ui.popup_hwnd(id).unwrap());
    let ask = |ui: &Ui| {
        let mut info = IMECHARPOSITION { dwSize: std::mem::size_of::<IMECHARPOSITION>() as u32, ..Default::default() };
        let _ = ui;
        // SAFETY: the structure outlives the call; the window procedure runs inside it.
        let answered = unsafe {
            windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                crate::sys::hwnd(h),
                WM_IME_REQUEST,
                Some(windows::Win32::Foundation::WPARAM(IMR_QUERYCHARPOSITION as usize)),
                Some(windows::Win32::Foundation::LPARAM(&mut info as *mut IMECHARPOSITION as isize)),
            )
        };
        (answered.0, info)
    };
    assert_eq!(ask(&ui).0, 0, "no caret yet: nothing to answer");
    let origin = ui.popup_rect(id).expect("the popup has a rect");
    ui.set_ime_caret(id, Some(RectPx::new(30, 12, 31, 30)));
    let (answered, info) = ask(&ui);
    assert_eq!(answered, 1);
    assert_eq!((info.pt.x, info.pt.y), (origin.x0 + 30, origin.y0 + 12));
    assert_eq!(info.cLineHeight, 18);
}

#[test]
fn typed_text_joins_surrogates_and_drops_control_characters() {
    let mut ui = offscreen_ui();
    let id = make(&mut ui, SurfaceRole::Popup, true);
    collect(&mut ui);
    ui.inject(TestInput::Text { popup: id, input: TextInput::Chars("h\u{e9}\u{1F600}!".into()) });
    let h = sys::raw(ui.popup_hwnd(id).unwrap());
    for control in [0x08usize, 0x0D, 0x09, 0x1B, 0x03] {
        sys::send(h, WM_CHAR, control, 1);
    }
    // A stray low surrogate and a high one never completed are dropped.
    sys::send(h, WM_CHAR, 0xDE00, 1);
    sys::send(h, WM_CHAR, 0xD83D, 1);
    sys::send(h, WM_CHAR, b'z' as usize, 1);
    let text: Vec<String> = texts_of(&collect(&mut ui), id)
        .into_iter()
        .map(|t| match t {
            TextInput::Chars(s) => s,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(text, vec!["h", "\u{e9}", "\u{1F600}", "!", "z"]);
}

#[test]
fn ime_stages_become_text_events_in_order_with_the_text_from_the_event() {
    let mut ui = offscreen_ui();
    let id = make(&mut ui, SurfaceRole::Popup, true);
    collect(&mut ui);
    let stages = [
        TextInput::CompositionStart,
        TextInput::CompositionUpdate { text: "ni".into(), cursor: 2 },
        TextInput::CompositionUpdate { text: "\u{4F60}".into(), cursor: 1 },
        TextInput::CompositionCommit("\u{4F60}".into()),
        TextInput::CompositionEnd,
    ];
    for input in stages.clone() {
        ui.inject(TestInput::Text { popup: id, input });
    }
    assert_eq!(texts_of(&collect(&mut ui), id), stages.to_vec());
    // Nothing is left over for the next real IME message.
    assert!(ui.shared.inject_comp.borrow().is_none());
}

#[test]
fn the_input_context_exists_only_after_set_ime_enabled_true() {
    let mut ui = offscreen_ui();
    let id = make(&mut ui, SurfaceRole::Popup, true);
    assert!(!ui.ime_context_present(id), "a new popup has no input context");
    ui.set_ime_enabled(id, true);
    assert!(ui.ime_context_present(id), "enabled");
    ui.set_ime_enabled(id, false);
    assert!(!ui.ime_context_present(id), "disabled again");
    // The caret call with no context or no caret is harmless.
    ui.set_ime_caret(id, Some(RectPx::new(5, 5, 6, 20)));
    ui.set_ime_caret(id, None);
    ui.set_ime_enabled(id, true);
    ui.set_ime_caret(id, Some(RectPx::new(5, 5, 6, 20)));
    assert_eq!(ui.activation_calls(), 0);
}

// ---- focus, dpi, outside presses, session ----

#[test]
fn losing_focus_to_another_process_is_reported_and_to_our_own_window_is_not() {
    let mut ui = offscreen_ui();
    let a = make(&mut ui, SurfaceRole::Popup, true);
    let b = make(&mut ui, SurfaceRole::Popup, true);
    collect(&mut ui);
    ui.inject(TestInput::Deactivate { popup: a });
    let events = collect(&mut ui);
    assert!(events.contains(&UiEvent::PopupFocus { popup: a, focused: false }), "{events:?}");
    // Focus moving to another of our own windows is the manager's business.
    let hb = ui.popup_hwnd(b).unwrap();
    let ha = ui.popup_hwnd(a).unwrap();
    // SAFETY: a message to one of our windows, naming another of ours.
    unsafe {
        SendMessageW(
            ha,
            WM_ACTIVATE,
            Some(windows::Win32::Foundation::WPARAM(0)),
            Some(windows::Win32::Foundation::LPARAM(hb.0 as isize)),
        );
    }
    assert!(collect(&mut ui).iter().all(|e| !matches!(e, UiEvent::PopupFocus { .. })));
    assert_eq!(ui.activation_calls(), 0);
}

#[test]
fn a_dpi_change_on_a_popup_is_reported_and_leaves_the_window_alone() {
    let mut ui = offscreen_ui();
    let id = make(&mut ui, SurfaceRole::Popup, false);
    collect(&mut ui);
    let h = ui.popup_hwnd(id).unwrap();
    let (rect, geo) = (window_rect(h), ui.popup_geometry(id));
    ui.inject(TestInput::PopupDpi { popup: id, dpi: 144 });
    let events = collect(&mut ui);
    assert!(events.contains(&UiEvent::PopupDpiChanged { popup: id, dpi: 144 }), "{events:?}");
    assert_eq!(window_rect(h), rect, "the window was not resized or moved");
    assert_eq!(ui.popup_geometry(id), geo);
    assert!(events.iter().all(|e| !matches!(e, UiEvent::PopupGeometry { .. })));
}

#[test]
fn an_outside_press_and_session_notices_arrive_as_events() {
    let mut ui = offscreen_ui();
    eprintln!(
        "lock and unlock notices registered in this session ({}): {}",
        session_name(),
        ui.session_notices_registered()
    );
    ui.inject(TestInput::OutsidePress(OutsidePress { x: -5, y: 1200 }));
    ui.inject(TestInput::SessionLock { locked: true });
    ui.inject(TestInput::SessionLock { locked: false });
    let (events, _) =
        drive(&mut ui, Duration::from_millis(400), |_, e| matches!(e, UiEvent::SessionLock { locked: false }));
    assert!(events.contains(&UiEvent::OutsidePress(OutsidePress { x: -5, y: 1200 })), "{events:?}");
    let locks: Vec<bool> = events
        .iter()
        .filter_map(|e| match e {
            UiEvent::SessionLock { locked } => Some(*locked),
            _ => None,
        })
        .collect();
    assert_eq!(locks, vec![true, false]);
}

// ---- simulated monitors ----

fn extra(w: i32, h: i32, dpi: u32) -> MonitorInfo {
    MonitorInfo {
        index: 0,
        handle: 0,
        device: String::new(),
        primary: false,
        rect: RectPx::new(0, 0, w, h),
        work: RectPx::new(0, 0, w, h),
        dpi,
    }
}

#[test]
fn extra_monitors_appear_in_monitors_only_in_an_offscreen_run_before_the_first_bar() {
    let options =
        UiOptions { extra_monitors: vec![extra(2560, 1440, 144), extra(3840, 2160, 192)], ..UiOptions::default() };
    let mut ui = Ui::new(options.clone()).unwrap();
    let real = monitor::enumerate().len();
    assert_eq!(ui.monitors().len(), real, "ignored unless offscreen");
    ui.set_offscreen(true);
    let all = ui.monitors();
    assert_eq!(all.len(), real + 2);
    let (a, b) = (&all[real], &all[real + 1]);
    assert_ne!(a.device, b.device);
    assert!(all.iter().filter(|m| m.primary).count() == 1);
    let real_right = all[..real].iter().map(|m| m.rect.x1).max().unwrap();
    assert!(a.rect.x0 >= real_right && b.rect.x0 >= a.rect.x1, "placed side by side to the right: {a:?} {b:?}");
    assert_eq!((a.rect.width(), a.rect.height(), a.dpi), (2560, 1440, 144));
    assert_eq!((b.rect.width(), b.rect.height(), b.dpi), (3840, 2160, 192));
    assert!(!on_real_screen(a.rect) && !on_real_screen(b.rect));

    // A bar on a simulated monitor gets that monitor's scale and sits offscreen.
    let mut s = spec(a);
    s.monitor = a.device.clone();
    let bar = ui.create_bar(&s).unwrap();
    let g = ui.geometry(bar).unwrap();
    assert_eq!(g.scale, 1.5);
    assert!(!on_real_screen(ui.window_rect(bar).unwrap()));
    // A popup on it takes its scale too.
    let spec = PopupWindowSpec {
        role: SurfaceRole::Popup,
        rect: far_rect(&ui, 100, 60),
        monitor: b.device.clone(),
        owner: Some(bar),
        focusable: false,
    };
    let p = ui.create_popup(&spec).unwrap();
    assert_eq!(ui.popup_geometry(p).unwrap().scale, 2.0);

    // Asking after a bar exists adds nothing.
    let mut late = Ui::new(options).unwrap();
    let m = primary(&late);
    late.create_bar(&spec_for(&m)).unwrap();
    late.set_offscreen(true);
    assert_eq!(late.monitors().len(), real);
}

fn spec_for(m: &MonitorInfo) -> BarWindowSpec {
    spec(m)
}

// ---- clipboard worker through the Ui, with a fake ----

struct Gate(Arc<(Mutex<bool>, Condvar)>);

impl ClipApi for Gate {
    fn open(&mut self) -> bool {
        true
    }
    fn close(&mut self) {}
    fn read_text(&mut self) -> Option<String> {
        let (lock, cv) = &*self.0;
        let mut open = lock.lock().unwrap();
        while !*open {
            open = cv.wait(open).unwrap();
        }
        Some("late".into())
    }
    fn write_text(&mut self, _text: &str) -> bool {
        true
    }
}

fn gated_client(ui: &Ui, open: bool) -> (ClipClient, Arc<(Mutex<bool>, Condvar)>) {
    let gate = Arc::new((Mutex::new(open), Condvar::new()));
    let g = Arc::clone(&gate);
    let client = ClipClient::new(
        Box::new(move || {
            let g = Arc::clone(&g);
            Box::new(move || Box::new(Gate(g)) as Box<dyn ClipApi>)
        }),
        ui.clip_notifier(),
    );
    (client, gate)
}

fn clipboard_events(events: &[UiEvent]) -> Vec<(u64, Option<String>)> {
    events
        .iter()
        .filter_map(|e| match e {
            UiEvent::Clipboard { token, text } => Some((*token, text.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn a_clipboard_read_is_answered_by_the_ui_within_500_ms_even_when_the_worker_is_stuck() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let (client, gate) = gated_client(&ui, false);
    ui.use_clipboard(client);
    let t0 = Instant::now();
    ui.clipboard_read(7);
    let mut at = None;
    let (events, _) = drive(&mut ui, Duration::from_millis(1500), |_, e| {
        if matches!(e, UiEvent::Clipboard { token: 7, .. }) {
            at = Some(t0.elapsed());
            return true;
        }
        false
    });
    let at = at.expect("the answer");
    eprintln!("stuck clipboard read answered after {at:?} ({})", session_name());
    assert_eq!(clipboard_events(&events), vec![(7, None)]);
    assert!(at >= Duration::from_millis(480) && at < Duration::from_millis(800), "{at:?}");

    // Later requests are answered at once while the worker is still stuck.
    let t1 = Instant::now();
    ui.clipboard_read(8);
    let events = collect(&mut ui);
    assert_eq!(clipboard_events(&events), vec![(8, None)]);
    assert!(t1.elapsed() < Duration::from_millis(100));
    ui.clipboard_write("never".into());
    assert_eq!(ui.clip.as_ref().unwrap().workers_started(), 1, "no second worker");

    // The worker returns; reads work again.
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let t2 = Instant::now();
    let mut answered = Vec::new();
    while answered.is_empty() && t2.elapsed() < Duration::from_secs(3) {
        ui.clipboard_read(9);
        let (events, _) =
            drive(&mut ui, Duration::from_millis(250), |_, e| matches!(e, UiEvent::Clipboard { token: 9, .. }));
        answered = clipboard_events(&events);
        if answered.first().is_some_and(|(_, t)| t.is_none()) {
            answered.clear();
        }
    }
    assert_eq!(answered.first().map(|(t, s)| (*t, s.clone())), Some((9, Some("late".to_string()))));
    assert_eq!(ui.clip.as_ref().unwrap().workers_started(), 1);
}

#[test]
fn a_clipboard_read_that_finishes_in_time_arrives_as_an_event() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let (client, _gate) = gated_client(&ui, true);
    ui.use_clipboard(client);
    let t0 = Instant::now();
    ui.clipboard_read(1);
    let (events, _) = drive(&mut ui, Duration::from_millis(1500), |_, e| matches!(e, UiEvent::Clipboard { .. }));
    assert_eq!(clipboard_events(&events), vec![(1, Some("late".to_string()))]);
    assert!(t0.elapsed() < Duration::from_millis(300), "{:?}", t0.elapsed());
}

// ---- stall guard ----

#[test]
fn the_stall_guard_reports_how_long_a_loop_turn_has_run() {
    let g = StallGuard::default();
    let t0 = Instant::now();
    assert_eq!(g.busy_for(t0), Duration::ZERO, "waiting for messages");
    g.mark_busy(t0);
    let busy = g.busy_for(t0 + Duration::from_secs(3));
    assert!(busy >= Duration::from_millis(2999) && busy <= Duration::from_millis(3002), "{busy:?}");
    g.mark_idle();
    assert_eq!(g.busy_for(t0 + Duration::from_secs(9)), Duration::ZERO);
    assert_eq!(g.hide_overlays(), 0, "no overlay shown");
    let clone = g.clone();
    clone.mark_busy(t0);
    assert!(g.busy_for(t0 + Duration::from_secs(1)) >= Duration::from_millis(999), "clones share the stamp");
}

#[test]
fn a_loop_turn_is_marked_busy_while_the_handler_runs_and_idle_while_waiting() {
    let mut ui = Ui::new(UiOptions::default()).unwrap();
    let guard = ui.stall_guard();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (s2, g2) = (Arc::clone(&seen), guard.clone());
    let (_, timed_out) = drive_run(&mut ui, Duration::from_millis(400), move |_, e| {
        if matches!(e, UiEvent::Wake) {
            s2.lock().unwrap().push(g2.busy_for(Instant::now()));
            return s2.lock().unwrap().len() >= 3;
        }
        false
    });
    assert!(!timed_out);
    assert!(seen.lock().unwrap().iter().all(|d| *d < Duration::from_millis(100)), "{:?}", seen.lock().unwrap());
    assert_eq!(guard.busy_for(Instant::now()), Duration::ZERO, "idle once the loop ended");
}

// ---- windows that show: run through the desktop lock ----

#[test]
#[ignore = "shows a window or installs the mouse hook: run through the desktop lock"]
fn a_popup_is_shown_without_focus_above_its_bar_moved_and_destroyed() {
    let mut ui = offscreen_ui();
    let mon = primary(&ui);
    let bar = ui.create_bar(&spec(&mon)).unwrap();
    ui.present(bar, &mut |px| {
        fill(px, [10, 10, 10, 255]);
        Some(full(px))
    })
    .unwrap();
    let mut s = popup_spec(&ui, SurfaceRole::Popup, 200, 120, false);
    s.owner = Some(bar);
    let id = ui.create_popup(&s).unwrap();
    let h = ui.popup_hwnd(id).unwrap();
    assert!(!visible(h));
    // SAFETY: no arguments.
    let front_before = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
    ui.present_popup(id, &mut paint_full).unwrap();
    assert!(visible(h), "shown by the first present");
    // SAFETY: no arguments.
    let front_after = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
    assert_eq!(front_before, front_after, "showing a popup does not change the foreground window");
    assert_ne!(front_after, h);
    assert!(is_above(h, hwnd(&ui, bar)), "above its owner bar");
    assert_eq!(ui.activation_calls(), 0);

    // A second present with a dirty rectangle, then a move and a resize.
    ui.present_popup(id, &mut |px| {
        fill(px, [90, 90, 90, 255]);
        Some(RectPx::new(0, 0, 20, 20))
    })
    .unwrap();
    let to = RectPx::new(s.rect.x0 + 40, s.rect.y0, s.rect.x0 + 340, s.rect.y0 + 200);
    ui.move_popup(id, to).unwrap();
    assert_eq!(window_rect(h), to);
    ui.present_popup(id, &mut paint_full).unwrap();
    assert!(visible(h));
    ui.destroy_popup(id);
    // SAFETY: plain handle argument.
    assert!(!unsafe { IsWindow(Some(h)) }.as_bool());
    assert_eq!(ui.activation_calls(), 0);
    ui.destroy_bar(bar);
}

#[test]
#[ignore = "shows a window or installs the mouse hook: run through the desktop lock"]
fn a_tooltip_is_click_through_and_sits_above_every_other_popup() {
    let mut ui = offscreen_ui();
    let menu = make(&mut ui, SurfaceRole::Menu, false);
    let tip = make(&mut ui, SurfaceRole::Tooltip, false);
    ui.present_popup(tip, &mut paint_full).unwrap();
    ui.present_popup(menu, &mut paint_full).unwrap();
    let (hm, ht) = (ui.popup_hwnd(menu).unwrap(), ui.popup_hwnd(tip).unwrap());
    assert_ne!(ex_style(ht) & WS_EX_TRANSPARENT.0, 0);
    assert!(visible(hm) && visible(ht));
    // The tooltip opened first, the menu after: the menu is the newer one on
    // top until the manager raises the tooltip again by presenting a new one.
    let tip2 = make(&mut ui, SurfaceRole::Tooltip, false);
    ui.present_popup(tip2, &mut paint_full).unwrap();
    assert!(is_above(ui.popup_hwnd(tip2).unwrap(), hm));
    assert_eq!(ui.activation_calls(), 0);
}

#[test]
#[ignore = "shows a window or installs the mouse hook: run through the desktop lock"]
fn the_dim_covers_the_monitor_fades_without_repainting_and_sits_under_the_content() {
    let mut ui = offscreen_ui();
    let mon = primary(&ui);
    let dim =
        ui.create_dim(&DimSpec { rect: mon.rect, monitor: mon.device.clone(), owner: None, backdrop: false }).unwrap();
    let hd = ui.popup_hwnd(dim).unwrap();
    let r = window_rect(hd);
    assert_eq!((r.width(), r.height()), (mon.rect.width(), mon.rect.height()), "covers the whole monitor");
    assert!(!on_real_screen(r), "an offscreen run never covers a real screen");
    assert!(!visible(hd), "hidden until the first set_dim");
    assert!(!ui.dim_backdrop_active(dim));

    // The content is created after the dim, so it is above it.
    let content = make(&mut ui, SurfaceRole::Overlay, true);
    let hc = ui.popup_hwnd(content).unwrap();
    ui.present_popup(content, &mut paint_full).unwrap();
    ui.set_dim(dim, Color::rgba8(0, 0, 0, 0));
    assert!(visible(hd) && visible(hc));
    assert!(is_above(hc, hd), "the content is above the dim");
    drive(&mut ui, Duration::from_millis(150), |_, _| false);
    let paints = ui.dim_paints(dim).unwrap();
    assert_eq!(ui.dim_alpha(dim), Some(0));

    // A fade only changes the alpha.
    let mut seen = Vec::new();
    for a in [26u8, 64, 128, 191, 230] {
        ui.set_dim(dim, Color::rgba8(0, 0, 0, a));
        drive(&mut ui, Duration::from_millis(30), |_, _| false);
        seen.push((a, ui.dim_alpha(dim)));
    }
    for (want, got) in &seen {
        assert_eq!(*got, Some(*want), "alpha read back");
    }
    assert_eq!(ui.dim_paints(dim).unwrap(), paints, "no repaint while fading");
    eprintln!("dim paints: {paints} after the first show, {} after 5 fade steps", ui.dim_paints(dim).unwrap());
    assert!(paints <= 1);

    // A new colour is painted again (at most once more).
    ui.set_dim(dim, Color::rgba8(20, 10, 40, 128));
    drive(&mut ui, Duration::from_millis(100), |_, _| false);
    assert!(ui.dim_paints(dim).unwrap() <= paints + 1);

    // The watchdog's guard hides both windows from another thread.
    let guard = ui.stall_guard();
    let n = std::thread::spawn(move || guard.hide_overlays()).join().unwrap();
    assert_eq!(n, 2, "the dim and the content");
    drive(&mut ui, Duration::from_millis(100), |_, _| false);
    assert!(!visible(hd) && !visible(hc));
    ui.destroy_popup(content);
    ui.destroy_popup(dim);
    assert_eq!(ui.stall_guard().hide_overlays(), 0);
    assert_eq!(ui.activation_calls(), 0);
}

#[test]
#[ignore = "shows a window or installs the mouse hook: run through the desktop lock"]
fn a_press_on_the_dim_arrives_as_a_pointer_event_on_the_dim() {
    let mut ui = offscreen_ui();
    let mon = primary(&ui);
    let dim =
        ui.create_dim(&DimSpec { rect: mon.rect, monitor: mon.device.clone(), owner: None, backdrop: false }).unwrap();
    ui.set_dim(dim, Color::rgba8(0, 0, 0, 100));
    let at = PointerEvent { kind: PointerKind::Down(Button::Left), x: 300.0, y: 200.0 };
    ui.inject(TestInput::PopupPointer { popup: dim, event: at });
    let (events, _) = drive(&mut ui, Duration::from_millis(300), |_, e| matches!(e, UiEvent::PopupPointer { .. }));
    assert!(events.contains(&UiEvent::PopupPointer { popup: dim, event: at }), "{events:?}");
    assert_eq!(ui.activation_calls(), 0);
}

#[test]
#[ignore = "shows a window or installs the mouse hook: run through the desktop lock"]
fn the_backdrop_window_exists_only_when_asked_and_only_when_it_took() {
    let mut ui = offscreen_ui();
    let mon = primary(&ui);
    let spec = |backdrop| DimSpec { rect: mon.rect, monitor: mon.device.clone(), owner: None, backdrop };
    let without = ui.create_dim(&spec(false)).unwrap();
    assert!(!ui.dim_backdrop_active(without));
    assert!(ui.dim_backdrop_hwnd(without).is_none());

    let with = ui.create_dim(&spec(true)).unwrap();
    let active = ui.dim_backdrop_active(with);
    eprintln!("system backdrop active on this PC ({} session): {active}", session_name());
    match ui.dim_backdrop_hwnd(with) {
        Some(b) => {
            assert!(active, "a backdrop window that did not take is destroyed");
            assert_eq!(ex_style(b) & WS_EX_LAYERED.0, 0, "not layered");
            assert_ne!(ex_style(b) & WS_EX_NOACTIVATE.0, 0);
            assert!(is_above(ui.popup_hwnd(with).unwrap(), b), "the dim is above the backdrop");
            ui.set_dim(with, Color::rgba8(0, 0, 0, 90));
            assert!(visible(b), "shown with the dim");
            let n = ui.stall_guard().hide_overlays();
            assert_eq!(n, 2);
        }
        None => assert!(!active, "no window, no backdrop"),
    }
    ui.destroy_popup(with);
    ui.destroy_popup(without);
}

#[test]
#[ignore = "shows a window or installs the mouse hook: run through the desktop lock"]
fn a_hook_thread_slower_than_the_wait_is_kept_and_removed_with_its_watch() {
    let shared = hook::HookShared::new(0);
    // No wait at all: the thread has not answered when the start returns.
    let watch = hook::Hook::start_waiting(&shared, Duration::ZERO).expect("a slow start is kept, not dropped");
    assert_eq!(hook::active_count(), 1, "listed at once, so the panic path can remove it");
    let end = Instant::now() + Duration::from_secs(2);
    while !shared.is_installed() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(shared.is_installed(), "it installs late");
    watch.stop();
    assert_eq!(hook::active_count(), 0);
    assert!(!shared.is_installed(), "and nothing is left behind");
}

#[test]
#[ignore = "shows a window or installs the mouse hook: run through the desktop lock"]
fn the_mouse_watch_exists_only_while_on_and_is_removed_at_off_quit_and_in_the_panic_path() {
    let mut ui = offscreen_ui();
    assert!(!ui.hook_installed() && ui.hook_stats().is_none(), "nothing before the first on");
    assert_eq!(hook::active_count(), 0);

    ui.watch_outside_presses(true);
    assert!(ui.hook_installed());
    assert_eq!(hook::active_count(), 1);
    ui.watch_outside_presses(true);
    assert_eq!(hook::active_count(), 1, "turning it on twice starts one");
    drive(&mut ui, Duration::from_millis(400), |_, _| false);
    let stats = ui.hook_stats().expect("stats after the first on");
    eprintln!("hook stats after 400 ms ({}, the user's own mouse only): {stats:?}", session_name());

    ui.watch_outside_presses(false);
    assert!(!ui.hook_installed());
    assert_eq!(hook::active_count(), 0);
    assert!(ui.hook_stats().is_some(), "the numbers stay readable after it is off");

    // Quit: the loop ends with the watch on.
    ui.watch_outside_presses(true);
    assert!(ui.hook_installed());
    let (_, timed_out) = drive_run(&mut ui, Duration::from_millis(500), |_, _| true);
    assert!(!timed_out);
    assert!(!ui.hook_installed(), "removed when the loop ended");
    assert_eq!(hook::active_count(), 0);

    // The panic path: removal from another thread with no loop running.
    ui.watch_outside_presses(true);
    assert!(ui.hook_installed());
    std::thread::spawn(hook::remove_outside_press_hooks).join().unwrap();
    let end = Instant::now() + Duration::from_secs(2);
    while ui.hook_state.is_installed() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!ui.hook_state.is_installed(), "the hook was removed");
    let end = Instant::now() + Duration::from_secs(2);
    while ui.hook_installed() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!ui.hook_installed());
    ui.watch_outside_presses(false);

    // Dropping the Ui with the watch on removes it too.
    let mut other = offscreen_ui();
    other.watch_outside_presses(true);
    assert_eq!(hook::active_count(), 1);
    drop(other);
    assert_eq!(hook::active_count(), 0);
}

#[test]
#[ignore = "shows a window or installs the mouse hook: run through the desktop lock"]
fn the_region_snapshot_is_swapped_under_a_running_watch() {
    let mut ui = offscreen_ui();
    let id = make(&mut ui, SurfaceRole::Popup, false);
    ui.present_popup(id, &mut paint_full).unwrap();
    ui.watch_outside_presses(true);
    let a = HitRegion { points: vec![(0.0, 0.0), (120.0, 0.0), (120.0, 80.0), (0.0, 80.0)] };
    let b = HitRegion { points: vec![(10.0, 10.0), (60.0, 10.0), (35.0, 70.0)] };
    for n in 0..400 {
        ui.set_popup_region(id, Some(if n % 2 == 0 { a.clone() } else { b.clone() }));
        if n % 50 == 0 {
            ui.move_popup(id, RectPx::new(ui.offscreen_x() + n, 40, ui.offscreen_x() + n + 120, 120)).unwrap();
        }
    }
    let snap = ui.hook_state.snapshot();
    assert_eq!(snap.popups.len(), 1);
    assert_eq!(snap.popups[0].outline.as_ref(), Some(&b.points), "the last region won");
    assert_eq!(snap.popups[0].origin.1, 40);
    assert!(snap.bars.is_empty());
    ui.set_popup_region(id, None);
    assert!(ui.hook_state.snapshot().popups[0].outline.is_none());
    drive(&mut ui, Duration::from_millis(200), |_, _| false);
    assert!(ui.hook_installed(), "still running after the swaps");
    ui.destroy_popup(id);
    assert!(ui.hook_state.snapshot().popups.is_empty());
}

#[test]
#[ignore = "timing: run alone, through the desktop lock"]
fn frame_deadlines_follow_the_compositor_period() {
    struct Run {
        gaps: Vec<Duration>,
        distinct: usize,
        pairs: usize,
        period: Option<Duration>,
    }
    fn run(align: bool) -> Run {
        let mut ui = Ui::new(UiOptions::default()).unwrap();
        ui.pacer.align = align;
        let step = Duration::from_micros(16_700);
        let mut frames: Vec<Option<u64>> = Vec::new();
        let mut gaps = Vec::new();
        let mut last = Instant::now();
        ui.set_frame_deadline(Some(last + step));
        let (_, timed_out) = drive(&mut ui, Duration::from_secs(8), |ui, e| {
            if matches!(e, UiEvent::FrameDue) {
                let now = Instant::now();
                gaps.push(now - last);
                last = now;
                frames.push(ui.compositor_frame());
                if frames.len() >= 60 {
                    return true;
                }
                // The next animation frame is asked for one step after this one.
                ui.set_frame_deadline(Some(now + step));
            }
            false
        });
        assert!(!timed_out, "60 deadlines did not all fire");
        let known: Vec<u64> = frames.iter().flatten().copied().collect();
        let distinct = known.windows(2).filter(|w| w[0] != w[1]).count();
        Run { gaps, distinct, pairs: known.len().saturating_sub(1), period: ui.compositor_period() }
    }
    for align in [true, false] {
        let mut r = run(align);
        r.gaps.sort();
        let mean = r.gaps.iter().sum::<Duration>() / r.gaps.len() as u32;
        let (p50, p95) = (r.gaps[r.gaps.len() / 2], r.gaps[r.gaps.len() * 95 / 100]);
        match r.period {
            Some(period) => {
                let per_period = period.as_secs_f64() / mean.as_secs_f64();
                eprintln!(
                    "pacing ({}) align={align}: display period {period:?}, 60 deadlines asked 16.7 ms apart, gap mean {mean:?} p50 {p50:?} p95 {p95:?}, {per_period:.2} presents per display period; compositor counter changed on {} of {} reads",
                    session_name(),
                    r.distinct,
                    r.pairs
                );
                if align {
                    assert!(per_period <= 1.05, "{per_period}");
                    assert!(p95 >= period.mul_f64(0.9), "{p95:?}");
                }
            }
            None => eprintln!(
                "pacing ({}) align={align}: Windows gave no compositor timing; gap mean {mean:?} p50 {p50:?} p95 {p95:?}",
                session_name()
            ),
        }
    }
}

#[test]
#[ignore = "manual: move the mouse while it runs (10 s); shows nothing, installs the mouse hook"]
fn the_mouse_watch_counts_the_real_mouse_for_ten_seconds() {
    // Installs the real mouse hook for ten seconds: only on request.
    if std::env::var("RSB_TEST_REAL_HOOK").as_deref() != Ok("1") {
        eprintln!("SKIPPED: set RSB_TEST_REAL_HOOK=1 to watch the real mouse for ten seconds");
        return;
    }
    let mut ui = offscreen_ui();
    ui.watch_outside_presses(true);
    assert!(ui.hook_installed());
    drive(&mut ui, Duration::from_secs(10), |_, _| false);
    let stats = ui.hook_stats().expect("stats");
    eprintln!("mouse watch over 10 s ({}): {stats:?}", session_name());
    assert!(ui.hook_installed(), "still installed after 10 s");
    ui.watch_outside_presses(false);
    assert!(!ui.hook_installed());
}

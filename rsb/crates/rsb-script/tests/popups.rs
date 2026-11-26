//! Popups as the script sees them: the open state both sides write, content that exists only while open, the
//! user input that opened one, popups reached by name, and the permissions around them.

mod common;

use common::*;
use rsb_core::element::ElementKind;
use rsb_core::hit::ElementEvent;
use rsb_core::msg::{InstanceSpec, MonitorRef, ScriptIn, WidgetState};
use rsb_core::popup::{CloseReason, Placement, PopupAction};

fn popup_of(h: &Harness, id: u32) -> rsb_core::element::Element {
    find_kind(h.root(id).expect("a tree"), ElementKind::Popup).expect("a popup element").clone()
}

fn props(h: &Harness, id: u32) -> rsb_core::popup::PopupProps {
    *popup_of(h, id).popup.expect("popup props")
}

fn two_monitors() -> Vec<InstanceSpec> {
    let mut a = Harness::spec(1, "pp");
    a.monitor = MonitorRef { index: 0, handle: 11, device: "A".into(), primary: true, scale: 1.0 };
    let mut b = Harness::spec(2, "pp");
    b.monitor = MonitorRef { index: 1, handle: 22, device: "B".into(), primary: false, scale: 1.0 };
    vec![a, b]
}

const BASIC: &str = r#"
    local shown = rsb.state(false)
    return rsb.row {
        rsb.text "anchor",
        on_click = function() shown.value = true end,
        rsb.popup {
            open = shown,
            class = "panel",
            on_close = function(reason) rsb.log.info("closed " .. reason) end,
            function() return { rsb.text "hello" } end,
        },
    }
"#;

#[test]
fn a_popup_is_closed_and_empty_until_a_click_opens_it_with_its_content_in_the_first_tree() {
    let mut h = Harness::new();
    h.user("widgets/pp.luau", &pwidget("{}", BASIC));
    h.start(&[(1, "pp")]);
    let closed = props(&h, 1);
    assert!(!closed.open);
    assert_eq!(closed.open_seq, 0);
    assert!(popup_of(&h, 1).children.is_empty(), "no content while closed");
    assert_eq!(popup_of(&h, 1).classes.iter().map(|c| &**c).collect::<Vec<_>>(), ["panel"]);

    let click = first_click(h.root(1).unwrap()).unwrap();
    h.click_gesture(1, click, 77);
    let open = props(&h, 1);
    assert!(open.open);
    assert_eq!(open.open_seq, 1);
    assert_eq!(open.gesture, 77, "the click's input id travels with the open_seq");
    let content = popup_of(&h, 1);
    let mut texts = Vec::new();
    collect_text(&content, &mut texts);
    assert_eq!(texts, ["hello"], "the content is in the first tree that has the new open_seq");
}

#[test]
fn the_ui_closing_a_popup_writes_false_then_calls_on_close_with_the_reason() {
    let mut h = Harness::new();
    h.user("widgets/pp.luau", &pwidget("{}", BASIC));
    h.start(&[(1, "pp")]);
    let click = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, click);
    let on_close = props(&h, 1).on_close.expect("a close handler");
    h.send(element_call(1, on_close, ElementEvent::PopupClosed(CloseReason::Escape), 0));
    let p = props(&h, 1);
    assert!(!p.open, "the open state was written false");
    assert_eq!(p.open_seq, 1, "closing does not move the count");
    assert!(popup_of(&h, 1).children.is_empty(), "the content is gone with the popup");
    assert_eq!(log_lines("pp: closed escape").len(), 1);

    // Opening again is a new open_seq.
    h.click(1, click);
    assert_eq!(props(&h, 1).open_seq, 2);
}

#[test]
fn a_popup_given_a_function_only_hears_on_close() {
    let mut h = Harness::new();
    h.user(
        "widgets/pp.luau",
        &pwidget(
            "{}",
            r#"
    local mode = rsb.state("a")
    return rsb.row {
        rsb.text "x",
        on_click = function() mode.value = "b" end,
        rsb.popup {
            open = function() return mode.value == "b" end,
            on_close = function(reason) mode.value = "a" rsb.log.info("fn-close " .. reason) end,
            rsb.text "body",
        },
    }
"#,
        ),
    );
    h.start(&[(1, "pp")]);
    let click = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, click);
    assert!(props(&h, 1).open);
    let on_close = props(&h, 1).on_close.unwrap();
    h.send(element_call(1, on_close, ElementEvent::PopupClosed(CloseReason::Outside), 0));
    assert!(!props(&h, 1).open, "the widget reacted to on_close");
    assert_eq!(log_lines("fn-close outside").len(), 1);
}

#[test]
fn content_made_while_open_is_disposed_on_close_timers_and_service_reads_included() {
    let mut h = Harness::new();
    h.user(
        "widgets/pp.luau",
        &pwidget(
            "{}",
            r#"
    local shown = rsb.state(false)
    local ticks = rsb.state(0)
    return rsb.row {
        rsb.text { text = function() return "ticks " .. ticks.value end },
        on_click = function() shown.value = not shown.value end,
        rsb.popup {
            open = shown,
            function()
                rsb.timer.every("1s", function() ticks.value = ticks.value + 1 end)
                return { rsb.text { text = function() return "cpu " .. tostring(rsb.services.system.cpu) end } }
            end,
        },
    }
"#,
        ),
    );
    h.start(&[(1, "pp")]);
    let click = first_click(h.root(1).unwrap()).unwrap();
    let subs = |h: &Harness| h.requests().iter().filter(|r| r.contains("Subscribe") && r.contains("System")).count();
    let unsubs =
        |h: &Harness| h.requests().iter().filter(|r| r.contains("Unsubscribe") && r.contains("System")).count();
    h.advance(3000);
    assert_eq!(h.texts(1)[0], "ticks 0", "closed: nothing runs");
    assert_eq!(subs(&h), 0, "closed: no subscription");

    h.click(1, click);
    assert_eq!(subs(&h), 1);
    h.advance(1000);
    h.advance(1000);
    assert_eq!(h.texts(1)[0], "ticks 2");

    h.click(1, click);
    assert_eq!(unsubs(&h), 1, "closing lets the service go");
    h.advance(5000);
    assert_eq!(h.texts(1)[0], "ticks 2", "the timer inside stopped with the content");
    assert!(popup_of(&h, 1).children.is_empty());
    h.click(1, click);
    assert_eq!(subs(&h), 2, "opening again subscribes again");
}

#[test]
fn a_service_the_widget_reads_outside_the_popup_stays_subscribed_when_the_popup_closes() {
    let mut h = Harness::new();
    h.user(
        "widgets/pp.luau",
        &pwidget(
            "{}",
            r#"
    local shown = rsb.state(false)
    return rsb.row {
        rsb.text { text = function() return tostring(rsb.services.system.cpu) end },
        on_click = function() shown.value = not shown.value end,
        rsb.popup { open = shown, function() return { rsb.text { text = function() return tostring(rsb.services.system.memory) end } } end },
    }
"#,
        ),
    );
    h.start(&[(1, "pp")]);
    let click = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, click);
    h.click(1, click);
    assert!(!h.requests().iter().any(|r| r.contains("Unsubscribe")), "{:?}", h.requests());
}

#[test]
fn only_a_click_handler_before_its_first_yield_records_a_gesture() {
    let mut h = Harness::new();
    h.user(
        "widgets/pp.luau",
        &pwidget(
            "{}",
            r#"
    local by_click = rsb.state(false)
    local by_timer = rsb.state(false)
    local after_wait = rsb.state(false)
    local by_effect = rsb.state(false)
    rsb.timer.after("1s", function() by_timer.value = true end)
    rsb.effect(function() if by_timer.value then by_effect.value = true end end)
    return rsb.row {
        rsb.text "x",
        on_click = function()
            by_click.value = true
            rsb.task.sleep("100ms")
            after_wait.value = true
        end,
        rsb.popup { key = "click", open = by_click, rsb.text "a" },
        rsb.popup { key = "timer", open = by_timer, rsb.text "b" },
        rsb.popup { key = "wait", open = after_wait, rsb.text "c" },
        rsb.popup { key = "effect", open = by_effect, rsb.text "d" },
    }
"#,
        ),
    );
    h.start(&[(1, "pp")]);
    let click = first_click(h.root(1).unwrap()).unwrap();
    h.click_gesture(1, click, 9);
    h.advance(1200);
    let mut all = Vec::new();
    find_all(h.root(1).unwrap(), ElementKind::Popup, &mut all);
    let gesture = |i: usize| all[i].popup.as_ref().unwrap().gesture;
    let open = |i: usize| all[i].popup.as_ref().unwrap().open;
    assert!(open(0) && open(1) && open(2) && open(3));
    assert_eq!(gesture(0), 9, "written in the click handler");
    assert_eq!(gesture(1), 0, "written by a timer");
    assert_eq!(gesture(2), 0, "written after the handler waited");
    assert_eq!(gesture(3), 0, "written by an effect a timer woke");
}

#[test]
fn a_key_handler_is_a_gesture_too() {
    let mut h = Harness::new();
    h.user(
        "widgets/pp.luau",
        &pwidget(
            "{}",
            r#"
    local shown = rsb.state(false)
    return rsb.row {
        rsb.text "x",
        on_key = function(e) if e.key == "Enter" then shown.value = true end end,
        rsb.popup { open = shown, rsb.text "a" },
    }
"#,
        ),
    );
    h.start(&[(1, "pp")]);
    let key = h.root(1).unwrap().handlers.key.expect("a key handler");
    let enter =
        rsb_core::key::KeyEvent { key: rsb_core::key::Key::Enter, mods: Default::default(), down: true, repeat: false };
    h.send(element_call(1, key, ElementEvent::Key(enter), 31));
    assert_eq!(props(&h, 1).gesture, 31);
}

#[test]
fn rsb_popup_by_name_reaches_the_popup_on_the_asked_monitor_and_the_primary_by_default() {
    let mut h = Harness::new();
    h.user(
        "widgets/pp.luau",
        &pwidget(
            "{}",
            r#"
    local shown = rsb.state(false)
    return rsb.row {
        rsb.text "x",
        rsb.popup { open = shown, name = "tools", on_open = function(n) rsb.log.info("opened as " .. n) end, rsb.text "a" },
    }
"#,
        ),
    );
    h.start_specs(two_monitors());
    let is_open = |h: &Harness, id: u32| props(h, id).open;
    h.send(ScriptIn::Popup { name: "tools".into(), action: PopupAction::Open, monitor: Some(22), gesture: 5 });
    assert!(!is_open(&h, 1) && is_open(&h, 2));
    assert_eq!(props(&h, 2).gesture, 5, "the command's gesture is the popup's");
    h.send(ScriptIn::Popup { name: "tools".into(), action: PopupAction::Close, monitor: Some(22), gesture: 0 });
    assert!(!is_open(&h, 2));
    h.send(ScriptIn::Popup { name: "tools".into(), action: PopupAction::Toggle, monitor: None, gesture: 6 });
    assert!(is_open(&h, 1) && !is_open(&h, 2), "no monitor means the primary one");
    h.send(ScriptIn::Popup { name: "tools".into(), action: PopupAction::Toggle, monitor: None, gesture: 0 });
    assert!(!is_open(&h, 1));
    assert!(log_lines("pp: opened as tools").len() >= 2, "on_open got the name");
}

#[test]
fn a_popup_with_several_names_answers_to_each_and_on_open_gets_the_one_used() {
    let mut h = Harness::new();
    h.user(
        "widgets/pp.luau",
        &pwidget(
            "{}",
            r#"
    local shown = rsb.state(false)
    return rsb.row {
        rsb.text "x",
        rsb.popup {
            open = shown,
            name = { "wide-panel", "side-panel" },
            on_open = function(n) rsb.log.info("page for " .. n) end,
            rsb.text "a",
        },
    }
"#,
        ),
    );
    h.start(&[(1, "pp")]);
    assert_eq!(props(&h, 1).names.iter().map(|n| &**n).collect::<Vec<_>>(), ["wide-panel", "side-panel"]);
    h.send(ScriptIn::Popup { name: "side-panel".into(), action: PopupAction::Open, monitor: None, gesture: 1 });
    h.send(ScriptIn::Popup { name: "wide-panel".into(), action: PopupAction::Open, monitor: None, gesture: 2 });
    assert_eq!(log_lines("pp: page for side-panel").len(), 1);
    assert_eq!(log_lines("pp: page for wide-panel").len(), 1, "an open popup told to open again still hears its name");
}

#[test]
fn a_built_in_name_in_a_user_widget_is_dropped_with_one_warning() {
    let mut h = Harness::new();
    h.user(
        "widgets/pp.luau",
        &pwidget(
            "{}",
            r#"
    local a, b = rsb.state(false), rsb.state(false)
    return rsb.row {
        rsb.text "x",
        rsb.popup { key = "a", open = a, name = { "launcher", "mine" }, rsb.text "a" },
        rsb.popup { key = "b", open = b, name = "launcher", rsb.text "b" },
    }
"#,
        ),
    );
    h.start(&[(1, "pp")]);
    let mut all = Vec::new();
    find_all(h.root(1).unwrap(), ElementKind::Popup, &mut all);
    assert_eq!(all[0].popup.as_ref().unwrap().names.iter().map(|n| &**n).collect::<Vec<_>>(), ["mine"]);
    assert!(all[1].popup.as_ref().unwrap().names.is_empty(), "the second popup has no name");
    assert_eq!(h.state_of(1), WidgetState::Running, "an unnamed popup needs no state");
    let warned = log_lines("popup name 'launcher' belongs to a built-in widget; this popup has no name");
    assert_eq!(warned.len(), 1, "{warned:?}");
}

#[test]
fn a_user_copy_of_a_built_in_widget_keeps_the_popup_names_of_that_widget_and_no_others() {
    let mut h = Harness::new();
    h.user(
        "widgets/dashboard.luau",
        &pwidget(
            "{}",
            r#"
    local a, b = rsb.state(false), rsb.state(false)
    return rsb.row {
        rsb.text "x",
        rsb.popup { key = "a", open = a, name = { "dashboard", "launcher" }, rsb.text "a" },
        rsb.popup { key = "b", open = b, name = "power", rsb.text "b" },
    }
"#,
        ),
    );
    h.start(&[(1, "dashboard")]);
    let mut all = Vec::new();
    find_all(h.root(1).unwrap(), ElementKind::Popup, &mut all);
    assert_eq!(
        all[0].popup.as_ref().unwrap().names.iter().map(|n| &**n).collect::<Vec<_>>(),
        ["dashboard", "launcher"],
        "the copy replaces the built-in dashboard, so it has its names"
    );
    assert!(all[1].popup.as_ref().unwrap().names.is_empty(), "the power popup belongs to another widget");
}

#[test]
fn a_user_widget_may_open_a_built_in_popup_only_inside_a_click() {
    let mut h = Harness::new();
    h.builtin(
        "widgets/panelx.luau",
        &pwidget(
            "{}",
            r#"
    local shown = rsb.state(false)
    return rsb.row { rsb.text "built-in", rsb.popup { open = shown, name = "launcher", rsb.text "l" } }
"#,
        ),
    );
    h.user(
        "widgets/pp.luau",
        &pwidget(
            "{}",
            r#"
    rsb.timer.after("1s", function() pcall(rsb.popups.open, "launcher") end)
    return rsb.row {
        rsb.text "x",
        on_click = function() rsb.popups.toggle("launcher") end,
        on_right_click = function() rsb.popups.close("launcher") end,
    }
"#,
        ),
    );
    h.start(&[(1, "panelx"), (2, "pp")]);
    h.advance(1500);
    assert!(!props(&h, 1).open, "a timer of a user widget cannot open it");
    let click = first_click(h.root(2).unwrap()).unwrap();
    h.click_gesture(2, click, 40);
    assert!(props(&h, 1).open);
    assert_eq!(props(&h, 1).gesture, 40, "the opening click's id carries over to the built-in's popup");
    h.click_gesture(2, click, 41);
    assert!(!props(&h, 1).open, "toggle closes it");
    assert_eq!(h.state_of(2), WidgetState::Running);
}

#[test]
fn another_user_widgets_popup_needs_the_popups_permission() {
    let owner = pwidget(
        "{}",
        r#"
    local shown = rsb.state(false)
    return rsb.row { rsb.text "owner", rsb.popup { open = shown, name = "notes", rsb.text "n" } }
"#,
    );
    let caller = |perms: &str| {
        pwidget(
            perms,
            r#"
    return rsb.row { rsb.text "caller", on_click = function() rsb.popups.open("notes") end }
"#,
        )
    };
    let mut h = Harness::new();
    h.user("widgets/owner.luau", &owner);
    h.user("widgets/caller.luau", &caller("{}"));
    h.start(&[(1, "owner"), (2, "caller")]);
    let click = first_click(h.root(2).unwrap()).unwrap();
    h.click(2, click);
    assert!(!props(&h, 1).open);
    assert_eq!(h.plate(2).as_deref(), Some("PERMISSION - caller needs popups:notes; add it to permissions"));

    let mut h = Harness::new();
    h.user("widgets/owner.luau", &owner);
    h.user("widgets/caller.luau", &caller("{ popups = { 'notes' } }"));
    h.start(&[(1, "owner"), (2, "caller")]);
    let click = first_click(h.root(2).unwrap()).unwrap();
    h.click(2, click);
    assert!(props(&h, 1).open, "declared: allowed");
}

#[test]
fn a_widget_may_open_its_own_popup_without_a_click() {
    let mut h = Harness::new();
    h.user(
        "widgets/pp.luau",
        &pwidget(
            "{}",
            r#"
    local shown = rsb.state(false)
    rsb.timer.after("1s", function() rsb.popups.open("note") end)
    return rsb.row { rsb.text "x", rsb.popup { open = shown, name = "note", rsb.text "n" } }
"#,
        ),
    );
    h.start(&[(1, "pp")]);
    h.advance(1500);
    assert!(props(&h, 1).open);
    assert_eq!(props(&h, 1).gesture, 0, "no click: it takes no focus");
}

#[test]
fn a_named_popup_without_a_state_is_a_load_error() {
    let mut h = Harness::new();
    h.user(
        "widgets/pp.luau",
        &pwidget(
            "{}",
            r#"return rsb.row { rsb.text "x", rsb.popup { open = function() return false end, name = "tools", rsb.text "a" } }"#,
        ),
    );
    h.start(&[(1, "pp")]);
    let plate = h.plate(1).expect("a plate");
    assert!(
        plate.contains("a named popup needs open = rsb.state(false) so rsb popup and rsb.popups can change it"),
        "{plate}"
    );
}

#[test]
fn a_function_open_without_on_close_warns() {
    let mut h = Harness::new();
    h.user(
        "widgets/pp.luau",
        &pwidget(
            "{}",
            r#"return rsb.row { rsb.text "x", rsb.popup { open = function() return false end, rsb.text "a" } }"#,
        ),
    );
    h.start(&[(1, "pp")]);
    assert_eq!(
        log_lines(
            "the UI closes this popup but cannot change its open function; add on_close and update what open reads"
        )
        .len(),
        1
    );
}

#[test]
fn an_overlay_opened_outside_a_click_is_written_back_closed_and_refused() {
    let body = r#"
    local shown = rsb.state(false)
    rsb.timer.after("1s", function() shown.value = true end)
    return rsb.row {
        rsb.text "x",
        on_click = function() shown.value = true end,
        rsb.popup {
            open = shown,
            placement = "screen",
            on_close = function(reason) rsb.log.info("overlay " .. reason) end,
            rsb.text "menu",
        },
    }
"#;
    let mut h = Harness::new();
    h.user("widgets/pp.luau", &pwidget("{ overlay = true }", body));
    h.start(&[(1, "pp")]);
    h.advance(1500);
    let p = props(&h, 1);
    assert!(!p.open, "the timer cannot open an overlay");
    assert_eq!(p.placement, Placement::Screen);
    assert_eq!(log_lines("pp: overlay refused").len(), 1, "on_close('refused') ran");
    assert_eq!(log_lines("an overlay only opens from a click or a key").len(), 1);
    let click = first_click(h.root(1).unwrap()).unwrap();
    h.click_gesture(1, click, 3);
    assert!(props(&h, 1).open, "a click opens it");
    assert_eq!(props(&h, 1).gesture, 3);
}

#[test]
fn placement_screen_needs_the_overlay_permission() {
    let mut h = Harness::new();
    h.user(
        "widgets/pp.luau",
        &pwidget(
            "{}",
            r#"return rsb.row { rsb.text "x", rsb.popup { open = rsb.state(false), placement = "screen", rsb.text "a" } }"#,
        ),
    );
    h.start(&[(1, "pp")]);
    let plate = h.plate(1).expect("a plate");
    assert!(plate.contains("needs overlay = true in the widget's permissions"), "{plate}");
}

#[test]
fn is_open_follows_a_named_popup_across_widgets_on_one_monitor_only() {
    let mut h = Harness::new();
    h.builtin(
        "widgets/panelx.luau",
        &pwidget(
            "{}",
            r#"
    local shown = rsb.state(false)
    return rsb.row { rsb.text "built-in", rsb.popup { open = shown, name = "power", rsb.text "p" } }
"#,
        ),
    );
    h.user(
        "widgets/pp.luau",
        &pwidget(
            "{}",
            r#"return rsb.text { text = function() return rsb.popups.is_open("power") and "on" or "off" end }"#,
        ),
    );
    let mut a = Harness::spec(1, "panelx");
    a.monitor.handle = 11;
    let mut b = Harness::spec(2, "pp");
    b.monitor.handle = 11;
    let mut c = Harness::spec(3, "pp");
    c.monitor.handle = 22;
    c.monitor.index = 1;
    c.monitor.primary = false;
    h.start_specs(vec![a, b, c]);
    assert_eq!((h.texts(2)[0].as_str(), h.texts(3)[0].as_str()), ("off", "off"));
    h.send(ScriptIn::Popup { name: "power".into(), action: PopupAction::Open, monitor: Some(11), gesture: 1 });
    assert_eq!((h.texts(2)[0].as_str(), h.texts(3)[0].as_str()), ("on", "off"));
    h.send(ScriptIn::Popup { name: "power".into(), action: PopupAction::Close, monitor: Some(11), gesture: 0 });
    assert_eq!(h.texts(2)[0], "off");
}

#[test]
fn a_stopped_widgets_popups_are_gone_for_everyone() {
    let mut h = Harness::new();
    h.user(
        "widgets/pp.luau",
        &pwidget(
            "{}",
            r#"
    local shown = rsb.state(true)
    return rsb.row {
        rsb.text "x",
        on_click = function() rsb.open_url("https://elsewhere.example") end,
        rsb.popup { open = shown, name = "tools", rsb.text "a" },
    }
"#,
        ),
    );
    h.user(
        "widgets/reader.luau",
        &pwidget(
            "{}",
            r#"return rsb.text { text = function() return rsb.popups.is_open("tools") and "on" or "off" end }"#,
        ),
    );
    h.start(&[(1, "pp"), (2, "reader")]);
    assert_eq!(h.texts(2)[0], "on");
    let click = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, click);
    assert!(matches!(h.state_of(1), WidgetState::Stopped(_)), "denied: the widget stopped");
    assert_eq!(h.texts(2)[0], "off", "its popup is gone with it");
}

#[test]
fn at_most_sixteen_popups_per_widget() {
    let mut h = Harness::new();
    h.user(
        "widgets/pp.luau",
        &pwidget(
            "{}",
            r#"
    local kids = {}
    for i = 1, 17 do kids[i] = rsb.popup { key = "p" .. i, open = false, rsb.text "a" } end
    return rsb.row(kids)
"#,
        ),
    );
    h.start(&[(1, "pp")]);
    let plate = h.plate(1).expect("a plate");
    assert!(plate.contains("too many popups (at most 16 per widget)"), "{plate}");
}

#[test]
fn a_popup_prop_on_an_element_says_to_make_a_popup_child_and_bad_popup_props_name_the_prop() {
    let mut h = Harness::new();
    h.user(
        "widgets/pp.luau",
        &pwidget(
            "{}",
            r#"
    local out = {}
    local function t(f: () -> any) local ok, e = pcall(f) table.insert(out, tostring(e)) end
    t(function() return rsb.row { popup = 1 } end)
    t(function() return rsb.popup { rsb.text "a" } end)
    t(function() return rsb.popup { open = 5 } end)
    t(function() return rsb.popup { open = true, placement = "middle" } end)
    t(function() return rsb.popup { open = true, name = "Bad Name" } end)
    t(function() return rsb.popup { open = true, dismiss = { outsde = true } } end)
    return rsb.text(table.concat(out, "@@"))
"#,
        ),
    );
    h.start(&[(1, "pp")]);
    let text = h.texts(1).join("");
    let lines: Vec<&str> = text.split("@@").collect();
    assert!(
        lines[0].ends_with("make a popup with rsb.popup { ... } as a child of the element it opens from"),
        "{}",
        lines[0]
    );
    assert!(lines[1].ends_with("rsb.popup needs open = rsb.state(false)"), "{}", lines[1]);
    assert!(lines[2].ends_with("open must be a boolean, a state or a function, got number"), "{}", lines[2]);
    assert!(lines[3].contains("placement must be one of"), "{}", lines[3]);
    assert!(lines[4].ends_with("popup names use a-z, 0-9, _ and -, at most 32 characters"), "{}", lines[4]);
    assert!(lines[5].ends_with("dismiss has only outside and escape, not outsde"), "{}", lines[5]);
}

#[test]
fn a_popup_key_handler_sits_on_the_popup_root_and_gets_the_key_table() {
    let mut h = Harness::new();
    h.user(
        "widgets/pp.luau",
        &pwidget(
            // `overlay` makes the widget trusted: the dismiss rules it writes are kept for a popup it opened itself.
            "{ overlay = true }",
            r#"
    local shown = rsb.state(true)
    return rsb.row {
        rsb.text "x",
        rsb.popup {
            open = shown,
            on_key = function(e) rsb.log.info(`key {e.key} ctrl={e.ctrl} shift={e.shift} alt={e.alt}`) end,
            dismiss = { escape = false },
            focus = "always",
            align = "end",
            offset = -4,
            attach = true,
            rsb.text "a",
        },
    }
"#,
        ),
    );
    h.start(&[(1, "pp")]);
    let p = props(&h, 1);
    assert!(!p.dismiss.escape && p.dismiss.outside);
    assert_eq!(p.align, rsb_core::popup::PopupAlign::End);
    assert_eq!(p.offset, -4.0);
    assert!(p.attach);
    assert_eq!(p.focus, rsb_core::popup::FocusMode::Always);
    let key = popup_of(&h, 1).handlers.key.expect("on_key on the root");
    let ev = rsb_core::key::KeyEvent {
        key: rsb_core::key::Key::ArrowDown,
        mods: rsb_core::key::Modifiers { ctrl: true, shift: false, alt: true, meta: false },
        down: true,
        repeat: false,
    };
    h.send(element_call(1, key, ElementEvent::Key(ev), 4));
    assert_eq!(log_lines("pp: key ArrowDown ctrl=true shift=false alt=true").len(), 1);
    let up = rsb_core::key::KeyEvent { down: false, ..ev };
    h.send(element_call(1, key, ElementEvent::Key(up), 5));
    assert_eq!(log_lines("pp: key ArrowDown ctrl=true shift=false alt=true").len(), 1, "key ups do not reach on_key");
}

#[test]
fn a_popup_opened_event_from_the_ui_calls_on_open_with_the_name() {
    let mut h = Harness::new();
    h.user(
        "widgets/pp.luau",
        &pwidget(
            "{}",
            r#"
    local shown = rsb.state(false)
    return rsb.row {
        rsb.text "x",
        rsb.popup { open = shown, name = "tools", on_open = function(n) rsb.log.info("ui opened " .. n) end, rsb.text "a" },
    }
"#,
        ),
    );
    h.start(&[(1, "pp")]);
    let on_open = props(&h, 1).on_open.expect("an open handler");
    h.send(element_call(1, on_open, ElementEvent::PopupOpened(std::sync::Arc::from("tools")), 0));
    assert_eq!(log_lines("pp: ui opened tools").len(), 1);
}

const SHUT_IN: &str = r#"
    local shown = rsb.state(true)
    return rsb.row {
        rsb.text "anchor",
        on_click = function() shown.value = true end,
        rsb.popup {
            open = shown,
            dismiss = { outside = false, escape = false },
            function() return { rsb.text "hello" } end,
        },
    }
"#;

#[test]
fn a_popup_nobody_asked_for_can_always_be_dismissed_unless_the_widget_may_cover_the_screen() {
    // Open from the start (no click or key): the widget's wish to be undismissable is ignored.
    let mut h = Harness::new();
    h.user("widgets/pp.luau", &pwidget("{}", SHUT_IN));
    h.start(&[(1, "pp")]);
    let p = props(&h, 1);
    assert!(p.open && p.gesture == 0 && !p.trusted);
    assert!(p.dismiss.outside && p.dismiss.escape, "a timer-opened popup can always be dismissed: {:?}", p.dismiss);
    // A widget with overlay may turn both off without a gesture, like a built-in.
    let mut h3 = Harness::new();
    h3.user("widgets/pp.luau", &pwidget("{ overlay = true }", SHUT_IN));
    h3.start(&[(1, "pp")]);
    let p = props(&h3, 1);
    assert!(p.trusted && !p.dismiss.outside && !p.dismiss.escape, "{p:?}");
}

#[test]
fn a_popup_whose_content_fails_shows_what_broke_and_the_bar_plate_counts_it() {
    let mut h = Harness::new();
    h.user(
        "widgets/pp.luau",
        &pwidget(
            "{}",
            r#"
    local shown = rsb.state(false)
    return rsb.row {
        rsb.text "anchor",
        on_click = function() shown.value = true end,
        rsb.popup {
            open = shown,
            function()
                local missing = nil
                return { rsb.text(missing.field) }
            end,
        },
    }
"#,
        ),
    );
    h.start(&[(1, "pp")]);
    let click = first_click(h.root(1).unwrap()).unwrap();
    h.click_gesture(1, click, 9);
    h.advance(50);
    let content = popup_of(&h, 1);
    let mut texts = Vec::new();
    collect_text(&content, &mut texts);
    let plate = content.children.iter().find(|c| c.kind == ElementKind::Plate).map(|c| c.text.clone());
    let text = plate.flatten().map(|t| t.to_string()).unwrap_or_default();
    assert!(text.starts_with("WIDGET ERROR - pp.luau:") && text.ends_with("(see log)"), "{text:?} in {texts:?}");
    assert!(!text.contains('\\') && !text.contains("C:"), "no path: {text:?}");
    let (_, note) = h.error_notes.first().expect("the bar plate is told");
    let note = note.as_ref().expect("a note, not its end");
    assert!(note.plate_text().starts_with("RSB ERROR - pp.luau:"), "{}", note.plate_text());
    // Ten seconds on the error state ends, and so does the note.
    h.advance(10_500);
    assert!(h.error_notes.iter().any(|(_, n)| n.is_none()), "{:?}", h.error_notes);
}

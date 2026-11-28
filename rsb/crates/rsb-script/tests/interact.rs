//! Menus, tooltips, inputs, sliders, key / scroll / focus handlers, animation, SVG icons and fuzzy search as the
//! script API makes them.

mod common;

use std::sync::Arc;

use common::*;
use rsb_core::element::{Element, ElementKind, IconRef, NavAxis};
use rsb_core::hit::{Button, ElementEvent, HandlerCall, InputKind};
use rsb_core::msg::ScriptIn;
use rsb_core::popup::{Placement, PopupProps};

fn kids(e: &Element, kind: ElementKind) -> Vec<&Element> {
    e.children.iter().filter(|c| c.kind == kind).collect()
}

fn class_of(e: &Element) -> Vec<&str> {
    e.classes.iter().map(|c| &**c).collect()
}

fn right_click(h: &mut Harness, id: u32, handler: u32, x: f32, y: f32, gesture: u64) {
    h.send(ScriptIn::Input(HandlerCall {
        instance: rsb_core::element::InstanceId(id),
        handler: rsb_core::element::HandlerId(handler),
        kind: InputKind::Click(Button::Right),
        x,
        y,
        gesture,
    }));
}

// ---------------------------------------------------------------- menus

const MENU: &str = r#"
    local open = rsb.state(false)
    return rsb.row {
        rsb.text "m",
        on_click = function() open.value = true end,
        rsb.menu {
            open = open,
            items = {
                { label = "Reload", shortcut = "Ctrl+R", icon = "cpu", on_click = function() rsb.log.info("reload picked") end },
                { separator = true },
                { label = "Mode", checked = true, items = { { label = "A" }, { label = "B", disabled = true } } },
                { label = "Gone", disabled = true, svg = "power" },
            },
        },
    }
"#;

#[test]
fn a_menu_is_built_from_its_items_only_while_open_with_every_kind_of_entry() {
    let mut h = Harness::new();
    h.user("widgets/mn.luau", &pwidget("{}", MENU));
    h.start(&[(1, "mn")]);
    let menu = find_kind(h.root(1).unwrap(), ElementKind::Menu).unwrap();
    assert!(menu.children.is_empty(), "closed: no items");
    assert_eq!(&*menu.popup.as_ref().unwrap().group, "menu");
    assert_eq!(menu.interaction.nav, NavAxis::Vertical, "menus move focus up and down");

    let click = first_click(h.root(1).unwrap()).unwrap();
    h.click_gesture(1, click, 12);
    let menu = find_kind(h.root(1).unwrap(), ElementKind::Menu).unwrap();
    let p: &PopupProps = menu.popup.as_ref().unwrap();
    assert!(p.open && p.open_seq == 1 && p.gesture == 12);
    assert_eq!(menu.children.len(), 4);
    let reload = &menu.children[0];
    assert_eq!((reload.kind, reload.text.as_deref()), (ElementKind::MenuItem, Some("Reload")));
    assert_eq!(reload.icon, Some(IconRef::Named(Arc::from("cpu"))));
    assert!(reload.handlers.click.is_some());
    let shortcut = &reload.children[0];
    assert_eq!((shortcut.text.as_deref(), class_of(shortcut)), (Some("Ctrl+R"), vec!["shortcut"]));
    assert_eq!((menu.children[1].kind, class_of(&menu.children[1])), (ElementKind::Box, vec!["separator"]));
    let mode = &menu.children[2];
    assert_eq!(mode.interaction.checked, Some(true));
    let sub = &kids(mode, ElementKind::Menu)[0];
    assert_eq!(sub.popup.as_ref().unwrap().placement, Placement::Right);
    assert_eq!(&*sub.popup.as_ref().unwrap().group, "menu");
    assert_eq!(sub.children.iter().map(|c| c.text.as_deref().unwrap()).collect::<Vec<_>>(), ["A", "B"]);
    assert!(sub.children[1].interaction.disabled);
    let gone = &menu.children[3];
    assert!(gone.interaction.disabled);
    assert_eq!(gone.icon, Some(IconRef::Svg(Arc::from("power"))));

    // Picking an item runs its on_click.
    let on_click = reload.handlers.click.unwrap().0;
    h.click(1, on_click);
    assert_eq!(log_lines("mn: reload picked").len(), 1);
}

#[test]
fn menus_are_capped_at_256_items_and_4_levels() {
    let mut h = Harness::new();
    h.user(
        "widgets/mn.luau",
        &pwidget(
            "{}",
            r#"
    local items = {}
    for i = 1, 257 do items[i] = { label = "i" .. i } end
    local deep = { label = "x" }
    local cur = deep
    for i = 1, 5 do cur.items = { { label = "l" .. i } } cur = cur.items[1] end
    return rsb.row {
        rsb.text "x",
        rsb.menu { key = "big", open = true, items = items },
        rsb.menu { key = "deep", open = true, items = { deep } },
        rsb.menu { key = "ok", open = true, items = { { label = "a", items = { { label = "b", items = { { label = "c", items = { { label = "d" } } } } } } } } },
    }
"#,
        ),
    );
    h.start(&[(1, "mn")]);
    assert_eq!(log_lines("mn: error: a menu has at most 256 items").len(), 1, "{:?}", log_lines("mn"));
    assert_eq!(log_lines("mn: error: menus nest at most 4 levels").len(), 1);
    let mut menus = Vec::new();
    find_all(h.root(1).unwrap(), ElementKind::Menu, &mut menus);
    let top: Vec<&&Element> = menus.iter().filter(|m| m.popup.as_ref().unwrap().open).collect();
    assert_eq!(top.len(), 3);
    assert!(top[0].children.is_empty() && top[1].children.is_empty(), "the refused menus draw nothing");
    assert_eq!(top[2].children.len(), 1, "four levels are fine");
}

#[test]
fn the_menu_prop_opens_at_the_pointer_on_a_right_click_and_refuses_on_right_click() {
    let mut h = Harness::new();
    h.user(
        "widgets/mn.luau",
        &pwidget("{}", r#"return rsb.row { rsb.text "x", menu = { { label = "Copy" }, { label = "Paste" } } }"#),
    );
    h.start(&[(1, "mn")]);
    let root = h.root(1).unwrap();
    let menu = find_kind(root, ElementKind::Menu).unwrap();
    assert!(!menu.popup.as_ref().unwrap().open);
    let handler = root.handlers.right_click.expect("the runtime listens for the right click").0;
    right_click(&mut h, 1, handler, 7.5, 3.0, 20);
    let menu = find_kind(h.root(1).unwrap(), ElementKind::Menu).unwrap();
    let p = menu.popup.as_ref().unwrap();
    assert!(p.open);
    assert_eq!(p.placement, Placement::Pointer { x: 7.5, y: 3.0 });
    assert_eq!(p.gesture, 20);
    assert_eq!(menu.children.len(), 2);

    let mut h = Harness::new();
    h.user(
        "widgets/mn2.luau",
        &pwidget(
            "{}",
            r#"return rsb.row { rsb.text "x", menu = { { label = "Copy" } }, on_right_click = function() end }"#,
        ),
    );
    h.start(&[(1, "mn2")]);
    let plate = h.plate(1).expect("a plate");
    assert!(plate.contains("an element cannot have both menu and on_right_click"), "{plate}");
}

// ---------------------------------------------------------------- tooltips

#[test]
fn a_tooltip_is_a_child_with_a_text_for_a_string_and_the_element_for_an_element() {
    let mut h = Harness::new();
    h.user(
        "widgets/tt.luau",
        &pwidget(
            "{}",
            r#"
    local n = rsb.state(1)
    return rsb.row {
        rsb.text "a",
        on_click = function() n.value = 2 end,
        tooltip = function() return "count " .. n.value end,
        rsb.row { key = "second", rsb.text "b", tooltip = rsb.column { rsb.text "l1", rsb.text "l2" } },
        rsb.row { key = "plain", rsb.text "c", tooltip = "plain tip" },
    }
"#,
        ),
    );
    h.start(&[(1, "tt")]);
    let root = h.root(1).unwrap();
    let tips = kids(root, ElementKind::Tooltip);
    assert_eq!(tips.len(), 1);
    assert_eq!(tips[0].children[0].text.as_deref(), Some("count 1"));
    let p = tips[0].popup.as_ref().unwrap();
    assert_eq!(&*p.group, "tooltip");
    assert_eq!(p.focus, rsb_core::popup::FocusMode::None);
    assert!(!p.dismiss.outside);
    let second = root.children.iter().find(|c| c.key.as_deref() == Some("second")).unwrap();
    let tip = &kids(second, ElementKind::Tooltip)[0];
    assert_eq!(tip.children[0].direction, rsb_core::style::FlexDirection::Column);
    assert_eq!(tip.children[0].children.len(), 2);
    let plain = root.children.iter().find(|c| c.key.as_deref() == Some("plain")).unwrap();
    assert_eq!(kids(plain, ElementKind::Tooltip)[0].children[0].text.as_deref(), Some("plain tip"));

    let click = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, click);
    let tips = kids(h.root(1).unwrap(), ElementKind::Tooltip);
    assert_eq!(tips[0].children[0].text.as_deref(), Some("count 2"), "a function tooltip follows what it reads");
}

// ---------------------------------------------------------------- inputs and sliders

#[test]
fn an_input_value_is_two_way_and_value_seq_counts_only_the_scripts_writes() {
    let mut h = Harness::new();
    h.user(
        "widgets/in.luau",
        &pwidget(
            "{}",
            r#"
    local q = rsb.state("")
    return rsb.column {
        on_click = function() q.value = "" end,
        rsb.input {
            value = q,
            placeholder = "Search",
            max_length = 10,
            autofocus = true,
            on_change = function(t) rsb.log.info("changed " .. t) end,
            on_submit = function(t) rsb.log.info("submit " .. t) end,
        },
        rsb.text { text = function() return "q=" .. q.value end },
    }
"#,
        ),
    );
    h.start(&[(1, "in")]);
    let input = |h: &Harness| find_kind(h.root(1).unwrap(), ElementKind::Input).unwrap().clone();
    let i = input(&h);
    let props = i.input.as_ref().unwrap();
    assert_eq!((&*props.value, props.value_seq, props.max_length, props.password), ("", 0, 10, false));
    assert_eq!(props.placeholder.as_deref(), Some("Search"));
    assert!(i.interaction.autofocus);

    let (on_change, on_submit) = (props.on_change.unwrap(), props.on_submit.unwrap());
    h.send(element_call(1, on_change, ElementEvent::TextChanged(Arc::from("ab")), 0));
    assert_eq!(h.texts(1), ["q=ab"], "the typed text reached the state");
    let props = input(&h).input.unwrap();
    assert_eq!((&*props.value, props.value_seq), ("ab", 0), "an echo of what the UI reported is not a new value");
    assert_eq!(log_lines("in: changed ab").len(), 1);
    h.send(element_call(1, on_submit, ElementEvent::Submitted(Arc::from("ab")), 0));
    assert_eq!(log_lines("in: submit ab").len(), 1);

    // The script clears it (a search box after a launch): the UI is told with a newer value_seq.
    let click = first_click(h.root(1).unwrap()).unwrap_or_else(|| h.root(1).unwrap().handlers.click.unwrap().0);
    h.click(1, click);
    let props = input(&h).input.unwrap();
    assert_eq!((&*props.value, props.value_seq), ("", 1));
}

#[test]
fn an_input_given_a_function_value_and_a_password_flag_reports_through_on_change_only() {
    let mut h = Harness::new();
    h.user(
        "widgets/in.luau",
        &pwidget(
            "{}",
            r#"
    return rsb.input {
        value = function() return "fixed" end,
        password = true,
        on_change = function(t) rsb.log.info("pw " .. #t) end,
    }
"#,
        ),
    );
    h.start(&[(1, "in")]);
    let i = h.root(1).unwrap().input.as_ref().unwrap().clone();
    assert!(i.password && &*i.value == "fixed");
    h.send(element_call(1, i.on_change.unwrap(), ElementEvent::TextChanged(Arc::from("abc")), 0));
    assert_eq!(log_lines("in: pw 3").len(), 1);
}

#[test]
fn a_slider_value_is_two_way_with_done_and_a_value_seq() {
    let mut h = Harness::new();
    h.user(
        "widgets/sl.luau",
        &pwidget(
            "{}",
            r#"
    local vol = rsb.state(40)
    return rsb.row {
        on_click = function() vol.value = 90 end,
        rsb.slider {
            value = vol, min = 0, max = 100, step = 5, vertical = true,
            on_change = function(v, done) rsb.log.info(`slid {v} {done}`) end,
        },
        rsb.text { text = function() return "v=" .. vol.value end },
    }
"#,
        ),
    );
    h.start(&[(1, "sl")]);
    let slider = |h: &Harness| *find_kind(h.root(1).unwrap(), ElementKind::Slider).unwrap().slider.clone().unwrap();
    let s = slider(&h);
    assert_eq!((s.value, s.min, s.max, s.step, s.vertical, s.value_seq), (40.0, 0.0, 100.0, 5.0, true, 0));
    let handler = s.on_change.unwrap();
    h.send(element_call(1, handler, ElementEvent::ValueChanged { value: 65.0, done: false }, 0));
    h.send(element_call(1, handler, ElementEvent::ValueChanged { value: 70.0, done: true }, 0));
    assert_eq!(h.texts(1), ["v=70"]);
    assert_eq!((slider(&h).value, slider(&h).value_seq), (70.0, 0));
    assert_eq!(log_lines("sl: slid 65 false").len(), 1);
    assert_eq!(log_lines("sl: slid 70 true").len(), 1);
    let click = h.root(1).unwrap().handlers.click.unwrap().0;
    h.click(1, click);
    assert_eq!((slider(&h).value, slider(&h).value_seq), (90.0, 1));
}

// ---------------------------------------------------------------- common props

#[test]
fn key_scroll_and_focus_handlers_get_their_tables_and_the_interaction_props_reach_the_element() {
    let mut h = Harness::new();
    h.user(
        "widgets/kb.luau",
        &pwidget(
            "{}",
            r#"
    local off = rsb.state(false)
    local idx = rsb.state(1)
    local count = rsb.state(0)
    return rsb.scroll {
        class = "list",
        focusable = true,
        nav = "grid",
        disabled = function() return off.value end,
        checked = true,
        reveal = function() return idx.value end,
        on_key = function(e) rsb.log.info(`key {e.key} {e["repeat"]}`) end,
        on_scroll = function(e) rsb.log.info(`scroll {e.x} {e.y}`) end,
        on_focus = function() rsb.log.info("focused") end,
        on_blur = function() rsb.log.info("blurred") end,
        on_click = function() idx.value = idx.value + 1 off.value = true end,
        rsb.text "row",
    }
"#,
        ),
    );
    h.start(&[(1, "kb")]);
    let root = h.root(1).unwrap().clone();
    assert_eq!(root.kind, ElementKind::Scroll);
    assert_eq!(root.direction, rsb_core::style::FlexDirection::Column);
    let i = root.interaction;
    assert!(i.focusable && !i.disabled && i.checked == Some(true) && i.nav == NavAxis::Grid);
    assert_eq!(i.reveal_seq, 1, "a truthy value asks for the element to be shown");

    let key = rsb_core::key::KeyEvent {
        key: rsb_core::key::Key::PageDown,
        mods: Default::default(),
        down: true,
        repeat: true,
    };
    h.send(element_call(1, root.handlers.key.unwrap(), ElementEvent::Key(key), 1));
    h.send(element_call(1, root.handlers.scroll.unwrap(), ElementEvent::Scrolled { x: 0.0, y: 42.5 }, 0));
    h.send(element_call(1, root.handlers.focus.unwrap(), ElementEvent::Focus, 0));
    h.send(element_call(1, root.handlers.blur.unwrap(), ElementEvent::Blur, 0));
    assert_eq!(log_lines("kb: key PageDown true").len(), 1);
    assert_eq!(log_lines("kb: scroll 0 42.5").len(), 1);
    assert_eq!(log_lines("kb: focused").len(), 1);
    assert_eq!(log_lines("kb: blurred").len(), 1);

    h.click(1, root.handlers.click.unwrap().0);
    let i = h.root(1).unwrap().interaction;
    assert!(i.disabled, "a function prop follows its signal");
    assert_eq!(i.reveal_seq, 2, "a new truthy value asks again");
}

#[test]
fn bad_values_for_the_new_common_props_name_the_prop() {
    let mut h = Harness::new();
    h.user(
        "widgets/bad.luau",
        &pwidget(
            "{}",
            r#"
    local out = {}
    local function t(f: () -> any) local ok, e = pcall(f) table.insert(out, tostring(e)) end
    t(function() return rsb.text { text = "x", nav = "diagonal" } end)
    t(function() return rsb.text { text = "x", focusable = "yes" } end)
    t(function() return rsb.text { text = "x", tooltip = 5 == 6 } end)
    t(function() return rsb.text { text = "x", menu = "no" } end)
    t(function() return rsb.slider {} end)
    t(function() return rsb.menu { open = true } end)
    return rsb.text(table.concat(out, "@@"))
"#,
        ),
    );
    h.start(&[(1, "bad")]);
    let text = h.texts(1).join("");
    let lines: Vec<&str> = text.split("@@").collect();
    assert!(lines[0].contains("nav must be one of"), "{}", lines[0]);
    assert!(lines[1].ends_with("focusable must be true or false"), "{}", lines[1]);
    assert!(lines[2].contains("tooltip must be a string, an element or a function, got boolean"), "{}", lines[2]);
    assert!(lines[3].contains("menu must be a list of menu items, got string"), "{}", lines[3]);
    assert!(lines[4].ends_with("rsb.slider needs a value"), "{}", lines[4]);
    assert!(lines[5].ends_with("rsb.menu needs items"), "{}", lines[5]);
}

// ---------------------------------------------------------------- animation

#[test]
fn animate_restarts_when_its_option_changes_and_turns_options_into_css_text() {
    let mut h = Harness::new();
    h.user(
        "widgets/an.luau",
        &pwidget(
            "{}",
            r#"
    local count = rsb.state(0)
    local steady = rsb.state(0)
    return rsb.row {
        on_click = function() count.value = count.value + 1 end,
        on_right_click = function() steady.value = 1 end,
        animate = function()
            return rsb.animate("flash", { duration = 0.3, easing = "ease-out", iterations = 2, direction = "alternate", fill = "forwards", delay = "50ms", restart = count.value })
        end,
        rsb.text "x",
    }
"#,
        ),
    );
    h.start(&[(1, "an")]);
    let a = |h: &Harness| h.root(1).unwrap().animate.clone().expect("an animation");
    let first = a(&h);
    assert_eq!(&*first.name, "flash");
    assert_eq!(first.duration.as_deref(), Some("300ms"));
    assert_eq!(first.easing.as_deref(), Some("ease-out"));
    assert_eq!(first.iterations.as_deref(), Some("2"));
    assert_eq!(first.direction.as_deref(), Some("alternate"));
    assert_eq!(first.fill.as_deref(), Some("forwards"));
    assert_eq!(first.delay.as_deref(), Some("50ms"));
    assert_eq!(first.restart_seq, 0);
    let click = h.root(1).unwrap().handlers.click.unwrap().0;
    h.click(1, click);
    assert_eq!(a(&h).restart_seq, 1, "a new restart value starts it again");
    let right = h.root(1).unwrap().handlers.right_click.unwrap().0;
    right_click(&mut h, 1, right, 0.0, 0.0, 1);
    assert_eq!(a(&h).restart_seq, 1, "unrelated changes do not");
}

#[test]
fn animate_refuses_unknown_options_with_a_suggestion() {
    let mut h = Harness::new();
    h.user(
        "widgets/an.luau",
        &pwidget(
            "{}",
            r#"
    local ok, e = pcall(function() return rsb.animate("x", { duraton = 1 }) end)
    local ok2, e2 = pcall(function() return rsb.animate("") end)
    return rsb.text(tostring(e) .. "@@" .. tostring(e2))
"#,
        ),
    );
    h.start(&[(1, "an")]);
    let text = h.texts(1).join("");
    assert!(text.contains("unknown animate option 'duraton'; did you mean 'duration'?"), "{text}");
    assert!(text.contains("rsb.animate needs the name of a @keyframes rule"), "{text}");
}

// ---------------------------------------------------------------- icons

#[test]
fn an_svg_icon_is_a_bundled_name_or_a_permitted_path() {
    let mut h = Harness::new();
    h.user(
        "widgets/my/ic.luau",
        &pwidget("{}", r#"return rsb.row { rsb.icon { svg = "power" }, rsb.icon { svg = "art/logo.svg" } }"#),
    );
    h.start(&[(1, "my/ic")]);
    let root = h.root(1).unwrap();
    assert_eq!(root.children[0].icon, Some(IconRef::Svg(Arc::from("power"))));
    let Some(IconRef::Svg(path)) = &root.children[1].icon else { panic!("{:?}", root.children[1].icon) };
    assert!(path.ends_with("art\\logo.svg") && path.contains("tester"), "{path}");

    let mut h = Harness::new();
    h.user("widgets/my/ic.luau", &pwidget("{}", r#"return rsb.icon { svg = "C:/Windows/logo.svg" }"#));
    h.start(&[(1, "my/ic")]);
    assert_eq!(h.plate(1).as_deref(), Some("PERMISSION - my/ic needs file:C:/Windows/logo.svg; add it to permissions"));
}

// ---------------------------------------------------------------- fuzzy search

#[test]
fn fuzzy_with_nothing_to_look_for_keeps_the_order_and_honours_limit_and_key() {
    let mut h = Harness::new();
    h.user(
        "widgets/fz.luau",
        &pwidget(
            "{}",
            r#"
    local apps = { { name = "Zed" }, { name = "Alpha" }, { name = "Beta" } }
    local all = rsb.fuzzy("", apps, { key = function(a) return a.name end })
    local two = rsb.fuzzy("  ", apps, { key = function(a) return a.name end, limit = 2 })
    local ok, e = pcall(rsb.fuzzy, "x", { 1, 2 })
    local ok2, e2 = pcall(rsb.fuzzy, 5, {})
    local m = rsb.fuzzy_match("", "text")
    return rsb.text(`{#all} {all[1].name} {#two} {two[2].name} {#m} {(string.gsub(tostring(e), "\n.*", ""))} | {(string.gsub(tostring(e2), "\n.*", ""))}`)
"#,
        ),
    );
    h.start(&[(1, "fz")]);
    let text = h.texts(1).join("");
    assert!(text.starts_with("3 Zed 2 Alpha 0 "), "{text}");
    assert!(text.contains("every item (or what key returns) must be a string, got number"), "{text}");
    assert!(text.contains("rsb.fuzzy needs the query as a string"), "{text}");
}

#[test]
fn fuzzy_ranks_typos_and_gives_the_matched_positions() {
    let mut h = Harness::new();
    h.user(
        "widgets/fz.luau",
        &pwidget(
            "{}",
            r#"
    local names = { "Notepad", "Calculator", "Camera" }
    local hit = rsb.fuzzy("clcul", names)
    local pos = rsb.fuzzy_match("cal", "Calculator")
    local none = rsb.fuzzy_match("zzz", "Calculator")
    return rsb.text(`{#hit} {hit[1]} {table.concat(pos, ",")} {tostring(none)}`)
"#,
        ),
    );
    h.start(&[(1, "fz")]);
    assert_eq!(h.texts(1), ["1 Calculator 1,2,3 nil"]);
}

// ---------------------------------------------------------------- the whole surface

#[test]
fn every_new_function_is_callable_as_typed_and_returns_what_the_types_say() {
    let mut h = Harness::new();
    h.user(
        "widgets/all.luau",
        &pwidget(
            "{}",
            r#"
    local out = {}
    local function note(name: string, v: any) table.insert(out, name .. "=" .. type(v)) end
    local open = rsb.state(false)
    note("popup", rsb.popup { open = open, rsb.text "a" })
    note("menu", rsb.menu { open = open, items = { { label = "a" } } })
    note("input", rsb.input { value = rsb.state(""), placeholder = "p" })
    note("slider", rsb.slider { value = rsb.state(1), min = 0, max = 2, step = 1, on_change = function(v: number, done: boolean) end })
    note("scroll", rsb.scroll { rsb.text "a" })
    rsb.popups.open("nothing")
    rsb.popups.close("nothing")
    rsb.popups.toggle("nothing")
    note("is_open", rsb.untrack(function() return rsb.popups.is_open("nothing") end))
    note("animate", rsb.animate("spin", { duration = "1s", restart = 1 }))
    note("fuzzy", rsb.fuzzy("", { "a" }))
    note("fuzzy_match", rsb.fuzzy_match("", "a"))
    return rsb.text(table.concat(out, " "))
"#,
        ),
    );
    h.start(&[(1, "all")]);
    assert_eq!(
        h.texts(1),
        [
            "popup=userdata menu=userdata input=userdata slider=userdata scroll=userdata is_open=boolean animate=table fuzzy=table fuzzy_match=table"
        ]
    );
}

#[test]
fn a_key_handler_may_open_a_page_and_an_event_for_a_handler_that_is_gone_is_ignored() {
    let mut h = Harness::new();
    h.user(
        "widgets/ky.luau",
        &pwidget(
            "{ net = { 'open-meteo.com' } }",
            r#"
    return rsb.row {
        rsb.text "x",
        on_key = function(e) rsb.open_url("https://open-meteo.com/key") end,
    }
"#,
        ),
    );
    h.start(&[(1, "ky")]);
    let key = h.root(1).unwrap().handlers.key.unwrap();
    let ev =
        rsb_core::key::KeyEvent { key: rsb_core::key::Key::Enter, mods: Default::default(), down: true, repeat: false };
    h.send(element_call(1, key, ElementEvent::Key(ev), 0));
    assert!(h.requests().iter().all(|r| !r.contains("OpenUrl")), "a key without an input id is not a gesture");
    h.send(element_call(1, key, ElementEvent::Key(ev), 50));
    assert_eq!(h.requests().iter().filter(|r| r.contains("OpenUrl")).count(), 1);
    let errors = h.status()[0].errors;
    h.send(element_call(1, rsb_core::element::HandlerId(9999), ElementEvent::Focus, 0));
    assert_eq!(h.status()[0].errors, errors, "an unknown handler is ignored, not an error");
}

#[test]
fn opening_a_popup_with_two_hundred_rows_stays_cheap() {
    // The budget is lifted so a busy test machine cannot stop the widget: the number is what is measured.
    let limits = rsb_script::Limits {
        callback_budget: std::time::Duration::from_millis(1000),
        hard_wall_budget: std::time::Duration::from_secs(5),
        ..rsb_script::Limits::default()
    };
    let mut h = Harness::with(limits, None);
    h.user(
        "widgets/big.luau",
        &pwidget(
            "{}",
            r#"
    local open = rsb.state(false)
    local items = {}
    for i = 1, 200 do items[i] = { id = i, name = "App " .. i } end
    return rsb.row {
        rsb.text "x",
        on_click = function() open.value = not open.value end,
        rsb.popup {
            open = open,
            function()
                return { rsb.scroll { rsb.each(items, function(it) return rsb.row { class = "row", rsb.icon "cpu", rsb.text(it.name) } end, function(it) return tostring(it.id) end) } }
            end,
        },
    }
"#,
        ),
    );
    h.start(&[(1, "big")]);
    let click = first_click(h.root(1).unwrap_or_else(|| panic!("{:?}", h.plate(1)))).unwrap();
    let t = std::time::Instant::now();
    h.click(1, click);
    let open_ms = t.elapsed().as_secs_f64() * 1000.0;
    let mut texts = Vec::new();
    collect_text(h.root(1).unwrap(), &mut texts);
    assert_eq!(texts.len(), 201);
    let t = std::time::Instant::now();
    h.click(1, click);
    let close_ms = t.elapsed().as_secs_f64() * 1000.0;
    let build = if cfg!(debug_assertions) { "debug" } else { "release" };
    println!("popup of 200 rows: open {open_ms:.2} ms, close {close_ms:.2} ms ({build} build, remote desktop session)");
    assert!(open_ms < 250.0 && close_ms < 250.0, "{open_ms} {close_ms}");
}

mod common;

use std::time::Instant;

use common::*;
use rsb_core::element::{Element, ElementKind, HandlerId, NavAxis};
use rsb_core::geom::PointF;
use rsb_core::hit::{Button, ElementEvent, InputKind, PointerEvent, PointerKind};
use rsb_core::key::Key;
use rsb_core::popup::{DismissRules, PopupProps, Side};
use rsb_core::scene::Scene;

const CSS: &str = "bar { font-size: 10px } popup { display: flex; flex-direction: column; width: 120px; background: #111 } \
                   .b { width: 40px; height: 12px; background: #222 } .row { display: flex; flex-direction: row } \
                   .nav { flex-direction: column; align-items: flex-start } .b:focus { background: #ff0000 } .b:focus-visible { color: #00ff00 } \
                   .grp:focus-within { background: #0000ff } .b:disabled { opacity: 0.5 }";

fn b(class: &str) -> Element {
    focusable(classed(boxed(vec![]), &["b", class]))
}

fn make_with(props: PopupProps, content: Vec<Element>, t: Instant) -> Scene {
    let (bar, _) = bar_with_popup(CSS, props, content);
    let d = decls(&bar);
    let mut s = popup_scene(CSS, &d[0], Side::Below, 200, 120);
    s.set_open(true, t);
    s.set_window_focus(true, t);
    frame(&mut s, t);
    s
}

fn make(content: Vec<Element>, t: Instant) -> Scene {
    make_with(open_props(1), content, t)
}

fn focused_class(s: &Scene) -> Option<String> {
    s.nodes()
        .into_iter()
        .find(|n| {
            n.style.background == rsb_core::style::Background::Color(rsb_core::color::Color::rgba8(255, 0, 0, 255))
        })
        .and_then(|n| n.classes.iter().find(|c| &***c != "b").map(|c| c.to_string()))
}

fn tab(s: &mut Scene, shift: bool, t: Instant) {
    let r = s.key(key_mod(Key::Tab, false, shift), t);
    assert!(r.consumed);
    frame(s, t);
}

#[test]
fn tab_walks_the_focusable_elements_in_tree_order_and_wraps() {
    let t = Instant::now();
    let mut dead = b("dead");
    dead.interaction.disabled = true;
    let mut s = make(vec![b("one"), dead, classed(boxed(vec![b("two")]), &["row"]), b("three")], t);
    assert_eq!(focused_class(&s), None);
    for want in ["one", "two", "three", "one"] {
        tab(&mut s, false, t);
        assert_eq!(focused_class(&s).as_deref(), Some(want));
    }
    tab(&mut s, true, t);
    assert_eq!(focused_class(&s).as_deref(), Some("three"));
}

#[test]
fn focus_pseudo_classes_follow_the_window_and_how_focus_moved() {
    let t = Instant::now();
    let grp = classed(boxed(vec![b("in")]), &["grp"]);
    let mut s = make(vec![grp, b("other")], t);
    let colour =
        |s: &Scene, class: &str| s.nodes().into_iter().find(|n| n.classes.iter().any(|c| &**c == class)).unwrap().style;
    // A click focuses and is not focus-visible.
    let r = node_by_class(&s, "in").rect;
    let p = PointF { x: r.x + 2.0, y: r.y + 2.0 };
    s.pointer(PointerEvent { kind: PointerKind::Down(Button::Left), x: p.x, y: p.y }, t);
    s.pointer(PointerEvent { kind: PointerKind::Up(Button::Left), x: p.x, y: p.y }, t);
    frame(&mut s, t);
    assert_eq!(focused_class(&s).as_deref(), Some("in"));
    let white = rsb_core::color::Color::WHITE;
    assert_eq!(colour(&s, "in").color, white, "no :focus-visible after a click");
    assert_eq!(
        colour(&s, "grp").background,
        rsb_core::style::Background::Color(rsb_core::color::Color::rgba8(0, 0, 255, 255)),
        ":focus-within on the ancestor"
    );
    // A key moves focus and it is visible.
    tab(&mut s, false, t);
    assert_eq!(focused_class(&s).as_deref(), Some("other"));
    assert_ne!(colour(&s, "other").color, white);
    // The window loses focus: nothing matches.
    s.set_window_focus(false, t);
    frame(&mut s, t);
    assert_eq!(focused_class(&s), None);
    assert_eq!(colour(&s, "grp").background, rsb_core::style::Background::None);
}

#[test]
fn focus_first_takes_autofocus_or_the_first_when_opened_from_the_keyboard() {
    let t = Instant::now();
    let mut auto = b("auto");
    auto.interaction.autofocus = true;
    let mut s = make(vec![b("one"), auto], t);
    s.focus_first(false, t);
    frame(&mut s, t);
    assert_eq!(focused_class(&s).as_deref(), Some("auto"));
    let mut s = make(vec![b("one"), b("two")], t);
    s.focus_first(false, t);
    frame(&mut s, t);
    assert_eq!(focused_class(&s), None, "a mouse open with nothing to autofocus takes none");
    s.focus_first(true, t);
    frame(&mut s, t);
    assert_eq!(focused_class(&s).as_deref(), Some("one"));
}

#[test]
fn an_autofocus_input_that_arrives_in_an_open_popup_takes_focus_from_the_element_that_left() {
    let t = Instant::now();
    let (bar, _) = bar_with_popup(CSS, open_props(1), vec![b("tile")]);
    let d = decls(&bar);
    let mut s = popup_scene(CSS, &d[0], Side::Below, 200, 120);
    s.set_open(true, t);
    s.set_window_focus(true, t);
    frame(&mut s, t);
    // A click on the tile focuses it.
    let r = node_by_class(&s, "tile").rect;
    let p = PointF { x: r.x + 2.0, y: r.y + 2.0 };
    s.pointer(PointerEvent { kind: PointerKind::Down(Button::Left), x: p.x, y: p.y }, t);
    s.pointer(PointerEvent { kind: PointerKind::Up(Button::Left), x: p.x, y: p.y }, t);
    frame(&mut s, t);
    assert_eq!(focused_class(&s).as_deref(), Some("tile"));
    // The panel changes page: the tile is gone and the search input of the new page asks for focus.
    let mut search = classed(input_el(""), &["b", "search"]);
    search.interaction.autofocus = true;
    let (bar2, _) = bar_with_popup(CSS, open_props(1), vec![search]);
    let d2 = decls(&bar2);
    s.set_popup_tree(d2[0].tree.clone(), d[0].path.clone());
    frame(&mut s, t);
    assert_eq!(focused_class(&s).as_deref(), Some("search"));
}

#[test]
fn focus_and_blur_reach_their_handlers() {
    let t = Instant::now();
    let mut one = b("one");
    one.handlers.focus = Some(HandlerId(1));
    one.handlers.blur = Some(HandlerId(2));
    let mut two = b("two");
    two.handlers.focus = Some(HandlerId(3));
    let mut s = make(vec![one, two], t);
    tab(&mut s, false, t);
    tab(&mut s, false, t);
    let ev: Vec<_> = s.take_events().into_iter().map(|e| (e.handler.0, e.event)).collect();
    assert_eq!(ev, [(1, ElementEvent::Focus), (2, ElementEvent::Blur), (3, ElementEvent::Focus)],);
}

fn nav(axis: NavAxis, content: Vec<Element>) -> Element {
    let mut e = classed(boxed(content), &["nav"]);
    e.interaction.nav = axis;
    e
}

#[test]
fn nav_groups_move_focus_with_arrows_on_their_axis() {
    let t = Instant::now();
    let mut s = make(vec![nav(NavAxis::Vertical, vec![b("a"), b("b2"), b("c")])], t);
    s.focus_first(true, t);
    let press = |s: &mut Scene, k: Key| {
        let r = s.key(key_down(k), t);
        frame(s, t);
        r
    };
    assert!(press(&mut s, Key::ArrowDown).consumed);
    assert_eq!(focused_class(&s).as_deref(), Some("b2"));
    assert!(!press(&mut s, Key::ArrowRight).consumed, "not this group's axis");
    press(&mut s, Key::ArrowDown);
    press(&mut s, Key::ArrowDown);
    assert_eq!(focused_class(&s).as_deref(), Some("a"), "wraps");
    press(&mut s, Key::ArrowUp);
    assert_eq!(focused_class(&s).as_deref(), Some("c"));
}

#[test]
fn a_grid_group_moves_by_the_laid_out_rows() {
    let t = Instant::now();
    let css = format!(
        "{CSS} .grid {{ display: flex; flex-direction: row; width: 90px; height: 40px }} .grid .b {{ width: 30px }}"
    );
    let mut grid = nav(NavAxis::Grid, vec![b("a"), b("b2"), b("c"), b("d"), b("e")]);
    grid.classes = vec![std::sync::Arc::from("grid")];
    // Three to a row at 30 px in 90 px: wrap needs a flex-wrap, which the engine lacks, so
    // two column groups build the rows.
    let rows = nav(
        NavAxis::Grid,
        vec![classed(boxed(vec![b("a"), b("b2"), b("c")]), &["row"]), classed(boxed(vec![b("d"), b("e")]), &["row"])],
    );
    let _ = grid;
    let (bar, _) = bar_with_popup(&css, open_props(1), vec![rows]);
    let mut s = popup_scene(&css, &decls(&bar)[0], Side::Below, 200, 120);
    s.set_open(true, t);
    s.set_window_focus(true, t);
    frame(&mut s, t);
    s.focus_first(true, t);
    let press = |s: &mut Scene, k: Key| {
        s.key(key_down(k), t);
        frame(s, t);
    };
    press(&mut s, Key::ArrowRight);
    press(&mut s, Key::ArrowDown);
    assert_eq!(focused_class(&s).as_deref(), Some("e"), "below the second column");
    press(&mut s, Key::ArrowUp);
    assert_eq!(focused_class(&s).as_deref(), Some("b2"));
    press(&mut s, Key::ArrowLeft);
    press(&mut s, Key::ArrowDown);
    assert_eq!(focused_class(&s).as_deref(), Some("d"));
    press(&mut s, Key::ArrowDown);
    assert_eq!(focused_class(&s).as_deref(), Some("d"), "no row below: stays");
}

#[test]
fn enter_and_space_click_the_focused_element_at_its_centre_but_not_a_disabled_one() {
    let t = Instant::now();
    let mut one = b("one");
    one.handlers.click = Some(HandlerId(5));
    let mut dead = b("dead");
    dead.handlers.click = Some(HandlerId(6));
    let mut s = make(vec![one, dead], t);
    s.focus_first(true, t);
    for k in [Key::Enter, Key::Space] {
        let r = s.key(key_down(k), t);
        assert!(r.consumed);
        assert_eq!(r.calls.len(), 1);
        let c = r.calls[0];
        assert_eq!((c.handler, c.kind, c.x, c.y), (HandlerId(5), InputKind::Click(Button::Left), 20.0, 6.0));
    }
    // A pointer click on a disabled element runs nothing either.
    let mut dead = b("dead");
    dead.handlers.click = Some(HandlerId(6));
    dead.interaction.disabled = true;
    let mut s = make(vec![dead], t);
    let r = node_by_class(&s, "dead").rect;
    let p = PointF { x: r.x + 3.0, y: r.y + 3.0 };
    s.pointer(PointerEvent { kind: PointerKind::Down(Button::Left), x: p.x, y: p.y }, t);
    let res = s.pointer(PointerEvent { kind: PointerKind::Up(Button::Left), x: p.x, y: p.y }, t);
    assert!(res.calls.is_empty());
}

#[test]
fn on_key_goes_to_the_focused_element_else_the_nearest_ancestor_else_the_root() {
    let t = Instant::now();
    let mut leaf = b("leaf");
    leaf.handlers.key = Some(HandlerId(1));
    let mut group = classed(boxed(vec![b("inner")]), &["group"]);
    group.handlers.key = Some(HandlerId(2));
    let plain = b("plain");
    let mut s = make(vec![leaf, group, plain], t);
    let send = |s: &mut Scene| -> Vec<u32> {
        s.key(key_down(Key::Char('x')), t);
        s.take_events().into_iter().map(|e| e.handler.0).collect()
    };
    tab(&mut s, false, t);
    assert_eq!(send(&mut s), [1]);
    tab(&mut s, false, t);
    assert_eq!(send(&mut s), [2], "the ancestor of the focused element");
    tab(&mut s, false, t);
    assert!(send(&mut s).is_empty(), "nothing on the path");
    // The popup root's own handler is the last resort.
    let mut root = popup_el(open_props(1), ElementKind::Popup, vec![b("plain")]);
    root.handlers.key = Some(HandlerId(3));
    let anchor = classed(boxed(vec![root]), &["anchor"]);
    let mut bar = scene_with(CSS, 400, 1.0, Some(classed(boxed(vec![anchor]), &["root"])), None, None);
    frame(&mut bar, t);
    let mut s = popup_scene(CSS, &decls(&bar)[0], Side::Below, 200, 100);
    s.set_open(true, t);
    s.set_window_focus(true, t);
    frame(&mut s, t);
    s.focus_first(true, t);
    assert_eq!(send(&mut s), [3]);
}

#[test]
fn escape_closes_the_surface_unless_the_popup_says_otherwise_and_still_reaches_on_key() {
    let t = Instant::now();
    let mut s = make(vec![b("one")], t);
    s.focus_first(true, t);
    let r = s.key(key_down(Key::Escape), t);
    assert!(r.dismiss && !r.consumed);
    let keep = PopupProps { dismiss: DismissRules { outside: true, escape: false }, ..open_props(1) };
    let mut one = b("one");
    one.handlers.key = Some(HandlerId(4));
    let mut s = make_with(keep, vec![one], t);
    s.focus_first(true, t);
    let r = s.key(key_down(Key::Escape), t);
    assert!(!r.dismiss, "dismiss.escape = false");
    let ev = s.take_events();
    assert_eq!(ev.len(), 1, "on_key still hears it");
    assert!(matches!(ev[0].event, ElementEvent::Key(k) if k.key == Key::Escape));
}

#[test]
fn a_bar_never_takes_focus_and_a_key_does_nothing_there() {
    let t = Instant::now();
    let mut s = scene_with(CSS, 300, 1.0, Some(b("one")), None, None);
    frame(&mut s, t);
    s.focus_first(true, t);
    s.set_window_focus(true, t);
    let r = s.key(key_down(Key::Escape), t);
    assert!(!r.dismiss && !r.consumed && r.calls.is_empty());
    assert!(s.take_events().is_empty());
}

#[test]
fn an_overlay_ignores_input_until_the_guard_lifts_and_repeats_never_click() {
    let t = Instant::now();
    let mut one = b("one");
    one.handlers.click = Some(HandlerId(5));
    let (bar, _) = bar_with_popup(CSS, open_props(1), vec![one]);
    let mut s = popup_scene(CSS, &decls(&bar)[0], Side::Screen, 200, 120);
    s.set_open(true, t);
    s.set_window_focus(true, t);
    frame(&mut s, t);
    s.focus_first(true, t);
    // Too early.
    let r = s.key(key_down(Key::Enter), t + ms(100));
    assert!(r.calls.is_empty() && !r.consumed);
    // Settled and 400 ms on: it works, but a repeat never clicks.
    frame(&mut s, t + ms(500));
    let mut rep = key_down(Key::Enter);
    rep.repeat = true;
    let r = s.key(rep, t + ms(500));
    assert!(r.calls.is_empty());
    let r = s.key(key_down(Key::Enter), t + ms(500));
    assert_eq!(r.calls.len(), 1);
    // A press before the guard is dropped with its release.
    let mut s2 = popup_scene(CSS, &decls(&bar)[0], Side::Screen, 200, 120);
    s2.set_open(true, t);
    frame(&mut s2, t);
    let rect = node_by_class(&s2, "one").rect;
    let p = PointF { x: rect.x + 2.0, y: rect.y + 2.0 };
    s2.pointer(PointerEvent { kind: PointerKind::Down(Button::Left), x: p.x, y: p.y }, t + ms(100));
    frame(&mut s2, t + ms(450));
    let r = s2.pointer(PointerEvent { kind: PointerKind::Up(Button::Left), x: p.x, y: p.y }, t + ms(450));
    assert!(r.calls.is_empty(), "the release of an early press is dropped");
    s2.pointer(PointerEvent { kind: PointerKind::Down(Button::Left), x: p.x, y: p.y }, t + ms(500));
    let r = s2.pointer(PointerEvent { kind: PointerKind::Up(Button::Left), x: p.x, y: p.y }, t + ms(520));
    assert_eq!(r.calls.len(), 1);
}

#[test]
fn an_overlay_with_an_endless_animation_still_takes_input_and_escape_closes_it() {
    let t = Instant::now();
    let css = format!(
        "{CSS} @keyframes pulse {{ from {{ opacity: 0.5 }} to {{ opacity: 1 }} }} .spin {{ animation: pulse 500ms linear infinite }}"
    );
    let mut one = b("one");
    one.classes.push(std::sync::Arc::from("spin"));
    one.handlers.click = Some(HandlerId(5));
    let (bar, _) = bar_with_popup(&css, open_props(1), vec![one]);
    let mut s = popup_scene(&css, &decls(&bar)[0], Side::Screen, 200, 120);
    s.set_open(true, t);
    s.set_window_focus(true, t);
    frame(&mut s, t);
    s.focus_first(true, t);
    for step in 1..=40u64 {
        frame(&mut s, t + ms(step * 16));
    }
    assert!(!s.is_settled(), "the animation never ends");
    // Past 400 ms but before the cap: Enter waits, Escape works.
    let r = s.key(key_down(Key::Enter), t + ms(700));
    assert!(r.calls.is_empty() && !r.consumed, "Enter waits for the settle or the cap");
    let r = s.key(key_down(Key::Escape), t + ms(300));
    assert!(!r.dismiss, "Escape waits for the short guard");
    let r = s.key(key_down(Key::Escape), t + ms(450));
    assert!(r.dismiss, "Escape closes it 400 ms in, settled or not");
    // At the cap Enter works too.
    for step in 41..=70u64 {
        frame(&mut s, t + ms(step * 16));
    }
    let r = s.key(key_down(Key::Enter), t + ms(1100));
    assert_eq!(r.calls.len(), 1, "Enter works from 1 s");
}

#[test]
fn an_overlay_that_forbids_escape_is_not_closed_by_it_while_guarded() {
    let t = Instant::now();
    let keep = PopupProps { dismiss: DismissRules { outside: true, escape: false }, ..open_props(1) };
    let (bar, _) = bar_with_popup(CSS, keep, vec![b("one")]);
    let mut s = popup_scene(CSS, &decls(&bar)[0], Side::Screen, 200, 120);
    s.set_open(true, t);
    s.set_window_focus(true, t);
    frame(&mut s, t);
    let r = s.key(key_down(Key::Escape), t + ms(450));
    assert!(!r.dismiss);
}

#[test]
fn checked_and_disabled_match_the_flags_the_script_set() {
    let t = Instant::now();
    let css = format!("{CSS} .c:checked {{ color: #00ff00 }} .c:disabled {{ color: #0000ff }}");
    let mut on = b("on");
    on.classes.push(std::sync::Arc::from("c"));
    on.interaction.checked = Some(true);
    let mut off = b("off");
    off.classes.push(std::sync::Arc::from("c"));
    off.interaction.checked = Some(false);
    let mut dead = b("dead");
    dead.classes.push(std::sync::Arc::from("c"));
    dead.interaction.disabled = true;
    let (bar, _) = bar_with_popup(&css, open_props(1), vec![on, off, dead]);
    let mut s = popup_scene(&css, &decls(&bar)[0], Side::Below, 200, 120);
    s.set_open(true, t);
    frame(&mut s, t);
    let colour = |class: &str| node_by_class(&s, class).style.color;
    assert_eq!(colour("on"), rsb_core::color::Color::rgba8(0, 255, 0, 255));
    assert_eq!(colour("off"), rsb_core::color::Color::WHITE);
    assert_eq!(colour("dead"), rsb_core::color::Color::rgba8(0, 0, 255, 255));
}

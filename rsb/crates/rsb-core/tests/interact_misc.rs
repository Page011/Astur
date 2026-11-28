mod common;

use std::sync::Arc;
use std::time::Instant;

use common::*;
use rsb_core::draw::{DrawCmd, DrawList, Fill};
use rsb_core::element::{Element, ElementKind, HandlerId, NavAxis, SliderProps};
use rsb_core::geom::PointF;
use rsb_core::hit::{Button, ElementEvent, PointerEvent, PointerKind};
use rsb_core::key::Key;
use rsb_core::popup::{CloseReason, PopupProps, Side};
use rsb_core::scene::Scene;

// ---- sliders ----

const SLIDER_CSS: &str = "bar { font-size: 10px } popup { display: flex; flex-direction: column; width: 120px } \
                          slider { width: 100px; height: 10px; background: #222222; accent-color: #00ff00 } \
                          .v { width: 10px; height: 100px }";

fn slider_scene(p: SliderProps, class: &str, t: Instant) -> Scene {
    let mut el = slider_el(SliderProps { on_change: Some(HandlerId(4)), ..p });
    el.classes = vec![Arc::from(class)];
    let (bar, _) = bar_with_popup(SLIDER_CSS, open_props(1), vec![el]);
    let mut s = popup_scene(SLIDER_CSS, &decls(&bar)[0], Side::Below, 140, 140);
    s.set_open(true, t);
    s.set_window_focus(true, t);
    frame(&mut s, t);
    s.focus_first(true, t);
    s
}

fn values(s: &mut Scene) -> Vec<(f64, bool)> {
    s.take_events()
        .into_iter()
        .filter_map(|e| match e.event {
            ElementEvent::ValueChanged { value, done } => Some((value, done)),
            _ => None,
        })
        .collect()
}

fn fill_rects(list: &DrawList) -> Vec<(f32, f32, f32, f32)> {
    list.items
        .iter()
        .filter_map(|i| match &i.cmd {
            DrawCmd::Fill { rect, fill: Fill::Solid(c), .. } if c.g > 0.9 && c.r < 0.1 => {
                Some((rect.x, rect.y, rect.w, rect.h))
            }
            _ => None,
        })
        .collect()
}

fn ptr(s: &mut Scene, kind: PointerKind, p: PointF) {
    s.pointer(PointerEvent { kind, x: p.x, y: p.y }, Instant::now());
}

#[test]
fn a_slider_press_sets_the_value_snaps_to_the_step_and_drags_with_a_rate_limit() {
    let t = Instant::now();
    let mut s = slider_scene(SliderProps { step: 5.0, ..SliderProps::default() }, "h", t);
    let r = node_by_class(&s, "h").rect;
    let at = |x: f32| PointF { x: r.x + x, y: r.y + 5.0 };
    let ev = |kind: PointerKind, p: PointF, _at: Instant| PointerEvent { kind, x: p.x, y: p.y };
    s.pointer(ev(PointerKind::Down(Button::Left), at(32.0), t), t);
    assert_eq!(values(&mut s), [(30.0, false)], "snapped to 5s, reported at once");
    // The fill runs from the start edge to the value, in the accent colour.
    let out = frame(&mut s, t);
    assert_eq!(fill_rects(&out.draw), [(r.x, r.y, 30.0, 10.0)]);
    // Dragging inside 33 ms of the last report is held back; the frame after sends it.
    s.pointer(ev(PointerKind::Move, at(52.0), t + ms(10)), t + ms(10));
    assert!(values(&mut s).is_empty());
    let out = frame(&mut s, t + ms(10));
    assert_eq!(fill_rects(&out.draw)[0].2, 50.0, "the drawn value is the dragged one at once");
    let due = out.next_frame.unwrap();
    assert_eq!(due.duration_since(t), ms(33));
    frame(&mut s, t + ms(33));
    assert_eq!(values(&mut s), [(50.0, false)]);
    // The pointer may leave the box; values stay inside the range.
    s.pointer(ev(PointerKind::Move, at(900.0), t + ms(100)), t + ms(100));
    s.pointer(ev(PointerKind::Up(Button::Left), at(900.0), t + ms(110)), t + ms(110));
    let v = values(&mut s);
    assert_eq!(v.last(), Some(&(100.0, true)), "release reports the last value with done: {v:?}");
}

#[test]
fn slider_keys_wheel_and_a_newer_value_seq() {
    let t = Instant::now();
    let mut s = slider_scene(SliderProps { value: 40.0, step: 5.0, ..SliderProps::default() }, "h", t);
    for (k, want) in [
        (Key::ArrowRight, 45.0),
        (Key::ArrowUp, 50.0),
        (Key::ArrowLeft, 45.0),
        (Key::ArrowDown, 40.0),
        (Key::PageUp, 90.0),
        (Key::Home, 0.0),
        (Key::End, 100.0),
        (Key::PageDown, 50.0),
    ] {
        let r = s.key(key_down(k), t);
        assert!(r.consumed);
        assert_eq!(values(&mut s), [(want, true)], "{k:?}");
    }
    let r = node_by_class(&s, "h").rect;
    s.pointer(PointerEvent { kind: PointerKind::Wheel(2.0), x: r.x + 5.0, y: r.y + 5.0 }, t);
    assert_eq!(values(&mut s), [(60.0, true)], "one step per notch");
    assert_eq!(fill_rects(&frame(&mut s, t).draw)[0].2, 60.0);
}

#[test]
fn a_vertical_slider_fills_from_the_bottom() {
    let t = Instant::now();
    let mut s = slider_scene(SliderProps { value: 25.0, vertical: true, ..SliderProps::default() }, "v", t);
    let r = node_by_class(&s, "v").rect;
    assert_eq!(fill_rects(&frame(&mut s, t).draw), [(r.x, r.y + 75.0, 10.0, 25.0)]);
    s.pointer(PointerEvent { kind: PointerKind::Down(Button::Left), x: r.x + 5.0, y: r.y + 10.0 }, t);
    assert_eq!(values(&mut s), [(90.0, false)]);
}

// ---- menus ----

const MENU_CSS: &str = "bar { font-size: 10px } menu { display: flex; flex-direction: column; width: 100px; background: #111 } \
                        menuitem { display: flex; height: 14px; width: 100px } menuitem:focus { background: #ff0000 }";

fn item(label: &str, click: Option<u32>, children: Vec<Element>) -> Element {
    let mut e = Element::new(ElementKind::MenuItem);
    e.text = Some(Arc::from(label));
    e.handlers.click = click.map(HandlerId);
    e.children = children;
    e
}

fn menu_props() -> PopupProps {
    PopupProps { group: Arc::from("menu"), on_close: Some(HandlerId(20)), ..open_props(1) }
}

fn menu_scene(items: Vec<Element>, t: Instant) -> Scene {
    menu_scene_with(menu_props(), items, t)
}

fn menu_scene_with(props: PopupProps, items: Vec<Element>, t: Instant) -> Scene {
    let mut menu = popup_el(props, ElementKind::Menu, items);
    menu.interaction.nav = NavAxis::Vertical;
    let anchor = classed(boxed(vec![menu]), &["anchor"]);
    let mut bar = scene_with(MENU_CSS, 400, 1.0, Some(anchor), None, None);
    frame(&mut bar, t);
    let d = decls(&bar);
    assert_eq!(d[0].kind, ElementKind::Menu);
    let mut s = popup_scene(MENU_CSS, &d[0], Side::Below, 120, 120);
    s.set_open(true, t);
    s.set_window_focus(true, t);
    frame(&mut s, t);
    s.focus_first(true, t);
    s
}

fn focused_label(s: &mut Scene, t: Instant) -> Option<String> {
    frame(s, t);
    let red = rsb_core::style::Background::Color(rsb_core::color::Color::rgba8(255, 0, 0, 255));
    let nodes = s.nodes();
    let at = nodes.iter().position(|n| n.kind == ElementKind::MenuItem && n.style.background == red)?;
    nodes[at + 1..]
        .iter()
        .find(|n| n.depth == nodes[at].depth + 1 && n.text.is_some())
        .and_then(|n| n.text.as_ref().map(|t| t.to_string()))
}

#[test]
fn menu_keys_move_over_enabled_items_wrap_jump_and_pick() {
    let t = Instant::now();
    let mut off = item("Disabled", Some(2), vec![]);
    off.interaction.disabled = true;
    let mut s = menu_scene(
        vec![item("Alpha", Some(1), vec![]), off, item("Beta", Some(3), vec![]), item("Gamma", Some(4), vec![])],
        t,
    );
    assert_eq!(focused_label(&mut s, t).as_deref(), Some("Alpha"));
    s.key(key_down(Key::ArrowDown), t);
    assert_eq!(focused_label(&mut s, t).as_deref(), Some("Beta"), "the disabled item is skipped");
    s.key(key_down(Key::ArrowDown), t);
    s.key(key_down(Key::ArrowDown), t);
    assert_eq!(focused_label(&mut s, t).as_deref(), Some("Alpha"), "wraps");
    s.key(key_down(Key::End), t);
    assert_eq!(focused_label(&mut s, t).as_deref(), Some("Gamma"));
    s.key(key_down(Key::Home), t);
    assert_eq!(focused_label(&mut s, t).as_deref(), Some("Alpha"));
    s.key(key_down(Key::Char('g')), t);
    assert_eq!(focused_label(&mut s, t).as_deref(), Some("Gamma"), "a letter jumps to the next item starting with it");
    // Enter runs the item's click, then the chain closes as Picked.
    let r = s.key(key_down(Key::Enter), t);
    assert_eq!(r.calls.len(), 1);
    assert_eq!(r.calls[0].handler, HandlerId(4));
    assert!(r.picked, "the manager closes the chain from this flag");
    let ev = s.take_events();
    assert!(
        ev.iter().any(|e| e.handler == HandlerId(20) && e.event == ElementEvent::PopupClosed(CloseReason::Picked)),
        "{ev:?}"
    );
}

#[test]
fn a_pick_is_reported_by_the_result_even_when_the_menu_has_no_on_close() {
    let t = Instant::now();
    let props = PopupProps { on_close: None, ..menu_props() };
    let mut s = menu_scene_with(props, vec![item("One", Some(1), vec![]), item("Two", Some(2), vec![])], t);
    let r = s.key(key_down(Key::Enter), t);
    assert!(r.picked);
    assert!(s.take_events().is_empty(), "no handler to tell");
    let rows: Vec<_> = s.nodes().into_iter().filter(|n| n.kind == ElementKind::MenuItem).collect();
    let p = PointF { x: rows[1].rect.x + 3.0, y: rows[1].rect.y + 3.0 };
    s.pointer(PointerEvent { kind: PointerKind::Down(Button::Left), x: p.x, y: p.y }, t);
    let r = s.pointer(PointerEvent { kind: PointerKind::Up(Button::Left), x: p.x, y: p.y }, t);
    assert!(r.picked);
    // The flag is per result: a later event that picks nothing does not repeat it.
    assert!(!s.key(key_down(Key::ArrowDown), t).picked);
}

#[test]
fn a_click_on_a_menu_item_runs_it_and_closes_the_chain_but_a_disabled_item_does_nothing() {
    let t = Instant::now();
    let mut off = item("Off", Some(2), vec![]);
    off.interaction.disabled = true;
    let mut s = menu_scene(vec![item("One", Some(1), vec![]), off], t);
    let rows: Vec<_> = s.nodes().into_iter().filter(|n| n.kind == ElementKind::MenuItem).collect();
    let click = |s: &mut Scene, r: rsb_core::geom::RectF| {
        let p = PointF { x: r.x + 3.0, y: r.y + 3.0 };
        s.pointer(PointerEvent { kind: PointerKind::Down(Button::Left), x: p.x, y: p.y }, t);
        s.pointer(PointerEvent { kind: PointerKind::Up(Button::Left), x: p.x, y: p.y }, t)
    };
    let r = click(&mut s, rows[1].rect);
    assert!(r.calls.is_empty());
    assert!(!r.picked);
    assert!(s.take_events().iter().all(|e| e.handler != HandlerId(20)));
    let r = click(&mut s, rows[0].rect);
    assert_eq!(r.calls[0].handler, HandlerId(1));
    assert!(r.picked);
    assert!(s.take_events().iter().any(|e| e.event == ElementEvent::PopupClosed(CloseReason::Picked)));
}

fn submenu_props() -> PopupProps {
    PopupProps {
        group: Arc::from("menu"),
        open: false,
        open_seq: 0,
        on_close: Some(HandlerId(21)),
        placement: rsb_core::popup::Placement::Right,
        ..PopupProps::default()
    }
}

#[test]
fn a_scene_says_whether_it_declares_popups_without_building_the_list() {
    let t = Instant::now();
    let sub = popup_el(submenu_props(), ElementKind::Menu, vec![item("Inner", Some(9), vec![])]);
    let with = menu_scene(vec![item("More", None, vec![sub])], t);
    assert!(with.declares_popups());
    assert_eq!(with.declares_popups(), !with.popups().is_empty());
    let without = menu_scene(vec![item("Plain", Some(1), vec![])], t);
    assert!(!without.declares_popups());
    assert!(without.popups().is_empty());
}

#[test]
fn a_submenu_opens_from_right_enter_and_hover_and_closes_with_left_or_the_dismiss() {
    let t = Instant::now();
    let sub = popup_el(submenu_props(), ElementKind::Menu, vec![item("Inner", Some(9), vec![])]);
    let mut s = menu_scene(
        vec![item("Plain", Some(1), vec![]), item("More", None, vec![sub]), item("Last", Some(3), vec![])],
        t,
    );
    let sub_decl = |s: &Scene| s.popups().into_iter().find(|d| d.kind == ElementKind::Menu).unwrap();
    assert!(!sub_decl(&s).open, "the script gives it open = false");
    // Right on an item without a submenu does nothing.
    assert!(!s.key(key_down(Key::ArrowRight), t).consumed);
    s.key(key_down(Key::ArrowDown), t);
    assert!(s.key(key_down(Key::ArrowRight), t).consumed);
    let d = sub_decl(&s);
    assert!(d.open, "Right opened the submenu");
    assert_eq!(d.path.len(), 3);
    // The manager's scene for it: Left closes one level, Escape too.
    let mut inner = popup_scene(MENU_CSS, &d, Side::Right, 120, 60);
    inner.set_open(true, t);
    inner.set_window_focus(true, t);
    frame(&mut inner, t);
    inner.focus_first(true, t);
    assert!(inner.key(key_down(Key::ArrowLeft), t).dismiss);
    assert!(inner.key(key_down(Key::Escape), t).dismiss);
    // Down the line the manager tells the parent scene it was closed.
    s.dismiss_popup(d.key, 0);
    assert!(!sub_decl(&s).open);
    // Enter opens it too, and hover does.
    s.key(key_down(Key::Enter), t);
    assert!(sub_decl(&s).open);
    s.dismiss_popup(d.key, 0);
    let rows: Vec<_> = s.nodes().into_iter().filter(|n| n.kind == ElementKind::MenuItem).collect();
    let move_to = |s: &mut Scene, r: rsb_core::geom::RectF| {
        s.pointer(PointerEvent { kind: PointerKind::Move, x: r.x + 3.0, y: r.y + 3.0 }, t)
    };
    move_to(&mut s, rows[1].rect);
    assert!(sub_decl(&s).open, "hover opened it");
    move_to(&mut s, rows[2].rect);
    assert!(!sub_decl(&s).open, "hovering the next item closes it");
    // Enter on the submenu item never picks.
    let r = s.key(key_down(Key::Enter), t);
    assert!(r.calls.is_empty());
    assert!(s.take_events().iter().all(|e| e.handler != HandlerId(20)));
}

// ---- tooltips ----

#[test]
fn the_tooltip_of_the_deepest_hovered_node_that_has_one() {
    let t = Instant::now();
    let tip = |name: &str| {
        popup_el(
            PopupProps { group: Arc::from("tooltip"), names: vec![Arc::from(name)], ..PopupProps::default() },
            ElementKind::Tooltip,
            vec![text("tip")],
        )
    };
    let inner = classed(boxed(vec![tip("inner")]), &["inner"]);
    let outer = classed(boxed(vec![inner, tip("outer")]), &["outer"]);
    let plain = classed(boxed(vec![]), &["plain"]);
    let root = classed(boxed(vec![outer, plain]), &["root"]);
    let css = "bar { font-size: 10px } .outer { width: 60px; height: 24px; padding: 4px } .inner { width: 20px; height: 16px; background: #444 } .plain { width: 30px; height: 20px; background: #555 }";
    let mut s = scene_with(css, 300, 1.0, Some(root), None, None);
    frame(&mut s, t);
    let decls = s.popups();
    assert_eq!(decls.len(), 2);
    let key_of = |name: &str| decls.iter().find(|d| d.props.answers_to(name)).unwrap().key;
    assert_eq!(s.hovered_tooltip(), None);
    let centre = |class: &str| {
        let r = node_by_class(&s, class).rect;
        PointF { x: r.x + r.w / 2.0, y: r.y + r.h / 2.0 }
    };
    let (inner_c, plain_c) = (centre("inner"), centre("plain"));
    let outer_r = node_by_class(&s, "outer").rect;
    s.pointer(PointerEvent { kind: PointerKind::Move, x: inner_c.x, y: inner_c.y }, t);
    assert_eq!(s.hovered_tooltip(), Some(key_of("inner")));
    s.pointer(PointerEvent { kind: PointerKind::Move, x: outer_r.x + 1.0, y: outer_r.y + 1.0 }, t);
    assert_eq!(s.hovered_tooltip(), Some(key_of("outer")));
    s.pointer(PointerEvent { kind: PointerKind::Move, x: plain_c.x, y: plain_c.y }, t);
    assert_eq!(s.hovered_tooltip(), None);
    let _ = ptr;
}

// ---- key routing order ----

#[test]
fn a_focused_slider_uses_its_arrows_before_the_nav_group_but_tab_still_moves_focus() {
    let t = Instant::now();
    let slider =
        slider_el(SliderProps { value: 10.0, step: 1.0, on_change: Some(HandlerId(4)), ..SliderProps::default() });
    let mut group = kind_el(ElementKind::Box, vec![slider, focusable(classed(boxed(vec![]), &["next"]))]);
    group.interaction.nav = NavAxis::Vertical;
    let (bar, _) = bar_with_popup(SLIDER_CSS, open_props(1), vec![group]);
    let mut s = popup_scene(SLIDER_CSS, &decls(&bar)[0], Side::Below, 140, 140);
    s.set_open(true, t);
    s.set_window_focus(true, t);
    frame(&mut s, t);
    s.focus_first(true, t);
    // Up and Down are the slider's, not the group's.
    s.key(key_down(Key::ArrowUp), t);
    assert_eq!(values(&mut s), [(11.0, true)]);
    s.key(key_down(Key::ArrowDown), t);
    assert_eq!(values(&mut s), [(10.0, true)]);
    // Tab leaves it for the next focusable element; arrows then no longer change the slider.
    s.key(key_down(Key::Tab), t);
    s.key(key_down(Key::ArrowUp), t);
    assert!(values(&mut s).is_empty());
}

#[test]
fn a_slider_that_does_not_jump_treats_a_press_without_a_drag_as_a_click() {
    let t = Instant::now();
    let click = |mut e: Element| {
        e.handlers.click = Some(HandlerId(9));
        e
    };
    let mut el = click(slider_el(SliderProps {
        value: 40.0,
        jump: false,
        on_change: Some(HandlerId(4)),
        ..SliderProps::default()
    }));
    el.classes = vec![Arc::from("h")];
    let (bar, _) = bar_with_popup(SLIDER_CSS, open_props(1), vec![el]);
    let mut s = popup_scene(SLIDER_CSS, &decls(&bar)[0], Side::Below, 140, 140);
    s.set_open(true, t);
    s.set_window_focus(true, t);
    frame(&mut s, t);
    let r = node_by_class(&s, "h").rect;
    let at = |x: f32| PointF { x: r.x + x, y: r.y + 5.0 };
    let ev = |kind: PointerKind, p: PointF| PointerEvent { kind, x: p.x, y: p.y };
    // A press and release 2 px apart: a click, the value untouched, nothing reported.
    s.pointer(ev(PointerKind::Down(Button::Left), at(80.0)), t);
    s.pointer(ev(PointerKind::Move, at(82.0)), t + ms(5));
    let res = s.pointer(ev(PointerKind::Up(Button::Left), at(82.0)), t + ms(10));
    assert_eq!(res.calls.len(), 1, "{:?}", res.calls);
    assert_eq!((res.calls[0].handler.0, res.calls[0].kind), (9, rsb_core::hit::InputKind::Click(Button::Left)));
    assert!(values(&mut s).is_empty(), "the level did not move");
    let out = frame(&mut s, t + ms(20));
    assert_eq!(fill_rects(&out.draw), [(r.x, r.y, 40.0, 10.0)], "and the fill still shows 40");
    // A drag past the slop follows the pointer from there, reports it, and is not a click.
    s.pointer(ev(PointerKind::Down(Button::Left), at(80.0)), t + ms(100));
    s.pointer(ev(PointerKind::Move, at(90.0)), t + ms(110));
    let res = s.pointer(ev(PointerKind::Up(Button::Left), at(90.0)), t + ms(120));
    assert!(res.calls.is_empty(), "a drag is no click");
    let v = values(&mut s);
    assert_eq!(v.last(), Some(&(90.0, true)), "{v:?}");
    // Keys still change the value of a slider that does not jump.
    s.focus_first(true, t + ms(200));
    s.key(key_down(Key::ArrowLeft), t + ms(210));
    assert_eq!(values(&mut s).last(), Some(&(89.0, true)));
}

#[test]
fn a_slider_jumps_to_the_press_by_default() {
    let t = Instant::now();
    let mut s = slider_scene(SliderProps::default(), "h", t);
    let r = node_by_class(&s, "h").rect;
    s.pointer(PointerEvent { kind: PointerKind::Down(Button::Left), x: r.x + 70.0, y: r.y + 5.0 }, t);
    assert_eq!(values(&mut s), [(70.0, false)]);
}

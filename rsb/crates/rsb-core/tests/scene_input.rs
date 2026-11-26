mod common;

use std::time::Instant;

use common::*;
use rsb_core::diag::{DiagKind, Plate};
use rsb_core::element::{HandlerId, Handlers, InstanceId};
use rsb_core::geom::PointF;
use rsb_core::hit::{Button, HandlerCall, InputKind, PlateTarget, PointerEvent, PointerKind, PointerResult};
use rsb_core::scene::Scene;

const CSS: &str = "bar { font-size: 10px } .outer { width: 60px; height: 24px; padding: 4px; background: #202020 } \
                   .inner { width: 20px; height: 16px; background: #404040 } .gone { display: none; width: 10px; height: 10px } \
                   .ghost { visibility: hidden; width: 10px; height: 10px } .clip { overflow: hidden; width: 20px; height: 10px } \
                   .wide { width: 80px; height: 10px; flex-shrink: 0 } .plain { width: 10px; height: 10px }";

fn ev(kind: PointerKind, p: PointF) -> PointerEvent {
    PointerEvent { kind, x: p.x, y: p.y }
}

fn centre(s: &Scene, class: &str) -> PointF {
    let r = node_by_class(s, class).rect;
    PointF { x: r.x + r.w / 2.0, y: r.y + r.h / 2.0 }
}

fn press(s: &mut Scene, p: PointF, button: Button, now: Instant) -> PointerResult {
    s.pointer(ev(PointerKind::Down(button), p), now);
    s.pointer(ev(PointerKind::Up(button), p), now)
}

fn press_at(s: &mut Scene, class: &str, button: Button, now: Instant) -> PointerResult {
    let p = centre(s, class);
    press(s, p, button, now)
}

fn nested() -> Scene {
    let inner = click(classed(boxed(vec![]), &["inner"]), 2);
    let outer = click(classed(boxed(vec![inner]), &["outer"]), 1);
    let mut s = scene_with(CSS, 300, 1.0, Some(outer), None, None);
    frame(&mut s, Instant::now());
    s
}

fn calls(r: &PointerResult) -> Vec<(u32, InputKind)> {
    r.calls.iter().map(|c| (c.handler.0, c.kind)).collect()
}

#[test]
fn the_deepest_node_with_a_handler_gets_the_click() {
    let t = Instant::now();
    let mut s = nested();
    let r = press_at(&mut s, "inner", Button::Left, t);
    assert_eq!(calls(&r), [(2, InputKind::Click(Button::Left))]);
    assert_eq!(r.unhandled, None);
    // On the outer padding the outer handler runs.
    let o = node_by_class(&s, "outer").rect;
    let r = press(&mut s, PointF { x: o.x + 1.0, y: o.y + 1.0 }, Button::Left, t);
    assert_eq!(calls(&r), [(1, InputKind::Click(Button::Left))]);
    assert_eq!(r.calls[0].instance, InstanceId(1));
}

#[test]
fn a_click_needs_down_and_up_on_the_same_node() {
    let t = Instant::now();
    let mut s = nested();
    let inner = centre(&s, "inner");
    let o = node_by_class(&s, "outer").rect;
    let edge = PointF { x: o.x + 1.0, y: o.y + 1.0 };
    s.pointer(ev(PointerKind::Down(Button::Left), inner), t);
    let r = s.pointer(ev(PointerKind::Up(Button::Left), edge), t);
    assert!(r.calls.is_empty(), "{:?}", r.calls);
    assert_eq!(r.unhandled, None, "a handler exists under the pointer: no menu");
    // An up with no down is no click.
    let r = s.pointer(ev(PointerKind::Up(Button::Left), inner), t);
    assert!(r.calls.is_empty());
    // A different button than the one pressed is none either.
    s.pointer(ev(PointerKind::Down(Button::Left), inner), t);
    let r = s.pointer(ev(PointerKind::Up(Button::Middle), inner), t);
    assert!(r.calls.is_empty());
}

#[test]
fn click_coordinates_are_logical_and_relative_to_the_node() {
    let t = Instant::now();
    let mut s = scene_with(CSS, 400, 2.0, Some(click(classed(boxed(vec![]), &["outer"]), 7)), None, None);
    frame(&mut s, t);
    let r = node_by_class(&s, "outer").rect;
    // 10 css px right and 6 down from the node's corner, in physical px at scale 2.
    let p = PointF { x: r.x + 20.0, y: r.y + 12.0 };
    let res = press(&mut s, p, Button::Left, t);
    assert_eq!(res.calls.len(), 1);
    let c: HandlerCall = res.calls[0];
    assert_eq!((c.x, c.y), (10.0, 6.0));
}

#[test]
fn right_and_middle_handlers_and_the_menu_request() {
    let t = Instant::now();
    let h = Handlers { right_click: Some(HandlerId(5)), middle_click: Some(HandlerId(6)), ..Handlers::default() };
    let only_left = click(classed(boxed(vec![]), &["inner"]), 1);
    let both = handlers(classed(boxed(vec![]), &["outer"]), h);
    let mut s = scene_with(CSS, 300, 1.0, Some(boxed(vec![only_left, both])), None, None);
    frame(&mut s, t);
    // A right-click handler wins: no menu.
    let r = press_at(&mut s, "outer", Button::Right, t);
    assert_eq!(calls(&r), [(5, InputKind::Click(Button::Right))]);
    assert_eq!(r.unhandled, None);
    let r = press_at(&mut s, "outer", Button::Middle, t);
    assert_eq!(calls(&r), [(6, InputKind::Click(Button::Middle))]);
    // A node with only a left handler leaves the right button unhandled: the binary opens the menu.
    let r = press_at(&mut s, "inner", Button::Right, t);
    assert!(r.calls.is_empty());
    assert_eq!(r.unhandled, Some(Button::Right));
    // So does empty bar space, in a gap where nothing is drawn.
    let r = press(&mut s, PointF { x: 290.0, y: 3.0 }, Button::Right, t);
    assert_eq!(r.unhandled, Some(Button::Right));
    // And the left button where nothing handles it.
    let r = press(&mut s, PointF { x: 290.0, y: 3.0 }, Button::Left, t);
    assert_eq!(r.unhandled, Some(Button::Left));
}

#[test]
fn enter_and_leave_run_for_the_nodes_the_pointer_enters_and_leaves() {
    let t = Instant::now();
    let hv = |e: u32, l: u32| Handlers { enter: Some(HandlerId(e)), leave: Some(HandlerId(l)), ..Handlers::default() };
    let inner = handlers(classed(boxed(vec![]), &["inner"]), hv(21, 22));
    let outer = handlers(classed(boxed(vec![inner]), &["outer"]), hv(11, 12));
    let mut s = scene_with(CSS, 300, 1.0, Some(outer), None, None);
    frame(&mut s, t);
    let o = node_by_class(&s, "outer").rect;
    let edge = PointF { x: o.x + 1.0, y: o.y + 1.0 };
    let r = s.pointer(ev(PointerKind::Move, edge), t);
    assert_eq!(calls(&r), [(11, InputKind::Enter)]);
    // Moving within the same node is silent.
    let r = s.pointer(ev(PointerKind::Move, PointF { x: edge.x + 1.0, y: edge.y }), t);
    assert!(r.calls.is_empty());
    // Entering the inner node: enter for it only.
    let r = s.pointer(ev(PointerKind::Move, centre(&s, "inner")), t);
    assert_eq!(calls(&r), [(21, InputKind::Enter)]);
    // Back out to the outer node: the inner one is left.
    let r = s.pointer(ev(PointerKind::Move, edge), t);
    assert_eq!(calls(&r), [(22, InputKind::Leave)]);
    let r = s.pointer(ev(PointerKind::Move, centre(&s, "inner")), t);
    assert_eq!(calls(&r), [(21, InputKind::Enter)]);
    // The pointer leaves the window: deepest first.
    let r = s.pointer(ev(PointerKind::Leave, PointF::default()), t);
    assert_eq!(calls(&r), [(22, InputKind::Leave), (12, InputKind::Leave)]);
}

#[test]
fn the_wheel_goes_to_the_deepest_wheel_handler() {
    let t = Instant::now();
    let w = |id| Handlers { wheel: Some(HandlerId(id)), ..Handlers::default() };
    let inner = classed(boxed(vec![]), &["inner"]);
    let outer = handlers(classed(boxed(vec![inner]), &["outer"]), w(3));
    let mut s = scene_with(CSS, 300, 1.0, Some(outer), None, None);
    frame(&mut s, t);
    let r = s.pointer(ev(PointerKind::Wheel(-1.0), centre(&s, "inner")), t);
    assert_eq!(calls(&r), [(3, InputKind::Wheel(-1.0))]);
    let r = s.pointer(ev(PointerKind::Wheel(2.5), PointF { x: 290.0, y: 3.0 }), t);
    assert!(r.calls.is_empty(), "no handler under the pointer");
    assert_eq!(r.unhandled, None, "a wheel turn is never a menu request");
}

#[test]
fn clipped_hidden_and_display_none_nodes_are_not_hit() {
    let t = Instant::now();
    let wide = click(classed(boxed(vec![]), &["wide"]), 1);
    let clip = classed(boxed(vec![wide]), &["clip"]);
    let gone = click(classed(boxed(vec![]), &["gone"]), 2);
    let ghost = click(classed(boxed(vec![]), &["ghost"]), 3);
    let plain = click(classed(boxed(vec![]), &["plain"]), 4);
    let mut s = scene_with(CSS, 300, 1.0, Some(boxed(vec![clip, gone, ghost, plain])), None, None);
    frame(&mut s, t);
    let c = node_by_class(&s, "clip").rect;
    let wide_r = node_by_class(&s, "wide").rect;
    assert_eq!(wide_r.w, 80.0);
    // Inside the clip: the wide child gets it. Past the clip's edge it is cut off.
    let inside = PointF { x: c.x + 5.0, y: c.y + 5.0 };
    assert_eq!(calls(&press(&mut s, inside, Button::Left, t)), [(1, InputKind::Click(Button::Left))]);
    let outside = PointF { x: c.right() + 20.0, y: c.y + 5.0 };
    assert!(wide_r.contains(outside), "the child reaches there");
    let r = press(&mut s, outside, Button::Left, t);
    assert!(r.calls.is_empty(), "clipped away: {:?}", r.calls);
    // A visibility:hidden node keeps its space but is not hit; the next node's own area is.
    let g = node_by_class(&s, "ghost").rect;
    let r = press(&mut s, PointF { x: g.x + 2.0, y: g.y + 2.0 }, Button::Left, t);
    assert!(r.calls.is_empty());
    assert_eq!(r.unhandled, Some(Button::Left));
    let r = press_at(&mut s, "plain", Button::Left, t);
    assert_eq!(calls(&r), [(4, InputKind::Click(Button::Left))]);
    // display: none takes no space at all.
    assert_eq!(node_by_class(&s, "gone").rect.w, 0.0);
}

#[test]
fn a_clicked_plate_is_reported_and_runs_no_script() {
    let t = Instant::now();
    let plate = |text: &str| Plate { kind: DiagKind::Stopped, text: text.into(), more: 0 };
    let mut s = scene_with(CSS, 400, 1.0, Some(text("fine")), Some(text("also fine")), None);
    s.set_widget_plate(InstanceId(1), Some(plate("STOPPED - w ran over 10 ms")));
    frame(&mut s, t);
    let p = node_by_class(&s, "l").rect;
    let at = PointF { x: p.x + 5.0, y: p.y + p.h / 2.0 };
    let r = press(&mut s, at, Button::Left, t);
    assert_eq!(r.plate_click, Some(PlateTarget::Widget(InstanceId(1))));
    assert!(r.calls.is_empty());
    assert_eq!(r.unhandled, None);
    // Press on the plate, release elsewhere: no click.
    s.pointer(ev(PointerKind::Down(Button::Left), at), t);
    let r = s.pointer(ev(PointerKind::Up(Button::Left), PointF { x: 395.0, y: 3.0 }), t);
    assert_eq!(r.plate_click, None);
    // The hand cursor shows over a plate.
    s.pointer(ev(PointerKind::Move, at), t);
    assert_eq!(frame(&mut s, t).cursor, rsb_core::style::Cursor::Pointer);

    // The bar plate.
    let mut s = scene_with(CSS, 400, 1.0, None, None, None);
    s.set_bar_plate(Some(plate("RSB ERROR - style.css:1:1 x")));
    frame(&mut s, t);
    let p = node_by_class_kind(&s);
    let r = press(&mut s, p, Button::Left, t);
    assert_eq!(r.plate_click, Some(PlateTarget::Bar));
    // Right button on a plate: still just a plate click only for the left? It is a click of any button.
    let r = press(&mut s, p, Button::Right, t);
    assert_eq!(r.plate_click, Some(PlateTarget::Bar));
    assert_eq!(r.unhandled, None);
}

fn node_by_class_kind(s: &Scene) -> PointF {
    let n = s.nodes().into_iter().find(|n| n.kind == rsb_core::element::ElementKind::Plate).expect("a plate");
    PointF { x: n.rect.x + n.rect.w / 2.0, y: n.rect.y + n.rect.h / 2.0 }
}

#[test]
fn handler_ids_of_an_old_frame_do_not_reach_a_new_handler() {
    // The scene returns what the tree it holds says; the runtime ignores ids it does
    // not know. After a new tree with different ids the old id is never returned.
    let t = Instant::now();
    let mut s = scene_with(CSS, 300, 1.0, Some(click(classed(boxed(vec![]), &["outer"]), 1)), None, None);
    frame(&mut s, t);
    let p = centre(&s, "outer");
    s.set_tree(tree(1, 2, Some(click(classed(boxed(vec![]), &["outer"]), 9))));
    frame(&mut s, t);
    let r = press(&mut s, p, Button::Left, t);
    assert_eq!(calls(&r), [(9, InputKind::Click(Button::Left))]);
}

#[test]
fn a_press_that_leaves_the_window_is_cancelled() {
    let t = Instant::now();
    let mut s = nested();
    let p = centre(&s, "inner");
    s.pointer(ev(PointerKind::Down(Button::Left), p), t);
    s.pointer(ev(PointerKind::Leave, PointF::default()), t);
    let r = s.pointer(ev(PointerKind::Up(Button::Left), p), t);
    assert!(r.calls.is_empty());
}

#[test]
fn points_outside_the_bar_hit_nothing() {
    let t = Instant::now();
    let mut s = nested();
    let r = press(&mut s, PointF { x: -5.0, y: -5.0 }, Button::Left, t);
    assert_eq!(r.unhandled, Some(Button::Left));
    let r = press(&mut s, PointF { x: 100.0, y: 500.0 }, Button::Right, t);
    assert_eq!(r.unhandled, Some(Button::Right));
}

// ---- the strip above the widgets ------------------------------------------------------------

#[test]
fn the_strip_above_the_widgets_belongs_to_the_widget_below_it() {
    // The default bar hangs its segments from 10 px down; a pointer pushed against the top edge of the screen is
    // in that empty strip and must still reach the widget under it.
    let css = "bar { font-size: 10px; padding: 10px 0 0 0 } .btn { width: 40px; height: 20px; background: #404040 }";
    let t = Instant::now();
    let mut s = scene_with(css, 300, 1.0, Some(click(classed(boxed(vec![]), &["btn"]), 7)), None, None);
    frame(&mut s, t);
    let btn = node_by_class(&s, "btn").rect;
    assert!(btn.y >= 10.0, "the button hangs below the 10 px strip: {btn:?}");
    for y in [0.0, 3.0, btn.y - 1.0] {
        let p = PointF { x: btn.x + btn.w / 2.0, y };
        let r = press(&mut s, p, Button::Left, t);
        assert_eq!(calls(&r), [(7, InputKind::Click(Button::Left))], "a press at y {y} reaches the button");
    }
    // Hover follows too, so the pointer cursor shows at the edge.
    s.pointer(ev(PointerKind::Move, PointF { x: btn.x + 5.0, y: 0.0 }), t);
    assert_eq!(frame(&mut s, t).cursor, rsb_core::style::Cursor::Pointer);
    // Above empty space nothing is hit: the strip is not stretched sideways.
    let r = press(&mut s, PointF { x: btn.right() + 50.0, y: 0.0 }, Button::Left, t);
    assert!(r.calls.is_empty());
}

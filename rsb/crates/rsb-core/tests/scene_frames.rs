mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use rsb_core::color::Color;
use rsb_core::draw::{DrawCmd, Fill};
use rsb_core::element::{Element, ElementKind, ImageRef, InstanceId};
use rsb_core::geom::{PointF, RectPx};
use rsb_core::hit::{PointerEvent, PointerKind};
use rsb_core::scene::Scene;
use rsb_core::style::Cursor;

const BASE: &str = "bar { font-size: 10px }";

fn at(base: Instant, millis: u64) -> Instant {
    base + Duration::from_millis(millis)
}

fn mv(s: &mut Scene, x: f32, y: f32, now: Instant) {
    s.pointer(PointerEvent { kind: PointerKind::Move, x, y }, now);
}

fn centre(s: &Scene, class: &str) -> PointF {
    let r = node_by_class(s, class).rect;
    PointF { x: r.x + r.w / 2.0, y: r.y + r.h / 2.0 }
}

fn fill_colour(out: &rsb_core::scene::FrameOut, class_rect: rsb_core::geom::RectF) -> Color {
    out.draw
        .items
        .iter()
        .find_map(|i| match &i.cmd {
            DrawCmd::Fill { rect, fill: Fill::Solid(c), .. } if *rect == class_rect => Some(*c),
            _ => None,
        })
        .expect("the box has a background")
}

#[test]
fn the_first_frame_is_the_whole_surface_and_an_unchanged_scene_needs_none() {
    let t0 = Instant::now();
    let mut s = scene_with(BASE, 300, 1.0, Some(text("abc")), Some(text("def")), None);
    assert!(s.needs_frame(t0));
    let f = frame(&mut s, t0);
    assert_eq!(f.dirty, Some(RectPx::new(0, 0, 300, 32)));
    assert!(f.next_frame.is_none());
    assert!(!s.needs_frame(t0));
    let f = frame(&mut s, at(t0, 5));
    assert_eq!(f.dirty, None, "nothing changed: no frame at all");
}

#[test]
fn a_pointer_move_that_changes_nothing_hands_back_the_same_draw_list() {
    let t0 = Instant::now();
    let mut s = scene_with(BASE, 300, 1.0, Some(text("abc")), Some(text("def")), None);
    let _ = frame(&mut s, t0);
    // The first move enters the bar (a hover change) and is drawn.
    mv(&mut s, 290.0, 4.0, at(t0, 5));
    let first = frame(&mut s, at(t0, 6));
    // The next ones stay on the same nodes: nothing changes.
    mv(&mut s, 291.0, 4.0, at(t0, 7));
    mv(&mut s, 292.0, 5.0, at(t0, 8));
    let second = frame(&mut s, at(t0, 9));
    assert_eq!(second.dirty, None);
    assert!(Arc::ptr_eq(&first.draw, &second.draw), "the list was not rebuilt");
    assert_eq!(second.cursor, first.cursor);
    assert_eq!(second.backdrop, first.backdrop);

    // A real change still draws: new text after the idle frames.
    s.set_tree(tree(1, 2, Some(text("abd"))));
    let third = frame(&mut s, at(t0, 8));
    assert!(third.dirty.is_some());
    assert!(!Arc::ptr_eq(&second.draw, &third.draw));
}

#[test]
fn a_one_text_change_damages_only_that_text() {
    let t0 = Instant::now();
    let mut s = scene_with(BASE, 300, 1.0, Some(text("abc")), Some(text("12:00")), Some(text("xyz")));
    frame(&mut s, t0);
    let before = s.nodes().into_iter().find(|n| n.text.as_deref() == Some("12:00")).unwrap();
    s.set_tree(tree(2, 2, Some(text("12:01"))));
    let f = frame(&mut s, at(t0, 10));
    let after = s.nodes().into_iter().find(|n| n.text.as_deref() == Some("12:01")).unwrap();
    assert_eq!(before.rect, after.rect, "same width: nothing moved");
    // The text's content box, rounded outwards: exactly that and nothing else.
    assert_eq!(f.dirty, Some(RectPx::outward(&after.rect)));
    assert!(f.stats.relayout);
    assert!(f.stats.restyled <= 1, "only the changed node is restyled: {}", f.stats.restyled);
    // The frame holds the new text.
    assert!(texts(&f.draw.items).contains(&"12:01".to_string()));
}

#[test]
fn a_wider_text_moves_its_neighbours_and_damages_both() {
    let t0 = Instant::now();
    let mut s = scene_with(BASE, 300, 1.0, Some(text("abc")), Some(text("12")), Some(text("xyz")));
    frame(&mut s, t0);
    s.set_tree(tree(1, 2, Some(text("abcdefghij"))));
    let f = frame(&mut s, at(t0, 10));
    let d = f.dirty.expect("changed");
    // The left text grew from x 0..18 to 0..60: the damage reaches the new right edge.
    assert!(d.x0 <= 0 && d.x1 >= 60, "{d:?}");
    // The right zone did not move, so the damage does not reach it.
    assert!(d.x1 < 270, "{d:?}");
}

#[test]
fn removing_a_widget_damages_where_it_was() {
    let t0 = Instant::now();
    let mut s = scene_with(BASE, 300, 1.0, Some(text("abc")), Some(text("middle")), None);
    frame(&mut s, t0);
    let old = s.nodes().into_iter().find(|n| n.text.as_deref() == Some("middle")).unwrap().rect;
    s.set_tree(tree(2, 2, None));
    let f = frame(&mut s, at(t0, 5));
    let d = f.dirty.unwrap();
    assert_eq!(d, RectPx::outward(&old));
    assert!(!texts(&f.draw.items).contains(&"middle".to_string()));
}

#[test]
fn an_older_tree_version_is_ignored() {
    let t0 = Instant::now();
    let mut s = scene_with(BASE, 300, 1.0, Some(text("new")), None, None);
    s.set_tree(tree(1, 5, Some(text("newer"))));
    s.set_tree(tree(1, 3, Some(text("stale"))));
    let f = frame(&mut s, t0);
    assert_eq!(texts(&f.draw.items), ["newer"]);
    // A tree for an instance no slot holds is ignored too.
    s.set_tree(tree(9, 1, Some(text("other bar"))));
    let f = frame(&mut s, at(t0, 1));
    assert_eq!(f.dirty, None);
}

fn image_element(w: Option<f32>, h: Option<f32>) -> Element {
    let mut e = Element::new(ElementKind::Image);
    e.image = Some(ImageRef { path: Arc::from("img-one"), width: w, height: h });
    classed(e, &["pic"])
}

#[test]
fn set_image_size_damages_the_image_node_and_gives_it_its_size() {
    let t0 = Instant::now();
    // No size known: the node is 0 x 0 and draws nothing.
    let mut s = scene_with(BASE, 300, 1.0, Some(boxed(vec![text("ab"), image_element(None, None)])), None, None);
    let f = frame(&mut s, t0);
    assert!(!f.draw.items.iter().any(|i| matches!(i.cmd, DrawCmd::Image { .. })));
    assert_eq!(node_by_class(&s, "pic").rect.w, 0.0);
    // The binary tells the scene the decoded size.
    s.set_image_size("img-one", 20, 10);
    let f = frame(&mut s, at(t0, 5));
    let pic = node_by_class(&s, "pic");
    assert_eq!((pic.rect.w, pic.rect.h), (20.0, 10.0));
    let item = f.draw.items.iter().find(|i| matches!(i.cmd, DrawCmd::Image { .. })).expect("image is drawn now");
    let d = f.dirty.expect("damaged");
    let b = RectPx::outward(&item.bounds);
    assert!(d.x0 <= b.x0 && d.x1 >= b.x1 && d.y0 <= b.y0 && d.y1 >= b.y1, "{d:?} covers {b:?}");

    // The same size told again: the list is equal, but the image node is still redrawn.
    s.set_image_size("img-one", 20, 10);
    let f = frame(&mut s, at(t0, 10));
    let item = f.draw.items.iter().find(|i| matches!(i.cmd, DrawCmd::Image { .. })).unwrap();
    assert_eq!(f.dirty, Some(RectPx::outward(&item.bounds).intersect(&RectPx::new(0, 0, 300, 32)).unwrap()));
    let f = frame(&mut s, at(t0, 15));
    assert_eq!(f.dirty, None);

    // A failed image is not drawn and its old place is damaged.
    let before = item.bounds;
    s.image_failed("img-one");
    let f = frame(&mut s, at(t0, 20));
    assert!(!f.draw.items.iter().any(|i| matches!(i.cmd, DrawCmd::Image { .. })));
    let d = f.dirty.expect("damaged");
    let b = RectPx::outward(&before);
    assert!(d.x0 <= b.x0 && d.x1 >= b.x1, "{d:?} covers {b:?}");
}

#[test]
fn an_image_with_one_dimension_keeps_its_aspect() {
    let t0 = Instant::now();
    let mut s = scene_with(BASE, 300, 1.0, Some(image_element(Some(40.0), None)), None, None);
    s.set_image_size("img-one", 200, 100);
    frame(&mut s, t0);
    let pic = node_by_class(&s, "pic");
    assert_eq!((pic.rect.w, pic.rect.h), (40.0, 20.0));
    let mut s = scene_with(BASE, 300, 2.0, Some(image_element(Some(10.0), Some(10.0))), None, None);
    frame(&mut s, t0);
    let pic = node_by_class(&s, "pic");
    assert_eq!((pic.rect.w, pic.rect.h), (20.0, 20.0));
}

const FADE: &str = "bar { font-size: 10px } .b { background: #000000; width: 40px; height: 20px; transition: background-color 100ms linear } .b:hover { background: #ffffff }";

fn lum(c: Color) -> f32 {
    c.r
}

#[test]
fn hover_restyles_and_transitions_run_then_stop() {
    let t0 = Instant::now();
    let mut s = scene_with(FADE, 300, 1.0, Some(classed(boxed(vec![]), &["b"])), None, None);
    let f = frame(&mut s, t0);
    let rect = node_by_class(&s, "b").rect;
    assert!(f.next_frame.is_none());
    assert_eq!(lum(fill_colour(&f, rect)), 0.0);

    let p = centre(&s, "b");
    mv(&mut s, p.x, p.y, at(t0, 100));
    let f = frame(&mut s, at(t0, 100));
    assert!(f.next_frame.is_some(), "a transition is running");
    assert_eq!(f.stats.running_transitions, 1);
    assert!(lum(fill_colour(&f, rect)) < 0.05, "starts from the old colour");

    let f = frame(&mut s, at(t0, 150));
    let mid = lum(fill_colour(&f, rect));
    assert!((mid - 0.5).abs() < 0.02, "linear halfway: {mid}");
    assert!(f.dirty.is_some(), "a frame with a new colour is damage");
    assert!(f.next_frame.is_some());

    let f = frame(&mut s, at(t0, 200));
    assert_eq!(lum(fill_colour(&f, rect)), 1.0, "finished");
    assert!(f.next_frame.is_none(), "frames stop when the transition is done");
    assert_eq!(f.stats.running_transitions, 0);
    let f = frame(&mut s, at(t0, 216));
    assert_eq!(f.dirty, None);
    assert!(!s.needs_frame(at(t0, 216)));
}

#[test]
fn an_interrupted_transition_continues_from_its_current_value() {
    let t0 = Instant::now();
    let mut s = scene_with(FADE, 300, 1.0, Some(classed(boxed(vec![]), &["b"])), None, None);
    frame(&mut s, t0);
    let rect = node_by_class(&s, "b").rect;
    let p = centre(&s, "b");
    mv(&mut s, p.x, p.y, at(t0, 100));
    frame(&mut s, at(t0, 100));
    let f = frame(&mut s, at(t0, 140));
    let at_leave = lum(fill_colour(&f, rect));
    assert!((at_leave - 0.4).abs() < 0.02, "{at_leave}");
    // The pointer leaves 40 ms in: the fade back starts from 0.4, not from 1 or 0.
    mv(&mut s, 290.0, 5.0, at(t0, 140));
    let f = frame(&mut s, at(t0, 140));
    let start = lum(fill_colour(&f, rect));
    assert!((start - at_leave).abs() < 0.02, "continues from {at_leave}, got {start}");
    let f = frame(&mut s, at(t0, 170));
    let later = lum(fill_colour(&f, rect));
    assert!(later < start && later > 0.0, "fading back down: {later}");
    let f = frame(&mut s, at(t0, 300));
    assert_eq!(lum(fill_colour(&f, rect)), 0.0);
    assert!(f.next_frame.is_none());
}

#[test]
fn state_survives_a_rebuild_of_the_same_nodes() {
    let t0 = Instant::now();
    let mut s = scene_with(FADE, 300, 1.0, Some(classed(boxed(vec![]), &["b"])), None, None);
    frame(&mut s, t0);
    let rect = node_by_class(&s, "b").rect;
    let p = centre(&s, "b");
    mv(&mut s, p.x, p.y, at(t0, 0));
    frame(&mut s, at(t0, 0));
    let f = frame(&mut s, at(t0, 50));
    let mid = lum(fill_colour(&f, rect));
    // A new tree (a rebuild) arrives mid-transition: hover and the running transition carry on.
    s.set_tree(tree(1, 2, Some(classed(boxed(vec![]), &["b"]))));
    let f = frame(&mut s, at(t0, 60));
    let later = lum(fill_colour(&f, rect));
    assert!(later > mid && later < 1.0, "{mid} -> {later}");
    let f = frame(&mut s, at(t0, 120));
    assert_eq!(lum(fill_colour(&f, rect)), 1.0, "still hovered after the rebuild");
}

#[test]
fn hover_without_a_transition_changes_at_once_and_active_follows_the_button() {
    let css = ".b { background: #000000; width: 40px; height: 20px } .b:hover { background: #404040 } .b:active { background: #808080 }";
    let t0 = Instant::now();
    let mut s = scene_with(css, 300, 1.0, Some(classed(boxed(vec![]), &["b"])), None, None);
    frame(&mut s, t0);
    let rect = node_by_class(&s, "b").rect;
    let p = centre(&s, "b");
    mv(&mut s, p.x, p.y, t0);
    let f = frame(&mut s, t0);
    assert!((lum(fill_colour(&f, rect)) - 64.0 / 255.0).abs() < 0.01);
    assert!(f.next_frame.is_none());
    s.pointer(PointerEvent { kind: PointerKind::Down(rsb_core::hit::Button::Left), x: p.x, y: p.y }, t0);
    let f = frame(&mut s, t0);
    assert!((lum(fill_colour(&f, rect)) - 128.0 / 255.0).abs() < 0.01);
    s.pointer(PointerEvent { kind: PointerKind::Up(rsb_core::hit::Button::Left), x: p.x, y: p.y }, t0);
    let f = frame(&mut s, t0);
    assert!((lum(fill_colour(&f, rect)) - 64.0 / 255.0).abs() < 0.01, "back to hover");
    s.pointer(PointerEvent { kind: PointerKind::Leave, x: 0.0, y: 0.0 }, t0);
    let f = frame(&mut s, t0);
    assert_eq!(lum(fill_colour(&f, rect)), 0.0);
}

#[test]
fn a_hover_rule_that_changes_size_relayouts() {
    let css = ".b { width: 40px; height: 20px; background: #111111 } .b:hover { width: 60px }";
    let t0 = Instant::now();
    let mut s = scene_with(css, 300, 1.0, Some(classed(boxed(vec![]), &["b"])), Some(text("c")), None);
    let f = frame(&mut s, t0);
    assert!(f.stats.relayout);
    let p = centre(&s, "b");
    mv(&mut s, p.x, p.y, t0);
    let f = frame(&mut s, t0);
    assert!(f.stats.relayout);
    assert_eq!(node_by_class(&s, "b").rect.w, 60.0);
    // A hover rule that only recolours does not relayout.
    let css = ".b { width: 40px; height: 20px; background: #111111 } .b:hover { background: #222222 }";
    let mut s = scene_with(css, 300, 1.0, Some(classed(boxed(vec![]), &["b"])), None, None);
    frame(&mut s, t0);
    let p = centre(&s, "b");
    mv(&mut s, p.x, p.y, t0);
    let f = frame(&mut s, t0);
    assert!(!f.stats.relayout);
    assert!(f.dirty.is_some());
}

#[test]
fn the_cursor_is_a_hand_over_clickable_nodes_and_css_pointer() {
    let css = ".p { cursor: pointer; width: 10px; height: 10px } .n, .k { width: 10px; height: 10px }";
    let t0 = Instant::now();
    let kids =
        vec![classed(boxed(vec![]), &["p"]), classed(boxed(vec![]), &["n"]), click(classed(boxed(vec![]), &["k"]), 1)];
    let mut s = scene_with(css, 300, 1.0, Some(boxed(kids)), None, None);
    frame(&mut s, t0);
    for (class, want) in [("p", Cursor::Pointer), ("n", Cursor::Default), ("k", Cursor::Pointer)] {
        let p = centre(&s, class);
        mv(&mut s, p.x, p.y, t0);
        let f = frame(&mut s, t0);
        assert_eq!(f.cursor, want, "{class}");
    }
}

#[test]
fn a_stylesheet_change_applies_at_once_and_removing_a_rule_removes_its_effect() {
    let t0 = Instant::now();
    let mut s = scene_with("bar { background: #ff0000 }", 100, 1.0, Some(text("a")), None, None);
    let f = frame(&mut s, t0);
    assert!(f.draw.items.iter().any(|i| matches!(&i.cmd, DrawCmd::Fill { fill: Fill::Solid(c), .. } if c.r == 1.0)));
    s.set_stylesheet(sheet("bar { background: #0000ff }"));
    let f = frame(&mut s, at(t0, 1));
    assert!(f.dirty.is_some());
    assert!(!f.draw.items.iter().any(|i| matches!(&i.cmd, DrawCmd::Fill { fill: Fill::Solid(c), .. } if c.r == 1.0)));
    s.set_stylesheet(sheet(""));
    let f = frame(&mut s, at(t0, 2));
    assert_eq!(
        f.draw.items.len(),
        2,
        "hit fill and the text only: {:?}",
        f.draw.items.iter().map(|i| &i.cmd).collect::<Vec<_>>()
    );
}

#[test]
fn light_and_dark_media_follow_the_environment() {
    let css = "bar { background: #ffffff } @media (prefers-color-scheme: dark) { bar { background: #101010 } }";
    let t0 = Instant::now();
    let mut s = scene_with(css, 100, 1.0, Some(text("a")), None, None);
    let bg = |s: &mut Scene, t: Instant| {
        let f = frame(s, t);
        match &f.draw.items[1].cmd {
            DrawCmd::Fill { fill: Fill::Solid(c), .. } => c.r,
            other => panic!("{other:?}"),
        }
    };
    assert!(bg(&mut s, t0) < 0.1, "dark");
    s.set_env(env(false));
    assert_eq!(bg(&mut s, at(t0, 1)), 1.0);
    s.set_env(env(true));
    assert!(bg(&mut s, at(t0, 2)) < 0.1);
}

#[test]
fn scene_diagnostics_are_positioned_and_reported_once() {
    let t0 = Instant::now();
    // Inline style problems point at the Luau line that made the element.
    let mut e = text("a");
    e.style = vec![("colr".into(), "red".into()), ("padding".into(), "wide".into())];
    e.pos = Some(rsb_core::diag::SourcePos::new("widgets/my/x.luau", 12, 0));
    let mut icon = Element::new(ElementKind::Icon);
    icon.icon = Some(rsb_core::element::IconRef::Named(Arc::from("volum_2")));
    icon.pos = Some(rsb_core::diag::SourcePos::new("widgets/my/x.luau", 14, 0));
    let mut s = scene_with("", 300, 1.0, Some(boxed(vec![e, icon])), None, None);
    frame(&mut s, t0);
    let d = s.take_diagnostics();
    let lines: Vec<String> = d.iter().map(|d| d.diagnostic.to_string()).collect();
    assert_eq!(d.len(), 3, "{lines:?}");
    assert!(d.iter().all(|x| x.instance == Some(InstanceId(1))));
    assert_eq!(lines[0], "warning: widgets/my/x.luau:12: unknown property 'colr'; did you mean 'color'?");
    assert!(lines[1].starts_with("warning: widgets/my/x.luau:12: bad value for 'padding'"), "{lines:?}");
    assert_eq!(lines[2], "warning: widgets/my/x.luau:14: unknown icon 'volum_2'; did you mean 'volume_2'?");
    // Taken once.
    assert!(s.take_diagnostics().is_empty());
    // A clock tick (same problems) does not repeat them.
    let mut e = text("b");
    e.style = vec![("colr".into(), "red".into()), ("padding".into(), "wide".into())];
    e.pos = Some(rsb_core::diag::SourcePos::new("widgets/my/x.luau", 12, 0));
    let mut icon = Element::new(ElementKind::Icon);
    icon.icon = Some(rsb_core::element::IconRef::Named(Arc::from("volum_2")));
    icon.pos = Some(rsb_core::diag::SourcePos::new("widgets/my/x.luau", 14, 0));
    s.set_tree(tree(1, 2, Some(boxed(vec![e, icon]))));
    frame(&mut s, at(t0, 1000));
    assert!(s.take_diagnostics().is_empty());
    // A new style sheet reports again (the sheet may be what is wrong).
    s.set_stylesheet(sheet(""));
    frame(&mut s, at(t0, 1001));
    assert_eq!(s.take_diagnostics().len(), 3);
    // Fixed, then broken again: reported again.
    s.set_tree(tree(1, 3, Some(text("ok"))));
    frame(&mut s, at(t0, 1002));
    assert!(s.take_diagnostics().is_empty());
    let mut e = text("b");
    e.style = vec![("colr".into(), "red".into())];
    e.pos = Some(rsb_core::diag::SourcePos::new("widgets/my/x.luau", 12, 0));
    s.set_tree(tree(1, 4, Some(e)));
    frame(&mut s, at(t0, 1003));
    assert_eq!(s.take_diagnostics().len(), 1);
}

#[test]
fn a_shadow_on_the_bar_warns() {
    let t0 = Instant::now();
    let mut s = scene_with("bar { box-shadow: 0 2px 4px #000000 }", 100, 1.0, Some(text("a")), None, None);
    frame(&mut s, t0);
    let d = s.take_diagnostics();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].instance, None);
    assert_eq!(d[0].diagnostic.message, "box-shadow on bar is cut off by the window edge");
}

#[test]
fn a_preset_rule_that_uses_a_bad_user_variable_points_at_the_users_value() {
    use rsb_core::css::Stylesheet;
    let mut sheet = Stylesheet::empty();
    sheet.append(Stylesheet::parse(".a { padding: var(--pad) }", "preset.css").0);
    sheet.append(Stylesheet::parse(":root { --pad: wide }", "style.css").0);
    let t0 = Instant::now();
    let mut s = scene_with("", 100, 1.0, Some(classed(text("a"), &["a"])), None, None);
    s.set_stylesheet(Arc::new(sheet));
    frame(&mut s, t0);
    let d = s.take_diagnostics();
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(
        d[0].diagnostic.plate_text(),
        "RSB ERROR - style.css:1:9 --pad: expected a length like 12px, got 'wide' (the preset uses it for 'padding')"
    );
}

#[test]
fn var_problems_come_through_the_scene_with_both_positions() {
    let css = ":root { --pad: wide; --gap: 4px }\n.a { padding: var(--pad); gap: var(--gap) }\n";
    let (sheet, d) = rsb_core::css::Stylesheet::parse(css, "style.css");
    assert!(d.is_empty());
    let t0 = Instant::now();
    let mut s = scene_with("", 100, 1.0, Some(classed(text("a"), &["a"])), None, None);
    s.set_stylesheet(Arc::new(sheet));
    frame(&mut s, t0);
    let d = s.take_diagnostics();
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(
        d[0].diagnostic.to_string(),
        "warning: style.css:2:6: 'padding' is invalid after var(--pad) (defined at style.css:1:9): expected a length like 12px, got 'wide'"
    );
    // The property is unset, the rest of the rule applies.
    let a = node_by_class(&s, "a");
    assert_eq!(a.style.padding.left, 0.0);
    assert_eq!(a.style.gap, 4.0);
    // Reported once per style sheet.
    s.set_tree(tree(1, 2, Some(classed(text("a"), &["a"]))));
    frame(&mut s, at(t0, 5));
    assert!(s.take_diagnostics().is_empty());
}

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use rsb_core::color::Color;
use rsb_core::draw::{DrawCmd, Fill};
use rsb_core::element::Element;
use rsb_core::geom::{PointF, RectPx};
use rsb_core::hit::{PointerEvent, PointerKind};
use rsb_core::scene::Scene;

fn at(base: Instant, millis: u64) -> Instant {
    base + Duration::from_millis(millis)
}

fn mv(s: &mut Scene, p: PointF, now: Instant) {
    s.pointer(PointerEvent { kind: PointerKind::Move, x: p.x, y: p.y }, now);
}

fn centre(s: &Scene, class: &str) -> PointF {
    let r = node_by_class(s, class).rect;
    PointF { x: r.x + r.w / 2.0, y: r.y + r.h / 2.0 }
}

fn bg(items: &[rsb_core::draw::DrawItem], index: usize) -> Color {
    match &items[index].cmd {
        DrawCmd::Fill { fill: Fill::Solid(c), .. } => *c,
        other => panic!("{other:?}"),
    }
}

fn fills(f: &rsb_core::scene::FrameOut) -> Vec<f32> {
    f.draw
        .items
        .iter()
        .filter_map(|i| match &i.cmd {
            DrawCmd::Fill { fill: Fill::Solid(c), .. } if c.a > 0.5 => Some(c.r),
            _ => None,
        })
        .collect()
}

#[test]
fn hovering_a_child_hovers_its_ancestors() {
    let css =
        ".p { background: #000000; padding: 2px } .p:hover { background: #ffffff } .c { width: 10px; height: 10px }";
    let t0 = Instant::now();
    let mut s =
        scene_with(css, 200, 1.0, Some(classed(boxed(vec![classed(boxed(vec![]), &["c"])]), &["p"])), None, None);
    frame(&mut s, t0);
    let p = centre(&s, "c");
    mv(&mut s, p, t0);
    let f = frame(&mut s, t0);
    assert!(fills(&f).contains(&1.0), "the parent is in the hover chain: {:?}", fills(&f));
    mv(&mut s, PointF { x: 190.0, y: 3.0 }, t0);
    let f = frame(&mut s, t0);
    assert!(!fills(&f).contains(&1.0));
}

#[test]
fn a_transition_delay_holds_the_old_value_first() {
    let css = ".b { background: #000000; width: 20px; height: 10px; transition: background-color 100ms linear 50ms } .b:hover { background: #ffffff }";
    let t0 = Instant::now();
    let mut s = scene_with(css, 200, 1.0, Some(classed(boxed(vec![]), &["b"])), None, None);
    frame(&mut s, t0);
    let p = centre(&s, "b");
    mv(&mut s, p, t0);
    let f = frame(&mut s, t0);
    assert!(f.next_frame.is_some(), "a transition with a pending delay keeps frames coming");
    let f = frame(&mut s, at(t0, 25));
    assert_eq!(bg(&f.draw.items, 1).r, 0.0, "still the old colour inside the delay");
    let f = frame(&mut s, at(t0, 100));
    assert!((bg(&f.draw.items, 1).r - 0.5).abs() < 0.02);
    let f = frame(&mut s, at(t0, 150));
    assert_eq!(bg(&f.draw.items, 1).r, 1.0);
    assert!(f.next_frame.is_none());
}

#[test]
fn a_size_transition_relayouts_every_frame_until_done() {
    let css = ".b { background: #101010; width: 40px; height: 10px; transition: width 100ms linear } .b:hover { width: 60px }";
    let t0 = Instant::now();
    let mut s = scene_with(css, 200, 1.0, Some(classed(boxed(vec![]), &["b"])), Some(text("c")), None);
    frame(&mut s, t0);
    let p = centre(&s, "b");
    mv(&mut s, p, t0);
    frame(&mut s, t0);
    let f = frame(&mut s, at(t0, 50));
    assert!(f.stats.relayout);
    assert_eq!(node_by_class(&s, "b").rect.w, 50.0);
    let f = frame(&mut s, at(t0, 100));
    assert_eq!(node_by_class(&s, "b").rect.w, 60.0);
    assert!(f.next_frame.is_none());
    let f = frame(&mut s, at(t0, 120));
    assert_eq!(f.dirty, None);
}

#[test]
fn a_class_change_in_a_new_tree_starts_a_transition() {
    let css = ".b { background: #000000; width: 20px; height: 10px; transition: background-color 100ms linear } .b.on { background: #ffffff }";
    let t0 = Instant::now();
    let mut s = scene_with(css, 200, 1.0, Some(classed(boxed(vec![]), &["b"])), None, None);
    frame(&mut s, t0);
    s.set_tree(tree(1, 2, Some(classed(boxed(vec![]), &["b", "on"]))));
    let f = frame(&mut s, at(t0, 10));
    assert!(f.next_frame.is_some());
    let f = frame(&mut s, at(t0, 60));
    assert!((bg(&f.draw.items, 1).r - 0.5).abs() < 0.02);
    let f = frame(&mut s, at(t0, 200));
    assert_eq!(bg(&f.draw.items, 1).r, 1.0);
}

#[test]
fn without_a_transition_rule_changes_are_immediate() {
    let css = ".b { background: #000000; width: 20px; height: 10px } .b.on { background: #ffffff }";
    let t0 = Instant::now();
    let mut s = scene_with(css, 200, 1.0, Some(classed(boxed(vec![]), &["b"])), None, None);
    frame(&mut s, t0);
    s.set_tree(tree(1, 2, Some(classed(boxed(vec![]), &["b", "on"]))));
    let f = frame(&mut s, at(t0, 10));
    assert_eq!(bg(&f.draw.items, 1).r, 1.0);
    assert!(f.next_frame.is_none());
}

#[test]
fn a_box_shadow_and_opacity_transition() {
    let css = ".b { background: #202020; width: 20px; height: 10px; opacity: 1; transition: opacity 100ms linear } .b:hover { opacity: 0.5 }";
    let t0 = Instant::now();
    let mut s = scene_with(css, 200, 1.0, Some(classed(boxed(vec![]), &["b"])), None, None);
    frame(&mut s, t0);
    let p = centre(&s, "b");
    mv(&mut s, p, t0);
    frame(&mut s, t0);
    let f = frame(&mut s, at(t0, 50));
    let op = f.draw.items.iter().find_map(|i| match i.cmd {
        DrawCmd::PushOpacity(v) => Some(v),
        _ => None,
    });
    assert!((op.unwrap() - 0.75).abs() < 0.02, "{op:?}");
}

#[test]
fn reordering_keyed_siblings_damages_both_places() {
    let t0 = Instant::now();
    let mk = |order: &[&str]| {
        boxed(
            order
                .iter()
                .map(|k| {
                    let mut e = classed(boxed(vec![]), &[k]);
                    e.key = Some(Arc::from(*k));
                    e
                })
                .collect(),
        )
    };
    let css = ".a { width: 20px; height: 10px; background: #ff0000 } .b { width: 20px; height: 10px; background: #00ff00 } .c { width: 20px; height: 10px; background: #0000ff }";
    let mut s = scene_with(css, 200, 1.0, Some(mk(&["a", "b", "c"])), None, None);
    frame(&mut s, t0);
    let a = node_by_class(&s, "a").rect;
    s.set_tree(tree(1, 2, Some(mk(&["b", "a", "c"]))));
    let f = frame(&mut s, at(t0, 1));
    let d = f.dirty.expect("two boxes swapped");
    // The swapped pair spans x 0..40 (a and b); c did not move.
    assert!(d.x0 <= 0 && d.x1 >= 40 && d.x1 < 60, "{d:?}");
    assert_eq!(node_by_class(&s, "a").rect.x, a.x + 20.0);
    // Same key means same node: the boxes did not restyle.
    assert_eq!(f.stats.restyled, 0);
}

#[test]
fn a_changed_bar_spec_relayouts() {
    let t0 = Instant::now();
    let mut s = scene_with("", 200, 1.0, Some(text("a")), None, None);
    frame(&mut s, t0);
    let mut b = bar();
    b.height = 32;
    s.set_bar(&b);
    assert!(!s.needs_frame(t0), "the same bar spec changes nothing");
    b.name = "other".into();
    s.set_bar(&b);
    assert!(s.needs_frame(t0), "a new bar spec needs a frame");
    frame(&mut s, at(t0, 1));
    assert!(s.nodes()[0].classes.iter().any(|c| c.as_ref() == "other"));
    // Geometry changes redraw everything.
    s.set_geometry(rsb_core::scene::BarGeometry { width_px: 250, height_px: 32, scale: 1.0 });
    let f = frame(&mut s, at(t0, 2));
    assert_eq!(f.dirty, Some(RectPx::new(0, 0, 250, 32)));
    s.set_geometry(rsb_core::scene::BarGeometry { width_px: 250, height_px: 40, scale: 1.25 });
    let f = frame(&mut s, at(t0, 3));
    assert_eq!(f.dirty, Some(RectPx::new(0, 0, 250, 40)));
}

#[test]
fn removing_a_slot_forgets_its_tree_and_a_returning_slot_starts_empty() {
    let t0 = Instant::now();
    let mut s = scene_with("", 200, 1.0, Some(text("a")), None, None);
    frame(&mut s, t0);
    s.set_slots(Default::default());
    let f = frame(&mut s, at(t0, 1));
    assert!(texts(&f.draw.items).is_empty());
    s.set_slots(rsb_core::scene::ZoneSlots { left: vec![slot(1, "l")], ..Default::default() });
    let f = frame(&mut s, at(t0, 2));
    assert!(texts(&f.draw.items).is_empty(), "the old tree is not remembered");
    s.set_tree(tree(1, 1, Some(text("again"))));
    let f = frame(&mut s, at(t0, 3));
    assert_eq!(texts(&f.draw.items), ["again"]);
}

#[test]
fn image_nodes_and_nested_clip_bounds_are_consistent() {
    // Every leaf item lies inside its bounds' own rect; groups share one node key.
    let css = ".c { overflow: hidden; width: 30px; height: 12px; border-radius: 4px; border: 1px solid #fff; background: #111111; box-shadow: 0 1px 3px #000000 }";
    let t0 = Instant::now();
    let kid: Element = text("a long text inside");
    let mut s = scene_with(css, 200, 1.0, Some(classed(boxed(vec![kid]), &["c"])), None, None);
    let f = frame(&mut s, t0);
    for i in &f.draw.items {
        if let DrawCmd::Text { rect, .. } | DrawCmd::Fill { rect, .. } | DrawCmd::Border { rect, .. } = &i.cmd {
            assert!(
                i.bounds.x <= rect.x
                    && i.bounds.y <= rect.y
                    && i.bounds.right() >= rect.right()
                    && i.bounds.bottom() >= rect.bottom()
            );
        }
    }
    let push = f.draw.items.iter().find(|i| matches!(i.cmd, DrawCmd::PushClip { .. })).unwrap();
    let pop = f.draw.items.iter().rev().find(|i| matches!(i.cmd, DrawCmd::PopClip)).unwrap();
    assert_eq!(push.node, pop.node);
    assert_eq!(push.bounds, pop.bounds);
}

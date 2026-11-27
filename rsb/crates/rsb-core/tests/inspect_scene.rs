//! What the scene keeps for the inspector: frame cost attributed to widget
//! instances, the root's custom properties, and the copy of a widget's tree.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use rsb_core::element::{Element, ElementKind, InputProps, InstanceId};
use rsb_core::hit::{PointerEvent, PointerKind};
use rsb_core::popup::PopupProps;
use rsb_core::scene::{FrameOut, Scene};

const BASE: &str = "bar { font-size: 10px }";

fn at(base: Instant, millis: u64) -> Instant {
    base + Duration::from_millis(millis)
}

fn spent(f: &FrameOut) -> u64 {
    u64::from(f.stats.style_us) + u64::from(f.stats.layout_us) + u64::from(f.stats.draw_us)
}

/// `n` text elements in a box: enough work that a frame takes measurable time.
fn wide(n: usize) -> Element {
    boxed((0..n).map(|i| text(&format!("item {i}"))).collect())
}

fn two_instances() -> (Scene, Instant) {
    let t0 = Instant::now();
    let s = scene_with(BASE, 300, 1.0, Some(wide(120)), Some(wide(120)), None);
    (s, t0)
}

#[test]
fn a_first_frame_with_two_new_trees_is_shared_between_them() {
    let (mut s, t0) = two_instances();
    let f = frame(&mut s, t0);
    let (a, b) = (s.instance_cost(InstanceId(1)), s.instance_cost(InstanceId(2)));
    assert_eq!((a.own, a.shared, b.own, b.shared), (0, 1, 0, 1));
    assert_eq!(a.shared_us, spent(&f) / 2);
    assert_eq!(b.shared_us, spent(&f) / 2);
    let t = s.frame_totals();
    assert_eq!((t.frames, t.no_tree_frames), (1, 0));
    assert_eq!(t.style_us + t.layout_us + t.paint_us, spent(&f));
}

#[test]
fn a_frame_after_one_instances_tree_is_that_instances_own() {
    let (mut s, t0) = two_instances();
    frame(&mut s, t0);
    s.set_tree(tree(2, 2, Some(wide(121))));
    let f = frame(&mut s, at(t0, 10));
    let (a, b) = (s.instance_cost(InstanceId(1)), s.instance_cost(InstanceId(2)));
    assert_eq!((a.own, a.own_us), (0, 0), "the other widget did not send a tree");
    assert_eq!((b.own, b.own_us, b.own_us_max as u64), (1, spent(&f), spent(&f)));
    assert_eq!(b.shared, 1, "the first frame is still counted as shared");
    assert_eq!(s.frame_totals().frames, 2);
    assert_eq!(s.frame_totals().no_tree_frames, 0);
}

#[test]
fn a_hover_only_frame_goes_to_no_tree_and_an_idle_one_adds_nothing() {
    let (mut s, t0) = two_instances();
    frame(&mut s, t0);
    let before = s.instance_cost(InstanceId(1));
    s.pointer(PointerEvent { kind: PointerKind::Move, x: 5.0, y: 5.0 }, at(t0, 5));
    let f = frame(&mut s, at(t0, 6));
    assert!(f.dirty.is_some() || s.frame_totals().frames == 2, "the move changed hover and was built");
    let t = s.frame_totals().clone();
    assert_eq!((t.frames, t.no_tree_frames), (2, 1), "no tree arrived: hover cost belongs to nobody");
    assert_eq!(s.instance_cost(InstanceId(1)), before);

    // Nothing changed: the early return adds no frame and no time.
    let idle = frame(&mut s, at(t0, 20));
    assert_eq!(idle.dirty, None);
    let u = s.frame_totals();
    assert_eq!(
        (u.frames, u.no_tree_frames, u.style_us + u.layout_us + u.paint_us),
        (2, 1, t.style_us + t.layout_us + t.paint_us)
    );
}

#[test]
fn the_longest_frame_and_the_last_60_seconds_are_kept() {
    let (mut s, t0) = two_instances();
    let first = frame(&mut s, t0);
    s.set_tree(tree(1, 2, Some(wide(300))));
    let big = frame(&mut s, at(t0, 30_000));
    let t = s.frame_totals().clone();
    assert_eq!(u64::from(t.frame_us_max), spent(&first).max(spent(&big)));
    assert_eq!(t.last_frame_at, Some(at(t0, 30_000)));
    assert_eq!(s.frame_us_last_60s(at(t0, 31_000)), spent(&first) + spent(&big), "both are inside the minute");
    // 70 s later the first frame, at second 0, has dropped out and the 30 s one has not.
    assert_eq!(s.frame_us_last_60s(at(t0, 70_000)), spent(&big));
    assert_eq!(s.frame_us_last_60s(at(t0, 100_000)), 0);
}

#[test]
fn a_tree_that_is_ignored_is_not_a_fresh_one() {
    let (mut s, t0) = two_instances();
    frame(&mut s, t0);
    // An older version and a tree for an instance on another bar are dropped.
    s.set_tree(tree(1, 0, Some(wide(5))));
    s.set_tree(tree(9, 5, Some(wide(5))));
    assert!(!s.pending());
    frame(&mut s, at(t0, 5));
    assert_eq!(s.frame_totals().frames, 1);
}

#[test]
fn pending_says_a_newer_tree_or_style_waits_for_the_next_frame() {
    let (mut s, t0) = two_instances();
    assert!(s.pending(), "nothing is drawn yet");
    frame(&mut s, t0);
    assert!(!s.pending());
    s.set_tree(tree(1, 2, Some(wide(3))));
    assert!(s.pending());
    frame(&mut s, at(t0, 5));
    assert!(!s.pending());
    s.set_stylesheet(sheet("bar { font-size: 12px }"));
    assert!(s.pending());
    frame(&mut s, at(t0, 10));
    assert!(!s.pending());
}

#[test]
fn the_root_keeps_its_custom_properties_and_the_scheme() {
    let t0 = Instant::now();
    let mut s = scene_with(
        ":root { --accent: #e5a50a; --fg: rgb(230, 230, 230); --note: hello }",
        300,
        1.0,
        Some(text("x")),
        None,
        None,
    );
    frame(&mut s, t0);
    assert_eq!(s.root_var("accent").as_deref(), Some("#e5a50a"));
    assert_eq!(s.root_var("--fg").as_deref(), Some("#e6e6e6"), "a colour is written as hex");
    assert_eq!(s.root_var("note").as_deref(), Some("hello"));
    assert_eq!(s.root_var("missing"), None);
    assert!(s.dark(), "the helper makes dark scenes");
    let root = s.root_node(true).expect("a root");
    assert_eq!(root.kind, ElementKind::Bar);
    assert!(root.style.is_some());
    assert_eq!(root.rect[2], 300.0);
}

fn nested() -> Element {
    let mut password = Element::new(ElementKind::Input);
    password.input =
        Some(Box::new(InputProps { value: Arc::from("hunter2"), password: true, ..InputProps::default() }));
    classed(
        boxed(vec![
            classed(text("first"), &["a"]),
            click(classed(boxed(vec![classed(text("deep"), &["d"]), text("deeper")]), &["b"]), 7),
            password,
            input_el("typed words"),
        ]),
        &["root"],
    )
}

#[test]
fn the_copy_is_breadth_first_with_parents_before_children() {
    let t0 = Instant::now();
    let mut s = scene_with(BASE, 300, 1.0, Some(nested()), None, None);
    frame(&mut s, t0);
    let c = s.nodes_of(InstanceId(1), 100, 64, false);
    assert_eq!(c.total, 7);
    assert_eq!(c.nodes.len(), 7);
    assert!(!c.limit_hit);
    let kinds: Vec<(&str, u8)> = c.nodes.iter().map(|n| (n.kind.css_name(), n.depth)).collect();
    assert_eq!(
        kinds,
        vec![("box", 1), ("text", 2), ("box", 2), ("input", 2), ("input", 2), ("text", 3), ("text", 3)],
        "the root, its four children, then the grandchildren"
    );
    assert_eq!(c.nodes[0].parent, None);
    for (i, n) in c.nodes.iter().enumerate().skip(1) {
        assert!(n.parent.is_some_and(|p| (p as usize) < i), "node {i} comes after its parent");
    }
    let by_class = |class: &str| c.nodes.iter().position(|n| n.classes.iter().any(|x| x.as_ref() == class)).unwrap();
    let b = by_class("b");
    assert_eq!(c.nodes[b].handler_names(), vec!["click"]);
    assert_eq!(c.nodes[by_class("d")].parent, Some(b as u32));
    assert_eq!(c.nodes[by_class("a")].text.as_deref(), Some("first"));
    assert_eq!(c.nodes[by_class("a")].text_len, 5);
    assert!(c.base_style.is_some());
}

#[test]
fn rects_are_logical_px_and_hover_shows_as_a_state() {
    let t0 = Instant::now();
    let mut s = scene_with(BASE, 600, 2.0, Some(classed(wide(1), &["root"])), None, None);
    frame(&mut s, t0);
    let physical = node_by_class(&s, "root").rect;
    let c = s.nodes_of(InstanceId(1), 10, 64, false);
    assert_eq!(c.nodes[0].rect, [physical.x / 2.0, physical.y / 2.0, physical.w / 2.0, physical.h / 2.0]);
    assert!(c.nodes[0].state_names().is_empty());
    let (x, y) = (physical.x + physical.w / 2.0, physical.y + physical.h / 2.0);
    s.pointer(PointerEvent { kind: PointerKind::Move, x, y }, at(t0, 5));
    frame(&mut s, at(t0, 6));
    let c = s.nodes_of(InstanceId(1), 10, 64, false);
    assert_eq!(c.nodes[0].state_names(), vec!["hover"]);
}

#[test]
fn a_password_value_is_never_copied_and_a_plain_input_value_is() {
    let t0 = Instant::now();
    let mut s = scene_with(BASE, 300, 1.0, Some(nested()), None, None);
    frame(&mut s, t0);
    let c = s.nodes_of(InstanceId(1), 100, 64, false);
    let inputs: Vec<_> = c.nodes.iter().filter(|n| n.kind == ElementKind::Input).collect();
    assert_eq!(inputs.len(), 2);
    let secret = inputs.iter().find(|n| n.password).expect("the password input");
    assert_eq!((secret.text.as_deref(), secret.text_len), (None, 7));
    let plain = inputs.iter().find(|n| !n.password).expect("the plain input");
    assert_eq!((plain.text.as_deref(), plain.text_len), (Some("typed words"), 11));
    assert!(!format!("{c:?}").contains("hunter2"), "the copy holds no trace of the value");
}

#[test]
fn the_node_limit_and_the_depth_limit_are_counted_not_hidden() {
    let t0 = Instant::now();
    let mut s = scene_with(BASE, 300, 1.0, Some(wide(2000)), None, None);
    frame(&mut s, t0);
    let c = s.nodes_of(InstanceId(1), 600, 64, false);
    assert_eq!((c.nodes.len(), c.total, c.limit_hit), (600, 2001, true), "the box and its 2000 children");
    assert_eq!(c.nodes[0].depth, 1);

    let mut s = scene_with(BASE, 300, 1.0, Some(nested()), None, None);
    frame(&mut s, t0);
    let c = s.nodes_of(InstanceId(1), 100, 2, false);
    assert_eq!((c.nodes.len(), c.total, c.limit_hit), (5, 7, false), "two levels copied, the rest counted");
    assert!(c.nodes.iter().all(|n| n.depth <= 2));
}

#[test]
fn a_widget_that_is_not_on_the_bar_has_no_nodes() {
    let t0 = Instant::now();
    let mut s = scene_with(BASE, 300, 1.0, Some(text("x")), None, None);
    frame(&mut s, t0);
    let c = s.nodes_of(InstanceId(42), 10, 64, false);
    assert_eq!((c.nodes.len(), c.total), (0, 0));
}

#[test]
fn style_is_kept_only_when_asked_for() {
    let t0 = Instant::now();
    let mut s = scene_with(
        "bar { font-size: 10px } .a { gap: 4px }",
        300,
        1.0,
        Some(classed(boxed(vec![]), &["a"])),
        None,
        None,
    );
    frame(&mut s, t0);
    assert!(s.nodes_of(InstanceId(1), 10, 64, false).nodes[0].style.is_none());
    let c = s.nodes_of(InstanceId(1), 10, 64, true);
    assert_eq!(c.nodes[0].style.as_ref().map(|st| st.gap), Some(4.0));
}

#[test]
fn a_declared_popup_is_listed_with_its_name_and_size_and_is_not_a_node() {
    let props = PopupProps { names: vec![Arc::from("calendar"), Arc::from("cal")], ..PopupProps::default() };
    let (mut s, _) = bar_with_popup(BASE, props, vec![text("a"), text("b"), text("c")]);
    frame(&mut s, Instant::now());
    let c = s.nodes_of(InstanceId(1), 100, 64, false);
    assert_eq!(c.nodes.len(), 2, "the anchor box and its parent box; the popup is no node");
    assert_eq!(c.popups.len(), 1);
    let p = &c.popups[0];
    assert_eq!((p.name.as_str(), p.kind, p.open, p.nodes), ("calendar", ElementKind::Popup, false, 4));
    let anchor = p.anchor.expect("it hangs from a copied node") as usize;
    assert!(c.nodes[anchor].classes.iter().any(|x| x.as_ref() == "anchor"));
}

#[test]
fn an_open_popup_scene_copies_its_own_content() {
    let props = PopupProps { names: vec![Arc::from("calendar")], open: true, open_seq: 1, ..PopupProps::default() };
    let (bar, tr) = bar_with_popup(BASE, props.clone(), vec![text("a"), text("b")]);
    let decl = decls(&bar).remove(0);
    let mut pop = Scene::new_popup(
        spec(rsb_core::popup::Side::Below),
        rsb_core::scene::BarGeometry { width_px: 400, height_px: 300, scale: 1.0 },
        env(true),
    );
    pop.set_stylesheet(sheet(BASE));
    pop.set_popup_tree(tr, decl.path.clone());
    pop.set_open(true, Instant::now());
    frame(&mut pop, Instant::now());
    let c = pop.popup_nodes(50, 64, false);
    assert_eq!(c.total, 3, "the popup root and its two texts");
    assert_eq!(c.nodes[0].kind, ElementKind::Popup);
    assert_eq!(bar.popup_nodes(50, 64, false).nodes.len(), 0, "a bar scene has no popup content");
}

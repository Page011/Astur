mod common;

use std::sync::Arc;
use std::time::Instant;

use common::*;
use rsb_core::draw::DrawCmd;
use rsb_core::element::ElementKind;
use rsb_core::geom::PointF;
use rsb_core::hit::{PointerEvent, PointerKind};
use rsb_core::layout::FixedMeasure;
use rsb_core::popup::{PopupProps, Side};

const CSS: &str = "bar { font-size: 10px } .root { padding: 4px } .anchor { width: 40px; height: 20px; background: #333 } \
                   popup { display: flex; flex-direction: column; width: 100px; background: #111; \
                   box-shadow: 0 4px 10px #000000 } popup:open { opacity: 1 } popup { opacity: 0 } \
                   .row { height: 10px; background: #444 }";

fn rows(n: usize) -> Vec<rsb_core::element::Element> {
    (0..n).map(|_| classed(boxed(vec![]), &["row"])).collect()
}

fn names(list: &[&str]) -> PopupProps {
    PopupProps { names: list.iter().map(|n| Arc::from(*n)).collect(), ..open_props(1) }
}

#[test]
fn a_bar_reports_its_popups_with_the_anchor_box_and_no_node() {
    let (s, _) = bar_with_popup(CSS, open_props(1), rows(2));
    let d = decls(&s);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].kind, ElementKind::Popup);
    assert!(d[0].open);
    let anchor = node_by_class(&s, "anchor");
    assert_eq!(d[0].anchor_rect, anchor.rect);
    assert_eq!(d[0].anchor, anchor.key);
    assert_eq!(d[0].path.as_ref(), &[0, 0][..]);
    assert!(d[0].anchor_flat.bottom.is_some(), "a plain box has a flat bottom edge");
    // The popup and its rows are never laid out in the bar.
    assert!(s.nodes().iter().all(|n| n.kind != ElementKind::Popup && !n.classes.iter().any(|c| &**c == "row")));
}

#[test]
fn a_dismissed_open_seq_never_reopens_but_a_newer_one_does() {
    let (mut s, tree) = bar_with_popup(CSS, open_props(3), rows(1));
    let key = decls(&s)[0].key;
    s.dismiss_popup(key, 3);
    assert!(!decls(&s)[0].open, "closed at once");
    // The same tree arrives again (it was in flight): still closed.
    s.set_tree(Arc::new(rsb_core::element::WidgetTree { version: 2, ..(*tree).clone() }));
    frame(&mut s, Instant::now());
    assert!(!decls(&s)[0].open);
    // A tree with a newer open_seq opens it.
    let mut root = tree.root.clone().unwrap();
    root.children[0].children[0].popup.as_mut().unwrap().open_seq = 4;
    s.set_tree(Arc::new(rsb_core::element::WidgetTree { version: 3, root: Some(root), ..(*tree).clone() }));
    frame(&mut s, Instant::now());
    assert!(decls(&s)[0].open);
}

#[test]
fn an_open_popup_with_seq_zero_is_open_until_dismissed() {
    let (mut s, _) = bar_with_popup(CSS, PopupProps { open: true, ..PopupProps::default() }, rows(1));
    let d = decls(&s);
    assert!(d[0].open);
    s.dismiss_popup(d[0].key, 0);
    assert!(!decls(&s)[0].open);
}

#[test]
fn the_first_popup_with_a_name_keeps_it_and_the_rest_warn_once() {
    let a = popup_el(names(&["menu", "extra"]), ElementKind::Popup, vec![]);
    let b = popup_el(names(&["menu", "other"]), ElementKind::Popup, vec![]);
    let root =
        classed(boxed(vec![classed(boxed(vec![a]), &["anchor"]), classed(boxed(vec![b]), &["anchor2"])]), &["root"]);
    let mut s = scene_with(CSS, 400, 1.0, Some(root), None, None);
    frame(&mut s, Instant::now());
    let d = decls(&s);
    assert_eq!(d.len(), 2);
    let n0: Vec<&str> = d[0].props.names.iter().map(|n| &**n).collect();
    let n1: Vec<&str> = d[1].props.names.iter().map(|n| &**n).collect();
    assert_eq!(n0, ["menu", "extra"]);
    assert_eq!(n1, ["other"]);
    let warnings = diag_text(&s.take_diagnostics().into_iter().map(|d| d.diagnostic).collect::<Vec<_>>());
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("'menu'"), "{warnings:?}");
    frame(&mut s, Instant::now());
    assert!(s.take_diagnostics().is_empty(), "reported once");
}

#[test]
fn the_anchor_rect_is_where_the_anchor_will_be_not_where_it_is() {
    let css = format!("{CSS} .anchor {{ transition: width 200ms linear }} .anchor:open {{ width: 80px }}");
    let (mut s, _) = bar_with_popup(&css, open_props(1), rows(1));
    let t = Instant::now();
    let d = decls(&s);
    assert_eq!(d[0].anchor_rect.w, 40.0);
    s.set_anchor_open(d[0].key, true, t);
    frame(&mut s, t);
    frame(&mut s, t + ms(100));
    let now = node_by_class(&s, "anchor").rect.w;
    assert!(now > 40.0 && now < 80.0, "drawn width part way: {now}");
    assert_eq!(decls(&s)[0].anchor_rect.w, 80.0, "reported at the end of the change");
    s.set_anchor_open(d[0].key, false, t + ms(100));
    frame(&mut s, t + ms(100));
    assert_eq!(decls(&s)[0].anchor_rect.w, 40.0);
}

#[test]
fn a_popup_inside_a_popup_is_reported_by_the_popup_scene() {
    let inner = popup_el(open_props(1), ElementKind::Popup, rows(1));
    let (bar, _) = bar_with_popup(CSS, open_props(1), vec![classed(boxed(vec![inner]), &["holder"])]);
    let d = decls(&bar);
    assert_eq!(d.len(), 1, "the bar sees only the outer popup");
    let mut p = popup_scene(CSS, &d[0], Side::Below, 200, 200);
    p.set_open(true, Instant::now());
    frame(&mut p, Instant::now());
    let nested = p.popups();
    assert_eq!(nested.len(), 1);
    assert_eq!(nested[0].path.as_ref(), &[0, 0, 0, 0][..]);
    assert_eq!(nested[0].anchor, node_by_class(&p, "holder").key);
}

#[test]
fn a_popup_scene_lays_the_root_out_at_its_natural_size_and_offset() {
    let (bar, _) = bar_with_popup(CSS, open_props(1), rows(3));
    let d = decls(&bar);
    let mut p = popup_scene(CSS, &d[0], Side::Below, 10, 10);
    let size = p.natural_size(&mut FixedMeasure);
    // 100 x 30 root, shadow 0 4px 10px: blur extent ceil(15) + 1 = 16 around the
    // offset rect (0, 4).
    assert_eq!((size.w, size.h), (132.0, 62.0));
    let off = p.root_offset();
    assert_eq!((off.x, off.y), (16.0, 12.0));
    p.set_open(true, Instant::now());
    let t = Instant::now();
    frame(&mut p, t);
    let root = p.nodes().into_iter().next().unwrap();
    assert_eq!((root.rect.x, root.rect.y, root.rect.w, root.rect.h), (16.0, 12.0, 100.0, 30.0));
    assert_eq!(root.kind, ElementKind::Popup);
    assert_eq!(root.key, d[0].key, "the popup scene and the bar agree on the node key");
    let outline = p.root_outline();
    assert!(outline.len() >= 4);
    assert!(outline.iter().all(|pt| pt.x >= 16.0 && pt.x <= 116.0 && pt.y >= 12.0 && pt.y <= 42.0));
}

#[test]
fn the_first_frame_is_closed_then_the_open_transitions_run() {
    let css = format!("{CSS} popup {{ transition: opacity 100ms linear }}");
    let (bar, _) = bar_with_popup(&css, open_props(1), rows(1));
    let d = decls(&bar);
    let mut p = popup_scene(&css, &d[0], Side::Below, 200, 100);
    let t = Instant::now();
    frame(&mut p, t);
    let opacity = |p: &rsb_core::scene::Scene| p.nodes()[0].style.opacity;
    assert_eq!(opacity(&p), 0.0, "closed first");
    p.set_open(true, t);
    frame(&mut p, t);
    frame(&mut p, t + ms(50));
    let half = opacity(&p);
    assert!(half > 0.3 && half < 0.7, "{half}");
    frame(&mut p, t + ms(150));
    assert_eq!(opacity(&p), 1.0);
    assert!(p.is_settled());
    // Closing plays the transition back.
    p.set_open(false, t + ms(200));
    frame(&mut p, t + ms(200));
    frame(&mut p, t + ms(250));
    let back = opacity(&p);
    assert!(back > 0.3 && back < 0.7, "{back}");
}

#[test]
fn a_closing_popup_ignores_new_trees_and_input_and_a_reopen_takes_the_new_one() {
    let (bar, tree) = bar_with_popup(CSS, open_props(1), rows(2));
    let d = decls(&bar);
    let mut p = popup_scene(CSS, &d[0], Side::Below, 200, 100);
    let t = Instant::now();
    p.set_open(true, t);
    frame(&mut p, t);
    assert_eq!(p.nodes().len(), 3);
    p.set_open(false, t + ms(10));
    // The script has dropped the content: the new tree holds the popup with no children.
    let mut empty = tree.root.clone().unwrap();
    empty.children[0].children[0].children.clear();
    let bare = Arc::new(rsb_core::element::WidgetTree { version: 2, root: Some(empty), ..(*tree).clone() });
    p.set_popup_tree(bare.clone(), d[0].path.clone());
    frame(&mut p, t + ms(20));
    assert_eq!(p.nodes().len(), 3, "keeps drawing what it had");
    let r = p.pointer(PointerEvent { kind: PointerKind::Down(rsb_core::hit::Button::Left), x: 20.0, y: 5.0 }, t);
    assert!(r.calls.is_empty() && !r.missed);
    // Told to open again: the new tree is taken.
    p.set_open(true, t + ms(30));
    p.set_popup_tree(bare, d[0].path.clone());
    frame(&mut p, t + ms(40));
    assert_eq!(p.nodes().len(), 1);
}

#[test]
fn an_overlay_is_the_bounds_of_its_content_and_its_background_becomes_the_dim() {
    let css = "popup { width: 100%; height: 100%; background: rgba(0,0,0,0.5); display: flex; \
               justify-content: center; align-items: center } .btn { width: 40px; height: 30px; background: #fff }";
    let btn = classed(boxed(vec![]), &["btn"]);
    let (bar, _) = bar_with_popup(css, open_props(1), vec![btn.clone(), btn]);
    let d = decls(&bar);
    let mut sp = spec(Side::Screen);
    sp.kind = ElementKind::Popup;
    let mut p = rsb_core::scene::Scene::new_popup(
        sp,
        rsb_core::scene::BarGeometry { width_px: 400, height_px: 300, scale: 1.0 },
        env(true),
    );
    p.set_stylesheet(sheet(css));
    p.set_popup_tree(d[0].tree.clone(), d[0].path.clone());
    let size = p.natural_size(&mut FixedMeasure);
    assert_eq!((size.w, size.h), (80.0, 30.0), "the two buttons");
    let off = p.root_offset();
    assert_eq!((off.x, off.y), (-160.0, -135.0), "the monitor-sized root starts left of and above the surface");
    let t = Instant::now();
    p.set_open(true, t);
    p.set_geometry(rsb_core::scene::BarGeometry { width_px: 80, height_px: 30, scale: 1.0 });
    let out = frame(&mut p, t);
    let dim = p.overlay_dim().expect("an overlay has a dim");
    assert!((dim.a - 0.5).abs() < 0.01);
    // The root's background is not drawn into the content.
    let fills: Vec<_> = out
        .draw
        .items
        .iter()
        .filter_map(|i| match &i.cmd {
            DrawCmd::Fill { rect, fill, .. } => {
                Some((rect.w, matches!(fill, rsb_core::draw::Fill::Solid(c) if c.a > 0.9)))
            }
            _ => None,
        })
        .collect();
    assert!(fills.iter().all(|(w, opaque)| !(*w == 400.0 && *opaque)), "{fills:?}");
    assert_eq!(fills.iter().filter(|(w, o)| *w == 40.0 && *o).count(), 2);
    assert!(bar.popups()[0].open);
    assert!(rsb_core::scene::Scene::overlay_dim(&bar).is_none());
}

#[test]
fn a_press_on_a_shadow_pixel_is_missed_and_one_on_the_root_is_not() {
    let (bar, _) = bar_with_popup(CSS, open_props(1), rows(2));
    let d = decls(&bar);
    let mut p = popup_scene(CSS, &d[0], Side::Below, 132, 60);
    let t = Instant::now();
    p.set_open(true, t);
    frame(&mut p, t);
    frame(&mut p, t + ms(500));
    let down = |p: &mut rsb_core::scene::Scene, x: f32, y: f32| {
        p.pointer(PointerEvent { kind: PointerKind::Down(rsb_core::hit::Button::Left), x, y }, t)
    };
    assert!(down(&mut p, 2.0, 2.0).missed, "shadow margin");
    assert!(!down(&mut p, 60.0, 20.0).missed, "root");
    let _ = PointF::default();
}

fn root_of(p: &rsb_core::scene::Scene) -> rsb_core::scene::NodeInfo {
    p.nodes().into_iter().next().unwrap()
}

fn open_popup(css: &str, content: Vec<rsb_core::element::Element>, side: Side) -> (rsb_core::scene::Scene, Instant) {
    let (bar, _) = bar_with_popup(css, open_props(1), content);
    let mut p = popup_scene(css, &decls(&bar)[0], side, 200, 100);
    let t = Instant::now();
    p.set_open(true, t);
    frame(&mut p, t);
    (p, t)
}

#[test]
fn the_virtual_bar_and_widget_reach_the_content_through_variables_inheritance_and_selectors() {
    let css = ":root { --tone: #ff0000 } bar { font-size: 11px } widget.l popup { background: var(--tone) } \
               popup.below { color: #00ff00 } popup.blur { color: #0000ff } bar popup .t { height: 8px }";
    let (p, _) = open_popup(css, vec![classed(text("hi"), &["t"])], Side::Below);
    let root = root_of(&p);
    assert_eq!(
        root.style.background,
        rsb_core::style::Background::Color(rsb_core::color::Color::rgba8(255, 0, 0, 255))
    );
    assert!(root.classes.iter().any(|c| &**c == "below"));
    assert_eq!(root.style.color, rsb_core::color::Color::rgba8(0, 255, 0, 255));
    let t = node_by_class(&p, "t");
    assert_eq!(t.style.font_size, 11.0, "inherited from the bar");
    assert_eq!(t.rect.h, 8.0, "a descendant selector through the virtual bar reaches the content");
    // The virtual ancestors are not nodes of the popup.
    assert!(p.nodes().iter().all(|n| n.kind != ElementKind::Bar && n.kind != ElementKind::Widget));
    // Another side and the system backdrop change the classes.
    let (mut p, t0) = open_popup(css, vec![], Side::Above);
    p.set_popup_surface(Side::Above, true, None);
    frame(&mut p, t0);
    let root = root_of(&p);
    assert!(root.classes.iter().any(|c| &**c == "above"));
    assert!(root.classes.iter().any(|c| &**c == "blur"));
    assert_eq!(root.style.color, rsb_core::color::Color::rgba8(0, 0, 255, 255));
}

#[test]
fn open_matches_the_popup_root_while_set_open_and_an_attached_popup_takes_the_width_given() {
    let css = "popup { width: 100px; height: 20px; color: #ff0000 } popup:open { color: #00ff00 }";
    let (bar, _) = bar_with_popup(css, open_props(1), vec![]);
    let mut p = popup_scene(css, &decls(&bar)[0], Side::Below, 200, 100);
    let t = Instant::now();
    frame(&mut p, t);
    assert_eq!(root_of(&p).style.color, rsb_core::color::Color::rgba8(255, 0, 0, 255));
    p.set_popup_surface(Side::Below, false, Some(160.0));
    p.set_open(true, t);
    frame(&mut p, t);
    let root = root_of(&p);
    assert_eq!(root.style.color, rsb_core::color::Color::rgba8(0, 255, 0, 255));
    assert_eq!(root.rect.w, 160.0, "the width the manager gave replaces the CSS width");
    assert_eq!(p.natural_size(&mut FixedMeasure).w, 160.0);
}

#[test]
fn root_outline_follows_round_bevelled_and_clipped_roots() {
    let size = "popup { width: 100px; height: 40px; background: #123456 }";
    let (p, _) = open_popup(size, vec![], Side::Below);
    let o = p.root_outline();
    assert!(o.len() == 4 || o.len() == 5, "a square box: {}", o.len());
    let (p, _) = open_popup(&format!("{size} popup {{ border-radius: 10px }}"), vec![], Side::Below);
    let o = p.root_outline();
    assert!(o.len() > 4 && o.len() <= 4 * 9 + 4, "rounded corners flattened: {}", o.len());
    assert!(o.iter().all(|pt| pt.x >= 0.0 && pt.x <= 100.0 && pt.y >= 0.0 && pt.y <= 40.0));
    let (p, _) =
        open_popup(&format!("{size} popup {{ border-radius: 10px; corner-shape: bevel }}"), vec![], Side::Below);
    assert_eq!(p.root_outline().len(), 8, "a cut corner is one straight line");
    let (p, _) =
        open_popup(&format!("{size} popup {{ clip-path: polygon(0 0, 100% 0, 50% 100%) }}"), vec![], Side::Below);
    let tri = p.root_outline();
    assert_eq!(tri.len(), 3);
    assert!(tri.iter().any(|pt| pt.x == 50.0 && pt.y == 40.0));
    // Nothing is drawn while the root has no size.
    let (p, _) = open_popup("popup { width: 100px; height: 0px }", vec![], Side::Below);
    assert!(p.root_outline().is_empty());
}

#[test]
fn overlay_dim_follows_the_roots_transitions_and_opacity() {
    let css = "popup { width: 100%; height: 100%; background-color: rgba(0, 0, 0, 0.2); transition: background-color 100ms linear } \
               popup:open { background-color: rgba(0, 0, 0, 0.6) }";
    let (bar, _) = bar_with_popup(css, open_props(1), vec![]);
    let mut p = popup_scene(css, &decls(&bar)[0], Side::Screen, 400, 300);
    let t = Instant::now();
    frame(&mut p, t);
    assert!((p.overlay_dim().unwrap().a - 0.2).abs() < 0.01);
    p.set_open(true, t);
    frame(&mut p, t);
    frame(&mut p, t + ms(50));
    let half = p.overlay_dim().unwrap().a;
    assert!((half - 0.4).abs() < 0.02, "{half}");
    frame(&mut p, t + ms(200));
    assert!((p.overlay_dim().unwrap().a - 0.6).abs() < 0.01);
    // The root's opacity multiplies in.
    let css2 = "popup { width: 100%; height: 100%; background-color: rgba(0, 0, 0, 0.6); opacity: 0.5 }";
    let (bar, _) = bar_with_popup(css2, open_props(1), vec![]);
    let mut p = popup_scene(css2, &decls(&bar)[0], Side::Screen, 400, 300);
    frame(&mut p, t);
    assert!((p.overlay_dim().unwrap().a - 0.3).abs() < 0.01);
}

#[test]
fn the_hit_fill_covers_the_root_border_box_only_and_the_draw_list_is_the_surface_size() {
    let (mut p, t) = open_popup(CSS, rows(1), Side::Below);
    p.set_geometry(rsb_core::scene::BarGeometry { width_px: 132, height_px: 62, scale: 1.0 });
    let out = frame(&mut p, t);
    assert_eq!((out.draw.width, out.draw.height), (132, 62));
    let first = &out.draw.items[0];
    match &first.cmd {
        DrawCmd::Fill { rect, fill: rsb_core::draw::Fill::Solid(c), .. } => {
            assert_eq!((rect.x, rect.y, rect.w, rect.h), (16.0, 12.0, 100.0, 10.0));
            assert!(c.a > 0.0 && c.a < 0.01);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn state_of_the_popup_survives_a_reopen_while_closing_and_starts_fresh_in_a_new_scene() {
    let css = format!("{CSS} input {{ width: 80px; height: 14px }}");
    let mut input = rsb_core::element::Element::new(ElementKind::Input);
    input.input = Some(Box::new(rsb_core::element::InputProps::default()));
    let (bar, _) = bar_with_popup(&css, open_props(1), vec![input]);
    let d = decls(&bar);
    let mut p = popup_scene(&css, &d[0], Side::Below, 200, 100);
    let t = Instant::now();
    p.set_open(true, t);
    p.focus_first(true, t);
    p.set_window_focus(true, t);
    p.text_input(rsb_core::key::TextInput::Chars("kept".into()), t);
    p.set_open(false, t + ms(10));
    p.set_open(true, t + ms(20));
    let out = frame(&mut p, t + ms(30));
    assert_eq!(texts(&out.draw.items), ["kept"]);
    let mut fresh = popup_scene(&css, &d[0], Side::Below, 200, 100);
    fresh.set_open(true, t);
    let out = frame(&mut fresh, t);
    assert!(texts(&out.draw.items).is_empty());
}

#[test]
fn the_gesture_that_opened_a_popup_travels_with_its_declaration() {
    let (s, _) = bar_with_popup(CSS, PopupProps { gesture: 42, ..open_props(2) }, rows(1));
    let d = decls(&s);
    assert_eq!((d[0].props.gesture, d[0].props.open_seq), (42, 2));
}

#[test]
fn a_popup_is_not_declared_once_its_anchor_or_widget_is_gone() {
    let (mut s, tree) = bar_with_popup(CSS, open_props(1), rows(1));
    assert_eq!(decls(&s).len(), 1);
    // The anchor hides itself.
    let mut root = tree.root.clone().unwrap();
    root.children[0].style = vec![("display".into(), "none".into())];
    s.set_tree(Arc::new(rsb_core::element::WidgetTree { version: 2, root: Some(root), ..(*tree).clone() }));
    frame(&mut s, Instant::now());
    assert!(decls(&s).is_empty());
    // It comes back, then the widget stops: a plate replaces its tree.
    s.set_tree(Arc::new(rsb_core::element::WidgetTree { version: 3, ..(*tree).clone() }));
    frame(&mut s, Instant::now());
    assert_eq!(decls(&s).len(), 1);
    s.set_widget_plate(
        rsb_core::element::InstanceId(1),
        Some(
            rsb_core::diag::Plate::from_diagnostics(&[rsb_core::diag::Diagnostic::error(
                rsb_core::diag::DiagKind::Stopped,
                None,
                "stopped",
            )])
            .unwrap(),
        ),
    );
    frame(&mut s, Instant::now());
    assert!(decls(&s).is_empty());
}

#[test]
fn the_flat_edge_for_an_attached_panel_comes_from_the_target_layout_on_whole_pixels() {
    let css = "bar { font-size: 10px } .root { padding: 4px } popup { width: 50px } .anchor { width: 60px; height: 20px; \
               background: #333; transition: width 200ms linear; \
               clip-path: polygon(10px 0, calc(100% - 10px) 0, 100% 50%, calc(100% - 10px) 100%, 10px 100%, 0 50%) } \
               .anchor:open { width: 100px }";
    let (mut s, _) = bar_with_popup(css, open_props(1), vec![]);
    let d = decls(&s);
    assert_eq!(d[0].anchor_flat.bottom, Some((14.0, 54.0)));
    assert_eq!(d[0].anchor_flat.top, Some((14.0, 54.0)));
    let t = Instant::now();
    s.set_anchor_open(d[0].key, true, t);
    frame(&mut s, t);
    frame(&mut s, t + ms(60));
    let drawn = node_by_class(&s, "anchor").rect.w;
    assert!(drawn > 60.0 && drawn < 100.0);
    assert_eq!(decls(&s)[0].anchor_flat.bottom, Some((14.0, 94.0)), "the width it is going to have");
    // At a fractional scale every vertex is on a whole device pixel.
    let mut b = bar();
    b.height = 32;
    let mut s = rsb_core::scene::Scene::new(
        &b,
        rsb_core::scene::BarGeometry { width_px: 500, height_px: 40, scale: 1.25 },
        env(true),
    );
    s.set_stylesheet(sheet(css));
    s.set_slots(rsb_core::scene::ZoneSlots { left: vec![slot(1, "l")], ..Default::default() });
    let popup = popup_el(open_props(1), ElementKind::Popup, vec![]);
    s.set_tree(tree(1, 1, Some(classed(boxed(vec![classed(boxed(vec![popup]), &["anchor"])]), &["root"]))));
    frame(&mut s, t);
    let (x0, x1) = decls(&s)[0].anchor_flat.bottom.unwrap();
    assert_eq!((x0.fract(), x1.fract()), (0.0, 0.0), "{x0} {x1}");
    assert!(x1 > x0);
}

/// Cost of a list popup: run with `cargo test --release -- --ignored --nocapture`.
#[test]
#[ignore = "timing, for release builds"]
fn a_popup_of_200_rows_opens_and_updates_quickly() {
    let t = Instant::now();
    let (bar, _) = bar_with_popup(CSS, open_props(1), rows(200));
    let d = decls(&bar);
    let mut p = popup_scene(CSS, &d[0], Side::Below, 200, 400);
    let start = Instant::now();
    let size = p.natural_size(&mut FixedMeasure);
    let natural = start.elapsed();
    p.set_open(true, t);
    let start = Instant::now();
    frame(&mut p, t);
    let first = start.elapsed();
    // The same tree again with one more row: the style cache keeps the rest.
    let mut root = d[0].tree.root.clone().unwrap();
    root.children[0].children[0].children[0].children.push(classed(boxed(vec![]), &["row"]));
    p.set_popup_tree(
        Arc::new(rsb_core::element::WidgetTree { version: 2, root: Some(root), ..(*d[0].tree).clone() }),
        d[0].path.clone(),
    );
    let start = Instant::now();
    frame(&mut p, t + ms(50));
    let update = start.elapsed();
    eprintln!("200 rows: natural size {natural:?}, first frame {first:?}, a tree update {update:?} ({size:?})");
}

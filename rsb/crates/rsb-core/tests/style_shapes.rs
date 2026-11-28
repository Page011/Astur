//! Shapes, the pinned centre, empty widgets and the pointer rules: bevelled
//! corners, `clip-path` on whole device pixels, `anchor_flat`, `center_on`,
//! structural pseudo-classes, `pointer-events`, SVG icons and a hover
//! reveal.

mod common;

use std::sync::Arc;
use std::time::Instant;

use common::*;
use rsb_core::config::{WidgetRef, load_config};
use rsb_core::draw::{DrawCmd, DrawItem};
use rsb_core::element::{Element, ElementKind, HandlerId, IconRef};
use rsb_core::geom::{PathEl, PointF};
use rsb_core::hit::{Button, PointerEvent, PointerKind};
use rsb_core::scene::{BarGeometry, Scene, ZoneSlots};
use rsb_core::style::Cursor;

fn at(t0: Instant, n: u64) -> Instant {
    t0 + ms(n)
}

fn points(item: &DrawItem) -> Vec<PointF> {
    let path = match &item.cmd {
        DrawCmd::PushClipPath { path, .. } | DrawCmd::FillPath { path, .. } => path,
        other => panic!("not a path: {other:?}"),
    };
    path.els
        .iter()
        .filter_map(|e| match e {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => Some(*p),
            _ => None,
        })
        .collect()
}

fn clip_of(items: &[DrawItem], key: rsb_core::element::NodeKey) -> Vec<PointF> {
    let i = items
        .iter()
        .find(|i| i.node == key && matches!(i.cmd, DrawCmd::PushClipPath { .. }))
        .expect("a clip path for the node");
    points(i)
}

/// Area of `poly` inside the pixel `[x, x + 1] x [y, y + 1]`, as a share of the pixel.
fn coverage(poly: &[PointF], x: f32, y: f32) -> f32 {
    let mut p: Vec<(f64, f64)> = poly.iter().map(|q| (f64::from(q.x), f64::from(q.y))).collect();
    let (x, y) = (f64::from(x), f64::from(y));
    // Four half planes: a * px + b * py + c >= 0.
    for (a, b, c) in [(1.0, 0.0, -x), (-1.0, 0.0, x + 1.0), (0.0, 1.0, -y), (0.0, -1.0, y + 1.0)] {
        let inside = |q: &(f64, f64)| a * q.0 + b * q.1 + c >= 0.0;
        let mut out = Vec::new();
        for i in 0..p.len() {
            let (cur, prev) = (p[i], p[(i + p.len() - 1) % p.len()]);
            let cross = || {
                let (dp, dc) = (a * prev.0 + b * prev.1 + c, a * cur.0 + b * cur.1 + c);
                let t = dp / (dp - dc);
                (prev.0 + t * (cur.0 - prev.0), prev.1 + t * (cur.1 - prev.1))
            };
            match (inside(&cur), inside(&prev)) {
                (true, true) => out.push(cur),
                (true, false) => {
                    out.push(cross());
                    out.push(cur);
                }
                (false, true) => out.push(cross()),
                (false, false) => {}
            }
        }
        p = out;
        if p.is_empty() {
            return 0.0;
        }
    }
    let area: f64 = (0..p.len()).map(|i| p[i].0 * p[(i + 1) % p.len()].1 - p[(i + 1) % p.len()].0 * p[i].1).sum();
    (area.abs() / 2.0) as f32
}

// ---- bevelled corners -------------------------------------------------------

fn down_up(s: &mut Scene, p: PointF, t: Instant) -> rsb_core::hit::PointerResult {
    s.pointer(PointerEvent { kind: PointerKind::Down(Button::Left), x: p.x, y: p.y }, t);
    s.pointer(PointerEvent { kind: PointerKind::Up(Button::Left), x: p.x, y: p.y }, t)
}

const BEVEL: &str = ".b { width: 60px; height: 24px; background: #112233; border: 2px solid #ff0000; border-radius: 8px 0 0 0; \
                     corner-shape: bevel round round round; box-shadow: 1px 2px 4px 1px #000000 }";

#[test]
fn a_bevelled_box_paints_paths_and_a_round_one_keeps_the_rect_commands() {
    let t = Instant::now();
    let mut e = classed(boxed(vec![]), &["b"]);
    e.handlers.click = Some(HandlerId(1));
    let mut s = scene_with(BEVEL, 300, 1.0, Some(e), None, None);
    let items = frame(&mut s, t).draw.items.clone();
    let node = node_by_class(&s, "b");
    let mine: Vec<&DrawItem> = items.iter().filter(|i| i.node == node.key).collect();
    let kinds: Vec<&str> = mine
        .iter()
        .map(|i| match &i.cmd {
            DrawCmd::ShadowPath { .. } => "shadow",
            DrawCmd::FillPath { .. } => "fill",
            DrawCmd::BorderPath { .. } => "border",
            DrawCmd::Shadow { .. } | DrawCmd::Fill { .. } | DrawCmd::Border { .. } => "rect command",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["shadow", "fill", "border"], "{kinds:?}");
    // The fill's outline: a cut from (8, 0) to (0, 8) in the box's corner.
    let fill = mine.iter().find(|i| matches!(i.cmd, DrawCmd::FillPath { .. })).unwrap();
    let p = points(fill);
    let r = node.rect;
    assert!(p.contains(&PointF { x: r.x + 8.0, y: r.y }) && p.contains(&PointF { x: r.x, y: r.y + 8.0 }), "{p:?}");
    assert!(!p.contains(&PointF { x: r.x, y: r.y }), "the corner itself is cut off");
    // The border: outer outline minus the same shape moved in, both kept as paths.
    let DrawCmd::BorderPath { outer, inner, .. } = &mine[2].cmd else { panic!() };
    assert!(inner.bounds().w < outer.bounds().w && inner.bounds().h < outer.bounds().h);
    // The shadow follows the outline, offset by 1 px / 2 px and grown by 1 px of spread.
    let DrawCmd::ShadowPath { path, blur, .. } = &mine[0].cmd else { panic!() };
    let sb = path.bounds();
    assert_eq!((sb.x, sb.y, sb.w, sb.h), (r.x + 1.0 - 1.0, r.y + 2.0 - 1.0, r.w + 2.0, r.h + 2.0));
    assert_eq!(*blur, 4.0);
    // A box with only round corners is still drawn with the rect commands.
    s.set_stylesheet(sheet(&BEVEL.replace("corner-shape: bevel round round round;", "")));
    let items = frame(&mut s, at(t, 1)).draw.items.clone();
    assert!(items.iter().any(|i| i.node == node.key && matches!(i.cmd, DrawCmd::Fill { .. })));
    assert!(!items.iter().any(|i| matches!(i.cmd, DrawCmd::FillPath { .. } | DrawCmd::BorderPath { .. })));
}

#[test]
fn the_cut_corner_is_not_part_of_the_hit_area() {
    let t = Instant::now();
    let mut e = classed(boxed(vec![]), &["b"]);
    e.handlers.click = Some(HandlerId(1));
    let mut s = scene_with(BEVEL, 300, 1.0, Some(e), None, None);
    frame(&mut s, t);
    let r = node_by_class(&s, "b").rect;
    // Inside the cut (x + y < 8): outside the shape.
    assert!(down_up(&mut s, PointF { x: r.x + 2.0, y: r.y + 2.0 }, t).calls.is_empty());
    // Just past it, and in the far corner: inside.
    assert_eq!(down_up(&mut s, PointF { x: r.x + 6.0, y: r.y + 6.0 }, t).calls.len(), 1);
    assert_eq!(down_up(&mut s, PointF { x: r.right() - 1.0, y: r.bottom() - 1.0 }, t).calls.len(), 1);
}

#[test]
fn overflow_hidden_on_a_bevelled_box_clips_to_the_shape() {
    let mut s = scene_with(
        ".b { width: 60px; height: 24px; border-radius: 8px; corner-shape: bevel; overflow: hidden } .c { width: 80px; height: 40px; background: #fff }",
        300,
        1.0,
        Some(classed(boxed(vec![classed(boxed(vec![]), &["c"])]), &["b"])),
        None,
        None,
    );
    let items = frame(&mut s, Instant::now()).draw.items.clone();
    let key = node_by_class(&s, "b").key;
    let p = clip_of(&items, key);
    assert_eq!(p.len(), 8, "a rectangle with four cut corners: {p:?}");
    assert_eq!(items.iter().filter(|i| matches!(i.cmd, DrawCmd::PopClip)).count(), 1);
}

#[test]
fn the_outline_follows_the_corner_shape() {
    let mut s = scene_with(
        ".o { width: 40px; height: 20px; outline: 2px solid #00ff00; outline-offset: 1px; border-radius: 6px }",
        300,
        1.0,
        Some(classed(boxed(vec![]), &["o"])),
        None,
        None,
    );
    let items = frame(&mut s, Instant::now()).draw.items.clone();
    let r = node_by_class(&s, "o").rect;
    let outline = items
        .iter()
        .find_map(|i| match &i.cmd {
            DrawCmd::Border { rect, radii, widths, .. } => Some((*rect, *radii, *widths)),
            _ => None,
        })
        .expect("an outline");
    // 1 px offset + 2 px width outside the border box; radii grow by the same.
    assert_eq!(outline.0, r.outset(3.0));
    assert_eq!(outline.1.tl, 9.0);
    assert_eq!(outline.2.top, 2.0);
    s.set_stylesheet(sheet(
        ".o { width: 40px; height: 20px; outline: 2px solid #00ff00; border-radius: 6px; corner-shape: bevel }",
    ));
    let items = frame(&mut s, Instant::now()).draw.items.clone();
    assert!(items.iter().any(|i| matches!(i.cmd, DrawCmd::BorderPath { .. })), "a bevelled box gets a path outline");
}

// ---- clip-path ------------------------------------------------------------------

const HEX: &str = "bar { font-size: 14px } .seg { width: 100px; height: 26px; background: #112233; \
                   clip-path: polygon(13px 0, calc(100% - 13px) 0, 100% 50%, calc(100% - 13px) 100%, 13px 100%, 0 50%) } \
                   .left { margin-left: 3px }";

fn sized(css: &str, scale: f32, e: Element) -> Scene {
    let mut b = bar();
    b.height = 32;
    let mut s = Scene::new(
        &b,
        BarGeometry { width_px: (400.0 * scale) as u32, height_px: (32.0 * scale).round() as u32, scale },
        env(true),
    );
    s.set_stylesheet(sheet(css));
    let mut slots = ZoneSlots::default();
    slots.left.push(slot(1, "w"));
    s.set_slots(slots);
    s.set_tree(tree(1, 1, Some(e)));
    s
}

#[test]
fn clip_path_vertices_land_on_whole_device_pixels_at_every_scale() {
    for scale in [1.0f32, 1.25, 1.5, 2.0] {
        let mut s = sized(HEX, scale, classed(boxed(vec![]), &["seg", "left"]));
        let items = frame(&mut s, Instant::now()).draw.items.clone();
        let key = node_by_class(&s, "seg").key;
        let p = clip_of(&items, key);
        assert_eq!(p.len(), 6);
        for v in &p {
            assert_eq!((v.x, v.y), (v.x.round(), v.y.round()), "{scale}: {v:?}");
        }
        // Horizontal edges are crisp: the top and bottom runs sit on two rows only.
        let rows: std::collections::BTreeSet<i32> = p.iter().map(|v| v.y as i32).collect();
        assert_eq!(rows.len(), 3, "top, middle, bottom: {rows:?}");
        // The group is bracketed by one push and one pop of the node.
        let pops = items.iter().filter(|i| i.node == key && matches!(i.cmd, DrawCmd::PopClip)).count();
        assert_eq!(pops, 1);
        // A box is hit only inside the polygon: its notch corner is outside it.
    }
}

#[test]
fn rounding_is_half_up_and_follows_the_scale() {
    let mut s = sized(HEX, 1.25, classed(boxed(vec![]), &["seg"]));
    let items = frame(&mut s, Instant::now()).draw.items.clone();
    let r = node_by_class(&s, "seg").rect;
    let p = clip_of(&items, node_by_class(&s, "seg").key);
    // 13 px at 1.25 is 16.25: the first vertex lands at x + 16, the right one at right - 16.
    assert_eq!(p[0].x, (r.x + 16.25 + 0.5).floor());
    assert_eq!(p[1].x, (r.x + r.w - 16.25 + 0.5).floor());
    assert_eq!(p[2], PointF { x: (r.x + r.w).round(), y: (r.y + r.h / 2.0 + 0.5).floor() });
}

#[test]
fn clip_path_is_the_hit_area_and_follows_damage() {
    let t = Instant::now();
    let mut e = classed(boxed(vec![]), &["seg"]);
    e.handlers.click = Some(HandlerId(1));
    let mut s = sized(HEX, 1.0, e);
    frame(&mut s, t);
    let r = node_by_class(&s, "seg").rect;
    // The top-left corner is cut away; the middle of the left point and the centre are inside.
    assert!(down_up(&mut s, PointF { x: r.x + 2.0, y: r.y + 1.0 }, t).calls.is_empty());
    assert_eq!(down_up(&mut s, PointF { x: r.x + 3.0, y: r.y + r.h / 2.0 }, t).calls.len(), 1);
    assert_eq!(down_up(&mut s, PointF { x: r.x + r.w / 2.0, y: r.y + 1.0 }, t).calls.len(), 1);
    // Descendants are clipped to it too: a child poking out of the corner is not hit.
    let mut parent = classed(boxed(vec![]), &["seg"]);
    let mut kid = classed(boxed(vec![]), &["kid"]);
    kid.handlers.click = Some(HandlerId(2));
    parent.children = vec![kid];
    let mut s = sized(&format!("{HEX} .kid {{ width: 100px; height: 26px }}"), 1.0, parent);
    frame(&mut s, t);
    let r = node_by_class(&s, "seg").rect;
    let k = node_by_class(&s, "kid").rect;
    assert_eq!((k.x, k.y), (r.x, r.y), "the kid covers the whole segment");
    assert!(down_up(&mut s, PointF { x: r.x + 2.0, y: r.y + 1.0 }, t).calls.is_empty());
    assert_eq!(down_up(&mut s, PointF { x: r.x + r.w / 2.0, y: r.y + 10.0 }, t).calls.len(), 1);
}

#[test]
fn a_changed_clip_path_damages_the_old_and_new_bounds_and_nothing_else() {
    let t0 = Instant::now();
    let css = ".seg { width: 100px; height: 20px; background: #fff; clip-path: polygon(13px 0, 100% 0, 100% 100%, 0 100%) }                .alt { clip-path: polygon(0 0, calc(100% - 30px) 0, calc(100% - 30px) 100%, 0 100%) } .far { width: 10px; height: 10px; background: #f00 }";
    let e = boxed(vec![classed(boxed(vec![]), &["seg"]), classed(boxed(vec![]), &["far"])]);
    let mut s = sized(css, 1.0, e.clone());
    let first = frame(&mut s, t0);
    assert_eq!(first.dirty, Some(rsb_core::geom::RectPx::new(0, 0, 400, 32)), "the first frame is the whole surface");
    let r = node_by_class(&s, "seg").rect;
    let mut e2 = e;
    e2.children[0].classes.push(Arc::from("alt"));
    s.set_tree(tree(1, 2, Some(e2)));
    let d = frame(&mut s, at(t0, 10)).dirty.expect("the shape changed");
    // Old bounds are the whole box, new bounds 30 px narrower; the fill's
    // anti-aliasing margin is one pixel. Nothing beyond that is redrawn.
    let want = rsb_core::geom::RectPx::outward(&r.outset(1.0)).intersect(&rsb_core::geom::RectPx::new(0, 0, 400, 32));
    assert_eq!(Some(d), want);
    // A frame with nothing new has no damage.
    assert_eq!(frame(&mut s, at(t0, 20)).dirty, None);
}

#[test]
fn a_clip_path_animates_between_polygons_with_the_same_point_count() {
    let t0 = Instant::now();
    let css = ".seg { width: 100px; height: 20px; background: #fff; transition: clip-path 100ms linear; \
               clip-path: polygon(0 0, 100% 0, 100% 100%) } .open { clip-path: polygon(40px 0, calc(100% - 20px) 0, 100% 100%) }";
    let mut s = sized(css, 1.0, classed(boxed(vec![]), &["seg"]));
    frame(&mut s, t0);
    s.set_tree(tree(1, 2, Some(classed(boxed(vec![]), &["seg", "open"]))));
    let t1 = at(t0, 1000);
    frame(&mut s, t1);
    let f = frame(&mut s, at(t1, 50));
    let key = node_by_class(&s, "seg").key;
    let r = node_by_class(&s, "seg").rect;
    let p = clip_of(&f.draw.items, key);
    assert_eq!(p[0].x, (r.x + 20.0).round(), "half way from 0 to 40");
    assert_eq!(p[1].x, (r.x + 90.0).round(), "half way from 100 to 80");
    // Different point counts never animate: the new polygon is used at once.
    s.set_tree(tree(1, 3, Some(classed(boxed(vec![]), &["seg"]))));
    frame(&mut s, at(t1, 400));
    let f = frame(&mut s, at(t1, 600));
    assert_eq!(clip_of(&f.draw.items, key).len(), 3);
}

// ---- anchor_flat, center_on and the seam ------------------------------------------

const CENTRE: &str = "bar { font-size: 14px } zone { gap: 0px } \
    .mem { width: 60px; height: 26px; background: #222 } \
    .clock { width: 77px; height: 26px; background: #333 } \
    .wide { width: 150px } \
    .dash { width: 54px; height: 26px; background: #81cbea; \
            clip-path: polygon(13px 0, calc(100% - 13px) 0, 100% 50%, calc(100% - 13px) 100%, 13px 100%, 0 50%) }";

fn centre_scene(css: &str, scale: f32, pin: Option<&str>, mem: &str, clock: &str) -> Scene {
    let mut b = bar();
    b.height = 32;
    b.center_on = pin.map(str::to_string);
    let w = (800.0 * scale) as u32;
    let h = (32.0 * scale).round() as u32;
    let mut s = Scene::new(&b, BarGeometry { width_px: w, height_px: h, scale }, env(true));
    s.set_stylesheet(sheet(css));
    let mut slots = ZoneSlots::default();
    for (n, class) in [(1, "wmem"), (2, "wdash"), (3, "wclock")] {
        slots.center.push(slot(n, class));
    }
    s.set_slots(slots);
    s.set_tree(tree(1, 1, Some(classed(boxed(vec![]), &["mem", mem]))));
    s.set_tree(tree(2, 1, Some(classed(boxed(vec![]), &["dash"]))));
    s.set_tree(tree(3, 1, Some(classed(boxed(vec![]), &["clock", clock]))));
    s
}

fn centre_x(s: &Scene, class: &str) -> f32 {
    let r = node_by_class(s, class).rect;
    r.x + r.w / 2.0
}

#[test]
fn the_pinned_widget_stays_on_the_bars_centre_while_neighbours_grow_away() {
    for scale in [1.0f32, 1.25, 1.5, 2.0] {
        let mid = 800.0 * scale / 2.0;
        let mut s = centre_scene(CENTRE, scale, Some("wdash"), "", "");
        frame(&mut s, Instant::now());
        let zone = node_by_id(&s, "center").rect;
        assert_eq!(zone.x, zone.x.floor(), "{scale}: the zone starts on a whole pixel");
        assert!((centre_x(&s, "dash") - mid).abs() <= 0.5, "{scale}: {} vs {mid}", centre_x(&s, "dash"));
        let dash_x = node_by_class(&s, "dash").rect.x;
        // The clock to its right widens: the dash does not move.
        let mut s2 = centre_scene(CENTRE, scale, Some("wdash"), "", "wide");
        frame(&mut s2, Instant::now());
        assert_eq!(node_by_class(&s2, "dash").rect.x, dash_x, "{scale}: right neighbour grows away");
        assert!(node_by_class(&s2, "clock").rect.right() > node_by_class(&s, "clock").rect.right());
        // The same for the one on its left.
        let mut s3 = centre_scene(&CENTRE.replace(".wide", ".mem.wide"), scale, Some("wdash"), "wide", "");
        frame(&mut s3, Instant::now());
        assert_eq!(node_by_class(&s3, "dash").rect.x, dash_x, "{scale}: left neighbour grows away");
        // Without the pin the whole zone is centred, so the dash moves.
        let mut s4 = centre_scene(CENTRE, scale, None, "", "wide");
        frame(&mut s4, Instant::now());
        assert_ne!(node_by_class(&s4, "dash").rect.x, dash_x);
    }
}

#[test]
fn the_pinned_zone_still_clears_its_neighbours() {
    // A very wide left neighbour pushes the centre zone right, as before.
    let mut b = bar();
    b.height = 32;
    b.center_on = Some("wdash".into());
    let mut s = Scene::new(&b, BarGeometry { width_px: 300, height_px: 32, scale: 1.0 }, env(true));
    s.set_stylesheet(sheet(&format!("{CENTRE} .big {{ width: 200px; height: 20px }}")));
    let mut slots = ZoneSlots::default();
    slots.left.push(slot(4, "big"));
    slots.center.push(slot(2, "wdash"));
    s.set_slots(slots);
    s.set_tree(tree(4, 1, Some(classed(boxed(vec![]), &["big"]))));
    s.set_tree(tree(2, 1, Some(classed(boxed(vec![]), &["dash"]))));
    frame(&mut s, Instant::now());
    assert_eq!(node_by_class(&s, "dash").rect.x, 208.0, "200 + the 8 px zone gap");
}

#[test]
fn center_on_reads_a_ref_from_bar_toml_and_warns_when_it_is_not_in_center() {
    let none = |_: &str| None;
    let l = load_config(
        Some(("[[bar]]\ncenter = [\"clock\", \"dashboard\"]\ncenter_on = \"dashboard\"\n", "bar.toml")),
        &none,
    );
    assert!(l.diagnostics.is_empty(), "{:?}", l.diagnostics);
    assert_eq!(l.config.unwrap().bars[0].center_on.as_deref(), Some("dashboard"));
    let l = load_config(Some(("[[bar]]\ncenter = [\"clock\"]\ncenter_on = \"dashboard\"\n", "bar.toml")), &none);
    let d: Vec<String> = l.diagnostics.iter().map(|d| d.to_string()).collect();
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].starts_with("warning: bar.toml:3:"), "{d:?}");
    assert!(d[0].contains("'center_on' names 'dashboard', which is not in center"), "{d:?}");
    assert_eq!(l.config.unwrap().bars[0].center_on, None);
    let l = load_config(Some(("[[bar]]\ncenter = [\"clock#utc\"]\ncenter_on = \"clock#utc\"\n", "bar.toml")), &none);
    assert!(l.diagnostics.is_empty(), "{:?}", l.diagnostics);
    let l = load_config(Some(("[[bar]]\ncenter_on = 5\n", "bar.toml")), &none);
    assert!(l.diagnostics[0].to_string().contains("'center_on'"), "{:?}", l.diagnostics);
    assert!(rsb_core::config::BAR_KEYS.contains(&"center_on"));
    assert_eq!(WidgetRef::parse("dashboard").unwrap().css_class(), "dashboard");
}

#[test]
fn a_hexagons_flat_edges_are_its_top_and_bottom_runs() {
    for scale in [1.0f32, 1.25, 1.5, 2.0] {
        let mut s = centre_scene(CENTRE, scale, Some("wdash"), "", "");
        frame(&mut s, Instant::now());
        let n = node_by_class(&s, "dash");
        let flat = s.flat_edges_of(n.key).expect("a visible node");
        let (top, bottom) = (flat.top.unwrap(), flat.bottom.unwrap());
        assert_eq!(top, bottom, "a hexagon is the same on both sides");
        let d = (13.0 * scale).round();
        let w = n.rect.w;
        // The run starts where the left point's edge meets the top, and ends at the right one.
        assert!(
            (top.0 - (n.rect.x + 13.0 * scale)).abs() <= 0.5 && (top.1 - (n.rect.x + w - 13.0 * scale)).abs() <= 0.5
        );
        assert_eq!((top.0, top.1), (top.0.round(), top.1.round()), "whole pixels");
        assert!(d > 0.0);
        // A chevron tip has no flat run on that side.
        let tip = format!("{CENTRE} .dash {{ clip-path: polygon(0 0, 100% 50%, 0 100%) }}");
        let mut t = centre_scene(&tip, scale, Some("wdash"), "", "");
        frame(&mut t, Instant::now());
        let f = t.flat_edges_of(node_by_class(&t, "dash").key).unwrap();
        assert_eq!((f.top, f.bottom), (None, None));
    }
    // Without a clip-path it is the border box.
    let mut s = centre_scene(&format!("{CENTRE} .dash {{ clip-path: none }}"), 1.5, Some("wdash"), "", "");
    frame(&mut s, Instant::now());
    let n = node_by_class(&s, "dash");
    let f = s.flat_edges_of(n.key).unwrap();
    assert_eq!(f.bottom, Some((n.rect.x.round(), n.rect.right().round())));
    assert_eq!(s.flat_edges_of(rsb_core::element::NodeKey(1)), None);
}

#[test]
fn the_seam_rows_are_solid_above_and_below_at_every_scale() {
    for scale in [1.0f32, 1.25, 1.5, 2.0] {
        let mut s = centre_scene(CENTRE, scale, Some("wdash"), "", "");
        let items = frame(&mut s, Instant::now()).draw.items.clone();
        let n = node_by_class(&s, "dash");
        let flat = s.flat_edges_of(n.key).unwrap().bottom.expect("a flat bottom");
        let poly = clip_of(&items, n.key);
        let bottom = poly.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        let row = bottom - 1.0;
        // The attached panel takes exactly these columns, from the row below the strip.
        let (x0, x1) = flat;
        assert_eq!((x0, x1), (x0.round(), x1.round()));
        for x in (x0 as i32)..(x1 as i32) {
            let strip = coverage(&poly, x as f32, row);
            assert!(strip > 0.999, "{scale}: strip alpha at column {x}, row {row} is {strip}");
        }
        // And they are exactly the columns that are solid: the one beside each end is not.
        assert!(coverage(&poly, x0 - 1.0, row) < 0.999, "{scale}: left of the run");
        assert!(coverage(&poly, x1, row) < 0.999, "{scale}: right of the run");
        // The panel: a rect from the flat edge's columns, whole pixels, is solid in every column.
        let panel = [
            PointF { x: x0, y: bottom },
            PointF { x: x1, y: bottom },
            PointF { x: x1, y: bottom + 40.0 },
            PointF { x: x0, y: bottom + 40.0 },
        ];
        for x in (x0 as i32)..(x1 as i32) {
            assert!(coverage(&panel, x as f32, bottom) > 0.999, "{scale}: panel alpha at column {x}");
        }
    }
}

#[test]
fn without_snapping_the_seam_would_leave_a_partial_column() {
    // The same hexagon with vertices left where the arithmetic puts them (negative control).
    let scale = 1.25f32;
    let x0 = 16.25f32;
    let raw = [
        PointF { x: x0, y: 0.0 },
        PointF { x: 100.0, y: 0.0 },
        PointF { x: 100.0, y: 32.0 },
        PointF { x: x0, y: 32.0 },
    ];
    assert!(coverage(&raw, 16.0, 31.0) < 0.999, "column 16 is only 3/4 covered at scale {scale}");
}

// ---- empty widgets and structural pseudo-classes ----------------------------------------

fn hidden(mut e: Element) -> Element {
    e.style.push(("display".into(), "none".into()));
    e
}

fn left_scene(css: &str, widgets: Vec<Option<Element>>) -> Scene {
    let mut b = bar();
    b.height = 32;
    let mut s = Scene::new(&b, BarGeometry { width_px: 400, height_px: 32, scale: 1.0 }, env(true));
    s.set_stylesheet(sheet(css));
    let mut slots = ZoneSlots::default();
    for (i, _) in widgets.iter().enumerate() {
        slots.left.push(slot(i as u32 + 1, &format!("w{i}")));
    }
    s.set_slots(slots);
    for (i, w) in widgets.into_iter().enumerate() {
        s.set_tree(tree(i as u32 + 1, 1, w));
    }
    s
}

fn seg(class: &str) -> Element {
    classed(boxed(vec![]), &[class, "seg"])
}

const SEGS: &str = ".seg { width: 20px; height: 10px; background: #fff } \
    zone#left > widget:first-child { color: #ff0000 } \
    zone#left > widget:last-child { color: #00ff00 } \
    zone#left > widget:nth-child(2) { color: #0000ff } \
    zone#left > widget:nth-child(odd) { opacity: 0.5 }";

fn colour(s: &Scene, id: &str) -> (u8, u8, u8) {
    let c = node_by_id(s, id).style.color;
    ((c.r * 255.0).round() as u8, (c.g * 255.0).round() as u8, (c.b * 255.0).round() as u8)
}

#[test]
fn an_empty_widget_takes_no_space_and_is_not_counted() {
    let mut s = left_scene(SEGS, vec![Some(seg("a")), Some(hidden(seg("b"))), Some(seg("c")), None]);
    frame(&mut s, Instant::now());
    let b = node_by_id(&s, "w1");
    assert!(b.style.display_none, "a widget whose root is hidden is display: none");
    assert_eq!(node_by_id(&s, "w2").rect.x, node_by_id(&s, "w0").rect.right(), "no space for the hidden one");
    // c is the second child, and the last; a is first. The hidden one matched nothing.
    assert_eq!(colour(&s, "w0"), (255, 0, 0));
    assert_eq!(colour(&s, "w2"), (0, 0, 255), "second and last: the later rule, nth-child(2), wins");
    assert_eq!(node_by_id(&s, "w2").style.opacity, 1.0, "second is not odd");
    assert_eq!(node_by_id(&s, "w0").style.opacity, 0.5);
    // The widget with no root at all is not in the scene.
    assert!(s.nodes().iter().all(|n| n.id.as_deref() != Some("w3")));
    // The drawn list has nothing for the hidden one, and it is not hit.
    let items = frame(&mut s, Instant::now()).draw.items.clone();
    assert!(!items.iter().any(|i| i.node == b.key));
}

#[test]
fn counts_update_when_a_widget_appears_or_leaves() {
    let t = Instant::now();
    let mut s = left_scene(SEGS, vec![Some(seg("a")), None, Some(seg("c"))]);
    frame(&mut s, t);
    assert_eq!(colour(&s, "w0"), (255, 0, 0));
    assert_eq!(colour(&s, "w2"), (0, 0, 255), "second and last");
    // The one between them appears: c is the third child now, a is still first.
    s.set_tree(tree(2, 2, Some(seg("b"))));
    frame(&mut s, at(t, 10));
    assert_eq!(colour(&s, "w1"), (0, 0, 255), "second child");
    assert_eq!(colour(&s, "w2"), (0, 255, 0));
    assert_eq!(node_by_id(&s, "w2").style.opacity, 0.5, "third is odd");
    // The first one leaves: b becomes first.
    s.set_tree(tree(1, 2, None));
    frame(&mut s, at(t, 20));
    assert_eq!(colour(&s, "w1"), (255, 0, 0));
    assert_eq!(colour(&s, "w2"), (0, 0, 255), "c is second and last now");
    // With only one widget left it is both first and last (last wins by order).
    s.set_tree(tree(2, 3, None));
    frame(&mut s, at(t, 30));
    assert_eq!(colour(&s, "w2"), (0, 255, 0));
}

#[test]
fn a_sibling_hidden_by_a_rule_leaves_the_count_too() {
    let css = format!("{SEGS} .gone {{ display: none }}");
    let hide = |c: &str| classed(boxed(vec![]), &[c, "seg", "gone"]);
    let mut s = left_scene(&css, vec![Some(hide("a")), Some(seg("b")), Some(seg("c"))]);
    // The widgets' own roots carry the class: only they are hidden, their wrappers stay.
    frame(&mut s, Instant::now());
    assert_eq!(colour(&s, "w0"), (255, 0, 0));
    // Hide a wrapper itself through its own class instead.
    let css = format!("{SEGS} widget.w0 {{ display: none }}");
    let mut s = left_scene(&css, vec![Some(seg("a")), Some(seg("b")), Some(seg("c"))]);
    frame(&mut s, Instant::now());
    assert!(node_by_id(&s, "w0").style.display_none);
    assert_eq!(colour(&s, "w1"), (255, 0, 0), "b is the first one that takes part");
    assert_eq!(colour(&s, "w2"), (0, 0, 255), "c is second and last");
    assert_eq!(node_by_id(&s, "w2").style.opacity, 1.0, "c is second now, not odd");
    assert_eq!(node_by_id(&s, "w1").style.opacity, 0.5);
}

#[test]
fn a_structural_rule_inside_a_widget_counts_the_widgets_own_children() {
    let css = ".row { width: 60px; height: 10px } .k { width: 10px; height: 10px; background: #fff } \
               .k:first-child { background: #ff0000 } .k:last-child { background: #00ff00 } .k:nth-child(2n) { opacity: 0.5 } .none { display: none }";
    let kid = |c: &str| classed(boxed(vec![]), &["k", c]);
    let e = classed(boxed(vec![kid("a"), kid("none"), kid("b"), kid("c"), kid("d")]), &["row"]);
    let mut s = left_scene(css, vec![Some(e)]);
    frame(&mut s, Instant::now());
    let bg = |s: &Scene, c: &str| match node_by_class(s, c).style.background {
        rsb_core::style::Background::Color(c) => (c.r, c.g, c.b),
        _ => panic!(),
    };
    assert_eq!(bg(&s, "a"), (1.0, 0.0, 0.0));
    assert_eq!(bg(&s, "d"), (0.0, 1.0, 0.0));
    // a, b, c, d are positions 1 to 4 (the hidden one is not counted).
    assert_eq!(node_by_class(&s, "b").style.opacity, 0.5);
    assert_eq!(node_by_class(&s, "c").style.opacity, 1.0);
    assert_eq!(node_by_class(&s, "d").style.opacity, 0.5);
}

// ---- pointer rules ------------------------------------------------------------------------

#[test]
fn cursor_text_shows_the_i_beam_and_pointer_events_none_lets_the_pointer_through() {
    let t = Instant::now();
    let css = ".t { cursor: text; width: 40px; height: 20px } .mute { pointer-events: none; width: 40px; height: 20px } \
               .row { width: 100px; height: 20px }";
    let mut under = classed(boxed(vec![]), &["t"]);
    under.handlers.click = Some(HandlerId(1));
    let mut over = classed(boxed(vec![classed(boxed(vec![]), &["t"])]), &["mute"]);
    over.handlers.click = Some(HandlerId(2));
    let row = classed(boxed(vec![under, over]), &["row"]);
    let mut s = scene_with(css, 300, 1.0, Some(row), None, None);
    frame(&mut s, t);
    let r = node_by_class(&s, "t").rect;
    s.pointer(PointerEvent { kind: PointerKind::Move, x: r.x + 5.0, y: r.y + 5.0 }, t);
    assert_eq!(frame(&mut s, at(t, 1)).cursor, Cursor::Text);
    // The muted box and its kid take no pointer: a press there reaches nothing (the row has no handler).
    let m = node_by_class(&s, "mute").rect;
    let res = down_up(&mut s, PointF { x: m.x + 5.0, y: m.y + 5.0 }, t);
    assert!(res.calls.is_empty(), "{res:?}");
    assert_eq!(frame(&mut s, at(t, 2)).cursor, Cursor::Default);
    // The box before it still works.
    assert_eq!(down_up(&mut s, PointF { x: r.x + 5.0, y: r.y + 5.0 }, t).calls.len(), 1);
}

// ---- SVG ------------------------------------------------------------------------------------

#[test]
fn svg_icons_and_svg_images_become_svg_commands() {
    let mut icon = Element::new(ElementKind::Icon);
    icon.icon = Some(IconRef::Svg(Arc::from("power")));
    icon.classes = vec![Arc::from("i")];
    let mut img = Element::new(ElementKind::Image);
    img.image =
        Some(rsb_core::element::ImageRef { path: Arc::from("C:/x/logo.svg"), width: Some(20.0), height: Some(10.0) });
    img.classes = vec![Arc::from("m")];
    let mut s =
        scene_with(".i { font-size: 16px; color: #ff8800 } .m { }", 300, 1.0, Some(boxed(vec![icon, img])), None, None);
    let items = frame(&mut s, Instant::now()).draw.items.clone();
    let i = node_by_class(&s, "i");
    assert_eq!((i.rect.w, i.rect.h), (16.0, 16.0), "a picture icon is a square the size of its font");
    let svgs: Vec<&DrawItem> = items.iter().filter(|x| matches!(x.cmd, DrawCmd::Svg { .. })).collect();
    assert_eq!(svgs.len(), 2);
    match &svgs[0].cmd {
        DrawCmd::Svg { key, tint, rect } => {
            assert_eq!(key.as_ref(), "power");
            assert_eq!(tint.map(|c| (c.r * 255.0).round() as u8), Some(255), "tinted with the element's colour");
            assert_eq!((rect.w, rect.h), (16.0, 16.0));
        }
        _ => unreachable!(),
    }
    match &svgs[1].cmd {
        DrawCmd::Svg { key, tint, .. } => assert_eq!((key.as_ref(), *tint), ("C:/x/logo.svg", None), "own colours"),
        _ => unreachable!(),
    }
    // No text is drawn for it, and a wrong name is a warning with a hint.
    assert!(texts(&items).is_empty());
    let mut bad = Element::new(ElementKind::Icon);
    bad.icon = Some(IconRef::Svg(Arc::from("pwer")));
    s.set_tree(tree(1, 2, Some(boxed(vec![bad]))));
    frame(&mut s, Instant::now());
    let items = frame(&mut s, Instant::now()).draw.items.clone();
    // The wrong name draws a small outlined square (not nothing), so the gap shows.
    assert!(!items.iter().any(|x| matches!(x.cmd, DrawCmd::Svg { .. })));
    assert!(
        items.iter().any(|x| matches!(&x.cmd, DrawCmd::Border { rect, .. } if rect.w >= 2.0 && rect.w == rect.h)),
        "{items:?}"
    );
    let d = s.take_diagnostics();
    assert!(d.iter().any(|x| x.diagnostic.message.contains("unknown svg icon 'pwer'; did you mean 'power'?")), "{d:?}");
}

#[test]
fn the_bundled_svg_names_are_the_documented_set() {
    use rsb_core::icons::SVG_ICON_NAMES;
    assert_eq!(
        &SVG_ICON_NAMES[..10],
        ["logo", "power", "restart", "sleep", "lock", "sign-out", "bolt", "leaf", "balance", "rocket"]
    );
    assert_eq!(SVG_ICON_NAMES.last(), Some(&"music"));
    assert_eq!(SVG_ICON_NAMES.len(), 54);
}

// ---- a hover reveal that moves what is hovered ----------------------------------------------

#[test]
fn a_reveal_on_the_whole_zone_settles_with_a_parked_pointer() {
    // The right zone shows a hidden segment while hovered and collapses 300 ms after the pointer leaves it.
    let css = "bar { font-size: 10px } \
        .seg { height: 20px; background: #234 }         .seg:hover { opacity: 0.9 } \
        .near { width: 50px } \
        .reveal { width: 20px; overflow: hidden; transition: width 100ms linear 300ms } \
        zone#right:hover .reveal { width: 120px; transition: width 100ms linear }";
    let t0 = Instant::now();
    let mut b = bar();
    b.height = 32;
    let mut s = Scene::new(&b, BarGeometry { width_px: 400, height_px: 32, scale: 1.0 }, env(true));
    s.set_stylesheet(sheet(css));
    let mut slots = ZoneSlots::default();
    slots.right.push(slot(1, "wnear"));
    slots.right.push(slot(2, "wreveal"));
    s.set_slots(slots);
    s.set_tree(tree(1, 1, Some(classed(boxed(vec![]), &["seg", "near"]))));
    s.set_tree(tree(2, 1, Some(classed(boxed(vec![]), &["seg", "reveal"]))));
    frame(&mut s, t0);
    // Park the pointer at the right edge of the segment next to the one that reveals.
    let near = node_by_class(&s, "near").rect;
    let p = PointF { x: near.right() - 1.0, y: near.y + 5.0 };
    // Who is hovered, read from `.seg:hover`, frame by frame.
    let mut hovered: Vec<(bool, bool)> = Vec::new();
    let mut widths = Vec::new();
    for i in 0..80u64 {
        let now = at(t0, 10 + i * 16);
        s.pointer(PointerEvent { kind: PointerKind::Move, x: p.x, y: p.y }, now);
        frame(&mut s, now);
        hovered.push((node_by_class(&s, "near").style.opacity < 1.0, node_by_class(&s, "reveal").style.opacity < 1.0));
        widths.push(width_of(&s, "reveal"));
    }
    let chain_changes = hovered.windows(2).filter(|w| w[0] != w[1]).count();
    // Hover held the whole time: the reveal reaches its full width and stays there.
    assert_eq!(*widths.last().unwrap(), 120.0, "{widths:?}");
    assert!(widths.windows(2).all(|w| w[1] >= w[0]), "never collapses and re-opens: {widths:?}");
    let settled_at = widths.iter().position(|w| *w == 120.0).unwrap();
    assert!(settled_at < 62, "settled within 1 s: frame {settled_at}");
    // The pointer starts on the neighbour and ends on the revealed segment: one change, then quiet.
    assert_eq!(chain_changes, 1, "{hovered:?}");
    assert_eq!(hovered[0], (true, false));
    assert_eq!(*hovered.last().unwrap(), (false, true));
    assert!(hovered[settled_at..].iter().all(|h| *h == (false, true)));
    assert!(s.is_settled());
    // After the pointer leaves, it collapses only after the 300 ms delay.
    let leave = at(t0, 2000);
    s.pointer(PointerEvent { kind: PointerKind::Leave, x: 0.0, y: 0.0 }, leave);
    frame(&mut s, leave);
    frame(&mut s, at(leave, 250));
    assert_eq!(width_of(&s, "reveal"), 120.0, "still open inside the delay");
    frame(&mut s, at(leave, 350));
    assert!(width_of(&s, "reveal") < 120.0);
    frame(&mut s, at(leave, 500));
    assert_eq!(width_of(&s, "reveal"), 20.0);
}

fn width_of(s: &Scene, class: &str) -> f32 {
    node_by_class(s, class).rect.w
}

mod common;

use std::sync::Arc;
use std::time::Instant;

use common::*;
use rsb_core::draw::{DrawCmd, DrawItem, Fill};
use rsb_core::element::{Element, ElementKind};
use rsb_core::geom::RectF;
use rsb_core::scene::NodeInfo;

/// Every character is 0.6 * size wide with `FixedMeasure`; at 10 px that is 6 px.
const BASE: &str = "bar { font-size: 10px }";

fn pad(n: usize) -> String {
    "x".repeat(n)
}

fn drawn(s: &mut rsb_core::scene::Scene) -> Vec<DrawItem> {
    frame(s, Instant::now()).draw.items.clone()
}

fn by_text<'a>(items: &'a [DrawItem], text: &str) -> &'a DrawItem {
    items
        .iter()
        .find(|i| matches!(&i.cmd, DrawCmd::Text { text: t, .. } if t.as_ref() == text))
        .unwrap_or_else(|| panic!("no text {text:?} in {:?}", texts(items)))
}

#[test]
fn zones_sit_at_the_start_the_end_and_the_middle() {
    let mut s = scene_with(BASE, 400, 1.0, Some(text(&pad(10))), Some(text(&pad(10))), Some(text(&pad(10))));
    frame(&mut s, Instant::now());
    let (l, c, r) = (node_by_id(&s, "left"), node_by_id(&s, "center"), node_by_id(&s, "right"));
    assert_eq!((l.rect.x, l.rect.w), (0.0, 60.0));
    assert_eq!((r.rect.x, r.rect.right()), (340.0, 400.0));
    assert_eq!((c.rect.x, c.rect.w), (170.0, 60.0));
    assert_eq!(c.rect.x + c.rect.w / 2.0, 200.0, "centred on the bar");
    // Zones fill the bar's height; their content is centred inside.
    assert_eq!((l.rect.y, l.rect.h), (0.0, 32.0));
    let t = node_by_class(&s, "l");
    assert_eq!(t.rect.x, 0.0);
}

#[test]
fn the_centre_is_only_pushed_as_far_as_it_must() {
    // Left is 180 wide, so a 60 wide centre at 170 must move to 180 plus the 8 px zone gap.
    let mut s = scene_with(BASE, 400, 1.0, Some(text(&pad(30))), Some(text(&pad(10))), Some(text(&pad(5))));
    frame(&mut s, Instant::now());
    assert_eq!(node_by_id(&s, "center").rect.x, 188.0);
    // Right is 180 wide (starts at 220), so a centre at 170 would overlap it: it ends 8 px short of it.
    let mut s = scene_with(BASE, 400, 1.0, Some(text(&pad(5))), Some(text(&pad(10))), Some(text(&pad(30))));
    frame(&mut s, Instant::now());
    let c = node_by_id(&s, "center");
    assert_eq!(c.rect.right(), 212.0);
    assert_eq!(node_by_id(&s, "right").rect.x, 220.0);
    // A centre that already clears its neighbours is not moved by the gap.
    let mut s = scene_with(BASE, 400, 1.0, Some(text(&pad(10))), Some(text(&pad(10))), Some(text(&pad(10))));
    frame(&mut s, Instant::now());
    assert_eq!(node_by_id(&s, "center").rect.x, 170.0);
    // The gap scales with the display.
    let mut s = scene_with(BASE, 800, 2.0, Some(text(&pad(30))), Some(text(&pad(10))), Some(text(&pad(5))));
    frame(&mut s, Instant::now());
    let (l, c) = (node_by_id(&s, "left"), node_by_id(&s, "center"));
    assert_eq!(c.rect.x - l.rect.right(), 16.0);
}

#[test]
fn a_zone_that_cannot_fit_is_shrunk_and_clipped() {
    // 240 + 120 + 240 does not fit 400 px. The left zone keeps its width and
    // the right zone shrinks; the centre gets what remains (nothing).
    let mut s = scene_with(BASE, 400, 1.0, Some(text(&pad(40))), Some(text(&pad(20))), Some(text(&pad(40))));
    let items = drawn(&mut s);
    let (l, c, r) = (node_by_id(&s, "left"), node_by_id(&s, "center"), node_by_id(&s, "right"));
    assert_eq!(l.rect.w, 240.0);
    assert_eq!(r.rect.w, 160.0);
    assert_eq!(r.rect.right(), 400.0);
    assert!(c.rect.w <= 0.5, "no room for the centre: {:?}", c.rect);
    let clips = items.iter().filter(|i| matches!(i.cmd, DrawCmd::PushClip { .. })).count();
    let pops = items.iter().filter(|i| matches!(i.cmd, DrawCmd::PopClip)).count();
    assert!(clips >= 2 && clips == pops, "shrunk zones clip their content: {clips} pushes, {pops} pops");
    // Nothing a clipped zone holds is drawn outside it.
    let r_clip = items.iter().find_map(|i| match &i.cmd {
        DrawCmd::PushClip { rect, .. } if rect.x == 240.0 && rect.w > 0.0 => Some(*rect),
        _ => None,
    });
    assert_eq!(r_clip.map(|r| r.w), Some(160.0));
}

#[test]
fn ellipsis_cuts_at_a_char_and_adds_the_dots() {
    let css = format!("{BASE} .e {{ text-overflow: ellipsis; max-width: 60px }}");
    let long = classed(text("abcdefghijklmnop"), &["e"]);
    let mut s = scene_with(&css, 400, 1.0, Some(long), None, None);
    let items = drawn(&mut s);
    // 60 px = 10 chars; nine letters and the ellipsis.
    let shown = texts(&items);
    assert_eq!(shown, ["abcdefghi\u{2026}"]);
    let e = node_by_class(&s, "e");
    assert_eq!(e.rect.w, 60.0);
    assert_eq!(e.text.as_deref(), Some("abcdefghi\u{2026}"));

    // Text that fits is untouched.
    let short = classed(text("abc"), &["e"]);
    let mut s = scene_with(&css, 400, 1.0, Some(short), None, None);
    assert_eq!(texts(&drawn(&mut s)), ["abc"]);

    // Without text-overflow the box is as wide as the text.
    let mut s = scene_with(BASE, 400, 1.0, Some(classed(text("abcdefghijklmnop"), &["e"])), None, None);
    frame(&mut s, Instant::now());
    assert_eq!(node_by_class(&s, "e").rect.w, 96.0);
}

#[test]
fn text_a_fraction_of_a_pixel_wider_than_its_rounded_box_is_not_cut() {
    // 15 chars at 10.04 px: 90.36 px of text in a box rounded to 90 px.
    let css = format!("{BASE} .e {{ text-overflow: ellipsis; max-width: 200px; font-size: 10.04px }}");
    let mut s = scene_with(&css, 400, 1.0, Some(classed(text("abcdefghijklmno"), &["e"])), None, None);
    assert_eq!(texts(&drawn(&mut s)), ["abcdefghijklmno"]);
    assert_eq!(node_by_class(&s, "e").rect.w, 90.0);
}

#[test]
fn ellipsis_with_no_room_shows_nothing() {
    let css = format!("{BASE} .e {{ text-overflow: ellipsis; max-width: 4px }}");
    let mut s = scene_with(&css, 400, 1.0, Some(classed(text("abcdef"), &["e"])), None, None);
    assert!(texts(&drawn(&mut s)).is_empty());
}

#[test]
fn physical_pixels_follow_the_scale_with_whole_pixel_edges() {
    let css = "bar { font-size: 10px; padding: 0 5px } .b { padding: 3px; border: 1px solid #fff; margin: 0 2px } zone { gap: 3px }";
    for scale in [1.0f32, 1.25, 1.5, 1.75, 2.0] {
        let b1 = classed(boxed(vec![text("ab"), text("cd")]), &["b"]);
        let b2 = classed(boxed(vec![text("e")]), &["b"]);
        let mut s = scene_with(css, 401, scale, Some(boxed(vec![b1, b2])), Some(text("mid")), Some(text("end")));
        frame(&mut s, Instant::now());
        for n in s.nodes() {
            for (name, v) in [("x", n.rect.x), ("y", n.rect.y), ("right", n.rect.right()), ("bottom", n.rect.bottom())]
            {
                assert_eq!(
                    v,
                    v.round(),
                    "scale {scale}: {:?} {:?} has a fractional {name}: {:?}",
                    n.kind,
                    n.classes,
                    n.rect
                );
            }
        }
    }
    // A 1 px border stays one whole pixel at any scale.
    let mut s = scene_with(
        "bar { font-size: 10px } .b { border: 1px solid #fff; padding: 2px }",
        400,
        1.25,
        Some(classed(text("a"), &["b"])),
        None,
        None,
    );
    let items = drawn(&mut s);
    let widths: Vec<_> = items
        .iter()
        .filter_map(|i| match &i.cmd {
            DrawCmd::Border { widths, .. } => Some(*widths),
            _ => None,
        })
        .collect();
    assert_eq!(widths.len(), 1);
    assert_eq!((widths[0].top, widths[0].left), (1.0, 1.0));
    // The text is 0.6 * 12.5 = 7.5 wide; padding 2 px * 1.25 = 2.5 rounds to 3 whole pixels a side.
    let b = node_by_class(&s, "b");
    assert_eq!(b.style.padding.left, 2.0);
    assert_eq!(b.rect.w, (7.5f32 + 2.0 * 3.0 + 2.0).round());
    let content = items.iter().find_map(|i| match &i.cmd {
        DrawCmd::Text { rect, .. } => Some(*rect),
        _ => None,
    });
    let content = content.unwrap();
    assert_eq!((content.x, content.y), (b.rect.x + 1.0 + 3.0, b.rect.y + 1.0 + 3.0), "text starts on a whole pixel");
}

#[test]
fn units_em_and_percent() {
    let css = "bar { font-size: 10px } .a { padding: 1em; font-size: 20px } .p { width: 50% }";
    let a = classed(boxed(vec![text("x")]), &["a"]);
    let wrap = classed(boxed(vec![classed(boxed(vec![]), &["p"])]), &["wrap"]);
    let mut s = scene_with(css, 400, 1.0, Some(a), Some(classed(wrap, &["w"])), None);
    frame(&mut s, Instant::now());
    let a = node_by_class(&s, "a");
    // em padding against the element's own font size (20 px), text 12 px wide.
    assert_eq!(a.style.padding.left, 20.0);
    assert_eq!(a.rect.w, 12.0 + 40.0);
}

#[test]
fn the_draw_list_starts_with_the_hit_fill_and_keeps_the_specified_order() {
    let css = "bar { background: #112233 } .a { background: #445566; border: 1px solid #778899; border-radius: 4px; box-shadow: 0 1px 2px #000000 } .a text { color: #ffffff }";
    let a = classed(boxed(vec![text("hi")]), &["a"]);
    let mut s = scene_with(css, 200, 1.0, Some(a), None, None);
    let items = drawn(&mut s);
    // First item: the bar's border box, black at alpha 1/255.
    match &items[0].cmd {
        DrawCmd::Fill { rect, fill: Fill::Solid(c), .. } => {
            assert_eq!(*rect, RectF::new(0.0, 0.0, 200.0, 32.0));
            assert_eq!((c.r, c.g, c.b), (0.0, 0.0, 0.0));
            assert!((c.a * 255.0 - 1.0).abs() < 1e-4, "alpha {}", c.a);
        }
        other => panic!("first item is {other:?}"),
    }
    // Then the bar's own background, then the card: shadow, background, border, then its text.
    let kinds: Vec<&str> = items
        .iter()
        .map(|i| match &i.cmd {
            DrawCmd::Shadow { .. } => "shadow",
            DrawCmd::Fill { .. } => "fill",
            DrawCmd::Border { .. } => "border",
            DrawCmd::Text { .. } => "text",
            DrawCmd::Image { .. } => "image",
            DrawCmd::PushClip { .. } => "push-clip",
            DrawCmd::PopClip => "pop-clip",
            DrawCmd::PushOpacity(_) => "push-opacity",
            DrawCmd::PopOpacity => "pop-opacity",
            DrawCmd::FillPath { .. } => "fill-path",
            DrawCmd::BorderPath { .. } => "border-path",
            DrawCmd::ShadowPath { .. } => "shadow-path",
            DrawCmd::PushClipPath { .. } => "push-clip-path",
            DrawCmd::Svg { .. } => "svg",
        })
        .collect();
    assert_eq!(kinds, ["fill", "fill", "shadow", "fill", "border", "text"]);
    // Bounds: a leaf's shape plus 1 px; a shadow's three sigma plus 1 px.
    let card = items.iter().find(|i| matches!(&i.cmd, DrawCmd::Border { .. })).unwrap();
    let r = rect_of(card).unwrap();
    assert_eq!(card.bounds, r.outset(1.0));
    let sh = items.iter().find(|i| matches!(&i.cmd, DrawCmd::Shadow { .. })).unwrap();
    let DrawCmd::Shadow { rect, blur, .. } = &sh.cmd else { unreachable!() };
    assert_eq!(sh.bounds, rsb_core::draw::shadow_bounds(*rect, *blur));
    assert_eq!(*rect, RectF::new(r.x, r.y + 1.0, r.w, r.h));
    // Text bounds are exactly its rect.
    let t = by_text(&items, "hi");
    assert_eq!(Some(t.bounds), rect_of(t));
}

#[test]
fn a_transparent_bar_still_paints_every_pixel_of_its_box() {
    let mut s = scene_with("bar { background: transparent }", 300, 1.0, Some(text("a")), None, None);
    let items = drawn(&mut s);
    assert!(matches!(&items[0].cmd, DrawCmd::Fill { rect, .. } if rect.w == 300.0 && rect.h == 32.0));
    // A theme cannot remove it: not even display: none on the bar.
    let mut s = scene_with("bar { background: none; opacity: 0 }", 300, 1.0, Some(text("a")), None, None);
    let items = drawn(&mut s);
    assert!(matches!(&items[0].cmd, DrawCmd::Fill { .. }));
}

#[test]
fn a_floating_bar_keeps_its_margin_transparent() {
    let mut b = bar();
    b.floating = true;
    b.height = 32;
    b.margin = 8;
    b.radius = 10;
    let g = rsb_core::scene::BarGeometry { width_px: 400, height_px: 48, scale: 1.0 };
    let mut s = rsb_core::scene::Scene::new(&b, g, env(true));
    s.set_stylesheet(sheet("bar { background: #333333 }"));
    let items = drawn(&mut s);
    match &items[0].cmd {
        DrawCmd::Fill { rect, radii, .. } => {
            // Bar height 32 inside a 48 px surface with an 8 px margin all round.
            assert_eq!(*rect, RectF::new(8.0, 8.0, 384.0, 32.0));
            assert_eq!(radii.tl, 10.0);
        }
        other => panic!("{other:?}"),
    }
    let bars = s.nodes();
    assert!(bars[0].classes.iter().any(|c| c.as_ref() == "floating"));
}

#[test]
fn opacity_wraps_the_node_and_overflow_clips_its_children() {
    let css = "bar { font-size: 10px } .o { opacity: 0.5; background: #ff0000 } .c { overflow: hidden; width: 20px; background: #0000ff }";
    let o = classed(boxed(vec![text("a")]), &["o"]);
    let c = classed(boxed(vec![text("abcdef")]), &["c"]);
    let mut s = scene_with(css, 300, 1.0, Some(o), Some(c), None);
    let items = drawn(&mut s);
    let push = items.iter().position(|i| matches!(i.cmd, DrawCmd::PushOpacity(_))).unwrap();
    let pop = items.iter().position(|i| matches!(i.cmd, DrawCmd::PopOpacity)).unwrap();
    assert!(push < pop);
    assert!(matches!(items[push].cmd, DrawCmd::PushOpacity(v) if v == 0.5));
    // The pair's bounds cover what is inside, and a group's items share one key.
    assert_eq!(items[push].node, items[pop].node);
    assert!(items[push].bounds.w >= 6.0);
    let cpush = items.iter().position(|i| matches!(i.cmd, DrawCmd::PushClip { .. })).unwrap();
    let cpop = items.iter().position(|i| matches!(i.cmd, DrawCmd::PopClip)).unwrap();
    assert!(cpush < cpop);
    // Items between are the clip's children.
    assert!(items[cpush + 1..cpop].iter().any(|i| matches!(&i.cmd, DrawCmd::Text { .. })));
}

#[test]
fn display_none_and_visibility_hidden() {
    let css = "bar { font-size: 10px } .gone { display: none } .ghost { visibility: hidden; background: #fff }";
    let kids = vec![classed(text("gone"), &["gone"]), classed(text("ghost"), &["ghost"]), text("seen")];
    let mut s = scene_with(css, 300, 1.0, Some(boxed(kids)), None, None);
    let items = drawn(&mut s);
    assert_eq!(texts(&items), ["seen"]);
    // The hidden-but-visible-less box still takes its space; display: none does not.
    frame(&mut s, Instant::now());
    let ghost = node_by_class(&s, "ghost");
    let seen = s.nodes().into_iter().find(|n| n.text.as_deref() == Some("seen")).unwrap();
    assert_eq!(ghost.rect.w, 30.0);
    assert_eq!(seen.rect.x, 30.0);
    let gone = node_by_class(&s, "gone");
    assert_eq!(gone.rect.w, 0.0);
}

#[test]
fn an_empty_widget_takes_no_space() {
    let mut b = bar();
    b.height = 32;
    let mut s = rsb_core::scene::Scene::new(
        &b,
        rsb_core::scene::BarGeometry { width_px: 200, height_px: 32, scale: 1.0 },
        env(true),
    );
    s.set_stylesheet(sheet("bar { font-size: 10px } zone { gap: 10px }"));
    let slots =
        rsb_core::scene::ZoneSlots { left: vec![slot(1, "a"), slot(2, "b"), slot(3, "c")], ..Default::default() };
    s.set_slots(slots);
    s.set_tree(tree(1, 1, Some(text("aa"))));
    s.set_tree(tree(2, 1, None));
    s.set_tree(tree(3, 1, Some(text("cc"))));
    frame(&mut s, Instant::now());
    let a = node_by_class(&s, "a");
    let c = node_by_class(&s, "c");
    assert_eq!(a.rect.w, 12.0);
    assert_eq!(c.rect.x, 12.0 + 10.0, "one gap, not two");
    assert!(s.nodes().iter().all(|n| !n.classes.iter().any(|c| c.as_ref() == "b")));
}

#[test]
fn flex_row_column_gap_and_alignment() {
    let css = "bar { font-size: 10px } .col { flex-direction: column; gap: 4px } .row { gap: 2px; justify-content: space-between; width: 100px }";
    let col = classed(boxed(vec![text("a"), text("b")]), &["col"]);
    let row = classed(boxed(vec![text("a"), text("b")]), &["row"]);
    let mut s = scene_with(css, 300, 1.0, Some(col), Some(row), None);
    frame(&mut s, Instant::now());
    let col = node_by_class(&s, "col");
    assert_eq!((col.rect.w, col.rect.h), (6.0, 29.0));
    let row = node_by_class(&s, "row");
    assert_eq!(row.rect.w, 100.0);
    let kids: Vec<NodeInfo> =
        s.nodes().into_iter().filter(|n| n.kind == ElementKind::Text && n.instance == row.instance).collect();
    assert_eq!(kids[0].rect.x, row.rect.x);
    assert_eq!(kids[1].rect.right(), row.rect.right());
}

#[test]
fn a_tree_deeper_than_the_cap_becomes_a_plate() {
    fn nest(depth: usize) -> Element {
        let mut e = text("deep");
        for _ in 1..depth {
            e = boxed(vec![e]);
        }
        e
    }
    let ok = nest(rsb_core::element::MAX_DEPTH);
    assert_eq!(ok.depth(), rsb_core::element::MAX_DEPTH);
    let mut s = scene_with(BASE, 400, 1.0, Some(ok), None, None);
    let items = drawn(&mut s);
    assert_eq!(texts(&items), ["deep"]);
    assert!(s.take_diagnostics().is_empty());

    let too_deep = nest(rsb_core::element::MAX_DEPTH + 1);
    let mut s = scene_with(BASE, 400, 1.0, Some(too_deep), None, None);
    let items = drawn(&mut s);
    let shown = texts(&items);
    assert_eq!(shown.len(), 1);
    assert!(shown[0].starts_with("STOPPED - w has more than 64 nested elements"), "{shown:?}");
    assert!(s.nodes().iter().any(|n| n.classes.iter().any(|c| c.as_ref() == "stopped")));
    let d = s.take_diagnostics();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].instance, Some(rsb_core::element::InstanceId(1)));
}

#[test]
fn duplicate_keys_warn_once_and_fall_back_to_position() {
    let mut a = text("a");
    a.key = Some(Arc::from("k"));
    let mut b = text("b");
    b.key = Some(Arc::from("k"));
    let mut c = text("c");
    c.key = Some(Arc::from("other"));
    let mut s = scene_with(BASE, 400, 1.0, Some(boxed(vec![a.clone(), b.clone(), c.clone()])), None, None);
    frame(&mut s, Instant::now());
    let d = s.take_diagnostics();
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].diagnostic.message.contains("duplicate key 'k'"));
    // Nodes keep distinct keys.
    let keys: Vec<_> = s.nodes().iter().map(|n| n.key).collect();
    let mut uniq = keys.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), keys.len());
    // The same tree again does not warn again.
    s.set_tree(tree(1, 1, Some(boxed(vec![a, b, c]))));
    frame(&mut s, Instant::now());
    assert!(s.take_diagnostics().is_empty());
}

#[test]
fn node_keys_are_stable_and_follow_explicit_keys_not_position() {
    let mk = |order: &[&str]| {
        let kids = order
            .iter()
            .map(|k| {
                let mut e = text(k);
                e.key = Some(Arc::from(*k));
                e
            })
            .collect();
        boxed(kids)
    };
    let mut s = scene_with(BASE, 400, 1.0, Some(mk(&["a", "b", "c"])), None, None);
    frame(&mut s, Instant::now());
    let key_of =
        |s: &rsb_core::scene::Scene, t: &str| s.nodes().into_iter().find(|n| n.text.as_deref() == Some(t)).unwrap().key;
    let before: Vec<_> = ["a", "b", "c"].iter().map(|t| key_of(&s, t)).collect();
    s.set_tree(tree(1, 2, Some(mk(&["c", "a", "b"]))));
    frame(&mut s, Instant::now());
    let after: Vec<_> = ["a", "b", "c"].iter().map(|t| key_of(&s, t)).collect();
    assert_eq!(before, after, "a reordered keyed list keeps each item's identity");
    // Without keys the identity is kind plus index.
    let mut s1 = scene_with(BASE, 400, 1.0, Some(boxed(vec![text("x"), text("y")])), None, None);
    let mut s2 = scene_with(BASE, 400, 1.0, Some(boxed(vec![text("p"), text("q")])), None, None);
    frame(&mut s1, Instant::now());
    frame(&mut s2, Instant::now());
    let k1: Vec<_> = s1.nodes().iter().map(|n| n.key).collect();
    let k2: Vec<_> = s2.nodes().iter().map(|n| n.key).collect();
    assert_eq!(k1, k2);
}

#[test]
fn plates_take_colours_from_plate_rules_and_ignore_the_rest() {
    let css = "plate { color: #fefefe; background: #123456; border: 1px solid #abcdef; font-family: Inter; \
               display: none; visibility: hidden; opacity: 0; font-size: 3px; padding: 50px; width: 1px }";
    let mut s = scene_with(css, 600, 1.0, None, None, None);
    s.set_bar_plate(Some(rsb_core::diag::Plate {
        kind: rsb_core::diag::DiagKind::Style,
        text: "RSB ERROR - style.css:1:1 x".into(),
        more: 1,
    }));
    let items = drawn(&mut s);
    // Drawn despite display / visibility / opacity.
    let t = items
        .iter()
        .find_map(|i| match &i.cmd {
            DrawCmd::Text { text, style, color, .. } => Some((text.to_string(), style.clone(), *color)),
            _ => None,
        })
        .expect("the plate text is drawn");
    assert_eq!(t.0, "RSB ERROR - style.css:1:1 x (+1 more, see log)");
    assert_eq!(&*t.1.families[0], "Inter", "the font family is themable");
    assert!(t.1.size_px >= 9.0, "size {}", t.1.size_px);
    assert_eq!((t.2.r * 255.0).round(), 254.0, "the colour is themable");
    assert!(
        items
            .iter()
            .any(|i| matches!(&i.cmd, DrawCmd::Fill { fill: Fill::Solid(c), .. } if (c.b * 255.0).round() == 86.0)),
        "background from the rule"
    );
    assert!(items.iter().any(|i| matches!(i.cmd, DrawCmd::Border { .. })));
    assert!(!items.iter().any(|i| matches!(i.cmd, DrawCmd::PushOpacity(_))));
    // Padding and width are the engine's too: nowhere near 50 px of padding.
    let plate = s.nodes().into_iter().find(|n| n.kind == ElementKind::Plate).unwrap();
    assert!(plate.rect.h < 40.0, "{:?}", plate.rect);
    // Without rules a plate has the engine's red look.
    let mut s = scene_with("", 600, 1.0, None, None, None);
    s.set_bar_plate(Some(rsb_core::diag::Plate {
        kind: rsb_core::diag::DiagKind::Config,
        text: "RSB ERROR - x".into(),
        more: 0,
    }));
    let items = drawn(&mut s);
    assert!(
        items.iter().any(|i| matches!(&i.cmd, DrawCmd::Fill { fill: Fill::Solid(c), .. } if c.r > 0.5 && c.g < 0.2))
    );
    let plate = s.nodes().into_iter().find(|n| n.kind == ElementKind::Plate).unwrap();
    // The bar plate is the first child of the left zone.
    let left = node_by_id(&s, "left");
    assert!(plate.rect.x >= left.rect.x && plate.rect.right() <= left.rect.right() + 0.5);
    assert_eq!(plate.depth, left.depth + 1);
}

#[test]
fn a_widget_plate_replaces_the_widget_and_marks_it_stopped() {
    let mut s = scene_with(BASE, 400, 1.0, Some(text("fine")), None, None);
    s.set_widget_plate(
        rsb_core::element::InstanceId(1),
        Some(rsb_core::diag::Plate {
            kind: rsb_core::diag::DiagKind::Stopped,
            text: "STOPPED - w ran over 10 ms".into(),
            more: 0,
        }),
    );
    let items = drawn(&mut s);
    assert_eq!(texts(&items), ["STOPPED - w ran over 10 ms"]);
    let w = node_by_class(&s, "l");
    assert!(w.classes.iter().any(|c| c.as_ref() == "stopped"));
    s.set_widget_plate(rsb_core::element::InstanceId(1), None);
    let items = drawn(&mut s);
    assert_eq!(texts(&items), ["fine"]);
}

#[test]
fn engine_classes_on_the_bar() {
    let mut s = scene_with(
        "bar.main { background: #00ff00 } bar.fullscreen { background: #ff0000 }",
        100,
        1.0,
        Some(text("a")),
        None,
        None,
    );
    let bar_classes =
        |s: &rsb_core::scene::Scene| s.nodes()[0].classes.iter().map(|c| c.to_string()).collect::<Vec<_>>();
    frame(&mut s, Instant::now());
    assert_eq!(bar_classes(&s), ["main"]);
    let mut e = env(true);
    e.fullscreen = true;
    s.set_env(e);
    let items = drawn(&mut s);
    assert_eq!(bar_classes(&s), ["main", "fullscreen"]);
    assert!(items.iter().any(|i| matches!(&i.cmd, DrawCmd::Fill { fill: Fill::Solid(c), .. } if c.r > 0.9)));
}

#[test]
fn bar_settings_are_the_initial_values() {
    let mut b = bar();
    b.radius = 9;
    let mut s = rsb_core::scene::Scene::new(
        &b,
        rsb_core::scene::BarGeometry { width_px: 100, height_px: 32, scale: 1.0 },
        env(true),
    );
    s.set_stylesheet(sheet("bar { background: #222222 }"));
    let items = drawn(&mut s);
    assert!(matches!(&items[0].cmd, DrawCmd::Fill { radii, .. } if radii.tl == 9.0));
    // CSS border-radius wins over the setting.
    s.set_stylesheet(sheet("bar { background: #222222; border-radius: 3px }"));
    let items = drawn(&mut s);
    assert!(matches!(&items[0].cmd, DrawCmd::Fill { radii, .. } if radii.tl == 3.0));
}

#[test]
fn text_style_scales_and_the_icon_font_is_the_icon_family() {
    let mut icon = Element::new(ElementKind::Icon);
    icon.icon = Some(rsb_core::element::IconRef::Named(Arc::from("cpu")));
    let mut s = scene_with(
        "bar { font-size: 10px; letter-spacing: 1px; font-variant-numeric: tabular-nums }",
        400,
        2.0,
        Some(boxed(vec![icon, text("t")])),
        None,
        None,
    );
    let items = drawn(&mut s);
    let icon_item = items
        .iter()
        .find_map(|i| match &i.cmd {
            DrawCmd::Text { text, style, .. } if text.chars().next().is_some_and(|c| (c as u32) >= 0xE000) => {
                Some(style.clone())
            }
            _ => None,
        })
        .expect("an icon glyph is drawn");
    assert_eq!(&*icon_item.families[0], "Segoe Fluent Icons");
    assert_eq!(icon_item.size_px, 20.0);
    assert_eq!(icon_item.letter_spacing, 2.0);
    assert!(icon_item.tabular_nums);
}

#[test]
fn the_text_measurer_is_asked_once_per_text() {
    use rsb_core::draw::TextStyle;
    use rsb_core::geom::SizeF;
    use rsb_core::layout::TextMeasure;
    struct Counting(usize);
    impl TextMeasure for Counting {
        fn measure(&mut self, text: &str, style: &TextStyle, _: Option<f32>) -> SizeF {
            self.0 += 1;
            SizeF { w: text.chars().count() as f32 * 0.6 * style.size_px, h: 1.25 * style.size_px }
        }
    }
    let mut s = scene_with(BASE, 400, 1.0, Some(text("aaaa")), Some(text("bb")), None);
    let mut m = Counting(0);
    s.frame(Instant::now(), &mut m);
    let first = m.0;
    assert!(first >= 2);
    // A new tree with the same texts and styles lays out again without measuring again.
    s.set_tree(tree(1, 2, Some(text("aaaa"))));
    s.frame(Instant::now(), &mut m);
    assert_eq!(m.0, first, "sizes are cached per node while text and style stay the same");
    // A different text is measured.
    s.set_tree(tree(1, 3, Some(text("aaaaa"))));
    s.frame(Instant::now(), &mut m);
    assert_eq!(m.0, first + 1);
}

#[test]
fn a_plate_cannot_be_made_invisible_by_colour_or_border() {
    let mut s = scene_with("plate { color: transparent; border: 80px solid #ffffff }", 600, 1.0, None, None, None);
    s.set_bar_plate(Some(rsb_core::diag::Plate {
        kind: rsb_core::diag::DiagKind::Config,
        text: "RSB ERROR - x".into(),
        more: 0,
    }));
    let items = drawn(&mut s);
    let text_colour = items.iter().find_map(|i| match &i.cmd {
        DrawCmd::Text { color, .. } => Some(*color),
        _ => None,
    });
    assert_eq!(text_colour.map(|c| c.a), Some(1.0));
    let border = items.iter().find_map(|i| match &i.cmd {
        DrawCmd::Border { widths, .. } => Some(*widths),
        _ => None,
    });
    assert!(border.is_some_and(|w| w.top <= 3.0 && w.left <= 3.0), "{border:?}");
}

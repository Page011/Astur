//! Outline and SVG helpers and scenes for the renderer tests.
#![allow(dead_code)]

use std::sync::Arc;

use rsb_core::color::Color;
use rsb_core::draw::{DrawCmd, DrawItem, Fill, shadow_bounds};
use rsb_core::geom::{PathEl, PathF, PointF, RectF};
use rsb_core::style::TextAlign;

use super::*;

pub fn pts(points: &[(f32, f32)]) -> Vec<PointF> {
    points.iter().map(|&(x, y)| PointF { x, y }).collect()
}

pub fn poly(points: &[(f32, f32)]) -> Arc<PathF> {
    Arc::new(PathF::polygon(&pts(points)))
}

/// A rectangle with its four corners cut at 45 degrees by `cut`.
pub fn bevel_points(r: RectF, cut: f32) -> Vec<(f32, f32)> {
    let (l, t, rt, b) = (r.x, r.y, r.right(), r.bottom());
    vec![
        (l + cut, t),
        (rt - cut, t),
        (rt, t + cut),
        (rt, b - cut),
        (rt - cut, b),
        (l + cut, b),
        (l, b - cut),
        (l, t + cut),
    ]
}

pub fn fill_path(n: u64, path: Arc<PathF>, color: Color) -> DrawItem {
    let bounds = path.bounds().outset(1.0);
    item(n, bounds, DrawCmd::FillPath { path, fill: Fill::Solid(color) })
}

pub fn fill_path_gradient(
    n: u64,
    path: Arc<PathF>,
    start: (f32, f32),
    end: (f32, f32),
    stops: &[(f32, &str)],
) -> DrawItem {
    let bounds = path.bounds().outset(1.0);
    let stops = stops.iter().map(|(p, c)| (*p, col(c))).collect();
    let fill = Fill::Linear { start: PointF { x: start.0, y: start.1 }, end: PointF { x: end.0, y: end.1 }, stops };
    item(n, bounds, DrawCmd::FillPath { path, fill })
}

pub fn border_path(n: u64, outer: Arc<PathF>, inner: Arc<PathF>, color: Color) -> DrawItem {
    let bounds = outer.bounds().outset(1.0);
    item(n, bounds, DrawCmd::BorderPath { outer, inner, color })
}

pub fn shadow_path(n: u64, path: Arc<PathF>, blur: f32, color: Color) -> DrawItem {
    let bounds = shadow_bounds(path.bounds(), blur);
    item(n, bounds, DrawCmd::ShadowPath { path, blur, color })
}

pub fn push_clip_path(n: u64, path: Arc<PathF>) -> DrawItem {
    let bounds = path.bounds().outset(1.0);
    item(n, bounds, DrawCmd::PushClipPath { path, even_odd: false })
}

pub fn svg_item(n: u64, r: RectF, key: &str, tint: Option<Color>) -> DrawItem {
    item(n, r.outset(1.0), DrawCmd::Svg { rect: r, key: key.into(), tint })
}

/// Bar segment shapes: which ends take a point, a notch or stay flat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seg {
    /// point left, point right
    Hex,
    /// notch left, point right
    NotchPoint,
    /// point left, notch right
    PointNotch,
    /// point left, flat right
    PointFlat,
}

/// Outline of a segment at (x, y), `w` wide and `h` tall, chevron depth `d`.
pub fn seg_points(kind: Seg, x: f32, y: f32, w: f32, h: f32, d: f32) -> Vec<(f32, f32)> {
    let m = h / 2.0;
    let p: Vec<(f32, f32)> = match kind {
        Seg::Hex => vec![(d, 0.0), (w - d, 0.0), (w, m), (w - d, h), (d, h), (0.0, m)],
        Seg::NotchPoint => vec![(0.0, 0.0), (w - d, 0.0), (w, m), (w - d, h), (0.0, h), (d, m)],
        Seg::PointNotch => vec![(d, 0.0), (w, 0.0), (w - d, m), (w, h), (d, h), (0.0, m)],
        Seg::PointFlat => vec![(d, 0.0), (w, 0.0), (w, h), (d, h), (0.0, m)],
    };
    p.into_iter().map(|(px, py)| (x + px, y + py)).collect()
}

/// One bar segment as the scene paints it: an outline clip, the box fill, the
/// end of the clip. Uses nodes `n` and `n + 1`.
#[allow(clippy::too_many_arguments)]
pub fn segment(n: u64, kind: Seg, x: f32, y: f32, w: f32, h: f32, d: f32, color: Color) -> Vec<DrawItem> {
    let path = poly(&seg_points(kind, x, y, w, h, d));
    let hull = path.bounds().outset(1.0);
    vec![push_clip_path(n, path), fill(n + 1, rect(x, y, w, h), 0.0, color), pop_clip(n, hull)]
}

pub fn scene_paths() -> Scene {
    let ink = col("#366382");
    let amber = col("#E79B25");
    let plum = col("#7D5168");
    let slate = col("#394855");
    let blob = {
        // a closed shape with quadratic and cubic edges
        let p = |x: f32, y: f32| PointF { x, y };
        Arc::new(PathF {
            els: vec![
                PathEl::MoveTo(p(100.0, 40.0)),
                PathEl::CubicTo(p(100.0, 10.0), p(150.0, 10.0), p(150.0, 40.0)),
                PathEl::QuadTo(p(150.0, 62.0), p(125.0, 64.0)),
                PathEl::LineTo(p(105.0, 56.0)),
                PathEl::Close,
            ],
        })
    };
    let star = poly(&[(240.0, 8.0), (254.0, 56.0), (214.0, 26.0), (266.0, 26.0), (226.0, 56.0)]);
    let bevel_outer = rect(10.0, 80.0, 110.0, 54.0);
    let bevel_shadow = rect(150.0, 84.0, 90.0, 40.0);
    let hexagon = poly(&[(290.0, 84.0), (330.0, 84.0), (350.0, 110.0), (330.0, 136.0), (290.0, 136.0), (270.0, 110.0)]);
    let g = rect(268.0, 82.0, 84.0, 56.0);
    let inner_cut = 14.0 - 3.0 * (2.0 - 2f32.sqrt());
    let items = vec![
        background(400, 200, "#f4f6f8"),
        fill_path(1, poly(&[(10.0, 10.0), (70.0, 10.0), (40.0, 62.0)]), ink),
        fill_path(2, blob, amber),
        fill_path_gradient(
            3,
            poly(&[(180.0, 10.0), (205.0, 10.0), (210.0, 40.0), (192.5, 62.0), (175.0, 40.0)]),
            (175.0, 10.0),
            (210.0, 62.0),
            &[(0.0, "#366382"), (1.0, "#E79B25")],
        ),
        fill_path(4, star, plum),
        fill_path(5, poly(&bevel_points(bevel_outer, 14.0)), slate),
        border_path(
            6,
            poly(&bevel_points(bevel_outer, 14.0)),
            poly(&bevel_points(rect(13.0, 83.0, 104.0, 48.0), inner_cut)),
            amber,
        ),
        shadow_path(7, poly(&bevel_points(bevel_shadow, 10.0)), 8.0, col("#00000066")),
        fill_path(8, poly(&bevel_points(bevel_shadow, 10.0)), col("#ffffff")),
        push_clip_path(9, hexagon),
        fill_path_gradient(
            10,
            poly(&[(260.0, 80.0), (360.0, 80.0), (360.0, 140.0), (260.0, 140.0)]),
            (260.0, 80.0),
            (360.0, 140.0),
            &[(0.0, "#366382"), (1.0, "#E79B25")],
        ),
        fill(11, rect(300.0, 100.0, 40.0, 40.0), 20.0, plum),
        pop_clip(9, g.outset(1.0)),
        fill_path(12, poly(&[(20.0, 150.0), (120.0, 150.0), (120.0, 190.0), (20.0, 190.0)]), col("#36638299")),
        fill_path(13, poly(&[(70.0, 160.0), (170.0, 160.0), (170.0, 200.0), (70.0, 200.0)]), col("#E79B2599")),
        // reaches past the left and bottom edges of the surface
        fill_path(14, poly(&[(-20.0, 170.0), (30.0, 150.0), (60.0, 230.0), (-10.0, 230.0)]), slate),
        // a thin sliver
        fill_path(15, poly(&[(200.0, 150.0), (380.0, 150.5), (200.0, 151.0)]), plum),
        push_clip(16, rect(210.0, 160.0, 160.0, 30.0), 10.0),
        push_clip_path(17, poly(&[(220.0, 160.0), (370.0, 160.0), (350.0, 190.0), (230.0, 190.0)])),
        fill(18, rect(200.0, 150.0, 200.0, 50.0), 0.0, amber),
        pop_clip(17, rect(219.0, 159.0, 152.0, 32.0)),
        pop_clip(16, rect(209.0, 159.0, 162.0, 32.0)),
    ];
    Scene { name: "paths", list: list(400, 200, items), text: false, images: vec![], svgs: vec![] }
}

/// Bar-like rows over white: both joint constructions, and a bevelled button
/// with a shadow.
pub fn scene_chevrons() -> Scene {
    let (h, d) = (26.0, 13.0);
    let mut items = vec![background(420, 130, "#ffffff")];
    let c = [col("#2f3639"), col("#1b1f22"), col("#090f12"), col("#366382")];
    // `>` joints: the earlier segment runs 2 px under the next one
    items.extend(segment(1, Seg::Hex, 10.0, 10.0, 100.0 + 2.0, h, d, c[0]));
    items.extend(segment(10, Seg::NotchPoint, 97.0, 10.0, 90.0 + 2.0, h, d, c[1]));
    items.extend(segment(20, Seg::NotchPoint, 174.0, 10.0, 80.0, h, d, c[2]));
    // `<` joints: the earlier segment is flat on its right
    items.extend(segment(30, Seg::PointFlat, 10.0, 50.0, 100.0, h, d, c[1]));
    items.extend(segment(40, Seg::Hex, 97.0, 50.0, 90.0, h, d, c[0]));
    items.extend(segment(50, Seg::PointNotch, 174.0, 50.0, 80.0, h, d, c[3]));
    items.extend(segment(60, Seg::PointNotch, 10.0, 90.0, 100.0, h, d, c[0]));
    items.extend(segment(70, Seg::Hex, 97.0, 90.0, 90.0, h, d, c[1]));
    let button = poly(&bevel_points(rect(280.0, 12.0, 120.0, 40.0), 12.0));
    items.push(shadow_path(80, button.clone(), 6.0, col("#0000004d")));
    items.push(fill_path(81, button, col("#1b1f22")));
    Scene { name: "chevrons", list: list(420, 130, items), text: false, images: vec![], svgs: vec![] }
}

pub const SVG_BOLT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M13 3 6 14h5l-1 7 7-11h-5z"/></svg>"#;

pub const SVG_DIAL: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></svg>"#;

/// An image with its own colours and a gradient, not square.
pub const SVG_PICTURE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="32" viewBox="0 0 48 32"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#366382"/><stop offset="1" stop-color="#E79B25"/></linearGradient></defs><rect width="48" height="32" rx="4" fill="url(#g)"/><circle cx="14" cy="12" r="6" fill="#ffffff"/><path d="M4 28 18 16 30 24 38 18 44 28z" fill="#7D5168"/></svg>"##;

pub fn scene_svg() -> Scene {
    let ink = col("#366382");
    let items = vec![
        background(400, 150, "#ffffff"),
        svg_item(1, rect(10.0, 10.0, 16.0, 16.0), "bolt", Some(ink)),
        svg_item(2, rect(40.0, 10.0, 42.0, 42.0), "bolt", Some(col("#E79B25"))),
        svg_item(3, rect(100.0, 10.0, 64.0, 64.0), "bolt", Some(col("#7D5168"))),
        svg_item(4, rect(180.0, 10.0, 96.0, 64.0), "picture", None),
        svg_item(5, rect(290.0, 10.0, 96.0, 64.0), "picture", Some(col("#394855"))),
        svg_item(6, rect(10.4, 90.3, 40.0, 40.0), "dial", Some(col("#36638280"))),
        svg_item(7, rect(70.0, 90.0, 60.0, 40.0), "dial", Some(ink)),
        push_clip(8, rect(150.0, 90.0, 60.0, 40.0), 14.0),
        svg_item(9, rect(140.0, 80.0, 80.0, 60.0), "picture", None),
        pop_clip(8, rect(149.0, 89.0, 62.0, 42.0)),
        svg_item(10, rect(250.0, 90.0, 30.0, 40.0), "missing", Some(ink)),
    ];
    Scene {
        name: "svg",
        list: list(400, 150, items),
        text: false,
        images: vec![],
        svgs: vec![("bolt", SVG_BOLT), ("dial", SVG_DIAL), ("picture", SVG_PICTURE)],
    }
}

/// The bundled monospace font at two weights and two sizes (registered by
/// `install_images` for this scene only).
pub fn scene_mono() -> Scene {
    let mono = ["JetBrains Mono", "monospace"];
    let fg = col("#e0e5e8");
    let accent = col("#81cbea");
    let items = vec![
        background(360, 120, "#1b1f22"),
        text(
            1,
            rect(10.0, 8.0, 340.0, 20.0),
            "Memory 42%   CPU 07%   12:34",
            &style(&mono, 14.0, 400),
            fg,
            TextAlign::Start,
        ),
        text(
            2,
            rect(10.0, 32.0, 340.0, 20.0),
            "Memory 42%   CPU 07%   12:34",
            &style(&mono, 14.0, 700),
            fg,
            TextAlign::Start,
        ),
        text(
            3,
            rect(10.0, 56.0, 340.0, 20.0),
            "Wednesday 25 November, 2026",
            &style(&mono, 14.0, 400),
            accent,
            TextAlign::Start,
        ),
        text(4, rect(10.0, 80.0, 340.0, 32.0), "Aa 0123 -> !=", &style(&mono, 24.0, 500), fg, TextAlign::Start),
    ];
    Scene { name: "mono", list: list(360, 120, items), text: false, images: vec![], svgs: vec![] }
}

/// The bundled font file.
pub fn bundled_mono() -> Arc<[u8]> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("assets")
        .join("fonts")
        .join("JetBrainsMono-wght.ttf");
    std::fs::read(path).expect("the bundled font is in the tree").into()
}

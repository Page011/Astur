//! Shapes: the affine a `transform` resolves to, outlines with bevelled
//! corners, `clip-path` polygons with vertices on whole device pixels, and
//! the flat edges an attached popup meets.

use std::sync::Arc;

use crate::draw::{DrawCmd, DrawItem, Fill};
use crate::geom::{Edges, PathEl, PathF, PointF, Radii, RectF};
use crate::popup::FlatEdges;
use crate::style::{ClipPolygon, ComputedStyle, CornerShape, Transform2};

/// `p' = (sx * x + tx, sy * y + ty)`: a scale and a translation, which is
/// all `translate()` and `scale()` can make.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Affine {
    pub sx: f32,
    pub sy: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Default for Affine {
    fn default() -> Self {
        Affine::IDENTITY
    }
}

impl Affine {
    pub const IDENTITY: Affine = Affine { sx: 1.0, sy: 1.0, tx: 0.0, ty: 0.0 };

    pub fn is_identity(&self) -> bool {
        *self == Affine::IDENTITY
    }

    pub fn point(&self, p: PointF) -> PointF {
        PointF { x: self.sx * p.x + self.tx, y: self.sy * p.y + self.ty }
    }

    /// None when a scale is 0: nothing is hit through a flattened element.
    pub fn inverse_point(&self, p: PointF) -> Option<PointF> {
        if self.sx.abs() < 1e-6 || self.sy.abs() < 1e-6 {
            return None;
        }
        Some(PointF { x: (p.x - self.tx) / self.sx, y: (p.y - self.ty) / self.sy })
    }

    pub fn rect(&self, r: &RectF) -> RectF {
        let (a, b) = (self.point(PointF { x: r.x, y: r.y }), self.point(PointF { x: r.right(), y: r.bottom() }));
        RectF::new(a.x.min(b.x), a.y.min(b.y), (b.x - a.x).abs(), (b.y - a.y).abs())
    }

    /// `self` applied after `inner`.
    pub fn after(&self, inner: &Affine) -> Affine {
        Affine {
            sx: self.sx * inner.sx,
            sy: self.sy * inner.sy,
            tx: self.sx * inner.tx + self.tx,
            ty: self.sy * inner.ty + self.ty,
        }
    }

    fn mean_scale(&self) -> f32 {
        (self.sx.abs() * self.sy.abs()).sqrt()
    }

    fn path(&self, p: &PathF) -> PathF {
        let m = |q: PointF| self.point(q);
        PathF {
            els: p
                .els
                .iter()
                .map(|e| match *e {
                    PathEl::MoveTo(a) => PathEl::MoveTo(m(a)),
                    PathEl::LineTo(a) => PathEl::LineTo(m(a)),
                    PathEl::QuadTo(a, b) => PathEl::QuadTo(m(a), m(b)),
                    PathEl::CubicTo(a, b, c) => PathEl::CubicTo(m(a), m(b), m(c)),
                    PathEl::Close => PathEl::Close,
                })
                .collect(),
        }
    }

    /// The item as drawn after this transform: rects, radii, border widths,
    /// shadow offsets and blur, clip rects and paths, text size.
    pub fn item(&self, mut it: DrawItem) -> DrawItem {
        if self.is_identity() {
            return it;
        }
        let k = self.mean_scale();
        let r = |rect: &RectF| self.rect(rect);
        let radii = |q: Radii| Radii { tl: q.tl * k, tr: q.tr * k, br: q.br * k, bl: q.bl * k };
        it.bounds = r(&it.bounds);
        it.cmd = match it.cmd {
            DrawCmd::Shadow { rect, radii: q, blur, color } => {
                DrawCmd::Shadow { rect: r(&rect), radii: radii(q), blur: blur * k, color }
            }
            DrawCmd::Fill { rect, radii: q, fill } => {
                DrawCmd::Fill { rect: r(&rect), radii: radii(q), fill: self.fill(fill) }
            }
            DrawCmd::Border { rect, radii: q, widths, color } => DrawCmd::Border {
                rect: r(&rect),
                radii: radii(q),
                widths: Edges {
                    top: widths.top * self.sy.abs(),
                    right: widths.right * self.sx.abs(),
                    bottom: widths.bottom * self.sy.abs(),
                    left: widths.left * self.sx.abs(),
                },
                color,
            },
            DrawCmd::Text { rect, text, mut style, color, align } => {
                style.size_px *= self.sy.abs();
                style.letter_spacing *= self.sx.abs();
                DrawCmd::Text { rect: r(&rect), text, style, color, align }
            }
            DrawCmd::Image { rect, key, radii: q } => DrawCmd::Image { rect: r(&rect), key, radii: radii(q) },
            DrawCmd::PushClip { rect, radii: q } => DrawCmd::PushClip { rect: r(&rect), radii: radii(q) },
            DrawCmd::FillPath { path, fill } => {
                DrawCmd::FillPath { path: Arc::new(self.path(&path)), fill: self.fill(fill) }
            }
            DrawCmd::BorderPath { outer, inner, color } => {
                DrawCmd::BorderPath { outer: Arc::new(self.path(&outer)), inner: Arc::new(self.path(&inner)), color }
            }
            DrawCmd::ShadowPath { path, blur, color } => {
                DrawCmd::ShadowPath { path: Arc::new(self.path(&path)), blur: blur * k, color }
            }
            DrawCmd::PushClipPath { path, even_odd } => {
                DrawCmd::PushClipPath { path: Arc::new(self.path(&path)), even_odd }
            }
            DrawCmd::Svg { rect, key, tint } => DrawCmd::Svg { rect: r(&rect), key, tint },
            other @ (DrawCmd::PopClip | DrawCmd::PushOpacity(_) | DrawCmd::PopOpacity) => other,
        };
        it
    }

    fn fill(&self, f: Fill) -> Fill {
        match f {
            Fill::Solid(c) => Fill::Solid(c),
            Fill::Linear { start, end, stops } => {
                Fill::Linear { start: self.point(start), end: self.point(end), stops }
            }
        }
    }
}

/// What `transform` and `transform-origin` make of a border box: the affine
/// in layout space (physical px).
pub(super) fn own_affine(s: &ComputedStyle, rect: &RectF, scale: f32) -> Affine {
    let t: &Transform2 = &s.transform;
    if t.is_identity() {
        return Affine::IDENTITY;
    }
    let ox = rect.x + s.transform_origin.0.resolve(rect.w, scale);
    let oy = rect.y + s.transform_origin.1.resolve(rect.h, scale);
    let (tx, ty) = (t.tx.resolve(rect.w, scale), t.ty.resolve(rect.h, scale));
    Affine { sx: t.sx, sy: t.sy, tx: ox - t.sx * ox + tx, ty: oy - t.sy * oy + ty }
}

/// Round half up: `x.5` goes to `x + 1`.
pub(crate) fn snap(v: f32) -> f32 {
    (v + 0.5).floor()
}

/// The vertices of a `clip-path` polygon in layout px, each on a whole
/// device pixel so horizontal and vertical edges are crisp.
pub(crate) fn clip_points(c: &ClipPolygon, rect: &RectF, scale: f32) -> Vec<PointF> {
    c.points
        .iter()
        .map(|(x, y)| PointF { x: snap(rect.x + x.resolve(rect.w, scale)), y: snap(rect.y + y.resolve(rect.h, scale)) })
        .collect()
}

const KAPPA: f32 = 0.552_284_8;

/// The border box outline: rounded corners as curves, bevelled corners as
/// straight cuts from `(r, 0)` to `(0, r)`.
pub(crate) fn outline_path(rect: &RectF, radii: &Radii, shapes: &[CornerShape; 4]) -> PathF {
    let (x0, y0, x1, y1) = (rect.x, rect.y, rect.right(), rect.bottom());
    let p = |x: f32, y: f32| PointF { x, y };
    let mut els = Vec::new();
    let cut = |r: f32| r.max(0.0);
    let (tl, tr, br, bl) = (cut(radii.tl), cut(radii.tr), cut(radii.br), cut(radii.bl));
    // Corner by corner, clockwise from the top left.
    let round = |s: CornerShape, r: f32| s == CornerShape::Round && r > 0.0;
    els.push(PathEl::MoveTo(p(x0 + tl, y0)));
    els.push(PathEl::LineTo(p(x1 - tr, y0)));
    if tr > 0.0 {
        if round(shapes[1], tr) {
            els.push(PathEl::CubicTo(p(x1 - tr + KAPPA * tr, y0), p(x1, y0 + tr - KAPPA * tr), p(x1, y0 + tr)));
        } else {
            els.push(PathEl::LineTo(p(x1, y0 + tr)));
        }
    }
    els.push(PathEl::LineTo(p(x1, y1 - br)));
    if br > 0.0 {
        if round(shapes[2], br) {
            els.push(PathEl::CubicTo(p(x1, y1 - br + KAPPA * br), p(x1 - br + KAPPA * br, y1), p(x1 - br, y1)));
        } else {
            els.push(PathEl::LineTo(p(x1 - br, y1)));
        }
    }
    els.push(PathEl::LineTo(p(x0 + bl, y1)));
    if bl > 0.0 {
        if round(shapes[3], bl) {
            els.push(PathEl::CubicTo(p(x0 + bl - KAPPA * bl, y1), p(x0, y1 - bl + KAPPA * bl), p(x0, y1 - bl)));
        } else {
            els.push(PathEl::LineTo(p(x0, y1 - bl)));
        }
    }
    els.push(PathEl::LineTo(p(x0, y0 + tl)));
    // A bevel here is the closing edge back to the start.
    if round(shapes[0], tl) {
        els.push(PathEl::CubicTo(p(x0, y0 + tl - KAPPA * tl), p(x0 + tl - KAPPA * tl, y0), p(x0 + tl, y0)));
    }
    els.push(PathEl::Close);
    PathF { els }
}

/// The same outline as a polygon (curves flattened to `steps` segments),
/// for hit testing and popup outlines.
pub(crate) fn outline_polygon(rect: &RectF, radii: &Radii, shapes: &[CornerShape; 4], steps: u32) -> Vec<PointF> {
    flatten(&outline_path(rect, radii, shapes), steps)
}

pub(crate) fn flatten(path: &PathF, steps: u32) -> Vec<PointF> {
    let steps = steps.max(1);
    let mut out: Vec<PointF> = Vec::new();
    let mut last = PointF::default();
    for e in &path.els {
        match *e {
            PathEl::MoveTo(a) | PathEl::LineTo(a) => {
                out.push(a);
                last = a;
            }
            PathEl::QuadTo(c, a) => {
                for i in 1..=steps {
                    let t = i as f32 / steps as f32;
                    let u = 1.0 - t;
                    out.push(PointF {
                        x: u * u * last.x + 2.0 * u * t * c.x + t * t * a.x,
                        y: u * u * last.y + 2.0 * u * t * c.y + t * t * a.y,
                    });
                }
                last = a;
            }
            PathEl::CubicTo(c1, c2, a) => {
                for i in 1..=steps {
                    let t = i as f32 / steps as f32;
                    let u = 1.0 - t;
                    out.push(PointF {
                        x: u * u * u * last.x + 3.0 * u * u * t * c1.x + 3.0 * u * t * t * c2.x + t * t * t * a.x,
                        y: u * u * u * last.y + 3.0 * u * u * t * c1.y + 3.0 * u * t * t * c2.y + t * t * t * a.y,
                    });
                }
                last = a;
            }
            PathEl::Close => {}
        }
    }
    out
}

pub(crate) use crate::geom::polygon_contains;

/// Which corner shapes need the cut triangles tested: a point of the box in
/// a bevelled corner's cut is outside.
pub(crate) fn in_bevel_cut(rect: &RectF, radii: &Radii, shapes: &[CornerShape; 4], p: PointF) -> bool {
    let (dx0, dx1, dy0, dy1) = (p.x - rect.x, rect.right() - p.x, p.y - rect.y, rect.bottom() - p.y);
    (shapes[0] == CornerShape::Bevel && radii.tl > 0.0 && dx0 + dy0 < radii.tl)
        || (shapes[1] == CornerShape::Bevel && radii.tr > 0.0 && dx1 + dy0 < radii.tr)
        || (shapes[2] == CornerShape::Bevel && radii.br > 0.0 && dx1 + dy1 < radii.br)
        || (shapes[3] == CornerShape::Bevel && radii.bl > 0.0 && dx0 + dy1 < radii.bl)
}

/// The bevelled corners' inner radii for a border: the cut line moved in
/// parallel by the border widths. A rounded corner shrinks as CSS does.
pub(crate) fn inner_corner_radii(r: Radii, shapes: &[CornerShape; 4], b: &Edges<f32>) -> Radii {
    let bevel = |r: f32, w1: f32, w2: f32| (r - (2.0 - std::f32::consts::SQRT_2) / 2.0 * (w1 + w2)).max(0.0);
    let round = |r: f32, w1: f32, w2: f32| (r - w1.max(w2)).max(0.0);
    let pick = |s: CornerShape, r: f32, w1: f32, w2: f32| {
        if s == CornerShape::Bevel { bevel(r, w1, w2) } else { round(r, w1, w2) }
    };
    Radii {
        tl: pick(shapes[0], r.tl, b.top, b.left),
        tr: pick(shapes[1], r.tr, b.top, b.right),
        br: pick(shapes[2], r.br, b.bottom, b.right),
        bl: pick(shapes[3], r.bl, b.bottom, b.left),
    }
}

/// The longest straight run of the outline along the top and the bottom of
/// `rect`, in whole device pixels. `polygon` is the snapped `clip-path`
/// polygon, when there is one; otherwise the border box, its bevelled
/// corners cut. A point (a chevron tip) or an empty box gives None.
pub(crate) fn flat_edges(
    rect: &RectF,
    polygon: Option<&[PointF]>,
    radii: &Radii,
    shapes: &[CornerShape; 4],
) -> FlatEdges {
    if rect.is_empty() {
        return FlatEdges::default();
    }
    let (top, bottom) = (snap(rect.y), snap(rect.bottom()));
    match polygon {
        Some(pts) => FlatEdges { top: longest_run(pts, top), bottom: longest_run(pts, bottom) },
        None => {
            let (x0, x1) = (snap(rect.x), snap(rect.right()));
            let cut = |s: CornerShape, r: f32| if s == CornerShape::Bevel { snap(r) } else { 0.0 };
            let span = |l: f32, r: f32| ((x1 - r) > (x0 + l)).then_some((x0 + l, x1 - r));
            FlatEdges {
                top: span(cut(shapes[0], radii.tl), cut(shapes[1], radii.tr)),
                bottom: span(cut(shapes[3], radii.bl), cut(shapes[2], radii.br)),
            }
        }
    }
}

/// Longest run of consecutive vertices (the polygon is closed) lying on the
/// horizontal line `y`, as the x extent of the run.
fn longest_run(pts: &[PointF], y: f32) -> Option<(f32, f32)> {
    let n = pts.len();
    if n < 2 {
        return None;
    }
    let on = |i: usize| (pts[i % n].y - y).abs() < 0.001;
    let mut best: Option<(f32, f32)> = None;
    for start in 0..n {
        // A run begins where the previous vertex is off the line.
        if !on(start) || on(start + n - 1) {
            continue;
        }
        let (mut lo, mut hi) = (pts[start].x, pts[start].x);
        let mut i = start;
        while on(i + 1) && i + 1 < start + n {
            i += 1;
            lo = lo.min(pts[i % n].x);
            hi = hi.max(pts[i % n].x);
        }
        if hi > lo && best.is_none_or(|b| hi - lo > b.1 - b.0) {
            best = Some((lo, hi));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::LengthPct;

    fn lp(px: f32, pct: f32) -> LengthPct {
        LengthPct { px, pct }
    }

    fn hexagon(d: f32) -> ClipPolygon {
        ClipPolygon {
            points: vec![
                (lp(d, 0.0), lp(0.0, 0.0)),
                (lp(-d, 100.0), lp(0.0, 0.0)),
                (lp(0.0, 100.0), lp(0.0, 50.0)),
                (lp(-d, 100.0), lp(0.0, 100.0)),
                (lp(d, 0.0), lp(0.0, 100.0)),
                (lp(0.0, 0.0), lp(0.0, 50.0)),
            ],
            evenodd: false,
        }
    }

    #[test]
    fn affine_composes_and_inverts() {
        let a = Affine { sx: 2.0, sy: 3.0, tx: 5.0, ty: 7.0 };
        let b = Affine { sx: 0.5, sy: 1.0, tx: 1.0, ty: 0.0 };
        let p = PointF { x: 4.0, y: 2.0 };
        assert_eq!(a.after(&b).point(p), a.point(b.point(p)));
        assert_eq!(a.inverse_point(a.point(p)), Some(p));
        assert_eq!(Affine { sx: 0.0, ..a }.inverse_point(p), None);
    }

    #[test]
    fn snap_rounds_half_up() {
        assert_eq!([snap(1.5), snap(2.49), snap(-0.5), snap(16.25), snap(16.75)], [2.0, 2.0, 0.0, 16.0, 17.0]);
    }

    #[test]
    fn polygon_vertices_land_on_whole_pixels_at_every_scale() {
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let rect = RectF::new(37.0, 12.0, (180.0 * scale).round(), (26.0 * scale).round());
            for p in clip_points(&hexagon(13.0 * scale / scale), &rect, scale) {
                assert_eq!(p.x, p.x.round(), "x at {scale}");
                assert_eq!(p.y, p.y.round(), "y at {scale}");
            }
        }
    }

    #[test]
    fn flat_run_of_a_hexagon_and_of_a_point() {
        let rect = RectF::new(10.0, 0.0, 100.0, 26.0);
        let pts = clip_points(&hexagon(13.0), &rect, 1.0);
        let f = flat_edges(&rect, Some(&pts), &Radii::ZERO, &[CornerShape::Round; 4]);
        assert_eq!((f.top, f.bottom), (Some((23.0, 97.0)), Some((23.0, 97.0))));
        // A chevron whose right side is a tip has no flat run on that side but keeps the left.
        let tip = [PointF { x: 10.0, y: 0.0 }, PointF { x: 110.0, y: 13.0 }, PointF { x: 10.0, y: 26.0 }];
        let f = flat_edges(&rect, Some(&tip), &Radii::ZERO, &[CornerShape::Round; 4]);
        assert_eq!((f.top, f.bottom), (None, None));
    }

    #[test]
    fn border_box_flat_edge_skips_bevelled_corners() {
        let rect = RectF::new(0.0, 0.0, 100.0, 40.0);
        let mut shapes = [CornerShape::Round; 4];
        shapes[0] = CornerShape::Bevel;
        let f = flat_edges(&rect, None, &Radii::all(10.0), &shapes);
        assert_eq!((f.top, f.bottom), (Some((10.0, 100.0)), Some((0.0, 100.0))));
    }

    #[test]
    fn bevel_outline_cuts_the_corner_and_hit_tests_it() {
        let rect = RectF::new(0.0, 0.0, 40.0, 20.0);
        let mut shapes = [CornerShape::Round; 4];
        shapes[0] = CornerShape::Bevel;
        let poly = outline_polygon(&rect, &Radii { tl: 8.0, tr: 0.0, br: 0.0, bl: 0.0 }, &shapes, 4);
        assert!(poly.contains(&PointF { x: 8.0, y: 0.0 }) && poly.contains(&PointF { x: 0.0, y: 8.0 }));
        assert!(!poly.contains(&PointF { x: 0.0, y: 0.0 }));
        let radii = Radii { tl: 8.0, ..Radii::ZERO };
        assert!(in_bevel_cut(&rect, &radii, &shapes, PointF { x: 2.0, y: 2.0 }));
        assert!(!in_bevel_cut(&rect, &radii, &shapes, PointF { x: 5.0, y: 5.0 }));
        assert!(polygon_contains(&poly, PointF { x: 5.0, y: 5.0 }, false));
        assert!(!polygon_contains(&poly, PointF { x: 2.0, y: 2.0 }, false));
    }

    #[test]
    fn evenodd_leaves_the_overlap_out() {
        let square = |o: f32, s: f32| {
            [PointF { x: o, y: o }, PointF { x: o + s, y: o }, PointF { x: o + s, y: o + s }, PointF { x: o, y: o + s }]
        };
        let mut pts = square(0.0, 10.0).to_vec();
        pts.extend(square(2.0, 4.0));
        let inner = PointF { x: 4.0, y: 4.0 };
        assert!(polygon_contains(&pts, inner, false));
        assert!(!polygon_contains(&pts, inner, true));
    }

    #[test]
    fn inner_bevel_moves_in_parallel() {
        let shapes = [CornerShape::Bevel; 4];
        let r = inner_corner_radii(Radii::all(10.0), &shapes, &Edges::all(2.0));
        assert!((r.tl - (10.0 - 4.0 * (2.0 - std::f32::consts::SQRT_2) / 2.0)).abs() < 1e-4, "{r:?}");
    }
}

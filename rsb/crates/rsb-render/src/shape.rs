//! Paths and paints for boxes: rounded rectangles with per-corner radii,
//! border rings with per-side widths, linear gradients.

use rsb_core::color::Color;
use rsb_core::draw::Fill;
use rsb_core::geom::{Edges, PathEl, PathF, PointF, Radii, RectF, RectPx};
use tiny_skia::{GradientStop, LinearGradient, Paint, Path, PathBuilder, Point, SpreadMode, Transform};

/// Control point distance of a cubic that approximates a quarter circle.
const KAPPA: f32 = 0.552_284_7;

/// Elliptical corner radii `(x, y)`, clockwise from the top left.
pub(crate) type Corners = [(f32, f32); 4];

/// True when every coordinate is a finite number of sane size; tiny-skia
/// refuses (and logs) anything else, so callers skip such items quietly.
pub(crate) fn sane(r: &RectF) -> bool {
    const LIMIT: f32 = 1.0e6;
    [r.x, r.y, r.w, r.h].iter().all(|v| v.is_finite() && v.abs() < LIMIT)
}

pub(crate) fn circular(r: Radii) -> Corners {
    [(r.tl, r.tl), (r.tr, r.tr), (r.br, r.br), (r.bl, r.bl)]
}

/// CSS corner clamping: negative radii become 0, and when two radii on one
/// side add up to more than the side, every radius shrinks by the same factor.
pub(crate) fn clamp_corners(mut c: Corners, w: f32, h: f32) -> Corners {
    for (x, y) in c.iter_mut() {
        if !(*x > 0.0 && *y > 0.0) || !x.is_finite() || !y.is_finite() {
            *x = 0.0;
            *y = 0.0;
        }
    }
    let ratio = |side: f32, sum: f32| if sum > side { side / sum } else { 1.0 };
    let f = ratio(w, c[0].0 + c[1].0)
        .min(ratio(h, c[1].1 + c[2].1))
        .min(ratio(w, c[2].0 + c[3].0))
        .min(ratio(h, c[3].1 + c[0].1));
    if f < 1.0 {
        for (x, y) in c.iter_mut() {
            *x *= f;
            *y *= f;
        }
    }
    c
}

/// Adds one closed rounded rectangle to `pb`.
fn add_rrect(pb: &mut PathBuilder, rect: RectF, c: Corners) {
    let (l, t, r, b) = (rect.x, rect.y, rect.right(), rect.bottom());
    let [(tlx, tly), (trx, try_), (brx, bry), (blx, bly)] = c;
    pb.move_to(l + tlx, t);
    pb.line_to(r - trx, t);
    if trx > 0.0 {
        pb.cubic_to(r - trx + KAPPA * trx, t, r, t + try_ - KAPPA * try_, r, t + try_);
    }
    pb.line_to(r, b - bry);
    if brx > 0.0 {
        pb.cubic_to(r, b - bry + KAPPA * bry, r - brx + KAPPA * brx, b, r - brx, b);
    }
    pb.line_to(l + blx, b);
    if blx > 0.0 {
        pb.cubic_to(l + blx - KAPPA * blx, b, l, b - bly + KAPPA * bly, l, b - bly);
    }
    pb.line_to(l, t + tly);
    if tlx > 0.0 {
        pb.cubic_to(l, t + tly - KAPPA * tly, l + tlx - KAPPA * tlx, t, l + tlx, t);
    }
    pb.close();
}

/// A filled rounded rectangle. None for an empty or unusable rectangle.
pub(crate) fn rrect_path(rect: RectF, radii: Radii) -> Option<Path> {
    if rect.is_empty() || !sane(&rect) {
        return None;
    }
    let mut pb = PathBuilder::new();
    add_rrect(&mut pb, rect, clamp_corners(circular(radii), rect.w, rect.h));
    pb.finish()
}

/// A rounded rectangle with whole-pixel rectangles cut out of it (fill with
/// the even-odd rule): the part of a box that needs anti-aliasing when the
/// holes are filled directly. The holes must not overlap.
pub(crate) fn rrect_ring(rect: RectF, radii: Radii, holes: &[RectPx]) -> Option<Path> {
    if rect.is_empty() || !sane(&rect) {
        return None;
    }
    let mut pb = PathBuilder::new();
    add_rrect(&mut pb, rect, clamp_corners(circular(radii), rect.w, rect.h));
    for hole in holes.iter().filter(|h| !h.is_empty()) {
        let (x0, y0, x1, y1) = (hole.x0 as f32, hole.y0 as f32, hole.x1 as f32, hole.y1 as f32);
        pb.move_to(x0, y0);
        pb.line_to(x1, y0);
        pb.line_to(x1, y1);
        pb.line_to(x0, y1);
        pb.close();
    }
    pb.finish()
}

/// Whole-pixel rectangles inside a rounded rectangle that no corner or
/// partly covered edge touches: every pixel of them is fully covered. A
/// band along the top, a band along the bottom and the middle between them
/// (any of them may be empty). None when they add up to little (the
/// ordinary path is as fast there).
pub(crate) fn solid_parts(rect: RectF, radii: Radii) -> Option<[RectPx; 3]> {
    const MIN_PIXELS: i64 = 2048;
    if rect.is_empty() || !sane(&rect) {
        return None;
    }
    let c = clamp_corners(circular(radii), rect.w, rect.h);
    let (top, bottom) = (c[0].1.max(c[1].1), c[2].1.max(c[3].1));
    let (left, right) = (c[0].0.max(c[3].0), c[1].0.max(c[2].0));
    let (x0, x1) = (rect.x.ceil() as i32, rect.right().floor() as i32);
    let (my0, my1) = ((rect.y + top).ceil() as i32, (rect.bottom() - bottom).floor() as i32);
    if my1 <= my0 || x1 <= x0 {
        return None;
    }
    let (bx0, bx1) = ((rect.x + left).ceil() as i32, (rect.right() - right).floor() as i32);
    let parts = [
        RectPx::new(bx0, rect.y.ceil() as i32, bx1, my0),
        RectPx::new(x0, my0, x1, my1),
        RectPx::new(bx0, my1, bx1, rect.bottom().floor() as i32),
    ];
    let area: i64 = parts.iter().filter(|p| !p.is_empty()).map(|p| p.width() as i64 * p.height() as i64).sum();
    (area >= MIN_PIXELS).then_some(parts)
}

/// The ring between the outer rounded rectangle and the same rectangle
/// shrunk by the side widths (fill with the even-odd rule). When the widths
/// leave no inside, the ring is the whole outer shape. None when there is
/// nothing to draw.
pub(crate) fn border_path(rect: RectF, radii: Radii, widths: Edges<f32>) -> Option<Path> {
    if rect.is_empty() || !sane(&rect) {
        return None;
    }
    let side = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };
    let (top, right, bottom, left) = (side(widths.top), side(widths.right), side(widths.bottom), side(widths.left));
    if top + right + bottom + left <= 0.0 {
        return None;
    }
    let outer = clamp_corners(circular(radii), rect.w, rect.h);
    let mut pb = PathBuilder::new();
    add_rrect(&mut pb, rect, outer);
    let inner_rect = RectF::new(rect.x + left, rect.y + top, rect.w - left - right, rect.h - top - bottom);
    if !inner_rect.is_empty() {
        let shrink = |c: (f32, f32), dx: f32, dy: f32| ((c.0 - dx).max(0.0), (c.1 - dy).max(0.0));
        let mut inner = [
            shrink(outer[0], left, top),
            shrink(outer[1], right, top),
            shrink(outer[2], right, bottom),
            shrink(outer[3], left, bottom),
        ];
        for c in inner.iter_mut() {
            if c.0 == 0.0 || c.1 == 0.0 {
                *c = (0.0, 0.0);
            }
        }
        add_rrect(&mut pb, inner_rect, clamp_corners(inner, inner_rect.w, inner_rect.h));
    }
    pb.finish()
}

/// Calls `f` with every point of the outline, control points included.
fn for_each_point(p: &PathF, mut f: impl FnMut(PointF)) {
    for el in &p.els {
        match *el {
            PathEl::MoveTo(a) | PathEl::LineTo(a) => f(a),
            PathEl::QuadTo(a, b) => {
                f(a);
                f(b);
            }
            PathEl::CubicTo(a, b, c) => {
                f(a);
                f(b);
                f(c);
            }
            PathEl::Close => {}
        }
    }
}

/// The rectangle holding every point of an outline (control points
/// included). None for an outline with no points, or with a point that is
/// not a finite number of sane size: such outlines are skipped quietly.
pub(crate) fn path_bounds(p: &PathF) -> Option<RectF> {
    let mut b: Option<(f32, f32, f32, f32)> = None;
    let mut ok = true;
    for_each_point(p, |pt| {
        const LIMIT: f32 = 1.0e6;
        if !(pt.x.is_finite() && pt.y.is_finite() && pt.x.abs() < LIMIT && pt.y.abs() < LIMIT) {
            ok = false;
        }
        b = Some(match b {
            Some((x0, y0, x1, y1)) => (x0.min(pt.x), y0.min(pt.y), x1.max(pt.x), y1.max(pt.y)),
            None => (pt.x, pt.y, pt.x, pt.y),
        });
    });
    let (x0, y0, x1, y1) = b?;
    ok.then(|| RectF::new(x0, y0, x1 - x0, y1 - y0))
}

fn add_outline(pb: &mut PathBuilder, p: &PathF) {
    for el in &p.els {
        match *el {
            PathEl::MoveTo(a) => pb.move_to(a.x, a.y),
            PathEl::LineTo(a) => pb.line_to(a.x, a.y),
            PathEl::QuadTo(a, b) => pb.quad_to(a.x, a.y, b.x, b.y),
            PathEl::CubicTo(a, b, c) => pb.cubic_to(a.x, a.y, b.x, b.y, c.x, c.y),
            PathEl::Close => pb.close(),
        }
    }
}

/// An outline as a tiny-skia path. None when it has no area to fill or a
/// point that is not usable.
pub(crate) fn outline_path(p: &PathF) -> Option<Path> {
    path_bounds(p)?;
    let mut pb = PathBuilder::new();
    add_outline(&mut pb, p);
    pb.finish()
}

/// Two outlines in one path, to be filled with the even-odd rule: the ring
/// between `outer` and `inner`. An unusable `inner` leaves the whole outer
/// shape; an unusable `outer` leaves nothing.
pub(crate) fn ring_path(outer: &PathF, inner: &PathF) -> Option<Path> {
    path_bounds(outer)?;
    let mut pb = PathBuilder::new();
    add_outline(&mut pb, outer);
    if path_bounds(inner).is_some() {
        add_outline(&mut pb, inner);
    }
    pb.finish()
}

pub(crate) fn sk_color(c: Color) -> tiny_skia::Color {
    let ch = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    tiny_skia::Color::from_rgba(ch(c.r), ch(c.g), ch(c.b), ch(c.a)).unwrap_or(tiny_skia::Color::TRANSPARENT)
}

/// An anti-aliased paint for a `Fill`. None when a gradient cannot be built
/// (no stops); a degenerate gradient (start equals end) paints its last stop.
pub(crate) fn paint_for(fill: &Fill) -> Option<Paint<'static>> {
    let mut paint = Paint { anti_alias: true, ..Paint::default() };
    match fill {
        Fill::Solid(c) => paint.set_color(sk_color(*c)),
        Fill::Linear { start, end, stops } => {
            let last = stops.last()?.1;
            let mut prev = 0.0f32;
            let list: Vec<GradientStop> = stops
                .iter()
                .map(|(pos, c)| {
                    let p = if pos.is_finite() { pos.clamp(0.0, 1.0) } else { 0.0 }.max(prev);
                    prev = p;
                    GradientStop::new(p, sk_color(*c))
                })
                .collect();
            let ok = [start.x, start.y, end.x, end.y].iter().all(|v| v.is_finite());
            if !ok || (start.x == end.x && start.y == end.y) {
                paint.set_color(sk_color(last));
            } else {
                paint.shader = LinearGradient::new(
                    Point::from_xy(start.x, start.y),
                    Point::from_xy(end.x, end.y),
                    list,
                    SpreadMode::Pad,
                    Transform::identity(),
                )?;
            }
        }
    }
    Some(paint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_scale_together_when_they_do_not_fit() {
        let c = clamp_corners(circular(Radii::all(30.0)), 40.0, 100.0);
        assert!((c[0].0 - 20.0).abs() < 1e-4 && (c[1].0 - 20.0).abs() < 1e-4, "{c:?}");
        assert!((c[0].1 - 20.0).abs() < 1e-4, "uniform scale keeps circles round: {c:?}");
    }

    #[test]
    fn negative_and_nan_radii_are_square() {
        let c = clamp_corners(circular(Radii { tl: -3.0, tr: f32::NAN, br: 4.0, bl: 0.0 }), 50.0, 50.0);
        assert_eq!(c[0], (0.0, 0.0));
        assert_eq!(c[1], (0.0, 0.0));
        assert_eq!(c[2], (4.0, 4.0));
    }

    #[test]
    fn border_without_width_is_nothing() {
        assert!(border_path(RectF::new(0.0, 0.0, 10.0, 10.0), Radii::ZERO, Edges::all(0.0)).is_none());
    }

    #[test]
    fn empty_and_huge_rects_make_no_path() {
        assert!(rrect_path(RectF::new(0.0, 0.0, 0.0, 5.0), Radii::ZERO).is_none());
        assert!(rrect_path(RectF::new(0.0, 0.0, f32::INFINITY, 5.0), Radii::ZERO).is_none());
        assert!(rrect_path(RectF::new(0.0, 0.0, 5.0, 5.0), Radii::all(2.0)).is_some());
    }

    #[test]
    fn gradient_without_stops_has_no_paint() {
        let f =
            Fill::Linear { start: Default::default(), end: rsb_core::geom::PointF { x: 1.0, y: 0.0 }, stops: vec![] };
        assert!(paint_for(&f).is_none());
    }
}

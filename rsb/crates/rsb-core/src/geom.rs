//! Geometry. `RectF` and friends are in physical pixels unless a field says
//! otherwise; `RectPx` is an integer, half-open pixel rectangle.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PointF {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SizeF {
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectF {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> RectF {
        RectF { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    /// Half-open: the right and bottom edges are outside.
    pub fn contains(&self, p: PointF) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.right() && p.y < self.bottom()
    }

    /// Grown by `d` on every side (shrunk when negative).
    pub fn outset(&self, d: f32) -> RectF {
        RectF { x: self.x - d, y: self.y - d, w: self.w + 2.0 * d, h: self.h + 2.0 * d }
    }

    pub fn union(&self, o: &RectF) -> RectF {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        RectF { x, y, w: self.right().max(o.right()) - x, h: self.bottom().max(o.bottom()) - y }
    }

    pub fn intersect(&self, o: &RectF) -> Option<RectF> {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = RectF { x, y, w: self.right().min(o.right()) - x, h: self.bottom().min(o.bottom()) - y };
        (!r.is_empty()).then_some(r)
    }
}

/// Integer pixel rectangle, half-open: `x0..x1`, `y0..y1`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RectPx {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl RectPx {
    pub const fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> RectPx {
        RectPx { x0, y0, x1, y1 }
    }

    pub fn width(&self) -> i32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> i32 {
        self.y1 - self.y0
    }

    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    /// Smallest integer rectangle that covers `r`.
    pub fn outward(r: &RectF) -> RectPx {
        RectPx {
            x0: r.x.floor() as i32,
            y0: r.y.floor() as i32,
            x1: r.right().ceil() as i32,
            y1: r.bottom().ceil() as i32,
        }
    }

    pub fn union(&self, o: &RectPx) -> RectPx {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        RectPx { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }

    pub fn intersect(&self, o: &RectPx) -> Option<RectPx> {
        let r = RectPx { x0: self.x0.max(o.x0), y0: self.y0.max(o.y0), x1: self.x1.min(o.x1), y1: self.y1.min(o.y1) };
        (!r.is_empty()).then_some(r)
    }
}

/// Corner radii, clockwise from top left.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Radii {
    pub tl: f32,
    pub tr: f32,
    pub br: f32,
    pub bl: f32,
}

impl Radii {
    pub const ZERO: Radii = Radii { tl: 0.0, tr: 0.0, br: 0.0, bl: 0.0 };

    pub const fn all(r: f32) -> Radii {
        Radii { tl: r, tr: r, br: r, bl: r }
    }

    pub fn is_zero(&self) -> bool {
        self.tl == 0.0 && self.tr == 0.0 && self.br == 0.0 && self.bl == 0.0
    }
}

/// Four sides in CSS order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Edges<T> {
    pub top: T,
    pub right: T,
    pub bottom: T,
    pub left: T,
}

impl<T: Copy> Edges<T> {
    pub const fn all(v: T) -> Edges<T> {
        Edges { top: v, right: v, bottom: v, left: v }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outward_covers_fractional_edges() {
        let r = RectPx::outward(&RectF::new(1.5, 2.25, 3.0, 1.0));
        assert_eq!(r, RectPx::new(1, 2, 5, 4));
    }

    #[test]
    fn union_ignores_empty() {
        let a = RectPx::new(0, 0, 0, 0);
        let b = RectPx::new(2, 2, 4, 4);
        assert_eq!(a.union(&b), b);
        assert_eq!(b.intersect(&RectPx::new(3, 3, 9, 9)), Some(RectPx::new(3, 3, 4, 4)));
    }
}

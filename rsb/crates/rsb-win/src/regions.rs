//! The shapes the outside-press watch tests a point against: every bar's
//! window rectangle and every popup's clickable outline, in screen pixels.
//! Pure data and functions, so the hook procedure can use a snapshot with no
//! locks, and tests need no window.

use rsb_core::geom::RectPx;

/// One popup window's clickable area.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PopupRegion {
    /// The window's top-left corner, virtual-screen px.
    pub origin: (i32, i32),
    /// The window's size, px.
    pub size: (i32, i32),
    /// The outline in window-relative px; None = the whole window.
    pub outline: Option<Vec<(f32, f32)>>,
}

/// Everything that counts as "inside RSB" at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Regions {
    pub bars: Vec<RectPx>,
    pub popups: Vec<PopupRegion>,
}

impl Regions {
    /// Whether the screen point is on one of our bars or inside a popup's
    /// outline.
    pub(crate) fn contains(&self, x: i32, y: i32) -> bool {
        if self.bars.iter().any(|r| x >= r.x0 && x < r.x1 && y >= r.y0 && y < r.y1) {
            return true;
        }
        self.popups.iter().any(|p| p.contains(x, y))
    }
}

impl PopupRegion {
    fn contains(&self, x: i32, y: i32) -> bool {
        let (rx, ry) = (x - self.origin.0, y - self.origin.1);
        if rx < 0 || ry < 0 || rx >= self.size.0 || ry >= self.size.1 {
            return false;
        }
        match &self.outline {
            // The pixel's centre, as the renderer samples it.
            Some(poly) if poly.len() >= 3 => point_in_polygon(poly, rx as f32 + 0.5, ry as f32 + 0.5),
            _ => true,
        }
    }
}

/// Even-odd rule: a ray towards +x crosses the polygon's edges an odd number
/// of times from inside. Points on a horizontal edge follow the half-open
/// convention (the top edge counts, the bottom does not), so two polygons
/// that share an edge never both claim a point on it.
pub(crate) fn point_in_polygon(poly: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) {
            let cross = (xj - xi) * (y - yi) / (yj - yi) + xi;
            if x < cross {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<(f32, f32)> {
        vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]
    }

    #[test]
    fn a_point_in_a_square() {
        let s = square();
        assert!(point_in_polygon(&s, 5.0, 5.0));
        assert!(!point_in_polygon(&s, 15.0, 5.0));
        assert!(!point_in_polygon(&s, 5.0, -1.0));
        assert!(!point_in_polygon(&s, -0.5, 5.0));
    }

    #[test]
    fn the_hit_test_here_agrees_with_the_one_that_decides_what_a_click_reaches() {
        use rsb_core::geom::{PointF, polygon_contains};
        // A chevron-like shape, a star drawn with crossing edges, and a plain box.
        let shapes: [Vec<(f32, f32)>; 3] = [
            vec![(0.0, 0.0), (40.0, 0.0), (50.0, 12.0), (40.0, 24.0), (0.0, 24.0), (10.0, 12.0)],
            vec![(25.0, 0.0), (10.0, 48.0), (48.0, 16.0), (2.0, 16.0), (40.0, 48.0)],
            square(),
        ];
        for poly in &shapes {
            let points: Vec<PointF> = poly.iter().map(|&(x, y)| PointF { x, y }).collect();
            for iy in 0..120 {
                for ix in 0..120 {
                    // Off the whole-number grid so no sample sits on an edge.
                    let (x, y) = (ix as f32 * 0.43 + 0.137, iy as f32 * 0.41 + 0.211);
                    let ours = point_in_polygon(poly, x, y);
                    let theirs = polygon_contains(&points, PointF { x, y }, true);
                    assert_eq!(ours, theirs, "({x}, {y}) in {poly:?}");
                }
            }
        }
    }

    #[test]
    fn a_concave_polygon_has_a_notch() {
        // A "U": the notch between x 3 and 7 is open from the top down to y 6.
        let u =
            vec![(0.0, 0.0), (3.0, 0.0), (3.0, 6.0), (7.0, 6.0), (7.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert!(point_in_polygon(&u, 1.0, 1.0));
        assert!(point_in_polygon(&u, 9.0, 1.0));
        assert!(point_in_polygon(&u, 5.0, 8.0));
        assert!(!point_in_polygon(&u, 5.0, 2.0), "inside the notch");
    }

    #[test]
    fn a_bevelled_corner_is_outside_the_cut() {
        // A square with its top-left corner cut by 4 px.
        let b = vec![(4.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (0.0, 4.0)];
        assert!(point_in_polygon(&b, 5.0, 1.0));
        assert!(point_in_polygon(&b, 1.5, 4.5));
        assert!(!point_in_polygon(&b, 0.5, 0.5), "the cut corner");
        assert!(!point_in_polygon(&b, 1.5, 1.5), "still outside the diagonal");
        assert!(point_in_polygon(&b, 3.0, 3.0), "on the inside of the diagonal");
    }

    #[test]
    fn two_polygons_sharing_an_edge_do_not_both_own_a_point_on_it() {
        let top = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)];
        let bottom = vec![(0.0, 5.0), (10.0, 5.0), (10.0, 10.0), (0.0, 10.0)];
        let on_edge = (5.0, 5.0);
        let a = point_in_polygon(&top, on_edge.0, on_edge.1);
        let b = point_in_polygon(&bottom, on_edge.0, on_edge.1);
        assert!(a ^ b, "exactly one owns it: {a} {b}");
    }

    #[test]
    fn bars_are_tested_by_their_rectangle_half_open() {
        let r = Regions { bars: vec![RectPx::new(0, 0, 100, 30)], popups: Vec::new() };
        assert!(r.contains(0, 0));
        assert!(r.contains(99, 29));
        assert!(!r.contains(100, 10));
        assert!(!r.contains(10, 30));
        assert!(!r.contains(-1, 10));
    }

    #[test]
    fn a_popup_without_an_outline_is_its_window_and_one_with_an_outline_is_its_shape() {
        let whole = PopupRegion { origin: (200, 100), size: (50, 40), outline: None };
        let shaped = PopupRegion {
            origin: (400, 100),
            size: (50, 40),
            // Only the left half is clickable.
            outline: Some(vec![(0.0, 0.0), (25.0, 0.0), (25.0, 40.0), (0.0, 40.0)]),
        };
        let r = Regions { bars: Vec::new(), popups: vec![whole, shaped] };
        assert!(r.contains(200, 100));
        assert!(r.contains(249, 139));
        assert!(!r.contains(250, 100));
        assert!(r.contains(410, 120));
        assert!(!r.contains(430, 120), "a shadow pixel inside the window but outside the outline");
        assert!(!r.contains(399, 120));
        assert!(!r.contains(300, 300));
    }

    #[test]
    fn a_degenerate_outline_falls_back_to_the_window() {
        let p = PopupRegion { origin: (0, 0), size: (10, 10), outline: Some(vec![(0.0, 0.0), (5.0, 5.0)]) };
        assert!(p.contains(3, 3));
    }
}

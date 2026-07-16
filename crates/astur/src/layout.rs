//! Tiling layout math: pure geometry, no Win32 state. Given a work area and a
//! window count, produce the target rectangle for each window.

use windows::Win32::Foundation::RECT;

/// Master-stack layout: one master column on the left,
/// the remaining windows stacked vertically on the right.
pub(crate) fn master_stack(area: RECT, n: usize, ratio: f32, outer: i32, inner: i32) -> Vec<RECT> {
    let mut out = Vec::with_capacity(n);
    let x0 = area.left + outer;
    let y0 = area.top + outer;
    let w = area.right - area.left - 2 * outer;
    let h = area.bottom - area.top - 2 * outer;
    if n == 0 || w <= 0 || h <= 0 {
        return out;
    }
    if n == 1 {
        out.push(RECT {
            left: x0,
            top: y0,
            right: x0 + w,
            bottom: y0 + h,
        });
        return out;
    }
    let master_w = ((w - inner) as f32 * ratio) as i32;
    let stack_w = (w - inner) - master_w;
    out.push(RECT {
        left: x0,
        top: y0,
        right: x0 + master_w,
        bottom: y0 + h,
    });
    let sx = x0 + master_w + inner;
    let sc = (n - 1) as i32;
    let each = (h - (sc - 1) * inner) / sc;
    for i in 0..sc {
        let sy = y0 + i * (each + inner);
        let bottom = if i == sc - 1 { y0 + h } else { sy + each };
        out.push(RECT {
            left: sx,
            top: sy,
            right: sx + stack_w,
            bottom,
        });
    }
    out
}

/// The split ratio for level `i`, defaulting to 0.5 and clamped to a sane range.
pub(crate) fn split_ratio(splits: &[f32], i: usize) -> f32 {
    splits.get(i).copied().unwrap_or(0.5).clamp(0.05, 0.95)
}

/// Dwindle/spiral layout (spiral default): each window takes a
/// fraction (`splits[i]`, default half) of the remaining space, alternating the
/// split along the longer side, so windows spiral toward the bottom corner.
/// Resizing a window edits the relevant `splits` entry (see `resize_dwindle`).
pub(crate) fn dwindle_layout(
    area: RECT,
    n: usize,
    outer: i32,
    inner: i32,
    splits: &[f32],
) -> Vec<RECT> {
    let mut out = Vec::with_capacity(n);
    if n == 0 {
        return out;
    }
    let mut cur = RECT {
        left: area.left + outer,
        top: area.top + outer,
        right: area.right - outer,
        bottom: area.bottom - outer,
    };
    if cur.right <= cur.left || cur.bottom <= cur.top {
        return out;
    }
    for i in 0..n {
        if i == n - 1 {
            out.push(cur);
            break;
        }
        let w = cur.right - cur.left;
        let h = cur.bottom - cur.top;
        let r = split_ratio(splits, i);
        if w >= h {
            let half = ((w - inner) as f32 * r) as i32;
            out.push(RECT {
                left: cur.left,
                top: cur.top,
                right: cur.left + half,
                bottom: cur.bottom,
            });
            cur.left += half + inner;
        } else {
            let half = ((h - inner) as f32 * r) as i32;
            out.push(RECT {
                left: cur.left,
                top: cur.top,
                right: cur.right,
                bottom: cur.top + half,
            });
            cur.top += half + inner;
        }
    }
    out
}

/// Equal-width columns. Useful for ultrawide monitors and predictable placement.
pub(crate) fn columns_layout(area: RECT, n: usize, outer: i32, inner: i32) -> Vec<RECT> {
    grid_cells(area, n, n.max(1), outer, inner)
}

/// Balanced grid using approximately square cells. Last row stretches across
/// its available columns rather than leaving dead tiles.
pub(crate) fn grid_layout(area: RECT, n: usize, outer: i32, inner: i32) -> Vec<RECT> {
    if n == 0 {
        return Vec::new();
    }
    let cols = (n as f64).sqrt().ceil() as usize;
    grid_cells(area, n, cols.max(1), outer, inner)
}

fn grid_cells(area: RECT, n: usize, cols: usize, outer: i32, inner: i32) -> Vec<RECT> {
    let mut out = Vec::with_capacity(n);
    let left = area.left + outer;
    let top = area.top + outer;
    let width = area.right - area.left - 2 * outer;
    let height = area.bottom - area.top - 2 * outer;
    if n == 0 || width <= 0 || height <= 0 {
        return out;
    }
    let rows = n.div_ceil(cols);
    let row_gap = fitted_gap(height, rows, inner);
    let row_h = (height - row_gap * (rows.saturating_sub(1) as i32)) / rows as i32;
    for row in 0..rows {
        let start = row * cols;
        let count = (n - start).min(cols);
        let col_gap = fitted_gap(width, count, inner);
        let cell_w = (width - col_gap * (count.saturating_sub(1) as i32)) / count as i32;
        let y = top + row as i32 * (row_h + row_gap);
        let bottom = if row + 1 == rows {
            top + height
        } else {
            y + row_h
        };
        for col in 0..count {
            let x = left + col as i32 * (cell_w + col_gap);
            let right = if col + 1 == count {
                left + width
            } else {
                x + cell_w
            };
            out.push(RECT {
                left: x,
                top: y,
                right,
                bottom,
            });
        }
    }
    out
}

fn fitted_gap(extent: i32, cells: usize, requested: i32) -> i32 {
    if cells <= 1 {
        return 0;
    }
    let gaps = (cells - 1) as i32;
    requested.max(0).min((extent - cells as i32).max(0) / gaps)
}

/// Monocle layout: every tiled window fills the work area. Focus determines
/// which stacked window is visible; no window is resized differently.
pub(crate) fn monocle_layout(area: RECT, n: usize, outer: i32) -> Vec<RECT> {
    if n == 0 {
        return Vec::new();
    }
    let rect = RECT {
        left: area.left + outer,
        top: area.top + outer,
        right: area.right - outer,
        bottom: area.bottom - outer,
    };
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return Vec::new();
    }
    vec![rect; n]
}

/// Update `splits` so the dwindle window at tiled index `idx` matches the size
/// the user dragged it to (`new`). Replays the cascade to find that window's
/// split level + axis, then back-computes the ratio. Neighbours reflow to fill.
pub(crate) fn resize_dwindle(
    splits: &mut Vec<f32>,
    area: RECT,
    n: usize,
    outer: i32,
    inner: i32,
    idx: usize,
    new: RECT,
) {
    if n < 2 {
        return;
    }
    // The window at idx owns split level idx (it takes the first part); the very
    // last window instead shares level n-2 (it is that split's remainder).
    let (level, is_remainder) = if idx < n - 1 {
        (idx, false)
    } else {
        (n - 2, true)
    };
    if splits.len() < n - 1 {
        splits.resize(n - 1, 0.5);
    }
    // Replay the cascade up to `level` to find that split's available rect.
    let mut cur = RECT {
        left: area.left + outer,
        top: area.top + outer,
        right: area.right - outer,
        bottom: area.bottom - outer,
    };
    for i in 0..level {
        let w = cur.right - cur.left;
        let h = cur.bottom - cur.top;
        let r = split_ratio(splits, i);
        if w >= h {
            let half = ((w - inner) as f32 * r) as i32;
            cur.left += half + inner;
        } else {
            let half = ((h - inner) as f32 * r) as i32;
            cur.top += half + inner;
        }
    }
    let w = cur.right - cur.left;
    let h = cur.bottom - cur.top;
    let vertical = w >= h;
    let avail = (if vertical { w } else { h } - inner).max(1) as f32;
    let new_size = if vertical {
        new.right - new.left
    } else {
        new.bottom - new.top
    } as f32;
    // First-half window: ratio = its size / available. Remainder window: it gets
    // (1 - ratio), so ratio = 1 - its size / available.
    let ratio = if is_remainder {
        1.0 - new_size / avail
    } else {
        new_size / avail
    };
    splits[level] = ratio.clamp(0.05, 0.95);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn split_ratio_defaults_and_clamps() {
        assert_eq!(split_ratio(&[], 0), 0.5); // missing -> default
        assert_eq!(split_ratio(&[0.3], 0), 0.3);
        assert_eq!(split_ratio(&[0.0], 0), 0.05); // clamp low
        assert_eq!(split_ratio(&[1.0], 0), 0.95); // clamp high
        assert_eq!(split_ratio(&[0.7], 5), 0.5); // out-of-range index -> default
    }

    #[test]
    fn master_stack_empty_and_degenerate() {
        assert!(master_stack(r(0, 0, 100, 100), 0, 0.5, 0, 0).is_empty());
        assert!(master_stack(r(0, 0, 0, 0), 3, 0.5, 0, 0).is_empty());
        // outer gap larger than the area leaves no usable space
        assert!(master_stack(r(0, 0, 10, 10), 2, 0.5, 20, 0).is_empty());
    }

    #[test]
    fn master_stack_single_fills_area_minus_outer() {
        let v = master_stack(r(0, 0, 100, 100), 1, 0.5, 10, 5);
        assert_eq!(v, vec![r(10, 10, 90, 90)]);
    }

    #[test]
    fn master_stack_two_split_by_ratio_no_overlap() {
        let v = master_stack(r(0, 0, 100, 100), 2, 0.5, 0, 0);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], r(0, 0, 50, 100));
        assert_eq!(v[1], r(50, 0, 100, 100)); // master right == stack left
    }

    #[test]
    fn master_stack_stack_covers_full_height() {
        // master + two stacked; last stack window's bottom hits the area bottom.
        let v = master_stack(r(0, 0, 200, 100), 3, 0.5, 0, 0);
        assert_eq!(v.len(), 3);
        assert_eq!(v[1].top, 0);
        assert_eq!(v[2].bottom, 100);
        assert!(v[1].bottom <= v[2].top); // no vertical overlap in the stack
    }

    #[test]
    fn dwindle_single_is_area_minus_outer() {
        let v = dwindle_layout(r(0, 0, 100, 100), 1, 8, 4, &[]);
        assert_eq!(v, vec![r(8, 8, 92, 92)]);
    }

    #[test]
    fn dwindle_count_and_first_split_vertical_when_wide() {
        let v = dwindle_layout(r(0, 0, 200, 100), 2, 0, 0, &[0.5]);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].left, 0);
        assert_eq!(v[0].right, v[1].left); // touching (inner gap 0)
        assert_eq!(v[1].right, 200);
    }

    #[test]
    fn dwindle_degenerate_area_empty() {
        assert!(dwindle_layout(r(0, 0, 5, 5), 2, 10, 0, &[]).is_empty());
    }

    #[test]
    fn columns_split_width_and_preserve_edge() {
        let v = columns_layout(r(0, 0, 100, 80), 3, 0, 10);
        assert_eq!(
            v,
            vec![r(0, 0, 26, 80), r(36, 0, 62, 80), r(72, 0, 100, 80)]
        );
    }

    #[test]
    fn grid_balances_rows_and_stretches_last_row() {
        let v = grid_layout(r(0, 0, 300, 200), 5, 0, 0);
        assert_eq!(v.len(), 5);
        assert_eq!(v[0], r(0, 0, 100, 100));
        assert_eq!(v[2], r(200, 0, 300, 100));
        assert_eq!(v[3], r(0, 100, 150, 200));
        assert_eq!(v[4], r(150, 100, 300, 200));
    }

    #[test]
    fn grid_clamps_gap_to_keep_cells_valid() {
        let v = grid_layout(r(0, 0, 20, 10), 4, 0, 500);
        assert_eq!(v.len(), 4);
        assert!(v.iter().all(|cell| cell.right > cell.left));
        assert!(v.iter().all(|cell| cell.bottom > cell.top));
    }

    #[test]
    fn monocle_repeats_full_target() {
        let v = monocle_layout(r(0, 0, 100, 80), 3, 5);
        assert_eq!(v, vec![r(5, 5, 95, 75); 3]);
        assert!(monocle_layout(r(0, 0, 4, 4), 1, 3).is_empty());
    }

    #[test]
    fn resize_dwindle_sets_focused_split_from_size() {
        let mut splits = vec![0.5];
        let area = r(0, 0, 200, 100);
        // n=2, inner 0: drag window 0 to width 120 -> ratio 0.6.
        resize_dwindle(&mut splits, area, 2, 0, 0, 0, r(0, 0, 120, 100));
        assert!((splits[0] - 0.6).abs() < 1e-3, "got {}", splits[0]);
    }

    #[test]
    fn resize_dwindle_remainder_uses_inverse() {
        let mut splits = vec![0.5];
        let area = r(0, 0, 200, 100);
        // The last window is the remainder of level 0: width 120 -> ratio 1-0.6.
        resize_dwindle(&mut splits, area, 2, 0, 0, 1, r(80, 0, 200, 100));
        assert!((splits[0] - 0.4).abs() < 1e-3, "got {}", splits[0]);
    }

    #[test]
    fn resize_dwindle_noop_when_single() {
        let mut splits = vec![0.5];
        resize_dwindle(&mut splits, r(0, 0, 200, 100), 1, 0, 0, 0, r(0, 0, 50, 50));
        assert_eq!(splits, vec![0.5]);
    }
}

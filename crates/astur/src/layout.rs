//! Tiling layout math: pure geometry, no Win32 state. Given a work area and a
//! window count, produce the target rectangle for each window. The scrolling
//! strip (bottom of the file) also decides where off-screen tiles go.

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
    // Clamp the gaps to what actually fits. `inner_gap` accepts up to 500 in the
    // config, and on a small work area with many stacked windows the unclamped
    // arithmetic produced rects with bottom < top — SetWindowPos then does
    // something arbitrary (review B-11). `grid_cells` has always guarded this
    // with `fitted_gap`; master and dwindle did not.
    let col_gap = fitted_gap(w, 2, inner);
    let master_w = ((w - col_gap) as f32 * ratio) as i32;
    let stack_w = (w - col_gap) - master_w;
    out.push(RECT {
        left: x0,
        top: y0,
        right: x0 + master_w,
        bottom: y0 + h,
    });
    let sx = x0 + master_w + col_gap;
    let sc = (n - 1) as i32;
    let row_gap = fitted_gap(h, n - 1, inner);
    let each = ((h - (sc - 1) * row_gap) / sc).max(1);
    for i in 0..sc {
        let sy = y0 + i * (each + row_gap);
        let bottom = if i == sc - 1 { y0 + h } else { sy + each };
        if bottom <= sy || sx + stack_w <= sx {
            break; // out of room; better to leave a window put than invert it
        }
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
        // No room for another split: give every remaining window the rect we
        // have. Dropping them would leave real windows wherever they happened
        // to be, which is worse than a stack.
        if w.max(h) < MIN_TILE * 2 {
            for _ in i..n {
                out.push(cur);
            }
            break;
        }
        let r = split_ratio(splits, i);
        if w >= h {
            // See the note in master_stack: an unclamped gap wider than the
            // remaining space made `half` negative and inverted the rect.
            let gap = fitted_gap(w, 2, inner);
            let half = (((w - gap) as f32 * r) as i32).max(1);
            out.push(RECT {
                left: cur.left,
                top: cur.top,
                right: cur.left + half,
                bottom: cur.bottom,
            });
            cur.left += half + gap;
        } else {
            let gap = fitted_gap(h, 2, inner);
            let half = (((h - gap) as f32 * r) as i32).max(1);
            out.push(RECT {
                left: cur.left,
                top: cur.top,
                right: cur.right,
                bottom: cur.top + half,
            });
            cur.top += half + gap;
        }
        if cur.right <= cur.left || cur.bottom <= cur.top {
            // Belt and braces: never emit an inverted rect, whatever the gaps.
            for _ in i + 1..n {
                out.push(*out.last().expect("pushed above"));
            }
            break;
        }
    }
    out
}

/// True while `dwindle_layout` can still give every window its own tile. Above
/// this count the tail stacks (see `MIN_TILE`).
#[cfg(test)]
fn dwindle_tiles_fit(area: RECT, n: usize, outer: i32, inner: i32, splits: &[f32]) -> bool {
    let rects = dwindle_layout(area, n, outer, inner, splits);
    rects.len() == n && rects.windows(2).all(|p| p[0] != p[1])
}

/// Smallest tile a dwindle split may produce (roughly a title bar). Below this
/// the spiral stops dividing and the remaining windows share the last rect
/// (monocle-style tail) — every window still gets a real, on-screen rect, which
/// is what matters. Splitting past this point produced 1-px slivers, and before
/// the gaps were clamped it produced INVERTED rects that went straight to
/// SetWindowPos (review B-11).
///
/// Measured, not guessed: because the spiral halves geometrically, the value
/// barely moves the practical limit on a 1920x1052 work area with outer 8 /
/// inner 4 — 60 px gives 9 distinct tiles, 32 gives 10, and 16 still gives only
/// 12. Past ~10 windows dwindle is unusable whatever we do, so 32 is chosen for
/// staying closest to the old behaviour at realistic counts.
const MIN_TILE: i32 = 32;

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
    // Must replay EXACTLY what dwindle_layout does, gap clamping included, or a
    // drag maps onto the wrong split level.
    for i in 0..level {
        let w = cur.right - cur.left;
        let h = cur.bottom - cur.top;
        let r = split_ratio(splits, i);
        if w >= h {
            let gap = fitted_gap(w, 2, inner);
            let half = (((w - gap) as f32 * r) as i32).max(1);
            cur.left += half + gap;
        } else {
            let gap = fitted_gap(h, 2, inner);
            let half = (((h - gap) as f32 * r) as i32).max(1);
            cur.top += half + gap;
        }
    }
    let w = cur.right - cur.left;
    let h = cur.bottom - cur.top;
    let vertical = w >= h;
    let gap = fitted_gap(if vertical { w } else { h }, 2, inner);
    let avail = (if vertical { w } else { h } - gap).max(1) as f32;
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

// ---- scrolling strip (Niri-style) --------------------------------------------
// Contract: plan/scrolling.md, "Geometry contract". Strip x 0 is the view's left
// edge at offset 0 and column 0 starts at `outer`; screen x = area.left +
// strip_x - offset. Unlike the layouts above, strip rects may be off-screen on
// purpose: the WM decides per rect (`classify`) whether to place it where it
// is or park it outside every monitor (`park_spot`).

/// One column of the scrolling strip, as the geometry needs it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StripCol {
    /// Fraction of the usable view width (see `col_px`).
    pub width: f32,
    /// One entry per tile, top to bottom; heights are proportional to these.
    pub weights: Vec<f32>,
    pub tabbed: bool,
    /// Active tab. Every tab gets the same rect here and the WM parks the
    /// inactive ones, so the geometry itself never reads it.
    pub active: usize,
}

/// Where `scroll_to_show` puts the focused column (`scroll_center`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Center {
    Never,
    Always,
    OnOverflow,
}

/// What the WM does with one strip rect (see `classify`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Vis {
    Full,
    Peek,
    Park,
}

/// `tiles` is column-major in input order; `tab_bars` has one entry per column.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StripOut {
    pub tiles: Vec<RECT>,
    pub tab_bars: Vec<Option<RECT>>,
}

/// How far outside the monitors a parked window sits. Anything past the DWM
/// shadow (drawn outside the frame, wider at high DPI) would do; 200 px is far
/// past it, so no sliver of a parked window ever shows on a screen.
const PARK_MARGIN: i32 = 200;

/// Pixel width of a column that is `width` of the view. Adding one `inner`
/// back is what makes n columns of 1/n, their n-1 gaps and both outer gaps
/// sum to `view_w` (exact when it divides evenly, else within rounding).
pub(crate) fn col_px(view_w: i32, width: f32, outer: i32, inner: i32) -> i32 {
    ((width * (view_w - 2 * outer + inner) as f32).round() as i32 - inner).max(1)
}

/// Inverse of `col_px`: the width fraction whose column is `px` wide (an
/// Alt+right-drag resize). Not clamped; the caller's set_width clamps.
pub(crate) fn col_width(view_w: i32, px: i32, outer: i32, inner: i32) -> f32 {
    (px + inner) as f32 / (view_w - 2 * outer + inner).max(1) as f32
}

/// Strip-space `(left, right)` of every column.
pub(crate) fn strip_spans(view_w: i32, widths: &[f32], outer: i32, inner: i32) -> Vec<(i32, i32)> {
    let mut x = outer;
    widths
        .iter()
        .map(|&w| {
            let span = (x, x + col_px(view_w, w, outer, inner));
            x = span.1 + inner;
            span
        })
        .collect()
}

/// Keeps the view on the strip: a strip that fits is left-aligned, a longer
/// one never shows empty space past either end.
pub(crate) fn clamp_offset(view_w: i32, spans: &[(i32, i32)], outer: i32, offset: i32) -> i32 {
    let total = spans.last().map_or(0, |s| s.1 + outer);
    if total <= view_w {
        0
    } else {
        offset.clamp(0, total - view_w)
    }
}

/// The offset that shows column `focus` according to `mode`.
pub(crate) fn scroll_to_show(
    view_w: i32,
    spans: &[(i32, i32)],
    outer: i32,
    focus: usize,
    offset: i32,
    mode: Center,
) -> i32 {
    // Judged from what is on screen: the stored offset can be past the end
    // of a strip that shrank, and only the layout clamps it.
    let offset = clamp_offset(view_w, spans, outer, offset);
    let Some(&(a, b)) = spans.get(focus) else {
        return offset;
    };
    let left_out = a - outer < offset;
    let right_out = b + outer > offset + view_w;
    let centred = (a + b) / 2 - view_w / 2;
    let want = if b - a > view_w - 2 * outer {
        // Wider than the view (only a user-dragged column can be): show its
        // left edge; its middle would hide both edges.
        a - outer
    } else {
        match mode {
            Center::Never if left_out => a - outer,
            Center::Never if right_out => b + outer - view_w,
            Center::Always => centred,
            Center::OnOverflow if left_out || right_out => centred,
            _ => offset,
        }
    };
    // Clamped in every mode, `Always` included: centring the first or last
    // column would otherwise show empty space past the strip end. Niri allows
    // that; we chose a full view over exact centring (plan/scrolling.md).
    clamp_offset(view_w, spans, outer, want)
}

/// Screen rects for every tile of the strip at `offset` (may be off-screen).
/// A degenerate area yields no tiles, so the caller leaves windows put.
pub(crate) fn strip_layout(
    area: RECT,
    cols: &[StripCol],
    offset: i32,
    outer: i32,
    inner: i32,
    tab_h: i32,
) -> StripOut {
    let mut out = StripOut {
        tiles: Vec::new(),
        tab_bars: vec![None; cols.len()],
    };
    let view_w = area.right - area.left;
    let top = area.top + outer;
    let h = area.bottom - area.top - 2 * outer;
    if view_w - 2 * outer <= 0 || h <= 0 {
        return out;
    }
    let widths: Vec<f32> = cols.iter().map(|c| c.width).collect();
    let spans = strip_spans(view_w, &widths, outer, inner);
    for (i, (col, &(a, b))) in cols.iter().zip(&spans).enumerate() {
        let left = area.left + a - offset;
        let right = area.left + b - offset;
        let n = col.weights.len();
        let full = RECT {
            left,
            top,
            right,
            bottom: top + h,
        };
        if col.tabbed {
            // The tile keeps at least MIN_TILE whatever tab_bar_height says.
            let bar = tab_h.min(h - MIN_TILE).max(0);
            out.tab_bars[i] = Some(RECT {
                bottom: top + bar,
                ..full
            });
            let tile = RECT {
                top: top + bar,
                ..full
            };
            out.tiles.extend(std::iter::repeat_n(tile, n));
            continue;
        }
        let gap = fitted_gap(h, n, inner);
        let avail = h - gap * n.saturating_sub(1) as i32;
        if avail < n as i32 {
            // Not even 1 px per tile: stack them on the column rect rather
            // than emit empty rects (the same call dwindle makes for its tail).
            out.tiles.extend(std::iter::repeat_n(full, n));
            continue;
        }
        let weight = |w: f32| if w > 0.0 && w.is_finite() { w } else { 1.0 };
        let total: f32 = col.weights.iter().map(|&w| weight(w)).sum();
        let mut cum = 0.0;
        let mut prev = 0; // bottom of the previous tile, px from `top`, gaps excluded
        for (k, &w) in col.weights.iter().enumerate() {
            cum += weight(w);
            // Rounding the running sum (not each height) lands the last tile
            // exactly on the bottom; the clamp keeps every tile >= 1 px and
            // cannot invert because avail >= n.
            let edge = if k + 1 == n {
                avail
            } else {
                ((avail as f32 * cum / total).round() as i32)
                    .clamp(prev + 1, avail - (n - k - 1) as i32)
            };
            let y = top + prev + k as i32 * gap;
            out.tiles.push(RECT {
                top: y,
                bottom: y + edge - prev,
                ..full
            });
            prev = edge;
        }
    }
    out
}

/// Strict overlap: rects that only share an edge do not intersect, and an
/// empty rect intersects nothing.
fn intersects(a: RECT, b: RECT) -> bool {
    a.left.max(b.left) < a.right.min(b.right) && a.top.max(b.top) < a.bottom.min(b.bottom)
}

/// Full / Peek / Park for tile rect `r` on monitor `own` (FULL monitor rects;
/// `others` excludes `own`). Peek only when the part hanging off `own` lands
/// on no other monitor, so a scrolled strip never bleeds onto a neighbour.
pub(crate) fn classify(r: RECT, own: RECT, others: &[RECT]) -> Vis {
    if r.left >= own.left && r.top >= own.top && r.right <= own.right && r.bottom <= own.bottom {
        return Vis::Full;
    }
    if !intersects(r, own) {
        return Vis::Park;
    }
    // The part of r outside own as four bands: left and right at full height,
    // top and bottom between them. A band that does not exist comes out empty
    // and so intersects nothing.
    let mid = RECT {
        left: r.left.max(own.left),
        right: r.right.min(own.right),
        ..r
    };
    let outside = [
        RECT {
            right: r.right.min(own.left),
            ..r
        },
        RECT {
            left: r.left.max(own.right),
            ..r
        },
        RECT {
            bottom: r.bottom.min(own.top),
            ..mid
        },
        RECT {
            top: r.top.max(own.bottom),
            ..mid
        },
    ];
    if outside
        .iter()
        .any(|&p| others.iter().any(|&o| intersects(p, o)))
    {
        Vis::Park
    } else {
        Vis::Peek
    }
}

/// Squared distance between two rects; 0 when they touch or overlap.
fn rect_dist2(a: RECT, b: RECT) -> i64 {
    let dx = (b.left - a.right).max(a.left - b.right).max(0) as i64;
    let dy = (b.top - a.bottom).max(a.top - b.bottom).max(0) as i64;
    dx * dx + dy * dy
}

/// Top-left for a parked `w` x `h` window of monitor `own`: outside every
/// monitor in `all`, with `own` the nearest one. Windows gives an off-screen
/// window the DPI of its nearest monitor, so this keeps its DPI and spares a
/// WM_DPICHANGED relayout when it scrolls back.
pub(crate) fn park_spot(w: i32, h: i32, own: RECT, all: &[RECT]) -> (i32, i32) {
    let u = all.iter().fold(own, |u, m| RECT {
        left: u.left.min(m.left),
        top: u.top.min(m.top),
        right: u.right.max(m.right),
        bottom: u.bottom.max(m.bottom),
    });
    // Centred on own's span rather than flush with its edge: flush, a
    // neighbour sharing that edge is exactly as near as own, and Windows may
    // break that tie toward the neighbour.
    let cx = own.left + (own.right - own.left - w) / 2;
    let cy = own.top + (own.bottom - own.top - h) / 2;
    let below = (cx, u.bottom + PARK_MARGIN);
    let candidates = [
        below,
        (cx, u.top - PARK_MARGIN - h),
        (u.left - PARK_MARGIN - w, cy),
        (u.right + PARK_MARGIN, cy),
        // Own's diagonal corners. Needed when other monitors stick out past
        // own on every side (portrait | big | small, own = big, with a wide
        // window): each spot beside the union is then nearer a neighbour.
        (own.right + PARK_MARGIN, own.bottom + PARK_MARGIN),
        (own.left - PARK_MARGIN - w, own.bottom + PARK_MARGIN),
        (own.right + PARK_MARGIN, own.top - PARK_MARGIN - h),
        (own.left - PARK_MARGIN - w, own.top - PARK_MARGIN - h),
    ];
    // Every candidate is >= PARK_MARGIN from own, so "own strictly nearest"
    // also means clear of every monitor by more than the margin. A tie is
    // accepted only when no candidate wins outright.
    for strict in [true, false] {
        for (x, y) in candidates {
            let r = RECT {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            };
            let d = rect_dist2(r, own);
            let own_nearest = all.iter().all(|&m| {
                let dm = rect_dist2(r, m);
                m == own || dm > d || (!strict && dm == d)
            });
            if own_nearest {
                return (x, y);
            }
        }
    }
    below
}

/// Next entry of a cycle (Alt+R / Alt+Shift+R): the smallest preset above
/// `cur`, wrapping to the smallest overall, so the cycle runs low to high
/// whatever order the config lists them in. Wrapping to the first LISTED
/// preset got stuck on an unsorted list (`1.0, 0.5` never left 1.0). The 0.01
/// slack treats a value a hair off a preset (float noise, 0.333 vs 1/3) as
/// sitting on it.
pub(crate) fn next_preset(presets: &[f32], cur: f32) -> f32 {
    let it = presets.iter().copied();
    it.clone()
        .filter(|&p| p > cur + 0.01)
        .min_by(f32::total_cmp)
        .or_else(|| it.min_by(f32::total_cmp))
        .unwrap_or(cur)
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

    // ---- invariants -------------------------------------------------------
    // The bug these exist for: `master_stack` with many stack windows and a
    // large `inner_gap` (the config allows up to 500) produced rects with
    // `bottom < top`, and `workspace_layout` fed them straight to SetWindowPos
    // without checking (review B-11). Pure functions, so this sweep is free.

    /// Partial overlap = bug. Two IDENTICAL rects are a deliberate stack
    /// (monocle, and dwindle's tail once there is no room left to split).
    fn overlaps(a: RECT, b: RECT) -> bool {
        a != b && a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
    }

    /// Every rect is non-degenerate, inside the work area, and never partially
    /// on top of another.
    fn assert_sane(name: &str, area: RECT, rects: &[RECT], disjoint: bool) {
        for (i, c) in rects.iter().enumerate() {
            assert!(
                c.right > c.left && c.bottom > c.top,
                "{name}: rect {i} is inverted/empty: {:?}",
                (c.left, c.top, c.right, c.bottom)
            );
            assert!(
                c.left >= area.left
                    && c.top >= area.top
                    && c.right <= area.right
                    && c.bottom <= area.bottom,
                "{name}: rect {i} escapes the work area: {:?}",
                (c.left, c.top, c.right, c.bottom)
            );
        }
        if disjoint {
            for i in 0..rects.len() {
                for j in i + 1..rects.len() {
                    assert!(
                        !overlaps(rects[i], rects[j]),
                        "{name}: rects {i} and {j} overlap: {:?} {:?}",
                        (rects[i].left, rects[i].top, rects[i].right, rects[i].bottom),
                        (rects[j].left, rects[j].top, rects[j].right, rects[j].bottom)
                    );
                }
            }
        }
    }

    #[test]
    fn every_layout_stays_sane_across_counts_and_gaps() {
        // Small areas included on purpose: that is where the gap arithmetic
        // used to go negative.
        let areas = [
            r(0, 0, 1920, 1080),
            r(-1920, 0, 0, 1080), // monitor left of the primary (negative x)
            r(0, 28, 1280, 800),  // bar reserved
            r(0, 0, 320, 240),    // tiny
        ];
        for area in areas {
            for n in 1..=20usize {
                for outer in [0, 8, 40] {
                    for inner in [0, 4, 60, 200] {
                        let splits: Vec<f32> = vec![0.5; n];
                        let tag = format!("n={n} outer={outer} inner={inner}");
                        assert_sane(
                            &format!("master {tag}"),
                            area,
                            &master_stack(area, n, 0.55, outer, inner),
                            true,
                        );
                        assert_sane(
                            &format!("dwindle {tag}"),
                            area,
                            &dwindle_layout(area, n, outer, inner, &splits),
                            true,
                        );
                        assert_sane(
                            &format!("columns {tag}"),
                            area,
                            &columns_layout(area, n, outer, inner),
                            true,
                        );
                        assert_sane(
                            &format!("grid {tag}"),
                            area,
                            &grid_layout(area, n, outer, inner),
                            true,
                        );
                        // Monocle deliberately returns n copies of one rect.
                        assert_sane(
                            &format!("monocle {tag}"),
                            area,
                            &monocle_layout(area, n, outer),
                            false,
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn layouts_actually_place_every_window_on_a_normal_desktop() {
        // Guards the sweep above from passing vacuously: `assert_sane` is
        // trivially true for an empty result, so pin the ordinary case.
        let area = r(0, 28, 1920, 1080);
        for n in 1..=20usize {
            let splits = vec![0.5; n];
            assert_eq!(master_stack(area, n, 0.55, 8, 4).len(), n, "master n={n}");
            assert_eq!(
                dwindle_layout(area, n, 8, 4, &splits).len(),
                n,
                "dwindle n={n}"
            );
            // The spiral halves each time, so it runs out of room before the
            // other layouts do; past that the tail stacks rather than being
            // dropped or inverted. 14 is where that starts on a 1920x1052 work
            // area with these gaps — a number, not an adjective.
            assert_eq!(
                dwindle_tiles_fit(area, n, 8, 4, &splits),
                n <= 10,
                "dwindle distinct-tile limit moved at n={n}"
            );
            assert_eq!(columns_layout(area, n, 8, 4).len(), n, "columns n={n}");
            assert_eq!(grid_layout(area, n, 8, 4).len(), n, "grid n={n}");
            assert_eq!(monocle_layout(area, n, 8).len(), n, "monocle n={n}");
        }
    }

    #[test]
    fn master_stack_survives_an_absurd_gap() {
        // Was: `each` went <= 0 and every stack rect came out with bottom < top.
        let v = master_stack(r(0, 0, 800, 600), 12, 0.5, 0, 500);
        assert!(v.iter().all(|c| c.right > c.left && c.bottom > c.top));
    }

    #[test]
    fn dwindle_survives_an_absurd_gap() {
        let splits = vec![0.5; 12];
        let v = dwindle_layout(r(0, 0, 800, 600), 12, 0, 500, &splits);
        assert!(v.iter().all(|c| c.right > c.left && c.bottom > c.top));
    }

    #[test]
    fn resize_dwindle_noop_when_single() {
        let mut splits = vec![0.5];
        resize_dwindle(&mut splits, r(0, 0, 200, 100), 1, 0, 0, 0, r(0, 0, 50, 50));
        assert_eq!(splits, vec![0.5]);
    }

    // ---- scrolling strip --------------------------------------------------
    // The strip gets its own invariants: its rects leave the work area on
    // purpose, so `assert_sane` above does not apply.

    fn col(width: f32, weights: &[f32]) -> StripCol {
        StripCol {
            width,
            weights: weights.to_vec(),
            tabbed: false,
            active: 0,
        }
    }

    fn tabbed(width: f32, tiles: usize) -> StripCol {
        StripCol {
            tabbed: true,
            ..col(width, &vec![1.0; tiles])
        }
    }

    #[test]
    fn strip_n_columns_of_one_nth_fill_the_view() {
        // outer 8 / inner 16 makes view_w - 2*outer + inner = 1920, which every
        // n here divides, so the identity is exact rather than "close".
        for n in [1usize, 2, 3, 4, 5, 6, 8] {
            let spans = strip_spans(1920, &vec![1.0 / n as f32; n], 8, 16);
            assert_eq!(spans[0].0, 8, "n={n}");
            assert_eq!(spans[n - 1].1 + 8, 1920, "n={n}");
        }
        // Otherwise each column rounds on its own: off by at most n/2 px.
        for view_w in [1366, 1920, 2560, 3440] {
            for (outer, inner) in [(0, 0), (8, 4), (13, 7)] {
                for n in 1..=7usize {
                    let spans = strip_spans(view_w, &vec![1.0 / n as f32; n], outer, inner);
                    let err = (spans[n - 1].1 + outer - view_w).abs();
                    assert!(err <= n as i32 / 2, "view={view_w} n={n} err={err}");
                }
            }
        }
    }

    #[test]
    fn col_width_inverts_col_px() {
        for (view_w, outer, inner) in [(1920, 8, 8), (2560, 0, 0), (3440, 13, 7)] {
            for w in [0.1f32, 0.333, 0.5, 0.667, 1.0] {
                let px = col_px(view_w, w, outer, inner);
                let back = col_width(view_w, px, outer, inner);
                assert_eq!(
                    col_px(view_w, back, outer, inner),
                    px,
                    "view={view_w} w={w}"
                );
                assert!((back - w).abs() < 0.002, "view={view_w} w={w} back={back}");
            }
        }
    }

    #[test]
    fn strip_spans_are_monotonic_and_gapped() {
        let widths = [0.333, 0.5, 0.667, 1.0, 0.1, 0.25];
        for view_w in [320, 1280, 1920, 3440] {
            for (outer, inner) in [(0, 0), (8, 4), (40, 200)] {
                let spans = strip_spans(view_w, &widths, outer, inner);
                assert_eq!(spans.len(), widths.len());
                assert_eq!(spans[0].0, outer);
                for (i, s) in spans.iter().enumerate() {
                    assert!(s.1 > s.0, "view={view_w} col {i} empty: {s:?}");
                    if i > 0 {
                        assert_eq!(s.0, spans[i - 1].1 + inner, "view={view_w} col {i}");
                    }
                }
            }
        }
    }

    #[test]
    fn clamp_offset_left_aligns_short_strips_and_bounds_long_ones() {
        let short = strip_spans(1000, &[0.25, 0.25], 10, 10);
        assert_eq!(clamp_offset(1000, &short, 10, 300), 0);
        assert_eq!(clamp_offset(1000, &short, 10, -50), 0);
        assert_eq!(clamp_offset(1000, &[], 10, 300), 0);
        // Five halves: spans end at 2475, so the strip is 2485 wide.
        let long = strip_spans(1000, &[0.5; 5], 10, 10);
        assert_eq!(long[4], (1990, 2475));
        assert_eq!(clamp_offset(1000, &long, 10, -50), 0);
        assert_eq!(clamp_offset(1000, &long, 10, 700), 700);
        assert_eq!(clamp_offset(1000, &long, 10, 99_999), 1485);
    }

    #[test]
    fn scroll_to_show_honours_each_center_mode() {
        // view 1000, outer/inner 10: halves are 485 px, spans (10,495)
        // (505,990) (1000,1485) (1495,1980) (1990,2475); max offset 1485.
        let s = strip_spans(1000, &[0.5; 5], 10, 10);
        let show = |focus, offset, mode| scroll_to_show(1000, &s, 10, focus, offset, mode);

        // Never: minimal scroll, nothing if already fully visible.
        assert_eq!(show(1, 0, Center::Never), 0);
        assert_eq!(show(2, 0, Center::Never), 495); // right-aligned
        assert_eq!(show(0, 495, Center::Never), 0); // left-aligned
        assert_eq!(show(4, 0, Center::Never), 1485);

        // Always: centred, but clamped at the strip ends.
        assert_eq!(show(2, 0, Center::Always), 742);
        assert_eq!(show(0, 700, Center::Always), 0);
        assert_eq!(show(4, 0, Center::Always), 1485);

        // OnOverflow: leave a visible column alone, otherwise centre it.
        assert_eq!(show(1, 495, Center::OnOverflow), 495);
        assert_eq!(show(2, 495, Center::OnOverflow), 495);
        assert_eq!(show(3, 495, Center::OnOverflow), 1237);

        // Out of range focus only clamps.
        assert_eq!(show(9, 99_999, Center::Never), 1485);
        assert_eq!(scroll_to_show(1000, &[], 10, 0, 300, Center::Always), 0);

        // A stale offset past the end (the strip shrank) is judged as the
        // clamped view on screen: a column shown there stays put.
        assert_eq!(show(3, 99_999, Center::OnOverflow), 1485);
        let thirds = strip_spans(1000, &[0.3; 5], 0, 0);
        assert_eq!(scroll_to_show(1000, &thirds, 0, 2, 800, Center::Never), 500);
    }

    #[test]
    fn scroll_to_show_aligns_an_oversize_column_left() {
        // Middle column is 1.6 views wide: span (505,2079).
        let s = strip_spans(1000, &[0.5, 1.6, 0.5], 10, 10);
        assert_eq!(s[1], (505, 2079));
        for mode in [Center::Never, Center::Always, Center::OnOverflow] {
            assert_eq!(scroll_to_show(1000, &s, 10, 1, 0, mode), 495, "{mode:?}");
            assert_eq!(scroll_to_show(1000, &s, 10, 1, 1500, mode), 495, "{mode:?}");
        }
    }

    #[test]
    fn strip_exactly_view_width_never_scrolls_and_full_width_is_not_oversize() {
        // Two halves fill the view exactly (strip 1000 wide): no mode may
        // scroll, whatever stale offset it starts from.
        let s = strip_spans(1000, &[0.5, 0.5], 10, 10);
        assert_eq!(s[1].1 + 10, 1000);
        for mode in [Center::Never, Center::Always, Center::OnOverflow] {
            for (focus, offset) in [(0, 0), (1, 0), (1, 300), (0, -40)] {
                assert_eq!(scroll_to_show(1000, &s, 10, focus, offset, mode), 0);
            }
        }
        // A 1.0 column is exactly view - 2*outer, so it takes the normal path
        // (not the oversize left-align) and every mode lands it flush.
        let s = strip_spans(1000, &[1.0, 1.0], 10, 10);
        assert_eq!(s, vec![(10, 990), (1000, 1980)]);
        for mode in [Center::Never, Center::Always, Center::OnOverflow] {
            assert_eq!(scroll_to_show(1000, &s, 10, 1, 0, mode), 990, "{mode:?}");
            assert_eq!(scroll_to_show(1000, &s, 10, 0, 990, mode), 0, "{mode:?}");
        }
        // Edge-sharing neighbours: one beside own only matters on that side.
        let own = r(0, 0, 1920, 1080);
        let right = r(1920, 0, 3840, 1080);
        assert_eq!(classify(r(100, 900, 500, 1300), own, &[right]), Vis::Peek);
        assert_eq!(classify(r(1800, 900, 2100, 1300), own, &[right]), Vis::Park);
    }

    #[test]
    fn strip_layout_tiles_stay_in_their_column_and_the_area() {
        let cols = [
            col(0.5, &[1.0]),
            col(0.333, &[1.0, 2.0, 0.5]),
            col(1.0, &[1.0, 1.0]),
            col(0.25, &[1.0; 6]),
        ];
        let areas = [
            r(0, 0, 1920, 1080),
            r(-1920, 28, 0, 1080),
            r(0, 0, 320, 240),
        ];
        for area in areas {
            for (outer, inner) in [(0, 0), (8, 4), (40, 200)] {
                for offset in [0, 333, 2000] {
                    let tag = format!(
                        "area={:?} outer={outer} inner={inner} offset={offset}",
                        (area.left, area.top)
                    );
                    let out = strip_layout(area, &cols, offset, outer, inner, 26);
                    let widths: Vec<f32> = cols.iter().map(|c| c.width).collect();
                    let spans = strip_spans(area.right - area.left, &widths, outer, inner);
                    assert_eq!(out.tab_bars, vec![None; cols.len()], "{tag}");
                    assert_eq!(out.tiles.len(), 12, "{tag}");
                    let mut tiles = out.tiles.iter();
                    for (c, &(a, b)) in cols.iter().zip(&spans) {
                        let mut prev_bottom = area.top + outer;
                        for k in 0..c.weights.len() {
                            let t = *tiles.next().unwrap();
                            assert_eq!(t.left, area.left + a - offset, "{tag}");
                            assert_eq!(t.right, area.left + b - offset, "{tag}");
                            assert!(t.bottom > t.top, "{tag}: empty tile {t:?}");
                            assert!(t.top >= prev_bottom, "{tag}: tiles overlap");
                            prev_bottom = t.bottom;
                            if k + 1 == c.weights.len() {
                                assert_eq!(t.bottom, area.bottom - outer, "{tag}");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn strip_layout_fills_the_view_and_follows_the_offset() {
        let area = r(100, 0, 2020, 1080);
        let cols = vec![col(1.0 / 3.0, &[1.0]); 3];
        let out = strip_layout(area, &cols, 0, 8, 16, 26);
        assert_eq!(out.tiles[0], r(108, 8, 732, 1072));
        assert_eq!(out.tiles[2].right, 2012);
        let moved = strip_layout(area, &cols, 50, 8, 16, 26);
        for (a, b) in out.tiles.iter().zip(&moved.tiles) {
            assert_eq!((b.left, b.right), (a.left - 50, a.right - 50));
            assert_eq!((b.top, b.bottom), (a.top, a.bottom));
        }
    }

    #[test]
    fn strip_layout_honours_weights() {
        // 400 px of tiles: 1:3 -> 100 / 300.
        let out = strip_layout(r(0, 0, 1000, 400), &[col(0.5, &[1.0, 3.0])], 0, 0, 0, 26);
        assert_eq!(out.tiles, vec![r(0, 0, 500, 100), r(0, 100, 500, 400)]);
        // Same with a 20 px gap between them (the column is 490 wide now).
        let out = strip_layout(r(0, 0, 1000, 420), &[col(0.5, &[1.0, 3.0])], 0, 0, 20, 26);
        assert_eq!(out.tiles, vec![r(0, 0, 490, 100), r(0, 120, 490, 420)]);
        // Non-positive and NaN weights count as 1.0.
        let out = strip_layout(
            r(0, 0, 1000, 400),
            &[col(0.5, &[0.0, -1.0, f32::NAN, 1.0])],
            0,
            0,
            0,
            26,
        );
        let heights: Vec<i32> = out.tiles.iter().map(|t| t.bottom - t.top).collect();
        assert_eq!(heights, vec![100; 4]);
        // A tiny weight still gets a real tile.
        let out = strip_layout(
            r(0, 0, 1000, 400),
            &[col(0.5, &[0.0001, 1000.0])],
            0,
            0,
            0,
            26,
        );
        assert!(out.tiles.iter().all(|t| t.bottom > t.top));
    }

    #[test]
    fn strip_layout_tabbed_column_shares_one_rect_below_its_bar() {
        let area = r(0, 0, 1000, 800);
        let out = strip_layout(area, &[tabbed(0.5, 3), col(0.5, &[1.0])], 0, 10, 10, 26);
        let s = strip_spans(1000, &[0.5, 0.5], 10, 10);
        assert_eq!(out.tab_bars[0], Some(r(s[0].0, 10, s[0].1, 36)));
        assert_eq!(out.tab_bars[1], None);
        assert_eq!(out.tiles.len(), 4);
        assert_eq!(out.tiles[..3], [r(s[0].0, 36, s[0].1, 790); 3]);
        assert_eq!(out.tiles[3], r(s[1].0, 10, s[1].1, 790));
        // An absurd bar height leaves the tile MIN_TILE tall.
        let out = strip_layout(area, &[tabbed(0.5, 2)], 0, 10, 10, 5000);
        assert_eq!(out.tiles[0].bottom - out.tiles[0].top, MIN_TILE);
        assert_eq!(out.tab_bars[0].unwrap().bottom, out.tiles[0].top);
    }

    #[test]
    fn strip_layout_degenerate_area_is_empty() {
        let out = strip_layout(
            r(0, 0, 10, 10),
            &[col(0.5, &[1.0]), tabbed(0.5, 2)],
            0,
            20,
            0,
            26,
        );
        assert!(out.tiles.is_empty());
        assert_eq!(out.tab_bars, vec![None, None]);
    }

    #[test]
    fn classify_full_peek_and_park() {
        let own = r(0, 0, 1920, 1080);
        let right = r(1920, 0, 3840, 1080);
        assert_eq!(classify(r(100, 100, 500, 500), own, &[right]), Vis::Full);
        assert_eq!(classify(own, own, &[right]), Vis::Full);
        // Hangs off the left edge, nothing there: the Niri peek.
        assert_eq!(classify(r(-300, 100, 200, 500), own, &[right]), Vis::Peek);
        // Hangs onto the right-hand monitor: bleed, so park.
        assert_eq!(classify(r(1700, 100, 2200, 500), own, &[right]), Vis::Park);
        // Monitor below: bleeding down parks too.
        let below = r(0, 1080, 1920, 2160);
        assert_eq!(classify(r(100, 900, 500, 1300), own, &[below]), Vis::Park);
        // Fully off own, or only touching its edge.
        assert_eq!(classify(r(2000, 100, 2500, 500), own, &[]), Vis::Park);
        assert_eq!(classify(r(1920, 0, 2400, 500), own, &[]), Vis::Park);
    }

    #[test]
    fn classify_staggered_neighbour_is_tested_geometrically() {
        let own = r(0, 0, 1920, 1080);
        let lower_right = r(1920, 600, 3840, 1680);
        // The overhang (y 100..500) misses the neighbour (from y 600): peek.
        assert_eq!(
            classify(r(1700, 100, 2200, 500), own, &[lower_right]),
            Vis::Peek
        );
        // The overhang (y 500..900) lands on it: park.
        assert_eq!(
            classify(r(1700, 500, 2200, 900), own, &[lower_right]),
            Vis::Park
        );
        // Sharing only an edge (overhang ends at y 600) is not a bleed.
        assert_eq!(
            classify(r(1700, 100, 2200, 600), own, &[lower_right]),
            Vis::Peek
        );
    }

    #[test]
    fn park_spot_is_off_every_monitor_and_nearest_its_own() {
        let arrangements: [&[RECT]; 10] = [
            &[r(0, 0, 1920, 1080)],
            &[r(0, 0, 1920, 1080), r(1920, 0, 3840, 1080)],
            // side by side, vertically offset and mixed size
            &[r(0, 0, 1920, 1080), r(1920, -400, 4480, 1040)],
            // stacked vertically
            &[r(0, 0, 1920, 1080), r(0, -1080, 1920, 0)],
            // L-shaped
            &[
                r(0, 0, 1920, 1080),
                r(1920, 0, 3840, 1080),
                r(0, 1080, 1920, 2160),
            ],
            // portrait | big | small, negative coordinates: needs own's corners
            &[
                r(-1080, -300, 0, 1620),
                r(0, 0, 2560, 1440),
                r(2560, 200, 3840, 1224),
            ],
            &[r(-1920, 0, 0, 1080), r(0, 0, 1920, 1080)],
            &[
                r(-1920, 0, 0, 1080),
                r(0, 0, 1920, 1080),
                r(1920, 0, 3840, 1080),
            ],
            // 2x2 grid
            &[
                r(0, 0, 1920, 1080),
                r(1920, 0, 3840, 1080),
                r(0, 1080, 1920, 2160),
                r(1920, 1080, 3840, 2160),
            ],
            // laptop centred directly below a bigger external monitor
            &[r(0, 0, 2560, 1440), r(320, 1440, 2240, 2520)],
        ];
        for all in arrangements {
            for &own in all {
                let (ow, oh) = (own.right - own.left, own.bottom - own.top);
                for (w, h) in [(ow / 2, oh), (ow - 16, oh - 16), (ow, oh), (100, 100)] {
                    let (x, y) = park_spot(w, h, own, all);
                    let p = r(x, y, x + w, y + h);
                    let tag = format!(
                        "own={:?} size={w}x{h} spot={:?}",
                        (own.left, own.top),
                        (x, y)
                    );
                    assert!(x.abs() < 32_000 && y.abs() < 32_000, "{tag}");
                    for &m in all {
                        assert!(!intersects(p, m), "{tag}: on monitor {m:?}");
                        assert!(
                            rect_dist2(p, own) <= rect_dist2(p, m),
                            "{tag}: nearer {m:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn park_spot_prefers_below_and_wins_outright_when_it_can() {
        // Single monitor: straight below, centred, PARK_MARGIN clear.
        let own = r(0, 0, 1920, 1080);
        assert_eq!(park_spot(960, 1080, own, &[own]), (480, 1080 + PARK_MARGIN));
        // Right of a side-by-side pair: below again, and strictly nearer own
        // than the left monitor (centring keeps it off the shared edge).
        let left = r(0, 0, 1920, 1080);
        let right = r(1920, 0, 3840, 1080);
        let (x, y) = park_spot(1904, 1064, right, &[left, right]);
        let p = r(x, y, x + 1904, y + 1064);
        assert_eq!(y, 1080 + PARK_MARGIN);
        assert!(rect_dist2(p, right) < rect_dist2(p, left));
        // No monitors listed: falls back on own alone.
        assert_eq!(park_spot(100, 100, own, &[]), (910, 1080 + PARK_MARGIN));
    }

    #[test]
    fn next_preset_cycles() {
        let p = [0.333, 0.5, 0.667, 1.0];
        assert_eq!(next_preset(&p, 0.333), 0.5);
        assert_eq!(next_preset(&p, 0.5), 0.667);
        assert_eq!(next_preset(&p, 0.667), 1.0);
        assert_eq!(next_preset(&p, 1.0), 0.333); // wraps to the smallest
        assert_eq!(next_preset(&p, 1.0 / 3.0), 0.5); // float noise is "on" 0.333
        assert_eq!(next_preset(&p, 0.4), 0.5); // between presets: next one up
        assert_eq!(next_preset(&[1.0, 0.5], 0.6), 1.0); // order does not matter
                                                        // An unsorted list still cycles through every entry: this used to wrap
                                                        // to the first listed (1.0) and stick there.
        assert_eq!(next_preset(&[1.0, 0.5], 1.0), 0.5);
        assert_eq!(next_preset(&[0.667, 0.333, 1.0, 0.5], 1.0), 0.333);
        // Above every preset (a dragged width) wraps too.
        assert_eq!(next_preset(&p, 1.2), 0.333);
        assert_eq!(next_preset(&[], 0.42), 0.42);
    }
}

//! The draw list: everything a renderer needs, in physical pixels, in paint
//! order. Items are grouped by node; `damage` compares two lists group by
//! group and returns the pixels that changed.

use std::collections::HashMap;
use std::sync::Arc;

use crate::color::Color;
use crate::element::NodeKey;
use crate::geom::{Edges, PointF, Radii, RectF, RectPx};
use crate::style::TextAlign;

#[derive(Clone, Debug, PartialEq)]
pub struct DrawList {
    pub width: u32,
    pub height: u32,
    pub items: Vec<DrawItem>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DrawItem {
    /// The node that produced the item. A node's group is every item with
    /// its key, wherever it sits in the list (a node's `PopClip` comes after
    /// its children).
    pub node: NodeKey,
    /// Pixels the item can touch. Leaf items: the shape plus 1 px of
    /// anti-aliasing; for `Shadow`, `shadow_bounds`; for `Text`, its rect
    /// (the renderer clips every leaf item, text included, to its bounds,
    /// so overhangs never leave pixels a partial redraw would miss). For
    /// `PushClip` / `PushOpacity`: the whole group it opens. Renderers skip
    /// leaf items whose bounds miss the redraw region but always process
    /// push and pop items so the stacks stay balanced.
    pub bounds: RectF,
    pub cmd: DrawCmd,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DrawCmd {
    /// Outer box shadow: `rect` already offset and spread, Gaussian blur
    /// with standard deviation `blur / 2` (CSS). The renderer cuts its kernel
    /// at 3 standard deviations, which is what `shadow_bounds` allows for.
    Shadow {
        rect: RectF,
        radii: Radii,
        blur: f32,
        color: Color,
    },
    Fill {
        rect: RectF,
        radii: Radii,
        fill: Fill,
    },
    /// Border drawn inside `rect` (border-box), uniform colour.
    Border {
        rect: RectF,
        radii: Radii,
        widths: Edges<f32>,
        color: Color,
    },
    /// One line of text inside `rect`, vertically centred, aligned by
    /// `align`. `text` is final (ellipsis already applied).
    Text {
        rect: RectF,
        text: Arc<str>,
        style: TextStyle,
        color: Color,
        align: TextAlign,
    },
    /// A decoded image from the renderer's cache, scaled to `rect`.
    Image {
        rect: RectF,
        key: Arc<str>,
        radii: Radii,
    },
    PushClip {
        rect: RectF,
        radii: Radii,
    },
    PopClip,
    /// Group opacity 0..=1 (drawn to a layer, then blended).
    PushOpacity(f32),
    PopOpacity,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fill {
    Solid(Color),
    Linear { start: PointF, end: PointF, stops: Vec<(f32, Color)> },
}

/// Text style in physical pixels. Also the key the renderer caches text
/// layouts under, so measuring and drawing agree.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    /// Family names in preference order; the renderer falls back through
    /// the system (emoji, symbols) after the last one.
    pub families: Arc<[Arc<str>]>,
    pub size_px: f32,
    pub weight: u16,
    pub italic: bool,
    /// Physical px added after every character (`letter-spacing`).
    pub letter_spacing: f32,
    /// `font-variant-numeric: tabular-nums`: digits all one width, so a
    /// ticking clock does not shift its neighbours.
    pub tabular_nums: bool,
}

/// Bounds of a `Shadow` item: `rect` (already offset and spread) grown by
/// 3 standard deviations of the blur (`1.5 * blur`, rounded up) plus 1 px.
pub fn shadow_bounds(rect: RectF, blur: f32) -> RectF {
    rect.outset((1.5 * blur.max(0.0)).ceil() + 1.0)
}

/// Everything one node drew.
struct Group<'a> {
    items: Vec<&'a DrawItem>,
    bounds: RectF,
}

/// Groups in order of first appearance.
fn groups(list: &DrawList) -> (Vec<NodeKey>, HashMap<NodeKey, Group<'_>>) {
    let mut order = Vec::new();
    let mut map: HashMap<NodeKey, Group> = HashMap::new();
    for item in &list.items {
        let g = map.entry(item.node).or_insert_with(|| {
            order.push(item.node);
            Group { items: Vec::new(), bounds: RectF::default() }
        });
        g.items.push(item);
        g.bounds = g.bounds.union(&item.bounds);
    }
    (order, map)
}

/// Push and pop items grow with their children, so only their command counts
/// as a change; leaf items compare whole.
fn same_item(a: &DrawItem, b: &DrawItem) -> bool {
    match (&a.cmd, &b.cmd) {
        (DrawCmd::PushClip { .. } | DrawCmd::PopClip | DrawCmd::PushOpacity(_) | DrawCmd::PopOpacity, _) => {
            a.cmd == b.cmd
        }
        _ => a == b,
    }
}

/// Pixels that differ between `prev` and `next`, or None when the two lists
/// draw the same thing (then no frame is needed). `prev` None (first frame,
/// size change) = the whole surface.
///
/// Rule: group both lists' items by node in order; a group that was added,
/// removed or differs in any item contributes the union of its old and new
/// bounds; groups whose paint order changed relative to an overlapping group
/// count as changed. The result is rounded outwards and cut to the surface.
pub fn damage(prev: Option<&DrawList>, next: &DrawList) -> Option<RectPx> {
    let surface = RectPx::new(0, 0, next.width as i32, next.height as i32);
    let whole = || (!surface.is_empty()).then_some(surface);
    let Some(prev) = prev else { return whole() };
    if prev.width != next.width || prev.height != next.height {
        return whole();
    }
    let (order_p, gp) = groups(prev);
    let (order_n, gn) = groups(next);
    let mut changed: HashMap<NodeKey, RectF> = HashMap::new();
    let mut mark = |key: NodeKey, r: RectF| {
        let e = changed.entry(key).or_default();
        *e = e.union(&r);
    };
    for (key, g) in &gp {
        match gn.get(key) {
            None => mark(*key, g.bounds),
            Some(n) => {
                let same = g.items.len() == n.items.len() && g.items.iter().zip(&n.items).all(|(a, b)| same_item(a, b));
                if !same {
                    mark(*key, g.bounds.union(&n.bounds));
                }
            }
        }
    }
    for (key, g) in &gn {
        if !gp.contains_key(key) {
            mark(*key, g.bounds);
        }
    }

    // Paint order: a group that moved past an overlapping group changed.
    let common_p: Vec<NodeKey> = order_p.iter().copied().filter(|k| gn.contains_key(k)).collect();
    let common_n: Vec<NodeKey> = order_n.iter().copied().filter(|k| gp.contains_key(k)).collect();
    if common_p != common_n {
        let pos_n: HashMap<NodeKey, usize> = common_n.iter().enumerate().map(|(i, k)| (*k, i)).collect();
        for (i, a) in common_p.iter().enumerate() {
            for b in &common_p[i + 1..] {
                if pos_n[b] < pos_n[a] {
                    let (ba, bb) = (gn[a].bounds, gn[b].bounds);
                    if ba.intersect(&bb).is_some() {
                        mark(*a, gp[a].bounds.union(&ba));
                        mark(*b, gp[b].bounds.union(&bb));
                    }
                }
            }
        }
    }

    let mut out = RectPx::default();
    for r in changed.values() {
        out = out.union(&RectPx::outward(r));
    }
    out.intersect(&surface)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::TextAlign;

    fn key(n: u64) -> NodeKey {
        NodeKey(n)
    }

    fn fill(node: u64, x: f32, c: u8) -> DrawItem {
        let rect = RectF::new(x, 0.0, 10.0, 10.0);
        DrawItem {
            node: key(node),
            bounds: rect.outset(1.0),
            cmd: DrawCmd::Fill { rect, radii: Radii::ZERO, fill: Fill::Solid(Color::rgba8(c, 0, 0, 255)) },
        }
    }

    fn list(items: Vec<DrawItem>) -> DrawList {
        DrawList { width: 100, height: 20, items }
    }

    #[test]
    fn identical_lists_need_no_frame() {
        let a = list(vec![fill(1, 0.0, 1), fill(2, 20.0, 2)]);
        assert_eq!(damage(Some(&a), &a.clone()), None);
    }

    #[test]
    fn first_frame_and_size_change_are_whole_surface() {
        let a = list(vec![fill(1, 0.0, 1)]);
        assert_eq!(damage(None, &a), Some(RectPx::new(0, 0, 100, 20)));
        let mut b = a.clone();
        b.width = 120;
        assert_eq!(damage(Some(&a), &b), Some(RectPx::new(0, 0, 120, 20)));
    }

    #[test]
    fn one_changed_node_gives_its_old_and_new_bounds() {
        let a = list(vec![fill(1, 0.0, 1), fill(2, 20.0, 2), fill(3, 40.0, 3)]);
        let b = list(vec![fill(1, 0.0, 1), fill(2, 24.0, 2), fill(3, 40.0, 3)]);
        // Old bounds x 19..31, new 23..35: union 19..35, y -1..11 cut to 0..11.
        assert_eq!(damage(Some(&a), &b), Some(RectPx::new(19, 0, 35, 11)));
    }

    #[test]
    fn removed_and_added_groups_count() {
        let a = list(vec![fill(1, 0.0, 1), fill(2, 20.0, 2)]);
        let b = list(vec![fill(1, 0.0, 1)]);
        assert_eq!(damage(Some(&a), &b), Some(RectPx::new(19, 0, 31, 11)));
        assert_eq!(damage(Some(&b), &a), Some(RectPx::new(19, 0, 31, 11)));
    }

    #[test]
    fn reordered_overlapping_groups_are_damaged_disjoint_ones_are_not() {
        let a = list(vec![fill(1, 0.0, 1), fill(2, 5.0, 2)]);
        let b = list(vec![fill(2, 5.0, 2), fill(1, 0.0, 1)]);
        assert_eq!(damage(Some(&a), &b), Some(RectPx::new(0, 0, 16, 11)));
        let c = list(vec![fill(1, 0.0, 1), fill(2, 50.0, 2)]);
        let d = list(vec![fill(2, 50.0, 2), fill(1, 0.0, 1)]);
        assert_eq!(damage(Some(&c), &d), None);
    }

    #[test]
    fn push_bounds_alone_are_not_a_change() {
        let push = |b: RectF| DrawItem {
            node: key(9),
            bounds: b,
            cmd: DrawCmd::PushClip { rect: RectF::new(0.0, 0.0, 50.0, 20.0), radii: Radii::ZERO },
        };
        let pop = |b: RectF| DrawItem { node: key(9), bounds: b, cmd: DrawCmd::PopClip };
        let a =
            list(vec![push(RectF::new(0.0, 0.0, 10.0, 10.0)), fill(1, 0.0, 1), pop(RectF::new(0.0, 0.0, 10.0, 10.0))]);
        let b =
            list(vec![push(RectF::new(0.0, 0.0, 30.0, 10.0)), fill(1, 0.0, 1), pop(RectF::new(0.0, 0.0, 30.0, 10.0))]);
        assert_eq!(damage(Some(&a), &b), None);
    }

    #[test]
    fn text_change_damages_only_the_text_rect() {
        let text = |s: &str| DrawItem {
            node: key(5),
            bounds: RectF::new(10.0, 4.0, 30.0, 12.0),
            cmd: DrawCmd::Text {
                rect: RectF::new(10.0, 4.0, 30.0, 12.0),
                text: Arc::from(s),
                style: TextStyle {
                    families: Arc::from(vec![Arc::<str>::from("Segoe UI")]),
                    size_px: 12.0,
                    weight: 400,
                    italic: false,
                    letter_spacing: 0.0,
                    tabular_nums: false,
                },
                color: Color::WHITE,
                align: TextAlign::Start,
            },
        };
        let a = list(vec![fill(1, 0.0, 1), text("12:00")]);
        let b = list(vec![fill(1, 0.0, 1), text("12:01")]);
        assert_eq!(damage(Some(&a), &b), Some(RectPx::new(10, 4, 40, 16)));
    }

    #[test]
    fn shadow_bounds_grow_by_three_sigma_and_a_pixel() {
        let r = shadow_bounds(RectF::new(10.0, 10.0, 20.0, 20.0), 4.0);
        assert_eq!(r, RectF::new(3.0, 3.0, 34.0, 34.0));
    }
}

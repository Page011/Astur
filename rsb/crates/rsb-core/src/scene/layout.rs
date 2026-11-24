//! Layout: Taffy flexbox per zone, then the zones placed in the bar.

use std::collections::HashMap;
use std::sync::Arc;

use taffy::{
    AlignContent, AlignItems, AvailableSpace, BoxSizing, Dimension, Display, LengthPercentage, LengthPercentageAuto,
    NodeId, Overflow, Point, Rect, Size, Style, TaffyTree,
};

use super::{Laid, Node, Scene, bar_rect, text_style};
use crate::element::{ElementKind, NodeKey};
use crate::geom::{Edges, RectF, SizeF};
use crate::layout::TextMeasure;
use crate::style::{Align, ComputedStyle, Dim, FlexDirection, Justify};

/// Border widths in physical px: whole pixels, at least one when set.
pub(super) fn border_px(w: &Edges<f32>, scale: f32) -> Edges<f32> {
    let one = |v: f32| if v > 0.0 { (v * scale).round().max(1.0) } else { 0.0 };
    Edges { top: one(w.top), right: one(w.right), bottom: one(w.bottom), left: one(w.left) }
}

fn dimension(d: Dim, scale: f32) -> Dimension {
    match d {
        Dim::Auto => Dimension::auto(),
        Dim::Px(v) => Dimension::length(v * scale),
        Dim::Percent(p) => Dimension::percent(p / 100.0),
    }
}

fn limit(d: Dim, scale: f32) -> LengthPercentageAuto {
    match d {
        Dim::Auto => LengthPercentageAuto::auto(),
        Dim::Px(v) => LengthPercentageAuto::length(v * scale),
        Dim::Percent(p) => LengthPercentageAuto::percent(p / 100.0),
    }
}

fn lp(v: f32) -> LengthPercentage {
    LengthPercentage::length(v)
}

/// A CSS length in physical px, whole pixels: spacing that is a fraction of a
/// pixel would put text and box edges between pixels and blur them.
fn whole(css_px: f32, scale: f32) -> f32 {
    (css_px * scale).round()
}

fn taffy_style(s: &ComputedStyle, scale: f32, shrinkable: bool) -> Style {
    let mut st = Style {
        display: Display::Flex,
        box_sizing: BoxSizing::BorderBox,
        flex_direction: match s.flex_direction {
            FlexDirection::Row => taffy::FlexDirection::Row,
            FlexDirection::Column => taffy::FlexDirection::Column,
        },
        flex_grow: s.flex_grow,
        flex_shrink: s.flex_shrink,
        justify_content: match s.justify_content {
            Justify::Start => AlignContent::START,
            Justify::End => AlignContent::END,
            Justify::Center => AlignContent::CENTER,
            Justify::SpaceBetween => AlignContent::SPACE_BETWEEN,
            Justify::SpaceAround => AlignContent::SPACE_AROUND,
            Justify::SpaceEvenly => AlignContent::SPACE_EVENLY,
        },
        align_items: match s.align_items {
            Align::Start => AlignItems::START,
            Align::End => AlignItems::END,
            Align::Center => AlignItems::CENTER,
            Align::Stretch => AlignItems::STRETCH,
        },
        gap: Size { width: lp(whole(s.gap, scale)), height: lp(whole(s.gap, scale)) },
        size: Size { width: dimension(s.width, scale), height: dimension(s.height, scale) },
        min_size: Size { width: limit(s.min_width, scale), height: limit(s.min_height, scale) },
        max_size: Size { width: limit(s.max_width, scale), height: limit(s.max_height, scale) },
        margin: Rect {
            left: s.margin.left.map_or(LengthPercentageAuto::auto(), |v| LengthPercentageAuto::length(whole(v, scale))),
            right: s
                .margin
                .right
                .map_or(LengthPercentageAuto::auto(), |v| LengthPercentageAuto::length(whole(v, scale))),
            top: s.margin.top.map_or(LengthPercentageAuto::auto(), |v| LengthPercentageAuto::length(whole(v, scale))),
            bottom: s
                .margin
                .bottom
                .map_or(LengthPercentageAuto::auto(), |v| LengthPercentageAuto::length(whole(v, scale))),
        },
        padding: Rect {
            left: lp(whole(s.padding.left, scale)),
            right: lp(whole(s.padding.right, scale)),
            top: lp(whole(s.padding.top, scale)),
            bottom: lp(whole(s.padding.bottom, scale)),
        },
        ..Style::default()
    };
    let b = border_px(&s.border_width, scale);
    st.border = Rect { left: lp(b.left), right: lp(b.right), top: lp(b.top), bottom: lp(b.bottom) };
    if s.clip {
        st.overflow = Point { x: Overflow::Hidden, y: Overflow::Hidden };
    }
    if shrinkable && matches!(s.min_width, Dim::Auto) {
        st.min_size.width = LengthPercentageAuto::length(0.0);
    }
    st
}

/// Taffy tree of one zone and the maps back to arena indexes.
struct Lay<'a> {
    arena: &'a [Node],
    scale: f32,
    tree: TaffyTree<Size<f32>>,
    ids: Vec<Option<NodeId>>,
    text: &'a mut dyn TextMeasure,
    cache: &'a mut HashMap<NodeKey, (Arc<str>, crate::draw::TextStyle, SizeF)>,
    images: &'a HashMap<String, (u32, u32)>,
}

impl Lay<'_> {
    /// Size of what a leaf shows: text, icon or image.
    fn leaf_size(&mut self, idx: usize) -> Option<Size<f32>> {
        let n = &self.arena[idx];
        match n.kind {
            ElementKind::Text | ElementKind::Icon | ElementKind::Plate => {
                let text = n.text.clone().unwrap_or_else(|| Arc::from(""));
                let style = text_style(&n.style, self.scale);
                if let Some((t, s, size)) = self.cache.get(&n.key)
                    && *t == text
                    && *s == style
                {
                    return Some(Size { width: size.w, height: size.h });
                }
                let size = self.text.measure(&text, &style, None);
                self.cache.insert(n.key, (text, style, size));
                Some(Size { width: size.w, height: size.h })
            }
            ElementKind::Image => {
                let r = n.image.as_ref()?;
                let nat = self.images.get(r.path.as_ref()).map(|(w, h)| (*w as f32, *h as f32));
                let (w, h) = match (r.width, r.height, nat) {
                    (Some(w), Some(h), _) => (w, h),
                    (Some(w), None, Some((nw, nh))) if nw > 0.0 => (w, nh * w / nw),
                    (None, Some(h), Some((nw, nh))) if nh > 0.0 => (nw * h / nh, h),
                    (w, h, Some((nw, nh))) => (w.unwrap_or(nw), h.unwrap_or(nh)),
                    (w, h, None) => (w.unwrap_or(0.0), h.unwrap_or(0.0)),
                };
                Some(Size { width: w * self.scale, height: h * self.scale })
            }
            _ => None,
        }
    }

    fn build(&mut self, idx: usize, root: Option<&Style>) -> Option<NodeId> {
        let n = &self.arena[idx];
        if n.style.display_none {
            return None;
        }
        let kids: Vec<usize> = n.children.clone();
        let is_leaf = matches!(n.kind, ElementKind::Text | ElementKind::Icon | ElementKind::Plate | ElementKind::Image);
        let shrinkable =
            matches!(n.kind, ElementKind::Text | ElementKind::Icon | ElementKind::Plate) && n.style.ellipsis;
        let mut style = taffy_style(&n.style, self.scale, shrinkable);
        if let Some(r) = root {
            style = r.clone();
        }
        let id = if is_leaf {
            let size = self.leaf_size(idx).unwrap_or(Size { width: 0.0, height: 0.0 });
            self.tree.new_leaf_with_context(style, size).ok()?
        } else {
            let mut ch = Vec::new();
            for k in kids {
                if let Some(c) = self.build(k, None) {
                    ch.push(c);
                }
            }
            self.tree.new_with_children(style, &ch).ok()?
        };
        self.ids[idx] = Some(id);
        Some(id)
    }

    fn compute(&mut self, root: NodeId, avail: Size<AvailableSpace>) {
        let _ = self.tree.compute_layout_with_measure(root, avail, |inputs, _, ctx, style| {
            taffy::compute_leaf_layout(
                inputs,
                style,
                |_, _| 0.0,
                |known, _| {
                    let c = ctx.map_or(Size::ZERO, |s| *s);
                    Size { width: known.width.unwrap_or(c.width), height: known.height.unwrap_or(c.height) }
                },
            )
        });
    }

    fn harvest(&self, idx: usize, origin: (f32, f32), out: &mut [Laid], root_rect: Option<(f32, f32)>) {
        let Some(id) = self.ids[idx] else { return };
        let Ok(l) = self.tree.layout(id) else { return };
        let (x, y) = root_rect.unwrap_or((origin.0 + l.location.x, origin.1 + l.location.y));
        let rect = RectF::new(x, y, l.size.width, l.size.height);
        out[idx] = Laid {
            rect,
            border: Edges { top: l.border.top, right: l.border.right, bottom: l.border.bottom, left: l.border.left },
            padding: Edges {
                top: l.padding.top,
                right: l.padding.right,
                bottom: l.padding.bottom,
                left: l.padding.left,
            },
            text: None,
            clip: false,
        };
        for &c in &self.arena[idx].children {
            self.harvest(c, (x, y), out, None);
        }
    }
}

impl Scene {
    pub(super) fn layout(&mut self, text: &mut dyn TextMeasure) {
        let scale = self.geometry.scale;
        let mut arena = std::mem::take(&mut self.arena);
        if arena.is_empty() {
            return;
        }
        let mut out: Vec<Laid> = vec![Laid::default(); arena.len()];

        // The bar box.
        let rect = bar_rect(&self.bar, &self.geometry);
        let bar_style = arena[0].style.clone();
        let bb = border_px(&bar_style.border_width, scale);
        let bp = &bar_style.padding;
        // Whole pixels, so every zone starts on a pixel edge at any scale.
        let pad = Edges {
            top: whole(bp.top, scale),
            right: whole(bp.right, scale),
            bottom: whole(bp.bottom, scale),
            left: whole(bp.left, scale),
        };
        out[0] = Laid { rect, border: bb, padding: pad, text: None, clip: false };
        let cx0 = rect.x + bb.left + pad.left;
        let cx1 = rect.right() - bb.right - pad.right;
        let cy0 = rect.y + bb.top + pad.top;
        let content_w = (cx1 - cx0).max(0.0);
        let content_h = (rect.bottom() - bb.bottom - pad.bottom - cy0).max(0.0);

        let zones: Vec<usize> = arena[0].children.clone();
        let mut lay = Lay {
            arena: &arena,
            scale,
            tree: TaffyTree::new(),
            ids: vec![None; arena.len()],
            text: &mut *text,
            cache: &mut self.measure_cache,
            images: &self.image_sizes,
        };

        // Each zone at its natural width first.
        let mut roots: Vec<Option<(NodeId, Style)>> = Vec::new();
        for &z in &zones {
            let mut st = taffy_style(&arena[z].style, scale, false);
            st.margin = Rect {
                left: LengthPercentageAuto::length(0.0),
                right: LengthPercentageAuto::length(0.0),
                top: LengthPercentageAuto::length(0.0),
                bottom: LengthPercentageAuto::length(0.0),
            };
            if matches!(arena[z].style.height, Dim::Auto) {
                st.size.height = Dimension::length(content_h);
            }
            let id = lay.build(z, Some(&st));
            if let Some(id) = id {
                lay.compute(
                    id,
                    Size { width: AvailableSpace::MaxContent, height: AvailableSpace::Definite(content_h) },
                );
            }
            roots.push(id.map(|i| (i, st)));
        }
        let natural: Vec<f32> = roots
            .iter()
            .map(|r| r.as_ref().and_then(|(id, _)| lay.tree.layout(*id).ok()).map_or(0.0, |l| l.size.width))
            .collect();
        let at = |i: usize| natural.get(i).copied().unwrap_or(0.0);
        let (lw_nat, cw_nat, rw_nat) = (at(0), at(1), at(2));

        // Place them: left at the start, right at the end, centre on the bar
        // and only moved as far as needed to clear the others, with a small
        // gap so a crowded bar does not run its zones into each other.
        let lw = lw_nat.min(content_w);
        let rw = rw_nat.min((content_w - lw).max(0.0));
        let left_end = cx0 + lw;
        let right_start = cx1 - rw;
        let gap = (ZONE_GAP * scale).round();
        let centre_from = if lw > 0.0 { left_end + gap } else { left_end };
        let centre_to = if rw > 0.0 { right_start - gap } else { right_start };
        let mut cw = cw_nat;
        let mut cx = (rect.x + rect.w / 2.0 - cw / 2.0).round();
        if cx < centre_from {
            cx = centre_from;
        }
        if cx + cw > centre_to {
            cx = (centre_to - cw).max(centre_from);
        }
        if cx + cw > centre_to {
            cw = (centre_to - cx).max(0.0);
        }
        let widths = [lw, cw, rw];
        let xs = [cx0, cx.round(), right_start];
        for (i, z) in zones.iter().enumerate().take(3) {
            let Some((id, st)) = &roots[i] else { continue };
            let clipped = widths[i] + 0.5 < at(i);
            if clipped {
                let mut st2 = st.clone();
                st2.size.width = Dimension::length(widths[i]);
                let _ = lay.tree.set_style(*id, st2);
                lay.compute(
                    *id,
                    Size { width: AvailableSpace::Definite(widths[i]), height: AvailableSpace::Definite(content_h) },
                );
            }
            let h = lay.tree.layout(*id).map_or(content_h, |l| l.size.height);
            let y = cy0 + ((content_h - h) / 2.0).max(0.0).round();
            lay.harvest(*z, (0.0, 0.0), &mut out, Some((xs[i], y)));
            out[*z].clip = clipped;
        }
        drop(lay);

        // Text that does not fit: cut with an ellipsis, measured with the
        // same measurer that sized it.
        for idx in 0..arena.len() {
            let n = &arena[idx];
            if !matches!(n.kind, ElementKind::Text | ElementKind::Icon | ElementKind::Plate) {
                continue;
            }
            let Some(full) = n.text.clone() else { continue };
            let l = &out[idx];
            let avail = l.rect.w - l.border.left - l.border.right - l.padding.left - l.padding.right;
            let shown = if n.style.ellipsis && !full.is_empty() {
                let style = text_style(&n.style, scale);
                let natural_w = self.measure_cache.get(&n.key).map_or(0.0, |c| c.2.w);
                // Box edges are rounded to whole pixels, so a text 128.16 px wide gets a 128 px box:
                // under half a pixel of overhang is not a reason to cut it.
                if natural_w > avail + 0.5 { ellipsize(&full, avail, &style, &mut *text) } else { full }
            } else {
                full
            };
            out[idx].text = Some(shown);
        }

        for (n, l) in arena.iter_mut().zip(out) {
            n.laid = l;
        }
        self.arena = arena;
    }
}

/// Logical px kept between the centre zone and a neighbour it was moved away from.
const ZONE_GAP: f32 = 8.0;

/// U+2026, the horizontal ellipsis.
const ELLIPSIS: char = match char::from_u32(0x2026) {
    Some(c) => c,
    None => '.',
};

/// The longest start of `full` that fits `avail` px with an ellipsis added.
fn ellipsize(full: &str, avail: f32, style: &crate::draw::TextStyle, m: &mut dyn TextMeasure) -> Arc<str> {
    let ends: Vec<usize> = full.char_indices().map(|(i, _)| i).collect();
    let with_cut = |n: usize| {
        let cut = ends.get(n).copied().unwrap_or(full.len());
        format!("{}{ELLIPSIS}", &full[..cut])
    };
    let fits = |n: usize, m: &mut dyn TextMeasure| m.measure(&with_cut(n), style, None).w <= avail + 0.01;
    if !fits(0, m) {
        return Arc::from("");
    }
    // Largest n in 0..len whose first n chars plus the ellipsis fit.
    let (mut lo, mut hi) = (0usize, ends.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(mid, m) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    Arc::from(with_cut(lo))
}

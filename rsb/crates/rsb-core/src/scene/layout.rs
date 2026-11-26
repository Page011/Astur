//! Layout: Taffy flexbox per zone, then the zones placed in the bar.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use taffy::{
    AlignContent, AlignItems, AvailableSpace, BoxSizing, Dimension, Display, LengthPercentage, LengthPercentageAuto,
    NodeId, Overflow, Point, Rect, Size, Style, TaffyTree,
};

use super::motion::SizeRun;
use super::shape::{self, Affine};
use super::trans;
use super::{Laid, Node, Scene, bar_rect, text_style};
use crate::config::WidgetRef;
use crate::css::PropertyId;
use crate::element::{ElementKind, IconRef, NodeKey};
use crate::geom::{Edges, RectF, SizeF};
use crate::layout::TextMeasure;
use crate::style::{Align, ComputedStyle, Dim, FlexDirection, Justify, Overflow as CssOverflow};

/// Width and height forced for a node, physical px (a running size change).
type Forced = (Option<f32>, Option<f32>);

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

/// A size forced to `px` physical px, or the style's own.
fn forced_dimension(px: Option<f32>, d: Dim, scale: f32) -> Dimension {
    px.map_or_else(|| dimension(d, scale), Dimension::length)
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
    /// Pass two: per node, the width and height a running size change
    /// holds it at. None = the target layout (every node at its own size).
    forced: Option<&'a [Forced]>,
}

impl Lay<'_> {
    /// Size of what a leaf shows: text, icon or image.
    fn leaf_size(&mut self, idx: usize) -> Option<Size<f32>> {
        let n = &self.arena[idx];
        // A picture icon is a square the size of its font.
        if let (ElementKind::Icon, Some(IconRef::Svg(_))) = (n.kind, &n.icon) {
            let side = n.style.font_size * self.scale;
            return Some(Size { width: side, height: side });
        }
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
            ElementKind::Input => {
                // A box of about twelve characters unless the style sizes it.
                let style = text_style(&n.style, self.scale);
                let one = self.text.measure("0", &style, None);
                Some(Size { width: one.w * 12.0, height: one.h })
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
        let is_leaf = matches!(
            n.kind,
            ElementKind::Text | ElementKind::Icon | ElementKind::Plate | ElementKind::Image | ElementKind::Input
        );
        let shrinkable =
            matches!(n.kind, ElementKind::Text | ElementKind::Icon | ElementKind::Plate) && n.style.ellipsis;
        let mut style = taffy_style(&n.style, self.scale, shrinkable);
        // Sizes come from the target style; a running size change holds its own.
        let (fw, fh) = self.forced.map_or((None, None), |f| f[idx]);
        style.size = Size {
            width: forced_dimension(fw, n.target.width, self.scale),
            height: forced_dimension(fh, n.target.height, self.scale),
        };
        // Content of a scroll container keeps its natural size along the
        // scrolling axis instead of shrinking to fit.
        if let Some(p) = n.parent {
            let ps = &self.arena[p].style;
            let main = match ps.flex_direction {
                FlexDirection::Column => ps.overflow_y,
                FlexDirection::Row => ps.overflow_x,
            };
            if matches!(main, CssOverflow::Auto | CssOverflow::Scroll) {
                style.flex_shrink = 0.0;
            }
        }
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
            target: rect,
            xf: Affine::IDENTITY,
            border: Edges { top: l.border.top, right: l.border.right, bottom: l.border.bottom, left: l.border.left },
            padding: Edges {
                top: l.padding.top,
                right: l.padding.right,
                bottom: l.padding.bottom,
                left: l.padding.left,
            },
            text: None,
            clip: false,
            scroll: (0.0, 0.0),
        };
        for &c in &self.arena[idx].children {
            self.harvest(c, (x, y), out, None);
        }
    }
}

impl Scene {
    /// Lays the bar out: once with every node at the size its style gives
    /// (the target layout, where popups are placed), and a second time only
    /// while a width or height change is running, with those nodes held at
    /// their current size (the layout that is drawn).
    pub(super) fn layout(&mut self, text: &mut dyn TextMeasure, now: Instant) {
        // A popup is placed at the offset its settled size leaves for shadows.
        if self.popup.is_some() {
            self.natural(text);
        }
        let arena = std::mem::take(&mut self.arena);
        if arena.is_empty() {
            self.arena = arena;
            return;
        }
        let target = self.pass(&arena, text, None);
        let snap = std::mem::take(&mut self.snap_sizes);
        if snap {
            self.size_runs.clear();
        }
        self.update_size_runs(&arena, &target, now, snap);
        let forced: Vec<Forced> = arena
            .iter()
            .map(|n| {
                let runs = self.size_runs.get(&n.key);
                let w = runs.and_then(|r| r[0]).and_then(|r| r.at(now));
                let h = runs.and_then(|r| r[1]).and_then(|r| r.at(now));
                // A keyframe animation on width or height holds it too.
                let anim_w = (n.style.width != n.target.width).then_some(n.style.width);
                let anim_h = (n.style.height != n.target.height).then_some(n.style.height);
                let px = |d: Option<Dim>| match d {
                    Some(Dim::Px(v)) => Some(v * self.geometry.scale),
                    _ => None,
                };
                (w.or(px(anim_w)), h.or(px(anim_h)))
            })
            .collect();
        // Runs that arrived are done.
        for runs in self.size_runs.values_mut() {
            for r in runs.iter_mut() {
                if r.is_some_and(|run| run.at(now).is_none()) {
                    *r = None;
                }
            }
        }
        self.size_runs.retain(|_, r| r.iter().any(Option::is_some));
        let out = if forced.iter().any(|f| f.0.is_some() || f.1.is_some()) {
            let mut drawn = self.pass(&arena, text, Some(&forced));
            for (d, t) in drawn.iter_mut().zip(&target) {
                d.target = t.rect;
            }
            drawn
        } else {
            target
        };
        for (n, l) in arena.iter().zip(&out) {
            let (w, h) = if n.style.display_none { (0.0, 0.0) } else { (l.rect.w, l.rect.h) };
            self.drawn_size.insert(n.key, (w, h));
        }
        let mut arena = arena;
        for (n, l) in arena.iter_mut().zip(out) {
            n.laid = l;
        }
        self.arena = arena;
        self.update_transforms();
    }

    /// The transform each node is drawn with (its own and its ancestors'),
    /// parents first. Runs after every restyle as well as every layout: a
    /// transform moves without any box changing size.
    pub(super) fn update_transforms(&mut self) {
        let scale = self.geometry.scale;
        for i in 0..self.arena.len() {
            let own = shape::own_affine(&self.arena[i].style, &self.arena[i].laid.rect, scale);
            let (outer, scroll) = match self.arena[i].parent {
                Some(p) => {
                    // What the parent has scrolled moves its content.
                    let (dx, dy) = self.scrolls.get(&self.arena[p].key).map_or((0.0, 0.0), |s| s.displayed());
                    let shift = Affine { sx: 1.0, sy: 1.0, tx: -dx, ty: -dy };
                    let (sx, sy) = self.arena[p].laid.scroll;
                    (self.arena[p].laid.xf.after(&shift), (sx + dx, sy + dy))
                }
                None => (Affine::IDENTITY, (0.0, 0.0)),
            };
            self.arena[i].laid.xf = outer.after(&own);
            self.arena[i].laid.scroll = scroll;
        }
    }

    /// Starts, retargets or drops the width and height changes of nodes whose
    /// style has a transition on them: when the size the target layout gives
    /// differs from the size last drawn, the node goes from one to the other.
    fn update_size_runs(&mut self, arena: &[Node], target: &[Laid], now: Instant, snap: bool) {
        let start = self.change_time.min(now);
        for (n, t) in arena.iter().zip(target) {
            let used = if n.style.display_none { (0.0, 0.0) } else { (t.rect.w, t.rect.h) };
            let Some(&drawn) = self.drawn_size.get(&n.key) else { continue };
            if snap {
                continue;
            }
            for (axis, id) in [(0usize, PropertyId::Width), (1, PropertyId::Height)] {
                let (to, from_drawn) = if axis == 0 { (used.0, drawn.0) } else { (used.1, drawn.1) };
                let Some(spec) = trans::transition_for(&n.target, id) else {
                    if let Some(r) = self.size_runs.get_mut(&n.key) {
                        r[axis] = None;
                    }
                    continue;
                };
                let running = self.size_runs.get(&n.key).and_then(|r| r[axis]);
                if running.is_some_and(|r| (r.to - to).abs() < 0.01) {
                    continue;
                }
                if (to - from_drawn).abs() < 0.01 {
                    if let Some(r) = self.size_runs.get_mut(&n.key) {
                        r[axis] = None;
                    }
                    continue;
                }
                let run = SizeRun {
                    from: from_drawn,
                    to,
                    start: if running.is_some() { now } else { start },
                    delay: spec.delay,
                    duration: spec.duration,
                    easing: spec.easing,
                };
                self.size_runs.entry(n.key).or_default()[axis] = Some(run);
            }
        }
    }

    fn pass(&mut self, arena: &[Node], text: &mut dyn TextMeasure, forced: Option<&[Forced]>) -> Vec<Laid> {
        let scale = self.geometry.scale;
        let mut out = if self.popup.is_some() {
            self.popup_boxes(arena, text, forced)
        } else {
            self.bar_boxes(arena, text, forced)
        };

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
        out
    }

    /// The popup root at its natural size (or across the width it was given,
    /// or the monitor for an overlay), placed at the root offset.
    fn popup_boxes(&mut self, arena: &[Node], text: &mut dyn TextMeasure, forced: Option<&[Forced]>) -> Vec<Laid> {
        let scale = self.geometry.scale;
        let mut out: Vec<Laid> = vec![Laid::default(); arena.len()];
        let root = self.root_idx;
        let Some(p) = &self.popup else { return out };
        if root >= arena.len() {
            return out;
        }
        let overlay = p.spec.side == crate::popup::Side::Screen;
        let (monitor, width, offset) = (p.monitor, p.width, p.offset);
        let mut lay = Lay {
            arena,
            scale,
            tree: TaffyTree::new(),
            ids: vec![None; arena.len()],
            text: &mut *text,
            cache: &mut self.measure_cache,
            images: &self.image_sizes,
            forced,
        };
        let mut st = taffy_style(&arena[root].style, scale, false);
        let (fw, fh) = forced.map_or((None, None), |f| f[root]);
        st.size.width = forced_dimension(fw, arena[root].target.width, scale);
        st.size.height = forced_dimension(fh, arena[root].target.height, scale);
        st.margin = Rect {
            left: LengthPercentageAuto::length(0.0),
            right: LengthPercentageAuto::length(0.0),
            top: LengthPercentageAuto::length(0.0),
            bottom: LengthPercentageAuto::length(0.0),
        };
        let mut avail = Size { width: AvailableSpace::MaxContent, height: AvailableSpace::MaxContent };
        if overlay {
            st.size = Size { width: Dimension::length(monitor.w), height: Dimension::length(monitor.h) };
            avail = Size { width: AvailableSpace::Definite(monitor.w), height: AvailableSpace::Definite(monitor.h) };
        } else if let Some(w) = width {
            st.size.width = Dimension::length(w);
            avail.width = AvailableSpace::Definite(w);
        }
        if let Some(id) = lay.build(root, Some(&st)) {
            lay.compute(id, avail);
            lay.harvest(root, (0.0, 0.0), &mut out, Some((offset.x, offset.y)));
        }
        out
    }

    fn bar_boxes(&mut self, arena: &[Node], text: &mut dyn TextMeasure, forced: Option<&[Forced]>) -> Vec<Laid> {
        let scale = self.geometry.scale;
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
        out[0] = Laid { rect, target: rect, border: bb, padding: pad, ..Laid::default() };
        let cx0 = rect.x + bb.left + pad.left;
        let cx1 = rect.right() - bb.right - pad.right;
        let cy0 = rect.y + bb.top + pad.top;
        let content_w = (cx1 - cx0).max(0.0);
        let content_h = (rect.bottom() - bb.bottom - pad.bottom - cy0).max(0.0);

        let zones: Vec<usize> = arena[0].children.clone();
        let pinned = self.pinned_widget(arena, &zones);
        let mut lay = Lay {
            arena,
            scale,
            tree: TaffyTree::new(),
            ids: vec![None; arena.len()],
            text: &mut *text,
            cache: &mut self.measure_cache,
            images: &self.image_sizes,
            forced,
        };

        // Each zone at its natural width first.
        let mut roots: Vec<Option<(NodeId, Style)>> = Vec::new();
        for &z in &zones {
            let mut st = taffy_style(&arena[z].style, scale, false);
            let (fw, fh) = forced.map_or((None, None), |f| f[z]);
            st.size.width = forced_dimension(fw, arena[z].target.width, scale);
            st.size.height = forced_dimension(fh, arena[z].target.height, scale);
            st.margin = Rect {
                left: LengthPercentageAuto::length(0.0),
                right: LengthPercentageAuto::length(0.0),
                top: LengthPercentageAuto::length(0.0),
                bottom: LengthPercentageAuto::length(0.0),
            };
            if matches!(arena[z].target.height, Dim::Auto) && fh.is_none() {
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
        // Where the pinned widget's centre is inside the centre zone.
        let pinned_centre = pinned.and_then(|w| {
            let l = lay.tree.layout(lay.ids[w]?).ok()?;
            Some(l.location.x + l.size.width / 2.0)
        });

        // Place them: left at the start, right at the end, centre on the bar
        // (or its pinned widget on the bar's centre) and only moved as far
        // as needed to clear the others, with a small gap so a crowded bar
        // does not run its zones into each other.
        let lw = lw_nat.min(content_w);
        let rw = rw_nat.min((content_w - lw).max(0.0));
        let left_end = cx0 + lw;
        let right_start = cx1 - rw;
        let gap = (ZONE_GAP * scale).round();
        let centre_from = if lw > 0.0 { left_end + gap } else { left_end };
        let centre_to = if rw > 0.0 { right_start - gap } else { right_start };
        let mut cw = cw_nat;
        let bar_mid = rect.x + rect.w / 2.0;
        // The pinned case floors the origin so the widget's box, and the
        // clip-path vertices cut from it, start on a whole device pixel.
        let mut cx = match pinned_centre {
            Some(off) => (bar_mid - off).floor(),
            None => (bar_mid - cw / 2.0).round(),
        };
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
        let xs = [cx0, if pinned_centre.is_some() { cx } else { cx.round() }, right_start];
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
        out
    }

    /// The widget wrapper of the centre zone the bar is centred on.
    fn pinned_widget(&self, arena: &[Node], zones: &[usize]) -> Option<usize> {
        let want = WidgetRef::parse(self.bar.center_on.as_deref()?).ok()?;
        let (class, id) = (want.css_class(), want.css_id());
        let centre = *zones.get(1)?;
        arena[centre].children.iter().copied().find(|&c| {
            let n = &arena[c];
            n.kind == ElementKind::Widget
                && !n.style.display_none
                && n.id.as_deref() == Some(id.as_str())
                && n.classes.iter().any(|k| k.as_ref() == class)
        })
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

//! Scroll containers: offsets kept per node, smooth wheel steps, keys,
//! reveal, and the scrollbar the scene draws over the padding box.

use std::time::{Duration, Instant};

use super::{Drag, Scene};
use crate::anim::Easing;
use crate::color::Color;
use crate::draw::{DrawCmd, DrawItem, Fill};
use crate::element::NodeKey;
use crate::geom::{PointF, Radii, RectF};
use crate::hit::{ElementCall, ElementEvent};
use crate::key::Key;
use crate::style::{Overflow, ScrollbarWidth};

/// Logical px one wheel notch moves.
pub(super) const WHEEL_NOTCH: f32 = 48.0;
const SMOOTH: Duration = Duration::from_millis(120);
const BAR_HOLD: Duration = Duration::from_millis(800);
const BAR_FADE: Duration = Duration::from_millis(200);
/// `on_scroll` runs at most 30 times a second while moving.
const REPORT_GAP: Duration = Duration::from_millis(33);
const REVEAL_MARGIN: f32 = 8.0;
const MIN_THUMB: f32 = 24.0;
const BAR_INSET: f32 = 2.0;

#[derive(Clone, Copy)]
struct Anim {
    from: (f32, f32),
    start: Instant,
}

/// The offset of one scroll container, physical px.
pub(super) struct ScrollState {
    pub cur: (f32, f32),
    pub target: (f32, f32),
    anim: Option<Anim>,
    /// How far each axis can move and the padding box size, from the last layout.
    pub max: (f32, f32),
    pub view: (f32, f32),
    /// The last wheel, key or drag: the scrollbar stays up for a while after.
    pub active: Option<Instant>,
    reported: (f32, f32),
    last_report: Option<Instant>,
}

impl Default for ScrollState {
    fn default() -> Self {
        ScrollState {
            cur: (0.0, 0.0),
            target: (0.0, 0.0),
            anim: None,
            max: (0.0, 0.0),
            view: (0.0, 0.0),
            active: None,
            reported: (0.0, 0.0),
            last_report: None,
        }
    }
}

impl ScrollState {
    /// The offset drawn: whole pixels, so text stays crisp.
    pub fn displayed(&self) -> (f32, f32) {
        (self.cur.0.round(), self.cur.1.round())
    }

    fn clamp(&self, t: (f32, f32)) -> (f32, f32) {
        (t.0.clamp(0.0, self.max.0.max(0.0)), t.1.clamp(0.0, self.max.1.max(0.0)))
    }

    /// Moves smoothly to `t` (clamped), from where it is drawn now.
    pub fn scroll_to(&mut self, t: (f32, f32), now: Instant) {
        let t = self.clamp(t);
        self.active = Some(now);
        if t == self.target {
            return;
        }
        self.target = t;
        self.anim = Some(Anim { from: self.cur, start: now });
    }

    /// Jumps to `t` with no smoothing (a thumb drag, a change of range).
    fn set_now(&mut self, t: (f32, f32), now: Instant) {
        let t = self.clamp(t);
        self.cur = t;
        self.target = t;
        self.anim = None;
        self.active = Some(now);
    }

    /// Advances a running step; true when the offset moved.
    fn sample(&mut self, now: Instant) -> bool {
        let Some(a) = self.anim else { return false };
        let before = self.cur;
        let t = now.saturating_duration_since(a.start).as_secs_f32() / SMOOTH.as_secs_f32();
        if t >= 1.0 {
            self.cur = self.target;
            self.anim = None;
        } else {
            let e = Easing::EaseOut.apply(t);
            self.cur = (a.from.0 + (self.target.0 - a.from.0) * e, a.from.1 + (self.target.1 - a.from.1) * e);
        }
        self.cur != before
    }

    pub fn moving(&self) -> bool {
        self.anim.is_some()
    }

    /// Scrollbar opacity: solid while hovered or dragged, held 800 ms after
    /// the last movement, then faded out over 200 ms.
    fn bar_alpha(&self, solid: bool, now: Instant) -> f32 {
        if solid {
            return 1.0;
        }
        let Some(t) = self.active else { return 0.0 };
        let dt = now.saturating_duration_since(t);
        if dt < BAR_HOLD {
            1.0
        } else if dt < BAR_HOLD + BAR_FADE {
            1.0 - (dt - BAR_HOLD).as_secs_f32() / BAR_FADE.as_secs_f32()
        } else {
            0.0
        }
    }
}

/// Track and thumb of one scrollbar, in the container's layout space.
pub(super) struct BarGeom {
    pub track: RectF,
    pub thumb: RectF,
    pub width: f32,
}

impl Scene {
    pub(super) fn scroll_axes(&self, idx: usize) -> (bool, bool) {
        let s = &self.arena[idx].style;
        let on = |o: Overflow| matches!(o, Overflow::Auto | Overflow::Scroll);
        (on(s.overflow_x), on(s.overflow_y))
    }

    /// How far each container can move, after a layout.
    pub(super) fn update_scroll_extents(&mut self) {
        for idx in self.root_idx..self.arena.len() {
            let (ax, ay) = self.scroll_axes(idx);
            if !(ax || ay) || self.arena[idx].style.display_none {
                continue;
            }
            let n = &self.arena[idx];
            let (r, b, p) = (n.laid.rect, n.laid.border, n.laid.padding);
            let view = (r.w - b.left - b.right, r.h - b.top - b.bottom);
            let (ox, oy) = (r.x + b.left, r.y + b.top);
            let (mut cw, mut ch) = (0.0f32, 0.0f32);
            for &c in &n.children {
                let k = &self.arena[c];
                if k.style.display_none {
                    continue;
                }
                cw = cw.max(k.laid.rect.right() - ox);
                ch = ch.max(k.laid.rect.bottom() - oy);
            }
            cw += p.right;
            ch += p.bottom;
            let max = (if ax { (cw - view.0).max(0.0) } else { 0.0 }, if ay { (ch - view.1).max(0.0) } else { 0.0 });
            let st = self.scrolls.entry(n.key).or_default();
            st.max = max;
            st.view = view;
            st.target = st.clamp(st.target);
            let clamped = st.clamp(st.cur);
            if clamped != st.cur {
                st.cur = clamped;
            }
        }
    }

    /// Moves smooth steps along and reports `on_scroll`; true when an offset
    /// moved, so transforms and the draw list need redoing.
    pub(super) fn scroll_tick(&mut self, now: Instant) -> bool {
        let mut changed = false;
        let mut reports: Vec<(NodeKey, (f32, f32))> = Vec::new();
        for (k, st) in &mut self.scrolls {
            changed |= st.sample(now);
            let shown = st.displayed();
            if shown != st.reported && (st.anim.is_none() || st.last_report.is_none_or(|t| now - t >= REPORT_GAP)) {
                st.reported = shown;
                st.last_report = Some(now);
                reports.push((*k, shown));
            }
        }
        let scale = self.geometry.scale.max(0.01);
        for (k, (x, y)) in reports {
            if let Some(i) = self.idx_of(k)
                && let (Some(handler), Some(instance)) = (self.arena[i].handlers.scroll, self.arena[i].instance)
            {
                let event = ElementEvent::Scrolled { x: x / scale, y: y / scale };
                self.events.push(ElementCall { instance, handler, event, gesture: 0 });
            }
        }
        changed
    }

    /// The wheel over `chain` (root first): the nearest container that can
    /// move in that direction scrolls. A vertical wheel moves a container
    /// that only scrolls sideways.
    pub(super) fn wheel_scroll(&mut self, chain: &[usize], notches: f32, now: Instant) -> bool {
        let delta = -notches * WHEEL_NOTCH * self.geometry.scale;
        for &i in chain.iter().rev() {
            let (ax, ay) = self.scroll_axes(i);
            if !(ax || ay) {
                continue;
            }
            let Some(st) = self.scrolls.get_mut(&self.arena[i].key) else { continue };
            let t = st.target;
            let next = if st.max.1 > 0.0 {
                (t.0, (t.1 + delta).clamp(0.0, st.max.1))
            } else if st.max.0 > 0.0 {
                ((t.0 + delta).clamp(0.0, st.max.0), t.1)
            } else {
                continue;
            };
            if next != t {
                st.scroll_to(next, now);
                self.repaint = true;
                return true;
            }
        }
        false
    }

    /// The sideways wheel over `chain` (root first): the nearest container
    /// that can move along x scrolls.
    pub(super) fn wheel_scroll_sideways(&mut self, chain: &[usize], notches: f32, now: Instant) -> bool {
        let delta = notches * WHEEL_NOTCH * self.geometry.scale;
        for &i in chain.iter().rev() {
            if !self.scroll_axes(i).0 {
                continue;
            }
            let Some(st) = self.scrolls.get_mut(&self.arena[i].key) else { continue };
            let t = st.target;
            let next = ((t.0 + delta).clamp(0.0, st.max.0), t.1);
            if st.max.0 > 0.0 && next != t {
                st.scroll_to(next, now);
                self.repaint = true;
                return true;
            }
        }
        false
    }

    /// Scrolls the nearest scroll container at or above `idx` by `by` physical px.
    fn scroll_nearest(&mut self, idx: usize, by: (f32, f32), now: Instant) -> bool {
        let mut i = Some(idx);
        while let Some(n) = i {
            let (ax, ay) = self.scroll_axes(n);
            if (ax || ay)
                && let Some(st) = self.scrolls.get_mut(&self.arena[n].key)
                && ((by.1 != 0.0 && st.max.1 > 0.0) || (by.0 != 0.0 && st.max.0 > 0.0))
            {
                let t = st.target;
                st.scroll_to((t.0 + by.0, t.1 + by.1), now);
                self.repaint = true;
                return true;
            }
            i = self.arena[n].parent;
        }
        false
    }

    /// PageUp, PageDown, Home and End from a focused descendant, and the
    /// arrows when the container itself is focused.
    pub(super) fn scroll_key(&mut self, focused: usize, key: Key, now: Instant) -> bool {
        let scale = self.geometry.scale;
        let view_h = |s: &Scene, i: usize| s.scrolls.get(&s.arena[i].key).map_or(0.0, |st| st.view.1);
        match key {
            Key::PageUp | Key::PageDown => {
                let Some(c) = self.nearest_container(focused) else { return false };
                let page = view_h(self, c) * 0.9;
                let sign = if key == Key::PageDown { 1.0 } else { -1.0 };
                self.scroll_nearest(c, (0.0, sign * page), now)
            }
            Key::Home | Key::End => {
                let Some(c) = self.nearest_container(focused) else { return false };
                let big = if key == Key::End { 1.0e7 } else { -1.0e7 };
                self.scroll_nearest(c, (0.0, big), now)
            }
            Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight => {
                let (ax, ay) = self.scroll_axes(focused);
                if !(ax || ay) {
                    return false;
                }
                let step = WHEEL_NOTCH * scale;
                let by = match key {
                    Key::ArrowUp => (0.0, -step),
                    Key::ArrowDown => (0.0, step),
                    Key::ArrowLeft => (-step, 0.0),
                    _ => (step, 0.0),
                };
                self.scroll_nearest(focused, by, now)
            }
            _ => false,
        }
    }

    fn nearest_container(&self, idx: usize) -> Option<usize> {
        let mut i = Some(idx);
        while let Some(n) = i {
            let (ax, ay) = self.scroll_axes(n);
            if ax || ay {
                return Some(n);
            }
            i = self.arena[n].parent;
        }
        None
    }

    /// Scrolls every container around `idx` so that it is inside, with a margin
    /// of 8 px at the nearest edge, or the container's own padding on that
    /// side when that is more: the first row of a padded list comes back to
    /// where it rests, not closer to the edge.
    pub(super) fn reveal(&mut self, idx: usize, now: Instant) {
        let margin = REVEAL_MARGIN * self.geometry.scale;
        let mut want = self.arena[idx].laid.rect;
        let mut i = self.arena[idx].parent;
        while let Some(c) = i {
            let (ax, ay) = self.scroll_axes(c);
            if ax || ay {
                let (r, b, p) = (self.arena[c].laid.rect, self.arena[c].laid.border, self.arena[c].laid.padding);
                let key = self.arena[c].key;
                if let Some(st) = self.scrolls.get_mut(&key) {
                    let rel = (want.x - (r.x + b.left), want.y - (r.y + b.top));
                    let mut t = st.target;
                    let fit = |pos: f32, len: f32, off: f32, view: f32, before: f32, after: f32| {
                        let (before, after) = (margin.max(before), margin.max(after));
                        // Above the view, or room for less than the element (a panel still growing open):
                        // show its start.
                        if len + before + after > view || pos < off + before {
                            (pos - before).max(0.0)
                        } else if pos + len > off + view - after {
                            pos + len - view + after
                        } else {
                            off
                        }
                    };
                    if ax {
                        t.0 = fit(rel.0, want.w, t.0, st.view.0, p.left, p.right);
                    }
                    if ay {
                        t.1 = fit(rel.1, want.h, t.1, st.view.1, p.top, p.bottom);
                    }
                    st.scroll_to(t, now);
                }
                want = r;
            }
            i = self.arena[c].parent;
        }
    }

    pub(super) fn solid_bar(&self, key: NodeKey) -> bool {
        self.hover.contains(&key) || matches!(self.drag, Some(Drag::Thumb { key: k, .. }) if k == key)
    }

    /// Track and thumb of the scrollbar on `axis` (0 across, 1 down) of a container.
    pub(super) fn scrollbar(&self, idx: usize, axis: usize) -> Option<BarGeom> {
        let n = &self.arena[idx];
        let s = &n.style;
        if n.style.display_none || s.scrollbar_width == ScrollbarWidth::None {
            return None;
        }
        let st = self.scrolls.get(&n.key)?;
        let (ax, ay) = self.scroll_axes(idx);
        let (on, max, view) = if axis == 1 { (ay, st.max.1, st.view.1) } else { (ax, st.max.0, st.view.0) };
        if !on || max <= 0.0 || view <= 0.0 {
            return None;
        }
        let scale = self.geometry.scale;
        let hovered = self.hover.contains(&n.key);
        let logical = match s.scrollbar_width {
            ScrollbarWidth::Auto => 8.0,
            ScrollbarWidth::Thin if hovered => 6.0,
            _ => 4.0,
        };
        let w = (logical * scale).round().max(1.0);
        let inset = BAR_INSET * scale;
        let (r, b) = (n.laid.rect, n.laid.border);
        let pad = RectF::new(r.x + b.left, r.y + b.top, r.w - b.left - b.right, r.h - b.top - b.bottom);
        // Leave the corner to the other bar when both show.
        let other = if axis == 1 { st.max.0 > 0.0 && ax } else { st.max.1 > 0.0 && ay };
        let corner = if other { w + inset } else { 0.0 };
        let (track, along) = if axis == 1 {
            (RectF::new(pad.right() - inset - w, pad.y + inset, w, (pad.h - 2.0 * inset - corner).max(0.0)), pad.h)
        } else {
            (RectF::new(pad.x + inset, pad.bottom() - inset - w, (pad.w - 2.0 * inset - corner).max(0.0), w), pad.w)
        };
        let len = if axis == 1 { track.h } else { track.w };
        let thumb_len = (len * along / (along + max)).max(MIN_THUMB * scale).min(len);
        let off = if axis == 1 { st.displayed().1 } else { st.displayed().0 };
        let pos = (off / max).clamp(0.0, 1.0) * (len - thumb_len);
        let thumb = if axis == 1 {
            RectF::new(track.x, track.y + pos, w, thumb_len)
        } else {
            RectF::new(track.x + pos, track.y, thumb_len, w)
        };
        Some(BarGeom { track, thumb, width: w })
    }

    /// A press on a visible scrollbar thumb of a container in `chain` starts a drag.
    pub(super) fn thumb_press(&mut self, chain: &[usize], p: PointF, now: Instant) -> bool {
        for &i in chain.iter().rev() {
            let key = self.arena[i].key;
            let solid = self.solid_bar(key);
            let Some(st) = self.scrolls.get(&key) else { continue };
            if st.bar_alpha(solid, now) <= 0.0 {
                continue;
            }
            let Some(q) = self.arena[i].laid.xf.inverse_point(p) else { continue };
            for axis in [1usize, 0] {
                if let Some(g) = self.scrollbar(i, axis)
                    && g.thumb.contains(q)
                {
                    let grab = if axis == 1 { q.y - g.thumb.y } else { q.x - g.thumb.x };
                    self.drag = Some(Drag::Thumb { key, axis, grab });
                    if let Some(st) = self.scrolls.get_mut(&key) {
                        st.active = Some(now);
                    }
                    return true;
                }
            }
        }
        false
    }

    /// The pointer moved with a thumb held.
    pub(super) fn thumb_drag(&mut self, key: NodeKey, axis: usize, grab: f32, p: PointF, now: Instant) {
        let Some(i) = self.idx_of(key) else { return };
        let Some(g) = self.scrollbar(i, axis) else { return };
        let Some(q) = self.arena[i].laid.xf.inverse_point(p) else { return };
        let (start, len, thumb_len) =
            if axis == 1 { (g.track.y, g.track.h, g.thumb.h) } else { (g.track.x, g.track.w, g.thumb.w) };
        let along = if axis == 1 { q.y } else { q.x };
        let room = (len - thumb_len).max(1.0);
        let ratio = ((along - grab - start) / room).clamp(0.0, 1.0);
        if let Some(st) = self.scrolls.get_mut(&key) {
            let t = if axis == 1 { (st.cur.0, ratio * st.max.1) } else { (ratio * st.max.0, st.cur.1) };
            st.set_now(t, now);
        }
        self.repaint = true;
    }

    /// Scrollbar items for a container, after its children.
    pub(super) fn paint_scrollbars(&self, idx: usize, out: &mut Vec<DrawItem>) {
        let n = &self.arena[idx];
        let Some(st) = self.scrolls.get(&n.key) else { return };
        let alpha = st.bar_alpha(self.solid_bar(n.key), self.now);
        if alpha <= 0.0 {
            return;
        }
        let (thumb_c, track_c) = n
            .style
            .scrollbar_color
            .unwrap_or((Color { a: n.style.color.a * 0.35, ..n.style.color }, Color::TRANSPARENT));
        let fade = |c: Color| Color { a: c.a * alpha, ..c };
        for axis in [1usize, 0] {
            let Some(g) = self.scrollbar(idx, axis) else { continue };
            let radii = Radii { tl: g.width / 2.0, tr: g.width / 2.0, br: g.width / 2.0, bl: g.width / 2.0 };
            let xf = n.laid.xf;
            for (rect, colour) in [(g.track, track_c), (g.thumb, thumb_c)] {
                if colour.a <= 0.0 {
                    continue;
                }
                out.push(xf.item(DrawItem {
                    node: n.key,
                    bounds: rect.outset(1.0),
                    cmd: DrawCmd::Fill { rect, radii, fill: Fill::Solid(fade(colour)) },
                }));
            }
        }
    }

    /// True while a smooth step or a fade needs frames; the next time
    /// something is due when nothing moves.
    pub(super) fn scroll_wake(&self, now: Instant) -> (bool, Option<Instant>) {
        let mut busy = false;
        let mut wake: Option<Instant> = None;
        for (k, st) in &self.scrolls {
            busy |= st.moving();
            if st.max.0 <= 0.0 && st.max.1 <= 0.0 {
                continue;
            }
            if self.solid_bar(*k) {
                continue;
            }
            if let Some(t) = st.active {
                let dt = now.saturating_duration_since(t);
                if dt < BAR_HOLD {
                    let at = t + BAR_HOLD;
                    wake = Some(wake.map_or(at, |w| w.min(at)));
                } else if dt < BAR_HOLD + BAR_FADE {
                    busy = true;
                }
            }
        }
        (busy, wake)
    }
}

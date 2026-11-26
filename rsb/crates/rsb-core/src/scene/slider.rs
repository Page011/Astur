//! Sliders: the element's box is the track, the scene paints the fill.

use std::time::{Duration, Instant};

use super::paint::resolve_radii;
use super::{Drag, Scene};
use crate::draw::{DrawCmd, DrawItem, Fill};
use crate::element::NodeKey;
use crate::geom::{PointF, RectF};
use crate::hit::{ElementCall, ElementEvent};
use crate::key::Key;

/// `on_change` while dragging runs at most 30 times a second.
const REPORT_GAP: Duration = Duration::from_millis(33);

/// A slider with `jump = false` treats a press and release that stay within
/// this many layout pixels as a click, not a drag.
const CLICK_SLOP: f32 = 4.0;

pub(super) struct SliderState {
    pub value: f64,
    pub seen_seq: u64,
    last_report: Option<Instant>,
    pending: bool,
    /// Where a press that has not moved far enough to be a drag started
    /// (`jump = false` sliders only).
    press: Option<PointF>,
}

impl SliderState {
    pub fn new(value: f64, seq: u64) -> SliderState {
        SliderState { value, seen_seq: seq, last_report: None, pending: false, press: None }
    }
}

impl Scene {
    fn slider_range(&self, idx: usize) -> (f64, f64, f64, bool) {
        let p = self.arena[idx].slider.as_deref();
        p.map_or((0.0, 100.0, 1.0, false), |p| (p.min, p.max.max(p.min), p.step, p.vertical))
    }

    /// `v` snapped to the step and held inside the range.
    fn snap(&self, idx: usize, v: f64) -> f64 {
        let (min, max, step, _) = self.slider_range(idx);
        let v = if step > 0.0 { min + ((v - min) / step).round() * step } else { v };
        ((v.clamp(min, max)) * 1.0e9).round() / 1.0e9
    }

    pub(super) fn slider_value(&self, idx: usize) -> f64 {
        let n = &self.arena[idx];
        self.sliders.get(&n.key).map_or_else(|| n.slider.as_deref().map_or(0.0, |p| p.value), |s| s.value)
    }

    fn push_value(&mut self, idx: usize, done: bool) {
        let n = &self.arena[idx];
        let (Some(handler), Some(instance)) = (n.slider.as_deref().and_then(|p| p.on_change), n.instance) else {
            return;
        };
        let event = ElementEvent::ValueChanged { value: self.slider_value(idx), done };
        self.events.push(ElementCall { instance, handler, event, gesture: 0 });
    }

    /// A new value, from a drag (`done` false: rate limited) or a key.
    fn slider_set(&mut self, idx: usize, v: f64, done: bool, now: Instant) {
        let v = self.snap(idx, v);
        let key = self.arena[idx].key;
        let Some(st) = self.sliders.get_mut(&key) else { return };
        let changed = st.value != v;
        st.value = v;
        self.repaint = true;
        if done {
            st.pending = false;
            st.last_report = Some(now);
            self.push_value(idx, true);
        } else if changed {
            if st.last_report.is_none_or(|t| now.saturating_duration_since(t) >= REPORT_GAP) {
                st.last_report = Some(now);
                st.pending = false;
                self.push_value(idx, false);
            } else {
                st.pending = true;
            }
        }
    }

    fn value_at(&self, idx: usize, p: PointF) -> f64 {
        let (min, max, _, vertical) = self.slider_range(idx);
        let r = self.arena[idx].laid.rect;
        let q = self.arena[idx].laid.xf.inverse_point(p).unwrap_or(p);
        let frac = if vertical { (r.bottom() - q.y) / r.h.max(1.0) } else { (q.x - r.x) / r.w.max(1.0) };
        min + f64::from(frac.clamp(0.0, 1.0)) * (max - min)
    }

    pub(super) fn slider_press(&mut self, idx: usize, p: PointF, now: Instant) {
        let key = self.arena[idx].key;
        self.drag = Some(Drag::Slider(key));
        let jump = self.arena[idx].slider.as_deref().is_none_or(|s| s.jump);
        if jump {
            let v = self.value_at(idx, p);
            self.slider_set(idx, v, false, now);
        } else if let Some(st) = self.sliders.get_mut(&key) {
            st.press = Some(p);
        }
    }

    pub(super) fn slider_drag(&mut self, key: NodeKey, p: PointF, now: Instant) {
        let slop = CLICK_SLOP * self.geometry.scale;
        if let Some(st) = self.sliders.get_mut(&key)
            && let Some(start) = st.press
        {
            if (p.x - start.x).hypot(p.y - start.y) < slop {
                return;
            }
            st.press = None;
        }
        if let Some(idx) = self.idx_of(key) {
            let v = self.value_at(idx, p);
            self.slider_set(idx, v, false, now);
        }
    }

    /// The button went up: the last value is reported with `done`. A press
    /// that never became a drag reports nothing and returns the slider, so
    /// the caller can deliver it as a click.
    pub(super) fn slider_release(&mut self, key: NodeKey, now: Instant) -> Option<usize> {
        let idx = self.idx_of(key)?;
        if self.sliders.get_mut(&key).and_then(|st| st.press.take()).is_some() {
            return Some(idx);
        }
        let v = self.slider_value(idx);
        self.slider_set(idx, v, true, now);
        None
    }

    pub(super) fn slider_key(&mut self, idx: usize, key: Key, now: Instant) -> bool {
        let (min, max, step, vertical) = self.slider_range(idx);
        let unit = if step > 0.0 { step } else { (max - min) / 100.0 };
        let cur = self.slider_value(idx);
        let to = match key {
            Key::ArrowLeft | Key::ArrowDown => cur - unit,
            Key::ArrowRight | Key::ArrowUp => cur + unit,
            Key::PageDown => cur - 10.0 * unit,
            Key::PageUp => cur + 10.0 * unit,
            Key::Home => min,
            Key::End => max,
            _ => return false,
        };
        let _ = vertical;
        self.slider_set(idx, to, true, now);
        true
    }

    /// One step per notch.
    pub(super) fn slider_wheel(&mut self, idx: usize, notches: f32, now: Instant) {
        let (min, max, step, _) = self.slider_range(idx);
        let unit = if step > 0.0 { step } else { (max - min) / 100.0 };
        let to = self.slider_value(idx) + unit * f64::from(notches);
        self.slider_set(idx, to, true, now);
    }

    /// Sends the value a fast drag held back.
    pub(super) fn flush_sliders(&mut self, now: Instant) {
        let due: Vec<NodeKey> = self
            .sliders
            .iter()
            .filter(|(_, s)| s.pending && s.last_report.is_none_or(|t| now.saturating_duration_since(t) >= REPORT_GAP))
            .map(|(k, _)| *k)
            .collect();
        for k in due {
            if let Some(s) = self.sliders.get_mut(&k) {
                s.pending = false;
                s.last_report = Some(now);
            }
            if let Some(i) = self.idx_of(k) {
                self.push_value(i, false);
            }
        }
    }

    pub(super) fn slider_wake(&self) -> Option<Instant> {
        self.sliders.values().filter(|s| s.pending).filter_map(|s| s.last_report).map(|t| t + REPORT_GAP).min()
    }

    pub(super) fn paint_slider(&self, idx: usize, out: &mut Vec<DrawItem>) {
        let n = &self.arena[idx];
        let (min, max, _, vertical) = self.slider_range(idx);
        if max <= min {
            return;
        }
        let frac = ((self.slider_value(idx) - min) / (max - min)).clamp(0.0, 1.0);
        let r = n.laid.rect;
        // Worked out in f64 so 30 % of 100 px is 30 px.
        let part = |len: f32| (f64::from(len) * frac) as f32;
        let fill = if vertical {
            RectF::new(r.x, r.bottom() - part(r.h), r.w, part(r.h))
        } else {
            RectF::new(r.x, r.y, part(r.w), r.h)
        };
        if fill.is_empty() {
            return;
        }
        let colour = n.style.accent_color.unwrap_or(n.style.color);
        let radii = resolve_radii(&n.style.border_radius, &r, self.geometry.scale);
        out.push(n.laid.xf.item(DrawItem {
            node: n.key,
            bounds: fill.outset(1.0),
            cmd: DrawCmd::Fill { rect: fill, radii, fill: Fill::Solid(colour) },
        }));
    }
}

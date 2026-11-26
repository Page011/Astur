//! Pointer input: the hit test against the last drawn frame, hover and active
//! state, and the handler calls that come out.

use std::time::Instant;

use super::shape::{self, Affine};
use super::{Drag, Pressed, Scene};
use crate::element::{ElementKind, HandlerId, NodeKey};
use crate::geom::{PointF, RectF};
use crate::hit::{Button, HandlerCall, InputKind, PlateTarget, PointerEvent, PointerKind, PointerResult};
use crate::style::{CornerShape, Cursor};

/// A clip a point must be inside, in the layout space of the node that made
/// it (`xf` takes layout space to the surface).
struct HitClip {
    rect: Option<RectF>,
    poly: Option<(Vec<PointF>, bool)>,
    xf: Affine,
}

impl HitClip {
    fn contains(&self, p: PointF) -> bool {
        let Some(q) = self.xf.inverse_point(p) else { return false };
        self.rect.is_none_or(|r| r.contains(q))
            && self.poly.as_ref().is_none_or(|(pts, evenodd)| shape::polygon_contains(pts, q, *evenodd))
    }
}

impl Scene {
    /// Node indexes from the root to the topmost, deepest node under `p`.
    fn hit_chain(&self, p: PointF) -> Vec<usize> {
        let mut path = Vec::new();
        if self.root_idx >= self.arena.len() {
            return path;
        }
        let surface = RectF::new(0.0, 0.0, self.geometry.width_px as f32, self.geometry.height_px as f32);
        let mut clips = vec![HitClip { rect: Some(surface), poly: None, xf: Affine::IDENTITY }];
        if !self.visit(self.root_idx, p, &mut clips, &mut path) {
            path.clear();
        }
        path
    }

    /// `hit_chain` for the bar. The strip above the widgets is empty, but a
    /// pointer pushed against the top edge of the screen is in it, so a point
    /// there is tested at the widgets' top row: the column above a widget
    /// belongs to the widget.
    fn bar_hit_chain(&self, p: PointF) -> Vec<usize> {
        if self.popup.is_some() {
            return self.hit_chain(p);
        }
        let top = self
            .arena
            .iter()
            .filter(|n| n.kind == ElementKind::Widget && !n.style.display_none && n.laid.rect.h > 0.0)
            .map(|n| n.laid.rect.y)
            .fold(f32::INFINITY, f32::min);
        if top.is_finite() && p.y < top { self.hit_chain(PointF { x: p.x, y: top + 0.5 }) } else { self.hit_chain(p) }
    }

    fn visit(&self, idx: usize, p: PointF, clips: &mut Vec<HitClip>, path: &mut Vec<usize>) -> bool {
        let n = &self.arena[idx];
        let s = &n.style;
        // `pointer-events: none` skips the element and everything inside it.
        if s.display_none || !s.pointer_events {
            return false;
        }
        path.push(idx);
        let rect = n.laid.rect;
        let xf = n.laid.xf;
        let scale = self.geometry.scale;
        let mut pushed = 0;
        if s.clip || n.laid.clip {
            let b = n.laid.border;
            let pad_box =
                RectF::new(rect.x + b.left, rect.y + b.top, rect.w - b.left - b.right, rect.h - b.top - b.bottom);
            clips.push(HitClip { rect: Some(pad_box), poly: None, xf });
            pushed += 1;
        }
        if let Some(c) = &s.clip_path {
            clips.push(HitClip { rect: None, poly: Some((shape::clip_points(c, &rect, scale), c.evenodd)), xf });
            pushed += 1;
        }
        for &c in n.children.iter().rev() {
            if self.visit(c, p, clips, path) {
                return true;
            }
        }
        clips.truncate(clips.len() - pushed);
        if (s.visible || n.kind == ElementKind::Plate)
            && let Some(q) = xf.inverse_point(p)
            && self.inside_shape(idx, q)
            && clips.iter().all(|c| c.contains(p))
        {
            return true;
        }
        path.pop();
        false
    }

    /// Whether a point in layout space is inside the node's drawn shape: its
    /// border box, bevelled corners cut, and its `clip-path`.
    fn inside_shape(&self, idx: usize, q: PointF) -> bool {
        let n = &self.arena[idx];
        let rect = n.laid.rect;
        if !rect.contains(q) {
            return false;
        }
        let s = &n.style;
        if let Some(c) = &s.clip_path
            && !shape::polygon_contains(&shape::clip_points(c, &rect, self.geometry.scale), q, c.evenodd)
        {
            return false;
        }
        if s.corner_shape.contains(&CornerShape::Bevel) {
            let radii = super::paint::resolve_radii(&s.border_radius, &rect, self.geometry.scale);
            if shape::in_bevel_cut(&rect, &radii, &s.corner_shape, q) {
                return false;
            }
        }
        true
    }

    /// Deepest node of `chain` with a handler for `button`'s click.
    fn click_target(&self, chain: &[usize], button: Button) -> Option<(usize, HandlerId)> {
        chain.iter().rev().filter(|&&i| !self.arena[i].interaction.disabled).find_map(|&i| {
            let h = &self.arena[i].handlers;
            let id = match button {
                Button::Left => h.click,
                Button::Right => h.right_click,
                Button::Middle => h.middle_click,
            };
            id.map(|id| (i, id))
        })
    }

    fn call(&self, idx: usize, handler: HandlerId, kind: InputKind, p: PointF) -> Option<HandlerCall> {
        let n = &self.arena[idx];
        let instance = n.instance?;
        let scale = self.geometry.scale.max(0.01);
        let q = n.laid.xf.inverse_point(p).unwrap_or(p);
        Some(HandlerCall {
            instance,
            handler,
            kind,
            x: (q.x - n.laid.rect.x) / scale,
            y: (q.y - n.laid.rect.y) / scale,
            gesture: 0,
        })
    }

    fn plate_in(&self, chain: &[usize]) -> Option<PlateTarget> {
        chain.iter().find_map(|&i| {
            let n = &self.arena[i];
            (n.kind == ElementKind::Plate).then_some(match n.instance {
                Some(inst) => PlateTarget::Widget(inst),
                None => PlateTarget::Bar,
            })
        })
    }

    fn set_hover(&mut self, chain: &[usize], p: PointF, now: Instant, res: &mut PointerResult) {
        let new_keys: Vec<NodeKey> = chain.iter().map(|&i| self.arena[i].key).collect();
        if new_keys != self.hover_chain {
            // Leave handlers deepest first, then enter handlers root first.
            for key in self.hover_chain.iter().rev().filter(|k| !new_keys.contains(k)) {
                if let Some(i) = self.arena.iter().position(|n| n.key == *key)
                    && let Some(h) = self.arena[i].handlers.leave
                {
                    res.calls.extend(self.call(i, h, InputKind::Leave, p));
                }
            }
            for &i in chain {
                if !self.hover_chain.contains(&self.arena[i].key)
                    && let Some(h) = self.arena[i].handlers.enter
                {
                    res.calls.extend(self.call(i, h, InputKind::Enter, p));
                }
            }
            self.hover = new_keys.iter().copied().collect();
            self.hover_chain = new_keys;
            self.rebuild = true;
            self.state_time = Some(now);
            self.hover_submenu(chain);
        }
        let text = chain.last().is_some_and(|&i| self.arena[i].style.cursor == Cursor::Text);
        let pointer = chain.last().is_some_and(|&i| self.arena[i].style.cursor == Cursor::Pointer)
            || chain.iter().any(|&i| self.arena[i].handlers.any_click() || self.arena[i].kind == ElementKind::Plate);
        self.cursor = if text {
            Cursor::Text
        } else if pointer {
            Cursor::Pointer
        } else {
            Cursor::Default
        };
    }

    /// Hovering a menu item that holds a submenu opens it and closes the
    /// submenus of the items beside it.
    fn hover_submenu(&mut self, chain: &[usize]) {
        if self.popup.is_none() || self.recs.is_empty() {
            return;
        }
        let Some(&item) = chain.iter().rev().find(|&&i| self.arena[i].kind == ElementKind::MenuItem) else { return };
        let item_key = self.arena[item].key;
        let enabled = !self.arena[item].interaction.disabled;
        let subs: Vec<(NodeKey, NodeKey)> =
            self.recs.iter().filter(|r| r.kind == ElementKind::Menu).map(|r| (r.key, r.anchor)).collect();
        for (sub, anchor) in subs {
            if anchor == item_key && enabled {
                self.sub_open.insert(sub);
            } else {
                self.sub_open.remove(&sub);
            }
        }
    }

    fn set_active(&mut self, chain: &[usize], now: Instant) {
        let keys: std::collections::HashSet<NodeKey> = chain.iter().map(|&i| self.arena[i].key).collect();
        if keys != self.active {
            self.active = keys;
            self.rebuild = true;
            self.state_time = Some(now);
        }
    }

    pub(super) fn pointer_impl(&mut self, ev: PointerEvent, now: Instant) -> PointerResult {
        let mut res = PointerResult::default();
        // A closing popup keeps its picture but takes no input: its handlers
        // may be gone.
        if self.is_closing() {
            return res;
        }
        let p = PointF { x: ev.x, y: ev.y };
        if self.overlay_guarded(now) {
            // A press before the guard lifts is dropped, and so is its release.
            match ev.kind {
                PointerKind::Down(_) => {
                    if let Some(pop) = &mut self.popup {
                        pop.dropped_press = true;
                    }
                    return res;
                }
                PointerKind::Up(_) | PointerKind::Wheel(_) | PointerKind::HWheel(_) => return res,
                _ => {}
            }
        } else if let Some(pop) = &mut self.popup {
            match ev.kind {
                PointerKind::Up(_) if pop.dropped_press => {
                    pop.dropped_press = false;
                    return res;
                }
                PointerKind::Down(_) => pop.dropped_press = false,
                _ => {}
            }
        }
        // A button held on an input, slider or scrollbar steers it wherever the pointer goes.
        if let Some(d) = self.drag {
            match (ev.kind, d) {
                (PointerKind::Move, Drag::Input(k)) => self.input_drag(k, p, now),
                (PointerKind::Move, Drag::Slider(k)) => self.slider_drag(k, p, now),
                (PointerKind::Move, Drag::Thumb { key, axis, grab }) => self.thumb_drag(key, axis, grab, p, now),
                (PointerKind::Up(_), d) => {
                    let mut click = None;
                    if let Drag::Slider(k) = d {
                        click = self.slider_release(k, now);
                    }
                    self.drag = None;
                    self.set_active(&[], now);
                    self.pressed = None;
                    // An unmoved press on a slider that does not jump is a click.
                    if let Some(i) = click
                        && let PointerKind::Up(Button::Left) = ev.kind
                        && let Some(h) = self.arena[i].handlers.click
                        && !self.arena[i].interaction.disabled
                        && self.hit_chain(p).contains(&i)
                    {
                        res.calls.extend(self.call(i, h, InputKind::Click(Button::Left), p));
                    }
                    return res;
                }
                _ => {}
            }
            if matches!(ev.kind, PointerKind::Move) {
                return res;
            }
        }
        match ev.kind {
            PointerKind::Leave => {
                self.set_hover(&[], p, now, &mut res);
                self.set_active(&[], now);
                self.pressed = None;
            }
            PointerKind::Move => {
                let chain = self.bar_hit_chain(p);
                self.set_hover(&chain, p, now, &mut res);
            }
            PointerKind::Down(button) => {
                let chain = self.bar_hit_chain(p);
                res.missed = chain.is_empty();
                self.set_hover(&chain, p, now, &mut res);
                if button == Button::Left && self.thumb_press(&chain, p, now) {
                    return res;
                }
                if self.popup.is_some()
                    && button == Button::Left
                    && let Some(i) = chain.iter().rev().copied().find(|&i| self.is_focusable(i))
                {
                    self.set_focus(Some(i), false, now);
                    match self.arena[i].kind {
                        ElementKind::Input => self.input_press(i, p, now),
                        ElementKind::Slider => self.slider_press(i, p, now),
                        _ => {}
                    }
                }
                self.set_active(&chain, now);
                let menu_item = chain
                    .iter()
                    .rev()
                    .find(|&&i| self.arena[i].kind == ElementKind::MenuItem)
                    .map(|&i| self.arena[i].key);
                self.pressed = Some(Pressed {
                    button,
                    target: self.click_target(&chain, button).map(|(i, _)| self.arena[i].key),
                    plate: self.plate_in(&chain).is_some(),
                    menu_item,
                });
            }
            PointerKind::Up(button) => {
                let chain = self.bar_hit_chain(p);
                self.set_hover(&chain, p, now, &mut res);
                self.set_active(&[], now);
                let target = self.click_target(&chain, button);
                let plate = self.plate_in(&chain);
                let pressed = self.pressed.take().filter(|d| d.button == button);
                match (&pressed, target, plate) {
                    // Down and up on the node that has the handler.
                    (Some(d), Some((i, h)), _) if d.target == Some(self.arena[i].key) => {
                        res.calls.extend(self.call(i, h, InputKind::Click(button), p));
                    }
                    (Some(d), _, Some(pl)) if d.plate => res.plate_click = Some(pl),
                    (_, None, None) => res.unhandled = Some(button),
                    _ => {}
                }
                // A menu item pressed and released opens its submenu or is picked.
                if button == Button::Left
                    && let Some(mk) = pressed.as_ref().and_then(|d| d.menu_item)
                    && chain.iter().any(|&i| self.arena[i].key == mk)
                    && let Some(mi) = self.idx_of(mk)
                    && !self.arena[mi].interaction.disabled
                    && !self.open_submenu(mi)
                {
                    self.picked();
                }
            }
            PointerKind::Wheel(notches) => {
                let chain = self.bar_hit_chain(p);
                if let Some(&i) = chain.iter().rev().find(|&&i| self.arena[i].kind == ElementKind::Slider) {
                    self.slider_wheel(i, notches, now);
                } else if !self.wheel_scroll(&chain, notches, now)
                    && let Some(&i) = chain.iter().rev().find(|&&i| self.arena[i].handlers.wheel.is_some())
                    && let Some(h) = self.arena[i].handlers.wheel
                {
                    res.calls.extend(self.call(i, h, InputKind::Wheel(notches), p));
                }
            }
            PointerKind::HWheel(notches) => {
                let chain = self.bar_hit_chain(p);
                // Shift with the wheel and the tilt wheel arrive here: where nothing scrolls sideways they
                // reach the widget's own `on_wheel`, turned back into the vertical direction.
                if !self.wheel_scroll_sideways(&chain, notches, now)
                    && let Some(&i) = chain.iter().rev().find(|&&i| self.arena[i].handlers.wheel.is_some())
                    && let Some(h) = self.arena[i].handlers.wheel
                {
                    res.calls.extend(self.call(i, h, InputKind::Wheel(-notches), p));
                }
            }
        }
        res
    }
}

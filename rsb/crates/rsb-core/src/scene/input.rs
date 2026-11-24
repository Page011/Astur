//! Pointer input: the hit test against the last drawn frame, hover and active
//! state, and the handler calls that come out.

use std::time::Instant;

use super::{Pressed, Scene};
use crate::element::{ElementKind, HandlerId, NodeKey};
use crate::geom::{PointF, RectF};
use crate::hit::{Button, HandlerCall, InputKind, PlateTarget, PointerEvent, PointerKind, PointerResult};
use crate::style::Cursor;

impl Scene {
    /// Node indexes from the root to the topmost, deepest node under `p`.
    fn hit_chain(&self, p: PointF) -> Vec<usize> {
        let mut path = Vec::new();
        if self.arena.is_empty() {
            return path;
        }
        let surface = RectF::new(0.0, 0.0, self.geometry.width_px as f32, self.geometry.height_px as f32);
        if !self.visit(0, p, &surface, &mut path) {
            path.clear();
        }
        path
    }

    fn visit(&self, idx: usize, p: PointF, clip: &RectF, path: &mut Vec<usize>) -> bool {
        let n = &self.arena[idx];
        let s = &n.style;
        if s.display_none {
            return false;
        }
        path.push(idx);
        let rect = n.laid.rect;
        let inner = if s.clip || n.laid.clip {
            let b = n.laid.border;
            let pad_box =
                RectF::new(rect.x + b.left, rect.y + b.top, rect.w - b.left - b.right, rect.h - b.top - b.bottom);
            clip.intersect(&pad_box).unwrap_or(RectF::new(0.0, 0.0, 0.0, 0.0))
        } else {
            *clip
        };
        for &c in n.children.iter().rev() {
            if self.visit(c, p, &inner, path) {
                return true;
            }
        }
        if (s.visible || n.kind == ElementKind::Plate) && rect.contains(p) && clip.contains(p) {
            return true;
        }
        path.pop();
        false
    }

    /// Deepest node of `chain` with a handler for `button`'s click.
    fn click_target(&self, chain: &[usize], button: Button) -> Option<(usize, HandlerId)> {
        chain.iter().rev().find_map(|&i| {
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
        Some(HandlerCall {
            instance,
            handler,
            kind,
            x: (p.x - n.laid.rect.x) / scale,
            y: (p.y - n.laid.rect.y) / scale,
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
        }
        let pointer = chain.last().is_some_and(|&i| self.arena[i].style.cursor == Cursor::Pointer)
            || chain.iter().any(|&i| self.arena[i].handlers.any_click() || self.arena[i].kind == ElementKind::Plate);
        self.cursor = if pointer { Cursor::Pointer } else { Cursor::Default };
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
        let p = PointF { x: ev.x, y: ev.y };
        match ev.kind {
            PointerKind::Leave => {
                self.set_hover(&[], p, now, &mut res);
                self.set_active(&[], now);
                self.pressed = None;
            }
            PointerKind::Move => {
                let chain = self.hit_chain(p);
                self.set_hover(&chain, p, now, &mut res);
            }
            PointerKind::Down(button) => {
                let chain = self.hit_chain(p);
                self.set_hover(&chain, p, now, &mut res);
                self.set_active(&chain, now);
                self.pressed = Some(Pressed {
                    button,
                    target: self.click_target(&chain, button).map(|(i, _)| self.arena[i].key),
                    plate: self.plate_in(&chain).is_some(),
                });
            }
            PointerKind::Up(button) => {
                let chain = self.hit_chain(p);
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
            }
            PointerKind::Wheel(notches) => {
                let chain = self.hit_chain(p);
                if let Some(&i) = chain.iter().rev().find(|&&i| self.arena[i].handlers.wheel.is_some())
                    && let Some(h) = self.arena[i].handlers.wheel
                {
                    res.calls.extend(self.call(i, h, InputKind::Wheel(notches), p));
                }
            }
        }
        res
    }
}

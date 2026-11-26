//! Keyboard focus: which element has it, Tab order, `nav` groups.

use std::time::Instant;

use super::Scene;
use crate::element::{ElementKind, NavAxis, NodeKey};
use crate::hit::{ElementCall, ElementEvent};
use crate::key::Key;

#[derive(Default)]
pub(super) struct FocusState {
    pub node: Option<NodeKey>,
    /// The surface's window has keyboard focus.
    pub window: bool,
    /// Focus last moved by keyboard.
    pub visible: bool,
}

impl Scene {
    pub(super) fn idx_of(&self, key: NodeKey) -> Option<usize> {
        self.arena.iter().position(|n| n.key == key)
    }

    pub(super) fn focused_idx(&self) -> Option<usize> {
        self.focus.node.and_then(|k| self.idx_of(k))
    }

    /// Brings the arena up to date with what was set since the last frame, so
    /// focus and key calls see the elements that were just given.
    pub(super) fn sync(&mut self, now: Instant) {
        if self.rebuild {
            self.refresh(now);
            self.rebuild = false;
        }
    }

    /// Not hidden by `display: none` on it or an ancestor, and visible.
    pub(super) fn is_shown(&self, idx: usize) -> bool {
        let mut i = Some(idx);
        while let Some(n) = i {
            if self.arena[n].style.display_none {
                return false;
            }
            i = self.arena[n].parent;
        }
        self.arena[idx].style.visible
    }

    pub(super) fn is_focusable(&self, idx: usize) -> bool {
        let n = &self.arena[idx];
        let kind_ok = matches!(n.kind, ElementKind::Input | ElementKind::Slider | ElementKind::MenuItem);
        (kind_ok || n.interaction.focusable) && !n.interaction.disabled && self.is_shown(idx)
    }

    /// The focusable elements below `within` (the surface root when None), in
    /// tree order.
    pub(super) fn focusables(&self, within: Option<usize>) -> Vec<usize> {
        (self.root_idx..self.arena.len())
            .filter(|&i| self.is_focusable(i) && within.is_none_or(|w| self.is_under(i, w)))
            .collect()
    }

    /// `idx` is `ancestor` or below it.
    pub(super) fn is_under(&self, idx: usize, ancestor: usize) -> bool {
        let mut i = Some(idx);
        while let Some(n) = i {
            if n == ancestor {
                return true;
            }
            i = self.arena[n].parent;
        }
        false
    }

    pub(super) fn set_focus(&mut self, to: Option<usize>, keyboard: bool, now: Instant) {
        let new = to.map(|i| self.arena[i].key);
        let old = self.focus.node;
        if old == new {
            if old.is_some() && self.focus.visible != keyboard && keyboard {
                self.focus.visible = true;
                self.rebuild = true;
                self.state_time = Some(now);
            }
            return;
        }
        if let Some(i) = old.and_then(|k| self.idx_of(k)) {
            self.focus_event(i, false);
        }
        self.focus.node = new;
        self.focus.visible = keyboard && new.is_some();
        if let Some(i) = to {
            self.focus_event(i, true);
            // A click is on something already in view.
            if keyboard {
                self.pending_reveals.extend(new);
            }
            if let Some(e) = self.edits.get_mut(&self.arena[i].key) {
                e.epoch = now;
            }
        }
        self.repaint = true;
        self.rebuild = true;
        self.state_time = Some(now);
    }

    fn focus_event(&mut self, idx: usize, gained: bool) {
        let n = &self.arena[idx];
        let handler = if gained { n.handlers.focus } else { n.handlers.blur };
        if let (Some(handler), Some(instance)) = (handler, n.instance) {
            let event = if gained { ElementEvent::Focus } else { ElementEvent::Blur };
            self.events.push(ElementCall { instance, handler, event, gesture: 0 });
        }
    }

    pub(super) fn set_window_focus_impl(&mut self, focused: bool, now: Instant) {
        if self.focus.window == focused {
            return;
        }
        self.focus.window = focused;
        if let Some(e) = self.focus.node.and_then(|k| self.edits.get_mut(&k)) {
            e.epoch = now;
        }
        self.repaint = true;
        self.rebuild = true;
        self.state_time = Some(now);
    }

    pub(super) fn focus_first_impl(&mut self, keyboard: bool, now: Instant) {
        if self.popup.is_none() {
            return;
        }
        self.sync(now);
        let all = self.focusables(None);
        let auto = all.iter().copied().find(|&i| self.arena[i].interaction.autofocus);
        let pick = auto.or(if keyboard { all.first().copied() } else { None });
        if let Some(i) = pick {
            self.set_focus(Some(i), keyboard, now);
            if !keyboard {
                self.pending_reveals.push(self.arena[i].key);
            }
        } else if keyboard {
            // Nothing to focus: `:focus-visible` still records how the
            // surface was opened.
            self.focus.visible = true;
        }
    }

    /// Tab and Shift+Tab: the next or previous focusable element, wrapping.
    pub(super) fn tab(&mut self, back: bool, now: Instant) -> bool {
        let all = self.focusables(None);
        if all.is_empty() {
            return false;
        }
        let pos = self.focused_idx().and_then(|f| all.iter().position(|&i| i == f));
        let next = match (pos, back) {
            (None, false) => 0,
            (None, true) => all.len() - 1,
            (Some(p), false) => (p + 1) % all.len(),
            (Some(p), true) => (p + all.len() - 1) % all.len(),
        };
        self.set_focus(Some(all[next]), true, now);
        true
    }

    /// The nearest element at or above `idx` that moves focus with arrows.
    pub(super) fn nav_group_of(&self, idx: usize) -> Option<(usize, NavAxis)> {
        let mut i = Some(idx);
        while let Some(n) = i {
            let node = &self.arena[n];
            match node.interaction.nav {
                NavAxis::None if node.kind == ElementKind::Menu => return Some((n, NavAxis::Vertical)),
                NavAxis::None => {}
                axis => return Some((n, axis)),
            }
            i = node.parent;
        }
        None
    }

    /// An arrow key inside a `nav` group. False when it does nothing.
    pub(super) fn nav_move(&mut self, key: Key, now: Instant) -> bool {
        let Some(cur) = self.focused_idx() else { return false };
        let Some((group, axis)) = self.nav_group_of(cur) else { return false };
        let items = self.focusables(Some(group));
        let Some(pos) = items.iter().position(|&i| i == cur) else { return false };
        let step = |forward: bool| {
            let n = items.len();
            items[if forward { (pos + 1) % n } else { (pos + n - 1) % n }]
        };
        let to = match (axis, key) {
            (NavAxis::Vertical, Key::ArrowDown)
            | (NavAxis::Horizontal, Key::ArrowRight)
            | (NavAxis::Grid, Key::ArrowRight) => Some(step(true)),
            (NavAxis::Vertical, Key::ArrowUp)
            | (NavAxis::Horizontal, Key::ArrowLeft)
            | (NavAxis::Grid, Key::ArrowLeft) => Some(step(false)),
            (NavAxis::Grid, Key::ArrowDown) => self.grid_row(&items, cur, true),
            (NavAxis::Grid, Key::ArrowUp) => self.grid_row(&items, cur, false),
            _ => return false,
        };
        if let Some(to) = to {
            self.set_focus(Some(to), true, now);
        }
        true
    }

    /// The item in the row below or above `cur` whose centre is nearest across.
    fn grid_row(&self, items: &[usize], cur: usize, down: bool) -> Option<usize> {
        let centre = |i: usize| {
            let r = self.arena[i].laid.rect;
            (r.x + r.w / 2.0, r.y + r.h / 2.0, r.h)
        };
        let (cx, cy, ch) = centre(cur);
        let tol = ch / 2.0;
        let rows: Vec<(usize, f32, f32)> = items
            .iter()
            .filter(|&&i| i != cur)
            .map(|&i| {
                let (x, y, _) = centre(i);
                (i, x, y)
            })
            .filter(|&(_, _, y)| if down { y > cy + tol } else { y < cy - tol })
            .collect();
        // The nearest row first, then the nearest across inside it.
        let row_y = rows.iter().map(|r| r.2).min_by(|a, b| (a - cy).abs().total_cmp(&(b - cy).abs()))?;
        rows.iter()
            .filter(|r| (r.2 - row_y).abs() <= tol)
            .min_by(|a, b| (a.1 - cx).abs().total_cmp(&(b.1 - cx).abs()))
            .map(|r| r.0)
    }
}

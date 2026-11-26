//! Key routing: what a key down does on a surface that has keyboard focus.

use std::time::Instant;

use super::edit::Used;
use super::{KeyResult, Scene};
use crate::element::{ElementKind, NodeKey};
use crate::hit::{Button, ElementCall, ElementEvent, HandlerCall, InputKind};
use crate::key::{Key, KeyEvent};
use crate::popup::{CloseReason, Side};

impl Scene {
    pub(super) fn key_impl(&mut self, ev: KeyEvent, now: Instant) -> KeyResult {
        let mut res = KeyResult::default();
        if !ev.down || self.popup.is_none() || self.is_closing() {
            return res;
        }
        if self.overlay_guarded(now) {
            // Moving the focus along the buttons is harmless, so it works while
            // they are still opening; nothing else runs until the guard lifts.
            self.sync(now);
            if ev.key == Key::Escape && self.overlay_escape_ready(now) {
                let allowed = self.popup.as_ref().and_then(|p| p.root_props.as_ref()).is_none_or(|p| p.dismiss.escape);
                res.dismiss = allowed;
                res.consumed = allowed;
                return res;
            }
            res.consumed = match ev.key {
                Key::Tab if !ev.mods.ctrl && !ev.mods.alt => self.tab(ev.mods.shift, now),
                Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight => self.nav_move(ev.key, now),
                _ => false,
            };
            return res;
        }
        self.sync(now);
        let focused = self.focused_idx();

        // 1. The focused element's own handling.
        if let Some(i) = focused {
            let handled = match self.arena[i].kind {
                ElementKind::Input => match self.edit_key(i, ev, now, &mut res) {
                    Used::Yes => true,
                    Used::AndKey => {
                        self.send_key(Some(i), ev);
                        true
                    }
                    Used::No => false,
                },
                ElementKind::Slider => self.slider_key(i, ev.key, now),
                ElementKind::MenuItem => self.menu_key(i, ev, now, &mut res),
                _ => false,
            };
            if handled || self.scroll_key(i, ev.key, now) {
                res.consumed = true;
                return res;
            }
        }

        // 2. Tab and Shift+Tab.
        if ev.key == Key::Tab && !ev.mods.ctrl && !ev.mods.alt && self.tab(ev.mods.shift, now) {
            res.consumed = true;
            return res;
        }

        // 3. Arrows inside a `nav` group.
        if matches!(ev.key, Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight)
            && self.nav_move(ev.key, now)
        {
            res.consumed = true;
            return res;
        }

        // 4. Enter or Space on a focused element runs its click.
        if matches!(ev.key, Key::Enter | Key::Space)
            && let Some(i) = focused
            && self.arena[i].kind != ElementKind::Input
        {
            if self.is_overlay() && ev.repeat {
                res.consumed = true;
                return res;
            }
            if let Some(call) = self.click_call(i) {
                res.calls.push(call);
                res.consumed = true;
                return res;
            }
        }

        // 5. The script's `on_key`, then 6. Escape closes the surface.
        self.send_key(focused, ev);
        if ev.key == Key::Escape
            && self.popup.as_ref().and_then(|p| p.root_props.as_ref()).is_none_or(|p| p.dismiss.escape)
        {
            res.dismiss = true;
        }
        res
    }

    fn is_overlay(&self) -> bool {
        self.popup.as_ref().is_some_and(|p| p.spec.side == Side::Screen)
    }

    /// The element's click handler as a key press makes it (the pointer at its centre).
    fn click_call(&self, idx: usize) -> Option<HandlerCall> {
        let n = &self.arena[idx];
        let handler = n.handlers.click?;
        let instance = n.instance?;
        if n.interaction.disabled {
            return None;
        }
        let scale = self.geometry.scale.max(0.01);
        Some(HandlerCall {
            instance,
            handler,
            kind: InputKind::Click(Button::Left),
            x: n.laid.rect.w / 2.0 / scale,
            y: n.laid.rect.h / 2.0 / scale,
            gesture: 0,
        })
    }

    /// `on_key` of the element, else of the nearest ancestor with one, up to
    /// the surface root.
    fn send_key(&mut self, from: Option<usize>, ev: KeyEvent) {
        let mut i = from.unwrap_or(self.root_idx);
        loop {
            let Some(n) = self.arena.get(i) else { return };
            if let (Some(handler), Some(instance)) = (n.handlers.key, n.instance) {
                self.events.push(ElementCall { instance, handler, event: ElementEvent::Key(ev), gesture: 0 });
                return;
            }
            if i <= self.root_idx {
                return;
            }
            let Some(p) = n.parent else { return };
            i = p;
        }
    }

    /// The submenu declared inside a menu item.
    fn submenu_of(&self, idx: usize) -> Option<NodeKey> {
        let key = self.arena[idx].key;
        self.recs.iter().find(|r| r.kind == ElementKind::Menu && r.anchor == key).map(|r| r.key)
    }

    pub(super) fn open_submenu(&mut self, idx: usize) -> bool {
        match self.submenu_of(idx) {
            Some(k) => {
                self.sub_open.insert(k);
                true
            }
            None => false,
        }
    }

    /// The menu's label of an item, for jumping to it by letter.
    fn label_of(&self, idx: usize) -> String {
        let n = &self.arena[idx];
        if let Some(t) = n.text.as_deref().filter(|t| !t.is_empty()) {
            return t.to_string();
        }
        n.children
            .iter()
            .find_map(|&c| self.arena[c].text.as_deref().filter(|_| self.arena[c].kind == ElementKind::Text))
            .unwrap_or("")
            .to_string()
    }

    /// Menu navigation on a focused item (Up and Down are the `nav` group's).
    fn menu_key(&mut self, idx: usize, ev: KeyEvent, now: Instant, res: &mut KeyResult) -> bool {
        let submenu = self.popup.as_ref().is_some_and(|p| p.submenu);
        match ev.key {
            Key::ArrowRight => self.open_submenu(idx),
            Key::ArrowLeft if submenu => {
                res.dismiss = true;
                true
            }
            Key::Enter | Key::Space => {
                if self.is_overlay() && ev.repeat {
                    return true;
                }
                if self.open_submenu(idx) {
                    return true;
                }
                if let Some(call) = self.click_call(idx) {
                    res.calls.push(call);
                }
                self.picked();
                true
            }
            Key::Home | Key::End => {
                let all = self.focusables(None);
                let to = if ev.key == Key::Home { all.first() } else { all.last() };
                if let Some(&to) = to {
                    self.set_focus(Some(to), true, now);
                }
                true
            }
            Key::Char(c) if !ev.mods.ctrl && !ev.mods.alt => {
                let all = self.focusables(None);
                let Some(pos) = all.iter().position(|&i| i == idx) else { return false };
                let want = c.to_lowercase().next().unwrap_or(c);
                let next = (1..=all.len())
                    .map(|d| all[(pos + d) % all.len()])
                    .find(|&i| self.label_of(i).chars().next().is_some_and(|f| f.to_lowercase().next() == Some(want)));
                if let Some(i) = next {
                    self.set_focus(Some(i), true, now);
                }
                true
            }
            _ => false,
        }
    }

    /// A menu item was picked: the whole chain closes. The scene tells the
    /// script through the popup's `on_close`.
    pub(super) fn picked(&mut self) {
        self.pick_pending = true;
        let Some(p) = &self.popup else { return };
        let (Some(props), Some(c)) = (&p.root_props, &p.content) else { return };
        if let Some(handler) = props.on_close {
            let event = ElementEvent::PopupClosed(CloseReason::Picked);
            self.events.push(ElementCall { instance: c.tree.instance, handler, event, gesture: 0 });
        }
    }
}

//! Input to popup surfaces and the dismissal rules it drives: pointer
//! presses, keys, text, clipboard, focus changes, presses outside every
//! window, the session lock, and `rsb popup`.

use std::time::Instant;

use rsb_core::geom::PointF;
use rsb_core::hit::{ElementCall, ElementEvent, HandlerCall, InputKind, PointerEvent, PointerKind};
use rsb_core::ipc::Reply;
use rsb_core::key::{KeyEvent, TextInput};
use rsb_core::msg::ScriptIn;
use rsb_core::popup::{CloseReason, OVERLAY_INPUT_GUARD, PopupAction};
use rsb_core::scene::{BarGeometry, KeyResult};
use rsb_win::{BarId, PopupId, SurfaceRole};
use serde_json::json;

use super::{Anchor, Env, FAKE_CLIPBOARD, Popups, Stage};
use crate::gesture::GestureKind;

/// The key being handled, so a submenu it opens can say which gesture opened it.
#[derive(Clone, Copy, Debug)]
pub(super) struct KeyCtx {
    pub popup: u64,
    pub gesture: u64,
}

/// A paste waiting for the clipboard worker.
#[derive(Clone, Copy, Debug)]
pub(super) struct PasteRead {
    popup: u64,
    started: Instant,
}

fn element_kind(event: &ElementEvent) -> &'static str {
    match event {
        ElementEvent::Key(_) => "key",
        ElementEvent::TextChanged(_) => "change",
        ElementEvent::Submitted(_) => "submit",
        ElementEvent::ValueChanged { .. } => "value",
        ElementEvent::Focus => "focus",
        ElementEvent::Blur => "blur",
        ElementEvent::Scrolled { .. } => "scroll",
        ElementEvent::PopupClosed(_) => "close",
        ElementEvent::PopupOpened(_) => "open",
    }
}

fn ime_stage(input: &TextInput) -> Option<&'static str> {
    match input {
        TextInput::Chars(_) => None,
        TextInput::CompositionStart => Some("start"),
        TextInput::CompositionUpdate { .. } => Some("update"),
        TextInput::CompositionCommit(_) => Some("commit"),
        TextInput::CompositionEnd => Some("end"),
    }
}

impl Popups {
    // ------------------------------------------------------------ gestures and the script

    /// The gesture a pointer event belongs to: a press starts one, its
    /// release (and so its click) carries it, nothing else is one.
    pub fn pointer_gesture(&mut self, kind: PointerKind, now: Instant) -> u64 {
        match kind {
            PointerKind::Down(_) => {
                let g = self.gestures.begin(now, GestureKind::Pointer);
                self.press = Some(g);
                g
            }
            PointerKind::Up(_) => {
                let g = self.press.unwrap_or(0);
                if g != 0 {
                    self.gestures.touch(g, now);
                }
                g
            }
            _ => 0,
        }
    }

    /// Handler calls to the script; a click carries the gesture of its press.
    pub fn send_calls(&self, calls: Vec<HandlerCall>, gesture: u64) {
        for mut call in calls {
            if matches!(call.kind, InputKind::Click(_)) {
                call.gesture = gesture;
            }
            self.send(ScriptIn::Input(call));
        }
    }

    /// Element events from a bar's scene.
    pub fn send_bar_events(&self, events: Vec<ElementCall>, gesture: u64) {
        for mut call in events {
            call.gesture = gesture;
            self.stats.emit_with("element", || json!({"kind": element_kind(&call.event)}));
            self.send(ScriptIn::Element(call));
        }
    }

    /// Element events a popup's scene queued (keys, typing, values, a picked
    /// menu item). A picked item closes the whole chain, after the script
    /// heard about the pick.
    pub(super) fn send_scene_events(&mut self, id: u64, gesture: u64, env: &mut Env<'_>) {
        let Some(i) = self.idx(id) else { return };
        let events = self.managed[i].scene.take_events();
        if events.is_empty() {
            return;
        }
        let mut picked = false;
        for mut call in events {
            if matches!(call.event, ElementEvent::PopupClosed(CloseReason::Picked)) {
                self.picked_sent.insert((call.instance, call.handler));
                picked = true;
            }
            call.gesture = gesture;
            self.stats.emit_with("element", || json!({"kind": element_kind(&call.event)}));
            self.send(ScriptIn::Element(call));
        }
        if picked {
            self.close_chain(id, CloseReason::Picked, env);
            self.picked_sent.clear();
        }
    }

    fn close_with(&mut self, id: u64, reason: CloseReason, gesture: u64, env: &mut Env<'_>) {
        self.close_gesture = gesture;
        self.close(id, reason, env);
        self.close_gesture = 0;
    }

    fn root_of(&self, mut id: u64) -> u64 {
        while let Some(p) = self.idx(id).and_then(|i| self.managed[i].parent) {
            id = p;
        }
        id
    }

    // ------------------------------------------------------------ pointer

    /// A button went down on one of our bars (before the bar handles it).
    /// Every popup chain closes as `Outside` unless the press is on the
    /// anchor of its first popup: the anchor's own handler decides then.
    pub fn on_bar_press(&mut self, env: &mut Env<'_>, bar: BarId, x: f32, y: f32, gesture: u64) {
        self.hide_tip(env);
        let roots: Vec<(u64, bool)> = self
            .managed
            .iter()
            .filter(|m| {
                m.parent.is_none()
                    && m.stage != Stage::Closing
                    && m.role != SurfaceRole::Tooltip
                    && m.props.dismiss.outside
            })
            .map(|m| (m.id, m.anchor == Anchor::Bar(bar) && m.anchor_local.contains(PointF { x, y })))
            .collect();
        for (id, on_anchor) in roots {
            if !on_anchor {
                self.close_with(id, CloseReason::Outside, gesture, env);
            }
        }
    }

    /// After any pointer event on a bar: the tooltip it hovers.
    pub fn bar_pointer_done(&mut self, env: &mut Env<'_>, bar: BarId, kind: PointerKind) {
        self.tip_pointer(env, Anchor::Bar(bar), kind);
    }

    /// Pointer input on a popup, menu, tooltip or overlay window, or an overlay's dim.
    pub fn on_popup_pointer(&mut self, env: &mut Env<'_>, win: PopupId, event: PointerEvent) {
        let now = env.now;
        let found = self
            .managed
            .iter()
            .position(|m| m.win == win)
            .map(|i| (i, false))
            .or_else(|| self.managed.iter().position(|m| m.dim == Some(win)).map(|i| (i, true)));
        let Some((i, is_dim)) = found else { return };
        let (id, closing) = (self.managed[i].id, self.managed[i].stage == Stage::Closing);
        let gesture = self.pointer_gesture(event.kind, now);
        if closing {
            return;
        }
        if is_dim {
            // A press that reaches the dim is outside the overlay's content, once the guard has lifted.
            let m = &self.managed[i];
            let lifted = now.saturating_duration_since(m.opened_at) >= OVERLAY_INPUT_GUARD;
            if matches!(event.kind, PointerKind::Down(_)) && lifted {
                self.close_with(id, CloseReason::Outside, gesture, env);
            }
            return;
        }
        let down = matches!(event.kind, PointerKind::Down(_));
        if down {
            // A press inside this chain is outside every other.
            let mine = self.root_of(id);
            let others: Vec<u64> = self
                .managed
                .iter()
                .filter(|m| {
                    m.parent.is_none()
                        && m.id != mine
                        && m.stage != Stage::Closing
                        && m.role != SurfaceRole::Tooltip
                        && m.props.dismiss.outside
                })
                .map(|m| m.id)
                .collect();
            for other in others {
                self.close_with(other, CloseReason::Outside, gesture, env);
            }
        }
        let Some(i) = self.idx(id) else { return };
        let result = self.managed[i].scene.pointer(event, now);
        if down && result.missed {
            // The window's transparent margin, or a part that is not grown yet.
            if self.managed[i].props.dismiss.outside {
                self.close_with(id, CloseReason::Outside, gesture, env);
            }
            return;
        }
        if down {
            self.press_activate(id, env);
        }
        let picked = result.picked;
        self.send_calls(result.calls, gesture);
        self.send_scene_events(id, gesture, env);
        if picked {
            self.close_chain(id, CloseReason::Picked, env);
        }
        self.tip_pointer(env, Anchor::Popup(id), event.kind);
    }

    /// A button went down outside every window of ours (seen by the watch).
    /// Popups without keyboard focus close; the ones that hold it hear of
    /// the focus change instead.
    pub fn on_outside_press(&mut self, env: &mut Env<'_>) {
        self.hide_tip(env);
        let ids: Vec<u64> = self
            .managed
            .iter()
            .filter(|m| {
                m.parent.is_none()
                    && m.stage != Stage::Closing
                    && m.role != SurfaceRole::Tooltip
                    && !m.holds
                    && m.props.dismiss.outside
            })
            .map(|m| m.id)
            .collect();
        for id in ids {
            self.close(id, CloseReason::Outside, env);
        }
    }

    // ------------------------------------------------------------ keys, text, clipboard, focus

    pub fn on_key(&mut self, env: &mut Env<'_>, win: PopupId, event: KeyEvent) {
        let Some(i) = self.managed.iter().position(|m| m.win == win) else { return };
        if self.managed[i].stage == Stage::Closing || !event.down {
            return;
        }
        if !self.managed[i].holds {
            // A key can only reach a popup that holds focus.
            self.stats.emit("input_dropped", json!({"kind": "key"}));
            return;
        }
        let (id, now) = (self.managed[i].id, env.now);
        let gesture = self.gestures.begin(now, GestureKind::Key);
        self.key_ctx = Some(KeyCtx { popup: id, gesture });
        let result = self.managed[i].scene.key(event, now);
        self.finish_key(id, result, gesture, env);
    }

    pub fn on_text(&mut self, env: &mut Env<'_>, win: PopupId, input: TextInput) {
        let Some(i) = self.managed.iter().position(|m| m.win == win) else { return };
        if self.managed[i].stage == Stage::Closing {
            return;
        }
        if !self.managed[i].holds {
            self.stats.emit("input_dropped", json!({"kind": "text"}));
            return;
        }
        if let Some(stage) = ime_stage(&input) {
            self.stats.emit("ime", json!({"stage": stage}));
        }
        let (id, now) = (self.managed[i].id, env.now);
        let result = self.managed[i].scene.text_input(input, now);
        self.finish_key(id, result, 0, env);
    }

    fn finish_key(&mut self, id: u64, result: KeyResult, gesture: u64, env: &mut Env<'_>) {
        let picked = result.picked;
        self.send_calls(result.calls, gesture);
        if let Some(text) = result.copy {
            self.clipboard_write(text, env);
        }
        if result.paste {
            self.clipboard_read(id, env);
        }
        self.send_scene_events(id, gesture, env);
        if picked {
            self.close_chain(id, CloseReason::Picked, env);
        }
        if result.dismiss {
            self.close(id, CloseReason::Escape, env);
        }
    }

    fn clipboard_write(&mut self, text: String, env: &mut Env<'_>) {
        if self.o.gated {
            // A test run never changes the user's clipboard.
            self.stats.emit("clipboard", json!({"op": "write", "ok": true, "ms": 0, "stand_in": true}));
            return;
        }
        env.ui.clipboard_write(text);
        self.stats.emit("clipboard", json!({"op": "write", "ok": true, "ms": 0}));
    }

    fn clipboard_read(&mut self, id: u64, env: &mut Env<'_>) {
        if self.o.gated {
            if let Some(i) = self.idx(id) {
                self.managed[i].scene.paste(FAKE_CLIPBOARD, env.now);
            }
            self.stats.emit("clipboard", json!({"op": "read", "ok": true, "ms": 0, "stand_in": true}));
            self.send_scene_events(id, 0, env);
            return;
        }
        let token = self.next_token;
        self.next_token += 1;
        self.reads.insert(token, PasteRead { popup: id, started: env.now });
        env.ui.clipboard_read(token);
    }

    /// The clipboard worker answered (or ran out of time: `None`).
    pub fn on_clipboard(&mut self, env: &mut Env<'_>, token: u64, text: Option<String>) {
        let Some(read) = self.reads.remove(&token) else { return };
        let ms = env.now.saturating_duration_since(read.started).as_millis() as u64;
        self.stats.emit("clipboard", json!({"op": "read", "ok": text.is_some(), "ms": ms}));
        let (Some(text), Some(i)) = (text, self.idx(read.popup)) else { return };
        if self.managed[i].stage == Stage::Closing {
            return;
        }
        self.managed[i].scene.paste(&text, env.now);
        self.send_scene_events(read.popup, 0, env);
    }

    /// A popup window got or lost keyboard focus.
    pub fn on_focus(&mut self, env: &mut Env<'_>, win: PopupId, focused: bool) {
        let Some(i) = self.managed.iter().position(|m| m.win == win) else { return };
        if self.managed[i].stage == Stage::Closing {
            return;
        }
        let id = self.managed[i].id;
        if focused {
            if !self.managed[i].holds {
                self.set_holds(i, env, None);
            }
            return;
        }
        if !self.managed[i].holds {
            return;
        }
        let now = env.now;
        let outside = {
            let m = &mut self.managed[i];
            m.holds = false;
            m.scene.set_window_focus(false, now);
            m.scene.set_caret_blink(None);
            m.props.dismiss.outside
        };
        if outside {
            self.close_chain(id, CloseReason::Deactivated, env);
        }
    }

    // ------------------------------------------------------------ the rest of the world

    /// The popup window's surface size is new.
    pub fn on_popup_geometry(&mut self, win: PopupId, geometry: BarGeometry) {
        if let Some(m) = self.managed.iter_mut().find(|m| m.win == win) {
            m.scene.set_geometry(geometry);
            m.need_full = true;
        }
    }

    /// The session was locked: every popup and overlay closes.
    pub fn on_session(&mut self, env: &mut Env<'_>, locked: bool) {
        self.stats.emit("session", json!({"locked": locked}));
        if locked {
            self.close_all(env, CloseReason::Deactivated);
        }
    }

    /// A popup's window got a DPI change: every popup on its monitor closes.
    pub fn on_popup_dpi(&mut self, env: &mut Env<'_>, win: PopupId) {
        let Some(device) =
            self.managed.iter().find(|m| m.win == win || m.dim == Some(win)).map(|m| m.monitor.device.clone())
        else {
            return;
        };
        self.close_monitor(env, &device, CloseReason::DisplayChange);
    }

    /// A fullscreen app came to a bar's monitor: its popups must not stay
    /// topmost over it, unless the bar's mode is `stay`. Our own overlay
    /// covers the monitor too and is not a fullscreen app: it stays.
    pub fn on_fullscreen(&mut self, env: &mut Env<'_>, bar: BarId, active: bool, stay: bool) {
        if active && !stay {
            self.hide_tip(env);
            self.close_where(env, CloseReason::BarHidden, |m| m.bar == bar && m.role != SurfaceRole::Overlay);
        }
    }

    /// `rsb popup <name> [action]` and `--simulate-popup`: the script is
    /// asked, inside a gesture of its own, on the monitor under the pointer.
    pub fn command(&mut self, env: &mut Env<'_>, name: &str, action: PopupAction) -> Reply {
        // Popups in bars, and popups inside open popups (a menu in a panel).
        let named = |d: &rsb_core::popup::PopupDecl| {
            d.kind != rsb_core::element::ElementKind::Tooltip && d.props.answers_to(name)
        };
        let known = env.bars.iter().any(|b| b.scene.popups().iter().any(named))
            || self.managed.iter().any(|m| m.stage != Stage::Closing && m.scene.popups().iter().any(named));
        if !known {
            return Reply::Err(format!("no popup named '{name}'"));
        }
        self.monitors = env.ui.monitors();
        let monitor = env.ui.pointer_monitor().and_then(|dev| self.monitor_of(&dev)).map(|m| m.handle);
        let gesture = self.gestures.begin(env.now, GestureKind::Command);
        self.send(ScriptIn::Popup { name: name.to_string(), action, monitor, gesture });
        Reply::Ok("ok".into())
    }
}

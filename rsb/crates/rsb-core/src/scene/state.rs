//! Interaction state that outlives a rebuild of the arena: text being
//! edited, slider values, scroll offsets, focus. Matched by node key.

use std::collections::HashSet;
use std::time::Instant;

use super::Scene;
use super::edit::EditState;
use super::slider::SliderState;
use crate::element::{ElementKind, NodeKey};
use crate::layout::TextMeasure;

impl Scene {
    /// After the arena was rebuilt: take values the script wrote, drop state of
    /// nodes that are gone, queue reveals the script asked for.
    pub(super) fn sync_states(&mut self, now: Instant) {
        // An element marked `autofocus` that appears while a popup is already
        // open (a page change inside a panel) takes focus once the element that
        // had it is gone, as it does when the popup opens.
        let mut arriving: Option<NodeKey> = None;
        let mut autofocus_now: Vec<NodeKey> = Vec::new();
        for idx in self.root_idx..self.arena.len() {
            let n = &self.arena[idx];
            if n.interaction.autofocus && n.kind != ElementKind::Input {
                autofocus_now.push(n.key);
                if arriving.is_none() && !self.autofocus_seen.contains(&n.key) {
                    arriving = Some(n.key);
                }
            }
            match n.kind {
                ElementKind::Input => {
                    if let Some(p) = n.input.as_deref() {
                        match self.edits.get_mut(&n.key) {
                            Some(e) if p.value_seq > e.seen_seq => e.replace(&p.value, p.value_seq, now),
                            Some(_) => {}
                            None => {
                                if n.interaction.autofocus && arriving.is_none() {
                                    arriving = Some(n.key);
                                }
                                self.edits.insert(n.key, EditState::new(&p.value, p.value_seq, now));
                            }
                        }
                    }
                }
                ElementKind::Slider => {
                    if let Some(p) = n.slider.as_deref() {
                        match self.sliders.get_mut(&n.key) {
                            Some(s) if p.value_seq > s.seen_seq => *s = SliderState::new(p.value, p.value_seq),
                            Some(_) => {}
                            None => {
                                self.sliders.insert(n.key, SliderState::new(p.value, p.value_seq));
                            }
                        }
                    }
                }
                _ => {}
            }
            let seq = n.interaction.reveal_seq;
            if seq > self.reveal_seen.get(&n.key).copied().unwrap_or(0) {
                self.reveal_seen.insert(n.key, seq);
                self.pending_reveals.push(n.key);
            }
        }
        if self.arena.is_empty() {
            return;
        }
        let alive: HashSet<NodeKey> = self.arena.iter().map(|n| n.key).collect();
        self.autofocus_seen = autofocus_now.into_iter().collect();
        self.edits.retain(|k, _| alive.contains(k));
        self.sliders.retain(|k, _| alive.contains(k));
        self.scrolls.retain(|k, _| alive.contains(k));
        self.reveal_seen.retain(|k, _| alive.contains(k));
        self.pending_reveals.retain(|k| alive.contains(k));
        let keyboard = self.focus.visible;
        if self.focus.node.is_some_and(|k| !alive.contains(&k)) {
            self.focus.node = None;
            self.focus.visible = false;
        }
        if let Some(i) = arriving.and_then(|k| self.idx_of(k))
            && self.popup.is_some()
            && self.focus.node.is_none()
            && self.is_focusable(i)
        {
            self.set_focus(Some(i), keyboard, now);
            // The styles of this pass were matched before the element had focus.
            self.restyle_again = true;
        }
        let gone = match &self.drag {
            Some(super::Drag::Input(k) | super::Drag::Slider(k)) => !alive.contains(k),
            Some(super::Drag::Thumb { key, .. }) => !alive.contains(key),
            None => false,
        };
        if gone {
            self.drag = None;
        }
    }

    /// What moves on its own between inputs: smooth scrolling, held back
    /// reports, the caret. Runs after every layout, before the draw list.
    pub(super) fn tick(&mut self, now: Instant, text: &mut dyn TextMeasure) {
        self.update_scroll_extents();
        for k in std::mem::take(&mut self.pending_reveals) {
            if let Some(i) = self.idx_of(k) {
                self.reveal(i, now);
            }
        }
        self.scroll_tick(now);
        self.update_transforms();
        self.prepare_inputs(text);
        self.flush_reports(now);
        self.flush_sliders(now);
        let (busy, scroll_wake) = self.scroll_wake(now);
        self.dyn_busy = busy;
        self.wake = [scroll_wake, self.edit_wake(now), self.slider_wake()].into_iter().flatten().min();
        if let Some(p) = &mut self.popup
            && p.open
            && !self.dyn_busy
            && self.running.is_empty()
            && !self.anim_busy
            && self.size_runs.is_empty()
        {
            p.settled_seen = true;
        }
    }
}

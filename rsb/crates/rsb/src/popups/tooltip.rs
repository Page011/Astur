//! Tooltips. One surface at a time, never activated, click-through. After a
//! pointer event the manager asks the scene under the pointer which tooltip
//! it hovers; a new one shows after `TOOLTIP_DELAY`, or at once when another
//! one hid less than `TOOLTIP_RESHOW` ago. A button, the wheel, a popup
//! opening, or the pointer moving to something else hides it.

use std::time::Instant;

use rsb_core::element::{ElementKind, NodeKey};
use rsb_core::hit::PointerKind;
use rsb_core::popup::{CloseReason, TOOLTIP_DELAY, TOOLTIP_RESHOW};

use super::{Anchor, DeclRec, Env, Popups, push_decls, scene_of};

#[derive(Clone, Copy, Debug)]
struct Candidate {
    src: Anchor,
    key: NodeKey,
    since: Instant,
    /// Already shown once for this hover: leaving and coming back starts over.
    done: bool,
}

#[derive(Debug, Default)]
pub(super) struct Tips {
    cand: Option<Candidate>,
    pub shown: Option<u64>,
    pub hid_at: Option<Instant>,
}

impl Tips {
    /// When the next tooltip is due to show.
    pub fn deadline(&self) -> Option<Instant> {
        self.cand.filter(|c| !c.done && self.shown.is_none()).map(|c| c.since + TOOLTIP_DELAY)
    }
}

/// Whether a tooltip that hid at `hid_at` lets the next one show at once.
fn reshow_at_once(hid_at: Option<Instant>, now: Instant) -> bool {
    hid_at.is_some_and(|h| now.saturating_duration_since(h) < TOOLTIP_RESHOW)
}

/// Whether a candidate that began at `since` is due.
fn due(since: Instant, now: Instant) -> bool {
    now.saturating_duration_since(since) >= TOOLTIP_DELAY
}

impl Popups {
    /// After a pointer event on a bar or a popup surface.
    pub(super) fn tip_pointer(&mut self, env: &mut Env<'_>, src: Anchor, kind: PointerKind) {
        if matches!(kind, PointerKind::Down(_) | PointerKind::Wheel(_) | PointerKind::HWheel(_)) {
            // It stays hidden until the pointer moves to something else, as the system's own tooltips do.
            self.hide_tip(env);
            if let Some(c) = self.tips.cand.as_mut() {
                c.done = true;
            }
            return;
        }
        let hovered = scene_of(&mut self.managed, env.bars, src).and_then(|s| s.hovered_tooltip());
        let Some(key) = hovered else {
            self.hide_tip(env);
            self.tips.cand = None;
            return;
        };
        if self.tips.cand.is_some_and(|c| c.src == src && c.key == key) {
            return;
        }
        self.hide_tip(env);
        self.tips.cand = Some(Candidate { src, key, since: env.now, done: false });
        if reshow_at_once(self.tips.hid_at, env.now) {
            self.show_tip(env);
        }
    }

    /// The delay passed: show the tooltip being waited for.
    pub(super) fn tip_tick(&mut self, env: &mut Env<'_>) {
        if let Some(c) = self.tips.cand
            && !c.done
            && self.tips.shown.is_none()
            && due(c.since, env.now)
        {
            self.show_tip(env);
        }
    }

    /// Hides the tooltip now (its window goes with its closing frame).
    pub(super) fn hide_tip(&mut self, env: &mut Env<'_>) {
        if let Some(id) = self.tips.shown.take() {
            self.tips.hid_at = Some(env.now);
            self.close(id, CloseReason::Command, env);
        }
    }

    fn show_tip(&mut self, env: &mut Env<'_>) {
        let Some(c) = self.tips.cand else { return };
        self.monitors = env.ui.monitors();
        let Some(r) = self.tooltip_decl(env, c) else { return };
        if self.create(&r, 0, false, env).is_ok() {
            self.tips.shown = self.managed.last().map(|m| m.id);
            if let Some(c) = self.tips.cand.as_mut() {
                c.done = true;
            }
        } else {
            log::warn!("a tooltip could not be shown");
        }
    }

    /// The declaration of the candidate tooltip, from the scene it hangs in.
    fn tooltip_decl(&mut self, env: &mut Env<'_>, c: Candidate) -> Option<DeclRec> {
        let (bar, origin, device) = match c.src {
            Anchor::Bar(id) => {
                let b = env.bars.iter().find(|b| b.id == id)?;
                (id, self.bar_origin(env, b)?, b.plan.monitor.device.clone())
            }
            Anchor::Popup(id) => {
                let m = self.managed.iter().find(|m| m.id == id)?;
                (m.bar, (m.rect.x0, m.rect.y0), m.monitor.device.clone())
            }
        };
        let decls = scene_of(&mut self.managed, env.bars, c.src)?.popups();
        let mut out = Vec::new();
        push_decls(&mut out, decls, c.src, bar, origin, &device);
        out.into_iter().find(|r| r.d.key == c.key && r.d.kind == ElementKind::Tooltip)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_tooltip_shows_after_the_delay_or_at_once_after_another_hid() {
        let t0 = Instant::now();
        assert!(!due(t0, t0 + Duration::from_millis(499)));
        assert!(due(t0, t0 + TOOLTIP_DELAY));
        assert!(reshow_at_once(Some(t0), t0 + Duration::from_millis(299)));
        assert!(!reshow_at_once(Some(t0), t0 + Duration::from_millis(300)));
        assert!(!reshow_at_once(None, t0));
    }

    #[test]
    fn the_deadline_is_the_delay_after_the_hover_began() {
        let t0 = Instant::now();
        let mut tips = Tips::default();
        assert_eq!(tips.deadline(), None);
        tips.cand = Some(Candidate { src: Anchor::Popup(1), key: NodeKey(7), since: t0, done: false });
        assert_eq!(tips.deadline(), Some(t0 + TOOLTIP_DELAY));
        tips.shown = Some(3);
        assert_eq!(tips.deadline(), None, "one is showing");
        tips.shown = None;
        if let Some(c) = tips.cand.as_mut() {
            c.done = true;
        }
        assert_eq!(tips.deadline(), None, "already shown for this hover");
    }
}

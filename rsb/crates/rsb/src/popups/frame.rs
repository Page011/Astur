//! Drawing one popup surface: the scene's frame, the dim of an overlay, the
//! present, the hit region the outside-press watch uses, and the settle
//! bookkeeping.

use std::time::Instant;

use rsb_win::{HitRegion, SurfaceRole};
use serde_json::json;

use super::{Env, Popups, SLOW_STEP, SlowStep, Stage};

/// The step to remember for `took` on `widget`: one of `SLOW_STEP` or more that beats `current`.
fn slower(current: Option<&SlowStep>, widget: &str, took: std::time::Duration, at: Instant) -> Option<SlowStep> {
    (took >= SLOW_STEP && current.is_none_or(|s| took > s.took)).then(|| SlowStep {
        label: format!("widget {widget}"),
        took,
        at,
    })
}

impl Popups {
    /// Draws the popup surface `id` now and presents what changed. A draw
    /// that took a long time is remembered, so a slow loop turn can say what
    /// it was busy with.
    pub(super) fn frame_one(&mut self, id: u64, env: &mut Env<'_>) {
        let started = Instant::now();
        self.frame_one_inner(id, env);
        self.note_slow(id, started);
    }

    /// Remembers the work on popup `id` since `started` when it was long and the longest so far. The
    /// redraw after a move or resize goes through here too, so a stall there is not nameless.
    pub(super) fn note_slow(&mut self, id: u64, started: Instant) {
        let took = started.elapsed();
        if let Some(i) = self.idx(id)
            && let Some(step) = slower(self.slow_step.as_ref(), &self.managed[i].tree.widget.to_string(), took, started)
        {
            self.slow_step = Some(step);
        }
    }

    fn frame_one_inner(&mut self, id: u64, env: &mut Env<'_>) {
        let Some(i) = self.idx(id) else { return };
        let now = env.now;
        let started = Instant::now();
        let m = &mut self.managed[i];
        let built = Instant::now();
        let out = m.scene.frame(now, env.renderer);
        let build_us = built.elapsed().as_micros() as u64;
        m.next_frame = out.next_frame;
        m.frames += 1;
        if m.stage == Stage::Closing {
            m.closing_frames += 1;
        }
        let diags = m.scene.take_diagnostics();
        if out.cursor != m.cursor {
            m.cursor = out.cursor;
            env.ui.set_popup_cursor(m.win, out.cursor);
        }
        // The dim goes first: it shows its window on the first call, and the content must land above it.
        if let Some(dim) = m.dim
            && let Some(color) = m.scene.overlay_dim()
        {
            env.ui.set_dim(dim, color);
        }
        let region = if m.need_full { None } else { out.dirty };
        let mut first_present = false;
        let mut presented = None;
        if region.is_some() || m.need_full {
            env.icons.ensure(env.renderer, &out.draw);
            let draw = &*out.draw;
            let size = (draw.width, draw.height);
            if !m.sizes.contains(&size) {
                // The renderer keeps only the last few sizes anyway.
                if m.sizes.len() >= 8 {
                    m.sizes.remove(0);
                }
                m.sizes.push(size);
            }
            let renderer = &mut *env.renderer;
            let mut failure: Option<String> = None;
            let result = env.ui.present_popup(m.win, &mut |px| match renderer.render(draw, px, region) {
                Ok(rect) => {
                    presented = rect;
                    rect
                }
                Err(e) => {
                    failure = Some(e.to_string());
                    None
                }
            });
            if let Err(e) = result {
                failure = Some(e.to_string());
            }
            match failure {
                None => {
                    m.need_full = false;
                    if renderer.svg_pending() {
                        // a user's picture waits for a frame with time to spare
                        m.need_full = true;
                        m.next_frame = Some(now);
                    }
                    if !m.presented && presented.is_some() {
                        m.presented = true;
                        first_present = true;
                    }
                }
                Some(why) => {
                    log::warn!("could not draw a popup: {why}");
                    m.need_full = true;
                }
            }
        }
        // What counts as inside, for a press that reaches no window of ours.
        let outline = m.scene.root_outline();
        let same = outline.len() == m.region.len() && outline.iter().zip(&m.region).all(|(p, q)| (p.x, p.y) == *q);
        if !same {
            let points: Vec<(f32, f32)> = outline.iter().map(|p| (p.x, p.y)).collect();
            env.ui.set_popup_region(m.win, (!points.is_empty()).then(|| HitRegion { points: points.clone() }));
            m.region = points;
        }
        let settled = m.scene.is_settled();
        if settled && !m.settled_once && m.stage != Stage::Closing && m.frames >= 2 {
            m.settled_once = true;
            m.settle_ms = Some(now.saturating_duration_since(m.opened_at).as_millis() as u64);
            if m.stage == Stage::Opening {
                m.stage = Stage::Open;
            }
        }
        let (name, rect, side, gesture, role, bar_scale) = (m.name.clone(), m.rect, m.side, m.gesture, m.role, m.scale);
        // The root's own box inside the window (the window also holds its shadow), screen px.
        let drawn = (first_present && !m.region.is_empty()).then(|| {
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for &(x, y) in &m.region {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
            let at = |v: f32, origin: i32| origin + v.round() as i32;
            [at(x0, rect.x0), at(y0, rect.y0), at(x1, rect.x0), at(y1, rect.y0)]
        });
        let stat_frames = self.stats.frames;
        if presented.is_some() {
            self.stats.note_present();
        }
        if first_present {
            // Real time since the click, not the turn's clock: what the person waited for.
            let ms = self
                .gestures
                .at(gesture)
                .map(|t| crate::clock::now().saturating_duration_since(t).as_secs_f64() * 1000.0);
            self.stats.emit(
                "popup",
                json!({"name": name, "state": "shown", "click_to_first_frame_ms": ms,
                       "rect": [rect.x0, rect.y0, rect.x1, rect.y1], "side": side.class(),
                       "box": drawn,
                       "scale": bar_scale, "overlay": role == SurfaceRole::Overlay}),
            );
        }
        if stat_frames && let Some(r) = presented {
            self.stats.emit(
                "frame",
                json!({"surface": format!("popup:{name}"), "dirty": [r.x0, r.y0, r.x1, r.y1],
                       "render_us": started.elapsed().as_micros() as u64,
                       "build_us": build_us, "style_us": out.stats.style_us, "layout_us": out.stats.layout_us,
                       "paint_us": out.stats.draw_us,
                       "compositor_frame": env.ui.compositor_frame()}),
            );
        }
        self.diags.extend(diags);
        // Held-back reports (typing, dragging, scrolling) are queued by the frame.
        self.send_scene_events(id, 0, env);
        if let Some(i) = self.idx(id)
            && self.managed[i].holds
        {
            self.ime_after_frame(i, env);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn only_a_long_step_that_beats_the_one_held_is_remembered() {
        let now = Instant::now();
        let ms = Duration::from_millis;
        assert!(slower(None, "w", ms(100), now).is_none(), "under the threshold");
        let first = slower(None, "w", ms(300), now).expect("a long step");
        assert_eq!(first.label, "widget w");
        assert!(slower(Some(&first), "x", ms(280), now).is_none(), "shorter than the one held");
        assert_eq!(slower(Some(&first), "x", ms(900), now).map(|s| s.label).as_deref(), Some("widget x"));
    }
}

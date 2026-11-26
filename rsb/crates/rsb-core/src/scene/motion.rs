//! Keyframe animations and size transitions: time in, values out.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::trans;
use crate::anim::{Easing, interpolate};
use crate::style::{AnimDirection, AnimFill, AnimationSpec, ComputedStyle, KeyframeSet};

/// One running animation of a node, kept while the node keeps the name.
#[derive(Clone, Debug)]
pub(super) struct AnimRun {
    pub start: Instant,
    /// `AnimateSpec::restart_seq` it started under.
    pub seq: u64,
    pub paused_since: Option<Instant>,
}

pub(super) type AnimRuns = Vec<(Arc<str>, AnimRun)>;

/// Keeps the runs whose names the node still has, starts runs for new
/// names at `start`, and starts a name again when `seq` grew.
pub(super) fn reconcile(
    mut old: AnimRuns,
    specs: &[AnimationSpec],
    seq: u64,
    start: Instant,
    now: Instant,
) -> AnimRuns {
    let mut out: AnimRuns = Vec::new();
    for spec in specs.iter().filter(|s| s.frames.is_some()) {
        if out.iter().any(|(n, _)| *n == spec.name) {
            continue;
        }
        let mut run = match old.iter().position(|(n, _)| *n == spec.name) {
            Some(i) => {
                let (_, mut r) = old.remove(i);
                if seq > r.seq {
                    r = AnimRun { start: now, seq, paused_since: None };
                }
                r
            }
            None => AnimRun { start, seq, paused_since: None },
        };
        match (spec.paused, run.paused_since) {
            (true, None) => run.paused_since = Some(now),
            (false, Some(since)) => {
                run.start += now.saturating_duration_since(since);
                run.paused_since = None;
            }
            _ => {}
        }
        out.push((spec.name.clone(), run));
    }
    out
}

/// What an animation does at `elapsed` seconds after its delay ran out
/// (negative before it): the directed progress through one iteration, and
/// whether it still needs frames.
pub(crate) fn progress(spec: &AnimationSpec, elapsed: f64) -> (Option<f32>, bool) {
    let dur = spec.duration.as_secs_f64();
    let iters = spec.iterations;
    let directed = |n: f64, t: f64| -> f32 {
        let odd = (n as u64) % 2 == 1;
        let back = match spec.direction {
            AnimDirection::Normal => false,
            AnimDirection::Reverse => true,
            AnimDirection::Alternate => odd,
            AnimDirection::AlternateReverse => !odd,
        };
        (if back { 1.0 - t } else { t }) as f32
    };
    let filled_back = matches!(spec.fill, AnimFill::Backwards | AnimFill::Both);
    let filled_fwd = matches!(spec.fill, AnimFill::Forwards | AnimFill::Both);
    if elapsed < 0.0 {
        return (filled_back.then(|| directed(0.0, 0.0)), true);
    }
    let total = dur * iters;
    if dur <= 0.0 || iters <= 0.0 || (total.is_finite() && elapsed >= total) {
        let end = (filled_fwd && dur >= 0.0 && iters > 0.0).then(|| {
            let frac = iters.fract();
            let (last, t) = if frac == 0.0 { (iters - 1.0, 1.0) } else { (iters.floor(), frac) };
            directed(last, t)
        });
        return (end, false);
    }
    let pos = elapsed / dur;
    (Some(directed(pos.floor(), pos.fract())), true)
}

/// The style values of `frames` at directed progress `t`.
fn sample(frames: &KeyframeSet, spec_easing: Easing, t: f32, slot: trans::Slot) -> Option<crate::anim::AnimValue> {
    let f = &frames.frames;
    let t = t.clamp(0.0, 1.0);
    let i = f.iter().rposition(|k| k.offset <= t).unwrap_or(0).min(f.len().saturating_sub(2));
    let (a, b) = (&f[i], &f[i + 1]);
    let span = (b.offset - a.offset).max(1e-6);
    let u = ((t - a.offset) / span).clamp(0.0, 1.0);
    let eased = a.easing.unwrap_or(spec_easing).apply(u);
    let (va, vb) = (trans::get(&a.style, slot)?, trans::get(&b.style, slot)?);
    Some(interpolate(&va, &vb, eased))
}

/// Lays every active (or filled) animation of the node over `display`.
/// True while one still needs frames.
pub(super) fn apply(display: &mut ComputedStyle, specs: &[AnimationSpec], runs: &AnimRuns, now: Instant) -> bool {
    let mut busy = false;
    for spec in specs {
        let (Some(frames), Some((_, run))) = (&spec.frames, runs.iter().find(|(n, _)| *n == spec.name)) else {
            continue;
        };
        let at = run.paused_since.unwrap_or(now);
        let elapsed = at.saturating_duration_since(run.start).as_secs_f64() - spec.delay_s;
        let (t, active) = progress(spec, elapsed);
        busy |= active && run.paused_since.is_none();
        let Some(t) = t else { continue };
        for slot in trans::ALL {
            let vals: Vec<_> = frames.frames.iter().map(|k| trans::get(&k.style, slot)).collect();
            if vals.iter().all(|v| *v == vals[0]) {
                continue;
            }
            if let Some(v) = sample(frames, spec.easing, t, slot) {
                trans::set(display, slot, &v);
            }
        }
    }
    busy
}

/// A running width or height change, in physical px.
#[derive(Clone, Copy, Debug)]
pub(super) struct SizeRun {
    pub from: f32,
    pub to: f32,
    pub start: Instant,
    pub delay: Duration,
    pub duration: Duration,
    pub easing: Easing,
}

impl SizeRun {
    /// The size at `now`, None once it has arrived.
    pub fn at(&self, now: Instant) -> Option<f32> {
        let elapsed = now.saturating_duration_since(self.start);
        if elapsed < self.delay {
            return Some(self.from);
        }
        let t = if self.duration.is_zero() {
            1.0
        } else {
            (elapsed - self.delay).as_secs_f32() / self.duration.as_secs_f32()
        };
        if t >= 1.0 {
            return None;
        }
        Some(self.from + (self.to - self.from) * self.easing.apply(t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(dur_ms: u64, iters: f64, dir: AnimDirection, fill: AnimFill, delay: f64) -> AnimationSpec {
        AnimationSpec {
            name: Arc::from("a"),
            duration: Duration::from_millis(dur_ms),
            delay_s: delay,
            easing: Easing::Linear,
            iterations: iters,
            direction: dir,
            fill,
            paused: false,
            frames: None,
        }
    }

    #[test]
    fn fill_modes_before_and_after() {
        let none = spec(100, 1.0, AnimDirection::Normal, AnimFill::None, 0.5);
        assert_eq!(progress(&none, -0.1), (None, true));
        assert_eq!(progress(&none, 0.2), (None, false));
        let both = spec(100, 1.0, AnimDirection::Normal, AnimFill::Both, 0.5);
        assert_eq!(progress(&both, -0.1), (Some(0.0), true));
        assert_eq!(progress(&both, 0.2), (Some(1.0), false));
        let back = spec(100, 1.0, AnimDirection::Normal, AnimFill::Backwards, 0.5);
        assert_eq!(progress(&back, 0.2), (None, false));
    }

    #[test]
    fn iterations_and_direction() {
        let alt = spec(100, 3.0, AnimDirection::Alternate, AnimFill::Forwards, 0.0);
        assert_eq!(progress(&alt, 0.05), (Some(0.5), true));
        assert_eq!(progress(&alt, 0.125), (Some(0.75), true), "second pass runs backwards");
        assert_eq!(progress(&alt, 0.225), (Some(0.25), true));
        assert_eq!(progress(&alt, 0.4), (Some(1.0), false), "third pass ends at its last frame");
        let rev = spec(100, 1.0, AnimDirection::Reverse, AnimFill::Both, 0.0);
        assert_eq!(progress(&rev, 0.025), (Some(0.75), true));
        let half = spec(100, 1.5, AnimDirection::Normal, AnimFill::Forwards, 0.0);
        assert_eq!(progress(&half, 0.2), (Some(0.5), false), "a half iteration stops half way");
        let inf = spec(100, f64::INFINITY, AnimDirection::Normal, AnimFill::None, 0.0);
        let (t, busy) = progress(&inf, 12.34);
        assert!(busy && (t.unwrap() - 0.4).abs() < 1e-4, "{t:?}");
    }

    #[test]
    fn a_negative_delay_starts_part_way_in() {
        // elapsed already includes the delay: a -50 ms delay at time 0 is 50 ms in.
        let s = spec(100, 1.0, AnimDirection::Normal, AnimFill::None, -0.05);
        let elapsed = 0.0 - s.delay_s;
        assert_eq!(progress(&s, elapsed), (Some(0.5), true));
    }

    #[test]
    fn a_size_run_eases_and_ends() {
        let t0 = Instant::now();
        let run = SizeRun {
            from: 77.0,
            to: 262.0,
            start: t0,
            delay: Duration::ZERO,
            duration: Duration::from_millis(200),
            easing: Easing::Linear,
        };
        assert_eq!(run.at(t0), Some(77.0));
        assert_eq!(run.at(t0 + Duration::from_millis(100)), Some(169.5));
        assert_eq!(run.at(t0 + Duration::from_millis(200)), None);
    }
}

//! User gestures: a click, a key press or an `rsb popup` command. Each gets
//! a new non-zero id that travels with the handler call it causes, so the
//! script can tell "the user did this" from a timer, and the UI can tell how
//! old the input is when a popup asks for keyboard focus. Id 0 is never a
//! gesture. The last few are kept with the time they happened.

use std::time::{Duration, Instant};

use rsb_core::popup::ACTIVATE_WITHIN;

/// How many recent gestures are remembered.
const KEPT: usize = 16;

/// What produced a gesture; a popup opened from the keyboard shows its
/// focus ring, one opened with the pointer does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GestureKind {
    Pointer,
    Key,
    Command,
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    id: u64,
    at: Instant,
    kind: GestureKind,
}

#[derive(Debug)]
pub struct Gestures {
    next: u64,
    ring: Vec<Entry>,
}

impl Default for Gestures {
    fn default() -> Self {
        Gestures { next: 1, ring: Vec::with_capacity(KEPT) }
    }
}

impl Gestures {
    /// A new gesture happening at `at`. The id is never 0.
    pub fn begin(&mut self, at: Instant, kind: GestureKind) -> u64 {
        let id = self.next;
        self.next = self.next.checked_add(1).unwrap_or(1);
        if self.ring.len() == KEPT {
            self.ring.remove(0);
        }
        self.ring.push(Entry { id, at, kind });
        id
    }

    /// The input completed later than it began (a click counts from its
    /// pointer-up): its age is measured from `at` from now on.
    pub fn touch(&mut self, id: u64, at: Instant) {
        if let Some(e) = self.ring.iter_mut().find(|e| e.id == id) {
            e.at = at;
        }
    }

    /// How long ago the gesture happened; None for id 0 or one that is no
    /// longer among the last 16.
    pub fn age(&self, id: u64, now: Instant) -> Option<Duration> {
        if id == 0 {
            return None;
        }
        self.ring.iter().find(|e| e.id == id).map(|e| now.saturating_duration_since(e.at))
    }

    /// The age, when the gesture is recent enough to give a popup focus.
    pub fn fresh(&self, id: u64, now: Instant) -> Option<Duration> {
        self.age(id, now).filter(|a| *a <= ACTIVATE_WITHIN)
    }

    pub fn kind(&self, id: u64) -> Option<GestureKind> {
        self.ring.iter().find(|e| e.id == id).map(|e| e.kind)
    }

    /// When the gesture happened, for the click-to-first-frame number.
    pub fn at(&self, id: u64) -> Option<Instant> {
        self.ring.iter().find(|e| e.id == id).map(|e| e.at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_new_and_never_zero() {
        let mut g = Gestures::default();
        let t = Instant::now();
        let a = g.begin(t, GestureKind::Pointer);
        let b = g.begin(t, GestureKind::Key);
        assert!(a != 0 && b != 0 && a != b);
        assert_eq!(g.kind(a), Some(GestureKind::Pointer));
        assert_eq!(g.kind(b), Some(GestureKind::Key));
        assert_eq!(g.age(0, t), None, "id 0 is not a gesture");
    }

    #[test]
    fn only_the_last_sixteen_are_kept() {
        let mut g = Gestures::default();
        let t = Instant::now();
        let first = g.begin(t, GestureKind::Pointer);
        let mut last = first;
        for _ in 0..KEPT {
            last = g.begin(t, GestureKind::Pointer);
        }
        assert_eq!(g.age(first, t), None, "the oldest was forgotten");
        assert!(g.age(last, t).is_some());
        assert_eq!(g.ring.len(), KEPT);
    }

    #[test]
    fn a_gesture_is_fresh_for_one_second_and_a_click_counts_from_its_release() {
        let mut g = Gestures::default();
        let down = Instant::now();
        let id = g.begin(down, GestureKind::Pointer);
        let up = down + Duration::from_millis(900);
        g.touch(id, up);
        assert_eq!(g.fresh(id, up + Duration::from_millis(999)), Some(Duration::from_millis(999)));
        assert_eq!(g.fresh(id, up + Duration::from_millis(1001)), None);
        assert_eq!(g.fresh(0, up), None);
        assert_eq!(g.fresh(id + 100, up), None, "an id nobody gave is not fresh");
    }
}

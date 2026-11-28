//! Single-line text editing for `input` elements: the edit state kept per
//! node, key and IME handling, the caret and selection geometry, painting.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{Drag, Scene, text_style};
use crate::color::Color;
use crate::draw::{DrawCmd, DrawItem, Fill, TextStyle};
use crate::element::NodeKey;
use crate::geom::{PointF, Radii, RectF};
use crate::hit::{ElementCall, ElementEvent};
use crate::key::{Key, KeyEvent, TextInput};
use crate::layout::TextMeasure;
use crate::scene::KeyResult;
use crate::style::TextAlign;

/// Characters an input holds when it sets no `max_length`.
pub(super) const TEXT_CAP: usize = 4096;
/// `on_change` runs at most this often while typing.
const REPORT_GAP: Duration = Duration::from_millis(50);
/// Clicks this close together in time count as a double or triple click.
const MULTI_CLICK: Duration = Duration::from_millis(500);
/// Logical px the caret and text keep from the edge when scrolling.
const SCROLL_SPARE: f32 = 2.0;
const BULLET: char = '\u{2022}';

#[derive(Clone)]
struct Snap {
    text: String,
    caret: usize,
    anchor: usize,
}

/// What kind of edit an undo group collects.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    Typing,
    Delete,
    Other,
}

/// What `edit_key` did with a key.
pub(super) enum Used {
    No,
    Yes,
    /// Used, and the script's `on_key` hears it too (Enter).
    AndKey,
}

/// The text of one input and everything about editing it.
pub(super) struct EditState {
    pub text: String,
    pub caret: usize,
    pub anchor: usize,
    /// The text being composed and the cursor in it (chars).
    pub comp: Option<(String, usize)>,
    pub scroll_x: f32,
    pub seen_seq: u64,
    undo: Option<Snap>,
    group: Option<Group>,
    last_report: Option<Instant>,
    pending: bool,
    /// The last edit, move or focus: the caret blinks from here.
    pub epoch: Instant,
    /// Caret stops of the displayed text and the style they were measured for.
    stops: Vec<(usize, f32)>,
    stops_for: Option<(String, TextStyle)>,
    /// Width of the placeholder while it is the thing shown (empty text), else 0.
    ph_w: f32,
    line_h: f32,
    last_click: Option<(Instant, PointF)>,
    clicks: u8,
}

impl EditState {
    pub fn new(text: &str, seq: u64, now: Instant) -> EditState {
        EditState {
            text: text.to_string(),
            caret: text.len(),
            anchor: text.len(),
            comp: None,
            scroll_x: 0.0,
            seen_seq: seq,
            undo: None,
            group: None,
            last_report: None,
            pending: false,
            epoch: now,
            stops: Vec::new(),
            stops_for: None,
            ph_w: 0.0,
            line_h: 0.0,
            last_click: None,
            clicks: 0,
        }
    }

    /// The script wrote a new value: it replaces the text, caret at the end.
    pub fn replace(&mut self, text: &str, seq: u64, now: Instant) {
        self.text = text.to_string();
        self.caret = self.text.len();
        self.anchor = self.caret;
        self.seen_seq = seq;
        self.comp = None;
        self.undo = None;
        self.group = None;
        self.pending = false;
        self.epoch = now;
    }

    fn selection(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    fn has_selection(&self) -> bool {
        self.caret != self.anchor
    }

    fn selected(&self) -> &str {
        let (a, b) = self.selection();
        &self.text[a..b]
    }

    /// Starts an undo group unless the last edit was of the same kind.
    fn begin(&mut self, g: Group) {
        if self.group != Some(g) || self.undo.is_none() {
            self.undo = Some(Snap { text: self.text.clone(), caret: self.caret, anchor: self.anchor });
        }
        self.group = Some(g);
    }

    fn delete_selection(&mut self) {
        let (a, b) = self.selection();
        self.text.replace_range(a..b, "");
        self.caret = a;
        self.anchor = a;
    }

    /// Inserts at the caret in place of the selection; whatever does not fit
    /// under `cap` characters is dropped.
    fn insert(&mut self, s: &str, cap: usize) {
        let kept = self.text.chars().count() - self.selected().chars().count();
        let room = cap.saturating_sub(kept);
        let s: String = s.chars().filter(|c| !c.is_control()).take(room).collect();
        self.delete_selection();
        self.text.insert_str(self.caret, &s);
        self.caret += s.len();
        self.anchor = self.caret;
    }

    fn move_to(&mut self, to: usize, extend: bool) {
        self.caret = to;
        if !extend {
            self.anchor = to;
        }
        self.group = None;
    }

    /// The caret is drawn: always without blinking, else on in the first and every other period since the last edit.
    fn caret_visible(&self, period: Option<Duration>, now: Instant) -> bool {
        let Some(p) = period.filter(|p| !p.is_zero()) else { return true };
        (now.saturating_duration_since(self.epoch).as_micros() / p.as_micros()).is_multiple_of(2)
    }
}

/// The text as drawn: the composition spliced in at the caret, bullets for a password.
struct Disp {
    s: String,
    caret: usize,
    comp: Option<(usize, usize)>,
}

impl EditState {
    fn display(&self, password: bool) -> Disp {
        let mask = |t: &str| -> String { if password { t.chars().map(|_| BULLET).collect() } else { t.to_string() } };
        let (pre, post) = (mask(&self.text[..self.caret]), mask(&self.text[self.caret..]));
        match &self.comp {
            Some((c, cursor)) if !c.is_empty() => {
                let inside: usize = c.chars().take(*cursor).map(char::len_utf8).sum();
                Disp {
                    caret: pre.len() + inside,
                    comp: Some((pre.len(), pre.len() + c.len())),
                    s: format!("{pre}{c}{post}"),
                }
            }
            _ => Disp { caret: pre.len(), comp: None, s: format!("{pre}{post}") },
        }
    }

    /// A byte of the text as a byte of the displayed string.
    fn to_display(&self, real: usize, password: bool) -> usize {
        let base = if password { self.text[..real].chars().count() * BULLET.len_utf8() } else { real };
        match &self.comp {
            Some((c, _)) if real > self.caret => base + c.len(),
            _ => base,
        }
    }

    /// A byte of the displayed string as a byte of the text.
    fn to_real(&self, shown: usize, password: bool) -> usize {
        if !password {
            return shown.min(self.text.len());
        }
        let n = shown / BULLET.len_utf8();
        self.text.char_indices().nth(n).map_or(self.text.len(), |(i, _)| i)
    }
}

/// X offset of a byte of the displayed text: the nearest stop at or before it.
fn x_at(stops: &[(usize, f32)], byte: usize) -> f32 {
    stops.iter().take_while(|(b, _)| *b <= byte).last().map_or(0.0, |(_, x)| *x)
}

// ---- clusters and words ----

/// Combining marks, joiners, variation selectors and emoji modifiers: they
/// stay with the character before them.
fn is_mark(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x064B..=0x065F | 0x0E31 | 0x0E34..=0x0E3A
        | 0x0E47..=0x0E4E | 0x093C | 0x0941..=0x0948 | 0x094D | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF
        | 0x200C | 0x200D | 0x20D0..=0x20FF | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F)
}

fn is_regional(c: char) -> bool {
    matches!(c as u32, 0x1F1E6..=0x1F1FF)
}

/// The caret stop after the one at `i` (a byte index on a char boundary).
pub(super) fn next_boundary(text: &str, i: usize) -> usize {
    let mut it = text[i..].char_indices().peekable();
    let Some((_, first)) = it.next() else { return text.len() };
    let mut end = i + first.len_utf8();
    if is_regional(first)
        && let Some(&(_, n)) = it.peek()
        && is_regional(n)
    {
        return end + n.len_utf8();
    }
    while let Some(&(off, c)) = it.peek() {
        if !is_mark(c) {
            break;
        }
        it.next();
        end = i + off + c.len_utf8();
        // A joiner takes the character after it too.
        if c == '\u{200D}'
            && let Some((o2, c2)) = it.next()
        {
            end = i + o2 + c2.len_utf8();
        }
    }
    end
}

/// The caret stop before the one at `i`.
pub(super) fn prev_boundary(text: &str, i: usize) -> usize {
    let mut chars = text[..i].char_indices().rev().peekable();
    let Some((mut start, mut c)) = chars.next() else { return 0 };
    loop {
        if is_mark(c) {
            match chars.next() {
                Some((s, p)) => {
                    start = s;
                    c = p;
                    continue;
                }
                None => return 0,
            }
        }
        // A character joined to the one before it by a zero width joiner.
        if let Some(&(s, p)) = chars.peek()
            && p == '\u{200D}'
        {
            chars.next();
            if let Some((s2, p2)) = chars.next() {
                start = s2;
                c = p2;
                continue;
            }
            start = s;
        }
        break;
    }
    if is_regional(c) {
        let before = text[..start].chars().rev().take_while(|&r| is_regional(r)).count();
        if before % 2 == 1 {
            start -= text[..start].chars().next_back().map_or(0, char::len_utf8);
        }
    }
    start
}

fn word_class(c: char) -> u8 {
    if c.is_alphanumeric() || c == '_' {
        0
    } else if c.is_whitespace() {
        1
    } else {
        2
    }
}

/// Start of the word before `i`: spaces are skipped, then one run of one kind.
pub(super) fn prev_word(text: &str, i: usize) -> usize {
    let mut j = i;
    while let Some(c) = text[..j].chars().next_back() {
        if word_class(c) != 1 {
            break;
        }
        j -= c.len_utf8();
    }
    let Some(first) = text[..j].chars().next_back() else { return 0 };
    let class = word_class(first);
    while let Some(c) = text[..j].chars().next_back() {
        if word_class(c) != class {
            break;
        }
        j -= c.len_utf8();
    }
    j
}

/// End of the word after `i`.
pub(super) fn next_word(text: &str, i: usize) -> usize {
    let mut j = i;
    let Some(first) = text[j..].chars().next() else { return text.len() };
    let class = word_class(first);
    if class != 1 {
        while let Some(c) = text[j..].chars().next() {
            if word_class(c) != class {
                break;
            }
            j += c.len_utf8();
        }
    }
    while let Some(c) = text[j..].chars().next() {
        if word_class(c) != 1 {
            break;
        }
        j += c.len_utf8();
    }
    j
}

/// The word (or the run of spaces or punctuation) around `i`.
fn word_range(text: &str, i: usize) -> (usize, usize) {
    let class_at = |j: usize| text[j..].chars().next().map(word_class);
    let class = class_at(i).or_else(|| text[..i].chars().next_back().map(word_class)).unwrap_or(1);
    let mut a = i;
    while let Some(c) = text[..a].chars().next_back() {
        if word_class(c) != class {
            break;
        }
        a -= c.len_utf8();
    }
    let mut b = i;
    while let Some(c) = text[b..].chars().next() {
        if word_class(c) != class {
            break;
        }
        b += c.len_utf8();
    }
    (a, b)
}

fn is_boundary(text: &str, b: usize) -> bool {
    text.is_char_boundary(b) && (b == 0 || b == text.len() || next_boundary(text, prev_boundary(text, b)) == b)
}

/// Where an input draws its text.
struct Geom {
    content: RectF,
    /// Layout x of the first character.
    origin: f32,
    line_h: f32,
    tw: f32,
}

impl Scene {
    fn input_cap(&self, idx: usize) -> usize {
        match self.arena[idx].input.as_deref().map_or(0, |p| p.max_length) {
            0 => TEXT_CAP,
            n => (n as usize).min(TEXT_CAP),
        }
    }

    fn is_password(&self, idx: usize) -> bool {
        self.arena[idx].input.as_deref().is_some_and(|p| p.password)
    }

    fn edit_of(&mut self, idx: usize) -> Option<&mut EditState> {
        let key = self.arena[idx].key;
        self.edits.get_mut(&key)
    }

    /// Where `idx`'s text starts and how much room it has.
    fn input_geom(&self, idx: usize) -> Option<(Geom, &EditState)> {
        let n = &self.arena[idx];
        let e = self.edits.get(&n.key)?;
        let (r, b, p) = (n.laid.rect, n.laid.border, n.laid.padding);
        let content = RectF::new(
            r.x + b.left + p.left,
            r.y + b.top + p.top,
            (r.w - b.left - b.right - p.left - p.right).max(0.0),
            (r.h - b.top - b.bottom - p.top - p.bottom).max(0.0),
        );
        let tw = e.stops.last().map_or(0.0, |s| s.1);
        // With no text the caret stands where the placeholder starts, not through the middle of it.
        let shown = if tw == 0.0 { e.ph_w } else { tw };
        let origin = if shown <= content.w {
            content.x
                + match n.style.text_align {
                    TextAlign::Start => 0.0,
                    TextAlign::Center => (content.w - shown) / 2.0,
                    TextAlign::End => content.w - shown,
                }
        } else {
            content.x - e.scroll_x
        };
        Some((Geom { content, origin, line_h: e.line_h, tw }, e))
    }

    /// Measures the displayed text and keeps the caret in view; after a layout.
    pub(super) fn prepare_inputs(&mut self, text: &mut dyn TextMeasure) {
        let scale = self.geometry.scale;
        for idx in self.root_idx..self.arena.len() {
            if self.arena[idx].kind != crate::element::ElementKind::Input || self.arena[idx].style.display_none {
                continue;
            }
            let password = self.is_password(idx);
            let style = text_style(&self.arena[idx].style, scale);
            let (r, b, p) = (self.arena[idx].laid.rect, self.arena[idx].laid.border, self.arena[idx].laid.padding);
            let cw = (r.w - b.left - b.right - p.left - p.right).max(0.0);
            let ph = self.arena[idx].input.as_deref().and_then(|p| p.placeholder.clone());
            let mut ph_style = style.clone();
            ph_style.italic = self.arena[idx].style.placeholder.italic.unwrap_or(ph_style.italic);
            let Some(e) = self.edits.get_mut(&self.arena[idx].key) else { continue };
            let disp = e.display(password);
            e.ph_w = match &ph {
                Some(t) if disp.s.is_empty() => text.measure(t, &ph_style, None).w,
                _ => 0.0,
            };
            if e.stops_for.as_ref().is_none_or(|(s, st)| *s != disp.s || *st != style) {
                e.stops = if disp.s.is_empty() { vec![(0, 0.0)] } else { text.caret_stops(&disp.s, &style) };
                e.line_h = text.measure("M", &style, None).h;
                e.stops_for = Some((disp.s.clone(), style));
            }
            let tw = e.stops.last().map_or(0.0, |s| s.1);
            if tw <= cw {
                e.scroll_x = 0.0;
            } else {
                let spare = SCROLL_SPARE * scale;
                let cx = x_at(&e.stops, disp.caret);
                let mut s = e.scroll_x;
                if cx - s < spare {
                    s = cx - spare;
                } else if cx - s > cw - spare {
                    s = cx - cw + spare;
                }
                e.scroll_x = s.clamp(0.0, tw - cw + spare);
            }
        }
    }

    pub(super) fn focused_password_impl(&self) -> bool {
        self.focused_idx()
            .is_some_and(|i| self.arena[i].kind == crate::element::ElementKind::Input && self.is_password(i))
    }

    /// The focused input's caret, surface px.
    pub(super) fn caret_rect_impl(&self) -> Option<RectF> {
        let idx = self.focused_idx()?;
        if self.arena[idx].kind != crate::element::ElementKind::Input {
            return None;
        }
        let (g, e) = self.input_geom(idx)?;
        let disp = e.display(self.is_password(idx));
        let x = g.origin + x_at(&e.stops, disp.caret);
        let w = self.caret_width();
        let y = g.content.y + (g.content.h - g.line_h) / 2.0;
        Some(self.arena[idx].laid.xf.rect(&RectF::new(x, y, w, g.line_h)))
    }

    fn caret_width(&self) -> f32 {
        self.geometry.scale.round().max(1.0)
    }

    // ---- keys ----

    pub(super) fn edit_key(&mut self, idx: usize, ev: KeyEvent, now: Instant, res: &mut KeyResult) -> Used {
        let (ctrl, shift) = (ev.mods.ctrl, ev.mods.shift);
        let password = self.is_password(idx);
        let handler = self.arena[idx].input.as_deref().and_then(|p| p.on_submit);
        let instance = self.arena[idx].instance;
        let Some(e) = self.edit_of(idx) else { return Used::No };
        let mut changed = false;
        let mut moved = false;
        let used = match ev.key {
            Key::ArrowLeft | Key::ArrowRight => {
                let left = ev.key == Key::ArrowLeft;
                let (a, b) = e.selection();
                let to = if e.has_selection() && !shift {
                    if left { a } else { b }
                } else if left {
                    if ctrl { prev_word(&e.text, e.caret) } else { prev_boundary(&e.text, e.caret) }
                } else if ctrl {
                    next_word(&e.text, e.caret)
                } else {
                    next_boundary(&e.text, e.caret)
                };
                e.move_to(to, shift);
                moved = true;
                Used::Yes
            }
            Key::Home | Key::End => {
                let to = if ev.key == Key::Home { 0 } else { e.text.len() };
                e.move_to(to, shift);
                moved = true;
                Used::Yes
            }
            Key::Backspace | Key::Delete => {
                let back = ev.key == Key::Backspace;
                let range = if e.has_selection() {
                    Some(e.selection())
                } else if back && e.caret > 0 {
                    Some((if ctrl { prev_word(&e.text, e.caret) } else { prev_boundary(&e.text, e.caret) }, e.caret))
                } else if !back && e.caret < e.text.len() {
                    Some((e.caret, if ctrl { next_word(&e.text, e.caret) } else { next_boundary(&e.text, e.caret) }))
                } else {
                    None
                };
                if let Some((from, to)) = range {
                    e.begin(Group::Delete);
                    e.text.replace_range(from..to, "");
                    e.caret = from;
                    e.anchor = from;
                    changed = true;
                }
                Used::Yes
            }
            Key::Char('a') if ctrl => {
                e.anchor = 0;
                e.caret = e.text.len();
                e.group = None;
                moved = true;
                Used::Yes
            }
            Key::Char('c') | Key::Char('x') if ctrl => {
                if !password && e.has_selection() {
                    res.copy = Some(e.selected().to_string());
                    if ev.key == Key::Char('x') {
                        e.begin(Group::Other);
                        e.delete_selection();
                        changed = true;
                    }
                }
                Used::Yes
            }
            Key::Char('v') if ctrl => {
                res.paste = true;
                Used::Yes
            }
            Key::Char('z') if ctrl => {
                if let Some(s) = e.undo.take() {
                    e.text = s.text;
                    e.caret = s.caret;
                    e.anchor = s.anchor;
                    e.group = None;
                    changed = true;
                }
                Used::Yes
            }
            Key::Enter => {
                if let (Some(handler), Some(instance)) = (handler, instance) {
                    let event = ElementEvent::Submitted(Arc::from(e.text.as_str()));
                    self.events.push(ElementCall { instance, handler, event, gesture: 0 });
                }
                Used::AndKey
            }
            // Typed characters arrive as text.
            Key::Char(_) | Key::Space if !ctrl && !ev.mods.alt => Used::Yes,
            _ => Used::No,
        };
        if changed || moved {
            self.touch_edit(idx, now, changed);
        }
        used
    }

    /// An edit or a caret move happened: restart the blink, repaint, report.
    fn touch_edit(&mut self, idx: usize, now: Instant, changed: bool) {
        if let Some(e) = self.edit_of(idx) {
            e.epoch = now;
        }
        self.repaint = true;
        self.pending_reveals.push(self.arena[idx].key);
        if changed {
            self.report_change(idx, now);
        }
    }

    /// `on_change`: at once when the input was idle for 50 ms, else at the
    /// next 50 ms mark with the text as it is then.
    fn report_change(&mut self, idx: usize, now: Instant) {
        let key = self.arena[idx].key;
        let Some(e) = self.edits.get_mut(&key) else { return };
        if e.last_report.is_none_or(|t| now.saturating_duration_since(t) >= REPORT_GAP) {
            e.last_report = Some(now);
            e.pending = false;
            self.push_change(idx);
        } else {
            e.pending = true;
        }
    }

    fn push_change(&mut self, idx: usize) {
        let n = &self.arena[idx];
        let (Some(handler), Some(instance)) = (n.input.as_deref().and_then(|p| p.on_change), n.instance) else {
            return;
        };
        let Some(e) = self.edits.get(&n.key) else { return };
        let event = ElementEvent::TextChanged(Arc::from(e.text.as_str()));
        self.events.push(ElementCall { instance, handler, event, gesture: 0 });
    }

    /// Sends the report a rapid typist was made to wait for.
    pub(super) fn flush_reports(&mut self, now: Instant) {
        let due: Vec<NodeKey> = self
            .edits
            .iter()
            .filter(|(_, e)| e.pending && e.last_report.is_none_or(|t| now.saturating_duration_since(t) >= REPORT_GAP))
            .map(|(k, _)| *k)
            .collect();
        for k in due {
            if let Some(e) = self.edits.get_mut(&k) {
                e.pending = false;
                e.last_report = Some(now);
            }
            if let Some(i) = self.idx_of(k) {
                self.push_change(i);
            }
        }
    }

    /// The soonest a pending report is due.
    pub(super) fn edit_wake(&self, now: Instant) -> Option<Instant> {
        let mut wake: Option<Instant> = None;
        let mut add = |t: Instant| wake = Some(wake.map_or(t, |w: Instant| w.min(t)));
        for e in self.edits.values() {
            if e.pending
                && let Some(t) = e.last_report
            {
                add(t + REPORT_GAP);
            }
        }
        // The caret toggles.
        if let (Some(p), true, Some(idx)) = (self.caret_blink, self.focus.window, self.focused_idx())
            && self.arena[idx].kind == crate::element::ElementKind::Input
            && let Some(e) = self.edits.get(&self.arena[idx].key)
            && !p.is_zero()
        {
            let since = now.saturating_duration_since(e.epoch);
            let next = (since.as_micros() / p.as_micros() + 1) as u32;
            add(e.epoch + p * next);
        }
        wake
    }

    pub(super) fn text_input_impl(&mut self, input: TextInput, now: Instant) -> KeyResult {
        let mut res = KeyResult::default();
        if self.popup.is_none() || self.is_closing() || self.overlay_guarded(now) {
            return res;
        }
        self.sync(now);
        let Some(idx) = self.focused_idx() else { return res };
        if self.arena[idx].kind != crate::element::ElementKind::Input {
            return res;
        }
        let cap = self.input_cap(idx);
        let Some(e) = self.edit_of(idx) else { return res };
        let mut changed = true;
        match input {
            TextInput::Chars(s) => {
                e.begin(Group::Typing);
                e.insert(&s, cap);
            }
            TextInput::CompositionStart => {
                if e.has_selection() {
                    e.begin(Group::Typing);
                    e.delete_selection();
                } else {
                    changed = false;
                }
                e.comp = Some((String::new(), 0));
            }
            TextInput::CompositionUpdate { text, cursor } => {
                if e.has_selection() {
                    e.begin(Group::Typing);
                    e.delete_selection();
                } else {
                    changed = false;
                }
                e.comp = Some((text, cursor));
                self.repaint = true;
            }
            TextInput::CompositionCommit(s) => {
                e.comp = None;
                e.begin(Group::Typing);
                e.insert(&s, cap);
            }
            TextInput::CompositionEnd => {
                e.comp = None;
                changed = false;
            }
        }
        res.consumed = true;
        self.touch_edit(idx, now, changed);
        res
    }

    pub(super) fn paste_impl(&mut self, text: &str, now: Instant) {
        if self.popup.is_none() || self.is_closing() {
            return;
        }
        let Some(idx) = self.focused_idx() else { return };
        if self.arena[idx].kind != crate::element::ElementKind::Input {
            return;
        }
        let cap = self.input_cap(idx);
        let line = text.split(['\r', '\n']).next().unwrap_or("");
        let Some(e) = self.edit_of(idx) else { return };
        e.begin(Group::Other);
        e.insert(line, cap);
        self.touch_edit(idx, now, true);
    }

    // ---- pointer ----

    /// The byte of the text nearest layout x.
    fn byte_at(&self, idx: usize, x: f32) -> Option<usize> {
        let (g, e) = self.input_geom(idx)?;
        let password = self.is_password(idx);
        let rel = x - g.origin;
        e.stops
            .iter()
            .map(|&(b, sx)| (e.to_real(b, password), sx))
            .filter(|(real, _)| is_boundary(&e.text, *real))
            .min_by(|a, b| (a.1 - rel).abs().total_cmp(&(b.1 - rel).abs()))
            .map(|(real, _)| real)
    }

    pub(super) fn input_press(&mut self, idx: usize, p: PointF, now: Instant) {
        let Some(q) = self.arena[idx].laid.xf.inverse_point(p) else { return };
        let Some(at) = self.byte_at(idx, q.x) else { return };
        let key = self.arena[idx].key;
        let scale = self.geometry.scale;
        let Some(e) = self.edits.get_mut(&key) else { return };
        let near = |a: PointF| (a.x - p.x).abs() <= 4.0 * scale && (a.y - p.y).abs() <= 4.0 * scale;
        let again = e.last_click.is_some_and(|(t, a)| now.saturating_duration_since(t) <= MULTI_CLICK && near(a));
        e.clicks = if again { (e.clicks % 3) + 1 } else { 1 };
        e.last_click = Some((now, p));
        match e.clicks {
            1 => e.move_to(at, false),
            2 => {
                let (a, b) = word_range(&e.text, at);
                e.anchor = a;
                e.caret = b;
                e.group = None;
            }
            _ => {
                e.anchor = 0;
                e.caret = e.text.len();
                e.group = None;
            }
        }
        self.drag = Some(Drag::Input(key));
        self.touch_edit(idx, now, false);
    }

    pub(super) fn input_drag(&mut self, key: NodeKey, p: PointF, now: Instant) {
        let Some(idx) = self.idx_of(key) else { return };
        let Some(q) = self.arena[idx].laid.xf.inverse_point(p) else { return };
        let Some(at) = self.byte_at(idx, q.x) else { return };
        if let Some(e) = self.edits.get_mut(&key)
            && e.clicks <= 1
        {
            e.caret = at;
            e.group = None;
        }
        self.touch_edit(idx, now, false);
    }

    // ---- painting ----

    pub(super) fn paint_input(&self, idx: usize, out: &mut Vec<DrawItem>) {
        let Some((g, e)) = self.input_geom(idx) else { return };
        let n = &self.arena[idx];
        let s = &n.style;
        let xf = n.laid.xf;
        let scale = self.geometry.scale;
        let password = self.is_password(idx);
        let disp = e.display(password);
        let key = n.key;
        let focused = self.focus.window && self.focus.node == Some(key);
        let clip = g.content;
        if clip.is_empty() {
            return;
        }
        out.push(xf.item(DrawItem {
            node: key,
            bounds: clip,
            cmd: DrawCmd::PushClip { rect: clip, radii: Radii::default() },
        }));
        let line_y = g.content.y + (g.content.h - g.line_h) / 2.0;
        let style = text_style(s, scale);
        if focused && e.has_selection() && e.comp.is_none() {
            let (a, b) = e.selection();
            let (xa, xb) = (e.to_display(a, password), e.to_display(b, password));
            let r =
                RectF::new(g.origin + x_at(&e.stops, xa), line_y, x_at(&e.stops, xb) - x_at(&e.stops, xa), g.line_h);
            let colour = s.selection.background.unwrap_or_else(|| {
                let base = s.accent_color.unwrap_or(s.color);
                Color { a: base.a * 0.4, ..base }
            });
            if r.w > 0.0 && colour.a > 0.0 {
                out.push(xf.item(DrawItem {
                    node: key,
                    bounds: r,
                    cmd: DrawCmd::Fill { rect: r, radii: Radii::default(), fill: Fill::Solid(colour) },
                }));
            }
        }
        if disp.s.is_empty() {
            if let Some(ph) = n.input.as_deref().and_then(|p| p.placeholder.clone()) {
                let base = s.placeholder.color.unwrap_or(Color { a: s.color.a * 0.5, ..s.color });
                let colour = Color { a: base.a * s.placeholder.opacity.unwrap_or(1.0), ..base };
                let mut st = style.clone();
                st.italic = s.placeholder.italic.unwrap_or(st.italic);
                out.push(xf.item(DrawItem {
                    node: key,
                    bounds: g.content,
                    cmd: DrawCmd::Text { rect: g.content, text: ph, style: st, color: colour, align: s.text_align },
                }));
            }
        } else if g.tw > 0.0 {
            let r = RectF::new(g.origin, g.content.y, g.tw, g.content.h);
            out.push(xf.item(DrawItem {
                node: key,
                bounds: r,
                cmd: DrawCmd::Text {
                    rect: r,
                    text: Arc::from(disp.s.as_str()),
                    style: style.clone(),
                    color: s.color,
                    align: TextAlign::Start,
                },
            }));
        }
        if let Some((a, b)) = disp.comp {
            let h = self.caret_width();
            let r = RectF::new(
                g.origin + x_at(&e.stops, a),
                line_y + g.line_h - h,
                x_at(&e.stops, b) - x_at(&e.stops, a),
                h,
            );
            if r.w > 0.0 {
                out.push(xf.item(DrawItem {
                    node: key,
                    bounds: r,
                    cmd: DrawCmd::Fill { rect: r, radii: Radii::default(), fill: Fill::Solid(s.color) },
                }));
            }
        }
        if focused && e.caret_visible(self.caret_blink, self.now) {
            let w = self.caret_width();
            let r = RectF::new(g.origin + x_at(&e.stops, disp.caret), line_y, w, g.line_h);
            let colour = s.caret_color.unwrap_or(s.color);
            out.push(xf.item(DrawItem {
                node: key,
                bounds: r,
                cmd: DrawCmd::Fill { rect: r, radii: Radii::default(), fill: Fill::Solid(colour) },
            }));
        }
        out.push(xf.item(DrawItem { node: key, bounds: clip, cmd: DrawCmd::PopClip }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_steps_over_combining_marks_and_emoji_sequences() {
        let t = "ae\u{301}b";
        assert_eq!(next_boundary(t, 1), 4);
        assert_eq!(prev_boundary(t, 4), 1);
        let family = "\u{1F468}\u{200D}\u{1F469}x";
        assert_eq!(next_boundary(family, 0), family.len() - 1);
        assert_eq!(prev_boundary(family, family.len() - 1), 0);
        let flags = "\u{1F1E6}\u{1F1FA}\u{1F1E7}\u{1F1EA}";
        assert_eq!(next_boundary(flags, 0), 8);
        assert_eq!(prev_boundary(flags, 16), 8);
        assert_eq!(next_boundary("", 0), 0);
        assert_eq!(prev_boundary("a", 0), 0);
    }

    #[test]
    fn words_move_by_runs() {
        let t = "foo  bar.baz";
        assert_eq!(next_word(t, 0), 5);
        assert_eq!(next_word(t, 5), 8);
        assert_eq!(prev_word(t, 12), 9);
        assert_eq!(prev_word(t, 9), 8);
        assert_eq!(prev_word(t, 5), 0);
        assert_eq!(word_range(t, 6), (5, 8));
        assert_eq!(word_range(t, 0), (0, 3));
    }

    #[test]
    fn boundaries_are_found_inside_clusters() {
        let t = "ae\u{301}b";
        assert!(is_boundary(t, 1));
        assert!(!is_boundary(t, 2));
        assert!(is_boundary(t, 4));
    }

    #[test]
    fn typing_is_capped_and_replaces_the_selection() {
        let mut e = EditState::new("hello", 0, Instant::now());
        e.anchor = 0;
        e.caret = 5;
        e.insert("abcdef", 4);
        assert_eq!(e.text, "abcd");
        assert_eq!((e.caret, e.anchor), (4, 4));
        e.insert("\nz\u{7}", 10);
        assert_eq!(e.text, "abcdz");
    }
}

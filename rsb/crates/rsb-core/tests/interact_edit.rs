mod common;

use std::sync::Arc;
use std::time::Instant;

use common::*;
use rsb_core::draw::DrawCmd;
use rsb_core::element::{Element, ElementKind, HandlerId, InputProps};
use rsb_core::geom::PointF;
use rsb_core::hit::{Button, ElementEvent, PointerEvent, PointerKind};
use rsb_core::key::{Key, TextInput};
use rsb_core::popup::Side;
use rsb_core::scene::Scene;

const CSS: &str = "bar { font-size: 10px } popup { display: flex; flex-direction: column; width: 100px; background: #111 } \
                   input { width: 100px; height: 16px; color: #fff; caret-color: #f00 } \
                   input::selection { background-color: #00f } input::placeholder { color: #888 }";
const CHANGE: HandlerId = HandlerId(7);
const SUBMIT: HandlerId = HandlerId(8);

fn props(f: impl FnOnce(&mut InputProps)) -> InputProps {
    let mut p =
        InputProps { value: Arc::from(""), on_change: Some(CHANGE), on_submit: Some(SUBMIT), ..InputProps::default() };
    f(&mut p);
    p
}

/// A focused input in a popup scene; the clock starts at `t`.
fn make(css: &str, p: InputProps, t: Instant) -> Scene {
    let mut input = Element::new(ElementKind::Input);
    input.input = Some(Box::new(p));
    let (bar, _) = bar_with_popup(css, open_props(1), vec![input]);
    let d = decls(&bar);
    let mut s = popup_scene(css, &d[0], Side::Below, 160, 60);
    s.set_open(true, t);
    s.set_window_focus(true, t);
    s.focus_first(true, t);
    frame(&mut s, t);
    s
}

fn texts_of(s: &mut Scene, t: Instant) -> Vec<String> {
    let out = frame(s, t);
    texts(&out.draw.items)
}

fn typed(s: &mut Scene, text: &str, t: Instant) {
    s.text_input(TextInput::Chars(text.into()), t);
}

fn press(s: &mut Scene, k: Key, ctrl: bool, shift: bool, t: Instant) -> rsb_core::scene::KeyResult {
    s.key(key_mod(k, ctrl, shift), t)
}

fn events(s: &mut Scene) -> Vec<(HandlerId, ElementEvent)> {
    s.take_events().into_iter().map(|e| (e.handler, e.event)).collect()
}

fn changes(s: &mut Scene) -> Vec<String> {
    events(s)
        .into_iter()
        .filter_map(|(h, e)| match e {
            ElementEvent::TextChanged(t) if h == CHANGE => Some(t.to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn typing_inserts_at_the_caret_and_reports_the_first_edit_at_once() {
    let t = Instant::now();
    let mut s = make(CSS, props(|_| {}), t);
    typed(&mut s, "ab", t);
    assert_eq!(texts_of(&mut s, t), ["ab"]);
    assert_eq!(changes(&mut s), ["ab"], "leading edge");
    press(&mut s, Key::ArrowLeft, false, false, t);
    typed(&mut s, "X", t + ms(10));
    assert_eq!(texts_of(&mut s, t + ms(10)), ["aXb"]);
    assert!(changes(&mut s).is_empty(), "inside 50 ms of the last report: held back");
}

#[test]
fn reports_are_at_most_one_every_50_ms_and_the_last_text_follows() {
    let t = Instant::now();
    let mut s = make(CSS, props(|_| {}), t);
    typed(&mut s, "a", t);
    assert_eq!(changes(&mut s), ["a"]);
    typed(&mut s, "b", t + ms(10));
    typed(&mut s, "c", t + ms(20));
    assert!(changes(&mut s).is_empty());
    let out = frame(&mut s, t + ms(30));
    let due = out.next_frame.expect("a held back report sets the next frame");
    assert_eq!(due.duration_since(t), ms(50));
    frame(&mut s, t + ms(49));
    assert!(changes(&mut s).is_empty());
    frame(&mut s, t + ms(50));
    assert_eq!(changes(&mut s), ["abc"], "the trailing report carries the final text");
    let out = frame(&mut s, t + ms(60));
    assert!(out.next_frame.is_none() || changes(&mut s).is_empty());
    // After 50 ms of quiet the next edit is reported at once again.
    typed(&mut s, "d", t + ms(400));
    assert_eq!(changes(&mut s), ["abcd"]);
}

#[test]
fn backspace_delete_and_ctrl_delete_whole_words() {
    let t = Instant::now();
    let mut s = make(CSS, props(|p| p.value = Arc::from("foo bar baz")), t);
    press(&mut s, Key::Backspace, false, false, t);
    assert_eq!(texts_of(&mut s, t), ["foo bar ba"]);
    press(&mut s, Key::Backspace, true, false, t);
    assert_eq!(texts_of(&mut s, t), ["foo bar "]);
    press(&mut s, Key::Home, false, false, t);
    press(&mut s, Key::Delete, false, false, t);
    assert_eq!(texts_of(&mut s, t), ["oo bar "]);
    press(&mut s, Key::Delete, true, false, t);
    assert_eq!(texts_of(&mut s, t), ["bar "]);
    // Nothing to delete at the ends.
    press(&mut s, Key::End, false, false, t);
    press(&mut s, Key::Delete, false, false, t);
    assert_eq!(texts_of(&mut s, t), ["bar "]);
}

#[test]
fn arrows_extend_collapse_and_select_all() {
    let t = Instant::now();
    let mut s = make(CSS, props(|p| p.value = Arc::from("hello")), t);
    press(&mut s, Key::ArrowLeft, false, true, t);
    press(&mut s, Key::ArrowLeft, false, true, t);
    typed(&mut s, "Z", t);
    assert_eq!(texts_of(&mut s, t), ["helZ"], "the selection was replaced");
    press(&mut s, Key::Char('a'), true, false, t);
    typed(&mut s, "q", t + ms(100));
    assert_eq!(texts_of(&mut s, t + ms(100)), ["q"]);
    // Left without shift collapses a selection to its left edge.
    typed(&mut s, "rs", t + ms(200));
    press(&mut s, Key::ArrowLeft, false, true, t + ms(200));
    press(&mut s, Key::ArrowLeft, false, true, t + ms(200));
    press(&mut s, Key::ArrowRight, false, false, t + ms(200));
    typed(&mut s, "-", t + ms(300));
    assert_eq!(texts_of(&mut s, t + ms(300)), ["qrs-"]);
}

#[test]
fn copy_and_cut_give_the_selection_and_a_password_gives_nothing() {
    let t = Instant::now();
    let mut s = make(CSS, props(|p| p.value = Arc::from("secret")), t);
    press(&mut s, Key::Char('a'), true, false, t);
    let r = press(&mut s, Key::Char('c'), true, false, t);
    assert_eq!(r.copy.as_deref(), Some("secret"));
    assert!(r.consumed);
    let r = press(&mut s, Key::Char('x'), true, false, t);
    assert_eq!(r.copy.as_deref(), Some("secret"));
    assert_eq!(texts_of(&mut s, t), Vec::<String>::new(), "cut it all");
    let mut pw = make(
        CSS,
        props(|p| {
            p.value = Arc::from("secret");
            p.password = true;
        }),
        t,
    );
    press(&mut pw, Key::Char('a'), true, false, t);
    assert_eq!(press(&mut pw, Key::Char('c'), true, false, t).copy, None);
    assert_eq!(press(&mut pw, Key::Char('x'), true, false, t).copy, None);
    assert_eq!(
        texts_of(&mut pw, t),
        ["\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}"],
        "cut refused, bullets drawn"
    );
}

#[test]
fn paste_takes_the_first_line_and_respects_max_length() {
    let t = Instant::now();
    let mut s = make(CSS, props(|p| p.max_length = 6), t);
    let r = press(&mut s, Key::Char('v'), true, false, t);
    assert!(r.paste);
    s.paste("abcd\nsecond line", t);
    assert_eq!(texts_of(&mut s, t), ["abcd"]);
    typed(&mut s, "efgh", t + ms(100));
    assert_eq!(texts_of(&mut s, t + ms(100)), ["abcdef"], "dropped past max_length");
}

#[test]
fn undo_takes_back_a_run_of_typing_at_once() {
    let t = Instant::now();
    let mut s = make(CSS, props(|p| p.value = Arc::from("x")), t);
    typed(&mut s, "a", t);
    typed(&mut s, "b", t);
    typed(&mut s, "c", t);
    assert_eq!(texts_of(&mut s, t), ["xabc"]);
    press(&mut s, Key::Char('z'), true, false, t);
    assert_eq!(texts_of(&mut s, t), ["x"]);
    // One level only.
    press(&mut s, Key::Char('z'), true, false, t);
    assert_eq!(texts_of(&mut s, t), ["x"]);
}

#[test]
fn enter_submits_and_goes_to_on_key_while_up_and_down_are_left_to_it() {
    let t = Instant::now();
    let mut input = Element::new(ElementKind::Input);
    input.input = Some(Box::new(props(|p| p.value = Arc::from("go"))));
    input.handlers.key = Some(HandlerId(9));
    let (bar, _) = bar_with_popup(CSS, open_props(1), vec![input]);
    let mut s = popup_scene(CSS, &decls(&bar)[0], Side::Below, 160, 60);
    s.set_open(true, t);
    s.focus_first(true, t);
    frame(&mut s, t);
    let r = press(&mut s, Key::Enter, false, false, t);
    assert!(r.consumed && !r.dismiss);
    let ev = events(&mut s);
    assert!(ev.iter().any(|(h, e)| *h == SUBMIT && matches!(e, ElementEvent::Submitted(t) if &**t == "go")), "{ev:?}");
    assert!(ev.iter().any(|(h, e)| *h == HandlerId(9) && matches!(e, ElementEvent::Key(_))), "{ev:?}");
    let r = press(&mut s, Key::ArrowDown, false, false, t);
    assert!(!r.consumed);
    let ev = events(&mut s);
    assert_eq!(ev.len(), 1);
    assert!(matches!(&ev[0], (HandlerId(9), ElementEvent::Key(k)) if k.key == Key::ArrowDown));
}

#[test]
fn a_newer_value_seq_replaces_the_text_and_an_echo_does_not() {
    let t = Instant::now();
    let mut input = Element::new(ElementKind::Input);
    input.input = Some(Box::new(props(|p| p.value = Arc::from("one"))));
    let (mut bar, tree) = bar_with_popup(CSS, open_props(1), vec![input]);
    let d = decls(&bar);
    let mut s = popup_scene(CSS, &d[0], Side::Below, 160, 60);
    s.set_open(true, t);
    s.focus_first(true, t);
    typed(&mut s, "2", t);
    assert_eq!(texts_of(&mut s, t), ["one2"]);
    let with = |seq: u64, value: &str, version: u64| {
        let mut root = tree.root.clone().unwrap();
        let i = root.children[0].children[0].children[0].input.as_mut().unwrap();
        i.value = Arc::from(value);
        i.value_seq = seq;
        Arc::new(rsb_core::element::WidgetTree { version, root: Some(root), ..(*tree).clone() })
    };
    // The script echoes the reported text: same seq, nothing changes.
    s.set_popup_tree(with(0, "one2", 2), d[0].path.clone());
    assert_eq!(texts_of(&mut s, t + ms(10)), ["one2"]);
    // The script wrote a new value: it wins and the caret goes to the end.
    s.set_popup_tree(with(1, "cleared", 3), d[0].path.clone());
    assert_eq!(texts_of(&mut s, t + ms(20)), ["cleared"]);
    typed(&mut s, "!", t + ms(300));
    assert_eq!(texts_of(&mut s, t + ms(300)), ["cleared!"]);
    let _ = &mut bar;
}

#[test]
fn ime_composition_is_drawn_at_the_caret_underlined_and_commits_text() {
    let t = Instant::now();
    let mut s = make(CSS, props(|p| p.value = Arc::from("ab")), t);
    press(&mut s, Key::ArrowLeft, false, false, t);
    s.text_input(TextInput::CompositionStart, t);
    s.text_input(TextInput::CompositionUpdate { text: "xyz".into(), cursor: 2 }, t);
    let out = frame(&mut s, t);
    assert_eq!(texts(&out.draw.items), ["axyzb"], "the composition sits at the caret");
    assert!(changes(&mut s).is_empty(), "it is not part of the value yet");
    // An underline 1 px tall under the 3 composed characters (18 px).
    let underline =
        out.draw.items.iter().any(|i| matches!(&i.cmd, DrawCmd::Fill { rect, .. } if rect.h == 1.0 && rect.w == 18.0));
    assert!(underline);
    // The caret sits inside the composition, after 2 of its 3 characters.
    let caret = s.caret_rect().unwrap();
    s.text_input(TextInput::CompositionCommit("XY".into()), t + ms(10));
    s.text_input(TextInput::CompositionEnd, t + ms(10));
    assert_eq!(texts_of(&mut s, t + ms(10)), ["aXYb"]);
    assert_eq!(changes(&mut s), ["aXYb"]);
    assert!(caret.x > 0.0);
    // A cancelled composition leaves the text alone.
    s.text_input(TextInput::CompositionStart, t + ms(200));
    s.text_input(TextInput::CompositionUpdate { text: "q".into(), cursor: 1 }, t + ms(200));
    s.text_input(TextInput::CompositionEnd, t + ms(200));
    assert_eq!(texts_of(&mut s, t + ms(200)), ["aXYb"]);
}

#[test]
fn text_align_center_centres_text_and_caret_and_scrolls_like_left_when_it_overflows() {
    let t = Instant::now();
    let css = format!("{CSS} input {{ text-align: center }}");
    let mut s = make(&css, props(|_| {}), t);
    // Empty: the caret alone is centred in the 100 px box.
    let c = s.caret_rect().unwrap();
    let input = s.nodes().into_iter().find(|n| n.kind == ElementKind::Input).unwrap().rect;
    assert!((c.x - (input.x + 50.0)).abs() <= 1.0, "{} vs {}", c.x, input.x + 50.0);
    typed(&mut s, "abcd", t);
    let out = frame(&mut s, t);
    let text = out.draw.items.iter().find_map(|i| match &i.cmd {
        DrawCmd::Text { rect, text, .. } if &**text == "abcd" => Some(*rect),
        _ => None,
    });
    let r = text.unwrap();
    // 4 chars x 6 px = 24 px, centred: starts at (100 - 24) / 2.
    assert_eq!(r.x - input.x, 38.0);
    let c = s.caret_rect().unwrap();
    assert_eq!(c.x - input.x, 38.0 + 24.0);
    // Wider than the box: it scrolls to keep the caret in view, as a left-aligned one does.
    typed(&mut s, "efghijklmnopqrstuvwxyz", t + ms(100));
    let out = frame(&mut s, t + ms(100));
    let r = out.draw.items.iter().find_map(|i| match &i.cmd {
        DrawCmd::Text { rect, text, .. } if text.starts_with("abcd") => Some(*rect),
        _ => None,
    });
    let r = r.unwrap();
    assert!(r.x < input.x, "scrolled left: {}", r.x);
    let c = s.caret_rect().unwrap();
    assert!(c.x >= input.x && c.right() <= input.x + 100.0 - 1.0, "caret in view: {c:?}");
}

#[test]
fn the_caret_blinks_with_the_period_it_was_given_and_is_steady_after_an_edit() {
    let t = Instant::now();
    let mut s = make(CSS, props(|_| {}), t);
    s.set_caret_blink(Some(ms(530)));
    let carets = |s: &mut Scene, at: Instant| {
        let out = frame(s, at);
        out.draw.items.iter().filter(|i| matches!(&i.cmd, DrawCmd::Fill { fill: rsb_core::draw::Fill::Solid(c), .. } if c.r > 0.9 && c.g < 0.1)).count()
    };
    assert_eq!(carets(&mut s, t), 1, "on");
    let out = frame(&mut s, t + ms(100));
    assert_eq!(out.next_frame.map(|n| n.duration_since(t)), Some(ms(530)), "the next toggle");
    assert_eq!(carets(&mut s, t + ms(600)), 0, "off");
    assert_eq!(carets(&mut s, t + ms(1100)), 1, "on again");
    typed(&mut s, "a", t + ms(1200));
    assert_eq!(carets(&mut s, t + ms(1200)), 1);
    assert_eq!(carets(&mut s, t + ms(1700)), 1, "steady for a period after an edit");
    assert_eq!(carets(&mut s, t + ms(1800)), 0);
    s.set_caret_blink(None);
    assert_eq!(carets(&mut s, t + ms(1800)), 1, "no blink: steady");
    // Without window focus there is no caret.
    s.set_window_focus(false, t + ms(1900));
    assert_eq!(carets(&mut s, t + ms(1900)), 0);
}

#[test]
fn an_empty_input_puts_the_caret_at_the_start_of_its_placeholder_not_through_it() {
    let t = Instant::now();
    let css = format!("{CSS} input {{ text-align: center }}");
    let mut s = make(&css, props(|p| p.placeholder = Some(Arc::from("Search"))), t);
    let input = s.nodes().into_iter().find(|n| n.kind == ElementKind::Input).unwrap().rect;
    // "Search" is 6 chars x 6 px = 36 px, centred in 100 px: it starts at 32.
    let c = s.caret_rect().unwrap();
    assert_eq!(c.x - input.x, 32.0, "caret at the placeholder's first edge");
    // Typing replaces the placeholder: the text centres on its own width.
    typed(&mut s, "ab", t);
    frame(&mut s, t);
    let c = s.caret_rect().unwrap();
    assert_eq!(c.x - input.x, 44.0 + 12.0);
    // Left-aligned, the placeholder and the caret both start at the edge.
    let s = make(CSS, props(|p| p.placeholder = Some(Arc::from("Search"))), t);
    assert_eq!(s.caret_rect().unwrap().x - input.x, 0.0);
}

#[test]
fn selection_and_placeholder_are_painted() {
    let t = Instant::now();
    let mut s = make(CSS, props(|p| p.placeholder = Some(Arc::from("Search"))), t);
    let out = frame(&mut s, t);
    assert_eq!(texts(&out.draw.items), ["Search"], "placeholder while empty");
    typed(&mut s, "abcd", t);
    press(&mut s, Key::ArrowLeft, false, true, t);
    press(&mut s, Key::ArrowLeft, false, true, t);
    let out = frame(&mut s, t);
    assert_eq!(texts(&out.draw.items), ["abcd"]);
    let sel = out.draw.items.iter().find_map(|i| match &i.cmd {
        DrawCmd::Fill { rect, fill: rsb_core::draw::Fill::Solid(c), .. } if c.b > 0.9 && c.r < 0.1 => Some(*rect),
        _ => None,
    });
    let r = sel.expect("a selection fill in the ::selection colour");
    assert_eq!(r.w, 12.0, "two characters");
}

#[test]
fn a_click_places_the_caret_and_a_drag_selects_double_selects_a_word_triple_all() {
    let t = Instant::now();
    let mut s = make(CSS, props(|p| p.value = Arc::from("foo bar")), t);
    let input = s.nodes().into_iter().find(|n| n.kind == ElementKind::Input).unwrap().rect;
    let at = |chars: f32| PointF { x: input.x + chars * 6.0 + 1.0, y: input.y + 8.0 };
    let ev = |kind: PointerKind, p: PointF| PointerEvent { kind, x: p.x, y: p.y };
    // Click between "fo" and "o".
    s.pointer(ev(PointerKind::Down(Button::Left), at(2.0)), t);
    s.pointer(ev(PointerKind::Up(Button::Left), at(2.0)), t);
    typed(&mut s, "_", t);
    assert_eq!(texts_of(&mut s, t), ["fo_o bar"]);
    // Drag from 1 to 4 selects "o_o".
    let t2 = t + ms(2000);
    s.pointer(ev(PointerKind::Down(Button::Left), at(1.0)), t2);
    s.pointer(ev(PointerKind::Move, at(4.0)), t2);
    s.pointer(ev(PointerKind::Up(Button::Left), at(4.0)), t2);
    typed(&mut s, "#", t2);
    assert_eq!(texts_of(&mut s, t2), ["f# bar"]);
    // Double click selects the word.
    let t3 = t + ms(4000);
    for _ in 0..2 {
        s.pointer(ev(PointerKind::Down(Button::Left), at(4.0)), t3);
        s.pointer(ev(PointerKind::Up(Button::Left), at(4.0)), t3);
    }
    typed(&mut s, "W", t3 + ms(100));
    assert_eq!(texts_of(&mut s, t3 + ms(100)), ["f# W"]);
    // Triple click selects all.
    let t4 = t + ms(6000);
    for _ in 0..3 {
        s.pointer(ev(PointerKind::Down(Button::Left), at(1.0)), t4);
        s.pointer(ev(PointerKind::Up(Button::Left), at(1.0)), t4);
    }
    typed(&mut s, "all", t4 + ms(100));
    assert_eq!(texts_of(&mut s, t4 + ms(100)), ["all"]);
}

#[test]
fn the_input_shows_the_text_cursor() {
    let t = Instant::now();
    let css = format!("{CSS} input {{ cursor: text }}");
    let mut s = make(&css, props(|_| {}), t);
    let input = s.nodes().into_iter().find(|n| n.kind == ElementKind::Input).unwrap().rect;
    s.pointer(PointerEvent { kind: PointerKind::Move, x: input.x + 5.0, y: input.y + 5.0 }, t);
    let out = frame(&mut s, t);
    assert_eq!(out.cursor, rsb_core::style::Cursor::Text);
}

#[test]
fn focused_password_is_true_only_while_a_password_input_has_focus() {
    let t = Instant::now();
    let plain = make(CSS, props(|_| {}), t);
    assert!(!plain.focused_password());
    let pw = make(CSS, props(|p| p.password = true), t);
    assert!(pw.focused_password());
}

#[test]
fn focused_password_follows_the_focus_between_a_plain_and_a_password_input() {
    let t = Instant::now();
    let field = |password: bool| {
        let mut input = Element::new(ElementKind::Input);
        input.input = Some(Box::new(props(|p| p.password = password)));
        input
    };
    let (bar, _) = bar_with_popup(CSS, open_props(1), vec![field(false), field(true)]);
    let mut s = popup_scene(CSS, &decls(&bar)[0], Side::Below, 160, 60);
    s.set_open(true, t);
    s.set_window_focus(true, t);
    s.focus_first(true, t);
    frame(&mut s, t);
    assert!(!s.focused_password(), "the plain field has the focus first");
    press(&mut s, Key::Tab, false, false, t);
    frame(&mut s, t);
    assert!(s.focused_password(), "Tab moves to the password field");
    press(&mut s, Key::Tab, false, false, t);
    frame(&mut s, t);
    assert!(!s.focused_password(), "and on to the plain one again");
}

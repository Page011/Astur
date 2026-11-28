//! Random input into a popup that holds an input, a slider, a scroll list and
//! menu items: nothing panics and the scene keeps answering.

mod common;

use std::sync::Arc;
use std::time::Instant;

use common::*;
use rsb_core::element::{Element, ElementKind, HandlerId, InputProps, NavAxis, SliderProps};
use rsb_core::hit::{Button, PointerEvent, PointerKind};
use rsb_core::key::{Key, KeyEvent, Modifiers, TextInput};
use rsb_core::popup::Side;
use rsb_core::scene::Scene;

const CSS: &str = "bar { font-size: 10px } popup { display: flex; flex-direction: column; width: 120px; height: 100px; background: #111 } \
                   input { width: 100px; height: 14px; text-align: center } slider { width: 100px; height: 8px } \
                   .list { overflow-y: auto; height: 40px; display: flex; flex-direction: column } .row { height: 16px } \
                   menuitem { height: 14px } menuitem:focus { color: #f00 } input:focus { outline: 1px solid #fff }";

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

fn content() -> Vec<Element> {
    let mut input = Element::new(ElementKind::Input);
    input.input = Some(Box::new(InputProps {
        value: Arc::from("h\u{e9}llo e\u{301} \u{1F468}\u{200D}\u{1F469}"),
        on_change: Some(HandlerId(1)),
        on_submit: Some(HandlerId(2)),
        max_length: 40,
        placeholder: Some(Arc::from("type")),
        ..InputProps::default()
    }));
    let slider = slider_el(SliderProps { on_change: Some(HandlerId(3)), step: 3.0, ..SliderProps::default() });
    let rows = (0..8).map(|_| classed(focusable(boxed(vec![])), &["row"])).collect();
    let list = classed(boxed(rows), &["list"]);
    let mut sub = popup_el(rsb_core::popup::PopupProps::default(), ElementKind::Menu, vec![]);
    sub.children = vec![];
    let mut items = Element::new(ElementKind::MenuItem);
    items.text = Some(Arc::from("Item"));
    items.children = vec![sub];
    let mut menu = kind_el(ElementKind::Box, vec![items.clone(), items]);
    menu.interaction.nav = NavAxis::Vertical;
    vec![input, slider, list, menu]
}

fn random_key(r: &mut Lcg) -> KeyEvent {
    let keys = [
        Key::Enter,
        Key::Escape,
        Key::Tab,
        Key::Backspace,
        Key::Delete,
        Key::Space,
        Key::ArrowUp,
        Key::ArrowDown,
        Key::ArrowLeft,
        Key::ArrowRight,
        Key::Home,
        Key::End,
        Key::PageUp,
        Key::PageDown,
        Key::Char('a'),
        Key::Char('c'),
        Key::Char('x'),
        Key::Char('v'),
        Key::Char('z'),
        Key::Char('q'),
        Key::F(5),
        Key::Other(255),
    ];
    KeyEvent {
        key: *r.pick(&keys),
        mods: Modifiers { ctrl: r.below(3) == 0, shift: r.below(3) == 0, alt: r.below(8) == 0, meta: false },
        down: r.below(10) != 0,
        repeat: r.below(6) == 0,
    }
}

fn random_text(r: &mut Lcg) -> TextInput {
    let words =
        ["a", "bc", "\u{e9}", "e\u{301}", "\u{1F600}", "\u{1F1E6}\u{1F1FA}", "\u{4e2d}\u{6587}", " ", "x y z", ""];
    let w = r.pick(&words).to_string();
    match r.below(6) {
        0 => TextInput::CompositionStart,
        1 => TextInput::CompositionUpdate { text: w, cursor: r.below(5) as usize },
        2 => TextInput::CompositionCommit(w),
        3 => TextInput::CompositionEnd,
        _ => TextInput::Chars(w),
    }
}

fn run(seed: u64) {
    let mut r = Lcg(seed);
    let (bar, tree) = bar_with_popup(CSS, open_props(1), content());
    let d = decls(&bar);
    let mut s: Scene = popup_scene(CSS, &d[0], Side::Below, 160, 140);
    let start = Instant::now();
    let mut now = start;
    s.set_open(true, now);
    s.set_window_focus(true, now);
    s.focus_first(true, now);
    s.set_caret_blink(Some(ms(500)));
    for step in 0..400u64 {
        now += ms(1 + r.below(40));
        match r.below(14) {
            0..=3 => {
                s.key(random_key(&mut r), now);
            }
            4..=6 => {
                s.text_input(random_text(&mut r), now);
            }
            7 => s.paste(["abc", "d\ne", "\u{1F468}\u{200D}\u{1F469}", ""][r.below(4) as usize], now),
            8..=10 => {
                let kinds = [
                    PointerKind::Move,
                    PointerKind::Down(Button::Left),
                    PointerKind::Up(Button::Left),
                    PointerKind::Wheel(r.below(5) as f32 - 2.0),
                    PointerKind::Leave,
                    PointerKind::Down(Button::Right),
                ];
                let ev =
                    PointerEvent { kind: *r.pick(&kinds), x: r.below(170) as f32 - 5.0, y: r.below(150) as f32 - 5.0 };
                s.pointer(ev, now);
            }
            11 => {
                let open = r.below(2) == 0;
                s.set_open(open, now);
            }
            12 => {
                s.set_window_focus(r.below(2) == 0, now);
            }
            _ => {
                let mut root = tree.root.clone().unwrap();
                let el = &mut root.children[0].children[0].children[0];
                if let Some(i) = el.input.as_mut() {
                    i.value_seq = step;
                    i.value = Arc::from(["", "reset", "\u{e9}\u{301}x"][r.below(3) as usize]);
                }
                s.set_popup_tree(
                    Arc::new(rsb_core::element::WidgetTree { version: 2 + step, root: Some(root), ..(*tree).clone() }),
                    d[0].path.clone(),
                );
            }
        }
        if step % 3 == 0 {
            let out = s.frame(now, &mut rsb_core::layout::FixedMeasure);
            assert_eq!(out.draw.width, 160);
        }
        let _ = (s.caret_rect(), s.take_events(), s.popups(), s.root_outline(), s.hovered_tooltip(), s.overlay_dim());
    }
}

#[test]
fn random_input_never_panics() {
    for seed in 1..=60 {
        run(seed);
    }
}

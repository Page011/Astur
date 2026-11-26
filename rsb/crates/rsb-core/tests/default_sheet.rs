//! Rules of the default sheet that no widget exercises on its own: a bare
//! slider, menu separators and labels, and the one-line accent override.

mod common;

use std::sync::Arc;
use std::time::Instant;

use common::*;
use rsb_core::color::Color;
use rsb_core::draw::{DrawCmd, Fill};
use rsb_core::element::{Element, ElementKind, HandlerId, NavAxis, SliderProps};
use rsb_core::popup::{PopupProps, Side};
use rsb_core::scene::Scene;

const BASE: &str = include_str!("../../../assets/themes/base.css");
const DEFAULT: &str = include_str!("../../../assets/presets/default.css");

fn css(extra: &str) -> String {
    format!("{BASE}\n{DEFAULT}\n{extra}")
}

fn open_popup(css: &str, kind: ElementKind, content: Vec<Element>, t: Instant) -> Scene {
    let mut popup = popup_el(PopupProps { group: Arc::from("g"), ..open_props(1) }, kind, content);
    popup.interaction.nav = NavAxis::Vertical;
    // `rsb.menu` makes its menu a column.
    popup.direction = rsb_core::style::FlexDirection::Column;
    let anchor = classed(boxed(vec![popup]), &["anchor"]);
    let mut bar = scene_with(css, 600, 1.0, Some(anchor), None, None);
    frame(&mut bar, t);
    let d = decls(&bar);
    let mut s = popup_scene(css, &d[0], Side::Below, 400, 300);
    s.set_open(true, t);
    s.set_window_focus(true, t);
    frame(&mut s, t);
    s
}

fn solid(c: &str) -> Color {
    Color::parse(c).unwrap()
}

fn same(a: Color, b: Color) -> bool {
    (a.r - b.r).abs() < 0.01 && (a.g - b.g).abs() < 0.01 && (a.b - b.b).abs() < 0.01
}

#[test]
fn a_bare_slider_has_a_width_a_visible_track_and_a_focus_ring() {
    let t = Instant::now();
    let slider = classed(slider_el(SliderProps { value: 40.0, ..SliderProps::default() }), &["bare"]);
    let mut s = open_popup(&css(""), ElementKind::Popup, vec![slider], t);
    let r = node_by_class(&s, "bare").rect;
    assert!(r.w >= 80.0, "a slider with no width is not 0 px wide: {r:?}");
    s.focus_first(true, t);
    let out = frame(&mut s, t);
    let ring = out.draw.items.iter().any(|i| match &i.cmd {
        DrawCmd::Border { rect, widths, color, .. } => {
            widths.top == 2.0 && same(*color, solid("#81cbea")) && (rect.w - (r.w + 8.0)).abs() <= 1.0
        }
        _ => false,
    });
    assert!(ring, "a focused slider is ringed in the accent line colour");
    // The fill shows in the accent colour over the track.
    assert!(
        out.draw
            .items
            .iter()
            .any(|i| matches!(&i.cmd, DrawCmd::Fill { fill: Fill::Solid(c), .. } if same(*c, solid("#81cbea")))),
        "the value is drawn in the accent colour"
    );
}

#[test]
fn a_menu_separator_draws_a_line_across_and_labels_start_at_the_left() {
    let t = Instant::now();
    let item = |label: &str| {
        let mut e = Element::new(ElementKind::MenuItem);
        e.text = Some(Arc::from(label));
        e.handlers.click = Some(HandlerId(1));
        e
    };
    let separator = classed(boxed(vec![]), &["separator"]);
    let mut s = open_popup(&css(""), ElementKind::Menu, vec![item("Open"), separator, item("Close the window")], t);
    let out = frame(&mut s, t);
    let nodes = s.nodes();
    let sep = nodes.iter().find(|n| has(n, "separator")).expect("the separator").rect;
    let items: Vec<_> = nodes.iter().filter(|n| n.kind == ElementKind::MenuItem).collect();
    assert_eq!(items.len(), 2);
    assert_eq!(sep.h, 1.0);
    assert!(
        (items[0].rect.x - items[1].rect.x).abs() < 0.5 && (items[0].rect.w - items[1].rect.w).abs() < 0.5,
        "every entry fills the menu's width, so a short label is not centred: {:?} {:?}",
        items[0].rect,
        items[1].rect
    );
    assert!(sep.w >= items[1].rect.w - 0.5, "the line runs across the menu: {sep:?} against {:?}", items[1].rect);
    assert!(
        out.draw.items.iter().any(|i| matches!(&i.cmd, DrawCmd::Fill { rect, fill: Fill::Solid(c), .. }
            if rect.h == 1.0 && rect.w >= sep.w - 0.5 && same(*c, solid("#2f3639")))),
        "and is drawn"
    );
    // Labels start 10 px in, whatever their length.
    for it in items {
        let text =
            nodes.iter().skip_while(|n| n.key != it.key).find(|n| n.kind == ElementKind::Text).expect("a label").rect;
        assert!((text.x - (it.rect.x + 10.0)).abs() <= 1.0, "{:?} label at {} in {:?}", it.text, text.x, it.rect);
    }
}

fn has(n: &rsb_core::scene::NodeInfo, class: &str) -> bool {
    n.classes.iter().any(|c| &**c == class)
}

#[test]
fn the_one_line_accent_override_recolours_every_accent_use_in_the_dark_theme() {
    let t = Instant::now();
    let mut row = classed(boxed(vec![]), &["row", "current"]);
    row.interaction.focusable = true;
    let slider = classed(slider_el(SliderProps { value: 40.0, ..SliderProps::default() }), &["bare"]);
    let mut s = open_popup(&css(":root { --accent: #e5a50a; }"), ElementKind::Popup, vec![row, slider], t);
    s.focus_first(true, t);
    let out = frame(&mut s, t);
    let old = [solid("#81cbea")];
    let mut found_new = 0;
    for i in &out.draw.items {
        let colour = match &i.cmd {
            DrawCmd::Fill { fill: Fill::Solid(c), .. } => *c,
            DrawCmd::Border { color, .. } => *color,
            DrawCmd::Text { color, .. } => *color,
            _ => continue,
        };
        assert!(!old.iter().any(|o| same(colour, *o)), "{:?} is still the old accent", i.cmd);
        if same(colour, solid("#e5a50a")) {
            found_new += 1;
        }
    }
    assert!(found_new >= 2, "the slider fill and a ring use the new colour: {found_new}");
}

mod common;

use std::time::Instant;

use common::*;
use rsb_core::draw::{DrawCmd, DrawList, Fill};
use rsb_core::element::{Element, ElementKind, HandlerId};
use rsb_core::geom::PointF;
use rsb_core::hit::{Button, ElementEvent, PointerEvent, PointerKind};
use rsb_core::key::Key;
use rsb_core::popup::Side;
use rsb_core::scene::Scene;

const CSS: &str = "bar { font-size: 10px } popup { display: flex; flex-direction: column; width: 100px; background: #111 } \
                   .list { overflow-y: auto; height: 50px; width: 100px; display: flex; flex-direction: column } \
                   .row { height: 20px; width: 90px; background: #345678 } .outer { overflow-y: auto; height: 80px; display: flex; flex-direction: column }";

fn rows(n: usize) -> Vec<Element> {
    (0..n).map(|i| classed(focusable(boxed(vec![])), &["row", &format!("r{i}")])).collect()
}

fn list(content: Vec<Element>) -> Element {
    classed(boxed(content), &["list"])
}

fn make(content: Vec<Element>, t: Instant) -> Scene {
    let (bar, _) = bar_with_popup(CSS, open_props(1), content);
    let mut s = popup_scene(CSS, &decls(&bar)[0], Side::Below, 120, 120);
    s.set_open(true, t);
    s.set_window_focus(true, t);
    frame(&mut s, t);
    s
}

/// Drawn tops of the 20 px rows, in order.
fn row_tops(list: &DrawList) -> Vec<f32> {
    list.items
        .iter()
        .filter_map(|i| match &i.cmd {
            DrawCmd::Fill { rect, fill: Fill::Solid(c), .. } if rect.h == 20.0 && c.b > 0.4 && c.b < 0.5 => {
                Some(rect.y)
            }
            _ => None,
        })
        .collect()
}

fn first_row_y(s: &mut Scene, t: Instant) -> f32 {
    let out = frame(s, t);
    row_tops(&out.draw)[0]
}

fn wheel(s: &mut Scene, notches: f32, t: Instant) {
    let r = node_by_class(s, "list").rect;
    s.pointer(PointerEvent { kind: PointerKind::Wheel(notches), x: r.x + 10.0, y: r.y + 10.0 }, t);
}

#[test]
fn the_wheel_scrolls_smoothly_48_px_a_notch_in_120_ms_ease_out() {
    let t = Instant::now();
    let mut s = make(vec![list(rows(10))], t);
    let y0 = first_row_y(&mut s, t);
    wheel(&mut s, -1.0, t);
    let mut seen = Vec::new();
    for step in 0..=8 {
        seen.push(y0 - first_row_y(&mut s, t + ms(step * 16)));
    }
    // Monotonic, starts at 0, ends at 48 by 120 ms, ease-out: the first step covers more than the last.
    assert_eq!(seen[0], 0.0);
    assert!(seen.windows(2).all(|w| w[1] >= w[0]), "{seen:?}");
    assert_eq!(*seen.last().unwrap(), 48.0, "{seen:?}");
    assert!(seen[1] - seen[0] > seen[8] - seen[7], "{seen:?}");
    let mid = seen[3];
    assert!(mid > 20.0 && mid < 48.0, "48 ms in: {mid}");
    // A touchpad's fraction moves in proportion.
    wheel(&mut s, 0.5, t + ms(500));
    let y = first_row_y(&mut s, t + ms(700));
    assert_eq!(y0 - y, 24.0);
    // The frame after the end asks for nothing but the scrollbar's fade.
    let out = frame(&mut s, t + ms(800));
    assert!(out.next_frame.is_some());
}

#[test]
fn a_new_notch_retargets_from_where_it_is_and_the_offset_stops_at_the_end() {
    let t = Instant::now();
    let mut s = make(vec![list(rows(10))], t);
    let y0 = first_row_y(&mut s, t);
    wheel(&mut s, -1.0, t);
    frame(&mut s, t + ms(60));
    wheel(&mut s, -1.0, t + ms(60));
    let after = y0 - first_row_y(&mut s, t + ms(400));
    assert_eq!(after, 96.0);
    for k in 0..6 {
        wheel(&mut s, -1.0, t + ms(500 + k * 10));
    }
    // 200 px of rows in a 50 px box: 150 is the end.
    assert_eq!(y0 - first_row_y(&mut s, t + ms(900)), 150.0);
    wheel(&mut s, 3.0, t + ms(1000));
    assert_eq!(y0 - first_row_y(&mut s, t + ms(1200)), 6.0);
}

#[test]
fn nested_containers_the_inner_scrolls_first_then_the_outer_and_a_wheel_handler_last() {
    let t = Instant::now();
    let inner = classed(boxed(rows(4)), &["list"]);
    let rest = (0..3).map(|i| classed(boxed(vec![]), &["row", &format!("t{i}")])).collect::<Vec<_>>();
    let mut outer = classed(boxed([vec![inner], rest].concat()), &["outer"]);
    outer.handlers.wheel = Some(HandlerId(9));
    let mut s = make(vec![outer], t);
    let list = node_by_class(&s, "list").rect;
    let at = PointF { x: list.x + 5.0, y: list.y + 5.0 };
    let send = |s: &mut Scene, n: f32, t: Instant| {
        s.pointer(PointerEvent { kind: PointerKind::Wheel(n), x: at.x, y: at.y }, t)
    };
    let top = |s: &mut Scene, t: Instant| row_tops(&frame(s, t).draw)[0];
    let y0 = top(&mut s, t);
    // 4 rows (80 px) in a 50 px box: 30 px of range for the inner list.
    send(&mut s, -1.0, t);
    assert_eq!(y0 - top(&mut s, t + ms(200)), 30.0, "the inner list moved to its end");
    // The next notch goes to the outer container: 50 + 60 px of content in 80: 30 of range.
    let r = send(&mut s, -1.0, t + ms(300));
    assert!(r.calls.is_empty());
    assert_eq!(y0 - top(&mut s, t + ms(600)), 60.0, "the outer container took it: the inner row moved with it");
    // At the end of both the wheel handler under the pointer runs.
    let r = send(&mut s, -1.0, t + ms(700));
    assert_eq!(r.calls.len(), 1);
    assert_eq!(r.calls[0].handler, HandlerId(9));
    // Back up: the inner list is at its end, so it gives way first.
    send(&mut s, 1.0, t + ms(800));
    assert_eq!(y0 - top(&mut s, t + ms(1100)), 60.0 - 30.0 + 18.0 - 18.0, "the outer scrolls back up first");
}

#[test]
fn page_keys_home_end_and_arrows_scroll_a_focused_container_or_its_descendant() {
    let t = Instant::now();
    let mut container = list(rows(10));
    container.interaction.focusable = true;
    let mut s = make(vec![container], t);
    s.focus_first(true, t);
    let y0 = first_row_y(&mut s, t);
    s.key(key_down(Key::PageDown), t);
    // One viewport less 10 %: 45 px.
    assert_eq!(y0 - first_row_y(&mut s, t + ms(300)), 45.0);
    s.key(key_down(Key::End), t + ms(400));
    assert_eq!(y0 - first_row_y(&mut s, t + ms(700)), 150.0);
    s.key(key_down(Key::Home), t + ms(800));
    assert_eq!(first_row_y(&mut s, t + ms(1100)), y0);
    s.key(key_down(Key::ArrowDown), t + ms(1200));
    assert_eq!(
        y0 - first_row_y(&mut s, t + ms(1500)),
        48.0,
        "arrows move the container when it is the focused element"
    );
}

#[test]
fn focus_scrolls_the_element_into_view_with_an_8_px_margin() {
    let t = Instant::now();
    let mut s = make(vec![list(rows(10))], t);
    let y0 = first_row_y(&mut s, t);
    // Tab to the third row: its bottom (60) is below the 50 px view.
    for _ in 0..3 {
        s.key(key_down(Key::Tab), t);
    }
    frame(&mut s, t);
    // bottom 60 + 8 margin - 50 = 18.
    assert_eq!(y0 - first_row_y(&mut s, t + ms(400)), 18.0);
    // Back to the first row: top margin.
    for _ in 0..2 {
        s.key(key_mod(Key::Tab, false, true), t + ms(500));
    }
    frame(&mut s, t + ms(500));
    let y = first_row_y(&mut s, t + ms(900));
    assert_eq!(y, y0, "scrolled back to the start");
}

#[test]
fn a_reveal_seq_that_grows_scrolls_that_element_into_view() {
    let t = Instant::now();
    let mut target = classed(boxed(vec![]), &["row", "pick"]);
    target.interaction.reveal_seq = 0;
    let mut content = rows(8);
    content.push(target);
    let (bar, tree) = bar_with_popup(CSS, open_props(1), vec![list(content)]);
    let d = decls(&bar);
    let mut s = popup_scene(CSS, &d[0], Side::Below, 120, 120);
    s.set_open(true, t);
    frame(&mut s, t);
    let y0 = first_row_y(&mut s, t);
    let mut root = tree.root.clone().unwrap();
    root.children[0].children[0].children[0].children[8].interaction.reveal_seq = 1;
    s.set_popup_tree(
        std::sync::Arc::new(rsb_core::element::WidgetTree { version: 2, root: Some(root), ..(*tree).clone() }),
        d[0].path.clone(),
    );
    frame(&mut s, t + ms(10));
    // 9 rows = 180 px in a 50 px box: the end of the range is 130.
    frame(&mut s, t + ms(20));
    assert_eq!(y0 - first_row_y(&mut s, t + ms(300)), 130.0);
}

fn bar_alpha(list: &DrawList) -> Option<f32> {
    list.items.iter().find_map(|i| match &i.cmd {
        DrawCmd::Fill { rect, fill: Fill::Solid(c), radii, .. } if rect.w == 8.0 && radii.tl == 4.0 => Some(c.a),
        _ => None,
    })
}

#[test]
fn the_scrollbar_shows_while_hovered_and_for_800_ms_after_a_scroll_then_fades_over_200_ms() {
    let t = Instant::now();
    let mut s = make(vec![list(rows(10))], t);
    assert_eq!(bar_alpha(&frame(&mut s, t).draw), None, "not shown at rest");
    wheel(&mut s, -1.0, t + ms(1000));
    let thumb_alpha = bar_alpha(&frame(&mut s, t + ms(1000)).draw).expect("shown after a scroll");
    assert!((thumb_alpha - 0.35).abs() < 0.01, "default thumb: currentColor at 35%: {thumb_alpha}");
    assert_eq!(bar_alpha(&frame(&mut s, t + ms(1700)).draw), Some(thumb_alpha));
    let mid = bar_alpha(&frame(&mut s, t + ms(1900)).draw).unwrap();
    assert!((mid - thumb_alpha * 0.5).abs() < 0.02, "half way through the fade: {mid}");
    assert_eq!(bar_alpha(&frame(&mut s, t + ms(2100)).draw), None);
    // Hovering the container shows it and keeps it.
    let r = node_by_class(&s, "list").rect;
    s.pointer(PointerEvent { kind: PointerKind::Move, x: r.x + 5.0, y: r.y + 5.0 }, t + ms(3000));
    assert!(bar_alpha(&frame(&mut s, t + ms(3000)).draw).is_some());
    assert!(bar_alpha(&frame(&mut s, t + ms(9000)).draw).is_some());
    // The thumb is at least 24 px: 50 px view of 200 px would be 12.
    let out = frame(&mut s, t + ms(9000));
    let thumb = out.draw.items.iter().find_map(|i| match &i.cmd {
        DrawCmd::Fill { rect, radii, .. } if rect.w == 8.0 && radii.tl == 4.0 => Some(*rect),
        _ => None,
    });
    assert!(thumb.unwrap().h >= 24.0);
}

#[test]
fn dragging_the_thumb_scrolls_and_the_scrollbar_can_be_turned_off() {
    let t = Instant::now();
    let mut s = make(vec![list(rows(10))], t);
    let r = node_by_class(&s, "list").rect;
    s.pointer(PointerEvent { kind: PointerKind::Move, x: r.x + 5.0, y: r.y + 5.0 }, t);
    let out = frame(&mut s, t);
    let thumb = out
        .draw
        .items
        .iter()
        .find_map(|i| match &i.cmd {
            DrawCmd::Fill { rect, radii, .. } if rect.w == 8.0 && radii.tl == 4.0 => Some(*rect),
            _ => None,
        })
        .unwrap();
    let grab = PointF { x: thumb.x + 4.0, y: thumb.y + 2.0 };
    let y0 = row_tops(&out.draw)[0];
    s.pointer(PointerEvent { kind: PointerKind::Down(Button::Left), x: grab.x, y: grab.y }, t);
    // Track is 46 px (2 px inset each end), thumb 24: 22 px of travel for 150 px of range.
    let to = PointF { x: grab.x, y: grab.y + 11.0 };
    s.pointer(PointerEvent { kind: PointerKind::Move, x: to.x, y: to.y }, t);
    let moved = y0 - first_row_y(&mut s, t);
    assert_eq!(moved, 75.0, "half the travel, half the range");
    // The pointer may leave the container while the thumb is held.
    s.pointer(PointerEvent { kind: PointerKind::Move, x: 500.0, y: 900.0 }, t);
    assert_eq!(y0 - first_row_y(&mut s, t), 150.0);
    s.pointer(PointerEvent { kind: PointerKind::Up(Button::Left), x: 500.0, y: 900.0 }, t);
    // No scrollbar with scrollbar-width: none.
    let css = format!("{CSS} .list {{ scrollbar-width: none }}");
    let (bar, _) = bar_with_popup(&css, open_props(1), vec![list(rows(10))]);
    let mut s = popup_scene(&css, &decls(&bar)[0], Side::Below, 120, 120);
    s.set_open(true, t);
    frame(&mut s, t);
    wheel(&mut s, -1.0, t);
    assert_eq!(bar_alpha(&frame(&mut s, t + ms(50)).draw), None);
}

#[test]
fn on_scroll_reports_the_offset_at_most_30_times_a_second_and_the_final_one() {
    let t = Instant::now();
    let mut c = list(rows(10));
    c.handlers.scroll = Some(HandlerId(3));
    let mut s = make(vec![c], t);
    wheel(&mut s, -1.0, t);
    let mut reports = Vec::new();
    for step in 0..=10 {
        frame(&mut s, t + ms(step * 16));
        for e in s.take_events() {
            if let ElementEvent::Scrolled { x, y } = e.event {
                reports.push((step * 16, x, y));
            }
        }
    }
    assert!(reports.len() >= 2 && reports.len() <= 5, "{reports:?}");
    assert_eq!(reports.last().unwrap().2, 48.0, "the final offset is always sent");
    assert!(reports.windows(2).all(|w| w[1].0 - w[0].0 >= 33 || w[1].2 == 48.0));
    let _ = ElementKind::Scroll;
}

#[test]
fn hit_testing_and_clicks_follow_the_scrolled_content_and_the_clip() {
    let t = Instant::now();
    let mut content = rows(10);
    for (i, r) in content.iter_mut().enumerate() {
        r.handlers.click = Some(HandlerId(100 + i as u32));
    }
    let mut s = make(vec![list(content)], t);
    let r = node_by_class(&s, "list").rect;
    let click = |s: &mut Scene, y: f32, at: Instant| {
        let p = PointF { x: r.x + 5.0, y: r.y + y };
        s.pointer(PointerEvent { kind: PointerKind::Down(Button::Left), x: p.x, y: p.y }, at);
        s.pointer(PointerEvent { kind: PointerKind::Up(Button::Left), x: p.x, y: p.y }, at)
    };
    assert_eq!(click(&mut s, 5.0, t).calls[0].handler, HandlerId(100));
    wheel(&mut s, -1.0, t + ms(100));
    frame(&mut s, t + ms(100));
    frame(&mut s, t + ms(400));
    // Scrolled 48: the top of the box shows row 2 (content y 48 is inside 40..60).
    let call = click(&mut s, 5.0, t + ms(400)).calls;
    assert_eq!(call[0].handler, HandlerId(102));
    assert_eq!(call[0].y, 13.0, "relative to the row, not the box");
    // Below the box nothing is hit, though the row exists in the content.
    let below = PointF { x: r.x + 5.0, y: r.y + 70.0 };
    let res = s.pointer(PointerEvent { kind: PointerKind::Down(Button::Left), x: below.x, y: below.y }, t + ms(500));
    assert!(res.missed || res.calls.is_empty());
}

#[test]
fn scrollbar_colour_and_the_anchor_of_a_popup_inside_a_scrolled_list() {
    let t = Instant::now();
    let css = format!("{CSS} .list {{ scrollbar-color: #ff0000 #0000ff }}");
    let (bar, _) = bar_with_popup(&css, open_props(1), vec![list(rows(10))]);
    let mut s = popup_scene(&css, &decls(&bar)[0], Side::Below, 120, 120);
    s.set_open(true, t);
    frame(&mut s, t);
    wheel(&mut s, -1.0, t);
    let out = frame(&mut s, t);
    let fills: Vec<_> = out
        .draw
        .items
        .iter()
        .filter_map(|i| match &i.cmd {
            DrawCmd::Fill { rect, fill: Fill::Solid(c), .. } if rect.w == 8.0 => Some((c.r, c.b)),
            _ => None,
        })
        .collect();
    assert_eq!(fills, [(0.0, 1.0), (1.0, 0.0)], "track then thumb");
    // A popup declared by a row of a scrolled list is anchored where the row is drawn.
    let pop = popup_el(open_props(1), ElementKind::Popup, vec![]);
    let mut anchor_row = classed(boxed(vec![pop]), &["row", "anchor_row"]);
    anchor_row.interaction.focusable = false;
    let mut content = rows(5);
    content.push(anchor_row);
    let inner = classed(boxed(content), &["list"]);
    let mut s = scene_with(&format!("{CSS} .root {{ padding: 0 }}"), 300, 1.0, Some(inner), None, None);
    frame(&mut s, t);
    let before = s.popups()[0].anchor_rect;
    // The list is taller than the bar: the pointer goes where the bar is.
    s.pointer(PointerEvent { kind: PointerKind::Wheel(-1.0), x: 5.0, y: 10.0 }, t);
    frame(&mut s, t);
    frame(&mut s, t + ms(300));
    let after = s.popups()[0].anchor_rect;
    assert_eq!(before.y - after.y, 48.0);
}

#[test]
fn the_sideways_wheel_scrolls_along_x_and_leaves_a_vertical_only_list_alone() {
    let t = Instant::now();
    let css = format!(
        "{CSS} .hl {{ overflow-x: auto; width: 100px; height: 30px; display: flex; flex-direction: row }} \
         .cell {{ width: 40px; height: 20px; flex-shrink: 0; background: #345678 }}"
    );
    let cells = (0..5).map(|i| classed(boxed(vec![]), &["cell", &format!("c{i}")])).collect::<Vec<_>>();
    let (bar, _) = bar_with_popup(&css, open_props(1), vec![classed(boxed(cells), &["hl"]), list(rows(10))]);
    let mut s = popup_scene(&css, &decls(&bar)[0], Side::Below, 120, 120);
    s.set_open(true, t);
    frame(&mut s, t);
    let first_x = |s: &mut Scene, t: Instant| {
        frame(s, t)
            .draw
            .items
            .iter()
            .find_map(|i| match &i.cmd {
                DrawCmd::Fill { rect, fill: Fill::Solid(c), .. }
                    if rect.h == 20.0 && rect.w == 40.0 && c.b > 0.4 && c.b < 0.5 =>
                {
                    Some(rect.x)
                }
                _ => None,
            })
            .expect("a cell is drawn")
    };
    let x0 = first_x(&mut s, t);
    let hl = node_by_class(&s, "hl").rect;
    s.pointer(PointerEvent { kind: PointerKind::HWheel(1.0), x: hl.x + 10.0, y: hl.y + 10.0 }, t);
    assert_eq!(first_x(&mut s, t + ms(300)), x0 - 48.0, "one notch right moves the content 48 px left");
    s.pointer(PointerEvent { kind: PointerKind::HWheel(-5.0), x: hl.x + 10.0, y: hl.y + 10.0 }, t + ms(400));
    assert_eq!(first_x(&mut s, t + ms(700)), x0, "and back to the start, no further");
    // A list that scrolls only vertically ignores the sideways wheel.
    let l = node_by_class(&s, "list").rect;
    let y0 = first_row_y(&mut s, t + ms(800));
    s.pointer(PointerEvent { kind: PointerKind::HWheel(1.0), x: l.x + 10.0, y: l.y + 10.0 }, t + ms(800));
    assert_eq!(first_row_y(&mut s, t + ms(1100)), y0);
}

#[test]
fn a_sideways_wheel_over_a_widget_that_cannot_scroll_sideways_reaches_its_wheel_handler() {
    let t = Instant::now();
    let mut over = classed(boxed(rows(1)), &["over"]);
    over.handlers.wheel = Some(HandlerId(7));
    let mut s = make(vec![over], t);
    let r = node_by_class(&s, "over").rect;
    let at = |kind| PointerEvent { kind, x: r.x + 5.0, y: r.y + 5.0 };
    // Shift with the wheel down arrives as a sideways wheel to the right (+1); the handler hears a wheel down.
    let res = s.pointer(at(PointerKind::HWheel(1.0)), t);
    assert_eq!(res.calls.len(), 1);
    assert_eq!(res.calls[0].handler, HandlerId(7));
    assert!(matches!(res.calls[0].kind, rsb_core::hit::InputKind::Wheel(n) if n == -1.0), "{:?}", res.calls[0].kind);
    // A plain wheel still reaches it with the notches as they came.
    let res = s.pointer(at(PointerKind::Wheel(2.0)), t);
    assert!(matches!(res.calls[0].kind, rsb_core::hit::InputKind::Wheel(n) if n == 2.0));
}

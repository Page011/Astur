//! Keyframe animations, size transitions to and from `auto`, and transforms,
//! frame by frame.

mod common;

use std::sync::Arc;
use std::time::Instant;

use common::*;
use rsb_core::anim::Easing;
use rsb_core::draw::{DrawCmd, DrawItem};
use rsb_core::element::{AnimateSpec, Element, HandlerId, InstanceId};
use rsb_core::geom::{PointF, RectF};
use rsb_core::hit::{Button, PointerEvent, PointerKind};
use rsb_core::layout::{FixedMeasure, TextMeasure};
use rsb_core::scene::{BarGeometry, Scene};

fn at(t0: Instant, n: u64) -> Instant {
    t0 + ms(n)
}

fn opacity(s: &Scene, class: &str) -> f32 {
    node_by_class(s, class).style.opacity
}

fn width(s: &Scene, class: &str) -> f32 {
    node_by_class(s, class).rect.w
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

fn one(s: &mut Scene, e: Element, version: u64) {
    s.set_tree(tree(1, version, Some(e)));
}

// ---- keyframes ----------------------------------------------------------

const FADE: &str = "@keyframes fade { from { opacity: 0 } to { opacity: 1 } } .a { width: 10px; height: 10px; ";

fn animated(css: &str) -> (Scene, Instant) {
    let t0 = Instant::now();
    let mut s = scene_with(css, 200, 1.0, Some(classed(boxed(vec![]), &["a"])), None, None);
    frame(&mut s, t0);
    (s, t0)
}

#[test]
fn an_animation_runs_from_the_first_frame_and_holds_its_end_with_forwards() {
    let (mut s, t0) = animated(&format!("{FADE} animation: fade 100ms linear forwards }}"));
    assert!(near(opacity(&s, "a"), 0.0));
    assert!(!s.is_settled());
    for (ms_, want) in [(25, 0.25), (50, 0.5), (75, 0.75)] {
        let f = frame(&mut s, at(t0, ms_));
        assert!(near(opacity(&s, "a"), want), "{ms_} ms: {}", opacity(&s, "a"));
        assert!(f.next_frame.is_some(), "frames keep coming while it runs");
    }
    let f = frame(&mut s, at(t0, 100));
    assert!(near(opacity(&s, "a"), 1.0));
    assert!(f.next_frame.is_none(), "an ended animation asks for no frames");
    frame(&mut s, at(t0, 5000));
    assert!(near(opacity(&s, "a"), 1.0), "forwards keeps the last frame");
    assert!(s.is_settled());
}

#[test]
fn fill_modes_decide_what_shows_before_and_after() {
    // No fill: the element's own value outside the active time.
    let (mut s, t0) = animated(&format!("{FADE} opacity: 0.8; animation: fade 100ms linear 100ms }}"));
    assert!(near(opacity(&s, "a"), 0.8), "before the delay");
    frame(&mut s, at(t0, 150));
    assert!(near(opacity(&s, "a"), 0.5));
    frame(&mut s, at(t0, 250));
    assert!(near(opacity(&s, "a"), 0.8), "after the end");
    // Backwards: the first frame holds during the delay, not after.
    let (mut s, t0) = animated(&format!("{FADE} opacity: 0.8; animation: fade 100ms linear 100ms backwards }}"));
    assert!(near(opacity(&s, "a"), 0.0));
    frame(&mut s, at(t0, 99));
    assert!(near(opacity(&s, "a"), 0.0));
    frame(&mut s, at(t0, 250));
    assert!(near(opacity(&s, "a"), 0.8));
    // Both: first frame before, last frame after.
    let (mut s, t0) = animated(&format!("{FADE} opacity: 0.8; animation: fade 100ms linear 100ms both }}"));
    assert!(near(opacity(&s, "a"), 0.0));
    frame(&mut s, at(t0, 400));
    assert!(near(opacity(&s, "a"), 1.0));
}

#[test]
fn direction_and_iteration_count_follow_the_spec() {
    let (mut s, t0) = animated(&format!("{FADE} animation: fade 100ms linear 3 alternate forwards }}"));
    // Pass 1 forwards, pass 2 backwards, pass 3 forwards again.
    for (ms_, want) in [(50, 0.5), (125, 0.75), (150, 0.5), (225, 0.25), (250, 0.5), (275, 0.75)] {
        frame(&mut s, at(t0, ms_));
        assert!(near(opacity(&s, "a"), want), "{ms_} ms: {}", opacity(&s, "a"));
    }
    frame(&mut s, at(t0, 400));
    assert!(near(opacity(&s, "a"), 1.0), "the third pass ends on its last frame");
    assert!(s.is_settled());
    let (mut s, t0) = animated(&format!("{FADE} animation: fade 100ms linear reverse forwards }}"));
    assert!(near(opacity(&s, "a"), 1.0));
    frame(&mut s, at(t0, 25));
    assert!(near(opacity(&s, "a"), 0.75));
    frame(&mut s, at(t0, 200));
    assert!(near(opacity(&s, "a"), 0.0), "reverse ends at the first keyframe");
    let (mut s, t0) = animated(&format!("{FADE} animation: fade 100ms linear infinite }}"));
    for k in 1..20u64 {
        let f = frame(&mut s, at(t0, 130 * k));
        assert!(f.next_frame.is_some(), "an infinite animation never settles");
    }
    assert!(!s.is_settled());
}

#[test]
fn a_negative_delay_starts_part_way_in() {
    let (mut s, t0) = animated(&format!("{FADE} animation: fade 100ms linear -40ms forwards }}"));
    assert!(near(opacity(&s, "a"), 0.4), "{}", opacity(&s, "a"));
    frame(&mut s, at(t0, 30));
    assert!(near(opacity(&s, "a"), 0.7));
    frame(&mut s, at(t0, 60));
    assert!(near(opacity(&s, "a"), 1.0));
    assert!(s.is_settled());
}

#[test]
fn keyframes_with_stops_and_their_own_easing() {
    let css = "@keyframes k { from { opacity: 0 } 50% { opacity: 1; animation-timing-function: steps(2, end) } to { opacity: 0 } } \
               .a { width: 10px; height: 10px; animation: k 100ms linear forwards }";
    let (mut s, t0) = animated(css);
    frame(&mut s, at(t0, 25));
    assert!(near(opacity(&s, "a"), 0.5), "linear to the middle stop");
    frame(&mut s, at(t0, 60));
    assert!(near(opacity(&s, "a"), 1.0), "steps(2, end) holds the value for the first half of the second span");
    frame(&mut s, at(t0, 80));
    assert!(near(opacity(&s, "a"), 0.5), "then jumps to the middle of the way to the end");
}

#[test]
fn a_missing_first_or_last_keyframe_uses_the_elements_own_value() {
    let css = "@keyframes k { 50% { opacity: 1 } } .a { width: 10px; height: 10px; opacity: 0.2; animation: k 100ms linear forwards }";
    let (mut s, t0) = animated(css);
    assert!(near(opacity(&s, "a"), 0.2));
    frame(&mut s, at(t0, 50));
    assert!(near(opacity(&s, "a"), 1.0));
    frame(&mut s, at(t0, 75));
    assert!(near(opacity(&s, "a"), 0.6));
    frame(&mut s, at(t0, 100));
    assert!(near(opacity(&s, "a"), 0.2));
}

#[test]
fn an_animation_starts_when_the_name_appears_and_again_on_restart_seq() {
    let t0 = Instant::now();
    let css = "@keyframes fade { from { opacity: 0 } to { opacity: 1 } } .a { width: 10px; height: 10px } .on { animation: fade 100ms linear forwards }";
    let mut s = scene_with(css, 200, 1.0, Some(classed(boxed(vec![]), &["a"])), None, None);
    frame(&mut s, t0);
    assert!(near(opacity(&s, "a"), 1.0), "no animation-name yet");
    // A class change gives it the name: it starts then.
    one(&mut s, classed(boxed(vec![]), &["a", "on"]), 2);
    frame(&mut s, at(t0, 1000));
    assert!(near(opacity(&s, "a"), 0.0));
    frame(&mut s, at(t0, 1050));
    assert!(near(opacity(&s, "a"), 0.5));
    // An unrelated tree update does not restart it.
    one(&mut s, classed(boxed(vec![]), &["a", "on"]), 3);
    frame(&mut s, at(t0, 1075));
    assert!(near(opacity(&s, "a"), 0.75));
    frame(&mut s, at(t0, 1200));
    assert!(near(opacity(&s, "a"), 1.0));

    // The `animate` prop: restart_seq growing starts it from the first keyframe.
    let spec = |seq: u64| AnimateSpec {
        name: Arc::from("fade"),
        duration: Some(Arc::from("100ms")),
        easing: Some(Arc::from("linear")),
        delay: None,
        iterations: None,
        direction: None,
        fill: Some(Arc::from("forwards")),
        restart_seq: seq,
    };
    let mut e = classed(boxed(vec![]), &["a"]);
    // The name from the class is gone for a frame, so its run is forgotten.
    one(&mut s, classed(boxed(vec![]), &["a"]), 3);
    frame(&mut s, at(t0, 1500));
    e.animate = Some(Box::new(spec(0)));
    one(&mut s, e.clone(), 4);
    frame(&mut s, at(t0, 2000));
    assert!(near(opacity(&s, "a"), 0.0), "a new name starts");
    frame(&mut s, at(t0, 2100));
    assert!(near(opacity(&s, "a"), 1.0));
    e.animate = Some(Box::new(spec(1)));
    one(&mut s, e.clone(), 5);
    frame(&mut s, at(t0, 3000));
    assert!(near(opacity(&s, "a"), 0.0), "restart_seq grew: from the start again");
    frame(&mut s, at(t0, 3040));
    assert!(near(opacity(&s, "a"), 0.4));
    // The same seq again changes nothing.
    one(&mut s, e, 6);
    frame(&mut s, at(t0, 3060));
    assert!(near(opacity(&s, "a"), 0.6));
}

#[test]
fn a_paused_animation_holds_and_resumes_where_it_stopped() {
    let t0 = Instant::now();
    let css = "@keyframes fade { from { opacity: 0 } to { opacity: 1 } } .a { width: 10px; height: 10px; animation: fade 100ms linear forwards } .p { animation-play-state: paused }";
    let mut s = scene_with(css, 200, 1.0, Some(classed(boxed(vec![]), &["a"])), None, None);
    frame(&mut s, t0);
    frame(&mut s, at(t0, 40));
    one(&mut s, classed(boxed(vec![]), &["a", "p"]), 2);
    frame(&mut s, at(t0, 40));
    let f = frame(&mut s, at(t0, 500));
    assert!(near(opacity(&s, "a"), 0.4), "{}", opacity(&s, "a"));
    assert!(f.next_frame.is_none(), "paused needs no frames");
    one(&mut s, classed(boxed(vec![]), &["a"]), 3);
    frame(&mut s, at(t0, 500));
    frame(&mut s, at(t0, 530));
    assert!(near(opacity(&s, "a"), 0.7), "{}", opacity(&s, "a"));
}

// ---- size transitions -----------------------------------------------------

/// The date: an auto-width box that clips and animates its width, around a
/// content box whose own width changes.
const DATE: &str = ".date { overflow: hidden; transition: width 200ms ease-out; height: 10px } \
                    .short { width: 77px; height: 10px } .long { width: 262px; height: 10px } .hidden { display: none }";

fn date(inner: &str) -> Element {
    classed(boxed(vec![classed(boxed(vec![]), &[inner])]), &["date"])
}

#[test]
fn the_date_widens_from_77_to_262_px_over_200_ms() {
    let t0 = Instant::now();
    let mut s = scene_with(DATE, 600, 1.0, Some(date("short")), None, None);
    frame(&mut s, t0);
    assert_eq!(width(&s, "date"), 77.0, "the first layout does not animate");
    assert!(s.is_settled());
    one(&mut s, date("long"), 2);
    let t1 = at(t0, 1000);
    let f = frame(&mut s, t1);
    assert_eq!(width(&s, "date"), 77.0, "starts from what was drawn");
    assert!(f.next_frame.is_some());
    let mut seen = Vec::new();
    for ms_ in [20, 40, 60, 80, 100, 120, 140, 160, 180] {
        frame(&mut s, at(t1, ms_));
        let w = width(&s, "date");
        let want = 77.0 + 185.0 * Easing::EaseOut.apply(ms_ as f32 / 200.0);
        assert!((w - want).abs() <= 0.5, "{ms_} ms: {w} vs {want}");
        seen.push(w);
    }
    assert!(seen.windows(2).all(|p| p[1] >= p[0]), "monotonic: {seen:?}");
    let f = frame(&mut s, at(t1, 200));
    assert_eq!(width(&s, "date"), 262.0);
    assert!(f.next_frame.is_none() && s.is_settled());
    // The children are laid out inside the interpolated box and clipped to it.
    one(&mut s, date("short"), 3);
    let t2 = at(t0, 2000);
    frame(&mut s, t2);
    let f = frame(&mut s, at(t2, 100));
    let w = width(&s, "date");
    assert!(w < 262.0 && w > 77.0, "{w}");
    let pushes: Vec<&DrawItem> = f.draw.items.iter().filter(|i| matches!(i.cmd, DrawCmd::PushClip { .. })).collect();
    assert!(
        pushes.iter().any(|i| matches!(&i.cmd, DrawCmd::PushClip { rect, .. } if near(rect.w, w))),
        "clip follows the box"
    );
}

#[test]
fn an_interrupted_size_change_continues_from_the_drawn_width() {
    let t0 = Instant::now();
    let mut s = scene_with(DATE, 600, 1.0, Some(date("short")), None, None);
    frame(&mut s, t0);
    one(&mut s, date("long"), 2);
    let t1 = at(t0, 1000);
    frame(&mut s, t1);
    frame(&mut s, at(t1, 100));
    let mid = width(&s, "date");
    assert!(mid > 77.0 && mid < 262.0);
    // Back to the short one: the new run starts at `mid`, not at 262 or 77.
    one(&mut s, date("short"), 3);
    let t2 = at(t1, 100);
    frame(&mut s, t2);
    assert!(near(width(&s, "date"), mid), "{} vs {mid}", width(&s, "date"));
    frame(&mut s, at(t2, 100));
    let w = width(&s, "date");
    let want = mid + (77.0 - mid) * Easing::EaseOut.apply(0.5);
    assert!((w - want).abs() <= 0.5, "{w} vs {want}");
    frame(&mut s, at(t2, 200));
    assert_eq!(width(&s, "date"), 77.0);
    assert!(s.is_settled());
}

#[test]
fn a_shown_box_grows_from_nothing_when_it_has_a_width_transition() {
    let t0 = Instant::now();
    let mut s = scene_with(
        DATE,
        600,
        1.0,
        Some(classed(boxed(vec![classed(boxed(vec![]), &["long"])]), &["date", "hidden"])),
        None,
        None,
    );
    frame(&mut s, t0);
    assert_eq!(width(&s, "date"), 0.0);
    one(&mut s, date("long"), 2);
    let t1 = at(t0, 500);
    frame(&mut s, t1);
    assert_eq!(width(&s, "date"), 0.0, "starts from 0");
    frame(&mut s, at(t1, 100));
    let w = width(&s, "date");
    let want = 262.0 * Easing::EaseOut.apply(0.5);
    assert!((w - want).abs() <= 0.5, "{w} vs {want}");
    frame(&mut s, at(t1, 200));
    assert_eq!(width(&s, "date"), 262.0);
}

#[test]
fn a_box_without_a_transition_resizes_at_once() {
    let t0 = Instant::now();
    let css = ".date { overflow: hidden; height: 10px } .short { width: 77px; height: 10px } .long { width: 262px; height: 10px }";
    let mut s = scene_with(css, 600, 1.0, Some(date("short")), None, None);
    frame(&mut s, t0);
    one(&mut s, date("long"), 2);
    let f = frame(&mut s, at(t0, 10));
    assert_eq!(width(&s, "date"), 262.0);
    assert!(f.next_frame.is_none());
}

#[test]
fn nothing_slides_after_a_scale_change_a_new_sheet_or_a_font() {
    let t0 = Instant::now();
    let mut s = scene_with(DATE, 1200, 1.0, Some(date("short")), None, None);
    frame(&mut s, t0);
    assert_eq!(width(&s, "date"), 77.0);
    // A DPI change doubles every size: no transition runs.
    s.set_geometry(BarGeometry { width_px: 1200, height_px: 64, scale: 2.0 });
    let f = frame(&mut s, at(t0, 10));
    assert_eq!(width(&s, "date"), 154.0);
    assert!(f.next_frame.is_none() && s.is_settled());
    // A reload with a new sheet that changes the width.
    s.set_stylesheet(sheet(&DATE.replace("77px", "100px")));
    let f = frame(&mut s, at(t0, 20));
    assert_eq!(width(&s, "date"), 200.0);
    assert!(f.next_frame.is_none() && s.is_settled());
}

/// A measurer whose letters can be made wider, like a font that finished loading.
struct Wide(f32);

impl TextMeasure for Wide {
    fn measure(&mut self, text: &str, style: &rsb_core::draw::TextStyle, w: Option<f32>) -> rsb_core::geom::SizeF {
        let mut m = FixedMeasure.measure(text, style, w);
        m.w *= self.0;
        m
    }
}

#[test]
fn registering_a_font_snaps_but_a_text_change_animates() {
    let t0 = Instant::now();
    let css = ".t { transition: width 200ms linear; font-size: 10px; height: 10px }";
    let word = |s: &str| classed(boxed(vec![text(s)]), &["t"]);
    let mut s = scene_with(css, 600, 1.0, Some(word("abcdefghij")), None, None);
    s.frame(t0, &mut Wide(1.0));
    assert_eq!(width(&s, "t"), 60.0);
    // The font arrives: the text measures wider, the box takes it at once.
    s.fonts_changed();
    let f = s.frame(at(t0, 10), &mut Wide(2.0));
    assert_eq!(width(&s, "t"), 120.0);
    assert!(f.next_frame.is_none() && s.is_settled());
    // The same change through new text animates.
    s.set_tree(tree(1, 2, Some(word("abcdefghijklmnopqrst"))));
    s.frame(at(t0, 1000), &mut Wide(2.0));
    assert_eq!(width(&s, "t"), 120.0);
    s.frame(at(t0, 1100), &mut Wide(2.0));
    assert!(near(width(&s, "t"), 180.0), "{}", width(&s, "t"));
    s.frame(at(t0, 1200), &mut Wide(2.0));
    assert_eq!(width(&s, "t"), 240.0);
}

#[test]
fn a_keyframe_animation_on_width_holds_the_layout_to_its_value() {
    let t0 = Instant::now();
    let css = "@keyframes grow { from { width: 10px } to { width: 50px } } .a { width: 10px; height: 10px; animation: grow 100ms linear forwards }";
    let mut s = scene_with(css, 200, 1.0, Some(classed(boxed(vec![]), &["a"])), None, None);
    frame(&mut s, t0);
    assert_eq!(width(&s, "a"), 10.0);
    frame(&mut s, at(t0, 50));
    assert_eq!(width(&s, "a"), 30.0);
    frame(&mut s, at(t0, 100));
    assert_eq!(width(&s, "a"), 50.0);
    // The target layout (where popups are placed) is the style's, not the animation's.
    let key = node_by_class(&s, "a").key;
    frame(&mut s, at(t0, 200));
    assert_eq!(s.target_rect_of(key).map(|r| r.w), Some(10.0));
}

#[test]
fn the_target_rect_is_where_a_widening_box_is_going() {
    let t0 = Instant::now();
    let mut s = scene_with(DATE, 600, 1.0, Some(date("short")), None, None);
    frame(&mut s, t0);
    one(&mut s, date("long"), 2);
    let t1 = at(t0, 1000);
    frame(&mut s, t1);
    frame(&mut s, at(t1, 100));
    let key = node_by_class(&s, "date").key;
    assert!(width(&s, "date") < 262.0);
    assert_eq!(s.target_rect_of(key).map(|r| r.w), Some(262.0));
}

// ---- transforms ------------------------------------------------------------

const XF: &str = "bar { font-size: 10px } .t { width: 40px; height: 20px; background: #ff0000 } \
                  .moved { transform: translateX(10px) scale(2) } .about { transform: scale(0.5); transform-origin: left top }";

fn fill_of(items: &[DrawItem]) -> RectF {
    items
        .iter()
        .find_map(|i| match &i.cmd {
            DrawCmd::Fill { rect, fill: rsb_core::draw::Fill::Solid(c), .. } if c.r > 0.9 && c.g < 0.1 => Some(*rect),
            _ => None,
        })
        .expect("a red fill")
}

fn scene_xf(class: &str) -> Scene {
    let mut e = classed(boxed(vec![]), &["t", class]);
    e.handlers.click = Some(HandlerId(1));
    let mut s = scene_with(XF, 300, 1.0, Some(e), None, None);
    frame(&mut s, Instant::now());
    s
}

#[test]
fn transforms_map_the_drawn_box_about_its_origin() {
    let mut s = scene_xf("moved");
    let r = node_by_class(&s, "t").rect;
    assert_eq!((r.w, r.h), (40.0, 20.0), "layout is not affected");
    let drawn = fill_of(&frame(&mut s, Instant::now()).draw.items);
    // scale(2) about the centre, then translateX(10px): centre moves right by 10.
    let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    assert_eq!(drawn, RectF::new(cx + 10.0 - 40.0, cy - 20.0, 80.0, 40.0));
    let s2 = scene_xf("about");
    let r = node_by_class(&s2, "t").rect;
    let mut s2 = s2;
    let drawn = fill_of(&frame(&mut s2, Instant::now()).draw.items);
    assert_eq!(drawn, RectF::new(r.x, r.y, 20.0, 10.0), "left top origin: the corner stays");
}

#[test]
fn hit_testing_maps_the_point_back_through_the_transform() {
    let t = Instant::now();
    let mut s = scene_xf("moved");
    let r = node_by_class(&s, "t").rect;
    let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let down = |s: &mut Scene, p: PointF| {
        s.pointer(PointerEvent { kind: PointerKind::Down(Button::Left), x: p.x, y: p.y }, t);
        s.pointer(PointerEvent { kind: PointerKind::Up(Button::Left), x: p.x, y: p.y }, t)
    };
    // Inside the drawn (bigger, shifted) box but outside the layout box.
    let r1 = down(&mut s, PointF { x: cx + 10.0 + 35.0, y: cy });
    assert_eq!(r1.calls.len(), 1, "{r1:?}");
    // The drawn box is [-10, 70]: right of it is empty again, though the layout box ended at 40.
    let r2 = down(&mut s, PointF { x: r.x + 75.0, y: cy });
    assert!(r2.calls.is_empty() && r2.unhandled.is_some(), "{r2:?}");
    // The handler gets the point in the element's own, untransformed, px: 45 of 40.
    assert!((r1.calls[0].x - 37.5).abs() < 0.01 && (r1.calls[0].y - 10.0).abs() < 0.01, "{:?}", r1.calls[0]);
    // A shrunk box leaves its layout corner empty.
    let mut s = scene_xf("about");
    let r = node_by_class(&s, "t").rect;
    let r3 = down(&mut s, PointF { x: r.x + 30.0, y: r.y + 5.0 });
    assert!(r3.calls.is_empty(), "{r3:?}");
}

#[test]
fn a_zero_scale_is_never_hit_and_damage_follows_the_mapped_bounds() {
    let t0 = Instant::now();
    let css = format!("{XF} .gone {{ transform: scale(0) }}");
    let mut e = classed(boxed(vec![]), &["t"]);
    e.handlers.click = Some(HandlerId(1));
    let mut s = scene_with(&css, 300, 1.0, Some(e.clone()), None, None);
    frame(&mut s, t0);
    // Moving the element by a class: damage is the old and the new mapped bounds.
    let r = node_by_class(&s, "t").rect;
    let mut moved = e.clone();
    moved.classes = vec![Arc::from("t"), Arc::from("moved")];
    one(&mut s, moved, 2);
    let f = frame(&mut s, at(t0, 10));
    let d = f.dirty.expect("something changed");
    let drawn = (r.x + 10.0 - 20.0, r.y - 10.0, r.x + 10.0 + 60.0, r.y + 30.0);
    // The surface cuts what is left of 0 and above 0.
    assert!(d.x0 as f32 <= drawn.0.max(0.0) && d.x1 as f32 >= drawn.2.max(r.right()), "{d:?} vs {drawn:?}");
    assert!(d.y0 as f32 <= drawn.1.max(0.0) && d.y1 as f32 >= drawn.3.min(32.0), "{d:?}");
    // scale(0): nothing to hit, and nothing visible.
    let mut gone = e;
    gone.classes = vec![Arc::from("t"), Arc::from("gone")];
    one(&mut s, gone, 3);
    frame(&mut s, at(t0, 20));
    let p = PointF { x: r.x + 20.0, y: r.y + 10.0 };
    s.pointer(PointerEvent { kind: PointerKind::Down(Button::Left), x: p.x, y: p.y }, t0);
    let res = s.pointer(PointerEvent { kind: PointerKind::Up(Button::Left), x: p.x, y: p.y }, t0);
    assert!(res.calls.is_empty());
}

#[test]
fn a_transform_transitions_like_any_other_value() {
    let t0 = Instant::now();
    let css = ".t { width: 40px; height: 20px; background: #ff0000; transition: transform 100ms linear } .moved { transform: translateX(20px) scale(2) }";
    let mut s = scene_with(css, 300, 1.0, Some(classed(boxed(vec![]), &["t"])), None, None);
    frame(&mut s, t0);
    let r = node_by_class(&s, "t").rect;
    one(&mut s, classed(boxed(vec![]), &["t", "moved"]), 2);
    let t1 = at(t0, 500);
    frame(&mut s, t1);
    let f = frame(&mut s, at(t1, 50));
    let d = fill_of(&f.draw.items);
    // Half way: scale 1.5, shifted 10.
    assert!(near(d.w, 60.0) && near(d.x, r.x + 10.0 - 10.0), "{d:?} for {r:?}");
}

#[test]
fn the_instance_id_is_kept_in_handler_calls_of_transformed_elements() {
    let t = Instant::now();
    let mut s = scene_xf("about");
    let r = node_by_class(&s, "t").rect;
    let p = PointF { x: r.x + 5.0, y: r.y + 5.0 };
    s.pointer(PointerEvent { kind: PointerKind::Down(Button::Left), x: p.x, y: p.y }, t);
    let res = s.pointer(PointerEvent { kind: PointerKind::Up(Button::Left), x: p.x, y: p.y }, t);
    assert_eq!(res.calls.len(), 1);
    assert_eq!(res.calls[0].instance, InstanceId(1));
    // 5 px into the drawn (half size) box is 10 px into the element.
    assert!((res.calls[0].x - 10.0).abs() < 0.01 && (res.calls[0].y - 10.0).abs() < 0.01, "{:?}", res.calls[0]);
}

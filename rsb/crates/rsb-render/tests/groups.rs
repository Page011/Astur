//! Clips and opacity groups: what they cost, what they keep between frames,
//! and that a buffer reused for a later group never shows an earlier one.
//! Timings print in every build and are asserted in a release build only
//! (`cargo test --release -p rsb-render --test groups -- --nocapture`).

mod common;

use std::sync::Arc;
use std::time::Instant;

use common::*;
use rsb_core::draw::{DrawCmd, DrawItem, DrawList};
use rsb_core::geom::{PathF, PointF, RectF, RectPx};
use rsb_render::{Canvas, FontConfig, Renderer};

const BUDGET_MS: f64 = 50.0;
const SCRATCH_BUDGET: usize = 100 << 20;

fn build() -> &'static str {
    if cfg!(debug_assertions) { "debug build" } else { "release build" }
}

/// Renders `l` three times on a fresh renderer and returns the median ms and the scratch bytes kept.
fn cost(l: &DrawList) -> (f64, usize) {
    let mut r = Renderer::new(FontConfig::default());
    let mut canvas = Canvas::new(l.width, l.height);
    let mut times = Vec::new();
    for _ in 0..3 {
        let t = Instant::now();
        r.render(l, &mut canvas.pixels(), None).unwrap();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (times[1], r.stats().scratch_bytes)
}

fn report_only(what: &str, ms: f64, bytes: usize) {
    eprintln!(
        "{what} at 3840x2160 ({} session, {}): {ms:.1} ms a frame, {:.1} MB of buffers kept",
        session_name(),
        build(),
        bytes as f64 / (1 << 20) as f64
    );
}

fn report(what: &str, (ms, bytes): (f64, usize)) {
    eprintln!(
        "{what} at 3840x2160 ({} session, {}): {ms:.1} ms a frame, {:.1} MB of buffers kept",
        session_name(),
        build(),
        bytes as f64 / (1 << 20) as f64
    );
    assert!(bytes < SCRATCH_BUDGET, "{what}: {bytes} bytes of buffers kept");
    if !cfg!(debug_assertions) {
        assert!(ms < BUDGET_MS, "{what}: {ms:.1} ms a frame");
    }
}

fn cell(i: usize) -> RectF {
    rect((i % 64) as f32 * 60.0 + 4.0, (i / 64) as f32 * 60.0 + 4.0, 52.0, 52.0)
}

#[test]
fn two_thousand_sibling_clips_stay_cheap() {
    let mut items = vec![background(3840, 2160, "#101418")];
    for i in 0..2000 {
        let c = cell(i);
        items.push(push_clip(1, c, 8.0));
        items.push(fill(2, rect(c.x - 6.0, c.y - 6.0, 70.0, 30.0), 0.0, col("#366382")));
        items.push(fill(3, rect(c.x - 6.0, c.y + 10.0, 70.0, 30.0), 0.0, col("#e79b25")));
        items.push(pop_clip(1, c.outset(1.0)));
    }
    report("2000 sibling clips", cost(&list(3840, 2160, items)));
}

#[test]
fn two_thousand_sibling_opacity_groups_stay_cheap() {
    let mut items = vec![background(3840, 2160, "#101418")];
    for i in 0..2000 {
        let c = cell(i);
        // A gradient is not a solid colour, so its group gets a real layer (a solid shape has its alpha folded in).
        items.push(push_opacity(1, 0.5, c.outset(1.0)));
        items.push(gradient(2, c, 4.0, (c.x, c.y), (c.right(), c.bottom()), &[(0.0, "#366382"), (1.0, "#e79b25")]));
        items.push(pop_opacity(1, c.outset(1.0)));
    }
    // Drawing the gradients themselves costs what it costs; the groups must add little to it.
    let plain: Vec<DrawItem> =
        items.iter().filter(|i| !matches!(i.cmd, DrawCmd::PushOpacity(_) | DrawCmd::PopOpacity)).cloned().collect();
    let (plain_ms, _) = cost(&list(3840, 2160, plain));
    let (ms, bytes) = cost(&list(3840, 2160, items));
    eprintln!("2000 opacity groups: {ms:.1} ms with the groups, {plain_ms:.1} ms for the same shapes with none");
    assert!(bytes < SCRATCH_BUDGET, "{bytes} bytes of buffers kept");
    if !cfg!(debug_assertions) {
        assert!(ms - plain_ms < BUDGET_MS / 2.0, "the groups add {:.1} ms", ms - plain_ms);
    }
    report_only("2000 opacity groups", ms, bytes);
}

#[test]
fn sixty_three_nested_opacity_and_clip_groups_stay_cheap_and_small() {
    let mut items = vec![background(3840, 2160, "#101418")];
    for d in 0..63 {
        let inset = d as f32 * 6.0;
        let r = rect(inset, inset, 3840.0 - 2.0 * inset, 2160.0 - 2.0 * inset);
        items.push(push_opacity(1, 0.97, r.outset(1.0)));
        items.push(push_clip(2, r, 10.0));
    }
    items.push(fill(3, rect(400.0, 300.0, 3000.0, 1500.0), 0.0, col("#366382")));
    items.push(fill(4, rect(900.0, 700.0, 2000.0, 800.0), 0.0, col("#e79b25")));
    for d in (0..63).rev() {
        let inset = d as f32 * 6.0;
        let r = rect(inset, inset, 3840.0 - 2.0 * inset, 2160.0 - 2.0 * inset);
        items.push(pop_clip(2, r.outset(1.0)));
        items.push(pop_opacity(1, r.outset(1.0)));
    }
    report("63 nested opacity and clip groups", cost(&list(3840, 2160, items)));
}

fn pixels(l: &DrawList, r: &mut Renderer) -> Vec<u8> {
    let mut c = Canvas::new(l.width, l.height);
    r.render(l, &mut c.pixels(), None).unwrap();
    c.data.to_vec()
}

#[test]
fn a_mask_or_layer_reused_for_a_later_group_never_shows_an_earlier_one() {
    // A big clip, a big opacity group, then small ones: the buffers the first two left behind are the ones the
    // small groups get. Drawn on a renderer that has seen the big groups, the small list must equal the same
    // list drawn on a fresh renderer.
    let big = {
        let r = rect(10.0, 10.0, 300.0, 200.0);
        list(
            320,
            220,
            vec![
                background(320, 220, "#101418"),
                push_clip(1, r, 20.0),
                fill(2, rect(0.0, 0.0, 320.0, 220.0), 0.0, col("#ff0000")),
                pop_clip(1, r.outset(1.0)),
                push_opacity(3, 0.5, r.outset(1.0)),
                fill(4, r, 0.0, col("#00ff00")),
                fill(5, rect(40.0, 40.0, 200.0, 100.0), 0.0, col("#0000ff")),
                pop_opacity(3, r.outset(1.0)),
            ],
        )
    };
    let small = {
        let c = rect(200.0, 120.0, 40.0, 30.0);
        let o = rect(20.0, 20.0, 30.0, 30.0);
        list(
            320,
            220,
            vec![
                background(320, 220, "#101418"),
                push_clip(1, c, 6.0),
                fill(2, rect(180.0, 100.0, 120.0, 100.0), 0.0, col("#366382")),
                pop_clip(1, c.outset(1.0)),
                push_opacity(3, 0.5, o.outset(1.0)),
                fill(4, o, 0.0, col("#e79b25")),
                fill(5, rect(30.0, 30.0, 30.0, 30.0), 0.0, col("#ffffff")),
                pop_opacity(3, rect(20.0, 20.0, 40.0, 40.0).outset(1.0)),
            ],
        )
    };
    let mut fresh = Renderer::new(FontConfig::default());
    let want = pixels(&small, &mut fresh);
    let mut used = Renderer::new(FontConfig::default());
    pixels(&big, &mut used);
    pixels(&big, &mut used);
    assert_eq!(pixels(&small, &mut used), want, "the small list drew differently after the big groups");
    // The same through a partial redraw: only a corner is rewritten.
    let mut canvas = Canvas::new(320, 220);
    used.render(&big, &mut canvas.pixels(), None).unwrap();
    used.render(&small, &mut canvas.pixels(), Some(RectPx::new(0, 0, 320, 220))).unwrap();
    assert_eq!(canvas.data.to_vec(), want);
}

fn star() -> Arc<PathF> {
    // A five-point star drawn in one stroke: its middle is covered twice.
    let c = PointF { x: 100.0, y: 100.0 };
    let tip = |k: usize| {
        let a = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::TAU / 5.0;
        (c.x + 90.0 * a.cos(), c.y + 90.0 * a.sin())
    };
    poly(&[tip(0), tip(2), tip(4), tip(1), tip(3)])
}

#[test]
fn a_clip_path_follows_its_fill_rule() {
    let path = star();
    let draw = |even_odd: bool| {
        let items = vec![
            item(1, rect(0.0, 0.0, 200.0, 200.0), DrawCmd::PushClipPath { path: path.clone(), even_odd }),
            fill(2, rect(0.0, 0.0, 200.0, 200.0), 0.0, col("#ff0000")),
            pop_clip(1, rect(0.0, 0.0, 200.0, 200.0)),
        ];
        let mut r = Renderer::new(FontConfig::default());
        let mut c = Canvas::new(200, 200);
        r.render(&list(200, 200, items), &mut c.pixels(), None).unwrap();
        c
    };
    let (zero, odd) = (draw(false), draw(true));
    // The star's middle has winding number 2: inside for non-zero, a hole for even-odd. A point on an arm is in both.
    assert_eq!(zero.rgba_at(100, 100)[3], 255, "non-zero fills the middle");
    assert_eq!(odd.rgba_at(100, 100)[3], 0, "even-odd leaves the middle empty");
    assert_eq!(zero.rgba_at(100, 30)[3], 255);
    assert_eq!(odd.rgba_at(100, 30)[3], 255);
}

#[test]
fn a_group_past_the_layer_cap_is_still_drawn() {
    // 40 nested opacity groups on a 3840x2160 surface: the layers past the cap are drawn flat, the frame
    // completes, and the buffers kept stay small.
    let mut items = vec![background(3840, 2160, "#101418")];
    for _ in 0..40 {
        items.push(push_opacity(1, 0.9, rect(0.0, 0.0, 3840.0, 2160.0).outset(1.0)));
    }
    items.push(fill(2, rect(100.0, 100.0, 800.0, 600.0), 0.0, col("#366382")));
    items.push(fill(3, rect(300.0, 300.0, 800.0, 600.0), 0.0, col("#e79b25")));
    for _ in 0..40 {
        items.push(pop_opacity(1, rect(0.0, 0.0, 3840.0, 2160.0).outset(1.0)));
    }
    let l = list(3840, 2160, items);
    let mut r = Renderer::new(FontConfig::default());
    let mut c = Canvas::new(3840, 2160);
    r.render(&l, &mut c.pixels(), None).unwrap();
    assert!(c.rgba_at(500, 500)[3] > 0, "the content is drawn");
    assert!(r.stats().scratch_bytes < SCRATCH_BUDGET, "{} bytes kept", r.stats().scratch_bytes);
}

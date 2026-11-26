//! Renderer behaviour: bounds, partial redraws, measuring, caches, errors.

mod common;

use std::sync::Arc;
use std::time::Instant;

use common::*;
use rsb_core::draw::{DrawCmd, DrawList};
use rsb_core::geom::{Edges, Radii, RectPx};
use rsb_core::layout::TextMeasure;
use rsb_core::pixels::PixelsMut;
use rsb_core::style::TextAlign;
use rsb_render::{
    Canvas, FontConfig, GLYPH_CACHE_MAX_BYTES, IMAGE_CACHE_MAX_BYTES, Image, LAYOUT_CACHE_MAX, RenderError, Renderer,
    render_offscreen,
};

const SANS: &[&str] = &["Segoe UI"];

fn px(c: &Canvas, x: u32, y: u32) -> [u8; 4] {
    c.rgba_at(x, y)
}

fn surface(l: &DrawList) -> RectPx {
    RectPx::new(0, 0, l.width as i32, l.height as i32)
}

fn is_leaf(c: &DrawCmd) -> bool {
    !matches!(
        c,
        DrawCmd::PushClip { .. }
            | DrawCmd::PushClipPath { .. }
            | DrawCmd::PopClip
            | DrawCmd::PushOpacity(_)
            | DrawCmd::PopOpacity
    )
}

/// Bytes of a canvas inside / outside a rectangle.
fn rows(c: &Canvas, r: RectPx) -> impl Iterator<Item = (i32, i32, [u8; 4])> + '_ {
    (0..c.height as i32).flat_map(move |y| {
        (0..c.width as i32).map(move |x| {
            let i = (y as usize * c.width as usize + x as usize) * 4;
            let _ = r;
            (x, y, [c.data[i], c.data[i + 1], c.data[i + 2], c.data[i + 3]])
        })
    })
}

fn inside(r: &RectPx, x: i32, y: i32) -> bool {
    x >= r.x0 && x < r.x1 && y >= r.y0 && y < r.y1
}

// ---------------------------------------------------------------- bounds

#[test]
fn every_item_alone_stays_inside_its_bounds() {
    let mut checked = 0;
    for scene in all_scenes() {
        for it in scene.list.items.iter().filter(|i| i.node.0 != 0 && is_leaf(&i.cmd)) {
            let alone = list(scene.list.width, scene.list.height, vec![it.clone()]);
            let canvas = {
                let mut r = renderer();
                install_images(&mut r, &scene);
                render_offscreen(&mut r, &alone).unwrap()
            };
            let allowed = rsb_core::geom::RectPx::outward(&it.bounds);
            for (x, y, p) in rows(&canvas, allowed) {
                assert!(
                    inside(&allowed, x, y) || p == [0, 0, 0, 0],
                    "scene {} node {} painted {p:?} at ({x},{y}) outside {allowed:?}",
                    scene.name,
                    it.node.0
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 100, "checked {checked} items");
}

#[test]
fn items_draw_something() {
    // every leaf of every scene leaves ink when drawn alone, except the ones
    // that are meant to draw nothing
    for scene in all_scenes() {
        for it in scene.list.items.iter().filter(|i| i.node.0 != 0 && is_leaf(&i.cmd)) {
            let skip = matches!(&it.cmd, DrawCmd::Image { key, .. } | DrawCmd::Svg { key, .. } if &**key == "missing")
                || matches!(&it.cmd, DrawCmd::Fill { rect, .. } if rect.w == 0.0)
                || (scene.name == "clips" && it.node.0 == 13)
                || (scene.name == "opacity" && it.node.0 == 12);
            if skip {
                continue;
            }
            let alone = list(scene.list.width, scene.list.height, vec![it.clone()]);
            let canvas = {
                let mut r = renderer();
                install_images(&mut r, &scene);
                render_offscreen(&mut r, &alone).unwrap()
            };
            let ink = canvas.data.as_chunks::<4>().0.iter().filter(|p| p[3] != 0).count();
            assert!(ink > 0, "scene {} node {} drew nothing", scene.name, it.node.0);
        }
    }
}

#[test]
fn items_with_understated_bounds_are_cut_to_them() {
    let area = rect(10.0, 10.0, 60.0, 40.0);
    let st = style(SANS, 16.0, 400);
    // each command with bounds that cover only part of what it would draw
    let cases = vec![
        (
            rect(30.0, 20.0, 20.0, 10.0),
            DrawCmd::Fill { rect: area, radii: Radii::all(6.0), fill: rsb_core::draw::Fill::Solid(col("#366382")) },
        ),
        (
            rect(30.0, 5.0, 20.0, 12.0),
            DrawCmd::Border { rect: area, radii: Radii::all(6.0), widths: Edges::all(3.0), color: col("#E79B25") },
        ),
        (
            rect(30.0, 20.0, 20.0, 10.0),
            DrawCmd::Shadow { rect: area, radii: Radii::all(6.0), blur: 8.0, color: col("#000000") },
        ),
        (
            rect(30.0, 22.0, 20.0, 12.0),
            DrawCmd::Text {
                rect: area,
                text: "Delta Delta Delta".into(),
                style: st,
                color: col("#000000"),
                align: TextAlign::Start,
            },
        ),
        (rect(30.0, 20.0, 20.0, 10.0), DrawCmd::Image { rect: area, key: "ramp16".into(), radii: Radii::all(4.0) }),
    ];
    for (bounds, cmd) in cases {
        let allowed = RectPx::outward(&bounds);
        let name = format!("{cmd:?}").chars().take(12).collect::<String>();
        let l = list(100, 70, vec![item(1, bounds, cmd)]);
        let canvas = {
            let mut r = renderer();
            r.set_image(Arc::from("ramp16"), test_image(16));
            render_offscreen(&mut r, &l).unwrap()
        };
        let mut ink = 0;
        for (x, y, p) in rows(&canvas, allowed) {
            if p[3] != 0 {
                ink += 1;
                assert!(inside(&allowed, x, y), "{name} painted outside its bounds at ({x},{y})");
            }
        }
        assert!(ink > 0, "{name} drew nothing inside its bounds");
    }
}

// --------------------------------------------------------------- partial

fn regions(w: i32, h: i32) -> Vec<RectPx> {
    vec![
        RectPx::new(0, 0, w, h),
        RectPx::new(w / 4, h / 4, w * 3 / 4, h * 3 / 4),
        RectPx::new(0, 0, w / 2, h / 3),
        RectPx::new(w - 40, h - 30, w, h),
        RectPx::new(17, 9, 61, 33),
        RectPx::new(w / 2, 0, w / 2 + 1, h),
        RectPx::new(5, 5, 6, 6),
        RectPx::new(-20, -20, 30, 30),
        RectPx::new(w - 25, h - 25, w + 50, h + 50),
        RectPx::new(30, 0, 200, 14),
    ]
}

fn render_into(
    r: &mut Renderer,
    l: &DrawList,
    data: &mut [u8],
    region: Option<RectPx>,
) -> Result<Option<RectPx>, RenderError> {
    let mut target = PixelsMut::new(data, l.width, l.height).unwrap();
    r.render(l, &mut target, region)
}

#[test]
fn partial_redraw_matches_full_inside_and_leaves_outside_alone() {
    for scene in all_scenes() {
        let mut r = renderer();
        install_images(&mut r, &scene);
        let l = &scene.list;
        let len = l.width as usize * l.height as usize * 4;
        let mut full = vec![0u8; len];
        assert_eq!(render_into(&mut r, l, &mut full, None).unwrap(), Some(surface(l)));
        for region in regions(l.width as i32, l.height as i32) {
            let mut data = vec![0xABu8; len];
            let got = render_into(&mut r, l, &mut data, Some(region)).unwrap();
            let want = region.intersect(&surface(l));
            assert_eq!(got, want, "scene {} region {region:?}", scene.name);
            let Some(cut) = want else { continue };
            for y in 0..l.height as i32 {
                for x in 0..l.width as i32 {
                    let i = (y as usize * l.width as usize + x as usize) * 4;
                    if inside(&cut, x, y) {
                        assert_eq!(
                            &data[i..i + 4],
                            &full[i..i + 4],
                            "scene {} region {region:?} differs at ({x},{y})",
                            scene.name
                        );
                    } else {
                        assert_eq!(
                            &data[i..i + 4],
                            &[0xAB; 4],
                            "scene {} region {region:?} touched ({x},{y})",
                            scene.name
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn random_regions_match_a_full_redraw() {
    let mut seed = 99u64;
    for scene in all_scenes() {
        let mut r = renderer();
        install_images(&mut r, &scene);
        let l = &scene.list;
        let (w, h) = (l.width as i32, l.height as i32);
        let len = l.width as usize * l.height as usize * 4;
        let mut full = vec![0u8; len];
        render_into(&mut r, l, &mut full, None).unwrap();
        // one buffer reused for every region, so frames build on each other
        let mut data = full.clone();
        for _ in 0..40 {
            let x0 = (lcg(&mut seed) % w as u64) as i32;
            let y0 = (lcg(&mut seed) % h as u64) as i32;
            let region =
                RectPx::new(x0, y0, x0 + 1 + (lcg(&mut seed) % 120) as i32, y0 + 1 + (lcg(&mut seed) % 40) as i32);
            render_into(&mut r, l, &mut data, Some(region)).unwrap();
            assert_eq!(data, full, "scene {} region {region:?}", scene.name);
        }
    }
}

#[test]
fn partial_redraws_do_not_leak_into_later_frames() {
    let scene = scene_opacity();
    let mut r = renderer();
    let l = &scene.list;
    let len = l.width as usize * l.height as usize * 4;
    let mut fresh = vec![0u8; len];
    render_into(&mut r, l, &mut fresh, None).unwrap();
    let mut data = vec![0u8; len];
    for region in regions(l.width as i32, l.height as i32) {
        render_into(&mut r, l, &mut data, Some(region)).unwrap();
    }
    render_into(&mut r, l, &mut data, None).unwrap();
    assert_eq!(data, fresh);
}

#[test]
fn empty_or_outside_regions_draw_nothing() {
    let scene = scene_shapes();
    let mut r = renderer();
    let l = &scene.list;
    let mut data = vec![7u8; l.width as usize * l.height as usize * 4];
    for region in [
        RectPx::new(5, 5, 5, 20),
        RectPx::new(5, 5, 20, 5),
        RectPx::new(500, 500, 600, 600),
        RectPx::new(-50, -50, -1, -1),
        RectPx::new(20, 20, 10, 10),
    ] {
        assert_eq!(render_into(&mut r, l, &mut data, Some(region)).unwrap(), None, "{region:?}");
    }
    assert!(data.iter().all(|b| *b == 7));
}

// ----------------------------------------------------------------- shapes

#[test]
fn bar_fill_with_alpha_one_over_255_survives() {
    let bar = rect(0.0, 0.0, 200.0, 32.0);
    let l = list(200, 32, vec![fill(1, bar, 0.0, rsb_core::color::Color::rgba8(0, 0, 0, 1))]);
    let c = render_offscreen(&mut renderer(), &l).unwrap();
    assert!(c.data.as_chunks::<4>().0.iter().all(|p| p[3] >= 1), "every bar pixel keeps alpha >= 1");
}

#[test]
fn rounded_rect_corners_are_transparent_and_centre_is_solid() {
    let l = list(60, 40, vec![fill(1, rect(10.0, 10.0, 40.0, 20.0), 10.0, col("#366382"))]);
    let c = render_offscreen(&mut renderer(), &l).unwrap();
    assert_eq!(px(&c, 10, 10)[3], 0, "outer corner pixel is outside the pill");
    assert_eq!(px(&c, 30, 20), [0x36, 0x63, 0x82, 255]);
    assert_eq!(px(&c, 20, 10)[3], 255, "top edge inside the cap");
}

#[test]
fn border_leaves_the_inside_untouched() {
    let l = list(60, 40, vec![border(1, rect(10.0, 10.0, 40.0, 20.0), 0.0, Edges::all(2.0), col("#E79B25"))]);
    let c = render_offscreen(&mut renderer(), &l).unwrap();
    assert_eq!(px(&c, 10, 10), [0xE7, 0x9B, 0x25, 255]);
    assert_eq!(px(&c, 11, 20), [0xE7, 0x9B, 0x25, 255]);
    assert_eq!(px(&c, 30, 20)[3], 0, "inside stays transparent");
    assert_eq!(px(&c, 30, 12)[3], 0, "just inside the 2 px border");
}

#[test]
fn per_side_widths_are_honoured() {
    let widths = Edges { top: 5.0, right: 1.0, bottom: 0.0, left: 3.0 };
    let l = list(60, 40, vec![border(1, rect(10.0, 10.0, 40.0, 20.0), 0.0, widths, col("#000000"))]);
    let c = render_offscreen(&mut renderer(), &l).unwrap();
    assert_eq!(px(&c, 30, 14)[3], 255, "top 5 px");
    assert_eq!(px(&c, 30, 15)[3], 0);
    assert_eq!(px(&c, 12, 20)[3], 255, "left 3 px");
    assert_eq!(px(&c, 13, 20)[3], 0);
    assert_eq!(px(&c, 49, 20)[3], 255, "right 1 px");
    assert_eq!(px(&c, 48, 20)[3], 0);
    assert_eq!(px(&c, 30, 29)[3], 0, "no bottom border");
}

#[test]
fn linear_gradient_runs_between_its_stops() {
    let it =
        gradient(1, rect(0.0, 0.0, 100.0, 10.0), 0.0, (0.0, 0.0), (100.0, 0.0), &[(0.0, "#000000"), (1.0, "#ffffff")]);
    let c = render_offscreen(&mut renderer(), &list(100, 10, vec![it])).unwrap();
    let left = px(&c, 1, 5)[0] as i32;
    let mid = px(&c, 50, 5)[0] as i32;
    let right = px(&c, 98, 5)[0] as i32;
    assert!(left < 12 && (mid - 128).abs() < 8 && right > 243, "{left} {mid} {right}");
}

#[test]
fn shadow_is_darkest_under_the_box_and_fades_out() {
    let l = list(100, 80, vec![shadow(1, rect(30.0, 25.0, 40.0, 30.0), 6.0, 12.0, col("#000000"))]);
    let c = render_offscreen(&mut renderer(), &l).unwrap();
    let centre = px(&c, 50, 40)[3];
    let edge = px(&c, 30, 40)[3];
    let outside = px(&c, 20, 40)[3];
    let far = px(&c, 5, 40)[3];
    assert!(centre > 240, "centre {centre}");
    assert!((100..160).contains(&edge), "edge ~50 % {edge}");
    assert!(outside < edge && outside > 0, "outside {outside}");
    assert!(far <= 2, "far {far}");
}

#[test]
fn group_opacity_does_not_double_blend_overlaps() {
    let scene = scene_opacity();
    let c = render_offscreen(&mut renderer(), &scene.list).unwrap();
    // overlap of the blue and orange boxes in group 1: orange (top) at 50 % over white
    let p = px(&c, 50, 40);
    let expect = [(0xE7 + 255) / 2, (0x9B + 255) / 2, (0x25 + 255) / 2];
    for (got, want) in p.iter().zip(expect) {
        assert!((*got as i32 - want).abs() <= 2, "{p:?} vs {expect:?}");
    }
    assert_eq!(px(&c, 50, 95), [255, 255, 255, 255]);
    assert_eq!(px(&c, 150, 95), [0x36, 0x63, 0x82, 255]);
}

#[test]
fn clips_cut_children_and_empty_clips_hide_everything() {
    let scene = scene_clips();
    let c = render_offscreen(&mut renderer(), &scene.list).unwrap();
    let bg = px(&c, 2, 2);
    // inside the rounded clip: gradient; at the clip's rounded corner: background
    assert_ne!(px(&c, 70, 50), bg);
    assert_eq!(px(&c, 20, 20), bg, "rounded corner pixel is clipped away");
    assert_eq!(px(&c, 140, 50), bg, "outside the clip rect");
    // the zero-width clip hides the red bar
    for x in 0..400 {
        assert_ne!(px(&c, x, 100)[0..3], [255, 0, 0], "red leaked at x={x}");
    }
}

#[test]
fn unbalanced_pops_and_open_groups_do_not_panic() {
    let bounds = rect(0.0, 0.0, 40.0, 40.0);
    let items = vec![
        pop_clip(1, bounds),
        pop_opacity(2, bounds),
        push_opacity(3, 0.5, bounds),
        push_clip(4, rect(0.0, 0.0, 20.0, 20.0), 4.0),
        fill(5, rect(0.0, 0.0, 40.0, 40.0), 0.0, col("#ff0000")),
    ];
    let c = render_offscreen(&mut renderer(), &list(40, 40, items)).unwrap();
    assert!(px(&c, 10, 10)[3] > 100);
    assert_eq!(px(&c, 30, 30)[3], 0);
}

#[test]
fn non_finite_numbers_do_not_panic() {
    let nan = f32::NAN;
    let items = vec![
        fill(1, rect(nan, 0.0, 10.0, 10.0), 2.0, col("#000000")),
        fill(2, rect(0.0, 0.0, f32::INFINITY, 10.0), 2.0, col("#000000")),
        fill(3, rect(0.0, 0.0, 10.0, 10.0), nan, col("#000000")),
        shadow(4, rect(0.0, 0.0, 10.0, 10.0), 2.0, nan, col("#000000")),
        border(5, rect(0.0, 0.0, 10.0, 10.0), 2.0, Edges::all(nan), col("#000000")),
        push_opacity(6, nan, rect(0.0, 0.0, 10.0, 10.0)),
        pop_opacity(6, rect(0.0, 0.0, 10.0, 10.0)),
    ];
    render_offscreen(&mut renderer(), &list(20, 20, items)).unwrap();
}

#[test]
fn images_scale_clip_and_missing_ones_draw_nothing() {
    let scene = scene_images();
    let mut r = renderer();
    install_images(&mut r, &scene);
    assert!(r.has_image("ramp16") && !r.has_image("missing"));
    let c = render_offscreen(&mut r, &scene.list).unwrap();
    // 1:1 copy: top-left transparent texel shows the background, the next one the ramp
    let bg = px(&c, 5, 5);
    assert_eq!(px(&c, 10, 10), bg, "transparent texel");
    assert_ne!(px(&c, 25, 25), bg, "opaque texel");
    // 4x scale with radius 32 (a circle): corner is background, centre is image
    assert_eq!(px(&c, 120, 10), bg);
    assert_ne!(px(&c, 152, 42), bg);
    // missing key: background
    assert_eq!(px(&c, 70, 100), bg);
}

#[test]
fn images_are_validated_and_counted() {
    let mut r = Renderer::new(FontConfig::default());
    r.set_image(Arc::from("bad"), Image { width: 4, height: 4, data: vec![0; 10] });
    assert!(!r.has_image("bad"));
    r.set_image(Arc::from("ok"), test_image(8));
    let st = r.stats();
    assert_eq!((st.images, st.image_bytes), (1, 8 * 8 * 4));
}

// ------------------------------------------------------------------ errors

#[test]
fn bad_sizes_and_small_targets_are_errors() {
    let mut r = renderer();
    let l = list(100, 50, vec![]);
    let mut small = vec![0u8; 100 * 49 * 4];
    let mut t = PixelsMut { data: &mut small, width: 100, height: 49 };
    assert_eq!(r.render(&l, &mut t, None), Err(RenderError::TargetSize { need: (100, 50), got: (100, 49) }));
    let mut narrow = vec![0u8; 99 * 50 * 4];
    let mut t = PixelsMut { data: &mut narrow, width: 99, height: 50 };
    assert!(matches!(r.render(&l, &mut t, None), Err(RenderError::TargetSize { .. })));
    // lying width/height with too little memory
    let mut short = vec![0u8; 10];
    let mut t = PixelsMut { data: &mut short, width: 100, height: 50 };
    assert!(matches!(r.render(&l, &mut t, None), Err(RenderError::TargetSize { .. })));
    for (w, h) in [(0, 10), (10, 0), (16385, 10), (10, 16385)] {
        assert_eq!(render_offscreen(&mut r, &list(w, h, vec![])).err(), Some(RenderError::BadSize));
    }
    assert!(render_offscreen(&mut r, &list(1, 1, vec![])).is_ok());
}

#[test]
fn bigger_targets_are_allowed() {
    let mut r = renderer();
    let l = list(10, 4, vec![fill(1, rect(0.0, 0.0, 10.0, 4.0), 0.0, col("#ff0000"))]);
    let mut big = vec![0u8; 16 * 8 * 4];
    let mut t = PixelsMut { data: &mut big, width: 16, height: 8 };
    r.render(&l, &mut t, None).unwrap();
    // the target's own stride is used: pixel (9, 3) is red, (10, 3) untouched
    let at = |x: usize, y: usize| big[(y * 16 + x) * 4..(y * 16 + x) * 4 + 4].to_vec();
    assert_eq!(at(9, 3), vec![0, 0, 255, 255], "BGRA red");
    assert_eq!(at(10, 3), vec![0, 0, 0, 0]);
}

#[test]
fn output_is_premultiplied_bgra() {
    let l = list(2, 1, vec![fill(1, rect(0.0, 0.0, 2.0, 1.0), 0.0, rsb_core::color::Color::rgba8(200, 100, 50, 128))]);
    let c = render_offscreen(&mut renderer(), &l).unwrap();
    let (b, g, rr, a) = (c.data[0] as i32, c.data[1] as i32, c.data[2] as i32, c.data[3] as i32);
    assert!((a - 128).abs() <= 1, "alpha {a}");
    assert!((rr - 100).abs() <= 2 && (g - 50).abs() <= 2 && (b - 25).abs() <= 2, "premultiplied {rr} {g} {b}");
    let s = c.rgba_at(0, 0);
    assert!((s[0] as i32 - 200).abs() <= 3 && (s[1] as i32 - 100).abs() <= 3 && (s[2] as i32 - 50).abs() <= 3);
    assert!(c.to_png().unwrap().starts_with(&[0x89, b'P', b'N', b'G']));
    assert_eq!(c.rgba_at(5, 5), [0; 4]);
}

// -------------------------------------------------------------------- text

#[test]
fn has_family_and_icon_font() {
    let mut r = renderer();
    assert!(r.has_family("Segoe UI"));
    assert!(r.has_family("segoe ui"), "case-insensitive");
    assert!(r.has_family("  SEGOE UI "), "surrounding spaces ignored");
    assert!(!r.has_family("Definitely Not Installed 123"));
    let icon = r.icon_font();
    assert!(r.has_family(icon.family()), "chosen icon font {icon:?} is installed");
    assert_eq!(icon, rsb_core::icons::IconFont::Fluent, "Windows 11 has Segoe Fluent Icons");
    assert!(r.has_family("Segoe MDL2 Assets"));
}

fn width_of(r: &mut Renderer, text: &str, st: &rsb_core::draw::TextStyle) -> f32 {
    r.measure(text, st, None).w
}

/// Ink bounds (x range) of the opaque-ish pixels in a canvas.
fn ink_x(c: &Canvas, y0: u32, y1: u32) -> Option<(u32, u32)> {
    let mut lo = u32::MAX;
    let mut hi = 0;
    for y in y0..y1 {
        for x in 0..c.width {
            if c.rgba_at(x, y)[3] > 40 {
                lo = lo.min(x);
                hi = hi.max(x);
            }
        }
    }
    (lo != u32::MAX).then_some((lo, hi))
}

#[test]
fn measured_width_equals_drawn_width() {
    let mut r = renderer();
    for (size, weight) in [(13.0, 400), (20.0, 400), (16.0, 700)] {
        let st = style(SANS, size, weight);
        let one = width_of(&mut r, "H", &st);
        let eight = width_of(&mut r, "HHHHHHHH", &st);
        assert!((eight - 8.0 * one).abs() < 1.0, "advance is additive: {eight} vs 8 x {one}");
        // draw one H and eight H left aligned in a wide rect; ink starts and ends a bearing in
        let draw = |r: &mut Renderer, s: &str| {
            let h = r.measure(s, &st, None).h;
            let l = list(300, 60, vec![text(1, rect(20.0, 5.0, 260.0, h), s, &st, col("#000000"), TextAlign::Start)]);
            render_offscreen(r, &l).unwrap()
        };
        let c1 = draw(&mut r, "H");
        let c8 = draw(&mut r, "HHHHHHHH");
        let (a1, b1) = ink_x(&c1, 0, 60).unwrap();
        let (a8, b8) = ink_x(&c8, 0, 60).unwrap();
        let bearings = (one - (b1 - a1 + 1) as f32).max(0.0);
        let ink8 = (b8 - a8 + 1) as f32;
        assert!(
            (ink8 - (eight - bearings)).abs() <= 1.0,
            "size {size}: ink {ink8}, measured {eight}, bearings {bearings}"
        );
        // left aligned text starts at the rect edge plus its left bearing
        assert!((a8 as f32 - 20.0).abs() <= bearings, "starts at {a8}");
    }
}

#[test]
fn centre_and_end_alignment_use_the_measured_width() {
    let mut r = renderer();
    let st = style(SANS, 16.0, 400);
    let s = "HHHHHH";
    let w = width_of(&mut r, s, &st);
    let h = r.measure(s, &st, None).h;
    for (align, expect_left) in
        [(TextAlign::Center, 50.0 + (200.0 - w) / 2.0), (TextAlign::End, 50.0 + 200.0 - w), (TextAlign::Start, 50.0)]
    {
        let l = list(300, 40, vec![text(1, rect(50.0, 4.0, 200.0, h), s, &st, col("#000000"), align)]);
        let c = render_offscreen(&mut r, &l).unwrap();
        let (a, _) = ink_x(&c, 0, 40).unwrap();
        assert!((a as f32 - expect_left).abs() <= 3.0, "{align:?}: ink starts {a}, expected about {expect_left}");
    }
}

#[test]
fn letter_spacing_adds_to_every_character() {
    let mut r = renderer();
    let base = style(SANS, 14.0, 400);
    let mut spaced = base.clone();
    spaced.letter_spacing = 3.0;
    let s = "Spacing";
    let d = width_of(&mut r, s, &spaced) - width_of(&mut r, s, &base);
    assert!((d - 3.0 * s.chars().count() as f32).abs() < 0.5, "extra width {d}");
    let mut tight = base.clone();
    tight.letter_spacing = -1.0;
    let d = width_of(&mut r, s, &tight) - width_of(&mut r, s, &base);
    assert!((d + s.chars().count() as f32).abs() < 0.5, "negative spacing {d}");
}

#[test]
fn tabular_digits_have_one_width() {
    let mut r = renderer();
    // a font whose digits are proportional until `tnum` is on
    let prop = style(&["Candara"], 18.0, 400);
    let mut tab = prop.clone();
    tab.tabular_nums = true;
    let ones = width_of(&mut r, "1111111111", &prop);
    let zeros = width_of(&mut r, "0000000000", &prop);
    assert!((ones - zeros).abs() > 1.0, "Candara digits are proportional by default ({ones} vs {zeros})");
    let ones_t = width_of(&mut r, "1111111111", &tab);
    let zeros_t = width_of(&mut r, "0000000000", &tab);
    assert!((ones_t - zeros_t).abs() < 0.01, "tabular: {ones_t} vs {zeros_t}");
    // Segoe UI: all ten digits one width with the feature on
    let mut seg = style(SANS, 16.0, 400);
    seg.tabular_nums = true;
    let w0 = width_of(&mut r, "0", &seg);
    for d in '1'..='9' {
        assert!((width_of(&mut r, &d.to_string(), &seg) - w0).abs() < 0.01, "digit {d}");
    }
}

#[test]
fn arabic_joins_and_forms_ligatures() {
    let mut r = renderer();
    let st = style(SANS, 18.0, 400);
    // lam + alef is one ligature glyph; a zero width non-joiner in between breaks it
    let joined = width_of(&mut r, "\u{644}\u{627}", &st);
    let split = width_of(&mut r, "\u{644}\u{200C}\u{627}", &st);
    assert!(joined > 0.0 && (joined - split).abs() > 0.5, "joined {joined}, split {split}");
    // joined letters are narrower than the same letters set apart with spaces
    let word = width_of(&mut r, "\u{645}\u{631}\u{62D}\u{628}\u{627}", &st);
    let apart = width_of(&mut r, "\u{645} \u{631} \u{62D} \u{628} \u{627}", &st);
    assert!(apart > word + 4.0 * width_of(&mut r, " ", &st) - 1.0, "word {word}, apart {apart}");
}

#[test]
fn right_to_left_text_is_drawn() {
    let st = style(SANS, 20.0, 400);
    let l = list(
        200,
        40,
        vec![text(
            1,
            rect(10.0, 4.0, 180.0, 30.0),
            "\u{645}\u{631}\u{62D}\u{628}\u{627}",
            &st,
            col("#000000"),
            TextAlign::Start,
        )],
    );
    let c = render_offscreen(&mut renderer(), &l).unwrap();
    assert!(c.data.as_chunks::<4>().0.iter().filter(|p| p[3] > 100).count() > 100);
}

#[test]
fn colour_emoji_draws_colour_and_follows_text_alpha() {
    let mut r = renderer();
    let st = style(SANS, 32.0, 400);
    let draw = |r: &mut Renderer, color: &str| {
        let l = list(
            80,
            50,
            vec![
                fill(0, rect(0.0, 0.0, 80.0, 50.0), 0.0, col("#ffffff")),
                text(1, rect(10.0, 4.0, 60.0, 42.0), "\u{1F525}", &st, col(color), TextAlign::Start),
            ],
        );
        render_offscreen(r, &l).unwrap()
    };
    let before = r.stats().colour_glyphs_drawn;
    let full = draw(&mut r, "#000000");
    assert!(r.stats().colour_glyphs_drawn > before, "the emoji came from a colour font");
    let colourful = full.data.as_chunks::<4>().0.iter().filter(|p| p[0].abs_diff(p[2]) > 60).count();
    assert!(colourful > 50, "{colourful} colourful pixels");
    // alpha 0.5 text colour halves the emoji's strength: distance from white
    let half = draw(&mut r, "#00000080");
    let strength = |c: &Canvas| c.data.as_chunks::<4>().0.iter().map(|p| 255 - p[1] as u64).sum::<u64>() as f64;
    let ratio = strength(&half) / strength(&full);
    assert!((ratio - 0.5).abs() < 0.08, "ratio {ratio}");
}

#[test]
fn icon_glyphs_draw_ink() {
    let mut r = renderer();
    let st = style(&["icons"], 20.0, 400);
    for c in ICON_POINTS {
        let l = list(
            40,
            36,
            vec![text(1, rect(4.0, 2.0, 32.0, 32.0), &c.to_string(), &st, col("#000000"), TextAlign::Center)],
        );
        let canvas = render_offscreen(&mut r, &l).unwrap();
        let ink = canvas.data.as_chunks::<4>().0.iter().filter(|p| p[3] > 0).count();
        assert!(ink > 20, "U+{:04X} drew {ink} pixels", *c as u32);
    }
}

#[test]
fn missing_family_falls_back_to_the_sans_family() {
    let mut r = renderer();
    let ghost = style(&["Definitely Not Installed 123"], 15.0, 400);
    let seg = style(SANS, 15.0, 400);
    assert_eq!(width_of(&mut r, "Fallback text", &ghost), width_of(&mut r, "Fallback text", &seg));
    let generic = style(&["sans-serif"], 15.0, 400);
    assert_eq!(width_of(&mut r, "Fallback text", &generic), width_of(&mut r, "Fallback text", &seg));
}

#[test]
fn empty_text_and_huge_text_are_safe() {
    let mut r = renderer();
    let st = style(SANS, 14.0, 400);
    assert_eq!(width_of(&mut r, "", &st), 0.0);
    let long = "w".repeat(100_000);
    let w = width_of(&mut r, &long, &st);
    assert!(w > 1000.0);
    let l = list(100, 30, vec![text(1, rect(0.0, 0.0, 100.0, 24.0), &long, &st, col("#000000"), TextAlign::Start)]);
    render_offscreen(&mut r, &l).unwrap();
    // absurd sizes
    for size in [0.0, -5.0, f32::NAN, 100_000.0] {
        let st = style(SANS, size, 400);
        let l = list(100, 30, vec![text(1, rect(0.0, 0.0, 100.0, 24.0), "x", &st, col("#000000"), TextAlign::Start)]);
        render_offscreen(&mut r, &l).unwrap();
        let _ = width_of(&mut r, "x", &st);
    }
}

// ------------------------------------------------------------------ caches

fn lcg(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    *state >> 33
}

#[test]
fn glyph_cache_stays_under_its_cap_over_ten_thousand_strings() {
    let mut r = Renderer::new(FontConfig::default());
    let mut seed = 7u64;
    let mut canvas = Canvas::new(420, 90);
    let mut peak = 0usize;
    let mut shrank = false;
    let mut last = 0usize;
    for i in 0..10_000u32 {
        // distinct strings: unified ideographs (system fallback fonts) at changing sizes
        let s: String =
            (0..5).map(|_| char::from_u32(0x4E00 + (lcg(&mut seed) % 2000) as u32).unwrap()).collect::<String>()
                + &i.to_string();
        let st = style(SANS, 22.0 + (i % 40) as f32, if i % 3 == 0 { 700 } else { 400 });
        let l = list(420, 90, vec![text(1, rect(0.0, 0.0, 420.0, 90.0), &s, &st, col("#000000"), TextAlign::Start)]);
        r.render(&l, &mut canvas.pixels(), None).unwrap();
        let stats = r.stats();
        assert!(stats.glyph_bytes <= GLYPH_CACHE_MAX_BYTES, "after string {i}: {} bytes", stats.glyph_bytes);
        assert!(stats.layouts_cached <= LAYOUT_CACHE_MAX, "after string {i}: {} layouts", stats.layouts_cached);
        peak = peak.max(stats.glyph_bytes);
        if stats.glyph_bytes < last {
            shrank = true;
        }
        last = stats.glyph_bytes;
    }
    assert!(peak > GLYPH_CACHE_MAX_BYTES / 2, "the test filled the cache to {peak} bytes");
    assert!(shrank, "entries were evicted");
    eprintln!(
        "glyph cache: peak {peak} of {GLYPH_CACHE_MAX_BYTES} bytes after 10000 strings, {} glyphs kept",
        r.stats().glyphs_cached
    );
}

#[test]
fn layout_cache_stays_under_its_cap() {
    let mut r = Renderer::new(FontConfig::default());
    let st = style(SANS, 13.0, 400);
    for i in 0..(LAYOUT_CACHE_MAX * 4) {
        r.measure(&format!("string number {i}"), &st, None);
        assert!(r.stats().layouts_cached <= LAYOUT_CACHE_MAX);
    }
    assert_eq!(r.stats().layouts_cached, LAYOUT_CACHE_MAX);
    // the most recent ones are still cached: measuring again adds no entry
    let before = r.stats().layouts_cached;
    r.measure(&format!("string number {}", LAYOUT_CACHE_MAX * 4 - 1), &st, None);
    assert_eq!(r.stats().layouts_cached, before);
}

#[test]
fn image_cache_stays_under_its_cap() {
    let mut r = Renderer::new(FontConfig::default());
    for i in 0..9 {
        let side = 1000;
        r.set_image(
            Arc::from(format!("big{i}")),
            Image { width: side, height: side, data: vec![9; (side * side * 4) as usize] },
        );
        assert!(r.stats().image_bytes <= IMAGE_CACHE_MAX_BYTES);
    }
    assert!(!r.has_image("big0") && r.has_image("big8"));
}

// -------------------------------------------------------------------- perf

/// A 1920x32 bar close to what the default preset draws: background,
/// pills, icons, labels, a centred clock.
fn bar_list(clock: &str) -> (DrawList, rsb_core::geom::RectF) {
    let ink = col("#E6E9F2");
    let label = style(SANS, 13.0, 400);
    let icons = style(&["icons"], 14.0, 400);
    let mut items = vec![
        fill(1, rect(0.0, 0.0, 1920.0, 32.0), 0.0, col("#1A1B26CC")),
        border(
            2,
            rect(0.0, 31.0, 1920.0, 1.0),
            0.0,
            Edges { top: 1.0, right: 0.0, bottom: 0.0, left: 0.0 },
            col("#ffffff1a"),
        ),
    ];
    let mut n = 10u64;
    for i in 0..5 {
        let x = 8.0 + i as f32 * 30.0;
        let active = i == 1;
        items.push(fill(n, rect(x, 5.0, 26.0, 22.0), 6.0, if active { col("#66AAFF") } else { col("#ffffff14") }));
        items.push(text(n + 1, rect(x, 5.0, 26.0, 22.0), &(i + 1).to_string(), &label, ink, TextAlign::Center));
        n += 2;
    }
    items.push(text(
        n,
        rect(180.0, 6.0, 320.0, 20.0),
        "Quarterly report - Microsoft Excel",
        &label,
        ink,
        TextAlign::Start,
    ));
    n += 1;
    for i in 0..6 {
        let x = 1500.0 + i as f32 * 62.0;
        items.push(text(n, rect(x, 6.0, 16.0, 20.0), "\u{E767}", &icons, ink, TextAlign::Center));
        items.push(text(n + 1, rect(x + 18.0, 6.0, 34.0, 20.0), "42%", &label, ink, TextAlign::Start));
        n += 2;
    }
    let clock_rect = rect(880.0, 5.0, 160.0, 22.0);
    items.push(fill(n, clock_rect, 6.0, col("#ffffff14")));
    let mut tab = style(SANS, 13.0, 600);
    tab.tabular_nums = true;
    items.push(text(n + 1, clock_rect, clock, &tab, ink, TextAlign::Center));
    (list(1920, 32, items), clock_rect)
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

#[test]
fn clock_tick_redraw_timing() {
    let mut r = Renderer::new(FontConfig::default());
    let mut canvas = Canvas::new(1920, 32);
    let (first, clock_rect) = bar_list("Tue 24 Nov 12:00:00");
    let t = Instant::now();
    r.render(&first, &mut canvas.pixels(), None).unwrap();
    let first_full_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    r.render(&first, &mut canvas.pixels(), None).unwrap();
    let warm_full_ms = t.elapsed().as_secs_f64() * 1000.0;
    let region = RectPx::outward(&clock_rect.outset(1.0));

    // every tick shows a text never seen before (layout built, glyphs mostly cached)
    let mut new_text = Vec::new();
    for i in 0..400u32 {
        let s = format!("Tue 24 Nov {:02}:{:02}:{:02}", i / 3600 % 24, i / 60 % 60, i % 60);
        let (l, _) = bar_list(&s);
        let t = Instant::now();
        r.render(&l, &mut canvas.pixels(), Some(region)).unwrap();
        if i >= 100 {
            new_text.push(t.elapsed().as_secs_f64() * 1000.0);
        }
    }
    // the same text again (layout cached)
    let (l, _) = bar_list("Tue 24 Nov 12:00:01");
    let mut same_text = Vec::new();
    for _ in 0..300 {
        let t = Instant::now();
        r.render(&l, &mut canvas.pixels(), Some(region)).unwrap();
        same_text.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let m_new = median(&mut new_text);
    let m_same = median(&mut same_text);
    eprintln!(
        "render timing ({} session, {}): first full frame {first_full_ms:.2} ms, warm full frame {warm_full_ms:.2} ms, \
         clock tick with new text median {m_new:.3} ms (p95 {:.3}), clock tick same text median {m_same:.3} ms",
        session_name(),
        if cfg!(debug_assertions) { "debug build" } else { "release build" },
        new_text[new_text.len() * 95 / 100],
    );
    if !cfg!(debug_assertions) {
        assert!(m_new < 1.0, "clock tick median {m_new:.3} ms");
    }
}

#[test]
fn memory_report() {
    // process memory after the fonts are loaded and a default-like bar was drawn
    #[repr(C)]
    struct Counters {
        cb: u32,
        page_faults: u32,
        peak_working_set: usize,
        working_set: usize,
        quota_peak_paged: usize,
        quota_paged: usize,
        quota_peak_nonpaged: usize,
        quota_nonpaged: usize,
        pagefile: usize,
        peak_pagefile: usize,
        private: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
    }
    let read = || {
        let mut c = Counters {
            cb: std::mem::size_of::<Counters>() as u32,
            page_faults: 0,
            peak_working_set: 0,
            working_set: 0,
            quota_peak_paged: 0,
            quota_paged: 0,
            quota_peak_nonpaged: 0,
            quota_nonpaged: 0,
            pagefile: 0,
            peak_pagefile: 0,
            private: 0,
        };
        // SAFETY: `c` is a correctly sized, writable PROCESS_MEMORY_COUNTERS_EX
        // and the pseudo handle of the current process is always valid.
        let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
        assert_ne!(ok, 0);
        (c.private as f64 / 1048576.0, c.working_set as f64 / 1048576.0)
    };
    let (p0, w0) = read();
    let t = Instant::now();
    let mut r = Renderer::new(FontConfig::default());
    let new_ms = t.elapsed().as_secs_f64() * 1000.0;
    let (bar, _) = bar_list("Tue 24 Nov 12:00:00");
    let mut canvas = Canvas::new(1920, 32);
    let t = Instant::now();
    r.render(&bar, &mut canvas.pixels(), None).unwrap();
    let first_ms = t.elapsed().as_secs_f64() * 1000.0;
    let (p1, w1) = read();
    eprintln!(
        "renderer memory ({} session, {}): before private {p0:.1} MB working set {w0:.1} MB; after Renderer::new ({new_ms:.1} ms) and the first 1920x32 frame ({first_ms:.1} ms) private {p1:.1} MB working set {w1:.1} MB; renderer adds {:.1} MB private",
        session_name(),
        if cfg!(debug_assertions) { "debug build" } else { "release build" },
        p1 - p0
    );
}

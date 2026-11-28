//! Outline drawing: path fills, borders, shadows and clips, and the two ways
//! bar segments meet without letting the desktop show through.

mod common;

use common::*;
use rsb_core::color::Color;
use rsb_core::draw::DrawItem;
use rsb_core::geom::RectF;
use rsb_render::{Canvas, render_offscreen};

/// Draws `items` on a transparent surface.
fn draw(w: u32, h: u32, items: Vec<DrawItem>) -> Canvas {
    let mut r = renderer();
    render_offscreen(&mut r, &list(w, h, items)).expect("render")
}

fn rgb(c: Color) -> [f32; 3] {
    [c.r * 255.0, c.g * 255.0, c.b * 255.0]
}

/// Largest channel distance between `p` and the closest point on the line
/// between the fills `a` and `b` (a blend of the two).
fn blend_distance(p: [f32; 3], a: [f32; 3], b: [f32; 3]) -> f32 {
    let ab = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let len2 = ab.iter().map(|v| v * v).sum::<f32>();
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p[0] - b[0]) * ab[0] + (p[1] - b[1]) * ab[1] + (p[2] - b[2]) * ab[2]) / len2).clamp(0.0, 1.0)
    };
    (0..3).map(|i| (p[i] - (b[i] + t * ab[i])).abs()).fold(0.0, f32::max)
}

struct Joint {
    /// Lowest alpha in the checked area (0..=255).
    min_alpha: u8,
    /// Largest distance from a blend of the two fills, composited over white.
    worst_blend: f32,
    checked: u32,
}

/// Looks at every pixel of `area`: its alpha, and how far its colour over
/// white is from a blend of the two fills.
fn inspect(c: &Canvas, area: RectF, a: Color, b: Color) -> Joint {
    let (fa, fb) = (rgb(a), rgb(b));
    let mut j = Joint { min_alpha: 255, worst_blend: 0.0, checked: 0 };
    for y in area.y as u32..area.bottom() as u32 {
        for x in area.x as u32..area.right() as u32 {
            let [r, g, bl, al] = c.rgba_at(x, y);
            let k = al as f32 / 255.0;
            let over_white = [r, g, bl].map(|v| v as f32 * k + 255.0 * (1.0 - k));
            j.min_alpha = j.min_alpha.min(al);
            j.worst_blend = j.worst_blend.max(blend_distance(over_white, fa, fb));
            j.checked += 1;
        }
    }
    j
}

const H: f32 = 26.0;
const D: f32 = 13.0;

/// Two butting segments: the earlier one spans x 10..110, the later one
/// starts at 97 (their overlap is the chevron depth).
fn pair(earlier: (Seg, f32, Color), later: (Seg, f32, Color)) -> Vec<DrawItem> {
    let mut items = segment(1, earlier.0, 10.0, 10.0, earlier.1, H, D, earlier.2);
    items.extend(segment(10, later.0, 97.0, 10.0, later.1, H, D, later.2));
    items
}

/// Where the two segments meet, with room for the anti-aliased edge.
fn joint_area() -> RectF {
    rect(94.0, 10.0, 24.0, H)
}

#[test]
fn point_into_notch_joint_with_the_earlier_segment_run_under_holds_the_fills() {
    let (a, b) = (col("#2f3639"), col("#1b1f22"));
    // the earlier segment runs 1.5 px under the later one
    let c = draw(220, 50, pair((Seg::Hex, 100.0 + 1.5, a), (Seg::NotchPoint, 90.0, b)));
    let j = inspect(&c, joint_area(), a, b);
    eprintln!(
        "point-into-notch joint: {} px, lowest alpha {}, worst blend distance {:.2}",
        j.checked, j.min_alpha, j.worst_blend
    );
    assert!(j.min_alpha >= 250, "desktop shows through: alpha {}", j.min_alpha);
    assert!(j.worst_blend <= 8.0, "colour is {:.1} levels from any blend of the fills", j.worst_blend);
}

#[test]
fn flat_end_under_a_point_joint_holds_the_fills() {
    let (a, b) = (col("#1b1f22"), col("#2f3639"));
    // the earlier segment is flat on its right; the later one's point draws the joint
    let c = draw(220, 50, pair((Seg::PointFlat, 100.0, a), (Seg::Hex, 90.0, b)));
    let j = inspect(&c, joint_area(), a, b);
    eprintln!(
        "flat-under-point joint: {} px, lowest alpha {}, worst blend distance {:.2}",
        j.checked, j.min_alpha, j.worst_blend
    );
    assert!(j.min_alpha >= 250, "desktop shows through: alpha {}", j.min_alpha);
    assert!(j.worst_blend <= 8.0, "colour is {:.1} levels from any blend of the fills", j.worst_blend);
}

#[test]
fn point_into_notch_joint_without_the_run_under_leaks() {
    // control: the same shapes with no run under put two anti-aliased edges
    // on one line, which lets part of the desktop through; if this passed the
    // joint tests above would prove nothing
    let (a, b) = (col("#2f3639"), col("#1b1f22"));
    let c = draw(220, 50, pair((Seg::Hex, 100.0, a), (Seg::NotchPoint, 90.0, b)));
    let j = inspect(&c, joint_area(), a, b);
    eprintln!("control joint: lowest alpha {}, worst blend distance {:.2}", j.min_alpha, j.worst_blend);
    assert!(j.min_alpha < 240, "control joint did not leak: alpha {}", j.min_alpha);
    assert!(j.worst_blend > 8.0, "control joint stayed within a blend: {:.1}", j.worst_blend);
}

#[test]
fn a_chevron_edge_is_a_clean_45_degree_line() {
    // one hexagon on transparent: the slanted edge's coverage rises across
    // exactly one to two pixels, and rows above and below the middle mirror
    let hex = fill_path(1, poly(&seg_points(Seg::Hex, 10.0, 10.0, 100.0, H, D)), col("#000000"));
    let c = draw(130, 50, vec![hex]);
    for dy in 1..12u32 {
        let up = (0..130).find(|&x| c.rgba_at(x, 23 - dy)[3] > 0).unwrap();
        let down = (0..130).find(|&x| c.rgba_at(x, 22 + dy)[3] > 0).unwrap();
        assert_eq!(up, down, "left edge rows {} and {} differ", 23 - dy, 22 + dy);
    }
    // the left edge moves one pixel to the left for every row towards the middle
    let first = |y: u32| (0..130).find(|&x| c.rgba_at(x, y)[3] > 0).unwrap() as i32;
    assert_eq!(first(20) - first(22), 2);
}

#[test]
fn a_rectangle_path_equals_the_rectangle_fill() {
    let r = rect(10.25, 8.5, 60.5, 30.25);
    let c = col("#366382");
    let by_path = draw(
        90,
        50,
        vec![fill_path(1, poly(&[(r.x, r.y), (r.right(), r.y), (r.right(), r.bottom()), (r.x, r.bottom())]), c)],
    );
    let by_rect = draw(90, 50, vec![fill(1, r, 0.0, c)]);
    let worst = by_path.data.iter().zip(&by_rect.data).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
    assert!(worst <= 2, "path and rectangle differ by {worst}");
}

#[test]
fn a_border_path_leaves_the_inside_and_outside_alone() {
    let outer = rect(10.0, 10.0, 60.0, 40.0);
    let inner = rect(14.0, 13.0, 52.0, 34.0);
    let b = border_path(1, poly(&bevel_points(outer, 10.0)), poly(&bevel_points(inner, 8.0)), col("#E79B25"));
    let c = draw(80, 60, vec![b]);
    assert_eq!(c.rgba_at(40, 30)[3], 0, "inside is touched");
    assert_eq!(c.rgba_at(5, 30)[3], 0, "outside is touched");
    assert_eq!(c.rgba_at(11, 30)[3], 255, "left side of the ring is missing");
    assert_eq!(c.rgba_at(40, 11)[3], 255, "top side of the ring is missing");
    // the bevelled corner is cut: the outer corner pixel is empty
    assert_eq!(c.rgba_at(10, 10)[3], 0);
}

#[test]
fn a_ring_with_a_hole_is_even_odd() {
    // both outlines run the same way: only even-odd leaves the hole open
    let outer = rect(10.0, 10.0, 60.0, 40.0);
    let inner = rect(25.0, 20.0, 30.0, 20.0);
    let pts = |r: RectF| [(r.x, r.y), (r.right(), r.y), (r.right(), r.bottom()), (r.x, r.bottom())];
    let c = draw(80, 60, vec![border_path(1, poly(&pts(outer)), poly(&pts(inner)), col("#7D5168"))]);
    assert_eq!(c.rgba_at(40, 30)[3], 0);
    assert_eq!(c.rgba_at(15, 30)[3], 255);
}

#[test]
fn a_path_shadow_is_darkest_under_the_shape_and_fades_out() {
    let r = rect(40.0, 40.0, 60.0, 40.0);
    let c = draw(140, 120, vec![shadow_path(1, poly(&bevel_points(r, 10.0)), 12.0, col("#000000"))]);
    let centre = c.rgba_at(70, 60)[3];
    let edge = c.rgba_at(40, 60)[3];
    let near = c.rgba_at(30, 60)[3];
    let far = c.rgba_at(10, 60)[3];
    eprintln!("path shadow alpha: centre {centre}, edge {edge}, 10 px out {near}, 30 px out {far}");
    assert!(centre >= 250, "centre {centre}");
    assert!((100..=160).contains(&edge), "edge {edge}");
    assert!(near < edge && near > far);
    assert_eq!(far, 0, "kernel cut at 3 sigma: nothing 30 px out for blur 12");
}

#[test]
fn a_rectangle_path_shadow_matches_the_box_shadow() {
    let r = rect(40.0, 40.0, 60.0, 40.0);
    let pts = [(r.x, r.y), (r.right(), r.y), (r.right(), r.bottom()), (r.x, r.bottom())];
    let by_path = draw(140, 120, vec![shadow_path(1, poly(&pts), 10.0, col("#00000099"))]);
    let by_box = draw(140, 120, vec![shadow(1, r, 0.0, 10.0, col("#00000099"))]);
    let worst = by_path.data.iter().zip(&by_box.data).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
    assert!(worst <= 3, "path shadow and box shadow differ by {worst}");
}

#[test]
fn a_clip_path_cuts_what_follows_and_pop_restores() {
    let tri = poly(&[(10.0, 10.0), (70.0, 10.0), (40.0, 60.0)]);
    let items = vec![
        push_clip_path(1, tri.clone()),
        fill(2, rect(0.0, 0.0, 100.0, 100.0), 0.0, col("#366382")),
        pop_clip(1, tri.bounds().outset(1.0)),
        fill(3, rect(80.0, 70.0, 10.0, 10.0), 0.0, col("#E79B25")),
    ];
    let c = draw(100, 100, items);
    assert_eq!(c.rgba_at(40, 30)[3], 255, "inside the triangle");
    assert_eq!(c.rgba_at(12, 55)[3], 0, "outside, below the left edge");
    assert_eq!(c.rgba_at(90, 20)[3], 0, "outside, right of the shape");
    assert_eq!(c.rgba_at(85, 75)[3], 255, "drawn after the pop");
    // the slanted edge is anti-aliased: some pixel is partly covered
    let partial = (0..100).any(|x| (1..254).contains(&c.rgba_at(x, 30)[3]));
    assert!(partial);
}

#[test]
fn a_clip_path_cuts_a_shadow_too() {
    let clip = poly(&[(20.0, 20.0), (100.0, 20.0), (100.0, 80.0), (20.0, 80.0)]);
    let items = vec![
        push_clip_path(1, clip.clone()),
        shadow(2, rect(10.0, 10.0, 60.0, 60.0), 0.0, 20.0, col("#000000")),
        pop_clip(1, clip.bounds().outset(1.0)),
    ];
    let c = draw(130, 110, items);
    assert_eq!(c.rgba_at(5, 40)[3], 0, "shadow left of the clip");
    assert_eq!(c.rgba_at(40, 5)[3], 0, "shadow above the clip");
    assert!(c.rgba_at(40, 40)[3] > 200);
}

#[test]
fn an_empty_or_degenerate_outline_clips_everything_away() {
    let line = poly(&[(10.0, 10.0), (60.0, 10.0)]);
    let items = vec![
        push_clip_path(1, line.clone()),
        fill(2, rect(0.0, 0.0, 80.0, 80.0), 0.0, col("#366382")),
        pop_clip(1, line.bounds().outset(1.0)),
    ];
    let c = draw(80, 80, items);
    assert!(c.data.iter().all(|b| *b == 0));
}

#[test]
fn outlines_with_non_finite_points_do_not_panic() {
    let bad = poly(&[(f32::NAN, 0.0), (10.0, f32::INFINITY), (5.0, 5.0)]);
    let items = vec![
        fill_path(1, bad.clone(), col("#366382")),
        shadow_path(2, bad.clone(), 4.0, col("#000000")),
        border_path(3, bad.clone(), bad.clone(), col("#000000")),
        push_clip_path(4, bad),
        fill(5, rect(0.0, 0.0, 10.0, 10.0), 0.0, col("#ffffff")),
        pop_clip(4, rect(0.0, 0.0, 10.0, 10.0)),
    ];
    let _ = draw(20, 20, items);
}

#[test]
fn partial_redraw_of_path_commands_equals_the_full_frame() {
    // scene_paths holds every path command, with clips and a shape reaching
    // past the surface; redraw random regions over the frame and compare
    let scene = scene_paths();
    let mut r = renderer();
    let full = render_offscreen(&mut r, &scene.list).unwrap();
    let mut canvas = Canvas::new(scene.list.width, scene.list.height);
    let mut state = 7u64;
    for _ in 0..40 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let x0 = (state >> 33) as i32 % 300;
        let y0 = (state >> 40) as i32 % 150;
        let w = 10 + (state >> 20) as i32 % 120;
        let h = 10 + (state >> 27) as i32 % 80;
        let region = rsb_core::geom::RectPx::new(x0, y0, (x0 + w).min(400), (y0 + h).min(200));
        r.render(&scene.list, &mut canvas.pixels(), Some(region)).unwrap();
        for y in region.y0..region.y1 {
            for x in region.x0..region.x1 {
                assert_eq!(canvas.rgba_at(x as u32, y as u32), full.rgba_at(x as u32, y as u32), "pixel {x},{y}");
            }
        }
    }
}

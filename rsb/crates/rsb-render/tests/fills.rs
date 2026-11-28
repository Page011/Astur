//! Big solid boxes: the inside is written directly and only the ring around
//! it goes through the path fill. The result has to match the plain path
//! fill, and a partial redraw has to match a full one.

mod common;

use common::*;
use rsb_core::draw::{DrawCmd, DrawItem, Fill};
use rsb_core::geom::{Radii, RectF, RectPx};
use rsb_core::pixels::PixelsMut;
use rsb_render::{Canvas, render_offscreen};

const W: u32 = 420;
const H: u32 = 300;

fn boxed(n: u64, r: RectF, radii: Radii, c: &str, as_gradient: bool) -> DrawItem {
    let color = col(c);
    let fill = if as_gradient {
        // the same colour as a gradient: drawn by the ordinary path fill
        Fill::Linear {
            start: rsb_core::geom::PointF { x: 0.0, y: 0.0 },
            end: rsb_core::geom::PointF { x: 1.0, y: 0.0 },
            stops: vec![(0.0, color), (1.0, color)],
        }
    } else {
        Fill::Solid(color)
    };
    item(n, r.outset(1.0), DrawCmd::Fill { rect: r, radii, fill })
}

/// Boxes in every shape the split has to get right.
fn items(as_gradient: bool) -> Vec<DrawItem> {
    let b = |n, r, radii, c| boxed(n, r, radii, c, as_gradient);
    vec![
        b(1, rect(0.0, 0.0, W as f32, H as f32), Radii::ZERO, "#0f1317"),
        // whole-pixel edges, round corners
        b(2, rect(10.0, 10.0, 200.0, 120.0), Radii::all(12.0), "#366382"),
        // fractional edges
        b(3, rect(20.4, 40.6, 180.3, 90.2), Radii::all(9.5), "#E79B2599"),
        // different radii per corner, a square top
        b(4, rect(230.0, 10.0, 160.0, 140.0), Radii { tl: 0.0, tr: 0.0, br: 20.0, bl: 20.0 }, "#7D5168"),
        b(5, rect(230.5, 160.25, 150.0, 100.0), Radii { tl: 30.0, tr: 0.0, br: 5.0, bl: 14.0 }, "#394855cc"),
        // radii larger than the box allows
        b(6, rect(20.0, 150.0, 120.0, 60.0), Radii::all(500.0), "#81cbea"),
        // reaches past the surface
        b(7, rect(-30.0, 220.0, 120.0, 100.0), Radii::all(16.0), "#E79B25"),
        // translucent over the others
        b(8, rect(60.0, 60.0, 300.0, 200.0), Radii::all(24.0), "#ffffff40"),
        // inside a rounded clip and a group opacity
        push_clip(10, rect(150.0, 140.0, 120.0, 130.0), 18.0),
        b(11, rect(140.0, 130.0, 160.0, 160.0), Radii::all(10.0), "#1b1f22"),
        pop_clip(10, rect(149.0, 139.0, 122.0, 132.0)),
        push_opacity(12, 0.5, rect(0.0, 0.0, W as f32, H as f32)),
        b(13, rect(300.0, 200.0, 100.0, 80.0), Radii::all(6.0), "#ff0000"),
        pop_opacity(12, rect(0.0, 0.0, W as f32, H as f32)),
    ]
}

fn render(items: Vec<DrawItem>) -> Canvas {
    let mut r = renderer();
    render_offscreen(&mut r, &list(W, H, items)).unwrap()
}

#[test]
fn the_split_box_matches_the_plain_path_fill() {
    let split = render(items(false));
    let plain = render(items(true));
    let mut worst = 0i32;
    let mut off = 0usize;
    for (a, b) in split.data.iter().zip(&plain.data) {
        let d = (*a as i32 - *b as i32).abs();
        worst = worst.max(d);
        off += (d > 1) as usize;
    }
    eprintln!(
        "split box vs plain path fill: worst channel difference {worst}, bytes off by more than 1: {off} of {}",
        split.data.len()
    );
    assert!(worst <= 2, "worst difference {worst}");
}

#[test]
fn a_partial_redraw_of_split_boxes_equals_the_full_frame() {
    let l = list(W, H, items(false));
    let mut r = renderer();
    let mut full = Canvas::new(W, H);
    r.render(&l, &mut full.pixels(), None).unwrap();
    let regions = [
        RectPx::new(0, 0, 60, 60),
        RectPx::new(5, 5, 200, 140),
        RectPx::new(100, 100, 101, 101),
        RectPx::new(215, 0, 420, 300),
        RectPx::new(140, 130, 300, 290),
        RectPx::new(0, 200, 120, 300),
        RectPx::new(299, 199, 402, 281),
        RectPx::new(-10, -10, 25, 25),
        RectPx::new(60, 70, 61, 250),
    ];
    for region in regions {
        let mut data = vec![0xABu8; W as usize * H as usize * 4];
        r.render(&l, &mut PixelsMut::new(&mut data, W, H).unwrap(), Some(region)).unwrap();
        let cut = region.intersect(&RectPx::new(0, 0, W as i32, H as i32)).unwrap();
        for y in 0..H as i32 {
            for x in 0..W as i32 {
                let i = (y as usize * W as usize + x as usize) * 4;
                let inside = x >= cut.x0 && x < cut.x1 && y >= cut.y0 && y < cut.y1;
                if inside {
                    assert_eq!(&data[i..i + 4], &full.data[i..i + 4], "region {region:?} differs at ({x},{y})");
                } else {
                    assert_eq!(&data[i..i + 4], &[0xAB; 4], "region {region:?} touched ({x},{y})");
                }
            }
        }
    }
}

#[test]
fn the_inside_of_an_opaque_box_is_its_colour() {
    let c = render(vec![boxed(1, rect(10.0, 10.0, 200.0, 120.0), Radii::all(12.0), "#366382", false)]);
    assert_eq!(c.rgba_at(100, 70), [0x36, 0x63, 0x82, 255]);
    assert_eq!(c.rgba_at(10, 70), [0x36, 0x63, 0x82, 255]);
    assert_eq!(c.rgba_at(209, 70), [0x36, 0x63, 0x82, 255]);
    assert_eq!(c.rgba_at(100, 10), [0x36, 0x63, 0x82, 255]);
    assert_eq!(c.rgba_at(9, 70)[3], 0);
    assert_eq!(c.rgba_at(10, 10)[3], 0, "the rounded corner is cut");
}

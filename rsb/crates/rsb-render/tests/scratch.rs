//! One scratch canvas per surface size: a bar and a panel drawn in turn do
//! not allocate each other's buffers again.

mod common;

use common::*;
use rsb_core::draw::DrawList;
use rsb_core::geom::RectPx;
use rsb_render::{Canvas, FontConfig, Renderer};

/// A list that uses clips and a group opacity, so masks and layers are needed.
fn busy(w: u32, h: u32) -> DrawList {
    let (wf, hf) = (w as f32, h as f32);
    let items = vec![
        fill(1, rect(0.0, 0.0, wf, hf), 0.0, col("#1b1f22")),
        push_clip(2, rect(4.0, 4.0, wf - 8.0, hf - 8.0), 6.0),
        push_opacity(3, 0.8, rect(0.0, 0.0, wf, hf)),
        fill(4, rect(10.0, 10.0, wf / 2.0, hf / 2.0), 8.0, col("#366382")),
        fill(5, rect(20.0, 20.0, wf / 2.0, hf / 2.0), 8.0, col("#E79B25")),
        pop_opacity(3, rect(0.0, 0.0, wf, hf)),
        pop_clip(2, rect(0.0, 0.0, wf, hf)),
    ];
    list(w, h, items)
}

fn render(r: &mut Renderer, l: &DrawList, region: Option<RectPx>) {
    let mut c = Canvas::new(l.width, l.height);
    r.render(l, &mut c.pixels(), region).unwrap();
}

#[test]
fn a_released_surface_gives_back_its_buffers_and_the_shadow_blur_with_them() {
    let mut r = Renderer::new(FontConfig::default());
    let bar = busy(1920, 36);
    render(&mut r, &bar, None);
    let bar_only = r.stats();
    assert_eq!(bar_only.shadow_bytes, 0, "a bar with no shadow keeps no blur buffers");
    // A panel with a shadow: the blur buffers and a surface of its own.
    let mut shadowed = busy(500, 560);
    shadowed.items.insert(1, shadow(99, rect(40.0, 46.0, 400.0, 400.0), 8.0, 18.0, col("#000000")));
    render(&mut r, &shadowed, None);
    let open = r.stats();
    assert_eq!(open.scratch_surfaces, 2);
    assert!(open.shadow_bytes > 0, "the blur kept its buffers: {open:?}");
    assert!(open.scratch_bytes > bar_only.scratch_bytes);
    r.release_surface(500, 560);
    let closed = r.stats();
    assert_eq!(closed.scratch_surfaces, 1);
    assert_eq!(closed.scratch_bytes, bar_only.scratch_bytes, "back to what the bar alone holds");
    assert_eq!(closed.shadow_bytes, 0);
    eprintln!(
        "scratch: bar alone {} B; with a 500x560 panel and its shadow {} B (blur buffers {} B); after release {} B (blur buffers {} B)",
        bar_only.scratch_bytes, open.scratch_bytes, open.shadow_bytes, closed.scratch_bytes, closed.shadow_bytes
    );
    // The bar still draws, and releasing a size nobody drew is harmless.
    r.release_surface(7, 7);
    render(&mut r, &bar, Some(RectPx::new(100, 4, 400, 30)));
}

#[test]
fn alternating_surface_sizes_do_not_allocate_again() {
    let mut r = Renderer::new(FontConfig::default());
    let (bar, panel) = (busy(1920, 36), busy(500, 560));
    // the first rounds build both sets of buffers (a partial redraw adds a mask)
    for _ in 0..2 {
        render(&mut r, &bar, None);
        render(&mut r, &panel, None);
        render(&mut r, &bar, Some(RectPx::new(100, 4, 400, 30)));
        render(&mut r, &panel, Some(RectPx::new(40, 40, 300, 300)));
    }
    let warm = r.stats();
    assert_eq!(warm.scratch_surfaces, 2);
    for i in 0..50 {
        render(&mut r, &bar, None);
        render(&mut r, &panel, None);
        render(&mut r, &bar, Some(RectPx::new(100, 4, 400, 30)));
        render(&mut r, &panel, Some(RectPx::new(40, 40, 300, 300)));
        let now = r.stats();
        assert_eq!(now.scratch_allocations, warm.scratch_allocations, "round {i} allocated");
        assert_eq!(now.scratch_bytes, warm.scratch_bytes);
    }
    eprintln!(
        "scratch for a 1920x36 bar and a 500x560 panel: {} surfaces, {:.2} MB, {} buffers made in total over 204 frames",
        warm.scratch_surfaces,
        warm.scratch_bytes as f64 / 1048576.0,
        warm.scratch_allocations
    );
}

#[test]
fn only_the_four_most_recent_sizes_are_kept() {
    let mut r = Renderer::new(FontConfig::default());
    let sizes = [(100, 40), (200, 40), (300, 40), (400, 40)];
    for (w, h) in sizes {
        render(&mut r, &busy(w, h), None);
    }
    assert_eq!(r.stats().scratch_surfaces, 4);
    let kept = r.stats().scratch_allocations;
    // all four are still there: no new allocation
    for (w, h) in sizes {
        render(&mut r, &busy(w, h), None);
    }
    assert_eq!(r.stats().scratch_allocations, kept);
    // a fifth size evicts the least recently drawn (100 x 40)
    render(&mut r, &busy(500, 40), None);
    assert_eq!(r.stats().scratch_surfaces, 4);
    let after_fifth = r.stats().scratch_allocations;
    assert!(after_fifth > kept);
    render(&mut r, &busy(200, 40), None);
    assert_eq!(r.stats().scratch_allocations, after_fifth, "a kept size was rebuilt");
    render(&mut r, &busy(100, 40), None);
    assert!(r.stats().scratch_allocations > after_fifth, "the evicted size was not rebuilt");
    assert_eq!(r.stats().scratch_surfaces, 4);
}

#[test]
fn a_partial_redraw_after_other_sizes_still_matches_the_full_frame() {
    let mut r = Renderer::new(FontConfig::default());
    let panel = busy(500, 560);
    let mut full = Canvas::new(500, 560);
    r.render(&panel, &mut full.pixels(), None).unwrap();
    // draw other sizes in between, then redraw a region of the panel into a copy
    render(&mut r, &busy(1920, 36), None);
    let mut copy = Canvas::new(500, 560);
    copy.data.copy_from_slice(&full.data);
    let region = RectPx::new(30, 30, 260, 200);
    r.render(&panel, &mut copy.pixels(), Some(region)).unwrap();
    assert_eq!(copy.data, full.data);
    // and after the panel's buffers were evicted and rebuilt
    for w in [10u32, 20, 30, 40] {
        render(&mut r, &busy(w, 10), None);
    }
    r.render(&panel, &mut copy.pixels(), Some(region)).unwrap();
    assert_eq!(copy.data, full.data);
}

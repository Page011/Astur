//! Timings. They print numbers and assert a budget only in a release build
//! (`cargo test --release -p rsb-render --test bench -- --nocapture`).

mod common;

use std::sync::Arc;
use std::time::Instant;

use common::*;
use rsb_core::draw::{DrawItem, DrawList};
use rsb_core::geom::RectF;
use rsb_render::{Canvas, FontConfig, Renderer, SvgSource, parse_svg};

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn build() -> &'static str {
    if cfg!(debug_assertions) { "debug build" } else { "release build" }
}

type Page = fn(f32) -> Vec<DrawItem>;

fn renderer_with_icons() -> Renderer {
    let mut r = Renderer::new(FontConfig::default());
    r.register_font(bundled_mono(), None).unwrap();
    r.set_svg(Arc::from("bolt"), SVG_BOLT.as_bytes()).unwrap();
    r.set_svg(Arc::from("dial"), SVG_DIAL.as_bytes()).unwrap();
    r
}

/// Median and 95th percentile of a full redraw, ms.
fn frame_times(r: &mut Renderer, l: &DrawList, frames: usize) -> (f64, f64) {
    let mut canvas = Canvas::new(l.width, l.height);
    for _ in 0..if cfg!(debug_assertions) { 1 } else { 20 } {
        r.render(l, &mut canvas.pixels(), None).unwrap();
    }
    let mut t = Vec::with_capacity(frames);
    for _ in 0..frames {
        let start = Instant::now();
        r.render(l, &mut canvas.pixels(), None).unwrap();
        t.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    t.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (t[t.len() / 2], t[t.len() * 95 / 100])
}

#[test]
fn panel_frame_time() {
    let mut r = renderer_with_icons();
    let pages: [(&str, Page); 4] = [
        ("list page", page_list),
        ("tile page", page_tiles),
        ("list page with menu and tooltip", page_with_menu),
        ("stress page, 9 shadows", page_stress),
    ];
    let mut worst = (0.0f64, 0.0f64);
    for (i, (name, page)) in pages.iter().enumerate() {
        let (l1, l2) = (panel_list(1.0, *page), panel_list(2.0, *page));
        let (n1, n2) = if cfg!(debug_assertions) { (3, 3) } else { (200, 100) };
        let (m1, p1) = frame_times(&mut r, &l1, n1);
        let (m2, p2) = frame_times(&mut r, &l2, n2);
        eprintln!(
            "panel frame, {name} ({} session, {}): 500x560 at 96 dpi {} items, median {m1:.2} ms (p95 {p1:.2}); 1000x1120 at 192 dpi median {m2:.2} ms (p95 {p2:.2})",
            session_name(),
            build(),
            l1.items.len()
        );
        if i < 3 {
            worst = (worst.0.max(m1), worst.1.max(m2));
        }
    }
    eprintln!(
        "budgets 4 ms at 96 dpi and 8 ms at 192 dpi; worst designed page {:.2} ms and {:.2} ms",
        worst.0, worst.1
    );
    if !cfg!(debug_assertions) {
        assert!(worst.0 < 4.0, "96 dpi median {:.2} ms", worst.0);
        assert!(worst.1 < 8.0, "192 dpi median {:.2} ms", worst.1);
    }
}

#[test]
fn panel_frame_time_with_cold_caches() {
    // the first frame after a panel opens: layouts and rasters are made now
    let mut r = renderer_with_icons();
    let l = panel_list(1.0, page_list);
    let mut canvas = Canvas::new(l.width, l.height);
    let t = Instant::now();
    r.render(&l, &mut canvas.pixels(), None).unwrap();
    let first = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    r.render(&l, &mut canvas.pixels(), None).unwrap();
    let second = t.elapsed().as_secs_f64() * 1000.0;
    eprintln!("panel first frame ({}): {first:.2} ms, second {second:.2} ms", build());
}

/// About 0.85 MB of 10 000 small elements.
fn big_svg() -> String {
    let mut s = String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 400 400">"#);
    for i in 0..9_999 {
        s.push_str(&format!(
            r##"<rect x="{}.5" y="{}.25" width="{}.5" height="8.25" fill="#366382" stroke="#E79B25"/>"##,
            i % 390,
            (i / 40) % 390,
            2 + i % 7
        ));
    }
    s.push_str("</svg>");
    s
}

#[test]
fn user_svg_parse_and_first_raster() {
    let doc = big_svg();
    assert!(doc.len() <= 1 << 20, "{} bytes", doc.len());
    let mut parse = Vec::new();
    let mut raster = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let svg = parse_svg(doc.as_bytes(), SvgSource::User).expect("parses");
        parse.push(t.elapsed().as_secs_f64() * 1000.0);
        let mut r = Renderer::new(FontConfig::default());
        r.set_parsed_svg(Arc::from("big"), svg);
        let l = list(256, 256, vec![svg_item(1, RectF::new(0.0, 0.0, 256.0, 256.0), "big", None)]);
        let t = Instant::now();
        let mut canvas = Canvas::new(256, 256);
        r.render(&l, &mut canvas.pixels(), None).unwrap();
        raster.push(t.elapsed().as_secs_f64() * 1000.0);
        assert!(canvas.data.as_chunks::<4>().0.iter().any(|p| p[3] != 0));
    }
    eprintln!(
        "user SVG, {} bytes and 10000 elements ({}, {} session): parse median {:.1} ms, first raster at 256x256 median {:.1} ms (runs on the loader thread)",
        doc.len(),
        build(),
        session_name(),
        median(&mut parse),
        median(&mut raster)
    );
}

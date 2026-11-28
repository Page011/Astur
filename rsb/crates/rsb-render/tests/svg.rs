//! SVG icons and images: what they draw, what they may never do, and the
//! limits on files from users.

mod common;

use std::sync::Arc;

use common::*;
use rsb_core::color::Color;
use rsb_render::{
    Canvas, FontConfig, RenderError, Renderer, SVG_CACHE_MAX_BYTES, SVG_MAX_FILE_BYTES, SVG_MAX_NODES, SvgSource,
    parse_svg, render_offscreen,
};

fn key(s: &str) -> Arc<str> {
    Arc::from(s)
}

fn draw(r: &mut Renderer, w: u32, h: u32, k: &str, at: rsb_core::geom::RectF, tint: Option<Color>) -> Canvas {
    render_offscreen(r, &list(w, h, vec![svg_item(1, at, k, tint)])).unwrap()
}

fn inked(c: &Canvas) -> usize {
    c.data.as_chunks::<4>().0.iter().filter(|p| p[3] != 0).count()
}

/// `n` small rectangles in a document; the root is one more element.
fn many_rects(n: usize) -> String {
    let mut s = String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">"#);
    for i in 0..n {
        s.push_str(&format!(r##"<rect x="{}" y="{}" width="2" height="2" fill="#366382"/>"##, i % 98, (i / 98) % 98));
    }
    s.push_str("</svg>");
    s
}

// ------------------------------------------------------------------ drawing

#[test]
fn a_tinted_icon_paints_its_colour_through_the_coverage_at_16_42_and_64_px() {
    let mut r = renderer();
    r.set_svg(key("t-bolt"), SVG_BOLT.as_bytes()).unwrap();
    let tint = col("#366382");
    let mut counts = Vec::new();
    for side in [16.0f32, 42.0, 64.0] {
        let c = draw(&mut r, 70, 70, "t-bolt", rect(2.0, 2.0, side, side), Some(tint));
        let n = inked(&c);
        assert!(n > 20, "{side} px icon drew {n} pixels");
        counts.push(n);
        // premultiplied BGRA: each channel is the tint scaled by the pixel's alpha
        for p in c.data.as_chunks::<4>().0.iter().filter(|p| p[3] != 0) {
            let a = p[3] as f32 / 255.0;
            for (got, want) in [(p[2], tint.r), (p[1], tint.g), (p[0], tint.b)] {
                assert!((got as f32 - want * 255.0 * a).abs() <= 1.6, "pixel {p:?} is not the tint at alpha {a}");
            }
        }
    }
    eprintln!("tinted icon ink pixels at 16, 42, 64 px: {counts:?}");
    assert!(counts[0] < counts[1] && counts[1] < counts[2]);
}

#[test]
fn a_tint_with_alpha_scales_the_coverage() {
    let mut r = renderer();
    r.set_svg(key("a-bolt"), SVG_BOLT.as_bytes()).unwrap();
    let solid = draw(&mut r, 50, 50, "a-bolt", rect(2.0, 2.0, 42.0, 42.0), Some(col("#ffffff")));
    let half = draw(&mut r, 50, 50, "a-bolt", rect(2.0, 2.0, 42.0, 42.0), Some(col("#ffffff80")));
    let (a, b) = (solid.data.as_chunks::<4>().0, half.data.as_chunks::<4>().0);
    let max_solid = a.iter().map(|p| p[3]).max().unwrap();
    let max_half = b.iter().map(|p| p[3]).max().unwrap();
    assert_eq!(max_solid, 255);
    assert!((126..=130).contains(&max_half), "half alpha tint peaks at {max_half}");
}

#[test]
fn a_new_tint_does_not_rasterise_again() {
    let mut r = Renderer::new(FontConfig::default());
    r.set_svg(key("bolt"), SVG_BOLT.as_bytes()).unwrap();
    let a = draw(&mut r, 50, 50, "bolt", rect(2.0, 2.0, 42.0, 42.0), Some(col("#366382")));
    let after_first = r.stats();
    let b = draw(&mut r, 50, 50, "bolt", rect(2.0, 2.0, 42.0, 42.0), Some(col("#E79B25")));
    let after_second = r.stats();
    assert_eq!(after_first.svg_rasters, 1);
    assert_eq!(after_second.svg_rasters, 1, "a colour change made another raster");
    assert_eq!(after_second.svg_raster_bytes, after_first.svg_raster_bytes);
    assert_ne!(a.data, b.data);
    assert_eq!(inked(&a), inked(&b));
}

#[test]
fn an_image_keeps_its_own_colours_and_aspect_ratio() {
    let mut r = renderer();
    r.set_svg(key("pic"), SVG_PICTURE.as_bytes()).unwrap();
    // 48 x 32 drawn at its own size
    let c = draw(&mut r, 60, 40, "pic", rect(0.0, 0.0, 48.0, 32.0), None);
    assert_eq!(c.rgba_at(14, 12), [255, 255, 255, 255], "the white circle");
    let plum = c.rgba_at(24, 26);
    assert!(
        plum[3] == 255
            && (plum[0] as i32 - 0x7D).abs() <= 2
            && (plum[1] as i32 - 0x51).abs() <= 2
            && (plum[2] as i32 - 0x68).abs() <= 2,
        "plum {plum:?}"
    );
    // into a square twice as big: scaled by 2, centred vertically
    let c = draw(&mut r, 100, 100, "pic", rect(0.0, 0.0, 96.0, 96.0), None);
    assert_eq!(c.rgba_at(28, 16 + 24), [255, 255, 255, 255]);
    assert_eq!(c.rgba_at(50, 5)[3], 0, "above the centred image");
    assert_eq!(c.rgba_at(50, 95)[3], 0, "below the centred image");
}

#[test]
fn a_missing_key_and_an_empty_rect_draw_nothing() {
    let mut r = renderer();
    assert_eq!(inked(&draw(&mut r, 40, 40, "no-such-svg", rect(0.0, 0.0, 30.0, 30.0), Some(col("#fff")))), 0);
    r.set_svg(key("e-bolt"), SVG_BOLT.as_bytes()).unwrap();
    assert_eq!(inked(&draw(&mut r, 40, 40, "e-bolt", rect(5.0, 5.0, 0.0, 30.0), Some(col("#fff")))), 0);
    assert_eq!(inked(&draw(&mut r, 40, 40, "e-bolt", rect(f32::NAN, 5.0, 30.0, 30.0), Some(col("#fff")))), 0);
    assert_eq!(inked(&draw(&mut r, 40, 40, "e-bolt", rect(0.0, 0.0, 1.0e9, 1.0e9), Some(col("#fff")))), 0);
}

#[test]
fn an_icon_is_cut_by_a_clip_and_by_the_item_bounds() {
    let mut r = renderer();
    r.set_svg(key("c-pic"), SVG_PICTURE.as_bytes()).unwrap();
    let items = vec![
        push_clip(1, rect(0.0, 0.0, 24.0, 32.0), 0.0),
        svg_item(2, rect(0.0, 0.0, 48.0, 32.0), "c-pic", None),
        pop_clip(1, rect(0.0, 0.0, 24.0, 32.0)),
    ];
    let c = render_offscreen(&mut r, &list(60, 40, items)).unwrap();
    assert_eq!(c.rgba_at(10, 10)[3], 255);
    assert_eq!(c.rgba_at(30, 10)[3], 0, "right of the clip");
}

#[test]
fn replacing_an_svg_drops_its_old_rasters() {
    let mut r = Renderer::new(FontConfig::default());
    r.set_svg(key("k"), SVG_BOLT.as_bytes()).unwrap();
    draw(&mut r, 50, 50, "k", rect(0.0, 0.0, 40.0, 40.0), Some(col("#fff")));
    assert_eq!(r.stats().svg_rasters, 1);
    r.set_svg(key("k"), SVG_DIAL.as_bytes()).unwrap();
    assert_eq!(r.stats().svg_rasters, 0);
    assert_eq!(r.stats().svg_raster_bytes, 0);
    assert!(r.has_svg("k") && !r.has_svg("other"));
}

// ----------------------------------------------------------------- the rules

#[test]
fn an_svg_never_reads_another_file() {
    let here = env!("CARGO_MANIFEST_DIR").replace('\\', "/");
    let picture = format!("{here}/tests/golden/shapes.png");
    assert!(std::path::Path::new(&picture).exists());
    let png1x1 = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
    let hrefs = [
        format!("file:///{picture}"),
        picture.clone(),
        "tests/golden/shapes.png".to_string(),
        "../golden/shapes.png".to_string(),
        "https://example.invalid/a.png".to_string(),
        png1x1.to_string(),
    ];
    for source in [SvgSource::Bundled, SvgSource::User] {
        for href in &hrefs {
            let doc = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="40" height="40"><image x="0" y="0" width="40" height="40" href="{href}" xlink:href="{href}"/></svg>"#
            );
            let Ok(svg) = parse_svg(doc.as_bytes(), source) else { continue };
            let mut r = renderer();
            r.set_parsed_svg(key("href-test"), svg);
            let c = draw(&mut r, 50, 50, "href-test", rect(0.0, 0.0, 40.0, 40.0), None);
            assert_eq!(inked(&c), 0, "an image href drew something ({source:?}, {})", &href[..12.min(href.len())]);
        }
    }
    // a `<use>` of another file's element
    let doc = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="40" height="40"><use xlink:href="{picture}#a"/><use href="{picture}"/></svg>"##
    );
    if let Ok(svg) = parse_svg(doc.as_bytes(), SvgSource::User) {
        let mut r = renderer();
        r.set_parsed_svg(key("use-test"), svg);
        let c = draw(&mut r, 50, 50, "use-test", rect(0.0, 0.0, 40.0, 40.0), None);
        assert_eq!(inked(&c), 0);
    }
}

#[test]
fn text_in_an_svg_is_not_drawn() {
    let doc = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="30"><text x="2" y="24" font-size="24" fill="#000">Hello</text></svg>"##;
    let mut r = renderer();
    if r.set_svg(key("text-svg"), doc.as_bytes()).is_ok() {
        assert_eq!(inked(&draw(&mut r, 90, 40, "text-svg", rect(0.0, 0.0, 80.0, 30.0), None)), 0);
    }
}

#[test]
fn a_user_svg_over_the_element_limit_is_refused() {
    assert_eq!(SVG_MAX_NODES, 10_000);
    // the root counts as one element
    assert!(parse_svg(many_rects(SVG_MAX_NODES - 1).as_bytes(), SvgSource::User).is_ok());
    let over = parse_svg(many_rects(SVG_MAX_NODES).as_bytes(), SvgSource::User);
    assert!(matches!(&over, Err(RenderError::Svg(m)) if m.contains("limit")), "{over:?}");
    // a bundled icon is not counted: it is ours
    assert!(parse_svg(many_rects(SVG_MAX_NODES + 500).as_bytes(), SvgSource::Bundled).is_ok());
}

#[test]
fn elements_made_by_use_count_as_elements() {
    // 200 rectangles used 100 times: 20 000 drawn from a 310 element file
    let mut s = String::from(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><defs><g id="a">"##);
    for i in 0..200 {
        s.push_str(&format!(r#"<rect x="{}" y="1" width="1" height="1"/>"#, i % 90));
    }
    s.push_str("</g></defs>");
    for i in 0..100 {
        s.push_str(&format!(r##"<use href="#a" y="{}"/>"##, i % 50));
    }
    s.push_str("</svg>");
    assert!(s.len() < SVG_MAX_FILE_BYTES);
    let r = parse_svg(s.as_bytes(), SvgSource::User);
    assert!(matches!(r, Err(RenderError::Svg(_))), "{r:?}");
}

#[test]
fn nested_copies_cannot_grow_a_small_file_into_a_huge_one() {
    // each level uses the one before it twice: 2^30 elements from a few hundred bytes
    let mut s = String::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><defs><rect id="l0" width="1" height="1"/>"##,
    );
    for i in 1..30 {
        s.push_str(&format!(r##"<g id="l{i}"><use href="#l{}"/><use href="#l{}" x="1"/></g>"##, i - 1, i - 1));
    }
    s.push_str(r##"</defs><use href="#l29"/></svg>"##);
    let t = std::time::Instant::now();
    let r = parse_svg(s.as_bytes(), SvgSource::User);
    assert!(matches!(r, Err(RenderError::Svg(_))), "{r:?}");
    assert!(t.elapsed().as_millis() < 2000, "took {:?}", t.elapsed());
}

#[test]
fn entities_cycles_and_deep_nesting_are_handled() {
    let entity = r##"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;">]><svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><desc>&b;</desc></svg>"##;
    assert!(matches!(parse_svg(entity.as_bytes(), SvgSource::User), Err(RenderError::Svg(_))));
    // a `use` that points at its own parent ends (an error or an empty drawing, not a hang)
    let cycle =
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><g id="a"><use href="#a"/></g></svg>"##;
    let _ = parse_svg(cycle.as_bytes(), SvgSource::User);
    // 100 000 nested groups: refused without overflowing the stack
    let mut deep = String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">"#);
    deep.push_str(&"<g>".repeat(100_000));
    deep.push_str(&"</g>".repeat(100_000));
    deep.push_str("</svg>");
    assert!(matches!(parse_svg(deep.as_bytes(), SvgSource::User), Err(RenderError::Svg(_))));
}

#[test]
fn a_user_svg_with_a_filter_is_refused_and_a_bundled_one_is_not() {
    let filter = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20"><filter id="f"><feGaussianBlur stdDeviation="2"/></filter><rect width="10" height="10" filter="url(#f)"/></svg>"##;
    let user = parse_svg(filter.as_bytes(), SvgSource::User);
    assert!(matches!(&user, Err(RenderError::Svg(m)) if m.contains("filter")), "{user:?}");
    assert!(parse_svg(filter.as_bytes(), SvgSource::Bundled).is_ok());
    // a filter reached only through a mask is found too
    let masked = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20"><filter id="f"><feOffset dx="1"/></filter><mask id="m"><rect width="10" height="10" fill="#fff" filter="url(#f)"/></mask><rect width="20" height="20" mask="url(#m)"/></svg>"##;
    assert!(matches!(parse_svg(masked.as_bytes(), SvgSource::User), Err(RenderError::Svg(_))));
}

#[test]
fn big_and_broken_files_are_refused() {
    let big = vec![b' '; SVG_MAX_FILE_BYTES + 1];
    assert!(matches!(parse_svg(&big, SvgSource::Bundled), Err(RenderError::Svg(_))));
    assert!(matches!(parse_svg(&[0xff, 0xfe, 0xfd], SvgSource::User), Err(RenderError::Svg(_))));
    assert!(matches!(parse_svg(b"<svg", SvgSource::User), Err(RenderError::Svg(_))));
    assert!(matches!(parse_svg(b"", SvgSource::Bundled), Err(RenderError::Svg(_))));
    assert!(matches!(parse_svg(b"<html></html>", SvgSource::User), Err(RenderError::Svg(_))));
}

#[test]
fn a_parsed_svg_crosses_threads() {
    let svg = std::thread::spawn(|| parse_svg(SVG_BOLT.as_bytes(), SvgSource::User).unwrap()).join().unwrap();
    assert_eq!(svg.size(), (24.0, 24.0));
    let mut r = renderer();
    assert_eq!(r.set_parsed_svg(key("from-thread"), svg), (24.0, 24.0));
    assert!(r.has_svg("from-thread"));
    assert!(inked(&draw(&mut r, 40, 40, "from-thread", rect(4.0, 4.0, 32.0, 32.0), Some(col("#fff")))) > 50);
}

// -------------------------------------------------------------------- cache

#[test]
fn the_raster_cache_stays_under_its_cap() {
    let mut r = Renderer::new(FontConfig::default());
    r.set_svg(key("pic"), SVG_PICTURE.as_bytes()).unwrap();
    r.set_svg(key("bolt"), SVG_BOLT.as_bytes()).unwrap();
    let (w, h) = (900u32, 700u32);
    let mut peak = 0usize;
    // thirty sizes of an image in its own colours (4 bytes a pixel) and thirty of a tinted icon
    for i in 0..30u32 {
        let side = 300.0 + i as f32 * 20.0;
        let l = list(
            w,
            h,
            vec![
                svg_item(1, rect(0.0, 0.0, side, side * 2.0 / 3.0), "pic", None),
                svg_item(2, rect(0.0, 0.0, 20.0 + side / 2.0, 20.0 + side / 2.0), "bolt", Some(col("#fff"))),
            ],
        );
        render_offscreen(&mut r, &l).unwrap();
        let st = r.stats();
        peak = peak.max(st.svg_raster_bytes);
        assert!(st.svg_raster_bytes <= SVG_CACHE_MAX_BYTES, "{} bytes after size {i}", st.svg_raster_bytes);
    }
    let st = r.stats();
    eprintln!(
        "raster cache: {} rasters, {} bytes of {SVG_CACHE_MAX_BYTES} after 60 sizes, peak {peak}",
        st.svg_rasters, st.svg_raster_bytes
    );
    assert!(st.svg_rasters >= 2);
    assert!(peak > SVG_CACHE_MAX_BYTES / 2, "the cache was barely used: {peak}");
}

#[test]
fn a_raster_bigger_than_the_cache_is_not_made() {
    let mut r = Renderer::new(FontConfig::default());
    r.set_svg(key("pic"), SVG_PICTURE.as_bytes()).unwrap();
    // 2400 x 1600 in its own colours is 15 MB
    let c = render_offscreen(&mut r, &list(2400, 1600, vec![svg_item(1, rect(0.0, 0.0, 2400.0, 1600.0), "pic", None)]))
        .unwrap();
    assert_eq!(inked(&c), 0);
    assert_eq!(r.stats().svg_raster_bytes, 0);
}

#[test]
fn the_least_recently_drawn_raster_goes_first() {
    let mut r = Renderer::new(FontConfig::default());
    r.set_svg(key("pic"), SVG_PICTURE.as_bytes()).unwrap();
    let draw_at = |r: &mut Renderer, side: f32| {
        render_offscreen(r, &list(800, 600, vec![svg_item(1, rect(0.0, 0.0, side, side * 2.0 / 3.0), "pic", None)]))
            .unwrap();
        r.stats().svg_rasters_made
    };
    // each is about 1.3 MB: six fit under the cap, a seventh pushes one out
    let made: Vec<u64> = (0..6).map(|i| draw_at(&mut r, 700.0 + i as f32)).collect();
    assert_eq!(made, vec![1, 2, 3, 4, 5, 6]);
    assert_eq!(draw_at(&mut r, 700.0), 6, "a cached raster was made again");
    assert_eq!(draw_at(&mut r, 706.0), 7);
    let st = r.stats();
    assert!(st.svg_raster_bytes <= SVG_CACHE_MAX_BYTES);
    assert_eq!(st.svg_rasters, 6);
    // 700 was drawn again before 706 came, so 701 (the oldest) went
    assert_eq!(draw_at(&mut r, 700.0), 7, "the recently drawn raster was dropped");
    assert_eq!(draw_at(&mut r, 701.0), 8, "the oldest raster was kept");
}

// ------------------------------------------------------------------ cost limits on a user's file

const SVG_HEAD: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">"#;

/// `levels` masks, each drawing a rectangle masked by the next one, and `uses` rectangles masked by the first.
fn mask_chain(levels: usize, uses: usize) -> String {
    let mut s = String::from(SVG_HEAD);
    s.push_str("<defs>");
    for i in 0..levels {
        if i + 1 < levels {
            s.push_str(&format!(
                r##"<mask id="m{i}"><rect width="100" height="100" fill="white" mask="url(#m{})"/></mask>"##,
                i + 1
            ));
        } else {
            s.push_str(&format!(r#"<mask id="m{i}"><rect width="100" height="100" fill="white"/></mask>"#));
        }
    }
    s.push_str("</defs>");
    for i in 0..uses {
        s.push_str(&format!(r##"<rect x="{}" y="1" width="2" height="2" fill="#366382" mask="url(#m0)"/>"##, i % 90));
    }
    s.push_str("</svg>");
    s
}

fn nested_opacity(depth: usize) -> String {
    let mut s = String::from(SVG_HEAD);
    for _ in 0..depth {
        s.push_str(r#"<g opacity="0.9">"#);
    }
    s.push_str(r##"<rect width="100" height="100" fill="#366382"/>"##);
    for _ in 0..depth {
        s.push_str("</g>");
    }
    s.push_str("</svg>");
    s
}

/// `n` sibling groups with opacity, each holding one rectangle.
fn sibling_layers(n: usize) -> String {
    let mut s = String::from(SVG_HEAD);
    for i in 0..n {
        s.push_str(&format!(
            r##"<g opacity="0.5"><rect x="{}" y="{}" width="60" height="60" fill="#366382"/></g>"##,
            i % 40,
            i % 40
        ));
    }
    s.push_str("</svg>");
    s
}

#[test]
fn a_chain_of_masks_is_refused_however_small_the_file() {
    let doc = mask_chain(60, 50);
    assert!(doc.len() < 16 * 1024, "{} bytes", doc.len());
    let t = std::time::Instant::now();
    let r = parse_svg(doc.as_bytes(), SvgSource::User);
    assert!(matches!(&r, Err(RenderError::Svg(m)) if m.contains("layers") || m.contains("elements")), "{r:?}");
    eprintln!("mask chain of {} bytes refused in {:?}", doc.len(), t.elapsed());
    assert!(t.elapsed().as_millis() < 2000, "refusing took {:?}", t.elapsed());
    // A short chain is fine, and the file's own author is not limited.
    assert!(parse_svg(mask_chain(3, 5).as_bytes(), SvgSource::User).is_ok());
    assert!(parse_svg(doc.as_bytes(), SvgSource::Bundled).is_ok());
}

#[test]
fn a_reference_to_a_mask_counts_as_a_copy_of_it() {
    // 600 rectangles use one mask of 20 elements: 12 600 drawn from 640 elements.
    let mut s = String::from(SVG_HEAD);
    s.push_str(r#"<defs><mask id="m">"#);
    for i in 0..20 {
        s.push_str(&format!(r#"<rect x="{i}" width="1" height="100" fill="white"/>"#));
    }
    s.push_str("</mask></defs>");
    for i in 0..600 {
        s.push_str(&format!(r##"<rect x="{}" width="1" height="1" mask="url(#m)"/>"##, i % 99));
    }
    s.push_str("</svg>");
    let r = parse_svg(s.as_bytes(), SvgSource::User);
    assert!(matches!(&r, Err(RenderError::Svg(m)) if m.contains("elements")), "{r:?}");
    // The same through a style attribute and a clip path.
    let styled = s.replace(r##"mask="url(#m)""##, r##"style="mask:url(#m)""##);
    let r = parse_svg(styled.as_bytes(), SvgSource::User);
    assert!(matches!(&r, Err(RenderError::Svg(m)) if m.contains("elements")), "{r:?}");
}

#[test]
fn layers_nested_deeper_than_eight_are_refused_in_a_user_file() {
    assert!(parse_svg(nested_opacity(8).as_bytes(), SvgSource::User).is_ok());
    let r = parse_svg(nested_opacity(9).as_bytes(), SvgSource::User);
    assert!(matches!(&r, Err(RenderError::Svg(m)) if m.contains("nested layers")), "{r:?}");
    assert!(parse_svg(nested_opacity(60).as_bytes(), SvgSource::Bundled).is_ok());
}

#[test]
fn a_user_file_is_drawn_small_and_with_few_layers_a_bundled_one_is_not_limited() {
    let mut r = Renderer::new(FontConfig::default());
    r.set_parsed_svg(key("flat"), parse_svg(many_rects(50).as_bytes(), SvgSource::User).unwrap());
    r.set_parsed_svg(key("flat-bundled"), parse_svg(many_rects(50).as_bytes(), SvgSource::Bundled).unwrap());
    let side = |r: &mut Renderer, k: &str, s: f32| inked(&draw(r, 700, 700, k, rect(0.0, 0.0, s, s), None));
    let at_512 = side(&mut r, "flat", 512.0);
    assert!(at_512 > 0, "512 px is the largest side of a user file");
    let made = r.stats().svg_rasters_made;
    // a larger picture is stretched from a 512 px raster: its ink grows with its area (1.37 times here)
    let at_600 = side(&mut r, "flat", 600.0);
    assert!(at_600 * 100 > at_512 * 120 && at_600 * 100 < at_512 * 155, "{at_512} px of ink at 512, {at_600} at 600");
    assert_eq!(r.stats().svg_rasters_made, made, "it reuses the 512 px raster");
    assert!(side(&mut r, "flat", 690.0) > at_600, "and serves every larger size");
    assert_eq!(r.stats().svg_rasters_made, made);
    assert!(side(&mut r, "flat-bundled", 600.0) > 0, "a bundled file has no such limit");

    // Many layers: cheap at a small size, refused where layers x pixels is too much.
    r.set_parsed_svg(key("layers"), parse_svg(sibling_layers(40).as_bytes(), SvgSource::User).unwrap());
    assert!(side(&mut r, "layers", 64.0) > 0);
    let made = r.stats().svg_rasters_made;
    assert_eq!(side(&mut r, "layers", 400.0), 0, "41 layers of 400 x 400 px is not drawn");
    assert_eq!(r.stats().svg_rasters_made, made);
    // Replacing the file clears the refusal, so a lighter version draws.
    r.set_parsed_svg(key("layers"), parse_svg(sibling_layers(2).as_bytes(), SvgSource::User).unwrap());
    assert!(side(&mut r, "layers", 400.0) > 0);
}

#[test]
fn the_heaviest_user_file_that_is_accepted_draws_inside_the_frame_budget() {
    // 14 layers (12 siblings and a nested pair) is the most a 512 px raster allows; each covers the whole picture.
    let mut s = String::from(SVG_HEAD);
    for _ in 0..12 {
        s.push_str(r##"<g opacity="0.9"><rect width="100" height="100" fill="#366382"/></g>"##);
    }
    s.push_str(r##"<g opacity="0.9"><g opacity="0.9"><rect width="100" height="100" fill="#e79b25"/></g></g></svg>"##);
    let mut r = Renderer::new(FontConfig::default());
    r.set_parsed_svg(key("heavy"), parse_svg(s.as_bytes(), SvgSource::User).unwrap());
    let mut times = Vec::new();
    for _ in 0..5 {
        r.set_parsed_svg(key("heavy"), parse_svg(s.as_bytes(), SvgSource::User).unwrap());
        let t = std::time::Instant::now();
        let c = draw(&mut r, 512, 512, "heavy", rect(0.0, 0.0, 512.0, 512.0), None);
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        assert!(inked(&c) > 0, "drawn");
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    eprintln!(
        "heaviest accepted user svg at 512 px ({} session, {}): median {:.1} ms of rasterising",
        session_name(),
        if cfg!(debug_assertions) { "debug build" } else { "release build" },
        times[2]
    );
    if !cfg!(debug_assertions) {
        assert!(times[2] < 50.0, "{:.1} ms", times[2]);
    }
}

#[test]
fn user_pictures_that_overfill_the_raster_cache_settle_instead_of_redrawing_for_ever() {
    // 15 pictures of 450 x 450 px are 12 MB in their own colours; the cache holds 8 MB.
    let files = 15;
    let src = sibling_layers(15);
    let mut r = Renderer::new(FontConfig::default());
    let mut items = Vec::new();
    for i in 0..files {
        let k = format!("f{i}");
        r.set_parsed_svg(key(&k), parse_svg(src.as_bytes(), SvgSource::User).unwrap());
        let x = (i % 5) as f32 * 450.0;
        let y = (i / 5) as f32 * 450.0;
        items.push(svg_item(i as u64 + 1, rect(x, y, 450.0, 450.0), &k, None));
    }
    let l = list(2250, 1350, items);
    let mut frames = 0;
    loop {
        render_offscreen(&mut r, &l).unwrap();
        frames += 1;
        if !r.svg_pending() {
            break;
        }
        assert!(frames <= 20, "still redrawing after {frames} frames");
    }
    let st = r.stats();
    eprintln!("settled after {frames} frames: {} rasters, {} bytes", st.svg_rasters, st.svg_raster_bytes);
    assert!(st.svg_raster_bytes <= SVG_CACHE_MAX_BYTES);
    assert!(st.svg_rasters >= 8, "the pictures that fit are kept: {}", st.svg_rasters);
    // a settled surface makes no more rasters, however often it is drawn
    let made = st.svg_rasters_made;
    for _ in 0..5 {
        render_offscreen(&mut r, &l).unwrap();
        assert!(!r.svg_pending());
    }
    assert_eq!(r.stats().svg_rasters_made, made, "rasters were made again after it settled");
}

#[test]
fn many_user_files_do_not_hold_one_frame_and_all_appear_in_the_end() {
    // Each file needs most of a 4 million layer-pixel allowance at 96 px: a few ms apiece, so 100 of them in one
    // frame would hold it for about a second.
    let files = 100;
    let src = sibling_layers(400);
    let mut r = Renderer::new(FontConfig::default());
    let mut items = Vec::new();
    for i in 0..files {
        let k = format!("f{i}");
        r.set_parsed_svg(key(&k), parse_svg(src.as_bytes(), SvgSource::User).unwrap());
        let x = (i % 10) as f32 * 100.0;
        let y = (i / 10) as f32 * 100.0;
        items.push(svg_item(i as u64 + 1, rect(x, y, 96.0, 96.0), &k, None));
    }
    let l = list(1000, 1000, items);
    let mut frames = Vec::new();
    let last = loop {
        let t = std::time::Instant::now();
        let canvas = render_offscreen(&mut r, &l).unwrap();
        frames.push(t.elapsed().as_secs_f64() * 1000.0);
        if !r.svg_pending() {
            break canvas;
        }
        assert!(frames.len() < 200, "never finishes");
    };
    assert_eq!(r.stats().svg_rasters_made, files as u64, "every file is rasterised once");
    let c = last;
    for i in 0..files {
        let (x, y) = ((i % 10) * 100, (i / 10) * 100);
        let w = c.width as usize;
        let ink = (y..y + 96)
            .flat_map(|yy| (x..x + 96).map(move |xx| (xx, yy)))
            .filter(|&(xx, yy)| c.data[(yy * w + xx) * 4 + 3] != 0)
            .count();
        assert!(ink > 0, "file {i} is drawn in the last frame");
    }
    let worst = frames.iter().cloned().fold(0.0, f64::max);
    eprintln!("{} frames for {files} files, worst {worst:.1} ms ({} session)", frames.len(), session_name());
    if !cfg!(debug_assertions) {
        assert!(frames.len() > 3, "the work was spread over frames, not done in one: {} frames", frames.len());
        assert!(worst < 50.0, "worst frame {worst:.1} ms");
    }
}

static LOG_LINES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        LOG_LINES.lock().unwrap().push(r.args().to_string());
    }
    fn flush(&self) {}
}

#[test]
fn the_warnings_about_a_picture_name_its_folder_and_file_but_never_the_full_path() {
    static LOGGER: Capture = Capture;
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Warn);
    let home = r"C:\Users\Pat\AppData\Roaming\rsb\widgets";
    let file = |name: &str| format!(r"{home}\{name}\icon.svg");

    // too many layers for the size it is drawn at
    let heavy = file("layered");
    let mut r = Renderer::new(FontConfig::default());
    r.set_parsed_svg(key(&heavy), parse_svg(sibling_layers(40).as_bytes(), SvgSource::User).unwrap());
    assert_eq!(inked(&draw(&mut r, 700, 700, &heavy, rect(0.0, 0.0, 400.0, 400.0), None)), 0);

    // a granted file straight in the home folder: its folder is the account name
    let loose = r"C:\Users\Pat\logo.svg";
    r.set_parsed_svg(key(loose), parse_svg(sibling_layers(40).as_bytes(), SvgSource::User).unwrap());
    assert_eq!(inked(&draw(&mut r, 700, 700, loose, rect(0.0, 0.0, 400.0, 400.0), None)), 0);

    // more pictures on screen than the raster cache holds
    let src = sibling_layers(15);
    let mut items = Vec::new();
    for i in 0..15 {
        let k = file(&format!("w{i}"));
        r.set_parsed_svg(key(&k), parse_svg(src.as_bytes(), SvgSource::User).unwrap());
        items.push(svg_item(
            i as u64 + 1,
            rect((i % 5) as f32 * 450.0, (i / 5) as f32 * 450.0, 450.0, 450.0),
            &k,
            None,
        ));
    }
    let l = list(2250, 1350, items);
    for frame in 0.. {
        render_offscreen(&mut r, &l).unwrap();
        if !r.svg_pending() {
            break;
        }
        assert!(frame < 30, "still redrawing");
    }

    let lines: Vec<String> = LOG_LINES.lock().unwrap().iter().filter(|l| l.starts_with("svg '")).cloned().collect();
    assert!(lines.iter().any(|l| l.contains("too many layers")), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("fill the raster cache")), "{lines:?}");
    for l in &lines {
        assert!(!l.contains('\\'), "{l}");
        for private in ["C:", "Users", "Pat", "AppData", "Roaming", "widgets"] {
            assert!(!l.contains(private), "{private} in: {l}");
        }
    }
    assert!(lines.iter().any(|l| l.contains("layered/icon.svg")), "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with("svg 'logo.svg'")), "{lines:?}");
}

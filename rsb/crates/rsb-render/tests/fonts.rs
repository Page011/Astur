//! Registered fonts (the bundled JetBrains Mono) and caret stops.

mod common;

use std::sync::Arc;

use common::*;
use rsb_core::draw::TextStyle;
use rsb_core::layout::TextMeasure;
use rsb_core::style::TextAlign;
use rsb_render::{Canvas, FONT_MAX_FILE_BYTES, RenderError, Renderer, render_offscreen};

const MONO: &[&str] = &["JetBrains Mono", "monospace"];
const SANS: &[&str] = &["Segoe UI"];

fn font_bytes() -> Arc<[u8]> {
    bundled_mono()
}

/// The shared renderer with the bundled font registered.
fn with_mono() -> std::sync::MutexGuard<'static, Renderer> {
    let mut r = renderer();
    r.register_font(font_bytes(), None).expect("bundled font registers");
    r
}

fn draw_text(r: &mut Renderer, s: &str, st: &TextStyle) -> Canvas {
    let l = list(260, 30, vec![text(1, rect(2.0, 2.0, 250.0, 26.0), s, st, col("#ffffff"), TextAlign::Start)]);
    render_offscreen(r, &l).unwrap()
}

/// Sum of the alpha of every pixel: grows with the weight of the strokes.
fn ink(c: &Canvas) -> u64 {
    c.data.as_chunks::<4>().0.iter().map(|p| p[3] as u64).sum()
}

#[test]
fn the_bundled_font_registers_and_draws() {
    let mut r = with_mono();
    let names = r.register_font(font_bytes(), None).unwrap();
    assert!(names.iter().any(|n| n == "JetBrains Mono"), "family names: {names:?}");
    assert!(r.has_family("JetBrains Mono"));
    let st = style(MONO, 14.0, 400);
    let c = draw_text(&mut r, "Memory 42% CPU", &st);
    assert!(ink(&c) > 5_000, "ink {}", ink(&c));
}

#[test]
fn the_monospace_advance_at_14_px_is_8_4() {
    let mut r = with_mono();
    let st = style(MONO, 14.0, 400);
    let w10 = r.measure("0123456789", &st, None).w;
    assert!((w10 / 10.0 - 8.4).abs() < 0.3, "advance {}", w10 / 10.0);
    // every character the same width: a narrow letter and a wide one
    let (i, m) = (r.measure("iiiiiiiiii", &st, None).w, r.measure("MMMMMMMMMM", &st, None).w);
    assert!((i - m).abs() < 0.1, "i {i} vs M {m}");
}

#[test]
fn the_weight_axis_is_applied() {
    let mut r = with_mono();
    let inks: Vec<(u16, u64)> = [100u16, 400, 700, 800]
        .iter()
        .map(|&w| (w, ink(&draw_text(&mut r, "Hamburgefonts 0123", &style(MONO, 14.0, w)))))
        .collect();
    eprintln!("variable wght axis, ink by weight at 14 px: {inks:?}");
    let get = |w: u16| inks.iter().find(|(x, _)| *x == w).unwrap().1;
    assert!(get(700) as f64 > get(400) as f64 * 1.15, "700 vs 400: {} vs {}", get(700), get(400));
    assert!(get(400) as f64 > get(100) as f64 * 1.1, "400 vs 100: {} vs {}", get(400), get(100));
    assert!(get(800) > get(700), "800 vs 700: {} vs {}", get(800), get(700));
    // the advance does not change with the weight
    let w4 = r.measure("0123456789", &style(MONO, 14.0, 400), None).w;
    let w7 = r.measure("0123456789", &style(MONO, 14.0, 700), None).w;
    assert!((w4 - w7).abs() < 0.1);
    // 400 and 700 are different pictures, not the same one scaled
    let a = draw_text(&mut r, "Hamburgefonts", &style(MONO, 14.0, 400));
    let b = draw_text(&mut r, "Hamburgefonts", &style(MONO, 14.0, 700));
    let differing = a.data.iter().zip(&b.data).filter(|(x, y)| x != y).count();
    assert!(differing > 400, "only {differing} bytes differ");
}

#[test]
fn a_family_override_renames_the_faces() {
    let mut r = renderer();
    let names = r.register_font(font_bytes(), Some("Bundled Mono")).unwrap();
    assert_eq!(names, vec!["Bundled Mono".to_string()]);
    let c = draw_text(&mut r, "Hamburg", &style(&["Bundled Mono"], 14.0, 400));
    assert!(ink(&c) > 2_000);
    let w = r.measure("0123456789", &style(&["Bundled Mono"], 14.0, 400), None).w;
    assert!((w / 10.0 - 8.4).abs() < 0.3);
}

#[test]
fn registering_the_same_bytes_twice_adds_nothing() {
    let mut r = Renderer::new(rsb_render::FontConfig::default());
    let before = r.stats().fonts_registered;
    let a = r.register_font(font_bytes(), None).unwrap();
    let once = r.stats().fonts_registered;
    let b = r.register_font(font_bytes(), None).unwrap();
    let c = r.register_font(font_bytes(), None).unwrap();
    assert_eq!(once, before + 1);
    assert_eq!(r.stats().fonts_registered, once);
    assert_eq!((&a, &a), (&b, &c));
    // another family name for the same bytes is a different registration
    r.register_font(font_bytes(), Some("Second Name")).unwrap();
    assert_eq!(r.stats().fonts_registered, once + 1);
}

#[test]
fn oversized_and_non_font_data_is_refused() {
    let mut r = renderer();
    let big: Arc<[u8]> = vec![0u8; FONT_MAX_FILE_BYTES + (1 << 20)].into();
    assert!(matches!(r.register_font(big, None), Err(RenderError::Font(_))));
    let junk: Arc<[u8]> = b"this is not a font file at all, only text".to_vec().into();
    assert!(matches!(r.register_font(junk, None), Err(RenderError::Font(_))));
    let empty: Arc<[u8]> = Vec::new().into();
    assert!(matches!(r.register_font(empty, None), Err(RenderError::Font(_))));
    // a truncated real font is not usable either
    let bytes = font_bytes();
    let cut: Arc<[u8]> = bytes[..2000].to_vec().into();
    assert!(matches!(r.register_font(cut, None), Err(RenderError::Font(_))));
}

#[test]
fn a_font_registered_later_replaces_the_fallback_for_text_laid_out_before() {
    let mut r = Renderer::new(rsb_render::FontConfig::default());
    let st = style(&["Later Mono"], 14.0, 400);
    let before = r.measure("0123456789", &st, None).w;
    r.register_font(font_bytes(), Some("Later Mono")).unwrap();
    let after = r.measure("0123456789", &st, None).w;
    assert!((after / 10.0 - 8.4).abs() < 0.3, "advance {} (fallback was {})", after / 10.0, before / 10.0);
}

// ------------------------------------------------------------- caret stops

fn stops(r: &mut Renderer, s: &str, st: &TextStyle) -> Vec<(usize, f32)> {
    r.caret_stops(s, st)
}

#[test]
fn latin_stops_equal_the_measured_prefixes() {
    let mut r = with_mono();
    let s = "Hello, World 42%";
    for (st, tolerance) in [(style(MONO, 14.0, 400), 0.05), (style(SANS, 16.0, 400), 0.7)] {
        let got = stops(&mut r, s, &st);
        assert_eq!(got.len(), s.len() + 1);
        for (i, x) in &got {
            let want = r.measure(&s[..*i], &st, None).w;
            assert!((x - want).abs() <= tolerance, "stop {i}: {x} vs measured prefix {want}");
        }
        assert_eq!(got[0], (0, 0.0));
    }
}

#[test]
fn a_combining_sequence_is_one_stop() {
    let mut r = renderer();
    let st = style(SANS, 16.0, 400);
    // e + combining acute (3 bytes together), then x
    let s = "e\u{301}x";
    let got = stops(&mut r, s, &st);
    let at: Vec<usize> = got.iter().map(|(i, _)| *i).collect();
    assert_eq!(at, vec![0, 3, 4], "stops {got:?}");
    assert!(got[1].1 > 0.0 && got[2].1 > got[1].1);
    // an emoji family sequence is one cluster
    let family = "a\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}b";
    let at: Vec<usize> = stops(&mut r, family, &st).iter().map(|(i, _)| *i).collect();
    assert_eq!(at, vec![0, 1, family.len() - 1, family.len()]);
}

#[test]
fn a_ligature_still_has_a_stop_between_its_characters() {
    let mut r = renderer();
    let st = style(SANS, 24.0, 400);
    let got = stops(&mut r, "ffi", &st);
    let at: Vec<usize> = got.iter().map(|(i, _)| *i).collect();
    assert_eq!(at, vec![0, 1, 2, 3], "stops {got:?}");
    assert!(got.windows(2).all(|w| w[1].1 > w[0].1), "not increasing: {got:?}");
}

#[test]
fn right_to_left_stops_run_from_the_right_edge() {
    let mut r = renderer();
    let st = style(SANS, 16.0, 400);
    let s = "\u{645}\u{631}\u{62D}\u{628}\u{627}";
    let got = stops(&mut r, s, &st);
    let width = r.measure(s, &st, None).w;
    assert_eq!(got.first().map(|g| g.0), Some(0));
    assert_eq!(got.last().map(|g| g.0), Some(s.len()));
    assert!((got[0].1 - width).abs() < 1.0, "start at {} of {width}", got[0].1);
    assert!(got.last().unwrap().1.abs() < 1.0);
    assert!(got.windows(2).all(|w| w[1].1 <= w[0].1), "not decreasing: {got:?}");
}

#[test]
fn empty_and_huge_text_have_stops() {
    let mut r = renderer();
    let st = style(MONO, 14.0, 400);
    assert_eq!(stops(&mut r, "", &st), vec![(0, 0.0)]);
    let long = "x".repeat(50_000);
    let got = stops(&mut r, &long, &st);
    assert!(got.len() >= 2);
    assert!(got.windows(2).all(|w| w[1].0 > w[0].0));
}

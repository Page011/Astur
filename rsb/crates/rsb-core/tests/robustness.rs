//! Hostile and broken input must come back as diagnostics, never as a panic:
//! a panic on the UI or config thread would end the bar.

mod common;

use std::sync::Arc;
use std::time::Instant;

use common::*;
use rsb_core::config::load_config;
use rsb_core::css::{Stylesheet, parse_inline};
use rsb_core::element::{Element, ElementKind};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[(self.next() % items.len() as u64) as usize]
    }
}

const CSS_PIECES: &[&str] = &[
    ".a",
    ".b",
    "#x",
    "bar",
    "zone",
    "widget",
    "box",
    "text",
    ":root",
    ":hover",
    ":active",
    ":focus",
    ">",
    " ",
    "  ",
    "\n",
    "\r\n",
    "\t",
    "{",
    "}",
    "(",
    ")",
    ";",
    ":",
    ",",
    "\"",
    "'",
    "/*",
    "*/",
    "@media",
    "@import",
    "@keyframes",
    "(prefers-color-scheme: dark)",
    "color",
    "padding",
    "width",
    "background",
    "transition",
    "box-shadow",
    "font-family",
    "var(",
    "var(--a",
    "--a",
    "--b",
    "!important",
    "10px",
    "1em",
    "50%",
    "#fff",
    "#12",
    "red",
    "rgb(",
    "rgba(1,2,3,4)",
    "linear-gradient(",
    "to right",
    "cubic-bezier(",
    "\u{e9}",
    "\u{1F600}",
    "\u{0}",
    "calc(",
    "1.5",
    "-",
    "+",
    ".",
    "/",
    "\\",
    "url(",
    "none",
    "auto",
];

#[test]
fn random_style_sheets_never_panic() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for round in 0..4000 {
        let mut css = String::new();
        for _ in 0..(rng.next() % 60) {
            css.push_str(rng.pick(CSS_PIECES));
        }
        let (sheet, d) = Stylesheet::parse(&css, "style.css");
        // Whatever came out can be applied.
        if round % 8 == 0 {
            let t = classed(boxed(vec![classed(text("t"), &["a", "b"])]), &["a"]);
            let mut s = scene_with("", 200, 1.0, Some(t), None, None);
            s.set_stylesheet(Arc::new(sheet));
            frame(&mut s, Instant::now());
            let _ = s.take_diagnostics();
        }
        for x in &d {
            if let Some(p) = &x.pos {
                assert!(p.line >= 1 && p.col >= 1, "{x} for {css:?}");
            }
        }
    }
}

#[test]
fn random_inline_styles_never_panic() {
    let mut rng = Rng(12345);
    let names = [
        "color",
        "padding",
        "font_size",
        "--x",
        "width",
        "box_shadow",
        "transition",
        "opacity",
        "background",
        "bad name",
        "",
        "border-radius",
    ];
    for _ in 0..3000 {
        let mut value = String::new();
        for _ in 0..(rng.next() % 12) {
            value.push_str(rng.pick(CSS_PIECES));
        }
        let (_, d) = parse_inline(&[(rng.pick(&names).to_string(), value.clone())], None);
        let _ = d;
    }
}

const TOML_PIECES: &[&str] = &[
    "[[bar]]\n",
    "[bar]\n",
    "[widget.\"clock\"]\n",
    "[widget.clock#utc]\n",
    "left = [\"clock\"]\n",
    "left = [",
    "]",
    "height = 40\n",
    "height = \"x\"\n",
    "name = \"a\"\n",
    "monitors = [1, \"DISPLAY2\", \"*\"]\n",
    "preset = \"glass\"\n",
    "config_version = 1\n",
    "= 1\n",
    "\"",
    "'",
    "\n",
    "x = {a=1}\n",
    "bar = []\n",
    "\u{e9} = 1\n",
    "1 = 2\n",
    "[[widget]]\n",
    "radius = 1e400\n",
    "margin = 0x10\n",
    "fullscreen = \"stay\"\n",
    "# c\n",
];

#[test]
fn random_config_files_never_panic() {
    let mut rng = Rng(777);
    let presets = |n: &str| if n == "glass" { Some("[[bar]]\nfloating = true\n".to_string()) } else { None };
    for _ in 0..4000 {
        let mut text = String::new();
        for _ in 0..(rng.next() % 14) {
            text.push_str(rng.pick(TOML_PIECES));
        }
        let l = load_config(Some((&text, "bar.toml")), &presets);
        if let Some(c) = &l.config {
            for b in &c.bars {
                assert!((16..=128).contains(&b.height), "{text:?}");
                assert!(rsb_core::config::is_valid_bar_name(&b.name), "{text:?}");
            }
        } else {
            assert!(l.diagnostics.iter().any(|d| d.severity == rsb_core::diag::Severity::Error), "{text:?}");
        }
    }
}

#[test]
fn hostile_trees_are_survived() {
    // Very long text, many siblings, repeated keys, odd characters, zero-size surface.
    let long = "w".repeat(100_000);
    let kids: Vec<Element> = (0..1500)
        .map(|i| {
            let mut e = text(&format!("n{i}\u{1F600}\u{0301}"));
            e.key = Some(Arc::from(if i % 2 == 0 { "dup" } else { "x" }));
            e
        })
        .collect();
    let mut icons = Element::new(ElementKind::Icon);
    icons.icon = Some(rsb_core::element::IconRef::Glyph(Arc::from("\u{1F600}x")));
    let t = boxed(vec![
        text(&long),
        boxed(kids),
        icons,
        Element::new(ElementKind::Image),
        Element::new(ElementKind::Plate),
    ]);
    let mut s = scene_with("zone { gap: 3px }", 300, 1.5, Some(t), Some(text("")), None);
    let f = frame(&mut s, Instant::now());
    assert!(f.draw.items.len() > 1000);
    let _ = s.take_diagnostics();
    // A surface of no size.
    let mut s = scene_with("", 0, 1.0, Some(text("a")), None, None);
    s.set_geometry(rsb_core::scene::BarGeometry { width_px: 0, height_px: 0, scale: 1.0 });
    let f = frame(&mut s, Instant::now());
    assert_eq!(f.dirty, None);
    s.set_geometry(rsb_core::scene::BarGeometry { width_px: 1, height_px: 1, scale: 4.0 });
    frame(&mut s, Instant::now());
}

#[test]
fn a_big_flat_tree_lays_out_quickly() {
    // 2000 nodes is the cap per widget instance; the frame must not be slow even in a debug build.
    let kids: Vec<Element> = (0..1999).map(|i| text(&format!("{i}"))).collect();
    let mut s = scene_with("bar { font-size: 10px }", 4000, 1.0, Some(boxed(kids)), None, None);
    let start = Instant::now();
    frame(&mut s, start);
    let first = start.elapsed();
    // One text changes: only that node is restyled.
    let mut kids: Vec<Element> = (0..1999).map(|i| text(&format!("{i}"))).collect();
    kids[5] = text("changed");
    s.set_tree(tree(1, 2, Some(boxed(kids))));
    let start = Instant::now();
    let f = frame(&mut s, start);
    let second = start.elapsed();
    assert!(f.stats.restyled <= 2, "restyled {}", f.stats.restyled);
    eprintln!("2000 nodes: first frame {first:?}, one-text change {second:?}");
    assert!(first.as_millis() < 2000 && second.as_millis() < 2000);
}

#[test]
fn a_typical_bar_frame_cost() {
    // About 45 nodes in three zones; a clock text changes each round.
    let row = |n: usize, tag: &str| {
        boxed((0..n).map(|i| classed(boxed(vec![text(&format!("{tag}{i}")), text("%")]), &["pill"])).collect())
    };
    let css = ".pill { padding: 2px 8px; border-radius: 6px; background: #223344; gap: 4px } zone { gap: 6px }";
    let mut s = scene_with(css, 1920, 1.0, Some(row(6, "w")), Some(text("12:00")), Some(row(8, "r")));
    let t0 = Instant::now();
    let first = frame(&mut s, t0);
    let first_us = first.stats.style_us + first.stats.layout_us + first.stats.draw_us;
    let mut times: Vec<u128> = Vec::new();
    for i in 0..300u64 {
        s.set_tree(tree(2, 2 + i, Some(text(if i % 2 == 0 { "12:01" } else { "12:02" }))));
        let start = Instant::now();
        let f = frame(&mut s, start);
        times.push(start.elapsed().as_micros());
        assert!(f.dirty.is_some());
    }
    times.sort_unstable();
    eprintln!(
        "typical bar ({} nodes, {} items): first frame {first_us} us (style+layout+draw), clock tick median {} us, p95 {} us, worst {} us",
        first.stats.nodes,
        first.draw.items.len(),
        times[times.len() / 2],
        times[times.len() * 95 / 100],
        times[times.len() - 1]
    );
    assert!(times[times.len() / 2] < 20_000, "even a debug build stays far under a frame");
}

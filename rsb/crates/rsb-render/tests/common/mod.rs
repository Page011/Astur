//! Shared helpers for the renderer tests: draw list builders, the scenes the
//! golden and behaviour tests draw, and one renderer per test binary.
#![allow(dead_code)]

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use rsb_core::color::Color;
use rsb_core::draw::{DrawCmd, DrawItem, DrawList, Fill, TextStyle, shadow_bounds};
use rsb_core::element::NodeKey;
use rsb_core::geom::{Edges, PointF, Radii, RectF};
use rsb_core::style::TextAlign;
use rsb_render::{FontConfig, Image, Renderer};

mod outlines;
#[allow(unused_imports)]
pub use outlines::*;
mod pages;
#[allow(unused_imports)]
pub use pages::*;

/// "rdp" in a Remote Desktop session, else "console": the label every timing
/// printed by a test carries, since an rdp session paces frames differently.
pub fn session_name() -> &'static str {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetSystemMetrics(index: i32) -> i32;
    }
    const SM_REMOTESESSION: i32 = 0x1000;
    // SAFETY: a plain query with no pointers.
    if unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0 { "rdp" } else { "console" }
}

/// One renderer for the whole test binary: loading the system fonts is the
/// expensive part, and tests that share it also exercise cache reuse.
pub fn renderer() -> MutexGuard<'static, Renderer> {
    static R: OnceLock<Mutex<Renderer>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Renderer::new(FontConfig::default()))).lock().unwrap_or_else(|e| e.into_inner())
}

pub fn col(s: &str) -> Color {
    Color::parse(s).expect("test colour")
}

pub fn rect(x: f32, y: f32, w: f32, h: f32) -> RectF {
    RectF::new(x, y, w, h)
}

pub fn item(n: u64, bounds: RectF, cmd: DrawCmd) -> DrawItem {
    DrawItem { node: NodeKey(n), bounds, cmd }
}

pub fn fill(n: u64, r: RectF, radius: f32, color: Color) -> DrawItem {
    item(n, r.outset(1.0), DrawCmd::Fill { rect: r, radii: Radii::all(radius), fill: Fill::Solid(color) })
}

pub fn fill_radii(n: u64, r: RectF, radii: Radii, color: Color) -> DrawItem {
    item(n, r.outset(1.0), DrawCmd::Fill { rect: r, radii, fill: Fill::Solid(color) })
}

pub fn gradient(n: u64, r: RectF, radius: f32, start: (f32, f32), end: (f32, f32), stops: &[(f32, &str)]) -> DrawItem {
    let stops = stops.iter().map(|(p, c)| (*p, col(c))).collect();
    item(
        n,
        r.outset(1.0),
        DrawCmd::Fill {
            rect: r,
            radii: Radii::all(radius),
            fill: Fill::Linear { start: PointF { x: start.0, y: start.1 }, end: PointF { x: end.0, y: end.1 }, stops },
        },
    )
}

pub fn border(n: u64, r: RectF, radius: f32, widths: Edges<f32>, color: Color) -> DrawItem {
    item(n, r.outset(1.0), DrawCmd::Border { rect: r, radii: Radii::all(radius), widths, color })
}

pub fn shadow(n: u64, r: RectF, radius: f32, blur: f32, color: Color) -> DrawItem {
    item(n, shadow_bounds(r, blur), DrawCmd::Shadow { rect: r, radii: Radii::all(radius), blur, color })
}

pub fn style(families: &[&str], size: f32, weight: u16) -> TextStyle {
    TextStyle {
        families: families.iter().map(|f| Arc::<str>::from(*f)).collect::<Vec<_>>().into(),
        size_px: size,
        weight,
        italic: false,
        letter_spacing: 0.0,
        tabular_nums: false,
    }
}

pub fn text(n: u64, r: RectF, s: &str, st: &TextStyle, color: Color, align: TextAlign) -> DrawItem {
    item(n, r, DrawCmd::Text { rect: r, text: s.into(), style: st.clone(), color, align })
}

pub fn image(n: u64, r: RectF, key: &str, radius: f32) -> DrawItem {
    item(n, r.outset(1.0), DrawCmd::Image { rect: r, key: key.into(), radii: Radii::all(radius) })
}

pub fn push_clip(n: u64, r: RectF, radius: f32) -> DrawItem {
    item(n, r.outset(1.0), DrawCmd::PushClip { rect: r, radii: Radii::all(radius) })
}

pub fn pop_clip(n: u64, bounds: RectF) -> DrawItem {
    item(n, bounds, DrawCmd::PopClip)
}

pub fn push_opacity(n: u64, a: f32, bounds: RectF) -> DrawItem {
    item(n, bounds, DrawCmd::PushOpacity(a))
}

pub fn pop_opacity(n: u64, bounds: RectF) -> DrawItem {
    item(n, bounds, DrawCmd::PopOpacity)
}

pub fn list(w: u32, h: u32, items: Vec<DrawItem>) -> DrawList {
    DrawList { width: w, height: h, items }
}

/// A deterministic test image: a diagonal colour ramp with a transparent corner.
pub fn test_image(side: u32) -> Image {
    let mut data = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let r = (x * 255 / (side - 1)) as u8;
            let g = (y * 255 / (side - 1)) as u8;
            let b = 160u8;
            let a: u32 = if x + y < 3 { 0 } else { 255 };
            data.extend_from_slice(&[
                (r as u32 * a / 255) as u8,
                (g as u32 * a / 255) as u8,
                (b as u32 * a / 255) as u8,
                a as u8,
            ]);
        }
    }
    Image { width: side, height: side, data }
}

pub struct Scene {
    pub name: &'static str,
    pub list: DrawList,
    /// Needs the system fonts (skipped when their versions differ from the
    /// recorded ones).
    pub text: bool,
    pub images: Vec<(&'static str, Image)>,
    /// SVG sources installed under their key before drawing.
    pub svgs: Vec<(&'static str, &'static str)>,
}

const SANS: &[&str] = &["Segoe UI"];

pub fn background(w: u32, h: u32, c: &str) -> DrawItem {
    fill(0, rect(0.0, 0.0, w as f32, h as f32), 0.0, col(c))
}

pub fn scene_shapes() -> Scene {
    let items = vec![
        background(400, 170, "#f4f6f8"),
        fill(1, rect(10.0, 10.0, 60.0, 40.0), 0.0, col("#366382")),
        fill(2, rect(80.5, 10.5, 60.0, 40.0), 8.0, col("#E79B25")),
        fill(3, rect(150.0, 10.0, 80.0, 24.0), 12.0, col("#7D5168")),
        fill(4, rect(240.0, 10.0, 40.0, 40.0), 20.0, col("#394855")),
        fill_radii(5, rect(290.0, 10.0, 60.0, 40.0), Radii { tl: 20.0, tr: 0.0, br: 20.0, bl: 0.0 }, col("#366382")),
        fill(6, rect(10.0, 70.0, 60.0, 30.0), 50.0, col("#366382")),
        fill(7, rect(90.0, 70.0, 50.0, 50.0), 6.0, col("#E79B2580")),
        fill(8, rect(110.0, 85.0, 50.0, 50.0), 6.0, col("#36638280")),
        fill(9, rect(180.0, 70.3, 80.0, 1.0), 0.0, col("#394855")),
        fill(10, rect(180.0, 80.5, 80.0, 1.0), 0.0, col("#394855")),
        fill(11, rect(180.0, 90.0, 3.5, 3.5), 1.75, col("#7D5168")),
        fill_radii(12, rect(280.0, 70.0, 80.0, 40.0), Radii { tl: 4.0, tr: 16.0, br: 4.0, bl: 16.0 }, col("#E79B25")),
        fill(13, rect(10.0, 130.0, 380.0, 30.0), 15.0, col("#366382")),
    ];
    Scene { name: "shapes", list: list(400, 170, items), text: false, images: vec![], svgs: vec![] }
}

pub fn scene_borders() -> Scene {
    let items = vec![
        background(400, 170, "#ffffff"),
        border(1, rect(10.0, 10.0, 70.0, 40.0), 0.0, Edges::all(1.0), col("#366382")),
        border(2, rect(95.0, 10.0, 70.0, 40.0), 8.0, Edges::all(2.0), col("#366382")),
        border(
            3,
            rect(180.0, 10.0, 70.0, 40.0),
            10.0,
            Edges { top: 4.0, right: 1.0, bottom: 2.0, left: 0.0 },
            col("#7D5168"),
        ),
        border(4, rect(265.0, 10.0, 70.0, 40.0), 12.0, Edges::all(3.0), col("#E79B2599")),
        border(5, rect(10.0, 70.0, 40.0, 40.0), 20.0, Edges::all(30.0), col("#394855")),
        fill(6, rect(70.0, 70.0, 90.0, 30.0), 15.0, col("#E79B25")),
        border(7, rect(70.0, 70.0, 90.0, 30.0), 15.0, Edges::all(2.0), col("#394855")),
        border(
            8,
            rect(180.0, 70.0, 80.0, 40.0),
            6.0,
            Edges { top: 1.5, right: 1.5, bottom: 1.5, left: 1.5 },
            col("#366382"),
        ),
        border(9, rect(280.0, 70.5, 80.0, 40.0), 0.0, Edges::all(1.0), col("#366382")),
        border(
            10,
            rect(10.0, 130.0, 120.0, 30.0),
            15.0,
            Edges { top: 0.0, right: 0.0, bottom: 3.0, left: 0.0 },
            col("#7D5168"),
        ),
    ];
    Scene { name: "borders", list: list(400, 170, items), text: false, images: vec![], svgs: vec![] }
}

pub fn scene_gradients() -> Scene {
    let items = vec![
        background(400, 170, "#ffffff"),
        gradient(
            1,
            rect(10.0, 10.0, 100.0, 40.0),
            0.0,
            (10.0, 0.0),
            (110.0, 0.0),
            &[(0.0, "#366382"), (1.0, "#E79B25")],
        ),
        gradient(
            2,
            rect(120.0, 10.0, 60.0, 60.0),
            0.0,
            (0.0, 10.0),
            (0.0, 70.0),
            &[(0.0, "#ffffff"), (1.0, "#394855")],
        ),
        gradient(
            3,
            rect(190.0, 10.0, 80.0, 60.0),
            12.0,
            (190.0, 10.0),
            (270.0, 70.0),
            &[(0.0, "#7D5168"), (1.0, "#E79B25")],
        ),
        gradient(
            4,
            rect(280.0, 10.0, 110.0, 40.0),
            8.0,
            (280.0, 0.0),
            (390.0, 0.0),
            &[(0.0, "#366382"), (0.5, "#36638200"), (1.0, "#E79B25")],
        ),
        gradient(
            5,
            rect(10.0, 90.0, 100.0, 30.0),
            15.0,
            (60.0, 90.0),
            (60.0, 90.0),
            &[(0.0, "#366382"), (1.0, "#E79B25")],
        ),
        gradient(
            6,
            rect(120.0, 90.0, 140.0, 30.0),
            0.0,
            (120.0, 0.0),
            (260.0, 0.0),
            &[(0.3, "#394855"), (0.3, "#E79B25"), (0.8, "#7D5168")],
        ),
        gradient(
            7,
            rect(10.0, 130.0, 380.0, 30.0),
            6.0,
            (10.0, 0.0),
            (390.0, 0.0),
            &[(0.0, "#000000"), (1.0, "#ffffff")],
        ),
    ];
    Scene { name: "gradients", list: list(400, 170, items), text: false, images: vec![], svgs: vec![] }
}

pub fn scene_shadows() -> Scene {
    let box_a = rect(30.0, 30.0, 70.0, 40.0);
    let box_b = rect(150.0, 30.0, 70.0, 40.0);
    let box_c = rect(270.0, 30.0, 70.0, 40.0);
    let box_d = rect(30.0, 120.0, 90.0, 36.0);
    let box_e = rect(-12.0, 100.0, 40.0, 30.0);
    let box_f = rect(200.0, 125.0, 60.0, 30.0);
    let items = vec![
        background(400, 190, "#ffffff"),
        shadow(1, box_a, 8.0, 0.0, col("#00000066")),
        fill(2, box_a, 8.0, col("#ffffff")),
        shadow(3, rect(150.0, 34.0, 70.0, 40.0), 8.0, 4.0, col("#00000059")),
        fill(4, box_b, 8.0, col("#ffffff")),
        shadow(5, rect(266.0, 26.0, 78.0, 48.0), 10.0, 12.0, col("#00000059")),
        fill(6, box_c, 8.0, col("#ffffff")),
        shadow(7, box_d, 18.0, 10.0, col("#E79B25cc")),
        fill(8, box_d, 18.0, col("#E79B25")),
        shadow(9, box_e, 6.0, 6.0, col("#000000aa")),
        shadow(10, box_f, 4.0, 2.5, col("#366382")),
        fill(11, box_f, 4.0, col("#366382")),
    ];
    Scene { name: "shadows", list: list(400, 190, items), text: false, images: vec![], svgs: vec![] }
}

pub fn scene_clips() -> Scene {
    let c1 = rect(20.0, 20.0, 110.0, 60.0);
    let c2 = rect(160.0, 20.0, 110.0, 60.0);
    let inner = rect(175.0, 30.0, 50.0, 40.0);
    let c3 = rect(290.0, 20.0, 90.0, 60.0);
    let items = vec![
        background(400, 120, "#f4f6f8"),
        push_clip(1, c1, 16.0),
        gradient(
            2,
            rect(0.0, 0.0, 200.0, 120.0),
            0.0,
            (0.0, 0.0),
            (200.0, 120.0),
            &[(0.0, "#366382"), (1.0, "#E79B25")],
        ),
        fill(3, rect(100.0, 50.0, 80.0, 80.0), 40.0, col("#7D5168")),
        pop_clip(1, c1.outset(1.0)),
        push_clip(4, c2, 8.0),
        fill(5, rect(150.0, 10.0, 140.0, 40.0), 0.0, col("#394855")),
        push_clip(6, inner, 20.0),
        fill(7, rect(160.0, 20.0, 80.0, 60.0), 0.0, col("#E79B25")),
        pop_clip(6, inner.outset(1.0)),
        fill(8, rect(235.0, 40.0, 60.0, 50.0), 0.0, col("#366382")),
        pop_clip(4, c2.outset(1.0)),
        push_clip(9, c3, 0.0),
        shadow(10, rect(300.0, 30.0, 60.0, 30.0), 6.0, 8.0, col("#000000aa")),
        fill(11, rect(300.0, 30.0, 60.0, 30.0), 6.0, col("#E79B25")),
        pop_clip(9, c3.outset(1.0)),
        push_clip(12, rect(20.0, 95.0, 0.0, 10.0), 0.0),
        fill(13, rect(20.0, 95.0, 300.0, 10.0), 0.0, col("#ff0000")),
        pop_clip(12, rect(19.0, 94.0, 2.0, 12.0)),
    ];
    Scene { name: "clips", list: list(400, 120, items), text: false, images: vec![], svgs: vec![] }
}

pub fn scene_opacity() -> Scene {
    let g1 = rect(10.0, 10.0, 100.0, 70.0);
    let g2 = rect(130.0, 10.0, 130.0, 70.0);
    let g3 = rect(280.0, 10.0, 110.0, 70.0);
    let items = vec![
        background(400, 110, "#ffffff"),
        push_opacity(1, 0.5, g1.outset(1.0)),
        fill(2, rect(10.0, 10.0, 60.0, 50.0), 0.0, col("#366382")),
        fill(3, rect(40.0, 30.0, 70.0, 50.0), 0.0, col("#E79B25")),
        pop_opacity(1, g1.outset(1.0)),
        push_opacity(4, 0.5, g2.outset(1.0)),
        push_opacity(5, 0.5, g2.outset(1.0)),
        fill(6, rect(130.0, 10.0, 80.0, 50.0), 10.0, col("#7D5168")),
        pop_opacity(5, g2.outset(1.0)),
        fill(7, rect(170.0, 30.0, 90.0, 50.0), 10.0, col("#394855")),
        pop_opacity(4, g2.outset(1.0)),
        push_clip(8, g3, 20.0),
        push_opacity(9, 0.4, g3.outset(1.0)),
        fill(10, rect(270.0, 0.0, 130.0, 100.0), 0.0, col("#366382")),
        pop_opacity(9, g3.outset(1.0)),
        pop_clip(8, g3.outset(1.0)),
        push_opacity(11, 0.0, rect(10.0, 90.0, 100.0, 10.0)),
        fill(12, rect(10.0, 90.0, 100.0, 10.0), 0.0, col("#ff0000")),
        pop_opacity(11, rect(10.0, 90.0, 100.0, 10.0)),
        push_opacity(13, 1.0, rect(130.0, 90.0, 100.0, 10.0)),
        fill(14, rect(130.0, 90.0, 100.0, 10.0), 0.0, col("#366382")),
        pop_opacity(13, rect(130.0, 90.0, 100.0, 10.0)),
    ];
    Scene { name: "opacity", list: list(400, 110, items), text: false, images: vec![], svgs: vec![] }
}

pub fn scene_images() -> Scene {
    let items = vec![
        background(400, 120, "#e8ecef"),
        image(1, rect(10.0, 10.0, 16.0, 16.0), "ramp16", 0.0),
        image(2, rect(40.0, 10.0, 64.0, 64.0), "ramp16", 0.0),
        image(3, rect(120.0, 10.0, 64.0, 64.0), "ramp16", 32.0),
        image(4, rect(200.0, 10.5, 40.0, 40.0), "ramp16", 8.0),
        image(5, rect(260.0, 10.0, 80.0, 30.0), "ramp16", 4.0),
        image(6, rect(20.0, 90.0, 20.0, 20.0), "ramp64", 0.0),
        image(7, rect(60.0, 90.0, 20.0, 20.0), "missing", 0.0),
        fill(8, rect(100.0, 90.0, 20.0, 20.0), 0.0, col("#366382")),
    ];
    Scene {
        name: "images",
        list: list(400, 120, items),
        text: false,
        images: vec![("ramp16", test_image(16)), ("ramp64", test_image(64))],
        svgs: vec![],
    }
}

pub fn scene_text_latin() -> Scene {
    let ink = col("#1b1e24");
    let rt = |y: f32, h: f32| rect(10.0, y, 340.0, h);
    let items = vec![
        background(360, 190, "#ffffff"),
        text(
            1,
            rt(6.0, 20.0),
            "Delta Healthcare 12:34 - pressure care",
            &style(SANS, 13.0, 400),
            ink,
            TextAlign::Start,
        ),
        text(
            2,
            rt(30.0, 20.0),
            "Delta Healthcare 12:34 - pressure care",
            &style(SANS, 13.0, 600),
            ink,
            TextAlign::Start,
        ),
        text(
            3,
            rt(54.0, 28.0),
            "Quick brown fox, 0123456789",
            &style(SANS, 20.0, 400),
            col("#366382"),
            TextAlign::Start,
        ),
        text(4, rt(86.0, 20.0), "Centred text", &style(SANS, 13.0, 400), ink, TextAlign::Center),
        text(5, rt(108.0, 20.0), "Right aligned", &style(SANS, 13.0, 400), ink, TextAlign::End),
        text(6, rt(130.0, 20.0), "Half transparent text", &style(SANS, 15.0, 400), col("#1b1e2480"), TextAlign::Start),
        {
            let mut it = style(SANS, 15.0, 400);
            it.italic = true;
            text(7, rt(152.0, 20.0), "Italic and small 9 px 10 px", &it, ink, TextAlign::Start)
        },
        text(
            8,
            rect(10.0, 172.0, 90.0, 16.0),
            "Clipped to its box edge",
            &style(SANS, 13.0, 400),
            ink,
            TextAlign::Start,
        ),
    ];
    Scene { name: "text-latin", list: list(360, 190, items), text: true, images: vec![], svgs: vec![] }
}

pub fn scene_text_light() -> Scene {
    let items = vec![
        background(360, 90, "#1A1B26"),
        text(
            1,
            rect(10.0, 6.0, 340.0, 20.0),
            "Light text on a dark bar 12:34",
            &style(SANS, 13.0, 400),
            col("#C0CAF5"),
            TextAlign::Start,
        ),
        text(
            2,
            rect(10.0, 30.0, 340.0, 20.0),
            "Accent and dim 66AAFF 565F89",
            &style(SANS, 13.0, 400),
            col("#66AAFF"),
            TextAlign::Start,
        ),
        text(
            3,
            rect(10.0, 54.0, 340.0, 28.0),
            "Larger light text",
            &style(SANS, 22.0, 600),
            col("#ffffff"),
            TextAlign::Start,
        ),
    ];
    Scene { name: "text-light", list: list(360, 90, items), text: true, images: vec![], svgs: vec![] }
}

pub fn scene_text_arabic() -> Scene {
    let ink = col("#1b1e24");
    let rt = |y: f32| rect(10.0, y, 340.0, 28.0);
    let items = vec![
        background(360, 130, "#ffffff"),
        text(
            1,
            rt(6.0),
            "\u{645}\u{631}\u{62D}\u{628}\u{627} \u{628}\u{627}\u{644}\u{639}\u{627}\u{644}\u{645}",
            &style(SANS, 18.0, 400),
            ink,
            TextAlign::Start,
        ),
        text(
            2,
            rt(36.0),
            "Delta \u{645}\u{631}\u{62D}\u{628}\u{627} 123",
            &style(SANS, 18.0, 400),
            ink,
            TextAlign::Start,
        ),
        text(
            3,
            rt(66.0),
            "\u{644}\u{627} \u{644}\u{644}\u{647} \u{644}\u{627}\u{200C}\u{644}\u{627}",
            &style(SANS, 18.0, 400),
            ink,
            TextAlign::Start,
        ),
        text(
            4,
            rt(96.0),
            "\u{5E9}\u{5DC}\u{5D5}\u{5DD} Hebrew then Arabic \u{645}\u{631}\u{62D}\u{628}\u{627}",
            &style(SANS, 16.0, 400),
            ink,
            TextAlign::Start,
        ),
    ];
    Scene { name: "text-arabic", list: list(360, 130, items), text: true, images: vec![], svgs: vec![] }
}

pub fn scene_text_ligature() -> Scene {
    let ink = col("#1b1e24");
    let calibri = style(&["Calibri"], 22.0, 400);
    let items = vec![
        background(360, 100, "#ffffff"),
        text(1, rect(10.0, 6.0, 340.0, 30.0), "office fi fl ffi ffl", &calibri, ink, TextAlign::Start),
        text(
            2,
            rect(10.0, 40.0, 340.0, 30.0),
            "Candara: ffi ffl ft tt",
            &style(&["Candara"], 22.0, 400),
            ink,
            TextAlign::Start,
        ),
        text(3, rect(10.0, 70.0, 340.0, 26.0), "Segoe: office fi fl", &style(SANS, 20.0, 400), ink, TextAlign::Start),
    ];
    Scene { name: "text-ligature", list: list(360, 100, items), text: true, images: vec![], svgs: vec![] }
}

pub fn scene_text_spacing() -> Scene {
    let ink = col("#1b1e24");
    let mut s0 = style(SANS, 14.0, 400);
    let mut s2 = s0.clone();
    let mut s6 = s0.clone();
    let mut sneg = s0.clone();
    s2.letter_spacing = 2.0;
    s6.letter_spacing = 6.0;
    sneg.letter_spacing = -1.0;
    s0.letter_spacing = 0.0;
    let r = |y: f32| rect(10.0, y, 340.0, 22.0);
    let items = vec![
        background(360, 120, "#ffffff"),
        text(1, r(6.0), "SPACING 0 Delta", &s0, ink, TextAlign::Start),
        text(2, r(32.0), "SPACING 2 Delta", &s2, ink, TextAlign::Start),
        text(3, r(58.0), "SPACING 6 Delta", &s6, ink, TextAlign::Start),
        text(4, r(84.0), "SPACING -1 Delta", &sneg, ink, TextAlign::Start),
    ];
    Scene { name: "text-spacing", list: list(360, 120, items), text: true, images: vec![], svgs: vec![] }
}

pub fn scene_text_tabular() -> Scene {
    let ink = col("#1b1e24");
    let plain = style(&["Candara"], 18.0, 400);
    let mut tab = plain.clone();
    tab.tabular_nums = true;
    let seg_tab = {
        let mut s = style(SANS, 18.0, 400);
        s.tabular_nums = true;
        s
    };
    let r = |y: f32| rect(10.0, y, 340.0, 24.0);
    let items = vec![
        background(360, 130, "#ffffff"),
        text(1, r(6.0), "1111111111 0000000000", &plain, ink, TextAlign::Start),
        text(2, r(34.0), "1111111111 0000000000", &tab, ink, TextAlign::Start),
        text(3, r(62.0), "1111111111 0000000000", &seg_tab, ink, TextAlign::Start),
        text(4, r(90.0), "12:08:15 11:11:11", &seg_tab, ink, TextAlign::Start),
    ];
    Scene { name: "text-tabular", list: list(360, 130, items), text: true, images: vec![], svgs: vec![] }
}

pub fn scene_emoji() -> Scene {
    let ink = col("#1b1e24");
    let big = style(SANS, 28.0, 400);
    let items = vec![
        background(360, 120, "#ffffff"),
        text(
            1,
            rect(10.0, 6.0, 340.0, 40.0),
            "\u{1F525} \u{1F600} \u{2764}\u{FE0F} \u{1F44D} Delta",
            &big,
            ink,
            TextAlign::Start,
        ),
        text(
            2,
            rect(10.0, 50.0, 340.0, 24.0),
            "\u{1F525} half transparent \u{1F600}",
            &style(SANS, 18.0, 400),
            col("#1b1e2480"),
            TextAlign::Start,
        ),
        text(
            3,
            rect(10.0, 78.0, 340.0, 22.0),
            "Check \u{2713} arrows \u{2192} sun \u{2600}",
            &style(SANS, 16.0, 400),
            ink,
            TextAlign::Start,
        ),
    ];
    Scene { name: "emoji", list: list(360, 120, items), text: true, images: vec![], svgs: vec![] }
}

/// Code points the icon golden draws: all present in Segoe MDL2 Assets and
/// Segoe Fluent Icons.
pub const ICON_POINTS: &[char] =
    &['\u{E700}', '\u{E713}', '\u{E767}', '\u{E74F}', '\u{E774}', '\u{E7E8}', '\u{E83F}', '\u{E8D6}'];

pub fn scene_icons() -> Scene {
    let ink = col("#1b1e24");
    let icons = style(&["icons"], 20.0, 400);
    let mut items = vec![background(360, 100, "#ffffff")];
    for (i, c) in ICON_POINTS.iter().enumerate() {
        items.push(text(
            10 + i as u64,
            rect(10.0 + i as f32 * 40.0, 8.0, 32.0, 28.0),
            &c.to_string(),
            &icons,
            ink,
            TextAlign::Center,
        ));
    }
    items.push(text(
        30,
        rect(10.0, 50.0, 340.0, 24.0),
        "\u{E767} 87%  \u{E83F} 64%",
        &style(&["icons", "Segoe UI"], 16.0, 400),
        ink,
        TextAlign::Start,
    ));
    Scene { name: "icons", list: list(360, 100, items), text: true, images: vec![], svgs: vec![] }
}

pub fn all_scenes() -> Vec<Scene> {
    vec![
        scene_shapes(),
        scene_borders(),
        scene_gradients(),
        scene_shadows(),
        scene_clips(),
        scene_opacity(),
        scene_images(),
        scene_text_latin(),
        scene_text_light(),
        scene_text_arabic(),
        scene_text_ligature(),
        scene_text_spacing(),
        scene_text_tabular(),
        scene_emoji(),
        scene_icons(),
        scene_paths(),
        scene_chevrons(),
        scene_svg(),
        scene_mono(),
    ]
}

pub fn install_images(r: &mut Renderer, scene: &Scene) {
    for (k, img) in &scene.images {
        r.set_image(Arc::<str>::from(*k), img.clone());
    }
    if scene.name == "mono" {
        r.register_font(bundled_mono(), None).expect("bundled font");
    }
    for (k, src) in &scene.svgs {
        r.set_svg(Arc::<str>::from(*k), src.as_bytes()).expect("test svg");
    }
}

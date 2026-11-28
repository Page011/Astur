//! The render-failure plate: when drawing the bar fails, the last good
//! frame stays on screen and this fixed plate (plain white text on dark red,
//! Segoe UI 12 px, no CSS, no scene) is drawn over its left corner, so the
//! failure is visible even when the style sheet or the scene is what broke.

use std::sync::Arc;

use rsb_core::color::Color;
use rsb_core::draw::{DrawCmd, DrawItem, DrawList, TextStyle};
use rsb_core::element::NodeKey;
use rsb_core::geom::{RectF, RectPx};
use rsb_core::pixels::PixelsMut;
use rsb_core::style::TextAlign;
use rsb_render::Renderer;

pub const TEXT: &str = "RENDER ERROR - could not draw the bar; see log";
const BACKGROUND: [u8; 4] = [0, 0, 139, 255];

/// Paints the plate into `px` and returns the rectangle to present. The
/// background is written straight into the pixels, so the plate still shows
/// when the renderer is what failed; the text is added when it can be.
pub fn draw(renderer: &mut Renderer, px: &mut PixelsMut<'_>, scale: f32) -> Option<RectPx> {
    let (w, h) = (px.width as i32, px.height as i32);
    if w == 0 || h == 0 {
        return None;
    }
    let size = 12.0 * scale;
    let text_w = (TEXT.chars().count() as f32 * size * 0.55).ceil() as i32 + (16.0 * scale) as i32;
    let rect = RectPx::new(0, 0, text_w.min(w), h);
    let stride = px.width as usize * 4;
    for y in rect.y0..rect.y1 {
        let row = &mut px.data[y as usize * stride..(y as usize + 1) * stride];
        for x in rect.x0..rect.x1 {
            row[x as usize * 4..x as usize * 4 + 4].copy_from_slice(&BACKGROUND);
        }
    }
    let text_area = RectF::new(8.0 * scale, 0.0, (rect.width() as f32 - 8.0 * scale).max(0.0), rect.height() as f32);
    let family: Arc<[Arc<str>]> = Arc::from(vec![Arc::<str>::from("Segoe UI")]);
    let list = DrawList {
        width: px.width,
        height: px.height,
        items: vec![DrawItem {
            node: NodeKey(0),
            bounds: text_area,
            cmd: DrawCmd::Text {
                rect: text_area,
                text: Arc::from(TEXT),
                style: TextStyle {
                    families: family,
                    size_px: size,
                    weight: 400,
                    italic: false,
                    letter_spacing: 0.0,
                    tabular_nums: false,
                },
                color: Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 },
                align: TextAlign::Start,
            },
        }],
    };
    // Drawing text clears the region first, so paint it over a copy of the
    // background: render into a scratch buffer and copy the text pixels over.
    let mut scratch = vec![0u8; px.width as usize * px.height as usize * 4];
    {
        let mut target = PixelsMut { data: &mut scratch, width: px.width, height: px.height };
        if renderer.render(&list, &mut target, Some(rect)).is_ok() {
            for y in rect.y0..rect.y1 {
                for x in rect.x0..rect.x1 {
                    let i = y as usize * stride + x as usize * 4;
                    let a = u32::from(scratch[i + 3]);
                    if a > 0 {
                        for c in 0..3 {
                            let over = u32::from(scratch[i + c]) + u32::from(px.data[i + c]) * (255 - a) / 255;
                            px.data[i + c] = over.min(255) as u8;
                        }
                    }
                }
            }
        }
    }
    Some(rect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsb_render::{Canvas, FontConfig};

    #[test]
    fn the_plate_is_dark_red_with_white_text_in_the_left_corner() {
        let mut renderer = Renderer::new(FontConfig::default());
        let mut canvas = Canvas::new(800, 32);
        let rect = draw(&mut renderer, &mut canvas.pixels(), 1.0).expect("a rect to present");
        assert_eq!((rect.x0, rect.y0, rect.y1), (0, 0, 32));
        assert!(rect.x1 > 200 && rect.x1 <= 800, "{rect:?}");
        assert_eq!(canvas.rgba_at(2, 2), [139, 0, 0, 255], "dark red background");
        assert_eq!(canvas.rgba_at(790, 16), [0, 0, 0, 0], "nothing beyond the plate");
        let white = (0..rect.x1 as u32).flat_map(|x| (0..32).map(move |y| (x, y))).filter(|&(x, y)| {
            let p = canvas.rgba_at(x, y);
            p[0] > 200 && p[1] > 200 && p[2] > 200
        });
        assert!(white.count() > 100, "the text is drawn in white");
    }

    #[test]
    fn a_zero_sized_surface_draws_nothing() {
        let mut renderer = Renderer::new(FontConfig::default());
        let mut canvas = Canvas::new(0, 0);
        assert_eq!(draw(&mut renderer, &mut canvas.pixels(), 1.0), None);
    }
}

//! Gaussian box shadows.
//!
//! The shadow shape is rasterised once at 1:1 (anti-aliased coverage), then
//! blurred with a separable kernel of standard deviation `blur / 2` cut at 3
//! deviations, which is what `rsb_core::draw::shadow_bounds` allows for.
//! Only the pixels asked for (`vis`) are computed, but each of them reads the
//! same inputs in the same order whatever `vis` is, so a partial redraw is
//! identical to a full one.

use rsb_core::color::Color;
use rsb_core::geom::{Radii, RectF, RectPx};
use tiny_skia::{FillRule, Mask, Paint, Pixmap, Transform};

use crate::shape;

/// Reusable buffers, kept between frames.
#[derive(Default)]
pub(crate) struct ShadowScratch {
    source: Vec<u8>,
    rows: Vec<f32>,
}

/// Kernel radius for a blur: 3 standard deviations, rounded up.
pub(crate) fn kernel_radius(blur: f32) -> i32 {
    if blur.is_finite() && blur > 0.0 { (1.5 * blur).ceil() as i32 } else { 0 }
}

fn kernel(blur: f32, radius: i32) -> Vec<f32> {
    if radius == 0 {
        return vec![1.0];
    }
    let sigma = blur / 2.0;
    let mut k: Vec<f32> = (-radius..=radius).map(|i| (-((i * i) as f32) / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= sum);
    k
}

#[inline]
fn div255(v: u32) -> u32 {
    (v + 127) / 255
}

/// Draws the shadow of `rect` into `dst`, only inside `vis`, scaled by `mask`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw(
    scratch: &mut ShadowScratch,
    dst: &mut Pixmap,
    mask: Option<&Mask>,
    vis: RectPx,
    rect: RectF,
    radii: Radii,
    blur: f32,
    color: Color,
) {
    let Some(path) = shape::rrect_path(rect, radii) else { return };
    let alpha = if color.a.is_finite() { color.a.clamp(0.0, 1.0) } else { 0.0 };
    if alpha <= 0.0 || vis.is_empty() {
        return;
    }
    let radius = kernel_radius(blur);
    let shape_px = RectPx::outward(&rect);
    let reach = RectPx::new(vis.x0 - radius, vis.y0 - radius, vis.x1 + radius, vis.y1 + radius);
    if shape_px.intersect(&reach).is_none() {
        return;
    }
    // The raster window must not depend on `vis`: tiny-skia chops curves
    // differently when the window cuts through them, which would make a
    // partial redraw differ from a full one. The surface (plus the kernel
    // margin) is the only cut.
    let surface = RectPx::new(-radius, -radius, dst.width() as i32 + radius, dst.height() as i32 + radius);
    let Some(need) = shape_px.intersect(&surface) else { return };
    let (nw, nh) = (need.width() as usize, need.height() as usize);

    // Coverage of the shape over `need`, anchored at integer offsets so the
    // result does not depend on which window was asked for.
    let mut buf = std::mem::take(&mut scratch.source);
    buf.clear();
    buf.resize(nw * nh * 4, 0);
    let Some(size) = tiny_skia::IntSize::from_wh(nw as u32, nh as u32) else { return };
    let Some(mut pm) = Pixmap::from_vec(buf, size) else { return };
    let moved = path.transform(Transform::from_translate(-(need.x0 as f32), -(need.y0 as f32)));
    if let Some(moved) = moved {
        let mut paint = Paint { anti_alias: true, ..Paint::default() };
        paint.set_color_rgba8(0, 0, 0, 255);
        pm.fill_path(&moved, &paint, FillRule::Winding, Transform::identity(), None);
    }
    let source = pm.take();

    let k = kernel(blur, radius);
    let out_w = vis.width() as usize;
    // vertical extent of the rows the horizontal pass has to produce
    let row0 = need.y0.max(vis.y0 - radius);
    let row1 = need.y1.min(vis.y1 + radius);
    let row_count = (row1 - row0).max(0) as usize;
    let mut rows = std::mem::take(&mut scratch.rows);
    rows.clear();
    rows.resize(row_count * out_w, 0.0);

    // horizontal pass: rows[ty][x] for x in vis columns
    for ty in 0..row_count {
        let sy = (row0 - need.y0) as usize + ty;
        let src_row = &source[sy * nw * 4..(sy + 1) * nw * 4];
        let out_row = &mut rows[ty * out_w..(ty + 1) * out_w];
        for (ox, o) in out_row.iter_mut().enumerate() {
            let x = vis.x0 + ox as i32;
            let lo = (need.x0).max(x - radius);
            let hi = (need.x1 - 1).min(x + radius);
            if lo > hi {
                continue;
            }
            let mut acc = 0.0f32;
            for sx in lo..=hi {
                let w = k[(sx - x + radius) as usize];
                acc += w * src_row[(sx - need.x0) as usize * 4 + 3] as f32;
            }
            *o = acc;
        }
    }

    // vertical pass and blend
    let color_premul = [color.r, color.g, color.b].map(|c| if c.is_finite() { c.clamp(0.0, 1.0) } else { 0.0 });
    let dst_w = dst.width() as usize;
    let mask_data = mask.map(|m| m.data());
    let data = dst.data_mut();
    for y in vis.y0..vis.y1 {
        let lo = row0.max(y - radius);
        let hi = (row1 - 1).min(y + radius);
        if lo > hi {
            continue;
        }
        for ox in 0..out_w {
            let mut acc = 0.0f32;
            for sy in lo..=hi {
                let w = k[(sy - y + radius) as usize];
                acc += w * rows[(sy - row0) as usize * out_w + ox];
            }
            let cov = (acc / 255.0 * alpha).clamp(0.0, 1.0);
            let x = vis.x0 + ox as i32;
            if x < 0 || y < 0 || x as usize >= dst_w {
                continue;
            }
            let pos = y as usize * dst_w + x as usize;
            let mut sa = (cov * 255.0).round() as u32;
            if let Some(m) = mask_data {
                sa = div255(sa * m[pos] as u32);
            }
            if sa == 0 {
                continue;
            }
            let src = |c: f32| div255((c * 255.0).round() as u32 * sa);
            let d = &mut data[pos * 4..pos * 4 + 4];
            let inv = 255 - sa;
            d[0] = (src(color_premul[0]) + div255(d[0] as u32 * inv)).min(255) as u8;
            d[1] = (src(color_premul[1]) + div255(d[1] as u32 * inv)).min(255) as u8;
            d[2] = (src(color_premul[2]) + div255(d[2] as u32 * inv)).min(255) as u8;
            d[3] = (sa + div255(d[3] as u32 * inv)).min(255) as u8;
        }
    }
    scratch.source = source;
    scratch.rows = rows;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_is_normalised_and_symmetric() {
        let k = kernel(8.0, kernel_radius(8.0));
        assert_eq!(k.len(), 2 * 12 + 1);
        assert!((k.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!((k[0] - k[k.len() - 1]).abs() < 1e-9);
        assert!(k[12] > k[11]);
    }

    #[test]
    fn radius_matches_shadow_bounds() {
        for blur in [0.0f32, 1.0, 4.0, 7.5, 12.0, 31.0] {
            let rect = RectF::new(10.0, 10.0, 20.0, 20.0);
            let b = rsb_core::draw::shadow_bounds(rect, blur);
            assert_eq!(b.x, rect.x - (kernel_radius(blur) as f32 + 1.0), "blur {blur}");
        }
    }

    #[test]
    fn hard_shadow_is_a_filled_shape() {
        let mut s = ShadowScratch::default();
        let mut pm = Pixmap::new(20, 20).unwrap();
        draw(
            &mut s,
            &mut pm,
            None,
            RectPx::new(0, 0, 20, 20),
            RectF::new(5.0, 5.0, 10.0, 10.0),
            Radii::ZERO,
            0.0,
            Color::BLACK,
        );
        assert_eq!(pm.pixel(10, 10).unwrap().alpha(), 255);
        assert_eq!(pm.pixel(2, 2).unwrap().alpha(), 0);
    }
}

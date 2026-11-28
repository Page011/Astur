//! PNG in and out. Decoding is bounded before anything large is allocated.

use std::io::Cursor;

use crate::{IMAGE_MAX_FILE_BYTES, IMAGE_MAX_SIDE, Image, RenderError};

/// Decoder allocation ceiling: a 1024 x 1024 RGBA frame is 4 MiB, the
/// decoder's own row buffers come on top.
const DECODER_LIMIT_BYTES: usize = 32 << 20;

fn png_err(e: impl std::fmt::Display) -> RenderError {
    RenderError::Png(e.to_string())
}

#[inline]
fn div255(v: u32) -> u8 {
    ((v + 127) / 255) as u8
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Image, RenderError> {
    if bytes.len() > IMAGE_MAX_FILE_BYTES {
        return Err(RenderError::Png(format!("file is {} bytes, limit {IMAGE_MAX_FILE_BYTES}", bytes.len())));
    }
    let mut decoder = png::Decoder::new_with_limits(Cursor::new(bytes), png::Limits { bytes: DECODER_LIMIT_BYTES });
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(png_err)?;
    let (width, height) = {
        let info = reader.info();
        (info.width, info.height)
    };
    if width == 0 || height == 0 {
        return Err(RenderError::Png("image has no pixels".into()));
    }
    if width > IMAGE_MAX_SIDE || height > IMAGE_MAX_SIDE {
        return Err(RenderError::ImageTooLarge { width, height });
    }
    let size = reader.output_buffer_size().ok_or_else(|| RenderError::Png("image is too large".into()))?;
    let mut raw = vec![0u8; size];
    let frame = reader.next_frame(&mut raw).map_err(png_err)?;
    let (color, depth) = reader.output_color_type();
    if depth != png::BitDepth::Eight {
        return Err(RenderError::Png(format!("unsupported bit depth {depth:?}")));
    }
    let raw = &raw[..frame.buffer_size()];
    let pixels = width as usize * height as usize;
    let mut data = Vec::with_capacity(pixels * 4);
    match color {
        png::ColorType::Rgba => {
            for p in raw.as_chunks::<4>().0 {
                let a = p[3] as u32;
                data.extend_from_slice(&[
                    div255(p[0] as u32 * a),
                    div255(p[1] as u32 * a),
                    div255(p[2] as u32 * a),
                    p[3],
                ]);
            }
        }
        png::ColorType::Rgb => {
            for p in raw.as_chunks::<3>().0 {
                data.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for p in raw.as_chunks::<2>().0 {
                let g = div255(p[0] as u32 * p[1] as u32);
                data.extend_from_slice(&[g, g, g, p[1]]);
            }
        }
        png::ColorType::Grayscale => {
            for &g in raw {
                data.extend_from_slice(&[g, g, g, 255]);
            }
        }
        png::ColorType::Indexed => return Err(RenderError::Png("palette was not expanded".into())),
    }
    if data.len() != pixels * 4 {
        return Err(RenderError::Png("decoded size does not match the header".into()));
    }
    Ok(Image { width, height, data })
}

/// Premultiplied BGRA8 to a straight-alpha RGBA8 PNG.
pub(crate) fn encode_bgra_premul(width: u32, height: u32, bgra: &[u8]) -> Result<Vec<u8>, RenderError> {
    if width == 0 || height == 0 || bgra.len() < width as usize * height as usize * 4 {
        return Err(RenderError::BadSize);
    }
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for p in bgra[..width as usize * height as usize * 4].as_chunks::<4>().0 {
        let px = unpremultiply([p[2], p[1], p[0], p[3]]);
        rgba.extend_from_slice(&px);
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(png_err)?;
        writer.write_image_data(&rgba).map_err(png_err)?;
    }
    Ok(out)
}

/// Straight-alpha RGBA of one premultiplied RGBA pixel.
pub(crate) fn unpremultiply(p: [u8; 4]) -> [u8; 4] {
    let a = p[3] as u32;
    if a == 0 {
        return [0, 0, 0, 0];
    }
    if a == 255 {
        return p;
    }
    let un = |c: u8| ((c as u32 * 255 + a / 2) / a).min(255) as u8;
    [un(p[0]), un(p[1]), un(p[2]), p[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_rgba(w: u32, h: u32, color: png::ColorType, depth: png::BitDepth, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(color);
        enc.set_depth(depth);
        enc.write_header().unwrap().write_image_data(data).unwrap();
        out
    }

    #[test]
    fn rgba_is_premultiplied() {
        let png = encode_rgba(1, 1, png::ColorType::Rgba, png::BitDepth::Eight, &[200, 100, 50, 128]);
        let img = decode(&png).unwrap();
        assert_eq!((img.width, img.height), (1, 1));
        assert_eq!(img.data, vec![100, 50, 25, 128]);
    }

    #[test]
    fn sixteen_bit_and_gray_expand_to_rgba() {
        let png16 = encode_rgba(1, 1, png::ColorType::Rgb, png::BitDepth::Sixteen, &[0x80, 0, 0x40, 0, 0xff, 0]);
        assert_eq!(decode(&png16).unwrap().data, vec![0x80, 0x40, 0xff, 255]);
        let gray = encode_rgba(2, 1, png::ColorType::Grayscale, png::BitDepth::Eight, &[10, 250]);
        assert_eq!(decode(&gray).unwrap().data, vec![10, 10, 10, 255, 250, 250, 250, 255]);
        let ga = encode_rgba(1, 1, png::ColorType::GrayscaleAlpha, png::BitDepth::Eight, &[200, 51]);
        assert_eq!(decode(&ga).unwrap().data, vec![40, 40, 40, 51]);
    }

    #[test]
    fn palette_is_expanded() {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, 2, 1);
        enc.set_color(png::ColorType::Indexed);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_palette(vec![255, 0, 0, 0, 0, 255]);
        enc.write_header().unwrap().write_image_data(&[0, 1]).unwrap();
        assert_eq!(decode(&out).unwrap().data, vec![255, 0, 0, 255, 0, 0, 255, 255]);
    }

    #[test]
    fn big_sides_are_refused_before_decoding() {
        let wide = encode_rgba(
            IMAGE_MAX_SIDE + 1,
            1,
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            &vec![0; IMAGE_MAX_SIDE as usize + 1],
        );
        assert_eq!(decode(&wide), Err(RenderError::ImageTooLarge { width: IMAGE_MAX_SIDE + 1, height: 1 }));
        let tall = encode_rgba(
            1,
            IMAGE_MAX_SIDE + 1,
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            &vec![0; IMAGE_MAX_SIDE as usize + 1],
        );
        assert!(matches!(decode(&tall), Err(RenderError::ImageTooLarge { .. })));
        let ok = encode_rgba(
            IMAGE_MAX_SIDE,
            1,
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            &vec![7; IMAGE_MAX_SIDE as usize],
        );
        assert_eq!(decode(&ok).unwrap().width, IMAGE_MAX_SIDE);
    }

    #[test]
    fn oversized_file_and_garbage_are_errors() {
        let big = vec![0u8; IMAGE_MAX_FILE_BYTES + 1];
        assert!(matches!(decode(&big), Err(RenderError::Png(_))));
        assert!(matches!(decode(b"not a png at all"), Err(RenderError::Png(_))));
        assert!(matches!(decode(&[]), Err(RenderError::Png(_))));
        let mut truncated = encode_rgba(4, 4, png::ColorType::Rgb, png::BitDepth::Eight, &[9; 48]);
        truncated.truncate(truncated.len() - 12);
        assert!(matches!(decode(&truncated), Err(RenderError::Png(_))));
    }

    #[test]
    fn encode_roundtrip_keeps_straight_colour() {
        // premultiplied BGRA of straight (200, 100, 50) at alpha 128
        let bgra = [25u8, 50, 100, 128, 0, 0, 0, 0];
        let png = encode_bgra_premul(2, 1, &bgra).unwrap();
        let mut dec = png::Decoder::new(Cursor::new(&png[..])).read_info().unwrap();
        let mut buf = vec![0; dec.output_buffer_size().unwrap()];
        dec.next_frame(&mut buf).unwrap();
        assert!(
            (buf[0] as i32 - 200).abs() <= 2 && (buf[1] as i32 - 100).abs() <= 2 && (buf[2] as i32 - 50).abs() <= 2
        );
        assert_eq!(&buf[3..8], &[128, 0, 0, 0, 0]);
    }
}

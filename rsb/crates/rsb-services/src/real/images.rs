//! Pictures for services: encoded bytes (album art) or shell icons to
//! premultiplied RGBA with the Windows Imaging Component, scaled down to fit
//! a square, on the calling service thread (COM already initialised there).

use std::time::{Duration, Instant};

use rsb_core::pixels::RgbaImage;
use windows::Win32::Graphics::Gdi::{BITMAP, DIBSECTION, GetObjectW, HGDIOBJ};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatBmp, GUID_ContainerFormatGif, GUID_ContainerFormatJpeg,
    GUID_ContainerFormatPng, GUID_WICPixelFormat32bppPBGRA, IWICBitmapSource, IWICImagingFactory, IWICPalette,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom,
    WICDecodeMetadataCacheOnDemand, WICRect,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IStream};
use windows::core::{GUID, Interface};

/// Album art and app icons are kept at most this many px per side.
pub(crate) const MAX_SIDE: u32 = 256;
/// A source picture whose frame declares a longer side is refused before
/// any pixel is decoded: the bytes come from whatever app owns the media
/// session, and a small file can claim an enormous size.
pub(crate) const MAX_SOURCE_SIDE: u32 = 8192;
/// Same, for the frame's area (width x height).
pub(crate) const MAX_SOURCE_PIXELS: u64 = 32_000_000;
/// The whole decode and scale stops after this long.
pub(crate) const DECODE_DEADLINE: Duration = Duration::from_secs(2);
/// Encoded bytes accepted by `decode`.
pub(crate) const MAX_SOURCE_BYTES: usize = 4 << 20;

/// Rows copied out of the scaler between two looks at the clock.
const BAND_ROWS: u32 = 32;

/// The four containers whose built-in decoders are used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Container {
    Png,
    Jpeg,
    Bmp,
    Gif,
}

impl Container {
    fn guid(self) -> GUID {
        match self {
            Container::Png => GUID_ContainerFormatPng,
            Container::Jpeg => GUID_ContainerFormatJpeg,
            Container::Bmp => GUID_ContainerFormatBmp,
            Container::Gif => GUID_ContainerFormatGif,
        }
    }
}

/// Which container the first bytes announce; anything else (WebP, TIFF,
/// HEIF, an icon file) is not decoded at all.
fn sniff(bytes: &[u8]) -> Option<Container> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some(Container::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Container::Jpeg)
    } else if bytes.starts_with(b"BM") {
        Some(Container::Bmp)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(Container::Gif)
    } else {
        None
    }
}

/// The declared frame size against the limits, before any pixel is read.
fn check_frame(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("the picture is empty".into());
    }
    if width > MAX_SOURCE_SIDE || height > MAX_SOURCE_SIDE || u64::from(width) * u64::from(height) > MAX_SOURCE_PIXELS {
        return Err(format!("the picture is too large ({width} x {height})"));
    }
    Ok(())
}

/// The size that fits `max_side` x `max_side`, keeping the aspect ratio and
/// never enlarging.
fn fit(width: u32, height: u32, max_side: u32) -> (u32, u32) {
    let long = width.max(height);
    if long <= max_side {
        return (width, height);
    }
    let scale = f64::from(max_side) / f64::from(long);
    let side = |v: u32| ((f64::from(v) * scale).round() as u32).clamp(1, max_side);
    (side(width), side(height))
}

fn within(deadline: Instant) -> Result<(), String> {
    if Instant::now() >= deadline { Err("the picture took too long to decode".into()) } else { Ok(()) }
}

/// Decodes PNG, JPEG, BMP or GIF bytes and scales them to fit
/// `max_side` x `max_side`, keeping the aspect ratio. The container is
/// told from the first bytes and only that built-in WIC decoder is created
/// (`CreateDecoder` with its container GUID, metadata on demand), never one
/// picked from every codec installed on the PC; anything else, a frame over
/// `MAX_SOURCE_SIDE` or `MAX_SOURCE_PIXELS`, or a decode past
/// `DECODE_DEADLINE` is an error. Scaling uses `IWICBitmapScaler` (or the
/// decoder's own `IWICBitmapSourceTransform`), so the full-size frame is
/// never held.
pub(crate) fn decode(bytes: &[u8], max_side: u32) -> Result<RgbaImage, String> {
    decode_within(bytes, max_side, Instant::now() + DECODE_DEADLINE)
}

/// `decode` with the moment it must be finished by.
pub(crate) fn decode_within(bytes: &[u8], max_side: u32, deadline: Instant) -> Result<RgbaImage, String> {
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err("the picture file is too large".into());
    }
    let container = sniff(bytes).ok_or("the picture format is not supported")?;
    let max_side = max_side.clamp(1, MAX_SOURCE_SIDE);
    within(deadline)?;

    let fail = |what: &str| {
        let what = what.to_string();
        move |e: windows::core::Error| format!("{what} failed (0x{:08X})", e.code().0 as u32)
    };

    // SAFETY: plain COM calls on interfaces created here; `bytes` outlives the stream that reads it.
    unsafe {
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
            .map_err(fail("the imaging component"))?;
        let guid = container.guid();
        let decoder = factory.CreateDecoder(&guid, std::ptr::null()).map_err(fail("creating the decoder"))?;
        let stream = factory.CreateStream().map_err(fail("opening the picture"))?;
        stream.InitializeFromMemory(bytes).map_err(fail("opening the picture"))?;
        let istream: IStream = stream.cast().map_err(fail("opening the picture"))?;
        decoder.Initialize(&istream, WICDecodeMetadataCacheOnDemand).map_err(fail("reading the picture"))?;
        within(deadline)?;
        let frame = decoder.GetFrame(0).map_err(fail("reading the picture"))?;
        let (mut width, mut height) = (0u32, 0u32);
        frame.GetSize(&mut width, &mut height).map_err(fail("reading the picture size"))?;
        check_frame(width, height)?;

        // Premultiplied BGRA first (streaming), then the scaler on that.
        let converter = factory.CreateFormatConverter().map_err(fail("converting the picture"))?;
        converter
            .Initialize(
                &frame,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None::<&IWICPalette>,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(fail("converting the picture"))?;
        let (tw, th) = fit(width, height, max_side);
        let source: IWICBitmapSource = if (tw, th) == (width, height) {
            converter.cast().map_err(fail("converting the picture"))?
        } else {
            let scaler = factory.CreateBitmapScaler().map_err(fail("scaling the picture"))?;
            scaler
                .Initialize(&converter, tw, th, WICBitmapInterpolationModeFant)
                .map_err(fail("scaling the picture"))?;
            scaler.cast().map_err(fail("scaling the picture"))?
        };
        within(deadline)?;

        let stride = tw as usize * 4;
        let mut data = vec![0u8; stride * th as usize];
        let mut y = 0u32;
        while y < th {
            within(deadline)?;
            let rows = BAND_ROWS.min(th - y);
            let rect = WICRect { X: 0, Y: y as i32, Width: tw as i32, Height: rows as i32 };
            let band = &mut data[y as usize * stride..(y + rows) as usize * stride];
            source.CopyPixels(&rect, stride as u32, band).map_err(fail("reading the picture"))?;
            y += rows;
        }
        for px in data.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
        RgbaImage::new(tw, th, data).ok_or_else(|| "the picture size is inconsistent".to_string())
    }
}

/// Copies a 32-bit top-down or bottom-up DIB section (from
/// `IShellItemImageFactory::GetImage`) into premultiplied RGBA.
pub(crate) fn from_hbitmap(hbitmap: isize) -> Result<RgbaImage, String> {
    if hbitmap == 0 {
        return Err("no bitmap".into());
    }
    let mut ds = DIBSECTION::default();
    // SAFETY: `ds` is a writable DIBSECTION of the size passed; a handle that is not a bitmap gives 0.
    let got = unsafe {
        GetObjectW(
            HGDIOBJ(hbitmap as *mut core::ffi::c_void),
            size_of::<DIBSECTION>() as i32,
            Some((&raw mut ds).cast()),
        )
    };
    if (got as usize) < size_of::<DIBSECTION>() {
        return Err("the bitmap is not a DIB section".into());
    }
    let bm: BITMAP = ds.dsBm;
    if bm.bmBitsPixel != 32 || bm.bmBits.is_null() || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
        return Err("the bitmap is not 32-bit".into());
    }
    let (width, height) = (bm.bmWidth as u32, bm.bmHeight as u32);
    check_frame(width, height)?;
    let stride = bm.bmWidthBytes as usize;
    let row_bytes = width as usize * 4;
    if stride < row_bytes {
        return Err("the bitmap rows are shorter than its width".into());
    }
    // A negative header height marks a top-down section; the usual one stores the bottom row first.
    let top_down = ds.dsBmih.biHeight < 0;
    // SAFETY: a DIB section's bits cover `bmWidthBytes * bmHeight` bytes and stay valid while the handle lives.
    let bits = unsafe { std::slice::from_raw_parts(bm.bmBits.cast::<u8>(), stride * height as usize) };
    let mut data = Vec::with_capacity(row_bytes * height as usize);
    for out_row in 0..height as usize {
        let src_row = if top_down { out_row } else { height as usize - 1 - out_row };
        data.extend_from_slice(&bits[src_row * stride..src_row * stride + row_bytes]);
    }
    // Some icons carry colour with an all-zero alpha channel: they are opaque.
    let pixels = data.as_chunks::<4>().0;
    if pixels.iter().all(|p| p[3] == 0) && pixels.iter().any(|p| p[..3].iter().any(|c| *c != 0)) {
        for px in data.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
    }
    for px in data.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
        // Premultiplied colour never exceeds its alpha.
        let a = px[3];
        px[0] = px[0].min(a);
        px[1] = px[1].min(a);
        px[2] = px[2].min(a);
    }
    RgbaImage::new(width, height, data).ok_or_else(|| "the bitmap size is inconsistent".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containers_are_told_from_the_first_bytes() {
        assert_eq!(sniff(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0]), Some(Container::Png));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(Container::Jpeg));
        assert_eq!(sniff(b"BM\0\0"), Some(Container::Bmp));
        assert_eq!(sniff(b"GIF89a.."), Some(Container::Gif));
        assert_eq!(sniff(b"GIF87a.."), Some(Container::Gif));
        assert_eq!(sniff(b"RIFF\x10\0\0\0WEBPVP8 "), None, "WebP is not one of the four");
        assert_eq!(sniff(b"II*\0"), None, "TIFF is not one of the four");
        assert_eq!(sniff(&[]), None);
    }

    #[test]
    fn frame_limits_are_checked_on_both_sides_and_the_area() {
        assert!(check_frame(256, 256).is_ok());
        assert!(check_frame(MAX_SOURCE_SIDE, 1).is_ok());
        assert!(check_frame(MAX_SOURCE_SIDE + 1, 1).is_err());
        assert!(check_frame(1, MAX_SOURCE_SIDE + 1).is_err());
        assert!(check_frame(60_000, 60_000).is_err());
        // Both sides in range, the area over 32 Mpx.
        assert!(check_frame(8000, 4100).is_err());
        assert!(check_frame(8000, 4000).is_ok());
        assert!(check_frame(0, 5).is_err());
    }

    #[test]
    fn fitting_keeps_the_aspect_ratio_and_never_enlarges() {
        assert_eq!(fit(100, 50, 256), (100, 50));
        assert_eq!(fit(1000, 500, 256), (256, 128));
        assert_eq!(fit(500, 1000, 256), (128, 256));
        assert_eq!(fit(4000, 1, 256), (256, 1), "a thin picture keeps one row");
        assert_eq!(fit(256, 256, 256), (256, 256));
    }

    #[test]
    fn a_decode_that_is_already_late_stops_before_any_com_call() {
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let late = Instant::now() - Duration::from_millis(1);
        let e = decode_within(&png, 64, late).unwrap_err();
        assert!(e.contains("too long"), "{e}");
    }

    #[test]
    fn bytes_over_the_size_cap_are_refused_without_a_look() {
        let big = vec![0u8; MAX_SOURCE_BYTES + 1];
        assert!(decode(&big, 64).unwrap_err().contains("too large"));
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for b in data {
            crc ^= u32::from(*b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    fn adler32(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for d in data {
            a = (a + u32::from(*d)) % 65_521;
            b = (b + a) % 65_521;
        }
        (b << 16) | a
    }

    fn chunk(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = (body.len() as u32).to_be_bytes().to_vec();
        let mut crc_input = kind.to_vec();
        crc_input.extend_from_slice(body);
        out.extend_from_slice(&crc_input);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
        out
    }

    /// An RGBA PNG of one flat colour, with the image data in stored (uncompressed) blocks.
    fn png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        // Each row starts with filter type 0 (none).
        let mut row = vec![0u8];
        for _ in 0..width {
            row.extend_from_slice(&rgba);
        }
        let raw = row.repeat(height as usize);
        let mut z = vec![0x78, 0x01];
        let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
        for (i, block) in blocks.iter().enumerate() {
            z.push(u8::from(i + 1 == blocks.len()));
            z.extend_from_slice(&(block.len() as u16).to_le_bytes());
            z.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
            z.extend_from_slice(block);
        }
        z.extend_from_slice(&adler32(&raw).to_be_bytes());
        let mut header = Vec::new();
        header.extend_from_slice(&width.to_be_bytes());
        header.extend_from_slice(&height.to_be_bytes());
        header.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        out.extend(chunk(b"IHDR", &header));
        out.extend(chunk(b"IDAT", &z));
        out.extend(chunk(b"IEND", &[]));
        out
    }

    /// Initialises COM for the test thread (the decoder needs it, as on a service thread).
    fn com() {
        // SAFETY: plain initialisation; a repeat call on the same thread is harmless.
        let _ = unsafe {
            windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED)
        };
    }

    #[test]
    fn a_png_made_in_the_test_decodes_and_a_large_one_is_scaled_down() {
        com();
        let small = decode(&png(40, 30, [200, 100, 50, 255]), MAX_SIDE).expect("a small PNG decodes");
        assert_eq!((small.width, small.height), (40, 30), "never enlarged");
        assert_eq!(&small.data[..4], &[200, 100, 50, 255], "RGBA order, opaque");
        assert!(small.data.as_chunks::<4>().0.iter().all(|p| *p == [200, 100, 50, 255]));

        let big = decode(&png(1000, 500, [10, 20, 30, 255]), 64).expect("a larger PNG decodes");
        assert_eq!((big.width, big.height), (64, 32), "scaled to fit, ratio kept");
        assert_eq!(big.data.len(), 64 * 32 * 4);
        assert_eq!(&big.data[..4], &[10, 20, 30, 255]);

        // Colour is premultiplied: a half-transparent pixel holds half the colour.
        let half = decode(&png(4, 4, [200, 100, 50, 128]), 64).unwrap();
        let p = &half.data[..4];
        assert!(p[3] == 128 && (i32::from(p[0]) - 100).abs() <= 1, "premultiplied: {p:?}");
    }

    #[test]
    fn a_header_claiming_sixty_thousand_pixels_a_side_is_refused_before_decoding() {
        com();
        // A real PNG whose header is rewritten to a huge size: the data after it is far too short, so a decoder
        // that tried to read pixels would fail differently (or allocate).
        let mut bytes = png(2, 2, [1, 2, 3, 255]);
        bytes[16..20].copy_from_slice(&60_000u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&60_000u32.to_be_bytes());
        // The header checksum no longer matches; WIC may refuse that first, which is also a refusal. Fix it up so
        // the size check is what is tested.
        let crc = crc32(&bytes[12..29]);
        bytes[29..33].copy_from_slice(&crc.to_be_bytes());
        let t = Instant::now();
        let e = decode(&bytes, MAX_SIDE).unwrap_err();
        assert!(e.contains("too large"), "{e}");
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
    }

    #[test]
    fn a_webp_and_other_containers_are_refused_without_a_decoder_being_created() {
        com();
        let webp = b"RIFF\x1a\0\0\0WEBPVP8L\x0d\0\0\0\x2f\0\0\0\x10\x07\x10\x11\x11\x88\x88\xfe\x07\0";
        let e = decode(webp, MAX_SIDE).unwrap_err();
        assert!(e.contains("not supported"), "{e}");
        assert!(decode(b"II*\0\x08\0\0\0", MAX_SIDE).is_err(), "TIFF");
        assert!(decode(&[], MAX_SIDE).is_err(), "empty");
    }

    #[test]
    fn a_truncated_png_gives_an_error_or_a_picture_of_the_declared_size_never_a_crash() {
        com();
        let mut bytes = png(8, 8, [9, 9, 9, 255]);
        let n = bytes.len();
        bytes.truncate(n / 2);
        // The decoder may fill what is missing or refuse; either way the result is consistent.
        if let Ok(image) = decode(&bytes, 64) {
            assert_eq!(image.data.len(), image.width as usize * image.height as usize * 4);
            assert_eq!((image.width, image.height), (8, 8));
        }
        bytes.truncate(20);
        assert!(decode(&bytes, 64).is_err(), "a header cut in the middle is refused");
    }

    #[test]
    fn a_dib_section_copies_into_premultiplied_rgba_with_rows_the_right_way_up() {
        use windows::Win32::Graphics::Gdi::{
            BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, DeleteObject,
        };
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: 2,
                biHeight: 2, // bottom-up
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: a 2 x 2 x 32-bit section with no file mapping; the pixels are written inside its 16 bytes.
        let bitmap =
            unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }.expect("a DIB section");
        // SAFETY: `bits` points at 16 writable bytes, BGRA, bottom row first.
        unsafe {
            let px = std::slice::from_raw_parts_mut(bits.cast::<u8>(), 16);
            px[..8].copy_from_slice(&[0, 0, 255, 255, 0, 255, 0, 255]); // bottom row: red, green
            px[8..].copy_from_slice(&[255, 0, 0, 255, 255, 255, 255, 255]); // top row: blue, white
        }
        let image = from_hbitmap(bitmap.0 as isize).expect("copied");
        // SAFETY: the bitmap is deleted once.
        let _ = unsafe { DeleteObject(bitmap.into()) };
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(&image.data[..], &[0, 0, 255, 255, 255, 255, 255, 255, 255, 0, 0, 255, 0, 255, 0, 255]);
        assert!(from_hbitmap(0).is_err());
    }
}

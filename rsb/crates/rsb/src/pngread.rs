//! Reads a PNG of any size the screen has (the renderer's own decoder stops at
//! 1024 px a side, which suits icons and not a 1920 x 1200 screenshot) with
//! the Windows Imaging Component. Used for the `--background` picture of a
//! timeline run and by the tests that look at the pictures it wrote.
//!
//! The decode runs on a thread of its own that joins COM for its short life,
//! so no other thread's apartment is touched.

use std::path::Path;

use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatPng, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, IWICPalette,
    WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, IStream,
};
use windows::core::Interface;

/// Longest side and largest area accepted.
pub const MAX_SIDE: u32 = 8192;
pub const MAX_PIXELS: u64 = 32_000_000;
/// Largest file read.
pub const MAX_BYTES: u64 = 64 << 20;

/// A decoded picture: premultiplied RGBA8, top-down.
#[derive(Debug)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Reads and decodes a PNG file.
pub fn read(path: &Path) -> Result<Picture, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("cannot read the picture: {e}"))?;
    if meta.len() > MAX_BYTES {
        return Err("the picture file is too large".into());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read the picture: {e}"))?;
    decode(bytes)
}

/// Decodes PNG bytes on a short-lived thread.
pub fn decode(bytes: Vec<u8>) -> Result<Picture, String> {
    std::thread::Builder::new()
        .name("rsb-png".into())
        .spawn(move || {
            // SAFETY: balanced below; the thread does nothing else with COM.
            let joined = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
            let result = decode_here(&bytes);
            if joined {
                // SAFETY: pairs the successful CoInitializeEx above.
                unsafe { CoUninitialize() };
            }
            result
        })
        .map_err(|e| e.to_string())?
        .join()
        .map_err(|_| "the picture decoder stopped".to_string())?
}

fn decode_here(bytes: &[u8]) -> Result<Picture, String> {
    if bytes.len() < 8 || bytes[..8] != *b"\x89PNG\r\n\x1a\n" {
        return Err("the picture is not a PNG".into());
    }
    let fail = |what: &str| {
        let what = what.to_string();
        move |e: windows::core::Error| format!("{what} failed (0x{:08X})", e.code().0 as u32)
    };
    // SAFETY: plain COM calls on interfaces created here; `bytes` outlives the stream that reads it.
    unsafe {
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
            .map_err(fail("the imaging component"))?;
        let decoder =
            factory.CreateDecoder(&GUID_ContainerFormatPng, std::ptr::null()).map_err(fail("creating the decoder"))?;
        let stream = factory.CreateStream().map_err(fail("opening the picture"))?;
        stream.InitializeFromMemory(bytes).map_err(fail("opening the picture"))?;
        let istream: IStream = stream.cast().map_err(fail("opening the picture"))?;
        decoder.Initialize(&istream, WICDecodeMetadataCacheOnDemand).map_err(fail("reading the picture"))?;
        let frame = decoder.GetFrame(0).map_err(fail("reading the picture"))?;
        let (mut width, mut height) = (0u32, 0u32);
        frame.GetSize(&mut width, &mut height).map_err(fail("reading the picture size"))?;
        if width == 0
            || height == 0
            || width > MAX_SIDE
            || height > MAX_SIDE
            || u64::from(width) * u64::from(height) > MAX_PIXELS
        {
            return Err(format!("the picture is {width}x{height}: more than {MAX_SIDE} px a side or {MAX_PIXELS} px"));
        }
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
        let stride = width as usize * 4;
        let mut data = vec![0u8; stride * height as usize];
        converter.CopyPixels(std::ptr::null(), stride as u32, &mut data).map_err(fail("reading the picture"))?;
        for px in data.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
        Ok(Picture { width, height, rgba: data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        // Straight-alpha input through the renderer's own encoder (premultiplied BGRA in, straight RGBA out).
        let mut canvas = rsb_render::Canvas::new(width, height);
        canvas.data.copy_from_slice(rgba);
        canvas.to_png().unwrap()
    }

    #[test]
    fn a_png_wider_than_the_renderers_limit_is_read_with_its_pixels() {
        // 1500 x 2, opaque: the left half blue, the right half green (BGRA).
        let mut bgra = Vec::new();
        for _y in 0..2 {
            for x in 0..1500 {
                bgra.extend_from_slice(if x < 750 { &[255, 0, 0, 255] } else { &[0, 255, 0, 255] });
            }
        }
        let pic = decode(png(1500, 2, &bgra)).unwrap();
        assert_eq!((pic.width, pic.height), (1500, 2));
        assert_eq!(&pic.rgba[0..4], &[0, 0, 255, 255], "RGBA: blue");
        assert_eq!(&pic.rgba[(1499 * 4)..(1499 * 4 + 4)], &[0, 255, 0, 255], "RGBA: green");
    }

    #[test]
    fn something_that_is_not_a_png_is_refused() {
        assert!(decode(b"GIF89a....".to_vec()).unwrap_err().contains("not a PNG"));
        assert!(decode(Vec::new()).is_err());
        let mut cut = png(4, 4, &[255u8; 64]);
        cut.truncate(40);
        assert!(decode(cut).is_err());
        assert!(read(Path::new("no-such-picture.png")).is_err());
    }
}

//! Draw list to pixels, on the CPU.
//!
//! - Shapes, gradients, shadows, clips and group opacity: tiny-skia, into an
//!   RGBA8 premultiplied canvas.
//! - Text: Parley lays out (shaping, bidi, system font fallback incl. colour
//!   emoji), swash renders glyph images (COLR layers, bitmaps, outlines; no
//!   hinting). Glyphs are cached by font, glyph, size and quarter-pixel
//!   position in a cache bounded by bytes.
//! - Output: premultiplied BGRA8, top-down, written only inside the region
//!   asked for. Channels are swapped exactly once, while copying the redrawn
//!   rows out.
//!
//! Text code stays in the `text` module so a DirectWrite path could replace
//! it without touching the rest.

mod images;
mod paint;
mod png_io;
mod shadow;
mod shape;
mod text;

use std::sync::Arc;

use rsb_core::draw::{DrawList, TextStyle};
use rsb_core::geom::{RectPx, SizeF};
use rsb_core::icons::IconFont;
use rsb_core::layout::TextMeasure;
use rsb_core::pixels::PixelsMut;

/// Glyph image cache limit; least recently used glyphs go first.
pub const GLYPH_CACHE_MAX_BYTES: usize = 4 << 20;
/// Text layout cache limit (entries); the clock changes its text every tick.
pub const LAYOUT_CACHE_MAX: usize = 256;
/// Largest image side accepted, px; bigger images are refused.
pub const IMAGE_MAX_SIDE: u32 = 1024;
/// Largest PNG file accepted, bytes.
pub const IMAGE_MAX_FILE_BYTES: usize = 4 << 20;
/// Largest surface side accepted, px.
pub const MAX_SURFACE_SIDE: u32 = 16384;
/// Decoded image cache limit; least recently drawn go first.
pub const IMAGE_CACHE_MAX_BYTES: usize = 16 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderError {
    /// The target is smaller than the draw list's size.
    TargetSize {
        need: (u32, u32),
        got: (u32, u32),
    },
    /// Width or height is zero or above 16384.
    BadSize,
    Png(String),
    ImageTooLarge {
        width: u32,
        height: u32,
    },
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::TargetSize { need, got } => {
                write!(f, "target is {}x{}, draw list needs {}x{}", got.0, got.1, need.0, need.1)
            }
            RenderError::BadSize => f.write_str("surface size out of range"),
            RenderError::Png(m) => write!(f, "png: {m}"),
            RenderError::ImageTooLarge { width, height } => {
                write!(f, "image is {width}x{height}, limit {IMAGE_MAX_SIDE} per side")
            }
        }
    }
}

impl std::error::Error for RenderError {}

/// Font choices that are not CSS: what generic names and icons map to.
#[derive(Clone, Debug, PartialEq)]
pub struct FontConfig {
    pub sans_serif: String,
    pub monospace: Vec<String>,
    pub emoji: String,
}

impl Default for FontConfig {
    fn default() -> Self {
        FontConfig {
            sans_serif: "Segoe UI".into(),
            monospace: vec!["Cascadia Mono".into(), "Consolas".into()],
            emoji: "Segoe UI Emoji".into(),
        }
    }
}

/// A decoded image: premultiplied RGBA8, top-down.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Decodes a PNG (any colour type, 8 or 16 bit) to premultiplied RGBA8.
/// Refuses files over `IMAGE_MAX_FILE_BYTES` and sides over
/// `IMAGE_MAX_SIDE` before allocating.
pub fn decode_png(bytes: &[u8]) -> Result<Image, RenderError> {
    png_io::decode(bytes)
}

/// An owned pixel buffer for offscreen rendering (tests, `rsb render`).
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    /// Premultiplied BGRA8, top-down.
    pub data: Vec<u8>,
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Canvas {
        Canvas { width, height, data: vec![0; width as usize * height as usize * 4] }
    }

    pub fn pixels(&mut self) -> PixelsMut<'_> {
        PixelsMut { data: &mut self.data, width: self.width, height: self.height }
    }

    /// 8-bit RGBA PNG with straight alpha.
    pub fn to_png(&self) -> Result<Vec<u8>, RenderError> {
        png_io::encode_bgra_premul(self.width, self.height, &self.data)
    }

    /// Straight-alpha RGBA8 of one pixel, for tests. Outside the canvas:
    /// transparent.
    pub fn rgba_at(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0; 4];
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        png_io::unpremultiply([self.data[i + 2], self.data[i + 1], self.data[i], self.data[i + 3]])
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub glyphs_cached: usize,
    pub glyph_bytes: usize,
    pub layouts_cached: usize,
    pub images: usize,
    pub image_bytes: usize,
    pub colour_glyphs_drawn: u64,
}

pub struct Renderer {
    text: text::TextEngine,
    images: images::ImageCache,
    scratch: paint::Scratch,
}

impl Renderer {
    /// Loads the system font collection (once per process; measured in the
    /// first-frame budget).
    pub fn new(fonts: FontConfig) -> Renderer {
        Renderer {
            text: text::TextEngine::new(fonts),
            images: images::ImageCache::default(),
            scratch: paint::Scratch::default(),
        }
    }

    /// Segoe Fluent Icons when installed, else Segoe MDL2 Assets.
    pub fn icon_font(&self) -> IconFont {
        self.text.icon_font()
    }

    /// True when the system has a family with this name (case-insensitive).
    pub fn has_family(&mut self, name: &str) -> bool {
        self.text.has_family(name)
    }

    /// Draws `list` into `target`. With `region`, only that rectangle (cut
    /// to the surface) is cleared and redrawn and only items whose bounds
    /// meet it are painted. Returns the rectangle actually rewritten (None
    /// when the region was empty): that is the rectangle to present.
    pub fn render(
        &mut self,
        list: &DrawList,
        target: &mut PixelsMut<'_>,
        region: Option<RectPx>,
    ) -> Result<Option<RectPx>, RenderError> {
        check_size(list.width, list.height)?;
        let stride = target.width as usize * 4;
        if target.width < list.width || target.height < list.height || target.data.len() < stride * list.height as usize
        {
            return Err(RenderError::TargetSize {
                need: (list.width, list.height),
                got: (target.width, target.height),
            });
        }
        let surface = RectPx::new(0, 0, list.width as i32, list.height as i32);
        let region = match region {
            Some(r) => match r.intersect(&surface) {
                Some(r) => r,
                None => return Ok(None),
            },
            None => surface,
        };
        self.text.begin_frame();
        let painted = paint::paint(&mut self.scratch, &mut self.text, &mut self.images, list, region);
        self.text.end_frame();
        painted?;
        paint::copy_out(&self.scratch, target, region);
        Ok(Some(region))
    }

    /// Images are decoded off the UI thread and handed in here; an image
    /// item whose key is missing draws nothing.
    pub fn set_image(&mut self, key: Arc<str>, image: Image) {
        self.images.set(key, image);
    }

    pub fn has_image(&self, key: &str) -> bool {
        self.images.has(key)
    }

    pub fn stats(&self) -> RenderStats {
        let t = self.text.stats();
        RenderStats {
            glyphs_cached: t.glyphs,
            glyph_bytes: t.glyph_bytes,
            layouts_cached: t.layouts,
            images: self.images.len(),
            image_bytes: self.images.bytes(),
            colour_glyphs_drawn: t.colour_drawn,
        }
    }
}

impl TextMeasure for Renderer {
    fn measure(&mut self, text: &str, style: &TextStyle, max_width: Option<f32>) -> SizeF {
        // single line only: wrapping is reserved for later
        let _ = max_width;
        self.text.measure(text, style)
    }
}

fn check_size(width: u32, height: u32) -> Result<(), RenderError> {
    if width == 0 || height == 0 || width > MAX_SURFACE_SIDE || height > MAX_SURFACE_SIDE {
        return Err(RenderError::BadSize);
    }
    Ok(())
}

/// Renders a whole draw list into a new canvas.
pub fn render_offscreen(renderer: &mut Renderer, list: &DrawList) -> Result<Canvas, RenderError> {
    check_size(list.width, list.height)?;
    let mut canvas = Canvas::new(list.width, list.height);
    renderer.render(list, &mut canvas.pixels(), None)?;
    Ok(canvas)
}

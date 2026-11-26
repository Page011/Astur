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
//! - Outlines (bevelled boxes, `clip-path`): path fills, borders, shadows and
//!   clips, anti-aliased like the box commands.
//! - SVG: parsed with `usvg` under limits (shapes only, no files), rasterised
//!   by `resvg` into a cache and blitted tinted or in its own colours.
//!
//! Text code stays in the `text` module so a DirectWrite path could replace
//! it without touching the rest.

mod images;
mod paint;
mod png_io;
mod shadow;
mod shape;
mod svg;
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
/// Largest font file accepted by `Renderer::register_font`, bytes.
pub const FONT_MAX_FILE_BYTES: usize = 16 << 20;
/// Largest SVG file accepted, bytes.
pub const SVG_MAX_FILE_BYTES: usize = 1 << 20;
/// Most elements a user's SVG may hold; more is refused (bundled icons
/// hold a few dozen).
pub const SVG_MAX_NODES: usize = 10_000;
/// Rasterised SVG cache limit (one entry per key, size and tint); least
/// recently drawn go first.
pub const SVG_CACHE_MAX_BYTES: usize = 8 << 20;

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
    /// A font file that could not be used: too large, or no face in it.
    Font(String),
    /// An SVG that could not be parsed or is too large.
    Svg(String),
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
            RenderError::Font(m) => write!(f, "font: {m}"),
            RenderError::Svg(m) => write!(f, "svg: {m}"),
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
    /// SVGs kept, and the rasters made from them with their bytes.
    pub svgs: usize,
    pub svg_rasters: usize,
    pub svg_raster_bytes: usize,
    /// Rasters made so far, including ones dropped since.
    pub svg_rasters_made: u64,
    /// Fonts added with `register_font` (identical bytes count once).
    pub fonts_registered: usize,
    /// Surface sizes whose buffers are kept, those buffers' bytes, and how
    /// many pixmaps and masks were ever created (a reused one is not counted).
    pub scratch_surfaces: usize,
    pub scratch_bytes: usize,
    pub scratch_allocations: u64,
    /// Bytes the shadow blur keeps between frames.
    pub shadow_bytes: usize,
}

pub struct Renderer {
    text: text::TextEngine,
    images: images::ImageCache,
    svgs: svg::SvgCache,
    scratch: paint::Scratch,
}

impl Renderer {
    /// Loads the system font collection (once per process; measured in the
    /// first-frame budget).
    pub fn new(fonts: FontConfig) -> Renderer {
        Renderer {
            text: text::TextEngine::new(fonts),
            images: images::ImageCache::default(),
            svgs: svg::SvgCache::default(),
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
        self.svgs.begin_frame();
        let painted = paint::paint(&mut self.scratch, &mut self.text, &mut self.images, &mut self.svgs, list, region);
        self.text.end_frame();
        painted?;
        paint::copy_out(&self.scratch, target, region);
        Ok(Some(region))
    }

    /// True when the last `render` left users' SVGs undrawn to keep the frame
    /// short: render again (the whole surface) soon and they appear.
    pub fn svg_pending(&self) -> bool {
        self.svgs.pending()
    }

    /// Adds a font file's faces to the font collection (TrueType, OpenType
    /// or a collection; variable fonts keep their axes). `family` renames
    /// every face (an `@font-face` rule's `font-family`); None keeps the
    /// names in the file. Returns the family names now usable in
    /// `font-family`. Fonts stay for the life of the renderer; registering
    /// the same bytes again adds nothing. Files over `FONT_MAX_FILE_BYTES`
    /// and data that holds no face are refused.
    pub fn register_font(&mut self, data: Arc<[u8]>, family: Option<&str>) -> Result<Vec<String>, RenderError> {
        self.text.register_font(data, family)
    }

    /// Parses a bundled SVG icon and keeps it under `key` for
    /// `DrawCmd::Svg` (`parse_svg(bytes, SvgSource::Bundled)` then
    /// `set_parsed_svg`). Bundled icons are a few hundred bytes, so this is
    /// done on the UI thread at first use. Returns its intrinsic size in px.
    pub fn set_svg(&mut self, key: Arc<str>, bytes: &[u8]) -> Result<(f32, f32), RenderError> {
        let svg = parse_svg(bytes, SvgSource::Bundled)?;
        Ok(self.svgs.set(key, svg))
    }

    /// Keeps an SVG parsed elsewhere (a user's `.svg` icon or image is
    /// parsed on the image loader thread with `parse_svg`, never on the UI
    /// thread). Returns its intrinsic size in px.
    pub fn set_parsed_svg(&mut self, key: Arc<str>, svg: ParsedSvg) -> (f32, f32) {
        self.svgs.set(key, svg)
    }

    /// Drops the image stored under exactly `key`; true when there was one.
    pub fn remove_image(&mut self, key: &str) -> bool {
        self.images.remove(key)
    }

    /// Drops every image whose key starts with `prefix` (app icons once
    /// the launcher's subscription ends); returns how many.
    pub fn remove_images(&mut self, prefix: &str) -> usize {
        self.images.remove_prefix(prefix)
    }

    pub fn has_svg(&self, key: &str) -> bool {
        self.svgs.has(key)
    }

    /// Images are decoded off the UI thread and handed in here; an image
    /// item whose key is missing draws nothing.
    pub fn set_image(&mut self, key: Arc<str>, image: Image) {
        self.images.set(key, image);
    }

    pub fn has_image(&self, key: &str) -> bool {
        self.images.has(key)
    }

    /// A surface of this size is gone (a popup closed): its buffers, and the
    /// shadow blur's, are given back instead of waiting for the renderer to end.
    pub fn release_surface(&mut self, width: u32, height: u32) {
        self.scratch.release(width, height);
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
            svgs: self.svgs.len(),
            svg_rasters: self.svgs.raster_count(),
            svg_raster_bytes: self.svgs.raster_bytes(),
            svg_rasters_made: self.svgs.rasters_made(),
            fonts_registered: t.fonts_registered,
            scratch_surfaces: self.scratch.surfaces(),
            scratch_bytes: self.scratch.bytes(),
            scratch_allocations: self.scratch.allocations(),
            shadow_bytes: self.scratch.shadow_bytes(),
        }
    }
}

impl TextMeasure for Renderer {
    fn measure(&mut self, text: &str, style: &TextStyle, max_width: Option<f32>) -> SizeF {
        // single line only: wrapping is reserved for later
        let _ = max_width;
        self.text.measure(text, style)
    }

    fn caret_stops(&mut self, text: &str, style: &TextStyle) -> Vec<(usize, f32)> {
        self.text.caret_stops(text, style)
    }
}

fn check_size(width: u32, height: u32) -> Result<(), RenderError> {
    if width == 0 || height == 0 || width > MAX_SURFACE_SIDE || height > MAX_SURFACE_SIDE {
        return Err(RenderError::BadSize);
    }
    Ok(())
}

/// Where an SVG comes from: a user's file gets the stricter checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvgSource {
    Bundled,
    User,
}

/// A parsed SVG, ready for `Renderer::set_parsed_svg`. `Send`, so the
/// parse can run on any thread.
#[derive(Clone)]
pub struct ParsedSvg {
    pub(crate) tree: resvg::usvg::Tree,
    /// Layers one render of a user's file needs; `None` for a bundled file,
    /// which is not limited.
    pub(crate) layers: Option<u64>,
}

impl ParsedSvg {
    /// Intrinsic size in px (the `viewBox` or the width and height).
    pub fn size(&self) -> (f32, f32) {
        let s = self.tree.size();
        (s.width(), s.height())
    }
}

impl std::fmt::Debug for ParsedSvg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ParsedSvg")
    }
}

/// Parses an SVG for drawing. References to other files, fonts, text and
/// embedded raster images are ignored: an SVG draws shapes only (the
/// `usvg::Options` refuse every href and have no resources folder). Files
/// over `SVG_MAX_FILE_BYTES` are refused; a `User` file is also refused
/// when it holds more than `SVG_MAX_NODES` elements or any `filter`.
pub fn parse_svg(bytes: &[u8], source: SvgSource) -> Result<ParsedSvg, RenderError> {
    svg::parse(bytes, source)
}

// The image loader thread parses user SVGs and hands them to the UI thread.
const _: () = {
    const fn send<T: Send>() {}
    send::<ParsedSvg>();
};

/// Renders a whole draw list into a new canvas.
pub fn render_offscreen(renderer: &mut Renderer, list: &DrawList) -> Result<Canvas, RenderError> {
    check_size(list.width, list.height)?;
    let mut canvas = Canvas::new(list.width, list.height);
    renderer.render(list, &mut canvas.pixels(), None)?;
    Ok(canvas)
}

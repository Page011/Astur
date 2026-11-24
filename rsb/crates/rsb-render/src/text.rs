//! Text: Parley lays out (bidi, shaping, system font fallback), swash renders
//! glyph images, both behind caches bounded by the limits in `lib.rs`.
//!
//! This module is the only place that knows about Parley and swash, so a
//! DirectWrite path could replace it without touching the rest.
//!
//! Line box: width and height come from the layout, so the height follows
//! the tallest font used on the line (an emoji can make a line a pixel or
//! two taller than the same text without one). Measuring and drawing read
//! the same cached layout, so they always agree.
//!
//! Vertical position: a line box has room for descenders, so centring the box
//! leaves letters and digits low. Text that starts with a letter or digit is
//! centred on its capital height instead; icons and symbols keep the box.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use parley::setting::Tag;
use parley::style::{FontFamily, FontFamilyName, FontFeature, FontFeatures, FontStyle, FontWeight};
use parley::{FontContext, Layout, LayoutContext, PositionedLayoutItem, StyleProperty};
use rsb_core::color::Color;
use rsb_core::draw::TextStyle;
use rsb_core::geom::{RectF, RectPx, SizeF};
use rsb_core::icons::IconFont;
use rsb_core::style::TextAlign;
use swash::FontRef;
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Format, Vector};
use tiny_skia::{Mask, Pixmap};

use crate::{FontConfig, GLYPH_CACHE_MAX_BYTES, LAYOUT_CACHE_MAX};

/// Horizontal glyph positions are quantised to this many steps per pixel so
/// cached glyph images are reused across draws.
const SUBPIXEL_STEPS: f32 = 4.0;
/// Fixed cost counted per cached glyph on top of its pixels.
const GLYPH_ENTRY_OVERHEAD: usize = 64;
/// Text longer than this many bytes is cut before shaping.
const MAX_TEXT_BYTES: usize = 16 * 1024;
/// Glyphs for fonts bigger than this are not drawn (and so not cached).
const MAX_GLYPH_PX: f32 = 1024.0;
/// Light text on a dark background looks thin with plain coverage blending
/// (no gamma); its glyph coverage is lifted by this exponent (below 1 = bolder).
const LIGHT_TEXT_COVERAGE_POWER: f32 = 0.78;

#[derive(Clone, PartialEq, Eq, Hash)]
struct StyleKey {
    families: Arc<[Arc<str>]>,
    size_bits: u32,
    weight: u16,
    italic: bool,
    spacing_bits: u32,
    tabular: bool,
}

impl StyleKey {
    fn new(s: &TextStyle) -> StyleKey {
        // -0.0 and 0.0 must be one key
        let spacing = if s.letter_spacing == 0.0 { 0.0 } else { s.letter_spacing };
        StyleKey {
            families: s.families.clone(),
            size_bits: s.size_px.to_bits(),
            weight: s.weight,
            italic: s.italic,
            spacing_bits: spacing.to_bits(),
            tabular: s.tabular_nums,
        }
    }
}

struct LayoutEntry {
    layout: Layout<()>,
    width: f32,
    height: f32,
    /// Added to the line box's top so capitals centre on the box (see the module doc).
    cap_shift: f32,
    used: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    blob: u64,
    index: u32,
    glyph: u32,
    size_bits: u32,
    sub: u8,
    /// FNV-1a of the variation coordinates; no allocation per glyph.
    coords: u64,
}

struct GlyphImage {
    left: i32,
    top: i32,
    w: u32,
    h: u32,
    colour: bool,
    /// Alpha mask (1 byte per pixel) or straight RGBA (4 bytes per pixel).
    data: Vec<u8>,
}

struct GlyphEntry {
    /// None for glyphs with no ink (spaces): remembered so they are not
    /// rendered again.
    image: Option<GlyphImage>,
    used: u64,
    bytes: usize,
}

pub(crate) struct TextEngine {
    font_cx: FontContext,
    layout_cx: LayoutContext<()>,
    scale_cx: ScaleContext,
    config: FontConfig,
    icon: IconFont,
    /// Installed symbol families (Nerd Fonts) appended to every stack.
    symbols: Vec<String>,
    layouts: HashMap<StyleKey, HashMap<Box<str>, LayoutEntry>>,
    layout_count: usize,
    glyphs: HashMap<GlyphKey, GlyphEntry>,
    glyph_bytes: usize,
    tick: u64,
    frame_start: u64,
    colour_drawn: u64,
}

#[derive(Default, Clone, Copy)]
pub(crate) struct TextStats {
    pub glyphs: usize,
    pub glyph_bytes: usize,
    pub layouts: usize,
    pub colour_drawn: u64,
}

impl TextEngine {
    pub(crate) fn new(config: FontConfig) -> TextEngine {
        let mut font_cx = FontContext::new();
        let icon = if font_cx.collection.family_id(IconFont::Fluent.family()).is_some() {
            IconFont::Fluent
        } else if font_cx.collection.family_id(IconFont::Mdl2.family()).is_some() {
            IconFont::Mdl2
        } else {
            IconFont::Fluent
        };
        let symbols = find_symbol_families(&mut font_cx);
        TextEngine {
            font_cx,
            layout_cx: LayoutContext::new(),
            scale_cx: ScaleContext::new(),
            config,
            icon,
            symbols,
            layouts: HashMap::new(),
            layout_count: 0,
            glyphs: HashMap::new(),
            glyph_bytes: 0,
            tick: 0,
            frame_start: 0,
            colour_drawn: 0,
        }
    }

    pub(crate) fn icon_font(&self) -> IconFont {
        self.icon
    }

    pub(crate) fn has_family(&mut self, name: &str) -> bool {
        self.font_cx.collection.family_id(name.trim()).is_some()
    }

    pub(crate) fn stats(&self) -> TextStats {
        TextStats {
            glyphs: self.glyphs.len(),
            glyph_bytes: self.glyph_bytes,
            layouts: self.layout_count,
            colour_drawn: self.colour_drawn,
        }
    }

    /// Called once per `render`: glyphs touched from here on are kept by the
    /// trim that ends the frame.
    pub(crate) fn begin_frame(&mut self) {
        self.frame_start = self.tick;
    }

    /// Drops the least recently used glyph images until the cache is under
    /// its byte cap. Images drawn in this frame are never dropped.
    pub(crate) fn end_frame(&mut self) {
        if self.glyph_bytes <= GLYPH_CACHE_MAX_BYTES {
            return;
        }
        let mut old: Vec<(u64, GlyphKey)> =
            self.glyphs.iter().filter(|(_, e)| e.used <= self.frame_start).map(|(k, e)| (e.used, *k)).collect();
        old.sort_unstable_by_key(|(used, _)| *used);
        // go well below the cap so the next frames do not trim again at once
        let target = GLYPH_CACHE_MAX_BYTES / 4 * 3;
        for (_, key) in old {
            if self.glyph_bytes <= target {
                break;
            }
            if let Some(e) = self.glyphs.remove(&key) {
                self.glyph_bytes -= e.bytes;
            }
        }
    }

    /// Size of one line of text: advance width and line height.
    pub(crate) fn measure(&mut self, text: &str, style: &TextStyle) -> SizeF {
        let text = cut_text(text);
        self.ensure(text, style);
        match self.layouts.get(&StyleKey::new(style)).and_then(|m| m.get(text)) {
            Some(e) => SizeF { w: e.width, h: e.height },
            None => SizeF::default(),
        }
    }

    fn family_stack(&self, style: &TextStyle) -> Vec<FontFamilyName<'static>> {
        let mut names: Vec<String> = Vec::new();
        let mut push = |n: &str| {
            if !n.is_empty() && !names.iter().any(|e| e.eq_ignore_ascii_case(n)) {
                names.push(n.to_string());
            }
        };
        for f in style.families.iter() {
            match f.trim().to_ascii_lowercase().as_str() {
                "sans-serif" => push(&self.config.sans_serif),
                "monospace" => self.config.monospace.iter().for_each(|m| push(m)),
                "icons" => push(self.icon.family()),
                _ => push(f.trim()),
            }
        }
        // a missing family ends up in the configured sans family, then the
        // emoji and symbol families, then whatever the system falls back to
        push(&self.config.sans_serif);
        push(&self.config.emoji);
        self.symbols.iter().for_each(|s| push(s));
        names.into_iter().map(|n| FontFamilyName::Named(Cow::Owned(n))).collect()
    }

    fn build_layout(&mut self, text: &str, style: &TextStyle) -> Layout<()> {
        let stack = self.family_stack(style);
        let size = if style.size_px.is_finite() { style.size_px.clamp(1.0, MAX_GLYPH_PX) } else { 13.0 };
        let spacing = if style.letter_spacing.is_finite() { style.letter_spacing.clamp(-64.0, 256.0) } else { 0.0 };
        let mut b = self.layout_cx.ranged_builder(&mut self.font_cx, text, 1.0, true);
        b.push_default(StyleProperty::FontFamily(FontFamily::List(Cow::Owned(stack))));
        b.push_default(StyleProperty::FontSize(size));
        b.push_default(StyleProperty::FontWeight(FontWeight::new(style.weight.clamp(1, 1000) as f32)));
        if style.italic {
            b.push_default(StyleProperty::FontStyle(FontStyle::Italic));
        }
        if spacing != 0.0 {
            b.push_default(StyleProperty::LetterSpacing(spacing));
        }
        if style.tabular_nums {
            let tnum = FontFeature::new(Tag::new(b"tnum"), 1);
            b.push_default(StyleProperty::FontFeatures(FontFeatures::List(Cow::Owned(vec![tnum]))));
        }
        let mut layout: Layout<()> = b.build(text);
        layout.break_all_lines(None);
        layout
    }

    /// Makes sure the layout for (text, style) is cached and fresh.
    fn ensure(&mut self, text: &str, style: &TextStyle) {
        self.tick += 1;
        let tick = self.tick;
        let key = StyleKey::new(style);
        if let Some(e) = self.layouts.get_mut(&key).and_then(|m| m.get_mut(text)) {
            e.used = tick;
            return;
        }
        let layout = self.build_layout(text, style);
        while self.layout_count >= LAYOUT_CACHE_MAX {
            if !self.evict_oldest_layout() {
                break;
            }
        }
        let entry = LayoutEntry {
            width: layout.full_width(),
            height: layout.height(),
            cap_shift: cap_shift(&layout, text),
            layout,
            used: tick,
        };
        self.layouts.entry(key).or_default().insert(text.into(), entry);
        self.layout_count += 1;
    }

    fn evict_oldest_layout(&mut self) -> bool {
        let oldest = self
            .layouts
            .iter()
            .flat_map(|(sk, m)| m.iter().map(move |(t, e)| (e.used, sk, t)))
            .min_by_key(|(used, _, _)| *used)
            .map(|(_, sk, t)| (sk.clone(), t.clone()));
        let Some((sk, t)) = oldest else { return false };
        if let Some(m) = self.layouts.get_mut(&sk) {
            m.remove(&t);
            if m.is_empty() {
                self.layouts.remove(&sk);
            }
            self.layout_count -= 1;
        }
        true
    }

    /// Draws one line of text inside `rect`, clipped to `vis` (and `mask`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw(
        &mut self,
        dst: &mut Pixmap,
        mask: Option<&Mask>,
        vis: RectPx,
        rect: &RectF,
        text: &str,
        style: &TextStyle,
        color: Color,
        align: TextAlign,
    ) {
        let text = cut_text(text);
        if text.is_empty() || vis.is_empty() {
            return;
        }
        self.ensure(text, style);
        let key = StyleKey::new(style);
        let TextEngine { layouts, glyphs, glyph_bytes, scale_cx, tick, colour_drawn, .. } = self;
        let Some(entry) = layouts.get(&key).and_then(|m| m.get(text)) else { return };
        let layout = &entry.layout;

        let lw = entry.width;
        let ox = match align {
            TextAlign::Start => rect.x,
            TextAlign::Center => rect.x + (rect.w - lw) / 2.0,
            TextAlign::End => rect.x + rect.w - lw,
        };
        let oy = rect.y + (rect.h - entry.height) / 2.0 + entry.cap_shift;

        let lut = coverage_table(luma(color) > 0.5);
        let ca = (color.a.clamp(0.0, 1.0) * 255.0).round() as u32;
        let ch = |v: f32| (v.clamp(0.0, 1.0) * ca as f32).round() as u32;
        let premul = [ch(color.r), ch(color.g), ch(color.b)];

        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(run) = item else { continue };
                let font = run.run().font();
                let blob = font.data.id();
                let index = font.index;
                let fsize = run.run().font_size();
                if !(fsize > 0.0 && fsize <= MAX_GLYPH_PX) {
                    continue;
                }
                let coords = run.run().normalized_coords();
                let coords_hash = fnv(coords.iter().map(|c| c.to_bits() as u16 as u64));
                for g in run.positioned_glyphs() {
                    let gx = ox + g.x;
                    let gy = (oy + g.y).round();
                    let ix = gx.floor();
                    let sub = (((gx - ix) * SUBPIXEL_STEPS).floor() as u8).min(SUBPIXEL_STEPS as u8 - 1);
                    let gk =
                        GlyphKey { blob, index, glyph: g.id, size_bits: fsize.to_bits(), sub, coords: coords_hash };
                    *tick += 1;
                    let entry = glyphs.entry(gk).or_insert_with(|| {
                        let norm: Vec<i16> = coords.iter().map(|c| c.to_bits()).collect();
                        let image = render_glyph(
                            scale_cx,
                            font.data.data(),
                            index,
                            g.id,
                            fsize,
                            sub as f32 / SUBPIXEL_STEPS,
                            &norm,
                        );
                        let bytes = GLYPH_ENTRY_OVERHEAD + image.as_ref().map_or(0, |i| i.data.len());
                        *glyph_bytes += bytes;
                        GlyphEntry { image, used: 0, bytes }
                    });
                    entry.used = *tick;
                    if let Some(img) = &entry.image {
                        if img.colour {
                            *colour_drawn += 1;
                        }
                        blit(dst, mask, vis, img, ix as i32, gy as i32, premul, ca, lut);
                    }
                }
            }
        }
    }
}

/// Truncates at a char boundary so one absurd string cannot stall shaping.
/// How far the line box must move down so the capital letters, not the box
/// with its empty descender space, are centred in it. Zero for text that does
/// not start with a letter or digit (icons, symbols, emoji) and for fonts that
/// do not say their capital height.
fn cap_shift(layout: &Layout<()>, text: &str) -> f32 {
    if !text.trim_start().chars().next().is_some_and(char::is_alphanumeric) {
        return 0.0;
    }
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(run) = item else { continue };
            let font = run.run().font();
            let size = run.run().font_size();
            let Some(f) = FontRef::from_index(font.data.data(), font.index as usize) else { return 0.0 };
            let cap = f.metrics(&[]).scale(size).cap_height;
            if !(cap.is_finite() && cap > 0.0 && cap < size * 2.0) {
                return 0.0;
            }
            // The box is centred, so its middle is at height / 2; the capitals' middle is cap / 2 above the baseline.
            return layout.height() / 2.0 - (run.baseline() - cap / 2.0);
        }
    }
    0.0
}

fn cut_text(text: &str) -> &str {
    if text.len() <= MAX_TEXT_BYTES {
        return text;
    }
    let mut end = MAX_TEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn fnv(values: impl Iterator<Item = u64>) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for v in values {
        h ^= v;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Installed symbol-only families (Nerd Fonts), in preference order.
fn find_symbol_families(cx: &mut FontContext) -> Vec<String> {
    let mut out = Vec::new();
    for name in ["Symbols Nerd Font Mono", "Symbols Nerd Font"] {
        if cx.collection.family_id(name).is_some() {
            out.push(name.to_string());
        }
    }
    if out.is_empty() {
        let found = cx.collection.family_names().find(|n| n.ends_with("Nerd Font Mono") || n.ends_with("Nerd Font"));
        out.extend(found.map(str::to_string));
    }
    out
}

fn render_glyph(
    cx: &mut ScaleContext,
    data: &[u8],
    index: u32,
    glyph: u32,
    size: f32,
    frac: f32,
    coords: &[i16],
) -> Option<GlyphImage> {
    let font = FontRef::from_index(data, index as usize)?;
    let mut scaler = cx.builder(font).size(size).hint(false).normalized_coords(coords.iter().copied()).build();
    let img = Render::new(&[Source::ColorOutline(0), Source::ColorBitmap(StrikeWith::BestFit), Source::Outline])
        .format(Format::Alpha)
        .offset(Vector::new(frac, 0.0))
        .render(&mut scaler, u16::try_from(glyph).ok()?)?;
    if img.placement.width == 0 || img.placement.height == 0 {
        return None;
    }
    Some(GlyphImage {
        left: img.placement.left,
        top: img.placement.top,
        w: img.placement.width,
        h: img.placement.height,
        colour: img.content == Content::Color,
        data: img.data,
    })
}

fn luma(c: Color) -> f32 {
    0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b
}

/// Glyph coverage remapping: identity for dark text, lifted for light text.
fn coverage_table(light: bool) -> &'static [u8; 256] {
    static TABLES: OnceLock<[[u8; 256]; 2]> = OnceLock::new();
    let t = TABLES.get_or_init(|| {
        let mut t = [[0u8; 256]; 2];
        let [plain, lifted] = &mut t;
        for (i, (p, l)) in plain.iter_mut().zip(lifted.iter_mut()).enumerate() {
            *p = i as u8;
            *l = ((i as f32 / 255.0).powf(LIGHT_TEXT_COVERAGE_POWER) * 255.0).round() as u8;
        }
        t
    });
    &t[light as usize]
}

#[inline]
fn div255(v: u32) -> u32 {
    (v + 127) / 255
}

/// Source-over of a glyph image onto the premultiplied RGBA canvas, limited
/// to `vis` and scaled by the clip mask. `pc` is the text colour premultiplied
/// by `ca` (its alpha, 0..255); a colour glyph's own alpha is scaled by `ca`
/// too, so `color: rgba(.., 0.5)` fades emoji like it fades letters.
#[allow(clippy::too_many_arguments)]
fn blit(
    dst: &mut Pixmap,
    mask: Option<&Mask>,
    vis: RectPx,
    img: &GlyphImage,
    ox: i32,
    oy: i32,
    pc: [u32; 3],
    ca: u32,
    lut: &[u8; 256],
) {
    let width = dst.width() as usize;
    let x_off = ox + img.left;
    let y_off = oy - img.top;
    let mask_data = mask.map(|m| m.data());
    let data = dst.data_mut();
    for row in 0..img.h as i32 {
        let y = y_off + row;
        if y < vis.y0 || y >= vis.y1 {
            continue;
        }
        for col in 0..img.w as i32 {
            let x = x_off + col;
            if x < vis.x0 || x >= vis.x1 {
                continue;
            }
            let i = row as usize * img.w as usize + col as usize;
            let (mut sr, mut sg, mut sb, mut sa) = if img.colour {
                let p = &img.data[i * 4..i * 4 + 4];
                let a = div255(p[3] as u32 * ca);
                (div255(p[0] as u32 * a), div255(p[1] as u32 * a), div255(p[2] as u32 * a), a)
            } else {
                let c = lut[img.data[i] as usize] as u32;
                (div255(pc[0] * c), div255(pc[1] * c), div255(pc[2] * c), div255(ca * c))
            };
            let pos = y as usize * width + x as usize;
            if let Some(m) = mask_data {
                let cov = m[pos] as u32;
                if cov == 0 {
                    continue;
                }
                if cov != 255 {
                    sr = div255(sr * cov);
                    sg = div255(sg * cov);
                    sb = div255(sb * cov);
                    sa = div255(sa * cov);
                }
            }
            if sa == 0 {
                continue;
            }
            let d = &mut data[pos * 4..pos * 4 + 4];
            let inv = 255 - sa;
            d[0] = (sr + div255(d[0] as u32 * inv)).min(255) as u8;
            d[1] = (sg + div255(d[1] as u32 * inv)).min(255) as u8;
            d[2] = (sb + div255(d[2] as u32 * inv)).min(255) as u8;
            d[3] = (sa + div255(d[3] as u32 * inv)).min(255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(spacing: f32) -> TextStyle {
        TextStyle {
            families: vec![Arc::<str>::from("Segoe UI")].into(),
            size_px: 13.0,
            weight: 400,
            italic: false,
            letter_spacing: spacing,
            tabular_nums: false,
        }
    }

    #[test]
    fn negative_zero_spacing_is_the_same_key() {
        assert!(StyleKey::new(&style(0.0)) == StyleKey::new(&style(-0.0)));
        assert!(StyleKey::new(&style(0.0)) != StyleKey::new(&style(0.5)));
    }

    #[test]
    fn long_text_is_cut_on_a_char_boundary() {
        let long = "\u{20AC}".repeat(MAX_TEXT_BYTES);
        let cut = cut_text(&long);
        assert!(cut.len() <= MAX_TEXT_BYTES && cut.len() > MAX_TEXT_BYTES - 3);
        assert_eq!(cut_text("short"), "short");
    }

    #[test]
    fn light_text_coverage_is_lifted_and_dark_is_untouched() {
        let dark = coverage_table(false);
        let light = coverage_table(true);
        assert!((0..256).all(|i| dark[i] == i as u8));
        assert_eq!((light[0], light[255]), (0, 255));
        assert!(light[128] > 128 && light.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn luma_orders_light_above_dark() {
        assert!(luma(Color::WHITE) > 0.5 && luma(Color::BLACK) < 0.5);
        assert!(luma(Color::rgba8(0x1b, 0x1e, 0x24, 255)) < 0.5);
        assert!(luma(Color::rgba8(0xC0, 0xCA, 0xF5, 255)) > 0.5);
    }
}

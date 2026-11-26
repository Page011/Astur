//! Text measurement seam. The scene lays boxes out with Taffy (flexbox) and
//! asks a `TextMeasure` how big each text run is; the renderer implements it
//! with the same shaping it draws with, so measured and drawn text agree.

use crate::draw::TextStyle;
use crate::geom::SizeF;

pub trait TextMeasure {
    /// Size of `text` on one line in physical px: advance width and line
    /// height (ascent + descent + line gap of the primary font). `max_width`
    /// is reserved for wrapping text later; single-line callers pass None.
    fn measure(&mut self, text: &str, style: &TextStyle, max_width: Option<f32>) -> SizeF;

    /// Where a caret may stand in `text` laid out on one line: `(byte
    /// index, x offset in physical px)` for the start, every cluster
    /// boundary (a base character with its combining marks, an emoji
    /// sequence, a ligature counts as its characters) and the end, in byte
    /// order. Text editing moves the caret between these and hit-tests
    /// clicks against them. The default steps over chars and measures each
    /// prefix; the renderer answers from its shaped layout.
    fn caret_stops(&mut self, text: &str, style: &TextStyle) -> Vec<(usize, f32)> {
        let mut stops: Vec<(usize, f32)> = vec![(0, 0.0)];
        for (i, c) in text.char_indices() {
            let end = i + c.len_utf8();
            stops.push((end, self.measure(&text[..end], style, None).w));
        }
        stops
    }
}

/// A measurer for tests: every char is `0.6 * size` wide, lines are
/// `1.25 * size` high. Deterministic, no fonts.
pub struct FixedMeasure;

impl TextMeasure for FixedMeasure {
    fn measure(&mut self, text: &str, style: &TextStyle, _max_width: Option<f32>) -> SizeF {
        SizeF { w: text.chars().count() as f32 * 0.6 * style.size_px, h: 1.25 * style.size_px }
    }
}

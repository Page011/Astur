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
}

/// A measurer for tests: every char is `0.6 * size` wide, lines are
/// `1.25 * size` high. Deterministic, no fonts.
pub struct FixedMeasure;

impl TextMeasure for FixedMeasure {
    fn measure(&mut self, text: &str, style: &TextStyle, _max_width: Option<f32>) -> SizeF {
        SizeF { w: text.chars().count() as f32 * 0.6 * style.size_px, h: 1.25 * style.size_px }
    }
}

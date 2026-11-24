//! The CSS subset. Hand-written tokenizer and parser so every diagnostic
//! carries file:line:col and nothing is silently ignored: an unknown
//! property, bad value, unknown selector or unsupported at-rule is a warning
//! and only that declaration (or rule) is dropped.
//!
//! The grammar and property table are documented in `docs/authoring.md`;
//! `PROPERTIES` is the source of truth for both the docs and the Luau
//! `Style` type.
//!
//! Custom properties and `var()`: a declaration whose value holds `var()`
//! keeps its raw tokens and position. At computed-value time the cascade
//! substitutes the variables (fallbacks, cycle detection), then parses the
//! result with the same property parser, shorthands included. A value that
//! is invalid after substitution makes that property unset (inherited or
//! initial, as CSS does) and gives one warning per stylesheet version with
//! the declaration's position and the variable's definition position,
//! through `Scene::take_diagnostics`.
//!
//! `:root` is accepted as another name for `bar` (themes put their
//! variables there). Unknown properties get a did-you-mean hint (edit
//! distance <= 2); `pt` and `rem` get "use px" and "use em" hints;
//! `@define-color` gets "use a custom property: --name: value";
//! `text-shadow` gets "not supported in this version".
//!
//! `@import "colors.css";` (only before the first rule) pulls in a file
//! relative to the config folder, never outside it; the parser only records
//! imports (`Stylesheet::imports`), the binary's watch thread reads and
//! parses them (at most 8 files, 4 levels) and puts their rules before the
//! importing sheet's. Any `.css` change in the config folder re-reads them.

/// Names under which the built-in sheets are parsed: positions in these files
/// are not the user's to edit.
pub const BASE_SHEET: &str = "base.css";
pub const PRESET_SHEET: &str = "preset.css";
pub(crate) const BUILTIN_SHEETS: [&str; 2] = [BASE_SHEET, PRESET_SHEET];

use crate::diag::{Diagnostic, SourcePos};

pub(crate) mod cascade;
pub(crate) mod parse;
pub(crate) mod select;
pub(crate) mod value;

/// A parsed style sheet. Later sheets appended with `append` win over
/// earlier ones at equal specificity (preset CSS first, user CSS last).
#[derive(Clone, Debug, Default)]
pub struct Stylesheet {
    /// Selector lists are split: one entry per selector, in source order.
    pub(crate) rules: Vec<parse::Rule>,
    pub(crate) imports: Vec<(String, SourcePos)>,
    pub(crate) families: Vec<(String, SourcePos)>,
}

impl Stylesheet {
    pub fn empty() -> Stylesheet {
        Stylesheet::default()
    }

    /// Parses `text`. `file` is the name used in diagnostics (`style.css`).
    /// Never fails: what cannot be used is reported and skipped. Problems
    /// that make the file unusable as a whole (an unterminated comment or
    /// block, a stray `}`) come back with `Severity::Error`, so the caller
    /// can keep the last good sheet; everything else is a warning and only
    /// that declaration or rule is dropped.
    pub fn parse(text: &str, file: &str) -> (Stylesheet, Vec<Diagnostic>) {
        parse::parse_sheet(text, file)
    }

    /// Appends `later`'s rules after this sheet's.
    pub fn append(&mut self, later: Stylesheet) {
        self.rules.extend(later.rules);
        self.imports.extend(later.imports);
        self.families.extend(later.families);
    }

    /// Number of rules; a selector list counts once per selector.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// `@import` targets in order, as written, with their positions.
    pub fn imports(&self) -> Vec<(String, SourcePos)> {
        self.imports.clone()
    }

    /// Every family name used in `font-family` (generic names left out),
    /// with the declaration's position. The binary checks each against the
    /// installed fonts and warns `font 'X' is not installed; using Segoe UI`.
    /// A name written through `var()` is not listed.
    pub fn font_families(&self) -> Vec<(String, SourcePos)> {
        self.families.clone()
    }
}

/// Inline style from a script: `(property name, value text)` pairs, names in
/// CSS form (`font-size`; the script turns `font_size` into it). `pos` is
/// the widget file, used for warnings. A unitless number counts as px where
/// a length is expected.
pub fn parse_inline(decls: &[(String, String)], pos: Option<SourcePos>) -> (InlineDecls, Vec<Diagnostic>) {
    let mut diags = Vec::new();
    let mut out = Vec::new();
    for (name, value) in decls {
        let name = if name.starts_with("--") { name.clone() } else { name.replace('_', "-") };
        if let Some(d) = parse::parse_decl(&name, value, pos.clone(), value::Ctx { inline: true }, &mut diags, None) {
            out.push(d);
        }
    }
    (InlineDecls { decls: out }, diags)
}

/// Parsed inline declarations, applied above every rule.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InlineDecls {
    pub(crate) decls: Vec<parse::Decl>,
}

/// Environment for `@media` queries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaEnv {
    /// `prefers-color-scheme: dark` matches when true.
    pub dark: bool,
}

/// Properties of the subset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PropertyId {
    Color,
    Background,
    BackgroundColor,
    Opacity,
    Border,
    BorderWidth,
    BorderColor,
    BorderRadius,
    BorderTopLeftRadius,
    BorderTopRightRadius,
    BorderBottomRightRadius,
    BorderBottomLeftRadius,
    BorderTop,
    BorderRight,
    BorderBottom,
    BorderLeft,
    Padding,
    PaddingTop,
    PaddingRight,
    PaddingBottom,
    PaddingLeft,
    Margin,
    MarginTop,
    MarginRight,
    MarginBottom,
    MarginLeft,
    Gap,
    FlexDirection,
    FlexGrow,
    FlexShrink,
    JustifyContent,
    AlignItems,
    Width,
    Height,
    MinWidth,
    MinHeight,
    MaxWidth,
    MaxHeight,
    FontFamily,
    FontSize,
    FontWeight,
    FontStyle,
    FontVariantNumeric,
    LetterSpacing,
    TextAlign,
    TextOverflow,
    Overflow,
    BoxShadow,
    Display,
    Visibility,
    Transition,
    TransitionProperty,
    TransitionDuration,
    TransitionDelay,
    TransitionTimingFunction,
    Cursor,
    Backdrop,
}

/// One row of the property table.
#[derive(Clone, Copy, Debug)]
pub struct PropertyInfo {
    pub id: PropertyId,
    /// CSS name, e.g. `font-size`. The Luau inline-style key is the same with
    /// `_` for `-` (`font_size`).
    pub name: &'static str,
    /// Accepted values in the docs' notation.
    pub values: &'static str,
    pub inherited: bool,
    /// Transitions interpolate it (colours, opacity, lengths).
    pub animatable: bool,
    /// Only meaningful on the `bar` element; a warning elsewhere.
    pub bar_only: bool,
}

macro_rules! prop {
    ($id:ident, $name:literal, $values:literal, $inh:literal, $anim:literal) => {
        PropertyInfo {
            id: PropertyId::$id,
            name: $name,
            values: $values,
            inherited: $inh,
            animatable: $anim,
            bar_only: false,
        }
    };
}

/// The whole subset. Shorthands expand to their longhands when parsed.
pub static PROPERTIES: &[PropertyInfo] = &[
    prop!(Color, "color", "<color>", true, true),
    prop!(Background, "background", "<color> | <linear-gradient> | none", false, true),
    prop!(BackgroundColor, "background-color", "<color>", false, true),
    prop!(Opacity, "opacity", "<number 0..1> | <percentage>", false, true),
    prop!(Border, "border", "<length> || solid | none || <color>", false, true),
    prop!(BorderWidth, "border-width", "<length>{1,4}", false, true),
    prop!(BorderColor, "border-color", "<color>", false, true),
    prop!(BorderRadius, "border-radius", "<length-percentage>{1,4}", false, true),
    prop!(BorderTopLeftRadius, "border-top-left-radius", "<length-percentage>", false, true),
    prop!(BorderTopRightRadius, "border-top-right-radius", "<length-percentage>", false, true),
    prop!(BorderBottomRightRadius, "border-bottom-right-radius", "<length-percentage>", false, true),
    prop!(BorderBottomLeftRadius, "border-bottom-left-radius", "<length-percentage>", false, true),
    prop!(BorderTop, "border-top", "<length> || solid | none || <color>", false, true),
    prop!(BorderRight, "border-right", "<length> || solid | none || <color>", false, true),
    prop!(BorderBottom, "border-bottom", "<length> || solid | none || <color>", false, true),
    prop!(BorderLeft, "border-left", "<length> || solid | none || <color>", false, true),
    prop!(Padding, "padding", "<length>{1,4}", false, true),
    prop!(PaddingTop, "padding-top", "<length>", false, true),
    prop!(PaddingRight, "padding-right", "<length>", false, true),
    prop!(PaddingBottom, "padding-bottom", "<length>", false, true),
    prop!(PaddingLeft, "padding-left", "<length>", false, true),
    prop!(Margin, "margin", "(<length> | auto){1,4}", false, true),
    prop!(MarginTop, "margin-top", "<length> | auto", false, true),
    prop!(MarginRight, "margin-right", "<length> | auto", false, true),
    prop!(MarginBottom, "margin-bottom", "<length> | auto", false, true),
    prop!(MarginLeft, "margin-left", "<length> | auto", false, true),
    prop!(Gap, "gap", "<length>", false, true),
    prop!(FlexDirection, "flex-direction", "row | column", false, false),
    prop!(FlexGrow, "flex-grow", "<number>", false, false),
    prop!(FlexShrink, "flex-shrink", "<number>", false, false),
    prop!(
        JustifyContent,
        "justify-content",
        "start | end | center | space-between | space-around | space-evenly",
        false,
        false
    ),
    prop!(AlignItems, "align-items", "start | end | center | stretch", false, false),
    prop!(Width, "width", "<length-percentage> | auto", false, true),
    prop!(Height, "height", "<length-percentage> | auto", false, true),
    prop!(MinWidth, "min-width", "<length-percentage> | auto", false, true),
    prop!(MinHeight, "min-height", "<length-percentage> | auto", false, true),
    prop!(MaxWidth, "max-width", "<length-percentage> | none", false, true),
    prop!(MaxHeight, "max-height", "<length-percentage> | none", false, true),
    prop!(FontFamily, "font-family", "[<family-name> | sans-serif | monospace | icons]#", true, false),
    prop!(FontSize, "font-size", "<length-percentage>", true, true),
    prop!(FontWeight, "font-weight", "normal | bold | <number 100..900>", true, true),
    prop!(FontStyle, "font-style", "normal | italic", true, false),
    prop!(FontVariantNumeric, "font-variant-numeric", "normal | tabular-nums", true, false),
    prop!(LetterSpacing, "letter-spacing", "normal | <length>", true, true),
    prop!(TextAlign, "text-align", "start | center | end | left | right", true, false),
    prop!(TextOverflow, "text-overflow", "clip | ellipsis", false, false),
    prop!(Overflow, "overflow", "visible | hidden", false, false),
    prop!(BoxShadow, "box-shadow", "none | [<length>{2,4} <color>]#", false, true),
    prop!(Display, "display", "flex | none", false, false),
    prop!(Visibility, "visibility", "visible | hidden", true, false),
    prop!(Transition, "transition", "[<property> | all] <time> [<easing>] [<time>]#", false, false),
    prop!(TransitionProperty, "transition-property", "[<property> | all]#", false, false),
    prop!(TransitionDuration, "transition-duration", "<time>#", false, false),
    prop!(TransitionDelay, "transition-delay", "<time>#", false, false),
    prop!(TransitionTimingFunction, "transition-timing-function", "<easing>#", false, false),
    prop!(Cursor, "cursor", "default | pointer", true, false),
    PropertyInfo {
        id: PropertyId::Backdrop,
        name: "backdrop",
        values: "none | acrylic | mica",
        inherited: false,
        animatable: false,
        bar_only: true,
    },
];

pub fn property_by_name(name: &str) -> Option<&'static PropertyInfo> {
    PROPERTIES.iter().find(|p| p.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_names_are_unique() {
        for (i, p) in PROPERTIES.iter().enumerate() {
            assert!(PROPERTIES[i + 1..].iter().all(|q| q.name != p.name && q.id != p.id), "{}", p.name);
        }
    }
}

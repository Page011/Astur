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
//!
//! `@keyframes name { from { .. } 50% { .. } to { .. } }` with the
//! `animation` properties runs keyframe animations; only animatable
//! properties are read inside a keyframe. `@font-face` names a font file in
//! the config folder's `fonts` folder (`Stylesheet::font_faces`; the config
//! reader loads the files). Selectors take `:focus`, `:focus-visible`,
//! `:focus-within`, `:disabled`, `:checked`, `:open`, `:first-child`,
//! `:last-child`, `:nth-child()` (counting only siblings that take part in
//! layout) and, on inputs, `::placeholder` and `::selection`. `calc()` is
//! accepted only inside `polygon()`.

/// Names under which the built-in sheets are parsed: positions in these files
/// are not the user's to edit.
pub const BASE_SHEET: &str = "base.css";
pub const PRESET_SHEET: &str = "preset.css";
pub(crate) const BUILTIN_SHEETS: [&str; 2] = [BASE_SHEET, PRESET_SHEET];

use crate::diag::{Diagnostic, SourcePos};

pub(crate) mod cascade;
pub(crate) mod extra;
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
    pub(crate) keyframes: Vec<parse::Keyframes>,
    pub(crate) faces: Vec<FontFace>,
    /// Some selector uses `:first-child`, `:last-child` or `:nth-child()`, so
    /// a node's style depends on its siblings.
    pub(crate) structural: bool,
    /// Some rule styles `::placeholder` or `::selection`.
    pub(crate) pseudo_rules: bool,
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
        self.keyframes.extend(later.keyframes);
        self.faces.extend(later.faces);
        self.structural |= later.structural;
        self.pseudo_rules |= later.pseudo_rules;
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

    /// Every `@font-face` rule, in sheet order. The config reader reads
    /// each `src` (inside the config folder's `fonts` folder only) and the UI
    /// registers it with the renderer under `family`.
    pub fn font_faces(&self) -> Vec<FontFace> {
        self.faces.clone()
    }

    /// Names of the `@keyframes` rules, in sheet order.
    pub fn keyframe_names(&self) -> Vec<String> {
        self.keyframes.iter().map(|k| k.name.to_string()).collect()
    }

    /// The last `@keyframes` rule of this name (a later sheet wins).
    pub(crate) fn keyframes_named(&self, name: &str) -> Option<&parse::Keyframes> {
        self.keyframes.iter().rev().find(|k| &*k.name == name)
    }
}

/// One `@font-face` rule.
#[derive(Clone, Debug, PartialEq)]
pub struct FontFace {
    pub family: String,
    /// As written in `url(...)` with `/` separators: a file name inside the
    /// config folder's `fonts` folder, with or without a leading `fonts/`.
    pub src: String,
    /// `font-weight`: one weight, or a range for a variable font.
    pub weight: (u16, u16),
    pub italic: bool,
    pub pos: SourcePos,
}

/// Largest font file an `@font-face` may name.
pub const FONT_FILE_MAX_BYTES: u64 = 16 * 1024 * 1024;
/// Most font files a style sheet may load.
pub const FONT_FILES_MAX: usize = 16;

impl FontFace {
    /// The file's name inside the config folder's `fonts` folder.
    pub fn file_name(&self) -> &str {
        self.src.strip_prefix("fonts/").unwrap_or(&self.src)
    }

    fn warn(&self, message: String) -> Diagnostic {
        Diagnostic::warning(crate::diag::DiagKind::Style, Some(self.pos.clone()), message)
    }

    /// The file is not in the config folder's `fonts` folder.
    pub fn not_found(&self) -> Diagnostic {
        self.warn(format!("font file 'fonts/{}' was not found in the config folder's fonts folder", self.file_name()))
    }

    /// The file is bigger than `FONT_FILE_MAX_BYTES`.
    pub fn too_large(&self) -> Diagnostic {
        self.warn(format!(
            "font file 'fonts/{}' is larger than {} MiB",
            self.file_name(),
            FONT_FILE_MAX_BYTES / (1024 * 1024)
        ))
    }

    /// The file is not a font the renderer can read.
    pub fn not_a_font(&self) -> Diagnostic {
        self.warn(format!("font file 'fonts/{}' is not a font", self.file_name()))
    }

    /// The sheet names more than `FONT_FILES_MAX` files; this one is skipped.
    pub fn over_the_limit(&self) -> Diagnostic {
        self.warn(format!("more than {FONT_FILES_MAX} font files; 'fonts/{}' is not loaded", self.file_name()))
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
    CornerShape,
    ClipPath,
    Transform,
    TransformOrigin,
    Animation,
    AnimationName,
    AnimationDuration,
    AnimationTimingFunction,
    AnimationDelay,
    AnimationIterationCount,
    AnimationDirection,
    AnimationFillMode,
    AnimationPlayState,
    OverflowX,
    OverflowY,
    ScrollbarWidth,
    ScrollbarColor,
    CaretColor,
    AccentColor,
    Outline,
    OutlineOffset,
    PointerEvents,
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
    prop!(Overflow, "overflow", "visible | hidden | auto | scroll (1-2 values: x, y)", false, false),
    prop!(BoxShadow, "box-shadow", "none | [<length>{2,4} <color>]#", false, true),
    prop!(Display, "display", "flex | none", false, false),
    prop!(Visibility, "visibility", "visible | hidden", true, false),
    prop!(Transition, "transition", "[<property> | all] <time> [<easing>] [<time>]#", false, false),
    prop!(TransitionProperty, "transition-property", "[<property> | all]#", false, false),
    prop!(TransitionDuration, "transition-duration", "<time>#", false, false),
    prop!(TransitionDelay, "transition-delay", "<time>#", false, false),
    prop!(TransitionTimingFunction, "transition-timing-function", "<easing>#", false, false),
    prop!(Cursor, "cursor", "default | pointer | text", true, false),
    prop!(CornerShape, "corner-shape", "[round | bevel]{1,4}", false, false),
    prop!(
        ClipPath,
        "clip-path",
        "none | polygon([nonzero | evenodd,] <coord> <coord>#), coord = <length-percentage> | calc(<percentage> +/- <length>)",
        false,
        true
    ),
    prop!(
        Transform,
        "transform",
        "none | [translate(<lp>[, <lp>]) | translateX(<lp>) | translateY(<lp>) | scale(<n>[, <n>]) | scaleX(<n>) | scaleY(<n>)]+",
        false,
        true
    ),
    prop!(TransformOrigin, "transform-origin", "<lp> [<lp>] | left | center | right | top | bottom", false, true),
    prop!(
        Animation,
        "animation",
        "<name> <time> [<easing>] [<time>] [<count> | infinite] [<direction>] [<fill>] [<play>], ...",
        false,
        false
    ),
    prop!(AnimationName, "animation-name", "none | <ident>#", false, false),
    prop!(AnimationDuration, "animation-duration", "<time>#", false, false),
    prop!(AnimationTimingFunction, "animation-timing-function", "<easing>#", false, false),
    prop!(AnimationDelay, "animation-delay", "<time>#", false, false),
    prop!(AnimationIterationCount, "animation-iteration-count", "[<number> | infinite]#", false, false),
    prop!(
        AnimationDirection,
        "animation-direction",
        "[normal | reverse | alternate | alternate-reverse]#",
        false,
        false
    ),
    prop!(AnimationFillMode, "animation-fill-mode", "[none | forwards | backwards | both]#", false, false),
    prop!(AnimationPlayState, "animation-play-state", "[running | paused]#", false, false),
    prop!(OverflowX, "overflow-x", "visible | hidden | auto | scroll", false, false),
    prop!(OverflowY, "overflow-y", "visible | hidden | auto | scroll", false, false),
    prop!(ScrollbarWidth, "scrollbar-width", "auto | thin | none", true, false),
    prop!(ScrollbarColor, "scrollbar-color", "auto | <color> <color>", true, true),
    prop!(CaretColor, "caret-color", "auto | <color>", true, true),
    prop!(AccentColor, "accent-color", "auto | <color>", true, true),
    prop!(Outline, "outline", "<length> || solid | none || <color>", false, true),
    prop!(OutlineOffset, "outline-offset", "<length>", false, true),
    prop!(PointerEvents, "pointer-events", "auto | none", true, false),
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

//! Computed style: the result of the cascade for one node. Lengths are in
//! logical (CSS) pixels; the scene multiplies by the DPI scale.

use std::sync::Arc;
use std::time::Duration;

use crate::anim::Easing;
use crate::color::Color;
use crate::css::PropertyId;
use crate::geom::Edges;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Dim {
    #[default]
    Auto,
    Px(f32),
    /// 0..=100
    Percent(f32),
}

/// Corner radii as written (px or % of the box), resolved at layout.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RadiusDims {
    pub tl: Dim,
    pub tr: Dim,
    pub br: Dim,
    pub bl: Dim,
}

impl RadiusDims {
    pub const ZERO: RadiusDims = RadiusDims { tl: Dim::Px(0.0), tr: Dim::Px(0.0), br: Dim::Px(0.0), bl: Dim::Px(0.0) };
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FlexDirection {
    #[default]
    Row,
    Column,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Justify {
    #[default]
    Start,
    End,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    Start,
    End,
    #[default]
    Center,
    Stretch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    #[default]
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Cursor {
    #[default]
    Default,
    Pointer,
    /// The I-beam, over editable text.
    Text,
}

/// System backdrop behind the bar's transparent pixels (Windows 11 only;
/// Remote Desktop shows a flat colour instead).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Backdrop {
    #[default]
    None,
    Acrylic,
    Mica,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Background {
    None,
    Color(Color),
    Linear(LinearGradient),
}

#[derive(Clone, Debug, PartialEq)]
pub struct LinearGradient {
    /// CSS angle: 0 = to top, 90 = to right, 180 = to bottom (the default).
    pub angle_deg: f32,
    /// Stops with positions 0..=1, filled in where the CSS left them out.
    pub stops: Vec<(f32, Color)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Color,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TransitionProperty {
    All,
    One(PropertyId),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransitionSpec {
    pub property: TransitionProperty,
    pub duration: Duration,
    pub delay: Duration,
    pub easing: Easing,
}

/// How a corner is cut: round (the usual arc) or bevel (a straight line
/// between the two points the radius marks on the edges).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CornerShape {
    #[default]
    Round,
    Bevel,
}

/// A length made of a share of the box and px: `50%`, `13px`,
/// `calc(100% - 13px)`. Both parts interpolate, so any two lengths animate.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LengthPct {
    /// Logical px.
    pub px: f32,
    /// 0..=100 of the box side the length runs along.
    pub pct: f32,
}

impl LengthPct {
    pub const ZERO: LengthPct = LengthPct { px: 0.0, pct: 0.0 };

    /// Physical px along a side `side_px` long.
    pub fn resolve(self, side_px: f32, scale: f32) -> f32 {
        self.px * scale + self.pct / 100.0 * side_px
    }
}

/// `clip-path: polygon(...)`: points in the border box.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipPolygon {
    pub points: Vec<(LengthPct, LengthPct)>,
    /// `evenodd` fill rule written (`nonzero` otherwise).
    pub evenodd: bool,
}

/// `translate(tx, ty) scale(sx, sy)` about `transform-origin`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform2 {
    pub tx: LengthPct,
    pub ty: LengthPct,
    pub sx: f32,
    pub sy: f32,
}

impl Transform2 {
    pub const IDENTITY: Transform2 = Transform2 { tx: LengthPct::ZERO, ty: LengthPct::ZERO, sx: 1.0, sy: 1.0 };

    pub fn is_identity(&self) -> bool {
        *self == Transform2::IDENTITY
    }
}

impl Default for Transform2 {
    fn default() -> Self {
        Transform2::IDENTITY
    }
}

/// `transform-origin`: the point the transform is about, in the border box.
pub const ORIGIN_CENTER: (LengthPct, LengthPct) = (LengthPct { px: 0.0, pct: 50.0 }, LengthPct { px: 0.0, pct: 50.0 });

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Overflow {
    #[default]
    Visible,
    Hidden,
    Auto,
    Scroll,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollbarWidth {
    #[default]
    Auto,
    Thin,
    None,
}

/// `outline`: drawn outside the border box, following its shape. Width 0 =
/// none.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outline {
    /// Logical px.
    pub width: f32,
    pub color: Color,
    /// Logical px between the border box and the outline (may be negative).
    pub offset: f32,
}

impl Default for Outline {
    fn default() -> Self {
        Outline { width: 0.0, color: Color::WHITE, offset: 0.0 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnimDirection {
    #[default]
    Normal,
    Reverse,
    Alternate,
    AlternateReverse,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnimFill {
    #[default]
    None,
    Forwards,
    Backwards,
    Both,
}

/// One entry of `animation`.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationSpec {
    pub name: Arc<str>,
    pub duration: Duration,
    /// Seconds; negative starts the animation part way through.
    pub delay_s: f64,
    /// Between keyframes that give none of their own.
    pub easing: Easing,
    /// `f64::INFINITY` for `infinite`.
    pub iterations: f64,
    pub direction: AnimDirection,
    pub fill: AnimFill,
    pub paused: bool,
    /// The keyframes with their styles worked out; None when the style sheet
    /// has no `@keyframes` of this name.
    pub frames: Option<Arc<KeyframeSet>>,
}

/// An `@keyframes` rule worked out for one element: the element's own style
/// with each keyframe's declarations on top.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyframeSet {
    /// Sorted by offset, 0 and 1 always present.
    pub frames: Vec<KeyframeStyle>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyframeStyle {
    /// 0..=1.
    pub offset: f32,
    /// `animation-timing-function` of this keyframe: the curve towards the next.
    pub easing: Option<Easing>,
    pub style: Arc<ComputedStyle>,
}

/// What a `::placeholder` or `::selection` rule sets.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PseudoStyle {
    pub color: Option<Color>,
    pub background: Option<Color>,
    pub opacity: Option<f32>,
    pub italic: Option<bool>,
}

impl PseudoStyle {
    pub fn is_empty(&self) -> bool {
        *self == PseudoStyle::default()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComputedStyle {
    pub display_none: bool,
    pub visible: bool,
    pub flex_direction: FlexDirection,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub justify_content: Justify,
    pub align_items: Align,
    pub gap: f32,
    pub width: Dim,
    pub height: Dim,
    pub min_width: Dim,
    pub min_height: Dim,
    /// `Auto` here means `none`.
    pub max_width: Dim,
    pub max_height: Dim,
    pub padding: Edges<f32>,
    /// `None` = `auto`.
    pub margin: Edges<Option<f32>>,
    pub color: Color,
    pub background: Background,
    pub opacity: f32,
    pub border_width: Edges<f32>,
    pub border_color: Color,
    /// As written; the scene resolves `%` against the border box at layout.
    pub border_radius: RadiusDims,
    pub box_shadow: Vec<Shadow>,
    /// Family names in order; generic names already mapped
    /// (`sans-serif` -> Segoe UI, `monospace` -> Cascadia Mono then Consolas,
    /// `icons` -> the icon font).
    pub font_family: Arc<[Arc<str>]>,
    pub font_size: f32,
    pub font_weight: u16,
    pub italic: bool,
    /// Logical px.
    pub letter_spacing: f32,
    pub tabular_nums: bool,
    pub text_align: TextAlign,
    pub ellipsis: bool,
    pub clip: bool,
    pub transitions: Vec<TransitionSpec>,
    pub cursor: Cursor,
    pub backdrop: Backdrop,
    /// Top left, top right, bottom right, bottom left.
    pub corner_shape: [CornerShape; 4],
    pub clip_path: Option<ClipPolygon>,
    pub transform: Transform2,
    pub transform_origin: (LengthPct, LengthPct),
    pub animations: Vec<AnimationSpec>,
    pub overflow_x: Overflow,
    pub overflow_y: Overflow,
    pub scrollbar_width: ScrollbarWidth,
    /// Thumb and track; None = `auto`.
    pub scrollbar_color: Option<(Color, Color)>,
    /// None = `auto` (the text colour).
    pub caret_color: Option<Color>,
    /// None = `auto` (the theme's own).
    pub accent_color: Option<Color>,
    pub outline: Outline,
    /// False = `pointer-events: none`: the element and its subtree are
    /// skipped by hit testing.
    pub pointer_events: bool,
    /// `::placeholder` and `::selection` rules (inputs only).
    pub placeholder: PseudoStyle,
    pub selection: PseudoStyle,
}

impl Default for ComputedStyle {
    /// Initial values: transparent, opacity 1, Segoe UI 13 px weight 400,
    /// colour white, row, align-items center, everything else zero / auto.
    fn default() -> Self {
        ComputedStyle {
            display_none: false,
            visible: true,
            flex_direction: FlexDirection::Row,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            justify_content: Justify::Start,
            align_items: Align::Center,
            gap: 0.0,
            width: Dim::Auto,
            height: Dim::Auto,
            min_width: Dim::Auto,
            min_height: Dim::Auto,
            max_width: Dim::Auto,
            max_height: Dim::Auto,
            padding: Edges::all(0.0),
            margin: Edges::all(Some(0.0)),
            color: Color::WHITE,
            background: Background::None,
            opacity: 1.0,
            border_width: Edges::all(0.0),
            border_color: Color::TRANSPARENT,
            border_radius: RadiusDims::ZERO,
            box_shadow: Vec::new(),
            font_family: Arc::from(vec![Arc::<str>::from("Segoe UI")]),
            font_size: 13.0,
            font_weight: 400,
            italic: false,
            letter_spacing: 0.0,
            tabular_nums: false,
            text_align: TextAlign::Start,
            ellipsis: false,
            clip: false,
            transitions: Vec::new(),
            cursor: Cursor::Default,
            backdrop: Backdrop::None,
            corner_shape: [CornerShape::Round; 4],
            clip_path: None,
            transform: Transform2::IDENTITY,
            transform_origin: ORIGIN_CENTER,
            animations: Vec::new(),
            overflow_x: Overflow::Visible,
            overflow_y: Overflow::Visible,
            scrollbar_width: ScrollbarWidth::Auto,
            scrollbar_color: None,
            caret_color: None,
            accent_color: None,
            outline: Outline::default(),
            pointer_events: true,
            placeholder: PseudoStyle::default(),
            selection: PseudoStyle::default(),
        }
    }
}

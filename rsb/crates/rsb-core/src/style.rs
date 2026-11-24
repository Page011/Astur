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
        }
    }
}

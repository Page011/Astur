//! A computed style written back as CSS text, for `rsb inspect --style`.
//!
//! Every value is written in a form the style sheet parser reads back to the
//! same style, so what the inspector prints can be pasted into a rule.
//! Lengths are logical px, colours are `#rrggbb` (`#rrggbbaa` when not
//! opaque), numbers are rounded to three decimals.

use std::time::Duration;

use crate::anim::Easing;
use crate::color::Color;
use crate::css::{PROPERTIES, PropertyId, property_by_name};
use crate::geom::Edges;
use crate::style::{
    Align, AnimDirection, AnimFill, Backdrop, Background, ComputedStyle, CornerShape, Cursor, Dim, FlexDirection,
    Justify, LengthPct, Overflow, PseudoStyle, RadiusDims, ScrollbarWidth, TextAlign, TransitionProperty,
};

/// Longest value written, in characters; a longer one is cut with `...`.
pub const MAX_VALUE_CHARS: usize = 120;

/// The properties `rsb inspect --style` looks at, in the order it lists
/// them. Shorthands stand for their longhands, so no value is shown twice.
pub const SHOWN: &[&str] = &[
    "display",
    "visibility",
    "flex-direction",
    "flex-grow",
    "flex-shrink",
    "justify-content",
    "align-items",
    "gap",
    "width",
    "height",
    "min-width",
    "min-height",
    "max-width",
    "max-height",
    "padding",
    "margin",
    "color",
    "background",
    "opacity",
    "border-width",
    "border-color",
    "border-radius",
    "corner-shape",
    "box-shadow",
    "outline",
    "outline-offset",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "font-variant-numeric",
    "letter-spacing",
    "text-align",
    "text-overflow",
    "overflow",
    "clip-path",
    "transform",
    "transform-origin",
    "transition",
    "animation",
    "cursor",
    "pointer-events",
    "scrollbar-width",
    "scrollbar-color",
    "caret-color",
    "accent-color",
    "backdrop",
    "::placeholder",
    "::selection",
];

/// The value of `property` (CSS name, `font-size`) in `style` as CSS text,
/// or None for a name the style does not have. `border` and its sides have
/// no text when the four borders differ (use `border-width` and
/// `border-color`). A value longer than `MAX_VALUE_CHARS` ends in `...`.
pub fn css_text(property: &str, style: &ComputedStyle) -> Option<String> {
    value(property, style).map(cut)
}

/// What differs, as `(property, value)` in `SHOWN` order: a property that is
/// not inherited when it differs from `base` (the initial style), an
/// inherited one only when it differs from `parent`, so a value a node
/// merely received is not repeated on every descendant.
pub fn differing(
    style: &ComputedStyle,
    base: &ComputedStyle,
    parent: Option<&ComputedStyle>,
) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for name in SHOWN {
        let inherited = property_by_name(name).is_some_and(|p| p.inherited);
        let against = if inherited { parent.unwrap_or(base) } else { base };
        let (Some(now), Some(was)) = (value(name, style), value(name, against)) else { continue };
        if now != was {
            out.push((*name, cut(now)));
        }
    }
    out
}

fn cut(text: String) -> String {
    if text.chars().count() <= MAX_VALUE_CHARS {
        return text;
    }
    let mut out: String = text.chars().take(MAX_VALUE_CHARS).collect();
    out.push_str("...");
    out
}

fn num(v: f32) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    if r == 0.0 { "0".to_string() } else { format!("{r}") }
}

fn num64(v: f64) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    if r == 0.0 { "0".to_string() } else { format!("{r}") }
}

fn px(v: f32) -> String {
    format!("{}px", num(v))
}

fn dim(d: Dim, none: bool) -> String {
    match d {
        Dim::Auto if none => "none".into(),
        Dim::Auto => "auto".into(),
        Dim::Px(v) => px(v),
        Dim::Percent(v) => format!("{}%", num(v)),
    }
}

fn lp(v: LengthPct) -> String {
    match (v.px == 0.0, v.pct == 0.0) {
        (true, true) => "0px".into(),
        (_, true) => px(v.px),
        (true, false) => format!("{}%", num(v.pct)),
        (false, false) => {
            let (sign, rest) = if v.px < 0.0 { ("-", -v.px) } else { ("+", v.px) };
            format!("calc({}% {sign} {})", num(v.pct), px(rest))
        }
    }
}

/// `#rrggbb`, or `#rrggbbaa` when the colour is not opaque.
pub fn color_hex(c: Color) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    if (c.a - 1.0).abs() < 0.0005 {
        format!("#{:02x}{:02x}{:02x}", b(c.r), b(c.g), b(c.b))
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", b(c.r), b(c.g), b(c.b), b(c.a))
    }
}

/// The shortest of the one to four value forms of a box property.
fn sides(e: [String; 4]) -> String {
    let [t, r, b, l] = e;
    if t == r && r == b && b == l {
        t
    } else if t == b && r == l {
        format!("{t} {r}")
    } else if r == l {
        format!("{t} {r} {b}")
    } else {
        format!("{t} {r} {b} {l}")
    }
}

fn edges_px(e: Edges<f32>) -> String {
    sides([px(e.top), px(e.right), px(e.bottom), px(e.left)])
}

fn ms(d: Duration) -> String {
    format!("{}ms", num64(d.as_secs_f64() * 1000.0))
}

fn easing(e: Easing) -> String {
    match e {
        Easing::Linear => "linear".into(),
        Easing::Ease => "ease".into(),
        Easing::EaseIn => "ease-in".into(),
        Easing::EaseOut => "ease-out".into(),
        Easing::EaseInOut => "ease-in-out".into(),
        Easing::CubicBezier(a, b, c, d) => format!("cubic-bezier({}, {}, {}, {})", num(a), num(b), num(c), num(d)),
        Easing::Steps(n, start) => format!("steps({n}, {})", if start { "start" } else { "end" }),
    }
}

fn overflow(o: Overflow) -> &'static str {
    match o {
        Overflow::Visible => "visible",
        Overflow::Hidden => "hidden",
        Overflow::Auto => "auto",
        Overflow::Scroll => "scroll",
    }
}

fn family(name: &str) -> String {
    let plain = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    // A name that is a keyword of the language would be read as the generic.
    let generic = matches!(name.to_ascii_lowercase().as_str(), "sans-serif" | "system-ui" | "monospace" | "icons");
    if plain && !generic { name.to_string() } else { format!("\"{}\"", name.replace(['"', '\\'], "")) }
}

fn property_name(id: PropertyId) -> &'static str {
    PROPERTIES.iter().find(|p| p.id == id).map_or("all", |p| p.name)
}

fn list<T>(items: &[T], f: impl Fn(&T) -> String) -> String {
    items.iter().map(f).collect::<Vec<_>>().join(", ")
}

fn border_side(w: f32, color: Color) -> String {
    if w == 0.0 { "none".into() } else { format!("{} solid {}", px(w), color_hex(color)) }
}

fn radius(r: &RadiusDims) -> String {
    sides([dim(r.tl, false), dim(r.tr, false), dim(r.br, false), dim(r.bl, false)])
}

fn pseudo(p: &PseudoStyle) -> String {
    let mut parts = Vec::new();
    if let Some(c) = p.color {
        parts.push(format!("color: {}", color_hex(c)));
    }
    if let Some(c) = p.background {
        parts.push(format!("background: {}", color_hex(c)));
    }
    if let Some(o) = p.opacity {
        parts.push(format!("opacity: {}", num(o)));
    }
    if let Some(i) = p.italic {
        parts.push(format!("font-style: {}", if i { "italic" } else { "normal" }));
    }
    if parts.is_empty() { "none".into() } else { parts.join("; ") }
}

fn value(property: &str, s: &ComputedStyle) -> Option<String> {
    use PropertyId as P;
    if let Some(rest) = property.strip_prefix("::") {
        return match rest {
            "placeholder" => Some(pseudo(&s.placeholder)),
            "selection" => Some(pseudo(&s.selection)),
            _ => None,
        };
    }
    let id = property_by_name(property)?.id;
    Some(match id {
        P::Color => color_hex(s.color),
        P::Background => match &s.background {
            Background::None => "none".into(),
            Background::Color(c) => color_hex(*c),
            Background::Linear(g) => {
                let stops = list(&g.stops, |(at, c)| format!("{} {}%", color_hex(*c), num(at * 100.0)));
                format!("linear-gradient({}deg, {stops})", num(g.angle_deg))
            }
        },
        P::BackgroundColor => match &s.background {
            Background::Color(c) => color_hex(*c),
            _ => "transparent".into(),
        },
        P::Opacity => num(s.opacity),
        P::Border => {
            let b = s.border_width;
            if b.top != b.right || b.right != b.bottom || b.bottom != b.left {
                return None;
            }
            border_side(b.top, s.border_color)
        }
        P::BorderTop => border_side(s.border_width.top, s.border_color),
        P::BorderRight => border_side(s.border_width.right, s.border_color),
        P::BorderBottom => border_side(s.border_width.bottom, s.border_color),
        P::BorderLeft => border_side(s.border_width.left, s.border_color),
        P::BorderWidth => edges_px(s.border_width),
        P::BorderColor => color_hex(s.border_color),
        P::BorderRadius => radius(&s.border_radius),
        P::BorderTopLeftRadius => dim(s.border_radius.tl, false),
        P::BorderTopRightRadius => dim(s.border_radius.tr, false),
        P::BorderBottomRightRadius => dim(s.border_radius.br, false),
        P::BorderBottomLeftRadius => dim(s.border_radius.bl, false),
        P::Padding => edges_px(s.padding),
        P::PaddingTop => px(s.padding.top),
        P::PaddingRight => px(s.padding.right),
        P::PaddingBottom => px(s.padding.bottom),
        P::PaddingLeft => px(s.padding.left),
        P::Margin => {
            let m = |v: Option<f32>| v.map_or_else(|| "auto".to_string(), px);
            sides([m(s.margin.top), m(s.margin.right), m(s.margin.bottom), m(s.margin.left)])
        }
        P::MarginTop => s.margin.top.map_or_else(|| "auto".into(), px),
        P::MarginRight => s.margin.right.map_or_else(|| "auto".into(), px),
        P::MarginBottom => s.margin.bottom.map_or_else(|| "auto".into(), px),
        P::MarginLeft => s.margin.left.map_or_else(|| "auto".into(), px),
        P::Gap => px(s.gap),
        P::FlexDirection => match s.flex_direction {
            FlexDirection::Row => "row",
            FlexDirection::Column => "column",
        }
        .into(),
        P::FlexGrow => num(s.flex_grow),
        P::FlexShrink => num(s.flex_shrink),
        P::JustifyContent => match s.justify_content {
            Justify::Start => "start",
            Justify::End => "end",
            Justify::Center => "center",
            Justify::SpaceBetween => "space-between",
            Justify::SpaceAround => "space-around",
            Justify::SpaceEvenly => "space-evenly",
        }
        .into(),
        P::AlignItems => match s.align_items {
            Align::Start => "start",
            Align::End => "end",
            Align::Center => "center",
            Align::Stretch => "stretch",
        }
        .into(),
        P::Width => dim(s.width, false),
        P::Height => dim(s.height, false),
        P::MinWidth => dim(s.min_width, false),
        P::MinHeight => dim(s.min_height, false),
        P::MaxWidth => dim(s.max_width, true),
        P::MaxHeight => dim(s.max_height, true),
        P::FontFamily => {
            // A stack that names a family twice shows it once, where it first appears.
            let mut first: Vec<&std::sync::Arc<str>> = Vec::new();
            for name in s.font_family.iter() {
                if !first.contains(&name) {
                    first.push(name);
                }
            }
            list(&first, |n| family(n))
        }
        P::FontSize => px(s.font_size),
        P::FontWeight => s.font_weight.to_string(),
        P::FontStyle => if s.italic { "italic" } else { "normal" }.into(),
        P::FontVariantNumeric => if s.tabular_nums { "tabular-nums" } else { "normal" }.into(),
        P::LetterSpacing => {
            if s.letter_spacing == 0.0 {
                "normal".into()
            } else {
                px(s.letter_spacing)
            }
        }
        P::TextAlign => match s.text_align {
            TextAlign::Start => "start",
            TextAlign::Center => "center",
            TextAlign::End => "end",
        }
        .into(),
        P::TextOverflow => if s.ellipsis { "ellipsis" } else { "clip" }.into(),
        P::Overflow => {
            if s.overflow_x == s.overflow_y {
                overflow(s.overflow_x).into()
            } else {
                format!("{} {}", overflow(s.overflow_x), overflow(s.overflow_y))
            }
        }
        P::OverflowX => overflow(s.overflow_x).into(),
        P::OverflowY => overflow(s.overflow_y).into(),
        P::BoxShadow => {
            if s.box_shadow.is_empty() {
                "none".into()
            } else {
                list(&s.box_shadow, |h| {
                    format!(
                        "{} {} {} {} {}",
                        px(h.offset_x),
                        px(h.offset_y),
                        px(h.blur),
                        px(h.spread),
                        color_hex(h.color)
                    )
                })
            }
        }
        P::Display => if s.display_none { "none" } else { "flex" }.into(),
        P::Visibility => if s.visible { "visible" } else { "hidden" }.into(),
        P::Transition => {
            if s.transitions.is_empty() {
                "none".into()
            } else {
                list(&s.transitions, |t| {
                    let prop = match t.property {
                        TransitionProperty::All => "all",
                        TransitionProperty::One(id) => property_name(id),
                    };
                    format!("{prop} {} {} {}", ms(t.duration), easing(t.easing), ms(t.delay))
                })
            }
        }
        P::TransitionProperty => {
            if s.transitions.is_empty() {
                "none".into()
            } else {
                list(&s.transitions, |t| {
                    match t.property {
                        TransitionProperty::All => "all",
                        TransitionProperty::One(id) => property_name(id),
                    }
                    .to_string()
                })
            }
        }
        P::TransitionDuration => or_zero(s.transitions.iter().map(|t| ms(t.duration))),
        P::TransitionDelay => or_zero(s.transitions.iter().map(|t| ms(t.delay))),
        P::TransitionTimingFunction => or_ease(s.transitions.iter().map(|t| easing(t.easing))),
        P::Cursor => match s.cursor {
            Cursor::Default => "default",
            Cursor::Pointer => "pointer",
            Cursor::Text => "text",
        }
        .into(),
        P::Backdrop => match s.backdrop {
            Backdrop::None => "none",
            Backdrop::Acrylic => "acrylic",
            Backdrop::Mica => "mica",
        }
        .into(),
        P::CornerShape => {
            let c = |v: CornerShape| if v == CornerShape::Bevel { "bevel" } else { "round" }.to_string();
            sides([c(s.corner_shape[0]), c(s.corner_shape[1]), c(s.corner_shape[2]), c(s.corner_shape[3])])
        }
        P::ClipPath => match &s.clip_path {
            None => "none".into(),
            Some(p) => {
                let pts = list(&p.points, |(x, y)| format!("{} {}", lp(*x), lp(*y)));
                format!("polygon({}{pts})", if p.evenodd { "evenodd, " } else { "" })
            }
        },
        P::Transform => {
            let t = &s.transform;
            if t.is_identity() {
                "none".into()
            } else {
                let mut parts = Vec::new();
                if t.tx != LengthPct::ZERO || t.ty != LengthPct::ZERO {
                    parts.push(format!("translate({}, {})", lp(t.tx), lp(t.ty)));
                }
                if t.sx != 1.0 || t.sy != 1.0 {
                    parts.push(format!("scale({}, {})", num(t.sx), num(t.sy)));
                }
                parts.join(" ")
            }
        }
        P::TransformOrigin => format!("{} {}", lp(s.transform_origin.0), lp(s.transform_origin.1)),
        P::Animation => {
            if s.animations.is_empty() {
                "none".into()
            } else {
                list(&s.animations, |a| {
                    // The defaults are left out: a written `none` would be read as the animation's name.
                    let mut parts = vec![
                        a.name.to_string(),
                        ms(a.duration),
                        easing(a.easing),
                        format!("{}ms", num64(a.delay_s * 1000.0)),
                        iterations(a.iterations),
                    ];
                    if a.direction != AnimDirection::Normal {
                        parts.push(direction(a.direction).to_string());
                    }
                    if a.fill != AnimFill::None {
                        parts.push(fill(a.fill).to_string());
                    }
                    if a.paused {
                        parts.push("paused".to_string());
                    }
                    parts.join(" ")
                })
            }
        }
        P::AnimationName => {
            if s.animations.is_empty() {
                "none".into()
            } else {
                list(&s.animations, |a| a.name.to_string())
            }
        }
        P::AnimationDuration => or_zero(s.animations.iter().map(|a| ms(a.duration))),
        P::AnimationTimingFunction => or_ease(s.animations.iter().map(|a| easing(a.easing))),
        P::AnimationDelay => or_zero(s.animations.iter().map(|a| format!("{}ms", num64(a.delay_s * 1000.0)))),
        P::AnimationIterationCount => or_one(s.animations.iter().map(|a| iterations(a.iterations))),
        P::AnimationDirection => or_default(s.animations.iter().map(|a| direction(a.direction).to_string()), "normal"),
        P::AnimationFillMode => or_default(s.animations.iter().map(|a| fill(a.fill).to_string()), "none"),
        P::AnimationPlayState => {
            or_default(s.animations.iter().map(|a| if a.paused { "paused" } else { "running" }.to_string()), "running")
        }
        P::ScrollbarWidth => match s.scrollbar_width {
            ScrollbarWidth::Auto => "auto",
            ScrollbarWidth::Thin => "thin",
            ScrollbarWidth::None => "none",
        }
        .into(),
        P::ScrollbarColor => match s.scrollbar_color {
            None => "auto".into(),
            Some((thumb, track)) => format!("{} {}", color_hex(thumb), color_hex(track)),
        },
        P::CaretColor => s.caret_color.map_or_else(|| "auto".into(), color_hex),
        P::AccentColor => s.accent_color.map_or_else(|| "auto".into(), color_hex),
        P::Outline => {
            if s.outline.width == 0.0 {
                "none".into()
            } else {
                format!("{} solid {}", px(s.outline.width), color_hex(s.outline.color))
            }
        }
        P::OutlineOffset => px(s.outline.offset),
        P::PointerEvents => if s.pointer_events { "auto" } else { "none" }.into(),
    })
}

fn or_zero(items: impl Iterator<Item = String>) -> String {
    or_default(items, "0ms")
}

fn or_ease(items: impl Iterator<Item = String>) -> String {
    or_default(items, "ease")
}

fn or_one(items: impl Iterator<Item = String>) -> String {
    or_default(items, "1")
}

fn or_default(items: impl Iterator<Item = String>, none: &str) -> String {
    let v: Vec<String> = items.collect();
    if v.is_empty() { none.to_string() } else { v.join(", ") }
}

fn iterations(n: f64) -> String {
    if n.is_infinite() { "infinite".into() } else { num64(n) }
}

fn direction(d: AnimDirection) -> &'static str {
    match d {
        AnimDirection::Normal => "normal",
        AnimDirection::Reverse => "reverse",
        AnimDirection::Alternate => "alternate",
        AnimDirection::AlternateReverse => "alternate-reverse",
    }
}

fn fill(f: AnimFill) -> &'static str {
    match f {
        AnimFill::None => "none",
        AnimFill::Forwards => "forwards",
        AnimFill::Backwards => "backwards",
        AnimFill::Both => "both",
    }
}

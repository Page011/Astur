//! Transitions: which style values animate, how a change starts one, and the
//! value of a running one at a given time.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::anim::{AnimValue, Easing, interpolate};
use crate::css::PropertyId;
use crate::geom::Edges;
use crate::style::{
    Background, ClipPolygon, ComputedStyle, Dim, LengthPct, RadiusDims, Shadow, Transform2, TransitionProperty,
};

/// How often frames are asked for while something animates (about 60 Hz).
pub(super) const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// The style values a transition can run on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    Color,
    Background,
    Opacity,
    BorderColor,
    BorderWidth,
    BorderRadius,
    Padding,
    Margin,
    Gap,
    Width,
    Height,
    MinWidth,
    MinHeight,
    MaxWidth,
    MaxHeight,
    FontSize,
    FontWeight,
    LetterSpacing,
    BoxShadow,
    Transform,
    TransformOrigin,
    ClipPath,
    OutlineWidth,
    OutlineColor,
    OutlineOffset,
    CaretColor,
    AccentColor,
    ScrollbarColor,
}

/// Every slot an animation can move.
pub(super) const ALL: [Slot; 28] = [
    Slot::Color,
    Slot::Background,
    Slot::Opacity,
    Slot::BorderColor,
    Slot::BorderWidth,
    Slot::BorderRadius,
    Slot::Padding,
    Slot::Margin,
    Slot::Gap,
    Slot::Width,
    Slot::Height,
    Slot::MinWidth,
    Slot::MinHeight,
    Slot::MaxWidth,
    Slot::MaxHeight,
    Slot::FontSize,
    Slot::FontWeight,
    Slot::LetterSpacing,
    Slot::BoxShadow,
    Slot::Transform,
    Slot::TransformOrigin,
    Slot::ClipPath,
    Slot::OutlineWidth,
    Slot::OutlineColor,
    Slot::OutlineOffset,
    Slot::CaretColor,
    Slot::AccentColor,
    Slot::ScrollbarColor,
];

#[derive(Clone, Debug)]
pub(super) struct Running {
    slot: Slot,
    from: AnimValue,
    to: AnimValue,
    start: Instant,
    delay: Duration,
    duration: Duration,
    easing: Easing,
}

/// Whether a `transition-property` entry names this slot (a shorthand covers
/// its longhands, `background` covers `background-color`).
fn covers(slot: Slot, prop: TransitionProperty) -> bool {
    use PropertyId as P;
    let id = match prop {
        TransitionProperty::All => return true,
        TransitionProperty::One(id) => id,
    };
    match slot {
        Slot::Color => id == P::Color,
        Slot::Background => matches!(id, P::Background | P::BackgroundColor),
        Slot::Opacity => id == P::Opacity,
        Slot::BorderColor => {
            matches!(id, P::BorderColor | P::Border | P::BorderTop | P::BorderRight | P::BorderBottom | P::BorderLeft)
        }
        Slot::BorderWidth => {
            matches!(id, P::BorderWidth | P::Border | P::BorderTop | P::BorderRight | P::BorderBottom | P::BorderLeft)
        }
        Slot::BorderRadius => matches!(
            id,
            P::BorderRadius
                | P::BorderTopLeftRadius
                | P::BorderTopRightRadius
                | P::BorderBottomRightRadius
                | P::BorderBottomLeftRadius
        ),
        Slot::Padding => matches!(id, P::Padding | P::PaddingTop | P::PaddingRight | P::PaddingBottom | P::PaddingLeft),
        Slot::Margin => matches!(id, P::Margin | P::MarginTop | P::MarginRight | P::MarginBottom | P::MarginLeft),
        Slot::Gap => id == P::Gap,
        Slot::Width => id == P::Width,
        Slot::Height => id == P::Height,
        Slot::MinWidth => id == P::MinWidth,
        Slot::MinHeight => id == P::MinHeight,
        Slot::MaxWidth => id == P::MaxWidth,
        Slot::MaxHeight => id == P::MaxHeight,
        Slot::FontSize => id == P::FontSize,
        Slot::FontWeight => id == P::FontWeight,
        Slot::LetterSpacing => id == P::LetterSpacing,
        Slot::BoxShadow => id == P::BoxShadow,
        Slot::Transform => id == P::Transform,
        Slot::TransformOrigin => id == P::TransformOrigin,
        Slot::ClipPath => id == P::ClipPath,
        Slot::OutlineWidth | Slot::OutlineColor => id == P::Outline,
        Slot::OutlineOffset => id == P::OutlineOffset,
        Slot::CaretColor => id == P::CaretColor,
        Slot::AccentColor => id == P::AccentColor,
        Slot::ScrollbarColor => id == P::ScrollbarColor,
    }
}

fn lp_list(v: &[LengthPct]) -> AnimValue {
    AnimValue::List(v.iter().flat_map(|l| [AnimValue::Num(l.px), AnimValue::Num(l.pct)]).collect())
}

fn unlp_list(v: &AnimValue) -> Option<Vec<LengthPct>> {
    let AnimValue::List(l) = v else { return None };
    let mut out = Vec::new();
    for pair in l.chunks(2) {
        match pair {
            [AnimValue::Num(px), AnimValue::Num(pct)] => out.push(LengthPct { px: *px, pct: *pct }),
            _ => return None,
        }
    }
    Some(out)
}

fn num4(e: [f32; 4]) -> AnimValue {
    AnimValue::List(e.iter().map(|v| AnimValue::Num(*v)).collect())
}

fn unnum4(v: &AnimValue) -> Option<[f32; 4]> {
    match v {
        AnimValue::List(l) if l.len() == 4 => {
            let mut out = [0.0; 4];
            for (o, x) in out.iter_mut().zip(l) {
                match x {
                    AnimValue::Num(n) => *o = *n,
                    _ => return None,
                }
            }
            Some(out)
        }
        _ => None,
    }
}

fn dim_px(d: Dim) -> Option<AnimValue> {
    match d {
        Dim::Px(v) => Some(AnimValue::Num(v)),
        _ => None,
    }
}

fn set_dim(d: &mut Dim, v: &AnimValue) {
    if let AnimValue::Num(n) = v {
        *d = Dim::Px(*n);
    }
}

/// The animatable value of `slot`, None when it cannot be interpolated in the
/// style's current state (a gradient, `auto`, a percentage).
pub(super) fn get(s: &ComputedStyle, slot: Slot) -> Option<AnimValue> {
    Some(match slot {
        Slot::Color => AnimValue::Color(s.color),
        Slot::Background => match &s.background {
            Background::Color(c) => AnimValue::Color(*c),
            Background::None => AnimValue::Color(crate::color::Color::TRANSPARENT),
            Background::Linear(_) => return None,
        },
        Slot::Opacity => AnimValue::Num(s.opacity),
        Slot::BorderColor => AnimValue::Color(s.border_color),
        Slot::BorderWidth => {
            let w = s.border_width;
            num4([w.top, w.right, w.bottom, w.left])
        }
        Slot::BorderRadius => {
            let r = s.border_radius;
            let px = |d: Dim| if let Dim::Px(v) = d { Some(v) } else { None };
            num4([px(r.tl)?, px(r.tr)?, px(r.br)?, px(r.bl)?])
        }
        Slot::Padding => {
            let p = s.padding;
            num4([p.top, p.right, p.bottom, p.left])
        }
        Slot::Margin => {
            let m = s.margin;
            num4([m.top?, m.right?, m.bottom?, m.left?])
        }
        Slot::Gap => AnimValue::Num(s.gap),
        Slot::Width => dim_px(s.width)?,
        Slot::Height => dim_px(s.height)?,
        Slot::MinWidth => dim_px(s.min_width)?,
        Slot::MinHeight => dim_px(s.min_height)?,
        Slot::MaxWidth => dim_px(s.max_width)?,
        Slot::MaxHeight => dim_px(s.max_height)?,
        Slot::FontSize => AnimValue::Num(s.font_size),
        Slot::FontWeight => AnimValue::Num(f32::from(s.font_weight)),
        Slot::LetterSpacing => AnimValue::Num(s.letter_spacing),
        Slot::BoxShadow => AnimValue::List(
            s.box_shadow
                .iter()
                .map(|x| {
                    AnimValue::List(vec![
                        AnimValue::Num(x.offset_x),
                        AnimValue::Num(x.offset_y),
                        AnimValue::Num(x.blur),
                        AnimValue::Num(x.spread),
                        AnimValue::Color(x.color),
                    ])
                })
                .collect(),
        ),
        Slot::Transform => {
            let t = s.transform;
            let mut l = match lp_list(&[t.tx, t.ty]) {
                AnimValue::List(l) => l,
                _ => Vec::new(),
            };
            l.push(AnimValue::Num(t.sx));
            l.push(AnimValue::Num(t.sy));
            AnimValue::List(l)
        }
        Slot::TransformOrigin => lp_list(&[s.transform_origin.0, s.transform_origin.1]),
        Slot::ClipPath => {
            let c = s.clip_path.as_ref()?;
            let mut l = vec![AnimValue::Num(f32::from(u8::from(c.evenodd)))];
            for (x, y) in &c.points {
                l.push(lp_list(&[*x, *y]));
            }
            AnimValue::List(l)
        }
        Slot::OutlineWidth => AnimValue::Num(s.outline.width),
        Slot::OutlineColor => AnimValue::Color(s.outline.color),
        Slot::OutlineOffset => AnimValue::Num(s.outline.offset),
        Slot::CaretColor => AnimValue::Color(s.caret_color?),
        Slot::AccentColor => AnimValue::Color(s.accent_color?),
        Slot::ScrollbarColor => {
            let (a, b) = s.scrollbar_color?;
            AnimValue::List(vec![AnimValue::Color(a), AnimValue::Color(b)])
        }
    })
}

pub(super) fn set(s: &mut ComputedStyle, slot: Slot, v: &AnimValue) {
    match (slot, v) {
        (Slot::Color, AnimValue::Color(c)) => s.color = *c,
        (Slot::Background, AnimValue::Color(c)) => s.background = Background::Color(*c),
        (Slot::Opacity, AnimValue::Num(n)) => s.opacity = n.clamp(0.0, 1.0),
        (Slot::BorderColor, AnimValue::Color(c)) => s.border_color = *c,
        (Slot::BorderWidth, v) => {
            if let Some([t, r, b, l]) = unnum4(v) {
                s.border_width = Edges { top: t.max(0.0), right: r.max(0.0), bottom: b.max(0.0), left: l.max(0.0) };
            }
        }
        (Slot::BorderRadius, v) => {
            if let Some([a, b, c, d]) = unnum4(v) {
                s.border_radius = RadiusDims {
                    tl: Dim::Px(a.max(0.0)),
                    tr: Dim::Px(b.max(0.0)),
                    br: Dim::Px(c.max(0.0)),
                    bl: Dim::Px(d.max(0.0)),
                };
            }
        }
        (Slot::Padding, v) => {
            if let Some([t, r, b, l]) = unnum4(v) {
                s.padding = Edges { top: t.max(0.0), right: r.max(0.0), bottom: b.max(0.0), left: l.max(0.0) };
            }
        }
        (Slot::Margin, v) => {
            if let Some([t, r, b, l]) = unnum4(v) {
                s.margin = Edges { top: Some(t), right: Some(r), bottom: Some(b), left: Some(l) };
            }
        }
        (Slot::Gap, AnimValue::Num(n)) => s.gap = n.max(0.0),
        (Slot::Width, v) => set_dim(&mut s.width, v),
        (Slot::Height, v) => set_dim(&mut s.height, v),
        (Slot::MinWidth, v) => set_dim(&mut s.min_width, v),
        (Slot::MinHeight, v) => set_dim(&mut s.min_height, v),
        (Slot::MaxWidth, v) => set_dim(&mut s.max_width, v),
        (Slot::MaxHeight, v) => set_dim(&mut s.max_height, v),
        (Slot::FontSize, AnimValue::Num(n)) => s.font_size = n.max(1.0),
        (Slot::FontWeight, AnimValue::Num(n)) => s.font_weight = n.round().clamp(1.0, 1000.0) as u16,
        (Slot::LetterSpacing, AnimValue::Num(n)) => s.letter_spacing = *n,
        (Slot::BoxShadow, AnimValue::List(items)) => {
            let mut out = Vec::new();
            for it in items {
                if let AnimValue::List(f) = it
                    && let [
                        AnimValue::Num(x),
                        AnimValue::Num(y),
                        AnimValue::Num(b),
                        AnimValue::Num(sp),
                        AnimValue::Color(c),
                    ] = f.as_slice()
                {
                    out.push(Shadow { offset_x: *x, offset_y: *y, blur: b.max(0.0), spread: *sp, color: *c });
                }
            }
            s.box_shadow = out;
        }
        (Slot::Transform, v) => {
            if let AnimValue::List(l) = v
                && let [a, b, c, d, AnimValue::Num(sx), AnimValue::Num(sy)] = l.as_slice()
                && let Some(p) = unlp_list(&AnimValue::List(vec![a.clone(), b.clone(), c.clone(), d.clone()]))
            {
                s.transform = Transform2 { tx: p[0], ty: p[1], sx: sx.max(0.0), sy: sy.max(0.0) };
            }
        }
        (Slot::TransformOrigin, v) => {
            if let Some(p) = unlp_list(v)
                && p.len() == 2
            {
                s.transform_origin = (p[0], p[1]);
            }
        }
        (Slot::ClipPath, AnimValue::List(l)) => {
            if let Some((AnimValue::Num(eo), rest)) = l.split_first() {
                let points: Option<Vec<_>> =
                    rest.iter().map(|p| unlp_list(p).filter(|v| v.len() == 2).map(|v| (v[0], v[1]))).collect();
                if let Some(points) = points {
                    s.clip_path = Some(ClipPolygon { points, evenodd: *eo >= 0.5 });
                }
            }
        }
        (Slot::OutlineWidth, AnimValue::Num(n)) => s.outline.width = n.max(0.0),
        (Slot::OutlineColor, AnimValue::Color(c)) => s.outline.color = *c,
        (Slot::OutlineOffset, AnimValue::Num(n)) => s.outline.offset = *n,
        (Slot::CaretColor, AnimValue::Color(c)) => s.caret_color = Some(*c),
        (Slot::AccentColor, AnimValue::Color(c)) => s.accent_color = Some(*c),
        (Slot::ScrollbarColor, AnimValue::List(l)) => {
            if let [AnimValue::Color(a), AnimValue::Color(b)] = l.as_slice() {
                s.scrollbar_color = Some((*a, *b));
            }
        }
        _ => {}
    }
}

/// The transition that covers `id` on a style (the last one written).
pub(super) fn transition_for(s: &ComputedStyle, id: PropertyId) -> Option<crate::style::TransitionSpec> {
    s.transitions
        .iter()
        .rev()
        .find(|t| match t.property {
            TransitionProperty::All => true,
            TransitionProperty::One(p) => p == id,
        })
        .copied()
}

/// Starts, retargets or cancels transitions for the slots that differ
/// between the old and the new target style. A new one starts from what was
/// on screen (`old_display`), so an interrupted transition continues from its
/// current value.
pub(super) fn detect(
    old_target: &ComputedStyle,
    old_display: &ComputedStyle,
    new_target: &ComputedStyle,
    runs: &mut Vec<Running>,
    start: Instant,
) {
    // Width and height are tracked by the layout (their used size, `auto`
    // included), not here.
    for slot in ALL.into_iter().filter(|s| !matches!(s, Slot::Width | Slot::Height)) {
        let (a, b) = (get(old_target, slot), get(new_target, slot));
        if a == b {
            continue;
        }
        runs.retain(|r| r.slot != slot);
        let (Some(a), Some(b)) = (a, b) else { continue };
        let Some(spec) = new_target.transitions.iter().rev().find(|t| covers(slot, t.property)) else { continue };
        let from = get(old_display, slot).unwrap_or(a);
        if from == b {
            continue;
        }
        // Lists of different lengths cannot interpolate; they would only jump.
        if let (AnimValue::List(x), AnimValue::List(y)) = (&from, &b)
            && x.len() != y.len()
        {
            continue;
        }
        runs.push(Running {
            slot,
            from,
            to: b,
            start,
            delay: spec.delay,
            duration: spec.duration,
            easing: spec.easing,
        });
    }
}

/// The style to draw at `now` and the transitions still running.
pub(super) fn evaluate(
    target: &Arc<ComputedStyle>,
    runs: Vec<Running>,
    now: Instant,
) -> (Arc<ComputedStyle>, Vec<Running>) {
    if runs.is_empty() {
        return (target.clone(), runs);
    }
    let mut display = (**target).clone();
    let mut keep = Vec::new();
    for r in runs {
        let elapsed = now.saturating_duration_since(r.start);
        if elapsed < r.delay {
            set(&mut display, r.slot, &r.from);
            keep.push(r);
            continue;
        }
        let t = if r.duration.is_zero() { 1.0 } else { (elapsed - r.delay).as_secs_f32() / r.duration.as_secs_f32() };
        if t >= 1.0 {
            continue;
        }
        let v = interpolate(&r.from, &r.to, r.easing.apply(t));
        set(&mut display, r.slot, &v);
        keep.push(r);
    }
    (Arc::new(display), keep)
}

/// True when the two styles lay out identically (colour, background, shadow
/// and the like do not count).
pub(super) fn layout_eq(a: &ComputedStyle, b: &ComputedStyle) -> bool {
    a.display_none == b.display_none
        && a.flex_direction == b.flex_direction
        && a.flex_grow == b.flex_grow
        && a.flex_shrink == b.flex_shrink
        && a.justify_content == b.justify_content
        && a.align_items == b.align_items
        && a.gap == b.gap
        && a.width == b.width
        && a.height == b.height
        && a.min_width == b.min_width
        && a.min_height == b.min_height
        && a.max_width == b.max_width
        && a.max_height == b.max_height
        && a.padding == b.padding
        && a.margin == b.margin
        && a.border_width == b.border_width
        && a.font_family == b.font_family
        && a.font_size == b.font_size
        && a.font_weight == b.font_weight
        && a.italic == b.italic
        && a.letter_spacing == b.letter_spacing
        && a.tabular_nums == b.tabular_nums
        && a.ellipsis == b.ellipsis
}

//! Values of the shape, motion and scroll properties: `corner-shape`,
//! `clip-path`, `transform`, `animation`, `overflow`, scrollbars, carets,
//! `outline`.

use std::time::Duration;

use super::PropertyId;
use super::value::{
    CALC_MESSAGE, ClipVal, Comp, Ctx, Len, Longhand, LpVal, TransformVal, color, components, easing, keyword_of, len,
    no_commas, number, one, split_num, split_top,
};
use crate::anim::Easing;
use crate::style::{AnimDirection, AnimFill, CornerShape, Overflow};

/// Longest `animation-duration`, `transition-duration` or delay kept.
pub(crate) const MAX_ANIM_SECONDS: f64 = 10.0;
/// Most iterations an animation may run.
pub(crate) const MAX_ANIM_COUNT: f64 = 1000.0;
/// Most points a `polygon()` may have.
const MAX_POLYGON_POINTS: usize = 64;

pub(super) fn overflow_kw(c: &Comp) -> Result<Overflow, String> {
    match c.keyword().as_deref() {
        Some("visible") => Ok(Overflow::Visible),
        Some("hidden") => Ok(Overflow::Hidden),
        Some("auto") => Ok(Overflow::Auto),
        Some("scroll") => Ok(Overflow::Scroll),
        Some("clip") => Err("'clip' is not supported; use hidden".into()),
        _ => Err(format!("expected visible, hidden, auto or scroll, got '{}'", c.text())),
    }
}

pub(super) fn parse_overflow(comps: &[Comp]) -> Result<Vec<Longhand>, String> {
    match no_commas(comps)? {
        [a] => {
            let v = overflow_kw(a)?;
            Ok(vec![Longhand::OverflowX(v), Longhand::OverflowY(v)])
        }
        [a, b] => Ok(vec![Longhand::OverflowX(overflow_kw(a)?), Longhand::OverflowY(overflow_kw(b)?)]),
        _ => Err("expected one or two values: x, then y".into()),
    }
}

pub(super) fn parse_corner_shape(comps: &[Comp]) -> Result<Vec<Longhand>, String> {
    let v: Vec<CornerShape> = no_commas(comps)?
        .iter()
        .map(|c| match c.keyword().as_deref() {
            Some("round") => Ok(CornerShape::Round),
            Some("bevel") => Ok(CornerShape::Bevel),
            Some(k @ ("scoop" | "notch" | "squircle" | "square")) => {
                Err(format!("'{k}' is not supported; use round or bevel"))
            }
            _ => Err(format!("expected round or bevel, got '{}'", c.text())),
        })
        .collect::<Result<_, _>>()?;
    Ok(vec![Longhand::CornerShape(super::value::expand4(&v, "corner shapes")?)])
}

/// `13px`, `50%`, `0`, and with `allow_calc` also `calc(100% - 13px)`.
fn lp(c: &Comp, ctx: Ctx, allow_calc: bool) -> Result<LpVal, String> {
    match c {
        Comp::Word(w) => {
            if let Some((n, "%")) = split_num(w) {
                return Ok(LpVal { pct: n, ..LpVal::ZERO });
            }
            Ok(match len(c, ctx)? {
                Len::Px(v) => LpVal { px: v, ..LpVal::ZERO },
                Len::Em(v) => LpVal { em: v, ..LpVal::ZERO },
            })
        }
        Comp::Func(_, name, inner) if name == "calc" => {
            if allow_calc {
                calc(inner, ctx)
            } else {
                Err(CALC_MESSAGE.into())
            }
        }
        other => Err(format!("expected a length or a percentage, got '{}'", other.text())),
    }
}

/// `calc(<percentage> + <length>)` or `-`, either order.
fn calc(inner: &str, ctx: Ctx) -> Result<LpVal, String> {
    const SHAPE: &str = "calc() takes a percentage and a length: calc(100% - 13px)";
    let toks: Vec<&str> = inner.split_whitespace().collect();
    let [a, op, b] = toks.as_slice() else { return Err(SHAPE.into()) };
    let sign = match *op {
        "+" => 1.0,
        "-" => -1.0,
        _ => return Err(SHAPE.into()),
    };
    let term = |t: &str| lp(&Comp::Word(t), ctx, false);
    let (x, y) = (term(a)?, term(b)?);
    Ok(LpVal { px: x.px + sign * y.px, em: x.em + sign * y.em, pct: x.pct + sign * y.pct })
}

pub(super) fn parse_clip_path(comps: &[Comp], ctx: Ctx) -> Result<Vec<Longhand>, String> {
    let c = one(comps, "none or polygon(...)")?;
    if c.keyword().as_deref() == Some("none") {
        return Ok(vec![Longhand::ClipPath(None)]);
    }
    let Comp::Func(_, name, inner) = c else {
        return Err("expected none or polygon(...)".into());
    };
    if name != "polygon" {
        return Err(format!("'{name}()' is not supported; use polygon(x y, x y, ...)"));
    }
    let args = split_top(inner, b',');
    let first = args.first().map(|a| a.trim().to_ascii_lowercase()).unwrap_or_default();
    let (evenodd, points) = match first.as_str() {
        "nonzero" => (false, &args[1..]),
        "evenodd" => (true, &args[1..]),
        _ => (false, &args[..]),
    };
    if points.len() < 3 {
        return Err("'clip-path' needs at least 3 points".into());
    }
    if points.len() > MAX_POLYGON_POINTS {
        return Err(format!("polygon() has more than {MAX_POLYGON_POINTS} points"));
    }
    let mut out = Vec::with_capacity(points.len());
    for p in points {
        let parts = components(p)?;
        let [x, y] = parts.as_slice() else {
            return Err("polygon() points need two values each".into());
        };
        out.push((lp(x, ctx, true)?, lp(y, ctx, true)?));
    }
    Ok(vec![Longhand::ClipPath(Some(ClipVal { points: out, evenodd }))])
}

pub(super) fn parse_transform(comps: &[Comp], ctx: Ctx) -> Result<Vec<Longhand>, String> {
    if matches!(comps, [c] if c.keyword().as_deref() == Some("none")) {
        return Ok(vec![Longhand::Transform(None)]);
    }
    let mut t = TransformVal { tx: LpVal::ZERO, ty: LpVal::ZERO, sx: 1.0, sy: 1.0 };
    let scaled = |v: LpVal, k: f32| LpVal { px: v.px * k, em: v.em * k, pct: v.pct * k };
    let add = |a: LpVal, b: LpVal| LpVal { px: a.px + b.px, em: a.em + b.em, pct: a.pct + b.pct };
    for c in no_commas(comps)? {
        let Comp::Func(_, name, inner) = c else {
            return Err(format!("expected a transform function like translateX(4px), got '{}'", c.text()));
        };
        let args: Vec<Vec<Comp>> =
            split_top(inner, b',').into_iter().map(|a| components(a)).collect::<Result<_, _>>()?;
        let arg = |i: usize| -> Result<&Comp, String> {
            match args.get(i).map(Vec::as_slice) {
                Some([c]) => Ok(c),
                _ => {
                    Err(format!("{name}() takes {}", if name.starts_with("translate") { "lengths" } else { "numbers" }))
                }
            }
        };
        let count = |min: usize, max: usize| {
            if (min..=max).contains(&args.len()) { Ok(()) } else { Err(format!("wrong number of values in {name}()")) }
        };
        match name.as_str() {
            "translate" => {
                count(1, 2)?;
                let x = lp(arg(0)?, ctx, false)?;
                let y = if args.len() > 1 { lp(arg(1)?, ctx, false)? } else { LpVal::ZERO };
                t.tx = add(t.tx, scaled(x, t.sx));
                t.ty = add(t.ty, scaled(y, t.sy));
            }
            "translatex" => {
                count(1, 1)?;
                t.tx = add(t.tx, scaled(lp(arg(0)?, ctx, false)?, t.sx));
            }
            "translatey" => {
                count(1, 1)?;
                t.ty = add(t.ty, scaled(lp(arg(0)?, ctx, false)?, t.sy));
            }
            "scale" | "scalex" | "scaley" => {
                count(1, if name == "scale" { 2 } else { 1 })?;
                let x = scale_factor(arg(0)?)?;
                let y = if args.len() > 1 { scale_factor(arg(1)?)? } else { x };
                match name.as_str() {
                    "scale" => {
                        t.sx *= x;
                        t.sy *= y;
                    }
                    "scalex" => t.sx *= x,
                    _ => t.sy *= x,
                }
            }
            "rotate" | "rotatex" | "rotatey" | "rotatez" | "rotate3d" | "skew" | "skewx" | "skewy" | "matrix"
            | "matrix3d" | "perspective" => {
                return Err("rotate, skew and matrix are not supported; use translate and scale".into());
            }
            other => {
                return Err(format!(
                    "unknown transform function '{other}'; use translate, translateX, translateY, scale, scaleX or scaleY"
                ));
            }
        }
    }
    Ok(vec![Longhand::Transform(Some(t))])
}

fn scale_factor(c: &Comp) -> Result<f32, String> {
    let n = number(c)?;
    if n < 0.0 { Err("a scale must not be negative".into()) } else { Ok(n) }
}

pub(super) fn parse_origin(comps: &[Comp], ctx: Ctx) -> Result<Vec<Longhand>, String> {
    let comps = no_commas(comps)?;
    // Keywords name an axis (top / bottom: y, left / right: x, center: either).
    enum V {
        X(LpVal),
        Y(LpVal),
        Either(LpVal),
    }
    let pct = |p: f32| LpVal { pct: p, ..LpVal::ZERO };
    let one_val = |c: &Comp| -> Result<V, String> {
        Ok(match c.keyword().as_deref() {
            Some("left") => V::X(pct(0.0)),
            Some("right") => V::X(pct(100.0)),
            Some("top") => V::Y(pct(0.0)),
            Some("bottom") => V::Y(pct(100.0)),
            Some("center") => V::Either(pct(50.0)),
            _ => V::Either(lp(c, ctx, false)?),
        })
    };
    let (x, y) = match comps {
        [a] => match one_val(a)? {
            V::X(v) | V::Either(v) => (v, pct(50.0)),
            V::Y(v) => (pct(50.0), v),
        },
        [a, b] => match (one_val(a)?, one_val(b)?) {
            (V::X(x) | V::Either(x), V::Y(y) | V::Either(y)) => (x, y),
            (V::Y(y), V::X(x) | V::Either(x)) => (x, y),
            _ => return Err("expected one value for each axis, like 'left top' or '10px 50%'".into()),
        },
        _ => return Err("expected one or two values".into()),
    };
    Ok(vec![Longhand::TransformOrigin(x, y)])
}

/// A time in seconds, negative allowed (`animation-delay`).
fn time_s(c: &Comp) -> Result<f64, String> {
    let Comp::Word(w) = c else { return Err(format!("expected a time like 200ms, got '{}'", c.text())) };
    let Some((n, unit)) = split_num(w) else { return Err(format!("expected a time like 200ms, got '{w}'")) };
    match unit.to_ascii_lowercase().as_str() {
        "ms" => Ok(f64::from(n) / 1000.0),
        "s" => Ok(f64::from(n)),
        "" if n == 0.0 => Ok(0.0),
        _ => Err(format!("expected a time like 200ms or 0.2s, got '{w}'")),
    }
}

fn dur(secs: f64) -> Result<Duration, String> {
    if secs < 0.0 {
        return Err("a duration must be 0 or more".into());
    }
    Ok(Duration::from_secs_f64(secs.min(1.0e6)))
}

fn is_time(c: &Comp) -> bool {
    matches!(c, Comp::Word(w) if matches!(split_num(w), Some((_, "ms" | "s" | "MS" | "S"))))
}

fn direction(k: &str) -> Option<AnimDirection> {
    Some(match k {
        "normal" => AnimDirection::Normal,
        "reverse" => AnimDirection::Reverse,
        "alternate" => AnimDirection::Alternate,
        "alternate-reverse" => AnimDirection::AlternateReverse,
        _ => return None,
    })
}

fn fill(k: &str) -> Option<AnimFill> {
    Some(match k {
        "none" => AnimFill::None,
        "forwards" => AnimFill::Forwards,
        "backwards" => AnimFill::Backwards,
        "both" => AnimFill::Both,
        _ => return None,
    })
}

fn count(c: &Comp) -> Result<f64, String> {
    if c.keyword().as_deref() == Some("infinite") {
        return Ok(f64::INFINITY);
    }
    let n = number(c)?;
    if n < 0.0 { Err("an iteration count must not be negative".into()) } else { Ok(f64::from(n)) }
}

fn ident(c: &Comp) -> Result<std::sync::Arc<str>, String> {
    match c {
        Comp::Word(w)
            if !w.is_empty()
                && w.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                && !w.as_bytes()[0].is_ascii_digit() =>
        {
            Ok(std::sync::Arc::from(*w))
        }
        other => Err(format!("expected an animation name like 'pulse', got '{}'", other.text())),
    }
}

pub(super) fn parse_animation(comps: &[Comp]) -> Result<Vec<Longhand>, String> {
    let groups: Vec<&[Comp]> = comps.split(|c| *c == Comp::Comma).collect();
    if matches!(groups.as_slice(), [[c]] if c.keyword().as_deref() == Some("none")) {
        return Ok(vec![
            Longhand::AnimName(vec![None]),
            Longhand::AnimDuration(vec![Duration::ZERO]),
            Longhand::AnimTiming(vec![Easing::Ease]),
            Longhand::AnimDelay(vec![0.0]),
            Longhand::AnimIterations(vec![1.0]),
            Longhand::AnimDirection(vec![AnimDirection::Normal]),
            Longhand::AnimFill(vec![AnimFill::None]),
            Longhand::AnimPaused(vec![false]),
        ]);
    }
    let (mut names, mut durs, mut eases, mut delays) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut counts, mut dirs, mut fills, mut paused) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for g in groups {
        if g.is_empty() {
            return Err("expected '<name> <duration> [easing] [delay] [count] [direction] [fill]'".into());
        }
        let (mut name, mut d, mut delay, mut ease, mut cnt, mut dir, mut fl, mut play) =
            (None, None, None, None, None, None, None, None);
        for c in g {
            let kw = c.keyword();
            if let Some(e) = easing(c) {
                if ease.replace(e?).is_some() {
                    return Err("only one easing per animation".into());
                }
            } else if is_time(c) {
                let t = time_s(c)?;
                if d.is_none() {
                    d = Some(t);
                } else if delay.is_none() {
                    delay = Some(t);
                } else {
                    return Err("at most two times (duration and delay) per animation".into());
                }
            } else if matches!(c, Comp::Word(w) if split_num(w).is_some_and(|(_, u)| u.is_empty()))
                || kw.as_deref() == Some("infinite")
            {
                if cnt.replace(count(c)?).is_some() {
                    return Err("only one iteration count per animation".into());
                }
            } else if let Some(v) = kw.as_deref().and_then(direction) {
                if dir.replace(v).is_some() {
                    return Err("only one direction per animation".into());
                }
            } else if let Some(v) = kw.as_deref().filter(|k| *k != "none").and_then(fill) {
                if fl.replace(v).is_some() {
                    return Err("only one fill mode per animation".into());
                }
            } else if let Some(p @ ("running" | "paused")) = kw.as_deref() {
                if play.replace(p == "paused").is_some() {
                    return Err("only one play state per animation".into());
                }
            } else if kw.as_deref() == Some("none") {
                name = Some(None);
            } else {
                if name.is_some() {
                    return Err(format!("unexpected '{}'", c.text()));
                }
                name = Some(Some(ident(c)?));
            }
        }
        let Some(name) = name else { return Err("an animation needs a name like 'pulse'".into()) };
        let Some(d) = d else { return Err("an animation needs a duration like 300ms".into()) };
        names.push(name);
        durs.push(dur(d)?);
        eases.push(ease.unwrap_or(Easing::Ease));
        delays.push(delay.unwrap_or(0.0));
        counts.push(cnt.unwrap_or(1.0));
        dirs.push(dir.unwrap_or(AnimDirection::Normal));
        fills.push(fl.unwrap_or(AnimFill::None));
        paused.push(play.unwrap_or(false));
    }
    Ok(vec![
        Longhand::AnimName(names),
        Longhand::AnimDuration(durs),
        Longhand::AnimTiming(eases),
        Longhand::AnimDelay(delays),
        Longhand::AnimIterations(counts),
        Longhand::AnimDirection(dirs),
        Longhand::AnimFill(fills),
        Longhand::AnimPaused(paused),
    ])
}

pub(super) fn parse_animation_longhand(id: PropertyId, comps: &[Comp]) -> Result<Vec<Longhand>, String> {
    use PropertyId as P;
    let each = comps.split(|c| *c == Comp::Comma);
    Ok(vec![match id {
        P::AnimationName => Longhand::AnimName(
            each.map(|g| {
                let c = one(g, "none or an animation name")?;
                if c.keyword().as_deref() == Some("none") { Ok(None) } else { ident(c).map(Some) }
            })
            .collect::<Result<_, String>>()?,
        ),
        P::AnimationDuration => Longhand::AnimDuration(
            each.map(|g| one(g, "a time like 300ms").and_then(time_s).and_then(dur)).collect::<Result<_, _>>()?,
        ),
        P::AnimationTimingFunction => Longhand::AnimTiming(
            each.map(|g| {
                let c = one(g, "an easing")?;
                easing(c).unwrap_or_else(|| Err(format!("unknown easing '{}'", c.text())))
            })
            .collect::<Result<_, String>>()?,
        ),
        P::AnimationDelay => {
            Longhand::AnimDelay(each.map(|g| one(g, "a time like 300ms").and_then(time_s)).collect::<Result<_, _>>()?)
        }
        P::AnimationIterationCount => Longhand::AnimIterations(
            each.map(|g| one(g, "a number or infinite").and_then(count)).collect::<Result<_, _>>()?,
        ),
        P::AnimationDirection => Longhand::AnimDirection(
            each.map(|g| {
                let c = one(g, "normal, reverse, alternate or alternate-reverse")?;
                c.keyword().as_deref().and_then(direction).ok_or_else(|| {
                    format!("expected normal, reverse, alternate or alternate-reverse, got '{}'", c.text())
                })
            })
            .collect::<Result<_, String>>()?,
        ),
        P::AnimationFillMode => Longhand::AnimFill(
            each.map(|g| {
                let c = one(g, "none, forwards, backwards or both")?;
                c.keyword()
                    .as_deref()
                    .and_then(fill)
                    .ok_or_else(|| format!("expected none, forwards, backwards or both, got '{}'", c.text()))
            })
            .collect::<Result<_, String>>()?,
        ),
        _ => Longhand::AnimPaused(
            each.map(|g| match keyword_of(g, &["running", "paused"])?.as_str() {
                "paused" => Ok(true),
                _ => Ok(false),
            })
            .collect::<Result<_, String>>()?,
        ),
    }])
}

pub(super) fn auto_or_color(comps: &[Comp]) -> Result<Option<super::value::ColorVal>, String> {
    let c = one(no_commas(comps)?, "auto or a colour")?;
    if c.keyword().as_deref() == Some("auto") { Ok(None) } else { color(c).map(Some) }
}

pub(super) fn parse_scrollbar_color(comps: &[Comp]) -> Result<Vec<Longhand>, String> {
    let comps = no_commas(comps)?;
    if matches!(comps, [c] if c.keyword().as_deref() == Some("auto")) {
        return Ok(vec![Longhand::ScrollbarColor(None)]);
    }
    match comps {
        [thumb, track] => Ok(vec![Longhand::ScrollbarColor(Some((color(thumb)?, color(track)?)))]),
        _ => Err("expected auto, or a thumb colour and a track colour".into()),
    }
}

pub(super) fn parse_outline(comps: &[Comp], ctx: Ctx) -> Result<Vec<Longhand>, String> {
    let comps = no_commas(comps)?;
    let (mut width, mut col, mut none) = (None, None, false);
    for c in comps {
        if let Some(kw) = c.keyword() {
            match kw.as_str() {
                "solid" => continue,
                "none" | "hidden" => {
                    none = true;
                    continue;
                }
                "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset" => {
                    return Err(format!("only solid outlines are supported, not '{kw}'"));
                }
                "thin" | "medium" | "thick" => return Err(format!("'{kw}' is not supported; use a length like 2px")),
                _ => {}
            }
        }
        if matches!(c, Comp::Word(w) if split_num(w).is_some()) {
            if width.replace(super::value::nonneg(len(c, ctx)?)?).is_some() {
                return Err("only one width".into());
            }
        } else if col.replace(color(c)?).is_some() {
            return Err("only one colour".into());
        }
    }
    if comps.is_empty() {
        return Err("expected '2px solid <colour>' or none".into());
    }
    let w = if none { Len::Px(0.0) } else { width.unwrap_or(Len::Px(1.0)) };
    let mut out = vec![Longhand::OutlineWidth(w)];
    if !none {
        out.push(Longhand::OutlineColor(col.unwrap_or(super::value::ColorVal::Current)));
    }
    Ok(out)
}

/// Clamps long durations and big counts, one note each.
pub(super) fn clamp_longhands(lhs: &mut [Longhand], warns: &mut Vec<String>) {
    let max = Duration::from_secs_f64(MAX_ANIM_SECONDS);
    let mut clamped_time = false;
    let mut clamped_count = false;
    for l in lhs {
        match l {
            Longhand::TransitionDuration(v) | Longhand::TransitionDelay(v) | Longhand::AnimDuration(v) => {
                for d in v {
                    if *d > max {
                        *d = max;
                        clamped_time = true;
                    }
                }
            }
            Longhand::AnimDelay(v) => {
                for d in v {
                    if d.abs() > MAX_ANIM_SECONDS {
                        *d = d.signum() * MAX_ANIM_SECONDS;
                        clamped_time = true;
                    }
                }
            }
            Longhand::AnimIterations(v) => {
                for n in v {
                    if n.is_finite() && *n > MAX_ANIM_COUNT {
                        *n = MAX_ANIM_COUNT;
                        clamped_count = true;
                    }
                }
            }
            _ => {}
        }
    }
    if clamped_time {
        warns.push("a time above 10s was cut to 10s".into());
    }
    if clamped_count {
        warns.push("an iteration count above 1000 was cut to 1000".into());
    }
}

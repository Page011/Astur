//! Declaration values: splitting a value into components and turning each
//! property's text into the typed longhands the cascade applies.

use std::sync::Arc;
use std::time::Duration;

use super::{PropertyId, PropertyInfo};
use crate::anim::Easing;
use crate::color::Color;
use crate::style::{Align, Backdrop, Cursor, FlexDirection, Justify, LinearGradient, TextAlign, TransitionProperty};

/// A length as written: px or em (em is resolved against the element's own
/// font size at computed-value time).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Len {
    Px(f32),
    Em(f32),
}

/// A size as written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DimVal {
    Auto,
    Px(f32),
    Em(f32),
    Percent(f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ColorVal {
    Color(Color),
    Current,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FamilyName {
    Named(Arc<str>),
    Sans,
    Mono,
    Icons,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Corner {
    Tl,
    Tr,
    Br,
    Bl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SizeProp {
    Width,
    Height,
    MinWidth,
    MinHeight,
    MaxWidth,
    MaxHeight,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Bg {
    None,
    Color(ColorVal),
    Gradient(LinearGradient),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ShadowVal {
    pub x: Len,
    pub y: Len,
    pub blur: Len,
    pub spread: Len,
    pub color: ColorVal,
}

/// One longhand value. Shorthands expand to several of these in written order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Longhand {
    /// The property goes back to inherited (inheriting ones) or initial.
    Unset(PropertyId),
    Color(ColorVal),
    Background(Bg),
    Opacity(f32),
    BorderWidth(Side, Len),
    BorderColor(ColorVal),
    Radius(Corner, DimVal),
    Padding(Side, Len),
    Margin(Side, Option<Len>),
    Gap(Len),
    FlexDirection(FlexDirection),
    FlexGrow(f32),
    FlexShrink(f32),
    Justify(Justify),
    AlignItems(Align),
    Size(SizeProp, DimVal),
    FontFamily(Vec<FamilyName>),
    FontSize(DimVal),
    FontWeight(u16),
    Italic(bool),
    TabularNums(bool),
    LetterSpacing(Len),
    TextAlign(TextAlign),
    Ellipsis(bool),
    Clip(bool),
    BoxShadow(Vec<ShadowVal>),
    DisplayNone(bool),
    Visible(bool),
    /// Empty = `none`.
    TransitionProperty(Vec<TransitionProperty>),
    TransitionDuration(Vec<Duration>),
    TransitionDelay(Vec<Duration>),
    TransitionTiming(Vec<Easing>),
    Cursor(Cursor),
    Backdrop(Backdrop),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Ctx {
    /// Inline style from a script: a unitless number is a length in px.
    pub inline: bool,
}

/// A piece of a value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Comp<'a> {
    Word(&'a str),
    /// `name(inner)`: the whole text, the lower-case name, the inner text.
    Func(&'a str, String, &'a str),
    Str(String),
    Comma,
}

impl Comp<'_> {
    fn text(&self) -> String {
        match self {
            Comp::Word(w) => (*w).to_string(),
            Comp::Func(w, _, _) => (*w).to_string(),
            Comp::Str(s) => format!("\"{s}\""),
            Comp::Comma => ",".into(),
        }
    }

    fn keyword(&self) -> Option<String> {
        match self {
            Comp::Word(w) => Some(w.to_ascii_lowercase()),
            _ => None,
        }
    }
}

/// Splits a value into words, functions, strings and commas.
pub(crate) fn components(s: &str) -> Result<Vec<Comp<'_>>, String> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b',' {
            out.push(Comp::Comma);
            i += 1;
        } else if c == b'"' || c == b'\'' {
            let mut j = i + 1;
            let mut text = String::new();
            loop {
                match bytes.get(j) {
                    None => return Err("missing closing quote".into()),
                    Some(b'\\') if j + 1 < bytes.len() => {
                        let ch = s[j + 1..].chars().next().unwrap_or('\\');
                        text.push(ch);
                        j += 1 + ch.len_utf8();
                    }
                    Some(&q) if q == c => break,
                    Some(_) => {
                        let ch = s[j..].chars().next().unwrap_or(' ');
                        text.push(ch);
                        j += ch.len_utf8();
                    }
                }
            }
            out.push(Comp::Str(text));
            i = j + 1;
        } else {
            let start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b',' && bytes[i] != b'(' {
                i += 1;
            }
            if bytes.get(i) == Some(&b'(') {
                let open = i;
                let close = matching_paren(s, open).ok_or_else(|| "missing ')'".to_string())?;
                out.push(Comp::Func(&s[start..=close], s[start..open].to_ascii_lowercase(), &s[open + 1..close]));
                i = close + 1;
            } else {
                out.push(Comp::Word(&s[start..i]));
            }
        }
    }
    Ok(out)
}

/// Index of the `)` that closes the `(` at `open`, skipping strings.
pub(crate) fn matching_paren(s: &str, open: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            q @ (b'"' | b'\'') => {
                i += 1;
                while i < bytes.len() && bytes[i] != q {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Splits at top-level occurrences of `sep` (outside parentheses and strings).
pub(crate) fn split_top(s: &str, sep: u8) -> Vec<&str> {
    let bytes = s.as_bytes();
    let (mut out, mut depth, mut start, mut i) = (Vec::new(), 0i32, 0, 0);
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            q @ (b'"' | b'\'') => {
                i += 1;
                while i < bytes.len() && bytes[i] != q {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            c if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(&s[start..]);
    out
}

/// `12.5px` -> (12.5, "px"). Plain decimal numbers only (no exponent).
pub(crate) fn split_num(s: &str) -> Option<(f32, &str)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    let digits_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let mut had = i > digits_start;
    if bytes.get(i) == Some(&b'.') {
        let frac = i + 1;
        let mut j = frac;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j > frac {
            had = true;
            i = j;
        } else if had {
            return None;
        }
    }
    if !had {
        return None;
    }
    let n: f32 = s[..i].parse().ok()?;
    let rest = &s[i..];
    (n.is_finite() && rest.bytes().all(|b| b.is_ascii_alphabetic() || b == b'%')).then_some((n, rest))
}

fn number(c: &Comp) -> Result<f32, String> {
    match c {
        Comp::Word(w) => match split_num(w) {
            Some((n, "")) => Ok(n),
            _ => Err(format!("expected a number, got '{w}'")),
        },
        other => Err(format!("expected a number, got '{}'", other.text())),
    }
}

fn len(c: &Comp, ctx: Ctx) -> Result<Len, String> {
    match c {
        Comp::Word(w) => {
            let Some((n, unit)) = split_num(w) else {
                return Err(format!("expected a length like 12px, got '{w}'"));
            };
            match unit.to_ascii_lowercase().as_str() {
                "px" => Ok(Len::Px(n)),
                "em" => Ok(Len::Em(n)),
                "" if n == 0.0 || ctx.inline => Ok(Len::Px(n)),
                "" => Err(format!("'{w}' needs a unit; use px (or em)")),
                "pt" => Err("unit 'pt' is not supported; use px".into()),
                "rem" => Err("unit 'rem' is not supported; use em".into()),
                "%" => Err(format!("'{w}': a percentage is not allowed here; use px or em")),
                u => Err(format!("unknown unit '{u}'; use px or em")),
            }
        }
        Comp::Func(_, name, _) if name == "calc" => Err("calc() is not supported; write one length".into()),
        other => Err(format!("expected a length like 12px, got '{}'", other.text())),
    }
}

fn nonneg(l: Len) -> Result<Len, String> {
    match l {
        Len::Px(v) | Len::Em(v) if v < 0.0 => Err("must not be negative".into()),
        l => Ok(l),
    }
}

/// px, em or %.
fn dim(c: &Comp, ctx: Ctx) -> Result<DimVal, String> {
    if let Comp::Word(w) = c
        && let Some((n, "%")) = split_num(w)
    {
        return Ok(DimVal::Percent(n));
    }
    Ok(match len(c, ctx)? {
        Len::Px(v) => DimVal::Px(v),
        Len::Em(v) => DimVal::Em(v),
    })
}

fn color(c: &Comp) -> Result<ColorVal, String> {
    match c {
        Comp::Word(w) if w.eq_ignore_ascii_case("currentcolor") => Ok(ColorVal::Current),
        Comp::Word(_) | Comp::Func(..) => Color::parse(&c.text()).map(ColorVal::Color),
        other => Err(format!("expected a colour, got '{}'", other.text())),
    }
}

fn one<'a, 'b>(comps: &'b [Comp<'a>], what: &str) -> Result<&'b Comp<'a>, String> {
    match comps {
        [c] => Ok(c),
        [] => Err(format!("expected {what}")),
        _ => Err(format!("expected one value ({what})")),
    }
}

/// Expands 1 to 4 values in CSS order (top right bottom left).
fn expand4<T: Copy>(v: &[T], what: &str) -> Result<[T; 4], String> {
    match v {
        [a] => Ok([*a, *a, *a, *a]),
        [a, b] => Ok([*a, *b, *a, *b]),
        [a, b, c] => Ok([*a, *b, *c, *b]),
        [a, b, c, d] => Ok([*a, *b, *c, *d]),
        _ => Err(format!("expected 1 to 4 {what}")),
    }
}

fn no_commas<'a, 'b>(comps: &'b [Comp<'a>]) -> Result<&'b [Comp<'a>], String> {
    if comps.contains(&Comp::Comma) { Err("unexpected ','".into()) } else { Ok(comps) }
}

fn keyword_of(comps: &[Comp], allowed: &[&str]) -> Result<String, String> {
    let c = one(comps, &allowed.join(" | "))?;
    match c.keyword() {
        Some(k) if allowed.contains(&k.as_str()) => Ok(k),
        _ => Err(format!("expected {}, got '{}'", allowed.join(", "), c.text())),
    }
}

const SIDES: [Side; 4] = [Side::Top, Side::Right, Side::Bottom, Side::Left];
const CORNERS: [Corner; 4] = [Corner::Tl, Corner::Tr, Corner::Br, Corner::Bl];

fn time(c: &Comp) -> Result<Duration, String> {
    let Comp::Word(w) = c else { return Err(format!("expected a time like 200ms, got '{}'", c.text())) };
    let Some((n, unit)) = split_num(w) else { return Err(format!("expected a time like 200ms, got '{w}'")) };
    let secs = match unit.to_ascii_lowercase().as_str() {
        "ms" => n / 1000.0,
        "s" => n,
        "" if n == 0.0 => 0.0,
        _ => return Err(format!("expected a time like 200ms or 0.2s, got '{w}'")),
    };
    crate::duration::from_seconds(secs as f64)
}

fn easing(c: &Comp) -> Option<Result<Easing, String>> {
    match c {
        Comp::Word(w) => Some(Ok(match w.to_ascii_lowercase().as_str() {
            "linear" => Easing::Linear,
            "ease" => Easing::Ease,
            "ease-in" => Easing::EaseIn,
            "ease-out" => Easing::EaseOut,
            "ease-in-out" => Easing::EaseInOut,
            _ => return None,
        })),
        Comp::Func(_, name, inner) if name == "cubic-bezier" => {
            let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
            let nums: Option<Vec<f32>> =
                parts.iter().map(|p| split_num(p).filter(|(_, u)| u.is_empty()).map(|x| x.0)).collect();
            Some(match nums.as_deref() {
                Some(&[x1, y1, x2, y2]) if (0.0..=1.0).contains(&x1) && (0.0..=1.0).contains(&x2) => {
                    Ok(Easing::CubicBezier(x1, y1, x2, y2))
                }
                Some(&[_, _, _, _]) => Err("cubic-bezier x values must be between 0 and 1".into()),
                _ => Err("expected cubic-bezier(x1, y1, x2, y2)".into()),
            })
        }
        _ => None,
    }
}

fn transition_property(name: &str) -> Result<TransitionProperty, String> {
    if name == "all" {
        return Ok(TransitionProperty::All);
    }
    match super::property_by_name(name) {
        Some(p) if p.animatable => Ok(TransitionProperty::One(p.id)),
        Some(_) => Err(format!("'{name}' cannot be animated")),
        None => Err(format!(
            "unknown property '{name}'{}",
            crate::suggest::hint(name, super::PROPERTIES.iter().map(|p| p.name))
        )),
    }
}

fn parse_transition(comps: &[Comp]) -> Result<Vec<Longhand>, String> {
    let groups: Vec<&[Comp]> = comps.split(|c| *c == Comp::Comma).collect();
    if groups.len() == 1 && matches!(groups[0], [c] if c.keyword().as_deref() == Some("none")) {
        return Ok(vec![
            Longhand::TransitionProperty(Vec::new()),
            Longhand::TransitionDuration(vec![Duration::ZERO]),
            Longhand::TransitionDelay(vec![Duration::ZERO]),
            Longhand::TransitionTiming(vec![Easing::Ease]),
        ]);
    }
    let (mut props, mut durs, mut delays, mut eases) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for g in groups {
        if g.is_empty() {
            return Err("expected 'property duration [easing] [delay]'".into());
        }
        let (mut prop, mut dur, mut delay, mut ease) = (None, None, None, None);
        for c in g {
            if let Some(e) = easing(c) {
                if ease.replace(e?).is_some() {
                    return Err("only one easing per transition".into());
                }
            } else if let Ok(t) = time(c) {
                if dur.is_none() {
                    dur = Some(t);
                } else if delay.is_none() {
                    delay = Some(t);
                } else {
                    return Err("at most two times (duration and delay) per transition".into());
                }
            } else if let Comp::Word(w) = c {
                if prop.is_some() {
                    return Err(format!("unexpected '{w}'"));
                }
                prop = Some(transition_property(&w.to_ascii_lowercase())?);
            } else {
                return Err(format!("unexpected '{}'", c.text()));
            }
        }
        let Some(dur) = dur else { return Err("a transition needs a duration like 150ms".into()) };
        props.push(prop.unwrap_or(TransitionProperty::All));
        durs.push(dur);
        delays.push(delay.unwrap_or(Duration::ZERO));
        eases.push(ease.unwrap_or(Easing::Ease));
    }
    Ok(vec![
        Longhand::TransitionProperty(props),
        Longhand::TransitionDuration(durs),
        Longhand::TransitionDelay(delays),
        Longhand::TransitionTiming(eases),
    ])
}

fn parse_gradient(inner: &str) -> Result<LinearGradient, String> {
    let args = split_top(inner, b',');
    let mut angle = 180.0f32;
    let mut rest = &args[..];
    let first = args.first().map(|a| a.trim().to_ascii_lowercase()).unwrap_or_default();
    if let Some(sides) = first.strip_prefix("to ") {
        let words: Vec<&str> = sides.split_whitespace().collect();
        angle = match words.as_slice() {
            ["top"] => 0.0,
            ["right"] => 90.0,
            ["bottom"] => 180.0,
            ["left"] => 270.0,
            ["top", "right"] | ["right", "top"] => 45.0,
            ["bottom", "right"] | ["right", "bottom"] => 135.0,
            ["bottom", "left"] | ["left", "bottom"] => 225.0,
            ["top", "left"] | ["left", "top"] => 315.0,
            _ => return Err("expected 'to top', 'to right bottom' and so on".into()),
        };
        rest = &args[1..];
    } else if let Some((n, unit)) = split_num(&first) {
        match unit {
            "deg" => angle = n,
            "turn" => angle = n * 360.0,
            _ => {}
        }
        if matches!(unit, "deg" | "turn") {
            rest = &args[1..];
        }
    }
    let mut stops: Vec<(Option<f32>, Color)> = Vec::new();
    for arg in rest {
        let comps = components(arg)?;
        let Some(first) = comps.first() else { return Err("empty colour stop".into()) };
        let c = match color(first)? {
            ColorVal::Color(c) => c,
            ColorVal::Current => return Err("currentcolor is not supported inside a gradient".into()),
        };
        let mut positions = Vec::new();
        for p in &comps[1..] {
            match p {
                Comp::Word(w) => match split_num(w) {
                    Some((n, "%")) => positions.push(n / 100.0),
                    _ => return Err(format!("stop positions must be percentages, got '{w}'")),
                },
                other => return Err(format!("unexpected '{}' in a colour stop", other.text())),
            }
        }
        match positions.as_slice() {
            [] => stops.push((None, c)),
            [a] => stops.push((Some(*a), c)),
            [a, b] => {
                stops.push((Some(*a), c));
                stops.push((Some(*b), c));
            }
            _ => return Err("a colour stop has at most two positions".into()),
        }
    }
    if stops.len() < 2 {
        return Err("a gradient needs at least two colours".into());
    }
    let n = stops.len();
    if stops[0].0.is_none() {
        stops[0].0 = Some(0.0);
    }
    if stops[n - 1].0.is_none() {
        stops[n - 1].0 = Some(1.0);
    }
    let mut pos: Vec<f32> = Vec::with_capacity(n);
    let mut prev = f32::NEG_INFINITY;
    for s in &stops {
        let v = s.0.map_or(f32::NAN, |p| p.clamp(0.0, 1.0).max(prev.max(0.0)));
        if !v.is_nan() {
            prev = v;
        }
        pos.push(v);
    }
    let mut i = 0;
    while i < n {
        if pos[i].is_nan() {
            let start = i - 1;
            let mut end = i;
            while pos[end].is_nan() {
                end += 1;
            }
            let (a, b) = (pos[start], pos[end]);
            for (k, p) in pos.iter_mut().enumerate().take(end).skip(start + 1) {
                *p = a + (b - a) * (k - start) as f32 / (end - start) as f32;
            }
            i = end;
        }
        i += 1;
    }
    Ok(LinearGradient { angle_deg: angle, stops: pos.into_iter().zip(stops).map(|(p, (_, c))| (p, c)).collect() })
}

fn parse_background(comps: &[Comp], ctx: Ctx) -> Result<Vec<Longhand>, String> {
    let _ = ctx;
    let c = one(no_commas(comps)?, "a colour, a gradient or none")?;
    if c.keyword().as_deref() == Some("none") {
        return Ok(vec![Longhand::Background(Bg::None)]);
    }
    match c {
        Comp::Func(_, name, inner) if name == "linear-gradient" => {
            Ok(vec![Longhand::Background(Bg::Gradient(parse_gradient(inner)?))])
        }
        Comp::Func(_, name, _) if name.ends_with("gradient") => Err("only linear-gradient() is supported".into()),
        Comp::Func(_, name, _) if name == "url" => Err("images are not supported as backgrounds".into()),
        _ => Ok(vec![Longhand::Background(Bg::Color(color(c)?))]),
    }
}

fn parse_border(side: Option<Side>, comps: &[Comp], ctx: Ctx) -> Result<Vec<Longhand>, String> {
    let comps = no_commas(comps)?;
    if comps.is_empty() {
        return Err("expected '1px solid <colour>'".into());
    }
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
                    return Err(format!("only solid borders are supported, not '{kw}'"));
                }
                "thin" | "medium" | "thick" => return Err(format!("'{kw}' is not supported; use a length like 1px")),
                _ => {}
            }
        }
        if matches!(c, Comp::Word(w) if split_num(w).is_some()) || matches!(c, Comp::Func(_, n, _) if n == "calc") {
            if width.replace(nonneg(len(c, ctx)?)?).is_some() {
                return Err("only one width".into());
            }
        } else if col.replace(color(c)?).is_some() {
            return Err("only one colour".into());
        }
    }
    let w = if none { Len::Px(0.0) } else { width.unwrap_or(Len::Px(1.0)) };
    let mut out = Vec::new();
    match side {
        None => out.extend(SIDES.iter().map(|s| Longhand::BorderWidth(*s, w))),
        Some(s) => out.push(Longhand::BorderWidth(s, w)),
    }
    if !none {
        out.push(Longhand::BorderColor(col.unwrap_or(ColorVal::Current)));
    }
    Ok(out)
}

fn parse_family(comps: &[Comp]) -> Result<Vec<FamilyName>, String> {
    let mut out = Vec::new();
    for g in comps.split(|c| *c == Comp::Comma) {
        if g.is_empty() {
            return Err("expected a font name".into());
        }
        if let [Comp::Str(s)] = g {
            if s.trim().is_empty() {
                return Err("empty font name".into());
            }
            out.push(FamilyName::Named(Arc::from(s.trim())));
            continue;
        }
        let mut words = Vec::new();
        for c in g {
            match c {
                Comp::Word(w) => words.push(*w),
                other => return Err(format!("unexpected '{}' in font-family", other.text())),
            }
        }
        let name = words.join(" ");
        out.push(match name.to_ascii_lowercase().as_str() {
            "sans-serif" | "system-ui" | "ui-sans-serif" => FamilyName::Sans,
            "monospace" | "ui-monospace" => FamilyName::Mono,
            "icons" => FamilyName::Icons,
            _ => FamilyName::Named(Arc::from(name.as_str())),
        });
    }
    Ok(out)
}

fn parse_shadow(comps: &[Comp], ctx: Ctx) -> Result<Longhand, String> {
    if let [c] = comps
        && c.keyword().as_deref() == Some("none")
    {
        return Ok(Longhand::BoxShadow(Vec::new()));
    }
    let mut out = Vec::new();
    for g in comps.split(|c| *c == Comp::Comma) {
        let (mut lens, mut col) = (Vec::new(), None);
        for c in g {
            if c.keyword().as_deref() == Some("inset") {
                return Err("inset shadows are not supported".into());
            }
            if matches!(c, Comp::Word(w) if split_num(w).is_some()) {
                lens.push(len(c, ctx)?);
            } else if col.replace(color(c)?).is_some() {
                return Err("only one colour per shadow".into());
            }
        }
        if !(2..=4).contains(&lens.len()) {
            return Err("expected 'x y [blur [spread]] colour'".into());
        }
        let blur =
            nonneg(*lens.get(2).unwrap_or(&Len::Px(0.0))).map_err(|_| "blur must not be negative".to_string())?;
        out.push(ShadowVal {
            x: lens[0],
            y: lens[1],
            blur,
            spread: *lens.get(3).unwrap_or(&Len::Px(0.0)),
            color: col.unwrap_or(ColorVal::Current),
        });
    }
    Ok(Longhand::BoxShadow(out))
}

fn parse_size(prop: SizeProp, comps: &[Comp], ctx: Ctx) -> Result<Vec<Longhand>, String> {
    let c = one(no_commas(comps)?, "a length, a percentage or auto")?;
    let max = matches!(prop, SizeProp::MaxWidth | SizeProp::MaxHeight);
    if let Some(k) = c.keyword() {
        match (k.as_str(), max) {
            ("auto", false) | ("none", true) => return Ok(vec![Longhand::Size(prop, DimVal::Auto)]),
            ("auto", true) => return Err("use 'none' for no limit".into()),
            ("none", false) => return Err("use 'auto'".into()),
            ("min-content" | "max-content" | "fit-content" | "stretch", _) => {
                return Err(format!("'{k}' is not supported; use a length, a percentage or auto"));
            }
            _ => {}
        }
    }
    let d = dim(c, ctx)?;
    if matches!(d, DimVal::Px(v) | DimVal::Em(v) | DimVal::Percent(v) if v < 0.0) {
        return Err("must not be negative".into());
    }
    Ok(vec![Longhand::Size(prop, d)])
}

/// Parses one declaration's value for the property `info`. The error is the
/// short reason shown after `bad value for '<property>':`.
pub(crate) fn parse_property(info: &PropertyInfo, value: &str, ctx: Ctx) -> Result<Vec<Longhand>, String> {
    let comps = components(value)?;
    if comps.is_empty() {
        return Err("expected a value".into());
    }
    if let [c] = comps.as_slice() {
        match c.keyword().as_deref() {
            Some("unset") => return Ok(vec![Longhand::Unset(info.id)]),
            Some("inherit" | "initial" | "revert") => return Err("only 'unset' is supported".into()),
            _ => {}
        }
    }
    use PropertyId as P;
    let comps = &comps[..];
    Ok(match info.id {
        P::Color => vec![Longhand::Color(color(one(comps, "a colour")?)?)],
        P::Background => return parse_background(comps, ctx),
        P::BackgroundColor => {
            let c = one(comps, "a colour")?;
            if matches!(c, Comp::Func(_, n, _) if n.ends_with("gradient")) {
                return Err("expected a colour; use 'background' for a gradient".into());
            }
            vec![Longhand::Background(Bg::Color(color(c)?))]
        }
        P::Opacity => {
            let c = one(comps, "a number from 0 to 1")?;
            let v = match c {
                Comp::Word(w) => match split_num(w) {
                    Some((n, "")) => n,
                    Some((n, "%")) => n / 100.0,
                    _ => return Err(format!("expected a number from 0 to 1, got '{w}'")),
                },
                other => return Err(format!("expected a number from 0 to 1, got '{}'", other.text())),
            };
            if !(0.0..=1.0).contains(&v) {
                return Err("must be between 0 and 1".into());
            }
            vec![Longhand::Opacity(v)]
        }
        P::Border => return parse_border(None, comps, ctx),
        P::BorderTop => return parse_border(Some(Side::Top), comps, ctx),
        P::BorderRight => return parse_border(Some(Side::Right), comps, ctx),
        P::BorderBottom => return parse_border(Some(Side::Bottom), comps, ctx),
        P::BorderLeft => return parse_border(Some(Side::Left), comps, ctx),
        P::BorderWidth => {
            let v: Vec<Len> =
                no_commas(comps)?.iter().map(|c| len(c, ctx).and_then(nonneg)).collect::<Result<_, _>>()?;
            let e = expand4(&v, "lengths")?;
            SIDES.iter().zip(e).map(|(s, l)| Longhand::BorderWidth(*s, l)).collect()
        }
        P::BorderColor => {
            if comps.len() > 1 {
                return Err("a box has one border colour; give a single colour".into());
            }
            vec![Longhand::BorderColor(color(&comps[0])?)]
        }
        P::BorderRadius => {
            if value.contains('/') {
                return Err("elliptical radii ('/') are not supported".into());
            }
            let v: Vec<DimVal> = no_commas(comps)?
                .iter()
                .map(|c| {
                    dim(c, ctx).and_then(|d| {
                        if matches!(d, DimVal::Px(v) | DimVal::Em(v) | DimVal::Percent(v) if v < 0.0) {
                            Err("must not be negative".into())
                        } else {
                            Ok(d)
                        }
                    })
                })
                .collect::<Result<_, _>>()?;
            let e = expand4(&v, "lengths or percentages")?;
            CORNERS.iter().zip(e).map(|(c, d)| Longhand::Radius(*c, d)).collect()
        }
        P::BorderTopLeftRadius | P::BorderTopRightRadius | P::BorderBottomRightRadius | P::BorderBottomLeftRadius => {
            let corner = match info.id {
                P::BorderTopLeftRadius => Corner::Tl,
                P::BorderTopRightRadius => Corner::Tr,
                P::BorderBottomRightRadius => Corner::Br,
                _ => Corner::Bl,
            };
            let d = dim(one(comps, "a length or percentage")?, ctx)?;
            if matches!(d, DimVal::Px(v) | DimVal::Em(v) | DimVal::Percent(v) if v < 0.0) {
                return Err("must not be negative".into());
            }
            vec![Longhand::Radius(corner, d)]
        }
        P::Padding => {
            let v: Vec<Len> =
                no_commas(comps)?.iter().map(|c| len(c, ctx).and_then(nonneg)).collect::<Result<_, _>>()?;
            let e = expand4(&v, "lengths")?;
            SIDES.iter().zip(e).map(|(s, l)| Longhand::Padding(*s, l)).collect()
        }
        P::PaddingTop | P::PaddingRight | P::PaddingBottom | P::PaddingLeft => {
            let side = side_of(info.id);
            vec![Longhand::Padding(side, nonneg(len(one(comps, "a length")?, ctx)?)?)]
        }
        P::Margin => {
            let v: Vec<Option<Len>> =
                no_commas(comps)?.iter().map(|c| margin_value(c, ctx)).collect::<Result<_, _>>()?;
            let e = expand4(&v, "lengths or auto")?;
            SIDES.iter().zip(e).map(|(s, l)| Longhand::Margin(*s, l)).collect()
        }
        P::MarginTop | P::MarginRight | P::MarginBottom | P::MarginLeft => {
            vec![Longhand::Margin(side_of(info.id), margin_value(one(comps, "a length or auto")?, ctx)?)]
        }
        P::Gap => vec![Longhand::Gap(nonneg(len(one(comps, "one length")?, ctx)?)?)],
        P::FlexDirection => vec![Longhand::FlexDirection(match keyword_of(comps, &["row", "column"])?.as_str() {
            "row" => FlexDirection::Row,
            _ => FlexDirection::Column,
        })],
        P::FlexGrow | P::FlexShrink => {
            let n = number(one(comps, "a number")?)?;
            if n < 0.0 {
                return Err("must not be negative".into());
            }
            vec![if info.id == P::FlexGrow { Longhand::FlexGrow(n) } else { Longhand::FlexShrink(n) }]
        }
        P::JustifyContent => vec![Longhand::Justify(
            match keyword_of(
                comps,
                &["start", "end", "center", "space-between", "space-around", "space-evenly", "flex-start", "flex-end"],
            )?
            .as_str()
            {
                "start" | "flex-start" => Justify::Start,
                "end" | "flex-end" => Justify::End,
                "center" => Justify::Center,
                "space-between" => Justify::SpaceBetween,
                "space-around" => Justify::SpaceAround,
                _ => Justify::SpaceEvenly,
            },
        )],
        P::AlignItems => vec![Longhand::AlignItems(
            match keyword_of(comps, &["start", "end", "center", "stretch", "flex-start", "flex-end"])?.as_str() {
                "start" | "flex-start" => Align::Start,
                "end" | "flex-end" => Align::End,
                "center" => Align::Center,
                _ => Align::Stretch,
            },
        )],
        P::Width => return parse_size(SizeProp::Width, comps, ctx),
        P::Height => return parse_size(SizeProp::Height, comps, ctx),
        P::MinWidth => return parse_size(SizeProp::MinWidth, comps, ctx),
        P::MinHeight => return parse_size(SizeProp::MinHeight, comps, ctx),
        P::MaxWidth => return parse_size(SizeProp::MaxWidth, comps, ctx),
        P::MaxHeight => return parse_size(SizeProp::MaxHeight, comps, ctx),
        P::FontFamily => vec![Longhand::FontFamily(parse_family(comps)?)],
        P::FontSize => {
            let c = one(no_commas(comps)?, "a length or percentage")?;
            let d = dim(c, ctx)?;
            if matches!(d, DimVal::Auto) || matches!(d, DimVal::Px(v) | DimVal::Em(v) | DimVal::Percent(v) if v <= 0.0)
            {
                return Err("must be more than 0".into());
            }
            vec![Longhand::FontSize(d)]
        }
        P::FontWeight => {
            let c = one(comps, "normal, bold or 100 to 900")?;
            let w = match c.keyword().as_deref() {
                Some("normal") => 400,
                Some("bold") => 700,
                Some("lighter" | "bolder") => {
                    return Err("'lighter' and 'bolder' are not supported; use 100 to 900".into());
                }
                _ => {
                    let n = number(c)?;
                    if !(100.0..=900.0).contains(&n) || n.fract() != 0.0 {
                        return Err("must be a whole number from 100 to 900".into());
                    }
                    n as u16
                }
            };
            vec![Longhand::FontWeight(w)]
        }
        P::FontStyle => vec![Longhand::Italic(keyword_of(comps, &["normal", "italic"])? == "italic")],
        P::FontVariantNumeric => {
            vec![Longhand::TabularNums(keyword_of(comps, &["normal", "tabular-nums"])? == "tabular-nums")]
        }
        P::LetterSpacing => {
            let c = one(comps, "normal or a length")?;
            vec![Longhand::LetterSpacing(if c.keyword().as_deref() == Some("normal") {
                Len::Px(0.0)
            } else {
                len(c, ctx)?
            })]
        }
        P::TextAlign => {
            vec![Longhand::TextAlign(match keyword_of(comps, &["start", "center", "end", "left", "right"])?.as_str() {
                "center" => TextAlign::Center,
                "end" | "right" => TextAlign::End,
                _ => TextAlign::Start,
            })]
        }
        P::TextOverflow => vec![Longhand::Ellipsis(keyword_of(comps, &["clip", "ellipsis"])? == "ellipsis")],
        P::Overflow => vec![Longhand::Clip(keyword_of(comps, &["visible", "hidden"])? == "hidden")],
        P::BoxShadow => vec![parse_shadow(comps, ctx)?],
        P::Display => vec![Longhand::DisplayNone(keyword_of(comps, &["flex", "none"])? == "none")],
        P::Visibility => vec![Longhand::Visible(keyword_of(comps, &["visible", "hidden"])? == "visible")],
        P::Transition => return parse_transition(comps),
        P::TransitionProperty => {
            let mut v = Vec::new();
            if matches!(comps, [c] if c.keyword().as_deref() == Some("none")) {
                return Ok(vec![Longhand::TransitionProperty(v)]);
            }
            for g in comps.split(|c| *c == Comp::Comma) {
                let c = one(g, "a property name or all")?;
                match c {
                    Comp::Word(w) => v.push(transition_property(&w.to_ascii_lowercase())?),
                    other => return Err(format!("unexpected '{}'", other.text())),
                }
            }
            vec![Longhand::TransitionProperty(v)]
        }
        P::TransitionDuration | P::TransitionDelay => {
            let v: Vec<Duration> = comps
                .split(|c| *c == Comp::Comma)
                .map(|g| one(g, "a time like 150ms").and_then(time))
                .collect::<Result<_, _>>()?;
            vec![if info.id == P::TransitionDuration {
                Longhand::TransitionDuration(v)
            } else {
                Longhand::TransitionDelay(v)
            }]
        }
        P::TransitionTimingFunction => {
            let v: Vec<Easing> = comps
                .split(|c| *c == Comp::Comma)
                .map(|g| {
                    let c = one(g, "an easing")?;
                    easing(c).unwrap_or_else(|| Err(format!("unknown easing '{}'", c.text())))
                })
                .collect::<Result<_, _>>()?;
            vec![Longhand::TransitionTiming(v)]
        }
        P::Cursor => vec![Longhand::Cursor(match keyword_of(comps, &["default", "pointer", "auto"])?.as_str() {
            "pointer" => Cursor::Pointer,
            _ => Cursor::Default,
        })],
        P::Backdrop => vec![Longhand::Backdrop(match keyword_of(comps, &["none", "acrylic", "mica"])?.as_str() {
            "acrylic" => Backdrop::Acrylic,
            "mica" => Backdrop::Mica,
            _ => Backdrop::None,
        })],
    })
}

fn side_of(id: PropertyId) -> Side {
    match id {
        PropertyId::PaddingTop | PropertyId::MarginTop => Side::Top,
        PropertyId::PaddingRight | PropertyId::MarginRight => Side::Right,
        PropertyId::PaddingBottom | PropertyId::MarginBottom => Side::Bottom,
        _ => Side::Left,
    }
}

fn margin_value(c: &Comp, ctx: Ctx) -> Result<Option<Len>, String> {
    if c.keyword().as_deref() == Some("auto") { Ok(None) } else { len(c, ctx).map(Some) }
}

/// Why a name is not a property of the subset, when it is a known CSS
/// property people reach for.
pub(crate) fn unsupported_property(name: &str) -> Option<&'static str> {
    Some(match name {
        "transform" | "text-shadow" | "filter" | "backdrop-filter" | "clip-path" | "mask" => {
            "is not supported in this version"
        }
        "animation"
        | "animation-name"
        | "animation-duration"
        | "animation-delay"
        | "animation-timing-function"
        | "animation-iteration-count"
        | "animation-direction"
        | "animation-fill-mode" => "is not supported: animations come in a later version",
        "flex" | "flex-basis" | "flex-flow" => "is not supported; use flex-grow and flex-shrink",
        "flex-wrap"
        | "position"
        | "top"
        | "left"
        | "right"
        | "bottom"
        | "z-index"
        | "float"
        | "line-height"
        | "white-space"
        | "box-sizing"
        | "outline"
        | "text-decoration"
        | "text-transform"
        | "word-spacing"
        | "background-image"
        | "background-size"
        | "background-position"
        | "background-repeat"
        | "font"
        | "align-self"
        | "align-content"
        | "order"
        | "row-gap"
        | "column-gap"
        | "overflow-x"
        | "overflow-y"
        | "pointer-events"
        | "user-select" => "is not supported in this version",
        _ => return None,
    })
}

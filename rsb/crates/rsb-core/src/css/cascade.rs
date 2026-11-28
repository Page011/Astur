//! The cascade: which rules match a node, in what order, and what the
//! declarations make of its computed style. Custom properties and `var()`
//! are resolved here.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use super::parse::{Decl, DeclTarget, Keyframes, has_var, property_info};
use super::select::{MatchTree, Pseudo};
use super::value::{
    Bg, ColorVal, Corner, Ctx, DimVal, FamilyName, Len, Longhand, LpVal, ShadowVal, Side, SizeProp, matching_paren,
    parse_property_w, split_top,
};
use super::{BUILTIN_SHEETS, InlineDecls, MediaEnv, PropertyId, Stylesheet};
use crate::anim::Easing;
use crate::color::Color;
use crate::diag::{DiagKind, Diagnostic, SourcePos};
use crate::element::ElementKind;
use crate::geom::Edges;
use crate::icons::IconFont;
use crate::style::{
    AnimDirection, AnimFill, AnimationSpec, Backdrop, Background, ClipPolygon, ComputedStyle, Cursor, Dim,
    FlexDirection, KeyframeSet, KeyframeStyle, LengthPct, Overflow, PseudoStyle, RadiusDims, Shadow, Transform2,
    TransitionProperty, TransitionSpec,
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct VarDef {
    pub value: Arc<str>,
    pub pos: Option<SourcePos>,
    /// The custom property whose declaration the value was written in, when
    /// this one only passes it on (`--accent-text: var(--accent)`).
    pub origin: Option<Arc<str>>,
}

/// Custom properties visible at a node (inherited, resolved).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Vars {
    pub map: BTreeMap<Arc<str>, VarDef>,
}

pub(crate) struct StyleEnv {
    pub media: MediaEnv,
    pub icon_font: IconFont,
}

pub(crate) struct NodeInput<'a> {
    pub kind: ElementKind,
    pub parent: Option<&'a ComputedStyle>,
    pub parent_vars: &'a Arc<Vars>,
    pub inline: Option<&'a InlineDecls>,
    /// Initial `border-radius` (px, logical) before any rule: the bar's
    /// `radius` setting.
    pub initial_radius: Option<f32>,
    /// `rsb.column` starts a box as a column; CSS still overrides it.
    pub initial_direction: FlexDirection,
}

enum Lookup {
    Found(Arc<str>, Option<SourcePos>),
    Missing,
    Cycle,
}

enum VarErr {
    Undefined(String),
    Cycle(String),
    Syntax(String),
}

/// First variable a value used: its name and where it was defined.
type FirstVar = Option<(String, Option<SourcePos>)>;

fn substitute(
    raw: &str,
    lookup: &mut dyn FnMut(&str) -> Lookup,
    first: &mut FirstVar,
    depth: usize,
) -> Result<String, VarErr> {
    if depth > 16 {
        return Err(VarErr::Syntax("var() nests too deeply".into()));
    }
    let lower = raw.to_ascii_lowercase();
    let b = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            q @ (b'"' | b'\'') => {
                let start = i;
                i += 1;
                while i < b.len() && b[i] != q {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i = (i + 1).min(b.len());
                out.push_str(&raw[start..i]);
            }
            b'v' | b'V'
                if lower[i..].starts_with("var(")
                    && (i == 0 || !b[i - 1].is_ascii_alphanumeric() && b[i - 1] != b'-') =>
            {
                let open = i + 3;
                let close =
                    matching_paren(raw, open).ok_or_else(|| VarErr::Syntax("var( is missing its ')'".into()))?;
                let inner = &raw[open + 1..close];
                let parts = split_top(inner, b',');
                let name = parts[0].trim();
                if !name.starts_with("--") {
                    return Err(VarErr::Syntax(format!("var({name}) must name a custom property like --accent")));
                }
                let fallback = if parts.len() > 1 { Some(inner[parts[0].len() + 1..].trim()) } else { None };
                match lookup(name) {
                    Lookup::Found(v, pos) => {
                        if first.is_none() {
                            *first = Some((name.to_string(), pos));
                        }
                        out.push_str(&v);
                    }
                    Lookup::Cycle => return Err(VarErr::Cycle(name.to_string())),
                    Lookup::Missing => match fallback {
                        Some(f) => {
                            if first.is_none() {
                                *first = Some((name.to_string(), None));
                            }
                            out.push_str(&substitute(f, lookup, first, depth + 1)?);
                        }
                        None => return Err(VarErr::Undefined(name.to_string())),
                    },
                }
                i = close + 1;
            }
            _ => {
                let ch = raw[i..].chars().next().unwrap_or(' ');
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    Ok(out)
}

/// The rules that match `node`, in cascade order (specificity, then source order).
pub(crate) fn matching_decls<'a>(
    sheet: &'a Stylesheet,
    tree: &dyn MatchTree,
    node: usize,
    media: MediaEnv,
) -> Vec<&'a Decl> {
    let mut hits: Vec<(&(u32, u32, u32), usize)> = Vec::new();
    for (idx, r) in sheet.rules.iter().enumerate() {
        if r.media.is_some_and(|dark| dark != media.dark) || r.sel.pseudo().is_some() {
            continue;
        }
        if r.sel.matches(tree, node) {
            hits.push((&r.spec, idx));
        }
    }
    hits.sort_by_key(|(spec, idx)| (**spec, *idx));
    hits.iter().flat_map(|(_, idx)| sheet.rules[*idx].decls.iter()).collect()
}

fn px(l: Len, fs: f32) -> f32 {
    match l {
        Len::Px(v) => v,
        Len::Em(v) => v * fs,
    }
}

fn dim(d: DimVal, fs: f32) -> Dim {
    match d {
        DimVal::Auto => Dim::Auto,
        DimVal::Px(v) => Dim::Px(v),
        DimVal::Em(v) => Dim::Px(v * fs),
        DimVal::Percent(v) => Dim::Percent(v),
    }
}

#[derive(Default)]
struct Pending {
    border_color: Option<ColorVal>,
    shadows: Option<Vec<ShadowVal>>,
    tprops: Option<Vec<TransitionProperty>>,
    tdur: Option<Vec<Duration>>,
    tdelay: Option<Vec<Duration>>,
    ttiming: Option<Vec<Easing>>,
    aname: Option<Vec<Option<Arc<str>>>>,
    adur: Option<Vec<Duration>>,
    atiming: Option<Vec<Easing>>,
    adelay: Option<Vec<f64>>,
    acount: Option<Vec<f64>>,
    adir: Option<Vec<AnimDirection>>,
    afill: Option<Vec<AnimFill>>,
    apaused: Option<Vec<bool>>,
    outline_width: Option<Len>,
    outline_color: Option<ColorVal>,
    outline_offset: Option<Len>,
}

/// Where the last declaration that satisfies `pred` was written.
fn last_pos(decls: &[&Decl], pred: impl Fn(&Longhand) -> bool, props: &[PropertyId]) -> Option<SourcePos> {
    decls
        .iter()
        .rev()
        .find(|d| match &d.target {
            DeclTarget::Ready(ls) => ls.iter().any(&pred),
            DeclTarget::Deferred { prop, .. } => props.contains(prop),
            DeclTarget::Custom { .. } => false,
        })
        .and_then(|d| d.pos.clone())
}

/// Declarations as longhands, in cascade order, with `var()` substituted.
fn expand(decls: &[&Decl], vars: &Arc<Vars>, input: &NodeInput, diags: &mut Vec<Diagnostic>) -> Vec<Longhand> {
    let mut longhands: Vec<Longhand> = Vec::new();
    for d in decls {
        match &d.target {
            DeclTarget::Custom { .. } => {}
            DeclTarget::Ready(ls) => {
                for l in ls {
                    // `backdrop` belongs to the bar window; say where it was written.
                    if matches!(l, Longhand::Backdrop(_)) && input.kind != ElementKind::Bar {
                        diags.push(Diagnostic::warning(DiagKind::Style, d.pos.clone(), "backdrop only works on bar"));
                    } else {
                        longhands.push(l.clone());
                    }
                }
            }
            DeclTarget::Deferred { prop, raw, inline } => {
                let info = property_info(*prop);
                let mut first = None;
                let mut lk = |n: &str| match vars.map.get(n) {
                    Some(v) => Lookup::Found(v.value.clone(), v.pos.clone()),
                    None => Lookup::Missing,
                };
                let at = d.pos.clone();
                let warn = |diags: &mut Vec<Diagnostic>, m: String| {
                    diags.push(Diagnostic::warning(DiagKind::Style, at.clone(), m));
                };
                match substitute(raw, &mut lk, &mut first, 0) {
                    Ok(text) => {
                        let mut notes = Vec::new();
                        match parse_property_w(info, &text, Ctx { inline: *inline }, &mut notes) {
                            Ok(ls) => {
                                for n in notes {
                                    warn(diags, format!("'{}': {n}", info.name));
                                }
                                longhands.extend(ls)
                            }
                            Err(reason) => {
                                let (name, def) = first.clone().unwrap_or_default();
                                // Name the property the value was written in, not one that only passes it on.
                                let name = match vars.map.get(name.as_str()).and_then(|v| v.origin.clone()) {
                                    Some(origin) => origin.to_string(),
                                    None => name,
                                };
                                let builtin = at.as_ref().is_some_and(|p| BUILTIN_SHEETS.contains(&&*p.file));
                                match def {
                                    // The rule is the preset's, the value is the user's: point at the value.
                                    Some(user) if builtin => diags.push(Diagnostic::warning(
                                        DiagKind::Style,
                                        Some(user),
                                        format!("{name}: {reason} (the preset uses it for '{}')", info.name),
                                    )),
                                    Some(p) => warn(
                                        diags,
                                        format!(
                                            "'{}' is invalid after var({name}) (defined at {p}): {reason}",
                                            info.name
                                        ),
                                    ),
                                    None => {
                                        warn(diags, format!("'{}' is invalid after var({name}): {reason}", info.name))
                                    }
                                }
                                longhands.push(Longhand::Unset(*prop));
                            }
                        }
                    }
                    Err(VarErr::Undefined(n)) => {
                        warn(diags, format!("'{}' uses var({n}), which is not defined", info.name));
                        longhands.push(Longhand::Unset(*prop));
                    }
                    Err(VarErr::Cycle(n)) => {
                        warn(diags, format!("'{}' uses var({n}), which refers to itself", info.name));
                        longhands.push(Longhand::Unset(*prop));
                    }
                    Err(VarErr::Syntax(m)) => {
                        warn(diags, format!("bad value for '{}': {m}", info.name));
                        longhands.push(Longhand::Unset(*prop));
                    }
                }
            }
        }
    }
    longhands
}

/// Computes the style of `node`. Warnings (bad values after `var()`, a
/// backdrop outside the bar) go to `diags`.
pub(crate) fn compute(
    sheet: &Stylesheet,
    tree: &dyn MatchTree,
    node: usize,
    env: &StyleEnv,
    input: &NodeInput,
    diags: &mut Vec<Diagnostic>,
) -> (ComputedStyle, Arc<Vars>) {
    let mut decls = matching_decls(sheet, tree, node, env.media);
    if let Some(inline) = input.inline {
        decls.extend(inline.decls.iter());
    }

    // Custom properties first: every var() below may use them.
    let mut local: BTreeMap<Arc<str>, (Arc<str>, Option<SourcePos>)> = BTreeMap::new();
    for d in &decls {
        if let DeclTarget::Custom { name, raw } = &d.target {
            local.insert(name.clone(), (raw.clone(), d.pos.clone()));
        }
    }
    let vars = if local.is_empty() {
        input.parent_vars.clone()
    } else {
        Arc::new(resolve_locals(&local, input.parent_vars, diags))
    };

    // Expand to longhands in cascade order.
    let longhands = expand(&decls, &vars, input, diags);

    let mut style = apply(&longhands, env, input, diags, true);
    if !style.animations.is_empty() {
        attach_keyframes(&mut style, sheet, &longhands, &decls, &vars, env, input, diags);
    }
    style_warnings(&style, &decls, diags);
    if input.kind == ElementKind::Input && sheet.pseudo_rules {
        style.placeholder = pseudo_style(sheet, tree, node, Pseudo::Placeholder, &vars, input, diags);
        style.selection = pseudo_style(sheet, tree, node, Pseudo::Selection, &vars, input, diags);
    }
    (style, vars)
}

/// Looks up each animation's `@keyframes` and works out its frames: the
/// element's own style with each keyframe's declarations on top.
#[allow(clippy::too_many_arguments)]
fn attach_keyframes(
    style: &mut ComputedStyle,
    sheet: &Stylesheet,
    own: &[Longhand],
    decls: &[&Decl],
    vars: &Arc<Vars>,
    env: &StyleEnv,
    input: &NodeInput,
    diags: &mut Vec<Diagnostic>,
) {
    let name_pos =
        last_pos(decls, |l| matches!(l, Longhand::AnimName(_)), &[PropertyId::Animation, PropertyId::AnimationName]);
    for a in &mut style.animations {
        let Some(k) = sheet.keyframes_named(&a.name) else {
            diags.push(Diagnostic::warning(
                DiagKind::Style,
                name_pos.clone(),
                format!("unknown @keyframes '{}' in animation-name", a.name),
            ));
            continue;
        };
        a.frames = Some(Arc::new(keyframe_set(k, own, vars, env, input, diags)));
    }
}

fn keyframe_set(
    k: &Keyframes,
    own: &[Longhand],
    vars: &Arc<Vars>,
    env: &StyleEnv,
    input: &NodeInput,
    diags: &mut Vec<Diagnostic>,
) -> KeyframeSet {
    // Blocks at the same offset merge, later ones on top.
    let mut offsets: Vec<f32> = k.frames.iter().map(|f| f.offset).collect();
    offsets.dedup();
    for edge in [0.0, 1.0] {
        if !offsets.contains(&edge) {
            offsets.push(edge);
        }
    }
    offsets.sort_by(f32::total_cmp);
    let mut frames = Vec::new();
    for off in offsets {
        let mut lhs: Vec<Longhand> = own.to_vec();
        let mut easing = None;
        for f in k.frames.iter().filter(|f| f.offset == off) {
            let refs: Vec<&Decl> = f.decls.iter().collect();
            lhs.extend(expand(&refs, vars, input, diags));
            easing = f.easing.or(easing);
        }
        let style = apply(&lhs, env, input, diags, false);
        frames.push(KeyframeStyle { offset: off, easing, style: Arc::new(style) });
    }
    KeyframeSet { frames }
}

/// What a `::placeholder` or `::selection` rule sets on this node.
fn pseudo_style(
    sheet: &Stylesheet,
    tree: &dyn MatchTree,
    node: usize,
    which: Pseudo,
    vars: &Arc<Vars>,
    input: &NodeInput,
    diags: &mut Vec<Diagnostic>,
) -> PseudoStyle {
    let mut hits: Vec<(&(u32, u32, u32), usize)> = Vec::new();
    for (idx, r) in sheet.rules.iter().enumerate() {
        if r.sel.pseudo() == Some(which) && r.sel.matches(tree, node) {
            hits.push((&r.spec, idx));
        }
    }
    if hits.is_empty() {
        return PseudoStyle::default();
    }
    hits.sort_by_key(|(spec, idx)| (**spec, *idx));
    let decls: Vec<&Decl> = hits.iter().flat_map(|(_, idx)| sheet.rules[*idx].decls.iter()).collect();
    let mut out = PseudoStyle::default();
    let base = input.parent.map_or(Color::WHITE, |p| p.color);
    for l in expand(&decls, vars, input, diags) {
        match l {
            Longhand::Color(ColorVal::Color(c)) => out.color = Some(c),
            Longhand::Color(ColorVal::Current) => out.color = Some(base),
            Longhand::Background(Bg::Color(ColorVal::Color(c))) => out.background = Some(c),
            Longhand::Background(Bg::None) => out.background = Some(Color::TRANSPARENT),
            Longhand::Opacity(v) => out.opacity = Some(v),
            Longhand::Italic(b) => out.italic = Some(b),
            _ => {}
        }
    }
    out
}

/// Mistakes only the finished style shows.
fn style_warnings(s: &ComputedStyle, decls: &[&Decl], diags: &mut Vec<Diagnostic>) {
    if s.outline.width > 0.0 && s.clip_path.is_some() {
        let pos = last_pos(
            decls,
            |l| matches!(l, Longhand::OutlineWidth(_) | Longhand::OutlineColor(_)),
            &[PropertyId::Outline],
        );
        diags.push(Diagnostic::warning(
            DiagKind::Style,
            pos,
            "outline is cut by clip-path; use corner-shape: bevel, or show focus inside the shape",
        ));
    }
    let radius_px = |d: Dim| match d {
        Dim::Px(v) => v,
        Dim::Percent(p) => p,
        Dim::Auto => 0.0,
    };
    let r = s.border_radius;
    let radii = [r.tl, r.tr, r.br, r.bl].map(radius_px);
    if s.corner_shape.iter().zip(radii).any(|(c, r)| *c == crate::style::CornerShape::Bevel && r <= 0.0) {
        let pos = last_pos(decls, |l| matches!(l, Longhand::CornerShape(_)), &[PropertyId::CornerShape]);
        diags.push(Diagnostic::warning(DiagKind::Style, pos, "corner-shape bevel needs a border-radius to cut"));
    }
}

fn resolve_locals(
    local: &BTreeMap<Arc<str>, (Arc<str>, Option<SourcePos>)>,
    parent: &Arc<Vars>,
    diags: &mut Vec<Diagnostic>,
) -> Vars {
    let mut out = Vars { map: parent.map.clone() };
    let mut done: BTreeMap<Arc<str>, Option<VarDef>> = BTreeMap::new();
    for name in local.keys() {
        let mut stack = Vec::new();
        resolve_one(name, local, parent, &mut done, &mut stack, diags);
    }
    for (name, def) in done {
        match def {
            Some(d) => {
                out.map.insert(name, d);
            }
            None => {
                out.map.remove(&name);
            }
        }
    }
    out
}

fn resolve_one(
    name: &Arc<str>,
    local: &BTreeMap<Arc<str>, (Arc<str>, Option<SourcePos>)>,
    parent: &Arc<Vars>,
    done: &mut BTreeMap<Arc<str>, Option<VarDef>>,
    stack: &mut Vec<Arc<str>>,
    diags: &mut Vec<Diagnostic>,
) -> Option<VarDef> {
    if let Some(d) = done.get(name) {
        return d.clone();
    }
    let (raw, pos) = &local[name];
    stack.push(name.clone());
    let def = if has_var(raw) {
        let mut first = None;
        let res = {
            // The closure resolves other local names on demand.
            let mut lk = |n: &str| -> Lookup {
                if let Some((key, _)) = local.get_key_value(n) {
                    if stack.iter().any(|s| s.as_ref() == n) {
                        return Lookup::Cycle;
                    }
                    let key = key.clone();
                    return match resolve_one(&key, local, parent, done, stack, diags) {
                        Some(d) => Lookup::Found(d.value, d.pos),
                        None => Lookup::Missing,
                    };
                }
                match parent.map.get(n) {
                    Some(v) => Lookup::Found(v.value.clone(), v.pos.clone()),
                    None => Lookup::Missing,
                }
            };
            substitute(raw, &mut lk, &mut first, 0)
        };
        match res {
            // A value taken from another custom property is reported where that one was written, under its
            // name (a preset rule `--accent-text: var(--accent)` hands on the user's `--accent` and its line).
            Ok(v) => {
                let origin = first.as_ref().map(|(n, _)| {
                    let key: Arc<str> = Arc::from(n.as_str());
                    let carried = match done.get(&key) {
                        Some(d) => d.as_ref().and_then(|d| d.origin.clone()),
                        None => parent.map.get(&key).and_then(|d| d.origin.clone()),
                    };
                    carried.unwrap_or(key)
                });
                Some(VarDef {
                    value: Arc::from(v.trim()),
                    pos: first.as_ref().and_then(|(_, at)| at.clone()).or_else(|| pos.clone()),
                    origin,
                })
            }
            Err(e) => {
                let msg = match e {
                    VarErr::Undefined(n) => format!("'{name}' uses var({n}), which is not defined"),
                    VarErr::Cycle(n) => format!("'{name}' uses var({n}), which refers back to it"),
                    VarErr::Syntax(m) => format!("bad value for '{name}': {m}"),
                };
                diags.push(Diagnostic::warning(DiagKind::Style, pos.clone(), msg));
                None
            }
        }
    } else {
        Some(VarDef { value: raw.clone(), pos: pos.clone(), origin: None })
    };
    stack.pop();
    done.insert(name.clone(), def.clone());
    def
}

fn apply(
    lhs: &[Longhand],
    env: &StyleEnv,
    input: &NodeInput,
    diags: &mut Vec<Diagnostic>,
    with_animations: bool,
) -> ComputedStyle {
    let mut s = ComputedStyle::default();
    let parent = input.parent;
    if let Some(p) = parent {
        s.color = p.color;
        s.font_family = p.font_family.clone();
        s.font_size = p.font_size;
        s.font_weight = p.font_weight;
        s.italic = p.italic;
        s.letter_spacing = p.letter_spacing;
        s.tabular_nums = p.tabular_nums;
        s.text_align = p.text_align;
        s.visible = p.visible;
        s.cursor = p.cursor;
        s.scrollbar_width = p.scrollbar_width;
        s.scrollbar_color = p.scrollbar_color;
        s.caret_color = p.caret_color;
        s.accent_color = p.accent_color;
        s.pointer_events = p.pointer_events;
    }
    if input.kind == ElementKind::Icon {
        s.font_family = Arc::from(vec![Arc::<str>::from(env.icon_font.family())]);
    }
    s.flex_direction = input.initial_direction;
    if input.kind == ElementKind::Plate {
        // Colours of the engine's own plate; `plate` rules may replace them.
        s.color = Color::WHITE;
        s.background = Background::Color(Color::rgba8(0x9b, 0x1c, 0x1c, 255));
        s.border_radius = RadiusDims { tl: Dim::Px(4.0), tr: Dim::Px(4.0), br: Dim::Px(4.0), bl: Dim::Px(4.0) };
        s.padding = Edges { top: 2.0, right: 8.0, bottom: 2.0, left: 8.0 };
        s.font_size = 12.0;
        s.font_weight = 600;
    }
    if let Some(r) = input.initial_radius {
        s.border_radius = RadiusDims { tl: Dim::Px(r), tr: Dim::Px(r), br: Dim::Px(r), bl: Dim::Px(r) };
    }
    let parent_fs = parent.map_or(13.0, |p| p.font_size);
    let mut pend = Pending { border_color: Some(ColorVal::Current), ..Pending::default() };

    // Font size first: em lengths elsewhere use this element's own size.
    for l in lhs {
        match l {
            Longhand::FontSize(d) => {
                s.font_size = match d {
                    DimVal::Px(v) => *v,
                    DimVal::Em(v) => v * parent_fs,
                    DimVal::Percent(v) => v / 100.0 * parent_fs,
                    DimVal::Auto => parent_fs,
                }
            }
            Longhand::Unset(PropertyId::FontSize) => s.font_size = parent_fs,
            _ => {}
        }
    }
    let fs = s.font_size;
    for l in lhs {
        match l {
            Longhand::FontSize(_) => {}
            Longhand::Unset(id) => unset(&mut s, &mut pend, *id, parent),
            Longhand::Color(c) => {
                s.color = match c {
                    ColorVal::Color(c) => *c,
                    ColorVal::Current => parent.map_or(Color::WHITE, |p| p.color),
                }
            }
            Longhand::Background(b) => {
                s.background = match b {
                    Bg::None => Background::None,
                    Bg::Color(ColorVal::Color(c)) => Background::Color(*c),
                    Bg::Color(ColorVal::Current) => Background::Color(s.color),
                    Bg::Gradient(g) => Background::Linear(g.clone()),
                }
            }
            Longhand::Opacity(v) => s.opacity = *v,
            Longhand::BorderWidth(side, l) => set_side(&mut s.border_width, *side, px(*l, fs)),
            Longhand::BorderColor(c) => pend.border_color = Some(*c),
            Longhand::Radius(c, d) => {
                let d = dim(*d, fs);
                match c {
                    Corner::Tl => s.border_radius.tl = d,
                    Corner::Tr => s.border_radius.tr = d,
                    Corner::Br => s.border_radius.br = d,
                    Corner::Bl => s.border_radius.bl = d,
                }
            }
            Longhand::Padding(side, l) => set_side(&mut s.padding, *side, px(*l, fs)),
            Longhand::Margin(side, l) => set_side(&mut s.margin, *side, l.map(|l| px(l, fs))),
            Longhand::Gap(l) => s.gap = px(*l, fs),
            Longhand::FlexDirection(d) => s.flex_direction = *d,
            Longhand::FlexGrow(v) => s.flex_grow = *v,
            Longhand::FlexShrink(v) => s.flex_shrink = *v,
            Longhand::Justify(j) => s.justify_content = *j,
            Longhand::AlignItems(a) => s.align_items = *a,
            Longhand::Size(p, d) => {
                let d = dim(*d, fs);
                match p {
                    SizeProp::Width => s.width = d,
                    SizeProp::Height => s.height = d,
                    SizeProp::MinWidth => s.min_width = d,
                    SizeProp::MinHeight => s.min_height = d,
                    SizeProp::MaxWidth => s.max_width = d,
                    SizeProp::MaxHeight => s.max_height = d,
                }
            }
            Longhand::FontFamily(list) => s.font_family = map_families(list, env.icon_font),
            Longhand::FontWeight(w) => s.font_weight = *w,
            Longhand::Italic(b) => s.italic = *b,
            Longhand::TabularNums(b) => s.tabular_nums = *b,
            Longhand::LetterSpacing(l) => s.letter_spacing = px(*l, fs),
            Longhand::TextAlign(a) => s.text_align = *a,
            Longhand::Ellipsis(b) => s.ellipsis = *b,
            Longhand::OverflowX(o) => s.overflow_x = *o,
            Longhand::OverflowY(o) => s.overflow_y = *o,
            Longhand::BoxShadow(v) => pend.shadows = Some(v.clone()),
            Longhand::DisplayNone(b) => s.display_none = *b,
            Longhand::Visible(b) => s.visible = *b,
            Longhand::TransitionProperty(v) => pend.tprops = Some(v.clone()),
            Longhand::TransitionDuration(v) => pend.tdur = Some(v.clone()),
            Longhand::TransitionDelay(v) => pend.tdelay = Some(v.clone()),
            Longhand::TransitionTiming(v) => pend.ttiming = Some(v.clone()),
            Longhand::Cursor(c) => s.cursor = *c,
            Longhand::Backdrop(b) => {
                if input.kind == ElementKind::Bar {
                    s.backdrop = *b;
                } else {
                    diags.push(Diagnostic::warning(DiagKind::Style, None, "backdrop only works on bar"));
                }
            }
            Longhand::CornerShape(c) => s.corner_shape = *c,
            Longhand::ClipPath(c) => {
                s.clip_path = c.as_ref().map(|c| ClipPolygon {
                    points: c.points.iter().map(|(x, y)| (lengthpct(*x, fs), lengthpct(*y, fs))).collect(),
                    evenodd: c.evenodd,
                })
            }
            Longhand::Transform(t) => {
                s.transform = t.map_or(Transform2::IDENTITY, |t| Transform2 {
                    tx: lengthpct(t.tx, fs),
                    ty: lengthpct(t.ty, fs),
                    sx: t.sx,
                    sy: t.sy,
                })
            }
            Longhand::TransformOrigin(x, y) => s.transform_origin = (lengthpct(*x, fs), lengthpct(*y, fs)),
            Longhand::AnimName(v) => pend.aname = Some(v.clone()),
            Longhand::AnimDuration(v) => pend.adur = Some(v.clone()),
            Longhand::AnimTiming(v) => pend.atiming = Some(v.clone()),
            Longhand::AnimDelay(v) => pend.adelay = Some(v.clone()),
            Longhand::AnimIterations(v) => pend.acount = Some(v.clone()),
            Longhand::AnimDirection(v) => pend.adir = Some(v.clone()),
            Longhand::AnimFill(v) => pend.afill = Some(v.clone()),
            Longhand::AnimPaused(v) => pend.apaused = Some(v.clone()),
            Longhand::ScrollbarWidth(w) => s.scrollbar_width = *w,
            Longhand::ScrollbarColor(c) => {
                s.scrollbar_color = c.map(|(a, b)| (resolve_color(a, s.color), resolve_color(b, s.color)))
            }
            Longhand::CaretColor(c) => s.caret_color = c.map(|c| resolve_color(c, s.color)),
            Longhand::AccentColor(c) => s.accent_color = c.map(|c| resolve_color(c, s.color)),
            Longhand::OutlineWidth(w) => pend.outline_width = Some(*w),
            Longhand::OutlineColor(c) => pend.outline_color = Some(*c),
            Longhand::OutlineOffset(o) => pend.outline_offset = Some(*o),
            Longhand::PointerEvents(b) => s.pointer_events = *b,
        }
    }
    s.clip = s.overflow_x != Overflow::Visible || s.overflow_y != Overflow::Visible;
    if let Some(w) = pend.outline_width {
        s.outline.width = px(w, fs).max(0.0);
    }
    if let Some(o) = pend.outline_offset {
        s.outline.offset = px(o, fs);
    }

    // Values that follow the final text colour or font size.
    s.border_color = match pend.border_color {
        Some(ColorVal::Color(c)) => c,
        _ => s.color,
    };
    if let Some(list) = &pend.shadows {
        s.box_shadow = list
            .iter()
            .map(|v| Shadow {
                offset_x: px(v.x, fs),
                offset_y: px(v.y, fs),
                blur: px(v.blur, fs),
                spread: px(v.spread, fs),
                color: match v.color {
                    ColorVal::Color(c) => c,
                    ColorVal::Current => s.color,
                },
            })
            .collect();
    }
    s.outline.color = match pend.outline_color {
        Some(ColorVal::Color(c)) => c,
        _ => s.color,
    };
    s.transitions = build_transitions(&pend);
    if with_animations {
        s.animations = build_animations(&pend);
    }
    s
}

fn resolve_color(c: ColorVal, current: Color) -> Color {
    match c {
        ColorVal::Color(c) => c,
        ColorVal::Current => current,
    }
}

fn lengthpct(v: LpVal, fs: f32) -> LengthPct {
    LengthPct { px: v.px + v.em * fs, pct: v.pct }
}

fn cycle<T: Copy>(list: &Option<Vec<T>>, i: usize, default: T) -> T {
    match list {
        Some(l) if !l.is_empty() => l[i % l.len()],
        _ => default,
    }
}

fn build_animations(p: &Pending) -> Vec<AnimationSpec> {
    let Some(names) = p.aname.as_ref() else { return Vec::new() };
    let mut out = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let Some(name) = name else { continue };
        out.push(AnimationSpec {
            name: name.clone(),
            duration: cycle(&p.adur, i, Duration::ZERO),
            delay_s: cycle(&p.adelay, i, 0.0),
            easing: cycle(&p.atiming, i, Easing::Ease),
            iterations: cycle(&p.acount, i, 1.0),
            direction: cycle(&p.adir, i, AnimDirection::Normal),
            fill: cycle(&p.afill, i, AnimFill::None),
            paused: cycle(&p.apaused, i, false),
            frames: None,
        });
    }
    out
}

fn set_side<T>(e: &mut Edges<T>, side: Side, v: T) {
    match side {
        Side::Top => e.top = v,
        Side::Right => e.right = v,
        Side::Bottom => e.bottom = v,
        Side::Left => e.left = v,
    }
}

fn map_families(list: &[FamilyName], icon: IconFont) -> Arc<[Arc<str>]> {
    let mut out: Vec<Arc<str>> = Vec::new();
    for f in list {
        match f {
            FamilyName::Named(n) => out.push(n.clone()),
            FamilyName::Sans => out.push(Arc::from("Segoe UI")),
            FamilyName::Mono => {
                out.push(Arc::from("Cascadia Mono"));
                out.push(Arc::from("Consolas"));
            }
            FamilyName::Icons => out.push(Arc::from(icon.family())),
        }
    }
    Arc::from(out)
}

fn build_transitions(p: &Pending) -> Vec<TransitionSpec> {
    let Some(durs) = p.tdur.as_ref().filter(|d| !d.is_empty()) else { return Vec::new() };
    let default_props = [TransitionProperty::All];
    let props: &[TransitionProperty] = p.tprops.as_deref().unwrap_or(&default_props);
    let zero = [Duration::ZERO];
    let delays: &[Duration] = p.tdelay.as_deref().filter(|d| !d.is_empty()).unwrap_or(&zero);
    let ease = [Easing::Ease];
    let eases: &[Easing] = p.ttiming.as_deref().filter(|d| !d.is_empty()).unwrap_or(&ease);
    props
        .iter()
        .enumerate()
        .map(|(i, prop)| TransitionSpec {
            property: *prop,
            duration: durs[i % durs.len()],
            delay: delays[i % delays.len()],
            easing: eases[i % eases.len()],
        })
        .filter(|t| !(t.duration.is_zero() && t.delay.is_zero()))
        .collect()
}

fn unset(s: &mut ComputedStyle, pend: &mut Pending, id: PropertyId, parent: Option<&ComputedStyle>) {
    use PropertyId as P;
    let d = ComputedStyle::default();
    let inherit = |own: bool| own && parent.is_some();
    let _ = inherit;
    match id {
        P::Color => s.color = parent.map_or(d.color, |p| p.color),
        P::Background | P::BackgroundColor => s.background = Background::None,
        P::Opacity => s.opacity = 1.0,
        P::Border => {
            s.border_width = Edges::all(0.0);
            pend.border_color = Some(ColorVal::Current);
        }
        P::BorderWidth => s.border_width = Edges::all(0.0),
        P::BorderColor => pend.border_color = Some(ColorVal::Current),
        P::BorderTop => s.border_width.top = 0.0,
        P::BorderRight => s.border_width.right = 0.0,
        P::BorderBottom => s.border_width.bottom = 0.0,
        P::BorderLeft => s.border_width.left = 0.0,
        P::BorderRadius => s.border_radius = RadiusDims::ZERO,
        P::BorderTopLeftRadius => s.border_radius.tl = Dim::Px(0.0),
        P::BorderTopRightRadius => s.border_radius.tr = Dim::Px(0.0),
        P::BorderBottomRightRadius => s.border_radius.br = Dim::Px(0.0),
        P::BorderBottomLeftRadius => s.border_radius.bl = Dim::Px(0.0),
        P::Padding => s.padding = Edges::all(0.0),
        P::PaddingTop => s.padding.top = 0.0,
        P::PaddingRight => s.padding.right = 0.0,
        P::PaddingBottom => s.padding.bottom = 0.0,
        P::PaddingLeft => s.padding.left = 0.0,
        P::Margin => s.margin = Edges::all(Some(0.0)),
        P::MarginTop => s.margin.top = Some(0.0),
        P::MarginRight => s.margin.right = Some(0.0),
        P::MarginBottom => s.margin.bottom = Some(0.0),
        P::MarginLeft => s.margin.left = Some(0.0),
        P::Gap => s.gap = 0.0,
        P::FlexDirection => s.flex_direction = d.flex_direction,
        P::FlexGrow => s.flex_grow = d.flex_grow,
        P::FlexShrink => s.flex_shrink = d.flex_shrink,
        P::JustifyContent => s.justify_content = d.justify_content,
        P::AlignItems => s.align_items = d.align_items,
        P::Width => s.width = Dim::Auto,
        P::Height => s.height = Dim::Auto,
        P::MinWidth => s.min_width = Dim::Auto,
        P::MinHeight => s.min_height = Dim::Auto,
        P::MaxWidth => s.max_width = Dim::Auto,
        P::MaxHeight => s.max_height = Dim::Auto,
        P::FontFamily => s.font_family = parent.map_or(d.font_family, |p| p.font_family.clone()),
        P::FontSize => s.font_size = parent.map_or(d.font_size, |p| p.font_size),
        P::FontWeight => s.font_weight = parent.map_or(d.font_weight, |p| p.font_weight),
        P::FontStyle => s.italic = parent.is_some_and(|p| p.italic),
        P::FontVariantNumeric => s.tabular_nums = parent.is_some_and(|p| p.tabular_nums),
        P::LetterSpacing => s.letter_spacing = parent.map_or(0.0, |p| p.letter_spacing),
        P::TextAlign => s.text_align = parent.map_or(d.text_align, |p| p.text_align),
        P::TextOverflow => s.ellipsis = false,
        P::Overflow => {
            s.overflow_x = Overflow::Visible;
            s.overflow_y = Overflow::Visible;
        }
        P::OverflowX => s.overflow_x = Overflow::Visible,
        P::OverflowY => s.overflow_y = Overflow::Visible,
        P::CornerShape => s.corner_shape = d.corner_shape,
        P::ClipPath => s.clip_path = None,
        P::Transform => s.transform = Transform2::IDENTITY,
        P::TransformOrigin => s.transform_origin = d.transform_origin,
        P::Animation => {
            pend.aname = None;
            pend.adur = None;
            pend.atiming = None;
            pend.adelay = None;
            pend.acount = None;
            pend.adir = None;
            pend.afill = None;
            pend.apaused = None;
        }
        P::AnimationName => pend.aname = None,
        P::AnimationDuration => pend.adur = None,
        P::AnimationTimingFunction => pend.atiming = None,
        P::AnimationDelay => pend.adelay = None,
        P::AnimationIterationCount => pend.acount = None,
        P::AnimationDirection => pend.adir = None,
        P::AnimationFillMode => pend.afill = None,
        P::AnimationPlayState => pend.apaused = None,
        P::ScrollbarWidth => s.scrollbar_width = parent.map_or(d.scrollbar_width, |p| p.scrollbar_width),
        P::ScrollbarColor => s.scrollbar_color = parent.and_then(|p| p.scrollbar_color),
        P::CaretColor => s.caret_color = parent.and_then(|p| p.caret_color),
        P::AccentColor => s.accent_color = parent.and_then(|p| p.accent_color),
        P::Outline => {
            pend.outline_width = Some(Len::Px(0.0));
            pend.outline_color = None;
        }
        P::OutlineOffset => pend.outline_offset = Some(Len::Px(0.0)),
        P::PointerEvents => s.pointer_events = parent.is_none_or(|p| p.pointer_events),
        P::BoxShadow => pend.shadows = Some(Vec::new()),
        P::Display => s.display_none = false,
        P::Visibility => s.visible = parent.is_none_or(|p| p.visible),
        P::Transition => {
            pend.tprops = None;
            pend.tdur = None;
            pend.tdelay = None;
            pend.ttiming = None;
        }
        P::TransitionProperty => pend.tprops = None,
        P::TransitionDuration => pend.tdur = None,
        P::TransitionDelay => pend.tdelay = None,
        P::TransitionTimingFunction => pend.ttiming = None,
        P::Cursor => s.cursor = parent.map_or(Cursor::Default, |p| p.cursor),
        P::Backdrop => s.backdrop = Backdrop::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::Stylesheet;

    /// One node: an `input` with the class `q`.
    struct Input;

    impl MatchTree for Input {
        fn kind(&self, _: usize) -> ElementKind {
            ElementKind::Input
        }
        fn id(&self, _: usize) -> Option<&str> {
            None
        }
        fn has_class(&self, _: usize, class: &str) -> bool {
            class == "q"
        }
        fn hover(&self, _: usize) -> bool {
            false
        }
        fn active(&self, _: usize) -> bool {
            false
        }
        fn parent(&self, _: usize) -> Option<usize> {
            None
        }
    }

    fn style_of(css: &str) -> ComputedStyle {
        let (sheet, d) = Stylesheet::parse(css, "style.css");
        assert!(d.is_empty(), "{d:?}");
        let vars = Arc::new(Vars::default());
        let env = StyleEnv { media: MediaEnv { dark: true }, icon_font: IconFont::Fluent };
        let input = NodeInput {
            kind: ElementKind::Input,
            parent: None,
            parent_vars: &vars,
            inline: None,
            initial_radius: None,
            initial_direction: FlexDirection::Row,
        };
        let mut diags = Vec::new();
        compute(&sheet, &Input, 0, &env, &input, &mut diags).0
    }

    #[test]
    fn placeholder_and_selection_rules_style_an_input_without_touching_its_own_style() {
        let s = style_of(
            "input.q { color: #112233 } input.q::placeholder { color: #ff0000; opacity: 0.5; font-style: italic } \
             input::selection { background-color: #0000ff } input::selection { color: currentcolor }",
        );
        assert_eq!(s.color, Color::rgba8(0x11, 0x22, 0x33, 255), "a pseudo-element rule never styles the input");
        assert_eq!(s.opacity, 1.0);
        assert_eq!(
            s.placeholder,
            PseudoStyle {
                color: Some(Color::rgba8(255, 0, 0, 255)),
                background: None,
                opacity: Some(0.5),
                italic: Some(true)
            }
        );
        assert_eq!(s.selection.background, Some(Color::rgba8(0, 0, 255, 255)));
        assert!(s.selection.color.is_some(), "currentcolor resolves");
        // A rule that does not match leaves them empty.
        let s = style_of("input.other::placeholder { color: #ff0000 }");
        assert!(s.placeholder.is_empty() && s.selection.is_empty());
    }

    #[test]
    fn a_later_pseudo_rule_wins_at_equal_specificity_and_a_more_specific_one_always_does() {
        let s = style_of(
            "input::placeholder { opacity: 0.2 } input::placeholder { opacity: 0.3 } input.q::placeholder { opacity: 0.4 } input::placeholder { opacity: 0.9 }",
        );
        assert_eq!(s.placeholder.opacity, Some(0.4));
    }
}

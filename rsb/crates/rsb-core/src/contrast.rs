//! Contrast: one module for the checks that tests, `settings presets`,
//! `settings get` and `settings set color.accent` all rely on.
//!
//! Colours are read from the real style sheet cascade, never with a text
//! search. Pairs come from a preset's `[about.contrast]` lists. Translucent
//! layers are composited over black and over white and the worse result is
//! kept, as the draw-list checks do.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::color::Color;
use crate::contract::{AccentSchemes, AccentSet, AccentTokens, Lowest, SchemeContrast};
use crate::css::cascade::{self, NodeInput, StyleEnv};
use crate::css::select::MatchTree;
use crate::css::{BASE_SHEET, MediaEnv, PRESET_SHEET, Stylesheet};
use crate::diag::SourcePos;
use crate::draw::{DrawCmd, DrawList, Fill};
use crate::element::ElementKind;
use crate::geom::{PathEl, PathF, PointF, RectF};
use crate::icons::IconFont;
use crate::preset::{About, Scheme, split_pair_line, surface_layers};
use crate::style::FlexDirection;

/// Which sheet of the chain a declaration came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// The base theme.
    Base,
    /// The preset's sheet.
    Preset,
    /// `style.css` or a file it imports.
    User,
}

/// Where a custom property got its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub pos: SourcePos,
    pub layer: Layer,
}

/// The custom properties on `:root` that hold a colour in one scheme,
/// without the `--`, with the declaration that won.
pub type Vars = BTreeMap<String, (Color, Origin)>;

/// What a pair is measured against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairKind {
    /// Text: 4.5:1.
    Text,
    /// An icon, large text or the off state: 3:1.
    Icon,
    /// A ring, outline or progress bar: 3:1.
    Line,
}

impl PairKind {
    /// The ratio a pair of this kind must reach.
    pub fn need(self) -> f64 {
        match self {
            PairKind::Text => 4.5,
            PairKind::Icon | PairKind::Line => 3.0,
        }
    }
}

/// One measured pair.
#[derive(Clone, Debug, PartialEq)]
pub struct PairResult {
    pub kind: PairKind,
    /// The foreground's name, e.g. `fg`.
    pub fg: String,
    /// The surface as written, e.g. `bg+pill`.
    pub surface: String,
    pub ratio: f64,
    pub need: f64,
}

impl PairResult {
    pub fn passes(&self) -> bool {
        self.ratio >= self.need
    }

    /// `fg on surface`, the text a result is shown with.
    pub fn what(&self) -> String {
        format!("{} on {}", self.fg, self.surface)
    }
}

/// The backgrounds an accent token must stay readable on in one scheme: the
/// surfaces of every pair whose foreground is the token, over all the
/// built-in presets. A translucent surface is listed twice, composited over
/// black and over white.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SchemeSurfaces {
    /// Surfaces of the `text` and `icon` pairs of `accent-text` (4.5:1).
    pub text: Vec<Color>,
    /// Surfaces of the `line` pairs of `accent-line` (3:1).
    pub line: Vec<Color>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AccentSurfaces {
    pub dark: SchemeSurfaces,
    pub light: SchemeSurfaces,
}

impl AccentSurfaces {
    pub fn scheme_mut(&mut self, scheme: Scheme) -> &mut SchemeSurfaces {
        match scheme {
            Scheme::Dark => &mut self.dark,
            Scheme::Light => &mut self.light,
        }
    }
}

/// One text or icon item of a draw list and its measured ratio.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawContrast {
    /// The text, or `icon <key>`.
    pub what: String,
    pub ratio: f32,
    pub need: f32,
    /// The text or icon colour.
    pub fg: Color,
    /// What the fills under it add up to over black and over white.
    pub under: [Color; 2],
}

/// The ratio an item drawn in the off-state colour (`--dim`) has to reach:
/// an icon or disabled label, not running text.
pub const OFF_STATE_NEED: f32 = 3.0;
/// The two `--dim` values of the default preset, dark and light.
const OFF_STATE_COLORS: [u32; 2] = [0x6d_7477, 0x6b_7378];

// ---------------------------------------------------------------- variables

/// One node: the bar. `:root` and `bar` are the same selector.
struct RootTree;

impl MatchTree for RootTree {
    fn kind(&self, _node: usize) -> ElementKind {
        ElementKind::Bar
    }
    fn id(&self, _node: usize) -> Option<&str> {
        None
    }
    fn has_class(&self, _node: usize, _class: &str) -> bool {
        false
    }
    fn hover(&self, _node: usize) -> bool {
        false
    }
    fn active(&self, _node: usize) -> bool {
        false
    }
    fn parent(&self, _node: usize) -> Option<usize> {
        None
    }
}

fn layer_of(file: &str) -> Layer {
    match file {
        BASE_SHEET => Layer::Base,
        PRESET_SHEET => Layer::Preset,
        _ => Layer::User,
    }
}

/// The custom properties on `:root` of `chain` that resolve to a colour in
/// `scheme`, through the real parser and cascade (the scheme sets the
/// `@media (prefers-color-scheme)` environment). A property that holds
/// something else (`--d: 13px`) is left out; a `var()` hand-over reports the
/// place of the colour it passes on.
pub fn resolve_vars(chain: &Stylesheet, scheme: Scheme) -> Vars {
    let env = StyleEnv { media: MediaEnv { dark: scheme == Scheme::Dark }, icon_font: IconFont::Fluent };
    let parent = Arc::new(cascade::Vars::default());
    let input = NodeInput {
        kind: ElementKind::Bar,
        parent: None,
        parent_vars: &parent,
        inline: None,
        initial_radius: None,
        initial_direction: FlexDirection::Row,
    };
    let mut diags = Vec::new();
    let (_, vars) = cascade::compute(chain, &RootTree, 0, &env, &input, &mut diags);
    vars.map
        .iter()
        .filter_map(|(name, def)| {
            let color = Color::parse(&def.value).ok()?;
            let pos = def.pos.clone().unwrap_or_else(|| SourcePos::new("", 0, 0));
            let layer = layer_of(&pos.file);
            let name = name.strip_prefix("--").unwrap_or(name).to_string();
            Some((name, (color, Origin { pos, layer })))
        })
        .collect()
}

/// Names `about` relies on that `vars` does not define in this scheme: the
/// `tune` variables and every name in the contrast lists. Empty = all defined.
pub fn undefined_names(about: &About, vars: &Vars) -> Vec<String> {
    let mut missing: Vec<String> = Vec::new();
    let mut need = |name: &str| {
        if !vars.contains_key(name) && !missing.iter().any(|m| m == name) {
            missing.push(name.to_string());
        }
    };
    for t in &about.tune {
        need(&t.var);
    }
    let c = &about.contrast;
    for line in c.text.iter().chain(&c.icon).chain(&c.line) {
        if let Some((fg, surfaces)) = split_pair_line(line) {
            need(fg);
            for s in surfaces {
                for layer in surface_layers(s).unwrap_or_default() {
                    need(layer);
                }
            }
        }
    }
    for name in &c.decorative {
        need(name);
    }
    missing
}

// ---------------------------------------------------------------- pairs

/// `layers` (bottom first) composited over black and over white.
fn stack_over(vars: &Vars, surface: &str) -> Option<[Color; 2]> {
    let layers = surface_layers(surface)?;
    let mut out = [Color::BLACK, Color::WHITE];
    for name in layers {
        let c = vars.get(name)?.0;
        for bg in &mut out {
            *bg = c.over(*bg);
        }
    }
    Some(out)
}

/// The colours a surface adds up to over black and over white (equal for an
/// opaque surface). None when a name is not defined.
pub fn surface_colors(vars: &Vars, surface: &str) -> Option<[Color; 2]> {
    stack_over(vars, surface)
}

/// The worse of the ratios over black and over white.
fn pair_ratio(fg: Color, under: [Color; 2]) -> f64 {
    under.iter().map(|bg| f64::from(Color::contrast_ratio(fg.over(*bg), *bg))).fold(f64::MAX, f64::min)
}

/// Measures every pair of `about.contrast`. A name that does not resolve in
/// `vars` is a result with ratio 0, so it fails visibly instead of vanishing.
pub fn pairs(about: &About, vars: &Vars) -> Vec<PairResult> {
    let mut out = Vec::new();
    let lists = [
        (PairKind::Text, &about.contrast.text),
        (PairKind::Icon, &about.contrast.icon),
        (PairKind::Line, &about.contrast.line),
    ];
    for (kind, lines) in lists {
        for line in lines {
            let Some((fg, surfaces)) = split_pair_line(line) else {
                out.push(PairResult { kind, fg: line.clone(), surface: String::new(), ratio: 0.0, need: kind.need() });
                continue;
            };
            let fg_color = vars.get(fg).map(|v| v.0);
            for surface in surfaces {
                let ratio = match (fg_color, stack_over(vars, surface)) {
                    (Some(f), Some(under)) => pair_ratio(f, under),
                    _ => 0.0,
                };
                out.push(PairResult {
                    kind,
                    fg: fg.to_string(),
                    surface: surface.to_string(),
                    ratio,
                    need: kind.need(),
                });
            }
        }
    }
    out
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn lowest_of<'a>(results: impl Iterator<Item = &'a PairResult>, need: f64) -> Lowest {
    match results.min_by(|a, b| a.ratio.total_cmp(&b.ratio)) {
        Some(r) => Lowest { lowest: round2(r.ratio), what: r.what(), need },
        // Nothing to measure: the highest ratio there is, so nothing reads as low.
        None => Lowest { lowest: 21.0, what: "none".to_string(), need },
    }
}

/// The lowest text and graphic (icon and line) ratios of one scheme, rounded
/// to two places, and whether every pair passes.
pub fn summary(pairs: &[PairResult]) -> SchemeContrast {
    let text = lowest_of(pairs.iter().filter(|p| p.kind == PairKind::Text), PairKind::Text.need());
    let graphic = lowest_of(pairs.iter().filter(|p| p.kind != PairKind::Text), PairKind::Icon.need());
    SchemeContrast { pass: pairs.iter().all(PairResult::passes), text, graphic }
}

// ---------------------------------------------------------------- accent

/// The channels rounded to 8 bits, opaque: the colour a file can hold.
fn quantise(c: Color) -> Color {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() / 255.0;
    Color { r: q(c.r), g: q(c.g), b: q(c.b), a: 1.0 }
}

/// `#rrggbb`, lower case.
pub fn hex(c: Color) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c.r), b(c.g), b(c.b))
}

fn ratio_on(fg: Color, bg: Color) -> f64 {
    f64::from(Color::contrast_ratio(fg, bg))
}

/// The smallest mix of `accent` toward `toward`, in 2% steps, that reaches
/// `need` on every surface; the mix in percent. Pure `toward` when no mix does.
fn nudge(accent: Color, toward: Color, surfaces: &[Color], need: f64) -> (Color, u32) {
    for step in 0..=50u32 {
        let c = quantise(accent.lerp(toward, step as f32 * 0.02));
        if surfaces.iter().all(|s| ratio_on(c, *s) >= need) {
            return (c, step * 2);
        }
    }
    (quantise(toward), 100)
}

fn derive_scheme(accent: Color, surfaces: &SchemeSurfaces, toward: Color) -> AccentTokens {
    let (on_black, on_white) = (ratio_on(accent, Color::BLACK), ratio_on(accent, Color::WHITE));
    let fg = if on_black >= on_white { Color::BLACK } else { Color::WHITE };
    let away = if fg == Color::BLACK { Color::WHITE } else { Color::BLACK };
    let hover = quantise(accent.lerp(away, 0.10));
    let press = quantise(accent.lerp(away, 0.20));
    let fg_ratio = [accent, hover, press].iter().map(|f| ratio_on(fg, *f)).fold(f64::MAX, f64::min);
    let (text, moved_text) = nudge(accent, toward, &surfaces.text, PairKind::Text.need());
    let (line, moved_line) = nudge(accent, toward, &surfaces.line, PairKind::Line.need());
    AccentTokens {
        fg: hex(fg),
        fg_ratio: round2(fg_ratio),
        hover: hex(hover),
        press: hex(press),
        text: hex(text),
        line: hex(line),
        moved: moved_text.max(moved_line),
    }
}

/// The accent's companion colours for `accent` in both schemes, readable on
/// every surface in `surfaces`: `fg` black or white (the better ratio on the
/// accent), `hover` and `press` 10% and 20% away from `fg`, `text` and `line`
/// the smallest step toward white (dark) or black (light) that passes.
/// Any alpha of `accent` is dropped.
pub fn derive_accent(accent: Color, surfaces: &AccentSurfaces) -> AccentSet {
    let accent = quantise(accent);
    AccentSet {
        value: hex(accent),
        schemes: AccentSchemes {
            dark: derive_scheme(accent, &surfaces.dark, Color::WHITE),
            light: derive_scheme(accent, &surfaces.light, Color::BLACK),
        },
    }
}

/// Adds the surfaces `accent-text` and `accent-line` sit on in `about`'s
/// pairs to `into` for `scheme`, as `vars` resolves them: the union over the
/// presets is what an accent has to stay readable on.
pub fn collect_accent_surfaces(about: &About, vars: &Vars, scheme: Scheme, into: &mut AccentSurfaces) {
    let target = into.scheme_mut(scheme);
    let lists = [(&about.contrast.text, true), (&about.contrast.icon, true), (&about.contrast.line, false)];
    for (lines, is_text) in lists {
        let token = if is_text { "accent-text" } else { "accent-line" };
        for line in lines {
            let Some((fg, surfaces)) = split_pair_line(line) else { continue };
            if fg != token {
                continue;
            }
            for surface in surfaces {
                let Some(under) = stack_over(vars, surface) else { continue };
                let list = if is_text { &mut target.text } else { &mut target.line };
                for c in under {
                    if !list.contains(&c) {
                        list.push(c);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- draw lists

fn polygon_points(path: &PathF) -> Vec<PointF> {
    path.els
        .iter()
        .filter_map(|e| match e {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => Some(*p),
            _ => None,
        })
        .collect()
}

/// `p` inside the polygon (even-odd).
fn in_polygon(p: PointF, pts: &[PointF]) -> bool {
    let mut inside = false;
    let mut j = pts.len().wrapping_sub(1);
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn is_off_state(c: Color) -> bool {
    OFF_STATE_COLORS.iter().any(|h| c == Color::rgba8((h >> 16) as u8, (h >> 8) as u8, *h as u8, 255))
}

/// Every text and icon item of `draw` (inside `within` when given) against
/// the fills drawn under it: text needs 4.5:1, an icon from the SVG set 3:1,
/// the off-state colour 3:1. Translucent fills are composited over black and
/// over white and the worse result counts. Items in a group that is fully
/// transparent are not on screen and are skipped. A gradient behind an item,
/// or a group half way through a fade, is an error: contrast is not measured
/// for it, so the caller settles the frame first.
pub fn of_draw_list(draw: &DrawList, within: Option<RectF>) -> Result<Vec<DrawContrast>, String> {
    let mut out = Vec::new();
    let mut open: Vec<f32> = Vec::new();
    for (i, item) in draw.items.iter().enumerate() {
        match &item.cmd {
            DrawCmd::PushOpacity(a) => {
                open.push(*a);
                continue;
            }
            DrawCmd::PopOpacity => {
                open.pop();
                continue;
            }
            _ => {}
        }
        let (rect, label, color, need) = match &item.cmd {
            DrawCmd::Text { rect, text, color, .. } if !text.trim().is_empty() => {
                (*rect, text.to_string(), *color, 4.5)
            }
            DrawCmd::Svg { rect, key, tint: Some(color) } => (*rect, format!("icon {key}"), *color, 3.0),
            _ => continue,
        };
        if let Some(w) = within
            && !(rect.x >= w.x - 0.5
                && rect.right() <= w.right() + 0.5
                && rect.y >= w.y - 0.5
                && rect.bottom() <= w.bottom() + 0.5)
        {
            continue;
        }
        if open.iter().any(|a| *a <= 0.001) {
            continue;
        }
        if open.iter().any(|a| *a < 0.999) {
            return Err(format!("{label}: group opacity half way, contrast is not measured"));
        }
        let centre = PointF { x: rect.x + rect.w / 2.0, y: rect.y + rect.h / 2.0 };
        let mut under = [Color::BLACK, Color::WHITE];
        for prev in &draw.items[..i] {
            let solid = match &prev.cmd {
                DrawCmd::Fill { rect, fill, .. } if rect.contains(centre) => Some(fill),
                DrawCmd::FillPath { path, fill } if in_polygon(centre, &polygon_points(path)) => Some(fill),
                _ => None,
            };
            match solid {
                Some(Fill::Solid(c)) => {
                    for bg in &mut under {
                        *bg = c.over(*bg);
                    }
                }
                Some(Fill::Linear { .. }) => {
                    return Err(format!("{label}: a gradient behind it, contrast is not measured"));
                }
                None => {}
            }
        }
        let ratio = under.iter().map(|bg| Color::contrast_ratio(color.over(*bg), *bg)).fold(f32::MAX, f32::min);
        let need = if is_off_state(color) { OFF_STATE_NEED } else { need };
        out.push(DrawContrast { what: label, ratio, need, fg: color, under });
    }
    Ok(out)
}

/// One token pair that a draw list also measured, and what each says.
#[derive(Clone, Debug, PartialEq)]
pub struct Agreement {
    /// `fg on surface`, as the pair is named.
    pub pair: String,
    /// The ratio from the colours of the sheet.
    pub token: f64,
    /// The ratio measured on the drawn bar for an item of the same colour on
    /// the same fills.
    pub drawn: f32,
    /// What the item showed.
    pub item: String,
}

/// The same colour up to the rounding of a translucent layer's alpha to 8
/// bits, which the drawn bar does and the sheet's colours do not (up to
/// about 0.0015 over three layers). One step of a channel, a 255th (0.0039),
/// is a different colour.
fn same_color(a: Color, b: Color) -> bool {
    let near = |x: f32, y: f32| (x - y).abs() < 3e-3;
    near(a.r, b.r) && near(a.g, b.g) && near(a.b, b.b) && near(a.a, b.a)
}

/// Every (pair, item) where an item of `items` has the pair's foreground on
/// fills that add up to the pair's surface. The two ratios of each match are
/// measured by different routes (the sheet's colours, the drawn fills), so a
/// difference between them is a fault in one of the two.
pub fn compare_with_draw(about: &About, vars: &Vars, items: &[DrawContrast]) -> Vec<Agreement> {
    let mut out = Vec::new();
    for p in pairs(about, vars) {
        let Some(fg) = vars.get(&p.fg).map(|v| v.0) else { continue };
        let Some(under) = stack_over(vars, &p.surface) else { continue };
        for item in items {
            if same_color(item.fg, fg) && same_color(item.under[0], under[0]) && same_color(item.under[1], under[1]) {
                out.push(Agreement { pair: p.what(), token: p.ratio, drawn: item.ratio, item: item.what.clone() });
            }
        }
    }
    out
}

//! One bar's scene: widget trees in, a draw list and a damage rectangle out.
//!
//! The bar is a fixed skeleton the scene builds itself:
//!
//! ```text
//! bar.<bar name>                      the bar root (type `bar`)
//!   zone#left / zone#center / zone#right
//!     widget.<class>#<id>             one per widget ref in the zone
//!       <the widget's own tree>       or a `plate` when the widget failed
//! ```
//!
//! A bar plate (config or style error) is drawn as the first child of
//! `zone#left`. Zones: left starts at the bar's start, right ends at the
//! bar's end, center is centred on the bar and only shifted as far as it
//! must be to avoid overlapping the other two.
//!
//! Engine rules no theme can change:
//! - The bar's border box (with the bar's radii) is always painted with at
//!   least alpha 1/255, as the first draw item: a layered window passes
//!   clicks through fully transparent pixels, so without it hover and the
//!   right-click menu would fall through gaps in a transparent bar.
//!   Floating margins stay fully transparent.
//! - Plates take colour, background, border and font family only from
//!   `plate` rules; `display`, `visibility` and `opacity` are ignored for
//!   them and their font size is at least 9 px, so no theme can hide an
//!   error.
//! - A widget tree deeper than `element::MAX_DEPTH` is not laid out: the
//!   widget shows a plate instead.
//! - Boxes are always border-box sized (`width` includes padding and border).
//!
//! Work is done lazily in `frame`: restyle only nodes whose tree, classes,
//! hover/active state or matching rules changed; relayout only when a size
//! can have changed; rebuild the draw list; diff it for damage. Nothing
//! changed and no transition running = `FrameOut::dirty` is None.
//!
//! A floating bar keeps `margin` from the left and right edges of the
//! surface and from the screen edge it sits on (the surface is as tall as
//! the bar plus that margin); the margin is fully transparent.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use crate::color::Color;
use crate::config::{BarSpec, Position};
use crate::css::cascade::Vars;
use crate::css::{MediaEnv, Stylesheet};
use crate::diag::{Diagnostic, Plate};
use crate::draw::{DrawList, TextStyle, damage};
use crate::element::{ElementKind, Handlers, IconRef, ImageRef, InstanceId, NodeKey, WidgetTree};
use crate::geom::{Edges, RectF, RectPx};
use crate::hit::{PointerEvent, PointerResult};
use crate::icons::IconFont;
use crate::layout::TextMeasure;
use crate::style::{Backdrop, ComputedStyle, Cursor};

mod build;
mod input;
mod layout;
mod paint;
mod trans;

/// Size of the bar surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarGeometry {
    pub width_px: u32,
    pub height_px: u32,
    /// DPI / 96.
    pub scale: f32,
}

/// Things outside the trees that change the look.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneEnv {
    pub media: MediaEnv,
    /// Which icon font the renderer found (Segoe Fluent Icons on Windows 11,
    /// Segoe MDL2 Assets on Windows 10).
    pub icon_font: IconFont,
    /// A fullscreen app is in front of this bar's monitor and the bar's
    /// mode is `stay`: the bar element gets the class `fullscreen`.
    pub fullscreen: bool,
}

/// A warning the scene found while styling or laying out, with the widget
/// instance it belongs to (None = the bar or the style sheet).
#[derive(Clone, Debug, PartialEq)]
pub struct SceneDiagnostic {
    pub instance: Option<InstanceId>,
    pub diagnostic: Diagnostic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Left,
    Center,
    Right,
}

/// The widget instances of each zone, in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ZoneSlots {
    pub left: Vec<SlotSpec>,
    pub center: Vec<SlotSpec>,
    pub right: Vec<SlotSpec>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SlotSpec {
    pub instance: InstanceId,
    /// `WidgetRef::css_class()`.
    pub class: String,
    /// `WidgetRef::css_id()`.
    pub id: String,
}

#[derive(Clone, Debug, Default)]
pub struct FrameStats {
    pub nodes: usize,
    pub restyled: usize,
    pub relayout: bool,
    pub style_us: u32,
    pub layout_us: u32,
    pub draw_us: u32,
    pub running_transitions: usize,
}

#[derive(Clone, Debug)]
pub struct FrameOut {
    pub draw: Arc<DrawList>,
    /// Pixels to redraw and present; None = nothing visible changed.
    pub dirty: Option<RectPx>,
    /// When the next frame is due (a transition is running or has a delay
    /// pending); None = no frame until something else changes.
    pub next_frame: Option<Instant>,
    pub cursor: Cursor,
    /// Computed `backdrop` of the bar element.
    pub backdrop: Backdrop,
    pub stats: FrameStats,
}

/// A node as the scene sees it, for tests and the future inspector.
#[derive(Clone, Debug)]
pub struct NodeInfo {
    pub key: NodeKey,
    pub kind: ElementKind,
    pub instance: Option<InstanceId>,
    pub depth: u32,
    pub id: Option<Arc<str>>,
    pub classes: Vec<Arc<str>>,
    pub text: Option<Arc<str>>,
    /// Border box in physical px.
    pub rect: RectF,
    pub style: ComputedStyle,
}

/// Where a node ended up after layout.
#[derive(Clone, Debug, Default)]
struct Laid {
    /// Border box, physical px, bar-relative.
    rect: RectF,
    /// Border and padding widths as laid out (physical px).
    border: Edges<f32>,
    padding: Edges<f32>,
    /// Text as drawn (ellipsis applied).
    text: Option<Arc<str>>,
    /// A zone that was shrunk to fit: its content is clipped.
    clip: bool,
}

/// One node of the laid-out bar.
struct Node {
    key: NodeKey,
    kind: ElementKind,
    instance: Option<InstanceId>,
    parent: Option<usize>,
    children: Vec<usize>,
    depth: u32,
    id: Option<Arc<str>>,
    classes: Vec<Arc<str>>,
    /// Text content before layout (text, icon glyph or plate line).
    text: Option<Arc<str>>,
    image: Option<ImageRef>,
    handlers: Handlers,
    /// Style the rules gave (what transitions run towards).
    target: Arc<ComputedStyle>,
    /// Style drawn right now: the target with running transitions applied.
    style: Arc<ComputedStyle>,
    vars: Arc<Vars>,
    /// A running transition or an animating ancestor.
    animating: bool,
    #[allow(dead_code)]
    sig: u64,
    base_sig: u64,
    laid: Laid,
}

/// What a node was last frame, for transitions and for relayout decisions.
struct Prev {
    target: Arc<ComputedStyle>,
    display: Arc<ComputedStyle>,
    base_sig: u64,
}

struct Pressed {
    button: crate::hit::Button,
    target: Option<NodeKey>,
    plate: bool,
}

pub struct Scene {
    bar: BarSpec,
    geometry: BarGeometry,
    env: SceneEnv,
    sheet: Arc<Stylesheet>,
    slots: ZoneSlots,
    trees: HashMap<InstanceId, Arc<WidgetTree>>,
    widget_plates: HashMap<InstanceId, Plate>,
    bar_plate: Option<Plate>,
    image_sizes: HashMap<String, (u32, u32)>,
    image_failed: HashSet<String>,
    /// Image keys whose nodes must be redrawn even if the draw list is equal.
    image_touched: Vec<String>,

    hover: HashSet<NodeKey>,
    active: HashSet<NodeKey>,
    /// Hover chain, root first, as of the last pointer event.
    hover_chain: Vec<NodeKey>,
    pressed: Option<Pressed>,
    cursor: Cursor,
    /// When the pointer last changed hover or active state: transitions that
    /// causes start from then.
    state_time: Option<Instant>,

    arena: Vec<Node>,
    prev: HashMap<NodeKey, Prev>,
    running: HashMap<NodeKey, Vec<trans::Running>>,
    measure_cache: HashMap<NodeKey, (Arc<str>, TextStyle, crate::geom::SizeF)>,
    stamp: u64,
    last_draw: Option<Arc<DrawList>>,

    rebuild: bool,
    layout_dirty: bool,
    full_redraw: bool,
    diags: Vec<SceneDiagnostic>,
    diag_seen: HashSet<(Option<InstanceId>, String)>,
    style_cache: HashMap<NodeKey, build::Cached>,
    last_stats: FrameStats,
}

/// Initial state of the bar element's classes and `border-radius`.
fn bar_radius_px(bar: &BarSpec) -> f32 {
    bar.radius as f32
}

pub(crate) fn text_style(style: &ComputedStyle, scale: f32) -> TextStyle {
    TextStyle {
        families: style.font_family.clone(),
        size_px: style.font_size * scale,
        weight: style.font_weight,
        italic: style.italic,
        letter_spacing: style.letter_spacing * scale,
        tabular_nums: style.tabular_nums,
    }
}

/// The bar's border box inside the surface.
fn bar_rect(bar: &BarSpec, g: &BarGeometry) -> RectF {
    let (w, h) = (g.width_px as f32, g.height_px as f32);
    if !bar.floating {
        return RectF::new(0.0, 0.0, w, h);
    }
    let m = (bar.margin as f32 * g.scale).round();
    let bar_h = (bar.height as f32 * g.scale).round().min(h);
    let edge = m.min((h - bar_h).max(0.0));
    let y = match bar.position {
        Position::Top => edge,
        Position::Bottom => h - bar_h - edge,
    };
    RectF::new(m.min(w / 2.0), y, (w - 2.0 * m).max(0.0), bar_h)
}

impl Scene {
    pub fn new(bar: &BarSpec, geometry: BarGeometry, env: SceneEnv) -> Scene {
        Scene {
            bar: bar.clone(),
            geometry,
            env,
            sheet: Arc::new(Stylesheet::empty()),
            slots: ZoneSlots::default(),
            trees: HashMap::new(),
            widget_plates: HashMap::new(),
            bar_plate: None,
            image_sizes: HashMap::new(),
            image_failed: HashSet::new(),
            image_touched: Vec::new(),
            hover: HashSet::new(),
            active: HashSet::new(),
            hover_chain: Vec::new(),
            pressed: None,
            cursor: Cursor::Default,
            state_time: None,
            arena: Vec::new(),
            prev: HashMap::new(),
            running: HashMap::new(),
            measure_cache: HashMap::new(),
            stamp: 0,
            last_draw: None,
            rebuild: true,
            layout_dirty: true,
            full_redraw: true,
            diags: Vec::new(),
            diag_seen: HashSet::new(),
            style_cache: HashMap::new(),
            last_stats: FrameStats::default(),
        }
    }

    fn touch(&mut self) {
        self.rebuild = true;
        self.layout_dirty = true;
    }

    /// Bar name, radius and the zone layout come from the spec.
    pub fn set_bar(&mut self, bar: &BarSpec) {
        if self.bar != *bar {
            self.bar = bar.clone();
            self.touch();
        }
    }

    pub fn set_geometry(&mut self, geometry: BarGeometry) {
        if self.geometry != geometry {
            self.geometry = geometry;
            self.measure_cache.clear();
            self.full_redraw = true;
            self.touch();
        }
    }

    pub fn set_env(&mut self, env: SceneEnv) {
        if self.env != env {
            self.env = env;
            self.style_cache.clear();
            self.touch();
        }
    }

    /// The full style sheet (base + preset + user), already concatenated.
    pub fn set_stylesheet(&mut self, sheet: Arc<Stylesheet>) {
        self.sheet = sheet;
        self.style_cache.clear();
        self.diag_seen.clear();
        self.touch();
    }

    pub fn set_slots(&mut self, slots: ZoneSlots) {
        if self.slots == slots {
            return;
        }
        let keep: HashSet<InstanceId> =
            slots.left.iter().chain(&slots.center).chain(&slots.right).map(|s| s.instance).collect();
        self.trees.retain(|i, _| keep.contains(i));
        self.widget_plates.retain(|i, _| keep.contains(i));
        self.slots = slots;
        self.touch();
    }

    fn in_slots(&self, instance: InstanceId) -> bool {
        self.slots.left.iter().chain(&self.slots.center).chain(&self.slots.right).any(|s| s.instance == instance)
    }

    /// A new tree for one instance. Trees for instances not in a slot are
    /// ignored (they belong to another bar).
    pub fn set_tree(&mut self, tree: Arc<WidgetTree>) {
        let instance = tree.instance;
        if !self.in_slots(instance) {
            return;
        }
        if self.trees.get(&instance).is_some_and(|old| old.version > tree.version) {
            return;
        }
        self.trees.insert(instance, tree);
        self.touch();
    }

    /// Replace an instance's content with a plate, or clear it.
    pub fn set_widget_plate(&mut self, instance: InstanceId, plate: Option<Plate>) {
        let changed = match plate {
            Some(p) => self.widget_plates.insert(instance, p.clone()).as_ref() != Some(&p),
            None => self.widget_plates.remove(&instance).is_some(),
        };
        if changed {
            self.touch();
        }
    }

    pub fn set_bar_plate(&mut self, plate: Option<Plate>) {
        if self.bar_plate != plate {
            self.bar_plate = plate;
            self.touch();
        }
    }

    /// The natural size of a decoded image, physical px at scale 1 (the
    /// binary calls this on `UiMsg::Image`, before `Renderer::set_image`).
    /// Nodes using `key` relayout and their groups are damaged, so the
    /// next frame draws the image even when nothing else changed.
    pub fn set_image_size(&mut self, key: &str, width: u32, height: u32) {
        self.image_sizes.insert(key.to_string(), (width, height));
        self.image_failed.remove(key);
        self.image_touched.push(key.to_string());
        self.touch();
    }

    /// The image at `key` could not be loaded: nodes using it keep their
    /// declared size (else 0 x 0) and are damaged once; the binary logs it
    /// and shows a widget plate.
    pub fn image_failed(&mut self, key: &str) {
        self.image_sizes.remove(key);
        self.image_failed.insert(key.to_string());
        self.image_touched.push(key.to_string());
        self.touch();
    }

    /// Hover and active state change at once; handler calls, plate clicks
    /// and unhandled buttons are returned. `now` starts any transition this
    /// causes. The scene never opens anything itself.
    pub fn pointer(&mut self, event: PointerEvent, now: Instant) -> PointerResult {
        self.pointer_impl(event, now)
    }

    /// True when `frame` would produce damage or a transition is due.
    pub fn needs_frame(&self, _now: Instant) -> bool {
        self.rebuild || self.layout_dirty || self.last_draw.is_none() || !self.running.is_empty()
    }

    pub fn frame(&mut self, now: Instant, text: &mut dyn TextMeasure) -> FrameOut {
        // A pointer move that changes no hover or active state (the common
        // case while the mouse crosses the bar) leaves nothing to restyle,
        // lay out or draw: hand back the last list.
        if !self.needs_frame(now)
            && !self.full_redraw
            && self.image_touched.is_empty()
            && let Some(last) = &self.last_draw
        {
            let backdrop = self.arena.first().map_or(Backdrop::None, |n| n.style.backdrop);
            let stats = FrameStats { nodes: self.arena.len(), ..FrameStats::default() };
            return FrameOut {
                draw: last.clone(),
                dirty: None,
                next_frame: None,
                cursor: self.cursor,
                backdrop,
                stats,
            };
        }
        let t0 = Instant::now();
        let mut stats = FrameStats::default();
        if self.rebuild || !self.running.is_empty() || self.last_draw.is_none() {
            stats.restyled = self.refresh(now);
            self.rebuild = false;
        }
        stats.style_us = t0.elapsed().as_micros() as u32;

        let t1 = Instant::now();
        if self.layout_dirty {
            self.layout(text);
            self.layout_dirty = false;
            stats.relayout = true;
        }
        stats.layout_us = t1.elapsed().as_micros() as u32;

        let t2 = Instant::now();
        let list = Arc::new(self.paint());
        let prev = if self.full_redraw { None } else { self.last_draw.as_deref() };
        let mut dirty = damage(prev, &list);
        self.full_redraw = false;
        // An image that just arrived (or failed) must be drawn even when the
        // list did not change.
        for key in std::mem::take(&mut self.image_touched) {
            for item in &list.items {
                if let crate::draw::DrawCmd::Image { key: k, .. } = &item.cmd
                    && k.as_ref() == key
                {
                    let surface = RectPx::new(0, 0, list.width as i32, list.height as i32);
                    let r = RectPx::outward(&item.bounds).intersect(&surface);
                    dirty = match (dirty, r) {
                        (Some(a), Some(b)) => Some(a.union(&b)),
                        (a, b) => a.or(b),
                    };
                }
            }
        }
        stats.draw_us = t2.elapsed().as_micros() as u32;
        self.last_draw = Some(list.clone());

        stats.nodes = self.arena.len();
        stats.running_transitions = self.running.values().map(Vec::len).sum();
        let next_frame = (stats.running_transitions > 0).then(|| now + trans::FRAME_INTERVAL);
        let backdrop = self.arena.first().map_or(Backdrop::None, |n| n.style.backdrop);
        self.last_stats = stats.clone();
        FrameOut { draw: list, dirty, next_frame, cursor: self.cursor, backdrop, stats }
    }

    /// Warnings since the last call: inline styles (with the element's
    /// `pos`), unknown icon names, duplicate sibling keys, a `var()` that is
    /// invalid after substitution (the declaration's position plus where the
    /// variable was defined), a box-shadow on the bar (cut off by the window
    /// edge). A message is returned once while it keeps being produced, and
    /// again when it goes away and comes back or the style sheet changes.
    pub fn take_diagnostics(&mut self) -> Vec<SceneDiagnostic> {
        std::mem::take(&mut self.diags)
    }

    /// Every laid-out node, root first, after the last `frame`.
    pub fn nodes(&self) -> Vec<NodeInfo> {
        self.arena
            .iter()
            .map(|n| NodeInfo {
                key: n.key,
                kind: n.kind,
                instance: n.instance,
                depth: n.depth,
                id: n.id.clone(),
                classes: n.classes.clone(),
                text: n.laid.text.clone().or_else(|| n.text.clone()),
                rect: n.laid.rect,
                style: (*n.style).clone(),
            })
            .collect()
    }
}

/// Alpha 1/255 black: the pixel that keeps a transparent bar clickable.
pub(crate) fn hit_fill_color() -> Color {
    Color::rgba8(0, 0, 0, 1)
}

/// The icon text for a node: the named glyph or the raw text.
fn icon_text(icon: &IconRef) -> Option<String> {
    match icon {
        IconRef::Named(n) => crate::icons::icon_char(n).map(String::from),
        IconRef::Glyph(g) => Some(g.to_string()),
    }
}

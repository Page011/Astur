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
//! Layout runs once with every node at the size its style gives (the
//! target layout, where popups are placed) and a second time only while a
//! width or height change is running, with those nodes held at their
//! current size (the layout that is drawn). A transition on `width` or
//! `height` runs whenever the size a node ends up at changes, `auto`
//! included; the first layout, a scale change, a new style sheet and
//! `fonts_changed` start none. A widget whose root is hidden is
//! `display: none`: it takes no space and no structural pseudo-class counts
//! it. `transform` is applied when the draw list is made and undone for
//! hit testing; `corner-shape` and `clip-path` make path commands.
//!
//! A floating bar keeps `margin` from the left and right edges of the
//! surface and from the screen edge it sits on (the surface is as tall as
//! the bar plus that margin); the margin is fully transparent.
//!
//! Popups, menus and tooltips are elements of a widget's tree that the bar
//! never lays out: `popups` reports them, and each open one is drawn by a
//! scene of its own (`new_popup`) that styles the popup root as a child of
//! virtual `bar` and `widget` nodes, so selectors and inherited values reach
//! the content, and lays it out at its natural size. Interaction state that
//! belongs to a node (text being edited, slider values, scroll offsets, focus)
//! is kept in the scene that draws the node, by node key, and `frame` also
//! moves what changes between inputs: smooth scrolling, the scrollbar's fade,
//! the caret's blink and reports that were held back.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::color::Color;
use crate::config::{BarSpec, Position};
use crate::css::cascade::Vars;
use crate::css::{MediaEnv, Stylesheet};
use crate::diag::{Diagnostic, Plate};
use crate::draw::{DrawList, TextStyle, damage};
use crate::element::{
    ElementKind, Handlers, IconRef, ImageRef, InputProps, InstanceId, Interaction, NodeKey, SliderProps, WidgetTree,
};
use crate::geom::{Edges, RectF, RectPx};
use crate::geom::{PointF, SizeF};
use crate::hit::{ElementCall, HandlerCall};
use crate::hit::{PointerEvent, PointerResult};
use crate::icons::IconFont;
use crate::key::{KeyEvent, TextInput};
use crate::layout::TextMeasure;
use crate::popup::{FlatEdges, PopupDecl, Side};
use crate::style::{Backdrop, ComputedStyle, Cursor};

use motion::{AnimRuns, SizeRun};
use shape::Affine;

mod build;
mod edit;
mod focus;
mod input;
mod keys;
mod layout;
mod motion;
mod paint;
mod popup;
mod scroll;
mod shape;
mod slider;
mod state;
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

/// A popup, menu or tooltip surface: what its root is styled under. The
/// root is matched as a child of `bar.<name>` > `widget.<class>#<id>` (not
/// drawn, not laid out), so `widget.media popup` selectors work and the
/// bar's inherited properties and variables (`:root`) reach the content.
#[derive(Clone, Debug, PartialEq)]
pub struct PopupSurfaceSpec {
    pub bar: BarSpec,
    /// The widget slot of the instance that declared the popup.
    pub slot: SlotSpec,
    /// `Popup`, `Menu` or `Tooltip`.
    pub kind: ElementKind,
    /// Where `popup::place` put it: the root gets the class `below`,
    /// `above`, `left`, `right`, `screen` or `pointer`.
    pub side: Side,
    /// The system backdrop (blur) is active behind the surface: the root
    /// gets the class `blur`.
    pub blur: bool,
}

/// What one key or text event led to, from `Scene::key` and
/// `Scene::text_input`. Element events (`on_key`, `on_change`, focus) are
/// collected for `Scene::take_events`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeyResult {
    /// Click handlers run by Enter or Space on a focused element (and by
    /// Enter on a menu item).
    pub calls: Vec<HandlerCall>,
    /// The scene used the key (editing, focus movement, scrolling, a click).
    pub consumed: bool,
    /// Escape that nothing inside used: close this surface.
    pub dismiss: bool,
    /// Ctrl+V in an input: read the clipboard off the UI thread, then call
    /// `Scene::paste`.
    pub paste: bool,
    /// Ctrl+C or Ctrl+X in an input (not a password): put this text on the
    /// clipboard off the UI thread.
    pub copy: Option<String>,
    /// Enter or Space picked a menu item (one that opens no submenu): the
    /// whole menu chain closes, whether or not the menu has an `on_close`.
    pub picked: bool,
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
    /// Border box, physical px, bar-relative, as drawn.
    rect: RectF,
    /// The border box once every running size change has arrived (the
    /// target layout).
    target: RectF,
    /// What the node's own and its ancestors' `transform` make of layout
    /// space.
    xf: Affine,
    /// Border and padding widths as laid out (physical px).
    border: Edges<f32>,
    padding: Edges<f32>,
    /// Text as drawn (ellipsis applied).
    text: Option<Arc<str>>,
    /// A zone that was shrunk to fit: its content is clipped.
    clip: bool,
    /// How far the scroll containers around this node have moved its content.
    scroll: (f32, f32),
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
    icon: Option<IconRef>,
    handlers: Handlers,
    interaction: Interaction,
    input: Option<Box<InputProps>>,
    slider: Option<Box<SliderProps>>,
    /// Style the rules gave (what transitions run towards).
    target: Arc<ComputedStyle>,
    /// Style drawn right now: the target with running transitions applied.
    style: Arc<ComputedStyle>,
    vars: Arc<Vars>,
    /// A running transition or an animating ancestor.
    animating: bool,
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
    /// The menu item the press went down on.
    menu_item: Option<NodeKey>,
}

/// A button held down that keeps steering after the pointer leaves the element.
#[derive(Clone, Copy)]
enum Drag {
    Input(NodeKey),
    Slider(NodeKey),
    Thumb { key: NodeKey, axis: usize, grab: f32 },
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
    /// A menu item was picked since the last pointer or key result.
    pick_pending: bool,

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
    /// Keyframe animations per node, by animation name.
    anims: HashMap<NodeKey, AnimRuns>,
    /// An animation is running and needs frames.
    anim_busy: bool,
    /// Width and height changes in progress, per axis.
    size_runs: HashMap<NodeKey, [Option<SizeRun>; 2]>,
    /// The border-box size each node was last drawn at (0 x 0 while hidden).
    drawn_size: HashMap<NodeKey, (f32, f32)>,
    /// The next layout starts no size change: the scale, the style sheet or
    /// the fonts changed under it.
    snap_sizes: bool,
    /// A refresh changed focus after it had matched styles: run it once more.
    restyle_again: bool,
    /// When the change behind this frame's restyle happened (the pointer
    /// event, else the frame): transitions and size changes start then.
    change_time: Instant,
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

    /// Popup scenes only: the surface's own state.
    popup: Option<popup::PopupScene>,
    /// First arena node that is laid out, drawn and hit (the virtual
    /// ancestors of a popup root come before it).
    root_idx: usize,
    /// Popups, menus and tooltips the trees declare.
    recs: Vec<popup::PopupRec>,
    dismissed: HashMap<NodeKey, u64>,
    /// Popups whose anchor matches `:open`.
    anchor_open: HashSet<NodeKey>,
    /// Submenus the scene opened (hover, keys).
    sub_open: HashSet<NodeKey>,
    natural: Option<popup::Natural>,
    focus: focus::FocusState,
    edits: HashMap<NodeKey, edit::EditState>,
    sliders: HashMap<NodeKey, slider::SliderState>,
    scrolls: HashMap<NodeKey, scroll::ScrollState>,
    drag: Option<Drag>,
    events: Vec<ElementCall>,
    caret_blink: Option<Duration>,
    /// The draw list must be made again although no style or layout changed.
    repaint: bool,
    /// The time of the frame being made (scrollbar fade, caret blink).
    now: Instant,
    /// Something needs frames until it settles / the next time one is due.
    dyn_busy: bool,
    wake: Option<Instant>,
    reveal_seen: HashMap<NodeKey, u64>,
    pending_reveals: Vec<NodeKey>,
    /// Elements marked `autofocus` that were already in the tree, so one that
    /// arrives with a page change is told apart from one that stays.
    autofocus_seen: HashSet<NodeKey>,

    /// When the scene was made: the second counter of the 60 s ring counts from it.
    born: Instant,
    /// What the built frames cost, for the inspector.
    totals: FrameTotals,
    /// Frame cost attributed to each widget instance (a small list: a bar
    /// holds a dozen instances, and one lookup per built frame is all it
    /// takes).
    costs: Vec<(InstanceId, InstanceCost)>,
    /// Instances whose tree arrived since the last built frame.
    fresh: Vec<InstanceId>,
    /// Built frames are added to `totals` and `costs` (on unless a caller
    /// turns it off to measure what it costs).
    accounting: bool,
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
            pick_pending: false,
            hover: HashSet::new(),
            active: HashSet::new(),
            hover_chain: Vec::new(),
            pressed: None,
            cursor: Cursor::Default,
            state_time: None,
            arena: Vec::new(),
            prev: HashMap::new(),
            running: HashMap::new(),
            anims: HashMap::new(),
            anim_busy: false,
            size_runs: HashMap::new(),
            drawn_size: HashMap::new(),
            snap_sizes: false,
            restyle_again: false,
            change_time: Instant::now(),
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
            popup: None,
            root_idx: 0,
            recs: Vec::new(),
            dismissed: HashMap::new(),
            anchor_open: HashSet::new(),
            sub_open: HashSet::new(),
            natural: None,
            focus: focus::FocusState::default(),
            edits: HashMap::new(),
            sliders: HashMap::new(),
            scrolls: HashMap::new(),
            drag: None,
            events: Vec::new(),
            caret_blink: None,
            repaint: false,
            now: Instant::now(),
            dyn_busy: false,
            wake: None,
            reveal_seen: HashMap::new(),
            pending_reveals: Vec::new(),
            autofocus_seen: HashSet::new(),
            born: Instant::now(),
            totals: FrameTotals::default(),
            costs: Vec::new(),
            fresh: Vec::new(),
            accounting: true,
        }
    }

    fn touch(&mut self) {
        self.rebuild = true;
        self.layout_dirty = true;
        self.natural = None;
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
            let rescaled = self.geometry.scale != geometry.scale;
            self.geometry = geometry;
            self.measure_cache.clear();
            // A popup whose window is re-placed while it opens (its anchor is still stretching) keeps the
            // size changes it has started; only a new scale makes sizes jump.
            if rescaled || self.popup.is_none() {
                self.snap_sizes = true;
            }
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
        self.snap_sizes = true;
        self.touch();
    }

    /// A font was registered: text measures differently now. Cached
    /// measurements are dropped and the next layout starts no size change,
    /// so nothing slides because a font finished loading.
    pub fn fonts_changed(&mut self) {
        self.measure_cache.clear();
        self.snap_sizes = true;
        self.full_redraw = true;
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
        self.costs.retain(|(i, _)| keep.contains(i));
        self.fresh.retain(|i| keep.contains(i));
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
        if !self.fresh.contains(&instance) {
            self.fresh.push(instance);
        }
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
        let mut result = self.pointer_impl(event, now);
        result.picked = std::mem::take(&mut self.pick_pending);
        result
    }

    /// True when `frame` would produce damage or a transition is due.
    pub fn needs_frame(&self, now: Instant) -> bool {
        self.rebuild
            || self.layout_dirty
            || self.last_draw.is_none()
            || !self.running.is_empty()
            || self.anim_busy
            || !self.size_runs.is_empty()
            || self.repaint
            || self.dyn_busy
            || self.wake.is_some_and(|t| now >= t)
    }

    pub fn frame(&mut self, now: Instant, text: &mut dyn TextMeasure) -> FrameOut {
        self.now = now;
        // A pointer move that changes no hover or active state (the common
        // case while the mouse crosses the bar) leaves nothing to restyle,
        // lay out or draw: hand back the last list.
        if !self.needs_frame(now)
            && !self.full_redraw
            && self.image_touched.is_empty()
            && let Some(last) = &self.last_draw
        {
            let backdrop = self.arena.get(self.root_idx).map_or(Backdrop::None, |n| n.style.backdrop);
            let stats = FrameStats { nodes: self.arena.len(), ..FrameStats::default() };
            return FrameOut {
                draw: last.clone(),
                dirty: None,
                next_frame: self.wake,
                cursor: self.cursor,
                backdrop,
                stats,
            };
        }
        let t0 = Instant::now();
        let mut stats = FrameStats::default();
        if self.rebuild || !self.running.is_empty() || self.anim_busy || self.last_draw.is_none() {
            stats.restyled = self.refresh(now);
            self.rebuild = false;
        }
        stats.style_us = t0.elapsed().as_micros() as u32;

        let t1 = Instant::now();
        if self.layout_dirty || !self.size_runs.is_empty() {
            self.layout(text, now);
            self.layout_dirty = false;
            stats.relayout = true;
        }
        stats.layout_us = t1.elapsed().as_micros() as u32;
        self.tick(now, text);

        let t2 = Instant::now();
        let list = Arc::new(self.paint());
        self.repaint = false;
        let prev = if self.full_redraw { None } else { self.last_draw.as_deref() };
        let mut dirty = damage(prev, &list);
        self.full_redraw = false;
        // An image or svg icon that just arrived (or failed) must be drawn even
        // when the list did not change.
        for key in std::mem::take(&mut self.image_touched) {
            for item in &list.items {
                if let crate::draw::DrawCmd::Image { key: k, .. } | crate::draw::DrawCmd::Svg { key: k, .. } = &item.cmd
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
        stats.running_transitions = self.running.values().map(Vec::len).sum::<usize>()
            + self.size_runs.values().map(|r| r.iter().flatten().count()).sum::<usize>();
        let busy = stats.running_transitions > 0 || self.anim_busy || self.dyn_busy;
        let next_frame = if busy { Some(now + trans::FRAME_INTERVAL) } else { self.wake };
        let backdrop = self.arena.get(self.root_idx).map_or(Backdrop::None, |n| n.style.backdrop);
        self.last_stats = stats.clone();
        if self.accounting && self.popup.is_none() {
            self.account(now, &stats);
        }
        FrameOut { draw: list, dirty, next_frame, cursor: self.cursor, backdrop, stats }
    }

    /// A scene for one popup, menu or tooltip surface. Its content arrives
    /// with `set_popup_tree`; `frame` lays the popup root out at its
    /// natural size at `root_offset`. An overlay (`Side::Screen`) is laid out
    /// at the size of `geometry` (the monitor) however the surface is sized
    /// later.
    pub fn new_popup(spec: PopupSurfaceSpec, geometry: BarGeometry, env: SceneEnv) -> Scene {
        Scene::new_popup_impl(spec, geometry, env)
    }

    /// The popup's content: the element at `path` (child indexes from the
    /// widget root) in `tree`. Node keys are the same as in the bar's
    /// scene, so the two agree on which popup this is.
    ///
    /// Ignored while the scene is closing (`set_open(false)` until it is
    /// set open again): a closing popup keeps drawing the content it had
    /// when it started closing, even though the script has already dropped
    /// it, and takes no pointer or key input (its handlers may be gone).
    pub fn set_popup_tree(&mut self, tree: Arc<WidgetTree>, path: Arc<[u32]>) {
        self.set_popup_tree_impl(tree, path);
    }

    /// Placement side or backdrop changed (engine classes on the root).
    /// `width`: physical px the root must take across (an attached popup
    /// takes its anchor's flat edge, `PopupProps::attach`); None = its CSS
    /// width.
    pub fn set_popup_surface(&mut self, side: Side, blur: bool, width: Option<f32>) {
        self.set_popup_surface_impl(side, blur, width);
    }

    /// Popup scene: whether `:open` matches its root. A new popup scene
    /// draws its first frame closed and is then set open, so the closed
    /// style is where its opening transitions start; set false to play the
    /// closing transitions (`is_settled` says when they are done).
    pub fn set_open(&mut self, open: bool, now: Instant) {
        self.set_open_impl(open, now);
    }

    /// Popup scene: the surface size the popup needs once open and
    /// settled (every transition and animation at its end), physical px,
    /// shadow extents included. The window is made this size, so an
    /// opening animation never resizes it.
    ///
    /// Overlay (`Side::Screen`): the root is laid out at the monitor's size
    /// but its own background is not drawn (it becomes the dim window's
    /// colour, `overlay_dim`); the surface is only the bounds of what the
    /// root's descendants draw, settled, shadows included.
    pub fn natural_size(&mut self, text: &mut dyn TextMeasure) -> SizeF {
        self.natural(text).size
    }

    /// Popup scene: the size of the root's own border box, which is what a
    /// popup places against its anchor; the surface is larger by the shadow
    /// extents. Known after `natural_size`.
    pub fn root_box_size(&self) -> SizeF {
        self.natural.map_or_else(SizeF::default, |n| n.boxed)
    }

    /// Popup scene: where the root's border box starts inside the surface
    /// (its shadow extents above and to the left). For an overlay this is
    /// the monitor-sized root's origin relative to the content surface (it
    /// is negative); the window goes at the monitor's origin minus it.
    /// Known after `natural_size` or the first `frame`.
    pub fn root_offset(&self) -> PointF {
        self.root_offset_impl()
    }

    /// Popup scene: the root's drawn outline (border box with its radii,
    /// bevels or `clip-path`), surface physical px, as a polygon with curves
    /// flattened to at most 8 segments per corner. Presses outside it are
    /// outside the popup, even where the window is (its transparent margin,
    /// its shadow, a part not grown yet). Empty while nothing is drawn.
    pub fn root_outline(&self) -> Vec<PointF> {
        self.root_outline_impl()
    }

    /// Overlay scene: the root's background colour as drawn now (its
    /// transitions applied, its `opacity` multiplied into the alpha). The
    /// binary paints the dim window with it; None for other popups.
    pub fn overlay_dim(&self) -> Option<Color> {
        self.overlay_dim_impl()
    }

    /// Popups declared in this scene's trees, after the last `frame` (any
    /// scene: a popup's content may declare popups too).
    pub fn popups(&self) -> Vec<PopupDecl> {
        self.popups_impl()
    }

    /// True when the trees declare at least one popup. Cheap: no list is built.
    pub fn declares_popups(&self) -> bool {
        self.declares_popups_impl()
    }

    /// The UI closed the popup at `key` while its tree said open with this
    /// `open_seq`: it is reported closed until a tree with a newer
    /// `open_seq` arrives.
    pub fn dismiss_popup(&mut self, key: NodeKey, open_seq: u64) {
        self.dismiss_popup_impl(key, open_seq);
    }

    /// Whether `:open` matches the anchor of the popup at `key` (set while
    /// its surface is opening or open, cleared when it starts closing).
    pub fn set_anchor_open(&mut self, key: NodeKey, open: bool, now: Instant) {
        self.set_anchor_open_impl(key, open, now);
    }

    /// The tooltip element of the deepest hovered node that has one.
    pub fn hovered_tooltip(&self) -> Option<NodeKey> {
        self.hovered_tooltip_impl()
    }

    /// Keyboard focus reached or left this surface's window (`:focus` only
    /// matches while the window has it).
    pub fn set_window_focus(&mut self, focused: bool, now: Instant) {
        self.set_window_focus_impl(focused, now);
    }

    /// Focuses the first `autofocus` element, else (when `keyboard`: the
    /// surface was opened from the keyboard) the first focusable one.
    /// `keyboard` also makes `:focus-visible` match.
    pub fn focus_first(&mut self, keyboard: bool, now: Instant) {
        self.focus_first_impl(keyboard, now);
    }

    /// A key on this surface. Order: the focused element's own handling
    /// (input editing, slider steps, scrolling), then Tab / Shift+Tab and
    /// arrows in a `nav` group, then Enter / Space as a click, then
    /// `on_key` of the focused element or its nearest ancestor; Escape that
    /// none of those used sets `dismiss`. An overlay takes no key until its
    /// input guard has lifted.
    pub fn key(&mut self, event: KeyEvent, now: Instant) -> KeyResult {
        let mut result = self.key_impl(event, now);
        result.picked = std::mem::take(&mut self.pick_pending);
        result
    }

    /// Whether the focused element is a password input (the input method
    /// stays off for it).
    pub fn focused_password(&self) -> bool {
        self.focused_password_impl()
    }

    /// Typed text and IME composition, for the focused input.
    pub fn text_input(&mut self, input: TextInput, now: Instant) -> KeyResult {
        self.text_input_impl(input, now)
    }

    /// Clipboard text for an earlier `KeyResult::paste` (first line only,
    /// cut to the input's length cap).
    pub fn paste(&mut self, text: &str, now: Instant) {
        self.paste_impl(text, now);
    }

    /// The focused input's caret, surface physical px (the IME window is
    /// placed against it).
    pub fn caret_rect(&self) -> Option<RectF> {
        self.caret_rect_impl()
    }

    /// Handler events since the last call: `on_key`, `on_change`,
    /// `on_submit`, slider values, focus, `on_scroll`, a menu item picked.
    /// Reports that were held back (typing, dragging) are queued by `frame`
    /// at the `next_frame` the last frame gave.
    pub fn take_events(&mut self) -> Vec<ElementCall> {
        std::mem::take(&mut self.events)
    }

    /// How long the caret stays on and off (Windows' `GetCaretBlinkTime`);
    /// None = it does not blink.
    pub fn set_caret_blink(&mut self, period: Option<Duration>) {
        if self.caret_blink != period {
            self.caret_blink = period;
            self.repaint = true;
        }
    }

    /// No transition, animation, caret blink or smooth scroll is running.
    pub fn is_settled(&self) -> bool {
        self.running.is_empty() && !self.anim_busy && self.size_runs.is_empty() && !self.dyn_busy
    }

    /// The flat parts of a node's outline along its top and bottom edges
    /// (its `clip-path` polygon, else its border box with bevelled corners
    /// cut), at the target layout, vertices on whole device px. None for an
    /// unknown or hidden node.
    pub fn flat_edges_of(&self, key: NodeKey) -> Option<FlatEdges> {
        let n = self.arena.iter().find(|n| n.key == key)?;
        if n.style.display_none {
            return None;
        }
        let rect = n.laid.target;
        let radii = paint::resolve_radii(&n.target.border_radius, &rect, self.geometry.scale);
        let poly = n.target.clip_path.as_ref().map(|c| shape::clip_points(c, &rect, self.geometry.scale));
        Some(shape::flat_edges(&rect, poly.as_deref(), &radii, &n.target.corner_shape))
    }

    /// The outline a node is drawn with, as a polygon in surface px: its
    /// `clip-path` vertices, else its border box with rounded corners
    /// flattened to 8 segments and bevelled corners cut; transform applied.
    /// Empty for an unknown or hidden node.
    pub fn outline_of(&self, key: NodeKey) -> Vec<PointF> {
        let Some(n) = self.arena.iter().find(|n| n.key == key && !n.style.display_none) else { return Vec::new() };
        let (rect, scale) = (n.laid.rect, self.geometry.scale);
        let poly = match &n.style.clip_path {
            Some(c) => shape::clip_points(c, &rect, scale),
            None => {
                let radii = paint::resolve_radii(&n.style.border_radius, &rect, scale);
                shape::outline_polygon(&rect, &radii, &n.style.corner_shape, 8)
            }
        };
        poly.into_iter().map(|p| n.laid.xf.point(p)).collect()
    }

    /// A node's border box once every running size change has arrived,
    /// physical px (popups are placed against where an anchor is going).
    pub fn target_rect_of(&self, key: NodeKey) -> Option<RectF> {
        self.arena.iter().find(|n| n.key == key && !n.style.display_none).map(|n| n.laid.target)
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
            .skip(self.root_idx)
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

// ---------------------------------------------------------------- inspector

/// What a bar's built frames cost. A frame that finds nothing to do returns
/// early and adds nothing here.
#[derive(Clone, Debug)]
pub struct FrameTotals {
    /// Frames that restyled, laid out or painted.
    pub frames: u64,
    pub style_us: u64,
    pub layout_us: u64,
    pub paint_us: u64,
    /// The longest single frame (style + layout + paint).
    pub frame_us_max: u32,
    /// Frames no widget tree arrived for (hover, transitions, a new scale).
    pub no_tree_frames: u64,
    /// The `now` of the last built frame.
    pub last_frame_at: Option<Instant>,
    /// One slot per second: (second number + 1, microseconds spent in it).
    ring: [(u64, u64); 60],
}

impl Default for FrameTotals {
    fn default() -> Self {
        FrameTotals {
            frames: 0,
            style_us: 0,
            layout_us: 0,
            paint_us: 0,
            frame_us_max: 0,
            no_tree_frames: 0,
            last_frame_at: None,
            ring: [(0, 0); 60],
        }
    }
}

/// Frame cost attributed to one widget instance. The scene lays the whole
/// bar out at once, so no widget has a timer of its own: a frame that
/// followed one instance's new tree and no other counts as that instance's
/// own, a frame that followed several is split evenly between them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InstanceCost {
    pub own: u64,
    pub own_us: u64,
    pub own_us_max: u32,
    pub shared: u64,
    pub shared_us: u64,
}

/// One node of a widget's tree as the inspector shows it.
#[derive(Clone, Debug)]
pub struct InspectNode {
    pub kind: ElementKind,
    pub id: Option<Arc<str>>,
    pub classes: Vec<Arc<str>>,
    /// The text the node holds. None for a password input: its value is
    /// never copied, only its length.
    pub text: Option<Arc<str>>,
    /// Characters in the text or input value.
    pub text_len: usize,
    pub password: bool,
    /// The picture's path or key, for an image node.
    pub image: Option<Arc<str>>,
    /// Border box in logical px, bar-relative.
    pub rect: [f32; 4],
    /// `display: none`.
    pub hidden: bool,
    /// Bit set over `STATE_NAMES`.
    pub states: u8,
    /// Bit set over `HANDLER_NAMES`.
    pub handlers: u16,
    /// 1 for the widget's root element.
    pub depth: u8,
    /// Index of the parent in the same list; None for the widget root.
    pub parent: Option<u32>,
    /// Child elements the node has, whether or not they were copied.
    pub children: u32,
    /// The style as drawn; kept only when asked for.
    pub style: Option<Arc<ComputedStyle>>,
}

pub const STATE_NAMES: [&str; 5] = ["hover", "active", "focus", "checked", "disabled"];
pub const HANDLER_NAMES: [&str; 10] =
    ["click", "right_click", "middle_click", "wheel", "enter", "leave", "key", "scroll", "focus", "blur"];

impl InspectNode {
    pub fn state_names(&self) -> Vec<&'static str> {
        STATE_NAMES.iter().enumerate().filter(|(i, _)| self.states & (1 << i) != 0).map(|(_, n)| *n).collect()
    }

    pub fn handler_names(&self) -> Vec<&'static str> {
        HANDLER_NAMES.iter().enumerate().filter(|(i, _)| self.handlers & (1 << i) != 0).map(|(_, n)| *n).collect()
    }
}

/// A popup, menu or tooltip a widget declares. It is not part of the bar's
/// layout; its nodes live in the scene of its own surface.
#[derive(Clone, Debug)]
pub struct InspectPopup {
    /// Its first name, empty when it has none.
    pub name: String,
    pub kind: ElementKind,
    /// The script says it is open.
    pub open: bool,
    /// Elements it declares.
    pub nodes: usize,
    /// Index in `InstanceNodes::nodes` of the element it hangs from; None
    /// when that element was not copied.
    pub anchor: Option<u32>,
}

/// What `Scene::nodes_of` copies of one widget instance.
#[derive(Clone, Debug, Default)]
pub struct InstanceNodes {
    /// Breadth first: a parent always comes before its children.
    pub nodes: Vec<InspectNode>,
    /// Nodes the instance has in all, copied or not.
    pub total: usize,
    /// The copy stopped at the node limit (else, when `nodes.len() < total`,
    /// at the depth limit).
    pub limit_hit: bool,
    pub popups: Vec<InspectPopup>,
    /// The style of what the widget root hangs from, for inheritance.
    pub base_style: Option<Arc<ComputedStyle>>,
}

fn count_elements(e: &crate::element::Element) -> usize {
    1 + e.children.iter().map(count_elements).sum::<usize>()
}

impl Scene {
    fn account(&mut self, now: Instant, stats: &FrameStats) {
        let total = u64::from(stats.style_us) + u64::from(stats.layout_us) + u64::from(stats.draw_us);
        let total_u32 = total.min(u64::from(u32::MAX)) as u32;
        let second = now.saturating_duration_since(self.born).as_secs();
        let t = &mut self.totals;
        t.frames += 1;
        t.style_us += u64::from(stats.style_us);
        t.layout_us += u64::from(stats.layout_us);
        t.paint_us += u64::from(stats.draw_us);
        t.frame_us_max = t.frame_us_max.max(total_u32);
        t.last_frame_at = Some(now);
        let slot = &mut t.ring[(second % 60) as usize];
        if slot.0 != second + 1 {
            *slot = (second + 1, 0);
        }
        slot.1 += total;
        match self.fresh.len() {
            0 => self.totals.no_tree_frames += 1,
            1 => {
                let c = Self::cost_of(&mut self.costs, self.fresh[0]);
                c.own += 1;
                c.own_us += total;
                c.own_us_max = c.own_us_max.max(total_u32);
            }
            n => {
                let share = total / n as u64;
                for i in &self.fresh {
                    let c = Self::cost_of(&mut self.costs, *i);
                    c.shared += 1;
                    c.shared_us += share;
                }
            }
        }
        self.fresh.clear();
    }

    fn cost_of(costs: &mut Vec<(InstanceId, InstanceCost)>, instance: InstanceId) -> &mut InstanceCost {
        let at = match costs.iter().position(|(i, _)| *i == instance) {
            Some(at) => at,
            None => {
                costs.push((instance, InstanceCost::default()));
                costs.len() - 1
            }
        };
        &mut costs[at].1
    }

    /// Turns the per-frame accounting on or off (on by default). Off, `frame_totals`
    /// and `instance_cost` stop growing.
    pub fn set_accounting(&mut self, on: bool) {
        self.accounting = on;
        self.fresh.clear();
    }

    /// Runs the accounting step `frames` times as a frame that followed one
    /// widget's tree would, and returns how long that took. It exists to
    /// measure the step on its own: its cost is far below the noise of timing
    /// whole frames.
    #[doc(hidden)]
    pub fn accounting_probe(&mut self, instance: InstanceId, frames: u32, now: Instant) -> Duration {
        let stats = FrameStats { style_us: 90, layout_us: 120, draw_us: 60, ..FrameStats::default() };
        let t = Instant::now();
        for _ in 0..frames {
            self.fresh.push(instance);
            self.account(now, &stats);
        }
        t.elapsed()
    }

    /// What this bar's built frames cost so far.
    pub fn frame_totals(&self) -> &FrameTotals {
        &self.totals
    }

    /// Microseconds spent in built frames during the last 60 seconds up to `now`.
    pub fn frame_us_last_60s(&self, now: Instant) -> u64 {
        let second = now.saturating_duration_since(self.born).as_secs();
        self.totals
            .ring
            .iter()
            .filter(|(s, _)| *s != 0 && second + 1 >= *s && second + 1 - *s < 60)
            .map(|(_, us)| us)
            .sum()
    }

    /// The frame cost attributed to `instance`.
    pub fn instance_cost(&self, instance: InstanceId) -> InstanceCost {
        self.costs.iter().find(|(i, _)| *i == instance).map(|(_, c)| *c).unwrap_or_default()
    }

    /// Nodes in the scene: the root and everything below it.
    pub fn node_count(&self) -> usize {
        self.arena.len().saturating_sub(self.root_idx)
    }

    /// A newer tree or style is waiting for the next frame.
    pub fn pending(&self) -> bool {
        self.rebuild || self.layout_dirty
    }

    /// Whether the bar is styled for a dark colour scheme.
    pub fn dark(&self) -> bool {
        self.env.media.dark
    }

    /// The value of the custom property `name` (with or without the leading
    /// `--`) on the root node, as text: a colour is written `#rrggbb`.
    pub fn root_var(&self, name: &str) -> Option<String> {
        let name = name.strip_prefix("--").unwrap_or(name);
        let root = self.arena.get(self.root_idx)?;
        let def = root.vars.map.get(format!("--{name}").as_str())?;
        Some(Color::parse(&def.value).map_or_else(|_| def.value.to_string(), crate::css_text::color_hex))
    }

    /// The root node (the bar, or a popup's own root) as the inspector shows it.
    pub fn root_node(&self, with_style: bool) -> Option<InspectNode> {
        self.arena.get(self.root_idx)?;
        Some(self.inspect_node(self.root_idx, 1, None, with_style))
    }

    fn inspect_node(&self, i: usize, depth: u8, parent: Option<u32>, with_style: bool) -> InspectNode {
        let n = &self.arena[i];
        let scale = self.geometry.scale.max(0.01);
        let r = n.laid.rect;
        let mut states = 0u8;
        for (bit, on) in [
            self.hover.contains(&n.key),
            self.active.contains(&n.key),
            self.focus.node == Some(n.key),
            n.interaction.checked == Some(true),
            n.interaction.disabled,
        ]
        .into_iter()
        .enumerate()
        {
            states |= u8::from(on) << bit;
        }
        let h = &n.handlers;
        let mut handlers = 0u16;
        for (bit, on) in [
            h.click.is_some(),
            h.right_click.is_some(),
            h.middle_click.is_some(),
            h.wheel.is_some(),
            h.enter.is_some(),
            h.leave.is_some(),
            h.key.is_some(),
            h.scroll.is_some(),
            h.focus.is_some(),
            h.blur.is_some(),
        ]
        .into_iter()
        .enumerate()
        {
            handlers |= u16::from(on) << bit;
        }
        let password = n.input.as_ref().is_some_and(|p| p.password);
        let (text, text_len) = match &n.input {
            Some(p) if p.password => (None, p.value.chars().count()),
            Some(p) => (Some(p.value.clone()), p.value.chars().count()),
            None => {
                let t = n.text.clone().or_else(|| n.laid.text.clone());
                let len = t.as_ref().map_or(0, |t| t.chars().count());
                (t, len)
            }
        };
        InspectNode {
            kind: n.kind,
            id: n.id.clone(),
            classes: n.classes.clone(),
            text,
            text_len,
            password,
            image: n.image.as_ref().map(|i| i.path.clone()),
            rect: [r.x / scale, r.y / scale, r.w / scale, r.h / scale],
            hidden: n.style.display_none,
            states,
            handlers,
            depth,
            parent,
            children: n.children.len() as u32,
            style: with_style.then(|| n.style.clone()),
        }
    }

    /// Copies the tree under `roots` breadth first. Returns the copy and the
    /// arena index each copied node came from.
    fn nodes_below(&self, roots: &[usize], max: usize, max_depth: u8, with_style: bool) -> (InstanceNodes, Vec<usize>) {
        let mut total = 0usize;
        let mut stack: Vec<usize> = roots.to_vec();
        while let Some(i) = stack.pop() {
            total += 1;
            stack.extend(self.arena[i].children.iter().copied());
        }
        let mut nodes: Vec<InspectNode> = Vec::with_capacity(max.min(total));
        let mut from: Vec<usize> = Vec::with_capacity(max.min(total));
        let mut queue: std::collections::VecDeque<(usize, Option<u32>, u8)> =
            roots.iter().map(|r| (*r, None, 1u8)).collect();
        let mut limit_hit = false;
        while let Some((i, parent, depth)) = queue.pop_front() {
            if nodes.len() >= max {
                limit_hit = true;
                break;
            }
            let at = nodes.len() as u32;
            nodes.push(self.inspect_node(i, depth, parent, with_style));
            from.push(i);
            if depth < max_depth {
                for &c in &self.arena[i].children {
                    queue.push_back((c, Some(at), depth + 1));
                }
            }
        }
        (InstanceNodes { nodes, total, limit_hit, popups: Vec::new(), base_style: None }, from)
    }

    /// The nodes of one widget instance, breadth first, at most `max` of
    /// them and no deeper than `max_depth` (the widget root is depth 1); the
    /// rest are counted in `total`. Popups the widget declares are listed
    /// with the element they hang from. `with_style` keeps each node's
    /// style. Empty when the instance is not on this bar.
    pub fn nodes_of(&self, instance: InstanceId, max: usize, max_depth: u8, with_style: bool) -> InstanceNodes {
        let Some(wrapper) =
            self.arena.iter().position(|n| n.kind == ElementKind::Widget && n.instance == Some(instance))
        else {
            return InstanceNodes::default();
        };
        let roots = self.arena[wrapper].children.clone();
        let (mut out, from) = self.nodes_below(&roots, max, max_depth, with_style);
        out.base_style = Some(self.arena[wrapper].style.clone());
        out.popups = self
            .recs
            .iter()
            .filter(|r| r.instance == instance)
            .map(|r| {
                let nodes = r
                    .tree
                    .root
                    .as_ref()
                    .and_then(|root| popup::element_at(root, &r.path))
                    .map_or(0, |(el, _)| count_elements(el));
                let anchor = self
                    .arena
                    .iter()
                    .position(|n| n.key == r.anchor)
                    .and_then(|a| from.iter().position(|f| *f == a))
                    .map(|at| at as u32);
                InspectPopup {
                    name: r.props.names.first().map(|n| n.to_string()).unwrap_or_default(),
                    kind: r.kind,
                    open: r.props.open,
                    nodes,
                    anchor,
                }
            })
            .collect();
        out
    }

    /// A popup scene's content: its root element and everything below, at
    /// most `max` nodes (the rest are counted in `total`).
    pub fn popup_nodes(&self, max: usize, max_depth: u8, with_style: bool) -> InstanceNodes {
        if self.popup.is_none() || self.arena.len() <= self.root_idx {
            return InstanceNodes::default();
        }
        let (mut out, _) = self.nodes_below(&[self.root_idx], max, max_depth, with_style);
        out.base_style = self.root_idx.checked_sub(1).and_then(|p| self.arena.get(p)).map(|n| n.style.clone());
        out
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
        // Drawn as a picture, not text: no glyph to measure.
        IconRef::Svg(_) => Some(String::new()),
    }
}

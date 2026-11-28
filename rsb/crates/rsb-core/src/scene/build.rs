//! Building the node arena: the skeleton, the widget trees, node keys,
//! style (with a per-node cache) and transitions.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Instant;

use super::motion::{self, AnimRuns};
use super::popup::{PopupRec, element_at};
use super::trans::{self, Running};
use super::{Laid, Node, Prev, Scene, SlotSpec, icon_text};
use crate::color::Color;
use crate::config::BarSpec;
use crate::css::cascade::{self, NodeInput, StyleEnv, Vars};
use crate::css::parse_inline;
use crate::css::select::MatchTree;
use crate::diag::{DiagKind, Diagnostic, Plate, SourcePos};
use crate::element::{
    Element, ElementKind, Handlers, IconRef, ImageRef, InputProps, InstanceId, Interaction, MAX_DEPTH, NodeKey,
    SliderProps, WidgetTree,
};
use crate::geom::Edges;
use crate::style::{ComputedStyle, FlexDirection};

/// FNV-1a, 64 bit.
pub(super) struct Fnv(pub u64);

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

impl Fnv {
    pub fn new() -> Fnv {
        Fnv(FNV_OFFSET)
    }
}

impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 ^ u64::from(*b)).wrapping_mul(FNV_PRIME);
        }
    }
}

/// `seed` extended with `bytes`, FNV-1a.
pub(super) fn fnv(seed: u64, bytes: &[u8]) -> u64 {
    let mut h = Fnv(seed);
    h.write(bytes);
    h.finish()
}

pub(super) fn fixed_key(name: &str) -> u64 {
    fnv(FNV_OFFSET, name.as_bytes())
}

/// Style computed for one node, kept while its inputs stay the same.
pub(super) struct Cached {
    sig: u64,
    target: Arc<ComputedStyle>,
    vars: Arc<Vars>,
    /// What computing this style complained about, so a cached node keeps
    /// reporting the same warnings (the scene only shows a warning once while
    /// it persists).
    diags: Vec<Diagnostic>,
}

/// What the scene knows about focus and open state, for the pseudo-classes.
#[derive(Default)]
pub(super) struct FxView {
    pub focus: Option<NodeKey>,
    /// The surface's window has keyboard focus.
    pub window: bool,
    pub visible: bool,
    /// The focused node and its ancestors.
    pub chain: HashSet<NodeKey>,
    /// The popup root and whether its surface is open.
    pub root: Option<NodeKey>,
    pub open: bool,
    /// Anchors of popups whose surface is open.
    pub anchors: HashSet<NodeKey>,
}

struct ArenaTree<'a> {
    nodes: &'a [Node],
    hover: &'a HashSet<NodeKey>,
    active: &'a HashSet<NodeKey>,
    fx: &'a FxView,
}

impl ArenaTree<'_> {
    /// Whether a sibling takes part in layout (`display: none` and surface
    /// roots do not). `me` is always counted: a node's own position does not
    /// change with the state its own rules gave it.
    fn takes_part(&self, i: usize, me: usize) -> bool {
        i == me || (!self.nodes[i].target.display_none && !self.nodes[i].kind.is_surface_root())
    }
}

impl MatchTree for ArenaTree<'_> {
    fn kind(&self, node: usize) -> ElementKind {
        self.nodes[node].kind
    }

    fn id(&self, node: usize) -> Option<&str> {
        self.nodes[node].id.as_deref()
    }

    fn has_class(&self, node: usize, class: &str) -> bool {
        self.nodes[node].classes.iter().any(|c| c.as_ref() == class)
    }

    fn hover(&self, node: usize) -> bool {
        self.hover.contains(&self.nodes[node].key)
    }

    fn active(&self, node: usize) -> bool {
        self.active.contains(&self.nodes[node].key)
    }

    fn parent(&self, node: usize) -> Option<usize> {
        self.nodes[node].parent
    }

    fn focus(&self, node: usize) -> bool {
        self.fx.window && self.fx.focus == Some(self.nodes[node].key)
    }

    fn focus_visible(&self, node: usize) -> bool {
        self.focus(node) && self.fx.visible
    }

    fn focus_within(&self, node: usize) -> bool {
        self.fx.window && self.fx.chain.contains(&self.nodes[node].key)
    }

    fn disabled(&self, node: usize) -> bool {
        self.nodes[node].interaction.disabled
    }

    fn checked(&self, node: usize) -> bool {
        self.nodes[node].interaction.checked == Some(true)
    }

    fn open(&self, node: usize) -> bool {
        let key = self.nodes[node].key;
        (self.fx.open && self.fx.root == Some(key)) || self.fx.anchors.contains(&key)
    }

    fn index_in_parent(&self, node: usize) -> usize {
        let Some(p) = self.nodes[node].parent else { return 1 };
        let mut n = 0;
        for &c in &self.nodes[p].children {
            if self.takes_part(c, node) {
                n += 1;
            }
            if c == node {
                break;
            }
        }
        n
    }

    fn sibling_count(&self, node: usize) -> usize {
        let Some(p) = self.nodes[node].parent else { return 1 };
        self.nodes[p].children.iter().filter(|&&c| self.takes_part(c, node)).count()
    }
}

/// What a node is made from, kept until the arena is styled.
struct Spec<'a> {
    key: NodeKey,
    kind: ElementKind,
    id: Option<Arc<str>>,
    inline: Cow<'a, [(String, String)]>,
    inline_pos: Option<SourcePos>,
    direction: FlexDirection,
    /// `AnimateSpec::restart_seq`.
    restart_seq: u64,
    instance: Option<InstanceId>,
    /// An empty widget wrapper: `display: none`.
    hidden: bool,
}

/// Everything `push` needs besides the key and the parent.
struct NodeArgs<'a> {
    key: NodeKey,
    kind: ElementKind,
    instance: Option<InstanceId>,
    parent: Option<usize>,
    id: Option<Arc<str>>,
    classes: Vec<Arc<str>>,
    text: Option<Arc<str>>,
    image: Option<ImageRef>,
    icon: Option<IconRef>,
    handlers: Handlers,
    inline: Cow<'a, [(String, String)]>,
    inline_pos: Option<SourcePos>,
    direction: FlexDirection,
    restart_seq: u64,
    /// An empty widget: drawn as `display: none`.
    hidden: bool,
    interaction: Interaction,
    input: Option<Box<InputProps>>,
    slider: Option<Box<SliderProps>>,
}

impl<'a> NodeArgs<'a> {
    fn new(key: NodeKey, kind: ElementKind, parent: Option<usize>) -> NodeArgs<'a> {
        NodeArgs {
            key,
            kind,
            instance: None,
            parent,
            id: None,
            classes: Vec::new(),
            text: None,
            image: None,
            icon: None,
            handlers: Handlers::default(),
            inline: Cow::Borrowed(&[]),
            inline_pos: None,
            direction: FlexDirection::Row,
            restart_seq: 0,
            hidden: false,
            interaction: Interaction::default(),
            input: None,
            slider: None,
        }
    }
}

/// What a popup scene builds its arena from.
struct PopupCtx {
    slot: SlotSpec,
    content: Option<super::popup::Content>,
    side_class: &'static str,
    blur: bool,
}

struct Builder<'a> {
    sheet: &'a crate::css::Stylesheet,
    env: StyleEnv,
    bar: &'a BarSpec,
    hover: &'a HashSet<NodeKey>,
    active: &'a HashSet<NodeKey>,
    old_cache: HashMap<NodeKey, Cached>,
    new_cache: HashMap<NodeKey, Cached>,
    old_prev: &'a HashMap<NodeKey, Prev>,
    new_prev: HashMap<NodeKey, Prev>,
    old_running: HashMap<NodeKey, Vec<Running>>,
    new_running: HashMap<NodeKey, Vec<Running>>,
    old_anims: HashMap<NodeKey, AnimRuns>,
    new_anims: HashMap<NodeKey, AnimRuns>,
    nodes: Vec<Node>,
    specs: Vec<Spec<'a>>,
    /// Style warnings per node, replaced each time the node is styled again.
    node_diags: Vec<Vec<Diagnostic>>,
    /// With the arena index the warning belongs at, so they read in tree order.
    diags: Vec<(usize, Option<InstanceId>, Diagnostic)>,
    restyled: usize,
    layout_changed: bool,
    anim_busy: bool,
    now: Instant,
    state_time: Instant,
    stamp: u64,
    placeholder: Arc<ComputedStyle>,
    hidden_style: Arc<ComputedStyle>,
    empty_vars: Arc<Vars>,
    fx: FxView,
    /// The tree being added and the child indexes down to the element.
    tree: Option<Arc<WidgetTree>>,
    path: Vec<u32>,
    recs: Vec<PopupRec>,
}

impl<'a> Builder<'a> {
    /// Works out what the pseudo-classes need from the finished structure: the
    /// focused node's ancestors, the popup root, the anchors of open popups.
    fn prepare_fx(&mut self, anchor_open: &HashSet<NodeKey>, root_idx: usize) {
        if let Some(f) = self.fx.focus
            && let Some(mut i) = self.nodes.iter().position(|n| n.key == f)
        {
            loop {
                self.fx.chain.insert(self.nodes[i].key);
                match self.nodes[i].parent {
                    Some(p) => i = p,
                    None => break,
                }
            }
        }
        self.fx.root = self.nodes.get(root_idx).map(|n| n.key);
        self.fx.anchors = self.recs.iter().filter(|r| anchor_open.contains(&r.key)).map(|r| r.anchor).collect();
    }

    /// Adds a node to the arena (structure only; `style_all` styles it).
    fn push(&mut self, a: NodeArgs<'a>) -> usize {
        let idx = self.nodes.len();
        let depth = a.parent.map_or(0, |p| self.nodes[p].depth + 1);
        let style = if a.hidden { self.hidden_style.clone() } else { self.placeholder.clone() };
        self.nodes.push(Node {
            key: a.key,
            kind: a.kind,
            instance: a.instance,
            parent: a.parent,
            children: Vec::new(),
            depth,
            id: a.id.clone(),
            classes: a.classes,
            text: a.text,
            image: a.image,
            icon: a.icon,
            handlers: a.handlers,
            interaction: a.interaction,
            input: a.input,
            slider: a.slider,
            target: style.clone(),
            style,
            vars: self.empty_vars.clone(),
            animating: false,
            base_sig: 0,
            laid: Laid::default(),
        });
        self.specs.push(Spec {
            key: a.key,
            kind: a.kind,
            id: a.id,
            inline: a.inline,
            inline_pos: a.inline_pos,
            direction: a.direction,
            restart_seq: a.restart_seq,
            instance: a.instance,
            hidden: a.hidden,
        });
        self.node_diags.push(Vec::new());
        if let Some(p) = a.parent {
            self.nodes[p].children.push(idx);
        }
        idx
    }

    /// Styles the whole arena, level by level: the children of one parent
    /// together, so structural pseudo-classes can see their siblings.
    fn style_all(&mut self) {
        if self.nodes.is_empty() {
            return;
        }
        self.compute_target(0, false);
        self.finalize(0);
        self.style_children(0);
    }

    fn style_children(&mut self, p: usize) {
        let kids = self.nodes[p].children.clone();
        if kids.is_empty() {
            return;
        }
        for &c in &kids {
            self.compute_target(c, false);
        }
        // A sibling that styled itself `display: none` leaves the count, which
        // can change what the others match: style again with the real counts.
        if self.sheet.structural {
            let hidden = |b: &Builder, kids: &[usize]| -> Vec<bool> {
                kids.iter().map(|&c| b.nodes[c].target.display_none).collect()
            };
            let mut seen = hidden(self, &kids);
            for _ in 0..3 {
                if !seen.iter().any(|h| *h) {
                    break;
                }
                for &c in &kids {
                    self.compute_target(c, true);
                }
                let now = hidden(self, &kids);
                if now == seen {
                    break;
                }
                seen = now;
            }
        }
        for &c in &kids {
            self.finalize(c);
        }
        for &c in &kids {
            self.style_children(c);
        }
    }

    /// The rules' style for one node: from the cache when nothing it
    /// depends on changed.
    fn compute_target(&mut self, idx: usize, again: bool) {
        let (key, kind) = (self.specs[idx].key, self.specs[idx].kind);
        let parent = self.nodes[idx].parent;
        let parent_sig = parent.map(|p| (self.nodes[p].base_sig, self.nodes[p].animating));
        let (parent_style, parent_vars) = match parent {
            Some(p) => (Some(self.nodes[p].style.clone()), self.nodes[p].vars.clone()),
            None => (None, self.empty_vars.clone()),
        };
        let hidden = self.specs[idx].hidden;

        // Inputs that decide the style.
        let spec = &self.specs[idx];
        let mut h = Fnv::new();
        kind.hash(&mut h);
        spec.id.hash(&mut h);
        self.nodes[idx].classes.hash(&mut h);
        self.hover.contains(&key).hash(&mut h);
        self.active.contains(&key).hash(&mut h);
        spec.inline.hash(&mut h);
        spec.inline_pos.hash(&mut h);
        (spec.direction == FlexDirection::Column).hash(&mut h);
        hidden.hash(&mut h);
        if kind == ElementKind::Bar {
            self.bar.radius.hash(&mut h);
        }
        {
            let tree = ArenaTree { nodes: &self.nodes, hover: self.hover, active: self.active, fx: &self.fx };
            tree.focus(idx).hash(&mut h);
            tree.focus_visible(idx).hash(&mut h);
            tree.focus_within(idx).hash(&mut h);
            tree.disabled(idx).hash(&mut h);
            tree.checked(idx).hash(&mut h);
            tree.open(idx).hash(&mut h);
            if self.sheet.structural {
                tree.index_in_parent(idx).hash(&mut h);
                tree.sibling_count(idx).hash(&mut h);
            }
        }
        h.write_u64(parent_sig.map_or(0, |p| p.0));
        let base_sig = h.finish();
        let sig = fnv(base_sig, &[u8::from(parent_sig.is_some_and(|p| p.1))])
            ^ if parent_sig.is_some_and(|p| p.1) { self.stamp } else { 0 };

        let cached = if again { None } else { self.old_cache.remove(&key) }
            .filter(|c| c.sig == sig)
            .or_else(|| self.new_cache.remove(&key).filter(|c| c.sig == sig));
        let (target, vars, style_diags) = match cached {
            Some(c) => (c.target, c.vars, c.diags),
            None => {
                self.restyled += 1;
                let mut diags = Vec::new();
                let inline = if self.specs[idx].inline.is_empty() {
                    None
                } else {
                    let (decls, d) = parse_inline(&self.specs[idx].inline, self.specs[idx].inline_pos.clone());
                    diags.extend(d);
                    Some(decls)
                };
                let input = NodeInput {
                    kind,
                    parent: parent_style.as_deref(),
                    parent_vars: &parent_vars,
                    inline: inline.as_ref(),
                    initial_radius: (kind == ElementKind::Bar).then(|| super::bar_radius_px(self.bar)),
                    initial_direction: self.specs[idx].direction,
                };
                let tree = ArenaTree { nodes: &self.nodes, hover: self.hover, active: self.active, fx: &self.fx };
                let (mut style, vars) = cascade::compute(self.sheet, &tree, idx, &self.env, &input, &mut diags);
                if kind == ElementKind::Plate {
                    style = plate_style(&style);
                }
                if hidden {
                    style.display_none = true;
                }
                if kind == ElementKind::Bar && !style.box_shadow.is_empty() {
                    diags.push(Diagnostic::warning(
                        DiagKind::Style,
                        None,
                        "box-shadow on bar is cut off by the window edge",
                    ));
                }
                let inline_pos = self.specs[idx].inline_pos.clone();
                let diags = diags
                    .into_iter()
                    .map(|d| {
                        if d.pos.is_none() && inline_pos.is_some() {
                            Diagnostic { pos: inline_pos.clone(), ..d }
                        } else {
                            d
                        }
                    })
                    .collect();
                (Arc::new(style), vars, diags)
            }
        };
        self.node_diags[idx] = style_diags.clone();
        self.new_cache.insert(key, Cached { sig, target: target.clone(), vars: vars.clone(), diags: style_diags });
        let n = &mut self.nodes[idx];
        n.target = target;
        n.vars = vars;
        n.base_sig = base_sig;
    }

    /// Transitions and animations: the style drawn now.
    fn finalize(&mut self, idx: usize) {
        let (key, kind, instance) = (self.specs[idx].key, self.specs[idx].kind, self.specs[idx].instance);
        let restart_seq = self.specs[idx].restart_seq;
        let parent_animating = self.nodes[idx].parent.is_some_and(|p| self.nodes[p].animating);
        let (target, base_sig) = (self.nodes[idx].target.clone(), self.nodes[idx].base_sig);
        for d in std::mem::take(&mut self.node_diags[idx]) {
            self.diags.push((idx, instance, d));
        }

        let mut runs = self.old_running.remove(&key).unwrap_or_default();
        let prev = self.old_prev.get(&key);
        if let Some(p) = prev
            && p.base_sig != base_sig
            && kind != ElementKind::Plate
        {
            trans::detect(&p.target, &p.display, &target, &mut runs, self.state_time);
        }
        let (mut display, runs) = trans::evaluate(&target, runs, self.now);
        let mut anim_busy = false;
        if !target.animations.is_empty() {
            let old = self.old_anims.remove(&key).unwrap_or_default();
            let aruns = motion::reconcile(old, &target.animations, restart_seq, self.state_time, self.now);
            anim_busy = motion::apply(Arc::make_mut(&mut display), &target.animations, &aruns, self.now);
            if !aruns.is_empty() {
                self.new_anims.insert(key, aruns);
            }
        }
        self.anim_busy |= anim_busy;
        let animating = !runs.is_empty() || anim_busy || parent_animating;
        match prev {
            None => self.layout_changed = true,
            Some(p) if !trans::layout_eq(&p.display, &display) => self.layout_changed = true,
            _ => {}
        }
        if !runs.is_empty() {
            self.new_running.insert(key, runs);
        }
        self.new_prev.insert(key, Prev { target: target.clone(), display: display.clone(), base_sig });
        let n = &mut self.nodes[idx];
        n.style = display;
        n.animating = animating;
    }

    fn note(&mut self, instance: Option<InstanceId>, d: Diagnostic) {
        self.diags.push((self.nodes.len(), instance, d));
    }

    /// The text an icon node shows, with a warning for a name that is not known.
    fn icon_text_of(&mut self, icon: &Option<IconRef>, pos: &Option<SourcePos>, instance: InstanceId) -> Arc<str> {
        let text = match icon {
            Some(icon) => match icon_text(icon) {
                Some(t) => Arc::from(t),
                None => {
                    let name = match icon {
                        IconRef::Named(n) => n.to_string(),
                        IconRef::Glyph(g) | IconRef::Svg(g) => g.to_string(),
                    };
                    let hint = crate::suggest::hint(&name, crate::icons::ICON_NAMES.iter().copied());
                    let msg = format!("unknown icon '{name}'{hint}");
                    self.note(Some(instance), Diagnostic::warning(DiagKind::Script, pos.clone(), msg));
                    Arc::from("")
                }
            },
            None => Arc::from(""),
        };
        if let Some(IconRef::Svg(name)) = icon
            && !is_svg_path(name)
            && !crate::icons::is_svg_icon_name(name)
        {
            let hint = crate::suggest::hint(name, crate::icons::SVG_ICON_NAMES.iter().copied());
            let msg = format!("unknown svg icon '{name}'{hint}");
            self.note(Some(instance), Diagnostic::warning(DiagKind::Script, pos.clone(), msg));
        }
        text
    }

    fn add_children(&mut self, children: &'a [Element], parent: usize, parent_hash: u64, instance: InstanceId) {
        for (i, (e, seg)) in children.iter().zip(child_segments(children)).enumerate() {
            if let Some(k) = &seg.duplicate_of {
                let msg = format!("duplicate key '{k}' among siblings; the later ones are matched by position");
                self.note(Some(instance), Diagnostic::warning(DiagKind::Script, e.pos.clone(), msg));
            }
            let hash = fnv(parent_hash, seg.text.as_bytes());
            self.path.push(i as u32);
            if e.kind.is_surface_root() {
                self.record_popup(e, NodeKey(hash), parent, instance);
            } else {
                self.add_element(e, hash, parent, instance, &[]);
            }
            self.path.pop();
        }
    }

    /// A popup, menu or tooltip element: kept for `Scene::popups`, never a node.
    fn record_popup(&mut self, e: &Element, key: NodeKey, parent: usize, instance: InstanceId) {
        let (Some(props), Some(tree)) = (e.popup.as_deref(), self.tree.clone()) else { return };
        self.recs.push(PopupRec {
            key,
            instance,
            kind: e.kind,
            props: props.clone(),
            anchor: self.nodes[parent].key,
            tree,
            path: Arc::from(self.path.as_slice()),
        });
    }

    /// One element and everything below it. `extra` classes are the engine's
    /// (the side of a popup root).
    fn add_element(&mut self, e: &'a Element, hash: u64, parent: usize, instance: InstanceId, extra: &[&str]) -> usize {
        let kind = node_kind(e.kind);
        let mut text = e.text.clone();
        if kind == ElementKind::Icon {
            text = Some(self.icon_text_of(&e.icon, &e.pos, instance));
        }
        let mut a = NodeArgs::new(NodeKey(hash), kind, Some(parent));
        a.instance = Some(instance);
        a.id = e.id.clone();
        a.classes = e.classes.clone();
        a.classes.extend(extra.iter().map(|c| Arc::<str>::from(*c)));
        a.text = if kind == ElementKind::MenuItem { None } else { text };
        a.image = if kind == ElementKind::Image { e.image.clone() } else { None };
        a.icon = if kind == ElementKind::Icon { e.icon.clone() } else { None };
        a.handlers = e.handlers;
        a.inline = inline_with_animation(e);
        a.inline_pos = e.pos.clone();
        a.direction = e.direction;
        a.restart_seq = e.animate.as_ref().map_or(0, |a| a.restart_seq);
        a.interaction = e.interaction;
        a.input = if kind == ElementKind::Input { Some(e.input.clone().unwrap_or_default()) } else { None };
        a.slider = if kind == ElementKind::Slider { Some(e.slider.clone().unwrap_or_default()) } else { None };
        let idx = self.push(a);
        match kind {
            ElementKind::MenuItem => {
                // The item's icon and label are children, so a theme styles
                // them like any other.
                if e.icon.is_some() {
                    let t = self.icon_text_of(&e.icon, &e.pos, instance);
                    let mut c = NodeArgs::new(NodeKey(fnv(hash, b"item:icon")), ElementKind::Icon, Some(idx));
                    c.instance = Some(instance);
                    c.text = Some(t);
                    c.icon = e.icon.clone();
                    self.push(c);
                }
                if let Some(label) = e.text.clone().filter(|t| !t.is_empty()) {
                    let mut c = NodeArgs::new(NodeKey(fnv(hash, b"item:label")), ElementKind::Text, Some(idx));
                    c.instance = Some(instance);
                    c.classes = vec![Arc::from("label")];
                    c.text = Some(label);
                    self.push(c);
                }
                self.add_children(&e.children, idx, hash, instance);
            }
            ElementKind::Box
            | ElementKind::Scroll
            | ElementKind::Slider
            | ElementKind::Popup
            | ElementKind::Menu
            | ElementKind::Tooltip => self.add_children(&e.children, idx, hash, instance),
            _ => {}
        }
        idx
    }

    fn add_plate(&mut self, parent: usize, instance: Option<InstanceId>, plate: &Plate) {
        let name = match instance {
            Some(i) => format!("plate:{}", i.0),
            None => "plate:bar".to_string(),
        };
        let mut a = NodeArgs::new(NodeKey(fixed_key(&name)), ElementKind::Plate, Some(parent));
        a.instance = instance;
        a.text = Some(Arc::from(plate.display_text()));
        self.push(a);
    }
}

/// The kind a script element is built as; the engine's own kinds cannot be made by a script.
fn node_kind(k: ElementKind) -> ElementKind {
    match k {
        ElementKind::Plate | ElementKind::Bar | ElementKind::Zone | ElementKind::Widget => ElementKind::Box,
        k => k,
    }
}

/// One child's step in its node key.
pub(super) struct Segment {
    pub text: String,
    /// The explicit key this child repeats (it is matched by position).
    pub duplicate_of: Option<String>,
}

/// The key steps of a list of siblings: an explicit `key`, else the kind and
/// the index among siblings of that kind. A key an earlier sibling has
/// already used falls back to the kind and index.
pub(super) fn child_segments(children: &[Element]) -> Vec<Segment> {
    let mut counts: HashMap<ElementKind, u32> = HashMap::new();
    let mut seen: HashSet<&str> = HashSet::new();
    children
        .iter()
        .map(|e| {
            let kind = node_kind(e.kind);
            let n = counts.entry(kind).or_insert(0);
            let index = *n;
            *n += 1;
            let by_position = format!("{}:{index}", kind.css_name());
            match e.key.as_deref() {
                Some(k) if seen.insert(k) => Segment { text: format!("k:{k}"), duplicate_of: None },
                Some(k) => Segment { text: by_position, duplicate_of: Some(k.to_string()) },
                None => Segment { text: by_position, duplicate_of: None },
            }
        })
        .collect()
}

/// The node key of the element at `path` below a widget's root element.
pub(super) fn path_hash(instance: InstanceId, root: &Element, path: &[u32]) -> Option<u64> {
    let mut hash = fnv(fixed_key("instance"), &instance.0.to_le_bytes());
    hash = fnv(hash, child_segments(std::slice::from_ref(root)).first()?.text.as_bytes());
    let mut el = root;
    for &i in path {
        let seg = child_segments(&el.children).into_iter().nth(i as usize)?;
        hash = fnv(hash, seg.text.as_bytes());
        el = el.children.get(i as usize)?;
    }
    Some(hash)
}

/// A path or an absolute file name rather than a bundled icon name.
pub(super) fn is_svg_path(s: &str) -> bool {
    s.contains(['/', '\\', ':']) || s.to_ascii_lowercase().ends_with(".svg")
}

/// The element's inline style, plus the declarations its `animate` prop
/// stands for.
fn inline_with_animation(e: &Element) -> Cow<'_, [(String, String)]> {
    let Some(a) = e.animate.as_deref() else { return Cow::Borrowed(&e.style) };
    let mut v = e.style.clone();
    v.push(("animation-name".into(), a.name.to_string()));
    for (name, value) in [
        ("animation-duration", &a.duration),
        ("animation-timing-function", &a.easing),
        ("animation-delay", &a.delay),
        ("animation-iteration-count", &a.iterations),
        ("animation-direction", &a.direction),
        ("animation-fill-mode", &a.fill),
    ] {
        if let Some(value) = value {
            v.push((name.into(), value.to_string()));
        }
    }
    Cow::Owned(v)
}

/// True when a widget's tree draws nothing: its root is hidden by inline
/// style (`visible = false`).
fn root_hidden(root: &Element) -> bool {
    root.style.iter().rev().find(|(k, _)| k == "display").is_some_and(|(_, v)| v.trim().eq_ignore_ascii_case("none"))
}

/// A plate keeps only what `plate` rules may theme: colour, background,
/// border and font family. Everything else is the engine's. The text colour
/// is always opaque and a border is at most 3 px, so no theme can make the
/// text vanish.
fn plate_style(s: &ComputedStyle) -> ComputedStyle {
    let border = |w: f32| w.clamp(0.0, 3.0);
    ComputedStyle {
        color: Color { a: 1.0, ..s.color },
        background: s.background.clone(),
        border_width: Edges {
            top: border(s.border_width.top),
            right: border(s.border_width.right),
            bottom: border(s.border_width.bottom),
            left: border(s.border_width.left),
        },
        border_color: s.border_color,
        border_radius: s.border_radius,
        font_family: s.font_family.clone(),
        font_size: 12.0,
        font_weight: 600,
        padding: Edges { top: 2.0, right: 8.0, bottom: 2.0, left: 8.0 },
        cursor: crate::style::Cursor::Pointer,
        ..ComputedStyle::default()
    }
}

impl Scene {
    /// Rebuilds the arena from the stored trees and plates: styles (cached
    /// per node), transitions, and whether anything needs a new layout.
    /// Returns the number of nodes whose style was computed again.
    pub(super) fn refresh(&mut self, now: Instant) -> usize {
        let had_running = !self.running.is_empty() || self.anim_busy;
        if had_running {
            self.stamp = self.stamp.wrapping_add(1);
        }
        let sheet = self.sheet.clone();
        let bar = self.bar.clone();
        let trees = self.trees.clone();
        let popup_ctx = self.popup.as_ref().map(|p| PopupCtx {
            slot: p.spec.slot.clone(),
            content: p.content.clone(),
            side_class: p.spec.side.class(),
            blur: p.spec.blur,
        });
        let fx = FxView {
            focus: self.focus.node,
            window: self.focus.window,
            visible: self.focus.visible,
            open: self.popup.as_ref().is_some_and(|p| p.open),
            ..FxView::default()
        };
        let mut b = Builder {
            sheet: &sheet,
            env: StyleEnv { media: self.env.media, icon_font: self.env.icon_font },
            bar: &bar,
            hover: &self.hover,
            active: &self.active,
            old_cache: std::mem::take(&mut self.style_cache),
            new_cache: HashMap::new(),
            old_prev: &self.prev,
            new_prev: HashMap::new(),
            old_running: std::mem::take(&mut self.running),
            new_running: HashMap::new(),
            old_anims: std::mem::take(&mut self.anims),
            new_anims: HashMap::new(),
            nodes: Vec::new(),
            specs: Vec::new(),
            node_diags: Vec::new(),
            diags: Vec::new(),
            restyled: 0,
            layout_changed: false,
            anim_busy: false,
            now,
            state_time: self.state_time.unwrap_or(now).min(now),
            stamp: self.stamp,
            placeholder: Arc::new(ComputedStyle::default()),
            hidden_style: Arc::new(ComputedStyle { display_none: true, ..ComputedStyle::default() }),
            empty_vars: Arc::new(Vars::default()),
            fx,
            tree: None,
            path: Vec::new(),
            recs: Vec::new(),
        };

        // Bar, zones, widgets.
        let mut bar_classes: Vec<Arc<str>> = vec![Arc::from(bar.name.as_str())];
        if bar.floating {
            bar_classes.push(Arc::from("floating"));
        }
        if self.env.fullscreen {
            bar_classes.push(Arc::from("fullscreen"));
        }
        let mut a = NodeArgs::new(NodeKey(fixed_key("bar")), ElementKind::Bar, None);
        a.classes = bar_classes;
        let bar_idx = b.push(a);
        let root_idx;
        if let Some(popup) = &popup_ctx {
            // A popup surface: the bar and the widget it hangs from are
            // matched and inherited from, never laid out or drawn.
            root_idx = 2;
            let slot = &popup.slot;
            let mut a = NodeArgs::new(
                NodeKey(fixed_key(&format!("widget:{}", slot.instance.0))),
                ElementKind::Widget,
                Some(bar_idx),
            );
            a.instance = Some(slot.instance);
            a.id = Some(Arc::from(slot.id.as_str()));
            a.classes = vec![Arc::from(slot.class.as_str())];
            let widget = b.push(a);
            if let Some(c) = &popup.content
                && let Some(root) = c.tree.root.as_ref()
                && let Some((el, _)) = element_at(root, &c.path)
                && let Some(hash) = path_hash(c.tree.instance, root, &c.path)
            {
                b.tree = Some(c.tree.clone());
                b.path = c.path.to_vec();
                let mut extra = vec![popup.side_class];
                if popup.blur {
                    extra.push("blur");
                }
                b.add_element(el, hash, widget, c.tree.instance, &extra);
            }
        } else {
            root_idx = 0;
            let zones: [(&str, &Vec<SlotSpec>); 3] =
                [("left", &self.slots.left), ("center", &self.slots.center), ("right", &self.slots.right)];
            for (name, slots) in zones {
                let mut a =
                    NodeArgs::new(NodeKey(fixed_key(&format!("zone:{name}"))), ElementKind::Zone, Some(bar_idx));
                a.id = Some(Arc::from(name));
                let zone = b.push(a);
                if name == "left"
                    && let Some(p) = &self.bar_plate
                {
                    b.add_plate(zone, None, p);
                }
                for slot in slots {
                    let tree = trees.get(&slot.instance);
                    let plate = self.widget_plates.get(&slot.instance);
                    let root = tree.and_then(|t| t.root.as_ref());
                    let too_deep = root.is_some_and(|r| r.depth() > MAX_DEPTH);
                    if plate.is_none() && root.is_none() {
                        continue;
                    }
                    let stopped = plate.is_some() || too_deep;
                    let mut classes: Vec<Arc<str>> = vec![Arc::from(slot.class.as_str())];
                    if stopped {
                        classes.push(Arc::from("stopped"));
                    }
                    let wkey = fixed_key(&format!("widget:{}", slot.instance.0));
                    let empty = !stopped && root.is_some_and(root_hidden);
                    let mut a = NodeArgs::new(NodeKey(wkey), ElementKind::Widget, Some(zone));
                    a.instance = Some(slot.instance);
                    a.id = Some(Arc::from(slot.id.as_str()));
                    a.classes = classes;
                    a.hidden = empty;
                    let wrapper = b.push(a);
                    if let Some(p) = plate {
                        b.add_plate(wrapper, Some(slot.instance), p);
                    } else if too_deep {
                        let name = tree.map(|t| t.widget.to_string()).unwrap_or_default();
                        let msg = format!("{name} has more than {MAX_DEPTH} nested elements; fix it and save");
                        let diag = Diagnostic::error(DiagKind::Stopped, None, msg);
                        let plate = Plate::from_diagnostics(std::slice::from_ref(&diag)).expect("one diagnostic");
                        b.note(Some(slot.instance), diag);
                        b.add_plate(wrapper, Some(slot.instance), &plate);
                    } else if let Some(root) = root {
                        // The widget's own root: a path of its own below the wrapper.
                        let ih = fnv(fixed_key("instance"), &slot.instance.0.to_le_bytes());
                        let seg = &child_segments(std::slice::from_ref(root))[0];
                        let hash = fnv(ih, seg.text.as_bytes());
                        b.tree = tree.cloned();
                        b.path.clear();
                        if root.kind.is_surface_root() {
                            b.record_popup(root, NodeKey(hash), wrapper, slot.instance);
                        } else {
                            b.add_element(root, hash, wrapper, slot.instance, &[]);
                        }
                    }
                }
            }
        }
        b.prepare_fx(&self.anchor_open, root_idx);
        b.style_all();

        // A name belongs to the first popup that declares it.
        let mut recs = std::mem::take(&mut b.recs);
        let mut claimed: HashSet<Arc<str>> = HashSet::new();
        for r in &mut recs {
            let mut lost = Vec::new();
            r.props.names.retain(|n| {
                let first = claimed.insert(n.clone());
                if !first {
                    lost.push(n.clone());
                }
                first
            });
            for n in lost {
                let msg = format!("popup name '{n}' is already used by an earlier popup; this one stays unnamed");
                b.diags.push((usize::MAX, Some(r.instance), Diagnostic::warning(DiagKind::Script, None, msg)));
            }
        }

        // Anything that went away or arrived changes the layout.
        let nodes = std::mem::take(&mut b.nodes);
        let layout_changed = b.layout_changed || b.new_prev.len() != self.prev.len();
        let restyled = b.restyled;
        let anim_busy = b.anim_busy;
        let change_time = b.state_time;
        let mut diags = std::mem::take(&mut b.diags);
        diags.sort_by_key(|(order, _, _)| *order);
        let new_cache = std::mem::take(&mut b.new_cache);
        let new_prev = std::mem::take(&mut b.new_prev);
        let new_running = std::mem::take(&mut b.new_running);
        let new_anims = std::mem::take(&mut b.new_anims);
        drop(b);
        let mut nodes = nodes;
        if !self.layout_dirty && !layout_changed {
            // Nothing moved: the new nodes keep last frame's boxes.
            let old: HashMap<NodeKey, &Laid> = self.arena.iter().map(|n| (n.key, &n.laid)).collect();
            for n in &mut nodes {
                if let Some(l) = old.get(&n.key) {
                    n.laid = (*l).clone();
                }
            }
        }
        self.arena = nodes;
        self.root_idx = root_idx;
        self.recs = recs;
        self.sync_states(now);
        self.update_transforms();
        self.style_cache = new_cache;
        self.prev = new_prev;
        self.running = new_running;
        self.anims = new_anims;
        self.anim_busy = anim_busy;
        self.change_time = change_time;
        self.state_time = None;
        if layout_changed {
            self.layout_dirty = true;
        }
        // A warning is reported once while it keeps being produced, and again
        // if it goes away and comes back (or the style sheet changes).
        let mut current = HashSet::new();
        for (_, inst, d) in diags {
            let key = (inst, format!("{}|{:?}", d.message, d.pos));
            if current.insert(key.clone()) && !self.diag_seen.contains(&key) {
                self.diags.push(super::SceneDiagnostic { instance: inst, diagnostic: d });
            }
        }
        self.diag_seen = current;
        // An element took focus while the tree was synced: match the styles again with it focused.
        if std::mem::take(&mut self.restyle_again) {
            return restyled.max(self.refresh(now));
        }
        restyled
    }
}

//! Building the node arena: the skeleton, the widget trees, node keys,
//! style (with a per-node cache) and transitions.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Instant;

use super::trans::{self, Running};
use super::{Laid, Node, Prev, Scene, SlotSpec, icon_text};
use crate::color::Color;
use crate::config::BarSpec;
use crate::css::cascade::{self, NodeInput, StyleEnv, Vars};
use crate::css::parse_inline;
use crate::css::select::MatchTree;
use crate::diag::{DiagKind, Diagnostic, Plate, SourcePos};
use crate::element::{Element, ElementKind, Handlers, ImageRef, InstanceId, MAX_DEPTH, NodeKey};
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

fn fixed_key(name: &str) -> u64 {
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

struct ArenaTree<'a> {
    nodes: &'a [Node],
    hover: &'a HashSet<NodeKey>,
    active: &'a HashSet<NodeKey>,
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
}

struct Spec<'a> {
    key: NodeKey,
    kind: ElementKind,
    instance: Option<InstanceId>,
    parent: Option<usize>,
    id: Option<Arc<str>>,
    classes: Vec<Arc<str>>,
    text: Option<Arc<str>>,
    image: Option<ImageRef>,
    handlers: Handlers,
    inline: &'a [(String, String)],
    inline_pos: Option<SourcePos>,
    direction: FlexDirection,
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
    nodes: Vec<Node>,
    diags: Vec<(Option<InstanceId>, Diagnostic)>,
    restyled: usize,
    layout_changed: bool,
    now: Instant,
    state_time: Instant,
    stamp: u64,
    placeholder: Arc<ComputedStyle>,
    empty_vars: Arc<Vars>,
}

impl Builder<'_> {
    fn add(&mut self, spec: Spec) -> usize {
        let idx = self.nodes.len();
        let parent_sig = spec.parent.map(|p| (self.nodes[p].base_sig, self.nodes[p].animating));
        let (parent_style, parent_vars) = match spec.parent {
            Some(p) => (Some(self.nodes[p].style.clone()), self.nodes[p].vars.clone()),
            None => (None, self.empty_vars.clone()),
        };

        // Inputs that decide the style.
        let mut h = Fnv::new();
        spec.kind.hash(&mut h);
        spec.id.hash(&mut h);
        spec.classes.hash(&mut h);
        self.hover.contains(&spec.key).hash(&mut h);
        self.active.contains(&spec.key).hash(&mut h);
        spec.inline.hash(&mut h);
        spec.inline_pos.hash(&mut h);
        (spec.direction == FlexDirection::Column).hash(&mut h);
        if spec.kind == ElementKind::Bar {
            self.bar.radius.hash(&mut h);
        }
        h.write_u64(parent_sig.map_or(0, |p| p.0));
        let base_sig = h.finish();
        let sig = fnv(base_sig, &[u8::from(parent_sig.is_some_and(|p| p.1))])
            ^ if parent_sig.is_some_and(|p| p.1) { self.stamp } else { 0 };

        // The node goes in first so selectors can see it.
        let depth = spec.parent.map_or(0, |p| self.nodes[p].depth + 1);
        self.nodes.push(Node {
            key: spec.key,
            kind: spec.kind,
            instance: spec.instance,
            parent: spec.parent,
            children: Vec::new(),
            depth,
            id: spec.id.clone(),
            classes: spec.classes.clone(),
            text: spec.text.clone(),
            image: spec.image.clone(),
            handlers: spec.handlers,
            target: self.placeholder.clone(),
            style: self.placeholder.clone(),
            vars: parent_vars.clone(),
            animating: false,
            sig,
            base_sig,
            laid: Laid::default(),
        });
        if let Some(p) = spec.parent {
            self.nodes[p].children.push(idx);
        }

        let (target, vars, style_diags) = match self.old_cache.remove(&spec.key) {
            Some(c) if c.sig == sig => (c.target, c.vars, c.diags),
            _ => {
                self.restyled += 1;
                let mut diags = Vec::new();
                let inline = if spec.inline.is_empty() {
                    None
                } else {
                    let (decls, d) = parse_inline(spec.inline, spec.inline_pos.clone());
                    diags.extend(d);
                    Some(decls)
                };
                let input = NodeInput {
                    kind: spec.kind,
                    parent: parent_style.as_deref(),
                    parent_vars: &parent_vars,
                    inline: inline.as_ref(),
                    initial_radius: (spec.kind == ElementKind::Bar).then(|| super::bar_radius_px(self.bar)),
                    initial_direction: spec.direction,
                };
                let tree = ArenaTree { nodes: &self.nodes, hover: self.hover, active: self.active };
                let (mut style, vars) = cascade::compute(self.sheet, &tree, idx, &self.env, &input, &mut diags);
                if spec.kind == ElementKind::Plate {
                    style = plate_style(&style);
                }
                if spec.kind == ElementKind::Bar && !style.box_shadow.is_empty() {
                    diags.push(Diagnostic::warning(
                        DiagKind::Style,
                        None,
                        "box-shadow on bar is cut off by the window edge",
                    ));
                }
                let diags = diags
                    .into_iter()
                    .map(|d| {
                        if d.pos.is_none() && spec.inline_pos.is_some() {
                            Diagnostic { pos: spec.inline_pos.clone(), ..d }
                        } else {
                            d
                        }
                    })
                    .collect();
                (Arc::new(style), vars, diags)
            }
        };
        for d in &style_diags {
            self.diags.push((spec.instance, d.clone()));
        }
        self.new_cache.insert(spec.key, Cached { sig, target: target.clone(), vars: vars.clone(), diags: style_diags });

        // Transitions.
        let mut runs = self.old_running.remove(&spec.key).unwrap_or_default();
        let prev = self.old_prev.get(&spec.key);
        if let Some(p) = prev
            && p.base_sig != base_sig
            && spec.kind != ElementKind::Plate
        {
            trans::detect(&p.target, &p.display, &target, &mut runs, self.state_time);
        }
        let (display, runs) = trans::evaluate(&target, runs, self.now);
        let animating = !runs.is_empty() || parent_sig.is_some_and(|p| p.1);
        match prev {
            None => self.layout_changed = true,
            Some(p) if !trans::layout_eq(&p.display, &display) => self.layout_changed = true,
            _ => {}
        }
        if !runs.is_empty() {
            self.new_running.insert(spec.key, runs);
        }
        self.new_prev.insert(spec.key, Prev { target: target.clone(), display: display.clone(), base_sig });
        let n = &mut self.nodes[idx];
        n.target = target;
        n.style = display;
        n.vars = vars;
        n.animating = animating;
        idx
    }

    fn add_children(&mut self, children: &[Element], parent: usize, parent_hash: u64, instance: InstanceId) {
        let mut counts: HashMap<ElementKind, u32> = HashMap::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for e in children {
            let kind = match e.kind {
                ElementKind::Text | ElementKind::Icon | ElementKind::Image | ElementKind::Box => e.kind,
                _ => ElementKind::Box,
            };
            let n = counts.entry(kind).or_insert(0);
            let index = *n;
            *n += 1;
            let seg = match e.key.as_deref() {
                Some(k) if seen.insert(k) => format!("k:{k}"),
                Some(k) => {
                    let msg = format!("duplicate key '{k}' among siblings; the later ones are matched by position");
                    self.diags.push((Some(instance), Diagnostic::warning(DiagKind::Script, e.pos.clone(), msg)));
                    format!("{}:{index}", kind.css_name())
                }
                None => format!("{}:{index}", kind.css_name()),
            };
            let hash = fnv(parent_hash, seg.as_bytes());
            let mut text = e.text.clone();
            if kind == ElementKind::Icon {
                text = match &e.icon {
                    Some(icon) => match icon_text(icon) {
                        Some(t) => Some(Arc::from(t)),
                        None => {
                            let name = match icon {
                                crate::element::IconRef::Named(n) => n.to_string(),
                                crate::element::IconRef::Glyph(g) => g.to_string(),
                            };
                            let hint = crate::suggest::hint(&name, crate::icons::ICON_NAMES.iter().copied());
                            let msg = format!("unknown icon '{name}'{hint}");
                            self.diags
                                .push((Some(instance), Diagnostic::warning(DiagKind::Script, e.pos.clone(), msg)));
                            Some(Arc::from(""))
                        }
                    },
                    None => Some(Arc::from("")),
                };
            }
            let idx = self.add(Spec {
                key: NodeKey(hash),
                kind,
                instance: Some(instance),
                parent: Some(parent),
                id: e.id.clone(),
                classes: e.classes.clone(),
                text,
                image: if kind == ElementKind::Image { e.image.clone() } else { None },
                handlers: e.handlers,
                inline: &e.style,
                inline_pos: e.pos.clone(),
                direction: e.direction,
            });
            if kind == ElementKind::Box {
                self.add_children(&e.children, idx, hash, instance);
            }
        }
    }

    fn add_plate(&mut self, parent: usize, instance: Option<InstanceId>, plate: &Plate) {
        let name = match instance {
            Some(i) => format!("plate:{}", i.0),
            None => "plate:bar".to_string(),
        };
        self.add(Spec {
            key: NodeKey(fixed_key(&name)),
            kind: ElementKind::Plate,
            instance,
            parent: Some(parent),
            id: None,
            classes: Vec::new(),
            text: Some(Arc::from(plate.display_text())),
            image: None,
            handlers: Handlers::default(),
            inline: &[],
            inline_pos: None,
            direction: FlexDirection::Row,
        });
    }
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
        let had_running = !self.running.is_empty();
        if had_running {
            self.stamp = self.stamp.wrapping_add(1);
        }
        let sheet = self.sheet.clone();
        let bar = self.bar.clone();
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
            nodes: Vec::new(),
            diags: Vec::new(),
            restyled: 0,
            layout_changed: false,
            now,
            state_time: self.state_time.unwrap_or(now).min(now),
            stamp: self.stamp,
            placeholder: Arc::new(ComputedStyle::default()),
            empty_vars: Arc::new(Vars::default()),
        };

        // Bar, zones, widgets.
        let mut bar_classes: Vec<Arc<str>> = vec![Arc::from(bar.name.as_str())];
        if bar.floating {
            bar_classes.push(Arc::from("floating"));
        }
        if self.env.fullscreen {
            bar_classes.push(Arc::from("fullscreen"));
        }
        let bar_idx = b.add(Spec {
            key: NodeKey(fixed_key("bar")),
            kind: ElementKind::Bar,
            instance: None,
            parent: None,
            id: None,
            classes: bar_classes,
            text: None,
            image: None,
            handlers: Handlers::default(),
            inline: &[],
            inline_pos: None,
            direction: FlexDirection::Row,
        });
        let zones: [(&str, &Vec<SlotSpec>); 3] =
            [("left", &self.slots.left), ("center", &self.slots.center), ("right", &self.slots.right)];
        for (name, slots) in zones {
            let zone = b.add(Spec {
                key: NodeKey(fixed_key(&format!("zone:{name}"))),
                kind: ElementKind::Zone,
                instance: None,
                parent: Some(bar_idx),
                id: Some(Arc::from(name)),
                classes: Vec::new(),
                text: None,
                image: None,
                handlers: Handlers::default(),
                inline: &[],
                inline_pos: None,
                direction: FlexDirection::Row,
            });
            if name == "left"
                && let Some(p) = &self.bar_plate
            {
                b.add_plate(zone, None, p);
            }
            for slot in slots {
                let tree = self.trees.get(&slot.instance);
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
                let wrapper = b.add(Spec {
                    key: NodeKey(wkey),
                    kind: ElementKind::Widget,
                    instance: Some(slot.instance),
                    parent: Some(zone),
                    id: Some(Arc::from(slot.id.as_str())),
                    classes,
                    text: None,
                    image: None,
                    handlers: Handlers::default(),
                    inline: &[],
                    inline_pos: None,
                    direction: FlexDirection::Row,
                });
                if let Some(p) = plate {
                    b.add_plate(wrapper, Some(slot.instance), p);
                } else if too_deep {
                    let name = tree.map(|t| t.widget.to_string()).unwrap_or_default();
                    let msg = format!("{name} has more than {MAX_DEPTH} nested elements; fix it and save");
                    let diag = Diagnostic::error(DiagKind::Stopped, None, msg);
                    let plate = Plate::from_diagnostics(std::slice::from_ref(&diag)).expect("one diagnostic");
                    b.diags.push((Some(slot.instance), diag));
                    b.add_plate(wrapper, Some(slot.instance), &plate);
                } else if let Some(root) = root {
                    // The widget's own root: a path of its own below the wrapper.
                    let ih = fnv(fixed_key("instance"), &slot.instance.0.to_le_bytes());
                    let one = std::slice::from_ref(root);
                    b.add_children(one, wrapper, ih, slot.instance);
                }
            }
        }

        // Anything that went away or arrived changes the layout.
        let nodes = std::mem::take(&mut b.nodes);
        let layout_changed = b.layout_changed || b.new_prev.len() != self.prev.len();
        let restyled = b.restyled;
        let diags = std::mem::take(&mut b.diags);
        let new_cache = std::mem::take(&mut b.new_cache);
        let new_prev = std::mem::take(&mut b.new_prev);
        let new_running = std::mem::take(&mut b.new_running);
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
        self.style_cache = new_cache;
        self.prev = new_prev;
        self.running = new_running;
        self.state_time = None;
        if layout_changed {
            self.layout_dirty = true;
        }
        // A warning is reported once while it keeps being produced, and again
        // if it goes away and comes back (or the style sheet changes).
        let mut current = HashSet::new();
        for (inst, d) in diags {
            let key = (inst, format!("{}|{:?}", d.message, d.pos));
            if current.insert(key.clone()) && !self.diag_seen.contains(&key) {
                self.diags.push(super::SceneDiagnostic { instance: inst, diagnostic: d });
            }
        }
        self.diag_seen = current;
        restyled
    }
}

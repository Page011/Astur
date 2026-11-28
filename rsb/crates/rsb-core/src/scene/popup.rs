//! Popup, menu and tooltip surfaces: the popups a scene finds in its trees,
//! and the scene of one popup surface (its tree, open state, natural size).

use std::sync::Arc;
use std::time::Instant;

use super::{PopupSurfaceSpec, Scene};
use crate::draw::shadow_bounds;
use crate::element::{Element, ElementKind, InstanceId, NodeKey, WidgetTree};
use crate::geom::{PointF, RectF, SizeF};
use crate::layout::TextMeasure;
use crate::popup::{PopupDecl, PopupProps, Side};

/// A popup element a scene's trees declare.
#[derive(Clone)]
pub(super) struct PopupRec {
    pub key: NodeKey,
    pub instance: InstanceId,
    pub kind: ElementKind,
    pub props: PopupProps,
    pub anchor: NodeKey,
    pub tree: Arc<WidgetTree>,
    pub path: Arc<[u32]>,
}

/// The tree a popup scene shows.
#[derive(Clone)]
pub(super) struct Content {
    pub tree: Arc<WidgetTree>,
    pub path: Arc<[u32]>,
}

/// State only a popup scene has.
pub(super) struct PopupScene {
    pub spec: PopupSurfaceSpec,
    pub open: bool,
    /// Has been open since it was made: `set_open(false)` after that starts a
    /// close, `set_open(false)` before it changes nothing.
    pub opened: bool,
    pub closing: bool,
    /// Physical px the root must take across (an attached popup).
    pub width: Option<f32>,
    pub content: Option<Content>,
    /// The monitor size an overlay's root is laid out at (the geometry the
    /// scene was made with).
    pub monitor: SizeF,
    /// Props of the popup element at the root, and whether it is the menu
    /// of a menu item (a submenu).
    pub root_props: Option<PopupProps>,
    pub submenu: bool,
    pub opened_at: Option<Instant>,
    /// The scene was settled at least once while open.
    pub settled_seen: bool,
    /// A press went down before the overlay's input guard lifted: it is
    /// dropped together with its release.
    pub dropped_press: bool,
    /// Where the root's border box starts in the surface.
    pub offset: PointF,
    /// A throw-away scene that measures the open, settled popup.
    pub probe: bool,
}

/// The settled size of a popup surface.
#[derive(Clone, Copy)]
pub(super) struct Natural {
    pub size: SizeF,
    pub offset: PointF,
    /// The root's own border box (what is placed against the anchor), inside `size`.
    pub boxed: SizeF,
}

/// The element at `path` below the widget root, and its parent.
pub(super) fn element_at<'a>(root: &'a Element, path: &[u32]) -> Option<(&'a Element, Option<&'a Element>)> {
    let mut el = root;
    let mut parent = None;
    for &i in path {
        parent = Some(el);
        el = el.children.get(i as usize)?;
    }
    Some((el, parent))
}

impl Scene {
    pub(super) fn new_popup_impl(spec: PopupSurfaceSpec, geometry: super::BarGeometry, env: super::SceneEnv) -> Scene {
        let mut s = Scene::new(&spec.bar, geometry, env);
        let monitor = SizeF { w: geometry.width_px as f32, h: geometry.height_px as f32 };
        s.popup = Some(PopupScene {
            spec,
            open: false,
            opened: false,
            closing: false,
            width: None,
            content: None,
            monitor,
            root_props: None,
            submenu: false,
            opened_at: None,
            settled_seen: false,
            dropped_press: false,
            offset: PointF::default(),
            probe: false,
        });
        s.root_idx = 2;
        s
    }

    pub(super) fn set_popup_tree_impl(&mut self, tree: Arc<WidgetTree>, path: Arc<[u32]>) {
        let Some(p) = &mut self.popup else { return };
        if p.closing {
            return;
        }
        if let Some(c) = &p.content
            && c.tree.instance == tree.instance
            && c.tree.version > tree.version
        {
            return;
        }
        let found = tree.root.as_ref().and_then(|r| element_at(r, &path));
        p.root_props = found.and_then(|(e, _)| e.popup.as_deref().cloned());
        p.submenu = found.is_some_and(|(_, parent)| parent.is_some_and(|e| e.kind == ElementKind::MenuItem));
        p.content = Some(Content { tree, path });
        self.touch();
    }

    pub(super) fn set_popup_surface_impl(&mut self, side: Side, blur: bool, width: Option<f32>) {
        let Some(p) = &mut self.popup else { return };
        if p.spec.side == side && p.spec.blur == blur && p.width == width {
            return;
        }
        p.spec.side = side;
        p.spec.blur = blur;
        p.width = width;
        self.touch();
    }

    pub(super) fn set_open_impl(&mut self, open: bool, now: Instant) {
        let Some(p) = &mut self.popup else { return };
        if p.open == open {
            return;
        }
        p.open = open;
        if open {
            p.closing = false;
            p.opened = true;
            p.opened_at = Some(now);
            p.settled_seen = false;
            p.dropped_press = false;
        } else if p.opened {
            p.closing = true;
        }
        self.state_time = Some(now);
        self.rebuild = true;
    }

    /// True while a closing popup keeps drawing but takes no input.
    pub(super) fn is_closing(&self) -> bool {
        self.popup.as_ref().is_some_and(|p| p.closing)
    }

    /// An overlay ignores presses and keys until its guard lifts: once its
    /// opening has settled, or at the latest after `OVERLAY_INPUT_GUARD_CAP`
    /// (an animation that never ends must not keep the screen deaf).
    pub(super) fn overlay_guarded(&self, now: Instant) -> bool {
        let Some(p) = &self.popup else { return false };
        if p.spec.side != Side::Screen {
            return false;
        }
        let age = p.opened_at.map(|t| now.saturating_duration_since(t));
        let waited = age.is_some_and(|a| a >= crate::popup::OVERLAY_INPUT_GUARD);
        let capped = age.is_some_and(|a| a >= crate::popup::OVERLAY_INPUT_GUARD_CAP);
        !(waited && (p.settled_seen || capped))
    }

    /// An overlay's Escape works as soon as the short guard has passed, even
    /// while its opening animation is still running.
    pub(super) fn overlay_escape_ready(&self, now: Instant) -> bool {
        let Some(p) = &self.popup else { return false };
        p.opened_at.is_some_and(|t| now.saturating_duration_since(t) >= crate::popup::OVERLAY_INPUT_GUARD)
    }

    /// The surface size and the root's offset in it, measured on a copy of
    /// this scene that is open and settled.
    pub(super) fn natural(&mut self, text: &mut dyn TextMeasure) -> Natural {
        if let Some(n) = self.natural {
            return n;
        }
        let n = self.probe(text);
        self.natural = Some(n);
        if let Some(p) = &mut self.popup {
            p.offset = n.offset;
        }
        n
    }

    fn probe(&self, text: &mut dyn TextMeasure) -> Natural {
        let zero = Natural { size: SizeF::default(), offset: PointF::default(), boxed: SizeF::default() };
        let Some(p) = &self.popup else { return zero };
        if p.probe || p.content.is_none() {
            return zero;
        }
        let mut s = Scene::new_popup_impl(p.spec.clone(), self.geometry, self.env.clone());
        s.sheet = self.sheet.clone();
        s.image_sizes = self.image_sizes.clone();
        s.image_failed = self.image_failed.clone();
        s.snap_sizes = true;
        if let Some(q) = &mut s.popup {
            q.content = p.content.clone();
            q.width = p.width;
            q.monitor = p.monitor;
            q.root_props = p.root_props.clone();
            q.submenu = p.submenu;
            q.open = true;
            q.opened = true;
            q.probe = true;
        }
        s.frame(self.now, text);
        let overlay = p.spec.side == Side::Screen;
        let u = s.settled_bounds(overlay);
        if u.is_empty() {
            return zero;
        }
        let (x0, y0) = (u.x.floor(), u.y.floor());
        let (x1, y1) = (u.right().ceil(), u.bottom().ceil());
        let root = s.arena.get(s.root_idx).map_or_else(RectF::default, |n| n.laid.target);
        Natural {
            size: SizeF { w: x1 - x0, h: y1 - y0 },
            offset: PointF { x: -x0, y: -y0 },
            boxed: SizeF { w: root.w, h: root.h },
        }
    }

    /// What the popup draws once every transition has ended, shadows
    /// included, layout px. An overlay's own root is left out.
    fn settled_bounds(&self, overlay: bool) -> RectF {
        let Some(root) = self.arena.get(self.root_idx) else { return RectF::default() };
        if root.target.display_none {
            return RectF::default();
        }
        self.bounds_of(self.root_idx, !overlay)
    }

    fn bounds_of(&self, idx: usize, include_self: bool) -> RectF {
        let n = &self.arena[idx];
        if n.target.display_none {
            return RectF::default();
        }
        let scale = self.geometry.scale;
        let r = n.laid.target;
        let mut u = RectF::default();
        if include_self {
            u = r;
            for sh in &n.target.box_shadow {
                if sh.color.a <= 0.0 {
                    continue;
                }
                let spread = sh.spread * scale;
                let s = RectF::new(
                    r.x + sh.offset_x * scale - spread,
                    r.y + sh.offset_y * scale - spread,
                    r.w + 2.0 * spread,
                    r.h + 2.0 * spread,
                );
                u = u.union(&shadow_bounds(s, sh.blur * scale));
            }
            // A clipping box cuts what its children draw.
            if n.target.clip {
                return u;
            }
        }
        for &c in &n.children {
            u = u.union(&self.bounds_of(c, true));
        }
        u
    }

    pub(super) fn root_offset_impl(&self) -> PointF {
        self.popup.as_ref().map_or_else(PointF::default, |p| p.offset)
    }

    pub(super) fn root_outline_impl(&self) -> Vec<PointF> {
        let Some(root) = self.arena.get(self.root_idx) else { return Vec::new() };
        if self.popup.is_none() || root.laid.rect.is_empty() {
            return Vec::new();
        }
        self.outline_of(root.key)
    }

    pub(super) fn overlay_dim_impl(&self) -> Option<crate::color::Color> {
        let p = self.popup.as_ref()?;
        if p.spec.side != Side::Screen {
            return None;
        }
        let root = self.arena.get(self.root_idx)?;
        match &root.style.background {
            crate::style::Background::Color(c) => Some(crate::color::Color { a: c.a * root.style.opacity, ..*c }),
            _ => None,
        }
    }

    pub(super) fn declares_popups_impl(&self) -> bool {
        !self.recs.is_empty()
    }

    pub(super) fn popups_impl(&self) -> Vec<PopupDecl> {
        let scale_shift = |key: NodeKey| -> Option<(RectF, (f32, f32))> {
            let n = self.arena.iter().find(|n| n.key == key && !n.style.display_none)?;
            Some((n.laid.target, n.laid.scroll))
        };
        self.recs
            .iter()
            .filter_map(|r| {
                let (rect, shift) = scale_shift(r.anchor)?;
                let anchor_rect = RectF::new(rect.x - shift.0, rect.y - shift.1, rect.w, rect.h);
                let anchor_flat = self.flat_edges_of(r.anchor).unwrap_or_default();
                let by_script = r.props.open && self.dismissed.get(&r.key).is_none_or(|d| r.props.open_seq > *d);
                let open = by_script || self.sub_open.contains(&r.key);
                Some(PopupDecl {
                    key: r.key,
                    instance: r.instance,
                    kind: r.kind,
                    props: r.props.clone(),
                    anchor: r.anchor,
                    anchor_rect,
                    anchor_flat,
                    tree: r.tree.clone(),
                    path: r.path.clone(),
                    open,
                })
            })
            .collect()
    }

    pub(super) fn dismiss_popup_impl(&mut self, key: NodeKey, open_seq: u64) {
        let e = self.dismissed.entry(key).or_insert(0);
        *e = (*e).max(open_seq);
        self.sub_open.remove(&key);
    }

    pub(super) fn set_anchor_open_impl(&mut self, key: NodeKey, open: bool, now: Instant) {
        let changed = if open { self.anchor_open.insert(key) } else { self.anchor_open.remove(&key) };
        if changed {
            self.state_time = Some(now);
            self.rebuild = true;
        }
    }

    /// The tooltip element of the deepest hovered node that has one.
    pub(super) fn hovered_tooltip_impl(&self) -> Option<NodeKey> {
        self.hover_chain
            .iter()
            .rev()
            .find_map(|k| self.recs.iter().find(|r| r.kind == ElementKind::Tooltip && r.anchor == *k).map(|r| r.key))
    }
}

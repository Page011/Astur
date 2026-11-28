//! Shared helpers for the scene and style tests.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use rsb_core::config::{BarSpec, WidgetRef, default_config};
use rsb_core::css::{MediaEnv, Stylesheet};
use rsb_core::diag::Diagnostic;
use rsb_core::draw::{DrawCmd, DrawItem};
use rsb_core::element::{Element, ElementKind, HandlerId, Handlers, InstanceId, WidgetTree};
use rsb_core::geom::RectF;
use rsb_core::icons::IconFont;
use rsb_core::layout::FixedMeasure;
use rsb_core::scene::{BarGeometry, FrameOut, NodeInfo, Scene, SceneEnv, SlotSpec, ZoneSlots};

pub fn bar() -> BarSpec {
    default_config().bars.remove(0)
}

pub fn env(dark: bool) -> SceneEnv {
    SceneEnv { media: MediaEnv { dark }, icon_font: IconFont::Fluent, fullscreen: false }
}

pub fn slot(n: u32, class: &str) -> SlotSpec {
    SlotSpec { instance: InstanceId(n), class: class.into(), id: class.into() }
}

pub fn tree(n: u32, version: u64, root: Option<Element>) -> Arc<WidgetTree> {
    Arc::new(WidgetTree { instance: InstanceId(n), widget: WidgetRef::parse("w").unwrap(), version, root, grant: None })
}

pub fn text(s: &str) -> Element {
    let mut e = Element::new(ElementKind::Text);
    e.text = Some(Arc::from(s));
    e
}

pub fn boxed(children: Vec<Element>) -> Element {
    let mut e = Element::new(ElementKind::Box);
    e.children = children;
    e
}

pub fn classed(mut e: Element, classes: &[&str]) -> Element {
    e.classes = classes.iter().map(|c| Arc::from(*c)).collect();
    e
}

pub fn click(mut e: Element, id: u32) -> Element {
    e.handlers.click = Some(HandlerId(id));
    e
}

pub fn handlers(mut e: Element, h: Handlers) -> Element {
    e.handlers = h;
    e
}

pub fn sheet(css: &str) -> Arc<Stylesheet> {
    let (s, d) = Stylesheet::parse(css, "style.css");
    assert!(d.is_empty(), "unexpected diagnostics: {d:?}");
    Arc::new(s)
}

/// A scene with `css`, one widget per zone as given (left, center, right).
pub fn scene_with(
    css: &str,
    width: u32,
    scale: f32,
    left: Option<Element>,
    center: Option<Element>,
    right: Option<Element>,
) -> Scene {
    let mut b = bar();
    b.height = 32;
    let h = (32.0 * scale).round() as u32;
    let mut s = Scene::new(&b, BarGeometry { width_px: width, height_px: h, scale }, env(true));
    s.set_stylesheet(sheet(css));
    let mut slots = ZoneSlots::default();
    if left.is_some() {
        slots.left.push(slot(1, "l"));
    }
    if center.is_some() {
        slots.center.push(slot(2, "c"));
    }
    if right.is_some() {
        slots.right.push(slot(3, "r"));
    }
    s.set_slots(slots);
    for (n, e) in [(1, left), (2, center), (3, right)] {
        if let Some(e) = e {
            s.set_tree(tree(n, 1, Some(e)));
        }
    }
    s
}

pub fn frame(s: &mut Scene, now: Instant) -> FrameOut {
    s.frame(now, &mut FixedMeasure)
}

pub fn node_by_class(s: &Scene, class: &str) -> NodeInfo {
    s.nodes()
        .into_iter()
        .find(|n| n.classes.iter().any(|c| c.as_ref() == class))
        .unwrap_or_else(|| panic!("no node with class {class}"))
}

pub fn node_by_id(s: &Scene, id: &str) -> NodeInfo {
    s.nodes().into_iter().find(|n| n.id.as_deref() == Some(id)).unwrap_or_else(|| panic!("no node with id {id}"))
}

pub fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

pub fn texts(items: &[DrawItem]) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| match &i.cmd {
            DrawCmd::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect()
}

pub fn rect_of(i: &DrawItem) -> Option<RectF> {
    match &i.cmd {
        DrawCmd::Fill { rect, .. } | DrawCmd::Text { rect, .. } | DrawCmd::Border { rect, .. } => Some(*rect),
        _ => None,
    }
}

pub fn diag_text(d: &[Diagnostic]) -> Vec<String> {
    d.iter().map(|d| d.to_string()).collect()
}

// ---- popups, inputs and friends ----

use rsb_core::element::{InputProps, SliderProps};
use rsb_core::popup::{PopupDecl, PopupProps, Side};
use rsb_core::scene::PopupSurfaceSpec;

pub fn popup_el(props: PopupProps, kind: ElementKind, children: Vec<Element>) -> Element {
    let mut e = Element::new(kind);
    e.popup = Some(Box::new(props));
    e.children = children;
    e
}

pub fn open_props(seq: u64) -> PopupProps {
    PopupProps { open: true, open_seq: seq, ..PopupProps::default() }
}

pub fn input_el(value: &str) -> Element {
    let mut e = Element::new(ElementKind::Input);
    e.input = Some(Box::new(InputProps { value: Arc::from(value), ..InputProps::default() }));
    e
}

pub fn slider_el(props: SliderProps) -> Element {
    let mut e = Element::new(ElementKind::Slider);
    e.slider = Some(Box::new(props));
    e
}

pub fn kind_el(kind: ElementKind, children: Vec<Element>) -> Element {
    let mut e = Element::new(kind);
    e.children = children;
    e
}

pub fn focusable(mut e: Element) -> Element {
    e.interaction.focusable = true;
    e
}

pub fn spec(side: Side) -> PopupSurfaceSpec {
    PopupSurfaceSpec { bar: bar(), slot: slot(1, "l"), kind: ElementKind::Popup, side, blur: false }
}

/// A bar scene with one widget: `root > .anchor > popup(content)`.
pub fn bar_with_popup(css: &str, props: PopupProps, content: Vec<Element>) -> (Scene, Arc<WidgetTree>) {
    let popup = popup_el(props, ElementKind::Popup, content);
    let anchor = classed(boxed(vec![popup]), &["anchor"]);
    let root = classed(boxed(vec![anchor]), &["root"]);
    let mut s = scene_with(css, 400, 1.0, Some(root), None, None);
    frame(&mut s, Instant::now());
    let tr = tree_of(&s);
    (s, tr)
}

/// The tree the scene was given for instance 1, rebuilt from `decls`.
fn tree_of(s: &Scene) -> Arc<WidgetTree> {
    s.popups().first().map(|d| d.tree.clone()).unwrap_or_else(|| tree(1, 1, None))
}

pub fn decls(s: &Scene) -> Vec<PopupDecl> {
    s.popups()
}

/// A popup scene showing the popup `decl` declares.
pub fn popup_scene(css: &str, decl: &PopupDecl, side: Side, w: u32, h: u32) -> Scene {
    let kind = decl.kind;
    let mut sp = spec(side);
    sp.kind = kind;
    let mut s = Scene::new_popup(sp, BarGeometry { width_px: w, height_px: h, scale: 1.0 }, env(true));
    s.set_stylesheet(sheet(css));
    s.set_popup_tree(decl.tree.clone(), decl.path.clone());
    s
}

pub fn key_down(k: rsb_core::key::Key) -> rsb_core::key::KeyEvent {
    rsb_core::key::KeyEvent { key: k, mods: rsb_core::key::Modifiers::default(), down: true, repeat: false }
}

pub fn key_mod(k: rsb_core::key::Key, ctrl: bool, shift: bool) -> rsb_core::key::KeyEvent {
    rsb_core::key::KeyEvent {
        key: k,
        mods: rsb_core::key::Modifiers { ctrl, shift, alt: false, meta: false },
        down: true,
        repeat: false,
    }
}

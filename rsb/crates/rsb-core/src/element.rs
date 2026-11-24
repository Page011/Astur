//! The element tree a widget produces. Scripts build it; the scene styles,
//! lays out and draws it. A tree is a plain value: the script thread sends a
//! fresh tree for a widget when it changes, and the scene matches nodes by
//! key so hover and running transitions survive the rebuild.

use std::sync::Arc;

use crate::config::WidgetRef;
use crate::diag::SourcePos;
use crate::io::IoGrant;
use crate::style::FlexDirection;

/// Deepest element nesting in one widget tree (the widget's root is depth
/// 1). The script runtime stops an instance whose tree is deeper, and the
/// scene refuses such a tree too, so layout and key hashing never recurse
/// further than this.
pub const MAX_DEPTH: usize = 64;

/// One running widget (a widget ref on one bar window). Chosen by the
/// binary, stable across config reloads while the ref, its options, the bar
/// and the monitor stay the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceId(pub u32);

/// A script-side handler. Ids only grow within an instance and are never
/// reused, so a click on a frame drawn from an older tree can never reach a
/// different handler; the runtime ignores ids it no longer knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HandlerId(pub u32);

/// Stable identity of a node across rebuilds: hash of the instance, the
/// chain of keys from the widget root (explicit `key`, else kind plus index
/// among same-kind siblings). Engine-made nodes (bar, zones, widget
/// wrappers, plates) use fixed keys. When two siblings share an explicit
/// key, the scene warns once and uses the kind-plus-index segment for every
/// sibling after the first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeKey(pub u64);

/// Element kinds. The CSS type selector is the lower-case name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ElementKind {
    /// Flex container (`rsb.box`, `rsb.row`, `rsb.column`). Selector `box`.
    Box,
    /// One line of text. Selector `text`.
    Text,
    /// A glyph from the icon font, by name or raw. Selector `icon`.
    Icon,
    /// A PNG. Selector `image`.
    Image,
    /// Error plate (engine-made). Selector `plate`.
    Plate,
    /// The bar root (engine-made). Selector `bar`.
    Bar,
    /// `#left`, `#center`, `#right` (engine-made). Selector `zone`.
    Zone,
    /// Wrapper around each widget's root, class = widget class, id = widget
    /// id (engine-made). Selector `widget`.
    Widget,
}

impl ElementKind {
    pub fn css_name(self) -> &'static str {
        match self {
            ElementKind::Box => "box",
            ElementKind::Text => "text",
            ElementKind::Icon => "icon",
            ElementKind::Image => "image",
            ElementKind::Plate => "plate",
            ElementKind::Bar => "bar",
            ElementKind::Zone => "zone",
            ElementKind::Widget => "widget",
        }
    }
}

/// Element kinds and props reserved for later versions. A script using one
/// gets an error naming it, never a silent no-op.
pub const RESERVED_KINDS: &[&str] = &["popup", "menu", "tooltip", "slider", "input", "graph"];

#[derive(Clone, Debug, PartialEq)]
pub enum IconRef {
    /// A name from `icons::ICONS` (`volume_high`).
    Named(Arc<str>),
    /// Raw text drawn in the icon font (a Nerd Font glyph, an emoji).
    Glyph(Arc<str>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageRef {
    /// Absolute path, already permission-checked by the script runtime.
    /// Also the renderer's image cache key. The binary's image loader opens
    /// it with `rsb_services::files::open_permitted`, like `rsb.file.read`.
    pub path: Arc<str>,
    /// Logical px; None = the image's own size, which the scene learns from
    /// `Scene::set_image_size` (0 x 0 until then).
    pub width: Option<f32>,
    pub height: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Handlers {
    pub click: Option<HandlerId>,
    pub right_click: Option<HandlerId>,
    pub middle_click: Option<HandlerId>,
    pub wheel: Option<HandlerId>,
    pub enter: Option<HandlerId>,
    pub leave: Option<HandlerId>,
}

impl Handlers {
    pub fn any_click(&self) -> bool {
        self.click.is_some() || self.right_click.is_some() || self.middle_click.is_some()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub kind: ElementKind,
    pub key: Option<Arc<str>>,
    pub id: Option<Arc<str>>,
    pub classes: Vec<Arc<str>>,
    /// Inline style, `(css-name, value text)`, applied above every rule.
    pub style: Vec<(String, String)>,
    /// Initial flex direction for `Box` (`rsb.column` sets Column); CSS
    /// `flex-direction` overrides it.
    pub direction: FlexDirection,
    /// `Text`: the text. `Plate`: the plate text.
    pub text: Option<Arc<str>>,
    pub icon: Option<IconRef>,
    pub image: Option<ImageRef>,
    pub handlers: Handlers,
    pub children: Vec<Element>,
    /// Where the script created this element (`widgets/my/weather.luau:12`).
    /// Set when the element has inline `style`, so style warnings point at
    /// the widget file; None otherwise (keeps trees small).
    pub pos: Option<SourcePos>,
}

impl Element {
    pub fn new(kind: ElementKind) -> Element {
        Element {
            kind,
            key: None,
            id: None,
            classes: Vec::new(),
            style: Vec::new(),
            direction: FlexDirection::Row,
            text: None,
            icon: None,
            image: None,
            handlers: Handlers::default(),
            children: Vec::new(),
            pos: None,
        }
    }

    /// Node count of this subtree, for the per-instance node cap.
    pub fn node_count(&self) -> usize {
        1 + self.children.iter().map(Element::node_count).sum::<usize>()
    }

    /// Nesting depth of this subtree (a leaf is 1). Stops counting past
    /// `MAX_DEPTH + 1`, so a too-deep tree is cheap to reject.
    pub fn depth(&self) -> usize {
        fn walk(e: &Element, d: usize) -> usize {
            if d > MAX_DEPTH {
                return d;
            }
            e.children.iter().map(|c| walk(c, d + 1)).max().unwrap_or(d)
        }
        walk(self, 1)
    }
}

/// A widget instance's current tree, as sent by the script thread.
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetTree {
    pub instance: InstanceId,
    pub widget: WidgetRef,
    /// Increases with every tree the instance sends.
    pub version: u64,
    /// None = the widget renders nothing right now (it hides itself, e.g.
    /// media with nothing playing): its slot takes no space.
    pub root: Option<Element>,
    /// What the widget may read, for the binary's image loader (it opens
    /// `image` paths with `rsb_services::files::open_permitted`). Shared, so
    /// a new tree costs one reference count.
    pub grant: Option<Arc<IoGrant>>,
}

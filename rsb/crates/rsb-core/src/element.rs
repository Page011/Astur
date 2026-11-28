//! The element tree a widget produces. Scripts build it; the scene styles,
//! lays out and draws it. A tree is a plain value: the script thread sends a
//! fresh tree for a widget when it changes, and the scene matches nodes by
//! key so hover and running transitions survive the rebuild.
//!
//! Popups, menus and tooltips are elements too. They sit in the tree under
//! the element they belong to (their anchor) but are never laid out in the
//! bar: the bar's scene reports them (`Scene::popups`) and the binary draws
//! each open one on its own surface (see `popup`).

use std::sync::Arc;

use crate::config::WidgetRef;
use crate::diag::SourcePos;
use crate::io::IoGrant;
use crate::popup::PopupProps;
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
    /// A glyph from the icon font, by name or raw, or an SVG icon. Selector `icon`.
    Icon,
    /// A PNG or SVG picture. Selector `image`.
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
    /// A popup surface's root (`rsb.popup`): anchored to its parent
    /// element, or covering a monitor (`placement = "screen"`). Selector `popup`.
    Popup,
    /// A menu surface's root (`rsb.menu`, the `menu` prop). Selector `menu`.
    Menu,
    /// One entry of a menu (made by the runtime from the item list).
    /// Focusable. Selector `menuitem`.
    MenuItem,
    /// A tooltip surface's root (the `tooltip` prop). Selector `tooltip`.
    Tooltip,
    /// One line of editable text (`rsb.input`). Selector `input`.
    Input,
    /// A value on a track (`rsb.slider`). Selector `slider`.
    Slider,
    /// A column whose content scrolls (`rsb.scroll`; `overflow-y: auto`
    /// by default). Selector `scroll`.
    Scroll,
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
            ElementKind::Popup => "popup",
            ElementKind::Menu => "menu",
            ElementKind::MenuItem => "menuitem",
            ElementKind::Tooltip => "tooltip",
            ElementKind::Input => "input",
            ElementKind::Slider => "slider",
            ElementKind::Scroll => "scroll",
        }
    }

    /// Roots of their own surface: never laid out where they sit in the tree.
    pub fn is_surface_root(self) -> bool {
        matches!(self, ElementKind::Popup | ElementKind::Menu | ElementKind::Tooltip)
    }
}

/// Element kinds and props reserved for later versions. A script using one
/// gets an error naming it, never a silent no-op.
pub const RESERVED_KINDS: &[&str] = &["graph"];

#[derive(Clone, Debug, PartialEq)]
pub enum IconRef {
    /// A name from `icons::ICONS` (`volume_high`).
    Named(Arc<str>),
    /// Raw text drawn in the icon font (a Nerd Font glyph, an emoji).
    Glyph(Arc<str>),
    /// An SVG icon: a bundled name from `icons::SVG_ICON_NAMES` (`power`),
    /// or an absolute path to an `.svg` file, already permission-checked by
    /// the script runtime. Drawn in the element's `color` (every painted
    /// pixel takes it, as an icon font glyph would), sized by `font-size`
    /// unless `width` / `height` are set.
    Svg(Arc<str>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageRef {
    /// Absolute path, already permission-checked by the script runtime, or
    /// an in-memory image key published by a service (`rsb-image:` prefix,
    /// see `pixels::IMAGE_KEY_PREFIX`). Also the renderer's image cache
    /// key. The binary's image loader opens a path with
    /// `rsb_services::files::open_permitted`, like `rsb.file.read`; a path
    /// ending in `.svg` is drawn as an SVG in its own colours.
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
    /// `on_key`: keys the element (or a focused descendant) did not use.
    pub key: Option<HandlerId>,
    /// `on_scroll`: the element's scroll offset changed.
    pub scroll: Option<HandlerId>,
    /// `on_focus` / `on_blur`: keyboard focus arrived or left.
    pub focus: Option<HandlerId>,
    pub blur: Option<HandlerId>,
}

impl Handlers {
    pub fn any_click(&self) -> bool {
        self.click.is_some() || self.right_click.is_some() || self.middle_click.is_some()
    }
}

/// Arrow-key focus movement among the focusable descendants of an element
/// (`nav` prop): menus are vertical, the power row horizontal, a tile grid
/// `grid` (Left/Right move by one, Up/Down by one row of the laid-out grid).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum NavAxis {
    #[default]
    None,
    Vertical,
    Horizontal,
    Grid,
}

/// Interaction state the script sets. All false / None by default.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Interaction {
    /// Takes keyboard focus (Tab, arrows in a `nav` group, a click). Input,
    /// slider and menuitem elements are focusable whatever this says.
    pub focusable: bool,
    /// Focused when its surface opens (the first one in tree order wins).
    pub autofocus: bool,
    /// `:disabled` matches; no handler runs, no focus.
    pub disabled: bool,
    /// `:checked` matches when Some(true).
    pub checked: Option<bool>,
    pub nav: NavAxis,
    /// Scroll the nearest scroll container so this element is in view
    /// whenever `reveal_seq` changes (the script bumps it, e.g. when the
    /// selected row of a list moves). 0 = never asked.
    pub reveal_seq: u64,
}

/// `rsb.input`: one line of editable text. The text being edited lives in
/// the scene while the element has focus (typing never waits for a
/// script); `value` replaces it only when `value_seq` is newer than what the
/// scene last saw (the script set the value itself, e.g. cleared it after a
/// search).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InputProps {
    pub value: Arc<str>,
    /// Increases each time the script writes `value` (not when it echoes
    /// back a change the scene reported).
    pub value_seq: u64,
    pub placeholder: Option<Arc<str>>,
    /// Draw each character as U+2022; copy and cut are refused.
    pub password: bool,
    /// Characters (Unicode scalar values); 0 = the text cap (4096).
    pub max_length: u32,
    /// `on_change(text)`: after each edit, at most every 50 ms while typing,
    /// and once more when typing stops.
    pub on_change: Option<HandlerId>,
    /// `on_submit(text)`: Enter.
    pub on_submit: Option<HandlerId>,
}

/// `rsb.slider`: a value on a track. While the pointer drags it the scene
/// shows the dragged value at once and reports it (`on_change`) at most 30
/// times a second plus once on release.
#[derive(Clone, Debug, PartialEq)]
pub struct SliderProps {
    pub value: f64,
    pub min: f64,
    pub max: f64,
    /// Arrow keys and the wheel move by `step`; values snap to it. 0 = no snapping.
    pub step: f64,
    pub vertical: bool,
    /// A press on the track moves the value there at once. When false a press
    /// only starts a gesture: the value follows the pointer once it has moved
    /// a few pixels, and a press and release without that movement is a click.
    pub jump: bool,
    /// Same rule as `InputProps::value_seq`.
    pub value_seq: u64,
    pub on_change: Option<HandlerId>,
}

impl Default for SliderProps {
    fn default() -> Self {
        SliderProps {
            value: 0.0,
            min: 0.0,
            max: 100.0,
            step: 1.0,
            vertical: false,
            jump: true,
            value_seq: 0,
            on_change: None,
        }
    }
}

/// The `animate` prop: run the `@keyframes` rule `name` on this element,
/// and run it again from the start whenever `restart` changes.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimateSpec {
    pub name: Arc<str>,
    /// Inline `animation-*` values as CSS text (`"300ms"`, `"ease-out"`,
    /// `"infinite"`, `"alternate"`, `"forwards"`), validated by the scene
    /// like inline style; None = the stylesheet's value.
    pub duration: Option<Arc<str>>,
    pub easing: Option<Arc<str>>,
    pub delay: Option<Arc<str>>,
    pub iterations: Option<Arc<str>>,
    pub direction: Option<Arc<str>>,
    pub fill: Option<Arc<str>>,
    /// Bumped by the runtime when the script's `restart` value changes.
    pub restart_seq: u64,
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
    /// `Text`: the text. `Plate`: the plate text. `MenuItem`: its label.
    pub text: Option<Arc<str>>,
    pub icon: Option<IconRef>,
    pub image: Option<ImageRef>,
    pub handlers: Handlers,
    pub children: Vec<Element>,
    /// Where the script created this element (`widgets/my/weather.luau:12`).
    /// Set when the element has inline `style`, so style warnings point at
    /// the widget file; None otherwise (keeps trees small).
    pub pos: Option<SourcePos>,
    pub interaction: Interaction,
    /// `Popup`, `Menu`, `Tooltip`: how the surface opens, where it goes and
    /// what closes it.
    pub popup: Option<Box<PopupProps>>,
    pub input: Option<Box<InputProps>>,
    pub slider: Option<Box<SliderProps>>,
    pub animate: Option<Box<AnimateSpec>>,
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
            interaction: Interaction::default(),
            popup: None,
            input: None,
            slider: None,
            animate: None,
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

//! Popups, menus and tooltips as the rest of the program sees them: the
//! props a script gives a popup element, what a scene reports about the
//! popups in its trees (`PopupDecl`), why one closed, and where it goes on
//! the screen (`place`).
//!
//! Ownership: the script decides when a popup should be open (`open`,
//! `open_seq`); the UI thread decides when it closes on its own (a click
//! outside, Escape, its anchor going away) and tells the script, which
//! writes `false` back. A tree that still says open with the `open_seq`
//! the UI closed is treated as closed, so an old tree in flight never
//! reopens a popup the user just dismissed.

use std::sync::Arc;
use std::time::Duration;

use crate::element::{ElementKind, HandlerId, InstanceId, NodeKey, WidgetTree};
use crate::geom::{RectF, RectPx, SizeF};

/// Hover time before a tooltip shows.
pub const TOOLTIP_DELAY: Duration = Duration::from_millis(500);
/// A tooltip asked for within this long after another hid shows at once.
pub const TOOLTIP_RESHOW: Duration = Duration::from_millis(300);
/// Logical px between an anchor and its tooltip.
pub const TOOLTIP_OFFSET: f32 = 6.0;
/// Longest a closing popup keeps its surface for its closing transitions.
pub const CLOSE_ANIMATION_CAP: Duration = Duration::from_millis(1000);
/// Default popup group: opening a popup closes the other open popup of
/// the same group on the same monitor (never one of its own chain).
pub const DEFAULT_GROUP: &str = "bar";
/// Group of menus, so a menu opened inside a panel leaves the panel open.
pub const MENU_GROUP: &str = "menu";
/// Group of tooltips (one tooltip surface at a time).
pub const TOOLTIP_GROUP: &str = "tooltip";
/// A popup takes keyboard focus only when the click or key that opened it
/// is at most this old when its window is ready.
pub const ACTIVATE_WITHIN: Duration = Duration::from_millis(1000);
/// An overlay ignores presses and keys until this long after its first
/// visible frame, and until its opening animations have settled once, so
/// a double click or a held key cannot pick an action the user never saw.
pub const OVERLAY_INPUT_GUARD: Duration = Duration::from_millis(400);
/// The longest an overlay waits for its animations to settle before it
/// takes input anyway; Escape and a press on the dim never wait for it.
pub const OVERLAY_INPUT_GUARD_CAP: Duration = Duration::from_millis(1000);
/// Popup names that belong to the built-in widgets. A user widget that
/// uses one gets a warning and its popup stays unnamed, so `rsb popup
/// launcher` from a hotkey always reaches the built-in launcher.
pub const BUILTIN_POPUP_NAMES: &[&str] = &["dashboard", "launcher", "power", "calendar", "media", "volume", "network"];

/// Where a popup goes relative to its anchor.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Placement {
    /// Below the anchor in a top bar, above it in a bottom bar (and
    /// generally: towards the larger part of the monitor).
    #[default]
    Auto,
    Below,
    Above,
    Left,
    Right,
    /// Covers the anchor's whole monitor (the power menu overlay).
    Screen,
    /// At a point inside the anchor, logical px from its border box's top
    /// left (context menus open where the pointer was).
    Pointer {
        x: f32,
        y: f32,
    },
}

/// Alignment across the placement side (`Below`: Start = left edges line up).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PopupAlign {
    Start,
    #[default]
    Center,
    End,
}

/// Whether a popup takes keyboard focus (activation) when it opens.
/// Focus is only ever taken for a popup opened by the user: its `open`
/// turned true while a click or key handler (or `rsb popup`) ran
/// (`PopupProps::gesture` is not 0) and that input is at most
/// `ACTIVATE_WITHIN` old. A popup opened from a timer, an effect, setup or
/// a service update never takes focus, whatever this says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FocusMode {
    /// Takes focus when it holds something focusable (an input, a slider,
    /// a menu item, a `focusable` element).
    #[default]
    Auto,
    /// Never takes focus; dismissed by clicks outside only (and its anchor).
    None,
    /// Always asks for focus.
    Always,
}

/// What may close a popup besides its script and its anchor going away.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DismissRules {
    /// A button press outside every surface of this popup's chain.
    pub outside: bool,
    /// Escape while it has focus.
    pub escape: bool,
}

impl Default for DismissRules {
    fn default() -> Self {
        DismissRules { outside: true, escape: true }
    }
}

/// Props of a `Popup`, `Menu` or `Tooltip` element.
#[derive(Clone, Debug, PartialEq)]
pub struct PopupProps {
    /// For `rsb popup <name>` and `rsb.popups.open(name)`; the first is the
    /// main name, the rest answer too (the dashboard is also `launcher`).
    /// Empty = unnamed. Unique per monitor: a built-in's name always wins
    /// over a user widget's, then the first in tree order (a warning names
    /// the rest).
    pub names: Vec<Arc<str>>,
    pub group: Arc<str>,
    pub open: bool,
    /// Increases each time the script turns `open` from false to true.
    pub open_seq: u64,
    /// The input that turned `open` true for this `open_seq`: the gesture
    /// id the UI gave the click, key or `rsb popup` command whose handler
    /// was running; 0 = a timer, an effect, setup or a service update.
    /// Focus and overlays need a gesture; a popup an outside press just
    /// closed is not reopened by the same press's click.
    pub gesture: u64,
    pub placement: Placement,
    pub align: PopupAlign,
    /// Logical px between the anchor and the popup along the placement
    /// side (may be negative to overlap); 0 = edge to edge.
    pub offset: f32,
    /// Below or above: the popup takes the anchor's flat facing edge as
    /// its own width and position (`PopupDecl::anchor_flat`), in whole
    /// device px, so a panel that grows out of its anchor meets it with
    /// no partial pixel column at any scale. Its CSS width is replaced.
    pub attach: bool,
    pub focus: FocusMode,
    pub dismiss: DismissRules,
    /// Set by the runtime: told when the UI closed the popup (it writes
    /// false into the `open` state, then calls the script's `on_close`).
    pub on_close: Option<HandlerId>,
    /// Set by the runtime: told the name used when `rsb popup <name>` or
    /// `rsb.popups.open` / `toggle` opened the popup.
    pub on_open: Option<HandlerId>,
    /// Set by the runtime: the widget is a built-in or holds `overlay`.
    /// Any other widget's popup stays small, can always be dismissed, and
    /// is not put back at once after the user closed it.
    pub trusted: bool,
}

impl Default for PopupProps {
    fn default() -> Self {
        PopupProps {
            names: Vec::new(),
            group: Arc::from(DEFAULT_GROUP),
            open: false,
            open_seq: 0,
            gesture: 0,
            placement: Placement::Auto,
            align: PopupAlign::Center,
            offset: 0.0,
            attach: false,
            focus: FocusMode::Auto,
            dismiss: DismissRules::default(),
            on_close: None,
            on_open: None,
            trusted: false,
        }
    }
}

impl PopupProps {
    /// The main name, if any.
    pub fn name(&self) -> Option<&str> {
        self.names.first().map(|n| &**n)
    }

    pub fn answers_to(&self, name: &str) -> bool {
        self.names.iter().any(|n| &**n == name)
    }
}

/// Why the UI closed a popup (passed to `on_close` as a string).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CloseReason {
    /// A press outside the popup and its chain (including a click on the
    /// bar that is not on the anchor).
    Outside,
    Escape,
    /// The popup lost keyboard focus to another window.
    Deactivated,
    /// Its anchor element is gone from the tree, or its widget stopped.
    AnchorGone,
    /// Its bar was hidden (`rsb toggle`, a fullscreen app with `hide`).
    BarHidden,
    /// Monitors or DPI changed.
    DisplayChange,
    /// Another popup of the same group opened.
    Replaced,
    /// A menu item was picked.
    Picked,
    /// `rsb popup <name> close`, `rsb.popups.close`.
    Command,
    /// It may not open this way: an overlay (`placement = "screen"`)
    /// whose `open` turned true outside a click or key handler. Logged once
    /// with the reason.
    Refused,
}

impl CloseReason {
    pub fn name(self) -> &'static str {
        match self {
            CloseReason::Outside => "outside",
            CloseReason::Escape => "escape",
            CloseReason::Deactivated => "deactivated",
            CloseReason::AnchorGone => "anchor_gone",
            CloseReason::BarHidden => "bar_hidden",
            CloseReason::DisplayChange => "display_change",
            CloseReason::Replaced => "replaced",
            CloseReason::Picked => "picked",
            CloseReason::Command => "command",
            CloseReason::Refused => "refused",
        }
    }
}

/// `rsb popup <name> [open|close|toggle]`, `rsb.popups.*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PopupAction {
    Open,
    Close,
    Toggle,
}

impl PopupAction {
    pub fn name(self) -> &'static str {
        match self {
            PopupAction::Open => "open",
            PopupAction::Close => "close",
            PopupAction::Toggle => "toggle",
        }
    }

    pub fn from_name(s: &str) -> Option<PopupAction> {
        match s {
            "open" => Some(PopupAction::Open),
            "close" => Some(PopupAction::Close),
            "toggle" => Some(PopupAction::Toggle),
            _ => None,
        }
    }
}

/// A popup element a scene found in its trees, after `Scene::frame`.
#[derive(Clone, Debug, PartialEq)]
pub struct PopupDecl {
    /// The popup element's node key (stable while its path and keys are).
    pub key: NodeKey,
    pub instance: InstanceId,
    /// `Popup`, `Menu` or `Tooltip`.
    pub kind: ElementKind,
    pub props: PopupProps,
    /// The element the popup belongs to (its parent in the tree).
    pub anchor: NodeKey,
    /// The anchor's border box once its running transitions end, in the
    /// reporting surface's physical px: popups are placed against where
    /// the anchor is going, so a widening anchor and its panel line up.
    /// The manager compares it every frame and moves an open popup whose
    /// placed rect changes by a pixel or more.
    pub anchor_rect: RectF,
    /// The flat parts of the anchor's outline (its `clip-path` polygon or
    /// border box, at the target layout, vertices on whole device px) at
    /// its top and bottom edges, for `PopupProps::attach`.
    pub anchor_flat: FlatEdges,
    /// The tree that holds the popup, and the child indices from the
    /// widget root down to the popup element.
    pub tree: Arc<WidgetTree>,
    pub path: Arc<[u32]>,
    /// The tree says open and the UI has not closed this `open_seq`.
    pub open: bool,
}

/// Horizontal spans `(x0, x1)` of an outline's longest straight run along
/// its top and bottom edges, surface physical px, whole pixels. None when
/// the edge is a point (a chevron tip) or the anchor has no box.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FlatEdges {
    pub top: Option<(f32, f32)>,
    pub bottom: Option<(f32, f32)>,
}

/// Which side a popup ended up on (engine class on the popup root:
/// `below`, `above`, `left`, `right`, `screen`, `pointer`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Below,
    Above,
    Left,
    Right,
    Screen,
    Pointer,
}

impl Side {
    pub fn class(self) -> &'static str {
        match self {
            Side::Below => "below",
            Side::Above => "above",
            Side::Left => "left",
            Side::Right => "right",
            Side::Screen => "screen",
            Side::Pointer => "pointer",
        }
    }
}

/// Layout px a popup that is moved to fit keeps from the edge of the work area.
const EDGE_MARGIN: f32 = 8.0;

/// Whether `auto` goes below the anchor: its middle is in the top half of the
/// monitor (the middle itself counts as top). `place` and the side the
/// manager asks for before the surface size is known both use it.
pub fn auto_below(anchor: &RectF, monitor: &RectPx) -> bool {
    anchor.y + anchor.h / 2.0 <= (monitor.y0 + monitor.y1) as f32 / 2.0
}

/// Everything `place` needs, in physical px, virtual-screen coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaceInput {
    pub anchor: RectF,
    /// The popup surface's size.
    pub size: SizeF,
    /// The monitor's work area (popups stay inside it).
    pub work: RectPx,
    /// The whole monitor (`Screen` covers it).
    pub monitor: RectPx,
    pub placement: Placement,
    pub align: PopupAlign,
    /// `PopupProps::offset` times the scale.
    pub offset: f32,
    /// DPI / 96, for `Pointer` coordinates.
    pub scale: f32,
    /// `PopupProps::attach`: the anchor's flat edge on the side the popup
    /// goes (`FlatEdges::bottom` below, `top` above), virtual-screen px.
    /// Below or above, the popup takes exactly this span across.
    pub attach: Option<FlatEdges>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub rect: RectPx,
    pub side: Side,
}

/// Where a popup goes: on its side of the anchor, flipped to the other
/// side when it does not fit there and the other side has more room,
/// aligned across, then moved (never shrunk) to stay inside the work area.
/// Whole pixels, so its edge meets the anchor's edge with no gap or overlap.
pub fn place(input: &PlaceInput) -> Placed {
    let w = input.size.w.round().max(0.0);
    let h = input.size.h.round().max(0.0);
    if input.placement == Placement::Screen {
        return Placed { rect: input.monitor, side: Side::Screen };
    }
    let a = input.anchor;
    let work = input.work;
    let (wx0, wy0, wx1, wy1) = (work.x0 as f32, work.y0 as f32, work.x1 as f32, work.y1 as f32);
    let along = |start: f32, len: f32, size: f32| match input.align {
        PopupAlign::Start => start,
        PopupAlign::Center => start + (len - size) / 2.0,
        PopupAlign::End => start + len - size,
    };
    let clamp = |v: f32, lo: f32, hi: f32, size: f32| v.min(hi - size).max(lo);
    // A popup that has to be moved to fit stops a little short of the screen
    // edge instead of touching it (one that is glued to an anchor's flat edge
    // is not moved this way).
    let gap = EDGE_MARGIN * input.scale.max(1.0);
    let clamp_off_edge = |v: f32, lo: f32, hi: f32, size: f32| clamp(v, lo + gap, hi - gap, size);
    let (x, y, side) = match input.placement {
        Placement::Pointer { x, y } => {
            let px = a.x + x * input.scale;
            let py = a.y + y * input.scale;
            // Like a native context menu: right and down from the point,
            // flipped left or up when that runs off the work area.
            let x = if px + w > wx1 && px - w >= wx0 { px - w } else { px };
            let y = if py + h > wy1 && py - h >= wy0 { py - h } else { py };
            (clamp(x, wx0, wx1, w), clamp(y, wy0, wy1, h), Side::Pointer)
        }
        Placement::Left | Placement::Right => {
            let room_left = a.x - wx0;
            let room_right = wx1 - a.right();
            let mut right = input.placement == Placement::Right;
            let need = w + input.offset.max(0.0);
            if right && room_right < need && room_left > room_right {
                right = false;
            } else if !right && room_left < need && room_right > room_left {
                right = true;
            }
            let x = if right { a.right() + input.offset } else { a.x - input.offset - w };
            let y = along(a.y, a.h, h);
            // Neither side had room: the popup is moved over the anchor, not off the screen.
            (clamp(x, wx0, wx1, w), clamp_off_edge(y, wy0, wy1, h), if right { Side::Right } else { Side::Left })
        }
        _ => {
            let room_above = a.y - wy0.min(a.y);
            let room_below = wy1 - a.bottom();
            let mut below = match input.placement {
                Placement::Above => false,
                Placement::Below => true,
                _ => auto_below(&a, &input.monitor),
            };
            let need = h + input.offset.max(0.0);
            if below && room_below < need && room_above > room_below {
                below = false;
            } else if !below && room_above < need && room_below > room_above {
                below = true;
            }
            let y = if below { a.bottom() + input.offset } else { a.y - input.offset - h };
            let edge = input.attach.and_then(|e| if below { e.bottom } else { e.top });
            if let Some((x0, x1)) = edge {
                // Glued to the anchor's flat edge: its span, not the CSS width.
                let (x0, x1) = (x0.round(), x1.round().max(x0.round()));
                let x = clamp(x0, wx0, wx1, x1 - x0);
                let (x, y) = (x as i32, y.round() as i32);
                let side = if below { Side::Below } else { Side::Above };
                return Placed { rect: RectPx::new(x, y, x + (x1 - x0) as i32, y + h as i32), side };
            }
            let x = along(a.x, a.w, w);
            (clamp_off_edge(x, wx0, wx1, w), clamp(y, wy0, wy1, h), if below { Side::Below } else { Side::Above })
        }
    };
    let (x, y) = (x.round() as i32, y.round() as i32);
    Placed { rect: RectPx::new(x, y, x + w as i32, y + h as i32), side }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(anchor: RectF, w: f32, h: f32, placement: Placement) -> PlaceInput {
        PlaceInput {
            anchor,
            size: SizeF { w, h },
            work: RectPx::new(0, 36, 1920, 1200),
            monitor: RectPx::new(0, 0, 1920, 1200),
            placement,
            align: PopupAlign::Center,
            offset: 0.0,
            scale: 1.0,
            attach: None,
        }
    }

    #[test]
    fn a_panel_below_a_top_bar_meets_the_anchor_edge() {
        // The reference panel: 500 wide, centred under the stretched anchor.
        let p = place(&input(RectF::new(697.0, 10.0, 526.0, 26.0), 500.0, 500.0, Placement::Auto));
        assert_eq!(p.side, Side::Below);
        assert_eq!(p.rect, RectPx::new(710, 36, 1210, 536));
    }

    #[test]
    fn it_flips_when_the_other_side_has_more_room_and_clamps_across() {
        let a = RectF::new(1900.0, 1100.0, 20.0, 20.0);
        let p = place(&input(a, 300.0, 400.0, Placement::Below));
        assert_eq!(p.side, Side::Above);
        // Moved to fit: 8 px short of the screen edge, not touching it.
        assert_eq!(p.rect, RectPx::new(1612, 700, 1912, 1100));
    }

    #[test]
    fn a_popup_that_is_moved_to_fit_keeps_8_layout_px_from_the_edge_and_a_glued_one_does_not() {
        let mut i = input(RectF::new(1850.0, 10.0, 60.0, 26.0), 380.0, 300.0, Placement::Below);
        assert_eq!(place(&i).rect.x1, 1912, "right edge");
        i.anchor = RectF::new(10.0, 10.0, 60.0, 26.0);
        assert_eq!(place(&i).rect.x0, 8, "left edge");
        // At 150 % the margin is 12 px.
        i.scale = 1.5;
        assert_eq!(place(&i).rect.x0, 12);
        // A panel glued to the flat edge of its anchor takes exactly that span.
        i.scale = 1.0;
        i.attach = Some(FlatEdges { top: None, bottom: Some((0.0, 380.0)) });
        assert_eq!((place(&i).rect.x0, place(&i).rect.x1), (0, 380));
    }

    #[test]
    fn auto_goes_up_from_a_bottom_bar() {
        let p = place(&input(RectF::new(100.0, 1170.0, 40.0, 30.0), 200.0, 100.0, Placement::Auto));
        assert_eq!(p.side, Side::Above);
        assert_eq!(p.rect.y1, 1170);
    }

    #[test]
    fn screen_covers_the_monitor_and_pointer_flips_at_the_edge() {
        let p = place(&input(RectF::new(0.0, 0.0, 10.0, 10.0), 1.0, 1.0, Placement::Screen));
        assert_eq!(p.rect, RectPx::new(0, 0, 1920, 1200));
        let m =
            place(&input(RectF::new(1800.0, 10.0, 100.0, 26.0), 200.0, 100.0, Placement::Pointer { x: 50.0, y: 20.0 }));
        assert_eq!(m.side, Side::Pointer);
        assert_eq!(m.rect, RectPx::new(1650, 36, 1850, 136));
    }

    #[test]
    fn an_attached_panel_takes_the_anchors_flat_edge_at_a_fractional_scale() {
        // 150 %: the open anchor is 789 px wide from x 885 with 19.5 px
        // chevrons; its snapped flat bottom edge is 905..1655.
        let mut i = input(RectF::new(885.0, 15.0, 789.0, 39.0), 750.0, 750.0, Placement::Below);
        i.scale = 1.5;
        i.work = RectPx::new(0, 54, 2560, 1600);
        i.monitor = RectPx::new(0, 0, 2560, 1600);
        i.attach = Some(FlatEdges { top: None, bottom: Some((905.0, 1655.0)) });
        let p = place(&i);
        assert_eq!(p.side, Side::Below);
        assert_eq!(p.rect, RectPx::new(905, 54, 1655, 804));
        // No flat edge on that side: placed from the CSS size as usual.
        i.attach = Some(FlatEdges { top: Some((905.0, 1655.0)), bottom: None });
        assert_eq!(place(&i).rect.width(), 750);
    }
}

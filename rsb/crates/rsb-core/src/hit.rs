//! Pointer input in, handler calls out. Coordinates are physical pixels
//! relative to the bar window's top-left corner.

use crate::element::{HandlerId, InstanceId, NodeKey};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Left,
    Right,
    Middle,
}

impl Button {
    pub fn name(self) -> &'static str {
        match self {
            Button::Left => "left",
            Button::Right => "right",
            Button::Middle => "middle",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerKind {
    Move,
    /// The pointer left the bar window.
    Leave,
    Down(Button),
    Up(Button),
    /// Wheel notches, positive = away from the user (up). A precision
    /// touchpad gives fractions.
    Wheel(f32),
    /// Sideways wheel notches, positive = right. A tilt wheel gives them, and
    /// the window layer turns Shift with the vertical wheel into them.
    HWheel(f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerEvent {
    pub kind: PointerKind,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputKind {
    /// Down and up on the same node.
    Click(Button),
    Wheel(f32),
    Enter,
    Leave,
}

/// What the scene asks the script thread to run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HandlerCall {
    pub instance: InstanceId,
    pub handler: HandlerId,
    pub kind: InputKind,
    /// Logical px relative to the node's box.
    pub x: f32,
    pub y: f32,
    /// The user input this call answers: one id per button press and its
    /// release, or per key down, given by the UI thread (the scene leaves
    /// 0 and the binary fills it in). 0 = not a user input (hover, wheel).
    /// A click or key handler runs "inside a gesture" (focus, overlays and
    /// state-changing commands need one).
    pub gesture: u64,
}

/// A plate that was clicked (plates open the log; no script runs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlateTarget {
    Bar,
    Widget(InstanceId),
}

/// What one pointer event led to, from `Scene::pointer`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PointerResult {
    /// Handler calls for the script thread, in order.
    pub calls: Vec<HandlerCall>,
    /// A click (down and up) on a plate.
    pub plate_click: Option<PlateTarget>,
    /// A button released with no handler for it under the pointer (and no
    /// plate). The binary opens the native menu for `Some(Right)`.
    pub unhandled: Option<Button>,
    /// A button went down where this surface has no node: a popup's
    /// transparent margin or shadow. The popup manager treats it as a press
    /// outside the popup.
    pub missed: bool,
    /// A menu item was picked (released on an item that opens no submenu).
    /// The whole menu chain closes, whether or not the menu has an
    /// `on_close` to tell.
    pub picked: bool,
}

/// Result of a hit test.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// Deepest node under the point that has a handler for the event, if any.
    pub target: Option<NodeKey>,
    /// Every node under the point, root first: these match `:hover`.
    pub hover_chain: Vec<NodeKey>,
}

/// Events other than pointer clicks, wheel and hover that reach a script
/// handler: keys, text and value changes, focus, scrolling, popups closing.
#[derive(Clone, Debug, PartialEq)]
pub enum ElementEvent {
    /// `on_key` (key downs the scene did not use itself).
    Key(crate::key::KeyEvent),
    /// `InputProps::on_change` with the whole text.
    TextChanged(std::sync::Arc<str>),
    /// `InputProps::on_submit` (Enter) with the whole text.
    Submitted(std::sync::Arc<str>),
    /// `SliderProps::on_change`; `done` on the last report of a drag.
    ValueChanged {
        value: f64,
        done: bool,
    },
    Focus,
    Blur,
    /// `on_scroll`: the new offset, logical px from the top left.
    Scrolled {
        x: f32,
        y: f32,
    },
    /// `PopupProps::on_close`: the UI closed the popup.
    PopupClosed(crate::popup::CloseReason),
    /// `PopupProps::on_open`: `rsb popup <name>` or `rsb.popups` opened the
    /// popup by this name.
    PopupOpened(std::sync::Arc<str>),
}

/// An `ElementEvent` for one handler (sent as `ScriptIn::Element`).
#[derive(Clone, Debug, PartialEq)]
pub struct ElementCall {
    pub instance: InstanceId,
    pub handler: HandlerId,
    pub event: ElementEvent,
    /// As `HandlerCall::gesture`: set for key downs (the first resume of an
    /// `on_key` task runs inside the gesture), 0 for everything else.
    pub gesture: u64,
}

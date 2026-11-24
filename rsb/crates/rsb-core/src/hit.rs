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
}

/// Result of a hit test.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// Deepest node under the point that has a handler for the event, if any.
    pub target: Option<NodeKey>,
    /// Every node under the point, root first: these match `:hover`.
    pub hover_chain: Vec<NodeKey>,
}

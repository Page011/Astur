//! What the UI thread's inbox carries. Every other thread sends one of
//! these and wakes the UI; the UI drains them all on each wake.

use std::sync::Arc;
use std::sync::mpsc::Sender;

use rsb_core::element::InstanceId;
use rsb_core::hit::PointerEvent;
use rsb_core::ipc::{Command, Reply};
use rsb_core::msg::ScriptOut;
use rsb_render::Image;

use crate::args::ClockKind;
use crate::cfgload::Built;

/// Which files a reload read, for the `reload` stats line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReloadKind {
    /// The first reading at start.
    Initial,
    Toml,
    Css,
    /// `rsb reload` and the menu: everything.
    All,
}

impl ReloadKind {
    pub fn name(self) -> &'static str {
        match self {
            ReloadKind::Initial => "initial",
            ReloadKind::Toml => "toml",
            ReloadKind::Css => "css",
            ReloadKind::All => "all",
        }
    }
}

/// A parsed config, produced off the UI thread.
pub struct ConfigUpdate {
    pub kind: ReloadKind,
    pub built: Built,
    /// When the file watcher saw the change (wall clock, ms since the epoch).
    pub notified_wall_ms: u64,
}

/// What `rsb status` asks the UI about its bars.
#[derive(Clone, Debug, PartialEq)]
pub struct BarInfo {
    pub name: String,
    pub monitor: u32,
    pub visible: bool,
    pub reserved: bool,
    pub render_failed: bool,
    pub plate: Option<String>,
}

/// An event a hidden test flag injects at a set time.
#[derive(Clone, Debug, PartialEq)]
pub enum SimAction {
    Quit,
    TaskbarCreated,
    Dpi(u32),
    DisplayChange,
    Theme(bool),
    Fullscreen(bool),
    Clock(ClockKind),
    FullscreenWindow { for_ms: u64 },
    Pointer { bar: String, event: PointerEvent },
    RenderFailure,
    Panic,
}

pub enum UiMsg {
    Script(ScriptOut),
    Config(ConfigUpdate),
    /// reload, quit, toggle, trigger from the pipe (status is gathered by the pipe thread).
    Pipe {
        command: Command,
        reply: Sender<Reply>,
    },
    BarsQuery {
        reply: Sender<Vec<BarInfo>>,
    },
    Image {
        key: Arc<str>,
        result: Result<Image, String>,
    },
    /// A script call has run for over 2 s (instance 0 = the runtime itself).
    Stalled {
        instance: InstanceId,
        for_ms: u64,
    },
    StalledCleared,
    /// A widget file changed (its source goes straight to the script thread;
    /// this only lets the UI time the reload).
    WidgetFile {
        notified_wall_ms: u64,
    },
    Sim(SimAction),
    Quit,
}

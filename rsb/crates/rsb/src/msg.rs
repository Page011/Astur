//! What the UI thread's inbox carries. Every other thread sends one of
//! these and wakes the UI; the UI drains them all on each wake.

use std::sync::Arc;
use std::sync::mpsc::Sender;

use rsb_core::element::InstanceId;
use rsb_core::hit::PointerEvent;
use rsb_core::ipc::{Command, Reply};
use rsb_core::key::{KeyEvent, TextInput};
use rsb_core::msg::ScriptOut;
use rsb_core::pixels::RgbaImage;
use rsb_core::popup::PopupAction;
use rsb_render::{Image, ParsedSvg};

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

/// The window a simulated pointer event goes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SimSurface {
    /// A bar by name.
    Bar(String),
    /// A popup, menu, tooltip or overlay's content by its name.
    Popup(String),
    /// An overlay's dim window, by the overlay's name.
    Dim(String),
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
    FullscreenWindow {
        for_ms: u64,
    },
    Pointer {
        surface: SimSurface,
        event: PointerEvent,
    },
    /// A key to a popup window (as if it had keyboard focus).
    Key {
        popup: String,
        event: KeyEvent,
    },
    /// Typed text or IME composition for a popup window.
    Text {
        popup: String,
        input: TextInput,
    },
    /// As `rsb popup <name> <action>`, with a gesture of its own.
    Popup {
        name: String,
        action: PopupAction,
    },
    /// A button went down outside every window of ours.
    OutsidePress,
    /// Keyboard focus went from the popup to another app.
    Deactivate {
        popup: String,
    },
    /// The session was locked or unlocked.
    Session {
        locked: bool,
    },
    /// The popup's window got a DPI change.
    PopupDpi {
        popup: String,
        dpi: u32,
    },
    /// The UI thread stops for this long (the watchdog's overlay check).
    UiStall {
        for_ms: u64,
    },
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
    /// A user's `.svg` file, already parsed by the image loader.
    Svg {
        key: Arc<str>,
        result: Result<ParsedSvg, String>,
    },
    /// A picture a service made (album art, an app icon). Straight from the
    /// hub: the script thread never sees it.
    ServiceImage {
        key: Arc<str>,
        image: RgbaImage,
    },
    /// The power service reported an error (a lock, sleep or other action
    /// failed): an overlay that was waiting for it closes.
    PowerError,
    /// The apps service stopped: nothing needs its icons any more.
    DropAppImages,
    /// The service value no longer names this key (the track changed).
    DropServiceImage {
        key: Arc<str>,
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

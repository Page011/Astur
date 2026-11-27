//! What the UI thread's inbox carries. Every other thread sends one of
//! these and wakes the UI; the UI drains them all on each wake.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use rsb_core::contract::{AppliedReply, FileHash};
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

/// The versions of the files a config was built from: the hash of the raw
/// bytes that were parsed (a byte order mark included). `None` where that
/// file's part was not used (the file was unusable and the last good one
/// stays); `Some(FileHash::Missing)` where the file does not exist.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AppliedHashes {
    pub bar_toml: Option<FileHash>,
    pub style_css: Option<FileHash>,
}

/// A parsed config, produced off the UI thread.
pub struct ConfigUpdate {
    pub kind: ReloadKind,
    pub built: Built,
    /// The versions of the files `built` was parsed from (a copy of
    /// `built.hashes`, kept with the update the UI thread applies).
    pub hashes: AppliedHashes,
    /// When the file watcher saw the change (wall clock, ms since the epoch).
    pub notified_wall_ms: u64,
}

/// What the bar shows, readable from any thread: the versions of the files
/// of the config now in effect, how many diagnostics it came with and the
/// folders it reads. The UI thread sets it when it swaps a config in; the
/// pipe thread answers `applied` from it with no round trip.
#[derive(Debug)]
pub struct AppliedState {
    inner: Mutex<AppliedSnapshot>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppliedSnapshot {
    pub hashes: AppliedHashes,
    pub diagnostics: u32,
    pub loads: u64,
    /// Canonical paths.
    pub config_dir: String,
    pub log_dir: String,
}

impl AppliedState {
    pub fn new(config_dir: String, log_dir: String) -> AppliedState {
        AppliedState { inner: Mutex::new(AppliedSnapshot { config_dir, log_dir, ..Default::default() }) }
    }

    /// A config was swapped in. A file whose part was not applied (`None`)
    /// keeps the version it had.
    pub fn set(&self, hashes: AppliedHashes, diagnostics: u32) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.hashes.bar_toml = hashes.bar_toml.or(inner.hashes.bar_toml);
        inner.hashes.style_css = hashes.style_css.or(inner.hashes.style_css);
        inner.diagnostics = diagnostics;
        inner.loads += 1;
    }

    pub fn snapshot(&self) -> AppliedSnapshot {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The answer to the pipe line `applied`.
    pub fn reply(&self) -> AppliedReply {
        let s = self.snapshot();
        AppliedReply {
            bar_toml: s.hashes.bar_toml,
            style_css: s.hashes.style_css,
            diagnostics: s.diagnostics,
            loads: s.loads,
            config_dir: s.config_dir,
            log_dir: s.log_dir,
        }
    }
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
    /// `rsb inspect` asks for the bars' frame cost and the trees of the
    /// widgets it names. Answered from the UI thread's own state at once.
    Inspect {
        query: crate::inspect::InspectQuery,
        reply: Sender<crate::inspect::UiPart>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applied_state_keeps_the_version_of_a_file_that_was_not_applied() {
        let state = AppliedState::new("cfg-dir".into(), "log-dir".into());
        assert_eq!(state.reply().bar_toml, None, "nothing applied yet");
        let (a, b) = (FileHash::Fnv(1), FileHash::Fnv(2));
        state.set(AppliedHashes { bar_toml: Some(a), style_css: Some(FileHash::Missing) }, 0);
        // The next build could not use style.css: it keeps its old version.
        state.set(AppliedHashes { bar_toml: Some(b), style_css: None }, 3);
        let reply = state.reply();
        assert_eq!((reply.bar_toml, reply.style_css, reply.diagnostics), (Some(b), Some(FileHash::Missing), 3));
        assert_eq!((reply.config_dir.as_str(), reply.log_dir.as_str()), ("cfg-dir", "log-dir"));
    }

    #[test]
    fn every_config_swapped_in_counts_even_when_it_shows_the_same_versions() {
        let state = AppliedState::new("c".into(), "l".into());
        assert_eq!(state.reply().loads, 0);
        let same = AppliedHashes { bar_toml: Some(FileHash::Fnv(1)), style_css: None };
        state.set(same, 0);
        state.set(same, 0);
        assert_eq!(state.reply().loads, 2);
    }
}

//! What `settings::serve` needs from the outside, as plain data.
//!
//! The functions behind it live in other modules (the edit engine, the
//! writer, the contrast checks, the preview renderer, the script sandbox,
//! the pipe to the bar). Keeping them behind one trait makes the reply
//! logic testable with a fake that returns what those modules would, and
//! keeps the real wiring in one file (`real.rs`).

use std::path::Path;

use rsb_core::cfgedit::{FileTexts, Plan};
use rsb_core::config::ConfigSources;
use rsb_core::contract::{AppliedReply, ConfigFile, FileHash, Op, PresetContrast, SettingsError, Source};
use rsb_core::preset::Scheme;
use serde_json::Value;

use crate::cfgwrite::{Applied, LogEntry, WriteOpts, Written};
use crate::preview::{Preview, PreviewOpts};
use crate::settings::Ctx;

/// One of the two files as read from the config folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileRead {
    /// The version of the raw bytes; `Missing` for no file.
    pub hash: Option<FileHash>,
    /// The text without a byte order mark. For bytes that are not UTF-8 it
    /// is the lossy text, and `utf8` is false.
    pub text: Option<String>,
    pub utf8: bool,
    /// Why the file could not be read at all (too large, locked).
    pub unreadable: Option<String>,
}

impl FileRead {
    pub fn missing() -> FileRead {
        FileRead { hash: Some(FileHash::Missing), text: None, utf8: true, unreadable: None }
    }

    pub fn of_text(text: &str) -> FileRead {
        FileRead {
            hash: Some(FileHash::of(text.as_bytes())),
            text: Some(text.to_string()),
            utf8: true,
            unreadable: None,
        }
    }

    pub fn hash(&self) -> FileHash {
        self.hash.unwrap_or(FileHash::Missing)
    }
}

/// A file that does not parse: where, and what is wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileError {
    pub at: String,
    pub message: String,
}

/// The accent in one scheme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemeAccent {
    /// `#rrggbb`; None when no sheet defines it.
    pub value: Option<String>,
    pub source: Source,
    /// `file:line:col` when the user's file set it.
    pub at: Option<String>,
    /// A later user rule that wins over the colours block.
    pub shadowed_by: Option<String>,
    /// User declarations of an accent token that the colours block beats.
    pub overridden: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccentView {
    pub dark: SchemeAccent,
    pub light: SchemeAccent,
}

/// Everything read of the config folder for one request.
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    pub bar_toml: FileRead,
    pub style_css: FileRead,
    /// The effective config and where each value came from; None when
    /// `bar.toml` does not parse.
    pub sources: Option<ConfigSources>,
    pub bar_error: Option<FileError>,
    pub style_error: Option<FileError>,
    /// None when the style chain does not parse.
    pub accent: Option<AccentView>,
}

impl Loaded {
    pub fn file(&self, file: ConfigFile) -> &FileRead {
        match file {
            ConfigFile::BarToml => &self.bar_toml,
            ConfigFile::StyleCss => &self.style_css,
        }
    }

    pub fn error(&self, file: ConfigFile) -> Option<&FileError> {
        match file {
            ConfigFile::BarToml => self.bar_error.as_ref(),
            ConfigFile::StyleCss => self.style_error.as_ref(),
        }
    }

    /// The texts the edit engine plans against.
    pub fn texts(&self) -> FileTexts {
        FileTexts { bar_toml: self.bar_toml.text.clone(), style_css: self.style_css.text.clone() }
    }
}

/// One colour a page may offer a picker for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TuneView {
    pub var: String,
    pub label: String,
    pub editable: bool,
    pub dark: Option<String>,
    pub light: Option<String>,
}

/// One preset as `settings presets` shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct PresetView {
    pub name: String,
    pub title: String,
    pub description: String,
    pub schemes: Vec<Scheme>,
    /// How many `[[bar]]` tables the preset has.
    pub bars: u32,
    pub position: String,
    pub height: u32,
    pub floating: bool,
    pub tune: Vec<TuneView>,
    pub contrast: PresetContrast,
}

/// One widget as `settings widgets` lists it, before `used` is added.
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetView {
    pub name: String,
    pub builtin: bool,
    pub replaces_builtin: bool,
    /// What `describe::widget` returned.
    pub about: Value,
}

/// A widget file that could not be described.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WidgetFailure {
    pub name: String,
    /// The file, relative to the config folder.
    pub source: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listing {
    pub widgets: Vec<WidgetView>,
    pub failed: Vec<WidgetFailure>,
}

/// What the running bar answered to `applied`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BarState {
    /// Nobody owns the pipe.
    NotRunning,
    /// The pipe exists but refused, or did not answer in time.
    Unreachable,
    Running(AppliedReply),
}

pub trait Backend {
    /// Reads both files and the effective config. The accent costs a
    /// walk of the style cascade, so only `with_accent` asks for it.
    fn load(&self, dir: &Path, with_accent: bool) -> Loaded;

    /// The edit engine. `popups` says whether to find out which menus a
    /// widget opens, which runs the widget's module: not work for the bar's
    /// pipe thread.
    fn plan(&self, dir: &Path, texts: &FileTexts, ops: &[Op], reset: bool, popups: bool)
    -> Result<Plan, SettingsError>;

    /// The writer.
    fn apply(&self, dir: &Path, plan: &Plan, opts: &WriteOpts) -> Result<Applied, SettingsError>;
    fn undo(&self, dir: &Path, n: Option<u32>, opts: &WriteOpts) -> Result<Applied, SettingsError>;
    fn restore(&self, dir: &Path, file: ConfigFile, opts: &WriteOpts) -> Result<Applied, SettingsError>;
    fn log_entries(&self, dir: &Path, last: usize) -> Result<Vec<LogEntry>, SettingsError>;
    fn written(&self, dir: &Path) -> Written;
    fn has_last_good(&self, dir: &Path, file: ConfigFile) -> bool;
    /// Appends `config: <message>` to the live log in `log_dir`.
    fn append_log(&self, log_dir: &Path, message: &str);

    /// Every built-in preset with its metadata and contrast.
    fn presets(&self, dir: &Path) -> Result<Vec<PresetView>, SettingsError>;
    /// Every widget, described by evaluating its module.
    fn widgets(&self, dir: &Path) -> Listing;
    /// Previews, drawn or found in the cache.
    fn previews(&self, presets: &[String], opts: &PreviewOpts) -> Result<Vec<Preview>, SettingsError>;

    /// The bar's own record of the files it shows.
    fn bar(&self, ctx: &Ctx) -> BarState;
    /// How many configs the running bar has swapped in so far; none when no bar answers.
    fn bar_loads(&self, ctx: &Ctx) -> Option<u64> {
        match self.bar(ctx) {
            BarState::Running(a) => Some(a.loads),
            _ => None,
        }
    }
    fn high_contrast(&self) -> bool;
    /// A steady clock in ms and a sleep, so the wait for the bar is testable.
    fn now_ms(&self) -> u64;
    fn sleep_ms(&self, ms: u64);
}

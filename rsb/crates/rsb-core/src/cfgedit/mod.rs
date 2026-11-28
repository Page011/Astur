//! The edit engine for `bar.toml` and `style.css`: pure functions from the
//! text of a file and a list of changes to the splices that make the change.
//! No file is read or written here.
//!
//! A change touches as few bytes as possible: a value, one line, or one item
//! of an array. Everything else, comments and line endings included, is
//! copied as it is. Every written token is rendered from a validated value;
//! caller text is never copied into a file.

mod colors;
mod plan;
mod text;
mod toml_splice;
mod validate;

pub use colors::{Conflict, Conflicts, END as COLORS_END, START as COLORS_START, VARS as ACCENT_VARS};
pub use colors::{accent_conflicts, block_lines};
pub use plan::Derive;
pub use text::apply_splices;
pub use validate::MAX_ZONE_ITEMS;

use crate::contract::{AccentSet, ConfigFile, FileHash, Notice, Op, PresetSwitch, SettingsError};
use crate::contrast::{self, AccentSurfaces};
use crate::css::Stylesheet;

/// One replacement in a file's text: bytes `start..end` become `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Splice {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// The current text of each file. `None` is a file that does not exist.
/// The text has no byte order mark and keeps its line endings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileTexts {
    pub bar_toml: Option<String>,
    pub style_css: Option<String>,
}

/// What the engine needs to know about the outside.
pub struct PlanContext<'a> {
    /// `set preset` with `reset`: also unset the user's bar keys, zone lists
    /// and accent block.
    pub reset: bool,
    /// The TOML of a built-in preset.
    pub preset_toml: &'a dyn Fn(&str) -> Option<String>,
    /// Whether a widget ref (`clock`, `my/weather`) is a built-in or an
    /// existing user file.
    pub widget_exists: &'a dyn Fn(&str) -> bool,
    /// The popups a widget opens, from its `about` table.
    pub widget_opens: &'a dyn Fn(&str) -> Vec<String>,
    /// Template text of a file, used when the file is missing.
    pub template: &'a dyn Fn(ConfigFile) -> String,
    /// The surfaces an accent is derived against.
    pub surfaces: &'a AccentSurfaces,
    /// The style sheet chain a `style.css` text makes: the files it imports,
    /// then the text itself. Used to find accent declarations that the colours
    /// block beats or that beat it, in imported files as well. None: only the
    /// text itself is read.
    pub style_chain: &'a dyn Fn(&str) -> Option<Stylesheet>,
}

/// The change to one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePlan {
    pub file: ConfigFile,
    /// The version of the bytes the plan was made against; writing it
    /// requires the file to still have exactly this version.
    pub base: FileHash,
    /// The file was missing and starts from its template.
    pub created: bool,
    /// The file started with a byte order mark, which is written back.
    pub bom: bool,
    /// Byte ranges of the text (the template's when `created`) and what
    /// replaces them, in order, not overlapping.
    pub splices: Vec<Splice>,
    /// The whole new text, without the byte order mark.
    pub after: String,
    /// One line for the log: `bar main: right [..] -> [..]`, validated
    /// tokens only.
    pub what: String,
}

/// Everything one request does.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The files that change, `bar.toml` first. A change that leaves a file
    /// as it is does not list it.
    pub files: Vec<FilePlan>,
    /// The validated changes, verb ops normalised to the request shape.
    pub changes: Vec<Op>,
    pub notices: Vec<Notice>,
    /// Only on `set preset`.
    pub preset_switch: Option<PresetSwitch>,
    /// Only on `set color.accent`.
    pub accent: Option<AccentSet>,
}

impl Plan {
    pub fn file(&self, file: ConfigFile) -> Option<&FilePlan> {
        self.files.iter().find(|f| f.file == file)
    }
}

/// Checks `ops` against the current files and plans the splices. Any refusal
/// refuses the whole request; nothing is half planned.
pub fn plan_change(texts: &FileTexts, ops: &[Op], ctx: &PlanContext<'_>) -> Result<Plan, SettingsError> {
    plan_change_with(texts, ops, ctx, &contrast::derive_accent)
}

/// `plan_change` with the accent derivation passed in, so the edit engine can
/// be tested apart from the contrast measurement.
pub fn plan_change_with(
    texts: &FileTexts,
    ops: &[Op],
    ctx: &PlanContext<'_>,
    derive: Derive<'_>,
) -> Result<Plan, SettingsError> {
    plan::plan(texts, ops, ctx, derive)
}

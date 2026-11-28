//! The settings contract: the request and reply shapes of `rsb settings`, the
//! same JSON on the command line and on the bar's pipe.
//!
//! Every type that is serialised lives here, so the crates that only build
//! `serde_json::Value`s (the binary, the script runtime) need no serde
//! dependency of their own. A reply is one line of JSON that carries
//! `contract`. Adding a field, key, op, error or notice keeps the version
//! (callers ignore what they do not know); removing or renaming one, or
//! changing a meaning, raises `CONTRACT`, and the old version stays served
//! until `CONTRACT_MIN` is raised on purpose.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fnv::fnv1a64;

/// The contract version this build speaks.
pub const CONTRACT: u32 = 1;
/// The oldest version this build still serves.
pub const CONTRACT_MIN: u32 = 1;
const _: () = assert!(CONTRACT_MIN <= CONTRACT);

/// Checks the `contract` a caller asked for: left out, or within
/// `CONTRACT_MIN..=CONTRACT`, is served.
pub fn check_contract(wanted: Option<u32>) -> Result<(), SettingsError> {
    match wanted {
        Some(v) if v > CONTRACT => Err(SettingsError::new(
            ErrorCode::ContractTooNew,
            format!("this rsb speaks settings contract {CONTRACT}; the caller wants {v}"),
        )
        .with_next("update RSB")),
        Some(v) if v < CONTRACT_MIN => Err(SettingsError::new(
            ErrorCode::ContractTooOld,
            format!("settings contract {v} is no longer served ({CONTRACT_MIN} to {CONTRACT})"),
        )
        .with_next("update the settings page")),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------- files and hashes

/// The two files settings may write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ConfigFile {
    #[serde(rename = "bar.toml")]
    BarToml,
    #[serde(rename = "style.css")]
    StyleCss,
}

impl ConfigFile {
    pub const ALL: [ConfigFile; 2] = [ConfigFile::BarToml, ConfigFile::StyleCss];

    /// The file's name in the config folder.
    pub fn name(self) -> &'static str {
        match self {
            ConfigFile::BarToml => crate::paths::BAR_TOML,
            ConfigFile::StyleCss => crate::paths::STYLE_CSS,
        }
    }
}

impl fmt::Display for ConfigFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for ConfigFile {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        ConfigFile::ALL.into_iter().find(|f| f.name() == s).ok_or(())
    }
}

/// The version of one file: the FNV-1a 64 of its raw bytes (a byte order
/// mark included), written `fnv1a64:<16 hex digits>`, or `none` for a file
/// that does not exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileHash {
    Missing,
    Fnv(u64),
}

impl FileHash {
    pub const PREFIX: &'static str = "fnv1a64:";

    pub fn of(bytes: &[u8]) -> FileHash {
        FileHash::Fnv(fnv1a64(bytes))
    }

    /// `None` is a missing file.
    pub fn of_file(bytes: Option<&[u8]>) -> FileHash {
        bytes.map_or(FileHash::Missing, FileHash::of)
    }
}

impl fmt::Display for FileHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileHash::Missing => f.write_str("none"),
            FileHash::Fnv(h) => write!(f, "{}{h:016x}", FileHash::PREFIX),
        }
    }
}

impl FromStr for FileHash {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        if s == "none" {
            return Ok(FileHash::Missing);
        }
        let hex = s
            .strip_prefix(FileHash::PREFIX)
            .ok_or_else(|| format!("'{s}' is not a file version: use none or fnv1a64:<16 hex digits>"))?;
        if hex.len() != 16 || !hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(format!("'{s}' is not a file version: 16 lower case hex digits after fnv1a64:"));
        }
        u64::from_str_radix(hex, 16).map(FileHash::Fnv).map_err(|e| e.to_string())
    }
}

impl Serialize for FileHash {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for FileHash {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        text.parse().map_err(de::Error::custom)
    }
}

/// The versions a change expects its files to have. Checked only for the
/// files the change writes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    #[serde(rename = "bar.toml", default, skip_serializing_if = "Option::is_none")]
    pub bar_toml: Option<FileHash>,
    #[serde(rename = "style.css", default, skip_serializing_if = "Option::is_none")]
    pub style_css: Option<FileHash>,
}

impl Expect {
    pub fn get(&self, file: ConfigFile) -> Option<FileHash> {
        match file {
            ConfigFile::BarToml => self.bar_toml,
            ConfigFile::StyleCss => self.style_css,
        }
    }

    pub fn set(&mut self, file: ConfigFile, hash: FileHash) {
        match file {
            ConfigFile::BarToml => self.bar_toml = Some(hash),
            ConfigFile::StyleCss => self.style_css = Some(hash),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.bar_toml.is_none() && self.style_css.is_none()
    }
}

/// The reply of the pipe line `applied`: the versions of the files the bar
/// shows (null before its first config is in) and the folders it reads.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedReply {
    #[serde(rename = "bar.toml")]
    pub bar_toml: Option<FileHash>,
    #[serde(rename = "style.css")]
    pub style_css: Option<FileHash>,
    pub diagnostics: u32,
    /// How many configs the bar has swapped in since it started. A write that leaves the bar showing the versions
    /// it showed before (a restore of what it already shows) is seen to land when this moves.
    #[serde(default)]
    pub loads: u64,
    pub config_dir: String,
    pub log_dir: String,
}

// ---------------------------------------------------------------- request

/// What a request asks for. `preview` is not here: only the command line
/// answers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verb {
    Get,
    Presets,
    Widgets,
    Schema,
    Change,
    Undo,
    Restore,
    Log,
}

impl Verb {
    pub fn name(self) -> &'static str {
        match self {
            Verb::Get => "get",
            Verb::Presets => "presets",
            Verb::Widgets => "widgets",
            Verb::Schema => "schema",
            Verb::Change => "change",
            Verb::Undo => "undo",
            Verb::Restore => "restore",
            Verb::Log => "log",
        }
    }

    /// Verbs that may write a file.
    pub fn writes(self) -> bool {
        matches!(self, Verb::Change | Verb::Undo | Verb::Restore)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Zone {
    Left,
    Center,
    Right,
}

impl Zone {
    pub const ALL: [Zone; 3] = [Zone::Left, Zone::Center, Zone::Right];

    pub fn name(self) -> &'static str {
        match self {
            Zone::Left => "left",
            Zone::Center => "center",
            Zone::Right => "right",
        }
    }
}

/// A place in a zone. `index` counts from 0; left out or -1 is the end.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub bar: String,
    pub zone: Zone,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<i32>,
}

/// The `from` of `move` and `remove`: a bar name, or the exact place when
/// the widget is twice in one zone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FromRef {
    Bar(String),
    At(Target),
}

/// One change. `undo` and `restore` appear in the log's `changes` only: a
/// `change` request holding them is refused.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Op {
    Set {
        key: String,
        value: Value,
    },
    Unset {
        key: String,
    },
    Move {
        widget: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<FromRef>,
        to: Target,
    },
    Add {
        widget: String,
        to: Target,
    },
    Remove {
        widget: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<FromRef>,
    },
    Undo {
        n: u32,
    },
    Restore {
        file: ConfigFile,
    },
}

/// One request, the same shape for `change`, the pipe and what the command
/// line verbs build. A file given to `change` may leave out `verb`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Request {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verb: Option<Verb>,
    #[serde(default, skip_serializing_if = "Expect::is_empty")]
    pub expect: Expect,
    /// `set preset` only: also unset the user's bar keys, zone lists and
    /// accent block.
    #[serde(default, skip_serializing_if = "is_false")]
    pub reset: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub dry_run: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<Op>,
    /// `get`: one bar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bar: Option<String>,
    /// `log`: how many entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<u32>,
    /// `undo`: the entry, which must be the newest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    /// `restore`: the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<ConfigFile>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Request {
    /// The request as it is sent: one line of JSON.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// Reads a request. It must be one JSON object (a derived reader would
    /// also take a bare array as the fields in order).
    pub fn from_json(text: &str) -> Result<Request, SettingsError> {
        let bad = |why: String| {
            SettingsError::new(ErrorCode::BadJson, format!("the request is not valid: {why}"))
                .with_next("send one JSON object as in docs\\settings.md")
        };
        let value: Value = serde_json::from_str(text).map_err(|e| {
            SettingsError::new(ErrorCode::BadJson, format!("the request is not JSON: {e}"))
                .with_next("send one JSON object as in docs\\settings.md")
        })?;
        if !value.is_object() {
            return Err(bad("it is not a JSON object".into()));
        }
        serde_json::from_value(value).map_err(|e| bad(e.to_string()))
    }
}

// ---------------------------------------------------------------- errors

macro_rules! error_codes {
    ($($variant:ident = $name:literal, $exit:literal;)*) => {
        /// Every refusal, with the exit code the command line returns for it.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum ErrorCode {
            $($variant,)*
        }

        impl ErrorCode {
            pub const ALL: &'static [ErrorCode] = &[$(ErrorCode::$variant,)*];

            pub fn as_str(self) -> &'static str {
                match self {
                    $(ErrorCode::$variant => $name,)*
                }
            }

            /// 2: the request is wrong; 1: it is right but cannot be done.
            pub fn exit_code(self) -> u8 {
                match self {
                    $(ErrorCode::$variant => $exit,)*
                }
            }
        }
    };
}

error_codes! {
    BadUsage = "bad_usage", 2;
    BadJson = "bad_json", 2;
    ContractTooNew = "contract_too_new", 2;
    ContractTooOld = "contract_too_old", 2;
    CliOnly = "cli_only", 2;
    UnknownKey = "unknown_key", 2;
    FileOnly = "file_only", 2;
    BadValue = "bad_value", 2;
    UnknownPreset = "unknown_preset", 2;
    UnknownBar = "unknown_bar", 2;
    UnknownWidget = "unknown_widget", 2;
    WidgetNotPlaced = "widget_not_placed", 2;
    AmbiguousWidget = "ambiguous_widget", 2;
    FileBroken = "file_broken", 1;
    Elevated = "elevated", 1;
    ConfigTooNew = "config_too_new", 1;
    FileChanged = "file_changed", 1;
    Busy = "busy", 1;
    ReadOnly = "read_only", 1;
    LinkedFile = "linked_file", 1;
    NotUtf8 = "not_utf8", 1;
    UnsupportedLayout = "unsupported_layout", 1;
    ColorsBlockBroken = "colors_block_broken", 1;
    ColorsBlockEdited = "colors_block_edited", 1;
    WriteFailed = "write_failed", 1;
    NothingToUndo = "nothing_to_undo", 1;
    UndoNotLatest = "undo_not_latest", 1;
    UndoConflict = "undo_conflict", 1;
    UndoBroken = "undo_broken", 1;
    NothingToRestore = "nothing_to_restore", 1;
    RenderFailed = "render_failed", 1;
    Internal = "internal", 1;
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The structured remedy of a refusal or notice, for a page to act on. One of
/// `{"verb": "restore", "file": "bar.toml"}`, `{"verb": "get"}`,
/// `{"verb": "undo", "n": 3}`, `{"op": "unset", "key": "bar.main.center"}` or
/// `{"open": "<path>"}` (a file a person can open and edit).
pub mod fixes {
    use super::ConfigFile;
    use serde_json::{Value, json};

    pub fn restore(file: ConfigFile) -> Value {
        json!({"verb": "restore", "file": file.name()})
    }

    pub fn verb(name: &str) -> Value {
        json!({"verb": name})
    }

    pub fn undo(n: u32) -> Value {
        json!({"verb": "undo", "n": n})
    }

    pub fn unset(key: &str) -> Value {
        json!({"op": "unset", "key": key})
    }

    pub fn open(path: &str) -> Value {
        json!({"open": path})
    }
}

/// A refusal. `message` is for the person. `next` may name a command, as a
/// hint to read; a page acts on `fix`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SettingsError {
    pub code: ErrorCode,
    #[serde(default)]
    pub file: Option<ConfigFile>,
    /// `bar.toml:7:3` when the message is about a place in a file.
    #[serde(default)]
    pub at: Option<String>,
    pub message: String,
    #[serde(default)]
    pub next: Option<String>,
    /// The remedy as data (see `fixes`), or null when the person has to look.
    #[serde(default)]
    pub fix: Option<Box<Value>>,
    /// Paths of before copies kept when a rollback could not run.
    #[serde(default)]
    pub copies: Vec<String>,
}

impl SettingsError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> SettingsError {
        SettingsError { code, file: None, at: None, message: message.into(), next: None, fix: None, copies: Vec::new() }
    }

    pub fn with_file(mut self, file: ConfigFile) -> SettingsError {
        self.file = Some(file);
        self
    }

    pub fn with_at(mut self, at: impl Into<String>) -> SettingsError {
        self.at = Some(at.into());
        self
    }

    pub fn with_next(mut self, next: impl Into<String>) -> SettingsError {
        self.next = Some(next.into());
        self
    }

    pub fn with_fix(mut self, fix: Value) -> SettingsError {
        self.fix = Some(Box::new(fix));
        self
    }

    pub fn with_copies(mut self, copies: Vec<String>) -> SettingsError {
        self.copies = copies;
        self
    }

    pub fn exit_code(&self) -> u8 {
        self.code.exit_code()
    }
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SettingsError {}

/// The answer to a request: a document (`ok: true`) or a refusal.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    /// A JSON object. `contract` and `ok` are added when it lacks them.
    Ok(Value),
    Err(SettingsError),
}

impl Reply {
    /// 0 for a document, the error's code otherwise.
    pub fn exit_code(&self) -> u8 {
        match self {
            Reply::Ok(_) => 0,
            Reply::Err(e) => e.exit_code(),
        }
    }

    /// The reply as the JSON object a caller reads.
    pub fn to_json(&self) -> Value {
        match self {
            Reply::Ok(Value::Object(map)) => {
                let mut map = map.clone();
                map.entry("contract").or_insert_with(|| Value::from(CONTRACT));
                map.entry("ok").or_insert(Value::Bool(true));
                Value::Object(map)
            }
            Reply::Ok(other) => serde_json::json!({"contract": CONTRACT, "ok": true, "value": other}),
            Reply::Err(e) => serde_json::json!({"contract": CONTRACT, "ok": false, "error": e}),
        }
    }

    /// One line of printable ASCII (non-ASCII as `\u` escapes), no newline:
    /// the same text on stdout and on the pipe.
    pub fn to_line(&self) -> String {
        crate::ipc::ascii_json(&self.to_json())
    }

    /// Reads a reply line back. A line that is not a reply object is an
    /// error text, not a refusal.
    pub fn from_line(line: &str) -> Result<Reply, String> {
        let value: Value = serde_json::from_str(line.trim()).map_err(|e| e.to_string())?;
        let Some(ok) = value.get("ok").and_then(Value::as_bool) else {
            return Err("the reply has no \"ok\" field".into());
        };
        if ok {
            return Ok(Reply::Ok(value));
        }
        let error = value.get("error").cloned().ok_or("the reply has no \"error\" field")?;
        serde_json::from_value(error).map(Reply::Err).map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------- notices

macro_rules! notice_codes {
    ($($variant:ident = $name:literal;)*) => {
        /// What a change did besides what was asked.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum NoticeCode {
            $($variant,)*
        }

        impl NoticeCode {
            pub const ALL: &'static [NoticeCode] = &[$(NoticeCode::$variant,)*];

            pub fn as_str(self) -> &'static str {
                match self {
                    $(NoticeCode::$variant => $name,)*
                }
            }
        }
    };
}

notice_codes! {
    HandEditKept = "hand_edit_kept";
    FileCreated = "file_created";
    ExampleTurnedLive = "example_turned_live";
    BarsNowListed = "bars_now_listed";
    NowYours = "now_yours";
    Unpinned = "unpinned";
    PopupUnreachable = "popup_unreachable";
    Shadowed = "shadowed";
    ReplacesYours = "replaces_yours";
    NotConfirmed = "not_confirmed";
    BarUsesOtherFolder = "bar_uses_other_folder";
    BarUnreachable = "bar_unreachable";
    AlreadyPlaced = "already_placed";
    BlockEditReplaced = "block_edit_replaced";
}

/// One notice. `extra` holds the fields a code adds (`line`, `count`, `zones`,
/// `widget`, `popup`, `at`, `bar_dir`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notice {
    pub code: NoticeCode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<ConfigFile>,
    pub message: String,
    /// What to do about it, or null when nothing needs doing.
    #[serde(default)]
    pub next: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Notice {
    pub fn new(code: NoticeCode, message: impl Into<String>) -> Notice {
        Notice { code, file: None, message: message.into(), next: None, extra: BTreeMap::new() }
    }

    pub fn with_file(mut self, file: ConfigFile) -> Notice {
        self.file = Some(file);
        self
    }

    pub fn with_next(mut self, next: impl Into<String>) -> Notice {
        self.next = Some(next.into());
        self
    }

    pub fn with_extra(mut self, key: &str, value: Value) -> Notice {
        self.extra.insert(key.to_string(), value);
        self
    }
}

// ---------------------------------------------------------------- read shapes

/// Which layer supplied a setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Built in, nothing set.
    Default,
    /// The preset's value.
    Preset,
    /// A line in the user's file.
    User,
}

/// The lowest ratio found among some pairs, and which pair it was.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lowest {
    pub lowest: f64,
    pub what: String,
    pub need: f64,
}

/// Contrast of one preset in one scheme as shipped: text pairs against 4.5,
/// icon and line pairs against 3.0.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SchemeContrast {
    pub pass: bool,
    pub text: Lowest,
    pub graphic: Lowest,
}

/// `SchemeContrast` for each scheme the preset defines.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PresetContrast {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dark: Option<SchemeContrast>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<SchemeContrast>,
}

// ---------------------------------------------------------------- change results

/// One of a user's own settings that outlives a preset switch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeptKey {
    pub bar: String,
    pub key: String,
    pub yours: Value,
    pub preset: Value,
    /// Equal to the preset's value now, but no longer following the
    /// preset's future changes.
    pub same: bool,
}

/// What a preset switch leaves of the user's own lines.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PresetSwitch {
    pub from: String,
    pub to: String,
    pub kept: Vec<KeptKey>,
    pub bars_listed: u32,
    pub preset_bars: u32,
    pub accent_kept: Option<String>,
}

/// The accent's companion colours for one scheme.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccentTokens {
    pub fg: String,
    pub fg_ratio: f64,
    pub hover: String,
    pub press: String,
    pub text: String,
    pub line: String,
    /// How far `text` and `line` were moved from the accent, in percent.
    pub moved: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccentSchemes {
    pub dark: AccentTokens,
    pub light: AccentTokens,
}

/// The accent as picked and what was derived from it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccentSet {
    pub value: String,
    pub schemes: AccentSchemes,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_file_hash_has_one_text_form_and_round_trips() {
        let h = FileHash::of(b"x");
        let text = h.to_string();
        assert!(text.starts_with("fnv1a64:") && text.len() == 8 + 16, "{text}");
        assert_eq!(text.parse::<FileHash>(), Ok(h));
        assert_eq!(FileHash::Missing.to_string(), "none");
        assert_eq!("none".parse::<FileHash>(), Ok(FileHash::Missing));
        assert_eq!(FileHash::Fnv(0xab).to_string(), "fnv1a64:00000000000000ab");
        for bad in [
            "",
            "fnv1a64:",
            "fnv1a64:AB",
            "fnv1a64:00000000000000AB",
            "sha:00000000000000ab",
            "fnv1a64:0000000000000zzz",
        ] {
            assert!(bad.parse::<FileHash>().is_err(), "{bad}");
        }
        assert_eq!(serde_json::to_value(h).unwrap(), json!(text));
        assert_eq!(serde_json::from_value::<FileHash>(json!(text)).unwrap(), h);
        assert!(serde_json::from_value::<FileHash>(json!(5)).is_err());
    }

    #[test]
    fn every_error_code_has_one_name_and_the_documented_exit_code() {
        assert_eq!(ErrorCode::ALL.len(), 32);
        for &code in ErrorCode::ALL {
            let json = serde_json::to_value(code).unwrap();
            assert_eq!(json, json!(code.as_str()), "the serde name is the documented name");
            assert_eq!(serde_json::from_value::<ErrorCode>(json).unwrap(), code);
        }
        let exit_two: Vec<&str> = ErrorCode::ALL.iter().filter(|c| c.exit_code() == 2).map(|c| c.as_str()).collect();
        assert_eq!(
            exit_two,
            [
                "bad_usage",
                "bad_json",
                "contract_too_new",
                "contract_too_old",
                "cli_only",
                "unknown_key",
                "file_only",
                "bad_value",
                "unknown_preset",
                "unknown_bar",
                "unknown_widget",
                "widget_not_placed",
                "ambiguous_widget"
            ]
        );
        assert!(ErrorCode::ALL.iter().all(|c| matches!(c.exit_code(), 1 | 2)), "3, 4 and 5 are never settings codes");
        let mut names: Vec<&str> = ErrorCode::ALL.iter().map(|c| c.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 32);
    }

    #[test]
    fn every_notice_code_has_one_name() {
        assert_eq!(NoticeCode::ALL.len(), 14);
        for &code in NoticeCode::ALL {
            assert_eq!(serde_json::to_value(code).unwrap(), json!(code.as_str()));
        }
    }

    #[test]
    fn the_documented_change_request_parses_and_goes_back_out_unchanged() {
        let text = r##"{"contract":1,
 "expect":{"bar.toml":"fnv1a64:3f0c9a1e5b7d2c44","style.css":"fnv1a64:91aa04c3e2b8f610"},
 "verb":"change","reset":false,"dry_run":false,
 "changes":[
  {"op":"set","key":"preset","value":"fluent"},
  {"op":"set","key":"color.accent","value":"#e5a50a"},
  {"op":"set","key":"bar.main.left","value":["workspaces","media"]},
  {"op":"unset","key":"bar.main.height"},
  {"op":"move","widget":"clock","from":"main","to":{"bar":"main","zone":"right","index":0}},
  {"op":"add","widget":"clock#utc","to":{"bar":"main","zone":"right","index":-1}},
  {"op":"remove","widget":"media","from":"main"}]}"##;
        let req: Request = serde_json::from_str(text).unwrap();
        assert_eq!(req.verb, Some(Verb::Change));
        assert_eq!(req.changes.len(), 7);
        assert_eq!(req.expect.get(ConfigFile::BarToml), Some(FileHash::Fnv(0x3f0c9a1e5b7d2c44)));
        assert!(matches!(
            &req.changes[4],
            Op::Move { from: Some(FromRef::Bar(b)), to: Target { zone: Zone::Right, index: Some(0), .. }, .. } if b == "main"
        ));
        // Defaults (reset, dry_run) are left out on the way back; the rest is equal.
        let again: Request = serde_json::from_value(req.to_json()).unwrap();
        assert_eq!(again, req);
        let mut expected: Value = serde_json::from_str(text).unwrap();
        expected.as_object_mut().unwrap().remove("reset");
        expected.as_object_mut().unwrap().remove("dry_run");
        assert_eq!(req.to_json(), expected);
    }

    #[test]
    fn a_file_for_change_may_leave_out_verb_and_the_exact_from_place_parses() {
        let req: Request = serde_json::from_str(
            r#"{"changes":[{"op":"remove","widget":"clock","from":{"bar":"main","zone":"center","index":2}}]}"#,
        )
        .unwrap();
        assert_eq!(req.verb, None);
        assert!(matches!(
            &req.changes[0],
            Op::Remove { from: Some(FromRef::At(Target { zone: Zone::Center, index: Some(2), .. })), .. }
        ));
        for bad in [
            r#"{"verb":"preview"}"#,
            r#"{"changes":[{"op":"explode"}]}"#,
            r#"{"changes":[{"op":"set","key":"preset"}]}"#,
            r#"{"changes":[{"op":"add","widget":"x","to":{"bar":"main","zone":"top"}}]}"#,
            r#"{"expect":{"bar.toml":"abc"}}"#,
            r#"[1]"#,
        ] {
            let err = Request::from_json(bad).expect_err(bad);
            assert_eq!(err.code, ErrorCode::BadJson, "{bad}");
        }
        // A file name that is not one of the two is refused, not skipped: a typo must not lose the check.
        let err = Request::from_json(r#"{"expect":{"bar.toml ":"none"}}"#).unwrap_err();
        assert!(err.message.contains("unknown field"), "{}", err.message);
        assert!(Request::from_json(r#"{"expect":{"bar.toml":"none","style.css":"none"}}"#).is_ok());
        // Fields a later version adds are ignored.
        assert!(Request::from_json(r#"{"verb":"get","colour":"red"}"#).is_ok());
        assert!(Request::from_json("{nope").unwrap_err().message.starts_with("the request is not JSON: "));
    }

    #[test]
    fn an_error_serialises_with_every_field_and_reads_back() {
        let err = SettingsError::new(ErrorCode::FileChanged, "bar.toml changed since it was read")
            .with_file(ConfigFile::BarToml)
            .with_next("read the settings again, then make the change again");
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(
            json,
            json!({"code":"file_changed","file":"bar.toml","at":null,
                   "message":"bar.toml changed since it was read",
                   "next":"read the settings again, then make the change again","fix":null,"copies":[]})
        );
        assert_eq!(serde_json::from_value::<SettingsError>(json).unwrap(), err);
        // A caller may leave out the fields it does not know.
        let min: SettingsError = serde_json::from_value(json!({"code":"busy","message":"m"})).unwrap();
        assert_eq!((min.file, min.at, min.next, min.copies.len()), (None, None, None, 0));
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn a_reply_is_one_ascii_line_with_the_contract_version_and_reads_back() {
        let ok = Reply::Ok(json!({"kind":"log","entries":[],"note":"caf\u{e9}"}));
        let line = ok.to_line();
        assert!(line.is_ascii() && !line.contains('\n') && line.contains("caf\\u00e9"), "{line}");
        let doc: Value = serde_json::from_str(&line).unwrap();
        assert_eq!((doc["contract"].clone(), doc["ok"].clone()), (json!(CONTRACT), json!(true)));
        assert_eq!(ok.exit_code(), 0);
        assert_eq!(Reply::from_line(&line), Ok(Reply::Ok(doc)));

        let err = Reply::Err(SettingsError::new(ErrorCode::UnknownKey, "unknown key 'x'").with_next("see schema"));
        let line = err.to_line();
        assert!(line.contains("\"ok\":false") && line.contains("\"contract\":1"), "{line}");
        assert_eq!(err.exit_code(), 2);
        assert_eq!(Reply::from_line(&line), Ok(err));
        assert!(Reply::from_line("not json").is_err());
        assert!(Reply::from_line("{\"contract\":1}").is_err());
        assert!(Reply::from_line("{\"ok\":false}").is_err());
    }

    #[test]
    fn a_notice_carries_its_extra_fields_flat() {
        let n = Notice::new(NoticeCode::BarsNowListed, "bar.toml now lists 1 bar")
            .with_file(ConfigFile::BarToml)
            .with_extra("count", json!(1))
            .with_next("delete the [[bar]] lines in bar.toml to follow the preset's bars again");
        let json = serde_json::to_value(&n).unwrap();
        assert_eq!(json["code"], "bars_now_listed");
        assert_eq!(json["count"], 1);
        assert_eq!(json["file"], "bar.toml");
        assert_eq!(serde_json::from_value::<Notice>(json).unwrap(), n);
        let plain = serde_json::to_value(Notice::new(NoticeCode::NotConfirmed, "m")).unwrap();
        assert_eq!(plain, json!({"code":"not_confirmed","message":"m","next":null}));
    }

    #[test]
    fn the_contract_window_serves_the_current_version_and_refuses_the_rest_in_words() {
        assert!(check_contract(None).is_ok());
        assert!(check_contract(Some(CONTRACT)).is_ok());
        let new = check_contract(Some(CONTRACT + 1)).unwrap_err();
        assert_eq!(new.code, ErrorCode::ContractTooNew);
        assert_eq!(new.message, "this rsb speaks settings contract 1; the caller wants 2");
        assert_eq!(new.next.as_deref(), Some("update RSB"));
        let old = check_contract(Some(0)).unwrap_err();
        assert_eq!(old.code, ErrorCode::ContractTooOld);
        assert_eq!(old.message, "settings contract 0 is no longer served (1 to 1)");
        assert_eq!((new.exit_code(), old.exit_code()), (2, 2));
    }

    #[test]
    fn the_applied_reply_and_the_accent_shapes_match_the_documented_json() {
        let applied = AppliedReply {
            bar_toml: Some(FileHash::Fnv(1)),
            style_css: None,
            diagnostics: 0,
            loads: 3,
            config_dir: "C:\\x".into(),
            log_dir: "C:\\x".into(),
        };
        let json = serde_json::to_value(&applied).unwrap();
        assert_eq!(json["bar.toml"], "fnv1a64:0000000000000001");
        assert_eq!(json["style.css"], Value::Null);
        assert_eq!(serde_json::from_value::<AppliedReply>(json).unwrap(), applied);

        let tokens = |text: &str| AccentTokens {
            fg: "#000000".into(),
            fg_ratio: 10.6,
            hover: "#e8ae22".into(),
            press: "#eab73b".into(),
            text: text.into(),
            line: text.into(),
            moved: 0,
        };
        let accent = AccentSet {
            value: "#e5a50a".into(),
            schemes: AccentSchemes { dark: tokens("#e5a50a"), light: tokens("#6e4f05") },
        };
        let json = serde_json::to_value(&accent).unwrap();
        assert_eq!(json["schemes"]["light"]["text"], "#6e4f05");
        assert_eq!(json["schemes"]["dark"]["moved"], 0);
        assert_eq!(serde_json::from_value::<AccentSet>(json).unwrap(), accent);
    }
}

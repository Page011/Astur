//! `bar.toml`: schema, defaults, presets.
//!
//! Every key has a default, so no file at all gives the full default bar.
//! An unknown key or a value of the wrong type is a warning with file:line
//! (that key falls back to its default); a TOML syntax error is an error and
//! the caller keeps the last good config.

use std::collections::BTreeMap;
use std::fmt;

use toml::Spanned as TomlSpanned;
use toml::de::{DeInteger, DeTable, DeValue};

use crate::css::parse::LineIndex;
use crate::diag::{DiagKind, Diagnostic, Severity, SourcePos};
use crate::value::Value;

/// The `config_version` this build writes and reads.
pub const CONFIG_VERSION: i64 = 1;
/// Preset used when `preset` is not set.
pub const DEFAULT_PRESET: &str = "default";
/// Presets shipped inside the binary.
pub const BUILTIN_PRESETS: &[&str] = &["default", "fluent", "astur-classic", "glass"];

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub config_version: i64,
    /// Name of a built-in preset (`BUILTIN_PRESETS`). Its TOML fills keys the
    /// user left out; its CSS sits under the user's `style.css`.
    pub preset: String,
    /// At least one bar unless the user wrote `bar = []`, which means no bars.
    pub bars: Vec<BarSpec>,
    /// `[widget."<ref>"]` tables, keyed by the ref text (`clock`, `clock#utc`,
    /// `my/weather`).
    pub widgets: BTreeMap<String, WidgetOptions>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BarSpec {
    /// CSS class of the bar element and the name `rsb toggle <name>` uses.
    /// Default `main` for the first bar, `bar2`, `bar3`... after.
    pub name: String,
    /// Default `[All]`.
    pub monitors: Vec<MonitorSelector>,
    /// Default `Top`.
    pub position: Position,
    /// Logical pixels (scaled by the monitor's DPI). Default 32, range 16..=128.
    pub height: u32,
    /// Default false. A floating bar keeps `margin` from the screen edges.
    pub floating: bool,
    /// Logical pixels, floating only. Default 8, range 0..=64.
    pub margin: u32,
    /// Logical pixels. Initial `border-radius` of the bar element (CSS wins).
    /// Default 0, or 10 when `floating`.
    pub radius: u32,
    /// Register as an appbar so maximised windows stay clear. Default true.
    pub reserve: bool,
    /// What the bar does while a fullscreen app is in front. Default `Below`.
    pub fullscreen: FullscreenMode,
    pub left: Vec<ZoneItem>,
    pub center: Vec<ZoneItem>,
    pub right: Vec<ZoneItem>,
    /// The widget of `center` the centre zone is centred on (its ref as
    /// written, e.g. `dashboard`), so a neighbour that widens grows away
    /// from it instead of moving it. None = the zone as a whole is centred.
    pub center_on: Option<String>,
    /// Where the `[[bar]]` table starts, for messages. None for defaults.
    pub pos: Option<SourcePos>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MonitorSelector {
    /// `"*"`
    All,
    /// `"primary"`
    Primary,
    /// `0`, `1`, ... in Windows' enumeration order.
    Index(u32),
    /// A device name as Windows reports it (`"DISPLAY2"` in `bar.toml` is
    /// accepted and stored in the full form), e.g. `"\\\\.\\DISPLAY2"`.
    Device(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FullscreenMode {
    /// Drop below the fullscreen window (lose topmost) until it closes.
    Below,
    /// Hide until the fullscreen window closes.
    Hide,
    /// Stay on top.
    Stay,
}

/// One widget ref in a zone list, with where it was written so the script
/// thread can say `bar.toml:7:12: unknown widget 'clok'; did you mean
/// 'clock'?`. `pos` is None for refs that came from defaults or a preset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZoneItem {
    pub widget: WidgetRef,
    pub pos: Option<SourcePos>,
}

impl ZoneItem {
    pub fn new(widget: WidgetRef) -> ZoneItem {
        ZoneItem { widget, pos: None }
    }
}

/// Names a widget may not take: they are the zone element ids
/// (`zone#left`), so `#left` in CSS would match both.
pub const RESERVED_WIDGET_NAMES: &[&str] = &["left", "center", "right"];

/// Longest widget name (folder parts included) and instance name, bytes.
pub const MAX_WIDGET_NAME: usize = 64;

/// Bar names: `a-z 0-9 _ -`, 1 to 32 bytes (a CSS class and a word on the
/// `rsb toggle` command line).
pub fn is_valid_bar_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 32 && name.bytes().all(is_name_byte)
}

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'
}

/// A widget in a zone: `clock`, `clock#utc` (a second clock with its own
/// options table), or `my/weather` (the file `widgets\my\weather.luau`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WidgetRef {
    /// `clock` or `my/weather`. Lower case `a-z 0-9 _ -`, `/` between
    /// folder parts, no leading or trailing `/`, no `..`, at most 64 bytes.
    pub name: String,
    /// The part after `#`, same character set without `/`.
    pub instance: Option<String>,
}

impl WidgetRef {
    /// The error says what is wrong in words a person can act on.
    pub fn parse(text: &str) -> Result<WidgetRef, String> {
        let (name, instance) = match text.split_once('#') {
            Some((n, i)) => (n, Some(i)),
            None => (text, None),
        };
        if name.is_empty() {
            return Err(format!("'{text}' has no widget name"));
        }
        if name.len() > MAX_WIDGET_NAME {
            return Err(format!("widget name is {} bytes; at most {MAX_WIDGET_NAME}", name.len()));
        }
        if let Some(bad) = name.bytes().find(|&b| b != b'/' && !is_name_byte(b)) {
            let hint = if bad.is_ascii_uppercase() { "; widget names are lower case" } else { "" };
            return Err(format!("'{name}' may only use a-z, 0-9, _ and -, with / between folders{hint}"));
        }
        if name.starts_with('/') || name.ends_with('/') || name.contains("//") {
            return Err(format!("'{name}' has an empty folder part; write it like 'my/weather'"));
        }
        if RESERVED_WIDGET_NAMES.contains(&name) {
            return Err(format!("'{name}' is the name of a zone; pick another widget name"));
        }
        let instance = match instance {
            None => None,
            Some("") => return Err(format!("'{text}' has nothing after '#'")),
            Some(i) if i.len() > MAX_WIDGET_NAME => {
                return Err(format!("instance name is {} bytes; at most {MAX_WIDGET_NAME}", i.len()));
            }
            Some(i) if !i.bytes().all(is_name_byte) => {
                return Err(format!("instance name '{i}' may only use a-z, 0-9, _ and -"));
            }
            Some(i) => Some(i.to_string()),
        };
        Ok(WidgetRef { name: name.to_string(), instance })
    }

    /// True for refs with a folder part (`my/weather`): always a user file.
    pub fn is_path(&self) -> bool {
        self.name.contains('/')
    }

    /// CSS class on the widget element: the name with `/` turned into `-`
    /// (`my/weather` -> `my-weather`).
    pub fn css_class(&self) -> String {
        self.name.replace('/', "-")
    }

    /// CSS id on the widget element: class plus `-instance`
    /// (`clock#utc` -> `clock-utc`).
    pub fn css_id(&self) -> String {
        match &self.instance {
            Some(i) => format!("{}-{i}", self.css_class()),
            None => self.css_class(),
        }
    }
}

impl fmt::Display for WidgetRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.instance {
            Some(i) => write!(f, "{}#{i}", self.name),
            None => f.write_str(&self.name),
        }
    }
}

/// A value with the place it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Spanned<T> {
    pub value: T,
    pub pos: SourcePos,
}

/// One `[widget."<ref>"]` table. The widget's script checks each key against
/// its declared option defaults and reports unknown keys and wrong types
/// with the stored position.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WidgetOptions {
    pub values: BTreeMap<String, Spanned<Value>>,
}

/// Result of loading. `config` is None when nothing usable came out (a TOML
/// syntax error): keep the last good config and show the diagnostics.
#[derive(Clone, Debug)]
pub struct ConfigLoad {
    pub config: Option<Config>,
    pub diagnostics: Vec<Diagnostic>,
}

/// The built-in values of a bar that sets nothing: the bar of the `default`
/// preset (a test in the binary crate keeps the two equal).
fn default_bar(index: usize) -> BarSpec {
    let list = |names: &[&str]| -> Vec<ZoneItem> {
        names.iter().map(|n| ZoneItem::new(WidgetRef::parse(n).expect("built-in widget names are valid"))).collect()
    };
    BarSpec {
        name: default_bar_name(index),
        monitors: vec![MonitorSelector::All],
        position: Position::Top,
        height: 36,
        floating: false,
        margin: 8,
        radius: 0,
        reserve: true,
        fullscreen: FullscreenMode::Below,
        left: list(&["workspaces", "media"]),
        center: list(&["memory", "cpu", "dashboard", "clock", "date"]),
        right: list(&["status", "battery", "tray", "record", "power"]),
        center_on: Some("dashboard".into()),
        pos: None,
    }
}

fn default_bar_name(index: usize) -> String {
    if index == 0 { "main".into() } else { format!("bar{}", index + 1) }
}

/// The config used when there is no `bar.toml` (and the fallback when the
/// very first `bar.toml` read is broken, so a first start never shows an
/// empty screen edge): one bar on every monitor,
/// top, 36 px, reserved, left `workspaces media`, center `memory cpu
/// dashboard clock date` centred on `dashboard`, right `status battery tray
/// record`, preset `default`.
pub fn default_config() -> Config {
    Config {
        config_version: CONFIG_VERSION,
        preset: DEFAULT_PRESET.to_string(),
        bars: vec![default_bar(0)],
        widgets: BTreeMap::new(),
    }
}

/// A bar table with only the keys that were written.
#[derive(Clone, Default)]
struct PartialBar {
    name: Option<String>,
    monitors: Option<Vec<MonitorSelector>>,
    position: Option<Position>,
    height: Option<u32>,
    floating: Option<bool>,
    margin: Option<u32>,
    radius: Option<u32>,
    reserve: Option<bool>,
    fullscreen: Option<FullscreenMode>,
    left: Option<Vec<ZoneItem>>,
    center: Option<Vec<ZoneItem>>,
    right: Option<Vec<ZoneItem>>,
    /// The ref as written and where.
    center_on: Option<(String, SourcePos)>,
    pos: Option<SourcePos>,
}

type OptionTable = BTreeMap<String, Spanned<Value>>;

/// One TOML file after reading: what it said, nothing filled in.
#[derive(Default)]
struct Layer {
    config_version: Option<(i64, SourcePos)>,
    preset: Option<(String, SourcePos)>,
    /// None = no `bar` key; Some(empty) = `bar = []`.
    bars: Option<Vec<PartialBar>>,
    /// Ref text -> (where the table is, options).
    widgets: BTreeMap<String, (SourcePos, OptionTable)>,
}

struct Reader<'a> {
    text: &'a str,
    file: &'a str,
    li: LineIndex,
    diags: Vec<Diagnostic>,
}

/// Keys a `[[bar]]` table takes.
pub const BAR_KEYS: &[&str] = &[
    "name",
    "monitors",
    "position",
    "height",
    "floating",
    "margin",
    "radius",
    "reserve",
    "fullscreen",
    "left",
    "center",
    "right",
    "center_on",
];
/// Keys the top of `bar.toml` takes.
pub const TOP_KEYS: &[&str] = &["config_version", "preset", "bar", "widget"];

fn type_name(v: &DeValue) -> &'static str {
    match v {
        DeValue::String(_) => "a string",
        DeValue::Integer(_) => "an integer",
        DeValue::Float(_) => "a float",
        DeValue::Boolean(_) => "a boolean",
        DeValue::Datetime(_) => "a datetime",
        DeValue::Array(_) => "an array",
        DeValue::Table(_) => "a table",
    }
}

fn int_of(i: &DeInteger) -> Option<i64> {
    i64::from_str_radix(&i.as_str().replace('_', ""), i.radix()).ok()
}

fn to_value(v: &DeValue) -> Value {
    match v {
        DeValue::String(s) => Value::Str(s.to_string()),
        DeValue::Integer(i) => int_of(i).map_or_else(|| Value::Str(i.as_str().into()), Value::Int),
        DeValue::Float(f) => {
            f.as_str().replace('_', "").parse::<f64>().map_or_else(|_| Value::Str(f.as_str().into()), Value::Num)
        }
        DeValue::Boolean(b) => Value::Bool(*b),
        DeValue::Datetime(d) => Value::Str(d.to_string()),
        DeValue::Array(a) => Value::List(a.iter().map(|e| to_value(e.get_ref())).collect()),
        DeValue::Table(t) => {
            Value::Map(t.iter().map(|(k, e)| (k.get_ref().to_string(), to_value(e.get_ref()))).collect())
        }
    }
}

impl Reader<'_> {
    fn pos(&self, offset: usize) -> SourcePos {
        let (l, c) = self.li.line_col(self.text, offset);
        SourcePos::new(self.file, l, c)
    }

    fn warn(&mut self, offset: usize, msg: impl Into<String>) {
        let p = self.pos(offset);
        self.diags.push(Diagnostic::warning(DiagKind::Config, Some(p), msg));
    }

    fn wrong_type(&mut self, key: &str, want: &str, v: &TomlSpanned<DeValue>) {
        self.warn(v.span().start, format!("'{key}' must be {want}, got {}", type_name(v.get_ref())));
    }

    fn string(&mut self, key: &str, v: &TomlSpanned<DeValue>) -> Option<String> {
        match v.get_ref() {
            DeValue::String(s) => Some(s.to_string()),
            _ => {
                self.wrong_type(key, "a string", v);
                None
            }
        }
    }

    fn boolean(&mut self, key: &str, v: &TomlSpanned<DeValue>) -> Option<bool> {
        match v.get_ref() {
            DeValue::Boolean(b) => Some(*b),
            _ => {
                self.wrong_type(key, "true or false", v);
                None
            }
        }
    }

    fn uint(&mut self, key: &str, v: &TomlSpanned<DeValue>, lo: i64, hi: i64) -> Option<u32> {
        let n = match v.get_ref() {
            DeValue::Integer(i) => int_of(i),
            DeValue::Float(f) => {
                let x: f64 = f.as_str().replace('_', "").parse().unwrap_or(f64::NAN);
                if x.fract() != 0.0 {
                    self.warn(v.span().start, format!("'{key}' must be a whole number"));
                    return None;
                }
                Some(x as i64)
            }
            _ => {
                self.wrong_type(key, "a number", v);
                return None;
            }
        };
        match n {
            Some(n) if (lo..=hi).contains(&n) => Some(n as u32),
            _ => {
                self.warn(v.span().start, format!("'{key}' must be {lo} to {hi}"));
                None
            }
        }
    }

    fn choice(&mut self, key: &str, v: &TomlSpanned<DeValue>, options: &[&str]) -> Option<usize> {
        let s = self.string(key, v)?;
        let found = options.iter().position(|o| o.eq_ignore_ascii_case(&s));
        if found.is_none() {
            let list = options.iter().map(|o| format!("\"{o}\"")).collect::<Vec<_>>().join(", ");
            self.warn(v.span().start, format!("'{key}' must be one of {list}, got \"{s}\""));
        }
        found
    }

    fn monitor(&mut self, v: &TomlSpanned<DeValue>) -> Option<MonitorSelector> {
        match v.get_ref() {
            DeValue::Integer(i) => match int_of(i) {
                Some(n) if (0..=64).contains(&n) => Some(MonitorSelector::Index(n as u32)),
                _ => {
                    self.warn(v.span().start, "'monitors' index must be 0 to 64");
                    None
                }
            },
            DeValue::String(s) => {
                let lower = s.to_ascii_lowercase();
                let digits = |rest: &str| {
                    (!rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())).then(|| rest.to_string())
                };
                if s.as_ref() == "*" {
                    Some(MonitorSelector::All)
                } else if lower == "primary" {
                    Some(MonitorSelector::Primary)
                } else if let Some(n) = lower.strip_prefix("display").and_then(digits) {
                    Some(MonitorSelector::Device(format!("\\\\.\\DISPLAY{n}")))
                } else if let Some(n) = lower.strip_prefix("\\\\.\\display").and_then(digits) {
                    Some(MonitorSelector::Device(format!("\\\\.\\DISPLAY{n}")))
                } else {
                    self.warn(
                        v.span().start,
                        format!(
                            "'monitors' entry \"{s}\" must be \"*\", \"primary\", a number, or a name like \"DISPLAY2\""
                        ),
                    );
                    None
                }
            }
            _ => {
                self.wrong_type(
                    "monitors",
                    "\"*\", \"primary\", a number, a name like \"DISPLAY2\" or a list of them",
                    v,
                );
                None
            }
        }
    }

    fn monitors(&mut self, v: &TomlSpanned<DeValue>) -> Option<Vec<MonitorSelector>> {
        match v.get_ref() {
            DeValue::Array(a) => {
                let out: Vec<MonitorSelector> = a.iter().filter_map(|e| self.monitor(e)).collect();
                if out.is_empty() {
                    self.warn(v.span().start, "'monitors' selects nothing; this bar will not be shown");
                }
                Some(out)
            }
            _ => self.monitor(v).map(|m| vec![m]),
        }
    }

    fn zone(&mut self, key: &str, v: &TomlSpanned<DeValue>) -> Option<Vec<ZoneItem>> {
        let DeValue::Array(a) = v.get_ref() else {
            self.wrong_type(key, "a list of widget names", v);
            return None;
        };
        let mut out = Vec::new();
        for e in a.iter() {
            match e.get_ref() {
                DeValue::String(s) => match WidgetRef::parse(s) {
                    Ok(widget) => out.push(ZoneItem { widget, pos: Some(self.pos(e.span().start)) }),
                    Err(msg) => self.warn(e.span().start, format!("bad entry in '{key}': {msg}")),
                },
                other => self.warn(
                    e.span().start,
                    format!("'{key}' entries must be widget names (strings), got {}", type_name(other)),
                ),
            }
        }
        Some(out)
    }

    fn bar(&mut self, t: &DeTable, at: usize) -> PartialBar {
        let mut b = PartialBar { pos: Some(self.pos(at)), ..PartialBar::default() };
        for (k, v) in t.iter() {
            let key = k.get_ref().as_ref();
            match key {
                "name" => {
                    if let Some(s) = self.string(key, v) {
                        if is_valid_bar_name(&s) {
                            b.name = Some(s);
                        } else {
                            self.warn(v.span().start, "'name' must use a-z, 0-9, _ and -, at most 32 characters");
                        }
                    }
                }
                "monitors" => b.monitors = self.monitors(v),
                "position" => {
                    b.position = self
                        .choice(key, v, &["top", "bottom"])
                        .map(|i| if i == 0 { Position::Top } else { Position::Bottom })
                }
                "height" => b.height = self.uint(key, v, 16, 128),
                "floating" => b.floating = self.boolean(key, v),
                "margin" => b.margin = self.uint(key, v, 0, 64),
                "radius" => b.radius = self.uint(key, v, 0, 64),
                "reserve" => b.reserve = self.boolean(key, v),
                "fullscreen" => {
                    b.fullscreen = self.choice(key, v, &["below", "hide", "stay"]).map(|i| match i {
                        0 => FullscreenMode::Below,
                        1 => FullscreenMode::Hide,
                        _ => FullscreenMode::Stay,
                    })
                }
                "left" => b.left = self.zone(key, v),
                "center" => b.center = self.zone(key, v),
                "right" => b.right = self.zone(key, v),
                "center_on" => {
                    if let Some(s) = self.string(key, v) {
                        b.center_on = Some((s, self.pos(v.span().start)));
                    }
                }
                _ => {
                    let hint = crate::suggest::hint(key, BAR_KEYS.iter().copied());
                    self.warn(k.span().start, format!("unknown key '{key}' in [[bar]]{hint}"));
                }
            }
        }
        b
    }

    fn widget_tables(&mut self, t: &DeTable, l: &mut Layer) {
        for (rk, rv) in t.iter() {
            let rname = rk.get_ref().as_ref();
            if let Err(msg) = WidgetRef::parse(rname) {
                self.warn(rk.span().start, format!("bad widget table name: {msg}"));
                continue;
            }
            let DeValue::Table(opts) = rv.get_ref() else {
                self.warn(
                    rv.span().start,
                    format!("'widget.{rname}' must be a table, got {}", type_name(rv.get_ref())),
                );
                continue;
            };
            let mut values = BTreeMap::new();
            for (ok, ov) in opts.iter() {
                values.insert(
                    ok.get_ref().to_string(),
                    Spanned { value: to_value(ov.get_ref()), pos: self.pos(ok.span().start) },
                );
            }
            l.widgets.insert(rname.to_string(), (self.pos(rk.span().start), values));
        }
    }

    fn layer(&mut self, root: &DeTable) -> Layer {
        let mut l = Layer::default();
        for (k, v) in root.iter() {
            let key = k.get_ref().as_ref();
            match key {
                "config_version" => {
                    let n = match v.get_ref() {
                        DeValue::Integer(i) => int_of(i).unwrap_or(-1),
                        _ => -1,
                    };
                    l.config_version = Some((n, self.pos(v.span().start)));
                }
                "preset" => l.preset = self.string(key, v).map(|s| (s, self.pos(v.span().start))),
                "bar" => match v.get_ref() {
                    DeValue::Array(a) => {
                        let mut bars = Vec::new();
                        for e in a.iter() {
                            match e.get_ref() {
                                DeValue::Table(t) => bars.push(self.bar(t, e.span().start)),
                                other => self.warn(
                                    e.span().start,
                                    format!("'bar' entries must be [[bar]] tables, got {}", type_name(other)),
                                ),
                            }
                        }
                        l.bars = Some(bars);
                    }
                    DeValue::Table(t) => {
                        self.warn(v.span().start, "'bar' must be a list of tables: write [[bar]], not [bar]");
                        l.bars = Some(vec![self.bar(t, v.span().start)]);
                    }
                    _ => self.wrong_type(key, "a list of [[bar]] tables", v),
                },
                "widget" => match v.get_ref() {
                    DeValue::Table(t) => self.widget_tables(t, &mut l),
                    _ => self.wrong_type(key, "a table", v),
                },
                _ => {
                    // The usual slip: a bar key written without its [[bar]] line above it.
                    let hint = if BAR_KEYS.contains(&key) {
                        "; it belongs under [[bar]]".to_string()
                    } else {
                        crate::suggest::hint(key, TOP_KEYS.iter().copied())
                    };
                    self.warn(k.span().start, format!("unknown key '{key}' at the top level{hint}"));
                }
            }
        }
        l
    }
}

/// The TOML parser's wording for the mistakes people actually make, in plain words.
fn plain_toml_message(msg: &str) -> String {
    if msg.starts_with("invalid basic string") {
        return "missing closing quote (\") at the end of the text".to_string();
    }
    msg.to_string()
}

/// Parses one TOML file into a layer. A syntax error is returned as an error
/// diagnostic, with the quote hint when it is an unquoted widget ref.
fn read_layer(text: &str, file: &str, diags: &mut Vec<Diagnostic>) -> Option<Layer> {
    let li = LineIndex::new(text);
    let parsed = match DeTable::parse(text) {
        Ok(p) => p,
        Err(e) => {
            let (line, col, line_text) = match e.span() {
                Some(r) => {
                    let (l, c) = li.line_col(text, r.start);
                    (l, c, text.lines().nth(l as usize - 1).unwrap_or(""))
                }
                None => (0, 0, ""),
            };
            let mut msg = plain_toml_message(e.message().trim_end());
            let t = line_text.trim();
            if let Some(name) = t.strip_prefix("[widget.").and_then(|r| r.strip_suffix(']'))
                && name.contains('#')
                && !name.contains('"')
            {
                msg.push_str(&format!("; quote the name: [widget.\"{name}\"]"));
            }
            let pos = (line > 0).then(|| SourcePos::new(file, line, col));
            diags.push(Diagnostic::error(DiagKind::Config, pos, msg));
            return None;
        }
    };
    let mut r = Reader { text, file, li, diags: Vec::new() };
    let layer = r.layer(parsed.get_ref());
    diags.extend(r.diags);
    Some(layer)
}

/// Loads a config. `user` is `(text, file name for messages)` of `bar.toml`
/// or None when the file does not exist. `preset_toml(name)` returns a
/// built-in preset's TOML (None = unknown preset: warning, `default` used).
///
/// Layering, lowest first: built-in defaults, the preset's TOML, the user's
/// file. A user `[[bar]]` list replaces the preset's list as a whole, but
/// each user bar N starts from the preset's bar N (or the preset's last bar
/// when the user wrote more), and only keys that preset bar does not set
/// either take the built-in defaults. So `[[bar]] height = 36` under
/// `preset = "glass"` keeps glass's floating, margin, radius and zones.
/// Widget option tables merge per key, user last.
///
/// Also reported as warnings with file:line: a zone entry that is not a
/// valid widget ref (the `WidgetRef::parse` message), a `[widget."x"]` table
/// whose ref is in no zone of any bar (`[widget."clok"] is not used by any
/// bar`), and, for a TOML syntax error caused by an unquoted ref such as
/// `[widget.clock#utc]`, the hint `quote the name: [widget."clock#utc"]`.
/// Monitor selectors accept `"DISPLAY2"` as a short form of the full device
/// name.
pub fn load_config(user: Option<(&str, &str)>, preset_toml: &dyn Fn(&str) -> Option<String>) -> ConfigLoad {
    let mut diags = Vec::new();
    let user_layer = match user {
        Some((text, file)) => match read_layer(text, file, &mut diags) {
            Some(l) => Some(l),
            None => return ConfigLoad { config: None, diagnostics: diags },
        },
        None => None,
    };
    if let Some((v, pos)) = user_layer.as_ref().and_then(|l| l.config_version.clone())
        && v != CONFIG_VERSION
    {
        diags.push(Diagnostic::error(
            DiagKind::Config,
            Some(pos),
            format!("this rsb reads config_version {CONFIG_VERSION}"),
        ));
        return ConfigLoad { config: None, diagnostics: diags };
    }

    // The preset layer.
    let wanted = user_layer.as_ref().and_then(|l| l.preset.clone());
    let mut preset_name = wanted.as_ref().map_or_else(|| DEFAULT_PRESET.to_string(), |(n, _)| n.clone());
    let mut preset_text = preset_toml(&preset_name);
    if preset_text.is_none() && preset_name != DEFAULT_PRESET {
        diags.push(Diagnostic::warning(
            DiagKind::Config,
            wanted.map(|(_, p)| p),
            format!(
                "unknown preset '{preset_name}'; using '{DEFAULT_PRESET}' (presets: {})",
                BUILTIN_PRESETS.join(", ")
            ),
        ));
        preset_name = DEFAULT_PRESET.to_string();
        preset_text = preset_toml(&preset_name);
    }
    let mut preset_layer = Layer::default();
    if let Some(text) = preset_text {
        let mut pd = Vec::new();
        if let Some(l) = read_layer(&text, &format!("presets/{preset_name}.toml"), &mut pd) {
            preset_layer = l;
        }
        // A broken built-in preset must not stop the bar from starting.
        diags.extend(pd.into_iter().map(|mut d| {
            d.severity = Severity::Warning;
            d
        }));
    }

    // Bars.
    let preset_bars = preset_layer.bars.clone().unwrap_or_default();
    let user_wrote_bars = user_layer.as_ref().is_some_and(|l| l.bars.is_some());
    let user_bars: Vec<PartialBar> = match user_layer.as_ref().and_then(|l| l.bars.clone()) {
        Some(b) => b,
        None if preset_bars.is_empty() => vec![PartialBar::default()],
        None => preset_bars.clone(),
    };
    let mut bars: Vec<BarSpec> = Vec::new();
    for (n, ub) in user_bars.iter().enumerate() {
        let same_index = preset_bars.get(n);
        let base = same_index.or(preset_bars.last()).cloned().unwrap_or_default();
        let d = default_bar(n);
        let d_center_on = d.center_on.clone();
        let floating = ub.floating.or(base.floating).unwrap_or(d.floating);
        bars.push(BarSpec {
            name: ub.name.clone().or_else(|| same_index.and_then(|p| p.name.clone())).unwrap_or(d.name),
            monitors: ub.monitors.clone().or(base.monitors).unwrap_or(d.monitors),
            position: ub.position.or(base.position).unwrap_or(d.position),
            height: ub.height.or(base.height).unwrap_or(d.height),
            floating,
            margin: ub.margin.or(base.margin).unwrap_or(d.margin),
            radius: ub.radius.or(base.radius).unwrap_or(if floating { 10 } else { 0 }),
            reserve: ub.reserve.or(base.reserve).unwrap_or(d.reserve),
            fullscreen: ub.fullscreen.or(base.fullscreen).unwrap_or(d.fullscreen),
            left: ub.left.clone().or(base.left).unwrap_or(d.left),
            center: ub.center.clone().or(base.center).unwrap_or(d.center),
            right: ub.right.clone().or(base.right).unwrap_or(d.right),
            center_on: None,
            pos: ub.pos.clone(),
        });
        // The pinned widget must be one of the centre zone's. A pin the
        // user wrote is reported when it is not; one that only came from the
        // preset is dropped without a word, since the user never wrote it
        // (they replaced `center` and the preset's pin simply does not fit).
        let written = user_wrote_bars && ub.center_on.is_some();
        let pinned = ub
            .center_on
            .clone()
            .or(base.center_on)
            .map(|(t, at)| (t, Some(at)))
            .or_else(|| d_center_on.map(|t| (t, None)));
        if let (Some((text, at)), Some(bar)) = (pinned, bars.last_mut()) {
            let found = WidgetRef::parse(&text).ok().filter(|r| bar.center.iter().any(|z| z.widget == *r));
            match found {
                Some(_) => bar.center_on = Some(text),
                None if !written => {}
                None => diags.push(Diagnostic::warning(
                    DiagKind::Config,
                    at,
                    format!("'center_on' names '{text}', which is not in center; the zone is centred as a whole"),
                )),
            }
        }
    }
    // Names must be unique: `rsb toggle <name>` and the CSS class use them.
    let mut seen: Vec<String> = Vec::new();
    for (n, bar) in bars.iter_mut().enumerate() {
        if seen.contains(&bar.name) {
            let mut fresh = default_bar_name(n);
            while seen.contains(&fresh) {
                fresh.push_str("-2");
            }
            diags.push(Diagnostic::warning(
                DiagKind::Config,
                bar.pos.clone(),
                format!("bar name '{}' is used twice; this bar is called '{fresh}'", bar.name),
            ));
            bar.name = fresh;
        }
        seen.push(bar.name.clone());
    }

    // Widget option tables: preset first, user keys win.
    let mut widgets: BTreeMap<String, WidgetOptions> = BTreeMap::new();
    for (name, (_, values)) in &preset_layer.widgets {
        widgets.entry(name.clone()).or_default().values.extend(values.clone());
    }
    let mut user_tables: Vec<(String, SourcePos)> = Vec::new();
    if let Some(l) = &user_layer {
        for (name, (pos, values)) in &l.widgets {
            widgets.entry(name.clone()).or_default().values.extend(values.clone());
            user_tables.push((name.clone(), pos.clone()));
        }
    }

    // A table no zone uses. A plain `[widget.clock]` also applies to every
    // `clock#...` instance, so it counts as used by any of them.
    let mut used: Vec<String> = Vec::new();
    for b in &bars {
        for item in b.left.iter().chain(&b.center).chain(&b.right) {
            used.push(item.widget.to_string());
            used.push(item.widget.name.clone());
        }
    }
    for (name, pos) in user_tables {
        if !used.contains(&name) {
            diags.push(Diagnostic::warning(
                DiagKind::Config,
                Some(pos),
                format!("[widget.\"{name}\"] is not used by any bar"),
            ));
        }
    }

    diags.sort_by_key(|d| d.pos.as_ref().map(|p| (p.file.clone(), p.line, p.col)));
    ConfigLoad {
        config: Some(Config { config_version: CONFIG_VERSION, preset: preset_name, bars, widgets }),
        diagnostics: diags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widget_refs() {
        let r = WidgetRef::parse("clock").unwrap();
        assert_eq!((r.name.as_str(), r.instance.as_deref()), ("clock", None));
        let r = WidgetRef::parse("clock#utc").unwrap();
        assert_eq!(
            (r.css_class(), r.css_id(), r.to_string()),
            ("clock".into(), "clock-utc".into(), "clock#utc".into())
        );
        let r = WidgetRef::parse("my/weather").unwrap();
        assert!(r.is_path());
        assert_eq!(r.css_class(), "my-weather");
        assert!(WidgetRef::parse("a/b-c_1/d#x_2").is_ok());
        assert!(WidgetRef::parse(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn widget_ref_refusals() {
        let cases = [
            ("", "no widget name"),
            ("#utc", "no widget name"),
            ("clock#", "nothing after"),
            ("Clock", "lower case"),
            ("my weather", "may only use"),
            ("../x", "may only use"),
            ("my\\x", "may only use"),
            ("/x", "empty folder"),
            ("x/", "empty folder"),
            ("a//b", "empty folder"),
            ("clock#a/b", "instance name"),
            ("left", "name of a zone"),
            ("center#2", "name of a zone"),
            ("c:x", "may only use"),
        ];
        for (text, want) in cases {
            let err = WidgetRef::parse(text).unwrap_err();
            assert!(err.contains(want), "{text:?}: {err}");
        }
        assert!(WidgetRef::parse(&"a".repeat(65)).unwrap_err().contains("at most 64"));
        assert!(WidgetRef::parse("my/left").is_ok(), "only a whole name is reserved");
    }

    #[test]
    fn bar_names() {
        assert!(is_valid_bar_name("main") && is_valid_bar_name("bar-2_x"));
        assert!(!is_valid_bar_name("") && !is_valid_bar_name("Main") && !is_valid_bar_name("a b"));
        assert!(!is_valid_bar_name(&"a".repeat(33)));
    }
}

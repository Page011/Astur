//! Reading what a widget module declares, without running the widget.
//!
//! The module is evaluated once in a sandbox that has no natives at all:
//! `rsb` is a table whose only working member is `widget`, which records the
//! table it is given; every other member is a stand-in that raises
//! "not available while listing widgets" when it is called, so a module that
//! starts a process, opens a connection or reads a file at its top level
//! gets an error instead of doing it. Timers and tasks do not exist, and
//! `setup` is never called. The table the module returns is read: its
//! options with their defaults and types, and its optional `about` table.

use std::time::{Duration, Instant};

use mlua::{Function, Lua, LuaOptions, MultiValue, StdLib, Table, Value as Lv, VmState};
use serde_json::{Map, Number, Value};

/// The message every stand-in raises.
pub const NOT_AVAILABLE: &str = "not available while listing widgets";

/// Longest a module may take to load (the bar's hard wall budget).
const LOAD_BUDGET: Duration = Duration::from_millis(250);
/// Memory a module may use while it loads.
const MEMORY_CAP: usize = 16 << 20;
/// Longest source read, bytes (the same cap as a widget file).
const MAX_SOURCE: usize = 1 << 20;
/// Longest title, characters.
const MAX_TITLE: usize = 20;
/// Most popups an `about` table may name, and the longest name.
const MAX_OPENS: usize = 8;
const MAX_OPEN_NAME: usize = 32;
/// Longest summary, characters.
const MAX_SUMMARY: usize = 160;
/// How deep and how wide a default value is copied.
const MAX_DEFAULT_DEPTH: usize = 4;
const MAX_DEFAULT_ITEMS: usize = 64;
/// Longest string kept from a default, characters.
const MAX_DEFAULT_TEXT: usize = 200;

const SPEC_KEYS: &[&str] = &["name", "options", "optional", "permissions", "setup", "about"];
const ABOUT_KEYS: &[&str] = &["title", "visible", "needs_options", "opens"];

/// The description of the widget `name` whose source is `source`:
///
/// ```json
/// {"title":"Media","visible":true,"needs_options":false,"opens":[],
///  "summary":"The current media session.",
///  "options":[{"key":"max_width","type":"number","default":280}]}
/// ```
///
/// `options` are in alphabetical order. `type` is the default's Luau type
/// (`string`, `number`, `bool`, `table`); an option with no default is `any`
/// with a null `default`. `Err` carries a message for the person: the module
/// did not load, or took too long.
pub fn widget(name: &str, source: &str) -> Result<Value, String> {
    if source.len() > MAX_SOURCE {
        return Err(format!("the file is larger than {} KiB", MAX_SOURCE / 1024));
    }
    let spec = load(source)?;
    let (options, optional) = options_of(&spec)?;
    let about = about_of(name, &spec)?;
    let mut rows: Vec<(String, Value, &'static str)> = Vec::new();
    for (key, value) in options {
        let (ty, default) = describe_default(&value);
        rows.push((key, default, ty));
    }
    for key in optional {
        if !rows.iter().any(|(k, _, _)| *k == key) {
            rows.push((key, Value::Null, "any"));
        }
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let options: Vec<Value> = rows
        .into_iter()
        .map(|(key, default, ty)| serde_json::json!({"key": key, "type": ty, "default": default}))
        .collect();
    Ok(serde_json::json!({
        "title": about.title,
        "visible": about.visible,
        "needs_options": about.needs_options,
        "opens": about.opens,
        "summary": summary_of(source),
        "options": options,
    }))
}

struct About {
    title: String,
    visible: bool,
    needs_options: bool,
    opens: Vec<String>,
}

/// Evaluates the module and returns the table `rsb.widget` was given.
fn load(source: &str) -> Result<LoadedSpec, String> {
    let fail = |e: mlua::Error| clean(&e.to_string());
    let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default()).map_err(fail)?;
    install(&lua).map_err(fail)?;
    lua.sandbox(true).map_err(fail)?;
    lua.set_memory_limit(MEMORY_CAP).map_err(fail)?;
    let started = Instant::now();
    lua.set_interrupt(move |_| {
        if started.elapsed() > LOAD_BUDGET {
            Err(mlua::Error::runtime(format!("took longer than {} ms to load", LOAD_BUDGET.as_millis())))
        } else {
            Ok(VmState::Continue)
        }
    });
    let chunk = lua.load(source).set_name("=widget").into_function().map_err(fail)?;
    let thread = lua.create_thread(chunk).map_err(fail)?;
    thread.sandbox().map_err(fail)?;
    let returned: MultiValue = thread.resume(()).map_err(fail)?;
    if thread.status() != mlua::prelude::LuaThreadStatus::Finished {
        return Err("a widget file cannot wait or yield while it loads".into());
    }
    let recorded: Option<Table> = lua.named_registry_value("described_spec").map_err(fail)?;
    let same = match (returned.iter().next(), &recorded) {
        (Some(Lv::Table(r)), Some(s)) => r.to_pointer() == s.to_pointer(),
        _ => false,
    };
    match recorded {
        Some(table) if same => Ok(LoadedSpec { table, _lua: lua }),
        _ => Err("a widget file must return rsb.widget { ... }".into()),
    }
}

/// The recorded spec and the VM that owns it.
struct LoadedSpec {
    table: Table,
    _lua: Lua,
}

/// The stand-in `rsb` and `require`, and the globals a module may not use.
fn install(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.globals();
    // One table stands in for everything: reading a member gives it back,
    // calling it raises.
    let stand_in = lua.create_table()?;
    let meta = lua.create_table()?;
    let me = stand_in.clone();
    meta.set("__index", lua.create_function(move |_, _: MultiValue| Ok(me.clone()))?)?;
    meta.set(
        "__call",
        lua.create_function(|_, _: MultiValue| -> mlua::Result<()> { Err(mlua::Error::runtime(NOT_AVAILABLE)) })?,
    )?;
    stand_in.set_metatable(Some(meta))?;

    let rsb = lua.create_table()?;
    let rsb_meta = lua.create_table()?;
    let for_members = stand_in.clone();
    rsb_meta.set("__index", lua.create_function(move |_, _: MultiValue| Ok(for_members.clone()))?)?;
    rsb.set_metatable(Some(rsb_meta))?;
    rsb.raw_set(
        "widget",
        lua.create_function(|lua, spec: Table| {
            for pair in spec.pairs::<Lv, Lv>() {
                let (key, _) = pair?;
                match key {
                    Lv::String(s) if SPEC_KEYS.contains(&&*s.to_string_lossy()) => {}
                    other => {
                        return Err(mlua::Error::runtime(format!(
                            "unknown field '{}' in rsb.widget",
                            match other {
                                Lv::String(s) => s.to_string_lossy().to_string(),
                                _ => "?".into(),
                            }
                        )));
                    }
                }
            }
            if !matches!(spec.get::<Lv>("setup")?, Lv::Function(_)) {
                return Err(mlua::Error::runtime("rsb.widget needs a setup function"));
            }
            lua.set_named_registry_value("described_spec", spec.clone())?;
            Ok(spec)
        })?,
    )?;

    let for_require = stand_in.clone();
    let rsb_for_require = rsb.clone();
    let require: Function = lua.create_function(move |_, spec: String| {
        Ok(if spec == "@rsb" { Lv::Table(rsb_for_require.clone()) } else { Lv::Table(for_require.clone()) })
    })?;
    globals.set("rsb", rsb)?;
    globals.set("require", require)?;
    globals.set("print", lua.create_function(|_, _: MultiValue| Ok(()))?)?;
    for gone in ["loadstring", "getfenv", "setfenv", "dofile", "loadfile"] {
        globals.set(gone, Lv::Nil)?;
    }
    Ok(())
}

fn clean(message: &str) -> String {
    // The first line is the message; later lines are a Lua stack trace.
    let first = message.lines().next().unwrap_or(message);
    first.trim_start_matches("runtime error: ").trim_start_matches("[string \"widget\"]:").trim().to_string()
}

type OptionRows = Vec<(String, Lv)>;

fn options_of(spec: &LoadedSpec) -> Result<(OptionRows, Vec<String>), String> {
    let mut rows = Vec::new();
    match spec.table.get::<Lv>("options").map_err(|e| e.to_string())? {
        Lv::Nil => {}
        Lv::Table(t) => {
            for pair in t.pairs::<Lv, Lv>() {
                let (key, value) = pair.map_err(|e| e.to_string())?;
                if let Lv::String(k) = key {
                    rows.push((k.to_string_lossy().to_string(), value));
                }
            }
        }
        _ => return Err("rsb.widget: options must be a table".into()),
    }
    let mut optional = Vec::new();
    if let Lv::Table(t) = spec.table.get::<Lv>("optional").map_err(|e| e.to_string())? {
        for item in t.sequence_values::<Lv>() {
            match item.map_err(|e| e.to_string())? {
                Lv::String(name) => optional.push(name.to_string_lossy().to_string()),
                _ => return Err("rsb.widget: optional must be a list of option names".into()),
            }
        }
    }
    Ok((rows, optional))
}

fn about_of(name: &str, spec: &LoadedSpec) -> Result<About, String> {
    let mut about =
        About { title: name.chars().take(MAX_TITLE).collect(), visible: true, needs_options: false, opens: Vec::new() };
    let table = match spec.table.get::<Lv>("about").map_err(|e| e.to_string())? {
        Lv::Nil => return Ok(about),
        Lv::Table(t) => t,
        _ => return Err("rsb.widget: about must be a table".into()),
    };
    for pair in table.pairs::<Lv, Lv>() {
        let (key, value) = pair.map_err(|e| e.to_string())?;
        let Lv::String(key) = key else {
            return Err(format!("about holds a key that is not a name; the fields are {}", ABOUT_KEYS.join(", ")));
        };
        match (&*key.to_string_lossy(), value) {
            ("title", Lv::String(s)) => {
                let title = s.to_string_lossy().to_string();
                let chars = title.chars().count();
                if chars == 0 || chars > MAX_TITLE || title.chars().any(char::is_control) {
                    return Err(format!("about.title must be 1 to {MAX_TITLE} characters on one line"));
                }
                about.title = title;
            }
            ("visible", Lv::Boolean(b)) => about.visible = b,
            ("needs_options", Lv::Boolean(b)) => about.needs_options = b,
            ("opens", Lv::Table(t)) => {
                for item in t.sequence_values::<Lv>() {
                    let Ok(Lv::String(popup)) = item else {
                        return Err("about.opens must be a list of popup names".to_string());
                    };
                    let popup = popup.to_string_lossy().to_string();
                    let valid = !popup.is_empty()
                        && popup.len() <= MAX_OPEN_NAME
                        && popup
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
                    if !valid {
                        return Err(format!("about.opens: '{popup}' is not a popup name"));
                    }
                    about.opens.push(popup);
                }
                if about.opens.len() > MAX_OPENS {
                    return Err(format!("about.opens names more than {MAX_OPENS} popups"));
                }
            }
            (other, _) if ABOUT_KEYS.contains(&other) => {
                return Err(format!("about.{other} has the wrong type"));
            }
            (other, _) => {
                return Err(format!("unknown field '{other}' in about; the fields are {}", ABOUT_KEYS.join(", ")));
            }
        }
    }
    Ok(about)
}

/// The Luau type name of a default and its value as JSON.
fn describe_default(value: &Lv) -> (&'static str, Value) {
    match value {
        Lv::Boolean(_) => ("bool", json_of(value, 0)),
        Lv::Integer(_) | Lv::Number(_) => ("number", json_of(value, 0)),
        Lv::String(_) => ("string", json_of(value, 0)),
        Lv::Table(_) => ("table", json_of(value, 0)),
        _ => ("any", Value::Null),
    }
}

fn json_of(value: &Lv, depth: usize) -> Value {
    match value {
        Lv::Boolean(b) => Value::Bool(*b),
        Lv::Integer(i) => Value::from(*i),
        Lv::Number(n) => {
            if n.fract() == 0.0 && n.abs() < 9.0e15 {
                Value::from(*n as i64)
            } else {
                Number::from_f64(*n).map_or(Value::Null, Value::Number)
            }
        }
        Lv::String(s) => Value::String(s.to_string_lossy().chars().take(MAX_DEFAULT_TEXT).collect()),
        Lv::Table(t) if depth < MAX_DEFAULT_DEPTH => {
            let len = t.raw_len();
            if len > 0 {
                Value::Array(
                    t.sequence_values::<Lv>()
                        .take(MAX_DEFAULT_ITEMS)
                        .filter_map(Result::ok)
                        .map(|v| json_of(&v, depth + 1))
                        .collect(),
                )
            } else {
                let mut map = Map::new();
                for pair in t.pairs::<Lv, Lv>().take(MAX_DEFAULT_ITEMS).filter_map(Result::ok) {
                    if let Lv::String(k) = pair.0 {
                        map.insert(k.to_string_lossy().to_string(), json_of(&pair.1, depth + 1));
                    }
                }
                Value::Object(map)
            }
        }
        _ => Value::Null,
    }
}

/// The first sentence of the comment at the top of the file, skipping a
/// `--!strict` style line.
fn summary_of(source: &str) -> String {
    let mut text = String::new();
    for line in source.lines() {
        let t = line.trim();
        if t.starts_with("--!") && text.is_empty() {
            continue;
        }
        match t.strip_prefix("--") {
            Some(c) if !c.starts_with('[') => {
                let c = c.trim();
                if c.is_empty() {
                    if text.is_empty() {
                        continue;
                    }
                    break;
                }
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(c);
                if sentence_end(&text).is_some() {
                    break;
                }
            }
            None if t.is_empty() && text.is_empty() => continue,
            _ => break,
        }
    }
    let end = sentence_end(&text).unwrap_or(text.len());
    let sentence = text[..end].trim();
    if sentence.chars().count() <= MAX_SUMMARY {
        return sentence.to_string();
    }
    // Too long for a line of a page: cut at a word and say so.
    let cut: String = sentence.chars().take(MAX_SUMMARY - 3).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{}...", cut.trim_end_matches([',', ';', ':', ' ']))
}

/// The byte index just after the first full stop that ends a sentence.
fn sentence_end(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    (0..bytes.len()).find(|&i| bytes[i] == b'.' && bytes.get(i + 1).is_none_or(|b| *b == b' ')).map(|i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const MEDIA: &str = "--!strict\n-- The current media session: a play or pause icon (what the player is\n-- doing now) and the track. A click plays or pauses.\nlocal rsb = require(\"@rsb\")\nlocal text = require(\"./lib/format\")\nreturn rsb.widget {\n\tabout = { title = \"Media\", opens = { \"media\" } },\n\toptions = { format = \"{artist} - {title}\", max_width = 280, progress = false, icon = \"glyph\" },\n\toptional = { \"command\" },\n\tpermissions = { control = { \"media\" } },\n\tsetup = function(props, ctx)\n\t\treturn rsb.text(props.format)\n\tend,\n}\n";

    #[test]
    fn a_module_is_described_from_its_table_and_comment() {
        let d = widget("media", MEDIA).unwrap();
        assert_eq!(d["title"], "Media");
        assert_eq!(d["visible"], true);
        assert_eq!(d["needs_options"], false);
        assert_eq!(d["opens"], json!(["media"]));
        assert_eq!(
            d["summary"],
            "The current media session: a play or pause icon (what the player is doing now) and the track."
        );
        assert_eq!(
            d["options"],
            json!([
                {"key": "command", "type": "any", "default": null},
                {"key": "format", "type": "string", "default": "{artist} - {title}"},
                {"key": "icon", "type": "string", "default": "glyph"},
                {"key": "max_width", "type": "number", "default": 280},
                {"key": "progress", "type": "bool", "default": false},
            ])
        );
    }

    #[test]
    fn describe_reads_options_without_setup() {
        // setup would raise (and `rsb.exec` is not available): it is never called.
        let src = "local rsb = require(\"@rsb\")\nreturn rsb.widget {\n\toptions = { every = 5, list = { \"a\", \"b\" }, nested = { x = 1 } },\n\tsetup = function(props, ctx)\n\t\terror(\"setup ran\")\n\tend,\n}\n";
        let d = widget("my/clock", src).unwrap();
        assert_eq!(d["title"], "my/clock");
        assert_eq!(d["summary"], "");
        assert_eq!(
            d["options"],
            json!([
                {"key": "every", "type": "number", "default": 5},
                {"key": "list", "type": "table", "default": ["a", "b"]},
                {"key": "nested", "type": "table", "default": {"x": 1}},
            ])
        );
    }

    #[test]
    fn describe_runs_no_natives() {
        let marker = std::env::temp_dir().join(format!("rsb-describe-marker-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let command = format!("cmd /d /c echo x> \"{}\"", marker.display());
        let src = format!(
            "local rsb = require(\"@rsb\")\nrsb.exec.run({command:?})\nreturn rsb.widget {{ setup = function() end }}\n"
        );
        let err = widget("x", &src).unwrap_err();
        assert!(err.contains(NOT_AVAILABLE), "{err}");
        assert!(!marker.exists(), "a process was started");
        // The same through a protected call: the module carries on and lists.
        let src = format!(
            "local rsb = require(\"@rsb\")\nlocal ok = pcall(rsb.exec.shell, {command:?})\nlocal ok2 = pcall(rsb.http.get, \"https://example.com\")\nlocal ok3 = pcall(rsb.file.read, \"C:/x\")\nlocal ok4 = pcall(rsb.timer.every, 1, function() end)\nreturn rsb.widget {{ options = {{ ran = (not ok) and (not ok2) and (not ok3) and (not ok4) }}, setup = function() end }}\n"
        );
        let d = widget("x", &src).unwrap();
        assert_eq!(d["options"], json!([{"key": "ran", "type": "bool", "default": true}]));
        assert!(!marker.exists(), "a process was started");
    }

    #[test]
    fn about_defaults_and_checks() {
        let plain = "local rsb = require(\"@rsb\")\nreturn rsb.widget { setup = function() end }\n";
        let d = widget("a-very-long-widget-name-over-twenty", plain).unwrap();
        assert_eq!(d["title"], "a-very-long-widget-n", "the default title is the name cut to 20 characters");
        assert_eq!(
            (d["visible"].clone(), d["needs_options"].clone(), d["opens"].clone()),
            (json!(true), json!(false), json!([]))
        );
        let wrap = |about: &str| {
            format!("local rsb = require(\"@rsb\")\nreturn rsb.widget {{ about = {about}, setup = function() end }}\n")
        };
        let d = widget(
            "power",
            &wrap("{ title = \"Power menu\", visible = false, needs_options = true, opens = { \"power\" } }"),
        )
        .unwrap();
        assert_eq!(
            (d["visible"].clone(), d["needs_options"].clone(), d["opens"].clone()),
            (json!(false), json!(true), json!(["power"]))
        );
        for (about, why) in [
            ("{ title = \"\" }", "about.title"),
            ("{ title = \"a title that is far too long\" }", "about.title"),
            ("{ title = 5 }", "about.title has the wrong type"),
            ("{ visible = \"yes\" }", "about.visible has the wrong type"),
            ("{ opens = { 5 } }", "about.opens"),
            ("{ opens = { \"Bad Name\" } }", "not a popup name"),
            ("{ colour = \"red\" }", "unknown field 'colour' in about"),
            ("5", "about must be a table"),
        ] {
            let err = widget("x", &wrap(about)).unwrap_err();
            assert!(err.contains(why), "{about}: {err}");
        }
    }

    #[test]
    fn a_module_that_does_not_load_is_an_error_with_no_trace() {
        for (src, why) in [
            ("local x = (", "widget"),
            ("return 5", "must return rsb.widget"),
            ("local rsb = require(\"@rsb\")\nreturn rsb.widget { options = 5, setup = function() end }", ""),
            (
                "local rsb = require(\"@rsb\")\nreturn rsb.widget { speed = 5, setup = function() end }",
                "unknown field 'speed'",
            ),
            ("local rsb = require(\"@rsb\")\nreturn rsb.widget { name = \"x\" }", "needs a setup function"),
            ("error(\"boom\")", "boom"),
            ("while true do end", "took longer"),
        ] {
            let err = widget("x", src).unwrap_err();
            assert!(err.contains(why), "{src}: {err}");
            assert!(!err.contains('\n'), "{err}");
        }
    }

    #[test]
    fn a_long_first_sentence_is_cut_at_a_word_and_says_so() {
        let words = "word ".repeat(60);
        let src = format!(
            "-- {words}end.
local rsb = require(\"@rsb\")
return rsb.widget {{ setup = function() end }}
"
        );
        let d = widget("x", &src).unwrap();
        let summary = d["summary"].as_str().unwrap();
        assert!(summary.ends_with("word..."), "{summary}");
        assert!(summary.chars().count() <= MAX_SUMMARY, "{}", summary.len());
        let short = "-- Short one. And a second sentence.
local rsb = require(\"@rsb\")
return rsb.widget { setup = function() end }
";
        assert_eq!(widget("x", short).unwrap()["summary"], "Short one.");
    }

    #[test]
    fn a_large_source_is_refused() {
        let big = format!("-- {}\nreturn 1", "x".repeat(MAX_SOURCE));
        assert!(widget("x", &big).unwrap_err().contains("larger than"));
    }
}

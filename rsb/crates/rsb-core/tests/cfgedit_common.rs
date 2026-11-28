//! Shared helpers of the edit engine tests: the shipped files as they were
//! when the engine was written (copies under `fixtures/cfgedit`), a fake
//! derivation for the accent, and the checks every plan must pass.
#![allow(dead_code)]

use rsb_core::cfgedit::{FilePlan, FileTexts, Plan, PlanContext, apply_splices, plan_change_with};
use rsb_core::color::Color;
use rsb_core::contract::{
    AccentSchemes, AccentSet, AccentTokens, ConfigFile, FromRef, NoticeCode, Op, SettingsError, Target, Zone,
};
use rsb_core::contrast::AccentSurfaces;
use serde_json::{Value, json};

pub const TEMPLATE_BAR: &str = include_str!("fixtures/cfgedit/template.bar.toml");
pub const TEMPLATE_CSS: &str = include_str!("fixtures/cfgedit/template.style.css");
pub const PRESETS: [(&str, &str); 4] = [
    ("default", include_str!("fixtures/cfgedit/preset-default.toml")),
    ("fluent", include_str!("fixtures/cfgedit/preset-fluent.toml")),
    ("glass", include_str!("fixtures/cfgedit/preset-glass.toml")),
    ("astur-classic", include_str!("fixtures/cfgedit/preset-astur-classic.toml")),
];

pub fn preset_toml(name: &str) -> Option<String> {
    PRESETS.iter().find(|(n, _)| *n == name).map(|(_, t)| (*t).to_string())
}

pub const BUILTIN_WIDGETS: &[&str] = &[
    "workspaces",
    "title",
    "clock",
    "date",
    "cpu",
    "memory",
    "battery",
    "volume",
    "network",
    "media",
    "text",
    "status",
    "tray",
    "record",
    "dashboard",
    "power",
];

pub fn opens(name: &str) -> Vec<String> {
    match name {
        "media" => vec!["media".into()],
        "date" => vec!["calendar".into()],
        "power" => vec!["power".into()],
        _ => Vec::new(),
    }
}

/// A fixed derivation: the accent itself, with shades that only depend on it.
pub fn fake_derive(color: Color, _: &AccentSurfaces) -> AccentSet {
    let hex = |c: Color| {
        format!(
            "#{:02x}{:02x}{:02x}",
            (c.r * 255.0).round() as u8,
            (c.g * 255.0).round() as u8,
            (c.b * 255.0).round() as u8
        )
    };
    let value = hex(color);
    let tokens = |fg: &str| AccentTokens {
        fg: fg.into(),
        fg_ratio: 10.0,
        hover: hex(color.lerp(Color::WHITE, 0.1)),
        press: hex(color.lerp(Color::WHITE, 0.2)),
        text: value.clone(),
        line: value.clone(),
        moved: 0,
    };
    AccentSet { value: value.clone(), schemes: AccentSchemes { dark: tokens("#000000"), light: tokens("#111111") } }
}

pub fn template_for(file: ConfigFile) -> String {
    match file {
        ConfigFile::BarToml => TEMPLATE_BAR.to_string(),
        ConfigFile::StyleCss => TEMPLATE_CSS.to_string(),
    }
}

pub fn run_plan(bar: Option<&str>, css: Option<&str>, ops: &[Op], reset: bool) -> Result<Plan, SettingsError> {
    let texts = FileTexts { bar_toml: bar.map(str::to_string), style_css: css.map(str::to_string) };
    let preset = |n: &str| preset_toml(n);
    let exists = |n: &str| BUILTIN_WIDGETS.contains(&n) || n == "my/weather";
    let op = |n: &str| opens(n);
    let template = |f: ConfigFile| template_for(f);
    let surfaces = AccentSurfaces::default();
    let ctx = PlanContext {
        reset,
        preset_toml: &preset,
        widget_exists: &exists,
        widget_opens: &op,
        template: &template,
        surfaces: &surfaces,
        style_chain: &|_| None,
    };
    plan_change_with(&texts, ops, &ctx, &fake_derive)
}

pub fn plan(bar: &str, ops: &[Op]) -> Result<Plan, SettingsError> {
    run_plan(Some(bar), None, ops, false)
}

pub fn set(key: &str, value: Value) -> Op {
    Op::Set { key: key.to_string(), value }
}

pub fn unset(key: &str) -> Op {
    Op::Unset { key: key.to_string() }
}

pub fn target(bar: &str, zone: Zone, index: Option<i32>) -> Target {
    Target { bar: bar.to_string(), zone, index }
}

pub fn add(widget: &str, bar: &str, zone: Zone, index: Option<i32>) -> Op {
    Op::Add { widget: widget.to_string(), to: target(bar, zone, index) }
}

pub fn remove(widget: &str, from: Option<&str>) -> Op {
    Op::Remove { widget: widget.to_string(), from: from.map(|b| FromRef::Bar(b.to_string())) }
}

pub fn mv(widget: &str, from: Option<&str>, bar: &str, zone: Zone, index: Option<i32>) -> Op {
    Op::Move {
        widget: widget.to_string(),
        from: from.map(|b| FromRef::Bar(b.to_string())),
        to: target(bar, zone, index),
    }
}

pub fn bar_file(plan: &Plan) -> &FilePlan {
    plan.file(ConfigFile::BarToml).expect("bar.toml is planned")
}

pub fn css_file(plan: &Plan) -> &FilePlan {
    plan.file(ConfigFile::StyleCss).expect("style.css is planned")
}

pub fn notice_codes(plan: &Plan) -> Vec<NoticeCode> {
    plan.notices.iter().map(|n| n.code).collect()
}

/// The lines whose first non-blank character is `#`.
pub fn comment_lines(text: &str) -> Vec<String> {
    text.lines().filter(|l| l.trim_start().starts_with('#')).map(str::to_string).collect()
}

pub fn crlf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// What every planned file must satisfy: the splices are in order, replaying
/// them on the original gives the new text, every byte outside them is
/// unchanged, and every comment line survives except the example lines the
/// plan turned live.
pub fn check_file_plan(original: &str, fp: &FilePlan, live_lines: &[usize]) {
    let original = original.strip_prefix('\u{feff}').unwrap_or(original);
    let mut cursor = 0usize;
    for s in &fp.splices {
        assert!(s.start >= cursor && s.end >= s.start, "splices out of order: {:?}", fp.splices);
        cursor = s.end;
    }
    assert!(cursor <= original.len());
    assert_eq!(apply_splices(original, &fp.splices), fp.after, "the splices do not make the new text");
    // Bytes outside the splices, in order, are the original's.
    let mut outside_old = String::new();
    let mut at = 0;
    for s in &fp.splices {
        outside_old.push_str(&original[at..s.start]);
        at = s.end;
    }
    outside_old.push_str(&original[at..]);
    let mut outside_new = String::new();
    let mut at_new = 0usize;
    let mut old_at = 0usize;
    for s in &fp.splices {
        let keep = s.start - old_at;
        outside_new.push_str(&fp.after[at_new..at_new + keep]);
        at_new += keep + s.text.len();
        old_at = s.end;
    }
    outside_new.push_str(&fp.after[at_new..]);
    assert_eq!(outside_old, outside_new, "bytes outside the splices changed");
    // Comment lines.
    let after_lines: Vec<&str> = fp.after.lines().collect();
    for (n, line) in original.lines().enumerate() {
        if line.trim_start().starts_with('#') && !live_lines.contains(&(n + 1)) {
            assert!(after_lines.contains(&line), "comment line {} lost: {line:?}", n + 1);
        }
    }
    // The same claims again from a plain line diff of the two texts, which does not look at the
    // splices at all: only lines that changed are different, and a changed line keeps its comment.
    let old_lines: Vec<&str> = original.lines().collect();
    let removed = removed_lines(&old_lines, &after_lines);
    for &i in &removed {
        let line = old_lines[i];
        let was_comment = line.trim_start().starts_with('#') || line.trim_start().starts_with("/*");
        assert!(!was_comment || live_lines.contains(&(i + 1)), "the diff removes comment line {}: {line:?}", i + 1);
    }
    // The diff and the splices must tell one story: a line the diff removes lies inside a splice, and a blank line
    // (which separates what a person wrote) is never one of them.
    let spans = splice_lines(original, &fp.splices);
    for &i in &removed {
        let (n, line) = (i + 1, old_lines[i]);
        assert!(
            spans.iter().any(|(a, b)| (*a..=*b).contains(&n)) || live_lines.contains(&n),
            "the diff removes line {n}, which no splice covers: {line:?}"
        );
        assert!(
            fp.file != ConfigFile::BarToml || !line.trim().is_empty() || live_lines.contains(&n),
            "the change removes the blank line {n}"
        );
    }
    if fp.file == ConfigFile::BarToml {
        check_values_kept(original, &fp.after);
        for (i, line) in old_lines.iter().enumerate() {
            let Some((code, comment)) = split_trailing_comment(line) else { continue };
            if live_lines.contains(&(i + 1)) {
                continue;
            }
            // A key line stays while its key does; an array item line (`"media",  # songs`) while its item does.
            let survives = match code.split_once('=') {
                Some((key, _)) => {
                    let key = key.trim();
                    after_lines.iter().any(|l| l.trim_start().starts_with(key) && l.contains('='))
                }
                None => {
                    let item = code.trim().trim_end_matches(',').trim();
                    let key = array_key(&old_lines, i);
                    item.starts_with('"') && key.is_some_and(|k| array_text(&after_lines, k).contains(item))
                }
            };
            if survives {
                let item =
                    code.split_once('=').map_or_else(|| code.trim().trim_end_matches(',').trim(), |(k, _)| k.trim());
                assert!(
                    after_lines.iter().any(|l| l.contains(item) && l.contains(comment)),
                    "line {}: the comment {comment:?} was lost",
                    i + 1
                );
            }
        }
    }
}

/// Every value of a bar.toml as `path -> value`: `[[bar]]` tables are indexed (`bar[0].right`), arrays of plain
/// values are one value.
fn leaves(text: &str) -> std::collections::BTreeMap<String, toml::Value> {
    fn walk(v: &toml::Value, path: String, out: &mut std::collections::BTreeMap<String, toml::Value>) {
        match v {
            toml::Value::Table(t) => {
                for (k, x) in t {
                    walk(x, if path.is_empty() { k.clone() } else { format!("{path}.{k}") }, out);
                }
            }
            toml::Value::Array(a) if !a.is_empty() && a.iter().all(toml::Value::is_table) => {
                for (i, x) in a.iter().enumerate() {
                    walk(x, format!("{path}[{i}]"), out);
                }
            }
            _ => {
                out.insert(path, v.clone());
            }
        }
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let table: toml::Table = toml::from_str(text).unwrap_or_else(|e| panic!("not TOML: {e}\n{text}"));
    let mut out = std::collections::BTreeMap::new();
    walk(&toml::Value::Table(table), String::new(), &mut out);
    out
}

/// The values of `original` that `after` no longer has or has differently, by path.
fn lost_values(original: &str, after: &str) -> Vec<String> {
    let new = leaves(after);
    leaves(original).into_iter().filter(|(path, v)| new.get(path) != Some(v)).map(|(path, _)| path).collect()
}

fn leaf_name(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

const ZONE_KEYS: [&str; 4] = ["left", "center", "right", "center_on"];

/// Reads `after` as TOML next to `original`: the only values a bar.toml change may lose or alter are the ones a
/// change can name (`preset`, a bar's position, height, floating, zone lists and `center_on`). A splice that is
/// too wide and takes an unrelated key with it fails here even though a splice covers the line.
fn check_values_kept(original: &str, after: &str) {
    for path in lost_values(original, after) {
        let name = leaf_name(&path);
        assert!(
            ["preset", "position", "height", "floating"].contains(&name) || ZONE_KEYS.contains(&name),
            "the change lost or altered {path}, which no change can name"
        );
    }
}

/// Stricter, with the changes known: each value the plan loses or alters is one of them. `bar.<name>.<key>`
/// names that key of that bar, an add names its zone, and the rest the zone lists and `center_on`.
///
/// A value counts when it is lost, altered or added, and it must belong to the bar the change names: a `bar.a.height`
/// change may not touch bar b, a move or remove only the bars it takes the widget from and puts it into, and a key
/// outside a `[[bar]]` is only ever `preset`. On top of the values, every key line the line diff removes or adds must
/// be a key some change can name, so a key line rewritten with the same value inside a wide splice is caught too.
pub fn check_values_named(original: &str, plan: &Plan) {
    let Some(fp) = plan.file(ConfigFile::BarToml) else { return };
    let original = original.strip_prefix('\u{feff}').unwrap_or(original);
    let (old, new) = (leaves(original), leaves(&fp.after));
    let (eff_old, eff_new) = (effective_bars(original), effective_bars(&fp.after));
    let in_file = old.keys().chain(new.keys()).filter_map(|p| split_bar_path(p)).map(|(i, _)| i + 1).max().unwrap_or(0);
    let count = eff_old.len().max(eff_new.len()).max(in_file);
    let bar_name = |idx: usize| -> Option<String> {
        let key = format!("bar[{idx}].name");
        old.get(&key)
            .or_else(|| new.get(&key))
            .and_then(|v| v.as_str().map(str::to_string))
            .or_else(|| eff_old.get(idx).or_else(|| eff_new.get(idx)).map(|b| b.name.clone()))
    };
    let bars_for = |token: &str| -> Vec<usize> {
        (0..count).filter(|&i| token == i.to_string() || bar_name(i).is_some_and(|n| n == token)).collect()
    };
    // The bars a move or remove takes the widget out of: the one named after `from`, else those that hold it.
    let sources = |widget: &str, from: &Option<FromRef>| -> Vec<usize> {
        match from {
            Some(FromRef::Bar(b)) => bars_for(b),
            Some(FromRef::At(t)) => bars_for(&t.bar),
            None => (0..eff_old.len())
                .filter(|&i| {
                    let b = &eff_old[i];
                    b.left.iter().chain(&b.center).chain(&b.right).any(|z| z.widget.to_string() == widget)
                })
                .collect(),
        }
    };
    // A table the change itself inserts brings its `name` with it.
    let made = |idx: usize| !old.keys().any(|k| k.starts_with(&format!("bar[{idx}].")));
    let mut changed: Vec<String> = lost_values(original, &fp.after);
    changed.extend(new.keys().filter(|p| !old.contains_key(*p)).cloned());
    for path in changed {
        let at = split_bar_path(&path);
        let named = plan.changes.iter().any(|op| match op {
            Op::Set { key, .. } | Op::Unset { key } => match key.split('.').collect::<Vec<_>>().as_slice() {
                ["preset"] => path == "preset" || at.is_some_and(|(_, f)| ZONE_KEYS.contains(&f)),
                // Changing the centre list also drops a `center_on` that named a widget it no longer holds.
                ["bar", token, field] => at.is_some_and(|(i, f)| {
                    bars_for(token).contains(&i) && (f == *field || (*field == "center" && f == "center_on"))
                }),
                _ => false,
            },
            Op::Add { to, .. } => {
                at.is_some_and(|(i, f)| bars_for(&to.bar).contains(&i) && (f == to.zone.name() || f == "center_on"))
            }
            Op::Move { widget, from, to } => at.is_some_and(|(i, f)| {
                ZONE_KEYS.contains(&f) && (bars_for(&to.bar).contains(&i) || sources(widget, from).contains(&i))
            }),
            Op::Remove { widget, from } => {
                at.is_some_and(|(i, f)| ZONE_KEYS.contains(&f) && sources(widget, from).contains(&i))
            }
            _ => false,
        }) || at.is_some_and(|(i, f)| f == "name" && made(i));
        assert!(named, "the plan lost, altered or added {path}, which no change of {:?} names", plan.changes);
    }
    // The same from the text: a key line the diff removes or adds names a key a change can name.
    let mut keys: Vec<&str> = vec!["name"];
    for op in &plan.changes {
        match op {
            Op::Set { key, .. } | Op::Unset { key } => match key.split('.').collect::<Vec<_>>().as_slice() {
                ["preset"] => keys.extend(["preset", "left", "center", "right", "center_on"]),
                ["bar", _, field] => keys.extend([*field, "center_on"]),
                _ => {}
            },
            Op::Add { to, .. } => keys.extend([to.zone.name(), "center_on"]),
            Op::Move { .. } | Op::Remove { .. } => keys.extend(ZONE_KEYS),
            _ => {}
        }
    }
    let (old_lines, new_lines): (Vec<&str>, Vec<&str>) = (original.lines().collect(), fp.after.lines().collect());
    let removed = removed_lines(&old_lines, &new_lines).into_iter().map(|i| old_lines[i]);
    let added = removed_lines(&new_lines, &old_lines).into_iter().map(|i| new_lines[i]);
    for line in removed.chain(added) {
        if let Some(key) = key_of(line) {
            assert!(
                keys.contains(&key) && (key != "name" || (0..in_file).any(&made)),
                "the diff changes the key line {line:?}, which no change of {:?} names",
                plan.changes
            );
        }
    }
}

/// `(table index, field)` of a path such as `bar[1].height`.
fn split_bar_path(path: &str) -> Option<(usize, &str)> {
    let (i, field) = path.strip_prefix("bar[")?.split_once("].")?;
    Some((i.parse().ok()?, field))
}

/// The bars of a bar.toml text as the bar reads them, with the preset's defaults.
fn effective_bars(text: &str) -> Vec<rsb_core::config::BarSpec> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    rsb_core::config::load_config(Some((text, "bar.toml")), &preset_toml).config.map(|c| c.bars).unwrap_or_default()
}

/// The key of a `key = value` line (not a comment, a table header or an array item).
fn key_of(line: &str) -> Option<&str> {
    let (key, _) = line.trim_start().split_once('=')?;
    let key = key.trim();
    (!key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')).then_some(key)
}

/// The key of the multi-line array that line `i` (0-based) sits in.
fn array_key<'a>(lines: &[&'a str], i: usize) -> Option<&'a str> {
    for line in lines[..i].iter().rev() {
        let t = line.trim();
        if t.contains(']') {
            return None;
        }
        if let Some((key, rest)) = t.split_once('=')
            && rest.trim_start().starts_with('[')
        {
            return Some(key.trim());
        }
    }
    None
}

/// Every line of every array assigned to `key`, joined.
fn array_text(lines: &[&str], key: &str) -> String {
    let (mut out, mut inside) = (String::new(), false);
    for line in lines {
        let t = line.trim_start();
        if !inside && t.split_once('=').is_some_and(|(k, rest)| k.trim() == key && rest.trim_start().starts_with('[')) {
            inside = true;
        }
        if inside {
            out.push_str(line);
            out.push('\n');
            inside = !line.contains(']');
        }
    }
    out
}

/// The 1-based first and last line each splice touches in `original`.
fn splice_lines(original: &str, splices: &[rsb_core::cfgedit::Splice]) -> Vec<(usize, usize)> {
    let line_of = |at: usize| original[..at.min(original.len())].matches('\n').count() + 1;
    splices.iter().map(|s| (line_of(s.start), line_of(s.end.saturating_sub(1).max(s.start)))).collect()
}

/// The indices of the lines of `old` that a line diff does not carry over to `new`.
fn removed_lines(old: &[&str], new: &[&str]) -> Vec<usize> {
    let (n, m) = (old.len(), new.len());
    let mut t = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if old[i] == new[j] { t[i + 1][j + 1] + 1 } else { t[i + 1][j].max(t[i][j + 1]) };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n && j < m {
        if old[i] == new[j] {
            i += 1;
            j += 1;
        } else if t[i + 1][j] >= t[i][j + 1] {
            out.push(i);
            i += 1;
        } else {
            j += 1;
        }
    }
    out.extend(i..n);
    out
}

/// A TOML line split into its code and its trailing `# comment` (a `#` inside
/// a string is not one).
fn split_trailing_comment(line: &str) -> Option<(&str, &str)> {
    let (mut quote, mut escaped) = (None::<char>, false);
    for (at, c) in line.char_indices() {
        match (quote, c) {
            (Some('"'), '\\') => escaped = !escaped,
            (Some(q), c) if c == q && !escaped => quote = None,
            (Some(_), _) => escaped = false,
            (None, '"' | '\'') => quote = Some(c),
            (None, '#') => {
                let code = &line[..at];
                return (!code.trim().is_empty()).then_some((code, line[at..].trim_end()));
            }
            _ => {}
        }
    }
    None
}

/// The 1-based lines of the `example_turned_live` notices.
pub fn live_lines(plan: &Plan) -> Vec<usize> {
    plan.notices
        .iter()
        .filter(|n| n.code == NoticeCode::ExampleTurnedLive)
        .filter_map(|n| n.extra.get("line").and_then(Value::as_u64).map(|l| l as usize))
        .collect()
}

/// The effective config of a bar.toml text, no diagnostics expected.
pub fn load(text: &str) -> rsb_core::config::Config {
    let l = rsb_core::config::load_config(Some((text, "bar.toml")), &preset_toml);
    l.config.expect("the text loads")
}

pub fn j(list: &[&str]) -> Value {
    json!(list)
}

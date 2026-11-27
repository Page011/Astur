//! The shape of every reply: one golden file per verb and per error code,
//! holding the field names and the kind of value each carries (never the
//! values). `docs/settings.md` must name every field a golden holds, and
//! `docs/settings-schema.json` is the committed output of `settings schema`.
//!
//! `RSB_BLESS=1 cargo test -p rsb --lib settings::goldens` rewrites the
//! golden files from what the code answers now.

use std::collections::BTreeSet;
use std::path::PathBuf;

use rsb_core::contract::{
    AccentSchemes, AccentSet, AccentTokens, ConfigFile, ErrorCode, KeptKey, Notice, NoticeCode, Reply, Request,
    SettingsError, Verb,
};
use serde_json::{Map, Value, json};

use super::backend::*;
use super::fake::*;
use super::schema::tests::expected_specs;
use super::schema::{BarDefaults, schema_of};
use super::{Ctx, serve_with};
use crate::cfgwrite::{Applied, LogEntry};

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden").join("settings")
}

/// Field names and the kind of each value; an array is shown by its first
/// item.
pub fn shape(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), shape(v))).collect::<Map<_, _>>()),
        Value::Array(a) => Value::Array(a.first().map(shape).into_iter().collect()),
        Value::String(_) => json!("string"),
        Value::Number(_) => json!("number"),
        Value::Bool(_) => json!("bool"),
        Value::Null => json!("null"),
    }
}

fn keys_of(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                out.insert(k.clone());
                keys_of(v, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|v| keys_of(v, out)),
        _ => {}
    }
}

fn check(name: &str, actual: &Value) {
    let path = golden_dir().join(format!("{name}.json"));
    let mut text = serde_json::to_string_pretty(actual).unwrap();
    text.push('\n');
    if std::env::var_os("RSB_BLESS").is_some() {
        std::fs::create_dir_all(golden_dir()).unwrap();
        std::fs::write(&path, &text).unwrap();
    }
    let have = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; run with RSB_BLESS=1 to write the goldens", path.display()));
    let have: Value = serde_json::from_str(&have).unwrap();
    assert_eq!(
        &have, actual,
        "the shape of {name} changed; if that is meant, run with RSB_BLESS=1 and update docs\\settings.md"
    );
}

fn ctx(dir: &std::path::Path) -> Ctx {
    Ctx::cli(dir.to_path_buf(), dir.join("logs"), false)
}

fn scratch(tag: &str) -> PathBuf {
    // Tests run side by side, so every call gets a folder of its own.
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let tag = format!("{tag}{}", COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("test-tmp")
        .join(format!("goldens-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("bar.toml"), "").unwrap();
    dir
}

fn request(verb: Verb) -> Request {
    Request { verb: Some(verb), ..Request::default() }
}

fn change() -> Request {
    Request {
        changes: vec![rsb_core::contract::Op::Set { key: "preset".into(), value: json!("glass") }],
        ..request(Verb::Change)
    }
}

fn fake() -> Fake {
    let fake = Fake::default();
    *fake.loaded.borrow_mut() = loaded();
    fake.presets.borrow_mut().push(preset_view("default"));
    fake.written.set(crate::cfgwrite::Written { bar_toml: Some(BAR_HASH), style_css: Some(CSS_HASH) });
    fake.last_good.set(true);
    fake
}

/// Every reply the goldens hold, by file name.
fn replies() -> Vec<(String, Value)> {
    let dir = scratch("replies");
    let here = std::fs::canonicalize(&dir).unwrap().to_string_lossy().into_owned();
    let fake = fake();
    let mut out: Vec<(String, Value)> = Vec::new();
    let mut add = |name: &str, reply: Reply| out.push((name.to_string(), shape(&reply.to_json())));

    fake.bar.borrow_mut().push_back(BarState::Running(applied_reply(&here, BAR_HASH, CSS_HASH)));
    add("get", serve_with(&request(Verb::Get), &ctx(&dir), &fake));
    {
        let mut l = fake.loaded.borrow_mut();
        l.bar_error = Some(FileError { at: "bar.toml:7:3".into(), message: "missing closing quote".into() });
        l.sources = None;
    }
    add("get_broken", serve_with(&request(Verb::Get), &ctx(&dir), &fake));
    *fake.loaded.borrow_mut() = loaded();
    add("presets", serve_with(&request(Verb::Presets), &ctx(&dir), &fake));
    *fake.listing.borrow_mut() = Listing {
        widgets: vec![WidgetView {
            name: "clock".into(),
            builtin: true,
            replaces_builtin: false,
            about: json!({"title": "Clock", "visible": true, "needs_options": false, "opens": ["x"], "summary": "Time of day.", "options": [{"key": "format", "type": "string", "default": "HH:mm"}]}),
        }],
        failed: vec![WidgetFailure {
            name: "my/weather".into(),
            source: "widgets/my/weather.luau".into(),
            message: "boom".into(),
        }],
    };
    add("widgets", serve_with(&request(Verb::Widgets), &ctx(&dir), &fake));
    add(
        "schema",
        Reply::Ok(schema_of(
            &expected_specs(),
            &BarDefaults { position: "top".into(), height: 36, floating: false, ..BarDefaults::default() },
        )),
    );
    fake.log_entries.borrow_mut().push(LogEntry {
        n: 18,
        time: "2026-11-27T10:15:00+11:00".into(),
        what: "style.css: accent none -> #e5a50a".into(),
        changes: vec![rsb_core::contract::Op::Set { key: "color.accent".into(), value: json!("#e5a50a") }],
        files: vec![(ConfigFile::StyleCss, rsb_core::contract::FileHash::Missing, CSS_HASH)],
    });
    add("log", serve_with(&request(Verb::Log), &ctx(&dir), &fake));

    let mut plan = plan_of("bar main: right [..] -> [..]");
    plan.preset_switch = Some(preset_switch());
    plan.accent = Some(AccentSet {
        value: "#e5a50a".into(),
        schemes: AccentSchemes {
            dark: AccentTokens {
                fg: "#000000".into(),
                fg_ratio: 10.6,
                hover: "#e8ae22".into(),
                press: "#eab73b".into(),
                text: "#e5a50a".into(),
                line: "#e5a50a".into(),
                moved: 0,
            },
            light: AccentTokens {
                fg: "#000000".into(),
                fg_ratio: 10.6,
                hover: "#e8ae22".into(),
                press: "#eab73b".into(),
                text: "#6e4f05".into(),
                line: "#9c7007".into(),
                moved: 52,
            },
        },
    });
    plan.preset_switch.as_mut().unwrap().kept.push(KeptKey {
        bar: "main".into(),
        key: "center".into(),
        yours: json!(["clock"]),
        preset: json!(["clock"]),
        same: true,
    });
    plan.notices.push(
        Notice::new(NoticeCode::PopupUnreachable, "the power menu is no longer reachable")
            .with_next("add power back to a zone to reach the power menu again")
            .with_extra("popup", json!("power"))
            .with_extra("widget", json!("power")),
    );
    *fake.plan.borrow_mut() = Some(Ok(plan));
    *fake.applied.borrow_mut() =
        Some(Ok(Applied { kept_copy: Some("C:\\x\\.undo\\18\\before.bar.toml".into()), ..bar_written() }));
    fake.bar.borrow_mut().clear();
    fake.bar.borrow_mut().push_back(BarState::Running(applied_reply(&here, NEW_BAR_HASH, CSS_HASH)));
    add("change", serve_with(&change(), &ctx(&dir), &fake));
    add("change_dry_run", serve_with(&Request { dry_run: true, ..change() }, &ctx(&dir), &fake));
    add("undo", serve_with(&request(Verb::Undo), &ctx(&dir), &fake));
    add(
        "restore",
        serve_with(&Request { file: Some(ConfigFile::BarToml), ..request(Verb::Restore) }, &ctx(&dir), &fake),
    );

    fake.previews.borrow_mut().push(crate::preview::Preview {
        name: "glass".into(),
        theme: rsb_core::preset::Scheme::Dark,
        mine: false,
        zones_from_preset: false,
        path: PathBuf::from("C:\\cache\\glass-dark-1920x100-abc.png"),
        width: 1920,
        height: 56,
        scale: 1.0,
        cached: false,
    });
    let args = super::grammar::PreviewArgs {
        presets: vec!["glass".into()],
        all: false,
        themes: vec![rsb_core::preset::Scheme::Dark],
        width: 1920,
        scale: 1.0,
        mine: false,
        zones_from_preset: false,
        cache_dir: None,
    };
    add("preview", super::preview(&args, &ctx(&dir), &fake));
    out
}

/// A refusal with every field filled in.
fn full_error(code: ErrorCode) -> SettingsError {
    SettingsError::new(code, "message")
        .with_file(ConfigFile::BarToml)
        .with_at("bar.toml:7:3")
        .with_next("next")
        .with_copies(vec!["C:\\x\\.undo\\1\\before.bar.toml".into()])
}

#[test]
fn settings_reply_goldens() {
    let mut keys = BTreeSet::new();
    for (name, shape) in replies() {
        keys_of(&shape, &mut keys);
        check(&name, &shape);
    }
    for &code in ErrorCode::ALL {
        let reply = Reply::Err(full_error(code));
        let golden = json!({"code": code.as_str(), "exit": code.exit_code(), "shape": shape(&reply.to_json())});
        keys_of(&golden["shape"], &mut keys);
        check(&format!("error_{}", code.as_str()), &golden);
    }
    assert!(keys.len() > 80, "{} field names in all", keys.len());
    // No golden is left over from a verb or a code that no longer exists.
    let mut wanted: BTreeSet<String> = replies().into_iter().map(|(n, _)| format!("{n}.json")).collect();
    wanted.extend(ErrorCode::ALL.iter().map(|c| format!("error_{}.json", c.as_str())));
    let on_disk: BTreeSet<String> = std::fs::read_dir(golden_dir())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".json"))
        .collect();
    assert_eq!(on_disk, wanted);
}

fn docs() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("docs").join("settings.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())).replace("\r\n", "\n")
}

#[test]
fn settings_md_names_every_field_every_code_and_every_verb() {
    let text = docs();
    let mut keys = BTreeSet::new();
    for (_, shape) in replies() {
        keys_of(&shape, &mut keys);
    }
    keys_of(&shape(&Reply::Err(full_error(ErrorCode::Busy)).to_json()), &mut keys);
    let mut missing: Vec<String> = keys.iter().filter(|k| !text.contains(&format!("`{k}`"))).cloned().collect();
    for &code in ErrorCode::ALL {
        if !text.contains(&format!("`{}`", code.as_str())) {
            missing.push(format!("error {}", code.as_str()));
        }
    }
    for &code in NoticeCode::ALL {
        if !text.contains(&format!("`{}`", code.as_str())) {
            missing.push(format!("notice {}", code.as_str()));
        }
    }
    for extra in ["line", "count", "zones", "widget", "popup", "at", "bar_dir"] {
        if !text.contains(&format!("`{extra}`")) {
            missing.push(format!("notice field {extra}"));
        }
    }
    for verb in super::grammar::VERBS {
        if !text.contains(&format!("rsb settings {verb}")) {
            missing.push(format!("verb {verb}"));
        }
    }
    for flag in [
        "--contract",
        "--expect",
        "--dry-run",
        "--no-wait",
        "--reset",
        "--cache-dir",
        "--bar",
        "--from",
        "--last",
        "--theme",
        "--width",
        "--scale",
        "--mine",
        "--all",
        "--config-dir",
        "--log-dir",
    ] {
        if !text.contains(flag) {
            missing.push(format!("option {flag}"));
        }
    }
    assert!(missing.is_empty(), "docs\\settings.md does not mention: {missing:?}");
}

#[test]
fn settings_schema_matches_docs() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("docs").join("settings-schema.json");
    // The parser's own key table, so the file and the reader cannot drift apart.
    assert!(!rsb_core::config::BAR_SPECS.is_empty(), "the parser lists no bar keys");
    let reply = Reply::Ok(schema_of(rsb_core::config::BAR_SPECS, &super::schema::default_bar_values()));
    let generated = reply.to_json();
    if std::env::var_os("RSB_BLESS").is_some() {
        let mut text = serde_json::to_string_pretty(&generated).unwrap();
        text.push('\n');
        std::fs::write(&path, text).unwrap();
    }
    let committed: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
            .expect("docs/settings-schema.json is JSON");
    assert_eq!(
        committed, generated,
        "docs/settings-schema.json is not what `rsb settings schema` prints; regenerate it with: rsb settings schema > docs\\settings-schema.json (or RSB_BLESS=1 cargo test -p rsb --lib settings_schema_matches_docs)"
    );
}

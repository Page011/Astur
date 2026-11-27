//! `serve` against a fake backend: every verb, every transport rule, the
//! wait for the bar, the log lines and the exit codes.

use std::path::PathBuf;

use helpers::*;
use rsb_core::cfgedit::Plan;
use rsb_core::contract::{
    CONTRACT, ConfigFile, ErrorCode, FileHash, KeptKey, Notice, NoticeCode, Op, Reply, Request, SettingsError, Verb,
};
use serde_json::{Value, json};

use super::backend::*;
use super::fake::*;
use super::{Ctx, Transport, guarded, line_with, preview, serve_line, serve_with};
use crate::cfgwrite::{Applied, LogEntry, Written};

/// Helpers shared by every test below (a folder that holds a config file, so a
/// write to it is allowed).
mod helpers {
    use std::path::{Path, PathBuf};

    pub fn scratch(tag: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target")
            .join("test-tmp")
            .join(format!("settings-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bar.toml"), "preset = \"fluent\"\n").unwrap();
        dir
    }

    pub fn canonical(dir: &Path) -> String {
        std::fs::canonicalize(dir).unwrap().to_string_lossy().into_owned()
    }
}

fn cli(dir: &std::path::Path) -> Ctx {
    Ctx::cli(dir.to_path_buf(), dir.join("logs"), false)
}

fn pipe(dir: &std::path::Path, client_ok: bool) -> Ctx {
    Ctx {
        transport: Transport::Pipe,
        config_dir: dir.to_path_buf(),
        log_dir: dir.join("logs"),
        applied: None,
        no_wait: true,
        pipe_client_ok: client_ok,
    }
}

fn request(verb: Verb) -> Request {
    Request { verb: Some(verb), ..Request::default() }
}

fn change_request() -> Request {
    Request { changes: vec![Op::Set { key: "preset".into(), value: json!("glass") }], ..request(Verb::Change) }
}

fn ok(reply: Reply) -> Value {
    match reply {
        Reply::Ok(_) => reply.to_json(),
        Reply::Err(e) => panic!("refused: {} {}", e.code, e.message),
    }
}

fn err(reply: Reply) -> SettingsError {
    match reply {
        Reply::Err(e) => e,
        Reply::Ok(v) => panic!("served: {v}"),
    }
}

fn backend() -> Fake {
    let fake = Fake::default();
    *fake.loaded.borrow_mut() = loaded();
    fake.presets.borrow_mut().push(preset_view("default"));
    *fake.plan.borrow_mut() = Some(Ok(plan_of("preset default -> glass")));
    *fake.applied.borrow_mut() = Some(Ok(bar_written()));
    fake
}

// ---------------------------------------------------------------- get

#[test]
fn get_reports_where_every_value_came_from() {
    let dir = scratch("get");
    let fake = backend();
    fake.written.set(Written { bar_toml: Some(BAR_HASH), style_css: Some(FileHash::Fnv(1)) });
    fake.last_good.set(true);
    fake.high_contrast.set(true);
    fake.bar.borrow_mut().push_back(BarState::Running(applied_reply(&canonical(&dir), BAR_HASH, CSS_HASH)));
    let v = ok(serve_with(&request(Verb::Get), &cli(&dir), &fake));
    assert_eq!(v["kind"], "settings");
    assert_eq!(
        v["files"]["bar.toml"],
        json!({"hash": BAR_HASH.to_string(), "error": null, "edited_by_hand": false, "last_good": true})
    );
    assert_eq!(v["files"]["style.css"]["edited_by_hand"], true, "style.css differs from what settings last wrote");
    assert_eq!(v["preset"], json!({"value": "fluent", "source": "user", "at": "bar.toml:1:1"}));
    let bar = &v["bars"][0];
    assert_eq!((bar["name"].clone(), bar["index"].clone()), (json!("main"), json!(0)));
    assert_eq!(bar["height"], json!({"value": 40, "source": "user", "at": "bar.toml:4:1", "editable": true}));
    assert_eq!(bar["position"]["source"], "preset");
    assert_eq!(bar["floating"], json!({"value": false, "source": "default", "at": null, "editable": true}));
    assert_eq!(bar["center_on"], json!({"value": "dashboard", "source": "default", "at": null, "editable": false}));
    assert_eq!(bar["monitors"]["value"], json!(["*"]));
    assert_eq!(bar["fullscreen"]["value"], "below");
    for key in ["margin", "radius", "reserve", "fullscreen", "monitors", "center_on"] {
        assert_eq!(bar[key]["editable"], false, "{key}");
    }
    for key in ["position", "height", "floating", "left", "center", "right"] {
        assert_eq!(bar[key]["editable"], true, "{key}");
    }
    assert!(bar["left"]["value"].as_array().unwrap().iter().all(Value::is_string));
    let accent = &v["colors"]["accent"];
    assert_eq!((accent["label"].clone(), accent["editable"].clone()), (json!("Accent"), json!(true)));
    assert_eq!(
        accent["dark"],
        json!({"value": "#e5a50a", "source": "user", "at": "style.css:42:5", "shadowed_by": null, "overridden": []})
    );
    assert_eq!(accent["light"]["shadowed_by"], "style.css:50:3");
    assert_eq!(accent["light"]["overridden"], json!(["style.css:12:5"]));
    assert_eq!(v["high_contrast"], true);
    assert_eq!(
        v["bar"],
        json!({"running": true, "shows_files": true, "diagnostics": 0, "config_dir": canonical(&dir)})
    );
    assert_eq!(v["config_dir"], dir.to_string_lossy().as_ref());
    assert_eq!(v["contract"], CONTRACT);
    assert_eq!(fake.called("plan") + fake.called("apply"), 0, "a read writes nothing");
}

#[test]
fn a_file_settings_never_wrote_is_not_edited_by_hand() {
    let dir = scratch("never");
    let fake = backend();
    let v = ok(serve_with(&request(Verb::Get), &cli(&dir), &fake));
    assert_eq!(v["files"]["bar.toml"]["edited_by_hand"], false);
    assert_eq!(v["files"]["style.css"]["edited_by_hand"], false);
    assert_eq!(v["bar"], json!({"running": false, "shows_files": null, "diagnostics": null, "config_dir": null}));
}

#[test]
fn a_broken_file_nulls_only_what_it_feeds() {
    let dir = scratch("broken");
    let fake = backend();
    {
        let mut l = fake.loaded.borrow_mut();
        l.bar_error = Some(FileError { at: "bar.toml:7:3".into(), message: "missing closing quote".into() });
        l.sources = None;
    }
    let v = ok(serve_with(&request(Verb::Get), &cli(&dir), &fake));
    assert_eq!((v["preset"].clone(), v["bars"].clone()), (Value::Null, Value::Null));
    assert_eq!(v["files"]["bar.toml"]["error"], json!({"at": "bar.toml:7:3", "message": "missing closing quote"}));
    assert!(v["colors"].is_object(), "style.css still reads");
    assert!(v["files"]["style.css"]["error"].is_null());
    {
        let mut l = fake.loaded.borrow_mut();
        l.bar_error = None;
        l.sources = Some(sources());
        l.style_error = Some(FileError { at: "style.css:3:1".into(), message: "stray }".into() });
        l.accent = None;
    }
    let v = ok(serve_with(&request(Verb::Get), &cli(&dir), &fake));
    assert!(v["colors"].is_null() && v["bars"].is_array() && v["preset"].is_object());
    assert_eq!(v["files"]["style.css"]["error"]["at"], "style.css:3:1");
}

#[test]
fn bar_name_before_index() {
    let dir = scratch("barname");
    let fake = backend();
    {
        let mut l = fake.loaded.borrow_mut();
        let s = l.sources.as_mut().unwrap();
        let mut second = s.bars[0].clone();
        s.bars[0].name.value = "1".into();
        second.name.value = "0".into();
        s.bars.push(second);
    }
    let get = |bar: &str| ok(serve_with(&Request { bar: Some(bar.into()), ..request(Verb::Get) }, &cli(&dir), &fake));
    // `0` is a bar name here, so it is not "the first bar".
    let v = get("0");
    assert_eq!(v["bars"].as_array().unwrap().len(), 1);
    assert_eq!((v["bars"][0]["name"].clone(), v["bars"][0]["index"].clone()), (json!("0"), json!(1)));
    let v = get("1");
    assert_eq!((v["bars"][0]["name"].clone(), v["bars"][0]["index"].clone()), (json!("1"), json!(0)));
    // With no bar of that name the number is an index.
    {
        let mut l = fake.loaded.borrow_mut();
        let s = l.sources.as_mut().unwrap();
        s.bars[0].name.value = "main".into();
        s.bars[1].name.value = "side".into();
    }
    let v = get("1");
    assert_eq!((v["bars"][0]["name"].clone(), v["bars"][0]["index"].clone()), (json!("side"), json!(1)));
    let e = err(serve_with(&Request { bar: Some("top".into()), ..request(Verb::Get) }, &cli(&dir), &fake));
    assert_eq!(e.code, ErrorCode::UnknownBar);
    assert_eq!(e.message, "no bar called 'top' (bars: main, side)");
    assert_eq!(ok(serve_with(&request(Verb::Get), &cli(&dir), &fake))["bars"].as_array().unwrap().len(), 2);
}

#[test]
fn the_bar_state_in_get_follows_what_the_bar_shows() {
    let dir = scratch("barstate");
    let fake = backend();
    let state = |s: BarState| {
        fake.bar.borrow_mut().clear();
        fake.bar.borrow_mut().push_back(s);
        ok(serve_with(&request(Verb::Get), &cli(&dir), &fake))["bar"].clone()
    };
    let here = canonical(&dir);
    assert_eq!(
        state(BarState::Running(applied_reply(&here, FileHash::Fnv(9), CSS_HASH)))["shows_files"],
        false,
        "an older bar.toml"
    );
    assert_eq!(state(BarState::Running(applied_reply(&here, BAR_HASH, CSS_HASH)))["shows_files"], true);
    assert_eq!(
        state(BarState::Running(applied_reply("C:\\elsewhere", BAR_HASH, CSS_HASH)))["shows_files"],
        Value::Null
    );
    let v = state(BarState::Unreachable);
    assert_eq!((v["running"].clone(), v["shows_files"].clone()), (json!(true), json!(false)));
    let upper = here.to_ascii_uppercase().replace('\\', "/");
    assert_eq!(
        state(BarState::Running(applied_reply(&upper, BAR_HASH, CSS_HASH)))["shows_files"],
        true,
        "case and separators do not matter"
    );
}

// ---------------------------------------------------------------- the other reads

#[test]
fn presets_schema_and_log_are_served_from_their_sources() {
    let dir = scratch("reads");
    let fake = backend();
    let v = ok(serve_with(&request(Verb::Presets), &cli(&dir), &fake));
    assert_eq!(
        (v["kind"].clone(), v["current"].clone(), v["high_contrast"].clone()),
        (json!("presets"), json!("fluent"), json!(false))
    );
    let p = &v["presets"][0];
    assert_eq!(p["schemes"], json!(["dark", "light"]));
    assert_eq!(p["bar"], json!({"position": "top", "height": 36, "floating": false}));
    assert_eq!(
        p["tune"][0],
        json!({"var": "accent", "label": "Accent", "editable": true, "dark": "#81cbea", "light": "#81cbea"})
    );
    assert_eq!(p["contrast"]["dark"]["text"], json!({"lowest": 4.67, "what": "placeholder on input", "need": 4.5}));
    assert_eq!(p["contrast"]["light"]["pass"], true);
    fake.loaded.borrow_mut().sources = None;
    assert_eq!(ok(serve_with(&request(Verb::Presets), &cli(&dir), &fake))["current"], Value::Null);

    assert_eq!(ok(serve_with(&request(Verb::Schema), &cli(&dir), &fake))["kind"], "schema");

    fake.log_entries.borrow_mut().extend([
        LogEntry {
            n: 18,
            time: "2026-11-27T10:15:00+11:00".into(),
            what: "style.css: accent none -> #e5a50a".into(),
            changes: vec![Op::Set { key: "color.accent".into(), value: json!("#e5a50a") }],
            files: vec![(ConfigFile::StyleCss, FileHash::Missing, FileHash::Fnv(0x91aa04c3e2b8f610))],
        },
        LogEntry {
            n: 17,
            time: "2026-11-27T10:10:00+11:00".into(),
            what: "undo".into(),
            changes: vec![Op::Undo { n: 16 }],
            files: Vec::new(),
        },
    ]);
    let v = ok(serve_with(&request(Verb::Log), &cli(&dir), &fake));
    assert_eq!(v["entries"].as_array().unwrap().len(), 2);
    assert_eq!(v["entries"][0]["n"], 18, "newest first");
    assert_eq!(v["entries"][0]["files"], json!({"style.css": {"before": "none", "after": "fnv1a64:91aa04c3e2b8f610"}}));
    assert_eq!(v["entries"][0]["changes"][0], json!({"op": "set", "key": "color.accent", "value": "#e5a50a"}));
    assert_eq!(v["entries"][1]["changes"][0], json!({"op": "undo", "n": 16}));
    let v = ok(serve_with(&Request { last: Some(1), ..request(Verb::Log) }, &cli(&dir), &fake));
    assert_eq!(v["entries"].as_array().unwrap().len(), 1);
}

#[test]
fn widgets_list_where_each_is_used_and_only_the_command_line_answers() {
    let dir = scratch("widgets");
    let fake = backend();
    *fake.listing.borrow_mut() = Listing {
        widgets: vec![
            WidgetView {
                name: "clock".into(),
                builtin: true,
                replaces_builtin: false,
                about: json!({"title": "Clock", "visible": true, "needs_options": false, "opens": [], "summary": "Time of day.", "options": [{"key": "format", "type": "string", "default": "HH:mm"}]}),
            },
            WidgetView {
                name: "my/weather".into(),
                builtin: false,
                replaces_builtin: false,
                about: json!({"title": "Weather", "visible": true, "needs_options": false, "opens": [], "summary": "", "options": []}),
            },
        ],
        failed: vec![WidgetFailure { name: "bad".into(), source: "widgets/bad.luau".into(), message: "boom".into() }],
    };
    let v = ok(serve_with(&request(Verb::Widgets), &cli(&dir), &fake));
    let clock = &v["widgets"][0];
    assert_eq!((clock["name"].clone(), clock["source"].clone()), (json!("clock"), json!("builtin")));
    assert!(clock["used"].as_array().unwrap().iter().all(|u| u["ref"].is_string() && u["bar"] == "main"));
    assert!(!clock["used"].as_array().unwrap().is_empty(), "the default bar places a clock");
    assert_eq!(v["widgets"][1]["source"], "user");
    assert_eq!(v["widgets"][1]["used"], json!([]));
    assert_eq!(v["failed"][0], json!({"name": "bad", "source": "widgets/bad.luau", "message": "boom"}));
    let e = err(serve_with(&request(Verb::Widgets), &pipe(&dir, true), &fake));
    assert_eq!(e.code, ErrorCode::CliOnly);
    assert_eq!(e.exit_code(), 2);
}

// ---------------------------------------------------------------- change

#[test]
fn a_change_is_planned_written_logged_and_reported() {
    let dir = scratch("change");
    let fake = backend();
    fake.bar.borrow_mut().push_back(BarState::Running(applied_reply(&canonical(&dir), NEW_BAR_HASH, CSS_HASH)));
    let mut req = change_request();
    req.expect.set(ConfigFile::BarToml, BAR_HASH);
    let v = ok(serve_with(&req, &cli(&dir), &fake));
    assert_eq!(v["kind"], "change");
    assert_eq!(v["dry_run"], false);
    assert_eq!(
        v["files"]["bar.toml"],
        json!({"hash": NEW_BAR_HASH.to_string(), "written": true, "edited_by_hand": false})
    );
    assert_eq!(
        v["files"]["style.css"],
        json!({"hash": CSS_HASH.to_string(), "written": false, "edited_by_hand": true})
    );
    assert_eq!(v["undo"], 17);
    assert_eq!(v["notices"][0]["code"], "hand_edit_kept");
    assert_eq!(v["bar"]["shows_files"], true);
    assert_eq!(v["bar"]["running"], true);
    assert_eq!(v["bar"]["ms"], 0);
    assert_eq!(*fake.log.borrow(), ["bar.toml: preset default -> glass (undo 17)"]);
    let opts = fake.opts.borrow();
    assert_eq!(opts.len(), 1);
    assert!(opts[0].wait_lock && !opts[0].dry_run, "the command line waits for the lock");
    assert_eq!(opts[0].expect.get(ConfigFile::BarToml), Some(BAR_HASH));
    assert!(fake.sleeps.borrow().is_empty());
}

#[test]
fn a_dry_run_writes_nothing_logs_nothing_and_asks_nobody() {
    let dir = scratch("dry");
    let fake = backend();
    let mut plan = plan_of("preset default -> glass");
    plan.preset_switch = Some(preset_switch());
    *fake.plan.borrow_mut() = Some(Ok(plan));
    let v = ok(serve_with(&Request { dry_run: true, ..change_request() }, &cli(&dir), &fake));
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["files"]["bar.toml"]["written"], false);
    assert_eq!(v["undo"], Value::Null);
    assert_eq!(v["preset_switch"]["kept"][0]["key"], "floating");
    assert_eq!(v["preset_switch"]["accent_kept"], "#e5a50a");
    assert_eq!(
        v["bar"],
        json!({"running": null, "shows_files": null, "ms": null, "diagnostics": null, "config_dir": null})
    );
    assert!(fake.log.borrow().is_empty(), "no log line for a dry run");
    assert_eq!(fake.called("bar"), 0);
    assert!(fake.opts.borrow()[0].dry_run);
}

#[test]
fn no_wait_asks_nobody_either() {
    let dir = scratch("nowait");
    let fake = backend();
    let v = ok(serve_with(&change_request(), &Ctx::cli(dir.clone(), dir.join("logs"), true), &fake));
    assert_eq!(v["bar"]["running"], Value::Null);
    assert_eq!(fake.called("bar"), 0);
    assert_eq!(fake.log.borrow().len(), 1, "it was still written and logged");
}

#[test]
fn the_command_line_looks_at_5_ms_then_every_25_until_the_bar_shows_the_files() {
    let dir = scratch("wait");
    let fake = backend();
    let here = canonical(&dir);
    for _ in 0..3 {
        fake.bar.borrow_mut().push_back(BarState::Running(applied_reply(&here, BAR_HASH, CSS_HASH)));
    }
    fake.bar.borrow_mut().push_back(BarState::Running(applied_reply(&here, NEW_BAR_HASH, CSS_HASH)));
    let v = ok(serve_with(&change_request(), &cli(&dir), &fake));
    assert_eq!(*fake.sleeps.borrow(), [5, 25, 25]);
    assert_eq!(v["bar"]["shows_files"], true);
    assert_eq!(v["bar"]["ms"], 55, "the time the bar was first seen to show the files, not a poll step");
    assert!(v["notices"].as_array().unwrap().iter().all(|n| n["code"] != "not_confirmed"));
}

fn restore_request() -> Request {
    Request { file: Some(ConfigFile::BarToml), ..request(Verb::Restore) }
}

fn shown(dir: &std::path::Path, loads: u64, diagnostics: u32) -> BarState {
    let mut a = applied_reply(&canonical(dir), NEW_BAR_HASH, CSS_HASH);
    a.loads = loads;
    a.diagnostics = diagnostics;
    BarState::Running(a)
}

#[test]
fn a_write_of_what_the_bar_already_shows_waits_for_the_bar_to_reload_before_reporting() {
    let dir = scratch("restore-wait");
    let fake = backend();
    fake.loads_before.set(Some(4));
    // The versions match from the first ask, but the bar has not swapped a config in since the write.
    fake.bar.borrow_mut().push_back(shown(&dir, 4, 2));
    fake.bar.borrow_mut().push_back(shown(&dir, 4, 2));
    fake.bar.borrow_mut().push_back(shown(&dir, 5, 0));
    let v = ok(serve_with(&restore_request(), &cli(&dir), &fake));
    assert_eq!(*fake.sleeps.borrow(), [5, 25]);
    assert_eq!(v["bar"]["shows_files"], true);
    assert_eq!(v["bar"]["diagnostics"], 0, "the count after the reload, not the one before it");
}

#[test]
fn a_bar_that_never_reloads_what_it_already_shows_is_reported_as_it_stands_without_a_notice() {
    let dir = scratch("restore-cap");
    let fake = backend();
    fake.loads_before.set(Some(4));
    fake.bar.borrow_mut().push_back(shown(&dir, 4, 2));
    let v = ok(serve_with(&restore_request(), &cli(&dir), &fake));
    assert_eq!(fake.clock.get(), 2000, "it waited the whole cap");
    assert_eq!(v["bar"]["shows_files"], true);
    assert!(v["notices"].as_array().unwrap().iter().all(|n| n["code"] != "not_confirmed"), "{v}");
}

#[test]
fn a_bar_that_never_shows_the_files_gets_not_confirmed_after_2000_ms_and_exit_zero() {
    let dir = scratch("timeout");
    let fake = backend();
    fake.bar.borrow_mut().push_back(BarState::Running(applied_reply(&canonical(&dir), BAR_HASH, CSS_HASH)));
    let reply = serve_with(&change_request(), &cli(&dir), &fake);
    assert_eq!(reply.exit_code(), 0, "the change is saved");
    let v = ok(reply);
    let sleeps = fake.sleeps.borrow().clone();
    assert_eq!((sleeps.len(), sleeps[0], sleeps[1], *sleeps.last().unwrap()), (81, 5, 25, 20));
    assert_eq!(sleeps.iter().sum::<u64>(), 2000, "never past the cap");
    assert_eq!(fake.clock.get(), 2000);
    assert_eq!(v["bar"]["shows_files"], false);
    assert_eq!(v["bar"]["ms"], Value::Null);
    let n =
        v["notices"].as_array().unwrap().iter().find(|n| n["code"] == "not_confirmed").expect("a not_confirmed notice");
    assert!(n["next"].as_str().unwrap().contains("rsb status"));
}

#[test]
fn a_bar_in_another_folder_is_not_waited_for() {
    let dir = scratch("other");
    let fake = backend();
    fake.bar.borrow_mut().push_back(BarState::Running(applied_reply("D:\\other\\rsb", BAR_HASH, CSS_HASH)));
    let v = ok(serve_with(&change_request(), &cli(&dir), &fake));
    assert!(fake.sleeps.borrow().is_empty(), "no wait");
    assert_eq!((v["bar"]["shows_files"].clone(), v["bar"]["ms"].clone()), (Value::Null, Value::Null));
    let n = v["notices"].as_array().unwrap().iter().find(|n| n["code"] == "bar_uses_other_folder").unwrap();
    assert_eq!(n["bar_dir"], "D:\\other\\rsb");
    assert!(n["next"].as_str().unwrap().contains("D:\\other\\rsb"));
}

#[test]
fn no_bar_is_not_an_error_and_an_unreachable_bar_is_a_notice() {
    let dir = scratch("nobar");
    let fake = backend();
    let reply = serve_with(&change_request(), &cli(&dir), &fake);
    assert_eq!(reply.exit_code(), 0);
    let v = ok(reply);
    assert_eq!(
        v["bar"],
        json!({"running": false, "shows_files": null, "ms": null, "diagnostics": null, "config_dir": null})
    );
    assert_eq!(v["notices"].as_array().unwrap().len(), 1, "only the writer's own notice");
    fake.bar.borrow_mut().push_back(BarState::Unreachable);
    let v = ok(serve_with(&change_request(), &cli(&dir), &fake));
    assert!(v["notices"].as_array().unwrap().iter().any(|n| n["code"] == "bar_unreachable"));
    assert_eq!((v["bar"]["running"].clone(), v["bar"]["shows_files"].clone()), (json!(true), json!(false)));
}

#[test]
fn notices_from_the_edit_engine_come_through_with_their_fields() {
    let dir = scratch("notices");
    let fake = backend();
    let mut plan = plan_of("bar main: right [..] -> [..]");
    plan.notices.push(
        Notice::new(NoticeCode::PopupUnreachable, "the power menu is no longer reachable")
            .with_next("add power back to a zone to reach the power menu again")
            .with_extra("popup", json!("power"))
            .with_extra("widget", json!("power")),
    );
    plan.notices.push(
        Notice::new(NoticeCode::NowYours, "centre is now yours")
            .with_extra("zones", json!([{"bar": "main", "zone": "center"}])),
    );
    *fake.plan.borrow_mut() = Some(Ok(plan));
    let v = ok(serve_with(&change_request(), &cli(&dir), &fake));
    let notices = v["notices"].as_array().unwrap();
    assert_eq!(notices[0]["code"], "popup_unreachable");
    assert_eq!((notices[0]["popup"].clone(), notices[0]["widget"].clone()), (json!("power"), json!("power")));
    assert_eq!(notices[1]["zones"], json!([{"bar": "main", "zone": "center"}]));
    assert_eq!(notices[2]["code"], "hand_edit_kept", "the writer's notices follow the engine's");
    assert!(notices.iter().all(|n| n.get("next").is_some()), "every notice has a next, null when nothing is to do");
}

#[test]
fn a_refusal_is_logged_unless_the_request_itself_was_wrong() {
    let dir = scratch("refuse");
    let fake = backend();
    *fake.applied.borrow_mut() =
        Some(Err(SettingsError::new(ErrorCode::FileChanged, "bar.toml changed since it was read")
            .with_file(ConfigFile::BarToml)));
    let e = err(serve_with(&change_request(), &cli(&dir), &fake));
    assert_eq!((e.code, e.exit_code()), (ErrorCode::FileChanged, 1));
    assert_eq!(*fake.log.borrow(), ["bar.toml: refused file_changed"]);
    *fake.applied.borrow_mut() =
        Some(Err(SettingsError::new(ErrorCode::Busy, "another settings change is being written")));
    err(serve_with(&change_request(), &cli(&dir), &fake));
    assert_eq!(fake.log.borrow()[1], "settings: refused busy");
    *fake.plan.borrow_mut() =
        Some(Err(SettingsError::new(ErrorCode::BadValue, "'bar.main.height' must be 16 to 128, got 200")));
    let e = err(serve_with(&change_request(), &cli(&dir), &fake));
    assert_eq!(e.exit_code(), 2);
    assert_eq!(fake.log.borrow().len(), 2, "a bad value leaves no line");
    assert_eq!(fake.called("apply"), 2);
    *fake.applied.borrow_mut() = Some(Err(SettingsError::new(ErrorCode::Busy, "x")));
    *fake.plan.borrow_mut() = Some(Ok(plan_of("x")));
    err(serve_with(&Request { dry_run: true, ..change_request() }, &cli(&dir), &fake));
    assert_eq!(fake.log.borrow().len(), 2, "a refused dry run leaves no line either");
}

#[test]
fn a_file_that_is_not_utf8_refuses_only_when_the_change_writes_it() {
    let dir = scratch("utf8");
    let fake = backend();
    fake.loaded.borrow_mut().style_css.utf8 = false;
    ok(serve_with(&change_request(), &cli(&dir), &fake));
    let mut plan = plan_of("x");
    plan.files[0].file = ConfigFile::StyleCss;
    *fake.plan.borrow_mut() = Some(Ok(plan));
    let e = err(serve_with(&change_request(), &cli(&dir), &fake));
    assert_eq!((e.code, e.file), (ErrorCode::NotUtf8, Some(ConfigFile::StyleCss)));
    assert_eq!(fake.called("apply"), 1, "nothing was written the second time");
    fake.loaded.borrow_mut().bar_toml.unreadable = Some("the file is larger than 1024 KiB".into());
    let e = err(serve_with(&change_request(), &cli(&dir), &fake));
    assert_eq!(e.code, ErrorCode::WriteFailed);
}

#[test]
fn a_change_holding_undo_or_restore_or_nothing_is_bad_usage() {
    let dir = scratch("badchange");
    let fake = backend();
    for changes in [vec![], vec![Op::Undo { n: 1 }], vec![Op::Restore { file: ConfigFile::BarToml }]] {
        let e = err(serve_with(&Request { changes, ..request(Verb::Change) }, &cli(&dir), &fake));
        assert_eq!(e.code, ErrorCode::BadUsage);
    }
    assert_eq!(fake.called("plan"), 0);
}

#[test]
fn a_write_goes_only_to_a_config_folder() {
    let fake = backend();
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("test-tmp")
        .join(format!("settings-folder-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let other = base.join("Documents");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("notes.txt"), "x").unwrap();
    let e = err(serve_with(&change_request(), &cli(&other), &fake));
    assert_eq!(e.code, ErrorCode::BadUsage);
    assert!(e.message.ends_with("is not an RSB config folder"), "{}", e.message);
    assert_eq!(e.next.as_deref(), Some("pass the folder that holds bar.toml, or leave out --config-dir"));
    assert_eq!(fake.called("plan"), 0);
    // A folder that does not exist yet is made by the first write.
    ok(serve_with(&change_request(), &cli(&base.join("new")), &fake));
    // One that holds either file is a config folder.
    std::fs::write(other.join("style.css"), "").unwrap();
    ok(serve_with(&change_request(), &cli(&other), &fake));
    // A file is not a folder.
    let e = err(serve_with(&change_request(), &cli(&other.join("notes.txt")), &fake));
    assert_eq!(e.code, ErrorCode::BadUsage);
    // Reads work in any folder.
    ok(serve_with(&request(Verb::Get), &cli(&base.join("empty-and-missing")), &fake));
}

// ---------------------------------------------------------------- undo and restore

#[test]
fn undo_and_restore_use_the_same_result_and_log_what_they_did() {
    let dir = scratch("undo");
    let fake = backend();
    *fake.applied.borrow_mut() = Some(Ok(Applied { undo: None, notices: Vec::new(), ..bar_written() }));
    fake.log_entries.borrow_mut().push(LogEntry {
        n: 17,
        time: "t".into(),
        what: "w".into(),
        changes: Vec::new(),
        files: Vec::new(),
    });
    let v = ok(serve_with(&request(Verb::Undo), &cli(&dir), &fake));
    assert_eq!(v["kind"], "change");
    assert_eq!(v["undo"], Value::Null, "an undo makes no entry of its own");
    assert_eq!(v["preset_switch"], Value::Null);
    assert_eq!(*fake.log.borrow(), ["bar.toml: undo 17 restored"]);
    ok(serve_with(&Request { n: Some(17), ..request(Verb::Undo) }, &cli(&dir), &fake));
    assert_eq!(fake.log.borrow()[1], "bar.toml: undo 17 restored");

    *fake.applied.borrow_mut() = Some(Ok(Applied {
        undo: Some(18),
        kept_copy: Some("C:\\x\\.undo\\18\\before.bar.toml".into()),
        notices: Vec::new(),
        ..bar_written()
    }));
    let v = ok(serve_with(&Request { file: Some(ConfigFile::BarToml), ..request(Verb::Restore) }, &cli(&dir), &fake));
    assert_eq!(v["undo"], 18);
    assert_eq!(v["kept_copy"], "C:\\x\\.undo\\18\\before.bar.toml");
    assert_eq!(fake.log.borrow()[2], "bar.toml: restored from the last good copy (undo 18)");
    let e = err(serve_with(&request(Verb::Restore), &cli(&dir), &fake));
    assert_eq!(e.code, ErrorCode::BadUsage);

    *fake.applied.borrow_mut() =
        Some(Err(SettingsError::new(ErrorCode::UndoNotLatest, "change 16 is not the newest (17 is)")));
    assert_eq!(err(serve_with(&Request { n: Some(16), ..request(Verb::Undo) }, &cli(&dir), &fake)).exit_code(), 1);
    assert_eq!(fake.log.borrow().last().unwrap(), "settings: refused undo_not_latest");
}

// ---------------------------------------------------------------- the pipe

#[test]
fn a_write_over_the_pipe_never_waits_for_the_lock_or_the_bar() {
    let dir = scratch("pipewrite");
    let fake = backend();
    fake.bar.borrow_mut().push_back(BarState::Running(applied_reply(&canonical(&dir), BAR_HASH, CSS_HASH)));
    let v = ok(serve_with(&change_request(), &pipe(&dir, true), &fake));
    assert!(!fake.opts.borrow()[0].wait_lock, "busy at once, never a wait");
    assert!(fake.sleeps.borrow().is_empty());
    assert_eq!(v["bar"]["ms"], Value::Null);
    // The bar's own record, as of the write: it is running, and the reply says which folder it reads.
    assert_eq!(v["bar"]["running"], true);
    assert_eq!(v["bar"]["config_dir"], canonical(&dir));
    assert!(v["bar"]["diagnostics"].is_number() && v["bar"]["shows_files"].is_boolean(), "{}", v["bar"]);
    assert!(v["notices"].as_array().unwrap().iter().all(|n| n["code"] != "not_confirmed"));
    assert_eq!(fake.log.borrow().len(), 0, "the bar logs its own line through the logger");
}

#[test]
fn a_write_over_the_pipe_needs_a_client_at_the_bars_level_but_a_read_does_not() {
    let dir = scratch("pipeclient");
    let fake = backend();
    for verb in [Verb::Change, Verb::Undo, Verb::Restore] {
        let req = Request {
            changes: if verb == Verb::Change { change_request().changes } else { Vec::new() },
            file: Some(ConfigFile::BarToml),
            ..request(verb)
        };
        let e = err(serve_with(&req, &pipe(&dir, false), &fake));
        assert_eq!((e.code, e.exit_code()), (ErrorCode::CliOnly, 2), "{verb:?}");
    }
    assert_eq!(
        fake.called("plan") + fake.called("apply") + fake.called("undo") + fake.called("restore"),
        0,
        "nothing was touched"
    );
    for verb in [Verb::Get, Verb::Presets, Verb::Schema, Verb::Log] {
        ok(serve_with(&request(verb), &pipe(&dir, false), &fake));
    }
}

#[test]
fn the_same_get_over_the_pipe_and_the_command_line_gives_the_same_answer() {
    let dir = scratch("same");
    let fake = backend();
    let here = canonical(&dir);
    fake.bar.borrow_mut().push_back(BarState::Running(applied_reply(&here, BAR_HASH, CSS_HASH)));
    let a = ok(serve_with(&request(Verb::Get), &cli(&dir), &fake));
    let b = ok(serve_with(&request(Verb::Get), &pipe(&dir, true), &fake));
    assert_eq!(a, b);
}

#[test]
fn widgets_and_preview_lines_over_the_pipe_are_cli_only() {
    let dir = scratch("pipecli");
    for line in ["{\"verb\":\"widgets\"}", "{\"verb\":\"preview\",\"presets\":[\"glass\"]}"] {
        let e = err(serve_line(line, &pipe(&dir, true)));
        assert_eq!((e.code, e.exit_code()), (ErrorCode::CliOnly, 2), "{line}");
        assert!(e.message.contains("not over the pipe"), "{}", e.message);
    }
    // From a command line the same word is just an unknown verb of a request.
    assert_eq!(err(serve_line("{\"verb\":\"preview\"}", &cli(&dir))).code, ErrorCode::BadJson);
}

#[test]
fn a_log_too_long_for_the_pipe_is_refused_with_a_code_and_the_command_line_gets_it_whole() {
    let dir = scratch("biglog");
    let fake = backend();
    for n in 1..=20u32 {
        fake.log_entries.borrow_mut().push(LogEntry {
            n,
            time: "t".into(),
            what: "w".repeat(4000),
            changes: Vec::new(),
            files: Vec::new(),
        });
    }
    let line = "{\"verb\":\"log\"}";
    let whole = serve_with(&Request::from_json(line).unwrap(), &cli(&dir), &fake);
    assert!(whole.to_line().len() > rsb_core::ipc::MAX_REPLY_BYTES - 4, "the fixture must be over the limit");
    assert_eq!(ok(line_with(line, &cli(&dir), &fake))["entries"].as_array().unwrap().len(), 20);
    let e = err(line_with(line, &pipe(&dir, true), &fake));
    assert_eq!((e.code, e.exit_code()), (ErrorCode::CliOnly, 2));
    assert!(e.message.contains("too long for the pipe"), "{}", e.message);
    let few = "{\"verb\":\"log\",\"last\":2}";
    assert_eq!(ok(line_with(few, &pipe(&dir, true), &fake))["entries"].as_array().unwrap().len(), 2);
}

#[test]
fn a_settings_line_through_a_real_pipe_is_answered_by_serve_line() {
    use rsb_core::ipc::{Command, Reply as Wire};
    use std::sync::Arc;
    use std::time::Duration;
    let dir = scratch("realpipe");
    let state = Arc::new(crate::msg::AppliedState::new(canonical(&dir), canonical(&dir)));
    let (config, log) = (dir.clone(), dir.join("logs"));
    let handler: crate::pipe::Handler = Arc::new(move |command| match command {
        Command::Settings(text) => {
            Wire::Ok(serve_line(&text, &Ctx::pipe(config.clone(), log.clone(), state.clone())).to_line())
        }
        Command::Applied => Wire::Ok("{}".into()),
        _ => Wire::Err("not here".into()),
    });
    let name = format!(r"\\.\pipe\RSB-test-settings-{}", std::process::id());
    let server = crate::pipe::PipeServer::start_named(&name, handler).unwrap();
    let ask = |line: &str| match crate::pipe::send_to(&name, &Command::Settings(line.into()), Duration::from_secs(5)) {
        Ok(Wire::Ok(text)) => Reply::from_line(&text).unwrap(),
        other => panic!("{other:?}"),
    };
    let schema = ask("{\"verb\":\"schema\",\"contract\":1}");
    assert_eq!(schema.to_json()["kind"], "schema");
    assert_eq!(err(ask("{\"verb\":\"widgets\"}")).code, ErrorCode::CliOnly);
    assert_eq!(err(ask("{\"verb\":\"preview\"}")).code, ErrorCode::CliOnly);
    assert_eq!(err(ask("{\"verb\":\"schema\",\"contract\":2}")).code, ErrorCode::ContractTooNew);
    assert_eq!(err(ask("{nope")).code, ErrorCode::BadJson);
    server.stop();
}

#[test]
fn a_line_that_is_not_a_request_is_bad_json() {
    let dir = scratch("line");
    let e = err(serve_line("{nope", &cli(&dir)));
    assert_eq!((e.code, e.exit_code()), (ErrorCode::BadJson, 2));
    assert_eq!(err(serve_line("[1]", &cli(&dir))).code, ErrorCode::BadJson);
    assert_eq!(err(serve_line("{}", &cli(&dir))).code, ErrorCode::BadUsage, "no verb and no changes");
    // A request with changes may leave out the verb.
    let e = err(serve_line("{\"changes\":[{\"op\":\"undo\",\"n\":1}]}", &cli(&dir)));
    assert_eq!(e.code, ErrorCode::BadUsage);
    assert!(e.message.contains("undo and restore"));
}

// ---------------------------------------------------------------- contract and exit codes

#[test]
fn contract_version_window() {
    let dir = scratch("contract");
    let fake = backend();
    let with = |c: Option<u32>| serve_with(&Request { contract: c, ..request(Verb::Schema) }, &cli(&dir), &fake);
    assert_eq!(ok(with(None))["contract"], CONTRACT);
    assert_eq!(ok(with(Some(rsb_core::contract::CONTRACT_MIN)))["contract"], CONTRACT);
    assert_eq!(ok(with(Some(CONTRACT)))["contract"], CONTRACT);
    let e = err(with(Some(CONTRACT + 1)));
    assert_eq!((e.code, e.exit_code()), (ErrorCode::ContractTooNew, 2));
    assert_eq!(e.message, format!("this rsb speaks settings contract {CONTRACT}; the caller wants {}", CONTRACT + 1));
    if rsb_core::contract::CONTRACT_MIN > 0 {
        let e = err(with(Some(rsb_core::contract::CONTRACT_MIN - 1)));
        assert_eq!((e.code, e.exit_code()), (ErrorCode::ContractTooOld, 2));
    }
    // Every reply says which version it speaks, a refusal too.
    for reply in [with(None), with(Some(CONTRACT + 1))] {
        assert_eq!(reply.to_json()["contract"], CONTRACT);
    }
    // A refusal is checked before anything is read.
    assert_eq!(fake.called("load"), 0);
}

#[test]
fn settings_exit_codes() {
    // Every code a refusal can carry is 1 or 2, never 3, 4 or 5.
    for &code in ErrorCode::ALL {
        let reply = Reply::Err(SettingsError::new(code, "x"));
        assert!(matches!(reply.exit_code(), 1 | 2), "{code}");
        assert_eq!(reply.to_json()["error"]["code"], code.as_str());
    }
    // And across the verbs: a served reply is 0, a refusal 1 or 2.
    let dir = scratch("exit");
    let fake = backend();
    let mut codes = Vec::new();
    for verb in
        [Verb::Get, Verb::Presets, Verb::Widgets, Verb::Schema, Verb::Log, Verb::Change, Verb::Undo, Verb::Restore]
    {
        for ctx in [cli(&dir), pipe(&dir, false)] {
            let req = Request {
                changes: if verb == Verb::Change { change_request().changes } else { Vec::new() },
                ..request(verb)
            };
            codes.push(serve_with(&req, &ctx, &fake).exit_code());
        }
    }
    assert!(codes.iter().all(|c| matches!(c, 0..=2)), "{codes:?}");
    assert!(codes.contains(&0) && codes.contains(&2));
    // A bar that is not running is not an error here.
    assert_eq!(serve_with(&change_request(), &cli(&dir), &fake).exit_code(), 0);
}

#[test]
fn a_fault_inside_is_internal_so_the_pipe_thread_survives() {
    struct Panics;
    impl Backend for Panics {
        fn load(&self, _: &std::path::Path, _: bool) -> Loaded {
            panic!("a bug");
        }
        fn plan(
            &self,
            _: &std::path::Path,
            _: &rsb_core::cfgedit::FileTexts,
            _: &[Op],
            _: bool,
            _: bool,
        ) -> Result<Plan, SettingsError> {
            unreachable!()
        }
        fn apply(
            &self,
            _: &std::path::Path,
            _: &Plan,
            _: &crate::cfgwrite::WriteOpts,
        ) -> Result<Applied, SettingsError> {
            unreachable!()
        }
        fn undo(
            &self,
            _: &std::path::Path,
            _: Option<u32>,
            _: &crate::cfgwrite::WriteOpts,
        ) -> Result<Applied, SettingsError> {
            unreachable!()
        }
        fn restore(
            &self,
            _: &std::path::Path,
            _: ConfigFile,
            _: &crate::cfgwrite::WriteOpts,
        ) -> Result<Applied, SettingsError> {
            unreachable!()
        }
        fn log_entries(&self, _: &std::path::Path, _: usize) -> Result<Vec<LogEntry>, SettingsError> {
            unreachable!()
        }
        fn written(&self, _: &std::path::Path) -> Written {
            unreachable!()
        }
        fn has_last_good(&self, _: &std::path::Path, _: ConfigFile) -> bool {
            unreachable!()
        }
        fn append_log(&self, _: &std::path::Path, _: &str) {}
        fn presets(&self, _: &std::path::Path) -> Result<Vec<PresetView>, SettingsError> {
            unreachable!()
        }
        fn widgets(&self, _: &std::path::Path) -> Listing {
            unreachable!()
        }
        fn previews(
            &self,
            _: &[String],
            _: &crate::preview::PreviewOpts,
        ) -> Result<Vec<crate::preview::Preview>, SettingsError> {
            unreachable!()
        }
        fn bar(&self, _: &Ctx) -> BarState {
            unreachable!()
        }
        fn high_contrast(&self) -> bool {
            false
        }
        fn now_ms(&self) -> u64 {
            0
        }
        fn sleep_ms(&self, _: u64) {}
    }
    let dir = scratch("panic");
    let e = err(guarded(&request(Verb::Get), &cli(&dir), &Panics));
    assert_eq!((e.code, e.exit_code()), (ErrorCode::Internal, 1));
}

// ---------------------------------------------------------------- preview

fn preview_of(name: &str, theme: rsb_core::preset::Scheme, cached: bool) -> crate::preview::Preview {
    crate::preview::Preview {
        name: name.into(),
        theme,
        mine: false,
        zones_from_preset: false,
        path: PathBuf::from(format!("C:\\cache\\{name}-{}-1920x100-abc.png", theme.name())),
        width: 1920,
        height: 56,
        scale: 1.0,
        cached,
    }
}

#[test]
fn preview_names_each_picture_and_refuses_an_unknown_preset() {
    use super::grammar::{Call, parse};
    let dir = scratch("preview");
    let fake = backend();
    fake.previews.borrow_mut().extend([
        preview_of("glass", rsb_core::preset::Scheme::Dark, false),
        preview_of("glass", rsb_core::preset::Scheme::Light, true),
    ]);
    let words = |l: &str| l.split(' ').map(str::to_string).collect::<Vec<_>>();
    let Call::Preview(args) = parse(&words("preview glass")).call else { panic!() };
    let v = ok(preview(&args, &cli(&dir), &fake));
    assert_eq!(v["kind"], "preview");
    assert_eq!(v["previews"].as_array().unwrap().len(), 2);
    assert_eq!(
        v["previews"][0],
        json!({"name": "glass", "theme": "dark", "mine": false, "zones": "mine", "path": "C:\\cache\\glass-dark-1920x100-abc.png", "width": 1920, "height": 56, "scale": 1.0, "cached": false})
    );
    assert_eq!(v["previews"][1]["cached"], true);
    let Call::Preview(args) = parse(&words("preview glass --theme light")).call else { panic!() };
    assert_eq!(ok(preview(&args, &cli(&dir), &fake))["previews"].as_array().unwrap().len(), 1);
    let Call::Preview(args) = parse(&words("preview glas")).call else { panic!() };
    let e = err(preview(&args, &cli(&dir), &fake));
    assert_eq!((e.code, e.message.as_str()), (ErrorCode::UnknownPreset, "no preset 'glas'; did you mean 'glass'?"));
    assert_eq!(e.next.as_deref(), Some("rsb settings presets lists them"));
    *fake.preview_error.borrow_mut() =
        Some(SettingsError::new(ErrorCode::RenderFailed, "the preview of glass could not be drawn: x"));
    let Call::Preview(args) = parse(&words("preview --all")).call else { panic!() };
    assert_eq!(err(preview(&args, &cli(&dir), &fake)).exit_code(), 1);
}

#[test]
fn a_kept_key_and_an_accent_travel_through_as_the_engine_made_them() {
    let dir = scratch("accent");
    let fake = backend();
    let mut plan = plan_of("accent none -> #e5a50a");
    plan.accent = Some(rsb_core::contract::AccentSet {
        value: "#e5a50a".into(),
        schemes: rsb_core::contract::AccentSchemes {
            dark: rsb_core::contract::AccentTokens {
                fg: "#000000".into(),
                fg_ratio: 10.6,
                hover: "#e8ae22".into(),
                press: "#eab73b".into(),
                text: "#e5a50a".into(),
                line: "#e5a50a".into(),
                moved: 0,
            },
            light: rsb_core::contract::AccentTokens {
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
    plan.preset_switch = Some(rsb_core::contract::PresetSwitch {
        kept: vec![KeptKey {
            bar: "main".into(),
            key: "center".into(),
            yours: json!(["clock"]),
            preset: json!(["clock"]),
            same: true,
        }],
        ..preset_switch()
    });
    *fake.plan.borrow_mut() = Some(Ok(plan));
    let v = ok(serve_with(&change_request(), &cli(&dir), &fake));
    assert_eq!(v["accent"]["schemes"]["light"]["text"], "#6e4f05");
    assert_eq!(v["accent"]["schemes"]["light"]["moved"], 52);
    assert_eq!(v["preset_switch"]["kept"][0]["same"], true);
    assert_eq!(v["preset_switch"]["from"], "default");
}

//! The writer on real files in scratch folders: hand edits, refusals, the
//! undo entries, crashes, and the lock.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use rsb_core::cfgedit::{FileTexts, Plan, PlanContext, plan_change_with};
use rsb_core::color::Color;
use rsb_core::contract::{
    AccentSchemes, AccentSet, AccentTokens, ConfigFile, ErrorCode, FileHash, NoticeCode, Op, SettingsError, Zone,
};
use rsb_core::contrast::AccentSurfaces;
use serde_json::json;

use super::*;
use crate::testutil::TempDir;

const BAR: &str = "# my bar\nconfig_version = 1\n\n[[bar]]\nname = \"main\"\nheight = 36   # mine\nleft = [\"workspaces\", \"media\"]\n";
const CSS: &str = "bar { color: red }\n";

fn fake_derive(color: Color, _: &AccentSurfaces) -> AccentSet {
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
        hover: value.clone(),
        press: value.clone(),
        text: value.clone(),
        line: value.clone(),
        moved: 0,
    };
    AccentSet { value: value.clone(), schemes: AccentSchemes { dark: tokens("#000000"), light: tokens("#111111") } }
}

fn plan_of(bar: Option<&str>, css: Option<&str>, ops: &[Op]) -> Result<Plan, SettingsError> {
    let texts = FileTexts { bar_toml: bar.map(str::to_string), style_css: css.map(str::to_string) };
    let preset = |n: &str| crate::assets::preset_toml(n).map(str::to_string);
    let exists = |_: &str| true;
    let opens = |_: &str| Vec::<String>::new();
    let template = |f: ConfigFile| match f {
        ConfigFile::BarToml => crate::assets::file("config/bar.toml").unwrap_or("").to_string(),
        ConfigFile::StyleCss => crate::assets::file("config/style.css").unwrap_or("").to_string(),
    };
    let surfaces = AccentSurfaces::default();
    let ctx = PlanContext {
        reset: false,
        preset_toml: &preset,
        widget_exists: &exists,
        widget_opens: &opens,
        template: &template,
        surfaces: &surfaces,
        style_chain: &|_| None,
    };
    plan_change_with(&texts, ops, &ctx, &fake_derive)
}

fn set(key: &str, value: serde_json::Value) -> Op {
    Op::Set { key: key.into(), value }
}

fn height(n: u32) -> Op {
    set("bar.main.height", json!(n))
}

/// A config folder holding both files.
fn folder(bar: &str, css: &str) -> TempDir {
    let dir = TempDir::new("cfgwrite");
    std::fs::write(dir.path().join("bar.toml"), bar).unwrap();
    std::fs::write(dir.path().join("style.css"), css).unwrap();
    dir
}

fn read(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(name)).unwrap()
}

fn opts() -> WriteOpts {
    WriteOpts::default()
}

/// Every file under `dir` with its version, for "nothing changed".
fn snapshot(dir: &Path) -> BTreeMap<String, FileHash> {
    fn walk(root: &Path, at: &Path, out: &mut BTreeMap<String, FileHash>) {
        let Ok(read) = std::fs::read_dir(at) else { return };
        for e in read.flatten() {
            let p = e.path();
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            if p.is_dir() {
                out.insert(format!("{rel}/"), FileHash::Missing);
                walk(root, &p, out);
            } else {
                // A file held without sharing (the lock) reads as missing.
                out.insert(rel, FileHash::of_file(std::fs::read(&p).ok().as_deref()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn no_temps(dir: &Path) {
    for folder in [dir.to_path_buf(), dir.join(".undo")] {
        if let Ok(read) = std::fs::read_dir(&folder) {
            for e in read.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                assert!(!(name.ends_with(".tmp") && name.contains(".rsb-")), "a temporary file is left: {name}");
            }
        }
    }
}

fn apply_ok(dir: &Path, plan: &Plan) -> Applied {
    apply(dir, plan, &opts()).unwrap_or_else(|e| panic!("{e:?}"))
}

fn hash_of(dir: &Path, name: &str) -> FileHash {
    FileHash::of(&std::fs::read(dir.join(name)).unwrap())
}

// ---------------------------------------------------------------- the plain write

#[test]
fn a_change_is_written_with_an_undo_entry_and_the_versions_it_wrote() {
    let dir = folder(BAR, CSS);
    let plan = plan_of(Some(BAR), None, &[height(40)]).unwrap();
    let applied = apply_ok(dir.path(), &plan);
    assert_eq!(read(dir.path(), "bar.toml"), plan.files[0].after);
    assert_eq!(read(dir.path(), "style.css"), CSS);
    assert_eq!(applied.undo, Some(1));
    let bar = &applied.files[0];
    assert!(bar.written && !bar.edited_by_hand && bar.hash == hash_of(dir.path(), "bar.toml"));
    assert!(!applied.files[1].written && applied.files[1].hash == hash_of(dir.path(), "style.css"));
    let undo = dir.path().join(".undo");
    assert_eq!(std::fs::read_to_string(undo.join("1").join("before.bar.toml")).unwrap(), BAR);
    assert!(!undo.join("1").join("before.style.css.txt").exists(), "an unwritten file has no copy");
    assert!(!undo.join("lock").exists(), "the lock goes with the write");
    assert!(!undo.join("1.part").exists());
    assert_eq!(written(dir.path()).bar_toml, Some(bar.hash));
    let log = log_entries(dir.path(), 5).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!((log[0].n, log[0].what.as_str()), (1, "bar.toml: bar main: height 36 -> 40"));
    assert_eq!(log[0].files, vec![(ConfigFile::BarToml, FileHash::of(BAR.as_bytes()), bar.hash)]);
    assert_eq!(log[0].changes.len(), 1);
    no_temps(dir.path());
}

#[test]
fn a_byte_order_mark_and_the_line_endings_survive() {
    let text = BAR.replace('\n', "\r\n");
    let dir = TempDir::new("cfgwrite");
    let mut raw = vec![0xef, 0xbb, 0xbf];
    raw.extend_from_slice(text.as_bytes());
    std::fs::write(dir.path().join("bar.toml"), &raw).unwrap();
    let plan = plan_of(Some(&text), None, &[height(40), set("bar.main.position", json!("bottom"))]).unwrap();
    let applied = apply_ok(dir.path(), &plan);
    let now = std::fs::read(dir.path().join("bar.toml")).unwrap();
    assert!(now.starts_with(&[0xef, 0xbb, 0xbf]) && !now[3..].starts_with(&[0xef, 0xbb, 0xbf]));
    let body = String::from_utf8(now[3..].to_vec()).unwrap();
    assert_eq!(body, plan.files[0].after);
    assert_eq!(body.matches('\n').count(), body.matches("\r\n").count());
    assert_eq!(applied.files[0].hash, FileHash::of(&now), "the version is of the raw bytes, byte order mark included");
    let before = std::fs::read(dir.path().join(".undo").join("1").join("before.bar.toml")).unwrap();
    assert_eq!(before, raw);
}

// ---------------------------------------------------------------- hand edits

#[test]
fn expect_mismatch_refuses() {
    let dir = folder(BAR, CSS);
    let before = snapshot(dir.path());
    let plan = plan_of(Some(BAR), None, &[height(40)]).unwrap();
    let mut o = opts();
    o.expect.set(ConfigFile::BarToml, FileHash::of(b"something else"));
    let e = apply(dir.path(), &plan, &o).unwrap_err();
    assert_eq!((e.code, e.file), (ErrorCode::FileChanged, Some(ConfigFile::BarToml)));
    assert_eq!(
        snapshot(dir.path()).into_iter().filter(|(k, _)| !k.starts_with(".undo")).collect::<Vec<_>>(),
        before.into_iter().collect::<Vec<_>>()
    );
    no_temps(dir.path());
    assert!(log_entries(dir.path(), 5).unwrap().is_empty());
    // The right version goes through.
    o.expect.set(ConfigFile::BarToml, FileHash::of(BAR.as_bytes()));
    assert!(apply(dir.path(), &plan, &o).is_ok());
}

#[test]
fn expect_checked_only_for_written_files() {
    let dir = folder(BAR, CSS);
    let plan = plan_of(Some(BAR), None, &[height(40)]).unwrap();
    let mut o = opts();
    o.expect.set(ConfigFile::StyleCss, FileHash::of(b"not what is there"));
    let applied = apply(dir.path(), &plan, &o).unwrap();
    assert!(applied.files[0].written);
}

#[test]
fn no_expect_keeps_hand_edit() {
    let dir = folder(BAR, CSS);
    apply_ok(dir.path(), &plan_of(Some(BAR), None, &[height(40)]).unwrap());
    // A person edits the file by hand; the plan is made from what is there now.
    let by_hand = read(dir.path(), "bar.toml").replace("# my bar", "# my bar\n# note to self");
    std::fs::write(dir.path().join("bar.toml"), &by_hand).unwrap();
    let plan = plan_of(Some(&by_hand), None, &[height(44)]).unwrap();
    let applied = apply_ok(dir.path(), &plan);
    let now = read(dir.path(), "bar.toml");
    assert!(now.contains("# note to self") && now.contains("height = 44"), "{now}");
    let n = applied.notices.iter().find(|n| n.code == NoticeCode::HandEditKept).expect("hand_edit_kept");
    assert_eq!(n.file, Some(ConfigFile::BarToml));
    assert!(!applied.files[0].edited_by_hand, "the file is now what settings wrote");
    // The next change has nothing to report.
    let again = apply_ok(dir.path(), &plan_of(Some(&now), None, &[height(45)]).unwrap());
    assert!(again.notices.is_empty());
}

/// A folder whose `style.css` holds the colours block a first accent change wrote, plus a line of the person's own
/// after it; returns the folder and the file's text.
fn folder_with_a_colours_block() -> (TempDir, String) {
    let dir = folder(BAR, CSS);
    let accent =
        |hex: &str| plan_of(Some(BAR), Some(&read(dir.path(), "style.css")), &[set("color.accent", json!(hex))]);
    apply_ok(dir.path(), &accent("#e5a50a").unwrap());
    let text = read(dir.path(), "style.css");
    (dir, text)
}

fn hand_edit_notice_of(applied: &Applied) -> Option<&rsb_core::contract::Notice> {
    applied.notices.iter().find(|n| n.code == NoticeCode::HandEditKept)
}

#[test]
fn a_hand_edit_only_inside_the_colours_block_is_not_called_kept() {
    let (dir, text) = folder_with_a_colours_block();
    let edited = text.replacen("--accent-hover: #e5a50a", "--accent-hover: #ffffff", 1);
    assert_ne!(edited, text);
    std::fs::write(dir.path().join("style.css"), &edited).unwrap();
    let plan = plan_of(Some(BAR), Some(&edited), &[set("color.accent", json!("#123456"))]).unwrap();
    assert!(plan.notices.iter().any(|n| n.code == NoticeCode::BlockEditReplaced), "the plan says it replaces it");
    let applied = apply_ok(dir.path(), &plan);
    assert!(hand_edit_notice_of(&applied).is_none(), "{:?}", applied.notices);
    assert!(!read(dir.path(), "style.css").contains("#ffffff"), "the block edit was replaced");
}

#[test]
fn a_hand_edit_outside_the_colours_block_is_kept_and_says_where() {
    let (dir, text) = folder_with_a_colours_block();
    // One edit inside the block and one after it.
    let edited =
        format!("{}{}", text.replacen("--accent-hover: #e5a50a", "--accent-hover: #ffffff", 1), "/* mine */\n");
    std::fs::write(dir.path().join("style.css"), &edited).unwrap();
    let plan = plan_of(Some(BAR), Some(&edited), &[set("color.accent", json!("#123456"))]).unwrap();
    let applied = apply_ok(dir.path(), &plan);
    let n = hand_edit_notice_of(&applied).expect("hand_edit_kept");
    assert!(n.message.contains("outside the colours block"), "{}", n.message);
    assert!(read(dir.path(), "style.css").contains("/* mine */"));
}

#[test]
fn an_edit_inside_the_block_with_no_copy_of_what_was_written_is_still_reported() {
    let (dir, text) = folder_with_a_colours_block();
    let edited = text.replacen("--accent-hover: #e5a50a", "--accent-hover: #ffffff", 1);
    std::fs::write(dir.path().join("style.css"), &edited).unwrap();
    std::fs::remove_file(dir.path().join(".undo").join("last-good.style.css.txt")).unwrap();
    let plan = plan_of(Some(BAR), Some(&edited), &[set("color.accent", json!("#123456"))]).unwrap();
    let applied = apply_ok(dir.path(), &plan);
    assert!(hand_edit_notice_of(&applied).is_some(), "nothing proves where the edit was");
}

#[test]
fn a_save_between_planning_and_writing_is_a_changed_file() {
    let dir = folder(BAR, CSS);
    let plan = plan_of(Some(BAR), None, &[height(40)]).unwrap();
    std::fs::write(dir.path().join("bar.toml"), format!("{BAR}# added\n")).unwrap();
    let e = apply(dir.path(), &plan, &opts()).unwrap_err();
    assert_eq!(e.code, ErrorCode::FileChanged);
    assert!(read(dir.path(), "bar.toml").ends_with("# added\n"));
    no_temps(dir.path());
}

#[test]
fn undo_after_hand_edit_refused() {
    let dir = folder(BAR, CSS);
    apply_ok(dir.path(), &plan_of(Some(BAR), None, &[height(40)]).unwrap());
    let by_hand = format!("{}# later\n", read(dir.path(), "bar.toml"));
    std::fs::write(dir.path().join("bar.toml"), &by_hand).unwrap();
    let e = undo(dir.path(), None, &opts()).unwrap_err();
    assert_eq!(e.code, ErrorCode::UndoConflict);
    assert!(e.next.as_deref().unwrap().contains("before.bar.toml"), "{:?}", e.next);
    assert_eq!(read(dir.path(), "bar.toml"), by_hand);
    assert_eq!(log_entries(dir.path(), 5).unwrap().len(), 1, "the entry stays");
}

#[test]
fn undo_not_latest_refused() {
    let dir = folder(BAR, CSS);
    let first = plan_of(Some(BAR), None, &[height(40)]).unwrap();
    apply_ok(dir.path(), &first);
    let now = read(dir.path(), "bar.toml");
    apply_ok(dir.path(), &plan_of(Some(&now), None, &[height(44)]).unwrap());
    let e = undo(dir.path(), Some(1), &opts()).unwrap_err();
    assert_eq!(e.code, ErrorCode::UndoNotLatest);
    assert!(e.message.contains("change 1 is not the newest (2 is)"), "{}", e.message);
    assert!(read(dir.path(), "bar.toml").contains("height = 44"));
    // The newest can be undone, by number or without one, and then the one before.
    let u = undo(dir.path(), Some(2), &opts()).unwrap();
    assert_eq!(u.undo, None);
    assert!(read(dir.path(), "bar.toml").contains("height = 40"));
    undo(dir.path(), None, &opts()).unwrap();
    assert_eq!(read(dir.path(), "bar.toml"), BAR);
    assert_eq!(undo(dir.path(), None, &opts()).unwrap_err().code, ErrorCode::NothingToUndo);
}

#[test]
fn edited_by_hand_after_undo_is_false() {
    let dir = folder(BAR, CSS);
    let applied = apply_ok(dir.path(), &plan_of(Some(BAR), None, &[height(40)]).unwrap());
    let u = undo(dir.path(), None, &opts()).unwrap();
    assert_eq!(read(dir.path(), "bar.toml"), BAR);
    assert!(u.files[0].written && !u.files[0].edited_by_hand && u.files[0].hash == hash_of(dir.path(), "bar.toml"));
    assert_eq!(written(dir.path()).bar_toml, Some(hash_of(dir.path(), "bar.toml")));
    assert_ne!(applied.files[0].hash, u.files[0].hash);
    // A hand edit afterwards shows in the file outcomes of a change that writes the other file.
    std::fs::write(dir.path().join("bar.toml"), format!("{BAR}# x\n")).unwrap();
    let css = plan_of(None, Some(CSS), &[set("color.accent", json!("#e5a50a"))]).unwrap();
    let a = apply_ok(dir.path(), &css);
    assert!(a.files[0].edited_by_hand && !a.files[0].written);
    assert!(a.files[1].written);
}

#[test]
fn undo_restores_both_files_and_a_created_file_is_deleted_again() {
    let dir = TempDir::new("cfgwrite");
    std::fs::write(dir.path().join("bar.toml"), BAR).unwrap();
    let css = plan_of(Some(BAR), None, &[set("color.accent", json!("#e5a50a"))]).unwrap();
    assert!(css.file(ConfigFile::StyleCss).is_some_and(|f| f.created));
    let both = plan_of(Some(BAR), None, &[height(40), set("color.accent", json!("#e5a50a"))]).unwrap();
    let applied = apply_ok(dir.path(), &both);
    assert!(applied.files.iter().all(|f| f.written));
    assert!(dir.path().join("style.css").exists());
    let u = undo(dir.path(), None, &opts()).unwrap();
    assert_eq!(read(dir.path(), "bar.toml"), BAR);
    assert!(!dir.path().join("style.css").exists(), "RSB made it, undo removes it");
    assert_eq!(u.files[1].hash, FileHash::Missing);
    assert_eq!(written(dir.path()).style_css, Some(FileHash::Missing));
}

// ---------------------------------------------------------------- atomic saves and the lock

#[test]
fn editor_open_without_write_share_blocks_write() {
    let dir = folder(BAR, CSS);
    let plan = plan_of(Some(BAR), None, &[height(40)]).unwrap();
    let target = dir.path().join("bar.toml");
    let seen: Rc<Cell<Option<Option<i32>>>> = Rc::new(Cell::new(None));
    let s = seen.clone();
    AFTER_PREPARE.with(|h| {
        *h.borrow_mut() = Some(Box::new(move || {
            // An editor saving in place opens the file for writing: it must wait.
            let r = OpenOptions::new().write(true).open(&target);
            s.set(Some(r.err().and_then(|e| e.raw_os_error())));
        }))
    });
    apply_ok(dir.path(), &plan);
    assert_eq!(seen.get(), Some(Some(32)), "a sharing violation while we hold the file");
}

#[test]
fn atomic_save_between_read_and_rename_not_lost() {
    let dir = folder(BAR, CSS);
    let plan = plan_of(Some(BAR), None, &[height(40)]).unwrap();
    let target = dir.path().join("bar.toml");
    let editor_tmp = dir.path().join("editor.tmp");
    let editors_text = format!("{BAR}# saved by the editor\n");
    std::fs::write(&editor_tmp, &editors_text).unwrap();
    let (t, e) = (target.clone(), editor_tmp.clone());
    AFTER_PREPARE.with(|h| {
        *h.borrow_mut() = Some(Box::new(move || {
            // The editor's save: a new file renamed over the target.
            std::fs::rename(&e, &t).expect("the editor's rename works while we hold the file");
        }))
    });
    let err = apply(dir.path(), &plan, &opts()).unwrap_err();
    assert_eq!((err.code, err.file), (ErrorCode::FileChanged, Some(ConfigFile::BarToml)));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), editors_text, "the editor's bytes are the file");
    no_temps(dir.path());
    assert!(!dir.path().join(".undo").join("1.part").exists() && !dir.path().join(".undo").join("1").exists());
}

#[test]
fn busy_when_lock_held() {
    let dir = folder(BAR, CSS);
    let undo_dir = dir.path().join(".undo");
    std::fs::create_dir(&undo_dir).unwrap();
    let held = win::open_lock(&undo_dir.join("lock")).unwrap();
    let plan = plan_of(Some(BAR), None, &[height(40)]).unwrap();
    let before = snapshot(dir.path());
    let t = Instant::now();
    let e = apply(dir.path(), &plan, &opts()).unwrap_err();
    let no_wait = t.elapsed();
    assert_eq!(e.code, ErrorCode::Busy);
    assert!(no_wait.as_millis() < 50, "{no_wait:?}");
    assert_eq!(snapshot(dir.path()), before);
    let t = Instant::now();
    let e = apply(dir.path(), &plan, &WriteOpts { wait_lock: true, ..opts() }).unwrap_err();
    let waited = t.elapsed();
    assert_eq!(e.code, ErrorCode::Busy);
    assert!((1900..3000).contains(&(waited.as_millis() as u64)), "{waited:?}");
    // The lock of a holder that goes away is free again at once.
    drop(held);
    assert!(apply(dir.path(), &plan, &opts()).is_ok());
}

#[test]
fn a_waiting_write_goes_through_once_the_lock_is_free() {
    let dir = folder(BAR, CSS);
    let undo_dir = dir.path().join(".undo");
    std::fs::create_dir(&undo_dir).unwrap();
    let held = win::open_lock(&undo_dir.join("lock")).unwrap();
    let plan = plan_of(Some(BAR), None, &[height(40)]).unwrap();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(200));
        drop(held);
    });
    let t = Instant::now();
    let r = apply(dir.path(), &plan, &WriteOpts { wait_lock: true, ..opts() });
    releaser.join().unwrap();
    assert!(r.is_ok(), "{r:?}");
    assert!(t.elapsed().as_millis() >= 150);
}

// ---------------------------------------------------------------- refusals

fn assert_untouched(dir: &Path, before: &BTreeMap<String, FileHash>) {
    let now: BTreeMap<_, _> = snapshot(dir).into_iter().filter(|(k, _)| !k.starts_with(".undo")).collect();
    let before: BTreeMap<_, _> =
        before.iter().filter(|(k, _)| !k.starts_with(".undo")).map(|(k, v)| (k.clone(), *v)).collect();
    assert_eq!(now, before);
    no_temps(dir);
}

#[test]
// The attribute is Windows only here; the lint is about Unix permission bits.
#[allow(clippy::permissions_set_readonly_false)]
fn refuses_read_only() {
    let dir = folder(BAR, CSS);
    let plan = plan_of(Some(BAR), None, &[height(40)]).unwrap();
    let path = dir.path().join("bar.toml");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&path, perms.clone()).unwrap();
    let before = snapshot(dir.path());
    let e = apply(dir.path(), &plan, &opts()).unwrap_err();
    perms.set_readonly(false);
    std::fs::set_permissions(&path, perms).unwrap();
    assert_eq!(e.code, ErrorCode::ReadOnly);
    assert_untouched(dir.path(), &before);
}

#[test]
fn refuses_hardlink() {
    let dir = folder(BAR, CSS);
    std::fs::hard_link(dir.path().join("bar.toml"), dir.path().join("other.toml")).unwrap();
    let plan = plan_of(Some(BAR), None, &[height(40)]).unwrap();
    let before = snapshot(dir.path());
    let e = apply(dir.path(), &plan, &opts()).unwrap_err();
    assert_eq!(e.code, ErrorCode::LinkedFile);
    assert_untouched(dir.path(), &before);
}

/// A link to `target` at `link`, or false when this account may not make one.
fn make_file_link(link: &Path, target: &Path) -> bool {
    std::os::windows::fs::symlink_file(target, link).is_ok()
}

#[test]
fn refuses_link() {
    // A symbolic link when the account may make one; else a junction, which is a reparse point
    // the same check refuses. If neither can be made the test fails: it never passes by doing nothing.
    let dir = TempDir::new("cfgwrite");
    let real = dir.path().join("real.toml");
    std::fs::write(&real, BAR).unwrap();
    std::fs::write(dir.path().join("style.css"), CSS).unwrap();
    let link = dir.path().join("bar.toml");
    let mut kind = "symbolic link";
    let target = TempDir::new("cfgwrite-out");
    if !make_file_link(&link, &real) {
        kind = "junction";
        assert!(make_junction(&link, target.path()), "neither a symbolic link nor a junction could be made");
    }
    let plan = plan_of(Some(BAR), None, &[height(40)]).unwrap();
    let before = snapshot(dir.path());
    let e = apply(dir.path(), &plan, &opts()).unwrap_err();
    assert_eq!(e.code, ErrorCode::LinkedFile, "{kind}: {e:?}");
    assert!(e.message.contains("link"), "{kind}: {}", e.message);
    assert_untouched(dir.path(), &before);
    assert_eq!(std::fs::read_to_string(&real).unwrap(), BAR, "the target of the link is untouched");
    let _ = std::fs::remove_dir(&link);
}

fn bare_plan(file: ConfigFile, base: FileHash) -> Plan {
    Plan {
        files: vec![rsb_core::cfgedit::FilePlan {
            file,
            base,
            created: false,
            bom: false,
            splices: Vec::new(),
            after: "x = 1\n".into(),
            what: "x".into(),
        }],
        changes: Vec::new(),
        notices: Vec::new(),
        preset_switch: None,
        accent: None,
    }
}

#[test]
fn refuses_utf16() {
    for bytes in
        [vec![0xff, 0xfe, b'a', 0, b'=', 0], vec![0xfe, 0xff, 0, b'a', 0, b'='], vec![b'a', b' ', 0xc3, 0x28, b'\n']]
    {
        let dir = folder(BAR, CSS);
        std::fs::write(dir.path().join("bar.toml"), &bytes).unwrap();
        let before = snapshot(dir.path());
        let e = apply(dir.path(), &bare_plan(ConfigFile::BarToml, FileHash::of(b"a")), &opts()).unwrap_err();
        assert_eq!(e.code, ErrorCode::NotUtf8, "{bytes:?}");
        assert_untouched(dir.path(), &before);
    }
}

#[test]
fn refuses_inline_bar_table() {
    let text = "bar = [{ name = \"main\", height = 40 }]\n";
    let dir = folder(text, CSS);
    let before = snapshot(dir.path());
    let e = plan_of(Some(text), None, &[height(44)]).unwrap_err();
    assert_eq!(e.code, ErrorCode::UnsupportedLayout);
    assert_untouched(dir.path(), &before);
}

#[test]
fn stale_temp_cleaned() {
    let dir = folder(BAR, CSS);
    let undo_dir = dir.path().join(".undo");
    std::fs::create_dir(&undo_dir).unwrap();
    for (folder, name) in [
        (dir.path(), ".bar.toml.rsb-999-1.tmp"),
        (dir.path(), ".style.css.rsb-998-4.tmp"),
        (undo_dir.as_path(), ".written.rsb-9-9.tmp"),
    ] {
        std::fs::write(folder.join(name), b"old").unwrap();
    }
    std::fs::write(dir.path().join(".keep.tmp"), b"mine").unwrap();
    std::fs::write(dir.path().join(".hidden"), b"mine").unwrap();
    apply_ok(dir.path(), &plan_of(Some(BAR), None, &[height(40)]).unwrap());
    no_temps(dir.path());
    assert!(dir.path().join(".keep.tmp").exists() && dir.path().join(".hidden").exists(), "only our own names go");
}

#[test]
fn temp_name_link_refused() {
    let dir = folder(BAR, CSS);
    let outside = TempDir::new("cfgwrite-out");
    let target = outside.path().join("victim.txt");
    std::fs::write(&target, b"do not touch").unwrap();
    // Occupy the next temporary names: with a link when we may make one, else a plain file.
    let next = TEMP_SEQ.load(Ordering::Relaxed);
    let mut made_link = false;
    for k in 0..4 {
        let name = format!(".bar.toml.rsb-{}-{}.tmp", std::process::id(), next + k);
        let link = dir.path().join(&name);
        if make_file_link(&link, &target) {
            made_link = true;
        } else {
            std::fs::write(&link, b"squatter").unwrap();
        }
    }
    let plan = plan_of(Some(BAR), None, &[height(40)]).unwrap();
    // Other tests may take sequence numbers meanwhile: only count what is ours.
    let applied = apply(dir.path(), &plan, &opts());
    assert!(applied.is_ok(), "{applied:?}");
    assert_eq!(
        std::fs::read(&target).unwrap(),
        b"do not touch",
        "a link as a temporary name is never followed (link made: {made_link})"
    );
    assert_eq!(read(dir.path(), "bar.toml"), plan.files[0].after);
}

#[test]
fn not_config_folder_refused() {
    let dir = TempDir::new("cfgwrite");
    let plan = plan_of(Some(BAR), None, &[height(40)]).unwrap();
    let e = apply(dir.path(), &plan, &opts()).unwrap_err();
    assert_eq!(e.code, ErrorCode::BadUsage);
    assert!(e.message.contains("is not an RSB config folder"), "{}", e.message);
    assert!(snapshot(dir.path()).is_empty(), "nothing was made in it");
    let file = dir.path().join("plain.txt");
    std::fs::write(&file, b"x").unwrap();
    assert_eq!(apply(&file, &plan, &opts()).unwrap_err().code, ErrorCode::BadUsage);
    assert_eq!(undo(dir.path(), None, &opts()).unwrap_err().code, ErrorCode::BadUsage);
}

#[test]
fn first_change_with_no_folder() {
    let base = TempDir::new("cfgwrite");
    let dir = base.path().join("fresh");
    let plan = plan_of(None, None, &[set("color.accent", json!("#e5a50a"))]).unwrap();
    assert!(plan.file(ConfigFile::StyleCss).is_some_and(|f| f.created));
    let applied = apply(&dir, &plan, &opts()).unwrap();
    assert!(dir.join("bar.toml").exists() && dir.join("widgets").is_dir());
    assert_eq!(read(&dir, "style.css"), plan.files[0].after);
    assert!(read(&dir, "style.css").contains("/* End of colours set by rsb settings. */"));
    assert!(applied.notices.iter().any(|n| n.code == NoticeCode::FileCreated));
    assert_eq!(applied.undo, Some(1));
    // `bar.toml` is the starting file; a second change finds both in place.
    let after = read(&dir, "bar.toml");
    assert!(after.starts_with("config_version = 1"));
    let second = plan_of(Some(&after), Some(&read(&dir, "style.css")), &[set("bar.main.height", json!(40))]).unwrap();
    assert!(apply(&dir, &second, &opts()).is_ok());
}

#[test]
fn dry_run_writes_nothing() {
    let dir = folder(BAR, CSS);
    let plan = plan_of(Some(BAR), None, &[height(40)]).unwrap();
    let before = snapshot(dir.path());
    let applied = apply(dir.path(), &plan, &WriteOpts { dry_run: true, ..opts() }).unwrap();
    assert_eq!(snapshot(dir.path()), before, "no file, no .undo folder, no temp");
    assert!(!applied.files[0].written && applied.undo.is_none());
    assert_eq!(applied.files[0].hash, FileHash::of(plan.files[0].after.as_bytes()));
    // The same checks run: a mismatch is refused.
    let mut o = WriteOpts { dry_run: true, ..opts() };
    o.expect.set(ConfigFile::BarToml, FileHash::of(b"x"));
    assert_eq!(apply(dir.path(), &plan, &o).unwrap_err().code, ErrorCode::FileChanged);
    // A folder that does not exist is not made.
    let base = TempDir::new("cfgwrite");
    let fresh = base.path().join("fresh");
    let css = plan_of(None, None, &[set("color.accent", json!("#e5a50a"))]).unwrap();
    let a = apply(&fresh, &css, &WriteOpts { dry_run: true, ..opts() }).unwrap();
    assert!(!fresh.exists() && a.notices.iter().any(|n| n.code == NoticeCode::FileCreated));
    // Undo and restore have a dry run too.
    apply_ok(dir.path(), &plan);
    let snap = snapshot(dir.path());
    let u = undo(dir.path(), None, &WriteOpts { dry_run: true, ..opts() }).unwrap();
    assert_eq!(u.files[0].hash, FileHash::of(BAR.as_bytes()));
    assert_eq!(snapshot(dir.path()), snap);
}

#[test]
fn refuses_when_elevated() {
    let dir = folder(BAR, CSS);
    let plan = plan_of(Some(BAR), None, &[height(40)]).unwrap();
    let before = snapshot(dir.path());
    win::FAKE_HIGH.with(|f| f.set(Some(true)));
    let results = [
        apply(dir.path(), &plan, &opts()).unwrap_err().code,
        apply(dir.path(), &plan, &WriteOpts { dry_run: true, ..opts() }).unwrap_err().code,
        undo(dir.path(), None, &opts()).unwrap_err().code,
        restore(dir.path(), ConfigFile::BarToml, &opts()).unwrap_err().code,
    ];
    let kept = keep_last_good(dir.path(), ConfigFile::BarToml, BAR.as_bytes());
    win::FAKE_HIGH.with(|f| f.set(None));
    assert!(results.iter().all(|c| *c == ErrorCode::Elevated), "{results:?}");
    assert!(!kept);
    assert_eq!(snapshot(dir.path()), before, "not even an .undo folder");
    assert!(apply(dir.path(), &plan, &opts()).is_ok());
}

// ---------------------------------------------------------------- junction and links

/// Makes `link` a link to the folder `target`: a symbolic link when allowed,
/// else a junction, which needs no privilege.
fn make_dir_link(link: &Path, target: &Path) -> bool {
    if std::os::windows::fs::symlink_dir(target, link).is_ok() {
        return true;
    }
    make_junction(link, target)
}

fn make_junction(link: &Path, target: &Path) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::IO::DeviceIoControl;
    if std::fs::create_dir(link).is_err() {
        return false;
    }
    let Ok(target) = std::fs::canonicalize(target) else { return false };
    let sub: Vec<u16> = target.to_string_lossy().replace("\\\\?\\", "\\??\\").encode_utf16().collect();
    let print: Vec<u16> = Vec::new();
    // REPARSE_DATA_BUFFER for IO_REPARSE_TAG_MOUNT_POINT.
    let path_bytes = (sub.len() * 2 + 2 + print.len() * 2 + 2) as u16;
    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(&0xA000_0003u32.to_le_bytes());
    buf.extend_from_slice(&(path_bytes + 8).to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&((sub.len() * 2) as u16).to_le_bytes());
    buf.extend_from_slice(&((sub.len() * 2 + 2) as u16).to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    for c in sub.iter().chain([0u16].iter()).chain(print.iter()).chain([0u16].iter()) {
        buf.extend_from_slice(&c.to_le_bytes());
    }
    let Ok(dir) = OpenOptions::new().write(true).custom_flags(0x0200_0000 | 0x0020_0000).open(link) else {
        return false;
    };
    let mut returned = 0u32;
    // SAFETY: the handle is open; the buffer holds a well-formed mount point reparse record.
    let ok = unsafe {
        DeviceIoControl(
            HANDLE(dir.as_raw_handle()),
            0x0009_00A4,
            Some(buf.as_ptr().cast()),
            buf.len() as u32,
            None,
            0,
            Some(&mut returned),
            None,
        )
    };
    ok.is_ok()
}

#[test]
fn a_folder_or_junction_where_a_file_belongs_is_refused() {
    let dir = TempDir::new("cfgwrite");
    std::fs::write(dir.path().join("style.css"), CSS).unwrap();
    let elsewhere = TempDir::new("cfgwrite-out");
    std::fs::write(elsewhere.path().join("keep.txt"), b"keep").unwrap();
    // A plain folder named bar.toml.
    std::fs::create_dir(dir.path().join("bar.toml")).unwrap();
    let plan = plan_of(Some(BAR), None, &[height(40)]).unwrap();
    assert_eq!(apply(dir.path(), &plan, &opts()).unwrap_err().code, ErrorCode::LinkedFile);
    std::fs::remove_dir(dir.path().join("bar.toml")).unwrap();
    // A junction named bar.toml that points at another folder.
    if !make_junction(&dir.path().join("bar.toml"), elsewhere.path()) {
        eprintln!("a_folder_or_junction_where_a_file_belongs_is_refused: no junction could be made here");
        return;
    }
    let e = apply(dir.path(), &plan, &opts()).unwrap_err();
    assert_eq!(e.code, ErrorCode::LinkedFile);
    assert_eq!(std::fs::read(elsewhere.path().join("keep.txt")).unwrap(), b"keep");
    assert_eq!(std::fs::read_dir(elsewhere.path()).unwrap().count(), 1);
    no_temps(dir.path());
    let _ = std::fs::remove_dir(dir.path().join("bar.toml"));
}

#[test]
fn undo_folder_junction_refused() {
    let dir = folder(BAR, CSS);
    let elsewhere = TempDir::new("cfgwrite-out");
    if !make_dir_link(&dir.path().join(".undo"), elsewhere.path()) {
        eprintln!("undo_folder_junction_refused: no link could be made here");
        return;
    }
    let plan = plan_of(Some(BAR), None, &[height(40)]).unwrap();
    let before = snapshot(dir.path());
    let e = apply(dir.path(), &plan, &opts()).unwrap_err();
    assert_eq!(e.code, ErrorCode::LinkedFile);
    assert_eq!(read(dir.path(), "bar.toml"), BAR);
    assert!(std::fs::read_dir(elsewhere.path()).unwrap().next().is_none(), "nothing was written through the link");
    no_temps(dir.path());
    let _ = before;
    // The reader's copy refuses it too.
    assert!(!keep_last_good(dir.path(), ConfigFile::BarToml, BAR.as_bytes()));
    assert!(std::fs::read_dir(elsewhere.path()).unwrap().next().is_none());
    // Remove the junction itself, never what it points to.
    let _ = std::fs::remove_dir(dir.path().join(".undo"));
}

// ---------------------------------------------------------------- batches and crashes

#[test]
fn batch_restores_bar_toml_when_style_fails() {
    let dir = folder(BAR, CSS);
    let plan = plan_of(Some(BAR), Some(CSS), &[height(40), set("color.accent", json!("#e5a50a"))]).unwrap();
    assert_eq!(plan.files.len(), 2);
    let style = dir.path().join("style.css");
    let held: Rc<RefCell<Option<std::fs::File>>> = Rc::new(RefCell::new(None));
    let h = held.clone();
    BETWEEN_COMMITS.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || {
            // Someone holds style.css open without sharing delete: the second replace cannot work.
            *h.borrow_mut() = OpenOptions::new().read(true).share_mode(3).open(&style).ok();
        }))
    });
    let t = Instant::now();
    let e = apply(dir.path(), &plan, &opts()).unwrap_err();
    let took = t.elapsed();
    drop(held);
    assert_eq!((e.code, e.file), (ErrorCode::WriteFailed, Some(ConfigFile::StyleCss)), "{e:?}");
    assert!(e.copies.is_empty());
    assert_eq!(read(dir.path(), "bar.toml"), BAR, "bar.toml is back as it was");
    assert_eq!(read(dir.path(), "style.css"), CSS);
    no_temps(dir.path());
    assert!(log_entries(dir.path(), 5).unwrap().is_empty());
    assert!(!dir.path().join(".undo").join("1.part").exists());
    assert!(took.as_millis() < 10_000, "{took:?}");
    // With nothing in the way the same plan goes through as one entry.
    let ok = apply_ok(dir.path(), &plan);
    assert_eq!(ok.undo, Some(1));
    assert!(ok.files.iter().all(|f| f.written));
}

#[test]
fn a_rollback_that_cannot_run_names_both_copies_and_keeps_the_entry() {
    let dir = folder(BAR, CSS);
    let plan = plan_of(Some(BAR), Some(CSS), &[height(40), set("color.accent", json!("#e5a50a"))]).unwrap();
    let (style, bar) = (dir.path().join("style.css"), dir.path().join("bar.toml"));
    let held: Rc<RefCell<Option<std::fs::File>>> = Rc::new(RefCell::new(None));
    let h = held.clone();
    BETWEEN_COMMITS.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || {
            *h.borrow_mut() = OpenOptions::new().read(true).share_mode(3).open(&style).ok();
            // A person saves bar.toml by hand right after our replace.
            std::fs::write(&bar, "# the person's own save\n").unwrap();
        }))
    });
    let e = apply(dir.path(), &plan, &opts()).unwrap_err();
    drop(held);
    assert_eq!(e.code, ErrorCode::WriteFailed);
    assert_eq!(
        read(dir.path(), "bar.toml"),
        "# the person's own save\n",
        "a save we did not make is never overwritten"
    );
    assert!(!e.copies.is_empty() && e.copies.iter().all(|c| Path::new(c).exists()), "{:?}", e.copies);
    assert!(dir.path().join(".undo").join("1.part").exists(), "kept for the next write to settle");
    // The next write finds the entry unfinished and bar.toml neither where it was nor where the
    // change put it: the copies are not deleted, and the reply says so.
    let now = read(dir.path(), "bar.toml");
    let next = plan_of(Some(&now), None, &[set("preset", json!("glass"))]).unwrap();
    let applied = apply_ok(dir.path(), &next);
    let part = dir.path().join(".undo").join("1.part");
    assert!(part.join("before.bar.toml").exists() && part.join("before.style.css.txt").exists());
    assert_eq!(std::fs::read_to_string(part.join("before.bar.toml")).unwrap(), BAR);
    assert!(
        applied
            .notices
            .iter()
            .any(|n| n.code == NoticeCode::HandEditKept
                && n.next.as_deref().is_some_and(|t| t.contains("before.bar.toml"))),
        "{:?}",
        applied.notices
    );
    for c in &e.copies {
        assert!(Path::new(c).exists(), "a copy named in the error survives the next write: {c}");
    }
}

#[test]
fn kept_copies_survive_the_next_write() {
    // The notice comes with every later write while the entry is unsettled.
    let dir = folder(BAR, CSS);
    let plan = plan_of(Some(BAR), Some(CSS), &[height(40), set("color.accent", json!("#e5a50a"))]).unwrap();
    let style = dir.path().join("style.css");
    let bar = dir.path().join("bar.toml");
    let held: Rc<RefCell<Option<std::fs::File>>> = Rc::new(RefCell::new(None));
    let h = held.clone();
    BETWEEN_COMMITS.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || {
            *h.borrow_mut() = OpenOptions::new().read(true).share_mode(3).open(&style).ok();
            std::fs::write(&bar, "# edited\n").unwrap();
        }))
    });
    let e = apply(dir.path(), &plan, &opts()).unwrap_err();
    drop(held);
    let copies = e.copies.clone();
    assert_eq!(copies.len(), 2, "{copies:?}");
    for round in 0..2 {
        let now = read(dir.path(), "bar.toml");
        let next = plan_of(Some(&now), None, &[height(40 + round)]).unwrap();
        let a = apply_ok(dir.path(), &next);
        assert!(a.notices.iter().any(|n| n.code == NoticeCode::HandEditKept && n.message.contains("left as it is")));
        assert!(copies.iter().all(|c| Path::new(c).exists()), "round {round}");
    }
}

/// A two-file change that was killed between its two replaces: bar.toml is
/// already new, style.css is not, and the entry is still `.part`.
fn killed_between_the_replaces() -> TempDir {
    let dir = folder(BAR, CSS);
    let plan = plan_of(Some(BAR), Some(CSS), &[height(40), set("color.accent", json!("#e5a50a"))]).unwrap();
    apply_ok(dir.path(), &plan);
    std::fs::write(dir.path().join("style.css"), CSS).unwrap();
    let undo_dir = dir.path().join(".undo");
    std::fs::rename(undo_dir.join("1"), undo_dir.join("1.part")).unwrap();
    dir
}

#[test]
fn interrupted_two_file_change_rolled_back() {
    let dir = killed_between_the_replaces();
    assert_ne!(read(dir.path(), "bar.toml"), BAR);
    // The first write after the crash puts bar.toml back (and is itself refused, as the file it
    // was planned against is gone); nothing is half applied and nothing is lost.
    let now = read(dir.path(), "bar.toml");
    let plan = plan_of(Some(&now), None, &[height(44)]).unwrap();
    assert_eq!(apply(dir.path(), &plan, &opts()).unwrap_err().code, ErrorCode::FileChanged);
    assert_eq!(read(dir.path(), "bar.toml"), BAR, "back as it was before the change");
    assert_eq!(read(dir.path(), "style.css"), CSS);
    assert!(!dir.path().join(".undo").join("1.part").exists());
    no_temps(dir.path());
    // The write goes through once planned against the files as they are.
    let ok = apply_ok(dir.path(), &plan_of(Some(BAR), None, &[height(44)]).unwrap());
    assert!(ok.undo.is_some());
}

#[test]
fn an_undo_that_was_cut_off_is_finished() {
    let dir = folder(BAR, CSS);
    let plan = plan_of(Some(BAR), Some(CSS), &[height(40), set("color.accent", json!("#e5a50a"))]).unwrap();
    apply_ok(dir.path(), &plan);
    // The undo replaced style.css, then died: bar.toml is still the new text and the entry stays.
    std::fs::write(dir.path().join("style.css"), CSS).unwrap();
    std::fs::write(dir.path().join(".undo").join("1").join("undoing"), b"").unwrap();
    let now = read(dir.path(), "bar.toml");
    let r = apply(dir.path(), &plan_of(Some(&now), None, &[height(44)]).unwrap(), &opts());
    assert_eq!(r.unwrap_err().code, ErrorCode::FileChanged);
    assert_eq!(read(dir.path(), "bar.toml"), BAR);
    assert!(!dir.path().join(".undo").join("1").exists(), "the undone entry is gone");
    // A marker with no file changed means the undo never began: only the marker goes.
    let dir = folder(BAR, CSS);
    apply_ok(dir.path(), &plan_of(Some(BAR), None, &[height(40)]).unwrap());
    std::fs::write(dir.path().join(".undo").join("1").join("undoing"), b"").unwrap();
    let now = read(dir.path(), "bar.toml");
    apply_ok(dir.path(), &plan_of(Some(&now), None, &[height(44)]).unwrap());
    assert!(dir.path().join(".undo").join("1").join("change.json").exists());
    assert!(!dir.path().join(".undo").join("1").join("undoing").exists());
}

#[test]
fn a_part_folder_that_is_a_junction_is_not_followed_or_emptied() {
    let dir = folder(BAR, CSS);
    let elsewhere = TempDir::new("cfgwrite-out");
    for name in ["change.json", "before.bar.toml", "before.style.css.txt", "other.txt"] {
        std::fs::write(elsewhere.path().join(name), b"theirs").unwrap();
    }
    std::fs::create_dir_all(dir.path().join(".undo")).unwrap();
    if !make_junction(&dir.path().join(".undo").join("7.part"), elsewhere.path()) {
        eprintln!("a_part_folder_that_is_a_junction: no junction could be made here");
        return;
    }
    apply_ok(dir.path(), &plan_of(Some(BAR), None, &[height(40)]).unwrap());
    assert_eq!(std::fs::read_dir(elsewhere.path()).unwrap().count(), 4, "nothing was deleted through the junction");
    let (done, parts) = numbers(&dir.path().join(".undo"));
    assert!(parts.is_empty() && done == vec![1], "the junction is not an entry: {done:?} {parts:?}");
    let _ = std::fs::remove_dir(dir.path().join(".undo").join("7.part"));
}

#[test]
fn entry_numbers_do_not_wrap_and_odd_names_are_not_entries() {
    let dir = folder(BAR, CSS);
    let undo_dir = dir.path().join(".undo");
    for name in ["007", "+5", "4294967296", "1e3"] {
        std::fs::create_dir_all(undo_dir.join(name)).unwrap();
    }
    assert_eq!(numbers(&undo_dir), (Vec::new(), Vec::new()));
    std::fs::create_dir_all(undo_dir.join("4294967295")).unwrap();
    let before = read(dir.path(), "bar.toml");
    let e = apply(dir.path(), &plan_of(Some(BAR), None, &[height(40)]).unwrap(), &opts()).unwrap_err();
    assert_eq!(e.code, ErrorCode::WriteFailed);
    assert!(e.message.contains("too many undo entries"), "{}", e.message);
    assert_eq!(read(dir.path(), "bar.toml"), before, "nothing was written");
    no_temps(dir.path());
}

#[test]
fn a_folder_where_the_lock_belongs_is_not_a_busy_write() {
    let dir = folder(BAR, CSS);
    std::fs::create_dir_all(dir.path().join(".undo").join("lock")).unwrap();
    let t = Instant::now();
    let plan = plan_of(Some(BAR), None, &[height(40)]).unwrap();
    let e = apply(dir.path(), &plan, &WriteOpts { wait_lock: true, ..opts() }).unwrap_err();
    assert_eq!(e.code, ErrorCode::WriteFailed, "{e:?}");
    assert!(e.message.contains("not a file"), "{}", e.message);
    assert!(t.elapsed().as_millis() < 1000, "no wait for a lock that can never be taken");
}

#[test]
fn undoing_the_first_change_leaves_a_folder_that_still_takes_writes() {
    let base = TempDir::new("cfgwrite");
    let dir = base.path().join("fresh");
    let plan = plan_of(None, None, &[height(40), set("color.accent", json!("#e5a50a"))]).unwrap();
    assert_eq!(plan.files.len(), 2);
    apply(&dir, &plan, &opts()).unwrap();
    undo(&dir, None, &opts()).unwrap();
    assert!(!dir.join("bar.toml").exists() && !dir.join("style.css").exists());
    let again = plan_of(None, None, &[height(44)]).unwrap();
    assert!(apply(&dir, &again, &opts()).is_ok(), "the folder is still one of ours");
}

#[test]
fn a_write_leaves_a_last_good_copy_to_restore_from() {
    let dir = folder(BAR, CSS);
    assert!(!has_last_good(dir.path(), ConfigFile::BarToml));
    apply_ok(dir.path(), &plan_of(Some(BAR), None, &[height(40)]).unwrap());
    assert!(has_last_good(dir.path(), ConfigFile::BarToml), "what was written loaded clean");
    assert!(!has_last_good(dir.path(), ConfigFile::StyleCss), "style.css was not written");
    let written = read(dir.path(), "bar.toml");
    std::fs::write(dir.path().join("bar.toml"), "config_version = 1\n[[bar]]\nheight = \n").unwrap();
    let r = restore(dir.path(), ConfigFile::BarToml, &opts()).unwrap();
    assert!(r.files[0].written);
    assert_eq!(read(dir.path(), "bar.toml"), written);
}

#[test]
fn an_undo_conflict_names_the_copy_in_error_copies() {
    let dir = folder(BAR, CSS);
    apply_ok(dir.path(), &plan_of(Some(BAR), None, &[height(40)]).unwrap());
    std::fs::write(dir.path().join("bar.toml"), "# by hand\n").unwrap();
    let e = undo(dir.path(), None, &opts()).unwrap_err();
    assert_eq!(e.code, ErrorCode::UndoConflict);
    assert_eq!(e.copies.len(), 1, "{e:?}");
    assert_eq!(std::fs::read_to_string(&e.copies[0]).unwrap(), BAR);
}

fn write_part(dir: &Path, n: u32, files: &[(ConfigFile, FileHash, FileHash)]) {
    let pd = dir.join(".undo").join(format!("{n}.part"));
    std::fs::create_dir_all(&pd).unwrap();
    let meta = store::Meta {
        n,
        time: "2026-11-27T10:00:00+11:00".into(),
        what: "test".into(),
        changes: Vec::new(),
        files: files.to_vec(),
    };
    std::fs::write(pd.join("change.json"), serde_json::to_vec(&meta.to_json()).unwrap()).unwrap();
    std::fs::write(pd.join("before.bar.toml"), BAR).unwrap();
}

#[test]
fn crash_after_entry_part_finished_or_dropped() {
    // A crash after the rename and before the entry was finished: the file is what the entry says.
    let dir = folder(BAR, CSS);
    let now = hash_of(dir.path(), "bar.toml");
    write_part(dir.path(), 5, &[(ConfigFile::BarToml, FileHash::of(b"old"), now)]);
    // A crash before the rename: the file is not what the entry says.
    write_part(dir.path(), 6, &[(ConfigFile::BarToml, now, FileHash::of(b"never written"))]);
    apply_ok(dir.path(), &plan_of(Some(BAR), None, &[height(40)]).unwrap());
    let undo_dir = dir.path().join(".undo");
    assert!(undo_dir.join("5").is_dir() && !undo_dir.join("5.part").exists(), "finished");
    assert!(!undo_dir.join("6.part").exists(), "dropped");
    let entries = log_entries(dir.path(), 9).unwrap();
    let n: Vec<u32> = entries.iter().map(|e| e.n).collect();
    assert_eq!(n, vec![6, 5], "the new entry is numbered above the highest entry that is left");
    assert_eq!(
        entries[0].what, "bar.toml: bar main: height 36 -> 40",
        "the dropped entry's number holds the new change"
    );
}

#[test]
fn an_entry_whose_undo_never_finished_is_dropped_by_the_next_write() {
    let dir = folder(BAR, CSS);
    let a = apply_ok(dir.path(), &plan_of(Some(BAR), None, &[height(40)]).unwrap());
    assert_eq!(a.undo, Some(1));
    // A crash during undo: the file went back and the entry stayed.
    std::fs::write(dir.path().join("bar.toml"), BAR).unwrap();
    apply_ok(dir.path(), &plan_of(Some(BAR), None, &[height(44)]).unwrap());
    let entries = log_entries(dir.path(), 9).unwrap();
    assert_eq!(entries.len(), 1, "entry 1 was already undone");
    assert_eq!(entries[0].what, "bar.toml: bar main: height 36 -> 44");
}

#[test]
fn undo_broken_copy_refused() {
    let dir = folder(BAR, CSS);
    apply_ok(dir.path(), &plan_of(Some(BAR), None, &[height(40)]).unwrap());
    let copy = dir.path().join(".undo").join("1").join("before.bar.toml");
    std::fs::write(&copy, "# changed under us\n").unwrap();
    let now = read(dir.path(), "bar.toml");
    let e = undo(dir.path(), None, &opts()).unwrap_err();
    assert_eq!(e.code, ErrorCode::UndoBroken);
    assert!(e.message.contains(".undo"), "{}", e.message);
    assert_eq!(read(dir.path(), "bar.toml"), now);
    // A change.json that cannot be read is the same.
    std::fs::write(dir.path().join(".undo").join("1").join("change.json"), "{").unwrap();
    assert_eq!(undo(dir.path(), None, &opts()).unwrap_err().code, ErrorCode::UndoBroken);
}

#[test]
fn undo_keeps_20() {
    let dir = folder(BAR, CSS);
    let mut text = BAR.to_string();
    for i in 0..25u32 {
        let plan = plan_of(Some(&text), None, &[height(16 + i)]).unwrap();
        apply_ok(dir.path(), &plan);
        text = read(dir.path(), "bar.toml");
    }
    let entries = log_entries(dir.path(), 100).unwrap();
    assert_eq!(entries.len(), 20);
    assert_eq!((entries[0].n, entries[19].n), (25, 6));
    assert!(dir.path().join(".undo").join("6").is_dir() && !dir.path().join(".undo").join("5").exists());
    assert_eq!(log_entries(dir.path(), 3).unwrap().len(), 3);
    let (done, parts) = numbers(&dir.path().join(".undo"));
    assert_eq!((done.len(), parts.len()), (20, 0));
}

// ---------------------------------------------------------------- restore

#[test]
fn restore_puts_last_good_back() {
    let dir = folder(BAR, CSS);
    assert!(!has_last_good(dir.path(), ConfigFile::BarToml));
    assert_eq!(restore(dir.path(), ConfigFile::BarToml, &opts()).unwrap_err().code, ErrorCode::NothingToRestore);
    assert!(keep_last_good(dir.path(), ConfigFile::BarToml, BAR.as_bytes()));
    assert!(!keep_last_good(dir.path(), ConfigFile::BarToml, BAR.as_bytes()), "the same text again writes nothing");
    assert!(has_last_good(dir.path(), ConfigFile::BarToml));
    // A hand edit breaks the file.
    let broken = "config_version = 1\n[[bar]]\nheight = \n";
    std::fs::write(dir.path().join("bar.toml"), broken).unwrap();
    let r = restore(dir.path(), ConfigFile::BarToml, &opts()).unwrap();
    assert_eq!(read(dir.path(), "bar.toml"), BAR);
    assert_eq!(r.undo, Some(1));
    let kept = r.kept_copy.expect("the broken text is kept");
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), broken);
    // Undo brings the broken text back.
    undo(dir.path(), None, &opts()).unwrap();
    assert_eq!(read(dir.path(), "bar.toml"), broken);
    // `expect` is honoured, and a file that already equals the copy is left alone.
    let mut o = opts();
    o.expect.set(ConfigFile::BarToml, FileHash::of(b"x"));
    assert_eq!(restore(dir.path(), ConfigFile::BarToml, &o).unwrap_err().code, ErrorCode::FileChanged);
    restore(dir.path(), ConfigFile::BarToml, &opts()).unwrap();
    let again = restore(dir.path(), ConfigFile::BarToml, &opts()).unwrap();
    assert!(again.undo.is_none() && !again.files[0].written);
}

#[test]
fn a_copy_that_does_not_load_is_not_restored() {
    let dir = folder(BAR, CSS);
    let undo_dir = dir.path().join(".undo");
    std::fs::create_dir(&undo_dir).unwrap();
    std::fs::write(undo_dir.join("last-good.bar.toml"), "height = \n").unwrap();
    std::fs::write(undo_dir.join("last-good.style.css.txt"), "bar { color: red \n").unwrap();
    let before = read(dir.path(), "bar.toml");
    for f in ConfigFile::ALL {
        assert_eq!(restore(dir.path(), f, &opts()).unwrap_err().code, ErrorCode::NothingToRestore, "{f}");
    }
    assert_eq!(read(dir.path(), "bar.toml"), before);
    // A style copy that parses comes back; a lost style file is made again.
    std::fs::write(undo_dir.join("last-good.style.css.txt"), CSS).unwrap();
    std::fs::remove_file(dir.path().join("style.css")).unwrap();
    let r = restore(dir.path(), ConfigFile::StyleCss, &opts()).unwrap();
    assert_eq!(read(dir.path(), "style.css"), CSS);
    assert!(r.kept_copy.is_none(), "there was nothing to keep");
    undo(dir.path(), None, &opts()).unwrap();
    assert!(!dir.path().join("style.css").exists());
}

#[test]
fn broken_style_does_not_block_bar_toml() {
    let dir = folder(BAR, "bar {\n  color: red\n");
    let plan = plan_of(Some(BAR), Some("bar {\n  color: red\n"), &[height(40)]).unwrap();
    assert!(apply(dir.path(), &plan, &opts()).is_ok());
    // And writing the broken file is refused with its line, leaving both files.
    let before = snapshot(dir.path());
    let e = plan_of(Some(BAR), Some("bar {\n  color: red\n"), &[set("color.accent", json!("#e5a50a"))]).unwrap_err();
    assert_eq!(e.code, ErrorCode::FileBroken);
    assert_eq!(snapshot(dir.path()), before);
}

#[test]
fn keep_last_good_waits_for_no_one() {
    let dir = folder(BAR, CSS);
    let undo_dir = dir.path().join(".undo");
    std::fs::create_dir(&undo_dir).unwrap();
    let held = win::open_lock(&undo_dir.join("lock")).unwrap();
    let t = Instant::now();
    assert!(!keep_last_good(dir.path(), ConfigFile::BarToml, BAR.as_bytes()));
    assert!(t.elapsed().as_millis() < 100, "no waiting for a write in progress");
    drop(held);
    assert!(keep_last_good(dir.path(), ConfigFile::BarToml, BAR.as_bytes()));
    assert!(!undo_dir.join("lock").exists());
    no_temps(dir.path());
    let too_big = vec![b'x'; MAX_SOURCE_BYTES + 1];
    assert!(!keep_last_good(dir.path(), ConfigFile::StyleCss, &too_big));
}

// ---------------------------------------------------------------- the log

#[test]
fn config_log_holds_tokens_only() {
    // The files carry text nobody validated; the log line must not.
    let secret = "SENTINEL-hunter2-C-Users-Someone";
    let bar = format!("{BAR}# {secret}\n");
    let dir = folder(&bar, &format!("{CSS}/* {secret} */\n"));
    let plan = plan_of(
        Some(&bar),
        Some(&format!("{CSS}/* {secret} */\n")),
        &[
            height(40),
            set("preset", json!("glass")),
            set("bar.main.right", json!(["status", "clock#utc", "my/weather"])),
            set("color.accent", json!("#e5a50a")),
        ],
    )
    .unwrap();
    let applied = apply_ok(dir.path(), &plan);
    let lines = change_lines(&plan, &applied);
    assert_eq!(lines.len(), 2);
    assert!(
        lines[0].starts_with("bar.toml: bar main: height 36 -> 40; preset default -> glass; bar main: right ")
            && lines[0].ends_with("(undo 1)"),
        "{}",
        lines[0]
    );
    assert_eq!(lines[1], "style.css: accent none -> #e5a50a (undo 1)");
    let allowed = |c: char| c.is_ascii_alphanumeric() || " _-./#:,;>()[]\"".contains(c);
    for l in lines.iter().chain(&undo_lines(1, &applied)).chain(&restore_lines(&applied)) {
        assert!(l.chars().all(allowed), "{l:?}");
        assert!(!l.contains(secret) && !l.contains("Users"), "{l}");
    }
    assert_eq!(undo_lines(1, &applied), vec!["bar.toml: undo 1 restored", "style.css: undo 1 restored"]);
    // Refusals: exit-2 codes are not logged; the others are, with the file and the code only.
    let e = SettingsError::new(ErrorCode::FileChanged, format!("{secret} changed")).with_file(ConfigFile::BarToml);
    assert_eq!(refusal_line(&e).as_deref(), Some("bar.toml: refused file_changed"));
    assert_eq!(refusal_line(&SettingsError::new(ErrorCode::BadValue, "x")), None);
    assert_eq!(refusal_line(&SettingsError::new(ErrorCode::BadUsage, "x")), None);
    assert_eq!(refusal_line(&SettingsError::new(ErrorCode::Busy, "x")).as_deref(), Some("settings: refused busy"));
    // The file: one line per call, the logger's own shape, and nothing over 10 MiB grows.
    let logs = TempDir::new("cfgwrite-logs");
    append_log_line(logs.path(), &lines[1]);
    append_log_line(logs.path(), &format!("config: {}", lines[1]));
    let text = std::fs::read_to_string(logs.path().join("rsb.log")).unwrap();
    assert_eq!(text.lines().count(), 2);
    for l in text.lines() {
        assert!(l.contains(" INFO config: style.css: accent none -> #e5a50a (undo 1)"), "{l}");
        assert!(!l.contains("config: config:"));
    }
    let big = logs.path().join("rsb.log");
    std::fs::OpenOptions::new().write(true).open(&big).unwrap().set_len(10 * 1024 * 1024 + 1).unwrap();
    append_log_line(logs.path(), "bar.toml: x");
    assert_eq!(std::fs::metadata(&big).unwrap().len(), 10 * 1024 * 1024 + 1, "over the limit: no line is added");
    let _ = Zone::Left;
}

#[test]
fn a_write_takes_milliseconds() {
    let dir = folder(BAR, CSS);
    let mut text = BAR.to_string();
    let mut ms: Vec<u128> = Vec::new();
    for i in 0..30u32 {
        let plan = plan_of(Some(&text), None, &[height(16 + i)]).unwrap();
        let t = Instant::now();
        apply_ok(dir.path(), &plan);
        ms.push(t.elapsed().as_micros());
        text = read(dir.path(), "bar.toml");
    }
    ms.sort_unstable();
    let session = if std::env::var("SESSIONNAME").is_ok_and(|s| s.starts_with("RDP")) {
        "remote desktop session"
    } else {
        "console session"
    };
    println!("apply one file: p50 {} us, p95 {} us (30 runs, debug build, {session})", ms[15], ms[28]);
    assert!(ms[28] < 10_000_000, "a write took over ten seconds");
}

#[test]
fn paths_in_messages_have_no_verbatim_prefix() {
    assert_eq!(shown_path(Path::new(r"\\?\C:\cfg\.undo\3")), r"C:\cfg\.undo\3");
    assert_eq!(shown_path(Path::new(r"\\?\UNC\server\share")), r"\\?\UNC\server\share");
    assert_eq!(shown_path(Path::new(r"C:\cfg")), r"C:\cfg");
    // A real broken entry names its folder without the prefix.
    let dir = folder(BAR, CSS);
    apply_ok(dir.path(), &plan_of(Some(BAR), None, &[height(40)]).unwrap());
    std::fs::write(dir.path().join(".undo").join("1").join("change.json"), "{").unwrap();
    let e = undo(dir.path(), None, &opts()).unwrap_err();
    assert!(!e.message.contains(r"\\?\"), "{}", e.message);
}

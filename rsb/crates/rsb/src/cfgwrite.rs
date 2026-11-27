//! The one place `bar.toml` and `style.css` are written.
//!
//! Every write takes a lock file, checks every file it opens on the open
//! handle (links, read-only, link count), refuses to replace a file someone
//! saved since it was read, writes every new text to a temporary file first
//! and only then replaces the files, one undo entry per request. A crash
//! leaves the old file or the new one, never half of either.
//!
//! The text of the change is made by `rsb_core::cfgedit`; this module only
//! moves bytes.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use rsb_core::cfgedit::Plan;
use rsb_core::contract::{ConfigFile, ErrorCode, Expect, FileHash, Notice, NoticeCode, Op, SettingsError};
use rsb_core::paths::{BAR_TOML, MAX_SOURCE_BYTES, STYLE_CSS};
use serde_json::Value;

mod store;
#[cfg(test)]
mod tests;
mod win;

/// The folder inside the config folder that holds undo entries, the lock and
/// the last good copies. The reader does not watch it.
pub const UNDO_DIR: &str = store::UNDO;

use store::{
    CHANGE, LOCK, UNDO, UNDOING, WRITTEN, before_name, entry_dir, last_good_name, next_number, numbers, part_dir,
    read_meta, read_small, read_written, written_get, written_json, written_set,
};

/// How one request is written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WriteOpts {
    /// Do every check and build the result, write nothing.
    pub dry_run: bool,
    /// Wait for the lock up to 2 s (the command line); otherwise a held lock
    /// is `busy` at once (the pipe thread never waits).
    pub wait_lock: bool,
    /// The versions the caller read, checked only for the files written.
    pub expect: Expect,
}

/// What happened to one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileOutcome {
    pub file: ConfigFile,
    /// The version the file has now (or would have, for a dry run).
    pub hash: FileHash,
    pub written: bool,
    /// The file differs from what settings last wrote there.
    pub edited_by_hand: bool,
}

/// The result of a write, undo or restore.
#[derive(Clone, Debug, PartialEq)]
pub struct Applied {
    /// Both files, `bar.toml` first.
    pub files: Vec<FileOutcome>,
    /// The undo entry this change made (None for an undo, a restore that
    /// made none, and a dry run).
    pub undo: Option<u32>,
    /// What the writer noticed: `hand_edit_kept`, `file_created`.
    pub notices: Vec<Notice>,
    /// Where the broken text of a restored file was kept.
    pub kept_copy: Option<String>,
}

/// One undoable change, newest first in `log_entries`.
#[derive(Clone, Debug, PartialEq)]
pub struct LogEntry {
    pub n: u32,
    /// Local time with its offset, `2026-11-27T10:15:00+11:00`.
    pub time: String,
    pub what: String,
    pub changes: Vec<Op>,
    pub files: Vec<(ConfigFile, FileHash, FileHash)>,
}

/// The last version settings wrote to each file (`.undo\written`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Written {
    pub bar_toml: Option<FileHash>,
    pub style_css: Option<FileHash>,
}

// ---------------------------------------------------------------- hooks (tests only)

#[cfg(test)]
type Hook = std::cell::RefCell<Option<Box<dyn FnMut()>>>;

#[cfg(test)]
thread_local! {
    /// Runs once everything is prepared and nothing is replaced yet.
    pub(crate) static AFTER_PREPARE: Hook = const { std::cell::RefCell::new(None) };
    /// Runs between the first and the second file's replace.
    pub(crate) static BETWEEN_COMMITS: Hook = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn run_hook(slot: &'static std::thread::LocalKey<Hook>) {
    let taken = slot.with(|h| h.borrow_mut().take());
    if let Some(mut f) = taken {
        f();
    }
}

#[cfg(not(test))]
fn run_hook<T>(_: T) {}

#[cfg(not(test))]
const AFTER_PREPARE: () = ();
#[cfg(not(test))]
const BETWEEN_COMMITS: () = ();

// ---------------------------------------------------------------- errors

fn fail(code: ErrorCode, file: Option<ConfigFile>, message: impl Into<String>, next: &str) -> SettingsError {
    let mut e = SettingsError::new(code, message);
    if let Some(f) = file {
        e = e.with_file(f);
    }
    if next.is_empty() { e } else { e.with_next(next) }
}

fn busy() -> SettingsError {
    fail(ErrorCode::Busy, None, "another settings change is being written", "try again")
}

fn elevated() -> SettingsError {
    fail(
        ErrorCode::Elevated,
        None,
        "RSB is running as administrator; settings are not changed from an administrator process",
        "start RSB without administrator rights",
    )
}

fn file_changed(file: ConfigFile) -> SettingsError {
    fail(
        ErrorCode::FileChanged,
        Some(file),
        format!("{file} changed since it was read"),
        "read the settings again, then make the change again",
    )
}

fn why(e: &io::Error) -> String {
    match e.raw_os_error() {
        Some(112) | Some(39) => "the disk is full".to_string(),
        Some(5) => "access was denied".to_string(),
        Some(32) | Some(33) => "the file is in use".to_string(),
        _ => e.to_string(),
    }
}

fn write_failed(file: Option<ConfigFile>, e: &io::Error) -> SettingsError {
    let place = file.map_or("the config folder".to_string(), |f| f.name().to_string());
    fail(
        ErrorCode::WriteFailed,
        file,
        format!("cannot write {place}: {}", why(e)),
        "check the config folder is writable and the disk has room",
    )
}

fn drive_failed(file: ConfigFile) -> SettingsError {
    fail(
        ErrorCode::WriteFailed,
        Some(file),
        format!("cannot write {file}: the config folder's drive does not allow safe saves"),
        "move the config folder to a local NTFS drive",
    )
}

fn linked(file: ConfigFile, what: &str) -> SettingsError {
    fail(ErrorCode::LinkedFile, Some(file), format!("{file} is {what}"), "change the file it points to by hand")
}

fn not_utf8(file: ConfigFile) -> SettingsError {
    fail(ErrorCode::NotUtf8, Some(file), format!("{file} is not UTF-8"), &format!("save {file} as UTF-8"))
}

fn not_config_folder(dir: &Path) -> SettingsError {
    fail(
        ErrorCode::BadUsage,
        None,
        format!("{} is not an RSB config folder", dir.display()),
        "pass the folder that holds bar.toml, or leave out --config-dir",
    )
}

// ---------------------------------------------------------------- folder and session

/// A path as a person reads it, without the verbatim prefix a canonical path carries.
pub fn shown_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\").filter(|r| !r.starts_with(r"UNC\")).unwrap_or(&s).to_string()
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        let s = abs.to_string_lossy().replace('/', "\\");
        s.trim_start_matches("\\\\?\\").trim_end_matches('\\').to_ascii_lowercase()
    };
    norm(a) == norm(b)
}

/// The default config folder, a folder that already holds one of the two
/// files, or (for a first change) a folder that does not exist yet.
pub fn is_config_folder(dir: &Path) -> bool {
    let named = |name: &str| dir.join(name).symlink_metadata().is_ok();
    named(BAR_TOML)
        || named(STYLE_CSS)
        || dir.join(UNDO).join(WRITTEN).is_file()
        || same_path(dir, &crate::paths::config_dir(None))
}

struct Session {
    dir: PathBuf,
    undo: PathBuf,
    /// Held so the `.undo` folder cannot be swapped for a link.
    _undo_dir: std::fs::File,
    _lock: std::fs::File,
    notices: Vec<Notice>,
    /// The folder did not exist and was made for this change.
    folder_created: bool,
    /// May sleep and retry when a file is briefly in use (not the pipe thread).
    patient: bool,
}

fn take_lock(undo: &Path, wait: bool) -> Result<std::fs::File, SettingsError> {
    let path = undo.join(LOCK);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match win::open_lock(&path) {
            Ok(f) => {
                let facts = win::facts(&f).map_err(|e| write_failed(None, &e))?;
                if facts.is_link() || facts.is_dir() {
                    return Err(fail(
                        ErrorCode::LinkedFile,
                        None,
                        "the settings lock file is a link",
                        "delete the .undo folder in the config folder by hand",
                    ));
                }
                return Ok(f);
            }
            Err(e) if matches!(e.raw_os_error(), Some(5 | 32 | 33)) => {
                if win::open_attrs(&path).and_then(|h| win::facts(&h)).is_ok_and(|f| f.is_dir() || f.is_link()) {
                    return Err(fail(
                        ErrorCode::WriteFailed,
                        None,
                        "the settings lock .undo\\lock is not a file",
                        "delete .undo\\lock in the config folder",
                    ));
                }
                if wait && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
                return Err(busy());
            }
            Err(e) => return Err(write_failed(None, &e)),
        }
    }
}

/// The start of a write: the process, the folder, `.undo`, the lock, and
/// what a crashed write left behind.
fn open_session(dir: &Path, opts: &WriteOpts, may_create: bool) -> Result<Session, SettingsError> {
    if win::is_elevated() {
        return Err(elevated());
    }
    let mut notices = Vec::new();
    let mut folder_created = false;
    match std::fs::metadata(dir) {
        Ok(m) if m.is_dir() => {
            if !is_config_folder(dir) {
                return Err(not_config_folder(dir));
            }
        }
        Ok(_) => return Err(not_config_folder(dir)),
        Err(e) if e.kind() == io::ErrorKind::NotFound && may_create => {
            crate::firstrun::prepare(dir).map_err(|m| {
                fail(
                    ErrorCode::WriteFailed,
                    None,
                    format!("cannot make the config folder: {m}"),
                    "check the config folder's drive is writable",
                )
            })?;
            folder_created = true;
            notices.push(Notice::new(
                NoticeCode::FileCreated,
                "the config folder did not exist; it was made with its starting files",
            ));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(not_config_folder(dir)),
        Err(e) => return Err(write_failed(None, &e)),
    }
    let dir = std::fs::canonicalize(dir).map_err(|e| write_failed(None, &e))?;
    let undo = dir.join(UNDO);
    match std::fs::create_dir(&undo) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(write_failed(None, &e)),
    }
    let undo_dir = win::open_dir(&undo).map_err(|e| write_failed(None, &e))?;
    let facts = win::facts(&undo_dir).map_err(|e| write_failed(None, &e))?;
    if facts.is_link() || !facts.is_dir() {
        return Err(fail(
            ErrorCode::LinkedFile,
            None,
            "the .undo folder is a link",
            "delete the .undo folder in the config folder by hand",
        ));
    }
    let lock = take_lock(&undo, opts.wait_lock)?;
    notices.extend(recover(&dir, &undo, opts.wait_lock));
    Ok(Session { dir, undo, _undo_dir: undo_dir, _lock: lock, notices, folder_created, patient: opts.wait_lock })
}

fn raw_hash(dir: &Path, file: ConfigFile) -> FileHash {
    FileHash::of_file(read_small(&dir.join(file.name())).ok().flatten().as_deref())
}

/// Where a file stands against an entry that records its two versions.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stands {
    After,
    Before,
    Neither,
}

fn stands(dir: &Path, file: ConfigFile, before: FileHash, after: FileHash) -> Stands {
    let now = raw_hash(dir, file);
    if now == after {
        Stands::After
    } else if now == before {
        Stands::Before
    } else {
        Stands::Neither
    }
}

/// The notice for a file that matches neither end of an unfinished change: it
/// is left as it is, and the copy from before the change is named.
fn left_as_it_is(file: ConfigFile, folder: &Path, before: FileHash) -> Notice {
    let n = Notice::new(
        NoticeCode::HandEditKept,
        format!("{file} was edited by hand while a change to it was being written; it is left as it is"),
    )
    .with_file(file);
    if before == FileHash::Missing {
        return n;
    }
    n.with_next(format!(
        "the file before that change is kept at {}; copy what you need from it",
        shown_path(&folder.join(before_name(file)))
    ))
}

fn not_rolled_back(file: ConfigFile, folder: &Path, why: &str) -> Vec<Notice> {
    vec![
        Notice::new(NoticeCode::HandEditKept, format!("{file}: an interrupted change could not be rolled back: {why}"))
            .with_file(file)
            .with_next(format!("the saved copies are in {}", shown_path(folder))),
    ]
}

/// Puts back every file of an unfinished change that is already at its new
/// version, from the entry's before copies, so the change is all or none.
/// Nothing is touched when a file matches neither version (a person saved it
/// since) or a copy is damaged; the notices say so and the entry stays.
fn roll_back_unfinished(dir: &Path, folder: &Path, meta: &store::Meta, patient: bool) -> Result<(), Vec<Notice>> {
    let states: Vec<Stands> = meta.files.iter().map(|(f, b, a)| stands(dir, *f, *b, *a)).collect();
    let mut notices: Vec<Notice> = Vec::new();
    for ((f, before, _), st) in meta.files.iter().zip(&states) {
        if *st == Stands::Neither {
            notices.push(left_as_it_is(*f, folder, *before));
        }
    }
    if !notices.is_empty() {
        return Err(notices);
    }
    let mut todo: Vec<(ConfigFile, Option<Vec<u8>>, FileHash, FileHash)> = Vec::new();
    for ((f, before, after), st) in meta.files.iter().zip(&states) {
        if *st != Stands::After || before == after {
            continue;
        }
        let raw = if *before == FileHash::Missing {
            None
        } else {
            let copy = read_small(&folder.join(before_name(*f))).ok().flatten();
            match copy.filter(|b| FileHash::of(b) == *before) {
                Some(b) => Some(b),
                None => return Err(not_rolled_back(*f, folder, "its saved copy is damaged")),
            }
        };
        todo.push((*f, raw, *before, *after));
    }
    for (f, raw, before, after) in todo.into_iter().rev() {
        let spec =
            ChangeSpec { cur: Current { raw, hash: before, ..empty(f, dir) }, new: New::Delete, before_missing: false };
        if let Err(e) = rollback_one(dir, &spec, after, patient) {
            return Err(not_rolled_back(f, folder, &e.message));
        }
        log::warn!(target: "config", "{f}: an interrupted change was rolled back");
    }
    Ok(())
}

/// Whatever a crashed write left: an entry that was never finished is
/// finished when every file already is what it says, dropped when no file is,
/// and rolled back when only some are; one that a person's save got in the way
/// of stays, with its copies, and a notice says so. An undo that was cut off
/// is finished the same way. Temporary files go. Returns the notices.
fn recover(dir: &Path, undo: &Path, patient: bool) -> Vec<Notice> {
    let (done, parts) = numbers(undo);
    let mut written = read_written(undo);
    let mut changed = false;
    let mut notices = Vec::new();
    for n in parts {
        let pd = part_dir(undo, n);
        let Some(meta) = read_meta(&pd) else {
            store::remove_entry(&pd);
            continue;
        };
        let states: Vec<Stands> = meta.files.iter().map(|(f, b, a)| stands(dir, *f, *b, *a)).collect();
        if states.iter().all(|s| *s == Stands::After) {
            if std::fs::rename(&pd, entry_dir(undo, n)).is_ok() {
                for (f, _, after) in &meta.files {
                    written_set(&mut written, *f, *after);
                }
                changed = true;
            } else {
                store::remove_entry(&pd);
            }
        } else if states.iter().all(|s| *s == Stands::Before) {
            store::remove_entry(&pd);
        } else {
            match roll_back_unfinished(dir, &pd, &meta, patient) {
                Ok(()) => store::remove_entry(&pd),
                Err(n) => notices.extend(n),
            }
        }
    }
    if let Some(newest) = done.last().copied() {
        let folder = entry_dir(undo, newest);
        if let Some(meta) = read_meta(&folder) {
            let marker = folder.join(UNDOING);
            let marked = read_small(&marker).ok().flatten().is_some();
            let all_before = meta.files.iter().all(|(f, before, _)| raw_hash(dir, *f) == *before);
            if all_before && meta.files.iter().any(|(_, before, after)| before != after) {
                for (f, before, _) in &meta.files {
                    written_set(&mut written, *f, *before);
                }
                store::remove_entry(&folder);
                changed = true;
            } else if marked && meta.files.iter().all(|(f, b, a)| stands(dir, *f, *b, *a) == Stands::After) {
                // The undo died before it replaced anything: nothing to finish.
                let _ = std::fs::remove_file(&marker);
            } else if marked {
                // An undo that was cut off: finish it, unless a save got in the way.
                match roll_back_unfinished(dir, &folder, &meta, patient) {
                    Ok(()) if meta.files.iter().all(|(f, b, _)| raw_hash(dir, *f) == *b) => {
                        for (f, before, _) in &meta.files {
                            written_set(&mut written, *f, *before);
                        }
                        store::remove_entry(&folder);
                        changed = true;
                    }
                    Ok(()) => {
                        let _ = std::fs::remove_file(&marker);
                    }
                    Err(n) => {
                        let _ = std::fs::remove_file(&marker);
                        notices.extend(n);
                    }
                }
            }
        }
    }
    if changed {
        let _ = write_json(undo, WRITTEN, &written_json(&written));
    }
    for folder in [dir, undo] {
        let Ok(read) = std::fs::read_dir(folder) else { continue };
        for e in read.flatten() {
            let Ok(name) = e.file_name().into_string() else { continue };
            if name.starts_with('.') && name.ends_with(".tmp") && name.contains(".rsb-") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    notices
}

// ---------------------------------------------------------------- reading a file

/// A file as it stands under the lock.
struct Current {
    file: ConfigFile,
    path: PathBuf,
    handle: Option<std::fs::File>,
    ident: Option<win::Ident>,
    raw: Option<Vec<u8>>,
    hash: FileHash,
}

fn open_current(dir: &Path, file: ConfigFile, patient: bool) -> Result<Current, SettingsError> {
    let path = dir.join(file.name());
    let mut tries = 0;
    let max_tries = if patient { 3 } else { 0 };
    let handle = loop {
        match win::open_read(&path) {
            Ok(f) => break Some(f),
            Err(e) if e.kind() == io::ErrorKind::NotFound => break None,
            Err(e) if matches!(e.raw_os_error(), Some(32 | 33)) && tries < max_tries => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) if matches!(e.raw_os_error(), Some(5 | 32 | 33)) => return Err(busy()),
            Err(e) => return Err(write_failed(Some(file), &e)),
        }
    };
    let Some(handle) = handle else {
        return Ok(Current { file, path, handle: None, ident: None, raw: None, hash: FileHash::Missing });
    };
    let facts = win::facts(&handle).map_err(|e| write_failed(Some(file), &e))?;
    if facts.is_link() {
        return Err(linked(file, "a link to another file"));
    }
    if facts.is_dir() {
        return Err(linked(file, "a folder"));
    }
    if facts.links > 1 {
        return Err(linked(file, "linked under more than one name"));
    }
    if facts.is_read_only() {
        return Err(fail(
            ErrorCode::ReadOnly,
            Some(file),
            format!("{file} is read-only"),
            "clear that in its Properties",
        ));
    }
    let mut raw = Vec::new();
    (&handle).take(MAX_SOURCE_BYTES as u64 + 1).read_to_end(&mut raw).map_err(|e| write_failed(Some(file), &e))?;
    if raw.len() > MAX_SOURCE_BYTES {
        return Err(fail(
            ErrorCode::WriteFailed,
            Some(file),
            format!("{file} is larger than 1 MiB; it is not changed here"),
            "make the file smaller by hand",
        ));
    }
    let hash = FileHash::of(&raw);
    Ok(Current { file, path, handle: Some(handle), ident: Some(facts.ident), raw: Some(raw), hash })
}

/// The text of a file's bytes and whether it carried a byte order mark.
/// UTF-16 and invalid UTF-8 are refused, never read lossily.
fn text_of(raw: &[u8], file: ConfigFile) -> Result<(bool, &str), SettingsError> {
    if raw.starts_with(&[0xff, 0xfe]) || raw.starts_with(&[0xfe, 0xff]) {
        return Err(not_utf8(file));
    }
    let (bom, body) = match raw.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        Some(b) => (true, b),
        None => (false, raw),
    };
    std::str::from_utf8(body).map(|t| (bom, t)).map_err(|_| not_utf8(file))
}

/// The bytes of one planned file once it is checked against what is on disk, and
/// whether it was hand-edited since settings last wrote it.
fn check_planned(
    cur: &Current,
    fp: &rsb_core::cfgedit::FilePlan,
    opts: &WriteOpts,
    written: &Written,
    folder_created: bool,
    notices: &mut Vec<Notice>,
) -> Result<Vec<u8>, SettingsError> {
    let file = fp.file;
    let mut bom = false;
    match (&cur.raw, fp.base) {
        (None, FileHash::Missing) => {}
        (None, _) => return Err(file_changed(file)),
        (Some(_), FileHash::Missing) if folder_created => {}
        (Some(_), FileHash::Missing) => return Err(file_changed(file)),
        (Some(raw), base) => {
            let (has_bom, text) = text_of(raw, file)?;
            bom = has_bom;
            if FileHash::of(text.as_bytes()) != base {
                return Err(file_changed(file));
            }
        }
    }
    let seen = if folder_created { FileHash::Missing } else { cur.hash };
    match opts.expect.get(file) {
        Some(h) if h != seen => return Err(file_changed(file)),
        Some(_) => {}
        None => {
            if let Some(w) = written_get(written, file)
                && !folder_created
                && cur.raw.is_some()
                && w != cur.hash
                && let Some(n) = hand_edit_notice(cur, w)
            {
                notices.push(n);
            }
        }
    }
    let mut bytes = Vec::with_capacity(fp.after.len() + 3);
    if bom {
        bytes.extend_from_slice(&[0xef, 0xbb, 0xbf]);
    }
    bytes.extend_from_slice(fp.after.as_bytes());
    Ok(bytes)
}

/// The notice for a file that differs from what settings last wrote, or none when the whole difference lies inside
/// the colours block, which the change replaces anyway (its own notice says so). `style.css` is only ever written
/// by an accent change, which rewrites that block, so there the notice says which edits stay.
fn hand_edit_notice(cur: &Current, written: FileHash) -> Option<Notice> {
    let file = cur.file;
    let mut message = format!("{file} was edited by hand since the last settings change; that edit is kept");
    if file == ConfigFile::StyleCss
        && let Some(text) = cur.raw.as_deref().and_then(|raw| text_of(raw, file).ok()).map(|(_, t)| t)
        && rsb_core::cfgedit::block_lines(text).is_some()
    {
        if edit_is_inside_the_block(cur, text, written) {
            return None;
        }
        message = format!(
            "{file} was edited by hand since the last settings change; edits outside the colours block are kept"
        );
    }
    Some(Notice::new(NoticeCode::HandEditKept, message).with_file(file))
}

/// Whether the lines that differ between the copy settings last wrote and `text` are all between the two marker
/// lines of the colours block. False whenever that cannot be shown (no copy of what was written, or a copy that is
/// not it).
fn edit_is_inside_the_block(cur: &Current, text: &str, written: FileHash) -> bool {
    let Some(undo) = cur.path.parent().map(|d| d.join(UNDO)) else { return false };
    let Some(old) = read_small(&undo.join(last_good_name(cur.file))).ok().flatten() else { return false };
    if FileHash::of(&old) != written {
        return false;
    }
    let Ok((_, old)) = text_of(&old, cur.file) else { return false };
    let Some((first, last)) = rsb_core::cfgedit::block_lines(text) else { return false };
    let (old, new): (Vec<&str>, Vec<&str>) = (old.lines().collect(), text.lines().collect());
    let shared = old.len().min(new.len());
    let head = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let tail = old.iter().rev().zip(new.iter().rev()).take(shared - head).take_while(|(a, b)| a == b).count();
    // The differing lines of the new text are head+1 ..= new.len()-tail (1-based); both marker lines stay outside.
    let (from, to) = (head as u32 + 1, (new.len() - tail) as u32);
    old != new && from > first && to < last
}

// ---------------------------------------------------------------- writing

static TEMP_SEQ: AtomicU32 = AtomicU32::new(0);

fn write_temp(dir: &Path, file: ConfigFile, bytes: &[u8]) -> Result<PathBuf, SettingsError> {
    for _ in 0..16 {
        let name =
            format!(".{}.rsb-{}-{}.tmp", file.name(), std::process::id(), TEMP_SEQ.fetch_add(1, Ordering::Relaxed));
        let path = dir.join(name);
        match win::create_new(&path) {
            Ok(mut f) => {
                if let Err(e) = f.write_all(bytes).and_then(|()| f.sync_all()) {
                    drop(f);
                    let _ = std::fs::remove_file(&path);
                    return Err(write_failed(Some(file), &e));
                }
                return Ok(path);
            }
            // A name that is taken (a link included) is never followed: the next one is tried.
            Err(e) if matches!(e.kind(), io::ErrorKind::AlreadyExists) || e.raw_os_error() == Some(80) => continue,
            Err(e) => return Err(write_failed(Some(file), &e)),
        }
    }
    Err(write_failed(Some(file), &io::Error::other("no free temporary name")))
}

/// A small file made whole: written beside, then renamed over.
fn write_json(folder: &Path, name: &str, value: &Value) -> io::Result<()> {
    write_json_bytes(folder, name, &serde_json::to_vec(value).map_err(io::Error::other)?)
}

fn write_json_bytes(folder: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let tmp =
        folder.join(format!(".{name}.rsb-{}-{}.tmp", std::process::id(), TEMP_SEQ.fetch_add(1, Ordering::Relaxed)));
    let mut f = win::create_new(&tmp)?;
    let done = f.write_all(bytes).and_then(|()| f.sync_all());
    drop(f);
    if let Err(e) = done {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, folder.join(name)).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// A new file inside an entry folder, written and synced.
fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut f = win::create_new(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// What a write puts in place of a file.
enum New {
    Bytes(Vec<u8>),
    Delete,
}

struct ChangeSpec {
    cur: Current,
    new: New,
    /// What the undo entry records as the version before: `none` when the
    /// file did not exist, or settings made it for this very change.
    before_missing: bool,
}

enum Entry<'a> {
    /// A new undo entry.
    New { what: String, changes: &'a [Op] },
    /// Undo: this entry goes when the files are back.
    Pop(u32),
}

struct Done {
    n: Option<u32>,
    after: Vec<(ConfigFile, FileHash)>,
    entry_path: Option<PathBuf>,
}

fn now_text() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let offset = rsb_core::time::Clock::now(&rsb_services::clock::SystemClock, rsb_core::time::TimeZone::Local);
    let stamp = crate::logger::timestamp_at(now, offset.utc_offset_min);
    // Seconds are enough here: drop the milliseconds.
    match stamp.find('.') {
        Some(dot) => format!("{}{}", &stamp[..dot], &stamp[dot + 4..]),
        None => stamp,
    }
}

/// Replaces one file with the temp file (or deletes it), after checking that
/// nobody saved it since it was read.
fn commit_one(spec: &mut ChangeSpec, temp: Option<&Path>, patient: bool) -> Result<(), SettingsError> {
    let file = spec.cur.file;
    let max_tries = if patient { 5 } else { 0 };
    let path = spec.cur.path.clone();
    match (&spec.cur.handle, spec.cur.ident) {
        (Some(handle), Some(ident)) => {
            // An editor that saved by rename since our read left our handle
            // on the old file and a new file behind the name.
            let now = match win::open_attrs(&path) {
                Ok(f) => win::facts(&f).map_err(|e| write_failed(Some(file), &e))?,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(file_changed(file)),
                Err(e) => return Err(write_failed(Some(file), &e)),
            };
            if now.ident != ident || win::delete_pending(handle).map_err(|e| write_failed(Some(file), &e))? {
                return Err(file_changed(file));
            }
            let Some(temp) = temp else {
                return std::fs::remove_file(&path).map_err(|e| write_failed(Some(file), &e));
            };
            let mut tries = 0;
            loop {
                match std::fs::rename(temp, &path) {
                    Ok(()) => return Ok(()),
                    Err(e) if matches!(e.raw_os_error(), Some(5 | 32)) && tries < max_tries => {
                        tries += 1;
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(e) if matches!(e.raw_os_error(), Some(5 | 32)) => break,
                    Err(e) => return Err(write_failed(Some(file), &e)),
                }
            }
            // A drive without safe renames over an open file: close ours,
            // look again, and try once more.
            spec.cur.handle = None;
            let again = read_small(&path).ok().flatten();
            if FileHash::of_file(again.as_deref()) != spec.cur.hash {
                return Err(file_changed(file));
            }
            std::fs::rename(temp, &path).map_err(|_| drive_failed(file))
        }
        _ => match temp {
            // The file did not exist: the move must not replace one made since.
            Some(temp) => win::move_no_replace(temp, &path).map_err(|e| {
                if matches!(e.raw_os_error(), Some(80 | 183)) {
                    file_changed(file)
                } else {
                    write_failed(Some(file), &e)
                }
            }),
            None => Ok(()),
        },
    }
}

fn cleanup(temps: &[Option<PathBuf>]) {
    for t in temps.iter().flatten() {
        let _ = std::fs::remove_file(t);
    }
}

/// Takes the undo marker out of an entry whose undo did not go ahead.
fn unmark(entry: &Entry<'_>, undo: &Path) {
    if let Entry::Pop(num) = entry {
        let _ = std::fs::remove_file(entry_dir(undo, *num).join(UNDOING));
    }
}

/// The body of a write: temp files, the undo entry, the replaces in order with
/// rollback, the entry finished, `.undo\written` set, old entries pruned.
fn write_changes(s: &mut Session, mut specs: Vec<ChangeSpec>, entry: Entry<'_>) -> Result<Done, SettingsError> {
    // Every new text goes to disk beside its file first.
    let mut temps: Vec<Option<PathBuf>> = Vec::new();
    for spec in &specs {
        match &spec.new {
            New::Bytes(b) => match write_temp(&s.dir, spec.cur.file, b) {
                Ok(p) => temps.push(Some(p)),
                Err(e) => {
                    cleanup(&temps);
                    return Err(e);
                }
            },
            New::Delete => temps.push(None),
        }
    }
    let after_hash = |spec: &ChangeSpec| match &spec.new {
        New::Bytes(b) => FileHash::of(b),
        New::Delete => FileHash::Missing,
    };
    // The undo entry, as `.part` until the last replace is done.
    let mut part: Option<(u32, PathBuf)> = None;
    if let Entry::New { what, changes } = &entry {
        let Some(n) = next_number(&s.undo) else {
            cleanup(&temps);
            return Err(fail(
                ErrorCode::WriteFailed,
                None,
                "there are too many undo entries",
                "delete the numbered folders in .undo in the config folder",
            ));
        };
        let pd = part_dir(&s.undo, n);
        let made = (|| -> io::Result<()> {
            std::fs::create_dir(&pd)?;
            for spec in &specs {
                if let (Some(raw), false) = (&spec.cur.raw, spec.before_missing) {
                    write_new(&pd.join(before_name(spec.cur.file)), raw)?;
                }
            }
            let meta = store::Meta {
                n,
                time: now_text(),
                what: what.clone(),
                changes: changes.to_vec(),
                files: specs
                    .iter()
                    .map(|sp| {
                        (sp.cur.file, if sp.before_missing { FileHash::Missing } else { sp.cur.hash }, after_hash(sp))
                    })
                    .collect(),
            };
            write_new(&pd.join(CHANGE), &serde_json::to_vec(&meta.to_json()).map_err(io::Error::other)?)
        })();
        if let Err(e) = made {
            store::remove_entry(&pd);
            cleanup(&temps);
            return Err(write_failed(None, &e));
        }
        part = Some((n, pd));
    }
    if let Entry::Pop(num) = &entry {
        let marker = entry_dir(&s.undo, *num).join(UNDOING);
        let _ = std::fs::remove_file(&marker);
        if let Err(e) = write_new(&marker, b"") {
            cleanup(&temps);
            return Err(write_failed(None, &e));
        }
    }
    run_hook(&AFTER_PREPARE);
    // Replace the files, in order.
    let mut committed: Vec<usize> = Vec::new();
    for i in 0..specs.len() {
        let result = commit_one(&mut specs[i], temps[i].as_deref(), s.patient);
        if let Err(mut e) = result {
            cleanup(&temps);
            if committed.is_empty() {
                if let Some((_, pd)) = &part {
                    store::remove_entry(pd);
                }
                unmark(&entry, &s.undo);
                return Err(e);
            }
            let mut copies: Vec<String> = Vec::new();
            let mut all_back = true;
            for &k in committed.iter().rev() {
                if rollback_one(&s.dir, &specs[k], after_hash(&specs[k]), s.patient).is_err() {
                    all_back = false;
                }
            }
            if all_back {
                if let Some((_, pd)) = &part {
                    store::remove_entry(pd);
                }
                unmark(&entry, &s.undo);
            } else if let Some((_, pd)) = &part {
                for sp in &specs {
                    let p = pd.join(before_name(sp.cur.file));
                    if p.exists() {
                        copies.push(shown_path(&p));
                    }
                }
                e = e.with_copies(copies);
            }
            return Err(e);
        }
        committed.push(i);
        if i == 0 {
            run_hook(&BETWEEN_COMMITS);
        }
    }
    // The entry is final; the record of what settings wrote is updated.
    let mut n = None;
    let mut entry_path = None;
    match (&entry, part) {
        (Entry::New { .. }, Some((num, pd))) => {
            let final_dir = entry_dir(&s.undo, num);
            match std::fs::rename(&pd, &final_dir) {
                Ok(()) => {
                    n = Some(num);
                    entry_path = Some(final_dir);
                }
                Err(e) => log::warn!(target: "config", "the undo entry could not be finished: {}", why(&e)),
            }
        }
        (Entry::Pop(num), _) => store::remove_entry(&entry_dir(&s.undo, *num)),
        _ => {}
    }
    let mut written = read_written(&s.undo);
    let after: Vec<(ConfigFile, FileHash)> = specs.iter().map(|sp| (sp.cur.file, after_hash(sp))).collect();
    for (f, h) in &after {
        written_set(&mut written, *f, *h);
    }
    if let Err(e) = write_json(&s.undo, WRITTEN, &written_json(&written)) {
        log::warn!(target: "config", "the written versions could not be saved: {}", why(&e));
    }
    store::prune(&s.undo);
    // What was just written was parsed and checked: it is the newest text known to load.
    for sp in &specs {
        if let New::Bytes(b) = &sp.new
            && loads_clean(sp.cur.file, b)
        {
            let _ = put_last_good(&s.undo, sp.cur.file, b);
        }
    }
    Ok(Done { n, after, entry_path })
}

/// Puts a file back as it was before a request, when it is still what the
/// request wrote.
fn rollback_one(dir: &Path, spec: &ChangeSpec, wrote: FileHash, patient: bool) -> Result<(), SettingsError> {
    let now = open_current(dir, spec.cur.file, patient)?;
    if now.hash != wrote {
        return Err(file_changed(spec.cur.file));
    }
    match &spec.cur.raw {
        Some(before) => {
            let temp = write_temp(dir, spec.cur.file, before)?;
            let mut change = ChangeSpec { cur: now, new: New::Delete, before_missing: false };
            let r = commit_one(&mut change, Some(&temp), patient);
            let _ = std::fs::remove_file(&temp);
            r
        }
        None => {
            let mut change = ChangeSpec { cur: now, new: New::Delete, before_missing: false };
            commit_one(&mut change, None, patient)
        }
    }
}

fn empty(file: ConfigFile, dir: &Path) -> Current {
    Current { file, path: dir.join(file.name()), handle: None, ident: None, raw: None, hash: FileHash::Missing }
}

// ---------------------------------------------------------------- results

fn peek(dir: &Path, file: ConfigFile) -> FileHash {
    raw_hash(dir, file)
}

fn outcomes(
    dir: &Path,
    written: &Written,
    done: &[(ConfigFile, FileHash)],
    planned_only: &[(ConfigFile, FileHash)],
) -> Vec<FileOutcome> {
    ConfigFile::ALL
        .iter()
        .map(|&file| {
            if let Some((_, h)) = done.iter().find(|(f, _)| *f == file) {
                return FileOutcome { file, hash: *h, written: true, edited_by_hand: false };
            }
            if let Some((_, h)) = planned_only.iter().find(|(f, _)| *f == file) {
                return FileOutcome { file, hash: *h, written: false, edited_by_hand: false };
            }
            let hash = peek(dir, file);
            let edited_by_hand = written_get(written, file).is_some_and(|w| w != hash);
            FileOutcome { file, hash, written: false, edited_by_hand }
        })
        .collect()
}

/// Writes `plan` into the config folder `dir`. Each file is read again under
/// the lock and must still have the version the plan was made against
/// (`FilePlan::base`), else `file_changed` and nothing is written. Refuses an
/// elevated process, a folder that is not a config folder, a link, a
/// read-only file, a file that is not UTF-8.
pub fn apply(dir: &Path, plan: &Plan, opts: &WriteOpts) -> Result<Applied, SettingsError> {
    if opts.dry_run {
        return dry_run(dir, plan, opts);
    }
    if plan.files.is_empty() {
        if win::is_elevated() {
            return Err(elevated());
        }
        let written = read_written(&dir.join(UNDO));
        return Ok(Applied {
            files: outcomes(dir, &written, &[], &[]),
            undo: None,
            notices: Vec::new(),
            kept_copy: None,
        });
    }
    let mut s = open_session(dir, opts, true)?;
    let written = read_written(&s.undo);
    let mut notices = std::mem::take(&mut s.notices);
    let mut specs = Vec::new();
    for fp in &plan.files {
        let cur = open_current(&s.dir, fp.file, opts.wait_lock)?;
        let bytes = check_planned(&cur, fp, opts, &written, s.folder_created, &mut notices)?;
        let before_missing = cur.raw.is_none() || s.folder_created;
        specs.push(ChangeSpec { cur, new: New::Bytes(bytes), before_missing });
    }
    let what = plan.files.iter().map(|f| format!("{}: {}", f.file, f.what)).collect::<Vec<_>>().join("; ");
    let done = write_changes(&mut s, specs, Entry::New { what, changes: &plan.changes })?;
    let files = outcomes(&s.dir, &written_after(&s.undo), &done.after, &[]);
    Ok(Applied { files, undo: done.n, notices, kept_copy: None })
}

fn written_after(undo: &Path) -> Written {
    read_written(undo)
}

/// Every check and the same reply, nothing written, no lock, no folder made.
fn dry_run(dir: &Path, plan: &Plan, opts: &WriteOpts) -> Result<Applied, SettingsError> {
    if win::is_elevated() {
        return Err(elevated());
    }
    let mut notices = Vec::new();
    let exists = match std::fs::metadata(dir) {
        Ok(m) if m.is_dir() => {
            if !is_config_folder(dir) {
                return Err(not_config_folder(dir));
            }
            true
        }
        Ok(_) => return Err(not_config_folder(dir)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            notices.push(Notice::new(
                NoticeCode::FileCreated,
                "the config folder did not exist; it would be made with its starting files",
            ));
            false
        }
        Err(e) => return Err(write_failed(None, &e)),
    };
    let written = if exists { read_written(&dir.join(UNDO)) } else { Written::default() };
    let mut planned = Vec::new();
    for fp in &plan.files {
        let cur = if exists { open_current(dir, fp.file, opts.wait_lock)? } else { empty(fp.file, dir) };
        let bytes = check_planned(&cur, fp, opts, &written, !exists, &mut notices)?;
        planned.push((fp.file, FileHash::of(&bytes)));
    }
    Ok(Applied { files: outcomes(dir, &written, &[], &planned), undo: None, notices, kept_copy: None })
}

// ---------------------------------------------------------------- undo and restore

fn undo_specs(
    dir: &Path,
    undo_dir: &Path,
    n: Option<u32>,
    opts: &WriteOpts,
) -> Result<(u32, Vec<ChangeSpec>), SettingsError> {
    let (done, _) = numbers(undo_dir);
    let Some(&newest) = done.last() else {
        return Err(fail(ErrorCode::NothingToUndo, None, "no settings change to undo", ""));
    };
    if let Some(n) = n
        && n != newest
    {
        return Err(fail(
            ErrorCode::UndoNotLatest,
            None,
            format!("change {n} is not the newest ({newest} is)"),
            "undo the newer change first, or read the list with rsb settings log",
        )
        .with_fix(rsb_core::contract::fixes::undo(newest)));
    }
    let entry = entry_dir(undo_dir, newest);
    let broken = || {
        fail(
            ErrorCode::UndoBroken,
            None,
            format!("the saved copy for change {newest} is damaged ({})", shown_path(&entry)),
            "change the file by hand; rsb settings log lists the other changes",
        )
    };
    let meta = read_meta(&entry).ok_or_else(broken)?;
    let mut specs = Vec::new();
    for (file, before, after) in &meta.files {
        let cur = open_current(dir, *file, opts.wait_lock)?;
        if cur.hash != *after {
            let kept = entry.join(before_name(*file));
            return Err(fail(
                ErrorCode::UndoConflict,
                Some(*file),
                format!("{file} was edited after the last change; undo would lose that edit"),
                &format!(
                    "the file before the change is kept at {}; copy what you need, or run rsb settings restore {file}",
                    shown_path(&kept)
                ),
            )
            .with_copies(if kept.exists() { vec![shown_path(&kept)] } else { Vec::new() })
            .with_fix(if kept.exists() {
                rsb_core::contract::fixes::open(&shown_path(&kept))
            } else {
                rsb_core::contract::fixes::restore(*file)
            }));
        }
        if let Some(h) = opts.expect.get(*file)
            && h != cur.hash
        {
            return Err(file_changed(*file));
        }
        let new = if *before == FileHash::Missing {
            New::Delete
        } else {
            let bytes = read_small(&entry.join(before_name(*file))).ok().flatten().ok_or_else(broken)?;
            if FileHash::of(&bytes) != *before {
                return Err(broken());
            }
            New::Bytes(bytes)
        };
        specs.push(ChangeSpec { cur, new, before_missing: false });
    }
    Ok((newest, specs))
}

/// Puts back the files of change `n` (the newest when None). The current
/// files must still be what that change wrote.
pub fn undo(dir: &Path, n: Option<u32>, opts: &WriteOpts) -> Result<Applied, SettingsError> {
    if opts.dry_run {
        if win::is_elevated() {
            return Err(elevated());
        }
        if !is_config_folder(dir) || !dir.is_dir() {
            return Err(not_config_folder(dir));
        }
        let undo_dir = dir.join(UNDO);
        let (_, specs) = undo_specs(dir, &undo_dir, n, opts)?;
        let written = read_written(&undo_dir);
        let planned: Vec<(ConfigFile, FileHash)> = specs
            .iter()
            .map(|sp| {
                (
                    sp.cur.file,
                    match &sp.new {
                        New::Bytes(b) => FileHash::of(b),
                        New::Delete => FileHash::Missing,
                    },
                )
            })
            .collect();
        return Ok(Applied {
            files: outcomes(dir, &written, &[], &planned),
            undo: None,
            notices: Vec::new(),
            kept_copy: None,
        });
    }
    let mut s = open_session(dir, opts, false)?;
    let (newest, specs) = undo_specs(&s.dir, &s.undo, n, opts)?;
    let done = write_changes(&mut s, specs, Entry::Pop(newest))?;
    let written = read_written(&s.undo);
    Ok(Applied {
        files: outcomes(&s.dir, &written, &done.after, &[]),
        undo: None,
        notices: Vec::new(),
        kept_copy: None,
    })
}

/// Whether the bytes of a last-good copy still load clean.
fn loads_clean(file: ConfigFile, bytes: &[u8]) -> bool {
    let Ok((_, text)) = text_of(bytes, file) else { return false };
    match file {
        ConfigFile::BarToml => {
            let preset = |name: &str| crate::assets::preset_toml(name).map(str::to_string);
            let load = rsb_core::config::load_config(Some((text, BAR_TOML)), &preset);
            load.config.is_some()
        }
        ConfigFile::StyleCss => {
            let (_, diags) = rsb_core::css::Stylesheet::parse(text, STYLE_CSS);
            diags.iter().all(|d| d.severity != rsb_core::diag::Severity::Error)
        }
    }
}

/// Replaces `file` with the last copy of it that loaded clean. The text it
/// replaces becomes the before copy of a new undo entry.
pub fn restore(dir: &Path, file: ConfigFile, opts: &WriteOpts) -> Result<Applied, SettingsError> {
    let nothing = || {
        fail(
            ErrorCode::NothingToRestore,
            Some(file),
            format!("no copy of {file} that loaded clean is kept yet"),
            "fix the line named in rsb settings get by hand",
        )
    };
    if opts.dry_run {
        if win::is_elevated() {
            return Err(elevated());
        }
        if !is_config_folder(dir) || !dir.is_dir() {
            return Err(not_config_folder(dir));
        }
        let undo_dir = dir.join(UNDO);
        let good = read_small(&undo_dir.join(last_good_name(file)))
            .ok()
            .flatten()
            .filter(|b| loads_clean(file, b))
            .ok_or_else(nothing)?;
        let written = read_written(&undo_dir);
        let planned = [(file, FileHash::of(&good))];
        return Ok(Applied {
            files: outcomes(dir, &written, &[], &planned),
            undo: None,
            notices: Vec::new(),
            kept_copy: None,
        });
    }
    let mut s = open_session(dir, opts, false)?;
    let good = read_small(&s.undo.join(last_good_name(file)))
        .ok()
        .flatten()
        .filter(|b| loads_clean(file, b))
        .ok_or_else(nothing)?;
    let cur = open_current(&s.dir, file, opts.wait_lock)?;
    if let Some(h) = opts.expect.get(file)
        && h != cur.hash
    {
        return Err(file_changed(file));
    }
    if cur.hash == FileHash::of(&good) {
        let written = read_written(&s.undo);
        return Ok(Applied {
            files: outcomes(&s.dir, &written, &[], &[]),
            undo: None,
            notices: Vec::new(),
            kept_copy: None,
        });
    }
    let before_missing = cur.raw.is_none();
    let specs = vec![ChangeSpec { cur, new: New::Bytes(good), before_missing }];
    let changes = [Op::Restore { file }];
    let what = format!("{file}: restored from the last good copy");
    let done = write_changes(&mut s, specs, Entry::New { what, changes: &changes })?;
    let kept_copy = done.entry_path.map(|p| p.join(before_name(file))).filter(|p| p.exists()).map(|p| shown_path(&p));
    let written = read_written(&s.undo);
    Ok(Applied { files: outcomes(&s.dir, &written, &done.after, &[]), undo: done.n, notices: Vec::new(), kept_copy })
}

/// The undoable changes, newest first, at most `last` of them.
pub fn log_entries(dir: &Path, last: usize) -> Result<Vec<LogEntry>, SettingsError> {
    let undo_dir = dir.join(UNDO);
    let (done, _) = numbers(&undo_dir);
    Ok(done
        .into_iter()
        .rev()
        .filter_map(|n| read_meta(&entry_dir(&undo_dir, n)))
        .take(last)
        .map(|m| LogEntry { n: m.n, time: m.time, what: m.what, changes: m.changes, files: m.files })
        .collect())
}

/// What settings last wrote to each file.
pub fn written(dir: &Path) -> Written {
    read_written(&dir.join(UNDO))
}

/// Whether a copy of `file` that loaded clean is kept for `restore`.
pub fn has_last_good(dir: &Path, file: ConfigFile) -> bool {
    read_small(&dir.join(UNDO).join(last_good_name(file))).ok().flatten().is_some_and(|b| !b.is_empty())
}

/// The bar's reader keeps the text of a file that just loaded clean, for
/// `restore`. Skipped when the version is the one kept, when the process is
/// elevated, and when a settings write holds the lock (the next clean load
/// tries again). True when a copy was written.
pub fn keep_last_good(dir: &Path, file: ConfigFile, bytes: &[u8]) -> bool {
    if bytes.len() > MAX_SOURCE_BYTES || win::is_elevated() {
        return false;
    }
    let undo = dir.join(UNDO);
    match std::fs::create_dir(&undo) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(_) => return false,
    }
    let Ok(folder) = win::open_dir(&undo) else { return false };
    if !win::facts(&folder).is_ok_and(|f| f.is_dir() && !f.is_link()) {
        return false;
    }
    let Ok(_lock) = take_lock(&undo, false) else { return false };
    let target = undo.join(last_good_name(file));
    if read_small(&target).ok().flatten().is_some_and(|old| FileHash::of(&old) == FileHash::of(bytes)) {
        return false;
    }
    put_last_good(&undo, file, bytes).is_ok()
}

/// Writes the last-good copy of `file`: beside, then renamed over. The caller
/// holds the lock.
fn put_last_good(undo: &Path, file: ConfigFile, bytes: &[u8]) -> io::Result<()> {
    write_json_bytes(undo, last_good_name(file), bytes)
}

// ---------------------------------------------------------------- the log

/// The log lines of a finished write, one per written file, without the
/// `config:` prefix: `bar.toml: bar main: right [..] -> [..] (undo 17)`.
/// Only validated tokens are in them.
pub fn change_lines(plan: &Plan, applied: &Applied) -> Vec<String> {
    let mut lines = Vec::new();
    for fo in applied.files.iter().filter(|f| f.written) {
        let Some(fp) = plan.file(fo.file) else { continue };
        let tail = applied.undo.map_or(String::new(), |n| format!(" (undo {n})"));
        lines.push(format!("{}: {}{tail}", fo.file, fp.what));
    }
    lines
}

/// `bar.toml: undo 17 restored`.
pub fn undo_lines(undone: u32, applied: &Applied) -> Vec<String> {
    applied.files.iter().filter(|f| f.written).map(|f| format!("{}: undo {undone} restored", f.file)).collect()
}

/// `bar.toml: restored from the last good copy (undo 19)`.
pub fn restore_lines(applied: &Applied) -> Vec<String> {
    applied
        .files
        .iter()
        .filter(|f| f.written)
        .map(|f| match applied.undo {
            Some(n) => format!("{}: restored from the last good copy (undo {n})", f.file),
            None => format!("{}: restored from the last good copy", f.file),
        })
        .collect()
}

/// The line for a refused write, or None for the usage and value errors
/// (exit code 2) which are not logged.
pub fn refusal_line(err: &SettingsError) -> Option<String> {
    if err.exit_code() == 2 {
        return None;
    }
    let place = err.file.map_or("settings", ConfigFile::name);
    Some(format!("{place}: refused {}", err.code))
}

/// Appends one `config:` line to the live `rsb.log` in `log_dir`. Never
/// rotates, and skips the line when the file is over 10 MiB.
pub fn append_log_line(log_dir: &Path, line: &str) {
    const LIMIT: u64 = 10 * 1024 * 1024;
    let path = crate::logger::file_path(log_dir, 0);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > LIMIT) {
        return;
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let offset = rsb_core::time::Clock::now(&rsb_services::clock::SystemClock, rsb_core::time::TimeZone::Local);
    let stamp = crate::logger::timestamp_at(now, offset.utc_offset_min);
    let text = line.strip_prefix("config: ").unwrap_or(line);
    let full = crate::logger::format_line(&stamp, log::Level::Info, "config", text);
    let write = || -> io::Result<()> {
        std::fs::create_dir_all(log_dir)?;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        f.write_all(full.as_bytes())?;
        f.write_all(b"\n")
    };
    if let Err(e) = write() {
        log::warn!("the settings log line could not be written: {}", why(&e));
    }
}

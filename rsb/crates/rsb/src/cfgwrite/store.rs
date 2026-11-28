//! The `.undo` folder: undo entries, the versions settings last wrote, and
//! the last copies that loaded clean. Every read refuses a link; nothing is
//! deleted but the known file names of an entry.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use rsb_core::contract::{ConfigFile, FileHash, Op};
use rsb_core::paths::MAX_SOURCE_BYTES;
use serde_json::{Value, json};

use super::Written;
use super::win;

pub(crate) const UNDO: &str = ".undo";
pub(crate) const LOCK: &str = "lock";
pub(crate) const WRITTEN: &str = "written";
pub(crate) const CHANGE: &str = "change.json";
/// Put in an entry while its undo runs, so a crash in the middle can be told
/// from an edit by hand.
pub(crate) const UNDOING: &str = "undoing";
/// Entries kept.
pub(crate) const KEEP: u32 = 20;
const PART: &str = ".part";

pub(crate) fn before_name(file: ConfigFile) -> &'static str {
    match file {
        ConfigFile::BarToml => "before.bar.toml",
        ConfigFile::StyleCss => "before.style.css.txt",
    }
}

pub(crate) fn last_good_name(file: ConfigFile) -> &'static str {
    match file {
        ConfigFile::BarToml => "last-good.bar.toml",
        ConfigFile::StyleCss => "last-good.style.css.txt",
    }
}

pub(crate) fn entry_dir(undo: &Path, n: u32) -> PathBuf {
    undo.join(n.to_string())
}

pub(crate) fn part_dir(undo: &Path, n: u32) -> PathBuf {
    undo.join(format!("{n}{PART}"))
}

/// What one undo entry says about itself (`change.json`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Meta {
    pub n: u32,
    pub time: String,
    pub what: String,
    pub changes: Vec<Op>,
    /// File, version before, version after.
    pub files: Vec<(ConfigFile, FileHash, FileHash)>,
}

impl Meta {
    pub(crate) fn to_json(&self) -> Value {
        let mut files = serde_json::Map::new();
        for (f, before, after) in &self.files {
            files.insert(f.name().to_string(), json!({"before": before.to_string(), "after": after.to_string()}));
        }
        json!({
            "n": self.n,
            "time": self.time,
            "what": self.what,
            "changes": serde_json::to_value(&self.changes).unwrap_or(Value::Null),
            "files": Value::Object(files),
        })
    }

    pub(crate) fn from_json(v: &Value) -> Option<Meta> {
        let n = u32::try_from(v.get("n")?.as_u64()?).ok()?;
        let time = v.get("time")?.as_str()?.to_string();
        let what = v.get("what")?.as_str()?.to_string();
        let changes: Vec<Op> = serde_json::from_value(v.get("changes")?.clone()).unwrap_or_default();
        let mut files = Vec::new();
        for f in ConfigFile::ALL {
            if let Some(e) = v.get("files").and_then(|m| m.get(f.name())) {
                let before = e.get("before")?.as_str()?.parse().ok()?;
                let after = e.get("after")?.as_str()?.parse().ok()?;
                files.push((f, before, after));
            }
        }
        Some(Meta { n, time, what, changes, files })
    }
}

/// Reads a small file, refusing a link and anything over the size cap.
/// `Ok(None)`: it is not there.
pub(crate) fn read_small(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let file = match win::open_read(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let facts = win::facts(&file)?;
    if facts.is_link() || facts.is_dir() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a link or a folder where a file belongs"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "larger than the size cap"));
    }
    Ok(Some(bytes))
}

/// An entry number as written by `entry_dir`: no sign, no leading zeros.
fn entry_number(name: &str) -> Option<u32> {
    name.parse::<u32>().ok().filter(|n| n.to_string() == name)
}

/// The numbers of the finished entries and of the `.part` entries, ascending.
/// Only a real folder counts: a link or junction by such a name is ignored.
pub(crate) fn numbers(undo: &Path) -> (Vec<u32>, Vec<u32>) {
    let (mut done, mut parts) = (Vec::new(), Vec::new());
    let Ok(read) = std::fs::read_dir(undo) else { return (done, parts) };
    for e in read.flatten() {
        let Ok(name) = e.file_name().into_string() else { continue };
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        if let Some(n) = name.strip_suffix(PART).and_then(entry_number) {
            parts.push(n);
        } else if let Some(n) = entry_number(&name) {
            done.push(n);
        }
    }
    done.sort_unstable();
    parts.sort_unstable();
    (done, parts)
}

/// The next entry number: one above the highest, finished or not. None when
/// the numbers have run out (a planted folder named with the largest one).
pub(crate) fn next_number(undo: &Path) -> Option<u32> {
    let (done, parts) = numbers(undo);
    done.iter().chain(&parts).copied().max().map_or(Some(1), |m| m.checked_add(1))
}

pub(crate) fn read_meta(dir: &Path) -> Option<Meta> {
    let bytes = read_small(&dir.join(CHANGE)).ok()??;
    Meta::from_json(&serde_json::from_slice(&bytes).ok()?)
}

pub(crate) fn read_written(undo: &Path) -> Written {
    let parsed = read_small(&undo.join(WRITTEN)).ok().flatten().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    let get = |f: ConfigFile| parsed.as_ref()?.get(f.name())?.as_str()?.parse::<FileHash>().ok();
    Written { bar_toml: get(ConfigFile::BarToml), style_css: get(ConfigFile::StyleCss) }
}

pub(crate) fn written_json(w: &Written) -> Value {
    let mut map = serde_json::Map::new();
    if let Some(h) = w.bar_toml {
        map.insert("bar.toml".into(), json!(h.to_string()));
    }
    if let Some(h) = w.style_css {
        map.insert("style.css".into(), json!(h.to_string()));
    }
    Value::Object(map)
}

pub(crate) fn written_get(w: &Written, file: ConfigFile) -> Option<FileHash> {
    match file {
        ConfigFile::BarToml => w.bar_toml,
        ConfigFile::StyleCss => w.style_css,
    }
}

pub(crate) fn written_set(w: &mut Written, file: ConfigFile, hash: FileHash) {
    match file {
        ConfigFile::BarToml => w.bar_toml = Some(hash),
        ConfigFile::StyleCss => w.style_css = Some(hash),
    }
}

/// Removes an entry folder by the file names it may hold, then the folder,
/// which only goes when it is empty. Anything else in it stays. A folder that
/// is a link is left alone: its target is not ours.
pub(crate) fn remove_entry(dir: &Path) {
    let Ok(held) = win::open_dir(dir) else { return };
    if !win::facts(&held).is_ok_and(|f| f.is_dir() && !f.is_link()) {
        return;
    }
    let mut names = vec![CHANGE, UNDOING];
    names.extend(ConfigFile::ALL.map(before_name));
    for name in names {
        // A link is removed as the link, never followed; a folder of that
        // name is left, and then the folder stays.
        let _ = std::fs::remove_file(dir.join(name));
    }
    // The folder cannot go while we hold it open.
    drop(held);
    let _ = std::fs::remove_dir(dir);
}

/// The keep-20 rule: entries older than the newest 20 go.
pub(crate) fn prune(undo: &Path) {
    let (done, _) = numbers(undo);
    if done.len() as u32 <= KEEP {
        return;
    }
    let excess = done.len() - KEEP as usize;
    for n in done.into_iter().take(excess) {
        remove_entry(&entry_dir(undo, n));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_round_trips_through_json() {
        let meta = Meta {
            n: 7,
            time: "2026-11-27T10:15:00+11:00".into(),
            what: "bar.toml: preset default -> glass".into(),
            changes: vec![Op::Set { key: "preset".into(), value: json!("glass") }],
            files: vec![
                (ConfigFile::BarToml, FileHash::Fnv(1), FileHash::Fnv(2)),
                (ConfigFile::StyleCss, FileHash::Missing, FileHash::Fnv(3)),
            ],
        };
        assert_eq!(Meta::from_json(&meta.to_json()), Some(meta));
        assert_eq!(Meta::from_json(&json!({"n": "x"})), None);
    }
}

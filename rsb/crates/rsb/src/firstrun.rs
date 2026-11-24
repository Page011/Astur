//! The config folder's files: what the first start writes, what every start
//! refreshes, `rsb types` and `rsb eject`. Nothing here ever overwrites a
//! file the user may have edited; the one exception is `types/rsb.d.luau`,
//! which belongs to this build (stale types after an upgrade would mislead
//! luau-lsp).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use rsb_core::paths::{LUAURC, TYPES_FILE, VSCODE_SETTINGS, WIDGETS_DIR};
use rsb_script::types;

use crate::assets;

/// Writes `text` to `path` only when the file does not exist yet.
/// `Ok(true)` = written, `Ok(false)` = it was already there.
fn create_new(path: &Path, text: &str) -> std::io::Result<bool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut f) => {
            f.write_all(text.as_bytes())?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

/// Rewrites `types/rsb.d.luau` when it differs from this build's. Returns
/// whether it wrote.
pub fn refresh_types(dir: &Path) -> std::io::Result<bool> {
    let path = dir.join(TYPES_FILE);
    let want = types::generate();
    if fs::read_to_string(&path).is_ok_and(|have| have == want) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, want)?;
    Ok(true)
}

/// `rsb types`: the type file always, `.luaurc` and the VS Code settings
/// only when missing (people edit those).
pub fn write_types(dir: &Path) -> std::io::Result<Vec<String>> {
    let mut wrote = Vec::new();
    if refresh_types(dir)? {
        wrote.push(TYPES_FILE.to_string());
    }
    if create_new(&dir.join(LUAURC), types::luaurc())? {
        wrote.push(LUAURC.to_string());
    }
    if create_new(&dir.join(VSCODE_SETTINGS), types::vscode_settings())? {
        wrote.push(VSCODE_SETTINGS.to_string());
    }
    Ok(wrote)
}

/// What a start did to the config folder.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Prepared {
    /// `bar.toml` and `style.css` were both missing: this is a new setup,
    /// whether or not the folder itself existed (`rsb types` and `rsb eject`
    /// make it before the first start).
    pub first_run: bool,
    pub types_written: bool,
}

/// Every start: write what is missing and never touch what is there.
/// `bar.toml` and `style.css` (the templates), an empty `widgets\`, the
/// editor files (`.luaurc`, `.vscode\settings.json`), and the type file,
/// which is refreshed when this build's differs. An `Err` is for the caller
/// to turn into "the config folder is not writable".
pub fn prepare(dir: &Path) -> Result<Prepared, String> {
    let io = |what: &str, e: std::io::Error| format!("{what}: {e}");
    fs::create_dir_all(dir).map_err(|e| io("cannot create the config folder", e))?;
    let mut wrote_template = [false; 2];
    for (i, (name, asset)) in [("bar.toml", "config/bar.toml"), ("style.css", "config/style.css")].iter().enumerate() {
        let text = assets::file(asset).unwrap_or("");
        wrote_template[i] = create_new(&dir.join(name), text).map_err(|e| io(&format!("cannot write {name}"), e))?;
    }
    fs::create_dir_all(dir.join(WIDGETS_DIR)).map_err(|e| io("cannot create widgets", e))?;
    let types_written = match write_types(dir) {
        Ok(wrote) => wrote.iter().any(|w| w == TYPES_FILE),
        Err(e) => {
            // The bar runs without editor files; only a first setup treats this as fatal.
            if wrote_template.iter().all(|w| *w) {
                return Err(io("cannot write the type files", e));
            }
            log::warn!("the editor files could not be written: {e}");
            false
        }
    };
    Ok(Prepared { first_run: wrote_template.iter().all(|w| *w), types_written })
}

/// The result of `rsb eject`, for the CLI to print.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Ejected {
    pub copied: Vec<String>,
    /// Helper files that were already in the folder and were left alone.
    pub kept: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EjectError {
    /// Not a built-in widget name; carries the list for the message.
    Unknown(String, Vec<String>),
    /// `widgets\<name>.luau` is already there.
    Exists(String),
    Io(String),
}

impl std::fmt::Display for EjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EjectError::Unknown(name, list) => {
                write!(f, "'{name}' is not a built-in widget (built-in widgets: {})", list.join(", "))
            }
            EjectError::Exists(path) => write!(f, "{path} already exists: edit it, or delete it to start again"),
            EjectError::Io(e) => write!(f, "{e}"),
        }
    }
}

/// Copies a built-in widget and the helper modules it requires into
/// `widgets\`. The name must be exactly one of the built-ins, so no path
/// can be smuggled in; nothing is overwritten.
pub fn eject(dir: &Path, widget: &str) -> Result<Ejected, EjectError> {
    let names = assets::builtin_widget_names();
    if !names.iter().any(|n| n == widget) {
        return Err(EjectError::Unknown(widget.to_string(), names));
    }
    let io = |e: std::io::Error| EjectError::Io(format!("cannot write into the widgets folder: {e}"));
    let main = format!("widgets/{widget}.luau");
    let text = assets::file(&main).unwrap_or("");
    if !create_new(&dir.join(&main), text).map_err(io)? {
        return Err(EjectError::Exists(main.replace('/', "\\")));
    }
    let mut out = Ejected { copied: vec![main], kept: Vec::new() };
    for lib in assets::lib_modules_of(widget) {
        let text = assets::file(&lib).unwrap_or("");
        if create_new(&dir.join(&lib), text).map_err(io)? {
            out.copied.push(lib);
        } else {
            out.kept.push(lib);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn the_first_run_writes_everything_and_later_runs_only_refresh_the_types() {
        let root = TempDir::new("first");
        let dir = root.path().join("cfg");
        let p = prepare(&dir).unwrap();
        assert_eq!(p, Prepared { first_run: true, types_written: true });
        for f in ["bar.toml", "style.css", "types/rsb.d.luau", ".luaurc", ".vscode/settings.json"] {
            assert!(dir.join(f).is_file(), "{f}");
        }
        assert!(dir.join("widgets").is_dir());
        fs::write(dir.join("bar.toml"), "mine").unwrap();
        fs::write(dir.join("types/rsb.d.luau"), "stale").unwrap();
        fs::write(dir.join(".luaurc"), "mine").unwrap();
        let p = prepare(&dir).unwrap();
        assert_eq!(p, Prepared { first_run: false, types_written: true });
        assert_eq!(fs::read_to_string(dir.join("bar.toml")).unwrap(), "mine");
        assert_eq!(fs::read_to_string(dir.join(".luaurc")).unwrap(), "mine");
        assert_eq!(fs::read_to_string(dir.join("types/rsb.d.luau")).unwrap(), types::generate());
        assert!(!prepare(&dir).unwrap().types_written, "unchanged types are not rewritten");
    }

    #[test]
    fn an_existing_empty_folder_gets_the_first_run_files() {
        let root = TempDir::new("empty");
        let p = prepare(root.path()).unwrap();
        assert_eq!(p, Prepared { first_run: true, types_written: true });
        for f in ["bar.toml", "style.css", "types/rsb.d.luau", ".luaurc", ".vscode/settings.json"] {
            assert!(root.path().join(f).is_file(), "{f}");
        }
        assert!(root.path().join("widgets").is_dir());
    }

    #[test]
    fn a_folder_made_by_rsb_types_or_eject_still_gets_its_templates() {
        // `rsb types` first (the guide's editor setup), then the first start.
        let root = TempDir::new("after-types");
        write_types(root.path()).unwrap();
        assert!(!root.path().join("bar.toml").exists());
        let p = prepare(root.path()).unwrap();
        assert!(p.first_run && !p.types_written, "{p:?}");
        assert!(root.path().join("bar.toml").is_file() && root.path().join("style.css").is_file());
        // `rsb eject` first: widgets\ exists, the templates do not.
        let root = TempDir::new("after-eject");
        eject(root.path(), "clock").unwrap();
        assert!(prepare(root.path()).unwrap().first_run);
        assert!(root.path().join("bar.toml").is_file());
        assert!(root.path().join("widgets/clock.luau").is_file(), "the ejected file is untouched");
    }

    #[test]
    fn missing_editor_files_come_back_but_existing_ones_stay() {
        let root = TempDir::new("editor");
        prepare(root.path()).unwrap();
        fs::remove_file(root.path().join(".luaurc")).unwrap();
        fs::write(root.path().join(".vscode/settings.json"), "mine").unwrap();
        let p = prepare(root.path()).unwrap();
        assert!(!p.first_run);
        assert!(root.path().join(".luaurc").is_file());
        assert_eq!(fs::read_to_string(root.path().join(".vscode/settings.json")).unwrap(), "mine");
    }

    #[test]
    fn rsb_types_keeps_user_edited_files() {
        let root = TempDir::new("types");
        fs::create_dir_all(root.path().join(".vscode")).unwrap();
        fs::write(root.path().join(".vscode/settings.json"), "{}").unwrap();
        let wrote = write_types(root.path()).unwrap();
        assert_eq!(wrote, ["types/rsb.d.luau", ".luaurc"]);
        assert_eq!(fs::read_to_string(root.path().join(".vscode/settings.json")).unwrap(), "{}");
        assert!(write_types(root.path()).unwrap().is_empty());
    }

    #[test]
    fn eject_copies_the_widget_and_its_helpers_and_never_overwrites() {
        let root = TempDir::new("eject");
        let out = eject(root.path(), "text").unwrap();
        assert_eq!(out.copied[0], "widgets/text.luau");
        assert!(out.copied.iter().any(|c| c == "widgets/lib/cmdline.luau"), "{out:?}");
        assert_eq!(
            fs::read_to_string(root.path().join("widgets/text.luau")).unwrap(),
            assets::file("widgets/text.luau").unwrap()
        );
        // A second eject refuses and says so; a helper someone else already
        // ejected is kept, not replaced.
        assert!(matches!(eject(root.path(), "text"), Err(EjectError::Exists(p)) if p == "widgets\\text.luau"));
        fs::remove_file(root.path().join("widgets/text.luau")).unwrap();
        fs::write(root.path().join("widgets/lib/cmdline.luau"), "mine").unwrap();
        let again = eject(root.path(), "text").unwrap();
        assert!(again.kept.iter().any(|k| k == "widgets/lib/cmdline.luau"), "{again:?}");
        assert_eq!(fs::read_to_string(root.path().join("widgets/lib/cmdline.luau")).unwrap(), "mine");
    }

    #[test]
    fn eject_names_are_validated_against_the_builtins() {
        let root = TempDir::new("eject-names");
        for bad in ["", "nope", "../clock", "clock.luau", "lib/format", "format", "C:\\x", "clock/../clock"] {
            match eject(root.path(), bad) {
                Err(EjectError::Unknown(_, list)) => assert!(list.iter().any(|n| n == "clock")),
                other => panic!("{bad:?}: {other:?}"),
            }
        }
        assert!(!root.path().join("widgets").exists(), "a refused name writes nothing");
        let msg = eject(root.path(), "x").unwrap_err().to_string();
        assert!(msg.contains("built-in widgets: ") && msg.contains("clock"), "{msg}");
    }
}

//! Reading the config folder: `bar.toml`, then the style sheet chain (the
//! base theme, the preset's sheet, `@import`ed files, `style.css`). Used by
//! the first start, the config reader thread and `rsb render`; it never
//! touches the UI and never panics on bad input: problems come back as
//! diagnostics with file:line:col, and a file that cannot be used at all
//! leaves its part as `None` so the caller keeps the last good one.

use std::io::Read;
use std::path::{Component, Path};
use std::sync::Arc;

use rsb_core::config::{Config, DEFAULT_PRESET, load_config};
use rsb_core::contract::FileHash;
use rsb_core::css::{BASE_SHEET, PRESET_SHEET, Stylesheet};
use rsb_core::diag::{DiagKind, Diagnostic, Severity, SourcePos};
use rsb_core::io::limits::{FILE_SHARING_RETRIES, FILE_SHARING_RETRY_DELAY};
use rsb_core::paths::{BAR_TOML, MAX_SOURCE_BYTES, STYLE_CSS};

use crate::assets;
use crate::fonts::{FontCache, FontFile, FontLoad};
use crate::msg::AppliedHashes;

/// Most files an `@import` chain may pull in, and how deep it may go.
pub const MAX_IMPORT_FILES: u32 = 8;
pub const MAX_IMPORT_DEPTH: u32 = 4;

/// `Ok(None)`: the file does not exist. A sharing violation (an editor still
/// writing) is retried a few times. Text is lossy UTF-8 without a byte order
/// mark. The error text never holds the path.
pub fn read_capped(path: &Path, cap: usize) -> Result<Option<String>, String> {
    Ok(read_capped_hashed(path, cap)?.map(|(text, _)| text))
}

/// `read_capped`, and the version of the raw bytes the text was made from (a
/// byte order mark included), from the same read.
pub fn read_capped_hashed(path: &Path, cap: usize) -> Result<Option<(String, FileHash)>, String> {
    Ok(read_bytes_capped_hashed(path, cap)?.map(|(bytes, hash)| {
        let text = String::from_utf8_lossy(&bytes);
        (text.strip_prefix('\u{feff}').unwrap_or(&text).to_string(), hash)
    }))
}

/// The same read for a binary file (a font): the bytes as they are.
pub fn read_bytes_capped(path: &Path, cap: usize) -> Result<Option<Vec<u8>>, String> {
    read_raw(path, cap)
}

/// `read_bytes_capped`, and the version of the bytes read.
pub fn read_bytes_capped_hashed(path: &Path, cap: usize) -> Result<Option<(Vec<u8>, FileHash)>, String> {
    Ok(read_raw(path, cap)?.map(|bytes| {
        let hash = FileHash::of(&bytes);
        (bytes, hash)
    }))
}

fn read_raw(path: &Path, cap: usize) -> Result<Option<Vec<u8>>, String> {
    let mut tries = 0;
    loop {
        match std::fs::File::open(path) {
            Ok(file) => {
                let mut bytes = Vec::new();
                match file.take(cap as u64 + 1).read_to_end(&mut bytes) {
                    Ok(_) => {}
                    Err(e) if sharing_violation(&e) && tries < FILE_SHARING_RETRIES => {
                        tries += 1;
                        std::thread::sleep(FILE_SHARING_RETRY_DELAY);
                        continue;
                    }
                    Err(e) => return Err(e.to_string()),
                }
                if bytes.len() > cap {
                    return Err(format!("the file is larger than {} KiB", cap / 1024));
                }
                #[cfg(test)]
                hooks::after_read(path);
                return Ok(Some(bytes));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) if sharing_violation(&e) && tries < FILE_SHARING_RETRIES => {
                tries += 1;
                std::thread::sleep(FILE_SHARING_RETRY_DELAY);
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn sharing_violation(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(32 | 33))
}

/// What one reading of the config folder produced.
#[derive(Debug, Clone)]
pub struct Built {
    /// None: `bar.toml` was unusable, keep the last good config.
    pub config: Option<Config>,
    /// None: a style file was unusable, keep the last good sheet.
    pub sheet: Option<Arc<Stylesheet>>,
    /// The base theme and the preset's sheet alone: what runs when the
    /// user's own sheet cannot be used on the very first start.
    pub base_sheet: Arc<Stylesheet>,
    /// Config and style diagnostics in file order; the log gets every one.
    pub diagnostics: Vec<Diagnostic>,
    /// Font names the new sheet asks for, to check against the installed ones.
    pub fonts: Vec<(String, SourcePos)>,
    /// The font files the new sheet's `@font-face` rules name, read here so
    /// the UI thread never opens one.
    pub font_files: Vec<FontFile>,
    /// Font files that could not be used (missing, too large, too many).
    pub font_diagnostics: Vec<Diagnostic>,
    /// The versions of the raw bytes `config` and `sheet` were parsed from;
    /// `None` where that file's part was not used. `style_css` is the version
    /// of `style.css` itself, the file a settings write replaces; a change to
    /// a file it `@import`s shows in `user_files`, not here.
    pub hashes: AppliedHashes,
    /// Every user file the build read, in reading order (`bar.toml`,
    /// `style.css`, then each `@import`), with the version of the bytes read;
    /// a file that does not exist is `FileHash::Missing`. Empty when the
    /// folder's own files were ignored.
    pub user_files: Vec<(String, FileHash)>,
}

/// Which files a build reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source<'a> {
    /// The folder's own `bar.toml` and style chain.
    Folder,
    /// The folder is ignored: the named preset as shipped.
    Shipped(&'a str),
    /// The folder's own files with only the preset swapped for the named one.
    Swapped(&'a str),
}

impl<'a> Source<'a> {
    fn preset(self) -> Option<&'a str> {
        match self {
            Source::Folder => None,
            Source::Shipped(p) | Source::Swapped(p) => Some(p),
        }
    }

    fn reads_user_files(self) -> bool {
        !matches!(self, Source::Shipped(_))
    }
}

/// Reads `bar.toml` and the style chain from `dir`. With `preset_override`
/// the folder's own files are ignored and that preset is built as shipped.
/// `fallback_preset` names the preset whose sheet to build when `bar.toml`
/// cannot be used.
pub fn build(dir: &Path, preset_override: Option<&str>, fallback_preset: &str) -> Built {
    build_cached(dir, preset_override, fallback_preset, &mut FontCache::default())
}

/// `build` for a caller that reads the folder again and again: a font file
/// the cache says was already handed over travels without its bytes.
pub fn build_cached(dir: &Path, preset_override: Option<&str>, fallback_preset: &str, fonts: &mut FontCache) -> Built {
    let source = preset_override.map_or(Source::Folder, Source::Shipped);
    build_from(dir, source, fallback_preset, fonts)
}

/// The folder's own `bar.toml` and style files with only the preset swapped
/// for `preset`: what the bar would look like after switching to it. Nothing
/// is applied from this build, so it carries no hashes.
pub fn build_as(dir: &Path, preset: &str) -> Built {
    build_from(dir, Source::Swapped(preset), DEFAULT_PRESET, &mut FontCache::default())
}

fn build_from(dir: &Path, source: Source, fallback_preset: &str, fonts: &mut FontCache) -> Built {
    let mut diagnostics = Vec::new();
    let mut user_files: Vec<(String, FileHash)> = Vec::new();
    let mut bar_hash = None;
    let user_toml = match source.reads_user_files() {
        false => None,
        true => match read_capped_hashed(&dir.join(BAR_TOML), MAX_SOURCE_BYTES) {
            Ok(read) => {
                let hash = read.as_ref().map_or(FileHash::Missing, |(_, h)| *h);
                user_files.push((BAR_TOML.to_string(), hash));
                bar_hash = Some(hash);
                read.map(|(text, _)| text)
            }
            Err(e) => {
                diagnostics.push(Diagnostic::error(
                    DiagKind::Config,
                    Some(SourcePos::new(BAR_TOML, 0, 0)),
                    format!("cannot read the file: {e}"),
                ));
                return Built {
                    config: None,
                    sheet: None,
                    base_sheet: base_only(fallback_preset),
                    diagnostics,
                    fonts: Vec::new(),
                    font_files: Vec::new(),
                    font_diagnostics: Vec::new(),
                    hashes: AppliedHashes::default(),
                    user_files: Vec::new(),
                };
            }
        },
    };
    let preset_toml = |name: &str| -> Option<String> {
        let name = match source {
            Source::Shipped(p) if name == DEFAULT_PRESET => p,
            // The user's own `preset` line is the one being swapped out.
            Source::Swapped(p) => p,
            _ => name,
        };
        assets::preset_toml(name).map(str::to_string)
    };
    let load = load_config(user_toml.as_deref().map(|t| (t, BAR_TOML)), &preset_toml);
    diagnostics.extend(load.diagnostics);
    let mut config = load.config;
    if let (Some(cfg), Some(p)) = (config.as_mut(), source.preset()) {
        cfg.preset = p.to_string();
    }
    let preset = config.as_ref().map_or(fallback_preset, |c| c.preset.as_str()).to_string();

    let mut sheet = Stylesheet::empty();
    for (text, name) in [(assets::base_css(), BASE_SHEET), (assets::preset_css(&preset).unwrap_or(""), PRESET_SHEET)] {
        let (part, diags) = Stylesheet::parse(text, name);
        for d in diags {
            log::warn!("built-in style {d}");
        }
        sheet.append(part);
    }
    let base_sheet = Arc::new(sheet.clone());
    let mut usable = true;
    if source.reads_user_files() {
        let mut seen = Visited::default();
        let before = diagnostics.len();
        match load_user_css(dir, STYLE_CSS, 1, &mut seen, &mut diagnostics, &mut user_files, None) {
            Ok(Some(user)) => sheet.append(user),
            Ok(None) => {}
            Err(()) => usable = false,
        }
        if diagnostics[before..].iter().any(|d| d.severity == Severity::Error) {
            usable = false;
        }
    }
    let families = sheet.font_families();
    // Files are read only for a sheet that will be used, and only then does
    // the cache learn what was handed over.
    let loaded =
        if usable && source.reads_user_files() { fonts.load(dir, &sheet.font_faces()) } else { FontLoad::default() };
    // Only the folder's own build is applied by the bar: each hash says which
    // bytes that file's part was made from, and a part that was not used has none.
    let hashes = if source == Source::Folder {
        let style = user_files.iter().find(|(name, _)| name == STYLE_CSS).map(|(_, h)| *h);
        AppliedHashes { bar_toml: config.as_ref().and(bar_hash), style_css: if usable { style } else { None } }
    } else {
        AppliedHashes::default()
    };
    Built {
        config,
        sheet: usable.then(|| Arc::new(sheet)),
        base_sheet,
        diagnostics,
        fonts: families,
        font_files: loaded.files,
        font_diagnostics: loaded.diagnostics,
        hashes,
        user_files,
    }
}

/// The style chain a `style.css` of the text `style_text` would make in `dir`:
/// the files it imports, then the text. For checks made before the text is
/// written; an import that cannot be read is left out.
pub fn style_chain_of(dir: &Path, style_text: &str) -> Stylesheet {
    let mut seen = Visited::default();
    let (mut diags, mut reads) = (Vec::new(), Vec::new());
    match load_user_css(dir, STYLE_CSS, 1, &mut seen, &mut diags, &mut reads, Some(style_text)) {
        Ok(Some(sheet)) => sheet,
        _ => Stylesheet::empty(),
    }
}

/// The base theme plus a preset's sheet, nothing of the user's.
fn base_only(preset: &str) -> Arc<Stylesheet> {
    let mut sheet = Stylesheet::empty();
    sheet.append(Stylesheet::parse(assets::base_css(), BASE_SHEET).0);
    sheet.append(Stylesheet::parse(assets::preset_css(preset).unwrap_or(""), PRESET_SHEET).0);
    Arc::new(sheet)
}

/// The style files looked at so far in one walk, and the ones being read now.
#[derive(Default)]
struct Visited {
    files: u32,
    chain: Vec<String>,
}

/// A style file and everything it imports, imports first. `Err(())`: the
/// file could not be read (a diagnostic was added). Every file looked at is
/// added to `reads` with the version of the bytes read.
fn load_user_css(
    dir: &Path,
    rel: &str,
    depth: u32,
    seen: &mut Visited,
    diags: &mut Vec<Diagnostic>,
    reads: &mut Vec<(String, FileHash)>,
    text_of_root: Option<&str>,
) -> Result<Option<Stylesheet>, ()> {
    let given = text_of_root.filter(|_| depth == 1);
    let read = match given {
        Some(t) => Ok(Some((t.to_string(), FileHash::of(t.as_bytes())))),
        None => read_capped_hashed(&dir.join(rel), MAX_SOURCE_BYTES),
    };
    let text = match read {
        Ok(Some((t, hash))) => {
            reads.push((rel.to_string(), hash));
            t
        }
        Ok(None) => {
            reads.push((rel.to_string(), FileHash::Missing));
            return Ok(None);
        }
        Err(e) => {
            diags.push(Diagnostic::error(
                DiagKind::Style,
                Some(SourcePos::new(rel, 0, 0)),
                format!("cannot read the file: {e}"),
            ));
            return Err(());
        }
    };
    seen.files += 1;
    seen.chain.push(rel.to_string());
    let (sheet, parse_diags) = Stylesheet::parse(&text, rel);
    diags.extend(parse_diags);
    let mut combined = Stylesheet::empty();
    for (target, pos) in sheet.imports() {
        let problem = import_problem(&target, depth, seen.files, &seen.chain);
        if let Some(problem) = problem {
            diags.push(Diagnostic::warning(DiagKind::Style, Some(pos), format!("@import \"{target}\": {problem}")));
            continue;
        }
        match load_user_css(dir, &target, depth + 1, seen, diags, reads, None) {
            Ok(Some(child)) => combined.append(child),
            Ok(None) => diags.push(Diagnostic::warning(
                DiagKind::Style,
                Some(pos),
                format!("@import \"{target}\": file not found"),
            )),
            Err(()) => {}
        }
    }
    seen.chain.pop();
    combined.append(sheet);
    Ok(Some(combined))
}

/// Why an `@import` target may not be followed, if it may not.
fn import_problem(target: &str, depth: u32, files: u32, chain: &[String]) -> Option<&'static str> {
    let path = Path::new(target);
    let plain = !target.contains(['\\', ':']) && path.components().all(|c| matches!(c, Component::Normal(_)));
    if !plain || !target.to_ascii_lowercase().ends_with(".css") {
        return Some("use a .css file inside the config folder, like \"colors.css\"");
    }
    if depth >= MAX_IMPORT_DEPTH {
        return Some("imports are followed 4 levels deep at most");
    }
    if files >= MAX_IMPORT_FILES {
        return Some("a style sheet may pull in 8 files at most");
    }
    if chain.iter().any(|c| c.eq_ignore_ascii_case(target)) {
        return Some("this file is already being read (an import loop)");
    }
    None
}

/// A test can change a file between our read of it and whatever comes next,
/// which is what a hash taken by a second read would get wrong.
#[cfg(test)]
mod hooks {
    use std::cell::RefCell;
    use std::path::Path;

    type Hook = Box<dyn FnMut(&Path)>;

    thread_local! {
        static AFTER_READ: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    pub fn set(hook: impl FnMut(&Path) + 'static) {
        AFTER_READ.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
    }

    pub fn clear() {
        AFTER_READ.with(|h| *h.borrow_mut() = None);
    }

    pub fn after_read(path: &Path) {
        AFTER_READ.with(|h| {
            if let Some(hook) = h.borrow_mut().as_mut() {
                hook(path);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use rsb_core::contrast;
    use rsb_core::preset::Scheme;
    use std::collections::HashSet;

    fn probe(sheet: &Stylesheet) -> String {
        contrast::hex(contrast::resolve_vars(sheet, Scheme::Dark)["probe"].0)
    }

    fn write(dir: &TempDir, name: &str, text: &str) {
        let p = dir.path().join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn an_empty_folder_gives_the_default_bar_and_no_diagnostics() {
        let dir = TempDir::new("cfg");
        let b = build(dir.path(), None, "default");
        let cfg = b.config.expect("a config");
        let preset_height: u32 = crate::assets::preset_toml("default")
            .and_then(|t| t.lines().find_map(|l| l.trim().strip_prefix("height = ")?.parse().ok()))
            .expect("the default preset sets a height");
        assert_eq!((cfg.preset.as_str(), cfg.bars.len(), cfg.bars[0].height), ("default", 1, preset_height));
        assert!(b.diagnostics.is_empty(), "{:?}", b.diagnostics);
        assert!(b.sheet.is_some_and(|s| s.rule_count() > 10));
    }

    #[test]
    fn a_bar_toml_error_keeps_the_last_config_and_names_the_line() {
        let dir = TempDir::new("cfg");
        write(&dir, "bar.toml", "config_version = 1\n[[bar]]\nheight = \n");
        let b = build(dir.path(), None, "default");
        assert!(b.config.is_none());
        let first = b.diagnostics.first().expect("a diagnostic");
        assert_eq!(first.severity, Severity::Error);
        assert!(first.to_string().contains("bar.toml:3"), "{first}");
    }

    #[test]
    fn a_style_error_keeps_the_last_sheet_but_a_warning_does_not() {
        let dir = TempDir::new("cfg");
        write(&dir, "style.css", "bar { colr: red }\n");
        let b = build(dir.path(), None, "default");
        assert!(b.sheet.is_some(), "a bad property is a warning: the rest of the sheet still applies");
        assert!(
            b.diagnostics
                .iter()
                .any(|d| d.to_string().contains("style.css:1:") && d.message.contains("did you mean 'color'"))
        );
        write(&dir, "style.css", "bar { color: red \n");
        let b = build(dir.path(), None, "default");
        assert!(b.sheet.is_none(), "an unterminated block makes the file unusable");
        assert!(b.diagnostics.iter().any(|d| d.severity == Severity::Error));
    }

    #[test]
    fn imports_come_first_and_are_limited() {
        let dir = TempDir::new("cfg");
        write(
            &dir,
            "style.css",
            "@import \"colors.css\";\n@import \"../outside.css\";\n@import \"missing.css\";\nbar { color: red }\n",
        );
        write(&dir, "colors.css", "bar { background: blue }\n");
        let b = build(dir.path(), None, "default");
        let plain = build(TempDir::new("cfg").path(), None, "default").sheet.unwrap().rule_count();
        assert_eq!(b.sheet.unwrap().rule_count(), plain + 2, "colors.css and style.css add one rule each");
        let msgs: Vec<String> = b.diagnostics.iter().map(|d| d.to_string()).collect();
        assert!(msgs.iter().any(|m| m.contains("style.css:2") && m.contains("inside the config folder")), "{msgs:?}");
        assert!(msgs.iter().any(|m| m.contains("missing.css") && m.contains("not found")), "{msgs:?}");
        // A loop of imports is reported, not followed forever.
        write(&dir, "style.css", "@import \"a.css\";\n");
        write(&dir, "a.css", "@import \"b.css\";\n");
        write(&dir, "b.css", "@import \"a.css\";\n");
        let b = build(dir.path(), None, "default");
        assert!(b.diagnostics.iter().any(|d| d.message.contains("import loop")), "{:?}", b.diagnostics);
    }

    #[test]
    fn a_preset_override_ignores_the_folder_and_a_byte_order_mark_is_stripped() {
        let dir = TempDir::new("cfg");
        write(&dir, "bar.toml", "this is not toml");
        let b = build(dir.path(), Some("glass"), "default");
        let cfg = b.config.expect("glass as shipped");
        assert_eq!(cfg.preset, "glass");
        assert!(cfg.bars[0].floating);
        assert!(b.diagnostics.is_empty());
        write(&dir, "bar.toml", "\u{feff}config_version = 1\n");
        assert!(build(dir.path(), None, "default").config.is_some());
    }

    #[test]
    fn a_hashed_read_hashes_the_raw_bytes_including_the_byte_order_mark() {
        let dir = TempDir::new("cfg");
        let bytes = b"\xef\xbb\xbfconfig_version = 1\n";
        let path = dir.path().join("bar.toml");
        std::fs::write(&path, bytes).unwrap();
        let (text, hash) = read_capped_hashed(&path, 1024).unwrap().unwrap();
        assert_eq!(text, "config_version = 1\n");
        assert_eq!(hash, FileHash::of(bytes));
        let (raw, raw_hash) = read_bytes_capped_hashed(&path, 1024).unwrap().unwrap();
        assert_eq!((raw.as_slice(), raw_hash), (bytes.as_slice(), hash));
        assert_eq!(read_capped_hashed(&dir.path().join("none.toml"), 1024), Ok(None));
        assert_eq!(read_capped(&path, 1024), Ok(Some(text)));
    }

    #[test]
    fn an_over_long_file_is_refused_without_its_path() {
        let dir = TempDir::new("cfg");
        write(&dir, "big.txt", &"x".repeat(5000));
        let err = read_capped(&dir.path().join("big.txt"), 1024).unwrap_err();
        assert!(err.contains("larger than 1 KiB") && !err.contains("big.txt"), "{err}");
        assert_eq!(read_capped(&dir.path().join("none.txt"), 1024), Ok(None));
    }

    #[test]
    fn applied_hash_matches_parsed_text() {
        let dir = TempDir::new("cfg");
        let (old_toml, old_css) = (
            "config_version = 1
[[bar]]
height = 40
",
            ":root { --probe: #112233; }
",
        );
        let (new_toml, new_css) = (
            "config_version = 1
[[bar]]
height = 50
",
            ":root { --probe: #445566; }
",
        );
        write(&dir, "bar.toml", old_toml);
        write(&dir, "style.css", old_css);
        // A save lands right after each file is read: a second read for the hash would see it.
        let root = dir.path().to_path_buf();
        let mut seen = HashSet::new();
        hooks::set(move |path| {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if seen.insert(name.clone()) {
                match name.as_str() {
                    "bar.toml" => std::fs::write(root.join("bar.toml"), new_toml).unwrap(),
                    "style.css" => std::fs::write(root.join("style.css"), new_css).unwrap(),
                    _ => {}
                }
            }
        });
        let first = build(dir.path(), None, "default");
        hooks::clear();
        assert_eq!(first.config.as_ref().unwrap().bars[0].height, 40, "parsed from the first read");
        assert_eq!(probe(first.sheet.as_ref().unwrap()), "#112233");
        assert_eq!(first.hashes.bar_toml, Some(FileHash::of(old_toml.as_bytes())));
        assert_eq!(first.hashes.style_css, Some(FileHash::of(old_css.as_bytes())));
        assert_ne!(first.hashes.bar_toml, Some(FileHash::of(new_toml.as_bytes())));
        // The next build sees the saved files and hashes those.
        let second = build(dir.path(), None, "default");
        assert_eq!(second.config.as_ref().unwrap().bars[0].height, 50);
        assert_eq!(probe(second.sheet.as_ref().unwrap()), "#445566");
        assert_eq!(second.hashes.bar_toml, Some(FileHash::of(new_toml.as_bytes())));
        assert_eq!(second.hashes.style_css, Some(FileHash::of(new_css.as_bytes())));
    }

    #[test]
    fn a_part_that_was_not_used_has_no_hash_and_a_missing_file_says_so() {
        let dir = TempDir::new("cfg");
        let empty = build(dir.path(), None, "default");
        assert_eq!(
            empty.hashes,
            AppliedHashes { bar_toml: Some(FileHash::Missing), style_css: Some(FileHash::Missing) }
        );
        assert_eq!(
            empty.user_files,
            vec![("bar.toml".to_string(), FileHash::Missing), ("style.css".to_string(), FileHash::Missing)]
        );
        write(
            &dir,
            "bar.toml",
            "config_version = 1
[[bar]]
height = 
",
        );
        write(
            &dir,
            "style.css",
            "bar { color: red }
",
        );
        let bad_toml = build(dir.path(), None, "default");
        assert_eq!((bad_toml.hashes.bar_toml, bad_toml.hashes.style_css.is_some()), (None, true));
        write(
            &dir,
            "bar.toml",
            "config_version = 1
",
        );
        write(
            &dir,
            "style.css",
            "bar { color: red 
",
        );
        let bad_css = build(dir.path(), None, "default");
        assert_eq!((bad_css.hashes.bar_toml.is_some(), bad_css.hashes.style_css), (true, None));
        let shipped = build(dir.path(), Some("glass"), "default");
        assert_eq!(shipped.hashes, AppliedHashes::default());
        assert!(shipped.user_files.is_empty(), "a shipped preset reads none of the folder's files");
    }

    #[test]
    fn every_file_of_the_style_chain_is_listed_with_its_version() {
        let dir = TempDir::new("cfg");
        write(
            &dir,
            "style.css",
            "@import \"a.css\";
@import \"gone.css\";
bar { color: red }
",
        );
        write(
            &dir,
            "a.css",
            "bar { background: blue }
",
        );
        let b = build(dir.path(), None, "default");
        let names: Vec<&str> = b.user_files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["bar.toml", "style.css", "a.css", "gone.css"]);
        assert_eq!(
            b.user_files[2].1,
            FileHash::of(
                b"bar { background: blue }
"
            )
        );
        assert_eq!(b.user_files[3].1, FileHash::Missing);
        assert_eq!(b.hashes.style_css, Some(b.user_files[1].1), "style.css's own version, not its imports'");
        // An edit to an imported file changes the sheet and that file's entry, not style.css's version.
        write(
            &dir,
            "a.css",
            "bar { background: green }
",
        );
        let edited = build(dir.path(), None, "default");
        assert_eq!(edited.hashes.style_css, b.hashes.style_css);
        assert_ne!(edited.user_files[2].1, b.user_files[2].1);
    }

    #[test]
    fn build_as_keeps_the_users_files_and_swaps_only_the_preset() {
        let dir = TempDir::new("cfg");
        write(
            &dir,
            "bar.toml",
            "config_version = 1
preset = \"glass\"
[[bar]]
height = 40
",
        );
        write(
            &dir,
            "style.css",
            ":root { --probe: #112233; }
",
        );
        let mine = build_as(dir.path(), "fluent");
        let cfg = mine.config.as_ref().expect("a config");
        assert_eq!((cfg.preset.as_str(), cfg.bars[0].height, cfg.bars[0].floating), ("fluent", 40, false));
        assert_eq!(probe(mine.sheet.as_ref().unwrap()), "#112233", "the user's sheet is in");
        assert!(mine.diagnostics.is_empty(), "{:?}", mine.diagnostics);
        assert_eq!(mine.hashes, AppliedHashes::default(), "nothing is applied from it");
        assert_eq!(mine.user_files.len(), 2);
        // Shipped, the same preset ignores both files.
        let shipped = build(dir.path(), Some("fluent"), "default");
        assert_eq!(shipped.config.as_ref().unwrap().bars[0].height, 32);
        assert!(!contrast::resolve_vars(shipped.sheet.as_ref().unwrap(), Scheme::Dark).contains_key("probe"));
        // fluent's own sheet is the base of `mine`: its dark background is there.
        let fluent_bg = contrast::resolve_vars(shipped.sheet.as_ref().unwrap(), Scheme::Dark)["bg"].0;
        assert_eq!(contrast::resolve_vars(mine.sheet.as_ref().unwrap(), Scheme::Dark)["bg"].0, fluent_bg);
    }
}

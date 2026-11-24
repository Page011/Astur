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
use rsb_core::css::{BASE_SHEET, PRESET_SHEET, Stylesheet};
use rsb_core::diag::{DiagKind, Diagnostic, Severity, SourcePos};
use rsb_core::io::limits::{FILE_SHARING_RETRIES, FILE_SHARING_RETRY_DELAY};
use rsb_core::paths::{BAR_TOML, MAX_SOURCE_BYTES, STYLE_CSS};

use crate::assets;

/// Most files an `@import` chain may pull in, and how deep it may go.
pub const MAX_IMPORT_FILES: u32 = 8;
pub const MAX_IMPORT_DEPTH: u32 = 4;

/// `Ok(None)`: the file does not exist. A sharing violation (an editor still
/// writing) is retried a few times. Text is lossy UTF-8 without a byte order
/// mark. The error text never holds the path.
pub fn read_capped(path: &Path, cap: usize) -> Result<Option<String>, String> {
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
                let text = String::from_utf8_lossy(&bytes);
                return Ok(Some(text.strip_prefix('\u{feff}').unwrap_or(&text).to_string()));
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
}

/// Reads `bar.toml` and the style chain from `dir`. With `preset_override`
/// the folder's own files are ignored and that preset is built as shipped.
/// `fallback_preset` names the preset whose sheet to build when `bar.toml`
/// cannot be used.
pub fn build(dir: &Path, preset_override: Option<&str>, fallback_preset: &str) -> Built {
    let mut diagnostics = Vec::new();
    let user_toml = match preset_override {
        Some(_) => None,
        None => match read_capped(&dir.join(BAR_TOML), MAX_SOURCE_BYTES) {
            Ok(text) => text,
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
                };
            }
        },
    };
    let preset_toml = |name: &str| -> Option<String> {
        let name = match preset_override {
            Some(p) if name == DEFAULT_PRESET => p,
            _ => name,
        };
        assets::preset_toml(name).map(str::to_string)
    };
    let load = load_config(user_toml.as_deref().map(|t| (t, BAR_TOML)), &preset_toml);
    diagnostics.extend(load.diagnostics);
    let mut config = load.config;
    if let (Some(cfg), Some(p)) = (config.as_mut(), preset_override) {
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
    if preset_override.is_none() {
        let mut files = 0;
        let mut chain = Vec::new();
        let before = diagnostics.len();
        match load_user_css(dir, STYLE_CSS, 1, &mut files, &mut chain, &mut diagnostics) {
            Ok(Some(user)) => sheet.append(user),
            Ok(None) => {}
            Err(()) => usable = false,
        }
        if diagnostics[before..].iter().any(|d| d.severity == Severity::Error) {
            usable = false;
        }
    }
    let fonts = sheet.font_families();
    Built { config, sheet: usable.then(|| Arc::new(sheet)), base_sheet, diagnostics, fonts }
}

/// The base theme plus a preset's sheet, nothing of the user's.
fn base_only(preset: &str) -> Arc<Stylesheet> {
    let mut sheet = Stylesheet::empty();
    sheet.append(Stylesheet::parse(assets::base_css(), BASE_SHEET).0);
    sheet.append(Stylesheet::parse(assets::preset_css(preset).unwrap_or(""), PRESET_SHEET).0);
    Arc::new(sheet)
}

/// A style file and everything it imports, imports first. `Err(())`: the
/// file could not be read (a diagnostic was added).
fn load_user_css(
    dir: &Path,
    rel: &str,
    depth: u32,
    files: &mut u32,
    chain: &mut Vec<String>,
    diags: &mut Vec<Diagnostic>,
) -> Result<Option<Stylesheet>, ()> {
    let text = match read_capped(&dir.join(rel), MAX_SOURCE_BYTES) {
        Ok(Some(t)) => t,
        Ok(None) => return Ok(None),
        Err(e) => {
            diags.push(Diagnostic::error(
                DiagKind::Style,
                Some(SourcePos::new(rel, 0, 0)),
                format!("cannot read the file: {e}"),
            ));
            return Err(());
        }
    };
    *files += 1;
    chain.push(rel.to_string());
    let (sheet, parse_diags) = Stylesheet::parse(&text, rel);
    diags.extend(parse_diags);
    let mut combined = Stylesheet::empty();
    for (target, pos) in sheet.imports() {
        let problem = import_problem(&target, depth, *files, chain);
        if let Some(problem) = problem {
            diags.push(Diagnostic::warning(DiagKind::Style, Some(pos), format!("@import \"{target}\": {problem}")));
            continue;
        }
        match load_user_css(dir, &target, depth + 1, files, chain, diags) {
            Ok(Some(child)) => combined.append(child),
            Ok(None) => diags.push(Diagnostic::warning(
                DiagKind::Style,
                Some(pos),
                format!("@import \"{target}\": file not found"),
            )),
            Err(()) => {}
        }
    }
    chain.pop();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

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
        assert_eq!((cfg.preset.as_str(), cfg.bars.len(), cfg.bars[0].height), ("default", 1, 32));
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
    fn an_over_long_file_is_refused_without_its_path() {
        let dir = TempDir::new("cfg");
        write(&dir, "big.txt", &"x".repeat(5000));
        let err = read_capped(&dir.path().join("big.txt"), 1024).unwrap_err();
        assert!(err.contains("larger than 1 KiB") && !err.contains("big.txt"), "{err}");
        assert_eq!(read_capped(&dir.path().join("none.txt"), 1024), Ok(None));
    }
}

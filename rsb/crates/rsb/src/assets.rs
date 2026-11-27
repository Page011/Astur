//! The files inside the binary (everything under `assets/`) and the widget
//! source store the script thread reads from: built-ins from here, user
//! widgets from the config folder.

use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};

use std::sync::OnceLock;

use rsb_core::config::BUILTIN_PRESETS;
use rsb_core::contract::{FileHash, PresetContrast};
use rsb_core::contrast::{self, AccentSurfaces};
use rsb_core::css::{BASE_SHEET, PRESET_SHEET, Stylesheet};
use rsb_core::diag::{DiagKind, Diagnostic, SourcePos};
use rsb_core::paths::MAX_SOURCE_BYTES;
use rsb_core::preset::{About, Scheme, parse_about};
use rsb_script::SourceStore;

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

/// An embedded file as text.
pub fn file(rel: &str) -> Option<&'static str> {
    EMBEDDED.iter().find(|(name, _)| *name == rel).and_then(|(_, bytes)| std::str::from_utf8(bytes).ok())
}

/// An embedded file as raw bytes (the font and the icons are not text).
pub fn bytes(rel: &str) -> Option<&'static [u8]> {
    EMBEDDED.iter().find(|(name, _)| *name == rel).map(|(_, bytes)| *bytes)
}

/// `clock`, `cpu`, ...: every `widgets/<name>.luau` (the `lib/` helpers are not widgets).
pub fn builtin_widget_names() -> Vec<String> {
    let mut names: Vec<String> = EMBEDDED
        .iter()
        .filter_map(|(name, _)| name.strip_prefix("widgets/")?.strip_suffix(".luau"))
        .filter(|n| !n.contains('/'))
        .map(str::to_string)
        .collect();
    names.sort();
    names
}

pub fn preset_toml(name: &str) -> Option<&'static str> {
    if name.contains(['/', '\\', '.']) {
        return None;
    }
    file(&format!("presets/{name}.toml"))
}

pub fn preset_css(name: &str) -> Option<&'static str> {
    if name.contains(['/', '\\', '.']) {
        return None;
    }
    file(&format!("presets/{name}.css"))
}

pub fn base_css() -> &'static str {
    file("themes/base.css").unwrap_or("")
}

/// The base theme and a preset's sheet stacked as the bar stacks them, or
/// None for a name that is not a built-in preset.
pub fn preset_chain(name: &str) -> Option<Stylesheet> {
    let css = preset_css(name)?;
    let mut sheet = Stylesheet::empty();
    sheet.append(Stylesheet::parse(base_css(), BASE_SHEET).0);
    sheet.append(Stylesheet::parse(css, PRESET_SHEET).0);
    Some(sheet)
}

/// The `[about]` table of a built-in preset.
pub fn preset_about(name: &str) -> Result<About, Diagnostic> {
    let text = preset_toml(name).ok_or_else(|| {
        Diagnostic::error(
            DiagKind::Config,
            Some(SourcePos::new(name, 0, 0)),
            "there is no built-in preset of this name",
        )
    })?;
    parse_about(text, &format!("presets/{name}.toml"))
}

/// The contrast of a built-in preset as shipped, for each scheme it lists.
pub fn preset_contrast(name: &str) -> Result<PresetContrast, Diagnostic> {
    let about = preset_about(name)?;
    let chain = preset_chain(name).unwrap_or_default();
    let of = |scheme: Scheme| {
        about
            .schemes
            .contains(&scheme)
            .then(|| contrast::summary(&contrast::pairs(&about, &contrast::resolve_vars(&chain, scheme))))
    };
    Ok(PresetContrast { dark: of(Scheme::Dark), light: of(Scheme::Light) })
}

/// The surfaces an accent has to stay readable on: those of `accent-text` and
/// `accent-line` in the pairs of every built-in preset, in each scheme. Built
/// once; `contrast::derive_accent` takes it.
pub fn accent_surfaces() -> &'static AccentSurfaces {
    static SURFACES: OnceLock<AccentSurfaces> = OnceLock::new();
    SURFACES.get_or_init(|| {
        let mut out = AccentSurfaces::default();
        for name in BUILTIN_PRESETS {
            let (Ok(about), Some(chain)) = (preset_about(name), preset_chain(name)) else { continue };
            for scheme in &about.schemes {
                contrast::collect_accent_surfaces(&about, &contrast::resolve_vars(&chain, *scheme), *scheme, &mut out);
            }
        }
        out
    })
}

/// Modules a built-in widget requires from `./lib/`, followed through the
/// helpers themselves, in first-use order.
pub fn lib_modules_of(widget: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut queue = vec![format!("widgets/{widget}.luau")];
    while let Some(path) = queue.pop() {
        let Some(text) = file(&path) else { continue };
        for module in required_libs(text) {
            let rel = format!("widgets/lib/{module}.luau");
            if !found.contains(&rel) {
                found.push(rel.clone());
                queue.push(rel);
            }
        }
    }
    found
}

/// The `x` of every `require("./lib/x")` in `text`.
fn required_libs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("require(") {
        rest = &rest[at + "require(".len()..];
        let quote = rest.chars().next();
        if let Some(q @ ('"' | '\'')) = quote
            && let Some(end) = rest[1..].find(q)
            && let Some(name) = rest[1..1 + end].strip_prefix("./lib/")
        {
            let name = name.strip_suffix(".luau").unwrap_or(name);
            if !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')) {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// Built-in widgets from the binary; user widgets from the config folder.
pub struct Store {
    pub config_dir: PathBuf,
}

impl Store {
    /// `SourceStore::user`, and the version of the raw bytes the text was
    /// made from, from the same read.
    pub fn user_hashed(&self, rel_path: &str) -> std::io::Result<Option<(String, FileHash)>> {
        // The script runtime only asks for resolved paths inside `widgets/`;
        // this guards the folder boundary anyway.
        let rel = Path::new(rel_path);
        if rel.is_absolute() || rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return Ok(None);
        }
        crate::cfgload::read_capped_hashed(&self.config_dir.join(rel), MAX_SOURCE_BYTES).map_err(std::io::Error::other)
    }
}

impl SourceStore for Store {
    fn builtin(&self, rel_path: &str) -> Option<Cow<'static, str>> {
        file(rel_path).map(Cow::Borrowed)
    }

    fn builtin_widgets(&self) -> Vec<String> {
        builtin_widget_names()
    }

    fn user(&self, rel_path: &str) -> std::io::Result<Option<String>> {
        Ok(self.user_hashed(rel_path)?.map(|(text, _)| text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsb_core::config::{BUILTIN_PRESETS, load_config};
    use rsb_core::css::Stylesheet;

    #[test]
    fn every_embedded_text_file_is_utf8_and_the_templates_exist() {
        // Font files are the one binary kind under assets.
        for (name, bytes) in EMBEDDED.iter().filter(|(name, _)| !name.starts_with("fonts/") || name.ends_with(".txt")) {
            assert!(std::str::from_utf8(bytes).is_ok(), "{name} is not UTF-8");
        }
        for must in ["config/bar.toml", "config/style.css", "themes/base.css", "widgets/clock.luau"] {
            assert!(file(must).is_some(), "{must} is not embedded");
        }
    }

    #[test]
    fn the_builtins_are_the_luau_files_in_the_widgets_folder_and_lib_files_are_not_widgets() {
        let names = builtin_widget_names();
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/widgets");
        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
            .expect("assets/widgets")
            .filter_map(|e| e.ok()?.path().file_name()?.to_str()?.strip_suffix(".luau").map(str::to_string))
            .collect();
        on_disk.sort();
        assert!(!on_disk.is_empty());
        assert_eq!(names, on_disk, "the embedded list follows the folder");
        for w in ["workspaces", "clock", "date", "cpu", "memory", "battery", "media", "text"] {
            assert!(names.iter().any(|n| n == w), "{w} missing from {names:?}");
        }
        assert!(!names.iter().any(|n| n.contains("lib") || n == "format"));
    }

    #[test]
    fn the_default_config_is_the_default_presets_bar() {
        let preset = load_config(None, &|n| preset_toml(n).map(str::to_string)).config.expect("config");
        let built_in = rsb_core::config::default_config();
        let names = |b: &rsb_core::config::BarSpec| {
            [&b.left, &b.center, &b.right].map(|z| z.iter().map(|i| i.widget.to_string()).collect::<Vec<_>>())
        };
        let (p, d) = (&preset.bars[0], &built_in.bars[0]);
        assert_eq!(names(p), names(d));
        assert_eq!(
            (p.height, p.margin, p.floating, p.reserve, p.center_on.clone()),
            (36, 8, false, true, d.center_on.clone())
        );
        assert_eq!((&p.name, p.position, p.fullscreen), (&d.name, d.position, d.fullscreen));
    }

    #[test]
    fn presets_and_themes_parse_without_a_diagnostic() {
        for preset in BUILTIN_PRESETS {
            let toml = preset_toml(preset).unwrap_or_else(|| panic!("no toml for {preset}"));
            let load = load_config(Some((toml, "preset.toml")), &|n| preset_toml(n).map(str::to_string));
            // A preset's `[about]` table is read from presets only; read as a user file it is a warning.
            let diags: Vec<_> = load.diagnostics.iter().filter(|d| !d.message.contains("'about'")).collect();
            assert!(load.config.is_some() && diags.is_empty(), "{preset}: {diags:?}");
            let (_, diags) = Stylesheet::parse(preset_css(preset).expect("css"), "preset.css");
            assert!(diags.is_empty(), "{preset}: {diags:?}");
        }
        let (_, diags) = Stylesheet::parse(base_css(), "base.css");
        assert!(diags.is_empty(), "{diags:?}");
        assert!(preset_toml("../x").is_none() && preset_css("a/b").is_none());
    }

    #[test]
    fn lib_modules_follow_requires_through_helpers() {
        assert!(lib_modules_of("clock").is_empty() || lib_modules_of("clock").iter().all(|m| file(m).is_some()));
        let text = lib_modules_of("text");
        assert!(text.iter().any(|m| m == "widgets/lib/cmdline.luau"), "{text:?}");
        assert_eq!(required_libs("local f = require(\"./lib/format\")\nrequire('./lib/a_b')"), ["format", "a_b"]);
        assert!(required_libs("require(\"../x\") require(\"./lib/../x\")").is_empty());
    }

    #[test]
    fn the_user_store_stays_inside_the_config_folder() {
        let store = Store { config_dir: std::env::temp_dir() };
        assert!(matches!(store.user("../x.luau"), Ok(None)));
        assert!(matches!(store.user("C:/x.luau"), Ok(None)));
        assert!(matches!(store.user("widgets/definitely-missing.luau"), Ok(None)));
        assert!(store.builtin("widgets/clock.luau").is_some());
        assert!(matches!(store.user_hashed("../x.luau"), Ok(None)));
    }

    #[test]
    fn every_built_in_preset_has_metadata_and_passing_contrast() {
        for name in BUILTIN_PRESETS {
            let about = preset_about(name).unwrap_or_else(|d| panic!("{name}: {d}"));
            let c = preset_contrast(name).unwrap();
            for (scheme, got) in [(Scheme::Dark, &c.dark), (Scheme::Light, &c.light)] {
                let got = got.as_ref().unwrap_or_else(|| panic!("{name} {}", scheme.name()));
                assert!(
                    got.pass && got.text.lowest >= 4.59 && got.graphic.lowest >= 3.0,
                    "{name} {}: {got:?}",
                    scheme.name()
                );
            }
            assert!(!about.title.is_empty());
        }
        assert!(preset_about("nope").is_err() && preset_chain("nope").is_none());
    }

    #[test]
    fn the_accent_surfaces_come_from_every_preset_in_both_schemes() {
        let s = accent_surfaces();
        assert!(s.dark.text.len() >= 8 && s.light.text.len() >= 8, "{s:?}");
        assert!(s.dark.line.len() >= 10 && s.light.line.len() >= 10, "the flat presets add their rings: {s:?}");
        assert!(std::ptr::eq(s, accent_surfaces()), "built once");
    }

    #[test]
    fn every_embedded_text_file_has_unix_line_endings() {
        // The files a first start writes (bar.toml and style.css) must not differ in line ending,
        // and a Windows checkout must not leak CRLF into what is embedded.
        let mut checked = 0;
        for (name, bytes) in EMBEDDED {
            let text_file = [".toml", ".css", ".luau", ".md"].iter().any(|e| name.ends_with(e));
            if text_file {
                assert!(!bytes.windows(2).any(|w| w == b"\r\n"), "{name} has CRLF line endings");
                checked += 1;
            }
        }
        assert!(checked >= 20, "only {checked} text files were checked");
    }
}

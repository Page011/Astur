//! The files inside the binary (everything under `assets/`) and the widget
//! source store the script thread reads from: built-ins from here, user
//! widgets from the config folder.

use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};

use rsb_core::paths::MAX_SOURCE_BYTES;
use rsb_script::SourceStore;

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

/// An embedded file as text.
pub fn file(rel: &str) -> Option<&'static str> {
    EMBEDDED.iter().find(|(name, _)| *name == rel).and_then(|(_, bytes)| std::str::from_utf8(bytes).ok())
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

impl SourceStore for Store {
    fn builtin(&self, rel_path: &str) -> Option<Cow<'static, str>> {
        file(rel_path).map(Cow::Borrowed)
    }

    fn builtin_widgets(&self) -> Vec<String> {
        builtin_widget_names()
    }

    fn user(&self, rel_path: &str) -> std::io::Result<Option<String>> {
        // The script runtime only asks for resolved paths inside `widgets/`;
        // this guards the folder boundary anyway.
        let rel = Path::new(rel_path);
        if rel.is_absolute() || rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return Ok(None);
        }
        match crate::cfgload::read_capped(&self.config_dir.join(rel), MAX_SOURCE_BYTES) {
            Ok(text) => Ok(text),
            Err(e) => Err(std::io::Error::other(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsb_core::config::{BUILTIN_PRESETS, load_config};
    use rsb_core::css::Stylesheet;

    #[test]
    fn every_embedded_text_file_is_utf8_and_the_templates_exist() {
        for (name, bytes) in EMBEDDED {
            assert!(std::str::from_utf8(bytes).is_ok(), "{name} is not UTF-8");
        }
        for must in ["config/bar.toml", "config/style.css", "themes/base.css", "widgets/clock.luau"] {
            assert!(file(must).is_some(), "{must} is not embedded");
        }
    }

    #[test]
    fn the_eleven_builtins_are_listed_and_lib_files_are_not_widgets() {
        let names = builtin_widget_names();
        for w in
            ["workspaces", "title", "clock", "date", "cpu", "memory", "battery", "volume", "network", "media", "text"]
        {
            assert!(names.iter().any(|n| n == w), "{w} missing from {names:?}");
        }
        assert_eq!(names.len(), 11, "{names:?}");
        assert!(!names.iter().any(|n| n.contains("lib") || n == "format"));
    }

    #[test]
    fn presets_and_themes_parse_without_a_diagnostic() {
        for preset in BUILTIN_PRESETS {
            let toml = preset_toml(preset).unwrap_or_else(|| panic!("no toml for {preset}"));
            let load = load_config(Some((toml, "preset.toml")), &|n| preset_toml(n).map(str::to_string));
            assert!(load.config.is_some() && load.diagnostics.is_empty(), "{preset}: {:?}", load.diagnostics);
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
    }
}

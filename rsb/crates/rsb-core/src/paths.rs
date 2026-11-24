//! Config folder layout (no file access here; the binary does that).
//!
//! ```text
//! <config>\bar.toml          bars, zones, widget options
//! <config>\style.css         user CSS, above the preset's
//! <config>\widgets\          user widgets; widgets\clock.luau overrides the built-in clock
//! <config>\types\rsb.d.luau  types for luau-lsp (rewritten by `rsb types`)
//! <config>\.luaurc           alias @rsb -> types/rsb.d
//! <config>\.vscode\settings.json
//! ```

use crate::config::WidgetRef;

pub const BAR_TOML: &str = "bar.toml";
pub const STYLE_CSS: &str = "style.css";
pub const WIDGETS_DIR: &str = "widgets";
pub const TYPES_DIR: &str = "types";
pub const TYPES_FILE: &str = "types/rsb.d.luau";
pub const LUAURC: &str = ".luaurc";
pub const VSCODE_SETTINGS: &str = ".vscode/settings.json";
/// Largest config, style or widget file read; bigger is an error.
pub const MAX_SOURCE_BYTES: usize = 1 << 20;

/// Files that may hold a widget, in lookup order, relative to the config
/// folder with `/`: `clock` -> `widgets/clock.luau`; `my/weather` ->
/// `widgets/my/weather.luau`, `widgets/my/weather/init.luau`.
/// A ref without `/` that has no file is a built-in.
pub fn widget_files(r: &WidgetRef) -> Vec<String> {
    let base = format!("{WIDGETS_DIR}/{}", r.name);
    if r.is_path() { vec![format!("{base}.luau"), format!("{base}/init.luau")] } else { vec![format!("{base}.luau")] }
}

/// Resolves `require("./x")` / `require("../lib/x")` from the module at
/// `from` (relative to the config folder or the assets folder, `/`
/// separators, e.g. `widgets/my/weather.luau`). The result must stay inside
/// `root`, the folder that holds the widget's entry file (`widgets/my`), or
/// for a top-level widget file the `widgets` folder itself, so
/// `widgets/clock.luau` can use `widgets/lib/x.luau`. Returns the module
/// path without an extension (`widgets/my/lib/x`); the runtime tries
/// `<path>.luau`, then `<path>/init.luau`. An absolute path, a drive, a UNC
/// path, a `:` or a `\` is refused with a message. `"@rsb"` is handled by
/// the runtime before this is called.
pub fn resolve_require(root: &str, from: &str, spec: &str) -> Result<String, String> {
    let refuse = |why: &str| Err(format!("require(\"{spec}\"): {why}"));
    if !(spec.starts_with("./") || spec.starts_with("../")) {
        return refuse("use \"@rsb\" or a path starting with ./ or ../ inside the widget's folder");
    }
    if spec.contains(['\\', ':']) || spec.chars().any(char::is_control) {
        return refuse("use / between folders, and no drive or ':'");
    }
    let spec = spec.strip_suffix(".luau").unwrap_or(spec);
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for seg in spec.split('/') {
        match seg {
            "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return refuse("leaves the widget's folder");
                }
            }
            "" => return refuse("empty folder name"),
            s if s.ends_with('.') || s.ends_with(' ') => return refuse("a name ends with a dot or a space"),
            s => parts.push(s),
        }
    }
    let resolved = parts.join("/");
    let root = root.trim_end_matches('/');
    match resolved.strip_prefix(root) {
        Some(rest) if rest.starts_with('/') && rest.len() > 1 => Ok(resolved),
        _ => refuse("leaves the widget's folder"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widget_files_for_builtin_and_path_refs() {
        let clock = WidgetRef { name: "clock".into(), instance: Some("utc".into()) };
        assert_eq!(widget_files(&clock), vec!["widgets/clock.luau"]);
        let w = WidgetRef { name: "my/weather".into(), instance: None };
        assert_eq!(widget_files(&w), vec!["widgets/my/weather.luau", "widgets/my/weather/init.luau"]);
    }

    #[test]
    fn require_paths() {
        assert_eq!(resolve_require("widgets/my", "widgets/my/weather.luau", "./lib/x"), Ok("widgets/my/lib/x".into()));
        assert_eq!(resolve_require("widgets/my", "widgets/my/lib/x.luau", "../util"), Ok("widgets/my/util".into()));
        assert_eq!(
            resolve_require("widgets", "widgets/clock.luau", "./lib/format.luau"),
            Ok("widgets/lib/format".into())
        );
        assert_eq!(resolve_require("widgets/my", "widgets/my/a.luau", "./b/./c"), Ok("widgets/my/b/c".into()));
        for bad in ["../other/x", "./../../x", "lib/x", "/x", "C:/x", "./a\\b", "./a:b", "./a//b", "./a./b", "@rsb2"] {
            assert!(resolve_require("widgets/my", "widgets/my/weather.luau", bad).is_err(), "{bad}");
        }
        assert!(resolve_require("widgets/my", "widgets/my/weather.luau", "../my2/x").unwrap_err().contains("leaves"));
        assert!(resolve_require("widgets/my", "widgets/my/weather.luau", "./").is_err());
    }
}

//! The widget listing: every built-in widget and every file under the
//! config folder's `widgets\`, each described by `rsb_script::describe`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use rsb_core::config::WidgetRef;
use rsb_core::paths::{MAX_SOURCE_BYTES, WIDGETS_DIR, widget_files};

use super::backend::{Listing, WidgetFailure, WidgetView};
use crate::{assets, cfgload};

/// Most widget files read, and how deep the folders may go.
const MAX_USER_FILES: usize = 200;
const MAX_DEPTH: usize = 6;

/// One file under `widgets\`.
struct UserFile {
    /// Relative to the config folder, `/` separators.
    rel: String,
    text: Result<String, String>,
}

/// The files under `widgets\` by the widget name they would have, and the
/// files whose name cannot be a widget.
fn scan(dir: &Path) -> (BTreeMap<String, UserFile>, Vec<WidgetFailure>) {
    let mut found = BTreeMap::new();
    let mut failed = Vec::new();
    let mut count = 0usize;
    let root = dir.join(WIDGETS_DIR);
    let mut stack = vec![(root.clone(), 0usize)];
    while let Some((folder, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            // A link could lead anywhere: never followed.
            let Ok(kind) = std::fs::symlink_metadata(&path).map(|m| m.file_type()) else { continue };
            if kind.is_symlink() {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy().to_string();
            if kind.is_dir() {
                if depth < MAX_DEPTH && file_name != "lib" {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            let Some(stem) = file_name.strip_suffix(".luau") else { continue };
            let Ok(relative) = path.strip_prefix(&root) else { continue };
            let parts: Vec<String> = relative.iter().map(|p| p.to_string_lossy().to_string()).collect();
            let rel = format!("{WIDGETS_DIR}/{}", parts.join("/"));
            let name = if stem == "init" && parts.len() > 1 {
                parts[..parts.len() - 1].join("/")
            } else {
                let mut p = parts.clone();
                if let Some(last) = p.last_mut() {
                    *last = stem.to_string();
                }
                p.join("/")
            };
            count += 1;
            if count > MAX_USER_FILES {
                failed.push(WidgetFailure {
                    name: String::new(),
                    source: WIDGETS_DIR.into(),
                    message: format!("more than {MAX_USER_FILES} widget files; the rest are not listed"),
                });
                return (found, failed);
            }
            if name.contains('#') || WidgetRef::parse(&name).is_err() {
                let why = WidgetRef::parse(&name).err().unwrap_or_else(|| "a widget name has no '#'".into());
                failed.push(WidgetFailure { name, source: rel, message: why });
                continue;
            }
            let text = cfgload::read_capped(&path, MAX_SOURCE_BYTES).map(Option::unwrap_or_default);
            found.entry(name).or_insert(UserFile { rel, text });
        }
    }
    (found, failed)
}

/// Every widget with what its module declares.
pub fn list(dir: &Path) -> Listing {
    let (user, mut failed) = scan(dir);
    let builtin = assets::builtin_widget_names();
    let names: BTreeSet<String> = builtin.iter().cloned().chain(user.keys().cloned()).collect();
    let mut widgets = Vec::new();
    for name in names {
        let (source, rel, is_builtin) = match user.get(&name) {
            Some(u) => (u.text.clone(), u.rel.clone(), false),
            None => (
                assets::file(&format!("{WIDGETS_DIR}/{name}.luau")).map(str::to_string).ok_or("missing".to_string()),
                "builtin".to_string(),
                true,
            ),
        };
        let described = source.and_then(|text| rsb_script::describe::widget(&name, &text));
        match described {
            Ok(about) => widgets.push(WidgetView {
                replaces_builtin: !is_builtin && builtin.contains(&name),
                builtin: is_builtin,
                name,
                about,
            }),
            Err(message) => failed.push(WidgetFailure { name, source: rel, message }),
        }
    }
    failed.sort_by(|a, b| a.source.cmp(&b.source));
    Listing { widgets, failed }
}

/// The popups a widget opens, from its `about` table; none when it cannot
/// be read.
pub fn opens_of(dir: &Path, name: &str) -> Vec<String> {
    let Ok(r) = WidgetRef::parse(name) else { return Vec::new() };
    let mut text = None;
    for rel in widget_files(&r) {
        let path = dir.join(rel.replace('/', "\\"));
        if path.is_file() {
            text = cfgload::read_capped(&path, MAX_SOURCE_BYTES).ok().flatten();
            break;
        }
    }
    let text = text.or_else(|| assets::file(&format!("{WIDGETS_DIR}/{name}.luau")).map(str::to_string));
    let Some(text) = text else { return Vec::new() };
    rsb_script::describe::widget(name, &text)
        .ok()
        .and_then(|d| d["opens"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rsb-widgets-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("widgets")).unwrap();
        dir
    }

    const GOOD: &str = "-- A weather panel. More text.\nlocal rsb = require(\"@rsb\")\nreturn rsb.widget { options = { city = \"x\" }, setup = function() end }\n";

    #[test]
    fn built_ins_and_user_files_are_listed_with_what_they_declare() {
        let dir = scratch("list");
        std::fs::create_dir_all(dir.join("widgets").join("my")).unwrap();
        std::fs::write(dir.join("widgets").join("my").join("weather.luau"), GOOD).unwrap();
        std::fs::write(dir.join("widgets").join("clock.luau"), GOOD).unwrap();
        std::fs::write(dir.join("widgets").join("Bad Name.luau"), GOOD).unwrap();
        std::fs::write(dir.join("widgets").join("broken.luau"), "return 5").unwrap();
        std::fs::create_dir_all(dir.join("widgets").join("lib")).unwrap();
        std::fs::write(dir.join("widgets").join("lib").join("helper.luau"), "return 1").unwrap();
        let listing = list(&dir);
        let builtin = assets::builtin_widget_names();
        let names: Vec<&str> = listing.widgets.iter().map(|w| w.name.as_str()).collect();
        for b in &builtin {
            assert!(names.contains(&b.as_str()) || *b == "broken", "{b} listed");
        }
        let weather = listing.widgets.iter().find(|w| w.name == "my/weather").unwrap();
        assert!(!weather.builtin && !weather.replaces_builtin);
        assert_eq!(weather.about["summary"], "A weather panel.");
        let clock = listing.widgets.iter().find(|w| w.name == "clock").unwrap();
        assert!(!clock.builtin && clock.replaces_builtin, "a user file named like a built-in replaces it");
        assert!(!names.contains(&"helper") && !names.contains(&"lib/helper"), "lib folders hold helpers, not widgets");
        let failed: Vec<(&str, &str)> = listing.failed.iter().map(|f| (f.name.as_str(), f.source.as_str())).collect();
        assert!(failed.contains(&("broken", "widgets/broken.luau")), "{failed:?}");
        assert!(failed.iter().any(|(_, s)| *s == "widgets/Bad Name.luau"), "{failed:?}");
    }

    #[test]
    fn every_built_in_declares_an_about_table() {
        let dir = scratch("builtin");
        let listing = list(&dir);
        assert!(listing.failed.is_empty(), "{:?}", listing.failed);
        let builtin = assets::builtin_widget_names();
        assert_eq!(listing.widgets.len(), builtin.len());
        for w in &listing.widgets {
            let source = assets::file(&format!("widgets/{}.luau", w.name)).unwrap();
            assert!(source.contains("about = {"), "{} has no about table", w.name);
            let title = w.about["title"].as_str().unwrap();
            assert!(!title.is_empty() && title.chars().count() <= 20, "{}: {title}", w.name);
        }
        let find = |n: &str| listing.widgets.iter().find(|w| w.name == n).unwrap();
        assert_eq!(find("power").about["visible"], false);
        assert_eq!(find("power").about["opens"], serde_json::json!(["power"]));
        assert_eq!(find("text").about["needs_options"], true);
        assert_eq!(find("dashboard").about["opens"], serde_json::json!(["dashboard", "launcher", "power"]));
        assert_eq!(find("clock").about["visible"], true);
        for n in ["date", "media", "network", "volume"] {
            assert_eq!(find(n).about["opens"], serde_json::json!([n.replace("date", "calendar")]), "{n}");
        }
    }

    #[test]
    fn opens_of_reads_the_users_file_before_the_built_in() {
        let dir = scratch("opens");
        assert_eq!(opens_of(&dir, "power"), ["power"]);
        assert!(opens_of(&dir, "clock").is_empty());
        assert!(opens_of(&dir, "nothing-here").is_empty());
        let mine = "local rsb = require(\"@rsb\")\nreturn rsb.widget { about = { opens = { \"mine\" } }, setup = function() end }\n";
        std::fs::write(dir.join("widgets").join("power.luau"), mine).unwrap();
        assert_eq!(opens_of(&dir, "power"), ["mine"]);
    }
}

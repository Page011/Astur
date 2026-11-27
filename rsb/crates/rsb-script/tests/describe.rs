//! Every built-in widget can be described without running it, and says what
//! a settings page needs to know: a short title, whether it draws anything,
//! and which popups it opens.

use std::collections::BTreeSet;
use std::path::PathBuf;

use rsb_core::popup::BUILTIN_POPUP_NAMES;
use rsb_script::describe;

fn widgets() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("assets").join("widgets");
    let mut out: Vec<(String, String)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().strip_suffix(".luau")?.to_string();
            Some((name, std::fs::read_to_string(e.path()).ok()?))
        })
        .collect();
    out.sort();
    out
}

#[test]
fn every_built_in_describes_itself_with_an_about_table() {
    let all = widgets();
    assert!(all.len() >= 16, "{} widgets found", all.len());
    let mut titles = BTreeSet::new();
    for (name, source) in &all {
        assert!(source.contains("about = {"), "{name} has no about table");
        let d = describe::widget(name, source).unwrap_or_else(|e| panic!("{name}: {e}"));
        let title = d["title"].as_str().unwrap();
        assert!(!title.is_empty() && title.chars().count() <= 20, "{name}: '{title}'");
        assert!(titles.insert(title.to_string()), "two widgets are titled '{title}'");
        assert!(d["visible"].is_boolean() && d["needs_options"].is_boolean() && d["opens"].is_array(), "{name}: {d}");
        let summary = d["summary"].as_str().unwrap();
        assert!(summary.ends_with('.') && summary.len() > 5, "{name}: '{summary}'");
        let keys: Vec<&str> = d["options"].as_array().unwrap().iter().map(|o| o["key"].as_str().unwrap()).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted, "{name}: options are listed alphabetically");
        for option in d["options"].as_array().unwrap() {
            let ty = option["type"].as_str().unwrap();
            assert!(["string", "number", "bool", "table", "any"].contains(&ty), "{name}: {option}");
            assert_eq!(ty == "any", option["default"].is_null(), "{name}: {option}");
        }
    }
}

#[test]
fn the_popups_the_built_ins_open_are_exactly_the_built_in_popups() {
    let mut opened = BTreeSet::new();
    for (name, source) in widgets() {
        let d = describe::widget(&name, &source).unwrap();
        for popup in d["opens"].as_array().unwrap() {
            let popup = popup.as_str().unwrap();
            assert!(BUILTIN_POPUP_NAMES.contains(&popup), "{name} opens '{popup}', which is not a built-in popup");
            opened.insert(popup.to_string());
        }
    }
    let wanted: BTreeSet<String> = BUILTIN_POPUP_NAMES.iter().map(|s| (*s).to_string()).collect();
    assert_eq!(opened, wanted, "every built-in popup has a widget that opens it");
}

#[test]
fn an_anchor_is_invisible_and_a_widget_that_needs_options_says_so() {
    let get = |n: &str| {
        let (name, source) = widgets().into_iter().find(|(name, _)| name == n).unwrap();
        describe::widget(&name, &source).unwrap()
    };
    assert_eq!(get("power")["visible"], false);
    assert_eq!(get("text")["needs_options"], true);
    for plain in ["clock", "cpu", "memory", "title", "battery"] {
        let d = get(plain);
        assert_eq!((d["visible"].clone(), d["needs_options"].clone()), (true.into(), false.into()), "{plain}");
    }
}

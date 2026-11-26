//! `default_config()` is the bar of the `default` preset: it is what runs
//! when there is no `bar.toml` and no preset file can be read.

use rsb_core::config::*;

const DEFAULT_PRESET: &str = include_str!("../../../assets/presets/default.toml");

fn names(v: &[ZoneItem]) -> Vec<String> {
    v.iter().map(|i| i.widget.to_string()).collect()
}

fn presets(name: &str) -> Option<String> {
    (name == "default").then(|| DEFAULT_PRESET.to_string())
}

#[test]
fn the_built_in_bar_has_the_new_default_look() {
    let c = default_config();
    let b = &c.bars[0];
    assert_eq!(b.name, "main");
    assert_eq!((b.height, b.floating, b.reserve), (36, false, true));
    assert_eq!((b.position, b.fullscreen), (Position::Top, FullscreenMode::Below));
    assert_eq!(names(&b.left), ["workspaces", "media"]);
    assert_eq!(names(&b.center), ["memory", "cpu", "dashboard", "clock", "date"]);
    assert_eq!(names(&b.right), ["status", "battery", "tray", "record", "power"]);
    assert_eq!(b.center_on.as_deref(), Some("dashboard"));
}

/// A bar without the source positions the preset's zone entries carry.
fn bare(b: &BarSpec) -> BarSpec {
    let strip = |v: &[ZoneItem]| v.iter().map(|i| ZoneItem::new(i.widget.clone())).collect::<Vec<_>>();
    BarSpec { left: strip(&b.left), center: strip(&b.center), right: strip(&b.right), pos: None, ..b.clone() }
}

#[test]
fn it_equals_the_bar_the_default_preset_describes() {
    let from_preset = load_config(None, &presets);
    assert!(from_preset.diagnostics.is_empty(), "{:?}", from_preset.diagnostics);
    let preset_bar = bare(&from_preset.config.unwrap().bars[0]);
    assert_eq!(preset_bar, bare(&default_config().bars[0]));
}

#[test]
fn it_equals_what_an_empty_bar_toml_loads_to() {
    let none = |_: &str| None;
    let l = load_config(Some(("", "bar.toml")), &none);
    assert_eq!(l.config.unwrap(), default_config());
}

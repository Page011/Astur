//! `center_on` from a preset: it applies only when the named widget is in the
//! effective centre zone, and a user who replaced `center` is not warned
//! about a key they never wrote. A `center_on` the user wrote still is.

use rsb_core::config::*;

const DEFAULT_PRESET: &str = include_str!("../../../assets/presets/default.toml");

fn presets(name: &str) -> Option<String> {
    (name == "default").then(|| DEFAULT_PRESET.to_string())
}

fn load(user: Option<&str>) -> ConfigLoad {
    load_config(user.map(|t| (t, "bar.toml")), &presets)
}

fn msgs(l: &ConfigLoad) -> Vec<String> {
    l.diagnostics.iter().map(|d| d.to_string()).collect()
}

#[test]
fn the_default_preset_pins_the_dashboard() {
    let l = load(None);
    assert!(l.diagnostics.is_empty(), "{:?}", msgs(&l));
    assert_eq!(l.config.unwrap().bars[0].center_on.as_deref(), Some("dashboard"));
}

#[test]
fn a_replaced_centre_without_the_pinned_widget_drops_the_pin_silently() {
    let l = load(Some("[[bar]]\ncenter = [\"clock\"]\n"));
    assert!(l.diagnostics.is_empty(), "{:?}", msgs(&l));
    let c = l.config.unwrap();
    assert_eq!(c.bars[0].center_on, None);
    assert_eq!(c.bars[0].center.len(), 1);
}

#[test]
fn a_replaced_centre_that_still_holds_the_widget_keeps_the_pin() {
    let l = load(Some("[[bar]]\ncenter = [\"clock\", \"dashboard\"]\n"));
    assert!(l.diagnostics.is_empty(), "{:?}", msgs(&l));
    assert_eq!(l.config.unwrap().bars[0].center_on.as_deref(), Some("dashboard"));
}

#[test]
fn an_empty_centre_and_a_bar_with_no_centre_key_are_quiet_too() {
    let l = load(Some("[[bar]]\ncenter = []\n"));
    assert!(l.diagnostics.is_empty(), "{:?}", msgs(&l));
    assert_eq!(l.config.unwrap().bars[0].center_on, None);
    let l = load(Some("[[bar]]\nheight = 40\n"));
    assert!(l.diagnostics.is_empty(), "{:?}", msgs(&l));
    assert_eq!(l.config.unwrap().bars[0].center_on.as_deref(), Some("dashboard"));
}

#[test]
fn a_second_bar_without_the_pinned_widget_is_quiet() {
    let l = load(Some("[[bar]]\nname = \"a\"\n\n[[bar]]\nname = \"b\"\ncenter = [\"clock\"]\n"));
    assert!(l.diagnostics.is_empty(), "{:?}", msgs(&l));
    let c = l.config.unwrap();
    assert_eq!(c.bars[0].center_on.as_deref(), Some("dashboard"));
    assert_eq!(c.bars[1].center_on, None);
}

#[test]
fn a_center_on_the_user_wrote_still_warns_with_its_position() {
    let text = "[[bar]]\ncenter = [\"clock\"]\ncenter_on = \"dashboard\"\n";
    let l = load(Some(text));
    let d = msgs(&l);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].contains("'center_on' names 'dashboard', which is not in center"), "{d:?}");
    assert!(d[0].contains("bar.toml:3"), "the warning names the line of the key: {d:?}");
    assert_eq!(l.config.unwrap().bars[0].center_on, None);
}

#[test]
fn a_user_center_on_beats_the_preset_pin() {
    let l = load(Some("[[bar]]\ncenter = [\"clock\", \"date\"]\ncenter_on = \"date\"\n"));
    assert!(l.diagnostics.is_empty(), "{:?}", msgs(&l));
    assert_eq!(l.config.unwrap().bars[0].center_on.as_deref(), Some("date"));
}

#[test]
fn a_user_center_on_in_a_second_bar_warns_only_for_that_bar() {
    let l = load(Some("[[bar]]\nname = \"a\"\n\n[[bar]]\nname = \"b\"\ncenter = [\"clock\"]\ncenter_on = \"cpu\"\n"));
    let d = msgs(&l);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].contains("'center_on' names 'cpu'"), "{d:?}");
}

#[test]
fn without_any_preset_the_built_in_pin_follows_the_same_rule() {
    let none = |_: &str| None;
    let l = load_config(Some(("[[bar]]\ncenter = [\"clock\"]\n", "bar.toml")), &none);
    assert!(l.diagnostics.is_empty(), "{:?}", msgs(&l));
    assert_eq!(l.config.unwrap().bars[0].center_on, None);
    let l = load_config(None, &none);
    assert_eq!(l.config.unwrap(), default_config());
}

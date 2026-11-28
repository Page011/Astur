//! `settings schema`: every key the settings commands may change, with its
//! type and limits, built from the parser's own tables so no limit is
//! written twice. `docs/settings-schema.json` is the committed output.

use rsb_core::config::{BAR_SPECS, BUILTIN_PRESETS, CONFIG_VERSION, KeyKind, KeySpec};
use serde_json::{Value, json};

/// Most refs a zone list may be set to.
pub const MAX_ZONE_ITEMS: usize = 64;

/// The schema for the parser's real key table.
pub fn schema() -> Value {
    schema_of(BAR_SPECS, &default_bar_values())
}

/// What a bar that sets nothing has: position, height, floating.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BarDefaults {
    pub position: String,
    pub height: u32,
    pub floating: bool,
    /// The widgets of the built-in bar's left, centre and right zones.
    pub left: Vec<String>,
    pub center: Vec<String>,
    pub right: Vec<String>,
}

/// The built-in bar's values, read from the parser's own defaults.
pub fn default_bar_values() -> BarDefaults {
    let config = rsb_core::config::load_config(None, &|_| None).config;
    let bar = config.as_ref().and_then(|c| c.bars.first());
    BarDefaults {
        position: match bar.map(|b| b.position) {
            Some(rsb_core::config::Position::Bottom) => "bottom".into(),
            _ => "top".into(),
        },
        height: bar.map_or(36, |b| b.height),
        floating: bar.is_some_and(|b| b.floating),
        left: bar.map(|b| names(&b.left)).unwrap_or_default(),
        center: bar.map(|b| names(&b.center)).unwrap_or_default(),
        right: bar.map(|b| names(&b.right)).unwrap_or_default(),
    }
}

fn names(zone: &[rsb_core::config::ZoneItem]) -> Vec<String> {
    zone.iter().map(|z| z.widget.to_string()).collect()
}

fn spec<'a>(specs: &'a [KeySpec], name: &str) -> Option<&'a KeySpec> {
    specs.iter().find(|s| s.name == name)
}

/// The schema for a key table (tests pass their own).
pub fn schema_of(specs: &[KeySpec], defaults: &BarDefaults) -> Value {
    let mut keys = vec![json!({
        "key": "preset", "file": "bar.toml", "type": "choice",
        "label": "Preset", "help": "The look and the layout the bar starts from",
        "values": BUILTIN_PRESETS, "default": "default",
    })];
    let bar = |name: &str| format!("bar.*.{name}");
    if let Some(KeySpec { kind: KeyKind::Choice(values), .. }) = spec(specs, "position") {
        keys.push(json!({
            "key": bar("position"), "file": "bar.toml", "type": "choice",
            "label": "Position", "help": "The screen edge the bar sits on",
            "values": values, "default": defaults.position,
        }));
    }
    if let Some(KeySpec { kind: KeyKind::Int { lo, hi }, .. }) = spec(specs, "height") {
        keys.push(json!({
            "key": bar("height"), "file": "bar.toml", "type": "int",
            "label": "Height", "help": "How tall the bar is",
            "min": lo, "max": hi, "unit": "logical px", "default": defaults.height,
        }));
    }
    if spec(specs, "floating").is_some() {
        keys.push(json!({
            "key": bar("floating"), "file": "bar.toml", "type": "bool",
            "label": "Floating", "help": "Keep the bar clear of the screen edge",
            "default": defaults.floating,
        }));
    }
    for (zone, label, default) in [
        ("left", "Left zone", &defaults.left),
        ("center", "Centre zone", &defaults.center),
        ("right", "Right zone", &defaults.right),
    ] {
        if spec(specs, zone).is_some() {
            keys.push(json!({
                "key": bar(zone), "file": "bar.toml", "type": "zone", "max_items": MAX_ZONE_ITEMS,
                "label": label, "help": "The widgets in this part of the bar, in order",
                "default": default,
            }));
        }
    }
    keys.push(json!({
        "key": "color.accent", "file": "style.css", "type": "color", "schemes": ["dark", "light"],
        "label": "Accent", "help": "The highlight colour; every shade of it is worked out so text stays readable",
    }));
    let mut file_only: Vec<String> = specs.iter().filter(|s| !s.editable).map(|s| bar(s.name)).collect();
    file_only.sort();
    let mut all = vec!["config_version".to_string()];
    all.extend(file_only);
    all.push("bar".into());
    all.push("widget.*".into());
    json!({"kind": "schema", "config_version": CONFIG_VERSION, "keys": keys, "file_only": all})
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The key table the parser is expected to have.
    pub fn expected_specs() -> Vec<KeySpec> {
        let spec = |name, kind, editable| KeySpec { name, kind, editable };
        vec![
            spec("name", KeyKind::Name, false),
            spec("monitors", KeyKind::Monitors { max_index: 15 }, false),
            spec("position", KeyKind::Choice(&["top", "bottom"]), true),
            spec("height", KeyKind::Int { lo: 16, hi: 128 }, true),
            spec("floating", KeyKind::Bool, true),
            spec("margin", KeyKind::Int { lo: 0, hi: 64 }, false),
            spec("radius", KeyKind::Int { lo: 0, hi: 64 }, false),
            spec("reserve", KeyKind::Bool, false),
            spec("fullscreen", KeyKind::Choice(&["below", "hide", "stay"]), false),
            spec("left", KeyKind::Zone, true),
            spec("center", KeyKind::Zone, true),
            spec("right", KeyKind::Zone, true),
            spec("center_on", KeyKind::Text, false),
        ]
    }

    #[test]
    fn every_editable_row_is_listed_with_its_limits() {
        let v = schema_of(
            &expected_specs(),
            &BarDefaults { position: "top".into(), height: 36, floating: false, ..BarDefaults::default() },
        );
        let keys: Vec<&str> = v["keys"].as_array().unwrap().iter().map(|k| k["key"].as_str().unwrap()).collect();
        assert_eq!(
            keys,
            [
                "preset",
                "bar.*.position",
                "bar.*.height",
                "bar.*.floating",
                "bar.*.left",
                "bar.*.center",
                "bar.*.right",
                "color.accent"
            ]
        );
        let height = &v["keys"][2];
        assert_eq!((height["min"].clone(), height["max"].clone()), (json!(16), json!(128)));
        assert_eq!(v["keys"][0]["values"], json!(["default", "fluent", "astur-classic", "glass"]));
        assert_eq!(v["keys"][4]["max_items"], 64);
        assert_eq!(
            v["file_only"],
            json!([
                "config_version",
                "bar.*.center_on",
                "bar.*.fullscreen",
                "bar.*.margin",
                "bar.*.monitors",
                "bar.*.name",
                "bar.*.radius",
                "bar.*.reserve",
                "bar",
                "widget.*"
            ])
        );
    }

    #[test]
    fn a_key_the_parser_does_not_have_is_not_offered() {
        let only_height: Vec<KeySpec> = expected_specs().into_iter().filter(|s| s.name == "height").collect();
        let v = schema_of(
            &only_height,
            &BarDefaults { position: "top".into(), height: 36, floating: false, ..BarDefaults::default() },
        );
        let keys: Vec<&str> = v["keys"].as_array().unwrap().iter().map(|k| k["key"].as_str().unwrap()).collect();
        assert_eq!(keys, ["preset", "bar.*.height", "color.accent"]);
    }
}

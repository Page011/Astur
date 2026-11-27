//! `BAR_SPECS` is the one list of `[[bar]]` keys and limits: the parser reads
//! its limits from it, and each setting records which layer supplied it.

use rsb_core::config::{BAR_KEYS, BAR_SPECS, KeyKind, MonitorSelector, load_config, load_config_sourced};
use rsb_core::contract::Source;

fn no_preset(_: &str) -> Option<String> {
    None
}

fn bar_with(line: &str) -> String {
    format!("[[bar]]\n{line}\n")
}

fn warnings(text: &str) -> Vec<String> {
    let l = load_config(Some((text, "bar.toml")), &no_preset);
    assert!(l.config.is_some(), "{text}");
    l.diagnostics.iter().map(|d| d.message.clone()).collect()
}

/// A value of the right kind for a key, and ones just outside its limits.
fn sample(kind: KeyKind) -> String {
    match kind {
        KeyKind::Int { lo, .. } => lo.to_string(),
        KeyKind::Bool => "true".into(),
        KeyKind::Choice(words) => format!("\"{}\"", words[0]),
        KeyKind::Monitors { .. } => "\"*\"".into(),
        KeyKind::Zone => "[\"clock\"]".into(),
        KeyKind::Name => "\"side\"".into(),
        KeyKind::Text => "\"clock\"".into(),
    }
}

#[test]
fn bar_specs_limits_load() {
    for spec in BAR_SPECS {
        match spec.kind {
            KeyKind::Int { lo, hi } => {
                for ok in [lo, hi, (lo + hi) / 2] {
                    let w = warnings(&bar_with(&format!("{} = {ok}", spec.name)));
                    assert!(w.is_empty(), "{} = {ok}: {w:?}", spec.name);
                }
                for bad in [lo - 1, hi + 1] {
                    let w = warnings(&bar_with(&format!("{} = {bad}", spec.name)));
                    assert_eq!(w.len(), 1, "{} = {bad}: {w:?}", spec.name);
                    assert!(w[0].contains(&format!("must be {lo} to {hi}")), "{}", w[0]);
                }
            }
            KeyKind::Choice(words) => {
                for word in words.iter() {
                    for text in [word.to_string(), word.to_ascii_uppercase()] {
                        let w = warnings(&bar_with(&format!("{} = \"{text}\"", spec.name)));
                        assert!(w.is_empty(), "{} = {text}: {w:?}", spec.name);
                    }
                }
                let w = warnings(&bar_with(&format!("{} = \"sideways\"", spec.name)));
                assert_eq!(w.len(), 1, "{w:?}");
                assert!(words.iter().all(|word| w[0].contains(&format!("\"{word}\""))), "{}", w[0]);
            }
            KeyKind::Monitors { max_index } => {
                assert!(warnings(&bar_with(&format!("{} = {max_index}", spec.name))).is_empty());
                let w = warnings(&bar_with(&format!("{} = {}", spec.name, max_index + 1)));
                assert!(w.len() == 1 && w[0].contains(&format!("0 to {max_index}")), "{w:?}");
            }
            KeyKind::Bool => {
                assert!(warnings(&bar_with(&format!("{} = false", spec.name))).is_empty());
                assert_eq!(warnings(&bar_with(&format!("{} = 3", spec.name))).len(), 1);
            }
            KeyKind::Zone => {
                assert!(warnings(&bar_with(&format!("{} = [\"clock\", \"my/weather\"]", spec.name))).is_empty());
                assert_eq!(warnings(&bar_with(&format!("{} = 3", spec.name))).len(), 1);
            }
            KeyKind::Name | KeyKind::Text => {
                assert!(warnings(&bar_with(&format!("{} = \"clock\"", spec.name))).len() <= 1);
            }
        }
    }
}

#[test]
fn every_spec_has_reader_arm() {
    let spec_names: Vec<&str> = BAR_SPECS.iter().map(|s| s.name).collect();
    let mut keys: Vec<&str> = BAR_KEYS.to_vec();
    let mut specs = spec_names.clone();
    keys.sort_unstable();
    specs.sort_unstable();
    assert_eq!(keys, specs, "BAR_KEYS and BAR_SPECS list the same keys");
    for spec in BAR_SPECS {
        let text = bar_with(&format!("{} = {}", spec.name, sample(spec.kind)));
        let l = load_config(Some((&text, "bar.toml")), &no_preset);
        assert!(
            l.diagnostics.iter().all(|d| !d.message.contains("unknown key")),
            "the reader has no arm for '{}': {:?}",
            spec.name,
            l.diagnostics
        );
    }
    let l = load_config(Some((&bar_with("hieght = 3"), "bar.toml")), &no_preset);
    assert!(l.diagnostics.iter().any(|d| d.message.contains("unknown key 'hieght'")));
    // The keys a settings change may set are the editable ones.
    let editable: Vec<&str> = BAR_SPECS.iter().filter(|s| s.editable).map(|s| s.name).collect();
    assert_eq!(editable, ["position", "height", "floating", "left", "center", "right"]);
    assert!(matches!(BAR_SPECS.iter().find(|s| s.name == "monitors").unwrap().kind, KeyKind::Monitors { .. }));
    let _ = MonitorSelector::All;
}

fn preset_two_bars(name: &str) -> Option<String> {
    (name == "wide").then(|| {
        "[[bar]]\nname = \"main\"\nheight = 40\nposition = \"bottom\"\nleft = [\"clock\"]\ncenter_on = \"clock\"\n\
         [[bar]]\nname = \"side\"\nheight = 50\n"
            .to_string()
    })
}

#[test]
fn each_setting_says_which_layer_it_came_from() {
    let user = "preset = \"wide\"\n\n[[bar]]\nheight = 44\nfloating = true\ncenter = [\"cpu\"]\n";
    let l = load_config_sourced(Some((user, "bar.toml")), &preset_two_bars);
    let (config, sources) = (l.load.config.unwrap(), l.sources.unwrap());
    assert_eq!(config.bars.len(), 1, "the user's list replaces the preset's");
    assert_eq!(
        (sources.preset.source, sources.preset.at.as_ref().map(|p| (p.line, p.col))),
        (Source::User, Some((1, 1)))
    );
    let b = &sources.bars[0];
    // The user wrote height, floating and center.
    for (field, source, at) in [
        ("height", b.height.source, b.height.at.as_ref()),
        ("floating", b.floating.source, b.floating.at.as_ref()),
        ("center", b.center.source, b.center.at.as_ref()),
    ] {
        assert_eq!(source, Source::User, "{field}");
        assert!(at.is_some_and(|p| &*p.file == "bar.toml"), "{field}");
    }
    assert_eq!(b.height.at.as_ref().map(|p| p.line), Some(4));
    assert_eq!((b.height.value, b.floating.value), (44, true));
    // The preset's own bar supplies name, position, left and the pin; the rest is built in.
    assert_eq!((b.name.source, b.position.source, b.left.source), (Source::Preset, Source::Preset, Source::Preset));
    assert_eq!(
        (b.center_on.source, b.center_on.value.as_deref()),
        (Source::Default, None),
        "the preset's pin is not in the user's centre, so nothing is pinned"
    );
    assert_eq!(
        (b.margin.source, b.reserve.source, b.monitors.source, b.right.source),
        (Source::Default, Source::Default, Source::Default, Source::Default)
    );
    assert!(b.height.at.is_some() && b.position.at.is_none());
}

#[test]
fn no_user_file_gives_preset_and_default_sources_and_a_second_bar_follows_the_presets_second() {
    let l = load_config_sourced(Some(("preset = \"wide\"\n", "bar.toml")), &preset_two_bars);
    let (config, sources) = (l.load.config.unwrap(), l.sources.unwrap());
    assert_eq!(config.bars.len(), 2);
    assert_eq!((sources.bars[0].height.source, sources.bars[1].height.source), (Source::Preset, Source::Preset));
    assert_eq!(sources.bars[1].name.value, "side");
    assert_eq!(sources.bars[0].floating.source, Source::Default);
    assert!(sources.bars.iter().all(|b| b.height.at.is_none()), "a preset value has no place in the user's file");
    let none = load_config_sourced(None, &no_preset);
    let s = none.sources.unwrap();
    assert_eq!((s.preset.source, s.bars[0].height.source), (Source::Default, Source::Default));
    // A pin that fits comes with its layer.
    let pinned = load_config_sourced(Some(("preset = \"wide\"\n", "bar.toml")), &preset_two_bars);
    let s = pinned.sources.unwrap();
    assert_eq!((s.bars[0].center_on.source, s.bars[0].center_on.value.as_deref()), (Source::Preset, Some("clock")));
}

#[test]
fn the_sourced_load_matches_the_plain_load() {
    let text = "preset = \"wide\"\n[[bar]]\nheight = 44\nfoo = 1\n";
    let plain = load_config(Some((text, "bar.toml")), &preset_two_bars);
    let sourced = load_config_sourced(Some((text, "bar.toml")), &preset_two_bars);
    assert_eq!(plain.config, sourced.load.config);
    assert_eq!(plain.diagnostics.len(), sourced.load.diagnostics.len());
}

#[test]
fn about_is_read_from_presets_only() {
    let with_about = "[about]\ntitle = \"Wide\"\n\n[[bar]]\nname = \"main\"\nheight = 40\n";
    let preset = |name: &str| (name == "wide").then(|| with_about.to_string());
    let l = load_config(Some(("preset = \"wide\"\n", "bar.toml")), &preset);
    assert!(l.diagnostics.is_empty(), "{:?}", l.diagnostics);
    assert_eq!(l.config.unwrap().bars[0].height, 40);
    let l = load_config(Some(("[about]\ntitle = \"Mine\"\n", "bar.toml")), &preset);
    assert_eq!(l.diagnostics.len(), 1, "{:?}", l.diagnostics);
    assert!(l.diagnostics[0].message.contains("'about' is read from presets only"), "{}", l.diagnostics[0].message);
    assert!(l.config.is_some());
    let bad = |name: &str| (name == "wide").then(|| "about = 3\n".to_string());
    let l = load_config(Some(("preset = \"wide\"\n", "bar.toml")), &bad);
    assert_eq!(l.diagnostics.len(), 1, "{:?}", l.diagnostics);
}

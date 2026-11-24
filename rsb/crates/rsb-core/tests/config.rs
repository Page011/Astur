use rsb_core::config::*;
use rsb_core::diag::Severity;

fn none(_: &str) -> Option<String> {
    None
}

fn load(text: &str) -> ConfigLoad {
    load_config(Some((text, "bar.toml")), &none)
}

fn msgs(l: &ConfigLoad) -> Vec<String> {
    l.diagnostics.iter().map(|d| d.to_string()).collect()
}

fn items(v: &[ZoneItem]) -> Vec<String> {
    v.iter().map(|i| i.widget.to_string()).collect()
}

#[test]
fn no_file_is_the_default_config() {
    let l = load_config(None, &none);
    assert!(l.diagnostics.is_empty());
    assert_eq!(l.config.unwrap(), default_config());
    let c = default_config();
    let b = &c.bars[0];
    assert_eq!((b.name.as_str(), b.height, b.floating, b.margin, b.radius, b.reserve), ("main", 32, false, 8, 0, true));
    assert_eq!(b.monitors, vec![MonitorSelector::All]);
    assert_eq!((b.position, b.fullscreen), (Position::Top, FullscreenMode::Below));
    assert_eq!(items(&b.left), ["workspaces", "title"]);
    assert_eq!(items(&b.center), ["clock"]);
    assert_eq!(items(&b.right), ["media", "network", "volume", "battery", "cpu", "memory", "date"]);
}

#[test]
fn an_empty_file_is_the_default_config_too() {
    let l = load("");
    assert_eq!(l.config.unwrap(), default_config());
}

#[test]
fn every_key_is_read() {
    let text = r#"
config_version = 1
preset = "default"

[[bar]]
name = "top_bar"
monitors = ["primary", 1, "DISPLAY3"]
position = "bottom"
height = 40
floating = true
margin = 12
radius = 6
reserve = false
fullscreen = "hide"
left = ["clock"]
center = []
right = ["cpu#a", "my/weather"]

[widget.clock]
format = "HH:mm"

[widget."cpu#a"]
warn = 70
every = "5s"
"#;
    let l = load(text);
    assert!(l.diagnostics.is_empty(), "{:?}", msgs(&l));
    let c = l.config.unwrap();
    let b = &c.bars[0];
    assert_eq!(b.name, "top_bar");
    assert_eq!(
        b.monitors,
        vec![MonitorSelector::Primary, MonitorSelector::Index(1), MonitorSelector::Device("\\\\.\\DISPLAY3".into())]
    );
    assert_eq!(
        (b.position, b.height, b.floating, b.margin, b.radius, b.reserve),
        (Position::Bottom, 40, true, 12, 6, false)
    );
    assert_eq!(b.fullscreen, FullscreenMode::Hide);
    assert_eq!(items(&b.left), ["clock"]);
    assert!(b.center.is_empty());
    assert_eq!(items(&b.right), ["cpu#a", "my/weather"]);
    assert_eq!(c.widgets["clock"].values["format"].value, rsb_core::value::Value::Str("HH:mm".into()));
    assert_eq!(c.widgets["cpu#a"].values["warn"].value, rsb_core::value::Value::Int(70));
    // Positions are kept for messages.
    assert_eq!(c.widgets["cpu#a"].values["warn"].pos.line, 23);
}

#[test]
fn floating_bars_default_to_radius_10_and_an_explicit_radius_wins() {
    let c = load("[[bar]]\nfloating = true").config.unwrap();
    assert_eq!(c.bars[0].radius, 10);
    let c = load("[[bar]]\nfloating = true\nradius = 0").config.unwrap();
    assert_eq!(c.bars[0].radius, 0);
}

#[test]
fn zone_entries_keep_their_position() {
    let c = load("[[bar]]\nleft = [\"clock\",\n   \"cpu\"]").config.unwrap();
    let p0 = c.bars[0].left[0].pos.as_ref().unwrap();
    let p1 = c.bars[0].left[1].pos.as_ref().unwrap();
    assert_eq!((p0.file.as_ref(), p0.line, p0.col), ("bar.toml", 2, 9));
    assert_eq!((p1.line, p1.col), (3, 4));
    assert_eq!(c.bars[0].pos.as_ref().unwrap().line, 1);
}

fn presets(name: &str) -> Option<String> {
    match name {
        "default" => Some("[[bar]]\nheight = 30\n".into()),
        "glass" => Some(
            "[[bar]]\nname = \"glass1\"\nfloating = true\nmargin = 8\nradius = 12\nheight = 36\nleft = [\"workspaces\"]\ncenter = [\"clock\"]\nright = [\"cpu\"]\n\n[[bar]]\nposition = \"bottom\"\nheight = 28\nleft = [\"title\"]\n\n[widget.clock]\nformat = \"HH:mm\"\nalt_format = \"x\"\n"
                .into(),
        ),
        _ => None,
    }
}

#[test]
fn user_bar_n_starts_from_preset_bar_n() {
    let l = load_config(Some(("preset = \"glass\"\n[[bar]]\nheight = 40\n", "bar.toml")), &presets);
    let c = l.config.unwrap();
    assert_eq!(c.preset, "glass");
    assert_eq!(c.bars.len(), 1, "the user list replaces the preset list");
    let b = &c.bars[0];
    assert_eq!((b.name.as_str(), b.height, b.floating, b.margin, b.radius), ("glass1", 40, true, 8, 12));
    assert_eq!(items(&b.right), ["cpu"]);

    // A third user bar inherits from the preset's last bar, but not its name.
    let text = "preset = \"glass\"\n[[bar]]\n[[bar]]\n[[bar]]\nheight = 20\n";
    let c = load_config(Some((text, "bar.toml")), &presets).config.unwrap();
    assert_eq!(c.bars.len(), 3);
    assert_eq!((c.bars[1].position, c.bars[1].height), (Position::Bottom, 28));
    assert_eq!((c.bars[2].position, c.bars[2].height, c.bars[2].name.as_str()), (Position::Bottom, 20, "bar3"));
    assert_eq!(items(&c.bars[2].left), ["title"]);
    // Keys neither sets take the built-in defaults: the preset's bar 2 has no center list.
    assert_eq!(items(&c.bars[1].center), ["clock"]);
}

#[test]
fn without_a_user_bar_list_the_preset_bars_are_used() {
    let c = load_config(Some(("preset = \"glass\"\n", "bar.toml")), &presets).config.unwrap();
    assert_eq!(c.bars.len(), 2);
    assert_eq!(c.bars[0].name, "glass1");
    assert_eq!(c.bars[1].name, "bar2");
    // Option tables merge per key, user last.
    let text = "preset = \"glass\"\n[widget.clock]\nformat = \"h:mm\"\n";
    let c = load_config(Some((text, "bar.toml")), &presets).config.unwrap();
    use rsb_core::value::Value;
    assert_eq!(c.widgets["clock"].values["format"].value, Value::Str("h:mm".into()));
    assert_eq!(c.widgets["clock"].values["alt_format"].value, Value::Str("x".into()));
}

#[test]
fn an_unknown_preset_falls_back_with_a_warning() {
    let l = load_config(Some(("preset = \"nope\"\n", "bar.toml")), &presets);
    assert_eq!(l.config.as_ref().unwrap().preset, "default");
    assert!(msgs(&l)[0].starts_with("warning: bar.toml:1:10: unknown preset 'nope'"), "{:?}", msgs(&l));
    assert_eq!(l.config.unwrap().bars[0].height, 30);
}

#[test]
fn a_bar_key_without_its_bar_line_says_where_it_belongs() {
    let l = load("config_version = 1\nheight = 40\n");
    assert_eq!(msgs(&l), ["warning: bar.toml:2:1: unknown key 'height' at the top level; it belongs under [[bar]]"]);
    // Every bar key gets the same help, and a key that is neither stays a plain typo.
    for key in rsb_core::config::BAR_KEYS {
        let l = load(&format!("{key} = 1\n"));
        assert!(msgs(&l)[0].ends_with("it belongs under [[bar]]"), "{key}: {:?}", msgs(&l));
    }
}

#[test]
fn a_missing_closing_quote_is_said_in_plain_words() {
    let l = load("[[bar]]\nname = \"main\n");
    let m = msgs(&l);
    assert_eq!(m.len(), 1, "{m:?}");
    assert!(m[0].starts_with("error: bar.toml:2:") && m[0].contains("missing closing quote"), "{m:?}");
    assert!(!m[0].contains("basic string"), "{m:?}");
}

#[test]
fn empty_bar_list_means_no_bars() {
    let c = load("bar = []").config.unwrap();
    assert!(c.bars.is_empty());
}

#[test]
fn unknown_keys_get_a_line_and_a_hint() {
    let l = load("[[bar]]\nheigth = 40\n");
    assert_eq!(msgs(&l), ["warning: bar.toml:2:1: unknown key 'heigth' in [[bar]]; did you mean 'height'?"]);
    assert_eq!(l.config.unwrap().bars[0].height, 32, "the key falls back to its default");
    let l = load("presett = \"glass\"\n");
    assert!(msgs(&l)[0].starts_with("warning: bar.toml:1:1: unknown key 'presett'"), "{:?}", msgs(&l));
    let l = load("[[bar]]\nzzzzzzzz = 1\n");
    assert!(!msgs(&l)[0].contains("did you mean"));
}

#[test]
fn wrong_types_and_ranges_are_warnings_with_a_line() {
    let l = load(
        "[[bar]]\nheight = \"tall\"\nfloating = 1\nmargin = 99\nradius = 2.5\nposition = \"left\"\nfullscreen = 3\n",
    );
    let m = msgs(&l);
    assert_eq!(m[0], "warning: bar.toml:2:10: 'height' must be a number, got a string");
    assert_eq!(m[1], "warning: bar.toml:3:12: 'floating' must be true or false, got an integer");
    assert_eq!(m[2], "warning: bar.toml:4:10: 'margin' must be 0 to 64");
    assert_eq!(m[3], "warning: bar.toml:5:10: 'radius' must be a whole number");
    assert!(m[4].contains("'position' must be one of \"top\", \"bottom\""), "{}", m[4]);
    assert!(m[5].contains("'fullscreen' must be a string, got an integer"), "{}", m[5]);
    assert!(l.diagnostics.iter().all(|d| d.severity == Severity::Warning));
    let b = &l.config.unwrap().bars[0];
    assert_eq!((b.height, b.margin, b.floating), (32, 8, false));
    let l = load("[[bar]]\nheight = 8\n");
    assert_eq!(msgs(&l), ["warning: bar.toml:2:10: 'height' must be 16 to 128"]);
}

#[test]
fn bad_widget_refs_are_reported_and_dropped() {
    let l = load("[[bar]]\nleft = [\"Clock\", \"ok\", 3]\n");
    let m = msgs(&l);
    assert!(m[0].starts_with("warning: bar.toml:2:9: bad entry in 'left': 'Clock'"), "{m:?}");
    assert!(m[1].contains("must be widget names"), "{m:?}");
    assert_eq!(items(&l.config.unwrap().bars[0].left), ["ok"]);
}

#[test]
fn syntax_errors_give_no_config_and_a_position() {
    let l = load("[[bar]]\nheight 40\n");
    assert!(l.config.is_none());
    assert_eq!(l.diagnostics.len(), 1);
    let d = &l.diagnostics[0];
    assert_eq!(d.severity, Severity::Error);
    let p = d.pos.as_ref().unwrap();
    assert_eq!((p.file.as_ref(), p.line), ("bar.toml", 2));
    assert!(p.col >= 1);
    let l = load("[[bar]\n");
    assert!(l.config.is_none());
}

#[test]
fn an_unquoted_instance_table_gets_the_quote_hint() {
    let l = load("[[bar]]\nleft = [\"clock#utc\"]\n\n[widget.clock#utc]\nformat = \"x\"\n");
    assert!(l.config.is_none());
    let d = &l.diagnostics[0];
    assert_eq!(d.pos.as_ref().unwrap().line, 4);
    assert!(d.message.ends_with("quote the name: [widget.\"clock#utc\"]"), "{}", d.message);
}

#[test]
fn display_names_have_a_short_form() {
    let c = load("[[bar]]\nmonitors = \"DISPLAY2\"\n").config.unwrap();
    assert_eq!(c.bars[0].monitors, vec![MonitorSelector::Device("\\\\.\\DISPLAY2".into())]);
    let c = load("[[bar]]\nmonitors = \"\\\\\\\\.\\\\display2\"\n").config.unwrap();
    assert_eq!(c.bars[0].monitors, vec![MonitorSelector::Device("\\\\.\\DISPLAY2".into())]);
    let l = load("[[bar]]\nmonitors = \"left\"\n");
    assert!(msgs(&l)[0].contains("'monitors' entry \"left\""), "{:?}", msgs(&l));
}

#[test]
fn a_widget_table_no_zone_uses_is_a_warning() {
    let text = "[[bar]]\nleft = [\"clock\", \"clock#utc\"]\n\n[widget.clock]\nx = 1\n\n[widget.\"clock#utc\"]\nx = 2\n\n[widget.\"clok\"]\nx = 3\n\n[widget.\"cpu#a\"]\nx = 4\n";
    let l = load(text);
    let m = msgs(&l);
    assert_eq!(m.len(), 2, "{m:?}");
    assert_eq!(m[0], "warning: bar.toml:10:9: [widget.\"clok\"] is not used by any bar");
    assert_eq!(m[1], "warning: bar.toml:13:9: [widget.\"cpu#a\"] is not used by any bar");
}

#[test]
fn a_wrong_config_version_is_an_error() {
    let l = load("config_version = 2\n");
    assert!(l.config.is_none());
    assert_eq!(l.diagnostics[0].to_string(), "error: bar.toml:1:18: this rsb reads config_version 1");
    assert!(load("config_version = 1\n").config.is_some());
}

#[test]
fn bar_names_are_unique() {
    let l = load("[[bar]]\nname = \"a\"\n[[bar]]\nname = \"a\"\n");
    let c = l.config.clone().unwrap();
    assert_eq!(c.bars[0].name, "a");
    assert_ne!(c.bars[1].name, "a");
    assert!(msgs(&l)[0].contains("used twice"));
    let l = load("[[bar]]\nname = \"Big Bar\"\n");
    assert!(msgs(&l)[0].contains("'name' must use"));
}

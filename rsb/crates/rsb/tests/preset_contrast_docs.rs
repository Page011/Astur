//! The authoring guide documents the preset metadata, the limit of the
//! contrast checks and the widget `about` table, and what it shows parses.

use rsb_core::preset::parse_about;

fn guide() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/authoring.md");
    std::fs::read_to_string(path).expect("docs/authoring.md").replace("\r\n", "\n")
}

#[test]
fn the_guide_says_what_about_holds_and_its_example_parses() {
    let doc = guide();
    for needle in [
        "### Preset metadata: `[about]`",
        "`[about.contrast]`",
        "`contrast.decorative`",
        "`tune`",
        "`schemes`",
        "a stack of translucent",
    ] {
        assert!(doc.contains(needle), "the guide lacks {needle}");
    }
    let block = doc
        .split("```toml\n")
        .skip(1)
        .filter_map(|rest| rest.split("```").next())
        .find(|b| b.contains("[about]"))
        .expect("the guide shows an [about] table");
    let about = parse_about(block, "authoring.md").unwrap_or_else(|d| panic!("{d}"));
    assert_eq!((about.title.as_str(), about.tune.len(), about.contrast.text.len()), ("Glass", 2, 2));
}

#[test]
fn the_guide_states_the_token_only_limit_and_where_the_accent_lives() {
    let doc = guide();
    assert!(doc.contains("Contrast is measured for custom properties only"));
    assert!(doc.contains("is not a token"));
    assert!(doc.contains(
        "If the colours block made by rsb settings is at the end of your style.css, change the accent there."
    ));
    for token in ["--accent-hover", "--accent-press", "--accent-text", "--accent-line"] {
        assert!(doc.contains(token), "the guide lacks {token}");
    }
}

#[test]
fn the_guide_lists_the_widget_about_fields() {
    let doc = guide();
    let at = doc.find("### Describing a widget: `about`").expect("a section on the widget about table");
    let section = &doc[at..];
    let section = &section[..section.find("\n### ").map_or(section.len(), |n| n.max(1))];
    for field in ["`title`", "`visible`", "`needs_options`", "`opens`"] {
        assert!(section.contains(field), "the about table of a widget lacks {field}");
    }
}

#[test]
fn the_template_and_every_preset_sheet_point_to_the_colours_block() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let template = std::fs::read_to_string(root.join("config/style.css")).unwrap();
    assert!(
        template
            .contains("If the colours block made by rsb settings is at the end of this file, change the accent there.")
    );
    for name in ["default", "fluent", "glass", "astur-classic"] {
        let css = std::fs::read_to_string(root.join(format!("presets/{name}.css"))).unwrap();
        let header = &css[..css.find("*/").expect("a header comment")];
        assert!(header.contains("colours block made by rsb settings"), "{name}.css header");
    }
}

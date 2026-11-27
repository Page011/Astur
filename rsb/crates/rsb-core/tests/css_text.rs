//! `css_text` writes every style property as CSS the style sheet reads back
//! to the same style.

mod common;

use std::sync::Arc;
use std::time::Instant;

use common::*;
use rsb_core::css::{PROPERTIES, Stylesheet};
use rsb_core::css_text::{MAX_VALUE_CHARS, SHOWN, css_text, differing};
use rsb_core::style::ComputedStyle;

/// One value for every property of the subset, written the way a person would.
const SAMPLES: &[(&str, &str)] = &[
    ("color", "#336699"),
    ("background", "linear-gradient(90deg, #ff0000 0%, #00ff00 40%, #0000ff 100%)"),
    ("background-color", "#0a141e80"),
    ("opacity", "0.5"),
    ("border", "2px solid #00ff00"),
    ("border-width", "1px 2px 3px 4px"),
    ("border-color", "#aabbcc"),
    ("border-radius", "4px 8px 12px 50%"),
    ("border-top-left-radius", "7px"),
    ("border-top-right-radius", "30%"),
    ("border-bottom-right-radius", "5px"),
    ("border-bottom-left-radius", "0"),
    ("border-top", "3px solid #ff0000"),
    ("border-right", "1px solid #00ff00"),
    ("border-bottom", "none"),
    ("border-left", "2px solid #0000ff"),
    ("padding", "2px 6px"),
    ("padding-top", "1px"),
    ("padding-right", "2px"),
    ("padding-bottom", "3px"),
    ("padding-left", "4px"),
    ("margin", "auto 4px 0 2px"),
    ("margin-top", "5px"),
    ("margin-right", "auto"),
    ("margin-bottom", "6px"),
    ("margin-left", "auto"),
    ("gap", "9px"),
    ("flex-direction", "column"),
    ("flex-grow", "2"),
    ("flex-shrink", "0"),
    ("justify-content", "space-between"),
    ("align-items", "stretch"),
    ("width", "50%"),
    ("height", "20px"),
    ("min-width", "10px"),
    ("min-height", "12.5px"),
    ("max-width", "100px"),
    ("max-height", "40%"),
    ("font-family", "\"Segoe UI\", Cascadia-Mono, monospace"),
    ("font-size", "15px"),
    ("font-weight", "bold"),
    ("font-style", "italic"),
    ("font-variant-numeric", "tabular-nums"),
    ("letter-spacing", "0.5px"),
    ("text-align", "end"),
    ("text-overflow", "ellipsis"),
    ("overflow", "hidden auto"),
    ("box-shadow", "0 2px 4px 1px #0000004d, 1px 1px 0 0 #ff0000"),
    ("display", "none"),
    ("visibility", "hidden"),
    ("transition", "opacity 150ms ease-in-out 20ms, width 1s steps(4, start)"),
    ("transition-property", "opacity, all"),
    ("transition-duration", "100ms, 2s"),
    ("transition-delay", "10ms"),
    ("transition-timing-function", "cubic-bezier(0.1, 0.2, 0.3, 1), linear"),
    ("cursor", "pointer"),
    ("backdrop", "mica"),
    ("corner-shape", "bevel round"),
    ("clip-path", "polygon(evenodd, 0 0, 100% 0, calc(100% - 4px) 100%, 0 calc(100% + 3px))"),
    ("transform", "translate(4px, 50%) scale(1.5, 2)"),
    ("transform-origin", "left top"),
    ("animation", "pulse 1s ease-in-out 200ms 3 alternate both paused, spin 2s linear infinite"),
    ("animation-name", "pulse, spin"),
    ("animation-duration", "1s, 300ms"),
    ("animation-timing-function", "steps(3, end), ease-out"),
    ("animation-delay", "-200ms, 1s"),
    ("animation-iteration-count", "infinite, 2.5"),
    ("animation-direction", "reverse, alternate-reverse"),
    ("animation-fill-mode", "forwards, backwards"),
    ("animation-play-state", "paused, running"),
    ("overflow-x", "scroll"),
    ("overflow-y", "auto"),
    ("scrollbar-width", "thin"),
    ("scrollbar-color", "#111111 #222222"),
    ("caret-color", "#ff8800"),
    ("accent-color", "#0088ff"),
    ("outline", "2px solid #ff0000"),
    ("outline-offset", "-3px"),
    ("pointer-events", "none"),
];

/// The style of the node a rule `<selector> { <decl> }` reaches.
fn styled(property: &str, value: &str) -> ComputedStyle {
    let bar_rule = property == "backdrop";
    let selector = if bar_rule { "bar" } else { ".t" };
    let css = format!("{selector} {{ {property}: {value} }}");
    let (sheet, diags) = Stylesheet::parse(&css, "style.css");
    assert!(diags.is_empty(), "{css}: {diags:?}");
    let t = classed(boxed(vec![]), &["t"]);
    let p = classed(boxed(vec![t]), &["p"]);
    let mut s = scene_with("", 300, 1.0, Some(p), None, None);
    s.set_stylesheet(Arc::new(sheet));
    frame(&mut s, Instant::now());
    let nodes = s.nodes();
    let node = if bar_rule { nodes.into_iter().next().unwrap() } else { node_by_class(&s, "t") };
    node.style
}

#[test]
fn every_property_of_the_subset_has_a_sample() {
    for p in PROPERTIES {
        assert!(SAMPLES.iter().any(|(n, _)| *n == p.name), "no sample for {}", p.name);
    }
    for (name, _) in SAMPLES {
        assert!(PROPERTIES.iter().any(|p| p.name == *name), "{name} is not a property");
    }
}

#[test]
fn every_property_round_trips_through_its_text() {
    for (name, value) in SAMPLES {
        let first = styled(name, value);
        let text = css_text(name, &first).unwrap_or_else(|| panic!("{name} has no text"));
        let second = styled(name, &text);
        assert_eq!(first, second, "{name}: {value:?} written as {text:?}");
        assert_eq!(css_text(name, &second).as_deref(), Some(text.as_str()), "{name}: text is stable");
    }
}

#[test]
fn the_text_says_what_was_written() {
    for (name, value, want) in [
        ("color", "#336699", "#336699"),
        ("background-color", "#0a141e80", "#0a141e80"),
        ("padding", "2px 6px", "2px 6px"),
        ("padding", "1px 2px 1px 2px", "1px 2px"),
        ("margin", "auto 4px 0 2px", "auto 4px 0px 2px"),
        ("border", "2px solid #00ff00", "2px solid #00ff00"),
        ("border-bottom", "none", "none"),
        ("font-weight", "bold", "700"),
        ("letter-spacing", "0.5px", "0.5px"),
        ("width", "50%", "50%"),
        ("max-width", "100px", "100px"),
        ("overflow", "hidden auto", "hidden auto"),
        ("corner-shape", "bevel round", "bevel round"),
        ("transform", "translate(4px, 50%) scale(1.5, 2)", "translate(4px, 50%) scale(1.5, 2)"),
        ("transition", "opacity 150ms ease-in-out 20ms", "opacity 150ms ease-in-out 20ms"),
        ("outline", "2px solid #ff0000", "2px solid #ff0000"),
    ] {
        assert_eq!(css_text(name, &styled(name, value)).as_deref(), Some(want), "{name}: {value}");
    }
    let initial = ComputedStyle::default();
    for (name, want) in [
        ("max-width", "none"),
        ("width", "auto"),
        ("letter-spacing", "normal"),
        ("display", "flex"),
        ("transition", "none"),
        ("animation", "none"),
        ("scrollbar-color", "auto"),
        ("caret-color", "auto"),
        ("font-family", "\"Segoe UI\""),
        ("border", "none"),
        ("clip-path", "none"),
        ("transform", "none"),
        ("margin", "0px"),
    ] {
        assert_eq!(css_text(name, &initial).as_deref(), Some(want), "initial {name}");
    }
}

#[test]
fn an_unknown_name_has_no_text() {
    assert_eq!(css_text("fonts-size", &ComputedStyle::default()), None);
    assert_eq!(css_text("", &ComputedStyle::default()), None);
    assert_eq!(css_text("::first-line", &ComputedStyle::default()), None);
}

#[test]
fn a_border_that_differs_by_side_has_no_single_text() {
    let s = styled("border-width", "1px 2px 3px 4px");
    assert_eq!(css_text("border", &s), None);
    assert_eq!(css_text("border-width", &s).as_deref(), Some("1px 2px 3px 4px"));
}

#[test]
fn a_long_value_is_cut_with_dots() {
    let stops: Vec<String> = (0..30).map(|i| format!("#{:02x}0000 {}%", i * 8, i * 3)).collect();
    let value = format!("linear-gradient(90deg, {})", stops.join(", "));
    let s = styled("background", &value);
    let text = css_text("background", &s).unwrap();
    assert_eq!(text.chars().count(), MAX_VALUE_CHARS + 3, "{text}");
    assert!(text.ends_with("..."));
}

#[test]
fn every_shown_property_is_one_the_subset_knows() {
    let s = ComputedStyle::default();
    for name in SHOWN {
        assert!(css_text(name, &s).is_some(), "{name}");
    }
}

#[test]
fn differing_lists_changed_properties_in_a_fixed_order_and_skips_what_was_inherited() {
    let base = ComputedStyle::default();
    let parent =
        ComputedStyle { color: rsb_core::color::Color::rgba8(0x33, 0x66, 0x99, 255), ..ComputedStyle::default() };
    let mut child = parent.clone();
    child.gap = 4.0;
    child.opacity = 0.5;
    let found = differing(&child, &base, Some(&parent));
    assert_eq!(
        found.iter().map(|(n, v)| (*n, v.as_str())).collect::<Vec<_>>(),
        vec![("gap", "4px"), ("opacity", "0.5")],
        "color came from the parent, so it is not listed again"
    );
    // The same child with no parent: the inherited colour differs from the initial one.
    let alone = differing(&child, &base, None);
    assert_eq!(alone.iter().map(|(n, _)| *n).collect::<Vec<_>>(), vec!["gap", "color", "opacity"]);
    assert!(differing(&base, &base, None).is_empty());
}

#[test]
fn placeholder_and_selection_rules_are_listed_by_their_pseudo_name() {
    let mut s = ComputedStyle::default();
    assert_eq!(css_text("::placeholder", &s).as_deref(), Some("none"));
    s.placeholder.opacity = Some(0.5);
    s.selection.background = Some(rsb_core::color::Color::rgba8(0, 120, 215, 255));
    assert_eq!(css_text("::placeholder", &s).as_deref(), Some("opacity: 0.5"));
    assert_eq!(css_text("::selection", &s).as_deref(), Some("background: #0078d7"));
    let found = differing(&s, &ComputedStyle::default(), None);
    assert_eq!(found.iter().map(|(n, _)| *n).collect::<Vec<_>>(), vec!["::placeholder", "::selection"]);
}

mod common;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use common::*;
use rsb_core::color::Color;
use rsb_core::css::{PROPERTIES, Stylesheet, parse_inline};
use rsb_core::diag::{Diagnostic, Severity, SourcePos};
use rsb_core::element::Element;
use rsb_core::geom::Edges;
use rsb_core::scene::Scene;
use rsb_core::style::{Align, Background, ComputedStyle, Cursor, Dim, FlexDirection, Justify, TextAlign};

fn parse(css: &str) -> (Stylesheet, Vec<String>) {
    let (s, d) = Stylesheet::parse(css, "style.css");
    (s, d.iter().map(|d| d.to_string()).collect())
}

/// A scene with `.p > .t` and the given sheet; the `.t` node's computed style.
fn computed(css: &str) -> ComputedStyle {
    computed_in(css, |t| t).1
}

/// Like `computed`, for sheets that are meant to have problems.
fn computed_lenient(css: &str) -> ComputedStyle {
    let t = classed(boxed(vec![]), &["t"]);
    let p = classed(boxed(vec![t]), &["p"]);
    let mut s = scene_with("", 300, 1.0, Some(p), None, None);
    s.set_stylesheet(Arc::new(Stylesheet::parse(css, "style.css").0));
    frame(&mut s, Instant::now());
    node_by_class(&s, "t").style
}

fn computed_in(css: &str, f: impl FnOnce(Element) -> Element) -> (Scene, ComputedStyle) {
    let t = f(classed(boxed(vec![]), &["t"]));
    let p = classed(boxed(vec![t]), &["p"]);
    let mut s = scene_with("", 300, 1.0, Some(p), None, None);
    let (sh, d) = Stylesheet::parse(css, "style.css");
    assert!(d.is_empty(), "{css}: {d:?}");
    s.set_stylesheet(Arc::new(sh));
    frame(&mut s, Instant::now());
    let style = node_by_class(&s, "t").style;
    (s, style)
}

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::rgba8(r, g, b, 255)
}

type Check = fn(&ComputedStyle) -> bool;

/// (property, declaration list for `.t`, check). Every property in the table must appear.
fn good_cases() -> Vec<(&'static str, &'static str, Check)> {
    vec![
        ("color", "color: #ff0000", |s| s.color == rgb(255, 0, 0)),
        ("color", "color: rgb(0, 128, 255)", |s| s.color == rgb(0, 128, 255)),
        ("color", "color: hsl(120 100% 25%)", |s| {
            (s.color.g * 255.0).round() == 128.0 && s.color.r == 0.0 && s.color.b == 0.0
        }),
        ("color", "color: rebeccapurple", |s| s.color == rgb(102, 51, 153)),
        ("background", "background: #112233", |s| s.background == Background::Color(rgb(0x11, 0x22, 0x33))),
        ("background", "background: none", |s| s.background == Background::None),
        ("background", "background: linear-gradient(to right, #000000, #ffffff)", |s| {
            matches!(&s.background, Background::Linear(g) if g.angle_deg == 90.0 && g.stops.len() == 2
                && g.stops[0] == (0.0, rgb(0, 0, 0)) && g.stops[1] == (1.0, rgb(255, 255, 255)))
        }),
        ("background", "background: linear-gradient(45deg, #000000 10%, #ff0000, #ffffff 90%)", |s| {
            matches!(&s.background, Background::Linear(g) if g.angle_deg == 45.0 && g.stops.len() == 3
                && g.stops[0].0 == 0.1 && (g.stops[1].0 - 0.5).abs() < 1e-6 && g.stops[2].0 == 0.9)
        }),
        ("background-color", "background-color: #01020304", |s| {
            s.background == Background::Color(Color::rgba8(1, 2, 3, 4))
        }),
        ("opacity", "opacity: 0.25", |s| s.opacity == 0.25),
        ("opacity", "opacity: 40%", |s| (s.opacity - 0.4).abs() < 1e-6),
        ("border", "border: 2px solid #00ff00", |s| {
            s.border_width == Edges::all(2.0) && s.border_color == rgb(0, 255, 0)
        }),
        ("border", "border: none", |s| s.border_width == Edges::all(0.0)),
        ("border", "border: 1px solid", |s| s.border_width == Edges::all(1.0)),
        ("border-top", "border-top: 3px solid #ffffff", |s| {
            s.border_width == Edges { top: 3.0, right: 0.0, bottom: 0.0, left: 0.0 }
                && s.border_color == rgb(255, 255, 255)
        }),
        ("border-right", "border-right: 4px solid #ffffff", |s| {
            s.border_width.right == 4.0 && s.border_width.left == 0.0
        }),
        ("border-bottom", "border-bottom: 5px solid #ffffff", |s| s.border_width.bottom == 5.0),
        ("border-left", "border-left: 6px solid #ffffff", |s| s.border_width.left == 6.0),
        ("border-width", "border: 1px solid #fff; border-width: 1px 2px 3px 4px", |s| {
            s.border_width == Edges { top: 1.0, right: 2.0, bottom: 3.0, left: 4.0 }
        }),
        ("border-color", "border: 1px solid #fff; border-color: #123456", |s| s.border_color == rgb(0x12, 0x34, 0x56)),
        ("border-radius", "border-radius: 4px 8px", |s| {
            let r = s.border_radius;
            (r.tl, r.tr, r.br, r.bl) == (Dim::Px(4.0), Dim::Px(8.0), Dim::Px(4.0), Dim::Px(8.0))
        }),
        ("border-radius", "border-radius: 50%", |s| s.border_radius.tl == Dim::Percent(50.0)),
        ("border-top-left-radius", "border-top-left-radius: 1px", |s| {
            s.border_radius.tl == Dim::Px(1.0) && s.border_radius.tr == Dim::Px(0.0)
        }),
        ("border-top-right-radius", "border-top-right-radius: 2px", |s| s.border_radius.tr == Dim::Px(2.0)),
        ("border-bottom-right-radius", "border-bottom-right-radius: 3px", |s| s.border_radius.br == Dim::Px(3.0)),
        ("border-bottom-left-radius", "border-bottom-left-radius: 4px", |s| s.border_radius.bl == Dim::Px(4.0)),
        ("padding", "padding: 1px 2px 3px 4px", |s| {
            s.padding == Edges { top: 1.0, right: 2.0, bottom: 3.0, left: 4.0 }
        }),
        ("padding", "padding: 2px 6px", |s| s.padding == Edges { top: 2.0, right: 6.0, bottom: 2.0, left: 6.0 }),
        ("padding", "padding: 0", |s| s.padding == Edges::all(0.0)),
        ("padding-top", "padding-top: 1px", |s| s.padding.top == 1.0 && s.padding.left == 0.0),
        ("padding-right", "padding-right: 2px", |s| s.padding.right == 2.0),
        ("padding-bottom", "padding-bottom: 3px", |s| s.padding.bottom == 3.0),
        ("padding-left", "padding-left: 4px", |s| s.padding.left == 4.0),
        ("margin", "margin: 0 auto", |s| {
            s.margin == Edges { top: Some(0.0), right: None, bottom: Some(0.0), left: None }
        }),
        ("margin-top", "margin-top: 1px", |s| s.margin.top == Some(1.0)),
        ("margin-right", "margin-right: auto", |s| s.margin.right.is_none()),
        ("margin-bottom", "margin-bottom: 3px", |s| s.margin.bottom == Some(3.0)),
        ("margin-left", "margin-left: 4px", |s| s.margin.left == Some(4.0)),
        ("gap", "gap: 3px", |s| s.gap == 3.0),
        ("flex-direction", "flex-direction: column", |s| s.flex_direction == FlexDirection::Column),
        ("flex-grow", "flex-grow: 2", |s| s.flex_grow == 2.0),
        ("flex-shrink", "flex-shrink: 0", |s| s.flex_shrink == 0.0),
        ("justify-content", "justify-content: space-between", |s| s.justify_content == Justify::SpaceBetween),
        ("justify-content", "justify-content: flex-end", |s| s.justify_content == Justify::End),
        ("justify-content", "justify-content: space-evenly", |s| s.justify_content == Justify::SpaceEvenly),
        ("align-items", "align-items: stretch", |s| s.align_items == Align::Stretch),
        ("align-items", "align-items: flex-start", |s| s.align_items == Align::Start),
        ("width", "width: 50%", |s| s.width == Dim::Percent(50.0)),
        ("width", "width: 12px", |s| s.width == Dim::Px(12.0)),
        ("width", "width: 2em", |s| s.width == Dim::Px(26.0)),
        ("height", "height: 20px", |s| s.height == Dim::Px(20.0)),
        ("min-width", "min-width: 5px", |s| s.min_width == Dim::Px(5.0)),
        ("min-height", "min-height: 6px", |s| s.min_height == Dim::Px(6.0)),
        ("max-width", "max-width: 70%", |s| s.max_width == Dim::Percent(70.0)),
        ("max-height", "max-height: none", |s| s.max_height == Dim::Auto),
        ("font-family", "font-family: \"Segoe UI\", sans-serif", |s| {
            s.font_family.iter().map(|f| f.to_string()).collect::<Vec<_>>() == ["Segoe UI", "Segoe UI"]
        }),
        ("font-family", "font-family: Segoe UI Variable, monospace", |s| {
            s.font_family.iter().map(|f| f.to_string()).collect::<Vec<_>>()
                == ["Segoe UI Variable", "Cascadia Mono", "Consolas"]
        }),
        ("font-family", "font-family: icons", |s| &*s.font_family[0] == "Segoe Fluent Icons"),
        ("font-size", "font-size: 14px", |s| s.font_size == 14.0),
        ("font-size", "font-size: 2em", |s| s.font_size == 26.0),
        ("font-size", "font-size: 150%", |s| s.font_size == 19.5),
        ("font-weight", "font-weight: bold", |s| s.font_weight == 700),
        ("font-weight", "font-weight: 600", |s| s.font_weight == 600),
        ("font-weight", "font-weight: normal", |s| s.font_weight == 400),
        ("font-style", "font-style: italic", |s| s.italic),
        ("font-variant-numeric", "font-variant-numeric: tabular-nums", |s| s.tabular_nums),
        ("letter-spacing", "letter-spacing: 0.5px", |s| s.letter_spacing == 0.5),
        ("letter-spacing", "letter-spacing: normal", |s| s.letter_spacing == 0.0),
        ("text-align", "text-align: center", |s| s.text_align == TextAlign::Center),
        ("text-align", "text-align: right", |s| s.text_align == TextAlign::End),
        ("text-overflow", "text-overflow: ellipsis", |s| s.ellipsis),
        ("overflow", "overflow: hidden", |s| s.clip),
        ("box-shadow", "box-shadow: 0 2px 4px 1px rgba(0, 0, 0, 0.5), 1px 1px #ffffff", |s| {
            s.box_shadow.len() == 2
                && (s.box_shadow[0].offset_y, s.box_shadow[0].blur, s.box_shadow[0].spread) == (2.0, 4.0, 1.0)
                && (s.box_shadow[0].color.a - 0.5).abs() < 1e-6
                && (s.box_shadow[1].offset_x, s.box_shadow[1].blur) == (1.0, 0.0)
        }),
        ("box-shadow", "box-shadow: 0 0 4px #000; box-shadow: none", |s| s.box_shadow.is_empty()),
        ("display", "display: none", |s| s.display_none),
        ("display", "display: flex", |s| !s.display_none),
        ("visibility", "visibility: hidden", |s| !s.visible),
        ("transition", "transition: opacity 200ms ease-in 50ms, color 0.1s", |s| {
            let t = &s.transitions;
            t.len() == 2
                && t[0].duration.as_millis() == 200
                && t[0].delay.as_millis() == 50
                && t[0].easing == rsb_core::anim::Easing::EaseIn
                && t[1].duration.as_millis() == 100
                && t[1].easing == rsb_core::anim::Easing::Ease
        }),
        ("transition-property", "transition-property: opacity, background-color; transition-duration: 1s", |s| {
            s.transitions.len() == 2 && s.transitions[0].duration.as_secs() == 1
        }),
        ("transition-duration", "transition: all 100ms; transition-duration: 300ms", |s| {
            s.transitions[0].duration.as_millis() == 300
        }),
        ("transition-delay", "transition: all 100ms; transition-delay: 20ms", |s| {
            s.transitions[0].delay.as_millis() == 20
        }),
        (
            "transition-timing-function",
            "transition: all 100ms; transition-timing-function: cubic-bezier(0.1, 0.2, 0.3, 1)",
            |s| s.transitions[0].easing == rsb_core::anim::Easing::CubicBezier(0.1, 0.2, 0.3, 1.0),
        ),
        ("cursor", "cursor: pointer", |s| s.cursor == Cursor::Pointer),
    ]
}

#[test]
fn every_property_parses_its_documented_values() {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (prop, decl, check) in good_cases() {
        seen.insert(prop);
        let style = computed(&format!(".p {{ font-size: 13px }} .t {{ {decl} }}"));
        assert!(check(&style), "{decl}: computed {style:?}");
    }
    // `backdrop` is the bar's.
    let mut s = scene_with("bar { backdrop: acrylic }", 100, 1.0, None, None, None);
    assert_eq!(frame(&mut s, Instant::now()).backdrop, rsb_core::style::Backdrop::Acrylic);
    s.set_stylesheet(sheet("bar { backdrop: mica }"));
    assert_eq!(frame(&mut s, Instant::now()).backdrop, rsb_core::style::Backdrop::Mica);
    s.set_stylesheet(sheet("bar { backdrop: none }"));
    assert_eq!(frame(&mut s, Instant::now()).backdrop, rsb_core::style::Backdrop::None);
    seen.insert("backdrop");
    let all: BTreeSet<&str> = PROPERTIES.iter().map(|p| p.name).collect();
    let missing: Vec<_> = all.difference(&seen).collect();
    assert!(missing.is_empty(), "properties without a good-value test: {missing:?}");
}

#[test]
fn the_good_values_make_no_diagnostics_and_property_names_ignore_case() {
    for (_, decl, _) in good_cases() {
        let (_, d) = parse(&format!(".t {{ {decl} }}"));
        assert!(d.is_empty(), "{decl}: {d:?}");
    }
    let s = computed(".t { COLOR: #ff0000; Font-Weight: BOLD }");
    assert_eq!((s.color, s.font_weight), (rgb(255, 0, 0), 700));
}

#[test]
fn bad_values_say_what_is_wrong_with_the_declarations_position() {
    // (declaration, text that must be in the message)
    let bad: &[(&str, &str)] = &[
        ("colr: red", "unknown property 'colr'; did you mean 'color'?"),
        ("backgroud: red", "did you mean 'background'?"),
        ("zzzzzzzz: 1", "unknown property 'zzzzzzzz'"),
        ("color: notacolor", "bad value for 'color'"),
        ("color: #12", "bad value for 'color'"),
        ("color: red blue", "bad value for 'color'"),
        ("opacity: 2", "bad value for 'opacity': must be between 0 and 1"),
        ("opacity: high", "bad value for 'opacity'"),
        ("padding: 1px 2px 3px 4px 5px", "bad value for 'padding': expected 1 to 4 lengths"),
        ("padding: 5", "bad value for 'padding': '5' needs a unit; use px (or em)"),
        ("padding: -1px", "bad value for 'padding': must not be negative"),
        ("padding: 10%", "a percentage is not allowed here"),
        ("margin: 1px 2px 3px 4px 5px", "bad value for 'margin'"),
        ("width: 10pt", "bad value for 'width': unit 'pt' is not supported; use px"),
        ("width: 1rem", "bad value for 'width': unit 'rem' is not supported; use em"),
        ("width: 5vw", "unknown unit 'vw'"),
        ("width: calc(1px + 2px)", "calc() is not supported"),
        ("width: -3px", "must not be negative"),
        ("width: none", "use 'auto'"),
        ("max-width: auto", "use 'none'"),
        ("width: fit-content", "not supported"),
        ("display: block", "bad value for 'display'"),
        ("flex-direction: row-reverse", "bad value for 'flex-direction'"),
        ("flex-grow: -1", "must not be negative"),
        ("justify-content: stretch", "bad value for 'justify-content'"),
        ("align-items: baseline", "bad value for 'align-items'"),
        ("font-weight: 1000", "whole number from 100 to 900"),
        ("font-weight: bolder", "not supported"),
        ("font-size: 0", "must be more than 0"),
        ("font-size: large", "bad value for 'font-size'"),
        ("font-style: oblique", "bad value for 'font-style'"),
        ("text-align: justify", "bad value for 'text-align'"),
        ("overflow: scroll", "bad value for 'overflow'"),
        ("visibility: collapse", "bad value for 'visibility'"),
        ("cursor: grab", "bad value for 'cursor'"),
        ("border: 1px dashed red", "only solid borders are supported"),
        ("border: thin solid red", "not supported; use a length like 1px"),
        ("border: 1px 2px solid", "only one width"),
        ("border-color: red blue", "one border colour"),
        ("border-radius: 1px / 2px", "elliptical radii"),
        ("border-radius: -2px", "must not be negative"),
        ("gap: 1px 2px", "expected one value"),
        ("box-shadow: inset 0 0 2px red", "inset shadows are not supported"),
        ("box-shadow: 1px", "expected 'x y [blur [spread]] colour'"),
        ("box-shadow: 0 0 -2px red", "blur must not be negative"),
        ("background: radial-gradient(red, blue)", "only linear-gradient() is supported"),
        ("background: url(x.png)", "images are not supported"),
        ("background: linear-gradient(red)", "at least two colours"),
        ("background: linear-gradient(to middle, red, blue)", "expected 'to top'"),
        ("background-color: linear-gradient(red, blue)", "use 'background' for a gradient"),
        ("transition: color", "needs a duration"),
        ("transition: nope 1s", "unknown property 'nope'"),
        ("transition: display 1s", "cannot be animated"),
        ("transition: color 1s 2s 3s", "at most two times"),
        ("transition: color 1s ease linear", "only one easing"),
        ("transition-timing-function: cubic-bezier(2, 0, 0, 1)", "x values must be between 0 and 1"),
        ("transition-duration: fast", "expected a time"),
        ("letter-spacing: wide", "bad value for 'letter-spacing'"),
        ("font-family: \"\"", "empty font name"),
        ("color: inherit", "only 'unset' is supported"),
        ("backdrop: blur", "bad value for 'backdrop'"),
    ];
    for (decl, want) in bad {
        let (_, d) = parse(&format!(".t {{ {decl} }}"));
        assert_eq!(d.len(), 1, "{decl}: {d:?}");
        assert!(d[0].starts_with("warning: style.css:1:6: "), "{decl}: position of the property name: {}", d[0]);
        assert!(d[0].contains(want), "{decl}: wanted {want:?} in {}", d[0]);
    }
}

#[test]
fn a_bad_declaration_drops_only_itself() {
    let s = computed_lenient(".t { color: #ff0000; padding: 5; font-weight: bold; colr: red; opacity: 9 }");
    assert_eq!((s.color, s.font_weight, s.padding, s.opacity), (rgb(255, 0, 0), 700, Edges::all(0.0), 1.0));
}

#[test]
fn unsupported_features_are_reported_with_the_alternative() {
    let css = "/* c */\n\
        .a { text-shadow: 1px 1px red; transform: scale(2); color: red !important; animation: x 1s; flex: 1; line-height: 2 }\n\
        @define-color accent #fff;\n\
        @keyframes spin { from { opacity: 0 } to { opacity: 1 } }\n\
        @font-face { src: url(x) }\n\
        .a:focus { color: red }\n\
        .a[x] { color: red }\n\
        .a + .b { color: red }\n\
        .a ~ .b { color: red }\n\
        .a::before { color: red }\n\
        foo { color: red }\n\
        .a { nested { color: red } }\n";
    let (sheet, d) = parse(css);
    let has = |line: u32, want: &str| {
        assert!(
            d.iter().any(|m| m.starts_with(&format!("warning: style.css:{line}:")) && m.contains(want)),
            "line {line} {want:?} in {d:#?}"
        );
    };
    has(2, "'text-shadow' is not supported in this version");
    has(2, "'transform' is not supported in this version");
    has(2, "'!important' is not supported; raise the selector's specificity instead");
    has(2, "'animation' is not supported: animations come in a later version");
    has(2, "'flex' is not supported; use flex-grow and flex-shrink");
    has(2, "'line-height' is not supported");
    has(3, "@define-color is not supported; use a custom property: --name: value");
    has(4, "animations come in a later version; @keyframes is not supported");
    has(5, "unsupported at-rule '@font-face'");
    has(6, ":focus is not supported; supported: :hover, :active");
    has(7, "attribute selectors are not supported");
    has(8, "'+' combinator is not supported");
    has(9, "'~' combinator is not supported");
    has(10, "pseudo-elements are not supported");
    has(11, "unknown element type 'foo'");
    has(12, "nested rules are not supported");
    assert!(d.iter().all(|m| m.starts_with("warning")), "only warnings, the file is usable: {d:#?}");
    // The `!important` declaration keeps its value.
    assert_eq!(sheet.rule_count(), 2, "the .a rule and the rule holding the nested text");
}

#[test]
fn exact_positions_across_lines_and_comments() {
    let css = "/* one\n   two */ .a { colr: red; }\n.b {\n    padding: 1px 2px 3px 4px 5px;\n\tcolor: nope;\n}\n.c { color: red }  /* x */ .d { oapcity: 1 }\n";
    let (_, d) = parse(css);
    assert_eq!(
        d,
        [
            "warning: style.css:2:16: unknown property 'colr'; did you mean 'color'?",
            "warning: style.css:4:5: bad value for 'padding': expected 1 to 4 lengths",
            "warning: style.css:5:2: bad value for 'color': unknown colour 'nope'",
            "warning: style.css:7:33: unknown property 'oapcity'; did you mean 'opacity'?",
        ]
    );
}

#[test]
fn columns_count_characters_not_bytes() {
    let (_, d) = parse(".caf\u{e9} { colr: red }");
    assert!(d[0].starts_with("warning: style.css:1:9:"), "{d:?}");
}

#[test]
fn a_file_that_cannot_be_used_is_an_error() {
    for (css, want, line, col) in [
        (".a { color: red", "this '{' is never closed", 1, 4),
        (".a { color: red }\n}\n.b { color: red }", "unexpected '}'", 2, 1),
        (".a { color: red }\n/* never closed", "unterminated comment", 2, 1),
        (".a color: red }", "expected '{' after the selector", 1, 15),
    ] {
        let (_, d) = Stylesheet::parse(css, "style.css");
        let e = d.iter().find(|d| d.severity == Severity::Error).unwrap_or_else(|| panic!("{css}: no error in {d:?}"));
        assert!(e.message.contains(want), "{css}: {}", e.message);
        let p = e.pos.as_ref().unwrap();
        assert_eq!((p.line, p.col), (line, col), "{css}: {e}");
    }
    // Rules before the problem are still parsed.
    let (s, _) = Stylesheet::parse(".a { color: red }\n}\n.b { color: red }", "style.css");
    assert_eq!(s.rule_count(), 2);
}

#[test]
fn selector_lists_split_and_count_per_selector() {
    let (s, d) = parse(".a, .b > .c , bar zone { color: red }\n.x { color: blue }");
    assert!(d.is_empty(), "{d:?}");
    assert_eq!(s.rule_count(), 4);
    // A bad selector in a list drops only itself.
    let (s, d) = parse(".a, .b:focus, .c { color: red }");
    assert_eq!(s.rule_count(), 2);
    assert_eq!(d.len(), 1);
    assert!(d[0].starts_with("warning: style.css:1:7: :focus is not supported"), "{d:?}");
}

fn colour_of(css: &str) -> Color {
    computed(css).color
}

#[test]
fn specificity_decides_then_source_order() {
    let red = rgb(255, 0, 0);
    let blue = rgb(0, 0, 255);
    // Equal specificity: the later rule wins.
    assert_eq!(colour_of(".t { color: #ff0000 } .t { color: #0000ff }"), blue);
    assert_eq!(colour_of(".t { color: #0000ff } .t { color: #ff0000 }"), red);
    // More specific wins whatever the order.
    assert_eq!(colour_of(".p .t { color: #ff0000 } .t { color: #0000ff }"), red);
    assert_eq!(colour_of("box.t { color: #ff0000 } .t { color: #0000ff }"), red);
    assert_eq!(colour_of(".t.t { color: #ff0000 } .t { color: #0000ff }"), red);
    // An id beats any number of classes.
    let (_, c) = {
        let (s, st) = computed_in(".t.t.t.t { color: #0000ff } #me { color: #ff0000 }", |mut e| {
            e.id = Some(Arc::from("me"));
            e
        });
        (s, st.color)
    };
    assert_eq!(c, red);
    // The universal selector adds nothing; a type adds one.
    assert_eq!(colour_of("* { color: #ff0000 } .t { color: #0000ff }"), blue);
    assert_eq!(colour_of("box { color: #ff0000 } * { color: #0000ff }"), red);
    // Order inside one rule: the later declaration wins.
    assert_eq!(colour_of(".t { color: #ff0000; color: #0000ff }"), blue);
    // Selector lists give each selector its own specificity.
    assert_eq!(colour_of(".zzz, .p .t { color: #ff0000 } .t { color: #0000ff }"), red);
}

#[test]
fn child_and_descendant_combinators() {
    let tree = |css: &str| {
        let t = classed(boxed(vec![]), &["t"]);
        let mid = classed(boxed(vec![t]), &["mid"]);
        let p = classed(boxed(vec![mid]), &["p"]);
        let mut s = scene_with("", 300, 1.0, Some(p), None, None);
        s.set_stylesheet(sheet(css));
        frame(&mut s, Instant::now());
        node_by_class(&s, "t").style.color
    };
    assert_eq!(tree(".p .t { color: #ff0000 }"), rgb(255, 0, 0));
    assert_eq!(tree(".p > .t { color: #ff0000 }"), Color::WHITE, "not a direct child");
    assert_eq!(tree(".mid > .t { color: #ff0000 }"), rgb(255, 0, 0));
    assert_eq!(tree(".p>.mid>.t { color: #ff0000 }"), rgb(255, 0, 0));
    assert_eq!(tree("zone#left .t { color: #ff0000 }"), rgb(255, 0, 0));
    assert_eq!(tree("bar.main zone#left widget.l box.p .t { color: #ff0000 }"), rgb(255, 0, 0));
    assert_eq!(tree("zone#right .t { color: #ff0000 }"), Color::WHITE);
    assert_eq!(tree("widget#l > box.p { color: #ff0000 }"), rgb(255, 0, 0), "the colour is on .p; .t inherits it");
    assert_eq!(tree("widget#l > box.t { color: #ff0000 }"), Color::WHITE, ".t is not a direct child of the widget");
}

#[test]
fn root_is_another_name_for_the_bar() {
    let bar_bg = |css: &str| {
        let mut s = scene_with(css, 100, 1.0, None, None, None);
        let f = frame(&mut s, Instant::now());
        match &f.draw.items[1].cmd {
            rsb_core::draw::DrawCmd::Fill { fill: rsb_core::draw::Fill::Solid(c), .. } => *c,
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(bar_bg(":root { background: #ff0000 }"), rgb(255, 0, 0));
    assert_eq!(
        bar_bg("bar { background: #0000ff } :root { background: #ff0000 }"),
        rgb(255, 0, 0),
        "same specificity, later wins"
    );
    assert_eq!(bar_bg(":root { background: #ff0000 } bar { background: #0000ff }"), rgb(0, 0, 255));
    assert_eq!(bar_bg("bar.main { background: #00ff00 } :root { background: #ff0000 }"), rgb(0, 255, 0));
    assert_eq!(bar_bg(":root.main { background: #00ff00 }"), rgb(0, 255, 0));
}

#[test]
fn inherited_properties_follow_the_parent_and_the_rest_do_not() {
    let css = ".p { color: #ff0000; font-family: Inter; font-size: 20px; font-weight: 700; font-style: italic; \
               letter-spacing: 2px; text-align: center; visibility: hidden; cursor: pointer; font-variant-numeric: tabular-nums; \
               background: #00ff00; opacity: 0.5; padding: 7px; border: 1px solid #fff; border-radius: 3px; gap: 4px; \
               width: 100px; display: flex; overflow: hidden; text-overflow: ellipsis; box-shadow: 0 0 1px #000; transition: all 1s }";
    let s = computed(css);
    assert_eq!(s.color, rgb(255, 0, 0));
    assert_eq!(&*s.font_family[0], "Inter");
    assert_eq!((s.font_size, s.font_weight, s.italic, s.letter_spacing), (20.0, 700, true, 2.0));
    assert_eq!((s.text_align, s.visible, s.cursor, s.tabular_nums), (TextAlign::Center, false, Cursor::Pointer, true));
    // Not inherited.
    assert_eq!(s.background, Background::None);
    assert_eq!(s.opacity, 1.0);
    assert_eq!((s.padding, s.border_width, s.gap), (Edges::all(0.0), Edges::all(0.0), 0.0));
    assert_eq!((s.width, s.clip, s.ellipsis), (Dim::Auto, false, false));
    assert!(s.box_shadow.is_empty() && s.transitions.is_empty());
    assert_eq!(s.border_radius, rsb_core::style::RadiusDims::ZERO);
    // A child may override and `unset` goes back to the parent's value.
    let s = computed(".p { color: #ff0000 } .t { color: #0000ff; color: unset }");
    assert_eq!(s.color, rgb(255, 0, 0));
    let s = computed(".p { padding: 4px } .t { padding: 9px; padding: unset }");
    assert_eq!(s.padding, Edges::all(0.0));
    // Colours that follow `currentcolor`: border and shadows use the final text colour.
    let s = computed(".t { color: #ff0000; border: 1px solid; box-shadow: 0 0 2px }");
    assert_eq!(s.border_color, rgb(255, 0, 0));
    assert_eq!(s.box_shadow[0].color, rgb(255, 0, 0));
    let s = computed(".t { border-width: 1px; border-color: currentcolor; color: #00ff00 }");
    assert_eq!(s.border_color, rgb(0, 255, 0));
}

#[test]
fn em_uses_the_elements_own_font_size_and_font_size_the_parents() {
    let s = computed(".p { font-size: 10px } .t { font-size: 2em; padding: 1em; gap: 0.5em; letter-spacing: 0.1em }");
    assert_eq!(s.font_size, 20.0);
    assert_eq!((s.padding.left, s.gap, s.letter_spacing), (20.0, 10.0, 2.0));
    let s = computed(".p { font-size: 10px } .t { font-size: 200% }");
    assert_eq!(s.font_size, 20.0);
    // em in a nested box follows the nearest font size down the chain.
    let s = computed(".p { font-size: 10px } .t { font-size: 1.5em }");
    assert_eq!(s.font_size, 15.0);
}

#[test]
fn media_queries_only_apply_in_their_scheme() {
    let css = ".t { color: #111111 } @media (prefers-color-scheme: dark) { .t { color: #eeeeee } } @media (prefers-color-scheme: light) { .t { color: #222222 } }";
    let (sh, d) = parse(css);
    assert!(d.is_empty(), "{d:?}");
    assert_eq!(sh.rule_count(), 3);
    let run = |dark: bool| {
        let t = classed(boxed(vec![]), &["t"]);
        let mut s = scene_with(css, 100, 1.0, Some(t), None, None);
        s.set_env(env(dark));
        frame(&mut s, Instant::now());
        node_by_class(&s, "t").style.color
    };
    assert_eq!(run(true), rgb(0xee, 0xee, 0xee));
    assert_eq!(run(false), rgb(0x22, 0x22, 0x22));
    for bad in [
        "@media (min-width: 100px) { .t { color: red } }",
        "@media screen { .t { color: red } }",
        "@media (prefers-color-scheme: sepia) { .t { color: red } }",
        "@media (prefers-color-scheme: dark) { @media (prefers-color-scheme: dark) { .t { color: red } } }",
        "@media (prefers-color-scheme: dark) .t { color: red }",
    ] {
        let (sh, d) = parse(bad);
        assert_eq!(d.len(), 1, "{bad}: {d:?}");
        assert_eq!(sh.rule_count(), 0, "{bad}");
    }
}

#[test]
fn imports_are_recorded_and_checked() {
    let css = "@import \"colors.css\";\n@import url('sub/theme.css');\n@import \"..\\\\x.css\";\n@import \"/abs.css\";\n@import \"c:x.css\";\n@import \"nocss.txt\";\n@import \"a.css\" screen;\n.a { color: red }\n@import \"late.css\";\n";
    let (s, d) = parse(css);
    let imports = s.imports();
    assert_eq!(imports.len(), 2);
    assert_eq!((imports[0].0.as_str(), imports[0].1.line, imports[0].1.col), ("colors.css", 1, 1));
    assert_eq!((imports[1].0.as_str(), imports[1].1.line), ("sub/theme.css", 2));
    assert_eq!(d.len(), 6, "{d:#?}");
    for (i, line) in [3, 4, 5, 6, 7, 9].iter().enumerate() {
        assert!(d[i].starts_with(&format!("warning: style.css:{line}:1:")), "{d:#?}");
    }
    assert!(d[0].contains("inside the config folder"));
    assert!(d[5].contains("must come before every rule"));
    assert_eq!(s.rule_count(), 1);
}

#[test]
fn append_puts_the_later_sheet_last_and_it_wins_ties() {
    let (mut base, _) = Stylesheet::parse(".t { color: #ff0000 } @import \"x.css\";", "presets/default.css");
    let (user, _) = Stylesheet::parse("@import \"colors.css\"; .t { color: #0000ff }", "style.css");
    assert_eq!(user.rule_count(), 1);
    base.append(user);
    assert_eq!(base.rule_count(), 2);
    assert_eq!(base.imports().len(), 1, "the preset's late import was refused; the user's is kept");
    let t = classed(boxed(vec![]), &["t"]);
    let mut s = scene_with("", 100, 1.0, Some(t), None, None);
    s.set_stylesheet(Arc::new(base));
    frame(&mut s, Instant::now());
    assert_eq!(node_by_class(&s, "t").style.color, rgb(0, 0, 255));
}

#[test]
fn font_families_lists_named_fonts_with_their_positions() {
    let css = ".a { font-family: Inter, \"Segoe UI\", sans-serif, monospace, icons }\n.b { font-family: \"Fira Code\" }\n.c { font-family: var(--f) }\n";
    let (s, d) = parse(css);
    assert!(d.is_empty());
    let f: Vec<(String, String)> = s.font_families().iter().map(|(n, p)| (n.clone(), p.to_string())).collect();
    assert_eq!(
        f,
        [
            ("Inter".to_string(), "style.css:1:6".to_string()),
            ("Segoe UI".to_string(), "style.css:1:6".to_string()),
            ("Fira Code".to_string(), "style.css:2:6".to_string()),
        ]
    );
}

#[test]
fn custom_properties_and_var() {
    let ok = |css: &str| {
        let (_, d) = parse(css);
        assert!(d.is_empty(), "{css}: {d:?}");
        computed(css)
    };
    let s = ok(
        ":root { --accent: #ff0000; --pad: 4px 8px; --f: \"Inter\" } .t { color: var(--accent); padding: var(--pad); font-family: var(--f), sans-serif }",
    );
    assert_eq!(s.color, rgb(255, 0, 0));
    assert_eq!(s.padding, Edges { top: 4.0, right: 8.0, bottom: 4.0, left: 8.0 }, "var inside a shorthand");
    assert_eq!(&*s.font_family[0], "Inter");
    // Fallbacks, also nested.
    let s = ok(":root { --a: #00ff00 } .t { color: var(--nope, #ff0000); background: var(--nope, var(--a)) }");
    assert_eq!((s.color, s.background.clone()), (rgb(255, 0, 0), Background::Color(rgb(0, 255, 0))));
    // A fallback holding commas.
    let s = ok(".t { box-shadow: var(--none, 0 1px 2px #000000, 1px 1px 1px #ffffff) }");
    assert_eq!(s.box_shadow.len(), 2);
    // A variable that uses another one, and one inherited from a parent, overridden below.
    let s = ok(":root { --base: #ff0000; --alias: var(--base) } .t { color: var(--alias) }");
    assert_eq!(s.color, rgb(255, 0, 0));
    let s = ok(":root { --c: #ff0000 } .p { --c: #0000ff } .t { color: var(--c) }");
    assert_eq!(s.color, rgb(0, 0, 255));
    let s = ok(".t { --c: #00ff00; color: var(--c) }");
    assert_eq!(s.color, rgb(0, 255, 0));
    // Var names are case sensitive, the function name is not.
    let s = ok(":root { --Acc: #ff0000 } .t { color: VAR(--Acc) }");
    assert_eq!(s.color, rgb(255, 0, 0));
    // A var in a transition and in a length with em.
    let s = ok(":root { --d: 150ms; --len: 2em } .t { transition: color var(--d) linear; padding: var(--len) }");
    assert_eq!((s.transitions[0].duration.as_millis(), s.padding.left), (150, 26.0));
}

#[test]
fn var_problems_unset_the_property_and_warn_with_both_positions() {
    let diags = |css: &str| -> (ComputedStyle, Vec<String>) {
        let (sh, d) = Stylesheet::parse(css, "style.css");
        assert!(d.is_empty(), "{d:?}");
        let t = classed(boxed(vec![]), &["t"]);
        let p = classed(boxed(vec![t]), &["p"]);
        let mut s = scene_with("", 300, 1.0, Some(p), None, None);
        s.set_stylesheet(Arc::new(sh));
        frame(&mut s, Instant::now());
        let d = s.take_diagnostics().iter().map(|d| d.diagnostic.to_string()).collect();
        (node_by_class(&s, "t").style, d)
    };
    // Undefined, no fallback.
    let (s, d) = diags(".p { color: #ff0000 } .t { color: var(--missing) }");
    assert_eq!(d, ["warning: style.css:1:28: 'color' uses var(--missing), which is not defined"]);
    assert_eq!(s.color, rgb(255, 0, 0), "unset: inherited again");
    // Cycle.
    let (_, d) = diags(":root { --a: var(--b); --b: var(--a) }\n.t { color: var(--a) }");
    assert!(d.iter().any(|m| m.contains("refers back to it")), "{d:?}");
    assert!(d.iter().any(|m| m.contains("'color' uses var(--a), which is not defined")), "{d:?}");
    let (_, d) = diags(":root { --a: var(--a) }");
    assert_eq!(d, ["warning: style.css:1:9: '--a' uses var(--a), which refers back to it"]);
    // Invalid after substitution: the declaration's position and where the variable was defined.
    let (s, d) = diags(":root { --w: red }\n.t { width: var(--w) }");
    assert_eq!(s.width, Dim::Auto);
    assert_eq!(
        d,
        [
            "warning: style.css:2:6: 'width' is invalid after var(--w) (defined at style.css:1:9): expected a length like 12px, got 'red'"
        ]
    );
    // Inside a shorthand.
    let (s, d) = diags(":root { --p: 1px 2px 3px 4px 5px }\n.t { padding: var(--p) }");
    assert_eq!(s.padding, Edges::all(0.0));
    assert!(d[0].contains("'padding' is invalid after var(--p)") && d[0].contains("expected 1 to 4 lengths"), "{d:?}");
    // var() without a custom property name.
    let (_, d) = diags(".t { color: var(accent) }");
    assert!(d[0].contains("var(accent) must name a custom property"), "{d:?}");
    // Custom property syntax.
    let (_, d) = parse(":root { --: 1; --a b: 2; --ok: 3; --empty: }");
    assert_eq!(d.len(), 3, "{d:?}");
}

#[test]
fn deleting_a_line_removes_its_effect() {
    let full = ".t {\n  color: #ff0000;\n  padding: 4px;\n  border: 1px solid #fff;\n}\n.p { font-size: 20px }\n";
    let s = computed(full);
    assert_eq!((s.color, s.padding.left, s.border_width.top), (rgb(255, 0, 0), 4.0, 1.0));
    let without_padding = full.replace("  padding: 4px;\n", "");
    let s = computed(&without_padding);
    assert_eq!((s.color, s.padding.left, s.border_width.top), (rgb(255, 0, 0), 0.0, 1.0));
    let without_rule = full.replace(&full[..full.find(".p {").unwrap()], "");
    let s = computed(&without_rule);
    assert_eq!((s.color, s.padding.left, s.border_width.top, s.font_size), (Color::WHITE, 0.0, 0.0, 20.0));
    // And through the live scene: a new sheet on the same scene.
    let t = classed(boxed(vec![]), &["t"]);
    let mut s = scene_with(full, 100, 1.0, Some(t), None, None);
    frame(&mut s, Instant::now());
    assert_eq!(node_by_class(&s, "t").style.padding.left, 4.0);
    s.set_stylesheet(sheet(&without_padding));
    frame(&mut s, Instant::now());
    assert_eq!(node_by_class(&s, "t").style.padding.left, 0.0);
}

fn inline(pairs: &[(&str, &str)]) -> (Vec<String>, rsb_core::css::InlineDecls) {
    let v: Vec<(String, String)> = pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    let (decls, d) = parse_inline(&v, Some(SourcePos::new("widgets/my/x.luau", 7, 0)));
    (d.iter().map(Diagnostic::to_string).collect(), decls)
}

#[test]
fn inline_style_accepts_unitless_numbers_and_names_with_underscores() {
    let (d, _) = inline(&[
        ("font_size", "14"),
        ("padding", "4"),
        ("opacity", "0.5"),
        ("font_weight", "600"),
        ("--accent", "#fff"),
    ]);
    assert!(d.is_empty(), "{d:?}");
    let (d, _) = inline(&[("colr", "red"), ("padding", "wide"), ("width", "5pt")]);
    assert_eq!(d.len(), 3);
    assert_eq!(d[0], "warning: widgets/my/x.luau:7: unknown property 'colr'; did you mean 'color'?");
    assert!(d[1].starts_with("warning: widgets/my/x.luau:7: bad value for 'padding'"));
    assert!(d[2].contains("use px"));
    // The same text in a stylesheet needs units.
    let (_, d) = parse(".a { padding: 4 }");
    assert_eq!(d.len(), 1);
    // And an element's inline style beats every rule.
    let (_, s) = computed_in(".t { padding: 9px; font-size: 30px }", |mut e| {
        e.style = vec![("padding".into(), "4".into()), ("font_size".into(), "14".into())];
        e
    });
    assert_eq!((s.padding.left, s.font_size), (4.0, 14.0));
}

#[test]
fn side_borders_with_different_colours_warn_once_and_the_last_wins() {
    let (_, d) = parse(".a { border-top: 1px solid #ff0000; border-left: 1px solid #0000ff }");
    assert_eq!(d, ["warning: style.css:1:37: a box has one border colour; the last one set wins"]);
    let (_, d) = parse(".a { border-top: 1px solid #ff0000; border-left: 1px solid #ff0000 }");
    assert!(d.is_empty(), "{d:?}");
    let s = computed_lenient(".t { border-top: 1px solid #ff0000; border-left: 1px solid #0000ff }");
    assert_eq!(s.border_color, rgb(0, 0, 255));
}

#[test]
fn backdrop_outside_the_bar_is_reported_where_it_was_written() {
    let (_, d) = parse("text { backdrop: mica }");
    assert_eq!(d, ["warning: style.css:1:8: backdrop only works on bar"]);
    // A class selector may match the bar, so only the scene can tell; it knows the position.
    let t = classed(boxed(vec![]), &["x"]);
    let mut s = scene_with("", 100, 1.0, Some(t), None, None);
    s.set_stylesheet(Arc::new(Stylesheet::parse("\n.x { backdrop: acrylic }", "style.css").0));
    frame(&mut s, Instant::now());
    let d = s.take_diagnostics();
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].diagnostic.to_string(), "warning: style.css:2:6: backdrop only works on bar");
    assert_eq!(d[0].instance, Some(rsb_core::element::InstanceId(1)));
}

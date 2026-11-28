//! The CSS added for shapes, motion, scrolling and focus: selectors, at-rules,
//! property values and the warnings that say what to do about a mistake.

mod common;

use std::sync::Arc;
use std::time::Instant;

use common::*;
use rsb_core::css::Stylesheet;
use rsb_core::style::{AnimFill, ComputedStyle, CornerShape, Overflow};

fn parse(css: &str) -> (Stylesheet, Vec<String>) {
    let (s, d) = Stylesheet::parse(css, "style.css");
    (s, d.iter().map(|d| d.to_string()).collect())
}

/// The `.t` node under `.p`, with the sheet applied, and the scene's own warnings.
fn styled(css: &str) -> (ComputedStyle, Vec<String>) {
    let t = classed(boxed(vec![]), &["t"]);
    let p = classed(boxed(vec![t]), &["p"]);
    let mut s = scene_with("", 300, 1.0, Some(p), None, None);
    s.set_stylesheet(Arc::new(Stylesheet::parse(css, "style.css").0));
    frame(&mut s, Instant::now());
    let warnings = s.take_diagnostics().iter().map(|d| d.diagnostic.to_string()).collect();
    (node_by_class(&s, "t").style, warnings)
}

fn bad(decl: &str, want: &str) {
    let (_, d) = parse(&format!(".t {{ {decl} }}"));
    assert_eq!(d.len(), 1, "{decl}: {d:?}");
    assert!(d[0].starts_with("warning: style.css:1:6: "), "{decl}: position of the property name: {}", d[0]);
    assert!(d[0].contains(want), "{decl}: wanted {want:?} in {}", d[0]);
}

#[test]
fn bad_values_of_the_new_properties_say_what_to_use() {
    for (decl, want) in [
        ("corner-shape: scoop", "'scoop' is not supported; use round or bevel"),
        ("corner-shape: bevel round bevel round bevel", "expected 1 to 4"),
        ("clip-path: circle(50%)", "'circle()' is not supported; use polygon"),
        ("clip-path: polygon(0 0, 10px 0)", "'clip-path' needs at least 3 points"),
        ("clip-path: polygon(0 0, 10px, 5px 5px)", "polygon() points need two values each"),
        ("clip-path: polygon(0 0, calc(10px * 2) 0, 5px 5px)", "calc() takes a percentage and a length"),
        ("clip-path: polygon(0 0, 10px 0, 5px 5px 2px)", "polygon() points need two values each"),
        ("transform: rotate(10deg)", "rotate, skew and matrix are not supported"),
        ("transform: skewX(10deg)", "rotate, skew and matrix are not supported"),
        ("transform: matrix(1, 0, 0, 1, 0, 0)", "rotate, skew and matrix are not supported"),
        ("transform: translate(calc(1px + 2px))", "calc() only works inside polygon(): use a length or a percentage"),
        ("transform: scale(-1)", "a scale must not be negative"),
        ("transform: translateX(1px, 2px)", "wrong number of values in translatex()"),
        ("transform: spin(1)", "unknown transform function 'spin'"),
        ("transform-origin: left right", "expected one value for each axis"),
        ("animation: pulse", "an animation needs a duration like 300ms"),
        ("animation: 300ms", "an animation needs a name like 'pulse'"),
        ("animation: a 1s 2s 3s", "at most two times"),
        ("animation: a 1s ease linear", "only one easing"),
        ("animation: a 1s sideways", "unexpected 'sideways'"),
        ("animation-iteration-count: -1", "an iteration count must not be negative"),
        ("animation-duration: -1s", "a duration must be 0 or more"),
        ("animation-direction: up", "expected normal, reverse, alternate or alternate-reverse"),
        ("animation-fill-mode: always", "expected none, forwards, backwards or both"),
        ("animation-play-state: stopped", "expected running, paused"),
        ("animation-timing-function: linear(0, 1)", "linear() is not supported; use cubic-bezier() or steps()"),
        ("animation-timing-function: steps(0)", "expected steps(n)"),
        ("transition-timing-function: steps(2, middle)", "expected steps(n)"),
        ("overflow: auto scroll hidden", "expected one or two values"),
        ("overflow-x: clip", "'clip' is not supported; use hidden"),
        ("scrollbar-width: wide", "expected auto, thin, none"),
        ("scrollbar-color: red", "expected auto, or a thumb colour and a track colour"),
        ("caret-color: nope", "unknown colour"),
        ("outline: 1px dashed red", "only solid outlines are supported"),
        ("outline: thick solid red", "not supported; use a length like 2px"),
        ("outline: 1px 2px", "only one width"),
        ("outline-offset: wide", "expected a length"),
        ("pointer-events: all", "expected auto, none"),
        ("width: fit-content", "'width' cannot animate to 'fit-content'"),
        ("height: fit-content", "'height' cannot animate to 'fit-content'"),
        ("cursor: grab", "expected default, pointer, text, auto"),
    ] {
        bad(decl, want);
    }
}

#[test]
fn good_values_of_the_new_properties_are_exact() {
    let (s, w) = styled(".t { corner-shape: bevel; border-radius: 6px }");
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(s.corner_shape, [CornerShape::Bevel; 4]);
    let (s, _) = styled(".t { corner-shape: bevel round }");
    assert_eq!(s.corner_shape, [CornerShape::Bevel, CornerShape::Round, CornerShape::Bevel, CornerShape::Round]);
    // Polygon points keep their percentage and length parts apart, in px and em.
    let (s, _) =
        styled(".t { font-size: 10px; clip-path: polygon(1.5em 0, calc(100% - 13px) 50%, calc(2px + 100%) 100%) }");
    let c = s.clip_path.expect("a polygon");
    assert_eq!(
        c.points.iter().map(|(x, y)| (x.px, x.pct, y.px, y.pct)).collect::<Vec<_>>(),
        [(15.0, 0.0, 0.0, 0.0), (-13.0, 100.0, 0.0, 50.0), (2.0, 100.0, 0.0, 100.0)]
    );
    let (s, _) = styled(".t { clip-path: polygon(nonzero, 0 0, 5px 0, 5px 5px); clip-path: none }");
    assert!(s.clip_path.is_none());
    // `overflow` with two values, the longhands on top, `clip` true while either is not visible.
    let (s, _) = styled(".t { overflow: hidden auto; overflow-x: visible }");
    assert_eq!((s.overflow_x, s.overflow_y, s.clip), (Overflow::Visible, Overflow::Auto, true));
    // Keywords and lengths in the origin; a transform list composes in order.
    let (s, _) = styled(".t { transform: translate(10px, 20%) scale(2, 3) translateX(1px) }");
    let t = s.transform;
    assert_eq!((t.tx.px, t.tx.pct, t.ty.pct, t.sx, t.sy), (12.0, 0.0, 20.0, 2.0, 3.0));
    let (s, _) = styled(".t { transform-origin: bottom }");
    assert_eq!((s.transform_origin.0.pct, s.transform_origin.1.pct), (50.0, 100.0));
    let (s, _) = styled(".t { transform-origin: 3px }");
    assert_eq!((s.transform_origin.0.px, s.transform_origin.1.pct), (3.0, 50.0));
    // The caret and accent follow `auto` = None; both inherit.
    let t = classed(boxed(vec![]), &["t"]);
    let p = classed(boxed(vec![t]), &["p"]);
    let mut sc = scene_with("", 300, 1.0, Some(p), None, None);
    sc.set_stylesheet(sheet(
        ".p { caret-color: #00ff00; accent-color: #ff0000; scrollbar-width: none; pointer-events: none }",
    ));
    frame(&mut sc, Instant::now());
    let child = node_by_class(&sc, "t").style;
    assert!(child.caret_color.is_some() && child.accent_color.is_some() && !child.pointer_events);
    assert_eq!(child.scrollbar_width, rsb_core::style::ScrollbarWidth::None);
}

#[test]
fn long_times_and_big_counts_are_cut_with_a_warning() {
    let (_, d) = parse(".t { animation: a 30s ease 20s 5000 }");
    assert_eq!(d.len(), 2, "{d:?}");
    assert!(d[0].contains("'animation': a time above 10s was cut to 10s"), "{d:?}");
    assert!(d[1].contains("an iteration count above 1000 was cut to 1000"), "{d:?}");
    let (s, _) = styled(".t { animation: a 30s ease 20s 5000 }");
    let a = &s.animations[0];
    assert_eq!((a.duration.as_secs(), a.delay_s, a.iterations), (10, 10.0, 1000.0));
    let (_, d) = parse(".t { transition: color 12s }");
    assert!(d[0].contains("a time above 10s was cut to 10s"), "{d:?}");
    // `infinite` stays infinite.
    let (s, _) = styled(".t { animation: a 1s infinite }");
    assert!(s.animations[0].iterations.is_infinite());
}

#[test]
fn animation_shorthand_and_longhands_agree() {
    let (a, _) = styled(".t { animation: pulse 300ms ease-out 20ms 2 alternate both paused }");
    let (b, _) = styled(
        ".t { animation-name: pulse; animation-duration: 300ms; animation-timing-function: ease-out; \
         animation-delay: 20ms; animation-iteration-count: 2; animation-direction: alternate; \
         animation-fill-mode: both; animation-play-state: paused }",
    );
    assert_eq!(a.animations, b.animations);
    assert_eq!(a.animations[0].fill, AnimFill::Both);
    assert!(a.animations[0].paused);
    // Lists cycle: two names, one duration.
    let (s, _) = styled(".t { animation-name: a, b; animation-duration: 100ms }");
    assert_eq!(s.animations.len(), 2);
    assert!(s.animations.iter().all(|a| a.duration.as_millis() == 100));
    // `none` clears it.
    let (s, _) = styled(".t { animation: a 1s; animation: none }");
    assert!(s.animations.is_empty());
    // `steps()` and `jump-start`.
    let (s, _) = styled(".t { animation: a 1s steps(3, jump-start) }");
    assert_eq!(s.animations[0].easing, rsb_core::anim::Easing::Steps(3, true));
}

#[test]
fn keyframes_are_parsed_per_rule_and_cut_to_animatable_properties() {
    let css = "@keyframes pulse { from { opacity: 0; display: none } 50% { opacity: .5; animation-timing-function: ease-in } \
               to, 90% { opacity: 1 } 120% { opacity: 1 } }\n\
               @keyframes other { to { color: red } }";
    let (s, d) = parse(css);
    assert_eq!(s.keyframe_names(), ["pulse", "other"]);
    assert_eq!(d.len(), 2, "{d:?}");
    assert!(d[0].contains("'display' cannot be animated; it is dropped from @keyframes"), "{d:?}");
    assert!(d[1].contains("'120%' is not a keyframe selector; use from, to or 0% to 100%"), "{d:?}");
    assert!(d[0].starts_with("warning: style.css:1:"), "{d:?}");
    let (_, d) = parse("@keyframes { to { opacity: 1 } }");
    assert!(d[0].contains("@keyframes needs a name like 'pulse'"), "{d:?}");
    let (_, d) = parse("@keyframes a;");
    assert!(d[0].contains("@keyframes needs a '{ ... }' block"), "{d:?}");
    let (_, d) = parse("@media (prefers-color-scheme: dark) { @keyframes a { to { opacity: 1 } } }");
    assert!(d[0].contains("@keyframes cannot be inside @media"), "{d:?}");
    let (_, d) = parse("@keyframes a { to { --x: 1px } }");
    assert!(d[0].contains("custom properties cannot be set in @keyframes"), "{d:?}");
}

#[test]
fn an_unknown_keyframes_name_is_a_warning_at_the_declaration() {
    let (_, w) = styled(".t { animation: nope 1s }");
    assert_eq!(w.len(), 1, "{w:?}");
    assert_eq!(w[0], "warning: style.css:1:6: unknown @keyframes 'nope' in animation-name");
    // A name that exists is silent, and a later sheet's @keyframes wins.
    let (_, w) = styled("@keyframes nope { to { opacity: 0 } } .t { animation: nope 1s }");
    assert!(w.is_empty(), "{w:?}");
    let mut a = Stylesheet::parse("@keyframes k { to { opacity: 0 } }", "a.css").0;
    a.append(Stylesheet::parse("@keyframes k { to { opacity: 1 } } @keyframes z { to { opacity: 1 } }", "b.css").0);
    assert_eq!(a.keyframe_names(), ["k", "k", "z"]);
}

#[test]
fn font_faces_are_read_with_every_problem_a_warning() {
    let css = "@font-face { font-family: \"Fira Code\"; src: url(\"fonts/fira.ttf\") format(\"truetype\"); font-weight: 300 700 }\n\
               @font-face { font-family: Plain; src: url(x.otf); font-style: italic; font-weight: bold }\n\
               @font-face { font-family: A; src: local(Arial) }\n\
               @font-face { font-family: B; src: url(\"../x.ttf\") }\n\
               @font-face { font-family: C; src: url(\"sub/x.ttf\") }\n\
               @font-face { font-family: D; src: url(\"x.woff2\") }\n\
               @font-face { font-family: E }\n\
               @font-face { src: url(\"y.ttf\") }\n\
               @font-face { font-family: F; src: url(\"f.ttf\"), url(\"g.ttf\") }\n\
               @font-face { font-family: G; src: url(\"g.ttf\"); font-weight: 900 100; shadow: 1 }\n\
               @font-face;";
    let (s, d) = parse(css);
    let faces = s.font_faces();
    assert_eq!(faces.len(), 3, "{faces:?}");
    assert_eq!((faces[2].family.as_str(), faces[2].weight), ("G", (400, 400)), "a bad weight keeps the default");
    assert_eq!(
        (faces[0].family.as_str(), faces[0].src.as_str(), faces[0].weight, faces[0].italic),
        ("Fira Code", "fonts/fira.ttf", (300, 700), false)
    );
    assert_eq!(
        (faces[1].family.as_str(), faces[1].src.as_str(), faces[1].weight, faces[1].italic),
        ("Plain", "x.otf", (700, 700), true)
    );
    assert_eq!(faces[0].pos.to_string(), "style.css:1:1");
    let has = |line: u32, want: &str| {
        assert!(
            d.iter().any(|m| m.starts_with(&format!("warning: style.css:{line}:")) && m.contains(want)),
            "line {line} {want:?} in {d:#?}"
        );
    };
    has(3, "src: local() is not supported; put the font file in the config folder's fonts folder");
    has(4, "must be inside the config folder's fonts folder");
    has(5, "must be inside the config folder's fonts folder");
    has(6, "must be .ttf, .otf or .ttc");
    has(7, "@font-face needs font-family and src");
    has(8, "@font-face needs font-family and src");
    has(9, "only one url() is supported in src");
    has(10, "the first weight must not be above the second");
    has(10, "'shadow' is not used by @font-face");
    has(11, "@font-face needs a '{ ... }' block");
}

#[test]
fn pseudo_classes_and_elements_parse_and_count_in_specificity() {
    let css = "input:focus-visible { color: red }\n\
               popup:open menuitem:first-child:not-a-thing { color: red }\n\
               slider:disabled, menuitem:checked, tooltip, scroll:last-child { color: red }\n\
               input::placeholder { color: red }\n\
               input.a::selection { color: red }\n\
               li:nth-child(2n+1) { color: red }\n\
               .a:nth-child(odd), .a:nth-child(even), .a:nth-child(-n+3), .a:nth-child( 4 ), .a:nth-child(n) { color: red }";
    let (s, d) = parse(css);
    assert_eq!(d.len(), 2, "{d:?}");
    assert!(d[0].starts_with("warning: style.css:2:"), "{d:?}");
    assert!(d[0].contains(":not-a-thing is not supported; supported: :hover, :active, :focus, :focus-visible, :focus-within, :disabled, :checked, :open, :first-child, :last-child, :nth-child()"), "{d:?}");
    assert!(d[1].contains("unknown element type 'li'"), "{d:?}");
    assert_eq!(s.rule_count(), 1 + 4 + 1 + 1 + 5);
}

#[test]
fn pseudo_element_rules_take_four_properties_and_go_last() {
    let (_, d) = parse(
        "input::placeholder { color: red; padding: 1px; opacity: .5; font-style: italic; background-color: blue }",
    );
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].contains("::placeholder takes color, background-color, opacity and font-style"), "{d:?}");
    let (_, d) = parse("box::placeholder { color: red }");
    assert!(d[0].contains("::placeholder only works on input"), "{d:?}");
    let (_, d) = parse("input::selection .a { color: red }");
    assert!(d[0].contains("a pseudo-element must be last in the selector"), "{d:?}");
    let (_, d) = parse("input::placeholder.a { color: red }");
    assert!(d[0].contains("a pseudo-element must be last"), "{d:?}");
    let (_, d) = parse("input::before { color: red }");
    assert!(d[0].contains("supported: ::placeholder, ::selection"), "{d:?}");
}

#[test]
fn rules_that_can_only_match_in_a_bar_are_called_out() {
    for css in [
        "zone:focus { color: red }",
        "bar:focus-visible { color: red }",
        "widget:focus-within { color: red }",
        "zone .a:focus { color: red }",
        "plate:focus { color: red }",
    ] {
        let (s, d) = parse(css);
        assert_eq!(d.len(), 1, "{css}: {d:?}");
        assert!(d[0].contains("bars never take keyboard focus: this rule can only match inside a bar"), "{css}: {d:?}");
        assert_eq!(s.rule_count(), 1, "the rule is kept");
    }
    // Where focus can be, nothing is said.
    for css in [
        ".a:focus { color: red }",
        "popup input:focus-visible { color: red }",
        "widget.x popup .b:focus-within { color: red }",
        "box:hover { color: red }",
    ] {
        let (_, d) = parse(css);
        assert!(d.is_empty(), "{css}: {d:?}");
    }
}

#[test]
fn what_the_scene_alone_can_see_is_warned_with_the_declaration() {
    let (_, w) = styled(".t { outline: 2px solid red; clip-path: polygon(0 0, 10px 0, 5px 5px) }");
    assert_eq!(w.len(), 1, "{w:?}");
    assert_eq!(
        w[0],
        "warning: style.css:1:6: outline is cut by clip-path; use corner-shape: bevel, or show focus inside the shape"
    );
    let (_, w) = styled(".t { corner-shape: bevel }");
    assert_eq!(w, ["warning: style.css:1:6: corner-shape bevel needs a border-radius to cut"]);
    let (_, w) = styled(".t { corner-shape: bevel round; border-radius: 0 4px }");
    assert_eq!(w.len(), 1, "a bevel on a square corner has nothing to cut: {w:?}");
    let (_, w) = styled(".t { corner-shape: round; outline: 1px solid red }");
    assert!(w.is_empty(), "{w:?}");
}

#[test]
fn custom_properties_work_inside_polygon_and_transform() {
    let (s, w) = styled(
        ".p { --d: 13px } .t { clip-path: polygon(var(--d) 0, calc(100% - var(--d)) 0, 100% 100%); transform: translateX(var(--d)) }",
    );
    assert!(w.is_empty(), "{w:?}");
    let c = s.clip_path.expect("polygon");
    assert_eq!((c.points[0].0.px, c.points[1].0.px, c.points[1].0.pct), (13.0, -13.0, 100.0));
    assert_eq!(s.transform.tx.px, 13.0);
    // Wrong after substitution: one warning with both positions.
    let (_, w) = styled(".p { --d: 13px 2px } .t { transform: translateX(var(--d)) }");
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].contains("is invalid after var(--d)"), "{w:?}");
}

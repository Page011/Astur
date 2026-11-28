//! More of the new CSS: selector forms, the font messages the config reader
//! uses, and junk input.

mod common;

use rsb_core::css::{PROPERTIES, Stylesheet, parse_inline};

fn parse(css: &str) -> (Stylesheet, Vec<String>) {
    let (s, d) = Stylesheet::parse(css, "style.css");
    (s, d.iter().map(|d| d.to_string()).collect())
}

#[test]
fn font_face_problems_the_reader_finds_read_like_the_parsers() {
    let (s, _) = parse(
        "@font-face { font-family: X; src: url(\"fonts/x.ttf\") }\n@font-face { font-family: Y; src: url(y.otf) }",
    );
    let f = s.font_faces();
    assert_eq!((f[0].file_name(), f[1].file_name()), ("x.ttf", "y.otf"));
    assert_eq!(
        f[0].not_found().to_string(),
        "warning: style.css:1:1: font file 'fonts/x.ttf' was not found in the config folder's fonts folder"
    );
    assert_eq!(f[1].too_large().to_string(), "warning: style.css:2:1: font file 'fonts/y.otf' is larger than 16 MiB");
    assert!(f[1].not_a_font().to_string().ends_with("'fonts/y.otf' is not a font"));
    assert!(f[0].over_the_limit().to_string().contains("more than 16 font files"));
}

#[test]
fn nth_child_takes_the_documented_forms_only() {
    for ok in ["odd", "even", "3", "n", "2n", "2n+1", "-n+3", "+n", " 2n + 1 ", "0n+2", "-2n-1"] {
        let (s, d) = parse(&format!(".a:nth-child({ok}) {{ color: red }}"));
        assert!(d.is_empty() && s.rule_count() == 1, "{ok}: {d:?}");
    }
    for bad in ["", "x", "2n+", "n+n", "2x", "1.5", "2n 1", "99999999999"] {
        let (s, d) = parse(&format!(".a:nth-child({bad}) {{ color: red }}"));
        assert_eq!((s.rule_count(), d.len()), (0, 1), "{bad:?}: {d:?}");
        assert!(d[0].contains("expected odd, even, a number or an+b like 2n+1 inside :nth-child()"), "{bad:?}: {d:?}");
    }
    let (_, d) = parse(".a:nth-child(2 { color: red }");
    assert!(!d.is_empty(), "{d:?}");
}

#[test]
fn no_value_for_any_property_can_make_the_parser_panic() {
    let junk = [
        "",
        " ",
        ",",
        ",,",
        "(",
        ")",
        "()",
        "((((",
        "\"",
        "'",
        "\\",
        "1e999",
        "-1e999",
        "NaN",
        "inf",
        "99999999999999999999999px",
        "-99999999999999999999999px",
        "1px,",
        ",1px",
        "calc(",
        "calc()",
        "calc(1px +)",
        "calc(+ 1px)",
        "polygon(",
        "polygon()",
        "polygon(,,)",
        "polygon(0 0, 0 0, 0 0)",
        "translate(",
        "translate()",
        "translate(,)",
        "scale(1,2,3)",
        "steps(",
        "steps(1,",
        "steps(-1)",
        "cubic-bezier(1,2,3)",
        "var(",
        "var(--x",
        "url(",
        "\u{1F600}",
        "a b c d e f g h i j k l m n o p q r s t",
        "1s 2s 3s 4s",
        "none none",
        "auto auto auto",
        "#",
        "#12",
        "rgb(",
        "rgba(1,2,3",
        "hsl(1)",
        "1 / 2",
        "!important",
    ];
    for p in PROPERTIES {
        for v in junk {
            let _ = Stylesheet::parse(&format!(".t {{ {}: {v} }}", p.name), "style.css");
            let _ = parse_inline(&[(p.name.to_string(), v.to_string())], None);
        }
    }
    for sel in [
        ":nth-child(",
        ":nth-child()",
        "::",
        ":::placeholder",
        "a::placeholder::selection",
        ":",
        "::placeholder:hover",
        ":nth-child(2n+1",
        "input::placeholder",
    ] {
        let _ = Stylesheet::parse(&format!("{sel} {{ color: red }}"), "style.css");
    }
    for at in [
        "@keyframes a { from { } to { } }",
        "@keyframes a {",
        "@keyframes a { 50% }",
        "@keyframes a { from { opacity: 0 } from { opacity: 1 } }",
        "@font-face {",
        "@font-face { src: }",
        "@font-face { src: url( }",
        "@font-face { font-weight: 1 2 3 }",
        "@keyframes a { to { opacity: var(--x) } }",
    ] {
        let _ = Stylesheet::parse(at, "style.css");
    }
}

#[test]
fn the_service_tables_name_the_new_services() {
    use rsb_core::permissions::{CONTROL_SERVICES, READ_SERVICES, USER_DEFAULT_READS};
    use rsb_core::services::ServiceId as S;
    for s in [S::Audio, S::Media, S::Workspaces, S::Microphone, S::Bluetooth, S::Apps, S::Power, S::Wifi] {
        assert!(CONTROL_SERVICES.contains(&s), "{s:?}");
    }
    assert_eq!(CONTROL_SERVICES.len(), 8);
    for s in [S::Window, S::Media, S::Network, S::Bluetooth, S::Apps, S::Wifi] {
        assert!(READ_SERVICES.contains(&s), "{s:?}");
    }
    assert_eq!(READ_SERVICES.len(), 6);
    // The default grant did not grow with the table.
    assert_eq!(USER_DEFAULT_READS, [S::Window, S::Media, S::Network]);
}

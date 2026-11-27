//! Nothing a caller sends reaches a file without being checked.

#[path = "cfgedit_common.rs"]
mod common;

use common::*;
use rsb_core::contract::{ErrorCode, Op, Zone};
use serde_json::{Value, json};

/// Every character or string that could break out of a TOML string, a CSS
/// declaration or a comment.
const BAD_TOKENS: &[&str] = &[
    "\"",
    "\\",
    "\n",
    "\r",
    "[",
    "]",
    "{",
    "}",
    ";",
    "/*",
    "*/",
    "<",
    ">",
    "=",
    "'",
    "\0",
    "\u{1b}",
    "\u{202e}",
    "#",
    "\"\n[[bar]]",
    "red; } bar { x",
    "#fff */ x",
    "a\"b",
    "x\ty",
    "\n[[bar]]\nx = 1",
    "clock\"]\nx = [\"",
    "}} bar {{",
    "a b",
    "$(x)",
    "../x",
    "a%20b",
];

fn expect_bad(ops: &[Op], what: &str) {
    let e = plan(TEMPLATE_BAR, ops).unwrap_err();
    assert_eq!(e.code, ErrorCode::BadValue, "{what}: {ops:?}: {e:?}");
    assert_eq!(e.exit_code(), 2);
    assert!(!e.message.contains('\n') && !e.message.contains('\u{202e}'), "{what}: {}", e.message);
    // The same request on the colours file.
    let e2 = run_plan(None, Some(TEMPLATE_CSS), ops, false)
        .expect_err(&format!("{what}: the colours file accepted {ops:?}"));
    assert_eq!((e2.code, e2.exit_code()), (ErrorCode::BadValue, 2), "{what}: {e2:?}");
}

#[test]
fn no_value_reaches_a_file_unvalidated() {
    for token in BAD_TOKENS {
        let t = *token;
        let as_list = |t: &str| Value::Array(vec![json!("clock"), json!(t)]);
        expect_bad(&[set("preset", json!(t))], "preset");
        expect_bad(&[set("bar.main.position", json!(t))], "position");
        expect_bad(&[set("bar.main.height", json!(t))], "height as text");
        expect_bad(&[set("bar.main.floating", json!(t))], "floating as text");
        for zone in ["left", "center", "right"] {
            expect_bad(&[set(&format!("bar.main.{zone}"), as_list(t))], zone);
            expect_bad(&[set(&format!("bar.main.{zone}"), json!(t))], "zone as text");
        }
        expect_bad(&[set("color.accent", json!(t))], "accent");
        expect_bad(&[add(t, "main", Zone::Left, None)], "add widget");
        expect_bad(&[remove(t, None)], "remove widget");
        expect_bad(&[mv(t, None, "main", Zone::Left, None)], "move widget");
        expect_bad(&[add("clock", t, Zone::Left, None)], "target bar");
        expect_bad(&[remove("clock", Some(t))], "from bar");
        expect_bad(&[mv("clock", Some(t), "main", Zone::Right, None)], "move from bar");
        expect_bad(&[mv("clock", None, t, Zone::Right, None)], "move to bar");
        expect_bad(&[set(&format!("bar.main{t}.height"), json!(40))], "key with the token");
        expect_bad(&[set(&format!("preset{t}"), json!("glass"))], "key");
        expect_bad(&[unset(&format!("bar.main.height{t}"))], "unset key");
    }
}

#[test]
fn the_wrong_json_type_is_a_bad_value_too() {
    for (key, value) in [
        ("preset", json!(3)),
        ("preset", json!(["glass"])),
        ("preset", Value::Null),
        ("bar.main.position", json!(true)),
        ("bar.main.height", json!(36.5)),
        ("bar.main.height", json!(null)),
        ("bar.main.height", json!([40])),
        ("bar.main.floating", json!(1)),
        ("bar.main.floating", json!("true")),
        ("bar.main.left", json!({"a": 1})),
        ("bar.main.left", json!([1])),
        ("color.accent", json!(5)),
    ] {
        let e = plan(TEMPLATE_BAR, &[set(key, value.clone())]).unwrap_err();
        assert_eq!(e.code, ErrorCode::BadValue, "{key} {value}: {e:?}");
    }
}

#[test]
fn limits_are_the_parsers_limits() {
    for (n, ok) in [(15, false), (16, true), (128, true), (129, false), (-1, false)] {
        let r = plan(TEMPLATE_BAR, &[set("bar.main.height", json!(n))]);
        assert_eq!(r.is_ok(), ok, "{n}");
        if let Err(e) = r {
            assert_eq!(e.code, ErrorCode::BadValue);
            assert!(e.message.contains("16 to 128"), "{}", e.message);
        }
    }
    let ok64: Vec<&str> = vec!["clock"; 64];
    assert!(plan(TEMPLATE_BAR, &[set("bar.main.left", j(&ok64))]).is_ok());
    let over: Vec<&str> = vec!["clock"; 65];
    assert_eq!(plan(TEMPLATE_BAR, &[set("bar.main.left", j(&over))]).unwrap_err().code, ErrorCode::BadValue);
}

#[test]
fn keys_and_names_that_are_not_changed_here_say_so() {
    let e = plan(TEMPLATE_BAR, &[set("bar.main.margin", json!(8))]).unwrap_err();
    assert_eq!(e.code, ErrorCode::FileOnly);
    assert!(e.next.as_deref().unwrap().contains("margin = 0"), "{:?}", e.next);
    let e = plan(TEMPLATE_BAR, &[set("bar.main.hieght", json!(40))]).unwrap_err();
    assert_eq!(e.code, ErrorCode::UnknownKey);
    assert!(e.message.contains("did you mean 'bar.main.height'"), "{}", e.message);
    let e = plan(TEMPLATE_BAR, &[set("preset", json!("glas"))]).unwrap_err();
    assert_eq!(e.code, ErrorCode::UnknownPreset);
    assert!(e.message.contains("did you mean 'glass'"), "{}", e.message);
    assert_eq!(plan(TEMPLATE_BAR, &[set("bar.top.height", json!(40))]).unwrap_err().code, ErrorCode::UnknownBar);
    assert_eq!(plan(TEMPLATE_BAR, &[set("color.fg", json!("#fff"))]).unwrap_err().code, ErrorCode::FileOnly);
    assert_eq!(plan(TEMPLATE_BAR, &[Op::Undo { n: 1 }]).unwrap_err().code, ErrorCode::BadUsage);
    // A bar by index, a position in any case.
    let p = plan(TEMPLATE_BAR, &[set("bar.0.position", json!("Bottom"))]).unwrap();
    assert!(bar_file(&p).after.contains("position = \"bottom\""), "{}", bar_file(&p).after);
}

#[test]
fn what_is_written_is_made_from_the_checked_value() {
    // A valid request whose text carries nothing but the rendered tokens.
    let p = plan(
        TEMPLATE_BAR,
        &[
            set("preset", json!("glass")),
            set("bar.main.position", json!("BOTTOM")),
            set("bar.main.height", json!(48)),
            set("bar.main.floating", json!(true)),
            set("bar.main.right", j(&["status", "clock#utc", "my/weather"])),
        ],
    )
    .unwrap();
    let after = &bar_file(&p).after;
    for line in [
        "preset = \"glass\"",
        "position = \"bottom\"",
        "height = 48",
        "floating = true",
        "right = [\"status\", \"clock#utc\", \"my/weather\"]",
    ] {
        assert!(after.contains(line), "{line} not in:\n{after}");
    }
    assert!(!after.contains("BOTTOM"));
    for change in &p.changes {
        if let Op::Set { key, value } = change
            && key == "bar.main.position"
        {
            assert_eq!(value, &json!("bottom"), "the change is reported as it was written");
        }
    }
}

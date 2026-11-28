//! The edit engine on text: spans, example lines, zone edits, the empty
//! `[[bar]]` insert, line endings.

#[path = "cfgedit_common.rs"]
mod common;

use common::*;
use rsb_core::config::{Position, load_config};
use rsb_core::contract::{ErrorCode, NoticeCode, Op, Source, Zone};
use serde_json::json;

fn effective(text: &str) -> rsb_core::config::Config {
    let l = load_config(Some((text, "bar.toml")), &preset_toml);
    let real: Vec<_> = l.diagnostics.iter().filter(|d| !d.message.ends_with("is not used by any bar")).collect();
    assert!(real.is_empty(), "{real:?}");
    l.config.unwrap()
}

fn names(list: &[rsb_core::config::ZoneItem]) -> Vec<String> {
    list.iter().map(|z| z.widget.to_string()).collect()
}

fn line_of(text: &str, needle: &str) -> usize {
    text.lines().position(|l| l.contains(needle)).map(|n| n + 1).unwrap_or_else(|| panic!("{needle} not in text"))
}

// ---------------------------------------------------------------- E3

/// Every request the bases below are put through, one change list each.
fn requests() -> Vec<Vec<Op>> {
    vec![
        vec![set("preset", json!("glass"))],
        vec![set("preset", json!("fluent"))],
        vec![unset("preset")],
        vec![set("bar.main.position", json!("bottom"))],
        vec![set("bar.main.height", json!(40))],
        vec![set("bar.main.floating", json!(true))],
        vec![set("bar.main.left", j(&["clock"]))],
        vec![set("bar.main.right", j(&["status", "clock", "power"]))],
        vec![add("clock#utc", "main", Zone::Right, Some(-1))],
        vec![add("clock#utc", "main", Zone::Left, Some(0))],
        vec![remove("media", None)],
        vec![remove("power", None)],
        vec![remove("date", None)],
        vec![mv("status", None, "main", Zone::Left, Some(0))],
        vec![mv("workspaces", None, "main", Zone::Right, None)],
        vec![unset("bar.main.height")],
        vec![unset("bar.main.right")],
        vec![unset("bar.main.center")],
        vec![
            set("preset", json!("glass")),
            set("bar.main.height", json!(41)),
            add("clock#utc", "main", Zone::Left, None),
        ],
        vec![set("bar.main.center", j(&["clock", "memory"])), set("bar.main.center", j(&["memory", "cpu", "clock"]))],
    ]
}

const ARRAY_COMMENTS: &str = "# my bar\npreset = \"glass\" # look\n\n[[bar]]\nname = \"main\"\nheight = 38   # mine\nleft = [\n  \"workspaces\", # desktops\n  # \"title\",\n  \"media\",\n  \"clock\"\n]\ncenter = [\"cpu\", # a\n  \"memory\"]\nright = [\n    \"status\",\n    \"power\",\n]\n\n[widget.clock]\nformat = \"HH:mm\"\n";

const ONE_PER_LINE: &str = "config_version = 1\n\n[[bar]]\nleft = [\n  \"workspaces\",\n  \"media\",\n]\nright = [\n  \"status\",\n  \"battery\",\n  \"power\",\n]\ncenter = []\n";

fn bases() -> Vec<(String, String)> {
    let mut v = vec![("template".to_string(), TEMPLATE_BAR.to_string())];
    for (name, text) in PRESETS {
        v.push((format!("preset {name}"), text.to_string()));
    }
    v.push(("template CRLF".to_string(), crlf(TEMPLATE_BAR)));
    v.push(("template BOM".to_string(), format!("\u{feff}{TEMPLATE_BAR}")));
    v.push(("preset glass CRLF".to_string(), crlf(PRESETS[2].1)));
    v.push(("comments inside arrays".to_string(), ARRAY_COMMENTS.to_string()));
    v.push(("one item per line".to_string(), ONE_PER_LINE.to_string()));
    v.push(("array comments CRLF".to_string(), crlf(ARRAY_COMMENTS)));
    v
}

#[test]
fn splice_keeps_bytes_outside_ranges() {
    let mut planned = 0;
    for (label, text) in bases() {
        let mut ok = 0;
        for ops in requests() {
            match plan(&text, &ops) {
                Ok(p) => {
                    for fp in &p.files {
                        check_file_plan(&text, fp, &live_lines(&p));
                    }
                    check_values_named(&text, &p);
                    // A change that says what the file already says plans nothing.
                    let Some(fp) = p.file(rsb_core::contract::ConfigFile::BarToml) else { continue };
                    let after = &fp.after;
                    if text.contains("\r\n") {
                        assert_eq!(
                            after.matches('\n').count(),
                            after.matches("\r\n").count(),
                            "{label}: a bare LF in a CRLF file"
                        );
                    }
                    assert!(!after.starts_with('\u{feff}'));
                    // The new text loads, and with no diagnostics a preset user file did not have.
                    let l = load_config(Some((after, "bar.toml")), &preset_toml);
                    assert!(l.config.is_some(), "{label}: {ops:?}");
                    ok += 1;
                }
                Err(e) => assert!(
                    matches!(e.code, ErrorCode::WidgetNotPlaced | ErrorCode::AmbiguousWidget | ErrorCode::UnknownBar),
                    "{label}: {ops:?}: {e:?}"
                ),
            }
        }
        assert!(ok >= 8, "{label}: only {ok} requests planned");
        planned += ok;
    }
    assert!(planned >= 80, "{planned}");
}

/// A plan whose one splice replaces `from` with `with` in `original`.
fn hand_made_plan(original: &str, from: &str, with: &str) -> (rsb_core::cfgedit::FilePlan, Vec<Op>) {
    let start = original.find(from).expect("the text to replace is in the original");
    let splice = rsb_core::cfgedit::Splice { start, end: start + from.len(), text: with.to_string() };
    let after = rsb_core::cfgedit::apply_splices(original, std::slice::from_ref(&splice));
    let fp = rsb_core::cfgedit::FilePlan {
        file: rsb_core::contract::ConfigFile::BarToml,
        base: rsb_core::contract::FileHash::of(original.as_bytes()),
        created: false,
        bom: false,
        splices: vec![splice],
        after,
        what: String::new(),
    };
    (fp, vec![set("bar.main.height", json!(40))])
}

const TWO_KEYS: &str = "[[bar]]\nname = \"main\"\nheight = 30\nmargin = 4\nright = [\"clock\"]\n";

#[test]
fn a_splice_that_takes_an_unrelated_key_with_it_is_caught() {
    // Exactly the height line: fine.
    let (fp, _) = hand_made_plan(TWO_KEYS, "height = 30\n", "height = 40\n");
    check_file_plan(TWO_KEYS, &fp, &[]);
    // Too wide: the margin line goes with it, and a splice covers that line, so only the values give it away.
    let (fp, _) = hand_made_plan(TWO_KEYS, "height = 30\nmargin = 4\n", "height = 40\n");
    let caught = std::panic::catch_unwind(|| check_file_plan(TWO_KEYS, &fp, &[]));
    assert!(caught.is_err(), "a splice that dropped margin = 4 passed");
}

#[test]
fn a_change_that_alters_a_value_it_does_not_name_is_caught() {
    let (fp, ops) = hand_made_plan(TWO_KEYS, "height = 30\n", "height = 40\n");
    let plan = |fp: rsb_core::cfgedit::FilePlan, changes: Vec<Op>| rsb_core::cfgedit::Plan {
        files: vec![fp],
        changes,
        notices: vec![],
        preset_switch: None,
        accent: None,
    };
    check_values_named(TWO_KEYS, &plan(fp, ops));
    // The right list lost its item while only the height was asked for.
    let (fp, ops) = hand_made_plan(
        TWO_KEYS,
        "height = 30\nmargin = 4\nright = [\"clock\"]\n",
        "height = 40\nmargin = 4\nright = []\n",
    );
    let caught = std::panic::catch_unwind(|| check_values_named(TWO_KEYS, &plan(fp, ops)));
    assert!(caught.is_err(), "a lost zone item passed a height change");
}

fn plan_of(fp: rsb_core::cfgedit::FilePlan, changes: Vec<Op>) -> rsb_core::cfgedit::Plan {
    rsb_core::cfgedit::Plan { files: vec![fp], changes, notices: vec![], preset_switch: None, accent: None }
}

/// Whether `check_values_named` accepts the plan.
fn named_ok(original: &str, from: &str, with: &str, ops: Vec<Op>) -> bool {
    let (fp, _) = hand_made_plan(original, from, with);
    let plan = plan_of(fp, ops);
    std::panic::catch_unwind(|| check_values_named(original, &plan)).is_ok()
}

const TWO_TABLES: &str = "[[bar]]\nname = \"main\"\nheight = 30\n\n[widget.clock]\nheight = 5\n";
const TWO_BARS: &str = "[[bar]]\nname = \"a\"\nleft = [\"clock\"]\nright = [\"power\"]\n\n[[bar]]\nname = \"b\"\nleft = [\"media\"]\nright = []\n";

#[test]
fn a_change_may_only_touch_what_it_names_on_the_bar_it_names() {
    let height = || vec![set("bar.main.height", json!(40))];
    // A same-named key outside every bar is not the bar's.
    assert!(named_ok(TWO_TABLES, "height = 30\n", "height = 40\n", height()));
    assert!(!named_ok(TWO_TABLES, "height = 5\n", "height = 6\n", height()), "widget.clock.height passed");
    // A key the change adds without naming it.
    assert!(!named_ok(TWO_KEYS, "height = 30\n", "height = 40\nfloating = true\n", height()), "an added key passed");
    // A key line written again in other words inside a wide splice, with the same value.
    assert!(named_ok(TWO_KEYS, "height = 30\nmargin = 4\n", "height = 40\nmargin = 4\n", height()));
    assert!(!named_ok(TWO_KEYS, "height = 30\nmargin = 4\n", "height = 40\nmargin=4\n", height()), "margin=4 passed");
    // A remove of a widget in bar a may not empty a zone of bar b.
    let power = || vec![remove("power", None)];
    assert!(named_ok(TWO_BARS, "right = [\"power\"]\n", "right = []\n", power()));
    assert!(
        !named_ok(
            TWO_BARS,
            "right = [\"power\"]\n\n[[bar]]\nname = \"b\"\nleft = [\"media\"]\n",
            "right = []\n\n[[bar]]\nname = \"b\"\nleft = []\n",
            power()
        ),
        "a zone of another bar passed"
    );
    // The same for a change of bar a's own key.
    let right = || vec![set("bar.a.right", json!([]))];
    assert!(named_ok(TWO_BARS, "right = [\"power\"]\n", "right = []\n", right()));
    assert!(!named_ok(TWO_BARS, "left = [\"media\"]\n", "left = []\n", right()), "bar b's list passed a bar a change");
}

// ---------------------------------------------------------------- example lines

#[test]
fn the_templates_preset_example_is_turned_live_in_place() {
    let p = plan(TEMPLATE_BAR, &[set("preset", json!("fluent"))]).unwrap();
    let fp = bar_file(&p);
    let line = line_of(TEMPLATE_BAR, "# preset = ");
    assert_eq!(fp.after.lines().nth(line - 1), Some("preset = \"fluent\""));
    assert_eq!(fp.after.lines().count(), TEMPLATE_BAR.lines().count());
    // The default preset has the power anchor and fluent has not.
    assert_eq!(notice_codes(&p), vec![NoticeCode::ExampleTurnedLive, NoticeCode::PopupUnreachable]);
    assert_eq!(live_lines(&p), vec![line]);
    check_file_plan(TEMPLATE_BAR, fp, &live_lines(&p));
    assert_eq!(effective(&fp.after).preset, "fluent");
    assert_eq!(fp.what, "preset default -> fluent");
}

#[test]
fn the_templates_bar_is_made_live_in_place_and_its_other_examples_still_work() {
    let p = plan(TEMPLATE_BAR, &[set("bar.main.height", json!(40))]).unwrap();
    let fp = bar_file(&p);
    assert!(fp.after.contains("\n[[bar]]\n"), "{}", fp.after);
    assert!(!fp.after.contains("# [[bar]]"));
    assert!(fp.after.contains("\nheight = 40                # logical pixels, 16 to 128\n"), "{}", fp.after);
    assert_eq!(fp.after.lines().count(), TEMPLATE_BAR.lines().count(), "no line was added");
    assert_eq!(
        notice_codes(&p),
        vec![NoticeCode::ExampleTurnedLive, NoticeCode::BarsNowListed, NoticeCode::ExampleTurnedLive]
    );
    check_file_plan(TEMPLATE_BAR, fp, &live_lines(&p));
    let cfg = effective(&fp.after);
    assert_eq!((cfg.bars.len(), cfg.bars[0].height), (1, 40));
    // The person uncomments `position` by hand, as the template says: still one bar, no warning.
    let by_hand = fp.after.replace("# position = \"top\"", "position = \"bottom\"");
    let cfg = effective(&by_hand);
    assert_eq!((cfg.bars.len(), cfg.bars[0].position), (1, Position::Bottom));
}

#[test]
fn a_root_key_written_below_the_template_stays_a_root_key() {
    let text = format!("{TEMPLATE_BAR}preset = \"glass\"\n");
    let p = plan(&text, &[set("bar.main.height", json!(40))]).unwrap();
    let fp = bar_file(&p);
    assert!(
        fp.after.ends_with("preset = \"glass\"\n\n[[bar]]\nheight = 40\n"),
        "{:?}",
        &fp.after[fp.after.len() - 60..]
    );
    assert!(fp.after.contains("# [[bar]]"), "the commented line stays");
    check_file_plan(&text, fp, &live_lines(&p));
    let cfg = effective(&fp.after);
    assert_eq!((cfg.preset.as_str(), cfg.bars[0].height, cfg.bars[0].floating), ("glass", 40, true));
}

#[test]
fn empty_bar_tables_are_inserted_before_the_first_table_and_load_as_the_preset_bar() {
    let text = "preset = \"glass\"\n\n[widget.clock]\nformat = \"HH:mm\"\n";
    let p = plan(text, &[set("bar.main.height", json!(40))]).unwrap();
    let fp = bar_file(&p);
    assert_eq!(fp.after, "preset = \"glass\"\n\n[[bar]]\nheight = 40\n\n[widget.clock]\nformat = \"HH:mm\"\n");
    assert!(notice_codes(&p).contains(&NoticeCode::BarsNowListed));
    let (before, after) = (effective(text), effective(&fp.after));
    assert_eq!(before.bars.len(), after.bars.len());
    let (mut a, mut b) = (before.bars[0].clone(), after.bars[0].clone());
    a.pos = None;
    b.pos = None;
    b.height = a.height;
    assert_eq!(a, b, "only the height differs");
    assert_eq!(after.bars[0].height, 40);
}

#[test]
fn keys_out_of_name_order_get_the_new_line_after_the_last_line_in_the_file() {
    let text = "[[bar]]\nright = [\"clock\"]\nname = \"main\"\nleft = [\"media\"]\n\n[[bar]]\nname = \"side\"\n";
    let p = plan(text, &[set("bar.main.height", json!(40))]).unwrap();
    assert_eq!(
        bar_file(&p).after,
        "[[bar]]\nright = [\"clock\"]\nname = \"main\"\nleft = [\"media\"]\nheight = 40\n\n[[bar]]\nname = \"side\"\n"
    );
    let p = plan(text, &[set("bar.side.height", json!(40))]).unwrap();
    assert!(bar_file(&p).after.ends_with("name = \"side\"\nheight = 40\n"));
}

#[test]
fn inline_and_single_table_bars_are_refused_with_the_line() {
    for (text, line) in [
        ("config_version = 1\nbar = [{ name = \"main\", height = 40 }]\n", 2),
        ("[bar]\nheight = 40\n", 1),
        ("[[bar]]\nname = \"main\"\nx.y = 1\n", 1),
    ] {
        let e = plan(text, &[set("bar.main.height", json!(41))]).unwrap_err();
        assert_eq!(e.code, ErrorCode::UnsupportedLayout, "{text}");
        assert!(e.message.starts_with(&format!("bar.toml:{line}:")), "{}", e.message);
    }
    let e = plan("[[bar]]\nheight = \"36\"\n", &[set("bar.main.height", json!(41))]).unwrap_err();
    assert_eq!(e.code, ErrorCode::UnsupportedLayout);
}

#[test]
fn an_existing_value_changes_alone_and_a_trailing_comment_stays() {
    let text = "[[bar]]\nname = \"main\"\nheight = 38   # mine\nfloating=false\n";
    let p = plan(text, &[set("bar.main.height", json!(40)), set("bar.main.floating", json!(true))]).unwrap();
    assert_eq!(bar_file(&p).after, "[[bar]]\nname = \"main\"\nheight = 40   # mine\nfloating=true\n");
    assert_eq!(bar_file(&p).splices.len(), 2);
    let p = plan(text, &[unset("bar.main.height")]).unwrap();
    assert_eq!(bar_file(&p).after, "[[bar]]\nname = \"main\"\nfloating=false\n");
    // Unsetting what the file does not say changes nothing and plans no file.
    let p = plan(text, &[unset("bar.main.position")]).unwrap();
    assert!(p.files.is_empty());
}

#[test]
fn a_line_added_to_a_file_with_no_final_break_gets_one() {
    let text = "[[bar]]\nname = \"main\"";
    let p = plan(text, &[set("bar.main.height", json!(40))]).unwrap();
    assert_eq!(bar_file(&p).after, "[[bar]]\nname = \"main\"\nheight = 40\n");
}

// ---------------------------------------------------------------- zones

#[test]
fn removing_a_widget_leaves_the_comments_in_the_array() {
    let p = plan(ARRAY_COMMENTS, &[remove("media", None)]).unwrap();
    let fp = bar_file(&p);
    assert!(
        fp.after.contains("left = [\n  \"workspaces\", # desktops\n  # \"title\",\n  \"clock\"\n]\n"),
        "{}",
        fp.after
    );
    check_file_plan(ARRAY_COMMENTS, fp, &[]);
    assert_eq!(names(&effective(&fp.after).bars[0].left), ["workspaces", "clock"]);
    assert_eq!(fp.what, "bar main: left [\"workspaces\", \"media\", \"clock\"] -> [\"workspaces\", \"clock\"]");
}

#[test]
fn adding_a_widget_keeps_the_style_of_the_array() {
    let p = plan(ARRAY_COMMENTS, &[add("text", "main", Zone::Left, Some(-1))]).unwrap();
    assert!(bar_file(&p).after.contains("  \"clock\",\n  \"text\"\n]"), "{}", bar_file(&p).after);
    let p = plan(ARRAY_COMMENTS, &[add("text", "main", Zone::Right, Some(1))]).unwrap();
    assert!(bar_file(&p).after.contains("    \"status\",\n    \"text\",\n    \"power\",\n]"), "{}", bar_file(&p).after);
    let p = plan(ARRAY_COMMENTS, &[add("text", "main", Zone::Right, None)]).unwrap();
    assert!(bar_file(&p).after.contains("    \"power\",\n    \"text\",\n]"), "{}", bar_file(&p).after);
    let p = plan(ARRAY_COMMENTS, &[add("text", "main", Zone::Center, Some(0))]).unwrap();
    assert!(bar_file(&p).after.contains("center = [\"text\", \"cpu\", # a\n"), "{}", bar_file(&p).after);
    let p = plan(ONE_PER_LINE, &[add("text", "main", Zone::Center, None)]).unwrap();
    assert!(bar_file(&p).after.ends_with("center = [\"text\"]\n"), "{}", bar_file(&p).after);
}

#[test]
fn a_whole_list_is_diffed_so_comments_inside_the_array_stay() {
    let p = plan(ARRAY_COMMENTS, &[set("bar.main.left", j(&["workspaces", "title", "media", "text"]))]).unwrap();
    let fp = bar_file(&p);
    assert!(fp.after.contains("  \"workspaces\", # desktops\n"), "{}", fp.after);
    assert!(fp.after.contains("  # \"title\",\n"), "{}", fp.after);
    check_file_plan(ARRAY_COMMENTS, fp, &[]);
    assert_eq!(names(&effective(&fp.after).bars[0].left), ["workspaces", "title", "media", "text"]);
    // An equal list writes nothing.
    let same = plan(ARRAY_COMMENTS, &[set("bar.main.left", j(&["workspaces", "media", "clock"]))]).unwrap();
    assert!(same.files.is_empty());
    // A reorder is one removal and one insertion.
    let p = plan(ARRAY_COMMENTS, &[set("bar.main.left", j(&["media", "clock", "workspaces"]))]).unwrap();
    assert_eq!(names(&effective(&bar_file(&p).after).bars[0].left), ["media", "clock", "workspaces"]);
    assert!(bar_file(&p).after.contains("  # \"title\",\n"), "{}", bar_file(&p).after);
}

#[test]
fn a_zone_the_preset_set_is_written_in_full_and_reported() {
    let text = "[[bar]]\nname = \"main\"\nleft = [\"workspaces\"]\n";
    let q = plan(text, &[add("text", "main", Zone::Center, Some(0))]).unwrap();
    let fp = bar_file(&q);
    assert!(
        fp.after.ends_with("center = [\"text\", \"memory\", \"cpu\", \"dashboard\", \"clock\", \"date\"]\n"),
        "{}",
        fp.after
    );
    let now = q.notices.iter().find(|n| n.code == NoticeCode::NowYours).expect("now_yours");
    assert_eq!(now.extra["zones"], json!([{"bar": "main", "zone": "center"}]));
    assert!(now.next.as_deref().unwrap().contains("rsb settings unset bar.main.center"));
    check_file_plan(text, fp, &[]);
}

#[test]
fn moving_a_widget_between_zones_changes_both_lists() {
    let text = "[[bar]]\nname = \"main\"\nleft = [\"workspaces\", \"media\"]\nright = [\"status\"]\n";
    let p = plan(text, &[mv("media", Some("main"), "main", Zone::Right, Some(0))]).unwrap();
    let cfg = effective(&bar_file(&p).after);
    assert_eq!(names(&cfg.bars[0].left), ["workspaces"]);
    assert_eq!(names(&cfg.bars[0].right), ["media", "status"]);
    // Within one zone the index is the final place.
    let p = plan(text, &[mv("workspaces", None, "main", Zone::Left, None)]).unwrap();
    assert_eq!(names(&effective(&bar_file(&p).after).bars[0].left), ["media", "workspaces"]);
}

#[test]
fn moving_the_pinned_widget_out_of_the_centre_removes_the_users_pin() {
    let text = "[[bar]]\nname = \"main\"\ncenter_on = \"dashboard\"\n";
    let p = plan(text, &[mv("dashboard", None, "main", Zone::Right, None)]).unwrap();
    let fp = bar_file(&p);
    assert!(!fp.after.contains("center_on"), "{}", fp.after);
    assert!(notice_codes(&p).contains(&NoticeCode::Unpinned));
    let cfg = effective(&fp.after);
    assert_eq!(cfg.bars[0].center_on, None);
    assert!(names(&cfg.bars[0].right).contains(&"dashboard".to_string()));
    // A pin from the preset needs nothing.
    let p = plan("[[bar]]\nname = \"main\"\n", &[mv("dashboard", None, "main", Zone::Right, None)]).unwrap();
    assert!(!notice_codes(&p).contains(&NoticeCode::Unpinned));
}

#[test]
fn the_last_widget_that_opens_a_popup_reports_it_and_another_one_does_not() {
    let p = plan(TEMPLATE_BAR, &[remove("date", None)]).unwrap();
    let n = p.notices.iter().find(|n| n.code == NoticeCode::PopupUnreachable).expect("popup_unreachable");
    assert_eq!((n.extra["popup"].as_str(), n.extra["widget"].as_str()), (Some("calendar"), Some("date")));
    assert!(n.next.as_deref().unwrap().contains("add date back"));
    // The change is still made.
    assert!(!effective(&bar_file(&p).after).bars[0].center.iter().any(|z| z.widget.name == "date"));
    let two = "[[bar]]\nname = \"main\"\nleft = [\"date\"]\ncenter = []\nright = [\"date#b\"]\n";
    let p = plan(two, &[remove("date", None)]).unwrap();
    assert!(!notice_codes(&p).contains(&NoticeCode::PopupUnreachable));
}

#[test]
fn widget_lookup_refusals() {
    let text = "[[bar]]\nname = \"main\"\nleft = [\"clock\", \"clock\"]\nright = [\"clock#b\"]\n[[bar]]\nname = \"side\"\nleft = [\"media\"]\ncenter = [\"clock\"]\n";
    assert_eq!(plan(text, &[remove("clock", None)]).unwrap_err().code, ErrorCode::AmbiguousWidget);
    assert_eq!(plan(text, &[remove("clock", Some("main"))]).unwrap_err().code, ErrorCode::AmbiguousWidget);
    assert_eq!(plan(text, &[remove("text", None)]).unwrap_err().code, ErrorCode::WidgetNotPlaced);
    assert_eq!(plan(text, &[add("clok", "main", Zone::Left, None)]).unwrap_err().code, ErrorCode::UnknownWidget);
    assert_eq!(plan(text, &[add("clock", "top", Zone::Left, None)]).unwrap_err().code, ErrorCode::UnknownBar);
    assert_eq!(plan(text, &[add("clock", "main", Zone::Left, Some(9))]).unwrap_err().code, ErrorCode::BadValue);
    assert_eq!(plan(text, &[add("clock", "main", Zone::Left, Some(-2))]).unwrap_err().code, ErrorCode::BadValue);
    let e = plan(text, &[add("clok", "main", Zone::Left, None)]).unwrap_err();
    assert!(e.message.contains("did you mean 'clock'"), "{}", e.message);
    // A bar name wins over an index.
    let named = "[[bar]]\nname = \"1\"\n[[bar]]\nname = \"x\"\n";
    let p = plan(named, &[set("bar.1.height", json!(40))]).unwrap();
    assert!(bar_file(&p).after.starts_with("[[bar]]\nname = \"1\"\nheight = 40\n"), "{}", bar_file(&p).after);
    let p = plan("[[bar]]\nname = \"a\"\n[[bar]]\nname = \"b\"\n", &[set("bar.1.height", json!(40))]).unwrap();
    assert!(bar_file(&p).after.ends_with("name = \"b\"\nheight = 40\n"), "{}", bar_file(&p).after);
}

// ---------------------------------------------------------------- preset switch

#[test]
fn a_preset_switch_reports_what_the_users_lines_still_override() {
    let text = "preset = \"default\"\n\n[[bar]]\nname = \"main\"\nheight = 30\nfloating = true\ncenter = [\"clock\"]\n";
    let p = plan(text, &[set("preset", json!("glass"))]).unwrap();
    let ps = p.preset_switch.clone().expect("preset_switch");
    assert_eq!((ps.from.as_str(), ps.to.as_str(), ps.bars_listed, ps.preset_bars), ("default", "glass", 1, 1));
    let kept = |key: &str| ps.kept.iter().find(|k| k.key == key).unwrap_or_else(|| panic!("{key} not kept")).clone();
    let h = kept("height");
    assert_eq!((h.yours.clone(), h.preset.clone(), h.same), (json!(30), json!(36), false));
    let f = kept("floating");
    assert_eq!((f.yours.clone(), f.preset.clone(), f.same), (json!(true), json!(true), true));
    let c = kept("center");
    assert_eq!((c.yours.clone(), c.preset.clone(), c.same), (json!(["clock"]), json!(["clock"]), true));
    assert!(ps.kept.iter().all(|k| k.bar == "main"));
    assert_eq!(ps.kept.len(), 3);
    // Only the preset value moved; everything else is as it was.
    assert_eq!(bar_file(&p).after, text.replace("\"default\"", "\"glass\""));
    let cfg = effective(&bar_file(&p).after);
    assert_eq!((cfg.preset.as_str(), cfg.bars[0].height, cfg.bars[0].radius), ("glass", 30, 12));
}

#[test]
fn reset_removes_the_users_bar_keys_zone_lists_and_accent_block() {
    let css = format!("bar {{ color: red }}\n\n{}", rsb_block());
    let text = "preset = \"default\"\n[[bar]]\nname = \"main\"\nheight = 30 # x\nleft = [\"clock\"]\nreserve = false\n";
    let p = run_plan(Some(text), Some(&css), &[set("preset", json!("glass"))], true).unwrap();
    let fp = bar_file(&p);
    assert_eq!(fp.after, "preset = \"glass\"\n[[bar]]\nname = \"main\"\nreserve = false\n");
    let c = css_file(&p);
    assert!(!c.after.contains("Colours set by rsb settings"), "{}", c.after);
    assert_eq!(c.after, "bar { color: red }\n");
    let cfg = effective(&fp.after);
    assert_eq!((cfg.bars[0].height, cfg.bars[0].floating, cfg.bars[0].reserve), (36, true, false));
    let e = run_plan(Some(text), None, &[set("bar.main.height", json!(40))], true).unwrap_err();
    assert_eq!(e.code, ErrorCode::BadUsage);
}

fn rsb_block() -> String {
    let p = run_plan(None, Some(""), &[set("color.accent", json!("#e5a50a"))], false).unwrap();
    css_file(&p).after.clone()
}

// ---------------------------------------------------------------- broken input

#[test]
fn a_broken_bar_toml_refuses_with_its_line_and_a_missing_one_is_made_from_the_template() {
    let e = plan("a = 1\n[[bar]]\nheight = \n", &[set("bar.main.height", json!(40))]).unwrap_err();
    assert_eq!(e.code, ErrorCode::FileBroken);
    assert!(e.message.starts_with("bar.toml:3:"), "{}", e.message);
    let e = plan("config_version = 2\n", &[set("preset", json!("glass"))]).unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfigTooNew);
    let p = run_plan(None, None, &[set("bar.main.height", json!(40))], false).unwrap();
    let fp = bar_file(&p);
    assert!(fp.created && fp.base == rsb_core::contract::FileHash::Missing);
    assert!(notice_codes(&p).contains(&NoticeCode::FileCreated));
    check_file_plan(TEMPLATE_BAR, fp, &live_lines(&p));
    assert_eq!(effective(&fp.after).bars[0].height, 40);
    // Nothing to do on a missing file: no file is planned.
    let p = run_plan(None, None, &[unset("bar.main.height")], false).unwrap();
    assert!(p.files.is_empty());
    assert!(matches!(plan("", &[]).unwrap_err().code, ErrorCode::BadUsage));
    assert_eq!(Source::User, Source::User);
}

/// A plan whose splices and new text agree with each other, but which drops the trailing comment of
/// the line it changes: the check must see it from the two texts alone.
#[test]
#[should_panic(expected = "the comment")]
fn the_file_check_sees_a_lost_trailing_comment_without_the_splices() {
    use rsb_core::cfgedit::{FilePlan, Splice};
    use rsb_core::contract::{ConfigFile, FileHash};
    let original = "[[bar]]\nheight = 36   # mine\n";
    let start = original.find("36").unwrap();
    let fp = FilePlan {
        file: ConfigFile::BarToml,
        base: FileHash::of(original.as_bytes()),
        created: false,
        bom: false,
        splices: vec![Splice { start, end: original.len() - 1, text: "40".into() }],
        after: "[[bar]]\nheight = 40\n".into(),
        what: String::new(),
    };
    check_file_plan(original, &fp, &[]);
}

#[test]
#[should_panic(expected = "comment line 1 lost")]
fn the_file_check_sees_a_lost_comment_line_without_the_splices() {
    use rsb_core::cfgedit::{FilePlan, Splice};
    use rsb_core::contract::{ConfigFile, FileHash};
    let original = "# keep me\nheight = 36\n";
    let fp = FilePlan {
        file: ConfigFile::BarToml,
        base: FileHash::of(original.as_bytes()),
        created: false,
        bom: false,
        splices: vec![Splice { start: 0, end: 10, text: String::new() }],
        after: "height = 36\n".into(),
        what: String::new(),
    };
    check_file_plan(original, &fp, &[]);
}

/// A one-splice plan over `original` that makes `after`, for what the file check must catch.
fn plan_of_one_splice(
    original: &str,
    start: usize,
    end: usize,
    text: &str,
    after: &str,
) -> rsb_core::cfgedit::FilePlan {
    use rsb_core::cfgedit::{FilePlan, Splice};
    use rsb_core::contract::{ConfigFile, FileHash};
    FilePlan {
        file: ConfigFile::BarToml,
        base: FileHash::of(original.as_bytes()),
        created: false,
        bom: false,
        splices: vec![Splice { start, end, text: text.into() }],
        after: after.into(),
        what: String::new(),
    }
}

#[test]
#[should_panic(expected = "the comment")]
fn the_file_check_sees_a_lost_comment_on_an_array_item() {
    let original = "[[bar]]\nleft = [\n  \"workspaces\", # desktops\n  \"clock\"\n]\n";
    let start = original.find("\"workspaces\"").unwrap();
    let end = original.find("\n  \"clock\"").unwrap();
    let fp = plan_of_one_splice(original, start, end, "\"workspaces\",", &original.replace(" # desktops", ""));
    check_file_plan(original, &fp, &[]);
}

#[test]
fn the_file_check_lets_a_comment_go_with_an_item_that_left_its_array() {
    let original = "[[bar]]\nleft = [\n  \"workspaces\", # desktops\n  \"clock\"\n]\nright = [\"status\"]\n";
    let after = "[[bar]]\nleft = [\n  \"clock\"\n]\nright = [\"status\", \"workspaces\"]\n";
    let first = original.find("  \"workspaces\"").unwrap();
    let second = original.find("\"status\"]").unwrap() + "\"status\"".len();
    let fp = rsb_core::cfgedit::FilePlan {
        splices: vec![
            rsb_core::cfgedit::Splice { start: first, end: original.find("  \"clock\"").unwrap(), text: String::new() },
            rsb_core::cfgedit::Splice { start: second, end: second, text: ", \"workspaces\"".into() },
        ],
        after: after.into(),
        ..plan_of_one_splice(original, 0, 0, "", after)
    };
    check_file_plan(original, &fp, &[]);
}

#[test]
#[should_panic(expected = "blank line")]
fn the_file_check_sees_a_blank_line_taken_with_a_change() {
    let original = "[[bar]]\nheight = 36\n\n[widget.clock]\nformat = \"HH:mm\"\n";
    let end = original.find("[widget").unwrap();
    let fp = plan_of_one_splice(
        original,
        original.find("36").unwrap(),
        end,
        "40\n",
        &format!("[[bar]]\nheight = 40\n{}", &original[end..]),
    );
    check_file_plan(original, &fp, &[]);
}

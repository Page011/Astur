//! The colours block in `style.css`, and the refusals that protect it.

#[path = "cfgedit_common.rs"]
mod common;

use common::*;
use rsb_core::contract::{ConfigFile, ErrorCode, FileHash, NoticeCode};
use serde_json::json;

const BLOCK_START: &str = "/* Colours set by rsb settings. Edit them here or on the settings page. */";
const BLOCK_END: &str = "/* End of colours set by rsb settings. */";

fn accent_plan(css: Option<&str>, value: &str) -> rsb_core::cfgedit::Plan {
    run_plan(None, css, &[set("color.accent", json!(value))], false).unwrap()
}

#[test]
fn a_block_is_appended_after_a_blank_line_and_everything_above_it_is_untouched() {
    let p = accent_plan(Some(TEMPLATE_CSS), "#e5a50a");
    let fp = css_file(&p);
    assert!(fp.after.starts_with(TEMPLATE_CSS));
    let tail = &fp.after[TEMPLATE_CSS.len()..];
    assert!(tail.starts_with(&format!("\n{BLOCK_START}\n@media (prefers-color-scheme: dark) {{\n    :root {{ --accent: #e5a50a; --accent-fg: #000000;")), "{tail}");
    assert!(tail.ends_with(&format!("{BLOCK_END}\n")));
    assert_eq!(fp.splices.len(), 1);
    check_file_plan(TEMPLATE_CSS, fp, &[]);
    assert_eq!(fp.what, "accent none -> #e5a50a");
    assert_eq!(p.accent.as_ref().map(|a| a.value.as_str()), Some("#e5a50a"));
    assert!(p.notices.is_empty(), "{:?}", p.notices);
    // bar.toml is not touched.
    assert!(p.file(ConfigFile::BarToml).is_none());
}

#[test]
fn a_three_digit_colour_is_written_in_full_lower_case() {
    let p = accent_plan(Some("bar { color: red }\n"), "#FA0");
    assert!(css_file(&p).after.contains("--accent: #ffaa00;"), "{}", css_file(&p).after);
}

#[test]
fn a_missing_style_css_is_made_from_the_template_then_given_the_block() {
    let p = accent_plan(None, "#e5a50a");
    let fp = css_file(&p);
    assert!(fp.created && fp.base == FileHash::Missing);
    assert!(fp.after.starts_with(TEMPLATE_CSS) && fp.after.contains(BLOCK_START));
    assert_eq!(notice_codes(&p), vec![NoticeCode::FileCreated]);
}

#[test]
fn a_second_accent_replaces_the_block_in_place() {
    let first = accent_plan(Some("bar { color: red }\n"), "#e5a50a");
    let one = &css_file(&first).after;
    let second = accent_plan(Some(one), "#112233");
    let fp = css_file(&second);
    assert!(fp.after.starts_with("bar { color: red }\n\n"));
    assert!(fp.after.contains("--accent: #112233;") && !fp.after.contains("#e5a50a"));
    assert_eq!(fp.after.matches(BLOCK_START).count(), 1);
    assert_eq!(fp.what, "accent #e5a50a -> #112233");
    assert_eq!((fp.splices.len(), fp.splices[0].start), (1, one.find(BLOCK_START).unwrap()));
    // The same colour again writes nothing.
    let same = accent_plan(Some(one), "#e5a50a");
    assert!(same.files.is_empty(), "{:?}", same.files);
}

#[test]
fn unsetting_the_accent_takes_the_block_and_its_blank_line_out_again() {
    let original = "bar { color: red }\n";
    let with = accent_plan(Some(original), "#e5a50a");
    let p = run_plan(None, Some(&css_file(&with).after), &[unset("color.accent")], false).unwrap();
    assert_eq!(css_file(&p).after, original);
    assert_eq!(css_file(&p).what, "accent #e5a50a -> none");
    // No block, nothing to do.
    let p = run_plan(None, Some(original), &[unset("color.accent")], false).unwrap();
    assert!(p.files.is_empty());
    // A set and an unset in one request leave the file as it was.
    let p =
        run_plan(None, Some(original), &[set("color.accent", json!("#e5a50a")), unset("color.accent")], false).unwrap();
    assert!(p.files.is_empty());
}

#[test]
fn crlf_files_get_a_crlf_block() {
    let css = crlf("bar { color: red }\n");
    let p = accent_plan(Some(&css), "#e5a50a");
    let after = &css_file(&p).after;
    assert_eq!(after.matches('\n').count(), after.matches("\r\n").count());
    let again = accent_plan(Some(after), "#000000");
    assert_eq!(css_file(&again).after.matches('\n').count(), css_file(&again).after.matches("\r\n").count());
}

#[test]
fn a_rule_that_wins_over_the_block_and_lines_it_beats_are_reported_with_their_place() {
    let css = ":root { --accent: #123456; }\nbar { color: red }\n";
    let p = accent_plan(Some(css), "#e5a50a");
    let n = p.notices.iter().find(|n| n.code == NoticeCode::ReplacesYours).expect("replaces_yours");
    assert_eq!(n.extra["at"], json!(["style.css:1:9"]));
    assert!(n.next.as_deref().unwrap().contains("style.css:1:9"));
    assert!(!notice_codes(&p).contains(&NoticeCode::Shadowed));
    // A later rule at equal or higher specificity shadows the block.
    let block = css_file(&accent_plan(Some(""), "#e5a50a")).after.clone();
    let later = format!("{block}\nbar {{ --accent: red; }}\n");
    let p = accent_plan(Some(&later), "#112233");
    let n = p.notices.iter().find(|n| n.code == NoticeCode::Shadowed).expect("shadowed");
    let at = n.extra["at"].as_str().unwrap();
    assert!(at.starts_with("style.css:"), "{at}");
    assert!(n.next.as_deref().unwrap().contains(at));
    // A lower-specificity later rule does not.
    let clean = format!("{block}\nwidget {{ color: red }}\n");
    assert!(!notice_codes(&accent_plan(Some(&clean), "#112233")).contains(&NoticeCode::Shadowed));
}

#[test]
fn a_damaged_or_edited_block_is_refused_and_a_broken_style_file_names_its_line() {
    let block = css_file(&accent_plan(Some(""), "#e5a50a")).after.clone();
    let e = run_plan(None, Some(&block.replace(BLOCK_END, "")), &[set("color.accent", json!("#000000"))], false)
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::ColorsBlockBroken);
    assert!(e.message.starts_with("style.css:1:") || e.message.starts_with("style.css:"), "{}", e.message);
    let two = format!("{block}{block}");
    assert_eq!(
        run_plan(None, Some(&two), &[unset("color.accent")], false).unwrap_err().code,
        ErrorCode::ColorsBlockBroken
    );
    let edited = block.replace(BLOCK_END, &format!("clock {{ color: red }}\n{BLOCK_END}"));
    assert_eq!(
        run_plan(None, Some(&edited), &[set("color.accent", json!("#000000"))], false).unwrap_err().code,
        ErrorCode::ColorsBlockEdited
    );
    let hand = block.replace("--accent-hover: ", "--accent-hover: red; --x: ");
    assert_eq!(
        run_plan(None, Some(&hand), &[set("color.accent", json!("#000000"))], false).unwrap_err().code,
        ErrorCode::ColorsBlockEdited
    );
    let e = run_plan(None, Some("bar {\n  color: red\n"), &[set("color.accent", json!("#000000"))], false).unwrap_err();
    assert_eq!(e.code, ErrorCode::FileBroken);
    assert_eq!(e.file, Some(ConfigFile::StyleCss));
    assert!(e.message.starts_with("style.css:"), "{}", e.message);
}

#[test]
fn a_broken_style_file_does_not_stop_a_change_to_bar_toml_and_the_other_way_round() {
    let broken_css = "bar {\n  color: red\n";
    let p = run_plan(Some("[[bar]]\nname = \"main\"\n"), Some(broken_css), &[set("bar.main.height", json!(40))], false)
        .unwrap();
    assert!(p.file(ConfigFile::BarToml).is_some() && p.file(ConfigFile::StyleCss).is_none());
    let p = run_plan(Some("a = \n"), Some("bar { color: red }\n"), &[set("color.accent", json!("#000000"))], false)
        .unwrap();
    assert!(p.file(ConfigFile::StyleCss).is_some());
    // Both files in one request.
    let p = run_plan(
        Some("[[bar]]\nname = \"main\"\n"),
        Some("bar { color: red }\n"),
        &[set("preset", json!("glass")), set("color.accent", json!("#e5a50a"))],
        false,
    )
    .unwrap();
    assert_eq!(p.files.iter().map(|f| f.file).collect::<Vec<_>>(), vec![ConfigFile::BarToml, ConfigFile::StyleCss]);
    assert!(p.preset_switch.as_ref().is_some_and(|s| s.accent_kept.as_deref() == Some("#e5a50a")));
}

#[test]
fn a_colour_edited_inside_the_block_is_replaced_and_the_reply_says_so() {
    let first = run_plan(None, Some(TEMPLATE_CSS), &[set("color.accent", json!("#3366cc"))], false).unwrap();
    let block_css = css_file(&first).after.clone();
    // Untouched, a second write to the same colour has nothing to say.
    let again = run_plan(None, Some(&block_css), &[set("color.accent", json!("#3366cc"))], false).unwrap();
    assert!(!notice_codes(&again).contains(&NoticeCode::BlockEditReplaced));
    // A person types a short colour into the block: it is read, and replaced with a notice.
    let by_hand: String = block_css
        .lines()
        .map(|l| match (l.find("--accent-hover"), l.find("--accent-press")) {
            (Some(from), Some(to)) => format!("{}--accent-hover: #abc; {}", &l[..from], &l[to..]),
            _ => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join(
            "
",
        )
        + "
";
    let p = run_plan(None, Some(&by_hand), &[set("color.accent", json!("#3366cd"))], false)
        .unwrap_or_else(|e| panic!("a short colour typed in the block blocks the write: {e:?}"));
    assert!(notice_codes(&p).contains(&NoticeCode::BlockEditReplaced), "{:?}", p.notices);
    assert!(!css_file(&p).after.to_ascii_lowercase().contains("#abc"), "the hand colour is gone");
    // Unsetting the accent removes that colour too, and says so; an untouched block goes without a word.
    let q = run_plan(None, Some(&by_hand), &[unset("color.accent")], false).unwrap();
    assert!(notice_codes(&q).contains(&NoticeCode::BlockEditReplaced), "{:?}", q.notices);
    let q = run_plan(None, Some(&block_css), &[unset("color.accent")], false).unwrap();
    assert!(!notice_codes(&q).contains(&NoticeCode::BlockEditReplaced), "{:?}", q.notices);
}

#[test]
fn a_reset_says_so_when_it_removes_a_hand_edited_colour() {
    let first = run_plan(None, Some(TEMPLATE_CSS), &[set("color.accent", json!("#3366cc"))], false).unwrap();
    let block_css = css_file(&first).after.clone();
    let reset = |css: &str| {
        run_plan(
            Some("config_version = 1\n\n[[bar]]\nname = \"main\"\n"),
            Some(css),
            &[set("preset", json!("glass"))],
            true,
        )
        .unwrap()
    };
    let p = reset(&block_css);
    assert!(css_file(&p).after.find(BLOCK_START).is_none(), "the block went");
    assert!(!notice_codes(&p).contains(&NoticeCode::BlockEditReplaced), "{:?}", p.notices);
    let at = block_css.find("--accent-hover: #").unwrap() + "--accent-hover: #".len();
    let by_hand = format!("{}abcdef{}", &block_css[..at], &block_css[at + 6..]);
    let p = reset(&by_hand);
    assert!(notice_codes(&p).contains(&NoticeCode::BlockEditReplaced), "{:?}", p.notices);
    assert!(!css_file(&p).after.contains("abcdef"));
}

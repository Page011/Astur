//! What the engine says about a change besides doing it: refusals with a
//! hint and a remedy, and notices that name what they are about.

#[path = "cfgedit_common.rs"]
mod common;

use common::*;
use rsb_core::contract::{ErrorCode, NoticeCode, Zone};
use serde_json::json;

#[test]
fn a_preset_name_in_capitals_is_an_unknown_preset_with_a_hint() {
    let e = plan(TEMPLATE_BAR, &[set("preset", json!("Glass"))]).unwrap_err();
    assert_eq!(e.code, ErrorCode::UnknownPreset, "{e:?}");
    assert!(e.message.contains("did you mean 'glass'"), "{}", e.message);
    assert_eq!(e.exit_code(), 2);
}

#[test]
fn every_zone_the_preset_set_is_named_with_a_remedy_for_each() {
    let text = "[[bar]]\nname = \"main\"\n";
    let p = plan(text, &[add("text", "main", Zone::Left, Some(0)), add("text", "main", Zone::Right, Some(0))]).unwrap();
    let now = p.notices.iter().find(|n| n.code == NoticeCode::NowYours).expect("now_yours");
    let next = now.next.as_deref().unwrap();
    assert!(next.contains("unset bar.main.left") && next.contains("unset bar.main.right"), "{next}");
    assert_eq!(
        now.extra["fix"],
        json!([{"op": "unset", "key": "bar.main.left"}, {"op": "unset", "key": "bar.main.right"}])
    );
}

#[test]
fn a_widget_that_is_not_on_the_bar_gets_advice_for_what_was_asked() {
    let text = "[[bar]]\nname = \"main\"\nleft = [\"workspaces\"]\ncenter = []\nright = [\"status\"]\n";
    let removing = plan(text, &[remove("memory", None)]).unwrap_err();
    assert_eq!(removing.code, ErrorCode::WidgetNotPlaced);
    assert!(!removing.next.as_deref().unwrap().contains("instead of moving"), "{:?}", removing.next);
    let moving = plan(text, &[mv("memory", None, "main", Zone::Left, None)]).unwrap_err();
    assert!(moving.next.as_deref().unwrap().contains("add it instead of moving it"), "{:?}", moving.next);
}

#[test]
fn a_wrong_index_says_where_the_widget_is() {
    use rsb_core::contract::{FromRef, Op, Target};
    let text = "[[bar]]\nname = \"main\"\nleft = [\"workspaces\", \"status\"]\ncenter = [\"status\"]\n";
    let from = FromRef::At(Target { bar: "main".into(), zone: Zone::Left, index: Some(0) });
    let op = Op::Remove { widget: "status".into(), from: Some(from) };
    let e = plan(text, &[op]).unwrap_err();
    assert_eq!(e.code, ErrorCode::WidgetNotPlaced);
    assert!(e.message.contains("main.left:1") && e.message.contains("main.center:0"), "{}", e.message);
}

#[test]
fn adding_a_widget_that_is_already_there_says_so_and_still_adds_it() {
    let text = "[[bar]]\nname = \"main\"\nleft = [\"workspaces\", \"status\"]\ncenter = []\nright = []\n";
    let p = plan(text, &[add("status", "main", Zone::Left, None)]).unwrap();
    assert_eq!(bar_file(&p).after.matches("\"status\"").count(), 2, "the change is made");
    let n = p.notices.iter().find(|n| n.code == NoticeCode::AlreadyPlaced).expect("already_placed");
    assert_eq!(n.extra["places"], json!(["left:1"]));
    // A widget that is not there yet says nothing.
    let q = plan(text, &[add("clock", "main", Zone::Left, None)]).unwrap();
    assert!(!notice_codes(&q).contains(&NoticeCode::AlreadyPlaced));
}

#[test]
fn a_menu_that_loses_two_widgets_is_reported_once() {
    use rsb_core::cfgedit::{FileTexts, PlanContext, plan_change_with};
    use rsb_core::contract::ConfigFile;
    use rsb_core::contrast::AccentSurfaces;
    // Two widgets open the same menu: it is lost once, not twice.
    let opens = |n: &str| if n == "media" || n == "date" { vec!["shared".to_string()] } else { Vec::new() };
    let preset = |n: &str| preset_toml(n);
    let exists = |n: &str| BUILTIN_WIDGETS.contains(&n);
    let template = |f: ConfigFile| template_for(f);
    let surfaces = AccentSurfaces::default();
    let ctx = PlanContext {
        reset: false,
        preset_toml: &preset,
        widget_exists: &exists,
        widget_opens: &opens,
        template: &template,
        surfaces: &surfaces,
        style_chain: &|_| None,
    };
    let text = "[[bar]]\nname = \"main\"\nleft = [\"media\", \"date\"]\ncenter = []\nright = []\n";
    let texts = FileTexts { bar_toml: Some(text.into()), style_css: None };
    let ops = [remove("media", None), remove("date", None)];
    let p = plan_change_with(&texts, &ops, &ctx, &fake_derive).unwrap();
    let n = p.notices.iter().filter(|n| n.code == NoticeCode::PopupUnreachable).count();
    assert_eq!(n, 1, "{:?}", p.notices);
}

#[test]
fn a_bar_list_notice_does_not_send_the_person_into_the_file() {
    let p = plan(TEMPLATE_BAR, &[set("bar.main.height", json!(40))]).unwrap();
    let n = p.notices.iter().find(|n| n.code == NoticeCode::BarsNowListed).expect("bars_now_listed");
    assert!(n.next.is_none(), "nothing to do about it: {:?}", n.next);
    assert!(!n.message.contains("[[bar]]"), "{}", n.message);
}

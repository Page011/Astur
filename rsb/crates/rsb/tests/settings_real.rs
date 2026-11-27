//! Settings verbs against real files and the real edit engine, with no bar:
//! what `get` and a write say about the same style sheet, and what a broken
//! or odd file in the config folder does to a write.

mod support;

use serde_json::Value;
use support::{Env, Out};

fn write(env: &Env, name: &str, text: &str) {
    std::fs::create_dir_all(env.cfg()).unwrap();
    std::fs::write(env.cfg().join(name), text).unwrap();
}

fn ok(out: &Out) -> Value {
    let v = out.json();
    assert!(out.code == 0 && v["ok"] == true, "{out:?}");
    v
}

fn notice<'a>(reply: &'a Value, code: &str) -> Option<&'a Value> {
    reply["notices"].as_array()?.iter().find(|n| n["code"] == code)
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

#[test]
fn an_accent_set_in_an_imported_file_is_reported_like_one_in_style_css() {
    let env = Env::new("real-import");
    write(&env, "style.css", "@import \"colors.css\";\n\nbar { color: red }\n");
    write(&env, "colors.css", ":root { --accent: #ff0000; }\n");
    let done = ok(&env.settings(&["set", "color.accent", "#e5a50a"]));
    let n = notice(&done, "replaces_yours").unwrap_or_else(|| panic!("no replaces_yours: {done}"));
    assert_eq!(strings(&n["at"]), vec!["colors.css:1:9".to_string()]);
    // `get` names the same place.
    let got = ok(&env.settings(&["get"]));
    for scheme in ["dark", "light"] {
        assert_eq!(strings(&got["colors"]["accent"][scheme]["overridden"]), vec!["colors.css:1:9".to_string()]);
    }
}

#[test]
fn get_and_a_write_name_the_same_places_for_the_same_style_sheet() {
    let env = Env::new("real-same");
    write(&env, "style.css", ":root { --accent: #123456; }\n\nbar { color: red }\n");
    let first = ok(&env.settings(&["set", "color.accent", "#e5a50a"]));
    let replaced = strings(&notice(&first, "replaces_yours").expect("an earlier accent line")["at"]);
    // A later rule after the block that sets another accent token.
    let css = env.read("style.css");
    write(&env, "style.css", &format!("{css}\n:root {{ --accent-text: #0000ff; }}\n"));
    let dry = ok(&env.settings(&["set", "color.accent", "#3366cc", "--dry-run"]));
    let shadow = notice(&dry, "shadowed").expect("a later rule");
    assert!(shadow["message"].as_str().unwrap().contains("--accent-text"), "{shadow}");
    let got = ok(&env.settings(&["get"]));
    let acc = &got["colors"]["accent"]["dark"];
    assert_eq!(acc["shadowed_by"], shadow["at"], "get and the write name the same rule");
    assert_eq!(strings(&acc["overridden"]), replaced);
}

#[test]
fn a_broken_imported_file_stops_a_style_write_with_its_place() {
    let env = Env::new("real-broken-import");
    write(&env, "style.css", "@import \"extra.css\";\n\nbar { color: red }\n");
    write(&env, "extra.css", "bar { color: \n");
    let before = env.read("style.css");
    let out = env.settings(&["set", "color.accent", "#3366cc"]);
    let v = out.json();
    assert_eq!(
        (out.code, v["ok"].clone(), v["error"]["code"].clone()),
        (1, Value::Bool(false), "file_broken".into()),
        "{out:?}"
    );
    assert!(v["error"]["at"].as_str().unwrap().starts_with("extra.css:"), "{v}");
    assert_eq!(env.read("style.css"), before, "nothing was written");
    assert!(!env.cfg().join(".undo").join("1").exists());
    // bar.toml does not depend on it.
    write(&env, "bar.toml", "config_version = 1\n\n[[bar]]\nname = \"main\"\nheight = 36\n");
    ok(&env.settings(&["set", "bar.main.height", "40"]));
}

#[test]
fn a_folder_or_a_link_named_like_a_config_file_is_linked_file() {
    let env = Env::new("real-folder");
    write(&env, "style.css", "bar { color: red }\n");
    std::fs::create_dir_all(env.cfg().join("bar.toml")).unwrap();
    let out = env.settings(&["set", "bar.main.height", "40"]);
    let v = out.json();
    assert_eq!((out.code, v["error"]["code"].clone()), (1, "linked_file".into()), "{out:?}");
    assert!(v["error"]["message"].as_str().unwrap().contains("a folder"), "{v}");
    // A change to the other file is not in its way.
    ok(&env.settings(&["set", "color.accent", "#3366cc"]));
}

#[test]
fn an_expect_naming_another_file_is_refused() {
    let env = Env::new("real-expect");
    write(&env, "style.css", "bar { color: red }\n");
    let request = env.dir.join("request.json");
    std::fs::write(
        &request,
        r##"{"expect":{"other.txt":"none"},"changes":[{"op":"set","key":"color.accent","value":"#3366cc"}]}"##,
    )
    .unwrap();
    let out = env.settings(&["change", request.to_str().unwrap()]);
    let v = out.json();
    assert_eq!((out.code, v["error"]["code"].clone()), (2, "bad_json".into()), "{out:?}");
    assert_eq!(env.read("style.css"), "bar { color: red }\n");
}

const GOOD_BAR: &str = "config_version = 1\n\n[[bar]]\nname = \"main\"\nheight = 36\n";

#[test]
fn a_broken_file_names_its_remedy_as_data_and_restore_is_offered_only_when_there_is_a_copy() {
    let env = Env::new("real-fix");
    write(&env, "bar.toml", "config_version = 1\n[[bar]]\nheight = \n");
    // Nothing has loaded clean yet: no copy, so the remedy is the file itself.
    let v = env.settings(&["set", "bar.main.height", "40"]).json();
    assert_eq!(v["error"]["code"], "file_broken", "{v}");
    assert!(v["error"]["next"].as_str().unwrap().contains("by hand"), "{v}");
    assert!(!v["error"]["next"].as_str().unwrap().contains("restore"), "{v}");
    let open = v["error"]["fix"]["open"].as_str().unwrap_or_default();
    assert!(open.ends_with("bar.toml") && !open.starts_with(r"\?\"), "{v}");
    // A write leaves a copy that loaded clean; after a bad hand edit restore is the remedy.
    write(&env, "bar.toml", GOOD_BAR);
    ok(&env.settings(&["set", "bar.main.height", "40"]));
    let good = env.read("bar.toml");
    write(&env, "bar.toml", "config_version = 1\n[[bar]]\nheight = \n");
    let v = env.settings(&["set", "bar.main.height", "44"]).json();
    assert_eq!(v["error"]["code"], "file_broken", "{v}");
    assert_eq!(v["error"]["fix"], serde_json::json!({"verb": "restore", "file": "bar.toml"}), "{v}");
    assert!(v["error"]["next"].as_str().unwrap().contains("rsb settings restore bar.toml"), "{v}");
    // And restore works with no bar running.
    let done = ok(&env.settings(&["restore", "bar.toml"]));
    assert_eq!(done["files"]["bar.toml"]["written"], true, "{done}");
    assert_eq!(env.read("bar.toml"), good);
}

#[test]
fn a_stale_read_and_an_undo_out_of_order_carry_their_remedy() {
    let env = Env::new("real-fix-2");
    write(&env, "bar.toml", GOOD_BAR);
    let stale = env.hash_of("bar.toml");
    ok(&env.settings(&["set", "bar.main.height", "40"]));
    let v = env.settings(&["set", "bar.main.height", "44", "--expect", &format!("bar.toml={stale}")]).json();
    assert_eq!(v["error"]["code"], "file_changed", "{v}");
    assert_eq!(v["error"]["fix"], serde_json::json!({"verb": "get"}), "{v}");
    ok(&env.settings(&["set", "bar.main.height", "48"]));
    let v = env.settings(&["undo", "1"]).json();
    assert_eq!(v["error"]["code"], "undo_not_latest", "{v}");
    assert_eq!(v["error"]["fix"], serde_json::json!({"verb": "undo", "n": 2}), "{v}");
}

#[test]
fn a_widget_can_be_named_by_its_zone_after_from() {
    let env = Env::new("real-from");
    write(
        &env,
        "bar.toml",
        "config_version = 1\n\n[[bar]]\nname = \"main\"\nleft = [\"workspaces\", \"clock\"]\ncenter = [\"clock\"]\nright = []\n",
    );
    // Twice on the bar: the bar alone is not enough, a zone is.
    let v = env.settings(&["remove", "clock"]).json();
    assert_eq!(v["error"]["code"], "ambiguous_widget", "{v}");
    ok(&env.settings(&["remove", "clock", "--from", "main.left"]));
    assert!(env.read("bar.toml").contains("left = [\"workspaces\"]"), "{}", env.read("bar.toml"));
    ok(&env.settings(&["move", "clock", "main.right:0", "--from", "main.center:0"]));
    assert!(env.read("bar.toml").contains("right = [\"clock\"]"), "{}", env.read("bar.toml"));
}

#[test]
fn a_word_with_control_characters_is_not_echoed_raw() {
    let env = Env::new("real-echo");
    let out = env.settings(&["get", "--bar", "x\u{1b}[2J\u{7}y"]);
    assert_eq!(out.code, 2, "{out:?}");
    assert!(!out.stderr.contains('\u{1b}') && !out.stderr.contains('\u{7}'), "{:?}", out.stderr);
    assert!(!out.stdout.contains('\u{1b}'), "{:?}", out.stdout);
}

#[test]
fn a_write_over_the_pipe_does_not_run_widget_modules_to_find_menus() {
    use rsb_app::msg::AppliedState;
    use rsb_app::settings::{Ctx, Transport, serve};
    use rsb_core::contract::{Op, Reply, Request, Verb};
    let env = Env::new("real-pipe-popups");
    let text = "config_version = 1\n\n[[bar]]\nname = \"main\"\nleft = [\"power\"]\ncenter = []\nright = []\n";
    write(&env, "bar.toml", text);
    let request = Request {
        verb: Some(Verb::Change),
        dry_run: true,
        changes: vec![Op::Remove { widget: "power".into(), from: None }],
        ..Request::default()
    };
    let codes = |reply: Reply| -> Vec<String> {
        let v = reply.to_json();
        assert_eq!(v["ok"], true, "{v}");
        v["notices"].as_array().unwrap().iter().filter_map(|n| n["code"].as_str().map(str::to_string)).collect()
    };
    let cli = Ctx::cli(env.cfg(), env.logs(), true);
    assert!(
        codes(serve(request.clone(), &cli)).contains(&"popup_unreachable".to_string()),
        "the command line checks menus"
    );
    let applied =
        std::sync::Arc::new(AppliedState::new(env.cfg().display().to_string(), env.logs().display().to_string()));
    let pipe = Ctx {
        transport: Transport::Pipe,
        config_dir: env.cfg(),
        log_dir: env.logs(),
        applied: Some(applied),
        no_wait: true,
        pipe_client_ok: true,
    };
    assert!(
        !codes(serve(request, &pipe)).contains(&"popup_unreachable".to_string()),
        "the pipe thread runs no widget module"
    );
}

/// The colours block with the digits of every hex colour in capitals: the same colours, written differently.
fn capitals_in_block(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut chars = css.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        if c == '#' {
            while let Some(h) = chars.next_if(char::is_ascii_hexdigit) {
                out.push(h.to_ascii_uppercase());
            }
        }
    }
    out
}

/// The hover shade of the block replaced by another colour, as a person editing the file would.
fn hover_by_hand(css: &str) -> String {
    let at = css.find("--accent-hover: #").expect("the block holds a hover shade") + "--accent-hover: #".len();
    format!("{}abcdef{}", &css[..at], &css[at + 6..])
}

fn has_edit_notice(reply: &Value) -> bool {
    notice(reply, "block_edit_replaced").is_some()
}

#[test]
fn unsetting_an_untouched_accent_through_the_real_engine_says_nothing_about_edits() {
    let env = Env::new("real-unset-clean");
    ok(&env.settings(&["set", "color.accent", "#3366cc", "--no-wait"]));
    let done = ok(&env.settings(&["unset", "color.accent", "--no-wait"]));
    assert!(!has_edit_notice(&done), "{done}");
    // Capitals only: the same colours, so nothing of the person's is lost.
    ok(&env.settings(&["set", "color.accent", "#3366cc", "--no-wait"]));
    let css = env.read("style.css");
    write(&env, "style.css", &capitals_in_block(&css));
    let done = ok(&env.settings(&["unset", "color.accent", "--no-wait"]));
    assert!(!has_edit_notice(&done), "{done}");
}

#[test]
fn unsetting_an_accent_whose_block_was_edited_by_hand_says_so() {
    let env = Env::new("real-unset-edited");
    ok(&env.settings(&["set", "color.accent", "#3366cc", "--no-wait"]));
    let css = env.read("style.css");
    write(&env, "style.css", &hover_by_hand(&css));
    let done = ok(&env.settings(&["unset", "color.accent", "--no-wait"]));
    assert!(has_edit_notice(&done), "{done}");
    assert!(!env.read("style.css").contains("abcdef"), "the colour went with the block");
}

#[test]
fn a_preset_reset_says_so_when_it_removes_a_hand_edited_colour() {
    let env = Env::new("real-reset");
    ok(&env.settings(&["set", "color.accent", "#3366cc", "--no-wait"]));
    let done = ok(&env.settings(&["set", "preset", "fluent", "--reset", "--no-wait"]));
    assert!(!has_edit_notice(&done), "an untouched block goes without a word: {done}");
    ok(&env.settings(&["set", "color.accent", "#3366cc", "--no-wait"]));
    let css = env.read("style.css");
    write(&env, "style.css", &hover_by_hand(&css));
    let done = ok(&env.settings(&["set", "preset", "glass", "--reset", "--no-wait"]));
    assert!(has_edit_notice(&done), "{done}");
    assert!(!env.read("style.css").contains("abcdef"), "the colour went with the block");
}

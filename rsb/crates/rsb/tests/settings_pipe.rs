//! The settings commands over the running bar's pipe and over the command
//! line give the same answers; a write over the pipe never waits; a bar that
//! reads another folder is said so; `rsb inspect` against a live bar.
//!
//! The runs that start a bar show windows far outside the screen, so they go
//! through the desktop lock:
//!
//! `deskrun.sh cargo test -p rsb --test settings_pipe -- --ignored --test-threads=1`

mod support;

use std::os::windows::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

use rsb_app::pipe;
use rsb_core::ipc::{Command, Reply};
use serde_json::{Value, json};
use support::Env;

/// One settings line sent to the bar, the way a program with a pipe handle does it.
fn over_pipe(env: &Env, request: &Value) -> Value {
    let name = pipe::pipe_name_tagged(Some(&env.tag)).expect("a pipe name");
    match pipe::send_to(&name, &Command::Settings(request.to_string()), Duration::from_secs(5)) {
        Ok(Reply::Ok(text)) => serde_json::from_str(&text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}")),
        Ok(Reply::Err(why)) => panic!("the bar refused the line: {why}"),
        Err(e) => panic!("the pipe: {e}"),
    }
}

fn cli(env: &Env, words: &[&str]) -> Value {
    let out = env.settings(words);
    assert_eq!(out.code, 0, "{out:?}");
    out.json()
}

/// A write reply without the bar's side: the command line waits to see the
/// files shown, the pipe never waits, so that block is the one thing that differs.
fn without_bar(mut v: Value) -> Value {
    if let Some(map) = v.as_object_mut() {
        map.remove("bar");
    }
    v
}

const SLOW_WIDGET: &str = "local rsb = require(\"@rsb\")\nreturn rsb.widget {\n    setup = function()\n        __test_stall(3500)\n        return rsb.text \"slow\"\n    end,\n}\n";

#[test]
fn inspect_with_no_bar_exits_3() {
    let env = Env::new("settings-pipe-nobar");
    let out = env.inspect(&["--json"]);
    assert_eq!(out.code, 3, "{out:?}");
    assert!(out.stderr.contains("no bar running"), "{out:?}");
    let get = env.settings(&["get"]);
    assert_eq!(get.code, 0, "no bar is not an error for settings: {get:?}");
    assert_eq!(get.json()["bar"]["running"], false);
}

#[test]
#[ignore = "shows a window: run through the desktop lock"]
fn pipe_and_cli_serve_the_same() {
    let env = Env::new("settings-pipe-same");
    let mut bar = env.start_bar(30);
    env.wait_for_bar();

    let get_cli = cli(&env, &["get"]);
    let get_pipe = over_pipe(&env, &json!({"contract": 1, "verb": "get"}));
    assert_eq!(get_cli, get_pipe, "get");
    assert_eq!(get_pipe["bar"]["running"], true);

    let set = json!({"contract": 1, "changes": [{"op": "set", "key": "bar.main.height", "value": 40}]});
    let set_cli = without_bar(cli(&env, &["set", "bar.main.height", "40"]));
    let undo_cli = without_bar(cli(&env, &["undo"]));
    let set_pipe = without_bar(over_pipe(&env, &set));
    let undo_pipe = without_bar(over_pipe(&env, &json!({"contract": 1, "verb": "undo"})));
    assert_eq!(set_cli["ok"], true, "{set_cli}");
    assert_eq!(set_cli, set_pipe, "change");
    assert_eq!(undo_cli, undo_pipe, "undo");

    // Listing widgets runs Luau modules: not work for the bar's process.
    let widgets = over_pipe(&env, &json!({"contract": 1, "verb": "widgets"}));
    assert_eq!(
        (widgets["ok"].as_bool(), widgets["error"]["code"].as_str()),
        (Some(false), Some("cli_only")),
        "{widgets}"
    );

    assert_eq!(env.plain(&["quit"]).code, 0);
    assert!(bar.wait().unwrap().success());
}

#[test]
#[ignore = "shows a window: run through the desktop lock"]
fn pipe_write_busy_at_once() {
    let env = Env::new("settings-pipe-busy");
    let mut bar = env.start_bar(30);
    env.wait_for_bar();
    // Hold the writer's lock the way a write in progress does: no sharing, gone when closed.
    let undo = env.cfg().join(".undo");
    std::fs::create_dir_all(&undo).unwrap();
    let held = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .access_mode(0x4000_0000 | 0x0001_0000)
        .share_mode(0)
        .custom_flags(0x0400_0000)
        .open(undo.join("lock"))
        .expect("the lock file can be held");
    let before = env.hash_of("bar.toml");
    let request = json!({"contract": 1, "changes": [{"op": "set", "key": "bar.main.height", "value": 40}]});
    let mut worst = Duration::ZERO;
    for _ in 0..5 {
        let t = Instant::now();
        let reply = over_pipe(&env, &request);
        let took = t.elapsed();
        worst = worst.max(took);
        assert_eq!((reply["ok"].as_bool(), reply["error"]["code"].as_str()), (Some(false), Some("busy")), "{reply}");
        assert!(took < Duration::from_millis(50), "busy took {} ms", took.as_millis());
    }
    eprintln!(
        "a write over the pipe with the lock held, {}: worst of 5 {} ms",
        rsb_app::stats::session_name(),
        worst.as_millis()
    );
    drop(held);
    assert_eq!(env.hash_of("bar.toml"), before, "a refused write left the file as it was");
    // With the lock free the same request goes through.
    let done = over_pipe(&env, &request);
    assert_eq!(done["ok"], true, "{done}");
    assert_eq!(env.plain(&["quit"]).code, 0);
    assert!(bar.wait().unwrap().success());
}

#[test]
#[ignore = "shows a window: run through the desktop lock"]
fn bar_in_other_folder_noticed() {
    let env = Env::new("settings-pipe-other");
    let mut bar = env.start_bar(30);
    env.wait_for_bar();
    let other = env.dir.join("other");
    let t = Instant::now();
    let out = env
        .command()
        .args(["settings", "set", "bar.main.height", "40", "--config-dir"])
        .arg(&other)
        .arg("--log-dir")
        .arg(env.logs())
        .output()
        .expect("rsb runs");
    let took = t.elapsed();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let reply: Value = serde_json::from_slice(&out.stdout).expect("one line of JSON");
    assert_eq!(reply["bar"]["running"], true, "{reply}");
    assert!(
        reply["bar"]["shows_files"].is_null() && reply["bar"]["ms"].is_null(),
        "no waiting for another folder: {reply}"
    );
    let notice = reply["notices"]
        .as_array()
        .and_then(|n| n.iter().find(|n| n["code"] == "bar_uses_other_folder"))
        .unwrap_or_else(|| panic!("no bar_uses_other_folder notice: {reply}"));
    assert!(notice["bar_dir"].as_str().is_some_and(|d| !d.is_empty()), "{notice}");
    assert!(took < Duration::from_millis(1500), "the call waited {} ms", took.as_millis());
    assert!(other.join("bar.toml").is_file(), "the change went to the folder asked for");
    assert_eq!(env.plain(&["quit"]).code, 0);
    assert!(bar.wait().unwrap().success());
}

#[test]
#[ignore = "shows a window: run through the desktop lock"]
fn inspect_reads_a_live_bar_and_says_when_the_script_thread_is_busy() {
    let env = Env::new("settings-pipe-inspect");
    std::fs::create_dir_all(env.cfg().join("widgets")).unwrap();
    std::fs::write(env.cfg().join("widgets").join("slow.luau"), SLOW_WIDGET).unwrap();
    std::fs::write(
        env.cfg().join("bar.toml"),
        "config_version = 1\npreset = \"fluent\"\n[[bar]]\nname = \"main\"\nleft = [\"slow\"]\nright = [\"clock\"]\n",
    )
    .unwrap();
    let mut bar = env
        .command()
        .arg("--config-dir")
        .arg(env.cfg())
        .arg("--log-dir")
        .arg(env.logs())
        .args(["--no-reserve", "--test-offscreen", "--no-hook", "--no-activate", "--fake-services"])
        .args(["--test-script-stall", "slow:3500", "--seconds", "20"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the bar starts");
    env.wait_for_bar();
    // The script thread is inside the widget's setup: its part times out, the rest of the reply stands.
    let t = Instant::now();
    let out = env.inspect(&["--json"]);
    let took = t.elapsed();
    assert_eq!(out.code, 0, "{out:?}");
    let v = out.json();
    assert_eq!(v["parts"]["script"], "timeout", "{v}");
    assert_eq!(v["parts"]["ui"], "ok", "{v}");
    assert!(v["bars"][0]["root_vars"]["accent"].is_string(), "the bar's colours come from the UI thread: {v}");
    assert!(took < Duration::from_millis(3000), "the answer took {} ms", took.as_millis());
    // The bar's root node on its own.
    let root = env.inspect(&["--bar", "main", "--style", "--json"]);
    assert_eq!(root.code, 0, "{root:?}");
    assert!(root.json()["bars"][0]["root"]["kind"].is_string());
    assert_eq!(env.plain(&["quit"]).code, 0);
    assert!(bar.wait().unwrap().success());
}

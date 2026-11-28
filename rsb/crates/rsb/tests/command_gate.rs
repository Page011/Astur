//! Test runs never reach the real PC. Every flag that drives the bar for a
//! test, `RSB_TEST_TAG` and `rsb render` select the command recorder; only
//! `--fake-services` lets commands through (to the fakes); a plain run has no
//! recorder. The selection is checked flag by flag through the real argument
//! parser; the two ignored tests run the binary (through the desktop lock)
//! and read what it reports.

use std::path::{Path, PathBuf};
use std::process::Command;

use rsb_app::args::{self, Cmd, RunOpts};
use rsb_app::gate::{Selection, select_render, select_run};

const EXE: &str = env!("CARGO_BIN_EXE_rsb");

fn scratch(tag: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("gate-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the scratch folder can be made");
    dir
}

fn parse(words: &[&str]) -> Result<RunOpts, String> {
    let words: Vec<String> = words.iter().map(|w| w.to_string()).collect();
    match args::parse(&words)?.cmd {
        Cmd::Run(o) => Ok(*o),
        other => panic!("not a run: {other:?}"),
    }
}

/// What a run with these words does with commands.
fn selected(words: &[&str]) -> Option<Selection> {
    select_run(&parse(words).unwrap_or_else(|e| panic!("{words:?}: {e}")), false)
}

#[test]
fn every_test_flag_selects_the_recorder_and_none_forwards_to_a_real_service() {
    let pointer = scratch("flags").join("pointer.txt");
    let pointer = pointer.to_str().unwrap();
    let recorded = Some(Selection { forward: false });
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("--test-offscreen", vec!["--test-offscreen"]),
        ("--simulate-taskbar-created", vec!["--simulate-taskbar-created", "100"]),
        ("--simulate-dpi", vec!["--simulate-dpi", "100:120"]),
        ("--simulate-display-change", vec!["--simulate-display-change", "100"]),
        ("--simulate-theme", vec!["--simulate-theme", "100:dark"]),
        ("--simulate-fullscreen", vec!["--simulate-fullscreen", "100:on"]),
        ("--simulate-clock-change", vec!["--simulate-clock-change", "100:time"]),
        ("--simulate-pointer", vec!["--simulate-pointer", pointer]),
        ("--simulate-key", vec!["--simulate-key", pointer]),
        ("--simulate-popup", vec!["--simulate-popup", "100:launcher:open"]),
        ("--simulate-outside-press", vec!["--simulate-outside-press", "100"]),
        ("--simulate-deactivate", vec!["--simulate-deactivate", "100:launcher"]),
        ("--simulate-session", vec!["--simulate-session", "100:lock"]),
        ("--simulate-popup-dpi", vec!["--simulate-popup-dpi", "100:launcher:144"]),
        ("--test-ui-stall", vec!["--test-ui-stall", "100:300"]),
        ("--no-hook", vec!["--no-hook"]),
        ("--no-activate", vec!["--no-activate"]),
        ("--fake-glazewm", vec!["--fake-glazewm"]),
        ("--test-fullscreen-window", vec!["--test-fullscreen-window", "100:200"]),
        ("--test-script-stall", vec!["--test-script-stall", "clock:500"]),
        ("--test-render-failure", vec!["--test-render-failure", "100"]),
        ("--test-panic", vec!["--test-panic", "100"]),
    ];
    for (flag, words) in &cases {
        assert_eq!(selected(words), recorded, "{flag} must select the recorder without forwarding");
        let mut with_stats = words.clone();
        with_stats.extend(["--seconds", "1", "--no-reserve"]);
        assert_eq!(selected(&with_stats), recorded, "{flag} with measurement flags");
    }
}

#[test]
fn the_fakes_take_commands_and_a_capture_run_always_has_them() {
    assert_eq!(selected(&["--fake-services"]), Some(Selection { forward: true }));
    assert_eq!(selected(&["--fake-services", "--test-offscreen"]), Some(Selection { forward: true }));
    assert_eq!(selected(&["--service-storm", "10:1", "--fake-services"]), Some(Selection { forward: true }));
    let dir = scratch("capture");
    let dir = dir.to_str().unwrap();
    assert_eq!(selected(&["--capture", dir, "--fake-services", "--test-offscreen"]), Some(Selection { forward: true }));
    // A capture without the fakes would write a real launcher or track title to disk.
    let err = parse(&["--capture", dir]).unwrap_err();
    assert_eq!(err, "--capture needs --fake-services");
    let err = parse(&["--capture", dir, "--fake-services"]).unwrap_err();
    assert_eq!(err, "--capture needs --test-offscreen");
    let err =
        parse(&["--capture", "C:\\Users\\someone\\Documents", "--fake-services", "--test-offscreen"]).unwrap_err();
    assert!(err.contains("scratch or target folder"), "{err}");
}

#[test]
fn a_plain_run_and_the_measurement_flags_alone_have_no_recorder() {
    assert_eq!(selected(&[]), None, "a plain run uses the real services");
    for words in [
        vec!["--seconds", "5"],
        vec!["--no-reserve"],
        vec!["--stats-out", "s.jsonl"],
        vec!["--stats-frames"],
        vec!["--probe-ui"],
    ] {
        assert_eq!(selected(&words), None, "{words:?} only measures a normal run");
    }
}

#[test]
fn the_test_tag_and_rsb_render_select_the_recorder() {
    let plain = parse(&[]).unwrap();
    assert_eq!(select_run(&plain, true), Some(Selection { forward: false }), "RSB_TEST_TAG alone");
    let fake = parse(&["--fake-services"]).unwrap();
    assert_eq!(select_run(&fake, true), Some(Selection { forward: true }));
    assert_eq!(select_render(false), Selection { forward: true }, "render uses the fakes");
    assert_eq!(select_render(true), Selection { forward: false }, "render --live reads the PC and sends nothing");
}

#[test]
fn a_capture_run_is_refused_by_the_binary_with_exit_code_2() {
    let dir = scratch("capture-exit");
    let out = Command::new(EXE)
        .env("RSB_TEST_TAG", format!("gate-capture-{}", std::process::id()))
        .args(["--capture"])
        .arg(&dir)
        .output()
        .expect("rsb starts");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--capture needs --fake-services"));
    let out = Command::new(EXE)
        .env("RSB_TEST_TAG", format!("gate-capture-{}", std::process::id()))
        .args(["--fake-services", "--capture"])
        .arg(&dir)
        .output()
        .expect("rsb starts");
    assert_eq!(out.status.code(), Some(2), "a capture needs an offscreen run too");
    assert!(String::from_utf8_lossy(&out.stderr).contains("--capture needs --test-offscreen"));
}

/// A widget whose click asks the audio service for a volume change of zero,
/// so even a command that got through would change nothing.
const CLICKER: &str = "local rsb = require(\"@rsb\")\nreturn rsb.widget {\n    permissions = { control = { \"audio\" } },\n    setup = function()\n        return rsb.text {\n            text = \"gate test\",\n            on_click = function()\n                rsb.services.audio:change_volume(0)\n            end,\n        }\n    end,\n}\n";

/// Runs a bar for three seconds with these flags, clicks the widget, and
/// returns the stats lines.
fn clicked_run(tag: &str, extra: &[&str]) -> Vec<serde_json::Value> {
    let dir = scratch(tag);
    let cfg = dir.join("cfg");
    std::fs::create_dir_all(cfg.join("widgets")).unwrap();
    std::fs::write(cfg.join("widgets").join("clicker.luau"), CLICKER).unwrap();
    std::fs::write(
        cfg.join("bar.toml"),
        "config_version = 1\npreset = \"fluent\"\n[[bar]]\nname = \"main\"\nleft = [\"clicker\"]\n",
    )
    .unwrap();
    let pointer = dir.join("pointer.txt");
    std::fs::write(&pointer, "1500 main move 12 12\n1600 main down 12 12 left\n1700 main up 12 12 left\n").unwrap();
    let stats = dir.join("stats.jsonl");
    let out = Command::new(EXE)
        .env("RSB_TEST_TAG", format!("gate-{tag}-{}", std::process::id()))
        .arg("--config-dir")
        .arg(&cfg)
        .arg("--log-dir")
        .arg(dir.join("logs"))
        .arg("--stats-out")
        .arg(&stats)
        .args(["--no-reserve", "--test-offscreen", "--seconds", "3", "--simulate-pointer"])
        .arg(&pointer)
        .args(extra)
        .output()
        .expect("rsb starts");
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    std::fs::read_to_string(&stats)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).expect("a stats line is JSON"))
        .collect()
}

fn events<'a>(lines: &'a [serde_json::Value], name: &str) -> Vec<&'a serde_json::Value> {
    lines.iter().filter(|l| l["event"] == name).collect()
}

#[test]
#[ignore = "shows a window: run through the desktop lock"]
fn a_click_in_a_test_run_on_the_real_services_is_recorded_and_stopped() {
    let lines = clicked_run("real", &[]);
    let gate = events(&lines, "gate");
    assert_eq!(gate.len(), 1);
    assert_eq!((gate[0]["recorder"].as_bool(), gate[0]["forward"].as_bool()), (Some(true), Some(false)));
    let commands = events(&lines, "command");
    assert_eq!(commands.len(), 1, "one click, one command: {lines:?}");
    assert_eq!((commands[0]["service"].as_str(), commands[0]["kind"].as_str()), (Some("audio"), Some("change_volume")));
    assert_eq!(commands[0]["forwarded"], false);
    assert!(events(&lines, "service_command").is_empty(), "no fake saw it, and no real service was asked");
}

#[test]
#[ignore = "shows a window: run through the desktop lock"]
fn a_click_in_a_test_run_on_the_fakes_reaches_the_fake_and_is_recorded() {
    let lines = clicked_run("fake", &["--fake-services"]);
    let gate = events(&lines, "gate");
    assert_eq!((gate[0]["recorder"].as_bool(), gate[0]["forward"].as_bool()), (Some(true), Some(true)));
    let commands = events(&lines, "command");
    assert_eq!(commands.len(), 1, "{lines:?}");
    assert_eq!(commands[0]["forwarded"], true);
    let fake = events(&lines, "service_command");
    assert_eq!(fake.len(), 1, "the fake took it: {lines:?}");
    assert_eq!(fake[0]["service"], "audio");
}

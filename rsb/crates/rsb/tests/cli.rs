//! The real binary: `rsb render` with misbehaving widgets, exit codes as
//! cmd.exe reports them, eject and types, and (through the desktop lock)
//! a running bar answering every command over its pipe.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_rsb");

fn scratch(tag: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the scratch folder can be made");
    dir
}

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// rsb with its own pipe name, so no test ever reaches a real bar.
fn rsb(tag: &str) -> Command {
    let mut c = Command::new(EXE);
    c.env("RSB_TEST_TAG", tag);
    c
}

fn run(c: &mut Command) -> (i32, String, String) {
    let out = c.output().expect("rsb starts");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

const HOG: &str = "local rsb = require(\"@rsb\")\nreturn rsb.widget {\n    setup = function()\n        while true do end\n        return rsb.text \"never\"\n    end,\n}\n";
const DENIED: &str = "local rsb = require(\"@rsb\")\nreturn rsb.widget {\n    setup = function()\n        rsb.task.spawn(function()\n            rsb.http.get(\"https://example.com/\")\n        end)\n        return rsb.text \"denied\"\n    end,\n}\n";

#[test]
fn render_with_a_runaway_and_a_denied_widget_exits_zero_with_both_plates_in_the_log() {
    let dir = scratch("plates");
    let (cfg, logs) = (dir.join("cfg"), dir.join("logs"));
    write(&cfg, "widgets/hog.luau", HOG);
    write(&cfg, "widgets/denied.luau", DENIED);
    write(
        &cfg,
        "bar.toml",
        "config_version = 1\n[[bar]]\nleft = [\"hog\", \"denied\"]\ncenter = [\"clock\"]\nright = [\"cpu\"]\n",
    );
    let png = dir.join("plates.png");
    let (code, stdout, stderr) = run(rsb("plates")
        .args(["render", "--out"])
        .arg(&png)
        .args(["--wait-ms", "900", "--config-dir"])
        .arg(&cfg)
        .arg("--log-dir")
        .arg(&logs));
    assert_eq!(code, 0, "stdout: {stdout}\nstderr: {stderr}");
    assert!(std::fs::metadata(&png).is_ok_and(|m| m.len() > 1000), "a picture was written");
    let log = std::fs::read_to_string(logs.join("rsb.log")).unwrap();
    let hog =
        log.lines().find(|l| l.contains("plate on hog:")).unwrap_or_else(|| panic!("no hog plate in the log:\n{log}"));
    assert!(hog.contains("STOPPED - hog ran over"), "{hog}");
    let denied =
        log.lines().find(|l| l.contains("plate on denied:")).unwrap_or_else(|| panic!("no denied plate:\n{log}"));
    assert!(denied.contains("PERMISSION - denied needs net:example.com"), "{denied}");
    assert!(stdout.contains("plate: hog:") && stdout.contains("plate: denied:"), "{stdout}");
    assert!(log.contains("plate on hog") && !log.contains("panicked"), "no panic reached the log");
}

/// Runs a `.cmd` file with `cmd /d /c` and returns `name=code` pairs, so the
/// exit codes are the ones `%ERRORLEVEL%` shows a person.
fn errorlevels(dir: &Path, tag: &str, lines: &[(&str, String)]) -> Vec<(String, i32)> {
    let mut script = String::from("@echo off\r\n");
    for (name, args) in lines {
        script.push_str(&format!("\"{EXE}\" {args} >nul 2>nul\r\necho {name}=%ERRORLEVEL%\r\n"));
    }
    let file = dir.join("codes.cmd");
    std::fs::write(&file, script).unwrap();
    let out = Command::new("cmd").args(["/d", "/c"]).arg(&file).env("RSB_TEST_TAG", tag).output().expect("cmd runs");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().split_once('='))
        .filter_map(|(n, c)| Some((n.to_string(), c.trim().parse().ok()?)))
        .collect()
}

#[test]
fn exit_codes_are_the_ones_cmd_exe_reports() {
    let dir = scratch("codes");
    let (cfg, logs) = (dir.join("cfg"), dir.join("logs"));
    let dirs = format!("--config-dir \"{}\" --log-dir \"{}\"", cfg.display(), logs.display());
    let lines: Vec<(&str, String)> = vec![
        ("help", "--help".into()),
        ("version", "--version".into()),
        ("unknown", "frobnicate".into()),
        ("badflag", "--nonsense".into()),
        ("noconfigdir", "--config-dir".into()),
        ("reload", format!("reload {dirs}")),
        ("quit", format!("quit {dirs}")),
        ("toggle", format!("toggle {dirs}")),
        ("status", format!("status {dirs}")),
        ("statusbad", format!("status --yaml {dirs}")),
        ("statusjson", format!("status --json {dirs}")),
        ("trigger", format!("trigger deploy {dirs}")),
        ("badtrigger", "trigger Bad!".into()),
        ("badjson", "trigger deploy {nope".into()),
        ("ejectnone", format!("eject nothing {dirs}")),
        ("eject", format!("eject clock {dirs}")),
        ("ejectagain", format!("eject clock {dirs}")),
        ("ejectusage", format!("eject {dirs}")),
        ("types", format!("types {dirs}")),
        ("rendernoout", format!("render {dirs}")),
        ("renderpreset", format!("render --out \"{}\" --preset nope {dirs}", dir.join("x.png").display())),
        ("render", format!("render --out \"{}\" --wait-ms 100 {dirs}", dir.join("ok.png").display())),
    ];
    let got = errorlevels(&dir, "codes", &lines);
    let get = |n: &str| got.iter().find(|(name, _)| name == n).map(|(_, c)| *c);
    let expected = [
        ("help", 0),
        ("version", 0),
        ("unknown", 2),
        ("badflag", 2),
        ("noconfigdir", 2),
        ("reload", 3),
        ("quit", 3),
        ("toggle", 3),
        ("status", 3),
        ("statusbad", 2),
        ("statusjson", 3),
        ("trigger", 3),
        ("badtrigger", 2),
        ("badjson", 2),
        ("ejectnone", 1),
        ("eject", 0),
        ("ejectagain", 1),
        ("ejectusage", 2),
        ("types", 0),
        ("rendernoout", 2),
        ("renderpreset", 1),
        ("render", 0),
    ];
    for (name, code) in expected {
        assert_eq!(get(name), Some(code), "{name}: all codes {got:?}");
    }
    assert!(cfg.join("widgets/clock.luau").is_file(), "eject wrote the widget");
    assert!(cfg.join("types/rsb.d.luau").is_file() && cfg.join(".luaurc").is_file());
    assert!(dir.join("ok.png").is_file());
    assert!(!cfg.join("bar.toml").exists(), "render and the commands never run the first-run setup");
}

#[test]
fn help_names_every_command_and_the_hidden_flags_stay_out_of_it() {
    let (code, out, _) = run(rsb("help").arg("--help"));
    assert_eq!(code, 0);
    for word in
        ["reload", "quit", "toggle", "status", "trigger", "eject", "types", "render", "--config-dir", "--log-dir"]
    {
        assert!(out.contains(word), "{word} missing from the help");
    }
    for hidden in ["--seconds", "--no-reserve", "--stats-out", "--fake-services", "--test-offscreen", "--simulate"] {
        assert!(!out.contains(hidden), "{hidden} must stay hidden");
    }
    let (code, out, _) = run(rsb("help").arg("--version"));
    assert_eq!((code, out.trim()), (0, concat!("rsb ", env!("CARGO_PKG_VERSION"))));
}

#[test]
fn eject_copies_a_builtin_and_its_helpers_and_never_overwrites() {
    let dir = scratch("eject");
    let (code, out, _) = run(rsb("eject").args(["eject", "text", "--config-dir"]).arg(&dir));
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("copied widgets\\text.luau") && out.contains("widgets\\lib\\cmdline.luau"), "{out}");
    write(&dir, "widgets/text.luau", "mine");
    let (code, _, err) = run(rsb("eject").args(["eject", "text", "--config-dir"]).arg(&dir));
    assert_eq!(code, 1);
    assert!(err.contains("already exists"), "{err}");
    assert_eq!(std::fs::read_to_string(dir.join("widgets/text.luau")).unwrap(), "mine");
    let (code, _, err) = run(rsb("eject").args(["eject", "../etc", "--config-dir"]).arg(&dir));
    assert_eq!(code, 1);
    assert!(err.contains("built-in widgets: "), "{err}");
}

#[test]
fn render_draws_the_default_bar_with_sample_values() {
    let dir = scratch("render");
    let png = dir.join("default.png");
    let (code, out, err) = run(rsb("render")
        .args(["render", "--out"])
        .arg(&png)
        .args(["--width", "1200", "--theme", "light", "--log-dir"])
        .arg(dir.join("logs"))
        .arg("--config-dir")
        .arg(dir.join("none")));
    assert_eq!(code, 0, "{out} {err}");
    assert!(out.contains("1200x36 px") && out.contains("0 plates"), "{out}");
    let bytes = std::fs::read(&png).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    assert!(!dir.join("none").exists(), "render never creates the config folder");
}

/// Renders `cfg` and returns stdout and the log text.
fn render_config(tag: &str, files: &[(&str, &str)]) -> (String, String) {
    let dir = scratch(tag);
    let (cfg, logs) = (dir.join("cfg"), dir.join("logs"));
    std::fs::create_dir_all(&cfg).unwrap();
    for (rel, text) in files {
        write(&cfg, rel, text);
    }
    let (code, out, err) = run(rsb(tag)
        .args(["render", "--out"])
        .arg(dir.join("x.png"))
        .args(["--wait-ms", "300", "--config-dir"])
        .arg(&cfg)
        .arg("--log-dir")
        .arg(&logs));
    assert_eq!(code, 0, "{out} {err}");
    (out, std::fs::read_to_string(logs.join("rsb.log")).unwrap_or_default())
}

#[test]
fn a_bad_colour_in_a_custom_property_shows_one_plate_pointing_at_the_users_line() {
    let (out, log) = render_config("badvar", &[("style.css", ":root { --accent: #ggg; }\n")]);
    assert!(out.contains("1 plate)"), "{out}");
    let plate = out.lines().find(|l| l.starts_with("plate: bar:")).unwrap_or_else(|| panic!("{out}"));
    assert!(plate.contains("style.css:1:9 --accent:") && plate.contains("the preset uses it for"), "{plate}");
    assert!(!plate.contains("preset.css"), "the plate names only the user's file: {plate}");
    assert!(!log.contains("preset.css:"), "{log}");
}

#[test]
fn several_problems_say_how_many_more_the_log_holds_and_the_log_holds_them() {
    let (out, log) = render_config(
        "moreplates",
        &[
            ("style.css", "bar { colr: red; backround: blue }\n"),
            (
                "bar.toml",
                "config_version = 1\n[[bar]]\nleft = [\"clock\"]\n[widget.clock]\nformat = 5\nfromat = \"HH\"\n",
            ),
        ],
    );
    let plate = out.lines().find(|l| l.starts_with("plate: bar:")).unwrap_or_else(|| panic!("{out}"));
    assert!(plate.contains("more, see log"), "{plate}");
    // Every problem the plate counts is a line in the log.
    assert!(log.contains("unknown property 'colr'") && log.contains("unknown property 'backround'"), "{log}");
    let format =
        log.lines().find(|l| l.contains("option 'format' of widget 'clock'")).unwrap_or_else(|| panic!("{log}"));
    assert!(format.contains("WARN") && format.contains("bar.toml:5:"), "{format}");
    let typo = log.lines().find(|l| l.contains("unknown option 'fromat'")).unwrap_or_else(|| panic!("{log}"));
    assert!(typo.contains("WARN") && typo.contains("bar.toml:6:"), "{typo}");
}

/// Starts a bar that stops itself after `seconds`, offscreen and without
/// reserving screen space.
fn start_bar(dir: &Path, tag: &str, seconds: u32) -> Child {
    rsb(tag)
        .arg("--config-dir")
        .arg(dir.join("cfg"))
        .arg("--log-dir")
        .arg(dir.join("logs"))
        .arg("--stats-out")
        .arg(dir.join("stats.jsonl"))
        .args(["--no-reserve", "--test-offscreen", "--fake-services", "--seconds", &seconds.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the bar starts")
}

fn wait_for_status(tag: &str) -> String {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        let (code, out, _) = run(rsb(tag).args(["status", "--json"]));
        if code == 0 {
            return out;
        }
        assert!(Instant::now() < until, "the bar never answered (last code {code})");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "shows a window: run through the desktop lock"]
fn a_running_bar_answers_every_command_and_stops_cleanly() {
    let dir = scratch("live");
    let tag = format!("cli-live-{}", std::process::id());
    let mut bar = start_bar(&dir, &tag, 60);
    let status = wait_for_status(&tag);
    let (code, table, _) = run(rsb(&tag).arg("status"));
    assert_eq!(code, 0);
    assert!(table.contains("WIDGET") && table.contains("running for") && !table.contains('{'), "{table}");
    let v: serde_json::Value = serde_json::from_str(status.trim()).expect("status is one line of JSON");
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(v["bars"][0]["name"], "main");
    assert_eq!(v["bars"][0]["render"], "ok");
    assert_eq!(v["widgets"].as_array().map(Vec::len), Some(12), "the default bar has twelve widgets: {status}");
    assert!(v["services"].is_array() && v["memory"]["private_mb"].as_f64().unwrap_or(0.0) > 1.0, "{status}");
    assert!(v["widgets"].as_array().unwrap().iter().all(|w| w["state"] == "running"), "{status}");

    // Every command and its exit code.
    let ask = |args: &[&str]| run(rsb(&tag).args(args));
    assert_eq!(ask(&["trigger", "deploy", "{\"ok\":true}"]).0, 0);
    assert_eq!(ask(&["toggle"]).1.trim(), "hidden");
    assert_eq!(ask(&["toggle"]).1.trim(), "shown");
    assert_eq!(ask(&["toggle", "main"]).1.trim(), "hidden");
    assert_eq!(ask(&["toggle", "main"]).1.trim(), "shown");
    let (code, _, err) = ask(&["toggle", "nope"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("no bar named 'nope'"), "{err}");
    assert_eq!(ask(&["reload"]).0, 0);
    // The same commands as cmd.exe reports them.
    let live = errorlevels(
        &dir,
        &tag,
        &[
            ("status", "status".into()),
            ("reload", "reload".into()),
            ("toggle", "toggle".into()),
            ("untoggle", "toggle".into()),
            ("toggleunknown", "toggle nope".into()),
            ("again", "--config-dir x --log-dir y".into()),
        ],
    );
    let want = [("status", 0), ("reload", 0), ("toggle", 0), ("untoggle", 0), ("toggleunknown", 1), ("again", 0)];
    for (name, code) in want {
        assert_eq!(live.iter().find(|(n, _)| n == name).map(|(_, c)| *c), Some(code), "{name}: {live:?}");
    }
    // A second `rsb` run is told the bar is already running and exits 0.
    let (code, out, _) = run(rsb(&tag).arg("--config-dir").arg(dir.join("cfg")).arg("--log-dir").arg(dir.join("logs")));
    assert_eq!((code, out.trim()), (0, "rsb is already running"));

    // The pipe's DACL, read back from the live pipe: this user only.
    let name = rsb_app::pipe::pipe_name_tagged(Some(&tag)).unwrap();
    let sddl = rsb_app::pipe::read_back_sddl(&name).unwrap();
    assert!(sddl.starts_with("D:P") && sddl.matches("(A;").count() == 1, "{sddl}");
    assert!(sddl.contains(&rsb_app::pipe::current_sid().unwrap()), "{sddl}");

    let (code, out, _) = ask(&["quit"]);
    assert_eq!((code, out.trim()), (0, "stopping"));
    let until = Instant::now() + Duration::from_secs(10);
    let exit = loop {
        if let Some(s) = bar.try_wait().unwrap() {
            break s;
        }
        assert!(Instant::now() < until, "the bar did not stop after quit");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(exit.code(), Some(0));
    assert_eq!(ask(&["status"]).0, 3, "no bar is running any more");
    let log = std::fs::read_to_string(dir.join("logs/rsb.log")).unwrap();
    assert!(log.contains("rsb stopped (code 0)"), "{log}");
    let stats = std::fs::read_to_string(dir.join("stats.jsonl")).unwrap();
    assert!(
        stats.lines().last().is_some_and(|l| l.contains("\"event\":\"exit\"") && l.contains("\"code\":0")),
        "{stats}"
    );
    // A real run has no fake services, so no service_command line; the work area is read at start and at register.
    assert!(stats.contains("\"event\":\"workarea\"") && stats.contains("\"when\":\"start\""), "{stats}");
    assert!(!stats.contains("\"event\":\"service_command\""), "{stats}");
}

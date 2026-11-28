//! The whole settings round trip through the real binary: switch the preset,
//! survive a hand edit, move a widget, set the accent, undo. The same calls run
//! once against a live bar (which must show every change) and once with no bar
//! (the files and a rendered picture must show every change).
//!
//! The live run shows windows far outside the screen, so it goes through the
//! desktop lock:
//!
//! `deskrun.sh cargo test -p rsb --test settings_exit -- --ignored --test-threads=1`

mod support;

use std::time::{Duration, Instant};

use rsb_app::{assets, cfgload, pngread};
use rsb_core::contrast;
use rsb_core::preset::Scheme;
use serde_json::Value;
use support::{Env, Out, comment_lines, config_log, turned_live};

const ACCENT: &str = "#e5a50a";

/// What the sequence has just done, for the caller to check in its own way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Fluent,
    HandEdit,
    Moved,
    Accent,
    Undone,
    AccentAgain,
}

fn ok(out: &Out) -> Value {
    let v = out.json();
    assert!(out.code == 0 && v["ok"] == true, "{out:?}");
    v
}

fn hex_of(color: rsb_core::color::Color) -> String {
    rsb_core::css_text::color_hex(color)
}

/// The accent a preset's own sheet gives a scheme (no colours block).
fn preset_accent(preset: &str, scheme: Scheme) -> String {
    let chain = assets::preset_chain(preset).expect("the preset has a sheet");
    hex_of(contrast::resolve_vars(&chain, scheme)["accent"].0)
}

fn zone(env: &Env, name: &str) -> Vec<String> {
    let got = env.settings(&["get"]).json();
    got["bars"][0][name]["value"]
        .as_array()
        .unwrap_or_else(|| panic!("no zone {name}: {got}"))
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect()
}

/// Runs the calls; `observe` checks what each one did.
fn sequence(env: &Env, live: bool, observe: &mut dyn FnMut(Stage)) {
    let started = Instant::now();
    let mut exempt: Vec<String> = Vec::new();
    // The files as a first start (or the first write) makes them.
    let first_bar_toml = if live {
        env.read("bar.toml")
    } else {
        // No folder yet: the first change writes the template files, so read them after step 1.
        String::new()
    };
    let mut original_bar = first_bar_toml;
    let mut original_css = if live { env.read("style.css") } else { String::new() };

    // 1: get.
    let got = ok(&env.settings(&["get"]));
    assert_eq!(got["bar"]["running"], live, "{got}");
    let h0 = got["files"]["bar.toml"]["hash"].as_str().expect("a hash").to_string();
    if live {
        assert!(h0.starts_with("fnv1a64:"), "{h0}");
    } else {
        assert_eq!(h0, "none", "no folder: no file");
    }

    // 2: the preset.
    let before = if live { env.read("bar.toml") } else { String::new() };
    let r = ok(&env.settings(&["set", "preset", "fluent", "--expect", &format!("bar.toml={h0}")]));
    assert_eq!(r["files"]["bar.toml"]["written"], true, "{r}");
    if !live {
        original_bar = {
            // What the template said before this change: the lines a notice names are numbered in it.
            let t = assets::file("config/bar.toml").unwrap_or("").to_string();
            exempt.extend(turned_live(&r, &t));
            t
        };
        original_css = assets::file("config/style.css").unwrap_or("").to_string();
    } else {
        exempt.extend(turned_live(&r, &before));
    }
    let h1 = r["files"]["bar.toml"]["hash"].as_str().unwrap().to_string();
    if live {
        assert_eq!(r["bar"]["shows_files"], true, "{r}");
        assert!(r["bar"]["ms"].as_u64().is_some_and(|ms| ms <= 2000), "{r}");
    } else {
        assert_eq!(r["bar"]["running"], false, "{r}");
    }
    observe(Stage::Fluent);

    // 3: a hand edit, written the way an editor saves.
    let mut text = env.read("bar.toml");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("\n# my note\n[[bar]]\nheight = 40\n");
    std::fs::write(env.cfg().join("bar.toml"), &text).unwrap();
    let hand_hash = env.hash_of("bar.toml");
    assert_ne!(hand_hash, h1);
    if live {
        wait_for_files(env);
    }
    observe(Stage::HandEdit);

    // 4: a move that expects the old version is refused and leaves the file alone.
    let refused = env.settings(&["move", "clock", "main.right:0", "--expect", &format!("bar.toml={h1}")]);
    let refusal = refused.json();
    assert_eq!((refused.code, refusal["ok"].as_bool()), (1, Some(false)), "{refused:?}");
    assert_eq!(refusal["error"]["code"], "file_changed", "{refusal}");
    assert_eq!(env.hash_of("bar.toml"), hand_hash, "the refused change left the file as it was");

    // 5: the same without --expect keeps the hand edit.
    assert_ne!(zone(env, "right").first().map(String::as_str), Some("clock"), "the move must have something to do");
    let before = env.read("bar.toml");
    let r = ok(&env.settings(&["move", "clock", "main.right:0"]));
    exempt.extend(turned_live(&r, &before));
    let codes: Vec<&str> = r["notices"].as_array().unwrap().iter().filter_map(|n| n["code"].as_str()).collect();
    assert!(codes.contains(&"hand_edit_kept"), "{codes:?}");
    assert_eq!(zone(env, "right").first().map(String::as_str), Some("clock"));
    if live {
        assert_eq!(r["bar"]["shows_files"], true, "{r}");
    }
    observe(Stage::Moved);

    // 6: the accent.
    let before = env.read("style.css");
    let r = ok(&env.settings(&["set", "color.accent", ACCENT]));
    exempt.extend(turned_live(&r, &before));
    assert_eq!(r["accent"]["value"], ACCENT, "{r}");
    if live {
        assert_eq!(r["bar"]["shows_files"], true, "{r}");
    }
    observe(Stage::Accent);

    // 7: undo, then the same accent again.
    let r = ok(&env.settings(&["undo"]));
    if live {
        assert_eq!(r["bar"]["shows_files"], true, "{r}");
    }
    observe(Stage::Undone);
    let r = ok(&env.settings(&["set", "color.accent", ACCENT]));
    if live {
        assert_eq!(r["bar"]["shows_files"], true, "{r}");
    }
    observe(Stage::AccentAgain);

    // What the files and the log say at the end.
    let (bar_toml, style_css) = (env.read("bar.toml"), env.read("style.css"));
    let final_lines: Vec<String> = bar_toml.lines().map(|l| l.trim_end_matches('\r').to_string()).collect();
    for line in comment_lines(&original_bar, false) {
        assert!(
            final_lines.contains(&line) || exempt.contains(&line),
            "a comment line was lost from bar.toml: {line:?}"
        );
    }
    assert!(final_lines.contains(&"# my note".to_string()), "the hand-written note is kept");
    let css_lines: Vec<String> = style_css.lines().map(|l| l.trim_end_matches('\r').to_string()).collect();
    for line in comment_lines(&original_css, true) {
        assert!(
            css_lines.contains(&line) || exempt.contains(&line),
            "a comment line was lost from style.css: {line:?}"
        );
    }
    let got = ok(&env.settings(&["get"]));
    assert_eq!(got["bars"][0]["height"]["value"], 40, "{got}");
    assert_eq!(got["bars"][0]["height"]["source"], "user", "the hand edit is in effect");
    assert_eq!(got["colors"]["accent"]["dark"]["value"], ACCENT, "{got}");
    let (changes, refusals) = config_log(env);
    assert_eq!(changes.len(), 5, "five changes are logged: {changes:?}");
    assert_eq!(refusals.len(), 1, "the refusal is logged once: {refusals:?}");
    eprintln!("settings sequence, {}: {} ms", rsb_app::stats::session_name(), started.elapsed().as_millis());
}

/// Waits until the bar shows the files on disk.
fn wait_for_files(env: &Env) {
    let until = Instant::now() + Duration::from_millis(3000);
    loop {
        let got = env.settings(&["get"]).json();
        if got["bar"]["shows_files"] == true {
            return;
        }
        assert!(Instant::now() < until, "the bar never showed the hand edit: {got}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn inspect_bar(env: &Env) -> Value {
    let out = env.inspect(&["--json"]);
    assert_eq!(out.code, 0, "{out:?}");
    let bar = out.json()["bars"][0].clone();
    // A write that says the bar shows its files has to mean the look is on screen already.
    assert!(bar["frames"].as_u64().is_some_and(|n| n > 0), "the bar has not drawn a frame yet: {bar}");
    bar
}

/// The x and width of the widget's first node, read from the bar.
fn widget_span(env: &Env, name: &str) -> Option<(f64, f64)> {
    let out = env.inspect(&[name, "--json"]);
    assert_eq!(out.code, 0, "{out:?}");
    let rect = out.json()["widgets"][0]["tree"]["rect"].clone();
    Some((rect[0].as_f64()?, rect[2].as_f64()?))
}

/// The x of the widget's first node, read from the bar.
fn widget_x(env: &Env, name: &str) -> Option<f64> {
    widget_span(env, name).map(|(x, _)| x)
}

#[test]
#[ignore = "shows a window: run through the desktop lock"]
fn settings_exit_script() {
    let env = Env::new("settings-exit-live");
    let mut bar = env.start_bar(40);
    env.wait_for_bar();
    // The first start wrote the template files and the bar shows them.
    wait_for_files(&env);
    // Where the clock stands before the move: the move has to change it.
    let mut clock_before: Option<f64> = None;
    sequence(&env, true, &mut |stage| {
        let view = inspect_bar(&env);
        let scheme = if view["scheme"] == "light" { Scheme::Light } else { Scheme::Dark };
        let var = |name: &str| view["root_vars"][name].as_str().map(str::to_string);
        match stage {
            Stage::Fluent => {
                let chain = assets::preset_chain("fluent").unwrap();
                let want = hex_of(contrast::resolve_vars(&chain, scheme)["bg"].0);
                assert_eq!(var("bg"), Some(want), "the bar shows fluent's background");
            }
            Stage::HandEdit => {
                let got = env.settings(&["get"]).json();
                assert_eq!(got["bars"][0]["height"]["value"], 40, "{got}");
                clock_before = Some(widget_x(&env, "clock").expect("the clock is drawn"));
            }
            Stage::Moved => {
                let right = zone(&env, "right");
                let (clock, width) = widget_span(&env, "clock").expect("the clock is drawn");
                let before = clock_before.expect("the clock was read before the move");
                assert!(clock > before, "the clock did not move: x {before} before, {clock} after");
                assert_eq!(right.first().map(String::as_str), Some("clock"));
                // It sits first in the right zone: it ends before the widget that follows it.
                if let Some((next, _)) = right.get(1).and_then(|other| widget_span(&env, other)) {
                    assert!(
                        clock + width <= next + 0.5,
                        "clock x {clock} w {width} runs into the next widget at {next}"
                    );
                }
            }
            Stage::Accent | Stage::AccentAgain => assert_eq!(var("accent").as_deref(), Some(ACCENT)),
            Stage::Undone => {
                assert_eq!(var("accent"), Some(preset_accent("fluent", scheme)), "the preset's accent is back")
            }
        }
    });
    assert_eq!(env.plain(&["quit"]).code, 0);
    assert!(bar.wait().unwrap().success());
}

/// The default preset drawn as shipped, whatever folder the bar reads.
fn shipped_picture(env: &Env) -> pngread::Picture {
    let out_file = env.dir.join("shipped.png");
    let out = env
        .command()
        .args(["render", "--preset", "default", "--out"])
        .arg(&out_file)
        .args(["--width", "1000", "--scale", "1", "--theme", "dark"])
        .output()
        .expect("rsb render runs");
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    pngread::read(&out_file).expect("a PNG")
}

#[test]
fn settings_exit_headless() {
    let env = Env::new("settings-exit-headless");
    let picture = |env: &Env, name: &str| -> pngread::Picture {
        let out_file = env.dir.join(format!("{name}.png"));
        let out = env
            .command()
            .args(["render", "--out"])
            .arg(&out_file)
            .args(["--width", "1000", "--scale", "1", "--theme", "dark", "--config-dir"])
            .arg(env.cfg())
            .arg("--log-dir")
            .arg(env.logs())
            .output()
            .expect("rsb render runs");
        assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
        pngread::read(&out_file).expect("a PNG")
    };
    let has_pixel =
        |p: &pngread::Picture, rgb: [u8; 3]| p.rgba.as_chunks::<4>().0.iter().any(|px| px[..3] == rgb && px[3] == 255);
    let accent = [0xe5, 0xa5, 0x0a];
    // What the template looks like as shipped, drawn before any change.
    let default_picture = shipped_picture(&env);
    sequence(&env, false, &mut |stage| {
        let built = cfgload::build(&env.cfg(), None, "default");
        let config = built.config.as_ref().expect("the files load");
        let current = env.settings(&["get"]).json();
        assert_eq!(
            built.hashes.bar_toml.map(|h| h.to_string()),
            current["files"]["bar.toml"]["hash"].as_str().map(str::to_string)
        );
        match stage {
            Stage::Fluent => {
                assert_eq!(config.preset, "fluent");
                let p = picture(&env, "fluent");
                assert_ne!(p.rgba, default_picture.rgba, "the picture changed with the preset");
            }
            Stage::HandEdit => {
                assert_eq!(config.bars[0].height, 40);
                assert_eq!(picture(&env, "tall").height, 40, "the picture is as tall as the hand edit says");
            }
            Stage::Moved => {
                assert_eq!(config.bars[0].right.first().map(|w| w.widget.name.as_str()), Some("clock"));
            }
            Stage::Accent | Stage::AccentAgain => {
                assert!(has_pixel(&picture(&env, "accent"), accent), "the accent is drawn");
            }
            Stage::Undone => {
                assert!(!has_pixel(&picture(&env, "undone"), accent), "the preset's own accent is back");
            }
        }
    });
}

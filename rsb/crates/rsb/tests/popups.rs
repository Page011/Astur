//! The popup manager through the real binary: test runs with their own
//! widgets, driven by the hidden flags, read back from the stats file and
//! the log. Every run is offscreen, passes `--no-hook` and `--no-activate`
//! and uses the fake services; nothing here sends input to another window,
//! takes keyboard focus, or touches the clipboard. They show windows (far
//! outside the screen), so they run through the desktop lock:
//!
//! `deskrun.sh cargo test -p rsb --test popups -- --ignored --test-threads=1`

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use serde_json::Value;

const EXE: &str = env!("CARGO_BIN_EXE_rsb");

fn scratch(tag: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("popups-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the scratch folder can be made");
    dir
}

/// One file of a run, relative to the run's folder (`cfg/` is the config folder).
fn file(path: &str, text: impl Into<String>) -> (String, String) {
    (path.to_string(), text.into())
}

struct Run {
    lines: Vec<Value>,
    log: String,
    code: i32,
}

/// Runs the bar for `seconds` with these files and flags, in its own folder.
fn run(tag: &str, files: &[(String, String)], flags: &[&str], seconds: u32) -> Run {
    run_with(tag, files, flags, seconds, true)
}

/// `fake`: the fake services (a test run's default). Without them the services are the real ones, and the
/// command recorder (selected by the test flags) keeps every command away from them.
fn run_with(tag: &str, files: &[(String, String)], flags: &[&str], seconds: u32, fake: bool) -> Run {
    run_edited(tag, files, flags, seconds, fake, None)
}

/// `edit`: a file of the run (relative to its folder) that is written again after this many milliseconds,
/// as a person saving it while the bar runs.
fn run_edited(
    tag: &str,
    files: &[(String, String)],
    flags: &[&str],
    seconds: u32,
    fake: bool,
    edit: Option<(u64, &str, String)>,
) -> Run {
    let dir = scratch(tag);
    for (rel, text) in files {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let stats = dir.join("stats.jsonl");
    let here = dir.to_str().unwrap().to_string();
    let editor = edit.map(|(after_ms, rel, text)| {
        let path = dir.join(rel);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(after_ms));
            std::fs::write(path, text).expect("the edit is written");
        })
    });
    let out = Command::new(EXE)
        .env("RSB_TEST_TAG", format!("popups-{tag}-{}", std::process::id()))
        .arg("--config-dir")
        .arg(dir.join("cfg"))
        .arg("--log-dir")
        .arg(dir.join("logs"))
        .arg("--stats-out")
        .arg(&stats)
        .args(["--no-reserve", "--test-offscreen", "--no-hook", "--no-activate", "--seconds"])
        .arg(seconds.to_string())
        .args(fake.then_some("--fake-services"))
        .args(flags.iter().map(|f| f.replace("@dir@", &here)))
        .output()
        .expect("rsb starts");
    if let Some(e) = editor {
        e.join().expect("the editor thread ends");
    }
    let code = out.status.code().unwrap_or(-1);
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&out.stderr));
    let lines = std::fs::read_to_string(&stats)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).expect("a stats line is JSON"))
        .collect();
    let log = std::fs::read_to_string(dir.join("logs").join("rsb.log")).unwrap_or_default();
    Run { lines, log, code }
}

impl Run {
    fn events(&self, name: &str) -> Vec<&Value> {
        self.lines.iter().filter(|l| l["event"] == name).collect()
    }

    /// The `popup` lines of one popup, in order.
    fn popup(&self, name: &str) -> Vec<&Value> {
        self.lines.iter().filter(|l| l["event"] == "popup" && l["name"] == name).collect()
    }

    /// `open`, `shown`, `close:<reason>`, `closed`, `moved`, `refused:<reason>` in order.
    fn seq(&self, name: &str) -> Vec<String> {
        self.popup(name)
            .iter()
            .map(|l| {
                let state = l["state"].as_str().unwrap_or("?");
                match l["reason"].as_str() {
                    Some(r) => format!("{state}:{r}"),
                    None => state.to_string(),
                }
            })
            .collect()
    }

    fn shown(&self, name: &str) -> Vec<&Value> {
        self.popup(name).into_iter().filter(|l| l["state"] == "shown").collect()
    }

    fn rect_of(&self, name: &str) -> [i64; 4] {
        let l = self.shown(name).first().copied().unwrap_or_else(|| panic!("{name} was never shown: {:?}", self.lines));
        rect(l)
    }

    fn log_count(&self, needle: &str) -> usize {
        self.log.lines().filter(|l| l.contains(needle)).count()
    }

    fn t_ms(l: &Value) -> u64 {
        l["t_ms"].as_u64().unwrap()
    }

    /// The monitor's work area from the first `workarea` line.
    fn work(&self) -> [i64; 4] {
        let w = self.events("workarea")[0]["rect"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap())
            .collect::<Vec<_>>();
        [w[0], w[1], w[2], w[3]]
    }

    /// `hook` lines' `wanted` flags in order: when the outside-press watch was turned on and off.
    fn watch(&self) -> Vec<bool> {
        self.events("hook").iter().filter(|l| l["final"].is_null()).map(|l| l["wanted"].as_bool().unwrap()).collect()
    }
}

fn rect(l: &Value) -> [i64; 4] {
    let r = l["rect"].as_array().unwrap();
    [r[0].as_i64().unwrap(), r[1].as_i64().unwrap(), r[2].as_i64().unwrap(), r[3].as_i64().unwrap()]
}

/// A widget with one popup: an anchor row with `label`, and the popup `name`.
/// `anchor` is extra style for the anchor row, `props` extra popup props,
/// `content` the children Luau of the popup.
fn widget(name: &str, label: &str, anchor: &str, props: &str, content: &str) -> String {
    format!(
        r##"local rsb = require("@rsb")
return rsb.widget {{
	permissions = {{}},
	setup = function(props, ctx)
		local open = rsb.state(false)
		return rsb.row {{
			style = {{ padding = "0 12px", {anchor} }},
			on_click = function()
				open.value = not open.value
			end,
			rsb.text {{ text = "{label}" }},
			rsb.popup {{
				name = "{name}",
				open = open,
				{props}
				on_close = function(reason)
					rsb.log.info("closed " .. reason)
				end,
				function()
					return {{ {content} }}
				end,
			}},
		}}
	end,
}}
"##
    )
}

fn panel(name: &str, label: &str) -> String {
    widget(
        name,
        label,
        "",
        "style = { width = 200, height = 120, background_color = \"#223344\" },",
        "rsb.text { text = \"hello\" }",
    )
}

fn input_panel(name: &str, label: &str, props: &str) -> String {
    widget(
        name,
        label,
        "",
        &format!("style = {{ width = 220, height = 60, background_color = \"#334455\" }}, {props}"),
        "rsb.input { placeholder = \"type\", on_change = function(t) rsb.log.info(\"changed\") end, on_submit = function(t) rsb.log.info(\"submitted\") end }",
    )
}

fn bar(left: &[&str], center: &[&str], right: &[&str]) -> (String, String) {
    let list = |v: &[&str]| format!("[{}]", v.iter().map(|w| format!("\"{w}\"")).collect::<Vec<_>>().join(", "));
    file(
        "cfg/bar.toml",
        format!(
            "config_version = 1\n[[bar]]\nname = \"main\"\nleft = {}\ncenter = {}\nright = {}\n",
            list(left),
            list(center),
            list(right)
        ),
    )
}

fn w(name: &str, text: String) -> (String, String) {
    file(&format!("cfg/widgets/{name}.luau"), text)
}

/// The session's scale (dpi / 96), read once from a one-second run's geometry line: the sizes and the
/// pointer positions below are written in logical px and come out right at any dpi.
fn scale() -> f64 {
    static SCALE: OnceLock<f64> = OnceLock::new();
    *SCALE.get_or_init(|| {
        let r = run("scale", &[bar(&[], &[], &[])], &[], 1);
        let dpi = r.events("geometry").first().and_then(|l| l["dpi"].as_f64()).expect("the run reported its geometry");
        dpi / 96.0
    })
}

/// Logical px as device px at the session's scale.
fn px(v: f64) -> i64 {
    (v * scale()).round() as i64
}

/// A dpi that is not the session's, for the flags that change it.
fn other_dpi() -> u32 {
    if (scale() - 1.5).abs() < 0.01 { 192 } else { 144 }
}

/// The y of a click inside the bar's widget row, device px.
fn bar_y() -> i64 {
    px(18.0)
}

// ---------------------------------------------------------------- placement

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn popups_flip_and_clamp_at_all_four_edges_of_the_work_area() {
    // Top bar: one popup at the left edge, one at the right (both wider than their anchor), one that asks
    // for "above" where nothing fits. Bottom bar: one that asks for "below" where nothing fits.
    let wide = |name: &str, label: &str, placement: &str| {
        widget(
            name,
            label,
            "",
            &format!(
                "placement = \"{placement}\", style = {{ width = 300, height = 100, background_color = \"#223344\" }},"
            ),
            "rsb.text { text = \"x\" }",
        )
    };
    let toml = "config_version = 1\n[[bar]]\nname = \"top\"\nposition = \"top\"\nleft = [\"tl\", \"ta\"]\nright = [\"tr\"]\n\
                [[bar]]\nname = \"bottom\"\nposition = \"bottom\"\nleft = [\"bb\"]\n";
    let files = vec![
        file("cfg/bar.toml", toml),
        w("tl", wide("pl", "L", "below")),
        w("ta", wide("pa", "A", "above")),
        w("tr", wide("pr", "R", "below")),
        w("bb", wide("pb", "B", "below")),
    ];
    let r = run("edges", &files, &["--simulate-popup", "700:pl:open,1300:pr:open,1900:pa:open,2500:pb:open"], 4);
    let work = r.work();
    let (l, rr, a, b) = (r.rect_of("pl"), r.rect_of("pr"), r.rect_of("pa"), r.rect_of("pb"));
    // A popup that has to be moved stops 8 layout px short of the edge (12 px at 150 %).
    let scale = r.shown("pl")[0]["scale"].as_f64().unwrap().max(1.0);
    let margin = (8.0 * scale).round() as i64;
    assert_eq!(l[0], work[0] + margin, "the left edge clamps to the work area, a margin short of it: {l:?}");
    assert_eq!(rr[2], work[2] - margin, "the right edge clamps to the work area, a margin short of it: {rr:?}");
    let side = |n: &str| r.shown(n)[0]["side"].as_str().unwrap().to_string();
    assert_eq!((side("pl"), side("pr")), ("below".into(), "below".into()));
    assert_eq!(side("pa"), "below", "no room above a top bar: flipped, {a:?}");
    assert!(a[1] >= px(16.0), "and below the bar: {a:?}");
    assert_eq!(side("pb"), "above", "no room below a bottom bar: flipped, {b:?}");
    assert!(b[3] <= work[3] - px(16.0), "and above the bar: {b:?}");
    for rc in [l, rr, a, b] {
        assert!(
            rc[0] >= work[0] && rc[1] >= work[1] && rc[2] <= work[2] && rc[3] <= work[3],
            "inside the work area: {rc:?}"
        );
        assert_eq!((rc[2] - rc[0], rc[3] - rc[1]), (px(300.0), px(100.0)));
    }
}

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn an_open_popup_follows_its_anchor_when_the_anchor_moves() {
    let widget_src = r##"local rsb = require("@rsb")
return rsb.widget {
	permissions = {},
	setup = function(props, ctx)
		local open = rsb.state(false)
		local pad = rsb.state(0)
		rsb.timer.after("1500ms", function() pad.value = 100 end)
		return rsb.row {
			style = function() return { padding = "0 12px", margin_left = pad.value } end,
			rsb.text { text = "mover" },
			rsb.popup {
				name = "mv",
				open = open,
				align = "start",
				style = { width = 150, height = 80, background_color = "#223344" },
				function() return { rsb.text { text = "x" } } end,
			},
		}
	end,
}
"##;
    let files = vec![bar(&["mv"], &[], &[]), w("mv", widget_src.to_string())];
    let r = run("follow", &files, &["--simulate-popup", "600:mv:open"], 3);
    let first = r.rect_of("mv");
    let moved: Vec<[i64; 4]> = r.popup("mv").into_iter().filter(|l| l["state"] == "moved").map(rect).collect();
    assert!(!moved.is_empty(), "the popup was placed again: {:?}", r.seq("mv"));
    let last = *moved.last().unwrap();
    assert_eq!(last[0] - first[0], px(100.0), "it moved with its anchor: {first:?} -> {last:?}");
    assert_eq!((last[2] - last[0], last[3] - last[1]), (first[2] - first[0], first[3] - first[1]), "same size");
}

// ---------------------------------------------------------------- dismissal

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn an_outside_press_closes_popups_without_focus_on_shadow_pixels_and_on_the_dim() {
    let shadowed = widget(
        "sh",
        "S",
        "",
        "style = { width = 200, height = 100, background_color = \"#223344\", box_shadow = \"0 4px 12px rgba(0,0,0,0.5)\" },",
        "rsb.text { text = \"x\" }",
    );
    let overlay = r##"local rsb = require("@rsb")
return rsb.widget {
	permissions = { overlay = true },
	setup = function(props, ctx)
		local open = rsb.state(false)
		return rsb.row {
			style = { padding = "0 12px" },
			rsb.text { text = "O" },
			rsb.popup {
				name = "ov",
				open = open,
				placement = "screen",
				style = { width = "100%", height = "100%", background_color = "rgba(0,0,0,0.5)", justify_content = "center", align_items = "center" },
				on_close = function(reason) rsb.log.info("overlay closed " .. reason) end,
				function()
					return { rsb.box { focusable = true, autofocus = true, style = { width = 80, height = 40, background_color = "#445566" }, rsb.text { text = "Lock" } } }
				end,
			},
		}
	end,
}
"##;
    let files = vec![bar(&["sh", "ov"], &[], &[]), w("sh", shadowed), w("ov", overlay.to_string())];
    let pointer = "2400 popup:sh down 2 2 left\n2450 popup:sh up 2 2 left\n3100 dim:ov down 5 5 left\n3150 dim:ov up 5 5 left\n3800 dim:ov down 5 5 left\n3850 dim:ov up 5 5 left\n";
    let r = run(
        "outside",
        &[files, vec![file("pointer.txt", pointer)]].concat(),
        &[
            "--simulate-popup",
            "600:sh:open,1800:sh:open,2800:ov:open",
            "--simulate-outside-press",
            "1200",
            "--simulate-pointer",
            "@dir@/pointer.txt",
        ],
        5,
    );
    let sh = r.seq("sh");
    assert_eq!(
        sh,
        ["open", "shown", "close:outside", "closed", "open", "shown", "close:outside", "closed"],
        "a press outside every window, then a press on the popup's own shadow pixels: {:?}",
        r.popup("sh")
    );
    // The shadow case: the window is larger than the popup, so the press at (2, 2) hit the window, not the popup.
    let rc = r.rect_of("sh");
    assert!(rc[2] - rc[0] > 200 && rc[3] - rc[1] > 100, "the window includes the shadow: {rc:?}");
    // The overlay: a press on its dim inside the 400 ms guard is ignored, one after it closes it.
    let ov = r.seq("ov");
    assert_eq!(ov, ["open", "shown", "close:outside", "closed"], "{:?}", r.popup("ov"));
    let closed_at = Run::t_ms(r.popup("ov").iter().find(|l| l["state"] == "close").unwrap());
    let opened_at = Run::t_ms(r.popup("ov")[0]);
    assert!(
        closed_at - opened_at > 900,
        "the early press (300 ms after opening) was ignored: {opened_at} -> {closed_at}"
    );
    assert_eq!(r.log_count("overlay closed outside"), 1);
    // The watch is on exactly while a popup without focus is open: twice for the panel, never for the overlay.
    assert_eq!(r.watch(), [true, false, true, false], "{:?}", r.events("hook"));
}

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn escape_closes_the_focused_popup_unless_dismiss_escape_is_false() {
    let files = vec![
        bar(&["pi", "pq"], &[], &[]),
        w("pi", input_panel("ia", "I", "")),
        w("pq", input_panel("ib", "Q", "dismiss = { escape = false },")),
    ];
    let keys = "1200 popup:ia key Escape\n2400 popup:ib key Escape\n";
    let r = run(
        "escape",
        &[files, vec![file("keys.txt", keys)]].concat(),
        &["--simulate-popup", "700:ia:open,1800:ib:open,3000:ib:close", "--simulate-key", "@dir@/keys.txt"],
        4,
    );
    assert_eq!(r.seq("ia"), ["open", "shown", "close:escape", "closed"]);
    assert_eq!(
        r.seq("ib"),
        ["open", "shown", "close:command", "closed"],
        "Escape did not close it: {:?}",
        r.popup("ib")
    );
    assert_eq!(r.log_count("closed escape"), 1, "the script heard the close with its reason");
}

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn a_popup_closes_when_its_anchor_goes_away() {
    let src = r##"local rsb = require("@rsb")
return rsb.widget {
	permissions = {},
	setup = function(props, ctx)
		local open = rsb.state(false)
		local shown = rsb.state(true)
		rsb.timer.after("1500ms", function() shown.value = false end)
		return rsb.row {
			visible = shown,
			style = { padding = "0 12px" },
			rsb.text { text = "gone" },
			rsb.popup {
				name = "ag",
				open = open,
				style = { width = 150, height = 80, background_color = "#223344" },
				on_close = function(reason) rsb.log.info("closed " .. reason) end,
				function() return { rsb.text { text = "x" } } end,
			},
		}
	end,
}
"##;
    let r = run("anchor", &[bar(&["ag"], &[], &[]), w("ag", src.to_string())], &["--simulate-popup", "600:ag:open"], 3);
    assert_eq!(r.seq("ag"), ["open", "shown", "close:anchor_gone", "closed"], "{:?}", r.popup("ag"));
    assert_eq!(r.log_count("closed anchor_gone"), 1);
}

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn the_press_that_closed_a_popup_cannot_reopen_it_but_a_press_on_the_anchor_toggles_it() {
    // The widget's anchor is a 100 px row; a second 60 px "button" next to it toggles the same popup.
    let src = r##"local rsb = require("@rsb")
return rsb.widget {
	permissions = {},
	setup = function(props, ctx)
		local open = rsb.state(false)
		return rsb.row {
			rsb.row {
				style = { width = 100, height = 24 },
				on_click = function() open.value = not open.value end,
				rsb.text { text = "anchor" },
				rsb.popup {
					name = "tg",
					open = open,
					style = { width = 150, height = 80, background_color = "#223344" },
					on_close = function(reason) rsb.log.info("closed " .. reason) end,
					function() return { rsb.text { text = "x" } } end,
				},
			},
			rsb.row {
				style = { width = 60, height = 24 },
				on_click = function() rsb.popups.toggle("tg") end,
				rsb.text { text = "btn" },
			},
		}
	end,
}
"##;
    // Bar-relative px: the anchor covers x 0..100, the button x 100..160.
    let (btn, anchor, y) = (px(130.0), px(40.0), bar_y());
    let pointer = format!(
        "1500 main down {btn} {y} left\n1560 main up {btn} {y} left\n\
         2200 main down {anchor} {y} left\n2260 main up {anchor} {y} left\n\
         2800 main down {anchor} {y} left\n2860 main up {anchor} {y} left\n"
    );
    let files = vec![bar(&["tg"], &[], &[]), w("tg", src.to_string()), file("pointer.txt", pointer)];
    let r = run("toggle", &files, &["--simulate-popup", "700:tg:open", "--simulate-pointer", "@dir@/pointer.txt"], 4);
    assert_eq!(
        r.seq("tg"),
        ["open", "shown", "close:outside", "closed", "close:outside", "open", "shown", "close:command", "closed"],
        "the button's press closed it, and the same press's click did not reopen it; \
         the anchor then opened it and closed it as a script close, never as Outside: {:?}",
        r.popup("tg")
    );
    assert_eq!(r.shown("tg").len(), 2);
}

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn a_menu_inside_a_panel_leaves_the_panel_open_and_another_panel_replaces_both() {
    let host = r##"local rsb = require("@rsb")
return rsb.widget {
	permissions = {},
	setup = function(props, ctx)
		local open = rsb.state(false)
		local inner = rsb.state(false)
		return rsb.row {
			style = { padding = "0 12px" },
			rsb.text { text = "host" },
			rsb.popup {
				name = "hp",
				open = open,
				style = { width = 220, height = 120, background_color = "#223344" },
				on_close = function(reason) rsb.log.info("host closed " .. reason) end,
				function()
					return {
						rsb.row {
							style = { width = 100, height = 30 },
							rsb.text { text = "item" },
							rsb.menu { name = "hm", open = inner, items = { { label = "One" }, { label = "Two" } } },
						},
					}
				end,
			},
		}
	end,
}
"##;
    let files = vec![bar(&["host", "other"], &[], &[]), w("host", host.to_string()), w("other", panel("op", "O"))];
    let r = run("chain", &files, &["--simulate-popup", "600:hp:open,1200:hm:open,1900:op:open"], 3);
    assert_eq!(r.seq("hp"), ["open", "shown", "close:replaced", "closed"], "{:?}", r.popup("hp"));
    let hm = r.seq("hm");
    assert_eq!(hm[..2], ["open", "shown"], "the menu opened with the panel still open: {hm:?}");
    assert!(hm.contains(&"close:replaced".to_string()), "and went with the panel: {hm:?}");
    let t = |n: &str, s: &str| Run::t_ms(r.popup(n).iter().find(|l| l["state"] == s).unwrap());
    assert!(t("hm", "open") > t("hp", "shown"), "opened inside the panel");
    assert!(t("hp", "close") > t("hm", "shown") + 300, "the panel stayed open when its menu opened");
    assert_eq!(r.seq("op")[..2], ["open", "shown"]);
}

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn a_menu_the_panel_stops_declaring_closes_at_once() {
    // The panel's only menu is left out of its content at about 2500 ms; the menu has nothing to hang on any more.
    let host = r##"local rsb = require("@rsb")
return rsb.widget {
	permissions = {},
	setup = function(props, ctx)
		local open = rsb.state(false)
		local inner = rsb.state(false)
		local has = rsb.state(true)
		rsb.timer.after("2500ms", function() has.value = false end)
		return rsb.row {
			style = { padding = "0 12px" },
			rsb.text { text = "host" },
			rsb.popup {
				name = "hp",
				open = open,
				style = { width = 220, height = 120, background_color = "#223344" },
				function()
					local out = { rsb.text { text = "panel" } }
					if has.value then
						table.insert(out, rsb.row {
							style = { width = 100, height = 30 },
							rsb.text { text = "item" },
							rsb.menu { name = "hm", open = inner, items = { { label = "One" }, { label = "Two" } } },
						})
					end
					return out
				end,
			},
		}
	end,
}
"##;
    let files = vec![bar(&["host"], &[], &[]), w("host", host.to_string())];
    let r = run("menudrop", &files, &["--simulate-popup", "600:hp:open,1200:hm:open", "--stats-frames"], 5);
    let hm = r.seq("hm");
    assert_eq!(hm[..2], ["open", "shown"], "{hm:?}");
    assert!(hm.contains(&"close:anchor_gone".to_string()), "{hm:?}");
    // The panel draws again when its content changes: that frame is the moment the menu was taken out.
    let changed = r
        .lines
        .iter()
        .filter(|l| l["event"] == "frame" && l["surface"] == "popup:hp")
        .map(Run::t_ms)
        .find(|&t| t > 2000)
        .expect("the panel drew again without its menu");
    let closed = Run::t_ms(r.popup("hm").iter().find(|l| l["state"] == "close").unwrap());
    assert!(closed >= changed, "the menu closed at {closed} ms, before the panel changed at {changed} ms");
    assert!(closed - changed <= 300, "the menu closed {} ms after the panel changed", closed - changed);
    // Its window goes once the closing transition is over, not when the closing cap runs out.
    let gone = Run::t_ms(r.popup("hm").iter().find(|l| l["state"] == "closed").expect("the menu window was destroyed"));
    assert!(gone - closed <= 300, "the menu window stayed {} ms after it closed", gone - closed);
}

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn bar_hidden_fullscreen_display_dpi_and_session_changes_close_popups() {
    // A dpi equal to the session's is no change: take another one.
    let dpi = other_dpi();
    let files = vec![bar(&["pa"], &[], &[]), w("pa", panel("al", "A"))];
    let r = run(
        "changes",
        &files,
        &[
            "--simulate-popup",
            "500:al:open,1700:al:open,2900:al:open,4100:al:open,5300:al:open",
            "--simulate-fullscreen",
            "1100:on",
            "--simulate-display-change",
            "2300",
            "--simulate-popup-dpi",
            &format!("3500:al:{dpi}"),
            "--simulate-dpi",
            &format!("4700:{dpi}"),
            "--simulate-session",
            "5900:lock",
        ],
        7,
    );
    let reasons: Vec<String> = r
        .popup("al")
        .iter()
        .filter(|l| l["state"] == "close")
        .map(|l| l["reason"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        reasons,
        ["bar_hidden", "display_change", "display_change", "display_change", "deactivated"],
        "fullscreen notice, display change, popup DPI change, bar DPI change, session lock: {:?}",
        r.seq("al")
    );
    assert_eq!(r.shown("al").len(), 5);
}

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn a_config_change_that_makes_a_new_bar_window_closes_the_popup_and_tells_the_widget() {
    let bar_toml =
        |height: u32| format!("config_version = 1\n[[bar]]\nname = \"main\"\nheight = {height}\nleft = [\"pa\"]\n");
    let files = vec![file("cfg/bar.toml", bar_toml(36)), w("pa", panel("al", "A"))];
    let r = run_edited(
        "recreate",
        &files,
        &["--simulate-popup", "600:al:open"],
        5,
        true,
        Some((1500, "cfg/bar.toml", bar_toml(44))),
    );
    assert_eq!(
        r.seq("al"),
        ["open", "shown", "close:display_change", "closed"],
        "the bar window was made again: the popup closes once and does not come back by itself: {:?}",
        r.popup("al")
    );
    assert_eq!(r.log_count("closed display_change"), 1, "the widget was told why: {}", r.log);
}

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn a_fullscreen_notice_does_not_close_popups_of_a_bar_in_stay_mode() {
    let toml = "config_version = 1\n[[bar]]\nname = \"main\"\nfullscreen = \"stay\"\nleft = [\"pa\"]\n";
    let files = vec![file("cfg/bar.toml", toml), w("pa", panel("al", "A"))];
    let r =
        run("stay", &files, &["--simulate-popup", "500:al:open,2000:al:close", "--simulate-fullscreen", "1100:on"], 3);
    assert_eq!(r.seq("al"), ["open", "shown", "close:command", "closed"], "{:?}", r.popup("al"));
}

// ---------------------------------------------------------------- stages

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn a_popup_opens_from_its_closed_style_closes_with_its_last_content_and_can_reopen_while_closing() {
    let css = "popup.fade { opacity: 0; transition: opacity 200ms linear; }\npopup.fade:open { opacity: 1; }\n";
    let files = vec![
        bar(&["pa"], &[], &[]),
        w(
            "pa",
            widget(
                "fd",
                "F",
                "",
                "class = \"fade\", style = { width = 200, height = 100, background_color = \"#223344\" },",
                "rsb.text { text = \"content\" }",
            ),
        ),
        file("cfg/style.css", css),
    ];
    let r = run(
        "stages",
        &files,
        &["--simulate-popup", "600:fd:open,1500:fd:close,1600:fd:open,2400:fd:close", "--stats-frames"],
        4,
    );
    let seq = r.seq("fd");
    assert_eq!(
        seq,
        ["open", "shown", "close:command", "open", "close:command", "closed"],
        "one window: closed, reopened while closing, closed again: {:?}",
        r.popup("fd")
    );
    let closed = r.popup("fd").into_iter().find(|l| l["state"] == "closed").unwrap();
    let ms = closed["closing_ms"].as_u64().unwrap();
    assert!((150..=600).contains(&ms), "the closing transition (200 ms) ran before the window went: {ms} ms");
    assert!(r.popup("fd").iter().any(|l| l["reopened"] == true));
    // The windows of a closing popup keep drawing: popup frames exist after the first close.
    let first_close = Run::t_ms(r.popup("fd").iter().find(|l| l["state"] == "close").unwrap());
    let frames_after =
        r.events("frame").iter().filter(|l| l["surface"] == "popup:fd" && Run::t_ms(l) > first_close).count();
    assert!(frames_after >= 2, "frames kept coming while it closed and reopened: {frames_after}");
}

// ---------------------------------------------------------------- focus, keys, text

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn focus_goes_only_to_a_popup_opened_by_a_gesture_and_a_press_inside_gives_it_to_the_rest() {
    // `ta` opens a popup with an input from a timer (no gesture); `tb` from the command (a gesture).
    let timed = r##"local rsb = require("@rsb")
return rsb.widget {
	permissions = {},
	setup = function(props, ctx)
		local open = rsb.state(false)
		rsb.timer.after("700ms", function() open.value = true end)
		return rsb.row {
			style = { padding = "0 12px" },
			rsb.text { text = "T" },
			rsb.popup {
				name = "tm",
				open = open,
				style = { width = 200, height = 60, background_color = "#334455" },
				function() return { rsb.input { on_change = function(t) rsb.log.info("tm changed") end } } end,
			},
		}
	end,
}
"##;
    let files = vec![bar(&["ta", "tb"], &[], &[]), w("ta", timed.to_string()), w("tb", input_panel("tg", "G", ""))];
    // Keys to the timer-opened popup are dropped; a press inside it gives it focus; then keys land.
    let keys = "1000 popup:tm text a\n1700 popup:tm key Tab\n2000 popup:tm text b\n";
    let pointer = "1500 popup:tm down 20 20 left\n1550 popup:tm up 20 20 left\n";
    let r = run(
        "focus",
        &[files, vec![file("keys.txt", keys), file("pointer.txt", pointer)]].concat(),
        &[
            "--simulate-popup",
            "2400:tg:open",
            "--simulate-key",
            "@dir@/keys.txt",
            "--simulate-pointer",
            "@dir@/pointer.txt",
        ],
        4,
    );
    let focus: Vec<&Value> = r.events("focus");
    assert_eq!(focus.len(), 3, "{focus:?}");
    assert_eq!(
        (focus[0]["popup"].as_str(), focus[0]["got"].as_bool()),
        (Some("tm"), Some(false)),
        "a timer cannot give focus"
    );
    assert!(focus[0]["gesture_age_ms"].is_null(), "no gesture at all");
    assert_eq!(
        (focus[1]["popup"].as_str(), focus[1]["got"].as_bool()),
        (Some("tm"), Some(true)),
        "a press inside takes it"
    );
    assert_eq!(focus[1]["activated"], false, "simulated under --no-activate");
    assert_eq!(
        (focus[2]["popup"].as_str(), focus[2]["got"].as_bool()),
        (Some("tg"), Some(true)),
        "a gesture opened it"
    );
    assert!(focus[2]["gesture_age_ms"].as_u64().unwrap() < 1000);
    let dropped = r.events("input_dropped").len();
    assert_eq!(dropped, 1, "the first text reached no popup that held focus");
    assert_eq!(r.log_count("tm changed"), 1, "the second text did");
}

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn a_gesture_older_than_a_second_gives_no_focus() {
    // The UI thread sleeps right after the command, so the popup is made 1.4 s after the gesture. The
    // two inbox messages can in principle be separated by the script's reply; the run is repeated once then.
    let files = vec![bar(&["tg"], &[], &[]), w("tg", input_panel("tg", "G", ""))];
    let mut verdict = None;
    for attempt in 0..3 {
        let r = run(
            &format!("stale{attempt}"),
            &files,
            &["--simulate-popup", "600:tg:open", "--test-ui-stall", "600:1400"],
            3,
        );
        let focus = r.events("focus");
        assert_eq!(focus.len(), 1, "{focus:?}");
        if focus[0]["got"] == false {
            assert!(focus[0]["gesture_age_ms"].as_u64().unwrap() > 1000, "{:?}", focus[0]);
            verdict = Some(());
            break;
        }
    }
    assert!(verdict.is_some(), "a stale gesture gave focus in 3 attempts");
}

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn menu_keys_move_between_levels_pick_an_item_and_close_the_whole_chain() {
    let menu = r##"local rsb = require("@rsb")
return rsb.widget {
	permissions = {},
	setup = function(props, ctx)
		local open = rsb.state(false)
		return rsb.row {
			style = { padding = "0 12px" },
			rsb.text { text = "Menu" },
			rsb.menu {
				name = "mn",
				open = open,
				on_close = function(reason) rsb.log.info("menu closed " .. reason) end,
				items = {
					{ label = "One", on_click = function() rsb.log.info("one picked") end },
					{ label = "More", items = { { label = "Deep", on_click = function() rsb.log.info("deep picked") end } } },
				},
			},
		}
	end,
}
"##;
    // Down to "More", Right opens the submenu (focus moves to it), Left closes that level and the parent
    // has focus again (the next key reaches it), Right again, Enter picks.
    let keys = "400 popup:mn key ArrowDown\n600 popup:mn key ArrowRight\n900 popup:mn-1 key ArrowLeft\n1100 popup:mn key ArrowRight\n1400 popup:mn-1 key Enter\n";
    // Without the preset's shadow and padding the windows are the menus' own boxes and the items reach their
    // edges, so a level placed against an item touches its parent's window.
    let files = vec![
        bar(&["mn"], &[], &[]),
        w("mn", menu.to_string()),
        file("cfg/style.css", "menu { box-shadow: none; padding: 0 }".to_string()),
        file("keys.txt", keys),
    ];
    let r = run("menu", &files, &["--simulate-popup", "100:mn:open", "--simulate-key", "@dir@/keys.txt"], 3);
    assert_eq!(r.events("input_dropped").len(), 0, "focus went back to the parent when the submenu closed");
    let sub = r.seq("mn-1");
    assert_eq!(
        sub,
        ["open", "shown", "close:escape", "closed", "open", "shown", "close:picked", "closed"],
        "{:?}",
        r.popup("mn-1")
    );
    assert_eq!(r.seq("mn"), ["open", "shown", "close:picked", "closed"], "the pick closed the root too");
    assert_eq!(r.log_count("deep picked"), 1);
    assert_eq!(r.log_count("menu closed picked"), 1, "the script heard of it once");
    let focus: Vec<&Value> = r.events("focus");
    assert!(focus.iter().all(|f| f["got"] == true));
    // The submenu opened to the right of its item (the manager placed it against the item, in the panel's surface).
    let (parent, child) = (r.rect_of("mn"), r.rect_of("mn-1"));
    assert_eq!(r.shown("mn-1")[0]["side"], "right");
    assert_eq!(child[0], parent[2], "touching the right edge of the first menu");
}

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn a_submenu_in_the_default_look_starts_where_its_item_ends_inside_the_parents_shadow_and_padding() {
    let menu = r##"local rsb = require("@rsb")
return rsb.widget {
	permissions = {},
	setup = function(props, ctx)
		local open = rsb.state(false)
		return rsb.row {
			style = { padding = "0 12px" },
			rsb.text { text = "Menu" },
			rsb.menu {
				name = "mn",
				open = open,
				items = {
					{ label = "One", on_click = function() end },
					{ label = "More", items = { { label = "Deep", on_click = function() end } } },
				},
			},
		}
	end,
}
"##;
    // No style.css: the menu has the preset's shadow and 6 px of padding, so its window is larger than its box.
    let keys = "400 popup:mn key ArrowDown\n600 popup:mn key ArrowRight\n";
    let files = vec![bar(&["mn"], &[], &[]), w("mn", menu.to_string()), file("keys.txt", keys)];
    let r = run("menu-look", &files, &["--simulate-popup", "100:mn:open", "--simulate-key", "@dir@/keys.txt"], 2);
    let boxed = |name: &str| -> [i64; 4] {
        let l = r.shown(name).first().copied().unwrap_or_else(|| panic!("{name} was never shown: {:?}", r.lines));
        let b = l["box"].as_array().unwrap_or_else(|| panic!("{name} reported no box: {l}"));
        [b[0].as_i64().unwrap(), b[1].as_i64().unwrap(), b[2].as_i64().unwrap(), b[3].as_i64().unwrap()]
    };
    let (parent_window, parent) = (r.rect_of("mn"), boxed("mn"));
    let child = boxed("mn-1");
    assert!(parent_window[0] < parent[0] && parent[2] < parent_window[2], "the parent's window holds its shadow");
    assert_eq!(r.shown("mn-1")[0]["side"], "right");
    // The item ends at the parent's padding edge.
    assert_eq!(
        child[0],
        parent[2] - px(6.0),
        "the child's box starts where the item ends: {parent:?} in {parent_window:?} -> {child:?} in {:?}",
        r.rect_of("mn-1")
    );
}

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn ime_stages_the_clipboard_and_the_password_rule_go_through_the_pipeline() {
    let pw = widget(
        "pw",
        "P",
        "",
        "style = { width = 220, height = 60, background_color = \"#334455\" },",
        "rsb.input { password = true }",
    );
    let files = vec![bar(&["pi", "pp"], &[], &[]), w("pi", input_panel("ip", "I", "")), w("pp", pw)];
    // Compose and commit, paste, select all and copy; then a password popup.
    let keys = "700 popup:ip ime start\n750 popup:ip ime update \\u3042 1\n800 popup:ip ime commit \\u3042\n850 popup:ip ime end\n\
                1000 popup:ip key v down ctrl\n1200 popup:ip key a down ctrl\n1300 popup:ip key c down ctrl\n";
    let r = run(
        "ime",
        &[files, vec![file("keys.txt", keys)]].concat(),
        &["--simulate-popup", "500:ip:open,1900:pw:open", "--simulate-key", "@dir@/keys.txt"],
        3,
    );
    assert!(r.events("focus").iter().any(|f| f["popup"] == "pw" && f["got"] == true), "the password popup got focus");
    let ime: Vec<&str> = r.events("ime").iter().map(|l| l["stage"].as_str().unwrap()).collect();
    let stages: Vec<&&str> = ime.iter().filter(|s| **s != "caret").collect();
    assert_eq!(stages, [&"enabled", &"start", &"update", &"commit", &"end"], "{ime:?}");
    assert_eq!(
        ime.iter().filter(|s| **s == "enabled").count(),
        1,
        "the password popup never turned the input method on"
    );
    let clip = r.events("clipboard");
    assert_eq!(clip.len(), 2, "{clip:?}");
    assert_eq!((clip[0]["op"].as_str(), clip[0]["ok"].as_bool()), (Some("read"), Some(true)));
    assert_eq!((clip[1]["op"].as_str(), clip[1]["ok"].as_bool()), (Some("write"), Some(true)));
    assert!(clip.iter().all(|c| c["stand_in"] == true), "a test run never touches the user's clipboard");
    assert!(r.events("element").iter().any(|l| l["kind"] == "change"), "the paste reached the input and its on_change");
    // Nothing reported holds the text that was pasted.
    assert!(!serde_json::to_string(&r.lines).unwrap().contains("pasted"));
}

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn the_input_method_follows_the_focus_between_a_plain_and_a_password_input_in_one_popup() {
    let both = widget(
        "both",
        "B",
        "",
        "style = { width = 220, height = 90, background_color = \"#334455\" },",
        "rsb.input { placeholder = \"name\" }, rsb.input { password = true }",
    );
    let files = vec![bar(&["bo"], &[], &[]), w("bo", both)];
    // Opened by a command (a gesture) so it takes focus; the first input gets it, Tab moves on, Tab wraps back.
    let keys = "900 popup:both key Tab\n1300 popup:both key Tab\n";
    let r = run(
        "imepw",
        &[files, vec![file("keys.txt", keys)]].concat(),
        &["--simulate-popup", "500:both:open", "--simulate-key", "@dir@/keys.txt"],
        3,
    );
    let stages: Vec<&str> =
        r.events("ime").iter().map(|l| l["stage"].as_str().unwrap()).filter(|s| *s != "caret").collect();
    assert_eq!(stages, ["enabled", "disabled", "enabled"], "plain, password, plain: {:?}", r.events("ime"));
    assert!(r.events("focus").iter().any(|f| f["popup"] == "both" && f["got"] == true), "the popup got focus");
}

// ---------------------------------------------------------------- overlays

/// An overlay (the power menu's shape): a Lock button that has the focus, an Other button, and a root
/// that gets the class `locking` while a lock is under way. `lock_click` is the Lock button's Luau.
fn overlay_widget(lock_click: &str) -> String {
    format!(
        r##"local rsb = require("@rsb")
return rsb.widget {{
	permissions = {{ overlay = true, control = {{ "power" }} }},
	setup = function(props, ctx)
		local open = rsb.state(false)
		local locking = rsb.state(false)
		return rsb.row {{
			style = {{ padding = "0 12px" }},
			rsb.text {{ text = "O" }},
			rsb.popup {{
				name = "pw",
				open = open,
				placement = "screen",
				class = function() return locking.value and "locking" or "idle" end,
				style = {{ width = "100%", height = "100%", background_color = "rgba(0,0,0,0.5)", justify_content = "center", align_items = "center" }},
				on_close = function(reason)
					locking.value = false
					rsb.log.info("overlay closed " .. reason)
				end,
				function()
					return {{
						rsb.row {{
							style = {{ gap = 8 }},
							rsb.box {{
								focusable = true,
								autofocus = true,
								style = {{ width = 80, height = 40, background_color = "#445566" }},
								on_click = function()
									rsb.log.info("lock clicked")
									{lock_click}
								end,
								rsb.text {{ text = "Lock" }},
							}},
							rsb.box {{
								focusable = true,
								style = {{ width = 80, height = 40, background_color = "#554466" }},
								on_click = function() rsb.log.info("other clicked") end,
								rsb.text {{ text = "Other" }},
							}},
						}},
					}}
				end,
			}},
		}}
	end,
}}
"##
    )
}

/// What the Lock button does in the lock test: put up the curtain, then ask the power service.
const LOCKING_CLICK: &str =
    "locking.value = true; rsb.timer.after(\"150ms\", function() rsb.services.power:lock() end)";

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn an_overlay_ignores_input_until_its_guard_lifts_and_a_repeat_never_activates_a_button() {
    // Opened at about 140 ms: the guard lifts at about 540 ms. Input before it, and a held key after it, do nothing.
    let keys = "250 popup:pw key Enter\n800 popup:pw key Enter\n1000 popup:pw key Enter down repeat\n";
    let pointer = "300 popup:pw down 40 20 left\n350 popup:pw up 40 20 left\n1500 popup:pw down 40 20 left\n1550 popup:pw up 40 20 left\n";
    let files =
        vec![bar(&["ov"], &[], &[]), w("ov", overlay_widget("")), file("keys.txt", keys), file("pointer.txt", pointer)];
    let r = run(
        "guard",
        &files,
        &[
            "--simulate-popup",
            "100:pw:open,2200:pw:close",
            "--simulate-key",
            "@dir@/keys.txt",
            "--simulate-pointer",
            "@dir@/pointer.txt",
        ],
        3,
    );
    assert_eq!(
        r.log_count("lock clicked"),
        2,
        "only the Enter at 800 ms and the press and release at 1500 ms did anything: {}",
        r.log
    );
    assert_eq!(r.log_count("other clicked"), 0);
    assert_eq!(r.popup("pw")[1]["overlay"], true);
    assert_eq!(r.events("focus")[0]["got"], true, "an overlay always asks, and a gesture opened it");
    // Centred on the monitor: the content window is only as large as its two buttons.
    let rc = r.rect_of("pw");
    let work = r.work();
    assert_eq!((rc[0] + rc[2]) / 2, (work[0] + work[2]) / 2, "centred across: {rc:?}");
    assert_eq!((rc[2] - rc[0], rc[3] - rc[1]), (px(168.0), px(40.0)), "{rc:?}");
}

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn the_lock_goes_only_through_the_recorder_and_the_curtain_never_stays_up() {
    let keys = "700 popup:pw key Enter\n";
    let files = vec![bar(&["ov"], &[], &[]), w("ov", overlay_widget(LOCKING_CLICK)), file("keys.txt", keys)];
    // The real services, behind the command recorder: the Lock button's command stops at the gate.
    let r = run_with(
        "lock",
        &files,
        &[
            "--simulate-popup",
            "100:pw:open,2500:pw:open",
            "--simulate-key",
            "@dir@/keys.txt",
            "--simulate-session",
            "3200:lock",
        ],
        4,
        false,
    );
    let gate = r.events("gate");
    assert_eq!((gate[0]["recorder"].as_bool(), gate[0]["forward"].as_bool()), (Some(true), Some(false)));
    let commands = r.events("command");
    assert_eq!(commands.len(), 1, "{commands:?}");
    assert_eq!((commands[0]["service"].as_str(), commands[0]["kind"].as_str()), (Some("power"), Some("lock")));
    assert_eq!(commands[0]["forwarded"], false, "the lock never reached a real service");
    assert!(r.events("service_command").is_empty());
    assert!(r.events("power_action").is_empty(), "and so never reached the power actions either");
    // No session lock came, so the curtain timed out 1 s after it went up.
    let reasons: Vec<&str> =
        r.popup("pw").iter().filter(|l| l["state"] == "close").map(|l| l["reason"].as_str().unwrap()).collect();
    assert_eq!(reasons, ["command", "deactivated"], "curtain timeout, then a session lock: {:?}", r.seq("pw"));
    assert_eq!(r.log_count("lock clicked"), 1);
    assert_eq!(r.log_count("overlay closed command"), 1, "the script heard that the curtain was taken down");
    assert_eq!(r.log_count("overlay closed deactivated"), 1);
}

// ---------------------------------------------------------------- tooltips

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn a_tooltip_shows_after_its_delay_at_once_after_another_and_hides_on_leave_and_on_a_press() {
    let src = r##"local rsb = require("@rsb")
return rsb.widget {
	permissions = {},
	setup = function(props, ctx)
		return rsb.row {
			style = { padding = "0 12px", width = 120, height = 24 },
			tooltip = "tip",
			rsb.text { text = "hover me" },
		}
	end,
}
"##;
    // Onto the widget at 800, away at 2000, back at 2100 (less than 300 ms after it hid), a press at 2700.
    let (near, far, y) = (px(30.0), px(900.0), bar_y());
    let pointer = format!(
        "800 main move {near} {y}\n2000 main move {far} {y}\n2100 main move {near} {y}\n\
         2700 main down {near} {y} left\n2760 main up {near} {y} left\n"
    );
    let files = vec![bar(&["tp"], &[], &[]), w("tp", src.to_string()), file("pointer.txt", pointer)];
    let r = run("tooltip", &files, &["--simulate-pointer", "@dir@/pointer.txt"], 4);
    let seq = r.seq("tooltip");
    assert_eq!(
        seq,
        ["open", "shown", "close:command", "closed", "open", "shown", "close:command", "closed"],
        "{:?}",
        r.popup("tooltip")
    );
    let shown: Vec<u64> = r.shown("tooltip").iter().map(|l| Run::t_ms(l)).collect();
    let moved_in = 800 + 40; // the player starts about 40 ms after the process
    assert!(shown[0] >= moved_in + 450 && shown[0] <= moved_in + 800, "after the 500 ms delay: {shown:?}");
    assert!(shown[1] >= 2100 && shown[1] <= 2100 + 40 + 250, "at once, not after another 500 ms: {shown:?}");
    let rc = r.rect_of("tooltip");
    assert!(rc[3] > rc[1] && rc[2] > rc[0]);
}

// ---------------------------------------------------------------- the command

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn rsb_popup_reaches_the_running_bar_and_reports_unknown_names_and_bad_usage() {
    let files = vec![bar(&["pa"], &[], &[]), w("pa", panel("al", "A"))];
    let dir = scratch("cli");
    for (rel, text) in &files {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let tag = format!("popups-cli-{}", std::process::id());
    let stats = dir.join("stats.jsonl");
    let mut bar_proc = Command::new(EXE)
        .env("RSB_TEST_TAG", &tag)
        .arg("--config-dir")
        .arg(dir.join("cfg"))
        .arg("--log-dir")
        .arg(dir.join("logs"))
        .arg("--stats-out")
        .arg(&stats)
        .args(["--no-reserve", "--test-offscreen", "--no-hook", "--no-activate", "--fake-services", "--seconds", "9"])
        .spawn()
        .expect("rsb starts");
    let client = |args: &[&str]| {
        let out = Command::new(EXE).env("RSB_TEST_TAG", &tag).args(args).output().expect("rsb runs");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        )
    };
    // Wait for the bar's pipe.
    let until = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while client(&["status", "--json"]).0 != 0 && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    std::thread::sleep(std::time::Duration::from_millis(600));
    let open = client(&["popup", "al", "open"]);
    assert_eq!((open.0, open.1.as_str()), (0, "ok"), "{open:?}");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let unknown = client(&["popup", "nosuch"]);
    assert_eq!(unknown.0, 1, "{unknown:?}");
    assert!(unknown.2.contains("no popup named 'nosuch'"), "{unknown:?}");
    assert_eq!(client(&["popup"]).0, 2, "no name is a usage error");
    assert_eq!(client(&["popup", "al", "sideways"]).0, 2, "an unknown action is a usage error");
    assert_eq!(client(&["popup", "Bad Name"]).0, 2);
    let toggled = client(&["popup", "al"]);
    assert_eq!((toggled.0, toggled.1.as_str()), (0, "ok"), "toggle is the default action: {toggled:?}");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let open_again = client(&["popup", "al", "open"]);
    assert_eq!(open_again.0, 0);
    std::thread::sleep(std::time::Duration::from_millis(500));
    // Hiding the bar closes what it opened.
    assert_eq!(client(&["toggle", "main"]).0, 0);
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert_eq!(client(&["quit"]).0, 0);
    assert!(bar_proc.wait().unwrap().success());
    // No bar any more: exit 3.
    assert_eq!(client(&["popup", "al"]).0, 3);
    let lines: Vec<Value> =
        std::fs::read_to_string(&stats).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let seq: Vec<String> = lines
        .iter()
        .filter(|l| l["event"] == "popup" && l["name"] == "al")
        .map(|l| match l["reason"].as_str() {
            Some(r) => format!("{}:{r}", l["state"].as_str().unwrap()),
            None => l["state"].as_str().unwrap().to_string(),
        })
        .collect();
    assert_eq!(
        seq,
        ["open", "shown", "close:command", "closed", "open", "shown", "close:bar_hidden", "closed"],
        "open, the toggle closed it, open again, the bar hidden: {seq:?}"
    );
    assert!(lines.iter().filter(|l| l["event"] == "popup" && l["state"] == "open").all(|l| l["gesture"] == true));
}

// ---------------------------------------------------------------- the budget

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn the_first_popup_frame_follows_the_click_within_fifty_milliseconds_p95_of_twenty_opens() {
    let files = vec![bar(&["pa"], &[], &[]), w("pa", panel("al", "A"))];
    let mut pointer = String::new();
    // 40 clicks on the anchor, 150 ms apart: twenty opens and twenty closes.
    for i in 0..40 {
        let t = 800 + i * 150;
        let (x, y) = (px(20.0), bar_y());
        pointer.push_str(&format!("{t} main down {x} {y} left\n{} main up {x} {y} left\n", t + 20));
    }
    let r = run(
        "budget",
        &[files, vec![file("pointer.txt", pointer)]].concat(),
        &["--simulate-pointer", "@dir@/pointer.txt"],
        8,
    );
    let mut ms: Vec<f64> = r.shown("al").iter().map(|l| l["click_to_first_frame_ms"].as_f64().unwrap()).collect();
    assert_eq!(ms.len(), 20, "{:?}", r.seq("al"));
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p95 = ms[(ms.len() as f64 * 0.95).ceil() as usize - 1];
    println!(
        "click -> first popup frame over 20 opens ({}, this build): min {:.2} median {:.2} p95 {:.2} max {:.2} ms",
        rsb_app::stats::session_name(),
        ms[0],
        ms[10],
        p95,
        ms[19]
    );
    assert!(p95 < 50.0, "p95 {p95} ms");
    assert_eq!(r.code, 0);
}

// ---------------------------------------------------------------- attach, the watchdog

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn an_attached_popup_takes_the_width_of_its_anchors_flat_edge_and_sits_on_it() {
    let wide = |name: &str, label: &str, attach: &str| {
        widget(
            name,
            label,
            "width = 100, height = 24",
            &format!(
                "placement = \"below\", {attach} style = {{ width = 300, height = 80, background_color = \"#223344\" }},"
            ),
            "rsb.text { text = \"x\" }",
        )
    };
    let files = vec![
        bar(&["at", "na"], &[], &[]),
        w("at", wide("attached", "A", "attach = true,")),
        w("na", wide("plain", "P", "")),
    ];
    let r = run("attach", &files, &["--simulate-popup", "600:attached:open,1300:plain:open"], 3);
    let (a, p) = (r.rect_of("attached"), r.rect_of("plain"));
    assert_eq!(a[2] - a[0], px(100.0), "the anchor's width, not the CSS width: {a:?}");
    assert_eq!(p[2] - p[0], px(300.0), "{p:?}");
    assert_eq!(a[3] - a[1], px(80.0));
    assert_eq!(a[1], p[1], "both sit on their anchors' bottom edge (the same row)");
}

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn a_widget_without_overlay_cannot_cover_the_screen_or_put_its_popup_back_after_a_close() {
    // Opens its own popup from a timer, asks for the whole screen and for no way out, and opens it again
    // whenever it is closed.
    let src = r##"local rsb = require("@rsb")
return rsb.widget {
	permissions = {},
	setup = function(props, ctx)
		local open = rsb.state(false)
		rsb.timer.every("300ms", function() open.value = true end)
		return rsb.row {
			style = { padding = "0 12px" },
			rsb.text { text = "C" },
			rsb.popup {
				name = "cover",
				open = open,
				dismiss = { outside = false, escape = false },
				style = { width = 20000, height = 20000, background_color = "#000000" },
				function() return { rsb.text { text = "x" } } end,
			},
		}
	end,
}
"##;
    let files = vec![bar(&["cv"], &[], &[]), w("cv", src.to_string())];
    let r = run("cover", &files, &["--simulate-outside-press", "1500"], 5);
    let work = r.work();
    let rc = r.rect_of("cover");
    let (w_px, h_px) = (rc[2] - rc[0], rc[3] - rc[1]);
    assert!(w_px <= px(900.0) && h_px <= px(900.0), "at most 900 logical px each way: {rc:?}");
    assert!(w_px * 10 <= (work[2] - work[0]) * 6, "at most 60 % of the work area wide: {rc:?} in {work:?}");
    assert!(h_px * 10 <= (work[3] - work[1]) * 9, "at most 90 % of it tall: {rc:?} in {work:?}");
    let seq = r.seq("cover");
    assert_eq!(
        &seq[..4],
        ["open", "shown", "close:outside", "closed"],
        "dismiss.outside = false is ignored for a popup nobody asked for: {:?}",
        r.popup("cover")
    );
    assert!(!seq[4..].iter().any(|s| s == "open"), "the timer cannot put it back within ten seconds: {seq:?}");
    assert!(seq.iter().any(|s| s == "refused:refused"), "the refusals are on record: {seq:?}");
    assert_eq!(r.log_count("stays closed until a click or a key"), 1, "said once, without the name: {}", r.log);
}

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn a_loop_turn_of_a_second_or_more_is_logged_once_and_written_to_the_stats() {
    let files = vec![bar(&[], &[], &[])];
    let r = run("slowturn", &files, &["--test-ui-stall", "1000:1500"], 4);
    let lines = r.events("stalled");
    assert_eq!(lines.len(), 1, "exactly one line for one slow turn: {lines:?}");
    let ms = lines[0]["ui_ms"].as_u64().expect("ui_ms");
    assert!((1500..4000).contains(&ms), "the turn lasted about 1.5 s: {ms}");
    assert_eq!(r.log_count("the bar did not draw for"), 1, "{}", r.log);
    // A stall under a second is not reported.
    let short = run("shortturn", &files, &["--test-ui-stall", "1000:400"], 3);
    assert!(short.events("stalled").is_empty() && short.log_count("the bar did not draw for") == 0);
}

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn a_stalled_ui_thread_has_its_overlay_hidden_by_the_watchdog_and_the_script_is_told() {
    let files = vec![bar(&["ov"], &[], &[]), w("ov", overlay_widget(""))];
    // The UI thread stops for 3 s from 800 ms; the overlay was opened at 100 ms.
    let r = run("stall", &files, &["--simulate-popup", "100:pw:open", "--test-ui-stall", "800:3000"], 6);
    let dog = r.events("watchdog");
    assert_eq!(dog.len(), 1, "{dog:?}");
    assert_eq!(dog[0]["overlays_hidden"], 1);
    let at = Run::t_ms(dog[0]);
    assert!((2800..3600).contains(&at), "after 2 s of stall, not before the UI thread stopped answering: {at} ms");
    assert_eq!(r.seq("pw"), ["open", "shown", "close:command", "closed"], "{:?}", r.popup("pw"));
    assert_eq!(r.log_count("overlay closed command"), 1);
    // The dim was painted once, however long it was shown.
    let closed = r.popup("pw").into_iter().find(|l| l["state"] == "closed").unwrap();
    assert!(closed["dim_paints"].as_u64().unwrap() <= 1, "{closed:?}");
}

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn a_fullscreen_notice_closes_panels_but_never_our_own_overlay() {
    let files = vec![bar(&["ov", "pa"], &[], &[]), w("ov", overlay_widget("")), w("pa", panel("al", "A"))];
    // A panel and then the overlay (opening the overlay replaces the panel, so the panel goes first).
    let r = run(
        "own-overlay",
        &files,
        &["--simulate-popup", "300:al:open,900:pw:open,2300:pw:close", "--simulate-fullscreen", "700:on,1500:on"],
        3,
    );
    assert_eq!(r.seq("al")[..3], ["open", "shown", "close:bar_hidden"], "the notice at 700 ms closed the panel");
    assert_eq!(
        r.seq("pw"),
        ["open", "shown", "close:command", "closed"],
        "the notice at 1500 ms, with the overlay up, did not close it: {:?}",
        r.popup("pw")
    );
}

/// A widget with no permission that shows 100 pictures of its own in a popup and resizes them every 100 ms, so
/// every frame wants new rasters. Each picture is cheap alone and within its own cost cap; together they would
/// hold the UI thread for a second if all were drawn in one frame.
fn picture_wall_widget() -> String {
    let img = |i: usize| format!("img({i})");
    let rows: Vec<String> = (0..10)
        .map(|r| format!("rsb.row {{ {} }}", (0..10).map(|c| img(r * 10 + c)).collect::<Vec<_>>().join(", ")))
        .collect();
    format!(
        r##"local rsb = require("@rsb")
return rsb.widget {{
	permissions = {{}},
	setup = function(props, ctx)
		local n = rsb.state(60)
		rsb.timer.every("100ms", function()
			n.value = 60 + ((n:peek() - 59) % 7)
		end)
		local function img(i)
			return rsb.image {{
				src = "h" .. i .. ".svg",
				style = function()
					return {{ width = n.value, height = n.value }}
				end,
			}}
		end
		return rsb.row {{
			rsb.text "x",
			rsb.popup {{
				name = "wall",
				open = rsb.state(false),
				function()
					return {{ rsb.column {{ {} }} }}
				end,
			}},
		}}
	end,
}}
"##,
        rows.join(", ")
    )
}

/// 400 half-transparent squares: 401 layers, the most a 96 px picture may use under the layer allowance.
fn layered_svg(k: usize) -> String {
    let mut s = String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">"#);
    for i in 0..400 {
        s.push_str(&format!(
            r##"<g opacity="0.5"><rect x="{}" y="{}" width="60" height="60" fill="#{:02x}6382"/></g>"##,
            i % 40,
            (i * 3 + k) % 40,
            (k * 20) % 256
        ));
    }
    s.push_str("</svg>");
    s
}

#[test]
#[ignore = "shows windows: run through the desktop lock"]
fn a_hundred_pictures_from_a_widget_with_no_permission_never_hold_the_ui_thread() {
    let mut files = vec![bar(&["heavy/w"], &[], &[]), w("heavy/w", picture_wall_widget())];
    for k in 0..100 {
        files.push(file(&format!("cfg/widgets/heavy/h{k}.svg"), layered_svg(k)));
    }
    let r = run("wall", &files, &["--simulate-popup", "500:wall:open", "--probe-ui"], 8);
    assert!(!r.shown("wall").is_empty(), "the popup opened: {:?}", r.seq("wall"));
    let probe = r.events("probe");
    assert_eq!(probe.len(), 1, "{probe:?}");
    let worst = probe[0]["worst_after_first_frame_ms"].as_f64().unwrap();
    eprintln!("worst UI gap {worst:.1} ms ({} session)", probe[0]["session"]);
    // Before the frame budget the same run stalled for a second at a time. A debug build rasterises about
    // ten times slower (one 401-layer picture takes over a second), so both the number and the stall log
    // line are only held to in a release build:
    // cargo test --release -p rsb --test popups -- --ignored a_hundred
    if !cfg!(debug_assertions) {
        assert!(worst < 50.0, "the UI thread went {worst:.1} ms without answering");
        assert_eq!(r.log_count("the bar did not draw for"), 0);
    }
}

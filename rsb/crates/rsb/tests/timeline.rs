//! `rsb render --timeline`: a script of pointer, key, popup and fake-service
//! steps played on a virtual clock with no window, and the pictures it
//! captures. Nothing here shows a window or sends input anywhere, so these
//! run with the other tests.

use std::path::{Path, PathBuf};
use std::process::Command;

use rsb_app::pngread;

const EXE: &str = env!("CARGO_BIN_EXE_rsb");
/// The desktop colour a capture composes onto when no background is given.
const DESKTOP: [u8; 3] = [0x30, 0x36, 0x40];

fn scratch(tag: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("timeline-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the scratch folder can be made");
    dir
}

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Runs `rsb render --timeline` over `lines`, into `<dir>/out`, with `<dir>/cfg` as the config folder.
fn render(dir: &Path, lines: &str, extra: &[&str]) -> Out {
    write(dir, "t.txt", lines);
    std::fs::create_dir_all(dir.join("cfg")).unwrap();
    let out = Command::new(EXE)
        .env("RSB_TEST_TAG", format!("timeline-{}", std::process::id()))
        .arg("--config-dir")
        .arg(dir.join("cfg"))
        .arg("--log-dir")
        .arg(dir.join("logs"))
        .args(["render", "--timeline"])
        .arg(dir.join("t.txt"))
        .arg("--out-dir")
        .arg(dir.join("out"))
        .args(extra)
        .output()
        .expect("rsb starts");
    Out {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

struct Pic {
    width: u32,
    height: u32,
    /// Premultiplied RGBA, top-down.
    data: Vec<u8>,
}

impl Pic {
    fn open(dir: &Path, name: &str) -> Pic {
        let img = pngread::read(&dir.join("out").join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        Pic { width: img.width, height: img.height, data: img.rgba }
    }

    fn rgb(&self, x: u32, y: u32) -> [u8; 3] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    /// The last x in `from..to` on row `y` whose colour is not the desktop's.
    fn right_edge(&self, y: u32, from: u32, to: u32) -> u32 {
        (from..to.min(self.width)).rev().find(|&x| self.rgb(x, y) != DESKTOP).unwrap_or(0)
    }

    /// How many pixels of the box differ from `other`'s.
    fn diff(&self, other: &Pic, x0: u32, y0: u32, x1: u32, y1: u32) -> usize {
        let mut n = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                if self.rgb(x, y) != other.rgb(x, y) {
                    n += 1;
                }
            }
        }
        n
    }
}

fn pngs_equal(a: &Path, b: &Path, name: &str) -> bool {
    std::fs::read(a.join("out").join(name)).unwrap() == std::fs::read(b.join("out").join(name)).unwrap()
}

const HOVER: &str = "0 capture rest\n\
                     1000 pointer main move 1100 18\n\
                     1050 capture d050\n\
                     1100 capture d100\n\
                     1400 capture d400\n\
                     1500 pointer main leave 1100 18\n\
                     2200 capture after\n";

#[test]
fn hovering_the_date_widens_it_over_200_ms_of_virtual_time_and_leaving_narrows_it() {
    let dir = scratch("hover");
    let r = render(&dir, HOVER, &["--screen", "1920x200"]);
    assert_eq!(r.code, 0, "{} {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("wrote 5 picture(s), 0 problem(s)"), "{}", r.stdout);
    let (rest, d50, d100, d400, after) = (
        Pic::open(&dir, "rest.png"),
        Pic::open(&dir, "d050.png"),
        Pic::open(&dir, "d100.png"),
        Pic::open(&dir, "d400.png"),
        Pic::open(&dir, "after.png"),
    );
    assert_eq!((rest.width, rest.height), (1920, 200), "the screen is the size asked for");
    let edge = |p: &Pic| p.right_edge(30, 1000, 1700);
    let (e0, e50, e100, e400) = (edge(&rest), edge(&d50), edge(&d100), edge(&d400));
    assert!(e400 > e0 + 150, "the long date is 262 px against 77: {e0} -> {e400}");
    assert!(e0 < e50 && e50 < e100 && e100 < e400, "it grows with the virtual time: {e0} {e50} {e100} {e400}");
    assert_eq!(edge(&after), e0, "and goes back once the pointer left");
    assert_eq!(after.diff(&rest, 0, 0, 1920, 200), 0, "after the leave the screen is the resting screen again");
}

#[test]
fn the_same_timeline_gives_the_same_bytes_every_time() {
    let (a, b) = (scratch("same-a"), scratch("same-b"));
    assert_eq!(render(&a, HOVER, &["--screen", "1920x200"]).code, 0);
    assert_eq!(render(&b, HOVER, &["--screen", "1920x200"]).code, 0);
    for name in ["rest", "d050", "d100", "d400", "after"] {
        assert!(pngs_equal(&a, &b, &format!("{name}.png")), "{name} differs between two runs");
    }
    let index = std::fs::read_to_string(a.join("out").join("index.jsonl")).unwrap();
    let lines: Vec<serde_json::Value> = index.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 5);
    assert_eq!((lines[1]["label"].as_str(), lines[1]["t_ms"].as_u64()), (Some("d050"), Some(1050)));
    assert_eq!(lines[0]["target"], "screen");
    assert_eq!(lines[0]["size"], serde_json::json!([1920, 200]));
}

#[test]
fn the_clock_is_fixed_at_now_and_never_moves_with_the_virtual_time() {
    let dir = scratch("now");
    let lines = "0 capture early\n60000 capture late\n";
    let r = render(&dir, lines, &["--screen", "1920x200", "--now", "2031-03-04T05:06:07"]);
    assert_eq!(r.code, 0, "{} {}", r.stdout, r.stderr);
    let (early, late) = (Pic::open(&dir, "early.png"), Pic::open(&dir, "late.png"));
    assert_eq!(early.diff(&late, 0, 0, 1920, 200), 0, "a minute later the same wall clock is drawn");
    let default = scratch("now-default");
    assert_eq!(render(&default, "0 capture early\n", &["--screen", "1920x200"]).code, 0);
    let other = Pic::open(&default, "early.png");
    assert!(early.diff(&other, 900, 0, 1200, 40) > 0, "another --now draws another clock");
}

#[test]
fn a_fake_line_changes_what_a_widget_shows_and_keeps_the_other_fields() {
    let dir = scratch("fake");
    let lines = "0 capture before\n\
                 100 fake battery {\"percent\": 5}\n\
                 200 capture after\n\
                 300 fake battery {\"percent\": 87}\n\
                 400 capture back\n";
    let r = render(&dir, lines, &["--screen", "1920x200"]);
    assert_eq!(r.code, 0, "{} {}", r.stdout, r.stderr);
    let (before, after, back) =
        (Pic::open(&dir, "before.png"), Pic::open(&dir, "after.png"), Pic::open(&dir, "back.png"));
    assert!(before.diff(&after, 1700, 0, 1920, 40) > 20, "5 % is not drawn like 87 %");
    assert_eq!(before.diff(&after, 0, 0, 1600, 40), 0, "nothing else on the bar moved");
    assert_eq!(before.diff(&back, 0, 0, 1920, 200), 0, "setting it back draws the first picture again");
}

#[test]
fn a_bar_can_be_captured_alone_and_a_closed_popup_is_a_failed_step_not_a_blank_picture() {
    let dir = scratch("targets");
    let lines = "100 capture bar_only main\n200 capture nothing popup:nothing\n300 capture after\n";
    let r = render(&dir, lines, &["--screen", "1280x200"]);
    assert_eq!(r.code, 1, "{} {}", r.stdout, r.stderr);
    assert!(r.stderr.contains("line 2: capture 'nothing': popup:nothing is not on screen at 200 ms"), "{}", r.stderr);
    assert!(r.stdout.contains("wrote 2 picture(s), 1 problem(s)"), "{}", r.stdout);
    let bar = Pic::open(&dir, "bar_only.png");
    assert_eq!((bar.width, bar.height), (1280, 36), "a bar is its own window's size");
    assert!(!dir.join("out").join("nothing.png").exists());
    assert!(dir.join("out").join("after.png").exists(), "the steps after a failed one still run");
}

const PANEL: &str = "--!strict\nlocal rsb = require(\"@rsb\")\n\nreturn rsb.widget {\n\tpermissions = {},\n\tsetup = function(props, ctx)\n\t\tlocal open = rsb.state(false)\n\t\treturn rsb.row {\n\t\t\tclass = \"item dashboard button\",\n\t\t\trsb.icon { svg = \"logo\" },\n\t\t\ton_click = function()\n\t\t\t\trsb.popups.toggle(\"mypanel\")\n\t\t\tend,\n\t\t\trsb.popup {\n\t\t\t\tname = \"mypanel\",\n\t\t\t\topen = open,\n\t\t\t\tattach = true,\n\t\t\t\tplacement = \"below\",\n\t\t\t\tclass = \"testpanel\",\n\t\t\t\tfunction()\n\t\t\t\t\treturn { rsb.text { text = \"panel\", class = \"testtext\" } }\n\t\t\t\tend,\n\t\t\t},\n\t\t}\n\tend,\n}\n";
const PANEL_CSS: &str = "popup.testpanel { height: 300px; max-height: 0; background: #252b33; color: #e6e9ee; overflow: hidden; transition: max-height 200ms ease-in-out 200ms; }\npopup.testpanel:open { max-height: 300px; }\n.testtext { padding: 16px; opacity: 0; transition: opacity 150ms ease-out 420ms; }\npopup.testpanel:open .testtext { opacity: 1; }\n";

fn panel_config(dir: &Path) {
    write(&dir.join("cfg"), "widgets/mypanel.luau", PANEL);
    write(&dir.join("cfg"), "style.css", PANEL_CSS);
    write(
        &dir.join("cfg"),
        "bar.toml",
        "config_version = 1\n[[bar]]\ncenter = [\"memory\", \"cpu\", \"mypanel\", \"clock\", \"date\"]\ncenter_on = \"mypanel\"\n",
    );
}

const PANEL_CSS_HEIGHT: &str = "popup.testpanel { height: 0; background: #252b33; color: #e6e9ee; overflow: hidden; }
popup.testpanel:open { height: 300px; transition: height 200ms ease-in-out 200ms; }
";

#[test]
fn a_panel_that_grows_by_its_height_does_not_jump_while_the_anchor_stretches() {
    let dir = scratch("panel_height");
    panel_config(&dir);
    write(&dir.join("cfg"), "style.css", PANEL_CSS_HEIGHT);
    let lines = "500 pointer main move 960 18
                 1000 pointer main down 960 18
                 1040 pointer main up 960 18
                 1100 capture f100
                 1340 capture f300
                 1900 capture f860
";
    let r = render(&dir, lines, &["--screen", "1920x600"]);
    assert_eq!(r.code, 0, "{} {}", r.stdout, r.stderr);
    let panel_at = |p: &Pic, y: u32| p.rgb(960, y) == [0x25, 0x2b, 0x33];
    let (f100, f300, f860) = (Pic::open(&dir, "f100.png"), Pic::open(&dir, "f300.png"), Pic::open(&dir, "f860.png"));
    assert!(!panel_at(&f100, 200), "the panel waits for the stretch");
    assert!(panel_at(&f300, 60) && !panel_at(&f300, 330), "half way down 100 ms into its growth");
    assert!(panel_at(&f860, 330) && !panel_at(&f860, 400), "300 px tall when it is done");
}

#[test]
fn an_attached_popup_opens_on_the_anchors_edge_and_grows_with_the_virtual_clock() {
    let dir = scratch("panel");
    panel_config(&dir);
    let lines = "500 pointer main move 960 18\n\
                 1000 pointer main down 960 18\n\
                 1040 pointer main up 960 18\n\
                 1056 capture f016\n\
                 1140 capture f100\n\
                 1300 capture f260\n\
                 1500 capture f460\n\
                 1900 capture f860\n\
                 1900 capture panel popup:mypanel\n\
                 3000 pointer main down 960 18\n\
                 3040 pointer main up 960 18\n\
                 3300 capture closing popup:mypanel\n\
                 3800 capture closed\n\
                 3800 capture gone popup:mypanel\n";
    let stats = dir.join("stats.jsonl");
    let r = render(&dir, lines, &["--screen", "1920x600", "--stats-out", stats.to_str().unwrap()]);
    // The last capture is of a popup that has closed: one failed step, the rest written.
    assert_eq!(r.code, 1, "{} {}", r.stdout, r.stderr);
    assert!(r.stderr.contains("popup:mypanel is not on screen at 3800 ms"), "{}", r.stderr);
    let panel_at = |p: &Pic, y: u32| p.rgb(960, y) == [0x25, 0x2b, 0x33];
    let (f016, f100, f300, f460, f860) = (
        Pic::open(&dir, "f016.png"),
        Pic::open(&dir, "f100.png"),
        Pic::open(&dir, "f260.png"),
        Pic::open(&dir, "f460.png"),
        Pic::open(&dir, "f860.png"),
    );
    // The anchor stretches first (width over 200 ms), the panel's height starts when it is done (delay 200 ms).
    let strip = |p: &Pic| p.right_edge(20, 300, 1700) - (300..1700).find(|&x| p.rgb(x, 20) != DESKTOP).unwrap_or(0);
    assert!(
        strip(&f016) < strip(&f100) && strip(&f100) < strip(&f300),
        "{} {} {}",
        strip(&f016),
        strip(&f100),
        strip(&f300)
    );
    assert!(!panel_at(&f016, 200), "no panel in the first frame");
    assert!(!panel_at(&f100, 200), "the panel waits for the stretch");
    assert!(panel_at(&f460, 200) && panel_at(&f860, 330), "then it is there");
    assert!(!panel_at(&f860, 400), "300 px tall, no more");
    let panel = Pic::open(&dir, "panel.png");
    let anchor_flat = 500;
    assert!(
        panel.width.abs_diff(anchor_flat) <= 30,
        "an attached popup takes the width of the anchor's flat edge: {}",
        panel.width
    );
    assert_eq!(panel.height, 300);
    let closed = Pic::open(&dir, "closed.png");
    assert!(!panel_at(&closed, 200), "gone 760 ms after the click that closed it");
    assert_eq!(Pic::open(&dir, "closing.png").height, 300, "while it closes it keeps its last content");

    let text = std::fs::read_to_string(&stats).unwrap();
    let lines: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let frames = |surface: &str| lines.iter().filter(|l| l["event"] == "frame" && l["surface"] == surface).count();
    assert!(
        frames("bar:main") > 10 && frames("popup:mypanel") > 5,
        "{} {}",
        frames("bar:main"),
        frames("popup:mypanel")
    );
    let frame = lines.iter().find(|l| l["event"] == "frame" && l["surface"] == "popup:mypanel").unwrap();
    for key in ["render_us", "style_us", "layout_us"] {
        assert!(frame[key].is_u64(), "{key} in {frame}");
    }
    let marks: Vec<&str> =
        lines.iter().filter(|l| l["event"] == "timeline").filter_map(|l| l["label"].as_str()).collect();
    assert_eq!(marks, ["f016", "f100", "f260", "f460", "f860", "panel", "closing", "closed"]);
    assert!(lines.iter().any(|l| l["event"] == "popup" && l["state"] == "closed" && l["name"] == "mypanel"));
}

const TYPER: &str = "local rsb = require(\"@rsb\")\nreturn rsb.widget {\n\tpermissions = {},\n\tsetup = function(props, ctx)\n\t\tlocal open = rsb.state(false)\n\t\tlocal typed = rsb.state(\"\")\n\t\treturn rsb.row {\n\t\t\tstyle = { padding = \"0 12px\" },\n\t\t\ton_click = function() open.value = not open.value end,\n\t\t\trsb.text { text = \"T\" },\n\t\t\trsb.popup {\n\t\t\t\tname = \"typer\",\n\t\t\t\topen = open,\n\t\t\t\tplacement = \"below\",\n\t\t\t\tstyle = { width = 260, height = 90, background_color = \"#223344\" },\n\t\t\t\tfunction()\n\t\t\t\t\treturn {\n\t\t\t\t\t\trsb.input { placeholder = \"type\", autofocus = true, on_change = function(t) typed.value = t end },\n\t\t\t\t\t\trsb.text { text = function(): string return \"got:\" .. typed.value end },\n\t\t\t\t\t}\n\t\t\t\tend,\n\t\t\t},\n\t\t}\n\tend,\n}\n";

#[test]
fn keys_and_text_reach_a_popup_that_a_click_opened() {
    let dir = scratch("typing");
    write(&dir.join("cfg"), "widgets/typer.luau", TYPER);
    write(&dir.join("cfg"), "bar.toml", "config_version = 1\n[[bar]]\nleft = [\"typer\"]\ncenter = []\nright = []\n");
    let lines = "300 pointer main move 14 18\n\
                 400 pointer main down 14 18\n\
                 440 pointer main up 14 18\n\
                 1000 capture empty\n\
                 1100 text typer h\\u0069\n\
                 1500 capture typed\n\
                 1600 key typer Escape\n\
                 2200 capture escaped\n";
    let r = render(&dir, lines, &["--screen", "800x300"]);
    assert_eq!(r.code, 0, "{} {}", r.stdout, r.stderr);
    let (empty, typed, escaped) =
        (Pic::open(&dir, "empty.png"), Pic::open(&dir, "typed.png"), Pic::open(&dir, "escaped.png"));
    assert!(empty.diff(&typed, 0, 36, 300, 140) > 30, "the typed text is drawn in the popup");
    assert_ne!(empty.rgb(100, 100), DESKTOP, "the popup is up before the keys");
    assert_eq!(escaped.rgb(100, 100), DESKTOP, "only the desktop is where the popup was after Escape");
}

#[test]
fn a_background_picture_is_the_desktop_and_a_flat_colour_can_be_named() {
    let dir = scratch("background");
    let flat = render(&dir, "0 capture flat\n", &["--screen", "800x200", "--background", "#102030"]);
    assert_eq!(flat.code, 0, "{} {}", flat.stdout, flat.stderr);
    let pic = Pic::open(&dir, "flat.png");
    assert_eq!(pic.rgb(400, 150), [0x10, 0x20, 0x30]);
    // A 2 x 2 picture: its top left pixel shows at the screen's top left, the rest is the default desktop.
    let bg = dir.join("bg.png");
    let mut canvas = rsb_render::Canvas::new(2, 2);
    canvas.data.copy_from_slice(&[0, 0, 255, 255].repeat(4));
    std::fs::write(&bg, canvas.to_png().unwrap()).unwrap();
    let picture = render(&dir, "0 capture picture\n", &["--screen", "800x200", "--background", bg.to_str().unwrap()]);
    assert_eq!(picture.code, 0, "{} {}", picture.stdout, picture.stderr);
    let pic = Pic::open(&dir, "picture.png");
    let red = pic.rgb(1, 1);
    assert!(red[0] >= 253 && red[1] <= 2 && red[2] <= 2, "the picture's red where it reaches: {red:?}");
    assert_eq!(pic.rgb(400, 150), DESKTOP, "and the default desktop where it does not");
    let bad = render(&dir, "0 capture x\n", &["--background", "#12"]);
    assert_eq!(bad.code, 2);
    assert!(bad.stderr.contains("is not #rrggbb or a PNG file"), "{}", bad.stderr);
}

#[test]
fn a_timeline_that_cannot_be_used_exits_2_and_says_which_line() {
    let dir = scratch("refusals");
    let bad = render(&dir, "100 capture ok\n200 wiggle now\n", &[]);
    assert_eq!(bad.code, 2, "{}", bad.stderr);
    assert!(bad.stderr.contains("timeline line 2: unknown step 'wiggle'"), "{}", bad.stderr);
    assert!(!dir.join("out").join("ok.png").exists(), "nothing runs when the file does not parse");
    let late = render(&dir, "\n\n# c\n999999999 outside\n", &[]);
    assert_eq!(late.code, 2);
    assert!(late.stderr.contains("timeline line 4:"), "{}", late.stderr);
    let twice = render(&dir, "1 capture a\n2 capture a\n", &[]);
    assert_eq!(twice.code, 2);
    assert!(twice.stderr.contains("timeline line 2: the label 'a' is used twice"), "{}", twice.stderr);
    let toaster = render(&dir, "1 fake toaster {}\n", &[]);
    assert_eq!(toaster.code, 2);
    assert!(toaster.stderr.contains("'toaster' is not a service"), "{}", toaster.stderr);
    let missing = Command::new(EXE)
        .args(["render", "--timeline"])
        .arg(dir.join("no-such-file.txt"))
        .arg("--out-dir")
        .arg(dir.join("out2"))
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("cannot read the timeline"));
}

#[test]
fn the_options_a_timeline_refuses_exit_2_before_anything_is_written() {
    let dir = scratch("options");
    write(&dir, "t.txt", "0 capture a\n");
    let t = dir.join("t.txt");
    let run = |args: &[&str]| {
        let out = Command::new(EXE).args(["render", "--timeline"]).arg(&t).args(args).output().unwrap();
        (out.status.code(), String::from_utf8_lossy(&out.stderr).into_owned())
    };
    // An output folder that is not under a scratch or target folder: never created.
    let elsewhere = Path::new("D:\\rsb-timeline-refused\\photos");
    let (code, err) = run(&["--out-dir", elsewhere.to_str().unwrap()]);
    assert_eq!(code, Some(2));
    assert!(err.contains("--out-dir writes only under a scratch or target folder"), "{err}");
    assert!(!elsewhere.exists());
    let up = format!("{}\\..\\..\\x", dir.display());
    let (code, err) = run(&["--out-dir", &up]);
    assert_eq!((code, err.contains("scratch or target")), (Some(2), true), "a .. cannot walk out of the folder: {err}");
    let ok = dir.join("out").display().to_string();
    for (args, needle) in [
        (vec!["--out-dir", &ok, "--live"], "--live cannot be used with --timeline"),
        (vec!["--out-dir", &ok, "--out", "x.png"], "belongs to a single picture"),
        (vec!["--out-dir", &ok, "--width", "800"], "belongs to a single picture"),
        (vec!["--out-dir", &ok, "--screen", "100x100"], "is outside"),
        (vec!["--out-dir", &ok, "--screen", "wide"], "is not <width>x<height>"),
        (vec!["--out-dir", &ok, "--scale", "9"], "--scale must be 0.5 to 4"),
        (vec!["--out-dir", &ok, "--theme", "grey"], "is not dark or light"),
        (vec!["--out-dir", &ok, "--now", "tomorrow"], "is not YYYY-MM-DDTHH:MM"),
        (vec!["--out-dir", &ok, "--bogus"], "unknown option '--bogus'"),
        (vec![], "needs --out-dir"),
    ] {
        let (code, err) = run(&args);
        assert_eq!(code, Some(2), "{args:?}: {err}");
        assert!(err.contains(needle), "{args:?}: {err}");
    }
    assert!(!dir.join("out").exists(), "a refused run creates nothing");
}

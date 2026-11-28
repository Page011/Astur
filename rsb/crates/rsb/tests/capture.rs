//! `--capture` and `--simulate-monitors` through the real binary: an offscreen
//! run with the fake services writes every present of every surface, and the
//! monitors that do not exist get their bars like a real one. The runs show
//! windows (far outside the screen), so they go through the desktop lock:
//!
//! `deskrun.sh cargo test -p rsb --test capture -- --ignored --test-threads=1`

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const EXE: &str = env!("CARGO_BIN_EXE_rsb");

fn scratch(tag: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("capture-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the scratch folder can be made");
    dir
}

struct Run {
    lines: Vec<Value>,
    code: i32,
    stderr: String,
}

impl Run {
    fn events(&self, name: &str) -> Vec<&Value> {
        self.lines.iter().filter(|l| l["event"] == name).collect()
    }
}

fn run(dir: &Path, flags: &[&str]) -> Run {
    let stats = dir.join("stats.jsonl");
    let out = Command::new(EXE)
        .env("RSB_TEST_TAG", format!("capture-{}", std::process::id()))
        .arg("--config-dir")
        .arg(dir.join("cfg"))
        .arg("--log-dir")
        .arg(dir.join("logs"))
        .arg("--stats-out")
        .arg(&stats)
        .args(["--no-reserve", "--test-offscreen", "--fake-services", "--no-hook", "--no-activate"])
        .args(flags)
        .output()
        .expect("rsb starts");
    let lines = std::fs::read_to_string(&stats)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).expect("a stats line is JSON"))
        .collect();
    Run { lines, code: out.status.code().unwrap_or(-1), stderr: String::from_utf8_lossy(&out.stderr).into_owned() }
}

fn index(dir: &Path) -> Vec<Value> {
    std::fs::read_to_string(dir.join("frames").join("index.jsonl"))
        .expect("the capture wrote its index")
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
#[ignore = "shows a window: run through the desktop lock"]
fn a_capture_writes_every_present_with_an_index_line_and_the_stats_lines() {
    let dir = scratch("frames");
    let frames = dir.join("frames");
    let r = run(&dir, &["--seconds", "2", "--stats-frames", "--capture", frames.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let lines = index(&dir);
    assert!(!lines.is_empty(), "the first frame is written");
    // The bar is 36 logical px: its height in device px is whatever the session's dpi makes of it.
    let bar_h = r.events("geometry").first().and_then(|g| g["h"].as_u64()).expect("the run reported its geometry");
    for l in &lines {
        assert_eq!(l["surface"], "bar:main", "one bar on one monitor carries no monitor number");
        let png = std::fs::read(frames.join(l["file"].as_str().unwrap())).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert!(l["t_ms"].as_u64().unwrap() <= 2500, "{l}");
        assert_eq!(l["size"][1], bar_h, "the size of the bar's surface: {l}");
    }
    let written = r.events("capture").last().map(|c| c["frames"].as_u64().unwrap()).unwrap();
    assert_eq!(written as usize, lines.len(), "the stats count what the index lists");
    let frame_lines = r.events("frame");
    assert!(!frame_lines.is_empty());
    for f in frame_lines {
        for key in ["render_us", "build_us", "style_us", "layout_us", "paint_us", "surface", "dirty"] {
            assert!(f.get(key).is_some(), "frame line without {key}: {f}");
        }
    }
    let pacing = r.events("pacing");
    assert_eq!(pacing.len(), 1, "one pacing line at the end");
    assert!(pacing[0]["presents"].as_u64().unwrap() >= 1 && pacing[0].get("period_us").is_some());
}

#[test]
#[ignore = "shows a window: run through the desktop lock"]
fn simulated_monitors_each_get_a_bar_at_their_size_and_dpi() {
    let dir = scratch("monitors");
    let frames = dir.join("frames");
    let r = run(
        &dir,
        &["--seconds", "2", "--simulate-monitors", "1280x720@96,3840x2160@192", "--capture", frames.to_str().unwrap()],
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    let bars = r.events("config")[0]["bars"].as_u64().unwrap();
    assert_eq!(bars, 3, "the real (offscreen) monitor and the two simulated ones");
    let geo: Vec<(u64, u64, u64)> = r
        .events("geometry")
        .iter()
        .map(|g| (g["w"].as_u64().unwrap(), g["h"].as_u64().unwrap(), g["dpi"].as_u64().unwrap()))
        .collect();
    assert!(geo.contains(&(1280, 36, 96)), "{geo:?}");
    assert!(geo.contains(&(3840, 72, 192)), "a 36 px bar is 72 px at 192 dpi: {geo:?}");
    let sizes: Vec<u64> = index(&dir).iter().map(|l| l["size"][0].as_u64().unwrap()).collect();
    assert!(sizes.contains(&1280) && sizes.contains(&3840), "a picture of each: {sizes:?}");
}

#[test]
fn a_capture_outside_scratch_and_target_or_without_the_fakes_exits_2() {
    let dir = scratch("refused");
    for (flags, says) in [
        (
            vec!["--capture", "C:\\Users\\someone\\Documents\\frames", "--fake-services", "--test-offscreen"],
            "scratch or target",
        ),
        (vec!["--capture", dir.to_str().unwrap(), "--test-offscreen"], "--capture needs --fake-services"),
        (vec!["--capture", dir.to_str().unwrap(), "--fake-services"], "--capture needs --test-offscreen"),
        (vec!["--simulate-monitors", "1280x720@96", "--fake-services"], "--simulate-monitors needs --test-offscreen"),
        (vec!["--simulate-monitors", "0x720@96", "--test-offscreen"], "monitor"),
    ] {
        let out = Command::new(EXE)
            .env("RSB_TEST_TAG", format!("capture-refused-{}", std::process::id()))
            .args(&flags)
            .output()
            .expect("rsb starts");
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{flags:?}: {err}");
        assert!(err.contains(says), "{flags:?} should say '{says}': {err}");
    }
}

//! Fonts and SVG pictures through the real binary: `rsb render` reads the
//! config folder's `fonts`, registers what the style sheet names, draws the
//! bundled icons by name and the user's own `.svg` files under the widget's
//! file permission. Nothing here opens a window.

use std::path::{Path, PathBuf};
use std::process::Command;

const EXE: &str = env!("CARGO_BIN_EXE_rsb");
const FONT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts/JetBrainsMono-wght.ttf");

fn scratch(tag: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("assets-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the scratch folder can be made");
    dir
}

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Renders the bar of `cfg`; returns stdout, the log and the picture.
fn render(dir: &Path, tag: &str) -> (String, String, rsb_render::Image) {
    let png = dir.join("out.png");
    let out = Command::new(EXE)
        .env("RSB_TEST_TAG", format!("assets-{tag}-{}", std::process::id()))
        .args(["render", "--out"])
        .arg(&png)
        .args(["--width", "400", "--wait-ms", "400", "--config-dir"])
        .arg(dir.join("cfg"))
        .arg("--log-dir")
        .arg(dir.join("logs"))
        .output()
        .expect("rsb starts");
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let image = rsb_render::decode_png(&std::fs::read(&png).unwrap()).expect("a PNG");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        std::fs::read_to_string(dir.join("logs").join("rsb.log")).unwrap_or_default(),
        image,
    )
}

/// How many pixels are plainly red: the tint the test icons are drawn in.
fn red_pixels(image: &rsb_render::Image) -> usize {
    let (pixels, _) = image.data.as_chunks::<4>();
    pixels.iter().filter(|p| p[3] > 100 && p[0] > p[1].saturating_add(80) && p[0] > p[2].saturating_add(80)).count()
}

const RED_ICONS: &str = "icon { color: #ff0000; font-size: 20px; }\n";

fn icon_widget(svg_expr: &str, permissions: &str) -> String {
    format!(
        "local rsb = require(\"@rsb\")\nreturn rsb.widget {{\n    {permissions}\n    setup = function()\n        return rsb.icon {{ svg = {svg_expr} }}\n    end,\n}}\n"
    )
}

fn config(dir: &Path, widget: &str, css: &str) {
    write(&dir.join("cfg"), "widgets/pic.luau", widget);
    write(&dir.join("cfg"), "bar.toml", "config_version = 1\n[[bar]]\nleft = [\"pic\"]\n");
    write(&dir.join("cfg"), "style.css", css);
}

#[test]
fn a_bundled_icon_is_drawn_from_its_name_in_the_elements_colour() {
    let dir = scratch("bundled");
    config(&dir, &icon_widget("\"power\"", ""), RED_ICONS);
    let (out, log, image) = render(&dir, "bundled");
    assert!(out.contains("0 plates"), "{out}\n{log}");
    // The preset's other widgets have some red of their own: the same bar
    // with no icon is the baseline.
    let blank = scratch("bundled-blank");
    config(
        &blank,
        "local rsb = require(\"@rsb\")\nreturn rsb.widget { setup = function() return rsb.text { text = \"\" } end }\n",
        RED_ICONS,
    );
    let (_, _, baseline) = render(&blank, "bundled-blank");
    let (with_icon, base) = (red_pixels(&image), red_pixels(&baseline));
    assert!(with_icon > base + 10, "the power icon adds {} red pixels", with_icon.saturating_sub(base));
}

#[test]
fn a_users_svg_file_is_drawn_under_the_widgets_file_permission_and_refused_without_it() {
    let dir = scratch("user");
    let file = dir.join("cfg").join("mine.svg");
    write(
        &dir.join("cfg"),
        "mine.svg",
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\"><circle cx=\"12\" cy=\"12\" r=\"10\"/></svg>",
    );
    let path = file.to_string_lossy().replace('\\', "/");
    let allowed = format!("permissions = {{ file = {{ \"{path}\" }} }},");
    config(&dir, &icon_widget(&format!("\"{path}\""), &allowed), RED_ICONS);
    let (out, allowed_log, image) = render(&dir, "user");
    assert!(out.contains("0 plates"), "{out}\n{allowed_log}");
    let drawn = red_pixels(&image);

    let denied = scratch("user-denied");
    write(
        &denied.join("cfg"),
        "mine.svg",
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\"><circle cx=\"12\" cy=\"12\" r=\"10\"/></svg>",
    );
    let path = denied.join("cfg").join("mine.svg").to_string_lossy().replace('\\', "/");
    config(&denied, &icon_widget(&format!("\"{path}\""), ""), RED_ICONS);
    let (denied_out, denied_log, _) = render(&denied, "user-denied");
    assert!(denied_out.contains("1 plate"), "no file permission: the widget shows a plate\n{denied_out}");
    assert!(denied_log.contains("PERMISSION"), "{denied_log}");

    // The preset's own red is the baseline: a bar with nothing of ours in it.
    let blank = scratch("user-blank");
    config(
        &blank,
        "local rsb = require(\"@rsb\")\nreturn rsb.widget { setup = function() return rsb.text { text = \"\" } end }\n",
        RED_ICONS,
    );
    let base = red_pixels(&render(&blank, "user-blank").2);
    assert!(drawn > base + 50, "the circle adds {} red pixels\n{allowed_log}", drawn.saturating_sub(base));
}

#[test]
fn a_font_face_in_the_fonts_folder_makes_its_family_known_and_a_missing_one_is_a_plate() {
    let dir = scratch("fonts");
    let cfg = dir.join("cfg");
    std::fs::create_dir_all(cfg.join("fonts")).unwrap();
    std::fs::copy(FONT, cfg.join("fonts").join("office.ttf")).unwrap();
    let face = "@font-face { font-family: \"Office Mono\"; src: url(\"fonts/office.ttf\"); }\nbar { font-family: \"Office Mono\"; }\n";
    config(&dir, &icon_widget("\"check\"", ""), face);
    let (out, log, _) = render(&dir, "fonts");
    assert!(out.contains("0 plates"), "{out}\n{log}");

    // No rule for the family: the warning says it is not installed.
    let bare = scratch("fonts-bare");
    config(&bare, &icon_widget("\"check\"", ""), "bar { font-family: \"Office Mono\"; }\n");
    let (out, ..) = render(&bare, "fonts-bare");
    assert!(out.contains("font 'Office Mono' is not installed"), "{out}");

    // A rule whose file is missing and one whose file is not a font.
    let bad = scratch("fonts-bad");
    std::fs::create_dir_all(bad.join("cfg").join("fonts")).unwrap();
    std::fs::write(bad.join("cfg").join("fonts").join("junk.ttf"), b"not a font").unwrap();
    let css = "@font-face { font-family: \"A\"; src: url(\"fonts/gone.ttf\"); }\n@font-face { font-family: \"B\"; src: url(\"fonts/junk.ttf\"); }\n";
    config(&bad, &icon_widget("\"check\"", ""), css);
    let (out, log, _) = render(&bad, "fonts-bad");
    assert!(out.contains("2 plates") || out.contains("1 plate"), "{out}");
    assert!(log.contains("font file 'fonts/gone.ttf' was not found in the config folder's fonts folder"), "{log}");
    assert!(log.contains("font file 'fonts/junk.ttf' is not a font"), "{log}");
}

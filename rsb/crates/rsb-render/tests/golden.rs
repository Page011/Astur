//! Golden images: fixed draw lists rendered offscreen and compared with the
//! PNGs in `tests/golden/`. Set `RSB_UPDATE_GOLDEN=1` to rewrite them (and
//! `tests/golden/fonts.txt`, the versions of the font files the text goldens
//! were made with). Text goldens are skipped, with the reason printed, when
//! this PC's font files differ from the recorded ones.
//!
//! Tolerance: every channel within 3 for at least 99.5 % of the pixels, and no
//! pixel off by more than 64. A failure writes `<name>-actual.png` and
//! `<name>-diff.png` to `target/golden-failures/`.

mod common;

use std::path::{Path, PathBuf};

use common::*;
use rsb_render::{Canvas, render_offscreen};
use swash::FontRef;
use swash::StringId;

const CHANNEL_TOLERANCE: i32 = 3;
const MAX_PIXEL_DIFF: i32 = 64;
const MIN_PIXELS_WITHIN: f64 = 0.995;

/// Font files the text goldens depend on.
const FONT_FILES: &[&str] = &[
    "segoeui.ttf",
    "segoeuib.ttf",
    "segoeuii.ttf",
    "seguiemj.ttf",
    "SegoeIcons.ttf",
    "segmdl2.ttf",
    "calibri.ttf",
    "Candara.ttf",
];

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden")
}

fn failures_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(t) => PathBuf::from(t).join("golden-failures"),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("target").join("golden-failures"),
    }
}

fn updating() -> bool {
    std::env::var_os("RSB_UPDATE_GOLDEN").is_some_and(|v| v == "1")
}

/// One line per font file: name, byte size, version string.
fn current_fonts() -> Vec<String> {
    let dir = Path::new("C:\\Windows\\Fonts");
    FONT_FILES
        .iter()
        .map(|name| match std::fs::read(dir.join(name)) {
            Ok(bytes) => {
                let version = FontRef::from_index(&bytes, 0)
                    .and_then(|f| f.localized_strings().find_by_id(StringId::Version, None).map(|s| s.to_string()))
                    .unwrap_or_else(|| "unknown".into());
                format!("{name} size={} version={version}", bytes.len())
            }
            Err(_) => format!("{name} missing"),
        })
        .collect()
}

fn fonts_file() -> PathBuf {
    golden_dir().join("fonts.txt")
}

/// None when the text goldens can run; the reason when they cannot.
fn text_skip_reason() -> Option<String> {
    let now = current_fonts();
    if updating() {
        let mut body = String::from("# Font files the text goldens were rendered with (C:\\Windows\\Fonts).\n");
        for l in &now {
            body.push_str(l);
            body.push('\n');
        }
        std::fs::create_dir_all(golden_dir()).unwrap();
        std::fs::write(fonts_file(), body).unwrap();
        return None;
    }
    let recorded = match std::fs::read_to_string(fonts_file()) {
        Ok(t) => t,
        Err(e) => return Some(format!("tests/golden/fonts.txt unreadable: {e}")),
    };
    let recorded: Vec<&str> = recorded.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
    let differs: Vec<String> = now.iter().filter(|l| !recorded.contains(&l.as_str())).cloned().collect();
    if differs.is_empty() {
        None
    } else {
        Some(format!("font files differ from the recorded ones: {}", differs.join("; ")))
    }
}

fn read_png(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = png::Decoder::new(std::io::BufReader::new(file)).read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    Some((info.width, info.height, buf))
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

fn straight_rgba(c: &Canvas) -> Vec<u8> {
    let mut out = Vec::with_capacity(c.data.len());
    for y in 0..c.height {
        for x in 0..c.width {
            out.extend_from_slice(&c.rgba_at(x, y));
        }
    }
    out
}

fn render_scene(scene: &Scene) -> Canvas {
    let mut r = renderer();
    install_images(&mut r, scene);
    render_offscreen(&mut r, &scene.list).expect("render")
}

fn check(scene: Scene) {
    if scene.text
        && let Some(reason) = text_skip_reason()
    {
        eprintln!("SKIPPED golden '{}': {reason}", scene.name);
        return;
    }
    let canvas = render_scene(&scene);
    let actual = straight_rgba(&canvas);
    let path = golden_dir().join(format!("{}.png", scene.name));
    if updating() {
        write_png(&path, canvas.width, canvas.height, &actual);
        eprintln!("updated {}", path.display());
        return;
    }
    let Some((w, h, want)) = read_png(&path) else {
        panic!("golden {} is missing or unreadable (run with RSB_UPDATE_GOLDEN=1 to create it)", path.display());
    };
    assert_eq!((w, h), (canvas.width, canvas.height), "golden {} has another size", scene.name);
    let mut within = 0usize;
    let mut worst = 0i32;
    let mut diff = vec![0u8; actual.len()];
    for (i, (a, b)) in actual.as_chunks::<4>().0.iter().zip(want.as_chunks::<4>().0).enumerate() {
        let d = (0..4).map(|c| (a[c] as i32 - b[c] as i32).abs()).max().unwrap();
        worst = worst.max(d);
        if d <= CHANNEL_TOLERANCE {
            within += 1;
        }
        let v = (d * 4).min(255) as u8;
        diff[i * 4..i * 4 + 4].copy_from_slice(&[v, 0, 255 - v, 255]);
    }
    let total = actual.len() / 4;
    let share = within as f64 / total as f64;
    if share < MIN_PIXELS_WITHIN || worst > MAX_PIXEL_DIFF {
        let dir = failures_dir();
        write_png(&dir.join(format!("{}-actual.png", scene.name)), canvas.width, canvas.height, &actual);
        write_png(&dir.join(format!("{}-diff.png", scene.name)), canvas.width, canvas.height, &diff);
        panic!(
            "golden '{}': {:.3} % of pixels within {CHANNEL_TOLERANCE} (need {:.1} %), worst channel difference {worst} (limit {MAX_PIXEL_DIFF}); images in {}",
            scene.name,
            share * 100.0,
            MIN_PIXELS_WITHIN * 100.0,
            dir.display()
        );
    }
}

macro_rules! golden {
    ($test:ident, $scene:ident) => {
        #[test]
        fn $test() {
            check($scene());
        }
    };
}

golden!(golden_shapes, scene_shapes);
golden!(golden_borders, scene_borders);
golden!(golden_gradients, scene_gradients);
golden!(golden_shadows, scene_shadows);
golden!(golden_clips, scene_clips);
golden!(golden_opacity, scene_opacity);
golden!(golden_images, scene_images);
golden!(golden_text_latin, scene_text_latin);
golden!(golden_text_light, scene_text_light);
golden!(golden_text_arabic, scene_text_arabic);
golden!(golden_text_ligature, scene_text_ligature);
golden!(golden_text_spacing, scene_text_spacing);
golden!(golden_text_tabular, scene_text_tabular);
golden!(golden_emoji, scene_emoji);
golden!(golden_icons, scene_icons);
golden!(golden_paths, scene_paths);
golden!(golden_chevrons, scene_chevrons);
golden!(golden_svg, scene_svg);
golden!(golden_mono, scene_mono);

/// The reference images stay small.
#[test]
fn golden_files_are_under_two_megabytes() {
    let total: u64 = std::fs::read_dir(golden_dir())
        .map(|d| d.filter_map(|e| e.ok()).filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum())
        .unwrap_or(0);
    assert!(total <= 2 * 1024 * 1024, "golden files total {total} bytes");
}

/// The text goldens only run when the recorded font versions are present;
/// this prints which case this PC is in.
#[test]
fn golden_font_versions_report() {
    match text_skip_reason() {
        None => eprintln!("text goldens: font files match the recorded versions"),
        Some(r) => eprintln!("text goldens: skipped, {r}"),
    }
}

//! Preset previews: drawn on demand by the render path, kept in a cache
//! folder, and drawn again only when something the picture depends on
//! changed. Nothing here opens a window or touches the user's own folders:
//! every path is under the test's scratch folder.

use std::path::{Path, PathBuf};
use std::time::Instant;

use rsb_app::preview::{self, Preview, PreviewOpts};
use rsb_core::preset::Scheme;

const PRESETS: [&str; 4] = ["default", "fluent", "glass", "astur-classic"];

fn scratch(tag: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("preview-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the scratch folder can be made");
    dir
}

fn opts(dir: &Path, mine: bool) -> PreviewOpts {
    PreviewOpts {
        config_dir: dir.join("cfg"),
        cache_dir: Some(dir.join("cache")),
        themes: vec![Scheme::Dark, Scheme::Light],
        width: 1920,
        scale: 1.0,
        mine,
        zones_from_preset: false,
    }
}

fn names() -> Vec<String> {
    PRESETS.iter().map(|p| p.to_string()).collect()
}

fn image(p: &Preview) -> rsb_app::pngread::Picture {
    rsb_app::pngread::read(&p.path).expect("a PNG")
}

fn has_pixel(img: &rsb_app::pngread::Picture, rgb: [u8; 3]) -> bool {
    img.rgba.as_chunks::<4>().0.iter().any(|px| px[0] == rgb[0] && px[1] == rgb[1] && px[2] == rgb[2] && px[3] == 255)
}

#[test]
fn eight_previews_are_drawn_once_then_read_from_the_cache() {
    let dir = scratch("all");
    std::fs::create_dir_all(dir.join("cfg")).unwrap();
    let t = Instant::now();
    let cold = preview::render(&names(), &opts(&dir, false)).expect("the previews draw");
    let cold_ms = t.elapsed().as_millis();
    assert_eq!(cold.len(), 8);
    assert!(cold.iter().all(|p| !p.cached && !p.mine && p.scale == 1.0), "{cold:?}");
    let t = Instant::now();
    let warm = preview::render(&names(), &opts(&dir, false)).expect("the cache answers");
    let warm_ms = t.elapsed().as_millis();
    eprintln!("previews, rdp: cold {cold_ms} ms for 8, warm {warm_ms} ms for 8");
    assert!(warm.iter().all(|p| p.cached), "{warm:?}");
    assert_eq!(cold.iter().map(|p| &p.path).collect::<Vec<_>>(), warm.iter().map(|p| &p.path).collect::<Vec<_>>());
    assert!(warm_ms < 100, "a warm read took {warm_ms} ms");
    assert!(cold_ms < 2000, "a cold run took {cold_ms} ms");

    let mut sizes = Vec::new();
    let mut seen = Vec::new();
    for (c, w) in cold.iter().zip(&warm) {
        let img = image(c);
        assert_eq!((img.width, img.height), (c.width, c.height), "{} {}", c.name, c.theme.name());
        assert_eq!((w.width, w.height), (c.width, c.height), "the cache reports the same size");
        assert_eq!(img.width, 1920);
        assert!(img.height >= 28 && img.height <= 128, "{}: {} px high", c.name, img.height);
        let colours: std::collections::HashSet<[u8; 4]> = img.rgba.as_chunks::<4>().0.iter().copied().collect();
        assert!(colours.len() > 8, "{} {} looks blank: {} colours", c.name, c.theme.name(), colours.len());
        let file = c.path.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            file.starts_with(&format!("{}-{}-1920x100-", c.name, c.theme.name())) && file.ends_with(".png"),
            "{file}"
        );
        sizes.push(std::fs::metadata(&c.path).unwrap().len());
        assert!(!seen.contains(&img.rgba), "{} {} is the same picture as another", c.name, c.theme.name());
        seen.push(img.rgba);
    }
    // Eight files and nothing else (no temp files left behind).
    assert_eq!(std::fs::read_dir(dir.join("cache")).unwrap().count(), 8);
    eprintln!("preview PNG bytes: {sizes:?}");
}

#[test]
fn a_changed_scale_is_its_own_file() {
    let dir = scratch("size");
    std::fs::create_dir_all(dir.join("cfg")).unwrap();
    let one = vec!["glass".to_string()];
    let a = preview::render(&one, &PreviewOpts { themes: vec![Scheme::Dark], ..opts(&dir, false) }).unwrap();
    let b =
        preview::render(&one, &PreviewOpts { themes: vec![Scheme::Dark], scale: 2.0, ..opts(&dir, false) }).unwrap();
    assert_ne!(a[0].path, b[0].path);
    // The width is the picture's own, in pixels; the scale makes the bar taller.
    assert!(!b[0].cached && b[0].width == 1920 && b[0].height == a[0].height * 2, "{a:?} {b:?}");
    assert!(a[0].path.exists(), "a different size does not remove the first picture");
}

#[test]
fn mine_follows_the_users_files_and_the_shipped_pictures_stay() {
    let dir = scratch("mine");
    let cfg = dir.join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    let fluent = vec!["fluent".to_string()];
    let only_dark = |mine| PreviewOpts { themes: vec![Scheme::Dark], ..opts(&dir, mine) };

    let shipped = preview::render(&fluent, &only_dark(false)).unwrap();
    std::fs::write(cfg.join("style.css"), ":root { --accent: #ff0000; --accent-fg: #ffffff; }\n").unwrap();
    std::fs::write(cfg.join("bar.toml"), "config_version = 1\n[[bar]]\nheight = 40\n").unwrap();
    let mine = preview::render(&fluent, &only_dark(true)).unwrap();
    assert!(mine[0].mine && !mine[0].cached);
    assert!(mine[0].path.file_name().unwrap().to_string_lossy().starts_with("fluent-mine-dark-1920x100-"));
    let (s, m) = (image(&shipped[0]), image(&mine[0]));
    assert_eq!(m.height, 40, "the user's own height shows");
    assert_ne!(s.height, 40);
    assert!(!has_pixel(&s, [255, 0, 0]), "the shipped picture has no user colour");
    assert!(has_pixel(&m, [255, 0, 0]), "the user's accent shows on the focused workspace");

    // Same files: a hit. A changed style.css: a new file, and the old one goes.
    assert!(preview::render(&fluent, &only_dark(true)).unwrap()[0].cached);
    std::fs::write(cfg.join("style.css"), ":root { --accent: #00ff00; --accent-fg: #000000; }\n").unwrap();
    let again = preview::render(&fluent, &only_dark(true)).unwrap();
    assert!(!again[0].cached);
    assert_ne!(again[0].path, mine[0].path);
    assert!(!mine[0].path.exists(), "the older picture of the same name and size was removed");
    assert!(has_pixel(&image(&again[0]), [0, 255, 0]));
    // The shipped picture was not touched by either run.
    assert!(shipped[0].path.exists());
    assert!(preview::render(&fluent, &only_dark(false)).unwrap()[0].cached);

    // A widget script of the user's is drawn with, so changing one redraws.
    let before = preview::render(&fluent, &only_dark(true)).unwrap();
    assert!(before[0].cached);
    std::fs::create_dir_all(cfg.join("widgets")).unwrap();
    std::fs::write(
        cfg.join("widgets").join("extra.luau"),
        "return {}
",
    )
    .unwrap();
    let with_widget = preview::render(&fluent, &only_dark(true)).unwrap();
    assert!(!with_widget[0].cached, "a new widget file is a new picture");
    assert!(preview::render(&fluent, &only_dark(true)).unwrap()[0].cached);
    std::fs::write(
        cfg.join("widgets").join("extra.luau"),
        "return {} -- edited
",
    )
    .unwrap();
    assert!(!preview::render(&fluent, &only_dark(true)).unwrap()[0].cached, "an edit to it is too");
    assert!(preview::render(&fluent, &only_dark(false)).unwrap()[0].cached, "the shipped picture ignores it");
}

#[test]
fn a_cut_short_cache_file_is_drawn_again() {
    let dir = scratch("cut");
    std::fs::create_dir_all(dir.join("cfg")).unwrap();
    let one = vec!["astur-classic".to_string()];
    let o = PreviewOpts { themes: vec![Scheme::Light], ..opts(&dir, false) };
    let first = preview::render(&one, &o).unwrap();
    let bytes = std::fs::read(&first[0].path).unwrap();
    std::fs::write(&first[0].path, &bytes[..bytes.len() / 2]).unwrap();
    let second = preview::render(&one, &o).unwrap();
    assert!(!second[0].cached, "half a file is not a hit");
    assert_eq!(std::fs::read(&second[0].path).unwrap(), bytes, "the same picture comes out again");
}

#[test]
fn a_whole_picture_of_the_wrong_size_is_not_a_cache_hit() {
    let dir = scratch("planted");
    std::fs::create_dir_all(dir.join("cfg")).unwrap();
    let one = vec!["astur-classic".to_string()];
    let o = PreviewOpts { themes: vec![Scheme::Light], ..opts(&dir, false) };
    let first = preview::render(&one, &o).unwrap();
    let bytes = std::fs::read(&first[0].path).unwrap();
    // A well-formed 1x1 PNG under the cache name: signature, IHDR, an empty data chunk, IEND.
    let mut planted = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    planted.extend([0, 0, 0, 13]);
    planted.extend(b"IHDR");
    planted.extend([0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0, 0, 0, 0, 0]);
    planted.extend([0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82]);
    std::fs::write(&first[0].path, &planted).unwrap();
    let second = preview::render(&one, &o).unwrap();
    assert!(!second[0].cached, "a picture that is not as wide as asked is not a hit");
    assert_eq!(second[0].width, 1920);
    assert_eq!(std::fs::read(&second[0].path).unwrap(), bytes, "the real picture is back");
}

#[test]
fn zones_from_the_preset_are_their_own_picture_and_a_broken_file_is_refused() {
    let dir = scratch("zones");
    std::fs::create_dir_all(dir.join("cfg")).unwrap();
    std::fs::write(
        dir.join("cfg").join("bar.toml"),
        "config_version = 1\n\n[[bar]]\nname = \"main\"\nleft = [\"clock\"]\ncenter = []\nright = []\n",
    )
    .unwrap();
    let one = vec!["glass".to_string()];
    let mine = PreviewOpts { themes: vec![Scheme::Dark], ..opts(&dir, true) };
    let kept = preview::render(&one, &mine).unwrap();
    let preset_zones = preview::render(&one, &PreviewOpts { zones_from_preset: true, ..mine.clone() }).unwrap();
    assert_ne!(kept[0].path, preset_zones[0].path, "its own file");
    assert_ne!(
        std::fs::read(&kept[0].path).unwrap(),
        std::fs::read(&preset_zones[0].path).unwrap(),
        "the preset's own zones draw something other than a lone clock"
    );
    assert!(preset_zones[0].zones_from_preset && !kept[0].zones_from_preset);
    // Both stay in the cache: neither removed the other as an older picture.
    assert!(kept[0].path.exists() && preset_zones[0].path.exists());
    // A style file that does not parse is refused, not drawn as an error plate.
    std::fs::write(dir.join("cfg").join("style.css"), "bar { color: \n").unwrap();
    let e = preview::render(&one, &mine).unwrap_err();
    assert_eq!(e.code, rsb_core::contract::ErrorCode::FileBroken, "{e:?}");
    assert_eq!(e.file, Some(rsb_core::contract::ConfigFile::StyleCss));
    let open = e.fix.as_ref().and_then(|f| f["open"].as_str().map(str::to_string)).expect("a fix that opens the file");
    assert!(open.ends_with("style.css"), "{open}");
    assert!(!open.starts_with(r"\\?\"), "{open}");
}

#[test]
fn a_broken_file_that_style_css_pulls_in_is_refused_with_its_own_place_to_open() {
    let dir = scratch("broken-import");
    std::fs::create_dir_all(dir.join("cfg")).unwrap();
    std::fs::write(dir.join("cfg").join("bar.toml"), "config_version = 1\n").unwrap();
    std::fs::write(dir.join("cfg").join("style.css"), "@import \"extra.css\";\n").unwrap();
    std::fs::write(dir.join("cfg").join("extra.css"), "bar { color: \n").unwrap();
    let mine = PreviewOpts { themes: vec![Scheme::Dark], ..opts(&dir, true) };
    let e = preview::render(&["glass".to_string()], &mine).unwrap_err();
    assert_eq!(e.code, rsb_core::contract::ErrorCode::FileBroken, "{e:?}");
    assert!(e.at.as_deref().is_some_and(|at| at.starts_with("extra.css:")), "{e:?}");
    assert_eq!(e.file, Some(rsb_core::contract::ConfigFile::StyleCss), "the file the person changes is style.css");
    let open = e.fix.as_ref().and_then(|f| f["open"].as_str().map(str::to_string)).expect("a fix that opens the file");
    assert!(open.ends_with("extra.css"), "{open}");
    assert!(e.next.as_deref().is_some_and(|n| n.contains("extra.css")), "{e:?}");
}

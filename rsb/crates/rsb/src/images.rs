//! Image loading off the UI thread. A widget's `rsb.image` path is opened
//! with `files::open_permitted` against the widget's own grant (so a link or
//! junction cannot lead outside the permitted places), read with a size
//! cap, decoded, and handed to the UI, which tells the scene the size before
//! the renderer gets the pixels.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use rsb_core::element::{Element, ElementKind, IconRef};
use rsb_core::io::{IoError, IoGrant};
use rsb_core::pixels::IMAGE_KEY_PREFIX;
use rsb_core::wake::Wake;
use rsb_render::{IMAGE_MAX_FILE_BYTES, Image, ParsedSvg, SVG_MAX_FILE_BYTES, SvgSource, decode_png, parse_svg};
use rsb_services::files::open_permitted;

use crate::msg::UiMsg;

/// Opens `path` against the widget's grant and reads at most `cap` bytes;
/// one more means the file is too large. Errors never hold the path.
fn read_permitted(path: &str, grant: &IoGrant, cap: usize, what: &str) -> Result<Vec<u8>, String> {
    let file = open_permitted(Path::new(path), grant).map_err(|e| match e {
        // The grant's own wording names the file; logs never hold paths.
        IoError::NotPermitted(_) => "not permitted: the widget may not read that file".to_string(),
        other => other.to_string(),
    })?;
    let mut bytes = Vec::new();
    file.take(cap as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > cap {
        return Err(format!("the {what} is larger than {} MB", cap >> 20));
    }
    Ok(bytes)
}

/// Opens, reads (capped) and decodes one image. Errors never hold the path.
pub fn load(path: &str, grant: &IoGrant) -> Result<Image, String> {
    let bytes = read_permitted(path, grant, IMAGE_MAX_FILE_BYTES, "image")?;
    decode_png(&bytes).map_err(|e| e.to_string())
}

/// Opens, reads (at most 1 MiB) and parses one `.svg` file with the user
/// limits (no files, no scripts, a cap on elements). Parsing is the costly
/// part, so this runs on the loader thread; the UI thread only stores the
/// result.
pub fn load_svg(path: &str, grant: &IoGrant) -> Result<ParsedSvg, String> {
    let bytes = read_permitted(path, grant, SVG_MAX_FILE_BYTES, "svg")?;
    parse_svg(&bytes, SvgSource::User).map_err(|e| e.to_string())
}

/// What a path names, by its extension.
pub fn is_svg_path(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".svg")
}

/// What one request loads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Png,
    Svg,
}

/// Every file a tree asks the loader for: PNG images and the user's own
/// `.svg` icons and images. Bundled icon names and the keys services
/// publish pictures under are not files.
pub fn files_of(e: &Element, out: &mut Vec<(Arc<str>, Kind)>) {
    if e.kind == ElementKind::Image
        && let Some(img) = &e.image
        && !img.path.starts_with(IMAGE_KEY_PREFIX)
    {
        out.push((img.path.clone(), if is_svg_path(&img.path) { Kind::Svg } else { Kind::Png }));
    }
    if let Some(IconRef::Svg(path)) = &e.icon
        && path.contains(['\\', '/', ':'])
    {
        out.push((path.clone(), Kind::Svg));
    }
    for child in &e.children {
        files_of(child, out);
    }
}

/// How long a failed image is left alone before a tree that still wants it
/// asks again (the file may have appeared, or another widget may be allowed
/// to read it).
const RETRY_AFTER: Duration = Duration::from_secs(5);

/// Which images were already asked for. A path is asked for once while it is
/// loading or loaded; one that failed is asked for again after `RETRY_AFTER`.
#[derive(Default)]
pub struct ImageRequests {
    asked: HashSet<String>,
    failed_at: HashMap<String, Instant>,
}

impl ImageRequests {
    /// True when `path` should be requested now.
    pub fn should_request(&mut self, path: &str, now: Instant) -> bool {
        if self.asked.contains(path) {
            return false;
        }
        if let Some(at) = self.failed_at.get(path)
            && now.saturating_duration_since(*at) < RETRY_AFTER
        {
            return false;
        }
        self.failed_at.remove(path);
        self.asked.insert(path.to_string());
        true
    }

    /// The load of `path` failed.
    pub fn failed(&mut self, path: &str, now: Instant) {
        self.asked.remove(path);
        self.failed_at.insert(path.to_string(), now);
    }
}

/// One worker thread, started with the first request and idle (blocked on
/// its channel) otherwise.
pub struct ImageLoader {
    tx: Sender<(Arc<str>, Arc<IoGrant>, Kind)>,
}

impl ImageLoader {
    pub fn start(ui_tx: Sender<UiMsg>, waker: Arc<dyn Wake>) -> ImageLoader {
        let (tx, rx) = channel::<(Arc<str>, Arc<IoGrant>, Kind)>();
        let _ = std::thread::Builder::new().name("rsb-images".into()).spawn(move || {
            while let Ok((key, grant, kind)) = rx.recv() {
                let msg = match kind {
                    Kind::Png => UiMsg::Image { result: load(&key, &grant), key },
                    Kind::Svg => UiMsg::Svg { result: load_svg(&key, &grant), key },
                };
                if ui_tx.send(msg).is_err() {
                    break;
                }
                waker.wake();
            }
        });
        ImageLoader { tx }
    }

    pub fn request(&self, key: Arc<str>, grant: Arc<IoGrant>, kind: Kind) {
        let _ = self.tx.send((key, grant, kind));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use rsb_core::permissions::Permissions;
    use std::path::PathBuf;

    const DOT: &str =
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\"><circle cx=\"12\" cy=\"12\" r=\"8\"/></svg>";

    /// The folder as an absolute path with no `..` and no verbatim prefix,
    /// the form the grant compares against.
    fn plain(dir: &TempDir) -> PathBuf {
        let real = std::fs::canonicalize(dir.path()).unwrap();
        PathBuf::from(real.to_string_lossy().trim_start_matches(r"\\?\"))
    }

    /// A grant that reads only the widget's own folder.
    fn grant_for(dir: &Path) -> IoGrant {
        IoGrant {
            widget: "t".into(),
            permissions: Permissions::default(),
            widget_dir: dir.to_path_buf(),
            env: crate::paths::path_env(),
        }
    }

    #[test]
    fn a_user_svg_is_read_under_the_grant_and_parsed() {
        let dir = TempDir::new("svg");
        let path = plain(&dir).join("dot.svg");
        std::fs::write(&path, DOT).unwrap();
        let svg = load_svg(path.to_str().unwrap(), &grant_for(&plain(&dir))).expect("a small svg loads");
        assert_eq!(svg.size(), (24.0, 24.0));
    }

    #[test]
    fn a_user_svg_outside_the_grant_or_over_one_megabyte_is_refused_without_its_path() {
        let (inside, outside) = (TempDir::new("svg-in"), TempDir::new("svg-out"));
        let elsewhere = plain(&outside).join("dot.svg");
        std::fs::write(&elsewhere, DOT).unwrap();
        let err = load_svg(elsewhere.to_str().unwrap(), &grant_for(&plain(&inside))).unwrap_err();
        assert_eq!(err, "not permitted: the widget may not read that file");
        let big = plain(&inside).join("big.svg");
        std::fs::write(&big, format!("{DOT}{}", " ".repeat(SVG_MAX_FILE_BYTES))).unwrap();
        let err = load_svg(big.to_str().unwrap(), &grant_for(&plain(&inside))).unwrap_err();
        assert_eq!(err, "the svg is larger than 1 MB");
        let bad = plain(&inside).join("bad.svg");
        std::fs::write(&bad, "<svg").unwrap();
        assert!(load_svg(bad.to_str().unwrap(), &grant_for(&plain(&inside))).is_err());
    }

    #[test]
    fn the_loader_thread_parses_an_svg_and_sends_it_to_the_ui_inbox() {
        let dir = TempDir::new("svg-thread");
        let path = plain(&dir).join("dot.svg");
        std::fs::write(&path, DOT).unwrap();
        let (tx, rx) = channel();
        let loader = ImageLoader::start(tx, Arc::new(rsb_core::wake::NoWake));
        loader.request(Arc::from(path.to_str().unwrap()), Arc::new(grant_for(&plain(&dir))), Kind::Svg);
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(UiMsg::Svg { result, .. }) => assert!(result.is_ok()),
            _ => panic!("the loader did not answer with a parsed svg"),
        }
    }

    #[test]
    fn a_tree_asks_for_png_and_user_svg_files_only() {
        use rsb_core::element::ImageRef;
        let image = |path: &str| {
            let mut e = Element::new(ElementKind::Image);
            e.image = Some(ImageRef { path: Arc::from(path), width: None, height: None });
            e
        };
        let icon = |svg: &str| {
            let mut e = Element::new(ElementKind::Icon);
            e.icon = Some(IconRef::Svg(Arc::from(svg)));
            e
        };
        let mut root = Element::new(ElementKind::Box);
        root.children = vec![
            image("C:\\w\\a.png"),
            image("C:\\w\\b.svg"),
            image("rsb-image:media/3"),
            icon("power"),
            icon("C:\\w\\mine.svg"),
        ];
        let mut found = Vec::new();
        files_of(&root, &mut found);
        let found: Vec<(&str, Kind)> = found.iter().map(|(p, k)| (p.as_ref(), *k)).collect();
        assert_eq!(found, [("C:\\w\\a.png", Kind::Png), ("C:\\w\\b.svg", Kind::Svg), ("C:\\w\\mine.svg", Kind::Svg)]);
    }

    #[test]
    fn a_path_is_an_svg_by_its_extension_in_any_case() {
        assert!(is_svg_path("C:\\a\\b.SVG") && is_svg_path("x.svg"));
        assert!(!is_svg_path("x.png") && !is_svg_path("svg"));
    }

    #[test]
    fn an_image_is_asked_for_once_and_a_failed_one_again_after_a_pause() {
        let mut r = ImageRequests::default();
        let t0 = Instant::now();
        assert!(r.should_request("a.png", t0));
        assert!(!r.should_request("a.png", t0), "loading or loaded: not again");
        assert!(r.should_request("b.png", t0), "another path is independent");

        r.failed("a.png", t0);
        assert!(!r.should_request("a.png", t0 + Duration::from_secs(1)), "too soon to ask again");
        assert!(r.should_request("a.png", t0 + RETRY_AFTER), "asked again after the pause");
        assert!(!r.should_request("a.png", t0 + RETRY_AFTER), "and once while that load runs");
    }
}

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

use rsb_core::io::IoGrant;
use rsb_core::wake::Wake;
use rsb_render::{IMAGE_MAX_FILE_BYTES, Image, decode_png};
use rsb_services::files::open_permitted;

use crate::msg::UiMsg;

/// Opens, reads (capped) and decodes one image. Errors never hold the path.
pub fn load(path: &str, grant: &IoGrant) -> Result<Image, String> {
    let file = open_permitted(Path::new(path), grant).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(IMAGE_MAX_FILE_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > IMAGE_MAX_FILE_BYTES {
        return Err(format!("the image is larger than {} MB", IMAGE_MAX_FILE_BYTES >> 20));
    }
    decode_png(&bytes).map_err(|e| e.to_string())
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
    tx: Sender<(Arc<str>, Arc<IoGrant>)>,
}

impl ImageLoader {
    pub fn start(ui_tx: Sender<UiMsg>, waker: Arc<dyn Wake>) -> ImageLoader {
        let (tx, rx) = channel::<(Arc<str>, Arc<IoGrant>)>();
        let _ = std::thread::Builder::new().name("rsb-images".into()).spawn(move || {
            while let Ok((key, grant)) = rx.recv() {
                let result = load(&key, &grant);
                if ui_tx.send(UiMsg::Image { key, result }).is_err() {
                    break;
                }
                waker.wake();
            }
        });
        ImageLoader { tx }
    }

    pub fn request(&self, key: Arc<str>, grant: Arc<IoGrant>) {
        let _ = self.tx.send((key, grant));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

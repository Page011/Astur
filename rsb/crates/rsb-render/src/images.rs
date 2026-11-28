//! Decoded image cache, bounded by bytes; least recently drawn go first.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{IMAGE_CACHE_MAX_BYTES, IMAGE_MAX_SIDE, Image};

struct Entry {
    image: Image,
    used: u64,
}

#[derive(Default)]
pub(crate) struct ImageCache {
    map: HashMap<Arc<str>, Entry>,
    bytes: usize,
    tick: u64,
}

impl ImageCache {
    /// Stores an image. A malformed or oversized one is refused (and logged
    /// once per call); room is made by dropping the least recently drawn.
    pub(crate) fn set(&mut self, key: Arc<str>, image: Image) {
        let need = image.width as usize * image.height as usize * 4;
        let ok = image.width > 0
            && image.height > 0
            && image.width <= IMAGE_MAX_SIDE
            && image.height <= IMAGE_MAX_SIDE
            && image.data.len() == need;
        if !ok || need > IMAGE_CACHE_MAX_BYTES {
            log::warn!("image refused: {}x{} with {} bytes", image.width, image.height, image.data.len());
            return;
        }
        if let Some(old) = self.map.remove(&key) {
            self.bytes -= old.image.data.len();
        }
        while self.bytes + need > IMAGE_CACHE_MAX_BYTES {
            let Some(oldest) = self.map.iter().min_by_key(|(_, e)| e.used).map(|(k, _)| k.clone()) else { break };
            if let Some(e) = self.map.remove(&oldest) {
                self.bytes -= e.image.data.len();
            }
        }
        self.tick += 1;
        self.bytes += need;
        self.map.insert(key, Entry { image, used: self.tick });
    }

    pub(crate) fn has(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    /// The image for drawing; marks it as used.
    pub(crate) fn get(&mut self, key: &str) -> Option<&Image> {
        self.tick += 1;
        let tick = self.tick;
        self.map.get_mut(key).map(|e| {
            e.used = tick;
            &e.image
        })
    }

    /// Drops the image stored under exactly `key`; true when there was one.
    pub(crate) fn remove(&mut self, key: &str) -> bool {
        let Some(e) = self.map.remove(key) else { return false };
        self.bytes -= e.image.data.len();
        true
    }

    /// Drops every image whose key starts with `prefix`; returns how many.
    pub(crate) fn remove_prefix(&mut self, prefix: &str) -> usize {
        let gone: Vec<Arc<str>> = self.map.keys().filter(|k| k.starts_with(prefix)).cloned().collect();
        for k in &gone {
            if let Some(e) = self.map.remove(k) {
                self.bytes -= e.image.data.len();
            }
        }
        gone.len()
    }

    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }

    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(side: u32) -> Image {
        Image { width: side, height: side, data: vec![255; side as usize * side as usize * 4] }
    }

    #[test]
    fn stays_under_the_cap_and_drops_the_oldest() {
        let mut c = ImageCache::default();
        // 1000 x 1000 x 4 = ~3.8 MiB each: four fit in 16 MiB, the fifth pushes the first out
        for i in 0..5 {
            c.set(format!("k{i}").into(), img(1000));
            assert!(c.bytes() <= IMAGE_CACHE_MAX_BYTES);
        }
        assert!(!c.has("k0"));
        assert!(c.has("k4"));
        assert_eq!(c.len(), 4);
    }

    #[test]
    fn drawing_refreshes_an_image() {
        let mut c = ImageCache::default();
        for i in 0..4 {
            c.set(format!("k{i}").into(), img(1000));
        }
        assert!(c.get("k0").is_some());
        c.set("new".into(), img(1000));
        assert!(c.has("k0"), "k0 was drawn last, k1 is the oldest");
        assert!(!c.has("k1"));
    }

    #[test]
    fn malformed_images_are_refused() {
        let mut c = ImageCache::default();
        c.set("a".into(), Image { width: 2, height: 2, data: vec![0; 3] });
        c.set("b".into(), Image { width: 0, height: 2, data: vec![] });
        c.set(
            "c".into(),
            Image { width: IMAGE_MAX_SIDE + 1, height: 1, data: vec![0; (IMAGE_MAX_SIDE as usize + 1) * 4] },
        );
        assert_eq!(c.len(), 0);
        assert_eq!(c.bytes(), 0);
    }

    #[test]
    fn removing_one_key_leaves_a_longer_key_that_starts_with_it() {
        let mut c = ImageCache::default();
        c.set("rsb-image:media/1".into(), img(10));
        c.set("rsb-image:media/10".into(), img(10));
        assert!(c.remove("rsb-image:media/1"));
        assert!(!c.remove("rsb-image:media/1"), "already gone");
        assert!(c.has("rsb-image:media/10"));
        assert_eq!((c.len(), c.bytes()), (1, 10 * 10 * 4));
    }

    #[test]
    fn removing_by_prefix_frees_only_those_keys() {
        let mut c = ImageCache::default();
        c.set("rsb-image:apps/1".into(), img(10));
        c.set("rsb-image:apps/2".into(), img(10));
        c.set("rsb-image:media/1".into(), img(10));
        assert_eq!(c.remove_prefix("rsb-image:apps/"), 2);
        assert_eq!((c.len(), c.bytes()), (1, 10 * 10 * 4));
        assert!(c.has("rsb-image:media/1"));
    }

    #[test]
    fn replacing_a_key_does_not_leak_bytes() {
        let mut c = ImageCache::default();
        c.set("a".into(), img(10));
        c.set("a".into(), img(20));
        assert_eq!(c.bytes(), 20 * 20 * 4);
        assert_eq!(c.len(), 1);
    }
}

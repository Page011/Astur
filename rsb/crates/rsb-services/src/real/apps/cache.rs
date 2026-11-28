//! The apps service's icon memory: at most `MAX_ICONS` pictures and `MAX_BYTES`, and gone a minute
//! after the service went idle. Time is passed in, so the release is tested without waiting.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use rsb_core::pixels::RgbaImage;

pub(crate) const MAX_ICONS: usize = 512;
pub(crate) const MAX_BYTES: usize = 8 << 20;
pub(crate) const RELEASE_AFTER: Duration = Duration::from_secs(60);

/// `rsb-image:apps/<16 hex digits of a 64-bit FNV-1a hash of the app id>`: stable between runs, so
/// a restart finds its icons again, and short enough for a table key.
pub fn icon_key(id: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in id.bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{}apps/{hash:016x}", rsb_core::pixels::IMAGE_KEY_PREFIX)
}

#[derive(Default)]
pub(crate) struct IconCache {
    icons: HashMap<String, RgbaImage>,
    bytes: usize,
    idle_since: Option<Instant>,
}

impl IconCache {
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.icons.len()
    }

    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    pub(crate) fn contains(&self, key: &str) -> bool {
        self.icons.contains_key(key)
    }

    pub(crate) fn get(&self, key: &str) -> Option<&RgbaImage> {
        self.icons.get(key)
    }

    /// Keeps the picture unless that would pass either cap (then nothing is stored and the caller
    /// stops extracting). A key already held counts as stored.
    pub(crate) fn insert(&mut self, key: String, image: RgbaImage) -> bool {
        if self.icons.contains_key(&key) {
            return true;
        }
        if self.icons.len() >= MAX_ICONS || self.bytes + image.bytes() > MAX_BYTES {
            return false;
        }
        self.bytes += image.bytes();
        self.icons.insert(key, image);
        true
    }

    /// Drops the pictures of apps no longer in the list.
    pub(crate) fn retain_keys(&mut self, keep: &HashSet<String>) {
        self.icons.retain(|k, _| keep.contains(k));
        self.bytes = self.icons.values().map(RgbaImage::bytes).sum();
    }

    pub(crate) fn clear(&mut self) {
        self.icons.clear();
        self.bytes = 0;
    }

    /// The last reader is gone from `now`.
    pub(crate) fn mark_idle(&mut self, now: Instant) {
        self.idle_since = Some(now);
    }

    /// A reader is back: nothing is released.
    pub(crate) fn mark_active(&mut self) {
        self.idle_since = None;
    }

    /// Frees everything once idle for `after`; true when it did.
    pub(crate) fn release_due(&mut self, now: Instant, after: Duration) -> bool {
        match self.idle_since {
            Some(since) if now.saturating_duration_since(since) >= after => {
                self.clear();
                self.idle_since = None;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(side: u32) -> RgbaImage {
        RgbaImage::new(side, side, vec![255; (side * side * 4) as usize]).unwrap()
    }

    #[test]
    fn keys_are_stable_and_inside_the_service_prefix() {
        let k = icon_key("some.app!App");
        assert_eq!(k, icon_key("some.app!App"));
        assert_ne!(k, icon_key("other.app!App"));
        assert!(k.starts_with("rsb-image:apps/") && k.len() == "rsb-image:apps/".len() + 16, "{k}");
    }

    #[test]
    fn at_most_512_icons_are_kept() {
        let mut cache = IconCache::default();
        for i in 0..MAX_ICONS {
            assert!(cache.insert(format!("k{i}"), image(8)));
        }
        assert!(!cache.insert("one-more".into(), image(8)), "the 513th is refused");
        assert_eq!(cache.len(), MAX_ICONS);
        assert!(cache.insert("k0".into(), image(8)), "a key already held is not an overflow");
        assert_eq!(cache.bytes(), MAX_ICONS * 8 * 8 * 4);
    }

    #[test]
    fn at_most_8_mib_are_kept() {
        let mut cache = IconCache::default();
        // 256 x 256 x 4 = 256 KiB: 32 fit in 8 MiB.
        let mut stored = 0;
        for i in 0..64 {
            if cache.insert(format!("k{i}"), image(256)) {
                stored += 1;
            }
        }
        assert_eq!(stored, 32);
        assert_eq!(cache.bytes(), MAX_BYTES);
        assert!(!cache.insert("small".into(), image(1)), "not even a pixel past the cap");
    }

    #[test]
    fn everything_is_freed_60_s_after_the_last_reader_and_not_before() {
        let mut cache = IconCache::default();
        cache.insert("a".into(), image(32));
        let t0 = Instant::now();
        assert!(!cache.release_due(t0, RELEASE_AFTER), "never idle: nothing released");
        cache.mark_idle(t0);
        assert!(!cache.release_due(t0 + Duration::from_secs(59), RELEASE_AFTER));
        assert_eq!(cache.len(), 1);
        assert!(cache.release_due(t0 + Duration::from_secs(60), RELEASE_AFTER));
        assert_eq!((cache.len(), cache.bytes()), (0, 0));
    }

    #[test]
    fn a_reader_coming_back_cancels_the_release() {
        let mut cache = IconCache::default();
        cache.insert("a".into(), image(32));
        let t0 = Instant::now();
        cache.mark_idle(t0);
        cache.mark_active();
        assert!(!cache.release_due(t0 + Duration::from_secs(600), RELEASE_AFTER));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn icons_of_apps_that_left_the_list_are_dropped() {
        let mut cache = IconCache::default();
        cache.insert("a".into(), image(8));
        cache.insert("b".into(), image(8));
        cache.retain_keys(&["b".to_string()].into());
        assert!(!cache.contains("a") && cache.contains("b"));
        assert_eq!(cache.bytes(), 8 * 8 * 4);
    }
}

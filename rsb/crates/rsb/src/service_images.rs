//! Pictures services make (album art, app icons). The hub hands each one to
//! the UI thread, which keeps it in the renderer under its `rsb-image:` key;
//! nothing is written to disk and the script thread never sees the pixels.
//! This module also knows when a key is no longer wanted: the media art key
//! changes with every track, and the app icons go when the launcher closes.

use std::collections::HashSet;
use std::sync::Arc;

use rsb_core::pixels::RgbaImage;
use rsb_core::services::{ServiceEvent, ServiceId, ServiceStatus};
use rsb_render::{Image, Renderer};

/// Album art keys: `rsb-image:media/<n>`, one per track.
pub const MEDIA_ART_PREFIX: &str = "rsb-image:media/";
/// The player's own icon: `rsb-image:media/app-<hash>`; it is not art and
/// outlives a track.
const MEDIA_APP_ICON_PREFIX: &str = "rsb-image:media/app-";
/// Launcher icons: `rsb-image:apps/<hash>`.
pub const APPS_PREFIX: &str = "rsb-image:apps/";

/// Follows `media.art` through the service's values.
#[derive(Debug, Default)]
pub struct ArtWatch {
    current: Option<Arc<str>>,
}

impl ArtWatch {
    /// Feed every service event. Returns the art key that is no longer the
    /// value's `art` (the track changed, or the art went away), once.
    pub fn observe(&mut self, event: &ServiceEvent) -> Option<Arc<str>> {
        if event.service != ServiceId::Media {
            return None;
        }
        let value = event.value.as_ref()?;
        let art = value
            .get("art")
            .and_then(|a| a.as_str())
            .filter(|a| a.starts_with(MEDIA_ART_PREFIX) && !a.starts_with(MEDIA_APP_ICON_PREFIX));
        if art == self.current.as_deref() {
            return None;
        }
        std::mem::replace(&mut self.current, art.map(Arc::from))
    }
}

/// The apps service went off: no widget subscribes to the list any more.
pub fn apps_stopped(event: &ServiceEvent) -> bool {
    event.service == ServiceId::Apps && event.status == ServiceStatus::Off
}

/// A power action failed: the power service reports an error status.
pub fn power_failed(event: &ServiceEvent) -> bool {
    event.service == ServiceId::Power && event.status == ServiceStatus::Error
}

/// The keys the UI thread put in the renderer, so a removal can be exact.
#[derive(Debug, Default)]
pub struct ServiceImages {
    keys: HashSet<Arc<str>>,
}

impl ServiceImages {
    /// Stores `image` under `key`; returns its size for `Scene::set_image_size`.
    pub fn add(&mut self, renderer: &mut Renderer, key: Arc<str>, image: &RgbaImage) -> (u32, u32) {
        let size = (image.width, image.height);
        renderer
            .set_image(Arc::clone(&key), Image { width: image.width, height: image.height, data: image.data.to_vec() });
        self.keys.insert(key);
        size
    }

    /// Drops one key and only that key (`media/1` leaves `media/10` alone).
    /// Returns true when the renderer held it.
    pub fn remove(&mut self, renderer: &mut Renderer, key: &str) -> bool {
        self.keys.remove(key);
        renderer.remove_image(key)
    }

    /// Drops every launcher icon. Returns how many the renderer held.
    pub fn remove_apps(&mut self, renderer: &mut Renderer) -> usize {
        self.keys.retain(|k| !k.starts_with(APPS_PREFIX));
        renderer.remove_images(APPS_PREFIX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsb_core::services::ServiceStatus;
    use rsb_core::value::Value;
    use rsb_render::FontConfig;
    use std::collections::BTreeMap;

    #[test]
    fn only_the_apps_service_going_off_counts_as_the_app_list_stopping() {
        let event = |service, status| ServiceEvent { service, version: 1, status, value: None, error: None };
        assert!(apps_stopped(&event(ServiceId::Apps, ServiceStatus::Off)));
        assert!(!apps_stopped(&event(ServiceId::Apps, ServiceStatus::Ok)));
        assert!(!apps_stopped(&event(ServiceId::Media, ServiceStatus::Off)));
    }

    #[test]
    fn only_an_error_status_of_the_power_service_is_a_failed_action() {
        let event = |service, status| ServiceEvent { service, version: 1, status, value: None, error: None };
        assert!(power_failed(&event(ServiceId::Power, ServiceStatus::Error)));
        assert!(!power_failed(&event(ServiceId::Power, ServiceStatus::Ok)));
        assert!(!power_failed(&event(ServiceId::Audio, ServiceStatus::Error)));
    }

    fn media(art: Option<&str>) -> ServiceEvent {
        let mut map = BTreeMap::new();
        if let Some(a) = art {
            map.insert("art".to_string(), Value::Str(a.to_string()));
        }
        ServiceEvent {
            service: ServiceId::Media,
            version: 1,
            status: ServiceStatus::Ok,
            value: Some(Value::Map(map)),
            error: None,
        }
    }

    fn pixel() -> RgbaImage {
        RgbaImage::new(1, 1, vec![1, 2, 3, 255]).unwrap()
    }

    #[test]
    fn the_old_art_key_is_named_once_when_the_value_changes() {
        let mut w = ArtWatch::default();
        assert_eq!(w.observe(&media(Some("rsb-image:media/1"))), None, "the first art has nothing before it");
        assert_eq!(w.observe(&media(Some("rsb-image:media/1"))), None, "no change");
        assert_eq!(w.observe(&media(Some("rsb-image:media/2"))).as_deref(), Some("rsb-image:media/1"));
        assert_eq!(w.observe(&media(None)).as_deref(), Some("rsb-image:media/2"), "art that goes away is dropped too");
        assert_eq!(w.observe(&media(None)), None);
        assert_eq!(w.observe(&media(Some("rsb-image:media/3"))), None);
    }

    #[test]
    fn other_services_values_without_a_value_and_the_app_icon_are_ignored() {
        let mut w = ArtWatch::default();
        let mut other = media(Some("rsb-image:media/1"));
        other.service = ServiceId::Audio;
        assert_eq!(w.observe(&other), None);
        w.observe(&media(Some("rsb-image:media/1")));
        let mut status_only = media(None);
        status_only.value = None;
        assert_eq!(w.observe(&status_only), None, "a status change keeps the art");
        assert_eq!(
            w.observe(&media(Some("rsb-image:media/app-ff"))).as_deref(),
            Some("rsb-image:media/1"),
            "the player's icon is not art: the track has none now"
        );
        assert_eq!(w.observe(&media(Some("rsb-image:media/2"))), None, "and the icon key was never held as art");
    }

    #[test]
    fn a_dropped_key_leaves_the_renderer_and_a_longer_neighbour_stays() {
        let mut renderer = Renderer::new(FontConfig::default());
        let mut images = ServiceImages::default();
        for k in ["rsb-image:media/1", "rsb-image:media/10"] {
            assert_eq!(images.add(&mut renderer, Arc::from(k), &pixel()), (1, 1));
        }
        assert!(images.remove(&mut renderer, "rsb-image:media/1"));
        assert!(!renderer.has_image("rsb-image:media/1") && renderer.has_image("rsb-image:media/10"));
        assert!(!images.remove(&mut renderer, "rsb-image:media/1"), "already gone");
        assert!(images.remove(&mut renderer, "rsb-image:media/10"));
        assert_eq!(renderer.stats().images, 0);
    }

    #[test]
    fn app_icons_are_removed_by_prefix_and_other_keys_stay() {
        let mut renderer = Renderer::new(FontConfig::default());
        let mut images = ServiceImages::default();
        for k in ["rsb-image:apps/a", "rsb-image:apps/b", "rsb-image:media/7"] {
            images.add(&mut renderer, Arc::from(k), &pixel());
        }
        assert_eq!(images.remove_apps(&mut renderer), 2);
        assert!(renderer.has_image("rsb-image:media/7") && !renderer.has_image("rsb-image:apps/a"));
        assert_eq!(images.remove_apps(&mut renderer), 0);
    }
}

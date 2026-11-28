//! The bundled SVG icons. The scene draws one as `DrawCmd::Svg` keyed by its
//! name (`power`); the renderer keeps the parsed file, which is set the first
//! time a frame asks for it. A bundled icon is a few hundred bytes, so the
//! parse is cheap enough for the UI thread; user files go through the image
//! loader instead.

use std::collections::HashSet;
use std::sync::Arc;

use rsb_core::draw::{DrawCmd, DrawList};
use rsb_core::icons::is_svg_icon_name;
use rsb_render::Renderer;

use crate::assets;

/// Which bundled icons were set, and which could not be (logged once each).
#[derive(Debug, Default)]
pub struct BundledIcons {
    failed: HashSet<String>,
}

impl BundledIcons {
    /// Sets every bundled icon `list` draws that the renderer does not hold
    /// yet. Returns how many were set.
    pub fn ensure(&mut self, renderer: &mut Renderer, list: &DrawList) -> usize {
        let mut set = 0;
        for item in &list.items {
            let DrawCmd::Svg { key, .. } = &item.cmd else { continue };
            if !is_svg_icon_name(key) || renderer.has_svg(key) || self.failed.contains(key.as_ref()) {
                continue;
            }
            let loaded = assets::bytes(&format!("icons/{key}.svg"))
                .ok_or_else(|| "the icon is not embedded".to_string())
                .and_then(|b| renderer.set_svg(Arc::clone(key), b).map(drop).map_err(|e| e.to_string()));
            match loaded {
                Ok(()) => set += 1,
                Err(e) => {
                    log::error!("the bundled icon '{key}' could not be loaded: {e}");
                    self.failed.insert(key.to_string());
                }
            }
        }
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsb_core::draw::DrawItem;
    use rsb_core::element::NodeKey;
    use rsb_core::geom::RectF;
    use rsb_core::icons::SVG_ICON_NAMES;
    use rsb_render::FontConfig;

    fn svg_item(key: &str) -> DrawItem {
        let rect = RectF { x: 0.0, y: 0.0, w: 16.0, h: 16.0 };
        DrawItem { node: NodeKey(1), bounds: rect, cmd: DrawCmd::Svg { rect, key: Arc::from(key), tint: None } }
    }

    #[test]
    fn every_bundled_name_has_an_embedded_file_the_renderer_accepts() {
        let mut renderer = Renderer::new(FontConfig::default());
        let list = DrawList { width: 16, height: 16, items: SVG_ICON_NAMES.iter().map(|n| svg_item(n)).collect() };
        let mut icons = BundledIcons::default();
        assert_eq!(icons.ensure(&mut renderer, &list), SVG_ICON_NAMES.len());
        assert!(icons.failed.is_empty(), "{:?}", icons.failed);
        for name in SVG_ICON_NAMES {
            assert!(renderer.has_svg(name), "{name}");
        }
        assert_eq!(renderer.stats().svgs, SVG_ICON_NAMES.len());
    }

    #[test]
    fn an_icon_is_set_on_first_use_only_and_a_path_or_unknown_name_is_left_alone() {
        let mut renderer = Renderer::new(FontConfig::default());
        let list = DrawList {
            width: 16,
            height: 16,
            items: vec![svg_item("power"), svg_item("power"), svg_item("C:/mine/a.svg"), svg_item("no-such-icon")],
        };
        let mut icons = BundledIcons::default();
        assert_eq!(icons.ensure(&mut renderer, &list), 1);
        assert_eq!(icons.ensure(&mut renderer, &list), 0, "held from now on");
        assert!(renderer.has_svg("power") && !renderer.has_svg("C:/mine/a.svg") && !renderer.has_svg("no-such-icon"));
    }
}

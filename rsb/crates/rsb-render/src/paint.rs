//! The draw loop: walks a draw list and paints it into a premultiplied RGBA
//! canvas, then copies the rows that changed out as premultiplied BGRA.
//!
//! Clips and opacity groups are applied lazily: a `PushClip` or `PushOpacity`
//! only costs a mask or a layer once a leaf inside it is actually painted, so
//! a partial redraw that touches one text item does not pay for the groups
//! around items it skips. Pops always run, so the stacks stay balanced.

use rsb_core::draw::{DrawCmd, DrawItem, DrawList, Fill};
use rsb_core::geom::{Radii, RectF, RectPx};
use rsb_core::pixels::PixelsMut;
use tiny_skia::{FillRule, FilterQuality, Mask, Paint, Path, Pattern, Pixmap, PixmapRef, SpreadMode, Transform};

use crate::RenderError;
use crate::images::ImageCache;
use crate::shadow::{self, ShadowScratch};
use crate::shape;
use crate::text::TextEngine;

/// Buffers that live between frames.
#[derive(Default)]
pub(crate) struct Scratch {
    base: Option<Pixmap>,
    layers: Vec<Pixmap>,
    masks: Vec<Mask>,
    region_mask: Option<(RectPx, Mask)>,
    /// All 255: a full redraw also paints through a mask, so a partial
    /// redraw (which needs one) rounds exactly like it.
    full_mask: Option<Mask>,
    shadow: ShadowScratch,
}

impl Scratch {
    fn fit(&mut self, w: u32, h: u32) -> Result<(), RenderError> {
        if self.base.as_ref().is_some_and(|p| p.width() == w && p.height() == h) {
            return Ok(());
        }
        self.layers.clear();
        self.masks.clear();
        self.region_mask = None;
        self.full_mask = None;
        self.base = Some(Pixmap::new(w, h).ok_or(RenderError::BadSize)?);
        Ok(())
    }

    fn region_mask(&mut self, w: u32, h: u32, region: RectPx) -> Option<()> {
        if self.region_mask.as_ref().is_some_and(|(r, _)| *r == region) {
            return Some(());
        }
        let mut m = match self.region_mask.take() {
            Some((_, m)) => m,
            None => Mask::new(w, h)?,
        };
        let stride = w as usize;
        m.clear();
        for y in region.y0..region.y1 {
            m.data_mut()[y as usize * stride + region.x0 as usize..y as usize * stride + region.x1 as usize].fill(255);
        }
        self.region_mask = Some((region, m));
        Some(())
    }
}

enum Group {
    Clip { rect: RectF, radii: Radii },
    Opacity(f32),
}

struct Entry {
    group: Group,
    /// Visited by `materialise`.
    live: bool,
    /// A mask or a layer was pushed for this entry and must be popped.
    owned: bool,
}

struct Frame<'a> {
    w: u32,
    h: u32,
    region: RectPx,
    /// `targets[0]` is the canvas, anything above is an opacity layer.
    targets: Vec<Pixmap>,
    spare_layers: &'a mut Vec<Pixmap>,
    spare_masks: &'a mut Vec<Mask>,
    base_mask: Option<&'a Mask>,
    clip_masks: Vec<Mask>,
    stack: Vec<Entry>,
    live: usize,
    text: &'a mut TextEngine,
    images: &'a mut ImageCache,
    shadow: &'a mut ShadowScratch,
}

#[inline]
fn div255(v: u32) -> u32 {
    (v + 127) / 255
}

fn contains(outer: &RectPx, inner: &RectPx) -> bool {
    outer.x0 <= inner.x0 && outer.y0 <= inner.y0 && outer.x1 >= inner.x1 && outer.y1 >= inner.y1
}

/// Draws `list` into the scratch canvas, rewriting only `region`.
pub(crate) fn paint(
    scratch: &mut Scratch,
    text: &mut TextEngine,
    images: &mut ImageCache,
    list: &DrawList,
    region: RectPx,
) -> Result<(), RenderError> {
    let (w, h) = (list.width, list.height);
    scratch.fit(w, h)?;
    let full = region == RectPx::new(0, 0, w as i32, h as i32);
    if full {
        if scratch.full_mask.is_none() {
            let mut m = Mask::new(w, h).ok_or(RenderError::BadSize)?;
            m.data_mut().fill(255);
            scratch.full_mask = Some(m);
        }
    } else {
        scratch.region_mask(w, h, region).ok_or(RenderError::BadSize)?;
    }
    let Scratch { base, layers, masks, region_mask, full_mask, shadow } = scratch;
    let Some(mut canvas) = base.take() else { return Err(RenderError::BadSize) };
    clear_region(&mut canvas, region);

    let mut frame = Frame {
        w,
        h,
        region,
        targets: Vec::new(),
        spare_layers: layers,
        spare_masks: masks,
        base_mask: if full { full_mask.as_ref() } else { region_mask.as_ref().map(|(_, m)| m) },
        clip_masks: Vec::new(),
        stack: Vec::new(),
        live: 0,
        text,
        images,
        shadow,
    };
    frame.targets.push(canvas);
    for item in &list.items {
        frame.item(item);
    }
    while !frame.stack.is_empty() {
        frame.pop();
    }
    *base = frame.targets.pop();
    Ok(())
}

fn clear_region(pm: &mut Pixmap, r: RectPx) {
    let stride = pm.width() as usize * 4;
    let data = pm.data_mut();
    for y in r.y0..r.y1 {
        data[y as usize * stride + r.x0 as usize * 4..y as usize * stride + r.x1 as usize * 4].fill(0);
    }
}

/// Copies `region` of the canvas into `target`, swapping RGBA to BGRA. This
/// is the only place channels are swapped.
pub(crate) fn copy_out(scratch: &Scratch, target: &mut PixelsMut<'_>, region: RectPx) {
    let Some(base) = scratch.base.as_ref() else { return };
    let src_stride = base.width() as usize * 4;
    let dst_stride = target.stride();
    let src = base.data();
    for y in region.y0..region.y1 {
        let s =
            &src[y as usize * src_stride + region.x0 as usize * 4..y as usize * src_stride + region.x1 as usize * 4];
        let d = &mut target.data
            [y as usize * dst_stride + region.x0 as usize * 4..y as usize * dst_stride + region.x1 as usize * 4];
        for (d, s) in d.as_chunks_mut::<4>().0.iter_mut().zip(s.as_chunks::<4>().0) {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = s[3];
        }
    }
}

impl Frame<'_> {
    fn item(&mut self, it: &DrawItem) {
        match &it.cmd {
            DrawCmd::PushClip { rect, radii } => {
                self.stack.push(Entry { group: Group::Clip { rect: *rect, radii: *radii }, live: false, owned: false })
            }
            DrawCmd::PushOpacity(a) => {
                let a = if a.is_finite() { a.clamp(0.0, 1.0) } else { 1.0 };
                self.stack.push(Entry { group: Group::Opacity(a), live: false, owned: false })
            }
            DrawCmd::PopClip | DrawCmd::PopOpacity => self.pop(),
            leaf => self.leaf(it, leaf),
        }
    }

    fn leaf(&mut self, it: &DrawItem, cmd: &DrawCmd) {
        let bounds_px = RectPx::outward(&it.bounds);
        let Some(vis) = bounds_px.intersect(&self.region) else { return };
        // fully outside an enclosing clip: nothing to paint, nothing to build
        let clipped_out = self.stack.iter().any(|e| match &e.group {
            Group::Clip { rect, .. } => RectPx::outward(rect).intersect(&vis).is_none(),
            Group::Opacity(a) => *a <= 0.0,
        });
        if clipped_out {
            return;
        }
        self.materialise();
        match cmd {
            DrawCmd::Fill { rect, radii, fill } => {
                let (Some(path), Some(paint)) = (shape::rrect_path(*rect, *radii), shape::paint_for(fill)) else {
                    return;
                };
                self.fill_path(&path, &paint, FillRule::Winding, &bounds_px, vis, RectPx::outward(rect));
            }
            DrawCmd::Border { rect, radii, widths, color } => {
                let Some(path) = shape::border_path(*rect, *radii, *widths) else { return };
                let Some(paint) = shape::paint_for(&Fill::Solid(*color)) else { return };
                self.fill_path(&path, &paint, FillRule::EvenOdd, &bounds_px, vis, RectPx::outward(rect));
            }
            DrawCmd::Shadow { rect, radii, blur, color } => {
                let mask = self.clip_masks.last().or(self.base_mask);
                if let Some(dst) = self.targets.last_mut() {
                    shadow::draw(self.shadow, dst, mask, vis, *rect, *radii, *blur, *color);
                }
            }
            DrawCmd::Text { rect, text, style, color, align } => {
                let mask = self.clip_masks.last().or(self.base_mask);
                if let Some(dst) = self.targets.last_mut() {
                    self.text.draw(dst, mask, vis, rect, text, style, *color, *align);
                }
            }
            DrawCmd::Image { rect, key, radii } => self.image(&bounds_px, vis, rect, key, *radii),
            DrawCmd::PushClip { .. } | DrawCmd::PopClip | DrawCmd::PushOpacity(_) | DrawCmd::PopOpacity => {}
        }
    }

    /// Fills `path` into the current target. tiny-skia only knows the clip
    /// masks, so when the item's shape reaches past its declared bounds an
    /// extra mask cuts it to `vis`.
    fn fill_path(
        &mut self,
        path: &Path,
        paint: &Paint,
        rule: FillRule,
        bounds_px: &RectPx,
        vis: RectPx,
        shape_px: RectPx,
    ) {
        let limit = if contains(bounds_px, &shape_px) { None } else { self.limit_mask(vis) };
        let mask = limit.as_ref().or(self.clip_masks.last()).or(self.base_mask);
        if let Some(dst) = self.targets.last_mut() {
            dst.fill_path(path, paint, rule, Transform::identity(), mask);
        }
        if let Some(m) = limit {
            self.spare_masks.push(m);
        }
    }

    fn image(&mut self, bounds_px: &RectPx, vis: RectPx, rect: &RectF, key: &str, radii: Radii) {
        let Some(path) = shape::rrect_path(*rect, radii) else { return };
        let limit = if contains(bounds_px, &RectPx::outward(rect)) { None } else { self.limit_mask(vis) };
        if let Some(img) = self.images.get(key)
            && let Some(src) = PixmapRef::from_bytes(&img.data, img.width, img.height)
        {
            let sx = rect.w / img.width as f32;
            let sy = rect.h / img.height as f32;
            let exact =
                (sx - 1.0).abs() < 1e-4 && (sy - 1.0).abs() < 1e-4 && rect.x.fract() == 0.0 && rect.y.fract() == 0.0;
            let quality = if exact { FilterQuality::Nearest } else { FilterQuality::Bilinear };
            let transform = Transform::from_row(sx, 0.0, 0.0, sy, rect.x, rect.y);
            let paint = Paint {
                shader: Pattern::new(src, SpreadMode::Pad, quality, 1.0, transform),
                anti_alias: true,
                ..Paint::default()
            };
            let mask = limit.as_ref().or(self.clip_masks.last()).or(self.base_mask);
            if let Some(dst) = self.targets.last_mut() {
                dst.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), mask);
            }
        }
        if let Some(m) = limit {
            self.spare_masks.push(m);
        }
    }

    fn take_mask(&mut self) -> Option<Mask> {
        self.spare_masks.pop().or_else(|| Mask::new(self.w, self.h))
    }

    /// The current mask (or everything) cut to `vis`.
    fn limit_mask(&mut self, vis: RectPx) -> Option<Mask> {
        let mut m = self.take_mask()?;
        m.clear();
        let stride = self.w as usize;
        let cur = self.clip_masks.last().or(self.base_mask).map(|c| c.data());
        for y in vis.y0..vis.y1 {
            let a = y as usize * stride + vis.x0 as usize;
            let b = y as usize * stride + vis.x1 as usize;
            match cur {
                Some(c) => m.data_mut()[a..b].copy_from_slice(&c[a..b]),
                None => m.data_mut()[a..b].fill(255),
            }
        }
        Some(m)
    }

    /// Builds the masks and layers of every group a leaf is about to paint in.
    fn materialise(&mut self) {
        while self.live < self.stack.len() {
            let i = self.live;
            match self.stack[i].group {
                Group::Clip { rect, radii } => {
                    if let Some(m) = self.clip_mask(rect, radii) {
                        self.clip_masks.push(m);
                        self.stack[i].owned = true;
                    }
                }
                Group::Opacity(a) => {
                    if a < 1.0 {
                        let layer = self.spare_layers.pop().or_else(|| Pixmap::new(self.w, self.h));
                        if let Some(mut layer) = layer {
                            clear_region(&mut layer, self.region);
                            self.targets.push(layer);
                            self.stack[i].owned = true;
                        }
                    }
                }
            }
            self.stack[i].live = true;
            self.live += 1;
        }
    }

    /// The clip shape intersected with the mask already in force.
    fn clip_mask(&mut self, rect: RectF, radii: Radii) -> Option<Mask> {
        let mut m = self.take_mask()?;
        m.clear();
        if let Some(path) = shape::rrect_path(rect, radii) {
            m.fill_path(&path, FillRule::Winding, true, Transform::identity());
        }
        if let Some(parent) = self.clip_masks.last().or(self.base_mask) {
            for (a, b) in m.data_mut().iter_mut().zip(parent.data()) {
                *a = div255(*a as u32 * *b as u32) as u8;
            }
        }
        Some(m)
    }

    fn pop(&mut self) {
        let Some(entry) = self.stack.pop() else { return };
        if !entry.live {
            return;
        }
        self.live -= 1;
        match entry.group {
            Group::Clip { .. } => {
                if entry.owned
                    && let Some(m) = self.clip_masks.pop()
                {
                    self.spare_masks.push(m);
                }
            }
            Group::Opacity(a) => {
                if entry.owned
                    && let Some(layer) = self.targets.pop()
                {
                    if let Some(parent) = self.targets.last_mut() {
                        composite(parent, &layer, self.region, a);
                    }
                    self.spare_layers.push(layer);
                }
            }
        }
    }
}

/// Source-over of `layer` times `opacity` onto `parent`, inside `region`.
fn composite(parent: &mut Pixmap, layer: &Pixmap, region: RectPx, opacity: f32) {
    let op = (opacity * 255.0).round() as u32;
    if op == 0 {
        return;
    }
    let stride = parent.width() as usize * 4;
    let src = layer.data();
    let dst = parent.data_mut();
    for y in region.y0..region.y1 {
        let a = y as usize * stride + region.x0 as usize * 4;
        let b = y as usize * stride + region.x1 as usize * 4;
        for (d, s) in dst[a..b].as_chunks_mut::<4>().0.iter_mut().zip(src[a..b].as_chunks::<4>().0) {
            if s[3] == 0 {
                continue;
            }
            let (sr, sg, sb, sa) = if op == 255 {
                (s[0] as u32, s[1] as u32, s[2] as u32, s[3] as u32)
            } else {
                (div255(s[0] as u32 * op), div255(s[1] as u32 * op), div255(s[2] as u32 * op), div255(s[3] as u32 * op))
            };
            let inv = 255 - sa;
            d[0] = (sr + div255(d[0] as u32 * inv)).min(255) as u8;
            d[1] = (sg + div255(d[1] as u32 * inv)).min(255) as u8;
            d[2] = (sb + div255(d[2] as u32 * inv)).min(255) as u8;
            d[3] = (sa + div255(d[3] as u32 * inv)).min(255) as u8;
        }
    }
}

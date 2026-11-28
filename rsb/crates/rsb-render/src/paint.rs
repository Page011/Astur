//! The draw loop: walks a draw list and paints it into a premultiplied RGBA
//! canvas, then copies the rows that changed out as premultiplied BGRA.
//!
//! Clips and opacity groups are applied lazily: a `PushClip` or `PushOpacity`
//! only costs a mask or a layer once a leaf inside it is actually painted, so
//! a partial redraw that touches one text item does not pay for the groups
//! around items it skips. Pops always run, so the stacks stay balanced.
//!
//! A group only costs what it covers: its layer is cleared and composited
//! inside the group's own bounds (a push item's `bounds` is the whole group),
//! and its clip mask is written and cleared inside the clip's bounds. How many
//! layers and masks are alive at once, and how many are kept between frames,
//! is capped, so a draw list with deep nesting cannot grow the process.

use rsb_core::color::Color;
use rsb_core::draw::{DrawCmd, DrawItem, DrawList, Fill};
use std::sync::Arc;

use rsb_core::geom::{PathF, Radii, RectF, RectPx};
use rsb_core::pixels::PixelsMut;
use tiny_skia::{FillRule, FilterQuality, Mask, Paint, Path, Pattern, Pixmap, PixmapRef, SpreadMode, Transform};

use crate::RenderError;
use crate::images::ImageCache;
use crate::shadow::{self, ShadowScratch};
use crate::shape;
use crate::svg::SvgCache;
use crate::text::TextEngine;

/// Surface sizes whose buffers are kept; the least recently drawn size is
/// dropped when a fifth one comes along.
pub(crate) const SCRATCH_SURFACES: usize = 4;
/// Opacity layers alive at once; a group nested deeper is drawn without its
/// opacity.
const MAX_LIVE_LAYERS: usize = 16;
/// Bytes of opacity layers alive at once (a group beyond this is drawn
/// without its opacity) and of clip masks alive at once (a clip beyond this
/// lets nothing through).
const MAX_LIVE_BYTES: usize = 64 << 20;
/// Bytes of spare layers and masks one surface keeps between frames.
const SPARE_POOL_BYTES: usize = 48 << 20;

/// Buffers for one surface size.
struct SurfaceScratch {
    w: u32,
    h: u32,
    base: Option<Pixmap>,
    /// Spare layers: contents are whatever the last use left.
    layers: Vec<Pixmap>,
    /// Spare masks: all zero. Whoever takes one writes inside a rectangle and
    /// zeroes that rectangle before giving it back.
    masks: Vec<Mask>,
    region_mask: Option<(RectPx, Mask)>,
    /// All 255: a full redraw also paints through a mask, so a partial
    /// redraw (which needs one) rounds exactly like it.
    full_mask: Option<Mask>,
}

impl SurfaceScratch {
    fn new(w: u32, h: u32) -> Result<SurfaceScratch, RenderError> {
        Ok(SurfaceScratch {
            w,
            h,
            base: Some(Pixmap::new(w, h).ok_or(RenderError::BadSize)?),
            layers: Vec::new(),
            masks: Vec::new(),
            region_mask: None,
            full_mask: None,
        })
    }

    fn bytes(&self) -> usize {
        let pixmaps = self.base.iter().chain(self.layers.iter()).count();
        let masks = self.masks.len() + self.region_mask.iter().count() + self.full_mask.iter().count();
        pixmaps * self.w as usize * self.h as usize * 4 + masks * self.w as usize * self.h as usize
    }

    fn region_mask(&mut self, region: RectPx, allocs: &mut u64) -> Option<()> {
        if self.region_mask.as_ref().is_some_and(|(r, _)| *r == region) {
            return Some(());
        }
        let (w, h) = (self.w, self.h);
        let mut m = match self.region_mask.take() {
            Some((_, m)) => m,
            None => {
                *allocs += 1;
                Mask::new(w, h)?
            }
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

/// Buffers that live between frames: one set per recent surface size, so a
/// bar and a panel drawn in turn do not reallocate each other's canvases.
#[derive(Default)]
pub(crate) struct Scratch {
    /// Least recently drawn first.
    surfaces: Vec<SurfaceScratch>,
    shadow: ShadowScratch,
    /// Pixmaps and masks created so far (a reused one is not counted).
    allocations: u64,
}

impl Scratch {
    pub(crate) fn surfaces(&self) -> usize {
        self.surfaces.len()
    }

    pub(crate) fn allocations(&self) -> u64 {
        self.allocations
    }

    /// Bytes the shadow blur keeps between frames.
    pub(crate) fn shadow_bytes(&self) -> usize {
        self.shadow.bytes()
    }

    /// Drops the buffers of one surface size, and the shadow buffers with
    /// them: a closed popup must not leave its pixels behind.
    pub(crate) fn release(&mut self, w: u32, h: u32) {
        self.surfaces.retain(|s| !(s.w == w && s.h == h));
        self.shadow.shrink();
    }

    pub(crate) fn bytes(&self) -> usize {
        self.surfaces.iter().map(SurfaceScratch::bytes).sum()
    }

    /// The buffers of the surface drawn last.
    fn last_base(&self) -> Option<&Pixmap> {
        self.surfaces.last().and_then(|s| s.base.as_ref())
    }
}

/// What a clip group cuts to.
enum ClipShape {
    Rect(Radii),
    Path(Arc<PathF>),
}

enum Group {
    /// `bounds` holds everything the clip lets through (empty: nothing).
    Clip {
        bounds: RectF,
        shape: ClipShape,
        even_odd: bool,
    },
    Opacity(f32),
}

struct Entry {
    group: Group,
    /// The pixels the group can touch, cut to the surface (clips: one pixel
    /// more all round for the anti-aliased edge).
    area: RectPx,
    /// Visited by `materialise`.
    live: bool,
    /// A mask or a layer was pushed for this entry and must be popped.
    owned: bool,
    /// The group could not be set up: nothing inside it is drawn.
    void: bool,
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
    svgs: &'a mut SvgCache,
    shadow: &'a mut ShadowScratch,
    allocs: &'a mut u64,
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
    svgs: &mut SvgCache,
    list: &DrawList,
    region: RectPx,
) -> Result<(), RenderError> {
    let (w, h) = (list.width, list.height);
    let mut surface = match scratch.surfaces.iter().position(|s| s.w == w && s.h == h) {
        Some(i) => scratch.surfaces.remove(i),
        None => {
            scratch.allocations += 1;
            SurfaceScratch::new(w, h)?
        }
    };
    let result =
        paint_surface(&mut surface, &mut scratch.shadow, &mut scratch.allocations, text, images, svgs, list, region);
    scratch.surfaces.push(surface);
    if scratch.surfaces.len() > SCRATCH_SURFACES {
        scratch.surfaces.remove(0);
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn paint_surface(
    surface: &mut SurfaceScratch,
    shadow: &mut ShadowScratch,
    allocs: &mut u64,
    text: &mut TextEngine,
    images: &mut ImageCache,
    svgs: &mut SvgCache,
    list: &DrawList,
    region: RectPx,
) -> Result<(), RenderError> {
    let (w, h) = (list.width, list.height);
    let full = region == RectPx::new(0, 0, w as i32, h as i32);
    if full {
        if surface.full_mask.is_none() {
            *allocs += 1;
            let mut m = Mask::new(w, h).ok_or(RenderError::BadSize)?;
            m.data_mut().fill(255);
            surface.full_mask = Some(m);
        }
    } else {
        surface.region_mask(region, allocs).ok_or(RenderError::BadSize)?;
    }
    let SurfaceScratch { base, layers, masks, region_mask, full_mask, .. } = surface;
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
        svgs,
        shadow,
        allocs,
    };
    frame.targets.push(canvas);
    let items = &list.items;
    let mut i = 0;
    while i < items.len() {
        // A group that holds one solid shape is that shape with its alpha
        // scaled: no layer to make, fill and blend.
        if let DrawCmd::PushOpacity(a) = &items[i].cmd
            && let Some(leaf) = items.get(i + 1)
            && items.get(i + 2).is_some_and(|p| matches!(p.cmd, DrawCmd::PopOpacity))
            && let Some(folded) = fold_alpha(leaf, *a)
        {
            frame.item(&folded);
            i += 3;
            continue;
        }
        frame.item(&items[i]);
        i += 1;
    }
    while !frame.stack.is_empty() {
        frame.pop();
    }
    *base = frame.targets.pop();
    Ok(())
}

/// `leaf` painted at `alpha`, when it is a solid shape that alpha can be folded
/// into. Equal to drawing it in a layer and blending that at `alpha`, up to
/// rounding of one level.
fn fold_alpha(leaf: &DrawItem, alpha: f32) -> Option<DrawItem> {
    let a = if alpha.is_finite() { alpha.clamp(0.0, 1.0) } else { 1.0 };
    let scale = |c: &Color| Color { a: c.a * a, ..*c };
    let cmd = match &leaf.cmd {
        DrawCmd::Fill { rect, radii, fill: Fill::Solid(c) } => {
            DrawCmd::Fill { rect: *rect, radii: *radii, fill: Fill::Solid(scale(c)) }
        }
        DrawCmd::Border { rect, radii, widths, color } => {
            DrawCmd::Border { rect: *rect, radii: *radii, widths: *widths, color: scale(color) }
        }
        DrawCmd::FillPath { path, fill: Fill::Solid(c) } => {
            DrawCmd::FillPath { path: path.clone(), fill: Fill::Solid(scale(c)) }
        }
        DrawCmd::BorderPath { outer, inner, color } => {
            DrawCmd::BorderPath { outer: outer.clone(), inner: inner.clone(), color: scale(color) }
        }
        _ => return None,
    };
    Some(DrawItem { node: leaf.node, bounds: leaf.bounds, cmd })
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
    let Some(base) = scratch.last_base() else { return };
    let src_stride = base.width() as usize * 4;
    let dst_stride = target.stride();
    let src = base.data();
    for y in region.y0..region.y1 {
        let s =
            &src[y as usize * src_stride + region.x0 as usize * 4..y as usize * src_stride + region.x1 as usize * 4];
        let d = &mut target.data
            [y as usize * dst_stride + region.x0 as usize * 4..y as usize * dst_stride + region.x1 as usize * 4];
        for (d, s) in d.as_chunks_mut::<4>().0.iter_mut().zip(s.as_chunks::<4>().0) {
            // R and B trade places: one word a pixel, so the loop vectorises
            let v = u32::from_le_bytes(*s);
            *d = ((v & 0xff00_ff00) | ((v & 0xff) << 16) | ((v >> 16) & 0xff)).to_le_bytes();
        }
    }
}

impl Frame<'_> {
    fn item(&mut self, it: &DrawItem) {
        match &it.cmd {
            DrawCmd::PushClip { rect, radii } => self.push_clip(*rect, ClipShape::Rect(*radii), false),
            DrawCmd::PushOpacity(a) => {
                let a = if a.is_finite() { a.clamp(0.0, 1.0) } else { 1.0 };
                let area = self.surface().intersect(&RectPx::outward(&it.bounds)).unwrap_or_default();
                self.stack.push(Entry { group: Group::Opacity(a), area, live: false, owned: false, void: false })
            }
            // An outline that cannot be drawn clips everything away, like an
            // empty clip rectangle; the push still counts so pops stay balanced.
            DrawCmd::PushClipPath { path, even_odd } => {
                let bounds = shape::path_bounds(path).unwrap_or_default();
                self.push_clip(bounds, ClipShape::Path(path.clone()), *even_odd)
            }
            DrawCmd::PopClip | DrawCmd::PopOpacity => self.pop(),
            leaf => self.leaf(it, leaf),
        }
    }

    fn surface(&self) -> RectPx {
        RectPx::new(0, 0, self.w as i32, self.h as i32)
    }

    fn push_clip(&mut self, bounds: RectF, shape: ClipShape, even_odd: bool) {
        let o = RectPx::outward(&bounds);
        let grown = RectPx::new(o.x0 - 1, o.y0 - 1, o.x1 + 1, o.y1 + 1);
        let area = self.surface().intersect(&grown).unwrap_or_default();
        self.stack.push(Entry {
            group: Group::Clip { bounds, shape, even_odd },
            area,
            live: false,
            owned: false,
            void: false,
        });
    }

    fn leaf(&mut self, it: &DrawItem, cmd: &DrawCmd) {
        let bounds_px = RectPx::outward(&it.bounds);
        let Some(vis) = bounds_px.intersect(&self.region) else { return };
        // fully outside an enclosing clip: nothing to paint, nothing to build
        let clipped_out = self.stack.iter().any(|e| match &e.group {
            Group::Clip { bounds, .. } => RectPx::outward(bounds).intersect(&vis).is_none(),
            Group::Opacity(a) => *a <= 0.0,
        });
        if clipped_out {
            return;
        }
        self.materialise();
        if self.stack.iter().any(|e| e.void) {
            return;
        }
        match cmd {
            DrawCmd::Fill { rect, radii, fill } => {
                if let Fill::Solid(color) = fill
                    && self.fill_solid_box(*rect, *radii, *color, &bounds_px, vis)
                {
                    return;
                }
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
            DrawCmd::FillPath { path, fill } => {
                let (Some(sk), Some(hull), Some(paint)) =
                    (shape::outline_path(path), shape::path_bounds(path), shape::paint_for(fill))
                else {
                    return;
                };
                self.fill_path(&sk, &paint, FillRule::Winding, &bounds_px, vis, RectPx::outward(&hull));
            }
            DrawCmd::BorderPath { outer, inner, color } => {
                let (Some(sk), Some(hull), Some(paint)) =
                    (shape::ring_path(outer, inner), shape::path_bounds(outer), shape::paint_for(&Fill::Solid(*color)))
                else {
                    return;
                };
                self.fill_path(&sk, &paint, FillRule::EvenOdd, &bounds_px, vis, RectPx::outward(&hull));
            }
            DrawCmd::ShadowPath { path, blur, color } => {
                let (Some(sk), Some(hull)) = (shape::outline_path(path), shape::path_bounds(path)) else { return };
                let mask = self.clip_masks.last().or(self.base_mask);
                if let Some(dst) = self.targets.last_mut() {
                    shadow::draw_path(self.shadow, dst, mask, vis, sk, hull, *blur, *color);
                }
            }
            DrawCmd::Svg { rect, key, tint } => {
                let mask = self.clip_masks.last().or(self.base_mask);
                if let Some(dst) = self.targets.last_mut() {
                    self.svgs.draw(dst, mask, vis, rect, key, *tint);
                }
            }
            DrawCmd::PushClip { .. }
            | DrawCmd::PushClipPath { .. }
            | DrawCmd::PopClip
            | DrawCmd::PushOpacity(_)
            | DrawCmd::PopOpacity => {}
        }
    }

    /// A big solid box: the pixels no edge touches are written directly
    /// (tiny-skia's masked pipeline costs about 25 times as much a pixel as
    /// a plain blend), and only the ring around them goes through the path
    /// fill. Both halves use the same mask, so a partial redraw gives the
    /// same pixels as a full one. False: too small to be worth splitting.
    fn fill_solid_box(&mut self, rect: RectF, radii: Radii, color: Color, bounds_px: &RectPx, vis: RectPx) -> bool {
        let Some(parts) = shape::solid_parts(rect, radii) else { return false };
        let (Some(ring), Some(paint)) = (shape::rrect_ring(rect, radii, &parts), shape::paint_for(&Fill::Solid(color)))
        else {
            return false;
        };
        self.fill_path(&ring, &paint, FillRule::EvenOdd, bounds_px, vis, RectPx::outward(&rect));
        let mask = self.clip_masks.last().or(self.base_mask);
        if let Some(dst) = self.targets.last_mut() {
            for area in parts.iter().filter_map(|p| p.intersect(&vis)) {
                fill_rows(dst, mask, area, color);
            }
        }
        true
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
            self.keep_mask(m, vis);
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
            self.keep_mask(m, vis);
        }
    }

    fn take_mask(&mut self) -> Option<Mask> {
        self.spare_masks.pop().or_else(|| {
            *self.allocs += 1;
            Mask::new(self.w, self.h)
        })
    }

    fn spare_bytes(&self) -> usize {
        (self.spare_layers.len() * 4 + self.spare_masks.len()) * self.w as usize * self.h as usize
    }

    /// Zeroes `dirty` (all a user of the mask wrote) and keeps the mask for
    /// reuse while the spares stay under their cap.
    fn keep_mask(&mut self, mut m: Mask, dirty: RectPx) {
        if self.spare_bytes() + self.w as usize * self.h as usize > SPARE_POOL_BYTES {
            return;
        }
        let stride = self.w as usize;
        for y in dirty.y0.max(0)..dirty.y1.min(self.h as i32) {
            let row = y as usize * stride;
            m.data_mut()
                [row + dirty.x0.max(0) as usize..row + (dirty.x1.min(self.w as i32)).max(dirty.x0.max(0)) as usize]
                .fill(0);
        }
        self.spare_masks.push(m);
    }

    fn keep_layer(&mut self, layer: Pixmap) {
        if self.spare_bytes() + self.w as usize * self.h as usize * 4 <= SPARE_POOL_BYTES {
            self.spare_layers.push(layer);
        }
    }

    /// The current mask (or everything) cut to `vis`; zero outside it.
    fn limit_mask(&mut self, vis: RectPx) -> Option<Mask> {
        let mut m = self.take_mask()?;
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
        let surface_bytes = self.w as usize * self.h as usize;
        while self.live < self.stack.len() {
            let i = self.live;
            let area = self.stack[i].area;
            match &self.stack[i].group {
                Group::Clip { bounds, shape, even_odd } => {
                    let path = match shape {
                        ClipShape::Rect(radii) => shape::rrect_path(*bounds, *radii),
                        ClipShape::Path(p) => shape::outline_path(p),
                    };
                    let rule = if *even_odd { FillRule::EvenOdd } else { FillRule::Winding };
                    let room = (self.clip_masks.len() + 1) * surface_bytes <= MAX_LIVE_BYTES;
                    match room.then(|| self.clip_mask(path, rule, area)).flatten() {
                        Some(m) => {
                            self.clip_masks.push(m);
                            self.stack[i].owned = true;
                        }
                        // No room for the mask: the group lets nothing through.
                        None => self.stack[i].void = true,
                    }
                }
                Group::Opacity(a) => {
                    let layers = self.targets.len() - 1;
                    let room = layers < MAX_LIVE_LAYERS && (layers + 1) * surface_bytes * 4 <= MAX_LIVE_BYTES;
                    // Past the caps the group is drawn without its opacity.
                    if *a < 1.0
                        && room
                        && let Some(area) = area.intersect(&self.region)
                    {
                        let layer = self.spare_layers.pop().or_else(|| {
                            *self.allocs += 1;
                            Pixmap::new(self.w, self.h)
                        });
                        if let Some(mut layer) = layer {
                            clear_region(&mut layer, area);
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

    /// The clip shape (None: nothing gets through) intersected with the mask
    /// already in force. Only `area` is written; the rest stays zero.
    fn clip_mask(&mut self, path: Option<Path>, rule: FillRule, area: RectPx) -> Option<Mask> {
        let mut m = self.take_mask()?;
        if let Some(path) = path {
            m.fill_path(&path, rule, true, Transform::identity());
        }
        if let Some(parent) = self.clip_masks.last().or(self.base_mask) {
            let stride = self.w as usize;
            for y in area.y0..area.y1 {
                let row = y as usize * stride;
                let span = row + area.x0 as usize..row + area.x1 as usize;
                for (a, b) in m.data_mut()[span.clone()].iter_mut().zip(&parent.data()[span]) {
                    *a = div255(*a as u32 * *b as u32) as u8;
                }
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
                    self.keep_mask(m, entry.area);
                }
            }
            Group::Opacity(a) => {
                if entry.owned
                    && let Some(layer) = self.targets.pop()
                {
                    if let (Some(parent), Some(area)) = (self.targets.last_mut(), entry.area.intersect(&self.region)) {
                        composite(parent, &layer, area, a);
                    }
                    self.keep_layer(layer);
                }
            }
        }
    }
}

/// Source-over of a solid colour onto `area` of `dst`, each pixel scaled by
/// `mask`.
fn fill_rows(dst: &mut Pixmap, mask: Option<&Mask>, area: RectPx, color: Color) {
    let ch = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    let alpha = ch(color.a);
    let sa = (alpha * 255.0).round() as u32;
    if sa == 0 {
        return;
    }
    let premul = [color.r, color.g, color.b].map(|c| (ch(c) * alpha * 255.0).round() as u32);
    let width = dst.width() as usize;
    let mask_data = mask.map(|m| m.data());
    let data = dst.data_mut();
    let (x0, x1) = (area.x0.max(0) as usize, area.x1.clamp(0, width as i32) as usize);
    if x0 >= x1 {
        return;
    }
    for y in area.y0.max(0) as usize..area.y1.max(0) as usize {
        let row = &mut data[(y * width + x0) * 4..(y * width + x1) * 4];
        let covered = mask_data.map(|m| &m[y * width + x0..y * width + x1]);
        // a row the mask lets through whole: one store a pixel
        if covered.is_none_or(|m| m.iter().all(|&v| v == 255)) {
            let px = row.as_chunks_mut::<4>().0;
            if sa == 255 {
                px.fill([premul[0] as u8, premul[1] as u8, premul[2] as u8, 255]);
            } else {
                let inv = 255 - sa;
                for d in px {
                    d[0] = (premul[0] + div255(d[0] as u32 * inv)).min(255) as u8;
                    d[1] = (premul[1] + div255(d[1] as u32 * inv)).min(255) as u8;
                    d[2] = (premul[2] + div255(d[2] as u32 * inv)).min(255) as u8;
                    d[3] = (sa + div255(d[3] as u32 * inv)).min(255) as u8;
                }
            }
            continue;
        }
        let covered = covered.unwrap_or(&[]);
        for (d, &m) in row.as_chunks_mut::<4>().0.iter_mut().zip(covered) {
            let m = m as u32;
            if m == 0 {
                continue;
            }
            let (r, g, b, a) = if m == 255 {
                (premul[0], premul[1], premul[2], sa)
            } else {
                (div255(premul[0] * m), div255(premul[1] * m), div255(premul[2] * m), div255(sa * m))
            };
            let inv = 255 - a;
            d[0] = (r + div255(d[0] as u32 * inv)).min(255) as u8;
            d[1] = (g + div255(d[1] as u32 * inv)).min(255) as u8;
            d[2] = (b + div255(d[2] as u32 * inv)).min(255) as u8;
            d[3] = (a + div255(d[3] as u32 * inv)).min(255) as u8;
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

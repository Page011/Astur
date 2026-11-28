//! SVG icons and images: parsing with limits, and a cache of rasters.
//!
//! An SVG can only describe shapes here: `usvg` is built without text and
//! raster images, and its href resolvers refuse everything, so a file can
//! never make RSB read another file. A user's file also has to stay small:
//! its element count (with `<use>` copies and every clip, mask, pattern and
//! marker reference counted as a copy), its filters and the layers it needs
//! are checked before anything is drawn.
//!
//! Drawing: the tree is rasterised once per (key, pixel size, tinted or not)
//! and the raster is blitted at a whole pixel. A tinted icon keeps only the
//! coverage (one byte a pixel) and takes its colour when blitted, so a colour
//! transition does not rasterise again every frame.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use resvg::usvg;
use rsb_core::color::Color;
use rsb_core::geom::{RectF, RectPx};
use tiny_skia::{Mask, Pixmap, Transform};

use crate::{ParsedSvg, RenderError, SVG_CACHE_MAX_BYTES, SVG_MAX_FILE_BYTES, SVG_MAX_NODES, SvgSource};

/// Nesting allowed in a user's file (the parser's own default is 1024).
const USER_MAX_DEPTH: u32 = 64;
/// Most SVGs kept; a new key beyond this is refused (replacing one is fine).
const MAX_TREES: usize = 1024;
/// Largest side of a raster, px.
const MAX_RASTER_SIDE: u32 = 4096;
/// Largest side of a raster of a user's file, px.
const USER_MAX_RASTER_SIDE: u32 = 512;
/// Groups that need a layer of their own (opacity, clip, mask) nested inside
/// one another in a user's file. Each one is a surface-sized buffer to fill.
const USER_MAX_LAYER_DEPTH: u32 = 8;
/// Time one frame may spend rasterising users' files. Past it, new rasters wait
/// for a later frame, so many small files cannot add up to a long stall. A
/// frame always makes at least one, so every file is drawn in the end.
const USER_FRAME_BUDGET: Duration = Duration::from_millis(8);
/// Layers times pixels one raster of a user's file may cost: about 16 ms of
/// work (a layer costs roughly 4 ns a pixel), whatever the file holds.
const USER_LAYER_PIXEL_BUDGET: u64 = 4_000_000;

/// Parses `bytes` for drawing. See `crate::parse_svg`.
pub(crate) fn parse(bytes: &[u8], source: SvgSource) -> Result<ParsedSvg, RenderError> {
    if bytes.len() > SVG_MAX_FILE_BYTES {
        return Err(RenderError::Svg(format!("file is {} bytes, limit {SVG_MAX_FILE_BYTES}", bytes.len())));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| RenderError::Svg("not UTF-8 text".into()))?;
    let mut opt = usvg::Options {
        resources_dir: None,
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    };
    let tree = match source {
        SvgSource::Bundled => usvg::Tree::from_str(text, &opt),
        SvgSource::User => {
            // entity declarations are how small files expand into huge ones
            if text.contains("<!ENTITY") {
                return Err(RenderError::Svg("entity declarations are not allowed".into()));
            }
            opt.max_depth = USER_MAX_DEPTH;
            let xml = usvg::roxmltree::Document::parse_with_options(
                text,
                usvg::roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() },
            )
            .map_err(|e| RenderError::Svg(e.to_string()))?;
            let count = expanded_elements(&xml);
            if count > SVG_MAX_NODES as u64 {
                return Err(RenderError::Svg(format!("{count} elements, limit {SVG_MAX_NODES}")));
            }
            usvg::Tree::from_xmltree(&xml, &opt)
        }
    }
    .map_err(|e| RenderError::Svg(e.to_string()))?;
    let mut layers = None;
    if source == SvgSource::User {
        if group_has_filter(tree.root()) {
            return Err(RenderError::Svg("filters are not allowed".into()));
        }
        let cost = layer_cost(tree.root(), &mut HashMap::new());
        if cost.depth > USER_MAX_LAYER_DEPTH {
            return Err(RenderError::Svg(format!(
                "{} nested layers (opacity, clip-path, mask), limit {USER_MAX_LAYER_DEPTH}",
                cost.depth
            )));
        }
        layers = Some(cost.layers);
    }
    Ok(ParsedSvg { tree, layers })
}

/// Elements in the document, where a `<use>` counts the elements of what it
/// points at (the parser copies them). Saturates; cycles count once. Done
/// with an explicit stack: a file can nest far deeper than the thread stack.
fn expanded_elements(doc: &usvg::roxmltree::Document) -> u64 {
    use usvg::roxmltree::{Node, NodeId};

    let mut ids: HashMap<&str, NodeId> = HashMap::new();
    for n in doc.descendants().filter(Node::is_element) {
        if let Some(id) = n.attribute("id") {
            ids.entry(id).or_insert(n.id());
        }
    }
    let deps = |n: Node<'_, '_>| -> Vec<NodeId> {
        let mut out: Vec<NodeId> = n.children().filter(Node::is_element).map(|c| c.id()).collect();
        if n.tag_name().name() == "use" {
            let href = n.attribute("href").or_else(|| n.attribute(("http://www.w3.org/1999/xlink", "href")));
            if let Some(target) = href.and_then(|h| h.strip_prefix('#')).and_then(|h| ids.get(h)) {
                out.push(*target);
            }
        }
        // A clip, mask, pattern or marker is drawn again for every element
        // that names it: each `url(#id)` in any attribute (or in a `style`)
        // counts as a copy of what it points at.
        for attr in n.attributes() {
            let mut rest = attr.value();
            while let Some(at) = rest.find("url(#") {
                rest = &rest[at + 5..];
                let id = rest.split([')', '"', '\'']).next().unwrap_or("").trim();
                if let Some(target) = ids.get(id) {
                    out.push(*target);
                }
            }
        }
        out
    };

    let mut memo: HashMap<NodeId, u64> = HashMap::new();
    let mut visiting: HashSet<NodeId> = HashSet::new();
    let mut stack = vec![(doc.root_element().id(), false)];
    while let Some((id, finished)) = stack.pop() {
        if memo.contains_key(&id) {
            continue;
        }
        let Some(node) = doc.get_node(id) else { continue };
        let children = deps(node);
        if !finished {
            visiting.insert(id);
            stack.push((id, true));
            stack.extend(
                children.into_iter().filter(|c| !memo.contains_key(c) && !visiting.contains(c)).map(|c| (c, false)),
            );
        } else {
            visiting.remove(&id);
            let total = children.iter().fold(1u64, |t, c| t.saturating_add(memo.get(c).copied().unwrap_or(1)));
            memo.insert(id, total);
        }
    }
    memo.get(&doc.root_element().id()).copied().unwrap_or(0)
}

/// True when the group, anything inside it, or a clip, mask or pattern it
/// uses has a filter.
fn group_has_filter(g: &usvg::Group) -> bool {
    if !g.filters().is_empty() {
        return true;
    }
    if g.clip_path().is_some_and(|c| group_has_filter(c.root())) || g.mask().is_some_and(|m| group_has_filter(m.root()))
    {
        return true;
    }
    g.children().iter().any(|n| match n {
        usvg::Node::Group(c) => group_has_filter(c),
        usvg::Node::Path(p) => [p.fill().map(|f| f.paint()), p.stroke().map(|s| s.paint())]
            .into_iter()
            .flatten()
            .any(|paint| matches!(paint, usvg::Paint::Pattern(pat) if group_has_filter(pat.root()))),
        _ => false,
    })
}

/// What drawing a group costs in layers.
#[derive(Clone, Copy, Default)]
struct LayerCost {
    /// Surface-sized buffers made over one render (saturating).
    layers: u64,
    /// Most layers nested inside one another on any path.
    depth: u32,
}

/// Layers needed to draw `g`: the group's own (opacity, blend, isolation, clip
/// or mask), the buffer a clip or mask is drawn into with what it holds (once
/// for every group that names it), and everything inside. `memo` holds the
/// cost of shared clips, masks and patterns by address, so a file that
/// chains them stays linear to count.
fn layer_cost(g: &usvg::Group, memo: &mut HashMap<usize, LayerCost>) -> LayerCost {
    let own = u64::from(
        g.opacity().get() < 1.0
            || g.isolate()
            || g.blend_mode() != usvg::BlendMode::Normal
            || g.clip_path().is_some()
            || g.mask().is_some()
            || !g.filters().is_empty(),
    );
    let mut inner = LayerCost::default();
    let mut add = |c: LayerCost, extra: u64| {
        inner.layers = inner.layers.saturating_add(c.layers).saturating_add(extra);
        inner.depth = inner.depth.max(c.depth);
    };
    if let Some(c) = g.clip_path() {
        add(shared_cost(c.root(), memo), 1);
        if let Some(nested) = c.clip_path() {
            add(shared_cost(nested.root(), memo), 1);
        }
    }
    if let Some(m) = g.mask() {
        add(shared_cost(m.root(), memo), 1);
        if let Some(nested) = m.mask() {
            add(shared_cost(nested.root(), memo), 1);
        }
    }
    for n in g.children() {
        match n {
            usvg::Node::Group(c) => add(layer_cost(c, memo), 0),
            usvg::Node::Path(p) => {
                for paint in [p.fill().map(|f| f.paint()), p.stroke().map(|s| s.paint())].into_iter().flatten() {
                    if let usvg::Paint::Pattern(pat) = paint {
                        add(shared_cost(pat.root(), memo), 1);
                    }
                }
            }
            _ => {}
        }
    }
    LayerCost { layers: own.saturating_add(inner.layers), depth: inner.depth.saturating_add(own as u32) }
}

fn shared_cost(root: &usvg::Group, memo: &mut HashMap<usize, LayerCost>) -> LayerCost {
    let at = root as *const usvg::Group as usize;
    if let Some(c) = memo.get(&at) {
        return *c;
    }
    let c = layer_cost(root, memo);
    memo.insert(at, c);
    c
}

/// A raster: one byte of coverage a pixel (tinted icons) or premultiplied
/// RGBA (an SVG in its own colours).
struct Raster {
    w: u32,
    h: u32,
    coverage: bool,
    data: Vec<u8>,
    used: u64,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct RasterKey {
    key: Arc<str>,
    w: u32,
    h: u32,
    coverage: bool,
}

/// A picture's key as the log shows it: the widget folder and file name for a
/// file in a widget's own folder, the file name alone for anything else. The
/// key is the file's full path, which holds the user's account name, and a
/// file a widget was granted can sit straight in the home folder.
fn shown(key: &str) -> String {
    let mut parts = key.rsplit(['/', '\\']).filter(|p| !p.is_empty());
    match (parts.next(), parts.next(), parts.next()) {
        (Some(file), Some(dir), Some(up)) if up.eq_ignore_ascii_case("widgets") => format!("{dir}/{file}"),
        (Some(file), ..) => file.to_string(),
        _ => String::new(),
    }
}

#[derive(Default)]
pub(crate) struct SvgCache {
    trees: HashMap<Arc<str>, ParsedSvg>,
    rasters: HashMap<RasterKey, Raster>,
    raster_bytes: usize,
    tick: u64,
    /// `tick` when the current frame began: a raster used after it is in use.
    frame_tick: u64,
    made: u64,
    /// Keys already reported as too costly to draw, so the log says it once.
    refused: HashSet<Arc<str>>,
    /// Time spent on users' rasters in this frame, and whether one was left
    /// for a later frame.
    spent: Duration,
    deferred: bool,
}

impl SvgCache {
    pub(crate) fn has(&self, key: &str) -> bool {
        self.trees.contains_key(key)
    }

    pub(crate) fn len(&self) -> usize {
        self.trees.len()
    }

    pub(crate) fn raster_count(&self) -> usize {
        self.rasters.len()
    }

    pub(crate) fn raster_bytes(&self) -> usize {
        self.raster_bytes
    }

    /// Rasters made so far, including ones since dropped.
    pub(crate) fn rasters_made(&self) -> u64 {
        self.made
    }

    /// Starts a frame: the budget for users' files is whole again.
    pub(crate) fn begin_frame(&mut self) {
        self.frame_tick = self.tick;
        self.spent = Duration::ZERO;
        self.deferred = false;
    }

    /// True when the last frame left a user's file undrawn for want of time:
    /// draw again.
    pub(crate) fn pending(&self) -> bool {
        self.deferred
    }

    /// Keeps `svg` under `key` and returns its intrinsic size. Its old
    /// rasters go. A new key beyond `MAX_TREES` is refused (logged).
    pub(crate) fn set(&mut self, key: Arc<str>, svg: ParsedSvg) -> (f32, f32) {
        let size = svg.size();
        if self.trees.len() >= MAX_TREES && !self.trees.contains_key(&key) {
            log::warn!("svg refused: {MAX_TREES} are already loaded");
            return size;
        }
        self.drop_rasters(&key);
        self.refused.remove(&key);
        self.trees.insert(key, svg);
        size
    }

    fn drop_rasters(&mut self, key: &str) {
        let gone: Vec<RasterKey> = self.rasters.keys().filter(|k| &*k.key == key).cloned().collect();
        for k in gone {
            if let Some(r) = self.rasters.remove(&k) {
                self.raster_bytes -= r.data.len();
            }
        }
    }

    /// Raster for `key` at `w` x `h` px, made now when it is not cached.
    fn raster(&mut self, key: &str, w: u32, h: u32, coverage: bool) -> Option<&Raster> {
        self.tick += 1;
        let tick = self.tick;
        let (tree_key, svg) = self.trees.get_key_value(key)?;
        // A user's file is drawn on the UI thread: it gets a small raster and a
        // bounded number of layers, and a frame only so long, so no file can
        // hold a frame for long. A larger picture is rasterised at the largest
        // size allowed and scaled up when drawn.
        let user = svg.layers;
        let (w, h) = match user {
            Some(_) if w.max(h) > USER_MAX_RASTER_SIDE => {
                let k = USER_MAX_RASTER_SIDE as f32 / w.max(h) as f32;
                (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1))
            }
            _ => (w, h),
        };
        if let Some(layers) = user {
            let cost = layers.saturating_add(1).saturating_mul(u64::from(w) * u64::from(h));
            if cost > USER_LAYER_PIXEL_BUDGET {
                if self.refused.insert(tree_key.clone()) {
                    log::warn!("svg '{}' is not drawn at {w}x{h}: too many layers for a user file", shown(tree_key));
                }
                return None;
            }
        }
        let rk = RasterKey { key: tree_key.clone(), w, h, coverage };
        if !self.rasters.contains_key(&rk) {
            let bytes = w as usize * h as usize * if coverage { 1 } else { 4 };
            if w == 0 || h == 0 || w > MAX_RASTER_SIDE || h > MAX_RASTER_SIDE || bytes > SVG_CACHE_MAX_BYTES {
                return None;
            }
            // Rasters drawn in this frame stay: dropping one to make room would
            // only make the next frame draw it again, for ever. When the rest
            // cannot make room, this picture is left out.
            let evictable: usize =
                self.rasters.values().filter(|r| r.used <= self.frame_tick).map(|r| r.data.len()).sum();
            if self.raster_bytes - evictable + bytes > SVG_CACHE_MAX_BYTES {
                if self.refused.insert(tree_key.clone()) {
                    log::warn!(
                        "svg '{}' is not drawn at {w}x{h}: the pictures on screen fill the raster cache",
                        shown(tree_key)
                    );
                }
                return None;
            }
            if user.is_some() && self.spent >= USER_FRAME_BUDGET {
                self.deferred = true;
                return None;
            }
            let started = Instant::now();
            let (sw, sh) = svg.size();
            let mut pm = Pixmap::new(w, h)?;
            resvg::render(&svg.tree, Transform::from_scale(w as f32 / sw, h as f32 / sh), &mut pm.as_mut());
            let data = if coverage { pm.data().as_chunks::<4>().0.iter().map(|p| p[3]).collect() } else { pm.take() };
            if user.is_some() {
                self.spent += started.elapsed();
            }
            while self.raster_bytes + bytes > SVG_CACHE_MAX_BYTES {
                let frame_tick = self.frame_tick;
                let Some(oldest) = self
                    .rasters
                    .iter()
                    .filter(|(_, r)| r.used <= frame_tick)
                    .min_by_key(|(_, r)| r.used)
                    .map(|(k, _)| k.clone())
                else {
                    break;
                };
                if let Some(r) = self.rasters.remove(&oldest) {
                    self.raster_bytes -= r.data.len();
                }
            }
            self.raster_bytes += data.len();
            self.made += 1;
            self.rasters.insert(rk.clone(), Raster { w, h, coverage, data, used: tick });
        }
        let r = self.rasters.get_mut(&rk)?;
        r.used = tick;
        Some(&*r)
    }

    /// Draws the SVG under `key` inside `rect` (aspect ratio kept, centred,
    /// placed on whole pixels), only inside `vis` and scaled by `mask`.
    /// `tint` Some: every painted pixel takes that colour, its coverage
    /// kept. A missing key draws nothing.
    pub(crate) fn draw(
        &mut self,
        dst: &mut Pixmap,
        mask: Option<&Mask>,
        vis: RectPx,
        rect: &RectF,
        key: &str,
        tint: Option<Color>,
    ) {
        if rect.is_empty() || !crate::shape::sane(rect) {
            return;
        }
        let Some(svg) = self.trees.get(key) else { return };
        let (sw, sh) = svg.size();
        let scale = (rect.w / sw).min(rect.h / sh);
        let w = (sw * scale).round().max(1.0);
        let h = (sh * scale).round().max(1.0);
        if w > MAX_RASTER_SIDE as f32 || h > MAX_RASTER_SIDE as f32 {
            return;
        }
        let x = (rect.x + (rect.w - w) / 2.0 + 0.5).floor() as i32;
        let y = (rect.y + (rect.h - h) / 2.0 + 0.5).floor() as i32;
        let Some(r) = self.raster(key, w as u32, h as u32, tint.is_some()) else {
            if !self.refused.contains(key) && !self.deferred {
                log::warn!("svg could not be drawn at {w}x{h}");
            }
            return;
        };
        blit(dst, mask, vis, r, (x, y, w as u32, h as u32), tint);
    }
}

fn channel(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

#[inline]
fn div255(v: u32) -> u32 {
    (v + 127) / 255
}

/// Source-over of a raster onto the premultiplied RGBA canvas, at `(x, y, w, h)`.
/// A raster smaller than that (a user's large picture) is stretched with the
/// nearest pixel.
fn blit(dst: &mut Pixmap, mask: Option<&Mask>, vis: RectPx, r: &Raster, at: (i32, i32, u32, u32), tint: Option<Color>) {
    let (ox, oy, dw, dh) = at;
    let area = RectPx::new(ox, oy, ox + dw as i32, oy + dh as i32);
    let Some(area) = area.intersect(&vis) else { return };
    let width = dst.width() as usize;
    let (ca, premul) = match (r.coverage, tint) {
        (true, Some(c)) => {
            let ca = (channel(c.a) * 255.0).round() as u32;
            (ca, [channel(c.r), channel(c.g), channel(c.b)].map(|v| (v * ca as f32).round() as u32))
        }
        // a coverage raster is only made for a tint; without one nothing is drawn
        (true, None) => return,
        (false, _) => (255, [0; 3]),
    };
    let mask_data = mask.map(|m| m.data());
    let data = dst.data_mut();
    for y in area.y0..area.y1 {
        if y < 0 {
            continue;
        }
        for x in area.x0..area.x1 {
            if x < 0 || x as usize >= width {
                continue;
            }
            let sx = ((x - ox) as u64 * r.w as u64 / dw as u64) as usize;
            let sy = ((y - oy) as u64 * r.h as u64 / dh as u64) as usize;
            let i = sy * r.w as usize + sx;
            let (mut sr, mut sg, mut sb, mut sa) = if r.coverage {
                let c = r.data[i] as u32;
                (div255(premul[0] * c), div255(premul[1] * c), div255(premul[2] * c), div255(ca * c))
            } else {
                let p = &r.data[i * 4..i * 4 + 4];
                (p[0] as u32, p[1] as u32, p[2] as u32, p[3] as u32)
            };
            let pos = y as usize * width + x as usize;
            if let Some(m) = mask_data {
                let cov = m[pos] as u32;
                if cov == 0 {
                    continue;
                }
                if cov != 255 {
                    sr = div255(sr * cov);
                    sg = div255(sg * cov);
                    sb = div255(sb * cov);
                    sa = div255(sa * cov);
                }
            }
            if sa == 0 {
                continue;
            }
            let d = &mut data[pos * 4..pos * 4 + 4];
            let inv = 255 - sa;
            d[0] = (sr + div255(d[0] as u32 * inv)).min(255) as u8;
            d[1] = (sg + div255(d[1] as u32 * inv)).min(255) as u8;
            d[2] = (sb + div255(d[2] as u32 * inv)).min(255) as u8;
            d[3] = (sa + div255(d[3] as u32 * inv)).min(255) as u8;
        }
    }
}

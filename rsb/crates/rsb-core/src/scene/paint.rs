//! The draw list: shadows, background, border, content, then children, with
//! clips and group opacity wrapped around them.

use std::sync::Arc;

use super::{Scene, hit_fill_color, text_style};
use crate::draw::{DrawCmd, DrawItem, DrawList, Fill, shadow_bounds};
use crate::element::ElementKind;
use crate::geom::{Edges, PointF, Radii, RectF};
use crate::style::{Background, ComputedStyle, Dim, RadiusDims};

/// Corner radii for a border box: `%` is a share of the shorter side, and
/// radii that would overlap on a side shrink together (as CSS does).
pub(super) fn resolve_radii(r: &RadiusDims, rect: &RectF, scale: f32) -> Radii {
    let one = |d: Dim| match d {
        Dim::Px(v) => (v * scale).max(0.0),
        Dim::Percent(p) => (p / 100.0 * rect.w.min(rect.h)).max(0.0),
        Dim::Auto => 0.0,
    };
    let mut out = Radii { tl: one(r.tl), tr: one(r.tr), br: one(r.br), bl: one(r.bl) };
    let mut f = 1.0f32;
    for (sum, len) in
        [(out.tl + out.tr, rect.w), (out.bl + out.br, rect.w), (out.tl + out.bl, rect.h), (out.tr + out.br, rect.h)]
    {
        if sum > len && sum > 0.0 {
            f = f.min(len / sum);
        }
    }
    if f < 1.0 {
        out = Radii { tl: out.tl * f, tr: out.tr * f, br: out.br * f, bl: out.bl * f };
    }
    out
}

fn inner_radii(r: Radii, b: &Edges<f32>) -> Radii {
    Radii {
        tl: (r.tl - b.top.max(b.left)).max(0.0),
        tr: (r.tr - b.top.max(b.right)).max(0.0),
        br: (r.br - b.bottom.max(b.right)).max(0.0),
        bl: (r.bl - b.bottom.max(b.left)).max(0.0),
    }
}

fn inset(rect: &RectF, a: &Edges<f32>, b: &Edges<f32>) -> RectF {
    RectF::new(
        rect.x + a.left + b.left,
        rect.y + a.top + b.top,
        (rect.w - a.left - a.right - b.left - b.right).max(0.0),
        (rect.h - a.top - a.bottom - b.top - b.bottom).max(0.0),
    )
}

/// CSS gradient line for a box: `angle` 0 = to top, 90 = to right.
fn gradient_line(rect: &RectF, angle_deg: f32) -> (PointF, PointF) {
    let a = angle_deg.to_radians();
    let (dx, dy) = (a.sin(), -a.cos());
    let len = (rect.w * dx).abs() + (rect.h * dy).abs();
    let (cx, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
    (
        PointF { x: cx - dx * len / 2.0, y: cy - dy * len / 2.0 },
        PointF { x: cx + dx * len / 2.0, y: cy + dy * len / 2.0 },
    )
}

fn union_bounds(items: &[DrawItem]) -> RectF {
    items.iter().fold(RectF::default(), |u, i| u.union(&i.bounds))
}

impl Scene {
    pub(super) fn paint(&self) -> DrawList {
        let g = &self.geometry;
        let mut items = Vec::new();
        if let Some(bar) = self.arena.first() {
            // The bar's whole box is always at least 1/255 opaque, so a
            // layered window keeps receiving the pointer over gaps.
            let rect = bar.laid.rect;
            let radii = resolve_radii(&bar.style.border_radius, &rect, g.scale);
            items.push(DrawItem {
                node: bar.key,
                bounds: rect.outset(1.0),
                cmd: DrawCmd::Fill { rect, radii, fill: Fill::Solid(hit_fill_color()) },
            });
            self.emit(0, &mut items);
        }
        DrawList { width: g.width_px, height: g.height_px, items }
    }

    fn emit(&self, idx: usize, out: &mut Vec<DrawItem>) {
        let n = &self.arena[idx];
        let s: &ComputedStyle = &n.style;
        if s.display_none {
            return;
        }
        let scale = self.geometry.scale;
        let rect = n.laid.rect;
        let is_plate = n.kind == ElementKind::Plate;
        let radii = resolve_radii(&s.border_radius, &rect, scale);
        let opacity = if is_plate { 1.0 } else { s.opacity };
        let draw_self = (s.visible || is_plate) && rect.w > 0.0 && rect.h > 0.0;
        let key = n.key;
        let start = out.len();
        if opacity < 1.0 {
            out.push(DrawItem { node: key, bounds: rect, cmd: DrawCmd::PushOpacity(opacity.max(0.0)) });
        }

        if draw_self {
            for sh in s.box_shadow.iter().rev() {
                if sh.color.a <= 0.0 {
                    continue;
                }
                let spread = sh.spread * scale;
                let r = RectF::new(
                    rect.x + sh.offset_x * scale - spread,
                    rect.y + sh.offset_y * scale - spread,
                    rect.w + 2.0 * spread,
                    rect.h + 2.0 * spread,
                );
                if r.w <= 0.0 || r.h <= 0.0 {
                    continue;
                }
                let grow = |v: f32| (v + spread).max(0.0);
                let radii = Radii { tl: grow(radii.tl), tr: grow(radii.tr), br: grow(radii.br), bl: grow(radii.bl) };
                let blur = sh.blur * scale;
                out.push(DrawItem {
                    node: key,
                    bounds: shadow_bounds(r, blur),
                    cmd: DrawCmd::Shadow { rect: r, radii, blur, color: sh.color },
                });
            }
            let fill = match &s.background {
                Background::None => None,
                Background::Color(c) => (c.a > 0.0).then_some(Fill::Solid(*c)),
                Background::Linear(g) => {
                    let (start, end) = gradient_line(&rect, g.angle_deg);
                    Some(Fill::Linear { start, end, stops: g.stops.clone() })
                }
            };
            if let Some(fill) = fill {
                out.push(DrawItem { node: key, bounds: rect.outset(1.0), cmd: DrawCmd::Fill { rect, radii, fill } });
            }
            let b = n.laid.border;
            if (b.top > 0.0 || b.right > 0.0 || b.bottom > 0.0 || b.left > 0.0) && s.border_color.a > 0.0 {
                out.push(DrawItem {
                    node: key,
                    bounds: rect.outset(1.0),
                    cmd: DrawCmd::Border { rect, radii, widths: b, color: s.border_color },
                });
            }
            let content = inset(&rect, &n.laid.border, &n.laid.padding);
            match n.kind {
                ElementKind::Text | ElementKind::Icon | ElementKind::Plate => {
                    let text = n.laid.text.clone().or_else(|| n.text.clone());
                    if let Some(text) = text.filter(|t| !t.is_empty())
                        && content.w > 0.0
                        && content.h > 0.0
                    {
                        out.push(DrawItem {
                            node: key,
                            bounds: content,
                            cmd: DrawCmd::Text {
                                rect: content,
                                text,
                                style: text_style(s, scale),
                                color: s.color,
                                align: s.text_align,
                            },
                        });
                    }
                }
                ElementKind::Image => {
                    if let Some(img) = &n.image
                        && !self.image_failed.contains(img.path.as_ref())
                        && content.w > 0.0
                        && content.h > 0.0
                    {
                        out.push(DrawItem {
                            node: key,
                            bounds: content.outset(1.0),
                            cmd: DrawCmd::Image {
                                rect: content,
                                key: Arc::clone(&img.path),
                                radii: inner_radii(radii, &n.laid.border),
                            },
                        });
                    }
                }
                _ => {}
            }
        }

        let clip = s.clip || n.laid.clip;
        let clip_rect = inset(&rect, &n.laid.border, &Edges::all(0.0));
        let clip_at = clip.then(|| {
            out.push(DrawItem {
                node: key,
                bounds: clip_rect,
                cmd: DrawCmd::PushClip { rect: clip_rect, radii: inner_radii(radii, &n.laid.border) },
            });
            out.len() - 1
        });
        for &c in &n.children {
            self.emit(c, out);
        }
        if let Some(ci) = clip_at {
            let inner = union_bounds(&out[ci + 1..]);
            let b = clip_rect.intersect(&inner).unwrap_or(RectF::new(clip_rect.x, clip_rect.y, 0.0, 0.0));
            out[ci].bounds = b;
            out.push(DrawItem { node: key, bounds: b, cmd: DrawCmd::PopClip });
        }
        if opacity < 1.0 {
            let inner = union_bounds(&out[start + 1..]);
            let b = if inner.is_empty() { rect } else { inner };
            out[start].bounds = b;
            out.push(DrawItem { node: key, bounds: b, cmd: DrawCmd::PopOpacity });
        }
    }
}

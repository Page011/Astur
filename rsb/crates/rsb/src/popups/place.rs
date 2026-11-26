//! Where a popup window goes and how big it is: the side it asks for, the
//! size its scene reports, the flat edge an attached panel takes, and
//! `rsb_core::popup::place`'s answer. Kept apart from the manager so the
//! rules can be tested without a window.

use rsb_core::geom::{PointF, RectF, RectPx, SizeF};
use rsb_core::layout::TextMeasure;
use rsb_core::popup::{FlatEdges, PlaceInput, Placement, PopupProps, Side, auto_below, place};
use rsb_core::scene::Scene;
use rsb_win::MonitorInfo;

/// What the manager needs to make or move a window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// Physical screen px (virtual-screen coordinates, before any offscreen shift).
    pub rect: RectPx,
    pub side: Side,
}

/// The side asked for before the surface size is known. `Auto` follows
/// `place`: below when the anchor is in the top half of its monitor.
pub fn side_guess(placement: Placement, anchor: &RectF, monitor: &RectPx) -> Side {
    match placement {
        Placement::Below => Side::Below,
        Placement::Above => Side::Above,
        Placement::Left => Side::Left,
        Placement::Right => Side::Right,
        Placement::Screen => Side::Screen,
        Placement::Pointer { .. } => Side::Pointer,
        Placement::Auto => {
            if auto_below(anchor, monitor) {
                Side::Below
            } else {
                Side::Above
            }
        }
    }
}

/// The width an attached popup takes: the anchor's flat edge on the side it
/// opens to. None for an unattached popup, a side that is not above or
/// below, or an anchor with no flat edge there.
pub fn attach_width(attach: bool, side: Side, flat: &FlatEdges) -> Option<f32> {
    if !attach {
        return None;
    }
    let span = match side {
        Side::Below => flat.bottom,
        Side::Above => flat.top,
        _ => None,
    }?;
    let w = span.1 - span.0;
    (w >= 1.0).then_some(w)
}

/// Logical px a popup of a widget with no `overlay` permission may measure each way.
pub const UNTRUSTED_MAX: f32 = 900.0;

/// A popup is never larger than the work area of its monitor. One that a
/// widget without `overlay` opens also stays at most `UNTRUSTED_MAX` logical
/// px each way, 60 % of the work area wide and 90 % tall, so it can never
/// pass for a full-screen sign-in.
fn capped(size: SizeF, work: &RectPx, scale: f32, trusted: bool) -> SizeF {
    let (mut max_w, mut max_h) = (work.width() as f32, work.height() as f32);
    if !trusted {
        max_w = max_w.min(UNTRUSTED_MAX * scale).min(max_w * 0.6);
        max_h = max_h.min(UNTRUSTED_MAX * scale).min(max_h * 0.9);
    }
    SizeF { w: size.w.min(max_w), h: size.h.min(max_h) }
}

/// A window of at least one pixel each way: a popup with nothing in it still
/// gets a surface to draw its (empty) frame into.
fn at_least_one(size: SizeF) -> SizeF {
    SizeF { w: size.w.max(1.0), h: size.h.max(1.0) }
}

/// Everything `plan` reads that is not the scene.
pub struct Wanted<'a> {
    pub props: &'a PopupProps,
    /// The anchor's border box once its transitions end, physical screen px.
    pub anchor: RectF,
    /// Its flat edges, physical screen px.
    pub flat: FlatEdges,
    pub monitor: &'a MonitorInfo,
    pub scale: f32,
    pub blur: bool,
    /// The popup's widget is a built-in or holds `overlay` (`PopupProps::trusted`).
    pub trusted: bool,
}

/// Sizes the scene for its side and places the window. An overlay's content
/// window goes where the monitor-sized root's origin puts it; every other
/// popup is placed against its anchor, flipped when it does not fit, then
/// sized again for the side it ended up on.
pub fn plan(scene: &mut Scene, text: &mut dyn TextMeasure, w: &Wanted<'_>) -> Layout {
    let mon = w.monitor;
    if w.props.placement == Placement::Screen {
        scene.set_popup_surface(Side::Screen, w.blur, None);
        let size = at_least_one(scene.natural_size(text));
        let off: PointF = scene.root_offset();
        let x0 = mon.rect.x0 - off.x.round() as i32;
        let y0 = mon.rect.y0 - off.y.round() as i32;
        return Layout {
            rect: RectPx::new(x0, y0, x0 + size.w.round() as i32, y0 + size.h.round() as i32),
            side: Side::Screen,
        };
    }
    let input = |size: SizeF| PlaceInput {
        anchor: w.anchor,
        size,
        work: mon.work,
        monitor: mon.rect,
        placement: w.props.placement,
        align: w.props.align,
        offset: w.props.offset * w.scale,
        scale: w.scale,
        attach: w.props.attach.then_some(w.flat),
    };
    // The box is what sits against the anchor; the window also holds the shadow around it.
    let measure = |scene: &mut Scene, text: &mut dyn TextMeasure| {
        let surface = at_least_one(capped(scene.natural_size(text), &mon.work, w.scale, w.trusted));
        let boxed = at_least_one(capped(scene.root_box_size(), &mon.work, w.scale, w.trusted));
        (surface, boxed, scene.root_offset())
    };
    let mut side = side_guess(w.props.placement, &w.anchor, &mon.rect);
    scene.set_popup_surface(side, w.blur, attach_width(w.props.attach, side, &w.flat));
    let (mut surface, mut boxed, mut off) = measure(scene, text);
    let mut placed = place(&input(boxed));
    if placed.side != side {
        side = placed.side;
        scene.set_popup_surface(side, w.blur, attach_width(w.props.attach, side, &w.flat));
        (surface, boxed, off) = measure(scene, text);
        placed = place(&input(boxed));
    }
    let (x0, y0) = (placed.rect.x0 - off.x.round() as i32, placed.rect.y0 - off.y.round() as i32);
    let rect = RectPx::new(x0, y0, x0 + surface.w.round() as i32, y0 + surface.h.round() as i32);
    Layout { rect, side: placed.side }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsb_core::popup::PopupAlign;

    fn mon(w: i32, h: i32) -> RectPx {
        RectPx::new(0, 0, w, h)
    }

    #[test]
    fn auto_goes_below_in_the_top_half_and_above_in_the_bottom_half() {
        let top = RectF::new(10.0, 0.0, 40.0, 32.0);
        let bottom = RectF::new(10.0, 1048.0, 40.0, 32.0);
        assert_eq!(side_guess(Placement::Auto, &top, &mon(1920, 1080)), Side::Below);
        assert_eq!(side_guess(Placement::Auto, &bottom, &mon(1920, 1080)), Side::Above);
        // An anchor whose middle is exactly the monitor's middle goes below, in the guess and in `place`.
        let middle = RectF::new(10.0, 524.0, 40.0, 32.0);
        assert_eq!(side_guess(Placement::Auto, &middle, &mon(1920, 1080)), Side::Below);
        assert!(auto_below(&middle, &mon(1920, 1080)));
        assert_eq!(side_guess(Placement::Left, &top, &mon(1920, 1080)), Side::Left);
        assert_eq!(side_guess(Placement::Pointer { x: 1.0, y: 2.0 }, &top, &mon(1920, 1080)), Side::Pointer);
        assert_eq!(side_guess(Placement::Screen, &top, &mon(1920, 1080)), Side::Screen);
    }

    #[test]
    fn an_attached_popup_takes_the_flat_edge_of_the_side_it_opens_to() {
        let flat = FlatEdges { top: Some((100.0, 140.0)), bottom: Some((90.0, 190.0)) };
        assert_eq!(attach_width(true, Side::Below, &flat), Some(100.0));
        assert_eq!(attach_width(true, Side::Above, &flat), Some(40.0));
        assert_eq!(attach_width(true, Side::Left, &flat), None, "only above and below attach");
        assert_eq!(attach_width(false, Side::Below, &flat), None);
        assert_eq!(attach_width(true, Side::Below, &FlatEdges::default()), None, "a chevron tip has no flat edge");
        let sliver = FlatEdges { top: None, bottom: Some((10.0, 10.4)) };
        assert_eq!(attach_width(true, Side::Below, &sliver), None);
    }

    #[test]
    fn a_popup_never_exceeds_the_work_area_and_never_has_no_pixels() {
        let work = RectPx::new(0, 0, 800, 600);
        let c = capped(SizeF { w: 5000.0, h: 100.0 }, &work, 1.0, true);
        assert_eq!((c.w, c.h), (800.0, 100.0));
        let z = at_least_one(SizeF { w: 0.0, h: 0.0 });
        assert_eq!((z.w, z.h), (1.0, 1.0));
    }

    #[test]
    fn a_popup_of_a_widget_without_overlay_stays_small_and_never_covers_the_screen() {
        let huge = SizeF { w: 20000.0, h: 20000.0 };
        let full = RectPx::new(0, 0, 3840, 2160);
        let c = capped(huge, &full, 1.5, false);
        assert_eq!((c.w, c.h), (1350.0, 1350.0), "900 logical px at 150 %");
        let small = RectPx::new(0, 0, 1280, 720);
        let c = capped(huge, &small, 1.0, false);
        assert_eq!((c.w, c.h), (768.0, 648.0), "60 % wide, 90 % tall");
        let c = capped(huge, &small, 1.0, true);
        assert_eq!((c.w, c.h), (1280.0, 720.0), "a built-in may fill the work area");
        let dash = capped(SizeF { w: 526.0, h: 540.0 }, &RectPx::new(0, 0, 1920, 1040), 1.0, false);
        assert_eq!((dash.w, dash.h), (526.0, 540.0), "a dashboard-sized panel is not cut");
    }

    #[test]
    fn the_place_inputs_the_manager_builds_flip_and_clamp() {
        // The manager hands `place` exactly this; the cases pin the four edges.
        let work = RectPx::new(0, 40, 1000, 600);
        let base = |anchor: RectF, placement| PlaceInput {
            anchor,
            size: SizeF { w: 300.0, h: 200.0 },
            work,
            monitor: RectPx::new(0, 0, 1000, 600),
            placement,
            align: PopupAlign::Center,
            offset: 0.0,
            scale: 1.0,
            attach: None,
        };
        // Left edge: centred on an anchor at x 0..40 would start at -130; it stops 8 px short of the edge.
        let p = place(&base(RectF::new(0.0, 0.0, 40.0, 40.0), Placement::Below));
        assert_eq!((p.side, p.rect.x0, p.rect.y0), (Side::Below, 8, 40));
        // Right edge.
        let p = place(&base(RectF::new(960.0, 0.0, 40.0, 40.0), Placement::Below));
        assert_eq!(p.rect.x1, 992);
        // Bottom edge: no room below, more above, so it flips.
        let p = place(&base(RectF::new(400.0, 560.0, 40.0, 40.0), Placement::Below));
        assert_eq!((p.side, p.rect.y1), (Side::Above, 560));
        // Top edge: no room above, more below, so it flips.
        let p = place(&base(RectF::new(400.0, 0.0, 40.0, 40.0), Placement::Above));
        assert_eq!((p.side, p.rect.y0), (Side::Below, 40));
    }
}

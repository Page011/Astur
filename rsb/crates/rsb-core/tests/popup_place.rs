use rsb_core::geom::{RectF, RectPx, SizeF};
use rsb_core::popup::{FlatEdges, PlaceInput, Placement, PopupAlign, Side, place};

fn base(anchor: RectF, w: f32, h: f32, placement: Placement) -> PlaceInput {
    PlaceInput {
        anchor,
        size: SizeF { w, h },
        work: RectPx::new(0, 40, 1000, 800),
        monitor: RectPx::new(0, 0, 1000, 800),
        placement,
        align: PopupAlign::Center,
        offset: 0.0,
        scale: 1.0,
        attach: None,
    }
}

#[test]
fn below_and_above_keep_their_side_when_they_fit_and_flip_when_the_other_has_more_room() {
    let mid = RectF::new(480.0, 400.0, 40.0, 20.0);
    let below = place(&base(mid, 100.0, 100.0, Placement::Below));
    assert_eq!((below.side, below.rect), (Side::Below, RectPx::new(450, 420, 550, 520)));
    let above = place(&base(mid, 100.0, 100.0, Placement::Above));
    assert_eq!((above.side, above.rect), (Side::Above, RectPx::new(450, 300, 550, 400)));
    // Near the top: Above does not fit (room 360 - 40 < 400) and below has more room.
    let top = RectF::new(480.0, 60.0, 40.0, 20.0);
    let p = place(&base(top, 100.0, 300.0, Placement::Above));
    assert_eq!(p.side, Side::Below);
    assert_eq!(p.rect.y0, 80);
    // Near the bottom: Below flips up.
    let low = RectF::new(480.0, 740.0, 40.0, 20.0);
    let p = place(&base(low, 100.0, 300.0, Placement::Below));
    assert_eq!(p.side, Side::Above);
    assert_eq!(p.rect.y1, 740);
    // Neither side fits: the side with more room is used and the popup is moved, not shrunk.
    let p = place(&base(RectF::new(480.0, 300.0, 40.0, 20.0), 100.0, 700.0, Placement::Below));
    assert_eq!(p.rect.height(), 700);
    assert!(p.rect.y0 >= 40 && p.rect.y1 <= 800, "{:?}", p.rect);
}

#[test]
fn left_and_right_flip_and_align_down_the_anchor() {
    let a = RectF::new(500.0, 300.0, 40.0, 60.0);
    let right = place(&base(a, 100.0, 40.0, Placement::Right));
    assert_eq!((right.side, right.rect), (Side::Right, RectPx::new(540, 310, 640, 350)));
    let left = place(&base(a, 100.0, 40.0, Placement::Left));
    assert_eq!((left.side, left.rect), (Side::Left, RectPx::new(400, 310, 500, 350)));
    let edge = RectF::new(930.0, 300.0, 40.0, 60.0);
    let flipped = place(&base(edge, 100.0, 40.0, Placement::Right));
    assert_eq!((flipped.side, flipped.rect.x1), (Side::Left, 930));
    let edge = RectF::new(10.0, 300.0, 40.0, 60.0);
    let flipped = place(&base(edge, 100.0, 40.0, Placement::Left));
    assert_eq!((flipped.side, flipped.rect.x0), (Side::Right, 50));
    // Clamped along the edge: an anchor at the very top leaves the popup inside the work area, 8 px short of it.
    let high = RectF::new(500.0, 40.0, 40.0, 10.0);
    assert_eq!(place(&base(high, 100.0, 100.0, Placement::Right)).rect.y0, 48);
}

#[test]
fn across_alignment_and_clamping_into_the_work_area() {
    let a = RectF::new(480.0, 100.0, 40.0, 20.0);
    for (align, x0) in [(PopupAlign::Start, 480), (PopupAlign::Center, 450), (PopupAlign::End, 420)] {
        let mut i = base(a, 100.0, 50.0, Placement::Below);
        i.align = align;
        assert_eq!(place(&i).rect.x0, x0, "{align:?}");
    }
    let mut i = base(RectF::new(960.0, 100.0, 40.0, 20.0), 300.0, 50.0, Placement::Below);
    assert_eq!(place(&i).rect.x1, 992, "moved in from the right, 8 px short of the edge");
    i.anchor = RectF::new(0.0, 100.0, 20.0, 20.0);
    assert_eq!(place(&i).rect.x0, 8, "and from the left");
}

#[test]
fn offset_separates_and_a_negative_offset_overlaps() {
    let a = RectF::new(480.0, 100.0, 40.0, 20.0);
    let mut i = base(a, 100.0, 50.0, Placement::Below);
    i.offset = 6.0;
    assert_eq!(place(&i).rect.y0, 126);
    i.offset = -4.0;
    assert_eq!(place(&i).rect.y0, 116);
    i.placement = Placement::Above;
    i.offset = 6.0;
    assert_eq!(place(&i).rect.y1, 94);
}

#[test]
fn a_pointer_menu_goes_right_and_down_and_flips_left_and_up_at_the_edges() {
    let a = RectF::new(100.0, 100.0, 200.0, 40.0);
    let p = place(&base(a, 120.0, 80.0, Placement::Pointer { x: 30.0, y: 10.0 }));
    assert_eq!((p.side, p.rect), (Side::Pointer, RectPx::new(130, 110, 250, 190)));
    let right_edge = RectF::new(900.0, 100.0, 90.0, 40.0);
    let p = place(&base(right_edge, 120.0, 80.0, Placement::Pointer { x: 80.0, y: 10.0 }));
    assert_eq!(p.rect.x1, 980, "flipped left of the pointer");
    let bottom = RectF::new(100.0, 760.0, 90.0, 30.0);
    let p = place(&base(bottom, 120.0, 80.0, Placement::Pointer { x: 10.0, y: 20.0 }));
    assert_eq!(p.rect.y1, 780, "flipped above it");
    // Too big either way: clamped.
    let p = place(&base(RectF::new(0.0, 40.0, 50.0, 20.0), 120.0, 900.0, Placement::Pointer { x: 0.0, y: 0.0 }));
    assert_eq!(p.rect.y0, 40, "never above the work area top: {:?}", p.rect);
}

#[test]
fn screen_is_the_monitor_whatever_the_anchor() {
    let mut i = base(RectF::new(300.0, 10.0, 20.0, 20.0), 5.0, 5.0, Placement::Screen);
    i.monitor = RectPx::new(1000, 0, 2000, 800);
    let p = place(&i);
    assert_eq!((p.side, p.rect), (Side::Screen, RectPx::new(1000, 0, 2000, 800)));
}

#[test]
fn at_150_percent_the_pointer_point_scales_and_whole_pixels_are_kept() {
    let mut i = base(RectF::new(101.0, 55.0, 61.0, 39.0), 150.0, 90.0, Placement::Pointer { x: 10.0, y: 6.0 });
    i.scale = 1.5;
    let p = place(&i);
    // 10 and 6 logical px are 15 and 9 device px from the anchor's corner.
    assert_eq!((p.rect.x0, p.rect.y0), (116, 64));
    let mut i = base(RectF::new(100.4, 36.5, 52.3, 30.0), 99.6, 49.6, Placement::Below);
    i.scale = 1.5;
    i.offset = 4.0 * 1.5;
    let p = place(&i);
    assert_eq!((p.rect.width(), p.rect.height()), (100, 50), "rounded size");
    assert_eq!(p.rect.y0, 73, "66.5 + 6, rounded");
    // An attached panel takes the flat edge exactly and the CSS width is not used.
    i.attach = Some(FlatEdges { top: None, bottom: Some((100.0, 160.0)) });
    let p = place(&i);
    assert_eq!((p.rect.x0, p.rect.x1), (100, 160));
}

//! Panel-sized draw lists for the frame-time benchmark: the pages a popup
//! panel shows, built at a scale (1 at 96 dpi, 2 at 192 dpi).
#![allow(dead_code)]

use rsb_core::draw::DrawItem;
use rsb_core::geom::{Edges, Radii, RectF};
use rsb_core::style::TextAlign;

use super::*;

const MONO: &[&str] = &["JetBrains Mono", "monospace"];

struct Ids(u64);

impl Ids {
    fn next(&mut self) -> u64 {
        self.0 += 10;
        self.0
    }
}

fn panel_background(id: u64, s: f32) -> DrawItem {
    let radii = Radii { tl: 0.0, tr: 0.0, br: 10.0 * s, bl: 10.0 * s };
    fill_radii(id, rect(0.0, 0.0, 500.0 * s, 560.0 * s), radii, col("#0f1317"))
}

pub fn panel_list(scale: f32, page: fn(f32) -> Vec<DrawItem>) -> rsb_core::draw::DrawList {
    list((500.0 * scale) as u32, (560.0 * scale) as u32, page(scale))
}

/// The list page: a header card with a search input and a refresh button, a
/// card of eleven rows (icon, name, value, two icon buttons) and a scrollbar.
pub fn page_list(s: f32) -> Vec<DrawItem> {
    let fg = col("#e0e5e8");
    let muted = col("#c1c6c9");
    let accent = col("#81cbea");
    let body = style(MONO, 14.0 * s, 400);
    let mut ids = Ids(0);
    let mut items = vec![panel_background(ids.next(), s)];
    let id = ids.next();
    items.push(fill(id, rect(10.0 * s, 10.0 * s, 480.0 * s, 37.0 * s), 8.0 * s, col("#1b1f22")));
    items.push(svg_item(id + 1, rect(20.0 * s, 20.0 * s, 16.0 * s, 16.0 * s), "dial", Some(muted)));
    items.push(fill(id + 2, rect(100.0 * s, 15.0 * s, 376.0 * s, 27.0 * s), 9.0 * s, col("#252b2e")));
    let search = rect(100.0 * s, 15.0 * s, 376.0 * s, 27.0 * s);
    items.push(text(id + 3, search, "Search for a device...", &body, col("#8e9497"), TextAlign::Center));
    items.push(fill(id + 4, rect(50.0 * s, 14.0 * s, 28.0 * s, 28.0 * s), 6.0 * s, col("#3a4245")));
    items.push(svg_item(id + 5, rect(56.0 * s, 20.0 * s, 16.0 * s, 16.0 * s), "bolt", Some(fg)));
    let card = rect(10.0 * s, 57.0 * s, 480.0 * s, 493.0 * s);
    items.push(fill(ids.next(), card, 8.0 * s, col("#1b1f22")));
    // the rows scroll: they are drawn inside the card's clip
    let clip = ids.next();
    items.push(push_clip(clip, card, 8.0 * s));
    let names = [
        "Headphones",
        "Keyboard",
        "Speaker",
        "Mouse",
        "Phone",
        "Watch",
        "Tablet",
        "Camera",
        "Console",
        "Remote",
        "Display",
    ];
    for (i, name) in names.iter().enumerate() {
        let y = (62.0 + i as f32 * 43.0) * s;
        let id = ids.next();
        let r = rect(20.0 * s, y, 460.0 * s, 38.0 * s);
        items.push(fill(id, r, 8.0 * s, if i == 2 { col("#2f3639") } else { col("#252b2e") }));
        if i == 1 {
            items.push(border(id + 1, r, 8.0 * s, Edges::all(s), accent));
        }
        let ink = if i == 1 { accent } else { fg };
        items.push(svg_item(
            id + 2,
            rect(32.0 * s, y + 11.0 * s, 16.0 * s, 16.0 * s),
            if i % 2 == 0 { "bolt" } else { "dial" },
            Some(ink),
        ));
        items.push(text(id + 3, rect(58.0 * s, y + 9.0 * s, 260.0 * s, 20.0 * s), name, &body, ink, TextAlign::Start));
        items.push(text(
            id + 4,
            rect(320.0 * s, y + 9.0 * s, 80.0 * s, 20.0 * s),
            &format!("{}%", 100 - i * 7),
            &body,
            muted,
            TextAlign::End,
        ));
        items.push(svg_item(id + 5, rect(414.0 * s, y + 11.0 * s, 16.0 * s, 16.0 * s), "dial", Some(muted)));
        items.push(svg_item(id + 6, rect(450.0 * s, y + 11.0 * s, 16.0 * s, 16.0 * s), "bolt", Some(muted)));
    }
    items.push(pop_clip(clip, card.outset(1.0)));
    items.push(fill(ids.next(), rect(486.0 * s, 70.0 * s, 4.0 * s, 160.0 * s), 2.0 * s, col("#e0e5e859")));
    items
}

/// The tile page: six 80 x 80 tiles (one hovered) and a slider row.
pub fn page_tiles(s: f32) -> Vec<DrawItem> {
    let fg = col("#e0e5e8");
    let accent = col("#81cbea");
    let body = style(MONO, 14.0 * s, 400);
    let mut ids = Ids(0);
    let mut items = vec![panel_background(ids.next(), s)];
    for i in 0..6 {
        let r = rect((75.0 + (i % 3) as f32 * 110.0) * s, (90.0 + (i / 3) as f32 * 110.0) * s, 80.0 * s, 80.0 * s);
        let id = ids.next();
        if i == 1 {
            items.push(fill(id, r, 14.0 * s, col("#1b1f22")));
        }
        let icon = rect(r.x + 19.0 * s, r.y + 19.0 * s, 42.0 * s, 42.0 * s);
        items.push(svg_item(
            id + 1,
            icon,
            if i % 2 == 0 { "bolt" } else { "dial" },
            Some(if i == 1 { accent } else { fg }),
        ));
    }
    let id = ids.next();
    items.push(fill(id, rect(10.0 * s, 330.0 * s, 480.0 * s, 38.0 * s), 8.0 * s, col("#252b2e")));
    items.push(fill(id + 1, rect(10.0 * s, 330.0 * s, 260.0 * s, 38.0 * s), 8.0 * s, col("#ffffff12")));
    items.push(text(id + 2, rect(22.0 * s, 339.0 * s, 300.0 * s, 20.0 * s), "Speakers", &body, fg, TextAlign::Start));
    items
}

fn moved(r: RectF, dy: f32) -> RectF {
    rect(r.x, r.y + dy, r.w, r.h)
}

/// The list page with a menu and a tooltip on it: the two shadows the
/// designed popups have at most.
pub fn page_with_menu(s: f32) -> Vec<DrawItem> {
    let mut items = page_list(s);
    let menu = rect(300.0 * s, 120.0 * s, 170.0 * s, 130.0 * s);
    items.push(shadow(9001, moved(menu, 6.0 * s), 8.0 * s, 18.0 * s, col("#00000073")));
    items.push(fill(9002, menu, 8.0 * s, col("#0f1317")));
    let tip = rect(120.0 * s, 300.0 * s, 120.0 * s, 28.0 * s);
    items.push(shadow(9003, moved(tip, 4.0 * s), 6.0 * s, 12.0 * s, col("#00000066")));
    items.push(fill(9004, tip, 6.0 * s, col("#1b1f22")));
    items
}

/// A stress page with far more shadows than any designed page: six tiles
/// with a blurred shadow (one a hexagon outline), three slider knobs.
pub fn page_stress(s: f32) -> Vec<DrawItem> {
    let fg = col("#e0e5e8");
    let accent = col("#81cbea");
    let body = style(MONO, 14.0 * s, 400);
    let mut ids = Ids(0);
    let mut items = vec![panel_background(ids.next(), s)];
    for i in 0..6 {
        let r = rect((18.0 + (i % 3) as f32 * 156.0) * s, (64.0 + (i / 3) as f32 * 76.0) * s, 146.0 * s, 66.0 * s);
        let id = ids.next();
        if i == 0 {
            let d = 14.0 * s;
            let pts = [
                (r.x + d, r.y),
                (r.right() - d, r.y),
                (r.right(), r.y + r.h / 2.0),
                (r.right() - d, r.bottom()),
                (r.x + d, r.bottom()),
                (r.x, r.y + r.h / 2.0),
            ];
            items.push(shadow_path(id, poly(&pts), 10.0 * s, col("#00000066")));
            items.push(fill_path(id + 1, poly(&pts), accent));
        } else {
            items.push(shadow(id, r, 10.0 * s, 10.0 * s, col("#00000066")));
            items.push(fill(id + 1, r, 10.0 * s, col("#2f3639")));
        }
        items.push(svg_item(id + 2, rect(r.x + 12.0 * s, r.y + 12.0 * s, 20.0 * s, 20.0 * s), "bolt", Some(fg)));
        items.push(text(
            id + 3,
            rect(r.x + 12.0 * s, r.y + 38.0 * s, 120.0 * s, 20.0 * s),
            "Dashboard",
            &body,
            fg,
            TextAlign::Start,
        ));
    }
    for i in 0..3 {
        let y = (222.0 + i as f32 * 30.0) * s;
        let id = ids.next();
        let knob = rect((160.0 + 60.0 * i as f32) * s, y + 2.0 * s, 14.0 * s, 14.0 * s);
        items.push(fill(id, rect(48.0 * s, y + 7.0 * s, 360.0 * s, 4.0 * s), 2.0 * s, col("#2f3639")));
        items.push(shadow(id + 1, knob, 7.0 * s, 4.0 * s, col("#0000008c")));
        items.push(fill(id + 2, knob, 7.0 * s, fg));
    }
    items
}

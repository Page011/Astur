//! What the UI thread keeps per bar window.

use std::collections::VecDeque;
use std::time::Instant;

use rsb_core::geom::RectPx;
use rsb_core::style::Backdrop;
use windows::Win32::Foundation::{HWND, RECT};

use crate::BarWindowSpec;
use crate::dib::Dib;

pub(crate) struct BarState {
    pub hwnd: HWND,
    pub spec: BarWindowSpec,
    /// The DPI the bar is laid out for. Starts as the monitor's; changes with
    /// `WM_DPICHANGED`.
    pub dpi: u32,
    /// The monitor's own DPI at the last look, to tell a real DPI change on a
    /// display change from an unchanged one.
    pub monitor_dpi: u32,
    /// DPI / 96 of the surface the renderer was last told about.
    pub scale: f32,
    /// The window rectangle, physical px, virtual-screen coordinates.
    pub window: RectPx,
    pub dib: Dib,
    /// The next present must update the whole window (after creation or a
    /// resize the layered surface holds nothing valid).
    pub needs_full: bool,
    pub presented: bool,
    /// `set_visible` state (the user's toggle).
    pub want_visible: bool,
    /// What `ShowWindow` was last told.
    pub shown: bool,
    /// A fullscreen app is in front.
    pub fs_active: bool,
    /// The rectangle the shell last granted with `ABM_SETPOS`.
    pub last_grant: Option<RECT>,
    pub setpos_calls: u32,
    pub registrations: u32,
    pub recent_setpos: VecDeque<Instant>,
    pub loop_warned: bool,
    pub backdrop: Backdrop,
}

/// Window and appbar numbers for one bar, from its spec, DPI and monitor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Metrics {
    /// Bar height, physical px.
    pub height: i32,
    /// Margin around a floating bar, physical px (0 when not floating).
    pub margin: i32,
}

impl Metrics {
    pub fn new(spec: &BarWindowSpec, dpi: u32) -> Metrics {
        let scale = dpi as f32 / 96.0;
        let height = ((spec.height as f32 * scale).round() as i32).max(1);
        let margin = if spec.floating { (spec.margin as f32 * scale).round() as i32 } else { 0 };
        Metrics { height, margin }
    }

    /// How much screen the appbar claims at its edge: the bar plus the margin.
    pub fn reserved(&self) -> i32 {
        self.height + self.margin
    }
}

/// The window rectangle inside the granted (or would-be) appbar rectangle
/// `grant`: the whole of it for a bar that is not floating, inset by the
/// margin for one that is.
pub(crate) fn window_in_grant(spec: &BarWindowSpec, m: Metrics, grant: RectPx) -> RectPx {
    use rsb_core::config::Position;
    if !spec.floating {
        return match spec.position {
            Position::Top => RectPx::new(grant.x0, grant.y0, grant.x1, grant.y0 + m.height),
            Position::Bottom => RectPx::new(grant.x0, grant.y1 - m.height, grant.x1, grant.y1),
        };
    }
    let x0 = grant.x0 + m.margin;
    let x1 = (grant.x1 - m.margin).max(x0 + 1);
    match spec.position {
        Position::Top => RectPx::new(x0, grant.y0 + m.margin, x1, grant.y0 + m.margin + m.height),
        Position::Bottom => RectPx::new(x0, grant.y1 - m.margin - m.height, x1, grant.y1 - m.margin),
    }
}

#[cfg(test)]
mod tests {
    use rsb_core::config::{FullscreenMode, Position};

    use super::*;

    fn spec(position: Position, floating: bool) -> BarWindowSpec {
        BarWindowSpec {
            name: "main".into(),
            monitor: "\\\\.\\DISPLAY1".into(),
            position,
            height: 32,
            floating,
            margin: 8,
            reserve: true,
            fullscreen: FullscreenMode::Below,
        }
    }

    #[test]
    fn metrics_scale_with_dpi() {
        let s = spec(Position::Top, true);
        assert_eq!(Metrics::new(&s, 96), Metrics { height: 32, margin: 8 });
        assert_eq!(Metrics::new(&s, 120), Metrics { height: 40, margin: 10 });
        assert_eq!(Metrics::new(&s, 144), Metrics { height: 48, margin: 12 });
        assert_eq!(Metrics::new(&spec(Position::Top, false), 120).margin, 0);
        assert_eq!(Metrics::new(&s, 120).reserved(), 50);
    }

    #[test]
    fn flat_bar_fills_the_grant_at_its_edge() {
        let m = Metrics::new(&spec(Position::Top, false), 96);
        let grant = RectPx::new(0, 0, 1920, 32);
        assert_eq!(window_in_grant(&spec(Position::Top, false), m, grant), RectPx::new(0, 0, 1920, 32));
        let bottom = RectPx::new(0, 1048, 1920, 1080);
        assert_eq!(window_in_grant(&spec(Position::Bottom, false), m, bottom), RectPx::new(0, 1048, 1920, 1080));
    }

    #[test]
    fn floating_bar_is_inset_by_the_margin() {
        let s = spec(Position::Top, true);
        let m = Metrics::new(&s, 96);
        // Reserved: 8 margin + 32 bar.
        let grant = RectPx::new(0, 0, 1920, m.reserved());
        assert_eq!(window_in_grant(&s, m, grant), RectPx::new(8, 8, 1912, 40));
        let sb = spec(Position::Bottom, true);
        let grant = RectPx::new(0, 1080 - m.reserved(), 1920, 1080);
        assert_eq!(window_in_grant(&sb, m, grant), RectPx::new(8, 1040, 1912, 1072));
    }
}

//! The window layer with no windows: bars and popups are rectangles and pixel
//! buffers in memory, events that the real layer would get from Windows are
//! queued for the caller, and a frame deadline is only remembered. The
//! timeline renderer runs the real app against it, so a popup is placed,
//! attached, drawn and closed by the same code the live bar runs.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;
use std::time::{Duration, Instant};

use rsb_core::color::Color;
use rsb_core::config::Position;
use rsb_core::geom::RectPx;
use rsb_core::pixels::PixelsMut;
use rsb_core::scene::BarGeometry;
use rsb_core::style::{Backdrop, Cursor};
use rsb_render::Canvas;
use rsb_win::{BarId, BarWindowSpec, DimSpec, HitRegion, HookStats, MonitorInfo, PopupId, PopupWindowSpec};
use rsb_win::{SurfaceRole, TestInput, UiEvent, WinError};

use crate::winlayer::{Draw, Win};

/// One bar window.
struct HBar {
    id: BarId,
    spec: BarWindowSpec,
    scale: f32,
    rect: RectPx,
    pixels: Vec<u8>,
    visible: bool,
    presented: bool,
    reserved: bool,
}

/// An overlay's dim: one colour whose alpha is faded.
#[derive(Clone, Copy, Debug)]
struct Dim {
    color: Color,
    shown: bool,
    paints: u32,
    rgb: (u8, u8, u8),
}

/// One popup, menu, tooltip or overlay window (or an overlay's dim).
struct HPopup {
    id: PopupId,
    role: SurfaceRole,
    rect: RectPx,
    scale: f32,
    pixels: Vec<u8>,
    presented: bool,
    focusable: bool,
    dim: Option<Dim>,
    name: String,
}

/// Where a surface is on the screen and what it holds, for the composer.
pub struct Layer<'a> {
    pub rect: RectPx,
    pub pixels: Option<&'a [u8]>,
    /// A dim: straight colour and alpha 0..=1, painted over everything below.
    pub dim: Option<Color>,
}

pub struct Headless {
    monitors: Vec<MonitorInfo>,
    bars: Vec<HBar>,
    popups: Vec<HPopup>,
    next_bar: u32,
    next_popup: u32,
    events: RefCell<VecDeque<UiEvent>>,
    deadline: Option<Instant>,
    quit: bool,
    warned_dpi: RefCell<bool>,
}

fn byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// `window_in_grant` of the real layer: the bar's window inside the strip it reserves.
fn window_in_strip(spec: &BarWindowSpec, height: i32, margin: i32, strip: RectPx) -> RectPx {
    if !spec.floating {
        return match spec.position {
            Position::Top => RectPx::new(strip.x0, strip.y0, strip.x1, strip.y0 + height),
            Position::Bottom => RectPx::new(strip.x0, strip.y1 - height, strip.x1, strip.y1),
        };
    }
    let x0 = strip.x0 + margin;
    let x1 = (strip.x1 - margin).max(x0 + 1);
    match spec.position {
        Position::Top => RectPx::new(x0, strip.y0 + margin, x1, strip.y0 + margin + height),
        Position::Bottom => RectPx::new(x0, strip.y1 - margin - height, x1, strip.y1 - margin),
    }
}

impl Headless {
    /// One monitor per entry of `screens` (width, height, dpi), side by side from the origin.
    pub fn new(screens: &[(i32, i32, u32)]) -> Headless {
        let mut x = 0;
        let monitors = screens
            .iter()
            .enumerate()
            .map(|(i, &(w, h, dpi))| {
                let rect = RectPx::new(x, 0, x + w, h);
                x += w;
                MonitorInfo {
                    index: i as u32,
                    handle: 0x1000 + i as u64,
                    device: format!("\\\\.\\DISPLAY{}", i + 1),
                    primary: i == 0,
                    rect,
                    work: rect,
                    dpi,
                }
            })
            .collect();
        Headless {
            monitors,
            bars: Vec::new(),
            popups: Vec::new(),
            next_bar: 1,
            next_popup: 1,
            events: RefCell::new(VecDeque::new()),
            deadline: None,
            quit: false,
            warned_dpi: RefCell::new(false),
        }
    }

    /// The next event the real layer would have delivered.
    pub fn next_event(&mut self) -> Option<UiEvent> {
        self.events.borrow_mut().pop_front()
    }

    /// The pending frame deadline, if any.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Takes the deadline: it fires now.
    pub fn take_deadline(&mut self) -> Option<Instant> {
        self.deadline.take()
    }

    pub fn quit_requested(&self) -> bool {
        self.quit
    }

    /// The whole virtual screen.
    pub fn screen(&self) -> RectPx {
        let x1 = self.monitors.iter().map(|m| m.rect.x1).max().unwrap_or(0);
        let y1 = self.monitors.iter().map(|m| m.rect.y1).max().unwrap_or(0);
        RectPx::new(0, 0, x1, y1)
    }

    /// Whether any bar has been drawn at least once.
    pub fn bar_presented(&self, bar: BarId) -> bool {
        self.bars.iter().any(|b| b.id == bar && b.presented)
    }

    pub fn bar_by_name(&self, name: &str) -> Option<BarId> {
        self.bars.iter().find(|b| b.spec.name == name).map(|b| b.id)
    }

    pub fn popup_by_name(&self, name: &str) -> Option<PopupId> {
        self.popups.iter().rev().find(|p| p.name == name && p.dim.is_none()).map(|p| p.id)
    }

    /// A bar's window as a canvas (premultiplied BGRA), if it has been drawn.
    pub fn bar_canvas(&self, name: &str) -> Option<(RectPx, Canvas)> {
        let b = self.bars.iter().find(|b| b.spec.name == name && b.presented)?;
        Some((b.rect, Canvas { width: b.rect.width() as u32, height: b.rect.height() as u32, data: b.pixels.clone() }))
    }

    /// A popup's window as a canvas, if it is open and drawn.
    pub fn popup_canvas(&self, name: &str) -> Option<(RectPx, Canvas)> {
        let p = self.popups.iter().rev().find(|p| p.name == name && p.dim.is_none() && p.presented)?;
        Some((
            p.rect,
            Canvas {
                width: p.rect.width().max(0) as u32,
                height: p.rect.height().max(0) as u32,
                data: p.pixels.clone(),
            },
        ))
    }

    /// Every drawn surface from the bottom of the stack up: bars, panels and
    /// menus, dims, overlay content, tooltips.
    pub fn layers(&self) -> Vec<Layer<'_>> {
        let mut out: Vec<Layer<'_>> = self
            .bars
            .iter()
            .filter(|b| b.visible && b.presented)
            .map(|b| Layer { rect: b.rect, pixels: Some(&b.pixels), dim: None })
            .collect();
        let rank = |p: &HPopup| match (p.dim.is_some(), p.role) {
            (true, _) => 2,
            (false, SurfaceRole::Overlay) => 3,
            (false, SurfaceRole::Tooltip) => 4,
            _ => 1,
        };
        let mut shown: Vec<&HPopup> =
            self.popups.iter().filter(|p| p.presented || p.dim.is_some_and(|d| d.shown)).collect();
        shown.sort_by_key(|p| (rank(p), p.id));
        for p in shown {
            match p.dim {
                Some(d) => out.push(Layer { rect: p.rect, pixels: None, dim: Some(d.color) }),
                None => out.push(Layer { rect: p.rect, pixels: Some(&p.pixels), dim: None }),
            }
        }
        out
    }

    fn monitor_by_device(&self, device: &str) -> Option<&MonitorInfo> {
        self.monitors.iter().find(|m| m.device.eq_ignore_ascii_case(device))
    }

    fn bar_mut(&mut self, bar: BarId) -> Result<&mut HBar, WinError> {
        self.bars.iter_mut().find(|b| b.id == bar).ok_or(WinError::NoSuchBar(bar))
    }

    fn popup_mut(&mut self, popup: PopupId) -> Result<&mut HPopup, WinError> {
        self.popups
            .iter_mut()
            .find(|p| p.id == popup)
            .ok_or(WinError::Call { call: "popup", message: "no such popup window".into() })
    }

    fn push(&self, event: UiEvent) {
        self.events.borrow_mut().push_back(event);
    }
}

fn buffer(rect: &RectPx) -> Vec<u8> {
    vec![0; rect.width().max(0) as usize * rect.height().max(0) as usize * 4]
}

fn draw_into(pixels: &mut [u8], rect: &RectPx, draw: Draw<'_>) -> Result<(), WinError> {
    let (w, h) = (rect.width().max(0) as u32, rect.height().max(0) as u32);
    let Some(mut px) = PixelsMut::new(pixels, w, h) else { return Ok(()) };
    let _ = draw(&mut px);
    Ok(())
}

impl Win for Headless {
    fn monitors(&self) -> Vec<MonitorInfo> {
        // The work area is the monitor minus the strips its visible, reserving bars hold.
        self.monitors
            .iter()
            .map(|m| {
                let mut work = m.rect;
                for b in self.bars.iter().filter(|b| b.reserved && b.visible && b.spec.monitor == m.device) {
                    let depth = b.rect.height()
                        + if b.spec.floating { (b.spec.margin as f32 * b.scale).round() as i32 } else { 0 };
                    match b.spec.position {
                        Position::Top => work.y0 = work.y0.max(m.rect.y0 + depth),
                        Position::Bottom => work.y1 = work.y1.min(m.rect.y1 - depth),
                    }
                }
                MonitorInfo { work, ..m.clone() }
            })
            .collect()
    }

    fn work_area(&self, monitor: &str) -> Option<RectPx> {
        Win::monitors(self).into_iter().find(|m| m.device.eq_ignore_ascii_case(monitor)).map(|m| m.work)
    }

    fn set_offscreen(&mut self, _on: bool) {}

    fn quit(&mut self) {
        self.quit = true;
    }

    fn open_path(&self, _path: &Path) {}

    fn set_frame_deadline(&mut self, at: Option<Instant>) {
        self.deadline = at;
    }

    fn inject(&self, input: TestInput) {
        let event = match input {
            TestInput::Pointer { bar, event } => UiEvent::Pointer { bar, event },
            TestInput::PopupPointer { popup, event } => UiEvent::PopupPointer { popup, event },
            TestInput::Key { popup, event } => UiEvent::Key { popup, event },
            TestInput::Text { popup, input } => UiEvent::Text { popup, input },
            TestInput::Deactivate { popup } => UiEvent::PopupFocus { popup, focused: false },
            TestInput::PopupDpi { popup, dpi } => UiEvent::PopupDpiChanged { popup, dpi },
            TestInput::OutsidePress(p) => UiEvent::OutsidePress(p),
            TestInput::SessionLock { locked } => UiEvent::SessionLock { locked },
            TestInput::Theme { dark } => UiEvent::ThemeChanged { dark },
            TestInput::TaskbarCreated => UiEvent::TaskbarCreated,
            TestInput::DisplayChange => UiEvent::MonitorsChanged,
            TestInput::FullscreenNotice { bar, active } => UiEvent::Fullscreen { bar, active },
            TestInput::TimeChange | TestInput::Resume => UiEvent::ClockChanged,
            TestInput::Dpi { .. } => {
                if !self.warned_dpi.replace(true) {
                    log::warn!("a bar DPI change is not simulated without windows");
                }
                return;
            }
        };
        self.push(event);
    }

    fn compositor_frame(&self) -> Option<u64> {
        None
    }

    fn compositor_period(&self) -> Option<Duration> {
        None
    }

    fn create_bar(&mut self, spec: &BarWindowSpec) -> Result<BarId, WinError> {
        let mon =
            self.monitor_by_device(&spec.monitor).ok_or_else(|| WinError::NoSuchMonitor(spec.monitor.clone()))?.clone();
        let dpi = mon.dpi.clamp(48, 960);
        let scale = dpi as f32 / 96.0;
        let height = ((spec.height as f32 * scale).round() as i32).max(1);
        let margin = if spec.floating { (spec.margin as f32 * scale).round() as i32 } else { 0 };
        let depth = height + margin;
        let strip = match spec.position {
            Position::Top => RectPx::new(mon.rect.x0, mon.rect.y0, mon.rect.x1, mon.rect.y0 + depth),
            Position::Bottom => RectPx::new(mon.rect.x0, mon.rect.y1 - depth, mon.rect.x1, mon.rect.y1),
        };
        let rect = window_in_strip(spec, height, margin, strip);
        let id = BarId(self.next_bar);
        self.next_bar += 1;
        self.bars.push(HBar {
            id,
            spec: spec.clone(),
            scale,
            rect,
            pixels: buffer(&rect),
            visible: true,
            presented: false,
            reserved: spec.reserve,
        });
        let geometry = BarGeometry { width_px: rect.width() as u32, height_px: rect.height() as u32, scale };
        self.push(UiEvent::Geometry { bar: id, geometry });
        Ok(id)
    }

    fn destroy_bar(&mut self, bar: BarId) {
        self.bars.retain(|b| b.id != bar);
    }

    fn geometry(&self, bar: BarId) -> Option<BarGeometry> {
        let b = self.bars.iter().find(|b| b.id == bar)?;
        Some(BarGeometry { width_px: b.rect.width() as u32, height_px: b.rect.height() as u32, scale: b.scale })
    }

    fn window_rect(&self, bar: BarId) -> Option<RectPx> {
        self.bars.iter().find(|b| b.id == bar).map(|b| b.rect)
    }

    fn present(&mut self, bar: BarId, draw: Draw<'_>) -> Result<(), WinError> {
        let b = self.bar_mut(bar)?;
        let rect = b.rect;
        draw_into(&mut b.pixels, &rect, draw)?;
        b.presented = true;
        Ok(())
    }

    fn show_menu(&mut self, _bar: BarId, _x: i32, _y: i32) {}

    fn set_cursor(&mut self, _bar: BarId, _cursor: Cursor) {}

    fn set_backdrop(&mut self, _bar: BarId, _backdrop: Backdrop) {}

    fn set_visible(&mut self, bar: BarId, visible: bool) {
        if let Ok(b) = self.bar_mut(bar) {
            b.visible = visible;
        }
    }

    fn is_visible(&self, bar: BarId) -> bool {
        self.bars.iter().any(|b| b.id == bar && b.visible)
    }

    fn is_reserved(&self, bar: BarId) -> bool {
        self.bars.iter().any(|b| b.id == bar && b.reserved && b.visible)
    }

    fn create_popup(&mut self, spec: &PopupWindowSpec) -> Result<PopupId, WinError> {
        let mon =
            self.monitor_by_device(&spec.monitor).ok_or_else(|| WinError::NoSuchMonitor(spec.monitor.clone()))?.clone();
        let scale = mon.dpi.clamp(48, 960) as f32 / 96.0;
        let id = PopupId(self.next_popup);
        self.next_popup += 1;
        self.popups.push(HPopup {
            id,
            role: spec.role,
            rect: spec.rect,
            scale,
            pixels: buffer(&spec.rect),
            presented: false,
            focusable: spec.focusable && spec.role != SurfaceRole::Tooltip,
            dim: None,
            name: String::new(),
        });
        let geometry = BarGeometry {
            width_px: spec.rect.width().max(0) as u32,
            height_px: spec.rect.height().max(0) as u32,
            scale,
        };
        self.push(UiEvent::PopupGeometry { popup: id, geometry });
        Ok(id)
    }

    fn create_dim(&mut self, spec: &DimSpec) -> Result<PopupId, WinError> {
        let mon =
            self.monitor_by_device(&spec.monitor).ok_or_else(|| WinError::NoSuchMonitor(spec.monitor.clone()))?.clone();
        let id = PopupId(self.next_popup);
        self.next_popup += 1;
        self.popups.push(HPopup {
            id,
            role: SurfaceRole::Overlay,
            rect: spec.rect,
            scale: mon.dpi.clamp(48, 960) as f32 / 96.0,
            pixels: Vec::new(),
            presented: false,
            focusable: false,
            dim: Some(Dim { color: Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }, shown: false, paints: 0, rgb: (0, 0, 0) }),
            name: String::new(),
        });
        Ok(id)
    }

    fn move_popup(&mut self, popup: PopupId, rect: RectPx) -> Result<(), WinError> {
        let p = self.popup_mut(popup)?;
        let resized = rect.width() != p.rect.width() || rect.height() != p.rect.height();
        p.rect = rect;
        if resized {
            p.pixels = buffer(&rect);
            let geometry = BarGeometry {
                width_px: rect.width().max(0) as u32,
                height_px: rect.height().max(0) as u32,
                scale: p.scale,
            };
            self.push(UiEvent::PopupGeometry { popup, geometry });
        }
        Ok(())
    }

    fn destroy_popup(&mut self, popup: PopupId) {
        self.popups.retain(|p| p.id != popup);
    }

    fn popup_geometry(&self, popup: PopupId) -> Option<BarGeometry> {
        let p = self.popups.iter().find(|p| p.id == popup && p.dim.is_none())?;
        Some(BarGeometry {
            width_px: p.rect.width().max(0) as u32,
            height_px: p.rect.height().max(0) as u32,
            scale: p.scale,
        })
    }

    fn present_popup(&mut self, popup: PopupId, draw: Draw<'_>) -> Result<(), WinError> {
        let p = self.popup_mut(popup)?;
        let rect = p.rect;
        draw_into(&mut p.pixels, &rect, draw)?;
        p.presented = true;
        Ok(())
    }

    fn activate_popup(&mut self, popup: PopupId) -> bool {
        self.popups.iter().any(|p| p.id == popup && p.focusable && p.presented)
    }

    fn set_popup_focusable(&mut self, popup: PopupId, focusable: bool) {
        if let Ok(p) = self.popup_mut(popup) {
            p.focusable = focusable && p.role != SurfaceRole::Tooltip;
        }
    }

    fn set_popup_cursor(&mut self, _popup: PopupId, _cursor: Cursor) {}

    fn set_popup_region(&mut self, _popup: PopupId, _region: Option<HitRegion>) {}

    fn set_ime_enabled(&mut self, _popup: PopupId, _enabled: bool) {}

    fn set_ime_caret(&mut self, _popup: PopupId, _caret: Option<RectPx>) {}

    fn watch_outside_presses(&mut self, _on: bool) {}

    fn hook_stats(&self) -> Option<HookStats> {
        None
    }

    fn clipboard_read(&mut self, token: u64) {
        self.push(UiEvent::Clipboard { token, text: Some(crate::popups::FAKE_CLIPBOARD.to_string()) });
    }

    fn clipboard_write(&mut self, _text: String) {}

    fn pointer_monitor(&self) -> Option<String> {
        self.monitors.first().map(|m| m.device.clone())
    }

    fn set_dim(&mut self, dim: PopupId, color: Color) {
        let Some(p) = self.popups.iter_mut().find(|p| p.id == dim) else { return };
        let Some(d) = p.dim.as_mut() else { return };
        let rgb = (byte(color.r), byte(color.g), byte(color.b));
        // The colour is painted once; fading changes only the alpha.
        if !d.shown {
            d.shown = true;
            d.paints = 1;
            d.rgb = rgb;
        } else if d.rgb != rgb {
            d.rgb = rgb;
            d.paints += 1;
        }
        d.color = color;
    }

    fn dim_backdrop_active(&self, _dim: PopupId) -> bool {
        false
    }

    fn dim_paints(&self, dim: PopupId) -> Option<u32> {
        self.popups.iter().find(|p| p.id == dim).and_then(|p| p.dim).map(|d| d.paints)
    }

    fn name_popup(&mut self, popup: PopupId, name: &str) {
        let name = name.strip_prefix("popup:").or_else(|| name.strip_prefix("dim:")).unwrap_or(name).to_string();
        if let Ok(p) = self.popup_mut(popup) {
            p.name = name;
        }
    }
}

#[cfg(test)]
mod tests {
    use rsb_core::config::FullscreenMode;

    use super::*;

    fn spec(position: Position, floating: bool) -> BarWindowSpec {
        BarWindowSpec {
            name: "main".into(),
            monitor: "\\\\.\\DISPLAY1".into(),
            position,
            height: 36,
            floating,
            margin: 8,
            reserve: true,
            fullscreen: FullscreenMode::Below,
        }
    }

    #[test]
    fn a_bar_sits_at_its_edge_and_shrinks_the_work_area() {
        let mut w = Headless::new(&[(1920, 1200, 96)]);
        let id = w.create_bar(&spec(Position::Top, false)).unwrap();
        assert_eq!(w.window_rect(id), Some(RectPx::new(0, 0, 1920, 36)));
        assert_eq!(w.work_area("\\\\.\\DISPLAY1"), Some(RectPx::new(0, 36, 1920, 1200)));
        assert!(w.is_reserved(id));
        w.set_visible(id, false);
        assert_eq!(w.work_area("\\\\.\\DISPLAY1"), Some(RectPx::new(0, 0, 1920, 1200)), "a hidden bar gives it back");
        let b = w.create_bar(&spec(Position::Bottom, true)).unwrap();
        assert_eq!(w.window_rect(b), Some(RectPx::new(8, 1156, 1912, 1192)));
    }

    #[test]
    fn a_bar_is_scaled_by_the_monitor_dpi_and_announces_its_geometry() {
        let mut w = Headless::new(&[(2880, 1800, 144)]);
        let id = w.create_bar(&spec(Position::Top, false)).unwrap();
        assert_eq!(w.window_rect(id), Some(RectPx::new(0, 0, 2880, 54)));
        match w.next_event() {
            Some(UiEvent::Geometry { bar, geometry }) => {
                assert_eq!(bar, id);
                assert_eq!((geometry.width_px, geometry.height_px, geometry.scale), (2880, 54, 1.5));
            }
            other => panic!("{other:?}"),
        }
        assert!(w.next_event().is_none());
    }

    #[test]
    fn presenting_keeps_the_pixels_and_a_move_resizes_them() {
        let mut w = Headless::new(&[(800, 600, 96)]);
        let id = w
            .create_popup(&PopupWindowSpec {
                role: SurfaceRole::Popup,
                rect: RectPx::new(10, 40, 30, 50),
                monitor: "\\\\.\\DISPLAY1".into(),
                owner: None,
                focusable: false,
            })
            .unwrap();
        w.name_popup(id, "popup:dash");
        w.present_popup(id, &mut |px| {
            px.data[0] = 7;
            Some(RectPx::new(0, 0, 1, 1))
        })
        .unwrap();
        let (rect, canvas) = w.popup_canvas("dash").unwrap();
        assert_eq!((rect, canvas.width, canvas.height, canvas.data[0]), (RectPx::new(10, 40, 30, 50), 20, 10, 7));
        w.move_popup(id, RectPx::new(10, 40, 50, 80)).unwrap();
        assert_eq!(w.popup_geometry(id).map(|g| (g.width_px, g.height_px)), Some((40, 40)));
        let events: Vec<_> = std::iter::from_fn(|| w.next_event()).collect();
        assert_eq!(events.len(), 2, "one geometry event at creation and one for the resize");
    }

    #[test]
    fn a_dim_is_painted_once_and_only_a_new_colour_paints_again() {
        let mut w = Headless::new(&[(800, 600, 96)]);
        let dim = w
            .create_dim(&DimSpec {
                rect: RectPx::new(0, 0, 800, 600),
                monitor: "\\\\.\\DISPLAY1".into(),
                owner: None,
                backdrop: false,
            })
            .unwrap();
        assert_eq!(w.dim_paints(dim), Some(0));
        for a in [0.1, 0.3, 0.5] {
            w.set_dim(dim, Color { r: 0.0, g: 0.0, b: 0.0, a });
        }
        assert_eq!(w.dim_paints(dim), Some(1), "fading changes the alpha only");
        w.set_dim(dim, Color { r: 0.2, g: 0.0, b: 0.0, a: 0.5 });
        assert_eq!(w.dim_paints(dim), Some(2));
        assert_eq!(w.layers().len(), 1);
    }

    #[test]
    fn injected_input_comes_back_as_the_event_the_real_layer_would_send() {
        let w = Headless::new(&[(800, 600, 96)]);
        w.inject(TestInput::SessionLock { locked: true });
        w.inject(TestInput::DisplayChange);
        let mut w = w;
        assert_eq!(w.next_event(), Some(UiEvent::SessionLock { locked: true }));
        assert_eq!(w.next_event(), Some(UiEvent::MonitorsChanged));
        assert_eq!(w.next_event(), None);
    }
}

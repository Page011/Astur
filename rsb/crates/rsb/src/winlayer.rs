//! What the app and the popup manager need from the window layer, as a trait.
//! The real implementation is `rsb_win::Ui`; the headless timeline renderer
//! has a second one that keeps windows as rectangles and pixel buffers, so the
//! same app code runs with no window at all.

use std::path::Path;
use std::time::{Duration, Instant};

use rsb_core::color::Color;
use rsb_core::geom::RectPx;
use rsb_core::pixels::PixelsMut;
use rsb_core::scene::BarGeometry;
use rsb_core::style::{Backdrop, Cursor};
use rsb_render::Canvas;
use rsb_win::{BarId, BarWindowSpec, DimSpec, HitRegion, HookStats, MonitorInfo, PopupId, PopupWindowSpec};
use rsb_win::{TestInput, Ui, WinError};

use crate::capture::{Capture, Limit};

/// Draws into a surface's pixels and says which part changed.
pub type Draw<'a> = &'a mut dyn FnMut(&mut PixelsMut<'_>) -> Option<RectPx>;

pub trait Win {
    fn monitors(&self) -> Vec<MonitorInfo>;
    fn work_area(&self, monitor: &str) -> Option<RectPx>;
    fn set_offscreen(&mut self, on: bool);
    fn quit(&mut self);
    fn open_path(&self, path: &Path);
    fn set_frame_deadline(&mut self, at: Option<Instant>);
    fn inject(&self, input: TestInput);
    fn compositor_frame(&self) -> Option<u64>;
    fn compositor_period(&self) -> Option<Duration>;

    fn create_bar(&mut self, spec: &BarWindowSpec) -> Result<BarId, WinError>;
    fn destroy_bar(&mut self, bar: BarId);
    fn geometry(&self, bar: BarId) -> Option<BarGeometry>;
    fn window_rect(&self, bar: BarId) -> Option<RectPx>;
    fn present(&mut self, bar: BarId, draw: Draw<'_>) -> Result<(), WinError>;
    fn show_menu(&mut self, bar: BarId, x: i32, y: i32);
    fn set_cursor(&mut self, bar: BarId, cursor: Cursor);
    fn set_backdrop(&mut self, bar: BarId, backdrop: Backdrop);
    fn set_visible(&mut self, bar: BarId, visible: bool);
    fn is_visible(&self, bar: BarId) -> bool;
    fn is_reserved(&self, bar: BarId) -> bool;

    fn create_popup(&mut self, spec: &PopupWindowSpec) -> Result<PopupId, WinError>;
    fn create_dim(&mut self, spec: &DimSpec) -> Result<PopupId, WinError>;
    fn move_popup(&mut self, popup: PopupId, rect: RectPx) -> Result<(), WinError>;
    fn destroy_popup(&mut self, popup: PopupId);
    fn popup_geometry(&self, popup: PopupId) -> Option<BarGeometry>;
    fn present_popup(&mut self, popup: PopupId, draw: Draw<'_>) -> Result<(), WinError>;
    fn activate_popup(&mut self, popup: PopupId) -> bool;
    fn set_popup_focusable(&mut self, popup: PopupId, focusable: bool);
    fn set_popup_cursor(&mut self, popup: PopupId, cursor: Cursor);
    fn set_popup_region(&mut self, popup: PopupId, region: Option<HitRegion>);
    fn set_ime_enabled(&mut self, popup: PopupId, enabled: bool);
    fn set_ime_caret(&mut self, popup: PopupId, caret: Option<RectPx>);
    fn watch_outside_presses(&mut self, on: bool);
    fn hook_stats(&self) -> Option<HookStats>;
    fn clipboard_read(&mut self, token: u64);
    fn clipboard_write(&mut self, text: String);
    fn pointer_monitor(&self) -> Option<String>;
    fn set_dim(&mut self, dim: PopupId, color: Color);
    fn dim_backdrop_active(&self, dim: PopupId) -> bool;
    fn dim_paints(&self, dim: PopupId) -> Option<u32>;

    /// The first bar frame was drawn (the UI probe keeps its worst gap from here apart).
    fn first_frame_drawn(&mut self) {}

    /// The manager says what a popup window is called (`popup:<name>`,
    /// `dim:<name>`), for the frames a capture writes.
    fn name_popup(&mut self, _popup: PopupId, _name: &str) {}
}

impl Win for Ui {
    fn monitors(&self) -> Vec<MonitorInfo> {
        Ui::monitors(self)
    }
    fn work_area(&self, monitor: &str) -> Option<RectPx> {
        Ui::work_area(self, monitor)
    }
    fn set_offscreen(&mut self, on: bool) {
        Ui::set_offscreen(self, on)
    }
    fn quit(&mut self) {
        Ui::quit(self)
    }
    fn first_frame_drawn(&mut self) {
        Ui::probe_first_frame(self)
    }
    fn open_path(&self, path: &Path) {
        Ui::open_path(self, path)
    }
    fn set_frame_deadline(&mut self, at: Option<Instant>) {
        Ui::set_frame_deadline(self, at)
    }
    fn inject(&self, input: TestInput) {
        Ui::inject(self, input)
    }
    fn compositor_frame(&self) -> Option<u64> {
        Ui::compositor_frame(self)
    }
    fn compositor_period(&self) -> Option<Duration> {
        Ui::compositor_period(self)
    }
    fn create_bar(&mut self, spec: &BarWindowSpec) -> Result<BarId, WinError> {
        Ui::create_bar(self, spec)
    }
    fn destroy_bar(&mut self, bar: BarId) {
        Ui::destroy_bar(self, bar)
    }
    fn geometry(&self, bar: BarId) -> Option<BarGeometry> {
        Ui::geometry(self, bar)
    }
    fn window_rect(&self, bar: BarId) -> Option<RectPx> {
        Ui::window_rect(self, bar)
    }
    fn present(&mut self, bar: BarId, draw: Draw<'_>) -> Result<(), WinError> {
        Ui::present(self, bar, draw)
    }
    fn show_menu(&mut self, bar: BarId, x: i32, y: i32) {
        Ui::show_menu(self, bar, x, y)
    }
    fn set_cursor(&mut self, bar: BarId, cursor: Cursor) {
        Ui::set_cursor(self, bar, cursor)
    }
    fn set_backdrop(&mut self, bar: BarId, backdrop: Backdrop) {
        Ui::set_backdrop(self, bar, backdrop)
    }
    fn set_visible(&mut self, bar: BarId, visible: bool) {
        Ui::set_visible(self, bar, visible)
    }
    fn is_visible(&self, bar: BarId) -> bool {
        Ui::is_visible(self, bar)
    }
    fn is_reserved(&self, bar: BarId) -> bool {
        Ui::is_reserved(self, bar)
    }
    fn create_popup(&mut self, spec: &PopupWindowSpec) -> Result<PopupId, WinError> {
        Ui::create_popup(self, spec)
    }
    fn create_dim(&mut self, spec: &DimSpec) -> Result<PopupId, WinError> {
        Ui::create_dim(self, spec)
    }
    fn move_popup(&mut self, popup: PopupId, rect: RectPx) -> Result<(), WinError> {
        Ui::move_popup(self, popup, rect)
    }
    fn destroy_popup(&mut self, popup: PopupId) {
        Ui::destroy_popup(self, popup)
    }
    fn popup_geometry(&self, popup: PopupId) -> Option<BarGeometry> {
        Ui::popup_geometry(self, popup)
    }
    fn present_popup(&mut self, popup: PopupId, draw: Draw<'_>) -> Result<(), WinError> {
        Ui::present_popup(self, popup, draw)
    }
    fn activate_popup(&mut self, popup: PopupId) -> bool {
        Ui::activate_popup(self, popup)
    }
    fn set_popup_focusable(&mut self, popup: PopupId, focusable: bool) {
        Ui::set_popup_focusable(self, popup, focusable)
    }
    fn set_popup_cursor(&mut self, popup: PopupId, cursor: Cursor) {
        Ui::set_popup_cursor(self, popup, cursor)
    }
    fn set_popup_region(&mut self, popup: PopupId, region: Option<HitRegion>) {
        Ui::set_popup_region(self, popup, region)
    }
    fn set_ime_enabled(&mut self, popup: PopupId, enabled: bool) {
        Ui::set_ime_enabled(self, popup, enabled)
    }
    fn set_ime_caret(&mut self, popup: PopupId, caret: Option<RectPx>) {
        Ui::set_ime_caret(self, popup, caret)
    }
    fn watch_outside_presses(&mut self, on: bool) {
        Ui::watch_outside_presses(self, on)
    }
    fn hook_stats(&self) -> Option<HookStats> {
        Ui::hook_stats(self)
    }
    fn clipboard_read(&mut self, token: u64) {
        Ui::clipboard_read(self, token)
    }
    fn clipboard_write(&mut self, text: String) {
        Ui::clipboard_write(self, text)
    }
    fn pointer_monitor(&self) -> Option<String> {
        Ui::pointer_monitor(self)
    }
    fn set_dim(&mut self, dim: PopupId, color: Color) {
        Ui::set_dim(self, dim, color)
    }
    fn dim_backdrop_active(&self, dim: PopupId) -> bool {
        Ui::dim_backdrop_active(self, dim)
    }
    fn dim_paints(&self, dim: PopupId) -> Option<u32> {
        Ui::dim_paints(self, dim)
    }
}

/// A copy of a surface's pixels as a canvas.
pub fn canvas_of(px: &PixelsMut<'_>) -> Canvas {
    let len = px.width as usize * px.height as usize * 4;
    Canvas { width: px.width, height: px.height, data: px.data[..len].to_vec() }
}

/// The real window layer with a frame capture on every present, for
/// `--capture` runs. Everything else is the plain `Ui`.
pub struct Live<'a> {
    pub ui: &'a mut Ui,
    pub state: &'a mut CaptureState,
}

/// One presented frame waiting to be written.
pub struct Pending {
    t_ms: u64,
    label: String,
    dirty: RectPx,
    canvas: Canvas,
}

/// A capture that is on, with the labels of the windows it writes.
pub struct CaptureState {
    pub capture: Capture,
    pub names: Names,
    /// The limit was reached and said so.
    pub reported: bool,
    /// Frames copied during a present; they are encoded and written once the
    /// present returns, so the time a frame costs is not the capture's.
    pending: Vec<Pending>,
    /// Set when the capture stopped on a limit; the run then ends.
    pub stopped: Option<Limit>,
}

impl CaptureState {
    pub fn new(capture: Capture) -> CaptureState {
        CaptureState { capture, names: Names::default(), reported: false, pending: Vec::new(), stopped: None }
    }

    /// Writes the frames copied since the last call.
    pub fn flush(&mut self) {
        for f in std::mem::take(&mut self.pending) {
            match self.capture.record(f.t_ms, &f.label, f.dirty, &f.canvas) {
                Ok(None) => {}
                Ok(Some(limit)) => self.stopped = Some(limit),
                Err(e) => {
                    log::error!("{e}");
                    self.stopped.get_or_insert(Limit::Frames);
                }
            }
        }
    }
}

/// Window labels for the capture index.
#[derive(Default)]
pub struct Names {
    /// Id, bar name, and the monitor's index in the plan.
    bars: Vec<(BarId, String, u32)>,
    popups: Vec<(PopupId, String)>,
}

impl Names {
    pub fn bar(&self, id: BarId) -> String {
        let Some((_, name, index)) = self.bars.iter().find(|(b, ..)| *b == id) else { return format!("bar:{}", id.0) };
        // The monitor is named only when one bar name runs on several monitors.
        if self.bars.iter().filter(|(_, n, _)| n == name).count() > 1 {
            format!("bar:{name}@{index}")
        } else {
            format!("bar:{name}")
        }
    }

    pub fn popup(&self, id: PopupId) -> String {
        self.popups.iter().find(|(p, _)| *p == id).map_or_else(|| format!("popup:{}", id.0), |(_, n)| n.clone())
    }

    /// `bar:main` when the bar runs on one monitor; `bar:main@0` and `bar:main@1` when the same bar
    /// runs on two (numbered by the plan's monitor index, not by the display number Windows
    /// gave the screen, which changes between sessions).
    pub fn set_bar(&mut self, id: BarId, name: &str, monitor_index: u32) {
        self.bars.retain(|(b, ..)| *b != id);
        self.bars.push((id, name.to_string(), monitor_index));
    }

    pub fn set_popup(&mut self, id: PopupId, name: &str) {
        self.popups.retain(|(p, _)| *p != id);
        self.popups.push((id, name.to_string()));
    }

    pub fn forget_popup(&mut self, id: PopupId) {
        self.popups.retain(|(p, _)| *p != id);
    }
}

impl Live<'_> {
    fn save(state: &mut CaptureState, label: String, px: &PixelsMut<'_>, dirty: Option<RectPx>) {
        let Some(dirty) = dirty else { return };
        let t_ms = state.capture.elapsed_ms();
        state.pending.push(Pending { t_ms, label, dirty, canvas: canvas_of(px) });
    }
}

macro_rules! delegate {
    ($($name:ident(&self $(, $arg:ident: $ty:ty)*) $(-> $ret:ty)?;)*) => {
        $(fn $name(&self $(, $arg: $ty)*) $(-> $ret)? { Win::$name(&*self.ui, $($arg),*) })*
    };
}

macro_rules! delegate_mut {
    ($($name:ident(&mut self $(, $arg:ident: $ty:ty)*) $(-> $ret:ty)?;)*) => {
        $(fn $name(&mut self $(, $arg: $ty)*) $(-> $ret)? { Win::$name(&mut *self.ui, $($arg),*) })*
    };
}

impl Win for Live<'_> {
    delegate! {
        monitors(&self) -> Vec<MonitorInfo>;
        work_area(&self, monitor: &str) -> Option<RectPx>;
        open_path(&self, path: &Path);
        inject(&self, input: TestInput);
        compositor_frame(&self) -> Option<u64>;
        compositor_period(&self) -> Option<Duration>;
        geometry(&self, bar: BarId) -> Option<BarGeometry>;
        window_rect(&self, bar: BarId) -> Option<RectPx>;
        is_visible(&self, bar: BarId) -> bool;
        is_reserved(&self, bar: BarId) -> bool;
        popup_geometry(&self, popup: PopupId) -> Option<BarGeometry>;
        hook_stats(&self) -> Option<HookStats>;
        pointer_monitor(&self) -> Option<String>;
        dim_backdrop_active(&self, dim: PopupId) -> bool;
        dim_paints(&self, dim: PopupId) -> Option<u32>;
    }

    delegate_mut! {
        set_offscreen(&mut self, on: bool);
        quit(&mut self);
        first_frame_drawn(&mut self);
        set_frame_deadline(&mut self, at: Option<Instant>);
        destroy_bar(&mut self, bar: BarId);
        show_menu(&mut self, bar: BarId, x: i32, y: i32);
        set_cursor(&mut self, bar: BarId, cursor: Cursor);
        set_backdrop(&mut self, bar: BarId, backdrop: Backdrop);
        set_visible(&mut self, bar: BarId, visible: bool);
        create_dim(&mut self, spec: &DimSpec) -> Result<PopupId, WinError>;
        create_popup(&mut self, spec: &PopupWindowSpec) -> Result<PopupId, WinError>;
        move_popup(&mut self, popup: PopupId, rect: RectPx) -> Result<(), WinError>;
        activate_popup(&mut self, popup: PopupId) -> bool;
        set_popup_focusable(&mut self, popup: PopupId, focusable: bool);
        set_popup_cursor(&mut self, popup: PopupId, cursor: Cursor);
        set_popup_region(&mut self, popup: PopupId, region: Option<HitRegion>);
        set_ime_enabled(&mut self, popup: PopupId, enabled: bool);
        set_ime_caret(&mut self, popup: PopupId, caret: Option<RectPx>);
        watch_outside_presses(&mut self, on: bool);
        clipboard_read(&mut self, token: u64);
        clipboard_write(&mut self, text: String);
        set_dim(&mut self, dim: PopupId, color: Color);
    }

    fn create_bar(&mut self, spec: &BarWindowSpec) -> Result<BarId, WinError> {
        let id = Win::create_bar(&mut *self.ui, spec)?;
        let index =
            self.ui.monitors().iter().find(|m| m.device.eq_ignore_ascii_case(&spec.monitor)).map_or(0, |m| m.index);
        self.state.names.set_bar(id, &spec.name, index);
        Ok(id)
    }

    fn present(&mut self, bar: BarId, draw: Draw<'_>) -> Result<(), WinError> {
        let label = self.state.names.bar(bar);
        let state = &mut *self.state;
        Win::present(&mut *self.ui, bar, &mut |px| {
            let dirty = draw(px);
            Live::save(state, label.clone(), px, dirty);
            dirty
        })
    }

    fn present_popup(&mut self, popup: PopupId, draw: Draw<'_>) -> Result<(), WinError> {
        let label = self.state.names.popup(popup);
        let state = &mut *self.state;
        Win::present_popup(&mut *self.ui, popup, &mut |px| {
            let dirty = draw(px);
            Live::save(state, label.clone(), px, dirty);
            dirty
        })
    }

    fn destroy_popup(&mut self, popup: PopupId) {
        self.state.names.forget_popup(popup);
        Win::destroy_popup(&mut *self.ui, popup)
    }

    fn name_popup(&mut self, popup: PopupId, name: &str) {
        self.state.names.set_popup(popup, name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bar_on_one_monitor_has_no_monitor_in_its_name_and_on_two_it_has_the_plan_index() {
        let mut names = Names::default();
        names.set_bar(BarId(1), "main", 609);
        assert_eq!(names.bar(BarId(1)), "bar:main", "the display number of the session never shows");
        names.set_bar(BarId(2), "main", 1);
        names.set_bar(BarId(1), "main", 0);
        assert_eq!(names.bar(BarId(1)), "bar:main@0");
        assert_eq!(names.bar(BarId(2)), "bar:main@1");
        names.set_bar(BarId(3), "side", 1);
        assert_eq!(names.bar(BarId(3)), "bar:side");
        assert_eq!(names.bar(BarId(9)), "bar:9", "an unknown window keeps its number");
    }
}

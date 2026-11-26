//! Popup, menu, tooltip and overlay windows, the keyboard and IME on them,
//! the outside-press watch and the clipboard worker as the UI thread uses
//! them.
//!
//! Every popup is a layered window with its own DIB, created hidden and shown
//! without activation by its first present. Nothing here takes keyboard focus
//! on its own: only `activate_popup` asks for it, and never forces it.

use std::time::Instant;

use rsb_core::geom::RectPx;
use rsb_core::hit::PointerEvent;
use rsb_core::key::{KeyEvent, TextInput};
use rsb_core::pixels::PixelsMut;
use rsb_core::scene::BarGeometry;
use rsb_core::style::Cursor;
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DWM_SYSTEMBACKDROP_TYPE, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DwmExtendFrameIntoClientArea, DwmGetWindowAttribute, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::Input::Ime::{
    CANDIDATEFORM, CFS_EXCLUDE, CFS_FORCE_POSITION, CFS_POINT, COMPOSITIONFORM, HIMC, IACE_DEFAULT,
    ImmAssociateContextEx, ImmGetContext, ImmReleaseContext, ImmSetCandidateWindow, ImmSetCompositionWindow,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, GWL_EXSTYLE, GetCursorPos, GetForegroundWindow, GetWindowLongPtrW, HWND_TOPMOST, IsWindow,
    LWA_ALPHA, SW_SHOWNOACTIVATE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SetForegroundWindow, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow, WM_IME_COMPOSITION,
    WM_IME_ENDCOMPOSITION, WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};

use crate::dib::Dib;
use crate::fullscreen::is_own_process;
use crate::keys::{self, Composition};
use crate::regions::{PopupRegion, Regions};
use crate::shared::{Target, WinState};
use crate::{
    BarId, DimSpec, HitRegion, HookStats, MonitorInfo, PopupId, PopupWindowSpec, SurfaceRole, Ui, UiEvent, WinError,
    clip, disable_dwm_rounding, hook, monitor, pace, sys, wnd,
};

/// What the UI thread keeps per popup, menu, tooltip, overlay content or dim
/// window.
pub(crate) struct PopupState {
    pub hwnd: HWND,
    pub role: SurfaceRole,
    /// `MonitorInfo::device` it was made for.
    pub monitor: String,
    /// Window rectangle, physical px, virtual-screen coordinates.
    pub rect: RectPx,
    pub owner: Option<BarId>,
    pub focusable: bool,
    pub scale: f32,
    /// The surface (None for a dim window: it has no pixels of its own).
    pub dib: Option<Dib>,
    pub needs_full: bool,
    /// Shown by its first present (or first `set_dim`).
    pub presented: bool,
    pub region: Option<HitRegion>,
    /// The window we handed focus away from, to give it back when this one
    /// closes while still in front.
    pub prev_foreground: isize,
    pub dim: Option<DimState>,
}

pub(crate) struct DimState {
    /// The window under the dim that the system draws its backdrop into.
    pub backdrop: Option<HWND>,
    /// The colour painted into the window (COLORREF).
    pub rgb: u32,
    /// The constant alpha last applied.
    pub alpha: Option<u8>,
}

fn no_popup(call: &'static str) -> WinError {
    WinError::Call { call, message: "no such popup".into() }
}

fn colorref(r: f32, g: f32, b: f32) -> u32 {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    byte(r) | byte(g) << 8 | byte(b) << 16
}

impl Ui {
    /// The x a bar or popup window takes in an offscreen run: beyond every
    /// monitor, real or simulated.
    pub(crate) fn offscreen_x(&self) -> i32 {
        self.monitors().iter().map(|m| m.rect.x1).max().unwrap_or(0) + 200
    }

    /// An offscreen run never covers a real screen: a rectangle that would
    /// is moved beyond the monitors (the way bars are), keeping its offset
    /// from its monitor's left edge.
    pub(crate) fn offscreen_rect(&self, rect: RectPx, mon: &MonitorInfo) -> RectPx {
        if !self.offscreen {
            return rect;
        }
        if !monitor::enumerate().iter().any(|m| m.rect.intersect(&rect).is_some()) {
            return rect;
        }
        let dx = self.offscreen_x() - mon.rect.x0;
        RectPx::new(rect.x0 + dx, rect.y0, rect.x1 + dx, rect.y1)
    }

    // ---- popup windows ----

    /// Creates a popup window at `spec.rect` (layered, premultiplied like
    /// the bars, topmost above its owner bar, no taskbar button, hidden
    /// until its first present). A `PopupGeometry` event follows. Overlays
    /// go above every bar and popup; tooltips above everything and let the
    /// pointer through.
    pub fn create_popup(&mut self, spec: &PopupWindowSpec) -> Result<PopupId, WinError> {
        let mon = self.find_monitor(&spec.monitor).ok_or_else(|| WinError::NoSuchMonitor(spec.monitor.clone()))?;
        let rect = self.offscreen_rect(spec.rect, &mon);
        let focusable = spec.focusable && spec.role != SurfaceRole::Tooltip;
        let mut ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST;
        if !focusable {
            ex |= WS_EX_NOACTIVATE;
        }
        if spec.role == SurfaceRole::Tooltip {
            ex |= WS_EX_TRANSPARENT;
        }
        let dib = Dib::new(rect.width().max(0) as u32, rect.height().max(0) as u32)?;
        let hwnd = wnd::create_popup(std::rc::Rc::as_ptr(&self.shared), ex, &rect)?;
        let id = PopupId(self.next_popup);
        self.next_popup += 1;
        self.shared.wins.borrow_mut().insert(sys::raw(hwnd), WinState::new(Target::Popup(id)));
        disable_dwm_rounding(hwnd);
        associate_ime(hwnd, false);
        let scale = mon.dpi.clamp(48, 960) as f32 / 96.0;
        let geometry = BarGeometry { width_px: dib.width, height_px: dib.height, scale };
        self.popups.insert(
            id,
            PopupState {
                hwnd,
                role: spec.role,
                monitor: spec.monitor.clone(),
                rect,
                owner: spec.owner,
                focusable,
                scale,
                dib: Some(dib),
                needs_full: true,
                presented: false,
                region: None,
                prev_foreground: 0,
                dim: None,
            },
        );
        self.pending.push_back(UiEvent::PopupGeometry { popup: id, geometry });
        Ok(id)
    }

    /// Moves or resizes a popup window (a new DIB when the size changes;
    /// a `PopupGeometry` event follows).
    pub fn move_popup(&mut self, popup: PopupId, rect: RectPx) -> Result<(), WinError> {
        let device = self.popups.get(&popup).ok_or_else(|| no_popup("move_popup"))?.monitor.clone();
        let rect = match self.find_monitor(&device) {
            Some(mon) => self.offscreen_rect(rect, &mon),
            None => rect,
        };
        let p = self.popups.get_mut(&popup).ok_or_else(|| no_popup("move_popup"))?;
        let resized = rect.width() != p.rect.width() || rect.height() != p.rect.height();
        if resized && p.dib.is_some() {
            p.dib = Some(Dib::new(rect.width().max(0) as u32, rect.height().max(0) as u32)?);
            p.needs_full = true;
        }
        // SAFETY: the window is ours.
        unsafe {
            let _ = SetWindowPos(
                p.hwnd,
                None,
                rect.x0,
                rect.y0,
                rect.width(),
                rect.height(),
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        p.rect = rect;
        let geometry = p.dib.as_ref().map(|d| BarGeometry { width_px: d.width, height_px: d.height, scale: p.scale });
        if resized && let Some(geometry) = geometry {
            self.pending.push_back(UiEvent::PopupGeometry { popup, geometry });
        }
        self.publish_regions();
        Ok(())
    }

    /// Hides and destroys a popup window. If it held keyboard focus and
    /// we are still the foreground process, focus goes back to the window
    /// that had it before `activate_popup`.
    pub fn destroy_popup(&mut self, popup: PopupId) {
        let Some(p) = self.popups.remove(&popup) else { return };
        let key = sys::raw(p.hwnd);
        if p.focusable && p.prev_foreground != 0 {
            // SAFETY: plain handle arguments; a stale handle only fails.
            let (holds, back) = unsafe { (GetForegroundWindow() == p.hwnd, sys::hwnd(p.prev_foreground)) };
            // SAFETY: as above.
            if holds && unsafe { IsWindow(Some(back)) }.as_bool() {
                self.foreground_calls += 1;
                // SAFETY: allowed because this process is the foreground one;
                // nothing is forced.
                unsafe {
                    let _ = SetForegroundWindow(back);
                }
            }
        }
        self.stall.remove_overlay(key);
        if let Some(dim) = &p.dim
            && let Some(b) = dim.backdrop
        {
            self.stall.remove_overlay(sys::raw(b));
            // SAFETY: the window belongs to this thread.
            unsafe {
                let _ = DestroyWindow(b);
            }
        }
        // SAFETY: the window belongs to this thread.
        unsafe {
            let _ = DestroyWindow(p.hwnd);
        }
        self.shared.wins.borrow_mut().remove(&key);
        self.publish_regions();
        // The DIB goes with `p`, after the window that used it.
    }

    pub fn popup_geometry(&self, popup: PopupId) -> Option<BarGeometry> {
        let p = self.popups.get(&popup)?;
        let d = p.dib.as_ref()?;
        Some(BarGeometry { width_px: d.width, height_px: d.height, scale: p.scale })
    }

    /// The popup window's rectangle (physical px, virtual-screen
    /// coordinates), after the offscreen rule.
    pub fn popup_rect(&self, popup: PopupId) -> Option<RectPx> {
        self.popups.get(&popup).map(|p| p.rect)
    }

    /// Like `present`, for a popup window; shows it (without activating)
    /// on its first present.
    pub fn present_popup(
        &mut self,
        popup: PopupId,
        draw: &mut dyn FnMut(&mut PixelsMut<'_>) -> Option<RectPx>,
    ) -> Result<(), WinError> {
        let p = self.popups.get_mut(&popup).ok_or_else(|| no_popup("present_popup"))?;
        let Some(dib) = p.dib.as_mut() else { return Err(no_popup("present_popup")) };
        let (w, h) = (dib.width as i32, dib.height as i32);
        let dirty = {
            let mut px = dib.pixels();
            draw(&mut px)
        };
        let Some(dirty) = dirty.and_then(|r| r.intersect(&RectPx::new(0, 0, w, h))) else { return Ok(()) };
        let whole = p.needs_full || dirty == RectPx::new(0, 0, w, h);
        crate::blit(p.hwnd, dib.dc, (w, h), dirty, whole)?;
        p.needs_full = false;
        if !p.presented {
            p.presented = true;
            // Windows made later sit above earlier ones; a popup that opens
            // now goes above everything of ours that was raised since. An
            // overlay's content goes above its dim, whichever was shown first.
            raise(p.hwnd);
            // SAFETY: the window is ours; no activation.
            unsafe {
                let _ = ShowWindow(p.hwnd, SW_SHOWNOACTIVATE);
            }
            if p.role == SurfaceRole::Overlay {
                self.stall.add_overlay(sys::raw(p.hwnd));
            }
            self.publish_regions();
        }
        Ok(())
    }

    /// Puts every shown popup of this bar above it again (a bar that raised
    /// itself, such as one coming back after a fullscreen app).
    pub(crate) fn raise_popups_of(&self, bar: BarId) {
        for p in self.popups.values() {
            if p.owner == Some(bar) && p.presented && p.role != SurfaceRole::Overlay {
                raise(p.hwnd);
            }
        }
    }

    // ---- focus ----

    /// Gives a focusable popup keyboard focus (`SetForegroundWindow` on our
    /// own window). Windows allows it only right after the user's input
    /// reached this process (a click or key on our windows) or when the
    /// process that had that input passed its right on
    /// (`AllowSetForegroundWindow`, done by `rsb popup`). Returns whether
    /// the popup now has focus; never forces it.
    pub fn activate_popup(&mut self, popup: PopupId) -> bool {
        let Some(p) = self.popups.get_mut(&popup) else { return false };
        if !p.focusable || !p.presented || p.dim.is_some() {
            return false;
        }
        // SAFETY: no arguments.
        let front = unsafe { GetForegroundWindow() };
        if front == p.hwnd {
            return true;
        }
        if !front.0.is_null() {
            let mut pid = 0u32;
            // SAFETY: a live out pointer; a stale handle only fails the call.
            let thread =
                unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(front, Some(&mut pid)) };
            // SAFETY: no arguments.
            let own = unsafe { windows::Win32::System::Threading::GetCurrentProcessId() };
            if !is_own_process((thread != 0).then_some(pid), own) {
                p.prev_foreground = sys::raw(front);
            }
        }
        let hwnd = p.hwnd;
        self.foreground_calls += 1;
        // SAFETY: our own window; Windows decides whether it is allowed.
        let asked = unsafe { SetForegroundWindow(hwnd) }.as_bool();
        // SAFETY: no arguments.
        asked && unsafe { GetForegroundWindow() } == hwnd
    }

    /// The popup's content gained its first or lost its last focusable
    /// element: `WS_EX_NOACTIVATE` is cleared or set to match.
    pub fn set_popup_focusable(&mut self, popup: PopupId, focusable: bool) {
        let Some(p) = self.popups.get_mut(&popup) else { return };
        if p.role == SurfaceRole::Tooltip || p.dim.is_some() || p.focusable == focusable {
            return;
        }
        p.focusable = focusable;
        let hwnd = p.hwnd;
        // SAFETY: the window is ours; the style change is applied without
        // moving, resizing, reordering or activating it.
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let no_activate = WS_EX_NOACTIVATE.0 as isize;
            let style = if focusable { style & !no_activate } else { style | no_activate };
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }

    pub fn set_popup_cursor(&mut self, popup: PopupId, cursor: Cursor) {
        let Some(p) = self.popups.get(&popup) else { return };
        let key = sys::raw(p.hwnd);
        let hovered = {
            let mut wins = self.shared.wins.borrow_mut();
            let Some(w) = wins.get_mut(&key) else { return };
            w.cursor = cursor;
            w.hovered
        };
        // WM_SETCURSOR only comes with mouse movement: apply it now when the
        // pointer is already over the window.
        if hovered {
            wnd::apply_cursor(cursor);
        }
    }

    // ---- keyboard and IME ----

    /// Places the IME composition and candidate windows against the caret
    /// (popup-relative physical px); None = no editable text has focus.
    pub fn set_ime_caret(&mut self, popup: PopupId, caret: Option<RectPx>) {
        let (Some(p), Some(c)) = (self.popups.get(&popup), caret) else { return };
        // Windows IMEs built on the text services framework ask for the character's place instead
        // (`WM_IME_REQUEST`): keep it where the window procedure can answer.
        if let Some(w) = self.shared.wins.borrow_mut().get_mut(&sys::raw(p.hwnd)) {
            let (ox, oy) = (p.rect.x0, p.rect.y0);
            w.ime_caret = Some((c.x0 + ox, c.y0 + oy, c.x1 + ox, c.y1 + oy));
        }
        // SAFETY: the window is ours; the context is released again.
        unsafe {
            let himc = ImmGetContext(p.hwnd);
            if himc.0.is_null() {
                return;
            }
            let form = CANDIDATEFORM {
                dwIndex: 0,
                dwStyle: CFS_EXCLUDE,
                ptCurrentPos: POINT { x: c.x0, y: c.y1 },
                rcArea: RECT { left: c.x0, top: c.y0, right: c.x1, bottom: c.y1 },
            };
            let _ = ImmSetCandidateWindow(himc, &form);
            let comp = COMPOSITIONFORM {
                dwStyle: CFS_POINT | CFS_FORCE_POSITION,
                ptCurrentPos: POINT { x: c.x0, y: c.y0 },
                rcArea: RECT::default(),
            };
            let _ = ImmSetCompositionWindow(himc, &comp);
            let _ = ImmReleaseContext(p.hwnd, himc);
        }
    }

    /// Whether the popup window has an input context: on only while an
    /// editable input that is not a password has focus
    /// (`ImmAssociateContextEx`), so an IME never composes into a button
    /// and its learning dictionary never sees a password.
    pub fn set_ime_enabled(&mut self, popup: PopupId, enabled: bool) {
        let Some(p) = self.popups.get(&popup) else { return };
        associate_ime(p.hwnd, enabled);
    }

    /// Whether the window has an input context now (read back).
    #[cfg(test)]
    pub(crate) fn ime_context_present(&self, popup: PopupId) -> bool {
        let Some(p) = self.popups.get(&popup) else { return false };
        // SAFETY: the window is ours; the context is released again.
        unsafe {
            let himc = ImmGetContext(p.hwnd);
            let present = !himc.0.is_null();
            if present {
                let _ = ImmReleaseContext(p.hwnd, himc);
            }
            present
        }
    }

    pub(crate) fn inject_popup_pointer(&self, popup: PopupId, event: PointerEvent) -> bool {
        self.popups
            .get(&popup)
            .is_some_and(|p| crate::inject_pointer_at(sys::raw(p.hwnd), (p.rect.x0, p.rect.y0), event))
    }

    pub(crate) fn inject_key(&self, popup: PopupId, event: KeyEvent) -> bool {
        let Some(p) = self.popups.get(&popup) else { return false };
        let alt = event.mods.alt;
        let msg = match (event.down, alt) {
            (true, false) => WM_KEYDOWN,
            (true, true) => WM_SYSKEYDOWN,
            (false, false) => WM_KEYUP,
            (false, true) => WM_SYSKEYUP,
        };
        // A real key message reads the thread's key state for its modifiers;
        // a test one carries them here, for the length of the send.
        self.shared.inject_mods.set(Some(event.mods));
        sys::send(
            sys::raw(p.hwnd),
            msg,
            keys::vk_of(event.key) as usize,
            keys::key_lparam(event.down, event.repeat, alt),
        );
        self.shared.inject_mods.set(None);
        true
    }

    pub(crate) fn inject_text(&self, popup: PopupId, input: TextInput) -> bool {
        let Some(p) = self.popups.get(&popup) else { return false };
        let hwnd = sys::raw(p.hwnd);
        match input {
            TextInput::Chars(text) => {
                for unit in text.encode_utf16() {
                    sys::send(hwnd, windows::Win32::UI::WindowsAndMessaging::WM_CHAR, unit as usize, 1);
                }
            }
            TextInput::CompositionStart => sys::send(hwnd, WM_IME_STARTCOMPOSITION, 0, 0),
            TextInput::CompositionUpdate { text, cursor } => {
                *self.shared.inject_comp.borrow_mut() =
                    Some(Composition { result: None, update: Some((text, cursor)) });
                let flags = keys::GCS_COMPSTR_BIT | keys::GCS_CURSORPOS_BIT;
                sys::send(hwnd, WM_IME_COMPOSITION, 0, flags as isize);
            }
            TextInput::CompositionCommit(text) => {
                *self.shared.inject_comp.borrow_mut() = Some(Composition { result: Some(text), update: None });
                sys::send(hwnd, WM_IME_COMPOSITION, 0, keys::GCS_RESULTSTR_BIT as isize);
            }
            TextInput::CompositionEnd => sys::send(hwnd, WM_IME_ENDCOMPOSITION, 0, 0),
        }
        true
    }

    // ---- outside presses ----

    /// The popup's clickable shape for the outside-press watch (None = the
    /// whole window rect). Called when it changes; the watch thread reads
    /// an atomically swapped copy of every region.
    pub fn set_popup_region(&mut self, popup: PopupId, region: Option<HitRegion>) {
        let Some(p) = self.popups.get_mut(&popup) else { return };
        if p.region == region {
            return;
        }
        p.region = region;
        self.publish_regions();
    }

    /// While on, a low-level mouse hook reports button presses outside
    /// every RSB window (bars by their window rect, popups by their
    /// `set_popup_region` shape) as `OutsidePress`; popups that do not hold
    /// focus close on them. The hook runs on its own thread with a bare
    /// message loop, never on the UI thread, so a busy UI frame never
    /// delays the pointer anywhere on the desktop: it only reads the event,
    /// tests the point against the latest region snapshot, posts one
    /// message for an outside press, and calls the next hook. It is removed
    /// when turned off, at quit, when the loop ends and in the panic path,
    /// and installed again if it went quiet while the pointer moved.
    pub fn watch_outside_presses(&mut self, on: bool) {
        if on {
            if self.hook.as_ref().is_some_and(|h| h.is_running()) {
                return;
            }
            self.hook = None;
            self.hook_state.publish(self.build_regions());
            match hook::Hook::start(&self.hook_state) {
                Ok(h) => self.hook = Some(h),
                Err(e) => log::warn!("outside-press watch not started: {e}"),
            }
        } else if let Some(h) = self.hook.take() {
            h.stop();
        }
    }

    /// Numbers from the outside-press watch (None while it was never on).
    pub fn hook_stats(&self) -> Option<HookStats> {
        self.hook_state.ever_on().then(|| self.hook_state.stats())
    }

    /// Whether the mouse hook is installed right now.
    #[cfg(test)]
    pub(crate) fn hook_installed(&self) -> bool {
        self.hook.as_ref().is_some_and(|h| h.is_running()) && self.hook_state.is_installed()
    }

    fn build_regions(&self) -> Regions {
        let bars = self.bars.values().filter(|b| b.shown).map(|b| b.window).collect();
        let popups = self
            .popups
            .values()
            .filter(|p| p.presented && p.role != SurfaceRole::Tooltip)
            .map(|p| PopupRegion {
                origin: (p.rect.x0, p.rect.y0),
                size: (p.rect.width(), p.rect.height()),
                outline: p.region.as_ref().map(|r| r.points.clone()),
            })
            .collect();
        Regions { bars, popups }
    }

    /// Hands the hook thread the shapes as they are now. Nothing to do while
    /// the watch is off: it takes a snapshot when it starts.
    pub(crate) fn publish_regions(&self) {
        if self.hook.is_some() {
            self.hook_state.publish(self.build_regions());
        }
    }

    // ---- clipboard ----

    pub(crate) fn clip_notifier(&self) -> Box<dyn Fn() + Send + Sync> {
        let hidden = self.shared.hidden.get();
        Box::new(move || {
            sys::post(hidden, wnd::WM_CLIP_READY, 0, 0);
        })
    }

    /// Reads clipboard text on the clipboard worker (it can block on the
    /// app that owns the clipboard); the result arrives as
    /// `UiEvent::Clipboard` with this token, after at most 500 ms. The
    /// worker opens the clipboard with its own message-only window and
    /// tries 5 times 10 ms apart; while one request is stuck in another
    /// app no second worker starts: later requests answer None at once.
    pub fn clipboard_read(&mut self, token: u64) {
        let notify = self.clip_notifier();
        let client = self.clip.get_or_insert_with(|| clip::ClipClient::real(notify));
        let now = Instant::now();
        for (token, text) in client.read(token, now) {
            self.pending.push_back(UiEvent::Clipboard { token, text });
        }
        self.arm_clip_timer();
    }

    /// Puts text on the clipboard from the clipboard worker (500 ms limit;
    /// `CF_UNICODETEXT` in `GlobalAlloc(GMEM_MOVEABLE)` memory, the
    /// clipboard opened with the worker's own window so `SetClipboardData`
    /// is allowed after `EmptyClipboard`).
    pub fn clipboard_write(&mut self, text: String) {
        let notify = self.clip_notifier();
        let client = self.clip.get_or_insert_with(|| clip::ClipClient::real(notify));
        client.write(text, Instant::now());
    }

    /// Answers what finished, and what ran out of time.
    pub(crate) fn poll_clipboard(&mut self) {
        let Some(client) = self.clip.as_mut() else { return };
        for (token, text) in client.poll(Instant::now()) {
            self.pending.push_back(UiEvent::Clipboard { token, text });
        }
        self.arm_clip_timer();
    }

    /// A timer for the oldest unanswered read, so the 500 ms promise holds
    /// even when the worker never reports back.
    fn arm_clip_timer(&self) {
        let Some(due) = self.clip.as_ref().and_then(|c| c.next_deadline()) else { return };
        let ms = due.saturating_duration_since(Instant::now()).as_millis() as u32 + 1;
        sys::set_timer(self.shared.hidden.get(), wnd::TIMER_CLIP, ms);
    }

    #[cfg(test)]
    pub(crate) fn use_clipboard(&mut self, client: clip::ClipClient) {
        self.clip = Some(client);
    }

    /// The monitor under the pointer (`MonitorInfo::device`), for
    /// `rsb popup` with several bars.
    pub fn pointer_monitor(&self) -> Option<String> {
        let mut p = POINT::default();
        // SAFETY: a live out pointer.
        unsafe { GetCursorPos(&mut p) }.ok()?;
        monitor::enumerate()
            .into_iter()
            .find(|m| p.x >= m.rect.x0 && p.x < m.rect.x1 && p.y >= m.rect.y0 && p.y < m.rect.y1)
            .map(|m| m.device)
    }

    // ---- overlay dim ----

    /// Creates an overlay's dim window over `spec.rect`, hidden until the
    /// first `set_dim`: above every bar and popup, below the overlay's
    /// content window (create the content after it), click-catching (a
    /// press on it arrives as `PopupPointer` with this id), never
    /// activated, no taskbar button. With `spec.backdrop` a system
    /// backdrop window goes under it. `destroy_popup` removes both.
    pub fn create_dim(&mut self, spec: &DimSpec) -> Result<PopupId, WinError> {
        let mon = self.find_monitor(&spec.monitor).ok_or_else(|| WinError::NoSuchMonitor(spec.monitor.clone()))?;
        let rect = self.offscreen_rect(spec.rect, &mon);
        // The backdrop is created first so it sits under the dim.
        let backdrop = if spec.backdrop { self.make_backdrop(&rect) } else { None };
        let hwnd = match wnd::create_dim(std::rc::Rc::as_ptr(&self.shared), &rect) {
            Ok(h) => h,
            Err(e) => {
                if let Some(b) = backdrop {
                    // SAFETY: the window was just created by this thread.
                    unsafe {
                        let _ = DestroyWindow(b);
                    }
                }
                return Err(e);
            }
        };
        disable_dwm_rounding(hwnd);
        // A layered window shows nothing until its alpha is set.
        // SAFETY: the window is ours.
        unsafe {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA);
        }
        let id = PopupId(self.next_popup);
        self.next_popup += 1;
        self.shared.wins.borrow_mut().insert(sys::raw(hwnd), WinState::new(Target::Popup(id)));
        let scale = mon.dpi.clamp(48, 960) as f32 / 96.0;
        self.popups.insert(
            id,
            PopupState {
                hwnd,
                role: SurfaceRole::Overlay,
                monitor: spec.monitor.clone(),
                rect,
                owner: spec.owner,
                focusable: false,
                scale,
                dib: None,
                needs_full: false,
                presented: false,
                region: None,
                prev_foreground: 0,
                dim: Some(DimState { backdrop, rgb: 0, alpha: Some(0) }),
            },
        );
        Ok(id)
    }

    /// The system backdrop window, when it can work: the attribute is read
    /// back and the window is dropped again if it did not take, so a black
    /// window never ends up under the dim.
    fn make_backdrop(&self, rect: &RectPx) -> Option<HWND> {
        let hwnd = wnd::create_backdrop(std::rc::Rc::as_ptr(&self.shared), rect).ok()?;
        let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
        let kind: DWM_SYSTEMBACKDROP_TYPE = DWMSBT_TRANSIENTWINDOW;
        let dark: i32 = self.dark as i32;
        // SAFETY: the window is ours; each attribute pointer refers to a live
        // local of the size passed.
        let ok = unsafe {
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark as *const _ as *const std::ffi::c_void,
                4,
            );
            let extended = DwmExtendFrameIntoClientArea(hwnd, &margins).is_ok();
            let set =
                DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &kind as *const _ as *const std::ffi::c_void, 4)
                    .is_ok();
            extended && set && backdrop_kind(hwnd) == Some(DWMSBT_TRANSIENTWINDOW)
        };
        if ok {
            return Some(hwnd);
        }
        // SAFETY: the window belongs to this thread.
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        None
    }

    /// The dim's colour (straight alpha): the window's constant alpha
    /// changes (`SetLayeredWindowAttributes`); the solid colour is painted
    /// again only when its RGB changes. Shows the window on the first call.
    pub fn set_dim(&mut self, dim: PopupId, color: rsb_core::color::Color) {
        let Some(p) = self.popups.get_mut(&dim) else { return };
        let Some(d) = p.dim.as_mut() else { return };
        let rgb = colorref(color.r, color.g, color.b);
        let alpha = (color.a.clamp(0.0, 1.0) * 255.0).round() as u8;
        let key = sys::raw(p.hwnd);
        if rgb != d.rgb {
            d.rgb = rgb;
            if let Some(w) = self.shared.wins.borrow_mut().get_mut(&key) {
                w.dim_rgb = rgb;
            }
            if p.presented {
                // SAFETY: the window is ours.
                unsafe {
                    let _ = InvalidateRect(Some(p.hwnd), None, false);
                }
            }
        }
        if d.alpha != Some(alpha) {
            d.alpha = Some(alpha);
            // SAFETY: the window is ours.
            unsafe {
                let _ = SetLayeredWindowAttributes(p.hwnd, COLORREF(0), alpha, LWA_ALPHA);
            }
        }
        if !p.presented {
            p.presented = true;
            // The colour is in place and the alpha set: nothing flashes.
            // Backdrop, dim, then any overlay content already shown, so the
            // order holds whichever came first.
            if let Some(b) = d.backdrop {
                raise(b);
            }
            raise(p.hwnd);
            // SAFETY: the windows are ours; no activation.
            unsafe {
                if let Some(b) = d.backdrop {
                    let _ = ShowWindow(b, SW_SHOWNOACTIVATE);
                    self.stall.add_overlay(sys::raw(b));
                }
                let _ = ShowWindow(p.hwnd, SW_SHOWNOACTIVATE);
            }
            self.stall.add_overlay(key);
            for content in
                self.popups.values().filter(|c| c.dim.is_none() && c.role == SurfaceRole::Overlay && c.presented)
            {
                raise(content.hwnd);
            }
            self.publish_regions();
        }
    }

    /// Whether the system backdrop is in place behind the dim, read back
    /// from the backdrop window (`DwmGetWindowAttribute`). False without
    /// one. The overlay root gets the class `blur` only when true.
    pub fn dim_backdrop_active(&self, dim: PopupId) -> bool {
        self.popups
            .get(&dim)
            .and_then(|p| p.dim.as_ref())
            .and_then(|d| d.backdrop)
            .is_some_and(|b| backdrop_kind(b) == Some(DWMSBT_TRANSIENTWINDOW))
    }

    /// How many times the dim window was painted (its colour is painted
    /// once; fading only changes its alpha).
    pub fn dim_paints(&self, dim: PopupId) -> Option<u32> {
        let p = self.popups.get(&dim)?;
        p.dim.as_ref()?;
        self.shared.wins.borrow().get(&sys::raw(p.hwnd)).map(|w| w.paints)
    }

    /// The dim window's constant alpha as the system holds it.
    #[cfg(test)]
    pub(crate) fn dim_alpha(&self, dim: PopupId) -> Option<u8> {
        use windows::Win32::UI::WindowsAndMessaging::GetLayeredWindowAttributes;
        let p = self.popups.get(&dim)?;
        let mut alpha = 0u8;
        // SAFETY: a live out pointer; the window is ours.
        unsafe { GetLayeredWindowAttributes(p.hwnd, None, Some(&mut alpha), None) }.ok()?;
        Some(alpha)
    }

    #[cfg(test)]
    pub(crate) fn popup_hwnd(&self, popup: PopupId) -> Option<HWND> {
        self.popups.get(&popup).map(|p| p.hwnd)
    }

    #[cfg(test)]
    pub(crate) fn dim_backdrop_hwnd(&self, dim: PopupId) -> Option<HWND> {
        self.popups.get(&dim)?.dim.as_ref()?.backdrop
    }

    /// For the watchdog thread.
    pub fn stall_guard(&self) -> crate::StallGuard {
        self.stall.clone()
    }

    /// The compositor's frame counter now (`DWM_TIMING_INFO::cRefresh`),
    /// to count how many presents landed in distinct display frames. None
    /// when Windows gives no timing (some remote sessions).
    pub fn compositor_frame(&self) -> Option<u64> {
        pace::compositor_frame()
    }

    /// The display's refresh period as the compositor reports it, so the
    /// caller can say how many presents it made per display frame. None when
    /// Windows gives no timing.
    pub fn compositor_period(&self) -> Option<std::time::Duration> {
        pace::refresh_period()
    }

    /// Whether lock and unlock notices were registered for this session
    /// (a service session has none).
    pub fn session_notices_registered(&self) -> bool {
        self.session_notices
    }

    /// `SetForegroundWindow` calls this `Ui` made.
    #[cfg(test)]
    pub(crate) fn activation_calls(&self) -> u32 {
        self.foreground_calls
    }
}

/// `HWND_TOPMOST` without moving, sizing or activating.
fn raise(hwnd: HWND) {
    // SAFETY: the window is ours.
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

/// An input context only while an editable, non-password input has focus.
fn associate_ime(hwnd: HWND, enabled: bool) {
    // SAFETY: the window is ours. A null context detaches the IME; the
    // default flag gives the window its own again.
    unsafe {
        let _ = ImmAssociateContextEx(hwnd, HIMC::default(), if enabled { IACE_DEFAULT } else { 0 });
    }
}

fn backdrop_kind(hwnd: HWND) -> Option<DWM_SYSTEMBACKDROP_TYPE> {
    let mut kind = 0i32;
    // SAFETY: a live out pointer of the size passed.
    unsafe {
        DwmGetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &mut kind as *mut i32 as *mut std::ffi::c_void, 4)
    }
    .ok()?;
    Some(DWM_SYSTEMBACKDROP_TYPE(kind))
}

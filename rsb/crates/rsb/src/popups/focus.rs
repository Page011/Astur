//! Keyboard focus and the input method. Focus is never taken by a popup
//! that was not opened by the user: a timer, an effect or a service update
//! that opens one leaves the keyboard where it was. Only a gesture that is
//! at most `ACTIVATE_WITHIN` old, and a popup that has something to type
//! into or pick from, ask for it.

use std::time::Duration;

use rsb_core::element::Element;
use rsb_core::geom::RectPx;
use rsb_core::popup::{FocusMode, PopupProps};
use rsb_win::SurfaceRole;
use serde_json::json;
use windows::Win32::UI::WindowsAndMessaging::{GetCaretBlinkTime, GetSystemMetrics, SM_REMOTESESSION};

use super::{Env, Popups, element_at, has_focusable};

/// Whether this kind of surface asks for focus when it opens in a gesture:
/// overlays and menus always, popups by their `focus` setting and content,
/// tooltips never.
pub(super) fn wants_focus(role: SurfaceRole, props: &PopupProps, content: Option<&Element>) -> bool {
    match role {
        SurfaceRole::Tooltip => false,
        SurfaceRole::Overlay | SurfaceRole::Menu => true,
        SurfaceRole::Popup => match props.focus {
            FocusMode::None => false,
            FocusMode::Always => true,
            FocusMode::Auto => content.is_some_and(has_focusable),
        },
    }
}

/// The window may take focus when it would ask for it; it is made
/// `WS_EX_NOACTIVATE` otherwise, like a bar.
pub(super) fn window_focusable(role: SurfaceRole, props: &PopupProps, content: Option<&Element>) -> bool {
    wants_focus(role, props, content)
}

/// Windows' caret blink period as the scene wants it; None when the user
/// turned blinking off.
fn blink_period() -> Option<Duration> {
    // SAFETY: a plain query.
    let ms = unsafe { GetCaretBlinkTime() };
    (ms != 0 && ms != u32::MAX).then(|| Duration::from_millis(u64::from(ms)))
}

/// The system backdrop (blur) behind an overlay's dim window can work:
/// Windows 11 build 22621 or later, not a remote session, and transparency
/// effects on. Read only.
pub(super) fn backdrop_possible() -> bool {
    // SAFETY: a plain query.
    if unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0 {
        return false;
    }
    let build =
        registry::string(registry::LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentBuild")
            .and_then(|b| b.trim().parse::<u32>().ok())
            .unwrap_or(0);
    let transparency = registry::dword(
        registry::CURRENT_USER,
        r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
        "EnableTransparency",
    );
    build >= 22621 && transparency == Some(1)
}

mod registry {
    use std::ffi::c_void;

    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
    };
    use windows::core::HSTRING;

    pub const LOCAL_MACHINE: HKEY = HKEY_LOCAL_MACHINE;
    pub const CURRENT_USER: HKEY = HKEY_CURRENT_USER;

    pub fn dword(root: HKEY, path: &str, name: &str) -> Option<u32> {
        let mut data = 0u32;
        let mut size = 4u32;
        // SAFETY: the out pointers refer to live locals of the sizes passed.
        let status = unsafe {
            RegGetValueW(
                root,
                &HSTRING::from(path),
                &HSTRING::from(name),
                RRF_RT_REG_DWORD,
                None,
                Some(std::ptr::from_mut(&mut data).cast::<c_void>()),
                Some(&mut size),
            )
        };
        status.is_ok().then_some(data)
    }

    pub fn string(root: HKEY, path: &str, name: &str) -> Option<String> {
        let mut buf = [0u16; 64];
        let mut size = std::mem::size_of_val(&buf) as u32;
        // SAFETY: the buffer is live and `size` is its length in bytes.
        let status = unsafe {
            RegGetValueW(
                root,
                &HSTRING::from(path),
                &HSTRING::from(name),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast::<c_void>()),
                Some(&mut size),
            )
        };
        if status.is_err() {
            return None;
        }
        let units = (size as usize / 2).saturating_sub(1).min(buf.len());
        Some(String::from_utf16_lossy(&buf[..units]))
    }
}

impl Popups {
    /// When a popup's window is ready: asks for focus if the popup wants it
    /// and was opened by a gesture that is still fresh.
    pub(super) fn take_focus_on_open(&mut self, id: u64, env: &mut Env<'_>) {
        let Some(i) = self.idx(id) else { return };
        let m = &self.managed[i];
        if !wants_focus(m.role, &m.props, element_at(&m.tree, &m.path)) {
            return;
        }
        let age_ms = self.gestures.age(m.gesture, env.now).map(|a| a.as_millis() as u64);
        if self.gestures.fresh(m.gesture, env.now).is_none() {
            self.stats
                .emit("focus", json!({"popup": m.name, "got": false, "activated": false, "gesture_age_ms": age_ms}));
            return;
        }
        let keyboard = m.keyboard;
        self.activate(i, env, age_ms, Some(keyboard));
    }

    /// A press inside a popup that does not hold focus gives it focus: the
    /// window never activates itself on a click.
    pub(super) fn press_activate(&mut self, id: u64, env: &mut Env<'_>) {
        let Some(i) = self.idx(id) else { return };
        let m = &self.managed[i];
        if m.holds || m.role == SurfaceRole::Tooltip || !m.focusable {
            return;
        }
        if !wants_focus(m.role, &m.props, element_at(&m.tree, &m.path)) {
            return;
        }
        self.activate(i, env, Some(0), None);
    }

    /// Takes focus for `managed[i]`. `first_keyboard` is Some when the popup
    /// just opened: its first focusable element is focused (shown when the
    /// popup was opened from the keyboard).
    fn activate(&mut self, i: usize, env: &mut Env<'_>, age_ms: Option<u64>, first_keyboard: Option<bool>) {
        let (win, name) = (self.managed[i].win, self.managed[i].name.clone());
        let (got, activated) = if self.o.no_activate {
            (true, false)
        } else {
            let ok = env.ui.activate_popup(win);
            (ok, ok)
        };
        self.stats.emit("focus", json!({"popup": name, "got": got, "activated": activated, "gesture_age_ms": age_ms}));
        if got {
            self.set_holds(i, env, first_keyboard);
        }
    }

    /// Marks `managed[i]` as the surface holding focus; its parent, if any, no longer does.
    pub(super) fn set_holds(&mut self, i: usize, env: &mut Env<'_>, first_keyboard: Option<bool>) {
        let now = env.now;
        let parent = self.managed[i].parent;
        {
            let m = &mut self.managed[i];
            m.holds = true;
            m.scene.set_window_focus(true, now);
            m.scene.set_caret_blink(blink_period());
            if let Some(keyboard) = first_keyboard {
                m.scene.focus_first(keyboard, now);
            }
        }
        if let Some(p) = parent.and_then(|p| self.idx(p)) {
            let pm = &mut self.managed[p];
            pm.holds = false;
            pm.scene.set_window_focus(false, now);
            pm.scene.set_caret_blink(None);
        }
        self.frame_one(self.managed[i].id, env);
    }

    /// Focus goes back to the popup this one was opened from.
    pub(super) fn give_focus(&mut self, id: u64, env: &mut Env<'_>) {
        let Some(i) = self.idx(id) else { return };
        if self.managed[i].stage == super::Stage::Closing || self.managed[i].holds {
            return;
        }
        let win = self.managed[i].win;
        let got = self.o.no_activate || env.ui.activate_popup(win);
        if got {
            let now = env.now;
            let m = &mut self.managed[i];
            m.holds = true;
            m.scene.set_window_focus(true, now);
            m.scene.set_caret_blink(blink_period());
        }
    }

    /// After a frame of the popup that holds focus: the input context is on
    /// only while an editable input that is not a password has focus, and
    /// the candidate window follows the caret.
    pub(super) fn ime_after_frame(&mut self, i: usize, env: &mut Env<'_>) {
        let m = &mut self.managed[i];
        let caret = m.scene.caret_rect();
        let want = caret.is_some() && !m.scene.focused_password();
        if want != m.ime {
            m.ime = want;
            env.ui.set_ime_enabled(m.win, want);
            self.stats.emit_with("ime", || json!({"stage": if want { "enabled" } else { "disabled" }}));
        }
        if want {
            let rect = caret.map(|c| RectPx::outward(&c));
            if rect != m.ime_caret {
                m.ime_caret = rect;
                env.ui.set_ime_caret(m.win, rect);
                self.stats.emit_with("ime", || json!({"stage": "caret"}));
            }
        } else {
            m.ime_caret = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsb_core::element::{Element, ElementKind};

    fn el(kind: ElementKind) -> Element {
        Element::new(kind)
    }

    fn with_child(kind: ElementKind) -> Element {
        let mut root = el(ElementKind::Popup);
        let mut mid = el(ElementKind::Box);
        mid.children.push(el(kind));
        root.children.push(mid);
        root
    }

    #[test]
    fn overlays_and_menus_always_ask_and_tooltips_never() {
        let props = PopupProps::default();
        assert!(wants_focus(SurfaceRole::Overlay, &props, None));
        assert!(wants_focus(SurfaceRole::Menu, &props, None));
        assert!(!wants_focus(SurfaceRole::Tooltip, &PopupProps { focus: FocusMode::Always, ..props }, None));
    }

    #[test]
    fn a_popup_follows_its_focus_mode_and_its_content() {
        let auto = PopupProps::default();
        let text_only = with_child(ElementKind::Text);
        let with_input = with_child(ElementKind::Input);
        assert!(!wants_focus(SurfaceRole::Popup, &auto, Some(&text_only)), "nothing to focus");
        assert!(wants_focus(SurfaceRole::Popup, &auto, Some(&with_input)));
        assert!(wants_focus(SurfaceRole::Popup, &auto, Some(&with_child(ElementKind::Slider))));
        assert!(wants_focus(SurfaceRole::Popup, &auto, Some(&with_child(ElementKind::MenuItem))));
        let mut focusable = with_child(ElementKind::Box);
        focusable.children[0].children[0].interaction.focusable = true;
        assert!(wants_focus(SurfaceRole::Popup, &auto, Some(&focusable)));
        let never = PopupProps { focus: FocusMode::None, ..PopupProps::default() };
        assert!(!wants_focus(SurfaceRole::Popup, &never, Some(&with_input)));
        let always = PopupProps { focus: FocusMode::Always, ..PopupProps::default() };
        assert!(wants_focus(SurfaceRole::Popup, &always, Some(&text_only)));
        assert!(!wants_focus(SurfaceRole::Popup, &auto, None), "no content yet");
    }

    #[test]
    fn the_registry_reads_work_and_fail_quietly() {
        // Reads only: the build number is a number on every Windows this runs on.
        let build =
            registry::string(registry::LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentBuild");
        assert!(build.is_some_and(|b| b.trim().parse::<u32>().is_ok()));
        assert_eq!(registry::dword(registry::CURRENT_USER, r"Software\rsb-no-such-key", "x"), None);
    }
}

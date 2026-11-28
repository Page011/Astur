//! Keyboard and IME plumbing for popup windows: virtual keys to
//! `rsb_core::key::Key`, joining surrogate pairs from `WM_CHAR`, and reading
//! an IME composition.
//!
//! Letters and digits are identified by their virtual-key code, never by the
//! character the layout produces, so Ctrl+C is `Char('c')` on every layout.
//! Typed text comes separately from `WM_CHAR`.

use rsb_core::key::{Key, Modifiers};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::Ime::{
    GCS_COMPSTR, GCS_CURSORPOS, GCS_RESULTSTR, IME_COMPOSITION_STRING, ISC_SHOWUICOMPOSITIONWINDOW,
    ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_INSERT, VK_LEFT, VK_LWIN,
    VK_MENU, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_RWIN, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};

/// First and last function-key codes (F1 to F24).
const VK_F1: u16 = 0x70;
const VK_F24: u16 = 0x87;

/// `lParam` of `WM_IME_SETCONTEXT` without the request to show the IME's own
/// composition window: the composition is drawn by the popup.
pub(crate) fn without_composition_window(lparam: isize) -> isize {
    lparam & !(ISC_SHOWUICOMPOSITIONWINDOW as isize)
}

/// The longest composition string read from an IME, in UTF-16 units.
const MAX_COMPOSITION: usize = 4096;

pub(crate) const GCS_COMPSTR_BIT: u32 = GCS_COMPSTR.0;
pub(crate) const GCS_CURSORPOS_BIT: u32 = GCS_CURSORPOS.0;
pub(crate) const GCS_RESULTSTR_BIT: u32 = GCS_RESULTSTR.0;

/// A virtual-key code as a `Key`.
pub(crate) fn map_vk(vk: u16) -> Key {
    match vk {
        v if v == VK_RETURN.0 => Key::Enter,
        v if v == VK_ESCAPE.0 => Key::Escape,
        v if v == VK_TAB.0 => Key::Tab,
        v if v == VK_BACK.0 => Key::Backspace,
        v if v == VK_DELETE.0 => Key::Delete,
        v if v == VK_INSERT.0 => Key::Insert,
        v if v == VK_SPACE.0 => Key::Space,
        v if v == VK_UP.0 => Key::ArrowUp,
        v if v == VK_DOWN.0 => Key::ArrowDown,
        v if v == VK_LEFT.0 => Key::ArrowLeft,
        v if v == VK_RIGHT.0 => Key::ArrowRight,
        v if v == VK_HOME.0 => Key::Home,
        v if v == VK_END.0 => Key::End,
        v if v == VK_PRIOR.0 => Key::PageUp,
        v if v == VK_NEXT.0 => Key::PageDown,
        VK_F1..=VK_F24 => Key::F((vk - VK_F1 + 1) as u8),
        // The virtual-key codes of the digits and letters are their ASCII
        // codes (upper case for letters).
        0x30..=0x39 => Key::Char(char::from(vk as u8)),
        0x41..=0x5A => Key::Char(char::from(vk as u8).to_ascii_lowercase()),
        other => Key::Other(other),
    }
}

/// The virtual-key code that `map_vk` turns into `key` (for injected keys).
pub(crate) fn vk_of(key: Key) -> u16 {
    match key {
        Key::Enter => VK_RETURN.0,
        Key::Escape => VK_ESCAPE.0,
        Key::Tab => VK_TAB.0,
        Key::Backspace => VK_BACK.0,
        Key::Delete => VK_DELETE.0,
        Key::Insert => VK_INSERT.0,
        Key::Space => VK_SPACE.0,
        Key::ArrowUp => VK_UP.0,
        Key::ArrowDown => VK_DOWN.0,
        Key::ArrowLeft => VK_LEFT.0,
        Key::ArrowRight => VK_RIGHT.0,
        Key::Home => VK_HOME.0,
        Key::End => VK_END.0,
        Key::PageUp => VK_PRIOR.0,
        Key::PageDown => VK_NEXT.0,
        Key::F(n) => VK_F1 + (n.clamp(1, 24) as u16 - 1),
        Key::Char(c) if c.is_ascii_alphanumeric() => c.to_ascii_uppercase() as u16,
        Key::Char(c) => c as u32 as u16,
        Key::Other(v) => v,
    }
}

/// Modifier keys as the thread's input state has them now (a real key
/// message is handled while that state matches it).
pub(crate) fn real_modifiers() -> Modifiers {
    let down = |vk: u16| {
        // SAFETY: plain value argument.
        let state = unsafe { GetKeyState(vk as i32) };
        state < 0
    };
    Modifiers {
        ctrl: down(VK_CONTROL.0),
        shift: down(VK_SHIFT.0),
        alt: down(VK_MENU.0),
        meta: down(VK_LWIN.0) || down(VK_RWIN.0),
    }
}

/// The lParam of a key message: repeat count 1, previous-state bit for a
/// repeat, transition bit for a release, context-code bit for Alt.
pub(crate) fn key_lparam(down: bool, repeat: bool, alt: bool) -> isize {
    let mut v: u32 = 1;
    if alt {
        v |= 1 << 29;
    }
    if repeat || !down {
        v |= 1 << 30;
    }
    if !down {
        v |= 1 << 31;
    }
    v as isize
}

/// Whether a key message's lParam says the key was already down.
pub(crate) fn is_repeat(lparam: isize) -> bool {
    (lparam >> 30) & 1 == 1
}

/// Joins UTF-16 units from `WM_CHAR` into characters: a high surrogate waits
/// for its low half; a lone surrogate is dropped.
#[derive(Default)]
pub(crate) struct CharJoiner {
    high: Option<u16>,
}

impl CharJoiner {
    pub(crate) fn push(&mut self, unit: u16) -> Option<char> {
        match unit {
            0xD800..=0xDBFF => {
                self.high = Some(unit);
                None
            }
            0xDC00..=0xDFFF => {
                let high = self.high.take()?;
                char::decode_utf16([high, unit]).next().and_then(|r| r.ok())
            }
            _ => {
                self.high = None;
                char::from_u32(unit as u32)
            }
        }
    }
}

/// A typed character the scene should insert: control characters (Enter,
/// Tab, Backspace, Escape, Ctrl+letter) are keys, not text.
pub(crate) fn text_char(c: char) -> Option<char> {
    (!c.is_control()).then_some(c)
}

/// What a `WM_IME_COMPOSITION` carried.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Composition {
    /// The finished text (`GCS_RESULTSTR`).
    pub result: Option<String>,
    /// The text still being composed and the cursor in chars (`GCS_COMPSTR`).
    pub update: Option<(String, usize)>,
}

/// A cursor given in UTF-16 units as a count of chars into `text`.
pub(crate) fn cursor_in_chars(text: &str, utf16_cursor: usize) -> usize {
    let mut units = 0usize;
    let mut chars = 0usize;
    for c in text.chars() {
        if units >= utf16_cursor {
            break;
        }
        units += c.len_utf16();
        chars += 1;
    }
    chars
}

fn read_string(himc: windows::Win32::UI::Input::Ime::HIMC, what: IME_COMPOSITION_STRING) -> Option<String> {
    // SAFETY: a size query with no buffer.
    let bytes = unsafe { ImmGetCompositionStringW(himc, what, None, 0) };
    if bytes <= 0 {
        return Some(String::new());
    }
    let units = (bytes as usize / 2).min(MAX_COMPOSITION);
    let mut buf = vec![0u16; units];
    // SAFETY: `buf` holds `units * 2` bytes, the length passed.
    let got = unsafe { ImmGetCompositionStringW(himc, what, Some(buf.as_mut_ptr().cast()), (units * 2) as u32) };
    if got < 0 {
        return None;
    }
    buf.truncate((got as usize / 2).min(units));
    Some(String::from_utf16_lossy(&buf))
}

/// Reads what the IME produced for `flags` from the window's input context.
pub(crate) fn read_composition(hwnd: HWND, flags: u32) -> Composition {
    let mut out = Composition::default();
    // SAFETY: `hwnd` is one of our windows.
    let himc = unsafe { ImmGetContext(hwnd) };
    if himc.0.is_null() {
        return out;
    }
    if flags & GCS_RESULTSTR_BIT != 0 {
        out.result = read_string(himc, GCS_RESULTSTR);
    }
    if flags & GCS_COMPSTR_BIT != 0 {
        let text = read_string(himc, GCS_COMPSTR).unwrap_or_default();
        let cursor = if flags & GCS_CURSORPOS_BIT != 0 {
            // SAFETY: the value is the return code; no buffer.
            let c = unsafe { ImmGetCompositionStringW(himc, GCS_CURSORPOS, None, 0) };
            cursor_in_chars(&text, c.max(0) as usize)
        } else {
            text.chars().count()
        };
        out.update = Some((text, cursor));
    }
    // SAFETY: releases the context taken above.
    unsafe {
        let _ = ImmReleaseContext(hwnd, himc);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_digits_map_by_virtual_key() {
        assert_eq!(map_vk(0x43), Key::Char('c'));
        assert_eq!(map_vk(0x5A), Key::Char('z'));
        assert_eq!(map_vk(0x30), Key::Char('0'));
        assert_eq!(map_vk(0x39), Key::Char('9'));
        assert_eq!(map_vk(0x41), Key::Char('a'));
    }

    #[test]
    fn named_keys_and_function_keys_map() {
        assert_eq!(map_vk(0x0D), Key::Enter);
        assert_eq!(map_vk(0x1B), Key::Escape);
        assert_eq!(map_vk(0x09), Key::Tab);
        assert_eq!(map_vk(0x08), Key::Backspace);
        assert_eq!(map_vk(0x2E), Key::Delete);
        assert_eq!(map_vk(0x20), Key::Space);
        assert_eq!(map_vk(0x26), Key::ArrowUp);
        assert_eq!(map_vk(0x28), Key::ArrowDown);
        assert_eq!(map_vk(0x25), Key::ArrowLeft);
        assert_eq!(map_vk(0x27), Key::ArrowRight);
        assert_eq!(map_vk(0x21), Key::PageUp);
        assert_eq!(map_vk(0x22), Key::PageDown);
        assert_eq!(map_vk(0x24), Key::Home);
        assert_eq!(map_vk(0x23), Key::End);
        assert_eq!(map_vk(0x70), Key::F(1));
        assert_eq!(map_vk(0x7B), Key::F(12));
        assert_eq!(map_vk(0x87), Key::F(24));
        assert_eq!(map_vk(0x88), Key::Other(0x88));
        assert_eq!(map_vk(0xE5), Key::Other(0xE5));
        assert_eq!(map_vk(0x60), Key::Other(0x60), "numpad digits are not Char keys");
    }

    #[test]
    fn vk_of_is_the_inverse_of_map_vk() {
        for vk in (0x08u16..=0x87).chain([0xBA, 0xE5]) {
            let key = map_vk(vk);
            if matches!(key, Key::Other(_)) {
                assert_eq!(vk_of(key), vk);
            } else {
                assert_eq!(map_vk(vk_of(key)), key, "vk {vk:#x}");
            }
        }
        assert_eq!(vk_of(Key::Char('q')), 0x51);
        assert_eq!(vk_of(Key::F(5)), 0x74);
    }

    #[test]
    fn the_ime_composition_window_request_is_cleared_and_the_rest_kept() {
        let all = 0x8000_0000u32 as i32 as isize & 0xFFFF_FFFF;
        let other = 0x4000_000Fisize;
        assert_eq!(without_composition_window(all | other), other);
        assert_eq!(without_composition_window(other), other);
        assert_eq!(without_composition_window(0), 0);
    }

    #[test]
    fn repeat_is_bit_30_of_lparam() {
        assert!(!is_repeat(key_lparam(true, false, false)));
        assert!(is_repeat(key_lparam(true, true, false)));
        assert!(is_repeat(0x4000_0001));
        assert!(!is_repeat(0x0000_0001));
    }

    #[test]
    fn surrogates_join_and_stray_halves_are_dropped() {
        let mut j = CharJoiner::default();
        assert_eq!(j.push(b'a' as u16), Some('a'));
        // U+1F600 is D83D DE00.
        assert_eq!(j.push(0xD83D), None);
        assert_eq!(j.push(0xDE00), Some('\u{1F600}'));
        assert_eq!(j.push(0xDE00), None, "a low half with no high half");
        assert_eq!(j.push(0xD83D), None);
        assert_eq!(j.push(b'b' as u16), Some('b'), "a high half followed by a plain unit is dropped");
        assert_eq!(j.push(0xDE00), None, "the earlier high half is gone");
    }

    #[test]
    fn control_characters_are_not_text() {
        for c in ['\r', '\n', '\t', '\u{8}', '\u{1B}', '\u{7F}', '\u{3}'] {
            assert_eq!(text_char(c), None, "{:?}", c);
        }
        assert_eq!(text_char('a'), Some('a'));
        assert_eq!(text_char(' '), Some(' '));
        assert_eq!(text_char('\u{4E2D}'), Some('\u{4E2D}'));
    }

    #[test]
    fn composition_cursor_counts_chars_not_utf16_units() {
        assert_eq!(cursor_in_chars("abc", 2), 2);
        assert_eq!(cursor_in_chars("a\u{1F600}b", 3), 2, "an emoji is two units, one char");
        assert_eq!(cursor_in_chars("a\u{1F600}b", 1), 1);
        assert_eq!(cursor_in_chars("abc", 99), 3);
        assert_eq!(cursor_in_chars("", 0), 0);
    }
}

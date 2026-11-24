//! The native right-click menu. Opened only when the app asks (`Ui::show_menu`),
//! never from a window procedure: a widget's own right-click handler (the media
//! widget's "next track") must always win over it.

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetForegroundWindow, IsWindow, MF_SEPARATOR, MF_STRING, PostMessageW,
    SetForegroundWindow, TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TPM_TOPALIGN, TrackPopupMenu, WM_NULL,
};
use windows::core::PCWSTR;

use crate::{MenuItem, sys};

/// (item, command id, label). Ids start at 1: `TrackPopupMenu` returns 0 for
/// "dismissed".
pub(crate) const MENU_ITEMS: [(MenuItem, u32, &str); 4] = [
    (MenuItem::Reload, 1, "Reload"),
    (MenuItem::OpenConfig, 2, "Open config folder"),
    (MenuItem::OpenLog, 3, "Open log"),
    (MenuItem::Quit, 4, "Quit"),
];

pub(crate) fn item_from_id(id: u32) -> Option<MenuItem> {
    MENU_ITEMS.iter().find(|(_, i, _)| *i == id).map(|(m, _, _)| *m)
}

/// Shows the menu at screen position `at` and waits (modal) for a pick. The bar
/// window is made the foreground window only while the menu is open, so a
/// click elsewhere closes it; the previous foreground window gets it back
/// afterwards, so the user's typing is not left going nowhere.
pub(crate) fn track(hwnd: HWND, at: POINT, above: bool) -> Option<MenuItem> {
    // SAFETY: every handle is one this function creates or our own window;
    // the label buffers outlive the AppendMenuW calls that copy them.
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        for (item, id, label) in MENU_ITEMS {
            if item == MenuItem::Quit {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            }
            let text = sys::wide(label);
            let _ = AppendMenuW(menu, MF_STRING, id as usize, PCWSTR(text.as_ptr()));
        }
        let previous = GetForegroundWindow();
        let _ = SetForegroundWindow(hwnd);
        let align = if above { TPM_BOTTOMALIGN } else { TPM_TOPALIGN };
        let picked = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | align, at.x, at.y, None, hwnd, None).0;
        // Documented companion of TrackPopupMenu: lets the menu's own
        // dismissal finish before the next message.
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        if GetForegroundWindow() == hwnd && previous != hwnd && IsWindow(Some(previous)).as_bool() {
            let _ = SetForegroundWindow(previous);
        }
        u32::try_from(picked).ok().and_then(item_from_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_map_to_items_and_back() {
        for (item, id, _) in MENU_ITEMS {
            assert_eq!(item_from_id(id), Some(item));
        }
        assert_eq!(item_from_id(0), None, "0 is a dismissed menu");
        assert_eq!(item_from_id(99), None);
    }

    #[test]
    fn ids_are_unique_and_nonzero() {
        let mut ids: Vec<u32> = MENU_ITEMS.iter().map(|(_, i, _)| *i).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), MENU_ITEMS.len());
        assert!(ids.iter().all(|&i| i != 0));
    }
}

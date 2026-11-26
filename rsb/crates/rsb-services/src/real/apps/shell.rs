//! The shell calls behind the apps service: the AppsFolder list, icons, Start menu change
//! notices, and launching. Everything here runs on an STA thread (the list and the icons on the
//! service's own, a launch on a short-lived one).

use std::sync::atomic::{AtomicU32, Ordering};

use rsb_core::pixels::RgbaImage;
use windows::Win32::Foundation::{HWND, SIZE};
use windows::Win32::Graphics::Gdi::{DeleteObject, GetDC, GetDeviceCaps, HGDIOBJ, LOGPIXELSX, ReleaseDC};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_EnumItems, FOLDERID_CommonPrograms, FOLDERID_Programs, IEnumShellItems, IShellItem, IShellItemImageFactory,
    KF_FLAG_DEFAULT, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHCNE_CREATE, SHCNE_DELETE, SHCNE_MKDIR,
    SHCNE_RENAMEFOLDER, SHCNE_RENAMEITEM, SHCNE_RMDIR, SHCNE_UPDATEDIR, SHCNE_UPDATEITEM, SHCNRF_ShellLevel,
    SHChangeNotifyDeregister, SHChangeNotifyEntry, SHChangeNotifyRegister, SHCreateItemFromParsingName,
    SHELLEXECUTEINFOW, SHGetKnownFolderIDList, SIGDN, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING,
    SIIGBF_ICONONLY, SIIGBF_INCACHEONLY, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_MESSAGE, SW_SHOWNORMAL, WINDOW_EX_STYLE, WS_POPUP,
};
use windows::core::{Interface, PCWSTR, w};

use crate::real::images;
use crate::win::wide;

/// Names are cut to this many chars; an app list longer than `MAX_APPS` is cut.
const MAX_NAME: usize = 128;
const MAX_APPS: usize = 4096;
/// Launches running at once; a script that asks for more gets an error, not more threads.
const MAX_LAUNCHES: u32 = 8;

/// One Start menu entry as published.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
}

/// An entry with its shell item, for icons. COM pointers of the STA thread that listed it.
pub(crate) struct Listed {
    pub entry: AppEntry,
    pub item: IShellItem,
}

fn display_name(item: &IShellItem, kind: SIGDN) -> Option<String> {
    // SAFETY: a plain COM call on a live item; the returned string is freed below.
    let p = unsafe { item.GetDisplayName(kind) }.ok()?;
    // SAFETY: `p` is a valid NUL-terminated string until freed.
    let s = unsafe { p.to_string() }.ok();
    // SAFETY: frees the string GetDisplayName allocated, once.
    unsafe { CoTaskMemFree(Some(p.0.cast_const().cast())) };
    s
}

/// Every app of the shell's AppsFolder with a name and a launch id, sorted by name. The id is the
/// item's parsing name inside the folder (an AppUserModelID for packaged apps, a path form for
/// desktop shortcuts): what `shell:AppsFolder\<id>` opens again. Duplicates, empty names and ids
/// that could not be launched (control characters, over `MAX_ID` chars) are left out.
pub(crate) fn list_apps(max_id: usize) -> Result<Vec<Listed>, String> {
    // SAFETY: standard shell calls from an initialised STA thread.
    let folder: IShellItem = unsafe { SHCreateItemFromParsingName(w!("shell:AppsFolder"), None) }
        .map_err(|e| format!("the apps folder could not be opened: {e}"))?;
    // SAFETY: as above.
    let items: IEnumShellItems = unsafe { folder.BindToHandler(None, &BHID_EnumItems) }
        .map_err(|e| format!("the apps folder could not be listed: {e}"))?;
    let mut out: Vec<Listed> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    while out.len() < MAX_APPS {
        let mut next = [None];
        let mut fetched = 0u32;
        // SAFETY: `next` holds one slot and `fetched` is a live out pointer.
        let hr = unsafe { items.Next(&mut next, Some(&mut fetched)) };
        let Some(item) = next[0].take().filter(|_| hr.is_ok() && fetched == 1) else { break };
        let (Some(name), Some(id)) =
            (display_name(&item, SIGDN_NORMALDISPLAY), display_name(&item, SIGDN_PARENTRELATIVEPARSING))
        else {
            continue;
        };
        if name.trim().is_empty() || !crate::real::apps::launchable_id(&id, max_id) || !seen.insert(id.clone()) {
            continue;
        }
        out.push(Listed { entry: AppEntry { id, name: crate::util::cut_chars(name.trim(), MAX_NAME) }, item });
    }
    out.sort_by(|a, b| {
        a.entry.name.to_lowercase().cmp(&b.entry.name.to_lowercase()).then_with(|| a.entry.id.cmp(&b.entry.id))
    });
    Ok(out)
}

/// The icon side in pixels: 32 at 100 % scaling, more on a scaled desktop.
pub(crate) fn icon_side() -> i32 {
    // SAFETY: the screen DC is released below; LOGPIXELSX is a plain capability query.
    let dpi = unsafe {
        let dc = GetDC(None);
        let dpi = GetDeviceCaps(Some(dc), LOGPIXELSX);
        ReleaseDC(None, dc);
        dpi
    };
    let scale = if dpi > 0 { f64::from(dpi) / 96.0 } else { 1.0 };
    (32.0 * scale.clamp(1.0, 3.0)).round() as i32
}

/// The item's icon at `side` px. `cached_only` asks for an icon the shell already has
/// (`SIIGBF_INCACHEONLY`), which loads no icon handler into this process; without it the shell may
/// extract one from the app.
pub(crate) fn icon_of(item: &IShellItem, side: i32, cached_only: bool) -> Result<RgbaImage, String> {
    let factory: IShellItemImageFactory = item.cast().map_err(|e| format!("no image factory: {e}"))?;
    let flags = if cached_only { SIIGBF_ICONONLY | SIIGBF_INCACHEONLY } else { SIIGBF_ICONONLY };
    // SAFETY: a plain COM call on a live item; the bitmap is deleted below.
    let bitmap =
        unsafe { factory.GetImage(SIZE { cx: side, cy: side }, flags) }.map_err(|e| format!("no icon: {e}"))?;
    let image = images::from_hbitmap(bitmap.0 as isize);
    // SAFETY: the bitmap is ours and no longer used.
    let _ = unsafe { DeleteObject(HGDIOBJ(bitmap.0)) };
    image
}

/// Whether the shell hands back an icon bitmap for the item without extracting anything.
#[cfg(test)]
pub(crate) fn has_cached_icon(item: &IShellItem, side: i32) -> bool {
    let Ok(factory) = item.cast::<IShellItemImageFactory>() else { return false };
    // SAFETY: a plain COM call on a live item; the bitmap is deleted below.
    match unsafe { factory.GetImage(SIZE { cx: side, cy: side }, SIIGBF_ICONONLY | SIIGBF_INCACHEONLY) } {
        Ok(bitmap) => {
            // SAFETY: the bitmap is ours and no longer used.
            let _ = unsafe { DeleteObject(HGDIOBJ(bitmap.0)) };
            true
        }
        Err(_) => false,
    }
}

/// Start menu change notices for the two Start menu folders, posted to a message-only window of
/// this thread as `message`.
pub(crate) struct StartMenuWatch {
    hwnd: HWND,
    registration: u32,
}

impl StartMenuWatch {
    pub(crate) fn new(message: u32) -> Option<StartMenuWatch> {
        // SAFETY: the predefined STATIC class needs no instance; the window is destroyed on drop.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        }
        .ok()?;
        let mut lists: Vec<*mut ITEMIDLIST> = Vec::new();
        for folder in [&FOLDERID_Programs, &FOLDERID_CommonPrograms] {
            // SAFETY: a plain shell call; the list is freed below.
            if let Ok(pidl) = unsafe { SHGetKnownFolderIDList(folder, KF_FLAG_DEFAULT.0 as u32, None) } {
                lists.push(pidl);
            }
        }
        let entries: Vec<SHChangeNotifyEntry> =
            lists.iter().map(|p| SHChangeNotifyEntry { pidl: *p, fRecursive: true.into() }).collect();
        let events = SHCNE_CREATE.0
            | SHCNE_DELETE.0
            | SHCNE_MKDIR.0
            | SHCNE_RMDIR.0
            | SHCNE_RENAMEITEM.0
            | SHCNE_RENAMEFOLDER.0
            | SHCNE_UPDATEITEM.0
            | SHCNE_UPDATEDIR.0;
        let registration = if entries.is_empty() {
            0
        } else {
            // SAFETY: `entries` and the lists they point to are alive for the call, which copies them.
            unsafe {
                SHChangeNotifyRegister(
                    hwnd,
                    SHCNRF_ShellLevel,
                    events as i32,
                    message,
                    entries.len() as i32,
                    entries.as_ptr(),
                )
            }
        };
        for p in lists {
            // SAFETY: each list came from SHGetKnownFolderIDList and is freed once.
            unsafe { CoTaskMemFree(Some(p.cast_const().cast())) };
        }
        if registration == 0 {
            // SAFETY: the window was created above.
            let _ = unsafe { DestroyWindow(hwnd) };
            return None;
        }
        Some(StartMenuWatch { hwnd, registration })
    }
}

impl Drop for StartMenuWatch {
    fn drop(&mut self) {
        // SAFETY: the registration and the window were made in `new` and are released once.
        unsafe {
            let _ = SHChangeNotifyDeregister(self.registration);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// The calls that start things. A seam so no test starts anything: tests pass a recorder.
pub(crate) trait Shell: Send + Sync {
    /// Starts the app with this AppsFolder id.
    fn launch(&self, id: &str) -> Result<(), String>;
    /// Opens a Windows page by address (the caller has already checked the scheme).
    fn open(&self, uri: &str) -> Result<(), String>;
}

/// `ShellExecuteExW` on a short-lived STA thread, so a slow handler never holds the service.
pub(crate) struct SystemShell;

static LAUNCHES: AtomicU32 = AtomicU32::new(0);

impl Shell for SystemShell {
    fn launch(&self, id: &str) -> Result<(), String> {
        execute(format!("shell:AppsFolder\\{id}"))
    }

    fn open(&self, uri: &str) -> Result<(), String> {
        execute(uri.to_string())
    }
}

fn execute(target: String) -> Result<(), String> {
    if LAUNCHES.fetch_add(1, Ordering::AcqRel) >= MAX_LAUNCHES {
        LAUNCHES.fetch_sub(1, Ordering::AcqRel);
        return Err("too many starts at once".into());
    }
    let spawned = std::thread::Builder::new().name("rsb-launch".into()).spawn(move || {
        // SAFETY: initialises COM for this thread as an STA; balanced below.
        let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        let name = wide(&target);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpFile: PCWSTR(name.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        // SAFETY: `info` and the wide string outlive the call.
        if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
            log::warn!("the shell could not start an item: {e}");
        }
        if com.is_ok() {
            // SAFETY: balances the successful CoInitializeEx above.
            unsafe { CoUninitialize() };
        }
        LAUNCHES.fetch_sub(1, Ordering::AcqRel);
    });
    if let Err(e) = spawned {
        LAUNCHES.fetch_sub(1, Ordering::AcqRel);
        return Err(format!("could not start the launch thread: {e}"));
    }
    Ok(())
}

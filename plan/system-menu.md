# System / power menu (Alt+Shift+Space)

An omarchy-style centered popup for system actions. Sibling to the app launcher
(Alt+Space), reusing its window + render scaffolding.

## v1 SHIPPED 2026-06-27 (+ categories)

Built: `sysmenu_thread` + popup (same rounded card / pill / font as the launcher),
trigger **Alt+Shift+Space** in the keyboard hook (checked before the launcher so the
Shift variant doesn't open the picker), capture mode routing Up/Down/Enter/Esc/←.

**Categorised** (omarchy-style, 2-level, data-driven `SysItem { label, kind }` where
kind = `Category(&[SysItem])` or `Action(SysAct, needs_confirm)`):
- **Power** → Lock (`LockWorkStation`), Sleep (`SetSuspendState`), Sign out / Restart /
  Shut down (`ExitWindowsEx`; reboot/shutdown enable `SeShutdownPrivilege` lazily).
- **Setup** → Settings, Open config folder, Reload, Restart Astur, Screenshot, and
  Wallpaper submenu (directory configurable).
- Root/categories/actions are rebuilt from config on every open. Built-in ordering,
  width, custom categories/actions, icon path or built-in name, and confirm gating are
  configurable and hot-reloaded.

Navigation: Enter drills into a category (chevron `›` marks them; window resizes to the
level via `sysmenu_layout`); **←/Backspace/Esc** all step back one level — cancel a
confirm → back to root → close only from root (Esc was fixed 2026-07-07 to step back
rather than always close; see `known-issues.md`). Session-ending actions are
**confirm-gated** (first Enter arms, second runs).
Hierarchy is runtime `Vec<SysItem>` data assembled from built-ins plus
`system_actions`, matching declarative mod/IPC extension shape.

**Mouse support SHIPPED 2026-07-10**: hover selects (move-guarded so opening under a
still cursor doesn't steal selection; a selection change disarms a pending confirm),
click = the Enter path (drill / arm / run), wheel = Up/Down (routed from the LL mouse
hook via `SYSMENU_RECT_*` bounds published by `sysmenu_layout`), and click-outside
dismisses (posts `SM_CLOSE`, so an armed confirm is cancelled first — safety kept).

Remaining polish: wallpaper thumbnails/grid/random navigation, true per-monitor
`IDesktopWallpaper`, and non-blocking execution for custom commands that invoke slow
shell handlers. Core wallpaper/reload/restart/screenshot/custom actions are shipped.

## Original design / remaining pieces

## Trigger

**Alt+Shift+Space** (Left Alt is Astur's reserved modifier; the Space base is already
the launcher, so Shift+Space is the natural variant). Keyboard hook: when
`ALT_DOWN && shift && VK_SPACE && !SYSMENU_OPEN && !LAUNCHER_OPEN`, push open. Same
capture model as the launcher (hook posts intents; the menu window owns state). Hook
stays light — a flag check + post.

## Window + render

A second owner-drawn `WS_POPUP` topmost `NOACTIVATE` window, same scaffolding as the
launcher (`launcher_paint` patterns): rounded DWM corners, dark surface, rounded
accent selection pill, per-row icon + label, keyboard nav (Up/Down, Enter, Esc). It's
a fixed action list (not a search box), so no query row — just a title + the list.
Factor the shared bits (font, rounded card, pill, icon blit) so both popups use them.

## Actions (v1, omarchy-parity)

| Action | How (Win32) | Notes |
|---|---|---|
| Lock | `LockWorkStation()` | trivial, no privilege |
| Sleep / Suspend | `SetSuspendState(FALSE, FALSE, FALSE)` (powrprof) | feature `Win32_System_Power` already on |
| Hibernate | `SetSuspendState(TRUE, …)` | optional |
| Restart | `ExitWindowsEx(EWX_REBOOT \| EWX_FORCEIFHUNG, …)` | needs `SE_SHUTDOWN_NAME` priv (see below) |
| Shut down | `ExitWindowsEx(EWX_SHUTDOWN \| EWX_FORCEIFHUNG, …)` | needs `SE_SHUTDOWN_NAME` |
| Sign out | `ExitWindowsEx(EWX_LOGOFF, …)` | no special priv |
| Change wallpaper | submenu (below) | `SystemParametersInfoW(SPI_SETDESKWALLPAPER, …)` |
| Restart Astur | relaunch own exe + exit | nice for config/mod reload |
| Open config | `ShellExecute` the `.astur` folder / conf | |
| Screenshot | launch the Snipping Tool (`ms-screenclip:`) | omarchy has this |

**Shutdown/restart privilege:** `ExitWindowsEx` requires the process token to hold
`SE_SHUTDOWN_NAME`, enabled via `OpenProcessToken` + `LookupPrivilegeValue` +
`AdjustTokenPrivileges`. Do this once, lazily, the first time a power action is
invoked (not at startup — keep boot clean). Confirm-before-acting for shutdown/restart
(a second Enter / a yes-row) so a mis-key doesn't kill the session — this is
irreversible, so it gets a confirm step (matches the "confirm hard-to-reverse actions"
rule). Features to add: `Win32_System_Shutdown`, `Win32_Security` (token adjust).

## Wallpaper submenu

- Source: a wallpapers folder — `./wallpapers/` next to the exe (portable) and/or
  `%USERPROFILE%\.astur\wallpapers\`. Enumerate `*.jpg/png/bmp/jpeg`.
- Render thumbnails (reuse `IShellItemImageFactory::GetImage` from the launcher icon
  path, larger size) in a grid or list with a live preview.
- Enter sets it: `SystemParametersInfoW(SPI_SETDESKWALLPAPER, 0, path_wide,
  SPIF_UPDATEINIFILE | SPIF_SENDCHANGE)`. (For per-monitor or fit/fill control use
  `IDesktopWallpaper` COM — backlog.)
- Optional: "random" + "next/prev" entries; a `wallpaper_dir` config key.

## Threading / commands

Mirror the launcher: a `sysmenu_thread` owning the window + pump, idle on its message
queue. Actions that can block (shutdown, wallpaper set, launching tools) run on the
menu thread or a throwaway — never the hooks or the manager. Power/privilege calls are
Win32-only and isolated here.

## Config keys (astur.conf)

Shipped keys: `system_menu_enabled`, `system_menu_width`, `system_power_items`,
`system_setup_items`, `system_actions`, and `wallpaper_dir`. Trigger remains fixed at
Alt+Shift+Space; arbitrary alternate chords can call `system_menu` via `extra_hotkeys`.

## Modularity hook

The action list should be data-driven (a `Vec<SysAction { label, icon, kind }>`) so
**declarative mods can add entries** (e.g. a custom command, a URL, a script) — see
`plan/mods.md`. Built-in actions are just the default entries. This is the same
"providers/entries are data" lever as the launcher.

## Status

Core menu and data-driven customization shipped. Wallpaper thumbnails/grid and
per-monitor backend remain backlog; see current limits in README.

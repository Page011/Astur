# Win32 reference

APIs Astur leans on, where the docs are, and which calls are slow/buggy traps.

## Where to find docs

- **Microsoft Learn (Win32 API index):** https://learn.microsoft.com/windows/win32/api/
  - WindowsAndMessaging (windows, hooks, ShowWindow/SetWindowPos):
    https://learn.microsoft.com/windows/win32/api/winuser/
  - GDI (BitBlt, AlphaBlend, DCs, bitmaps):
    https://learn.microsoft.com/windows/win32/api/wingdi/
  - DWM (borders, attributes, thumbnails):
    https://learn.microsoft.com/windows/win32/api/dwmapi/
  - Accessibility / WinEvents (SetWinEventHook):
    https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-setwineventhook
- **windows-rs crate** (our binding, v0.58): https://microsoft.github.io/windows-docs-rs/
  - Names match Win32 1:1; feature-gated by module (see `Cargo.toml` features).
  - This repo also has the Microsoft Learn MCP available to agents — use it to
    pull exact signatures/behaviour rather than guessing.

## Core APIs in use

| Area | Calls | Notes |
|---|---|---|
| Input hooks | `SetWindowsHookExW(WH_MOUSE_LL / WH_KEYBOARD_LL)`, `CallNextHookEx` | Global, run in our process. **Must return fast.** |
| Window placement | `SetWindowPos` (`SWP_NOACTIVATE|NOZORDER|NOSENDCHANGING`), `ShowWindow(SW_HIDE/SW_SHOWNA/SW_RESTORE)` | Cross-process — keep off hook threads. |
| Enumeration | `EnumWindows`, `EnumDisplayMonitors`, `GetWindowRect`, `GetClassNameW` | |
| Window events | `SetWinEventHook` (`win_event_proc`) | Foreground/create/destroy/minimize tracking. |
| Borders / DWM | `DwmSetWindowAttribute` (border colour, Win11), `DwmGetWindowAttribute` (extended frame bounds) | Border colour is Win11-only; degrade gracefully on Win10. |
| Compositing | `CreateCompatibleDC/Bitmap`, `BitBlt(SRCCOPY)`, `AlphaBlend`, `PrintWindow` | The overlay compositor. DDBs are GPU-backed (~no process RAM). |
| Wallpaper capture | Find `Progman`/`WorkerW`, BitBlt | Used so slide gaps keep the still wallpaper. Can fail → flat slide fallback. |
| Layered windows | `WS_EX_LAYERED`, `UpdateLayeredWindow`, `SetLayeredWindowAttributes` | Needed for per-window snapshot-glide (Option A in `animations.md`). |

## Known-slow / known-buggy — use with care

- **`PrintWindow`** — used to grab window/wallpaper pixels. Slow on some apps;
  GPU-accelerated apps (Chrome with certain flags, some games) return black.
  `PW_RENDERFULLCONTENT` (flag 2) helps but is still per-window expensive. Keep it
  off the manager thread's critical path (the wallpaper capture is done on the
  transition worker for this reason).
- **Per-frame `SetWindowPos` (interpolated)** — DO NOT. Cross-process per frame,
  lands unreliably, stalls on slow apps. Removed once already. See `animations.md`.
- **`SetWindowPos` without `SWP_NOSENDCHANGING`** — sends `WM_WINDOWPOSCHANGING`,
  letting apps veto/clamp the rect. We pass `NOSENDCHANGING` for tile placement so
  apps can't fight the layout. (Trade-off: a few apps ignore size constraints.)
- **`GetWindowRect` mid-flight** — returns transient positions during an animation
  or drag; never snapshot layout from live rects while a tween is running. Cache
  the intended rects instead.
- **DWM border attribute on Win10** — `DwmSetWindowAttribute(DWMWA_BORDER_COLOR)`
  is Win11 22000+. No-op/err on older — guard, don't assume.
- **`AlphaBlend`** — software-composited unless source/dest are nice formats; fine
  for a single full-frame fade per workspace switch, do not call it per window per
  frame in a tight loop without profiling.

## Gotchas

- **LL keyboard hook delivers SPECIFIC left/right VK codes**, never the generic
  aggregate. Physical Shift arrives as `VK_LSHIFT` (0xA0) / `VK_RSHIFT` (0xA1), NOT
  `VK_SHIFT` (0x10); Alt as `VK_LMENU`/`VK_RMENU`; Ctrl as `VK_LCONTROL`/`VK_RCONTROL`.
  (The generic codes only appear via `GetKeyState`/`GetAsyncKeyState` aggregation.)
  Comparing `kb.vkCode` against a generic `VK_SHIFT` silently never matches — the
  "phantom Shift" bug (see `known-issues.md` 2026-07-07). Use `is_modifier_vk`.
- **`SWP_ASYNCWINDOWPOS`** — for cross-process `SetWindowPos` from a worker that must
  not block on a busy foreign app (the drag `position_worker`). Posts the request to
  the target's queue and returns immediately. Only for the transient drag-follow;
  the authoritative final rect is re-applied synchronously on drop.
- Swallowing a modifier key-UP in a hook (returning 1) while a capture mode is open
  leaves `GetAsyncKeyState` reporting that modifier stuck down globally — always let
  modifier keys fall through.
- Hidden windows (`SW_HIDE`) still report rects; filter on workspace membership,
  not visibility, when laying out.
- A topmost overlay must `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` and swallow
  `WM_ERASEBKGND` (return 1) or it flashes the class brush before frame 0.
- Elevated windows (Task Manager) can't be managed without running Astur as admin
  (UIPI blocks `SetWindowPos` into a higher-IL process).

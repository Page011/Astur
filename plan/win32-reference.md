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

- **Direct-to-window GDI paint flickers** (2026-07-13) — `FillRect` background +
  incremental draws on the window DC show intermediate states on screen (launcher
  icons blinked every wheel notch). `InvalidateRect(erase=FALSE)` +
  `WM_ERASEBKGND=>1` does NOT fix it; the paint must be atomic. Pattern:
  `backbuf_begin/backbuf_end` (CreateCompatibleDC + CreateCompatibleBitmap, draw
  everything, one `BitBlt`). Used by launcher/sysmenu/bar paints.
- **`WS_EX_NOACTIVATE` windows never get `WM_MOUSEWHEEL`** (2026-07-13) — the wheel
  targets the focused window only. Route it from the LL mouse hook with published
  rect atomics + `PostMessageW` (launcher `LA_SCROLL`, sysmenu `SM_UP/DOWN`, bar
  `WM_BAR_WHEEL`). Rects must be lock-free (hook rule): fixed atomic arrays
  (`BARHIT_*`), gated by one `BARS_HOT` load when idle.
- **`SetWindowCompositionAttribute`** (2026-07-13) — undocumented user32 export
  used for the opt-in acrylic accent (attr 19 / accent state 4). Resolved via
  `GetProcAddress` at call time; may silently stop working on future Windows.
  Never rely on it for correctness — cosmetic only, config default off.
- **`GlobalFree` lives in `Win32::Foundation`** in windows-rs 0.58 (not
  `System::Memory` where GlobalAlloc/Lock are) — clipboard code trap.
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

- **`SHIL_JUMBO` corner-sprite**: icons with no 256px frame return a small sprite in
  the corner of the 256px cell — downscale that and you get a speck. **`DrawIconEx`
  scaling is low-quality** — only draw HICONs 1:1; get scaling from
  `IShellItemImageFactory::GetImage` at the exact size (shell scales HQ). See
  `known-issues.md` 2026-07-10.
- **Mouse wheel in the LL hook**: `WM_MOUSEWHEEL` arrives with the signed delta in
  the HIGH word of `MSLLHOOKSTRUCT.mouseData` (`(mouseData >> 16) as u16 as i16`).
  Routing wheel via the hook is deterministic; wheel delivery to unfocused windows
  is otherwise a user setting ("scroll inactive windows").
- **LL keyboard hook delivers SPECIFIC left/right VK codes**, never the generic
  aggregate. Physical Shift arrives as `VK_LSHIFT` (0xA0) / `VK_RSHIFT` (0xA1), NOT
  `VK_SHIFT` (0x10); Alt as `VK_LMENU`/`VK_RMENU`; Ctrl as `VK_LCONTROL`/`VK_RCONTROL`.
  (The generic codes only appear via `GetKeyState`/`GetAsyncKeyState` aggregation.)
  Comparing `kb.vkCode` against a generic `VK_SHIFT` silently never matches — the
  "phantom Shift" bug (see `known-issues.md` 2026-07-07). Use `is_modifier_vk`.
- **DWM live thumbnail** (`DwmRegisterThumbnail` → `DwmUpdateThumbnailProperties` →
  `DwmUnregisterThumbnail`) mirrors ANY top-level window into a dest window, GPU-
  composited — works on GPU apps (Chrome) where `PrintWindow` is black. Used for the
  live move-drag preview. windows-rs 0.58: the id is a raw `isize` (no `HTHUMBNAIL`
  type); `DwmRegisterThumbnail(dest, src) -> Result<isize>`. It PRESERVES the source
  aspect ratio (letterboxes if the dest aspect differs). To avoid showing the original
  AND the thumbnail, the real window is parked far off-screen (`-32000`) during the
  drag — off-screen (NOT `SW_HIDE`/minimize, which blank the thumbnail source) keeps
  it composited, and `commit_rect` restores it on release. Accept the small risk that
  a hard crash mid-drag strands it; button-up always restores.
- Swallowing a modifier key-UP in a hook (returning 1) while a capture mode is open
  leaves `GetAsyncKeyState` reporting that modifier stuck down globally — always let
  modifier keys fall through.
- Hidden windows (`SW_HIDE`) still report rects; filter on workspace membership,
  not visibility, when laying out.
- A topmost overlay must `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` and swallow
  `WM_ERASEBKGND` (return 1) or it flashes the class brush before frame 0.
- Elevated windows (Task Manager) can't be managed without running Astur as admin
  (UIPI blocks `SetWindowPos` into a higher-IL process).

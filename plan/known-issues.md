# Known issues / traps / code to avoid

Dated. Newest on top. "Don't use X because Y" goes here with the reason.

## 2026-07-13 — RESOLVED: theme-vs-custom colours — heuristics dead, tri-state shipped

Two heuristics for "did the user customise this bar colour?" failed in a row:
per-field default-matching mixed presets with custom colours (black on black);
all-or-nothing froze the bar dark forever once ANY colour had ever been touched
by the GUI ("navbar doesn't update to light mode"). Lesson: NEVER infer intent
from value-equals-default — the shipped template and the GUI both write the
default values out literally. Now explicit tri-state: navbar colours are
`auto` (follow the theme; resolves against the shared `BAR_DARK`/`BAR_LIGHT`
presets in astur-config) or `#RRGGBB` (always wins). `Option<u32>` in Config,
`auto` in the conf/template, an "Auto" checkbox per colour in the GUI.
Migration: a colour equal to the OLD shipped dark default parses as auto, so
pre-theme files pick up light mode; GUI-wiggled values stay custom (tick Auto).

## 2026-07-13 — egui dark labels are ~gray(140) — restyle for contrast

Default egui dark visuals render labels at ~55% grey on a near-black panel —
users read it as broken ("settings text is so hard to read"). The settings GUI
now lifts `widgets.*.fg_stroke` per theme (dark labels gray(222), light
gray(25)) via `ctx.style_mut_of(Theme::Dark/Light, …)`. Do the same in any
future egui surface.

## 2026-07-13 — RESOLVED: hidden-workspace windows untracked by our OWN hide events

THE root cause of "windows on other workspaces died" (the crash-rescue below was
the safety net; this was the disease): `SUPPRESS` is a time-window flag, but
WinEvents are OUT-OF-CONTEXT — queued to the main thread. The tail of a
workspace switch's SW_HIDE batch could be delivered AFTER the manager cleared
SUPPRESS, so `EVENT_OBJECT_HIDE` pushed `Cmd::Remove` for live windows → hidden
AND untracked → orphaned. Intermittent (worse under load / many windows).
Fix: `HIDDEN_BY_US` set — every workspace hide is marked BEFORE its ShowWindow;
`EVENT_OBJECT_HIDE` is ignored for marked windows (app-driven hides still
untrack); `EVENT_OBJECT_SHOW` unmarks (so a later app hide counts again);
`EVENT_OBJECT_DESTROY` now ALWAYS untracks (a stray Remove is a no-op). Trap:
never rely on a time-window flag to classify async WinEvents — mark the affected
window identities instead.

## 2026-07-13 — RESOLVED: theme colour failures (mixed presets, GUI stuck light)

Three colour bugs from the first theming pass:
1. **Bar preset substitution was per-field** — one customised colour (e.g. a
   user-picked dark bg) + light-preset near-black text = black on black. Now
   ALL-OR-NOTHING: the light bar preset applies only when all four bar colours
   are still at their dark defaults; any customisation keeps the user's full set.
2. **Settings GUI never set an egui theme** — it trusted eframe's system detect
   (unreliable here). Now `ctx.set_theme` maps Astur's `theme` directly
   (dark/light/auto→System), re-applied each frame so the combo previews live.
3. **Acrylic's whole-window LWA_ALPHA in light mode** washed the light surface
   into whatever light window sat underneath — read as "white on white". The
   fade now applies in DARK theme only; light acrylic popups stay opaque.
Also: light palettes re-tuned for contrast (popups #F2F4F7/#14161A, bar
#ECEEF2/#1B1E24, dims darkened).

## 2026-07-13 — RESOLVED: hard kill orphaned hidden-workspace windows ("died")

Astur hides inactive-workspace windows with SW_HIDE; graceful exits restore
them, but taskkill /F, Task Manager "End task" on the no-console build, or a
crash skipping the panic hook left them hidden forever. Fixes, layered:
1. **Crash-rescue file** — the manager persists the currently-hidden set
   (`~/.astur/rescue.lst`, hwnd+pid+class, written only when the set changes via
   a hash guard in `sync_managed`); the next launch verifies each entry (same
   hwnd AND pid AND class — a recycled HWND can never show someone else's
   deliberately-hidden window) and `SW_SHOWNA`s survivors before adoption, so
   they come back on the active workspace. Graceful restores delete the file.
2. **Marker handles WM_CLOSE / WM_QUERYENDSESSION / WM_ENDSESSION** — the
   release build is windows-subsystem, so the console ctrl handler never fires;
   End-task's WM_CLOSE and logoff now restore-all before dying.
Note: nothing can restore DURING a hard kill — the rescue runs at next launch.

## 2026-07-13 — monitor unplug: windows collate to surviving monitor (by design)

`refresh_monitors` already re-homes every window from an unplugged monitor onto
the surviving ones, preserving its (global or per-monitor) workspace number, and
normalizes visibility (active ws shown, others hidden). So after an unplug the
windows ARE on the main monitor — some just live on its other workspaces
(reachable via Alt+N / bar pills), which can read as "disappeared" if you expect
Windows' flatten-everything behaviour. If that expectation wins, add a config
(`unplug = collate|flatten`) that dumps gone-monitor windows onto the surviving
ACTIVE workspace instead. Watch for user feedback before adding it.

## 2026-07-13 — RESOLVED: link/taskbar activation of a hidden-workspace window broke layout

An app surfacing its own window on a hidden workspace (clicking a link activates
the browser on ws2 while ws1 is shown) made the window visible OVER the current
workspace while the manager still tracked it on the hidden one — overlapping,
"half-pulled" mess. Fix: FOLLOW the activation. `Cmd::Focused` on a tracked
window whose workspace isn't active switches to that workspace; `Cmd::Add` for an
already-tracked window does the same but only when it's visible AND foreground
(so background self-shows — toasts, splash repaints — can't yank the workspace).
Never move the window to the current workspace: that breaks both layouts.

## 2026-07-13 — RESOLVED: Shift dead in the launcher (MAPVK_VK_TO_CHAR trap)

`MapVirtualKeyW(vk, MAPVK_VK_TO_CHAR)` returns the UNSHIFTED base character —
Shift+8 gave '8' not '*', and the result was force-lowercased. No capitals, and
the inline calculator's `+ * ( ) ^ %` were untypeable. Fix: the hook posts
vk + scancode + shift + caps packed in `LA_KEY`; the launcher thread converts
with `ToUnicode` + a synthetic 256-byte key state. Trap: do NOT call `ToUnicode`
inside the LL hook itself — it can desync dead-key state for the foreground app.

## 2026-07-13 — RESOLVED: launcher icons flashed on every wheel scroll

All three owner-drawn surfaces (launcher, sysmenu, bar) painted STRAIGHT to the
window DC: the background `FillRect` wiped the previous frame on screen before the
icons/text landed, so every repaint (wheel scroll, hover, 120Hz pill slide) visibly
blinked. Trap: `InvalidateRect(…, erase=FALSE)` + `WM_ERASEBKGND => 1` is NOT
enough — the paint itself must be atomic. Fix: `backbuf_begin`/`backbuf_end`
(memory DC + compatible bitmap, single `BitBlt`) wraps all three paints; also
`LA_SCROLL` now skips the repaint entirely when scroll+selection didn't change.

## 2026-07-13 — NOACTIVATE windows never receive WM_MOUSEWHEEL

The wheel goes to the FOCUSED window; the bar/popups are `WS_EX_NOACTIVATE` and
never take focus, so wheel input must be routed from the LL mouse hook (rect
check + `PostMessageW`). The bar's rects live in lock-free `BARHIT_*` atomic
arrays (hooks may not lock); `BARS_HOT` short-circuits to ONE atomic load when
idle. Same pattern as the launcher/sysmenu wheel routing.

## 2026-07-13 — acrylic on plain GDI popups is best-effort (experimental)

`SetWindowCompositionAttribute` + `ACCENT_ENABLE_ACRYLICBLURBEHIND` is
undocumented; opaque GDI paint covers the accent, so the popups also get
whole-window `LWA_ALPHA` (236) to let the blur read through. Acceptable for an
opt-in; a real acrylic surface needs DirectComposition. Config-gated
(`acrylic = false` default) so it can't hurt anyone who didn't ask for it.

## 2026-07-08 — RESOLVED: ghost tile (a dead window still held a slot)

A destroyed window whose `EVENT_OBJECT_DESTROY` was missed (WinEvent hooks drop
events under load) stayed in the workspace list, so `workspace_layout` reserved an
empty tile for it — seen as a gap showing the wallpaper ("ghost window taking a
tile"). Fix: the tiled filter now also requires `IsWindow(h)`, so a dead HWND can't
hold a slot. Also: the `astur-settings` stub is now `windows_subsystem = "windows"`
so launching it from the tray no longer flashes a console window the WM could briefly
tile. Future hardening: a periodic missed-destroy sweep over the whole managed set.

## 2026-07-08 — Live DWM thumbnail for move AND resize (Chrome-safe)

Alt-move and Alt-resize mirror the dragged window live via `DwmRegisterThumbnail`
into a topmost overlay — GPU-composited, so it works on Chrome (unlike `PrintWindow`,
which returns black on GPU apps). The real window is **parked far off-screen**
(`-32000,-32000`, same size) for the duration so the user sees only the thumbnail,
not the original AND a copy. Off-screen (not `SW_HIDE`/minimize) keeps it DWM-
composited so the thumbnail stays live; `commit_rect` restores it on release. The
only loss risk is a hard crash mid-drag; button-up always restores, so it's
acceptable (standard technique). In windows-rs 0.58 the thumbnail id is a raw `isize`
(NO `HTHUMBNAIL` type); `DwmRegisterThumbnail(dest, src) -> Result<isize>`. DWM
thumbnails preserve source aspect ratio, so resize letterboxes when the aspect
changes (accepted — user chose live content over the outline). Falls back to the
outline if registration fails (then the real window is NOT parked off-screen).

## 2026-07-07 — RESOLVED: move/resize slow — live cross-process SetWindowPos per frame

Alt-move / Alt-resize repositioned the REAL window every mouse-move via a
`position_worker` (`set_target`). Moving another process's window live forces that
app to process `WM_WINDOWPOSCHANGED` and repaint each step; resizing forces a full
client re-layout per pixel — a browser/Electron can't keep up, so it felt "awfully
slow." (Astur itself measured 0.4% CPU / 48 MB — NOT the bottleneck; the foreign
app's repaint is.) A Windows WM can't own another app's surface the way Mac/Wayland
compositors do, so live is a dead end. Fix: drag a cheap in-process **outline
overlay** (`OUTLINE_HWND`, a region-shaped frame) following the cursor and commit
the final rect to the real window ONCE on release (`commit_rect`) — same reason
"show window contents while dragging = off" is instant. Removed `position_worker` /
`set_target` / `Target`. Fancier future path with live content: a DWM thumbnail
proxy (`DwmRegisterThumbnail`, GPU-composited, works even for Chrome where
`PrintWindow` is black). (`architecture.md`, `win32-reference.md`)

## 2026-07-10 — RESOLVED: jumbo icon pass REGRESSED quality ("quality just died")

The 2026-07-08 change below made icons WORSE, for two reasons that are now traps:
1. **`DrawIconEx` downscaling is low-quality.** Scaling a 256px HICON into a 32px
   box uses plain stretch arithmetic — mushy/aliased. High-quality scaling only
   happens where the SHELL scales for you (`IShellItemImageFactory::GetImage` at the
   requested size).
2. **`SHIL_JUMBO` corner-sprite gotcha.** Icons that ship no 256px frame come back
   as a small 32px sprite in the TOP-LEFT CORNER of the 256px cell; drawn scaled
   down, the visible icon becomes a near-invisible speck.
Fix (current pipeline, `load_icon`): (1) `IShellItemImageFactory::GetImage` at
EXACTLY `LAUNCHER_ICON_PX` (shell picks the best frame + scales HQ; handles .lnk,
.exe, UWP AppsFolder names) wrapped to HICON; (2) fallback `SHGetFileInfo` +
`SHGetImageList(SHIL_LARGE)` (native 32px, 1:1); (3) cached generic .exe icon
(`SHGFI_USEFILEATTRIBUTES`) so no row is ever blank. Paint = `DrawIconEx` at 1:1
(no scaling). Traps: never request jumbo to downscale yourself; never scale icons
in `DrawIconEx`; load at the display size.

## 2026-07-10 — Hook purity restored: drag park/commit moved to the manager

The 2026-07-08 thumbnail drag did TWO cross-process `SetWindowPos` calls on the
mouse hook (park at drag start inside `thumb_begin`, commit on button-up) —
violating "hooks are sacred" (a busy foreign app can stall the OS input path).
Now the hook only pushes: `Cmd::DragPark(h)` (manager parks off-screen),
`Cmd::DragMoved(h, x, y, rect)` / `Cmd::DragResized(h, Some(rect))` (manager
`commit_rect`s the previewed rect, BEFORE any early-out so floating/unmanaged/
tiling-off windows land too, then re-tiles). `DragResized(h, None)` is the native
MOVESIZEEND path (reads the live rect). The preview overlays (thumbnail/outline)
are our own windows and stay on the hook — same precedent as the resize marker.

## 2026-07-07 — RESOLVED: launcher icons had a white halo (straight vs premultiplied alpha)

Launcher app icons showed a white outline on their antialiased edges. Cause: the
paint blits with `AlphaBlend` + `AlphaFormat = AC_SRC_ALPHA` (1), which requires
**premultiplied** BGRA, but `IShellItemImageFactory::GetImage` returns **straight**
(non-premultiplied) alpha — translucent edge pixels then blend too bright → white
halo. Fix: `premultiply_bgra()` multiplies each colour channel by A/255 in the DIB
section (`BITMAP.bmBits`) right after `GetImage`. Also now request the icon at 2×
the display box for crisper downscaling. Trap: any AlphaBlend of a shell-provided
32bpp icon must premultiply first — GetImage/thumbnail bitmaps are straight-alpha.
Still open: some apps' icons don't resolve at all (UWP/failed GetImage) and DPI
scaling of the fixed-px launcher — separate items. (`win32-reference.md`)

## 2026-07-07 — RESOLVED: phantom Shift (stuck-down after Alt+Shift+Space)

Shift read as held when it wasn't — e.g. Alt+3 acted as Alt+Shift+3 (move-to-ws
instead of switch), Alt+Space opened the system menu not the launcher. Cause: the
launcher/sysmenu capture blocks in `keyboard_proc` computed `is_mod` by comparing
`kb.vkCode` against the **generic** `VK_SHIFT` (0x10). The LL keyboard hook delivers
the **specific** codes (`VK_LSHIFT` 0xA0 / `VK_RSHIFT` 0xA1), so the check never
matched a real Shift → while a menu was open Shift was treated as a normal key and
**swallowed** (`return LRESULT(1)`). Releasing Shift before closing the menu (natural
after Alt+Shift+Space) meant the key-UP never reached the system, so
`GetAsyncKeyState(VK_SHIFT)` stayed stuck-down globally. Fix: `is_modifier_vk(vk)`
covers the generic AND both L/R specifics for Shift/Alt/Ctrl; both capture blocks use
it so modifiers always fall through. Trap: LL keyboard hook gives SPECIFIC L/R vkCodes
— never match a physical modifier against its generic VK. (`win32-reference.md`)

## 2026-07-07 — sysmenu Esc now steps back a level (was: always close)

Pressing Esc inside a system-menu submenu (e.g. Power) closed the whole menu. The
hook posted `SM_CLOSE` for Esc, which closes regardless of depth. Now Esc posts
`SM_BACK` (same as Left/Backspace): cancel a confirm → back to root → close only from
root. `SM_CLOSE` is now unused (kept as a referenced match arm). (`system-menu.md`)

## 2026-06-27 — RESOLVED: file search was ~900ms/query (leading-wildcard LIKE)

`WHERE System.FileName LIKE '%q%'` (leading wildcard) **scans the whole index** —
measured **914ms** per query on this box (40 results). Felt "way too slow." Measured
alternatives: `LIKE 'q%'` (prefix) 158ms, **`CONTAINS(System.FileName, '"q*"')` 108ms**
(full-text index, ~8× faster). Switched to CONTAINS (`build_contains`: each ≥2-char
word → `"word*"`, AND-ed) + cut the debounce 120ms→45ms. Tradeoff: CONTAINS is
**word-prefix**, not pure substring (won't find "report" inside "quarterlyreport") —
fine for a launcher; true Everything-style substring needs the in-RAM MFT index (see
`plan/roadmap-v2.md`). Trap: never use leading-wildcard LIKE against the Search index.

## 2026-06-27 — RESOLVED: Windows Search SQL has NO `LIKE … ESCAPE` (silent zero results)

File search returned nothing in the running app despite the OLE DB consumer being
probe-verified. Cause: the integrated query added `LIKE '%q%' ESCAPE '\'` to make
typed `%`/`_` literal. The `Search.CollatorDSO` dialect **rejects the ESCAPE clause**
— `ICommand::Execute` fails with `0x80040E14` ("errors during processing of command")
and `run()` returned empty for EVERY query, silently. Confirmed in a probe: identical
query without `ESCAPE` returns 40 rows, with `ESCAPE '\'` errors. Fix: drop the ESCAPE
clause; `sanitize_like` now only doubles `'` (the lone breakout/injection char) and
lets `%`/`_` act as wildcards (harmless). Trap: don't use `LIKE … ESCAPE` against the
Windows Search index. (`launcher.md`.)

## 2026-06-27 — OLE DB: numeric/date columns can't bind WSTR|BYREF

When reading the Windows Search rowset (`filesearch_worker`), binding ALL columns as
`DBTYPE_WSTR | DBTYPE_BYREF` (provider-owned) is tempting (uniform string reads) and
works for string columns (`System.ItemPathDisplay`) — but `System.Size` /
`System.DateModified` come back **empty** (status ≠ S_OK): the provider won't allocate
a string-by-ref for a numeric/date column. Bind those as their native types by value:
`Size` → `DBTYPE_I8` (i64), `DateModified` → `DBTYPE_DATE` (automation date, f64,
convert via the civil-date helper). Verified in the scratchpad probe. (`launcher.md`.)

## 2026-06-27 — Do NOT use `search-ms:` shell enum for file search

For Phase 3 file search, the tempting low-code path was `SHCreateItemFromParsingName
("search-ms:query=…")` → `BindToHandler(BHID_EnumItems)` to reuse the Phase 2
enumeration. **It returns 0 items** (tested `ext:.lnk`, `*.txt`, name terms, with an
async-populate retry) even though WSearch is running and the index is populated. The
working path is OLE DB `Search.CollatorDSO` (`System.FileName LIKE '%q%'`), verified
via ADO. Use OLE DB; don't reach for search-ms. (`launcher.md` Phase 3.)

## 2026-06-27 — RESOLVED: switch/glide flash — frame 0 blitted to a HIDDEN overlay

The real root cause of "windows flash hidden (wallpaper), then reappear and slide."
Both compositors built frame 0 in a back buffer, then `BitBlt` it to the overlay's
window DC **while the overlay was still hidden**, then `ShowWindow`. A blit to a
hidden window's DC is clipped to its (empty) visible region and silently dropped —
so the overlay came up empty, DWM showed the wallpaper underneath, and only the
animation loop's first frame (after the manager had already hidden the outgoing
windows) actually painted it. Visible as the wallpaper flashing through before the
slide. Fix: reorder to **ShowWindow → present frame 0 → `UpdateWindow` → `DwmFlush`
→ signal** in `run_transition` and `run_window_glide`. Now the present lands on the
visible window and is confirmed on the glass before the manager switches underneath.
Trap: never blit to an overlay's DC before `ShowWindow` — show first, then paint.
The 2026-06-26 "first-visit cover-hold" below was an incomplete fix (it relied on
the same lost blit); the cover-hold is kept (it's still correct once frame 0 lands)
but this ordering fix is what actually removes the flash. (`animations.md`)

## 2026-06-26 — RESOLVED: workspace-switch FIRST-visit flash (no overlay)

First entry to a workspace had no cached snapshot, so the switch ran `switch_plain`
bare — no overlay. A window shown via `SW_SHOW` after `SW_HIDE` flashes its
background through until it repaints (DWM discards a hidden window's surface), and
with nothing covering it the pop was fully visible. Fixed: `switch_monitor_workspace`
always raises the overlay when it has an outgoing capture; `run_transition` holds
the outgoing frame for `COVER_HOLD_MS` (48ms) when `in_bmp == 0`, covering the
switch + first paint before the synced reveal. (`animations.md`)

## 2026-06-26 — Do NOT use DWMWA_CLOAK to hide other apps' windows

Tempting flash fix: cloak instead of `SW_HIDE` so the DWM surface survives and the
reveal needs no repaint. **Doesn't work cross-process.** `DwmSetWindowAttribute`
with `DWMWA_CLOAK` only cloaks windows owned by the calling process — the readable
cloaked-state has values for "cloaked by owner app" and "cloaked by Shell" only, no
third-party path (MS docs + community reports). A WM hides foreign windows, so
cloak is a dead end. Use `SW_HIDE` + the cover-hold overlay instead.

## 2026-06-26 — RESOLVED: focus-follows-mouse fought keyboard switches

Dropping the hover poll 80ms → 16ms let the focus-follow worker re-grab focus
within a frame after a keyboard workspace switch / directional focus, snapping
focus back to whatever window the cursor was sitting over. Fixed with a settle
guard: `bump_follow_settle()` (manager thread) sets `FOLLOW_SETTLE_MS = now +
200ms` on every programmatic focus (`switch_monitor_workspace`, `FocusDir`,
`FocusGeo`); the worker skips while inside that window AND syncs `last_pt` so only
a genuine cursor move after expiry fires. Keep the guard if you ever speed the
poll up further. (`architecture.md`)

## 2026-06-26 — RESOLVED: workspace-switch START flash (overlay-up not composited)

`ShowWindow(SW_SHOWNA)` returns before DWM composites the overlay. Signaling the
manager right away let `switch_plain` run underneath before the overlay was on
screen → the destination workspace flashed for a frame. Fixed by `DwmFlush()`
after `ShowWindow` and BEFORE the overlay-up signal (both `run_transition` and
`run_window_glide`). Combined with the frame-0 exact-capture fix below, the start
of the transition is now clean. Trap: never signal overlay-up before a flush —
the whole compositor depends on the overlay genuinely covering before the switch.

## 2026-06-26 — RESOLVED: workspace-switch START flash (frame 0 exact-capture)

The visible "flash before the slide." Frame 0 was rebuilt from the PrintWindow
wallpaper capture + window rects; any diff vs the live DWM desktop (acrylic,
transparency, sub-pixel crop) popped the gaps when the overlay was raised. Fixed
by blitting the exact `capture_monitor` grab for frame 0 in both `run_transition`
and `run_window_glide`; the wallpaper-composited path only runs for moving frames.
Trap: never rebuild frame 0 from a *different* capture than what's live on screen.
(`animations.md`)

## 2026-06-26 — RESOLVED: workspace-switch reveal flash (sync teardown to vblank)

The overlay compositor tore down with `DestroyWindow` off-vblank, exposing a
frame before DWM recomposited the already-placed live windows — a flash where
the stale snapshot vanished a beat before the real window painted. Fixed by
`DwmFlush()` before `DestroyWindow` in both `run_transition` and
`run_window_glide`. Trap: do NOT move this flush onto the manager thread or into
a hook — it blocks ~one frame; it belongs on the worker only. (`animations.md`)

## 2026-06-26 — RESOLVED: window glide now shipped (snapshot-overlay)

Earlier the README over-claimed window glide while `animate_to` was instant. Now
real: `window_anim = glide` glides windows via the snapshot-overlay compositor
(`run_window_glide`), and the README is accurate. The naive per-frame approach
below is still banned — glide goes through the overlay, never the real window.

## 2026-06-26 — Do NOT interpolate SetWindowPos per frame

Naive per-window glide (move the real window a bit each frame) was implemented and
removed. Reasons (full detail in `animations.md` and `win32-reference.md`):

- Cross-process `SetWindowPos` every frame → DWM recomposite + target message-loop
  round-trip per frame, per window. Slow apps stall the tween.
- Apps clamp/veto/snap intermediate rects → jitter + wrong final rect on some apps.

`animate_to`'s name is historical; it's instant on purpose. Leave it instant.
Real glide must go through an overlay snapshot, not the real window.

## 2026-06-26 — PrintWindow returns black on GPU-accelerated apps

Capturing window pixels via `PrintWindow` can return a black frame for
hardware-accelerated surfaces (some Chrome configs, games, video). The workspace
slide tolerates this (falls back / wallpaper still captured). Any new per-window
snapshot feature must handle a black/failed capture gracefully, not show a black
rectangle.

## 2026-06-26 — Keep config.rs and layout.rs Win32-free

`config.rs` (parsing) and `layout.rs` (geometry) have **no** Win32 calls and must
stay that way — they're the only trivially-testable parts. Don't pull `windows`
crate types into them.

## 2026-06-26 — DeferWindowPos batching was rejected on purpose

Tempting "optimization": batch a retile's N `SetWindowPos` into one
`BeginDeferWindowPos`/`DeferWindowPos`/`EndDeferWindowPos` pass (fewer DWM
recomposites). **Don't.** `src/main.rs` (~line 700) documents the rejection: a
single defer batch can fail *wholesale* if one window misbehaves, leaving
everything un-tiled. Per-window restore-then-place (komorebi's approach) is the
robust choice and upholds the "never break window management" goal. Robustness >
the marginal reflow saving here. (`ideas.md` had this as a backlog item — it's a
NO, not a TODO.)

## 2026-06-26 — Hook procs are sacred

Anything added to `mouse_proc`/`keyboard_proc` is multiplied by the entire OS input
rate. No locks without an atomic guard, no allocation, no `SetWindowPos`. If you
must do work, push a `Cmd` and let the manager/worker handle it. Measure before and
after.

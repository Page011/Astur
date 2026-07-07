# Known issues / traps / code to avoid

Dated. Newest on top. "Don't use X because Y" goes here with the reason.

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

## 2026-07-08 — Launcher icons: switched to jumbo image list + DrawIconEx

SUPERSEDES the premultiply fix below. Icons are now HICONs from the system image list
at JUMBO (256px) via `SHGetFileInfo(SHGFI_SYSICONINDEX)` + `SHGetImageList(SHIL_JUMBO)`
→ `IImageList::GetIcon`, drawn with `DrawIconEx` (`DI_NORMAL`). DrawIconEx composites
the icon's own straight alpha correctly, so there's no manual premultiply and no halo,
and jumbo source = Start-Menu-quality downscale. UWP / `shell:AppsFolder` entries that
`SHGetFileInfo` can't resolve fall back to `IShellItemImageFactory` → HBITMAP →
`CreateIconIndirect` → HICON (this also fixes the "some icons don't load" cases). The
premultiply path below is gone. Traps: `SHGetImageList` is generic (`SHGetImageList::<
IImageList>(SHIL_JUMBO as i32)`); needs `Win32_UI_Controls`. Icons use STRAIGHT alpha
(do NOT premultiply for an HICON).

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

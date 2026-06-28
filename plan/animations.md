# Animations — design & honest state

Astur's headline differentiator vs komorebi/GlazeWM is motion polish. This file
is the source of truth for what animates, what doesn't, and why.

## Two very different problems

| Animation | Difficulty | Status |
|---|---|---|
| Workspace-switch transition | **Tractable** — we own a full-screen overlay and composite pixels we captured. | Shipped (slide). Extended here to off/slide/spring/fade. |
| Window open / close / move / resize | **Hard** — we'd be animating *another process's* real window position. | Instant placement. Naive glide was removed. Snapshot-glide proposed below. |

Understanding why these are different is the whole game. Astur renders **no
window pixels** — DWM does. We can only either (a) move the real window
(`SetWindowPos`, cross-process, expensive, lands unreliably mid-tween) or
(b) cover an area with our own overlay and blit pixels we captured (cheap, smooth,
GPU-composited, but cosmetic — it's a photo, not the live window).

The workspace overlay works because the whole monitor is being replaced at once,
so one rectangular photo covers everything. Per-window animation can't reuse one
big photo — each window needs its own.

---

## 1. Workspace-switch modes (off / slide / spring / fade)

Config key: `workspace_anim` in `astur.conf`. Back-compat: `workspace_slide = false`
maps to `off`; `true` (default) keeps `slide`.

| Mode | Motion | Easing / compose |
|---|---|---|
| `off` | none | Instant `switch_plain`, no overlay. |
| `slide` | old slides off one edge, new slides in from the other | horizontal offset, `ease_in_out_cubic`. Current behaviour. |
| `spring` | slide that overshoots the target then settles back | offset eased with `ease_out_back` (overshoot ~13%, `C1=1.40`). "Springs across then back to centre," like Hyprland. |
| `fade` | old fades out, new fades in, both in place | no offset; `AlphaBlend` the incoming over the outgoing with alpha ramping 0→255. |

### Implementation notes

- `SlideReq` carries a `mode` (or `WsAnim` enum). `run_transition` branches in the
  `compose(off)` closure:
  - slide/spring: existing BitBlt filmstrip; spring just changes how `off` is
    computed from `t` (different easing, can exceed the target then return).
  - fade: lay incoming static (offset 0), then `AlphaBlend` — or lay outgoing then
    alpha-blend incoming over it as alpha climbs. Whole-image constant alpha via
    `BLENDFUNCTION{ SourceConstantAlpha: a, AlphaFormat: 0 }`, `AC_SRC_OVER`.
    Needs `AlphaBlend` (Win32_Graphics_Gdi — already a Cargo feature).
- `spring` overshoot must be clamped so the incoming filmstrip slot is wide enough
  not to expose a seam at peak overshoot. Either widen the composed buffer by the
  overshoot margin, or cap overshoot at a value the existing `dir*w` slot covers
  (the incoming sits at `off + dir*w`; overshoot makes `off` pass `-dir*w`, i.e.
  the incoming passes centre toward the far edge — there IS spare filmstrip there,
  so a modest overshoot is safe without widening).
- Duration: `animation_ms` (floored to 200 for full-monitor pushes today). Fade can
  use the raw `animation_ms` — no steppiness concern, it's an alpha ramp.
- Easing lives next to `ease_in_out_cubic`. Add:
  - `ease_out_back(t)` = `1 + c3*(t-1)^3 + c1*(t-1)^2`, `c3=c1+1`.
    `c1=1.70158` is the classic; **shipped value is `c1=1.40`** (~13% overshoot).
    1.10 was too timid to read as a spring; 1.40 is a confident spring without
    looking cartoonish. The curve lands with zero velocity at t=1, so the settle
    is inherently soft — no extra tail smoothing needed (a smoothstep blend was
    tried and rejected: it cancels the overshoot). Must land EXACTLY on target at
    t=1 or the final frame misaligns with the real windows and the reveal pops.

### Start flash — frame 0 must be the exact screen capture (2026-06-26)

Separate from the teardown flash below. The overlay's frame 0 is painted before
`ShowWindow`, so raising it should be invisible — but only if frame 0 is
pixel-identical to the live screen. The `compose` closures rebuild frame 0 from
the **PrintWindow wallpaper capture + window rects**. If that wallpaper differs
from the live DWM-composited desktop even slightly (acrylic/transparency,
sub-pixel crop), the gaps pop the instant the overlay is raised — the "flash
before the slide." Fix: for frame 0 only, blit the **exact `capture_monitor`
grab** (`out_bmp`) straight through instead of calling `compose(0)`. Guaranteed
match. The wallpaper-composited path is still used for every moving frame
(`off != 0`), where a sub-pixel gap diff is invisible under motion. Applied to
both `run_transition` and `run_window_glide`.

### Start flash — overlay must be composited before the switch (2026-06-26)

Even with a pixel-exact frame 0, `ShowWindow(SW_SHOWNA)` returns before DWM has
composited the overlay onto the screen. The worker signaled the manager
immediately, so `switch_plain` ran underneath while the overlay wasn't yet
visible — the destination workspace flashed for a frame. Fix: `DwmFlush()` after
`ShowWindow` and BEFORE `signal_slide_overlay_up()` (and the glide equivalent).
The flush blocks until the overlay is genuinely on screen, so the real switch
underneath is always hidden. Three flushes now bracket a transition: overlay-up
(before signal), [animation frames], teardown (before `DestroyWindow`).

### Reveal flash — synced teardown (2026-06-26)

The overlay's last frame is target-aligned with the real windows placed
underneath, so teardown *should* be seamless — but `DestroyWindow` returning
off-vblank could expose a frame before DWM had recomposited the live windows.
Visible as a flash where the (stale) snapshot vanishes a beat before the live
window paints — the "changes to a snapshot instead of the window" flash. Fix:
call `DwmFlush()` immediately before `DestroyWindow` in **both** `run_transition`
and `run_window_glide`. It blocks until the next composition pass, so the
overlay's final pixels and the live windows hand off on the same vblank. Cheap
(once per switch, on the worker thread — never the manager or a hook).

### First-visit flash — always cover the switch (2026-06-26)

The start/reveal flush fixes above assume an overlay is up. But the FIRST time you
enter a workspace there is no cached incoming snapshot (`snap_get` returns None),
so the old code skipped the overlay entirely and ran `switch_plain` bare. Showing a
window with `SW_SHOW` after `SW_HIDE` flashes its background through until the app
repaints — DWM discards a hidden window's composited surface, so the freshly-shown
window has nothing to draw for a frame or two. With no overlay covering it, that
pop is fully visible. This is the dominant "background flashes through the windows"
jank early in a session, when most switches go somewhere new.

Fix: `switch_monitor_workspace` now dispatches the overlay whenever it has an
outgoing capture (`out != 0`), passing `in_bmp = 0` when there's no snapshot.
`run_transition` treats `in_bmp == 0` as **cover-hold**: it raises the overlay
showing frame 0 (the exact live screen), signals the manager to switch underneath,
then simply HOLDS that frame for `COVER_HOLD_MS` (48ms) — long enough for the
destination's first paint to land beneath the cover — and reveals on the synced
teardown flush. Deliberately no recompose during the hold: compositing the
window-less incoming would slide the outgoing off to bare wallpaper. After the
first visit, `old`'s snapshot is cached as before, so repeat visits animate
normally (slide/spring/fade), whose ≥200ms motion already hides the paint-in.

Why not DWM cloaking? `DWMWA_CLOAK` would keep a window's surface alive across a
hide (no repaint, no flash), but `DwmSetWindowAttribute(DWMWA_CLOAK)` only cloaks
windows owned by the **calling** process — the cloaked-state values are "by owner
app" or "by Shell," with no cross-process path (docs + community confirmed). A WM
hides other processes' windows, so cloak is a dead end here; `SW_HIDE` + cover-hold
is the cross-process-safe answer. (`known-issues.md`)

### Present order — show BEFORE blitting frame 0 (2026-06-27)

The actual cause of the reported "windows flash hidden (wallpaper) then reappear and
slide." Both compositors did: build frame 0 in the back buffer → `BitBlt` it to the
overlay's window DC → `ShowWindow`. **A blit to a still-hidden window's DC is clipped
to its empty visible region and lost.** So the overlay was raised EMPTY; DWM showed
the wallpaper underneath; the manager (signalled "overlay up") hid the outgoing
windows into that gap; only the animation loop's first frame, a few ms later, finally
painted the overlay — reading as the windows vanishing to wallpaper then the snapshot
popping back to slide. Fix (both `run_transition` and `run_window_glide`):

```
BitBlt(backdc, frame0)          // build frame 0
ShowWindow(overlay, SW_SHOWNA)  // SHOW FIRST so the DC has a visible region
BitBlt(odc, backdc)             // THEN present — now it lands on the live window
UpdateWindow(overlay)           // settle any pending paint onto our pixels
DwmFlush()                      // block until frame 0 is on the glass
signal_*_overlay_up()           // only now let the manager switch underneath
```

Rule: **never paint an overlay's DC before `ShowWindow`.** Show, then present. This
is the fix that actually removes the flash; the cover-hold (below) is complementary
(it covers the first-visit no-snapshot case) but was not sufficient alone.

### Future workspace modes (backlog)

- `push` (both monitors' content rigid, like current slide but no wallpaper-gap
  parallax) vs `cover` (new slides over a static old). Cheap variants of slide.
- Directional fade (fade + small slide) — Hyprland's "fade" actually drifts.

---

## 2. Window open / close / move / resize — SHIPPED (snapshot-glide)

Implemented 2026-06-26 as the **glide compositor** (`window_anim = off | glide`,
default `glide`). It is the snapshot-overlay design (Option A below), NOT
per-frame real-window movement.

### How it works (`retile_monitor` + `run_window_glide`)

1. A layout change calls `retile_monitor`. It computes each window's old rect
   (live `GetWindowRect`) and new tile slot. If nothing moved (>2px), it bails to
   instant — a no-op retile (e.g. refocus) never raises an overlay.
2. Freeze the work area to one bitmap (`capture_monitor`), `dispatch_glide`.
3. The `glide_worker` raises a topmost overlay, paints frame 0 (every window at
   its old rect over a captured wallpaper backdrop == current screen, no flash),
   and signals back.
4. The manager places the REAL windows at their targets instantly, underneath the
   overlay (hidden).
5. The worker glides each window's frozen image old→new (`StretchBlt`, so resizes
   scale), eased with `ease_out_cubic`, over the still wallpaper. At t=1 the image
   is pixel-aligned with the real window, so teardown is a seamless reveal.

Free wins: **opening** a window glides it from where it spawned into the slot;
**closing** reflows the rest as they glide to fill. No special open/close code.

Degrades safely: wallpaper capture fails → instant (no overlay); a glide already
running (`GLIDE_BUSY`) → instant; capture fails → instant. Never blocks window
management.

### Known limitations / backlog

- One overlay/worker slot: a simultaneous multi-monitor retile glides only one
  monitor; the others place instantly. Fine for the common single-monitor case.
- Uses `ease_out_cubic` (settle, no overshoot). A `spring` window option
  (`ease_out_back`) is a trivial follow-up now the foundation exists.
- Brand-new window's frozen image is its spawn-position pixels (not final), which
  is correct for an "open" glide.

The original analysis (why the naive path was wrong, and the two options) is kept
below for context.

### Why naive glide was removed (don't redo it)

The original approach interpolated `SetWindowPos` over time per window. Removed
because:

1. **Lands unreliably across apps.** Many apps debounce/snap/clamp `WM_WINDOWPOSCHANGING`
   or have min-size quirks, so mid-tween frames jitter and the final rect is wrong
   for some apps.
2. **Per-frame cross-process cost.** Each frame is a `SetWindowPos` round-trip into
   the target process's message loop + a DWM recomposite. N windows × 60 fps =
   a lot of cross-process traffic; slow apps stall the whole tween.
3. Tearing/lag is worst exactly on the apps users care about (browsers, editors).

So: **real-window interpolation is a dead end.** Two viable directions instead.

### Option A — Snapshot-glide overlay (recommended, heavier)

Mirror the workspace-overlay trick per window:

1. On a move/retile, capture the window's current pixels to an HBITMAP.
2. `SetWindowPos` the **real** window to the target instantly (as today) but keep
   it hidden/below, OR keep it visible and lay the snapshot overlay over the
   travel path.
3. A small topmost layered overlay blits the snapshot gliding from old rect →
   target rect (with scale for resize), eased.
4. On finish, destroy the overlay; the real window is already correctly placed.

Pros: smooth, GPU-composited, no per-frame cross-process call, lands reliably
(real window is placed once, instantly). Cons: real work — capture cost on open,
layered-window scaling for resize, must handle the window changing content under
the snapshot (acceptable for a ~140 ms glide), z-order/focus care, multi-window
retiles need batching. This is the Hyprland-grade path.

Open/close: open = snapshot from target, blit scaling-up + fading-in into place,
then reveal. Close = grab last snapshot before destroy, blit scaling-down/fading
out (Windows often destroys the window before we can — capture must be eager).

### Option B — DWM-only, no real animation

Accept instant placement; lean entirely on the workspace overlay for "wow."
Lowest risk, zero per-window cost. This is the current state. Fix the README to
match and call window-glide a non-goal until Option A is built.

### Recommendation

Ship **§1 (workspace modes)** now — clean, bounded, high visible payoff. Treat
**Option A** as a designed, separate piece of work (its own branch): it's the
right way to get real window glide, but it is not a small change and must not
regress the "never break window management" goal. Until then, **fix the README**
(Option B honesty).

### Spring for windows (later)

Once Option A exists, the same `ease_out_back` gives windows a spring settle. That
is the closest match to "the ACTUAL Hyprland" the user asked for — but it depends
on the snapshot-glide foundation, not on real-window interpolation.

---

## Easing reference

- `ease_in_out_cubic` — symmetric, no overshoot. Default for slide. (In code now.)
- `ease_out_back` — overshoot then settle. Spring. (To add.)
- `ease_out_cubic` — fast then slow, no overshoot. Good for fade alpha ramp.
- Linear — only for constant-velocity needs; reads "mechanical," avoid for motion.

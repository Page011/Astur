# Optimization plan (Phase 4 — NEXT)

Concrete, prioritized pass requested by the user ("optimise the hell out of
everything"). Targets chosen by the user: **input latency, animation smoothness,
binary size + memory, startup, general RAM** (i.e. all of them).

Rules that override any optimization here (from `CLAUDE.md` / `AGENTS.md`):
- **Never break window management.** A faster switch that drops a window is a
  regression, not an optimization.
- **Hooks are sacred.** No new locks/allocs on `mouse_proc`/`keyboard_proc`.
- **Measure before/after.** Don't land a micro-opt on faith; note the number in
  this file. Where a change can't be measured cheaply, keep it and say so.

Each item: location, the change, expected win, risk. Do them roughly top-down
(highest confidence / lowest risk first), build + verify after each.

## A. Input latency (hooks)

- **DONE 2026-07-07 — drag `position_worker` now uses `SWP_ASYNCWINDOWPOS`.** The
  worker's cross-process `SetWindowPos` was synchronous, so dragging a window that
  belongs to a busy app (browser/Electron) stalled the follow until that app ACKed
  each move. Async posts the request and returns immediately → smooth cursor-follow
  on heavy apps; the final rect is re-applied synchronously on drop
  (`Cmd::DragMoved`/`DragResized`) so nothing is lost. (`win32-reference.md`)


- **DONE 2026-06-27 — `PRESSED: Mutex<[bool; 256]>` → lockless `[AtomicBool; 256]`.**
  `keyboard_proc` no longer takes a Mutex on key-up or Alt-hotkey down; the
  repeat-guard is `swap(true)`/`store(false)`. Lock removed from the OS-wide key path.
- Audit the launcher capture path in `keyboard_proc`: it `PostMessageW`s per key
  while open — fine (only while the picker is up). No change unless measured hot.
- Confirm the Phase-2 click-outside block stays one atomic load when the launcher
  is closed (it does — `LAUNCHER_OPEN` short-circuits). Keep it that way.

## B. Animation smoothness

- **Tune `COVER_HOLD_MS` (Phase 1) against real apps.** 48ms is a guess; once the
  user confirms the first-visit flash is gone, try lowering toward ~32ms (snappier)
  or raising if a slow app still flashes. Note the landed value here.
- **Optional reveal nudge:** if any app still flashes on the slide-path reveal,
  add a best-effort `RedrawWindow(RDW_INVALIDATE|RDW_UPDATENOW)` over the active
  workspace's windows right after `switch_plain` (under the overlay). Cross-process
  RDW isn't synchronous, so it only *accelerates* paint-in; keep the cover/slide
  duration as the real guarantee. Only add if observed.
- Frame compositing in `run_transition`/`run_window_glide` re-blits the full
  wallpaper + every window rect each frame. Region-clipping to the moving band is a
  possible saving but adds complexity — low priority, only if profiling shows the
  blit is the bottleneck on large/multi-monitor setups.

## C. Binary size + memory

- **Audit `Cargo.toml` `windows` features** — drop any not actually referenced
  (smaller metadata + link). Phase 2 added `Win32_System_Com` +
  `Win32_UI_Shell_Common`; confirm both are needed (they are: COM init + ITEMIDLIST
  types) and that older features are all still used. Win: smaller `.exe`, faster
  link. Risk: low (compiler catches a wrongly-dropped feature).
- **DONE 2026-06-27 — `switch_plain` no longer clones the window Vec.** Now iterates
  the old/new workspace window lists by index (manager owns the data on its thread;
  ShowWindow touches no Astur state). Drops 2 allocations per workspace switch.
- Reuse scratch `Vec<u16>` buffers in `paint_bar`/`update_bar` widgets instead of
  allocating per widget per refresh (bar ticks ~1s, so low value — do only if the
  size audit leaves time).
- Try `opt-level = "s"` vs `3` (currently 3) and record the size/Δspeed tradeoff.
  Keep `3` unless `s` is clearly smaller with no felt regression. LTO/strip/
  `panic=abort`/`codegen-units=1` are already set — leave them.

## D. Startup + responsiveness

- Launcher enumeration already moved off the first-open path (Phase 2). Good.
- Lazily build the slide/glide/launcher GDI resources (fonts, brushes) — already
  largely lazy; confirm nothing heavy runs before the WM is interactive.
- Consider deferring `icon_worker`/`filesearch_worker` thread spawn until the first
  launcher open (saves two idle threads at boot). Minor; only if RAM audit wants it.

## E. General RAM

- Snapshots are GPU-backed DDBs (`dup_ddb`) — already ~no process RAM. Keep.
- Launcher icon HBITMAPs accumulate over a session (bounded by app count, 24×24×4 ≈
  4KB each). If this matters, evict icons for apps not shown recently, or cap the
  cache. Low priority (a few hundred KB worst case).
- File search (Phase 3) must hold only the top-N current results, never a
  persistent file table — that's the whole reason Option A (OS index) was chosen.

## Verification

Build each change with the GNU toolchain (MSVC linker isn't on PATH here):
`CARGO_HTTP_CHECK_REVOKE=false cargo +stable-x86_64-pc-windows-gnu build --target x86_64-pc-windows-gnu`.
Record `.exe` size before/after for size items; for latency items, reason from the
removed lock/alloc (a hook micro-bench is hard to set up — note that). Never ship a
change that can lose/misplace a window.

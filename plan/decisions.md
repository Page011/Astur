# Decisions log (mini-ADR)

One line each. Newest on top. Link detail elsewhere.

- 2026-07-07 — Full: **outline drag** replaces live per-frame `SetWindowPos` for
  Alt-move/resize. Foreign-window live resize stalls on the app's own repaint (Astur
  was 0.4% CPU — not the bottleneck; a Windows WM can't own another app's surface
  like Mac/Wayland). Hook draws a region-shaped outline overlay and commits once on
  release (`commit_rect`); removed `position_worker`/`set_target`. Also: hawk **logo**
  → `astur.ico` embedded in the exe (build.rs + `embed-resource`) + hawk tray PNG +
  installer/setup icon; launcher icon premultiply (white-halo) fix. Inno per-user
  installer at `packaging/astur.iss`. (`architecture.md`, `known-issues.md`)

- 2026-07-07 — Bug fixes + drag smoothness on `main` (Full): (1) **phantom Shift**
  fixed — capture-mode `is_mod` matched generic `VK_SHIFT` but the LL hook delivers
  `VK_LSHIFT`/`VK_RSHIFT`, so Shift key-up was swallowed while a menu was open →
  `GetAsyncKeyState` stuck; new `is_modifier_vk` covers L/R specifics. (2) **sysmenu
  Esc** steps back a level (posts `SM_BACK`) instead of closing outright. (3) drag
  `position_worker` uses `SWP_ASYNCWINDOWPOS` so a busy foreign app can't stall the
  move/resize follow. (`known-issues.md`, `win32-reference.md`, `system-menu.md`)

- 2026-07-07 — **Astur Lite v1.0.1 SHIPPED** (tag `v1.0.1` on `lite`). First
  maintained Lite release: the three v2 efficiency/quality backports — lockless
  `PRESSED` (`[AtomicBool;256]` off the keyboard hot path), `switch_plain` index
  iteration (no per-switch Vec clone), and the 2nd-gen workspace-switch flash fix
  (frame 0 = exact `out_bmp` capture, not `compose(0)`; show overlay before
  blitting frame 0; `UpdateWindow`+`DwmFlush`). No features — Lite's remit.
  (`editions.md`)
- 2026-06-28 — Astur Full: tray icon + no-console SHIPPED. Release builds drop the
  console (`cfg_attr(not(debug_assertions), windows_subsystem="windows")`); a
  `Shell_NotifyIcon` tray is the control surface (Settings launches the sibling
  `astur-settings.exe`; Quit restores windows + exits). Placeholder icon for now.
  (`editions.md`)
- 2026-06-28 — Editions split: Lite changed from frozen tag → **maintained `lite`
  branch** (created at `cc7e441`/v1.0.0). Lite = minimal console exe, core-only
  efficiency/quality updates; `main` = full app (tray + GUI + launcher + search +
  installer). Shared-core fixes backport manually between branches. Matrix +
  messaging in `plan/editions.md`. (supersedes the 2026-06-27 "frozen v1.0.0" pick)
- 2026-06-28 — Repo restructured into a **Cargo workspace** for v2: `crates/astur`
  (WM, moved from `src/`), `crates/astur-config` (config extracted to a shared lib,
  `pub`, Win32-free — the GUI will parse the same model; aliased `config` in the WM),
  `crates/astur-settings` (egui GUI, stub). Astur Lite = the frozen `v1.0.0` tag (no
  fork). Release workflow builds `-p astur`. (`roadmap-v2.md`, CLAUDE.md, AGENTS.md)
- 2026-06-28 — v2 base committed (`5c530ab`); never add a Claude co-author trailer to
  commits (user preference, saved to memory).
- 2026-06-27 — File search sped up ~8×: `LIKE '%q%'` (914ms, full index scan) →
  `CONTAINS(System.FileName, '"q*"')` (108ms, full-text index) + debounce 120→45ms.
  Word-prefix not substring; true Everything-speed needs the MFT index (v2).
  (`known-issues.md`, `roadmap-v2.md`)
- 2026-06-27 — v2 direction DISCUSSED (`roadmap-v2.md`): GUI config = YES but a
  separate companion app (egui, not iced) editing the same conf; installer = YES as an
  option, keep the portable exe; DON'T fork v1/v2 — one core + optional companions;
  "instant like Everything" = a feature-gated MFT/USN in-RAM index (admin + RAM).
- 2026-06-27 — Phase 3 file search SHIPPED (`launcher.md`): OLE DB `Search.CollatorDSO`
  on a debounced/cancellable `filesearch_worker`; results merged into the picker
  (`Hit::App|File`); Tab detail footer (path/modified/size); Enter opens, Shift+Enter
  reveals in Explorer. OLE DB consumer verified in a scratchpad probe before porting.
- 2026-06-27 — System/power menu SHIPPED v1 (`plan/system-menu.md`): Alt+Shift+Space
  popup — Lock/Sleep/Sign out/Restart/Shut down (confirm-gated)/Open config. Reuses the
  launcher scaffolding; `ExitWindowsEx` with lazy `SeShutdownPrivilege`. Wallpaper
  submenu + click-outside are backlog.
- 2026-06-27 — File search "no results" bug fixed: Windows Search SQL has no
  `LIKE … ESCAPE` (errored every query, 0x80040E14). Dropped ESCAPE; `sanitize_like`
  now only doubles `'`. (`known-issues.md`, found via probe.)
- 2026-06-27 — Workspace/glide flash ROOT CAUSE found + fixed: frame 0 was blitted
  to the overlay's window DC while it was still HIDDEN (clipped → lost), so the
  overlay came up empty and the wallpaper flashed through until the loop's first
  frame. Now: ShowWindow → present frame 0 → UpdateWindow → DwmFlush → signal, in
  both `run_transition` and `run_window_glide`. Supersedes the 2026-06-26 cover-hold
  attempt (which didn't address this — it relied on the same lost blit).
  (`animations.md`, `known-issues.md`)
- 2026-06-27 — Launcher v2.1: icons load in parallel (3 COM workers) + preloaded
  for the whole list at startup (fixes "slow / not all icons load"); visual polish
  — rounded window corners (DWM), rounded accent selection pill, thinner frame,
  bigger icons/rows, query caret. (`launcher.md`)
- 2026-06-27 — Phase 3 file search DE-RISKED: backend = OLE DB `Search.CollatorDSO`
  (verified rich+fast via ADO on this box). `search-ms:` shell-enum returns 0 —
  rejected. Next: port the OLE DB query into a probe, then a `filesearch_worker`.
  Phase 4 quick wins landed: lockless `PRESSED` (no Mutex on the key hot path),
  `switch_plain` no longer clones the window Vec per switch. (`launcher.md`,
  `optimization.md`, `known-issues.md`)
- 2026-06-27 — Modding architecture designed (NOT built): declarative mods now +
  out-of-process IPC mods later, so code mods can't break window management.
  (`plan/mods.md`)
- 2026-06-26 — Launcher v2 SHIPPED (`plan/launcher.md`): shell `AppsFolder`
  enumeration adds UWP/system apps (Notepad, Calculator) the `.lnk` walk missed;
  per-row shell icons resolved off the UI thread (`icon_worker`, `AlphaBlend`);
  click-outside-to-dismiss via the global mouse hook + published picker bounds.
  Launch unified through `ShellExecuteW` (lnk path / `shell:AppsFolder\<aumid>` /
  raw exe path). COM enumeration + icon verified standalone on a Win11 box.
- 2026-06-26 — Workspace-switch FIRST-visit flash fixed: the switch now always
  raises the overlay even with no cached snapshot (`in_bmp == 0`), holding the
  outgoing frame for `COVER_HOLD_MS` (48ms) so the destination's first paint lands
  under cover before reveal. Kills the "background flashes through the windows" pop
  on the first visit to each workspace. Cross-process `DWMWA_CLOAK` was rejected as
  the fix (only cloaks your own process's windows — verified via docs).
  (`animations.md`, `known-issues.md`)
- 2026-06-26 — App launcher v1 SHIPPED (`plan/launcher.md`): Alt+Space (NOT
  Win+Space — avoids clobbering the system layout toggle), Start Menu `.lnk`/`.url`
  source, hook-driven picker window (no foreground focus), fuzzy match, ShellExecute
  launch. File search (Windows Search index) remains a documented later phase.
- 2026-06-26 — Focus-follows-mouse now held off for `FOLLOW_SETTLE_GUARD_MS`
  (200ms) after any programmatic focus/switch (`bump_follow_settle`). Fixes the
  regression where the 16ms hover poll yanked focus back after a keyboard
  workspace switch. (`architecture.md`, `known-issues.md`)
- 2026-06-26 — Overlay-up synced to vblank with `DwmFlush()` before signaling the
  manager, so the real switch underneath can't flash the destination through a
  not-yet-composited overlay. Mirror of the teardown flush. (`animations.md`)
- 2026-06-26 — Overlay frame 0 now blits the exact `capture_monitor` grab (not
  the wallpaper-composited `compose(0)`) so raising the overlay can't pop — kills
  the "flash before the slide." Both compositors. (`animations.md`, `known-issues.md`)
- 2026-06-26 — Overlay teardown synced to vblank with `DwmFlush()` before
  `DestroyWindow` (both `run_transition` + `run_window_glide`) to kill the
  workspace-switch reveal flash. (`animations.md`, `known-issues.md`)
- 2026-06-26 — Spring overshoot strengthened: `ease_out_back` `C1` 1.10 → 1.40
  (~13% overshoot). Back-ease lands with zero velocity at t=1, so settle stays
  soft and final frame stays target-exact. (`animations.md`)
- 2026-06-26 — Focus-follows-mouse poll 80ms → 16ms for snappy hover; gated on
  cursor-moved so idle cost is one `GetCursorPos`/tick (no `WindowFromPoint`/lock
  when still). (`architecture.md`)
- 2026-06-26 — Window glide SHIPPED (`window_anim = glide`) via the snapshot-overlay
  compositor (`run_window_glide`), never per-frame real `SetWindowPos`. Degrades to
  instant on capture failure / busy / no-op. (`animations.md`, `known-issues.md`)
- 2026-06-26 — Workspace transition gets selectable modes `off|slide|spring|fade`
  via `workspace_anim`; `workspace_slide` kept as back-compat alias. (`animations.md`)
- 2026-06-26 — Ship workspace modes now; per-window snapshot-glide is a separate,
  later branch. README to be corrected to not over-claim window animations.
- (pre-existing) — Single big `main.rs` by design: one translation unit, fast
  build, no module ceremony. Pure data/math split into `config.rs`/`layout.rs`.
- (pre-existing) — Cosmetic animation runs over an already-correct instant switch;
  never let it own real window state.

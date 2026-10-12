# AGENTS.md — Astur

Guidance for AI agents (and humans) working on this repo. Read this first, then
the relevant file in `plan/`.

## What Astur is

A fast, free, portable **tiling window manager for Windows 10/11**, shipped as a
single Rust `.exe` with no installer and no required config. Core hooks:

- **Alt + left-drag** — move any window from anywhere on it (no title bar needed).
- **Alt + right-drag** — resize from the nearest corner (red bracket marker).
- **Tiling** — `dwindle` (spiral, default), `master`, `columns`, `grid`,
  `monocle`, or Niri-style `scrolling` (column strip, off-screen columns
  parked outside the virtual screen, tabbed columns; `plan/scrolling.md`).
  Layout is per monitor (`monitor_layouts`).
- **Up to 10 virtual workspaces**, shared or per-monitor.
- **Per-monitor status bar** (workspace pills, title, clock, CPU/RAM/battery).
- **Workspace-switch animation** — overlay compositor: off/slide/spring/fade.
- **Window glide** — windows animate to their tile slot on open/move/resize/
  re-tile/scroll via an overlay of live DWM thumbnails (off/glide).
- **App launcher** — Alt+Space opens a fuzzy app picker (Start Menu shortcuts),
  hook-driven with no foreground focus. See `plan/launcher.md`.
- Coloured DWM borders, unfocused dimming, focus-follows-mouse, per-app rules,
  live config hot-reload.

Left Alt is fully reserved as the Astur modifier (apps never see it). Right Alt is
untouched. Alt+Tab still works via a synthetic Alt.

## Project goals (the bar to hold)

1. **Zero-setup.** Run the exe, Alt-drag works immediately. No config to start.
2. **Tiny + fast.** Single portable exe. The global mouse hook is the hottest path
   in the whole OS input stream — it must never block, allocate, or lock
   needlessly. Latency over features.
3. **Never break window management.** Cosmetics (animations, bar) are layered
   *over* a switch/tile that already happened correctly and instantly. A dropped
   or failed animation must never lose or misplace a window.
4. **Smooth.** Beat komorebi/GlazeWM/Seelen on motion polish — that is the
   headline differentiator (see `README.md` comparison table).
5. **Honest docs.** If a feature is staged or removed, the README must not claim
   it. (See `plan/known-issues.md`.)

## Build / run / quit

Cargo workspace (v2). Astur Lite = the frozen `v1.0.0` tag; this tree is the full
"Astur". See `plan/roadmap-v2.md`.

```bash
cargo build --release -p astur # target/release/astur.exe  (LTO, strip, panic=abort)
cargo build                    # fast debug build of the whole workspace
cargo run -p astur             # run the WM from source
```

Quit: tray Quit or `Ctrl+C` in the console (runs `restore_all_windows`: shows
hidden windows and unparks parked ones), or kill the process (the next launch's
crash rescue does the same from `~/.astur/rescue.lst`).

Config lives in `%USERPROFILE%\.astur\astur.conf` + `navbar.conf`, both created
on first run, fully commented, **hot-reloaded on save**.

## File map (workspace)

| File | Lines (approx) | What |
|---|---|---|
| `crates/astur/src/main.rs` | ~23800 | Everything Win32: hooks, worker threads, manager, scrolling strip model, bar, tab bars, compositors, launcher, file search, system menu. One big file by preference (see CLAUDE.md; splitting would not change build time). |
| `crates/astur/src/layout.rs` | ~1300 | Pure tiling geometry (dwindle, master, columns, grid, monocle, scrolling strip: spans, offset clamp, peek/park `classify`, `park_spot`) + tests. `RECT` type only, no Win32 calls. |
| `crates/astur/src/yasb.rs` | ~800 | GlazeWM-compatible workspace WebSocket API for YASB (loopback only, off by default). |
| `crates/astur-config/src/lib.rs` | ~2900 | `Config` struct, `LAYOUTS` (the one list of layout names), documented-default file templates, key/value parser. **No Win32** — pure data, shared with the settings GUI (aliased `config` in the WM). |
| `crates/astur-settings/` | ~1400 | Settings GUI (egui), shipped. Separate process — a GUI crash can't touch the WM. |
| `Cargo.toml` (root) | — | `[workspace]` manifest + release profile. WM deps live in `crates/astur/Cargo.toml`. |

`main.rs` is large but sectioned with `// ====` banners. Use the function map
(`grep -n "^fn \|^unsafe extern\|^struct \|^static " crates/astur/src/main.rs`).

## Threads (mental model)

The process is a set of single-purpose threads talking over `Mutex`/`Condvar`
queues. Keep it that way — do not move work onto the hooks (main thread).

- **Main thread** — installs and runs the hooks: `mouse_proc` / `keyboard_proc`
  (`WH_MOUSE_LL` / `WH_KEYBOARD_LL`) and `win_event_proc` (out-of-context
  WinEvents). Hooks must return fast: push a `Cmd`, do no work, no lock without
  an atomic guard (`ANY_DRAG`/`ALT_DOWN`/`SCROLLMON_N` let them early-out).
  Also owns the bar windows, the tray and the scrolling layout's tab-bar
  windows (`astur_tabs`, synced from the manager's `TAB_SNAPSHOT` by
  `tab_bars_sync`; clicks push `Cmd::TabClick`, nothing places windows).
- **Manager loop** (`manager_loop` / `Manager`) — owns all window/workspace/
  strip state. Drains the command queue (`CMDQ`). All `SetWindowPos`/tiling/
  switching/parking here, live Alt-drag placement included (`place_drag_frame`).
- **Transition worker** (`transition_worker` / `run_transition`) — owns the
  workspace-switch overlay, pumps its own message loop, GDI-composites frames
  (fade: DWM blends a second, layered overlay instead).
- **Motion worker** (`glide_worker` / `run_window_glide`, thread "glide") —
  window motion overlay (move/open/close/reflow/scroll): live DWM thumbnails
  of already-placed windows over the wallpaper, re-aimed in place while it
  runs (`retarget_glide`). Owns its overlay, pump and every thumbnail.
- **Wallpaper capture** (`wallpaper_capture_worker`) — renders the wallpaper
  backdrop for both compositors (PrintWindow of the wallpaper window only).
- **Cold-snapshot worker** (`cold_worker`) — PrintWindows cold parked tiles
  (blank DWM surface: hidden, minimised, or never on screen since shown) into
  the `COLD` cache so the motion can draw them. Besides the wallpaper capture,
  the only thread that PrintWindows (never the manager, a compositor or a
  hook). A hung app may block it; nothing waits on it.
- **Stats worker** (`stats_worker`) — polls CPU/RAM/battery ~2s for the bar.
- **Disk writers** (`state_worker`, `mru_worker`, `rescue_worker`) — the
  manager only fills a latest-wins slot; the disk I/O (state, launcher MRU,
  `rescue.lst`) runs here, so a slow disk never delays a command.
- **Focus-follow worker**, **config watcher**, **launcher**, **sysmenu**,
  **icon/file-search workers**, **IPC**, **YASB**, **hook watchdog** — as named.

## Conventions

- **No emojis in code.** Monochrome only, UI surfaces only.
- Win32 calls are `unsafe`; keep `unsafe` blocks tight and comment the invariant.
- Hot paths: no allocation, no `Mutex` lock unless an atomic guard says there's
  work. Comment *why* a fast-path exists so nobody "simplifies" it away.
- Colours are `COLORREF` (`0x00BBGGRR`) internally; config parses `#RRGGBB`.
- Cosmetic threads get bitmap/rect data by **single-owner hand-off, never
  shared** — a handle the owner gives up, moved rather than copied (the
  incoming workspace snapshot is taken out of the cache; the outgoing capture
  goes to the transition worker and is handed back as the snapshot; a cold
  window's snapshot is taken out of its cache into the motion request and
  freed by the glide worker). Exactly
  one thread owns, and frees, each bitmap at any moment; the owned `Bmp` type
  (no `Clone`) enforces it for snapshots. Deselect, then `GdiFlush`, before
  every hand-off.
- Prefer the simplest built-in Win32 path first; document alternatives + tradeoffs
  in `plan/` before adding complexity.

## Where to go next

- `plan/architecture.md` — deeper component + data-flow notes.
- `plan/scrolling.md` — scrolling layout spec + contract: keys, config,
  geometry, peek/park, safety, manual test checklist.
- `plan/animations.md` — animation design: desktop-switch modes (off/slide/spring/
  fade) and the honest analysis of window open/close/move/resize animation.
- `plan/win32-reference.md` — APIs used, doc links, gotchas (slow/buggy calls).
- `plan/competitors.md` — komorebi, GlazeWM, Seelen UI: what they do, links.
- `plan/known-issues.md` — known traps, doc/reality mismatches, code to avoid.
- `plan/launcher.md` — Alt+Space picker: v1/v2 shipped + Phase 3 file-search plan.
- `plan/optimization.md` — Phase 4 optimization pass plan (in progress backlog).
- `plan/mods.md` — modding/extensibility architecture (declarative + IPC mods + security).
- `plan/system-menu.md` — Alt+Shift+Space power menu (shipped) + backlog.
- `plan/roadmap-v2.md` — v2 strategy: GUI config, installer, packaging, MFT search.
- `plan/ideas.md` — backlog / discussion.

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
- **Up to 20 virtual workspaces** (`MAX_WORKSPACES`), shared or per-monitor.
- **Per-monitor status bar** (workspace pills, title, clock, CPU/RAM/battery,
  keyboard language, disk, uptime, custom command text, music buttons, logo;
  palettes, strip/floating/islands/pills shapes, anti-aliased shapes on a
  cached DIB, Segoe Fluent / MDL2 glyph icons, per-widget pill colours,
  powerline segments, hover highlight; per-widget left/right/middle click
  actions from astur-config `BAR_ACTIONS` run by the manager
  (`Cmd::BarAction` -> `run_bar_action`, sharing `run_action` with extra
  hotkeys); `monitors` picks the screens; 9 presets as data in astur-config
  `BAR_PRESETS`, stamped by Settings).
- **Workspace-switch animation** — overlay compositor: off/slide/spring/fade.
- **Window glide** — windows animate to their tile slot on open/move/resize/
  re-tile/scroll via an overlay of live DWM thumbnails (off/glide).
- **App launcher** — Alt+Space opens a fuzzy app picker (Start Menu shortcuts),
  hook-driven with no foreground focus. See `plan/launcher.md`.
- Coloured DWM borders, unfocused dimming, focus-follows-mouse, per-app rules,
  live config hot-reload.

The Astur modifier is the **main key** (`main_key`, config `MainKey`): Left Alt by
default, fully reserved (apps never see it; Left Alt + an unbound
typing/editing/navigation key is swallowed, never passed bare — see
`swallow_unbound`; Right Alt is untouched; Alt+Tab still works via a synthetic
Alt). Caps Lock can be reserved the same way. Win, Alt or Ctrl+Win are passed
through instead: only bound keys are swallowed, plus the 0xE8 mask key so Start /
menu bars don't open (`needs_mask`). Every hotkey is one load from the lock-free
`BINDS` table, filled from astur-config's pure `compile_keys`.

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
hidden windows and unparks parked ones; an exit watchdog ends the process 2 s
after the restore starts, so a hung app cannot keep it alive), or kill the
process (the next launch's crash rescue does the same from
`~/.astur/rescue.lst`).

Config lives in `%USERPROFILE%\.astur\astur.conf` + `navbar.conf`, both created
on first run, fully commented, **hot-reloaded on save**.

## File map (workspace)

| File | Lines (approx) | What |
|---|---|---|
| `crates/astur/src/main.rs` | ~28700 | Everything Win32: hooks, worker threads, manager, scrolling strip model, bar, tab bars, compositors, launcher, file search, system menu. One big file by preference (see CLAUDE.md; splitting would not change build time). |
| `crates/astur/src/layout.rs` | ~1300 | Pure tiling geometry (dwindle, master, columns, grid, monocle, scrolling strip: spans, offset clamp, peek/park `classify`, `park_spot`) + tests. `RECT` type only, no Win32 calls. |
| `crates/astur/src/yasb.rs` | ~800 | GlazeWM-compatible workspace WebSocket API for YASB (loopback only, off by default). |
| `crates/astur-config/src/lib.rs` | ~5800 | `Config` struct, `LAYOUTS` (the one list of layout names), bar `PALETTES`, `BAR_PRESETS`, `BAR_ACTIONS` / `BAR_CLICK_DEFAULTS` and the shared bar formatters (`format_clock`, `format_date`, `workspace_label`, `bar_on`), documented-default file templates, key/value parser. **No Win32** — pure data, shared with the settings GUI (aliased `config` in the WM). |
| `crates/astur-settings/` | ~11200 (~1900 tests) | Settings GUI (egui), shipped. Separate process — a GUI crash can't touch the WM. Instant apply with Undo, Ctrl+F search, card rows (`row_core`), tables for record keys (`table`, `Rows`, `fold_*` / `*_check`), key capture chips, the Astur key picker with its 15 s Keep / revert trial (`key_picker`, `switch_key`, `Trial`) and per-shortcut notes from astur-config's `compile_keys` (`notes`, `row_noted`; the GUI has no conflict model of its own), the Status bar page (preset gallery `presets_ui` / `stamp` / `preset_diff`, egui previews `bar_preview` built from astur-config's own `format_clock` / `format_date` / `workspace_label`, drag-and-drop zones `zone_editor` / `move_widget`, format drop-downs `format_pick`, click actions `click_rows` / `click_ui` / `set_click` (the default is kept as no line), the screens picker `monitors_row`; `status_bar_rows_fit_their_width` guards against a row pushing the page past the window), Home's Quick setup (`bar_style_row`, `gaps_row`, `tips`), and pickers for non-technical users (`choice_row` drop-downs of detected apps / web searches with Custom… into More options, `font_row` with a GDI-drawn sample, `path_row` / `browse_button`, `window_picker`). Its only Win32 is `mod sys` (windows 0.58, the WM's version): font families, a font sample, installed browsers and App Paths, the open windows, the screen count, the file / folder dialog on its own thread. Never launch it to check a visual change: `cargo test -p astur-settings -- --ignored render_pages` renders every page to PNGs in %TEMP%\astur-settings-render. |
| `Cargo.toml` (root) | — | `[workspace]` manifest + release profile. WM deps live in `crates/astur/Cargo.toml`. |

`main.rs` is large but sectioned with `// ====` banners. Use the function map
(`grep -n "^fn \|^unsafe extern\|^struct \|^static " crates/astur/src/main.rs`).

## Threads (mental model)

The process is a set of single-purpose threads talking over `Mutex`/`Condvar`
queues. Keep it that way — do not move work onto the hooks (main thread).

- **Main thread** — installs and runs the hooks: `mouse_proc` / `keyboard_proc`
  (`WH_MOUSE_LL` / `WH_KEYBOARD_LL`) and `win_event_proc` (out-of-context
  WinEvents). Hooks must return fast: push a `Cmd`, do no work, no lock without
  an atomic guard (`ANY_DRAG`/`MAIN_DOWN`/`BIND_LIVE`/`SCROLLMON_N` let them early-out).
  Also owns the bar windows, the tray and the scrolling layout's tab-bar
  windows (`astur_tabs`, synced from the manager's `TAB_SNAPSHOT` by
  `tab_bars_sync`; clicks push `Cmd::TabClick`, nothing places windows).
- **Manager loop** (`manager_loop` / `Manager`) — owns all window/workspace/
  strip state. Drains the command queue (`CMDQ`). All `SetWindowPos`/tiling/
  switching/parking here, live Alt-drag placement included (`place_drag_frame`).
  Between commands it also runs the #19.1 border re-assert passes on a
  deadline in its own wait (`BorderRecheck`), with no extra thread.
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
- **Slow stats worker** (`slow_worker`, thread "slow-stats") — the disk
  widget's free space (30 s) and the custom widget's command (its own
  interval, killed after 5 s); both can block, so never on the stats worker.
  Idle unless those widgets are listed.
- **Disk writers** (`state_worker`, `mru_worker`, `rescue_worker`) — the
  manager only fills a latest-wins slot; the disk I/O (state, launcher MRU,
  `rescue.lst`) runs here, so a slow disk never delays a command.
- **Focus-follow worker**, **config watcher**, **launcher**, **sysmenu**,
  **icon/file-search workers**, **IPC**, **YASB**, **hook watchdog**,
  **exit watchdog** (spawned only once an exit restore starts) — as named.

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

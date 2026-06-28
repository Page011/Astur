# AGENTS.md — Astur

Guidance for AI agents (and humans) working on this repo. Read this first, then
the relevant file in `plan/`.

## What Astur is

A fast, free, portable **tiling window manager for Windows 10/11**, shipped as a
single Rust `.exe` with no installer and no required config. Core hooks:

- **Alt + left-drag** — move any window from anywhere on it (no title bar needed).
- **Alt + right-drag** — resize from the nearest corner (red bracket marker).
- **Tiling** — `dwindle` (spiral, default) or `master` layout.
- **Up to 10 virtual workspaces**, shared or per-monitor.
- **Per-monitor status bar** (workspace pills, title, clock, CPU/RAM/battery).
- **Workspace-switch animation** — overlay compositor: off/slide/spring/fade.
- **Window glide** — windows animate to their tile slot on open/move/resize/
  re-tile via a per-window snapshot overlay (off/glide).
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

Quit: `Ctrl+C` in the console (runs `restore_all_windows`), or kill the process.

Config lives in `%USERPROFILE%\.astur\astur.conf` + `navbar.conf`, both created
on first run, fully commented, **hot-reloaded on save**.

## File map (workspace)

| File | Lines (approx) | What |
|---|---|---|
| `crates/astur/src/main.rs` | ~5500 | Everything Win32: hooks, worker threads, manager, bar, animations, launcher, file search, system menu. One big file by design (single TU, fast build). |
| `crates/astur/src/layout.rs` | ~250 | `dwindle_layout` + `master_stack` geometry. `RECT` type only, no Win32 calls. |
| `crates/astur-config/src/lib.rs` | ~600 | `Config` struct, documented-default file templates, key/value parser. **No Win32** — pure data, shared with the settings GUI (aliased `config` in the WM). |
| `crates/astur-settings/` | stub | Settings GUI (egui, WIP). Separate process — a GUI crash can't touch the WM. |
| `Cargo.toml` (root) | — | `[workspace]` manifest + release profile. WM deps live in `crates/astur/Cargo.toml`. |

`main.rs` is large but sectioned with `// ====` banners. Use the function map
(`grep -n "^fn \|^unsafe extern\|^struct \|^static " crates/astur/src/main.rs`).

## Threads (mental model)

The process is a set of single-purpose threads talking over `Mutex`/`Condvar`
queues. Keep it that way — do not move work onto the hook threads.

- **Hook threads** (`mouse_proc`, `keyboard_proc`) — run inside `WH_MOUSE_LL` /
  `WH_KEYBOARD_LL`. Must return fast. They push to queues; they do not do work.
  `ANY_DRAG`/`ALT_DOWN` atomics let them early-out without locking.
- **Manager loop** (`manager_loop` / `Manager`) — owns all window/workspace state.
  Drains the command queue (`CMDQ`). All `SetWindowPos`/tiling/switching here.
- **Position worker** (`position_worker`) — applies drag move/resize off the hook
  so a slow app's `SetWindowPos` can't stall input.
- **Transition worker** (`transition_worker` / `run_transition`) — owns the
  workspace-switch overlay, pumps its own message loop, GDI-composites frames.
- **Glide worker** (`glide_worker` / `run_window_glide`) — same pattern for the
  per-window move/open/close/resize glide overlay.
- **Stats worker** (`stats_worker`) — polls CPU/RAM/battery ~2s for the bar.
- **Focus-follow worker**, **config watcher**, **bar windows** — as named.

## Conventions

- **No emojis in code.** Monochrome only, UI surfaces only.
- Win32 calls are `unsafe`; keep `unsafe` blocks tight and comment the invariant.
- Hot paths: no allocation, no `Mutex` lock unless an atomic guard says there's
  work. Comment *why* a fast-path exists so nobody "simplifies" it away.
- Colours are `COLORREF` (`0x00BBGGRR`) internally; config parses `#RRGGBB`.
- Cosmetic threads get **private copies** of any bitmap/rect data — never share a
  live handle the manager still owns.
- Prefer the simplest built-in Win32 path first; document alternatives + tradeoffs
  in `plan/` before adding complexity.

## Where to go next

- `plan/architecture.md` — deeper component + data-flow notes.
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

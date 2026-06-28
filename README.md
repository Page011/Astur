# Astur — Tiling Window Manager for Windows

**Astur** is a fast, free, open-source **tiling window manager for Windows** —
**Alt-drag** window movement, nearest-corner resize, dwindle/master tiling, a
per-monitor status bar, up to 10 virtual workspaces, and silky composited
animations. The full edition adds an **app launcher + file search** (Alt+Space),
a **power menu** (Alt+Shift+Space), and a tray icon. A keyboard-driven,
i3-style alternative to
[komorebi](https://github.com/LGUG2Z/komorebi),
[GlazeWM](https://github.com/glzr-io/glazewm), and PowerToys FancyZones on
Windows 10 and 11.

[![GitHub release](https://img.shields.io/github/v/release/Page011/Astur)](https://github.com/Page011/Astur/releases/latest)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Website](https://img.shields.io/badge/website-astur.app-366382)](https://astur.app)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org)
[![Platform: Windows 10/11](https://img.shields.io/badge/platform-Windows%2010%20%2F%2011-0078D6.svg)](https://github.com/Page011/Astur/releases/latest)

> **Keywords:** tiling window manager Windows · komorebi alternative · GlazeWM
> alternative · FancyZones alternative · i3 for Windows · Alt-drag windows ·
> app launcher · file search · master-stack layout · Rust window manager

![Astur tiling windows with a status bar on a live Windows desktop](https://astur.app/in-use-screenshot-1.png)

> See it in motion: [watch the demo clip](https://astur.app/#showcase) on the website.

## Editions

Astur comes in two editions from the same project:

| | **Astur Lite** | **Astur** |
|---|---|---|
| Format | single portable `.exe`, no install | installer *(coming)* + tray app |
| Stop / control | console window (`Ctrl+C`) | **tray icon** — Settings / Quit |
| Configuration | hand-edit `.conf` files | settings GUI *(coming)* + `.conf` |
| RAM | ~1 MB, minimal | higher (launcher, search) |
| Tiling · Alt-drag · bar · workspaces · animations | ✓ | ✓ |
| **App launcher** (Alt+Space) | — | ✓ |
| **File search** (Windows Search) | — | ✓ |
| **Power menu** (Alt+Shift+Space) | — | ✓ |

**Astur Lite** is the lean ~1 MB keyboard/console WM for power users who want text
config. **Astur** is the friendlier, installable app — launcher, file search, power
menu, tray, and a settings GUI — with the same motion polish and core tiling.

## Install

- **Astur** — download `astur-windows-x64.exe` from
  [Releases](../../releases/latest) and run it. (A proper installer with auto-start
  and a Start-Menu entry is on the way.)
- **Astur Lite** — the minimal portable build from the
  [`lite`](https://github.com/Page011/Astur/tree/lite) branch / its release. One
  `.exe`, no install, console window.

No admin required. Running as admin lets it manage elevated windows (e.g. Task
Manager) too.

## What it does

- **Alt + left-drag** — move any window by clicking anywhere on it, no title bar needed
- **Alt + right-drag** — resize from the nearest corner (red bracket shows which corner)
- **Tiling mode** — `dwindle` (spiral, default) or `master` layout across up to 10
  virtual workspaces
- **Status bar** — a per-monitor bar with workspace pills, focused title, clock,
  and optional date / CPU / RAM / battery widgets
- **Animations** — workspace switches animate with a composited overlay:
  `slide`, `spring` (overshoot-and-settle, Hyprland-style), `fade`, or `off`. Windows
  also **glide** to their tile slot on open / move / resize / re-tile, composited so
  the real windows land instantly underneath — smooth even with heavy apps.
- **App launcher + file search** *(Astur)* — `Alt+Space` opens a fuzzy picker over
  your installed apps (Start Menu **and** Store/UWP apps like Notepad, with icons) and
  your files (via the Windows Search index). `Tab` shows file details (path, size,
  modified); `Shift+Enter` opens a file's folder; click outside to dismiss.
- **Power menu** *(Astur)* — `Alt+Shift+Space` opens a categorised menu: **Power**
  (Lock / Sleep / Sign out / Restart / Shut down, with a confirm step) and **Setup**.
- **Tray icon** *(Astur)* — no console window; left-click for Settings, right-click for
  Settings / Quit.
- **Extras** — coloured window borders, unfocused-window dimming, focus-follows-mouse,
  per-app window rules, and live config hot-reload.

Left Alt is fully reserved as the Astur modifier — apps never see it. Right Alt is
untouched for normal use. Alt+Tab still works.

## Hotkeys

| Shortcut | Action |
|---|---|
| `Alt` + left-drag | Move window |
| `Alt` + right-drag | Resize from nearest corner |
| `Alt` + `Space` | **App launcher + file search** *(Astur)* |
| `Alt` + `Shift` + `Space` | **Power / system menu** *(Astur)* |
| `Alt` + `T` | Toggle tiling mode on/off |
| `Alt` + `J` / `K` | Focus next / previous window |
| `Alt` + `Shift+J` / `Shift+K` | Swap window with next / previous |
| `Alt` + arrows | Focus window by direction (cursor follows) |
| `Alt` + `Shift` + arrows | Move window by direction (across monitors) |
| `Alt` + `H` / `L` | Shrink / grow master column (`master` layout) |
| `Alt` + `M` | Promote focused window to master |
| `Alt` + `F` | Toggle float for focused window |
| `Alt` + `W` | Close focused window |
| `Alt` + `Enter` | Launch terminal |
| `Alt` + `Shift+Enter` | Launch browser |
| `Alt` + `1`–`9`, `0` | Switch to workspace 1–10 |
| `Alt` + `Shift` + `1`–`9`, `0` | Move focused window to workspace |
| `Alt` + `Tab` | Switch apps (pass-through preserved) |

The letter binds (`J K H L M T F W`) are rebindable in
`%USERPROFILE%\.astur\astur.conf`, along with workspace keys, gaps, layout,
borders, and behaviour. The status bar is configured in `navbar.conf` (same
folder). Both config files **hot-reload** on save — no restart needed.

## Configuration

Two files are created in `%USERPROFILE%\.astur\` on first run, both fully
commented and **hot-reloaded on save**:

- **`astur.conf`** — window manager: workspace mode/count, layout, gaps, master
  ratio, borders, dimming, focus-follows-mouse, cursor warping, animations
  (`workspace_anim` = off/slide/spring/fade, `window_anim` = off/glide), launchers,
  per-app rules (`ignore_classes` / `float_classes`), workspace keys, and the
  rebindable letter hotkeys.
- **`navbar.conf`** — the status bar.

A **settings GUI** is in the works for the full Astur edition so you can configure
everything without touching the `.conf` files (they remain the source of truth, and
power users can keep editing them directly).

## Build from source

Requires [Rust stable](https://rustup.rs). The repo is a Cargo workspace.

```bash
git clone https://github.com/Page011/Astur
cd Astur
cargo build --release -p astur
# binary at: target/release/astur.exe
```

`crates/astur` is the window manager, `crates/astur-config` the shared config
parser, `crates/astur-settings` the settings GUI (WIP). The minimal **Astur Lite**
lives on the [`lite`](https://github.com/Page011/Astur/tree/lite) branch (a single
crate, no workspace).

## How it works

Astur installs two low-level Windows hooks (`WH_MOUSE_LL`, `WH_KEYBOARD_LL`) that
intercept input before it reaches any application. Left Alt is swallowed so it never
triggers app menus or Alt shortcuts — only Astur sees it. Window moves and resizes
are dispatched to a dedicated worker thread so the hooks never stall on a slow
application's `SetWindowPos`. File search queries the Windows Search index off the
input path, so typing stays responsive.

## Quit

- **Astur** — right-click the tray icon → **Quit** (restores all windows, then exits).
- **Astur Lite** — `Ctrl+C` in the console window.

## How Astur compares

| | Astur | komorebi | GlazeWM | FancyZones |
|---|---|---|---|---|
| Master-stack tiling | Yes | Yes | Yes | No (zones only) |
| Alt-drag move/resize anywhere | Yes | No | No | No |
| Silky smooth animations | Yes | No | No | No |
| App launcher + file search built in | Yes | No | No | No |
| Settings GUI | Coming | No (CLI) | Partial | Yes |
| Single portable exe option | Yes (Lite) | No | No (installer) | Part of PowerToys |
| Virtual workspaces | Yes | Yes | Yes | Via Windows |
| Config file required to start | No | Yes | Yes | No |
| Language | Rust | Rust | C++ | C# |

See the full [comparison: Astur vs GlazeWM, komorebi & FancyZones](https://astur.app/compare).

## FAQ

**Is Astur a komorebi or GlazeWM alternative?** Yes — same master-stack tiling and
workspace idea, plus Alt-drag move/resize, an app launcher with file search, and a
power menu, with a single-exe (Lite) option and no required config file.

**Does it work on Windows 11?** Yes, on Windows 10 and 11, x64.

**Do I need admin rights?** No. Running as admin lets it manage elevated windows too.

**What's the difference between Astur and Astur Lite?** See [Editions](#editions) —
Lite is the minimal ~1 MB console exe; Astur adds the launcher, file search, power
menu, tray, and (soon) a settings GUI.

## Disclaimer

Astur is an independent project, not affiliated with or endorsed by any other window
manager. It is a clean-room Rust implementation for Windows. All trademarks belong to
their respective owners.

## Licence

Apache-2.0 — see [LICENSE](LICENSE).

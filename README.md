# Astur Lite — Tiling Window Manager for Windows

**Astur Lite** is a fast, free, open-source **tiling window manager for Windows**
with **Alt-drag** window movement, nearest-corner resize,
dwindle/master tiling, a per-monitor status bar, and up to 10 virtual
workspaces — all in a single portable Rust `.exe` with no installer (~1 MB,
system tray icon, runs silently in the background). A lightweight alternative to
[komorebi](https://github.com/LGUG2Z/komorebi),
[GlazeWM](https://github.com/glzr-io/glazewm), and PowerToys FancyZones for
keyboard-driven, i3-style window management on Windows 10 and 11.

> **Astur Lite** is the minimal edition. Want an **app launcher + file search**
> (Alt+Space), a **power menu** (Alt+Shift+Space), and a settings
> GUI? Use the full **[Astur](https://github.com/Page011/Astur)** (the `main` branch).

[![GitHub release](https://img.shields.io/github/v/release/Page011/Astur)](https://github.com/Page011/Astur/releases/latest)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Website](https://img.shields.io/badge/website-astur.app-366382)](https://astur.app)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org)
[![Platform: Windows 10/11](https://img.shields.io/badge/platform-Windows%2010%20%2F%2011-0078D6.svg)](https://github.com/Page011/Astur/releases/latest)

## contributors
<table>
  <tbody>
    <tr>
      <td align="center" valign="top" width="14.28%"><a href="https://github.com/azumafuji"><img src="https://avatars.githubusercontent.com/u/393782?v=4?s=100" width="100px;" alt="Dean Sellis"/><br /><sub><b>Dean Sellis</b></sub></a><br />
    </tr>
  </tbody>
</table>

> **Keywords:** tiling window manager Windows · komorebi
> alternative · GlazeWM alternative · FancyZones alternative · i3 for Windows ·
> Alt-drag windows · master-stack layout · Rust window manager

![Astur tiling windows with a status bar on a live Windows desktop](https://astur.app/in-use-screenshot-1.png)

> See it in motion: [watch the demo clip](https://astur.app/#showcase) on the website.

## Install

Download `astur-windows-x64.exe` from [Releases](../../releases/latest) and run it.
No installer. Single `.exe`.

## What it does

- **Alt + left-drag** (or **Win+Alt + left-drag**) — move any window by clicking anywhere on it, no title bar needed
- **Alt + right-drag** (or **Win+Alt + right-drag**) — resize from the nearest corner (red bracket shows which corner)
- **Tiling mode** — `dwindle` (spiral, default) or `master` layout across up to 10
  virtual workspaces
- **Configurable modifier** — choose `modifier = win_alt` or `alt` (default). In `win_alt` mode, plain Left Alt is 100% free for apps (e.g. Emacs Meta key `M-x`) and Windows key shortcuts are untouched. In `alt` mode, `passthrough_classes` lets specific apps bypass interception when focused.
- **Compound shortcut passthrough** — chords like `Win + Alt + Space` (PowerToys / Windows Command Palette) and `Ctrl + Alt + ...` pass straight through cleanly.
- **Status bar** — a per-monitor bar with workspace pills, focused title, clock,
  and optional date / CPU / RAM / battery widgets
- **Autohide & smooth fade** — optional OLED-friendly autohide with smooth cubic-ease alpha fade animations, zero idle CPU usage, and mouse edge reveal
- **Multi-monitor & DPI scaling** — Per-Monitor V2 DPI awareness with physical monitor bounding prevents window shadow bleed and cross-monitor scaling ballooning
- **System tray icon** — runs quietly with no persistent console window; left-click to open config folder, right-click to open configuration or quit cleanly; single-instance guard prevents duplicate processes
- **Animations** — workspace switches slide in; opening / moving / re-tiling
  glide to place (positional tweens, configurable speed or off)
- **Extras** — coloured window borders, unfocused-window dimming,
  focus-follows-mouse, per-app window rules, and live config hot-reload

By default, Left Alt is reserved as the Astur modifier (apps never see it). Set `modifier = win_alt` in `astur.conf` to use Win+Alt instead, freeing Left Alt for your applications. Right Alt is untouched for normal use. Alt+Tab and compound chords still work.

## Hotkeys

> **Note:** Hotkeys use your configured modifier (`Alt` by default, or `Win + Alt` if `modifier = win_alt` in `astur.conf`). Below, `[Mod]` represents your configured modifier (`Alt` or `Win+Alt`).

| Shortcut | Action |
|---|---|
| `[Mod]` + left-drag | Move window |
| `[Mod]` + right-drag | Resize from nearest corner |
| `[Mod]` + `T` | Toggle tiling mode on/off |
| `[Mod]` + `J` / `K` | Focus next / previous window |
| `[Mod]` + `Shift+J` / `Shift+K` | Swap window with next / previous |
| `[Mod]` + arrows | Focus window by direction (cursor follows) |
| `[Mod]` + `Shift` + arrows | Move window by direction (across monitors) |
| `[Mod]` + `H` / `L` | Shrink / grow master column (`master` layout) |
| `[Mod]` + `M` | Promote focused window to master |
| `[Mod]` + `F` | Toggle float for focused window |
| `[Mod]` + `W` | Close focused window |
| `[Mod]` + `Enter` | Launch terminal |
| `[Mod]` + `Shift+Enter` | Launch browser |
| `[Mod]` + `1`–`9`, `0` | Switch to workspace 1–10 |
| `[Mod]` + `Shift` + `1`–`9`, `0` | Move focused window to workspace |
| `Alt` + `Tab` | Switch apps (pass-through preserved) |
| `Win` + `Alt` + `Space` | Windows Command Palette / PowerToys (pass-through preserved) |

In the default `dwindle` layout, resize tiles with **[Mod] + right-drag** (the
split reflows); `[Mod] + H` / `L` adjust the master width in `master` layout.

The letter binds (`J K H L M T F W`) are rebindable in
`%USERPROFILE%\.astur\astur.conf` (`key_focus_next`, `key_close_window`,
etc.), along with workspace keys, gaps, layout, borders, and behaviour. The
status bar is configured separately in `navbar.conf` (same folder). Arrows and
`Enter` are fixed. Both config files **hot-reload** on save — no restart needed.

## Configuration

Two files are created in `%USERPROFILE%\.astur\` on first run, both
fully commented and **hot-reloaded on save**:

- **`astur.conf`** — window manager:
  - `modifier = alt | win_alt`: sets the primary modifier chord. Use `win_alt` if you rely heavily on Left Alt in applications (e.g. Emacs Meta key `M-x`).
  - `passthrough_classes`: comma-separated window classes (e.g. `Emacs, VirtualBox`) that bypass Astur's modifier when focused (in `modifier = alt` mode).
  - `workspace_mode` / `workspace_count`: shared or per-monitor virtual workspaces.
  - `layout`, `gaps`, `master_ratio`: tiling behavior and geometry.
  - `borders`, `dimming`: colored active borders and unfocused window dimming.
  - `focus_follows_mouse`, `warp_cursor`: mouse navigation options.
  - `animations`, `animation_ms`: window glide and workspace slide transitions.
  - `terminal_cmd`, `browser_cmd`: custom hotkey launchers.
  - `ignore_classes` / `float_classes`: per-app window rules.
  - Workspace keys and rebindable letter hotkeys (`key_focus_next`, `key_close_window`, etc.).
- **`navbar.conf`** — the status bar (see below).

Workspaces default to **shared** mode: numbered globally from your primary
monitor outward (ws1 = main monitor, ws2 = next, …). Set
`workspace_mode = per_monitor` to give each monitor its own independent 1–N.

### Status bar

A bar is drawn on every monitor: workspace pills on the left (click to switch),
the focused window title in the centre, and a widget cluster on the right.
`navbar.conf` options:

| Option | Description |
|---|---|
| `enabled`, `height`, `bottom`, `padding` | Show/size/dock the bar |
| `autohide` | Auto-hide the bar when inactive (`true` / `false`, default `false`) |
| `autohide_delay` | Delay before hiding after activity (e.g. `2s`, `500ms`, default `2s`) |
| `autohide_fade_ms` | Smooth fade-out duration in milliseconds (`180`, default `180`, `0` = instant) |
| `autohide_window_switch` | Whether switching windows reveals the bar (`true` / `false`, default `false`) |
| `font_name`, `font_size` | Any installed font family + text height |
| `hide_empty_workspaces` | Show only active + occupied pills |
| `show_title` | Focused window title (centre) |
| `show_layout` | Layout + tiling/floating state |
| `show_clock`, `clock_24h` | Clock (24h or 12h am/pm) |
| `show_date`, `date_format` | Date with `yyyy MM dd MMM ddd` tokens |
| `show_cpu`, `show_mem`, `show_battery` | Live CPU / RAM / battery %, polled ~2s |
| `bg`, `fg`, `accent`, `inactive` | Colours (`#RRGGBB`) |

## Build from source

Requires [Rust stable](https://rustup.rs).

```bash
git clone https://github.com/Page011/Astur
cd Astur
cargo build --release
# binary at: target/release/astur.exe
```

## How it works

Astur runs as a native Windows subsystem application (`windows_subsystem = "windows"`),
operating quietly in the background without keeping a console window open. If launched from a
terminal or command prompt, it automatically attaches to the parent console to print its dynamic
startup banner and hotkey summary.

Astur installs two low-level Windows hooks (`WH_MOUSE_LL`, `WH_KEYBOARD_LL`) that intercept
input before it reaches applications. Depending on your configuration (`modifier = alt` or `win_alt`),
Astur captures its modifier chords while intelligently passing through compound shortcuts (such as
`Win + Alt + Space` or `Ctrl + Alt + ...`) and respecting per-app passthrough rules (`passthrough_classes`).

Window moves and resizes are dispatched to a dedicated worker thread so hooks never stall on a slow
application's `SetWindowPos`. Tile geometries are clamped strictly within physical monitor bounds with
Per-Monitor V2 DPI awareness, preventing shadow bleed and cross-monitor scaling ballooning.

## Quit

- **System Tray**: Right-click the Astur hawk icon in the system notification tray and select **Quit Astur**.
- **Console**: Press `Ctrl+C` if launched from a terminal.
- (All managed windows are cleanly restored to their normal state upon exiting.)

## How Astur compares

If you've used a tiling window manager on Linux (i3, sway) and want
the same flow on Windows, Astur aims to be the smallest thing that works:

| | Astur | komorebi | GlazeWM | FancyZones |
|---|---|---|---|---|
| Master-stack tiling | Yes | Yes | Yes | No (zones only) |
| Alt-drag move/resize anywhere | Yes | No | No | No |
| Silky smooth animations | Yes | No | No | No |
| Single portable exe, no install | Yes | No (needs config) | No (installer) | Part of PowerToys |
| Virtual workspaces | Yes | Yes | Yes | Via Windows |
| Config file required to start | No | Yes | Yes | No |
| Language | Rust | Rust | C++ | C# |

Astur trades configurability for zero-setup speed: run the exe and Alt-drag
works immediately, tiling is one keypress away.

See the full [comparison: Astur vs GlazeWM, komorebi & FancyZones](https://astur.app/compare).

## FAQ

**Is Astur a komorebi or GlazeWM alternative?** Yes — same master-stack
tiling and workspace idea, but it runs from a single exe with no config file
required and adds Alt-drag move/resize.

**Does it work on Windows 11?** Yes, on Windows 10 and 11, x64.

**Do I need admin rights?** No — it's a portable exe. Running as admin is
recommended so it can manage elevated windows (e.g. Task Manager) too.

## Disclaimer

Astur is an independent project, not affiliated with or endorsed by any other
window manager. It is a clean-room Rust implementation for Windows. All
trademarks belong to their respective owners.

## Licence

Apache-2.0 — see [LICENSE](LICENSE).

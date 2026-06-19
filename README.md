# suprland — Tiling Window Manager for Windows

**suprland** is a fast, free, open-source **tiling window manager for Windows**
with **Hyprland-style** Alt-drag window movement, nearest-corner resize,
master-stack tiling, and 9 virtual workspaces — all in a single portable Rust
`.exe` with no installer. A lightweight alternative to
[komorebi](https://github.com/LGUG2Z/komorebi),
[GlazeWM](https://github.com/glzr-io/glazewm), and PowerToys FancyZones for
keyboard-driven, i3/Hyprland-style window management on Windows 10 and 11.

[![GitHub release](https://img.shields.io/github/v/release/Page011/Suprland)](https://github.com/Page011/Suprland/releases/latest)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Website](https://img.shields.io/badge/website-suprland.vercel.app-366382)](https://suprland.vercel.app)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org)
[![Platform: Windows 10/11](https://img.shields.io/badge/platform-Windows%2010%20%2F%2011-0078D6.svg)](https://github.com/Page011/Suprland/releases/latest)

> **Keywords:** tiling window manager Windows · Hyprland for Windows · komorebi
> alternative · GlazeWM alternative · FancyZones alternative · i3 for Windows ·
> Alt-drag windows · master-stack layout · Rust window manager

## Install

Download `suprland-windows-x64.exe` from [Releases](../../releases/latest) and run it.
No installer. Single `.exe`.

## What it does

- **Alt + left-drag** — move any window by clicking anywhere on it, no title bar needed
- **Alt + right-drag** — resize from the nearest corner (red bracket shows which corner)
- **Tiling mode** — master-stack layout across 9 virtual workspaces

Left Alt is fully reserved as the suprland modifier — apps never see it.
Right Alt is untouched for normal use. Alt+Tab still works.

## Hotkeys

| Shortcut | Action |
|---|---|
| `Alt` + left-drag | Move window |
| `Alt` + right-drag | Resize from nearest corner |
| `Alt` + `T` | Toggle tiling mode on/off |
| `Alt` + `J` / `K` | Focus next / previous window |
| `Alt` + `Shift+J` / `Shift+K` | Swap window with next / previous |
| `Alt` + arrows | Focus window by direction (cursor follows) |
| `Alt` + `Shift` + arrows | Move window by direction (across monitors) |
| `Alt` + `H` / `L` | Shrink / grow master column |
| `Alt` + `M` | Promote focused window to master |
| `Alt` + `F` | Toggle float for focused window |
| `Alt` + `W` | Close focused window |
| `Alt` + `Enter` | Launch terminal |
| `Alt` + `Shift+Enter` | Launch browser |
| `Alt` + `1`–`9` | Switch to workspace 1–9 |
| `Alt` + `Shift+1`–`9` | Move focused window to workspace |
| `Alt` + `Tab` | Switch apps (pass-through preserved) |

The letter binds (`J K H L M T F W`) are rebindable in
`%USERPROFILE%\.suprland\suprland.conf` (`key_focus_next`, `key_close_window`,
etc.). Workspace keys, gaps, layout, borders, and the status bar are configured
in the same file. Arrows and `Enter` are fixed.

## Build from source

Requires [Rust stable](https://rustup.rs).

```bash
git clone https://github.com/Page011/Suprland
cd suprland
cargo build --release
# binary at: target/release/suprland.exe
```

## How it works

suprland installs two low-level Windows hooks (`WH_MOUSE_LL`, `WH_KEYBOARD_LL`)
that intercept input before it reaches any application. Left Alt is swallowed
so it never triggers app menus or Alt shortcuts — only suprland sees it. Window
moves and resizes are dispatched to a dedicated worker thread so the hooks never
stall on a slow application's `SetWindowPos`.

## Quit

Press `Ctrl+C` in the console window. (Or kill the process from Task Manager.)

## How suprland compares

If you've used a tiling window manager on Linux (i3, Hyprland, sway) and want
the same flow on Windows, suprland aims to be the smallest thing that works:

| | suprland | komorebi | GlazeWM | FancyZones |
|---|---|---|---|---|
| Master-stack tiling | Yes | Yes | Yes | No (zones only) |
| Alt-drag move/resize anywhere | Yes | No | No | No |
| Single portable exe, no install | Yes | No (needs config) | No (installer) | Part of PowerToys |
| Virtual workspaces | Yes | Yes | Yes | Via Windows |
| Config file required to start | No | Yes | Yes | No |
| Language | Rust | Rust | C++ | C# |

suprland trades configurability for zero-setup speed: run the exe and Alt-drag
works immediately, tiling is one keypress away.

## FAQ

**Is suprland a komorebi or GlazeWM alternative?** Yes — same master-stack
tiling and workspace idea, but it runs from a single exe with no config file
required and adds Hyprland-style Alt-drag move/resize.

**Does it work on Windows 11?** Yes, on Windows 10 and 11, x64.

**Do I need admin rights?** No. It's a portable exe. It is reccomended to run as admin to beable to run to its best ability.

## Licence

Apache-2.0 — see [LICENSE](LICENSE).

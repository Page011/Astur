# suprland

Alt-drag move/resize and master-stack tiling window manager for Windows,
inspired by Hyprland's mouse bindings.

[![GitHub release](https://img.shields.io/github/v/release/Page011/Suprland)](https://github.com/Page011/Suprland/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

## Install

Download `suprland-windows-x64.exe` from [Releases](../../releases/latest) and run it.
No installer. No admin rights required. Single `.exe`.

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
| `Alt` + `H` / `L` | Shrink / grow master column |
| `Alt` + `Enter` | Promote focused window to master |
| `Alt` + `F` | Toggle float for focused window |
| `Alt` + `Q` | Close focused window |
| `Alt` + `1`–`9` | Switch to workspace 1–9 |
| `Alt` + `Shift+1`–`9` | Move focused window to workspace |
| `Alt` + `Tab` | Switch apps (pass-through preserved) |

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

## Licence

MIT — see [LICENSE](LICENSE).

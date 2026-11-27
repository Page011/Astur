# Astur — Tiling Window Manager for Windows

Astur is a free, portable tiling window manager for Windows 10 and 11, written in Rust.
Hold Left Alt and drag to move or resize a window. Windows tile themselves in one of
10 layouts (9 tiling, plus floating), across up to 20 workspaces, with a status bar on every monitor.

[![GitHub release](https://img.shields.io/github/v/release/Page011/Astur)](https://github.com/Page011/Astur/releases/latest)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Website](https://img.shields.io/badge/website-astur.app-366382)](https://astur.app)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org)
[![Platform: Windows 10/11](https://img.shields.io/badge/platform-Windows%2010%20%2F%2011-0078D6.svg)](https://github.com/Page011/Astur/releases/latest)

![Astur tiling windows with a status bar on a live Windows desktop](https://astur.app/astur-showcase-poster.webp)

> See it in motion: [watch the demo clip](https://astur.app/#showcase) on the website.

## Install

Download from the [latest release](https://github.com/Page011/Astur/releases/latest):

- **Installer (recommended):** `Astur-Setup-<version>.exe`. Installs for your user
  only, no admin needed. "Start Astur automatically when I sign in" is ticked by
  default. Untick it if you don't want that.
- **Portable:** `astur-windows-x64.exe`. One file, no install. Same window manager,
  but no Settings app.
- **Astur Lite:** the older, minimal edition, text config only. Download it from the
  [v1.0.1 release](https://github.com/Page011/Astur/releases/tag/v1.0.1) (it runs in a console window).
  Its file is also called `astur-windows-x64.exe`, so don't mix the two up.
  Source: the [`lite`](https://github.com/Page011/Astur/tree/lite) branch.

Windows that run as admin (an admin terminal, Task Manager) stay untiled and show on
every workspace, unless Astur runs as admin too.

## What you get

- Windows tile by themselves as you open them.
- Hold the main key and drag anywhere on a window to move or resize it.
- Workspaces: 10 by default, up to 20.
- A status bar on every monitor: workspaces, the focused window, clock and system
  info. Click a workspace, or scroll the wheel over the bar, to switch.
- App launcher with file search: Alt+Space.
- System menu: Alt+Shift+Space. **Power** (lock, sleep, restart, shut down) and
  **Setup** (Settings, config folder, reload).
- A tray icon: click it to open Settings (installer only). Right-click it for Quit.

## The main key

Every shortcut uses one key, the main key. **In this README, Alt means the main key.**

| `main_key` | Key | Notes |
|---|---|---|
| `lalt` (default) | Left Alt | Apps never see Left Alt. Use Right Alt for app shortcuts like Alt+F4. |
| `win` | Windows key | Astur's shortcuts replace the Windows ones on the same keys (Win+arrows, Win+1 ... Win+0, Win+Space ...). Win+L always locks the PC, so grow (L) moves to Win+O, unless one of your keys already uses O. |
| `alt` | Either Alt | Apps keep the Alt shortcuts Astur does not use. |
| `ctrl+win` | Ctrl + Windows key | Both held together. |
| `capslock` | Caps Lock | Caps Lock no longer turns capitals on. Turn it off first. |

Change it in **Settings > Shortcuts > Astur key**, or set `main_key` in `astur.conf`.
In Settings, a new key goes back after 15 seconds unless you keep it.

## Keys

| Keys | Action |
|---|---|
| Alt + left-drag | Move window |
| Alt + right-drag | Resize from the nearest corner |
| Alt+Space | Launcher and file search |
| Alt+Shift+Space | System menu (power, Settings, reload) |
| Alt+J / Alt+K | Focus next / previous window |
| Alt+Shift+J / K | Swap with next / previous window |
| Alt+arrows | Focus the window in that direction |
| Alt+Shift+arrows | Move the window in that direction (also to another monitor) |
| Alt+H / Alt+L | Shrink / grow (master column, focused split, or mosaic window) |
| Alt+M | Make the focused window the master |
| Alt+Y / Alt+Shift+Y | Next / previous layout |
| Alt+T | Tiling on / off |
| Alt+F | Float / tile the focused window |
| Alt+W | Close the focused window |
| Alt+Enter | Open a terminal (Windows Terminal) |
| Alt+Shift+Enter | Open your default browser |
| Alt+1 ... Alt+9, Alt+0 | Go to workspace 1 to 10 |
| Alt+Shift+1 ... Alt+Shift+0 | Move the focused window to that workspace and go with it |

- Alt+Tab is still the normal Windows switcher.
- The letter keys can be changed in Settings > Shortcuts or in `astur.conf`.
- Workspace keys: `workspace_keys` in `astur.conf`.

## Scrolling keys

The `scrolling` layouts add their own keys. `astur.conf` lists them all.

| Keys | Action |
|---|---|
| Alt+Left / Alt+Right | Focus the column left / right |
| Alt+Up / Alt+Down | Focus the window up / down in the column |
| Alt+Shift+arrows | Move the column, or the window inside its column |
| Alt+H / Alt+L | Column narrower / wider |
| Alt+R / Alt+Shift+R | Cycle column width / window height presets |
| Alt+C | Centre the focused column |
| Alt+[ / Alt+] | Pull the window into the column left / right, or push it out |
| Alt+Shift+T | Column stacked / tabbed |
| Alt+Home / Alt+End | First / last column |
| Alt+wheel | Move through the columns |

**Vertical scrolling:** on `scrolling_vertical` each column is a full-width row and the
axes swap. Alt+Up / Alt+Down go to the row above / below. Alt+Left / Alt+Right move
inside the row.

## Layouts

| Layout | What it does |
|---|---|
| `dwindle` (default) | Each new window takes half of the remaining space, spiralling toward the bottom corner. |
| `master` | One big window on the left, the rest stacked on the right. |
| `columns` | Equal-width columns, side by side. |
| `rows` | Every window is a full-width row of equal height, stacked top to bottom, all visible: `columns` turned on its side. Good for portrait monitors. |
| `grid` | Balanced rows and columns. |
| `monocle` | Every window fills the screen. The focused one is on top. |
| `scrolling` | Columns on an endless horizontal strip. Columns past the screen edge are a scroll away. |
| `scrolling_vertical` | The scrolling strip turned on its side: full-width rows you scroll up and down. |
| `mosaic` | Every window keeps its own size, in a centred cluster. They shrink together only when they don't fit. Drag a window anywhere to rearrange; Alt+M moves it to the middle. |
| `floating` | Astur places nothing. Windows stay where you put them. |

Pick a layout in Settings > Layout, or in `astur.conf`:

- `layout` sets the default for everything.
- `monitor_layouts` sets one per monitor (1 = leftmost).
- `workspace_layouts` sets one per workspace.
- Alt+Y / Alt+Shift+Y cycle the current workspace's layout, in the order above.
  It lasts until the config reloads or Astur restarts. Use `workspace_layouts` to keep it.

## Settings

The Settings app comes with the installer. To open it:

- click the tray icon, or
- press Alt+Shift+Space and pick **Setup > Settings**.

Changes apply at once and can be undone. Press Ctrl+F to find any setting.
Settings saves to the same config files as below.

## Config files

Two plain text files in `%APPDATA%\Astur\`:

- `astur.conf`: the window manager (keys, layouts, workspaces, rules, look).
- `navbar.conf`: the status bar.

Good to know:

- Astur creates both on first run, with a comment on every setting. Read them for
  everything this README leaves out.
- Save a file and Astur reloads it about 0.1 s later.
- Delete a file and restart Astur to get a fresh default copy.
- Save as UTF-8.
- `config_folder = home` keeps them in `%USERPROFILE%\.astur\` instead. Astur moves
  the folder there the next time it starts.
- Point `ASTUR_CONFIG` / `ASTUR_NAVBAR` outside both Astur folders: an override inside
  one keeps the folder from moving, or leaves the other file behind.
- Astur Lite reads only `%USERPROFILE%\.astur\`.
- Alt+Shift+Space > **Setup > Open config folder** opens the folder.

## Quit

- **Astur:** right-click the tray icon > **Quit**. Windows on hidden workspaces are shown again.
- **Astur Lite v1.0.1:** `Ctrl+C` in its console window.

## Help

- Run `astur.exe --check` in a terminal (the installer puts it at
  `%LOCALAPPDATA%\Programs\Astur\Astur.exe`). It prints a report and saves it as
  `astur-check.txt` in the config folder. Attach that to an issue.
- For more detail, set `log_level = info` in `astur.conf` and attach `astur.log`
  from the same folder.
- [Report a bug](https://github.com/Page011/Astur/issues).

## More

- **YASB:** Astur can drive YASB's workspace widget. Turn it on in
  Settings > Integrations, or set `yasb_enabled = true` in `astur.conf` (and
  `enabled = false` in `navbar.conf` to hide Astur's own bar). See the
  [YASB setup](https://github.com/Page011/Astur/blob/main/integrations/yasb/README.md).
- **Scripting:** off by default. `ipc_enabled = true` opens the local named pipe
  `\\.\pipe\astur`. Send `help` for the command list. Leave `ipc_allow_launch` off
  unless you need `launch`.

## Build from source

Needs Windows, [Rust stable](https://rustup.rs) with the MSVC toolchain, and the
Visual Studio C++ Build Tools (the build uses `rc.exe`).

```bash
git clone https://github.com/Page011/Astur
cd Astur
cargo build --release
```

- This builds `target/release/astur.exe` (window manager) and
  `target/release/astur-settings.exe` (Settings).
- `cargo build --release -p astur` builds only the window manager.
- `crates/astur` is the window manager, `crates/astur-config` the config parser,
  `crates/astur-settings` the Settings app.

## Author

Astur is created and maintained by **Nigel**.

## Disclaimer

Astur is an independent project, not affiliated with or endorsed by any other window
manager. It is a clean-room Rust implementation for Windows. All trademarks belong to
their respective owners.

## Star History

<a href="https://www.star-history.com/?repos=page011%2Fastur&type=date&legend=bottom-right">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=page011/astur&type=date&theme=dark&legend=bottom-right" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=page011/astur&type=date&legend=bottom-right" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=page011/astur&type=date&legend=bottom-right" />
 </picture>
</a>

## Licence

Apache-2.0 — see [LICENSE](LICENSE).

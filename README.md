# Astur — Tiling Window Manager for Windows

**Astur** is a fast, free, open-source **tiling window manager for Windows** —
**Alt-drag** window movement, nearest-corner resize, seven tiling layouts
(including a Niri-style scrolling layout, chosen per monitor or per workspace), a
per-monitor status bar, up to 20 virtual workspaces, and silky composited
animations. Full edition adds a configurable **app launcher + file search**
(Alt+Space), **system menu** (Alt+Shift+Space), desktop tools, settings GUI, and
tray icon. A keyboard-driven,
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
> app launcher · file search · master-stack layout · Niri-style scrolling layout ·
> Rust window manager

![Astur tiling windows with a status bar on a live Windows desktop](https://astur.app/in-use-screenshot-1.png)

> See it in motion: [watch the demo clip](https://astur.app/#showcase) on the website.

## Editions

Astur comes in two editions from the same project:

| | **Astur Lite** | **Astur** |
|---|---|---|
| Format | single portable `.exe`, no install | **installer** + tray app; WM-only portable option |
| Stop / control | console window (`Ctrl+C`) | **tray icon** — Settings / Quit |
| Configuration | hand-edit `.conf` files | **settings GUI** + `.conf` |
| RAM | ~1 MB, minimal | higher (launcher, search) |
| Tiling · Alt-drag · bar · workspaces · animations | ✓ | ✓ |
| **Scrolling layout** · tabbed columns · per-monitor and per-workspace layouts | — | ✓ |
| **App launcher + file search + calculator** (Alt+Space) | — | ✓ |
| **Power menu** (Alt+Shift+Space) | — | ✓ |
| **Bar widgets** (volume · network · app buttons) + light/dark theme | — | ✓ |
| Optional switcher · scratchpad · clipboard · emoji · wallpapers · media · IPC | — | ✓ |

**Astur Lite** is the lean ~1 MB keyboard/console WM for power users who want text
config. **Astur** is the friendlier, installable app — launcher, file search, power
menu, tray, and a settings GUI — with the same motion polish and core tiling.

## Install

- **Astur** — download `Astur-Setup-<version>.exe` from the
  [latest stable release](https://github.com/Page011/Astur/releases/latest) and run it
  (per-user install, no admin, optional
  start-on-login), or grab the portable `astur-windows-x64.exe` from the same
  release. The portable asset contains the full window manager but not the separate
  `astur-settings.exe` companion; use the installer for the Settings GUI.
- **Astur Lite** — the minimal portable build from the
  [`lite`](https://github.com/Page011/Astur/tree/lite) branch or pinned
  [v1.0.1 release](https://github.com/Page011/Astur/releases/tag/v1.0.1). One
  `.exe`, no install, console window.

No admin required. Elevated windows (an admin terminal, or Task Manager on an admin
account) are left untiled where they are, and stay visible on every workspace,
unless Astur runs as admin too: Windows does not let a normal process move or hide
them. An admin Astur (with UAC on) still starts the apps it launches unelevated
(through Explorer), keeps the custom bar widget off, and takes IPC only from elevated
clients: its config files are writable by any program you run. With UAC off, or on the
built-in Administrator account, every program already runs as admin, so none of this
applies.

## What it does

- **Alt + left-drag** — move any window by clicking anywhere on it, no title bar needed
- **Alt + right-drag** — resize from the nearest corner (red bracket shows which corner)
- **Tiling mode** — `dwindle` (spiral), `master`, equal `columns`, balanced `grid`,
  stacked `monocle`, or Niri-style `scrolling` and `scrolling_vertical` (see [Scrolling layout](#scrolling-layout))
  across up to 20 named/icon-labelled virtual workspaces. Each monitor and each
  workspace can run its own layout (`monitor_layouts`, `workspace_layouts`)
- **Status bar** — a per-monitor bar (or only on the screens you pick,
  `monitors`) with three fully configurable widget zones
  (left / center / right): workspace pills, focused title, app buttons (click to
  focus), clock, date, CPU, RAM, battery, **network speed**, **volume**
  (scroll over it to adjust, click to mute), **keyboard language**, **disk
  space**, **uptime**, **your own text or a command's output**, **music
  buttons** and the Astur logo. Scroll anywhere else on the bar to
  cycle workspaces, click a pill to switch. Every widget has **click actions**
  for the left, right and middle button (`clock_click = calendar`,
  `cpu_right_click = launch wt`): out of the box the clock and date open the
  calendar, volume mutes, battery and network open quick settings, CPU and RAM
  open Task Manager, a right-click opens the matching Windows Settings page.
  Workspace marks can show numbers, names, icons or both, plus the apps open
  on each (`workspace_label`, `workspace_apps`); an optional hover highlight,
  dot or powerline separators, a bold app-name title (`title_format = {app}`),
  and clock seconds / AM-PM and long date tokens. Shapes: full-width strip,
  **floating rounded bar** (margins + corner radius), **islands** (a rounded
  island per zone) or **pills** (a capsule per widget, each in its own palette
  colour if you like, with black or white text to suit it), with an optional
  outline, plus **auto-hide** (reveals on
  edge hover). Seven colour **palettes** (classic, Catppuccin, Nord, Gruvbox,
  Rose Pine, mono, and system, which takes your Windows accent), each with a
  dark and a light variant, and four active-workspace styles (box, anti-aliased
  pill, underline, dots). Stats can show **Windows icons** (`icon_mode = glyph`:
  Segoe Fluent Icons, or Segoe MDL2 Assets on Windows 10, no install) with the
  battery level and charging, the volume level and mute, and wifi or ethernet;
  a low battery turns red (`glyph_only`: the icons alone). Nine **presets**
  (classic, retro, minimal, floating, islands, pills, Mac style, fluent,
  powerline), which Settings applies by writing their keys into
  `navbar.conf`. The bar is opaque: no blur behind it (see
  `plan/decisions.md`). Font
  weight, widget gaps, app labels/tooltips, stat labels, formats and media text
  are configurable.
- **Animations** — workspace switches animate with a composited overlay:
  `slide`, `fade`, or `off` (`spring` is retired and reads as `slide`). Windows
  also **glide** to new slots (re-tile, open/close reflow, scrolling, resizes,
  drag drops): every window is placed instantly, and an overlay of live **DWM
  thumbnails** draws the motion over the wallpaper. A window that changes size
  morphs: position and size animate together, its image scaled in between and
  1:1 at both ends. Dimmed (unfocused) windows stay dimmed during the motion,
  and a window gaining or losing focus in it fades between the two levels.
  Another Astur hotkey pressed mid-glide re-aims it from where it is, at the
  speed it was moving; other keys and clicks reveal the result at once.
  Workspace slides and glides on a scrolling strip last at least 200 ms. At
  `animation_ms` up to 250 (default
  140) clicks pass straight through a running animation to those windows. A
  switch made while one is still sliding happens at once underneath it, and the
  overlay then reveals the result.
- **App launcher + file search** *(Astur)* — `Alt+Space` opens a fuzzy, MRU-ranked
  picker over installed apps (Start Menu and Store/UWP), custom commands/URLs, open
  windows, and indexed files. Optional prefix providers expose in-memory clipboard
  history and a curated emoji catalog. Inline calculator, configurable web fallback,
  file scope/excludes, result limit, placement, size, rows, font, colours, opacity,
  radius, icons, and mouse navigation are configurable. `Tab` shows Modified / Size /
  Path columns; `Shift+Enter` reveals a file; `F5` refreshes app/custom entries.
- **System menu** *(Astur)* — `Alt+Shift+Space` opens configurable Power and Setup
  categories with lock/sleep/hibernate/sign-out/restart/shutdown, settings, config,
  reload, restart-Astur, screenshot, and wallpaper actions. Order, width, icons,
  confirmation, custom categories, commands, URLs, and scripts are data-driven.
- **Desktop tools** *(Astur, opt-in)* — workspace-aware Alt+Tab picker, Alt+Grave
  scratchpad, clipboard/emoji launcher providers, workspace-triggered wallpaper,
  media title widget, active-workspace/MRU persistence, and local named-pipe IPC.
- **Settings GUI** *(Astur)* — Windows 11 style editor for shortcuts, layouts,
  workspaces, rules, appearance, motion, the status bar, launcher, power menu,
  desktop extras and integrations, with Ctrl+F search across every setting.
  Changes apply **as you make them** (no Save button; Ctrl+Z undoes) — the window
  manager hot-reloads, no restart. Home has a one-click **Quick setup** (theme,
  layout, bar style, animations, gap size) and a short how-to built from your
  own keys. Nothing needs typing: the terminal, browser and scratchpad app are
  picked from the ones installed (or any app, with the Windows file picker),
  fonts from the installed fonts with a sample, the web search from a list of
  sites, folders and pictures with Browse, and an app rule from a window that
  is open now; a value typed by hand stays editable under More options.
  Window rules, launcher entries, power-menu
  items, workspaces and extra hotkeys are tables (a row that isn't valid yet
  shows in red with the reason and is not written); shortcuts are set by
  pressing the key, with a warning when two share one, and a copyable cheat
  sheet. The Status bar page has a preset gallery with drawn previews, Mac
  style and Powerline included (Apply
  backs up `navbar.conf` to `navbar.conf.bak` first, once per session, and
  shows which preset your bar is closest to and what you changed), palettes
  with swatches, shape, separators, hover highlight and font (warning when
  the font is not installed), and a drag-and-drop editor for the left /
  centre / right zones: dropping a widget into a zone shows it, dragging it
  out hides it, and clicking one opens its settings. Formats are picked by
  what they show (`14:05`, `2:05:09 PM`, `Sun 4 Oct`, App name, `ENG-US`;
  Custom… types one), the disk widget's drive from the drives on the PC, and
  the custom widget's command says plainly that it runs in the background.
  A Workspaces section sets the active-workspace style, labels (numbers,
  names, icons) and app icons in each mark; Click actions gives every widget
  on the bar a drop-down per mouse button ("Open the calendar (default)",
  "Go to workspace…" with a number, "Open an app or command…" with the app
  picker); Behaviour picks which screens show the bar (all, the main one, or
  ticked screens). Troubleshooting lists config lines it did not understand,
  with a did-you-mean fix.
- **Tray icon** *(Astur)* — no console window; left-click for Settings, right-click for
  Settings / Quit.
- **Light / dark theme** *(Astur)* — the launcher and menus follow `theme = dark |
  light | auto` (auto tracks the Windows app theme); optional experimental acrylic
  blur behind the popups.
- **High-DPI aware** — per-monitor DPI v2. Tiles fill the whole work area at any
  display scale, and the bar, launcher and menus are scaled per monitor, so a
  100% screen next to a 150% one both look right. Sizes in the config are
  logical pixels at 100%.
- **Extras** — coloured borders, unfocused dimming, focus-follows-mouse, rich
  exe/class/title window rules with workspace/monitor routing, extra shortcuts on
  the main key, Alt or Win, and live config hot-reload.

**The main key.** Every Astur shortcut and mouse move / resize is held with one
main key, `Alt` in the tables below. It is **Left Alt** unless you pick another in
Settings (or `main_key` in `astur.conf`):

| `main_key` | What it means |
|---|---|
| `lalt` (default) | Left Alt, reserved: apps never see it. Left Alt plus a key Astur does not use does nothing (typing, editing and navigation keys, the numpad and F1-F12 are swallowed; volume, media, PrintScreen and F13-F24 still work). Right Alt is untouched; use it for app Alt shortcuts (Alt+F4, menus). Alt+Tab passes through by default; optional replacement uses Astur's workspace-aware picker. |
| `win` | The Windows key. Apps and Windows keep it; only Astur's shortcuts are taken, and they replace Windows' own (Win+D, Win+1...). Start does not pop up after a Win shortcut or Win+drag. Win+L always locks the PC, so grow-master moves to `Win+O` (unless another of your shortcuts already uses O). |
| `alt` | Either Alt, GlazeWM-style. Apps keep their Alt shortcuts except the ones Astur uses; menu bars do not pop up after an Astur shortcut. |
| `ctrl+win` | Ctrl and the Windows key held together. |
| `capslock` | Caps Lock becomes the main key and no longer turns capitals on (turn it off first). Pressed where Astur can't see it (an admin window, the lock screen, a UAC prompt, or before Astur starts) it still turns capitals on; press it there again (Win+L locks the PC), or use the on-screen keyboard. |

Right Alt and Ctrl+Alt are not offered: on German, French and similar keyboards
they type characters (AltGr). Win+L and Ctrl+Alt+Del always belong to Windows.

In Settings, **Shortcuts > Astur key** shows each choice with the Windows
shortcuts it would take over with your shortcuts. A new key works at once and
goes back by itself after 15 seconds (or when Settings is closed) unless you
press **Keep**, so a key that doesn't work on your keyboard can't lock you out.
Custom shortcuts written with `ALT` (the scratchpad's `ALT+GRAVE`) move to the
new key as `MOD`. Every label, the cheat sheet and the conflict notes on each
shortcut follow the key you picked.

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
| `Alt` + `Tab` | Windows switcher by default; optional Astur workspace-aware switcher (Left Alt main key) |

`Alt` here is the main key (above): with `main_key = win` read `Win` + `J`, and so on.

On a monitor running the scrolling layout, the arrows, `J K H L M` and a few new
keys work on columns instead — see [Scrolling layout](#scrolling-layout).

Letter binds (`J K H L M T F W`, and `R C [ ]` for scrolling) plus extra
chords (`MOD` = the main key, with `CTRL`, `SHIFT`, `ALT`, `WIN`) are configurable
in `astur.conf` (see [Configuration](#configuration) for where), along with workspace keys, gaps, layout,
borders, rules, launcher, menus, and desktop tools. Status bar uses `navbar.conf`
in same folder. Both files **hot-reload** on save.

## Scrolling layout

`layout = scrolling` (or an entry in `monitor_layouts` or `workspace_layouts`) lays windows out
like [Niri](https://github.com/YaLTeR/niri): **columns** on an endless horizontal
strip, with the view scrolling along it. Each workspace on each monitor keeps its
own strip and scroll position.

- **Columns** — widths are fractions of the screen, so several are on screen at
  once (two at 1/2, three at 1/3). A new window opens as a new column right of the
  focused one at `scroll_default_width` (0.5). `Alt+R` cycles
  `scroll_width_presets` (1/3, 1/2, 2/3, full), `Alt+H` / `Alt+L` step by 0.1,
  `Alt+M` toggles full width.
- **Stacking** — `Alt+[` / `Alt+]` pull the window into the column on the left /
  right, or push it back out into its own column if it already shares one. Windows
  in a column split its height; `Alt+Shift+R` cycles `scroll_height_presets`.
- **Tabbed columns** — `Alt+Shift+T` shows a column's windows as tabs, one at a
  time under a tab bar (`tab_bar_height`): app icon and title per tab, click to
  switch, Alt-drag the bar to move the column.
- **Peek slivers** — while more columns lie past a screen edge, a sliver of the
  next one shows there (`scroll_peek`, 32 px by default, scaled with the display;
  about twice that at an end of the strip; 0 = columns edge to edge), dimmed like
  any unfocused window: click it to bring that column in (Astur takes that click,
  so it never lands on the app's own buttons under the sliver). Hovering never
  scrolls. At an end of the strip the last column sits at the normal outer gap,
  no extra band, and columns that fit on screen fill it edge to edge (2.5.0).
- **Peek and park** — a column partly past the screen edge **peeks** in only where
  no other monitor is on that side (beside another monitor it parks and that
  edge shows wallpaper, about twice the peek wide at an end of the strip). Otherwise, and when fully off screen, it is
  **parked** outside the whole virtual screen, so it never bleeds onto a
  neighbouring monitor and multi-monitor setups work. Parked windows on the current
  workspace keep their taskbar button and Alt+Tab entry: activating one scrolls it
  into view. Quit, a relaunch after a crash, turning tiling off, or switching the
  workspace to another layout bring every parked window back on screen (Quit
  brings peeking slivers fully on screen too).
- **View** — `scroll_center = never` (default) scrolls only as far as needed to
  show the focused column, `always` keeps it centred, `on-overflow` centres it only
  when it does not fit. `Alt+C` centres once. Columns that fit on screen together
  sit centred (2.5.0; before, they hugged the left edge).
- **Motion** — scrolls, column moves and reflows glide through the DWM-thumbnail
  overlay (see Animations). A parked window that was hidden or minimised has no
  DWM image to show, so it glides in from a background snapshot when one is ready,
  and otherwise appears at the end of the glide.

| Shortcut (scrolling monitor) | Action |
|---|---|
| `Alt` + `Left` / `Right` | Focus column left / right (at the strip end: next monitor) |
| `Alt` + `Up` / `Down` | Focus window up / down in the column (tabbed: previous / next tab) |
| `Alt` + `Shift` + `Left` / `Right` | Move column (at the strip end: window to next monitor); `scroll_move = stack` moves only the window, see below |
| `Alt` + `Shift` + `Up` / `Down` | Move window up / down in its column (tabbed: reorder tab) |
| `Alt` + `J` / `K` | Focus next / previous window |
| `Alt` + `Shift+J` / `Shift+K` | Move column right / left (`scroll_move` too) |
| `Alt` + `H` / `L` | Column width − / + 0.1 |
| `Alt` + `Shift+H` / `Shift+L` | Window height − / + 0.1 within its column |
| `Alt` + `R` / `Shift+R` | Cycle column width / window height presets |
| `Alt` + `M` | Toggle full-width column |
| `Alt` + `C` | Centre the focused column |
| `Alt` + `[` / `]` | Pull window into the left / right column, or push it out |
| `Alt` + `Shift+T` | Toggle column between stacked and tabbed |
| `Alt` + `Home` / `End` | Focus first / last column (`Shift`: move column there) |
| `Alt` + wheel | Focus column, wheel down = right (`Shift`: move column), on the monitor under the cursor; the pointer stays put (unless focus-follows-mouse is on) |
| `Alt` + horizontal wheel / touchpad swipe | Focus column left / right: a short swipe one column, a longer one several |
| `Alt` + left-drag drop | Left / right quarter of a window = new column beside it; middle = stack into its column; in the gap between two columns = between them |
| `Alt` + right-drag | Side edge = column width; top / bottom edge = window height |

`scroll_move = stack` (Settings > Layout > Scrolling layout) makes the move-column
keys (`Shift` + `Left` / `Right`, `Up` / `Down` on vertical scrolling,
`Shift+J` / `Shift+K`, `Shift` + wheel,
`move_column_left` / `move_column_right`) move just the focused window, one
step a press: it stacks into the next column, the next press takes it into a
column of its own past that one, and so on (the `[` / `]` move, repeated). At
the strip end it goes to the next monitor as before. `Home` / `End` still move
the whole column. Default `column` swaps whole columns. A column width does
not travel with the window: once it leaves a column it gets the default width.

On other layouts `R`, `C`, `[`, `]`, `Home` and `End` do nothing, and
`Alt+Shift+T` no longer toggles tiling. The same actions are available to
`extra_hotkeys` and IPC: `focus_column_left`, `focus_column_right`,
`move_column_left`, `move_column_right`, `cycle_width`, `cycle_height`,
`center_column`, `consume_left`, `consume_right`, `toggle_tabbed`,
`maximize_column`, `first_column`, `last_column`.

### Vertical scrolling

`layout = scrolling_vertical` (2.5.0) is the same strip turned on its side:
full-width **rows** from top to bottom, the view scrolling up and down. A row's
windows sit side by side (or as tabs, with the tab bar on top). A strip of rows
that fits on screen sits at the top; the peek slivers are at the top and
bottom edges. The size keys mean "size along the strip": `scroll_default_width` and
`scroll_width_presets` are row heights, `scroll_height_presets` a window's share
of its row's width. With `scroll_center = always` the focused row stays in the
middle of the screen, like a slot machine. On a landscape screen, half-height
rows (the default 0.5) suit most apps; an app with a minimum height taller than
its row overlaps the next row.

| Shortcut (vertical scrolling) | Action |
|---|---|
| `Alt` + `Up` / `Down` | Focus row above / below (scrolls) |
| `Alt` + `Left` / `Right` | Focus window left / right in the row (tabbed: previous / next tab) |
| `Alt` + `Shift` + `Up` / `Down` | Move row up / down (`scroll_move = stack`: moves only the window, see above) |
| `Alt` + `Shift` + `Left` / `Right` | Move window left / right in its row (tabbed: reorder tab) |
| `Alt` + `H` / `L`, `R`, `M` | Row height − / +, cycle, full height |
| `Alt` + `Shift+H` / `Shift+L`, `Shift+R` | Window's share of the row's width |
| `Alt` + `[` / `]` | Pull window into the row above / below, or push it out |
| `Alt` + wheel | Focus row, wheel down = next row (`Shift`: move row) |
| `Alt` + left-drag drop | Top / bottom quarter of a window = new row above / below; middle = join its row |
| `Alt` + right-drag | Bottom edge = row height; side edge = window's share of the row |

At the end of a row, `Alt+Left` / `Right` (with `Shift`: the window) go on to
the next monitor; `Alt+Up` / `Down` stop at the strip's ends. `Alt+C`, `Alt+Shift+T`, `Home` / `End` and the
`extra_hotkeys` / IPC actions work as on `scrolling`, with rows for columns.
The bar shows `vertical 2/5`.

### Per-monitor and per-workspace layouts

`layout` is the default; `monitor_layouts` overrides it by monitor number (1 =
leftmost, the same numbering as window rules). An empty entry or `default`
inherits `layout`, so `monitor_layouts = scrolling, , master` runs scrolling on
monitor 1 and master on monitor 3. `workspace_layouts` overrides that by workspace
number, so `workspace_layouts = , scrolling, grid` runs scrolling on workspace 2 and
grid on workspace 3; an empty entry or `default` keeps its monitor's layout.

A runtime layout switch (`extra_hotkeys` action `layout` with a layout name,
`next` or `previous`, e.g. `ALT+CTRL+S|layout|scrolling` or
`ALT+CTRL+L|layout|next`; IPC `layout <name|next|previous>`; a bar click set to
`next_layout`, e.g. `layout_click = next_layout` in navbar.conf or Settings > Status bar)
changes the **workspace you are on only**, until a config reload
resets it to the file. `next` / `previous` step through the layouts in the order
Settings lists them, wrapping around. (Before 2.5.0 a switch changed the whole
monitor.) The bar's layout widget shows the layout of the workspace each monitor
shows, plus the focused column on scrolling ones (`scrolling 2/5`, or
`vertical 2/5` for rows).

Gaps work the same way. `outer_gap` takes one value or four (top, right,
bottom, left), e.g. `outer_gap = 8, 8, 60, 8` for an auto-hide taskbar at the
bottom, and `monitor_outer_gaps` overrides it by monitor number. Commas split
the monitors, so one monitor's four values go with spaces:
`monitor_outer_gaps = 16 16 48 16, default`.

## Configuration

Two files are created in `%APPDATA%\Astur\` on first run, both fully
commented and **hot-reloaded on save** (about 0.1 s after the save). The
folder also holds the log, state and the default `wallpapers` folder.
`config_folder = home` in `astur.conf` (Settings > Troubleshooting > Config
folder) keeps it in `%USERPROFILE%\.astur\` instead. Astur moves the whole
folder to the chosen place when it starts, so an install from before 2.5.0
moves from `.astur` to `%APPDATA%\Astur` on its first start. It never merges:
if a folder is already at the target, nothing moves until you rename or
delete that one. Nor while `ASTUR_CONFIG` or `ASTUR_NAVBAR` points inside
the folder (the log and Settings say so); point overrides outside both
Astur folders. Wallpaper paths inside the old folder follow the move;
paths you typed into commands or launcher entries do not. Astur Lite and
builds before 2.5.0 read only `%USERPROFILE%\.astur\`: set
`config_folder = home` and restart Astur before switching to one. A reload
redoes only what the save changed: a colour edit does not re-tile or restyle
windows. The system menu's **Reload** (or IPC `reload`) always re-applies
everything, which also repairs any window left mis-styled:

- **`astur.conf`** — workspaces/names/icons/wallpapers, seven layouts and per-monitor
  and per-workspace overrides (`monitor_layouts`, `workspace_layouts`), scrolling (`scroll_default_width`,
  `scroll_width_presets`, `scroll_height_presets`, `scroll_center`, `scroll_peek`,
  `tab_bar_height`), gaps, borders, focus, animations, popup theme/geometry,
  launcher providers/custom entries, system actions, desktop tools, rich rules,
  persistence, IPC, and hotkeys (scrolling keys: `key_cycle_width`,
  `key_center_column`, `key_consume_left`, `key_consume_right`).
- **`navbar.conf`** — bar placement/style (`shape`, `palette`, `workspace_style`,
  `font_weight`, `border_width`, `island_padding`, `widget_colors`), three widget
  zones, labels/formats/icons (`icon_mode = glyph` for Windows icons), app
  buttons/tooltips, stats, volume, network, media, floating mode, and auto-hide;
  `<widget>_click` / `_right_click` / `_middle_click` actions, `monitors`,
  `workspace_label`, `workspace_apps`, `separator_style`, `hover`,
  `title_format`, `title_bold`, and the keyboard / disk / uptime / custom
  widgets' keys (`disk_drive`, `custom_command`, `custom_interval`, formats).
  A bar preset in Settings writes these keys (look, clock/date formats and
  zones) and turns Astur's bar on (`enabled = true`); it never writes
  `bottom`, `autohide` or the wheel / hide-empty behaviour. The look includes icon mode, workspace style, widget colours,
  separator style, hover and the title's format and weight.
  A `shape` line overrides `floating`.

Save both as UTF-8. The window manager never overwrites a file it cannot read (a
UTF-16 "Unicode" save, or one locked mid-save): it keeps its current settings
(built-in defaults at startup), logs the error to `astur.log`, and tries again.

Full Astur ships **`astur-settings.exe`** to edit both files. `.conf` remains source
of truth; comments/layout survive saves. Structured records use `;;` between rows
and `|` between fields. Prefix literal separators with backslash (Settings edits
these as tables and escapes them for you). Built-in icon names:
`app browser calculator clipboard command file folder grid lock media power reload
power-circle restart screenshot search settings setup signout sleep terminal wallpaper web window`.

Commands Astur runs (`terminal`, `browser`, `scratchpad_command`, the `launch`
hotkey action, IPC `launch` and `cmd:` targets) read like cmd.exe command lines:
a plain one (an exe or bare program name and its arguments, a folder, a URL)
is opened directly the way Win+R opens it; one with cmd syntax or `%` pairs,
or whose program is a script or document (`.bat`, `.cmd`, `.txt`...), runs as
`cmd /C start "" <command>`, so a `.bat` keeps its window open as before. A
bare name that resolves to a script through PATH closes when it finishes:
write its extension to keep it open.
PATH and App Execution Aliases resolve and arguments work, e.g.
`terminal = wt -p Ubuntu`. Quote a path with spaces (or
whose `%VARIABLES%` expand to spaces, like a `C:\Users\John Smith` profile) when
arguments follow it: `"C:\Program Files\App\app.exe" --flag`. A bare path to an
existing file or folder works unquoted, `%VARIABLES%` included. cmd characters
(`& | < > ^ %`) work as in cmd, so quote a URL that contains `&`; inside `|`/`;;`
records write a pipe as `\|`.

## Build from source

Requires [Rust stable](https://rustup.rs). The repo is a Cargo workspace.

```bash
git clone https://github.com/Page011/Astur
cd Astur
cargo build --release
# binaries: target/release/astur.exe + target/release/astur-settings.exe
```

`cargo build --release -p astur` builds core WM only. Settings menu then reports
missing companion instead of failing silently.

`crates/astur` is window manager, `crates/astur-config` shared config parser,
`crates/astur-settings` settings GUI. Minimal **Astur Lite**
lives on the [`lite`](https://github.com/Page011/Astur/tree/lite) branch (a single
crate, no workspace).

## YASB integration

Use YASB's existing GlazeWM workspace widget with Astur's optional localhost
WebSocket server: live workspace indicators, app icons, click-to-switch and wheel
cycling, including per-monitor workspaces. Open **Settings > Integrations**
to enable it, choose a port, and copy the widget configuration and example CSS.
Alternatively, set `yasb_enabled = true` in `astur.conf` and connect the widget
to `ws://127.0.0.1:6123`.
See [setup and ready-to-merge YASB configuration](integrations/yasb/README.md).

## How it works

Astur installs two low-level Windows hooks (`WH_MOUSE_LL`, `WH_KEYBOARD_LL`) that
intercept input before it reaches any application. With the default main key, Left
Alt is swallowed so it never triggers app menus or Alt shortcuts — only Astur sees it.
A Left Alt chord Astur does not bind is swallowed too (typing, editing and navigation
keys), so the app never receives it as the bare key. With a Win, Alt or Ctrl+Win main
key the modifier reaches apps and only Astur's own shortcut keys are swallowed; Astur
then sends an unassigned key (0xE8) so Windows does not open Start, nor an app its menu
bar, on the release. Every shortcut is one lookup in a precomputed table: no lock on
the input path. Drag commands are queued to
the manager. Alt-drag moves and resizes the real window: no thumbnail, outline,
off-screen parking, or preview handoff. The manager samples the newest pointer
geometry at up to 120 Hz; repeated samples are skipped. Final drops run on the same
thread, preserving placement order. Tiling waits until an active drag ends.
Application repainting stays off the input hook. Tiles and live drag updates use
posted (`SWP_ASYNCWINDOWPOS`) moves,
so a re-tile finishes after the slowest app instead of after every app in turn, and
a busy app no longer holds up the next re-tile; `async_window_pos = false` in
`astur.conf` makes Astur wait for each app instead. Showing and hiding windows on a
workspace switch still waits for each app. File search queries the Windows Search
index off the input path, so typing stays responsive.

## Quit

- **Astur** — right-click the tray icon → **Quit** (restores all windows, then exits).
- **Astur Lite** — `Ctrl+C` in the console window.

## How Astur compares

| | Astur | komorebi | GlazeWM | FancyZones |
|---|---|---|---|---|
| Master-stack tiling | Yes | Yes | Yes | No (zones only) |
| Alt-drag move/resize anywhere | Yes | No | No | No |
| Animation scope | Window glide + workspace slide/fade | Movement (experimental) | Window movement | No tiling animation |
| App launcher + file search built in | Yes | No | No | No |
| Settings GUI | Yes | `komorebi-gui` companion | Tray controls + config file | Yes |
| Single portable exe option | Yes (Lite; Full WM-only option) | No (multiple binaries) | No (installer) | Part of PowerToys |
| Virtual workspaces | Yes | Yes | Yes | Via Windows |
| Config file required to start | No | Yes | Yes | No |
| Language | Rust | Rust | Rust | C++ core / C# editor |
| Licence | Apache-2.0 | Komorebi 2.0 personal-use source licence | GPL-3.0 | MIT |

See the full [comparison: Astur vs GlazeWM, komorebi & FancyZones](https://astur.app/compare).

## FAQ

**Is Astur a komorebi or GlazeWM alternative?** Yes — same master-stack tiling and
workspace idea, plus Alt-drag move/resize, an app launcher with file search, and a
power menu, with a single-exe (Lite) option and no required config file.

**Does it work on Windows 11?** Yes, on Windows 10 and 11, x64.

**Do I need admin rights?** No. Without them Astur leaves elevated windows (an admin
terminal, or Task Manager on an admin account) untiled, because Windows does not let a
normal process move or hide them. Run Astur as admin to tile those too. Then (with
UAC on) the apps Astur launches (hotkeys, launcher, bar, IPC) still start unelevated through
Explorer (an admin terminal needs a shortcut set to "Run as administrator"), the
custom bar widget is off, and IPC scripts must run elevated: Astur's config files can
be changed by any program you run, so they must not name what runs as admin.

**Does it work with display scaling (125% / 150%)?** Yes, from 2.1.3 on. Astur is
per-monitor-DPI aware, including mixed-scale multi-monitor setups. (Earlier
versions tiled into the top-left corner of a scaled screen — that was
[issue #5](https://github.com/Page011/Astur/issues/5).)

**Something is not working — what do I send?** Run `astur.exe --check` in a
terminal. It prints and saves a short report (version, Windows build, DPI
awareness, every monitor and its scale, config paths). For more, set
`log_level = info` in `astur.conf` and attach
`astur.log` from the same folder (the report shows its path).

**What's the difference between Astur and Astur Lite?** See [Editions](#editions) —
Lite is minimal ~1 MB console exe; Astur adds configurable launcher/search, system
menu, tray, desktop tools, rich widgets/themes/rules, IPC, persistence, and settings GUI.

### Current desktop-tool limits

- Workspace wallpaper changes Windows global wallpaper through
  `SystemParametersInfoW`; not independent per-monitor wallpaper.
- Window glide and the slide's still-wallpaper backdrop draw from a cached copy of
  the wallpaper, re-rendered about 1.5 s after it or the display changes, or a
  config change moves the work area or turns an animation on, or after an explicit
  Reload (system menu, tray, IPC `reload`), and once at startup. In
  that gap — e.g. right after a per-workspace wallpaper switch — windows place
  instantly instead of gliding and slides move the whole frame. Without Explorer (a
  replacement shell) that is always the case.
- Scrolling: a parked window that was hidden (its workspace was switched away) or
  minimised has no current DWM image, so it glides in from a snapshot taken in the
  background. Scrolled to right after the switch, before the snapshot exists, it
  appears at the end of the glide instead. Apps whose content PrintWindow cannot
  capture (some GPU-rendered windows) may glide in black.
- Media widget reads titles from known player windows; no play/pause/skip controls or
  Windows media-session API yet.
- Persisted state stores active workspace indexes and launcher MRU, not window-to-
  workspace assignments across process restarts.
- Astur Alt+Tab replacement shows title/app-icon rows; live DWM thumbnails remain
  future work.
- Clipboard history is text-only, memory-only, and cleared when Astur exits.

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

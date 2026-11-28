# Making RSB yours

RSB draws your status bar from three files you can edit. Save any of them and
the bar changes at once (well under a tenth of a second); a mistake shows on the
bar as a red plate with the file and line, and the last good version stays.

| Door | File | For |
|---|---|---|
| Layout | `bar.toml` | which bars, which monitors, which widgets where, widget options |
| Look | `style.css` | colours, fonts, spacing, hover effects, animation of colours |
| Behaviour | `widgets\*.luau` | your own widgets, or a changed copy of a built-in one |

The config folder is `%APPDATA%\rsb` (or the folder given with `--config-dir`).
A start writes whichever of these is missing, and never touches one that is
there: a short, commented `bar.toml`, an empty `style.css` (comments only), an
empty `widgets\` folder and the editor files described in
[Editor setup](#editor-setup-vs-code-and-luau-lsp). With no files at all you get the
`default` bar: chevron-shaped segments hanging from the top edge, workspaces and
media on the left, memory, CPU, a button, the clock and the date in the middle,
status, battery, tray and a record button on the right.

## 1. bar.toml

Every key is optional. An unknown key, a value of the wrong type or a value out
of range is a warning with its line (`bar.toml:4:9 unknown key 'heigth' in
[[bar]]; did you mean 'height'?`) and that key falls back to its default. A
syntax error keeps the previous file and shows the error. A `[[bar]]` key
written without its `[[bar]]` line above it is not at the top level
(`unknown key 'height' at the top level; it belongs under [[bar]]`): in the
commented template, uncomment the `[[bar]]` line together with the lines you
change.

```toml
config_version = 1          # must be 1 when present
preset = "default"          # default | fluent | astur-classic | glass

[[bar]]                     # zero or more; none written = the preset's bars
name = "main"               # a-z 0-9 _ -, at most 32; also the name for: rsb toggle main
monitors = "*"              # "*", "primary", 0, "DISPLAY2", or a list of these
position = "top"            # top | bottom
height = 36                 # logical px, 16 to 128
floating = false            # true: a rounded bar kept away from the screen edge
margin = 8                  # floating only, 0 to 64
radius = 0                  # 0 to 64; 10 when floating; a CSS border-radius wins
reserve = true              # keep maximised windows clear of the bar
fullscreen = "below"        # below | hide | stay, what to do over a fullscreen app
left = ["workspaces", "media"]
center = ["memory", "cpu", "dashboard", "clock", "date"]
center_on = "dashboard"     # this widget of `center` sits in the middle of the bar
right = ["status", "battery", "tray", "record"]

[widget.clock]              # options for every "clock"
format = "HH:mm"

[widget."clock#utc"]        # a second clock with its own options (quotes needed)
format = "HH:mm 'UTC'"
timezone = "utc"

[widget."my/weather"]       # the file widgets\my\weather.luau
latitude = -30.51
longitude = 151.67
```

| Key | Where | Meaning |
|---|---|---|
| `config_version` | top | the file format; `1` |
| `preset` | top | the look and bar layout underneath your files |
| `bar` | top | the list of `[[bar]]` tables |
| `widget` | top | the `[widget."<ref>"]` option tables |
| `name` | bar | the bar's name (a CSS class, and the argument of `rsb toggle`) |
| `monitors` | bar | which monitors get this bar |
| `position` | bar | `top` or `bottom` |
| `height` | bar | thickness in logical pixels (scaled by the monitor's DPI) |
| `floating` | bar | detach the bar from the edge |
| `margin` | bar | gap to the edges when floating |
| `radius` | bar | corner radius when floating |
| `reserve` | bar | register as an appbar so maximised windows stay clear |
| `fullscreen` | bar | `below` drops under a fullscreen window, `hide` hides, `stay` stays on top |
| `left`, `center`, `right` | bar | widget refs in each zone |
| `center_on` | bar | a widget ref of `center` whose middle is the middle of the bar; the zone is laid out around it and moved only as far as the left and right zones need. Not in `center`: a warning, and the zone is centred as a whole |

**Widget refs.** `clock` is the built-in or a file `widgets\clock.luau` (your
file wins). `clock#utc` is a second instance of `clock`: the plain
`[widget.clock]` table applies first, then `[widget."clock#utc"]`. `my/weather`
is always the file `widgets\my\weather.luau` (or `widgets\my\weather\init.luau`).
The names `left`, `center` and `right` are not allowed (they are zone names).
An unquoted `[widget.clock#utc]` is a TOML error; RSB tells you to quote it.

**Layering, lowest first:** built-in defaults, the preset's own `bar.toml`, your
file. A `[[bar]]` list of yours replaces the preset's list, but your bar N starts
from the preset's bar N (or its last bar), so `height = 36` under
`preset = "glass"` keeps the floating look and the zones. Widget option tables
merge key by key.

**Monitors.** `"*"` every monitor (one bar each), `"primary"`, a number (Windows'
order, from 0), a device name (`"DISPLAY2"`), or a list. A selector that matches
nothing is a warning and that bar is not shown. Two bars on one monitor and edge
stack in list order; a top bar and a bottom bar is two `[[bar]]` tables.

## 2. Built-in widgets

All are plain Luau files written against the public API (see
[Eject](#eject-change-a-built-in-widget)). Every widget puts the class `stale` on
its own root element and shows `--` when its data source has gone quiet, never an
old value as if it were fresh. A widget with nothing to show says so honestly:
workspaces draws nothing without a window manager, media hides without a player,
battery shows a plug on a PC without a battery.

| Widget | Options (default) | Behaviour |
|---|---|---|
| `workspaces` | `monitor = "own"`, `hide_empty = false`, `label = "display_name"`, `wheel = true`, `style = "pills"`, `active_format = "{label}"` | one pill per workspace from Astur, GlazeWM or komorebi; a click focuses, the wheel steps; `own` shows the bar's monitor (matched by handle, then device name, then position; no match shows all), `all` shows every monitor; `label` is `display_name` or `name`; `style = "text"` draws plain labels (the root gets the class `plain`), and `active_format` is how the focused label reads (`"-{label}-"` gives `-8-`). Needs `control = workspaces` |
| `title` | `max_width = 360`, `show_process = false` | the focused window's title with an ellipsis; empty on the desktop. Needs `read = window` |
| `clock` | `format = "HH:mm"`, `alt_format = "ddd d MMM HH:mm:ss"`, `hover_format = ""`, `timezone = "local"` | a click swaps to `alt_format` (set it to `""` to turn that off); `hover_format` is what the clock shows while the pointer is over it (`"HH:mm:ss"`), and the seconds only tick while it does; ticks on the boundary of the format's finest unit, and at once after sleep or a clock change |
| `date` | `format = "ddd d MMM"`, `hover_format = ""`, `timezone = "local"`, `popup = false` | `hover_format` is the long form shown while hovered (`"dddd d MMMM, yyyy"`); ticks once an hour. `popup = true` makes a click open a month calendar (the popup named `calendar`: today in the accent colour, the wheel and the arrow buttons change the month) |
| `cpu` | `format = "{value}%"`, `icon = "cpu"`, `icons = "font"`, `label = ""`, `every = "2s"`, `warn = 80` | class `warn` from `warn` percent up; a non-empty `label` replaces the icon; `icons = "svg"` draws the bundled SVG icon named by `icon` instead of the icon font |
| `memory` | `format = "{value}%"` (`{value}` `{used}` `{total}`), `icon = "memory"`, `icons = "font"`, `label = ""`, `every = "2s"`, `warn = 85` | as `cpu` |
| `battery` | `format = "{value}%"` (`{value}` `{time}`), `hover_format = ""`, `unknown = "--:--"`, `icons = "font"`, `label = ""`, `absent = "icon"` | icon by level; classes `charging`, `low` (15% or less on battery), `saver`, `absent`; `absent` is `icon`, `text` or `hide`; `{time}` is the time left as `h:mm`, or `unknown` when Windows reports none (charging, full); `hover_format` replaces `format` while hovered (`"{time}"`); `icons = "svg"` uses the bundled battery icons |
| `volume` | `format = "{value}%"`, `muted_format = ""`, `label = ""`, `step = 2`, `popup = false` | the wheel changes the volume by `step` percent per notch, a click toggles mute; `popup = true` makes the click open the mixer instead (the popup named `volume`: a master slider, then one slider row per output device, scrolling after six); `muted_format` (say `"MUTE"`) replaces `format` while muted; classes `muted`, `absent` (no output device). Needs `control = audio` |
| `network` | `format = ""` (`{ssid}` `{down}` `{up}` `{kind}`), `every = "2s"`, `popup = false` | an icon by connection kind and Wi-Fi strength; add a `format` for text; classes `offline`, `no-internet`. Rates are only read when the format uses them. `popup = true` makes a click open the Wi-Fi list (the popup named `network`: networks in range with their signal and a lock mark, a scan button; a click on a saved network connects and on the connected one disconnects). While Windows keeps network names from desktop apps (location is off for them) the list says so and offers a button to Windows' location settings. Needs `read = network`, `read = wifi`, `control = wifi` |
| `media` | `format = "{artist} - {title}"` (`{album}` too), `max_width = 280`, `hide = "idle"`, `progress = false`, `icon = "glyph"`, `popup = false` | an icon for what the player is doing now and the track; a click plays or pauses (with `popup = true` it opens the player instead, the popup named `media`: cover, title, artist, a seek slider and previous / play-pause / next buttons), a right click skips; `hide` is `idle` (hidden with no session) or `never`; a paused track still shows. `progress = true` draws a line along the bottom, as long as the played part of the track (it moves once a second while playing). `icon = "app"` leads with the player's own icon (the `music` icon when Windows has none) and draws the glyph of the click, pause while playing. Needs `control = media`, `read = media` |
| `status` | `mic = true`, `bluetooth = true` | a microphone icon, a Bluetooth icon and the battery of the first connected Bluetooth device that reports one. Both icons are buttons: the mic toggles the mute of the default recording device, the Bluetooth icon turns the radio on or off (each from a click only). A muted mic and a radio that is off are drawn dim (class `off`), a mic an app records from has the class `live`. The segment is a narrow sliver until the pointer is over its group. Needs `control = bluetooth`, `control = microphone`, `read = bluetooth` |
| `tray` | none | an empty segment the width of a sliver that never grows while it holds no icons; RSB has no tray icons yet |
| `record` | `command` (none) | a round button. With no `command` it opens Windows' own screen recorder; `command = ["obs64", "--startrecording"]` (a list, or a string split by the Windows argument rules) runs that program instead and grants only it. Needs `control = apps` |
| `dashboard` | `record_command` (none) | the button in the middle of the bar: an accent hexagon with the RSB mark; a click opens or closes the panel (the popup named `dashboard`; the name `launcher` opens the same panel on the apps page with the search focused). The panel has six tiles (power mode, Bluetooth, audio output, apps, record the screen, power menu) and a page for each: the Windows power schemes; paired Bluetooth devices with connect, disconnect and forget; every output as a volume slider (drag along a row to set its level, click it to make it the default output); and the app launcher (type to search, typos forgiven; Up and Down move, Enter starts the app). Escape goes back to the tiles from a page (the tile it came from has the focus) and closes the panel from the tiles and from the apps page. The record tile opens Windows' screen recorder, or runs `record_command` (a list or a string, like the `record` widget's `command`) and grants only that program. Needs `control = power, bluetooth, audio, apps` and `read = bluetooth, apps` |
| `power` | none | the power menu, a popup named `power` that covers the monitor: power off, restart, sleep and lock as four chevron buttons, Lock focused at the start. List it in a zone of `bar.toml` (it draws nothing there) and open it from the dashboard or with `rsb popup power`. Lock fades to a dark curtain and then asks Windows to lock the PC; RSB never draws a lock screen of its own. If Windows refuses an action the reason shows under the buttons. Needs `control = power, session` and `overlay = true` |
| `text` | `text = ""`, `exec` (none), `every = "30s"`, `format = "{output}"`, `click` (none), `class = ""` | your own widget from `bar.toml` alone: fixed text, or the first line a command prints |

Examples:

```toml
[widget.cpu]
label = "CPU"
warn = 70

[widget.volume]
step = 5

[widget."clock#tokyo"]
timezone = "+09:00"
format = "HH:mm 'Tokyo'"

[widget.text]
exec = ["git", "-C", "C:/work/project", "branch", "--show-current"]
format = "on {output}"
every = "10s"
click = "git -C C:/work/project fetch"
```

`exec` and `click` are either a list (`["git", "status"]`) or a string split by
the Windows argument rules, with no shell: `exec = "git status --short"` runs
`git` with two arguments, and `|`, `&` and `%VAR%` mean nothing. For shell
features write `exec = ["cmd", "/d", "/c", "dir | find /c \".luau\""]`; that
gives the widget the right to run `cmd`, which can run anything, and the grant
line in the log says so. The `text` widget may run only the programs you name in
`exec` and `click`: a default never grants anything.

The format fields use `{name}`; a name that does not exist is left as written so
a typo shows. Time formats are described under `rsb.time.format`.

## 3. Presets

| Preset | What it is |
|---|---|
| `default` | chevron segments hanging from the top edge in three colour steps and an accent, JetBrains Mono 14 px, SVG icons, plain workspace labels, the button in the middle centred on the bar (`center_on`), dark or light with Windows. The window is 36 px tall and draws nothing itself: the segments start 10 px down, so the wallpaper shows above them |
| `fluent` | full width, translucent, dark or light with Windows, Segoe UI 13 px, Fluent icons, accent pills for workspaces: the first default look, kept under its own name |
| `astur-classic` | Astur's own bar: height 28, Segoe UI at 14 px, Astur's dark (`#C0CAF5` on `#1A1B26`, accent `#66AAFF`) and light (`#1B1E24` on `#ECEEF2`, accent `#336382`) colours, semibold text, workspaces left, title in the middle, `cpu memory volume battery clock` right, text labels `CPU`, `RAM`, `VOL`, `BAT` instead of icons (`VOL MUTE` while muted), square 34 px workspace pills with no gap and no outline, 16 px between widgets, 8 px at the edges. Differences that remain: the dark palette is the base and the light one follows the Windows theme (Astur's own default is dark unless its theme setting says otherwise); the title is centred on the bar, not in the space left between the zones; Astur's layout indicator has no RSB widget and is left out. Astur's "inactive" colour is below 4.5:1 contrast, so empty workspace labels use a lighter or darker shade |
| `glass` | floating (margin 8, radius 12), acrylic backdrop with a translucent fallback colour, each zone a pill, no shadow on the bar (the window edge would cut it) |

Every preset keeps text at 4.5:1 contrast or better against what is behind it,
dark and light, over a black or a white wallpaper.

`fluent`, `astur-classic` and `glass` set custom properties on `:root` (`--bg`,
`--fg`, `--muted`, `--warn`, `--error`, `--pill`, `--hover`, `--press`,
`--accent`, `--accent-fg`). `default` sets the colours of its three segment steps
(`--seg0`, `--seg1`, `--seg2`, each with `-hover` and `-press` shades), the
accent (`--accent`, `--accent-fg`, `--accent-text` for text on a tinted row,
`--accent-line` for thin marks), the text colours (`--fg`, `--muted`, `--mem`,
`--dim` for an off state, `--placeholder`), and the colours popups use
(`--panel`, `--card`, `--input`, `--row`, `--row-selected`, `--row-hover`,
`--tile-hover`, `--scrollbar-thumb`, the power overlay's fills and dim), each
once for dark and once for light, and `--d`, the depth of a chevron point
(13 px). Change a colour in your `style.css` by setting the property there.

Every preset sets the same six accent tokens on `:root`, so one accent works on
all of them and survives a preset switch: `--accent` (fills, the focused
workspace), `--accent-fg` (text and icons on the accent), `--accent-hover` and
`--accent-press` (the accent while hovered and pressed), `--accent-text` (text
and icons in the accent colour) and `--accent-line` (rings and outlines). Where a
preset has no separate shade for one of them it is `var(--accent)`, so setting
`--accent` alone still changes everything.

If the colours block made by rsb settings is at the end of your style.css, change the accent there.

### Preset metadata: `[about]`

Each preset's TOML ends with an `[about]` table: its name for a gallery, the
colours worth tuning and every colour pair the preset draws, so contrast can be
measured without running the bar. Only a built-in preset takes it; in your own
`bar.toml` it is ignored with a warning.

```toml
[about]
title = "Glass"
description = "A floating acrylic bar with a rounded pill behind each zone."
schemes = ["dark", "light"]
tune = [{ var = "accent", label = "Accent" }, { var = "fg", label = "Text" }]

[about.contrast]
text = ["fg on bg+zone bg+zone+hover bg+zone+pill", "accent-fg on bg+zone+accent"]
icon = ["dim on row"]
line = ["accent-line on bg+zone bg+zone+pill"]
decorative = ["overlay-dim"]
```

The pairs are written under `[about.contrast]`.

| Field | Meaning |
|---|---|
| `title` | one line, 32 characters at most, different for every preset |
| `description` | one line, 100 characters at most |
| `schemes` | the schemes the sheet defines: `"dark"`, `"light"` or both. Each one defines every `tune` variable and every name used in `contrast` |
| `tune` | the colours a settings page may offer, in page order: `var` is a custom property of the preset written without `--`, `label` is 20 characters at most. `accent` is required |
| `contrast.text` | pairs that need 4.5:1: text and anything read like text |
| `contrast.icon` | pairs that need 3:1: icons, large text, the off state |
| `contrast.line` | pairs that need 3:1: rings, outlines and progress bars |
| `contrast.decorative` | colours that carry no text and are not measured (dimming layers, scrollbar thumbs) |

A pair line is `<colour> on <surface> <surface> ...`: names are custom
properties without `--`, and a surface written `a+b` is a stack of translucent
layers, bottom first (`bg+zone+pill`). A surface that is translucent is measured
over black and over white, and the worse result counts. A test checks that every
colour the preset's sheet draws with appears in a pair or in `decorative`.

### What is checked, and what is not

Contrast is measured for custom properties only. A colour set inside a rule
(`.item { color: #888888; }`) is not a token, belongs to no pair and is not
checked, so when you write one yourself, check it yourself. The colours rsb
settings derives from an accent (`--accent-fg`, `--accent-text` and the others)
are chosen so every pair of every built-in preset still passes; colours you set
by hand are not.

## 4. style.css

A small, fixed subset of CSS. The whole sheet is re-read on every save, so
deleting a line removes its effect at once.

### Elements and selectors

The bar is a tree: `bar` contains three `zone` elements (ids `left`, `center`,
`right`); each holds one `widget` element per widget (class = the widget's name,
with `/` written as `-`; id = the name plus `-instance`, so `clock#utc` is
`widget.clock#clock-utc`); inside is whatever the widget draws, made of `box`,
`text`, `icon` and `image` elements. `plate` is the error plate.

```text
selector-list := complex ("," complex)*
complex       := compound ((" " | ">") compound)*
compound      := ("*" | type | ":root")? ("." class | "#" id | pseudo-class)* pseudo-element?
type          := bar | zone | widget | box | text | icon | image | plate
              | popup | menu | menuitem | tooltip | input | slider | scroll
pseudo-class  := ":hover" | ":active" | ":focus" | ":focus-visible" | ":focus-within"
              | ":disabled" | ":checked" | ":open" | ":first-child" | ":last-child"
              | ":nth-child(" (an+b | odd | even) ")"
pseudo-element := "::placeholder" | "::selection"   (last in the selector, input only)
```

`:root` is another name for `bar`. Specificity is (ids, classes plus pseudo
classes, types); later rules win ties. Not supported, each reported with its
line and what to use instead: `!important`, `:not()` and other pseudo classes,
attribute selectors, `+` and `~`, `calc()` outside `polygon()`, `rotate()`,
`skew()` and `matrix()`, `z-index` (later siblings draw on top: reorder the
elements), `text-shadow`, `inset` shadows, units `pt` and `rem`.

`:first-child`, `:last-child` and `:nth-child()` count only the siblings that
take part in layout: a widget that draws nothing (no tree, or a root with
`visible = false`) takes no space and is not counted. That is how a preset gives
the ends of a group their shape whatever is in it:
`zone#left > widget:first-child` is the first widget that shows.
`:open` matches the element a popup is anchored to while the popup is open.

A pseudo-element rule (`input::placeholder`) takes `color`,
`background-color`, `opacity` and `font-style` only.

`@keyframes name { from { ... } 50% { ... } to { ... } }` defines an
animation (only animatable properties are read; each block may set
`animation-timing-function`), and the `animation` property runs it:
`animation: pulse 1s ease-in-out 3`. Durations are limited to 10 s and counts
to 1000. A widget can run one from Luau with the `animate` prop
(`rsb.animate`).

Classes the engine and the widgets provide: `bar.<name>`, `bar.floating`,
`bar.fullscreen` (a fullscreen app is in front and the bar `stay`s), `widget.stopped`
(the widget shows a plate), and on a widget's own root element `.stale` (set by
the widget), `.error` (set by RSB for ten seconds after a handler failed; the
widget keeps running), `.warn`, `.muted`, `.absent` and the others listed in the
widget table. Built-in widgets draw `.item` rows (`.item.button` when they react
to a click), `.value` and `.label` texts, and workspaces draw `.ws` pills
(`.focused`, `.visible`, `.occupied`, `.empty`).

Plates (`plate`) take colour, background, border and font family from your
rules and nothing else: `display`, `visibility`, `opacity` and the size do not
apply, so no theme can hide an error.

### Values

Units `px`, `%`, `em`, `ms`, `s` (a bare `0` is a length). Colours: `#rgb`,
`#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()`, `hsl()`, `hsla()` (commas or
spaces, optional `/ alpha`), the 148 CSS colour names, `transparent`,
`currentcolor`. `linear-gradient([<angle> | to <side>,] <color> [<%>], ...)`.
Easing: `linear`, `ease`, `ease-in`, `ease-out`, `ease-in-out`,
`cubic-bezier(x1, y1, x2, y2)`.

Layout is flexbox: every box is a flex container (`flex-direction`,
`justify-content`, `align-items`, `gap`, `flex-grow`, `flex-shrink`, sizes,
margins, padding). Each zone is laid out at its content width; the left zone
starts at the bar's content edge, the right zone ends at the other, the centre
zone is centred and shifted just far enough to clear the others. Spacing is
rounded to whole pixels so text and edges stay sharp at 125% and 150%.

### Custom properties and var()

```css
:root { --accent: #e5a50a; --gap: 6px; }
.item { gap: var(--gap); }
.ws.focused { background: var(--accent); color: var(--missing, #000000); }
```

`--name: value` can be set on any element and is inherited. `var(--name,
fallback)` works anywhere in a value, in shorthands too (`padding: var(--p)`),
and is resolved when the style is computed. A value that is invalid after
substitution, or an undefined variable without a fallback, makes that property
unset and gives one warning with both positions (`style.css:12:5 'padding' is
invalid after var(--p) (defined at style.css:3:3)`).

### Dark and light

```css
@media (prefers-color-scheme: dark)  { :root { --bg: #1a1b26; } }
@media (prefers-color-scheme: light) { :root { --bg: #eceef2; } }
```

The bar follows Windows' app theme and switches when it changes. No other media
query is supported.

### @import

```css
@import "colors.css";
```

For colours written by another tool (matugen, pywal). The path is relative to the
config folder, may not leave it, and `@import` lines must come before the first
rule. At most 8 files and 4 levels deep. Imported rules come before the file that
imports them, so your own rules win. Saving any `.css` file in the folder re-reads
the whole chain.

### Properties

| Property | Values | Inherited | Animates |
|---|---|---|---|
| `color` | `<color>` | yes | yes |
| `background` | `<color> \| <linear-gradient> \| none` | no | yes |
| `background-color` | `<color>` | no | yes |
| `opacity` | `<number 0..1> \| <percentage>` | no | yes |
| `border` | `<length> \|\| solid \| none \|\| <color>` | no | yes |
| `border-width` | `<length>{1,4}` | no | yes |
| `border-color` | `<color>` | no | yes |
| `border-radius` | `<length-percentage>{1,4}` | no | yes |
| `border-top-left-radius` | `<length-percentage>` | no | yes |
| `border-top-right-radius` | `<length-percentage>` | no | yes |
| `border-bottom-right-radius` | `<length-percentage>` | no | yes |
| `border-bottom-left-radius` | `<length-percentage>` | no | yes |
| `border-top` | `<length> \|\| solid \| none \|\| <color>` | no | yes |
| `border-right` | `<length> \|\| solid \| none \|\| <color>` | no | yes |
| `border-bottom` | `<length> \|\| solid \| none \|\| <color>` | no | yes |
| `border-left` | `<length> \|\| solid \| none \|\| <color>` | no | yes |
| `padding` | `<length>{1,4}` | no | yes |
| `padding-top` | `<length>` | no | yes |
| `padding-right` | `<length>` | no | yes |
| `padding-bottom` | `<length>` | no | yes |
| `padding-left` | `<length>` | no | yes |
| `margin` | `(<length> \| auto){1,4}` | no | yes |
| `margin-top` | `<length> \| auto` | no | yes |
| `margin-right` | `<length> \| auto` | no | yes |
| `margin-bottom` | `<length> \| auto` | no | yes |
| `margin-left` | `<length> \| auto` | no | yes |
| `gap` | `<length>` | no | yes |
| `flex-direction` | `row \| column` | no | no |
| `flex-grow` | `<number>` | no | no |
| `flex-shrink` | `<number>` | no | no |
| `justify-content` | `start \| end \| center \| space-between \| space-around \| space-evenly` | no | no |
| `align-items` | `start \| end \| center \| stretch` | no | no |
| `width` | `<length-percentage> \| auto` | no | yes |
| `height` | `<length-percentage> \| auto` | no | yes |
| `min-width` | `<length-percentage> \| auto` | no | yes |
| `min-height` | `<length-percentage> \| auto` | no | yes |
| `max-width` | `<length-percentage> \| none` | no | yes |
| `max-height` | `<length-percentage> \| none` | no | yes |
| `font-family` | `[<family-name> \| sans-serif \| monospace \| icons]#` | yes | no |
| `font-size` | `<length-percentage>` | yes | yes |
| `font-weight` | `normal \| bold \| <number 100..900>` | yes | yes |
| `font-style` | `normal \| italic` | yes | no |
| `font-variant-numeric` | `normal \| tabular-nums` | yes | no |
| `letter-spacing` | `normal \| <length>` | yes | yes |
| `text-align` | `start \| center \| end \| left \| right` | yes | no |
| `text-overflow` | `clip \| ellipsis` | no | no |
| `overflow` | `visible \| hidden \| auto \| scroll` (one value, or x then y) | no | no |
| `overflow-x`, `overflow-y` | `visible \| hidden \| auto \| scroll` | no | no |
| `corner-shape` | `round \| bevel`, one to four values (a bevel needs a radius) | no | no |
| `clip-path` | `none \| polygon([nonzero \| evenodd,] <x> <y>, ...)`, each coordinate a length, a percentage or `calc(<percentage> +/- <length>)`; 3 to 64 points | no | yes (same point count) |
| `transform` | `none \| translate(), translateX(), translateY(), scale(), scaleX(), scaleY()` in a list | no | yes |
| `transform-origin` | one or two of a length, a percentage, `left`, `center`, `right`, `top`, `bottom` | no | yes |
| `animation` | `<name> <time> [<easing>] [<time>] [<count> \| infinite] [<direction>] [<fill>] [<state>]`, a list | no | no |
| `animation-name` | `none \| <ident>#` | no | no |
| `animation-duration` | `<time>#` | no | no |
| `animation-timing-function` | `<easing>#` | no | no |
| `animation-delay` | `<time>#` (negative allowed) | no | no |
| `animation-iteration-count` | `<number> \| infinite`# | no | no |
| `animation-direction` | `normal \| reverse \| alternate \| alternate-reverse`# | no | no |
| `animation-fill-mode` | `none \| forwards \| backwards \| both`# | no | no |
| `animation-play-state` | `running \| paused`# | no | no |
| `scrollbar-width` | `auto \| thin \| none` | yes | no |
| `scrollbar-color` | `auto \| <color> <color>` (thumb, track) | yes | yes |
| `caret-color` | `auto \| <color>` | yes | yes |
| `accent-color` | `auto \| <color>` (the filled part of a slider) | yes | yes |
| `outline` | `<length> \|\| solid \| none \|\| <color>`, drawn outside the border box, following the corners (not for a `clip-path` box) | no | width, colour |
| `outline-offset` | `<length>` | no | yes |
| `pointer-events` | `auto \| none` (`none`: the element and what is inside it are skipped by the pointer) | yes | no |
| `box-shadow` | `none \| [<length>{2,4} <color>]#` | no | yes |
| `display` | `flex \| none` | no | no |
| `visibility` | `visible \| hidden` | yes | no |
| `transition` | `[<property> \| all] <time> [<easing>] [<time>]#` | no | no |
| `transition-property` | `[<property> \| all]#` | no | no |
| `transition-duration` | `<time>#` | no | no |
| `transition-delay` | `<time>#` | no | no |
| `transition-timing-function` | `<easing>#` | no | no |
| `cursor` | `default \| pointer \| text` | yes | no |
| `backdrop` | `none \| acrylic \| mica` | no | no |

`transition` takes `property duration [easing] [delay]`, a list is allowed
(`transition: background-color 120ms ease-out, color 120ms`). A size can also move to and from `auto`: with `.item { transition: width 200ms ease-out; }` a box that grows to fit new text, or opens a hidden part, slides to its new width instead of jumping. `backdrop` works
on the bar only; `acrylic` and `mica` need Windows 11 and show the bar's own
translucent background where Windows draws a flat colour. A box has one border
colour: two sides with different colours warn and the last one wins. Text that
does not fit a box with `text-overflow: ellipsis` is cut with `...`.
A `clip-path` polygon follows its fill rule both where it clips and where a pointer
hits it: with `evenodd` a polygon that crosses itself leaves its middle out.

Fonts: `font-family` takes quoted or bare names, `sans-serif`, `monospace` and
`icons` (Segoe Fluent Icons, or Segoe MDL2 Assets on Windows 10). A family that
is not installed is a warning on the bar and Segoe UI is used instead. A bar's
box shadow is cut off by its window and warns.

`"JetBrains Mono"` is built in: RSB registers it at start, so a rule can name
it without installing it. To use a font file of your own, put it in the `fonts\`
folder of the config folder and name it in `@font-face`:

```css
@font-face { font-family: "Mine"; src: url("fonts/mine.ttf"); font-weight: 400; }
bar { font-family: "Mine", monospace; }
```

`src` takes one `url()`, relative to the config folder and inside `fonts\`
(`"mine.ttf"` means the same), with an optional `format("truetype")` or
`format("opentype")`; `font-weight` is a number or a range for a variable font,
`font-style` is `normal` or `italic`. Files are `.ttf`, `.otf` or `.ttc`, at
most 16 MiB each and 16 files. A missing file, a file outside `fonts\`, one
that is too big or not a font is a warning at the rule, never a silent
fallback. Saving a file under `fonts\` loads it at once.

Icons: `rsb.icon { svg = ... }` draws an SVG icon in the text colour. The
bundled ones are named in [Icon names](#icon-names); a path to your own `.svg`
file needs a `file` permission for it (`permissions = { file = { "C:/icons/a.svg" } }`),
at most 1 MB, and it follows `color` like a bundled one. `rsb.image { src = ... }`
draws an `.svg` in its own colours.

## 5. Widgets in Luau

A widget is a file under `widgets\` that returns `rsb.widget { ... }`. The
smallest one:

```lua
--!strict
local rsb = require("@rsb")

return rsb.widget {
    setup = function(props, ctx)
        return rsb.text "hello"
    end,
}
```

Save it as `widgets\hello.luau`, add `"hello"` to a zone, and it is on the bar.
A full one:

```lua
--!strict
local rsb = require("@rsb")

return rsb.widget {
    name = "my/weather",                       -- optional; defaults to the file's ref
    options = { latitude = -33.9, longitude = 151.2, every = "15m" },
    optional = { "api_key" },                  -- options with no default
    permissions = { net = { "api.open-meteo.com", "open-meteo.com" } },
    setup = function(props, ctx)
        local temp = rsb.poll(props.every, function()
            local r = rsb.http.get(`https://api.open-meteo.com/v1/forecast?latitude={props.latitude}&longitude={props.longitude}&current=temperature_2m`)
            return if r.ok then r:json().current.temperature_2m else nil
        end)
        return rsb.row {
            class = "item",
            rsb.icon "cloud",
            rsb.text { text = function() return if temp.value then `{math.round(temp.value)}°` else "--" end },
            on_click = function() rsb.open_url("https://open-meteo.com") end,
        }
    end,
}
```

### Options and the nil-default cast

`options` gives every option a default; `bar.toml` overrides it, and a value of
the wrong type is a warning with its line. The type of an option is the type of
its default, so `props.every` is a string above. An option with no default
cannot be written as `api_key = nil` (a table cannot hold nil, so RSB would never
know the option exists and would call it unknown). Instead list it in
`optional` and give the type with a cast:

```lua
options = { api_key = (nil :: string?) },
optional = { "api_key" },
```

`props.api_key` is then `string?`: nil until `bar.toml` sets it.

### Describing a widget: `about`

A settings page lists widgets with a title, whether they draw anything and which
popups they open. The table given to `rsb.widget` may carry an optional `about`
for that; the bar itself ignores it.

```text
about = { title = "Power menu", visible = false, opens = { "power" } },
```

| Field | Meaning |
|---|---|
| `title` | what the page calls the widget: 1 to 20 characters, one line. Default: the widget's name |
| `visible` | `false` for a widget that draws nothing and only anchors a popup, like `power`. Default `true` |
| `needs_options` | `true` when the widget draws nothing until `bar.toml` gives it options, like `text`. Default `false` |
| `opens` | the popups it hosts or opens: a list of up to 8 names. Default empty |

Every built-in widget has one. A field of another type, or one that is not in the
table above, is an error when the page lists the widget.

### Reactivity

Props may be plain values, signals, or functions. A function prop is re-run
when any signal or service field it read changes, and only that element's prop
is updated; reading `.value` or a service field directly in `setup` gives the
value once and logs a one-time warning (read it inside a function to keep it
updated). `rsb.state`, `rsb.computed`, `rsb.effect`, `rsb.batch`, `rsb.untrack`
and `rsb.poll` are the tools; everything created while `setup`, a handler, a
timer, a task, an effect, a computed, a function prop or an `rsb.each` item
runs belongs to that scope and is cleaned up when it re-runs or goes away, so
an effect that starts a timer has exactly one timer, not a pile.

Children go in the array part of a table: strings and numbers become text, nil
is skipped, a function returning a list is re-run when what it read changes (use
`rsb.each` with a key to keep each item's state). An element value can be placed
once. Elements are opaque: a hand-made `{ kind = "text" }` is refused.
`rsb.each` keeps an item's element when it moves unless its render function
takes the index argument. A widget file runs in its own sandbox, and globals it
writes at file level are private to that widget instance.

Props every element takes: `id`, `class` (a string, a list, or reactive), `style`
(a table of CSS properties with `_` for `-`; numbers are px for lengths and plain
numbers otherwise, so `{ font_size = 14, opacity = 0.5, font_weight = 600 }`;
problems are warnings at the Luau line that made the element), `key`, `visible`
(`false` is `display: none`), and the handlers `on_click`, `on_right_click`,
`on_middle_click` (a `ClickEvent`: `button`, `x`, `y`), `on_wheel` (a
`WheelEvent`: `delta` in notches, positive up), `on_enter`, `on_leave`. A handler
runs as a task, so it may call functions that wait. A right click nothing
handles opens RSB's own menu; a right click handler of yours wins.

The context (`ctx`): `id` (`"clock#utc"`), `name`, `bar`, `dir` (the widget's
folder), and `monitor` (`index`, `handle`, `device`, `primary`, `scale`; the scale
updates in place).

### Waiting calls

`rsb.http.get`, `rsb.exec.run`, `rsb.exec.shell`, `rsb.file.read` and
`rsb.task.sleep` wait for a result without blocking anything: call them inside
`rsb.task.spawn`, `rsb.poll`, a timer, a handler or an `rsb.events.on` callback.
Anywhere else (setup, a computed, an effect, a function prop) they raise
`rsb.http.get waits for a result; call it inside rsb.task.spawn, rsb.poll, a timer
or a handler`. Errors travel like normal Luau errors, and `pcall` works.

Durations are seconds (`2`, `0.5`) or strings with a unit: `"500ms"`, `"2s"`,
`"1m"`, `"1h"`. Whole seconds, minutes and hours line up with the clock.

### Names for later

`rsb.settings`, `rsb.tray`, `rsb.graph`, `rsb.open` and `rsb.ipc` are reserved
and raise `rsb.settings is not available in this version`, as does the element
prop `draggable`. A few names people try raise a pointer: `rsb.sleep does not
exist; use rsb.task.sleep`. Any other misspelling gets a did-you-mean.

### Popups, menus, inputs and focus

`rsb.popup { open = shown, ... }` as a child of an element opens a popup next to
it; its content is built only while it is open, and everything it started
(timers, service reads, tasks) is released when it closes. `open` is a
`rsb.state(false)` the runtime also writes `false` to when the user closes the
popup. `name = "launcher"` (or a list) lets `rsb popup launcher` and
`rsb.popups.toggle("launcher")` reach it; the names `dashboard`, `launcher`,
`power`, `calendar`, `media`, `volume` and `network` belong to the built-in
widgets (a copy made with `rsb eject` keeps the names of its own widget). `placement` is `auto`, `below`, `above`, `left`, `right` or `screen`
(a popup that covers the monitor; the widget needs `overlay = true` in its
permissions and it only opens from a click or a key). `rsb.menu`, the `menu`
prop (a right-click menu), the `tooltip` prop, `rsb.input` (one line of editable
text with `value`, `placeholder`, `on_change`, `on_submit`), `rsb.slider`
(`value`, `min`, `max`, `step`, `on_change`) and `rsb.scroll` (a column that
scrolls; give it a `height` or `max-height`) are the other parts. Keys reach
`on_key`; `focusable`, `autofocus`, `nav`, `disabled`, `checked` and `reveal`
control focus and selection. `rsb.fuzzy(query, items, { key = ..., limit = 50 })`
ranks items best first and forgives typos; `rsb.fuzzy_match(query, text)` gives
the character positions to highlight. A click, a key press or `rsb popup` is a
*gesture*: commands that change the system (`power:sleep()`,
`bluetooth:set_enabled()`, `apps:launch()`, and the others listed under
permissions) only work inside one, so a timer can never power the PC off.

### Writing a popup

A popup is a child of the element it hangs from. This one opens under a bar
button and closes with Escape or a click outside it:

```lua
local rsb = require("@rsb")

return rsb.widget {
	setup = function(props, ctx)
		local shown = rsb.state(false)
		return rsb.row {
			class = "item hello button",
			rsb.text { text = "Hello" },
			on_click = function()
				shown.value = not shown.value
			end,
			rsb.popup {
				open = shown,
				name = "hello",
				class = "panel",
				placement = "below",
				align = "center",
				function(): { any }
					return { rsb.text { class = "note", text = "Hi there" } }
				end,
			},
		}
	end,
}
```

- **Open state.** `open` is a `rsb.state(false)`. The runtime writes `false`
  into it when the user closes the popup, so the button needs no extra code.
  The function child runs when the popup opens and again when what it reads
  changes; timers, service reads and tasks started inside it end with the popup.
- **Names.** `name = "hello"` (or `{ "hello", "other" }`) lets `rsb popup hello`
  and `rsb.popups.toggle("hello")` open it. `on_open = function(name) ... end`
  says which name opened it, as the dashboard uses to land on the apps page for
  `launcher`.
- **Where it goes.** `placement` is `below` (under the anchor; `align = "center"`
  centres it), `above`, `left`, `right`, `auto` (the side with room) or `screen`
  (covers the monitor: an overlay; the widget needs `overlay = true` in its
  permissions and it opens only from a click or a key). `attach = true` joins
  the popup to the bar's lower edge at the anchor's width, as the dashboard
  does under the centre button. A popup is moved to stay on its monitor. The
  popup of a widget without `overlay = true` is at most 900 px each way (at 100 %
  scale), 60 % of the work area wide and 90 % tall, so it can never pass for a
  full-screen prompt; the built-in panels are not limited this way.
- **Looks.** The class `panel` gives the dark panel with a square top and a
  round bottom; inside it use `card`, `row`, `note`, `icon-btn` and `search`.
  They take their colours from the preset and follow dark and light. `:open`
  matches the anchor while its popup is open.
- **Focus and keys.** A popup takes the keyboard only when a click, a key press
  or `rsb popup` opened it and its content holds something to focus (an input,
  a slider, a menu item or an element with `focusable = true`); `focus = "none"`
  or `"always"` overrides that. `autofocus = true` picks the element that gets
  it first. Tab walks the focusable elements; `nav = "vertical"`,
  `"horizontal"` or `"grid"` makes the arrow keys move among the focusable
  children of an element. Keys nothing used reach `on_key = function(e) ... end`
  on the popup (`e.key` is `"Enter"`, `"Escape"`, `"ArrowDown"`, `"a"`, ...).
  `dismiss = { escape = false }` keeps Escape for your own `on_key`. A popup
  that opens on its own (a timer, a service update: no click or key) can always
  be dismissed, whatever `dismiss` says, and when the user closes it it stays
  closed for ten seconds unless a click or a key opens it. Alt+F4 on a focused
  popup closes it like Escape does.

A line to type into, a list that scrolls and a row to click:

```lua
local rsb = require("@rsb")

return rsb.widget {
	setup = function(props, ctx)
		local shown = rsb.state(false)
		local query = rsb.state("")
		local names = { "Alder", "Birch", "Cedar", "Maple", "Oak", "Willow" }
		local found = rsb.computed(function(): { string }
			if query.value == "" then
				return names
			end
			return rsb.fuzzy(query.value, names)
		end)
		local rows = rsb.each(found, function(name: string): any
			return rsb.row {
				class = "row",
				focusable = true,
				tooltip = "Pick " .. name,
				rsb.text { text = name },
				on_click = function()
					shown.value = false
				end,
			}
		end)
		return rsb.row {
			class = "item picker button",
			rsb.text { text = "Pick" },
			on_click = function()
				shown.value = not shown.value
			end,
			rsb.popup {
				open = shown,
				class = "panel",
				placement = "below",
				function(): { any }
					return {
						rsb.input { class = "search", value = query, placeholder = "Search", autofocus = true },
						rsb.scroll { class = "card list", style = { max_height = 200 }, rows },
					}
				end,
			},
		}
	end,
}
```

- **Input.** `rsb.input { value = state, placeholder, max_length, on_change,
  on_submit }` is one line of editable text. With a state as `value` the box
  and the state stay equal both ways; setting the state fills or clears the
  box. `on_submit(text)` runs on Enter. Home, End, Ctrl+A and Ctrl+Backspace
  work as in any text box; the caret shows only while the input has the focus.
- **Slider.** `rsb.slider { value = state, min, max, step, on_change }`. The
  callback gets the new value and `done` (true on the last report of a drag).
  A focused slider takes the arrow keys, Page Up and Down, Home and End, and
  shows a ring. It is at least 80 px wide, so give it a `width` of your own for
  more; its track is the `--row` colour and its fill the accent line colour (set
  `accent-color`, `background` and `border-radius` to change them). A press on
  the track moves the value there at once; `jump = false` makes a press only
  start a gesture instead: the value follows the pointer once it has moved more
  than 4 px, and a press and release without that is a click (`on_click` runs and
  the value stays). The audio rows use it, so a click on a row picks the output
  and a drag sets its level.
- **Scroll.** `rsb.scroll` is a column that scrolls when its content is taller
  than its `height` or `max_height`. The wheel, the scrollbar and the keys move
  it, focus moving into it scrolls the focused element into view, and
  `reveal = value` scrolls an element into view whenever `value` changes.
- **Menus and tooltips.** `menu = { { label = "Reload", on_click = reload },
  { separator = true } }` on any element opens a menu at a right click;
  `rsb.menu { open = state, items = { ... } }` as a child opens one from code.
  `tooltip = "text"` (or an element) shows when the pointer rests on the
  element.
- **Gestures.** A click or a key press is a *gesture*. Commands that change the
  system (`apps:launch`, `power:sleep`, `bluetooth:connect`, `wifi:connect`,
  `audio:set_default` and the others under permissions) raise "only works
  inside a click or key handler" anywhere else, so a timer or a service
  callback can never do them. The popup of your own widget opens from
  anywhere; an overlay, and a popup that belongs to another widget, need one.
- **From the command line.** `rsb popup <name>` opens a popup by name on the
  monitor under the pointer (`rsb popup <name> close` closes it). It counts as
  a gesture, so a keyboard shortcut in your window manager can run
  `rsb popup launcher`.

The built-in popups follow the same rules and can be copied with `rsb eject`:
`dashboard` (also called `launcher`), `power`, `calendar`, `media`, `volume`
and `network`. `docs\examples\todo.luau` is a small complete popup widget.

## 6. API reference

Every function with a short example. The same names, typed, are in
`types\rsb.d.luau`.

#### `rsb.version`

`number`

API version, 1

```lua
if rsb.version >= 1 then ... end
```

#### `rsb.widget`

`<O>(spec: WidgetSpec<O>) -> Widget`

Defines a widget; a widget file returns this

```lua
return rsb.widget { options = { format = "HH:mm" }, setup = function(props, ctx) return rsb.text(props.format) end }
```

#### `rsb.box`

`(props: BoxProps) -> Element`

A flex container, children in a row

```lua
rsb.box { class = "pill", rsb.text "a", rsb.text "b" }
```

#### `rsb.row`

`(props: BoxProps) -> Element`

A box laid out left to right

```lua
rsb.row { rsb.icon "cpu", rsb.text "12%" }
```

#### `rsb.column`

`(props: BoxProps) -> Element`

A box laid out top to bottom

```lua
rsb.column { rsb.text "up", rsb.text "down" }
```

#### `rsb.text`

`(props: TextProps | string | number) -> Element`

One line of text (what comes before the first line break)

```lua
rsb.text { text = function() return `{cpu.value}%` end }
```

#### `rsb.icon`

`(props: IconProps | IconName) -> Element`

An icon by name, or a raw glyph

```lua
rsb.icon { name = "volume_3" }
```

#### `rsb.image`

`(props: ImageProps) -> Element`

A PNG image (needs file permission outside the widget folder)

```lua
rsb.image { src = "logo.png", width = 16 }
```

#### `rsb.popup`

`(props: PopupProps) -> Element`

A popup anchored to the element it is a child of; its content is built only while it is open

```lua
rsb.popup { open = shown, function() return rsb.text "hi" end }
```

#### `rsb.menu`

`(props: MenuProps) -> Element`

A menu (as a child: anchored; the menu prop of any element opens one at a right click)

```lua
rsb.menu { open = m, items = { { label = "Reload", on_click = reload } } }
```

Choosing an item runs its `on_click` and closes the whole menu chain. A
menu's `on_close` hears `"picked"` once for a pick in the menu or in any
submenu below it, so a submenu needs no `on_close` of its own.

#### `rsb.input`

`(props: InputProps) -> Element`

One line of editable text

```lua
rsb.input { value = query, placeholder = "Search", autofocus = true }
```

#### `rsb.slider`

`(props: SliderProps) -> Element`

A value on a track

```lua
rsb.slider { value = vol, on_change = function(v) audio:set_device_volume(id, v) end }
```

#### `rsb.scroll`

`(props: BoxProps) -> Element`

A column that scrolls

```lua
rsb.scroll { class = "list", rsb.each(items, row) }
```

#### `rsb.popups.open`

`(name: string) -> ()`

Opens the named popup on this widget's monitor (from a user widget only inside a click or key handler, unless it is the widget's own popup)

```lua
on_click = function() rsb.popups.open("launcher") end
```

#### `rsb.popups.close`

`(name: string) -> ()`

Closes the named popup on this widget's monitor

```lua
rsb.popups.close("launcher")
```

#### `rsb.popups.toggle`

`(name: string) -> ()`

Opens the named popup, or closes it when it is open

```lua
on_click = function() rsb.popups.toggle("launcher") end
```

#### `rsb.popups.is_open`

`(name: string) -> boolean`

Whether the named popup is open on this widget's monitor; read inside a function prop it updates

```lua
class = function() return rsb.popups.is_open("power") and "on" or "" end
```

#### `rsb.animate`

`(name: string, options: AnimateOptions?) -> Animate`

An animate prop value: runs @keyframes name, again from the start when options.restart changes

```lua
animate = function() return rsb.animate("flash", { duration = "300ms", restart = count.value }) end
```

#### `rsb.fuzzy`

`<T>(query: string, items: { T }, options: { key: ((T) -> string)?, limit: number? }?) -> { T }`

The items that match query, best first, typo tolerant

```lua
rsb.fuzzy(q.value, apps.apps, { key = function(a) return a.name end, limit = 50 })
```

#### `rsb.fuzzy_match`

`(query: string, text: string) -> { number }?`

The 1-based character positions of text that matched query, for highlighting; nil when it does not match

```lua
local hit = rsb.fuzzy_match(q.value, app.name)
```

#### `rsb.each`

`<T>(list: Reactive<{ T }>, render: (item: T, index: number) -> Element, key: ((item: T) -> string)?) -> () -> { Element }`

Children from a list; with key, unchanged items keep their state

```lua
rsb.row { rsb.each(function() local m = ws.monitors return if m and m[1] then m[1].workspaces else {} end, function(w) return rsb.text(w.display_name) end, function(w) return w.name end) }
```

#### `rsb.state`

`<T>(initial: T) -> State<T>`

A value you can set; readers update when it changes

```lua
local count = rsb.state(0)
```

#### `rsb.computed`

`<T>(fn: () -> T) -> Signal<T>`

A value derived from others; recomputed when they change

```lua
local label = rsb.computed(function() return `{count.value} items` end)
```

#### `rsb.effect`

`(fn: () -> (() -> ())?) -> ()`

Runs fn now and again whenever what it reads changes; fn may return a cleanup

```lua
rsb.effect(function() rsb.log.info(`volume {audio.volume}`) end)
```

#### `rsb.batch`

`(fn: () -> ()) -> ()`

Several changes, one update

```lua
rsb.batch(function() a.value = 1; b.value = 2 end)
```

#### `rsb.untrack`

`<T>(fn: () -> T) -> T`

Reads without subscribing

```lua
local v = rsb.untrack(function() return count.value end)
```

#### `rsb.poll`

`<T>(every: Duration, fn: () -> T) -> PollSignal<T>`

Runs fn now and every period inside a task (it may wait on I/O); .value holds the last result (nil before the first), .error the last error (the value is kept)

```lua
local ip = rsb.poll("5m", function() return rsb.http.get("https://api.ipify.org").body end)
```

#### `rsb.on_cleanup`

`(fn: () -> ()) -> ()`

Runs fn when the widget unloads or reloads

```lua
rsb.on_cleanup(function() rsb.log.info("bye") end)
```

#### `rsb.task.spawn`

`(fn: () -> ()) -> Task`

Starts a task: code that may wait on I/O without blocking anything

```lua
rsb.task.spawn(function() local r = rsb.http.get(url) end)
```

#### `rsb.task.sleep`

`(d: Duration) -> ()` (waits: use inside a task, handler or timer)

Waits inside a task

```lua
rsb.task.sleep("500ms")
```

#### `rsb.task.cancel`

`(task: Task) -> ()`

Stops a task (same as task:cancel())

```lua
rsb.task.cancel(t)
```

#### `rsb.timer.every`

`(d: Duration, fn: () -> ()) -> Timer`

Calls fn every d; whole seconds, minutes and hours line up with the clock

```lua
rsb.timer.every("1m", refresh)
```

#### `rsb.timer.after`

`(d: Duration, fn: () -> ()) -> Timer`

Calls fn once after d

```lua
rsb.timer.after(2, function() shown.value = false end)
```

#### `rsb.clock`

`(precision: ("second" | "minute" | "hour")?, timezone: string?) -> Signal<Time>`

The time, updated on each whole second / minute / hour and at once after sleep or a clock change; timezone "local" (default), "utc" or "+10:00"

```lua
local now = rsb.clock("minute")
```

#### `rsb.time.now`

`(timezone: string?) -> Time`

The time now

```lua
local t = rsb.time.now("utc")
```

#### `rsb.time.format`

`(t: Time, format: string) -> string`

Formats a time: yyyy MM MMM dd ddd HH hh mm ss tt (see the docs)

```lua
rsb.time.format(now.value, "ddd d MMM HH:mm")
```

#### `rsb.time.precision`

`(format: string) -> "second" | "minute" | "hour"`

The finest unit a format shows

```lua
rsb.clock(rsb.time.precision(props.format))
```

#### `rsb.services.system`

`SystemService`

CPU and memory

```lua
rsb.services.system.cpu
```

#### `rsb.services.battery`

`BatteryService`

Battery

```lua
rsb.services.battery.percent
```

#### `rsb.services.audio`

`AudioService`

Volume

```lua
rsb.services.audio:change_volume(5)
```

#### `rsb.services.network`

`NetworkService`

Network

```lua
rsb.services.network.kind
```

#### `rsb.services.media`

`MediaService`

Media session

```lua
rsb.services.media:play_pause()
```

#### `rsb.services.window`

`WindowService`

Foreground window

```lua
rsb.services.window.title
```

#### `rsb.services.workspaces`

`WorkspacesService`

Window manager workspaces

```lua
rsb.services.workspaces:focus("2")
```

#### `rsb.services.microphone`

`MicrophoneService`

The default recording device, and whether any app is using a microphone

```lua
rsb.services.microphone:toggle_mute()
```

#### `rsb.services.bluetooth`

`BluetoothService`

The Bluetooth radio and paired devices

```lua
rsb.services.bluetooth:set_enabled(false)
```

#### `rsb.services.apps`

`AppsService`

Apps in the Start menu, for a launcher

```lua
rsb.services.apps:open_uri("ms-settings:bluetooth")
```

#### `rsb.services.power`

`PowerService`

Power mode and the lock, sleep, restart, shut down and sign out actions

```lua
rsb.services.power:lock()
```

#### `rsb.services.wifi`

`WifiService`

Wi-Fi networks in range

```lua
rsb.services.wifi:scan()
```

#### `rsb.exec.run`

`(command: { string }, options: ExecOptions?) -> ExecResult` (waits: use inside a task, handler or timer)

Runs a program (argument list, no shell) and waits for it

```lua
local r = rsb.exec.run({ "git", "-C", dir, "status", "--short" })
```

#### `rsb.exec.shell`

`(command: string, options: ExecOptions?) -> ExecResult` (waits: use inside a task, handler or timer)

Runs a command line through cmd.exe (needs exec permission for "cmd")

```lua
rsb.exec.shell("ver")
```

#### `rsb.exec.spawn`

`(command: { string }, options: SpawnOptions?) -> Process`

Starts a long-running program and streams its lines

```lua
rsb.exec.spawn({ "ping", "-t", "1.1.1.1" }, { on_line = function(l) last.value = l end })
```

#### `rsb.exec.lines`

`(command: { string }) -> Signal<string?>`

The latest line a long-running program printed

```lua
local line = rsb.exec.lines({ "my-feed.exe" })
```

#### `rsb.file.read`

`(path: string, options: FileOptions?) -> (string?, string?)` (waits: use inside a task, handler or timer)

Reads a text file; returns text, or nil and the reason

```lua
local text, err = rsb.file.read("~/notes.txt")
```

#### `rsb.file.watch`

`(path: string, options: FileOptions?) -> FileSignal`

A file's text, re-read whenever it changes

```lua
local counter = rsb.file.watch("~/count.txt")
```

#### `rsb.http.get`

`(url: string, options: HttpOptions?) -> Response` (waits: use inside a task, handler or timer)

HTTPS GET

```lua
local r = rsb.http.get(url, { cache = "10m" })
```

#### `rsb.http.post`

`(url: string, body: string | { [any]: any }, options: HttpOptions?) -> Response` (waits: use inside a task, handler or timer)

HTTPS POST; a table body is sent as JSON

```lua
rsb.http.post(url, { ok = true })
```

#### `rsb.json.decode`

`(text: string) -> any`

JSON text (at most 2 MiB, 64 levels deep) to a value; null becomes nil

```lua
local v = rsb.json.decode(r.body)
```

#### `rsb.json.encode`

`(value: any) -> string`

A value to JSON text (tables that contain themselves or nest over 64 levels are refused)

```lua
rsb.json.encode({ a = 1 })
```

#### `rsb.open_url`

`(url: string) -> ()`

Opens an https URL in the default browser: only while a click is being handled, at most one address every 2 seconds (needs net permission for its host)

```lua
on_click = function() rsb.open_url("https://open-meteo.com") end
```

#### `rsb.events.on`

`(name: string, fn: (data: any) -> ()) -> Subscription`

Runs fn (as a task, on the next turn) for `rsb trigger <name> [json]` and rsb.events.emit; data comes from outside the widget, check it

```lua
rsb.events.on("deploy", function(d) status.value = d.state end)
```

#### `rsb.events.emit`

`(name: string, data: any?) -> ()`

Sends an event to every widget (at most 50 a second and 1 MiB of data a second per widget; one event carries at most 64 KiB)

```lua
rsb.events.emit("refresh")
```

#### `rsb.theme.dark`

`boolean`

Windows' app theme is dark (reactive)

```lua
rsb.text { text = function() return rsb.theme.dark and "dark" or "light" end }
```

#### `rsb.log.info`

`(...any) -> ()`

Writes to the RSB log (print does the same); 20 lines a second, line breaks escaped. What you log is yours: keep titles and names out

```lua
rsb.log.info("loaded", n)
```

#### `rsb.log.warn`

`(...any) -> ()`

Writes a warning to the log

```lua
rsb.log.warn("slow")
```

#### `rsb.log.error`

`(...any) -> ()`

Writes an error to the log

```lua
rsb.log.error(err)
```

#### `rsb.format.bytes`

`(n: number) -> string`

1536 -> "1.5 KB" (1024-based)

```lua
rsb.format.bytes(mem.memory_used)
```

#### `rsb.format.rate`

`(bytes_per_sec: number) -> string`

Bytes per second -> "1.2 MB/s"

```lua
rsb.format.rate(net.down_bps)
```

#### `rsb.format.percent`

`(n: number) -> string`

23.4 -> "23%"

```lua
rsb.format.percent(sys.cpu)
```

### Services

Until the first reading arrives (status `starting`, a moment after the first
read) a service table holds only its status: every value below is nil, so write
`local cpu = rsb.services.system.cpu` and check `if cpu then` (or use `or 0`)
before doing arithmetic. The types mark every value optional (`number?`) for
that reason, so strict mode asks for the check.

Reading a field of `rsb.services.<name>` is a tracked read and subscribes the
widget to that source (it starts when the first widget reads it and stops two
seconds after the last one leaves). Every service table also has:

- `status` (ServiceStatus): "off", "starting", "ok", "stale", "unavailable" or "error"
- `stale` (boolean): true when status is "stale": show a placeholder, not the old value
- `error` (string?): why the service is unavailable or failing

Commands are methods (`rsb.services.audio:change_volume(5)`) and need the
matching `control` permission. A service table is read-only. Sampled sources have
`:interval(d)`; repeated calls replace the widget's wish and the fastest reader
wins. Reading `window`, `media` or `network` needs `read` permission (your own
widget files get it by default; built-ins declare it). A service that does not
exist on this PC (no audio device, no window manager) reports `unavailable`, not
an error. The Wi-Fi name needs Windows' location permission for desktop apps on
Windows 11 24H2 and later; without it `ssid` is nil and `signal` still works.

#### `rsb.services.system`

CPU and memory load, sampled while someone reads it (default every 2 s)

- `cpu` (number?): CPU load in percent, 0-100, over the last interval
- `memory` (number?): memory in use in percent, 0-100
- `memory_used` (number?): memory in use, bytes
- `memory_total` (number?): installed memory visible to Windows, bytes
- `:interval(d)`: how often to sample (the fastest reader wins)

#### `rsb.services.battery`

Battery and power source

- `present` (boolean?): false on a PC without a battery: show nothing or a plug
- `percent` (number?): charge 0-100 when present
- `charging` (boolean?): charging now
- `plugged` (boolean?): on mains power
- `saver` (boolean?): battery saver is on
- `seconds_left` (number?): estimated seconds of battery left, when Windows knows

#### `rsb.services.audio`

Default output device volume

- `available` (boolean?): an output device exists
- `volume` (number?): 0-100
- `muted` (boolean?): muted
- `devices` ({AudioDevice}?): output devices, the default first; each has `id` (Windows' endpoint id), `name` (never logged), `kind`, `default`, `volume` (0-100), `muted` and `bluetooth`
- `inputs` ({AudioDevice}?): input (recording) devices, the default first
- `switch_available` (boolean?): `set_default` works here (it uses an interface Windows does not document)
- `:set_volume(percent: number)`: set the volume, 0-100
- `:change_volume(delta: number)`: add delta (may be negative) to the volume
- `:set_muted(muted: boolean)`: mute or unmute
- `:toggle_mute()`: flip mute
- `:set_default(id: string)`: make this output the default
- `:set_device_volume(id: string, percent: number)`: set one device's volume, 0-100
- `:set_device_muted(id: string, muted: boolean)`: mute or unmute one device

#### `rsb.services.network`

Connection state and throughput (rates sampled while read, default every 2 s)

- `connected` (boolean?): any network connected
- `internet` (boolean?): Windows reports internet access
- `kind` (NetworkKind?): "wifi", "ethernet", "cellular", "other" or "none"
- `ssid` (string?): Wi-Fi network name (never logged)
- `signal` (number?): Wi-Fi signal 0-100
- `down_bps` (number?): bytes per second received, all adapters
- `up_bps` (number?): bytes per second sent, all adapters
- `:interval(d)`: how often to sample (the fastest reader wins)

#### `rsb.services.media`

The current media session (Windows media controls)

- `available` (boolean?): a media session exists
- `playing` (boolean?): playing now
- `title` (string?): track title, empty when unknown (never logged)
- `artist` (string?): artist, empty when unknown (never logged)
- `album` (string?): album, empty when unknown
- `source` (string?): app id of the player
- `can_play_pause` (boolean?): play/pause is allowed
- `can_next` (boolean?): next is allowed
- `can_previous` (boolean?): previous is allowed
- `position` (number?): seconds into the track when `position_at` was read
- `position_at` (number?): when `position` was read, ms since 1970 (add the time since while a track plays)
- `length` (number?): track length in seconds
- `can_seek` (boolean?): seek is allowed
- `art` (string?): image key of the cover for `rsb.image { src = media.art }` (kept in memory only)
- `app_icon` (string?): image key of the player app's own icon, when Windows has one for it
- `:play_pause()`: toggle play and pause
- `:play()`: play
- `:pause()`: pause
- `:next()`: next track
- `:previous()`: previous track
- `:seek(seconds: number)`: jump to this position in the track

#### `rsb.services.window`

The foreground window

- `has_window` (boolean?): a normal window has focus (false on the desktop)
- `title` (string?): window title (never logged)
- `process` (string?): program file name without folder, e.g. "Code.exe"

#### `rsb.services.workspaces`

Workspaces from a running window manager (GlazeWM-compatible WebSocket, e.g. Astur or GlazeWM, or komorebi)

- `provider` (WorkspaceProvider?): "glazewm", "komorebi" or "none"
- `connected` (boolean?): talking to a window manager now
- `monitors` ({Monitor}?): monitors with their workspaces; empty when none
- `:focus(name: string)`: switch to the workspace with this name
- `:focus_next(monitor: number?)`: next workspace on a monitor (default: the focused one)
- `:focus_previous(monitor: number?)`: previous workspace on a monitor

#### `rsb.services.microphone`

The default recording device, and whether any app is using a microphone

- `available` (boolean?): a recording device exists
- `muted` (boolean?): the default recording device is muted
- `volume` (number?): its level, 0-100
- `in_use` (boolean?): an app is recording from a microphone now
- `:set_muted(muted: boolean)`: mute or unmute the default recording device
- `:toggle_mute()`: flip its mute

#### `rsb.services.bluetooth`

The Bluetooth radio and paired devices. Device names are private: reading needs `read = { "bluetooth" }`

- `available` (boolean?): this PC has a Bluetooth radio
- `enabled` (boolean?): the radio is on
- `can_toggle` (boolean?): RSB may turn the radio on and off
- `devices` ({BluetoothDevice}?): paired devices, connected first (`id`, `name`, `kind`, `connected`, `battery`, `can_connect`)
- `:set_enabled(on: boolean)`: turn the radio on or off
- `:connect(id: string)`, `:disconnect(id: string)`: a paired audio device
- `:forget(id: string)`: remove a pairing
- `:open_settings()`: open Windows' Bluetooth page
- `:rescan()`: read the paired devices and their state again

#### `rsb.services.apps`

The Start menu apps, for a launcher. Reading the list needs `read = { "apps" }`

- `ready` (boolean?): the list has been read
- `apps` ({App}?): every app, sorted by name (`id`, `name`, `icon`)
- `:launch(id: string)`: start an app from the list
- `:open_uri(uri: string)`: open a Windows page: `ms-settings:` or `ms-screenclip:` addresses only

#### `rsb.services.power`

- `mode` (PowerMode?): "efficiency", "balanced", "performance" or "unknown"
- `mode_available` (boolean?): `set_mode` works here
- `can_sleep` (boolean?): this PC can sleep
- `:set_mode(mode: PowerMode)`, `:lock()`, `:sleep()`, `:restart()`, `:shut_down()`, `:sign_out()`

A failed action shows as the service's error, not as an error of the call.

#### `rsb.services.wifi`

Wi-Fi networks in range. Network names reveal where the PC is: reading needs `read = { "wifi" }`

- `available` (boolean?): this PC has a Wi-Fi adapter
- `scanning` (boolean?): a scan is running
- `networks` ({WifiNetwork}?): networks in range, strongest first
- `access` (WifiAccess?): "needs_location" when Windows keeps network lists from desktop apps until location is on for them
- `:scan()`, `:connect(ssid: string)` (a network with a saved profile), `:disconnect()`

### Icon names

`rsb.icon "<name>"` takes one of these (drawn in Segoe Fluent Icons or Segoe
MDL2 Assets), or `rsb.icon { glyph = "..." }` for any text in the icon font:

`volume_mute`, `volume_0`, `volume_1`, `volume_2`, `volume_3`, `wifi_0`, `wifi_1`, `wifi_2`, `wifi_3`, `wifi_4`, `wifi_off`, `ethernet`, `network_off`, `battery_0`, `battery_1`, `battery_2`, `battery_3`, `battery_4`, `battery_5`, `battery_6`, `battery_7`, `battery_8`, `battery_9`, `battery_10`, `battery_charging`, `battery_saver`, `battery_none`, `cpu`, `memory`, `clock`, `calendar`, `play`, `pause`, `next`, `previous`, `music`, `window`, `desktop`, `cloud`, `sun`, `moon`, `rain`, `snow`, `settings`, `power`, `search`, `bell`, `globe`, `download`, `upload`, `warning`, `error`, `info`

### SVG icons

`rsb.icon { svg = "<name>" }` draws one of the bundled SVG icons in the text
colour at the size of `font-size`: `logo`, `power`, `restart`, `sleep`, `lock`,
`sign-out`, `bolt`, `leaf`, `balance`, `rocket`, `bluetooth`, `bluetooth-off`,
`bluetooth-connected`, `speaker`, `volume`, `volume-low`, `volume-mute`,
`headphones`, `mic`, `mic-off`, `apps`, `search`, `refresh`, `trash`, `shield`,
`check`, `close`, `video`, `record`, `battery-0`, `battery-1`, `battery-2`,
`battery-3`, `battery-4`, `battery-charging`, `plug`, `memory`, `cpu`, `wifi-0`,
`wifi-1`, `wifi-2`, `wifi-3`, `wifi-off`, `ethernet`, `play`, `pause`, `next`,
`previous`, `chevron-left`, `chevron-right`, `chevron-up`, `chevron-down`,
`calendar`, `music`. A path to your own `.svg` file works too (it needs a `file`
permission). The icons are from Tabler Icons (MIT), apart from `logo`; their
licence is in `assets\icons\LICENSE`.

### Time format

Tokens (case-sensitive; anything else is copied; text in single quotes is
copied as written, `''` is a quote): `yyyy` `yy` year; `MMMM` `MMM` `MM` `M`
month as a name, short name, two digits, number; `dddd` `ddd` `dd` `d` weekday
name, short weekday, two-digit day, day; `HH` `H` hour 0-23; `hh` `h` hour 1-12;
`mm` `m` minute; `ss` `s` second; `tt` `t` AM/PM or A/P. Names are English.
`rsb.time.precision(format)` says how often a clock must tick: a format without
`s` ticks once a minute on the minute, one without hours and minutes once an hour.
Time zones are `"local"`, `"utc"` or an offset like `"+10:00"`.

## 7. Permissions

A widget can draw and read the services that need no permission. Everything else
is declared in `permissions` and checked on every call:

```lua
permissions = {
    exec = { "git" },                           -- programs it may run
    net = { "api.open-meteo.com", "*.example.com", "localhost:8443" },
    file = { "~/notes/*.txt" },                 -- read and watch only
    control = { "audio", "media", "workspaces" },
    read = { "window", "media", "network" },
}
```

- **exec**: a bare program name (`git`; `.exe` only; compared without case and with
  or without `.exe`; looked up on PATH only, never the current folder) or an
  absolute `.exe` path. A `.bat` or `.cmd` file gets the message `use {"cmd",
  "/d", "/c", "x.bat"}`. An entry allows the program with any arguments.
  `"$option:<key>"` grants what the option names
  **only when the user set it in bar.toml**: an option that is just a program
  (`"git"`) grants that program with any arguments; an option with arguments
  (`"git status --short"`, or a list) grants exactly that command line and
  nothing else. A widget's own default never grants anything.
  `rsb.exec.shell(line)` runs `cmd.exe /d /c <line>` and needs `exec = { "cmd" }`.
  A program starts in its own folder, not the widget's (Windows and many programs
  look in the working folder for DLLs and modules); `cwd` sets another one and
  must be a place the widget may read: `cwd = "."` is the widget's own folder.
- **Broad programs.** Some programs can run anything, directly or from their
  arguments (`cmd`, `powershell`, `pwsh`, `explorer`, `rundll32`, `mshta`,
  `wscript`, `cscript`, `conhost`, `wsl`, `bash`, `start`, `git`, `python`,
  `python3`, `py`, `node`, `forfiles`, `schtasks`, `wmic`, `msiexec`,
  `regsvr32`, `certutil`, `pcalua`, `msbuild`). Giving
  one is giving the widget the whole machine, so the grant line in the log marks
  it: `exec:cmd (can run any program)`. Prefer naming the one command you need
  in the option (`"git status --short"`). RSB ends the programs a widget starts
  directly when it stops or times out; what those programs ask Windows services
  to start (WMI, the Task Scheduler, COM) is outside its reach.
- **net**: lower-case host names; `*.domain` for subdomains (not the domain
  itself, and a domain, not `*.com` or `*.1`); a non-default port is part of the
  entry (`host:8443`); IP literals exactly, as a dotted IPv4 address
  (`2130706433` and `0x7f.1` are refused); loopback entries must name a port. `https://` to listed hosts;
  `http://` only to loopback. Redirects are followed by RSB, at most 5, each hop
  checked against the list, never from https to http. Cookies are never kept and
  Windows credentials are never sent.
- **file**: read and watch only (no writing in this version). The widget's own
  folder is always readable: for `widgets\my\weather.luau` that is `widgets\my\`.
  A file directly in `widgets\` and the built-in widgets have no folder of their
  own (it would hold other widgets' files): give them a full path (`~/...`), or
  put the widget in a folder. Patterns start with `~`, `%APPDATA%`,
  `%LOCALAPPDATA%`, `%TEMP%` or a drive; `*` matches within one folder level, `**`
  across levels. A pattern that covers a whole drive (`C:/**`, `C:/*/**`) is
  refused; `~/**` and the like (`~/*/**`, `~/AppData/**`, `~/.*/**`,
  `C:/Users/<name>/**`, `C:/Users/*/AppData/**`) are allowed and the grant line says
  `(can read every file there)`. UNC and device paths,
  alternate data streams, 8.3 short names, trailing dots, device names and
  `..` that leaves the allowed area are refused, and a junction or symbolic link
  that leads outside the granted place is refused after it is opened (a hard
  link is another name for the same file and is not caught). Relative paths (and
  `cwd`, `image.src`) are relative to the widget's own folder; a path that starts
  with `/` or `\` is refused.
- **control**: `audio`, `media`, `workspaces`, `microphone`, `bluetooth`, `apps`,
  `power`, `wifi` (the commands of those services) and `session` (restart, shut
  down and sign out, apart from `power`, which locks, sleeps and changes the
  power mode, so a lock button never also holds shutdown). Restart, shut down,
  sign out, sleep, Bluetooth `set_enabled`, `connect`, `disconnect`, `forget`,
  `open_settings` and `rescan`, audio `set_default`, apps `launch` and `open_uri` and Wi-Fi
  `connect` and `disconnect` only work inside a click or key handler.
- **read**: `window`, `media`, `network` (values that name what you are doing).
  Widget files in your config folder get these three without asking; packages
  would have to. `bluetooth` (device names), `apps` (every installed app) and
  `wifi` (network names) must be declared; the grant line in the log shows them.
- **overlay**: `overlay = true` lets a popup with `placement = "screen"` cover
  the whole monitor.
- **popups**: `popups = { "name" }` lets the widget open or close another
  widget's popup by name (`rsb.popups.toggle("name")`); the built-in popups and
  a widget's own need nothing.
- `rsb.open_url(url)` needs `https` and `net` permission for the host. It works
  only while a click is being handled (not from `setup`, a timer or a task that
  waited), opens at most one address every 2 seconds, and refuses an address
  with `"`, `'`, `<`, `>`, `^`, a backtick, `{`, `}` or `|` (write it as a %
  escape): a browser launcher could read those as extra arguments.

At load RSB logs one line per widget: `granted my/weather: net:api.open-meteo.com,
read:window, ...`. A denied call stops that widget with a plate:
`PERMISSION - my/weather needs net:api.open-meteo.com; add it to permissions`.

A widget may log what it reads (`rsb.log.info(title)` puts the window title in
the log file), so the log, not RSB, is what to keep private if you share it.
RSB itself never logs window titles, media names or Wi-Fi names. It logs a file
path only when a widget is refused it, exactly as the widget wrote it
(`permission denied: peek needs file:~/Documents/notes.txt`), and the programs
a grant names.

## 8. Limits and errors

| Limit | Value | Over it |
|---|---|---|
| CPU time per call into a widget (handler, timer, effect, function prop, task resume) | 10 ms | the widget stops: `STOPPED - my/weather ran over 10 ms; fix it and save` |
| Wall time per call, whatever the CPU | 250 ms | same |
| Loading a file and `setup` | 50 ms | `ran over 50 ms while loading` |
| CPU per widget in any second | 250 ms | `used too much CPU` |
| Effect and computed re-runs in one turn | 1000 | `is in an update loop (an effect changes what it reads)` |
| Events a widget emits | 50 a second | dropped and counted |
| Uncaught errors | 10 within 10 s | stopped; fewer are logged and the widget keeps running |
| Memory, one user widget file | 32 MiB | `used over 32 MB` |
| Memory, the shared VM of the built-ins | 64 MiB | the offender stops; if it stays above 90% after a clean-up the VM is rebuilt |
| Memory, all widgets together | 256 MiB | a new widget file is refused |
| Timers per widget, shortest interval | 64, 100 ms | error, clamped with one warning |
| Tasks per widget | 64 | error `too many tasks` |
| Nodes per widget, nesting depth | 2000, 64 | the widget stops |
| Text per node | 4096 characters | cut |
| `table.sort` without a comparator | 100 000 items | error |
| `rsb.json.decode` | 2 MiB, 64 levels | error |
| Log lines | 20 a second per widget, 200 in all, 200 characters each | dropped and counted |
| Programs, HTTP, watches | 8 programs per widget (32 total), 4 requests per widget, 16 watches per widget (64 total) | `busy` returned to the script |

A stop is decided by RSB, not by your code: `pcall` cannot swallow it. A built-in
that stops restarts by itself after 1 s, then 2 s, doubling to 5 minutes; a file
of yours waits until you save it or run `rsb reload`.

**The error policy.** What stops a widget: an error while loading or in `setup`, a
time or memory budget, a permission denial, the node or depth cap, an update loop,
too much CPU, or 10 errors in 10 s. Any other uncaught error (in a handler, timer,
task, poll, effect or function prop) is logged, adds the class `error` to the
widget's root for ten seconds, and the widget keeps running; a poll keeps its last
value and sets `.error`. Other widgets are never affected.

### Plates

An error never leaves a blank bar. A problem shows as a red plate with its place
and what to do next, and the full text is always in the log:

| Situation | Plate |
|---|---|
| CSS, TOML or Luau syntax or value problem | `RSB ERROR - style.css:12:5 unknown property 'colr'; did you mean 'color'?` |
| `bar.toml` unusable | `RSB ERROR - bar.toml:4:9 expected '='` (the last good file stays) |
| a font that is not installed | `RSB ERROR - style.css:20:3 font 'Inter' is not installed; using Segoe UI` |
| a permission denied | `PERMISSION - my/weather needs net:api.open-meteo.com; add it to permissions` (a built-in says `; restarting`) |
| an error in your widget's Luau | `RSB ERROR - widgets/clock.luau:5 attempt to index nil; fix it and save` (a built-in says `; restarting - check its settings in bar.toml`) |
| a budget | `STOPPED - my/weather ran over 10 ms; fix it and save` (a built-in says `; restarting`, as it does for an update loop) |
| a stalled call | `STOPPED - widgets are not responding (my/x); see log` |
| the bar could not be drawn | `RENDER ERROR - could not draw the bar; see log` |
| commands cannot reach the bar | `PIPE ERROR - rsb commands will not reach this bar; see log` |

Several problems add ` (+N more, see log)`. Clicking a plate opens the log. A
stale data source is not a plate: the widget shows `--` and has the class `stale`.
Fix the file and save: the plate goes away by itself.

## 9. Eject: change a built-in widget

```text
rsb eject clock
```

copies `clock.luau` (and the `lib\` helpers it uses) into your `widgets\` folder.
Your copy overrides the built-in at once, and deleting it switches back. An
existing file is never overwritten (`kept your widgets\lib\format.luau`), and it
refuses when `widgets\clock.luau` already exists.

## 10. The rsb command

```text
rsb                       run the bar (or say it is already running)
rsb reload                re-read every config, style and widget file
rsb quit                  stop the bar and restore the work area
rsb toggle [bar]          hide or show every bar, or one by name
rsb popup <name> [open|close|toggle]  open a popup by name on the monitor under the pointer (counts as a click)
rsb status [--json]       widgets (state, calls, errors, memory), services, bars, memory as a table; --json is one line for scripts
rsb trigger <event> [json]  send an event to rsb.events.on(<event>)
rsb eject <widget>        copy a built-in widget into widgets\ to change it
rsb types                 write types\rsb.d.luau (and .luaurc, .vscode\settings.json when missing)
rsb render --out <png> [--width N] [--scale S] [--theme dark|light] [--preset P] [--live] [--wait-ms N]
rsb --help | --version
options: --config-dir <dir>  --log-dir <dir>
```

Exit codes: 0 success (also "already running"), 1 the command failed, 2 usage
error, 3 no bar is running, 4 the command pipe refused (not your user or session,
or no answer within 2 s), 5 RSB could not start. Only one RSB runs per user
session; a second start does nothing.

## Start, stop, logs, reset

- **Start.** Run `rsb.exe`. To start it with Windows, put a shortcut to it in the
  Startup folder (`shell:startup`). A second start does nothing.
- **Stop.** `rsb quit`, or the bar's right-click menu. The screen space comes
  back even when RSB has to exit on a fault.
- **Folders.** The config folder is `%APPDATA%\rsb`, or the folder given with
  `--config-dir` or the `RSB_CONFIG_DIR` environment variable. Logs are in
  `%LOCALAPPDATA%\rsb\logs` (or `--log-dir`): `rsb.log` and two older files of
  up to 5 MiB. Log times are local, with their UTC offset (`+10:00`).
- **Reset.** Quit RSB, delete `bar.toml` and `style.css` (or the whole config
  folder) and start it again: the next start writes fresh commented copies.
  Your `widgets\` folder is never touched.

## Editor setup (VS Code and luau-lsp)

RSB writes everything an editor needs into the config folder:

- `types\rsb.d.luau`: the typed `rsb` module, rewritten whenever RSB starts when it
  differs from this version's (do not edit it);
- `.luaurc`: `{ "languageMode": "strict", "aliases": { "rsb": "./types/rsb.d" } }`,
  which makes `require("@rsb")` typed;
- `.vscode\settings.json`: `luau-lsp.platform.type = "standard"` and the `*.luau`
  file association.

Install the **Luau Language Server** extension in VS Code, then open the config
folder (`code %APPDATA%\rsb`). In a widget file start with `--!strict`, then
`local rsb = require("@rsb")`: typing `rsb.` lists everything, hovering shows the
one-line description, and a misspelt function, a wrong prop (`rsb.text { colr =
"red" }`) or a style property is flagged as you type. `rsb types` rewrites the
type file (and the two editor files when they are missing) without starting the
bar. `.luaurc` and `settings.json` are yours to edit; RSB never overwrites them,
and writes one back at the next start only if it is missing.

## Examples

Three complete widgets live in `docs\examples\`. Copy one into `widgets\`, add
the `bar.toml` lines from its header, save.

| File | Shows |
|---|---|
| `weather.luau` | `rsb.http.get` in a poll, a `net` permission, `rsb.open_url` on click |
| `git-status.luau` | `rsb.exec.run` of one named program in a poll |
| `counter.luau` | `rsb.file.watch` of a file another program writes |

All three start with `--!strict` and use only the public API.

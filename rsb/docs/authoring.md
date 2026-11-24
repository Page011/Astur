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
`default` bar: workspaces and the window title on the left, the clock in the
middle, media, network, volume, battery, CPU, memory and the date on the right.

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
preset = "default"          # default | astur-classic | glass

[[bar]]                     # zero or more; none written = the preset's bars
name = "main"               # a-z 0-9 _ -, at most 32; also the name for: rsb toggle main
monitors = "*"              # "*", "primary", 0, "DISPLAY2", or a list of these
position = "top"            # top | bottom
height = 32                 # logical px, 16 to 128
floating = false            # true: a rounded bar kept away from the screen edge
margin = 8                  # floating only, 0 to 64
radius = 0                  # 0 to 64; 10 when floating; a CSS border-radius wins
reserve = true              # keep maximised windows clear of the bar
fullscreen = "below"        # below | hide | stay, what to do over a fullscreen app
left = ["workspaces", "title"]
center = ["clock"]
right = ["media", "network", "volume", "battery", "cpu", "memory", "date"]

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
| `workspaces` | `monitor = "own"`, `hide_empty = false`, `label = "display_name"`, `wheel = true` | one pill per workspace from Astur, GlazeWM or komorebi; a click focuses, the wheel steps; `own` shows the bar's monitor (matched by handle, then device name, then position; no match shows all), `all` shows every monitor; `label` is `display_name` or `name`. Needs `control = workspaces` |
| `title` | `max_width = 360`, `show_process = false` | the focused window's title with an ellipsis; empty on the desktop. Needs `read = window` |
| `clock` | `format = "HH:mm"`, `alt_format = "ddd d MMM HH:mm:ss"`, `timezone = "local"` | a click swaps to `alt_format` (set it to `""` to turn that off); ticks on the boundary of the format's finest unit, and at once after sleep or a clock change |
| `date` | `format = "ddd d MMM"`, `timezone = "local"` | ticks once an hour |
| `cpu` | `format = "{value}%"`, `icon = "cpu"`, `label = ""`, `every = "2s"`, `warn = 80` | class `warn` from `warn` percent up; a non-empty `label` replaces the icon |
| `memory` | `format = "{value}%"` (`{value}` `{used}` `{total}`), `icon = "memory"`, `label = ""`, `every = "2s"`, `warn = 85` | as `cpu` |
| `battery` | `format = "{value}%"`, `label = ""`, `absent = "icon"` | icon by level; classes `charging`, `low` (15% or less on battery), `saver`, `absent`; `absent` is `icon`, `text` or `hide` |
| `volume` | `format = "{value}%"`, `muted_format = ""`, `label = ""`, `step = 2` | the wheel changes the volume by `step` percent per notch, a click toggles mute; `muted_format` (say `"MUTE"`) replaces `format` while muted; classes `muted`, `absent` (no output device). Needs `control = audio` |
| `network` | `format = ""` (`{ssid}` `{down}` `{up}` `{kind}`), `every = "2s"` | an icon by connection kind and Wi-Fi strength; add a `format` for text; classes `offline`, `no-internet`. Rates are only read when the format uses them. Needs `read = network` |
| `media` | `format = "{artist} - {title}"` (`{album}` too), `max_width = 280`, `hide = "idle"` | an icon for what the player is doing now and the track; a click plays or pauses, a right click skips; `hide` is `idle` (hidden with no session) or `never`; a paused track still shows. Needs `control = media`, `read = media` |
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
| `default` | full width, translucent, dark or light with Windows, Segoe UI 13 px, Fluent icons, accent pills for workspaces |
| `astur-classic` | Astur's own bar: height 28, Segoe UI at 14 px, Astur's dark (`#C0CAF5` on `#1A1B26`, accent `#66AAFF`) and light (`#1B1E24` on `#ECEEF2`, accent `#336382`) colours, semibold text, workspaces left, title in the middle, `cpu memory volume battery clock` right, text labels `CPU`, `RAM`, `VOL`, `BAT` instead of icons (`VOL MUTE` while muted), square 34 px workspace pills with no gap and no outline, 16 px between widgets, 8 px at the edges. Differences that remain: the dark palette is the base and the light one follows the Windows theme (Astur's own default is dark unless its theme setting says otherwise); the title is centred on the bar, not in the space left between the zones; Astur's layout indicator has no RSB widget and is left out. Astur's "inactive" colour is below 4.5:1 contrast, so empty workspace labels use a lighter or darker shade |
| `glass` | floating (margin 8, radius 12), acrylic backdrop with a translucent fallback colour, each zone a pill, no shadow on the bar (the window edge would cut it) |

Every preset keeps text at 4.5:1 contrast or better against what is behind it,
dark and light, over a black or a white wallpaper.

A preset sets custom properties on `:root` (`--bg`, `--fg`, `--muted`, `--warn`,
`--error`, `--pill`, `--hover`, `--press`, `--accent`, `--accent-fg`). Change a
colour in your `style.css` by setting the property there.

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
compound      := ("*" | type | ":root")? ("." class | "#" id | ":hover" | ":active")*
type          := bar | zone | widget | box | text | icon | image | plate
```

`:root` is another name for `bar`. Specificity is (ids, classes plus pseudo
classes, types); later rules win ties. Not supported, each reported with its
line and what to use instead: `!important`, `:focus` and other pseudo classes,
attribute selectors, `+` and `~`, `@keyframes` (animation of keyframes comes in
a later version), `calc()`, `transform`, `text-shadow`, `inset` shadows, units
`pt` and `rem`.

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
| `overflow` | `visible \| hidden` | no | no |
| `box-shadow` | `none \| [<length>{2,4} <color>]#` | no | yes |
| `display` | `flex \| none` | no | no |
| `visibility` | `visible \| hidden` | yes | no |
| `transition` | `[<property> \| all] <time> [<easing>] [<time>]#` | no | no |
| `transition-property` | `[<property> \| all]#` | no | no |
| `transition-duration` | `<time>#` | no | no |
| `transition-delay` | `<time>#` | no | no |
| `transition-timing-function` | `<easing>#` | no | no |
| `cursor` | `default \| pointer` | yes | no |
| `backdrop` | `none \| acrylic \| mica` | no | no |

`transition` takes `property duration [easing] [delay]`, a list is allowed
(`transition: background-color 120ms ease-out, color 120ms`). `backdrop` works
on the bar only; `acrylic` and `mica` need Windows 11 and show the bar's own
translucent background where Windows draws a flat colour. A box has one border
colour: two sides with different colours warn and the last one wins. Text that
does not fit a box with `text-overflow: ellipsis` is cut with `...`.

Fonts: `font-family` takes quoted or bare names, `sans-serif`, `monospace` and
`icons` (Segoe Fluent Icons, or Segoe MDL2 Assets on Windows 10). A family that
is not installed is a warning on the bar and Segoe UI is used instead. A bar's
box shadow is cut off by its window and warns.

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

`rsb.popup`, `rsb.menu`, `rsb.tooltip`, `rsb.animate`, `rsb.settings`, `rsb.tray`,
`rsb.slider`, `rsb.input`, `rsb.graph`, `rsb.open` and `rsb.ipc` are reserved and
raise `rsb.popup is not available in this version`; so do the element props
`tooltip`, `popup`, `menu`, `animate`, `on_scroll`, `on_key` and `draggable`. A
few names people try raise a pointer: `rsb.sleep does not exist; use
rsb.task.sleep`. Any other misspelling gets a did-you-mean.

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
- `:set_volume(percent: number)`: set the volume, 0-100
- `:change_volume(delta: number)`: add delta (may be negative) to the volume
- `:set_muted(muted: boolean)`: mute or unmute
- `:toggle_mute()`: flip mute

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
- `:play_pause()`: toggle play and pause
- `:play()`: play
- `:pause()`: pause
- `:next()`: next track
- `:previous()`: previous track

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

### Icon names

`rsb.icon "<name>"` takes one of these (drawn in Segoe Fluent Icons or Segoe
MDL2 Assets), or `rsb.icon { glyph = "..." }` for any text in the icon font:

`volume_mute`, `volume_0`, `volume_1`, `volume_2`, `volume_3`, `wifi_0`, `wifi_1`, `wifi_2`, `wifi_3`, `wifi_4`, `wifi_off`, `ethernet`, `network_off`, `battery_0`, `battery_1`, `battery_2`, `battery_3`, `battery_4`, `battery_5`, `battery_6`, `battery_7`, `battery_8`, `battery_9`, `battery_10`, `battery_charging`, `battery_saver`, `battery_none`, `cpu`, `memory`, `clock`, `calendar`, `play`, `pause`, `next`, `previous`, `music`, `window`, `desktop`, `cloud`, `sun`, `moon`, `rain`, `snow`, `settings`, `power`, `search`, `bell`, `globe`, `download`, `upload`, `warning`, `error`, `info`

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
- **control**: `audio`, `media`, `workspaces` (the commands of those services).
- **read**: `window`, `media`, `network` (values that name what you are doing).
  Widget files in your config folder get these three without asking; packages
  would have to.
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

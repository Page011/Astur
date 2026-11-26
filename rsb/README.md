# RSB

A status bar for Windows 10 and 11, written in Rust.

- Native and light: a CPU renderer into a layered window, no browser engine,
  no GPU device.
- Three ways in: pick a preset, edit `bar.toml` and `style.css` (applied on
  save), or write widgets in Luau with live data, commands, files and HTTP.
  The built-in widgets are written in the same public Luau API.
- Sandboxed scripts: each widget declares what it may run, read or reach,
  and runs under a time budget and a memory cap.
- Workspaces from GlazeWM-compatible window managers and komorebi; works
  with no window manager at all.
- Errors show up on the bar as a plate with the file and line, never as a
  blank bar; the last good config keeps running until you fix the file.

## Status

Early development. Not ready for daily use.

## The default bar

With no `bar.toml` you get one bar per monitor along the top edge, 36 px tall,
reserving its space, dark or light with Windows. It is drawn as chevron-edged
segments in three tones with an accent, in JetBrains Mono with SVG icons:

- left: workspaces and the media control (title and artist, a progress line
  along its lower edge, a click opens the player panel);
- centre: memory, CPU, the dashboard button, the clock (hover for seconds) and
  the date (a click opens a calendar), with the button centred on the bar;
- right: microphone and Bluetooth at a glance, battery, an empty tray segment
  that keeps the group's shape, the screen recorder and the power button.

The dashboard button opens a panel that grows out of it: quick tiles (power
mode, Bluetooth, audio output, apps, record, power menu) and a page for each,
with a searchable app launcher (`rsb popup launcher` opens it from a hotkey
tool). The power button opens a full-screen menu (Power off, Restart, Sleep,
Lock) that needs a deliberate click or Enter and never forces programs closed.
The `volume` and `network` widgets, which are not in the default bar, bring
panels of their own. Everything above is a Luau widget file written against
the public API, so `rsb eject <widget>` copies any of them for you to change.
Other presets (`fluent`, `glass`, `astur-classic`) are selected with
`preset = "<name>"` in `bar.toml`.

## Build

Requires a recent stable Rust toolchain and the MSVC build tools.

```text
cargo build --release
```

The binary is `target\release\rsb.exe`; it needs nothing next to it (the
built-in widgets, presets and themes are inside it). The checks:

```text
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

A few tests show a window or reserve screen space; they are ignored by
default and run on purpose with
`cargo test --workspace -- --ignored --test-threads=1`. Two of them take the
foreground or watch the real mouse for ten seconds and only run with
`RSB_TEST_FOREGROUND=1` or `RSB_TEST_REAL_HOOK=1`.

## Run

```text
rsb                       run the bar (with no config you get the default bar)
rsb reload                re-read bar.toml, style.css and every widget file
rsb quit                  stop the bar and give the screen space back
rsb toggle [bar]          hide or show every bar, or one by name
rsb status [--json]       widgets, services, bars and memory as a table (--json: one line, for scripts)
rsb trigger <event> [json]  send an event to rsb.events.on(<event>)
rsb popup <name> [open|close|toggle]  open, close or toggle a popup (for a hotkey tool)
rsb eject <widget>        copy a built-in widget into widgets\ to change it
rsb types                 write types\rsb.d.luau for luau-lsp autocomplete
rsb render --out bar.png  draw the bar to a PNG without a window
rsb --help
```

A start creates `%APPDATA%\rsb` (or the folder in `--config-dir` or
`RSB_CONFIG_DIR`) if needed, and writes whatever is missing from a commented
`bar.toml`, an empty `style.css`, an empty `widgets\` folder and the Luau types.
Logs go to `%LOCALAPPDATA%\rsb\logs` (or `--log-dir`). To start the bar with Windows, put a shortcut to `rsb.exe`
in the Startup folder.

Fonts: JetBrains Mono is built in. To add another, put the file in the `fonts`
folder of the config folder and name it in `style.css`:

```css
@font-face { font-family: "My Font"; src: url("fonts/my-font.ttf"); }
```

Files must be in that folder (at most 16 files of 16 MiB each); a missing or
unreadable one shows on the bar plate. Icons can be `.svg` files too, if the
widget is allowed to read them.

Preview without a window, with sample values or this PC's data:

```text
rsb render --out bar.png --theme light --width 1600
rsb render --out bar.png --live
rsb render --out glass.png --preset glass
```

A scripted run with a virtual clock writes pictures of the bar and its popups
at chosen moments, for checking an animation frame by frame:

```text
rsb render --timeline moves.txt --out-dir shots --screen 1920x1200 --now 2026-11-25T11:10:00
```

Each line of the file is `<ms> pointer main move 1100 18`, `<ms> popup <name>
open`, `<ms> fake <service> {"muted": true}` or `<ms> capture <name> [screen]`.
The output folder must be under a `scratch` or `target` folder.

## Docs

- `docs/authoring.md`: `bar.toml`, the CSS subset, and the Luau API, with
  examples in `docs/examples`.
- `docs/architecture.md`: crates, threads, data flow and the stop order.

## Licence

Apache-2.0. See `LICENSE`.

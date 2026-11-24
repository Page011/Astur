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
`cargo test --workspace -- --ignored --test-threads=1`.

## Run

```text
rsb                       run the bar (with no config you get the default bar)
rsb reload                re-read bar.toml, style.css and every widget file
rsb quit                  stop the bar and give the screen space back
rsb toggle [bar]          hide or show every bar, or one by name
rsb status [--json]       widgets, services, bars and memory as a table (--json: one line, for scripts)
rsb trigger <event> [json]  send an event to rsb.events.on(<event>)
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

Preview without a window, with sample values or this PC's data:

```text
rsb render --out bar.png --theme light --width 1600
rsb render --out bar.png --live
rsb render --out glass.png --preset glass
```

## Docs

- `docs/authoring.md`: `bar.toml`, the CSS subset, and the Luau API, with
  examples in `docs/examples`.
- `docs/architecture.md`: crates, threads, data flow and the stop order.

## Licence

Apache-2.0. See `LICENSE`.

# Changing RSB's settings from a program

`rsb settings` reads and changes the settings that matter most (the preset, a
bar's position, height and floating, which widgets sit in which zone, and the
accent colour) without opening a file. It is made for programs: a settings
page, a script, a hotkey tool. Every command prints **one line of JSON** on
standard output, errors included, and exits with 0, 1 or 2 (see
[Exit codes](#exit-codes)).

The commands edit `bar.toml` and `style.css` in place. Your comments, blank
lines and line endings stay as they are; only the value, the line or the list
item that changes is touched. A change you made by hand in between is reported
and kept, never overwritten. A comment at the end of a commented example line that
a change turns live stays as written, even when it describes the old list.

## Commands

```text
rsb settings get [--bar NAME]    the settings in effect and where each came from
rsb settings presets             the presets, with what a gallery page shows
rsb settings preview <preset>|--all [--theme dark|light|both] [--width N] [--scale S] [--mine [--zones mine|preset]]
rsb settings widgets             every widget, its options and where it is used
rsb settings schema              every key these commands may change, with types and limits
rsb settings set <key> <value>   e.g. preset glass, bar.main.height 40, color.accent #e5a50a,
                                 bar.main.right '["status","clock"]'
rsb settings unset <key>         back to the preset's value (removes your line)
rsb settings move <widget> <bar>.<zone>[:<index>] [--from BAR[.ZONE[:INDEX]]]
rsb settings add <widget> <bar>.<zone>[:<index>]
rsb settings remove <widget> [--from BAR[.ZONE[:INDEX]]]
rsb settings change <file.json | ->   several changes at once, all or none (- reads stdin)
rsb settings undo [N]            undo the last settings change (N must be the newest)
rsb settings restore <bar.toml|style.css>   put back the last copy that loaded clean
rsb settings log [--last N]      the settings changes that can be undone, newest first
```

Options on every command:

| Option | Meaning |
|---|---|
| `--config-dir DIR` | The folder with `bar.toml` and `style.css` (default `%APPDATA%\rsb`, or `RSB_CONFIG_DIR`). A device name such as `COM1` or `NUL` is refused with `bad_usage`. |
| `--log-dir DIR` | Where `rsb.log` is written (default `%LOCALAPPDATA%\rsb\logs`). |
| `--contract N` | The reply version the caller understands (see [Versions](#versions)). |

Options on the commands that write (`set`, `unset`, `move`, `add`, `remove`,
`change`, `undo`, `restore`):

| Option | Meaning |
|---|---|
| `--expect bar.toml=VERSION`, `--expect style.css=VERSION` | The version of the file you read, from `files.<file>.hash` of a `get`. A file the change writes must still have it, else the change is refused with `file_changed` and nothing is written. Checked only for files the change writes. |
| `--dry-run` | Run every check and print the same reply, but write nothing, log nothing and make no undo entry. |
| `--no-wait` | Do not ask the running bar whether it shows the change. |

`set` also takes `--reset` (with `set preset`: also drop your own bar keys,
zone lists and accent colour, in one undoable change). `get` takes `--bar
NAME`. `move` and `remove` take `--from BAR`, or `--from BAR.ZONE` and `--from BAR.ZONE:INDEX` when
the widget is in the bar more than once. `log` takes `--last N`. `preview` takes `--cache-dir DIR`,
`--theme`, `--width`, `--scale`, `--mine`, `--zones` and `--all`.

`<value>` is read as JSON when it parses (`40`, `true`, `["status","clock"]`)
and as a plain word otherwise (`glass`, `#e5a50a`). `<index>` counts from 0;
left out or `-1` means the end. `<bar>` is a bar name; a number counts as an
index only when no bar has that name. A mistake in the words gives a
`bad_usage` reply with the usage line in `next`, and a misspelt command gets a
"did you mean" hint.

`change` reads at most 65,536 bytes of JSON from a file, or from standard input
when the file is `-` (a byte order mark is accepted). Options on the command
line win over the same fields in the file.

## The request

Every writing command builds this request; `change` takes it as JSON, and the
running bar takes it on its pipe (see [Over the pipe](#over-the-pipe)).

```json
{"contract":1,"verb":"change",
 "expect":{"bar.toml":"fnv1a64:3f0c9a1e5b7d2c44","style.css":"fnv1a64:91aa04c3e2b8f610"},
 "reset":false,"dry_run":false,
 "changes":[
  {"op":"set","key":"preset","value":"fluent"},
  {"op":"set","key":"color.accent","value":"#e5a50a"},
  {"op":"set","key":"bar.main.left","value":["workspaces","media"]},
  {"op":"unset","key":"bar.main.height"},
  {"op":"move","widget":"clock","from":"main","to":{"bar":"main","zone":"right","index":0}},
  {"op":"add","widget":"clock#utc","to":{"bar":"main","zone":"right","index":-1}},
  {"op":"remove","widget":"media","from":"main"}]}
```

- `verb` is one of `get`, `presets`, `widgets`, `schema`, `change`, `undo`,
  `restore`, `log`. (`preview` is a command line command only.) A file given to
  `change` may leave it out.
- `contract` is optional (see [Versions](#versions)).
- `get` takes `bar`; `log` takes `last`; `undo` takes `n`; `restore` takes
  `file` (`bar.toml` or `style.css`).
- Each change has an `op`: `set` and `unset` take a `key` (and `set` a `value`); `move`, `add` and `remove` take a `widget`; `move` and `add` take `to`, the place `{bar, zone, index}`.
- `from` is needed only when the widget is placed on several bars. It may also
  be the exact place, `{"bar":"main","zone":"center","index":2}`, when the
  widget is twice in one zone.
- `reset` applies to `set preset` only.
- All changes are checked first; a refusal means nothing is written. Each file
  is then written once.
- `undo` and `restore` are commands of their own: a `change` request that holds
  them is refused.

### What can be changed

| Key | File | Accepted | Written as |
|---|---|---|---|
| `preset` | `bar.toml` | one of `default`, `fluent`, `astur-classic`, `glass` | `preset = "fluent"` |
| `bar.<bar>.position` | `bar.toml` | `top`, `bottom` (any case) | `position = "bottom"` |
| `bar.<bar>.height` | `bar.toml` | an integer from 16 to 128 (logical px) | `height = 40` |
| `bar.<bar>.floating` | `bar.toml` | `true` or `false` | `floating = true` |
| `bar.<bar>.left`, `bar.<bar>.center`, `bar.<bar>.right` | `bar.toml` | a whole list of at most 64 widget refs, or `move` / `add` / `remove` | `right = ["clock", "clock#utc"]` |
| `color.accent` | `style.css` | `#rgb` or `#rrggbb` | the colours block (below) |

`rsb settings schema` lists these with their limits, generated from the same
tables the parser reads; `docs\settings-schema.json` is its committed output.
Everything else (a bar's `name`, `monitors`, `margin`, `radius`, `reserve`,
`fullscreen`, `center_on`, adding or removing a bar, widget options, CSS rules)
is changed in the files; asking for it gives `file_only`.

The accent is kept as one marked block at the end of `style.css`, the same for
every preset, with both colour schemes written:

```css
/* Colours set by rsb settings. Edit them here or on the settings page. */
@media (prefers-color-scheme: dark) {
    :root { --accent: #e5a50a; --accent-fg: #000000; --accent-hover: #e8ae22; --accent-press: #eab73b; --accent-text: #e5a50a; --accent-line: #e5a50a; }
}
@media (prefers-color-scheme: light) {
    :root { --accent: #e5a50a; --accent-fg: #000000; --accent-hover: #e8ae22; --accent-press: #eab73b; --accent-text: #6e4f05; --accent-line: #9c7007; }
}
/* End of colours set by rsb settings. */
```

The accent you pick is kept exactly as `--accent`. Its text, line, hover,
press and foreground shades are derived for each scheme so every pair a preset
draws stays readable. `unset color.accent` removes the block. A preset switch
never rewrites `style.css`.

## Replies

Every reply is one JSON object with `contract` (the version, a number) and
`ok` (`true` for a document, `false` for a refusal). A document also has
`kind`, which names its shape. All text is ASCII; other characters are written
as `\u` escapes.

### `get`

```json
{"contract":1,"ok":true,"kind":"settings","config_dir":"C:\\...\\rsb",
 "files":{"bar.toml":{"hash":"fnv1a64:3f0c9a1e5b7d2c44","error":null,"edited_by_hand":false,"last_good":true},
          "style.css":{"hash":"fnv1a64:91aa04c3e2b8f610","error":null,"edited_by_hand":true,"last_good":true}},
 "preset":{"value":"fluent","source":"user","at":"bar.toml:10:1"},
 "bars":[{"name":"main","index":0,
   "position":{"value":"top","source":"preset","at":null,"editable":true},
   "height":{"value":40,"source":"user","at":"bar.toml:18:1","editable":true},
   "floating":{"value":false,"source":"default","at":null,"editable":true},
   "left":{"value":["workspaces","media"],"source":"preset","at":null,"editable":true},
   "center":{"value":["clock"],"source":"preset","at":null,"editable":true},
   "right":{"value":["clock","media"],"source":"user","at":"bar.toml:21:1","editable":true},
   "center_on":{"value":null,"source":"default","at":null,"editable":false},
   "monitors":{"value":["*"],"source":"default","at":null,"editable":false},
   "margin":{"value":0,"source":"default","at":null,"editable":false},
   "radius":{"value":0,"source":"default","at":null,"editable":false},
   "reserve":{"value":true,"source":"default","at":null,"editable":false},
   "fullscreen":{"value":"below","source":"default","at":null,"editable":false}}],
 "colors":{"accent":{"label":"Accent","editable":true,
   "dark":{"value":"#e5a50a","source":"user","at":"style.css:42:5","shadowed_by":null,"overridden":[]},
   "light":{"value":"#e5a50a","source":"user","at":"style.css:45:5","shadowed_by":null,
            "overridden":["style.css:12:5"]}}},
 "high_contrast":false,
 "bar":{"running":true,"shows_files":true,"diagnostics":0,"config_dir":"C:\\...\\rsb"}}
```

| Field | Meaning |
|---|---|
| `kind` | `settings`. |
| `config_dir` | The folder this call read. |
| `files` | One entry for each of `bar.toml` and `style.css`, holding `hash`, `error`, `edited_by_hand` and `last_good`. |
| `files.<file>.hash` | The file's version: `fnv1a64:` and 16 hex digits of the raw bytes (a byte order mark included), or `none` for a file that does not exist. Null when the file could not be read. Send it back as `--expect`. |
| `files.<file>.error` | Null, or `{"at":"bar.toml:7:3","message":"..."}` when the file does not parse (`at` is `file:line:column`). |
| `files.<file>.edited_by_hand` | The file differs from what settings last wrote there. False when settings never wrote it. |
| `files.<file>.last_good` | A copy of the file that loaded clean is kept, so `restore` can run. |
| `preset` | `{value, source, at}` for the preset in effect; null when `bar.toml` does not parse. |
| `bars` | One entry per bar (or the one named by `--bar`); null when `bar.toml` does not parse. |
| `bars[].name`, `bars[].index` | The bar's name and its position in the list. |
| `bars[].<key>` | `position`, `height`, `floating`, `left`, `center`, `right`, `center_on`, `monitors`, `margin`, `radius`, `reserve`, `fullscreen`, each as `{value, source, at, editable}`. |
| `source` | Where the value came from: `default` (nothing set it), `preset`, or `user` (a line in your file). |
| `at` | `file:line:column` of your line when `source` is `user`, else null. |
| `editable` | Whether `set` can change it (the keys in the table above). |
| `colors` | `{accent: {...}}`; null when the style files do not parse. |
| `colors.accent.label`, `colors.accent.editable` | `"Accent"`, and whether `set color.accent` works (true). |
| `colors.accent.dark`, `colors.accent.light` | The accent in each scheme: `value` (`#rrggbb`, or null), `source`, `at`. |
| `shadowed_by` | `file:line:column` of a later rule of yours that wins over the colours block, else null. |
| `overridden` | A list of `file:line:column` of declarations of an accent colour that come before the colours block and so no longer apply. |
| `high_contrast` | Windows' high contrast mode is on. RSB only reports it; nothing on the bar changes. |
| `bar` | The running bar's side: see below. |

`bar` is `{running, shows_files, diagnostics, config_dir}`. `running` is false
when no bar is running (every other field is then null; that is not an error).
`shows_files` is true when the bar shows exactly the files on disk, false when
it shows an older version (it kept the last good one, or has not reloaded yet),
and null when the bar reads another folder. `diagnostics` is the number of
problems the bar is showing, and `config_dir` the folder it reads.

A broken file nulls only what it feeds: `bar.toml` feeds `preset` and `bars`,
`style.css` and the files it imports feed `colors`. The other file's fields
stay.

### `presets`

```json
{"contract":1,"ok":true,"kind":"presets","current":"default","high_contrast":false,
 "presets":[{"name":"default","title":"Default",
   "description":"Chevron segments along the top edge, with a panel for every widget.",
   "schemes":["dark","light"],"bars":1,"bar":{"position":"top","height":36,"floating":false},
   "tune":[{"var":"accent","label":"Accent","editable":true,"dark":"#81cbea","light":"#81cbea"}],
   "contrast":{"dark":{"pass":true,"text":{"lowest":4.67,"what":"placeholder on input","need":4.5},
                       "graphic":{"lowest":3.02,"what":"dim on row","need":3.0}},
               "light":{"pass":true,"text":{"lowest":4.59,"what":"mem on seg1-hover","need":4.5},
                        "graphic":{"lowest":3.46,"what":"dim on seg0","need":3.0}}}}]}
```

| Field | Meaning |
|---|---|
| `kind` | `presets`. |
| `current` | The preset in your `bar.toml` (null when the file does not parse). |
| `high_contrast` | As in `get`. |
| `presets` | One entry per built-in preset. |
| `presets[].name`, `title`, `description` | The name used in `bar.toml`, and what a gallery shows. |
| `schemes` | The colour schemes the preset defines (`dark`, `light`). |
| `bars` | How many bars the preset has. |
| `bar` | The preset's first bar: `position`, `height`, `floating`. |
| `tune` | The colours worth offering a picker for, in page order: `var` (the custom property without `--`), `label`, `editable` (only the accent is), and its value in each scheme as `dark` and `light`. |
| `contrast` | The preset as shipped, per scheme. `pass` is true when no pair is under its limit. `text` and `graphic` each give `lowest` (the lowest ratio found), `what` (which pair) and `need` (the limit: 4.5 for text, 3.0 for icons and lines). |

### `preview`

Command line only. Draws each preset's bar to a PNG with sample values and a
held clock, so the picture is stable, and keeps it in a cache folder
(`--cache-dir`, default `%LOCALAPPDATA%\rsb\previews`). Without `--mine` the
preset is drawn as shipped; with `--mine` it is drawn with your `bar.toml` and
`style.css` and only the preset swapped, which is what switching would show.
With `--mine --zones preset` the bar's zone lists are the preset's own and
everything else (accent, height, floating, ...) is yours: what "use the preset's
layout" would show. `--mine` refuses with `file_broken` when one of your files
does not parse, rather than drawing an error plate. Defaults: `--theme both`,
`--width 1920`, `--scale 1`. `--width` is 200 to 7680 and `--scale` 0.5 to 4; a
preset with a wide centre zone needs 1280 or more to show all three zones, so use
that or wider for a thumbnail strip.

```json
{"contract":1,"ok":true,"kind":"preview","previews":[{"name":"glass",
 "theme":"dark","mine":false,"zones":"mine","path":"C:\\...\\previews\\glass-dark-1920x100-<key>.png","width":1920,
 "height":56,"scale":1.0,"cached":true}]}
```

| Field | Meaning |
|---|---|
| `previews` | One entry for each preset and scheme asked for. |
| `previews[].name`, `theme` | The preset and the scheme (`dark` or `light`). |
| `mine` | True for `--mine`. |
| `zones` | `preset` for `--mine --zones preset`, else `mine`. |
| `path` | The PNG. It is written to a temporary file and renamed, so it is never half written. |
| `width`, `height` | The PNG's size in pixels. |
| `scale` | The scale it was drawn at. |
| `cached` | It was found in the cache and not drawn. |

The cache name holds a key over everything the picture depends on, so a changed
preset, `style.css` (with `--mine`) or RSB version draws a new file. A file by that
name that is not a picture of the width asked for is drawn again.

### `widgets`

Command line only: it reads every widget module, which is not work for the
running bar.

```json
{"contract":1,"ok":true,"kind":"widgets",
 "widgets":[{"name":"media","source":"builtin","replaces_builtin":false,
   "title":"Media","visible":true,"needs_options":false,"opens":["media"],
   "summary":"The current media session: a play or pause icon and the title.",
   "options":[{"key":"max_width","type":"number","default":280},
              {"key":"hide","type":"string","default":"idle"}],
   "used":[{"bar":"main","zone":"left","index":1,"ref":"media"}]}],
 "failed":[{"name":"my/weather","source":"widgets/my/weather.luau","message":"..."}]}
```

| Field | Meaning |
|---|---|
| `widgets` | Every built-in widget and every file under `widgets\`, by name. |
| `widgets[].name` | The ref as written in `bar.toml` (`media`, `my/weather`). |
| `source` | `builtin`, or `user` for a file of yours. |
| `replaces_builtin` | A file of yours has the name of a built-in and takes its place. |
| `title` | A short name for a page (at most 20 characters). |
| `visible` | False for an anchor that draws nothing, such as `power`. |
| `needs_options` | True when the widget draws nothing until its options are set, such as `text`. |
| `opens` | The popups the widget hosts or opens. |
| `summary` | The first sentence of the comment at the top of the module. |
| `options` | Each option with its `key`, `type` (`string`, `number`, `bool`, `table`, or `any` for an option with no default) and `default` (null for `any`). Alphabetical. |
| `used` | Where the widget is placed in the config in effect: `bar`, `zone`, `index` and `ref` (the ref as written, `clock#utc`). |
| `failed` | Files that could not be described: `name`, `source` (the file, relative to the config folder) and `message`. |

A widget describes itself with an optional `about` table in `rsb.widget {}`:
`title`, `visible`, `needs_options`, `opens`. The module is evaluated once in a
sandbox that has no natives: a call to `rsb.exec`, the network, files, timers
or any other `rsb` function raises "not available while listing widgets" and
starts nothing, and `setup` is never called. A module that takes longer than
250 ms, or uses more than 16 MiB, is listed under `failed`.

### `schema`

```json
{"contract":1,"ok":true,"kind":"schema","config_version":1,
 "keys":[{"key":"preset","file":"bar.toml","type":"choice","label":"Preset","help":"...","values":["default","fluent","astur-classic","glass"],"default":"default"},
         {"key":"bar.*.height","file":"bar.toml","type":"int","min":16,"max":128,"unit":"logical px","default":36},
         {"key":"bar.*.left","file":"bar.toml","type":"zone","max_items":64,"label":"Left zone","help":"...","default":["workspaces","title"]},
         {"key":"color.accent","file":"style.css","type":"color","schemes":["dark","light"]}],
 "file_only":["config_version","bar.*.name","bar.*.monitors"]}
```

| Field | Meaning |
|---|---|
| `config_version` | The `config_version` this build reads and writes. |
| `keys` | One entry for each settable key (`*` stands for any bar). |
| `key`, `file` | The key, and the file it is written in. |
| `label`, `help` | A short name and one line for a person, so a page need not write its own. |
| `type` | `choice` (with `values`), `int` (with `min`, `max`, `unit`), `bool`, `zone` (with `max_items`; set with a whole list) or `color` (with `schemes`). |
| `default` | The value when nothing sets it. For a zone, the built-in bar's list. |
| `values`, `min`, `max`, `unit`, `max_items`, `schemes` | The limits of the type. |
| `file_only` | Keys that exist but are changed in the file only. |

### `log`

```json
{"contract":1,"ok":true,"kind":"log",
 "entries":[{"n":18,"time":"2026-11-27T10:15:00+11:00","what":"style.css: accent none -> #e5a50a",
             "changes":[{"op":"set","key":"color.accent","value":"#e5a50a"}],
             "files":{"style.css":{"before":"none","after":"fnv1a64:91aa04c3e2b8f610"}}}]}
```

`entries` is a list, newest first, of at most the last 20 changes. `n` is the entry's number, `time`
the local time with its offset, `what` the log text, `changes` the validated
changes as served (a `restore` entry holds `{"op":"restore","file":"bar.toml"}`; an
`undo` makes no entry of its own, it removes the entry it undid), and `files` the version of each file
`before` and `after` (`none` = the file did not exist).

### Change result

For `change` and every command that writes (`set`, `unset`, `move`, `add`,
`remove`, `undo`, `restore`):

```json
{"contract":1,"ok":true,"kind":"change","dry_run":false,
 "files":{"bar.toml":{"hash":"fnv1a64:77d1f0e4a2c95b38","written":true,"edited_by_hand":false},
          "style.css":{"hash":"fnv1a64:91aa04c3e2b8f610","written":false,"edited_by_hand":true}},
 "undo":17,"preset_switch":null,"accent":null,"kept_copy":null,
 "notices":[{"code":"hand_edit_kept","file":"bar.toml",
             "message":"bar.toml was edited by hand since the last settings change; that edit is kept",
             "next":null}],
 "bar":{"running":true,"shows_files":true,"ms":61,"diagnostics":0,"config_dir":"C:\\...\\rsb"}}
```

| Field | Meaning |
|---|---|
| `kind` | `change`. |
| `dry_run` | The request was a dry run: `written` is false everywhere and nothing was logged. |
| `files.<file>.hash` | The file's version now (what it would be, for a dry run). |
| `files.<file>.written` | The change wrote this file. |
| `files.<file>.edited_by_hand` | For a file the change did not write: it differs from what settings last wrote, so a page learns of a hand edit to the other file without the change being refused. |
| `undo` | The undo entry this change made; null for an `undo`, and for a dry run. |
| `preset_switch` | Only for `set preset`, else null. What your own lines still override, so a page can offer "use the preset's" through `unset`. |
| `accent` | Only for `set color.accent`, else null. The derived colours. |
| `kept_copy` | After a `restore`: where the text it replaced was kept (`undo` brings it back). Otherwise null. |
| `notices` | What the change did besides what was asked. |
| `bar` | The running bar's side: see below. |

`preset_switch` is `{from, to, kept, bars_listed, preset_bars, accent_kept}`.
`kept` lists, for each key of yours that outlives the switch: `bar`, `key`,
`yours` (your value), `preset` (the new preset's value) and `same`. `same: true`
means the two are equal now, but your line no longer follows the preset's future
changes. `bars_listed` is how many `[[bar]]` tables your file has and
`preset_bars` how many the new preset has. `accent_kept` is your accent colour,
or null.

`accent` is `{value, schemes}`; `schemes.dark` and `schemes.light` each hold the
derived tokens `fg`, `fg_ratio`, `hover`, `press`, `text`, `line` and `moved`
(how far, in percent, `text` and `line` were moved from the accent to stay
readable).

`bar` is `{running, shows_files, ms, diagnostics, config_dir}`, with the meaning
it has in `get` plus `ms`, the time at which the bar was first seen to show the new
files. The bar reports a version only after it has drawn the frame that shows it, so a
caller that sees `shows_files` true can read the new look from `rsb inspect`. After a write, the command line asks the running bar which file versions it
shows: once after 5 ms, then every 25 ms, for at most 2000 ms (a style change takes
about 40 ms; a zone move about 30 ms), so `ms` is a time in 25 ms steps, not the bar's
own reload time. `config_dir` is written the way `get` writes it, without a `\\?\`
prefix. If the bar's `config_dir` differs from the one the command
works in, it does not wait: `shows_files` and `ms` are null and the notice
`bar_uses_other_folder` says where the bar reads. If the bar has not shown the
files within 2000 ms, `shows_files` is false and the notice `not_confirmed` is
added; the exit code is still 0, because the change is saved. A bar that already shows
the written versions (a `restore` of what it shows) is also waited for until it has loaded
the file once more, so `diagnostics` is the count after the reload; if it does not
within 2000 ms the reply is given as it stands, with no notice. With `--no-wait`
or a dry run nobody is asked and `running`, `shows_files`, `ms`, `diagnostics`
and `config_dir` are all null. `ms` is null whenever there was no waiting. Over the
pipe nobody is waited for either: `running`, `diagnostics` and `config_dir` are the
bar's own record at the moment of the write, `shows_files` says whether it already
showed the new files (usually not yet) and `ms` is null.

### Notices

A notice is `{"code","file"?,"message","next"}` plus the fields named here.
`message` is for the person; `next` is null when nothing needs doing, or a hint that
may name a command to run. A notice that has a remedy a page can act on also has a
`fix` (see [Errors](#errors)). Which notices a page shows to a person: `hand_edit_kept`,
`now_yours`, `unpinned`, `popup_unreachable`, `shadowed`, `replaces_yours`,
`block_edit_replaced`, `already_placed`, `not_confirmed`, `bar_uses_other_folder` and
`bar_unreachable`. `example_turned_live`, `file_created` and `bars_now_listed` describe
how the file was edited and belong in a log, not on a page.

| Code | When | Fields besides the common ones |
|---|---|---|
| `hand_edit_kept` | No `--expect` was given and the file differs from what settings last wrote; the change was made on the text as it is. For `style.css` with a colours block the message says that edits outside the block are kept, and the notice is left out when every changed line is inside the block (`block_edit_replaced` says that one is replaced or removed, but only when a colour of the block now differs from the shades of its accent: an edit inside the block that changes no colour, such as spacing or capitals, is not reported). | |
| `file_created` | The file or folder was missing and was written from the template first. | |
| `example_turned_live` | A commented example line of the template (`# preset = "default"`) was turned into the live line. | `line` |
| `bars_now_listed` | `[[bar]]` tables were inserted or uncommented, so the file now fixes the number of bars. | `count` |
| `now_yours` | A zone list the preset set is now written in your file. | `zones` (a list of `{bar, zone}`), `fix` (a list of `unset` changes, one for each zone) |
| `unpinned` | A `center_on` of yours that named the moved widget was removed. | `widget` |
| `popup_unreachable` | The removed widget was the last one that opens a popup; one notice for each popup. After a preset switch the message says what the preset lacks. Not checked over the pipe, which runs no widget module. | `popup`, `widget` |
| `shadowed` | A later rule of yours wins over the colours block; the message names the token (`--accent-text`). Found in `style.css` and in the files it imports, the same places `get` reports as `shadowed_by`. | `at` |
| `replaces_yours` | An earlier declaration of an accent colour of yours, in `style.css` or in a file it imports, now does nothing, the colours block beats it. | `at` (a list) |
| `block_edit_replaced` | A colour you edited by hand inside the colours block was replaced with the shades of the accent, or, for `unset color.accent` and for `set preset <name> --reset`, removed with the block; `undo` brings it back. | |
| `already_placed` | `add` put a widget on a bar where it already was; the change is made. | `places` (a list of `zone:index`) |
| `not_confirmed` | The bar did not show the new files within 2000 ms. | |
| `bar_uses_other_folder` | The running bar reads another config folder. | `bar_dir` |
| `bar_unreachable` | The bar's pipe exists but refused or did not answer. | |

## Errors

```json
{"contract":1,"ok":false,"error":{"code":"file_changed",
 "file":"bar.toml","at":null,"message":"...","next":"...","fix":{"verb":"get"},"copies":[]}}
```

The refusal is the `error` object: `code`, `file`, `at`, `message`, `next`, `fix` and `copies`. A refusal also prints `rsb: <message>` on standard error. `message`
is for the person and is shown as it is. `next` is a hint for a person at a terminal and
may name a command (`rsb settings restore bar.toml`); a page does not parse it. `fix` is
the remedy as data, or null when the person has to look: `{"verb":"restore","file":"bar.toml"}`,
`{"verb":"get"}`, `{"verb":"undo","n":3}`, `{"op":"unset","key":"bar.main.center"}` or
`{"open":"<path>"}` (a file the person can open and edit). A page acts on `fix`. `file` is the file concerned
(or null), `at` is `file:line` or `file:line:column` (the column only when the message is about
one) in a file, and `copies` lists the paths of before-copies that were kept when a
rollback could not run, or that `undo_conflict` names.

| Code | Exit | Meaning |
|---|---|---|
| `bad_usage` | 2 | The words are wrong; `next` is the usage line. Also: a folder that is not an RSB config folder. |
| `bad_json` | 2 | The request is not one JSON object, or `expect` names a file other than `bar.toml` and `style.css`. |
| `contract_too_new` | 2 | The caller wants a newer reply version than this build speaks. |
| `contract_too_old` | 2 | The caller's version is no longer served. |
| `cli_only` | 2 | `widgets` and `preview` are answered by `rsb.exe`, not over the pipe; a write over the pipe is served only to a program at the bar's own level of rights; a reply too long for the pipe. |
| `unknown_key` | 2 | The key does not exist; the message has a "did you mean". |
| `file_only` | 2 | The key is changed in the file only. |
| `bad_value` | 2 | The value is not allowed; the message gives the limits. |
| `unknown_preset` | 2 | No such preset (names are lower case; `Glass` gets a hint for `glass`). |
| `unknown_bar` | 2 | No such bar; the message lists the bars. |
| `unknown_widget` | 2 | No such widget. |
| `widget_not_placed` | 2 | The widget is in no zone of that bar. |
| `ambiguous_widget` | 2 | The widget is on several bars; say which with `--from`. |
| `file_broken` | 1 | The file does not parse (`at` says where), or a file that `style.css` imports does not and the change would write `style.css`. Fix that line, or run `rsb settings restore` when a copy that loaded clean is kept (`fix` says which). |
| `elevated` | 1 | RSB runs as administrator; settings are not changed from an administrator process. |
| `config_too_new` | 1 | The file's `config_version` is newer than this build reads. |
| `file_changed` | 1 | A file changed since it was read (an `--expect` did not match, or someone saved between the read and the write). Nothing was written. |
| `busy` | 1 | Another settings change is being written. |
| `read_only` | 1 | The file is read-only. |
| `linked_file` | 1 | The file is a link to another file, or a folder. |
| `not_utf8` | 1 | The file is not UTF-8. |
| `unsupported_layout` | 1 | Bars are written in a way the commands cannot edit (such as `[bar]` instead of `[[bar]]`). |
| `colors_block_broken` | 1 | The colours block has no end line, or two start lines. |
| `colors_block_edited` | 1 | The colours block holds a rule RSB did not write. |
| `write_failed` | 1 | The file could not be written (disk full, or a drive that does not allow safe saves), `.undo\lock` is not a file, or `.undo` holds too many entries. |
| `nothing_to_undo` | 1 | There is no settings change to undo. |
| `undo_not_latest` | 1 | The change asked for is not the newest. |
| `undo_conflict` | 1 | A file was edited after the change; undoing would lose that edit. `next` names where the file before the change is kept. |
| `undo_broken` | 1 | The saved copy for the change is damaged. |
| `nothing_to_restore` | 1 | No copy of the file that loaded clean is kept yet. |
| `render_failed` | 1 | A preview could not be drawn. |
| `internal` | 1 | The change did not check out, or something failed inside; nothing was written. Report it with `rsb.log`. |

### Exit codes

The settings commands exit with 0 (done, also a saved change the bar did not
confirm), 1 (the request is right but could not be done) or 2 (the request is
wrong). They never exit with 3, 4 or 5: a bar that is not running is not an
error here, it is `bar.running: false`.

## Versions

`contract` is in every reply and may be in every request. A request that leaves
it out, or names a version this build still serves, is answered; one above the
build's version is refused with `contract_too_new`, one below the oldest served
with `contract_too_old`. Adding a field, key, change, error code or notice does
not change the version: a caller ignores what it does not know. Removing or
renaming a field, or changing what one means, raises it, and the old version
keeps being served until it is dropped on purpose.

## Hand edits, undo and the log

- A file's version is the FNV-1a 64 of its raw bytes, written
  `fnv1a64:<16 hex digits>`. It detects edits; it is not a defence against a
  person who means to tamper.
- With `--expect`, a file the change writes must have exactly that version, else
  `file_changed`. Without it, a file that differs from what settings last wrote
  is changed as it is and the notice `hand_edit_kept` says so.
- A save from an editor that lands between the read and the write is refused
  with `file_changed`, never overwritten. The check and the replace are two
  calls, so a save by rename that completes in the one to five milliseconds between
  them on a busy PC can still be replaced without a trace; this needs an editor's save
  within a few milliseconds of a settings write.
- Every write keeps the file as it was in `.undo\<n>\`, at most 20 entries.
  `undo` puts the files of the newest entry back, provided they still are what
  that change wrote. `restore` puts back the last copy that loaded clean, and
  keeps the broken text as the new entry's before copy. Every write also keeps the text
  it wrote, once it checked it loads clean, as the copy `restore` puts back, so `restore`
  works with no bar running.
- A write that was killed between its two files leaves one entry unfinished. The next write
  puts the files that were already replaced back from the saved copies (so the change is all or
  none) and is itself refused with `file_changed`, as the file it was planned against is gone: read
  again and repeat it. A person's own save in the middle leaves the files as they are, keeps the copies
  and says so in a `hand_edit_kept` notice; an undo cut off the same way is finished.
- Each write, undo, restore and write refusal adds one line to `rsb.log` with
  only validated values in it, for example
  `config: bar.toml: bar main: right ["status", "battery"] -> ["status", "battery", "clock"] (undo 17)`.
  A request that was wrong (`bad_usage`, `bad_value` and the other exit code 2
  refusals) and a dry run leave no line.
- Writes go only to the default config folder, a folder that does not exist
  yet, or a folder that already holds `bar.toml` or `style.css`: a misspelt
  `--config-dir` does not turn another folder into a config folder.
- Nothing is written from an elevated process.

## Over the pipe

The running bar answers the same requests on its command pipe, one line per
request, for tools that already talk to it: `settings <the request as one line
of JSON>`. Requests are printable ASCII (write other characters as `\u`
escapes) and at most 4096 bytes. The reply is `ok <reply JSON>` for every request
that was served, `ok: false` included, and `err <text>` only for a line that does
not decode.

- `applied` returns the versions of the files the bar shows:
  `{"bar.toml":"fnv1a64:...","style.css":"fnv1a64:...","diagnostics":0,"loads":3,"config_dir":"...","log_dir":"..."}`
  (`config_dir` and `log_dir` canonical; `loads` counts the configs the bar has swapped in since it
  started, and moves even when the new files show the same versions). A caller that wrote a file asks
  `applied` until the versions match.
- Reads (`get`, `presets`, `schema`, `log`) are served inline. A reply of more than 65,532 bytes
  (a `log` of many long entries) is refused with `cli_only`; use `--last` or the command line.
- `widgets` and `preview` are refused with `cli_only`: they read widget modules and draw pictures, which is not work for the bar's process.
- A write never waits: if another change holds the lock it is `busy` at once.
  The reply's `bar` is the bar's own record as of the write (`shows_files` is usually
  still false) and `ms` is null; ask `applied` afterwards. It checks no popup menus.
- A write is served only to a client whose process runs at the bar's level of
  rights or above; otherwise `cli_only`.
- The same JSON comes back on the command line and on the pipe, except the `bar` block of a write.

## Examples

```text
rsb settings get
rsb settings set preset glass --expect bar.toml=fnv1a64:3f0c9a1e5b7d2c44
rsb settings set color.accent #e5a50a
rsb settings move clock main.right:0 --from main
rsb settings remove media --from main --dry-run
rsb settings undo
```

A settings page calls the command line once per action: `get` when it opens
(keeping each `files.<file>.hash`), then a change with those hashes as
`--expect`. On `file_changed` it reads again and says the file was edited
outside the page. A gallery click sends `set preset ... --dry-run` first, and
when `preset_switch.kept` lists zone lists it shows the `--mine` preview beside
the shipped one.

## Calling from a settings page

`rsb.exe` is a console program. A page that starts it:

- finds it beside its own executable, never through `PATH`, and leaves out `--config-dir` (both
  resolve `RSB_CONFIG_DIR`, else `%APPDATA%\rsb`);
- starts it with the `CREATE_NO_WINDOW` creation flag, so no console window flashes;
- runs it on a worker thread, never inside the frame that draws the page, and gives it a timeout
  (5 seconds is plenty: `get` takes about 0.2 s, a write about 0.1 s plus up to 2 s waiting for the bar);
- sends the hashes it read as `--expect` with every change, saves zones as whole lists, and acts on
  an error's `fix`, not on its `next`.

A process start per call is the cost of the design; the budget is 300 ms for `get`.

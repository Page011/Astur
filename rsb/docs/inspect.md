# rsb inspect

`rsb inspect` shows what each widget on the running bar draws and what it costs: the element tree and its
computed style, the services it reads, its script and drawing time, and what its reactive scope holds. It talks
to the bar over the command pipe, so a bar must be running.

```text
rsb inspect [widget] [--bar NAME] [--style] [--depth N] [--redact] [--json]
```

| Argument | Meaning |
|---|---|
| none | every widget, one summary block each, no tree |
| `widget` | a ref as written in `bar.toml` (`clock`, `clock#utc`, `my/weather`); every instance of it (one per bar and monitor) gets its tree |
| `--bar NAME` | only instances on that bar |
| `--style` | the computed style under each node. With `--bar` and no widget: the bar's own root node and its style, and no widget trees |
| `--depth N` | levels of the tree shown, 1 to 64 (default 64); the widget's root element is level 1 |
| `--redact` | every text, input value, image path and tooltip text becomes `<N chars>`; so does every text value of a widget option, at any depth; a stopped widget's reason keeps only the place of the failing line (`widgets/clock.luau:6`) and loses the error text; the path of a user widget outside the config folder becomes `<path>` |
| `--json` | one JSON document on stdout instead of the text |

| Exit code | Meaning |
|---|---|
| 0 | shown (a cut tree is still 0, and says so) |
| 1 | no widget (or bar) of that name; the message suggests the closest name |
| 2 | bad usage |
| 3 | no bar is running (`rsb inspect: no bar running` on stderr) |
| 4 | the bar did not answer |

## What the summary shows

```text
rsb 3.0.0-alpha.1, as of 11:10:04 (session console)
process CPU 0.42 s in the last 60 s, 31.00 s since start (uptime 1 h 2 min)
costliest in the last 60 s: service system 180 ms; clock 21 ms; media 9 ms
bar main  monitor 0  scale 1.5  52 nodes  1,214 frames: style 0.12 s, layout 0.11 s, paint 0.10 s; slowest 1.1 ms; last 0.4 s ago; 40 ms in the last 60 s
  colours: accent #e5a50a, fg #e6e6e6, bg #202020 (dark)

clock  on main, running, built-in, 0 errors, 0 restarts
  last drawn 0.4 s ago (1,204 trees since load); script last ran 0.4 s ago
  script 38.2 ms in 3,611 calls, slowest 0.9 ms, 21.0 ms in the last 60 s, 0.3 ms in the last second
  options: format "HH:mm" (bar.toml:31:1), timezone "local" (default)
  layout + paint 14.0 ms over 1,190 frames of its own, slowest 0.4 ms; 12 frames shared with other widgets
  memory: script VM shared by all built-ins (2,410 KB in all); 6 nodes on the bar
  services: clock ok (last value 0.4 s ago; polls 1.2 ms in the last 60 s)
  reads: clock.time; 1 signal of its own, 1 computed, 4 effects, 1 timer, 0 event listeners, 0 running tasks
```

The numbers are illustrations. Widgets and the `costliest` line are ordered by milliseconds in the last 60 s,
largest first, then by name; services by their poll time in the last 60 s.

### Where each number comes from

| Number | Source |
|---|---|
| calls, script time, slowest call, errors, restarts, stop reason, script VM size | the same snapshot `rsb status` reads |
| script time in the last second, in the last 60 s | the script thread's own accounting: the last second from the calls of that second, the last 60 s from one bucket per second, one add per script turn |
| last drawn, trees since load, script last ran | the clock reads the script thread already makes: one at the start of each turn and one when it hands over trees |
| layout + paint | the bar lays out and draws all its widgets in one pass, so no widget has a timer of its own. A built frame that followed one widget's new tree and no other counts as that widget's **own**; a frame that followed several is split evenly between them and counts as **shared**; a frame no tree arrived for (hover, a transition) counts for the bar only |
| services, last value age | the services hub; poll time is the time the hub thread spent on that service's reports and polls |
| options, with where each value was set | the values in effect: the widget's declared default, or the value from your `bar.toml` (with file, line and column) |
| process CPU in the last 60 s | a sample of the process's CPU time every 10 s, six kept; the window the figure covers is the age of the oldest sample (50 to 60 s) and is printed |
| reads | a walk of the widget's reactive scope tree, on request only (see below) |

### The reads line

The walk looks at the effects, computeds and signals under a widget and counts them. A signal nothing reads is
in no scope's sources, so it is not counted; a service field (`clock.time`) is named, not counted. It stops after
10,000 scopes and says so with `walk_cut` (the text says "at least"). The walk reads tables only and runs no
widget code; it takes about 2 ms at 10,000 scopes (1.9 ms at the 95th percentile, measured in a slow remote session).

## A widget tree

```text
rsb inspect clock --style
```

prints the summary block, then `tree (as of the frame 0.4 s ago; logical px)` and one line per node:

```text
box.clock x 1644 y 0 w 70 h 32 on click
  text "11:10" x 1644 y 7 w 52 h 18 hover
    color: #e6e6e6
```

A line is the element kind, `#id`, `.classes`, its text in quotes, its box in logical px (rounded to 0.1),
`hidden` when it takes no space, its states (`hover`, `active`, `focus`, `checked`, `disabled`), and `on` and the
handlers it has. With `--style` the properties that differ follow, written as CSS: a property that is not
inherited when it differs from the initial style, an inherited one (`color`, `font-*`, `letter-spacing`,
`text-align`, `cursor`, `caret-color`, `accent-color`) only where it differs from the parent. Values longer than
120 characters are cut with `...`; text longer than 80 characters is cut with `...` and `text_len` holds the full
length.

A popup, menu or tooltip the widget declares is listed under the element it hangs from. A closed one is
`popup calendar closed, 41 nodes (open it to see them)`; an open one shows the nodes of its own surface.

The style is the style as last drawn, transitions applied. When a newer tree or style is waiting for the next
frame the output says so (`pending` in the JSON).

## Limits, and how a cut is said

Every reply is one line under 65,536 bytes, so a big tree is cut. A cut is never silent.

| Limit | Value | What happens |
|---|---|---|
| reply | 65,536 bytes | the tree is written breadth first into a 60,000-byte budget; a node that does not fit is dropped and its parent says how many children it lost (`children_cut`) |
| nodes copied by the bar's UI thread | 600 per widget | the rest are counted, not copied |
| tree depth | 64 | the bar refuses deeper trees |
| summary page | 60,000 bytes of widgets | the bar answers one page at a time; the command joins the pages. A page holds about 90 widgets |
| text per node | 80 characters | `...`, and `text_len` |
| style value | 120 characters | `...` |

The last line of a cut tree says it, and the JSON carries it: `"cut": {"shown": 320, "not_shown": 1681,
"total": 2001, "reason": "reply limit"}`. `shown + not_shown == total`. The reason is `reply limit`, `node limit`
(the UI thread's copy limit) or `depth` (`--depth`). Use `--depth` or leave out `--style` to see more of the top.

If a part of the bar does not answer within one second the output says so (`did not answer: the script thread`)
and the fields that part owns are `null`.

## Privacy

- The reply goes only to the caller over the user-only pipe and to stdout. It can hold live text from widgets.
- A password input's value is never sent, with or without `--redact`: the node shows `<N chars>`.
- A widget option whose name contains `password`, `passwd`, `pwd`, `secret`, `token`, `apikey`, `api_key`,
  `api-key`, `credential`, `cookie` or `auth` (any case) is never sent, and neither is any value held by such a
  key inside a table or list option: it is `<N chars>`. Without `--redact` the other option values are sent as
  they are; with it every text value is `<N chars>` too, and a stopped widget's error text (which can quote a
  value) is cut to the place of the failing line. Share `--redact` output, not the plain one.
- Answering a request writes nothing to `rsb.log`. A request that cannot be made to fit is refused with one line
  that names only the reason (`inspect refused: reply limit`).

## The JSON

`--json` prints one object. The first page of the summary holds `contract`, `version`, `session`, `wall_ms`,
`uptime_ms`, `process` (`cpu_ms`, `cpu_ms_last_60s`, `cpu_window_ms`), `costliest`, `bars`, `services`, `widgets`,
`parts` (`script`, `ui`, `hub`: `ok` or `timeout`) and `next`; the command joins every page, so `next` is `null`.

A `bars` entry: `name`, `monitor`, `scale`, `nodes`, `frames`, `style_us`, `layout_us`, `paint_us`,
`frame_us_max`, `no_tree_frames`, `last_frame_ms_ago`, `ms_last_60s`, `scheme` (`dark` or `light`) and
`root_vars`, the value of the preset's colour variables on the bar's root. With `--bar --style` it also holds
`root`.

A `widgets` entry: `instance`, `widget`, `bar`, `state`, `source`, `errors`, `restarts`, `reason` (only when the
widget is stopped), `last_tree_ms_ago`, `trees`, `last_run_ms_ago`, `script` (`ms`, `ms_max`, `calls`, `ms_last_s`,
`ms_last_60s`), `frames` (`own`, `own_us`, `own_us_max`, `shared`, `shared_us`), `memory` (`vm_kb`, `shared_vm`,
`nodes`), `options` (`value`, `source`, `at` by name; `source` is `default`, `user` or `preset`), `services`
(`name`, `status`, `age_ms`) and `reads`. A request for one widget adds `tree`, `frame_ms_ago`, `pending` and, when
cut, `cut`. A node: `kind`, `id`, `classes`, `text`, `text_len`, `rect` (null when hidden), `hidden`, `states`,
`handlers`, `style` (only with `--style`), `children` and `children_cut`; an image has `src`.

## What it costs the bar

Always on, per widget per script turn: one bucket add. Per service poll: two adds. Per built frame of a bar: one
lookup in a small list and a few adds (measured at 0.006 us a frame, release); a frame that finds nothing to do
adds nothing. One process-time sample every 10 s. The node copy, the style text, the reactive walk and the JSON
run only while a request is answered.

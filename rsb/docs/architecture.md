# Architecture

RSB draws one bar window per monitor (or several: a top and a bottom bar)
from three kinds of user input: `bar.toml` (which bars, which widgets where),
`style.css` (how they look) and Luau widget files (what they show and do).
Built-in widgets are Luau files inside the binary, written against the same
public API as user widgets.

## Crates

| Crate | Job | Depends on |
|---|---|---|
| `rsb-core` | `bar.toml` schema and defaults, the CSS subset (tokenizer, parser, selectors, cascade), the element tree, the scene (layout with Taffy, zones, draw list, damage, transitions, hit test), diagnostics and the error plate, the command-pipe wire format, and every type that crosses a thread or crate boundary. No Windows, no Luau: plain Rust, unit-tested. | taffy, toml, serde_json |
| `rsb-render` | Draw list to premultiplied BGRA pixels: tiny-skia for shapes, Parley for text layout and font fallback, swash for glyph images (colour emoji included). Implements core's text measurement so measured and drawn text agree. Offscreen PNG output. | rsb-core, tiny-skia, parley, swash, png |
| `rsb-win` | The UI thread: message loop, layered bar windows, appbar registration, DPI, monitors, fullscreen detection, theme changes, pointer input, the native right-click menu. | rsb-core, windows |
| `rsb-services` | Data sources (system, battery, audio, network, media, foreground window, workspaces), script I/O (exec, HTTP via WinHTTP, file read and watch), folder watching, the real clock, and fakes of everything that changes the system. | rsb-core, windows, tungstenite |
| `rsb-script` | Luau (mlua): sandboxed VMs, the `rsb` module, signals, the task scheduler, timers, permissions, and the generator of the luau-lsp type file. | rsb-core, mlua |
| `rsb` | `rsb.exe`: wiring, command line, named pipe, single instance, logs, first run, embedded built-in widgets and presets. The wiring is a library (`rsb_app`) so tests can reach it; the binary is a few lines. | all of the above |

`rsb-render`, `rsb-win`, `rsb-services` and `rsb-script` never depend on each
other. Everything they exchange is defined in `rsb-core`, so each can be
built and tested alone.

## Threads

```text
 UI thread (rsb-win + core scene + rsb-render)          script thread (rsb-script)
 --------------------------------------------          ---------------------------
 Win32 message loop, bar windows, appbars               Luau VMs, signals, tasks, timers
 scene per bar: style, layout, draw list, damage        ScriptIn  <-- UI: input, theme, config
 renderer: draw only the damaged pixels                 ScriptOut --> UI: widget trees, plates
 present: UpdateLayeredWindowIndirect + dirty rect      HubRequest --> services
      ^                                                       ^
      | UiWaker (posted message) + channel                    | HubEvent (values, I/O results)
      |                                                       |
 file watch + config reader threads: config folder,   services hub thread + one thread
 debounced; read and parse changed files and           per running source, I/O workers
 `rsb reload`, never the UI                            (COM / WinRT on their own MTA threads)
 pipe thread: \\.\pipe\RSB-<user SID>-<session>; gathers `rsb status` itself
 watchdog thread: notices a script call stuck for 2 s
```

Rules every thread follows:

- The UI thread never runs Luau, never reads a file and never waits on another
  thread. Two things it does can wait on another process, with no timeout of
  their own: the appbar calls (`SHAppBarMessage` goes to the shell) and, while
  the native menu is open, the menu itself, which holds the loop until a pick
  or a dismissal. It also writes log lines under the logger's lock. Opening the
  log or config folder from the menu runs on a short-lived worker. `rsb status` is assembled by the pipe thread, which
  asks the script and services threads with 1 s timeouts; the UI thread only
  answers for its own bars.
- Scripts never block: I/O calls yield the calling coroutine; the scheduler
  resumes it when the result arrives. Every call into a VM runs under a CPU
  time budget enforced by an interrupt, each widget has a CPU allowance per
  second, and turns are shared round-robin; a widget over a limit is stopped
  and shows a plate while the others keep running. A call that the interrupt
  cannot reach (inside a C library function) is noticed by the watchdog,
  which puts a plate on the bar after 2 s.
- Services never block each other: each source runs on its own thread and
  only posts results. A source that stops sending is marked stale by the hub
  and widgets show a placeholder, never the old value as if it were fresh. The
  one thing the hub thread waits for is a source thread that is being stopped
  (at most 1 s each, only when that thread is stuck in a system call; a normal
  stop takes milliseconds). It is not moved off the hub thread because a source
  restarted while its predecessor is still shutting down would overlap it. A
  source's own waits on the system (the media session list, a track's
  properties) end after 2 to 5 s.
- Channels are unbounded `std::sync::mpsc`; receivers coalesce (the script
  thread sends at most one batch of trees per turn, the UI draws at most one
  frame per wake).

## Starting

`rsb` with no command runs the bar:

1. Console: a console made just for this process is released at once (a
   brief flash when started from Explorer); from cmd.exe it stays.
2. The single-instance mutex (`Local\RSB.Bar`). A second `rsb` says "rsb is
   already running" and exits 0.
3. The log folder and the panic hook.
4. The config folder: created and filled on the first run; the type file
   `types\rsb.d.luau` is rewritten at every start when it differs from this
   build's.
5. `bar.toml` and the style sheets are read and parsed (before the message
   loop exists, so this is not UI-thread I/O). A broken `bar.toml` on the
   very first start still gets the default bar, with a plate that says why.
6. The window layer, the renderer, the services hub, the script thread, the
   config watcher and reader, the command pipe and the watchdog start; the
   bars are created and the script thread is given the widget instances.
7. A bar is first drawn when all of its widgets have sent their first tree or
   plate, or 150 ms after the bar was created, whichever is first; the
   widgets that were late appear in the next frame.

Widget instances keep their id across config reloads (the same bar, monitor,
widget name and n-th occurrence), so moving a widget between zones or
editing an unrelated line never restarts it. A style edit never recreates a
window: only a changed position, height, margin, reserve or fullscreen mode
does.

## Stopping

Every way out (`rsb quit`, the menu, Ctrl+C or closing the console, logoff,
`WM_CLOSE`) starts the same order. Logoff and shutdown guarantee only the first
step: Windows may end the process after the `WM_ENDSESSION` message returns.
While the native menu is open, `rsb quit` waits for it (the pipe answers after
2 s and the bar stops when the menu closes). A console close that waits longer
than 3 s for the stop order gives the screen space back from the console
thread (every appbar is removed) before Windows ends the process, which it does
at the hung-app timeout (5 s by default). The wait is shorter when that timeout
has been lowered: the timeout less 1.5 s, but at least 0.5 s.

1. The UI thread destroys every bar and removes every appbar; on
   `WM_ENDSESSION` this happens inside the message, before Windows moves on.
2. The pipe thread stops.
3. The script thread is told to shut down and joined (1 s).
4. The services hub stops its sources and workers (1 s; a hung source
   thread is left to exit with the process).
5. The watcher, config reader and watchdog stop.
6. The log is flushed and the process exits with its code.

## Panics

Panics unwind (`panic = "unwind"` in every profile). Luau is built with C++
exceptions, and every script error raised from a Rust callback, a permission
denial or a budget stop included, travels through Rust frames; with
`panic = "abort"` each of those would end the whole process. A real Rust
panic is still fatal: the panic hook writes it to the log and aborts, so no
thread ever dies quietly and leaves a frozen bar. Before it aborts, the hook
removes every appbar the process registered, so the shell does not keep the
reserved space of a bar that no longer exists.

## From an event to pixels

A volume change:

1. The audio callback (COM, audio thread) reads the new level and publishes
   the service value.
2. The hub thread stamps a version and sends `HubEvent::Service` to the
   script thread.
3. The script thread updates `rsb.services.audio`; only the bindings that read
   `volume` re-run; the volume widget's tree is rebuilt and sent as
   `ScriptOut::Trees` with a wake of the UI thread.
4. The UI thread hands the tree to the scene. The scene restyles the changed
   nodes, re-lays out only if a size can have changed, rebuilds the draw list
   and compares it with the previous one: only the changed nodes' pixels are
   damaged.
5. The renderer clears and redraws only the damaged rectangle into the bar's
   DIB section, and `UpdateLayeredWindowIndirect` presents just that
   rectangle.

Nothing changed means no frame: a clock that shows minutes wakes the process
once a minute, on the minute.

A file save: the watch thread sees the change, waits 25 ms for more, and the
config reader reads and parses the changed file off the UI thread and sends
the result. A broken file
keeps the last good version in effect and puts a plate on the bar
(`RSB ERROR - style.css:12:5 unknown property 'colr'`); fixing it removes the
plate. A changed widget file reloads only the widgets that use it. A file that cannot be read (over 1 MiB, locked for good) leaves the old copy running and is logged.

A mouse move: the bar window's message goes to the scene's hit test; a new
hover chain restyles `:hover` rules at once (no script involved) and starts
any CSS transitions, which ask for frames until they finish. Enter, leave,
click and wheel handlers are sent to the script thread. A right click nothing
handles opens the native menu; a click on an error plate opens the log. The
bar's whole area is painted at least 1/255 opaque, because a layered window
lets clicks through fully transparent pixels.

## When something breaks

Nothing fails silently: every row is logged and visible.

| What | What you see | Where it is logged |
|---|---|---|
| A style, config or widget syntax problem | A plate with file:line (`RSB ERROR - style.css:12:5 ...`); the last good file stays in effect; fixing the file removes the plate | `warning:` or `error:` with the same position |
| A widget over a budget, out of memory, or denied a permission | A plate on that widget only; the others keep running; built-in widgets restart by themselves | the stop, with its numbers |
| A script call stuck inside a library function | A bar plate after 2 to 3 s; it clears when the call returns | `script call in <widget> running for N s` |
| A service that went quiet | `--` in the widget and the class `stale`; no plate | `service <name> stale after N s` |
| Drawing the bar failed | The last good frame stays and a fixed plate is drawn in the left corner; `rsb status` shows `DRAWING FAILED` (`--json`: `"render":"failed"`) | the error |
| The command pipe could not start | A bar plate (`PIPE ERROR`) | the error |

Clicking a plate opens the log.

## Rendering

The bar is drawn on the CPU (tiny-skia, Parley, swash) into a top-down 32-bit
DIB section and presented with `UpdateLayeredWindowIndirect`. Measured against
Direct2D and wgpu for the same bar, it used the least memory (about 3 MB
private versus 42 MB and 115 MB) and started fastest, and drew a clock tick
in under half a millisecond. Text is grayscale anti-aliased: per-pixel alpha
surfaces cannot use ClearType on any of the three paths.

## Sandbox and permissions

Each user widget file runs in its own Luau VM (about 1 MB); built-ins share
one, and every instance gets its own globals. Before sandboxing,
`loadstring`, `getfenv`, `setfenv` and the host `print` and `require` are
removed; RSB's own `require` resolves `@rsb` and modules inside the widget's
own folder only. Each VM has a memory cap, each call a CPU budget, each
widget a CPU allowance per second, and reactive updates a loop cap. The
decision to stop a widget is kept in Rust, so `pcall` cannot swallow it.
Built-in widgets that were stopped restart by themselves with backoff.

A widget declares what it needs: programs it runs, files it reads, HTTPS
hosts it calls, services it controls, and private service values it reads
(window titles, media, network names; user widgets get these by default).
Undeclared calls fail with a plate naming the missing permission. Every
request carries the widget's grant to the services thread, which checks
what only it can see: each HTTP redirect hop (WinHTTP never follows
redirects itself) and the final path of an opened file (junctions and symbolic
links cannot lead outside the allowed folders).

What one widget may do to another is limited too: timers, tasks, event
subscriptions and requests belong to the widget that made them, and a cancel
from another widget does nothing; a built-in widget loads its modules only from
the built-in set, never from the user's `widgets\lib\`; the metatables the
runtime shares between widgets of a VM are frozen. A widget directly in
`widgets\` and the built-ins have no folder of their own for relative paths.

## Window behaviour

- Broadcasts (`TaskbarCreated`, theme, display, power and time changes) go
  to a hidden top-level window that is never shown; a message-only window
  would not receive them.
- Sleep and clock changes: Windows waits do not count time asleep, so resume
  and time changes re-arm every wall-clock timer at once.
- Appbar: registered with `SHAppBarMessage` when the bar reserves space; the
  shell sends position notices for our own moves too, so an unchanged
  position is ignored (otherwise it loops). Registered again after Explorer
  restarts (`TaskbarCreated`), including a bar whose first registration was
  refused because the shell was not up yet at logon; removed on exit, which
  restores the work area exactly. The `workarea` stats line named `register` is
  read right after the call, and the shell applies the reservation of a second
  bar a moment later, so with two bars it can show the space before that
  bar's (the same short gap as after `TaskbarCreated`); read a later line.
- Per-Monitor-V2 DPI: `WM_DPICHANGED` resizes and relays out; monitor changes
  rebuild the bars from the `[[bar]]` monitor selectors.
- Fullscreen apps: `ABN_FULLSCREENAPP` drops the bar below them or hides it.
- The bar never takes focus and has no taskbar button.

## Command line

`rsb.exe` is a console program, so `rsb status` prints in place and
`%ERRORLEVEL%` is rsb's own exit code. Started from Explorer or a shortcut,
the bar detaches from the console it was given at once; Astur starts it with
no console at all. One instance runs per user session; other `rsb` commands
reach it over a named pipe only the same user can open (the client also
checks that the server runs as the same user in the same session before it
sends anything).

| Command | Does |
|---|---|
| `rsb reload` | re-reads `bar.toml`, every style sheet and every widget file |
| `rsb quit` | runs the stop order and gives the screen space back |
| `rsb toggle [bar]` | hides or shows every bar, or one by name |
| `rsb status` | a table for people: bars, widgets (state, calls, errors, restarts, memory), services, memory. `rsb status --json` prints the pipe's one line of JSON (script time and VM memory included) |
| `rsb trigger <event> [json]` | delivers an event to `rsb.events.on` |
| `rsb eject <widget>` | copies a built-in widget (and the helpers it needs) into `widgets\` without overwriting anything |
| `rsb types` | writes `types\rsb.d.luau`; `.luaurc` and the VS Code settings only when missing |
| `rsb render --out <png>` | draws the bar to a PNG without a window, with sample values or (`--live`) this PC's |

Exit codes: 0 done (or already running), 1 the command failed, 2 bad usage,
3 no bar is running, 4 the bar could not be reached (another user or session
owns the pipe, or no answer within 2 s), 5 the bar could not start.

## Files

- Config: `%APPDATA%\rsb` (or `--config-dir`, or `RSB_CONFIG_DIR`):
  `bar.toml`, `style.css`, `widgets\`, `types\rsb.d.luau`, `.luaurc`,
  `.vscode\settings.json`.
- Logs: `%LOCALAPPDATA%\rsb\logs`, rotated. Logs hold counts, sizes and
  timings, never window titles, media names, network names or file paths
  outside the config folder.

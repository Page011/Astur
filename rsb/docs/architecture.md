# Architecture

RSB draws one bar window per monitor (or several: a top and a bottom bar)
from three kinds of user input: `bar.toml` (which bars, which widgets where),
`style.css` (how they look) and Luau widget files (what they show and do).
Built-in widgets are Luau files inside the binary, written against the same
public API as user widgets. Widgets can open popups, menus, tooltips and
full-screen overlays; each is a window of its own, drawn the same way as a
bar.

## Crates

| Crate | Job | Depends on |
|---|---|---|
| `rsb-core` | `bar.toml` schema and defaults, the CSS subset (tokenizer, parser, selectors, cascade, keyframes), the element tree, the scene (layout with Taffy, zones, draw list, damage, transitions and animations, scrolling, text editing, keyboard focus, hit test), popup placement and the rules that close popups, diagnostics and the error plate, the command-pipe wire format, and every type that crosses a thread or crate boundary. No Windows, no Luau: plain Rust, unit-tested. | taffy, toml, serde_json |
| `rsb-render` | Draw list to premultiplied BGRA pixels: tiny-skia for shapes and outlines, Parley for text layout and font fallback, swash for glyph images (colour emoji included), resvg for SVG icons and images. Fonts can be added from files (the bundled monospace font, `@font-face`). Implements core's text measurement so measured and drawn text agree. Offscreen PNG output. | rsb-core, tiny-skia, parley, swash, png, resvg |
| `rsb-win` | The UI thread: message loop, layered bar and popup windows, appbar registration, DPI, monitors, fullscreen detection, theme changes, pointer and keyboard input, IME, the clipboard worker, the native right-click menu. | rsb-core, windows |
| `rsb-services` | Data sources (system, battery, audio and audio devices, microphone, network, Wi-Fi, media, foreground window, workspaces, Bluetooth, Start-menu apps, power), script I/O (exec, HTTP via WinHTTP, file read and watch), folder watching, the real clock, and fakes of everything that changes the system. | rsb-core, windows, tungstenite |
| `rsb-script` | Luau (mlua): sandboxed VMs, the `rsb` module, signals, the task scheduler, timers, permissions, and the generator of the luau-lsp type file. | rsb-core, mlua |
| `rsb` | `rsb.exe`: wiring, command line, named pipe, single instance, logs, first run, embedded built-in widgets and presets. The wiring is a library (`rsb_app`) so tests can reach it; the binary is a few lines. | all of the above |

`rsb-render`, `rsb-win`, `rsb-services` and `rsb-script` never depend on each
other. Everything they exchange is defined in `rsb-core`, so each can be
built and tested alone.

## Threads

```text
 UI thread (rsb-win + core scene + rsb-render)          script thread (rsb-script)
 --------------------------------------------          ---------------------------
 Win32 message loop, bar and popup windows, appbars     Luau VMs, signals, tasks, timers
 scene per surface (bar, popup, menu, tooltip,          ScriptIn  <-- UI: input, keys, popup
   overlay): style, layout, draw list, damage                         closes, theme, config
 renderer: draw only the damaged pixels                 ScriptOut --> UI: widget trees, plates
 present: UpdateLayeredWindowIndirect + dirty rect      HubRequest --> services
      ^                                                       ^
      | UiWaker (posted message) + channel                    | HubEvent (values, I/O results)
      |                                                       |
 file watch + config reader threads: config folder,   services hub thread + one thread
 debounced; read and parse changed files and           per running source, I/O workers
 `rsb reload`, never the UI                            (COM / WinRT on their own MTA threads)
 pipe thread: \\.\pipe\RSB-<user SID>-<session>; gathers `rsb status`, `rsb inspect`
   and `applied` itself and runs `settings` requests (see "Settings and inspect")
 watchdog thread: notices a script call stuck for 2 s, and a UI thread stuck
   for 2 s while a full-screen overlay is shown (it hides the overlay)
 clipboard worker: reads and writes clipboard text for inputs (500 ms limit)
 outside-press watch: a read-only low-level mouse hook on its own thread,
   only while a popup without keyboard focus is open
```

Rules every thread follows:

- The UI thread never runs Luau, never reads a file and never waits on another
  thread for anything it has to compute. Three things it does can wait: the
  appbar calls (`SHAppBarMessage` goes to another process, the shell, with no
  timeout of its own), the native menu while it is open (it holds the loop until
  a pick or a dismissal), and the outside-press hook's own thread, which the UI
  thread waits for while it starts (at most 2 s; a thread that is slower is kept
  and removed with the watch, never left running unseen) and stops (at most 1 s).
  A turn of the loop that lasts a second or more is logged (`the bar did not
  draw for N ms`, naming the slowest popup or bar draw in it) and written to the
  stats as `stalled {ui_ms}`. It also
  writes log lines under the logger's lock. Opening the
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
- Clipboard reads and writes run on a worker: opening the clipboard can wait
  on the program that owns it. A request stuck in another program is never
  answered by starting a second worker; later requests get no text until it
  returns. Typing, caret movement, selection and IME
  composition are handled on the UI thread by the scene, so an input never
  waits for a script.
- Pictures that services make (album art, app icons) are decoded on the
  service's thread and go straight to the UI thread's renderer, held in
  memory only. Only Windows' built-in PNG, JPEG, BMP and GIF decoders are
  used, and a picture that claims more than 8192 px a side or 32 million
  pixels is refused before it is decoded.
- The low-level mouse hook that notices clicks outside a popup runs on its
  own thread, never the UI thread: every mouse event on the desktop passes
  through a low-level hook, so a busy UI frame must not delay the pointer. The
  procedure allocates and frees nothing: a region snapshot the UI replaced is
  dropped by the UI thread on its next publish.
- User SVG files are parsed on the image loader thread; the UI thread only
  draws them.

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
wheel scrolls the nearest container under the pointer that can move; Shift
with the wheel, or a tilt wheel, scrolls sideways. The
bar's whole area is painted at least 1/255 opaque, because a layered window
lets clicks through fully transparent pixels.

## Popups, menus, tooltips and overlays

A popup is an element in a widget's tree, a child of the element it opens
from (its anchor). It is never laid out in the bar: the bar's scene reports
it, and while it is open the UI thread gives it a window and a scene of its
own. The script decides when a popup should be open (its `open` state); the
UI thread decides when it closes on its own and tells the script, which
writes the state back, so closing never waits on a script:

| What closes a popup | How RSB notices |
|---|---|
| A click outside it | a popup that holds keyboard focus loses it; one that does not is watched with a read-only low-level mouse hook on its own thread, installed only while such a popup is open. "Outside" is outside the popup's drawn shape, so a click on its shadow or on a part not grown yet counts |
| Escape | the popup's own keyboard input (unless the popup asks to keep Escape) |
| Its anchor going away, its bar hidden (also by a fullscreen app), a display or DPI change | the bar's scene and window events |
| Another popup of the same group opening | the popup manager (never the panel a menu was opened from) |
| A menu item picked | the menu's scene |

A press that closes a popup and whose click then asks to open it again (a
button elsewhere that toggles it) leaves it closed, so a second click on a
launcher button closes the launcher.

Placement: below or above the anchor (towards the larger part of the
monitor), flipped when it does not fit, moved to stay inside the work area,
on whole pixels so its edge meets the anchor's. A popup is placed against
where its anchor will be when the anchor's running transition ends, so an
anchor that widens and the panel that grows out of it line up; a panel can
take the exact pixel columns of its anchor's flat edge, so the two meet with
no seam at any scale. An open popup follows its anchor when the anchor
moves. The window is made at the popup's open size and is not resized by its
animation; it is resized when what it shows changes size (a list that gains
rows), and its transparent pixels let clicks through. A closing popup keeps
drawing the content it had while its closing animation runs.

Content inside a popup is built only while it is open: closing a launcher
also ends the app list it read, so a bar with every popup closed costs what a
bar without popups costs.

Focus: bars never take focus. A popup takes keyboard focus only when it holds
something to type into or choose with the keyboard and the user opened it:
its open state changed while a click or key handler ran (or for
`rsb popup <name>`, run by a program the user just used, which passes its
right on), less than a second earlier. A popup a widget opens from a timer or
on its own never takes focus, so no widget can catch keys meant for another
program. When it closes, focus goes back to the window that had it. Tooltips
never take focus and let the pointer through.

The power menu is an overlay: it opens only from a click or a key, and a user
widget needs the `overlay` permission for one. It is up to three windows: a
dim window over the whole monitor, painted with one colour and faded with its
window alpha (so a 4K monitor costs no repaint per frame); where Windows
offers it (Windows 11, not over Remote Desktop, transparency effects on) a
separate system-backdrop window under the dim blurs what is behind; and the
buttons in a window only as large as they are. RSB never captures the screen.
The overlay ignores clicks and keys until its buttons have landed (at least
400 ms), never acts on a held key's repeat, and puts focus on Lock, not on
shut down. Lock plays RSB's own short fade and then calls Windows' lock:
Windows' lock screen is drawn on a secure desktop no program can replace, and
RSB does not draw an imitation of one. Restart and shut down never force
programs closed: Windows' own "apps are preventing shutdown" screen still
protects unsaved work.

### Data flow of a popup

A click on the dashboard button, from the pointer to the panel:

1. The window layer sends the press to the UI thread's bar scene; the manager
   gives the press a gesture id, and the click handler call that follows
   carries it to the script thread.
2. The handler sets the popup's `open` state. The widget's tree is rebuilt
   with `open` true and a new `open_seq`, and the popup's content is built in
   that same turn; the trees go back to the UI thread as `ScriptOut::Trees`.
3. The bar scene takes the tree and reports a `PopupDecl` (anchor rectangle,
   flat edges, props, content path). `Popups::frame` places it, creates its
   window and scene, hands the scene its content and opens the popup's scene
   and its anchor's together.
4. The popup scene runs the same style, layout and draw-list steps as a bar
   scene; the renderer draws the damaged part into the popup window's DIB and
   it is presented.
5. Pointer, keys, text and clipboard reach the popup through the window layer
   and the manager into its scene. What the script must run goes back as
   `ScriptIn::Input` and `ScriptIn::Element` calls.
6. A close the UI decides (outside press, Escape, anchor gone, fullscreen,
   session lock, another popup of the group) marks the popup closing, tells
   the anchor's scene, and sends the script `PopupClosed(reason)`, which makes
   it write `open = false`. A close the script asks for needs no message back.

### The popup manager

`crates/rsb/src/popups/` holds it; the UI thread owns every popup window and
scene. After the bars draw, one pass (`Popups::frame`) does this:

1. Reads the popup declarations each scene reports: the bar scenes, and the
   scene of every open popup (a menu inside a panel is declared by the panel).
2. Closes what vanished (its anchor is gone: `anchor_gone`) or that the script
   closed (`open` false: `command`, the script is not told again), and keeps
   the rest in step with their anchor and their tree.
3. Opens what turned open. An overlay with no gesture is refused first. So is a
   popup that turns open with the gesture of the press that last closed it as
   `Outside` (that press also clicked a button that toggles it). Then the other
   open popup of its group on its monitor is closed (`replaced`), never one of
   its own chain.
4. Draws each popup surface and presents it, then follows anchors: every pass
   a popup is placed again from its anchor and its natural size, and its window
   moves or resizes on a change of a pixel or more.
5. Turns the outside-press hook on exactly while an open popup without
   keyboard focus closes on outside presses, and off when there is none. A
   popup that a widget opened on its own (no click or key) holds the hook for
   a minute at most. The pass asks for the hook on every time while it is
   wanted, which starts it again if its thread ended.

A popup goes through `Opening`, `Open` and `Closing`. Its scene draws one
frame in the closed style (nothing visible), then it and its anchor's scene
are set open together at one instant, so the strip and the panel run their
transitions on one clock. When it closes the scene keeps its last content and
ignores input until the transitions are settled (or a second has passed); then
the window goes. A popup told to open again while it closes keeps its window.

Windows are placed in physical pixels, in virtual-screen coordinates; the window layer moves them
off the screen in a test run. A surface that needs a size asks its scene
(`natural_size`), capped at the work area; the side asked for is guessed first
and the size taken again when `place` flips it.

Gestures (`gesture.rs`): every pointer-down, key-down and `rsb popup` gets a new
non-zero id; the click that follows a press carries the press's id, a key's
handler calls and element events carry the key's, and `ScriptIn::Popup` carries
the command's. The last 16 are kept with their time, which is how a popup's
focus request is judged: it is honoured for a gesture at most a second old.
A popup opened with no gesture never takes focus; a press inside a popup that
can take focus and does not hold it gives it focus. A submenu opened by a key
is the key's gesture and takes focus with its first item focused; closing it
gives focus back to its parent.

Keys, text, IME composition and clipboard requests from the scene go through
the manager (`input.rs`): keys reach a surface only when it holds focus, a paste
asks the clipboard worker (500 ms limit) and the answer is handed to the scene,
and the scene's events are sent to the script after every key, text, paste,
pointer event and frame. In a test run the clipboard is a stand-in (a fixed
text for a paste, nothing written for a copy). A menu item picked by a click,
Enter or Space (one that opens no submenu) is reported by the scene in the
pointer or key result, so the manager closes the whole menu chain after the
item's handler was sent, whether or not the menu has an `on_close`. Every popup
of the chain tells its own `on_close` that it closed, so the root menu's
handler hears `picked` once for a pick in any submenu and a submenu needs no
handler.

An overlay is two or three windows: the dim (and the optional backdrop) over the
monitor, and the content window the size of what its buttons draw. The dim's
colour is the overlay root's background as drawn now; only its window alpha
changes after the first paint. A press on the dim after the input guard is a
press outside. While the root has the class `locking` the overlay waits for
Windows to lock: a session lock notice closes it, so does a failed power action
(the power service's error status), and so does one second without either, with
the script told in the last two cases. A UI turn that runs for over 2 s while an
overlay is shown has the overlay windows hidden by the watchdog thread (it
wakes four times a second only while an overlay is shown); the manager then
closes the overlay and tells the script.

## Keyboard and text input

Keys reach only a popup that holds focus. The scene handles what must be
immediate (text editing, caret, selection, IME composition drawn at the
caret, sliders, scrolling, Tab and arrow focus movement, Enter and Space as a
click) and sends the rest to the focused element's `on_key` handler. Escape
closes the focused popup (a submenu closes one level) unless the popup keeps
Escape for itself. The IME is attached only while a text input that is not a
password has focus: the manager asks the scene (`focused_password`) after every
frame, so a popup with both a password field and a search field switches the
IME per field.

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

Bars and popups are drawn on the CPU (tiny-skia, Parley, swash, resvg) into
top-down 32-bit DIB sections and presented with `UpdateLayeredWindowIndirect`.
Shapes that are not rounded rectangles (bevelled corners, `clip-path`
polygons) are drawn as outlines; SVG icons are drawn in the element's colour.
JetBrains Mono (SIL Open Font License 1.1, licence in `assets/fonts`) ships
inside the binary for the default look. Measured against
Direct2D and wgpu for the same bar, it used the least memory (about 3 MB
private versus 42 MB and 115 MB) and started fastest, and drew a clock tick
in under half a millisecond. Text is grayscale anti-aliased: per-pixel alpha
surfaces cannot use ClearType on any of the three paths.

### Fonts

The bundled JetBrains Mono is registered with the renderer before the first
frame. A style sheet may add fonts with `@font-face`: the config reader
thread reads each named file from `<config>\fonts` (a plain file name, never a
path; links that lead out of the folder are refused; at most 16 files of at
most 16 MiB) and the UI thread registers it under the rule's family name, then
tells every scene to measure again. A file that is missing, too large, not a
font or over the limit is a diagnostic: a line in the log and a part of the
bar plate. The reader remembers which files (name, family, size, time) the UI
already has, so a style edit does not read or register them again; a change
in `fonts\` reads the sheet's font rules again. The "font is not installed"
warning is made after registration, so it knows every registered family.

### Pictures

- Bundled icons (`assets\icons\<name>.svg`, the names in
  `rsb_core::icons::SVG_ICON_NAMES`) are set into the renderer the first time
  a frame draws one; they are a few hundred bytes, so this takes place on the
  UI thread.
- A user's `.svg` file (an icon's `svg` path, or an image with an `.svg`
  path) is opened under the widget's file permission, like a PNG, read to at
  most 1 MiB and parsed on the image loader thread; the UI thread keeps the
  result and tells the scene its size. A node that draws that file is
  damaged and redrawn on the next frame even when its draw command did not
  change.
  Drawing it rasterises on the UI thread, so it is bounded: a picture is
  rasterised at 512 px at most (a larger one is stretched from that), a file
  whose layers times pixels pass a cost cap is not drawn, and one frame spends
  about 8 ms on such rasters at most. Rasters left over make the renderer report
  `svg_pending`, and the host draws the surface again on its next frame until
  every picture is there.
- Pictures from services arrive as `HubEvent::Image`; the engine's hub sink
  sends them to the UI thread only (`UiMsg::ServiceImage`) and the script
  thread never gets the pixels. When the media service's `art` changes, the
  old `rsb-image:media/<n>` key is dropped from the renderer by exact key
  (`media/1` leaves `media/10` alone). `App::remove_app_images` drops every
  `rsb-image:apps/` picture once no open popup uses them.

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
(user widgets get window titles, media and the network by default; Bluetooth
device names, the installed apps and Wi-Fi network names must be declared).
Restart, shut down and sign out are a permission of their own
(`control:session`), apart from lock and sleep; covering the whole screen is
the `overlay` permission. Commands that change the PC (Bluetooth, the default
output, starting apps, Wi-Fi, sleep and the session actions) work only while
the user's click or key is being handled, at most one every 2 s, and an app
or device must be one the service listed.
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
- Fullscreen apps: `ABN_FULLSCREENAPP` drops the bar below them or hides it,
  and closes open popups (a popup must not stay above a game). RSB's own
  full-screen overlay is not a fullscreen app.
- The bar never takes focus and has no taskbar button; neither do popups,
  except as described under Popups.
- Animation frames are timed with a high-resolution waitable timer and moved
  to the start of the compositor's next frame, so each display frame gets
  one new picture instead of the timer drifting against the display.
- Session lock and unlock (`WM_WTSSESSION_CHANGE`) close every popup.
- Runs with a test flag pass every service command through a gate that
  records it and, unless the services are fakes, stops it: a simulated click
  never changes the PC it runs on. See "Test runs" under Command line.

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
| `rsb popup <name> [open\|close\|toggle]` | opens, closes or toggles the named popup on the monitor under the pointer (for hotkey tools) |
| `rsb render --out <png>` | draws the bar to a PNG without a window, with sample values or (`--live`) this PC's |
| `rsb settings <verb>` | reads and changes the preset, a bar's position, height and floating, the zones and the accent colour; one line of JSON, see `docs/settings.md` |
| `rsb inspect [widget]` | what each widget draws and costs: element tree, services, timings, memory; see `docs/inspect.md` |

Exit codes: 0 done (or already running), 1 the command failed, 2 bad usage,
3 no bar is running, 4 the bar could not be reached (another user or session
owns the pipe, or no answer within 2 s), 5 the bar could not start.

### Test runs

Hidden flags drive the bar for tests and measurements. The popup flags:
`--simulate-key <file>` (lines `<ms> popup:<name> key <Key> [down|up]
[ctrl+shift+alt] [repeat]`, `text <text>` with `\uXXXX` escapes and `\s` for a
space, `ime start|update <text> <cursor>|commit <text>|end`),
`--simulate-pointer` with `popup:<name>` and `dim:<name>` surfaces next to a
bar name, `--simulate-popup <ms>:<name>:open|close|toggle` (as `rsb popup`, with
a gesture), `--simulate-outside-press <ms>`, `--simulate-deactivate
<ms>:<popup>`, `--simulate-session <ms>:lock|unlock`, `--simulate-popup-dpi
<ms>:<popup>:<dpi>`, `--test-ui-stall <ms>:<for ms>` (the UI thread sleeps, so
the watchdog's overlay check can be seen), and `--no-hook` and `--no-activate`
(no real hook, no real focus: focus is simulated and keys are sent to the
popup's own window). A popup test run also writes `popup` (`state` open, shown,
close, closed, moved or refused, the window rectangle, the click-to-first-frame
time), `focus`, `hook`, `ime` (a stage, never text), `clipboard` (an operation,
never text), `element` and `watchdog` stats lines.
 Every flag that
clicks, types or changes the environment (`--fake-services`,
`--fake-glazewm`, `--service-storm`, every `--simulate-*`, every `--test-*`,
`--no-hook`, `--no-activate`, `--capture`), a set `RSB_TEST_TAG` and every
`rsb render` select a command recorder in the services hub. It sees each
service command on the hub thread before its service is started and writes a
`command` stats line with the service and the method name (never an id).
Only with fake services does it let the command through, so the fake's value
changes as a real PC's would; with the real services the command stops there.
The same runs give the power service a stub that reports `lock`, `sleep`,
`restart`, `shut_down` or `sign_out` and does nothing. `--seconds`,
`--no-reserve`, `--stats-*` and `--probe-ui` only measure a normal run and
select nothing. A run with none of the above has no recorder: its commands
reach the services and the power actions are Windows'. `--capture <dir>`
needs `--fake-services`, `--test-offscreen` and a folder under a scratch or
`target` folder (exit 2 otherwise, so a real launcher list or track title can
never become a picture). Every present of every surface is written as
`<surface>-<t_ms>.png` with a line in `index.jsonl` (`t_ms`, `surface`,
`rect`, `size`); the run stops writing at 2000 frames or 120 s and says so in a
`capture` stats line. `--simulate-monitors <WxH@dpi>[,...]` (offscreen runs,
at most four) adds monitors that do not exist, to the right of the real ones;
each gets its bars from the same selectors as a real monitor, and popups,
overlays and the dim are placed on them by the same code.

### Timelines

`rsb render --timeline <file> --out-dir <dir>` plays a script on a virtual
clock and writes pictures, with no window, no hook and no real service
(`--live` is refused; the output folder follows the `--capture` rule). It runs
the same scenes, popup manager, script thread and fake services as the bar:
`headless.rs` implements the window layer in memory (a bar sits at its edge
and shrinks the work area, a present keeps its pixels, a dim keeps its
colour), `timeline_run.rs` replaces the clock with one that moves only when
the script says so, and the wall clock stays at `--now`. Between two lines it
runs the UI turns that a real run would have had in that time, so a
transition at 100 ms is drawn at 100 ms whatever the PC is doing.

Lines are `<ms> pointer <surface> move|down|up|wheel|hwheel|leave <x> <y>`,
`<ms> key`, `<ms> text`, `<ms> ime`, `<ms> popup <name> open|close|toggle`,
`<ms> outside`, `<ms> capture <label> [<surface>|screen]` and
`<ms> fake <service> <json>` (sets those fields of the fake service's value).
A `screen` capture lays every bar, popup and dim over the background (a flat
colour, or a PNG named by `--background`) at their screen positions, in the
order the windows stack. A capture of a popup that is not open is a failed
step, not a blank picture; the run says how many steps failed and exits 1.
A bad line is refused before anything runs, with its line number.

Stats for the measurements: a `frame` line per present
(`--stats-frames`; `surface` is `bar:<name>` or `popup:<name>`, with
`render_us`, `build_us` (the whole scene frame), its parts `style_us`, `layout_us`
and `paint_us`, and `compositor_frame`), the `popup`
closed line of an overlay carries `dim_paints` (the dim is painted once and
only its alpha changes), and a `pacing` line at the end of a run counts
presents per display period from `Ui::compositor_period`, which is the number
to use over Remote Desktop where `compositor_frame` means nothing. The `probe`
line (`--probe-ui`) gives the worst round trip of the run and
`worst_after_first_frame_ms`, the worst after the first bar frame was drawn.

## Settings and inspect

Two groups of commands serve a settings page without any window of RSB's own.
Both are a request on the pipe or a command line; the answer is one line of
JSON under 65,536 bytes.

```text
rsb settings <verb>                       rsb inspect [widget]
  CLI process                               CLI process, pipe line (paged)
   settings::serve(Request, Ctx::cli)           |
   edit:  cfgedit::plan_change (pure)           v
   write: cfgwrite::apply (lock, checks,     pipe thread: inspect::serve
          two-phase rename, undo entry)       status::snapshot asks, with one
   log:   one `config:` line per write        1 s deadline: the script thread
   ask the bar: pipe `applied`, every         (ScriptIn::Inspect), the hub, and
          50 ms for up to 2 s                 the UI thread (UiMsg::Inspect: a
                                              bounded copy of each scene)
  the bar, pipe thread
   `settings <json>`: settings::serve_line
   (Ctx::pipe): reads inline, writes only
   when the lock is free at once (else `busy`)
```

- **One function behind both transports.** `settings::serve` runs in the
  command-line process, so it works with no bar, and on the bar's pipe thread
  for a `settings` line. `widgets` and `preview` (Luau modules, PNGs) are
  command line only. A write over the pipe needs a client at or above the bar's
  level of rights, never waits for the writer's lock or retries a file that is
  busy, and runs no widget module (so it does not check which popup menus a
  removed widget opened).
- **Editing the files.** `rsb-core::cfgedit` plans a change as byte splices
  into the text of `bar.toml` and `style.css` (comments, layout and line
  endings stay), validates every value, and checks that the new text loads with
  the same fields. `rsb::cfgwrite` writes it: one lock file under `.undo`,
  checks on open handles, a temporary file renamed over the target after its
  file id is checked again, one undo entry per request (the last 20 are kept),
  and `.undo\written` for the "edited by hand" check. A hand edit made in
  between is kept and reported; an `--expect` version refuses a changed file.
- **The bar reloads through its folder watch only.** Nothing sends `reload`.
  The reader skips a save whose bytes equal the last build and keeps a
  last-good copy of each file for `settings restore`. After every swap the UI
  thread publishes the versions of the files it shows (`AppliedState`, FNV-1a
  of the raw bytes); the pipe line `applied` reads them with no round trip,
  and the command line polls it until the bar shows the change.
- **Presets** carry an `[about]` table (title, description, schemes, the colours
  a page may tune, the pairs the contrast check measures). `rsb-core::contrast`
  resolves a preset's custom properties, measures every pair, and derives the
  accent's text, line and hover shades so each pair passes in every preset.
  `settings preview` draws a preset through the render path and caches the PNG
  under `%LOCALAPPDATA%\rsb\previews`.
- **Inspect.** The script thread keeps per-widget run counts and times in
  one-second buckets, the scene attributes each built frame to the trees that
  caused it, and the hub keeps poll times. A request copies at most 600 nodes
  per widget on the UI thread, pages the summary, and cuts a tree to fit one
  reply with the cut stated. Nothing is written to the log.

## Files

- Config: `%APPDATA%\rsb` (or `--config-dir`, or `RSB_CONFIG_DIR`):
  `bar.toml`, `style.css`, `widgets\`, `types\rsb.d.luau`, `.luaurc`,
  `.vscode\settings.json`, and `.undo\` (the undo entries and last-good copies
  that `rsb settings` keeps; the bar ignores the folder).
- Logs: `%LOCALAPPDATA%\rsb\logs`, rotated. Logs hold counts, sizes and
  timings, never window titles, media names, network names or file paths
  outside the config folder.

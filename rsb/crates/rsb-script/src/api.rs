//! The Luau API, version 1: the one description that both the runtime
//! bindings and the generated type file (`types/rsb.d.luau`) follow. A name
//! here is a promise to widget authors; renaming one breaks their files.
//!
//! Conventions: snake_case everywhere; constructors take one table
//! (`rsb.text { text = "hi" }`); durations are seconds (`2`, `0.5`) or
//! strings with a unit (`"500ms"`, `"2s"`, `"1m"`, `"1h"`); any prop may be
//! a plain value, a signal, or a function (re-run when what it reads
//! changes); calls marked `yields` wait for a result and work only inside a
//! task (handlers, timers, `rsb.task.spawn`, `rsb.poll`). Elements are
//! opaque values made by the constructors; a hand-made table is refused.
//! Handlers take one event table (`on_click = function(e) ... e.button`),
//! the shape the rest of the suite follows.
//!
//! The `*Props` types repeat the common props instead of using an
//! intersection (`Props & {...}`): luau-lsp's strict checker often rejects
//! table literals against intersections.

/// A named Luau type.
#[derive(Clone, Copy, Debug)]
pub struct ApiType {
    pub name: &'static str,
    /// Generic parameters including the angle brackets, or "".
    pub generics: &'static str,
    /// The type expression after `=`.
    pub def: &'static str,
    pub doc: &'static str,
}

/// A function or value on the `rsb` module.
#[derive(Clone, Copy, Debug)]
pub struct ApiItem {
    /// Dotted path from the module, e.g. `timer.every`.
    pub path: &'static str,
    /// Luau type of the item (a function type for functions).
    pub ty: &'static str,
    pub doc: &'static str,
    /// Waits for a result: only inside a task.
    pub yields: bool,
    /// A short example, one or two lines.
    pub example: &'static str,
}

const fn t(name: &'static str, generics: &'static str, def: &'static str, doc: &'static str) -> ApiType {
    ApiType { name, generics, def, doc }
}

const fn i(path: &'static str, ty: &'static str, doc: &'static str, yields: bool, example: &'static str) -> ApiItem {
    ApiItem { path, ty, doc, yields, example }
}

/// Module-level names reserved for later versions. Using one raises
/// "rsb.<name> is not available in this version".
pub const RESERVED: &[&str] = &["settings", "tray", "graph", "open", "ipc"];

/// Names people try that live elsewhere (also never used for anything
/// else): `rsb.sleep` raises
/// "rsb.sleep does not exist; use rsb.task.sleep". Any other unknown name
/// gets a did-you-mean from `API_ITEMS` (edit distance <= 2).
pub const HINTS: &[(&str, &str)] = &[
    ("spawn", "task.spawn"),
    ("sleep", "task.sleep"),
    ("wait", "task.sleep"),
    ("delay", "timer.after"),
    ("cancel", "task.cancel"),
    ("print", "log.info"),
];

/// Element props reserved for later versions (same error).
pub const RESERVED_PROPS: &[&str] = &["draggable"];

/// The props every element takes, followed by the element's own fields.
macro_rules! props {
    ($own:literal) => {
        concat!(
            "{ id: string?, class: Reactive<string | { string }>?, style: Reactive<Style>?, key: string?, visible: Reactive<boolean>?, ",
            "on_click: ((ClickEvent) -> ())?, on_right_click: ((ClickEvent) -> ())?, on_middle_click: ((ClickEvent) -> ())?, ",
            "on_wheel: ((WheelEvent) -> ())?, on_enter: (() -> ())?, on_leave: (() -> ())?, ",
            "on_key: ((KeyEvent) -> ())?, on_scroll: ((ScrollEvent) -> ())?, on_focus: (() -> ())?, on_blur: (() -> ())?, ",
            "tooltip: Reactive<string | Element>?, menu: Reactive<{ MenuItem }>?, focusable: boolean?, autofocus: boolean?, ",
            "disabled: Reactive<boolean>?, checked: Reactive<boolean>?, nav: (\"vertical\" | \"horizontal\" | \"grid\")?, ",
            "reveal: Reactive<any>?, animate: Reactive<Animate?>?",
            $own,
            " }"
        )
    };
}

pub static API_TYPES: &[ApiType] = &[
    t(
        "Duration",
        "",
        "number | string",
        "Seconds as a number, or a string with a unit: \"500ms\", \"2s\", \"1m\", \"1h\"",
    ),
    t(
        "Signal",
        "<T>",
        "{ value: T, peek: (self: any) -> T }",
        "A value that can change; reading `.value` inside a function prop or effect re-runs it on change",
    ),
    t(
        "State",
        "<T>",
        "{ value: T, peek: (self: any) -> T, update: (self: any, fn: (T) -> T) -> () }",
        "A signal you set: `count.value = count.value + 1`",
    ),
    t(
        "Reactive",
        "<T>",
        "T | Signal<T> | State<T> | () -> T",
        "A prop value: plain, a signal, or a function that is re-run when what it reads changes",
    ),
    t(
        "PollSignal",
        "<T>",
        "{ value: T?, error: string?, peek: (self: any) -> T? }",
        "rsb.poll's result: the last value, and the last error (nil after a good run)",
    ),
    t(
        "Child",
        "",
        "Element | string | number | nil | () -> { Element }",
        "A child: an element, text, nothing, or a function returning a list (re-run on change)",
    ),
    t(
        "Element",
        "",
        "{ kind: \"box\" | \"text\" | \"icon\" | \"image\" | \"popup\" | \"menu\" | \"input\" | \"slider\" | \"scroll\" }",
        "Something to draw; made by rsb.box, rsb.row, rsb.column, rsb.text, rsb.icon, rsb.image, rsb.popup, rsb.menu, rsb.input, rsb.slider, rsb.scroll (opaque: tables you build yourself are refused)",
    ),
    t(
        "ClickEvent",
        "",
        "{ button: \"left\" | \"right\" | \"middle\", x: number, y: number }",
        "x and y are logical px inside the element",
    ),
    t("WheelEvent", "", "{ delta: number, x: number, y: number }", "delta: wheel notches, positive = up"),
    t("Props", "", props!(""), "Props every element takes"),
    t("BoxProps", "", props!(", [number]: Child"), "Children go in the array part"),
    t("TextProps", "", props!(", text: Reactive<string | number>?"), "One line of text"),
    t(
        "IconProps",
        "",
        props!(", name: Reactive<IconName>?, glyph: Reactive<string>?, svg: Reactive<string>?"),
        "A named icon, any glyph text in the icon font, or an SVG icon (a bundled name or a path to an .svg file)",
    ),
    t(
        "ImageProps",
        "",
        props!(", src: Reactive<string>, width: number?, height: number?"),
        "A PNG or SVG, path relative to the widget's folder; or an image key a service value gives (needs that service's read permission)",
    ),
    t(
        "MonitorInfo",
        "",
        "{ index: number, handle: number, device: string, primary: boolean, scale: number }",
        "The monitor a widget instance is on",
    ),
    t(
        "Context",
        "",
        "{ id: string, name: string, bar: string, monitor: MonitorInfo, dir: string }",
        "About this instance: ref text (\"clock#utc\"), widget name, bar name, monitor (scale updates in place), folder of the widget file relative to the config folder",
    ),
    t(
        "Permissions",
        "",
        "{ exec: { string }?, file: { string }?, net: { string }?, control: { \"audio\" | \"media\" | \"workspaces\" | \"microphone\" | \"bluetooth\" | \"apps\" | \"power\" | \"wifi\" | \"session\" }?, read: { \"window\" | \"media\" | \"network\" | \"bluetooth\" | \"apps\" | \"wifi\" }?, overlay: boolean?, popups: { string }? }",
        "What the widget may do besides drawing and reading services (read: private service values; user widgets get window, media and network without asking; session: restart, shut down, sign out; overlay: a popup that covers the monitor; popups: open or close another widget's popup by name)",
    ),
    t(
        "WidgetSpec",
        "<O>",
        "{ name: string?, options: O?, optional: { string }?, permissions: Permissions?, setup: (props: O, ctx: Context) -> Element? }",
        "A widget: option defaults (bar.toml overrides them), permissions, and setup returning its element; optional lists options that have no default (a table cannot hold nil), so bar.toml may still set them",
    ),
    t("Widget", "", "{ name: string }", "What a widget file returns"),
    t("Task", "", "{ cancel: (self: any) -> () }", "A running task"),
    t("Timer", "", "{ cancel: (self: any) -> () }", "A running timer"),
    t("Subscription", "", "{ cancel: (self: any) -> () }", "An event subscription"),
    t(
        "Time",
        "",
        "{ year: number, month: number, day: number, weekday: number, hour: number, minute: number, second: number, millis: number, utc_offset: number }",
        "Wall-clock time; weekday 0 = Sunday; utc_offset in minutes",
    ),
    t(
        "ExecOptions",
        "",
        "{ timeout: Duration?, cwd: string?, max_output: number? }",
        "timeout default 30 s (max 10 min); max_output bytes per stream, default 1 MiB (max 8 MiB)",
    ),
    t(
        "ExecResult",
        "",
        "{ ok: boolean, code: number?, stdout: string, stderr: string, timed_out: boolean, truncated: boolean, error: string? }",
        "ok = exit code 0",
    ),
    t(
        "SpawnOptions",
        "",
        "{ cwd: string?, on_line: ((line: string, stream: \"stdout\" | \"stderr\") -> ())?, on_exit: ((code: number?) -> ())? }",
        "A long-running process; killed when the widget unloads",
    ),
    t("Process", "", "{ kill: (self: any) -> () }", "A spawned process"),
    t(
        "HttpOptions",
        "",
        "{ headers: { [string]: string }?, timeout: Duration?, cache: Duration?, max_bytes: number? }",
        "timeout default 10 s (max 60); cache: reuse a good response younger than this; max_bytes default 2 MiB (max 8)",
    ),
    t(
        "Response",
        "",
        "{ ok: boolean, status: number, body: string, headers: { [string]: string }, from_cache: boolean, error: string?, json: (self: any) -> any }",
        "ok = status 200-299; on a network failure status is 0 and error says why",
    ),
    t(
        "PopupProps",
        "",
        "{ id: string?, class: Reactive<string | { string }>?, style: Reactive<Style>?, key: string?, open: State<boolean> | Reactive<boolean>, name: (string | { string })?, group: string?, placement: (\"auto\" | \"below\" | \"above\" | \"left\" | \"right\" | \"screen\")?, align: (\"start\" | \"center\" | \"end\")?, offset: number?, attach: boolean?, focus: (\"auto\" | \"none\" | \"always\")?, dismiss: { outside: boolean?, escape: boolean? }?, on_close: ((reason: CloseReason) -> ())?, on_open: ((name: string) -> ())?, on_key: ((KeyEvent) -> ())?, [number]: Child }",
        "A popup anchored to the element it is a child of. open given a State is two-way: the UI writes false when it closes the popup. Children are built only while it is open",
    ),
    t(
        "CloseReason",
        "",
        "\"outside\" | \"escape\" | \"deactivated\" | \"anchor_gone\" | \"bar_hidden\" | \"display_change\" | \"replaced\" | \"picked\" | \"command\" | \"refused\"",
        "Why a popup closed",
    ),
    t(
        "MenuItem",
        "",
        "{ label: string?, icon: IconName?, svg: string?, shortcut: string?, checked: boolean?, disabled: boolean?, separator: boolean?, on_click: (() -> ())?, items: { MenuItem }? }",
        "One menu entry; items makes a submenu (at most 256 entries and 4 levels per menu)",
    ),
    t(
        "MenuProps",
        "",
        "{ id: string?, class: Reactive<string | { string }>?, key: string?, open: State<boolean> | Reactive<boolean>, items: Reactive<{ MenuItem }>, name: (string | { string })?, placement: (\"auto\" | \"below\" | \"above\" | \"left\" | \"right\")?, on_close: ((reason: CloseReason) -> ())? }",
        "A menu; as a child it is anchored to its parent, like a popup",
    ),
    t(
        "InputProps",
        "",
        props!(
            ", value: State<string> | Reactive<string>?, placeholder: Reactive<string>?, password: boolean?, max_length: number?, on_change: ((text: string) -> ())?, on_submit: ((text: string) -> ())?"
        ),
        "One line of editable text; value given a State is two-way",
    ),
    t(
        "SliderProps",
        "",
        props!(
            ", value: State<number> | Reactive<number>, min: number?, max: number?, step: number?, vertical: boolean?, jump: boolean?, on_change: ((value: number, done: boolean) -> ())?"
        ),
        "A value on a track; value given a State is two-way; done is true on the last report of a drag",
    ),
    t(
        "KeyEvent",
        "",
        "{ key: string, ctrl: boolean, shift: boolean, alt: boolean, [\"repeat\"]: boolean }",
        "A key press: key follows the web's KeyboardEvent.key (\"Enter\", \"Escape\", \"ArrowDown\", \"a\"); repeat is a Luau keyword, so read it as e[\"repeat\"]",
    ),
    t("ScrollEvent", "", "{ x: number, y: number }", "The new scroll offset in logical px"),
    t("Animate", "", "{ name: string }", "An animate prop value: run @keyframes name (made by rsb.animate)"),
    t(
        "AnimateOptions",
        "",
        "{ duration: Duration?, easing: string?, delay: Duration?, iterations: number?, direction: string?, fill: string?, restart: any? }",
        "restart: the animation starts again from the start whenever this value changes",
    ),
    t("FileOptions", "", "{ max_bytes: number? }", "Default 1 MiB, max 8 MiB"),
    t("FileSignal", "", "{ value: string?, error: string? }", "The file's text, kept current; nil while it is missing"),
];

pub static API_ITEMS: &[ApiItem] = &[
    i("version", "number", "API version, 1", false, "if rsb.version >= 1 then ... end"),
    // Widgets and elements
    i(
        "widget",
        "<O>(spec: WidgetSpec<O>) -> Widget",
        "Defines a widget; a widget file returns this",
        false,
        "return rsb.widget { options = { format = \"HH:mm\" }, setup = function(props, ctx) return rsb.text(props.format) end }",
    ),
    i(
        "box",
        "(props: BoxProps) -> Element",
        "A flex container, children in a row",
        false,
        "rsb.box { class = \"pill\", rsb.text \"a\", rsb.text \"b\" }",
    ),
    i(
        "row",
        "(props: BoxProps) -> Element",
        "A box laid out left to right",
        false,
        "rsb.row { rsb.icon \"cpu\", rsb.text \"12%\" }",
    ),
    i(
        "column",
        "(props: BoxProps) -> Element",
        "A box laid out top to bottom",
        false,
        "rsb.column { rsb.text \"up\", rsb.text \"down\" }",
    ),
    i(
        "text",
        "(props: TextProps | string | number) -> Element",
        "One line of text (what comes before the first line break)",
        false,
        "rsb.text { text = function() return `{cpu.value}%` end }",
    ),
    i(
        "icon",
        "(props: IconProps | IconName) -> Element",
        "An icon by name, or a raw glyph",
        false,
        "rsb.icon { name = \"volume_3\" }",
    ),
    i(
        "image",
        "(props: ImageProps) -> Element",
        "A PNG image (needs file permission outside the widget folder)",
        false,
        "rsb.image { src = \"logo.png\", width = 16 }",
    ),
    i(
        "each",
        "<T>(list: Reactive<{ T }>, render: (item: T, index: number) -> Element, key: ((item: T) -> string)?) -> () -> { Element }",
        "Children from a list; with key, unchanged items keep their state",
        false,
        "rsb.row { rsb.each(function() local m = ws.monitors return if m and m[1] then m[1].workspaces else {} end, function(w) return rsb.text(w.display_name) end, function(w) return w.name end) }",
    ),
    i(
        "popup",
        "(props: PopupProps) -> Element",
        "A popup anchored to the element it is a child of; its content is built only while it is open",
        false,
        "rsb.popup { open = shown, function() return rsb.text \"hi\" end }",
    ),
    i(
        "menu",
        "(props: MenuProps) -> Element",
        "A menu (as a child: anchored; the menu prop of any element opens one at a right click)",
        false,
        "rsb.menu { open = m, items = { { label = \"Reload\", on_click = reload } } }",
    ),
    i(
        "input",
        "(props: InputProps) -> Element",
        "One line of editable text",
        false,
        "rsb.input { value = query, placeholder = \"Search\", autofocus = true }",
    ),
    i(
        "slider",
        "(props: SliderProps) -> Element",
        "A value on a track",
        false,
        "rsb.slider { value = vol, on_change = function(v) audio:set_device_volume(id, v) end }",
    ),
    i(
        "scroll",
        "(props: BoxProps) -> Element",
        "A column that scrolls",
        false,
        "rsb.scroll { class = \"list\", rsb.each(items, row) }",
    ),
    i(
        "popups.open",
        "(name: string) -> ()",
        "Opens the named popup on this widget's monitor (from a user widget only inside a click or key handler, unless it is the widget's own popup)",
        false,
        "on_click = function() rsb.popups.open(\"launcher\") end",
    ),
    i(
        "popups.close",
        "(name: string) -> ()",
        "Closes the named popup on this widget's monitor",
        false,
        "rsb.popups.close(\"launcher\")",
    ),
    i(
        "popups.toggle",
        "(name: string) -> ()",
        "Opens the named popup, or closes it when it is open",
        false,
        "on_click = function() rsb.popups.toggle(\"launcher\") end",
    ),
    i(
        "popups.is_open",
        "(name: string) -> boolean",
        "Whether the named popup is open on this widget's monitor; read inside a function prop it updates",
        false,
        "class = function() return rsb.popups.is_open(\"power\") and \"on\" or \"\" end",
    ),
    i(
        "animate",
        "(name: string, options: AnimateOptions?) -> Animate",
        "An animate prop value: runs @keyframes name, again from the start when options.restart changes",
        false,
        "animate = function() return rsb.animate(\"flash\", { duration = \"300ms\", restart = count.value }) end",
    ),
    i(
        "fuzzy",
        "<T>(query: string, items: { T }, options: { key: ((T) -> string)?, limit: number? }?) -> { T }",
        "The items that match query, best first, typo tolerant",
        false,
        "rsb.fuzzy(q.value, apps.apps, { key = function(a) return a.name end, limit = 50 })",
    ),
    i(
        "fuzzy_match",
        "(query: string, text: string) -> { number }?",
        "The 1-based character positions of text that matched query, for highlighting; nil when it does not match",
        false,
        "local hit = rsb.fuzzy_match(q.value, app.name)",
    ),
    // Signals
    i(
        "state",
        "<T>(initial: T) -> State<T>",
        "A value you can set; readers update when it changes",
        false,
        "local count = rsb.state(0)",
    ),
    i(
        "computed",
        "<T>(fn: () -> T) -> Signal<T>",
        "A value derived from others; recomputed when they change",
        false,
        "local label = rsb.computed(function() return `{count.value} items` end)",
    ),
    i(
        "effect",
        "(fn: () -> (() -> ())?) -> ()",
        "Runs fn now and again whenever what it reads changes; fn may return a cleanup",
        false,
        "rsb.effect(function() rsb.log.info(`volume {audio.volume}`) end)",
    ),
    i(
        "batch",
        "(fn: () -> ()) -> ()",
        "Several changes, one update",
        false,
        "rsb.batch(function() a.value = 1; b.value = 2 end)",
    ),
    i(
        "untrack",
        "<T>(fn: () -> T) -> T",
        "Reads without subscribing",
        false,
        "local v = rsb.untrack(function() return count.value end)",
    ),
    i(
        "poll",
        "<T>(every: Duration, fn: () -> T) -> PollSignal<T>",
        "Runs fn now and every period inside a task (it may wait on I/O); .value holds the last result (nil before the first), .error the last error (the value is kept)",
        false,
        "local ip = rsb.poll(\"5m\", function() return rsb.http.get(\"https://api.ipify.org\").body end)",
    ),
    // Lifecycle and tasks
    i(
        "on_cleanup",
        "(fn: () -> ()) -> ()",
        "Runs fn when the widget unloads or reloads",
        false,
        "rsb.on_cleanup(function() rsb.log.info(\"bye\") end)",
    ),
    i(
        "task.spawn",
        "(fn: () -> ()) -> Task",
        "Starts a task: code that may wait on I/O without blocking anything",
        false,
        "rsb.task.spawn(function() local r = rsb.http.get(url) end)",
    ),
    i("task.sleep", "(d: Duration) -> ()", "Waits inside a task", true, "rsb.task.sleep(\"500ms\")"),
    i("task.cancel", "(task: Task) -> ()", "Stops a task (same as task:cancel())", false, "rsb.task.cancel(t)"),
    i(
        "timer.every",
        "(d: Duration, fn: () -> ()) -> Timer",
        "Calls fn every d; whole seconds, minutes and hours line up with the clock",
        false,
        "rsb.timer.every(\"1m\", refresh)",
    ),
    i(
        "timer.after",
        "(d: Duration, fn: () -> ()) -> Timer",
        "Calls fn once after d",
        false,
        "rsb.timer.after(2, function() shown.value = false end)",
    ),
    // Time
    i(
        "clock",
        "(precision: (\"second\" | \"minute\" | \"hour\")?, timezone: string?) -> Signal<Time>",
        "The time, updated on each whole second / minute / hour and at once after sleep or a clock change; timezone \"local\" (default), \"utc\" or \"+10:00\"",
        false,
        "local now = rsb.clock(\"minute\")",
    ),
    i("time.now", "(timezone: string?) -> Time", "The time now", false, "local t = rsb.time.now(\"utc\")"),
    i(
        "time.format",
        "(t: Time, format: string) -> string",
        "Formats a time: yyyy MM MMM dd ddd HH hh mm ss tt (see the docs)",
        false,
        "rsb.time.format(now.value, \"ddd d MMM HH:mm\")",
    ),
    i(
        "time.precision",
        "(format: string) -> \"second\" | \"minute\" | \"hour\"",
        "The finest unit a format shows",
        false,
        "rsb.clock(rsb.time.precision(props.format))",
    ),
    // Services
    i("services.system", "SystemService", "CPU and memory", false, "rsb.services.system.cpu"),
    i("services.battery", "BatteryService", "Battery", false, "rsb.services.battery.percent"),
    i("services.audio", "AudioService", "Volume", false, "rsb.services.audio:change_volume(5)"),
    i("services.network", "NetworkService", "Network", false, "rsb.services.network.kind"),
    i("services.media", "MediaService", "Media session", false, "rsb.services.media:play_pause()"),
    i("services.window", "WindowService", "Foreground window", false, "rsb.services.window.title"),
    i(
        "services.workspaces",
        "WorkspacesService",
        "Window manager workspaces",
        false,
        "rsb.services.workspaces:focus(\"2\")",
    ),
    i(
        "services.microphone",
        "MicrophoneService",
        "Default recording device",
        false,
        "rsb.services.microphone:toggle_mute()",
    ),
    i(
        "services.bluetooth",
        "BluetoothService",
        "Bluetooth radio and paired devices",
        false,
        "rsb.services.bluetooth.devices",
    ),
    i("services.apps", "AppsService", "Start menu apps", false, "rsb.services.apps.apps"),
    i("services.power", "PowerService", "Power mode, lock and the session actions", false, "rsb.services.power:lock()"),
    i("services.wifi", "WifiService", "Wi-Fi networks in range", false, "rsb.services.wifi.networks"),
    // I/O
    i(
        "exec.run",
        "(command: { string }, options: ExecOptions?) -> ExecResult",
        "Runs a program (argument list, no shell) and waits for it",
        true,
        "local r = rsb.exec.run({ \"git\", \"-C\", dir, \"status\", \"--short\" })",
    ),
    i(
        "exec.shell",
        "(command: string, options: ExecOptions?) -> ExecResult",
        "Runs a command line through cmd.exe (needs exec permission for \"cmd\")",
        true,
        "rsb.exec.shell(\"ver\")",
    ),
    i(
        "exec.spawn",
        "(command: { string }, options: SpawnOptions?) -> Process",
        "Starts a long-running program and streams its lines",
        false,
        "rsb.exec.spawn({ \"ping\", \"-t\", \"1.1.1.1\" }, { on_line = function(l) last.value = l end })",
    ),
    i(
        "exec.lines",
        "(command: { string }) -> Signal<string?>",
        "The latest line a long-running program printed",
        false,
        "local line = rsb.exec.lines({ \"my-feed.exe\" })",
    ),
    i(
        "file.read",
        "(path: string, options: FileOptions?) -> (string?, string?)",
        "Reads a text file; returns text, or nil and the reason",
        true,
        "local text, err = rsb.file.read(\"~/notes.txt\")",
    ),
    i(
        "file.watch",
        "(path: string, options: FileOptions?) -> FileSignal",
        "A file's text, re-read whenever it changes",
        false,
        "local counter = rsb.file.watch(\"~/count.txt\")",
    ),
    i(
        "http.get",
        "(url: string, options: HttpOptions?) -> Response",
        "HTTPS GET",
        true,
        "local r = rsb.http.get(url, { cache = \"10m\" })",
    ),
    i(
        "http.post",
        "(url: string, body: string | { [any]: any }, options: HttpOptions?) -> Response",
        "HTTPS POST; a table body is sent as JSON",
        true,
        "rsb.http.post(url, { ok = true })",
    ),
    i(
        "json.decode",
        "(text: string) -> any",
        "JSON text (at most 2 MiB, 64 levels deep) to a value; null becomes nil",
        false,
        "local v = rsb.json.decode(r.body)",
    ),
    i(
        "json.encode",
        "(value: any) -> string",
        "A value to JSON text (tables that contain themselves or nest over 64 levels are refused)",
        false,
        "rsb.json.encode({ a = 1 })",
    ),
    i(
        "open_url",
        "(url: string) -> ()",
        "Opens an https URL in the default browser: only inside a click or key handler, at most one address every 2 seconds (needs net permission for its host)",
        false,
        "on_click = function() rsb.open_url(\"https://open-meteo.com\") end",
    ),
    // Events
    i(
        "events.on",
        "(name: string, fn: (data: any) -> ()) -> Subscription",
        "Runs fn (as a task, on the next turn) for `rsb trigger <name> [json]` and rsb.events.emit; data comes from outside the widget, check it",
        false,
        "rsb.events.on(\"deploy\", function(d) status.value = d.state end)",
    ),
    i(
        "events.emit",
        "(name: string, data: any?) -> ()",
        "Sends an event to every widget (at most 50 a second and 1 MiB of data a second per widget; one event carries at most 64 KiB)",
        false,
        "rsb.events.emit(\"refresh\")",
    ),
    // Theme, log, formatting
    i(
        "theme.dark",
        "boolean",
        "Windows' app theme is dark (reactive)",
        false,
        "rsb.text { text = function() return rsb.theme.dark and \"dark\" or \"light\" end }",
    ),
    i(
        "log.info",
        "(...any) -> ()",
        "Writes to the RSB log (print does the same); 20 lines a second, line breaks escaped. What you log is yours: keep titles and names out",
        false,
        "rsb.log.info(\"loaded\", n)",
    ),
    i("log.warn", "(...any) -> ()", "Writes a warning to the log", false, "rsb.log.warn(\"slow\")"),
    i("log.error", "(...any) -> ()", "Writes an error to the log", false, "rsb.log.error(err)"),
    i(
        "format.bytes",
        "(n: number) -> string",
        "1536 -> \"1.5 KB\" (1024-based)",
        false,
        "rsb.format.bytes(mem.memory_used)",
    ),
    i(
        "format.rate",
        "(bytes_per_sec: number) -> string",
        "Bytes per second -> \"1.2 MB/s\"",
        false,
        "rsb.format.rate(net.down_bps)",
    ),
    i("format.percent", "(n: number) -> string", "23.4 -> \"23%\"", false, "rsb.format.percent(sys.cpu)"),
];

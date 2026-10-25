//! Runtime configuration: the `Config` struct, its documented-default file
//! templates, and the key/value parser. No Win32 — pure data and string work.

/// User-supplied launcher entry. `icon` accepts an .ico/.png/.exe path or
/// `auto`; resolution belongs to the Win32 UI process, not this pure crate.
#[derive(Clone, PartialEq, Debug)]
pub struct LauncherEntry {
    pub label: String,
    pub target: String,
    pub icon: String,
}

/// User-supplied system-menu action. Actions are always user-triggered.
#[derive(Clone, PartialEq, Debug)]
pub struct SystemAction {
    pub category: String,
    pub label: String,
    pub target: String,
    pub icon: String,
    pub confirm: bool,
}

/// Rich window rule. Empty match fields are wildcards; every non-empty field
/// must match. `action` is tile, float, or ignore.
#[derive(Clone, PartialEq, Debug)]
pub struct WindowRule {
    pub action: String,
    pub exe: String,
    pub class: String,
    pub title: String,
    pub workspace: Option<usize>,
    pub monitor: Option<usize>,
}

/// Extra configurable key binding. Chords use names such as ALT+Q,
/// ALT+SHIFT+F2. `action` is resolved by the WM onto its command queue.
#[derive(Clone, PartialEq, Debug)]
pub struct HotkeyDef {
    pub chord: String,
    pub action: String,
    pub argument: String,
}

/// Runtime configuration, loaded from astur.conf + navbar.conf at startup.
#[derive(Clone, PartialEq)]
pub struct Config {
    pub per_monitor: bool,        // true: Alt+1..9 switches focused monitor only
    pub start_tiled: bool,        // tile automatically on launch
    pub outer_gap: i32,           // gap between windows and screen edge
    pub inner_gap: i32,           // gap between adjacent windows
    pub master_ratio: f32,        // fraction of width given to the master window
    pub workspaces: usize,        // shared: total; per_monitor: each (1..=MAX_WORKSPACES)
    pub workspace_keys: Vec<u32>, // VK code per workspace; Alt+key switches, +Shift moves
    pub workspace_names: Vec<String>, // optional display names, in workspace order
    pub workspace_icons: Vec<String>, // optional short labels/icons, in workspace order
    pub layout: String,           // default layout: one of LAYOUTS
    pub monitor_layouts: Vec<String>, // per monitor (1st entry = monitor 1); "" = `layout`
    pub scroll_default_width: f32, // scrolling: new column width, fraction of the view
    pub scroll_width_presets: Vec<f32>, // scrolling: column widths Alt+R cycles
    pub scroll_height_presets: Vec<f32>, // scrolling: tile heights Alt+Shift+R cycles
    pub scroll_center: String,    // scrolling viewport: never | always | on-overflow
    pub tab_bar_height: i32,      // tabbed-column tab bar, logical px
    pub terminal: String,         // command launched by Alt+Enter
    pub browser: String,          // Alt+Shift+Enter; empty = default browser
    pub unfocused_opacity: f32,   // 0.0-1.0 alpha for unfocused windows (1.0 = off)
    pub border_enabled: bool,     // draw coloured DWM borders (Windows 11)
    pub focused_border: u32,      // COLORREF for the focused window border
    pub unfocused_border: u32,    // COLORREF for unfocused window borders
    pub cursor_follows_focus: bool, // warp the mouse to the focused window
    pub focus_follows_mouse: bool, // hovering a window focuses it (focus follows mouse)
    pub animations: bool,         // animate tiling moves + workspace slides
    pub animation_ms: i32,        // animation duration in ms (0 disables; clamp 0..2000)
    pub workspace_slide: bool,    // back-compat: false forces workspace_anim = off
    pub workspace_anim: String,   // workspace-switch style: off | slide | spring | fade
    pub window_anim: String,      // window move/open/close/resize: off | glide | spring
    pub animation_easing: String, // cubic | smooth | spring
    pub popup_font_name: String,
    pub popup_font_size: i32,
    pub popup_font_weight: i32,
    pub popup_radius: i32,
    pub popup_border_width: i32,
    pub popup_opacity: i32,
    pub popup_bg: Option<u32>,
    pub popup_fg: Option<u32>,
    pub popup_muted: Option<u32>,
    pub popup_accent: Option<u32>,
    pub popup_accent_fg: Option<u32>,
    pub popup_border: Option<u32>,
    pub launcher_enabled: bool,
    pub launcher_width: i32,
    pub launcher_wide_width: i32,
    pub launcher_height: i32,
    pub launcher_row_height: i32,
    pub launcher_icon_size: i32,
    pub launcher_padding: i32,
    pub launcher_selection_radius: i32,
    pub launcher_placement: String,
    pub launcher_source_apps: bool,
    pub launcher_source_files: bool,
    pub launcher_source_calc: bool,
    pub launcher_source_web: bool,
    pub launcher_source_windows: bool,
    pub launcher_source_clipboard: bool,
    pub launcher_source_emoji: bool,
    pub launcher_web_url: String,
    pub launcher_max_results: usize,
    pub launcher_file_scope: String,
    pub launcher_file_exclude: Vec<String>,
    pub launcher_mru: bool,
    pub launcher_entries: Vec<LauncherEntry>,
    pub system_menu_enabled: bool,
    pub system_menu_width: i32,
    pub system_power_items: Vec<String>,
    pub system_setup_items: Vec<String>,
    pub system_actions: Vec<SystemAction>,
    pub alt_tab_replacement: bool,
    pub scratchpad_enabled: bool,
    pub scratchpad_command: String,
    pub scratchpad_class: String,
    pub clipboard_history: bool,
    pub clipboard_limit: usize,
    pub clipboard_prefix: String,
    pub emoji_picker: bool,
    pub emoji_prefix: String,
    pub wallpaper_dir: String,
    pub workspace_wallpapers: Vec<String>,
    pub media_enabled: bool,
    pub ipc_enabled: bool,
    pub ipc_pipe: String,
    /// GlazeWM-compatible workspace API for YASB, bound to IPv4 loopback only.
    pub yasb_enabled: bool,
    pub yasb_port: u16,
    /// Allow the IPC `launch` verb to run arbitrary commands. Off by default:
    /// scripting a window manager rarely needs arbitrary exec, and leaving it on
    /// makes Astur a convenient parent process for something else already
    /// running as the user.
    pub ipc_allow_launch: bool,
    /// Disable Windows' foreground-lock timeout so focus changes are reliable.
    /// This is a GLOBAL, system-wide setting; Astur restores the previous value
    /// on a graceful exit.
    pub foreground_lock_disable: bool,
    /// Place app windows with posted (SWP_ASYNCWINDOWPOS) SetWindowPos calls,
    /// so a retile waits for the slowest app rather than for every app in
    /// turn. false = wait for each app, as before this key existed: the escape
    /// hatch should some app misplace under posted placement.
    pub async_window_pos: bool,
    pub persist_state: bool,
    /// Diagnostics verbosity: off | error | info | debug. Astur ships without a
    /// console, so this file log is the only way a user can report what broke.
    pub log_level: String,
    /// `file: key` for every line the parser did not understand. Ignoring
    /// unknown keys keeps old and new configs loadable, but silently ignoring
    /// them means a typo (`bar_heigth = 40`) does nothing and says nothing —
    /// with 140+ keys across two files that is the likeliest user error there
    /// is. Collected here so the WM can log it and the GUI can show it.
    /// Not written back to disk, and not part of "has the user edited this".
    pub unknown_keys: Vec<String>,
    pub extra_hotkeys: Vec<HotkeyDef>,
    /// The key every hotkey is held with (`main_key`, alias `mod`).
    pub main_key: MainKey,
    pub window_rules: Vec<WindowRule>,
    pub bar_enabled: bool,         // draw the status bar on every monitor
    pub bar_height: i32,           // bar thickness in px (work area is reserved for it)
    pub bar_bottom: bool,          // dock the bar at the bottom instead of the top
    pub bar_font_size: i32,        // text height in px; 0 = auto from bar_height
    pub bar_show_title: bool,      // show the focused window title
    pub bar_show_clock: bool,      // show the clock
    pub bar_clock_24h: bool,       // 24-hour clock (false = 12-hour with am/pm)
    pub bar_show_layout: bool,     // show layout + tiling/floating state on the right
    pub bar_bg: Option<u32>,       // COLORREF bar background; None = follow theme
    pub bar_fg: Option<u32>,       // COLORREF bar text; None = follow theme
    pub bar_accent: Option<u32>,   // COLORREF active-workspace highlight; None = theme
    pub bar_inactive: Option<u32>, // COLORREF empty-workspace text; None = theme
    pub bar_font_name: String,     // font family (default "Segoe UI")
    pub bar_hide_empty: bool,      // hide empty workspace pills
    pub bar_widget_gap: i32,
    pub bar_icon_size: i32,
    pub bar_workspace_width: i32,
    pub bar_icon_mode: String,
    pub bar_show_tooltips: bool,
    pub bar_show_app_labels: bool,
    pub bar_cpu_format: String,
    pub bar_mem_format: String,
    pub bar_battery_format: String,
    pub bar_net_format: String,
    pub bar_volume_format: String,
    pub bar_clock_format: String,
    pub bar_icon_cpu: String,
    pub bar_icon_mem: String,
    pub bar_icon_battery: String,
    pub bar_icon_net: String,
    pub bar_icon_volume: String,
    pub bar_padding: i32,        // horizontal padding from each screen edge (px)
    pub bar_show_date: bool,     // show the date widget
    pub bar_date_format: String, // date token string, e.g. "ddd dd MMM"
    pub bar_show_cpu: bool,      // show CPU load %
    pub bar_show_mem: bool,      // show RAM load %
    pub bar_show_battery: bool,  // show battery %
    pub ignore_classes: Vec<String>, // window classes never tiled/managed
    pub float_classes: Vec<String>, // window classes managed but auto-floated
    pub key_focus_next: u32,     // Alt+<key> focus next window in the stack (default J)
    pub key_focus_prev: u32,     // Alt+<key> focus previous window in the stack (default K)
    pub key_shrink_master: u32,  // Alt+<key> shrink the master area (default H)
    pub key_grow_master: u32,    // Alt+<key> grow the master area (default L)
    pub key_promote_master: u32, // Alt+<key> promote focused window to master (default M)
    pub key_toggle_tiling: u32,  // Alt+<key> toggle tiling on/off (default T)
    pub key_toggle_float: u32,   // Alt+<key> toggle floating for focused window (default F)
    pub key_close_window: u32,   // Alt+<key> close the focused window (default W)
    pub key_cycle_width: u32,    // Alt+<key> scrolling: cycle width, +Shift height (default R)
    pub key_center_column: u32,  // Alt+<key> scrolling: centre focused column (default C)
    pub key_consume_left: u32,   // Alt+<key> scrolling: consume/expel left (default [)
    pub key_consume_right: u32,  // Alt+<key> scrolling: consume/expel right (default ])
    pub theme: String,           // popup palette: dark | light | auto (follows Windows)
    pub acrylic: bool,           // experimental acrylic blur behind the popups
    pub bar_floating: bool,      // detached rounded bar (margins from the screen edges)
    pub bar_margin: i32,         // gap between a floating bar and the screen edges (px)
    pub bar_radius: i32,         // floating-bar corner radius (px)
    pub bar_autohide: bool,      // slide the bar away; reveal on screen-edge hover
    pub bar_wheel_ws: bool,      // mouse wheel over the bar cycles workspaces
    pub bar_show_net: bool,      // show network up/down speed
    pub bar_show_volume: bool,   // show volume % (wheel adjusts, click mutes)
    pub bar_show_apps: bool,     // show app buttons for the active workspace's windows
    pub bar_show_media: bool,
    pub bar_left: Vec<String>, // widget names, left zone (drawn left-to-right)
    pub bar_center: Vec<String>, // widget names, centered in the remaining gap
    pub bar_right: Vec<String>, // widget names, right zone (listed left-to-right)
    pub bar_palette: String,   // PALETTES name the `auto` colours resolve through
    pub bar_workspace_style: String, // active workspace: box | pill | underline | dot
    pub bar_font_weight: i32,  // bar (and tab-bar) font weight, 100-900
    pub bar_surface: Option<u32>, // island / pill fill; None = palette
    pub bar_accent_fg: Option<u32>, // text on the accent; None = see `bar_roles`
    pub bar_border: Option<u32>, // outline colour; None = the muted role
    pub bar_border_width: i32, // outline width (px), 0 = none
    /// strip | floating | islands | pills, or "" when no `shape` line set it
    /// (then `floating` decides): read it through `effective_bar_shape`.
    pub bar_shape: String,
    pub bar_island_padding: i32, // space around an island's / pill's content (px)
    /// `widget_colors`: (text widget, swatch name | surface | #hex), the
    /// capsule fills of `shape = pills`; read through `bar_widget_fill`.
    pub bar_widget_colors: Vec<(String, String)>,
    /// `<widget>_click` / `_right_click` / `_middle_click` lines as (widget,
    /// button 0 left / 1 right / 2 middle, "action [argument]"): explicit
    /// lines only, one per widget and button, the last line wins. Read
    /// through `bar_click`, which falls back to `BAR_CLICK_DEFAULTS`.
    pub bar_clicks: Vec<(String, u8, String)>,
    pub bar_keyboard_format: String, // keyboard widget: {lang} {region}
    pub bar_disk_drive: String,      // disk widget: a drive or folder, ending in '\'
    pub bar_disk_format: String,     // {free} {used} {total} {value} (% used)
    pub bar_icon_disk: String,       // disk label for icon / both
    pub bar_uptime_format: String,   // {value}
    pub bar_custom_command: String,  // custom widget: cmd /S /C "<this>"; empty = never runs
    pub bar_custom_format: String,   // {output}
    pub bar_custom_interval: u32,    // seconds between custom runs, 1..=3600
    pub bar_workspace_label: String, // WORKSPACE_LABELS: what a mark shows
    pub bar_workspace_apps: bool,    // up to 3 app icons in each workspace mark
    /// `monitors`: all | primary | screen numbers ("1 3", 1-based, left to
    /// right); read through `bar_monitor_mask` and `bar_on`.
    pub bar_monitors: String,
    pub bar_separator_style: String, // line | dot | powerline
    pub bar_hover: bool,             // highlight the clickable item under the pointer
    pub bar_title_format: String,    // title widget: {title} {app}
    pub bar_title_bold: bool,        // title widget in a bold weight
}

/// Widget names accepted in the navbar `left` / `center` / `right` zone lists.
pub const BAR_WIDGETS: &[&str] = &[
    "workspaces",
    "apps",
    "title",
    "layout",
    "cpu",
    "mem",
    "net",
    "volume",
    "battery",
    "date",
    "clock",
    "media",
    "separator",
    "spacer",
    "keyboard",
    "disk",
    "uptime",
    "custom",
    "media_controls",
    "logo",
];

/// Widgets with click actions (`<widget>_click` keys). Workspaces, app
/// buttons and the music buttons keep their own fixed clicks; separators
/// and spacers have none.
pub const BAR_CLICK_WIDGETS: &[&str] = &[
    "title", "layout", "cpu", "mem", "net", "volume", "battery", "date", "clock", "media",
    "keyboard", "disk", "uptime", "custom", "logo",
];

/// Every bar click action: (value, plain-English label for Settings). The
/// last two take an argument after a space: `switch_workspace 3`,
/// `launch wt -p Ubuntu`. Hotkeys and bar clicks share the WM's action
/// runner, so a name means the same in both.
pub const BAR_ACTIONS: &[(&str, &str)] = &[
    ("none", "Do nothing"),
    ("calendar", "Open the calendar"),
    (
        "quick_settings",
        "Open quick settings (Wi-Fi, sound, battery)",
    ),
    ("mute", "Mute or unmute"),
    ("volume_mixer", "Open the volume mixer"),
    ("sound_settings", "Open sound settings"),
    ("network_settings", "Open network settings"),
    ("bluetooth_settings", "Open Bluetooth settings"),
    ("power_settings", "Open power and battery settings"),
    ("datetime_settings", "Open date and time settings"),
    ("language_settings", "Open language and keyboard settings"),
    ("next_input", "Switch keyboard language"),
    ("storage_settings", "Open storage settings"),
    ("display_settings", "Open display settings"),
    ("windows_settings", "Open Windows Settings"),
    ("astur_settings", "Open Astur Settings"),
    ("task_manager", "Open Task Manager"),
    ("open_drive", "Open the drive in File Explorer"),
    ("media_play_pause", "Play or pause"),
    ("media_next", "Next track"),
    ("media_prev", "Previous track"),
    ("launcher", "Open the app launcher"),
    ("system_menu", "Open the power menu"),
    ("workspace_next", "Next workspace"),
    ("workspace_prev", "Previous workspace"),
    ("next_layout", "Next layout"),
    ("switch_workspace", "Go to workspace\u{2026}"),
    ("launch", "Open an app or command\u{2026}"),
];

/// What a click does with no `<widget>_click` line: [left, right, middle].
/// Every action was a no-op before click actions existed except the volume
/// widget's left click (mute), which keeps it; a widget not listed does
/// nothing on any button.
pub const BAR_CLICK_DEFAULTS: &[(&str, [&str; 3])] = &[
    ("clock", ["calendar", "datetime_settings", "none"]),
    ("date", ["calendar", "datetime_settings", "none"]),
    ("volume", ["mute", "sound_settings", "none"]),
    ("battery", ["quick_settings", "power_settings", "none"]),
    ("net", ["quick_settings", "network_settings", "none"]),
    ("cpu", ["task_manager", "none", "none"]),
    ("mem", ["task_manager", "none", "none"]),
    ("disk", ["open_drive", "storage_settings", "none"]),
    ("keyboard", ["next_input", "language_settings", "none"]),
    ("media", ["media_play_pause", "media_next", "none"]),
    ("logo", ["system_menu", "astur_settings", "none"]),
];

/// `workspace_label` values: auto (icon, else name, else number: the bar
/// before this key), number, name, icon, icon_name.
pub const WORKSPACE_LABELS: &[&str] = &["auto", "number", "name", "icon", "icon_name"];

/// `bar_monitor_mask` bit meaning "the primary screen only".
pub const BAR_PRIMARY_ONLY: u64 = 1 << 63;

/// The text of one workspace mark (`workspace_label`): `number` is 1-based,
/// `name` / `icon` that workspace's workspace_names / workspace_icons entry
/// ("" = none). A missing name or icon falls back to the number, so a mark
/// is never blank. Shared by the WM and the settings preview.
pub fn workspace_label(mode: &str, number: usize, name: &str, icon: &str) -> String {
    let or_number = |s: &str| {
        if s.is_empty() {
            number.to_string()
        } else {
            s.to_string()
        }
    };
    match mode {
        "number" => number.to_string(),
        "name" => or_number(name),
        "icon" => or_number(icon),
        "icon_name" if !icon.is_empty() && !name.is_empty() => format!("{icon} {name}"),
        // auto, and icon_name with only one of the two.
        _ => or_number(if icon.is_empty() { name } else { icon }),
    }
}

/// Does bar monitor `index` (0-based, left to right) with `primary` get a
/// bar under `mask` (`Config::bar_monitor_mask`)? The WM's bar placement and
/// its work-area reservation both ask this one function: if they disagreed,
/// every tile on that screen would sit offset by a bar's height.
pub fn bar_on(mask: u64, index: usize, primary: bool) -> bool {
    if mask & BAR_PRIMARY_ONLY != 0 {
        primary
    } else {
        index < 63 && (mask >> index) & 1 == 1
    }
}

/// The clock widget's text. Tokens: HH / H (24-hour), hh / h (12-hour), mm,
/// ss, tt (am/pm), TT (AM/PM); an empty format is `HH:mm`, or `h:mm tt`
/// without `h24`. The replaces run longest-first, as the WM always did, so
/// every format without the newer ss / TT reads exactly as before. Here, not
/// in the WM, so Settings' samples cannot drift from the bar.
pub fn format_clock(fmt: &str, h24: bool, h: u32, m: u32, s: u32) -> String {
    let fmt = match fmt {
        "" if h24 => "HH:mm",
        "" => "h:mm tt",
        f => f,
    };
    let h12 = match h % 12 {
        0 => 12,
        x => x,
    };
    let pm = h >= 12;
    fmt.replace("HH", &format!("{h:02}"))
        .replace("hh", &format!("{h12:02}"))
        .replace("mm", &format!("{m:02}"))
        .replace("ss", &format!("{s:02}"))
        .replace("tt", if pm { "pm" } else { "am" })
        .replace("TT", if pm { "PM" } else { "AM" })
        .replace('H', &h.to_string())
        .replace('h', &h12.to_string())
}

/// The date widget's text. Tokens: yyyy yy, MMMM (January) MMM (Jan) MM,
/// dddd (Sunday) ddd (Sun) dd, and d (day, no leading zero) only where no
/// letter touches it, so a literal word such as "Today" keeps its d.
/// `mon` is 1..=12, `wday` 0 = Sunday; out of range gives no name. Anything
/// else is copied. Char-based so non-ASCII text never splits.
pub fn format_date(fmt: &str, y: u32, mon: u32, day: u32, wday: u32) -> String {
    const WD: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    const MO: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let chars: Vec<char> = fmt.chars().collect();
    let at = |i: usize, tok: &str| {
        tok.chars()
            .enumerate()
            .all(|(k, t)| chars.get(i + k) == Some(&t))
    };
    let letter = |i: Option<usize>| {
        i.and_then(|i| chars.get(i))
            .is_some_and(|c| c.is_alphabetic())
    };
    let wd = WD.get(wday as usize).copied().unwrap_or("");
    let mo = (mon as usize)
        .checked_sub(1)
        .and_then(|i| MO.get(i))
        .copied()
        .unwrap_or("");
    let short = |s: &str| s.chars().take(3).collect::<String>();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let (text, len) = if at(i, "yyyy") {
            (format!("{y:04}"), 4)
        } else if at(i, "yy") {
            (format!("{:02}", y % 100), 2)
        } else if at(i, "MMMM") {
            (mo.to_string(), 4)
        } else if at(i, "MMM") {
            (short(mo), 3)
        } else if at(i, "MM") {
            (format!("{mon:02}"), 2)
        } else if at(i, "dddd") {
            (wd.to_string(), 4)
        } else if at(i, "ddd") {
            (short(wd), 3)
        } else if at(i, "dd") {
            (format!("{day:02}"), 2)
        } else if chars[i] == 'd' && !letter(i.checked_sub(1)) && !letter(Some(i + 1)) {
            (day.to_string(), 1)
        } else {
            (chars[i].to_string(), 1)
        };
        out.push_str(&text);
        i += len;
    }
    out
}

/// A `disk_drive` value as the root the WM asks about: `C`, `C:` and `C:\`
/// are the drive, any other text a folder; always ending in '\'. None for an
/// empty value (the previous one stays).
pub fn disk_root(v: &str) -> Option<String> {
    let v = v.trim().trim_matches('"');
    let mut s = match v.as_bytes() {
        [] => return None,
        [d] if d.is_ascii_alphabetic() => format!("{}:", d.to_ascii_uppercase() as char),
        [d, b':', ..] if d.is_ascii_alphabetic() => {
            format!("{}{}", d.to_ascii_uppercase() as char, &v[1..])
        }
        _ => v.to_string(),
    };
    if !s.ends_with(['\\', '/']) {
        s.push('\\');
    }
    Some(s)
}

/// A `monitors` value normalised (all, primary, or sorted 1-based screen
/// numbers "1 3"), or None when it is none of those (the previous stays).
fn parse_monitors(v: &str) -> Option<String> {
    let m = v.trim().to_ascii_lowercase();
    match m.as_str() {
        "" | "all" => return Some("all".into()),
        "primary" | "main" => return Some("primary".into()),
        _ => {}
    }
    let mut n: Vec<u32> = m
        .split([',', ' ', '\t'])
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<u32>().ok().filter(|n| (1..=63).contains(n)))
        .collect::<Option<_>>()?;
    n.sort_unstable();
    n.dedup();
    let words: Vec<String> = n.iter().map(u32::to_string).collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// A `<widget>_click` key as (widget, button): `_middle_click` and
/// `_right_click` are stripped before `_click`, so `cpu_right_click` is
/// the right button, not a widget named `cpu_right`.
fn click_key(k: &str) -> Option<(&str, u8)> {
    let (w, b) = if let Some(w) = k.strip_suffix("_middle_click") {
        (w, 2)
    } else if let Some(w) = k.strip_suffix("_right_click") {
        (w, 1)
    } else {
        (k.strip_suffix("_click")?, 0)
    };
    BAR_CLICK_WIDGETS.contains(&w).then_some((w, b))
}

/// A click value `action [argument]`, split at the first whitespace, the
/// action lower-cased and the argument kept verbatim. None for an unknown
/// action; Some("") for an empty value (back to the default).
fn click_value(v: &str) -> Option<String> {
    let v = v.trim();
    let (a, arg) = v
        .split_once(char::is_whitespace)
        .map_or((v, ""), |(a, r)| (a, r.trim()));
    let a = a.to_ascii_lowercase();
    if a.is_empty() {
        return Some(String::new());
    }
    BAR_ACTIONS.iter().any(|x| x.0 == a).then(|| {
        if arg.is_empty() {
            a
        } else {
            format!("{a} {arg}")
        }
    })
}

/// The one list of valid layout names. The parser, IPC, extra hotkeys and the
/// WM's layout commands all check against it, so a new layout cannot be
/// accepted in one place and rejected in another.
pub const LAYOUTS: &[&str] = &[
    "dwindle",
    "master",
    "columns",
    "grid",
    "monocle",
    "scrolling",
];

/// Most workspaces `workspaces =` accepts (#18). The settings slider uses it
/// too, so the GUI can reach every value the parser accepts (a value dragged
/// or typed there is clamped to the slider range). 20, not the 64 the bar's
/// per-monitor pill mask allows: every workspace left on a monitor keeps a work-area-sized
/// slide snapshot (~32 MB of GPU/system-shared memory at 4K, an estimate),
/// and the pill strip is `workspaces` x `workspace_width` wide.
pub const MAX_WORKSPACES: usize = 20;

/// Built-in scrolling preset cycles. Shared by `defaults()` and the parser,
/// which falls back to them when a list has nothing valid in it.
const SCROLL_WIDTHS: [f32; 4] = [0.333, 0.5, 0.667, 1.0];
const SCROLL_HEIGHTS: [f32; 3] = [0.333, 0.5, 0.667];

/// Built-in bar palettes that `auto` colours resolve to: [bg, fg, accent,
/// inactive] as COLORREFs. Single source for the WM and the settings GUI.
pub const BAR_DARK: [u32; 4] = [
    0x0026_1B1A, // #1A1B26
    0x00F5_CAC0, // #C0CAF5
    0x00FF_AA66, // #66AAFF
    0x0089_5F56, // #565F89
];
pub const BAR_LIGHT: [u32; 4] = [
    0x00F2_EEEC, // #ECEEF2
    0x0024_1E1B, // #1B1E24
    0x0082_6333, // #336382
    0x0091_8277, // #778291
];

/// `#RRGGBB` written as 0xRRGGBB, as a COLORREF (0x00BBGGRR), so the palette
/// tables below read like the hex their sources publish.
const fn rgb(hex: u32) -> u32 {
    ((hex & 0xFF) << 16) | (hex & 0xFF00) | ((hex >> 16) & 0xFF)
}

/// One palette variant's colour roles (COLORREFs). `muted` is the `inactive`
/// key; `accent_fg` is text on the accent fill; `surface` fills islands and
/// pills. `swatches` are named by `SWATCH_NAMES`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BarRoles {
    pub bg: u32,
    pub surface: u32,
    pub fg: u32,
    pub muted: u32,
    pub accent: u32,
    pub accent_fg: u32,
    pub swatches: [u32; 8],
}

/// A named bar palette: what `auto` colours resolve to, per theme.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Palette {
    pub name: &'static str,
    pub dark: BarRoles,
    pub light: BarRoles,
}

/// Names of `BarRoles::swatches`, in order. A palette without one of these
/// hues repeats its nearest (Nord and Gruvbox have no pink, Rose Pine no green).
pub const SWATCH_NAMES: [&str; 8] = [
    "red", "orange", "yellow", "green", "teal", "blue", "purple", "pink",
];

/// Every `palette =` value, `classic` (the default) first. Hex values were
/// checked on 2026-10-24 against catppuccin/palette palette.json (Mocha /
/// Latte), nordtheme.com, morhetz/gruvbox colors/gruvbox.vim and
/// rose-pine/palette palette.json (main / dawn). classic is BAR_DARK /
/// BAR_LIGHT plus Astur's own surface and swatches; mono is Astur's own;
/// system's neutrals approximate Windows' and its accent is replaced by the
/// Windows accent colour in the WM (never read here: no Win32).
pub const PALETTES: &[Palette] = &[
    Palette {
        name: "classic",
        dark: BarRoles {
            bg: BAR_DARK[0],
            surface: rgb(0x292E42),
            fg: BAR_DARK[1],
            muted: BAR_DARK[3],
            accent: BAR_DARK[2],
            accent_fg: BAR_DARK[0],
            swatches: CLASSIC_DARK_SWATCHES,
        },
        light: BarRoles {
            bg: BAR_LIGHT[0],
            surface: rgb(0xDDE1E8),
            fg: BAR_LIGHT[1],
            muted: BAR_LIGHT[3],
            accent: BAR_LIGHT[2],
            accent_fg: BAR_LIGHT[0],
            swatches: CLASSIC_LIGHT_SWATCHES,
        },
    },
    Palette {
        name: "catppuccin",
        // Mocha: base, surface0, text, overlay0, mauve, crust.
        dark: BarRoles {
            bg: rgb(0x1E1E2E),
            surface: rgb(0x313244),
            fg: rgb(0xCDD6F4),
            muted: rgb(0x6C7086),
            accent: rgb(0xCBA6F7),
            accent_fg: rgb(0x11111B),
            swatches: [
                rgb(0xF38BA8),
                rgb(0xFAB387),
                rgb(0xF9E2AF),
                rgb(0xA6E3A1),
                rgb(0x94E2D5),
                rgb(0x89B4FA),
                rgb(0xCBA6F7),
                rgb(0xF5C2E7),
            ],
        },
        // Latte: base, surface0, text, overlay0, mauve, base.
        light: BarRoles {
            bg: rgb(0xEFF1F5),
            surface: rgb(0xCCD0DA),
            fg: rgb(0x4C4F69),
            muted: rgb(0x9CA0B0),
            accent: rgb(0x8839EF),
            accent_fg: rgb(0xEFF1F5),
            swatches: [
                rgb(0xD20F39),
                rgb(0xFE640B),
                rgb(0xDF8E1D),
                rgb(0x40A02B),
                rgb(0x179299),
                rgb(0x1E66F5),
                rgb(0x8839EF),
                rgb(0xEA76CB),
            ],
        },
    },
    Palette {
        name: "nord",
        // nord0, nord1, nord4, nord3, nord8, nord0.
        dark: BarRoles {
            bg: rgb(0x2E3440),
            surface: rgb(0x3B4252),
            fg: rgb(0xD8DEE9),
            muted: rgb(0x4C566A),
            accent: rgb(0x88C0D0),
            accent_fg: rgb(0x2E3440),
            swatches: NORD_AURORA,
        },
        // nord6, nord5, nord0, nord3, nord10, nord6.
        light: BarRoles {
            bg: rgb(0xECEFF4),
            surface: rgb(0xE5E9F0),
            fg: rgb(0x2E3440),
            muted: rgb(0x4C566A),
            accent: rgb(0x5E81AC),
            accent_fg: rgb(0xECEFF4),
            swatches: NORD_AURORA,
        },
    },
    Palette {
        name: "gruvbox",
        // dark0, dark1, light1, gray, bright_yellow, dark0; bright swatches.
        dark: BarRoles {
            bg: rgb(0x282828),
            surface: rgb(0x3C3836),
            fg: rgb(0xEBDBB2),
            muted: rgb(0x928374),
            accent: rgb(0xFABD2F),
            accent_fg: rgb(0x282828),
            swatches: [
                rgb(0xFB4934),
                rgb(0xFE8019),
                rgb(0xFABD2F),
                rgb(0xB8BB26),
                rgb(0x8EC07C),
                rgb(0x83A598),
                rgb(0xD3869B),
                rgb(0xD3869B),
            ],
        },
        // light0, light1, dark1, gray, faded_yellow, light0; faded swatches.
        light: BarRoles {
            bg: rgb(0xFBF1C7),
            surface: rgb(0xEBDBB2),
            fg: rgb(0x3C3836),
            muted: rgb(0x928374),
            accent: rgb(0xB57614),
            accent_fg: rgb(0xFBF1C7),
            swatches: [
                rgb(0x9D0006),
                rgb(0xAF3A03),
                rgb(0xB57614),
                rgb(0x79740E),
                rgb(0x427B58),
                rgb(0x076678),
                rgb(0x8F3F71),
                rgb(0x8F3F71),
            ],
        },
    },
    Palette {
        name: "rosepine",
        // main: base, overlay (surface is too close to base to read as an
        // island), text, muted, iris, base. love gold gold pine foam pine iris rose.
        dark: BarRoles {
            bg: rgb(0x191724),
            surface: rgb(0x26233A),
            fg: rgb(0xE0DEF4),
            muted: rgb(0x6E6A86),
            accent: rgb(0xC4A7E7),
            accent_fg: rgb(0x191724),
            swatches: [
                rgb(0xEB6F92),
                rgb(0xF6C177),
                rgb(0xF6C177),
                rgb(0x31748F),
                rgb(0x9CCFD8),
                rgb(0x31748F),
                rgb(0xC4A7E7),
                rgb(0xEBBCBA),
            ],
        },
        // dawn, the same roles.
        light: BarRoles {
            bg: rgb(0xFAF4ED),
            surface: rgb(0xF2E9E1),
            fg: rgb(0x464261),
            muted: rgb(0x9893A5),
            accent: rgb(0x907AA9),
            accent_fg: rgb(0xFAF4ED),
            swatches: [
                rgb(0xB4637A),
                rgb(0xEA9D34),
                rgb(0xEA9D34),
                rgb(0x286983),
                rgb(0x56949F),
                rgb(0x286983),
                rgb(0x907AA9),
                rgb(0xD7827E),
            ],
        },
    },
    Palette {
        name: "mono",
        dark: BarRoles {
            bg: rgb(0x111111),
            surface: rgb(0x1E1E1E),
            fg: rgb(0xE6E6E6),
            muted: rgb(0x6B6B6B),
            accent: rgb(0xE6E6E6),
            accent_fg: rgb(0x111111),
            swatches: MONO_DARK,
        },
        light: BarRoles {
            bg: rgb(0xF5F5F5),
            surface: rgb(0xE6E6E6),
            fg: rgb(0x1A1A1A),
            muted: rgb(0x8A8A8A),
            accent: rgb(0x1A1A1A),
            accent_fg: rgb(0xF5F5F5),
            swatches: MONO_LIGHT,
        },
    },
    Palette {
        name: "system",
        // Accent: Windows' default blue until the WM reads the real one.
        dark: BarRoles {
            bg: rgb(0x202020),
            surface: rgb(0x2B2B2B),
            fg: rgb(0xFFFFFF),
            muted: rgb(0x9D9D9D),
            accent: rgb(0x0078D4),
            accent_fg: rgb(0xFFFFFF),
            swatches: CLASSIC_DARK_SWATCHES,
        },
        light: BarRoles {
            bg: rgb(0xF3F3F3),
            surface: rgb(0xFBFBFB),
            fg: rgb(0x1B1B1B),
            muted: rgb(0x616161),
            accent: rgb(0x005FB8),
            accent_fg: rgb(0xFFFFFF),
            swatches: CLASSIC_LIGHT_SWATCHES,
        },
    },
];

/// Nord's Aurora plus Frost, in SWATCH_NAMES order (no pink: purple again).
const NORD_AURORA: [u32; 8] = [
    rgb(0xBF616A),
    rgb(0xD08770),
    rgb(0xEBCB8B),
    rgb(0xA3BE8C),
    rgb(0x8FBCBB),
    rgb(0x81A1C1),
    rgb(0xB48EAD),
    rgb(0xB48EAD),
];
/// Mono's grey ramps, away from the bar background first, so the first
/// pills stand out most. Slot 0 is a real red even here: it is the low-battery
/// alert, and a near-white "red" vanished on the light bar (1.05:1) and
/// matched fg on the dark one. The reds are classic's.
const MONO_DARK: [u32; 8] = [
    rgb(0xF7768E),
    rgb(0xD6D6D6),
    rgb(0xBDBDBD),
    rgb(0xA3A3A3),
    rgb(0x8A8A8A),
    rgb(0x707070),
    rgb(0x575757),
    rgb(0x3D3D3D),
];
const MONO_LIGHT: [u32; 8] = [
    rgb(0xB8364C),
    rgb(0x2A2A2A),
    rgb(0x3D3D3D),
    rgb(0x575757),
    rgb(0x707070),
    rgb(0x8A8A8A),
    rgb(0xA3A3A3),
    rgb(0xBDBDBD),
];
/// Astur's own, for classic and system: Tokyo-Night-like hues on dark,
/// darkened for contrast on light.
const CLASSIC_DARK_SWATCHES: [u32; 8] = [
    rgb(0xF7768E),
    rgb(0xFF9E64),
    rgb(0xE0AF68),
    rgb(0x9ECE6A),
    rgb(0x73DACA),
    rgb(0x7AA2F7),
    rgb(0xBB9AF7),
    rgb(0xF5A6D8),
];
const CLASSIC_LIGHT_SWATCHES: [u32; 8] = [
    rgb(0xB8364C),
    rgb(0xB15C1E),
    rgb(0x8C6C20),
    rgb(0x4E7A2A),
    rgb(0x237A6E),
    rgb(0x2F5FB3),
    rgb(0x7447B3),
    rgb(0xA8457F),
];

/// The palette `name` names; classic for anything else.
pub fn palette(name: &str) -> &'static Palette {
    PALETTES
        .iter()
        .find(|p| p.name == name)
        .unwrap_or(&PALETTES[0])
}

/// Readable text on the COLORREF `fill`: black when its luma (Rec. 601
/// weights) is over 128, else white. One rule shared by the WM (text on the
/// Windows accent, text on a coloured pill) and the settings preview, so the
/// preview cannot pick a different colour than the bar. The cut was 150 when
/// only `palette = system` used it; that put white on Latte orange (2.98:1)
/// and pure green (1.4:1). At 128 every palette swatch reads at 3.3:1 or more
/// (the PALETTES test checks it) and Windows' default blue keeps white text.
pub fn text_on(fill: u32) -> u32 {
    let (r, g, b) = (fill & 0xFF, (fill >> 8) & 0xFF, (fill >> 16) & 0xFF);
    if (r * 299 + g * 587 + b * 114) / 1000 > 128 {
        0
    } else {
        0x00FF_FFFF
    }
}

/// A bar look the settings GUI applies by writing `pairs` into navbar.conf
/// (`apply_updates`), so the file stays the one source of truth: there is no
/// `preset` key and no base layer in the parser (a template that spells out
/// every key would hide one anyway). A preset owns the look and the zones:
/// every preset writes the same look keys, so applying one after another
/// leaves nothing behind, plus `show_* = true` for each widget it lists.
/// It never writes behaviour (enabled, bottom, autohide, wheel_workspaces,
/// hide_empty_workspaces), and lists no `media` (that needs astur.conf's
/// media_enabled too).
#[derive(Clone, Copy, Debug)]
pub struct BarPreset {
    pub name: &'static str,
    pub label: &'static str,
    pub blurb: &'static str,
    pub pairs: &'static [(&'static str, &'static str)],
}

/// A preset's pairs with every colour key back on `auto`: a preset's colours
/// come from its palette, never a pinned #hex (and never a legacy default
/// hex, which the parser reads as auto anyway).
macro_rules! looks {
    ($($k:literal: $v:literal),* $(,)?) => {
        &[$(($k, $v),)* ("bg", "auto"), ("fg", "auto"), ("accent", "auto"),
          ("inactive", "auto"), ("accent_fg", "auto"), ("surface", "auto"), ("border", "auto")]
    };
}

/// Every bar preset, `classic` (today's default bar, minus the widgets it
/// hides) first.
pub const BAR_PRESETS: &[BarPreset] = &[
    BarPreset {
        name: "classic",
        label: "Classic",
        blurb: "The default: a full-width strip, square accent block, title centred, stats on the right.",
        pairs: looks! {
            "shape": "strip", "palette": "classic", "height": "28", "padding": "8", "margin": "8",
            "radius": "12", "island_padding": "10", "border_width": "0",
            "font_name": "Segoe UI", "font_size": "0", "font_weight": "600",
            "workspace_style": "box", "workspace_width": "34", "widget_gap": "16", "icon_size": "20",
            "icon_mode": "both", "widget_colors": "", "clock_format": "HH:mm", "date_format": "ddd dd MMM",
            "separator_style": "line", "hover": "false", "title_bold": "false", "title_format": "{title}",
            "left": "workspaces", "center": "title", "right": "layout cpu mem volume battery clock",
            "show_title": "true", "show_layout": "true", "show_cpu": "true", "show_mem": "true",
            "show_volume": "true", "show_battery": "true", "show_clock": "true",
        },
    },
    BarPreset {
        name: "retro",
        label: "Retro",
        blurb: "i3 / dwmblocks: a dense monospace strip, square boxes, stats split by separators, Gruvbox.",
        pairs: looks! {
            "shape": "strip", "palette": "gruvbox", "height": "24", "padding": "4", "margin": "8",
            "radius": "12", "island_padding": "10", "border_width": "0",
            "font_name": "Consolas", "font_size": "0", "font_weight": "400",
            "workspace_style": "box", "workspace_width": "24", "widget_gap": "8", "icon_size": "20",
            "icon_mode": "both", "widget_colors": "", "clock_format": "HH:mm", "date_format": "yyyy-MM-dd",
            "separator_style": "line", "hover": "false", "title_bold": "false", "title_format": "{title}",
            "left": "workspaces layout", "center": "title",
            "right": "cpu separator mem separator net separator volume separator battery separator date separator clock",
            "show_title": "true", "show_layout": "true", "show_cpu": "true", "show_mem": "true",
            "show_net": "true", "show_volume": "true", "show_battery": "true", "show_date": "true",
            "show_clock": "true",
        },
    },
    BarPreset {
        name: "minimal",
        label: "Minimal",
        blurb: "Quiet and monochrome: workspace dots, the clock centred, volume and battery.",
        pairs: looks! {
            "shape": "strip", "palette": "mono", "height": "26", "padding": "14", "margin": "8",
            "radius": "12", "island_padding": "10", "border_width": "0",
            "font_name": "Segoe UI", "font_size": "0", "font_weight": "400",
            "workspace_style": "dot", "workspace_width": "18", "widget_gap": "16", "icon_size": "20",
            "icon_mode": "glyph", "widget_colors": "", "clock_format": "HH:mm", "date_format": "ddd dd MMM",
            "separator_style": "line", "hover": "false", "title_bold": "false", "title_format": "{title}",
            "left": "workspaces", "center": "clock", "right": "volume battery",
            "show_volume": "true", "show_battery": "true", "show_clock": "true",
        },
    },
    BarPreset {
        name: "floating",
        label: "Floating",
        blurb: "A detached rounded bar with an outline and a rounded workspace pill, Nord.",
        pairs: looks! {
            "shape": "floating", "palette": "nord", "height": "32", "padding": "10", "margin": "8",
            "radius": "12", "island_padding": "10", "border_width": "1",
            "font_name": "Segoe UI", "font_size": "0", "font_weight": "600",
            "workspace_style": "pill", "workspace_width": "34", "widget_gap": "16", "icon_size": "20",
            "icon_mode": "glyph", "widget_colors": "", "clock_format": "HH:mm", "date_format": "ddd dd MMM",
            "separator_style": "line", "hover": "true", "title_bold": "false", "title_format": "{title}",
            "left": "workspaces", "center": "title", "right": "volume battery date clock",
            "show_title": "true", "show_volume": "true", "show_battery": "true", "show_date": "true",
            "show_clock": "true",
        },
    },
    BarPreset {
        name: "islands",
        label: "Islands",
        blurb: "Each zone its own rounded island, icons from the Windows icon font, Catppuccin.",
        pairs: looks! {
            "shape": "islands", "palette": "catppuccin", "height": "34", "padding": "6", "margin": "8",
            "radius": "10", "island_padding": "12", "border_width": "0",
            "font_name": "Segoe UI", "font_size": "0", "font_weight": "600",
            "workspace_style": "pill", "workspace_width": "30", "widget_gap": "14", "icon_size": "20",
            "icon_mode": "glyph", "widget_colors": "", "clock_format": "HH:mm", "date_format": "ddd dd MMM",
            "separator_style": "line", "hover": "true", "title_bold": "false", "title_format": "{title}",
            "left": "workspaces layout", "center": "title", "right": "cpu mem volume battery date clock",
            "show_title": "true", "show_layout": "true", "show_cpu": "true", "show_mem": "true",
            "show_volume": "true", "show_battery": "true", "show_date": "true", "show_clock": "true",
        },
    },
    BarPreset {
        name: "pills",
        label: "Pills",
        blurb: "Every widget its own capsule in a palette colour, black or white text to suit each fill, Catppuccin.",
        pairs: looks! {
            "shape": "pills", "palette": "catppuccin", "height": "32", "padding": "6", "margin": "8",
            "radius": "16", "island_padding": "10", "border_width": "0",
            "font_name": "Segoe UI", "font_size": "0", "font_weight": "700",
            "workspace_style": "pill", "workspace_width": "30", "widget_gap": "6", "icon_size": "20",
            "icon_mode": "glyph",
            "widget_colors": "layout:teal cpu:orange mem:yellow volume:blue battery:green date:pink clock:purple",
            "clock_format": "HH:mm", "date_format": "ddd dd MMM",
            "separator_style": "line", "hover": "true", "title_bold": "false", "title_format": "{title}",
            "left": "workspaces layout", "center": "title", "right": "cpu mem volume battery date clock",
            "show_title": "true", "show_layout": "true", "show_cpu": "true", "show_mem": "true",
            "show_volume": "true", "show_battery": "true", "show_date": "true", "show_clock": "true",
        },
    },
    // Internal name kept from the earlier "Menu bar" preset, so Settings and
    // its tests keep finding it. The logo is Astur's own bird, never an Apple
    // mark; "Mac" is only a description of the look.
    BarPreset {
        name: "menubar",
        label: "Mac style",
        blurb: "A thin, quiet menu bar like a Mac's: the Astur logo and the app's name on the left, workspace dots in the middle, icons, date and time on the right.",
        pairs: looks! {
            "shape": "strip", "palette": "mono", "height": "24", "padding": "12", "margin": "8",
            "radius": "12", "island_padding": "10", "border_width": "0",
            "font_name": "Segoe UI", "font_size": "13", "font_weight": "400",
            "workspace_style": "dot", "workspace_width": "18", "widget_gap": "14", "icon_size": "20",
            "icon_mode": "glyph_only", "widget_colors": "", "clock_format": "h:mm TT", "date_format": "ddd d MMM",
            "separator_style": "line", "hover": "true", "title_bold": "true", "title_format": "{app}",
            "left": "logo title", "center": "workspaces", "right": "net volume battery date clock",
            "show_title": "true", "show_net": "true", "show_volume": "true", "show_battery": "true",
            "show_date": "true", "show_clock": "true",
        },
    },
    BarPreset {
        name: "fluent",
        label: "Fluent",
        blurb: "Taskbar-like: workspaces and app icons centred, an accent underline, Windows colours.",
        pairs: looks! {
            "shape": "strip", "palette": "system", "height": "40", "padding": "12", "margin": "8",
            "radius": "12", "island_padding": "10", "border_width": "0",
            "font_name": "Segoe UI", "font_size": "14", "font_weight": "400",
            "workspace_style": "underline", "workspace_width": "34", "widget_gap": "16", "icon_size": "22",
            "icon_mode": "glyph", "widget_colors": "", "clock_format": "HH:mm", "date_format": "ddd dd MMM",
            "separator_style": "line", "hover": "true", "title_bold": "false", "title_format": "{title}",
            "left": "layout", "center": "workspaces apps", "right": "volume battery date clock",
            "show_layout": "true", "show_apps": "true", "show_volume": "true", "show_battery": "true",
            "show_date": "true", "show_clock": "true",
        },
    },
    BarPreset {
        name: "powerline",
        label: "Powerline",
        blurb: "Square coloured segments joined by arrows, as in a terminal prompt, monospace, Gruvbox.",
        pairs: looks! {
            "shape": "pills", "palette": "gruvbox", "height": "24", "padding": "0", "margin": "8",
            "radius": "0", "island_padding": "10", "border_width": "0",
            "font_name": "Consolas", "font_size": "0", "font_weight": "600",
            "workspace_style": "box", "workspace_width": "24", "widget_gap": "6", "icon_size": "20",
            "icon_mode": "glyph",
            "widget_colors": "layout:yellow cpu:orange mem:yellow net:teal volume:blue battery:green date:purple clock:orange",
            "clock_format": "HH:mm", "date_format": "ddd dd MMM",
            "separator_style": "powerline", "hover": "false", "title_bold": "false", "title_format": "{title}",
            "left": "workspaces layout", "center": "title", "right": "cpu mem net volume battery date clock",
            "show_title": "true", "show_layout": "true", "show_cpu": "true", "show_mem": "true",
            "show_net": "true", "show_volume": "true", "show_battery": "true", "show_date": "true",
            "show_clock": "true",
        },
    },
];

impl Config {
    pub fn defaults() -> Self {
        Config {
            per_monitor: false,
            start_tiled: true,
            outer_gap: 8,
            inner_gap: 8,
            master_ratio: 0.55,
            workspaces: 10,
            workspace_keys: parse_keys("1 2 3 4 5 6 7 8 9 0"),
            workspace_names: vec![
                "Web".into(),
                "Code".into(),
                "Chat".into(),
                "Files".into(),
                "Media".into(),
            ],
            workspace_icons: Vec::new(),
            layout: "dwindle".to_string(),
            monitor_layouts: Vec::new(),
            scroll_default_width: 0.5,
            scroll_width_presets: SCROLL_WIDTHS.to_vec(),
            scroll_height_presets: SCROLL_HEIGHTS.to_vec(),
            scroll_center: "never".to_string(),
            tab_bar_height: 26,
            terminal: "wt.exe".to_string(),
            browser: String::new(),
            unfocused_opacity: 0.8,
            border_enabled: true,
            focused_border: parse_color("#66AAFF", 0x00FFAA66),
            unfocused_border: parse_color("#223A5E", 0x005E3A22),
            cursor_follows_focus: true,
            focus_follows_mouse: false,
            animations: true,
            animation_ms: 140,
            workspace_slide: true,
            workspace_anim: "slide".to_string(),
            window_anim: "glide".to_string(),
            animation_easing: "cubic".to_string(),
            popup_font_name: "Segoe UI".to_string(),
            popup_font_size: 18,
            popup_font_weight: 400,
            popup_radius: 16,
            popup_border_width: 1,
            popup_opacity: 94,
            popup_bg: None,
            popup_fg: None,
            popup_muted: None,
            popup_accent: None,
            popup_accent_fg: None,
            popup_border: None,
            launcher_enabled: true,
            launcher_width: 660,
            launcher_wide_width: 1060,
            launcher_height: 452,
            launcher_row_height: 40,
            launcher_icon_size: 32,
            launcher_padding: 16,
            launcher_selection_radius: 12,
            launcher_placement: "cursor_monitor".to_string(),
            launcher_source_apps: true,
            launcher_source_files: true,
            launcher_source_calc: true,
            launcher_source_web: true,
            launcher_source_windows: false,
            launcher_source_clipboard: false,
            launcher_source_emoji: false,
            launcher_web_url: "https://www.google.com/search?q={query}".to_string(),
            launcher_max_results: 40,
            launcher_file_scope: String::new(),
            launcher_file_exclude: vec![".git".into(), "node_modules".into(), "target".into()],
            launcher_mru: true,
            launcher_entries: Vec::new(),
            system_menu_enabled: true,
            system_menu_width: 380,
            system_power_items: vec![
                "lock".into(),
                "sleep".into(),
                "hibernate".into(),
                "sign_out".into(),
                "restart".into(),
                "shutdown".into(),
            ],
            system_setup_items: vec![
                "settings".into(),
                "open_config".into(),
                "reload".into(),
                "restart_astur".into(),
                "screenshot".into(),
                "wallpapers".into(),
            ],
            system_actions: Vec::new(),
            alt_tab_replacement: false,
            scratchpad_enabled: false,
            scratchpad_command: "wt.exe".to_string(),
            scratchpad_class: "CASCADIA_HOSTING_WINDOW_CLASS".to_string(),
            clipboard_history: false,
            clipboard_limit: 50,
            clipboard_prefix: ">".to_string(),
            emoji_picker: false,
            emoji_prefix: ":".to_string(),
            wallpaper_dir: String::new(),
            workspace_wallpapers: Vec::new(),
            media_enabled: false,
            ipc_enabled: false,
            ipc_pipe: "astur".to_string(),
            yasb_enabled: false,
            yasb_port: 6123,
            ipc_allow_launch: false,
            foreground_lock_disable: true,
            async_window_pos: true,
            persist_state: true,
            log_level: "error".to_string(),
            unknown_keys: Vec::new(),
            extra_hotkeys: vec![HotkeyDef {
                chord: "ALT+GRAVE".to_string(),
                action: "scratchpad".to_string(),
                argument: String::new(),
            }],
            window_rules: Vec::new(),
            bar_enabled: true,
            bar_height: 28,
            bar_bottom: false,
            bar_font_size: 0,
            bar_show_title: true,
            bar_show_clock: true,
            bar_clock_24h: true,
            bar_show_layout: true,
            bar_bg: None, // follow theme
            bar_fg: None,
            bar_accent: None,
            bar_inactive: None,
            bar_font_name: "Segoe UI".to_string(),
            bar_hide_empty: false,
            bar_widget_gap: 16,
            bar_icon_size: 20,
            bar_workspace_width: 34,
            bar_icon_mode: "both".to_string(),
            bar_show_tooltips: true,
            bar_show_app_labels: false,
            bar_cpu_format: "{value}%".to_string(),
            bar_mem_format: "{value}%".to_string(),
            bar_battery_format: "{value}%".to_string(),
            bar_net_format: "D:{down} U:{up}".to_string(),
            bar_volume_format: "{value}%".to_string(),
            bar_clock_format: "HH:mm".to_string(),
            bar_icon_cpu: "CPU".to_string(),
            bar_icon_mem: "RAM".to_string(),
            bar_icon_battery: "BAT".to_string(),
            bar_icon_net: "NET".to_string(),
            bar_icon_volume: "VOL".to_string(),
            bar_padding: 8,
            bar_show_date: false,
            bar_date_format: "ddd dd MMM".to_string(),
            bar_show_cpu: true,
            bar_show_mem: true,
            bar_show_battery: true,
            ignore_classes: Vec::new(),
            float_classes: Vec::new(),
            key_focus_next: 0x4A,     // J
            key_focus_prev: 0x4B,     // K
            key_shrink_master: 0x48,  // H
            key_grow_master: 0x4C,    // L
            key_promote_master: 0x4D, // M
            key_toggle_tiling: 0x54,  // T
            key_toggle_float: 0x46,   // F
            key_close_window: 0x57,   // W
            key_cycle_width: 0x52,    // R
            key_center_column: 0x43,  // C
            key_consume_left: 0xDB,   // [ (VK_OEM_4)
            key_consume_right: 0xDD,  // ] (VK_OEM_6)
            main_key: MainKey::LAlt,
            theme: "dark".to_string(),
            acrylic: false,
            bar_floating: false,
            bar_margin: 8,
            bar_radius: 12,
            bar_autohide: false,
            bar_wheel_ws: true,
            bar_show_net: false,
            bar_show_volume: true,
            bar_show_apps: false,
            bar_show_media: false,
            bar_left: vec!["workspaces".into(), "apps".into()],
            bar_center: vec!["title".into()],
            bar_right: vec![
                "layout".into(),
                "cpu".into(),
                "mem".into(),
                "net".into(),
                "volume".into(),
                "battery".into(),
                "date".into(),
                "clock".into(),
            ],
            bar_palette: "classic".to_string(),
            bar_workspace_style: "box".to_string(),
            bar_font_weight: 600,
            bar_surface: None,
            bar_accent_fg: None,
            bar_border: None,
            bar_border_width: 0,
            bar_shape: String::new(),
            bar_island_padding: 10,
            bar_widget_colors: Vec::new(),
            bar_clicks: Vec::new(),
            bar_keyboard_format: "{lang}".to_string(),
            bar_disk_drive: "C:\\".to_string(),
            bar_disk_format: "{free}".to_string(),
            bar_icon_disk: "DISK".to_string(),
            bar_uptime_format: "{value}".to_string(),
            bar_custom_command: String::new(),
            bar_custom_format: "{output}".to_string(),
            bar_custom_interval: 10,
            bar_workspace_label: "auto".to_string(),
            bar_workspace_apps: false,
            bar_monitors: "all".to_string(),
            bar_separator_style: "line".to_string(),
            bar_hover: false,
            bar_title_format: "{title}".to_string(),
            bar_title_bold: false,
        }
    }

    /// The bar shape in effect: the `shape` line when there is one, else the
    /// older `floating` key. So a file or a GUI that only knows `floating`
    /// keeps working, and a `shape` line wins whatever the line order or the
    /// file it is in.
    pub fn effective_bar_shape(&self) -> &str {
        match self.bar_shape.as_str() {
            "" if self.bar_floating => "floating",
            "" => "strip",
            s => s,
        }
    }

    /// The bar's colours for one theme: the palette's roles with every pinned
    /// key on top. `accent_fg` on auto is the bar background while `bg` is
    /// pinned (the rule before palettes, so such a file looks the same),
    /// otherwise the palette's own. The WM swaps in the Windows accent for
    /// `palette = system`; this crate never reads it (no Win32).
    pub fn bar_roles(&self, light: bool) -> BarRoles {
        let p = palette(&self.bar_palette);
        let base = if light { p.light } else { p.dark };
        let bg = self.bar_bg.unwrap_or(base.bg);
        let accent_fg = match self.bar_bg {
            Some(_) => bg,
            None => base.accent_fg,
        };
        BarRoles {
            bg,
            surface: self.bar_surface.unwrap_or(base.surface),
            fg: self.bar_fg.unwrap_or(base.fg),
            muted: self.bar_inactive.unwrap_or(base.muted),
            accent: self.bar_accent.unwrap_or(base.accent),
            accent_fg: self.bar_accent_fg.unwrap_or(accent_fg),
            swatches: base.swatches,
        }
    }

    /// The capsule colour `widget_colors` gives `widget` under the roles `r`:
    /// a swatch (so a palette or theme switch recolours it) or a pinned #hex.
    /// None = the surface, also for `surface` itself. The last entry wins.
    pub fn bar_widget_fill(&self, widget: &str, r: &BarRoles) -> Option<u32> {
        let (_, spec) = self.bar_widget_colors.iter().rfind(|(w, _)| w == widget)?;
        match SWATCH_NAMES.iter().position(|s| s == spec) {
            Some(i) => Some(r.swatches[i]),
            None => Some(parse_color(spec, NO_LEGACY)).filter(|&c| c != NO_LEGACY),
        }
    }

    /// The layout monitor `index` uses (0-based: monitor 1 is index 0): its
    /// `monitor_layouts` entry when set, else the default `layout`.
    pub fn layout_for_monitor(&self, index: usize) -> &str {
        match self.monitor_layouts.get(index) {
            Some(l) if !l.is_empty() => l,
            _ => &self.layout,
        }
    }

    /// What clicking `widget` with `button` (0 left, 1 right, 2 middle)
    /// does, as (action, argument): its `_click` line, else the
    /// BAR_CLICK_DEFAULTS entry, else ("none", ""). No allocation: the bar
    /// resolves every widget on each snapshot.
    pub fn bar_click(&self, widget: &str, button: usize) -> (&str, &str) {
        if let Some((_, _, v)) = self
            .bar_clicks
            .iter()
            .rfind(|(w, b, _)| w == widget && *b as usize == button)
        {
            return v.split_once(' ').unwrap_or((v, ""));
        }
        BAR_CLICK_DEFAULTS
            .iter()
            .find(|(w, _)| *w == widget)
            .and_then(|(_, a)| a.get(button))
            .map_or(("none", ""), |a| (a, ""))
    }

    /// `monitors` as a mask for `bar_on`: bit i = screen i + 1 (left to
    /// right), or BAR_PRIMARY_ONLY.
    pub fn bar_monitor_mask(&self) -> u64 {
        match self.bar_monitors.as_str() {
            "primary" => BAR_PRIMARY_ONLY,
            "all" => !BAR_PRIMARY_ONLY,
            list => list
                .split(' ')
                .filter_map(|n| n.parse::<u32>().ok())
                .filter(|n| (1..=63).contains(n))
                .fold(0, |m, n| m | 1 << (n - 1)),
        }
    }
}

const DEFAULT_CONFIG: &str = "\
# ============================================================================
# Astur configuration  (window manager)
# ============================================================================
# Location : %USERPROFILE%\\.astur\\astur.conf
#            (override with the ASTUR_CONFIG environment variable)
# The status bar is configured separately in navbar.conf (same folder).
# Apply    : save this file; Astur hot-reloads it about 0.1 s after the save,
#            redoing only what changed (a colour edit does not re-tile). The
#            system menu's Reload (or IPC `reload`) always re-applies everything.
# Regen    : delete this file and relaunch to get a fresh, fully-commented copy.
# Encoding : save as UTF-8. The window manager never overwrites a file it
#            cannot read (a UTF-16 \"Unicode\" save, or one locked mid-save): it
#            keeps its current settings (built-in defaults at startup), logs the
#            error to astur.log and tries again.
#
# Syntax   : one  key = value  per line. '#' starts a comment. Blank lines and
#            surrounding whitespace are ignored. Unknown keys are ignored, so a
#            typo silently falls back to the default below rather than erroring.
#            Set log_level = info to have unknown keys reported in astur.log.
#
# Pixels   : every size here is a LOGICAL pixel, i.e. what it measures on a
#            100%-scale display. Astur is per-monitor-DPI aware, so a bar height
#            of 28 draws as 28 px at 100%, 35 px at 125% and 42 px at 150%, and
#            two monitors at different scales each get the right physical size.
#            (Before 2.1.3 the whole process was DPI-unaware; these numbers were
#            stretched by Windows instead, which also broke tiling — issue #5.)
#
# Value types:
#   bool   : true / false   (also accepts yes/no, 1/0, on/off; anything else
#            counts as false)
#   int    : whole number; clamped to the stated range
#   float  : decimal; clamped to the stated range
#   floats : space/comma list of decimals, each clamped; none valid = default
#   colour : #RRGGBB hex (e.g. #66AAFF). Malformed values keep the default.
#   text   : literal string (program name / command line)
#   keys   : space/comma list of key names: 0-9, A-Z, F1-F24, Space, Tab,
#            Enter, Backspace, Esc, Left Up Right Down, Home, End, PgUp, PgDn,
#            Insert, Delete, [ ] ` - = Semicolon Quote Comma Period Slash
#            Backslash (punctuation by its place on a US keyboard).
#
# Every key below is shown set to its built-in DEFAULT.
# ============================================================================

# ---------------------------------------------------------------------------
# Workspaces & layout
# ---------------------------------------------------------------------------

# How workspaces map to monitors.
#   shared      = workspaces are numbered globally, starting from your MAIN
#                 (primary) monitor and rotating outward. With 3 monitors,
#                 ws1 = main monitor, ws2 = next, ws3 = next, ws4 = main (its
#                 2nd workspace), and so on. The workspace key jumps focus to
#                 whichever monitor owns that workspace.
#   per_monitor = each monitor has its own independent workspaces 1..N
#                 (GlazeWM style). The workspace key switches the workspace on
#                 the monitor that currently has focus.
# values: shared | per_monitor
workspace_mode = shared

# Number of workspaces.  int 1 - 20
#   shared mode      = TOTAL across all monitors (distributed from the main
#                      monitor outward).
#   per_monitor mode = workspaces per monitor.
# More keys than 10 are fine (workspace_keys = 1 2 3 4 5 6 7 8 9 0 G). A
# workspace with no key is still reached by its bar pill, IPC `switch N`
# (ipc_enabled) or an extra_hotkeys switch_workspace chord.
workspaces = 10

# Keys (with LEFT ALT) that select workspaces 1, 2, 3, ... in order. Add Shift
# to MOVE the focused window to that workspace instead. Avoid the fixed binds
# (J K H L M T F W R C [ ], arrows, Enter, Space, Home, End): a fixed bind wins
# and that workspace gets no key. Example for Alt+Q/E/Y = ws 1/2/3:
#   workspace_keys = Q E Y U I O
# keys
workspace_keys = 1 2 3 4 5 6 7 8 9 0
# Optional display labels. Missing positions fall back to workspace number.
# list; quote-free names should not contain commas.
workspace_names = Web, Code, Chat, Files, Media
# Optional compact pills. Text, symbols from selected font, or icon-font glyphs.
workspace_icons =

# Tile windows automatically on launch (Alt+T toggles at runtime).  bool
start_tiled = true

# Tiling layout.
#   dwindle = each new window splits the remaining space, spiralling into the
#             bottom corner (spiral default).
#   master  = one large master column on the left, the rest stacked on the right.
#   columns = equal-width columns
#   grid    = balanced rows and columns
#   monocle = every tiled window fills work area; focused window sits on top
#   scrolling = Niri-style: windows sit in columns on an endless horizontal
#             strip; columns past the screen edge are a scroll away.
# values: dwindle | master | columns | grid | monocle | scrolling
layout = dwindle

# Per-monitor layout, by monitor number (1 = leftmost, as in window_rules).
# Empty or 'default' = use layout above. Example: scrolling on monitor 1 and
# master on monitor 3:  monitor_layouts = scrolling, , master
monitor_layouts =

# Fraction of the width given to the master window (master layout, and the
# master split that Alt+H / Alt+L adjust).  float 0.10 - 0.90
master_ratio = 0.55

# Scrolling layout: width of a new column, as a fraction of the screen
# (0.5 = two columns side by side).  float 0.10 - 1.00
scroll_default_width = 0.5
# Column widths Alt+R cycles through, smallest to largest.  floats 0.10 - 1.00
scroll_width_presets = 0.333, 0.5, 0.667, 1.0
# Window heights within a column Alt+Shift+R cycles through, smallest to
# largest.  floats 0.10 - 0.90
scroll_height_presets = 0.333, 0.5, 0.667
# When focus moves to a column, the view:
#   never       = scrolls only as far as needed to show it fully
#   always      = keeps the focused column centred
#   on-overflow = stays put if the column fits, otherwise centres it
# values: never | always | on-overflow
scroll_center = never
# Height of the tab bar on a tabbed column (Alt+Shift+T).  int 16 - 64
tab_bar_height = 26

# ---------------------------------------------------------------------------
# Gaps
# ---------------------------------------------------------------------------

# Gap in pixels between windows and the screen edge.  int (>= 0)
outer_gap = 8
# Gap in pixels between adjacent windows.  int (>= 0)
inner_gap = 8

# ---------------------------------------------------------------------------
# Focus & cursor behaviour
# ---------------------------------------------------------------------------

# Warp the mouse cursor onto the window that gains focus via Alt+arrows and on
# workspace switches.  bool
cursor_follows_focus = true

# Focus follows mouse: hovering a window with the cursor focuses it, like
# Focus follows mouse. Off by default (Windows focus-steal is more abrupt
# than on Linux); set true to enable.  bool
focus_follows_mouse = false

# ---------------------------------------------------------------------------
# Animations
# ---------------------------------------------------------------------------
# Animations apply to the WORKSPACE SWITCH. The switch itself is always instant
# and correct underneath; the animation is a cosmetic overlay composited on top,
# so it is smooth even with heavy apps (the apps themselves aren't moved).
# Window glide moves and resizes windows to their new slots (window_anim below).

# Enable animations.  bool
animations = true
# Animation duration in milliseconds. Lower = snappier, higher = smoother.
# 0 disables (same as animations = false). Up to 250, clicks and the wheel
# pass through a running animation to the windows (already in their final
# place underneath); above 250 the animation blocks mouse input until it ends.
# int 0-2000
animation_ms = 140
# Workspace-switch animation style:
#   off    - instant switch, no overlay
#   slide  - old slides off one edge, new slides in from the other (default)
#   spring - slide that overshoots the target then settles back (Hyprland-like)
#   fade   - old fades out, new fades in, both in place
# string
workspace_anim = slide
# Back-compat toggle. false forces workspace_anim = off; true leaves it as set.
# Prefer workspace_anim above. Needs animations = true.  bool
workspace_slide = true
# Window move / open / close / re-tile animation:
#   off   - instant placement
#   glide - windows glide to their new slots, moving and resizing together
#           (the live window image is scaled in between, 1:1 at both ends).
#           New layouts or keyboard/mouse input cancel obsolete glides.
#           Composited on a brief overlay (real windows are placed instantly
#           underneath). Drawn over a
#           cached copy of the wallpaper: for about 1.5 s after it changes (or a
#           reload that re-tiles or changes animation settings), and without
#           Explorer, placement is instant.  string
window_anim = glide
# Motion curve used by popup/placement transitions. values: cubic | smooth | spring
animation_easing = cubic

# ---------------------------------------------------------------------------
# Appearance: theme, window borders & dimming
# ---------------------------------------------------------------------------

# Colour theme for Astur's own surfaces (launcher, system menu, and any bar
# colour left at its default — explicit navbar.conf colours always win).
#   dark   = dark surfaces (default)
#   light  = light surfaces
#   auto   = follow the Windows app theme (Settings > Personalisation > Colours)
#            ('system' also accepted)
# values: dark | light | auto
theme = dark

# EXPERIMENTAL: acrylic blur behind the launcher and system menu (frosted-glass
# look). Uses an undocumented Windows API; if popups render oddly, turn it off.
# bool
acrylic = false
# Popup typography and geometry. See the pixel note at the top of this file.
popup_font_name = Segoe UI
popup_font_size = 18
popup_font_weight = 400
popup_radius = 16
popup_border_width = 1
popup_opacity = 94
# Set colours to auto to follow theme, or explicit #RRGGBB.
popup_bg = auto
popup_fg = auto
popup_muted = auto
popup_accent = auto
popup_accent_fg = auto
popup_border = auto

# Dim unfocused windows to this opacity.  float 0.10 - 1.00  (1.0 = disabled)
# Trade-off worth knowing: any value below 1.0 makes Astur add WS_EX_LAYERED to
# every window it manages, which puts those apps on a different composition
# path. A few (older games, some capture tools and Electron overlays) behave
# differently when layered. Set 1.0 to leave app window styles untouched.
# Astur removes the style again when it quits or reloads with dimming off.
unfocused_opacity = 0.8

# Coloured window borders. Requires Windows 11 (no effect on Windows 10).  bool
border_enabled = true
# Border colour of the focused window.    colour
focused_border = #66AAFF
# Border colour of unfocused windows.     colour
unfocused_border = #223A5E

# ---------------------------------------------------------------------------
# Window rules
# ---------------------------------------------------------------------------
# Comma-separated lists of window CLASS names (not titles). Use a tool like
# AutoHotkey's Window Spy, or Spy++, to find a window's class.

# Never tile/manage these (in addition to the built-in shell/tooltip/lock-screen
# filtering). Example: ignore_classes = TaskManagerWindow, MyOverlayClass
ignore_classes =
# Manage but always float these (let the app place them; don't tile).
# Example: float_classes = #32770, MsiDialogCloseClass
float_classes =
# Rich rules. Records separated by ;;, fields by |:
# action|exe|class|title|workspace|monitor
# Empty match fields are wildcards. workspace/monitor are 1-based; 0/empty = unset.
# Example, keep PowerToys popups out of the tiling (match by exe: titles are
# localized): ignore|PowerToys.AdvancedPaste.exe ;; ignore|PowerToys.Peek.UI.exe
window_rules =

# ---------------------------------------------------------------------------
# Launcher and system menu
# ---------------------------------------------------------------------------

# Program launched by Alt+Enter: a cmd.exe command line run through `start`,
# so PATH and App Execution Aliases (e.g. wt.exe) resolve and arguments work
# (wt -p Ubuntu). Quote a path with spaces (or whose %VARIABLES% expand to
# spaces) when arguments follow it:
# \"C:\\Program Files\\App\\app.exe\" --flag. A bare path to an existing file
# works unquoted. cmd characters (& | < > ^ %) act as in cmd, so quote a URL
# that has &. Same for browser, scratchpad_command, launch hotkeys, IPC
# launch and cmd: targets in launcher_entries / system_actions.  text
terminal = wt.exe
# Program launched by Alt+Shift+Enter. Leave EMPTY to open the system default
# browser.  text
browser =

# Alt+Space picker. Placement: cursor_monitor | focused_monitor | primary_monitor
launcher_enabled = true
launcher_width = 660
launcher_wide_width = 1060
launcher_height = 452
launcher_row_height = 40
launcher_icon_size = 32
launcher_padding = 16
launcher_selection_radius = 12
launcher_placement = cursor_monitor
launcher_max_results = 40
launcher_mru = true
# Providers can be enabled independently.
launcher_source_apps = true
launcher_source_files = true
launcher_source_calc = true
launcher_source_web = true
launcher_source_windows = false
launcher_source_clipboard = false
launcher_source_emoji = false
launcher_web_url = https://www.google.com/search?q={query}
# Empty file scope uses current user profile. Excludes are comma-separated path fragments.
launcher_file_scope =
launcher_file_exclude = .git, node_modules, target
# Custom records: label|target|icon ;; label|target|icon. icon: auto/path/builtin name.
launcher_entries =

# Alt+Shift+Space system menu.
system_menu_enabled = true
system_menu_width = 380
# Built-ins: lock sleep hibernate sign_out restart shutdown settings open_config
# reload restart_astur screenshot wallpapers. Order controls menu order.
system_power_items = lock, sleep, hibernate, sign_out, restart, shutdown
system_setup_items = settings, open_config, reload, restart_astur, screenshot, wallpapers
# Custom: category|label|target|icon|confirm, records separated by ;;
system_actions =

# ---------------------------------------------------------------------------
# Optional desktop features
# ---------------------------------------------------------------------------
alt_tab_replacement = false
scratchpad_enabled = false
scratchpad_command = wt.exe
scratchpad_class = CASCADIA_HOSTING_WINDOW_CLASS
clipboard_history = false
clipboard_limit = 50
clipboard_prefix = >
emoji_picker = false
emoji_prefix = :
wallpaper_dir =
# One wallpaper path per workspace, separated by ;;. Empty positions do nothing.
workspace_wallpapers =
media_enabled = false
persist_state = true
# Diagnostics log at %USERPROFILE%\\.astur\\astur.log (rotates at 1 MB).
#   off   = write nothing
#   error = failures only (default)
#   info  = + startup environment, monitors/DPI, hook re-arms, config reloads
#   debug = + per-command detail and latency probes (switch/glide/retile
#           stage timings, event counters). Verbose; for reproducing a bug.
# Run `astur.exe --check` for a paste-ready diagnostics dump.
log_level = error
# Local named-pipe command API. Pipe name only; no remote/network listener.
# The pipe is restricted to your own user account and rejects remote clients.
ipc_enabled = false
ipc_pipe = astur
# Allow `launch <command>` over IPC to run arbitrary commands. Window-management
# verbs (switch/move/focus/layout/...) work regardless. Leave this off unless you
# actually script launching through Astur: on, any process running as you can use
# Astur as the parent process for a command of its choosing.
ipc_allow_launch = false

# YASB workspace integration. Uses YASB's existing GlazeWM workspace widget.
# WebSocket server on 127.0.0.1 only; independent of the named-pipe API above.
# Local processes can read workspace/window handles and switch workspaces.
# Browser origins and arbitrary launch commands are rejected. Hot-reloaded.
# Disable Astur's bar in navbar.conf when replacing it with YASB.
yasb_enabled = false
yasb_port = 6123

# Windows normally refuses to let a background app steal the foreground. Astur
# disables that timeout so focus changes land reliably. It is a SYSTEM-WIDE
# setting affecting every application, not just Astur; the previous value is put
# back when Astur exits gracefully. Set false to leave the system setting alone
# (focus may occasionally flash in the taskbar instead of switching).
foreground_lock_disable = true
# Place windows without waiting for each app to finish moving (posted
# SetWindowPos). A re-tile then completes after the slowest app instead of
# after every app in turn, and a busy app no longer delays the next command
# that only moves windows. (Showing and hiding windows on a workspace switch
# still waits for each app.) Set false if some app lands in the wrong place
# after a re-tile; Astur then waits for each app, as it did before this key
# existed.  bool
async_window_pos = true
# Extra bindings: chord|action|argument ;; chord|action|argument
# A chord is keys joined by +: MOD (the main key, see main_key below), CTRL,
# SHIFT, ALT, WIN and one key name. It needs MOD, ALT or WIN. With
# main_key = lalt, ALT is Left Alt, the same as MOD.
# Escape literal field separators with a leading backslash.
extra_hotkeys = ALT+GRAVE|scratchpad|

# ============================================================================
# Hotkeys (the main key, Left Alt unless main_key says otherwise)
# ============================================================================
# The main key is held for every shortcut below and for mouse move / resize;
# 'Alt' in this list means the main key. Choices:
#   lalt      Left Alt (default). Astur keeps it: apps never see Left Alt, so
#             use Right Alt for app shortcuts (Alt+F4, menus).
#   win       Windows key. Apps keep it; only Astur's shortcuts are taken and
#             they replace Windows' own (Win+D ...). Win+L always locks the
#             PC, so key_grow_master's default L becomes O (if O is free).
#   alt       Either Alt, GlazeWM-style. Apps keep Alt shortcuts Astur doesn't use.
#   ctrl+win  Ctrl and the Windows key held together.
#   capslock  Caps Lock. It no longer turns capitals on: turn it off first.
#             Pressed where Astur can't see it (an admin window, the lock
#             screen, a UAC prompt) it still toggles; press it there again.
main_key = lalt
#   Alt + left-drag      move window under cursor (drops back into the tiling)
#   Alt + right-drag     resize nearest corner (red bracket marker)
#   Alt+T                toggle tiling on/off (floating mode; workspaces kept)
#   Alt+J / Alt+K        focus next / previous window in the stack
#   Alt+Shift+J / K      swap window order in the stack
#   Alt+arrows           focus window by direction (cursor follows)
#   Alt+Shift+arrows     move window by direction (across monitors)
#   Alt+M                promote focused window to master
#   Alt+H / Alt+L        master layout: shrink / grow the master column;
#                        dwindle layout: shrink / grow the focused window's split
#   Alt+F                toggle floating for the focused window
#   Alt+W                close the focused window
#   Alt+Enter            launch terminal
#   Alt+Shift+Enter      launch browser
#   Alt+<workspace_key>  switch to that workspace (see workspace_keys above)
#   Alt+Shift+<ws key>   move focused window to that workspace (and follow it)
#   Alt+Tab              normal task switcher (still works)
#   RIGHT ALT            normal Alt behaviour (with main_key = lalt)
#
# Scrolling layout only (elsewhere these do nothing or keep the meaning above):
#   Alt+Left / Right     focus column left / right (scrolls the view; at the
#                        strip end: next monitor)
#   Alt+Up / Down        focus window up / down in the column (tabbed: tab)
#   Alt+Shift+arrows     move column left / right (at the strip end: the window
#                        to the next monitor); window up / down in column
#   Alt+Shift+J / K      move column right / left
#   Alt+H / Alt+L        column width - / + 0.1
#   Alt+Shift+H / L      window height - / + 0.1 within its column
#   Alt+R / Alt+Shift+R  cycle column width / window height (presets above)
#   Alt+M                toggle full-width column
#   Alt+C                centre the focused column
#   Alt+[ / Alt+]        pull the window into the column left / right; if it
#                        shares a column, push it out into its own on that side
#   Alt+Shift+T          toggle the column between stacked and tabbed
#   Alt+Home / End       focus first / last column (+Shift: move column there)
#   Alt+wheel            focus column, wheel down = right (+Shift: move column);
#                        acts on the monitor under the cursor
#   Alt+touchpad swipe   focus column left / right, one per swipe (also
#                        horizontal wheel)
#   Alt + left-drag      drop on a window's left / right quarter = new column
#                        beside it, middle = stack into its column
#   Alt + right-drag     side edge = column width, top/bottom = window height
#
# The letter keys above (J K H L M T F W, and R C [ ] for scrolling) are
# rebindable. Each takes a single key name (see the 'keys' type at the top of
# this file). Arrows, Home, End and Enter are fixed.  key
key_focus_next = J
key_focus_prev = K
key_shrink_master = H
key_grow_master = L
key_promote_master = M
key_toggle_tiling = T
key_toggle_float = F
key_close_window = W
key_cycle_width = R
key_center_column = C
key_consume_left = [
key_consume_right = ]
# ============================================================================
";

const DEFAULT_NAVBAR: &str = "\
# ============================================================================
# Astur navbar configuration  (status bar)
# ============================================================================
# Location : %USERPROFILE%\\.astur\\navbar.conf
#            (override with the ASTUR_NAVBAR environment variable)
# Window-manager settings live separately in astur.conf (same folder).
# Apply    : save this file; Astur hot-reloads it about 0.1 s after the save.
# Presets  : a bar preset in Settings writes its keys into this file: the look
#            (shape, palette, sizes, fonts, workspace style, icon mode, widget
#            colours, clock and date formats, separator style, hover, title
#            format and weight, colours back to auto) and the
#            zones, with show_* on for every widget it lists. It never changes
#            enabled, bottom, autohide, wheel_workspaces or
#            hide_empty_workspaces. Edit any line afterwards as usual.
#
# One bar is drawn on every monitor (or those `monitors` lists). Each shows
# that monitor's workspaces and focused window. The tiling work area is reserved so windows never sit under a
# bar. Click a workspace pill to switch to it.
#
# Value types: bool, int, colour (#RRGGBB) -- see astur.conf for details.
#
# Pixels: every size here is a LOGICAL pixel (what it measures at 100% scale).
# Astur scales the bar per monitor, so height = 28 draws 28 px at 100% and
# 42 px at 150%, correctly on each screen of a mixed-scale setup.
# ============================================================================

# Show the bars.  bool   (set false to disable entirely)
enabled = true
# Bar thickness in pixels.  int 0 - 200  (0 also disables it)
height = 28
# Dock the bars at the bottom of each screen instead of the top.  bool
bottom = false
# Horizontal padding from each bar edge, in px.  int 0 - 200
padding = 8
# Font family for all bar text.  text  (e.g. Segoe UI, Cascadia Code, Consolas)
font_name = Segoe UI
# Text height in px. 0 = auto (about half the bar height).  int 0 - 100
font_size = 0
# Text weight: 400 = regular, 600 = semi-bold, 700 = bold.  int 100 - 900
font_weight = 600

# ---------------------------------------------------------------------------
# Style
# ---------------------------------------------------------------------------
# Bar shape:
#   strip     full-width bar along the screen edge (the default)
#   floating  detached from the edges by `margin`, with rounded corners
#   islands   a strip with a rounded island behind each zone
#   pills     a strip with a rounded pill behind each widget
# Islands and pills sit on the bar background (the desktop does not show
# between them yet). A non-empty `shape` line overrides `floating`, wherever
# it is (Settings writes it here when you pick a shape or apply a preset).
# Left empty, `floating` below decides between strip and floating.
shape =
# Floating bar: detached from the screen edge with a margin and rounded
# corners (waybar/Hyprland style). false = classic full-width strip. Ignored
# while a `shape` line is set.  bool
floating = false
# Gap between a floating bar and the screen edges, in px.  int 0 - 200
margin = 8
# Corner radius of a floating bar, an island or a pill, in px. At least half
# the height draws a full capsule.  int 0 - 40
radius = 12
# Space between an island's or a pill's edge and its content, in px.  int 0 - 50
island_padding = 10
# Outline width in px, 0 = none: around a floating bar, each island or each
# pill; a strip gets a line along its inner edge. Colour: `border`.  int 0 - 10
border_width = 0
# How the active workspace is marked:
#   box        accent block behind its number (the default)
#   pill       rounded accent pill behind its number
#   underline  short accent line under its number
#   dot        a dot per workspace instead of numbers (dim = empty), the
#              active one an accent pill
workspace_style = box
# Pills shape only: a colour per widget's pill, as widget:colour pairs. A
# colour is a palette swatch (red orange yellow green teal blue purple pink;
# switching palette or theme recolours it), surface, or #RRGGBB. Text on a
# coloured pill is black or white, whichever reads on that fill, unless
# accent_fg is pinned to a #hex. Text widgets only (not workspaces, apps,
# separator, spacer or logo). Empty = every pill in the surface colour.
#   e.g.  widget_colors = cpu:orange mem:yellow volume:blue clock:purple
widget_colors =
# What the separator widget draws:
#   line       a thin line (the default)
#   dot        a small dot
#   powerline  a small arrow; with shape = pills the pills join edge to
#              edge into square segments with arrow ends
separator_style = line
# Highlight the clickable item under the mouse pointer.  bool
hover = false
# Draw the window title in bold.  bool
title_bold = false
# Auto-hide: the bar slides away and stops reserving screen space; move the
# mouse to the bar's screen edge to reveal it.  bool
autohide = false

# ---------------------------------------------------------------------------
# Layout: three zones. List widget names per zone (space separated, drawn in
# order). Available widgets:
#   workspaces  the workspace pills (click to switch)
#   apps        app buttons for the active workspace's windows (click focuses)
#   title       focused window title
#   layout      layout name + tiling/floating state
#   cpu / mem   live CPU / RAM percent
#   net         network down/up speed
#   volume      speaker volume (wheel over it adjusts, click mutes)
#   battery     battery percent
#   date        the date (see date_format)
#   clock       the time
#   media       current media title
#   separator   divider (see separator_style)
#   spacer      fixed gap
#   keyboard    the active window's keyboard language (ENG, DEU, ...)
#   disk        free space on a drive (see disk_drive)
#   uptime      time since Windows started (42m, 5h 12m, 3d 4h)
#   custom      your own text, or the first line a command prints
#   media_controls  previous / play-pause / next buttons for any player
#   logo        the Astur bird (click: power menu)
# A widget only shows if it is BOTH listed in a zone and its show_* toggle
# below is true (so you can flip widgets without re-ordering). The widgets
# from keyboard on have no show_* toggle: listed means shown.
# ---------------------------------------------------------------------------
left = workspaces apps
center = title
right = layout cpu mem net volume battery date clock

# ---------------------------------------------------------------------------
# Behaviour
# ---------------------------------------------------------------------------
# Mouse wheel over the bar switches to the previous/next workspace, one per
# full wheel notch (a touchpad or smooth-scrolling wheel must scroll a whole
# notch's worth), and a fast spin lands straight on its last workspace. The
# wheel over the volume widget always adjusts volume instead.  bool
wheel_workspaces = true
# Hide empty workspace pills, showing only the active one and those with
# windows. false = show every workspace the monitor owns.  bool
hide_empty_workspaces = false
# Which screens get a bar: all, primary (the main screen only), or screen
# numbers counted from the left, e.g. 1 3. A screen without a bar keeps
# its whole height for windows.
monitors = all

# ---------------------------------------------------------------------------
# Workspace marks
# ---------------------------------------------------------------------------
# What each workspace mark shows (names and icons are workspace_names and
# workspace_icons in astur.conf):
#   auto       its icon, else its name, else its number (the default)
#   number     always the number
#   name       the name (the number when it has none)
#   icon       the icon (the number when it has none)
#   icon_name  icon and name together
# With anything but auto, a mark widens to fit its label.
workspace_label = auto
# Up to 3 app icons in each workspace mark, for the apps open there (not
# with workspace_style = dot).  bool
workspace_apps = false

# ---------------------------------------------------------------------------
# Click actions
# ---------------------------------------------------------------------------
# <widget>_click, <widget>_right_click and <widget>_middle_click set what a
# click on a widget does, as: action [argument]. Widgets: title layout cpu
# mem net volume battery date clock media keyboard disk uptime custom logo.
# Workspaces, app buttons and media_controls keep their own clicks.
# Actions:
#   none calendar quick_settings mute volume_mixer sound_settings
#   network_settings bluetooth_settings power_settings datetime_settings
#   language_settings next_input storage_settings display_settings
#   windows_settings astur_settings task_manager open_drive
#   media_play_pause media_next media_prev launcher system_menu
#   workspace_next workspace_prev next_layout
#   switch_workspace N        (go to workspace N)
#   launch COMMAND            (as extra_hotkeys' launch)
# Without a line, a click does this (left / right; middle does nothing):
#   clock, date   calendar / datetime_settings
#   volume        mute / sound_settings
#   battery       quick_settings / power_settings
#   net           quick_settings / network_settings
#   cpu, mem      task_manager / none
#   disk          open_drive / storage_settings
#   keyboard      next_input / language_settings
#   media         media_play_pause / media_next
#   logo          system_menu / astur_settings
#   others        none / none
# Examples (remove the # to use one):
#   cpu_right_click = launch wt -p Ubuntu
#   clock_click = none

# ---------------------------------------------------------------------------
# Widget toggles
# ---------------------------------------------------------------------------
# Show the focused window title.  bool
show_title = true
# Show the layout name + tiling/floating state.  bool
show_layout = true
# Show the clock.  bool
show_clock = true
# 24-hour clock when clock_format is empty; false = 12-hour with am/pm.  bool
clock_24h = true
# Show the date.  bool
show_date = false
# Date format tokens:  yyyy yy  MMMM (June) MMM (Jun) MM  dddd (Friday)
# ddd (Fri) dd  d (day without a leading zero, when no letter touches it)
# (e.g. \"ddd dd MMM\" -> Fri 19 Jun, \"dddd d MMMM\" -> Friday 19 June)
date_format = ddd dd MMM
# Live system stats. Updated every ~2s.  bool
show_cpu = true
show_mem = true
show_battery = true
# Network down/up speed (updated every ~2s).  bool
show_net = false
# Speaker volume %. Wheel over it adjusts; click toggles mute.  bool
show_volume = true
# App buttons: an icon per window on the active workspace; click focuses.  bool
show_apps = false
# Current media title. Requires media_enabled in astur.conf.
show_media = false

# ---------------------------------------------------------------------------
# Spacing, labels and icons
# ---------------------------------------------------------------------------
widget_gap = 16
icon_size = 20
workspace_width = 34
# How cpu / mem / net / volume / battery show:
#   icon   label only        text   value only        both   label + value
#   glyph  a Windows icon + value: Segoe Fluent Icons (Windows 11), else
#          Segoe MDL2 Assets. Battery shows its level (and charging), volume
#          its level (and mute), net wifi or ethernet. MDL2 has no cpu or
#          mem icon, so those show their label (below) there instead.
#   glyph_only  the Windows icon alone, no value (disk too); a widget
#          without an icon shows its label
icon_mode = both
show_tooltips = true
show_app_labels = false
cpu_format = {value}%
mem_format = {value}%
battery_format = {value}%
net_format = D:{down} U:{up}
volume_format = {value}%
# Clock tokens: HH H (24-hour)  hh h (12-hour)  mm  ss (seconds: the bar
# then repaints every second)  tt (am/pm)  TT (AM/PM). Empty = clock_24h
# decides between HH:mm and h:mm tt.
clock_format = HH:mm
# Labels for icon / both. Plain text works everywhere; your own icon-font
# glyphs work when font_name has them.
icon_cpu = CPU
icon_mem = RAM
icon_battery = BAT
icon_net = NET
icon_volume = VOL
icon_disk = DISK

# ---------------------------------------------------------------------------
# More widgets (listed in a zone = shown)
# ---------------------------------------------------------------------------
# Title text. Tokens: {title} (the window's title), {app} (the app's name,
# e.g. Microsoft Edge).
title_format = {title}
# Keyboard language. Tokens: {lang} (ENG, DEU, ...) {region} (US, GB, ...).
keyboard_format = {lang}
# The drive (or folder) the disk widget measures, checked every 30 s:
# C, C: or a folder path.
disk_drive = C:
# Disk tokens: {free} {used} {total} {value} (percent used).
disk_format = {free}
# Uptime token: {value} (42m, 5h 12m, 3d 4h).
uptime_format = {value}
# custom: a command run hidden (as cmd /C) every custom_interval seconds
# (1 - 3600); its first output line is {output}. Stopped after 5 s. Empty =
# custom_format alone, as plain text.
custom_command =
custom_format = {output}
custom_interval = 10

# Colour palette the 'auto' colours come from: classic (the default),
# catppuccin, nord, gruvbox, rosepine, mono, or system (Windows neutrals and
# your Windows accent colour, read at start and on reload). Each has a dark
# and a light variant; astur.conf 'theme' picks which.
palette = classic
# Colours: 'auto' follows the palette and theme, or set an explicit #RRGGBB to
# override that colour permanently.
bg = auto
fg = auto
# Active-workspace pill highlight.
accent = auto
# Empty workspaces / layout / stats text.
inactive = auto
# Text on the accent highlight. auto = the palette's, or bg while bg is set.
accent_fg = auto
# Island and pill fill.
surface = auto
# Outline colour (see border_width). auto = the inactive colour.
border = auto
";

pub fn parse_bool(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "1" | "on")
}

/// Parse a comma/semicolon-separated list of window-class names, trimmed.
fn parse_list(v: &str) -> Vec<String> {
    v.split([',', ';'])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Ordered slots preserve empty positions. `;;` is preferred because Windows
/// paths may contain commas; comma remains accepted for older configs.
fn parse_slots(v: &str) -> Vec<String> {
    let values: Vec<String> = if v.contains(";;") {
        v.split(";;").map(|s| s.trim().to_string()).collect()
    } else {
        v.split(',').map(|s| s.trim().to_string()).collect()
    };
    if values.iter().all(String::is_empty) {
        Vec::new()
    } else {
        values
    }
}

/// Split declarative records. `;;` separates records and `|` separates fields.
/// `\|` and `\;` embed literal separators; other backslashes stay untouched so
/// Windows paths round-trip without doubled escaping.
fn records(v: &str) -> std::vec::IntoIter<Vec<String>> {
    let chars: Vec<char> = v.chars().collect();
    let mut out = Vec::new();
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut i = 0usize;
    while i < chars.len() {
        let ch = chars[i];
        // `\\` before a separator is one escaped trailing backslash. Skip
        // whitespace first: format_* joins records with " ;; ", and without
        // the skip a field ending in `\` grew one backslash per save.
        if ch == '\\' && chars.get(i + 1) == Some(&'\\') {
            let mut j = i + 2;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j == chars.len()
                || chars[j] == '|'
                || (chars[j] == ';' && chars.get(j + 1) == Some(&';'))
            {
                field.push('\\');
                i += 2;
                continue;
            }
        }
        if ch == '\\' && i + 1 < chars.len() && matches!(chars[i + 1], '|' | ';') {
            field.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if ch == '|' {
            fields.push(field.trim().to_string());
            field.clear();
            i += 1;
            continue;
        }
        if ch == ';' && i + 1 < chars.len() && chars[i + 1] == ';' {
            fields.push(field.trim().to_string());
            field.clear();
            if fields.iter().any(|value| !value.is_empty()) {
                out.push(std::mem::take(&mut fields));
            } else {
                fields.clear();
            }
            i += 2;
            continue;
        }
        field.push(ch);
        i += 1;
    }
    fields.push(field.trim().to_string());
    if fields.iter().any(|value| !value.is_empty()) {
        out.push(fields);
    }
    out.into_iter()
}

fn escape_field(value: &str) -> String {
    let mut escaped = value.replace('|', "\\|").replace(';', "\\;");
    if escaped.ends_with('\\') {
        escaped.push('\\');
    }
    escaped
}

pub fn parse_launcher_entries(v: &str) -> Vec<LauncherEntry> {
    records(v)
        .filter_map(|f| {
            let label = f.first()?.clone();
            let target = f.get(1)?.clone();
            (!label.is_empty() && !target.is_empty()).then(|| LauncherEntry {
                label,
                target,
                icon: f.get(2).cloned().unwrap_or_else(|| "auto".to_string()),
            })
        })
        .collect()
}

pub fn format_launcher_entries(v: &[LauncherEntry]) -> String {
    v.iter()
        .map(|e| {
            format!(
                "{}|{}|{}",
                escape_field(&e.label),
                escape_field(&e.target),
                escape_field(&e.icon)
            )
        })
        .collect::<Vec<_>>()
        .join(" ;; ")
}

pub fn parse_system_actions(v: &str) -> Vec<SystemAction> {
    records(v)
        .filter_map(|f| {
            let category = f.first()?.clone();
            let category = if category.is_empty() {
                "Custom".to_string()
            } else {
                category
            };
            let label = f.get(1)?.clone();
            let target = f.get(2)?.clone();
            (!label.is_empty() && !target.is_empty()).then(|| SystemAction {
                category,
                label,
                target,
                icon: f.get(3).cloned().unwrap_or_else(|| "command".to_string()),
                confirm: f.get(4).is_some_and(|v| parse_bool(v)),
            })
        })
        .collect()
}

pub fn format_system_actions(v: &[SystemAction]) -> String {
    v.iter()
        .map(|e| {
            format!(
                "{}|{}|{}|{}|{}",
                escape_field(&e.category),
                escape_field(&e.label),
                escape_field(&e.target),
                escape_field(&e.icon),
                e.confirm
            )
        })
        .collect::<Vec<_>>()
        .join(" ;; ")
}

pub fn parse_window_rules(v: &str) -> Vec<WindowRule> {
    records(v)
        .filter_map(|f| {
            let action = f.first()?.to_ascii_lowercase();
            if !matches!(action.as_str(), "tile" | "float" | "ignore") {
                return None;
            }
            let one_based = |s: Option<&String>| {
                s.and_then(|v| v.parse::<usize>().ok())
                    .and_then(|v| v.checked_sub(1))
            };
            Some(WindowRule {
                action,
                exe: f.get(1).cloned().unwrap_or_default(),
                class: f.get(2).cloned().unwrap_or_default(),
                title: f.get(3).cloned().unwrap_or_default(),
                workspace: one_based(f.get(4)),
                monitor: one_based(f.get(5)),
            })
        })
        .collect()
}

pub fn format_window_rules(v: &[WindowRule]) -> String {
    v.iter()
        .map(|r| {
            format!(
                "{}|{}|{}|{}|{}|{}",
                escape_field(&r.action),
                escape_field(&r.exe),
                escape_field(&r.class),
                escape_field(&r.title),
                r.workspace.map(|n| (n + 1).to_string()).unwrap_or_default(),
                r.monitor.map(|n| (n + 1).to_string()).unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>()
        .join(" ;; ")
}

pub fn parse_hotkeys(v: &str) -> Vec<HotkeyDef> {
    records(v)
        .filter_map(|f| {
            let chord = f.first()?.to_ascii_uppercase().replace(' ', "");
            let action = f.get(1)?.to_ascii_lowercase();
            (!chord.is_empty() && !action.is_empty()).then(|| HotkeyDef {
                chord,
                action,
                argument: f.get(2).cloned().unwrap_or_default(),
            })
        })
        .collect()
}

pub fn format_hotkeys(v: &[HotkeyDef]) -> String {
    v.iter()
        .map(|h| {
            format!(
                "{}|{}|{}",
                escape_field(&h.chord),
                escape_field(&h.action),
                escape_field(&h.argument)
            )
        })
        .collect::<Vec<_>>()
        .join(" ;; ")
}
/// Key names beyond letters, digits and F1-F24, as (name, VK). The first name
/// of a VK is the one `vk_to_key` writes. `;` `,` and `\` are written as
/// words: `;;` and `\` are extra_hotkeys syntax, `,` separates workspace_keys.
const KEY_NAMES: &[(&str, u32)] = &[
    ("Space", 0x20),
    ("Tab", 0x09),
    ("Enter", 0x0D),
    ("Return", 0x0D),
    ("Backspace", 0x08),
    ("Back", 0x08),
    ("Esc", 0x1B),
    ("Escape", 0x1B),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("Home", 0x24),
    ("End", 0x23),
    ("PgUp", 0x21),
    ("PageUp", 0x21),
    ("PgDn", 0x22),
    ("PageDown", 0x22),
    ("Insert", 0x2D),
    ("Ins", 0x2D),
    ("Delete", 0x2E),
    ("Del", 0x2E),
    ("[", 0xDB),
    ("LBRACKET", 0xDB),
    ("OEM_4", 0xDB),
    ("]", 0xDD),
    ("RBRACKET", 0xDD),
    ("OEM_6", 0xDD),
    ("`", 0xC0),
    ("GRAVE", 0xC0),
    ("BACKTICK", 0xC0),
    ("OEM_3", 0xC0),
    ("-", 0xBD),
    ("MINUS", 0xBD),
    ("OEM_MINUS", 0xBD),
    ("=", 0xBB),
    ("EQUAL", 0xBB),
    ("OEM_PLUS", 0xBB),
    ("Semicolon", 0xBA),
    (";", 0xBA),
    ("Quote", 0xDE),
    ("'", 0xDE),
    ("Comma", 0xBC),
    (",", 0xBC),
    ("Period", 0xBE),
    (".", 0xBE),
    ("Slash", 0xBF),
    ("/", 0xBF),
    ("Backslash", 0xDC),
    ("\\", 0xDC),
];

/// Map a key name ("1", "Q", "F3", "Space", "Left", "[") to its Win32
/// virtual-key code, ignoring case.
///
/// Letters, digits, F-keys and the named keys are the same VK on every
/// Windows layout. The punctuation names are not: they bind the Windows
/// VK_OEM_* code, which is that glyph's key only on a US layout. On German,
/// French and similar layouts the same VK sits on a different physical key
/// (VK_OEM_4 is the German `ß` key), so the bind fires there, not on the key
/// that prints the glyph.
pub fn key_to_vk(name: &str) -> Option<u32> {
    let n = name.trim().to_ascii_uppercase();
    let b = n.as_bytes();
    if b.len() == 1 {
        let c = b[0];
        if c.is_ascii_digit() || c.is_ascii_uppercase() {
            return Some(c as u32); // ASCII '0'-'9'/'A'-'Z' == their VK codes
        }
    }
    if let Some(num) = n.strip_prefix('F') {
        if let Ok(k) = num.parse::<u32>() {
            if (1..=24).contains(&k) {
                return Some(0x70 + k - 1); // VK_F1 = 0x70
            }
        }
    }
    KEY_NAMES
        .iter()
        .find(|k| k.0.eq_ignore_ascii_case(&n))
        .map(|k| k.1)
}

/// Parse a space/comma-separated navbar zone list, keeping only known widget
/// names (typos vanish rather than erroring, matching the rest of the parser).
fn parse_widgets(v: &str) -> Vec<String> {
    v.split([',', ' ', '\t'])
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| BAR_WIDGETS.contains(&s.as_str()))
        .collect()
}

/// Parse `widget_colors`: space/comma-separated `widget:colour` pairs, kept
/// lower-cased. Text widgets only (workspaces and apps draw their own marks
/// and icons on the surface); a colour is a swatch name, `surface` or #hex.
/// Anything else is dropped, as `parse_widgets` drops a typo.
fn parse_widget_colors(v: &str) -> Vec<(String, String)> {
    // `cpu: orange` (a YAML habit) splits into `cpu:` and `orange`, both
    // dropped, and a later GUI edit then erased them from the file. Glue a
    // token ending in ':' (or a next one starting with ':') back on, unless
    // that makes two colons: `volume: date:pink` stays two entries.
    let mut toks: Vec<String> = Vec::new();
    for t in v.split([',', ' ', '\t']).filter(|t| !t.is_empty()) {
        match toks.last_mut() {
            Some(l)
                if (l.ends_with(':') || t.starts_with(':'))
                    && l.matches(':').count() + t.matches(':').count() == 1 =>
            {
                l.push_str(t)
            }
            _ => toks.push(t.to_string()),
        }
    }
    toks.iter()
        .filter_map(|t| {
            let (w, c) = t.split_once(':')?;
            let (w, c) = (w.trim().to_ascii_lowercase(), c.trim().to_ascii_lowercase());
            let text_widget = BAR_WIDGETS.contains(&w.as_str())
                && !matches!(
                    w.as_str(),
                    "workspaces" | "apps" | "separator" | "spacer" | "logo"
                );
            let colour = c == "surface"
                || SWATCH_NAMES.contains(&c.as_str())
                || parse_color(&c, NO_LEGACY) != NO_LEGACY;
            (text_widget && colour).then_some((w, c))
        })
        .collect()
}

/// Parse a space/comma-separated list of fractions, each clamped to `lo..=hi`.
/// Invalid tokens are dropped, and a list with nothing valid gives `default`:
/// an empty preset cycle would leave the cycle keys with nothing to step to.
fn parse_fractions(v: &str, lo: f32, hi: f32, default: &[f32]) -> Vec<f32> {
    let l: Vec<f32> = v
        .split([',', ' ', '\t'])
        .filter_map(|s| s.trim().parse::<f32>().ok())
        .filter(|n| !n.is_nan())
        .map(|n| n.clamp(lo, hi))
        .collect();
    if l.is_empty() {
        default.to_vec()
    } else {
        l
    }
}

/// Parse a space/comma-separated list of key names into VK codes.
fn parse_keys(v: &str) -> Vec<u32> {
    v.split([',', ' ', '\t'])
        .filter_map(|s| {
            let s = s.trim();
            if s.is_empty() {
                None
            } else {
                key_to_vk(s)
            }
        })
        .collect()
}

/// Themeable colour: `#RRGGBB` = custom, `auto`/`theme`/empty = follow the
/// theme presets. The key's OLD shipped dark default also maps to auto — every
/// pre-theme config file spelled the defaults out literally, and treating them
/// as customised froze those bars in dark forever (the "navbar doesn't update
/// to light mode" bug). Malformed input keeps the previous value.
fn parse_theme_color(v: &str, legacy_default: u32, prev: Option<u32>) -> Option<u32> {
    let t = v.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("auto") || t.eq_ignore_ascii_case("theme") {
        return None;
    }
    const SENTINEL: u32 = 0xFFFF_FFFF;
    let c = parse_color(t, SENTINEL);
    if c == SENTINEL {
        return prev;
    }
    if c == legacy_default {
        None
    } else {
        Some(c)
    }
}

/// `legacy_default` for a themed colour key that never shipped a hex default.
/// No #RRGGBB parses to it.
const NO_LEGACY: u32 = u32::MAX;

/// A colour the user pinned, made to survive a save: the parser reads a key's
/// legacy default hex as `auto` (above), so pinning exactly that colour would
/// silently flip back to Auto. One step of red is invisible and stays pinned.
pub fn pin_theme_color(c: u32, legacy_default: u32) -> u32 {
    if c == legacy_default {
        c ^ 1
    } else {
        c
    }
}

/// Parse a #RRGGBB hex string into a Win32 COLORREF (0x00BBGGRR). Falls back to
/// `fallback` on any malformed input.
fn parse_color(v: &str, fallback: u32) -> u32 {
    let s = v.trim().trim_start_matches('#');
    if s.len() == 6 {
        if let Ok(rgb) = u32::from_str_radix(s, 16) {
            let r = (rgb >> 16) & 0xFF;
            let g = (rgb >> 8) & 0xFF;
            let b = rgb & 0xFF;
            return (b << 16) | (g << 8) | r;
        }
    }
    fallback
}

/// Resolve a config file path: env override, else %USERPROFILE%\.astur\<name>.
pub fn config_path(env: &str, name: &str) -> std::path::PathBuf {
    if let Ok(p) = std::env::var(env) {
        return std::path::PathBuf::from(p);
    }
    let mut dir = std::env::var("USERPROFILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    dir.push(".astur");
    dir.push(name);
    dir
}

/// A config file that exists (or may exist) but could not be read. The file
/// has been left exactly as it was; the caller decides what to run on.
#[derive(Debug)]
pub struct ConfigReadError {
    pub path: std::path::PathBuf,
    pub error: std::io::Error,
}

impl std::fmt::Display for ConfigReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cannot read {}: {}", self.path.display(), self.error)
    }
}

/// Read a config file: `Ok(None)` only when it is genuinely absent (`NotFound`).
///
/// Any other failure — a sharing violation while an editor is mid-save, invalid
/// UTF-8, a UTF-16 "Unicode" save from Notepad — means the user's file IS there,
/// and writing over it would silently destroy it. That is exactly what the old
/// `Err(_) => write(default)` arm did, on every such read, startup included, and
/// what the settings GUI's template fallback did on Save. Both read through here.
pub fn read_existing(path: &std::path::Path) -> Result<Option<String>, ConfigReadError> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ConfigReadError {
            path: path.to_path_buf(),
            error,
        }),
    }
}

/// Read a config file, writing `default` the first time it is missing (and
/// only then: see `read_existing`).
fn read_or_create(path: &std::path::Path, default: &str) -> Result<String, ConfigReadError> {
    if let Some(t) = read_existing(path)? {
        return Ok(t);
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, default);
    println!("wrote default config: {}", path.display());
    Ok(default.to_string())
}

/// Load settings from astur.conf (window manager) and navbar.conf (status
/// bar), creating each with documented defaults when missing. A file that
/// exists but cannot be read is an error, never a reason to rewrite it.
pub fn load_config() -> Result<Config, ConfigReadError> {
    let mut c = Config::defaults();
    let wm = config_path("ASTUR_CONFIG", "astur.conf");
    parse_into_from(&mut c, &read_or_create(&wm, DEFAULT_CONFIG)?, "astur.conf");
    let nav = config_path("ASTUR_NAVBAR", "navbar.conf");
    parse_into_from(
        &mut c,
        &read_or_create(&nav, DEFAULT_NAVBAR)?,
        "navbar.conf",
    );
    Ok(c)
}

/// The settings a freshly written pair of default files would give, built in
/// memory and written nowhere. For a caller that could not read the real files.
pub fn default_config() -> Config {
    parse_pair(DEFAULT_CONFIG, DEFAULT_NAVBAR)
}

/// Apply `key = value` lines from `text` onto `c`. Unknown keys are ignored.
/// Recognises both the window-manager keys (astur.conf) and the navbar keys
/// (navbar.conf, unprefixed) so either file may set either, and old configs that
/// used the `bar_*` names keep working. Public for the settings GUI, which
/// stamps a bar preset's pairs onto the config being edited exactly as a
/// file read after them would.
pub fn parse_into(c: &mut Config, text: &str) {
    parse_into_from(c, text, "")
}

/// `parse_into` with a source label, so an unknown key can say which file it
/// came from.
fn parse_into_from(c: &mut Config, text: &str, source: &str) {
    // A UTF-8 BOM (Notepad's "UTF-8 with BOM") is not whitespace to `trim`, so
    // it stuck to the first key and that line was silently ignored.
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        match k {
            // ---- window manager (astur.conf) ----
            "workspace_mode" => c.per_monitor = v.eq_ignore_ascii_case("per_monitor"),
            "start_tiled" => c.start_tiled = parse_bool(v),
            "outer_gap" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.outer_gap = n.clamp(0, 500);
                }
            }
            "inner_gap" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.inner_gap = n.clamp(0, 500);
                }
            }
            "master_ratio" => {
                if let Ok(n) = v.parse::<f32>() {
                    c.master_ratio = n.clamp(0.1, 0.9);
                }
            }
            "workspaces" => {
                if let Ok(n) = v.parse::<usize>() {
                    c.workspaces = n.clamp(1, MAX_WORKSPACES);
                }
            }
            "workspace_keys" => {
                let keys = parse_keys(v);
                if !keys.is_empty() {
                    c.workspace_keys = keys;
                }
            }
            "workspace_names" => c.workspace_names = parse_list(v),
            "workspace_icons" => c.workspace_icons = parse_list(v),
            "workspace_wallpapers" => c.workspace_wallpapers = parse_slots(v),
            "layout" => {
                let m = v.trim().to_ascii_lowercase();
                if LAYOUTS.contains(&m.as_str()) {
                    c.layout = m;
                }
            }
            "monitor_layouts" => {
                // Positional: blanks must survive so "scrolling, , master"
                // still means monitor 3. `default` and typos read as blank
                // (inherit `layout`) rather than shifting later entries.
                let l: Vec<String> = v
                    .split(',')
                    .map(|s| {
                        let s = s.trim().to_ascii_lowercase();
                        if LAYOUTS.contains(&s.as_str()) {
                            s
                        } else {
                            String::new()
                        }
                    })
                    .collect();
                c.monitor_layouts = if l.iter().all(String::is_empty) {
                    Vec::new()
                } else {
                    l
                };
            }
            "scroll_default_width" => {
                if let Ok(n) = v.parse::<f32>() {
                    if !n.is_nan() {
                        c.scroll_default_width = n.clamp(0.1, 1.0);
                    }
                }
            }
            "scroll_width_presets" => {
                c.scroll_width_presets = parse_fractions(v, 0.1, 1.0, &SCROLL_WIDTHS)
            }
            "scroll_height_presets" => {
                c.scroll_height_presets = parse_fractions(v, 0.1, 0.9, &SCROLL_HEIGHTS)
            }
            "scroll_center" => {
                let m = v.trim().to_ascii_lowercase();
                if matches!(m.as_str(), "never" | "always" | "on-overflow") {
                    c.scroll_center = m;
                } else if m == "on_overflow" {
                    c.scroll_center = "on-overflow".to_string(); // underscore spelling
                }
            }
            "tab_bar_height" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.tab_bar_height = n.clamp(16, 64);
                }
            }
            "terminal" => c.terminal = v.to_string(),
            "browser" => c.browser = v.to_string(),
            "unfocused_opacity" => {
                if let Ok(n) = v.parse::<f32>() {
                    c.unfocused_opacity = n.clamp(0.1, 1.0);
                }
            }
            "border_enabled" => c.border_enabled = parse_bool(v),
            "focused_border" => c.focused_border = parse_color(v, c.focused_border),
            "unfocused_border" => c.unfocused_border = parse_color(v, c.unfocused_border),
            "cursor_follows_focus" => c.cursor_follows_focus = parse_bool(v),
            "focus_follows_mouse" => c.focus_follows_mouse = parse_bool(v),
            "animations" => c.animations = parse_bool(v),
            "animation_ms" | "animation_speed" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.animation_ms = n.clamp(0, 2000);
                }
            }
            "workspace_slide" => c.workspace_slide = parse_bool(v),
            "workspace_anim" => {
                let m = v.trim().to_ascii_lowercase();
                if matches!(m.as_str(), "off" | "slide" | "spring" | "fade") {
                    c.workspace_anim = m;
                }
            }
            "window_anim" => {
                let m = v.trim().to_ascii_lowercase();
                if matches!(m.as_str(), "off" | "glide" | "spring") {
                    c.window_anim = m;
                }
            }
            "animation_easing" => {
                let m = v.trim().to_ascii_lowercase();
                if matches!(m.as_str(), "cubic" | "smooth" | "spring") {
                    c.animation_easing = m;
                }
            }
            "popup_font_name" => {
                if !v.is_empty() {
                    c.popup_font_name = v.to_string()
                }
            }
            "popup_font_size" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.popup_font_size = n.clamp(10, 72)
                }
            }
            "popup_font_weight" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.popup_font_weight = n.clamp(100, 900)
                }
            }
            "popup_radius" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.popup_radius = n.clamp(0, 48)
                }
            }
            "popup_border_width" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.popup_border_width = n.clamp(0, 8)
                }
            }
            "popup_opacity" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.popup_opacity = n.clamp(20, 100)
                }
            }
            "popup_bg" => c.popup_bg = parse_theme_color(v, 0x0016_1616, c.popup_bg),
            "popup_fg" => c.popup_fg = parse_theme_color(v, 0x00E6_E6E6, c.popup_fg),
            "popup_muted" => c.popup_muted = parse_theme_color(v, 0x0089_8989, c.popup_muted),
            "popup_accent" => c.popup_accent = parse_theme_color(v, 0x0082_6333, c.popup_accent),
            "popup_accent_fg" => {
                c.popup_accent_fg = parse_theme_color(v, 0x00FF_FFFF, c.popup_accent_fg)
            }
            "popup_border" => c.popup_border = parse_theme_color(v, 0x0033_2A26, c.popup_border),
            "launcher_enabled" => c.launcher_enabled = parse_bool(v),
            "launcher_width" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.launcher_width = n.clamp(320, 2400)
                }
            }
            "launcher_wide_width" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.launcher_wide_width = n.clamp(480, 3200)
                }
            }
            "launcher_height" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.launcher_height = n.clamp(200, 1800)
                }
            }
            "launcher_row_height" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.launcher_row_height = n.clamp(24, 96)
                }
            }
            "launcher_icon_size" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.launcher_icon_size = n.clamp(12, 72)
                }
            }
            "launcher_padding" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.launcher_padding = n.clamp(4, 80)
                }
            }
            "launcher_selection_radius" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.launcher_selection_radius = n.clamp(0, 48)
                }
            }
            "launcher_placement" => {
                let m = v.trim().to_ascii_lowercase();
                if matches!(
                    m.as_str(),
                    "cursor_monitor" | "focused_monitor" | "primary_monitor"
                ) {
                    c.launcher_placement = m
                }
            }
            "launcher_source_apps" => c.launcher_source_apps = parse_bool(v),
            "launcher_source_files" => c.launcher_source_files = parse_bool(v),
            "launcher_source_calc" => c.launcher_source_calc = parse_bool(v),
            "launcher_source_web" => c.launcher_source_web = parse_bool(v),
            "launcher_source_windows" => c.launcher_source_windows = parse_bool(v),
            "launcher_source_clipboard" => c.launcher_source_clipboard = parse_bool(v),
            "launcher_source_emoji" => c.launcher_source_emoji = parse_bool(v),
            "launcher_web_url" => {
                if v.contains("{query}") {
                    c.launcher_web_url = v.to_string()
                }
            }
            "launcher_max_results" => {
                if let Ok(n) = v.parse::<usize>() {
                    c.launcher_max_results = n.clamp(5, 500)
                }
            }
            "launcher_file_scope" => c.launcher_file_scope = v.to_string(),
            "launcher_file_exclude" => c.launcher_file_exclude = parse_list(v),
            "launcher_mru" => c.launcher_mru = parse_bool(v),
            "launcher_entries" => c.launcher_entries = parse_launcher_entries(v),
            "system_menu_enabled" => c.system_menu_enabled = parse_bool(v),
            "system_menu_width" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.system_menu_width = n.clamp(280, 1200)
                }
            }
            "system_power_items" => c.system_power_items = parse_list(v),
            "system_setup_items" => c.system_setup_items = parse_list(v),
            "system_actions" => c.system_actions = parse_system_actions(v),
            "alt_tab_replacement" => c.alt_tab_replacement = parse_bool(v),
            "scratchpad_enabled" => c.scratchpad_enabled = parse_bool(v),
            "scratchpad_command" => c.scratchpad_command = v.to_string(),
            "scratchpad_class" => c.scratchpad_class = v.to_string(),
            "clipboard_history" => c.clipboard_history = parse_bool(v),
            "clipboard_limit" => {
                if let Ok(n) = v.parse::<usize>() {
                    c.clipboard_limit = n.clamp(1, 500)
                }
            }
            "clipboard_prefix" => {
                if !v.is_empty() {
                    c.clipboard_prefix = v.to_string()
                }
            }
            "emoji_picker" => c.emoji_picker = parse_bool(v),
            "emoji_prefix" => {
                if !v.is_empty() {
                    c.emoji_prefix = v.to_string()
                }
            }
            "wallpaper_dir" => c.wallpaper_dir = v.to_string(),
            "media_enabled" => c.media_enabled = parse_bool(v),
            "ipc_enabled" => c.ipc_enabled = parse_bool(v),
            "yasb_enabled" => c.yasb_enabled = parse_bool(v),
            "yasb_port" => {
                if let Ok(port @ 1..=65535) = v.parse::<u16>() {
                    c.yasb_port = port;
                }
            }
            "ipc_pipe" => {
                if !v.is_empty() {
                    c.ipc_pipe = v.to_string()
                }
            }
            "persist_state" => c.persist_state = parse_bool(v),
            "ipc_allow_launch" => c.ipc_allow_launch = parse_bool(v),
            "foreground_lock_disable" => c.foreground_lock_disable = parse_bool(v),
            "async_window_pos" => c.async_window_pos = parse_bool(v),
            "log_level" => {
                if matches!(v, "off" | "error" | "info" | "debug") {
                    c.log_level = v.to_string()
                }
            }
            "extra_hotkeys" => c.extra_hotkeys = parse_hotkeys(v),
            "window_rules" => c.window_rules = parse_window_rules(v),
            "ignore_classes" => c.ignore_classes = parse_list(v),
            "float_classes" => c.float_classes = parse_list(v),
            "key_focus_next" => {
                if let Some(k) = key_to_vk(v) {
                    c.key_focus_next = k;
                }
            }
            "key_focus_prev" => {
                if let Some(k) = key_to_vk(v) {
                    c.key_focus_prev = k;
                }
            }
            "key_shrink_master" => {
                if let Some(k) = key_to_vk(v) {
                    c.key_shrink_master = k;
                }
            }
            "key_grow_master" => {
                if let Some(k) = key_to_vk(v) {
                    c.key_grow_master = k;
                }
            }
            "key_promote_master" => {
                if let Some(k) = key_to_vk(v) {
                    c.key_promote_master = k;
                }
            }
            "key_toggle_tiling" => {
                if let Some(k) = key_to_vk(v) {
                    c.key_toggle_tiling = k;
                }
            }
            "key_toggle_float" => {
                if let Some(k) = key_to_vk(v) {
                    c.key_toggle_float = k;
                }
            }
            "key_close_window" => {
                if let Some(k) = key_to_vk(v) {
                    c.key_close_window = k;
                }
            }
            "key_cycle_width" => {
                if let Some(k) = key_to_vk(v) {
                    c.key_cycle_width = k;
                }
            }
            "key_center_column" => {
                if let Some(k) = key_to_vk(v) {
                    c.key_center_column = k;
                }
            }
            "key_consume_left" => {
                if let Some(k) = key_to_vk(v) {
                    c.key_consume_left = k;
                }
            }
            "key_consume_right" => {
                if let Some(k) = key_to_vk(v) {
                    c.key_consume_right = k;
                }
            }
            "main_key" | "mod" => {
                if let Some(k) = MainKey::from_name(v) {
                    c.main_key = k;
                }
            }
            // ---- navbar (navbar.conf, unprefixed) and legacy bar_* aliases ----
            "enabled" | "bar_enabled" => c.bar_enabled = parse_bool(v),
            "height" | "bar_height" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.bar_height = n.clamp(0, 200);
                }
            }
            "bottom" | "bar_bottom" => c.bar_bottom = parse_bool(v),
            "font_size" | "bar_font_size" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.bar_font_size = n.clamp(0, 100);
                }
            }
            "show_title" | "bar_show_title" => c.bar_show_title = parse_bool(v),
            "show_clock" | "bar_show_clock" => c.bar_show_clock = parse_bool(v),
            "clock_24h" | "bar_clock_24h" => c.bar_clock_24h = parse_bool(v),
            "show_layout" | "bar_show_layout" => c.bar_show_layout = parse_bool(v),
            "bg" | "bar_bg" => c.bar_bg = parse_theme_color(v, BAR_DARK[0], c.bar_bg),
            "fg" | "bar_fg" => c.bar_fg = parse_theme_color(v, BAR_DARK[1], c.bar_fg),
            "accent" | "bar_accent" => {
                c.bar_accent = parse_theme_color(v, BAR_DARK[2], c.bar_accent)
            }
            "inactive" | "bar_inactive" => {
                c.bar_inactive = parse_theme_color(v, BAR_DARK[3], c.bar_inactive)
            }
            "font_name" | "bar_font_name" => {
                if !v.is_empty() {
                    c.bar_font_name = v.to_string();
                }
            }
            "hide_empty" | "hide_empty_workspaces" | "bar_hide_empty" => {
                c.bar_hide_empty = parse_bool(v)
            }
            "widget_gap" | "bar_widget_gap" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.bar_widget_gap = n.clamp(0, 100)
                }
            }
            "icon_size" | "bar_icon_size" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.bar_icon_size = n.clamp(8, 64)
                }
            }
            "workspace_width" | "bar_workspace_width" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.bar_workspace_width = n.clamp(18, 120)
                }
            }
            "icon_mode" | "bar_icon_mode" => {
                let m = v.trim().to_ascii_lowercase();
                if matches!(
                    m.as_str(),
                    "icon" | "text" | "both" | "glyph" | "glyph_only"
                ) {
                    c.bar_icon_mode = m
                }
            }
            "show_tooltips" | "bar_show_tooltips" => c.bar_show_tooltips = parse_bool(v),
            "show_app_labels" | "bar_show_app_labels" => c.bar_show_app_labels = parse_bool(v),
            "cpu_format" | "bar_cpu_format" => c.bar_cpu_format = v.to_string(),
            "mem_format" | "bar_mem_format" => c.bar_mem_format = v.to_string(),
            "battery_format" | "bar_battery_format" => c.bar_battery_format = v.to_string(),
            "net_format" | "bar_net_format" => c.bar_net_format = v.to_string(),
            "volume_format" | "bar_volume_format" => c.bar_volume_format = v.to_string(),
            "clock_format" | "bar_clock_format" => c.bar_clock_format = v.to_string(),
            "icon_cpu" | "bar_icon_cpu" => c.bar_icon_cpu = v.to_string(),
            "icon_mem" | "bar_icon_mem" => c.bar_icon_mem = v.to_string(),
            "icon_battery" | "bar_icon_battery" => c.bar_icon_battery = v.to_string(),
            "icon_net" | "bar_icon_net" => c.bar_icon_net = v.to_string(),
            "icon_volume" | "bar_icon_volume" => c.bar_icon_volume = v.to_string(),
            "padding" | "bar_padding" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.bar_padding = n.clamp(0, 200);
                }
            }
            "show_date" | "bar_show_date" => c.bar_show_date = parse_bool(v),
            "date_format" | "bar_date_format" => {
                if !v.is_empty() {
                    c.bar_date_format = v.to_string();
                }
            }
            "show_cpu" | "bar_show_cpu" => c.bar_show_cpu = parse_bool(v),
            "show_mem" | "show_memory" | "bar_show_mem" => c.bar_show_mem = parse_bool(v),
            "show_battery" | "bar_show_battery" => c.bar_show_battery = parse_bool(v),
            "theme" => {
                let m = v.trim().to_ascii_lowercase();
                if matches!(m.as_str(), "dark" | "light" | "auto") {
                    c.theme = m;
                } else if m == "system" {
                    c.theme = "auto".to_string(); // friendly alias
                }
            }
            "acrylic" => c.acrylic = parse_bool(v),
            "floating" | "bar_floating" => c.bar_floating = parse_bool(v),
            "margin" | "bar_margin" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.bar_margin = n.clamp(0, 200);
                }
            }
            "radius" | "bar_radius" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.bar_radius = n.clamp(0, 40);
                }
            }
            "autohide" | "bar_autohide" => c.bar_autohide = parse_bool(v),
            "wheel_workspaces" | "bar_wheel_ws" => c.bar_wheel_ws = parse_bool(v),
            "show_net" | "show_network" | "bar_show_net" => c.bar_show_net = parse_bool(v),
            "show_volume" | "bar_show_volume" => c.bar_show_volume = parse_bool(v),
            "show_apps" | "bar_show_apps" => c.bar_show_apps = parse_bool(v),
            "show_media" | "bar_show_media" => c.bar_show_media = parse_bool(v),
            "left" | "bar_left" => c.bar_left = parse_widgets(v),
            "center" | "centre" | "bar_center" => c.bar_center = parse_widgets(v),
            "right" | "bar_right" => c.bar_right = parse_widgets(v),
            "palette" => {
                let m = v.trim().to_ascii_lowercase();
                if PALETTES.iter().any(|p| p.name == m) {
                    c.bar_palette = m;
                }
            }
            "workspace_style" => {
                let m = v.trim().to_ascii_lowercase();
                if matches!(m.as_str(), "box" | "pill" | "underline" | "dot") {
                    c.bar_workspace_style = m;
                }
            }
            "font_weight" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.bar_font_weight = n.clamp(100, 900);
                }
            }
            // New keys: no shipped default hex to read as auto.
            "surface" => c.bar_surface = parse_theme_color(v, NO_LEGACY, c.bar_surface),
            "accent_fg" => c.bar_accent_fg = parse_theme_color(v, NO_LEGACY, c.bar_accent_fg),
            "border" => c.bar_border = parse_theme_color(v, NO_LEGACY, c.bar_border),
            "border_width" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.bar_border_width = n.clamp(0, 10);
                }
            }
            "shape" => {
                let m = v.trim().to_ascii_lowercase();
                if matches!(m.as_str(), "strip" | "floating" | "islands" | "pills") {
                    c.bar_shape = m;
                }
            }
            "island_padding" => {
                if let Ok(n) = v.parse::<i32>() {
                    c.bar_island_padding = n.clamp(0, 50);
                }
            }
            "widget_colors" => c.bar_widget_colors = parse_widget_colors(v),
            "keyboard_format" => c.bar_keyboard_format = v.to_string(),
            "disk_drive" => {
                if let Some(d) = disk_root(v) {
                    c.bar_disk_drive = d;
                }
            }
            "disk_format" => c.bar_disk_format = v.to_string(),
            "icon_disk" => c.bar_icon_disk = v.to_string(),
            "uptime_format" => c.bar_uptime_format = v.to_string(),
            "custom_command" => c.bar_custom_command = v.to_string(),
            "custom_format" => c.bar_custom_format = v.to_string(),
            "custom_interval" => {
                if let Ok(n) = v.parse::<u32>() {
                    c.bar_custom_interval = n.clamp(1, 3600);
                }
            }
            "workspace_label" => {
                let m = v.trim().to_ascii_lowercase();
                if WORKSPACE_LABELS.contains(&m.as_str()) {
                    c.bar_workspace_label = m;
                }
            }
            "workspace_apps" => c.bar_workspace_apps = parse_bool(v),
            "monitors" | "bar_monitors" => {
                if let Some(m) = parse_monitors(v) {
                    c.bar_monitors = m;
                }
            }
            "separator_style" => {
                let m = v.trim().to_ascii_lowercase();
                if matches!(m.as_str(), "line" | "dot" | "powerline") {
                    c.bar_separator_style = m;
                }
            }
            "hover" => c.bar_hover = parse_bool(v),
            "title_format" => c.bar_title_format = v.to_string(),
            "title_bold" => c.bar_title_bold = parse_bool(v),
            // `<widget>_click` and its _right / _middle forms: a known key
            // whatever the value; an unknown action keeps the previous one,
            // an empty value goes back to the default.
            other if click_key(other).is_some() => {
                if let (Some((w, b)), Some(val)) = (click_key(other), click_value(v)) {
                    c.bar_clicks.retain(|x| !(x.0 == w && x.1 == b));
                    if !val.is_empty() {
                        c.bar_clicks.push((w.to_string(), b, val));
                    }
                }
            }
            other => {
                let label = if source.is_empty() {
                    other.to_string()
                } else {
                    format!("{source}: {other}")
                };
                if !c.unknown_keys.contains(&label) {
                    c.unknown_keys.push(label);
                }
            }
        }
    }
}

/// Rewrite one `key = value` assignment in conf text, preserving every other
/// line (comments and layout included). The first active (non-comment)
/// assignment of `key` is replaced in place; later duplicates are left alone
/// (the parser is last-write-wins, so the GUI also normalises: see
/// `apply_updates`). If the key is missing, `key = value` is appended.
pub fn set_conf_key(text: &str, key: &str, value: &str) -> String {
    // As the parser does: a BOM would hide a first-line key, so the write
    // appended a duplicate. The file is written back as plain UTF-8.
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    // A conf value is one line: a pasted newline would split it, leave an
    // orphan line behind and never read back, so every later write re-diffed.
    let value = value.replace(['\r', '\n'], " ");
    let mut out: Vec<String> = Vec::new();
    let mut replaced = false;
    for line in text.lines() {
        let t = line.trim();
        if !t.is_empty() && !t.starts_with('#') {
            if let Some((k, _)) = t.split_once('=') {
                if k.trim() == key {
                    if replaced {
                        // Drop later duplicates: the parser is last-write-wins,
                        // so a stale duplicate below would silently override the
                        // value we just set.
                        continue;
                    }
                    out.push(format!("{key} = {value}"));
                    replaced = true;
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }
    if !replaced {
        out.push(format!("{key} = {value}"));
    }
    let mut s = out.join("\n");
    if text.ends_with('\n') || !replaced {
        s.push('\n');
    }
    s
}

/// Apply many `(key, value)` updates onto conf text (see `set_conf_key`).
pub fn apply_updates(text: &str, updates: &[(&str, String)]) -> String {
    let mut s = text.to_string();
    for (k, v) in updates {
        s = set_conf_key(&s, k, v);
    }
    s
}

/// The built-in template for astur.conf (used to regenerate a fresh file).
pub fn default_config_text() -> &'static str {
    DEFAULT_CONFIG
}

/// The built-in template for navbar.conf (used to regenerate a fresh file).
pub fn default_navbar_text() -> &'static str {
    DEFAULT_NAVBAR
}

/// Parse arbitrary conf text onto defaults — used by the settings GUI to load
/// one file at a time (WM keys and navbar keys are both recognised).
pub fn parse_text(text: &str) -> Config {
    let mut c = Config::defaults();
    parse_into(&mut c, text);
    c
}

/// Parse both files' text in load order (astur.conf then navbar.conf).
pub fn parse_pair(wm_text: &str, nav_text: &str) -> Config {
    let mut c = Config::defaults();
    parse_into_from(&mut c, wm_text, "astur.conf");
    parse_into_from(&mut c, nav_text, "navbar.conf");
    c
}

/// What a config reload has to redo, by subsystem. Each flag names work the
/// WM does on reload; a key whose only consumer reads it where it is used
/// (hotkeys, popups, commands, launcher filters) sets `live`, which needs
/// nothing beyond storing the new config.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReloadGroups {
    /// Workspaces, layout, gaps and the bar's work-area reservation: the
    /// manager redistributes workspaces, re-reserves the bar and re-tiles.
    pub wm: bool,
    /// Window dimming and borders: every managed window is unstyled and restyled.
    pub style: bool,
    /// Animation settings: whether (and where) the wallpaper cache renders.
    pub anim: bool,
    /// Bar windows, fonts and paint-time statics: the bars are rebuilt.
    pub bar: bool,
    /// The launcher's app list (and its icon size): re-enumerated.
    pub launcher: bool,
    /// Read where used; nothing to redo.
    pub live: bool,
}

impl ReloadGroups {
    /// Everything: an explicit reload, the user's recovery tool for windows
    /// left mis-styled, and the answer for any change not classified here.
    pub fn full() -> Self {
        ReloadGroups {
            wm: true,
            style: true,
            anim: true,
            bar: true,
            launcher: true,
            live: true,
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == ReloadGroups::default()
    }
}

/// The subsystems that differ between `old` and `new`. Pure data, so the WM
/// can diff a reload instead of redoing everything: a colour edit used to
/// unstyle, re-tile and restyle every window (a visible flash) and
/// re-enumerate the launcher. Every field is destructured by name with no
/// `..`, and an unused binding is an error, so a new field that nobody
/// classified fails to compile rather than silently never applying.
#[deny(unused_variables)]
pub fn reload_groups(old: &Config, new: &Config) -> ReloadGroups {
    let Config {
        per_monitor,
        start_tiled,
        outer_gap,
        inner_gap,
        master_ratio,
        workspaces,
        workspace_keys,
        workspace_names,
        workspace_icons,
        layout,
        monitor_layouts,
        scroll_default_width,
        scroll_width_presets,
        scroll_height_presets,
        scroll_center,
        tab_bar_height,
        terminal,
        browser,
        unfocused_opacity,
        border_enabled,
        focused_border,
        unfocused_border,
        cursor_follows_focus,
        focus_follows_mouse,
        animations,
        animation_ms,
        workspace_slide,
        workspace_anim,
        window_anim,
        animation_easing,
        popup_font_name,
        popup_font_size,
        popup_font_weight,
        popup_radius,
        popup_border_width,
        popup_opacity,
        popup_bg,
        popup_fg,
        popup_muted,
        popup_accent,
        popup_accent_fg,
        popup_border,
        launcher_enabled,
        launcher_width,
        launcher_wide_width,
        launcher_height,
        launcher_row_height,
        launcher_icon_size,
        launcher_padding,
        launcher_selection_radius,
        launcher_placement,
        launcher_source_apps,
        launcher_source_files,
        launcher_source_calc,
        launcher_source_web,
        launcher_source_windows,
        launcher_source_clipboard,
        launcher_source_emoji,
        launcher_web_url,
        launcher_max_results,
        launcher_file_scope,
        launcher_file_exclude,
        launcher_mru,
        launcher_entries,
        system_menu_enabled,
        system_menu_width,
        system_power_items,
        system_setup_items,
        system_actions,
        alt_tab_replacement,
        scratchpad_enabled,
        scratchpad_command,
        scratchpad_class,
        clipboard_history,
        clipboard_limit,
        clipboard_prefix,
        emoji_picker,
        emoji_prefix,
        wallpaper_dir,
        workspace_wallpapers,
        media_enabled,
        ipc_enabled,
        ipc_pipe,
        yasb_enabled,
        yasb_port,
        ipc_allow_launch,
        foreground_lock_disable,
        async_window_pos,
        persist_state,
        log_level,
        unknown_keys,
        extra_hotkeys,
        window_rules,
        bar_enabled,
        bar_height,
        bar_bottom,
        bar_font_size,
        bar_show_title,
        bar_show_clock,
        bar_clock_24h,
        bar_show_layout,
        bar_bg,
        bar_fg,
        bar_accent,
        bar_inactive,
        bar_font_name,
        bar_hide_empty,
        bar_widget_gap,
        bar_icon_size,
        bar_workspace_width,
        bar_icon_mode,
        bar_show_tooltips,
        bar_show_app_labels,
        bar_cpu_format,
        bar_mem_format,
        bar_battery_format,
        bar_net_format,
        bar_volume_format,
        bar_clock_format,
        bar_icon_cpu,
        bar_icon_mem,
        bar_icon_battery,
        bar_icon_net,
        bar_icon_volume,
        bar_padding,
        bar_show_date,
        bar_date_format,
        bar_show_cpu,
        bar_show_mem,
        bar_show_battery,
        ignore_classes,
        float_classes,
        key_focus_next,
        key_focus_prev,
        key_shrink_master,
        key_grow_master,
        key_promote_master,
        key_toggle_tiling,
        key_toggle_float,
        key_close_window,
        key_cycle_width,
        key_center_column,
        key_consume_left,
        key_consume_right,
        main_key,
        theme,
        acrylic,
        bar_floating,
        bar_margin,
        bar_radius,
        bar_autohide,
        bar_wheel_ws,
        bar_show_net,
        bar_show_volume,
        bar_show_apps,
        bar_show_media,
        bar_left,
        bar_center,
        bar_right,
        bar_palette,
        bar_workspace_style,
        bar_font_weight,
        bar_surface,
        bar_accent_fg,
        bar_border,
        bar_border_width,
        bar_shape,
        bar_island_padding,
        bar_widget_colors,
        bar_clicks,
        bar_keyboard_format,
        bar_disk_drive,
        bar_disk_format,
        bar_icon_disk,
        bar_uptime_format,
        bar_custom_command,
        bar_custom_format,
        bar_custom_interval,
        bar_workspace_label,
        bar_workspace_apps,
        bar_monitors,
        bar_separator_style,
        bar_hover,
        bar_title_format,
        bar_title_bold,
    } = new;
    let mut g = ReloadGroups::default();
    macro_rules! diff {
        ($group:ident: $($field:ident),+ $(,)?) => {
            $(if *$field != old.$field {
                g.$group = true;
            })+
        };
    }
    // Workspace model and tile geometry. The bar keys here feed reserve_bar
    // (the work area the tiles fill), so they re-tile as well as rebuild.
    diff!(wm: per_monitor, workspaces, outer_gap, inner_gap, master_ratio, layout);
    diff!(wm: monitor_layouts, scroll_default_width, scroll_width_presets);
    diff!(wm: scroll_height_presets, scroll_center, tab_bar_height);
    diff!(wm: bar_enabled, bar_height, bar_bottom, bar_floating, bar_margin, bar_autohide);
    // `shape` decides floating, which changes the reserved height; `monitors`
    // which screens reserve one at all.
    diff!(wm: bar_shape, bar_monitors);
    diff!(style: unfocused_opacity, border_enabled, focused_border, unfocused_border);
    // wp_publish: whether any animation reads the wallpaper cache.
    diff!(anim: animations, animation_ms, workspace_slide, workspace_anim, window_anim);
    // Everything the bar draws. Snapshot fields (colours, formats, zones)
    // would reach it through the manager's next bar update anyway; statics
    // read at paint (padding, cell width, gap, radius, fonts) need the rebuild.
    diff!(bar: bar_enabled, bar_height, bar_bottom, bar_font_size, bar_font_name, bar_padding);
    diff!(bar: bar_floating, bar_margin, bar_radius, bar_autohide, bar_wheel_ws);
    diff!(bar: bar_show_title, bar_show_clock, bar_clock_24h, bar_show_layout, bar_show_date);
    diff!(bar: bar_show_cpu, bar_show_mem, bar_show_battery, bar_show_net, bar_show_volume);
    diff!(bar: bar_show_apps, bar_show_media, media_enabled, bar_left, bar_center, bar_right);
    diff!(bar: bar_bg, bar_fg, bar_accent, bar_inactive, theme, bar_hide_empty);
    diff!(bar: bar_widget_gap, bar_icon_size, bar_workspace_width, bar_icon_mode);
    diff!(bar: bar_show_tooltips, bar_show_app_labels, bar_date_format, bar_clock_format);
    diff!(bar: bar_cpu_format, bar_mem_format, bar_battery_format, bar_net_format);
    diff!(bar: bar_volume_format, bar_icon_cpu, bar_icon_mem, bar_icon_battery);
    diff!(bar: bar_icon_net, bar_icon_volume, workspace_names, workspace_icons);
    diff!(bar: bar_palette, bar_workspace_style, bar_font_weight, bar_surface);
    diff!(bar: bar_accent_fg, bar_border, bar_border_width, bar_shape, bar_island_padding);
    diff!(bar: bar_widget_colors, bar_clicks, bar_keyboard_format, bar_disk_drive);
    diff!(bar: bar_disk_format, bar_icon_disk, bar_uptime_format, bar_custom_command);
    diff!(bar: bar_custom_format, bar_custom_interval, bar_workspace_label);
    diff!(bar: bar_workspace_apps, bar_monitors, bar_separator_style, bar_hover);
    diff!(bar: bar_title_format, bar_title_bold);
    // LA_REFRESH: the enumerated app list, and the size its icons resolve at.
    diff!(launcher: launcher_source_apps, launcher_entries, launcher_icon_size);
    // Read where used: hooks and hotkeys (apply_hook_config), popups and the
    // launcher (UI_CFG / apply_theme), manager commands (mgr.cfg), workers.
    diff!(live: start_tiled, workspace_keys, terminal, browser, cursor_follows_focus);
    diff!(live: focus_follows_mouse, animation_easing, theme, acrylic);
    diff!(live: popup_font_name, popup_font_size, popup_font_weight, popup_radius);
    diff!(live: popup_border_width, popup_opacity, popup_bg, popup_fg, popup_muted);
    diff!(live: popup_accent, popup_accent_fg, popup_border);
    diff!(live: launcher_enabled, launcher_width, launcher_wide_width, launcher_height);
    diff!(live: launcher_row_height, launcher_padding, launcher_selection_radius);
    diff!(live: launcher_placement, launcher_source_files, launcher_source_calc);
    diff!(live: launcher_source_web, launcher_source_windows, launcher_source_clipboard);
    diff!(live: launcher_source_emoji, launcher_web_url, launcher_max_results);
    diff!(live: launcher_file_scope, launcher_file_exclude, launcher_mru);
    diff!(live: system_menu_enabled, system_menu_width, system_power_items);
    diff!(live: system_setup_items, system_actions, alt_tab_replacement);
    diff!(live: scratchpad_enabled, scratchpad_command, scratchpad_class);
    diff!(live: clipboard_history, clipboard_limit, clipboard_prefix, emoji_picker);
    diff!(live: emoji_prefix, wallpaper_dir, workspace_wallpapers, ipc_enabled, ipc_pipe);
    diff!(live: ipc_allow_launch, foreground_lock_disable, async_window_pos);
    diff!(live: yasb_enabled, yasb_port);
    diff!(live: persist_state, log_level, unknown_keys, extra_hotkeys, window_rules);
    diff!(live: ignore_classes, float_classes, key_focus_next, key_focus_prev);
    diff!(live: key_shrink_master, key_grow_master, key_promote_master);
    diff!(live: key_toggle_tiling, key_toggle_float, key_close_window);
    diff!(live: key_cycle_width, key_center_column, key_consume_left, key_consume_right);
    diff!(live: main_key);
    // Fail-safe: a difference none of the above caught (a field compared in
    // the wrong place, a PartialEq quirk) must still apply, so do it all.
    if g.is_empty() && old != new {
        return ReloadGroups::full();
    }
    g
}

/// Format a COLORREF (0x00BBGGRR) back to a `#RRGGBB` config string.
pub fn color_to_hex(c: u32) -> String {
    let r = c & 0xFF;
    let g = (c >> 8) & 0xFF;
    let b = (c >> 16) & 0xFF;
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// Map a VK code back to its config key name ("A".."Z", "0".."9", "F1".."F24",
/// "Space", "Left", "[", ...), or "" for a key with no name. The inverse of
/// `key_to_vk`, so the settings GUI can save any key the parser accepts.
pub fn vk_to_key(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).unwrap_or('?').to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x70 + 1),
        _ => KEY_NAMES
            .iter()
            .find(|k| k.1 == vk)
            .map_or_else(String::new, |k| k.0.to_string()),
    }
}

// ---- Hotkeys: the main key, chords and the WM's hotkey table (pure) ----

/// Chord modifier bits, side-agnostic. The WM's lock-free hotkey table is
/// indexed by these 32 sets x 256 VKs (`Bind::slot`).
pub const MOD_CTRL: u8 = 1;
pub const MOD_SHIFT: u8 = 2;
/// With `main_key = lalt` this is the reserved Left Alt only (Right Alt stays
/// the apps' Alt), so an old `ALT+...` chord means what it always did.
/// Otherwise either Alt, passed through to apps.
pub const MOD_ALT: u8 = 4;
pub const MOD_WIN: u8 = 8;
/// The reserved Caps Lock (`main_key = capslock`), which no app sees.
pub const MOD_HYPER: u8 = 16;

/// The main key: held for every built-in hotkey, the workspace keys, mouse
/// move / resize and Alt+wheel, and spelled `MOD` in extra_hotkeys.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MainKey {
    /// Reserved: the WM swallows it, apps never see Left Alt.
    #[default]
    LAlt,
    /// Passed through; only the bound keys are swallowed.
    Win,
    /// Either Alt, passed through (GlazeWM-style).
    Alt,
    /// Ctrl and Win held together, passed through.
    CtrlWin,
    /// Reserved: Caps Lock becomes a modifier and stops toggling.
    CapsLock,
}

impl MainKey {
    /// Every choice, in the order a picker lists them.
    pub const ALL: [MainKey; 5] = [
        MainKey::LAlt,
        MainKey::Win,
        MainKey::Alt,
        MainKey::CtrlWin,
        MainKey::CapsLock,
    ];

    /// The `main_key` value astur.conf spells it with.
    pub fn name(self) -> &'static str {
        match self {
            MainKey::LAlt => "lalt",
            MainKey::Win => "win",
            MainKey::Alt => "alt",
            MainKey::CtrlWin => "ctrl+win",
            MainKey::CapsLock => "capslock",
        }
    }

    /// Read a `main_key` value: case and spaces ignored, a few spellings.
    pub fn from_name(v: &str) -> Option<MainKey> {
        let v: String = v
            .split_whitespace()
            .collect::<String>()
            .to_ascii_lowercase();
        Some(match v.as_str() {
            "lalt" | "leftalt" | "left_alt" => MainKey::LAlt,
            "win" | "windows" => MainKey::Win,
            "alt" => MainKey::Alt,
            "ctrl+win" | "win+ctrl" => MainKey::CtrlWin,
            "capslock" | "caps" => MainKey::CapsLock,
            _ => return None,
        })
    }

    /// Its name for people.
    pub fn label(self) -> &'static str {
        match self {
            MainKey::LAlt => "Left Alt",
            MainKey::Win => "Windows key",
            MainKey::Alt => "Alt (either side)",
            MainKey::CtrlWin => "Ctrl + Windows key",
            MainKey::CapsLock => "Caps Lock",
        }
    }

    /// What choosing it means, in plain words.
    pub fn description(self) -> &'static str {
        match self {
            MainKey::LAlt => {
                "Recommended. Astur keeps Left Alt to itself, so apps never react to it. \
                 Use Right Alt for app shortcuts such as Alt+F4."
            }
            MainKey::Win => {
                "Apps and Windows keep the Windows key; only Astur's own shortcuts take \
                 it over, replacing Windows ones such as Win+D. Win+L always locks the PC, \
                 so \"grow\" moves to Win+O (unless one of your shortcuts uses O)."
            }
            MainKey::Alt => {
                "Like GlazeWM. Apps keep Alt, except for the shortcuts Astur uses, such as \
                 Alt+Space and Alt+1."
            }
            MainKey::CtrlWin => {
                "Hold Ctrl and the Windows key together. Few apps use this pair, but every \
                 shortcut takes an extra finger."
            }
            MainKey::CapsLock => {
                "Caps Lock becomes the Astur key and no longer turns capital letters on. \
                 Turn Caps Lock off before you choose this. Pressed in an admin window, \
                 on the lock screen or a UAC prompt it still turns capitals on: press it \
                 there again (Win+L locks the PC), or use the on-screen keyboard."
            }
        }
    }

    /// The modifier bits `MOD` stands for.
    pub fn bits(self) -> u8 {
        match self {
            MainKey::LAlt | MainKey::Alt => MOD_ALT,
            MainKey::Win => MOD_WIN,
            MainKey::CtrlWin => MOD_CTRL | MOD_WIN,
            MainKey::CapsLock => MOD_HYPER,
        }
    }

    /// The VK the WM swallows and tracks itself for a reserved main key
    /// (VK_LMENU, VK_CAPITAL); 0 for one passed through to apps.
    pub fn reserved_vk(self) -> u32 {
        match self {
            MainKey::LAlt => 0xA4,
            MainKey::CapsLock => 0x14,
            _ => 0,
        }
    }
}

/// What a hotkey does. `Extra` is extra_hotkeys entry `arg`; the workspace
/// actions take the workspace index. The WM turns each into its command.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Launcher,
    SystemMenu,
    FocusNext,
    FocusPrev,
    SwapNext,
    SwapPrev,
    ShrinkMaster,
    GrowMaster,
    ShrinkTile,
    GrowTile,
    PromoteMaster,
    ToggleTiling,
    ToggleTabbed,
    ToggleFloat,
    CloseWindow,
    CycleWidth,
    CycleHeight,
    CenterColumn,
    ConsumeLeft,
    ConsumeRight,
    FirstColumn,
    LastColumn,
    MoveFirstColumn,
    MoveLastColumn,
    Terminal,
    Browser,
    FocusLeft,
    FocusUp,
    FocusRight,
    FocusDown,
    MoveLeft,
    MoveUp,
    MoveRight,
    MoveDown,
    SwitchWorkspace,
    MoveToWorkspace,
    Extra,
}

impl Action {
    /// Every action in declaration order (`ALL[a as usize] == a`), which is
    /// how a table entry stores it.
    pub const ALL: [Action; 37] = {
        use Action::*;
        [
            Launcher,
            SystemMenu,
            FocusNext,
            FocusPrev,
            SwapNext,
            SwapPrev,
            ShrinkMaster,
            GrowMaster,
            ShrinkTile,
            GrowTile,
            PromoteMaster,
            ToggleTiling,
            ToggleTabbed,
            ToggleFloat,
            CloseWindow,
            CycleWidth,
            CycleHeight,
            CenterColumn,
            ConsumeLeft,
            ConsumeRight,
            FirstColumn,
            LastColumn,
            MoveFirstColumn,
            MoveLastColumn,
            Terminal,
            Browser,
            FocusLeft,
            FocusUp,
            FocusRight,
            FocusDown,
            MoveLeft,
            MoveUp,
            MoveRight,
            MoveDown,
            SwitchWorkspace,
            MoveToWorkspace,
            Extra,
        ]
    };

    /// What it does, in plain words.
    pub fn label(self) -> &'static str {
        use Action::*;
        match self {
            Launcher => "Open the app launcher",
            SystemMenu => "Open the power menu",
            FocusNext => "Focus the next window",
            FocusPrev => "Focus the previous window",
            SwapNext => "Swap with the next window",
            SwapPrev => "Swap with the previous window",
            ShrinkMaster => "Shrink the main area",
            GrowMaster => "Grow the main area",
            ShrinkTile => "Make the window shorter",
            GrowTile => "Make the window taller",
            PromoteMaster => "Make the window the main one",
            ToggleTiling => "Turn tiling on or off",
            ToggleTabbed => "Stack or tab the column",
            ToggleFloat => "Float or tile the window",
            CloseWindow => "Close the window",
            CycleWidth => "Change the column width",
            CycleHeight => "Change the window height",
            CenterColumn => "Centre the column",
            ConsumeLeft => "Join the column on the left",
            ConsumeRight => "Join the column on the right",
            FirstColumn => "Go to the first column",
            LastColumn => "Go to the last column",
            MoveFirstColumn => "Move the column to the start",
            MoveLastColumn => "Move the column to the end",
            Terminal => "Open the terminal",
            Browser => "Open the browser",
            FocusLeft => "Focus the window on the left",
            FocusUp => "Focus the window above",
            FocusRight => "Focus the window on the right",
            FocusDown => "Focus the window below",
            MoveLeft => "Move the window left",
            MoveUp => "Move the window up",
            MoveRight => "Move the window right",
            MoveDown => "Move the window down",
            SwitchWorkspace => "Go to a workspace",
            MoveToWorkspace => "Move the window to a workspace",
            Extra => "Custom shortcut",
        }
    }
}

/// One hotkey: `mods` + `vk` runs `action` with `arg`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bind {
    pub mods: u8,
    pub vk: u32,
    pub action: Action,
    pub arg: u16,
}

impl Bind {
    /// Its slot in the WM's table: the modifier set times 256 plus the VK.
    pub fn slot(&self) -> usize {
        (self.mods as usize & 31) << 8 | (self.vk as usize & 0xFF)
    }

    /// Its table value (`decode_bind` reads it back); never 0, which is
    /// what an unbound slot holds.
    pub fn entry(&self) -> u32 {
        (self.action as u32 + 1) | (self.arg as u32) << 8
    }
}

/// A table value back to (action, arg); None for an unbound slot.
pub fn decode_bind(e: u32) -> Option<(Action, u16)> {
    let a = Action::ALL.get(((e & 0xFF) as usize).checked_sub(1)?)?;
    Some((*a, (e >> 8) as u16))
}

/// How bad a `BindIssue` is.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Level {
    /// The shortcut does nothing.
    Error,
    /// It works but costs something (a Windows shortcut people rely on, typing).
    Warn,
    /// It works; worth knowing (it replaces a Windows shortcut).
    Info,
}

/// Something worth telling the user about one shortcut.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BindIssue {
    pub level: Level,
    /// Whose shortcut: the action and its arg, as in `Bind`.
    pub action: Action,
    pub arg: u16,
    /// As people read it ("Win+L"), or the chord as written if unreadable.
    pub chord: String,
    /// A plain sentence.
    pub text: String,
}

/// Every hotkey of a config and what is wrong with any of them.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Keymap {
    /// In lookup order; no two share a chord.
    pub binds: Vec<Bind>,
    pub issues: Vec<BindIssue>,
}

/// Read a chord such as `MOD+SHIFT+J` or `win+ctrl+left`: `+`-separated,
/// case-insensitive, `MOD` = the main key, then CTRL, SHIFT, ALT, WIN and one
/// key name (`key_to_vk`; the last one wins, as the WM always read it).
/// None for an unknown word or no key.
pub fn parse_chord(s: &str, main: MainKey) -> Option<(u8, u32)> {
    let mut mods = 0;
    let mut vk = None;
    for t in s.split('+').map(str::trim).filter(|t| !t.is_empty()) {
        match t.to_ascii_uppercase().as_str() {
            "MOD" => mods |= main.bits(),
            "CTRL" | "CONTROL" => mods |= MOD_CTRL,
            "SHIFT" => mods |= MOD_SHIFT,
            "ALT" => mods |= MOD_ALT,
            "WIN" => mods |= MOD_WIN,
            k => vk = Some(key_to_vk(k)?),
        }
    }
    Some((mods, vk?))
}

/// Write a chord as extra_hotkeys spells it, `MOD` for the main key's bits:
/// `parse_chord` reads it back as (mods, vk).
pub fn format_chord(mods: u8, vk: u32, main: MainKey) -> String {
    let mut s = String::new();
    let mut rest = mods;
    if mods & main.bits() == main.bits() {
        s.push_str("MOD+");
        rest &= !main.bits();
    }
    for (bit, name) in [
        (MOD_CTRL, "CTRL+"),
        (MOD_ALT, "ALT+"),
        (MOD_WIN, "WIN+"),
        (MOD_SHIFT, "SHIFT+"),
    ] {
        if rest & bit != 0 {
            s.push_str(name);
        }
    }
    s + &vk_to_key(vk).to_ascii_uppercase()
}

/// A chord as people read it: "Win+Shift+J", "Caps Lock+Space".
pub fn chord_label(mods: u8, vk: u32) -> String {
    let mut s = String::new();
    for (bit, name) in [
        (MOD_HYPER, "Caps Lock+"),
        (MOD_WIN, "Win+"),
        (MOD_CTRL, "Ctrl+"),
        (MOD_ALT, "Alt+"),
        (MOD_SHIFT, "Shift+"),
    ] {
        if mods & bit != 0 {
            s.push_str(name);
        }
    }
    s + &vk_to_key(vk)
}

/// Why a chord can never work, if it can't.
fn refusal(main: MainKey, mods: u8, vk: u32) -> Option<&'static str> {
    if mods & (MOD_ALT | MOD_WIN | MOD_HYPER) == 0 {
        // Ctrl/Shift-only chords stay ignored, as in the old WM. WIN-only and
        // MOD chords, which the old WM dropped for lacking an ALT token, now
        // bind (plan/known-issues.md, "Old files").
        return Some(
            "needs the Astur key, Alt or Win: Ctrl and Shift shortcuts belong to apps, \
             and a key on its own is typing",
        );
    }
    if main == MainKey::LAlt && mods & MOD_ALT != 0 && vk == 0x09 {
        return Some("Alt+Tab always switches windows");
    }
    if mods == MOD_WIN && vk == 0x4C {
        return Some("Windows always locks the PC on Win+L");
    }
    if mods & (MOD_CTRL | MOD_ALT) == MOD_CTRL | MOD_ALT && vk == 0x2E {
        return Some("Windows always handles Ctrl+Alt+Delete");
    }
    None
}

/// Windows' own shortcuts a bound chord replaces: (modifiers as Windows sees
/// them, first VK, last VK, Warn rather than Info, what it does). Notable
/// ones only; apps' own Alt shortcuts are not listed.
const WINDOWS_KEYS: &[(u8, u32, u32, bool, &str)] = &[
    (MOD_WIN, 0x20, 0x20, true, "switches the keyboard language"),
    (MOD_WIN, 0x09, 0x09, false, "opens Task View"),
    (MOD_WIN, 0x30, 0x39, false, "opens a taskbar app"),
    (MOD_WIN, 0x25, 0x28, false, "snaps the window"),
    (MOD_WIN, 0x24, 0x24, false, "minimises the other windows"),
    (MOD_WIN, 0x41, 0x41, false, "opens quick settings"),
    (MOD_WIN, 0x44, 0x44, false, "shows the desktop"),
    (MOD_WIN, 0x45, 0x45, false, "opens File Explorer"),
    (MOD_WIN, 0x47, 0x47, false, "opens the Game Bar"),
    (MOD_WIN, 0x48, 0x48, false, "starts voice typing"),
    (MOD_WIN, 0x49, 0x49, false, "opens Settings"),
    (MOD_WIN, 0x4B, 0x4B, false, "opens Cast"),
    (MOD_WIN, 0x4D, 0x4D, false, "minimises every window"),
    (MOD_WIN, 0x4E, 0x4E, false, "opens notifications"),
    (MOD_WIN, 0x50, 0x50, false, "changes the display mode"),
    (MOD_WIN, 0x52, 0x52, false, "opens Run"),
    (MOD_WIN, 0x53, 0x53, false, "opens search"),
    (MOD_WIN, 0x55, 0x55, true, "opens accessibility settings"),
    (MOD_WIN, 0x56, 0x56, false, "opens clipboard history"),
    (MOD_WIN, 0x58, 0x58, false, "opens the Start button menu"),
    (MOD_WIN, 0xBA, 0xBA, false, "opens the emoji panel"),
    (MOD_WIN, 0xBE, 0xBE, false, "opens the emoji panel"),
    (MOD_WIN, 0xBB, 0xBB, true, "zooms in with Magnifier"),
    (MOD_WIN, 0xBD, 0xBD, true, "zooms out with Magnifier"),
    (MOD_WIN | MOD_SHIFT, 0x53, 0x53, false, "takes a screenshot"),
    (
        MOD_WIN | MOD_SHIFT,
        0x25,
        0x28,
        false,
        "moves the window to another screen",
    ),
    (MOD_WIN | MOD_CTRL, 0x0D, 0x0D, true, "starts Narrator"),
    (
        MOD_WIN | MOD_CTRL,
        0x44,
        0x44,
        false,
        "adds a virtual desktop",
    ),
    (
        MOD_WIN | MOD_CTRL,
        0x25,
        0x27,
        false,
        "switches virtual desktops",
    ),
    (
        MOD_WIN | MOD_CTRL,
        0x73,
        0x73,
        false,
        "closes the virtual desktop",
    ),
    (MOD_ALT, 0x09, 0x09, true, "switches windows"),
    (MOD_ALT, 0x73, 0x73, true, "closes the window"),
    (MOD_ALT, 0x20, 0x20, false, "opens the window menu"),
    (MOD_ALT, 0x1B, 0x1B, false, "cycles windows"),
];

/// What a working chord costs, if anything.
fn caution(main: MainKey, mods: u8, vk: u32) -> Option<(Level, String)> {
    // A chord holding a reserved key (Left Alt under lalt, Caps Lock) costs
    // nothing: the reserved key never reaches Windows or apps, so the same
    // combo without it (plain Win+D, AltGr+Q) looks up another slot and does
    // what it always did. Reporting it as a takeover pushed users off valid
    // binds.
    let reserved = match main {
        MainKey::LAlt => MOD_ALT | MOD_HYPER,
        _ => MOD_HYPER,
    };
    if mods & reserved != 0 {
        return None;
    }
    let seen = mods;
    let printable = matches!(vk, 0x30..=0x39 | 0x41..=0x5A | 0xBA..=0xC0 | 0xDB..=0xDE);
    // Win held as well is no AltGr typing.
    if seen & (MOD_CTRL | MOD_ALT | MOD_WIN) == MOD_CTRL | MOD_ALT && printable {
        return Some((
            Level::Warn,
            "on many non-US keyboards Ctrl+Alt types characters such as @ and \u{20AC}; \
             this shortcut stops that"
                .into(),
        ));
    }
    let (_, _, _, warn, what) = WINDOWS_KEYS
        .iter()
        .find(|k| k.0 == seen && (k.1..=k.2).contains(&vk))?;
    let level = if *warn { Level::Warn } else { Level::Info };
    Some((
        level,
        format!("replaces Windows' own shortcut, which {what}"),
    ))
}

impl Keymap {
    fn issue(&mut self, level: Level, act: (Action, u16), chord: String, text: String) {
        self.issues.push(BindIssue {
            level,
            action: act.0,
            arg: act.1,
            chord,
            text,
        });
    }

    /// Bind `mods`+`vk` unless it can't work or an earlier bind has it (the
    /// earlier one is what the WM's old lookup ran). `quiet`: no issue, for
    /// the legacy Left Alt variants (`put_loose`).
    fn put(&mut self, c: &Config, mods: u8, vk: u32, act: (Action, u16), quiet: bool) {
        let chord = chord_label(mods, vk);
        if let Some(why) = refusal(c.main_key, mods, vk) {
            if !quiet {
                self.issue(
                    Level::Error,
                    act,
                    chord,
                    format!("{why}, so this does nothing"),
                );
            }
            return;
        }
        if let Some(b) = self.binds.iter().find(|b| b.mods == mods && b.vk == vk) {
            if !quiet && (b.action, b.arg) != act {
                let text = format!("does nothing: {} uses it first", what(c, b.action, b.arg));
                self.issue(Level::Warn, act, chord, text);
            }
            return;
        }
        if !quiet {
            if let Some((level, text)) = caution(c.main_key, mods, vk) {
                self.issue(level, act, chord, text);
            }
        }
        self.binds.push(Bind {
            mods,
            vk,
            action: act.0,
            arg: act.1,
        });
    }

    /// A built-in, popup or workspace key. Under Left Alt it also takes
    /// Ctrl: the old hook ignored Ctrl for these, so Alt+Ctrl+J was Alt+J.
    /// Not under the other main keys: where apps see Alt, Ctrl+Alt is AltGr
    /// (typing), and Win+Ctrl+arrows switch virtual desktops.
    fn put_loose(&mut self, c: &Config, mods: u8, vk: u32, act: (Action, u16), quiet: bool) {
        self.put(c, mods, vk, act, quiet);
        if c.main_key == MainKey::LAlt {
            self.put(c, mods | MOD_CTRL, vk, act, true);
        }
    }

    /// A built-in key: the action alone, its Shift action with Shift. With
    /// no Shift action, Left Alt ignores Shift too (Alt+Shift+W always closed
    /// the window), the other main keys leave that chord free.
    fn put_builtin(&mut self, c: &Config, vk: u32, plain: Action, shifted: Option<Action>) {
        let m = c.main_key.bits();
        self.put_loose(c, m, vk, (plain, 0), false);
        match shifted {
            Some(s) => self.put_loose(c, m | MOD_SHIFT, vk, (s, 0), false),
            None if c.main_key == MainKey::LAlt => {
                self.put_loose(c, m | MOD_SHIFT, vk, (plain, 0), true)
            }
            None => {}
        }
    }
}

/// Who owns a bind, for an issue's text.
fn what(c: &Config, action: Action, arg: u16) -> String {
    match action {
        Action::SwitchWorkspace => format!("workspace {}", arg + 1),
        Action::MoveToWorkspace => format!("moving a window to workspace {}", arg + 1),
        Action::Extra => match c.extra_hotkeys.get(arg as usize) {
            Some(h) => format!("custom shortcut {} ({})", arg + 1, h.action),
            None => format!("custom shortcut {}", arg + 1),
        },
        a => format!("\u{201C}{}\u{201D}", a.label()),
    }
}

/// `key_grow_master` as the WM binds it. Under `main_key = win` its shipped
/// L would be Win+L, which always locks the PC, so it moves to O: a letter
/// (same meaning on every layout), the key right above L, no Astur default,
/// and Windows' Win+O (rotation lock) exists only on tablets. Any other key
/// the user set stays. A user who set L on purpose loses nothing: Win+L can
/// never reach Astur. Only when O is free: grow_master is looked up before
/// most `key_*` keys and the workspace keys, so taking Win+O from a key the
/// user chose (a float key or workspace key on O, an extra on Win+O) would
/// silently break it. Then L stays, and compile_keys reports it honestly as
/// a Win+L that does nothing.
pub fn grow_master_key(c: &Config) -> u32 {
    const O: u32 = 0x4F;
    if c.main_key != MainKey::Win || c.key_grow_master != 0x4C {
        return c.key_grow_master;
    }
    let keys = [
        c.key_focus_next,
        c.key_focus_prev,
        c.key_shrink_master,
        c.key_promote_master,
        c.key_toggle_tiling,
        c.key_toggle_float,
        c.key_close_window,
        c.key_cycle_width,
        c.key_center_column,
        c.key_consume_left,
        c.key_consume_right,
    ];
    let extra_on_o = c.extra_hotkeys.iter().any(|h| {
        (h.action != "scratchpad" || c.scratchpad_enabled)
            && matches!(parse_chord(&h.chord, MainKey::Win),
                Some((m, O)) if m == MOD_WIN || m == MOD_WIN | MOD_SHIFT)
    });
    if keys.contains(&O) || c.workspace_keys.contains(&O) || extra_on_o {
        c.key_grow_master
    } else {
        O
    }
}

/// Every hotkey of `c` in the order the WM looks them up, plus every problem
/// with them. The WM fills its table from `binds`; Settings shows `issues`.
/// Order, first wins on a shared chord (as the old hook's lookup): the power
/// menu and launcher, extra_hotkeys, the built-in keys (the `key_*` ones,
/// then Home / End / Enter / arrows), the workspace keys.
pub fn compile_keys(c: &Config) -> Keymap {
    use Action::*;
    let mut k = Keymap::default();
    let m = c.main_key.bits();
    if c.system_menu_enabled {
        k.put_loose(c, m | MOD_SHIFT, 0x20, (SystemMenu, 0), false);
    }
    if c.launcher_enabled {
        k.put_loose(c, m, 0x20, (Launcher, 0), false);
    }
    for (i, h) in c.extra_hotkeys.iter().enumerate() {
        if h.action == "scratchpad" && !c.scratchpad_enabled {
            continue;
        }
        let act = (Extra, i as u16);
        match parse_chord(&h.chord, c.main_key) {
            Some((mods, vk)) => k.put(c, mods, vk, act, false),
            None => k.issue(
                Level::Error,
                act,
                h.chord.clone(),
                "Astur can't read this shortcut, so it does nothing".into(),
            ),
        }
    }
    if grow_master_key(c) != c.key_grow_master {
        k.issue(
            Level::Info,
            (GrowMaster, 0),
            "Win+L".into(),
            "Win+L always locks the PC, so this uses Win+O".into(),
        );
    }
    for (vk, plain, shifted) in [
        (c.key_focus_next, FocusNext, Some(SwapNext)),
        (c.key_focus_prev, FocusPrev, Some(SwapPrev)),
        (c.key_shrink_master, ShrinkMaster, Some(ShrinkTile)),
        (grow_master_key(c), GrowMaster, Some(GrowTile)),
        (c.key_promote_master, PromoteMaster, None),
        (c.key_toggle_tiling, ToggleTiling, Some(ToggleTabbed)),
        (c.key_toggle_float, ToggleFloat, None),
        (c.key_close_window, CloseWindow, None),
        (c.key_cycle_width, CycleWidth, Some(CycleHeight)),
        (c.key_center_column, CenterColumn, None),
        (c.key_consume_left, ConsumeLeft, None),
        (c.key_consume_right, ConsumeRight, None),
        (0x24, FirstColumn, Some(MoveFirstColumn)),
        (0x23, LastColumn, Some(MoveLastColumn)),
        (0x0D, Terminal, Some(Browser)),
        (0x25, FocusLeft, Some(MoveLeft)),
        (0x26, FocusUp, Some(MoveUp)),
        (0x27, FocusRight, Some(MoveRight)),
        (0x28, FocusDown, Some(MoveDown)),
    ] {
        k.put_builtin(c, vk, plain, shifted);
    }
    for (i, &vk) in c.workspace_keys.iter().enumerate() {
        k.put_loose(c, m, vk, (SwitchWorkspace, i as u16), false);
        k.put_loose(c, m | MOD_SHIFT, vk, (MoveToWorkspace, i as u16), false);
    }
    k
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_keys_are_reported_not_swallowed() {
        // The single likeliest user error with 140+ keys across two files.
        // Silence here is what F-B7 was about.
        let c = parse_pair(
            "bar_heigth = 40
outer_gap = 12
",
            "heigth = 30
",
        );
        assert_eq!(c.outer_gap, 12, "known keys still apply");
        assert_eq!(
            c.unknown_keys,
            vec![
                "astur.conf: bar_heigth".to_string(),
                "navbar.conf: heigth".to_string()
            ]
        );
    }

    #[test]
    fn a_clean_config_reports_nothing() {
        // Guards against the opposite failure: warning about every key in our
        // own shipped templates, which would train users to ignore the warning.
        let c = parse_pair(default_config_text(), default_navbar_text());
        assert!(
            c.unknown_keys.is_empty(),
            "the shipped templates must parse cleanly: {:?}",
            c.unknown_keys
        );
    }

    #[test]
    fn comments_and_blank_lines_are_not_unknown_keys() {
        let c = parse_text(
            "# comment = value

   
outer_gap = 4
",
        );
        assert!(c.unknown_keys.is_empty());
        assert_eq!(c.outer_gap, 4);
    }

    #[test]
    fn a_utf8_bom_does_not_hide_the_first_line() {
        // Notepad's "UTF-8 with BOM": the BOM used to stick to the first key,
        // which was then ignored (or, before a comment, reported as a typo).
        let c = parse_pair("\u{FEFF}outer_gap = 12\n", "\u{FEFF}# bar\nheight = 40\n");
        assert_eq!(c.outer_gap, 12);
        assert_eq!(c.bar_height, 40);
        assert!(c.unknown_keys.is_empty(), "{:?}", c.unknown_keys);
    }

    #[test]
    fn async_window_pos_defaults_on_parses_and_round_trips() {
        assert!(Config::defaults().async_window_pos);
        // The shipped template states the default, it does not override it.
        assert!(parse_pair(default_config_text(), "").async_window_pos);
        assert!(!parse_pair("async_window_pos = false\n", "").async_window_pos);
        assert!(parse_pair("async_window_pos = true\n", "").async_window_pos);
        let off = set_conf_key(default_config_text(), "async_window_pos", "false");
        assert!(!parse_pair(&off, "").async_window_pos);
        let on = set_conf_key(&off, "async_window_pos", "true");
        assert!(parse_pair(&on, "").async_window_pos);
        assert_eq!(
            on,
            default_config_text(),
            "a round trip restores the template"
        );
    }

    #[test]
    fn parse_bool_variants() {
        for t in ["true", "yes", "1", "on", "TRUE", "On"] {
            assert!(parse_bool(t), "{t} should be true");
        }
        for f in ["false", "no", "0", "off", "", "maybe"] {
            assert!(!parse_bool(f), "{f} should be false");
        }
    }

    #[test]
    fn key_to_vk_letters_digits_fkeys() {
        assert_eq!(key_to_vk("1"), Some(0x31));
        assert_eq!(key_to_vk("a"), Some(0x41)); // case-insensitive
        assert_eq!(key_to_vk("Z"), Some(0x5A));
        assert_eq!(key_to_vk("F1"), Some(0x70));
        assert_eq!(key_to_vk("F24"), Some(0x87));
        assert_eq!(key_to_vk("F25"), None); // out of range
        assert_eq!(key_to_vk(""), None);
        assert_eq!(key_to_vk("AB"), None); // multi-char non-F
        assert_eq!(key_to_vk("@"), None);
    }

    #[test]
    fn key_to_vk_brackets_and_aliases() {
        for name in ["[", " lbracket ", "LBRACKET", "oem_4", "OEM_4"] {
            assert_eq!(key_to_vk(name), Some(0xDB), "{name}");
        }
        for name in ["]", "rbracket", "RBRACKET", "oem_6", "OEM_6"] {
            assert_eq!(key_to_vk(name), Some(0xDD), "{name}");
        }
        assert_eq!(key_to_vk("OEM_5"), None);
        assert_eq!(parse_keys("[ , ]"), vec![0xDB, 0xDD]);
    }

    #[test]
    fn parse_keys_splits_and_skips_invalid() {
        assert_eq!(parse_keys("1 2 3"), vec![0x31, 0x32, 0x33]);
        assert_eq!(parse_keys("Q, W,  E"), vec![0x51, 0x57, 0x45]);
        assert_eq!(parse_keys("1 ?? 2"), vec![0x31, 0x32]); // invalid token dropped
        assert!(parse_keys("   ").is_empty());
    }

    #[test]
    fn parse_color_rgb_to_colorref_and_fallback() {
        assert_eq!(parse_color("#FF0000", 0), 0x0000_00FF); // red -> 0x00BBGGRR
        assert_eq!(parse_color("00FF00", 0), 0x0000_FF00); // green, no leading '#'
        assert_eq!(parse_color("#0000FF", 0), 0x00FF_0000); // blue
        assert_eq!(parse_color("#xyz", 0xDEAD), 0xDEAD); // malformed -> fallback
        assert_eq!(parse_color("#FFF", 7), 7); // wrong length -> fallback
    }

    #[test]
    fn structured_records_escape_separators_and_preserve_paths() {
        let original = vec![LauncherEntry {
            label: "Build | test".to_string(),
            target: r#"cmd:powershell -Command "a|b;;c""#.to_string(),
            icon: r"C:\".to_string(),
        }];
        let text = format_launcher_entries(&original);
        assert_eq!(parse_launcher_entries(&text), original);

        let hotkeys = vec![HotkeyDef {
            chord: "ALT+Q".to_string(),
            action: "launch".to_string(),
            argument: "cmd:echo one|findstr one; echo two".to_string(),
        }];
        assert_eq!(parse_hotkeys(&format_hotkeys(&hotkeys)), hotkeys);
    }

    #[test]
    fn a_trailing_backslash_survives_a_second_record() {
        let hk = |arg: &str| HotkeyDef {
            chord: "ALT+E".to_string(),
            action: "launch".to_string(),
            argument: arg.to_string(),
        };
        let hotkeys = vec![hk(r"explorer C:\Projects\"), hk("notepad")];
        let text = format_hotkeys(&hotkeys);
        assert_eq!(parse_hotkeys(&text), hotkeys);
        // Each save re-formats what it read: no growth.
        assert_eq!(format_hotkeys(&parse_hotkeys(&text)), text);
    }

    #[test]
    fn parse_into_gaps_clamped() {
        let mut c = Config::defaults();
        parse_into(&mut c, "outer_gap = -10\ninner_gap = 99999");
        assert_eq!(c.outer_gap, 0); // negative clamped up
        assert_eq!(c.inner_gap, 500); // huge clamped down
    }

    #[test]
    fn workspace_wallpaper_slots_preserve_empty_positions() {
        let c = parse_text("workspace_wallpapers = one.jpg ;; ;; three.png");
        assert_eq!(c.workspace_wallpapers, vec!["one.jpg", "", "three.png"]);
    }

    #[test]
    fn parse_into_unknown_key_ignored() {
        let mut c = Config::defaults();
        let before = c.outer_gap;
        parse_into(&mut c, "totally_unknown = 5\n# comment\n\n");
        assert_eq!(c.outer_gap, before);
    }

    #[test]
    fn workspaces_past_ten_are_accepted() {
        // #18: an 11th workspace on an extra key (GlazeWM-style 'G').
        let c = parse_text("workspaces = 11\nworkspace_keys = 1 2 3 4 5 6 7 8 9 0 G");
        assert_eq!(c.workspaces, 11);
        assert_eq!(c.workspace_keys.len(), 11);
        assert_eq!(parse_text("workspaces = 0").workspaces, 1);
    }

    #[test]
    fn parse_into_navbar_alias_and_clamps() {
        let mut c = Config::defaults();
        parse_into(&mut c, "workspaces = 999\nbar_height = 999\nheight = 40");
        assert_eq!(c.workspaces, MAX_WORKSPACES); // clamp 1..=MAX_WORKSPACES
        assert_eq!(c.bar_height, 40); // last write wins, clamp 0..=200
    }

    #[test]
    fn parse_into_float_clamps() {
        let mut c = Config::defaults();
        parse_into(&mut c, "master_ratio = 2.0\nunfocused_opacity = -1");
        assert_eq!(c.master_ratio, 0.9);
        assert_eq!(c.unfocused_opacity, 0.1);
    }

    #[test]
    fn parse_into_mode_and_layout() {
        let mut c = Config::defaults();
        parse_into(&mut c, "workspace_mode = per_monitor\nlayout = MASTER");
        assert!(c.per_monitor);
        assert_eq!(c.layout, "master"); // lowercased
        parse_into(&mut c, "layout = Scrolling");
        assert_eq!(c.layout, "scrolling");
        parse_into(&mut c, "layout = spiral"); // invalid -> keeps previous
        assert_eq!(c.layout, "scrolling");
    }

    #[test]
    fn layouts_lists_all_six() {
        for name in [
            "dwindle",
            "master",
            "columns",
            "grid",
            "monocle",
            "scrolling",
        ] {
            assert!(LAYOUTS.contains(&name), "{name}");
        }
        assert_eq!(LAYOUTS.len(), 6);
    }

    #[test]
    fn monitor_layouts_are_positional() {
        let c = parse_text("monitor_layouts = scrolling, , master");
        assert_eq!(c.monitor_layouts, vec!["scrolling", "", "master"]);
        // default and typos inherit without shifting later monitors.
        let c = parse_text("monitor_layouts = Default, spiral, GRID");
        assert_eq!(c.monitor_layouts, vec!["", "", "grid"]);
        // Trailing blanks are kept as written.
        let c = parse_text("monitor_layouts = scrolling, ");
        assert_eq!(c.monitor_layouts, vec!["scrolling", ""]);
        // Nothing set anywhere is the same as the key being absent.
        for v in ["", "default", " , , ", "bogus"] {
            let c = parse_text(&format!("monitor_layouts = {v}"));
            assert!(c.monitor_layouts.is_empty(), "{v:?}");
        }
    }

    #[test]
    fn layout_for_monitor_falls_back_to_layout() {
        let mut c = parse_text("layout = columns\nmonitor_layouts = scrolling, , master");
        assert_eq!(c.layout_for_monitor(0), "scrolling");
        assert_eq!(c.layout_for_monitor(1), "columns"); // blank entry
        assert_eq!(c.layout_for_monitor(2), "master");
        assert_eq!(c.layout_for_monitor(3), "columns"); // past the list
        assert_eq!(c.layout_for_monitor(usize::MAX), "columns");
        c.monitor_layouts.clear();
        assert_eq!(c.layout_for_monitor(0), "columns");
    }

    #[test]
    fn scrolling_keys_parse_clamp_and_fall_back() {
        let d = Config::defaults();
        assert_eq!(d.scroll_default_width, 0.5);
        assert_eq!(d.scroll_width_presets, vec![0.333, 0.5, 0.667, 1.0]);
        assert_eq!(d.scroll_height_presets, vec![0.333, 0.5, 0.667]);
        assert_eq!(d.scroll_center, "never");
        assert_eq!(d.tab_bar_height, 26);

        let mut c = Config::defaults();
        parse_into(&mut c, "scroll_default_width = 0.75");
        assert_eq!(c.scroll_default_width, 0.75);
        parse_into(&mut c, "scroll_default_width = 2");
        assert_eq!(c.scroll_default_width, 1.0);
        parse_into(&mut c, "scroll_default_width = 0");
        assert_eq!(c.scroll_default_width, 0.1);
        for bad in ["wide", "NaN", ""] {
            parse_into(&mut c, &format!("scroll_default_width = {bad}"));
            assert_eq!(
                c.scroll_default_width, 0.1,
                "{bad:?} keeps the previous value"
            );
        }

        parse_into(&mut c, "scroll_width_presets = 0.25 0.5, 2,x");
        assert_eq!(c.scroll_width_presets, vec![0.25, 0.5, 1.0]);
        for bad in ["", "x y", "NaN, -"] {
            parse_into(&mut c, &format!("scroll_width_presets = {bad}"));
            assert_eq!(c.scroll_width_presets, d.scroll_width_presets, "{bad:?}");
        }
        parse_into(&mut c, "scroll_height_presets = 0.05, 0.95");
        assert_eq!(c.scroll_height_presets, vec![0.1, 0.9]);
        parse_into(&mut c, "scroll_height_presets =");
        assert_eq!(c.scroll_height_presets, d.scroll_height_presets);

        parse_into(&mut c, "scroll_center = ALWAYS");
        assert_eq!(c.scroll_center, "always");
        parse_into(&mut c, "scroll_center = on_overflow");
        assert_eq!(c.scroll_center, "on-overflow");
        parse_into(&mut c, "scroll_center = sometimes"); // invalid -> keeps previous
        assert_eq!(c.scroll_center, "on-overflow");
        parse_into(&mut c, "scroll_center = never");
        assert_eq!(c.scroll_center, "never");

        parse_into(&mut c, "tab_bar_height = 5");
        assert_eq!(c.tab_bar_height, 16);
        parse_into(&mut c, "tab_bar_height = 100");
        assert_eq!(c.tab_bar_height, 64);
        parse_into(&mut c, "tab_bar_height = tall");
        assert_eq!(c.tab_bar_height, 64);
    }

    #[test]
    fn scrolling_key_bindings_parse() {
        let d = Config::defaults();
        assert_eq!(
            (d.key_cycle_width, d.key_center_column),
            (0x52, 0x43) // R, C
        );
        assert_eq!((d.key_consume_left, d.key_consume_right), (0xDB, 0xDD));
        let c = parse_text(
            "key_cycle_width = q\nkey_center_column = F2\nkey_consume_left = OEM_6\nkey_consume_right = ??",
        );
        assert_eq!(c.key_cycle_width, 0x51);
        assert_eq!(c.key_center_column, 0x71);
        assert_eq!(c.key_consume_left, 0xDD);
        assert_eq!(c.key_consume_right, 0xDD, "invalid name keeps the default");
    }

    #[test]
    fn template_states_the_scrolling_defaults() {
        let t = parse_pair(default_config_text(), "");
        let d = Config::defaults();
        assert_eq!(t.layout, d.layout);
        assert_eq!(t.monitor_layouts, d.monitor_layouts);
        assert_eq!(t.scroll_default_width, d.scroll_default_width);
        assert_eq!(t.scroll_width_presets, d.scroll_width_presets);
        assert_eq!(t.scroll_height_presets, d.scroll_height_presets);
        assert_eq!(t.scroll_center, d.scroll_center);
        assert_eq!(t.tab_bar_height, d.tab_bar_height);
        assert_eq!(t.key_cycle_width, d.key_cycle_width);
        assert_eq!(t.key_center_column, d.key_center_column);
        assert_eq!(t.key_consume_left, d.key_consume_left);
        assert_eq!(t.key_consume_right, d.key_consume_right);
    }

    #[test]
    fn parse_into_theme_validated() {
        let mut c = Config::defaults();
        parse_into(&mut c, "theme = LIGHT");
        assert_eq!(c.theme, "light");
        parse_into(&mut c, "theme = neon"); // invalid -> keeps previous
        assert_eq!(c.theme, "light");
    }

    #[test]
    fn parse_into_zones_filter_unknown() {
        let mut c = Config::defaults();
        parse_into(&mut c, "left = workspaces bogus apps\nright = clock");
        assert_eq!(c.bar_left, vec!["workspaces", "apps"]);
        assert_eq!(c.bar_right, vec!["clock"]);
        parse_into(&mut c, "center ="); // explicit empty zone
        assert!(c.bar_center.is_empty());
    }

    #[test]
    fn parse_into_bar_style_clamped() {
        let mut c = Config::defaults();
        parse_into(
            &mut c,
            "floating = yes\nmargin = 999\nradius = -3\nautohide = on",
        );
        assert!(c.bar_floating);
        assert_eq!(c.bar_margin, 200);
        assert_eq!(c.bar_radius, 0);
        assert!(c.bar_autohide);
    }

    #[test]
    fn set_conf_key_replaces_a_key_behind_a_bom() {
        assert_eq!(
            set_conf_key("\u{FEFF}outer_gap = 12\n", "outer_gap", "20"),
            "outer_gap = 20\n"
        );
    }

    #[test]
    fn set_conf_key_writes_a_multi_line_value_on_one_line() {
        assert_eq!(
            set_conf_key("terminal = a\n", "terminal", "x\ny\r\nz"),
            "terminal = x y  z\n"
        );
    }

    #[test]
    fn set_conf_key_replaces_in_place() {
        let text = "# comment\nheight = 28\nbottom = false\n";
        let out = set_conf_key(text, "height", "40");
        assert_eq!(out, "# comment\nheight = 40\nbottom = false\n");
    }

    #[test]
    fn set_conf_key_appends_when_missing() {
        let text = "height = 28\n";
        let out = set_conf_key(text, "radius", "16");
        assert_eq!(out, "height = 28\nradius = 16\n");
    }

    #[test]
    fn set_conf_key_ignores_commented_lines() {
        let text = "# height = 99\nheight = 28\n";
        let out = set_conf_key(text, "height", "40");
        assert_eq!(out, "# height = 99\nheight = 40\n");
    }

    #[test]
    fn set_conf_key_drops_duplicates() {
        // Last-write-wins parser: a stale duplicate below would override the
        // value we set, so duplicates are collapsed into the first slot.
        let text = "height = 28\nfont_size = 0\nheight = 30\n";
        let out = set_conf_key(text, "height", "40");
        assert_eq!(out, "height = 40\nfont_size = 0\n");
    }

    #[test]
    fn color_roundtrip() {
        let c = parse_color("#366382", 0);
        assert_eq!(color_to_hex(c), "#366382");
    }

    #[test]
    fn theme_colors_auto_legacy_custom() {
        let mut c = Config::defaults();
        parse_into(&mut c, "bg = auto");
        assert_eq!(c.bar_bg, None);
        // The old shipped dark default reads as auto (pre-theme files spelled
        // the defaults out; they must still pick up the light preset).
        parse_into(&mut c, "bg = #1A1B26");
        assert_eq!(c.bar_bg, None);
        parse_into(&mut c, "bg = #123456");
        assert_eq!(c.bar_bg, Some(parse_color("#123456", 0)));
        // Malformed keeps the previous value.
        parse_into(&mut c, "bg = #xyz");
        assert_eq!(c.bar_bg, Some(parse_color("#123456", 0)));
    }

    #[test]
    fn a_pinned_legacy_colour_stays_pinned() {
        // Settings seeds a pin with the key's legacy default, which reads back
        // as auto: unticking Auto and saving used to tick it again.
        for legacy in BAR_DARK.into_iter().chain([0x0016_1616, 0x00FF_FFFF]) {
            let pinned = pin_theme_color(legacy, legacy);
            assert_eq!(
                parse_theme_color(&color_to_hex(pinned), legacy, None),
                Some(pinned)
            );
        }
        assert_eq!(pin_theme_color(0x0012_3456, BAR_DARK[0]), 0x0012_3456);
    }

    #[test]
    fn vk_roundtrip() {
        for name in [
            "A", "Z", "0", "9", "F1", "F24", "[", "]", "Space", "Left", "`",
        ] {
            let vk = key_to_vk(name).unwrap();
            assert_eq!(vk_to_key(vk), name);
        }
        // Every named key reads back from the name it is written as, any case.
        for &(_, vk) in KEY_NAMES {
            assert_eq!(key_to_vk(&vk_to_key(vk).to_ascii_lowercase()), Some(vk));
        }
        assert_eq!(key_to_vk("pgdn"), Some(0x22));
        assert_eq!(key_to_vk("ESCAPE"), Some(0x1B));
    }

    /// Each bind of `c` as chord -> (action, arg).
    fn keymap(c: &Config) -> std::collections::HashMap<(u8, u32), (Action, u16)> {
        compile_keys(c)
            .binds
            .iter()
            .map(|b| ((b.mods, b.vk), (b.action, b.arg)))
            .collect()
    }

    #[test]
    fn chords_parse_and_round_trip() {
        let p = parse_chord;
        assert_eq!(
            p("ALT+SHIFT+F2", MainKey::LAlt),
            Some((MOD_ALT | MOD_SHIFT, 0x71))
        );
        assert_eq!(
            p(" mod + ctrl + left ", MainKey::Win),
            Some((MOD_WIN | MOD_CTRL, 0x25))
        );
        assert_eq!(
            p("MOD+J", MainKey::CtrlWin),
            Some((MOD_CTRL | MOD_WIN, 0x4A))
        );
        assert_eq!(p("MOD+Space", MainKey::CapsLock), Some((MOD_HYPER, 0x20)));
        assert_eq!(p("ALT+SHIFT", MainKey::LAlt), None); // no key
        assert_eq!(p("ALT+FOO+J", MainKey::LAlt), None); // unknown word
        for main in MainKey::ALL {
            assert_eq!(MainKey::from_name(main.name()), Some(main));
            for mods in 0..32u8 {
                // Caps Lock exists only as the main key.
                if mods & MOD_HYPER != 0 && main != MainKey::CapsLock {
                    continue;
                }
                for vk in [0x4A, 0x20, 0xBA, 0xDC, 0x70] {
                    let s = format_chord(mods, vk, main);
                    assert_eq!(p(&s, main), Some((mods, vk)), "{s}");
                }
            }
        }
        assert_eq!(MainKey::from_name(" Win + Ctrl "), Some(MainKey::CtrlWin));
        assert_eq!(chord_label(MOD_WIN | MOD_SHIFT, 0x4A), "Win+Shift+J");
        assert_eq!(chord_label(MOD_HYPER, 0x20), "Caps Lock+Space");
    }

    #[test]
    fn main_key_parses_with_its_alias() {
        assert_eq!(Config::defaults().main_key, MainKey::LAlt);
        assert_eq!(parse_text("main_key = win").main_key, MainKey::Win);
        assert_eq!(parse_text("mod = capslock").main_key, MainKey::CapsLock);
        assert_eq!(parse_text("main_key = shift").main_key, MainKey::LAlt);
        assert!(parse_text("mod = alt").unknown_keys.is_empty());
    }

    #[test]
    fn actions_decode_from_their_table_entry() {
        for (i, a) in Action::ALL.iter().enumerate() {
            assert_eq!(*a as usize, i);
            let b = Bind {
                mods: MOD_WIN,
                vk: 0x4A,
                action: *a,
                arg: 19,
            };
            assert_eq!(decode_bind(b.entry()), Some((*a, 19)));
        }
        assert_eq!(decode_bind(0), None);
        assert_eq!(decode_bind(Action::ALL.len() as u32 + 1), None);
    }

    #[test]
    fn default_keys_compile_without_issues() {
        let k = compile_keys(&default_config());
        assert!(k.issues.is_empty(), "{:?}", k.issues);
        let m = keymap(&default_config());
        let ws = |i| (Action::SwitchWorkspace, i);
        assert_eq!(m[&(MOD_ALT, 0x4A)], (Action::FocusNext, 0));
        assert_eq!(m[&(MOD_ALT | MOD_SHIFT, 0x4A)], (Action::SwapNext, 0));
        assert_eq!(m[&(MOD_ALT, 0x31)], ws(0));
        assert_eq!(m[&(MOD_ALT, 0x20)], (Action::Launcher, 0));
        assert_eq!(m[&(MOD_ALT | MOD_SHIFT, 0x20)], (Action::SystemMenu, 0));
        // Left Alt's old quirks: Ctrl ignored, Shift ignored with no Shift action.
        assert_eq!(m[&(MOD_ALT | MOD_CTRL, 0x31)], ws(0));
        assert_eq!(m[&(MOD_ALT | MOD_SHIFT, 0x57)], (Action::CloseWindow, 0));
        // The other main keys bind exactly what they say.
        let mut c = default_config();
        c.main_key = MainKey::Alt;
        let m = keymap(&c);
        assert_eq!(m[&(MOD_ALT, 0x31)], ws(0));
        assert!(
            !m.contains_key(&(MOD_ALT | MOD_CTRL, 0x31)),
            "Ctrl+Alt+1 is AltGr"
        );
        assert!(!m.contains_key(&(MOD_ALT | MOD_SHIFT, 0x57)));
        // An old ALT chord: Left Alt under lalt, the real Alt under the others.
        c.main_key = MainKey::Win;
        c.scratchpad_enabled = true;
        assert_eq!(keymap(&c)[&(MOD_ALT, 0xC0)], (Action::Extra, 0));
        c.main_key = MainKey::LAlt;
        assert_eq!(keymap(&c)[&(MOD_ALT, 0xC0)], (Action::Extra, 0));
    }

    #[test]
    fn win_main_key_moves_grow_master_off_win_l() {
        let mut c = default_config();
        c.main_key = MainKey::Win;
        let m = keymap(&c);
        assert_eq!(m[&(MOD_WIN, 0x4F)], (Action::GrowMaster, 0));
        assert_eq!(m[&(MOD_WIN | MOD_SHIFT, 0x4F)], (Action::GrowTile, 0));
        assert!(!m.contains_key(&(MOD_WIN, 0x4C)));
        let k = compile_keys(&c);
        assert!(
            k.issues.iter().all(|i| i.level != Level::Error),
            "{:?}",
            k.issues
        );
        assert!(k
            .issues
            .iter()
            .any(|i| i.action == Action::GrowMaster && i.level == Level::Info));
        // Win+Space (keyboard language) is a warning.
        assert!(k
            .issues
            .iter()
            .any(|i| i.chord == "Win+Space" && i.level == Level::Warn));
        // A key the user picked stays.
        c.key_grow_master = 0x50;
        assert_eq!(keymap(&c)[&(MOD_WIN, 0x50)], (Action::GrowMaster, 0));
        // Not under the other main keys.
        c = default_config();
        c.main_key = MainKey::CtrlWin;
        let m = keymap(&c);
        assert_eq!(m[&(MOD_CTRL | MOD_WIN, 0x4C)], (Action::GrowMaster, 0));
    }

    #[test]
    fn grow_master_takes_win_o_only_when_it_is_free() {
        let grow = |c: &Config| {
            let k = compile_keys(c);
            let levels: Vec<Level> = (k.issues.iter())
                .filter(|i| i.action == Action::GrowMaster)
                .map(|i| i.level)
                .collect();
            (keymap(c), levels)
        };
        // The user's own float key on O keeps Win+O; grow stays an honest
        // (refused) Win+L rather than a note that it uses Win+O.
        let mut c = default_config();
        c.main_key = MainKey::Win;
        c.key_toggle_float = 0x4F;
        let (m, levels) = grow(&c);
        assert_eq!(m[&(MOD_WIN, 0x4F)], (Action::ToggleFloat, 0));
        assert_eq!(levels, [Level::Error]);
        // A workspace key on O.
        let mut c = default_config();
        c.main_key = MainKey::Win;
        c.workspace_keys = vec![0x59, 0x55, 0x49, 0x4F];
        let (m, levels) = grow(&c);
        assert_eq!(m[&(MOD_WIN, 0x4F)], (Action::SwitchWorkspace, 3));
        assert_eq!(levels, [Level::Error]);
        // An extra on Win+Shift+O.
        let mut c = parse_text("extra_hotkeys = MOD+SHIFT+O|reload|");
        c.main_key = MainKey::Win;
        let (m, levels) = grow(&c);
        assert_eq!(m[&(MOD_WIN | MOD_SHIFT, 0x4F)], (Action::Extra, 0));
        assert!(!m.contains_key(&(MOD_WIN, 0x4F)));
        assert_eq!(levels, [Level::Error]);
        // An extra on another O chord (Ctrl) leaves O free for grow.
        let mut c = parse_text("extra_hotkeys = MOD+CTRL+O|reload|");
        c.main_key = MainKey::Win;
        let (m, levels) = grow(&c);
        assert_eq!(m[&(MOD_WIN, 0x4F)], (Action::GrowMaster, 0));
        assert_eq!(levels, [Level::Info]);
    }

    #[test]
    fn each_conflict_rule() {
        use Level::*;
        let check = |main: MainKey, chord: &str| {
            let mut c = parse_text(&format!("extra_hotkeys = {chord}|reload|"));
            c.main_key = main;
            let k = compile_keys(&c);
            let bound = k.binds.iter().any(|b| b.action == Action::Extra);
            let levels: Vec<Level> = k
                .issues
                .iter()
                .filter(|i| i.action == Action::Extra)
                .map(|i| i.level)
                .collect();
            (bound, levels)
        };
        // No Astur key, Alt or Win: an app shortcut or typing.
        assert_eq!(check(MainKey::LAlt, "SHIFT+J"), (false, vec![Error]));
        assert_eq!(check(MainKey::Win, "J"), (false, vec![Error]));
        assert_eq!(check(MainKey::Win, "CTRL+Q"), (false, vec![Error]));
        // Windows owns these.
        assert_eq!(check(MainKey::Win, "WIN+L"), (false, vec![Error]));
        assert_eq!(check(MainKey::Win, "CTRL+ALT+DELETE"), (false, vec![Error]));
        // Reserved Left Alt + Tab is always the window switcher.
        assert_eq!(check(MainKey::LAlt, "ALT+SHIFT+TAB"), (false, vec![Error]));
        assert_eq!(check(MainKey::Win, "ALT+FOO"), (false, vec![Error]));
        // Works, with a cost: Windows' Alt+Tab, AltGr typing.
        assert_eq!(check(MainKey::Win, "ALT+TAB"), (true, vec![Warn]));
        assert_eq!(check(MainKey::Alt, "CTRL+ALT+Q"), (true, vec![Warn]));
        assert_eq!(check(MainKey::LAlt, "CTRL+ALT+Q"), (true, vec![]));
        // Replaces a Windows shortcut: a note.
        assert_eq!(check(MainKey::LAlt, "WIN+D"), (true, vec![Info]));
        assert_eq!(check(MainKey::CapsLock, "MOD+D"), (true, vec![]));
        // A reserved key held too leaves Windows' plain combo alone: no note.
        assert_eq!(check(MainKey::CapsLock, "MOD+WIN+D"), (true, vec![]));
        assert_eq!(check(MainKey::LAlt, "ALT+WIN+D"), (true, vec![]));
        assert_eq!(check(MainKey::CapsLock, "MOD+CTRL+ALT+Q"), (true, vec![]));
        assert_eq!(check(MainKey::Alt, "CTRL+ALT+WIN+Q"), (true, vec![]));
        // A chord an earlier bind has: the earlier one runs, as before.
        let mut c = default_config();
        c.workspace_keys[0] = 0x4A; // J
        let k = compile_keys(&c);
        assert!(k
            .issues
            .iter()
            .any(|i| i.action == Action::SwitchWorkspace && i.level == Warn));
        assert_eq!(keymap(&c)[&(MOD_ALT, 0x4A)], (Action::FocusNext, 0));
    }

    /// The keys a template assigns (every documented key, at its default).
    fn template_keys(text: &str) -> Vec<&str> {
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .filter_map(|l| l.split_once('=').map(|(k, _)| k.trim()))
            .collect()
    }

    #[test]
    fn every_documented_key_lands_in_a_reload_group() {
        // The LAUNCH-19 guard: a key in no group would be saved, reloaded and
        // silently never applied. Each key is set to the first of these values
        // its parser accepts as a change from the default.
        const VALUES: &[&str] = &[
            "true",
            "false",
            "3",
            "0.33",
            "#123456",
            "per_monitor",
            "master",
            "fade",
            "off",
            "smooth",
            "light",
            "primary_monitor",
            "debug",
            "icon",
            "Q W E",
            "clock",
            "x y",
            "a|b|c|d|true",
            "float|x.exe||||",
            "ALT+Q|launch|x",
            "https://x/?q={query}",
            "always",
            "nord",
            "pill",
            "clock:blue",
            "islands",
            "win",
            "dot",
        ];
        let base = parse_pair(DEFAULT_CONFIG, DEFAULT_NAVBAR);
        assert!(reload_groups(&base, &base).is_empty());
        for (is_nav, text) in [(false, DEFAULT_CONFIG), (true, DEFAULT_NAVBAR)] {
            for key in template_keys(text) {
                let changed = VALUES.iter().find_map(|v| {
                    let edited = set_conf_key(text, key, v);
                    let c = if is_nav {
                        parse_pair(DEFAULT_CONFIG, &edited)
                    } else {
                        parse_pair(&edited, DEFAULT_NAVBAR)
                    };
                    (c != base).then_some(c)
                });
                let c = changed.unwrap_or_else(|| panic!("no test value changes `{key}`"));
                let g = reload_groups(&base, &c);
                assert!(!g.is_empty(), "`{key}` is in no reload group");
                // Classified, not caught by the fail-safe.
                assert_ne!(g, ReloadGroups::full(), "`{key}` fell through to full");
            }
        }
    }

    #[test]
    fn new_bar_keys_default_to_the_bar_before_them() {
        // Old files have none of these keys, fresh ones state the defaults:
        // both must draw the bar exactly as before palettes and styles.
        for c in [Config::defaults(), default_config(), parse_text("")] {
            assert_eq!(c.bar_palette, "classic");
            assert_eq!(c.bar_workspace_style, "box");
            assert_eq!(c.bar_font_weight, 600);
            assert_eq!(
                (c.bar_surface, c.bar_accent_fg, c.bar_border),
                (None, None, None)
            );
            assert_eq!(c.bar_border_width, 0);
            assert_eq!(c.effective_bar_shape(), "strip");
            // The navbar keys after them: today's bar again.
            assert!(c.bar_clicks.is_empty());
            assert_eq!(
                (c.bar_workspace_label.as_str(), c.bar_monitors.as_str()),
                ("auto", "all")
            );
            assert_eq!(c.bar_separator_style, "line");
            assert_eq!(c.bar_title_format, "{title}");
            assert!(!c.bar_hover && !c.bar_title_bold && !c.bar_workspace_apps);
            assert_eq!(c.bar_disk_drive, "C:\\");
            assert_eq!(c.bar_custom_command, "");
            assert_eq!(c.bar_custom_interval, 10);
            assert_eq!(c.bar_monitor_mask(), !BAR_PRIMARY_ONLY);
            for (light, old) in [(false, BAR_DARK), (true, BAR_LIGHT)] {
                let r = c.bar_roles(light);
                assert_eq!([r.bg, r.fg, r.accent, r.muted], old);
                assert_eq!(r.accent_fg, old[0], "active label in the bg colour");
            }
        }
        assert!(reload_groups(&Config::defaults(), &default_config()).is_empty());
    }

    #[test]
    fn palettes_resolve_by_theme_and_pins_win() {
        let mut c = parse_text(
            "palette = Catppuccin
",
        );
        assert_eq!(c.bar_palette, "catppuccin");
        assert_eq!(c.bar_roles(false).bg, rgb(0x1E1E2E));
        assert_eq!(c.bar_roles(true).accent, rgb(0x8839EF));
        assert_eq!(rgb(0x1E1E2E), parse_color("#1E1E2E", 0));
        // A pinned colour wins over the palette in both themes.
        parse_into(
            &mut c,
            "accent = #123456
surface = #010203",
        );
        for light in [false, true] {
            let r = c.bar_roles(light);
            assert_eq!(r.accent, parse_color("#123456", 0));
            assert_eq!(r.surface, parse_color("#010203", 0));
        }
        // accent_fg on auto: the palette's, or the bar bg once bg is pinned.
        assert_eq!(c.bar_roles(false).accent_fg, rgb(0x11111B));
        parse_into(&mut c, "bg = #202122");
        assert_eq!(c.bar_roles(false).accent_fg, parse_color("#202122", 0));
        parse_into(&mut c, "accent_fg = #FFFFFF");
        assert_eq!(c.bar_roles(false).accent_fg, 0x00FF_FFFF);
        // An unknown name keeps the previous palette; lookups fall back to classic.
        parse_into(&mut c, "palette = solarized");
        assert_eq!(c.bar_palette, "catppuccin");
        assert_eq!(palette("solarized").name, "classic");
        // New colour keys have no legacy default: the old bg hex stays pinned.
        parse_into(&mut c, "surface = #1A1B26");
        assert_eq!(c.bar_surface, Some(BAR_DARK[0]));
        // Every palette is reachable by its own name.
        for p in PALETTES {
            assert_eq!(
                parse_text(&format!("palette = {}", p.name)).bar_palette,
                p.name
            );
        }
    }

    /// WCAG contrast ratio of two COLORREFs.
    fn contrast(a: u32, b: u32) -> f32 {
        let lum = |c: u32| {
            let ch = |v: u32| {
                let v = v as f32 / 255.0;
                if v <= 0.039_28 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch(c & 0xFF) + 0.7152 * ch((c >> 8) & 0xFF) + 0.0722 * ch(c >> 16)
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn text_on_picks_black_on_light_fills_and_white_on_dark() {
        assert_eq!(text_on(rgb(0xFFB900)), 0, "Windows yellow accent");
        assert_eq!(text_on(rgb(0xEBCB8B)), 0, "Nord yellow");
        assert_eq!(text_on(rgb(0xFE640B)), 0, "Latte orange: 7.0:1, not 3.0:1");
        assert_eq!(text_on(rgb(0x0078D4)), 0x00FF_FFFF, "Windows default blue");
        assert_eq!(text_on(rgb(0x000080)), 0x00FF_FFFF);
    }

    #[test]
    fn every_palette_alert_and_pill_text_is_readable() {
        for p in PALETTES {
            for (v, r) in [("dark", p.dark), ("light", p.light)] {
                // swatches[0] is the low-battery alert, drawn on the bar or
                // an island. Nord dark's own red on nord1 is 2.46:1 upstream;
                // Nord keeps its published hex (the hue still tells it apart).
                let alert = r.swatches[0];
                assert!(contrast(alert, r.bg) >= 3.0, "{} {v} alert on bg", p.name);
                if (p.name, v) != ("nord", "dark") {
                    assert!(
                        contrast(alert, r.surface) >= 3.0,
                        "{} {v} alert on surface",
                        p.name
                    );
                }
                for s in r.swatches {
                    assert!(
                        contrast(text_on(s), s) >= 3.0,
                        "{} {v}: text on {s:06X}",
                        p.name
                    );
                }
            }
        }
    }

    #[test]
    fn a_shape_line_beats_floating_in_any_order_or_file() {
        let shape = |wm: &str, nav: &str| parse_pair(wm, nav).effective_bar_shape().to_string();
        assert_eq!(shape("", "floating = true"), "floating");
        assert_eq!(shape("bar_floating = true", ""), "floating");
        assert_eq!(
            shape(
                "",
                "floating = true
shape = strip"
            ),
            "strip"
        );
        assert_eq!(
            shape(
                "",
                "shape = strip
floating = true"
            ),
            "strip"
        );
        assert_eq!(shape("bar_floating = true", "shape = islands"), "islands");
        assert_eq!(shape("shape = pills", "floating = true"), "pills");
        assert_eq!(
            shape(
                "",
                "shape = floating
floating = false"
            ),
            "floating"
        );
        // A bad value is ignored: `floating` still decides.
        assert_eq!(
            shape(
                "",
                "shape = blob
floating = true"
            ),
            "floating"
        );
        // The template's `shape =` is empty, i.e. unset, so `floating` keeps
        // working for a GUI or a hand edit that only knows it...
        let nav = set_conf_key(default_navbar_text(), "floating", "true");
        assert_eq!(shape("", &nav), "floating");
        // ...and Settings' shape write lands on that line, next to the
        // comment saying it overrides `floating`, not at the end of the file.
        let t = default_navbar_text();
        let nav = set_conf_key(t, "shape", "pills");
        assert_eq!(nav.lines().count(), t.lines().count());
        assert_eq!(shape("", &nav), "pills");
        // Switching shape re-reserves the work area (floating adds margins).
        let g = reload_groups(&Config::defaults(), &parse_text("shape = floating"));
        assert!(g.wm && g.bar);
    }

    #[test]
    fn bar_geometry_re_tiles_and_colours_only_repaint() {
        let base = Config::defaults();
        let mut c = base.clone();
        c.bar_height = 40;
        let g = reload_groups(&base, &c);
        assert!(g.wm && g.bar, "the work area shrinks, so the tiles move");
        assert!(!g.style && !g.launcher);

        let mut c = base.clone();
        c.bar_bg = Some(0x0012_3456);
        let g = reload_groups(&base, &c);
        assert_eq!(
            g,
            ReloadGroups {
                bar: true,
                ..ReloadGroups::default()
            }
        );

        let mut c = base.clone();
        c.focused_border = 0x0012_3456;
        let g = reload_groups(&base, &c);
        assert_eq!(
            g,
            ReloadGroups {
                style: true,
                ..ReloadGroups::default()
            }
        );

        // The launcher list is re-enumerated only for its own keys.
        let mut c = base.clone();
        c.popup_bg = Some(0x0012_3456);
        assert!(!reload_groups(&base, &c).launcher);
        c.launcher_entries.push(LauncherEntry {
            label: "a".into(),
            target: "b".into(),
            icon: String::new(),
        });
        assert!(reload_groups(&base, &c).launcher);
    }

    #[test]
    fn scrolling_fields_re_tile_and_keys_are_live() {
        let base = Config::defaults();
        let wm_only = ReloadGroups {
            wm: true,
            ..ReloadGroups::default()
        };
        let edits: [fn(&mut Config); 6] = [
            |c| c.monitor_layouts = vec!["scrolling".into()],
            |c| c.scroll_default_width = 0.3,
            |c| c.scroll_width_presets = vec![0.5],
            |c| c.scroll_height_presets = vec![0.5],
            |c| c.scroll_center = "always".into(),
            |c| c.tab_bar_height = 30,
        ];
        for edit in edits {
            let mut c = base.clone();
            edit(&mut c);
            assert_eq!(reload_groups(&base, &c), wm_only);
        }
        let live_only = ReloadGroups {
            live: true,
            ..ReloadGroups::default()
        };
        let keys: [fn(&mut Config); 4] = [
            |c| c.key_cycle_width = 0x51,
            |c| c.key_center_column = 0x51,
            |c| c.key_consume_left = 0x51,
            |c| c.key_consume_right = 0x51,
        ];
        for edit in keys {
            let mut c = base.clone();
            edit(&mut c);
            assert_eq!(reload_groups(&base, &c), live_only);
        }
    }

    /// A fresh directory under the system temp dir, unique per test and run.
    fn scratch_dir(test: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("astur-config-test-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn read_or_create_writes_defaults_only_when_missing() {
        let dir = scratch_dir("missing");
        let path = dir.join("sub").join("astur.conf");
        let text = read_or_create(&path, DEFAULT_CONFIG).unwrap();
        assert_eq!(text, DEFAULT_CONFIG);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_CONFIG);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_or_create_never_overwrites_an_unreadable_file() {
        // The silent data-loss bug: any read error used to write the defaults
        // over the user's file. Invalid UTF-8 and a UTF-16LE "Unicode" save
        // (what Notepad writes) must both come back as errors, byte-identical.
        let dir = scratch_dir("unreadable");
        let invalid_utf8: Vec<u8> = b"outer_gap = 12\n\xff\xfe\xfd\n".to_vec();
        let mut utf16le: Vec<u8> = vec![0xFF, 0xFE];
        for u in "outer_gap = 12\r\n".encode_utf16() {
            utf16le.extend_from_slice(&u.to_le_bytes());
        }
        for (name, bytes) in [("utf8.conf", &invalid_utf8), ("utf16.conf", &utf16le)] {
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            let err = read_or_create(&path, DEFAULT_CONFIG).unwrap_err();
            assert_eq!(err.path, path);
            assert_eq!(&std::fs::read(&path).unwrap(), bytes, "{name} was modified");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn widget_colors_parse_and_follow_the_palette() {
        let c = parse_text(
            "palette = catppuccin\nwidget_colors = cpu:Orange, mem:#112233 workspaces:blue \
             bogus:red clock:mauve title:surface volume: date:pink",
        );
        let pairs = |v: &[(&str, &str)]| -> Vec<(String, String)> {
            v.iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect()
        };
        // Not workspaces (draws its own marks), not unknown widgets or colours.
        assert_eq!(
            c.bar_widget_colors,
            pairs(&[
                ("cpu", "orange"),
                ("mem", "#112233"),
                ("title", "surface"),
                ("date", "pink")
            ])
        );
        let mocha = c.bar_roles(false);
        assert_eq!(c.bar_widget_fill("cpu", &mocha), Some(rgb(0xFAB387)));
        assert_eq!(c.bar_widget_fill("mem", &mocha), Some(rgb(0x112233)));
        assert_eq!(c.bar_widget_fill("title", &mocha), None, "surface");
        assert_eq!(c.bar_widget_fill("clock", &mocha), None, "unlisted");
        // A swatch follows the theme and the palette; a #hex stays pinned.
        assert_eq!(
            c.bar_widget_fill("cpu", &c.bar_roles(true)),
            Some(rgb(0xFE640B))
        );
        let nord = palette("nord").dark;
        assert_eq!(c.bar_widget_fill("cpu", &nord), Some(rgb(0xD08770)));
        assert_eq!(c.bar_widget_fill("mem", &nord), Some(rgb(0x112233)));
        // The last entry for a widget wins, as lines do.
        let c = parse_text("widget_colors = cpu:red cpu:blue");
        assert_eq!(c.bar_widget_fill("cpu", &mocha), Some(mocha.swatches[5]));
        assert!(parse_text("widget_colors =").bar_widget_colors.is_empty());
    }

    #[test]
    fn widget_colors_tolerate_spaces_around_the_colon() {
        let c =
            parse_text("widget_colors = cpu: orange, mem :yellow clock : blue volume: date:pink");
        let got: Vec<(&str, &str)> = c
            .bar_widget_colors
            .iter()
            .map(|(w, c)| (w.as_str(), c.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("cpu", "orange"),
                ("mem", "yellow"),
                ("clock", "blue"),
                ("date", "pink")
            ]
        );
    }

    #[test]
    fn icon_mode_accepts_glyph() {
        assert_eq!(parse_text("icon_mode = Glyph").bar_icon_mode, "glyph");
        assert_eq!(parse_text("icon_mode = emoji").bar_icon_mode, "both");
    }

    /// A preset's pairs as conf lines.
    fn preset_text(p: &BarPreset) -> String {
        p.pairs
            .iter()
            .map(|(k, v)| format!("{k} = {v}\n"))
            .collect()
    }

    /// `zone` without the widgets its show_* toggles hide: what the bar draws.
    fn visible(c: &Config, zone: &[String]) -> Vec<String> {
        let shown = |w: &str| match w {
            "apps" => c.bar_show_apps,
            "title" => c.bar_show_title,
            "layout" => c.bar_show_layout,
            "cpu" => c.bar_show_cpu,
            "mem" => c.bar_show_mem,
            "net" => c.bar_show_net,
            "volume" => c.bar_show_volume,
            "battery" => c.bar_show_battery,
            "date" => c.bar_show_date,
            "clock" => c.bar_show_clock,
            "media" => c.bar_show_media && c.media_enabled,
            _ => true,
        };
        zone.iter().filter(|w| shown(w)).cloned().collect()
    }

    #[test]
    fn presets_are_clean_complete_and_own_only_the_look() {
        let keys = |p: &BarPreset| -> Vec<&str> {
            let mut k: Vec<&str> = p
                .pairs
                .iter()
                .map(|(k, _)| *k)
                .filter(|k| !k.starts_with("show_"))
                .collect();
            k.sort_unstable();
            k
        };
        let classic_keys = keys(&BAR_PRESETS[0]);
        assert_eq!(BAR_PRESETS[0].name, "classic");
        let mut names: Vec<&str> = BAR_PRESETS.iter().map(|p| p.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), BAR_PRESETS.len(), "unique names");
        for p in BAR_PRESETS {
            let c = parse_text(&preset_text(p));
            assert!(
                c.unknown_keys.is_empty(),
                "{}: {:?}",
                p.name,
                c.unknown_keys
            );
            // The same look keys, each once, so one preset after another
            // leaves nothing behind.
            assert_eq!(keys(p), classic_keys, "{}", p.name);
            let mut all: Vec<&str> = p.pairs.iter().map(|(k, _)| *k).collect();
            all.sort_unstable();
            all.dedup();
            assert_eq!(all.len(), p.pairs.len(), "{}: a key twice", p.name);
            for k in [
                "enabled",
                "bottom",
                "autohide",
                "wheel_workspaces",
                "hide_empty_workspaces",
                "floating",
                "media_enabled",
                "preset",
            ] {
                assert!(all.iter().all(|a| *a != k), "{} sets {k}", p.name);
            }
            let v = |k: &str| p.pairs.iter().find(|(a, _)| *a == k).map(|(_, v)| *v);
            // Values the parser would drop silently (keeping the old one).
            assert_eq!(Some(c.effective_bar_shape()), v("shape"), "{}", p.name);
            assert_eq!(Some(c.bar_palette.as_str()), v("palette"), "{}", p.name);
            assert_eq!(Some(c.bar_workspace_style.as_str()), v("workspace_style"));
            assert_eq!(Some(c.bar_icon_mode.as_str()), v("icon_mode"));
            let tokens = |k: &str| v(k).map_or(0, |s| s.split_whitespace().count());
            assert_eq!(c.bar_widget_colors.len(), tokens("widget_colors"));
            for (zone, list) in [
                ("left", &c.bar_left),
                ("center", &c.bar_center),
                ("right", &c.bar_right),
            ] {
                assert_eq!(list.len(), tokens(zone), "{}: unknown widget", p.name);
                for w in list {
                    assert_ne!(w, "media", "{}: media needs media_enabled", p.name);
                    // Zone membership is visibility: every listed widget on
                    // (those without a show_* key are on by being listed).
                    let unswitched = [
                        "workspaces",
                        "separator",
                        "spacer",
                        "keyboard",
                        "disk",
                        "uptime",
                        "custom",
                        "media_controls",
                        "logo",
                    ];
                    if !unswitched.contains(&w.as_str()) {
                        let key = format!("show_{w}");
                        assert_eq!(v(&key), Some("true"), "{}: {key}", p.name);
                    }
                }
                assert_eq!(&visible(&c, list), list, "{}", p.name);
            }
        }
    }

    #[test]
    fn a_preset_stamps_into_a_file_and_reads_back() {
        // What the settings GUI does: apply_updates onto the user's file.
        for p in BAR_PRESETS {
            let stamped = apply_updates(
                DEFAULT_NAVBAR,
                &p.pairs
                    .iter()
                    .map(|(k, v)| (*k, v.to_string()))
                    .collect::<Vec<_>>(),
            );
            let got = parse_pair(DEFAULT_CONFIG, &stamped);
            assert!(got.unknown_keys.is_empty(), "{}", p.name);
            // The same as the pairs written last, whatever the file had.
            let want = parse_pair(
                DEFAULT_CONFIG,
                &format!("{DEFAULT_NAVBAR}\n{}", preset_text(p)),
            );
            assert!(got == want, "{}", p.name);
            // Stamping again changes nothing.
            let again = apply_updates(
                &stamped,
                &p.pairs
                    .iter()
                    .map(|(k, v)| (*k, v.to_string()))
                    .collect::<Vec<_>>(),
            );
            assert_eq!(again, stamped, "{}", p.name);
        }
    }

    #[test]
    fn the_classic_preset_draws_the_default_bar() {
        let p = &BAR_PRESETS[0];
        let mut got = parse_pair(
            DEFAULT_CONFIG,
            &format!("{DEFAULT_NAVBAR}\n{}", preset_text(p)),
        );
        let mut want = default_config();
        // Classic lists only what the default shows (apps, net and date are
        // listed but switched off there), and spells out `shape = strip`.
        for c in [&mut got, &mut want] {
            c.bar_left = visible(c, &c.bar_left);
            c.bar_center = visible(c, &c.bar_center);
            c.bar_right = visible(c, &c.bar_right);
            c.bar_shape = c.effective_bar_shape().to_string();
        }
        assert!(got == want);
    }

    #[test]
    fn click_keys_parse_and_fall_back_to_the_defaults() {
        let c = parse_text(
            "cpu_right_click = launch wt -p Ubuntu\ncpu_click = Mute\nclock_middle_click = none\n",
        );
        assert!(c.unknown_keys.is_empty(), "{:?}", c.unknown_keys);
        assert_eq!(c.bar_click("cpu", 1), ("launch", "wt -p Ubuntu"));
        assert_eq!(c.bar_click("cpu", 0), ("mute", ""));
        assert_eq!(c.bar_click("clock", 2), ("none", ""));
        // Buttons without a line keep their default; a widget with none
        // listed does nothing.
        assert_eq!(c.bar_click("clock", 0), ("calendar", ""));
        assert_eq!(c.bar_click("title", 0), ("none", ""));
        assert_eq!(c.bar_click("nonsense", 1), ("none", ""));
        // An unknown action keeps the previous value; empty is the default
        // again; the last line wins.
        let c = parse_text("volume_click = sound_settings\nvolume_click = explode\n");
        assert_eq!(c.bar_click("volume", 0), ("sound_settings", ""));
        let c = parse_text("volume_click = sound_settings\nvolume_click =\n");
        assert_eq!(c.bar_click("volume", 0), ("mute", ""));
        let c = parse_text("date_click = launcher\ndate_click = switch_workspace 3\n");
        assert_eq!(c.bar_click("date", 0), ("switch_workspace", "3"));
        assert_eq!(c.bar_clicks.len(), 1);
        // The right / middle suffixes are stripped before _click; widgets
        // with fixed clicks have no keys.
        assert_eq!(click_key("cpu_right_click"), Some(("cpu", 1)));
        assert_eq!(click_key("media_middle_click"), Some(("media", 2)));
        assert_eq!(click_key("logo_click"), Some(("logo", 0)));
        assert_eq!(click_key("workspaces_click"), None);
        assert_eq!(click_key("media_controls_click"), None);
        assert!(!parse_text("workspaces_click = mute")
            .unknown_keys
            .is_empty());
        // Every default is a real action on a real widget.
        assert_eq!(BAR_ACTIONS[0].0, "none");
        for w in BAR_CLICK_WIDGETS {
            assert!(BAR_WIDGETS.contains(w), "{w}");
        }
        for (w, acts) in BAR_CLICK_DEFAULTS {
            assert!(BAR_CLICK_WIDGETS.contains(w), "{w}");
            for a in acts {
                assert!(BAR_ACTIONS.iter().any(|x| x.0 == *a), "{w}: {a}");
            }
        }
    }

    #[test]
    fn monitors_pick_screens_by_number_or_the_primary() {
        let mask = |v: &str| parse_text(&format!("monitors = {v}")).bar_monitor_mask();
        for i in 0..8 {
            assert!(bar_on(mask("all"), i, i == 3));
        }
        assert!(bar_on(mask("primary"), 2, true));
        assert!(!bar_on(mask("primary"), 0, false));
        assert_eq!(parse_text("monitors = 3, 1").bar_monitors, "1 3");
        let m = mask("3, 1");
        assert!(bar_on(m, 0, false) && !bar_on(m, 1, true) && bar_on(m, 2, false));
        assert!(!bar_on(m, 63, false), "past the mask: no bar, no panic");
        // Anything else keeps the previous value.
        let keep =
            |v: &str| parse_text(&format!("monitors = primary\nmonitors = {v}")).bar_monitors;
        for v in ["left", "0", "64", "1 x"] {
            assert_eq!(keep(v), "primary", "{v}");
        }
        assert_eq!(parse_text("bar_monitors = Main").bar_monitors, "primary");
    }

    #[test]
    fn workspace_labels_fall_back_to_the_number() {
        assert_eq!(workspace_label("auto", 3, "Web", ""), "Web");
        assert_eq!(workspace_label("auto", 3, "Web", "W"), "W");
        assert_eq!(workspace_label("auto", 3, "", ""), "3");
        assert_eq!(workspace_label("number", 3, "Web", "W"), "3");
        assert_eq!(workspace_label("name", 3, "", "W"), "3");
        assert_eq!(workspace_label("icon", 3, "Web", ""), "3");
        assert_eq!(workspace_label("icon_name", 3, "Web", "W"), "W Web");
        assert_eq!(workspace_label("icon_name", 3, "Web", ""), "Web");
        assert_eq!(
            parse_text("workspace_label = Icon_Name").bar_workspace_label,
            "icon_name"
        );
        assert_eq!(
            parse_text("workspace_label = big").bar_workspace_label,
            "auto"
        );
    }

    #[test]
    fn clock_and_date_formats_keep_old_output_and_add_tokens() {
        // Golden values of the WM's formatters before they moved here.
        assert_eq!(format_clock("HH:mm", true, 9, 5, 7), "09:05");
        assert_eq!(format_clock("", true, 21, 41, 0), "21:41");
        assert_eq!(format_clock("", false, 21, 41, 0), "9:41 pm");
        assert_eq!(format_clock("h:mm tt", true, 0, 1, 0), "12:01 am");
        assert_eq!(format_clock("hh:mm", true, 13, 0, 0), "01:00");
        assert_eq!(format_clock("H.mm", true, 7, 30, 0), "7.30");
        assert_eq!(format_date("ddd dd MMM", 2026, 10, 4, 0), "Sun 04 Oct");
        assert_eq!(format_date("yyyy-MM-dd", 2026, 1, 9, 5), "2026-01-09");
        assert_eq!(format_date("dd/MM/yy", 2026, 12, 31, 4), "31/12/26");
        assert_eq!(format_date("MMM dd, yyyy", 2026, 9, 1, 2), "Sep 01, 2026");
        assert_eq!(format_date("Today ddd", 2026, 9, 1, 2), "Today Tue");
        assert_eq!(format_date("MMM ddd", 2026, 13, 7, 9), " ");
        // The new tokens.
        assert_eq!(format_clock("HH:mm:ss", true, 9, 5, 7), "09:05:07");
        assert_eq!(format_clock("h:mm TT", true, 21, 41, 0), "9:41 PM");
        assert_eq!(format_clock("h:mm:ss TT", true, 12, 0, 59), "12:00:59 PM");
        assert_eq!(format_date("ddd d MMM", 2026, 10, 4, 6), "Sat 4 Oct");
        assert_eq!(
            format_date("dddd d MMMM", 2026, 10, 24, 6),
            "Saturday 24 October"
        );
        assert_eq!(format_date("d/MM/yyyy", 2026, 3, 7, 6), "7/03/2026");
    }

    #[test]
    fn disk_drive_takes_a_letter_or_a_folder() {
        assert_eq!(disk_root("c").as_deref(), Some("C:\\"));
        assert_eq!(disk_root("C:").as_deref(), Some("C:\\"));
        assert_eq!(disk_root("d:\\").as_deref(), Some("D:\\"));
        assert_eq!(disk_root("D:\\Games").as_deref(), Some("D:\\Games\\"));
        assert_eq!(
            disk_root("\\\\nas\\share").as_deref(),
            Some("\\\\nas\\share\\")
        );
        assert_eq!(disk_root("  "), None);
        assert_eq!(parse_text("disk_drive =").bar_disk_drive, "C:\\");
        assert_eq!(parse_text("custom_interval = 0").bar_custom_interval, 1);
        assert_eq!(
            parse_text("custom_interval = 99999").bar_custom_interval,
            3600
        );
    }
}

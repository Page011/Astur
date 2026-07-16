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
    pub workspaces: usize,        // workspaces per monitor (1..10)
    pub workspace_keys: Vec<u32>, // VK code per workspace; Alt+key switches, +Shift moves
    pub workspace_names: Vec<String>, // optional display names, in workspace order
    pub workspace_icons: Vec<String>, // optional short labels/icons, in workspace order
    pub layout: String,           // dwindle | master | columns | grid | monocle
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
    pub persist_state: bool,
    pub extra_hotkeys: Vec<HotkeyDef>,
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
];

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
            persist_state: true,
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
# Apply    : save this file; Astur hot-reloads it within about one second.
# Regen    : delete this file and relaunch to get a fresh, fully-commented copy.
#
# Syntax   : one  key = value  per line. '#' starts a comment. Blank lines and
#            surrounding whitespace are ignored. Unknown keys are ignored, so a
#            typo silently falls back to the default below rather than erroring.
#
# Value types:
#   bool   : true / false   (also accepts yes/no, 1/0, on/off; anything else
#            counts as false)
#   int    : whole number; clamped to the stated range
#   float  : decimal; clamped to the stated range
#   colour : #RRGGBB hex (e.g. #66AAFF). Malformed values keep the default.
#   text   : literal string (program name / command line)
#   keys   : space/comma list of key names: 0-9, A-Z, F1-F24.
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

# Number of workspaces.  int 1 - 10
#   shared mode      = TOTAL across all monitors (distributed from the main
#                      monitor outward).
#   per_monitor mode = workspaces per monitor.
workspaces = 10

# Keys (with LEFT ALT) that select workspaces 1, 2, 3, ... in order. Add Shift
# to MOVE the focused window to that workspace instead. Avoid the fixed binds
# (J K H L M T F W and arrows/Enter). Example for Alt+Q/W/E = ws 1/2/3:
#   workspace_keys = Q W E R T Y
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
# values: dwindle | master | columns | grid | monocle
layout = dwindle

# Fraction of the width given to the master window (master layout, and the
# master split that Alt+H / Alt+L adjust).  float 0.10 - 0.90
master_ratio = 0.55

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
# Window open/close/move/resize placement is currently instant.

# Enable animations.  bool
animations = true
# Animation duration in milliseconds. Lower = snappier, higher = smoother.
# 0 disables (same as animations = false).  int 0-2000
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
#   glide - windows glide from their old position to the new tile slot. Opening a
#           window glides it in from where it spawned; closing reflows the rest.
#           Composited on a brief overlay (the real windows are placed instantly
#           underneath), so it stays smooth even with heavy apps.  string
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
# Popup typography and geometry. Sizes are pixels in current process DPI mode.
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
window_rules =

# ---------------------------------------------------------------------------
# Launcher and system menu
# ---------------------------------------------------------------------------

# Program launched by Alt+Enter. Resolved via the shell, so PATH and App
# Execution Aliases (e.g. wt.exe) work.  text
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
# Local named-pipe command API. Pipe name only; no remote/network listener.
ipc_enabled = false
ipc_pipe = astur
# Extra bindings: chord|action|argument ;; chord|action|argument
# Escape literal field separators with a leading backslash.
extra_hotkeys = ALT+GRAVE|scratchpad|

# ============================================================================
# Hotkeys (LEFT ALT is the modifier)
# ============================================================================
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
#   RIGHT ALT            normal Alt behaviour (LEFT ALT is reserved by Astur)
#
# The letter keys above (J K H L M T F W) are rebindable. Each takes a single
# key name (see the 'keys' type at the top of this file). Arrows and Enter
# are fixed.  key
key_focus_next = J
key_focus_prev = K
key_shrink_master = H
key_grow_master = L
key_promote_master = M
key_toggle_tiling = T
key_toggle_float = F
key_close_window = W
# ============================================================================
";

const DEFAULT_NAVBAR: &str = "\
# ============================================================================
# Astur navbar configuration  (status bar)
# ============================================================================
# Location : %USERPROFILE%\\.astur\\navbar.conf
#            (override with the ASTUR_NAVBAR environment variable)
# Window-manager settings live separately in astur.conf (same folder).
# Apply    : save this file; Astur hot-reloads it within about one second.
#
# One bar is drawn on EVERY monitor. Each shows that monitor's workspaces and
# focused window. The tiling work area is reserved so windows never sit under a
# bar. Click a workspace pill to switch to it.
#
# Value types: bool, int, colour (#RRGGBB) -- see astur.conf for details.
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

# ---------------------------------------------------------------------------
# Style
# ---------------------------------------------------------------------------
# Floating bar: detached from the screen edge with a margin and rounded
# corners (waybar/Hyprland style). false = classic full-width strip.  bool
floating = false
# Gap between a floating bar and the screen edges, in px.  int 0 - 200
margin = 8
# Floating-bar corner radius, in px.  int 0 - 40
radius = 12
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
#   separator   thin divider
#   spacer      fixed gap
# A widget only shows if it is BOTH listed in a zone and its show_* toggle
# below is true (so you can flip widgets without re-ordering).
# ---------------------------------------------------------------------------
left = workspaces apps
center = title
right = layout cpu mem net volume battery date clock

# ---------------------------------------------------------------------------
# Behaviour
# ---------------------------------------------------------------------------
# Mouse wheel over the bar switches to the previous/next workspace (the wheel
# over the volume widget always adjusts volume instead).  bool
wheel_workspaces = true
# Hide empty workspace pills, showing only the active one and those with
# windows. false = show every workspace the monitor owns.  bool
hide_empty_workspaces = false

# ---------------------------------------------------------------------------
# Widget toggles
# ---------------------------------------------------------------------------
# Show the focused window title.  bool
show_title = true
# Show the layout name + tiling/floating state.  bool
show_layout = true
# Show the clock.  bool
show_clock = true
# 24-hour clock; false = 12-hour with am/pm.  bool
clock_24h = true
# Show the date.  bool
show_date = false
# Date format tokens:  yyyy yy  MMM MM  ddd dd  (e.g. \"ddd dd MMM\" -> Fri 19 Jun)
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
# icon = label only; text = value only; both = label + value
icon_mode = both
show_tooltips = true
show_app_labels = false
cpu_format = {value}%
mem_format = {value}%
battery_format = {value}%
net_format = D:{down} U:{up}
volume_format = {value}%
clock_format = HH:mm
# Plain labels work everywhere; icon-font glyphs work when font_name supports them.
icon_cpu = CPU
icon_mem = RAM
icon_battery = BAT
icon_net = NET
icon_volume = VOL

# Colours: 'auto' follows the theme (astur.conf 'theme' picks the dark or light
# preset), or set an explicit #RRGGBB to override that colour permanently.
bg = auto
fg = auto
# Active-workspace pill highlight.
accent = auto
# Empty workspaces / layout / stats text.
inactive = auto
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
        if ch == '\\'
            && i + 1 < chars.len()
            && chars[i + 1] == '\\'
            && (i + 2 == chars.len()
                || chars[i + 2] == '|'
                || (chars[i + 2] == ';' && i + 3 < chars.len() && chars[i + 3] == ';'))
        {
            field.push('\\');
            i += 2;
            continue;
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
/// Map a key name ("1", "Q", "F3") to its Win32 virtual-key code.
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
    None
}

/// Parse a space/comma-separated navbar zone list, keeping only known widget
/// names (typos vanish rather than erroring, matching the rest of the parser).
fn parse_widgets(v: &str) -> Vec<String> {
    v.split([',', ' ', '\t'])
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| BAR_WIDGETS.contains(&s.as_str()))
        .collect()
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

/// Read a config file, writing `default` the first time it is missing.
fn read_or_create(path: &std::path::Path, default: &str) -> String {
    match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(path, default);
            println!("wrote default config: {}", path.display());
            default.to_string()
        }
    }
}

/// Load settings from astur.conf (window manager) and navbar.conf (status
/// bar), creating each with documented defaults when missing.
pub fn load_config() -> Config {
    let mut c = Config::defaults();
    let wm = config_path("ASTUR_CONFIG", "astur.conf");
    parse_into(&mut c, &read_or_create(&wm, DEFAULT_CONFIG));
    let nav = config_path("ASTUR_NAVBAR", "navbar.conf");
    parse_into(&mut c, &read_or_create(&nav, DEFAULT_NAVBAR));
    c
}

/// Apply `key = value` lines from `text` onto `c`. Unknown keys are ignored.
/// Recognises both the window-manager keys (astur.conf) and the navbar keys
/// (navbar.conf, unprefixed) so either file may set either, and old configs that
/// used the `bar_*` names keep working.
fn parse_into(c: &mut Config, text: &str) {
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
                    c.workspaces = n.clamp(1, 10);
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
                if matches!(
                    m.as_str(),
                    "dwindle" | "master" | "columns" | "grid" | "monocle"
                ) {
                    c.layout = m;
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
            "ipc_pipe" => {
                if !v.is_empty() {
                    c.ipc_pipe = v.to_string()
                }
            }
            "persist_state" => c.persist_state = parse_bool(v),
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
                if matches!(m.as_str(), "icon" | "text" | "both") {
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
            _ => {}
        }
    }
}

/// Rewrite one `key = value` assignment in conf text, preserving every other
/// line (comments and layout included). The first active (non-comment)
/// assignment of `key` is replaced in place; later duplicates are left alone
/// (the parser is last-write-wins, so the GUI also normalises: see
/// `apply_updates`). If the key is missing, `key = value` is appended.
pub fn set_conf_key(text: &str, key: &str, value: &str) -> String {
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
    parse_into(&mut c, wm_text);
    parse_into(&mut c, nav_text);
    c
}

/// Format a COLORREF (0x00BBGGRR) back to a `#RRGGBB` config string.
pub fn color_to_hex(c: u32) -> String {
    let r = c & 0xFF;
    let g = (c >> 8) & 0xFF;
    let b = (c >> 16) & 0xFF;
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// Map a VK code back to its config key name ("A".."Z", "0".."9", "F1".."F24").
pub fn vk_to_key(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).unwrap_or('?').to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x70 + 1),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn parse_into_navbar_alias_and_clamps() {
        let mut c = Config::defaults();
        parse_into(&mut c, "workspaces = 50\nbar_height = 999\nheight = 40");
        assert_eq!(c.workspaces, 10); // clamp 1..=10
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
    fn vk_roundtrip() {
        for name in ["A", "Z", "0", "9", "F1", "F24"] {
            let vk = key_to_vk(name).unwrap();
            assert_eq!(vk_to_key(vk), name);
        }
    }
}

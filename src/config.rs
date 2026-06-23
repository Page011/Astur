//! Runtime configuration: the `Config` struct, its documented-default file
//! templates, and the key/value parser. No Win32 — pure data and string work.

/// Runtime configuration, loaded from astur.conf + navbar.conf at startup.
#[derive(Clone)]
pub(crate) struct Config {
    pub(crate) per_monitor: bool,          // true: Alt+1..9 switches focused monitor only
    pub(crate) start_tiled: bool,          // tile automatically on launch
    pub(crate) outer_gap: i32,             // gap between windows and screen edge
    pub(crate) inner_gap: i32,             // gap between adjacent windows
    pub(crate) master_ratio: f32,          // fraction of width given to the master window
    pub(crate) workspaces: usize,          // workspaces per monitor (1..10)
    pub(crate) workspace_keys: Vec<u32>,   // VK code per workspace; Alt+key switches, +Shift moves
    pub(crate) layout: String,             // "dwindle" (spiral into a corner) or "master"
    pub(crate) terminal: String,           // command launched by Alt+Enter
    pub(crate) browser: String,            // Alt+Shift+Enter; empty = default browser
    pub(crate) unfocused_opacity: f32,     // 0.0-1.0 alpha for unfocused windows (1.0 = off)
    pub(crate) border_enabled: bool,       // draw coloured DWM borders (Windows 11)
    pub(crate) focused_border: u32,        // COLORREF for the focused window border
    pub(crate) unfocused_border: u32,      // COLORREF for unfocused window borders
    pub(crate) cursor_follows_focus: bool, // warp the mouse to the focused window
    pub(crate) focus_follows_mouse: bool,  // hovering a window focuses it (focus follows mouse)
    pub(crate) animations: bool,           // animate tiling moves + workspace slides
    pub(crate) animation_ms: i32,          // animation duration in ms (0 disables; clamp 0..2000)
    pub(crate) workspace_slide: bool,      // GPU thumbnail slide transition on workspace switch
    pub(crate) bar_enabled: bool,          // draw the status bar on every monitor
    pub(crate) bar_height: i32,            // bar thickness in px (work area is reserved for it)
    pub(crate) bar_bottom: bool,           // dock the bar at the bottom instead of the top
    pub(crate) bar_font_size: i32,         // text height in px; 0 = auto from bar_height
    pub(crate) bar_show_title: bool,       // show the focused window title
    pub(crate) bar_show_clock: bool,       // show the clock
    pub(crate) bar_clock_24h: bool,        // 24-hour clock (false = 12-hour with am/pm)
    pub(crate) bar_show_layout: bool,      // show layout + tiling/floating state on the right
    pub(crate) bar_bg: u32,                // COLORREF bar background
    pub(crate) bar_fg: u32,                // COLORREF bar text
    pub(crate) bar_accent: u32,            // COLORREF active-workspace highlight
    pub(crate) bar_inactive: u32,          // COLORREF empty-workspace text
    pub(crate) bar_font_name: String,      // font family (default "Segoe UI")
    pub(crate) bar_hide_empty: bool,       // hide empty workspace pills
    pub(crate) bar_padding: i32,           // horizontal padding from each screen edge (px)
    pub(crate) bar_show_date: bool,        // show the date widget
    pub(crate) bar_date_format: String,    // date token string, e.g. "ddd dd MMM"
    pub(crate) bar_show_cpu: bool,         // show CPU load %
    pub(crate) bar_show_mem: bool,         // show RAM load %
    pub(crate) bar_show_battery: bool,     // show battery %
    pub(crate) ignore_classes: Vec<String>, // window classes never tiled/managed
    pub(crate) float_classes: Vec<String>,   // window classes managed but auto-floated
    pub(crate) key_focus_next: u32,        // Alt+<key> focus next window in the stack (default J)
    pub(crate) key_focus_prev: u32,        // Alt+<key> focus previous window in the stack (default K)
    pub(crate) key_shrink_master: u32,     // Alt+<key> shrink the master area (default H)
    pub(crate) key_grow_master: u32,       // Alt+<key> grow the master area (default L)
    pub(crate) key_promote_master: u32,    // Alt+<key> promote focused window to master (default M)
    pub(crate) key_toggle_tiling: u32,     // Alt+<key> toggle tiling on/off (default T)
    pub(crate) key_toggle_float: u32,      // Alt+<key> toggle floating for focused window (default F)
    pub(crate) key_close_window: u32,      // Alt+<key> close the focused window (default W)
}

impl Config {
    pub(crate) fn defaults() -> Self {
        Config {
            per_monitor: false,
            start_tiled: true,
            outer_gap: 8,
            inner_gap: 8,
            master_ratio: 0.55,
            workspaces: 9,
            workspace_keys: parse_keys("1 2 3 4 5 6 7 8 9 0"),
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
            bar_enabled: true,
            bar_height: 28,
            bar_bottom: false,
            bar_font_size: 0,
            bar_show_title: true,
            bar_show_clock: true,
            bar_clock_24h: true,
            bar_show_layout: true,
            bar_bg: parse_color("#1A1B26", 0x00261B1A),
            bar_fg: parse_color("#C0CAF5", 0x00F5CAC0),
            bar_accent: parse_color("#66AAFF", 0x00FFAA66),
            bar_inactive: parse_color("#565F89", 0x00895F56),
            bar_font_name: "Segoe UI".to_string(),
            bar_hide_empty: false,
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
# Apply    : edit this file, then restart Astur.
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

# Tile windows automatically on launch (Alt+T toggles at runtime).  bool
start_tiled = true

# Tiling layout.
#   dwindle = each new window splits the remaining space, spiralling into the
#             bottom corner (spiral default).
#   master  = one large master column on the left, the rest stacked on the right.
# values: dwindle | master
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
# Opening, moving, and re-tiling glide windows to their target. Closing is
# instant (the app destroys the window before it can be animated).

# Enable animations.  bool
animations = true
# Animation duration in milliseconds. Lower = snappier, higher = smoother but
# more window traffic. 0 disables (same as animations = false).  int 0-2000
animation_ms = 140
# Workspace-switch slide. Freezes the screen on an overlay and slides GPU-
# composited thumbnails of the new workspace in from the side — smooth even with
# heavy apps (the apps themselves aren't moved). false = instant switch. Needs
# animations = true.  bool
workspace_slide = true

# ---------------------------------------------------------------------------
# Appearance: window borders & dimming
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# Launchers
# ---------------------------------------------------------------------------

# Program launched by Alt+Enter. Resolved via the shell, so PATH and App
# Execution Aliases (e.g. wt.exe) work.  text
terminal = wt.exe
# Program launched by Alt+Shift+Enter. Leave EMPTY to open the system default
# browser.  text
browser =

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
# Apply    : edit this file, then restart Astur.
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
# Horizontal padding from each screen edge, in px.  int 0 - 200
padding = 8
# Font family for all bar text.  text  (e.g. Segoe UI, Cascadia Code, Consolas)
font_name = Segoe UI
# Text height in px. 0 = auto (about half the bar height).  int 0 - 100
font_size = 0

# ---------------------------------------------------------------------------
# Workspaces
# ---------------------------------------------------------------------------
# Hide empty workspace pills, showing only the active one and those with
# windows (spiral style). false = show every workspace the monitor
# owns.  bool
hide_empty_workspaces = false

# ---------------------------------------------------------------------------
# Widgets (right-hand cluster, drawn right-to-left in this order:
#   clock, date, battery, mem, cpu, layout)
# ---------------------------------------------------------------------------
# Show the focused window title (centre).  bool
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

# Colours (#RRGGBB).
bg = #1A1B26
fg = #C0CAF5
# Active-workspace pill highlight.
accent = #66AAFF
# Empty workspaces / layout / stats text.
inactive = #565F89
";

pub(crate) fn parse_bool(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "1" | "on")
}

/// Parse a comma/semicolon-separated list of window-class names, trimmed.
fn parse_list(v: &str) -> Vec<String> {
    v.split([',', ';'])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Map a key name ("1", "Q", "F3") to its Win32 virtual-key code.
pub(crate) fn key_to_vk(name: &str) -> Option<u32> {
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
pub(crate) fn config_path(env: &str, name: &str) -> std::path::PathBuf {
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
pub(crate) fn load_config() -> Config {
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
            "layout" => c.layout = v.to_ascii_lowercase(),
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
            "bg" | "bar_bg" => c.bar_bg = parse_color(v, c.bar_bg),
            "fg" | "bar_fg" => c.bar_fg = parse_color(v, c.bar_fg),
            "accent" | "bar_accent" => c.bar_accent = parse_color(v, c.bar_accent),
            "inactive" | "bar_inactive" => c.bar_inactive = parse_color(v, c.bar_inactive),
            "font_name" | "bar_font_name" => {
                if !v.is_empty() {
                    c.bar_font_name = v.to_string();
                }
            }
            "hide_empty" | "hide_empty_workspaces" | "bar_hide_empty" => {
                c.bar_hide_empty = parse_bool(v)
            }
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
            _ => {}
        }
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
    fn parse_into_gaps_clamped() {
        let mut c = Config::defaults();
        parse_into(&mut c, "outer_gap = -10\ninner_gap = 99999");
        assert_eq!(c.outer_gap, 0); // negative clamped up
        assert_eq!(c.inner_gap, 500); // huge clamped down
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
}

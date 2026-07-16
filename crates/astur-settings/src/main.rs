// Astur Settings — friendly GUI editor for astur.conf / navbar.conf.
//
// Runs as a SEPARATE process from the window manager (a GUI crash must never
// touch the input hooks or the manager). It edits the same two config files the
// WM watches, via the shared `astur-config` crate, so the parser and the GUI can
// never drift. Saving rewrites only the `key = value` lines (comments and layout
// are preserved) and the WM hot-reloads within a second — no restart.
//
// No console in release (launched from the WM's tray; a console window would
// flash and could briefly be tiled).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use astur_config::{
    apply_updates, color_to_hex, config_path, default_config_text, default_navbar_text,
    format_hotkeys, format_launcher_entries, format_system_actions, format_window_rules, key_to_vk,
    parse_hotkeys, parse_launcher_entries, parse_pair, parse_system_actions, parse_window_rules,
    vk_to_key, Config, BAR_DARK, BAR_WIDGETS,
};
use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([940.0, 660.0])
            .with_min_inner_size([760.0, 480.0])
            .with_title("Astur Settings"),
        ..Default::default()
    };
    eframe::run_native(
        "Astur Settings",
        options,
        Box::new(|_cc| Ok(Box::new(App::load()))),
    )
}

#[derive(Clone, Copy, PartialEq)]
enum Section {
    General,
    Layout,
    Focus,
    Animations,
    Appearance,
    Launcher,
    SystemMenu,
    Desktop,
    Bar,
    Widgets,
    Hotkeys,
    Rules,
    About,
}

const SECTIONS: &[(Section, &str)] = &[
    (Section::General, "General"),
    (Section::Layout, "Layout & gaps"),
    (Section::Focus, "Focus & mouse"),
    (Section::Animations, "Animations"),
    (Section::Appearance, "Appearance"),
    (Section::Launcher, "App picker"),
    (Section::SystemMenu, "System menu"),
    (Section::Desktop, "Desktop tools"),
    (Section::Bar, "Bar"),
    (Section::Widgets, "Bar widgets"),
    (Section::Hotkeys, "Hotkeys"),
    (Section::Rules, "Window rules"),
    (Section::About, "About"),
];

/// Text mirrors for fields the Config stores as lists/VK codes. Edited as text,
/// validated + folded back into the Config on save.
#[derive(Clone, PartialEq)]
struct Mirrors {
    ws_keys: String,
    keys: [String; 8], // focus_next, focus_prev, shrink, grow, promote, tiling, float, close
    workspace_names: String,
    workspace_icons: String,
    workspace_wallpapers: String,
    ignore: String,
    float: String,
    rich_rules: String,
    launcher_excludes: String,
    launcher_entries: String,
    system_power: String,
    system_setup: String,
    system_actions: String,
    extra_hotkeys: String,
    zone_l: String,
    zone_c: String,
    zone_r: String,
}

impl Mirrors {
    fn from(cfg: &Config) -> Self {
        Mirrors {
            ws_keys: cfg
                .workspace_keys
                .iter()
                .map(|&k| vk_to_key(k))
                .collect::<Vec<_>>()
                .join(" "),
            keys: [
                vk_to_key(cfg.key_focus_next),
                vk_to_key(cfg.key_focus_prev),
                vk_to_key(cfg.key_shrink_master),
                vk_to_key(cfg.key_grow_master),
                vk_to_key(cfg.key_promote_master),
                vk_to_key(cfg.key_toggle_tiling),
                vk_to_key(cfg.key_toggle_float),
                vk_to_key(cfg.key_close_window),
            ],
            workspace_names: cfg.workspace_names.join(", "),
            workspace_icons: cfg.workspace_icons.join(", "),
            workspace_wallpapers: cfg.workspace_wallpapers.join("\n"),
            ignore: cfg.ignore_classes.join(", "),
            float: cfg.float_classes.join(", "),
            rich_rules: format_window_rules(&cfg.window_rules),
            launcher_excludes: cfg.launcher_file_exclude.join(", "),
            launcher_entries: format_launcher_entries(&cfg.launcher_entries),
            system_power: cfg.system_power_items.join(", "),
            system_setup: cfg.system_setup_items.join(", "),
            system_actions: format_system_actions(&cfg.system_actions),
            extra_hotkeys: format_hotkeys(&cfg.extra_hotkeys),
            zone_l: cfg.bar_left.join(" "),
            zone_c: cfg.bar_center.join(" "),
            zone_r: cfg.bar_right.join(" "),
        }
    }
}

struct App {
    cfg: Config,
    mir: Mirrors,
    saved_cfg: Config,
    saved_mir: Mirrors,
    section: Section,
    saved_at: Option<std::time::Instant>,
    error: Option<String>,
}

impl App {
    fn load() -> Self {
        let (wm, nav) = read_confs();
        let cfg = parse_pair(&wm, &nav);
        let mir = Mirrors::from(&cfg);
        App {
            saved_cfg: cfg.clone(),
            saved_mir: mir.clone(),
            cfg,
            mir,
            section: Section::General,
            saved_at: None,
            error: None,
        }
    }

    fn dirty(&self) -> bool {
        self.cfg != self.saved_cfg || self.mir != self.saved_mir
    }

    /// Fold the text mirrors back into the Config (dropping invalid tokens) so
    /// what gets written is exactly what the WM will parse.
    fn fold_mirrors(&mut self) {
        let keys: Vec<u32> = self
            .mir
            .ws_keys
            .split([' ', ','])
            .filter_map(|s| key_to_vk(s.trim()))
            .collect();
        if !keys.is_empty() {
            self.cfg.workspace_keys = keys;
        }
        let binds = [
            &mut self.cfg.key_focus_next,
            &mut self.cfg.key_focus_prev,
            &mut self.cfg.key_shrink_master,
            &mut self.cfg.key_grow_master,
            &mut self.cfg.key_promote_master,
            &mut self.cfg.key_toggle_tiling,
            &mut self.cfg.key_toggle_float,
            &mut self.cfg.key_close_window,
        ];
        for (slot, text) in binds.into_iter().zip(&self.mir.keys) {
            if let Some(vk) = key_to_vk(text.trim()) {
                *slot = vk;
            }
        }
        let list = |s: &str| -> Vec<String> {
            s.split([',', ';'])
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        };
        self.cfg.workspace_names = list(&self.mir.workspace_names);
        self.cfg.workspace_icons = list(&self.mir.workspace_icons);
        self.cfg.workspace_wallpapers = self
            .mir
            .workspace_wallpapers
            .lines()
            .map(str::trim)
            .map(str::to_string)
            .collect();
        self.cfg.ignore_classes = list(&self.mir.ignore);
        self.cfg.float_classes = list(&self.mir.float);
        self.cfg.window_rules = parse_window_rules(&self.mir.rich_rules);
        self.cfg.launcher_file_exclude = list(&self.mir.launcher_excludes);
        self.cfg.launcher_entries = parse_launcher_entries(&self.mir.launcher_entries);
        self.cfg.system_power_items = list(&self.mir.system_power);
        self.cfg.system_setup_items = list(&self.mir.system_setup);
        self.cfg.system_actions = parse_system_actions(&self.mir.system_actions);
        self.cfg.extra_hotkeys = parse_hotkeys(&self.mir.extra_hotkeys);
        let zone = |s: &str| -> Vec<String> {
            s.split([' ', ','])
                .map(|t| t.trim().to_ascii_lowercase())
                .filter(|t| BAR_WIDGETS.contains(&t.as_str()))
                .collect()
        };
        self.cfg.bar_left = zone(&self.mir.zone_l);
        self.cfg.bar_center = zone(&self.mir.zone_c);
        self.cfg.bar_right = zone(&self.mir.zone_r);
        // Normalise the mirrors to what survived validation.
        self.mir = Mirrors::from(&self.cfg);
    }

    fn save(&mut self) {
        self.fold_mirrors();
        let c = &self.cfg;
        let b = |v: bool| if v { "true" } else { "false" }.to_string();
        let wm_updates: Vec<(&str, String)> = vec![
            (
                "workspace_mode",
                if c.per_monitor {
                    "per_monitor"
                } else {
                    "shared"
                }
                .to_string(),
            ),
            ("workspaces", c.workspaces.to_string()),
            ("workspace_keys", self.mir.ws_keys.clone()),
            ("workspace_names", self.mir.workspace_names.clone()),
            ("workspace_icons", self.mir.workspace_icons.clone()),
            ("workspace_wallpapers", c.workspace_wallpapers.join(" ;; ")),
            ("start_tiled", b(c.start_tiled)),
            ("layout", c.layout.clone()),
            ("master_ratio", format!("{:.2}", c.master_ratio)),
            ("outer_gap", c.outer_gap.to_string()),
            ("inner_gap", c.inner_gap.to_string()),
            ("cursor_follows_focus", b(c.cursor_follows_focus)),
            ("focus_follows_mouse", b(c.focus_follows_mouse)),
            ("animations", b(c.animations)),
            ("animation_ms", c.animation_ms.to_string()),
            ("workspace_anim", c.workspace_anim.clone()),
            ("window_anim", c.window_anim.clone()),
            ("animation_easing", c.animation_easing.clone()),
            ("theme", c.theme.clone()),
            ("acrylic", b(c.acrylic)),
            ("popup_font_name", c.popup_font_name.clone()),
            ("popup_font_size", c.popup_font_size.to_string()),
            ("popup_font_weight", c.popup_font_weight.to_string()),
            ("popup_radius", c.popup_radius.to_string()),
            ("popup_border_width", c.popup_border_width.to_string()),
            ("popup_opacity", c.popup_opacity.to_string()),
            ("popup_bg", opt_hex(c.popup_bg)),
            ("popup_fg", opt_hex(c.popup_fg)),
            ("popup_muted", opt_hex(c.popup_muted)),
            ("popup_accent", opt_hex(c.popup_accent)),
            ("popup_accent_fg", opt_hex(c.popup_accent_fg)),
            ("popup_border", opt_hex(c.popup_border)),
            ("unfocused_opacity", format!("{:.2}", c.unfocused_opacity)),
            ("border_enabled", b(c.border_enabled)),
            ("focused_border", color_to_hex(c.focused_border)),
            ("unfocused_border", color_to_hex(c.unfocused_border)),
            ("ignore_classes", c.ignore_classes.join(", ")),
            ("float_classes", c.float_classes.join(", ")),
            ("window_rules", format_window_rules(&c.window_rules)),
            ("terminal", c.terminal.clone()),
            ("browser", c.browser.clone()),
            ("launcher_enabled", b(c.launcher_enabled)),
            ("launcher_width", c.launcher_width.to_string()),
            ("launcher_wide_width", c.launcher_wide_width.to_string()),
            ("launcher_height", c.launcher_height.to_string()),
            ("launcher_row_height", c.launcher_row_height.to_string()),
            ("launcher_icon_size", c.launcher_icon_size.to_string()),
            ("launcher_padding", c.launcher_padding.to_string()),
            (
                "launcher_selection_radius",
                c.launcher_selection_radius.to_string(),
            ),
            ("launcher_placement", c.launcher_placement.clone()),
            ("launcher_source_apps", b(c.launcher_source_apps)),
            ("launcher_source_files", b(c.launcher_source_files)),
            ("launcher_source_calc", b(c.launcher_source_calc)),
            ("launcher_source_web", b(c.launcher_source_web)),
            ("launcher_source_windows", b(c.launcher_source_windows)),
            ("launcher_source_clipboard", b(c.launcher_source_clipboard)),
            ("launcher_source_emoji", b(c.launcher_source_emoji)),
            ("launcher_web_url", c.launcher_web_url.clone()),
            ("launcher_max_results", c.launcher_max_results.to_string()),
            ("launcher_file_scope", c.launcher_file_scope.clone()),
            ("launcher_file_exclude", c.launcher_file_exclude.join(", ")),
            ("launcher_mru", b(c.launcher_mru)),
            (
                "launcher_entries",
                format_launcher_entries(&c.launcher_entries),
            ),
            ("system_menu_enabled", b(c.system_menu_enabled)),
            ("system_menu_width", c.system_menu_width.to_string()),
            ("system_power_items", c.system_power_items.join(", ")),
            ("system_setup_items", c.system_setup_items.join(", ")),
            ("system_actions", format_system_actions(&c.system_actions)),
            ("alt_tab_replacement", b(c.alt_tab_replacement)),
            ("scratchpad_enabled", b(c.scratchpad_enabled)),
            ("scratchpad_command", c.scratchpad_command.clone()),
            ("scratchpad_class", c.scratchpad_class.clone()),
            ("clipboard_history", b(c.clipboard_history)),
            ("clipboard_limit", c.clipboard_limit.to_string()),
            ("clipboard_prefix", c.clipboard_prefix.clone()),
            ("emoji_picker", b(c.emoji_picker)),
            ("emoji_prefix", c.emoji_prefix.clone()),
            ("wallpaper_dir", c.wallpaper_dir.clone()),
            ("media_enabled", b(c.media_enabled)),
            ("ipc_enabled", b(c.ipc_enabled)),
            ("ipc_pipe", c.ipc_pipe.clone()),
            ("persist_state", b(c.persist_state)),
            ("extra_hotkeys", format_hotkeys(&c.extra_hotkeys)),
            ("key_focus_next", vk_to_key(c.key_focus_next)),
            ("key_focus_prev", vk_to_key(c.key_focus_prev)),
            ("key_shrink_master", vk_to_key(c.key_shrink_master)),
            ("key_grow_master", vk_to_key(c.key_grow_master)),
            ("key_promote_master", vk_to_key(c.key_promote_master)),
            ("key_toggle_tiling", vk_to_key(c.key_toggle_tiling)),
            ("key_toggle_float", vk_to_key(c.key_toggle_float)),
            ("key_close_window", vk_to_key(c.key_close_window)),
        ];
        let nav_updates: Vec<(&str, String)> = vec![
            ("enabled", b(c.bar_enabled)),
            ("height", c.bar_height.to_string()),
            ("bottom", b(c.bar_bottom)),
            ("padding", c.bar_padding.to_string()),
            ("font_name", c.bar_font_name.clone()),
            ("font_size", c.bar_font_size.to_string()),
            ("floating", b(c.bar_floating)),
            ("margin", c.bar_margin.to_string()),
            ("radius", c.bar_radius.to_string()),
            ("autohide", b(c.bar_autohide)),
            ("left", c.bar_left.join(" ")),
            ("center", c.bar_center.join(" ")),
            ("right", c.bar_right.join(" ")),
            ("wheel_workspaces", b(c.bar_wheel_ws)),
            ("hide_empty_workspaces", b(c.bar_hide_empty)),
            ("show_title", b(c.bar_show_title)),
            ("show_layout", b(c.bar_show_layout)),
            ("show_clock", b(c.bar_show_clock)),
            ("clock_24h", b(c.bar_clock_24h)),
            ("show_date", b(c.bar_show_date)),
            ("date_format", c.bar_date_format.clone()),
            ("show_cpu", b(c.bar_show_cpu)),
            ("show_mem", b(c.bar_show_mem)),
            ("show_battery", b(c.bar_show_battery)),
            ("show_net", b(c.bar_show_net)),
            ("show_volume", b(c.bar_show_volume)),
            ("show_apps", b(c.bar_show_apps)),
            ("show_media", b(c.bar_show_media)),
            ("widget_gap", c.bar_widget_gap.to_string()),
            ("icon_size", c.bar_icon_size.to_string()),
            ("workspace_width", c.bar_workspace_width.to_string()),
            ("icon_mode", c.bar_icon_mode.clone()),
            ("show_tooltips", b(c.bar_show_tooltips)),
            ("show_app_labels", b(c.bar_show_app_labels)),
            ("cpu_format", c.bar_cpu_format.clone()),
            ("mem_format", c.bar_mem_format.clone()),
            ("battery_format", c.bar_battery_format.clone()),
            ("net_format", c.bar_net_format.clone()),
            ("volume_format", c.bar_volume_format.clone()),
            ("clock_format", c.bar_clock_format.clone()),
            ("icon_cpu", c.bar_icon_cpu.clone()),
            ("icon_mem", c.bar_icon_mem.clone()),
            ("icon_battery", c.bar_icon_battery.clone()),
            ("icon_net", c.bar_icon_net.clone()),
            ("icon_volume", c.bar_icon_volume.clone()),
            ("bg", opt_hex(c.bar_bg)),
            ("fg", opt_hex(c.bar_fg)),
            ("accent", opt_hex(c.bar_accent)),
            ("inactive", opt_hex(c.bar_inactive)),
        ];
        let (wm_old, nav_old) = read_confs();
        let wm_new = apply_updates(&wm_old, &wm_updates);
        let nav_new = apply_updates(&nav_old, &nav_updates);
        let wm_path = config_path("ASTUR_CONFIG", "astur.conf");
        let nav_path = config_path("ASTUR_NAVBAR", "navbar.conf");
        self.error = None;
        for (path, text) in [(&wm_path, &wm_new), (&nav_path, &nav_new)] {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(path, text) {
                self.error = Some(format!("write failed: {} ({e})", path.display()));
            }
        }
        if self.error.is_none() {
            self.saved_cfg = self.cfg.clone();
            self.saved_mir = self.mir.clone();
            self.saved_at = Some(std::time::Instant::now());
        }
    }
}

/// Themeable colour to conf text: explicit hex, or `auto` (follow the theme).
fn opt_hex(c: Option<u32>) -> String {
    c.map(color_to_hex).unwrap_or_else(|| "auto".to_string())
}

/// Read both config files, falling back to the built-in commented templates so
/// a fresh install edits (and saves) fully-documented files.
fn read_confs() -> (String, String) {
    let wm = std::fs::read_to_string(config_path("ASTUR_CONFIG", "astur.conf"))
        .unwrap_or_else(|_| default_config_text().to_string());
    let nav = std::fs::read_to_string(config_path("ASTUR_NAVBAR", "navbar.conf"))
        .unwrap_or_else(|_| default_navbar_text().to_string());
    (wm, nav)
}

/// COLORREF (0x00BBGGRR) colour picker row.
fn color_row(ui: &mut egui::Ui, label: &str, c: &mut u32) {
    ui.horizontal(|ui| {
        let mut rgb = [
            (*c & 0xFF) as u8,
            ((*c >> 8) & 0xFF) as u8,
            ((*c >> 16) & 0xFF) as u8,
        ];
        if ui.color_edit_button_srgb(&mut rgb).changed() {
            *c = ((rgb[2] as u32) << 16) | ((rgb[1] as u32) << 8) | rgb[0] as u32;
        }
        ui.label(label);
    });
}

/// Themeable colour row: "Auto" follows the dark/light theme preset; unticking
/// it starts from `seed` and lets the user pick an explicit override.
fn theme_color_row(ui: &mut egui::Ui, label: &str, c: &mut Option<u32>, seed: u32) {
    ui.horizontal(|ui| {
        let mut auto = c.is_none();
        if ui
            .checkbox(&mut auto, "Auto")
            .on_hover_text("Follow the theme (dark/light preset)")
            .changed()
        {
            *c = if auto { None } else { Some(seed) };
        }
        if let Some(v) = c.as_mut() {
            let mut rgb = [
                (*v & 0xFF) as u8,
                ((*v >> 8) & 0xFF) as u8,
                ((*v >> 16) & 0xFF) as u8,
            ];
            if ui.color_edit_button_srgb(&mut rgb).changed() {
                *v = ((rgb[2] as u32) << 16) | ((rgb[1] as u32) << 8) | rgb[0] as u32;
            }
        }
        ui.label(label);
    });
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.heading(text);
    ui.separator();
    ui.add_space(4.0);
}

/// Keep the GUI's own look in lockstep with Astur's theme setting. `auto`
/// follows Windows; explicit dark/light win regardless of the OS theme.
/// Also lifts text contrast: egui's dark default labels are ~gray(140) on a
/// near-black panel — genuinely hard to read. Cheap + idempotent per frame.
fn apply_gui_theme(ctx: &egui::Context, theme: &str) {
    ctx.set_theme(match theme {
        "light" => egui::ThemePreference::Light,
        "auto" => egui::ThemePreference::System,
        _ => egui::ThemePreference::Dark,
    });
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        let w = &mut s.visuals.widgets;
        w.noninteractive.fg_stroke.color = egui::Color32::from_gray(222); // labels
        w.inactive.fg_stroke.color = egui::Color32::from_gray(215); // idle widgets
        w.hovered.fg_stroke.color = egui::Color32::from_gray(245);
        w.active.fg_stroke.color = egui::Color32::WHITE;
        w.open.fg_stroke.color = egui::Color32::from_gray(230);
    });
    ctx.style_mut_of(egui::Theme::Light, |s| {
        let w = &mut s.visuals.widgets;
        w.noninteractive.fg_stroke.color = egui::Color32::from_gray(25);
        w.inactive.fg_stroke.color = egui::Color32::from_gray(35);
        w.hovered.fg_stroke.color = egui::Color32::BLACK;
        w.active.fg_stroke.color = egui::Color32::BLACK;
        w.open.fg_stroke.color = egui::Color32::from_gray(30);
    });
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Cheap and idempotent; re-applying each frame also makes the theme
        // combo preview instantly (before Save).
        apply_gui_theme(ctx, &self.cfg.theme);
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("Astur Settings");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let dirty = self.dirty();
                    if ui
                        .add_enabled(dirty, egui::Button::new("Save"))
                        .on_hover_text("Writes the config files; Astur applies them live")
                        .clicked()
                    {
                        self.save();
                    }
                    if dirty {
                        ui.label(egui::RichText::new("unsaved changes").weak());
                    } else if let Some(t) = self.saved_at {
                        if t.elapsed().as_secs() < 6 {
                            ui.label(egui::RichText::new("saved — applied live").weak());
                        }
                    }
                    if let Some(e) = &self.error {
                        ui.colored_label(egui::Color32::from_rgb(220, 80, 80), e);
                    }
                });
            });
            ui.add_space(6.0);
        });

        egui::SidePanel::left("nav")
            .resizable(false)
            .exact_width(170.0)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                for (sec, label) in SECTIONS {
                    if ui.selectable_label(self.section == *sec, *label).clicked() {
                        self.section = *sec;
                    }
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.set_max_width(640.0);
                match self.section {
                    Section::General => self.ui_general(ui),
                    Section::Layout => self.ui_layout(ui),
                    Section::Focus => self.ui_focus(ui),
                    Section::Animations => self.ui_animations(ui),
                    Section::Appearance => self.ui_appearance(ui),
                    Section::Launcher => self.ui_launcher(ui),
                    Section::SystemMenu => self.ui_system_menu(ui),
                    Section::Desktop => self.ui_desktop(ui),
                    Section::Bar => self.ui_bar(ui),
                    Section::Widgets => self.ui_widgets(ui),
                    Section::Hotkeys => self.ui_hotkeys(ui),
                    Section::Rules => self.ui_rules(ui),
                    Section::About => self.ui_about(ui),
                }
                ui.add_space(24.0);
            });
        });
    }
}

impl App {
    fn ui_general(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Theme");
        egui::ComboBox::from_label("Colour theme")
            .selected_text(match self.cfg.theme.as_str() {
                "light" => "Light",
                "auto" => "System",
                _ => "Dark",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.cfg.theme, "dark".to_string(), "Dark");
                ui.selectable_value(&mut self.cfg.theme, "light".to_string(), "Light");
                ui.selectable_value(
                    &mut self.cfg.theme,
                    "auto".to_string(),
                    "System (follow Windows)",
                );
            });
        ui.label(
            egui::RichText::new(
                "Sets the base palette for the launcher, menus and bar. Any colour you customise in a section below overrides the theme for that element.",
            )
            .weak(),
        );

        heading(ui, "Workspaces");
        egui::ComboBox::from_label("Workspace mode")
            .selected_text(if self.cfg.per_monitor {
                "per monitor"
            } else {
                "shared"
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.cfg.per_monitor,
                    false,
                    "shared (numbered globally)",
                );
                ui.selectable_value(
                    &mut self.cfg.per_monitor,
                    true,
                    "per monitor (GlazeWM style)",
                );
            });
        ui.add(egui::Slider::new(&mut self.cfg.workspaces, 1..=10).text("Workspaces"));
        ui.horizontal(|ui| {
            ui.label("Workspace keys");
            ui.text_edit_singleline(&mut self.mir.ws_keys)
                .on_hover_text("Space-separated key names (0-9, A-Z, F1-F24), in workspace order");
        });
        ui.horizontal(|ui| {
            ui.label("Workspace names");
            ui.text_edit_singleline(&mut self.mir.workspace_names)
                .on_hover_text("Comma-separated display names; missing positions use numbers");
        });
        ui.horizontal(|ui| {
            ui.label("Workspace icons");
            ui.text_edit_singleline(&mut self.mir.workspace_icons)
                .on_hover_text("Comma-separated compact labels or glyphs from selected bar font");
        });
        ui.checkbox(
            &mut self.cfg.start_tiled,
            "Tile windows automatically on launch",
        );

        heading(ui, "Launchers");
        ui.horizontal(|ui| {
            ui.label("Terminal (Alt+Enter)");
            ui.text_edit_singleline(&mut self.cfg.terminal);
        });
        ui.horizontal(|ui| {
            ui.label("Browser (Alt+Shift+Enter)");
            ui.text_edit_singleline(&mut self.cfg.browser)
                .on_hover_text("Leave empty for the system default browser");
        });
    }

    fn ui_layout(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Tiling layout");
        egui::ComboBox::from_label("Layout")
            .selected_text(&self.cfg.layout)
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.cfg.layout,
                    "dwindle".to_string(),
                    "dwindle (spiral)",
                );
                ui.selectable_value(
                    &mut self.cfg.layout,
                    "master".to_string(),
                    "master (column + stack)",
                );
                ui.selectable_value(&mut self.cfg.layout, "columns".to_string(), "equal columns");
                ui.selectable_value(&mut self.cfg.layout, "grid".to_string(), "balanced grid");
                ui.selectable_value(&mut self.cfg.layout, "monocle".to_string(), "monocle");
            });
        ui.add(
            egui::Slider::new(&mut self.cfg.master_ratio, 0.10..=0.90)
                .text("Master ratio")
                .fixed_decimals(2),
        );

        heading(ui, "Gaps");
        ui.add(egui::Slider::new(&mut self.cfg.outer_gap, 0..=100).text("Outer gap (px)"));
        ui.add(egui::Slider::new(&mut self.cfg.inner_gap, 0..=100).text("Inner gap (px)"));
    }

    fn ui_focus(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Focus & mouse");
        ui.checkbox(
            &mut self.cfg.cursor_follows_focus,
            "Cursor follows focus (Alt+arrows, workspace switches)",
        );
        ui.checkbox(
            &mut self.cfg.focus_follows_mouse,
            "Focus follows mouse (hovering a window focuses it)",
        );
    }

    fn ui_animations(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Animations");
        ui.checkbox(&mut self.cfg.animations, "Enable animations");
        ui.add(
            egui::Slider::new(&mut self.cfg.animation_ms, 0..=500)
                .text("Duration (ms)")
                .clamping(egui::SliderClamping::Always),
        );
        egui::ComboBox::from_label("Workspace switch")
            .selected_text(&self.cfg.workspace_anim)
            .show_ui(ui, |ui| {
                for m in ["off", "slide", "spring", "fade"] {
                    ui.selectable_value(&mut self.cfg.workspace_anim, m.to_string(), m);
                }
            });
        egui::ComboBox::from_label("Window placement")
            .selected_text(&self.cfg.window_anim)
            .show_ui(ui, |ui| {
                for m in ["off", "glide", "spring"] {
                    ui.selectable_value(&mut self.cfg.window_anim, m.to_string(), m);
                }
            });
        egui::ComboBox::from_label("Easing")
            .selected_text(&self.cfg.animation_easing)
            .show_ui(ui, |ui| {
                for m in ["cubic", "smooth", "spring"] {
                    ui.selectable_value(&mut self.cfg.animation_easing, m.to_string(), m);
                }
            });
        ui.label(
            egui::RichText::new(
                "Animations are cosmetic overlays: the switch/tile underneath is always instant and correct.",
            )
            .weak(),
        );
    }

    fn ui_appearance(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Popups");
        ui.checkbox(
            &mut self.cfg.acrylic,
            "Acrylic blur behind popups (experimental)",
        );
        ui.horizontal(|ui| {
            ui.label("Font family");
            ui.text_edit_singleline(&mut self.cfg.popup_font_name);
        });
        ui.add(egui::Slider::new(&mut self.cfg.popup_font_size, 10..=48).text("Font size"));
        ui.add(egui::Slider::new(&mut self.cfg.popup_font_weight, 100..=900).text("Font weight"));
        ui.add(egui::Slider::new(&mut self.cfg.popup_radius, 0..=48).text("Card radius"));
        ui.add(egui::Slider::new(&mut self.cfg.popup_border_width, 0..=8).text("Border width"));
        ui.add(egui::Slider::new(&mut self.cfg.popup_opacity, 20..=100).text("Opacity %"));
        theme_color_row(ui, "Popup background", &mut self.cfg.popup_bg, 0x0016_1616);
        theme_color_row(ui, "Popup text", &mut self.cfg.popup_fg, 0x00E6_E6E6);
        theme_color_row(ui, "Popup muted", &mut self.cfg.popup_muted, 0x0089_8989);
        theme_color_row(ui, "Popup accent", &mut self.cfg.popup_accent, 0x0082_6333);
        theme_color_row(
            ui,
            "Accent text",
            &mut self.cfg.popup_accent_fg,
            0x00FF_FFFF,
        );
        theme_color_row(ui, "Popup border", &mut self.cfg.popup_border, 0x0033_2A26);

        heading(ui, "Windows");
        ui.add(
            egui::Slider::new(&mut self.cfg.unfocused_opacity, 0.10..=1.00)
                .text("Unfocused window opacity (1.0 = off)")
                .fixed_decimals(2),
        );
        ui.checkbox(
            &mut self.cfg.border_enabled,
            "Coloured window borders (Windows 11)",
        );
        color_row(ui, "Focused border", &mut self.cfg.focused_border);
        color_row(ui, "Unfocused border", &mut self.cfg.unfocused_border);
    }

    fn ui_launcher(&mut self, ui: &mut egui::Ui) {
        heading(ui, "App picker");
        ui.checkbox(
            &mut self.cfg.launcher_enabled,
            "Enable Alt+Space app picker",
        );
        ui.add(egui::Slider::new(&mut self.cfg.launcher_width, 320..=1600).text("Width"));
        ui.add(egui::Slider::new(&mut self.cfg.launcher_wide_width, 480..=2400).text("Wide width"));
        ui.add(egui::Slider::new(&mut self.cfg.launcher_height, 200..=1200).text("Height"));
        ui.add(egui::Slider::new(&mut self.cfg.launcher_row_height, 24..=72).text("Row height"));
        ui.add(egui::Slider::new(&mut self.cfg.launcher_icon_size, 12..=64).text("Icon size"));
        ui.add(egui::Slider::new(&mut self.cfg.launcher_padding, 4..=48).text("Padding"));
        ui.add(
            egui::Slider::new(&mut self.cfg.launcher_selection_radius, 0..=40)
                .text("Selection radius"),
        );
        egui::ComboBox::from_label("Placement")
            .selected_text(&self.cfg.launcher_placement)
            .show_ui(ui, |ui| {
                for mode in ["cursor_monitor", "focused_monitor", "primary_monitor"] {
                    ui.selectable_value(&mut self.cfg.launcher_placement, mode.to_string(), mode);
                }
            });
        ui.add(
            egui::Slider::new(&mut self.cfg.launcher_max_results, 5..=200).text("Maximum results"),
        );
        ui.checkbox(&mut self.cfg.launcher_mru, "Boost recently used results");

        heading(ui, "Providers");
        ui.columns(2, |cols| {
            cols[0].checkbox(&mut self.cfg.launcher_source_apps, "Installed apps");
            cols[0].checkbox(&mut self.cfg.launcher_source_files, "Indexed files");
            cols[0].checkbox(&mut self.cfg.launcher_source_calc, "Calculator");
            cols[0].checkbox(&mut self.cfg.launcher_source_web, "Web fallback");
            cols[1].checkbox(&mut self.cfg.launcher_source_windows, "Open windows");
            cols[1].checkbox(&mut self.cfg.launcher_source_clipboard, "Clipboard history");
            cols[1].checkbox(&mut self.cfg.launcher_source_emoji, "Emoji catalog");
        });
        ui.horizontal(|ui| {
            ui.label("Web URL");
            ui.text_edit_singleline(&mut self.cfg.launcher_web_url)
                .on_hover_text("Must contain {query}");
        });
        ui.horizontal(|ui| {
            ui.label("File scope");
            ui.text_edit_singleline(&mut self.cfg.launcher_file_scope)
                .on_hover_text("Empty = user profile; * = entire Windows Search index");
        });
        ui.label("Excluded path fragments (comma separated)");
        ui.text_edit_singleline(&mut self.mir.launcher_excludes);

        heading(ui, "Custom entries");
        ui.label(
            egui::RichText::new(
                "Records: label|target|icon ;; ... Target supports cmd: and url:. Icon supports path, shell target, auto, or built-in name. Escape separators with a leading backslash.",
            )
            .weak(),
        );
        ui.add(
            egui::TextEdit::multiline(&mut self.mir.launcher_entries)
                .desired_rows(5)
                .desired_width(f32::INFINITY),
        );
    }

    fn ui_system_menu(&mut self, ui: &mut egui::Ui) {
        heading(ui, "System menu");
        ui.checkbox(
            &mut self.cfg.system_menu_enabled,
            "Enable Alt+Shift+Space system menu",
        );
        ui.add(egui::Slider::new(&mut self.cfg.system_menu_width, 280..=900).text("Menu width"));
        ui.label("Power built-ins (comma separated, order preserved)");
        ui.text_edit_singleline(&mut self.mir.system_power);
        ui.label("Setup built-ins (comma separated, order preserved)");
        ui.text_edit_singleline(&mut self.mir.system_setup);
        ui.label(
            egui::RichText::new(
                "Built-ins: lock, sleep, hibernate, sign_out, restart, shutdown, settings, open_config, reload, restart_astur, screenshot, wallpapers.",
            )
            .weak(),
        );

        heading(ui, "Custom actions");
        ui.label(
            egui::RichText::new(
                "Records: category|label|target|icon|confirm ;; ... Target supports cmd:/url:/shell paths. Icon may be built-in name or file path. Escape separators with a leading backslash.",
            )
            .weak(),
        );
        ui.add(
            egui::TextEdit::multiline(&mut self.mir.system_actions)
                .desired_rows(6)
                .desired_width(f32::INFINITY),
        );
    }

    fn ui_desktop(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Window switcher and scratchpad");
        ui.checkbox(
            &mut self.cfg.alt_tab_replacement,
            "Use Astur window switcher for Alt+Tab",
        );
        ui.checkbox(
            &mut self.cfg.scratchpad_enabled,
            "Enable scratchpad (default Alt+Grave binding)",
        );
        ui.add_enabled_ui(self.cfg.scratchpad_enabled, |ui| {
            ui.horizontal(|ui| {
                ui.label("Command");
                ui.text_edit_singleline(&mut self.cfg.scratchpad_command);
            });
            ui.horizontal(|ui| {
                ui.label("Window class/glob");
                ui.text_edit_singleline(&mut self.cfg.scratchpad_class);
            });
        });

        heading(ui, "Clipboard and emoji");
        ui.checkbox(
            &mut self.cfg.clipboard_history,
            "Keep clipboard history in memory",
        );
        ui.add_enabled_ui(self.cfg.clipboard_history, |ui| {
            ui.add(egui::Slider::new(&mut self.cfg.clipboard_limit, 1..=200).text("History limit"));
            ui.horizontal(|ui| {
                ui.label("Picker prefix");
                ui.text_edit_singleline(&mut self.cfg.clipboard_prefix);
            });
        });
        ui.checkbox(&mut self.cfg.emoji_picker, "Enable emoji catalog");
        ui.horizontal(|ui| {
            ui.label("Emoji prefix");
            ui.text_edit_singleline(&mut self.cfg.emoji_prefix);
        });

        heading(ui, "Wallpapers and state");
        ui.horizontal(|ui| {
            ui.label("Wallpaper folder");
            ui.text_edit_singleline(&mut self.cfg.wallpaper_dir);
        });
        ui.label("Workspace-triggered global Windows wallpapers, one path per line");
        ui.add(
            egui::TextEdit::multiline(&mut self.mir.workspace_wallpapers)
                .desired_rows(5)
                .desired_width(f32::INFINITY),
        );
        ui.checkbox(
            &mut self.cfg.media_enabled,
            "Poll known player window titles for media widget",
        );
        ui.checkbox(
            &mut self.cfg.persist_state,
            "Persist active workspace indexes and launcher MRU",
        );

        heading(ui, "Local command API");
        ui.checkbox(&mut self.cfg.ipc_enabled, "Enable local named-pipe API");
        ui.add_enabled_ui(self.cfg.ipc_enabled, |ui| {
            ui.horizontal(|ui| {
                ui.label("Pipe name");
                ui.text_edit_singleline(&mut self.cfg.ipc_pipe);
            });
        });
    }

    fn ui_bar(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Bar");
        ui.checkbox(&mut self.cfg.bar_enabled, "Show the status bar");
        ui.add(egui::Slider::new(&mut self.cfg.bar_height, 16..=64).text("Height (px)"));
        ui.checkbox(&mut self.cfg.bar_bottom, "Dock at the bottom of the screen");
        ui.add(egui::Slider::new(&mut self.cfg.bar_padding, 0..=48).text("Edge padding (px)"));
        ui.add(egui::Slider::new(&mut self.cfg.bar_widget_gap, 0..=48).text("Widget gap"));
        ui.add(egui::Slider::new(&mut self.cfg.bar_icon_size, 8..=48).text("App icon size"));
        ui.add(
            egui::Slider::new(&mut self.cfg.bar_workspace_width, 18..=100)
                .text("Workspace pill width"),
        );
        egui::ComboBox::from_label("Stat label mode")
            .selected_text(&self.cfg.bar_icon_mode)
            .show_ui(ui, |ui| {
                for mode in ["icon", "text", "both"] {
                    ui.selectable_value(&mut self.cfg.bar_icon_mode, mode.to_string(), mode);
                }
            });
        ui.checkbox(&mut self.cfg.bar_show_tooltips, "Show widget tooltips");
        ui.checkbox(
            &mut self.cfg.bar_show_app_labels,
            "Show labels beside app icons",
        );

        heading(ui, "Style");
        ui.checkbox(
            &mut self.cfg.bar_floating,
            "Floating bar (detached, rounded)",
        );
        ui.add_enabled_ui(self.cfg.bar_floating, |ui| {
            ui.add(egui::Slider::new(&mut self.cfg.bar_margin, 0..=48).text("Margin (px)"));
            ui.add(egui::Slider::new(&mut self.cfg.bar_radius, 0..=40).text("Corner radius (px)"));
        });
        ui.checkbox(
            &mut self.cfg.bar_autohide,
            "Auto-hide (reveal on screen-edge hover)",
        );
        ui.checkbox(
            &mut self.cfg.bar_wheel_ws,
            "Mouse wheel over the bar cycles workspaces",
        );
        ui.checkbox(&mut self.cfg.bar_hide_empty, "Hide empty workspace pills");

        heading(ui, "Font");
        ui.horizontal(|ui| {
            ui.label("Font family");
            ui.text_edit_singleline(&mut self.cfg.bar_font_name);
        });
        ui.add(egui::Slider::new(&mut self.cfg.bar_font_size, 0..=40).text("Font size (0 = auto)"));

        heading(ui, "Colours");
        ui.label(
            egui::RichText::new(
                "Auto = the colour follows the theme (General section) and flips with dark/light. Untick Auto to pin an explicit colour.",
            )
            .weak(),
        );
        ui.add_space(4.0);
        theme_color_row(ui, "Background", &mut self.cfg.bar_bg, BAR_DARK[0]);
        theme_color_row(ui, "Text", &mut self.cfg.bar_fg, BAR_DARK[1]);
        theme_color_row(
            ui,
            "Accent (active workspace)",
            &mut self.cfg.bar_accent,
            BAR_DARK[2],
        );
        theme_color_row(
            ui,
            "Muted (empty workspaces, stats)",
            &mut self.cfg.bar_inactive,
            BAR_DARK[3],
        );
    }

    fn ui_widgets(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Zones");
        ui.label(
            egui::RichText::new(
                "Widgets per zone, space separated, drawn in order. Available: workspaces, apps, title, layout, cpu, mem, net, volume, battery, date, clock, media, separator, spacer.",
            )
            .weak(),
        );
        ui.add_space(4.0);
        for (label, s) in [
            ("Left", &mut self.mir.zone_l),
            ("Center", &mut self.mir.zone_c),
            ("Right", &mut self.mir.zone_r),
        ] {
            ui.horizontal(|ui| {
                ui.label(format!("{label:>6}"));
                ui.add(egui::TextEdit::singleline(s).desired_width(420.0));
            });
        }

        heading(ui, "Widget toggles");
        ui.label(
            egui::RichText::new("A widget shows only if it is listed in a zone AND ticked here.")
                .weak(),
        );
        ui.add_space(4.0);
        ui.columns(2, |cols| {
            cols[0].checkbox(&mut self.cfg.bar_show_title, "Window title");
            cols[0].checkbox(&mut self.cfg.bar_show_layout, "Layout indicator");
            cols[0].checkbox(&mut self.cfg.bar_show_clock, "Clock");
            cols[0].checkbox(&mut self.cfg.bar_show_date, "Date");
            cols[0].checkbox(&mut self.cfg.bar_show_apps, "App buttons (click to focus)");
            cols[0].checkbox(&mut self.cfg.bar_show_media, "Current media");
            cols[1].checkbox(&mut self.cfg.bar_show_cpu, "CPU %");
            cols[1].checkbox(&mut self.cfg.bar_show_mem, "RAM %");
            cols[1].checkbox(&mut self.cfg.bar_show_battery, "Battery %");
            cols[1].checkbox(&mut self.cfg.bar_show_net, "Network speed");
            cols[1].checkbox(
                &mut self.cfg.bar_show_volume,
                "Volume (wheel adjusts, click mutes)",
            );
        });

        heading(ui, "Clock & date");
        ui.checkbox(&mut self.cfg.bar_clock_24h, "24-hour clock");
        ui.horizontal(|ui| {
            ui.label("Date format");
            ui.text_edit_singleline(&mut self.cfg.bar_date_format)
                .on_hover_text("Tokens: yyyy yy MMM MM ddd dd — e.g. \"ddd dd MMM\" -> Fri 19 Jun");
        });
        ui.horizontal(|ui| {
            ui.label("Clock format");
            ui.text_edit_singleline(&mut self.cfg.bar_clock_format);
        });

        heading(ui, "Stat formats and labels");
        for (label, format, icon) in [
            (
                "CPU",
                &mut self.cfg.bar_cpu_format,
                &mut self.cfg.bar_icon_cpu,
            ),
            (
                "Memory",
                &mut self.cfg.bar_mem_format,
                &mut self.cfg.bar_icon_mem,
            ),
            (
                "Battery",
                &mut self.cfg.bar_battery_format,
                &mut self.cfg.bar_icon_battery,
            ),
            (
                "Network",
                &mut self.cfg.bar_net_format,
                &mut self.cfg.bar_icon_net,
            ),
            (
                "Volume",
                &mut self.cfg.bar_volume_format,
                &mut self.cfg.bar_icon_volume,
            ),
        ] {
            ui.horizontal(|ui| {
                ui.label(label);
                ui.add(egui::TextEdit::singleline(icon).desired_width(64.0));
                ui.add(egui::TextEdit::singleline(format).desired_width(220.0));
            });
        }
    }

    fn ui_hotkeys(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Rebindable keys (with Left Alt)");
        ui.label(
            egui::RichText::new(
                "Single key name each: 0-9, A-Z or F1-F24. Arrows, Enter, Space and Tab are fixed.",
            )
            .weak(),
        );
        ui.add_space(4.0);
        let labels = [
            "Focus next window",
            "Focus previous window",
            "Shrink master",
            "Grow master",
            "Promote to master",
            "Toggle tiling",
            "Toggle float",
            "Close window",
        ];
        for (label, key) in labels.iter().zip(self.mir.keys.iter_mut()) {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(key).desired_width(48.0));
                let ok = key_to_vk(key.trim()).is_some();
                if !ok {
                    ui.colored_label(egui::Color32::from_rgb(220, 80, 80), "?");
                }
                ui.label(*label);
            });
        }
        heading(ui, "Extra hotkeys");
        ui.label(
            egui::RichText::new(
                "Records: ALT+SHIFT+Q|action|argument ;; ... Actions include launch, layout, switch_workspace, move_to_workspace, scratchpad, launcher, system_menu, reload. Escape separators with a leading backslash.",
            )
            .weak(),
        );
        ui.add(
            egui::TextEdit::multiline(&mut self.mir.extra_hotkeys)
                .desired_rows(6)
                .desired_width(f32::INFINITY),
        );
    }

    fn ui_rules(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Window rules");
        ui.label(
            egui::RichText::new(
                "Comma-separated window CLASS names (find them with Spy++ or AutoHotkey Window Spy).",
            )
            .weak(),
        );
        ui.add_space(4.0);
        ui.label("Never manage (ignore):");
        ui.add(egui::TextEdit::singleline(&mut self.mir.ignore).desired_width(480.0));
        ui.add_space(8.0);
        ui.label("Manage but always float:");
        ui.add(egui::TextEdit::singleline(&mut self.mir.float).desired_width(480.0));

        heading(ui, "Rich rules");
        ui.label(
            egui::RichText::new(
                "Records: action|exe|class|title|workspace|monitor ;; ... Action: tile, float, ignore. Empty fields are wildcards; * and ? supported. Escape separators with a leading backslash.",
            )
            .weak(),
        );
        ui.add(
            egui::TextEdit::multiline(&mut self.mir.rich_rules)
                .desired_rows(7)
                .desired_width(f32::INFINITY),
        );
    }

    fn ui_about(&mut self, ui: &mut egui::Ui) {
        heading(ui, "About");
        ui.label(format!("Astur Settings {}", env!("CARGO_PKG_VERSION")));
        ui.label("Edits astur.conf and navbar.conf; the window manager applies saved changes live (no restart).");
        ui.add_space(8.0);
        if ui.button("Open config folder").clicked() {
            if let Some(dir) = config_path("ASTUR_CONFIG", "astur.conf").parent() {
                let _ = std::process::Command::new("explorer").arg(dir).spawn();
            }
        }
        if ui.button("Reload from disk").clicked() {
            *self = App::load();
        }
        ui.add_space(8.0);
        ui.hyperlink_to("astur.app", "https://astur.app");
        ui.hyperlink_to("GitHub", "https://github.com/Page011/Astur");
    }
}

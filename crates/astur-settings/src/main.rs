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
    key_to_vk, parse_pair, vk_to_key, Config, BAR_DARK, BAR_WIDGETS,
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
    ignore: String,
    float: String,
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
            ignore: cfg.ignore_classes.join(", "),
            float: cfg.float_classes.join(", "),
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
        self.cfg.ignore_classes = list(&self.mir.ignore);
        self.cfg.float_classes = list(&self.mir.float);
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
            ("workspace_mode", if c.per_monitor { "per_monitor" } else { "shared" }.to_string()),
            ("workspaces", c.workspaces.to_string()),
            ("workspace_keys", self.mir.ws_keys.clone()),
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
            ("theme", c.theme.clone()),
            ("acrylic", b(c.acrylic)),
            ("unfocused_opacity", format!("{:.2}", c.unfocused_opacity)),
            ("border_enabled", b(c.border_enabled)),
            ("focused_border", color_to_hex(c.focused_border)),
            ("unfocused_border", color_to_hex(c.unfocused_border)),
            ("ignore_classes", c.ignore_classes.join(", ")),
            ("float_classes", c.float_classes.join(", ")),
            ("terminal", c.terminal.clone()),
            ("browser", c.browser.clone()),
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
                    if ui
                        .selectable_label(self.section == *sec, *label)
                        .clicked()
                    {
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
                ui.selectable_value(&mut self.cfg.theme, "auto".to_string(), "System (follow Windows)");
            });
        ui.label(
            egui::RichText::new(
                "Sets the base palette for the launcher, menus and bar. Any colour you customise in a section below overrides the theme for that element.",
            )
            .weak(),
        );

        heading(ui, "Workspaces");
        egui::ComboBox::from_label("Workspace mode")
            .selected_text(if self.cfg.per_monitor { "per monitor" } else { "shared" })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.cfg.per_monitor, false, "shared (numbered globally)");
                ui.selectable_value(&mut self.cfg.per_monitor, true, "per monitor (GlazeWM style)");
            });
        ui.add(egui::Slider::new(&mut self.cfg.workspaces, 1..=10).text("Workspaces"));
        ui.horizontal(|ui| {
            ui.label("Workspace keys");
            ui.text_edit_singleline(&mut self.mir.ws_keys)
                .on_hover_text("Space-separated key names (0-9, A-Z, F1-F24), in workspace order");
        });
        ui.checkbox(&mut self.cfg.start_tiled, "Tile windows automatically on launch");

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
                ui.selectable_value(&mut self.cfg.layout, "dwindle".to_string(), "dwindle (spiral)");
                ui.selectable_value(&mut self.cfg.layout, "master".to_string(), "master (column + stack)");
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
                for m in ["off", "glide"] {
                    ui.selectable_value(&mut self.cfg.window_anim, m.to_string(), m);
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
        ui.checkbox(&mut self.cfg.acrylic, "Acrylic blur behind popups (experimental)");
        ui.label(
            egui::RichText::new("Popup colours follow the theme (General section).").weak(),
        );

        heading(ui, "Windows");
        ui.add(
            egui::Slider::new(&mut self.cfg.unfocused_opacity, 0.10..=1.00)
                .text("Unfocused window opacity (1.0 = off)")
                .fixed_decimals(2),
        );
        ui.checkbox(&mut self.cfg.border_enabled, "Coloured window borders (Windows 11)");
        color_row(ui, "Focused border", &mut self.cfg.focused_border);
        color_row(ui, "Unfocused border", &mut self.cfg.unfocused_border);
    }

    fn ui_bar(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Bar");
        ui.checkbox(&mut self.cfg.bar_enabled, "Show the status bar");
        ui.add(egui::Slider::new(&mut self.cfg.bar_height, 16..=64).text("Height (px)"));
        ui.checkbox(&mut self.cfg.bar_bottom, "Dock at the bottom of the screen");
        ui.add(egui::Slider::new(&mut self.cfg.bar_padding, 0..=48).text("Edge padding (px)"));

        heading(ui, "Style");
        ui.checkbox(&mut self.cfg.bar_floating, "Floating bar (detached, rounded)");
        ui.add_enabled_ui(self.cfg.bar_floating, |ui| {
            ui.add(egui::Slider::new(&mut self.cfg.bar_margin, 0..=48).text("Margin (px)"));
            ui.add(egui::Slider::new(&mut self.cfg.bar_radius, 0..=40).text("Corner radius (px)"));
        });
        ui.checkbox(&mut self.cfg.bar_autohide, "Auto-hide (reveal on screen-edge hover)");
        ui.checkbox(&mut self.cfg.bar_wheel_ws, "Mouse wheel over the bar cycles workspaces");
        ui.checkbox(&mut self.cfg.bar_hide_empty, "Hide empty workspace pills");

        heading(ui, "Font");
        ui.horizontal(|ui| {
            ui.label("Font family");
            ui.text_edit_singleline(&mut self.cfg.bar_font_name);
        });
        ui.add(
            egui::Slider::new(&mut self.cfg.bar_font_size, 0..=40).text("Font size (0 = auto)"),
        );

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
        theme_color_row(ui, "Accent (active workspace)", &mut self.cfg.bar_accent, BAR_DARK[2]);
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
                "Widgets per zone, space separated, drawn in order. Available: workspaces, apps, title, layout, cpu, mem, net, volume, battery, date, clock.",
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
            cols[1].checkbox(&mut self.cfg.bar_show_cpu, "CPU %");
            cols[1].checkbox(&mut self.cfg.bar_show_mem, "RAM %");
            cols[1].checkbox(&mut self.cfg.bar_show_battery, "Battery %");
            cols[1].checkbox(&mut self.cfg.bar_show_net, "Network speed");
            cols[1].checkbox(&mut self.cfg.bar_show_volume, "Volume (wheel adjusts, click mutes)");
        });

        heading(ui, "Clock & date");
        ui.checkbox(&mut self.cfg.bar_clock_24h, "24-hour clock");
        ui.horizontal(|ui| {
            ui.label("Date format");
            ui.text_edit_singleline(&mut self.cfg.bar_date_format)
                .on_hover_text("Tokens: yyyy yy MMM MM ddd dd — e.g. \"ddd dd MMM\" -> Fri 19 Jun");
        });
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

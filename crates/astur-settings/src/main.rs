// Astur Settings — the GUI for astur.conf / navbar.conf.
//
// Runs as a SEPARATE process from the window manager (a GUI crash must never
// touch the input hooks or the manager). It edits the same two config files the
// WM watches, via the shared `astur-config` crate, so the parser and the GUI can
// never drift. Changes apply as they are made, Windows 11 Settings style, with
// Undo instead of a Save button: each finished edit rewrites only the
// `key = value` lines that changed (comments, layout and hand edits are
// preserved), never a file it could not read, and the WM hot-reloads it within
// a second.
//
// No console in release (launched from the WM's tray; a console window would
// flash and could briefly be tiled).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use astur_config::{
    apply_updates, chord_label, color_to_hex, compile_keys, config_path, default_config_text,
    default_navbar_text, disk_root, format_chord, format_clock, format_date, format_hotkeys,
    format_launcher_entries, format_system_actions, format_window_rules, key_to_vk, parse_chord,
    parse_hotkeys, parse_into, parse_launcher_entries, parse_pair, parse_system_actions,
    parse_window_rules, pin_theme_color, set_conf_key, text_on, vk_to_key, workspace_label, Action,
    BarPreset, BarRoles, BindIssue, Config, HotkeyDef, LauncherEntry, Level, MainKey, SystemAction,
    WindowRule, BAR_ACTIONS, BAR_CLICK_DEFAULTS, BAR_CLICK_WIDGETS, BAR_DARK, BAR_LIGHT,
    BAR_PRESETS, BAR_WIDGETS, LAYOUTS, MAX_WORKSPACES, MOD_ALT, MOD_CTRL, MOD_HYPER, MOD_SHIFT,
    MOD_WIN, PALETTES, SWATCH_NAMES,
};
use eframe::egui::{
    self, Align, Color32, CornerRadius, FontFamily, FontId, Layout, Margin, RichText, Sense,
    Stroke, Ui,
};
use std::path::{Path, PathBuf};
use std::sync::PoisonError;
use std::time::{Duration, Instant, SystemTime};

// Embed the shipped examples so the portable settings exe can copy them without
// depending on a repository checkout or separate files beside the executable.
const YASB_WIDGET_YAML: &str = include_str!("../../../integrations/yasb/workspaces.yaml");
const YASB_WIDGET_CSS: &str = include_str!("../../../integrations/yasb/workspaces.css");

/// Quiet time after an edit is finished (toggle clicked, slider released, text
/// box left) before it is written. Coalesces a burst of clicks or arrow-key
/// steps into one write, so the WM reloads (and re-tiles) once, not per step.
const DEBOUNCE: Duration = Duration::from_millis(250);

/// How long a new Astur key is on trial. A key that turns out not to work (a
/// keyboard without it, a remote desktop that takes it) can't be used to get
/// back, so it goes back by itself unless kept, as Windows does with a new
/// display mode.
const TRIAL: Duration = Duration::from_secs(15);

fn main() -> eframe::Result {
    let paths = [
        config_path("ASTUR_CONFIG", "astur.conf"),
        config_path("ASTUR_NAVBAR", "navbar.conf"),
    ];
    // One Settings window at a time: two would interleave writes and undo
    // stacks over the same files. The OS drops the lock however this process
    // ends. No config folder yet (first run) or no lock file: skip the check.
    let lock = std::fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(paths[0].with_file_name("settings.lock"))
        .ok();
    if lock
        .as_ref()
        .is_some_and(|f| matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock)))
    {
        return Ok(());
    }
    // Installed fonts and apps for the pickers, found off the UI thread
    // (GDI, the registry and a few hundred file checks); a picker shows just
    // the current value until they are in.
    std::thread::spawn(|| {
        let _ = FONTS.set(sys::font_families());
        let _ = APPS.set(detect_apps());
        let _ = DRIVES.set(ready_drives());
    });
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 760.0])
            .with_min_inner_size([760.0, 480.0])
            .with_title("Astur Settings"),
        ..Default::default()
    };
    eframe::run_native(
        "Astur Settings",
        options,
        Box::new(|cc| {
            setup_look(&cc.egui_ctx);
            Ok(Box::new(App::load(paths)))
        }),
    )
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Page {
    Home,
    Shortcuts,
    Layout,
    Workspaces,
    Windows,
    Appearance,
    Motion,
    Bar,
    Launcher,
    Power,
    Extras,
    Integrations,
    Troubleshooting,
    About,
}

/// Navigation, in order: (page, title, Segoe Fluent / MDL2 icon, group heading
/// drawn above the page; empty = same group).
const NAV: &[(Page, &str, char, &str)] = &[
    (Page::Home, "Home", '\u{E80F}', ""),
    (Page::Shortcuts, "Shortcuts", '\u{E765}', "Core"),
    (Page::Layout, "Layout", '\u{ECA5}', ""),
    (Page::Workspaces, "Workspaces", '\u{E7C4}', ""),
    (Page::Windows, "Windows & rules", '\u{E71C}', ""),
    (Page::Appearance, "Appearance", '\u{E790}', "Look"),
    (Page::Motion, "Motion", '\u{EC4A}', ""),
    (Page::Bar, "Status bar", '\u{E7F4}', ""),
    (Page::Launcher, "Launcher", '\u{E721}', "Tools"),
    (Page::Power, "Power menu", '\u{E7E8}', ""),
    (Page::Extras, "Desktop extras", '\u{E71D}', ""),
    (Page::Integrations, "Integrations", '\u{EA86}', ""),
    (
        Page::Troubleshooting,
        "Troubleshooting",
        '\u{E90F}',
        "System",
    ),
    (Page::About, "About", '\u{E946}', ""),
];

const ICON_UNDO: char = '\u{E7A7}';
const ICON_REDO: char = '\u{E7A6}';
const ICON_OK: char = '\u{E73E}';
const ICON_WARN: char = '\u{E7BA}';
const ICON_UP: char = '\u{E70E}';
const ICON_DOWN: char = '\u{E70D}';
const ICON_DELETE: char = '\u{E74D}';
const ICON_FOLDER: char = '\u{E838}';

/// Text mirrors for fields the Config stores as lists. Edited as text,
/// validated + folded back into the Config when the edit is committed. (The
/// bar zones are not: the zone editor edits them in place.)
#[derive(Clone, PartialEq)]
struct Mirrors {
    launcher_excludes: String,
    rows: Rows,
}

impl Mirrors {
    fn from(cfg: &Config) -> Self {
        Mirrors {
            launcher_excludes: cfg.launcher_file_exclude.join(", "),
            rows: Rows::from(cfg),
        }
    }
}

/// The structured editors (tables, key chips, checklists). Unlike the text
/// mirrors these are never rebuilt from the Config at commit: a row that
/// can't be written yet stays as typed, in red with the reason, and its
/// table is folded into the Config (the `fold_*` functions, called when the
/// table changes) only while every row in it is valid. So a file never gets
/// half a list, and nothing the user typed disappears unseen.
#[derive(Clone, PartialEq, Default, Debug)]
struct Rows {
    /// The 12 single-key binds in `BINDS` order; "" = cleared (not written).
    keys: [String; 12],
    /// One per workspace, also past `workspaces`: those keep what the file
    /// sets for workspaces not shown, for when the count goes back up.
    ws: Vec<WsRow>,
    rules: Vec<RuleRow>,
    entries: Vec<EntryRow>,
    actions: Vec<ActionRow>,
    /// Power-menu built-ins as (name, shown), in menu order.
    power: Vec<(String, bool)>,
    setup: Vec<(String, bool)>,
    hotkeys: Vec<HotkeyDef>,
}

#[derive(Clone, PartialEq, Default, Debug)]
struct WsRow {
    name: String,
    icon: String,
    key: String,
    wallpaper: String,
}

/// A `window_rules` record, or (`list`) one class of `ignore_classes` /
/// `float_classes`: shown in the same table, written back to its own key.
#[derive(Clone, PartialEq, Debug)]
struct RuleRow {
    list: bool,
    rule: WindowRule,
}

#[derive(Clone, PartialEq, Debug)]
struct EntryRow {
    label: String,
    kind: Kind,
    value: String,
    icon: String,
}

#[derive(Clone, PartialEq, Debug)]
struct ActionRow {
    group: String,
    label: String,
    kind: Kind,
    value: String,
    icon: String,
    confirm: bool,
}

/// What a launcher entry or power-menu action opens. App, File and Folder
/// are all written as the plain path (the shell opens it), so the kind only
/// picks the field's hint; Web page and Command are the `url:` and `cmd:`
/// prefixes.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    App,
    File,
    Folder,
    Url,
    Command,
}

impl Rows {
    fn from(c: &Config) -> Self {
        let n = [
            c.workspaces,
            c.workspace_keys.len(),
            c.workspace_names.len(),
            c.workspace_icons.len(),
            c.workspace_wallpapers.len(),
        ]
        .into_iter()
        .max()
        .unwrap_or(0);
        let at = |v: &[String], i: usize| v.get(i).cloned().unwrap_or_default();
        let class = |action: &str, class: &String| RuleRow {
            list: true,
            rule: WindowRule {
                action: action.to_string(),
                exe: String::new(),
                class: class.clone(),
                title: String::new(),
                workspace: None,
                monitor: None,
            },
        };
        Rows {
            keys: binds(c).map(vk_to_key),
            ws: (0..n)
                .map(|i| WsRow {
                    name: at(&c.workspace_names, i),
                    icon: at(&c.workspace_icons, i),
                    key: c
                        .workspace_keys
                        .get(i)
                        .map_or_else(String::new, |&k| vk_to_key(k)),
                    wallpaper: at(&c.workspace_wallpapers, i),
                })
                .collect(),
            rules: c
                .window_rules
                .iter()
                .map(|r| RuleRow {
                    list: false,
                    rule: r.clone(),
                })
                .chain(c.ignore_classes.iter().map(|k| class("ignore", k)))
                .chain(c.float_classes.iter().map(|k| class("float", k)))
                .collect(),
            entries: c
                .launcher_entries
                .iter()
                .map(|e| {
                    let (kind, value) = split_target(&e.target);
                    EntryRow {
                        label: e.label.clone(),
                        kind,
                        value,
                        icon: e.icon.clone(),
                    }
                })
                .collect(),
            actions: c
                .system_actions
                .iter()
                .map(|a| {
                    let (kind, value) = split_target(&a.target);
                    ActionRow {
                        group: a.category.clone(),
                        label: a.label.clone(),
                        kind,
                        value,
                        icon: a.icon.clone(),
                        confirm: a.confirm,
                    }
                })
                .collect(),
            power: checklist(&c.system_power_items, &SYSTEM_ITEMS[..6]),
            setup: checklist(&c.system_setup_items, &SYSTEM_ITEMS[6..]),
            hotkeys: c.extra_hotkeys.clone(),
        }
    }

    /// Rows (or workspace columns, of the `workspaces` shown) that can't be
    /// written as they stand.
    fn problems(&self, workspaces: usize) -> usize {
        self.keys.iter().filter(|k| key_to_vk(k).is_none()).count()
            + (0..4)
                .filter(|&col| ws_column(&self.ws, col, workspaces).is_err())
                .count()
            + self.rules.iter().filter(|r| rule_check(r).is_err()).count()
            + self
                .entries
                .iter()
                .filter(|e| entry_check(e).is_err())
                .count()
            + self
                .actions
                .iter()
                .filter(|a| action_check(a).is_err())
                .count()
            + (self.power.iter().chain(&self.setup))
                .filter(|i| item_check(i).is_err())
                .count()
            + self
                .hotkeys
                .iter()
                .filter(|h| hotkey_check(h).is_err())
                .count()
    }
}

/// What the widgets edit (`Config` plus its text mirrors), as one snapshot.
type State = (Config, Mirrors);

/// A new Astur key until it is kept: the key before it, and the state just
/// after the switch (while nothing else changed, a plain Undo reverts it).
struct Trial {
    from: MainKey,
    after: State,
    /// Undo depth just after the switch. `after` alone can't prove nothing
    /// was committed since: a setting changed and put back reads as `after`
    /// too, and Undo would then restore the step in between (stale value).
    depth: usize,
    until: Instant,
}

/// Undo / redo of committed states. A commit records the state it replaced;
/// a new commit clears redo, as in every editor.
struct History<T> {
    undo: Vec<T>,
    redo: Vec<T>,
}

impl<T> Default for History<T> {
    fn default() -> Self {
        History {
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
}

impl<T> History<T> {
    fn record(&mut self, before: T) {
        self.undo.push(before);
        self.redo.clear();
    }

    fn undo(&mut self, current: T) -> Option<T> {
        let prev = self.undo.pop()?;
        self.redo.push(current);
        Some(prev)
    }

    fn redo(&mut self, current: T) -> Option<T> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        Some(next)
    }
}

/// Settings search for one frame. Every row hides itself unless it matches
/// (label, description or conf key), and the page and section headings above
/// a hit are drawn lazily by that hit, so all pages can render at once and a
/// page without hits leaves no trace.
#[derive(Default)]
struct Find {
    /// Lowercased query; empty = not searching.
    q: String,
    page: Option<(Page, &'static str)>,
    section: Option<&'static str>,
    hits: usize,
    goto: Option<Page>,
    /// Conf keys of the rows drawn this frame, for the test that proves
    /// every key the GUI writes can still be found and edited.
    #[cfg(test)]
    seen: Vec<String>,
}

impl Find {
    fn on(&self) -> bool {
        !self.q.is_empty()
    }

    fn matches(&self, texts: &[&str]) -> bool {
        !self.on() || texts.iter().any(|t| t.to_lowercase().contains(&self.q))
    }

    /// A matching row is about to draw: draw the headings it sits under first.
    fn hit(&mut self, ui: &mut Ui) {
        self.hits += 1;
        if let Some((page, title)) = self.page.take() {
            if self.hits > 1 {
                ui.add_space(18.0);
            }
            let title = RichText::new(format!("{title}  \u{203A}"))
                .size(20.0)
                .family(semibold());
            if ui
                .add(egui::Label::new(title).sense(Sense::click()))
                .on_hover_text("Open this page")
                .clicked()
            {
                self.goto = Some(page);
            }
        }
        if let Some(s) = self.section.take() {
            section_title(ui, s);
        }
    }
}

struct App {
    /// astur.conf, navbar.conf.
    paths: [PathBuf; 2],
    /// What the widgets edit. Differs from `applied` only mid-edit.
    cfg: Config,
    mir: Mirrors,
    /// The last committed state, written to disk.
    applied: State,
    /// As loaded, for "Undo all changes from this session".
    session: State,
    history: History<State>,
    /// What the files say: the base each write diffs against.
    disk: Config,
    /// File modification times at the last read or write; a different one at
    /// write time means someone else edited the file meanwhile.
    mtimes: [Option<SystemTime>; 2],
    def: Config,
    def_mir: Mirrors,
    commit_at: Option<Instant>,
    toast: Option<(Instant, &'static str)>,
    page: Page,
    query: String,
    find: Find,
    error: Option<String>,
    /// A config file exists but could not be read: built-in defaults are shown,
    /// the pages are disabled and nothing is written, so the user's file stays.
    read_error: Option<String>,
    /// The bar widget chip whose settings the Status bar page shows.
    bar_sel: Option<Src>,
    /// navbar.conf was copied to navbar.conf.bak this session (before the
    /// first preset was applied).
    backed_up: bool,
    trial: Option<Trial>,
}

impl App {
    fn load(paths: [PathBuf; 2]) -> Self {
        let mtimes = mtimes(&paths);
        let (disk, read_error) = match read_pair(&paths) {
            Ok(c) => (c, None),
            Err(e) => (astur_config::default_config(), Some(e)),
        };
        let state = (disk.clone(), Mirrors::from(&disk));
        let def = Config::defaults();
        App {
            paths,
            cfg: state.0.clone(),
            mir: state.1.clone(),
            applied: state.clone(),
            session: state,
            history: History::default(),
            disk,
            mtimes,
            def_mir: Mirrors::from(&def),
            def,
            commit_at: None,
            toast: None,
            page: Page::Home,
            query: String::new(),
            find: Find::default(),
            error: None,
            read_error,
            bar_sel: None,
            backed_up: false,
            trial: None,
        }
    }

    fn pending(&self) -> bool {
        self.cfg != self.applied.0 || self.mir != self.applied.1
    }

    /// Instant apply, debounced: the edit is written DEBOUNCE after the last
    /// gesture ends. `busy` = mid-gesture (a button held, a text box being
    /// typed in, a combo or colour popup open), which holds the write back so
    /// a half-typed number or a slider mid-drag never re-tiles the desktop.
    /// Returns how long until the write is due, for a repaint.
    fn settle(&mut self, now: Instant, busy: bool) -> Option<Duration> {
        if busy || !self.pending() {
            self.commit_at = None;
            return None;
        }
        let at = *self.commit_at.get_or_insert(now + DEBOUNCE);
        if now < at {
            return Some(at - now);
        }
        self.commit_at = None;
        self.commit();
        None
    }

    fn flush(&mut self) {
        if self.pending() {
            self.commit();
        }
    }

    fn commit(&mut self) {
        fold_mirrors(&mut self.cfg, &mut self.mir);
        let now = (self.cfg.clone(), self.mir.clone());
        if now == self.applied {
            return;
        }
        let wrote = now.0 != self.applied.0;
        let before = std::mem::replace(&mut self.applied, now);
        self.history.record(before);
        // A row that is not valid yet changes the editor, not the files.
        if wrote {
            self.write();
            self.toast_if_written("Applied");
        }
    }

    /// The toast claims the files took the edit, so only when they did: not
    /// read-only, and no write error beside it.
    fn toast_if_written(&mut self, text: &'static str) {
        if self.read_error.is_none() && self.error.is_none() {
            self.toast = Some((Instant::now(), text));
        }
    }

    fn can_undo(&self) -> bool {
        self.pending() || !self.history.undo.is_empty()
    }

    fn can_redo(&self) -> bool {
        !self.pending() && !self.history.redo.is_empty()
    }

    /// Mid-edit, undo just drops the unwritten edit.
    fn undo(&mut self) {
        if self.pending() {
            (self.cfg, self.mir) = self.applied.clone();
            return;
        }
        if self.outside_edit() {
            return;
        }
        if let Some(prev) = self.history.undo(self.applied.clone()) {
            self.restore(prev, "Undone");
        }
    }

    /// Not mid-edit (the header disables Redo then): restoring would drop
    /// the edit unrecorded and unwritten.
    fn redo(&mut self) {
        if self.pending() || self.outside_edit() {
            return;
        }
        let from = self.cfg.main_key;
        if let Some(next) = self.history.redo(self.applied.clone()) {
            self.restore(next, "Redone");
            // A key brought back by Redo is as untried as a picked one: on
            // trial, so one that doesn't work still goes back. Its Undo is
            // exact (redo pushed the state before it). Undo stays trial-free.
            if self.cfg.main_key != from {
                self.start_trial(from);
            }
        }
    }

    /// An ordinary commit of the state as loaded, so it can itself be undone.
    fn undo_all(&mut self) {
        if self.outside_edit() {
            return;
        }
        (self.cfg, self.mir) = self.session.clone();
        self.commit();
    }

    /// The files changed since the last read or write (a hand edit): adopt
    /// them instead of going back. A history snapshot predates that edit, and
    /// its diff against the old base would write the old value over it. A
    /// plain commit needs no check: only the keys it changed are written.
    fn outside_edit(&mut self) -> bool {
        if self.read_error.is_some() || mtimes(&self.paths) == self.mtimes {
            return false;
        }
        self.sync_disk(true);
        self.toast = Some((
            Instant::now(),
            "Files changed outside Settings; Undo restarted",
        ));
        true
    }

    fn restore(&mut self, state: State, toast: &'static str) {
        (self.cfg, self.mir) = state.clone();
        self.applied = state;
        self.write();
        self.toast_if_written(toast);
    }

    /// Write the committed state: only the keys whose text differs from what
    /// the files say, applied to each file as it is on disk now, so lines
    /// edited elsewhere while Settings is open survive. Every changed file is
    /// read before any is written; one that cannot be read aborts with both
    /// untouched.
    fn write(&mut self) {
        if self.read_error.is_some() {
            return;
        }
        let external = mtimes(&self.paths) != self.mtimes;
        let (wm, nav) = diff(&self.cfg, &self.disk);
        let templates = [default_config_text(), default_navbar_text()];
        let mut writes = Vec::new();
        for ((path, template), updates) in self.paths.iter().zip(templates).zip([&wm, &nav]) {
            if updates.is_empty() {
                continue;
            }
            match read_conf(path, template) {
                Ok(text) => writes.push((path, apply_updates(&text, updates))),
                Err(e) => {
                    self.error = Some(format!("Not saved: {e}"));
                    return;
                }
            }
        }
        self.error = None;
        for (path, text) in writes {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = write_atomic(path, &text) {
                self.error = Some(format!("Couldn't write {}: {e}", path.display()));
            }
        }
        self.sync_disk(external);
    }

    /// Re-read both files as the base for the next write (so a half-failed
    /// write is retried, not forgotten). `adopt` also takes their values into
    /// the editor: after someone else edited them, a Troubleshooting fix or a
    /// Reload. That restarts the session (Undo history and "Undo all"): an
    /// older snapshot would write its stale view of the outside edit back.
    /// Otherwise, a value the files do not read back is reported: a later line
    /// or the other file sets the same key (or an alias of it), or the parser
    /// rejected the value.
    fn sync_disk(&mut self, adopt: bool) {
        self.mtimes = mtimes(&self.paths);
        let disk = match read_pair(&self.paths) {
            Ok(d) => d,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        if adopt {
            let state = (disk.clone(), Mirrors::from(&disk));
            (self.cfg, self.mir) = state.clone();
            self.applied = state.clone();
            self.session = state;
            self.history = History::default();
        } else if self.error.is_none() {
            let (wm, nav) = diff(&self.cfg, &disk);
            let keys: Vec<&str> = wm.iter().chain(&nav).map(|p| p.0).collect();
            if !keys.is_empty() {
                self.error = Some(format!(
                    "Saved, but Astur reads a different value for {}: another line sets it again (later in the file, or in the other file), or the value is not valid.",
                    keys.join(", ")
                ));
            }
        }
        self.disk = disk;
    }

    /// Rename or remove a line the parser did not understand (`entry` is
    /// "file: key" from `unknown_keys`), then take the files as they are.
    fn fix_line(&mut self, entry: &str, to: Option<&str>) {
        if self.read_error.is_some() {
            return;
        }
        // The banner was about the files as they were; a failed write or
        // re-read below sets a fresh one.
        self.error = None;
        self.flush();
        let Some((file, key)) = entry.split_once(": ") else {
            return;
        };
        let path = &self.paths[usize::from(file == "navbar.conf")];
        match astur_config::read_existing(path) {
            Ok(Some(text)) => {
                if let Err(e) = write_atomic(path, &edit_key(&text, key, to)) {
                    self.error = Some(format!("Couldn't write {}: {e}", path.display()));
                    return;
                }
            }
            Ok(None) => {}
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        }
        self.sync_disk(true);
    }

    /// Stamp a bar preset's keys into the config as one undoable change, and
    /// show Astur's bar (Home's Quick setup offers a style with the bar off).
    /// The first time each session navbar.conf is copied to navbar.conf.bak
    /// first, so the bar as it was before stays one file copy away.
    fn apply_preset(&mut self, p: &BarPreset) {
        if self.read_error.is_some() {
            return;
        }
        self.flush(); // an edit in progress is its own undo step
        if !self.backed_up {
            let mut bak = self.paths[1].as_os_str().to_owned();
            bak.push(".bak");
            match std::fs::copy(&self.paths[1], &bak) {
                Ok(_) => {}
                // No navbar.conf yet: the template is all there is to keep.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    self.error = Some(format!(
                        "Preset not applied: couldn't back up navbar.conf: {e}"
                    ));
                    return;
                }
            }
            self.backed_up = true;
        }
        self.cfg = stamp(&self.cfg, p);
        self.cfg.bar_enabled = true;
        self.commit();
    }

    /// The Astur key and the custom shortcuts that follow it, as one undo step.
    /// `from` is the key the shortcuts were written under: during a trial
    /// the key before it, not the trial key. Win -> Left Alt -> Win must not
    /// read the user's real-Alt `ALT+GRAVE` as a Left Alt one and move it to
    /// Win; going back to the trial's own key (`from == to`) rewrites nothing.
    fn set_main_key(&mut self, from: MainKey, to: MainKey) {
        self.flush(); // an edit in progress is its own undo step
        follow_key(&mut self.cfg.extra_hotkeys, from, to);
        follow_key(&mut self.mir.rows.hotkeys, from, to);
        self.cfg.main_key = to;
        self.commit();
    }

    /// Make `to` the Astur key, on trial until kept (`TRIAL`). A second switch
    /// during a trial still goes back to the key before the first.
    fn switch_key(&mut self, to: MainKey) {
        if self.read_error.is_some() || to == self.cfg.main_key {
            return;
        }
        let from = self.trial.take().map_or(self.cfg.main_key, |t| t.from);
        self.set_main_key(from, to);
        if to != from {
            self.start_trial(from);
        }
    }

    /// Put the key just committed on trial, going back to `from`.
    fn start_trial(&mut self, from: MainKey) {
        self.trial = Some(Trial {
            from,
            after: self.applied.clone(),
            depth: self.history.undo.len(),
            until: Instant::now() + TRIAL,
        });
    }

    /// End a trial by going back: Undo when nothing was committed since the
    /// switch (the file reads exactly as before), else a new step that sets
    /// only the key back (the shortcuts were never rewritten for it).
    fn revert_key(&mut self) {
        let Some(t) = self.trial.take() else {
            return;
        };
        if !self.pending() && self.history.undo.len() == t.depth && self.applied == t.after {
            self.undo();
        }
        if self.cfg.main_key != t.from {
            self.set_main_key(t.from, t.from);
        }
    }

    /// Run the trial's clock: revert when it runs out, drop it when the key
    /// changed some other way (Undo, Undo all, a reload). Not mid-gesture:
    /// reverting flushes the edit in progress, and a half-typed command or a
    /// slider mid-drag must not be written (`settle`'s rule); the banner
    /// shows 0 s until the gesture ends.
    fn tick_trial(&mut self, ctx: &egui::Context, now: Instant) {
        let Some(t) = &self.trial else {
            return;
        };
        if self.cfg.main_key != t.after.0.main_key {
            self.trial = None;
        } else if now >= t.until && !busy(ctx) {
            self.revert_key();
        } else {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    /// Take the files as they are now. From read-only mode this is a fresh
    /// start: that session's "as loaded" state was the built-in defaults, and
    /// "Undo all" must never write those over the user's files.
    fn reload(&mut self) {
        if self.read_error.is_some() {
            let page = self.page;
            *self = App::load(self.paths.clone());
            self.page = page;
        } else {
            // As in fix_line: a failed flush or re-read sets a fresh banner.
            self.error = None;
            self.flush();
            self.sync_disk(true);
        }
    }

    fn ui(&mut self, ctx: &egui::Context) {
        ctx.set_theme(match self.cfg.theme.as_str() {
            "light" => egui::ThemePreference::Light,
            "auto" => egui::ThemePreference::System,
            _ => egui::ThemePreference::Dark,
        });
        self.keys(ctx);
        self.tick_trial(ctx, Instant::now());
        self.find = Find {
            q: self.query.trim().to_lowercase(),
            ..Find::default()
        };
        egui::SidePanel::left("nav")
            .resizable(false)
            .exact_width(244.0)
            .frame(
                egui::Frame::new()
                    .fill(ctx.style().visuals.panel_fill)
                    .inner_margin(Margin::symmetric(10, 0)),
            )
            .show(ctx, |ui| self.nav(ui));
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ctx.style().visuals.panel_fill)
                    .inner_margin(Margin {
                        left: 28,
                        right: 8,
                        top: 16,
                        bottom: 0,
                    }),
            )
            .show(ctx, |ui| {
                self.header(ui);
                egui::ScrollArea::vertical()
                    .id_salt((self.find.on(), self.page))
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        ui.set_max_width(ui.available_width().min(1000.0));
                        if self.find.on() {
                            self.search_results(ui);
                        } else {
                            self.page_ui(ui, self.page);
                        }
                        ui.add_space(32.0);
                    });
            });
        self.toast_ui(ctx);
        let busy = busy(ctx);
        if ctx.input(|i| i.viewport().close_requested()) {
            // Closing before Keep is not keeping: nothing would revert later.
            self.revert_key();
            self.flush();
            // A write that failed (a file held by a virus scan, say) gets one
            // more try rather than being dropped with the window.
            let (wm, nav) = diff(&self.cfg, &self.disk);
            if self.read_error.is_none() && !(wm.is_empty() && nav.is_empty()) {
                self.write();
            }
        } else if let Some(wait) = self.settle(Instant::now(), busy) {
            ctx.request_repaint_after(wait);
        }
    }

    fn keys(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};
        // A listening key chip owns every key: Ctrl+Z there records
        // Alt+Ctrl+Z, and must not undo (and write) instead.
        if ctx.data(|d| d.get_temp::<egui::Id>(capture_id())).is_some() {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::F)) {
            ctx.memory_mut(|m| m.request_focus(search_id()));
        }
        // A focused text box keeps Ctrl+Z / Ctrl+Y for its own text.
        if text_focus(ctx).is_some() {
            return;
        }
        // Shift+Z first: a plain Ctrl+Z shortcut also matches Ctrl+Shift+Z.
        if ctx.input_mut(|i| {
            i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
                || i.consume_key(Modifiers::COMMAND, Key::Y)
        }) {
            self.redo();
        } else if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Z)) {
            self.undo();
        }
    }

    fn nav(&mut self, ui: &mut Ui) {
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.label(RichText::new("Astur Settings").size(13.0).color(sub(ui)));
        });
        ui.add_space(10.0);
        let search = ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .id(search_id())
                .hint_text("Find a setting  (Ctrl+F)")
                .desired_width(f32::INFINITY)
                .margin(Margin::symmetric(10, 6)),
        );
        if search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.query.clear();
        }
        ui.add_space(8.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            for &(page, title, icon, group) in NAV {
                if !group.is_empty() {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label(RichText::new(group).size(12.0).color(sub(ui)));
                    });
                }
                let selected = self.page == page && !self.find.on();
                if nav_item(ui, selected, icon, title).clicked() {
                    self.page = page;
                    self.query.clear();
                    // A key chip left listening would wake up on return.
                    ui.ctx().data_mut(|d| d.remove::<egui::Id>(capture_id()));
                }
            }
            ui.add_space(12.0);
        });
    }

    fn header(&mut self, ui: &mut Ui) {
        let title = if self.find.on() {
            "Search results"
        } else {
            NAV.iter().find(|n| n.0 == self.page).map_or("", |n| n.1)
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).size(28.0).family(semibold()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(20.0);
                let icons = has_icons(ui);
                if ui
                    .add_enabled(
                        self.can_redo(),
                        egui::Button::new(glyph(icons, ICON_REDO, "Redo")),
                    )
                    .on_hover_text("Redo (Ctrl+Y)")
                    .clicked()
                {
                    self.redo();
                }
                if ui
                    .add_enabled(
                        self.can_undo(),
                        egui::Button::new(glyph(icons, ICON_UNDO, "Undo")),
                    )
                    .on_hover_text("Undo (Ctrl+Z)")
                    .clicked()
                {
                    self.undo();
                }
                if self.applied != self.session
                    && ui
                        .button("Undo all")
                        .on_hover_text("Undo all changes from this session")
                        .clicked()
                {
                    self.undo_all();
                }
                self.status(ui);
            });
        });
        self.trial_banner(ui);
        let v = ui.visuals();
        let banner = match (&self.read_error, &self.error) {
            (Some(e), _) => Some((v.warn_fg_color, format!("Read-only: {e}. Showing built-in defaults and writing nothing, so the file stays as it is. Save it as UTF-8 (or close the program holding it), then press Retry."))),
            (None, Some(e)) => Some((v.error_fg_color, e.clone())),
            _ => None,
        };
        if let Some((color, text)) = banner {
            ui.add_space(4.0);
            ui.colored_label(color, text);
        }
        ui.add_space(10.0);
    }

    /// "Keep this key?" with the seconds left, on every page: the user may
    /// have moved on before trying the new key.
    fn trial_banner(&mut self, ui: &mut Ui) {
        let Some(t) = &self.trial else {
            return;
        };
        let text = format!(
            "New Astur key: {}. Keep it? Going back to {} in {} s.",
            self.cfg.main_key.label(),
            t.from.label(),
            secs_left(t.until, Instant::now())
        );
        let (mut keep, mut back) = (false, false);
        ui.add_space(6.0);
        let v = ui.visuals();
        egui::Frame::new()
            .fill(v.faint_bg_color)
            .stroke(Stroke::new(1.0_f32, v.warn_fg_color))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(14, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(text).family(semibold()));
                    keep = ui.button("Keep").clicked();
                    back = ui.button("Undo").clicked();
                });
            });
        if keep {
            self.trial = None;
        } else if back {
            self.revert_key();
        }
    }

    /// The pill beside Undo: whether the last edit reached the files.
    fn status(&mut self, ui: &mut Ui) {
        if self.read_error.is_some() && ui.button("Retry").clicked() {
            self.reload();
        }
        let v = ui.visuals();
        let (text, color) = if self.read_error.is_some() {
            ("Read-only", v.warn_fg_color)
        } else if self.error.is_some() {
            ("Not applied", v.error_fg_color)
        } else if self.mir.rows.problems(self.cfg.workspaces) > 0 {
            ("Fix the red rows", v.warn_fg_color)
        } else if self.pending() {
            ("Applying\u{2026}", sub(ui))
        } else {
            ("All changes applied", sub(ui))
        };
        egui::Frame::new()
            .fill(v.faint_bg_color)
            .stroke(v.widgets.noninteractive.bg_stroke)
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::symmetric(10, 4))
            .show(ui, |ui| {
                ui.label(RichText::new(text).size(12.0).color(color))
            })
            .response
            .on_hover_text("Changes apply as you make them; Astur reloads them within a second. A list with a red row keeps its last good version in the file until the row is fixed.");
    }

    fn toast_ui(&mut self, ctx: &egui::Context) {
        let Some((at, text)) = self.toast else {
            return;
        };
        let left = Duration::from_secs(4).saturating_sub(at.elapsed());
        if left.is_zero() {
            self.toast = None;
            return;
        }
        ctx.request_repaint_after(left);
        egui::Area::new(egui::Id::new("toast"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-24.0, -24.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(text);
                        if self.can_undo() && ui.link("Undo").clicked() {
                            self.undo();
                        }
                    });
                });
            });
    }

    fn search_results(&mut self, ui: &mut Ui) {
        for &(page, title, _, _) in NAV {
            if page == Page::Home {
                continue; // its rows all live on other pages too
            }
            self.find.page = Some((page, title));
            self.find.section = None;
            self.page_ui(ui, page);
        }
        if self.find.hits == 0 {
            ui.label(format!(
                "No settings match \u{201C}{}\u{201D}.",
                self.query.trim()
            ));
        }
        if let Some(page) = self.find.goto.take() {
            self.page = page;
            self.query.clear();
        }
    }

    fn page_ui(&mut self, ui: &mut Ui, page: Page) {
        let editable = self.read_error.is_none()
            || matches!(page, Page::Home | Page::Troubleshooting | Page::About);
        // A scope only when needed: search draws every page, and an empty
        // scope still adds a line of spacing above the first hit.
        if editable {
            self.page_body(ui, page);
        } else {
            ui.add_enabled_ui(false, |ui| self.page_body(ui, page));
        }
    }

    fn page_body(&mut self, ui: &mut Ui, page: Page) {
        match page {
            Page::Home => self.home(ui),
            Page::Shortcuts => self.shortcuts(ui),
            Page::Layout => self.layout(ui),
            Page::Workspaces => self.workspaces(ui),
            Page::Windows => self.windows(ui),
            Page::Appearance => self.appearance(ui),
            Page::Motion => self.motion(ui),
            Page::Bar => self.bar(ui),
            Page::Launcher => self.launcher(ui),
            Page::Power => self.power(ui),
            Page::Extras => self.extras(ui),
            Page::Integrations => self.integrations(ui),
            Page::Troubleshooting => self.troubleshooting(ui),
            Page::About => self.about(ui),
        }
    }
}

// ---- pages -------------------------------------------------------------------

impl App {
    fn home(&mut self, ui: &mut Ui) {
        if self.find.on() {
            return;
        }
        let unknown = self.disk.unknown_keys.len();
        let (ok, text) = match &self.read_error {
            Some(e) => (false, format!("Read-only: {e}")),
            None if unknown > 0 => (
                false,
                format!(
                    "{unknown} config line{} not understood, so {} nothing",
                    if unknown == 1 { "" } else { "s" },
                    if unknown == 1 { "it does" } else { "they do" }
                ),
            ),
            None => (true, "Both config files OK".to_string()),
        };
        let mut review = false;
        card(ui, |ui| {
            ui.horizontal(|ui| {
                let v = ui.visuals();
                let color = if ok { accent(ui) } else { v.warn_fg_color };
                if has_icons(ui) {
                    let icon = if ok { ICON_OK } else { ICON_WARN };
                    ui.label(
                        RichText::new(icon)
                            .family(icons_family())
                            .size(16.0)
                            .color(color),
                    );
                }
                ui.label(text);
                if !ok {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        review = ui.button("Review").clicked();
                    });
                }
            });
        });
        if review {
            self.page = Page::Troubleshooting;
        }
        // The bar preset in use, if the bar is one exactly ("" = its own).
        let bar_now = if self.cfg.bar_enabled {
            (BAR_PRESETS.iter())
                .find(|p| preset_diff(&self.cfg, &stamp(&self.cfg, p)).is_empty())
                .map_or("", |p| p.name)
        } else {
            "off"
        };
        let (mut preset, mut all_keys) = (None, false);
        let launcher = format!("App launcher ({}Space)", mk(&self.cfg));
        let tile = tile_hint(&self.cfg);
        // The health card above stays clickable when read-only (page_ui
        // leaves Home enabled); the settings below do not.
        let (c, d, f) = (&mut self.cfg, &self.def, &mut self.find);
        ui.add_enabled_ui(self.read_error.is_none(), |ui| {
            section(ui, f, "Quick setup");
            note(ui, "The basics, one click each. Every change applies at once, and Undo (Ctrl+Z) takes it back.");
            theme_row(ui, f, &mut c.theme, &d.theme);
            layout_row(ui, f, &mut c.layout, &d.layout);
            preset = bar_style_row(ui, f, bar_now, &mut c.bar_enabled);
            toggle(
                ui,
                f,
                ("Animations", "Windows and workspaces glide into place. Choose the style on the Motion page.", "animations"),
                &mut c.animations,
                d.animations,
            );
            gaps_row(ui, f, c, d);
            section(ui, f, "How to use Astur");
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    let key = c.main_key.label();
                    ui.label(format!("Hold {key}, the Astur key, for every shortcut below."));
                    all_keys |= ui.link("Change").clicked();
                });
                ui.add_space(4.0);
                egui::Grid::new("tips")
                    .num_columns(2)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        for (keys, what) in tips(c) {
                            ui.label(RichText::new(keys).family(semibold()));
                            ui.label(RichText::new(what).color(sub(ui)));
                            ui.end_row();
                        }
                    });
                ui.add_space(4.0);
                all_keys |= ui.link("See every shortcut").clicked();
            });
            section(ui, f, "Quick switches");
            for (meta, v, dv) in [
                ((TILE_LABEL, tile.as_str(), "start_tiled"), &mut c.start_tiled, d.start_tiled),
                (("Status bar", "Workspaces, the window title and system stats on your screens (which ones: Status bar page).", "enabled"), &mut c.bar_enabled, d.bar_enabled),
                ((launcher.as_str(), "Search apps, files, sums and the web.", "launcher_enabled"), &mut c.launcher_enabled, d.launcher_enabled),
                (("Focus follows mouse", "Pointing at a window makes it the active one, no click needed.", "focus_follows_mouse"), &mut c.focus_follows_mouse, d.focus_follows_mouse),
            ] {
                toggle(ui, f, meta, v, dv);
            }
        });
        if let Some(p) = preset {
            self.apply_preset(p);
        }
        if all_keys {
            self.page = Page::Shortcuts;
        }
        ui.add_space(16.0);
        if ui
            .add_enabled(
                self.applied != self.session,
                egui::Button::new("Undo all changes from this session"),
            )
            .clicked()
        {
            self.undo_all();
        }
    }

    fn shortcuts(&mut self, ui: &mut Ui) {
        let picked = key_picker(ui, &mut self.find, &self.cfg);
        let (c, d, f) = (&mut self.cfg, &self.def, &mut self.find);
        let (m, dm) = (&mut self.mir, &self.def_mir);
        let issues = compile_keys(c).issues;
        let (k, held, name) = (mk(c), held(c), c.main_key.label());
        let live = binds(c);
        let (mut keys, mut go_ws) = (false, false);
        if !f.on() {
            note(ui, &format!("Hold {name} and press the key shown. To change a shortcut, click its key and press the new one ({held} is added for you). Esc cancels, Backspace clears."));
        }
        section(ui, f, "Windows");
        for (i, &meta) in BINDS.iter().enumerate().take(8) {
            let k = (&mut m.rows.keys[i], dm.rows.keys[i].as_str());
            let n = notes(&issues, |x| BIND_ACTIONS[i].contains(&x.action));
            keys |= key_row(ui, f, meta, k, live[i], (&held, &n), i);
        }
        section(ui, f, "Workspaces");
        let ws_keys: Vec<&str> = (m.rows.ws.iter().take(c.workspaces))
            .map(|w| w.key.as_str())
            .filter(|k| !k.is_empty())
            .collect();
        let ws_notes = notes(&issues, |x| {
            matches!(x.action, Action::SwitchWorkspace | Action::MoveToWorkspace)
        });
        let ws_desc = format!("{held} + a workspace's key switches to it; with Shift it takes the active window there. Set them in the Workspaces table.");
        row_noted(
            ui,
            f,
            ("Workspace keys", ws_desc.as_str(), "workspace_keys"),
            false,
            false,
            &ws_notes,
            |ui| {
                go_ws = ui.button("Edit").clicked();
                ui.label(ws_keys.join(" "));
            },
        );
        section(ui, f, "Launch apps");
        let apps = APPS.get();
        let terminal = (
            format!("Terminal ({k}Enter)"),
            format!("The command-line app {k}Enter opens. The list shows the ones on this PC."),
        );
        let browser = (
            format!("Browser ({k}Shift+Enter)"),
            format!("The web browser {k}Shift+Enter opens. System default is the one Windows opens links with."),
        );
        choice_row(
            ui,
            f,
            (terminal.0.as_str(), terminal.1.as_str(), "terminal"),
            (&mut c.terminal, &d.terminal),
            apps.map_or(&[], |a| a.terminals.as_slice()),
            Some(Pick::App),
            "shortcuts",
        );
        let browsers: Vec<Choice> = std::iter::once(Choice::new("System default", ""))
            .chain(apps.into_iter().flat_map(|a| a.browsers.iter().cloned()))
            .collect();
        choice_row(
            ui,
            f,
            (browser.0.as_str(), browser.1.as_str(), "browser"),
            (&mut c.browser, &d.browser),
            &browsers,
            Some(Pick::App),
            "shortcuts",
        );
        section(ui, f, "Custom shortcuts");
        let scratch_off = !c.scratchpad_enabled;
        // The notes are about the list Astur has, which is the table's only
        // while every row in it could be written (fold_hotkeys).
        let extra_issues = if c.extra_hotkeys == m.rows.hotkeys {
            issues.as_slice()
        } else {
            &[]
        };
        let (main, ws) = (c.main_key, c.workspaces);
        let extra_desc = format!("Hold {name} (or the keys you switch on) and press a key to run an action. A custom shortcut wins over Astur's own keys, except the launcher and power menu ones.");
        if table(
            ui,
            f,
            ("Extra hotkeys", extra_desc.as_str(), "extra_hotkeys"),
            &mut m.rows.hotkeys,
            &dm.rows.hotkeys,
            (
                "+ Add shortcut",
                HotkeyDef {
                    chord: "MOD+".into(),
                    action: "launch".into(),
                    argument: String::new(),
                },
            ),
            |ui, i, h| hotkey_ui(ui, i, h, (main, extra_issues, ws), scratch_off),
        ) {
            fold_hotkeys(&m.rows.hotkeys, c);
        }
        more(ui, f, "shortcuts", |ui, f| {
            section(ui, f, "Commands");
            let terminal = format!("What {k}Enter runs, typed out: for an app with options, e.g. wt -p Ubuntu. Put a path with spaces in quotes when options follow it.");
            text(
                ui,
                f,
                ("Terminal command", terminal.as_str(), "terminal"),
                &mut c.terminal,
                &d.terminal,
            );
            let browser = format!("What {k}Shift+Enter runs. Empty: the system default browser.");
            text(
                ui,
                f,
                ("Browser command", browser.as_str(), "browser"),
                &mut c.browser,
                &d.browser,
            );
            section(ui, f, "Scrolling layout keys");
            for (i, &meta) in BINDS.iter().enumerate().skip(8) {
                let k = (&mut m.rows.keys[i], dm.rows.keys[i].as_str());
                let n = notes(&issues, |x| BIND_ACTIONS[i].contains(&x.action));
                keys |= key_row(ui, f, meta, k, live[i], (&held, &n), i);
            }
        });
        if keys {
            fold_keys(&m.rows.keys, c);
        }
        if !f.on() {
            section_title(ui, "Fixed shortcuts");
            card(ui, |ui| {
                egui::Grid::new("fixed")
                    .num_columns(2)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        for (keys, what, acts) in fixed(c) {
                            ui.label(RichText::new(keys).family(semibold()));
                            ui.vertical(|ui| {
                                ui.label(RichText::new(what).color(sub(ui)));
                                note_labels(ui, &notes(&issues, |x| acts.contains(&x.action)));
                            });
                            ui.end_row();
                        }
                    });
            });
            section_title(ui, "Mouse");
            card(ui, |ui| {
                egui::Grid::new("mouse")
                    .num_columns(2)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        for (keys, what) in mouse(c) {
                            ui.label(RichText::new(keys).family(semibold()));
                            ui.label(RichText::new(what).color(sub(ui)));
                            ui.end_row();
                        }
                    });
            });
            ui.add_space(8.0);
            if ui
                .button("Copy cheat sheet")
                .on_hover_text("Every shortcut as plain text, for a note or a printout")
                .clicked()
            {
                ui.ctx().copy_text(cheat_sheet(c));
            }
        }
        if go_ws {
            self.page = Page::Workspaces;
        }
        if let Some(k) = picked {
            self.switch_key(k);
        }
    }

    fn layout(&mut self, ui: &mut Ui) {
        let keys = |shift, vk| chord_label(self.cfg.main_key.bits() | shift, vk);
        let (width, height, tab) = (
            keys(0, self.cfg.key_cycle_width),
            keys(MOD_SHIFT, self.cfg.key_cycle_width),
            keys(MOD_SHIFT, self.cfg.key_toggle_tiling),
        );
        let (c, d, f) = (&mut self.cfg, &self.def, &mut self.find);
        section(ui, f, "Tiling");
        layout_row(ui, f, &mut c.layout, &d.layout);
        pct(
            ui,
            f,
            ("Main window size", "How much of the screen's width the big window gets in Main + side, and the first split in Spiral.", "master_ratio"),
            &mut c.master_ratio,
            d.master_ratio,
            0.10..=0.90,
        );
        section(ui, f, "Gaps");
        num(
            ui,
            f,
            (
                "Outer gap",
                "Space between windows and the screen edges.",
                "outer_gap",
            ),
            &mut c.outer_gap,
            d.outer_gap,
            0..=100,
            " px",
        );
        num(
            ui,
            f,
            (
                "Inner gap",
                "Space between neighbouring windows.",
                "inner_gap",
            ),
            &mut c.inner_gap,
            d.inner_gap,
            0..=100,
            " px",
        );
        section(ui, f, "Per monitor");
        let opts: Vec<(&str, &str)> = std::iter::once(("", "Same as default"))
            .chain(LAYOUTS.iter().map(|&l| (l, layout_label(l))))
            .collect();
        // Always offer three rows so a second or third monitor can be set up
        // before it is plugged in; more appear once the config names them.
        for i in 0..c.monitor_layouts.len().max(3) {
            let before = c.monitor_layouts.get(i).cloned().unwrap_or_default();
            let mut cur = before.clone();
            let label = match i {
                0 => "Monitor 1 (leftmost)".to_string(),
                i => format!("Monitor {}", i + 1),
            };
            let desc = if i == 0 {
                "Monitors are counted from left to right, here and in window rules. A layout shortcut changes only the monitor you are on."
            } else {
                ""
            };
            pick(
                ui,
                f,
                (label.as_str(), desc, "monitor_layouts"),
                &mut cur,
                "",
                &opts,
            );
            if cur != before {
                // Positional list: pad with defaults so the entry lands on
                // monitor i + 1, not the next free slot.
                let ml = &mut c.monitor_layouts;
                if ml.len() <= i {
                    ml.resize(i + 1, String::new());
                }
                ml[i] = cur;
                trim_defaults(ml);
            }
        }
        section(ui, f, "Scrolling layout");
        pct(
            ui,
            f,
            (
                "New column width",
                "Width of a new column, as a share of the screen.",
                "scroll_default_width",
            ),
            &mut c.scroll_default_width,
            d.scroll_default_width,
            0.10..=1.00,
        );
        pick(
            ui,
            f,
            ("Keep the focused column centred", "", "scroll_center"),
            &mut c.scroll_center,
            &d.scroll_center,
            &[
                ("never", "Never: scroll only as far as needed"),
                ("always", "Always"),
                ("on-overflow", "Only when it doesn't fit"),
            ],
        );
        let label = format!("Column widths ({width})");
        let desc = format!("The widths {width} steps through, as a share of the screen.");
        shares_row(
            ui,
            f,
            (label.as_str(), desc.as_str(), "scroll_width_presets"),
            (&mut c.scroll_width_presets, &d.scroll_width_presets),
            10..=100,
        );
        let label = format!("Window heights ({height})");
        let desc = format!("The heights {height} steps through, for a window sharing a column.");
        shares_row(
            ui,
            f,
            (label.as_str(), desc.as_str(), "scroll_height_presets"),
            (&mut c.scroll_height_presets, &d.scroll_height_presets),
            10..=90,
        );
        more(ui, f, "layout", |ui, f| {
            let desc = format!("The tab strip of a tabbed column ({tab}).");
            num(
                ui,
                f,
                ("Tab bar height", desc.as_str(), "tab_bar_height"),
                &mut c.tab_bar_height,
                d.tab_bar_height,
                16..=64,
                " px",
            );
        });
    }

    fn workspaces(&mut self, ui: &mut Ui) {
        let (c, d, f) = (&mut self.cfg, &self.def, &mut self.find);
        let (m, dm) = (&mut self.mir, &self.def_mir);
        section(ui, f, "Workspaces");
        let mut mode = if c.per_monitor {
            "per_monitor"
        } else {
            "shared"
        }
        .to_string();
        pick(
            ui,
            f,
            ("With several monitors", "Shared: one numbered set, spread across the monitors from the main one outward. Each monitor has its own: every monitor has workspaces 1 to N, and the keys act on the monitor you are on.", "workspace_mode"),
            &mut mode,
            "shared",
            &[
                ("shared", "One set shared by all monitors"),
                ("per_monitor", "Each monitor has its own"),
            ],
        );
        c.per_monitor = mode == "per_monitor";
        let keys = (compile_keys(c).issues, held(c));
        let defs = (dm.rows.ws.as_slice(), d.workspaces);
        let keys = (keys.0.as_slice(), keys.1.as_str());
        if ws_table(ui, f, &mut m.rows.ws, &mut c.workspaces, defs, keys) {
            fold_ws(&m.rows.ws, c);
        }
    }

    fn windows(&mut self, ui: &mut Ui) {
        let (c, d, f) = (&mut self.cfg, &self.def, &mut self.find);
        let (m, dm) = (&mut self.mir, &self.def_mir);
        section(ui, f, "Behaviour");
        let tile = tile_hint(c);
        toggle(
            ui,
            f,
            (TILE_LABEL, &tile, "start_tiled"),
            &mut c.start_tiled,
            d.start_tiled,
        );
        toggle(
            ui,
            f,
            (
                "Focus follows mouse",
                "Pointing at a window makes it the active one, no click needed.",
                "focus_follows_mouse",
            ),
            &mut c.focus_follows_mouse,
            d.focus_follows_mouse,
        );
        toggle(
            ui,
            f,
            (
                "Move the mouse with the focus",
                "When a shortcut moves to another window or workspace, the pointer jumps to that window too.",
                "cursor_follows_focus",
            ),
            &mut c.cursor_follows_focus,
            d.cursor_follows_focus,
        );
        // Only Left Alt is Astur's own Alt; with any other Astur key Alt+Tab
        // stays Windows' (the WM ignores the setting then).
        // (A scope only when disabled: an empty one adds spacing in search.)
        let mut alt_tab = |ui: &mut Ui| {
            toggle(
                ui,
                f,
                (
                    "Astur's Alt+Tab switcher",
                    "Replaces the Windows task switcher with one that knows about workspaces. Needs Left Alt as the Astur key (Shortcuts).",
                    "alt_tab_replacement",
                ),
                &mut c.alt_tab_replacement,
                d.alt_tab_replacement,
            )
        };
        if c.main_key == MainKey::LAlt {
            alt_tab(ui);
        } else {
            ui.add_enabled_ui(false, alt_tab);
        }
        section(ui, f, "Rules");
        if table(
            ui,
            f,
            ("App rules", "What Astur does with an app's new windows: always float them (they keep their own size and place), always tile them, or leave them alone (Astur never touches them), and which workspace or monitor they open on. Pick a window that is open now, or type the app's file name, e.g. notepad.exe. The first matching rule wins. Older class-only rows stay at the bottom: a Leave alone one wins over every rule.", "window_rules, ignore_classes, float_classes"),
            &mut m.rows.rules,
            &dm.rows.rules,
            (
                "+ Add rule",
                RuleRow {
                    list: false,
                    rule: WindowRule {
                        action: "float".into(),
                        exe: String::new(),
                        class: String::new(),
                        title: String::new(),
                        workspace: None,
                        monitor: None,
                    },
                },
            ),
            |ui, _, r| rule_ui(ui, r),
        ) {
            // The WM checks the class lists apart from the rules (see the
            // description), so their place in the table means nothing: a
            // stable sort snaps them back below and keeps the rules' order.
            m.rows.rules.sort_by_key(|r| r.list);
            fold_rules(&m.rows.rules, c);
        }
        more(ui, f, "windows", |ui, f| {
            if !f.on() {
                warn_note(ui, "Expert settings: they change how Astur talks to Windows. Leave them on unless something misbehaves.");
            }
            toggle(
                ui,
                f,
                ("Place windows without waiting for each app", "On: a re-tile finishes after the slowest app, not after every app in turn. Turn off only if an app lands in the wrong place after a re-tile.", "async_window_pos"),
                &mut c.async_window_pos,
                d.async_window_pos,
            );
            toggle(
                ui,
                f,
                ("Reliable focus switching", "Turns off Windows' focus-stealing guard for the whole system while Astur runs (put back when Astur exits). Off: a focus change may only flash the taskbar button.", "foreground_lock_disable"),
                &mut c.foreground_lock_disable,
                d.foreground_lock_disable,
            );
        });
    }

    fn appearance(&mut self, ui: &mut Ui) {
        let (c, d, f) = (&mut self.cfg, &self.def, &mut self.find);
        section(ui, f, "Theme");
        theme_row(ui, f, &mut c.theme, &d.theme);
        section(ui, f, "Windows");
        let cur = dim_pct(c.unfocused_opacity);
        let mut dim = cur;
        num(
            ui,
            f,
            ("Dim inactive windows", "Fades the windows you aren't using, so the active one stands out. 0% turns it off (a few games and overlays dislike being dimmed).", "unfocused_opacity"),
            &mut dim,
            dim_pct(d.unfocused_opacity),
            0..=90,
            "%",
        );
        if dim != cur {
            c.unfocused_opacity = opacity_for_dim(dim);
        }
        toggle(
            ui,
            f,
            (
                "Coloured window borders",
                "Windows 11 only.",
                "border_enabled",
            ),
            &mut c.border_enabled,
            d.border_enabled,
        );
        when(ui, f, c.border_enabled, |ui, f| {
            color(
                ui,
                f,
                ("Active window's border", "", "focused_border"),
                &mut c.focused_border,
                d.focused_border,
            );
            color(
                ui,
                f,
                ("Other windows' border", "", "unfocused_border"),
                &mut c.unfocused_border,
                d.unfocused_border,
            );
        });
        section(ui, f, "Launcher and menus");
        font_row(
            ui,
            f,
            (
                "Font",
                "The lettering of the launcher and menus.",
                "popup_font_name",
            ),
            &mut c.popup_font_name,
            &d.popup_font_name,
        );
        num(
            ui,
            f,
            ("Text size", "", "popup_font_size"),
            &mut c.popup_font_size,
            d.popup_font_size,
            10..=48,
            " px",
        );
        num(
            ui,
            f,
            ("Corner radius", "", "popup_radius"),
            &mut c.popup_radius,
            d.popup_radius,
            0..=48,
            " px",
        );
        num(
            ui,
            f,
            (
                "Opacity",
                "100% is solid; less lets the desktop show through.",
                "popup_opacity",
            ),
            &mut c.popup_opacity,
            d.popup_opacity,
            20..=100,
            "%",
        );
        toggle(
            ui,
            f,
            ("Blur behind (experimental)", "Frosted glass behind the launcher and menus. It can misbehave on some PCs; turn it off if they look wrong.", "acrylic"),
            &mut c.acrylic,
            d.acrylic,
        );
        more(ui, f, "appearance", |ui, f| {
            num(
                ui,
                f,
                (
                    "Font weight",
                    "How bold the text is: 400 is regular, 700 bold.",
                    "popup_font_weight",
                ),
                &mut c.popup_font_weight,
                d.popup_font_weight,
                100..=900,
                "",
            );
            num(
                ui,
                f,
                ("Border width", "", "popup_border_width"),
                &mut c.popup_border_width,
                d.popup_border_width,
                0..=8,
                " px",
            );
            section(ui, f, "Popup colours");
            // Auto = follow the theme. Each legacy hex below reads back as
            // Auto, so a picked colour equal to it is nudged (pin_theme_color).
            for (meta, v, dv, legacy) in [
                (
                    ("Background", "", "popup_bg"),
                    &mut c.popup_bg,
                    d.popup_bg,
                    0x0016_1616,
                ),
                (
                    ("Text", "", "popup_fg"),
                    &mut c.popup_fg,
                    d.popup_fg,
                    0x00E6_E6E6,
                ),
                (
                    ("Secondary text", "", "popup_muted"),
                    &mut c.popup_muted,
                    d.popup_muted,
                    0x0089_8989,
                ),
                (
                    ("Accent", "", "popup_accent"),
                    &mut c.popup_accent,
                    d.popup_accent,
                    0x0082_6333,
                ),
                (
                    ("Text on accent", "", "popup_accent_fg"),
                    &mut c.popup_accent_fg,
                    d.popup_accent_fg,
                    0x00FF_FFFF,
                ),
                (
                    ("Border", "", "popup_border"),
                    &mut c.popup_border,
                    d.popup_border,
                    0x0033_2A26,
                ),
            ] {
                theme_color(ui, f, meta, v, dv, legacy, legacy);
            }
        });
    }

    fn motion(&mut self, ui: &mut Ui) {
        let (c, d, f) = (&mut self.cfg, &self.def, &mut self.find);
        section(ui, f, "Animation");
        let cur = motion_preset(c).unwrap_or("custom");
        let mut sel = cur.to_string();
        row(
            ui,
            f,
            ("Animation style", "Off, or how workspace switches and window moves animate. The switch underneath is always instant; the animation is drawn on top.", "animations, animation_ms, workspace_anim, window_anim, animation_easing"),
            &mut sel,
            &"smooth".to_string(),
            |ui, v| {
                seg(ui, v, &MOTION_OPTS);
                if *v == "custom" {
                    ui.label(RichText::new("Custom").color(sub(ui)));
                }
            },
        );
        if sel != cur {
            set_motion(c, &sel);
        }
        // The legacy key silently forces workspace_anim off (astur main.rs):
        // without this, choosing Springy could do nothing on a workspace switch.
        let mut fix = false;
        if !f.on() && !c.workspace_slide && c.workspace_anim != "off" {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "An old setting (workspace_slide = false) turns workspace animation off.",
                    );
                    fix = ui.button("Fix").clicked();
                });
            });
        }
        if fix {
            c.workspace_slide = true;
        }
        more(ui, f, "motion", |ui, f| {
            toggle(
                ui,
                f,
                ("Animations", "Off: everything is instant.", "animations"),
                &mut c.animations,
                d.animations,
            );
            num(
                ui,
                f,
                ("Duration", "How long each animation takes (1000 ms is a second). Up to 250 ms your clicks still go through while it plays; longer ones hold the mouse until it ends. 0 turns animation off.", "animation_ms"),
                &mut c.animation_ms,
                d.animation_ms,
                0..=500,
                " ms",
            );
            pick(
                ui,
                f,
                ("Workspace switch", "", "workspace_anim"),
                &mut c.workspace_anim,
                &d.workspace_anim,
                &[
                    ("off", "Off"),
                    ("slide", "Slide"),
                    ("spring", "Spring (overshoots, then settles)"),
                    ("fade", "Fade"),
                ],
            );
            pick(
                ui,
                f,
                (
                    "Window moves",
                    "Opening, closing, moving and re-tiling windows.",
                    "window_anim",
                ),
                &mut c.window_anim,
                &d.window_anim,
                &[("off", "Off"), ("glide", "Glide"), ("spring", "Spring")],
            );
            pick(
                ui,
                f,
                (
                    "Motion curve",
                    "How the movement speeds up and slows down.",
                    "animation_easing",
                ),
                &mut c.animation_easing,
                &d.animation_easing,
                &[
                    ("cubic", "Quick, then gentle"),
                    ("smooth", "Smooth"),
                    ("spring", "Springy (a little bounce)"),
                ],
            );
        });
    }

    fn bar(&mut self, ui: &mut Ui) {
        let (c, f) = (&mut self.cfg, &mut self.find);
        section(ui, f, "Bar");
        let now = match (c.bar_enabled, c.yasb_enabled) {
            (true, _) => "astur",
            (false, true) => "yasb",
            _ => "none",
        };
        let mut which = now.to_string();
        seg_row(
            ui,
            f,
            (
                "Status bar",
                "Astur's own bar, YASB showing Astur's workspaces, or no bar.",
                "enabled, yasb_enabled",
            ),
            &mut which,
            "astur",
            &[("astur", "Astur bar"), ("yasb", "YASB"), ("none", "No bar")],
        );
        if which != now {
            c.bar_enabled = which == "astur";
            if which != "astur" {
                c.yasb_enabled = which == "yasb";
            }
        }
        let mut go_yasb = false;
        if !f.on() {
            if c.bar_enabled {
                card(ui, |ui| bar_preview(ui, c, 40.0));
            } else if c.yasb_enabled {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new("YASB shows Astur's workspaces once it is set up under")
                            .color(sub(ui)),
                    );
                    go_yasb = ui.link("Integrations").clicked();
                });
            }
        }
        if go_yasb {
            self.page = Page::Integrations;
        }
        // The rest is Astur's own bar: greyed while another is chosen. Not
        // while searching (a hit's page heading must stay clickable).
        if self.cfg.bar_enabled || self.find.on() {
            self.bar_body(ui);
        } else {
            ui.add_enabled_ui(false, |ui| self.bar_body(ui));
        }
    }

    fn bar_body(&mut self, ui: &mut Ui) {
        let roles = self.cfg.bar_roles(!ui.visuals().dark_mode);
        let apply = if self.find.on() {
            None
        } else {
            presets_ui(ui, &self.cfg)
        };
        let (c, d, f) = (&mut self.cfg, &self.def, &mut self.find);
        section(ui, f, "Look");
        palette_row(ui, f, &mut c.bar_palette, &d.bar_palette);
        let before = c.effective_bar_shape().to_string();
        let mut shape = before.clone();
        seg_row(
            ui,
            f,
            ("Shape", "Strip: edge to edge. Floating: detached from the edges, rounded. Islands: a rounded island per zone. Pills: a capsule per widget.", "shape"),
            &mut shape,
            d.effective_bar_shape(),
            &[
                ("strip", "Strip"),
                ("floating", "Floating"),
                ("islands", "Islands"),
                ("pills", "Pills"),
            ],
        );
        if shape != before {
            c.bar_shape.clone_from(&shape);
        }
        let shaped = matches!(shape.as_str(), "islands" | "pills");
        when(ui, f, shape == "floating", |ui, f| {
            num(
                ui,
                f,
                (
                    "Margin",
                    "Gap between a floating bar and the screen edges.",
                    "margin",
                ),
                &mut c.bar_margin,
                d.bar_margin,
                0..=48,
                " px",
            );
        });
        when(ui, f, shape != "strip", |ui, f| {
            num(
                ui,
                f,
                ("Corner radius", "Of a floating bar, each island or each pill. Half the height or more makes a capsule.", "radius"),
                &mut c.bar_radius,
                d.bar_radius,
                0..=40,
                " px",
            );
        });
        when(ui, f, shaped, |ui, f| {
            num(
                ui,
                f,
                (
                    "Island padding",
                    "Space between an island's or a pill's edge and its content.",
                    "island_padding",
                ),
                &mut c.bar_island_padding,
                d.bar_island_padding,
                0..=50,
                " px",
            );
        });
        seg_row(
            ui,
            f,
            (
                "Separators",
                "How a Separator widget looks. With the Pills shape, Arrows joins the pills into one strip of arrow-shaped pieces.",
                "separator_style",
            ),
            &mut c.bar_separator_style,
            &d.bar_separator_style,
            &[("line", "Line"), ("dot", "Dot"), ("powerline", "Arrows")],
        );
        toggle(
            ui,
            f,
            (
                "Highlight on hover",
                "A soft highlight under whatever you can click, while the mouse is over it.",
                "hover",
            ),
            &mut c.bar_hover,
            d.bar_hover,
        );
        num(
            ui,
            f,
            ("Height", "", "height"),
            &mut c.bar_height,
            d.bar_height,
            16..=64,
            " px",
        );
        num(
            ui,
            f,
            (
                "Edge padding",
                "Space between the screen edges and the outermost widgets.",
                "padding",
            ),
            &mut c.bar_padding,
            d.bar_padding,
            0..=48,
            " px",
        );
        font_row(
            ui,
            f,
            ("Font", "The lettering of the bar.", "font_name"),
            &mut c.bar_font_name,
            &d.bar_font_name,
        );
        num(
            ui,
            f,
            (
                "Text size",
                "0 = automatic (about half the bar height).",
                "font_size",
            ),
            &mut c.bar_font_size,
            d.bar_font_size,
            0..=40,
            " px",
        );
        section(ui, f, "Colours");
        if !f.on() {
            note(ui, "Auto takes each colour from the palette, in its light or dark version as the theme says. A colour you pick stays, whatever the theme.");
        }
        // The first four have an old shipped hex the parser reads as Auto, so
        // a pin of exactly it is nudged (theme_color); the newer keys have none.
        for (meta, v, dv, start, legacy) in [
            (
                ("Background", "", "bg"),
                &mut c.bar_bg,
                d.bar_bg,
                roles.bg,
                BAR_DARK[0],
            ),
            (
                ("Text", "", "fg"),
                &mut c.bar_fg,
                d.bar_fg,
                roles.fg,
                BAR_DARK[1],
            ),
            (
                ("Accent", "The active workspace's mark.", "accent"),
                &mut c.bar_accent,
                d.bar_accent,
                roles.accent,
                BAR_DARK[2],
            ),
            (
                ("Muted", "Empty workspaces and the layout.", "inactive"),
                &mut c.bar_inactive,
                d.bar_inactive,
                roles.muted,
                BAR_DARK[3],
            ),
            (
                (
                    "Text on accent",
                    "The active workspace's label on its mark. Set, it is also the text on every coloured pill (Auto: black or white to suit each).",
                    "accent_fg",
                ),
                &mut c.bar_accent_fg,
                d.bar_accent_fg,
                roles.accent_fg,
                NO_LEGACY,
            ),
        ] {
            theme_color(ui, f, meta, v, dv, start, legacy);
        }
        when(ui, f, shaped, |ui, f| {
            theme_color(
                ui,
                f,
                ("Surface", "Island and pill fill.", "surface"),
                &mut c.bar_surface,
                d.bar_surface,
                roles.surface,
                NO_LEGACY,
            );
        });
        when(ui, f, c.bar_border_width > 0, |ui, f| {
            theme_color(
                ui,
                f,
                ("Outline colour", "Auto is the muted colour.", "border"),
                &mut c.bar_border,
                d.bar_border,
                roles.muted,
                NO_LEGACY,
            );
        });
        when(ui, f, shape == "pills", |ui, f| {
            pill_colors(ui, f, c, d, &roles)
        });
        section(ui, f, "Widgets");
        if !f.on() {
            note(ui, "Drag a widget into a zone to show it, or down to the list below the zones to hide it. Click one for its settings.");
        }
        zone_row(ui, f, c, d, &mut self.bar_sel);
        if f.on() {
            for w in BAR_WIDGETS {
                widget_rows(ui, f, c, d, w);
            }
        } else {
            widget_panel(ui, f, c, d, &mut self.bar_sel);
        }
        pick(
            ui,
            f,
            (
                "Show stats as",
                "For CPU, memory, network, volume and battery.",
                "icon_mode",
            ),
            &mut c.bar_icon_mode,
            &d.bar_icon_mode,
            &[
                ("both", "Label and value"),
                ("icon", "Label only"),
                ("text", "Value only"),
                ("glyph", "Windows icon and value"),
                ("glyph_only", "Windows icons only"),
            ],
        );
        section(ui, f, "Workspaces");
        seg_row(
            ui,
            f,
            (
                "Active workspace",
                "How the current workspace is marked.",
                "workspace_style",
            ),
            &mut c.bar_workspace_style,
            &d.bar_workspace_style,
            &[
                ("box", "Box"),
                ("pill", "Pill"),
                ("underline", "Underline"),
                ("dot", "Dots"),
            ],
        );
        // Dots draw no labels and stay one size.
        when(ui, f, c.bar_workspace_style != "dot", |ui, f| {
            ws_label_row(ui, f, c, d);
            toggle(
                ui,
                f,
                (
                    "App icons in each workspace",
                    "Up to three icons of the apps open on each workspace, beside its label.",
                    "workspace_apps",
                ),
                &mut c.bar_workspace_apps,
                d.bar_workspace_apps,
            );
        });
        toggle(
            ui,
            f,
            (
                "Hide empty workspaces",
                "Show only the active workspace and those with windows.",
                "hide_empty_workspaces",
            ),
            &mut c.bar_hide_empty,
            d.bar_hide_empty,
        );
        section(ui, f, "Click actions");
        if !f.on() {
            note(ui, "What a click on a widget does, for the widgets on the bar. Workspaces, app buttons and the music buttons have their own clicks.");
        }
        click_rows(ui, f, c, d);
        section(ui, f, "Behaviour");
        for (meta, v, dv) in [
            (
                ("Dock at the bottom", "", "bottom"),
                &mut c.bar_bottom,
                d.bar_bottom,
            ),
            (
                ("Auto-hide", "The bar slides away and frees its space; move the mouse to the screen edge to bring it back.", "autohide"),
                &mut c.bar_autohide,
                d.bar_autohide,
            ),
        ] {
            toggle(ui, f, meta, v, dv);
        }
        monitors_row(ui, f, &mut c.bar_monitors, &d.bar_monitors);
        for (meta, v, dv) in [
            (
                (
                    "Wheel switches workspaces",
                    "Scrolling over the bar moves to the previous or next workspace.",
                    "wheel_workspaces",
                ),
                &mut c.bar_wheel_ws,
                d.bar_wheel_ws,
            ),
            (
                ("Tooltips", "", "show_tooltips"),
                &mut c.bar_show_tooltips,
                d.bar_show_tooltips,
            ),
            (
                ("Names beside app icons", "", "show_app_labels"),
                &mut c.bar_show_app_labels,
                d.bar_show_app_labels,
            ),
        ] {
            toggle(ui, f, meta, v, dv);
        }
        more(ui, f, "bar", |ui, f| {
            num(
                ui,
                f,
                (
                    "Text weight",
                    "400 is regular, 600 semibold, 700 bold. The tab bars use it too.",
                    "font_weight",
                ),
                &mut c.bar_font_weight,
                d.bar_font_weight,
                100..=900,
                "",
            );
            num(
                ui,
                f,
                ("Outline", "Round a floating bar, each island or each pill; a strip gets a line along its inner edge. 0 = none.", "border_width"),
                &mut c.bar_border_width,
                d.bar_border_width,
                0..=10,
                " px",
            );
            num(
                ui,
                f,
                ("Space between widgets", "", "widget_gap"),
                &mut c.bar_widget_gap,
                d.bar_widget_gap,
                0..=48,
                " px",
            );
            num(
                ui,
                f,
                (
                    "Workspace mark width",
                    "With labels other than Automatic, or app icons, a mark grows to fit them.",
                    "workspace_width",
                ),
                &mut c.bar_workspace_width,
                d.bar_workspace_width,
                18..=100,
                " px",
            );
        });
        if let Some(p) = apply {
            self.apply_preset(p);
        }
    }

    fn launcher(&mut self, ui: &mut Ui) {
        let label = format!("App launcher ({}Space)", mk(&self.cfg));
        let (c, d, f) = (&mut self.cfg, &self.def, &mut self.find);
        let (m, dm) = (&mut self.mir, &self.def_mir);
        toggle(
            ui,
            f,
            (
                label.as_str(),
                "A search box for apps, files, calculations and the web.",
                "launcher_enabled",
            ),
            &mut c.launcher_enabled,
            d.launcher_enabled,
        );
        section(ui, f, "Sources");
        toggle(
            ui,
            f,
            (
                "Installed apps",
                "Start menu shortcuts.",
                "launcher_source_apps",
            ),
            &mut c.launcher_source_apps,
            d.launcher_source_apps,
        );
        toggle(
            ui,
            f,
            (
                "Files",
                "Results from the Windows Search index.",
                "launcher_source_files",
            ),
            &mut c.launcher_source_files,
            d.launcher_source_files,
        );
        when(ui, f, c.launcher_source_files, |ui, f| {
            choice_row(
                ui,
                f,
                (
                    "Search in",
                    "Where file results come from.",
                    "launcher_file_scope",
                ),
                (&mut c.launcher_file_scope, &d.launcher_file_scope),
                &[
                    Choice::new("Your user folder", ""),
                    Choice::new("Everything Windows indexes", "*"),
                ],
                Some(Pick::Folder),
                "launcher",
            );
            excludes_row(ui, f, &mut m.launcher_excludes, &dm.launcher_excludes);
        });
        toggle(
            ui,
            f,
            (
                "Calculator",
                "Type a sum to see the result.",
                "launcher_source_calc",
            ),
            &mut c.launcher_source_calc,
            d.launcher_source_calc,
        );
        toggle(
            ui,
            f,
            (
                "Web search",
                "Offers a web search for what you typed.",
                "launcher_source_web",
            ),
            &mut c.launcher_source_web,
            d.launcher_source_web,
        );
        when(ui, f, c.launcher_source_web, |ui, f| {
            choice_row(
                ui,
                f,
                (
                    "Search with",
                    "The site that answers a web search.",
                    "launcher_web_url",
                ),
                (&mut c.launcher_web_url, &d.launcher_web_url),
                &SEARCH_ENGINES.map(|(name, url)| Choice::new(name, url)),
                None,
                "launcher",
            );
        });
        toggle(
            ui,
            f,
            (
                "Open windows",
                "Switch to a window by its title.",
                "launcher_source_windows",
            ),
            &mut c.launcher_source_windows,
            d.launcher_source_windows,
        );
        // One switch per feature: the WM lists clipboard history only with
        // both keys on, and capturing with the source off is pointless (the
        // same holds for emoji). Turning the source on turns the feature on.
        let mut clip = c.launcher_source_clipboard && c.clipboard_history;
        if toggle(
            ui,
            f,
            ("Clipboard history", "Keeps recently copied text in memory and lists it in the launcher; start your search with the prefix below.", "launcher_source_clipboard, clipboard_history"),
            &mut clip,
            d.launcher_source_clipboard && d.clipboard_history,
        ) {
            c.launcher_source_clipboard = clip;
            c.clipboard_history = clip;
        }
        when(ui, f, clip, |ui, f| {
            num(
                ui,
                f,
                (
                    "History size",
                    "How many copied items it keeps.",
                    "clipboard_limit",
                ),
                &mut c.clipboard_limit,
                d.clipboard_limit,
                1..=200,
                "",
            );
            text(
                ui,
                f,
                (
                    "Clipboard prefix",
                    "Type this first in the launcher to search what you copied, e.g. > invoice.",
                    "clipboard_prefix",
                ),
                &mut c.clipboard_prefix,
                &d.clipboard_prefix,
            );
        });
        let mut emoji = c.launcher_source_emoji && c.emoji_picker;
        if toggle(
            ui,
            f,
            (
                "Emoji",
                "Search emoji by name; start with the prefix below.",
                "launcher_source_emoji, emoji_picker",
            ),
            &mut emoji,
            d.launcher_source_emoji && d.emoji_picker,
        ) {
            c.launcher_source_emoji = emoji;
            c.emoji_picker = emoji;
        }
        when(ui, f, emoji, |ui, f| {
            text(
                ui,
                f,
                (
                    "Emoji prefix",
                    "Type this first in the launcher to find an emoji, e.g. :smile.",
                    "emoji_prefix",
                ),
                &mut c.emoji_prefix,
                &d.emoji_prefix,
            );
        });
        section(ui, f, "Custom entries");
        if table(
            ui,
            f,
            ("Your own entries", "Your own results in the launcher: an app, file, folder, web page or command, found by typing its name. Icon: auto uses the item's own; the menu beside it has more.", "launcher_entries"),
            &mut m.rows.entries,
            &dm.rows.entries,
            (
                "+ Add entry",
                EntryRow {
                    label: String::new(),
                    kind: Kind::App,
                    value: String::new(),
                    icon: "auto".into(),
                },
            ),
            |ui, _, e| {
                cell(ui, &mut e.label, "Label", 140.0);
                target_ui(ui, &mut e.kind, &mut e.value);
                icon_ui(ui, &mut e.icon, true);
                entry_check(e).err()
            },
        ) {
            fold_entries(&m.rows.entries, c);
        }
        more(ui, f, "launcher", |ui, f| {
            text(
                ui,
                f,
                ("Search address", "For a site not in the list: its search address, with {query} where the words go, e.g. https://example.com/search?q={query}.", "launcher_web_url"),
                &mut c.launcher_web_url,
                &d.launcher_web_url,
            );
            path_row(
                ui,
                f,
                (
                    "Search in (folder)",
                    "The folder file results come from. Empty: your user folder; *: everything Windows indexes.",
                    "launcher_file_scope",
                ),
                (&mut c.launcher_file_scope, &d.launcher_file_scope),
                Pick::Folder,
                "Empty: your user folder",
            );
            pick(
                ui,
                f,
                (
                    "Opens on",
                    "Which monitor the launcher appears on.",
                    "launcher_placement",
                ),
                &mut c.launcher_placement,
                &d.launcher_placement,
                &[
                    ("cursor_monitor", "The monitor under the mouse"),
                    ("focused_monitor", "The monitor with the active window"),
                    ("primary_monitor", "The main monitor"),
                ],
            );
            num(
                ui,
                f,
                (
                    "Maximum results",
                    "How many results a search lists at most.",
                    "launcher_max_results",
                ),
                &mut c.launcher_max_results,
                d.launcher_max_results,
                5..=200,
                "",
            );
            toggle(
                ui,
                f,
                (
                    "Boost recently used",
                    "What you open often rises to the top.",
                    "launcher_mru",
                ),
                &mut c.launcher_mru,
                d.launcher_mru,
            );
            section(ui, f, "Size");
            for (meta, v, dv, range) in [
                (
                    ("Width", "", "launcher_width"),
                    &mut c.launcher_width,
                    d.launcher_width,
                    320..=1600,
                ),
                (
                    (
                        "Wide width",
                        "The width when results need more room (file paths, for one).",
                        "launcher_wide_width",
                    ),
                    &mut c.launcher_wide_width,
                    d.launcher_wide_width,
                    480..=2400,
                ),
                (
                    ("Height", "", "launcher_height"),
                    &mut c.launcher_height,
                    d.launcher_height,
                    200..=1200,
                ),
                (
                    ("Row height", "", "launcher_row_height"),
                    &mut c.launcher_row_height,
                    d.launcher_row_height,
                    24..=72,
                ),
                (
                    ("Icon size", "", "launcher_icon_size"),
                    &mut c.launcher_icon_size,
                    d.launcher_icon_size,
                    12..=64,
                ),
                (
                    ("Padding", "", "launcher_padding"),
                    &mut c.launcher_padding,
                    d.launcher_padding,
                    4..=48,
                ),
                (
                    ("Selection corner radius", "", "launcher_selection_radius"),
                    &mut c.launcher_selection_radius,
                    d.launcher_selection_radius,
                    0..=40,
                ),
            ] {
                num(ui, f, meta, v, dv, range, " px");
            }
        });
    }

    fn power(&mut self, ui: &mut Ui) {
        let label = format!("Power menu ({}Shift+Space)", mk(&self.cfg));
        let (c, d, f) = (&mut self.cfg, &self.def, &mut self.find);
        let (m, dm) = (&mut self.mir, &self.def_mir);
        toggle(
            ui,
            f,
            (
                label.as_str(),
                "Lock, sleep, restart and Astur's own actions in one menu.",
                "system_menu_enabled",
            ),
            &mut c.system_menu_enabled,
            d.system_menu_enabled,
        );
        section(ui, f, "Items");
        for (meta, items, def, field) in [
            (
                ("Power", "Ticked items show in the menu's Power section, in this order.", "system_power_items"),
                &mut m.rows.power,
                &dm.rows.power,
                &mut c.system_power_items,
            ),
            (
                ("Astur", "Ticked items show in the menu's Astur section, in this order. Wallpaper lists the images in the wallpaper folder (Desktop extras).", "system_setup_items"),
                &mut m.rows.setup,
                &dm.rows.setup,
                &mut c.system_setup_items,
            ),
        ] {
            if checklist_row(ui, f, meta, items, def) {
                fold_items(items, field);
            }
        }
        if table(
            ui,
            f,
            ("Your own actions", "Listed under their group (Custom when empty). Ask first shows a confirmation before it runs.", "system_actions"),
            &mut m.rows.actions,
            &dm.rows.actions,
            (
                "+ Add action",
                ActionRow {
                    group: String::new(),
                    label: String::new(),
                    kind: Kind::Command,
                    value: String::new(),
                    icon: "command".into(),
                    confirm: false,
                },
            ),
            |ui, _, a| {
                cell(ui, &mut a.group, "Group", 90.0)
                    .on_hover_text("The menu section it is listed under");
                cell(ui, &mut a.label, "Label", 130.0);
                target_ui(ui, &mut a.kind, &mut a.value);
                ui.end_row();
                icon_ui(ui, &mut a.icon, false);
                switch(ui, &mut a.confirm, "Ask first");
                action_check(a).err()
            },
        ) {
            fold_actions(&m.rows.actions, c);
        }
        more(ui, f, "power", |ui, f| {
            num(
                ui,
                f,
                ("Menu width", "", "system_menu_width"),
                &mut c.system_menu_width,
                d.system_menu_width,
                280..=900,
                " px",
            );
        });
    }

    fn extras(&mut self, ui: &mut Ui) {
        let c = &self.cfg;
        let key = (c.extra_hotkeys.iter())
            .find(|h| h.action == "scratchpad")
            .and_then(|h| parse_chord(&h.chord, c.main_key))
            .map_or_else(|| "its shortcut".into(), |(m, vk)| chord_label(m, vk));
        let desc = format!("An app (a terminal by default) that drops down over everything with {key} and hides again with the same key. Change the key under Shortcuts > Extra hotkeys.");
        let (c, d, f) = (&mut self.cfg, &self.def, &mut self.find);
        section(ui, f, "Scratchpad");
        toggle(
            ui,
            f,
            ("Scratchpad", desc.as_str(), "scratchpad_enabled"),
            &mut c.scratchpad_enabled,
            d.scratchpad_enabled,
        );
        when(ui, f, c.scratchpad_enabled, |ui, f| {
            let apps = APPS.get().map_or(&[][..], |a| a.terminals.as_slice());
            if choice_row(
                ui,
                f,
                ("App", "The app that drops down.", "scratchpad_command"),
                (&mut c.scratchpad_command, &d.scratchpad_command),
                apps,
                Some(Pick::App),
                "extras",
            ) {
                c.scratchpad_class = scratch_class(&c.scratchpad_command).to_string();
            }
            scratch_window_row(ui, f, c, d);
        });
        section(ui, f, "Wallpapers");
        path_row(
            ui,
            f,
            (
                "Wallpaper folder",
                "Where the power menu's Wallpaper item finds pictures to choose from.",
                "wallpaper_dir",
            ),
            (&mut c.wallpaper_dir, &d.wallpaper_dir),
            Pick::Folder,
            r"Empty: .astur\wallpapers in your user folder",
        );
        section(ui, f, "Memory");
        toggle(
            ui,
            f,
            ("Remember workspaces and launcher history", "After a restart, each monitor returns to its workspace and the launcher keeps its recent items.", "persist_state"),
            &mut c.persist_state,
            d.persist_state,
        );
        more(ui, f, "extras", |ui, f| {
            text(
                ui,
                f,
                ("Scratchpad command", "What starts the scratchpad, typed out: for an app with options, e.g. wt -p Ubuntu.", "scratchpad_command"),
                &mut c.scratchpad_command,
                &d.scratchpad_command,
            );
            text(
                ui,
                f,
                ("Scratchpad window class", "The window's class name, or a pattern with * and ?. Empty: the first window that opens after the command.", "scratchpad_class"),
                &mut c.scratchpad_class,
                &d.scratchpad_class,
            );
        });
    }

    fn integrations(&mut self, ui: &mut Ui) {
        let (c, d, f) = (&mut self.cfg, &self.def, &mut self.find);
        let mut go_bar = false;
        section(ui, f, "YASB");
        toggle(
            ui,
            f,
            ("YASB workspaces", "Show Astur workspaces in YASB, with live indicators, app icons and click or wheel switching.", "yasb_enabled"),
            &mut c.yasb_enabled,
            d.yasb_enabled,
        );
        when(ui, f, c.yasb_enabled, |ui, f| {
            row(
                ui,
                f,
                (
                    "Port",
                    "Default 6123. Choose a free port and use the same one in YASB.",
                    "yasb_port",
                ),
                &mut c.yasb_port,
                &d.yasb_port,
                |ui, v| {
                    ui.add(
                        egui::DragValue::new(v)
                            .range(1..=65535)
                            .clamp_existing_to_range(false),
                    );
                },
            );
        });
        if !f.on() {
            let endpoint = format!("ws://127.0.0.1:{}", c.yasb_port);
            let yaml = YASB_WIDGET_YAML.replace("ws://127.0.0.1:6123", &endpoint);
            card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("YASB server address:");
                    ui.monospace(&endpoint);
                    if ui.button("Copy address").clicked() {
                        ui.ctx().copy_text(endpoint.clone());
                    }
                });
                ui.add_space(8.0);
                ui.label(RichText::new("Set up YASB").family(semibold()));
                ui.label("1. Turn on YASB workspaces above.");
                ui.label("2. Merge the widget YAML into YASB's config.yaml. If a widgets section already exists, add astur_workspaces inside it.");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Copy widget YAML").clicked() {
                        ui.ctx().copy_text(yaml.clone());
                    }
                    if ui.button("Copy example CSS").clicked() {
                        ui.ctx().copy_text(YASB_WIDGET_CSS.to_string());
                    }
                });
                ui.label(RichText::new("The copied YAML uses the port shown above. Append the CSS to styles.css, or keep your existing GlazeWM workspace styles.").color(sub(ui)));
                egui::CollapsingHeader::new("Preview widget YAML")
                    .id_salt("yasb-yaml")
                    .show(ui, |ui| {
                        egui::ScrollArea::horizontal().show(ui, |ui| ui.code(&yaml));
                    });
                ui.label("3. Add astur_workspaces to your bar's widgets.left list. In that bar's window_flags, set windows_app_bar: true to reserve space for YASB.");
                ui.horizontal_wrapped(|ui| {
                    ui.label("4. Reload YASB, then turn off Astur's own bar on the");
                    go_bar = ui.link("Status bar").clicked();
                    ui.label("page.");
                });
                ui.add_space(4.0);
                ui.label(RichText::new("Uses YASB's GlazeWM workspace widget. GlazeWM itself must be stopped. Binding-mode and tiling-direction widgets are not supported.").color(sub(ui)));
            });
        }
        section(ui, f, "Scripts");
        toggle(
            ui,
            f,
            ("Let scripts control Astur", "Scripts and tools on this PC can switch workspaces, focus windows and more (a named pipe). Only your user account can connect; nothing listens on the network.", "ipc_enabled"),
            &mut c.ipc_enabled,
            d.ipc_enabled,
        );
        when(ui, f, c.ipc_enabled, |ui, f| {
            text(
                ui,
                f,
                (
                    "Connection name",
                    "Scripts connect to \\\\.\\pipe\\ and this name. Change it only if a script asks for another.",
                    "ipc_pipe",
                ),
                &mut c.ipc_pipe,
                &d.ipc_pipe,
            );
            toggle(
                ui,
                f,
                ("Allow the launch command", "Security: with this on, any program running as you can use Astur to start any command. Leave it off unless your scripts launch apps through Astur.", "ipc_allow_launch"),
                &mut c.ipc_allow_launch,
                d.ipc_allow_launch,
            );
        });
        if go_bar {
            self.page = Page::Bar;
        }
    }

    fn troubleshooting(&mut self, ui: &mut Ui) {
        let mut fix: Option<(String, Option<&'static str>)> = None;
        let mut reload = false;
        if !self.find.on() {
            let writable = self.read_error.is_none();
            section_title(ui, "Config health");
            if self.disk.unknown_keys.is_empty() {
                card(ui, |ui| {
                    ui.label("Every line in both config files is understood.");
                });
            }
            // The files' text, to say what a rename replaces. Read only while
            // a stray line has a suggestion (unknown keys are rare).
            let texts = if self
                .disk
                .unknown_keys
                .iter()
                .any(|e| did_you_mean(e.split_once(": ").map_or(e.as_str(), |e| e.1)).is_some())
            {
                self.paths
                    .each_ref()
                    .map(|p| astur_config::read_existing(p).ok().flatten())
            } else {
                [None, None]
            };
            // Unknown keys used to be ignored in total silence, which for a
            // 150-key config across two files makes a typo undetectable.
            for entry in &self.disk.unknown_keys {
                let (file, key) = entry.split_once(": ").unwrap_or(("", entry.as_str()));
                let hint = did_you_mean(key);
                let text = texts[usize::from(file == "navbar.conf")].as_deref();
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(entry.replacen(": ", "  \u{00B7}  ", 1)).monospace(),
                            );
                            let why = match hint {
                                Some(h) => {
                                    format!("Not understood, so it does nothing. Did you mean {h}?")
                                }
                                None => "Not understood, so it does nothing.".to_string(),
                            };
                            ui.label(RichText::new(why).small().color(sub(ui)));
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add_enabled_ui(writable, |ui| {
                                if ui.button("Remove line").clicked() {
                                    fix = Some((entry.clone(), None));
                                }
                                if let Some(h) = hint {
                                    // A working `h` line is replaced, not kept:
                                    // say so, with both values.
                                    let old = text.and_then(|t| line_value(t, h));
                                    let new = text.and_then(|t| line_value(t, key));
                                    let label = match (old, new) {
                                        (Some(o), Some(n)) => format!("Replace {h} = {o} with {n}"),
                                        _ => format!("Rename to {h}"),
                                    };
                                    if ui.button(label).clicked() {
                                        fix = Some((entry.clone(), Some(h)));
                                    }
                                }
                            });
                        });
                    });
                });
            }
            section_title(ui, "Files");
            for path in &self.paths {
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(path.display().to_string()).monospace());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Open in editor").clicked() {
                                let _ = std::process::Command::new("notepad.exe").arg(path).spawn();
                            }
                        });
                    });
                });
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Open folder").clicked() {
                    if let Some(dir) = self.paths[0].parent() {
                        let _ = std::process::Command::new("explorer").arg(dir).spawn();
                    }
                }
                reload = ui
                    .button("Reload from disk")
                    .on_hover_text("Take the files as they are now (after editing them by hand)")
                    .clicked();
            });
        }
        let writable = self.read_error.is_none();
        let (c, d, f) = (&mut self.cfg, &self.def, &mut self.find);
        let mut diagnostics = |ui: &mut Ui| {
            section(ui, f, "Diagnostics");
            seg_row(
                ui,
                f,
                ("Log level", "What Astur writes to its log. Error: failures only. Info: also startup, monitors and reloads. Debug: everything, for reporting a bug. Running astur.exe --check prints a paste-ready report.", "log_level"),
                &mut c.log_level,
                &d.log_level,
                &[("off", "Off"), ("error", "Error"), ("info", "Info"), ("debug", "Debug")],
            );
        };
        // Read-only writes nothing, so the level can't change either. A
        // scope only then (page_ui: an empty one spaces out search hits).
        if writable {
            diagnostics(ui);
        } else {
            ui.add_enabled_ui(false, diagnostics);
        }
        if !f.on() && ui.button("Open log").clicked() {
            let log = config_path("ASTUR_LOG", "astur.log");
            let _ = std::process::Command::new("notepad.exe").arg(log).spawn();
        }
        if let Some((entry, to)) = fix {
            self.fix_line(&entry, to);
        }
        if reload {
            self.reload();
        }
    }

    fn about(&mut self, ui: &mut Ui) {
        if self.find.on() {
            return;
        }
        card(ui, |ui| {
            ui.label(
                RichText::new(format!("Astur Settings {}", env!("CARGO_PKG_VERSION")))
                    .family(semibold()),
            );
            ui.label("Edits astur.conf and navbar.conf. Changes apply as you make them and Astur reloads them live; Undo (Ctrl+Z) takes them back. Comments and hand edits in the files are kept.");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.hyperlink_to("astur.app", "https://astur.app");
                ui.hyperlink_to("GitHub", "https://github.com/Page011/Astur");
            });
        });
    }
}

// ---- rows --------------------------------------------------------------------

/// (label, one plain-language sentence, conf key(s)) for a setting row.
type Meta<'a> = (&'a str, &'a str, &'a str);

/// The 12 single-key bindings, in `Mirrors::keys` order.
const BINDS: [Meta<'static>; 12] = [
    ("Focus next window", "With Shift too it swaps the window forward instead.", "key_focus_next"),
    ("Focus previous window", "With Shift too it swaps the window backward.", "key_focus_prev"),
    ("Make the main window smaller", "Main + side: a narrower main window. Spiral: a smaller first split. Scrolling: a narrower column (with Shift: a shorter window).", "key_shrink_master"),
    ("Make the main window bigger", "The opposite, in every layout.", "key_grow_master"),
    ("Make it the main window", "Moves the active window into the big spot. Scrolling: makes its column full width, or back.", "key_promote_master"),
    ("Turn tiling on or off", "Off leaves every window floating, as on plain Windows; workspaces keep working.", "key_toggle_tiling"),
    ("Float or tile the window", "Lifts the active window out of the tiling so you can place it yourself, or puts it back.", "key_toggle_float"),
    ("Close the window", "", "key_close_window"),
    ("Cycle column width", "Scrolling: steps the column through its widths. With Shift: the window's height.", "key_cycle_width"),
    ("Centre the column", "Scrolling: puts the active column in the middle of the screen.", "key_center_column"),
    ("Pull into the left column", "Scrolling: stacks the window into the column on its left. If it already shares a column, it gets its own instead.", "key_consume_left"),
    ("Pull into the right column", "Scrolling: stacks the window into the column on its right. If it already shares a column, it gets its own instead.", "key_consume_right"),
];

/// What each of the 12 binds runs, alone and with Shift (compile_keys'
/// built-ins), to find its notes among compile_keys' issues.
const BIND_ACTIONS: [[Action; 2]; 12] = {
    use Action::*;
    [
        [FocusNext, SwapNext],
        [FocusPrev, SwapPrev],
        [ShrinkMaster, ShrinkTile],
        [GrowMaster, GrowTile],
        [PromoteMaster, PromoteMaster],
        [ToggleTiling, ToggleTabbed],
        [ToggleFloat, ToggleFloat],
        [CloseWindow, CloseWindow],
        [CycleWidth, CycleHeight],
        [CenterColumn, CenterColumn],
        [ConsumeLeft, ConsumeLeft],
        [ConsumeRight, ConsumeRight],
    ]
};

/// One setting as a Windows 11 settings card: label and sentence on the left,
/// the control on the right (below it when `wide`), a reset arrow while the
/// value differs from the default, and the conf key on hover, so the GUI, the
/// README and the files read as one thing. Hidden when search does not match.
/// Returns true when reset was clicked.
fn row_core(
    ui: &mut Ui,
    f: &mut Find,
    meta: Meta,
    modified: bool,
    wide: bool,
    add: impl FnOnce(&mut Ui),
) -> bool {
    row_noted(ui, f, meta, modified, wide, &[], add)
}

/// `row_core` with `notes` under the sentence: compile_keys' word on a
/// shortcut can be long, and the control column is the narrow one.
fn row_noted(
    ui: &mut Ui,
    f: &mut Find,
    (label, desc, key): Meta,
    modified: bool,
    wide: bool,
    notes: &[(Level, String)],
    add: impl FnOnce(&mut Ui),
) -> bool {
    if !f.matches(&[label, desc, key]) {
        return false;
    }
    f.hit(ui);
    #[cfg(test)]
    f.seen.extend(key.split(", ").map(String::from));
    let mut reset = false;
    let mut add = Some(add);
    card(ui, |ui| {
        let w = ui.available_width();
        let ctrl = if wide {
            40.0
        } else {
            (w * 0.45).clamp(150.0, 360.0)
        };
        let gap = ui.spacing().item_spacing.x;
        // Top-aligned: a centred row would centre the (still empty) text
        // column in the row's first 28 px and push the label down.
        ui.horizontal_top(|ui| {
            let text = ui
                .allocate_ui_with_layout(
                    egui::vec2(w - ctrl - gap, 0.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_width(w - ctrl - gap);
                        if desc.is_empty() {
                            // Centre a lone label on the control row.
                            let line = ui.text_style_height(&egui::TextStyle::Body);
                            ui.add_space(((RESET_SLOT - line) / 2.0).max(0.0));
                        }
                        ui.label(label);
                        if !desc.is_empty() {
                            ui.label(RichText::new(desc).small().color(sub(ui)));
                        }
                        note_labels(ui, notes);
                    },
                )
                .response;
            if !key.is_empty() {
                text.on_hover_text(key_files(key));
            }
            let h = ui.min_rect().height().max(RESET_SLOT);
            ui.allocate_ui_with_layout(
                egui::vec2(ctrl, h),
                Layout::right_to_left(Align::Center),
                |ui| {
                    reset = reset_button(ui, modified);
                    if !wide {
                        if let Some(add) = add.take() {
                            add(ui);
                        }
                    }
                },
            );
        });
        if let Some(add) = add.take() {
            ui.add_space(2.0);
            add(ui);
        }
    });
    reset
}

fn row<T: PartialEq + Clone>(
    ui: &mut Ui,
    f: &mut Find,
    meta: Meta,
    v: &mut T,
    d: &T,
    add: impl FnOnce(&mut Ui, &mut T),
) {
    if row_core(ui, f, meta, v != d, false, |ui| add(ui, v)) {
        *v = d.clone();
    }
}

/// An on/off row. Returns true when the value changed (click or reset).
fn toggle(ui: &mut Ui, f: &mut Find, meta: Meta, v: &mut bool, d: bool) -> bool {
    let before = *v;
    row(ui, f, meta, v, &d, |ui, v| {
        toggle_switch(ui, v);
    });
    *v != before
}

fn text(ui: &mut Ui, f: &mut Find, meta: Meta, v: &mut String, d: &str) {
    if row_core(ui, f, meta, v != d, false, |ui| {
        ui.add(
            egui::TextEdit::singleline(v)
                .desired_width(ui.available_width())
                .margin(FIELD_MARGIN),
        );
        one_line(v);
    }) {
        *v = d.to_string();
    }
}

fn num<N: egui::emath::Numeric>(
    ui: &mut Ui,
    f: &mut Find,
    meta: Meta,
    v: &mut N,
    d: N,
    range: std::ops::RangeInclusive<N>,
    suffix: &str,
) {
    row(ui, f, meta, v, &d, |ui, v| {
        ui.horizontal(|ui| {
            ui.spacing_mut().slider_width = (ui.available_width() - 84.0).max(60.0);
            ui.add(slider(v, range).suffix(suffix));
        });
    });
}

/// A 0..1 fraction shown and typed as a percentage.
fn pct(
    ui: &mut Ui,
    f: &mut Find,
    meta: Meta,
    v: &mut f32,
    d: f32,
    range: std::ops::RangeInclusive<f32>,
) {
    row(ui, f, meta, v, &d, |ui, v| {
        ui.horizontal(|ui| {
            ui.spacing_mut().slider_width = (ui.available_width() - 84.0).max(60.0);
            ui.add(
                slider(v, range)
                    .custom_formatter(|n, _| format!("{:.0}%", n * 100.0))
                    .custom_parser(|s| {
                        let n = s.trim().trim_end_matches('%').trim().parse::<f64>().ok()?;
                        Some(n / 100.0)
                    }),
            );
        });
    });
}

/// A drop-down row. A value not among `opts` shows as itself.
fn pick(ui: &mut Ui, f: &mut Find, meta: Meta, v: &mut String, d: &str, opts: &[(&str, &str)]) {
    // Explicit id: search draws several pages at once, and a label-derived
    // id would clash where two pages show the same setting.
    let salt = (meta.2, meta.0);
    if row_core(ui, f, meta, v != d, false, |ui| {
        let w = ui.available_width().min(300.0);
        combo(ui, salt, v, opts, w);
    }) {
        *v = d.to_string();
    }
}

/// A drop-down of (value, label) options; a value not among them shows as
/// itself. The id is relative to the enclosing ui.
fn combo(ui: &mut Ui, salt: impl std::hash::Hash, v: &mut String, opts: &[(&str, &str)], w: f32) {
    let shown = opts
        .iter()
        .find(|o| o.0 == v.as_str())
        .map_or(v.as_str(), |o| o.1)
        .to_string();
    egui::ComboBox::from_id_salt(salt)
        .selected_text(shown)
        .width(w)
        .show_ui(ui, |ui| {
            for &(o, l) in opts {
                ui.selectable_value(v, o.to_string(), l);
            }
        });
}

fn seg_row(ui: &mut Ui, f: &mut Find, meta: Meta, v: &mut String, d: &str, opts: &[(&str, &str)]) {
    if row_core(ui, f, meta, v != d, false, |ui| seg(ui, v, opts)) {
        *v = d.to_string();
    }
}

fn theme_row(ui: &mut Ui, f: &mut Find, v: &mut String, d: &str) {
    seg_row(
        ui,
        f,
        ("Colour theme", "For Astur's launcher, menus and bar, and this window. A colour you set yourself still wins.", "theme"),
        v,
        d,
        &[("dark", "Dark"), ("light", "Light"), ("auto", "System")],
    );
}

fn layout_row(ui: &mut Ui, f: &mut Find, v: &mut String, d: &str) {
    let meta = (
        "Default layout",
        "How windows share the screen. Each monitor can have its own on the Layout page.",
        "layout",
    );
    if row_core(ui, f, meta, v != d, true, |ui| layout_picker(ui, v)) {
        *v = d.to_string();
    }
}

/// A drop-down of ready-made values (`opts`), then, with `browse`, "Choose
/// an app…" (or a folder: the native dialog), and "Custom…", which opens the
/// page's More options (`salt`), where the value can be typed. A value
/// matching no option shows as itself and is never rewritten, so nothing set
/// by hand is lost. Returns true when the value changed (picked or reset).
fn choice_row(
    ui: &mut Ui,
    f: &mut Find,
    meta: Meta,
    (v, d): (&mut String, &str),
    opts: &[Choice],
    browse: Option<Pick>,
    salt: &str,
) -> bool {
    let before = v.clone();
    let id = egui::Id::new(("choice", meta.2, meta.0));
    if let Some(p) = take_pick(id) {
        *v = p;
    }
    if row_core(ui, f, meta, v != d, false, |ui| {
        egui::ComboBox::from_id_salt(id)
            .selected_text(choice_label(v, opts, browse))
            .width(ui.available_width().min(300.0))
            .height(400.0)
            .truncate()
            .show_ui(ui, |ui| {
                for o in opts {
                    if ui
                        .selectable_label(same_app(&o.command, v), &o.label)
                        .clicked()
                    {
                        v.clone_from(&o.command);
                    }
                }
                if !opts.is_empty() {
                    ui.separator();
                }
                if let Some(kind) = browse {
                    if ui.selectable_label(false, kind.choose()).clicked() {
                        start_pick(ui.ctx(), id, kind, v);
                    }
                }
                if ui
                    .selectable_label(false, "Custom\u{2026}")
                    .on_hover_text("Type it yourself, under More options")
                    .clicked()
                {
                    open_more(ui.ctx(), salt);
                }
            });
    }) {
        *v = d.to_string();
    }
    *v != before
}

/// A folder or file path: typed, or picked with Browse. `hint` says what
/// empty means.
fn path_row(
    ui: &mut Ui,
    f: &mut Find,
    meta: Meta,
    (v, d): (&mut String, &str),
    kind: Pick,
    hint: &str,
) {
    let id = egui::Id::new(("path", meta.2));
    if row_core(ui, f, meta, v != d, false, |ui| {
        browse_button(ui, id, v, kind);
        ui.add(
            egui::TextEdit::singleline(v)
                .hint_text(hint)
                .desired_width(ui.available_width())
                .margin(FIELD_MARGIN),
        );
        one_line(v);
    }) {
        *v = d.to_string();
    }
}

/// What file search skips: bits of a path, comma-separated, and a button
/// that adds a whole folder (any path containing it is skipped).
fn excludes_row(ui: &mut Ui, f: &mut Find, v: &mut String, d: &str) {
    let meta = ("Skip folders", "File results never come from a path containing one of these, e.g. node_modules. Separate them with commas.", "launcher_file_exclude");
    let id = egui::Id::new("exclude-folder");
    let refused_id = id.with("refused");
    if let Some(p) = take_pick(id) {
        if add_folder(v, &p) {
            ui.data_mut(|m| m.remove::<String>(refused_id));
        } else {
            ui.data_mut(|m| m.insert_temp(refused_id, p));
        }
    }
    let notes: Vec<(Level, String)> = (ui.data(|m| m.get_temp::<String>(refused_id)))
        .map(|p| {
            let tail = p.rsplit([',', ';']).next().unwrap_or(&p).trim();
            let text = format!("Not added: {p} has a , or ; and this list is split at those. Type the end of its path instead, e.g. {tail}");
            (Level::Warn, text)
        })
        .into_iter()
        .collect();
    if row_noted(ui, f, meta, v != d, false, &notes, |ui| {
        if ui.button("Add a folder\u{2026}").clicked() {
            start_pick(ui.ctx(), id, Pick::Folder, "");
        }
        let edit = ui.add(
            egui::TextEdit::singleline(v)
                .desired_width(ui.available_width())
                .margin(FIELD_MARGIN),
        );
        one_line(v);
        if edit.changed() {
            ui.data_mut(|m| m.remove::<String>(refused_id));
        }
    }) {
        *v = d.to_string();
    }
}

/// Add folder `p` to the skip list `v`; false (`v` unchanged) when it has a
/// , or ;. The list is split at those with no escape, so `...\OneDrive -
/// Contoso, Inc\Archive` would go in as two pieces, the first a parent
/// folder that hides the whole drive from file search.
fn add_folder(v: &mut String, p: &str) -> bool {
    if p.contains([',', ';']) {
        return false;
    }
    if !v.trim().is_empty() {
        v.push_str(", ");
    }
    v.push_str(p);
    true
}

/// Shares of the screen as percentage boxes, each with a remove button (the
/// last one stays: an empty list reads back as the built-in one), then Add.
/// `range` is the parser's clamp, so what is typed reads back. A box writes
/// only when edited: 0.333 shows as 33% and stays 0.333.
fn shares_row(
    ui: &mut Ui,
    f: &mut Find,
    meta: Meta,
    (v, d): (&mut Vec<f32>, &[f32]),
    range: std::ops::RangeInclusive<i32>,
) {
    if row_core(ui, f, meta, v.as_slice() != d, true, |ui| {
        ui.horizontal_wrapped(|ui| {
            // Each box tight against its remove button, pairs apart.
            ui.spacing_mut().item_spacing.x = 2.0;
            let (n, mut remove) = (v.len(), None);
            for (i, x) in v.iter_mut().enumerate() {
                let mut p = (*x * 100.0).round() as i32;
                let drag = egui::DragValue::new(&mut p)
                    .range(range.clone())
                    .clamp_existing_to_range(false)
                    .suffix("%");
                if ui.add(drag).changed() {
                    *x = p as f32 / 100.0;
                }
                let x_button = egui::Button::new("\u{00D7}").frame(false).small();
                if n > 1 && ui.add(x_button).on_hover_text("Remove").clicked() {
                    remove = Some(i);
                }
                ui.add_space(14.0);
            }
            if let Some(i) = remove {
                v.remove(i);
            }
            if ui.button("+ Add").clicked() {
                v.push(0.5);
            }
        });
    }) {
        *v = d.to_vec();
    }
}

/// "Pick an open window": a menu of the app windows open now (title, then
/// the app's file name). Returns the one clicked.
fn window_picker(ui: &mut Ui) -> Option<OpenWindow> {
    let mut picked = None;
    ui.menu_button("Pick an open window", |ui| {
        let list = window_list(ui.ctx());
        if list.is_empty() {
            ui.label("No app windows are open.");
        }
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .show(ui, |ui| {
                for w in list.iter() {
                    let text = format!("{}  \u{00B7}  {}", clip(&w.title, 48), file_name(&w.exe));
                    if ui.button(text).clicked() {
                        picked = Some(w.clone());
                        ui.close_menu();
                    }
                }
            });
    })
    .response
    .on_hover_text("Choose one of the windows open right now");
    picked
}

/// How Astur knows the scratchpad's window: the first window its app opens,
/// the one its known app always has, or one picked from the windows open now
/// (which sets the app too).
fn scratch_window_row(ui: &mut Ui, f: &mut Find, c: &mut Config, d: &Config) {
    let meta = ("Its window", "How Astur recognises the app's window. If the scratchpad doesn't appear, open the app and pick its window here.", "scratchpad_class");
    let class = c.scratchpad_class.clone();
    let text = if class.is_empty() {
        "The first window it opens"
    } else if class == scratch_class(&c.scratchpad_command) {
        "Found automatically"
    } else {
        "The window you picked"
    };
    let mut picked = None;
    if row_core(ui, f, meta, class != d.scratchpad_class, false, |ui| {
        picked = window_picker(ui);
        ui.label(RichText::new(text).color(sub(ui)))
            .on_hover_text(format!("Window class: {class}"));
    }) {
        c.scratchpad_class.clone_from(&d.scratchpad_class);
    }
    if let Some(w) = picked {
        (c.scratchpad_command, c.scratchpad_class) = scratch_fields(&w, &c.scratchpad_command);
    }
}

/// Home's bar style: a preset by name (`now`: the one the bar is, "" for its
/// own, "off" for no bar), or no bar. Returns the preset to apply, which
/// also turns the bar on.
fn bar_style_row(
    ui: &mut Ui,
    f: &mut Find,
    now: &str,
    bar_on: &mut bool,
) -> Option<&'static BarPreset> {
    let shown = match now {
        "off" => "No bar",
        "" => "Your own style",
        n => BAR_PRESETS
            .iter()
            .find(|p| p.name == n)
            .map_or(n, |p| p.label),
    };
    let mut apply = None;
    let meta = (
        "Status bar style",
        "The bar with your workspaces, the clock and more. The Status bar page has previews, which screens show it and every detail.",
        "enabled, palette, shape",
    );
    if row_core(ui, f, meta, now != BAR_PRESETS[0].name, false, |ui| {
        egui::ComboBox::from_id_salt("home-bar-style")
            .selected_text(shown)
            .width(ui.available_width().min(300.0))
            .show_ui(ui, |ui| {
                for p in BAR_PRESETS {
                    if ui
                        .selectable_label(now == p.name, p.label)
                        .on_hover_text(p.blurb)
                        .clicked()
                    {
                        apply = Some(p);
                    }
                }
                ui.separator();
                if ui.selectable_label(now == "off", "No bar").clicked() {
                    *bar_on = false;
                }
            });
    }) {
        apply = BAR_PRESETS.first();
    }
    apply
}

/// Home's window gaps: one size for the edges and between windows.
fn gaps_row(ui: &mut Ui, f: &mut Find, c: &mut Config, d: &Config) {
    let now = gap_preset(c.outer_gap, c.inner_gap);
    let mut sel = now.to_string();
    let opts: Vec<(&str, &str)> = GAPS.iter().map(|g| (g.0, g.1)).collect();
    let meta = (
        "Space between windows",
        "Gaps around and between tiled windows. Exact sizes are on the Layout page.",
        "outer_gap, inner_gap",
    );
    row(
        ui,
        f,
        meta,
        &mut sel,
        &gap_preset(d.outer_gap, d.inner_gap).to_string(),
        |ui, v| {
            seg(ui, v, &opts);
            if v == "custom" {
                ui.label(RichText::new("Custom").color(sub(ui)));
            }
        },
    );
    if let Some(g) = GAPS.iter().find(|g| g.0 == sel && sel != now) {
        (c.outer_gap, c.inner_gap) = (g.2, g.2);
    }
}

/// Bind `i` of the 12 as a capture chip (and its default), after the Astur
/// key (`held`), which is implied: Left Alt never reaches this window. A
/// cleared key can't be written (the parser would keep the old one), so it
/// stays red until a key is set or reset. `live` is the key Astur uses
/// meanwhile. Returns true when it changed.
fn key_row(
    ui: &mut Ui,
    f: &mut Find,
    meta: Meta,
    (v, d): (&mut String, &str),
    live: u32,
    (held, notes): (&str, &[(Level, String)]),
    i: usize,
) -> bool {
    let mut changed = false;
    if row_noted(ui, f, meta, v != d, false, notes, |ui| {
        changed = key_chip(ui, egui::Id::new(("bind", i)), v, None);
        ui.label(format!("{held} +"));
        if v.is_empty() {
            let text = format!("No key: Astur keeps {}", vk_to_key(live));
            ui.colored_label(ui.visuals().error_fg_color, text);
        }
    }) {
        *v = d.to_string();
        changed = true;
    }
    changed
}

/// The power menu's built-ins as a checklist: ticked ones show, in this
/// order. A name Astur doesn't know stays (red) until it is unticked, which
/// drops it. Returns true when the list changed.
fn checklist_row(
    ui: &mut Ui,
    f: &mut Find,
    meta: Meta,
    items: &mut Vec<(String, bool)>,
    def: &[(String, bool)],
) -> bool {
    let before = items.clone();
    if row_core(ui, f, meta, items.as_slice() != def, true, |ui| {
        let (n, mut edit) = (items.len(), None);
        for (i, it) in items.iter_mut().enumerate() {
            ui.push_id((meta.2, i), |ui| {
                ui.horizontal(|ui| {
                    let label = SYSTEM_ITEMS
                        .iter()
                        .find(|s| s.0 == it.0)
                        .map_or(it.0.as_str(), |s| s.1);
                    // Arrows first, so they line up whatever the label.
                    tools(ui, i, n, false, &mut edit);
                    switch(ui, &mut it.1, label);
                    ui.label(RichText::new(&it.0).small().color(sub(ui)));
                    if let Err(e) = item_check(it) {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                });
            });
        }
        move_row(items, edit);
    }) {
        *items = def.to_vec();
    }
    *items != before
}

/// A list editor as a wide card row: each row framed on its own line (drawn
/// by `each`, which returns why the row can't be written, if it can't) with
/// up / down / delete buttons, then an Add button for `new`. Returns true
/// when the rows changed, for the caller to fold them into the Config.
fn table<T: Clone + PartialEq>(
    ui: &mut Ui,
    f: &mut Find,
    meta: Meta,
    rows: &mut Vec<T>,
    def: &[T],
    new: (&str, T),
    mut each: impl FnMut(&mut Ui, usize, &mut T) -> Option<String>,
) -> bool {
    let before = rows.clone();
    if row_core(ui, f, meta, rows.as_slice() != def, true, |ui| {
        let (n, mut edit, mut bad) = (rows.len(), None, false);
        for (i, row) in rows.iter_mut().enumerate() {
            ui.push_id((meta.2, i), |ui| {
                item(ui, |ui| {
                    let problem = each(ui, i, row);
                    tools(ui, i, n, true, &mut edit);
                    bad |= problem.is_some();
                    problem
                });
            });
        }
        move_row(rows, edit);
        if rows.is_empty() {
            ui.label(RichText::new("None yet.").color(sub(ui)));
        }
        if bad {
            warn_note(ui, "This list is applied once the red rows are fixed; until then Astur keeps its last good version.");
        }
        ui.add_space(4.0);
        if ui.button(new.0).clicked() {
            rows.push(new.1);
        }
    }) {
        *rows = def.to_vec();
    }
    *rows != before
}

/// The workspaces as a table: number, name, icon, key, wallpaper. The count
/// is the number of rows shown; removing the last one keeps its settings in
/// the file for when it comes back. Returns true when anything changed.
fn ws_table(
    ui: &mut Ui,
    f: &mut Find,
    rows: &mut Vec<WsRow>,
    count: &mut usize,
    (def, def_count): (&[WsRow], usize),
    (issues, held): (&[BindIssue], &str),
) -> bool {
    let before = (rows.clone(), *count);
    let problems = ws_problems(rows, *count);
    let meta = ("Names, keys and wallpapers", "Each workspace's name, a symbol the bar shows instead of the name (optional), its key, and a wallpaper Windows switches to with it (optional). When each monitor has its own, every monitor gets this many.", "workspaces, workspace_keys, workspace_names, workspace_icons, workspace_wallpapers");
    let modified = rows.as_slice() != def || *count != def_count;
    if row_core(ui, f, meta, modified, true, |ui| {
        let mut remove = false;
        for i in 0..*count {
            if rows.len() <= i {
                rows.push(WsRow::default());
            }
            let r = &mut rows[i];
            let last = i + 1 == *count && *count > 1;
            ui.push_id(("ws", i), |ui| {
                item(ui, |ui| {
                    ui.add_sized(
                        [22.0, RESET_SLOT],
                        egui::Label::new(RichText::new((i + 1).to_string()).family(semibold())),
                    );
                    cell(ui, &mut r.name, "Name", 140.0);
                    cell(ui, &mut r.icon, "Icon", 64.0).on_hover_text(
                        "Shown on the bar instead of the name (optional): a letter or a symbol",
                    );
                    ui.label(format!("{held} +"));
                    key_chip(ui, egui::Id::new(("ws-key", i)), &mut r.key, None);
                    cell(ui, &mut r.wallpaper, "Wallpaper picture (optional)", 220.0);
                    browse_button(ui, egui::Id::new(("ws-wall", i)), &mut r.wallpaper, Pick::Image);
                    // Only what stops the key: the Windows shortcuts a key
                    // replaces are listed once, on the Shortcuts page.
                    note_labels(ui, &notes(issues, |x| {
                        matches!(x.action, Action::SwitchWorkspace | Action::MoveToWorkspace)
                            && usize::from(x.arg) == i
                            && x.level != Level::Info
                    }));
                    if last {
                        let icons = has_icons(ui);
                        remove = ui
                            .add(egui::Button::new(glyph(icons, ICON_DELETE, "Remove")).frame(false))
                            .on_hover_text("Remove this workspace. Its name, key and wallpaper stay in the file for when you add it back.")
                            .clicked();
                    }
                    problems.get(i).cloned().flatten()
                });
            });
        }
        if remove {
            *count -= 1;
        }
        if problems.iter().any(Option::is_some) {
            warn_note(ui, "A red column is applied once it is fixed; until then Astur keeps its last good version.");
        }
        ui.add_space(4.0);
        if *count < MAX_WORKSPACES && ui.button("+ Add workspace").clicked() {
            *count += 1;
        }
    }) {
        *rows = def.to_vec();
        *count = def_count;
    }
    (rows.as_slice(), *count) != (before.0.as_slice(), before.1)
}

/// The fields of a window rule row: what to do, the app (typed, or from a
/// window open now), then where its windows go. The class and title fields
/// stay under Advanced until one is used: most rules need only the app.
fn rule_ui(ui: &mut Ui, r: &mut RuleRow) -> Option<String> {
    let actions: &[(&str, &str)] = if r.list {
        &[("ignore", "Leave alone"), ("float", "Always float")]
    } else {
        &[
            ("float", "Always float"),
            ("tile", "Always tile"),
            ("ignore", "Leave alone"),
        ]
    };
    combo(ui, "action", &mut r.rule.action, actions, 120.0);
    let wild = "* and ? are wildcards. Empty: any";
    if r.list {
        cell(ui, &mut r.rule.class, "Window class", 200.0)
            .on_hover_text("Class only: * and ? are not read here");
        ui.label(RichText::new("older class list").small().color(sub(ui)))
            .on_hover_text(if r.rule.action == "ignore" {
                "Kept in ignore_classes, which matches by window class alone and is checked before every rule."
            } else {
                "Kept in float_classes, which matches by window class alone and applies only when no rule matches."
            });
        return rule_check(r).err();
    }
    cell(ui, &mut r.rule.exe, "App, e.g. notepad.exe", 170.0).on_hover_text(format!(
        "The app's file name, or its full path. {wild} app."
    ));
    if let Some(w) = window_picker(ui) {
        (r.rule.exe, r.rule.class, r.rule.title) = rule_fields(&w);
    }
    let id = ui.make_persistent_id("advanced");
    let used = !(r.rule.class.is_empty() && r.rule.title.is_empty());
    let mut open = used || ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    if !used
        && ui
            .selectable_label(open, "Advanced")
            .on_hover_text("Match by the window's class or title too")
            .clicked()
    {
        open = !open;
        ui.data_mut(|d| d.insert_temp(id, open));
    }
    // What it matches on one line (two with Advanced), where it goes (and
    // the row's buttons) on the next, rather than wrapping anywhere.
    ui.end_row();
    if open {
        ui.add_space(128.0);
        cell(ui, &mut r.rule.class, "Window class", 170.0)
            .on_hover_text(format!("The window's class name. {wild} class."));
        cell(ui, &mut r.rule.title, "Title contains", 190.0)
            .on_hover_text(format!("Text in the window's title. {wild} title."));
        ui.end_row();
    }
    ui.add_space(128.0);
    place(ui, &mut r.rule.workspace, "workspace", MAX_WORKSPACES)
        .on_hover_text("Open its windows on this workspace");
    place(ui, &mut r.rule.monitor, "monitor", 9)
        .on_hover_text("Open its windows on this monitor (counted from the left)");
    rule_check(r).err()
}

/// A rule's 1-based workspace or monitor; 0, shown "Any ...", is unset.
fn place(ui: &mut Ui, v: &mut Option<usize>, what: &'static str, max: usize) -> egui::Response {
    let mut n = v.map_or(0, |x| x + 1);
    let resp = ui.add(
        egui::DragValue::new(&mut n)
            .range(0..=max)
            .clamp_existing_to_range(false)
            .custom_formatter(move |x, _| {
                if x < 1.0 {
                    format!("Any {what}")
                } else {
                    format!("{}{} {x}", what[..1].to_uppercase(), &what[1..])
                }
            })
            .custom_parser(|s| {
                let digits: String = s.chars().filter(char::is_ascii_digit).collect();
                Some(digits.parse().unwrap_or(0.0))
            }),
    );
    if resp.changed() {
        *v = n.checked_sub(1);
    }
    resp
}

/// The fields of an extra hotkey row: switches for the Astur key (on for a
/// new row), Ctrl and Shift + the captured key, then the action and an
/// argument editor that fits it. An edited chord is written with MOD for
/// the Astur key (`format_chord`), so it moves with a new Astur key.
fn hotkey_ui(
    ui: &mut Ui,
    i: usize,
    h: &mut HotkeyDef,
    (main, issues, ws): (MainKey, &[BindIssue], usize),
    scratch_off: bool,
) -> Option<String> {
    // Part by part as the WM reads it (a lone key name parses as a chord
    // with no modifiers): a chord it can't read still shows the parts it
    // can, so fixing one part does not lose the others.
    let (mut mods, mut key) = (0, String::new());
    for t in h.chord.split('+') {
        match parse_chord(t, main) {
            Some((_, vk)) => key = vk_to_key(vk),
            None => mods |= parse_chord(&format!("{t}+A"), main).map_or(0, |p| p.0),
        }
    }
    let mut edit = false;
    let (astur, had) = (chord_label(main.bits(), 0), mods);
    // Alt or Win besides the Astur key show only while the chord has them
    // (a hand-written one, ALT+GRAVE kept on Alt), so they can be turned
    // off: fewer switches for the usual Astur key + key.
    let switches = [(main.bits(), astur.trim_end_matches('+'))]
        .into_iter()
        .chain(
            [
                (MOD_CTRL, "Ctrl"),
                (MOD_SHIFT, "Shift"),
                (MOD_ALT, "Alt"),
                (MOD_WIN, "Win"),
            ]
            .into_iter()
            .filter(|s| s.0 & main.bits() == 0)
            .filter(|s| s.0 & (MOD_ALT | MOD_WIN) == 0 || had & s.0 != 0),
        );
    for (bits, name) in switches {
        let on = mods & bits == bits;
        if ui.selectable_label(on, name).clicked() {
            mods = if on { mods & !bits } else { mods | bits };
            edit = true;
        }
    }
    let (mut shift, mut ctrl) = (mods & MOD_SHIFT != 0, mods & MOD_CTRL != 0);
    edit |= key_chip(
        ui,
        egui::Id::new(("hotkey", i)),
        &mut key,
        Some((&mut shift, &mut ctrl)),
    );
    if edit {
        // key_chip only turns these on (Shift or Ctrl held with the key).
        for (held, bit) in [(shift, MOD_SHIFT), (ctrl, MOD_CTRL)] {
            if held {
                mods |= bit;
            }
        }
        // Ctrl / Shift alone are the apps' (compile_keys refuses them): the
        // Astur key comes back on, which also fixes such a chord in an old
        // file once its key is set again.
        if mods & (MOD_ALT | MOD_WIN | MOD_HYPER) == 0 {
            mods |= main.bits();
        }
        h.chord = format_chord(mods, key_to_vk(&key).unwrap_or(0), main);
    }
    ui.label("\u{2192}");
    let before = h.action.clone();
    let shown = action_info(&h.action).map_or(h.action.as_str(), |a| a.1);
    egui::ComboBox::from_id_salt("action")
        .selected_text(shown)
        .width(210.0)
        .height(360.0)
        .show_ui(ui, |ui| {
            for a in ACTIONS {
                ui.selectable_value(&mut h.action, a.0.to_string(), a.1);
            }
        });
    let arg = action_info(&h.action).map(|a| a.2);
    if h.action != before {
        h.argument = match arg {
            Some(Arg::Workspace) => "1",
            Some(Arg::Layout) => LAYOUTS[0],
            _ => "",
        }
        .to_string();
    }
    let mut missing = None;
    match arg {
        Some(Arg::Workspace) => {
            let mut n = h.argument.parse::<usize>().unwrap_or(0);
            if workspace_drag(ui, &mut n, ws) {
                h.argument = n.to_string();
            }
            missing = missing_workspace(n, ws);
        }
        Some(Arg::Layout) => {
            let opts: Vec<(&str, &str)> = LAYOUTS.iter().map(|&l| (l, layout_label(l))).collect();
            combo(ui, "layout", &mut h.argument, &opts, 190.0);
        }
        Some(Arg::Command) => {
            cell(ui, &mut h.argument, "Command, e.g. wt -p Ubuntu", 160.0);
            let id = ui.make_persistent_id("command");
            browse_button(ui, id, &mut h.argument, Pick::App);
        }
        _ => {}
    }
    let scratch = h.action == "scratchpad" && scratch_off;
    let err = hotkey_check(h).err();
    // A red row says what is wrong itself.
    let n = match err {
        Some(_) => Vec::new(),
        None => (notes(issues, |x| {
            x.action == Action::Extra && usize::from(x.arg) == i
        }))
        .into_iter()
        .chain(missing.map(|m| (Level::Warn, m)))
        .collect(),
    };
    if scratch || !n.is_empty() {
        ui.end_row(); // notes on their own line, not wrapped mid-row
    }
    note_labels(ui, &n);
    if scratch {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "Scratchpad is off (Desktop extras)",
        );
    }
    err
}

/// What a launcher entry or power-menu action opens: the kind, then its
/// path, address or command.
fn target_ui(ui: &mut Ui, kind: &mut Kind, value: &mut String) {
    let (_, label, hint) = KINDS
        .iter()
        .find(|k| k.0 == *kind)
        .copied()
        .unwrap_or(KINDS[0]);
    egui::ComboBox::from_id_salt("kind")
        .selected_text(label)
        .width(104.0)
        .show_ui(ui, |ui| {
            for &(k, l, _) in &KINDS {
                ui.selectable_value(kind, k, l);
            }
        });
    cell(ui, value, hint, 200.0);
    let pick = match kind {
        Kind::App => Pick::App,
        Kind::File => Pick::File,
        Kind::Folder => Pick::Folder,
        Kind::Url | Kind::Command => return,
    };
    let id = ui.make_persistent_id("target");
    browse_button(ui, id, value, pick);
}

/// An icon: typed (an .ico, .png or .exe path), or a built-in name picked
/// from the menu beside it. `auto` (the target's own icon) is for launcher
/// entries only.
fn icon_ui(ui: &mut Ui, v: &mut String, auto: bool) {
    cell(ui, v, "Icon", 90.0).on_hover_text(
        "A built-in icon (pick one from the menu), or the path of an .ico, .png or .exe",
    );
    let icons = has_icons(ui);
    let id = ui.make_persistent_id("icon-file");
    if let Some(p) = take_pick(id) {
        *v = p;
    }
    ui.menu_button(glyph(icons, ICON_DOWN, "Icons"), |ui| {
        if ui.button("Choose a file\u{2026}").clicked() {
            start_pick(ui.ctx(), id, Pick::Icon, v);
            ui.close_menu();
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .max_height(300.0)
            .show(ui, |ui| {
                let names = auto.then_some("auto").into_iter();
                let names = names.chain(ICON_NAMES.split(' '));
                for name in names {
                    if ui.button(name).clicked() {
                        *v = name.to_string();
                        ui.close_menu();
                    }
                }
            });
    });
}

fn color(ui: &mut Ui, f: &mut Find, meta: Meta, v: &mut u32, d: u32) {
    row(ui, f, meta, v, &d, color_button);
}

/// Themeable colour: Auto follows the theme; unticking it starts from `start`
/// (the colour the theme gives now). `legacy` is the hex the parser reads back
/// as Auto, so a pin of exactly it is nudged one step to stay pinned.
fn theme_color(
    ui: &mut Ui,
    f: &mut Find,
    meta: Meta,
    v: &mut Option<u32>,
    d: Option<u32>,
    start: u32,
    legacy: u32,
) {
    row(ui, f, meta, v, &d, |ui, v| {
        if let Some(c) = v.as_mut() {
            color_button(ui, c);
            *c = pin_theme_color(*c, legacy);
        }
        let mut auto = v.is_none();
        if ui
            .checkbox(&mut auto, "Auto")
            .on_hover_text("Follow the theme")
            .changed()
        {
            *v = if auto {
                None
            } else {
                Some(pin_theme_color(start, legacy))
            };
        }
    });
}

/// A collapsed "More options" for a page's rarer settings (one level only,
/// per Microsoft's settings guidance). While searching its rows show inline,
/// which opens it wherever a hit sits inside.
fn more(ui: &mut Ui, f: &mut Find, salt: &str, add: impl FnOnce(&mut Ui, &mut Find)) {
    if f.on() {
        f.section = Some("More options");
        add(ui, f);
        return;
    }
    ui.add_space(12.0);
    // A picker's "Custom…" asked for it (open_more): open it (it then stays
    // open, as if clicked) and bring it into view.
    let asked = ui
        .data_mut(|d| d.remove_temp::<bool>(more_id(salt)))
        .is_some();
    let r =
        egui::CollapsingHeader::new(RichText::new("More options").size(15.0).family(semibold()))
            .id_salt((salt, "more"))
            .open(asked.then_some(true))
            .show_unindented(ui, |ui| add(ui, f));
    if asked {
        r.header_response.scroll_to_me(Some(Align::Min));
    }
}

/// The flag `open_more` leaves for page `salt`'s More options.
fn more_id(salt: &str) -> egui::Id {
    egui::Id::new(("open-more", salt))
}

/// Open page `salt`'s More options on its next draw (where a picker's raw
/// value is typed).
fn open_more(ctx: &egui::Context, salt: &str) {
    ctx.data_mut(|d| d.insert_temp(more_id(salt), true));
}

/// Rows that only matter while `on`: indented under the switch they depend
/// on, greyed out while it is off. Not while searching: a hit's page heading
/// is drawn inside and must stay clickable.
fn when(ui: &mut Ui, f: &mut Find, on: bool, add: impl FnOnce(&mut Ui, &mut Find)) {
    if f.on() {
        // No frame: an empty one would still add a line of spacing.
        add(ui, f);
        return;
    }
    let indent = Margin {
        left: 24,
        right: 0,
        top: 0,
        bottom: 0,
    };
    egui::Frame::new().inner_margin(indent).show(ui, |ui| {
        ui.add_enabled_ui(on, |ui| add(ui, f));
    });
}

fn section(ui: &mut Ui, f: &mut Find, title: &'static str) {
    if f.on() {
        f.section = Some(title);
    } else {
        section_title(ui, title);
    }
}

fn section_title(ui: &mut Ui, title: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(title).size(15.0).family(semibold()));
    ui.add_space(2.0);
}

fn note(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).color(sub(ui)));
}

fn warn_note(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.colored_label(ui.visuals().warn_fg_color, text);
}

// ---- widgets -----------------------------------------------------------------

fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let v = ui.visuals();
    egui::Frame::new()
        .fill(v.faint_bg_color)
        .stroke(v.widgets.noninteractive.bg_stroke)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Text box padding: egui's 4 x 2 px reads cramped next to Windows 11 fields.
const FIELD_MARGIN: Margin = Margin::symmetric(8, 5);

/// Side of the reset arrow's square, which is also a row's minimum height.
const RESET_SLOT: f32 = 28.0;

/// The reset slot is always reserved, so controls line up whether or not a
/// row shows its arrow.
fn reset_button(ui: &mut Ui, modified: bool) -> bool {
    let sense = if modified {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(RESET_SLOT, RESET_SLOT), sense);
    if !modified {
        return false;
    }
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Reset to default")
    });
    let color = if resp.hovered() {
        ui.visuals().strong_text_color()
    } else {
        sub(ui)
    };
    let (text, font) = if has_icons(ui) {
        (ICON_UNDO.to_string(), FontId::new(14.0, icons_family()))
    } else {
        ("\u{21BA}".to_string(), FontId::proportional(16.0))
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        font,
        color,
    );
    resp.on_hover_text("Reset to default").clicked()
}

/// Windows 11 style on/off switch (egui's demo toggle, restyled): accent fill
/// with a dark knob when on, an outline when off.
fn toggle_switch(ui: &mut Ui, on: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(40.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *on, "")
    });
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(resp.id, *on);
        let fg = ui.style().interact(&resp).fg_stroke.color;
        let accent = accent(ui);
        let r = rect.height() / 2.0;
        let (fill, edge, knob) = if *on {
            (accent, accent, ui.visuals().panel_fill)
        } else {
            (Color32::TRANSPARENT, fg, fg)
        };
        let p = ui.painter();
        p.rect(
            rect,
            r,
            fill,
            Stroke::new(1.0_f32, edge),
            egui::StrokeKind::Inside,
        );
        let x = egui::lerp((rect.left() + r)..=(rect.right() - r), t);
        let size = if resp.hovered() { 7.0 } else { 6.0 };
        p.circle_filled(egui::pos2(x, rect.center().y), size, knob);
    }
    resp
}

/// An on/off switch with a clickable label after it, for lists (egui's
/// checkbox reads as a faint dot in this style).
fn switch(ui: &mut Ui, on: &mut bool, label: &str) {
    toggle_switch(ui, on);
    if ui
        .add(egui::Label::new(label).sense(Sense::click()))
        .clicked()
    {
        *on = !*on;
    }
}

/// Segmented control: a short row of options, the current one highlighted. In
/// a right-to-left row the options are added in reverse so they still read
/// left to right.
fn seg(ui: &mut Ui, v: &mut String, opts: &[(&str, &str)]) {
    let rtl = ui.layout().main_dir() == egui::Direction::RightToLeft;
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let mut each = |&(o, l): &(&str, &str)| {
                if ui.selectable_label(v.as_str() == o, l).clicked() {
                    *v = o.to_string();
                }
            };
            if rtl {
                opts.iter().rev().for_each(&mut each);
            } else {
                opts.iter().for_each(&mut each);
            }
        });
}

/// One table row: framed, its fields wrapping on a narrow window, with a red
/// outline and the reason under it while `add` says it can't be written.
fn item(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> Option<String>) {
    let resp = egui::Frame::new()
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let problem = ui.horizontal_wrapped(add).inner;
            if let Some(p) = &problem {
                ui.colored_label(ui.visuals().error_fg_color, p);
            }
            problem.is_some()
        });
    let v = ui.visuals();
    let stroke = if resp.inner {
        Stroke::new(1.0_f32, v.error_fg_color)
    } else {
        v.widgets.noninteractive.bg_stroke
    };
    ui.painter()
        .rect_stroke(resp.response.rect, 6.0, stroke, egui::StrokeKind::Inside);
}

/// Move up / down (and with `delete`, delete) buttons for row `i` of `n`;
/// a click is left in `edit` for `move_row`.
fn tools(ui: &mut Ui, i: usize, n: usize, delete: bool, edit: &mut Option<(usize, isize)>) {
    let icons = has_icons(ui);
    let mut button = |on: bool, icon: char, word: &str, step: isize| {
        let b = egui::Button::new(glyph(icons, icon, word)).frame(false);
        if ui.add_enabled(on, b).on_hover_text(word).clicked() {
            *edit = Some((i, step));
        }
    };
    button(i > 0, ICON_UP, "Move up", -1);
    button(i + 1 < n, ICON_DOWN, "Move down", 1);
    if delete {
        button(true, ICON_DELETE, "Delete", 0);
    }
}

/// Apply a `tools` click: (i, 0) deletes row i, (i, +-1) swaps it with that
/// neighbour.
fn move_row<T>(rows: &mut Vec<T>, edit: Option<(usize, isize)>) {
    match edit {
        Some((i, 0)) => {
            rows.remove(i);
        }
        Some((i, step)) => rows.swap(i, i.wrapping_add_signed(step)),
        None => {}
    }
}

/// A table cell: a one-line text box with a hint.
fn cell(ui: &mut Ui, v: &mut String, hint: &str, width: f32) -> egui::Response {
    let resp = ui.add(
        egui::TextEdit::singleline(v)
            .hint_text(hint)
            .desired_width(width)
            .margin(FIELD_MARGIN),
    );
    one_line(v);
    resp
}

/// egui's one-line box pastes a line break as is (0.31). Written, it would
/// split the conf line: the rest lands on a stray line of its own and the
/// value never reads back. A space keeps the box equal to the file.
fn one_line(v: &mut String) {
    if v.contains(['\r', '\n']) {
        *v = v.replace(['\r', '\n'], " ");
    }
}

/// Which key chip is listening, kept in egui's memory.
fn capture_id() -> egui::Id {
    egui::Id::new("astur-key-capture")
}

/// A key as a capture chip: click it, then press the key. Esc or a click
/// elsewhere cancels, Backspace clears. Only the key is read, never Alt:
/// Astur's hook eats Left Alt before this window could see it, so the user
/// is never asked to press Alt+J to record Alt+J (it would run instead).
/// With `mods` (an extra hotkey's Shift and Ctrl toggles), Shift or Ctrl
/// held during the press switch those on too. Keys the slot can't use are
/// ignored and it keeps listening. Returns true when a key was set or
/// cleared.
fn key_chip(
    ui: &mut Ui,
    id: egui::Id,
    v: &mut String,
    mut mods: Option<(&mut bool, &mut bool)>,
) -> bool {
    let listening = ui.ctx().data(|d| d.get_temp::<egui::Id>(capture_id())) == Some(id);
    let chord = mods.is_some();
    let (mut changed, mut stop) = (false, false);
    if listening {
        // Some(name): a key ("" = cleared) and the modifiers held with it.
        let mut got: Option<(String, egui::Modifiers)> = None;
        ui.input_mut(|i| {
            // What the press typed (no Text comes with Ctrl held), to check
            // that an OEM key is where its US name says.
            let typed = i.events.iter().find_map(|e| match e {
                egui::Event::Text(t) => t.chars().next(),
                _ => None,
            });
            let held = i.modifiers;
            i.events.retain(|e| match e {
                egui::Event::Key {
                    key,
                    physical_key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if got.is_none() && !stop {
                        match key {
                            egui::Key::Escape => stop = true,
                            egui::Key::Backspace => got = Some((String::new(), *modifiers)),
                            // The logical key, else the key at that place
                            // for a digit (Shift+1 is "!" logically, AZERTY's
                            // 1 is "&"; Windows layouts keep VK_1 there) or
                            // an OEM key (Shift+[ is "{"). Not letters: on
                            // AZERTY "," is where US has M. An OEM name binds
                            // its VK_OEM_* code, which sits on another key on
                            // most other layouts: taken only when the press
                            // typed its US glyph (egui gives the US place for
                            // German ü, ß, ^ and +).
                            k => {
                                got = key_name(*k, chord)
                                    .or_else(|| {
                                        physical_key.and_then(|p| key_name(p, chord)).filter(|n| {
                                            n.bytes().all(|b| b.is_ascii_digit())
                                                || oem_glyphs(n).is_some()
                                        })
                                    })
                                    .filter(|n| {
                                        // An OEM name also only at its US
                                        // place: German ; is Shift+, there.
                                        let place = physical_key
                                            .is_none_or(|p| key_name(p, chord).as_ref() == Some(n));
                                        oem_glyphs(n).is_none_or(|g| {
                                            place && typed.is_some_and(|t| g.contains(t))
                                        })
                                    })
                                    .map(|n| (n, *modifiers));
                            }
                        }
                    }
                    false
                }
                // egui-winit turns Ctrl+C / X / V into these, with no Key
                // event. (Paste comes only with text on the clipboard.)
                egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_) => {
                    if got.is_none() && !stop && held.ctrl {
                        let k = match e {
                            egui::Event::Copy => "C",
                            egui::Event::Cut => "X",
                            _ => "V",
                        };
                        got = Some((k.to_string(), held));
                    }
                    false
                }
                egui::Event::Key { .. } | egui::Event::Text(_) => false,
                _ => true,
            });
        });
        if let Some((name, held)) = got {
            if let Some((shift, ctrl)) = mods.as_mut().filter(|_| !name.is_empty()) {
                **shift |= held.shift;
                **ctrl |= held.ctrl;
            }
            // The same key again still counts: it rewrites a chord the WM
            // refuses (no Astur key) in the form it reads.
            changed = true;
            *v = name;
            stop = true;
        }
    }
    let text = if listening {
        "Press a key".to_string()
    } else if v.is_empty() {
        "None".to_string()
    } else {
        key_label(v)
    };
    let tip = if chord {
        "Click, then press the key: 0-9, A-Z, F1-F24, Space, Tab, Enter, or [ ] ` - = where a US keyboard has them (without Ctrl held: use the Ctrl switch). Esc cancels; Backspace clears."
    } else {
        "Click, then press the key: 0-9, A-Z, F1-F24, or [ ] where a US keyboard has them. Esc cancels; Backspace clears."
    };
    let resp = ui
        .add(
            egui::Button::new(RichText::new(text).family(semibold()))
                .selected(listening)
                .min_size(egui::vec2(56.0, 0.0)),
        )
        .on_hover_text(tip);
    if resp.clicked() {
        stop = listening;
        if !listening {
            ui.ctx().data_mut(|d| d.insert_temp(capture_id(), id));
        }
    } else if listening && resp.clicked_elsewhere() {
        stop = true;
    }
    if stop {
        ui.ctx().data_mut(|d| d.remove::<egui::Id>(capture_id()));
    }
    changed
}

/// `notes`, coloured by how bad: red does nothing, amber costs something.
fn note_labels(ui: &mut Ui, notes: &[(Level, String)]) {
    for (level, text) in notes {
        let v = ui.visuals();
        let color = match level {
            Level::Error => v.error_fg_color,
            Level::Warn => v.warn_fg_color,
            Level::Info => sub(ui),
        };
        ui.label(RichText::new(text).small().color(color));
    }
}

/// compile_keys' issues that `pick` keeps, as the lines a row shows: worst
/// first, and one line per text with its chords joined ("Win+1, Win+2:
/// replaces Windows' own shortcut, ..."), so ten workspace keys read once.
fn notes(issues: &[BindIssue], pick: impl Fn(&BindIssue) -> bool) -> Vec<(Level, String)> {
    let mut out: Vec<(Level, &str, Vec<&str>)> = Vec::new();
    for i in issues.iter().filter(|i| pick(i)) {
        match out.iter_mut().find(|o| o.0 == i.level && o.1 == i.text) {
            Some(o) => o.2.push(&i.chord),
            None => out.push((i.level, &i.text, vec![&i.chord])),
        }
    }
    out.sort_by_key(|o| o.0);
    (out.into_iter())
        .map(|(level, text, chords)| {
            // "Win+L always locks the PC, ..." names its chord itself.
            if text.contains(chords[0]) {
                (level, text.to_string())
            } else {
                (level, format!("{}: {text}", chords.join(", ")))
            }
        })
        .collect()
}

/// The Astur key as chords start with it ("Alt+", "Win+Ctrl+", "Caps
/// Lock+"), spelled as astur-config's `chord_label` spells every chord, so
/// the labels read like the notes.
fn mk(c: &Config) -> String {
    chord_label(c.main_key.bits(), 0)
}

/// `start_tiled`, on Home and Windows alike. The WM reads it once, when it
/// starts, and it sets tiling for every window (the Toggle tiling key's
/// state), not only new ones: the old "Tile new windows" promised neither.
const TILE_LABEL: &str = "Tile windows when Astur starts";

fn tile_hint(c: &Config) -> String {
    format!("Off: Astur starts with tiling off, so every window floats as on plain Windows until you press {}{}. Takes effect the next time Astur starts.", mk(c), vk_to_key(c.key_toggle_tiling))
}

/// The Astur key before a key chip: "Alt", "Win+Ctrl".
fn held(c: &Config) -> String {
    mk(c).trim_end_matches('+').to_string()
}

/// Leaving an Alt-based Astur key (Left Alt, Alt) for another, a custom
/// shortcut written with ALT meant the Astur key: it becomes MOD, so it
/// moves to the new key (the default scratchpad ALT+GRAVE) instead of
/// staying on a plain Alt. Under Left Alt, MOD and ALT are the same key, so
/// going back needs nothing.
fn follow_key(hotkeys: &mut [HotkeyDef], from: MainKey, to: MainKey) {
    if from.bits() != MOD_ALT || to.bits() == MOD_ALT {
        return;
    }
    for h in hotkeys {
        h.chord = (h.chord.split('+'))
            .map(|t| {
                if t.trim().eq_ignore_ascii_case("alt") {
                    "MOD"
                } else {
                    t
                }
            })
            .collect::<Vec<_>>()
            .join("+");
    }
}

/// Whole seconds left until `until`, rounded up: the banner reads 15 the
/// moment a trial starts and 1 in its last second.
fn secs_left(until: Instant, now: Instant) -> u128 {
    until
        .saturating_duration_since(now)
        .as_millis()
        .div_ceil(1000)
}

/// Windows' own shortcuts that Astur key `k` would take over with the
/// shortcuts `c` has: (how many, the notes).
fn takeovers(c: &Config, k: MainKey) -> (usize, Vec<(Level, String)>) {
    let mut t = c.clone();
    follow_key(&mut t.extra_hotkeys, c.main_key, k);
    t.main_key = k;
    let issues = compile_keys(&t).issues;
    // compile_keys' caution text for a chord on Windows' table.
    let pick = |i: &BindIssue| i.level != Level::Error && i.text.starts_with("replaces Windows");
    (
        issues.iter().filter(|i| pick(i)).count(),
        notes(&issues, pick),
    )
}

/// What the picker says beyond a key's own description.
fn caveat(k: MainKey) -> &'static str {
    match k {
        MainKey::LAlt => "",
        MainKey::Win | MainKey::CtrlWin => "Tapping the Windows key on its own still opens Start; after an Astur shortcut it stays closed.",
        MainKey::Alt => "Menu bars don't light up after an Astur shortcut.",
        MainKey::CapsLock => "Shift still types capital letters.",
    }
}

/// The Astur key as one card per choice: what it means, and which Windows
/// shortcuts it takes over with the shortcuts in use (counted on the other
/// cards, listed on the chosen one). Returns the key clicked, if new.
fn key_picker(ui: &mut Ui, f: &mut Find, c: &Config) -> Option<MainKey> {
    let mut picked = None;
    let meta = (
        "Astur key",
        "The key you hold for every Astur shortcut, and to drag or resize a window. A new one works at once; keep it within 15 seconds, or it goes back.",
        "main_key",
    );
    let modified = c.main_key != MainKey::LAlt;
    if row_core(ui, f, meta, modified, true, |ui| {
        for k in MainKey::ALL {
            let on = k == c.main_key;
            let (n, lines) = takeovers(c, k);
            let (grey, warn) = (sub(ui), ui.visuals().warn_fg_color);
            let stroke = if on {
                Stroke::new(2.0_f32, accent(ui))
            } else {
                ui.visuals().widgets.noninteractive.bg_stroke
            };
            let mut radio = false;
            let card = egui::Frame::new()
                .stroke(stroke)
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let title = RichText::new(k.label()).family(semibold());
                    radio = ui.radio(on, title).clicked();
                    let small = |t: &str| RichText::new(t).small().color(grey);
                    ui.label(small(k.description()));
                    if !caveat(k).is_empty() {
                        ui.label(small(caveat(k)));
                    }
                    if on && n > 0 {
                        ui.label(small("Windows shortcuts it takes over:"));
                        note_labels(ui, &lines);
                    } else if n > 0 {
                        let s = if n == 1 { "" } else { "s" };
                        let text = format!("Takes over {n} Windows shortcut{s}");
                        let all: Vec<&str> = lines.iter().map(|l| l.1.as_str()).collect();
                        ui.label(RichText::new(text).small().color(warn))
                            .on_hover_text(all.join("\n"));
                    }
                })
                .response
                .interact(Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if radio || card.clicked() {
                picked = Some(k);
            }
            ui.add_space(2.0);
        }
    }) {
        picked = Some(MainKey::LAlt);
    }
    picked.filter(|&k| k != c.main_key)
}

/// The shortcuts with no key of their own to set, as (keys, what they do,
/// the actions compile_keys calls them, to find their notes).
fn fixed(c: &Config) -> Vec<(String, &'static str, &'static [Action])> {
    use Action::*;
    let m = mk(c);
    let mut v: Vec<(String, &'static str, &'static [Action])> = vec![
        (
            format!("{m}arrows"),
            "Focus the window in that direction",
            &[FocusLeft, FocusUp, FocusRight, FocusDown],
        ),
        (
            format!("{m}Shift+arrows"),
            "Move the window in that direction",
            &[MoveLeft, MoveUp, MoveRight, MoveDown],
        ),
        (format!("{m}Enter"), "Open the terminal", &[Terminal]),
        (format!("{m}Shift+Enter"), "Open the browser", &[Browser]),
        (format!("{m}Space"), "Launcher", &[Launcher]),
        (format!("{m}Shift+Space"), "Power menu", &[SystemMenu]),
        (
            format!("{m}Home / End"),
            "Scrolling: first / last column (with Shift: take the window along)",
            &[FirstColumn, LastColumn, MoveFirstColumn, MoveLastColumn],
        ),
    ];
    // Astur's own Alt+Tab needs its Alt to be Astur's (the WM ignores
    // alt_tab_replacement under any other key).
    if c.main_key == MainKey::LAlt {
        let tab = "Switch windows (Astur's switcher when it is on)";
        v.insert(6, ("Alt+Tab".into(), tab, &[]));
    }
    v
}

fn mouse(c: &Config) -> Vec<(String, &'static str)> {
    let m = mk(c);
    vec![
        (format!("{m}left-drag"), "Move a window"),
        (format!("{m}right-drag"), "Resize from the nearest corner"),
        (
            format!("{m}wheel"),
            "Scrolling layout: scroll the columns (with Shift: take the window along)",
        ),
        (
            "Wheel over the bar".into(),
            "Previous / next workspace (Status bar > Behaviour)",
        ),
        ("Click a workspace on the bar".into(), "Switch to it"),
    ]
}

/// COLORREF (0x00BBGGRR) colour button.
fn color_button(ui: &mut Ui, c: &mut u32) {
    let mut rgb = [*c as u8, (*c >> 8) as u8, (*c >> 16) as u8];
    if ui.color_edit_button_srgb(&mut rgb).changed() {
        *c = u32::from(rgb[0]) | u32::from(rgb[1]) << 8 | u32::from(rgb[2]) << 16;
    }
}

fn colorref(c: u32) -> Color32 {
    Color32::from_rgb(c as u8, (c >> 8) as u8, (c >> 16) as u8)
}

/// Tile rectangles per layout as fractions of the screen (x0, y0, x1, y1), for
/// the picker thumbnails. Scrolling's last column runs off the edge on purpose.
fn layout_tiles(name: &str) -> &'static [[f32; 4]] {
    match name {
        "master" => &[
            [0.0, 0.0, 0.55, 1.0],
            [0.55, 0.0, 1.0, 0.34],
            [0.55, 0.34, 1.0, 0.67],
            [0.55, 0.67, 1.0, 1.0],
        ],
        "columns" => &[
            [0.0, 0.0, 0.34, 1.0],
            [0.34, 0.0, 0.67, 1.0],
            [0.67, 0.0, 1.0, 1.0],
        ],
        "grid" => &[
            [0.0, 0.0, 0.5, 0.5],
            [0.5, 0.0, 1.0, 0.5],
            [0.0, 0.5, 0.5, 1.0],
            [0.5, 0.5, 1.0, 1.0],
        ],
        "monocle" => &[[0.16, 0.0, 1.0, 0.84], [0.0, 0.16, 0.84, 1.0]],
        "scrolling" => &[
            [0.0, 0.0, 0.45, 1.0],
            [0.45, 0.0, 0.9, 1.0],
            [0.9, 0.0, 1.0, 1.0],
        ],
        _ => &[
            [0.0, 0.0, 0.5, 1.0],
            [0.5, 0.0, 1.0, 0.5],
            [0.5, 0.5, 0.75, 1.0],
            [0.75, 0.5, 1.0, 1.0],
        ],
    }
}

/// Six clickable drawn thumbnails, one per layout.
fn layout_picker(ui: &mut Ui, v: &mut String) {
    ui.horizontal_wrapped(|ui| {
        for &l in LAYOUTS {
            let selected = v.as_str() == l;
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(108.0, 92.0), Sense::click());
            let name = layout_label(l);
            resp.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, selected, name)
            });
            let vis = ui.visuals();
            let acc = accent(ui);
            let edge = if selected {
                Stroke::new(2.0_f32, acc)
            } else if resp.hovered() {
                Stroke::new(1.0_f32, vis.strong_text_color())
            } else {
                vis.widgets.noninteractive.bg_stroke
            };
            let p = ui.painter();
            p.rect(
                rect,
                8.0,
                vis.extreme_bg_color,
                edge,
                egui::StrokeKind::Inside,
            );
            let screen = egui::Rect::from_min_size(
                rect.min + egui::vec2(12.0, 10.0),
                egui::vec2(84.0, 52.0),
            );
            let tile = match (selected, vis.dark_mode) {
                (true, _) => acc,
                (false, true) => Color32::from_gray(92),
                (false, false) => Color32::from_gray(196),
            };
            for t in layout_tiles(l) {
                let r = egui::Rect::from_min_max(
                    screen.lerp_inside(egui::vec2(t[0], t[1])),
                    screen.lerp_inside(egui::vec2(t[2].min(1.0), t[3])),
                );
                p.rect_filled(r.shrink(1.5), 2.0, tile);
            }
            p.text(
                egui::pos2(rect.center().x, rect.bottom() - 15.0),
                egui::Align2::CENTER_CENTER,
                name,
                FontId::proportional(12.0),
                vis.text_color(),
            );
            if resp.on_hover_text(layout_help(l)).clicked() {
                *v = l.to_string();
            }
        }
    });
}

fn nav_item(ui: &mut Ui, selected: bool, icon: char, title: &str) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 36.0), Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, title)
    });
    let v = ui.visuals();
    // Opaque: egui's from_white_alpha is linear, so a "6 %" white tint
    // gamma-encodes to a far brighter box than Windows 11's.
    let g = Color32::from_gray;
    let (sel, hover) = if v.dark_mode {
        (g(46), g(40))
    } else {
        (g(232), g(237))
    };
    let p = ui.painter();
    if selected || resp.hovered() {
        p.rect_filled(rect, 6.0, if selected { sel } else { hover });
    }
    if selected {
        let pill = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 2.0, rect.center().y),
            egui::vec2(3.0, 16.0),
        );
        p.rect_filled(pill, 1.5, accent(ui));
    }
    let fg = v.text_color();
    if has_icons(ui) {
        p.text(
            egui::pos2(rect.left() + 14.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            icon,
            FontId::new(16.0, icons_family()),
            fg,
        );
    }
    p.text(
        egui::pos2(rect.left() + 44.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        FontId::proportional(14.0),
        fg,
    );
    resp
}

fn glyph(icons: bool, icon: char, word: &str) -> RichText {
    if icons {
        RichText::new(icon).family(icons_family())
    } else {
        RichText::new(word)
    }
}

/// Secondary text (descriptions): Windows 11's softer grey, not egui's faded one.
fn sub(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_gray(190)
    } else {
        Color32::from_gray(96)
    }
}

/// Astur's accent (set as the hyperlink colour by `palette`).
fn accent(ui: &Ui) -> Color32 {
    ui.visuals().hyperlink_color
}

fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

fn icons_family() -> FontFamily {
    FontFamily::Name("icons".into())
}

fn icons_id() -> egui::Id {
    egui::Id::new("astur-icons")
}

/// Whether an icon font loaded (kept per egui context, not global, so tests
/// with and without one do not collide).
fn has_icons(ui: &Ui) -> bool {
    ui.ctx()
        .data(|d| d.get_temp::<bool>(icons_id()))
        .unwrap_or(false)
}

fn search_id() -> egui::Id {
    egui::Id::new("astur-settings-search")
}

/// The text box (or a slider's typed number) that has the keyboard: its edit
/// is unfinished until it loses focus or Enter is pressed.
fn text_focus(ctx: &egui::Context) -> Option<egui::Id> {
    let id = ctx.memory(|m| m.focused())?;
    egui::text_edit::TextEditState::load(ctx, id).map(|_| id)
}

/// Mid-gesture: a button held, a text box being typed in, a combo or colour
/// popup open. Nothing is written then (`settle`, the trial's timeout), so a
/// half-typed number or a slider mid-drag never re-tiles the desktop. A held
/// key counts too (an arrow held on a focused slider), so a run of steps is
/// written once, after the release; `keys_down` has no modifiers, and
/// `focused` covers a key-up lost to another window.
fn busy(ctx: &egui::Context) -> bool {
    let typing = text_focus(ctx).is_some_and(|id| id != search_id());
    ctx.input(|i| i.focused && (i.pointer.any_down() || typing || !i.keys_down.is_empty()))
        || ctx.memory(|m| m.any_popup_open())
}

/// Fonts and style, once at startup. Segoe UI Variable (Windows 11; Segoe UI
/// on 10) for text, Consolas for code, Segoe UI Semibold for headings, and
/// Segoe Fluent Icons (MDL2 Assets on 10) for icons, read from the Windows
/// font folder so the exe stays small. egui's built-in fonts stay behind them
/// as the fallback (`default_fonts`), and without an icon font the icons are
/// left out.
fn setup_look(ctx: &egui::Context) {
    let dir = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into()))
        .join("Fonts");
    let read = |names: &[&str]| names.iter().find_map(|n| std::fs::read(dir.join(n)).ok());
    let mut fonts = egui::FontDefinitions::default();
    if let Some(data) = read(&["SegUIVar.ttf", "segoeui.ttf"]) {
        add_font(&mut fonts, "ui", data, FontFamily::Proportional);
    }
    if let Some(data) = read(&["consola.ttf"]) {
        add_font(&mut fonts, "mono", data, FontFamily::Monospace);
    }
    let body = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    fonts.families.insert(semibold(), body);
    if let Some(data) = read(&["seguisb.ttf"]) {
        add_font(&mut fonts, "semibold", data, semibold());
    }
    let icons = read(&["SegoeIcons.ttf", "segmdl2.ttf"]);
    let has = icons.is_some();
    if let Some(data) = icons {
        add_font(&mut fonts, "icons", data, icons_family());
    }
    ctx.set_fonts(fonts);
    ctx.data_mut(|d| d.insert_temp(icons_id(), has));
    ctx.all_styles_mut(|s| {
        use egui::TextStyle as T;
        s.text_styles = [
            (T::Small, 12.0),
            (T::Body, 14.0),
            (T::Button, 14.0),
            (T::Heading, 20.0),
            (T::Monospace, 13.0),
        ]
        .into_iter()
        .map(|(t, size)| {
            let family = if t == T::Monospace {
                FontFamily::Monospace
            } else {
                FontFamily::Proportional
            };
            (t, FontId::new(size, family))
        })
        .collect();
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(12.0, 5.0);
        s.spacing.interact_size.y = 28.0;
        s.spacing.slider_rail_height = 4.0;
        let w = &mut s.visuals.widgets;
        for v in [
            &mut w.noninteractive,
            &mut w.inactive,
            &mut w.hovered,
            &mut w.active,
            &mut w.open,
        ] {
            v.corner_radius = CornerRadius::same(6);
            v.expansion = 0.0;
        }
        s.visuals.window_corner_radius = CornerRadius::same(8);
        s.visuals.menu_corner_radius = CornerRadius::same(8);
        s.visuals.slider_trailing_fill = true;
    });
    ctx.style_mut_of(egui::Theme::Dark, |s| palette(&mut s.visuals, true));
    ctx.style_mut_of(egui::Theme::Light, |s| palette(&mut s.visuals, false));
}

fn add_font(fonts: &mut egui::FontDefinitions, name: &str, data: Vec<u8>, family: FontFamily) {
    fonts.font_data.insert(
        name.to_owned(),
        std::sync::Arc::new(egui::FontData::from_owned(data)),
    );
    fonts
        .families
        .entry(family)
        .or_default()
        .insert(0, name.to_owned());
}

/// Windows 11 settings colours over egui's defaults, with Astur's own theme
/// accent so Settings matches the bar and popups. Text contrast is lifted:
/// egui's dark default labels are ~gray(140) on near-black.
fn palette(v: &mut egui::Visuals, dark: bool) {
    let g = Color32::from_gray;
    let accent = colorref(if dark { BAR_DARK[2] } else { BAR_LIGHT[2] });
    let [bg, card, edge, field, fg, button, hover] = if dark {
        [g(32), g(43), g(56), g(29), g(242), g(55), g(66)]
    } else {
        [g(243), g(251), g(226), g(255), g(26), g(253), g(244)]
    };
    v.panel_fill = bg;
    v.window_fill = card;
    v.window_stroke = Stroke::new(1.0_f32, edge);
    v.faint_bg_color = card;
    v.extreme_bg_color = field;
    v.code_bg_color = field;
    v.hyperlink_color = accent;
    v.selection.bg_fill = accent.gamma_multiply(if dark { 0.45 } else { 0.3 });
    v.selection.stroke = Stroke::new(1.0_f32, fg);
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, edge);
    for (s, fill) in [
        (&mut w.inactive, button),
        (&mut w.hovered, hover),
        (&mut w.active, button),
        (&mut w.open, hover),
    ] {
        s.bg_fill = fill;
        s.weak_bg_fill = fill;
        s.bg_stroke = Stroke::new(1.0_f32, edge);
    }
    for s in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        s.fg_stroke.color = fg;
    }
}

// ---- pure helpers --------------------------------------------------------------

/// Motion presets: (id, animation_ms, workspace_anim, window_anim,
/// animation_easing), all with `animations` on. "smooth" is exactly the shipped
/// defaults, so an untouched config reads as Smooth; values matching no preset
/// read as Custom and are left alone.
const MOTION: [(&str, i32, &str, &str, &str); 3] = [
    ("subtle", 100, "fade", "glide", "smooth"),
    ("smooth", 140, "slide", "glide", "cubic"),
    ("springy", 220, "spring", "spring", "spring"),
];
const MOTION_OPTS: [(&str, &str); 4] = [
    ("off", "Off"),
    ("subtle", "Subtle"),
    ("smooth", "Smooth"),
    ("springy", "Springy"),
];

fn motion_preset(c: &Config) -> Option<&'static str> {
    if !c.animations {
        return Some("off");
    }
    let now = (
        c.animation_ms,
        c.workspace_anim.as_str(),
        c.window_anim.as_str(),
        c.animation_easing.as_str(),
    );
    MOTION
        .iter()
        .find(|p| (p.1, p.2, p.3, p.4) == now)
        .map(|p| p.0)
}

/// Off only clears `animations`, so the style is kept for a later Custom.
fn set_motion(c: &mut Config, id: &str) {
    c.animations = id != "off";
    if let Some(p) = MOTION.iter().find(|p| p.0 == id) {
        c.animation_ms = p.1;
        c.workspace_anim = p.2.to_string();
        c.window_anim = p.3.to_string();
        c.animation_easing = p.4.to_string();
    }
}

/// "Dim inactive windows" percent from `unfocused_opacity` (1.0 = no dimming).
fn dim_pct(opacity: f32) -> i32 {
    ((1.0 - opacity) * 100.0).round() as i32
}

fn opacity_for_dim(pct: i32) -> f32 {
    1.0 - pct as f32 / 100.0
}

/// Argument an extra hotkey action takes.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Arg {
    None,
    Workspace,
    Layout,
    Command,
}

/// The actions an extra hotkey can run, as (name, label, argument): the
/// WM's process_extra and scroll_action (astur main.rs), which this list
/// must follow. Not in astur-config yet (the keybind engine's ACTIONS table).
const ACTIONS: &[(&str, &str, Arg)] = &[
    ("launch", "Run a command", Arg::Command),
    ("switch_workspace", "Switch to workspace", Arg::Workspace),
    (
        "move_to_workspace",
        "Move the window to workspace",
        Arg::Workspace,
    ),
    ("layout", "Set this monitor's layout", Arg::Layout),
    ("focus_next", "Focus next window", Arg::None),
    ("focus_prev", "Focus previous window", Arg::None),
    ("swap_next", "Swap with the next window", Arg::None),
    ("swap_prev", "Swap with the previous window", Arg::None),
    ("promote_master", "Make it the main window", Arg::None),
    ("shrink_master", "Make the main window smaller", Arg::None),
    ("grow_master", "Make the main window bigger", Arg::None),
    ("toggle_tiling", "Turn tiling on or off", Arg::None),
    ("toggle_float", "Float or tile the window", Arg::None),
    ("close", "Close the window", Arg::None),
    ("terminal", "Open the terminal", Arg::None),
    ("browser", "Open the browser", Arg::None),
    ("scratchpad", "Show or hide the scratchpad", Arg::None),
    ("launcher", "Open the launcher", Arg::None),
    ("system_menu", "Open the power menu", Arg::None),
    ("reload", "Reload the config", Arg::None),
    (
        "focus_column_left",
        "Scrolling: focus the column on the left",
        Arg::None,
    ),
    (
        "focus_column_right",
        "Scrolling: focus the column on the right",
        Arg::None,
    ),
    (
        "move_column_left",
        "Scrolling: move the column left",
        Arg::None,
    ),
    (
        "move_column_right",
        "Scrolling: move the column right",
        Arg::None,
    ),
    (
        "cycle_width",
        "Scrolling: cycle the column width",
        Arg::None,
    ),
    (
        "cycle_height",
        "Scrolling: cycle the window height",
        Arg::None,
    ),
    ("center_column", "Scrolling: centre the column", Arg::None),
    (
        "consume_left",
        "Scrolling: pull into the left column",
        Arg::None,
    ),
    (
        "consume_right",
        "Scrolling: pull into the right column",
        Arg::None,
    ),
    (
        "toggle_tabbed",
        "Scrolling: tabbed column on or off",
        Arg::None,
    ),
    ("maximize_column", "Scrolling: full-width column", Arg::None),
    ("first_column", "Scrolling: first column", Arg::None),
    ("last_column", "Scrolling: last column", Arg::None),
];

/// An action by name, old spellings the WM also takes included.
fn action_info(name: &str) -> Option<&'static (&'static str, &'static str, Arg)> {
    let name = match name {
        "close_window" => "close",
        "command" => "launch",
        n => n,
    };
    ACTIONS.iter().find(|a| a.0 == name)
}

/// How a key name is shown: "`" for GRAVE, "Space" for SPACE.
fn key_label(name: &str) -> String {
    key_to_vk(name).map_or_else(|| name.trim().to_ascii_uppercase(), vk_to_key)
}

/// The conf name of a pressed key, if this slot can use it: what key_to_vk
/// reads (0-9, A-Z, F1-F24, [ ]) and, for a `chord`, the names only
/// extra_hotkeys take. OEM keys are named by their US-layout place.
fn key_name(k: egui::Key, chord: bool) -> Option<String> {
    use egui::Key as K;
    let name = match k {
        K::Space => "SPACE",
        K::Tab => "TAB",
        K::Enter => "ENTER",
        K::Backtick => "GRAVE",
        K::Minus => "MINUS",
        K::Equals => "EQUAL",
        k => return key_to_vk(k.symbol_or_name()).map(vk_to_key),
    };
    chord.then(|| name.to_string())
}

/// The US glyphs (plain, shifted) of a key named by its US place. Its
/// VK_OEM_* code is what the WM matches, and other layouts put that code on
/// another key (German: VK_OEM_4 is the ß key), so key_chip takes the name
/// only when the press typed one of these. ponytail: MapVirtualKeyW through
/// windows-sys would bind the real key on any layout.
fn oem_glyphs(name: &str) -> Option<&'static str> {
    Some(match name {
        "[" => "[{",
        "]" => "]}",
        "GRAVE" | "`" => "`~",
        "MINUS" | "-" => "-_",
        "EQUAL" | "=" => "=+",
        // Comma and Period are not here: VK_OEM_COMMA / PERIOD are the ,
        // and . keys on every layout (Win32 docs).
        "Semicolon" => ";:",
        "Quote" => "'\"",
        "Slash" => "/?",
        "Backslash" => "\\|",
        _ => return None,
    })
}

/// Every shortcut as plain text ("Copy cheat sheet"): compile_keys' binds,
/// so it lists exactly what Astur binds, on whichever Astur key.
fn cheat_sheet(c: &Config) -> String {
    use Action::*;
    let binds = compile_keys(c).binds;
    let mut lines = vec![format!("Astur shortcuts (hold {})", c.main_key.label())];
    let mut seen = Vec::new();
    for (title, group) in [
        ("Windows and layout", 0),
        ("Workspaces (with Shift: take the window along)", 1),
        ("Custom", 2),
    ] {
        let mut rows = Vec::new();
        for b in &binds {
            let g = match b.action {
                SwitchWorkspace | MoveToWorkspace => 1,
                Extra => 2,
                _ => 0,
            };
            let shown = b.action != SwitchWorkspace || usize::from(b.arg) < c.workspaces;
            // Under Left Alt a key also answers with Ctrl held: listed once.
            if g != group
                || b.action == MoveToWorkspace
                || !shown
                || seen.contains(&(b.action, b.arg))
            {
                continue;
            }
            seen.push((b.action, b.arg));
            let i = usize::from(b.arg);
            let what = match b.action {
                SwitchWorkspace => {
                    let name = c.workspace_names.get(i).filter(|n| !n.is_empty());
                    let name = name.map_or(String::new(), |n| format!(" ({n})"));
                    format!("Workspace {}{name}", i + 1)
                }
                Extra => c.extra_hotkeys.get(i).map_or_else(String::new, |h| {
                    let what = action_info(&h.action).map_or(h.action.as_str(), |a| a.1);
                    format!("{what} {}", h.argument).trim_end().to_string()
                }),
                a => a.label().to_string(),
            };
            rows.push(format!("  {}  {what}", chord_label(b.mods, b.vk)));
        }
        if !rows.is_empty() {
            lines.extend([String::new(), title.to_string()]);
            lines.extend(rows);
        }
    }
    lines.extend([String::new(), "Mouse".to_string()]);
    lines.extend(mouse(c).iter().map(|(k, what)| format!("  {k}  {what}")));
    lines.join("\n") + "\n"
}

/// The 12 single-key binds' VK codes, in `BINDS` order.
fn binds(c: &Config) -> [u32; 12] {
    [
        c.key_focus_next,
        c.key_focus_prev,
        c.key_shrink_master,
        c.key_grow_master,
        c.key_promote_master,
        c.key_toggle_tiling,
        c.key_toggle_float,
        c.key_close_window,
        c.key_cycle_width,
        c.key_center_column,
        c.key_consume_left,
        c.key_consume_right,
    ]
}

fn binds_mut(c: &mut Config) -> [&mut u32; 12] {
    [
        &mut c.key_focus_next,
        &mut c.key_focus_prev,
        &mut c.key_shrink_master,
        &mut c.key_grow_master,
        &mut c.key_promote_master,
        &mut c.key_toggle_tiling,
        &mut c.key_toggle_float,
        &mut c.key_close_window,
        &mut c.key_cycle_width,
        &mut c.key_center_column,
        &mut c.key_consume_left,
        &mut c.key_consume_right,
    ]
}

/// The kinds of target, as (kind, label, field hint).
const KINDS: [(Kind, &str, &str); 5] = [
    (Kind::App, "App", r"C:\...\app.exe or a shortcut"),
    (Kind::File, "File", r"C:\...\notes.txt"),
    (Kind::Folder, "Folder", r"C:\Projects"),
    (Kind::Url, "Web page", "https://..."),
    (Kind::Command, "Command", "wt -p Ubuntu"),
];

/// The built-in icon names (astur main.rs is_builtin_icon), as the README
/// lists them.
const ICON_NAMES: &str = "app browser calculator clipboard command file folder grid lock media power power-circle reload restart screenshot search settings setup signout sleep terminal wallpaper web window";

/// The power menu's built-ins (astur main.rs builtin_sys_item), as (name,
/// label): the Power section's six, then the Astur section's. Either list
/// takes any of them.
const SYSTEM_ITEMS: [(&str, &str); 12] = [
    ("lock", "Lock"),
    ("sleep", "Sleep"),
    ("hibernate", "Hibernate"),
    ("sign_out", "Sign out"),
    ("restart", "Restart"),
    ("shutdown", "Shut down"),
    ("settings", "Settings"),
    ("open_config", "Open config folder"),
    ("reload", "Reload configuration"),
    ("restart_astur", "Restart Astur"),
    ("screenshot", "Screenshot"),
    ("wallpapers", "Wallpaper"),
];

/// A built-ins list as checklist rows: the configured ones ticked, in their
/// order (old spellings renamed), then the rest of `group` unticked.
fn checklist(on: &[String], group: &[(&str, &str)]) -> Vec<(String, bool)> {
    let mut v: Vec<(String, bool)> = on
        .iter()
        .map(|s| {
            let s = s.trim().to_ascii_lowercase();
            let name = match s.as_str() {
                "signout" => "sign_out",
                "shut_down" => "shutdown",
                "wallpaper" => "wallpapers",
                n => n,
            };
            (name.to_string(), true)
        })
        .collect();
    for &(name, _) in group {
        if !v.iter().any(|i| i.0 == name) {
            v.push((name.to_string(), false));
        }
    }
    v
}

/// A launcher / power-menu target as (kind, the text after its prefix). App,
/// File and Folder are guessed from the path; they are written alike.
fn split_target(t: &str) -> (Kind, String) {
    if let Some(cmd) = t.strip_prefix("cmd:") {
        return (Kind::Command, cmd.to_string());
    }
    if let Some(url) = t.strip_prefix("url:") {
        return (Kind::Url, url.to_string());
    }
    if t.contains("://") {
        return (Kind::Url, t.to_string());
    }
    let ext = Path::new(t)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    let kind = match ext.as_deref() {
        Some("exe" | "lnk" | "bat" | "cmd" | "msc" | "appref-ms") => Kind::App,
        Some(_) => Kind::File,
        None => Kind::Folder,
    };
    (kind, t.to_string())
}

/// A web page already reading as one (`://`) stays unprefixed, as
/// split_target reads it: the WM keys launcher history and `auto` icons by
/// the target text, so adding `url:` to an untouched entry reset its rank.
fn join_target(kind: Kind, value: &str) -> String {
    match kind {
        Kind::Url if value.contains("://") => value.to_string(),
        Kind::Url => format!("url:{value}"),
        Kind::Command => format!("cmd:{value}"),
        _ => value.to_string(),
    }
}

/// `r` written as conf text and read back by the parser the WM uses comes
/// back unchanged, so the row shows exactly what Astur will read. Two
/// copies, so the record separator is in the text: a field's escapes can
/// read differently next to it than at the end of the line.
fn round_trip<T: PartialEq + Clone>(
    r: T,
    format: fn(&[T]) -> String,
    parse: fn(&str) -> Vec<T>,
) -> Result<T, String> {
    let two = [r.clone(), r.clone()];
    if parse(&format(&two)) == two {
        Ok(r)
    } else {
        Err("Can't be written as typed: check for a \\ next to a | or ;".into())
    }
}

/// A rule row as Astur will read it, or why it can't be written.
fn rule_check(r: &RuleRow) -> Result<WindowRule, String> {
    let t = |s: &str| s.trim().to_string();
    let rule = WindowRule {
        exe: t(&r.rule.exe),
        class: t(&r.rule.class),
        title: t(&r.rule.title),
        ..r.rule.clone()
    };
    if r.list {
        return match rule.class.as_str() {
            "" => Err("Type the window class".into()),
            c if c.contains([',', ';']) => Err("A class in this list can't contain , or ;".into()),
            _ => Ok(rule),
        };
    }
    // Instant apply: a new, empty row must not float every window at once.
    if rule.exe.is_empty() && rule.class.is_empty() && rule.title.is_empty() {
        return Err("Pick a window or type an app: an empty rule would match every window".into());
    }
    round_trip(rule, format_window_rules, parse_window_rules)
}

fn entry_check(e: &EntryRow) -> Result<LauncherEntry, String> {
    let (label, value) = (e.label.trim(), e.value.trim());
    if label.is_empty() {
        return Err("Type a label".into());
    }
    if value.is_empty() {
        return Err("Type what it opens".into());
    }
    let entry = LauncherEntry {
        label: label.to_string(),
        target: join_target(e.kind, value),
        icon: e.icon.trim().to_string(),
    };
    round_trip(entry, format_launcher_entries, parse_launcher_entries)
}

fn action_check(a: &ActionRow) -> Result<SystemAction, String> {
    let (label, value) = (a.label.trim(), a.value.trim());
    if label.is_empty() {
        return Err("Type a label".into());
    }
    if value.is_empty() {
        return Err("Type what it runs".into());
    }
    let action = SystemAction {
        // The parser's rule: no group = Custom.
        category: match a.group.trim() {
            "" => "Custom",
            g => g,
        }
        .to_string(),
        label: label.to_string(),
        target: join_target(a.kind, value),
        icon: a.icon.trim().to_string(),
        confirm: a.confirm,
    };
    round_trip(action, format_system_actions, parse_system_actions)
}

fn item_check((name, on): &(String, bool)) -> Result<(), String> {
    if *on && !SYSTEM_ITEMS.iter().any(|i| i.0 == name) {
        Err(format!(
            "Astur has no item \u{201C}{name}\u{201D}: untick it to drop it"
        ))
    } else {
        Ok(())
    }
}

fn hotkey_check(h: &HotkeyDef) -> Result<HotkeyDef, String> {
    // Which key MOD is doesn't matter here: every Astur key has Alt, Win or
    // Caps Lock in it.
    let key = h.chord.rsplit('+').next().unwrap_or_default();
    let Some((mods, _)) = parse_chord(&h.chord, MainKey::LAlt) else {
        return Err(match key_to_vk(key) {
            None => "Press the key for this shortcut".into(),
            Some(_) => format!("Astur can't read {}: set its key again", h.chord),
        });
    };
    // A chord compile_keys refuses under every Astur key stays a red row,
    // never written, as the old no-ALT chords did.
    if mods & (MOD_ALT | MOD_WIN | MOD_HYPER) == 0 {
        return Err(format!(
            "{} has no Astur key, Alt or Win, so it does nothing. Set its key again to add the Astur key.",
            h.chord
        ));
    }
    let Some(&(_, _, arg)) = action_info(&h.action) else {
        return Err(format!("Astur has no action \u{201C}{}\u{201D}", h.action));
    };
    let h = HotkeyDef {
        argument: h.argument.trim().to_string(),
        ..h.clone()
    };
    match arg {
        // The WM's own parse: a 1-based number.
        Arg::Workspace if !h.argument.parse::<usize>().is_ok_and(|n| n >= 1) => {
            Err("Pick a workspace number".into())
        }
        Arg::Layout if !LAYOUTS.contains(&h.argument.as_str()) => Err("Pick a layout".into()),
        Arg::Command if h.argument.is_empty() => Err("Type the command to run".into()),
        _ => round_trip(h, format_hotkeys, parse_hotkeys),
    }
}

/// Every row checked, or the first reason one can't be written.
fn all<T, R>(rows: &[T], check: fn(&T) -> Result<R, String>) -> Result<Vec<R>, String> {
    rows.iter().map(check).collect()
}

/// A workspace column (0 key, 1 name, 2 icon, 3 wallpaper) as its conf list,
/// or the first row that can't be written and why. Trailing blanks are
/// dropped. Keys, names and icons are plain lists the parser closes up, so
/// a blank before a filled cell would shift every later one up a workspace;
/// wallpapers are `;;` slots and may skip one. Only the `count` rows shown
/// are checked: a hidden one can't be fixed, so it must never hold the
/// column back. Hidden rows keep the file's values for when the count goes
/// back up, as far as they can be written in order (up to a gap or a value
/// the file can't take); past that they are dropped. ponytail: a `;;` slot
/// list for names, icons and keys in astur-config would allow gaps.
fn ws_column(rows: &[WsRow], col: usize, count: usize) -> Result<Vec<String>, (usize, String)> {
    let cells: Vec<&str> = rows
        .iter()
        .map(|r| [&r.key, &r.name, &r.icon, &r.wallpaper][col].trim())
        .collect();
    let count = count.min(cells.len());
    let filled = |cells: &[&str]| {
        cells
            .iter()
            .rposition(|s| !s.is_empty())
            .map_or(0, |p| p + 1)
    };
    let len = filled(&cells[..count]);
    let what = ["key", "name", "icon", "wallpaper"][col];
    let why = |s: &str| {
        match col {
        0..=2 if s.is_empty() => Some(format!("No {what}: the {what}s after it would move up a workspace (the file can't skip one). Set one, or clear the later ones.")),
        0 if key_to_vk(s).is_none() => Some(format!("{s} is not a key Astur can use here")),
        1 | 2 if s.contains([',', ';']) => Some(format!("A {what} can't contain , or ;")),
        3 if s.contains(";;") => Some("A wallpaper path can't contain ;;".to_string()),
        _ => None,
    }
    };
    if let Some((i, why)) = (cells[..len].iter().enumerate()).find_map(|(i, s)| Some((i, why(s)?)))
    {
        return Err((i, why));
    }
    if col == 0 && len == 0 {
        return Err((
            0,
            "Workspace 1 needs a key: with none, Astur goes back to 1 2 3 ... 0.".into(),
        ));
    }
    // Names, keys and icons can't skip a blank shown row to reach the
    // hidden ones; wallpaper slots can.
    let end = if len == count || col == 3 {
        count
            + cells[count..]
                .iter()
                .take_while(|s| why(s).is_none())
                .count()
    } else {
        len
    };
    Ok(cells[..filled(&cells[..end])]
        .iter()
        .map(|s| s.to_string())
        .collect())
}

/// Each shown workspace row's reasons, if any column stops at it.
fn ws_problems(rows: &[WsRow], count: usize) -> Vec<Option<String>> {
    let mut out: Vec<Option<String>> = vec![None; rows.len().max(1)];
    for col in 0..4 {
        if let Err((i, why)) = ws_column(rows, col, count) {
            if let Some(slot) = out.get_mut(i) {
                let p = slot.get_or_insert_with(String::new);
                if !p.is_empty() {
                    p.push(' ');
                }
                p.push_str(&why);
            }
        }
    }
    out
}

// The tables into the Config, each only while every row in it is valid.

fn fold_keys(keys: &[String; 12], c: &mut Config) {
    for (slot, k) in binds_mut(c).into_iter().zip(keys) {
        if let Some(vk) = key_to_vk(k) {
            *slot = vk;
        }
    }
}

/// Per column: a bad name does not hold back a key change.
fn fold_ws(rows: &[WsRow], c: &mut Config) {
    let n = c.workspaces;
    if let Ok(keys) = ws_column(rows, 0, n) {
        c.workspace_keys = keys.iter().filter_map(|k| key_to_vk(k)).collect();
    }
    for (col, field) in [
        (1, &mut c.workspace_names),
        (2, &mut c.workspace_icons),
        (3, &mut c.workspace_wallpapers),
    ] {
        if let Ok(v) = ws_column(rows, col, n) {
            *field = v;
        }
    }
}

fn fold_rules(rows: &[RuleRow], c: &mut Config) {
    let Ok(rules) = all(rows, rule_check) else {
        return;
    };
    let pick = |list: bool, action: &str| -> Vec<&WindowRule> {
        (rules.iter().zip(rows))
            .filter(|(r, row)| row.list == list && (!list || r.action == action))
            .map(|(r, _)| r)
            .collect()
    };
    c.window_rules = pick(false, "").into_iter().cloned().collect();
    c.ignore_classes = pick(true, "ignore")
        .iter()
        .map(|r| r.class.clone())
        .collect();
    c.float_classes = pick(true, "float")
        .iter()
        .map(|r| r.class.clone())
        .collect();
}

fn fold_entries(rows: &[EntryRow], c: &mut Config) {
    if let Ok(v) = all(rows, entry_check) {
        c.launcher_entries = v;
    }
}

fn fold_actions(rows: &[ActionRow], c: &mut Config) {
    if let Ok(v) = all(rows, action_check) {
        c.system_actions = v;
    }
}

fn fold_items(items: &[(String, bool)], field: &mut Vec<String>) {
    if all(items, item_check).is_ok() {
        *field = items.iter().filter(|i| i.1).map(|i| i.0.clone()).collect();
    }
}

fn fold_hotkeys(rows: &[HotkeyDef], c: &mut Config) {
    if let Ok(v) = all(rows, hotkey_check) {
        c.extra_hotkeys = v;
    }
}

/// Every key the shipped templates set, or show commented out as `#key = v` /
/// `# key = v`: what the parser understands, minus old aliases. Scanned, not
/// listed, so it cannot drift from the templates. Indented comment lines
/// (`#   off = write nothing`) are value tables, and a commented key must have
/// an underscore (`# icon = label only` is prose), so neither counts.
fn template_keys(text: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for line in text.lines() {
        let (line, commented) = match line.strip_prefix('#') {
            Some(rest) => (rest, true),
            None => (line, false),
        };
        if commented && line.starts_with("  ") {
            continue;
        }
        let Some((k, _)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim();
        let ident = !k.is_empty()
            && k.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        if ident && (!commented || k.contains('_')) && !keys.iter().any(|x| x == k) {
            keys.push(k.to_string());
        }
    }
    keys
}

/// [astur.conf keys, navbar.conf keys], scanned once.
fn known_keys() -> &'static [Vec<String>; 2] {
    static KEYS: std::sync::OnceLock<[Vec<String>; 2]> = std::sync::OnceLock::new();
    KEYS.get_or_init(|| {
        [
            template_keys(default_config_text()),
            template_keys(default_navbar_text()),
        ]
    })
}

/// Which file a row's key lives in, for its hover text: the file the GUI
/// writes it to (the template leaves some navbar keys, `shape`, commented out
/// with no underscore, so the scan above skips them).
fn conf_file(key: &str) -> &'static str {
    static NAV: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    let nav = NAV.get_or_init(|| {
        conf_pairs(&Config::defaults())
            .1
            .iter()
            .map(|p| p.0)
            .collect()
    });
    let first = key.split([',', ' ']).next().unwrap_or(key);
    if nav.contains(&first) || known_keys()[1].iter().any(|k| k == first) {
        "navbar.conf"
    } else {
        "astur.conf"
    }
}

/// A row's hover text: its keys under the file each lives in. One row can
/// write both files (Status bar: `enabled` and astur.conf's `yasb_enabled`),
/// and a key put in the wrong file overrides what Settings writes.
fn key_files(keys: &str) -> String {
    let mut groups: Vec<(&str, Vec<&str>)> = Vec::new();
    for k in keys.split([',', ' ']).filter(|k| !k.is_empty()) {
        let file = conf_file(k);
        match groups.iter_mut().find(|g| g.0 == file) {
            Some(g) => g.1.push(k),
            None => groups.push((file, vec![k])),
        }
    }
    let groups: Vec<String> = (groups.iter())
        .map(|(file, keys)| format!("{file} \u{00B7} {}", keys.join(", ")))
        .collect();
    groups.join("; ")
}

/// Optimal string alignment distance: edits, with a swap of two neighbours
/// counted as one (`heigth` is one typo from `height`).
fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

/// The known key an unknown one most likely meant: within a quarter of its
/// length in edits (1 to 3), case ignored. Old configs write bar keys with a
/// `bar_` prefix (`bar_heigth`); the navbar name works in either file.
fn did_you_mean(key: &str) -> Option<&'static str> {
    let lower = key.to_ascii_lowercase();
    let best = |form: &str| {
        let limit = (form.len() / 4).clamp(1, 3);
        known_keys()
            .iter()
            .flatten()
            .filter(|k| k.as_str() != key)
            .map(|k| (edit_distance(form, k), k))
            .filter(|(d, _)| *d <= limit)
            .min_by_key(|(d, _)| *d)
            .map(|(_, k)| k.as_str())
    };
    best(&lower).or_else(|| lower.strip_prefix("bar_").and_then(best))
}

/// The value of the last active `key = ...` line in `text`: the one read.
fn line_value(text: &str, key: &str) -> Option<String> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    text.lines().rev().find_map(|line| {
        let t = line.trim();
        let (k, v) = t.split_once('=')?;
        (!t.starts_with('#') && k.trim() == key).then(|| v.trim().to_string())
    })
}

/// `text` without its active `key = ...` lines. With `to`, the last value
/// moves to that key through `set_conf_key` (in place of an existing `to` line,
/// later duplicates dropped, else appended), so the renamed line wins.
fn edit_key(text: &str, key: &str, to: Option<&str>) -> String {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let value = line_value(text, key);
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| line_value(line, key).is_none())
        .collect();
    let mut out = kept.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    match (to, value) {
        (Some(to), Some(v)) => set_conf_key(&out, to, &v),
        _ => out,
    }
}

/// Fold the text mirrors back into the Config (dropping invalid tokens) so
/// what gets written is exactly what the WM will parse. The tables' rows
/// are folded as they change (`fold_*`), never dropped here.
fn fold_mirrors(c: &mut Config, m: &mut Mirrors) {
    c.launcher_file_exclude = (m.launcher_excludes.split([',', ';']))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    trim_defaults(&mut c.monitor_layouts);
    // Normalise the text mirrors to what survived validation; rows stay.
    *m = Mirrors {
        rows: std::mem::take(&mut m.rows),
        ..Mirrors::from(c)
    };
}

/// The pairs whose text differs (ignoring surrounding spaces, which the parser
/// trims), per file. Writing every key reverted edits made to the files while
/// Settings was open, froze today's defaults in, and appended ~150 lines to a
/// hand-written config.
fn diff(new: &Config, old: &Config) -> (Pairs, Pairs) {
    let (wm, nav) = conf_pairs(new);
    let (old_wm, old_nav) = conf_pairs(old);
    // Same keys in the same order on both sides: one function built them.
    let changed = |new: Pairs, old: Pairs| -> Pairs {
        new.into_iter()
            .zip(old)
            .filter(|(n, o)| n.1.trim() != o.1.trim())
            .map(|(n, _)| n)
            .collect()
    };
    (changed(wm, old_wm), changed(nav, old_nav))
}

/// Every key the GUI edits as conf text: (astur.conf, navbar.conf).
fn conf_pairs(c: &Config) -> (Pairs, Pairs) {
    let b = |v: bool| if v { "true" } else { "false" }.to_string();
    let ws_keys: Vec<String> = c.workspace_keys.iter().map(|&k| vk_to_key(k)).collect();
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
        ("workspace_keys", ws_keys.join(" ")),
        ("workspace_names", c.workspace_names.join(", ")),
        ("workspace_icons", c.workspace_icons.join(", ")),
        ("workspace_wallpapers", slots(&c.workspace_wallpapers)),
        ("start_tiled", b(c.start_tiled)),
        ("layout", c.layout.clone()),
        ("monitor_layouts", c.monitor_layouts.join(", ")),
        ("master_ratio", format!("{:.2}", c.master_ratio)),
        (
            "scroll_default_width",
            format!("{:.2}", c.scroll_default_width),
        ),
        ("scroll_width_presets", fractions(&c.scroll_width_presets)),
        ("scroll_height_presets", fractions(&c.scroll_height_presets)),
        ("scroll_center", c.scroll_center.clone()),
        ("tab_bar_height", c.tab_bar_height.to_string()),
        ("outer_gap", c.outer_gap.to_string()),
        ("inner_gap", c.inner_gap.to_string()),
        ("cursor_follows_focus", b(c.cursor_follows_focus)),
        ("focus_follows_mouse", b(c.focus_follows_mouse)),
        ("animations", b(c.animations)),
        ("animation_ms", c.animation_ms.to_string()),
        ("workspace_slide", b(c.workspace_slide)),
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
        ("ipc_allow_launch", b(c.ipc_allow_launch)),
        ("yasb_enabled", b(c.yasb_enabled)),
        ("yasb_port", c.yasb_port.to_string()),
        ("foreground_lock_disable", b(c.foreground_lock_disable)),
        ("async_window_pos", b(c.async_window_pos)),
        ("persist_state", b(c.persist_state)),
        ("log_level", c.log_level.clone()),
        ("main_key", c.main_key.name().to_string()),
        ("extra_hotkeys", format_hotkeys(&c.extra_hotkeys)),
        ("key_focus_next", vk_to_key(c.key_focus_next)),
        ("key_focus_prev", vk_to_key(c.key_focus_prev)),
        ("key_shrink_master", vk_to_key(c.key_shrink_master)),
        ("key_grow_master", vk_to_key(c.key_grow_master)),
        ("key_promote_master", vk_to_key(c.key_promote_master)),
        ("key_toggle_tiling", vk_to_key(c.key_toggle_tiling)),
        ("key_toggle_float", vk_to_key(c.key_toggle_float)),
        ("key_close_window", vk_to_key(c.key_close_window)),
        ("key_cycle_width", vk_to_key(c.key_cycle_width)),
        ("key_center_column", vk_to_key(c.key_center_column)),
        ("key_consume_left", vk_to_key(c.key_consume_left)),
        ("key_consume_right", vk_to_key(c.key_consume_right)),
    ];
    let mut nav_updates: Vec<(&str, String)> = vec![
        ("enabled", b(c.bar_enabled)),
        ("height", c.bar_height.to_string()),
        ("bottom", b(c.bar_bottom)),
        ("padding", c.bar_padding.to_string()),
        ("font_name", c.bar_font_name.clone()),
        ("font_size", c.bar_font_size.to_string()),
        // `shape`, never the older `floating`: a shape line wins in the WM
        // whatever the order, so a written `floating` would not stick. Empty
        // (Undo back to a file without one) reads as no line: floating decides.
        ("shape", c.bar_shape.clone()),
        ("palette", c.bar_palette.clone()),
        ("workspace_style", c.bar_workspace_style.clone()),
        ("font_weight", c.bar_font_weight.to_string()),
        ("border_width", c.bar_border_width.to_string()),
        ("island_padding", c.bar_island_padding.to_string()),
        ("widget_colors", widget_colors_text(&c.bar_widget_colors)),
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
        ("accent_fg", opt_hex(c.bar_accent_fg)),
        ("surface", opt_hex(c.bar_surface)),
        ("border", opt_hex(c.bar_border)),
        ("separator_style", c.bar_separator_style.clone()),
        ("hover", b(c.bar_hover)),
        ("title_format", c.bar_title_format.clone()),
        ("title_bold", b(c.bar_title_bold)),
        ("workspace_label", c.bar_workspace_label.clone()),
        ("workspace_apps", b(c.bar_workspace_apps)),
        ("monitors", c.bar_monitors.clone()),
        ("keyboard_format", c.bar_keyboard_format.clone()),
        ("disk_drive", c.bar_disk_drive.clone()),
        ("disk_format", c.bar_disk_format.clone()),
        ("icon_disk", c.bar_icon_disk.clone()),
        ("uptime_format", c.bar_uptime_format.clone()),
        ("custom_command", c.bar_custom_command.clone()),
        ("custom_format", c.bar_custom_format.clone()),
        ("custom_interval", c.bar_custom_interval.to_string()),
    ];
    // A click line holds the user's pick; empty (no line, or one emptied to
    // go back to the default) while it is the default.
    for (w, keys) in BAR_CLICK_WIDGETS.iter().zip(click_keys()) {
        for (b, key) in keys.iter().enumerate() {
            let v = c
                .bar_clicks
                .iter()
                .rfind(|x| x.0 == *w && usize::from(x.1) == b);
            nav_updates.push((key.as_str(), v.map_or(String::new(), |x| x.2.clone())));
        }
    }
    (wm_updates, nav_updates)
}

/// Each BAR_CLICK_WIDGETS entry's keys, [click, right_click, middle_click]:
/// built once, as conf_pairs' keys are 'static.
fn click_keys() -> &'static [[String; 3]] {
    static KEYS: std::sync::OnceLock<Vec<[String; 3]>> = std::sync::OnceLock::new();
    KEYS.get_or_init(|| {
        (BAR_CLICK_WIDGETS.iter())
            .map(|w| ["click", "right_click", "middle_click"].map(|b| format!("{w}_{b}")))
            .collect()
    })
}

/// What `button` (0 left, 1 right, 2 middle) on bar widget `w` does, as
/// `action [argument]`: the user's line, else the default.
fn click_now(c: &Config, w: &str, button: usize) -> String {
    match c.bar_click(w, button) {
        (a, "") => a.to_string(),
        (a, arg) => format!("{a} {arg}"),
    }
}

/// The action a click has with no line for it.
fn click_default(w: &str, button: usize) -> &'static str {
    (BAR_CLICK_DEFAULTS.iter())
        .find(|d| d.0 == w)
        .map_or("none", |d| d.1[button])
}

/// Set what a click does (`action [argument]`). The default is kept as no
/// entry, so picking it back writes nothing new (an existing line is
/// emptied); a changed entry keeps its place, so its line stays put.
fn set_click(c: &mut Config, w: &str, button: usize, v: &str) {
    let at = (c.bar_clicks.iter()).position(|x| x.0 == w && usize::from(x.1) == button);
    match (at, v == click_default(w, button)) {
        (Some(i), true) => {
            c.bar_clicks.remove(i);
        }
        (Some(i), false) => c.bar_clicks[i].2 = v.to_string(),
        (None, true) => {}
        (None, false) => (c.bar_clicks).push((w.to_string(), button as u8, v.to_string())),
    }
}

/// `widget_colors` as conf text, `widget:colour` pairs in the parser's
/// (lower-case) form, so a written value reads back the same.
fn widget_colors_text(v: &[(String, String)]) -> String {
    let pairs: Vec<String> = v.iter().map(|(w, c)| format!("{w}:{c}")).collect();
    pairs.join(" ")
}

/// One file's `(key, value)` updates, as `apply_updates` takes them.
type Pairs = Vec<(&'static str, String)>;

/// `workspace_wallpapers` as conf text. The parser splits on `;;`, or on
/// commas when there is none, so a single path with a comma in it gets a
/// trailing `;;` (an empty last slot changes nothing).
fn slots(v: &[String]) -> String {
    let mut s = v.join(" ;; ");
    if v.len() == 1 && s.contains(',') {
        s.push_str(" ;;");
    }
    s
}

/// Scrolling preset list to conf text, in the template's `0.5, 1` style.
fn fractions(v: &[f32]) -> String {
    v.iter().map(f32::to_string).collect::<Vec<_>>().join(", ")
}

/// Drop trailing "(default)" monitor entries: `scrolling, ,` means the same as
/// `scrolling`, and keeping the canonical form stops a no-op edit reading dirty.
fn trim_defaults(v: &mut Vec<String>) {
    while v.last().is_some_and(String::is_empty) {
        v.pop();
    }
}

/// The plain name of a layout from `LAYOUTS` (the conf name stays in the
/// hover text): "dwindle" and "master" mean nothing to most people.
fn layout_label(name: &str) -> &str {
    match name {
        "dwindle" => "Spiral",
        "master" => "Main + side",
        "columns" => "Columns",
        "grid" => "Grid",
        "monocle" => "One at a time",
        "scrolling" => "Scrolling",
        _ => name,
    }
}

/// What a layout does, in a sentence, for its thumbnail's hover text.
fn layout_help(name: &str) -> &str {
    match name {
        "dwindle" => "Spiral: each new window takes half of the space of the one before it.",
        "master" => "Main + side: one big window on the left, the others stacked on the right.",
        "columns" => "Columns: every window side by side, equally wide.",
        "grid" => "Grid: the windows in an even grid.",
        "monocle" => {
            "One at a time: each window fills the screen; the focus keys step through them."
        }
        "scrolling" => {
            "Scrolling: windows in columns on a strip wider than the screen, scrolled into view."
        }
        _ => name,
    }
}

/// Themeable colour to conf text: explicit hex, or `auto` (follow the theme).
fn opt_hex(c: Option<u32>) -> String {
    c.map(color_to_hex).unwrap_or_else(|| "auto".to_string())
}

/// Read one config file. A missing file reads as the built-in commented
/// template, so a fresh install edits (and writes) a fully documented file. A
/// file that exists but cannot be read (UTF-16 from Notepad, locked by an
/// editor mid-save) is an error, never a reason to write the template over it,
/// which the old fallback did on Save. A lock clears in milliseconds, so retry
/// briefly first, as the WM's startup load does.
fn read_conf(path: &Path, template: &str) -> Result<String, astur_config::ConfigReadError> {
    let mut text = astur_config::read_existing(path);
    for _ in 0..3 {
        if text.is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
        text = astur_config::read_existing(path);
    }
    text.map(|t| t.unwrap_or_else(|| template.to_string()))
}

fn read_pair(paths: &[PathBuf; 2]) -> Result<Config, String> {
    let wm = read_conf(&paths[0], default_config_text()).map_err(|e| e.to_string())?;
    let nav = read_conf(&paths[1], default_navbar_text()).map_err(|e| e.to_string())?;
    Ok(parse_pair(&wm, &nav))
}

/// Taken BEFORE reading: a change racing the read then shows up as a newer
/// mtime at the next write (a harmless extra merge), never as a missed one.
fn mtimes(paths: &[PathBuf; 2]) -> [Option<SystemTime>; 2] {
    paths
        .each_ref()
        .map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
}

/// Replace a config file whole or not at all. `std::fs::write` truncates
/// first, so a full disk or a kill mid-write left the user's file empty, and
/// instant apply writes on every finished edit. A sibling temp file, flushed,
/// then renamed over it (MoveFileExW with REPLACE_EXISTING: atomic on one
/// volume). The real path, so a symlinked conf stays a symlink; on any
/// failure the temp goes and the original is untouched.
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let r = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut f, text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, &path)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

/// A slider that clamps only what the user drags or types. egui's default
/// (`SliderClamping::Always`) clamps, and rounds to `fixed_decimals`, the
/// existing value every time the slider is drawn, and most ranges here are
/// narrower than the parser's: merely opening a page rewrote `outer_gap = 120`
/// to 100. ponytail: focusing the number box and leaving it still clamps;
/// shared parser/GUI range consts would end that.
fn slider<N: egui::emath::Numeric>(
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
) -> egui::Slider<'_> {
    egui::Slider::new(value, range).clamping(egui::SliderClamping::Edits)
}

// ---- status bar ----------------------------------------------------------------

/// Every bar widget (`BAR_WIDGETS`), in the order the list of widgets not on
/// the bar shows them, separator and spacer last: (name, chip label, what it
/// shows).
const WIDGETS: [(&str, &str, &str); 20] = [
    (
        "workspaces",
        "Workspaces",
        "A mark per workspace; click one to switch.",
    ),
    (
        "apps",
        "App buttons",
        "An icon per window on the workspace; click one to focus it.",
    ),
    ("title", "Window title", "The focused window's title."),
    (
        "layout",
        "Layout",
        "The layout's name and the tiling state.",
    ),
    ("cpu", "CPU", "Processor load."),
    ("mem", "Memory", "Memory in use."),
    ("net", "Network", "Download and upload speed."),
    (
        "volume",
        "Volume",
        "The wheel over it changes the volume; a click mutes.",
    ),
    (
        "battery",
        "Battery",
        "Charge left; red when low and unplugged.",
    ),
    ("date", "Date", "Today's date."),
    ("clock", "Clock", "The time."),
    (
        "media",
        "Now playing",
        "The title from known media players.",
    ),
    (
        "media_controls",
        "Music controls",
        "Previous, play or pause, and next buttons for the music playing.",
    ),
    (
        "keyboard",
        "Keyboard language",
        "Shows ENG, DEU\u{2026}; click to switch.",
    ),
    ("disk", "Disk space", "The space left on a drive."),
    ("uptime", "Uptime", "Time since Windows started."),
    (
        "custom",
        "Custom text",
        "Your own text, or the first line a command prints.",
    ),
    ("logo", "Astur logo", "Click it for the power menu."),
    (
        "separator",
        "Separator",
        "A thin line between widgets; use as many as you like.",
    ),
    (
        "spacer",
        "Spacer",
        "Extra room between widgets; use as many as you like.",
    ),
];

const ZONES: [&str; 3] = ["Left", "Centre", "Right"];

/// Every conf key the zone editor writes: the zones, and the switches that
/// placing or removing a widget sets.
const ZONE_KEYS: &str = "left, center, right, show_title, show_layout, show_apps, show_cpu, show_mem, show_net, show_volume, show_battery, show_date, show_clock, show_media, media_enabled";

/// A widget chip: the `i`-th widget zone `z` shows, or one in the list of
/// widgets not on the bar. The drag-and-drop payload, and the selection.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Src {
    Zone(usize, usize),
    Palette(&'static str),
}

/// (chip label, what it shows) for a widget name.
fn widget_info(w: &str) -> (&'static str, &'static str) {
    WIDGETS
        .iter()
        .find(|x| x.0 == w)
        .map_or(("?", ""), |x| (x.1, x.2))
}

fn zone(c: &Config, z: usize) -> &Vec<String> {
    match z {
        0 => &c.bar_left,
        1 => &c.bar_center,
        _ => &c.bar_right,
    }
}

fn zone_mut(c: &mut Config, z: usize) -> &mut Vec<String> {
    match z {
        0 => &mut c.bar_left,
        1 => &mut c.bar_center,
        _ => &mut c.bar_right,
    }
}

/// Whether a listed widget draws: the WM needs its show_ switch on too
/// (zone_widgets), and media also needs media_enabled (MEDIA_ON). Workspaces,
/// separators, spacers and the widgets added since show_* (keyboard, disk,
/// uptime, custom, media_controls, logo) have no switch: listed is shown.
fn widget_on(c: &Config, w: &str) -> bool {
    match w {
        "title" => c.bar_show_title,
        "layout" => c.bar_show_layout,
        "apps" => c.bar_show_apps,
        "cpu" => c.bar_show_cpu,
        "mem" => c.bar_show_mem,
        "net" => c.bar_show_net,
        "volume" => c.bar_show_volume,
        "battery" => c.bar_show_battery,
        "date" => c.bar_show_date,
        "clock" => c.bar_show_clock,
        "media" => c.bar_show_media && c.media_enabled,
        _ => true,
    }
}

fn set_widget_on(c: &mut Config, w: &str, on: bool) {
    let flag = match w {
        "title" => &mut c.bar_show_title,
        "layout" => &mut c.bar_show_layout,
        "apps" => &mut c.bar_show_apps,
        "cpu" => &mut c.bar_show_cpu,
        "mem" => &mut c.bar_show_mem,
        "net" => &mut c.bar_show_net,
        "volume" => &mut c.bar_show_volume,
        "battery" => &mut c.bar_show_battery,
        "date" => &mut c.bar_show_date,
        "clock" => &mut c.bar_show_clock,
        "media" => {
            c.media_enabled = on;
            &mut c.bar_show_media
        }
        _ => return,
    };
    *flag = on;
}

/// The widgets zone `z` draws, in order: its chips.
fn shown(c: &Config, z: usize) -> Vec<String> {
    zone(c, z)
        .iter()
        .filter(|w| widget_on(c, w))
        .cloned()
        .collect()
}

/// The chips under the zones: every widget no zone shows, with whether a
/// zone lists it switched off (shown as hidden). Separators and spacers can
/// repeat, so they are always there.
fn palette_chips(c: &Config) -> Vec<(&'static str, bool)> {
    let on: Vec<String> = (0..3).flat_map(|z| shown(c, z)).collect();
    WIDGETS
        .iter()
        .map(|w| w.0)
        .filter(|w| matches!(*w, "separator" | "spacer") || !on.iter().any(|x| x == w))
        .map(|w| {
            let listed = (0..3).any(|z| zone(c, z).iter().any(|x| x == w));
            (w, listed && !widget_on(c, w))
        })
        .collect()
}

/// The zone editor's one visibility rule: a widget in a zone shows, one in no
/// zone does not (the WM's "listed AND switched on" stays true underneath).
/// Moves chip `from` into zone `to.0` before its `to.1`-th chip (past the end
/// appends), or off the bar (`to` None). The zones it touches are first cut
/// down to what they draw: a listed-but-off widget drew nothing there, and
/// would start to once switched on. For the same reason placing a widget
/// drops its off listings everywhere before switching it on (media turns on
/// media_enabled too); taking one off switches it off unless another zone
/// still shows it. Zones it does not touch keep their text. Returns the
/// chip's new place.
fn move_widget(c: &mut Config, from: Src, to: Option<(usize, usize)>) -> Option<Src> {
    if to.is_some_and(|t| t.0 > 2) {
        return None;
    }
    let w = match from {
        Src::Zone(z, i) => shown(c, z).get(i)?.clone(),
        Src::Palette(w) => w.to_string(),
    };
    if matches!(from, Src::Palette(_)) && !widget_on(c, &w) {
        for z in 0..3 {
            zone_mut(c, z).retain(|x| *x != w);
        }
    }
    let from_zone = match from {
        Src::Zone(z, _) => Some(z),
        Src::Palette(_) => None,
    };
    for z in [from_zone, to.map(|t| t.0)].into_iter().flatten() {
        *zone_mut(c, z) = shown(c, z);
    }
    if let Src::Zone(z, i) = from {
        zone_mut(c, z).remove(i);
    }
    let Some((z, at)) = to else {
        if !(0..3).any(|z| zone(c, z).contains(&w)) {
            set_widget_on(c, &w, false);
        }
        return None;
    };
    // Its own old place counted among the chips before the pointer.
    let at = match from {
        Src::Zone(fz, i) if fz == z && at > i => at - 1,
        _ => at,
    };
    let list = zone_mut(c, z);
    let at = at.min(list.len());
    list.insert(at, w.clone());
    set_widget_on(c, &w, true);
    Some(Src::Zone(z, at))
}

/// A widget chip: dragged to move it, clicked to open its settings.
/// `hidden`: listed in navbar.conf but switched off, so it draws nothing.
fn chip(ui: &mut Ui, src: Src, w: &str, selected: bool, hidden: bool) -> egui::Response {
    let (label, desc) = widget_info(w);
    let (edge, hover, fill, text) = {
        let v = ui.visuals();
        let text = if hidden { sub(ui) } else { v.text_color() };
        let edge = if selected {
            Stroke::new(1.5_f32, accent(ui))
        } else {
            v.widgets.noninteractive.bg_stroke
        };
        (
            edge,
            v.widgets.hovered.bg_fill,
            v.widgets.inactive.bg_fill,
            text,
        )
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), FontId::proportional(13.0), text);
    let size = galley.size() + egui::vec2(20.0, 10.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    resp.dnd_set_drag_payload(src);
    let fill = match (hidden, resp.hovered()) {
        (true, _) => Color32::TRANSPARENT,
        (false, true) => hover,
        (false, false) => fill,
    };
    let paint = |p: &egui::Painter, r: egui::Rect| {
        p.rect(r, 6.0, fill, edge, egui::StrokeKind::Inside);
        p.galley(r.center() - galley.size() / 2.0, galley.clone(), text);
    };
    if resp.dragged() {
        // An outline keeps its place; the chip itself follows the pointer.
        ui.painter()
            .rect_stroke(rect, 6.0, edge, egui::StrokeKind::Inside);
        if let Some(pos) = ui.ctx().pointer_interact_pos() {
            let layer = egui::LayerId::new(egui::Order::Tooltip, resp.id);
            paint(
                &ui.ctx().layer_painter(layer),
                egui::Rect::from_center_size(pos, size),
            );
        }
    } else {
        paint(ui.painter(), rect);
    }
    let tip = if hidden {
        "Listed in navbar.conf but switched off, so it isn't drawn. Drag it into a zone to show it."
    } else {
        desc
    };
    resp.on_hover_text(format!("{tip} Drag to move it; click for its settings."))
}

/// The three zones side by side, as drop targets holding their widgets'
/// chips, then the widgets not on the bar. Returns true when they changed.
fn zone_editor(ui: &mut Ui, c: &mut Config, sel: &mut Option<Src>) -> bool {
    let mut moved: Option<(Src, Option<(usize, usize)>)> = None;
    let mut click = None;
    let pointer = ui.ctx().pointer_interact_pos();
    let (field, edge, drop_edge) = {
        let v = ui.visuals();
        (
            v.extreme_bg_color,
            v.widgets.noninteractive.bg_stroke,
            Stroke::new(1.5_f32, accent(ui)),
        )
    };
    let frame = egui::Frame::new()
        .fill(field)
        .stroke(edge)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(6));
    ui.columns(3, |cols| {
        for (z, ui) in cols.iter_mut().enumerate() {
            ui.label(RichText::new(ZONES[z]).small().color(sub(ui)));
            let mut rects = Vec::new();
            let resp = frame
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(RESET_SLOT + 6.0);
                    ui.horizontal_wrapped(|ui| {
                        let chips = shown(c, z);
                        for (i, w) in chips.iter().enumerate() {
                            let src = Src::Zone(z, i);
                            let r = chip(ui, src, w, *sel == Some(src), false);
                            if r.clicked() {
                                click = Some(src);
                            }
                            rects.push(r.rect);
                        }
                        if chips.is_empty() {
                            ui.label(RichText::new("Empty").small().color(sub(ui)));
                        }
                    });
                })
                .response;
            if resp.dnd_hover_payload::<Src>().is_some() {
                ui.painter()
                    .rect_stroke(resp.rect, 6.0, drop_edge, egui::StrokeKind::Inside);
            }
            if let (Some(src), Some(p)) = (resp.dnd_release_payload::<Src>(), pointer) {
                // Before every chip that starts left of the pointer on its
                // line, or sits on a line above it.
                let at = rects
                    .iter()
                    .filter(|r| r.bottom() < p.y || (r.top() <= p.y && r.center().x < p.x))
                    .count();
                moved = Some((*src, Some((z, at))));
            }
        }
    });
    ui.add_space(6.0);
    ui.label(RichText::new("Not on the bar").small().color(sub(ui)));
    let resp = frame
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                for (w, hidden) in palette_chips(c) {
                    let src = Src::Palette(w);
                    if chip(ui, src, w, *sel == Some(src), hidden).clicked() {
                        click = Some(src);
                    }
                }
            });
        })
        .response;
    if resp.dnd_hover_payload::<Src>().is_some() {
        ui.painter()
            .rect_stroke(resp.rect, 6.0, drop_edge, egui::StrokeKind::Inside);
    }
    if let Some(src) = resp.dnd_release_payload::<Src>() {
        if matches!(*src, Src::Zone(..)) {
            moved = Some((*src, None));
        }
    }
    if let Some(src) = click {
        *sel = (*sel != Some(src)).then_some(src);
    }
    match moved {
        Some((from, to)) => {
            *sel = move_widget(c, from, to);
            true
        }
        None => false,
    }
}

/// The zone editor as a settings row; reset puts back the default zones and
/// switches.
fn zone_row(ui: &mut Ui, f: &mut Find, c: &mut Config, d: &Config, sel: &mut Option<Src>) {
    let drawn = |c: &Config| (0..3).map(|z| shown(c, z)).collect::<Vec<_>>();
    let meta = (
        "Zones",
        "What the bar shows, left to right in each zone.",
        ZONE_KEYS,
    );
    if row_core(ui, f, meta, drawn(c) != drawn(d), true, |ui| {
        zone_editor(ui, c, sel);
    }) {
        for z in 0..3 {
            zone_mut(c, z).clone_from(zone(d, z));
        }
        for &(w, ..) in &WIDGETS {
            set_widget_on(c, w, widget_on(d, w));
        }
        *sel = None;
    }
}

/// The selected chip's card (what it is, with take-off / add buttons, so the
/// zones work without dragging too), then its settings.
fn widget_panel(ui: &mut Ui, f: &mut Find, c: &mut Config, d: &Config, sel: &mut Option<Src>) {
    let w = match *sel {
        Some(Src::Zone(z, i)) => shown(c, z).get(i).cloned(),
        Some(Src::Palette(w)) => palette_chips(c)
            .iter()
            .any(|p| p.0 == w)
            .then(|| w.to_string()),
        None => None,
    };
    // An Undo or a reset can leave the selection pointing at nothing.
    let (Some(src), Some(w)) = (*sel, w) else {
        *sel = None;
        return;
    };
    let (label, desc) = widget_info(&w);
    let mut to = None;
    card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(label).family(semibold()));
            ui.label(RichText::new(desc).color(sub(ui)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| match src {
                Src::Zone(..) => {
                    if ui.button("Take off the bar").clicked() {
                        to = Some(None);
                    }
                }
                Src::Palette(_) => {
                    for (z, name) in ZONES.iter().enumerate().rev() {
                        if ui.button(*name).clicked() {
                            to = Some(Some((z, usize::MAX)));
                        }
                    }
                    ui.label("Add to");
                }
            });
        });
    });
    if let Some(to) = to {
        *sel = move_widget(c, src, to);
    }
    widget_rows(ui, f, c, d, &w);
}

/// Sample readings for the previews: (widget, values, the glyph the glyph
/// icon modes draw for them: the WM's stat_glyph for 60 % volume, 87 %
/// battery on battery power, wifi). Disk sizes as the WM's fmt_size writes
/// them, the keyboard as kbd_names (ISO 639-2 language, ISO 3166 country),
/// uptime as fmt_uptime.
const READINGS: [Reading; 8] = [
    ("cpu", &[("value", "23")], '\u{EEA1}'),
    ("mem", &[("value", "41")], '\u{EEA0}'),
    ("net", &[("down", "1.2M"), ("up", "48K")], '\u{E701}'),
    ("volume", &[("value", "60")], '\u{E994}'),
    ("battery", &[("value", "87")], '\u{E859}'),
    (
        "disk",
        &[
            ("free", "120.50 GB"),
            ("used", "355.30 GB"),
            ("total", "475.80 GB"),
            ("value", "75"),
        ],
        '\u{EDA2}',
    ),
    ("keyboard", &[("lang", "ENG"), ("region", "US")], '\u{E765}'),
    ("uptime", &[("value", "3d 4h")], '\u{E823}'),
];

/// (widget, its sample values by token, its glyph).
type Reading = (&'static str, &'static [(&'static str, &'static str)], char);

/// `fmt` with each `{token}` replaced by its value (the WM's widget_format).
fn fill_tokens(fmt: &str, values: &[(&str, &str)]) -> String {
    (values.iter()).fold(fmt.to_string(), |t, (k, v)| {
        t.replace(&format!("{{{k}}}"), v)
    })
}

/// A stat widget's text for the READINGS sample, built as the WM's
/// bar_widget_text builds it (widget_format, then widget_icon_text); under
/// the glyph icon modes the label gives way to the glyph returned with it,
/// and `glyph_only` drops the value too. `drawable`: the icon font has the
/// glyph. Without it the WM shows the label as `both` does (alone under
/// glyph_only): CPU and RAM exist only in Segoe Fluent Icons, so on
/// Windows 10's MDL2 the bar reads "CPU 23%". The keyboard and uptime show
/// their value whatever the mode, and never a label.
fn stat_sample(
    mode: &str,
    icon: &str,
    fmt: &str,
    w: &str,
    drawable: &dyn Fn(char) -> bool,
) -> (Option<char>, String) {
    let Some(&(_, values, g)) = READINGS.iter().find(|r| r.0 == w) else {
        return (None, String::new());
    };
    let glyphs = matches!(mode, "glyph" | "glyph_only");
    let glyph = (glyphs && drawable(g)).then_some(g);
    let text = fill_tokens(fmt, values);
    if matches!(w, "keyboard" | "uptime") {
        return (glyph, text);
    }
    let mode = match (glyphs, glyph) {
        (true, Some(_)) if mode == "glyph_only" => return (glyph, String::new()),
        (true, Some(_)) => "text",
        (true, None) if mode == "glyph_only" => "icon",
        (true, None) => "both",
        (false, _) => mode,
    };
    let text = match mode {
        "icon" if !icon.is_empty() => icon.to_string(),
        "both" if !icon.is_empty() && !text.is_empty() => format!("{icon} {text}"),
        _ => text,
    };
    (glyph, text)
}

/// The clock at 14:05:09, by the WM's own formatter (an empty format is
/// clock_24h's).
fn clock_sample(fmt: &str, h24: bool) -> String {
    format_clock(fmt, h24, 14, 5, 9)
}

/// The date on Sunday 4 Oct 2026 (a one-digit day, so `d` and `dd` differ),
/// by the WM's own formatter.
fn date_sample(fmt: &str) -> String {
    format_date(fmt, 2026, 10, 4, 0)
}

/// The title widget for Notepad with Notes.txt open.
fn title_sample(fmt: &str) -> String {
    fill_tokens(fmt, &[("title", "Notes.txt - Notepad"), ("app", "Notepad")])
}

/// The custom widget: with no command its format is plain text, so
/// `{output}` is empty, as in the WM.
fn custom_sample(c: &Config) -> String {
    let out = if c.bar_custom_command.trim().is_empty() {
        ""
    } else {
        "hello"
    };
    fill_tokens(&c.bar_custom_format, &[("output", out)])
}

/// What text widget `w` shows in a preview (None for the rest).
fn widget_sample(
    c: &Config,
    w: &str,
    drawable: &dyn Fn(char) -> bool,
) -> Option<(Option<char>, String)> {
    let stat = |icon: &str, fmt: &str| stat_sample(&c.bar_icon_mode, icon, fmt, w, drawable);
    Some(match w {
        "title" => (None, title_sample(&c.bar_title_format)),
        "layout" => (None, format!("[{}]", c.layout)),
        "media" => (None, "Artist - Song".to_string()),
        "date" => (None, date_sample(&c.bar_date_format)),
        "clock" => (None, clock_sample(&c.bar_clock_format, c.bar_clock_24h)),
        "custom" => (None, custom_sample(c)),
        "cpu" => stat(&c.bar_icon_cpu, &c.bar_cpu_format),
        "mem" => stat(&c.bar_icon_mem, &c.bar_mem_format),
        "net" => stat(&c.bar_icon_net, &c.bar_net_format),
        "volume" => stat(&c.bar_icon_volume, &c.bar_volume_format),
        "battery" => stat(&c.bar_icon_battery, &c.bar_battery_format),
        "disk" => stat(&c.bar_icon_disk, &c.bar_disk_format),
        "keyboard" => stat("", &c.bar_keyboard_format),
        "uptime" => stat("", &c.bar_uptime_format),
        _ => return None,
    })
}

/// "Shows" and a sample, after its glyph (one `drawable` passed); "nothing"
/// for an empty one (the bar then skips the widget).
fn sample_label(ui: &mut Ui, (glyph, text): (Option<char>, String)) {
    ui.label(RichText::new("Shows").color(sub(ui)));
    if let Some(g) = glyph {
        ui.label(RichText::new(g).font(FontId::new(14.0, icons_family())));
    }
    if text.trim().is_empty() && glyph.is_none() {
        ui.label(RichText::new("nothing").color(sub(ui)));
    } else {
        ui.label(RichText::new(text).family(semibold()));
    }
}

/// Whether the icon font draws `g`: the WM's own test for a stat glyph
/// (stat_glyph), so the previews fall back exactly where the bar does.
fn drawable(ctx: &egui::Context) -> impl Fn(char) -> bool {
    let icons = ctx
        .data(|d| d.get_temp::<bool>(icons_id()))
        .unwrap_or(false);
    let (ctx, font) = (ctx.clone(), FontId::new(14.0, icons_family()));
    move |g| icons && ctx.fonts(|f| f.has_glyph(&font, g))
}

/// Buttons that add a format token at the end of `v`.
fn tokens_ui(ui: &mut Ui, v: &mut String, tokens: &[&str]) {
    for t in tokens {
        if ui
            .small_button(*t)
            .on_hover_text("Add it to the format")
            .clicked()
        {
            v.push_str(t);
        }
    }
}

/// The settings of bar widget `w`, where it has any: a stat's label and
/// format, the clock's and the date's format (each with token buttons and a
/// sample made as the WM makes the text), the workspace marks' width, the
/// app icons' size.
fn widget_rows(ui: &mut Ui, f: &mut Find, c: &mut Config, d: &Config, w: &str) {
    let mode = c.bar_icon_mode.clone();
    let draw = drawable(ui.ctx());
    let stat = match w {
        "cpu" => Some((
            "CPU",
            "icon_cpu, cpu_format",
            &mut c.bar_icon_cpu,
            &mut c.bar_cpu_format,
            &d.bar_icon_cpu,
            &d.bar_cpu_format,
        )),
        "mem" => Some((
            "Memory",
            "icon_mem, mem_format",
            &mut c.bar_icon_mem,
            &mut c.bar_mem_format,
            &d.bar_icon_mem,
            &d.bar_mem_format,
        )),
        "net" => Some((
            "Network",
            "icon_net, net_format",
            &mut c.bar_icon_net,
            &mut c.bar_net_format,
            &d.bar_icon_net,
            &d.bar_net_format,
        )),
        "volume" => Some((
            "Volume",
            "icon_volume, volume_format",
            &mut c.bar_icon_volume,
            &mut c.bar_volume_format,
            &d.bar_icon_volume,
            &d.bar_volume_format,
        )),
        "battery" => Some((
            "Battery",
            "icon_battery, battery_format",
            &mut c.bar_icon_battery,
            &mut c.bar_battery_format,
            &d.bar_icon_battery,
            &d.bar_battery_format,
        )),
        "disk" => Some((
            "Disk space",
            "icon_disk, disk_format",
            &mut c.bar_icon_disk,
            &mut c.bar_disk_format,
            &d.bar_icon_disk,
            &d.bar_disk_format,
        )),
        _ => None,
    };
    if w == "disk" {
        drive_row(ui, f, &mut c.bar_disk_drive, &d.bar_disk_drive);
    }
    if let Some((label, key, icon, fmt, di, dfmt)) = stat {
        let tokens: &[&str] = match w {
            "net" => &["{down}", "{up}"],
            "disk" => &["{free}", "{used}", "{total}", "{value}"],
            _ => &["{value}"],
        };
        let desc = if w == "disk" {
            "Its label, then its format: {free} the space left, {used} the space used, {total} the drive's size, {value} the percent used."
        } else {
            "Its label, then its format. With Show stats as on Windows icons, the icon takes the label's place."
        };
        let meta = (label, desc, key);
        let modified = icon != di || fmt != dfmt;
        if row_core(ui, f, meta, modified, true, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Label");
                cell(ui, icon, "", 70.0);
                ui.label("Format");
                cell(ui, fmt, "", 150.0);
                tokens_ui(ui, fmt, tokens);
                sample_label(ui, stat_sample(&mode, icon, fmt, w, &draw));
            });
        }) {
            icon.clone_from(di);
            fmt.clone_from(dfmt);
        }
        return;
    }
    // Ready-made formats as (format, what it shows); "Custom…" types one.
    let named = |opts: &[(&'static str, &str)], sample: &dyn Fn(&str) -> String| {
        (opts.iter())
            .map(|&(v, what)| (v, format!("{} ({what})", sample(v))))
            .collect::<Vec<_>>()
    };
    let reading = |w: &str| READINGS.iter().find(|r| r.0 == w).map_or(&[][..], |r| r.1);
    match w {
        "clock" => {
            // An empty format falls back to clock_24h, as in the WM.
            let h24 = c.bar_clock_24h;
            format_pick(
                ui,
                f,
                ("Time format", "How the time shows. Custom\u{2026}: HH or H the hour 0-23, hh or h 1-12, mm minutes, ss seconds, tt am or pm, TT AM or PM.", "clock_format"),
                (&mut c.bar_clock_format, &d.bar_clock_format),
                &named(&CLOCKS, &|v| clock_sample(v, true)),
                &["HH", "h", "mm", "ss", "tt", "TT"],
                &|v| clock_sample(v, h24),
            );
        }
        "date" => format_pick(
            ui,
            f,
            ("Date format", "How the date shows. Custom\u{2026}: yyyy the year, MMMM the month (MMM short, MM its number), dddd the weekday (ddd short), dd or d the day.", "date_format"),
            (&mut c.bar_date_format, &d.bar_date_format),
            &DATES.map(|v| (v, date_sample(v))),
            &["dddd", "ddd", "dd", "d", "MMMM", "MMM", "MM", "yyyy"],
            &date_sample,
        ),
        "title" => {
            format_pick(
                ui,
                f,
                ("Title shows", "What it shows of the window in front. Custom\u{2026}: {title} its title, {app} its app's name.", "title_format"),
                (&mut c.bar_title_format, &d.bar_title_format),
                &TITLES.map(|(v, l)| (v, l.to_string())),
                &["{title}", "{app}"],
                &title_sample,
            );
            toggle(
                ui,
                f,
                ("Title in bold", "", "title_bold"),
                &mut c.bar_title_bold,
                d.bar_title_bold,
            );
        }
        "keyboard" => {
            let sample = |v: &str| fill_tokens(v, reading("keyboard"));
            format_pick(
                ui,
                f,
                ("Shows", "Custom\u{2026}: {lang} the language, {region} the country.", "keyboard_format"),
                (&mut c.bar_keyboard_format, &d.bar_keyboard_format),
                &named(&KEYBOARDS, &sample),
                &["{lang}", "{region}"],
                &sample,
            );
        }
        "uptime" => {
            let sample = |v: &str| fill_tokens(v, reading("uptime"));
            format_pick(
                ui,
                f,
                ("Shows", "Custom\u{2026}: {value} is the time since Windows started.", "uptime_format"),
                (&mut c.bar_uptime_format, &d.bar_uptime_format),
                &[("{value}", sample("{value}")), ("up {value}", sample("up {value}"))],
                &["{value}"],
                &sample,
            );
        }
        "custom" => custom_rows(ui, f, c, d),
        "workspaces" if !f.on() => note(ui, "How the workspaces look is under Workspaces, further down."),
        "apps" => num(
            ui,
            f,
            ("App icon size", "", "icon_size"),
            &mut c.bar_icon_size,
            d.bar_icon_size,
            8..=48,
            " px",
        ),
        _ => {}
    }
}

/// Ready-made clock formats: (format, what kind of clock).
const CLOCKS: [(&str, &str); 5] = [
    ("HH:mm", "24-hour"),
    ("h:mm tt", "12-hour"),
    ("h:mm TT", "12-hour"),
    ("HH:mm:ss", "24-hour with seconds"),
    ("h:mm:ss TT", "12-hour with seconds"),
];

/// Ready-made date formats; each shows as its sample.
const DATES: [&str; 6] = [
    "ddd d MMM",
    "ddd dd MMM",
    "dddd d MMMM",
    "d/MM/yyyy",
    "MM/dd/yyyy",
    "yyyy-MM-dd",
];

/// Ready-made title formats: (format, name).
const TITLES: [(&str, &str); 3] = [
    ("{title}", "Window title"),
    ("{app}", "App name"),
    ("{app} - {title}", "App and window title"),
];

/// Ready-made keyboard formats: (format, what it is).
const KEYBOARDS: [(&str, &str); 3] = [
    ("{lang}", "language"),
    ("{lang}-{region}", "language and country"),
    ("{region}", "country"),
];

/// How often the custom widget's command runs: (seconds, in words).
const INTERVALS: [(&str, &str); 8] = [
    ("1", "Every second"),
    ("5", "Every 5 seconds"),
    ("10", "Every 10 seconds"),
    ("30", "Every 30 seconds"),
    ("60", "Every minute"),
    ("300", "Every 5 minutes"),
    ("900", "Every 15 minutes"),
    ("3600", "Every hour"),
];

/// A format picked by what it shows (`presets`: format, label) from a
/// drop-down, or typed after "Custom…" with token buttons; then what it
/// shows now. Typing is out of the way until asked for: most people pick.
/// No presets: just the field.
fn format_pick(
    ui: &mut Ui,
    f: &mut Find,
    meta: Meta,
    (v, d): (&mut String, &str),
    presets: &[(&str, String)],
    tokens: &[&str],
    sample: &dyn Fn(&str) -> String,
) {
    let id = egui::Id::new(("format", meta.2));
    if row_core(ui, f, meta, v != d, true, |ui| {
        let preset = presets.iter().find(|p| p.0 == v.as_str());
        let typed = preset.is_none() || ui.data(|m| m.get_temp::<bool>(id)) == Some(true);
        ui.horizontal_wrapped(|ui| {
            if !presets.is_empty() {
                let shown = preset.map_or("Custom", |p| p.1.as_str());
                egui::ComboBox::from_id_salt(id)
                    .selected_text(shown)
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        for (p, label) in presets {
                            if ui.selectable_label(v == p, label).clicked() {
                                *v = p.to_string();
                                ui.data_mut(|m| m.remove::<bool>(id));
                            }
                        }
                        ui.separator();
                        if ui.selectable_label(false, "Custom\u{2026}").clicked() {
                            ui.data_mut(|m| m.insert_temp(id, true));
                        }
                    });
            }
            if typed {
                cell(ui, v, "", 150.0);
                tokens_ui(ui, v, tokens);
            }
            sample_label(ui, (None, sample(v)));
        });
    }) {
        *v = d.to_string();
    }
}

/// The custom widget: an optional command (with what running it means),
/// how often it runs, and the text around its output.
fn custom_rows(ui: &mut Ui, f: &mut Find, c: &mut Config, d: &Config) {
    text(
        ui,
        f,
        ("Command", "Optional. It runs hidden every so often, as you, and the first line it prints is shown; one that takes over 5 seconds is stopped. Only use a command you trust.", "custom_command"),
        &mut c.bar_custom_command,
        &d.bar_custom_command,
    );
    let command = !c.bar_custom_command.trim().is_empty();
    when(ui, f, command, |ui, f| {
        row(
            ui,
            f,
            ("How often", "", "custom_interval"),
            &mut c.bar_custom_interval,
            &d.bar_custom_interval,
            |ui, v| {
                let mut s = v.to_string();
                combo(ui, "custom-interval", &mut s, &INTERVALS, 200.0);
                if let Ok(n) = s.parse() {
                    *v = n;
                }
            },
        );
    });
    let out = if command { "hello" } else { "" };
    format_pick(
        ui,
        f,
        (
            "Text",
            "Your own words, with {output} where the command's first line goes.",
            "custom_format",
        ),
        (&mut c.bar_custom_format, &d.bar_custom_format),
        &[],
        &["{output}"],
        &|v| fill_tokens(v, &[("output", out)]),
    );
}

/// Drives with something in them (`C:\`), found by the startup thread: a
/// network drive can take seconds to answer. Unset until then, and in tests.
static DRIVES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

fn ready_drives() -> Vec<String> {
    (b'A'..=b'Z')
        .map(|d| format!("{}:\\", d as char))
        .filter(|d| std::fs::metadata(d).is_ok())
        .collect()
}

/// The disk widget's drive: one of this PC's, or a folder picked with the
/// native dialog (the space of the drive it is on).
fn drive_row(ui: &mut Ui, f: &mut Find, v: &mut String, d: &str) {
    let id = egui::Id::new("disk-drive");
    if let Some(r) = take_pick(id).as_deref().and_then(disk_root) {
        *v = r;
    }
    let name = |s: &str| s.trim_end_matches('\\').to_string();
    let meta = ("Drive", "Which drive's space it shows.", "disk_drive");
    row(ui, f, meta, v, &d.to_string(), |ui, v| {
        egui::ComboBox::from_id_salt(id)
            .selected_text(name(v))
            .width(ui.available_width().min(200.0))
            .show_ui(ui, |ui| {
                for drive in DRIVES.get().into_iter().flatten() {
                    if ui
                        .selectable_label(drive.eq_ignore_ascii_case(v), name(drive))
                        .clicked()
                    {
                        v.clone_from(drive);
                    }
                }
                ui.separator();
                if ui.selectable_label(false, Pick::Folder.choose()).clicked() {
                    start_pick(ui.ctx(), id, Pick::Folder, v);
                }
            });
    });
}

/// `workspace_label` values and their names.
const WS_LABELS: [(&str, &str); 5] = [
    ("auto", "Automatic"),
    ("number", "Numbers"),
    ("name", "Names"),
    ("icon", "Icons"),
    ("icon_name", "Icon and name"),
];

/// The first `n` workspace marks' labels, by the WM's own rule.
fn ws_labels(c: &Config, n: usize) -> Vec<String> {
    (0..n)
        .map(|i| {
            let name = c.workspace_names.get(i).map_or("", String::as_str);
            let icon = c.workspace_icons.get(i).map_or("", String::as_str);
            workspace_label(&c.bar_workspace_label, i + 1, name, icon)
        })
        .collect()
}

fn ws_label_row(ui: &mut Ui, f: &mut Find, c: &mut Config, d: &Config) {
    let meta = ("Workspace labels", "What each workspace mark shows. Automatic: its icon, else its name, else its number. Names and icons are set on the Workspaces page.", "workspace_label");
    let sample = ws_labels(c, 3).join("   ");
    let v = &mut c.bar_workspace_label;
    if row_core(ui, f, meta, *v != d.bar_workspace_label, true, |ui| {
        ui.horizontal_wrapped(|ui| {
            combo(ui, "ws-label", v, &WS_LABELS, 160.0);
            sample_label(ui, (None, sample));
        });
    }) {
        v.clone_from(&d.bar_workspace_label);
    }
}

/// The screen numbers a `monitors` list names ("1 3"); none for all / primary.
fn screen_list(v: &str) -> Vec<u32> {
    v.split(' ').filter_map(|n| n.parse().ok()).collect()
}

/// `monitors` with screen `s` ticked or unticked, in the parser's sorted
/// form; None when that would leave no screen (keep the last one).
fn toggle_screen(v: &str, s: u32) -> Option<String> {
    let mut list = screen_list(v);
    match list.iter().position(|&x| x == s) {
        Some(i) => {
            list.remove(i);
        }
        None => list.push(s),
    }
    list.sort_unstable();
    let words: Vec<String> = list.iter().map(u32::to_string).collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// `monitors`: every screen, the main one, or the ticked ones, numbered from
/// the left as the WM numbers them. As many boxes as screens are connected
/// (or as the value names).
fn monitors_row(ui: &mut Ui, f: &mut Find, v: &mut String, d: &str) {
    let meta = ("Show the bar on", "Screens are numbered from the left. Windows on a screen without a bar get its full height.", "monitors");
    let screens = sys::screens();
    if row_core(ui, f, meta, v != d, true, |ui| {
        let now = match v.as_str() {
            "all" | "primary" => v.clone(),
            _ => "some".to_string(),
        };
        let list = screen_list(v);
        let n = screens.max(list.last().copied().unwrap_or(0));
        ui.horizontal_wrapped(|ui| {
            let mut pick = now.clone();
            let opts = [
                ("all", "All screens"),
                ("primary", "Main screen only"),
                ("some", "Some screens"),
            ];
            seg(ui, &mut pick, &opts);
            if pick != now {
                *v = match pick.as_str() {
                    "some" => (1..=n).map(|s| s.to_string()).collect::<Vec<_>>().join(" "),
                    p => p.to_string(),
                };
            } else if now == "some" {
                for s in 1..=n {
                    let mut on = list.contains(&s);
                    if ui.checkbox(&mut on, format!("Screen {s}")).changed() {
                        if let Some(t) = toggle_screen(v, s) {
                            *v = t;
                        }
                    }
                }
            }
        });
        if now == "some" && list.iter().all(|&s| s > screens) {
            warn_note(
                ui,
                "None of these screens is connected now, so no bar shows.",
            );
        }
    }) {
        *v = d.to_string();
    }
}

/// Click actions for the widgets on the bar (every widget while searching):
/// a row per widget, a column per mouse button.
fn click_rows(ui: &mut Ui, f: &mut Find, c: &mut Config, d: &Config) {
    let on: Vec<String> = (0..3).flat_map(|z| shown(c, z)).collect();
    let mut any = false;
    for (w, keys) in BAR_CLICK_WIDGETS.iter().zip(click_keys()) {
        if !f.on() && !on.iter().any(|x| x == w) {
            continue;
        }
        any = true;
        let keys = keys.join(", ");
        let meta = (widget_info(w).0, "", keys.as_str());
        let modified = (0..3).any(|b| c.bar_click(w, b) != d.bar_click(w, b));
        if row_core(ui, f, meta, modified, true, |ui| {
            ui.columns(3, |cols| {
                for (b, ui) in cols.iter_mut().enumerate() {
                    let id = egui::Id::new(("click", *w, b));
                    let now = click_now(c, w, b);
                    let ws = c.workspaces;
                    if let Some(v) = click_ui(ui, id, b, &now, (click_default(w, b), ws)) {
                        set_click(c, w, b, &v);
                    }
                }
            });
        }) {
            c.bar_clicks.retain(|x| x.0 != *w);
        }
    }
    if !any && !f.on() {
        note(ui, "None of the widgets on the bar has click actions.");
    }
}

/// One mouse button's action, in a column of its own: the button's name and
/// a drop-down of plain names (the default marked), then under them what
/// the action needs: a workspace number (of the `ws` there are), or the app
/// to open. Returns the new `action [argument]` when it changed.
fn click_ui(
    ui: &mut Ui,
    id: egui::Id,
    button: usize,
    v: &str,
    (default, ws): (&str, usize),
) -> Option<String> {
    let (action, arg) = v.split_once(' ').unwrap_or((v, ""));
    let label = |a: &str| {
        let l = BAR_ACTIONS.iter().find(|x| x.0 == a).map_or(a, |x| x.1);
        if a == default {
            format!("{l} (default)")
        } else {
            l.to_string()
        }
    };
    let mut out = None;
    ui.horizontal(|ui| {
        let name = ["Click", "Right-click", "Middle-click"][button];
        ui.label(RichText::new(name).color(sub(ui)));
        // `width` is only a minimum; the name is cut to the room left.
        egui::ComboBox::from_id_salt(id)
            .selected_text(label(action))
            .width(ui.available_width())
            .height(420.0)
            .truncate()
            .show_ui(ui, |ui| {
                for &(a, _) in BAR_ACTIONS {
                    if ui.selectable_label(a == action, label(a)).clicked() && a != action {
                        out = Some(match a {
                            "switch_workspace" => "switch_workspace 1".to_string(),
                            _ => a.to_string(),
                        });
                    }
                }
            })
            .response
            .on_hover_text(label(action));
    });
    match action {
        "switch_workspace" => {
            let mut n = arg.parse::<usize>().unwrap_or(1);
            if workspace_drag(ui, &mut n, ws) {
                out = Some(format!("switch_workspace {n}"));
            }
            if let Some(why) = missing_workspace(n, ws) {
                warn_note(ui, &why);
            }
        }
        "launch" => {
            let mut app = arg.to_string();
            if app_menu(ui, id.with("app"), &mut app) {
                out = Some(launch_value(&app));
            }
        }
        _ => {}
    }
    out
}

/// A click's `launch` value for the app box's text. Only the start is
/// trimmed: the box is rebuilt from this value next frame, so a trailing
/// space trimmed here would be gone before the next word is typed (`wt -p`
/// became `wt-p`). `diff` and the parser trim it off the file.
fn launch_value(app: &str) -> String {
    let app = app.trim_start();
    if app.is_empty() {
        "launch".to_string()
    } else {
        format!("launch {app}")
    }
}

/// A workspace number, 1 to `ws` (the count set): the WM ignores a switch
/// past it. A number already past it (an older file) shows as it is.
fn workspace_drag(ui: &mut Ui, n: &mut usize, ws: usize) -> bool {
    let drag = egui::DragValue::new(n)
        .range(1..=ws.max(1))
        .clamp_existing_to_range(false)
        .prefix("Workspace ");
    ui.add(drag).changed()
}

/// Why a switch to workspace `n` does nothing, when it is past the `ws` set.
fn missing_workspace(n: usize, ws: usize) -> Option<String> {
    (n > ws).then(|| format!("Workspace {n} doesn't exist (you have {ws}), so this does nothing."))
}

/// Everyday apps a click can open, besides the terminals and browsers found
/// on this PC: (name, program). All ship with Windows.
const COMMON_APPS: [(&str, &str); 3] = [
    ("File Explorer", "explorer.exe"),
    ("Notepad", "notepad.exe"),
    ("Calculator", "calc.exe"),
];

/// The app a click opens: a common or found one, any program (the native
/// dialog), or a command typed after "Type a command…", under it. Returns
/// true when it changed.
fn app_menu(ui: &mut Ui, id: egui::Id, v: &mut String) -> bool {
    let before = v.clone();
    if let Some(p) = take_pick(id) {
        *v = p;
    }
    let found: Vec<Choice> = (COMMON_APPS.iter())
        .map(|&(l, cmd)| Choice::new(l, cmd))
        .chain(
            APPS.get()
                .into_iter()
                .flat_map(|a| a.terminals.iter().chain(&a.browsers).cloned()),
        )
        .collect();
    let typed_id = id.with("typed");
    let label = match v.trim() {
        "" => "Pick an app".to_string(),
        _ => choice_label(v, &found, Some(Pick::App)),
    };
    let typed =
        label.starts_with("Custom") || ui.data(|m| m.get_temp::<bool>(typed_id)) == Some(true);
    // Once shown, the box stays until an app is picked from the menu: typed
    // down to a known app (`wt`), the label stops reading Custom, and the
    // box must not vanish under the cursor.
    if typed {
        ui.data_mut(|m| m.insert_temp(typed_id, true));
    }
    egui::ComboBox::from_id_salt(id)
        .selected_text(label)
        .width(ui.available_width())
        .height(420.0)
        .truncate()
        .show_ui(ui, |ui| {
            for o in &found {
                if ui
                    .selectable_label(same_app(&o.command, v), &o.label)
                    .clicked()
                {
                    v.clone_from(&o.command);
                    ui.data_mut(|m| m.remove::<bool>(typed_id));
                }
            }
            ui.separator();
            if ui.selectable_label(false, Pick::App.choose()).clicked() {
                start_pick(ui.ctx(), id, Pick::App, v);
            }
            if ui
                .selectable_label(false, "Type a command\u{2026}")
                .clicked()
            {
                ui.data_mut(|m| m.insert_temp(typed_id, true));
            }
        });
    if typed {
        let w = ui.available_width();
        cell(ui, v, "Command, e.g. notepad", w);
    }
    *v != before
}

/// `legacy` for a themed colour key that never shipped a hex default: a
/// COLORREF is 24-bit, so no colour equals it and nothing is nudged.
const NO_LEGACY: u32 = u32::MAX;

/// Names for `palette =` values.
fn palette_label(name: &str) -> &str {
    match name {
        "classic" => "Classic (Tokyo Night)",
        "catppuccin" => "Catppuccin",
        "nord" => "Nord",
        "gruvbox" => "Gruvbox",
        "rosepine" => "Ros\u{E9} Pine",
        "mono" => "Mono",
        "system" => "Windows (your accent colour)",
        _ => name,
    }
}

/// A palette's colours as a strip: background, surface, text and accent,
/// then its eight swatches. One allocation, so it reads the same in a
/// right-to-left row.
fn swatches(ui: &mut Ui, r: &BarRoles) {
    let colours: Vec<u32> = [r.bg, r.surface, r.fg, r.accent]
        .into_iter()
        .chain(r.swatches)
        .collect();
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(colours.len() as f32 * 11.0, 16.0),
        Sense::hover(),
    );
    let edge = ui.visuals().widgets.noninteractive.bg_stroke;
    for (i, &c) in colours.iter().enumerate() {
        let sq = egui::Rect::from_min_size(
            rect.min + egui::vec2(i as f32 * 11.0, 0.0),
            egui::vec2(10.0, 16.0),
        );
        ui.painter()
            .rect(sq, 2.0, colorref(c), edge, egui::StrokeKind::Inside);
    }
}

fn palette_row(ui: &mut Ui, f: &mut Find, v: &mut String, d: &str) {
    let meta = ("Palette", "Where every Auto colour below comes from, in a light and a dark version that follow the theme.", "palette");
    if row_core(ui, f, meta, v != d, false, |ui| {
        let light = !ui.visuals().dark_mode;
        let roles = |name: &str| {
            let p = astur_config::palette(name);
            if light {
                p.light
            } else {
                p.dark
            }
        };
        egui::ComboBox::from_id_salt("bar-palette")
            .selected_text(palette_label(v))
            .width(200.0)
            .show_ui(ui, |ui| {
                for p in PALETTES {
                    ui.horizontal(|ui| {
                        swatches(ui, &roles(p.name));
                        ui.selectable_value(v, p.name.to_string(), palette_label(p.name));
                    });
                }
            });
        swatches(ui, &roles(v));
    }) {
        *v = d.to_string();
    }
}

/// `#rrggbb` as a COLORREF; anything else (a swatch name) is None.
fn hex_colorref(s: &str) -> Option<u32> {
    let hex = s.strip_prefix('#').filter(|h| h.len() == 6)?;
    let rgb = u32::from_str_radix(hex, 16).ok()?;
    Some((rgb & 0xFF) << 16 | (rgb & 0xFF00) | rgb >> 16)
}

/// `widget_colors`: a colour per text widget on the bar (the WM colours only
/// those, and only as pills). A swatch name follows a palette or theme
/// change; Custom pins a colour.
fn pill_colors(ui: &mut Ui, f: &mut Find, c: &mut Config, d: &Config, roles: &BarRoles) {
    let mut ws: Vec<String> = Vec::new();
    for w in (0..3).flat_map(|z| shown(c, z)) {
        let text = !matches!(
            w.as_str(),
            "workspaces" | "apps" | "separator" | "spacer" | "logo"
        );
        if text && !ws.contains(&w) {
            ws.push(w);
        }
    }
    let opts: Vec<(&str, &str)> = [("", "Surface"), ("custom", "Custom\u{2026}")]
        .into_iter()
        .chain(SWATCH_NAMES.iter().map(|s| (*s, *s)))
        .collect();
    let meta = ("Pill colours", "Each widget's capsule. A palette colour changes with the palette and theme; Custom stays. Text on a coloured pill is black or white to suit it, unless Text on accent is set.", "widget_colors");
    let mut set = None;
    let modified = c.bar_widget_colors != d.bar_widget_colors;
    if row_core(ui, f, meta, modified, true, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (i, w) in ws.iter().enumerate() {
                // A whole widget's group to a line: a ComboBox (any child
                // ui) is placed before egui knows its size, so it never
                // wraps by itself, and a space at a line's end doesn't
                // either. Either overflowed the card, and egui then grew
                // every card under it past the window's edge.
                let label = widget_info(w).0;
                let font = egui::TextStyle::Body.resolve(ui.style());
                let lw = ui.fonts(|f| f.layout_no_wrap(label.into(), font, Color32::WHITE));
                let gap = ui.spacing().item_spacing.x;
                let need = lw.size().x + 92.0 + ui.spacing().interact_size.x + 3.0 * gap;
                if i > 0 {
                    if ui.available_size_before_wrap().x < need + 12.0 {
                        ui.end_row();
                    } else {
                        ui.add_space(12.0);
                    }
                }
                let cur =
                    (c.bar_widget_colors.iter().rfind(|e| e.0 == *w)).map_or("", |e| e.1.as_str());
                let mut v = cur.to_string();
                let fill = c.bar_widget_fill(w, roles).unwrap_or(roles.surface);
                ui.label(label);
                combo(ui, ("pill", w.as_str()), &mut v, &opts, 92.0);
                if v == "custom" {
                    v = color_to_hex(fill).to_ascii_lowercase();
                }
                match hex_colorref(&v) {
                    Some(mut pinned) => {
                        color_button(ui, &mut pinned);
                        v = color_to_hex(pinned).to_ascii_lowercase();
                    }
                    None => {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::hover());
                        ui.painter().rect_filled(rect, 4.0, colorref(fill));
                    }
                }
                if v != cur {
                    set = Some((w.clone(), v));
                }
            }
            if ws.is_empty() {
                ui.label(RichText::new("No text widget on the bar.").color(sub(ui)));
            }
        });
    }) {
        c.bar_widget_colors.clone_from(&d.bar_widget_colors);
    }
    // In its old place, so the line in the file changes only where it must.
    if let Some((w, v)) = set {
        let at = c.bar_widget_colors.iter().position(|e| e.0 == w);
        c.bar_widget_colors.retain(|e| e.0 != w);
        if !v.is_empty() {
            let i = at.unwrap_or(c.bar_widget_colors.len());
            c.bar_widget_colors.insert(i, (w, v));
        }
    }
}

/// `c` with preset `p` stamped in: its pairs read onto it as lines after
/// everything else, which is what writing them into navbar.conf gives.
fn stamp(c: &Config, p: &BarPreset) -> Config {
    let text: String = p
        .pairs
        .iter()
        .map(|(k, v)| format!("{k} = {v}\n"))
        .collect();
    let mut s = c.clone();
    parse_into(&mut s, &text);
    s
}

/// The navbar keys as the bar draws them: a zone as the widgets it shows,
/// `shape` as in effect (a missing line means `floating` decides).
fn look(c: &Config) -> Pairs {
    let mut nav = conf_pairs(c).1;
    for (k, v) in &mut nav {
        match *k {
            "left" => *v = shown(c, 0).join(" "),
            "center" => *v = shown(c, 1).join(" "),
            "right" => *v = shown(c, 2).join(" "),
            "shape" => *v = c.effective_bar_shape().to_string(),
            // The bar reads the last entry per widget, in no order: a colour
            // set and set back is the same bar, not "modified".
            "widget_colors" => {
                let m: std::collections::BTreeMap<&str, &str> = (c.bar_widget_colors.iter())
                    .map(|(w, col)| (w.as_str(), col.as_str()))
                    .collect();
                *v = m
                    .iter()
                    .map(|(w, col)| format!("{w}:{col}"))
                    .collect::<Vec<_>>()
                    .join(" ");
            }
            _ => {}
        }
    }
    nav
}

/// The keys where `c` looks different from `stamped` (a preset stamped onto
/// it). The show_* switches are left out: the zones already say what shows.
fn preset_diff(c: &Config, stamped: &Config) -> Vec<&'static str> {
    look(c)
        .into_iter()
        .zip(look(stamped))
        .filter(|(a, b)| a.1 != b.1 && !a.0.starts_with("show_"))
        .map(|(a, _)| a.0)
        .collect()
}

/// The first few of `keys`, and how many more.
fn short_list(keys: &[&str]) -> String {
    let mut s = keys[..keys.len().min(4)].join(", ");
    if keys.len() > 4 {
        s.push_str(&format!(" +{} more", keys.len() - 4));
    }
    s
}

/// The preset gallery: which preset the bar is (the closest, with the keys
/// changed since), then a card per preset with a drawn preview. Returns the
/// preset to stamp in (Apply, or Reset to preset).
fn presets_ui(ui: &mut Ui, c: &Config) -> Option<&'static BarPreset> {
    let looks: Vec<(&'static BarPreset, Config, Vec<&str>)> = BAR_PRESETS
        .iter()
        .map(|p| {
            let s = stamp(c, p);
            let diff = preset_diff(c, &s);
            (p, s, diff)
        })
        .collect();
    let (near, _, diff) = looks.iter().min_by_key(|l| l.2.len())?;
    let mut apply = None;
    section_title(ui, "Presets");
    card(ui, |ui| {
        ui.horizontal(|ui| {
            let text = if diff.is_empty() {
                format!("Preset: {}", near.label)
            } else {
                format!(
                    "Preset: {} \u{2014} modified ({})",
                    near.label,
                    short_list(diff)
                )
            };
            ui.label(text).on_hover_text("The preset closest to your bar, and the keys that differ from it. Applying a preset first copies navbar.conf to navbar.conf.bak (once per session), and Undo takes it back.");
            if !diff.is_empty() {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Reset to preset").clicked() {
                        apply = Some(*near);
                    }
                });
            }
        });
    });
    // One card a row: a preview as wide as the page shows a bar about as
    // wide as a screen's, so its right zone no longer covers the rest.
    for (p, s, diff) in &looks {
        ui.add_space(6.0);
        card(ui, |ui| {
            bar_preview(ui, s, 34.0);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let w = ui.available_width() - 100.0;
                ui.allocate_ui_with_layout(
                    egui::vec2(w, 0.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_width(w);
                        ui.label(RichText::new(p.label).family(semibold()));
                        ui.label(RichText::new(p.blurb).small().color(sub(ui)));
                    },
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if diff.is_empty() {
                        ui.label(RichText::new("In use").color(sub(ui)));
                    } else if ui.button("Apply").clicked() {
                        apply = Some(*p);
                    }
                });
            });
        });
    }
    apply
}

/// Preview px per bar px.
const PREVIEW: f32 = 0.75;

/// The music buttons' glyphs (Previous, Play, Next), as the WM's
/// MEDIA_GLYPHS; text stand-ins where the icon font lacks them.
const MEDIA: [(char, &str); 3] = [('\u{E892}', "|<"), ('\u{E768}', ">"), ('\u{E893}', ">|")];

/// A drawn miniature of the bar `c` describes, on a strip of desktop: shape,
/// palette, separators and workspace style as flat shapes, each widget as
/// its sample at a small size, in the light or dark version the theme picks.
/// Close to the GDI bar, not equal to it (egui's font and no ClearType): the
/// real bar is the exact preview, since every change applies live.
fn bar_preview(ui: &mut Ui, c: &Config, h: f32) {
    let light = !ui.visuals().dark_mode;
    let draw = drawable(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), Sense::hover());
    let p = ui.painter_at(rect);
    let r = c.bar_roles(light);
    let k = |v: i32| v as f32 * PREVIEW;
    let desk = if light {
        Color32::from_rgb(188, 200, 216)
    } else {
        Color32::from_rgb(40, 48, 62)
    };
    p.rect_filled(rect, 6.0, desk);
    let shape = c.effective_bar_shape();
    let m = if shape == "floating" {
        k(c.bar_margin).min(8.0)
    } else {
        0.0
    };
    let bh = k(c.bar_height).min(h - 2.0 * m).max(12.0);
    let top = if c.bar_bottom {
        rect.bottom() - m - bh
    } else {
        rect.top() + m
    };
    let bar = egui::Rect::from_min_size(
        egui::pos2(rect.left() + m, top),
        egui::vec2(rect.width() - 2.0 * m, bh),
    );
    let radius = k(c.bar_radius).min(bh / 2.0);
    let outline = (c.bar_border_width > 0).then(|| {
        Stroke::new(
            k(c.bar_border_width).max(1.0),
            colorref(c.bar_border.unwrap_or(r.muted)),
        )
    });
    let floating = shape == "floating";
    p.rect_filled(bar, if floating { radius } else { 0.0 }, colorref(r.bg));
    match (outline, shape) {
        (Some(s), "floating") => {
            p.rect_stroke(bar, radius, s, egui::StrokeKind::Inside);
        }
        (Some(s), "strip") => {
            let y = if c.bar_bottom {
                bar.top() + s.width / 2.0
            } else {
                bar.bottom() - s.width / 2.0
            };
            p.hline(bar.x_range(), y, s);
        }
        _ => {}
    }
    let size = if c.bar_font_size > 0 {
        k(c.bar_font_size)
    } else {
        bh / 2.0
    }
    .clamp(8.0, 15.0);
    let name = c.bar_font_name.to_ascii_lowercase();
    let mono = ["mono", "consol", "code", "courier"]
        .iter()
        .any(|m| name.contains(m));
    let family = if mono {
        FontFamily::Monospace
    } else if c.bar_font_weight >= 600 {
        semibold()
    } else {
        FontFamily::Proportional
    };
    let font = FontId::new(size, family);
    // egui has one heavier face: a bold title in a mono or semibold bar
    // looks as the rest.
    let title_font = if c.bar_title_bold && !mono {
        FontId::new(size, semibold())
    } else {
        font.clone()
    };
    let icon_font = FontId::new(size * 1.2, icons_family());
    let pills = shape == "pills";
    // The WM's powerline_pills: the pills join into arrow segments, the gap
    // is the arrow, separators and spacers have nowhere to go.
    let joined = pills && c.bar_separator_style == "powerline";
    let ipad = if pills { k(c.bar_island_padding) } else { 0.0 };
    let zpad = if shape == "islands" {
        k(c.bar_island_padding)
    } else {
        0.0
    };
    let inset = (bh / 6.0).clamp(2.0, 6.0) / 2.0;
    let cap = (bh - 2.0 * inset) / 2.0;
    let gap = if joined { cap } else { k(c.bar_widget_gap) };
    let marks = ws_marks(&p, c, &font, k(c.bar_workspace_width));
    let icon_px = k(c.bar_icon_size).min(bh - 4.0).max(6.0);
    let media_w = icon_font.size + k(10);
    let fill = |w: &str| pills.then(|| c.bar_widget_fill(w, &r)).flatten();
    // Each zone's pieces: (widget, width, glyph, text, colour).
    type Galley = Option<std::sync::Arc<egui::Galley>>;
    type Piece = (String, f32, Galley, Galley, u32);
    let zones: Vec<Vec<Piece>> = (0..3)
        .map(|z| {
            shown(c, z)
                .into_iter()
                .filter(|w| !(joined && matches!(w.as_str(), "separator" | "spacer")))
                .filter_map(|w| {
                    // The WM's BarItem::color: black or white to suit each
                    // pill unless accent_fg is pinned.
                    let colour = match (fill(&w), w.as_str()) {
                        (Some(f), _) => c.bar_accent_fg.unwrap_or_else(|| text_on(f)),
                        (None, "layout") => r.muted,
                        _ => r.fg,
                    };
                    let (glyph, text) = match widget_sample(c, &w, &draw) {
                        // A text widget with nothing to show takes no room.
                        Some((None, t)) if t.trim().is_empty() => return None,
                        Some((g, t)) => {
                            let g = g.map(|g| {
                                p.layout_no_wrap(g.to_string(), icon_font.clone(), colorref(colour))
                            });
                            let f = if w == "title" { &title_font } else { &font };
                            let t = (!t.is_empty())
                                .then(|| p.layout_no_wrap(t, f.clone(), colorref(colour)));
                            (g, t)
                        }
                        None => (None, None),
                    };
                    let content = match w.as_str() {
                        "workspaces" => marks.iter().map(|m| m.1).sum(),
                        "apps" => 3.0 * (icon_px + 6.0),
                        "separator" => match c.bar_separator_style.as_str() {
                            "dot" => (bh * 0.14).max(3.0),
                            "powerline" => bh * 0.3,
                            _ => 1.0,
                        },
                        "spacer" => 2.0 * k(c.bar_widget_gap),
                        "logo" => bh * 1.05,
                        "media_controls" => 3.0 * media_w,
                        _ => {
                            let gw = glyph.as_ref().map_or(0.0, |g| g.size().x);
                            let tw = text.as_ref().map_or(0.0, |t| t.size().x);
                            gw + tw + if gw > 0.0 && tw > 0.0 { 3.0 } else { 0.0 }
                        }
                    };
                    let pad = if matches!(w.as_str(), "separator" | "spacer") {
                        0.0
                    } else {
                        ipad
                    };
                    Some((w, content + 2.0 * pad, glyph, text, colour))
                })
                .collect()
        })
        .collect();
    let span =
        |z: &[Piece]| z.iter().map(|x| x.1).sum::<f32>() + gap * z.len().saturating_sub(1) as f32;
    // As the WM lays them out: the right zone first, the left one cut off
    // where the right one starts, the centre in the room between them.
    let pad = k(c.bar_padding);
    let right = bar.right() - pad - zpad - span(&zones[2]);
    let left_end = bar.left() + pad + zpad + span(&zones[0]);
    let mid = span(&zones[1]);
    let room = (left_end + 2.0 * zpad + gap, right - 2.0 * zpad - gap);
    let centre = (bar.center().x - mid / 2.0).min(room.1 - mid).max(room.0);
    let starts = [bar.left() + pad + zpad, centre, right];
    let clips = [
        (bar.left(), right - zpad - gap),
        (room.0 - zpad, room.1 + zpad),
        (bar.left(), bar.right()),
    ];
    let band = |l: f32, rr: f32| {
        egui::Rect::from_min_max(
            egui::pos2(l, bar.top() + inset),
            egui::pos2(rr, bar.bottom() - inset),
        )
    };
    let backdrop = |p: &egui::Painter, rect: egui::Rect, colour: u32| {
        p.rect_filled(rect, radius, colorref(colour));
        if let Some(s) = outline {
            p.rect_stroke(rect, radius, s, egui::StrokeKind::Inside);
        }
    };
    let cy = bar.center().y;
    for (z, pieces) in zones.iter().enumerate() {
        let (l, rr) = clips[z];
        let p = p.with_clip_rect(egui::Rect::from_x_y_ranges(l..=rr, bar.y_range()));
        let xs: Vec<f32> = (pieces.iter())
            .scan(starts[z], |x, pc| {
                let at = *x;
                *x += pc.1 + gap;
                Some(at)
            })
            .collect();
        if zpad > 0.0 && !pieces.is_empty() {
            backdrop(
                &p,
                band(starts[z] - zpad, starts[z] + span(pieces) + zpad),
                r.surface,
            );
        }
        if pills {
            let segs: Vec<(f32, f32, u32)> = (pieces.iter().zip(&xs))
                .filter(|(pc, _)| !matches!(pc.0.as_str(), "separator" | "spacer"))
                .map(|(pc, &x)| (x, x + pc.1, fill(&pc.0).unwrap_or(r.surface)))
                .collect();
            if joined {
                powerline_segments(&p, &segs, z == 2, &band, cap);
            } else {
                for &(a, b, colour) in &segs {
                    backdrop(&p, band(a, b), colour);
                }
            }
        }
        for ((w, width, glyph, text, colour), &x) in pieces.iter().zip(&xs) {
            let spaced = matches!(w.as_str(), "separator" | "spacer");
            let mut cx = x + if spaced { 0.0 } else { ipad };
            match w.as_str() {
                "workspaces" => preview_workspaces(&p, c, &r, bar, cx, &marks, &font),
                "apps" => {
                    for (i, s) in [5, 3, 1].into_iter().enumerate() {
                        let at =
                            egui::pos2(cx + 3.0 + i as f32 * (icon_px + 6.0), cy - icon_px / 2.0);
                        p.rect_filled(
                            egui::Rect::from_min_size(at, egui::vec2(icon_px, icon_px)),
                            3.0,
                            colorref(r.swatches[s]),
                        );
                    }
                }
                "separator" => {
                    let muted = colorref(r.muted);
                    match c.bar_separator_style.as_str() {
                        "dot" => {
                            p.circle_filled(egui::pos2(x + width / 2.0, cy), width / 2.0, muted);
                        }
                        "powerline" => {
                            // Away from the screen edge its zone hugs.
                            let half = bh * 0.18;
                            let (base, tip) = if z == 2 {
                                (x + width, x)
                            } else {
                                (x, x + width)
                            };
                            let pts = [
                                egui::pos2(base, cy - half),
                                egui::pos2(tip, cy),
                                egui::pos2(base, cy + half),
                            ];
                            triangle(&p, pts, muted);
                        }
                        _ => {
                            p.vline(
                                x,
                                (cy - bh / 4.0)..=(cy + bh / 4.0),
                                Stroke::new(1.0_f32, muted),
                            );
                        }
                    }
                }
                "logo" => {
                    let size = egui::vec2(bh * 1.05, bh * 0.5);
                    let at = egui::Rect::from_min_size(egui::pos2(cx, cy - size.y / 2.0), size);
                    bird(&p, at, colorref(*colour));
                }
                "media_controls" => {
                    for (i, (g, stand_in)) in MEDIA.iter().enumerate() {
                        let galley = if draw(*g) {
                            p.layout_no_wrap(g.to_string(), icon_font.clone(), colorref(*colour))
                        } else {
                            p.layout_no_wrap(stand_in.to_string(), font.clone(), colorref(*colour))
                        };
                        let mid = cx + (i as f32 + 0.5) * media_w;
                        let at =
                            egui::pos2(mid - galley.size().x / 2.0, cy - galley.size().y / 2.0);
                        p.galley(at, galley, Color32::WHITE);
                    }
                }
                _ => {
                    for g in [glyph, text].into_iter().flatten() {
                        let y = cy - g.size().y / 2.0;
                        p.galley(egui::pos2(cx, y), g.clone(), Color32::WHITE);
                        cx += g.size().x + 3.0;
                    }
                }
            }
        }
    }
}

/// A filled triangle; egui wants a convex polygon's points clockwise on
/// screen, so they are put in that order (its anti-aliased edge goes the
/// wrong way otherwise).
fn triangle(p: &egui::Painter, mut pts: [egui::Pos2; 3], colour: Color32) {
    let [a, b, c] = pts;
    if (b - a).x * (c - a).y - (b - a).y * (c - a).x < 0.0 {
        pts.swap(1, 2);
    }
    p.add(egui::Shape::convex_polygon(
        pts.to_vec(),
        colour,
        Stroke::NONE,
    ));
}

/// Joined powerline segments `segs` ((left, right, fill) each), as the WM's
/// bar_backdrop draws them: square, the gap before each segment in its own
/// colour (after it on the `right` zone, which mirrors), then every arrow in
/// its segment's colour, `cap` long, pointing away from the zone's edge.
fn powerline_segments(
    p: &egui::Painter,
    segs: &[(f32, f32, u32)],
    right: bool,
    band: &dyn Fn(f32, f32) -> egui::Rect,
    cap: f32,
) {
    for (i, &(a, b, colour)) in segs.iter().enumerate() {
        let (a, b) = if right {
            (a, segs.get(i + 1).map_or(b, |s| s.0))
        } else {
            (i.checked_sub(1).map_or(a, |j| segs[j].1), b)
        };
        p.rect_filled(band(a, b), 0.0, colorref(colour));
    }
    for &(a, b, colour) in segs {
        let r = band(a, b);
        let (base, tip) = if right { (a, a - cap) } else { (b, b + cap) };
        let pts = [
            egui::pos2(base, r.top()),
            egui::pos2(tip, r.center().y),
            egui::pos2(base, r.bottom()),
        ];
        triangle(p, pts, colorref(colour));
    }
}

/// The logo in a preview: a bird with spread wings in `rect`, standing in for
/// Astur's bird, which the WM draws from its tray icon.
fn bird(p: &egui::Painter, rect: egui::Rect, colour: Color32) {
    let at = |x: f32, y: f32| rect.lerp_inside(egui::vec2(x, y));
    let parts = [
        [at(0.0, 0.1), at(0.5, 0.35), at(0.46, 0.75)],
        [at(1.0, 0.1), at(0.54, 0.75), at(0.5, 0.35)],
        [at(0.5, 0.25), at(0.57, 0.55), at(0.5, 1.0)],
        [at(0.5, 0.25), at(0.5, 1.0), at(0.43, 0.55)],
    ];
    for pts in parts {
        triangle(p, pts, colour);
    }
}

/// The workspace marks a preview draws: (label, width, app icons). Four
/// workspaces at most, the first two occupied (two apps, then one). Widths
/// as the WM's mark_widths: `cell`, or room for the label and its icons
/// once workspace_label is not auto or workspace_apps is on (never dots).
fn ws_marks(p: &egui::Painter, c: &Config, font: &FontId, cell: f32) -> Vec<(String, f32, usize)> {
    let dots = c.bar_workspace_style == "dot";
    let fit = !dots && (c.bar_workspace_label != "auto" || c.bar_workspace_apps);
    (ws_labels(c, c.workspaces.clamp(1, 4))
        .into_iter()
        .enumerate())
    .map(|(i, label)| {
        let icons = match c.bar_workspace_apps && !dots {
            true => [2, 1].get(i).copied().unwrap_or(0),
            false => 0,
        };
        let w = if fit {
            let tw = p.layout_no_wrap(label.clone(), font.clone(), Color32::WHITE);
            cell.max(tw.size().x + mark_icons_w(icons) + 16.0 * PREVIEW)
        } else {
            cell
        };
        (label, w, icons)
    })
    .collect()
}

/// The room `n` app icons take after a mark's label (the WM's mark_icons_w,
/// at the preview's scale).
fn mark_icons_w(n: usize) -> f32 {
    match n {
        0 => 0.0,
        n => (4.0 + n as f32 * 16.0 + (n - 1) as f32 * 2.0) * PREVIEW,
    }
}

/// The workspace marks in a preview, from `x`, as the WM's draw_workspaces
/// draws each style: the first active.
fn preview_workspaces(
    p: &egui::Painter,
    c: &Config,
    r: &BarRoles,
    bar: egui::Rect,
    x: f32,
    marks: &[(String, f32, usize)],
    font: &FontId,
) {
    let Some(&(_, w0, _)) = marks.first() else {
        return;
    };
    let bh = bar.height();
    let ipad = (bh / 6.0).clamp(2.0, 6.0);
    let cy = bar.center().y;
    let spans: Vec<(f32, f32)> = (marks.iter())
        .scan(x, |at, m| {
            *at += m.1;
            Some((*at - m.1, *at))
        })
        .collect();
    let rest = |i: usize| colorref(if i < 2 { r.fg } else { r.muted });
    let icon = (16.0 * PREVIEW).min(bh - 4.0);
    // Each label, then its app icons, centred together and clipped to the
    // mark, as DrawTextW clips it.
    let label = |i: usize, colour: Color32| {
        let ((a, b), (text, _, n)) = (spans[i], &marks[i]);
        let g = p.layout_no_wrap(text.clone(), font.clone(), colour);
        let gw = g.size().x;
        let left = a + (b - a - gw - mark_icons_w(*n)) / 2.0;
        let p = p.with_clip_rect(egui::Rect::from_x_y_ranges(a..=b, bar.y_range()));
        p.galley(egui::pos2(left, cy - g.size().y / 2.0), g, colour);
        let mut ix = left + gw + 4.0 * PREVIEW;
        for s in 0..*n {
            let sq =
                egui::Rect::from_min_size(egui::pos2(ix, cy - icon / 2.0), egui::vec2(icon, icon));
            p.rect_filled(sq, 2.0, colorref(r.swatches[[5, 3][s % 2]]));
            ix += icon + 2.0 * PREVIEW;
        }
    };
    let accent = colorref(r.accent);
    match c.bar_workspace_style.as_str() {
        "dot" => {
            let d = (bh * 0.2).max(3.0);
            for (i, &(a, b)) in spans.iter().enumerate() {
                p.circle_filled(egui::pos2((a + b) / 2.0, cy), d / 2.0, rest(i));
            }
            let w = (d * 2.6).min(w0 - 4.0).max(d);
            let mark = egui::Rect::from_center_size(egui::pos2(x + w0 / 2.0, cy), egui::vec2(w, d));
            p.rect_filled(mark, d / 2.0, accent);
        }
        "underline" => {
            for i in 1..marks.len() {
                label(i, rest(i));
            }
            label(0, colorref(r.fg));
            let (uw, uh) = ((w0 * 0.45).max(6.0), (bh / 14.0).max(1.5));
            let bottom = bar.bottom() - (ipad / 2.0).max(1.0);
            let line = egui::Rect::from_min_max(
                egui::pos2(x + (w0 - uw) / 2.0, bottom - uh),
                egui::pos2(x + (w0 + uw) / 2.0, bottom),
            );
            p.rect_filled(line, uh / 2.0, accent);
        }
        style => {
            for i in 1..marks.len() {
                label(i, rest(i));
            }
            let mark = egui::Rect::from_min_max(
                egui::pos2(x + 2.0, bar.top() + ipad),
                egui::pos2(x + w0 - 2.0, bar.bottom() - ipad),
            );
            let round = if style == "pill" { bh / 2.0 } else { 0.0 };
            p.rect_filled(mark, round, accent);
            label(0, colorref(r.accent_fg));
        }
    }
}

/// Installed font family names, set by a thread at startup
/// (`sys::font_families`); unset until it is done, and in tests.
static FONTS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

/// Offered first in the font menu, when installed.
const FONT_PICKS: [&str; 8] = [
    "Segoe UI",
    "Segoe UI Variable Text",
    "Segoe UI Semibold",
    "Bahnschrift",
    "Cascadia Code",
    "Consolas",
    "JetBrains Mono",
    "Arial",
];

/// What a font's sample says: letters, digits and a time, as on the bar.
const FONT_SAMPLE: &str = "Aa Bb Cc 0123 12:45";

/// Whether a family called `name` is installed, case ignored (an empty name
/// is the default font). The list is GDI's own, which holds every name
/// CreateFont finds, variable-font instances (Segoe UI Variable Text)
/// included, so an exact match is the check.
fn font_installed(all: &[String], name: &str) -> bool {
    let n = name.trim();
    n.is_empty() || all.iter().any(|f| f.eq_ignore_ascii_case(n))
}

/// A font family: picked from the installed ones (with a search box) or
/// typed after "Custom…", and a sample drawn by Windows' own text renderer,
/// as the bar and the popups draw it. "Not installed" when no installed
/// family has the name: Windows would quietly draw a stand-in.
fn font_row(ui: &mut Ui, f: &mut Find, meta: Meta, v: &mut String, d: &str) {
    let fonts = FONTS.get();
    let missing = fonts.is_some_and(|all| !font_installed(all, v));
    let id = egui::Id::new(("font", meta.2));
    if row_core(ui, f, meta, v != d, true, |ui| {
        let typed = ui.data(|m| m.get_temp::<bool>(id.with("custom"))) == Some(true);
        ui.horizontal_wrapped(|ui| {
            let shown = if v.is_empty() { "Default" } else { v.as_str() };
            egui::ComboBox::from_id_salt(id)
                .selected_text(shown.to_string())
                .width(240.0)
                .height(420.0)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show_ui(ui, |ui| font_menu(ui, id, v, fonts));
            if typed || missing {
                ui.add(
                    egui::TextEdit::singleline(v)
                        .hint_text("Font name")
                        .desired_width(200.0)
                        .margin(FIELD_MARGIN),
                );
                one_line(v);
            }
            if missing {
                ui.colored_label(ui.visuals().warn_fg_color, "Not installed")
                    .on_hover_text(
                        "No installed font has this name, so Windows draws a stand-in font.",
                    );
            }
            font_preview(ui, id, v);
        });
    }) {
        *v = d.to_string();
    }
}

/// The font drop-down: a search box, the usual picks, every installed
/// family, then Custom… (type a name). A click picks and closes it.
fn font_menu(ui: &mut Ui, id: egui::Id, v: &mut String, fonts: Option<&Vec<String>>) {
    let fid = id.with("filter");
    let mut filter = ui.data(|m| m.get_temp::<String>(fid)).unwrap_or_default();
    ui.add(
        egui::TextEdit::singleline(&mut filter)
            .hint_text("Search fonts")
            .desired_width(f32::INFINITY),
    );
    let q = filter.trim().to_lowercase();
    let mut item = |ui: &mut Ui, name: &str| {
        if (q.is_empty() || name.to_lowercase().contains(&q))
            && ui
                .selectable_label(v.eq_ignore_ascii_case(name), name)
                .clicked()
        {
            *v = name.to_string();
            ui.memory_mut(|m| m.close_popup());
        }
    };
    egui::ScrollArea::vertical()
        .max_height(320.0)
        .show(ui, |ui| {
            let picks = FONT_PICKS.iter();
            for p in picks.filter(|p| fonts.is_none_or(|all| font_installed(all, p))) {
                item(ui, p);
            }
            ui.separator();
            for name in fonts.into_iter().flatten() {
                item(ui, name);
            }
        });
    ui.separator();
    if ui
        .selectable_label(false, "Custom\u{2026}")
        .on_hover_text("Type a font name")
        .clicked()
    {
        ui.data_mut(|m| m.insert_temp(id.with("custom"), true));
        ui.memory_mut(|m| m.close_popup());
    }
    ui.data_mut(|m| m.insert_temp(fid, filter));
}

/// `name`'s sample as GDI draws it (so it looks as the bar will), tinted
/// with the text colour; rendered again only when the name changes.
fn font_preview(ui: &mut Ui, id: egui::Id, name: &str) {
    if name.trim().is_empty() {
        return;
    }
    type Cached = (String, Option<egui::TextureHandle>);
    let key = id.with("preview");
    let ppp = ui.ctx().pixels_per_point();
    let tex = match ui.data(|m| m.get_temp::<Cached>(key)) {
        Some((n, t)) if n == name => t,
        _ => {
            let px = (16.0 * ppp).round() as i32;
            let t = sys::font_sample(name, px, FONT_SAMPLE).map(|(w, h, alpha)| {
                let pixels = (alpha.into_iter())
                    .map(|a| Color32::from_rgba_premultiplied(a, a, a, a))
                    .collect();
                let image = egui::ColorImage {
                    size: [w, h],
                    pixels,
                };
                (ui.ctx()).load_texture(format!("font {name}"), image, egui::TextureOptions::LINEAR)
            });
            ui.data_mut(|m| m.insert_temp(key, (name.to_string(), t.clone())));
            t
        }
    };
    if let Some(t) = tex {
        let image = egui::Image::new((t.id(), t.size_vec2() / ppp)).tint(ui.visuals().text_color());
        ui.add(image).on_hover_text("How Windows draws this font");
    }
}

// ---- pickers -------------------------------------------------------------------

/// A ready-made value in a picker: its name, and the text written.
#[derive(Clone, Debug, PartialEq)]
struct Choice {
    label: String,
    command: String,
}

impl Choice {
    fn new(label: &str, command: &str) -> Self {
        Choice {
            label: label.to_string(),
            command: command.to_string(),
        }
    }
}

/// The apps the pickers offer, found on this PC by the startup thread
/// (`detect_apps`); unset until it is done, and in tests.
static APPS: std::sync::OnceLock<Apps> = std::sync::OnceLock::new();

#[derive(Debug, Default)]
struct Apps {
    terminals: Vec<Choice>,
    browsers: Vec<Choice>,
}

/// Terminals the pickers look for: (name, program, the other places it
/// installs). Windows Terminal is a Store app, started by its `wt.exe` alias.
type Known = (&'static str, &'static str, &'static [&'static str]);
const TERMINALS: [Known; 11] = [
    (
        "Windows Terminal",
        "wt.exe",
        &[r"%LOCALAPPDATA%\Microsoft\WindowsApps\wt.exe"],
    ),
    (
        "PowerShell 7",
        "pwsh.exe",
        &[r"%ProgramFiles%\PowerShell\7\pwsh.exe"],
    ),
    ("Windows PowerShell", "powershell.exe", &[]),
    ("Command Prompt", "cmd.exe", &[]),
    (
        "Git Bash",
        "git-bash.exe",
        &[
            r"%ProgramFiles%\Git\git-bash.exe",
            r"%LOCALAPPDATA%\Programs\Git\git-bash.exe",
        ],
    ),
    (
        "WezTerm",
        "wezterm-gui.exe",
        &[r"%ProgramFiles%\WezTerm\wezterm-gui.exe"],
    ),
    (
        "Alacritty",
        "alacritty.exe",
        &[r"%ProgramFiles%\Alacritty\alacritty.exe"],
    ),
    ("Rio", "rio.exe", &[r"%ProgramFiles%\Rio\rio.exe"]),
    (
        "Tabby",
        "tabby.exe",
        &[r"%LOCALAPPDATA%\Programs\Tabby\Tabby.exe"],
    ),
    (
        "Hyper",
        "hyper.exe",
        &[r"%LOCALAPPDATA%\Programs\Hyper\Hyper.exe"],
    ),
    (
        "ConEmu",
        "conemu64.exe",
        &[r"%ProgramFiles%\ConEmu\ConEmu64.exe"],
    ),
];

/// Web searches the launcher offers: (name, address, {query} = the words).
const SEARCH_ENGINES: [(&str, &str); 5] = [
    ("Google", "https://www.google.com/search?q={query}"),
    ("Bing", "https://www.bing.com/search?q={query}"),
    ("DuckDuckGo", "https://duckduckgo.com/?q={query}"),
    ("Brave Search", "https://search.brave.com/search?q={query}"),
    ("Ecosia", "https://www.ecosia.org/search?q={query}"),
];

/// Home's gap sizes: (id, label, px at the edges and between windows).
/// Medium is the shipped 8.
const GAPS: [(&str, &str, i32); 4] = [
    ("none", "None", 0),
    ("small", "Small", 4),
    ("medium", "Medium", 8),
    ("large", "Large", 16),
];

/// The gap size `outer` and `inner` make, or "custom".
fn gap_preset(outer: i32, inner: i32) -> &'static str {
    GAPS.iter()
        .find(|g| outer == g.2 && inner == g.2)
        .map_or("custom", |g| g.0)
}

/// Home's "How to use Astur": (keys, what they do), from the keys in use.
fn tips(c: &Config) -> Vec<(String, &'static str)> {
    let (m, k) = (mk(c), vk_to_key);
    let mut v: Vec<(String, &'static str)> = vec![
        (
            format!("{m}drag"),
            "Move a window: hold the key and drag anywhere on it",
        ),
        (
            format!("{m}right-drag"),
            "Resize a window from its nearest corner",
        ),
    ];
    let ws: Vec<String> = (c.workspace_keys.iter())
        .take(c.workspaces.min(9))
        .map(|&w| k(w))
        .collect();
    if let (Some(a), Some(b)) = (ws.first(), ws.last()) {
        let keys = if ws.len() > 1 {
            format!("{m}{a} \u{2026} {m}{b}")
        } else {
            format!("{m}{a}")
        };
        v.push((
            keys,
            "Go to a workspace (add Shift to take the window along)",
        ));
    }
    let focus = format!("{m}{} / {m}{}", k(c.key_focus_next), k(c.key_focus_prev));
    v.push((focus, "Focus the next or the previous window"));
    v.push((
        format!("{m}arrows"),
        "Focus the window in that direction (add Shift to move it)",
    ));
    if c.launcher_enabled {
        v.push((format!("{m}Space"), "Find and open apps, files and more"));
    }
    v.push((format!("{m}Enter"), "Open your terminal"));
    let float = format!("{m}{}", k(c.key_toggle_float));
    v.push((float, "Float the window, or tile it again"));
    v.push((format!("{m}{}", k(c.key_close_window)), "Close the window"));
    if c.system_menu_enabled {
        v.push((
            format!("{m}Shift+Space"),
            "Lock, sleep, restart, and Astur's own menu",
        ));
    }
    v
}

/// Whether two commands start the same app: equal (case and outer quotes
/// aside), or each just a program with the same name (`pwsh`, `pwsh.exe`
/// and its full path are one app). Options after it make a command its own,
/// and an address never matches by name (Google and Bing both end in
/// `search?q=`). Two full paths match only whole: Chrome and Chrome Beta
/// are both `chrome.exe`, in different folders.
fn same_app(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim().trim_matches('"').to_ascii_lowercase();
    let program = |s: &str| -> Option<String> {
        if let Some(stem) = s.strip_suffix(".exe") {
            return Some(file_name(stem).to_string());
        }
        let bare = !s.is_empty() && !s.contains([' ', '.', '/', ':', '\\']);
        bare.then(|| s.to_string())
    };
    let has_dir = |s: &str| s.contains(['\\', '/']);
    let (a, b) = (norm(a), norm(b));
    a == b || (!(has_dir(&a) && has_dir(&b)) && program(&a).is_some_and(|p| program(&b) == Some(p)))
}

/// What a picker shows for `v`: its option's name; for an app picker a known
/// terminal's name or the program's file name; a path as itself; else the
/// value marked Custom.
fn choice_label(v: &str, opts: &[Choice], browse: Option<Pick>) -> String {
    if let Some(o) = opts.iter().find(|o| same_app(&o.command, v)) {
        return o.label.clone();
    }
    let plain = v.trim().trim_matches('"');
    match browse {
        _ if plain.is_empty() => "None".to_string(),
        Some(Pick::App) => match TERMINALS.iter().find(|t| same_app(t.1, v)) {
            Some(t) => t.0.to_string(),
            None if plain.to_ascii_lowercase().ends_with(".exe") => file_name(plain).to_string(),
            None => format!("Custom: {v}"),
        },
        Some(Pick::Folder) => plain.to_string(),
        _ => format!("Custom: {v}"),
    }
}

/// `%NAME%` replaced by `env(NAME)`; None when one is not set (that place
/// does not exist on this PC).
fn expand(template: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let mut parts = template.split('%');
    let mut out = parts.next()?.to_string();
    while let Some(name) = parts.next() {
        out.push_str(&env(name)?);
        out.push_str(parts.next()?);
    }
    Some(out)
}

/// The apps of `list` on this PC, as choices. One App Paths or PATH can
/// find (`on_path`) is written by name, as cmd's `start` finds it so; else
/// the first install place that exists, in full (Astur quotes a path).
fn find_apps(
    list: &[Known],
    on_path: &dyn Fn(&str) -> bool,
    env: &dyn Fn(&str) -> Option<String>,
    exists: &dyn Fn(&str) -> bool,
) -> Vec<Choice> {
    (list.iter())
        .filter_map(|&(label, exe, places)| {
            let command = if on_path(exe) {
                exe.to_string()
            } else {
                (places.iter()).find_map(|p| expand(p, env).filter(|p| exists(p)))?
            };
            Some(Choice::new(label, &command))
        })
        .collect()
}

/// The program of a registered command line: its quoted first part, or the
/// text up to `.exe`.
fn exe_of(cmd: &str) -> &str {
    let cmd = cmd.trim();
    if let Some(rest) = cmd.strip_prefix('"') {
        return rest.split('"').next().unwrap_or(rest);
    }
    // ASCII lowercasing keeps byte offsets.
    match cmd.to_ascii_lowercase().find(".exe") {
        Some(i) => &cmd[..i + 4],
        None => cmd,
    }
}

/// Registered browsers, (name, command), as choices: those whose program
/// is there, each program once (several hives list one), by name.
fn browser_choices(found: Vec<(String, String)>, exists: &dyn Fn(&str) -> bool) -> Vec<Choice> {
    let mut v: Vec<Choice> = Vec::new();
    for (label, cmd) in found {
        let exe = exe_of(&cmd);
        if exists(exe) && !v.iter().any(|c| c.command.eq_ignore_ascii_case(exe)) {
            v.push(Choice::new(&label, exe));
        }
    }
    v.sort_by_key(|c| c.label.to_lowercase());
    v
}

/// Terminals and browsers on this PC (registry, PATH and file checks).
fn detect_apps() -> Apps {
    let dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let exists = |p: &str| present(Path::new(p));
    let on_path = |exe: &str| {
        sys::app_path(exe).is_some_and(|p| exists(exe_of(&p)))
            || dirs.iter().any(|d| present(&d.join(exe)))
    };
    Apps {
        terminals: find_apps(&TERMINALS, &on_path, &|n| std::env::var(n).ok(), &exists),
        browsers: browser_choices(sys::browsers(), &exists),
    }
}

/// Whether anything is at `p`. Not `Path::exists`: a Store app's alias
/// (WindowsApps\wt.exe) is a link that can't be opened as a file, so
/// following it fails although the app is there.
fn present(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// The last part of a path: `C:\x\app.exe` is app.exe.
fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// `s` cut to `n` characters, with an ellipsis when cut.
fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    s.chars().take(n - 1).chain(['\u{2026}']).collect()
}

/// What a native dialog picks.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Pick {
    App,
    File,
    Folder,
    Image,
    Icon,
}

impl Pick {
    /// The dialog's title, and its file types as (name, patterns).
    fn dialog(self) -> (&'static str, &'static [(&'static str, &'static str)]) {
        const ALL: (&str, &str) = ("All files", "*.*");
        match self {
            Pick::App => ("Choose an app", &[("Apps", "*.exe;*.lnk;*.bat;*.cmd"), ALL]),
            Pick::File => ("Choose a file", &[]),
            Pick::Folder => ("Choose a folder", &[]),
            Pick::Image => (
                "Choose a picture",
                &[("Pictures", "*.jpg;*.jpeg;*.png;*.bmp;*.gif"), ALL],
            ),
            Pick::Icon => (
                "Choose an icon",
                &[("Icons and apps", "*.ico;*.png;*.exe"), ALL],
            ),
        }
    }

    /// A picker's menu item that opens the dialog.
    fn choose(self) -> String {
        format!("{}\u{2026}", self.dialog().0)
    }
}

/// A native dialog's answer: the field that asked, and the path. The dialog
/// runs on a thread of its own (it is modal and pumps its own messages), so
/// the field takes the answer on a later frame (`take_pick`).
static PICKED: std::sync::Mutex<Option<(egui::Id, String)>> = std::sync::Mutex::new(None);

/// A dialog is open. One at a time: it disables this window while it shows,
/// but a second click can land before it appears.
static PICKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Open the native dialog for field `id`, in the folder `start` names (or
/// the folder of the file it names), if any.
fn start_pick(ctx: &egui::Context, id: egui::Id, kind: Pick, start: &str) {
    use std::sync::atomic::Ordering::SeqCst;
    if PICKING.swap(true, SeqCst) {
        return;
    }
    let (ctx, start, owner) = (ctx.clone(), start.to_string(), sys::own_window());
    let spawned = std::thread::Builder::new()
        .name("picker".into())
        .spawn(move || {
            if let Some(path) = sys::file_dialog(owner, kind, &start) {
                let mut slot = PICKED.lock().unwrap_or_else(PoisonError::into_inner);
                *slot = Some((id, path));
            }
            PICKING.store(false, SeqCst);
            ctx.request_repaint();
        });
    if spawned.is_err() {
        PICKING.store(false, SeqCst);
    }
}

/// The path the dialog picked for field `id`, once.
fn take_pick(id: egui::Id) -> Option<String> {
    let mut slot = PICKED.lock().unwrap_or_else(PoisonError::into_inner);
    if slot.as_ref().is_some_and(|p| p.0 == id) {
        slot.take().map(|p| p.1)
    } else {
        None
    }
}

/// A Browse button (a folder icon: table rows are tight): the native
/// dialog's path lands in `v` a frame or so later. Returns true when it did.
fn browse_button(ui: &mut Ui, id: egui::Id, v: &mut String, kind: Pick) -> bool {
    let got = take_pick(id);
    if let Some(p) = &got {
        v.clone_from(p);
    }
    let icons = has_icons(ui);
    if ui
        .button(glyph(icons, ICON_FOLDER, "Browse\u{2026}"))
        .on_hover_text(kind.choose())
        .clicked()
    {
        start_pick(ui.ctx(), id, kind, v);
    }
    got.is_some()
}

/// A window open now, for "Pick an open window".
#[derive(Clone, Debug, Default, PartialEq)]
struct OpenWindow {
    title: String,
    class: String,
    /// Its program's full path ("" when Windows won't say: an elevated app).
    exe: String,
}

/// The open windows, listed again at most every 2 s while a menu shows them.
fn window_list(ctx: &egui::Context) -> std::sync::Arc<Vec<OpenWindow>> {
    type Cached = (f64, std::sync::Arc<Vec<OpenWindow>>);
    let id = egui::Id::new("astur-open-windows");
    let now = ctx.input(|i| i.time);
    if let Some((at, list)) = ctx.data(|d| d.get_temp::<Cached>(id)) {
        if now - at < 2.0 {
            return list;
        }
    }
    let list = std::sync::Arc::new(sys::open_windows());
    ctx.data_mut(|d| d.insert_temp(id, (now, list.clone())));
    list
}

/// Programs that show other apps' windows: their name alone would match all
/// of those (every Store app runs in ApplicationFrameHost.exe).
const HOSTS: [&str; 6] = [
    "applicationframehost.exe",
    "java.exe",
    "javaw.exe",
    "python.exe",
    "pythonw.exe",
    "mmc.exe",
];

/// A rule's (app, class, title) for a picked window: the app's file name,
/// and the title too where the app is a host. Not the class: one app's
/// windows differ in class (its dialogs), and the app is what people mean.
/// Only when Windows won't name the program is the class the match.
fn rule_fields(w: &OpenWindow) -> (String, String, String) {
    let exe = file_name(&w.exe).to_string();
    if exe.is_empty() {
        return (String::new(), w.class.clone(), String::new());
    }
    let host = HOSTS.iter().any(|h| exe.eq_ignore_ascii_case(h));
    let title = if host { w.title.clone() } else { String::new() };
    (exe, String::new(), title)
}

/// Store apps whose program can't be started by its path (WindowsApps is
/// locked), as (program, the command that starts it).
const PACKAGED: [(&str, &str); 1] = [("windowsterminal.exe", "wt.exe")];

/// The scratchpad's (command, window class) for a picked window: its
/// program starts it, unless that is a Store app's (a known one's command,
/// else the command stays as it is). A command that already starts the
/// picked app stays as it is too: its options (`wt -p Ubuntu`, WezTerm's
/// `--class scratch`, which the picked class depends on) are the point.
fn scratch_fields(w: &OpenWindow, command: &str) -> (String, String) {
    let name = file_name(&w.exe).to_ascii_lowercase();
    let locked = w.exe.is_empty() || w.exe.to_ascii_lowercase().contains(r"\windowsapps\");
    let cmd = match PACKAGED.iter().find(|p| p.0 == name) {
        Some(p) => p.1.to_string(),
        None if locked => command.to_string(),
        None => w.exe.clone(),
    };
    // exe_of returns all of an unquoted command with no .exe ("wt -p
    // Ubuntu"): its program is then the first word.
    let prog = exe_of(command);
    let prog = if prog.to_ascii_lowercase().ends_with(".exe") {
        prog
    } else {
        prog.split_whitespace().next().unwrap_or("")
    };
    let cmd = if same_app(prog, &cmd) {
        command.to_string()
    } else {
        cmd
    };
    (cmd, w.class.clone())
}

/// The window class a known scratchpad app always has, so its own window is
/// found even if another opens first; "" (the first window that opens after
/// the command) for the rest.
fn scratch_class(command: &str) -> &'static str {
    if same_app("wt.exe", command) {
        "CASCADIA_HOSTING_WINDOW_CLASS"
    } else {
        ""
    }
}

// ---- Windows -------------------------------------------------------------------

/// Every Win32 call Settings makes, all on demand or on its startup thread:
/// font families, a font sample, installed browsers and App Paths, the open
/// windows, and the file / folder dialog. This is a separate process from
/// the WM, so none of it can stall the input hooks.
mod sys {
    use super::{OpenWindow, Pick};
    use std::path::Path;
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, BOOL, COLORREF, HWND, LPARAM, SIZE};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, CreateFontIndirectW, DeleteDC, DeleteObject,
        EnumFontFamiliesExW, GdiFlush, GetDC, GetTextExtentPoint32W, ReleaseDC, SelectObject,
        SetBkMode, SetTextColor, TextOutW, ANTIALIASED_QUALITY, BITMAPINFO, BITMAPINFOHEADER,
        BI_RGB, DEFAULT_CHARSET, DIB_RGB_COLORS, LOGFONTW, TEXTMETRICW, TRANSPARENT,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    };
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER,
        HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_SZ,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{
        FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName,
        FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetForegroundWindow, GetWindow, GetWindowLongW, GetWindowTextW,
        GetWindowThreadProcessId, IsWindowVisible, GWL_EXSTYLE, GW_OWNER, WS_EX_TOOLWINDOW,
    };

    /// How many screens are connected: how many "Screen N" boxes the bar's
    /// screen picker offers. One call, no message sent, cheap every frame.
    pub fn screens() -> u32 {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS};
        // SAFETY: a plain query with a valid index.
        unsafe { GetSystemMetrics(SM_CMONITORS).max(1) as u32 }
    }

    /// Every font family GDI can make, by the name CreateFont takes: what
    /// the bar's and the popups' font names are looked up by.
    pub fn font_families() -> Vec<String> {
        unsafe extern "system" fn each(
            lf: *const LOGFONTW,
            _: *const TEXTMETRICW,
            _: u32,
            names: LPARAM,
        ) -> i32 {
            // SAFETY: GDI passes a valid LOGFONTW; `names` is the Vec below,
            // alive for the whole enumeration.
            let face = &(*lf).lfFaceName;
            let n = face.iter().position(|&c| c == 0).unwrap_or(face.len());
            let name = String::from_utf16_lossy(&face[..n]);
            // "@Name" is a CJK family's vertical-writing twin.
            if !name.starts_with('@') {
                (*(names.0 as *mut Vec<String>)).push(name);
            }
            1
        }
        let mut names: Vec<String> = Vec::new();
        // SAFETY: a screen DC, released below; the callback only pushes.
        unsafe {
            let dc = GetDC(None);
            let lf = LOGFONTW {
                lfCharSet: DEFAULT_CHARSET,
                ..Default::default()
            };
            let all = LPARAM(&mut names as *mut Vec<String> as isize);
            EnumFontFamiliesExW(dc, &lf, Some(each), all, 0);
            ReleaseDC(None, dc);
        }
        names.sort_by_key(|n| n.to_lowercase());
        names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        names
    }

    /// `text` in font `name` at `px` pixels as GDI draws it, white on black
    /// with grayscale smoothing, so a pixel's brightness is the text's
    /// coverage: (width, height, coverage per pixel).
    pub fn font_sample(name: &str, px: i32, text: &str) -> Option<(usize, usize, Vec<u8>)> {
        let mut face = [0u16; 32];
        for (d, s) in face.iter_mut().zip(name.encode_utf16().take(31)) {
            *d = s;
        }
        let wide: Vec<u16> = text.encode_utf16().collect();
        let lf = LOGFONTW {
            lfHeight: -px,
            lfWeight: 400,
            lfCharSet: DEFAULT_CHARSET,
            lfQuality: ANTIALIASED_QUALITY,
            lfFaceName: face,
            ..Default::default()
        };
        // SAFETY: every GDI object made here is selected out and deleted
        // before returning; the DIB's bits are read while it is alive.
        unsafe {
            let font = CreateFontIndirectW(&lf);
            if font.is_invalid() {
                return None;
            }
            let dc = CreateCompatibleDC(None);
            let old_font = SelectObject(dc, font);
            let mut size = SIZE::default();
            let _ = GetTextExtentPoint32W(dc, &wide, &mut size);
            let (w, h) = (size.cx.clamp(1, 2000), size.cy.clamp(1, 400));
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down rows
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let out = match CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(bmp) if !bits.is_null() => {
                    let old_bmp = SelectObject(dc, bmp);
                    SetTextColor(dc, COLORREF(0x00FF_FFFF));
                    SetBkMode(dc, TRANSPARENT);
                    let _ = TextOutW(dc, 0, 0, &wide);
                    let _ = GdiFlush();
                    let len = (w * h * 4) as usize;
                    let px = std::slice::from_raw_parts(bits as *const u8, len);
                    let alpha = (px.as_chunks::<4>().0.iter())
                        .map(|p| p[0].max(p[1]).max(p[2]))
                        .collect();
                    SelectObject(dc, old_bmp);
                    let _ = DeleteObject(bmp);
                    Some((w as usize, h as usize, alpha))
                }
                _ => None,
            };
            SelectObject(dc, old_font);
            let _ = DeleteObject(font);
            let _ = DeleteDC(dc);
            out
        }
    }

    /// A key's default value as text (REG_EXPAND_SZ comes expanded).
    unsafe fn reg_default(root: HKEY, path: &str) -> Option<String> {
        let mut buf = vec![0u16; 2048];
        let mut cb = (buf.len() * 2) as u32;
        let data = Some(buf.as_mut_ptr().cast());
        let path = HSTRING::from(path);
        RegGetValueW(
            root,
            &path,
            PCWSTR::null(),
            RRF_RT_REG_SZ,
            None,
            data,
            Some(&mut cb),
        )
        .ok()
        .ok()?;
        // Up to the terminator, not by `cb`: for an expanded REG_EXPAND_SZ
        // it is the stored (unexpanded) size, which can be longer.
        let n = (cb as usize / 2).min(buf.len());
        let n = buf[..n].iter().position(|&c| c == 0).unwrap_or(n);
        Some(String::from_utf16_lossy(&buf[..n]))
    }

    /// The names of a key's subkeys.
    unsafe fn reg_subkeys(root: HKEY, path: &str) -> Vec<String> {
        let mut key = HKEY::default();
        let mut out = Vec::new();
        if RegOpenKeyExW(root, &HSTRING::from(path), 0, KEY_READ, &mut key).is_err() {
            return out;
        }
        for i in 0.. {
            let mut buf = [0u16; 256];
            let mut n = buf.len() as u32;
            let name = PWSTR(buf.as_mut_ptr());
            if RegEnumKeyExW(key, i, name, &mut n, None, PWSTR::null(), None, None).is_err() {
                break;
            }
            out.push(String::from_utf16_lossy(&buf[..n as usize]));
        }
        let _ = RegCloseKey(key);
        out
    }

    /// Where App Paths puts `exe` (how Run and `start` find an app that is
    /// not on PATH), if it is registered.
    pub fn app_path(exe: &str) -> Option<String> {
        let key = format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{exe}");
        // SAFETY: plain registry reads into owned buffers.
        [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
            .into_iter()
            .find_map(|root| unsafe { reg_default(root, &key) })
    }

    /// Installed browsers as (name, command line): every browser registers
    /// itself under Clients\StartMenuInternet for Windows' Default apps.
    pub fn browsers() -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (root, base) in [
            (HKEY_CURRENT_USER, r"SOFTWARE\Clients\StartMenuInternet"),
            (HKEY_LOCAL_MACHINE, r"SOFTWARE\Clients\StartMenuInternet"),
            (
                HKEY_LOCAL_MACHINE,
                r"SOFTWARE\WOW6432Node\Clients\StartMenuInternet",
            ),
        ] {
            // SAFETY: plain registry reads into owned buffers.
            unsafe {
                for sub in reg_subkeys(root, base) {
                    let key = format!(r"{base}\{sub}");
                    let Some(cmd) = reg_default(root, &format!(r"{key}\shell\open\command")) else {
                        continue;
                    };
                    let name = reg_default(root, &key).filter(|n| !n.trim().is_empty());
                    out.push((name.unwrap_or(sub), cmd));
                }
            }
        }
        out
    }

    /// The app windows open now, front to back: visible, unowned, titled,
    /// not tool windows, not cloaked (a Store app's hidden frame), not this
    /// one's or the desktop's.
    pub fn open_windows() -> Vec<OpenWindow> {
        unsafe extern "system" fn each(hwnd: HWND, list: LPARAM) -> BOOL {
            // SAFETY: `list` is the Vec below, alive for the enumeration.
            if let Some(w) = app_window(hwnd) {
                (*(list.0 as *mut Vec<OpenWindow>)).push(w);
            }
            BOOL(1)
        }
        let mut list: Vec<OpenWindow> = Vec::new();
        // SAFETY: the callback only reads window properties and pushes.
        unsafe {
            let _ = EnumWindows(Some(each), LPARAM(&mut list as *mut _ as isize));
        }
        list
    }

    unsafe fn app_window(hwnd: HWND) -> Option<OpenWindow> {
        if !IsWindowVisible(hwnd).as_bool()
            || GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.0.is_null())
        {
            return None;
        }
        if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
            return None;
        }
        let mut cloaked = 0u32;
        let at = &mut cloaked as *mut u32 as *mut _;
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, at, 4);
        if cloaked != 0 {
            return None;
        }
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(hwnd, &mut buf).max(0) as usize;
        let title = String::from_utf16_lossy(&buf[..n]);
        let n = GetClassNameW(hwnd, &mut buf).max(0) as usize;
        let class = String::from_utf16_lossy(&buf[..n]);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let desktop = matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd");
        if title.trim().is_empty() || desktop || pid == std::process::id() {
            return None;
        }
        Some(OpenWindow {
            title,
            class,
            exe: process_exe(pid).unwrap_or_default(),
        })
    }

    unsafe fn process_exe(pid: u32) -> Option<String> {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let name = PWSTR(buf.as_mut_ptr());
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, name, &mut len);
        let _ = CloseHandle(process);
        ok.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }

    /// This process's window when it is in front (it is when a button was
    /// just clicked), to own a dialog: it stays above Settings, and Astur
    /// never tiles an owned window. 0 = none.
    pub fn own_window() -> isize {
        // SAFETY: reads only.
        unsafe {
            let fg = GetForegroundWindow();
            let mut pid = 0u32;
            GetWindowThreadProcessId(fg, Some(&mut pid));
            if pid == std::process::id() {
                fg.0 as isize
            } else {
                0
            }
        }
    }

    /// The native file or folder dialog (modal to `owner`); the path picked,
    /// or None when cancelled. Runs COM on the calling thread, which must be
    /// one of its own (the dialog pumps messages until it closes).
    pub fn file_dialog(owner: isize, kind: Pick, start: &str) -> Option<String> {
        let (title, types) = kind.dialog();
        // SAFETY: COM is set up for this thread and torn down after; every
        // interface is released when it goes out of scope, before that.
        unsafe {
            let com =
                CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok();
            let path = (|| -> windows::core::Result<Option<String>> {
                let dialog: IFileOpenDialog =
                    CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
                let mut options = dialog.GetOptions()? | FOS_FORCEFILESYSTEM;
                if kind == Pick::Folder {
                    options |= FOS_PICKFOLDERS;
                }
                dialog.SetOptions(options)?;
                dialog.SetTitle(&HSTRING::from(title))?;
                let names: Vec<(HSTRING, HSTRING)> = (types.iter())
                    .map(|(n, s)| (HSTRING::from(*n), HSTRING::from(*s)))
                    .collect();
                let specs: Vec<COMDLG_FILTERSPEC> = (names.iter())
                    .map(|(n, s)| COMDLG_FILTERSPEC {
                        pszName: PCWSTR(n.as_ptr()),
                        pszSpec: PCWSTR(s.as_ptr()),
                    })
                    .collect();
                if !specs.is_empty() {
                    dialog.SetFileTypes(&specs)?;
                }
                let start = Path::new(start.trim().trim_matches('"'));
                let dir = if start.is_dir() {
                    Some(start)
                } else {
                    start.parent().filter(|p| p.is_dir())
                };
                if let Some(dir) = dir.filter(|d| d.is_absolute()) {
                    let item: windows::core::Result<IShellItem> =
                        SHCreateItemFromParsingName(&HSTRING::from(dir), None);
                    if let Ok(item) = item {
                        let _ = dialog.SetFolder(&item);
                    }
                }
                dialog.Show(HWND(owner as *mut _))?;
                let picked = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
                let text = picked.to_string().ok();
                CoTaskMemFree(Some(picked.0 as *const _));
                Ok(text)
            })();
            if com {
                CoUninitialize();
            }
            path.ok().flatten()
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use astur_config::parse_text;

    /// Two config files in a fresh temp folder; never the user's real ones.
    fn temp_paths(name: &str, wm: &str, nav: &str) -> [PathBuf; 2] {
        let dir =
            std::env::temp_dir().join(format!("astur-settings-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = [dir.join("astur.conf"), dir.join("navbar.conf")];
        std::fs::write(&paths[0], wm).unwrap();
        std::fs::write(&paths[1], nav).unwrap();
        paths
    }

    #[test]
    fn save_writes_only_what_changed() {
        // Every save used to write ~155 keys: a hand-written file gained ~150
        // lines and edits made while Settings was open were reverted.
        for (wm, nav) in [
            (default_config_text(), default_navbar_text()),
            ("# mine\nouter_gap = 120\n", "height = 30\n"),
        ] {
            let disk = parse_pair(wm, nav);
            let (mut c, mut m) = (disk.clone(), Mirrors::from(&disk));
            // Folding untouched mirrors back is no edit.
            fold_mirrors(&mut c, &mut m);
            let (w, n) = diff(&c, &disk);
            assert!(w.is_empty() && n.is_empty(), "{w:?} {n:?}");
            c.inner_gap = 4;
            let (w, n) = diff(&c, &disk);
            assert_eq!(w, [("inner_gap", "4".to_string())]);
            assert!(n.is_empty());
        }
    }

    #[test]
    fn drawing_a_slider_keeps_an_out_of_range_value() {
        // egui's default clamping rewrote a value just by drawing its slider:
        // opening Layout & gaps and saving turned outer_gap = 120 into 100.
        let mut gap = 120;
        let _ = egui::Context::default().run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add(slider(&mut gap, 0..=100));
            });
        });
        assert_eq!(gap, 120);
    }

    #[test]
    fn drawing_every_page_changes_nothing() {
        // With instant apply, a widget that rewrote its value on draw would be
        // written to disk without the user touching anything.
        let wm = "outer_gap = 120\nanimation_ms = 900\nunfocused_opacity = 0.855\nworkspace_slide = false\nbar_heigth = 3\nmonitor_layouts = , scrolling\n\
            window_rules = float||||2| ;; ignore|x.exe|||99|\nignore_classes = A\n\
            extra_hotkeys = CTRL+Q|launch|x ;; ALT+1|bogus| ;; ALT+J|switch_workspace|0 ;; ALT+F2|layout|nope\n\
            system_power_items = lock, bogus\nlauncher_entries = a|https://x| ;; b|cmd:|\n\
            workspaces = 3\nworkspace_keys = 1 1 J 4\n";
        let nav = "height = 90\nclock_format = h:mm tt\nbg = #123456\nshape = pills\n\
            widget_colors = clock:#FF8800 cpu:teal\nfont_name = No Such Font\n\
            monitors = 3 7\nworkspace_label = name\nworkspace_apps = true\nseparator_style = powerline\n\
            clock_click = launch C:\\x y\\app.exe\ncpu_right_click = switch_workspace 99\n\
            right = clock cpu custom disk keyboard logo media_controls\ncustom_command = echo hi\n\
            custom_interval = 7\ndisk_drive = Q:\\Games\ndate_format = Today\n";
        let mut app = App::load(temp_paths("draw", wm, nav));
        app.bar_sel = Some(Src::Zone(2, 0));
        let ctx = egui::Context::default();
        setup_look(&ctx);
        let pages: Vec<Page> = NAV.iter().map(|n| n.0).collect();
        for page in pages {
            app.page = page;
            for _ in 0..2 {
                let out = ctx.run(egui::RawInput::default(), |ctx| app.ui(ctx));
                assert!(!id_clash(&out), "{page:?} has an egui id clash");
            }
            assert!(!app.pending(), "{page:?} changed a value by drawing it");
        }
        // Search draws every page into one view, where ids derived from a
        // label would clash (the critic's point): explicit id_salt avoids it.
        for query in ["a", "e", "gap", "zzzz-nothing"] {
            app.query = query.to_string();
            let out = ctx.run(egui::RawInput::default(), |ctx| app.ui(ctx));
            assert!(!id_clash(&out), "search {query:?} has an egui id clash");
            assert!(!app.pending(), "search {query:?} changed a value");
        }
        assert!(app.history.undo.is_empty());
    }

    #[test]
    fn id_clash_detector_sees_a_clash() {
        let out = egui::Context::default().run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for _ in 0..2 {
                    let mut s = String::new();
                    ui.add(egui::TextEdit::singleline(&mut s).id(egui::Id::new("same")));
                }
            });
        });
        assert!(id_clash(&out));
    }

    /// egui (debug builds) paints a "\u{1F525} ... use of widget id" warning
    /// for every id used twice in a frame.
    fn id_clash(out: &egui::FullOutput) -> bool {
        fn walk(s: &egui::Shape) -> bool {
            match s {
                egui::Shape::Text(t) => t.galley.text().contains('\u{1F525}'),
                egui::Shape::Vec(v) => v.iter().any(walk),
                _ => false,
            }
        }
        out.shapes.iter().any(|c| walk(&c.shape))
    }

    #[test]
    fn every_key_the_gui_writes_has_a_row() {
        // The rebuild moved every control to a new page: prove none was lost,
        // and that searching a conf key finds the row that edits it.
        let mut app = App::load(temp_paths("keys", "", ""));
        let ctx = egui::Context::default();
        setup_look(&ctx);
        let (wm, nav) = conf_pairs(&app.cfg);
        for (key, _) in wm.into_iter().chain(nav) {
            if key == "workspace_slide" {
                continue; // legacy: only the Motion page's Fix writes it
            }
            app.query = key.to_string();
            let _ = ctx.run(egui::RawInput::default(), |ctx| app.ui(ctx));
            assert!(app.find.seen.iter().any(|k| k == key), "no row edits {key}");
        }
    }

    #[test]
    fn edits_apply_after_the_debounce_and_undo_writes_back() {
        let paths = temp_paths("apply", "# mine\nouter_gap = 10\n", "height = 30\n");
        let mut app = App::load(paths.clone());
        let t0 = Instant::now();
        app.cfg.inner_gap = 4;
        // Mid-gesture nothing is written, and the clock restarts after it.
        assert_eq!(app.settle(t0, true), None);
        assert!(app.settle(t0, false).is_some());
        assert!(app.settle(t0 + DEBOUNCE / 2, false).is_some());
        // A held key is busy too (ui()): a write is due DEBOUNCE after the
        // last busy frame, not DEBOUNCE after the first step.
        let ms = Duration::from_millis;
        assert_eq!(app.settle(t0 + ms(200), true), None);
        assert!(app.settle(t0 + ms(400), false).is_some());
        let read = || std::fs::read_to_string(&paths[0]).unwrap();
        assert_eq!(read(), "# mine\nouter_gap = 10\n");
        assert_eq!(app.settle(t0 + ms(400) + DEBOUNCE, false), None);
        assert_eq!(read(), "# mine\nouter_gap = 10\ninner_gap = 4\n");
        app.undo();
        assert_eq!(app.cfg.inner_gap, Config::defaults().inner_gap);
        app.redo();
        assert_eq!(read(), "# mine\nouter_gap = 10\ninner_gap = 4\n");
        // Someone edits the file meanwhile: the next write keeps their line,
        // Settings takes it in, and Undo can no longer reach past it.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&paths[0], "# mine\nouter_gap = 33\ninner_gap = 4\n").unwrap();
        app.cfg.layout = "grid".to_string();
        app.commit();
        assert_eq!(
            read(),
            "# mine\nouter_gap = 33\ninner_gap = 4\nlayout = grid\n"
        );
        assert_eq!(app.cfg.outer_gap, 33);
        assert!(app.error.is_none(), "{:?}", app.error);
        assert!(!app.can_undo());
        app.cfg.layout = "master".to_string();
        app.commit();
        app.undo();
        assert_eq!(
            read(),
            "# mine\nouter_gap = 33\ninner_gap = 4\nlayout = grid\n"
        );
        app.redo();
        assert!(read().contains("layout = master"));
        app.undo_all();
        assert_eq!(app.cfg.layout, "grid");
        assert!(read().contains("layout = grid"));
        // The navbar file was never touched.
        assert_eq!(std::fs::read_to_string(&paths[1]).unwrap(), "height = 30\n");
    }

    #[test]
    fn a_value_overridden_elsewhere_is_reported() {
        // navbar.conf sets the WM key too and is read last, so astur.conf's
        // line never sticks; say so instead of claiming it applied.
        let paths = temp_paths("stick", "outer_gap = 10\n", "outer_gap = 50\n");
        let mut app = App::load(paths.clone());
        app.cfg.outer_gap = 20;
        app.commit();
        assert!(app
            .error
            .as_deref()
            .is_some_and(|e| e.contains("outer_gap")));
        assert!(app.toast.is_none(), "not \"Applied\" beside the error");
        // The user drops the other line and reloads: the banner goes.
        std::fs::write(&paths[1], "").unwrap();
        app.reload();
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.cfg.outer_gap, 20);
    }

    #[test]
    fn read_only_writes_nothing() {
        let paths = temp_paths("ro", "", "");
        std::fs::write(&paths[0], [0xFF, 0xFE, b'a', 0]).unwrap(); // UTF-16
        let mut app = App::load(paths.clone());
        assert!(app.read_error.is_some());
        app.cfg.inner_gap = 3;
        app.commit();
        assert_eq!(std::fs::read(&paths[0]).unwrap(), [0xFF, 0xFE, b'a', 0]);
        assert!(
            app.toast.is_none(),
            "no \"Applied\" for what was not written"
        );
    }

    #[test]
    fn undo_adopts_an_outside_edit_instead_of_writing_over_it() {
        let paths = temp_paths("outside", "outer_gap = 10\n", "");
        let mut app = App::load(paths.clone());
        let read = || std::fs::read_to_string(&paths[0]).unwrap();
        let hand_edit = |text: &str| {
            std::thread::sleep(Duration::from_millis(20)); // a newer mtime
            std::fs::write(&paths[0], text).unwrap();
        };
        app.cfg.outer_gap = 20;
        app.commit();
        assert_eq!(read(), "outer_gap = 20\n");
        assert!(!paths[0].with_file_name("astur.conf.tmp").exists());
        hand_edit("outer_gap = 24\n");
        app.undo();
        assert_eq!(read(), "outer_gap = 24\n", "the hand edit stays");
        assert_eq!(app.cfg.outer_gap, 24);
        assert!(!app.can_undo());
        // Undo all takes the same guard (Redo too).
        app.cfg.inner_gap = 3;
        app.commit();
        hand_edit("outer_gap = 30\ninner_gap = 3\n");
        app.undo_all();
        assert_eq!(read(), "outer_gap = 30\ninner_gap = 3\n");
        assert_eq!(app.cfg.outer_gap, 30);
    }

    #[test]
    fn a_failed_atomic_write_leaves_no_temp_file() {
        // A directory can't be replaced by a file: the rename fails after
        // the temp file was written.
        let dir = temp_paths("atomic", "", "")[0].with_file_name("sub");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(write_atomic(&dir, "x").is_err());
        assert!(!dir.with_file_name("sub.tmp").exists());
        assert!(dir.is_dir());
    }

    #[test]
    fn a_listening_key_chip_keeps_ctrl_z() {
        let paths = temp_paths("chip-z", "outer_gap = 10\n", "");
        let mut app = App::load(paths);
        app.cfg.outer_gap = 20;
        app.commit();
        let ctx = egui::Context::default();
        let ctrl_z = || egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND | egui::Modifiers::CTRL,
            }],
            ..Default::default()
        };
        ctx.data_mut(|d| d.insert_temp(capture_id(), egui::Id::new("chip")));
        let _ = ctx.run(ctrl_z(), |ctx| app.keys(ctx));
        assert_eq!(app.cfg.outer_gap, 20, "Ctrl+Z went to the chip");
        ctx.data_mut(|d| d.remove::<egui::Id>(capture_id()));
        let _ = ctx.run(ctrl_z(), |ctx| app.keys(ctx));
        assert_eq!(app.cfg.outer_gap, 10);
    }

    #[test]
    fn history_undoes_and_redoes_in_order() {
        let mut h = History::default();
        h.record(1);
        h.record(2);
        assert_eq!(h.undo(3), Some(2));
        assert_eq!(h.undo(2), Some(1));
        assert_eq!(h.undo(1), None);
        assert_eq!(h.redo(1), Some(2));
        h.record(2); // a new edit drops what could be redone
        assert_eq!(h.redo(5), None);
    }

    #[test]
    fn motion_presets_map_exactly() {
        let mut c = Config::defaults();
        assert_eq!(motion_preset(&c), Some("smooth"));
        assert_eq!(
            motion_preset(&astur_config::default_config()),
            Some("smooth")
        );
        for &(id, ..) in &MOTION_OPTS {
            set_motion(&mut c, id);
            assert_eq!(motion_preset(&c), Some(id));
        }
        set_motion(&mut c, "springy");
        c.animation_ms += 1;
        assert_eq!(motion_preset(&c), None); // Custom, left alone
        set_motion(&mut c, "off");
        assert!(!c.animations);
        assert_eq!(c.animation_ms, 221, "Off keeps the style");
    }

    #[test]
    fn dim_percent_round_trips_opacity() {
        assert_eq!(dim_pct(Config::defaults().unfocused_opacity), 20);
        assert_eq!(dim_pct(1.0), 0);
        assert_eq!(dim_pct(0.1), 90);
        for p in 0..=90 {
            assert_eq!(dim_pct(opacity_for_dim(p)), p);
        }
    }

    #[test]
    fn template_keys_skip_prose_and_value_tables() {
        let keys = known_keys().concat();
        for k in [
            "outer_gap",
            "async_window_pos",
            "workspace_slide",
            "height",
            "clock_24h",
        ] {
            assert!(keys.iter().any(|x| x == k), "{k} missing");
        }
        for k in ["off", "dark", "icon", "dwindle", "per_monitor", "never"] {
            assert!(!keys.iter().any(|x| x == k), "{k} is not a key");
        }
        // Every scanned key is one the parser understands.
        let all: String = keys.iter().map(|k| format!("{k} = 1\n")).collect();
        assert!(parse_text(&all).unknown_keys.is_empty());
        assert_eq!(conf_file("height"), "navbar.conf");
        assert_eq!(conf_file("outer_gap"), "astur.conf");
        // One row, two files: each key under its own.
        assert_eq!(
            key_files("enabled, yasb_enabled"),
            "navbar.conf \u{B7} enabled; astur.conf \u{B7} yasb_enabled"
        );
    }

    #[test]
    fn did_you_mean_finds_typos_only() {
        assert_eq!(edit_distance("heigth", "height"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(did_you_mean("heigth"), Some("height"));
        assert_eq!(did_you_mean("outer_gp"), Some("outer_gap"));
        assert_eq!(did_you_mean("Outer_Gap"), Some("outer_gap"));
        assert_eq!(
            did_you_mean("focus_follow_mouse"),
            Some("focus_follows_mouse")
        );
        assert_eq!(did_you_mean("bar_heigth"), Some("height"));
        assert_eq!(did_you_mean("completely_unrelated"), None);
    }

    #[test]
    fn edit_key_renames_and_removes_lines() {
        let text = "\u{FEFF}outer_gp = 20\n# outer_gp = 1\nouter_gap = 8\nx = 1\n";
        assert_eq!(
            edit_key(text, "outer_gp", None),
            "# outer_gp = 1\nouter_gap = 8\nx = 1\n"
        );
        // The renamed value replaces the existing key, so it is the one read.
        let renamed = edit_key(text, "outer_gp", Some("outer_gap"));
        assert_eq!(renamed, "# outer_gp = 1\nouter_gap = 20\nx = 1\n");
        assert_eq!(parse_text(&renamed).outer_gap, 20);
        assert_eq!(edit_key("a = 1", "b", Some("c")), "a = 1");
        // What the Rename button names: the working line it replaces.
        assert_eq!(line_value(text, "outer_gap").as_deref(), Some("8"));
        assert_eq!(line_value(text, "outer_gp").as_deref(), Some("20"));
        assert_eq!(line_value(text, "x_y"), None);
    }

    /// Every table folded into a Config.
    fn fold_all(r: &Rows, c: &mut Config) {
        fold_keys(&r.keys, c);
        fold_ws(&r.ws, c);
        fold_rules(&r.rules, c);
        fold_entries(&r.entries, c);
        fold_actions(&r.actions, c);
        fold_items(&r.power, &mut c.system_power_items);
        fold_items(&r.setup, &mut c.system_setup_items);
        fold_hotkeys(&r.hotkeys, c);
    }

    /// The astur.conf text the GUI would write for `c`.
    fn wm_text(c: &Config) -> String {
        conf_pairs(c)
            .0
            .iter()
            .map(|(k, v)| format!("{k} = {v}\n"))
            .collect()
    }

    #[test]
    fn tables_round_trip_through_conf_text() {
        // Escaped separators, prefixes, defaults the parser fills in (no
        // group = Custom), old spellings and positional workspace slots.
        let wm = r"window_rules = float|Teams.exe|||2| ;; ignore||Chrome_\|Widget|* - Notes||3
ignore_classes = TaskManagerWindow
float_classes = #32770, MsiDialogCloseClass
launcher_entries = Notes|C:\notes.txt|file ;; Docs|url:https://x.y/a\;b|web ;; Term|cmd:wt -p Ubuntu|terminal ;; Proj|C:\Projects|auto ;; Bare|https://example.com|
system_actions = Dev|Code|cmd:code .|command|true ;; |Lock it|C:\lock.bat||false
system_power_items = lock, signout, shut_down
system_setup_items = settings, wallpaper
extra_hotkeys = ALT+GRAVE|scratchpad| ;; ALT+SHIFT+F2|layout|grid ;; ALT+CTRL+Q|launch|wt \| more ;; ALT+9|switch_workspace|3 ;; ALT+X|close_window|
workspaces = 4
workspace_keys = Q W E R T
workspace_names = Web, Code
workspace_icons = A, B, C
workspace_wallpapers = C:\a,b.jpg ;; ;; D:\c.png
key_close_window = F4
";
        let c = parse_text(wm);
        assert!(c.unknown_keys.is_empty(), "{:?}", c.unknown_keys);
        let rows = Rows::from(&c);
        assert_eq!(rows.problems(c.workspaces), 0);
        assert_eq!(rows.rules.len(), 5, "rules, then the class lists");
        assert_eq!(rows.rules[1].rule.class, "Chrome_|Widget");
        assert_eq!(rows.entries[1].kind, Kind::Url);
        assert_eq!(rows.entries[1].value, "https://x.y/a;b");
        assert_eq!(rows.entries[2].kind, Kind::Command);
        assert_eq!(rows.entries[0].kind, Kind::File);
        assert_eq!(rows.entries[3].kind, Kind::Folder);
        assert_eq!(rows.ws.len(), 5, "a key past the count keeps its row");
        // Rows -> Config: what the WM reads, starting from other values.
        let mut folded = Config::defaults();
        folded.workspaces = c.workspaces;
        fold_all(&rows, &mut folded);
        assert_eq!(folded.window_rules, c.window_rules);
        assert_eq!(folded.ignore_classes, c.ignore_classes);
        assert_eq!(folded.float_classes, c.float_classes);
        // A bare web address is kept as typed (the WM keys launcher history
        // by the text); a prefixed one with :// is written bare, same meaning.
        for i in [0, 2, 3, 4] {
            assert_eq!(folded.launcher_entries[i], c.launcher_entries[i]);
        }
        assert_eq!(folded.launcher_entries[1].target, "https://x.y/a;b");
        assert_eq!(join_target(Kind::Url, "example.com"), "url:example.com");
        assert_eq!(folded.system_actions, c.system_actions);
        assert_eq!(folded.system_actions[1].category, "Custom");
        assert_eq!(folded.extra_hotkeys, c.extra_hotkeys);
        assert_eq!(folded.system_power_items, ["lock", "sign_out", "shutdown"]);
        assert_eq!(folded.system_setup_items, ["settings", "wallpapers"]);
        assert_eq!(folded.workspace_keys, c.workspace_keys);
        assert_eq!(folded.workspace_names, c.workspace_names);
        assert_eq!(folded.workspace_icons, c.workspace_icons);
        assert_eq!(folded.workspace_wallpapers, c.workspace_wallpapers);
        assert_eq!(folded.key_close_window, 0x73);
        // -> conf text -> rows: the same table comes back.
        let back = parse_text(&wm_text(&folded));
        assert!(back.unknown_keys.is_empty());
        assert_eq!(Rows::from(&back), Rows::from(&folded));
        assert_eq!(Rows::from(&back), rows);

        // A new row with every separator in it survives the trip.
        let mut r = Rows::from(&Config::defaults());
        r.rules.push(RuleRow {
            list: false,
            rule: WindowRule {
                action: "tile".into(),
                exe: r"C:\Apps\x;;y.exe".into(),
                class: "a|b".into(),
                title: r"ends in \".into(),
                workspace: Some(1),
                monitor: None,
            },
        });
        r.hotkeys.push(HotkeyDef {
            chord: "ALT+CTRL+SHIFT+EQUAL".into(),
            action: "launch".into(),
            argument: "cmd /c echo a;;b | sort".into(),
        });
        let mut c = Config::defaults();
        fold_all(&r, &mut c);
        assert_eq!(Rows::from(&parse_text(&wm_text(&c))), r);
        // One wallpaper whose path has a comma is not split in two.
        let one = vec![r"C:\a,b.jpg".to_string()];
        let text = format!("workspace_wallpapers = {}", slots(&one));
        assert_eq!(parse_text(&text).workspace_wallpapers[0], one[0]);
    }

    #[test]
    fn invalid_rows_stay_and_are_never_written() {
        let c0 = parse_text("extra_hotkeys = CTRL+Q|launch|x ;; ALT+1|bogus|\nsystem_power_items = lock, bogus\nworkspace_names = A, B, C\n");
        let mut r = Rows::from(&c0);
        // What the WM silently ignores shows as a problem, not as nothing.
        assert!(hotkey_check(&r.hotkeys[0]).is_err_and(|e| e.contains("no Astur key")));
        assert!(hotkey_check(&r.hotkeys[1]).is_err_and(|e| e.contains("bogus")));
        assert!(item_check(&r.power[1]).is_err());
        assert_eq!(r.problems(c0.workspaces), 3);
        let mut c = c0.clone();
        // An edit elsewhere in a list with a bad row writes nothing.
        r.hotkeys.push(HotkeyDef {
            chord: "ALT+F5".into(),
            action: "reload".into(),
            argument: String::new(),
        });
        r.power[0].1 = false;
        fold_hotkeys(&r.hotkeys, &mut c);
        fold_items(&r.power, &mut c.system_power_items);
        assert!(c == c0);
        // Unticking the unknown item drops it, on purpose.
        r.power[1].1 = false;
        fold_items(&r.power, &mut c.system_power_items);
        assert!(c.system_power_items.is_empty());
        // A fresh rule matches nothing yet, so it is not applied to everything.
        r.rules.push(RuleRow {
            list: false,
            rule: WindowRule {
                action: "float".into(),
                exe: " ".into(),
                class: String::new(),
                title: String::new(),
                workspace: None,
                monitor: None,
            },
        });
        fold_rules(&r.rules, &mut c);
        assert!(c.window_rules.is_empty());
        // A blank name between named workspaces would shift the rest: it is
        // held back, while a key change in the same table still applies.
        r.ws[1].name.clear();
        r.ws[0].key = "Q".into();
        fold_ws(&r.ws, &mut c);
        assert_eq!(c.workspace_names, ["A", "B", "C"]);
        assert_eq!(c.workspace_keys[0], 0x51);
        assert!(ws_problems(&r.ws, c.workspaces)[1].is_some());
        // A cleared bind keeps the old key.
        r.keys[0].clear();
        fold_keys(&r.keys, &mut c);
        assert_eq!(c.key_focus_next, Config::defaults().key_focus_next);
    }

    #[test]
    fn hidden_workspaces_never_block_a_column() {
        // Default keys 1..0, workspace 10 removed (hidden, key 0), then 9's
        // key cleared: the hidden 0 used to make 9 red ("clear the later
        // ones", which can't be seen) and hold back every key change.
        let mut c = Config::defaults();
        let mut r = Rows::from(&c);
        assert_eq!(r.ws.len(), 10);
        c.workspaces = 9;
        fold_ws(&r.ws, &mut c);
        assert_eq!(c.workspace_keys.len(), 10, "a hidden row keeps its key");
        r.ws[8].key.clear();
        r.ws[0].key = "Q".into();
        assert!(ws_problems(&r.ws, 9).iter().all(Option::is_none));
        assert_eq!(r.problems(9), 0);
        fold_ws(&r.ws, &mut c);
        // Past the gap the hidden key can't be written in order: dropped.
        assert_eq!(c.workspace_keys.len(), 8);
        assert_eq!(c.workspace_keys[0], 0x51);
        // Shown, the same gap is the user's to fix.
        assert!(ws_problems(&r.ws, 10)[8].is_some());
        // A hidden value the file can't take stops the list there.
        r.ws[8].key = "9".into();
        r.ws[9].key = "?".into();
        assert_eq!(ws_column(&r.ws, 0, 9).map(|k| k.len()), Ok(9));
        // Wallpaper slots may skip one, shown or hidden.
        r.ws[9].wallpaper = "w.png".into();
        let w = ws_column(&r.ws, 3, 9).unwrap();
        assert_eq!((w.len(), w[9].as_str()), (10, "w.png"));
    }

    #[test]
    fn a_red_row_survives_commits_and_is_written_once_fixed() {
        let paths = temp_paths("rows", "# mine\n", "");
        let mut app = App::load(paths.clone());
        let read = || std::fs::read_to_string(&paths[0]).unwrap();
        app.mir.rows.entries.push(EntryRow {
            label: "Notes".into(),
            kind: Kind::File,
            value: String::new(),
            icon: "auto".into(),
        });
        fold_entries(&app.mir.rows.entries, &mut app.cfg);
        app.commit();
        assert_eq!(read(), "# mine\n", "an invalid row is never written");
        assert_eq!(app.mir.rows.entries.len(), 1, "and never dropped");
        assert!(app.toast.is_none(), "nothing was applied");
        app.mir.rows.entries[0].value = r"C:\notes.txt".into();
        fold_entries(&app.mir.rows.entries, &mut app.cfg);
        app.commit();
        assert_eq!(
            read(),
            "# mine\nlauncher_entries = Notes|C:\\notes.txt|auto\n"
        );
        app.undo();
        assert_eq!(read(), "# mine\nlauncher_entries = \n");
        assert_eq!(
            app.mir.rows.entries[0].value, "",
            "undo brings the row back as typed"
        );
    }

    #[test]
    fn notes_come_from_the_keymap_the_wm_binds() {
        for c in [Config::defaults(), astur_config::default_config()] {
            let r = Rows::from(&c);
            assert_eq!(r.problems(c.workspaces), 0);
            assert!(
                compile_keys(&c).issues.is_empty(),
                "the shipped keys have notes"
            );
        }
        let row = |c: &Config, i: usize| {
            notes(&compile_keys(c).issues, |x| {
                BIND_ACTIONS[i].contains(&x.action)
            })
        };
        let mut c = Config::defaults();
        // Two binds on J: the first in the WM's order runs, the other says so.
        c.key_grow_master = 0x4A;
        let taken = "Alt+J: does nothing: \u{201C}Focus the next window\u{201D} uses it first";
        // (Its Shift action loses Alt+Shift+J the same way: a second line.)
        assert_eq!(row(&c, 3)[0], (Level::Warn, taken.to_string()));
        assert!(row(&c, 0).is_empty());
        // Under Win, L moves to O, and ten workspace keys read as one line.
        c.key_grow_master = 0x4C;
        c.main_key = MainKey::Win;
        let lock = "Win+L always locks the PC, so this uses Win+O";
        assert_eq!(row(&c, 3), [(Level::Info, lock.to_string())]);
        let ws = notes(&compile_keys(&c).issues, |x| {
            x.action == Action::SwitchWorkspace
        });
        assert_eq!(ws.len(), 1, "{ws:?}");
        assert!(ws[0].1.starts_with("Win+1, Win+2, "), "{ws:?}");
        // The picker counts what each key would take over with these binds.
        assert_eq!(takeovers(&c, MainKey::LAlt).0, 0);
        let (n, lines) = takeovers(&c, MainKey::Win);
        assert!(n > 10, "{n}");
        assert!(
            lines.iter().any(|l| l.1.starts_with("Win+H: ")),
            "{lines:?}"
        );
        // The cheat sheet is the keymap, on the key in use.
        let sheet = cheat_sheet(&c);
        assert!(sheet.starts_with("Astur shortcuts (hold Windows key)"));
        assert!(sheet.contains("  Win+O  Grow the main area\n"), "{sheet}");
        let mut c = Config::defaults();
        c.scratchpad_enabled = true;
        let sheet = cheat_sheet(&c);
        let focus = "  Alt+J  Focus the next window\n  Alt+Shift+J  Swap with the next window\n";
        assert!(sheet.contains(focus), "{sheet}");
        assert!(sheet.contains("  Alt+1  Workspace 1 (Web)"));
        assert!(sheet.contains("  Alt+`  Show or hide the scratchpad\n"));
        assert!(
            !sheet.contains("Ctrl"),
            "Left Alt's Ctrl copies are listed once"
        );
    }

    #[test]
    fn custom_chords_and_labels_follow_the_astur_key() {
        let hk = |chord: &str| HotkeyDef {
            chord: chord.into(),
            action: "reload".into(),
            argument: String::new(),
        };
        let mut v = vec![
            hk("ALT+GRAVE"),
            hk("alt + shift+J"),
            hk("WIN+Q"),
            hk("MOD+F2"),
        ];
        follow_key(&mut v, MainKey::LAlt, MainKey::Alt);
        assert_eq!(v[0].chord, "ALT+GRAVE", "Alt-based to Alt-based: no change");
        follow_key(&mut v, MainKey::LAlt, MainKey::Win);
        let chords: Vec<&str> = v.iter().map(|h| h.chord.as_str()).collect();
        assert_eq!(chords, ["MOD+GRAVE", "MOD+ shift+J", "WIN+Q", "MOD+F2"]);
        let win_shift_j = Some((MOD_WIN | MOD_SHIFT, 0x4A));
        assert_eq!(parse_chord(&v[1].chord, MainKey::Win), win_shift_j);
        // A chord no Astur key can run stays a red row; one with no key asks.
        assert!(hotkey_check(&hk("CTRL+Q")).is_err_and(|e| e.contains("no Astur key")));
        let no_key = hotkey_check(&hk("MOD+")).unwrap_err();
        assert_eq!(no_key, "Press the key for this shortcut");
        assert!(hotkey_check(&hk("ALT+FOO+Q")).is_err_and(|e| e.contains("can't read")));
        assert!(hotkey_check(&hk("WIN+SHIFT+Q")).is_ok());
        assert_eq!(action_info("close_window").map(|a| a.0), Some("close"));
        assert_eq!(split_target(r"C:\x\app.EXE").0, Kind::App);
        assert_eq!(join_target(Kind::Command, "wt"), "cmd:wt");
        assert_eq!(key_label("GRAVE"), "`");
        // Every label names the key as the chords and notes do.
        let mut c = Config::defaults();
        for (k, prefix) in [
            (MainKey::LAlt, "Alt+"),
            (MainKey::Win, "Win+"),
            (MainKey::Alt, "Alt+"),
            (MainKey::CtrlWin, "Win+Ctrl+"),
            (MainKey::CapsLock, "Caps Lock+"),
        ] {
            c.main_key = k;
            assert_eq!(mk(&c), prefix);
            assert_eq!(fixed(&c)[4].0, format!("{prefix}Space"));
            assert_eq!(mouse(&c)[0].0, format!("{prefix}left-drag"));
        }
        assert_eq!(held(&c), "Caps Lock");
        assert!(fixed(&c).iter().all(|f| f.0 != "Alt+Tab"));
        c.main_key = MainKey::LAlt;
        assert!(fixed(&c).iter().any(|f| f.0 == "Alt+Tab"));
    }

    #[test]
    fn a_new_astur_key_goes_back_unless_kept() {
        let t0 = Instant::now();
        assert_eq!(secs_left(t0 + TRIAL, t0), 15);
        assert_eq!(secs_left(t0 + Duration::from_millis(200), t0), 1);
        assert_eq!(secs_left(t0, t0 + Duration::from_secs(1)), 0);
        let paths = temp_paths("trial", "extra_hotkeys = ALT+GRAVE|scratchpad|\n", "");
        let read = || parse_text(&std::fs::read_to_string(&paths[0]).unwrap());
        let mut app = App::load(paths.clone());
        let ctx = egui::Context::default();
        setup_look(&ctx);
        let frame = |app: &mut App| {
            let _ = ctx.run(egui::RawInput::default(), |ctx| app.ui(ctx));
        };
        // At once, the scratchpad moving along, as one step.
        app.switch_key(MainKey::Win);
        assert_eq!(read().main_key, MainKey::Win);
        assert_eq!(read().extra_hotkeys[0].chord, "MOD+GRAVE");
        assert_eq!(app.history.undo.len(), 1);
        frame(&mut app);
        assert!(app.trial.is_some(), "on trial until kept");
        // The banner's Undo (and closing Settings): nothing else changed,
        // so the file reads exactly as before.
        app.revert_key();
        assert_eq!(read().main_key, MainKey::LAlt);
        assert_eq!(read().extra_hotkeys[0].chord, "ALT+GRAVE");
        // A second switch still goes back to the first key when time is up.
        app.switch_key(MainKey::Win);
        app.switch_key(MainKey::CapsLock);
        app.trial.as_mut().unwrap().until = Instant::now();
        frame(&mut app);
        assert!(app.trial.is_none());
        assert_eq!(
            (app.cfg.main_key, read().main_key),
            (MainKey::LAlt, MainKey::LAlt)
        );
        // Ctrl+Z ends a trial too; Keep ends it where it is.
        app.switch_key(MainKey::Alt);
        app.undo();
        frame(&mut app);
        assert!(app.trial.is_none());
        app.switch_key(MainKey::Alt);
        app.trial = None; // Keep
        frame(&mut app);
        assert_eq!(read().main_key, MainKey::Alt);
    }

    #[test]
    fn a_trial_goes_back_to_the_key_and_nothing_else() {
        // Under Win, ALT+GRAVE is the real Alt+`: no trial step may move it.
        let wm = "main_key = win\nextra_hotkeys = ALT+GRAVE|scratchpad|\n";
        let paths = temp_paths("trial-back", wm, "");
        let read = || parse_text(&std::fs::read_to_string(&paths[0]).unwrap());
        let start = |app: &App| {
            assert_eq!(read().main_key, MainKey::Win);
            assert_eq!(read().extra_hotkeys[0].chord, "ALT+GRAVE");
            assert_eq!(app.cfg.extra_hotkeys[0].chord, "ALT+GRAVE");
        };
        let mut app = App::load(paths.clone());
        let ctx = egui::Context::default();
        setup_look(&ctx);
        let frame = |app: &mut App, input: egui::RawInput| {
            let _ = ctx.run(input, |ctx| app.ui(ctx));
        };
        // Back to Win through Left Alt, by the picker.
        app.switch_key(MainKey::LAlt);
        app.switch_key(MainKey::Win);
        assert!(app.trial.is_none());
        start(&app);
        // Through Left Alt to Caps Lock, then reverted.
        app.switch_key(MainKey::LAlt);
        app.switch_key(MainKey::CapsLock);
        assert_eq!(read().extra_hotkeys[0].chord, "ALT+GRAVE");
        app.revert_key();
        start(&app);
        // Another setting changed and put back during the trial: Undo would
        // restore the step between (Animations off); the key alone goes back.
        let anim = app.cfg.animations;
        app.switch_key(MainKey::LAlt);
        app.cfg.animations = !anim;
        app.flush();
        app.cfg.animations = anim;
        app.flush();
        assert!(app.applied == app.trial.as_ref().unwrap().after);
        app.revert_key();
        start(&app);
        assert_eq!((app.cfg.animations, read().animations), (anim, anim));
        // Redo brings a key back on trial, as picking it does.
        app.switch_key(MainKey::CapsLock);
        app.revert_key(); // nothing else changed: an Undo
        assert_eq!(read().main_key, MainKey::Win);
        app.redo();
        assert_eq!(read().main_key, MainKey::CapsLock);
        assert_eq!(app.trial.as_ref().map(|t| t.from), Some(MainKey::Win));
        // Time up mid-gesture (a button held): it waits for the release.
        app.trial.as_mut().unwrap().until = Instant::now();
        let button = |pressed| egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos: egui::pos2(5.0, 5.0),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            }],
            ..Default::default()
        };
        frame(&mut app, button(true));
        assert!(app.trial.is_some(), "not reverted mid-gesture");
        frame(&mut app, button(false));
        assert!(app.trial.is_none());
        start(&app);
    }

    #[test]
    fn key_chip_captures_cancels_and_clears() {
        let ctx = egui::Context::default();
        setup_look(&ctx);
        let id = egui::Id::new("chip");
        let listen = || ctx.data_mut(|d| d.insert_temp(capture_id(), id));
        let listening = || ctx.data(|d| d.get_temp::<egui::Id>(capture_id())) == Some(id);
        let press = |key, physical_key, shift| egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers {
                    shift,
                    ..Default::default()
                },
            }],
            ..Default::default()
        };
        let frame = |input, v: &mut String, mods: Option<(&mut bool, &mut bool)>| {
            let mut changed = false;
            let mut mods = mods;
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let m = mods.as_mut().map(|(s, c)| (&mut **s, &mut **c));
                    changed = key_chip(ui, id, v, m);
                });
            });
            changed
        };
        let mut v = "J".to_string();
        listen();
        // Space is not a bind key: ignored, still listening.
        assert!(!frame(press(egui::Key::Space, None, false), &mut v, None));
        assert!(listening());
        assert!(frame(press(egui::Key::K, None, false), &mut v, None));
        assert_eq!((v.as_str(), listening()), ("K", false));
        listen();
        assert!(!frame(press(egui::Key::Escape, None, false), &mut v, None));
        assert_eq!((v.as_str(), listening()), ("K", false));
        listen();
        assert!(frame(
            press(egui::Key::Backspace, None, false),
            &mut v,
            None
        ));
        assert_eq!(v, "");
        // A chord takes Space, and Shift+1 (logically "!") by its place.
        let (mut shift, mut ctrl) = (false, false);
        listen();
        let input = press(egui::Key::Exclamationmark, Some(egui::Key::Num1), true);
        assert!(frame(input, &mut v, Some((&mut shift, &mut ctrl))));
        assert_eq!((v.as_str(), shift, ctrl), ("1", true, false));
        listen();
        frame(
            press(egui::Key::Space, None, false),
            &mut v,
            Some((&mut shift, &mut ctrl)),
        );
        assert_eq!(
            (v.as_str(), shift),
            ("SPACE", true),
            "a bare press keeps Shift on"
        );
        // A chord the WM refuses (no Astur key) is fixed by setting its key again.
        let mut h = HotkeyDef {
            chord: "CTRL+Q".into(),
            action: "launch".into(),
            argument: "x".into(),
        };
        assert!(hotkey_check(&h).is_err());
        ctx.data_mut(|d| d.insert_temp(capture_id(), egui::Id::new(("hotkey", 0))));
        let _ = ctx.run(press(egui::Key::Q, None, false), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                hotkey_ui(ui, 0, &mut h, (MainKey::LAlt, &[], 10), false)
            });
        });
        assert_eq!(h.chord, "MOD+CTRL+Q");
        assert!(hotkey_check(&h).is_ok());
        // An OEM key is bound by its US place's VK, so only where it is on a
        // US layout: egui gives German ü as the [ place, typing ü.
        let typing = |key, physical_key, text: &str| {
            let mut input = press(key, Some(physical_key), false);
            input.events.push(egui::Event::Text(text.into()));
            input
        };
        let (mut shift, mut ctrl) = (false, false);
        let mut v = String::new();
        listen();
        let ue = typing(egui::Key::OpenBracket, egui::Key::OpenBracket, "\u{FC}");
        assert!(!frame(ue, &mut v, None));
        assert!(listening());
        assert!(frame(
            typing(egui::Key::OpenBracket, egui::Key::OpenBracket, "["),
            &mut v,
            None
        ));
        assert_eq!(v, "[");
        // German + sits on the US ] place: not "]". Shift+= on US types + too.
        listen();
        let plus = typing(egui::Key::Plus, egui::Key::CloseBracket, "+");
        assert!(!frame(plus, &mut v, Some((&mut shift, &mut ctrl))));
        let plus = typing(egui::Key::Plus, egui::Key::Equals, "+");
        assert!(frame(plus, &mut v, Some((&mut shift, &mut ctrl))));
        assert_eq!(v, "EQUAL");
        // No letter by place: AZERTY's comma is where US has M. It binds
        // the comma key (VK_OEM_COMMA, the same on every layout), never M.
        listen();
        let comma = typing(egui::Key::Comma, egui::Key::M, ",");
        assert!(frame(comma, &mut v, Some((&mut shift, &mut ctrl))));
        assert_eq!(v, "Comma");
        // A layout-bound OEM key only at its US place, typing its US glyph:
        // German ; is Shift+, and its VK_OEM_1 is the u-umlaut key.
        listen();
        let semi = typing(egui::Key::Semicolon, egui::Key::Comma, ";");
        assert!(!frame(semi, &mut v, Some((&mut shift, &mut ctrl))));
        let oe = typing(egui::Key::Semicolon, egui::Key::Semicolon, "\u{F6}");
        assert!(!frame(oe, &mut v, Some((&mut shift, &mut ctrl))));
        let semi = typing(egui::Key::Semicolon, egui::Key::Semicolon, ";");
        assert!(frame(semi, &mut v, Some((&mut shift, &mut ctrl))));
        assert_eq!(v, "Semicolon");
        listen();
        // Ctrl+C reaches egui as Copy, with no Key event.
        let copy = egui::RawInput {
            events: vec![egui::Event::Copy],
            modifiers: egui::Modifiers::CTRL,
            ..Default::default()
        };
        assert!(frame(copy, &mut v, Some((&mut shift, &mut ctrl))));
        assert_eq!((v.as_str(), ctrl), ("C", true));
    }

    /// Renders the pages offscreen to PNGs, since Settings must not be
    /// launched on a working desktop to check a visual change:
    ///   cargo test -p astur-settings -- --ignored render_pages
    /// writes %TEMP%\astur-settings-render\*.png. A plain CPU rasterizer of
    /// egui's meshes with nearest-texel sampling, so text is a little rougher
    /// than on screen; layout, spacing and colours are exact.
    #[test]
    #[ignore]
    fn render_pages() {
        use std::collections::HashMap;
        let dir = std::env::temp_dir().join("astur-settings-render");
        std::fs::create_dir_all(&dir).unwrap();
        // The pickers' lists, as the startup thread fills them (read-only
        // registry, PATH and font lookups).
        FONTS.get_or_init(sys::font_families);
        APPS.get_or_init(detect_apps);
        DRIVES.get_or_init(ready_drives);
        let wm = "outer_gap = 120\nbar_heigth = 3\nworkspace_slide = false\n\
            window_rules = float|Teams.exe|||| ;; tile||Chrome_WidgetWin_1|* - YouTube|3|2\nignore_classes = TaskManagerWindow\n\
            extra_hotkeys = ALT+GRAVE|scratchpad| ;; ALT+SHIFT+J|launch|wt ;; CTRL+Q|launch|notepad ;; ALT+F2|layout|grid\n\
            launcher_entries = Notes|C:\\notes.txt|file ;; Docs|url:https://docs.rs|web\n\
            system_actions = Dev|VS Code|cmd:code .|command|false\nsystem_power_items = lock, sleep, bogus\n\
            workspace_names = Web, Code, Chat\nworkspaces = 5\nscratchpad_enabled = true\n";
        let nav = "bg = #203040\nclock_right_click = launch notepad.exe\n\
            cpu_click = switch_workspace 2\nmem_middle_click = launch wt -p Ubuntu\n\
            monitors = 1 3\ncustom_command = echo hello\n";
        let mut app = App::load(temp_paths("render", wm, nav));
        app.mir.rows.ws[1].name.clear(); // a red row: the names after it would shift
        app.mir.rows.keys[3] = "J".into(); // a clash with focus next
        app.cfg.key_grow_master = 0x4A;
        app.applied = (app.cfg.clone(), app.mir.clone());
        let ctx = egui::Context::default();
        setup_look(&ctx);
        let (w, h) = (1100usize, 760usize);
        let mut tex: HashMap<egui::TextureId, (usize, usize, Vec<Color32>)> = HashMap::new();
        let mut shots: Vec<(String, Page, &str, &str)> = NAV
            .iter()
            .map(|n| (format!("{:?}", n.0), n.0, "", "dark"))
            .collect();
        shots.push(("Search".into(), Page::Home, "gap", "dark"));
        shots.push(("SearchWide".into(), Page::Home, "e", "dark"));
        // The tables, alone: searching a conf key shows just its row.
        for key in [
            "extra_hotkeys",
            "window_rules",
            "launcher_entries",
            "system_actions",
        ] {
            shots.push((format!("Table-{key}"), Page::Home, key, "dark"));
        }
        // The status bar's newer rows, alone.
        for key in [
            "custom_",
            "disk_",
            "keyboard_",
            "title_",
            "monitors",
            "_click",
        ] {
            shots.push((format!("Bar-{key}"), Page::Bar, key, "dark"));
        }
        shots.push(("HomeLight".into(), Page::Home, "", "light"));
        shots.push(("BarLight".into(), Page::Bar, "", "light"));
        // The whole Status bar page, with the clock chip open.
        shots.push(("BarFull".into(), Page::Bar, "", "dark"));
        shots.push(("BarFullLight".into(), Page::Bar, "", "light"));
        // The Windows key as the Astur key, just switched (Keep banner).
        // Before the Full- shots, whose More options scroll the page.
        shots.push(("Win-Shortcuts".into(), Page::Shortcuts, "", "dark"));
        shots.push(("Win-Home".into(), Page::Home, "", "light"));
        // Whole pages, More options open (as a picker's Custom… opens them).
        for page in [
            Page::Home,
            Page::Shortcuts,
            Page::Layout,
            Page::Windows,
            Page::Appearance,
            Page::Launcher,
            Page::Extras,
        ] {
            shots.push((format!("Full-{page:?}"), page, "", "dark"));
        }
        // ASTUR_RENDER_ONLY=Bar renders just the shots whose name starts so.
        if let Ok(only) = std::env::var("ASTUR_RENDER_ONLY") {
            shots.retain(|s| s.0.starts_with(&only));
        }
        app.bar_sel = Some(Src::Zone(2, 5));
        for (name, page, query, theme) in shots {
            let win = name.starts_with("Win-");
            let key = if win { MainKey::Win } else { MainKey::LAlt };
            (app.cfg.main_key, app.applied.0.main_key) = (key, key);
            app.trial = win.then(|| Trial {
                from: MainKey::LAlt,
                after: app.applied.clone(),
                depth: app.history.undo.len(),
                until: Instant::now() + TRIAL,
            });
            if name.starts_with("Full-") {
                for salt in ["shortcuts", "launcher", "extras", "windows"] {
                    open_more(&ctx, salt);
                }
            }
            let h = if name.starts_with("BarFull") {
                5600
            } else if name.ends_with("-Shortcuts") {
                3000
            } else if name.starts_with("Full-") || win {
                2000
            } else {
                h
            };
            (app.page, app.query) = (page, query.to_string());
            app.cfg.theme = theme.to_string();
            app.applied.0.theme = theme.to_string();
            let mut last = None;
            // Enough frames for More options' opening animation (1/60 s each).
            let frames = if name.starts_with("Full-") { 10 } else { 3 };
            for _ in 0..frames {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(w as f32, h as f32),
                    )),
                    ..Default::default()
                };
                let out = ctx.run(input, |ctx| app.ui(ctx));
                for (id, delta) in &out.textures_delta.set {
                    let [dw, dh] = delta.image.size();
                    let pixels: Vec<Color32> = match &delta.image {
                        egui::ImageData::Color(img) => img.pixels.clone(),
                        egui::ImageData::Font(img) => img.srgba_pixels(None).collect(),
                    };
                    let [x0, y0] = delta.pos.unwrap_or([0, 0]);
                    let entry = tex
                        .entry(*id)
                        .or_insert_with(|| (dw, dh, vec![Color32::TRANSPARENT; dw * dh]));
                    if delta.pos.is_none() {
                        *entry = (dw, dh, pixels);
                        continue;
                    }
                    for y in 0..dh {
                        for x in 0..dw {
                            entry.2[(y0 + y) * entry.0 + x0 + x] = pixels[y * dw + x];
                        }
                    }
                }
                last = Some(out);
            }
            let out = last.unwrap();
            let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
            let rgba = raster(&prims, &tex, w, h);
            png(&dir.join(format!("{name}.png")), w, h, &rgba);
        }
    }

    fn raster(
        prims: &[egui::ClippedPrimitive],
        tex: &std::collections::HashMap<egui::TextureId, (usize, usize, Vec<Color32>)>,
        w: usize,
        h: usize,
    ) -> Vec<u8> {
        let mut px = vec![[0f32; 4]; w * h];
        let edge = |a: egui::Pos2, b: egui::Pos2, c: egui::Pos2| {
            (c.x - a.x) * (b.y - a.y) - (c.y - a.y) * (b.x - a.x)
        };
        for prim in prims {
            let egui::epaint::Primitive::Mesh(mesh) = &prim.primitive else {
                continue;
            };
            let Some((tw, th, texels)) = tex.get(&mesh.texture_id) else {
                continue;
            };
            let clip = prim.clip_rect;
            for tri in mesh.indices.as_chunks::<3>().0 {
                let v = tri.map(|i| mesh.vertices[i as usize]);
                let area = edge(v[1].pos, v[2].pos, v[0].pos);
                if area.abs() < 1e-6 {
                    continue;
                }
                let lo = v
                    .iter()
                    .fold(egui::pos2(f32::MAX, f32::MAX), |m, v| m.min(v.pos));
                let hi = v
                    .iter()
                    .fold(egui::pos2(f32::MIN, f32::MIN), |m, v| m.max(v.pos));
                let x0 = lo.x.max(clip.min.x).max(0.0).floor() as usize;
                let y0 = lo.y.max(clip.min.y).max(0.0).floor() as usize;
                let x1 = (hi.x.min(clip.max.x).ceil().max(0.0) as usize).min(w);
                let y1 = (hi.y.min(clip.max.y).ceil().max(0.0) as usize).min(h);
                for y in y0..y1 {
                    for x in x0..x1 {
                        let c = egui::pos2(x as f32 + 0.5, y as f32 + 0.5);
                        let b = [
                            edge(v[1].pos, v[2].pos, c) / area,
                            edge(v[2].pos, v[0].pos, c) / area,
                            edge(v[0].pos, v[1].pos, c) / area,
                        ];
                        if b.iter().any(|&k| k < -1e-4) {
                            continue;
                        }
                        let uv = v[0].uv.to_vec2() * b[0]
                            + v[1].uv.to_vec2() * b[1]
                            + v[2].uv.to_vec2() * b[2];
                        let tx = ((uv.x * *tw as f32) as usize).min(tw - 1);
                        let ty = ((uv.y * *th as f32) as usize).min(th - 1);
                        let t = texels[ty * tw + tx].to_array();
                        let dst = &mut px[y * w + x];
                        let mut src = [0f32; 4];
                        for (k, s) in src.iter_mut().enumerate() {
                            let col: f32 = v
                                .iter()
                                .zip(b)
                                .map(|(v, b)| v.color.to_array()[k] as f32 * b)
                                .sum();
                            *s = col / 255.0 * t[k] as f32 / 255.0;
                        }
                        for k in 0..4 {
                            dst[k] = src[k] + dst[k] * (1.0 - src[3]);
                        }
                    }
                }
            }
        }
        px.iter()
            .flat_map(|p| [p[0], p[1], p[2], 1.0].map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8))
            .collect()
    }

    /// Minimal PNG writer: stored (uncompressed) deflate blocks.
    fn png(path: &Path, w: usize, h: usize, rgba: &[u8]) {
        fn crc(data: &[u8]) -> u32 {
            let mut c = !0u32;
            for &b in data {
                c ^= u32::from(b);
                for _ in 0..8 {
                    c = if c & 1 == 1 {
                        0xEDB8_8320 ^ (c >> 1)
                    } else {
                        c >> 1
                    };
                }
            }
            !c
        }
        let mut raw = Vec::new();
        for line in rgba.chunks(w * 4) {
            raw.push(0);
            raw.extend_from_slice(line);
        }
        let mut z = vec![0x78, 0x01];
        let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
        for (i, block) in blocks.iter().enumerate() {
            z.push(u8::from(i + 1 == blocks.len()));
            let n = block.len() as u16;
            z.extend(n.to_le_bytes());
            z.extend((!n).to_le_bytes());
            z.extend_from_slice(block);
        }
        let (mut a, mut b) = (1u32, 0u32);
        for &x in &raw {
            a = (a + u32::from(x)) % 65521;
            b = (b + a) % 65521;
        }
        z.extend(((b << 16) | a).to_be_bytes());
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend((w as u32).to_be_bytes());
        ihdr.extend((h as u32).to_be_bytes());
        ihdr.extend([8, 6, 0, 0, 0]);
        for (kind, data) in [(b"IHDR", ihdr), (b"IDAT", z), (b"IEND", Vec::new())] {
            out.extend((data.len() as u32).to_be_bytes());
            let mut chunk = kind.to_vec();
            chunk.extend(data);
            out.extend_from_slice(&chunk);
            out.extend(crc(&chunk).to_be_bytes());
        }
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn icon_glyphs_exist_in_both_icon_fonts() {
        // Segoe Fluent Icons (11) and MDL2 Assets (10) are not documented to
        // share every code point; check the ones used here.
        let dir = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into()))
            .join("Fonts");
        let used: Vec<char> = NAV
            .iter()
            .map(|n| n.2)
            .chain([ICON_UNDO, ICON_REDO, ICON_OK, ICON_WARN])
            .chain([ICON_UP, ICON_DOWN, ICON_DELETE, ICON_FOLDER])
            .collect();
        for file in ["SegoeIcons.ttf", "segmdl2.ttf"] {
            let Ok(data) = std::fs::read(dir.join(file)) else {
                continue; // not this Windows version
            };
            let mut fonts = egui::FontDefinitions::default();
            add_font(&mut fonts, "icons", data, icons_family());
            fonts.families.insert(icons_family(), vec!["icons".into()]);
            let ctx = egui::Context::default();
            ctx.set_fonts(fonts);
            let _ = ctx.run(egui::RawInput::default(), |_| {});
            let id = FontId::new(16.0, icons_family());
            for &c in &used {
                assert!(
                    ctx.fonts(|f| f.has_glyph(&id, c)),
                    "{file} has no U+{:04X}",
                    c as u32
                );
            }
        }
    }

    /// The zones as the WM draws them (listed AND switched on).
    fn drawn(c: &Config) -> Vec<Vec<String>> {
        (0..3).map(|z| shown(c, z)).collect()
    }

    #[test]
    fn zones_and_visibility_follow_the_chips() {
        let d = Config::defaults();
        let s = |v: &[&str]| v.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        // Every widget the WM knows has a chip, once.
        let mut names: Vec<&str> = WIDGETS.iter().map(|w| w.0).collect();
        names.sort_unstable();
        let mut wm = BAR_WIDGETS.to_vec();
        wm.sort_unstable();
        assert_eq!(names, wm);
        // The shipped zones list apps, net and date switched off: they are
        // the hidden chips, not chips in a zone. The widgets without a
        // show_* switch are off the bar only by being in no zone.
        assert_eq!(
            palette_chips(&d),
            [
                ("apps", true),
                ("net", true),
                ("date", true),
                ("media", false),
                ("media_controls", false),
                ("keyboard", false),
                ("disk", false),
                ("uptime", false),
                ("custom", false),
                ("logo", false),
                ("separator", false),
                ("spacer", false)
            ]
        );
        let mut c = d.clone();
        move_widget(&mut c, Src::Palette("logo"), Some((0, 0)));
        assert_eq!(c.bar_left, s(&["logo", "workspaces"]));
        assert!(!palette_chips(&c).iter().any(|p| p.0 == "logo"));
        let logo = Src::Zone(0, 0);
        move_widget(&mut c, logo, None);
        assert!(palette_chips(&c).contains(&("logo", false)));
        // A move normalises the zones it touches and only those: the right
        // zone loses its off listings, the centre keeps its text.
        let mut c = d.clone();
        let clock = shown(&c, 2).iter().position(|w| w == "clock").unwrap();
        let to = move_widget(&mut c, Src::Zone(2, clock), Some((0, 1)));
        assert_eq!(to, Some(Src::Zone(0, 1)));
        assert_eq!(c.bar_left, s(&["workspaces", "clock"]));
        assert_eq!(
            c.bar_right,
            s(&["layout", "cpu", "mem", "volume", "battery"])
        );
        assert_eq!(c.bar_center, d.bar_center);
        assert!(c.bar_show_clock && !c.bar_show_apps && !c.bar_show_net);
        // Placing an off widget switches it on, after dropping its off
        // listings elsewhere (they would start drawing too).
        let mut c = d.clone();
        move_widget(&mut c, Src::Palette("net"), Some((0, 99)));
        assert!(c.bar_show_net);
        assert_eq!(c.bar_left, s(&["workspaces", "net"]));
        assert_eq!(
            c.bar_right,
            s(&["layout", "cpu", "mem", "volume", "battery", "date", "clock"])
        );
        // ...also into the zone that lists it: one listing.
        let mut c = d.clone();
        move_widget(&mut c, Src::Palette("apps"), Some((0, 0)));
        assert_eq!(c.bar_left, s(&["apps", "workspaces"]));
        assert!(c.bar_show_apps);
        // Taking one off switches it off, unless another zone shows it.
        let mut c = d.clone();
        let vol = shown(&c, 2).iter().position(|w| w == "volume").unwrap();
        assert_eq!(move_widget(&mut c, Src::Zone(2, vol), None), None);
        assert!(!c.bar_show_volume);
        assert!(palette_chips(&c).contains(&("volume", false)));
        let mut c = d.clone();
        c.bar_left.push("clock".into());
        move_widget(&mut c, Src::Zone(0, 1), None);
        assert!(c.bar_show_clock);
        assert_eq!(drawn(&c)[2].last().map(String::as_str), Some("clock"));
        // Media needs media_enabled too; separators repeat.
        let mut c = d.clone();
        move_widget(&mut c, Src::Palette("media"), Some((1, 0)));
        assert!(c.bar_show_media && c.media_enabled);
        move_widget(&mut c, Src::Palette("separator"), Some((1, 1)));
        move_widget(&mut c, Src::Palette("separator"), Some((1, 0)));
        assert_eq!(
            c.bar_center,
            s(&["separator", "media", "separator", "title"])
        );
        assert!(palette_chips(&c).contains(&("separator", false)));
        let media = shown(&c, 1).iter().position(|w| w == "media").unwrap();
        move_widget(&mut c, Src::Zone(1, media), None);
        assert!(!c.bar_show_media && !c.media_enabled);
        // Reordering: the chip's own old place doesn't count.
        let mut c = d.clone();
        move_widget(&mut c, Src::Zone(2, 0), Some((2, 3)));
        assert_eq!(
            c.bar_right,
            s(&["cpu", "mem", "layout", "volume", "battery", "clock"])
        );
        // A stale or bad place changes nothing.
        let before = c.clone();
        assert_eq!(move_widget(&mut c, Src::Zone(2, 40), None), None);
        assert_eq!(move_widget(&mut c, Src::Palette("cpu"), Some((3, 0))), None);
        assert!(c == before);
        // What Settings writes, the WM reads back as the same chips.
        let nav: String = (conf_pairs(&c).1.iter())
            .map(|(k, v)| format!("{k} = {v}\n"))
            .collect();
        assert_eq!(drawn(&parse_pair(&wm_text(&c), &nav)), drawn(&c));
    }

    #[test]
    fn presets_read_as_themselves_and_report_what_changed() {
        // The shipped bar is Classic: switched-off listings and a missing
        // shape line change nothing it draws.
        let classic = &BAR_PRESETS[0];
        for c in [Config::defaults(), astur_config::default_config()] {
            assert!(preset_diff(&c, &stamp(&c, classic)).is_empty());
        }
        // Every key a preset writes is one the GUI writes, so the write and
        // the diff both see it; and each preset reads as itself.
        let nav: Vec<&str> = (conf_pairs(&Config::defaults()).1.iter())
            .map(|p| p.0)
            .collect();
        for p in BAR_PRESETS {
            for (k, _) in p.pairs {
                assert!(
                    nav.contains(k),
                    "{} writes {k}, which Settings doesn't",
                    p.name
                );
            }
            let s = stamp(&Config::defaults(), p);
            assert!(preset_diff(&s, &stamp(&s, p)).is_empty(), "{}", p.name);
        }
        let islands = BAR_PRESETS.iter().find(|p| p.name == "islands").unwrap();
        let mut c = stamp(&Config::defaults(), islands);
        c.bar_height = 40;
        c.bar_palette = "nord".into();
        assert_eq!(preset_diff(&c, &stamp(&c, islands)), ["height", "palette"]);
        // A listed widget switched off is a zone change, not a switch.
        c.bar_show_clock = false;
        let diff = preset_diff(&c, &stamp(&c, islands));
        assert!(diff.contains(&"right"), "{diff:?}");
        assert!(!diff.iter().any(|k| k.starts_with("show_")), "{diff:?}");
        // `floating` without a shape line is the floating shape.
        let mut c = Config::defaults();
        c.bar_floating = true;
        assert_eq!(preset_diff(&c, &stamp(&c, classic)), ["shape"]);
        assert_eq!(
            short_list(&["a", "b", "c", "d", "e", "f"]),
            "a, b, c, d +2 more"
        );
    }

    #[test]
    fn a_preset_is_one_undo_step_and_backs_up_navbar_conf() {
        let nav = "height = 30\nfloating = true\n";
        let paths = temp_paths("preset", "", nav);
        let mut app = App::load(paths.clone());
        let islands = BAR_PRESETS.iter().find(|p| p.name == "islands").unwrap();
        app.apply_preset(islands);
        assert!(app.error.is_none(), "{:?}", app.error);
        let bak = paths[1].with_file_name("navbar.conf.bak");
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), nav);
        let text = std::fs::read_to_string(&paths[1]).unwrap();
        assert!(text.contains("shape = islands") && text.contains("height = 34"));
        assert!(
            text.contains("floating = true"),
            "floating is never written"
        );
        assert!(preset_diff(&app.disk, &stamp(&app.disk, islands)).is_empty());
        assert_eq!(app.history.undo.len(), 1);
        // A second preset keeps the first backup: the bar from before.
        app.apply_preset(&BAR_PRESETS[0]);
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), nav);
        app.undo();
        app.undo();
        let back = parse_pair("", &std::fs::read_to_string(&paths[1]).unwrap());
        assert_eq!(back.bar_height, 30);
        // The emptied shape line reads as none: floating decides again.
        assert_eq!(back.effective_bar_shape(), "floating");
        assert!(app.error.is_none(), "{:?}", app.error);
    }

    #[test]
    fn installed_fonts_come_from_gdi() {
        // GDI's own list: what CreateFont finds by name, the way the bar and
        // the popups ask for a font. Every Windows ships these two.
        let all = sys::font_families();
        for family in ["Segoe UI", "Consolas"] {
            assert!(font_installed(&all, family), "{family} missing: {all:?}");
        }
        assert!(!all.iter().any(|f| f.starts_with('@')), "vertical twins");
        assert!(font_installed(&all, " segoe ui "));
        assert!(font_installed(&all, ""));
        assert!(!font_installed(&all, "Segoe"));
        assert!(!font_installed(&all, "Consolasx"));
        // A sample is drawn: some ink, none outside the bitmap.
        let (w, h, alpha) = sys::font_sample("Segoe UI", 16, FONT_SAMPLE).unwrap();
        assert_eq!(alpha.len(), w * h);
        assert!(w > 50 && h >= 16 && alpha.iter().any(|&a| a > 200));
    }

    #[test]
    fn apps_are_found_where_they_install() {
        let env = |n: &str| match n {
            "ProgramFiles" => Some(r"C:\PF".to_string()),
            "LOCALAPPDATA" => Some(r"C:\U\L".to_string()),
            _ => None,
        };
        assert_eq!(
            expand(r"%ProgramFiles%\Git\git-bash.exe", &env).as_deref(),
            Some(r"C:\PF\Git\git-bash.exe")
        );
        assert_eq!(expand(r"%NOPE%\x.exe", &env), None);
        // Windows registers PowerShell.exe in App Paths as REG_EXPAND_SZ:
        // read as REG_SZ, it comes back expanded.
        let ps = sys::app_path("PowerShell.exe").unwrap_or_default();
        assert!(
            ps.to_ascii_lowercase().ends_with("powershell.exe") && !ps.contains('%'),
            "{ps}"
        );
        assert_eq!(expand("plain", &env).as_deref(), Some("plain"));
        // On PATH (or App Paths): by name. Else the first place that exists,
        // in full. Neither: not offered.
        let on_path = |exe: &str| matches!(exe, "cmd.exe" | "powershell.exe" | "wt.exe");
        let exists = |p: &str| p == r"C:\PF\Git\git-bash.exe";
        let found = find_apps(&TERMINALS, &on_path, &env, &exists);
        let got: Vec<(&str, &str)> = (found.iter())
            .map(|c| (c.label.as_str(), c.command.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("Windows Terminal", "wt.exe"),
                ("Windows PowerShell", "powershell.exe"),
                ("Command Prompt", "cmd.exe"),
                ("Git Bash", r"C:\PF\Git\git-bash.exe"),
            ]
        );
        // Browsers: the program of the registered command, each once, by name.
        assert_eq!(exe_of(r#""C:\P F\b.exe" --x"#), r"C:\P F\b.exe");
        assert_eq!(exe_of(r"C:\P F\b.EXE --x"), r"C:\P F\b.EXE");
        assert_eq!(exe_of("  b  "), "b");
        let reg = |n: &str, c: &str| (n.to_string(), c.to_string());
        let found = vec![
            reg("Mozilla Firefox", r#""C:\F\firefox.exe""#),
            reg("Google Chrome", r#""C:\G\chrome.exe""#),
            reg("Firefox again", r"C:\F\FIREFOX.exe"),
            reg("Gone", r#""C:\Gone\x.exe""#),
        ];
        let b = browser_choices(found, &|p: &str| !p.contains("Gone"));
        assert_eq!(
            b,
            [
                Choice::new("Google Chrome", r"C:\G\chrome.exe"),
                Choice::new("Mozilla Firefox", r"C:\F\firefox.exe")
            ]
        );
    }

    #[test]
    fn pickers_map_values_both_ways() {
        // An app is the same app by name, path or alias; options make a
        // command its own, and an address matches only itself.
        assert!(same_app("wt.exe", "WT"));
        assert!(same_app(
            r"C:\Program Files\PowerShell\7\pwsh.exe",
            "pwsh.exe"
        ));
        assert!(same_app(r#""C:\Program Files\x\app.exe""#, "app.exe"));
        assert!(!same_app("wt -p Ubuntu", "wt.exe"));
        assert!(same_app("", ""));
        assert!(!same_app("", "*"));
        // Two full paths: only whole (Chrome and Chrome Beta are chrome.exe).
        assert!(!same_app(
            r"C:\G\Chrome\chrome.exe",
            r"C:\G\Chrome Beta\chrome.exe"
        ));
        assert!(same_app(
            r"C:\G\Chrome\chrome.exe",
            r#""c:\g\chrome\CHROME.exe""#
        ));
        let [google, bing] = [SEARCH_ENGINES[0].1, SEARCH_ENGINES[1].1];
        assert!(!same_app(google, bing));
        let terms = [Choice::new("Windows Terminal", "wt.exe")];
        let app = Some(Pick::App);
        assert_eq!(choice_label("wt.exe", &terms, app), "Windows Terminal");
        // Not detected (yet), still named: a known terminal, a program.
        assert_eq!(choice_label("cmd", &[], app), "Command Prompt");
        assert_eq!(choice_label(r"C:\Apps\Foo.exe", &[], app), "Foo.exe");
        assert_eq!(
            choice_label("wt -p Ubuntu", &terms, app),
            "Custom: wt -p Ubuntu"
        );
        assert_eq!(choice_label("", &terms, app), "None");
        assert_eq!(
            choice_label(r"D:\Docs", &[], Some(Pick::Folder)),
            r"D:\Docs"
        );
        // Web search: every engine reads back as itself; the shipped default
        // is Google; anything else is Custom, kept as typed.
        let engines = SEARCH_ENGINES.map(|(n, u)| Choice::new(n, u));
        for (name, url) in SEARCH_ENGINES {
            assert_eq!(choice_label(url, &engines, None), name);
        }
        let default = Config::defaults().launcher_web_url;
        assert_eq!(choice_label(&default, &engines, None), "Google");
        assert_eq!(
            choice_label("https://x.y/?s={query}", &engines, None),
            "Custom: https://x.y/?s={query}"
        );
        // Gaps: the shipped 8 px is Medium; each size reads back.
        let d = Config::defaults();
        assert_eq!(gap_preset(d.outer_gap, d.inner_gap), "medium");
        for g in GAPS {
            assert_eq!(gap_preset(g.2, g.2), g.0);
        }
        assert_eq!(gap_preset(8, 4), "custom");
    }

    #[test]
    fn picked_windows_fill_rules_and_the_scratchpad() {
        let win = |exe: &str, class: &str, title: &str| OpenWindow {
            title: title.into(),
            class: class.into(),
            exe: exe.into(),
        };
        // A rule: the app's name; a host's windows also by title; the class
        // only when Windows won't name the program.
        let notes = win(r"C:\Windows\notepad.exe", "Notepad", "a.txt - Notepad");
        assert_eq!(
            rule_fields(&notes),
            ("notepad.exe".into(), String::new(), String::new())
        );
        let calc = win(
            r"C:\Windows\System32\ApplicationFrameHost.exe",
            "ApplicationFrameWindow",
            "Calculator",
        );
        assert_eq!(rule_fields(&calc).2, "Calculator");
        assert_eq!(rule_fields(&win("", "Admin", "x")).1, "Admin");
        // A rule made from a pick reads back as a valid row.
        let (exe, class, title) = rule_fields(&calc);
        let row = RuleRow {
            list: false,
            rule: WindowRule {
                action: "float".into(),
                exe,
                class,
                title,
                workspace: None,
                monitor: None,
            },
        };
        assert!(rule_check(&row).is_ok());
        // The scratchpad: the program, but a Store app by its command.
        let wt = win(
            r"C:\Program Files\WindowsApps\Microsoft.WindowsTerminal_1\WindowsTerminal.exe",
            "CASCADIA_HOSTING_WINDOW_CLASS",
            "PowerShell",
        );
        let cascadia = "CASCADIA_HOSTING_WINDOW_CLASS".to_string();
        assert_eq!(
            scratch_fields(&wt, "x.exe"),
            ("wt.exe".into(), cascadia.clone())
        );
        let other = win(r"C:\Program Files\WindowsApps\Foo\foo.exe", "FooWin", "Foo");
        assert_eq!(scratch_fields(&other, "keep.exe").0, "keep.exe");
        let alac = win(r"C:\PF\Alacritty\alacritty.exe", "Window Class", "~");
        assert_eq!(
            scratch_fields(&alac, "wt.exe").0,
            r"C:\PF\Alacritty\alacritty.exe"
        );
        // A command that already starts the picked app keeps its options.
        assert_eq!(scratch_fields(&wt, "wt -p Ubuntu").0, "wt -p Ubuntu");
        let wez = win(r"C:\PF\WezTerm\wezterm-gui.exe", "scratch", "~");
        let wez_cmd = "wezterm-gui.exe start --class scratch";
        assert_eq!(
            scratch_fields(&wez, wez_cmd),
            (wez_cmd.into(), "scratch".into())
        );
        // A known app's class, else the first window it opens; the shipped
        // pair is Windows Terminal and its class.
        let d = Config::defaults();
        assert_eq!(scratch_class(&d.scratchpad_command), d.scratchpad_class);
        assert_eq!(scratch_class("alacritty.exe"), "");
        assert_eq!(clip("abcdef", 4), "abc\u{2026}");
        assert_eq!(clip("abc", 4), "abc");
        assert_eq!(file_name(r"C:\a\b.exe"), "b.exe");
    }

    #[test]
    fn tips_follow_the_keys_in_use() {
        let mut c = Config::defaults();
        let tips = |c: &Config| tips(c).into_iter().map(|t| t.0).collect::<Vec<_>>();
        let t = tips(&c);
        assert!(t.contains(&"Alt+1 \u{2026} Alt+9".to_string()), "{t:?}");
        assert!(t.contains(&"Alt+J / Alt+K".to_string()), "{t:?}");
        assert!(t.contains(&"Alt+Space".to_string()));
        c.key_close_window = 0x51; // Q
        c.workspaces = 3;
        c.launcher_enabled = false;
        let t = tips(&c);
        assert!(t.contains(&"Alt+Q".to_string()));
        assert!(t.contains(&"Alt+1 \u{2026} Alt+3".to_string()));
        assert!(!t.contains(&"Alt+Space".to_string()));
        c.main_key = MainKey::CtrlWin;
        let t = tips(&c);
        assert!(t.contains(&"Win+Ctrl+J / Win+Ctrl+K".to_string()), "{t:?}");
        assert!(t.contains(&"Win+Ctrl+drag".to_string()), "{t:?}");
    }

    #[test]
    fn a_picked_path_reaches_only_the_field_that_asked() {
        let (a, b) = (egui::Id::new("a"), egui::Id::new("b"));
        *PICKED.lock().unwrap() = Some((a, r"C:\x".into()));
        assert_eq!(take_pick(b), None);
        assert_eq!(take_pick(a).as_deref(), Some(r"C:\x"));
        assert_eq!(take_pick(a), None, "once");
    }

    #[test]
    fn previews_fall_back_as_the_bar_does() {
        // Windows 10's MDL2 has no CPU glyph: the bar shows the label.
        let sample = |drawable: bool| stat_sample("glyph", "CPU", "{value}%", "cpu", &|_| drawable);
        assert_eq!(sample(false), (None, "CPU 23%".to_string()));
        assert_eq!(sample(true), (Some('\u{EEA1}'), "23%".to_string()));
        // glyph_only: the glyph alone where there is one, else the label
        // alone; the keyboard and uptime keep their value.
        let vol = |drawable: bool| {
            stat_sample("glyph_only", "VOL", "{value}%", "volume", &move |_| {
                drawable
            })
        };
        assert_eq!(vol(true), (Some('\u{E994}'), String::new()));
        assert_eq!(vol(false), (None, "VOL".to_string()));
        let kbd = stat_sample("glyph_only", "", "{lang}-{region}", "keyboard", &|_| true);
        assert_eq!(kbd, (Some('\u{E765}'), "ENG-US".to_string()));
        assert_eq!(
            stat_sample("both", "DISK", "{free}", "disk", &|_| true),
            (None, "DISK 120.50 GB".to_string())
        );
        // An empty clock format is clock_24h's; samples are the WM's own.
        assert_eq!(clock_sample("", false), "2:05 pm");
        assert_eq!(clock_sample("", true), "14:05");
        assert_eq!(clock_sample("h:mm:ss TT", true), "2:05:09 PM");
        assert_eq!(date_sample("dddd d MMMM"), "Sunday 4 October");
        assert_eq!(
            title_sample("{app} - {title}"),
            "Notepad - Notes.txt - Notepad"
        );
        // A custom text with no command shows its format as plain text.
        let mut c = Config::defaults();
        assert_eq!(custom_sample(&c), "");
        c.bar_custom_format = "Hi {output}".into();
        assert_eq!(custom_sample(&c), "Hi ");
        c.bar_custom_command = "echo hello".into();
        assert_eq!(custom_sample(&c), "Hi hello");
        // Pill colours set and set back, in another order: the same bar.
        let pills = BAR_PRESETS.iter().find(|p| p.name == "pills").unwrap();
        let mut c = stamp(&Config::defaults(), pills);
        c.bar_widget_colors.reverse();
        assert!(preset_diff(&c, &stamp(&c, pills)).is_empty());
        // A pasted line break can't split a conf line.
        let mut s = "wt.exe\r\n-p Ubuntu".to_string();
        one_line(&mut s);
        assert_eq!(s, "wt.exe  -p Ubuntu");
    }

    #[test]
    fn status_bar_rows_fit_their_width() {
        // A ComboBox at the end of a wrapped line never wraps: the pill
        // colours row overflowed its card by a few px, and egui then grew
        // every card under it past the window's right edge.
        let ctx = egui::Context::default();
        setup_look(&ctx);
        let d = Config::defaults();
        let mut c = parse_text(
            "shape = pills\nclock_click = launch notepad.exe\ncpu_right_click = switch_workspace 2\n\
             monitors = 1 3\ncustom_command = echo hi\nright = cpu mem volume battery date clock keyboard disk uptime custom\n",
        );
        let roles = c.bar_roles(false);
        // From the narrowest window less its nav (760 - 244 px, give or take
        // the scroll bar): a chip wider than a zone column can't fit below.
        for width in (500..=900).step_by(6) {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.scope(|ui| {
                        ui.set_max_width(width as f32);
                        let mut f = Find::default();
                        let mut sel = None;
                        let fits = |ui: &mut Ui, what: &str| {
                            let w = ui.min_rect().width();
                            assert!(w <= width as f32 + 0.5, "{what} is {w} px in {width}");
                        };
                        when(ui, &mut f, true, |ui, f| {
                            pill_colors(ui, f, &mut c, &d, &roles)
                        });
                        fits(ui, "pill colours");
                        zone_row(ui, &mut f, &mut c, &d, &mut sel);
                        fits(ui, "zones");
                        for w in [
                            "clock", "date", "title", "keyboard", "uptime", "custom", "disk",
                        ] {
                            widget_rows(ui, &mut f, &mut c, &d, w);
                            fits(ui, w);
                        }
                        ws_label_row(ui, &mut f, &mut c, &d);
                        monitors_row(ui, &mut f, &mut c.bar_monitors, &d.bar_monitors);
                        fits(ui, "workspaces, screens");
                        click_rows(ui, &mut f, &mut c, &d);
                        fits(ui, "click actions");
                    });
                });
            });
        }
    }

    #[test]
    fn click_picks_write_what_the_wm_reads() {
        let mut c = Config::defaults();
        assert_eq!(click_now(&c, "clock", 0), "calendar");
        assert_eq!(click_now(&c, "title", 2), "none");
        set_click(&mut c, "clock", 0, "launch notepad.exe");
        set_click(&mut c, "cpu", 1, "switch_workspace 3");
        // Picking the default keeps no entry: nothing new to write.
        set_click(&mut c, "volume", 0, "mute");
        assert_eq!(c.bar_clicks.len(), 2);
        let nav = |c: &Config| -> String {
            (conf_pairs(c).1.iter())
                .map(|(k, v)| format!("{k} = {v}\n"))
                .collect()
        };
        let back = parse_pair("", &nav(&c));
        assert_eq!(back.bar_click("clock", 0), ("launch", "notepad.exe"));
        assert_eq!(back.bar_click("cpu", 1), ("switch_workspace", "3"));
        assert_eq!(back.bar_click("volume", 0), ("mute", ""));
        // A changed pick keeps its place; back to the default, the line
        // is emptied, which the WM reads as the default.
        set_click(&mut c, "clock", 0, "launch calc.exe");
        assert_eq!(c.bar_clicks[0].2, "launch calc.exe");
        set_click(&mut c, "clock", 0, "calendar");
        let (_, n) = diff(&c, &back);
        assert_eq!(n, [("clock_click", String::new())]);
        assert_eq!(
            parse_pair("", &nav(&c)).bar_click("clock", 0),
            ("calendar", "")
        );
        // Untouched clicks write nothing, and every click key is a navbar key.
        assert!(diff(&Config::defaults(), &astur_config::default_config())
            .1
            .iter()
            .all(|p| !p.0.ends_with("_click")));
        assert_eq!(click_keys().len(), BAR_CLICK_WIDGETS.len());
        assert_eq!(click_keys()[0][1], "title_right_click");
        assert_eq!(conf_file("logo_middle_click"), "navbar.conf");
        // A workspace past the count set does nothing in the WM: said so.
        assert!(missing_workspace(15, 10).is_some_and(|w| w.contains("you have 10")));
        assert_eq!(missing_workspace(10, 10), None);
    }

    #[test]
    fn a_typed_command_keeps_its_spaces() {
        // Each keystroke round-trips through the stored value: a trailing
        // space must survive it, or `wt -p` is typed as `wt-p`.
        let mut c = Config::defaults();
        let mut typed = String::new();
        for ch in "wt -p Ubuntu".chars() {
            let (_, arg) = click_now(&c, "clock", 0)
                .split_once(' ')
                .map_or((String::new(), String::new()), |(a, b)| {
                    (a.to_string(), b.to_string())
                });
            typed = format!("{arg}{ch}");
            set_click(&mut c, "clock", 0, &launch_value(&typed));
        }
        assert_eq!(typed, "wt -p Ubuntu");
        assert_eq!(click_now(&c, "clock", 0), "launch wt -p Ubuntu");
        assert_eq!(launch_value("  "), "launch");
        assert_eq!(launch_value(" wt "), "launch wt ");
        // The file never gets the trailing space.
        let back = parse_pair("", "clock_click = launch wt \n");
        assert_eq!(back.bar_click("clock", 0), ("launch", "wt"));
        // Once shown, the box stays while the text names a known app.
        let ctx = egui::Context::default();
        let id = egui::Id::new("menu");
        let typed_id = id.with("typed");
        for v in ["wt -p Ubuntu", "wt"] {
            let mut v = v.to_string();
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app_menu(ui, id, &mut v));
            });
            assert_eq!(ctx.data(|m| m.get_temp::<bool>(typed_id)), Some(true));
        }
    }

    #[test]
    fn pickers_offer_only_what_the_wm_reads() {
        // Screens: ticked and unticked in the parser's sorted form, never
        // down to none.
        // Skip folders: a picked folder with a , or ; would read back as
        // pieces (one a parent folder), so it is refused, the list as it was.
        let mut skip = ".git".to_string();
        assert!(add_folder(&mut skip, r"C:\Users\x\Archive"));
        assert!(!add_folder(
            &mut skip,
            r"C:\Users\x\OneDrive - Contoso, Inc\Archive"
        ));
        assert_eq!(skip, r".git, C:\Users\x\Archive");
        let back = parse_text(&format!("launcher_file_exclude = {skip}"));
        assert_eq!(back.launcher_file_exclude, [".git", r"C:\Users\x\Archive"]);
        assert_eq!(toggle_screen("1 3", 2).as_deref(), Some("1 2 3"));
        assert_eq!(toggle_screen("1 3", 3).as_deref(), Some("1"));
        assert_eq!(toggle_screen("2", 2), None);
        let v = toggle_screen("3 1", 2).unwrap();
        assert_eq!(parse_text(&format!("monitors = {v}")).bar_monitors, v);
        // Every ready-made value is one the parser keeps, and each shows
        // something different.
        let keeps = |key: &str, v: &str| {
            let text = format!("{key} = {v}\n");
            conf_pairs(&parse_text(&text))
                .1
                .iter()
                .any(|p| p.0 == key && p.1 == v)
        };
        for (v, _) in WS_LABELS {
            assert!(keeps("workspace_label", v), "{v}");
        }
        let labels: Vec<&str> = WS_LABELS.iter().map(|l| l.0).collect();
        assert_eq!(labels, astur_config::WORKSPACE_LABELS);
        for (key, opts) in [
            ("clock_format", CLOCKS.map(|c| c.0).to_vec()),
            ("date_format", DATES.to_vec()),
            ("title_format", TITLES.map(|t| t.0).to_vec()),
            ("keyboard_format", KEYBOARDS.map(|k| k.0).to_vec()),
            ("custom_interval", INTERVALS.map(|i| i.0).to_vec()),
        ] {
            for v in &opts {
                assert!(keeps(key, v), "{key} = {v}");
            }
        }
        let clocks: std::collections::HashSet<String> =
            CLOCKS.iter().map(|c| clock_sample(c.0, true)).collect();
        let dates: std::collections::HashSet<String> =
            DATES.iter().map(|d| date_sample(d)).collect();
        assert_eq!((clocks.len(), dates.len()), (CLOCKS.len(), DATES.len()));
        for opt in ["separator_style = powerline", "icon_mode = glyph_only"] {
            let (k, v) = opt.split_once(" = ").unwrap();
            assert!(keeps(k, v), "{opt}");
        }
        // Workspace labels as the WM writes them; dots never widen a mark.
        let mut c = Config::defaults();
        assert_eq!(ws_labels(&c, 2), ["Web", "Code"]);
        c.bar_workspace_label = "number".into();
        assert_eq!(ws_labels(&c, 2), ["1", "2"]);
        assert_eq!(mark_icons_w(0), 0.0);
        assert_eq!(mark_icons_w(2), (4.0 + 32.0 + 2.0) * PREVIEW);
    }

    #[test]
    fn dragging_a_chip_moves_it_and_a_click_selects_it() {
        let mut app = App::load(temp_paths("dnd", "", ""));
        let ctx = egui::Context::default();
        setup_look(&ctx);
        app.page = Page::Bar;
        app.query = "media_enabled".into(); // the zone editor alone
        fn frame(ctx: &egui::Context, app: &mut App, events: Vec<egui::Event>) -> egui::FullOutput {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1100.0, 900.0),
                )),
                events,
                ..Default::default()
            };
            ctx.run(input, |ctx| app.ui(ctx))
        }
        fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }
        }
        fn drag(
            ctx: &egui::Context,
            app: &mut App,
            from: egui::Pos2,
            to: egui::Pos2,
        ) -> egui::FullOutput {
            frame(ctx, app, vec![egui::Event::PointerMoved(from)]);
            frame(ctx, app, vec![button(from, true)]);
            frame(
                ctx,
                app,
                vec![egui::Event::PointerMoved(from + egui::vec2(12.0, 0.0))],
            );
            frame(ctx, app, vec![egui::Event::PointerMoved(to)]);
            frame(ctx, app, vec![button(to, false)]);
            frame(ctx, app, vec![])
        }
        // Where a chip's label was painted (right of the nav, which has
        // a Workspaces too).
        let at = |out: &egui::FullOutput, label: &str| {
            fn find(s: &egui::Shape, label: &str) -> Option<egui::Pos2> {
                match s {
                    egui::Shape::Text(t) if t.galley.text() == label => {
                        Some(t.visual_bounding_rect().center()).filter(|p| p.x > 260.0)
                    }
                    egui::Shape::Vec(v) => v.iter().find_map(|s| find(s, label)),
                    _ => None,
                }
            }
            out.shapes
                .iter()
                .find_map(|c| find(&c.shape, label))
                .unwrap()
        };
        frame(&ctx, &mut app, vec![]); // egui settles its layout first
        let out = frame(&ctx, &mut app, vec![]);
        // The clock, dropped right of Workspaces in the left zone.
        let to = at(&out, "Workspaces") + egui::vec2(70.0, 0.0);
        let out = drag(&ctx, &mut app, at(&out, "Clock"), to);
        assert_eq!(app.cfg.bar_left, ["workspaces", "clock"]);
        assert_eq!(app.bar_sel, Some(Src::Zone(0, 1)));
        // Volume, dropped on the widgets not on the bar: switched off.
        let out = drag(&ctx, &mut app, at(&out, "Volume"), at(&out, "Spacer"));
        assert!(!app.cfg.bar_show_volume);
        assert!(!app.cfg.bar_right.contains(&"volume".to_string()));
        // A click (no movement) selects a chip.
        let battery = at(&out, "Battery");
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(battery)]);
        frame(&ctx, &mut app, vec![button(battery, true)]);
        frame(&ctx, &mut app, vec![button(battery, false)]);
        let i = shown(&app.cfg, 2).iter().position(|w| w == "battery");
        assert_eq!(app.bar_sel, i.map(|i| Src::Zone(2, i)));
    }
}

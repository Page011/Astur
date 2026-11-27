//! The UI thread's side of the bar: bars, scenes, the renderer, presenting
//! frames, input, config reloads, plates and the test flags. Everything that
//! blocks or reads a file lives on another thread; this one only drains its
//! inbox (`UiMsg`) and draws.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use rsb_core::config::{Config, FullscreenMode, default_config};
use rsb_core::css::{MediaEnv, Stylesheet};
use rsb_core::diag::{DiagKind, Diagnostic, Plate, Severity};
use rsb_core::draw::DrawList;
use rsb_core::element::{InstanceId, WidgetTree};
use rsb_core::hit::{Button, HandlerCall, InputKind, PointerKind};
use rsb_core::ipc::{Command, Reply};
use rsb_core::msg::{InstanceSpec, ScriptIn, ScriptOut};
use rsb_core::popup::CloseReason;
use rsb_core::scene::{BarGeometry, Scene, SceneEnv};
use rsb_core::style::{Backdrop, Cursor};
use rsb_core::value::Value;
use rsb_render::Renderer;
use rsb_win::{BarId, BarWindowSpec, MenuItem, OutsidePress, TestInput, Ui, UiEvent, UiHandler};
use serde_json::json;

use crate::capture::Capture;
use crate::clock;
use crate::fallback;
use crate::fonts::FontRegistry;
use crate::icons::BundledIcons;
use crate::images::{ImageLoader, ImageRequests, files_of};
use crate::msg::{AppliedHashes, AppliedState, BarInfo, ConfigUpdate, ReloadKind, SimAction, SimSurface, UiMsg};
use crate::plan::{BarInstances, BarPlan, InstanceTable, build_instances, plan_bars};
use crate::popups::{Env, OverlayWatch, PopupOptions, Popups, SLOW_STEP, SlowStep};
use crate::reader::Reader;
use crate::service_images::ServiceImages;
use crate::stats::{Stats, since_process_start_ms, wall_ms};
use crate::winlayer::{CaptureState, Live, Win};

/// A bar is first drawn when all of its widgets have reported, or this long
/// after it was created, whichever comes first.
pub const FIRST_FRAME_WAIT: Duration = Duration::from_millis(150);
/// How long a test-injected render failure shows before the bar recovers.
const INJECTED_FAILURE_FOR: Duration = Duration::from_millis(1000);

pub struct AppOptions {
    pub config_dir: PathBuf,
    pub log_file: PathBuf,
    /// `--no-reserve`: no bar registers screen space.
    pub no_reserve: bool,
    /// `--test-offscreen`: every window outside the screen, no appbar.
    pub offscreen: bool,
    /// `--simulate-pointer`: a right click does not open the native menu.
    pub simulated_pointer: bool,
    /// `--no-hook`: the outside-press hook is never installed.
    pub no_hook: bool,
    /// `--no-activate`: no popup takes real keyboard focus.
    pub no_activate: bool,
    /// A test flag drives this run: the clipboard is a stand-in.
    pub gated: bool,
    /// Shared with the watchdog thread: an overlay is up, and the UI thread stalled.
    pub overlays: Arc<OverlayWatch>,
}

/// The window a bar plan asks for.
fn window_spec(plan: &BarPlan, no_reserve: bool) -> BarWindowSpec {
    BarWindowSpec {
        name: plan.spec.name.clone(),
        monitor: plan.monitor.device.clone(),
        position: plan.spec.position,
        height: plan.spec.height,
        floating: plan.spec.floating,
        margin: plan.spec.margin,
        reserve: plan.spec.reserve && !no_reserve,
        fullscreen: plan.spec.fullscreen,
    }
}

pub(crate) struct BarRt {
    key: String,
    pub(crate) id: BarId,
    pub(crate) plan: BarPlan,
    win: BarWindowSpec,
    pub(crate) scene: Scene,
    pub(crate) insts: BarInstances,
    /// Instances that have not yet sent a first tree or plate.
    awaiting: HashSet<InstanceId>,
    created: Instant,
    presented: bool,
    pub(crate) scale: f32,
    cursor: Cursor,
    backdrop: Backdrop,
    next_frame: Option<Instant>,
    render_failed: bool,
    /// A fullscreen app is in front of this bar's monitor.
    fullscreen: bool,
    /// Draw the whole surface at the next frame (after a failure, or a size change).
    need_full: bool,
    recover_at: Option<Instant>,
}

struct Pending {
    kind: &'static str,
    notified: u64,
    armed: bool,
}

pub struct App {
    o: AppOptions,
    stats: Arc<Stats>,
    rx: std::sync::mpsc::Receiver<UiMsg>,
    script_tx: Sender<ScriptIn>,
    reader: Option<Reader>,
    images: ImageLoader,
    renderer: Renderer,
    config: Config,
    /// The versions of the files the bars show, for the pipe thread's `applied` and `settings` answers.
    applied: Option<Arc<AppliedState>>,
    /// Versions of a config swapped in whose frame is not drawn yet; published to `applied` after it.
    unpublished: Option<(AppliedHashes, u32)>,
    /// The custom properties the active preset lists as tunable, shown on each bar's root by `rsb inspect`.
    tune_vars: Vec<String>,
    sheet: Arc<Stylesheet>,
    table: InstanceTable,
    bars: Vec<BarRt>,
    dark: bool,
    config_diags: Vec<Diagnostic>,
    plan_diags: Vec<Diagnostic>,
    font_diags: Vec<Diagnostic>,
    scene_diags: Vec<Diagnostic>,
    script_diags: Vec<Diagnostic>,
    /// Widgets in their error state (a handler, an effect or a popup's content failed lately).
    script_errors: BTreeMap<InstanceId, Diagnostic>,
    stall: Option<Diagnostic>,
    /// The slowest bar draw since the last slow-turn report.
    slow_bar: Option<SlowStep>,
    pipe_diag: Option<Diagnostic>,
    bar_plate: Option<Plate>,
    trees: HashMap<InstanceId, Arc<WidgetTree>>,
    widget_plates: HashMap<InstanceId, Plate>,
    names: HashMap<InstanceId, String>,
    images_requested: ImageRequests,
    fonts: FontRegistry,
    bundled_icons: BundledIcons,
    service_images: ServiceImages,
    /// Natural size of every image the renderer holds, for bars made later.
    image_sizes: HashMap<Arc<str>, (u32, u32)>,
    pending: Option<Pending>,
    first_frame_done: bool,
    popups: Popups,
    /// `--capture`: every present is written out (offscreen runs with fake services only).
    capture: Option<CaptureState>,
}

/// The parts of the app the popup manager borrows for one call.
macro_rules! popup_env {
    ($s:ident, $ui:expr, $now:expr) => {
        Env {
            ui: $ui,
            renderer: &mut $s.renderer,
            bars: &mut $s.bars,
            icons: &mut $s.bundled_icons,
            sheet: &$s.sheet,
            dark: $s.dark,
            image_sizes: &$s.image_sizes,
            now: $now,
        }
    };
}

fn log_diag(d: &Diagnostic) {
    match d.severity {
        Severity::Error => log::error!("{d}"),
        Severity::Warning => log::warn!("{d}"),
    }
}

impl App {
    pub fn new(
        o: AppOptions,
        stats: Arc<Stats>,
        rx: std::sync::mpsc::Receiver<UiMsg>,
        script_tx: Sender<ScriptIn>,
        images: ImageLoader,
        renderer: Renderer,
        dark: bool,
    ) -> App {
        let popups = Popups::new(
            PopupOptions { no_hook: o.no_hook, no_activate: o.no_activate, offscreen: o.offscreen, gated: o.gated },
            Arc::clone(&stats),
            script_tx.clone(),
            Arc::clone(&o.overlays),
        );
        App {
            o,
            stats,
            rx,
            script_tx,
            reader: None,
            images,
            renderer,
            config: default_config(),
            applied: None,
            unpublished: None,
            tune_vars: Vec::new(),
            sheet: Arc::new(Stylesheet::empty()),
            table: InstanceTable::new(),
            bars: Vec::new(),
            dark,
            config_diags: Vec::new(),
            plan_diags: Vec::new(),
            font_diags: Vec::new(),
            scene_diags: Vec::new(),
            script_diags: Vec::new(),
            script_errors: BTreeMap::new(),
            stall: None,
            slow_bar: None,
            pipe_diag: None,
            bar_plate: None,
            trees: HashMap::new(),
            widget_plates: HashMap::new(),
            names: HashMap::new(),
            images_requested: ImageRequests::default(),
            fonts: FontRegistry::default(),
            bundled_icons: BundledIcons::default(),
            service_images: ServiceImages::default(),
            image_sizes: HashMap::new(),
            pending: None,
            first_frame_done: false,
            popups,
            capture: None,
        }
    }

    /// Where the versions of the shown files are published after every config swap.
    pub fn set_applied(&mut self, applied: Arc<AppliedState>) {
        self.applied = Some(applied);
    }

    /// Starts writing every presented frame (`--capture`).
    pub fn set_capture(&mut self, capture: Capture) {
        self.capture = Some(CaptureState::new(capture));
    }

    /// Runs `f` against the real window layer, wrapped to capture frames when a capture is on. When the capture
    /// reaches a limit the run ends (and says so): a test run that went on without recording would look healthy.
    fn with_live<R>(&mut self, ui: &mut Ui, f: impl FnOnce(&mut App, &mut dyn Win) -> R) -> R {
        let Some(mut state) = self.capture.take() else { return f(self, ui) };
        let result = {
            let mut live = Live { ui: &mut *ui, state: &mut state };
            f(self, &mut live)
        };
        state.flush();
        if let Some(limit) = state.stopped.take()
            && !state.reported
        {
            state.reported = true;
            log::warn!("the capture reached its {} limit; stopping", limit.name());
            self.stats.emit("capture", json!({"frames": state.capture.frames(), "limit": limit.name()}));
            ui.quit();
        }
        self.capture = Some(state);
        result
    }

    /// The frame count of a capture that is on, once the run is over.
    pub fn finish_capture(&mut self) {
        if let Some(state) = self.capture.as_mut() {
            state.capture.finish();
            if !state.reported {
                self.stats.emit("capture", json!({"frames": state.capture.frames(), "limit": null}));
            }
        }
    }

    pub fn set_reader(&mut self, reader: Reader) {
        self.reader = Some(reader);
    }

    pub fn take_reader(&mut self) -> Option<Reader> {
        self.reader.take()
    }

    /// Shown on every bar as a plate until the pipe is back (it never is).
    pub fn set_pipe_failure(&mut self, text: &str) {
        self.pipe_diag = Some(Diagnostic::error(DiagKind::Pipe, None, text));
    }

    pub fn renderer(&mut self) -> &mut Renderer {
        &mut self.renderer
    }

    /// Applies the first config and creates the bars (before the loop runs).
    pub fn start(&mut self, ui: &mut Ui, first: ConfigUpdate) {
        self.with_live(ui, |app, win| app.start_in(win, first));
    }

    /// `start` on any window layer.
    pub fn start_in(&mut self, ui: &mut dyn Win, first: ConfigUpdate) {
        ui.set_offscreen(self.o.offscreen);
        self.emit_workarea(ui, "start");
        self.apply_update(ui, first, true);
        self.emit_workarea(ui, "register");
        self.frame_all(ui);
    }

    /// One `workarea` stats line per monitor: its work area right now.
    pub fn emit_workarea(&self, ui: &dyn Win, when: &str) {
        if !self.stats.enabled() {
            return;
        }
        for m in ui.monitors() {
            let rect = ui.work_area(&m.device).map_or(json!(null), |r| json!([r.x0, r.y0, r.x1, r.y1]));
            self.stats.emit("workarea", json!({"when": when, "monitor": m.index, "rect": rect}));
        }
    }

    /// What `rsb inspect` needs from the bars and the open popups.
    fn inspect_part(&self, query: &crate::inspect::InspectQuery) -> crate::inspect::UiPart {
        use crate::inspect::{BarView, DEFAULT_ROOT_VARS, PopupView, ui_part};
        let tuned: Vec<&str> = self.tune_vars.iter().map(String::as_str).collect();
        let vars: &[&str] = if tuned.is_empty() { &DEFAULT_ROOT_VARS } else { &tuned };
        let bars: Vec<BarView> = self
            .bars
            .iter()
            .map(|b| BarView {
                name: &b.plan.spec.name,
                monitor: b.plan.monitor.index,
                scale: b.scale,
                scene: &b.scene,
                instances: b.insts.all().map(|p| (p.slot.instance, &p.spec.widget)).collect(),
            })
            .collect();
        let popups: Vec<PopupView> = self
            .popups
            .inspect_views()
            .into_iter()
            .map(|(instance, name, scene)| PopupView { instance, name, scene })
            .collect();
        ui_part(query, &bars, &popups, vars, clock::now())
    }

    pub fn bars_info(&self, ui: &dyn Win) -> Vec<BarInfo> {
        self.bars
            .iter()
            .map(|b| BarInfo {
                name: b.plan.spec.name.clone(),
                monitor: b.plan.monitor.index,
                visible: ui.is_visible(b.id),
                reserved: ui.is_reserved(b.id),
                render_failed: b.render_failed,
                plate: self.bar_plate.as_ref().map(Plate::display_text),
            })
            .collect()
    }

    // ---------------------------------------------------------------- inbox

    fn drain(&mut self, ui: &mut dyn Win) {
        while let Ok(msg) = self.rx.try_recv() {
            self.on_msg(ui, msg);
        }
    }

    fn on_msg(&mut self, ui: &mut dyn Win, msg: UiMsg) {
        match msg {
            UiMsg::Script(out) => self.on_script(out),
            UiMsg::Config(update) => self.apply_update(ui, update, false),
            UiMsg::Pipe { command, reply } => {
                let answer = self.on_command(ui, command);
                let _ = reply.send(answer);
            }
            UiMsg::BarsQuery { reply } => {
                let _ = reply.send(self.bars_info(ui));
            }
            UiMsg::Inspect { query, reply } => {
                let _ = reply.send(self.inspect_part(&query));
            }
            UiMsg::Image { key, result } => self.on_image(&key, result),
            UiMsg::Svg { key, result } => self.on_svg(&key, result),
            UiMsg::ServiceImage { key, image } => {
                let (w, h) = self.service_images.add(&mut self.renderer, Arc::clone(&key), &image);
                self.learn_image_size(&key, w, h);
            }
            UiMsg::PowerError => self.power_error(ui),
            UiMsg::DropAppImages => {
                self.remove_app_images();
            }
            UiMsg::DropServiceImage { key } => {
                if self.service_images.remove(&mut self.renderer, &key) {
                    self.image_sizes.remove(&key);
                }
            }
            UiMsg::Stalled { instance, for_ms } => {
                let name = self.name_of(instance);
                log::error!("script call in {name} running for {:.1} s", for_ms as f64 / 1000.0);
                self.stats.emit("stalled", json!({"widget": name, "ms": for_ms}));
                self.stall = Some(Diagnostic::error(
                    DiagKind::Stopped,
                    None,
                    format!("widgets are not responding ({name}); see log"),
                ));
                self.refresh_plate();
            }
            UiMsg::StalledCleared => {
                log::info!("the stalled script call returned");
                self.stall = None;
                self.refresh_plate();
            }
            UiMsg::WidgetFile { notified_wall_ms } => {
                self.pending = Some(Pending { kind: "luau", notified: notified_wall_ms, armed: false });
            }
            UiMsg::Sim(action) => {
                let _ = self.on_sim(ui, action);
            }
            UiMsg::Quit => ui.quit(),
        }
    }

    fn name_of(&self, instance: InstanceId) -> String {
        if instance.0 == 0 {
            "the script runtime".to_string()
        } else {
            self.names.get(&instance).cloned().unwrap_or_else(|| format!("widget {}", instance.0))
        }
    }

    fn on_script(&mut self, out: ScriptOut) {
        if let Some(p) = self.pending.as_mut().filter(|p| p.kind == "luau") {
            p.armed = true;
        }
        match out {
            ScriptOut::Trees(batch) => {
                for tree in batch {
                    self.trees.insert(tree.instance, tree.clone());
                    for bar in &mut self.bars {
                        bar.awaiting.remove(&tree.instance);
                        bar.scene.set_tree(tree.clone());
                    }
                    self.request_images(&tree);
                }
            }
            ScriptOut::Plate { instance, plate } => {
                let name = self.name_of(instance);
                match &plate {
                    Some(p) => {
                        log::warn!("plate on {name}: {}", p.display_text());
                        self.widget_plates.insert(instance, p.clone());
                    }
                    None => {
                        log::info!("plate on {name} cleared");
                        self.widget_plates.remove(&instance);
                    }
                }
                self.stats.emit("plate", json!({"kind": "widget", "shown": plate.is_some()}));
                for bar in &mut self.bars {
                    bar.awaiting.remove(&instance);
                    bar.scene.set_widget_plate(instance, plate.clone());
                }
            }
            ScriptOut::ErrorNote { instance, note } => {
                match note {
                    Some(d) => self.script_errors.insert(instance, d),
                    None => self.script_errors.remove(&instance),
                };
                self.refresh_plate();
            }
            ScriptOut::Diagnostics(list) => {
                // Logged by the script thread when it made them.
                for d in list {
                    if !self.script_diags.iter().any(|x| x.to_string() == d.to_string()) {
                        self.script_diags.push(d);
                    }
                }
                self.refresh_plate();
            }
        }
    }

    // ---------------------------------------------------------------- images

    fn request_images(&mut self, tree: &Arc<WidgetTree>) {
        let mut files = Vec::new();
        if let Some(root) = &tree.root {
            files_of(root, &mut files);
        }
        for (path, kind) in files {
            if !self.images_requested.should_request(&path, clock::now()) {
                continue;
            }
            match &tree.grant {
                Some(grant) => self.images.request(path, grant.clone(), kind),
                None => self.fail_image(&path, "the widget has no file permission"),
            }
        }
    }

    /// Every scene needs an image's size to lay it out; a bar made later
    /// (a monitor plugged in) is told the sizes already known.
    fn learn_image_size(&mut self, key: &Arc<str>, width: u32, height: u32) {
        self.image_sizes.insert(Arc::clone(key), (width, height));
        for bar in &mut self.bars {
            bar.scene.set_image_size(key, width, height);
        }
        self.popups.image_size(key, width, height);
    }

    fn fail_image(&mut self, key: &Arc<str>, why: &str) {
        log::warn!("an image could not be loaded: {why}");
        self.images_requested.failed(key, clock::now());
        for bar in &mut self.bars {
            bar.scene.image_failed(key);
        }
        self.popups.image_failed(key);
    }

    /// A user's `.svg` was parsed on the loader thread; the renderer keeps
    /// it and the scene learns its size.
    fn on_svg(&mut self, key: &Arc<str>, result: Result<rsb_render::ParsedSvg, String>) {
        match result {
            Ok(svg) => {
                let (w, h) = self.renderer.set_parsed_svg(key.clone(), svg);
                self.learn_image_size(key, w.ceil() as u32, h.ceil() as u32);
            }
            Err(e) => self.fail_image(key, &e),
        }
    }

    /// Drops every launcher icon from the renderer. The launcher popup calls
    /// this when no open popup uses them any more. Returns how many it held.
    pub fn remove_app_images(&mut self) -> usize {
        let held = self.service_images.remove_apps(&mut self.renderer);
        self.image_sizes.retain(|k, _| !k.starts_with(crate::service_images::APPS_PREFIX));
        log::debug!("dropped {held} launcher icons");
        self.stats.emit("apps_icons", json!({"dropped": held}));
        rsb_services::trim_heap_soon();
        held
    }

    fn on_image(&mut self, key: &Arc<str>, result: Result<rsb_render::Image, String>) {
        match result {
            Ok(image) => {
                self.learn_image_size(key, image.width, image.height);
                self.renderer.set_image(key.clone(), image);
            }
            Err(e) => self.fail_image(key, &e),
        }
    }

    // ---------------------------------------------------------------- plates

    /// The bar plate is the most important of everything currently wrong.
    fn refresh_plate(&mut self) {
        let mut all: Vec<Diagnostic> = Vec::new();
        all.extend(self.stall.clone());
        all.extend(self.pipe_diag.clone());
        all.extend(self.config_diags.iter().cloned());
        all.extend(self.plan_diags.iter().cloned());
        all.extend(self.font_diags.iter().cloned());
        all.extend(self.script_diags.iter().cloned());
        all.extend(self.script_errors.values().cloned());
        all.extend(self.scene_diags.iter().cloned());
        let plate = Plate::from_diagnostics(&all);
        if plate == self.bar_plate {
            return;
        }
        let shown = plate.is_some();
        let kind = plate.as_ref().map(|p| format!("{:?}", p.kind));
        self.stats.emit("plate", json!({"kind": kind.unwrap_or_else(|| "bar".into()), "shown": shown}));
        for bar in &mut self.bars {
            bar.scene.set_bar_plate(plate.clone());
        }
        self.bar_plate = plate;
    }

    // ---------------------------------------------------------------- config

    fn apply_update(&mut self, ui: &mut dyn Win, update: ConfigUpdate, first: bool) {
        let ConfigUpdate { kind, built, notified_wall_ms, hashes } = update;
        for d in &built.diagnostics {
            log_diag(d);
        }
        self.config_diags = built.diagnostics;
        let config_changed = built.config.is_some() || first;
        if let Some(config) = built.config {
            self.tune_vars = crate::assets::preset_about(&config.preset)
                .map(|about| about.tune.into_iter().map(|t| t.var).collect())
                .unwrap_or_default();
            self.config = config;
        }
        let sheet = built.sheet.or_else(|| first.then(|| built.base_sheet.clone()));
        if let Some(sheet) = sheet {
            self.sheet = sheet;
            self.scene_diags.clear();
            for bar in &mut self.bars {
                bar.scene.set_stylesheet(self.sheet.clone());
            }
            self.popups.set_stylesheet(&self.sheet);
            // Fonts first, so the warning below knows every family that is registered.
            let applied = self.fonts.apply(&mut self.renderer, &built.font_files);
            if applied.added > 0 {
                for bar in &mut self.bars {
                    bar.scene.fonts_changed();
                }
                self.popups.fonts_changed();
            }
            let mut font_diags = built.font_diagnostics;
            font_diags.extend(applied.diagnostics);
            for (name, pos) in built.fonts {
                if !self.renderer.has_family(&name) {
                    font_diags.push(Diagnostic::warning(
                        DiagKind::Style,
                        Some(pos),
                        format!("font '{name}' is not installed; using Segoe UI"),
                    ));
                }
            }
            self.font_diags = font_diags;
            for d in &self.font_diags {
                log_diag(d);
            }
        }
        if config_changed {
            self.script_diags.clear();
            self.reconcile(ui);
        }
        self.refresh_plate();
        // Published once the bars have drawn it (`publish_applied`): a caller that sees these versions finds the
        // look on screen, not only built. A file a second update leaves unused keeps the version the first one had.
        let diags = u32::try_from(self.config_diags.len()).unwrap_or(u32::MAX);
        let held = self.unpublished.map_or_else(AppliedHashes::default, |(h, _)| h);
        self.unpublished = Some((
            AppliedHashes {
                bar_toml: hashes.bar_toml.or(held.bar_toml),
                style_css: hashes.style_css.or(held.style_css),
            },
            diags,
        ));
        if kind != ReloadKind::Initial {
            self.pending = Some(Pending { kind: kind.name(), notified: notified_wall_ms, armed: true });
        }
        if first {
            self.stats.emit("config", json!({"bars": self.bars.len(), "preset": self.config.preset}));
        }
    }

    /// Makes the bar windows match the config and the monitors: windows that
    /// are already right are kept (a style edit never recreates a window),
    /// the rest are destroyed first and then created, and the script thread
    /// gets the new set of widget instances.
    fn reconcile(&mut self, ui: &mut dyn Win) {
        let monitors = ui.monitors();
        let (plans, plan_diags) = plan_bars(&self.config, &monitors);
        for d in &plan_diags {
            log_diag(d);
        }
        self.plan_diags = plan_diags;
        let scales: Vec<f32> = plans
            .iter()
            .map(|p| self.bars.iter().find(|b| b.key == p.key()).map_or(p.monitor.dpi as f32 / 96.0, |b| b.scale))
            .collect();
        let insts = build_instances(&self.config, &plans, &scales, &mut self.table);
        let wanted: Vec<(BarPlan, BarWindowSpec)> = plans
            .into_iter()
            .map(|p| {
                let w = window_spec(&p, self.o.no_reserve);
                (p, w)
            })
            .collect();

        let mut old: Vec<Option<BarRt>> = std::mem::take(&mut self.bars).into_iter().map(Some).collect();
        let mut claimed: HashSet<usize> = HashSet::new();
        let mut reuse: Vec<Option<usize>> = Vec::new();
        for (plan, win) in &wanted {
            let hit = old.iter().enumerate().position(|(i, b)| {
                !claimed.contains(&i) && b.as_ref().is_some_and(|b| b.key == plan.key() && b.win == *win)
            });
            if let Some(i) = hit {
                claimed.insert(i);
            }
            reuse.push(hit);
        }
        for (i, slot) in old.iter_mut().enumerate() {
            if !claimed.contains(&i)
                && let Some(b) = slot.take()
            {
                ui.destroy_bar(b.id);
            }
        }
        let mut bars = Vec::new();
        for (((plan, win), inst), r) in wanted.into_iter().zip(insts).zip(reuse) {
            let existing = r.and_then(|i| old[i].take());
            match existing {
                Some(mut b) => {
                    b.plan = plan;
                    b.scene.set_bar(&b.plan.spec);
                    b.scene.set_slots(inst.slots());
                    b.insts = inst;
                    bars.push(b);
                }
                None => match self.create_bar(ui, plan, win, inst) {
                    Some(b) => bars.push(b),
                    None => continue,
                },
            }
        }
        self.bars = bars;
        self.drop_orphan_popups(ui);
        self.names.clear();
        for bar in &self.bars {
            for p in bar.insts.all() {
                self.names.insert(p.spec.instance, p.spec.widget.to_string());
            }
        }
        let live: HashSet<InstanceId> = self.names.keys().copied().collect();
        self.trees.retain(|i, _| live.contains(i));
        self.widget_plates.retain(|i, _| live.contains(i));
        self.send_instances();
    }

    fn create_bar(
        &mut self,
        ui: &mut dyn Win,
        plan: BarPlan,
        win: BarWindowSpec,
        insts: BarInstances,
    ) -> Option<BarRt> {
        let id = match ui.create_bar(&win) {
            Ok(id) => id,
            Err(e) => {
                let d = Diagnostic::error(
                    DiagKind::Config,
                    plan.spec.pos.clone(),
                    format!("bar '{}' could not be created: {e}", win.name),
                );
                log_diag(&d);
                self.plan_diags.push(d);
                return None;
            }
        };
        let geometry = ui.geometry(id)?;
        let env =
            SceneEnv { media: MediaEnv { dark: self.dark }, icon_font: self.renderer.icon_font(), fullscreen: false };
        let mut scene = Scene::new(&plan.spec, geometry, env);
        scene.set_stylesheet(self.sheet.clone());
        // Slots first: trees for instances that are not in a slot are ignored.
        scene.set_slots(insts.slots());
        let mut awaiting = HashSet::new();
        for p in insts.all() {
            let id = p.spec.instance;
            if let Some(tree) = self.trees.get(&id) {
                scene.set_tree(tree.clone());
            } else if !self.widget_plates.contains_key(&id) {
                awaiting.insert(id);
            }
            if let Some(plate) = self.widget_plates.get(&id) {
                scene.set_widget_plate(id, Some(plate.clone()));
            }
        }
        scene.set_bar_plate(self.bar_plate.clone());
        for (key, (w, h)) in &self.image_sizes {
            scene.set_image_size(key, *w, *h);
        }
        Some(BarRt {
            key: plan.key(),
            id,
            win,
            scene,
            insts,
            awaiting,
            created: clock::now(),
            presented: false,
            scale: geometry.scale,
            cursor: Cursor::Default,
            backdrop: Backdrop::None,
            next_frame: None,
            render_failed: false,
            fullscreen: false,
            need_full: true,
            recover_at: None,
            plan,
        })
    }

    fn send_instances(&self) {
        let mut specs: Vec<InstanceSpec> = Vec::new();
        for bar in &self.bars {
            for p in bar.insts.all() {
                let mut spec = p.spec.clone();
                spec.monitor.scale = bar.scale;
                specs.push(spec);
            }
        }
        let _ = self.script_tx.send(ScriptIn::SetInstances(specs));
    }

    // ---------------------------------------------------------------- frames

    fn frame_all(&mut self, ui: &mut dyn Win) {
        let now = clock::now();
        for i in 0..self.bars.len() {
            self.frame_bar(ui, i, now);
        }
        let popup_next = self.frame_popups(ui, now);
        let next = self.bars.iter().filter_map(|b| b.next_frame).chain(popup_next).min();
        ui.set_frame_deadline(next);
        self.publish_applied();
        if let Some(p) = self.pending.take_if(|p| p.armed) {
            self.stats.emit(
                "reload",
                json!({"kind": p.kind, "notify_wall_ms": p.notified, "present_wall_ms": wall_ms(),
                       "delta_ms": wall_ms().saturating_sub(p.notified)}),
            );
        }
    }

    /// Tells the pipe thread which file versions the bars show, once every bar has drawn a frame since the swap (a
    /// bar whose drawing failed shows its plate, which is all it will show, so it does not hold this back).
    fn publish_applied(&mut self) {
        if self.unpublished.is_none() || !self.bars.iter().all(|b| b.presented || b.render_failed) {
            return;
        }
        if let (Some(applied), Some((hashes, diags))) = (&self.applied, self.unpublished.take()) {
            applied.set(hashes, diags);
        }
    }

    /// The popup surfaces: opened, closed, drawn and placed after the bars
    /// drew. A bar whose anchor changed state draws again in the same turn,
    /// so the strip and its panel start their transitions on one clock.
    fn frame_popups(&mut self, ui: &mut dyn Win, now: Instant) -> Option<Instant> {
        let result = self.popups.frame(&mut popup_env!(self, ui, now));
        let mut next = result.next;
        for bar in result.bars {
            if let Some(i) = self.bars.iter().position(|b| b.id == bar) {
                self.frame_bar(ui, i, now);
                next = next.into_iter().chain(self.bars[i].next_frame).min();
            }
        }
        let diags = self.popups.take_diagnostics();
        self.after_frame(diags);
        if self.popups.take_apps_released() {
            self.remove_app_images();
        }
        next
    }

    /// Bars whose windows are gone take their popups with them.
    fn drop_orphan_popups(&mut self, ui: &mut dyn Win) {
        let now = clock::now();
        self.popups.drop_orphans(&mut popup_env!(self, ui, now));
    }

    /// The power service reported an error: an overlay that was waiting for
    /// its action (the lock curtain) must not stay up.
    fn power_error(&mut self, ui: &mut dyn Win) {
        let now = clock::now();
        self.popups.close_overlays(&mut popup_env!(self, ui, now), CloseReason::Command);
        self.frame_all(ui);
    }

    /// Draws one bar; a draw that took a long time is remembered for the
    /// slow-turn report.
    fn frame_bar(&mut self, ui: &mut dyn Win, i: usize, now: Instant) {
        let started = Instant::now();
        self.frame_bar_inner(ui, i, now);
        let took = started.elapsed();
        if took >= SLOW_STEP && self.slow_bar.as_ref().is_none_or(|s| took > s.took) {
            let label = format!("bar {}", self.bars[i].plan.spec.name);
            self.slow_bar = Some(SlowStep { label, took, at: started });
        }
    }

    /// A loop turn that ran for a second or more drew nothing and answered
    /// nothing: say so, and name the slowest draw inside it when there was one.
    fn on_slow_turn(&mut self, ms: u64) {
        let began = Instant::now().checked_sub(Duration::from_millis(ms));
        let step = [self.slow_bar.take(), self.popups.take_slow_step()]
            .into_iter()
            .flatten()
            .filter(|s| began.is_none_or(|b| s.at >= b))
            .max_by_key(|s| s.took);
        let what = step.as_ref().map(|s| s.label.as_str());
        match what {
            Some(w) => log::warn!("the bar did not draw for {ms} ms; the slowest draw in it was {w}"),
            None => log::warn!("the bar did not draw for {ms} ms"),
        }
        self.stats.emit("stalled", json!({"ui_ms": ms, "drawing": what}));
    }

    fn frame_bar_inner(&mut self, ui: &mut dyn Win, i: usize, now: Instant) {
        // Recovery from an injected failure.
        if self.bars[i].recover_at.is_some_and(|t| now >= t) {
            let bar = &mut self.bars[i];
            bar.recover_at = None;
            bar.need_full = true;
        }
        let App { bars, renderer, stats, first_frame_done, bundled_icons, .. } = self;
        let bar = &mut bars[i];
        if bar.recover_at.is_some() {
            bar.next_frame = bar.recover_at;
            return;
        }
        if !bar.presented && !bar.awaiting.is_empty() && now < bar.created + FIRST_FRAME_WAIT {
            bar.next_frame = Some(bar.created + FIRST_FRAME_WAIT);
            return;
        }
        let built = Instant::now();
        let out = bar.scene.frame(now, renderer);
        let build_us = built.elapsed().as_micros() as u64;
        bar.next_frame = out.next_frame;
        let diags = bar.scene.take_diagnostics();
        let events = bar.scene.take_events();
        if out.cursor != bar.cursor {
            bar.cursor = out.cursor;
            ui.set_cursor(bar.id, out.cursor);
        }
        if out.backdrop != bar.backdrop {
            bar.backdrop = out.backdrop;
            ui.set_backdrop(bar.id, out.backdrop);
        }
        let region = if bar.need_full { None } else { out.dirty };
        if region.is_some() || bar.need_full {
            let started = Instant::now();
            let mut failure: Option<String> = None;
            let mut presented = None;
            let draw: &DrawList = &out.draw;
            bundled_icons.ensure(renderer, draw);
            let result = ui.present(bar.id, &mut |px| match renderer.render(draw, px, region) {
                Ok(rect) => {
                    presented = rect;
                    rect
                }
                Err(e) => {
                    failure = Some(e.to_string());
                    None
                }
            });
            if let Err(e) = result {
                failure = Some(e.to_string());
            }
            match failure {
                None => {
                    bar.need_full = false;
                    if renderer.svg_pending() {
                        // a user's picture waits for a frame with time to spare
                        bar.need_full = true;
                        bar.next_frame = Some(now);
                    }
                    if bar.render_failed {
                        bar.render_failed = false;
                        log::info!("drawing the bar works again");
                        stats.emit("plate", json!({"kind": "Render", "shown": false}));
                    }
                    if !bar.presented {
                        bar.presented = true;
                        if !*first_frame_done {
                            *first_frame_done = true;
                            ui.first_frame_drawn();
                            stats.emit("first_frame", json!({"ms": since_process_start_ms()}));
                        }
                    }
                    if presented.is_some() {
                        stats.note_present();
                    }
                    if stats.frames
                        && let Some(r) = presented
                    {
                        stats.emit(
                            "frame",
                            json!({"surface": format!("bar:{}", bar.plan.spec.name),
                                   "dirty": [r.x0, r.y0, r.x1, r.y1], "render_us": started.elapsed().as_micros() as u64,
                                   "build_us": build_us, "style_us": out.stats.style_us, "layout_us": out.stats.layout_us,
                                   "paint_us": out.stats.draw_us,
                                   "compositor_frame": ui.compositor_frame()}),
                        );
                    }
                }
                Some(why) => Self::fail_bar(ui, renderer, stats, bar, &why),
            }
        }
        self.after_frame(diags);
        self.popups.send_bar_events(events, 0);
    }

    /// Logs scene warnings once and keeps them for the plate.
    fn after_frame(&mut self, diags: Vec<rsb_core::scene::SceneDiagnostic>) {
        let mut changed = false;
        for d in diags {
            log_diag(&d.diagnostic);
            // A style problem inside a widget (a bad custom property the preset uses) is the user's to fix
            // too, so it reaches the bar plate whichever widget drew it.
            if !self.scene_diags.iter().any(|x| x.to_string() == d.diagnostic.to_string()) {
                self.scene_diags.push(d.diagnostic);
                changed = true;
            }
        }
        if changed {
            self.refresh_plate();
        }
    }

    /// The failure path: the last good frame stays and the fixed plate is
    /// drawn over its left corner.
    fn fail_bar(ui: &mut dyn Win, renderer: &mut Renderer, stats: &Stats, bar: &mut BarRt, why: &str) {
        if !bar.render_failed {
            log::error!("could not draw the bar: {why}");
            stats.emit("plate", json!({"kind": "Render", "shown": true}));
        }
        bar.render_failed = true;
        bar.need_full = true;
        let scale = bar.scale;
        let _ = ui.present(bar.id, &mut |px| fallback::draw(renderer, px, scale));
        bar.next_frame = Some(clock::now() + Duration::from_secs(1));
    }

    // ---------------------------------------------------------------- input

    fn on_pointer(&mut self, ui: &mut dyn Win, bar_id: BarId, event: rsb_core::hit::PointerEvent) {
        let now = clock::now();
        let Some(i) = self.bars.iter().position(|b| b.id == bar_id) else { return };
        let gesture = self.popups.pointer_gesture(event.kind, now);
        if matches!(event.kind, PointerKind::Down(_)) {
            // A press on a bar is outside every open popup unless it is on the popup's anchor.
            self.popups.on_bar_press(&mut popup_env!(self, ui, now), bar_id, event.x, event.y, gesture);
        }
        let result = self.bars[i].scene.pointer(event, now);
        for call in &result.calls {
            self.stats.emit_with("handler", || json!({"widget": self.name_of(call.instance), "kind": call_kind(call)}));
        }
        self.popups.send_calls(result.calls.clone(), gesture);
        let events = self.bars[i].scene.take_events();
        self.popups.send_bar_events(events, gesture);
        if result.plate_click.is_some() {
            ui.open_path(&self.o.log_file);
        }
        if result.unhandled == Some(Button::Right) {
            self.stats.emit("menu_requested", json!({"bar": self.bars[i].plan.spec.name}));
            if !self.o.simulated_pointer {
                ui.show_menu(bar_id, event.x as i32, event.y as i32);
            }
        }
        self.popups.bar_pointer_done(&mut popup_env!(self, ui, now), bar_id, event.kind);
        self.frame_all(ui);
    }

    // ---------------------------------------------------------------- commands

    fn on_command(&mut self, ui: &mut dyn Win, command: Command) -> Reply {
        match command {
            Command::Reload => {
                if let Some(r) = &self.reader {
                    r.read_all();
                }
                Reply::Ok("reloading".into())
            }
            Command::Quit => {
                ui.quit();
                Reply::Ok("stopping".into())
            }
            Command::Toggle(name) => {
                let ids: Vec<BarId> = self
                    .bars
                    .iter()
                    .filter(|b| name.as_ref().is_none_or(|n| *n == b.plan.spec.name))
                    .map(|b| b.id)
                    .collect();
                if ids.is_empty() {
                    return Reply::Err(format!("no bar named '{}'", name.unwrap_or_default()));
                }
                let show = !ids.iter().any(|id| ui.is_visible(*id));
                let now = clock::now();
                for id in ids {
                    if !show {
                        self.popups.close_bar(&mut popup_env!(self, ui, now), id, CloseReason::BarHidden);
                    }
                    ui.set_visible(id, show);
                }
                Reply::Ok(if show { "shown" } else { "hidden" }.into())
            }
            Command::Popup { name, action } => {
                let now = clock::now();
                self.popups.command(&mut popup_env!(self, ui, now), &name, action)
            }
            Command::Trigger { event, json } => {
                let data = json
                    .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
                    .map_or(Value::Nil, |v| Value::from_json(&v));
                let _ = self.script_tx.send(ScriptIn::Trigger { event, data });
                Reply::Ok("sent".into())
            }
            Command::Status => Reply::Err("status is answered by the pipe thread".into()),
            Command::Applied | Command::Settings(_) | Command::Inspect { .. } => {
                Reply::Err("this command is answered by the pipe thread".into())
            }
        }
    }

    // ---------------------------------------------------------------- test flags

    /// Plays one test action. A target that is not there (no such bar, no open popup of that name) is logged and
    /// also returned, so a scripted run can say so instead of looking healthy.
    pub fn on_sim(&mut self, ui: &mut dyn Win, action: SimAction) -> Option<String> {
        let mut problem: Option<String> = None;
        let warn = |m: &str| {
            log::warn!("{m}");
            Some(m.to_string())
        };
        let ids: Vec<BarId> = self.bars.iter().map(|b| b.id).collect();
        match action {
            SimAction::Quit => ui.quit(),
            SimAction::TaskbarCreated => ui.inject(TestInput::TaskbarCreated),
            SimAction::Dpi(dpi) => {
                for bar in ids {
                    ui.inject(TestInput::Dpi { bar, dpi });
                }
            }
            SimAction::DisplayChange => ui.inject(TestInput::DisplayChange),
            SimAction::Theme(dark) => ui.inject(TestInput::Theme { dark }),
            SimAction::Fullscreen(active) => {
                for bar in ids {
                    ui.inject(TestInput::FullscreenNotice { bar, active });
                }
            }
            SimAction::Clock(crate::args::ClockKind::Time) => ui.inject(TestInput::TimeChange),
            SimAction::Clock(crate::args::ClockKind::Resume) => ui.inject(TestInput::Resume),
            SimAction::FullscreenWindow { for_ms } => {
                if let Some(m) = ui.monitors().into_iter().find(|m| m.primary) {
                    match rsb_win::spawn_fullscreen_test_window(&m.device, Duration::from_millis(for_ms)) {
                        Ok(()) => log::info!("a fullscreen test window opened for {for_ms} ms"),
                        Err(e) => log::warn!("the fullscreen test window could not open: {e}"),
                    }
                }
            }
            SimAction::Pointer { surface, event } => match surface {
                SimSurface::Bar(bar) => {
                    if let Some(b) = self.bars.iter().find(|b| b.plan.spec.name == bar) {
                        ui.inject(TestInput::Pointer { bar: b.id, event });
                    } else {
                        problem = warn("the pointer script names a bar that does not exist");
                    }
                }
                SimSurface::Popup(name) => match self.popups.popup_window(&name) {
                    Some(popup) => ui.inject(TestInput::PopupPointer { popup, event }),
                    None => problem = warn("the pointer script names a popup that is not open"),
                },
                SimSurface::Dim(name) => match self.popups.dim_window(&name) {
                    Some(popup) => ui.inject(TestInput::PopupPointer { popup, event }),
                    None => problem = warn("the pointer script names an overlay that is not open"),
                },
            },
            SimAction::Key { popup, event } => match self.popups.popup_window(&popup) {
                Some(popup) => ui.inject(TestInput::Key { popup, event }),
                None => problem = warn("the key script names a popup that is not open"),
            },
            SimAction::Text { popup, input } => match self.popups.popup_window(&popup) {
                Some(popup) => ui.inject(TestInput::Text { popup, input }),
                None => problem = warn("the key script names a popup that is not open"),
            },
            SimAction::Popup { name, action } => {
                let now = clock::now();
                if let Reply::Err(e) = self.popups.command(&mut popup_env!(self, ui, now), &name, action) {
                    problem = warn(&format!("simulated rsb popup: {e}"));
                }
            }
            SimAction::OutsidePress => ui.inject(TestInput::OutsidePress(OutsidePress { x: -32000, y: -32000 })),
            SimAction::Deactivate { popup } => match self.popups.popup_window(&popup) {
                Some(popup) => ui.inject(TestInput::Deactivate { popup }),
                None => problem = warn("the deactivate flag names a popup that is not open"),
            },
            SimAction::Session { locked } => ui.inject(TestInput::SessionLock { locked }),
            SimAction::PopupDpi { popup, dpi } => match self.popups.popup_window(&popup) {
                Some(popup) => ui.inject(TestInput::PopupDpi { popup, dpi }),
                None => problem = warn("the popup dpi flag names a popup that is not open"),
            },
            SimAction::UiStall { for_ms } => {
                log::info!("the UI thread stops for {for_ms} ms (test flag)");
                std::thread::sleep(Duration::from_millis(for_ms));
            }
            SimAction::Panic => panic!("panic injected by a test flag"),
            SimAction::RenderFailure => {
                let App { bars, renderer, stats, .. } = self;
                for bar in bars.iter_mut() {
                    Self::fail_bar(ui, renderer, stats, bar, "injected by a test flag");
                    bar.recover_at = Some(clock::now() + INJECTED_FAILURE_FOR);
                }
            }
        }
        problem
    }

    /// Widgets that have not yet reported a tree or a plate, over every bar.
    pub fn awaiting(&self) -> usize {
        self.bars.iter().map(|b| b.awaiting.len()).sum()
    }

    /// Every bar has drawn at least once.
    pub fn all_presented(&self) -> bool {
        self.bars.iter().all(|b| b.presented)
    }
}

fn call_kind(call: &HandlerCall) -> String {
    match call.kind {
        InputKind::Click(b) => format!("click_{}", b.name()),
        InputKind::Wheel(_) => "wheel".into(),
        InputKind::Enter => "enter".into(),
        InputKind::Leave => "leave".into(),
    }
}

impl UiHandler for App {
    fn handle(&mut self, ui: &mut Ui, event: UiEvent) {
        self.with_live(ui, |app, win| app.handle_in(win, event));
    }
}

impl App {
    /// One event from the window layer, on any window layer.
    pub fn handle_in(&mut self, ui: &mut dyn Win, event: UiEvent) {
        match event {
            UiEvent::Wake => {
                self.drain(ui);
                self.frame_all(ui);
            }
            UiEvent::FrameDue => self.frame_all(ui),
            UiEvent::SlowTurn { ms } => self.on_slow_turn(ms),
            UiEvent::Pointer { bar, event } => self.on_pointer(ui, bar, event),
            UiEvent::Geometry { bar, geometry } => {
                let mut resend = false;
                if let Some(b) = self.bars.iter_mut().find(|b| b.id == bar) {
                    resend = (b.scale - geometry.scale).abs() > f32::EPSILON;
                    b.scale = geometry.scale;
                    b.scene.set_geometry(BarGeometry { ..geometry });
                    b.need_full = true;
                    self.stats.emit(
                        "geometry",
                        json!({"bar": b.plan.spec.name, "w": geometry.width_px, "h": geometry.height_px,
                               "dpi": (geometry.scale * 96.0).round() as u32}),
                    );
                }
                if resend {
                    self.send_instances();
                    let now = clock::now();
                    self.popups.close_bar(&mut popup_env!(self, ui, now), bar, CloseReason::DisplayChange);
                }
                self.frame_all(ui);
            }
            UiEvent::MonitorsChanged => {
                log::info!("monitors changed; rebuilding the bars");
                let now = clock::now();
                self.popups.close_all(&mut popup_env!(self, ui, now), CloseReason::DisplayChange);
                self.reconcile(ui);
                self.refresh_plate();
                self.frame_all(ui);
                self.emit_workarea(ui, "display_change");
            }
            UiEvent::ThemeChanged { dark } => {
                log::info!("Windows switched to the {} theme", if dark { "dark" } else { "light" });
                self.dark = dark;
                let icon_font = self.renderer.icon_font();
                for b in &mut self.bars {
                    let fullscreen = b.plan.spec.fullscreen == FullscreenMode::Stay && b.fullscreen;
                    b.scene.set_env(SceneEnv { media: MediaEnv { dark }, icon_font, fullscreen });
                }
                self.popups.set_env(dark, icon_font);
                let _ = self.script_tx.send(ScriptIn::Theme { dark });
                self.frame_all(ui);
            }
            UiEvent::Fullscreen { bar, active } => {
                log::info!("a fullscreen app {} this bar's monitor", if active { "came to" } else { "left" });
                let icon_font = self.renderer.icon_font();
                let dark = self.dark;
                let mut stay = false;
                if let Some(b) = self.bars.iter_mut().find(|b| b.id == bar) {
                    stay = b.plan.spec.fullscreen == FullscreenMode::Stay;
                    let fullscreen = active && stay;
                    b.fullscreen = active;
                    b.scene.set_env(SceneEnv { media: MediaEnv { dark }, icon_font, fullscreen });
                }
                let now = clock::now();
                self.popups.on_fullscreen(&mut popup_env!(self, ui, now), bar, active, stay);
                self.frame_all(ui);
            }
            UiEvent::TaskbarCreated => {
                log::info!("the shell restarted; appbars were registered again");
                self.emit_workarea(ui, "taskbar_created");
            }
            UiEvent::Menu { item, .. } => match item {
                MenuItem::Reload => {
                    if let Some(r) = &self.reader {
                        r.read_all();
                    }
                }
                MenuItem::OpenConfig => ui.open_path(&self.o.config_dir),
                MenuItem::OpenLog => ui.open_path(&self.o.log_file),
                MenuItem::Quit => ui.quit(),
            },
            UiEvent::ClockChanged => {
                log::info!("the clock changed (resume or a manual change); timers re-armed");
                let _ = self.script_tx.send(ScriptIn::ClockChanged);
            }
            UiEvent::CloseRequested => ui.quit(),
            UiEvent::PopupPointer { popup, event } => {
                let now = clock::now();
                self.popups.on_popup_pointer(&mut popup_env!(self, ui, now), popup, event);
                self.frame_all(ui);
            }
            UiEvent::PopupGeometry { popup, geometry } => {
                self.popups.on_popup_geometry(popup, geometry);
                self.frame_all(ui);
            }
            UiEvent::PopupDpiChanged { popup, .. } => {
                let now = clock::now();
                self.popups.on_popup_dpi(&mut popup_env!(self, ui, now), popup);
                self.frame_all(ui);
            }
            UiEvent::Key { popup, event } => {
                let now = clock::now();
                self.popups.on_key(&mut popup_env!(self, ui, now), popup, event);
                self.frame_all(ui);
            }
            UiEvent::Text { popup, input } => {
                let now = clock::now();
                self.popups.on_text(&mut popup_env!(self, ui, now), popup, input);
                self.frame_all(ui);
            }
            UiEvent::PopupFocus { popup, focused } => {
                let now = clock::now();
                self.popups.on_focus(&mut popup_env!(self, ui, now), popup, focused);
                self.frame_all(ui);
            }
            UiEvent::OutsidePress(_) => {
                let now = clock::now();
                self.popups.on_outside_press(&mut popup_env!(self, ui, now));
                self.frame_all(ui);
            }
            UiEvent::Clipboard { token, text } => {
                let now = clock::now();
                self.popups.on_clipboard(&mut popup_env!(self, ui, now), token, text);
                self.frame_all(ui);
            }
            UiEvent::SessionLock { locked } => {
                let now = clock::now();
                self.popups.on_session(&mut popup_env!(self, ui, now), locked);
                self.frame_all(ui);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use rsb_core::wake::Wake;
    use rsb_render::FontConfig;

    use super::*;
    use crate::cfgload::build_cached;
    use crate::fonts::FontCache;
    use crate::headless::Headless;
    use crate::testutil::TempDir;

    struct NoWake;
    impl Wake for NoWake {
        fn wake(&self) {}
    }

    fn update(dir: &TempDir, kind: ReloadKind) -> ConfigUpdate {
        let built = build_cached(dir.path(), None, "default", &mut FontCache::default());
        ConfigUpdate { kind, hashes: built.hashes, built, notified_wall_ms: 0 }
    }

    #[test]
    fn a_swapped_in_version_is_published_after_the_bars_have_drawn_it() {
        let dir = TempDir::new("app-applied");
        let (ui_tx, ui_rx) = channel::<UiMsg>();
        let (script_tx, _script_rx) = channel::<ScriptIn>();
        let o = AppOptions {
            config_dir: dir.path().to_path_buf(),
            log_file: dir.path().join("rsb.log"),
            no_reserve: true,
            offscreen: true,
            simulated_pointer: true,
            no_hook: true,
            no_activate: true,
            gated: true,
            overlays: Arc::new(OverlayWatch::default()),
        };
        let mut app = App::new(
            o,
            Arc::new(Stats::off()),
            ui_rx,
            script_tx,
            ImageLoader::start(ui_tx, Arc::new(NoWake)),
            Renderer::new(FontConfig::default()),
            true,
        );
        let state = Arc::new(AppliedState::new("c".into(), "l".into()));
        app.set_applied(Arc::clone(&state));
        let mut win = Headless::new(&[(1920, 1080, 96)]);
        let first = update(&dir, ReloadKind::Initial);
        let first_css = first.hashes.style_css;
        app.start_in(&mut win, first);
        // No widget has reported, so a new bar waits for its first frame.
        assert_eq!(state.snapshot().hashes.style_css, None, "nothing is drawn yet");
        std::thread::sleep(FIRST_FRAME_WAIT + Duration::from_millis(30));
        app.frame_all(&mut win);
        assert_eq!(state.snapshot().hashes.style_css, first_css, "the first look is published after its first frame");

        std::fs::write(dir.path().join("style.css"), ":root { --accent: #123456; }\n").unwrap();
        let second = update(&dir, ReloadKind::Css);
        let second_css = second.hashes.style_css;
        assert_ne!(first_css, second_css);
        app.apply_update(&mut win, second, false);
        assert_eq!(state.snapshot().hashes.style_css, first_css, "built is not drawn: not shown yet");
        app.frame_all(&mut win);
        assert_eq!(state.snapshot().hashes.style_css, second_css, "drawn: shown");
    }
}

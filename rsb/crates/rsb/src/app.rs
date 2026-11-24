//! The UI thread's side of the bar: bars, scenes, the renderer, presenting
//! frames, input, config reloads, plates and the test flags. Everything that
//! blocks or reads a file lives on another thread; this one only drains its
//! inbox (`UiMsg`) and draws.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use rsb_core::config::{Config, FullscreenMode, default_config};
use rsb_core::css::{MediaEnv, Stylesheet};
use rsb_core::diag::{DiagKind, Diagnostic, Plate, Severity};
use rsb_core::draw::DrawList;
use rsb_core::element::{Element, ElementKind, InstanceId, WidgetTree};
use rsb_core::hit::{Button, HandlerCall, InputKind};
use rsb_core::ipc::{Command, Reply};
use rsb_core::msg::{InstanceSpec, ScriptIn, ScriptOut};
use rsb_core::scene::{BarGeometry, Scene, SceneEnv};
use rsb_core::style::{Backdrop, Cursor};
use rsb_core::value::Value;
use rsb_render::Renderer;
use rsb_win::{BarId, BarWindowSpec, MenuItem, TestInput, Ui, UiEvent, UiHandler};
use serde_json::json;

use crate::fallback;
use crate::images::{ImageLoader, ImageRequests};
use crate::msg::{BarInfo, ConfigUpdate, ReloadKind, SimAction, UiMsg};
use crate::plan::{BarInstances, BarPlan, InstanceTable, build_instances, plan_bars};
use crate::reader::Reader;
use crate::stats::{Stats, since_process_start_ms, wall_ms};

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

struct BarRt {
    key: String,
    id: BarId,
    plan: BarPlan,
    win: BarWindowSpec,
    scene: Scene,
    insts: BarInstances,
    /// Instances that have not yet sent a first tree or plate.
    awaiting: HashSet<InstanceId>,
    created: Instant,
    presented: bool,
    scale: f32,
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
    sheet: Arc<Stylesheet>,
    table: InstanceTable,
    bars: Vec<BarRt>,
    dark: bool,
    config_diags: Vec<Diagnostic>,
    plan_diags: Vec<Diagnostic>,
    font_diags: Vec<Diagnostic>,
    scene_diags: Vec<Diagnostic>,
    script_diags: Vec<Diagnostic>,
    stall: Option<Diagnostic>,
    pipe_diag: Option<Diagnostic>,
    bar_plate: Option<Plate>,
    trees: HashMap<InstanceId, Arc<WidgetTree>>,
    widget_plates: HashMap<InstanceId, Plate>,
    names: HashMap<InstanceId, String>,
    images_requested: ImageRequests,
    pending: Option<Pending>,
    first_frame_done: bool,
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
        App {
            o,
            stats,
            rx,
            script_tx,
            reader: None,
            images,
            renderer,
            config: default_config(),
            sheet: Arc::new(Stylesheet::empty()),
            table: InstanceTable::new(),
            bars: Vec::new(),
            dark,
            config_diags: Vec::new(),
            plan_diags: Vec::new(),
            font_diags: Vec::new(),
            scene_diags: Vec::new(),
            script_diags: Vec::new(),
            stall: None,
            pipe_diag: None,
            bar_plate: None,
            trees: HashMap::new(),
            widget_plates: HashMap::new(),
            names: HashMap::new(),
            images_requested: ImageRequests::default(),
            pending: None,
            first_frame_done: false,
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
        ui.set_offscreen(self.o.offscreen);
        self.emit_workarea(ui, "start");
        self.apply_update(ui, first, true);
        self.emit_workarea(ui, "register");
        self.frame_all(ui);
    }

    /// One `workarea` stats line per monitor: its work area right now.
    pub fn emit_workarea(&self, ui: &Ui, when: &str) {
        if !self.stats.enabled() {
            return;
        }
        for m in ui.monitors() {
            let rect = ui.work_area(&m.device).map_or(json!(null), |r| json!([r.x0, r.y0, r.x1, r.y1]));
            self.stats.emit("workarea", json!({"when": when, "monitor": m.index, "rect": rect}));
        }
    }

    pub fn bars_info(&self, ui: &Ui) -> Vec<BarInfo> {
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

    fn drain(&mut self, ui: &mut Ui) {
        while let Ok(msg) = self.rx.try_recv() {
            self.on_msg(ui, msg);
        }
    }

    fn on_msg(&mut self, ui: &mut Ui, msg: UiMsg) {
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
            UiMsg::Image { key, result } => self.on_image(&key, result),
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
            UiMsg::Sim(action) => self.on_sim(ui, action),
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
        fn walk(e: &Element, out: &mut Vec<Arc<str>>) {
            if e.kind == ElementKind::Image
                && let Some(img) = &e.image
            {
                out.push(img.path.clone());
            }
            for c in &e.children {
                walk(c, out);
            }
        }
        let mut paths = Vec::new();
        if let Some(root) = &tree.root {
            walk(root, &mut paths);
        }
        for path in paths {
            if !self.images_requested.should_request(&path, Instant::now()) {
                continue;
            }
            match &tree.grant {
                Some(grant) => self.images.request(path, grant.clone()),
                None => self.on_image(&path, Err("the widget has no file permission".into())),
            }
        }
    }

    fn on_image(&mut self, key: &Arc<str>, result: Result<rsb_render::Image, String>) {
        match result {
            Ok(image) => {
                for bar in &mut self.bars {
                    bar.scene.set_image_size(key, image.width, image.height);
                }
                self.renderer.set_image(key.clone(), image);
            }
            Err(e) => {
                log::warn!("an image could not be loaded: {e}");
                self.images_requested.failed(key, Instant::now());
                for bar in &mut self.bars {
                    bar.scene.image_failed(key);
                }
            }
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

    fn apply_update(&mut self, ui: &mut Ui, update: ConfigUpdate, first: bool) {
        let ConfigUpdate { kind, built, notified_wall_ms } = update;
        for d in &built.diagnostics {
            log_diag(d);
        }
        self.config_diags = built.diagnostics;
        let config_changed = built.config.is_some() || first;
        if let Some(config) = built.config {
            self.config = config;
        }
        let sheet = built.sheet.or_else(|| first.then(|| built.base_sheet.clone()));
        if let Some(sheet) = sheet {
            self.sheet = sheet;
            self.scene_diags.clear();
            for bar in &mut self.bars {
                bar.scene.set_stylesheet(self.sheet.clone());
            }
            self.font_diags = built
                .fonts
                .into_iter()
                .filter(|(name, _)| !self.renderer.has_family(name))
                .map(|(name, pos)| {
                    Diagnostic::warning(
                        DiagKind::Style,
                        Some(pos),
                        format!("font '{name}' is not installed; using Segoe UI"),
                    )
                })
                .collect();
            for d in &self.font_diags {
                log_diag(d);
            }
        }
        if config_changed {
            self.script_diags.clear();
            self.reconcile(ui);
        }
        self.refresh_plate();
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
    fn reconcile(&mut self, ui: &mut Ui) {
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

    fn create_bar(&mut self, ui: &mut Ui, plan: BarPlan, win: BarWindowSpec, insts: BarInstances) -> Option<BarRt> {
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
        Some(BarRt {
            key: plan.key(),
            id,
            win,
            scene,
            insts,
            awaiting,
            created: Instant::now(),
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

    fn frame_all(&mut self, ui: &mut Ui) {
        let now = Instant::now();
        for i in 0..self.bars.len() {
            self.frame_bar(ui, i, now);
        }
        let next = self.bars.iter().filter_map(|b| b.next_frame).min();
        ui.set_frame_deadline(next);
        if let Some(p) = self.pending.take_if(|p| p.armed) {
            self.stats.emit(
                "reload",
                json!({"kind": p.kind, "notify_wall_ms": p.notified, "present_wall_ms": wall_ms(),
                       "delta_ms": wall_ms().saturating_sub(p.notified)}),
            );
        }
    }

    fn frame_bar(&mut self, ui: &mut Ui, i: usize, now: Instant) {
        // Recovery from an injected failure.
        if self.bars[i].recover_at.is_some_and(|t| now >= t) {
            let bar = &mut self.bars[i];
            bar.recover_at = None;
            bar.need_full = true;
        }
        let App { bars, renderer, stats, first_frame_done, .. } = self;
        let bar = &mut bars[i];
        if bar.recover_at.is_some() {
            bar.next_frame = bar.recover_at;
            return;
        }
        if !bar.presented && !bar.awaiting.is_empty() && now < bar.created + FIRST_FRAME_WAIT {
            bar.next_frame = Some(bar.created + FIRST_FRAME_WAIT);
            return;
        }
        let out = bar.scene.frame(now, renderer);
        bar.next_frame = out.next_frame;
        let diags = bar.scene.take_diagnostics();
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
                    if bar.render_failed {
                        bar.render_failed = false;
                        log::info!("drawing the bar works again");
                        stats.emit("plate", json!({"kind": "Render", "shown": false}));
                    }
                    if !bar.presented {
                        bar.presented = true;
                        if !*first_frame_done {
                            *first_frame_done = true;
                            stats.emit("first_frame", json!({"ms": since_process_start_ms()}));
                        }
                    }
                    if stats.frames
                        && let Some(r) = presented
                    {
                        stats.emit(
                            "frame",
                            json!({"dirty": [r.x0, r.y0, r.x1, r.y1], "render_us": started.elapsed().as_micros() as u64,
                                   "style_us": out.stats.style_us, "layout_us": out.stats.layout_us}),
                        );
                    }
                }
                Some(why) => Self::fail_bar(ui, renderer, stats, bar, &why),
            }
        }
        self.after_frame(diags);
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
    fn fail_bar(ui: &mut Ui, renderer: &mut Renderer, stats: &Stats, bar: &mut BarRt, why: &str) {
        if !bar.render_failed {
            log::error!("could not draw the bar: {why}");
            stats.emit("plate", json!({"kind": "Render", "shown": true}));
        }
        bar.render_failed = true;
        bar.need_full = true;
        let scale = bar.scale;
        let _ = ui.present(bar.id, &mut |px| fallback::draw(renderer, px, scale));
        bar.next_frame = Some(Instant::now() + Duration::from_secs(1));
    }

    // ---------------------------------------------------------------- input

    fn on_pointer(&mut self, ui: &mut Ui, bar_id: BarId, event: rsb_core::hit::PointerEvent) {
        let now = Instant::now();
        let Some(i) = self.bars.iter().position(|b| b.id == bar_id) else { return };
        let result = self.bars[i].scene.pointer(event, now);
        for call in &result.calls {
            let _ = self.script_tx.send(ScriptIn::Input(*call));
            self.stats.emit("handler", json!({"widget": self.name_of(call.instance), "kind": call_kind(call)}));
        }
        if result.plate_click.is_some() {
            ui.open_path(&self.o.log_file);
        }
        if result.unhandled == Some(Button::Right) {
            self.stats.emit("menu_requested", json!({"bar": self.bars[i].plan.spec.name}));
            if !self.o.simulated_pointer {
                ui.show_menu(bar_id, event.x as i32, event.y as i32);
            }
        }
        self.frame_all(ui);
    }

    // ---------------------------------------------------------------- commands

    fn on_command(&mut self, ui: &mut Ui, command: Command) -> Reply {
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
                for id in ids {
                    ui.set_visible(id, show);
                }
                Reply::Ok(if show { "shown" } else { "hidden" }.into())
            }
            Command::Trigger { event, json } => {
                let data = json
                    .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
                    .map_or(Value::Nil, |v| Value::from_json(&v));
                let _ = self.script_tx.send(ScriptIn::Trigger { event, data });
                Reply::Ok("sent".into())
            }
            Command::Status => Reply::Err("status is answered by the pipe thread".into()),
        }
    }

    // ---------------------------------------------------------------- test flags

    fn on_sim(&mut self, ui: &mut Ui, action: SimAction) {
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
            SimAction::Pointer { bar, event } => {
                if let Some(b) = self.bars.iter().find(|b| b.plan.spec.name == bar) {
                    ui.inject(TestInput::Pointer { bar: b.id, event });
                } else {
                    log::warn!("the pointer script names a bar that does not exist");
                }
            }
            SimAction::Panic => panic!("panic injected by a test flag"),
            SimAction::RenderFailure => {
                let App { bars, renderer, stats, .. } = self;
                for bar in bars.iter_mut() {
                    Self::fail_bar(ui, renderer, stats, bar, "injected by a test flag");
                    bar.recover_at = Some(Instant::now() + INJECTED_FAILURE_FOR);
                }
            }
        }
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
        match event {
            UiEvent::Wake => {
                self.drain(ui);
                self.frame_all(ui);
            }
            UiEvent::FrameDue => self.frame_all(ui),
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
                }
                self.frame_all(ui);
            }
            UiEvent::MonitorsChanged => {
                log::info!("monitors changed; rebuilding the bars");
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
                let _ = self.script_tx.send(ScriptIn::Theme { dark });
                self.frame_all(ui);
            }
            UiEvent::Fullscreen { bar, active } => {
                log::info!("a fullscreen app {} this bar's monitor", if active { "came to" } else { "left" });
                let icon_font = self.renderer.icon_font();
                let dark = self.dark;
                if let Some(b) = self.bars.iter_mut().find(|b| b.id == bar) {
                    let fullscreen = active && b.plan.spec.fullscreen == FullscreenMode::Stay;
                    b.fullscreen = active;
                    b.scene.set_env(SceneEnv { media: MediaEnv { dark }, icon_font, fullscreen });
                }
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
        }
    }
}

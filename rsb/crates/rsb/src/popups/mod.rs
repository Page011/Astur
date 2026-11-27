//! The popup manager, on the UI thread. It owns the windows and scenes of
//! every popup, menu, tooltip and overlay; the script decides when one
//! should be open (`PopupProps::open`), and this decides everything the
//! screen decides: where it goes, when it closes on its own, who holds
//! keyboard focus, and what the script is told.
//!
//! One pass (`Popups::frame`) runs after the bars have drawn: read the popup
//! declarations every scene reports, open the ones that turned open, close
//! the ones that went away, draw each popup surface, follow anchors that
//! moved, and keep the outside-press watch on only while a popup without
//! focus is open.

mod focus;
mod frame;
mod input;
mod place;
mod tooltip;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use rsb_core::css::{MediaEnv, Stylesheet};
use rsb_core::element::{Element, ElementKind, HandlerId, InstanceId, NodeKey, WidgetTree};
use rsb_core::geom::{RectF, RectPx};
use rsb_core::hit::{ElementCall, ElementEvent};
use rsb_core::msg::ScriptIn;
use rsb_core::popup::{CLOSE_ANIMATION_CAP, CloseReason, FlatEdges, Placement, PopupDecl, PopupProps, Side};
use rsb_core::scene::{BarGeometry, PopupSurfaceSpec, Scene, SceneDiagnostic, SceneEnv};
use rsb_core::style::Cursor;
use rsb_render::Renderer;
use rsb_win::{BarId, MonitorInfo, PopupId, SurfaceRole};
use serde_json::json;

use crate::app::BarRt;
use crate::gesture::Gestures;
use crate::icons::BundledIcons;
use crate::service_images::APPS_PREFIX;
use crate::stats::Stats;
use crate::winlayer::Win;

/// Longest an overlay stays up after its widget asked for the lock curtain
/// (class `locking`) when Windows never reports the lock.
pub const LOCK_CURTAIN_LIMIT: Duration = Duration::from_millis(1000);

/// How long a popup that a widget put up on its own stays shut after the user
/// closed it; also how long such a popup keeps the outside-press watch on.
pub const USER_CLOSE_HOLD: Duration = Duration::from_secs(10);
pub const UNASKED_WATCH: Duration = Duration::from_secs(60);

/// A draw step shorter than this is not remembered as a cause of a slow turn.
pub const SLOW_STEP: Duration = Duration::from_millis(250);

/// The slowest draw step since the last report: what a slow loop turn was
/// busy with (a widget reference or a bar name, never a path).
#[derive(Clone, Debug)]
pub struct SlowStep {
    pub label: String,
    pub took: Duration,
    pub at: Instant,
}

/// Text a simulated paste puts in the focused input: a test run never reads
/// the user's clipboard.
pub const FAKE_CLIPBOARD: &str = "pasted";

/// The flags and run kind that change how the manager touches the system.
#[derive(Clone, Copy, Debug, Default)]
pub struct PopupOptions {
    /// `--no-hook`: never install the outside-press hook.
    pub no_hook: bool,
    /// `--no-activate`: never take real keyboard focus; focus is simulated.
    pub no_activate: bool,
    /// `--test-offscreen`: windows are moved beyond the monitors by the window layer.
    pub offscreen: bool,
    /// A run driven by a test flag: the clipboard is a stand-in.
    pub gated: bool,
}

/// What the watchdog thread and the manager share about overlays.
#[derive(Debug, Default)]
pub struct OverlayWatch {
    /// An overlay window is up.
    pub shown: AtomicBool,
    /// How many times the watchdog hid the overlays because the UI thread stalled.
    pub hidden: AtomicU64,
}

/// Everything the manager borrows from the app for one call.
pub struct Env<'a> {
    pub(crate) ui: &'a mut dyn Win,
    pub(crate) renderer: &'a mut Renderer,
    pub(crate) bars: &'a mut Vec<BarRt>,
    pub(crate) icons: &'a mut BundledIcons,
    pub(crate) sheet: &'a Arc<Stylesheet>,
    pub(crate) dark: bool,
    pub(crate) image_sizes: &'a HashMap<Arc<str>, (u32, u32)>,
    pub(crate) now: Instant,
}

/// What one `frame` pass tells the app.
#[derive(Debug, Default)]
pub struct FrameResult {
    /// The earliest time a popup surface wants to draw again.
    pub next: Option<Instant>,
    /// Bars whose anchor state changed: draw them again in this turn.
    pub bars: Vec<BarId>,
}

/// The scene a popup hangs from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Anchor {
    Bar(BarId),
    Popup(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Opening,
    Open,
    Closing,
}

/// One declaration, with where its surface is on the screen.
struct DeclRec {
    anchor: Anchor,
    bar: BarId,
    /// The reporting surface's top left, logical screen px.
    origin: (i32, i32),
    device: String,
    name: String,
    d: PopupDecl,
}

/// One popup surface the manager has made.
struct Managed {
    id: u64,
    key: NodeKey,
    instance: InstanceId,
    role: SurfaceRole,
    name: String,
    props: PopupProps,
    parent: Option<u64>,
    bar: BarId,
    anchor: Anchor,
    scene: Scene,
    win: PopupId,
    dim: Option<PopupId>,
    monitor: MonitorInfo,
    scale: f32,
    /// The window, logical screen px.
    rect: RectPx,
    side: Side,
    stage: Stage,
    open_seq: u64,
    gesture: u64,
    keyboard: bool,
    opened_at: Instant,
    presented: bool,
    frames: u32,
    closing_at: Option<Instant>,
    closing_frames: u32,
    settled_once: bool,
    settle_ms: Option<u64>,
    /// The window may take keyboard focus (its content has something focusable).
    focusable: bool,
    /// This popup holds keyboard focus (real, or simulated under `--no-activate`).
    holds: bool,
    ime: bool,
    ime_caret: Option<RectPx>,
    cursor: Cursor,
    region: Vec<(f32, f32)>,
    next_frame: Option<Instant>,
    need_full: bool,
    anchor_local: RectF,
    anchor_screen: RectF,
    flat_screen: FlatEdges,
    tree: Arc<WidgetTree>,
    path: Arc<[u32]>,
    uses_apps: bool,
    locking_since: Option<Instant>,
    blur: bool,
    /// Surface sizes (device px) this popup drew at, so the renderer can give their buffers back when it closes.
    sizes: Vec<(u32, u32)>,
}

pub struct Popups {
    o: PopupOptions,
    stats: Arc<Stats>,
    script_tx: Sender<ScriptIn>,
    watch: Arc<OverlayWatch>,
    pub gestures: Gestures,
    managed: Vec<Managed>,
    next_id: u64,
    /// The gesture of the press that last closed each popup as `Outside`.
    outside_by: HashMap<NodeKey, u64>,
    /// When the user closed a popup of a widget that has no `overlay` (and
    /// whether the refusal was logged): the widget cannot put it back by itself for `USER_CLOSE_HOLD`.
    user_closed: HashMap<NodeKey, (Instant, bool)>,
    slow_step: Option<SlowStep>,
    /// The gesture of the press now being handled, so a close it causes can say so.
    close_gesture: u64,
    /// The newest pointer-down gesture, for the click that follows.
    press: Option<u64>,
    key_ctx: Option<input::KeyCtx>,
    watch_wanted: bool,
    tips: tooltip::Tips,
    reads: HashMap<u64, input::PasteRead>,
    next_token: u64,
    picked_sent: HashSet<(InstanceId, HandlerId)>,
    hidden_seen: u64,
    apps_used: bool,
    apps_released: bool,
    monitors: Vec<MonitorInfo>,
    diags: Vec<SceneDiagnostic>,
    dirty_bars: Vec<BarId>,
}

impl Popups {
    /// The popup surfaces that exist: the instance each belongs to, its name and the scene that draws it.
    pub fn inspect_views(&self) -> Vec<(InstanceId, &str, &Scene)> {
        self.managed.iter().map(|m| (m.instance, m.name.as_str(), &m.scene)).collect()
    }
}

/// Which popup element in the tree uses a service image under `prefix`.
fn uses_prefix(el: &Element, prefix: &str) -> bool {
    el.image.as_ref().is_some_and(|i| i.path.starts_with(prefix)) || el.children.iter().any(|c| uses_prefix(c, prefix))
}

/// The popup element at `path` below the widget root.
fn element_at<'a>(tree: &'a WidgetTree, path: &[u32]) -> Option<&'a Element> {
    let mut el = tree.root.as_ref()?;
    for &i in path {
        el = el.children.get(i as usize)?;
    }
    Some(el)
}

/// Whether a surface's content holds something that can take keyboard focus.
fn has_focusable(el: &Element) -> bool {
    matches!(el.kind, ElementKind::Input | ElementKind::Slider | ElementKind::MenuItem)
        || el.interaction.focusable
        || el.children.iter().any(has_focusable)
}

/// The scene of an anchor: a bar's, or a popup's own (a menu opened inside a panel).
fn scene_of<'a>(managed: &'a mut [Managed], bars: &'a mut [BarRt], anchor: Anchor) -> Option<&'a mut Scene> {
    match anchor {
        Anchor::Bar(id) => bars.iter_mut().find(|b| b.id == id).map(|b| &mut b.scene),
        Anchor::Popup(id) => managed.iter_mut().find(|m| m.id == id).map(|m| &mut m.scene),
    }
}

impl Popups {
    pub fn new(o: PopupOptions, stats: Arc<Stats>, script_tx: Sender<ScriptIn>, watch: Arc<OverlayWatch>) -> Popups {
        Popups {
            o,
            stats,
            script_tx,
            watch,
            gestures: Gestures::default(),
            managed: Vec::new(),
            next_id: 1,
            outside_by: HashMap::new(),
            user_closed: HashMap::new(),
            slow_step: None,
            close_gesture: 0,
            press: None,
            key_ctx: None,
            watch_wanted: false,
            tips: tooltip::Tips::default(),
            reads: HashMap::new(),
            next_token: 1,
            picked_sent: HashSet::new(),
            hidden_seen: 0,
            apps_used: false,
            apps_released: false,
            monitors: Vec::new(),
            diags: Vec::new(),
            dirty_bars: Vec::new(),
        }
    }

    // ------------------------------------------------------------ app hooks

    /// The slowest popup draw since the last call.
    pub fn take_slow_step(&mut self) -> Option<SlowStep> {
        self.slow_step.take()
    }

    /// Any popup, menu or overlay surface exists (tooltips excluded).
    pub fn any_open(&self) -> bool {
        self.managed.iter().any(|m| m.role != SurfaceRole::Tooltip)
    }

    /// True once, after the last popup that used launcher icons went away.
    pub fn take_apps_released(&mut self) -> bool {
        std::mem::take(&mut self.apps_released)
    }

    pub fn take_diagnostics(&mut self) -> Vec<SceneDiagnostic> {
        std::mem::take(&mut self.diags)
    }

    pub fn set_stylesheet(&mut self, sheet: &Arc<Stylesheet>) {
        for m in &mut self.managed {
            m.scene.set_stylesheet(sheet.clone());
        }
    }

    pub fn set_env(&mut self, dark: bool, icon_font: rsb_core::icons::IconFont) {
        for m in &mut self.managed {
            m.scene.set_env(SceneEnv { media: MediaEnv { dark }, icon_font, fullscreen: false });
        }
    }

    pub fn fonts_changed(&mut self) {
        for m in &mut self.managed {
            m.scene.fonts_changed();
        }
    }

    pub fn image_size(&mut self, key: &str, width: u32, height: u32) {
        for m in &mut self.managed {
            m.scene.set_image_size(key, width, height);
        }
    }

    pub fn image_failed(&mut self, key: &str) {
        for m in &mut self.managed {
            m.scene.image_failed(key);
        }
    }

    /// The popup window a simulated key, text or pointer event goes to.
    pub fn popup_window(&self, name: &str) -> Option<PopupId> {
        self.managed.iter().find(|m| m.name == name && m.stage != Stage::Closing).map(|m| m.win)
    }

    /// The dim window of the overlay with this name.
    pub fn dim_window(&self, name: &str) -> Option<PopupId> {
        self.managed.iter().find(|m| m.name == name && m.stage != Stage::Closing).and_then(|m| m.dim)
    }

    // ------------------------------------------------------------ geometry helpers

    fn idx(&self, id: u64) -> Option<usize> {
        self.managed.iter().position(|m| m.id == id)
    }

    /// How far a bar window is moved from where its monitor puts it in an
    /// offscreen run, so anchors are placed against the real monitors.
    fn offscreen_shift(&self, device: &str) -> i32 {
        if !self.o.offscreen {
            return 0;
        }
        let right = self.monitors.iter().map(|m| m.rect.x1).max().unwrap_or(0);
        let left = self.monitors.iter().find(|m| m.device.eq_ignore_ascii_case(device)).map_or(0, |m| m.rect.x0);
        right + 200 - left
    }

    fn bar_origin(&self, env: &Env<'_>, bar: &BarRt) -> Option<(i32, i32)> {
        let r = env.ui.window_rect(bar.id)?;
        Some((r.x0 - self.offscreen_shift(&bar.plan.monitor.device), r.y0))
    }

    fn monitor_of(&self, device: &str) -> Option<MonitorInfo> {
        self.monitors.iter().find(|m| m.device.eq_ignore_ascii_case(device)).cloned()
    }

    // ------------------------------------------------------------ declarations

    /// Every popup declaration the scenes report, with where its surface is.
    fn gather(&self, env: &Env<'_>) -> Vec<DeclRec> {
        let mut out = Vec::new();
        for bar in env.bars.iter() {
            let Some(origin) = self.bar_origin(env, bar) else { continue };
            push_decls(&mut out, bar.scene.popups(), Anchor::Bar(bar.id), bar.id, origin, &bar.plan.monitor.device);
        }
        for m in &self.managed {
            if m.stage == Stage::Closing || m.role == SurfaceRole::Tooltip {
                continue;
            }
            let origin = (m.rect.x0, m.rect.y0);
            push_decls(&mut out, m.scene.popups(), Anchor::Popup(m.id), m.bar, origin, &m.monitor.device);
        }
        out
    }

    fn send(&self, msg: ScriptIn) {
        let _ = self.script_tx.send(msg);
    }

    /// Tells the script the UI closed the popup (it writes `open = false`, then runs `on_close`).
    fn tell_closed(&self, instance: InstanceId, handler: Option<HandlerId>, reason: CloseReason) {
        let Some(handler) = handler else { return };
        if self.picked_sent.contains(&(instance, handler)) {
            return;
        }
        self.send(ScriptIn::Element(ElementCall {
            instance,
            handler,
            event: ElementEvent::PopupClosed(reason),
            gesture: 0,
        }));
    }

    /// One `popup` stats line. The line's own `event` key says `popup`, so the stage is `state`.
    fn popup_event(&self, name: &str, event: &str, extra: serde_json::Value) {
        if !self.stats.enabled() {
            return;
        }
        let mut line = json!({"name": name, "state": event});
        if let (Some(map), serde_json::Value::Object(more)) = (line.as_object_mut(), extra) {
            map.extend(more);
        }
        self.stats.emit("popup", line);
    }

    // ------------------------------------------------------------ the pass

    /// One pass: declarations, opens and closes, drawing, following anchors.
    pub fn frame(&mut self, env: &mut Env<'_>) -> FrameResult {
        // Only a test run that shifts its windows needs the monitors on every pass; a popup being made
        // asks for them itself (`create`, the tooltip, `rsb popup`).
        if self.o.offscreen {
            self.monitors = env.ui.monitors();
        }
        self.tick(env);
        let decls = self.gather(env);
        self.sync(env, &decls);
        if !self.managed.is_empty() {
            let ids: Vec<u64> = self.managed.iter().map(|m| m.id).collect();
            for id in ids {
                self.frame_one(id, env);
            }
            // Popups declared by popups (a menu inside a panel) are known only after the panel drew. The scan
            // is skipped when no open panel declares one and none is open that a panel declared: a panel that
            // just dropped its only menu declares nothing, yet that menu has to close in this pass.
            if self.managed.iter().any(|m| {
                m.stage != Stage::Closing
                    && m.role != SurfaceRole::Tooltip
                    && (m.scene.declares_popups() || matches!(m.anchor, Anchor::Popup(_)))
            }) {
                let decls = self.gather(env);
                self.sync(env, &decls);
                // A popup this scan closed has not drawn since; without a frame now it would hold its last
                // open frame on screen until the closing cap reaps it.
                let closed: Vec<u64> = self
                    .managed
                    .iter()
                    .filter(|m| m.stage == Stage::Closing && m.closing_frames == 0)
                    .map(|m| m.id)
                    .collect();
                for id in closed {
                    self.frame_one(id, env);
                }
            }
            self.follow_anchors(env);
        }
        self.reap(env);
        self.update_watch(env);
        self.watch.shown.store(
            self.managed.iter().any(|m| m.role == SurfaceRole::Overlay && m.stage != Stage::Closing),
            Ordering::Release,
        );
        self.check_apps();
        self.key_ctx = None;
        let mut next = self.managed.iter().filter_map(|m| m.next_frame).min();
        for m in self.managed.iter().filter(|m| m.stage == Stage::Closing) {
            if let Some(at) = m.closing_at {
                next = min_opt(next, Some(at + CLOSE_ANIMATION_CAP));
            }
        }
        next = min_opt(next, self.tips.deadline());
        for m in &self.managed {
            // A popup nobody asked for stops holding the mouse watch after a minute.
            if m.stage != Stage::Closing
                && !m.holds
                && m.props.dismiss.outside
                && !keeps_watch(m.gesture, m.props.trusted, Duration::MAX)
            {
                let end = m.opened_at + UNASKED_WATCH;
                if end > env.now {
                    next = min_opt(next, Some(end));
                }
            }
            if let Some(since) = m.locking_since {
                next = min_opt(next, Some(since + LOCK_CURTAIN_LIMIT));
            }
        }
        FrameResult { next, bars: std::mem::take(&mut self.dirty_bars) }
    }

    /// Timers that are not a frame: the tooltip delay, the lock curtain's
    /// limit, and overlays the watchdog hid.
    fn tick(&mut self, env: &mut Env<'_>) {
        self.tip_tick(env);
        let hidden = self.watch.hidden.load(Ordering::Acquire);
        if hidden != self.hidden_seen {
            self.hidden_seen = hidden;
            let ids: Vec<u64> = self.managed.iter().filter(|m| m.role == SurfaceRole::Overlay).map(|m| m.id).collect();
            // The windows are already hidden; the script is told so the overlay can be opened again.
            for id in ids {
                self.close_inner(id, CloseReason::Command, env, true);
            }
        }
        let late: Vec<u64> = self
            .managed
            .iter()
            .filter(|m| m.locking_since.is_some_and(|t| env.now.saturating_duration_since(t) >= LOCK_CURTAIN_LIMIT))
            .map(|m| m.id)
            .collect();
        for id in late {
            log::warn!("the lock did not follow the curtain; the overlay is closed");
            self.close_inner(id, CloseReason::Command, env, true);
        }
    }

    /// Reads the declarations: closes what vanished or was closed by the
    /// script, keeps the rest in step with their anchors and trees, opens
    /// what turned open.
    fn sync(&mut self, env: &mut Env<'_>, decls: &[DeclRec]) {
        let ids: Vec<u64> = self
            .managed
            .iter()
            .filter(|m| m.role != SurfaceRole::Tooltip && m.stage != Stage::Closing)
            .map(|m| m.id)
            .collect();
        for id in ids {
            let Some(i) = self.idx(id) else { continue };
            if self.managed[i].stage == Stage::Closing {
                continue;
            }
            let key = self.managed[i].key;
            match decls.iter().find(|r| r.d.key == key) {
                None => self.close(id, CloseReason::AnchorGone, env),
                Some(r) if !r.d.open => self.close(id, CloseReason::Command, env),
                Some(r) => self.refresh(i, r, env),
            }
        }
        for r in decls.iter().filter(|r| r.d.kind != ElementKind::Tooltip && r.d.open) {
            match self.managed.iter().position(|m| m.key == r.d.key) {
                None => self.try_open(r, env),
                Some(i) if self.managed[i].stage == Stage::Closing => {
                    if r.d.props.open_seq > self.managed[i].open_seq {
                        if self.held_shut(r, r.d.props.gesture, env.now) {
                            self.refuse(r, CloseReason::Refused, env);
                        } else {
                            self.reopen(i, r, env);
                        }
                    }
                }
                Some(_) => {}
            }
        }
    }

    /// Keeps an open popup's props, anchor and content current.
    fn refresh(&mut self, i: usize, r: &DeclRec, env: &mut Env<'_>) {
        let m = &mut self.managed[i];
        m.props = r.d.props.clone();
        m.anchor_local = r.d.anchor_rect;
        m.anchor_screen = to_screen(&r.d.anchor_rect, r.origin);
        m.flat_screen = flat_to_screen(&r.d.anchor_flat, r.origin);
        if !Arc::ptr_eq(&m.tree, &r.d.tree) || m.path != r.d.path {
            m.tree = r.d.tree.clone();
            m.path = r.d.path.clone();
            m.scene.set_popup_tree(r.d.tree.clone(), r.d.path.clone());
            m.retree(env.ui, env.now);
            let uses = element_at(&m.tree, &m.path).is_some_and(|e| uses_prefix(e, APPS_PREFIX));
            m.uses_apps = uses;
        }
    }

    // ------------------------------------------------------------ opening

    fn try_open(&mut self, r: &DeclRec, env: &mut Env<'_>) {
        let d = &r.d;
        let overlay = d.props.placement == Placement::Screen;
        let mut gesture = d.props.gesture;
        let mut keyboard = self.gestures.kind(gesture).is_some_and(|k| k != crate::gesture::GestureKind::Pointer);
        // A submenu the keyboard just opened has no script gesture of its own: the key is it.
        if gesture == 0
            && d.kind == ElementKind::Menu
            && let Anchor::Popup(parent) = r.anchor
            && let Some(k) = self.key_ctx.filter(|k| k.popup == parent)
        {
            gesture = k.gesture;
            keyboard = true;
        }
        if overlay && gesture == 0 {
            log::warn!("an overlay was refused: it may only open from a click, a key or rsb popup");
            self.refuse(r, CloseReason::Refused, env);
            return;
        }
        if self.held_shut(r, gesture, env.now) {
            self.refuse(r, CloseReason::Refused, env);
            return;
        }
        if gesture != 0 && self.outside_by.get(&d.key) == Some(&gesture) {
            // The press that closed it as Outside also clicked a button that toggles it.
            self.refuse(r, CloseReason::Outside, env);
            return;
        }
        self.hide_tip(env);
        self.replace_group(r, env);
        match self.create(r, gesture, keyboard, env) {
            Ok(_) => {}
            Err(e) => {
                log::warn!("a popup could not be opened: {e}");
                self.refuse(r, CloseReason::Refused, env);
            }
        }
    }

    /// A popup of a widget without `overlay` that the user just closed does
    /// not come back on a timer or a service update: only a click or a key
    /// opens it again before `USER_CLOSE_HOLD` is over.
    fn held_shut(&mut self, r: &DeclRec, gesture: u64, now: Instant) -> bool {
        if gesture != 0 || r.d.props.trusted || r.d.kind != ElementKind::Popup {
            return false;
        }
        let Some((at, logged)) = self.user_closed.get_mut(&r.d.key) else { return false };
        if now.saturating_duration_since(*at) >= USER_CLOSE_HOLD {
            self.user_closed.remove(&r.d.key);
            return false;
        }
        if !*logged {
            *logged = true;
            log::warn!("a widget opened a popup the user had just closed; it stays closed until a click or a key");
        }
        true
    }

    /// The popup never opens: its anchor's scene forgets this `open_seq` and the script hears why.
    fn refuse(&mut self, r: &DeclRec, reason: CloseReason, env: &mut Env<'_>) {
        if let Some(s) = scene_of(&mut self.managed, env.bars, r.anchor) {
            s.dismiss_popup(r.d.key, r.d.props.open_seq);
        }
        if let Anchor::Bar(b) = r.anchor {
            self.dirty_bars.push(b);
        }
        self.tell_closed(r.d.instance, r.d.props.on_close, reason);
        self.popup_event(
            &r.name,
            if reason == CloseReason::Refused { "refused" } else { "close" },
            json!({"reason": reason.name()}),
        );
    }

    /// Opening a popup closes the other open popup of its group on its monitor, never its own chain.
    fn replace_group(&mut self, r: &DeclRec, env: &mut Env<'_>) {
        let mut ancestors = HashSet::new();
        let mut at = match r.anchor {
            Anchor::Popup(p) => Some(p),
            Anchor::Bar(_) => None,
        };
        while let Some(p) = at {
            ancestors.insert(p);
            at = self.managed.iter().find(|m| m.id == p).and_then(|m| m.parent);
        }
        let victims: Vec<u64> = self
            .managed
            .iter()
            .filter(|m| {
                m.role != SurfaceRole::Tooltip
                    && m.stage != Stage::Closing
                    && m.props.group == r.d.props.group
                    && m.monitor.device.eq_ignore_ascii_case(&r.device)
                    && !ancestors.contains(&m.id)
            })
            .map(|m| m.id)
            .collect();
        for id in victims {
            self.close(id, CloseReason::Replaced, env);
        }
    }

    /// Makes the scene and the window(s), draws the closed frame, then the open one.
    fn create(&mut self, r: &DeclRec, gesture: u64, keyboard: bool, env: &mut Env<'_>) -> Result<u64, String> {
        let d = &r.d;
        self.monitors = env.ui.monitors();
        let bar_ix = env.bars.iter().position(|b| b.id == r.bar).ok_or("its bar is gone")?;
        let (bar_spec, slot, scale) = {
            let b = &env.bars[bar_ix];
            let slot = b.insts.all().find(|p| p.spec.instance == d.instance).map(|p| p.slot.clone());
            (b.plan.spec.clone(), slot.ok_or("its widget has no slot")?, b.scale)
        };
        let mon = self.monitor_of(&r.device).ok_or("its monitor is gone")?;
        let overlay = d.props.placement == Placement::Screen && d.kind == ElementKind::Popup;
        let role = match d.kind {
            ElementKind::Menu => SurfaceRole::Menu,
            ElementKind::Tooltip => SurfaceRole::Tooltip,
            _ if overlay => SurfaceRole::Overlay,
            _ => SurfaceRole::Popup,
        };
        let basis = if overlay { mon.rect } else { mon.work };
        let geometry =
            BarGeometry { width_px: basis.width().max(1) as u32, height_px: basis.height().max(1) as u32, scale };
        let side = place::side_guess(d.props.placement, &to_screen(&d.anchor_rect, r.origin), &mon.rect);
        let spec = PopupSurfaceSpec { bar: bar_spec, slot, kind: d.kind, side, blur: false };
        let env_scene =
            SceneEnv { media: MediaEnv { dark: env.dark }, icon_font: env.renderer.icon_font(), fullscreen: false };
        let mut scene = Scene::new_popup(spec, geometry, env_scene);
        scene.set_stylesheet(env.sheet.clone());
        for (key, (w, h)) in env.image_sizes {
            scene.set_image_size(key, *w, *h);
        }
        scene.set_popup_tree(d.tree.clone(), d.path.clone());

        let anchor_screen = to_screen(&d.anchor_rect, r.origin);
        let flat_screen = flat_to_screen(&d.anchor_flat, r.origin);
        let content = element_at(&d.tree, &d.path);
        let focusable = focus::window_focusable(role, &d.props, content);
        let uses_apps = content.is_some_and(|e| uses_prefix(e, APPS_PREFIX));
        let locking = content.is_some_and(|e| e.classes.iter().any(|c| &**c == "locking"));

        let mut dim = None;
        let mut blur = false;
        if overlay {
            let spec = rsb_win::DimSpec {
                rect: mon.rect,
                monitor: mon.device.clone(),
                owner: Some(r.bar),
                backdrop: focus::backdrop_possible(),
            };
            let id = env.ui.create_dim(&spec).map_err(|e| e.to_string())?;
            env.ui.name_popup(id, &format!("dim:{}", r.name));
            blur = env.ui.dim_backdrop_active(id);
            dim = Some(id);
        }
        let layout = place::plan(
            &mut scene,
            env.renderer,
            &place::Wanted {
                props: &d.props,
                anchor: anchor_screen,
                flat: flat_screen,
                monitor: &mon,
                scale,
                blur,
                trusted: d.props.trusted,
            },
        );
        let win = match env.ui.create_popup(&rsb_win::PopupWindowSpec {
            role,
            rect: layout.rect,
            monitor: mon.device.clone(),
            owner: Some(r.bar),
            focusable,
        }) {
            Ok(w) => w,
            Err(e) => {
                if let Some(dim) = dim {
                    env.ui.destroy_popup(dim);
                }
                return Err(e.to_string());
            }
        };
        let label = if role == SurfaceRole::Tooltip { "tooltip" } else { r.name.as_str() };
        env.ui.name_popup(win, &format!("popup:{label}"));
        if let Some(g) = env.ui.popup_geometry(win) {
            scene.set_geometry(g);
        }
        let id = self.next_id;
        self.next_id += 1;
        let now = env.now;
        let parent = match r.anchor {
            Anchor::Popup(p) => Some(p),
            Anchor::Bar(_) => None,
        };
        self.managed.push(Managed {
            id,
            key: d.key,
            instance: d.instance,
            role,
            name: if role == SurfaceRole::Tooltip { "tooltip".to_string() } else { r.name.clone() },
            props: d.props.clone(),
            parent,
            bar: r.bar,
            anchor: r.anchor,
            scene,
            win,
            dim,
            monitor: mon.clone(),
            scale,
            rect: layout.rect,
            side: layout.side,
            stage: Stage::Opening,
            open_seq: d.props.open_seq,
            gesture,
            keyboard,
            opened_at: now,
            presented: false,
            frames: 0,
            closing_at: None,
            closing_frames: 0,
            settled_once: false,
            settle_ms: None,
            focusable,
            holds: false,
            ime: false,
            ime_caret: None,
            cursor: Cursor::Default,
            region: Vec::new(),
            next_frame: None,
            need_full: true,
            anchor_local: d.anchor_rect,
            anchor_screen,
            flat_screen,
            tree: d.tree.clone(),
            path: d.path.clone(),
            uses_apps,
            locking_since: locking.then_some(now),
            blur,
            sizes: Vec::new(),
        });
        let kind_name = match role {
            SurfaceRole::Popup => "popup",
            SurfaceRole::Menu => "menu",
            SurfaceRole::Tooltip => "tooltip",
            SurfaceRole::Overlay => "overlay",
        };
        let shown_name = self.managed.last().map_or_else(|| r.name.clone(), |m| m.name.clone());
        self.popup_event(&shown_name, "open", json!({"kind": kind_name, "gesture": gesture != 0}));
        if uses_apps {
            self.apps_used = true;
        }
        // The first frame is the closed style (nothing visible, no flash); then both surfaces start the
        // opening transitions from it on one clock.
        self.frame_one(id, env);
        if let Some(i) = self.idx(id) {
            self.managed[i].scene.set_open(true, now);
        }
        if let Some(s) = scene_of(&mut self.managed, env.bars, r.anchor) {
            s.set_anchor_open(d.key, true, now);
        }
        if let Anchor::Bar(b) = r.anchor {
            self.dirty_bars.push(b);
        }
        self.frame_one(id, env);
        self.take_focus_on_open(id, env);
        Ok(id)
    }

    /// A popup told to open again while it was closing keeps its window and its state.
    fn reopen(&mut self, i: usize, r: &DeclRec, env: &mut Env<'_>) {
        let now = env.now;
        let id = self.managed[i].id;
        {
            let m = &mut self.managed[i];
            m.scene.set_open(true, now);
            m.stage = Stage::Opening;
            m.closing_at = None;
            m.closing_frames = 0;
            m.open_seq = r.d.props.open_seq;
            m.gesture = r.d.props.gesture;
            m.opened_at = now;
            m.settled_once = false;
        }
        self.refresh(i, r, env);
        if let Some(s) = scene_of(&mut self.managed, env.bars, r.anchor) {
            s.set_anchor_open(r.d.key, true, now);
        }
        if let Anchor::Bar(b) = r.anchor {
            self.dirty_bars.push(b);
        }
        self.popup_event(&r.name, "open", json!({"reopened": true}));
        self.frame_one(id, env);
    }

    // ------------------------------------------------------------ closing

    /// Starts closing a popup and everything opened from it. The windows go
    /// when the closing transitions have finished.
    pub(super) fn close(&mut self, id: u64, reason: CloseReason, env: &mut Env<'_>) {
        self.close_inner(id, reason, env, reason != CloseReason::Command);
    }

    /// `tell`: the script hears of it through `on_close`. It does not when the
    /// script closed the popup itself (`Command`); it does when the UI closes
    /// one for a reason of its own that uses the same name (the lock curtain).
    fn close_inner(&mut self, id: u64, reason: CloseReason, env: &mut Env<'_>, tell: bool) {
        let kids: Vec<u64> =
            self.managed.iter().filter(|m| m.parent == Some(id) && m.stage != Stage::Closing).map(|m| m.id).collect();
        for k in kids {
            self.close_inner(k, reason, env, tell);
        }
        let Some(i) = self.idx(id) else { return };
        if self.managed[i].stage == Stage::Closing {
            return;
        }
        let now = env.now;
        let (key, seq, anchor, instance, on_close, holds, parent, name, role, win) = {
            if matches!(reason, CloseReason::Outside | CloseReason::Escape | CloseReason::Deactivated)
                && !self.managed[i].props.trusted
            {
                self.user_closed.insert(self.managed[i].key, (now, false));
            }
            let m = &mut self.managed[i];
            m.stage = Stage::Closing;
            m.closing_at = Some(now);
            m.closing_frames = 0;
            m.locking_since = None;
            m.scene.set_open(false, now);
            let held = m.holds;
            m.holds = false;
            m.scene.set_window_focus(false, now);
            (m.key, m.open_seq, m.anchor, m.instance, m.props.on_close, held, m.parent, m.name.clone(), m.role, m.win)
        };
        if let Some(s) = scene_of(&mut self.managed, env.bars, anchor) {
            s.set_anchor_open(key, false, now);
            s.dismiss_popup(key, seq);
        }
        if let Anchor::Bar(b) = anchor {
            self.dirty_bars.push(b);
        }
        if reason == CloseReason::Outside && self.close_gesture != 0 {
            self.outside_by.insert(key, self.close_gesture);
        }
        if role != SurfaceRole::Tooltip && tell {
            self.tell_closed(instance, on_close, reason);
        }
        self.popup_event(&name, "close", json!({"reason": reason.name()}));
        if holds {
            env.ui.set_ime_enabled(win, false);
            if let Some(p) = parent {
                self.give_focus(p, env);
            }
        }
        if let Some(i) = self.idx(id) {
            self.managed[i].ime = false;
        }
    }

    /// Closes the whole chain that holds this popup (the root and everything below it).
    pub(super) fn close_chain(&mut self, id: u64, reason: CloseReason, env: &mut Env<'_>) {
        let mut root = id;
        while let Some(p) = self.idx(root).and_then(|i| self.managed[i].parent) {
            root = p;
        }
        self.close(root, reason, env);
    }

    /// Every popup of this bar, or of every bar, closes with this reason.
    fn close_where(&mut self, env: &mut Env<'_>, reason: CloseReason, pick: impl Fn(&Managed) -> bool) {
        let ids: Vec<u64> = self
            .managed
            .iter()
            .filter(|m| m.parent.is_none() && m.stage != Stage::Closing && pick(m))
            .map(|m| m.id)
            .collect();
        for id in ids {
            self.close(id, reason, env);
        }
    }

    pub fn close_all(&mut self, env: &mut Env<'_>, reason: CloseReason) {
        self.hide_tip(env);
        self.close_where(env, reason, |_| true);
    }

    pub fn close_bar(&mut self, env: &mut Env<'_>, bar: BarId, reason: CloseReason) {
        self.hide_tip(env);
        self.close_where(env, reason, |m| m.bar == bar);
    }

    /// An overlay that put up the lock curtain and is waiting for Windows to lock: the lock failed.
    pub fn close_overlays(&mut self, env: &mut Env<'_>, reason: CloseReason) {
        let ids: Vec<u64> = self
            .managed
            .iter()
            .filter(|m| m.role == SurfaceRole::Overlay && m.locking_since.is_some() && m.stage != Stage::Closing)
            .map(|m| m.id)
            .collect();
        for id in ids {
            self.close_inner(id, reason, env, true);
        }
    }

    pub fn close_monitor(&mut self, env: &mut Env<'_>, device: &str, reason: CloseReason) {
        self.hide_tip(env);
        self.close_where(env, reason, |m| m.monitor.device.eq_ignore_ascii_case(device));
    }

    /// A popup whose bar was destroyed (config reload, monitor gone) goes at once.
    pub fn drop_orphans(&mut self, env: &mut Env<'_>) {
        let gone: Vec<u64> =
            self.managed.iter().filter(|m| !env.bars.iter().any(|b| b.id == m.bar)).map(|m| m.id).collect();
        for id in gone {
            self.discard(id, env);
        }
    }

    /// Destroys the windows now, with no closing animation.
    fn discard(&mut self, id: u64, env: &mut Env<'_>) {
        let Some(i) = self.idx(id) else { return };
        let m = self.managed.remove(i);
        if m.stage != Stage::Closing {
            self.popup_event(&m.name, "close", json!({"reason": CloseReason::DisplayChange.name()}));
            // The widget keeps its id and its open state when its bar is made again: it must hear
            // that the popup is shut, or it opens it again on the new bar by itself.
            if m.role != SurfaceRole::Tooltip {
                self.tell_closed(m.instance, m.props.on_close, CloseReason::DisplayChange);
            }
        }
        env.ui.destroy_popup(m.win);
        if let Some(d) = m.dim {
            env.ui.destroy_popup(d);
        }
        self.release_buffers(&m, env);
        if self.tips.shown == Some(id) {
            self.tips.shown = None;
        }
        self.popup_event(&m.name, "closed", json!({}));
    }

    /// The renderer keeps a pixel buffer for every surface size it drew; a popup that is gone gives its
    /// sizes back, unless another open popup draws at the same size.
    fn release_buffers(&self, gone: &Managed, env: &mut Env<'_>) {
        for &(w, h) in &gone.sizes {
            if !self.managed.iter().any(|o| o.sizes.contains(&(w, h))) {
                env.renderer.release_surface(w, h);
            }
        }
    }

    /// Destroys the windows of popups whose closing transitions are over.
    fn reap(&mut self, env: &mut Env<'_>) {
        let now = env.now;
        let done: Vec<u64> = self
            .managed
            .iter()
            .filter(|m| {
                m.stage == Stage::Closing
                    && (m.closing_frames >= 1 && m.scene.is_settled()
                        || m.closing_at.is_some_and(|t| now.saturating_duration_since(t) >= CLOSE_ANIMATION_CAP))
            })
            .map(|m| m.id)
            .collect();
        for id in done {
            let Some(i) = self.idx(id) else { continue };
            let m = self.managed.remove(i);
            let took = m.closing_at.map(|t| now.saturating_duration_since(t).as_millis() as u64);
            // An overlay's dim is painted once and only its alpha changes after.
            let dim_paints = m.dim.and_then(|d| env.ui.dim_paints(d));
            env.ui.destroy_popup(m.win);
            if let Some(d) = m.dim {
                env.ui.destroy_popup(d);
            }
            self.release_buffers(&m, env);
            if self.tips.shown == Some(id) {
                self.tips.shown = None;
                self.tips.hid_at = Some(now);
            }
            // What the renderer still holds once the popup is gone (bytes), for the memory checks.
            let held = self.stats.enabled().then(|| {
                let r = env.renderer.stats();
                json!({"glyph_bytes": r.glyph_bytes, "layouts": r.layouts_cached, "image_bytes": r.image_bytes, "svg_raster_bytes": r.svg_raster_bytes,
                       "scratch_bytes": r.scratch_bytes, "shadow_bytes": r.shadow_bytes})
            });
            self.popup_event(
                &m.name,
                "closed",
                json!({"settle_ms": m.settle_ms, "closing_ms": took, "dim_paints": dim_paints, "held": held}),
            );
        }
    }

    /// Whether any open surface still draws a launcher icon; the renderer
    /// drops them when the last one goes.
    fn check_apps(&mut self) {
        let used = self.managed.iter().any(|m| m.uses_apps);
        if self.apps_used && !used {
            self.apps_released = true;
        }
        self.apps_used = used;
    }

    // ------------------------------------------------------------ following

    /// Places each open popup again from its anchor and its size; a change of
    /// a pixel or more moves (or resizes) its window.
    fn follow_anchors(&mut self, env: &mut Env<'_>) {
        let ids: Vec<u64> = self
            .managed
            .iter()
            .filter(|m| m.stage != Stage::Closing && m.role != SurfaceRole::Tooltip && m.role != SurfaceRole::Overlay)
            .map(|m| m.id)
            .collect();
        for id in ids {
            let started = Instant::now();
            self.follow_one(id, env);
            self.note_slow(id, started);
        }
    }

    /// `follow_anchors` for one popup.
    fn follow_one(&mut self, id: u64, env: &mut Env<'_>) {
        let Some(i) = self.idx(id) else { return };
        let m = &mut self.managed[i];
        let wanted = place::Wanted {
            props: &m.props,
            anchor: m.anchor_screen,
            flat: m.flat_screen,
            monitor: &m.monitor,
            scale: m.scale,
            blur: m.blur,
            trusted: m.props.trusted,
        };
        let layout = place::plan(&mut m.scene, env.renderer, &wanted);
        m.side = layout.side;
        if layout.rect == m.rect {
            return;
        }
        let resized = layout.rect.width() != m.rect.width() || layout.rect.height() != m.rect.height();
        m.rect = layout.rect;
        let rect = layout.rect;
        self.stats.emit_with(
            "popup",
            || json!({"name": m.name, "state": "moved", "rect": [rect.x0, rect.y0, rect.x1, rect.y1]}),
        );
        let m = &mut self.managed[i];
        if let Err(e) = env.ui.move_popup(m.win, layout.rect) {
            log::warn!("a popup could not be moved: {e}");
            return;
        }
        if resized && let Some(g) = env.ui.popup_geometry(m.win) {
            m.scene.set_geometry(g);
            m.need_full = true;
        }
        self.frame_one(id, env);
    }

    // ------------------------------------------------------------ the outside-press watch

    /// On exactly while an open popup that closes on outside presses has no
    /// keyboard focus; off the moment there is none.
    fn update_watch(&mut self, env: &mut Env<'_>) {
        let now = env.now;
        let wanted = self.managed.iter().any(|m| {
            m.role != SurfaceRole::Tooltip
                && m.stage != Stage::Closing
                && !m.holds
                && m.props.dismiss.outside
                && keeps_watch(m.gesture, m.props.trusted, now.saturating_duration_since(m.opened_at))
        });
        if wanted == self.watch_wanted {
            if wanted && !self.o.no_hook {
                // Cheap while it runs; starts it again when its thread ended (a failed re-install).
                env.ui.watch_outside_presses(true);
            }
            return;
        }
        self.watch_wanted = wanted;
        if !self.o.no_hook {
            env.ui.watch_outside_presses(wanted);
        }
        let stats = if self.o.no_hook { None } else { env.ui.hook_stats() };
        self.stats.emit(
            "hook",
            json!({"on": wanted && !self.o.no_hook, "wanted": wanted,
                   "events": stats.map(|s| s.events), "max_us": stats.map(|s| s.max_us),
                   "p99_delivery_us": stats.map(|s| s.p99_delivery_us), "reinstalls": stats.map(|s| s.reinstalls)}),
        );
    }
}

impl Managed {
    /// The content changed: whether the window can take focus follows it.
    fn retree(&mut self, ui: &mut dyn Win, now: Instant) {
        let content = element_at(&self.tree, &self.path);
        let focusable = focus::window_focusable(self.role, &self.props, content);
        // The lock curtain: while the root has the class `locking` the overlay waits for Windows to lock.
        let locking = content.is_some_and(|e| e.classes.iter().any(|c| &**c == "locking"));
        match (locking, self.locking_since) {
            (true, None) => self.locking_since = Some(now),
            (false, Some(_)) => self.locking_since = None,
            _ => {}
        }
        if focusable != self.focusable {
            self.focusable = focusable;
            ui.set_popup_focusable(self.win, focusable);
        }
    }
}

/// Whether a popup of this age still needs the outside-press watch: one the user opened (a click or a
/// key) or a built-in always; one a widget opened on its own for `UNASKED_WATCH` only, so a timer cannot
/// keep a desktop-wide mouse hook installed for the whole session.
fn keeps_watch(gesture: u64, trusted: bool, age: Duration) -> bool {
    gesture != 0 || trusted || age < UNASKED_WATCH
}

fn min_opt(a: Option<Instant>, b: Option<Instant>) -> Option<Instant> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

fn to_screen(r: &RectF, origin: (i32, i32)) -> RectF {
    RectF::new(r.x + origin.0 as f32, r.y + origin.1 as f32, r.w, r.h)
}

fn flat_to_screen(f: &FlatEdges, origin: (i32, i32)) -> FlatEdges {
    let shift = |s: Option<(f32, f32)>| s.map(|(a, b)| (a + origin.0 as f32, b + origin.0 as f32));
    FlatEdges { top: shift(f.top), bottom: shift(f.bottom) }
}

/// Adds one scene's declarations, naming the unnamed ones `<widget>-<n>` in tree order.
fn push_decls(
    out: &mut Vec<DeclRec>,
    decls: Vec<PopupDecl>,
    anchor: Anchor,
    bar: BarId,
    origin: (i32, i32),
    device: &str,
) {
    let mut counts: HashMap<InstanceId, u32> = HashMap::new();
    for d in decls {
        let n = counts.entry(d.instance).or_insert(0);
        *n += 1;
        let name = match d.props.name() {
            Some(n) => n.to_string(),
            None => format!("{}-{}", d.tree.widget.css_class(), n),
        };
        out.push(DeclRec { anchor, bar, origin, device: device.to_string(), name, d });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsb_core::element::ImageRef;

    fn image(path: &str) -> Element {
        let mut e = Element::new(ElementKind::Image);
        e.image = Some(ImageRef { path: Arc::from(path), width: None, height: None });
        e
    }

    #[test]
    fn launcher_icons_are_found_anywhere_below_the_popup_element() {
        let mut root = Element::new(ElementKind::Popup);
        let mut row = Element::new(ElementKind::Box);
        row.children.push(image("rsb-image:apps/abc"));
        root.children.push(row);
        assert!(uses_prefix(&root, APPS_PREFIX));
        let mut other = Element::new(ElementKind::Popup);
        other.children.push(image("rsb-image:media/1"));
        other.children.push(image("C:/pictures/a.png"));
        assert!(!uses_prefix(&other, APPS_PREFIX));
        assert!(!uses_prefix(&Element::new(ElementKind::Popup), APPS_PREFIX));
    }

    #[test]
    fn a_popup_a_widget_opened_on_its_own_holds_the_mouse_watch_for_a_minute_only() {
        let young = Duration::from_secs(59);
        let old = Duration::from_secs(61);
        assert!(keeps_watch(0, false, young));
        assert!(!keeps_watch(0, false, old), "a timer-opened popup of a user widget lets the hook go");
        assert!(keeps_watch(12, false, old), "one a click opened keeps it while it is open");
        assert!(keeps_watch(0, true, old), "a built-in does too");
    }

    #[test]
    fn the_shift_of_an_offscreen_bar_and_the_overlay_watch_flags() {
        let to = to_screen(&RectF::new(10.0, 4.0, 30.0, 20.0), (100, 50));
        assert_eq!((to.x, to.y, to.w, to.h), (110.0, 54.0, 30.0, 20.0));
        let flat = flat_to_screen(&FlatEdges { top: None, bottom: Some((5.0, 25.0)) }, (100, 50));
        assert_eq!((flat.top, flat.bottom), (None, Some((105.0, 125.0))));
        let w = OverlayWatch::default();
        assert!(!w.shown.load(Ordering::Relaxed));
        assert_eq!(w.hidden.load(Ordering::Relaxed), 0);
        let t = Instant::now();
        assert_eq!(min_opt(Some(t), None), Some(t));
        assert_eq!(min_opt(None, None), None);
        assert_eq!(min_opt(Some(t + Duration::from_secs(1)), Some(t)), Some(t));
    }
}

//! The panels of the default look: the dashboard and its pages, the launcher, the power menu and the
//! calendar, player, mixer and Wi-Fi popups. Each is built by its widget, laid out and drawn by the real
//! scene and renderer, and driven with simulated pointer and keys the way the window layer drives it.

mod common;

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use rsb_core::color::Color;
use rsb_core::config::{self, BarSpec, Config, WidgetOptions};
use rsb_core::css::{MediaEnv, Stylesheet};
use rsb_core::draw::{DrawCmd, DrawList, Fill};
use rsb_core::element::{ElementKind, InstanceId, NodeKey};
use rsb_core::geom::{PointF, RectF, RectPx, SizeF};
use rsb_core::hit::{Button, ElementCall, ElementEvent, HandlerCall, InputKind, PointerEvent, PointerKind};
use rsb_core::key::{Key, KeyEvent, Modifiers, TextInput};
use rsb_core::msg::{Heartbeat, HubEvent, HubRequest, InstanceSpec, MonitorRef, ScriptIn};
use rsb_core::popup::{CloseReason, Placement, PopupAction, PopupDecl, Side};
use rsb_core::scene::{BarGeometry, NodeInfo, PopupSurfaceSpec, Scene, SceneEnv, SlotSpec, ZoneSlots};
use rsb_core::services::{ServiceCommand, ServiceEvent, ServiceId, ServiceStatus};
use rsb_core::time::ThreadTime;
use rsb_core::value::Value;
use rsb_render::{Canvas, FontConfig, Renderer, render_offscreen};
use rsb_script::{Limits, Runtime, ScriptOptions, SourceStore};

// ---------------------------------------------------------------- harness

fn assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("assets")
}

fn png_dir() -> PathBuf {
    let dir = match std::env::var_os("RSB_PANEL_PNG_DIR") {
        Some(d) => PathBuf::from(d),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("target").join("widget-png"),
    };
    std::fs::create_dir_all(&dir).expect("the PNG folder can be made");
    dir
}

/// Built-ins from the assets folder on disk.
struct DiskStore {
    root: PathBuf,
    user: Arc<MemStore>,
}

impl SourceStore for DiskStore {
    fn builtin(&self, rel_path: &str) -> Option<Cow<'static, str>> {
        std::fs::read_to_string(self.root.join(rel_path)).ok().map(Cow::Owned)
    }

    fn builtin_widgets(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.root.join("widgets"))
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".luau")).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn user(&self, rel_path: &str) -> std::io::Result<Option<String>> {
        self.user.user(rel_path)
    }
}

/// Behaviour tests, not speed tests: script CPU is never charged, so a slow or busy PC cannot stop a widget
/// mid-test (the per-second CPU window counts on the manual clock, which stands still between key presses).
/// The wall-clock hang guard still applies; the limits have their own tests (limits.rs, sandbox.rs).
struct NoCpu;

impl ThreadTime for NoCpu {
    fn now(&self) -> Duration {
        Duration::ZERO
    }
}

fn bench() -> Harness {
    let mut h = Harness::new();
    let user = h.store.clone();
    let clock = h.clock.clone();
    let heartbeat = Arc::new(Heartbeat::new());
    let rt = Runtime::new(
        ScriptOptions {
            sources: Arc::new(DiskStore { root: assets(), user }),
            clock,
            config_dir: h.config_dir.clone(),
            path_env: path_env(),
            limits: Limits::default(),
            dark: true,
            thread_time: Some(Arc::new(NoCpu)),
            heartbeat: heartbeat.clone(),
        },
        Box::new(FakeHub(h.hub.clone(), heartbeat.clone(), h.hub_busy.clone())),
        Box::new(FakeSink(h.out.clone())),
    );
    h.rt = rt;
    h.heartbeat = heartbeat;
    h
}

fn num(n: f64) -> Value {
    Value::Num(n)
}

fn s(text: &str) -> Value {
    Value::Str(text.to_string())
}

fn b(v: bool) -> Value {
    Value::Bool(v)
}

fn list(items: Vec<Value>) -> Value {
    Value::List(items)
}

fn svc(service: ServiceId, status: ServiceStatus, value: Option<Value>, error: Option<&str>) -> ScriptIn {
    ScriptIn::Hub(HubEvent::Service(ServiceEvent {
        service,
        version: 1,
        status,
        value,
        error: error.map(str::to_string),
    }))
}

fn ok(service: ServiceId, value: Value) -> ScriptIn {
    svc(service, ServiceStatus::Ok, Some(value), None)
}

// ---------------------------------------------------------------- made-up service values

struct Out {
    id: &'static str,
    name: &'static str,
    default: bool,
    volume: f64,
    muted: bool,
    bluetooth: bool,
}

fn outputs() -> Vec<Out> {
    vec![
        Out {
            id: "out-1",
            name: "Studio Monitor Output Controller High",
            default: false,
            volume: 55.0,
            muted: false,
            bluetooth: false,
        },
        Out {
            id: "out-2",
            name: "Studio Monitor Output Controller Lowest",
            default: false,
            volume: 30.0,
            muted: true,
            bluetooth: false,
        },
        Out {
            id: "out-3",
            name: "Desk Dock Audio Adapter",
            default: false,
            volume: 80.0,
            muted: false,
            bluetooth: false,
        },
        Out { id: "out-4", name: "Quiet Room Headset", default: false, volume: 20.0, muted: false, bluetooth: true },
        Out { id: "out-5", name: "Workshop Speakers", default: true, volume: 35.0, muted: false, bluetooth: false },
    ]
}

fn audio_value(devices: &[Out], available: bool) -> Value {
    let default = devices.iter().find(|d| d.default);
    map(&[
        ("available", b(available)),
        ("volume", num(default.map_or(0.0, |d| d.volume))),
        ("muted", b(default.is_some_and(|d| d.muted))),
        ("switch_available", b(true)),
        (
            "devices",
            list(
                devices
                    .iter()
                    .map(|d| {
                        map(&[
                            ("id", s(d.id)),
                            ("name", s(d.name)),
                            ("kind", s("output")),
                            ("default", b(d.default)),
                            ("volume", num(d.volume)),
                            ("muted", b(d.muted)),
                            ("bluetooth", b(d.bluetooth)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("inputs", list(vec![])),
    ])
}

fn bt_dev(id: &str, name: &str, connected: bool, battery: Option<f64>, can_connect: bool) -> Value {
    let mut pairs = vec![
        ("id", s(id)),
        ("name", s(name)),
        ("kind", s("audio")),
        ("connected", b(connected)),
        ("can_connect", b(can_connect)),
    ];
    if let Some(p) = battery {
        pairs.push(("battery", num(p)));
    }
    map(&pairs)
}

fn bluetooth_value(available: bool, enabled: bool, devices: Vec<Value>) -> Value {
    map(&[
        ("available", b(available)),
        ("enabled", b(enabled)),
        ("can_toggle", b(available)),
        ("devices", list(devices)),
    ])
}

fn paired() -> Vec<Value> {
    vec![
        bt_dev("bt-1", "Garden Party Speaker", false, None, true),
        bt_dev("bt-2", "Desk Mouse", false, None, false),
        bt_dev("bt-3", "Quiet Room Headset", false, Some(61.0), true),
        bt_dev("bt-4", "Ridge Runner Earbuds", true, Some(27.0), true),
    ]
}

const APP_NAMES: [&str; 6] =
    ["Alder Notes", "Birch Browser", "Bit Tracker", "Maple Mail", "Oak Planner", "Willow Paint"];

fn apps_value(ready: bool, names: &[&str]) -> Value {
    map(&[
        ("ready", b(ready)),
        ("apps", list(names.iter().map(|n| map(&[("id", s(&format!("app:{n}"))), ("name", s(n))])).collect())),
    ])
}

fn power_value(mode: &str, available: bool, can_sleep: bool) -> Value {
    map(&[("mode", s(mode)), ("mode_available", b(available)), ("can_sleep", b(can_sleep))])
}

fn net(ssid: &str, signal: f64, secure: bool, connected: bool, known: bool) -> Value {
    map(&[
        ("ssid", s(ssid)),
        ("signal", num(signal)),
        ("secure", b(secure)),
        ("connected", b(connected)),
        ("known", b(known)),
    ])
}

fn wifi_value(networks: Vec<Value>, access: &str) -> Value {
    map(&[("available", b(true)), ("scanning", b(false)), ("networks", list(networks)), ("access", s(access))])
}

fn nets() -> Vec<Value> {
    vec![
        net("Workshop", 80.0, true, true, true),
        net("Guest Lounge", 62.0, false, false, true),
        net("Loading Dock", 41.0, true, false, true),
        net("Neighbour", 18.0, true, false, false),
    ]
}

fn track() -> Value {
    map(&[
        ("available", b(true)),
        ("playing", b(true)),
        ("title", s("Slow Harbour Light")),
        ("artist", s("Aurora Quartet")),
        ("album", s("")),
        ("source", s("player")),
        ("can_play_pause", b(true)),
        ("can_next", b(true)),
        ("can_previous", b(true)),
        ("can_seek", b(true)),
        ("length", num(200.0)),
        ("position", num(34.0)),
        ("position_at", Value::Int(pinned_now().to_epoch_ms())),
    ])
}

fn pinned_now() -> rsb_core::time::LocalTime {
    rsb_core::time::LocalTime {
        year: 2026,
        month: 11,
        day: 25,
        weekday: 3,
        hour: 11,
        minute: 10,
        second: 0,
        millis: 0,
        utc_offset_min: 600,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum World {
    Normal,
    Empty,
    Stale,
    Error,
}

impl World {
    const ALL: [World; 4] = [World::Normal, World::Empty, World::Stale, World::Error];

    fn name(self) -> &'static str {
        match self {
            World::Normal => "normal",
            World::Empty => "empty",
            World::Stale => "stale",
            World::Error => "error",
        }
    }

    fn events(self) -> Vec<ScriptIn> {
        let normal = |apps: &[&str]| {
            vec![
                ok(
                    ServiceId::System,
                    map(&[
                        ("cpu", num(2.0)),
                        ("memory", num(51.0)),
                        ("memory_used", Value::Int(8_160_000_000)),
                        ("memory_total", Value::Int(16_000_000_000)),
                    ]),
                ),
                ok(
                    ServiceId::Battery,
                    map(&[
                        ("present", b(true)),
                        ("percent", num(100.0)),
                        ("charging", b(false)),
                        ("plugged", b(true)),
                        ("saver", b(false)),
                    ]),
                ),
                ok(ServiceId::Audio, audio_value(&outputs(), true)),
                ok(
                    ServiceId::Network,
                    map(&[
                        ("connected", b(true)),
                        ("internet", b(true)),
                        ("kind", s("wifi")),
                        ("signal", num(72.0)),
                        ("ssid", s("Workshop")),
                        ("down_bps", Value::Int(0)),
                        ("up_bps", Value::Int(0)),
                    ]),
                ),
                ok(ServiceId::Media, track()),
                ok(
                    ServiceId::Microphone,
                    map(&[("available", b(true)), ("muted", b(false)), ("volume", num(70.0)), ("in_use", b(false))]),
                ),
                ok(ServiceId::Bluetooth, bluetooth_value(true, true, paired())),
                ok(ServiceId::Apps, apps_value(true, apps)),
                ok(ServiceId::Power, power_value("balanced", true, true)),
                ok(ServiceId::Wifi, wifi_value(nets(), "allowed")),
            ]
        };
        match self {
            World::Normal => normal(&APP_NAMES),
            World::Empty => vec![
                ok(ServiceId::Audio, audio_value(&[], false)),
                ok(ServiceId::Bluetooth, bluetooth_value(true, true, vec![])),
                ok(ServiceId::Apps, apps_value(true, &[])),
                ok(ServiceId::Power, power_value("unknown", false, false)),
                ok(ServiceId::Wifi, wifi_value(vec![], "allowed")),
                ok(ServiceId::Network, map(&[("connected", b(false)), ("internet", b(false))])),
                ok(ServiceId::Media, map(&[("available", b(false)), ("playing", b(false))])),
            ],
            World::Stale => {
                let mut ev = normal(&APP_NAMES);
                for id in [
                    ServiceId::Audio,
                    ServiceId::Bluetooth,
                    ServiceId::Apps,
                    ServiceId::Power,
                    ServiceId::Wifi,
                    ServiceId::Media,
                ] {
                    ev.push(svc(id, ServiceStatus::Stale, None, None));
                }
                ev
            }
            World::Error => {
                let mut ev = normal(&APP_NAMES);
                for id in [ServiceId::Audio, ServiceId::Bluetooth, ServiceId::Apps, ServiceId::Power, ServiceId::Wifi] {
                    ev.push(svc(id, ServiceStatus::Error, None, Some("the service failed")));
                }
                ev
            }
        }
    }
}

// ---------------------------------------------------------------- the rig

const SCREEN_W: u32 = 1920;
const SCREEN_H: u32 = 1200;

fn load_default() -> Config {
    let read = |name: &str| std::fs::read_to_string(assets().join("presets").join(format!("{name}.toml"))).ok();
    let user = "preset = \"default\"\n";
    let load = config::load_config(Some((user, "bar.toml")), &read);
    assert!(load.diagnostics.is_empty(), "{:?}", load.diagnostics);
    load.config.expect("the default preset loads")
}

fn sheet_for(preset: &str) -> Arc<Stylesheet> {
    let text = |rel: &str| std::fs::read_to_string(assets().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    let (mut sheet, diags) = Stylesheet::parse(&text("themes/base.css"), "base.css");
    assert!(diags.is_empty(), "base.css: {diags:?}");
    let (p, diags) = Stylesheet::parse(&text(&format!("presets/{preset}.css")), &format!("{preset}.css"));
    assert!(diags.is_empty(), "{preset}.css: {diags:?}");
    sheet.append(p);
    Arc::new(sheet)
}

fn monitor_ref() -> MonitorRef {
    MonitorRef { index: 0, handle: 1, device: "\\\\.\\DISPLAY1".into(), primary: true, scale: 1.0 }
}

fn options_for(config: &Config, widget: &config::WidgetRef) -> WidgetOptions {
    let mut out = WidgetOptions::default();
    for table in [config.widgets.get(&widget.name), config.widgets.get(&widget.to_string())].into_iter().flatten() {
        for (k, v) in &table.values {
            out.values.insert(k.clone(), v.clone());
        }
    }
    // The volume and network widgets are not part of the default bar; the rig adds them with their popups on.
    if ["volume", "network"].contains(&&*widget.name) && !out.values.contains_key("popup") {
        out.values.insert(
            "popup".into(),
            rsb_core::config::Spanned {
                value: Value::Bool(true),
                pos: rsb_core::diag::SourcePos::new("bar.toml", 1, 1),
            },
        );
    }
    out
}

fn renderer() -> Renderer {
    let mut r = Renderer::new(FontConfig::default());
    let font = std::fs::read(assets().join("fonts").join("JetBrainsMono-wght.ttf")).expect("the bundled font");
    r.register_font(Arc::from(font), None).expect("the bundled font registers");
    r
}

fn ensure_icons(renderer: &mut Renderer, list: &DrawList) {
    for item in &list.items {
        let DrawCmd::Svg { key, .. } = &item.cmd else { continue };
        if rsb_core::icons::is_svg_icon_name(key) && !renderer.has_svg(key) {
            let bytes = std::fs::read(assets().join("icons").join(format!("{key}.svg"))).expect("a bundled icon");
            renderer.set_svg(Arc::clone(key), &bytes).expect("the bundled icon parses");
        }
    }
}

/// One open panel: its scene, where the window went, and the declaration it came from.
struct Pop {
    scene: Scene,
    key: NodeKey,
    overlay: bool,
    rect: RectPx,
    side: Side,
    attach: Option<f32>,
}

thread_local! {
    static HELD: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static GUARD: std::cell::RefCell<Option<std::sync::MutexGuard<'static, ()>>> = const { std::cell::RefCell::new(None) };
}

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One rig at a time in the whole test process: popup names and the log are shared by every runtime, and a
/// test may keep several rigs of its own alive (the lock is held while any of them is).
fn hold_the_desk() {
    HELD.with(|h| {
        if h.get() == 0 {
            let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            GUARD.with(|g| *g.borrow_mut() = Some(guard));
        }
        h.set(h.get() + 1);
    });
}

fn release_the_desk() {
    HELD.with(|h| {
        h.set(h.get().saturating_sub(1));
        if h.get() == 0 {
            GUARD.with(|g| *g.borrow_mut() = None);
        }
    });
}

/// The default bar with the script runtime behind it, one popup at a time.
struct Rig {
    h: Harness,
    bar: BarSpec,
    slots: ZoneSlots,
    sheet: Arc<Stylesheet>,
    r: Renderer,
    scene: Scene,
    t0: Instant,
    ms: u64,
    dark: bool,
    pop: Option<Pop>,
    closed: Vec<CloseReason>,
    /// How many log lines there were when the rig started.
    log_base: usize,
}

impl Drop for Rig {
    fn drop(&mut self) {
        release_the_desk();
    }
}

fn scene_env(dark: bool, r: &Renderer) -> SceneEnv {
    SceneEnv { media: MediaEnv { dark }, icon_font: r.icon_font(), fullscreen: false }
}

impl Rig {
    fn new(world: World, dark: bool) -> Rig {
        Rig::with_events(world.events(), dark, &[])
    }

    /// The default bar with more widgets at the end of its right zone (the volume and network widgets are
    /// not part of the default bar; the mixer and the Wi-Fi list belong to them).
    fn with_extra(world: World, dark: bool, extra: &[&str]) -> Rig {
        Rig::with_events(world.events(), dark, extra)
    }

    fn with_events(events: Vec<ScriptIn>, dark: bool, extra: &[&str]) -> Rig {
        hold_the_desk();
        let log_base = log_lines("").len();
        let config = load_default();
        let mut bar = config.bars[0].clone();
        for name in extra {
            bar.right.push(config::ZoneItem::new(config::WidgetRef::parse(name).unwrap()));
        }
        let mut h = bench();
        h.clock.set_wall(pinned_now());
        let mut specs = Vec::new();
        let mut slots = ZoneSlots::default();
        let mut next = 1u32;
        for (zone, items) in [(0, &bar.left), (1, &bar.center), (2, &bar.right)] {
            for item in items {
                let id = InstanceId(next);
                next += 1;
                specs.push(InstanceSpec {
                    instance: id,
                    widget: item.widget.clone(),
                    options: options_for(&config, &item.widget),
                    bar: bar.name.clone(),
                    monitor: monitor_ref(),
                    pos: None,
                });
                let slot = SlotSpec { instance: id, class: item.widget.css_class(), id: item.widget.css_id() };
                match zone {
                    0 => slots.left.push(slot),
                    1 => slots.center.push(slot),
                    _ => slots.right.push(slot),
                }
            }
        }
        h.start_specs(specs);
        for e in events {
            h.send(e);
        }
        let r = renderer();
        let sheet = sheet_for("default");
        let mut scene =
            Scene::new(&bar, BarGeometry { width_px: SCREEN_W, height_px: 36, scale: 1.0 }, scene_env(dark, &r));
        scene.set_stylesheet(sheet.clone());
        scene.set_slots(slots.clone());
        let mut rig = Rig {
            h,
            bar,
            slots,
            sheet,
            r,
            scene,
            t0: Instant::now(),
            ms: 0,
            dark,
            pop: None,
            closed: Vec::new(),
            log_base,
        };
        rig.push_trees();
        rig
    }

    fn now(&self) -> Instant {
        self.t0 + Duration::from_millis(self.ms)
    }

    fn push_trees(&mut self) {
        for tree in self.h.latest.values() {
            self.scene.set_tree(tree.clone());
        }
        let now = self.now();
        self.scene.frame(now, &mut self.r);
    }

    fn decls(&self) -> Vec<PopupDecl> {
        self.scene.popups()
    }

    fn decl(&self, name: &str) -> Option<PopupDecl> {
        self.decls().into_iter().find(|d| d.props.names.iter().any(|n| &**n == name))
    }

    /// The script's tree changed: bar and popup scene take it.
    fn sync(&mut self) {
        self.push_trees();
        let Some(key) = self.pop.as_ref().map(|p| p.key) else { return };
        let decl = self.decls().into_iter().find(|d| d.key == key);
        let now = self.now();
        let Some(p) = self.pop.as_mut() else { return };
        match decl {
            Some(d) if d.open => {
                p.scene.set_popup_tree(d.tree.clone(), d.path.clone());
                self.replan();
            }
            _ => {
                p.scene.set_open(false, now);
            }
        }
    }

    /// `rsb popup <name>`: a gesture, then the popup scene the manager would make.
    fn open(&mut self, name: &str, keyboard: bool) {
        let g = self.h.next_gesture();
        self.h.send(ScriptIn::Popup {
            name: name.to_string(),
            action: PopupAction::Open,
            monitor: Some(1),
            gesture: g,
        });
        self.push_trees();
        let d = self.decl(name).unwrap_or_else(|| panic!("no popup named {name}"));
        assert!(d.open, "{name} did not open");
        self.make_popup(&d, keyboard);
    }

    fn make_popup(&mut self, d: &PopupDecl, keyboard: bool) {
        let overlay = d.props.placement == Placement::Screen;
        let side = if overlay { Side::Screen } else { Side::Below };
        let geometry =
            BarGeometry { width_px: SCREEN_W, height_px: if overlay { SCREEN_H } else { SCREEN_H - 36 }, scale: 1.0 };
        let slot = self
            .slots
            .left
            .iter()
            .chain(&self.slots.center)
            .chain(&self.slots.right)
            .find(|s| s.instance == d.instance)
            .cloned()
            .expect("the slot of the popup's widget");
        let spec = PopupSurfaceSpec { bar: self.bar.clone(), slot, kind: d.kind, side, blur: false };
        let mut scene = Scene::new_popup(spec, geometry, scene_env(self.dark, &self.r));
        scene.set_stylesheet(self.sheet.clone());
        scene.set_popup_tree(d.tree.clone(), d.path.clone());
        let attach = if d.props.attach { d.anchor_flat.bottom.map(|(a, bb)| bb - a) } else { None };
        scene.set_popup_surface(side, false, attach);
        let (size, placed) = self.plan(&mut scene, d);
        scene.set_geometry(BarGeometry { width_px: size.w.ceil() as u32, height_px: size.h.ceil() as u32, scale: 1.0 });
        let now = self.now();
        scene.set_window_focus(true, now);
        // The first frame is the closed style; the panel opens and its anchor stretches at one instant.
        scene.frame(now, &mut self.r);
        scene.set_open(true, now);
        scene.focus_first(keyboard, now);
        scene.frame(now, &mut self.r);
        self.scene.set_anchor_open(d.key, true, now);
        self.scene.frame(now, &mut self.r);
        self.pop = Some(Pop { scene, key: d.key, overlay, rect: placed.rect, side: placed.side, attach });
        // The anchor has its open flag now: its flat edge is where the panel goes.
        self.replan();
    }

    fn plan(&mut self, scene: &mut Scene, d: &PopupDecl) -> (SizeF, rsb_core::popup::Placed) {
        let size = scene.natural_size(&mut self.r);
        let work = RectPx::new(0, 0, SCREEN_W as i32, SCREEN_H as i32);
        let placed = rsb_core::popup::place(&rsb_core::popup::PlaceInput {
            anchor: d.anchor_rect,
            size,
            work,
            monitor: work,
            placement: d.props.placement,
            align: d.props.align,
            offset: d.props.offset,
            scale: 1.0,
            attach: d.props.attach.then_some(d.anchor_flat),
        });
        (size, placed)
    }

    /// The manager places a popup again on every pass: the anchor's flat edge, the width taken from it and
    /// the content's size may have changed since.
    fn replan(&mut self) {
        let Some(key) = self.pop.as_ref().map(|p| p.key) else { return };
        let Some(d) = self.decls().into_iter().find(|d| d.key == key) else { return };
        let mut pop = self.pop.take().unwrap();
        let attach = if d.props.attach { d.anchor_flat.bottom.map(|(a, bb)| bb - a) } else { None };
        pop.scene.set_popup_surface(pop.side, false, attach);
        let (size, placed) = self.plan(&mut pop.scene, &d);
        pop.scene.set_geometry(BarGeometry {
            width_px: size.w.ceil() as u32,
            height_px: size.h.ceil() as u32,
            scale: 1.0,
        });
        pop.rect = placed.rect;
        pop.attach = attach;
        self.pop = Some(pop);
    }

    /// Time passes: the script's timers, then a frame of every scene every 16 ms.
    fn step(&mut self, ms: u64) {
        let mut left = ms;
        while left > 0 {
            let d = left.min(16);
            left -= d;
            self.ms += d;
            self.h.advance(d);
            self.push_trees();
            let now = self.now();
            if self.pop.is_some() {
                self.sync();
                let p = self.pop.as_mut().unwrap();
                p.scene.frame(now, &mut self.r);
                let events = p.scene.take_events();
                for e in events {
                    self.h.send(ScriptIn::Element(e));
                }
            }
        }
    }

    fn natural(&mut self) -> SizeF {
        let p = self.pop.as_mut().expect("a popup is open");
        p.scene.natural_size(&mut self.r)
    }

    fn pop(&mut self) -> &mut Pop {
        self.pop.as_mut().expect("a popup is open")
    }

    fn frame_pop(&mut self) -> Arc<DrawList> {
        let now = self.now();
        let p = self.pop.as_mut().expect("a popup is open");
        let out = p.scene.frame(now, &mut self.r);
        ensure_icons(&mut self.r, &out.draw);
        out.draw
    }

    fn nodes(&mut self) -> Vec<NodeInfo> {
        let now = self.now();
        let p = self.pop.as_mut().expect("a popup is open");
        p.scene.frame(now, &mut self.r);
        p.scene.nodes()
    }

    /// Hands every handler call to the script as the window layer would, with a new gesture for each.
    fn calls(&mut self, calls: Vec<HandlerCall>) {
        for mut c in calls {
            if matches!(c.kind, InputKind::Click(_)) {
                c.gesture = self.h.next_gesture();
            }
            self.h.send(ScriptIn::Input(c));
        }
    }

    fn events(&mut self) {
        let Some(p) = self.pop.as_mut() else { return };
        let events = p.scene.take_events();
        for mut e in events {
            if matches!(e.event, ElementEvent::Key(_) | ElementEvent::Submitted(_)) {
                e.gesture = self.h.next_gesture();
            }
            self.h.send(ScriptIn::Element(e));
        }
        self.sync();
    }

    fn pointer(&mut self, kind: PointerKind, x: f32, y: f32) {
        let now = self.now();
        let p = self.pop.as_mut().expect("a popup is open");
        let res = p.scene.pointer(PointerEvent { kind, x, y }, now);
        self.calls(res.calls);
        self.events();
    }

    fn hover(&mut self, x: f32, y: f32) {
        self.pointer(PointerKind::Move, x, y);
    }

    fn click(&mut self, x: f32, y: f32) {
        self.pointer(PointerKind::Move, x, y);
        self.pointer(PointerKind::Down(Button::Left), x, y);
        self.pointer(PointerKind::Up(Button::Left), x, y);
    }

    fn click_node(&mut self, class: &str, nth: usize) {
        let r = self.rect(class, nth);
        self.click(r.x + r.w / 2.0, r.y + r.h / 2.0);
    }

    fn wheel(&mut self, x: f32, y: f32, notches: f32) {
        self.pointer(PointerKind::Wheel(notches), x, y);
    }

    fn key_with(&mut self, key: Key, mods: Modifiers) {
        let now = self.now();
        let key_ev = KeyEvent { key, mods, down: true, repeat: false };
        let p = self.pop.as_mut().expect("a popup is open");
        let res = p.scene.key(key_ev, now);
        let dismiss = res.dismiss;
        self.calls(res.calls);
        self.events();
        if dismiss {
            self.ui_closes(CloseReason::Escape);
        }
    }

    fn key(&mut self, key: Key) {
        self.key_with(key, Modifiers::default());
    }

    fn type_text(&mut self, text: &str) {
        let now = self.now();
        let p = self.pop.as_mut().expect("a popup is open");
        let res = p.scene.text_input(TextInput::Chars(text.to_string()), now);
        self.calls(res.calls);
        self.events();
        // The leading edge reports at once; the trailing one after the quiet period.
        self.step(60);
    }

    /// The window layer closes the popup and tells the script.
    fn ui_closes(&mut self, reason: CloseReason) {
        let key = self.pop().key;
        let d = self.decls().into_iter().find(|d| d.key == key).expect("the declaration");
        if let Some(h) = d.props.on_close {
            self.h.send(ScriptIn::Element(ElementCall {
                instance: d.instance,
                handler: h,
                event: ElementEvent::PopupClosed(reason),
                gesture: 0,
            }));
        }
        self.closed.push(reason);
        self.push_trees();
        let now = self.now();
        let p = self.pop();
        p.scene.dismiss_popup(key, d.props.open_seq);
        p.scene.set_open(false, now);
    }

    fn is_open(&self, name: &str) -> bool {
        self.decl(name).is_some_and(|d| d.open)
    }

    fn rects(&mut self, class: &str) -> Vec<RectF> {
        self.nodes().into_iter().filter(|n| n.classes.iter().any(|c| &**c == class)).map(|n| n.rect).collect()
    }

    fn rect(&mut self, class: &str, nth: usize) -> RectF {
        let all = self.rects(class);
        *all.get(nth).unwrap_or_else(|| panic!("no node {nth} with class {class}: {} found", all.len()))
    }

    fn texts(&mut self) -> Vec<String> {
        self.nodes().into_iter().filter_map(|n| n.text.map(|t| t.to_string())).filter(|t| !t.is_empty()).collect()
    }

    fn commands(&self) -> Vec<ServiceCommand> {
        self.h
            .hub
            .lock()
            .unwrap()
            .iter()
            .filter_map(|r| match r {
                HubRequest::Command { command, .. } => Some(command.clone()),
                _ => None,
            })
            .collect()
    }

    fn kinds(&self) -> Vec<String> {
        self.commands().iter().map(|c| c.kind().to_string()).collect()
    }

    /// Runs frames until every transition is done (at most `limit` ms).
    fn settle(&mut self, limit: u64) {
        let mut spent = 0;
        while spent < limit {
            self.step(16);
            spent += 16;
            let settled = self.pop.as_ref().is_none_or(|p| p.scene.is_settled());
            if settled && self.scene.is_settled() {
                break;
            }
        }
    }

    // ------------------------------------------------------------ pictures

    /// The screen: a flat wallpaper, the bar, the open popup (and the dim of an overlay).
    fn picture(&mut self) -> Canvas {
        let mut canvas = Canvas::new(SCREEN_W, SCREEN_H);
        // Under a dimmed overlay the wallpaper is a darker flat grey, so the first button's fill (about what
        // the dim makes of the lighter one) still stands out as it does on a real, busy wallpaper.
        let under_dim = self.pop.as_ref().is_some_and(|p| p.overlay);
        let wall = match (self.dark, under_dim) {
            (true, true) => [0x3c, 0x40, 0x46],
            (true, false) => [0x58, 0x5c, 0x62],
            (false, _) => [0xb4, 0xb9, 0xc0],
        };
        for px in canvas.data.as_chunks_mut::<4>().0.iter_mut() {
            px.copy_from_slice(&[wall[2], wall[1], wall[0], 255]);
        }
        let now = self.now();
        let bar = self.scene.frame(now, &mut self.r);
        ensure_icons(&mut self.r, &bar.draw);
        let bar_canvas = render_offscreen(&mut self.r, &bar.draw).unwrap();
        blend(&mut canvas, &bar_canvas, 0, 0);
        if self.pop.is_some() {
            let overlay = self.pop().overlay;
            let dim = self.pop().scene.overlay_dim();
            let draw = self.frame_pop();
            let content = render_offscreen(&mut self.r, &draw).unwrap();
            let (x, y) = if overlay {
                if let Some(c) = dim {
                    fill_over(&mut canvas, c);
                }
                let off = self.pop().scene.root_offset();
                (-off.x.round() as i32, -off.y.round() as i32)
            } else {
                let r = self.pop().rect;
                (r.x0, r.y0)
            };
            blend(&mut canvas, &content, x, y);
        }
        canvas
    }
}

/// `top` over `base` at (x0, y0): both premultiplied BGRA.
fn blend(base: &mut Canvas, top: &Canvas, x0: i32, y0: i32) {
    for y in 0..top.height as i32 {
        for x in 0..top.width as i32 {
            let (dx, dy) = (x + x0, y + y0);
            if dx < 0 || dy < 0 || dx >= base.width as i32 || dy >= base.height as i32 {
                continue;
            }
            let si = (y as usize * top.width as usize + x as usize) * 4;
            let di = (dy as usize * base.width as usize + dx as usize) * 4;
            let a = top.data[si + 3] as u32;
            for c in 0..3 {
                base.data[di + c] =
                    (top.data[si + c] as u32 + base.data[di + c] as u32 * (255 - a) / 255).min(255) as u8;
            }
        }
    }
}

fn fill_over(base: &mut Canvas, c: Color) {
    let a = c.a.clamp(0.0, 1.0);
    let px = [(c.b * 255.0).round(), (c.g * 255.0).round(), (c.r * 255.0).round()];
    for p in base.data.as_chunks_mut::<4>().0.iter_mut() {
        for (i, v) in px.iter().enumerate() {
            p[i] = (p[i] as f32 * (1.0 - a) + v * a).round() as u8;
        }
    }
}

fn save(canvas: &Canvas, name: &str) -> PathBuf {
    let path = png_dir().join(name);
    std::fs::write(&path, canvas.to_png().expect("the PNG encodes")).expect("the PNG is written");
    path
}

// ---------------------------------------------------------------- more of the rig

impl Rig {
    fn instance(&self, widget: &str) -> u32 {
        self.slots
            .left
            .iter()
            .chain(&self.slots.center)
            .chain(&self.slots.right)
            .find(|s| s.class == widget)
            .unwrap_or_else(|| panic!("no widget {widget} in the bar"))
            .instance
            .0
    }

    /// A pointer click on the bar at the middle of the first node with this class; a popup that
    /// turns open is made as the manager would make it.
    fn click_bar_with(&mut self, class: &str, button: Button) {
        let now = self.now();
        let r = self
            .scene
            .nodes()
            .into_iter()
            .find(|n| n.classes.iter().any(|c| &**c == class))
            .unwrap_or_else(|| panic!("no bar node with class {class}"))
            .rect;
        let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        for kind in [PointerKind::Move, PointerKind::Down(button), PointerKind::Up(button)] {
            let res = self.scene.pointer(PointerEvent { kind, x, y }, now);
            self.calls(res.calls);
        }
        self.push_trees();
        if self.pop.is_none()
            && let Some(d) = self.decls().into_iter().find(|d| d.open)
        {
            self.make_popup(&d, false);
        }
    }

    fn click_bar(&mut self, class: &str) {
        self.click_bar_with(class, Button::Left);
    }

    fn tab(&mut self) {
        self.key(Key::Tab);
    }

    fn pointer_away(&mut self) {
        self.pointer(PointerKind::Move, -50.0, -50.0);
    }

    /// The content element of an open popup, from the script's own tree.
    fn content(&self, widget: &str) -> rsb_core::element::Element {
        let id = self.instance(widget);
        find_kind(self.h.root(id).expect("a tree"), ElementKind::Popup).expect("a popup element").clone()
    }

    /// Every element of the popup's content with this class, in tree order.
    fn elements(&self, widget: &str, class: &str) -> Vec<rsb_core::element::Element> {
        let mut out = Vec::new();
        collect_class(&self.content(widget), class, &mut out);
        out
    }

    /// A click handler of the first element with this class, sent with the given gesture id.
    fn click_element(&mut self, widget: &str, class: &str, nth: usize, gesture: u64) {
        let els = self.elements(widget, class);
        let el = els.get(nth).unwrap_or_else(|| panic!("no element {nth} with class {class}"));
        let handler = el.handlers.click.unwrap_or_else(|| panic!("{class} has no click handler"));
        self.h.click_gesture(self.instance(widget), handler.0, gesture);
        self.sync();
    }

    fn classes_of_root(&mut self) -> Vec<String> {
        self.nodes().first().map(|n| n.classes.iter().map(|c| c.to_string()).collect()).unwrap_or_default()
    }

    /// Log lines this rig's run added that contain `needle`.
    fn new_log(&self, needle: &str) -> Vec<String> {
        log_lines("").into_iter().skip(self.log_base).filter(|l| l.contains(needle)).collect()
    }

    fn log_has(&self, needle: &str) -> bool {
        !self.new_log(needle).is_empty()
    }
}

fn collect_class(e: &rsb_core::element::Element, class: &str, out: &mut Vec<rsb_core::element::Element>) {
    if e.classes.iter().any(|c| &**c == class) {
        out.push(e.clone());
    }
    for c in &e.children {
        collect_class(c, class, out);
    }
}

fn has_class(n: &NodeInfo, class: &str) -> bool {
    n.classes.iter().any(|c| &**c == class)
}

fn center(r: &RectF) -> PointF {
    PointF { x: r.x + r.w / 2.0, y: r.y + r.h / 2.0 }
}

fn approx(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

fn rect_text(r: &RectF) -> String {
    format!("({:.1}, {:.1}, {:.1} x {:.1})", r.x, r.y, r.w, r.h)
}

/// Asserts a rect within a tolerance of x, y, w, h (None = not checked).
fn assert_rect(what: &str, r: RectF, want: (Option<f32>, Option<f32>, Option<f32>, Option<f32>), tol: f32) {
    let ok = |got: f32, w: Option<f32>| w.is_none_or(|w| approx(got, w, tol));
    assert!(
        ok(r.x, want.0) && ok(r.y, want.1) && ok(r.w, want.2) && ok(r.h, want.3),
        "{what}: {} against {want:?} (+/-{tol})",
        rect_text(&r)
    );
}

// ---------------------------------------------------------------- the panels in every state

/// What each surface must say in each state: a text that is on screen.
fn assert_has(texts: &[String], needle: &str, what: &str) {
    assert!(texts.iter().any(|t| t.contains(needle)), "{what}: {needle:?} not in {texts:?}");
}

fn assert_clean(rig: &mut Rig, what: &str) {
    for (id, plate) in &rig.h.plates {
        assert!(plate.is_none(), "{what}: widget {id:?} shows a plate");
    }
    let diags: Vec<String> =
        rig.pop().scene.take_diagnostics().into_iter().map(|d| format!("{:?}", d.diagnostic)).collect();
    assert!(diags.is_empty(), "{what}: {diags:?}");
    let errors = rig.new_log("ERROR ");
    assert!(errors.is_empty(), "{what}: {errors:?}");
}

#[test]
fn every_page_and_popup_builds_in_every_state_without_a_plate_or_a_warning() {
    for dark in [true, false] {
        for world in World::ALL {
            let what = |s: &str| format!("{s} / {} / {}", world.name(), if dark { "dark" } else { "light" });
            let w = world;

            // The dashboard and its four pages.
            let pages: [(usize, &str, [&str; 4]); 4] = [
                (
                    0,
                    "power mode",
                    [
                        "Balanced",
                        "Power mode is not available on this PC",
                        "Power mode is out of date",
                        "Power mode is not working",
                    ],
                ),
                (
                    1,
                    "bluetooth",
                    [
                        "Ridge Runner Earbuds",
                        "No paired devices",
                        "Bluetooth is out of date",
                        "Bluetooth is not working",
                    ],
                ),
                (
                    2,
                    "audio",
                    ["Workshop Speakers", "No output devices", "Audio is out of date", "Audio is not working"],
                ),
                (3, "apps", ["Maple Mail", "No apps", "The app list is out of date", "The app list is not working"]),
            ];
            for (tile, name, wants) in pages {
                let mut rig = Rig::new(w, dark);
                rig.open("dashboard", false);
                rig.settle(1500);
                let home = rig.texts();
                let _ = home;
                rig.click_node("tile", tile);
                rig.settle(1500);
                let texts = rig.texts();
                let want = wants[World::ALL.iter().position(|x| *x == w).unwrap()];
                assert_has(&texts, want, &what(name));
                assert_clean(&mut rig, &what(name));
            }

            // The secondary popups.
            let mut rig = Rig::with_extra(w, dark, &["volume", "network"]);
            rig.open("calendar", false);
            rig.settle(800);
            assert_has(&rig.texts(), "November 2026", &what("calendar"));
            assert_clean(&mut rig, &what("calendar"));

            let mut rig = Rig::with_extra(w, dark, &["volume", "network"]);
            if w == World::Empty {
                // No player: the widget hides itself, so its anchor is gone and so is the popup.
                assert!(rig.decl("media").is_none(), "{}", what("media"));
            } else {
                rig.open("media", false);
                rig.settle(800);
                let want = if w == World::Stale { "--" } else { "Slow Harbour Light" };
                assert_has(&rig.texts(), want, &what("media"));
                assert_clean(&mut rig, &what("media"));
            }

            let mut rig = Rig::with_extra(w, dark, &["volume", "network"]);
            rig.open("volume", false);
            rig.settle(800);
            let want = match w {
                World::Normal => "Workshop Speakers",
                World::Empty => "No output devices",
                World::Stale => "Audio is out of date",
                World::Error => "Audio is not working",
            };
            assert_has(&rig.texts(), want, &what("volume"));
            assert_has(&rig.texts(), "Volume", &what("volume"));
            assert_clean(&mut rig, &what("volume"));

            let mut rig = Rig::with_extra(w, dark, &["volume", "network"]);
            rig.open("network", false);
            rig.settle(800);
            let want = match w {
                World::Normal => "Guest Lounge",
                World::Empty => "No networks in range",
                World::Stale => "Wi-Fi is out of date",
                World::Error => "Wi-Fi is not working",
            };
            assert_has(&rig.texts(), want, &what("network"));
            assert_clean(&mut rig, &what("network"));

            // The power menu: four buttons, and what the service says when it fails.
            let mut rig = Rig::new(w, dark);
            rig.open("power", false);
            rig.settle(1500);
            assert_eq!(rig.rects("power-btn").len(), 4, "{}", what("power"));
            assert_clean(&mut rig, &what("power"));
        }
    }
}

#[test]
fn the_dashboard_home_has_six_tiles_and_the_launcher_name_opens_the_apps_page_at_once() {
    let mut rig = Rig::new(World::Normal, true);
    rig.open("dashboard", false);
    rig.settle(1500);
    assert_eq!(rig.rects("tile").len(), 6);
    assert!(rig.rects("search").is_empty(), "the tiles are showing, not a page");

    // `rsb popup launcher` on a fresh panel: the apps page is in the first tree and its search has focus.
    let mut rig = Rig::new(World::Normal, true);
    rig.open("launcher", true);
    assert_eq!(rig.rects("tile").len(), 0);
    assert_eq!(rig.rects("search").len(), 1, "the apps page is the first thing built");
    assert_has(&rig.texts(), "Alder Notes", "the launcher");
    // Typing lands in the search: the list is cut at once.
    rig.type_text("mapl");
    let texts = rig.texts();
    assert_has(&texts, "Maple Mail", "typed");
    assert!(!texts.iter().any(|t| t == "Alder Notes"), "{texts:?}");
}

// ---------------------------------------------------------------- the launcher (the behaviour list)

/// 120 made-up apps whose names all contain "app": a long list for the paging and the limit of 50.
fn many_apps() -> Vec<ScriptIn> {
    let mut ev = World::Normal.events();
    let names: Vec<String> = (1..=120).map(|i| format!("App {i:03}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    ev.push(ok(ServiceId::Apps, apps_value(true, &refs)));
    ev
}

fn launcher(events: Vec<ScriptIn>) -> Rig {
    let mut rig = Rig::with_events(events, true, &[]);
    rig.open("launcher", true);
    rig.settle(1500);
    rig
}

fn row_texts(rig: &mut Rig) -> Vec<String> {
    let nodes = rig.nodes();
    let mut out = Vec::new();
    let mut iter = nodes.iter().peekable();
    while let Some(n) = iter.next() {
        if n.kind == ElementKind::Box && has_class(n, "app") {
            // The text is the next node (the row's only child).
            if let Some(t) = iter.peek().and_then(|t| t.text.clone()) {
                out.push(t.to_string());
            }
        }
    }
    out
}

fn selected_text(rig: &mut Rig) -> Option<String> {
    let nodes = rig.nodes();
    let at = nodes.iter().position(|n| n.kind == ElementKind::Box && has_class(n, "selected"))?;
    nodes.get(at + 1).and_then(|n| n.text.clone()).map(|t| t.to_string())
}

fn launches(rig: &Rig) -> Vec<ServiceCommand> {
    rig.commands().into_iter().filter(|c| matches!(c, ServiceCommand::Apps(_))).collect()
}

fn launched(id: &str) -> ServiceCommand {
    ServiceCommand::Apps(rsb_core::services::AppsCommand::Launch(id.to_string()))
}

#[test]
fn launcher_opening_selects_row_one_and_an_empty_query_lists_every_app_a_to_z() {
    let mut rig = launcher(World::Normal.events());
    let rows = row_texts(&mut rig);
    assert_eq!(rows, APP_NAMES, "every app, in the service's A-Z order");
    assert_eq!(selected_text(&mut rig).as_deref(), Some("Alder Notes"), "row 1 is selected");
    // The service's list is sorted by name, and the launcher shows it as it is.
    let mut sorted: Vec<String> = rows.clone();
    sorted.sort_by_key(|n| n.to_lowercase());
    assert_eq!(rows, sorted);
}

#[test]
fn launcher_a_query_lists_at_most_fifty_best_first_and_no_match_says_so() {
    let mut rig = launcher(many_apps());
    assert_eq!(row_texts(&mut rig).len(), 120, "an empty query lists them all");
    rig.type_text("app");
    let rows = row_texts(&mut rig);
    assert_eq!(rows.len(), 50, "at most 50 matches");
    rig.type_text("9");
    let rows = row_texts(&mut rig);
    assert!(rows.len() <= 50 && !rows.is_empty(), "{}", rows.len());
    assert!(rows[0].contains('9'), "best first: {:?}", &rows[..3.min(rows.len())]);
    rig.type_text("zzzz");
    assert!(row_texts(&mut rig).is_empty());
    assert_has(&rig.texts(), "No apps match", "no match");
}

#[test]
fn launcher_rows_are_cut_at_the_list_inset_not_at_the_card_edge() {
    let mut rig = launcher(many_apps());
    let card = rig
        .nodes()
        .into_iter()
        .find(|n| has_class(n, "card") && has_class(n, "list"))
        .map(|n| n.rect)
        .expect("the list card");
    let origin = rig.pop().rect;
    let shot = rig.picture();
    let at = |x: f32, y: f32| {
        let (px, py) = (origin.x0 as f32 + x, origin.y0 as f32 + y);
        let i = (py as usize * shot.width as usize + px as usize) * 4;
        [shot.data[i], shot.data[i + 1], shot.data[i + 2]]
    };
    // the 10 px inset below the last visible row holds the card colour only: rows slide out under it
    let bg = at(card.x + card.w / 2.0, card.y + card.h - 1.0);
    for dy in 1..=9 {
        for dx in 12..(card.w as i32 - 12) {
            let y = card.y + card.h - dy as f32;
            assert_eq!(at(card.x + dx as f32, y), bg, "row ink {dy} px above the card's bottom edge");
        }
    }
}

#[test]
fn launcher_every_query_change_selects_row_one_again_and_scrolls_to_the_top() {
    let mut rig = launcher(many_apps());
    for _ in 0..20 {
        rig.key(Key::ArrowDown);
    }
    rig.settle(400);
    let moved = selected_text(&mut rig).expect("a selected row");
    assert_eq!(moved, "App 021");
    let list = rig.rect("list", 0);
    let row = drawn_text(&mut rig, "App 021");
    assert!(
        row.y >= list.y - 1.0 && row.bottom() <= list.bottom() + 1.0,
        "the selection is in view: {row:?} in {list:?}"
    );
    rig.type_text("a");
    rig.settle(400);
    assert_eq!(selected_text(&mut rig).as_deref(), Some("App 001"), "row 1 again");
    let top = drawn_text(&mut rig, "App 001");
    // Row 1 rests where it does unscrolled: the card's 10 px of padding and the 10 px above the text in its row.
    let list_top = rig.rect("list", 0).y;
    assert!(approx(top.y - list_top, 20.0, 0.5), "scrolled to the top: {top:?} in a list from {list_top}");
}

#[test]
fn launcher_arrows_page_keys_home_and_end_move_the_selection_without_wrapping() {
    let mut rig = launcher(many_apps());
    let sel = |r: &mut Rig| selected_text(r).unwrap();
    rig.key(Key::ArrowUp);
    assert_eq!(sel(&mut rig), "App 001", "no wrap above the first row");
    rig.key(Key::ArrowDown);
    rig.key(Key::ArrowDown);
    assert_eq!(sel(&mut rig), "App 003");
    rig.key(Key::PageDown);
    assert_eq!(sel(&mut rig), "App 011", "eight rows down");
    rig.key(Key::PageUp);
    assert_eq!(sel(&mut rig), "App 003");
    rig.key(Key::PageUp);
    assert_eq!(sel(&mut rig), "App 001", "stops at the first row");
    // Home and End keep their text meaning while the search has the focus: they move the caret and the
    // list stays put. The list's own Home and End run when the key reaches the panel with nothing focused.
    rig.key(Key::End);
    assert_eq!(sel(&mut rig), "App 001");
    for _ in 0..200 {
        rig.key(Key::ArrowDown);
        rig.step(40);
    }
    assert_eq!(sel(&mut rig), "App 120", "no wrap below the last row");
}

#[test]
fn launcher_home_and_end_reach_the_list_from_the_panel() {
    // The panel's own key handler takes them when the key is not for an input (here: sent to the
    // script's handler directly, as the scene does when nothing is focused).
    let mut rig = launcher(many_apps());
    let key_handler = rig.content("dashboard").handlers.key.expect("the panel hears keys");
    let g = rig.h.next_gesture();
    let send = |rig: &mut Rig, key: Key| {
        rig.h.send(ScriptIn::Element(ElementCall {
            instance: InstanceId(rig.instance("dashboard")),
            handler: key_handler,
            event: ElementEvent::Key(KeyEvent { key, mods: Modifiers::default(), down: true, repeat: false }),
            gesture: g,
        }));
        rig.sync();
    };
    send(&mut rig, Key::End);
    assert_eq!(selected_text(&mut rig).as_deref(), Some("App 120"));
    send(&mut rig, Key::Home);
    assert_eq!(selected_text(&mut rig).as_deref(), Some("App 001"));
}

#[test]
fn launcher_enter_launches_the_selected_row_and_closes_row_one_when_none_is_picked() {
    let mut rig = launcher(World::Normal.events());
    rig.key(Key::Enter);
    assert_eq!(launches(&rig), vec![launched("app:Alder Notes")], "row 1 when none was picked");
    assert!(!rig.is_open("launcher"), "the launcher closes after the launch");

    let mut rig = launcher(World::Normal.events());
    rig.key(Key::ArrowDown);
    rig.key(Key::ArrowDown);
    rig.key(Key::Enter);
    assert_eq!(launches(&rig), vec![launched("app:Bit Tracker")]);
    assert!(!rig.is_open("launcher"));
}

#[test]
fn launcher_the_pointer_highlights_a_row_without_moving_the_selection_and_a_click_launches_it() {
    let mut rig = launcher(World::Normal.events());
    let fourth = rig.rects("app")[3];
    rig.hover(fourth.x + fourth.w / 2.0, fourth.y + fourth.h / 2.0);
    rig.settle(300);
    assert_eq!(selected_text(&mut rig).as_deref(), Some("Alder Notes"), "hovering does not select");
    // The hovered row is drawn with the hover colour and the selected one keeps its own.
    let draw = rig.frame_pop();
    let fills = fill_colours_at(&draw, center(&fourth));
    assert!(fills.contains(&Color::parse("#32393a").unwrap()), "hover colour #32393a in {fills:?}");
    // Enter still launches the keyboard's row.
    rig.key(Key::Enter);
    assert_eq!(launches(&rig), vec![launched("app:Alder Notes")]);

    let mut rig = launcher(World::Normal.events());
    let fourth = rig.rects("app")[3];
    rig.click(fourth.x + fourth.w / 2.0, fourth.y + fourth.h / 2.0);
    assert_eq!(launches(&rig), vec![launched("app:Maple Mail")], "a click launches that row");
    assert!(!rig.is_open("launcher"));
}

#[test]
fn launcher_escape_closes_and_ctrl_backspace_deletes_a_word() {
    let mut rig = launcher(World::Normal.events());
    rig.type_text("alder zzz");
    assert!(row_texts(&mut rig).is_empty(), "nothing matches both words");
    rig.key_with(Key::Backspace, Modifiers { ctrl: true, ..Modifiers::default() });
    rig.step(80);
    assert_eq!(row_texts(&mut rig), ["Alder Notes"], "the last word is gone, the first still matches");
    rig.key(Key::Escape);
    assert!(!rig.is_open("launcher"), "Escape closes the launcher");
    assert!(launches(&rig).is_empty());
}

#[test]
fn a_dashboard_page_goes_back_on_escape_and_the_tiles_close_on_escape() {
    let mut rig = Rig::new(World::Normal, true);
    rig.open("dashboard", true);
    rig.settle(1500);
    rig.click_node("tile", 1);
    rig.settle(300);
    assert_eq!(rig.rects("tile").len(), 0, "the Bluetooth page is showing");
    rig.key(Key::Escape);
    rig.settle(300);
    assert_eq!(rig.rects("tile").len(), 6, "Escape goes back to the tiles");
    assert!(rig.is_open("dashboard"));
    rig.key(Key::Escape);
    assert!(!rig.is_open("dashboard"), "Escape on the tiles closes the panel");
}

fn fill_colours_at(draw: &DrawList, p: PointF) -> Vec<Color> {
    let mut out = Vec::new();
    for item in &draw.items {
        if let DrawCmd::Fill { rect, fill: Fill::Solid(c), .. } = &item.cmd
            && rect.contains(p)
        {
            out.push(*c);
        }
    }
    out
}

// ---------------------------------------------------------------- numeric spec

fn tiles_of(rig: &mut Rig) -> Vec<RectF> {
    rig.rects("tile")
}

#[test]
fn the_dashboard_panel_and_its_home_follow_the_numeric_spec() {
    let mut rig = Rig::new(World::Normal, true);
    rig.open("dashboard", false);
    rig.settle(1500);
    // The panel: 500 x 500, placed at x 710, y 36 (the centre button's flat edge and its lower edge).
    let root = rig.nodes()[0].rect;
    assert_rect("panel", root, (Some(0.0), Some(0.0), Some(500.0), Some(500.0)), 0.5);
    let placed = rig.pop().rect;
    assert_eq!((placed.x0, placed.y0, placed.x1, placed.y1), (710, 36, 1210, 536));
    let d = rig.decl("dashboard").unwrap();
    assert!(d.props.attach);
    assert_eq!(d.props.names.iter().map(|n| n.to_string()).collect::<Vec<_>>(), ["dashboard", "launcher"]);
    // Top corners square, bottom radius 10.
    let style = rig.nodes()[0].style.clone();
    let radii = style.border_radius;
    assert!(format!("{radii:?}").matches("10").count() >= 2, "{radii:?}");

    // Six tiles of 80 x 80 on a pitch of 110, the grid centred; icons 42 px.
    let t = tiles_of(&mut rig);
    assert_eq!(t.len(), 6);
    for (i, r) in t.iter().enumerate() {
        let (col, row) = ((i % 3) as f32, (i / 3) as f32);
        assert_rect(
            &format!("tile {i}"),
            *r,
            (Some(100.0 + 110.0 * col), Some(155.0 + 110.0 * row), Some(80.0), Some(80.0)),
            2.0,
        );
    }
    let icons: Vec<_> = rig.nodes().into_iter().filter(|n| n.kind == ElementKind::Icon).collect();
    assert_eq!(icons.len(), 6);
    // The glyph's ink is 42 px on the reference; the box is larger because a glyph does not fill its box (the
    // video glyph is wider than tall, so the last row's first tile has a smaller box).
    assert!(
        icons.iter().enumerate().all(|(i, n)| {
            let want = if i == 3 { 45.0 } else { 50.0 };
            approx(n.rect.w, want, 1.0) && approx(n.rect.h, want, 1.0)
        }),
        "{:?}",
        icons.iter().map(|n| n.rect).collect::<Vec<_>>()
    );
}

#[test]
fn the_pages_follow_the_numeric_spec_header_card_list_card_rows_and_inputs() {
    // Bluetooth: header card 480 x 37, a 376 x 27 search, a 28 x 28 refresh button, rows 460 x 38 on a pitch of 43.
    let mut rig = Rig::new(World::Normal, true);
    rig.open("dashboard", false);
    rig.settle(1500);
    rig.click_node("tile", 1);
    rig.settle(600);
    assert_rect("header card", rig.rect("head", 0), (Some(10.0), Some(10.0), Some(480.0), Some(37.0)), 1.0);
    let list = rig.rect("list", 0);
    assert_rect("list card", list, (Some(10.0), Some(57.0), Some(480.0), Some(433.0)), 1.0);
    assert!(approx(list.bottom(), 490.0, 1.0), "the list card ends 10 px above the bottom: {}", list.bottom());
    let search = rig.rect("search", 0);
    assert_rect("search", search, (None, None, Some(376.0), Some(27.0)), 1.0);
    assert!(approx(search.x, 62.0, 2.0), "the search starts 52 px into the card: {search:?}");
    assert_rect("refresh", rig.rect("icon-btn", 1), (None, None, Some(28.0), Some(28.0)), 1.0);
    let rows = rig.rects("device");
    assert_eq!(rows.len(), 4);
    for (i, r) in rows.iter().enumerate() {
        assert_rect(&format!("row {i}"), *r, (Some(20.0), Some(67.0 + 43.0 * i as f32), Some(460.0), Some(38.0)), 1.5);
    }
    // The row buttons: 16 px icons centred about 65 and 29 px from the row's right edge.
    let icons: Vec<_> = rig.nodes().into_iter().filter(|n| n.kind == ElementKind::Icon).collect();
    let row0 = rows[0];
    let right_icons: Vec<f32> = icons
        .iter()
        .filter(|n| n.rect.y >= row0.y && n.rect.bottom() <= row0.bottom())
        .map(|n| row0.right() - (n.rect.x + n.rect.w / 2.0))
        .collect();
    assert!(right_icons.iter().any(|d| approx(*d, 29.0, 3.0)), "forget icon: {right_icons:?}");
    assert!(right_icons.iter().any(|d| approx(*d, 65.0, 3.0)), "connect icon: {right_icons:?}");

    // Apps: a 460 x 27 search, rows of 460 x 38.
    let mut rig = Rig::new(World::Normal, true);
    rig.open("launcher", true);
    rig.settle(1500);
    assert_rect("apps search", rig.rect("search", 0), (Some(20.0), Some(15.0), Some(460.0), Some(27.0)), 1.5);
    let rows = rig.rects("app");
    for (i, r) in rows.iter().enumerate() {
        assert_rect(
            &format!("app row {i}"),
            *r,
            (Some(20.0), Some(67.0 + 43.0 * i as f32), Some(460.0), Some(38.0)),
            1.5,
        );
    }

    // Audio: icon-only header, every row 460 x 38 with the name cut at 302.
    let mut rig = Rig::new(World::Normal, true);
    rig.open("dashboard", false);
    rig.settle(1500);
    rig.click_node("tile", 2);
    rig.settle(600);
    assert_rect("audio header", rig.rect("head", 0), (Some(10.0), Some(10.0), Some(480.0), Some(37.0)), 1.0);
    let rows = rig.rects("output");
    assert_eq!(rows.len(), 5);
    for (i, r) in rows.iter().enumerate() {
        assert_rect(
            &format!("audio row {i}"),
            *r,
            (Some(20.0), Some(67.0 + 43.0 * i as f32), Some(460.0), Some(38.0)),
            1.5,
        );
    }
    let names: Vec<_> = rig
        .nodes()
        .into_iter()
        .filter(|n| n.kind == ElementKind::Text && n.text.as_deref().is_some_and(|t| t.starts_with("Studio")))
        .collect();
    assert_eq!(names.len(), 2);
    // A name takes what the lead button, the level and the mute button leave of the 460 px row; the whole
    // name fits here, and in the 340 px mixer it is cut (see the mixer test).
    assert!(
        names.iter().all(|n| approx(n.rect.w, 340.0, 3.0)),
        "{:?}",
        names.iter().map(|n| (n.rect.w, n.text.clone())).collect::<Vec<_>>()
    );
    // The fill is the level: the last row is at 35 %.
    let draw = rig.frame_pop();
    let last = rows[4];
    let fills: Vec<RectF> = draw
        .items
        .iter()
        .filter_map(|i| match &i.cmd {
            DrawCmd::Fill { rect, fill: Fill::Solid(c), .. }
                if c.a > 0.05 && c.a < 0.12 && rect.y >= last.y - 0.5 && rect.bottom() <= last.bottom() + 0.5 =>
            {
                Some(*rect)
            }
            _ => None,
        })
        .collect();
    assert_eq!(fills.len(), 1, "one faint fill on the last row");
    assert!(approx(fills[0].w, 460.0 * 0.35, 3.0), "{:?}", fills[0]);

    // Power mode: three rows.
    let mut rig = Rig::new(World::Normal, true);
    rig.open("dashboard", false);
    rig.settle(1500);
    rig.click_node("tile", 0);
    rig.settle(600);
    assert_eq!(rig.rects("mode").len(), 3);
}

#[test]
fn the_calendar_follows_the_numeric_spec() {
    let mut rig = Rig::with_extra(World::Normal, true, &[]);
    rig.click_bar("date");
    rig.settle(600);
    assert_rect("calendar", rig.nodes()[0].rect, (None, None, Some(300.0), Some(318.0)), 0.5);
    assert_rect("month header", rig.rect("head", 0), (Some(10.0), Some(10.0), Some(280.0), Some(37.0)), 1.0);
    let wd = rig.rects("weekday");
    assert_eq!(wd.len(), 7);
    assert!(wd.iter().all(|r| approx(r.h, 24.0, 1.0) || approx(r.h, 24.0, 12.0)));
    let weeks = rig.rects("week");
    assert_eq!(weeks.len(), 6, "six week rows");
    for (i, w) in weeks.iter().enumerate() {
        assert!(approx(w.h, 36.0, 0.5), "week {i}: {w:?}");
        if i > 0 {
            assert!(approx(w.y - weeks[i - 1].y, 36.0, 0.5));
        }
    }
    let days = rig.rects("day");
    assert_eq!(days.len(), 42);
    for c in 1..7 {
        assert!(approx(days[c].x - days[c - 1].x, 40.0, 0.5), "column pitch {}", days[c].x - days[c - 1].x);
    }
    let placed = rig.pop().rect;
    assert!(placed.x0 >= 0 && placed.x1 <= 1920 && placed.y0 == 36, "{placed:?}");
    let d = rig.decl("calendar").unwrap();
    assert_eq!((d.props.placement, d.props.attach), (Placement::Below, false));

    // The wheel and the two arrow buttons change the month: a notch up goes back, a notch down goes on.
    let body = rig.rect("weeks", 0);
    let at = center(&body);
    rig.wheel(at.x, at.y, 1.0);
    rig.settle(200);
    assert_has(&rig.texts(), "October 2026", "wheel up");
    rig.click_node("icon-btn", 1);
    rig.click_node("icon-btn", 1);
    rig.settle(200);
    assert_has(&rig.texts(), "December 2026", "next, next");
    rig.wheel(at.x, at.y, -1.0);
    rig.settle(200);
    assert_has(&rig.texts(), "January 2027", "wheel down over a year end");
    rig.click_node("icon-btn", 0);
    rig.settle(200);
    assert_has(&rig.texts(), "December 2026", "previous");
    // Today is marked only in its own month.
    assert!(rig.rects("today").is_empty(), "no today mark in December");
}

#[test]
fn the_player_the_mixer_and_the_wifi_list_follow_the_numeric_spec() {
    let mut rig = Rig::with_extra(World::Normal, true, &["volume", "network"]);
    rig.click_bar("media");
    rig.settle(600);
    assert_rect("player", rig.nodes()[0].rect, (None, None, Some(380.0), Some(160.0)), 0.5);
    assert_rect("cover", rig.rect("art", 0), (Some(10.0), Some(10.0), Some(140.0), Some(140.0)), 0.5);
    let transport = rig.rects("transport");
    assert_eq!(transport.len(), 3);
    assert!(transport.iter().all(|r| approx(r.w, 32.0, 0.5) && approx(r.h, 32.0, 0.5)), "{transport:?}");
    assert_rect("seek", rig.rect("seek", 0), (None, None, None, Some(6.0)), 0.5);

    let mut events = World::Normal.events();
    let many: Vec<Out> = (1..=8)
        .map(|i| Out {
            id: Box::leak(format!("out-{i}").into_boxed_str()),
            name: Box::leak(format!("Made-up output {i}").into_boxed_str()),
            default: i == 1,
            volume: 10.0 * i as f64,
            muted: false,
            bluetooth: false,
        })
        .collect();
    events.push(ok(ServiceId::Audio, audio_value(&many, true)));
    let mut rig = Rig::with_events(events, true, &["volume", "network"]);
    rig.click_bar("volume");
    rig.settle(600);
    assert_rect("mixer", rig.nodes()[0].rect, (None, None, Some(380.0), None), 0.5);
    let master = rig.rect("master", 0);
    assert_rect("master row", master, (Some(10.0), Some(10.0), Some(360.0), Some(38.0)), 1.0);
    let list = rig.rect("list", 0);
    assert!(approx(list.h, 273.0, 1.5), "six rows before it scrolls: {list:?}");
    let rows = rig.rects("output");
    assert_eq!(rows.len(), 9, "the master row and eight devices");
    for i in 2..6 {
        assert!(approx(rows[i].y - rows[i - 1].y, 43.0, 0.5));
    }
    assert!(list.bottom() - rows[7].y <= 6.0, "only a sliver of the seventh device shows: {:?} {list:?}", rows[7]);

    let mut rig = Rig::with_extra(World::Normal, true, &["volume", "network"]);
    rig.click_bar("network");
    rig.settle(600);
    assert_rect("wifi", rig.nodes()[0].rect, (None, None, Some(380.0), None), 0.5);
    assert_rect("wifi header", rig.rect("head", 0), (Some(10.0), Some(10.0), Some(360.0), Some(37.0)), 1.0);
    let rows = rig.rects("net");
    assert_eq!(rows.len(), 4);
    for i in 1..4 {
        assert!(approx(rows[i].y - rows[i - 1].y, 43.0, 0.5));
    }
}

// ---------------------------------------------------------------- the power menu

fn fills_of(dark: bool) -> [Color; 4] {
    let hex =
        if dark { ["#2f3639", "#1b1f22", "#171b1f", "#090f12"] } else { ["#f1f4f6", "#e4e9ec", "#dce2e5", "#d5dbdf"] };
    hex.map(|h| Color::parse(h).unwrap())
}

#[test]
fn the_power_menu_has_four_chevrons_300_by_150_on_a_pitch_of_225_centred_on_the_monitor() {
    // The dim over the wallpaper: flat, half opaque, in the theme's own colour.
    for (dark, want) in [(true, (9u8, 15u8, 18u8)), (false, (240, 243, 245))] {
        let mut rig = Rig::new(World::Normal, dark);
        rig.open("power", false);
        rig.settle(2000);
        let dim = rig.pop().scene.overlay_dim().expect("an overlay has a dim");
        let got = ((dim.r * 255.0).round() as u8, (dim.g * 255.0).round() as u8, (dim.b * 255.0).round() as u8);
        assert!(approx(dim.a, 0.5, 0.01) && got == want, "dim {dim:?} in {}", if dark { "dark" } else { "light" });
    }
    let mut rig = Rig::new(World::Normal, true);
    rig.open("power", false);
    rig.settle(2000);
    let b = rig.rects("power-btn");
    assert_eq!(b.len(), 4);
    for (i, r) in b.iter().enumerate() {
        assert!(approx(r.w, 300.0 + if i < 3 { 1.5 } else { 0.0 }, 1.0), "button {i} is {} wide", r.w);
        assert!(approx(r.h, 150.0, 0.5));
        if i > 0 {
            assert!(approx(r.x - b[i - 1].x, 225.0, 1.5), "pitch {}", r.x - b[i - 1].x);
        }
    }
    // The row is centred on the monitor: the surface is the row, the root is the monitor.
    let root = rig.nodes()[0].rect;
    assert!(approx(root.w, 1920.0, 0.5) && approx(root.h, 1200.0, 0.5), "the root fills the monitor: {root:?}");
    let row_left = b[0].x;
    let row_right = b[3].right();
    assert!(
        approx((row_left + row_right) / 2.0 - (root.x + root.w / 2.0), 0.0, 1.0),
        "centred: {row_left} {row_right} {root:?}"
    );
    assert!(approx(row_right - row_left, 975.0, 1.0), "975 wide in all");
    let d = rig.decl("power").unwrap();
    assert_eq!(d.props.placement, Placement::Screen);
    assert_eq!(d.props.names.iter().map(|n| n.to_string()).collect::<Vec<_>>(), ["power"]);
    // The surface is only the bounds of what is drawn: about 975 x 150.
    let size = rig.natural();
    assert!(approx(size.w, 975.0, 2.0) && approx(size.h, 150.0, 1.0), "{size:?}");
    // Each glyph fills its box differently; the boxes are sized so the drawn ink is 49 to 63 px, centred in
    // its button. The hover tile is 120 x 120.
    let icons: Vec<_> = rig.nodes().into_iter().filter(|n| n.kind == ElementKind::Icon).collect();
    assert_eq!(icons.len(), 4);
    for (icon, (size, button)) in icons.iter().zip([82.0, 68.0, 73.0, 76.0].into_iter().zip(&b)) {
        assert!(approx(icon.rect.w, size, 1.0) && approx(icon.rect.h, size, 1.0), "{:?}", icon.rect);
        let (c, bc) = (center(&icon.rect), center(button));
        assert!(approx(c.x, bc.x, 3.0) && approx(c.y, bc.y, 1.0), "{c:?} {bc:?}");
    }
    assert!(rig.rects("tile").iter().all(|r| approx(r.w, 120.0, 0.5) && approx(r.h, 120.0, 0.5)));
}

fn polygon_points(path: &rsb_core::geom::PathF) -> Vec<PointF> {
    path.els
        .iter()
        .filter_map(|e| match e {
            rsb_core::geom::PathEl::MoveTo(p) | rsb_core::geom::PathEl::LineTo(p) => Some(*p),
            _ => None,
        })
        .collect()
}

#[test]
fn the_power_chevrons_are_a_hexagon_then_three_notched_points_and_join_with_no_gap() {
    for dark in [true, false] {
        let mut rig = Rig::new(World::Normal, dark);
        rig.open("power", false);
        rig.settle(2000);
        let draw = rig.frame_pop();
        // Each button's background is a fill inside the clip of its outline.
        let mut shapes: Vec<(Vec<PointF>, Color)> = Vec::new();
        let mut clip: Option<Vec<PointF>> = None;
        for item in &draw.items {
            match &item.cmd {
                DrawCmd::PushClipPath { path, .. } => clip = Some(polygon_points(path)),
                DrawCmd::Fill { fill: Fill::Solid(c), .. } if clip.is_some() => shapes.push((clip.take().unwrap(), *c)),
                _ => {}
            }
        }
        assert_eq!(shapes.len(), 4, "four filled chevrons");
        assert_eq!(shapes[0].0.len(), 6, "the first is a hexagon");
        for (i, (pts, _)) in shapes.iter().enumerate().skip(1) {
            assert_eq!(pts.len(), 6, "button {i}: notch left, point right");
        }
        let fills = fills_of(dark);
        for i in 0..4 {
            assert_eq!(shapes[i].1, fills[i], "fill {i}");
        }
        // The depth of every point is 75 px over a height of 150: 45 degrees.
        let first = &shapes[0].0;
        let xs: Vec<f32> = first.iter().map(|p| p.x).collect();
        let (x0, x1) = (xs.iter().cloned().fold(f32::MAX, f32::min), xs.iter().cloned().fold(f32::MIN, f32::max));
        assert!(approx(x1 - x0, 301.5, 1.0), "{x0}..{x1}");
        assert!(
            first
                .iter()
                .any(|p| approx(p.y, 75.0 + first.iter().map(|q| q.y).fold(f32::MAX, f32::min), 0.6)
                    && approx(p.x, x0, 0.6)),
            "a left point at mid height: {first:?}"
        );

        // Joints: every pixel on the joint line is a blend of the two fills and opaque.
        let canvas = render_offscreen(&mut rig.r, &draw).unwrap();
        let base = rig.rects("power-btn")[0];
        let (ox, oy) = (base.x.round() as i32, base.y.round() as i32);
        let mut worst = 0.0f32;
        let mut seen = 0;
        for j in 0..3 {
            let (a, bcol) = (fills[j], fills[j + 1]);
            let joint_x = 225 * (j as i32 + 1);
            for t in (3..72).step_by(3) {
                for y in [t, 150 - t] {
                    let px = canvas.rgba_at((ox + joint_x + t) as u32, (oy + y) as u32);
                    assert!(px[3] >= 250, "joint {j} at {t}: alpha {}", px[3]);
                    let want = |c: f32| (c * 255.0).round();
                    let mut best = f32::MAX;
                    for k in 0..=50 {
                        let m = k as f32 / 50.0;
                        let err = [(a.r, bcol.r), (a.g, bcol.g), (a.b, bcol.b)]
                            .iter()
                            .zip(px.iter())
                            .map(|((p, q), got)| (want(p * (1.0 - m) + q * m) - *got as f32).abs())
                            .fold(0.0, f32::max);
                        best = best.min(err);
                    }
                    worst = worst.max(best);
                    seen += 1;
                }
            }
        }
        assert!(seen > 60);
        assert!(
            worst <= 8.0,
            "{}: a joint pixel is {worst} levels from a blend of its two fills",
            if dark { "dark" } else { "light" }
        );
    }
}

/// Button widths every 16 ms from the first frame.
fn stagger_widths(rig: &mut Rig, until_ms: u64) -> Vec<(u64, Vec<f32>)> {
    let mut out = Vec::new();
    let t0 = rig.ms;
    loop {
        let t = rig.ms - t0;
        let w: Vec<f32> = rig.rects("power-btn").iter().map(|r| r.w).collect();
        out.push((t, w));
        if t >= until_ms {
            break;
        }
        rig.step(16);
    }
    out
}

#[test]
fn the_power_buttons_open_one_after_another_240_ms_apart_in_150_ms_each() {
    let mut rig = Rig::new(World::Normal, true);
    rig.open("power", false);
    let series = stagger_widths(&mut rig, 1300);
    let width_at = |t: u64, i: usize| -> f32 { series.iter().min_by_key(|(s, _)| s.abs_diff(t)).unwrap().1[i] };
    for i in 0..4usize {
        let start = 240 * i as u64;
        // Collapsed to the tips (150 + the joint's 1.5) until its turn.
        if i > 0 {
            assert!(
                width_at(start.saturating_sub(32), i) <= 152.0,
                "button {i} waits: {}",
                width_at(start.saturating_sub(32), i)
            );
        }
        // Full 300 px by 240 i + 200 ms.
        assert!(
            width_at(start + 200, i) >= 299.0,
            "button {i} is full by {} ms: {}",
            start + 200,
            width_at(start + 200, i)
        );
        // Half way on an ease-out is past half at 40 % of the time.
        let mid = width_at(start + 60, i);
        assert!(mid > 150.0 + 0.5 * 150.0 * 0.5 && mid < 300.0, "button {i} at +60 ms: {mid}");
    }
    // The dim comes first: 200 ms ease-out.
    let mut rig = Rig::new(World::Normal, true);
    rig.open("power", false);
    let a0 = rig.pop().scene.overlay_dim().map_or(0.0, |c| c.a);
    rig.step(100);
    let a100 = rig.pop().scene.overlay_dim().map_or(0.0, |c| c.a);
    rig.step(150);
    let a250 = rig.pop().scene.overlay_dim().map_or(0.0, |c| c.a);
    assert!(a0 < 0.05, "{a0}");
    assert!(a100 > 0.25 && a100 < 0.5, "{a100}");
    assert!(approx(a250, 0.5, 0.01), "the dim ends at 0.50: {a250}");
    let dim = rig.pop().scene.overlay_dim().unwrap();
    assert_eq!((dim.r * 255.0).round() as u8, 9);
    assert_eq!((dim.g * 255.0).round() as u8, 15);
    assert_eq!((dim.b * 255.0).round() as u8, 18);
}

#[test]
fn lock_has_the_focus_the_menu_ignores_input_until_it_has_settled_and_lock_runs_a_curtain_first() {
    // Opened from the keyboard: Lock is focused and shows it.
    let mut rig = Rig::new(World::Normal, true);
    rig.open("power", true);
    rig.settle(1500);
    let lock = rig.rects("power-btn")[3];
    let draw = rig.frame_pop();
    let tile = rig.rects("tile")[3];
    let at = center(&tile);
    assert!(fill_colours_at(&draw, at).contains(&Color::parse("#1e2427").unwrap()), "the focused tile is drawn");
    for other in 0..3 {
        let t = rig.rects("tile")[other];
        assert!(
            !fill_colours_at(&draw, center(&t)).contains(&Color::parse("#1e2427").unwrap()),
            "only Lock is focused"
        );
    }
    let _ = lock;
    // A pointer open focuses Lock too but draws nothing for it.
    let mut rig = Rig::new(World::Normal, true);
    rig.open("power", false);
    rig.settle(1500);
    let draw = rig.frame_pop();
    let tile = rig.rects("tile")[3];
    assert!(!fill_colours_at(&draw, center(&tile)).contains(&Color::parse("#1e2427").unwrap()));

    // Enter in the first 400 ms does nothing; Enter later starts the curtain.
    let mut rig = Rig::new(World::Normal, true);
    rig.open("power", true);
    rig.step(100);
    rig.key(Key::Enter);
    rig.step(400);
    assert!(rig.kinds().is_empty(), "the guard ignored the early key: {:?}", rig.kinds());
    rig.settle(1500);
    assert!(!rig.classes_of_root().contains(&"locking".to_string()));
    rig.key(Key::Enter);
    assert!(
        rig.classes_of_root().contains(&"locking".to_string()),
        "the root takes class locking: {:?}",
        rig.classes_of_root()
    );
    assert!(rig.kinds().is_empty(), "the curtain runs before the command");
    rig.step(100);
    assert!(rig.kinds().is_empty(), "150 ms have not passed: {:?}", rig.kinds());
    rig.step(100);
    assert_eq!(rig.kinds(), ["lock"], "then Windows is asked to lock");
    // The dim went to the opaque curtain colour.
    rig.settle(400);
    let dim = rig.pop().scene.overlay_dim().unwrap();
    assert!(approx(dim.a, 1.0, 0.01), "{dim:?}");
    assert_eq!(
        ((dim.r * 255.0).round() as u8, (dim.g * 255.0).round() as u8, (dim.b * 255.0).round() as u8),
        (9, 15, 18)
    );
    // The menu itself faded out.
    let draw = rig.frame_pop();
    assert!(draw.items.iter().any(|i| matches!(i.cmd, DrawCmd::PushOpacity(a) if a < 0.05)), "the buttons are faded");
    // The overlay closing (the session locked) ends the class.
    rig.ui_closes(CloseReason::Deactivated);
    rig.step(50);
    assert!(!rig.is_open("power"));
}

#[test]
fn the_power_actions_run_from_a_click_after_the_guard_and_only_from_a_gesture() {
    let click_button = |rig: &mut Rig, i: usize| {
        let r = rig.rects("power-btn")[i];
        rig.click(r.x + r.w * 0.45, r.y + r.h / 2.0);
    };
    // Shut down, restart, sleep: one command each, at the right button.
    for (i, want) in [(0usize, "shut_down"), (1, "restart"), (2, "sleep")] {
        let mut rig = Rig::new(World::Normal, true);
        rig.open("power", false);
        // A click while the menu is still opening is ignored.
        rig.step(100);
        click_button(&mut rig, i);
        assert!(rig.kinds().is_empty(), "{want} before the guard lifted: {:?}", rig.kinds());
        rig.settle(1500);
        click_button(&mut rig, i);
        assert_eq!(rig.kinds(), [want], "button {i}");
        if want == "sleep" {
            assert!(!rig.is_open("power"), "sleep leaves the menu");
        } else {
            assert!(rig.is_open("power"), "{want} stays up so a refusal can be shown");
        }
    }

    // A held Enter does not fire: only a fresh key press does (the scene drops repeats on an overlay).
    let mut rig = Rig::new(World::Normal, true);
    rig.open("power", true);
    rig.settle(1500);
    let now = rig.now();
    let ev = KeyEvent { key: Key::Enter, mods: Modifiers::default(), down: true, repeat: true };
    let res = rig.pop().scene.key(ev, now);
    assert!(res.calls.is_empty());

    // The same actions without a gesture are refused with the exact message and reach no service.
    for (i, method) in [(0usize, "shut_down"), (1, "restart"), (2, "sleep")] {
        let mut rig = Rig::new(World::Normal, true);
        rig.open("power", false);
        rig.settle(1500);
        rig.click_element("power", "power-btn", i, 0);
        assert!(rig.kinds().is_empty(), "{method} without a gesture reached the service: {:?}", rig.kinds());
        assert!(rig.log_has(&format!("power:{method} only works inside a click or key handler")), "{method}");
    }
}

#[test]
fn an_overlay_does_not_open_without_a_click_or_key_and_a_failed_action_shows_the_services_reason() {
    let mut rig = Rig::new(World::Normal, true);
    let g = 0;
    rig.h.send(ScriptIn::Popup { name: "power".into(), action: PopupAction::Open, monitor: Some(1), gesture: g });
    rig.push_trees();
    assert!(!rig.is_open("power"), "an overlay opens only from a gesture");
    assert!(rig.log_has("an overlay only opens from a click or a key"));

    let mut rig = Rig::new(World::Normal, true);
    rig.open("power", false);
    rig.settle(1500);
    let shown = |rig: &mut Rig| draw_texts(&rig.frame_pop());
    assert!(!shown(&mut rig).iter().any(|t| t.contains("Windows said no")));
    rig.h.send(svc(ServiceId::Power, ServiceStatus::Error, None, Some("Windows said no")));
    rig.sync();
    rig.settle(300);
    assert!(shown(&mut rig).iter().any(|t| t.contains("Windows said no")), "{:?}", shown(&mut rig));
    // The next value clears it.
    rig.h.send(ok(ServiceId::Power, power_value("balanced", true, true)));
    rig.sync();
    rig.settle(300);
    assert!(!shown(&mut rig).iter().any(|t| t.contains("Windows said no")));
}

/// Where a text is drawn, with the scroll offset of its containers applied (a node's own rectangle is
/// where layout put it, before any scrolling).
fn drawn_text(rig: &mut Rig, what: &str) -> RectF {
    let draw = rig.frame_pop();
    draw.items
        .iter()
        .find_map(|i| match &i.cmd {
            DrawCmd::Text { rect, text, .. } if &**text == what => Some(*rect),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{what} is not drawn"))
}

fn draw_texts(draw: &DrawList) -> Vec<String> {
    draw.items
        .iter()
        .filter_map(|i| match &i.cmd {
            DrawCmd::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------- every surface

const SURFACES: [&str; 10] =
    ["home", "power-mode", "bluetooth", "audio", "apps", "calendar", "media", "volume", "network", "power"];

/// One surface open and settled: the dashboard home, one of its pages, a secondary popup or the power
/// menu. None when the world has nothing to open (no player, so no player popup).
fn open_surface(name: &str, world: World, dark: bool) -> Option<Rig> {
    let page = ["power-mode", "bluetooth", "audio", "apps"].iter().position(|p| *p == name);
    let mut rig = match name {
        "calendar" | "media" | "volume" | "network" => Rig::with_extra(world, dark, &["volume", "network"]),
        _ => Rig::new(world, dark),
    };
    let popup = match name {
        "home" | "power-mode" | "bluetooth" | "audio" | "apps" => "dashboard",
        other => other,
    };
    rig.decl(popup)?;
    rig.open(popup, false);
    rig.settle(1500);
    if let Some(i) = page {
        rig.click_node("tile", i);
        rig.settle(1500);
    }
    Some(rig)
}

/// Contrast of one draw list: every text item against the fills drawn under it (4.5:1) and every icon
/// (3:1); translucent fills are composited over black and over white and the worse of the two counts.
/// Items in a fully transparent group are not on screen; a group half way is refused (settle first).
/// Restricted to items inside `within`. Returns (what, ratio, ratio needed).
fn contrast_of(draw: &DrawList, within: Option<RectF>) -> Vec<(String, f32, f32)> {
    rsb_core::contrast::of_draw_list(draw, within)
        .unwrap_or_else(|e| panic!("{e}"))
        .into_iter()
        .map(|c| (c.what, c.ratio, c.need))
        .collect()
}

/// Worst contrast seen, scaled so that 4.5 is the pass mark for text and icons alike.
#[derive(Default)]
struct Worst {
    ratio: Option<(f32, String)>,
    count: usize,
}

impl Worst {
    fn take(&mut self, what: &str, rows: Vec<(String, f32, f32)>) {
        for (label, ratio, need) in rows {
            self.count += 1;
            let scaled = ratio * 4.5 / need;
            if self.ratio.as_ref().is_none_or(|(w, _)| scaled < *w) {
                self.ratio = Some((scaled, format!("{what}: {label:?} {ratio:.2}:1 of {need}")));
            }
        }
    }
}

const INTERACTIVE: [&str; 6] = ["tile", "row", "icon-btn", "power-btn", "search", "track"];

#[test]
fn contrast_holds_on_every_panel_in_every_state_dark_and_light() {
    let mut worst = Worst::default();
    let mut report = Vec::new();
    for dark in [true, false] {
        let theme = if dark { "dark" } else { "light" };
        let before = worst.count;
        for world in World::ALL {
            for name in SURFACES {
                let Some(mut rig) = open_surface(name, world, dark) else { continue };
                let what = format!("{name} / {} / {theme}", world.name());
                let draw = rig.frame_pop();
                worst.take(&what, contrast_of(&draw, None));
                if world != World::Normal {
                    continue;
                }
                // Hover and pressed, one node of each interactive class at a time.
                let nodes = rig.nodes();
                let targets: Vec<RectF> =
                    nodes.iter().filter(|n| INTERACTIVE.iter().any(|c| has_class(n, c))).map(|n| n.rect).collect();
                for r in targets {
                    let c = center(&r);
                    rig.hover(c.x, c.y);
                    rig.settle(300);
                    let draw = rig.frame_pop();
                    worst.take(&format!("{what} hover"), contrast_of(&draw, Some(r)));
                    rig.pointer(PointerKind::Down(Button::Left), c.x, c.y);
                    rig.settle(300);
                    let draw = rig.frame_pop();
                    worst.take(&format!("{what} pressed"), contrast_of(&draw, Some(r)));
                    rig.pointer(PointerKind::Up(Button::Left), -50.0, -50.0);
                    rig.pointer_away();
                    rig.settle(300);
                }
                // Keyboard focus walks every focusable element once.
                let mut rig = open_surface(name, world, dark).expect("the same surface opens again");
                for _ in 0..14 {
                    rig.tab();
                    rig.settle(300);
                    let draw = rig.frame_pop();
                    worst.take(&format!("{what} focus"), contrast_of(&draw, None));
                }
            }
        }
        report.push(format!("{theme}: {} text and icon items measured", worst.count - before));
    }
    let (ratio, at) = worst.ratio.clone().expect("something was measured");
    eprintln!("{}\nlowest {ratio:.2} (pass mark 4.5): {at}", report.join("\n"));
    assert!(worst.count > 1500, "only {} items measured", worst.count);
    assert!(ratio >= 4.5, "{ratio:.2}: {at}");
}

/// The ratio the default preset's own colours promise for a pair of its `[about.contrast]` lists
/// and the one measured on every drawn panel for an item of the same colour on the same fills are
/// worked out by different routes; they have to agree.
#[test]
fn token_and_draw_list_contrast_agree_on_every_panel() {
    use rsb_core::contrast;
    use rsb_core::preset::{Scheme, parse_about};
    let toml = std::fs::read_to_string(assets().join("presets").join("default.toml")).unwrap();
    let about = parse_about(&toml, "default.toml").unwrap();
    let sheet = sheet_for("default");
    let mut report = Vec::new();
    for dark in [true, false] {
        let scheme = if dark { Scheme::Dark } else { Scheme::Light };
        let vars = contrast::resolve_vars(&sheet, scheme);
        let mut seen = std::collections::BTreeSet::new();
        let (mut widest, mut compared) = (0.0f64, 0usize);
        for world in World::ALL {
            for name in SURFACES {
                let Some(mut rig) = open_surface(name, world, dark) else { continue };
                let draw = rig.frame_pop();
                let items = contrast::of_draw_list(&draw, None).unwrap_or_else(|e| panic!("{name}: {e}"));
                for a in contrast::compare_with_draw(&about, &vars, &items) {
                    let gap = (a.token - f64::from(a.drawn)).abs();
                    assert!(
                        gap <= 0.05,
                        "{} {name} {} {:?}: sheet {:.3}, drawn {:.3}",
                        scheme.name(),
                        a.pair,
                        a.item,
                        a.token,
                        a.drawn
                    );
                    widest = widest.max(gap);
                    compared += 1;
                    seen.insert(a.pair);
                }
            }
        }
        report.push(format!(
            "{:5} {:3} of {} pairs seen on the panels, {compared:4} items compared, widest gap {widest:.4}",
            scheme.name(),
            seen.len(),
            contrast::pairs(&about, &vars).len()
        ));
        assert!(seen.len() >= 20, "{}: only {} pairs seen: {seen:?}", scheme.name(), seen.len());
    }
    eprintln!(
        "{}",
        report.join(
            "
"
        )
    );
}

// ---------------------------------------------------------------- no one-edge accent stripes

/// A border on one side or two opposite sides only is a stripe; so is a thin accent-coloured fill that sits
/// against one edge of a bigger one.
fn stripes(cmds: &[DrawCmd], accents: &[Color]) -> Vec<String> {
    let mut found = Vec::new();
    let fills: Vec<(RectF, Fill)> = cmds
        .iter()
        .filter_map(|c| match c {
            DrawCmd::Fill { rect, fill, .. } => Some((*rect, fill.clone())),
            _ => None,
        })
        .collect();
    for cmd in cmds {
        match cmd {
            DrawCmd::Border { rect, widths, .. } => {
                let sides = [widths.top, widths.right, widths.bottom, widths.left];
                let drawn = sides.iter().filter(|w| **w > 0.0).count();
                if drawn != 0 && drawn != 4 {
                    found.push(format!("border on {drawn} side(s) of {}", rect_text(rect)));
                }
            }
            DrawCmd::Fill { rect, fill: Fill::Solid(c), .. }
                if accents
                    .iter()
                    .any(|a| (a.r - c.r).abs() < 0.01 && (a.g - c.g).abs() < 0.01 && (a.b - c.b).abs() < 0.01) =>
            {
                let (thin, long) = if rect.w < rect.h { (rect.w, rect.h) } else { (rect.h, rect.w) };
                if thin > 5.0 || long < 3.0 * thin {
                    continue;
                }
                // Against a bigger fill on exactly one side.
                for (host, _) in &fills {
                    if host.w <= rect.w + 0.5 && host.h <= rect.h + 0.5 {
                        continue;
                    }
                    let flush = [
                        (rect.x - host.x).abs() < 0.6 && rect.h > host.h * 0.5,
                        (rect.right() - host.right()).abs() < 0.6 && rect.h > host.h * 0.5,
                        (rect.y - host.y).abs() < 0.6 && rect.w > host.w * 0.5,
                        (rect.bottom() - host.bottom()).abs() < 0.6 && rect.w > host.w * 0.5,
                    ];
                    let inside = rect.x >= host.x - 0.6
                        && rect.right() <= host.right() + 0.6
                        && rect.y >= host.y - 0.6
                        && rect.bottom() <= host.bottom() + 0.6;
                    if inside && flush.iter().filter(|f| **f).count() == 1 {
                        found.push(format!(
                            "thin accent fill {} against one edge of {}",
                            rect_text(rect),
                            rect_text(host)
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    found
}

#[test]
fn no_panel_draws_an_accent_stripe_on_one_edge_in_any_state() {
    let accents: Vec<Color> =
        ["#81cbea", "#13648c", "#0f5a80", "#095470"].iter().map(|h| Color::parse(h).unwrap()).collect();
    // The checker finds what it is built to find.
    let probe = [DrawCmd::Border {
        rect: RectF { x: 0.0, y: 0.0, w: 100.0, h: 40.0 },
        radii: Default::default(),
        widths: rsb_core::geom::Edges { top: 0.0, right: 0.0, bottom: 0.0, left: 4.0 },
        color: accents[0],
    }];
    assert_eq!(stripes(&probe, &accents).len(), 1, "a left-only border is a stripe");
    let probe = [
        DrawCmd::Fill {
            rect: RectF { x: 0.0, y: 0.0, w: 100.0, h: 40.0 },
            radii: Default::default(),
            fill: Fill::Solid(Color::parse("#252b2e").unwrap()),
        },
        DrawCmd::Fill {
            rect: RectF { x: 0.0, y: 0.0, w: 4.0, h: 40.0 },
            radii: Default::default(),
            fill: Fill::Solid(accents[0]),
        },
    ];
    assert_eq!(stripes(&probe, &accents).len(), 1, "a thin accent fill on the left edge is a stripe");

    let mut seen = 0usize;
    for dark in [true, false] {
        for world in [World::Normal, World::Empty] {
            for name in SURFACES {
                let Some(mut rig) = open_surface(name, world, dark) else { continue };
                // Rest, then the connected row, a hovered row and a keyboard-focused one.
                let mut draws = vec![rig.frame_pop()];
                if let Some(r) = rig.nodes().iter().find(|n| has_class(n, "row")).map(|n| n.rect) {
                    let c = center(&r);
                    rig.hover(c.x, c.y);
                    rig.settle(300);
                    draws.push(rig.frame_pop());
                }
                for _ in 0..3 {
                    rig.tab();
                    rig.settle(300);
                    draws.push(rig.frame_pop());
                }
                for draw in draws {
                    seen += 1;
                    let cmds: Vec<DrawCmd> = draw.items.iter().map(|i| i.cmd.clone()).collect();
                    let found = stripes(&cmds, &accents);
                    assert!(
                        found.is_empty(),
                        "{name} / {} / {}: {found:?}",
                        world.name(),
                        if dark { "dark" } else { "light" }
                    );
                }
            }
        }
    }
    eprintln!("{seen} draw lists checked");
    assert!(seen > 100, "{seen} draw lists checked");

    // The stylesheet itself never asks for one: no single-side border, no inset shadow, no bar pseudo-element.
    let css = std::fs::read_to_string(assets().join("presets").join("default.css")).unwrap();
    for (n, line) in css.lines().enumerate() {
        let l = line.trim();
        let side = [
            "border-left:",
            "border-right:",
            "border-top:",
            "border-bottom:",
            "border-left-",
            "border-right-",
            "border-top-",
            "border-bottom-",
        ];
        assert!(!side.iter().any(|p| l.starts_with(p)), "default.css line {}: {l}", n + 1);
        assert!(!(l.starts_with("box-shadow") && l.contains("inset")), "default.css line {}: {l}", n + 1);
        assert!(!l.contains("::before") && !l.contains("::after"), "default.css line {}: {l}", n + 1);
    }
}

// ---------------------------------------------------------------- gesture-only actions

#[test]
fn every_panel_action_that_changes_something_is_refused_outside_a_gesture_and_runs_inside_one() {
    const REFUSAL: &str = "only works inside a click or key handler";
    // (surface, widget, class of the element, which one, the command it sends from a click)
    let cases: [(&str, &str, &str, usize, &str); 13] = [
        ("bluetooth", "dashboard", "icon-btn", 1, "rescan"),
        ("bluetooth", "dashboard", "icon-btn", 2, "connect"),
        ("bluetooth", "dashboard", "icon-btn", 3, "forget"),
        ("bluetooth", "dashboard", "icon-btn", 7, "disconnect"),
        ("audio", "dashboard", "lead", 0, "set_default"),
        ("apps", "dashboard", "row", 0, "launch"),
        ("home", "dashboard", "tile", 4, "open_uri"),
        ("volume", "volume", "lead", 1, "set_default"),
        ("network", "network", "icon-btn", 1, "disconnect"),
        ("network", "network", "net", 1, "connect"),
        ("power", "power", "power-btn", 0, "shut_down"),
        ("power", "power", "power-btn", 1, "restart"),
        ("power", "power", "power-btn", 2, "sleep"),
    ];
    for (name, widget, class, nth, kind) in cases {
        let mut rig = open_surface(name, World::Normal, true).unwrap_or_else(|| panic!("{name} opens"));
        // Forgetting a device takes a second press; the first only arms the button.
        let presses = if kind == "forget" { 2 } else { 1 };
        let (commands, lines) = (rig.commands().len(), rig.new_log(REFUSAL).len());
        for _ in 0..presses {
            rig.click_element(widget, class, nth, 0);
        }
        assert_eq!(rig.commands().len(), commands, "{name} {kind}: a command went out without a gesture");
        assert_eq!(rig.new_log(REFUSAL).len(), lines + 1, "{name} {kind}: no refusal was logged");

        let mut rig = open_surface(name, World::Normal, true).unwrap();
        // Past the guard of the power menu, past the 2 s gap between two commands.
        rig.step(2100);
        let before = rig.commands().len();
        for _ in 0..presses {
            let g = rig.h.next_gesture();
            rig.click_element(widget, class, nth, g);
        }
        let after = rig.kinds();
        assert!(after.len() > before && after.last().map(String::as_str) == Some(kind), "{name} {kind}: {after:?}");
    }

    // The same for the two things that need a gesture to open: the power menu from the dashboard's tile
    // and Windows' location settings from the Wi-Fi popup.
    let mut rig = open_surface("home", World::Normal, true).unwrap();
    rig.click_element("dashboard", "tile", 5, 0);
    rig.push_trees();
    assert!(!rig.is_open("power"), "the power menu opened without a gesture");
    assert!(rig.log_has("an overlay only opens from a click or a key"));

    let mut events = World::Normal.events();
    events.push(ok(ServiceId::Wifi, wifi_value(vec![], "needs_location")));
    let mut rig = Rig::with_events(events, true, &["volume", "network"]);
    rig.open("network", false);
    rig.settle(800);
    assert_has(&rig.texts(), "LOCATION OFF - Windows hides network names", "the location notice");
    assert_has(&rig.texts(), "Open location settings", "the settings button");
    let (commands, lines) = (rig.commands().len(), rig.new_log(REFUSAL).len());
    rig.click_element("network", "action", 0, 0);
    assert_eq!(rig.commands().len(), commands);
    assert_eq!(rig.new_log(REFUSAL).len(), lines + 1);
    rig.step(2100);
    let g = rig.h.next_gesture();
    rig.click_element("network", "action", 0, g);
    let sent = rig.commands();
    match sent.last() {
        Some(ServiceCommand::Apps(rsb_core::services::AppsCommand::OpenUri(uri))) => {
            assert_eq!(uri, "ms-settings:privacy-location");
        }
        other => panic!("expected the location settings page, got {other:?}"),
    }
}

// ---------------------------------------------------------------- pictures

fn shoot(rig: &mut Rig, state: &str) {
    let theme = if rig.dark { "dark" } else { "light" };
    let canvas = rig.picture();
    // The panel is on screen: far more than the bar's own strip differs from the flat wallpaper.
    let wall = &canvas.data[(700 * SCREEN_W as usize + 5) * 4..][..4];
    let below_bar = &canvas.data[(40 * SCREEN_W as usize * 4)..];
    let drawn = below_bar.as_chunks::<4>().0.iter().filter(|px| px[..] != *wall).count();
    assert!(drawn > 20_000, "{state}: only {drawn} pixels below the bar differ from the wallpaper");
    let path = save(&canvas, &format!("panel-{state}-{theme}.png"));
    assert!(std::fs::metadata(path).unwrap().len() > 2000, "{state}: the PNG is empty");
}

fn hover_node(rig: &mut Rig, class: &str, nth: usize) {
    let r = rig.rect(class, nth);
    let c = center(&r);
    rig.hover(c.x, c.y);
    rig.settle(400);
}

/// Every surface to a PNG (the folder is `RSB_PANEL_PNG_DIR`, else `target/widget-png`) in a lived-in state: the
/// apps tile hovered, the fourth launcher row hovered, the Bluetooth list with its last device connected, the
/// audio list with its last output current, Restart hovered.
#[test]
fn every_panel_state_is_drawn_to_a_picture() {
    for dark in [true, false] {
        let mut rig = open_surface("home", World::Normal, dark).unwrap();
        hover_node(&mut rig, "tile", 3);
        shoot(&mut rig, "dash-home");

        let mut rig = Rig::new(World::Normal, dark);
        rig.open("launcher", false);
        rig.settle(1500);
        hover_node(&mut rig, "row", 3);
        shoot(&mut rig, "launcher-empty");

        let mut rig = Rig::new(World::Normal, dark);
        rig.open("launcher", false);
        rig.settle(1500);
        rig.type_text("bt");
        rig.settle(600);
        shoot(&mut rig, "launcher-bt");

        for name in ["power-mode", "bluetooth", "audio"] {
            let mut rig = open_surface(name, World::Normal, dark).unwrap();
            rig.pointer_away();
            rig.settle(300);
            shoot(&mut rig, &format!("page-{name}"));
        }
        for name in ["calendar", "media", "volume", "network"] {
            let mut rig = open_surface(name, World::Normal, dark).unwrap();
            shoot(&mut rig, name);
        }
        let mut rig = open_surface("power", World::Normal, dark).unwrap();
        hover_node(&mut rig, "power-btn", 1);
        shoot(&mut rig, "power");
    }
}

// ---------------------------------------------------------------- the user popup example

#[test]
fn the_todo_example_adds_a_line_on_enter_and_ticks_it_off_with_a_click() {
    hold_the_desk();
    let src = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("docs").join("examples").join("todo.luau"),
    )
    .expect("docs/examples/todo.luau");
    assert!(src.starts_with("--!strict\n") && src.contains("-- Save as widgets") && src.contains("right = [\"todo\""));
    let mut h = bench();
    h.user("widgets/todo.luau", &src);
    h.start(&[(1, "todo")]);
    assert!(h.plate(1).is_none(), "{:?}", h.plate(1));
    // Not opened yet: the popup holds nothing; a click opens it and it says there is nothing to do.
    assert!(find_kind(h.root(1).unwrap(), ElementKind::Input).is_none());
    let click = h.root(1).unwrap().handlers.click.expect("the button takes a click").0;
    h.click(1, click);
    assert!(h.texts(1).iter().any(|t| t == "Nothing to do"), "{:?}", h.texts(1));

    let submit = find_kind(h.root(1).unwrap(), ElementKind::Input).unwrap().input.as_ref().unwrap().on_submit.unwrap();
    h.send(element_call(1, submit, rsb_core::hit::ElementEvent::Submitted(Arc::from("  Buy milk ")), 0));
    h.send(element_call(1, submit, rsb_core::hit::ElementEvent::Submitted(Arc::from("   ")), 0));
    assert!(h.texts(1).iter().any(|t| t == "Buy milk"), "{:?}", h.texts(1));
    assert!(!h.texts(1).iter().any(|t| t == "Nothing to do"));
    let mut rows = Vec::new();
    collect_class(h.root(1).unwrap(), "row", &mut rows);
    assert_eq!(rows.len(), 1, "a blank line adds nothing");

    h.click(1, rows[0].handlers.click.expect("a row takes a click").0);
    assert!(h.texts(1).iter().any(|t| t == "Nothing to do"), "{:?}", h.texts(1));
    assert!(h.plate(1).is_none());
    release_the_desk();
}

#[test]
fn the_record_tile_opens_the_recorder_or_runs_the_command_bar_toml_names() {
    use rsb_core::io::{IoRequest, ProgramRef};
    hold_the_desk();
    let tile_click = |h: &mut Harness| {
        let click = h.root(1).unwrap().handlers.click.expect("the centre button takes a click").0;
        h.click(1, click);
        let mut tiles = Vec::new();
        collect_class(h.root(1).unwrap(), "tile", &mut tiles);
        assert_eq!(tiles.len(), 6);
        let g = h.next_gesture();
        h.click_gesture(1, tiles[4].handlers.click.unwrap().0, g);
    };

    let mut h = bench();
    h.start_specs(vec![Harness::spec(1, "dashboard")]);
    tile_click(&mut h);
    assert!(
        matches!(h.hub.lock().unwrap().last(), Some(HubRequest::Command { command, .. }) if command.kind() == "open_uri")
    );
    assert!(h.hub.lock().unwrap().iter().all(|r| !matches!(r, HubRequest::Io { .. })), "no program runs by default");

    let mut h = bench();
    h.start_specs(vec![Harness::spec_with(
        1,
        "dashboard",
        &[("record_command", list(vec![s("obs64"), s("--startrecording")]))],
    )]);
    tile_click(&mut h);
    let ran: Vec<_> = h
        .hub
        .lock()
        .unwrap()
        .iter()
        .filter_map(|r| match r {
            HubRequest::Io { request: IoRequest::ExecRun(spec), .. } => Some(spec.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(ran.len(), 1);
    assert_eq!(ran[0].program, ProgramRef::Name("obs64".into()));
    assert_eq!(ran[0].args, vec!["--startrecording"]);
    assert!(
        !h.hub
            .lock()
            .unwrap()
            .iter()
            .any(|r| matches!(r, HubRequest::Command { command, .. } if command.kind() == "open_uri"))
    );
    assert!(
        log_lines("granted dashboard:").iter().any(|l| l.contains("exec:obs64")),
        "{:?}",
        log_lines("granted dashboard:")
    );
    release_the_desk();
}

// ---------------------------------------------------------------- rows, focus, contrast, copy and memory

#[test]
fn the_first_launcher_row_keeps_the_cards_padding_when_the_list_scrolls_back_to_it() {
    let mut rig = launcher(many_apps());
    // The text of row 1 below the top of the list, from the draw list (layout rectangles do not carry the scroll
    // offset).
    let first = |rig: &mut Rig| {
        rig.settle(800);
        let list = rig.rect("list", 0);
        drawn_text(rig, "App 001").y - list.y
    };
    let at_rest = first(&mut rig);
    for _ in 0..20 {
        rig.key(Key::ArrowDown);
    }
    rig.settle(600);
    let list = rig.rect("list", 0);
    assert!(drawn_text(&mut rig, "App 021").y > list.y, "the list has scrolled down");
    for _ in 0..20 {
        rig.key(Key::ArrowUp);
    }
    let back = first(&mut rig);
    assert!(approx(back, at_rest, 0.5), "back at the first row it sits where it did at rest ({at_rest}): {back}");
}

/// Colours of the rings a draw list holds (borders drawn on all four sides) that are one of the accent colours,
/// each with the contrast of that colour against what is drawn under the middle of its top edge.
fn ring_contrast(draw: &DrawList) -> Vec<(String, f32)> {
    let accents: Vec<Color> =
        ["#81cbea", "#13648c", "#0f5a80", "#095470"].iter().map(|h| Color::parse(h).unwrap()).collect();
    let mut out = Vec::new();
    for (i, item) in draw.items.iter().enumerate() {
        let DrawCmd::Border { rect, widths, color, .. } = &item.cmd else { continue };
        let w = widths.top;
        let all = [widths.right, widths.bottom, widths.left].iter().all(|x| *x == w);
        let accent = accents.iter().any(|a| (a.r - color.r).abs() < 0.01 && (a.g - color.g).abs() < 0.01);
        if w <= 0.0 || !all || !accent || color.a < 0.99 {
            continue;
        }
        let at = PointF { x: rect.x + rect.w / 2.0, y: rect.y + w / 2.0 };
        let mut worst = f32::MAX;
        for base in [Color::BLACK, Color::WHITE] {
            let mut bg = base;
            for prev in &draw.items[..i] {
                if let DrawCmd::Fill { rect, fill: Fill::Solid(c), .. } = &prev.cmd
                    && rect.contains(at)
                {
                    bg = c.over(bg);
                }
            }
            worst = worst.min(Color::contrast_ratio(*color, bg));
        }
        out.push((format!("ring {} at {:.0},{:.0}", rect_text(rect), at.x, at.y), worst));
    }
    out
}

#[test]
fn focus_rings_and_the_current_row_outline_reach_3_to_1_in_both_themes() {
    for dark in [true, false] {
        let theme = if dark { "dark" } else { "light" };
        let mut measured = 0;
        let mut lowest = f32::MAX;
        for name in SURFACES {
            let Some(mut rig) = open_surface(name, World::Normal, dark) else { continue };
            // The surface at rest (the current and the connected row), then every focusable element once.
            let mut frames = vec![rig.frame_pop()];
            for _ in 0..14 {
                rig.tab();
                rig.settle(300);
                frames.push(rig.frame_pop());
            }
            for draw in frames {
                for (what, ratio) in ring_contrast(&draw) {
                    measured += 1;
                    lowest = lowest.min(ratio);
                    assert!(ratio >= 3.0, "{name} / {theme}: {what} is {ratio:.2}:1, under 3:1");
                }
            }
        }
        eprintln!("{theme}: {measured} ring measurements, lowest {lowest:.2}:1 (pass mark 3)");
        assert!(measured > 20, "{theme}: only {measured} rings measured");
    }
}

#[test]
fn a_press_on_an_audio_row_picks_the_output_and_only_a_drag_sets_its_level() {
    let mut rig = open_surface("audio", World::Normal, true).unwrap();
    let row = rig.rects("output")[1];
    let (y, x) = (row.y + row.h / 2.0, row.x);
    rig.step(2100);
    let before = rig.kinds();
    rig.click(x + 200.0, y);
    let kinds = rig.kinds();
    assert_eq!(kinds.len(), before.len() + 1, "{kinds:?}");
    assert_eq!(kinds.last().map(String::as_str), Some("set_default"), "a click on the row picks the output");
    // The level is where it was: no volume command went out.
    assert!(!kinds[before.len()..].iter().any(|k| k == "set_device_volume"));
    // A drag along the row sets the level of that device.
    rig.step(2100);
    rig.pointer(PointerKind::Move, x + 150.0, y);
    rig.pointer(PointerKind::Down(Button::Left), x + 150.0, y);
    rig.pointer(PointerKind::Move, x + 300.0, y);
    rig.step(60);
    rig.pointer(PointerKind::Up(Button::Left), x + 300.0, y);
    assert!(rig.kinds().iter().any(|k| k == "set_device_volume"), "{:?}", rig.kinds());
    assert_ne!(rig.kinds().last().map(String::as_str), Some("set_default"), "a drag is no click");
}

#[test]
fn every_output_shows_its_level_as_a_number_and_the_mixers_master_row_fits_its_track() {
    let mut rig = open_surface("audio", World::Normal, true).unwrap();
    let levels: Vec<String> = rig.texts().into_iter().filter(|t| t.ends_with('%')).collect();
    assert_eq!(levels.len(), rig.rects("output").len(), "one level per output row: {levels:?}");
    assert!(levels.iter().all(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit())), "{levels:?}");

    let mut rig = open_surface("volume", World::Normal, true).unwrap();
    let tracks = rig.rects("track");
    let master = tracks[0];
    assert!(approx(master.w, 360.0, 1.0), "the master row is as wide as the popup's content: {master:?}");
    let list_row = tracks[1];
    assert!(approx(list_row.w, 340.0, 1.0), "and the rows under it as wide as the list: {list_row:?}");
    // Its level is inside the track, not past its end.
    let level = rig
        .nodes()
        .into_iter()
        .filter(|n| n.kind == ElementKind::Text && has_class(n, "level"))
        .map(|n| n.rect)
        .next()
        .expect("the master level");
    assert!(level.right() <= master.right(), "{level:?} past {master:?}");
    // Each device row names its device in a tooltip, since a long name is cut to the row.
    let rows = rig.elements("volume", "output");
    assert!(rows.len() >= 4, "the master row and the devices");
    let has_tooltip = |r: &rsb_core::element::Element| r.children.iter().any(|c| c.kind == ElementKind::Tooltip);
    assert!(rows[1..].iter().all(has_tooltip), "a device row has its name as a tooltip");
}

#[test]
fn escape_from_a_page_leaves_its_tile_focused_and_a_page_opens_with_a_focused_control() {
    // Back to the tiles with Escape: the tile of the page just left has the focus, so Enter opens it again.
    for tile in [0usize, 1, 2] {
        let mut rig = Rig::new(World::Normal, true);
        rig.open("dashboard", true);
        rig.settle(1500);
        rig.click_node("tile", tile);
        rig.settle(300);
        assert_eq!(rig.rects("tile").len(), 0, "page {tile}");
        rig.key(Key::Escape);
        rig.settle(300);
        assert_eq!(rig.rects("tile").len(), 6, "back at the tiles");
        rig.key(Key::Enter);
        rig.settle(300);
        assert_eq!(rig.rects("tile").len(), 0, "tile {tile} had the focus: Enter opened its page again");
    }
    // A page opens with its back button focused (Enter goes back), or with its search focused.
    for tile in [0usize, 2] {
        let mut rig = Rig::new(World::Normal, true);
        rig.open("dashboard", true);
        rig.settle(1500);
        rig.click_node("tile", tile);
        rig.settle(300);
        rig.key(Key::Enter);
        rig.settle(300);
        assert_eq!(rig.rects("tile").len(), 6, "page {tile}: the back button had the focus");
    }
    let mut rig = Rig::new(World::Normal, true);
    rig.open("dashboard", true);
    rig.settle(1500);
    rig.click_node("tile", 1);
    rig.settle(300);
    rig.type_text("zzzz");
    rig.settle(300);
    assert_has(&rig.texts(), "No devices match", "typing went to the Bluetooth search");
    // The player opens with its play button focused: Enter plays or pauses.
    let mut rig = open_surface("media", World::Normal, true).unwrap();
    let before = rig.kinds().len();
    rig.step(2100);
    rig.key(Key::Enter);
    assert_eq!(rig.kinds().get(before).map(String::as_str), Some("play_pause"), "{:?}", rig.kinds());
}

#[test]
fn a_launcher_row_chosen_by_the_keyboard_has_a_tint_and_a_full_outline() {
    for dark in [true, false] {
        let mut rig = Rig::with_events(many_apps(), dark, &[]);
        rig.open("launcher", true);
        rig.settle(1500);
        let selected: Vec<RectF> = rig
            .nodes()
            .into_iter()
            .filter(|n| n.kind == ElementKind::Box && has_class(n, "selected"))
            .map(|n| n.rect)
            .collect();
        assert_eq!(selected.len(), 1);
        let draw = rig.frame_pop();
        let rings = ring_contrast(&draw);
        let round_row = draw.items.iter().any(|i| match &i.cmd {
            DrawCmd::Border { rect, widths, .. } => {
                widths.top >= 1.0
                    && approx(rect.x, selected[0].x, 1.5)
                    && approx(rect.w, selected[0].w, 1.5)
                    && approx(rect.y, selected[0].y, 1.5)
            }
            _ => false,
        });
        assert!(round_row, "{dark}: no outline round the selected row {:?}; rings {rings:?}", selected[0]);
        assert!(rings.iter().all(|(_, ratio)| *ratio >= 3.0), "{rings:?}");
    }
}

#[test]
fn the_launcher_list_has_no_scroll_thumb_and_the_wifi_and_mixer_lists_keep_a_thin_one() {
    use rsb_core::style::ScrollbarWidth;
    let mut rig = Rig::with_events(many_apps(), true, &[]);
    rig.open("launcher", true);
    rig.settle(1500);
    let width_of = |rig: &mut Rig| {
        rig.nodes().into_iter().find(|n| has_class(n, "list")).map(|n| n.style.scrollbar_width).expect("a list")
    };
    assert_eq!(width_of(&mut rig), ScrollbarWidth::None, "the launcher list");
    let mut rig = open_surface("network", World::Normal, true).unwrap();
    assert_eq!(width_of(&mut rig), ScrollbarWidth::Thin, "the Wi-Fi list");
    let mut rig = open_surface("volume", World::Normal, true).unwrap();
    assert_eq!(width_of(&mut rig), ScrollbarWidth::Thin, "the mixer list");
}

#[test]
fn blocker_copy_has_an_uppercase_key_word_and_loading_notes_keep_their_names() {
    // Location off: a blocker, and the header names the popup.
    let mut events = World::Normal.events();
    events.push(ok(ServiceId::Wifi, wifi_value(vec![], "needs_location")));
    let mut rig = Rig::with_events(events, true, &["volume", "network"]);
    rig.open("network", false);
    rig.settle(800);
    assert_has(&rig.texts(), "LOCATION OFF - Windows hides network names", "the location notice");
    assert_has(&rig.texts(), "Wi-Fi", "the Wi-Fi header");
    // Services with no reading yet say what they are reading, names as spelt.
    let mut events = World::Normal.events();
    events.push(svc(ServiceId::Wifi, ServiceStatus::Starting, None, None));
    let mut rig = Rig::with_events(events, true, &["volume", "network"]);
    rig.open("network", false);
    rig.settle(800);
    assert_has(&rig.texts(), "Reading Wi-Fi...", "the Wi-Fi note");
    let mut events = World::Normal.events();
    events.push(svc(ServiceId::Bluetooth, ServiceStatus::Starting, None, None));
    events.push(svc(ServiceId::Apps, ServiceStatus::Starting, None, None));
    let mut rig = Rig::with_events(events, true, &[]);
    rig.open("dashboard", false);
    rig.settle(1500);
    rig.click_node("tile", 1);
    rig.settle(300);
    assert_has(&rig.texts(), "Reading Bluetooth...", "the Bluetooth note");
    rig.key(Key::Escape);
    rig.settle(300);
    rig.click_node("tile", 3);
    rig.settle(300);
    assert_has(&rig.texts(), "Reading the app list...", "the apps note keeps its lower case inside the sentence");
}

#[test]
fn forgetting_a_device_takes_a_second_press_and_the_connected_network_is_left_from_its_button() {
    let mut rig = open_surface("bluetooth", World::Normal, true).unwrap();
    rig.step(2100);
    assert!(!rig.elements("dashboard", "forget").is_empty(), "a forget button per device");
    let before = rig.kinds().len();
    let g = rig.h.next_gesture();
    rig.click_element("dashboard", "forget", 0, g);
    // The first press only armed it.
    assert_eq!(rig.kinds().len(), before, "{:?}", rig.kinds());
    rig.settle(100);
    let drawn = |rig: &mut Rig| draw_texts(&rig.frame_pop());
    assert_has(&drawn(&mut rig), "FORGET?", "the armed button");
    let g = rig.h.next_gesture();
    rig.click_element("dashboard", "forget", 0, g);
    assert_eq!(rig.kinds().last().map(String::as_str), Some("forget"));
    // After three seconds an armed button is a plain icon again.
    let mut rig = open_surface("bluetooth", World::Normal, true).unwrap();
    let g = rig.h.next_gesture();
    rig.click_element("dashboard", "forget", 0, g);
    rig.settle(100);
    assert_has(&drawn(&mut rig), "FORGET?", "armed");
    rig.step(3200);
    assert!(!drawn(&mut rig).iter().any(|t| t == "FORGET?"), "the arm ran out");

    // Wi-Fi: a click on the connected row does nothing; its own button disconnects.
    let mut rig = open_surface("network", World::Normal, true).unwrap();
    rig.step(2100);
    let before = rig.kinds().len();
    let g = rig.h.next_gesture();
    rig.click_element("network", "net", 0, g);
    assert_eq!(rig.kinds().len(), before, "the row click sends nothing: {:?}", rig.kinds());
    let g = rig.h.next_gesture();
    rig.click_element("network", "icon-btn", 1, g);
    assert_eq!(rig.kinds().last().map(String::as_str), Some("disconnect"));
}

/// Solid fills of `colour` containing `p` that are not inside a group with (nearly) no opacity, so what the
/// screen really shows.
fn shown_fills_at(draw: &DrawList, p: PointF, colour: Color) -> usize {
    let mut open: Vec<f32> = Vec::new();
    let mut n = 0;
    for item in &draw.items {
        match &item.cmd {
            DrawCmd::PushOpacity(a) => open.push(*a),
            DrawCmd::PopOpacity => {
                open.pop();
            }
            DrawCmd::Fill { rect, fill: Fill::Solid(c), .. }
                if rect.contains(p)
                    && (c.r - colour.r).abs() < 0.01
                    && (c.g - colour.g).abs() < 0.01
                    && open.iter().all(|a| *a > 0.05) =>
            {
                n += 1;
            }
            _ => {}
        }
    }
    n
}

#[test]
fn the_focused_power_button_shows_nothing_until_its_chevron_has_opened_and_arrows_work_meanwhile() {
    let mut rig = Rig::new(World::Normal, true);
    rig.open("power", true);
    // 300 ms in, the chevrons are still opening: Lock's tile has not been drawn into the empty chevron yet.
    rig.step(300);
    let tile = rig.rects("tile")[3];
    let draw = rig.frame_pop();
    let focus_fill = Color::parse("#1e2427").unwrap();
    assert_eq!(shown_fills_at(&draw, center(&tile), focus_fill), 0, "a fragment of the focus tile shows early");
    // The arrow keys move the focus while the buttons are still opening (the guard only holds back presses and
    // Enter), so the tile of the button now focused is drawn once it has faded in.
    rig.key(Key::ArrowLeft);
    rig.settle(1500);
    let draw = rig.frame_pop();
    let sleep = rig.rects("tile")[2];
    assert_eq!(shown_fills_at(&draw, center(&sleep), focus_fill), 1, "Sleep took the focus from Lock");
    assert_eq!(shown_fills_at(&draw, center(&rig.rects("tile")[3]), focus_fill), 0, "and Lock gave it up");
    // Enter in the same moment would still be ignored: nothing was sent.
    assert!(rig.kinds().is_empty(), "{:?}", rig.kinds());
}

#[test]
fn closing_the_launcher_gives_back_the_memory_its_list_made() {
    let mut rig = launcher(many_apps());
    let used = |rig: &mut Rig| rig.h.status().into_iter().map(|s| s.vm_bytes).max().unwrap_or(0);
    let open = used(&mut rig);
    rig.h.send(ScriptIn::Popup { name: "launcher".into(), action: PopupAction::Close, monitor: Some(1), gesture: 0 });
    let closed = used(&mut rig);
    println!("MEM launcher open {open} closed {closed}");
    assert!(closed + (200 << 10) < open, "the shared VM kept the launcher's data: {open} -> {closed}");
}

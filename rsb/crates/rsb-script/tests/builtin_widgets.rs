//! The built-in widgets, the presets and the docs, run for real: a `Runtime`
//! reading the widget sources straight from `assets/`, a manual clock, fake
//! service events and fake I/O replies; the same trees drawn through the
//! scene and the renderer into PNGs under `target/widget-png/`.

mod common;

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use rsb_core::color::Color;
use rsb_core::config::{self, BarSpec, Config, WidgetOptions, WidgetRef};
use rsb_core::css::{MediaEnv, Stylesheet};
use rsb_core::diag::Plate;
use rsb_core::draw::{DrawCmd, DrawList, Fill};
use rsb_core::element::{Element, ElementKind, IconRef, InstanceId};
use rsb_core::geom::{PointF, RectF};
use rsb_core::hit::{Button, InputKind, PointerEvent, PointerKind};
use rsb_core::icons::{ICON_NAMES, IconFont};
use rsb_core::io::{ExecResult, HttpResponse, IoError, IoEventKind, IoRequest};
use rsb_core::msg::{Heartbeat, HubEvent, HubRequest, InstanceSpec, MonitorRef, ScriptIn, WidgetState};
use rsb_core::scene::{BarGeometry, Scene, SceneEnv, SlotSpec, ZoneSlots};
use rsb_core::services::{
    AudioCommand, MediaCommand, ServiceCommand, ServiceEvent, ServiceId, ServiceStatus, WorkspaceCommand,
};
use rsb_core::time::ThreadTime;
use rsb_core::value::Value;
use rsb_render::{FontConfig, Renderer, render_offscreen};
use rsb_script::{Limits, Runtime, ScriptOptions, SourceStore};

// ---------------------------------------------------------------- harness

fn assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("assets")
}

fn docs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("docs")
}

fn png_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("target").join("widget-png");
    std::fs::create_dir_all(&dir).expect("the PNG folder can be made");
    dir
}

/// Built-ins from the assets folder on disk; user files from memory, so a
/// test can add a widget (the docs examples) next to them.
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

/// The script harness of the script tests, with built-ins from `assets/`.
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
            thread_time: None::<Arc<dyn ThreadTime>>,
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

/// One service event; `value` None = a status-only event.
fn event(service: ServiceId, status: ServiceStatus, value: Option<Value>) -> ScriptIn {
    ScriptIn::Hub(HubEvent::Service(ServiceEvent { service, version: 1, status, value, error: None }))
}

fn ok(service: ServiceId, value: Value) -> ScriptIn {
    event(service, ServiceStatus::Ok, Some(value))
}

fn stale(service: ServiceId) -> ScriptIn {
    event(service, ServiceStatus::Stale, None)
}

fn system(cpu: f64, memory: f64) -> Value {
    map(&[
        ("cpu", num(cpu)),
        ("memory", num(memory)),
        ("memory_used", Value::Int((memory / 100.0 * 16_000_000_000.0) as i64)),
        ("memory_total", Value::Int(16_000_000_000)),
    ])
}

fn battery(percent: Option<f64>, charging: bool, plugged: bool, saver: bool) -> Value {
    let mut pairs = vec![
        ("present", b(percent.is_some())),
        ("charging", b(charging)),
        ("plugged", b(plugged)),
        ("saver", b(saver)),
    ];
    if let Some(p) = percent {
        pairs.push(("percent", num(p)));
    }
    map(&pairs)
}

fn audio(available: bool, volume: f64, muted: bool) -> Value {
    map(&[("available", b(available)), ("volume", num(volume)), ("muted", b(muted))])
}

fn network(connected: bool, internet: bool, kind: &str, signal: Option<f64>) -> Value {
    let mut pairs = vec![
        ("connected", b(connected)),
        ("internet", b(internet)),
        ("kind", s(kind)),
        ("down_bps", Value::Int(1_258_291)),
        ("up_bps", Value::Int(20_480)),
    ];
    if let Some(sig) = signal {
        pairs.push(("signal", num(sig)));
        pairs.push(("ssid", s("Workshop")));
    }
    map(&pairs)
}

fn media(available: bool, playing: bool, artist: &str, title: &str) -> Value {
    map(&[
        ("available", b(available)),
        ("playing", b(playing)),
        ("title", s(title)),
        ("artist", s(artist)),
        ("album", s("")),
        ("source", s("player")),
        ("can_play_pause", b(available)),
        ("can_next", b(available)),
        ("can_previous", b(available)),
    ])
}

fn window(has_window: bool, title: &str, process: &str) -> Value {
    map(&[("has_window", b(has_window)), ("title", s(title)), ("process", s(process))])
}

fn ws_entry(name: &str, focused: bool, visible: bool, windows: i64) -> Value {
    map(&[
        ("name", s(name)),
        ("display_name", s(name)),
        ("focused", b(focused)),
        ("visible", b(visible)),
        ("has_windows", b(windows > 0)),
        ("window_count", Value::Int(windows)),
    ])
}

/// Two monitors; the first is the harness monitor (handle 1, DISPLAY1).
fn workspaces_value() -> Value {
    let monitor = |index: i64, handle: i64, device: &str, list: Vec<Value>| {
        map(&[
            ("index", Value::Int(index)),
            ("name", s(&format!("m{index}"))),
            ("handle", Value::Int(handle)),
            ("device", s(device)),
            ("workspaces", Value::List(list)),
        ])
    };
    map(&[
        ("provider", s("glazewm")),
        ("connected", b(true)),
        (
            "monitors",
            Value::List(vec![
                monitor(
                    0,
                    1,
                    "\\\\.\\DISPLAY1",
                    vec![
                        ws_entry("1", true, true, 2),
                        ws_entry("2", false, false, 1),
                        ws_entry("3", false, false, 0),
                        ws_entry("4", false, false, 0),
                    ],
                ),
                monitor(1, 2, "\\\\.\\DISPLAY2", vec![ws_entry("5", false, true, 1), ws_entry("6", false, false, 0)]),
            ]),
        ),
    ])
}

fn no_workspaces() -> Value {
    map(&[("provider", s("none")), ("connected", b(false)), ("monitors", Value::List(vec![]))])
}

/// What one scenario tells every service.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Scenario {
    Normal,
    Alert,
    Stale,
    Empty,
}

impl Scenario {
    const ALL: [Scenario; 4] = [Scenario::Normal, Scenario::Alert, Scenario::Stale, Scenario::Empty];

    fn name(self) -> &'static str {
        match self {
            Scenario::Normal => "normal",
            Scenario::Alert => "alert",
            Scenario::Stale => "stale",
            Scenario::Empty => "empty",
        }
    }

    fn events(self) -> Vec<ScriptIn> {
        match self {
            Scenario::Normal => vec![
                ok(ServiceId::System, system(23.4, 41.0)),
                ok(ServiceId::Battery, battery(Some(80.0), false, false, false)),
                ok(ServiceId::Audio, audio(true, 45.0, false)),
                ok(ServiceId::Network, network(true, true, "wifi", Some(72.0))),
                ok(ServiceId::Media, media(true, true, "Aurora Quartet", "Slow Harbour")),
                ok(ServiceId::Window, window(true, "Quarterly plan - Notes", "Notes.exe")),
                ok(ServiceId::Workspaces, workspaces_value()),
            ],
            Scenario::Alert => vec![
                ok(ServiceId::System, system(95.0, 92.0)),
                ok(ServiceId::Battery, battery(Some(10.0), false, false, false)),
                ok(ServiceId::Audio, audio(true, 45.0, true)),
                ok(ServiceId::Network, network(true, false, "wifi", Some(30.0))),
                ok(ServiceId::Media, media(true, false, "Aurora Quartet", "Slow Harbour")),
                ok(
                    ServiceId::Window,
                    window(true, "A very long window title that has to be cut with an ellipsis somewhere", "Code.exe"),
                ),
                ok(ServiceId::Workspaces, workspaces_value()),
            ],
            Scenario::Stale => vec![
                ok(ServiceId::System, system(23.4, 41.0)),
                ok(ServiceId::Battery, battery(Some(80.0), false, false, false)),
                ok(ServiceId::Audio, audio(true, 45.0, false)),
                ok(ServiceId::Network, network(true, true, "wifi", Some(72.0))),
                ok(ServiceId::Media, media(true, true, "Aurora Quartet", "Slow Harbour")),
                ok(ServiceId::Window, window(true, "Notes", "Notes.exe")),
                ok(ServiceId::Workspaces, workspaces_value()),
                stale(ServiceId::System),
                stale(ServiceId::Battery),
                stale(ServiceId::Audio),
                stale(ServiceId::Network),
                stale(ServiceId::Media),
                stale(ServiceId::Window),
                stale(ServiceId::Workspaces),
            ],
            Scenario::Empty => vec![
                ok(ServiceId::System, system(0.0, 0.0)),
                ok(ServiceId::Battery, battery(None, false, true, false)),
                ok(ServiceId::Audio, audio(false, 0.0, false)),
                ok(ServiceId::Network, network(false, false, "none", None)),
                ok(ServiceId::Media, media(false, false, "", "")),
                ok(ServiceId::Window, window(false, "", "")),
                ok(ServiceId::Workspaces, no_workspaces()),
            ],
        }
    }
}

/// Short text form of an element tree: `box.item.cpu(icon:cpu text.value="23%")`.
/// A hidden element (display none) gets a `~` after its classes.
fn show(e: &Element) -> String {
    let mut out = String::from(match e.kind {
        ElementKind::Box => "box",
        ElementKind::Text => "text",
        ElementKind::Icon => "icon",
        ElementKind::Image => "image",
        _ => "?",
    });
    for c in &e.classes {
        out.push('.');
        out.push_str(c);
    }
    if e.style.iter().any(|(k, v)| k == "display" && v == "none") {
        out.push('~');
    }
    match e.kind {
        ElementKind::Text => out.push_str(&format!("={:?}", e.text.as_deref().unwrap_or(""))),
        ElementKind::Icon => match &e.icon {
            Some(IconRef::Named(n)) => out.push_str(&format!(":{n}")),
            Some(IconRef::Glyph(g)) => out.push_str(&format!(":glyph({g})")),
            None => out.push_str(":none"),
        },
        _ => {
            let kids: Vec<String> = e.children.iter().map(show).collect();
            out.push('(');
            out.push_str(&kids.join(" "));
            out.push(')');
        }
    }
    out
}

fn shown(h: &Harness, id: u32) -> String {
    match h.root(id) {
        Some(root) => show(root),
        None => format!("(no tree; plate: {:?})", h.plate(id)),
    }
}

/// Starts one built-in as instance 1 with options from `bar.toml`.
fn start(h: &mut Harness, widget: &str, opts: &[(&str, Value)]) {
    h.start_specs(vec![Harness::spec_with(1, widget, opts)]);
}

fn send_all(h: &mut Harness, events: Vec<ScriptIn>) {
    for e in events {
        h.send(e);
    }
}

fn commands(h: &Harness) -> Vec<ServiceCommand> {
    h.hub
        .lock()
        .unwrap()
        .iter()
        .filter_map(|r| match r {
            HubRequest::Command { command, .. } => Some(command.clone()),
            _ => None,
        })
        .collect()
}

fn click_handler(e: &Element) -> u32 {
    first_click(e).expect("the widget has a click handler")
}

fn find_handler(e: &Element, pick: &dyn Fn(&Element) -> Option<u32>) -> Option<u32> {
    if let Some(h) = pick(e) {
        return Some(h);
    }
    e.children.iter().find_map(|c| find_handler(c, pick))
}

fn wheel(h: &mut Harness, id: u32, delta: f32) {
    let handler = find_handler(h.root(id).unwrap(), &|e| e.handlers.wheel.map(|w| w.0)).expect("a wheel handler");
    h.send(ScriptIn::Input(rsb_core::hit::HandlerCall {
        instance: InstanceId(id),
        handler: rsb_core::element::HandlerId(handler),
        kind: InputKind::Wheel(delta),
        x: 1.0,
        y: 1.0,
    }));
}

fn right_click(h: &mut Harness, id: u32) {
    let handler =
        find_handler(h.root(id).unwrap(), &|e| e.handlers.right_click.map(|w| w.0)).expect("a right click handler");
    h.send(ScriptIn::Input(rsb_core::hit::HandlerCall {
        instance: InstanceId(id),
        handler: rsb_core::element::HandlerId(handler),
        kind: InputKind::Click(Button::Right),
        x: 1.0,
        y: 1.0,
    }));
}

// ---------------------------------------------------------------- cpu and memory

#[test]
fn cpu_normal_warn_stale_and_empty_states() {
    let mut h = bench();
    start(&mut h, "cpu", &[]);
    assert_eq!(shown(&h, 1), "box.item.cpu(icon:cpu text.value=\"--\")", "before the first sample");
    h.send(ok(ServiceId::System, system(23.4, 41.0)));
    assert_eq!(shown(&h, 1), "box.item.cpu(icon:cpu text.value=\"23%\")");
    h.send(ok(ServiceId::System, system(95.0, 41.0)));
    assert_eq!(shown(&h, 1), "box.item.cpu.warn(icon:cpu text.value=\"95%\")");
    h.send(stale(ServiceId::System));
    assert_eq!(shown(&h, 1), "box.item.cpu.stale(icon:cpu text.value=\"--\")");
    h.send(ok(ServiceId::System, system(10.0, 41.0)));
    assert_eq!(shown(&h, 1), "box.item.cpu(icon:cpu text.value=\"10%\")", "it recovers by itself");
}

#[test]
fn memory_formats_used_and_total_and_follows_the_same_states() {
    let mut h = bench();
    start(&mut h, "memory", &[("format", s("{value}% {used}/{total}"))]);
    h.send(ok(ServiceId::System, system(5.0, 50.0)));
    assert_eq!(shown(&h, 1), "box.item.memory(icon:memory text.value=\"50% 7.5 GB/14.9 GB\")");
    h.send(ok(ServiceId::System, system(5.0, 91.0)));
    assert!(shown(&h, 1).starts_with("box.item.memory.warn("), "{}", shown(&h, 1));
    h.send(stale(ServiceId::System));
    assert_eq!(shown(&h, 1), "box.item.memory.stale(icon:memory text.value=\"--\")");
}

#[test]
fn a_label_replaces_the_icon_and_an_icon_name_can_be_chosen() {
    let mut h = bench();
    h.start_specs(vec![
        Harness::spec_with(1, "cpu#a", &[("label", s("CPU"))]),
        Harness::spec_with(2, "cpu#b", &[("icon", s("sun")), ("warn", num(20.0)), ("format", s("{value} pc"))]),
    ]);
    h.send(ok(ServiceId::System, system(23.4, 41.0)));
    assert_eq!(shown(&h, 1), "box.item.cpu(text.label=\"CPU\" text.value=\"23%\")");
    assert_eq!(shown(&h, 2), "box.item.cpu.warn(icon:sun text.value=\"23 pc\")");
}

// ---------------------------------------------------------------- battery

#[test]
fn battery_levels_charging_low_saver_absent_and_stale() {
    let mut h = bench();
    start(&mut h, "battery", &[]);
    assert_eq!(
        shown(&h, 1),
        "box.item.battery~(icon:battery_none text.value=\"--\")",
        "hidden before the first reading"
    );
    h.send(ok(ServiceId::Battery, battery(Some(80.0), false, false, false)));
    assert_eq!(shown(&h, 1), "box.item.battery(icon:battery_8 text.value=\"80%\")");
    h.send(ok(ServiceId::Battery, battery(Some(100.0), false, true, false)));
    assert_eq!(shown(&h, 1), "box.item.battery(icon:battery_10 text.value=\"100%\")");
    h.send(ok(ServiceId::Battery, battery(Some(55.0), true, true, false)));
    assert_eq!(shown(&h, 1), "box.item.battery.charging(icon:battery_charging text.value=\"55%\")");
    h.send(ok(ServiceId::Battery, battery(Some(12.0), false, false, false)));
    assert_eq!(shown(&h, 1), "box.item.battery.low(icon:battery_1 text.value=\"12%\")");
    h.send(ok(ServiceId::Battery, battery(Some(12.0), false, true, false)));
    assert_eq!(shown(&h, 1), "box.item.battery(icon:battery_1 text.value=\"12%\")", "on mains it is not low");
    h.send(ok(ServiceId::Battery, battery(Some(60.0), false, false, true)));
    assert_eq!(shown(&h, 1), "box.item.battery.saver(icon:battery_saver text.value=\"60%\")");
    h.send(ok(ServiceId::Battery, battery(None, false, true, false)));
    assert_eq!(shown(&h, 1), "box.item.battery.absent(icon:battery_none text.value=\"\")");
    h.send(stale(ServiceId::Battery));
    assert_eq!(shown(&h, 1), "box.item.battery.stale(icon:battery_none text.value=\"--\")", "no battery stays a plug");
    // A PC with a battery whose reading goes stale keeps its level glyph, never the plug.
    let mut h = bench();
    start(&mut h, "battery", &[]);
    h.send(ok(ServiceId::Battery, battery(Some(80.0), false, false, false)));
    h.send(stale(ServiceId::Battery));
    assert_eq!(shown(&h, 1), "box.item.battery.stale(icon:battery_8 text.value=\"--\")");
}

#[test]
fn battery_absent_option_picks_icon_text_or_nothing() {
    let mut h = bench();
    h.start_specs(vec![
        Harness::spec_with(1, "battery#icon", &[("absent", s("icon"))]),
        Harness::spec_with(2, "battery#text", &[("absent", s("text"))]),
        Harness::spec_with(3, "battery#hide", &[("absent", s("hide"))]),
    ]);
    h.send(ok(ServiceId::Battery, battery(None, false, true, false)));
    assert_eq!(shown(&h, 1), "box.item.battery.absent(icon:battery_none text.value=\"\")");
    assert_eq!(shown(&h, 2), "box.item.battery.absent(icon:battery_none text.value=\"AC\")");
    assert_eq!(shown(&h, 3), "box.item.battery.absent~(icon:battery_none text.value=\"\")");
}

// ---------------------------------------------------------------- volume

#[test]
fn volume_icons_muted_absent_stale_and_the_commands() {
    let mut h = bench();
    start(&mut h, "volume", &[]);
    assert!(shown(&h, 1).contains('~'), "hidden before the first reading: {}", shown(&h, 1));
    h.send(ok(ServiceId::Audio, audio(true, 0.0, false)));
    assert_eq!(shown(&h, 1), "box.item.volume.button(icon:volume_0 text.value=\"0%\")");
    h.send(ok(ServiceId::Audio, audio(true, 20.0, false)));
    assert_eq!(shown(&h, 1), "box.item.volume.button(icon:volume_1 text.value=\"20%\")");
    h.send(ok(ServiceId::Audio, audio(true, 50.0, false)));
    assert_eq!(shown(&h, 1), "box.item.volume.button(icon:volume_2 text.value=\"50%\")");
    h.send(ok(ServiceId::Audio, audio(true, 90.0, false)));
    assert_eq!(shown(&h, 1), "box.item.volume.button(icon:volume_3 text.value=\"90%\")");
    h.send(ok(ServiceId::Audio, audio(true, 90.0, true)));
    assert_eq!(shown(&h, 1), "box.item.volume.button.muted(icon:volume_mute text.value=\"90%\")");
    h.send(ok(ServiceId::Audio, audio(false, 0.0, false)));
    assert_eq!(shown(&h, 1), "box.item.volume.button.absent(icon:volume_mute text.value=\"\")");
    h.send(stale(ServiceId::Audio));
    assert_eq!(shown(&h, 1), "box.item.volume.button.stale(icon:volume_mute text.value=\"--\")");

    // Commands: a click toggles mute, the wheel moves by `step` (default 2)
    // per notch; with no output device nothing is sent.
    h.send(ok(ServiceId::Audio, audio(true, 50.0, false)));
    h.take_requests();
    let click = click_handler(h.root(1).unwrap());
    h.click(1, click);
    wheel(&mut h, 1, 1.0);
    wheel(&mut h, 1, -3.0);
    assert_eq!(
        commands(&h),
        vec![
            ServiceCommand::Audio(AudioCommand::ToggleMute),
            ServiceCommand::Audio(AudioCommand::ChangeVolume(2.0)),
            ServiceCommand::Audio(AudioCommand::ChangeVolume(-6.0)),
        ]
    );
    h.send(ok(ServiceId::Audio, audio(false, 0.0, false)));
    h.take_requests();
    h.click(1, click);
    wheel(&mut h, 1, 1.0);
    assert!(commands(&h).is_empty(), "no device, no command");
}

#[test]
fn volume_step_and_label_options() {
    let mut h = bench();
    start(&mut h, "volume", &[("step", num(5.0)), ("label", s("VOL")), ("format", s("{value}"))]);
    h.send(ok(ServiceId::Audio, audio(true, 33.0, false)));
    assert_eq!(shown(&h, 1), "box.item.volume.button(text.label=\"VOL\" text.value=\"33\")");
    h.take_requests();
    wheel(&mut h, 1, 2.0);
    assert_eq!(commands(&h), vec![ServiceCommand::Audio(AudioCommand::ChangeVolume(10.0))]);
}

#[test]
fn volume_muted_format_replaces_the_format_only_while_muted() {
    let mut h = bench();
    start(&mut h, "volume", &[("label", s("VOL")), ("format", s("{value}")), ("muted_format", s("MUTE"))]);
    h.send(ok(ServiceId::Audio, audio(true, 35.0, false)));
    assert_eq!(shown(&h, 1), "box.item.volume.button(text.label=\"VOL\" text.value=\"35\")");
    h.send(ok(ServiceId::Audio, audio(true, 35.0, true)));
    assert_eq!(shown(&h, 1), "box.item.volume.button.muted(text.label=\"VOL\" text.value=\"MUTE\")");
    // Left empty, the muted bar shows the level like before.
    let mut h = bench();
    start(&mut h, "volume", &[]);
    h.send(ok(ServiceId::Audio, audio(true, 35.0, true)));
    assert_eq!(shown(&h, 1), "box.item.volume.button.muted(icon:volume_mute text.value=\"35%\")");
}

// ---------------------------------------------------------------- network

#[test]
fn network_icon_by_kind_and_signal_offline_and_stale() {
    let mut h = bench();
    start(&mut h, "network", &[]);
    assert!(shown(&h, 1).contains('~'), "hidden before the first reading");
    let cases = [
        (network(true, true, "wifi", Some(90.0)), "box.item.network(icon:wifi_4)"),
        (network(true, true, "wifi", Some(50.0)), "box.item.network(icon:wifi_3)"),
        (network(true, true, "wifi", Some(30.0)), "box.item.network(icon:wifi_2)"),
        (network(true, true, "wifi", Some(5.0)), "box.item.network(icon:wifi_1)"),
        (network(true, true, "ethernet", None), "box.item.network(icon:ethernet)"),
        (network(true, true, "cellular", None), "box.item.network(icon:globe)"),
        (network(true, false, "wifi", Some(90.0)), "box.item.network.no-internet(icon:wifi_4)"),
        (network(false, false, "none", None), "box.item.network.offline(icon:network_off)"),
    ];
    for (value, want) in cases {
        h.send(ok(ServiceId::Network, value));
        assert_eq!(shown(&h, 1), want);
    }
    h.send(stale(ServiceId::Network));
    assert_eq!(shown(&h, 1), "box.item.network.stale(icon:globe)");
}

#[test]
fn network_format_adds_text_and_stale_shows_dashes() {
    let mut h = bench();
    start(&mut h, "network", &[("format", s("{ssid} {down} {up} {kind}"))]);
    h.send(ok(ServiceId::Network, network(true, true, "wifi", Some(90.0))));
    assert_eq!(shown(&h, 1), "box.item.network(icon:wifi_4 text.value=\"Workshop 1.2 MB/s 20.0 KB/s wifi\")");
    h.send(stale(ServiceId::Network));
    assert_eq!(shown(&h, 1), "box.item.network.stale(icon:globe text.value=\"--\")");
    // The sampling wish follows `every`.
    let mut h2 = bench();
    start(&mut h2, "network", &[("every", s("5s"))]);
    let wish = h2.requests().into_iter().find(|r| r.contains("Subscribe")).unwrap();
    assert!(wish.contains("5s") || wish.contains("5000") || wish.contains("secs: 5"), "{wish}");
}

// ---------------------------------------------------------------- media

#[test]
fn media_hidden_without_a_session_shows_the_track_and_keeps_a_paused_one() {
    let mut h = bench();
    start(&mut h, "media", &[]);
    h.send(ok(ServiceId::Media, media(false, false, "", "")));
    assert_eq!(shown(&h, 1), "box.item.media.button.idle~(icon:music text.value.ellipsis=\"\")");
    h.send(ok(ServiceId::Media, media(true, true, "Aurora Quartet", "Slow Harbour")));
    assert_eq!(
        shown(&h, 1),
        "box.item.media.button.playing(icon:play text.value.ellipsis=\"Aurora Quartet - Slow Harbour\")"
    );
    h.send(ok(ServiceId::Media, media(true, false, "Aurora Quartet", "Slow Harbour")));
    assert_eq!(
        shown(&h, 1),
        "box.item.media.button.paused(icon:pause text.value.ellipsis=\"Aurora Quartet - Slow Harbour\")",
        "a paused track still shows so it can be resumed"
    );
    h.send(ok(ServiceId::Media, media(true, true, "", "Only A Title")));
    assert!(shown(&h, 1).ends_with("text.value.ellipsis=\"Only A Title\")"), "{}", shown(&h, 1));
    h.send(stale(ServiceId::Media));
    assert_eq!(shown(&h, 1), "box.item.media.button.stale(icon:music text.value.ellipsis=\"--\")");
}

#[test]
fn media_text_is_cut_at_max_width_and_hide_never_keeps_it_visible() {
    let mut h = bench();
    start(&mut h, "media", &[("max_width", num(150.0)), ("hide", s("never"))]);
    h.send(ok(ServiceId::Media, media(false, false, "", "")));
    assert_eq!(shown(&h, 1), "box.item.media.button.idle(icon:music text.value.ellipsis=\"\")");
    let text = find(h.root(1).unwrap(), &|e| e.kind == ElementKind::Text).unwrap();
    assert!(text.style.contains(&("max-width".to_string(), "150px".to_string())), "{:?}", text.style);
}

#[test]
fn media_click_plays_or_pauses_and_right_click_skips() {
    let mut h = bench();
    start(&mut h, "media", &[]);
    h.send(ok(ServiceId::Media, media(true, true, "A", "T")));
    h.take_requests();
    let click = click_handler(h.root(1).unwrap());
    h.click(1, click);
    right_click(&mut h, 1);
    assert_eq!(
        commands(&h),
        vec![ServiceCommand::Media(MediaCommand::PlayPause), ServiceCommand::Media(MediaCommand::Next)]
    );
}

// ---------------------------------------------------------------- title

#[test]
fn title_empty_on_the_desktop_cut_by_max_width_and_with_the_process() {
    let mut h = bench();
    start(&mut h, "title", &[]);
    h.send(ok(ServiceId::Window, window(false, "", "")));
    assert_eq!(shown(&h, 1), "box.item.title~(text.ellipsis=\"\")", "nothing on the desktop");
    h.send(ok(ServiceId::Window, window(true, "Quarterly plan", "Notes.exe")));
    assert_eq!(shown(&h, 1), "box.item.title(text.ellipsis=\"Quarterly plan\")");
    let text = find(h.root(1).unwrap(), &|e| e.kind == ElementKind::Text).unwrap();
    assert!(text.style.contains(&("max-width".to_string(), "360px".to_string())), "{:?}", text.style);
    h.send(stale(ServiceId::Window));
    assert_eq!(shown(&h, 1), "box.item.title.stale(text.ellipsis=\"--\")");

    let mut h = bench();
    start(&mut h, "title", &[("show_process", b(true)), ("max_width", num(200.0))]);
    h.send(ok(ServiceId::Window, window(true, "Quarterly plan", "Notes.exe")));
    assert_eq!(shown(&h, 1), "box.item.title(text.ellipsis=\"Notes - Quarterly plan\")");
}

// ---------------------------------------------------------------- clock and date

fn tick_batches(h: &Harness) -> usize {
    h.tree_batches
}

#[test]
fn clock_ticks_on_the_minute_and_a_click_swaps_to_the_alt_format() {
    let mut h = bench();
    start(&mut h, "clock", &[]);
    assert_eq!(shown(&h, 1), "box.item.clock.button(text.value=\"09:05\")");
    let batches = tick_batches(&h);
    h.advance(40_000);
    assert_eq!(h.texts(1), vec!["09:05"]);
    assert_eq!(tick_batches(&h), batches, "nothing is drawn between minutes");
    h.advance(12_000);
    assert_eq!(h.texts(1), vec!["09:05"], "07.25 + 52 s is still 09:05:59");
    h.advance(750);
    assert_eq!(h.texts(1), vec!["09:06"], "on the boundary");

    let click = click_handler(h.root(1).unwrap());
    h.click(1, click);
    assert_eq!(h.texts(1), vec!["Tue 24 Nov 09:06:00"]);
    h.advance(1000);
    assert_eq!(h.texts(1), vec!["Tue 24 Nov 09:06:01"], "the alt format has seconds, so it ticks every second");
    h.advance(1000);
    assert_eq!(h.texts(1), vec!["Tue 24 Nov 09:06:02"]);
    h.click(1, click);
    assert_eq!(h.texts(1), vec!["09:06"]);
    let batches = tick_batches(&h);
    h.advance(5000);
    assert_eq!(tick_batches(&h), batches, "back to a minute clock: no second ticks");
}

#[test]
fn clock_options_timezone_format_and_no_alt_format() {
    let mut h = bench();
    h.start_specs(vec![
        Harness::spec_with(1, "clock#utc", &[("timezone", s("utc")), ("format", s("HH:mm 'UTC'"))]),
        Harness::spec_with(2, "clock#plain", &[("alt_format", s("")), ("format", s("h:mm tt"))]),
        Harness::spec_with(3, "clock#ist", &[("timezone", s("+05:30"))]),
    ]);
    assert_eq!(h.texts(1), vec!["23:05 UTC"], "09:05 at +10:00 is 23:05 the day before in UTC");
    assert_eq!(shown(&h, 2), "box.item.clock(text.value=\"9:05 AM\")", "no alt format: no click handler");
    assert!(h.root(2).unwrap().handlers.click.is_none());
    assert_eq!(h.texts(3), vec!["04:35"], "23:05 UTC plus 5:30");
}

#[test]
fn clock_follows_a_wall_clock_jump_at_once() {
    let mut h = bench();
    start(&mut h, "clock", &[]);
    let mut later = start_time();
    later.hour = 14;
    later.minute = 40;
    later.second = 5;
    h.clock.set_wall(later);
    h.send(ScriptIn::ClockChanged);
    assert_eq!(h.texts(1), vec!["14:40"]);
}

#[test]
fn date_shows_the_day_and_ticks_hourly() {
    let mut h = bench();
    start(&mut h, "date", &[]);
    assert_eq!(shown(&h, 1), "box.item.date(text.value=\"Tue 24 Nov\")");
    let batches = tick_batches(&h);
    h.advance(20 * 60_000);
    assert_eq!(tick_batches(&h), batches, "no tick before the hour");
    let mut late = start_time();
    late.hour = 23;
    late.minute = 59;
    late.second = 30;
    h.clock.set_wall(late);
    h.send(ScriptIn::ClockChanged);
    assert_eq!(h.texts(1), vec!["Tue 24 Nov"]);
    h.advance(31_000);
    assert_eq!(h.texts(1), vec!["Wed 25 Nov"], "the hour boundary is midnight");
}

// ---------------------------------------------------------------- workspaces

fn pills(h: &Harness, id: u32) -> Vec<String> {
    h.root(id).unwrap().children.iter().filter(|c| c.classes.iter().any(|k| &**k == "ws")).map(show).collect()
}

#[test]
fn workspaces_nothing_without_a_window_manager() {
    let mut h = bench();
    start(&mut h, "workspaces", &[]);
    assert_eq!(shown(&h, 1), "box.workspaces~()", "before the first reading");
    h.send(ok(ServiceId::Workspaces, no_workspaces()));
    assert_eq!(shown(&h, 1), "box.workspaces~()", "provider none: no pills and no space");
    h.send(ok(ServiceId::Workspaces, workspaces_value()));
    assert!(!shown(&h, 1).contains('~'), "{}", shown(&h, 1));
    h.send(ok(ServiceId::Workspaces, no_workspaces()));
    assert_eq!(shown(&h, 1), "box.workspaces~()", "and it goes away again");
}

#[test]
fn workspaces_pills_for_the_bars_own_monitor() {
    let mut h = bench();
    start(&mut h, "workspaces", &[]);
    h.send(ok(ServiceId::Workspaces, workspaces_value()));
    assert_eq!(
        pills(&h, 1),
        vec![
            "box.ws.focused.button(text=\"1\")",
            "box.ws.occupied.button(text=\"2\")",
            "box.ws.empty.button(text=\"3\")",
            "box.ws.empty.button(text=\"4\")",
        ]
    );
}

#[test]
fn workspaces_all_monitors_hide_empty_and_name_labels() {
    let mut h = bench();
    h.start_specs(vec![
        Harness::spec_with(1, "workspaces#all", &[("monitor", s("all"))]),
        Harness::spec_with(2, "workspaces#busy", &[("hide_empty", b(true))]),
    ]);
    h.send(ok(ServiceId::Workspaces, workspaces_value()));
    assert_eq!(pills(&h, 1).len(), 6);
    assert_eq!(pills(&h, 1)[4], "box.ws.visible.button(text=\"5\")", "visible on the other monitor");
    assert_eq!(
        pills(&h, 2),
        vec!["box.ws.focused.button(text=\"1\")", "box.ws.occupied.button(text=\"2\")"],
        "empty workspaces that are neither focused nor visible are left out"
    );
}

#[test]
fn workspaces_find_the_own_monitor_by_handle_then_device_then_index() {
    let monitor = |index: i64, handle: Option<i64>, device: &str, name: &str| {
        let mut pairs = vec![
            ("index", Value::Int(index)),
            ("name", s("m")),
            ("device", s(device)),
            ("workspaces", Value::List(vec![ws_entry(name, true, true, 1)])),
        ];
        if let Some(hd) = handle {
            pairs.push(("handle", Value::Int(hd)));
        }
        map(&pairs)
    };
    let value = |monitors: Vec<Value>| {
        map(&[("provider", s("glazewm")), ("connected", b(true)), ("monitors", Value::List(monitors))])
    };
    // The harness monitor: index 0, handle 1, device \\.\DISPLAY1.
    let cases = [
        // handle wins over a device and an index that point elsewhere
        (
            vec![monitor(0, Some(9), "\\\\.\\DISPLAY9", "x"), monitor(1, Some(1), "\\\\.\\DISPLAY7", "by-handle")],
            "by-handle",
        ),
        // no handle matches: the device name, compared without case
        (
            vec![monitor(0, Some(9), "\\\\.\\DISPLAY9", "x"), monitor(1, Some(8), "\\\\.\\display1", "by-device")],
            "by-device",
        ),
        // neither: the position
        (
            vec![monitor(1, Some(9), "\\\\.\\DISPLAY9", "x"), monitor(0, None, "\\\\.\\DISPLAY8", "by-index")],
            "by-index",
        ),
    ];
    for (monitors, want) in cases {
        let mut h = bench();
        start(&mut h, "workspaces", &[]);
        h.send(ok(ServiceId::Workspaces, value(monitors)));
        assert_eq!(pills(&h, 1), vec![format!("box.ws.focused.button(text=\"{want}\")")]);
    }
    // No match at all: every monitor's workspaces show, never a blank widget.
    let mut h = bench();
    start(&mut h, "workspaces", &[]);
    h.send(ok(
        ServiceId::Workspaces,
        value(vec![monitor(3, Some(9), "\\\\.\\DISPLAY9", "a"), monitor(4, Some(8), "\\\\.\\DISPLAY8", "b")]),
    ));
    assert_eq!(pills(&h, 1).len(), 2);
}

#[test]
fn workspaces_click_focuses_and_the_wheel_steps() {
    let mut h = bench();
    start(&mut h, "workspaces", &[]);
    h.send(ok(ServiceId::Workspaces, workspaces_value()));
    h.take_requests();
    let second = h.root(1).unwrap().children[1].handlers.click.unwrap().0;
    h.click(1, second);
    wheel(&mut h, 1, 1.0);
    wheel(&mut h, 1, -1.0);
    assert_eq!(
        commands(&h),
        vec![
            ServiceCommand::Workspaces(WorkspaceCommand::Focus("2".into())),
            ServiceCommand::Workspaces(WorkspaceCommand::FocusPrevious(Some(0))),
            ServiceCommand::Workspaces(WorkspaceCommand::FocusNext(Some(0))),
        ]
    );
    let mut h = bench();
    h.start_specs(vec![
        Harness::spec_with(1, "workspaces#all", &[("monitor", s("all"))]),
        Harness::spec_with(2, "workspaces#still", &[("wheel", b(false))]),
    ]);
    h.send(ok(ServiceId::Workspaces, workspaces_value()));
    h.take_requests();
    wheel(&mut h, 1, -1.0);
    assert_eq!(commands(&h), vec![ServiceCommand::Workspaces(WorkspaceCommand::FocusNext(None))]);
    assert!(h.root(2).unwrap().handlers.wheel.is_none(), "wheel = false: no wheel handler");
}

#[test]
fn workspaces_stale_shows_dashes_not_old_pills() {
    let mut h = bench();
    start(&mut h, "workspaces", &[]);
    h.send(ok(ServiceId::Workspaces, workspaces_value()));
    h.send(stale(ServiceId::Workspaces));
    assert_eq!(shown(&h, 1), "box.workspaces.stale(box.ws.empty(text=\"--\"))");
    h.send(ok(ServiceId::Workspaces, workspaces_value()));
    assert_eq!(pills(&h, 1).len(), 4, "and the pills are back");
}

// ---------------------------------------------------------------- text

fn exec_requests(h: &Harness) -> Vec<(rsb_core::io::IoId, rsb_core::io::ExecSpec)> {
    h.hub
        .lock()
        .unwrap()
        .iter()
        .filter_map(|r| match r {
            HubRequest::Io { id, request: IoRequest::ExecRun(spec), .. } => Some((*id, spec.clone())),
            _ => None,
        })
        .collect()
}

fn exec_ok(stdout: &str) -> IoEventKind {
    IoEventKind::ExecDone(Ok(ExecResult {
        code: Some(0),
        stdout: stdout.into(),
        stderr: String::new(),
        timed_out: false,
        truncated: false,
    }))
}

fn exec_fail() -> IoEventKind {
    IoEventKind::ExecDone(Ok(ExecResult {
        code: Some(1),
        stdout: String::new(),
        stderr: "no".into(),
        timed_out: false,
        truncated: false,
    }))
}

#[test]
fn text_static_text_and_nothing_to_show() {
    let mut h = bench();
    h.start_specs(vec![
        Harness::spec_with(1, "text#a", &[("text", s("Build OK")), ("class", s("mine"))]),
        Harness::spec(2, "text#b"),
    ]);
    assert_eq!(shown(&h, 1), "box.item.text.mine(text.value=\"Build OK\")");
    assert_eq!(shown(&h, 2), "box.item.text~(text.value=\"\")", "no text and no command: hidden");
    assert!(h.root(1).unwrap().handlers.click.is_none());
    assert!(exec_requests(&h).is_empty(), "nothing runs unless exec is set");
}

#[test]
fn text_runs_the_command_every_period_and_shows_the_first_line() {
    let mut h = bench();
    start(
        &mut h,
        "text",
        &[("exec", Value::List(vec![s("git"), s("branch"), s("--show-current")])), ("format", s("on {output}"))],
    );
    let reqs = exec_requests(&h);
    assert_eq!(reqs.len(), 1, "the first run starts at once");
    assert_eq!(reqs[0].1.program, rsb_core::io::ProgramRef::Name("git".into()));
    assert_eq!(reqs[0].1.args, vec!["branch", "--show-current"]);
    assert_eq!(h.texts(1), vec!["--"], "waiting for the first answer shows dashes without the stale class");
    assert_eq!(h.root(1).unwrap().classes.iter().map(|c| c.to_string()).collect::<Vec<_>>(), vec!["item", "text"]);
    h.io_reply(reqs[0].0, 1, exec_ok("main\nsecond line\n"));
    assert_eq!(shown(&h, 1), "box.item.text(text.value=\"on main\")");
    assert!(log_lines("granted text:").iter().any(|l| l.contains("exec:git")), "{:?}", log_lines("granted text:"));

    h.take_requests();
    h.advance(30_000);
    let again = exec_requests(&h);
    assert_eq!(again.len(), 1, "every 30 s by default");
    h.io_reply(again[0].0, 1, exec_fail());
    assert_eq!(shown(&h, 1), "box.item.text.stale(text.value=\"--\")", "a failing command shows dashes");
    h.take_requests();
    h.advance(30_000);
    let third = exec_requests(&h);
    h.io_reply(third[0].0, 1, exec_ok("dev"));
    assert_eq!(shown(&h, 1), "box.item.text(text.value=\"on dev\")", "and recovers");
}

#[test]
fn text_a_string_command_is_split_by_the_windows_rules_and_never_a_shell() {
    let mut h = bench();
    start(&mut h, "text", &[("exec", s("git log --format=\"%h %s\" -n 1 a\\\"b")), ("every", s("1m"))]);
    let reqs = exec_requests(&h);
    assert_eq!(reqs[0].1.program, rsb_core::io::ProgramRef::Name("git".into()));
    assert_eq!(reqs[0].1.args, vec!["log", "--format=%h %s", "-n", "1", "a\"b"]);
    assert!(reqs[0].1.raw_tail.is_none(), "no cmd.exe in between");
}

#[test]
fn text_click_runs_its_own_command_and_refreshes() {
    let mut h = bench();
    start(&mut h, "text", &[("exec", s("git branch")), ("click", s("git fetch --all"))]);
    let first = exec_requests(&h);
    h.io_reply(first[0].0, 1, exec_ok("main"));
    assert_eq!(shown(&h, 1), "box.item.text.button(text.value=\"main\")");
    h.take_requests();
    let click = click_handler(h.root(1).unwrap());
    h.click(1, click);
    let run = exec_requests(&h);
    assert_eq!(run.len(), 1);
    assert_eq!(run[0].1.args, vec!["fetch", "--all"]);
    h.io_reply(run[0].0, 1, exec_ok(""));
    let refreshed = exec_requests(&h);
    assert_eq!(refreshed.len(), 2, "after the click the output is read again");
    assert_eq!(refreshed[1].1.args, vec!["branch"]);
}

#[test]
fn text_without_a_user_set_command_is_denied_any_program() {
    // `exec` only grants what the user wrote in bar.toml: a widget with no
    // command set has no exec right at all.
    let mut h = bench();
    start(&mut h, "text#bare", &[("text", s("hi"))]);
    let granted = log_lines("granted text#bare:");
    assert_eq!(granted.len(), 1);
    assert!(granted.iter().all(|l| !l.contains("exec:")), "{granted:?}");
}

// ---------------------------------------------------------------- permissions, errors, hygiene

const BUILTINS: [&str; 11] =
    ["workspaces", "title", "clock", "date", "cpu", "memory", "battery", "volume", "network", "media", "text"];

#[test]
fn every_builtin_declares_exactly_the_permissions_it_uses() {
    let want = [
        ("workspaces", "control:workspaces"),
        ("title", "read:window"),
        ("clock", "nothing"),
        ("date", "nothing"),
        ("cpu", "nothing"),
        ("memory", "nothing"),
        ("battery", "nothing"),
        ("volume", "control:audio"),
        ("network", "read:network"),
        ("media", "control:media, read:media"),
        // `text` asks for the programs the user names in bar.toml and nothing else
        ("text", "nothing"),
    ];
    assert_eq!(want.len(), BUILTINS.len());
    for (name, grant) in want {
        let mut h = bench();
        start(&mut h, &format!("{name}#grants"), &[]);
        let lines = log_lines(&format!("granted {name}#grants:"));
        assert_eq!(lines, vec![format!("INFO granted {name}#grants: {grant}")], "{name}");
    }
    // text: what the user names is what it may run, resolved and logged.
    let mut h = bench();
    start(
        &mut h,
        "text#named",
        &[("exec", s("git status")), ("click", Value::List(vec![s("cmd"), s("/d"), s("/c"), s("ver")]))],
    );
    let lines = log_lines("granted text#named:");
    assert_eq!(lines.len(), 1);
    // Arguments in bar.toml mean exactly that command line is granted, not the whole program.
    assert!(lines[0].contains("exec:git status (only these arguments)"), "{lines:?}");
    assert!(lines[0].contains("exec:cmd /d /c ver (only these arguments)"), "{lines:?}");
    assert!(!lines[0].contains("$option"), "{lines:?}");
}

#[test]
fn a_wrong_option_is_a_warning_with_its_bar_toml_line_and_the_default_is_used() {
    let mut h = bench();
    start(&mut h, "cpu", &[("format", num(5.0)), ("colour", s("red"))]);
    h.send(ok(ServiceId::System, system(23.4, 41.0)));
    assert_eq!(h.texts(1), vec!["23%"], "the default format stays");
    let msgs: Vec<String> = h.diags.iter().map(|d| d.to_string()).collect();
    assert!(
        msgs.iter()
            .any(|m| m.contains("bar.toml:7:3") && m.contains("option 'format' of widget 'cpu' must be a string")),
        "{msgs:?}"
    );
    assert!(msgs.iter().any(|m| m.contains("unknown option 'colour' for widget 'cpu'")), "{msgs:?}");
    // Both problems are in the log too, each with the bar.toml line.
    let wrong = log_lines("option 'format' of widget 'cpu' must be a string");
    assert_eq!(wrong.len(), 1, "{wrong:?}");
    assert!(wrong[0].starts_with("WARN") && wrong[0].contains("bar.toml:7:3"), "{wrong:?}");
    let unknown = log_lines("unknown option 'colour' for widget 'cpu'");
    assert_eq!(unknown.len(), 1, "{unknown:?}");
    assert!(unknown[0].starts_with("WARN") && unknown[0].contains("bar.toml:"), "{unknown:?}");
}

#[test]
fn a_bad_setting_shows_a_plate_and_the_other_widgets_keep_updating() {
    let mut h = bench();
    h.start_specs(vec![Harness::spec_with(1, "clock", &[("timezone", s("Mars"))]), Harness::spec(2, "cpu")]);
    assert_eq!(
        h.plate(1).as_deref(),
        Some(
            "RSB ERROR - bad time zone 'Mars': use \"local\", \"utc\" or an offset like \"+10:00\"; restarting - check its settings in bar.toml"
        )
    );
    assert!(matches!(h.state_of(1), WidgetState::Stopped(_)));
    h.send(ok(ServiceId::System, system(42.0, 10.0)));
    assert_eq!(h.texts(2), vec!["42%"]);
    // The built-in restarts by itself and shows the plate again until the
    // setting is fixed.
    h.advance(1500);
    assert!(h.plate(1).is_some());
    assert!(h.status().iter().find(|w| w.instance == InstanceId(1)).unwrap().restarts >= 1);
}

#[test]
fn an_unexpected_service_value_is_logged_and_marked_with_the_error_class() {
    let mut h = bench();
    start(&mut h, "workspaces", &[]);
    h.send(ok(ServiceId::Workspaces, workspaces_value()));
    // Not a shape the schema allows: the hub would have dropped it.
    h.send(ok(
        ServiceId::Workspaces,
        map(&[("provider", s("glazewm")), ("connected", b(true)), ("monitors", num(5.0))]),
    ));
    assert!(h.state_of(1) == WidgetState::Running, "the widget keeps running");
    let status = h.status();
    assert!(status[0].errors >= 1, "{status:?}");
    let root = h.root(1).expect("the last good frame stays");
    assert!(root.classes.iter().any(|c| &**c == "error"), "{:?}", root.classes);
    // Ten seconds on, the class goes.
    h.send(ok(ServiceId::Workspaces, workspaces_value()));
    h.advance(11_000);
    h.send(ok(ServiceId::Workspaces, workspaces_value()));
    assert!(!h.root(1).unwrap().classes.iter().any(|c| &**c == "error"));
}

#[test]
fn no_builtin_raises_an_error_before_its_first_reading_or_in_any_state() {
    let mut h = bench();
    let specs: Vec<InstanceSpec> =
        BUILTINS.iter().enumerate().map(|(i, name)| Harness::spec(i as u32 + 1, name)).collect();
    h.start_specs(specs);
    let check = |h: &mut Harness, when: &str| {
        for w in h.status() {
            assert_eq!(w.state, WidgetState::Running, "{} {when}", w.widget);
            assert_eq!(w.errors, 0, "{} {when}", w.widget);
        }
        for i in 1..=BUILTINS.len() as u32 {
            if let Some(root) = h.root(i) {
                assert!(!root.classes.iter().any(|c| &**c == "error"), "{} {when}", BUILTINS[i as usize - 1]);
            }
            assert!(h.plate(i).is_none(), "{} {when}", BUILTINS[i as usize - 1]);
        }
    };
    check(&mut h, "before any reading");
    for scenario in Scenario::ALL {
        send_all(&mut h, scenario.events());
        check(&mut h, scenario.name());
    }
    // Every state again in the other direction, and a service that goes
    // back to starting, silent and live.
    for scenario in Scenario::ALL.iter().rev() {
        send_all(&mut h, scenario.events());
        check(&mut h, scenario.name());
    }
}

#[test]
fn builtin_sources_are_strict_use_only_the_public_module_and_leave_nothing_open() {
    let root = assets().join("widgets");
    let mut files = Vec::new();
    for dir in [root.clone(), root.join("lib")] {
        for e in std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()) {
            if e.path().extension().is_some_and(|x| x == "luau") {
                files.push(e.path());
            }
        }
    }
    assert!(files.len() >= BUILTINS.len() + 2, "{files:?}");
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap();
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        assert!(src.starts_with("--!strict\n"), "{name} is not strict");
        for bad in ["todo", "TODO", "FIXME", "XXX", "loadstring", "getfenv", "setfenv", "(rsb :: any)"] {
            assert!(!src.contains(bad), "{name} mentions {bad}");
        }
        for line in src.lines().filter(|l| l.contains("require(")) {
            assert!(line.contains("require(\"@rsb\")") || line.contains("require(\"./lib/"), "{name}: {line}");
        }
        assert!(src.is_ascii() || name == "weather.luau", "{name} has non-ASCII characters");
    }
    for name in BUILTINS {
        assert!(root.join(format!("{name}.luau")).exists(), "{name}");
    }
    let store = DiskStore {
        root: assets(),
        user: Arc::new(MemStore { user: Default::default(), builtin: Default::default() }),
    };
    let mut listed = store.builtin_widgets();
    listed.sort();
    let mut want: Vec<String> = BUILTINS.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(listed, want);
}

/// Corpus for the command line splitter: the Luau helper the text widget uses
/// must agree with the Rust function that grants the program.
#[test]
fn the_luau_command_splitter_agrees_with_the_one_that_grants_the_program() {
    let lines = [
        "git status",
        "  git   status  --short ",
        "\"C:\\Program Files\\Git\\bin\\git.exe\" status",
        "C:\\tools\\x.exe a\\b c\\\\d",
        "tool \"two words\" plain",
        "tool \"a\"\"b\"",
        "tool a\\\"b",
        "tool \"a\\\\\" b",
        "tool \"a\\\\\\\" b\"",
        "tool \\\\\\\"x",
        "tool a\"b c\"d e",
        "tool \"unterminated words",
        "\"quoted program\" arg",
        "\"unterminated program",
        "",
        "   ",
        "x",
        "tool\ttab\tseparated",
        "tool \"\" empty",
        "tool \"\"\"\"",
        "tool a | b & c %PATH%",
        "tool \\",
        "tool \\\\\"",
    ];
    let mut h = bench();
    h.user(
        "widgets/split.luau",
        "local rsb = require(\"@rsb\")\nlocal cmdline = require(\"./lib/cmdline\")\nreturn rsb.widget {\n  options = { line = \"\" },\n  permissions = {},\n  setup = function(props, ctx)\n    return rsb.text(table.concat(cmdline.split(props.line), \"\\1\"))\n  end,\n}\n",
    );
    for (i, line) in lines.iter().enumerate() {
        h.start_specs(vec![Harness::spec_with(100 + i as u32, "split", &[("line", s(line))])]);
        let got: Vec<String> = h
            .root(100 + i as u32)
            .and_then(|r| r.text.clone())
            .map(|t| t.to_string())
            .unwrap_or_default()
            .split('\u{1}')
            .map(str::to_string)
            .collect();
        let want = rsb_core::io::split_command_line(line);
        let got = if got == [""] && want.is_empty() { Vec::new() } else { got };
        assert_eq!(got, want, "line {line:?}");
    }
}

// ---------------------------------------------------------------- presets and templates

const PRESETS: [&str; 3] = ["default", "astur-classic", "glass"];

fn asset_text(rel: &str) -> String {
    std::fs::read_to_string(assets().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn preset_toml(name: &str) -> Option<String> {
    std::fs::read_to_string(assets().join("presets").join(format!("{name}.toml"))).ok()
}

/// The config a user gets by writing `preset = "<name>"` and nothing else.
fn load_preset(name: &str) -> Config {
    let user = format!("preset = \"{name}\"\n");
    let load = config::load_config(Some((&user, "bar.toml")), &preset_toml);
    assert!(load.diagnostics.is_empty(), "{name}: {:?}", load.diagnostics);
    load.config.expect("the preset loads")
}

/// base.css and the preset's css, as the binary stacks them.
fn preset_sheet(name: &str) -> Arc<Stylesheet> {
    let (mut sheet, diags) = Stylesheet::parse(&asset_text("themes/base.css"), "base.css");
    assert!(diags.is_empty(), "base.css: {diags:?}");
    let (preset, diags) = Stylesheet::parse(&asset_text(&format!("presets/{name}.css")), &format!("{name}.css"));
    assert!(diags.is_empty(), "{name}.css: {diags:?}");
    assert!(preset.imports().is_empty());
    sheet.append(preset);
    Arc::new(sheet)
}

#[test]
fn presets_templates_and_base_parse_with_no_diagnostics() {
    // No file at all gives the default preset.
    let load = config::load_config(None, &preset_toml);
    assert!(load.diagnostics.is_empty(), "{:?}", load.diagnostics);
    assert_eq!(load.config.unwrap().preset, "default");
    for name in PRESETS {
        let cfg = load_preset(name);
        assert_eq!(cfg.preset, name);
        assert_eq!(cfg.bars.len(), 1, "{name}");
        let _ = preset_sheet(name);
        assert!(config::BUILTIN_PRESETS.contains(&name));
    }
    // The first-run files.
    let template = asset_text("config/bar.toml");
    let load = config::load_config(Some((&template, "bar.toml")), &preset_toml);
    assert!(load.diagnostics.is_empty(), "bar.toml template: {:?}", load.diagnostics);
    let plain = |bars: Vec<BarSpec>| -> Vec<String> {
        bars.into_iter()
            .map(|mut b| {
                b.pos = None;
                for z in b.left.iter_mut().chain(b.center.iter_mut()).chain(b.right.iter_mut()) {
                    z.pos = None;
                }
                format!("{b:?}")
            })
            .collect()
    };
    assert_eq!(
        plain(load.config.unwrap().bars),
        plain(config::default_config().bars),
        "the template changes nothing until a line is uncommented"
    );
    let (sheet, diags) = Stylesheet::parse(&asset_text("config/style.css"), "style.css");
    assert!(diags.is_empty(), "style.css template: {diags:?}");
    assert_eq!(sheet.rule_count(), 0, "comments only");
    // Every line of the bar.toml template except config_version is a comment.
    for line in template.lines() {
        let t = line.trim();
        assert!(t.is_empty() || t.starts_with('#') || t == "config_version = 1", "{line}");
    }
    // Uncommenting the template's example lines gives a valid file too.
    let uncommented: String = template
        .lines()
        .map(|l| match l.strip_prefix("# ") {
            Some(rest) if rest.contains('=') || rest.starts_with("[[") || rest.starts_with("[widget") => {
                format!("{rest}\n")
            }
            _ => format!("{l}\n"),
        })
        .collect();
    let load = config::load_config(Some((&uncommented, "bar.toml")), &preset_toml);
    assert!(
        load.diagnostics.iter().all(|d| d.message.contains("not used by any bar")),
        "uncommented template: {:?}",
        load.diagnostics
    );
}

#[test]
fn preset_zones_and_bars_follow_the_design() {
    let default = load_preset("default");
    let bar = &default.bars[0];
    let names = |items: &[config::ZoneItem]| items.iter().map(|i| i.widget.to_string()).collect::<Vec<_>>();
    assert_eq!(names(&bar.left), ["workspaces", "title"]);
    assert_eq!(names(&bar.center), ["clock"]);
    assert_eq!(names(&bar.right), ["media", "network", "volume", "battery", "cpu", "memory", "date"]);
    assert_eq!((bar.height, bar.floating, bar.reserve), (32, false, true));

    let astur = load_preset("astur-classic");
    let bar = &astur.bars[0];
    assert_eq!(bar.height, 28);
    assert_eq!(names(&bar.left), ["workspaces"]);
    assert_eq!(names(&bar.center), ["title"]);
    assert_eq!(names(&bar.right), ["cpu", "memory", "volume", "battery", "clock"], "no layout widget");
    let label = |w: &str| astur.widgets[w].values["label"].value.clone();
    assert_eq!(label("cpu"), s("CPU"));
    assert_eq!(label("memory"), s("RAM"));
    assert_eq!(label("volume"), s("VOL"));
    assert_eq!(astur.widgets["volume"].values["muted_format"].value, s("MUTE"));
    assert_eq!(label("battery"), s("BAT"));
    assert_eq!(astur.widgets["clock"].values["format"].value, s("HH:mm"));

    let glass = load_preset("glass");
    let bar = &glass.bars[0];
    assert_eq!((bar.floating, bar.margin, bar.radius), (true, 8, 12));

    // A user bar under a preset keeps the preset's zones.
    let user = "preset = \"glass\"\n[[bar]]\nheight = 40\n";
    let load = config::load_config(Some((user, "bar.toml")), &preset_toml);
    let bar = &load.config.unwrap().bars[0];
    assert_eq!((bar.height, bar.floating, bar.margin), (40, true, 8));
    assert_eq!(names(&bar.right).len(), 7);
}

// ---------------------------------------------------------------- scenes and rendering

fn monitor_ref() -> MonitorRef {
    MonitorRef { index: 0, handle: 1, device: "\\\\.\\DISPLAY1".into(), primary: true, scale: 1.0 }
}

/// Options the way the binary builds them: the plain `[widget.x]` table
/// first, then the instance's own.
fn options_for(config: &Config, widget: &WidgetRef, extra: &[(&str, Value)]) -> WidgetOptions {
    let mut out = WidgetOptions::default();
    let plain = config.widgets.get(&widget.name);
    let own = config.widgets.get(&widget.to_string());
    for table in [plain, own].into_iter().flatten() {
        for (k, v) in &table.values {
            out.values.insert(k.clone(), v.clone());
        }
    }
    for (i, (k, v)) in extra.iter().enumerate() {
        out.values.insert(
            k.to_string(),
            rsb_core::config::Spanned {
                value: v.clone(),
                pos: rsb_core::diag::SourcePos::new("bar.toml", 90 + i as u32, 1),
            },
        );
    }
    out
}

/// A bar with its widgets run through the script runtime, fed one scenario.
struct Rig {
    harness: Harness,
    bar: BarSpec,
    slots: ZoneSlots,
}

fn rig(config: &Config, bar: &BarSpec, extra: &[(&str, Vec<(&str, Value)>)], scenario: Scenario) -> Rig {
    let mut h = bench();
    let mut specs = Vec::new();
    let mut slots = ZoneSlots::default();
    let mut next = 1u32;
    for (zone, items) in [(0, &bar.left), (1, &bar.center), (2, &bar.right)] {
        for item in items {
            let id = InstanceId(next);
            next += 1;
            let ex: Vec<(&str, Value)> =
                extra.iter().find(|(r, _)| *r == item.widget.to_string()).map(|(_, v)| v.clone()).unwrap_or_default();
            specs.push(InstanceSpec {
                instance: id,
                widget: item.widget.clone(),
                options: options_for(config, &item.widget, &ex),
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
    send_all(&mut h, scenario.events());
    Rig { harness: h, bar: bar.clone(), slots }
}

/// The preset's own bar.
fn native(config: &Config, scenario: Scenario) -> Rig {
    rig(config, &config.bars[0], &[], scenario)
}

/// Every built-in in one wide bar, workspaces twice (own monitor, all).
fn gallery(config: &Config, scenario: Scenario) -> Rig {
    let mut bar = config.bars[0].clone();
    let refs = |names: &[&str]| -> Vec<config::ZoneItem> {
        names.iter().map(|n| config::ZoneItem::new(WidgetRef::parse(n).unwrap())).collect()
    };
    bar.left = refs(&["workspaces", "workspaces#all", "title"]);
    bar.center = refs(&["clock", "date"]);
    bar.right = refs(&["text#note", "media", "network", "volume", "battery", "cpu", "memory"]);
    rig(
        config,
        &bar,
        &[("workspaces#all", vec![("monitor", s("all"))]), ("text#note", vec![("text", s("Build OK"))])],
        scenario,
    )
}

struct Drawn {
    draw: Arc<DrawList>,
    scene: Scene,
}

/// Runs a rig through a scene: stylesheet, slots, trees, plates, one frame.
fn draw(rig: &Rig, sheet: &Arc<Stylesheet>, dark: bool, renderer: &mut Renderer, width: u32, scale: f32) -> Drawn {
    let bar = &rig.bar;
    let pad = if bar.floating { 2 * bar.margin } else { 0 };
    let height = ((bar.height + pad) as f32 * scale).round() as u32;
    let env = SceneEnv { media: MediaEnv { dark }, icon_font: renderer.icon_font(), fullscreen: false };
    let mut scene = Scene::new(bar, BarGeometry { width_px: width, height_px: height, scale }, env);
    scene.set_stylesheet(sheet.clone());
    scene.set_slots(rig.slots.clone());
    for tree in rig.harness.latest.values() {
        scene.set_tree(tree.clone());
    }
    for (id, plate) in &rig.harness.plates {
        if let Some(p) = plate {
            scene.set_widget_plate(*id, Some(p.clone()));
        }
    }
    let out = scene.frame(Instant::now(), renderer);
    Drawn { draw: out.draw, scene }
}

fn renderer() -> Renderer {
    Renderer::new(FontConfig::default())
}

/// Text contrast of one draw list: every text item (icons included) against
/// the fills drawn under it, with translucent fills composited over black and
/// over white; the worse of the two counts. Restricted to items inside `within`.
fn contrast_of(draw: &DrawList, within: Option<RectF>) -> Vec<(String, f32)> {
    let mut out = Vec::new();
    for (i, item) in draw.items.iter().enumerate() {
        let DrawCmd::Text { rect, text, color, .. } = &item.cmd else { continue };
        if text.trim().is_empty() {
            continue;
        }
        if let Some(w) = within
            && !(rect.x >= w.x - 0.5
                && rect.right() <= w.right() + 0.5
                && rect.y >= w.y - 0.5
                && rect.bottom() <= w.bottom() + 0.5)
        {
            continue;
        }
        let centre = PointF { x: rect.x + rect.w / 2.0, y: rect.y + rect.h / 2.0 };
        let mut worst = f32::MAX;
        for base in [Color::BLACK, Color::WHITE] {
            let mut bg = base;
            for prev in &draw.items[..i] {
                match &prev.cmd {
                    DrawCmd::Fill { rect, fill, .. } if rect.contains(centre) => match fill {
                        Fill::Solid(c) => bg = c.over(bg),
                        Fill::Linear { .. } => panic!("a gradient behind text: contrast is not measured"),
                    },
                    DrawCmd::PushOpacity(_) => panic!("group opacity over text: contrast is not measured"),
                    _ => {}
                }
            }
            worst = worst.min(Color::contrast_ratio(color.over(bg), bg));
        }
        out.push((text.to_string(), worst));
    }
    out
}

#[test]
fn text_contrast_is_at_least_4_5_to_1_in_every_preset_theme_and_state() {
    let mut r = renderer();
    let mut report = Vec::new();
    let mut lines = Vec::new();
    for preset in PRESETS {
        let config = load_preset(preset);
        let sheet = preset_sheet(preset);
        for dark in [true, false] {
            let theme = if dark { "dark" } else { "light" };
            let mut worst = (f32::MAX, String::new());
            let mut count = 0usize;
            for scenario in Scenario::ALL {
                for (kind, rig) in [("gallery", gallery(&config, scenario)), ("bar", native(&config, scenario))] {
                    let mut d = draw(&rig, &sheet, dark, &mut r, 1920, 1.0);
                    let diags = d.scene.take_diagnostics();
                    assert!(diags.is_empty(), "{preset} {theme} {} {kind}: {diags:?}", scenario.name());
                    assert!(rig.harness.plates.values().all(|p| p.is_none()), "{preset} {scenario:?}");
                    for (text, ratio) in contrast_of(&d.draw, None) {
                        count += 1;
                        lines.push(format!("{preset} {theme} {} {kind} {text:?} {ratio:.2}", scenario.name()));
                        if ratio < worst.0 {
                            worst = (ratio, format!("{} {kind} {text:?}", scenario.name()));
                        }
                    }
                    // Hover: every clickable item once, after its transition.
                    if scenario == Scenario::Normal || scenario == Scenario::Alert {
                        let t0 = Instant::now();
                        let nodes = d.scene.nodes();
                        for n in nodes.iter().filter(|n| n.classes.iter().any(|c| &**c == "button")) {
                            let (x, y) = (n.rect.x + n.rect.w / 2.0, n.rect.y + n.rect.h / 2.0);
                            d.scene.pointer(PointerEvent { kind: PointerKind::Move, x, y }, t0);
                            let out = d.scene.frame(t0 + Duration::from_millis(500), &mut r);
                            for (text, ratio) in contrast_of(&out.draw, Some(n.rect)) {
                                count += 1;
                                lines.push(format!(
                                    "{preset} {theme} {} {kind} hover {text:?} {ratio:.2}",
                                    scenario.name()
                                ));
                                if ratio < worst.0 {
                                    worst = (ratio, format!("{} {kind} hover {text:?}", scenario.name()));
                                }
                            }
                            d.scene.pointer(PointerEvent { kind: PointerKind::Leave, x: 0.0, y: 0.0 }, t0);
                        }
                    }
                }
            }
            assert!(count > 100, "{preset} {theme}: only {count} text nodes measured");
            report.push(format!("{preset:14} {theme:5} {count:4} text nodes, lowest {:.2}:1 ({})", worst.0, worst.1));
            assert!(worst.0 >= 4.5, "{preset} {theme}: {:.2}:1 for {}", worst.0, worst.1);
        }
    }
    let dir = png_dir();
    std::fs::write(dir.join("contrast.txt"), lines.join("\n") + "\n").unwrap();
    eprintln!("{}", report.join("\n"));
}

#[test]
fn the_default_bar_leaves_the_same_gap_at_both_edges() {
    let mut r = renderer();
    let config = load_preset("default");
    let sheet = preset_sheet("default");
    let rig = native(&config, Scenario::Normal);
    let d = draw(&rig, &sheet, true, &mut r, 1920, 1.0);
    let nodes = d.scene.nodes();
    let has = |n: &rsb_core::scene::NodeInfo, class: &str| n.classes.iter().any(|c| &**c == class);
    let first_pill = nodes.iter().find(|n| has(n, "ws")).expect("a workspace pill");
    let last_item = nodes.iter().filter(|n| has(n, "item")).max_by(|a, b| a.rect.x.total_cmp(&b.rect.x)).unwrap();
    let left = first_pill.rect.x;
    let right = 1920.0 - (last_item.rect.x + last_item.rect.w - last_item.style.padding.right);
    assert_eq!(left, right, "first pill {left} px from the left edge, last item's text {right} px from the right");
}

#[test]
fn astur_classic_matches_astur_colours_sizes_and_zones() {
    let mut r = renderer();
    let config = load_preset("astur-classic");
    let sheet = preset_sheet("astur-classic");
    let color = |hex: &str| Color::parse(hex).unwrap();
    for dark in [true, false] {
        let (bg, fg, accent) = if dark { ("#1a1b26", "#c0caf5", "#66aaff") } else { ("#eceef2", "#1b1e24", "#336382") };
        let rig = native(&config, Scenario::Normal);
        let d = draw(&rig, &sheet, dark, &mut r, 1920, 1.0);
        let nodes = d.scene.nodes();
        let bar = &nodes[0];
        assert_eq!(bar.style.background, rsb_core::style::Background::Color(color(bg)), "dark={dark}");
        assert_eq!(bar.style.color, color(fg));
        assert_eq!(bar.rect.h, 28.0, "Astur's bar height");
        assert_eq!(bar.style.padding.left, 8.0, "Astur's bar padding");
        assert_eq!(bar.style.padding.right, 0.0, "the right widget's own 8 px is the edge");
        assert_eq!(bar.style.font_weight, 600, "Astur draws all text semibold");
        assert_eq!(bar.style.font_size, 14.0, "half the bar height");
        assert_eq!(bar.style.font_family[0].as_ref(), "Segoe UI");
        let has = |n: &rsb_core::scene::NodeInfo, class: &str| n.classes.iter().any(|c| &**c == class);
        let focused = nodes.iter().find(|n| has(n, "focused")).unwrap();
        assert_eq!(focused.style.background, rsb_core::style::Background::Color(color(accent)));
        assert_eq!(focused.rect.w, 34.0, "workspace pills are 34 px wide");
        let empty = nodes.iter().find(|n| has(n, "empty")).unwrap();
        assert_eq!(empty.style.border_color.a, 0.0, "Astur does not outline an empty workspace");
        assert_eq!(
            (empty.rect.h, empty.style.border_radius),
            (20.0, rsb_core::style::RadiusDims::ZERO),
            "square 20 px pills"
        );
        let items: Vec<_> = nodes.iter().filter(|n| has(n, "item")).collect();
        assert!(items.len() >= 5);
        for item in &items {
            assert_eq!(
                (item.style.padding.left, item.style.padding.right),
                (8.0, 8.0),
                "8 + 8 = Astur's 16 px between widgets"
            );
        }
        let labels: Vec<String> =
            nodes.iter().filter(|n| has(n, "label")).filter_map(|n| n.text.as_ref().map(|t| t.to_string())).collect();
        assert_eq!(labels, ["CPU", "RAM", "VOL", "BAT"], "text labels, not icons");
        assert!(!nodes.iter().any(|n| n.kind == ElementKind::Icon), "no icons in the classic bar");
        let wrappers: Vec<String> = nodes
            .iter()
            .filter(|n| n.kind == ElementKind::Widget)
            .map(|n| n.id.as_deref().unwrap_or("").to_string())
            .collect();
        assert_eq!(
            wrappers,
            ["workspaces", "title", "cpu", "memory", "volume", "battery", "clock"],
            "zones, in order; no layout widget"
        );
    }
}

// ---------------------------------------------------------------- pictures

fn wallpaper_px(dark: bool, x: u32, w: u32) -> [u8; 3] {
    let (a, b) = if dark { ([0x2a, 0x3a, 0x66], [0x7a, 0x4c, 0x7c]) } else { ([0xcf, 0xdc, 0xf0], [0xf3, 0xd9, 0xc5]) };
    let t = x as f32 / w.max(1) as f32;
    let mix = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8;
    [mix(0), mix(1), mix(2)]
}

/// Bars stacked over a flat-ish wallpaper, so translucent backgrounds show.
fn stack(rows: &[rsb_render::Canvas], dark: bool) -> rsb_render::Canvas {
    let gap = 6u32;
    let width = rows.iter().map(|c| c.width).max().unwrap();
    let height = rows.iter().map(|c| c.height + gap).sum::<u32>() + gap;
    let mut out = rsb_render::Canvas::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let [r, g, b] = wallpaper_px(dark, x, width);
            let i = (y as usize * width as usize + x as usize) * 4;
            out.data[i..i + 4].copy_from_slice(&[b, g, r, 255]);
        }
    }
    let mut y0 = gap;
    for row in rows {
        for y in 0..row.height {
            for x in 0..row.width {
                let si = (y as usize * row.width as usize + x as usize) * 4;
                let di = ((y0 + y) as usize * width as usize + x as usize) * 4;
                let a = row.data[si + 3] as u32;
                for c in 0..3 {
                    out.data[di + c] =
                        (row.data[si + c] as u32 + out.data[di + c] as u32 * (255 - a) / 255).min(255) as u8;
                }
            }
        }
        y0 += row.height + gap;
    }
    out
}

fn save(canvas: &rsb_render::Canvas, name: &str) -> PathBuf {
    let path = png_dir().join(name);
    std::fs::write(&path, canvas.to_png().expect("the PNG encodes")).expect("the PNG is written");
    path
}

/// A bar with one widget in the left zone, the preset's bar otherwise.
fn only(config: &Config, name: &str, scenario: Scenario) -> Rig {
    let mut bar = config.bars[0].clone();
    bar.left = vec![config::ZoneItem::new(WidgetRef::parse(name).unwrap())];
    bar.center.clear();
    bar.right.clear();
    rig(config, &bar, &[], scenario)
}

/// Pixels that differ clearly from the one at the top-left corner of the row.
fn ink(canvas: &rsb_render::Canvas) -> usize {
    let base = canvas.rgba_at(0, 0);
    let mut n = 0;
    for y in 0..canvas.height {
        for x in 0..canvas.width {
            let p = canvas.rgba_at(x, y);
            let d: i32 = (0..3).map(|c| (p[c] as i32 - base[c] as i32).abs()).sum();
            if d > 120 {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn every_builtin_and_every_preset_renders_to_a_picture() {
    let mut r = renderer();
    let mut written = Vec::new();
    for preset in PRESETS {
        let config = load_preset(preset);
        let sheet = preset_sheet(preset);
        for dark in [true, false] {
            let theme = if dark { "dark" } else { "light" };
            let mut rows = Vec::new();
            let d = draw(&native(&config, Scenario::Normal), &sheet, dark, &mut r, 1920, 1.0);
            rows.push(render_offscreen(&mut r, &d.draw).unwrap());
            for scenario in Scenario::ALL {
                let d = draw(&gallery(&config, scenario), &sheet, dark, &mut r, 1920, 1.0);
                rows.push(render_offscreen(&mut r, &d.draw).unwrap());
            }
            let path = save(&stack(&rows, dark), &format!("preset-{preset}-{theme}.png"));
            written.push(path);
        }
    }

    // Each built-in alone at 200 %, default preset, normal / alert / stale / empty.
    let config = load_preset("default");
    let sheet = preset_sheet("default");
    for dark in [true, false] {
        let theme = if dark { "dark" } else { "light" };
        for name in BUILTINS {
            let mut rows = Vec::new();
            for scenario in Scenario::ALL {
                let rig = if name == "text" {
                    let mut bar = config.bars[0].clone();
                    bar.left = vec![config::ZoneItem::new(WidgetRef::parse("text").unwrap())];
                    bar.center.clear();
                    bar.right.clear();
                    rig(&config, &bar, &[("text", vec![("text", s("Build OK"))])], scenario)
                } else {
                    only(&config, name, scenario)
                };
                let d = draw(&rig, &sheet, dark, &mut r, 640, 2.0);
                let canvas = render_offscreen(&mut r, &d.draw).unwrap();
                if scenario == Scenario::Normal {
                    assert!(ink(&canvas) > 40, "{name} draws nothing in its normal state");
                }
                rows.push(canvas);
            }
            written.push(save(&stack(&rows, dark), &format!("builtin-{name}-{theme}.png")));
        }
    }
    assert!(written.len() >= 6 + 22);
    for p in &written {
        let len = std::fs::metadata(p).unwrap().len();
        assert!(len > 1000, "{p:?} is {len} bytes");
    }
}

#[test]
fn plates_are_drawn_and_readable() {
    let mut r = renderer();
    let config = load_preset("default");
    let sheet = preset_sheet("default");
    let mut h = bench();
    h.start_specs(vec![Harness::spec_with(1, "clock", &[("timezone", s("Mars"))]), Harness::spec(2, "cpu")]);
    send_all(&mut h, vec![ok(ServiceId::System, system(30.0, 40.0))]);
    let mut bar = config.bars[0].clone();
    bar.left = vec![config::ZoneItem::new(WidgetRef::parse("clock").unwrap())];
    bar.center.clear();
    bar.right = vec![config::ZoneItem::new(WidgetRef::parse("cpu").unwrap())];
    let slots = ZoneSlots {
        left: vec![SlotSpec { instance: InstanceId(1), class: "clock".into(), id: "clock".into() }],
        center: vec![],
        right: vec![SlotSpec { instance: InstanceId(2), class: "cpu".into(), id: "cpu".into() }],
    };
    let rig = Rig { harness: h, bar, slots };
    for dark in [true, false] {
        let mut d = draw(&rig, &sheet, dark, &mut r, 1800, 1.0);
        d.scene.set_bar_plate(Some(Plate {
            kind: rsb_core::diag::DiagKind::Style,
            text: "RSB ERROR - style.css:12:5 unknown property 'colr'; did you mean 'color'?".into(),
            more: 1,
        }));
        let out = d.scene.frame(Instant::now() + Duration::from_millis(50), &mut r);
        let texts: Vec<String> = out
            .draw
            .items
            .iter()
            .filter_map(|i| match &i.cmd {
                DrawCmd::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t.starts_with("RSB ERROR - bad time zone 'Mars'")), "{texts:?}");
        assert!(
            texts.iter().any(|t| t.starts_with("RSB ERROR - style.css:12:5") && t.ends_with("(+1 more, see log)")),
            "{texts:?}"
        );
        assert!(texts.iter().any(|t| t == "30%"), "the other widget keeps drawing: {texts:?}");
        for (text, ratio) in contrast_of(&out.draw, None) {
            assert!(ratio >= 4.5, "{text:?} {ratio:.2}");
        }
        let canvas = render_offscreen(&mut r, &out.draw).unwrap();
        save(&stack(&[canvas], dark), &format!("plates-{}.png", if dark { "dark" } else { "light" }));
    }
}

#[test]
fn every_icon_glyph_draws_ink_in_the_icon_font_found() {
    let mut r = renderer();
    let font = r.icon_font();
    let mut root = Element::new(ElementKind::Box);
    for name in ICON_NAMES {
        let mut icon = Element::new(ElementKind::Icon);
        icon.icon = Some(IconRef::Named(Arc::from(*name)));
        root.children.push(icon);
    }
    let mut bar = config::default_config().bars.remove(0);
    bar.height = 40;
    let width = 48 * ICON_NAMES.len() as u32;
    let env = SceneEnv { media: MediaEnv { dark: true }, icon_font: font, fullscreen: false };
    let mut scene = Scene::new(&bar, BarGeometry { width_px: width, height_px: 40, scale: 1.0 }, env);
    let (sheet, diags) = Stylesheet::parse(
        "bar { background: #000000; color: #ffffff } icon { font-size: 22px; margin: 0 8px }",
        "t.css",
    );
    assert!(diags.is_empty());
    scene.set_stylesheet(Arc::new(sheet));
    scene.set_slots(ZoneSlots {
        left: vec![SlotSpec { instance: InstanceId(1), class: "w".into(), id: "w".into() }],
        ..Default::default()
    });
    scene.set_tree(Arc::new(rsb_core::element::WidgetTree {
        instance: InstanceId(1),
        widget: WidgetRef::parse("w").unwrap(),
        version: 1,
        root: Some(root),
        grant: None,
    }));
    let out = scene.frame(Instant::now(), &mut r);
    let canvas = render_offscreen(&mut r, &out.draw).unwrap();
    let nodes: Vec<_> = scene.nodes().into_iter().filter(|n| n.kind == ElementKind::Icon).collect();
    assert_eq!(nodes.len(), ICON_NAMES.len());
    let mut weakest = (usize::MAX, "");
    for (node, name) in nodes.iter().zip(ICON_NAMES) {
        let mut lit = 0usize;
        for y in node.rect.y as u32..(node.rect.y + node.rect.h) as u32 {
            for x in node.rect.x as u32..(node.rect.x + node.rect.w) as u32 {
                let p = canvas.rgba_at(x, y);
                if p[0] > 96 || p[1] > 96 || p[2] > 96 {
                    lit += 1;
                }
            }
        }
        assert!(lit >= 12, "{name} draws {lit} lit pixels in {font:?}");
        if lit < weakest.0 {
            weakest = (lit, name);
        }
    }
    save(&stack(&[canvas], true), "icons.png");
    eprintln!(
        "icon font {font:?}: all {} glyphs draw ink; the faintest is {} with {} lit pixels",
        ICON_NAMES.len(),
        weakest.1,
        weakest.0
    );
    assert!(matches!(font, IconFont::Fluent | IconFont::Mdl2));
}

#[test]
fn a_transparent_bar_keeps_alpha_one_in_every_pixel_of_its_box() {
    let mut r = renderer();
    for (preset, scale) in
        [("default", 1.0f32), ("default", 1.25), ("default", 2.0), ("glass", 1.0), ("astur-classic", 1.5)]
    {
        let config = load_preset(preset);
        let mut sheet = (*preset_sheet(preset)).clone();
        let (user, diags) = Stylesheet::parse("bar, zone, .item, .ws { background: transparent }", "style.css");
        assert!(diags.is_empty());
        sheet.append(user);
        let rig = native(&config, Scenario::Normal);
        let d = draw(&rig, &Arc::new(sheet), true, &mut r, 1280, scale);
        let canvas = render_offscreen(&mut r, &d.draw).unwrap();
        let bar = d.scene.nodes()[0].rect;
        let radius = match d.scene.nodes()[0].style.border_radius.tl {
            rsb_core::style::Dim::Px(v) => v * scale,
            _ => 0.0,
        };
        // The corners of a rounded bar are outside its shape; everything else is the bar.
        let outside_corner = |x: f32, y: f32| -> bool {
            if radius < 1.0 {
                return false;
            }
            let cx = if x < bar.x + radius {
                bar.x + radius
            } else if x > bar.right() - radius {
                bar.right() - radius
            } else {
                x
            };
            let cy = if y < bar.y + radius {
                bar.y + radius
            } else if y > bar.bottom() - radius {
                bar.bottom() - radius
            } else {
                y
            };
            ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() > radius - 1.0
        };
        let mut least = 255u8;
        let mut pixels = 0u32;
        for y in bar.y as u32..(bar.y + bar.h) as u32 {
            for x in bar.x as u32..(bar.x + bar.w) as u32 {
                if outside_corner(x as f32 + 0.5, y as f32 + 0.5) {
                    continue;
                }
                let a = canvas.rgba_at(x, y)[3];
                assert!(a >= 1, "{preset} at {scale}: pixel {x},{y} is fully transparent");
                least = least.min(a);
                pixels += 1;
            }
        }
        eprintln!("{preset} at {scale}x: {pixels} bar pixels, lowest alpha {least}");
    }
}

// ---------------------------------------------------------------- docs

fn example(name: &str) -> String {
    std::fs::read_to_string(docs().join("examples").join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn io_of(h: &Harness) -> Vec<(rsb_core::io::IoId, IoRequest)> {
    h.hub
        .lock()
        .unwrap()
        .iter()
        .filter_map(|r| match r {
            HubRequest::Io { id, request, .. } => Some((*id, request.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn the_examples_are_strict_and_name_the_bar_toml_lines_that_turn_them_on() {
    for (file, enable) in [
        ("weather.luau", "right = [\"my/weather\""),
        ("git-status.luau", "right = [\"git-status\""),
        ("counter.luau", "right = [\"counter\""),
    ] {
        let src = example(file);
        assert!(src.starts_with("--!strict\n"), "{file}");
        assert!(src.contains("require(\"@rsb\")"), "{file}");
        assert!(src.contains(enable), "{file} must show the bar.toml line {enable}");
        assert!(src.contains("-- Save as widgets"), "{file}");
    }
}

#[test]
fn the_weather_example_fetches_shows_the_temperature_and_opens_the_page() {
    let mut h = bench();
    h.user("widgets/my/weather.luau", &example("weather.luau"));
    h.start(&[(1, "my/weather")]);
    assert_eq!(shown(&h, 1), "box.item.weather.button(icon:cloud text.value=\"--\")", "{:?}", h.plate(1));
    let reqs = io_of(&h);
    assert_eq!(reqs.len(), 1);
    let IoRequest::Http(req) = &reqs[0].1 else { panic!("{:?}", reqs[0]) };
    assert_eq!(
        req.url,
        "https://api.open-meteo.com/v1/forecast?latitude=-30.51&longitude=151.67&current=temperature_2m"
    );
    assert!(log_lines("granted my/weather:").iter().any(|l| l.contains("net:api.open-meteo.com")));
    let body = br#"{"current":{"temperature_2m":21.4}}"#.to_vec();
    h.io_reply(
        reqs[0].0,
        1,
        IoEventKind::Http(Ok(HttpResponse { status: 200, headers: vec![], body, from_cache: false })),
    );
    assert_eq!(shown(&h, 1), "box.item.weather.button(icon:cloud text.value=\"21\u{b0}\")");
    h.take_requests();
    let click = click_handler(h.root(1).unwrap());
    h.click(1, click);
    assert!(io_of(&h).iter().any(|(_, r)| matches!(r, IoRequest::OpenUrl(u) if u == "https://open-meteo.com")));

    // A failed request: dashes and the class stale, no crash.
    let mut h = bench();
    h.user("widgets/my/weather.luau", &example("weather.luau"));
    h.start(&[(1, "my/weather")]);
    let id = io_of(&h)[0].0;
    h.io_reply(id, 1, IoEventKind::Http(Err(IoError::Timeout)));
    assert_eq!(shown(&h, 1), "box.item.weather.stale(icon:cloud text.value=\"--\")");
    assert!(h.state_of(1) == WidgetState::Running);
}

#[test]
fn the_weather_examples_coordinates_choose_the_place_and_there_is_no_city_option() {
    let mut h = bench();
    h.user("widgets/my/weather.luau", &example("weather.luau"));
    h.start_specs(vec![Harness::spec_with(
        1,
        "my/weather",
        &[("latitude", num(-42.88)), ("longitude", num(147.33)), ("city", s("Hobart"))],
    )]);
    let reqs = io_of(&h);
    let IoRequest::Http(req) = &reqs[0].1 else { panic!("{:?}", reqs[0]) };
    assert!(req.url.contains("latitude=-42.88") && req.url.contains("longitude=147.33"), "{}", req.url);
    // `city` once looked like it chose the place and did nothing: it is now reported as unknown.
    assert!(
        h.diags.iter().any(|d| d.message.contains("unknown option 'city' for widget 'my/weather'")),
        "{:?}",
        h.diags
    );
}

#[test]
fn the_git_status_example_counts_changed_files_from_one_named_program() {
    let mut h = bench();
    h.user("widgets/git-status.luau", &example("git-status.luau"));
    h.start_specs(vec![Harness::spec_with(1, "git-status", &[("repo", s("C:/work/project"))])]);
    let reqs = exec_requests(&h);
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].1.program, rsb_core::io::ProgramRef::Name("git".into()));
    assert_eq!(reqs[0].1.args, vec!["-C", "C:/work/project", "status", "--short"]);
    assert!(log_lines("granted git-status:").iter().any(|l| l.contains("exec:git")));
    h.io_reply(reqs[0].0, 1, exec_ok(" M a.txt\r\n?? b.txt\r\n"));
    assert_eq!(shown(&h, 1), "box.item.git(icon:info text.value=\"2 changed\")");
    h.take_requests();
    h.advance(30_000);
    let next = exec_requests(&h);
    h.io_reply(next[0].0, 1, exec_ok(""));
    assert_eq!(h.texts(1), vec!["clean"]);
    h.take_requests();
    h.advance(30_000);
    let next = exec_requests(&h);
    h.io_reply(next[0].0, 1, exec_fail());
    assert_eq!(shown(&h, 1), "box.item.git.stale(icon:info text.value=\"--\")");
}

#[test]
fn the_counter_example_follows_a_watched_file() {
    let mut h = bench();
    h.user("widgets/counter.luau", &example("counter.luau"));
    h.start(&[(1, "counter")]);
    let reqs = io_of(&h);
    assert_eq!(reqs.len(), 1);
    let IoRequest::WatchFile(f) = &reqs[0].1 else { panic!("{:?}", reqs[0]) };
    assert!(f.path.to_string_lossy().replace("\\", "/").ends_with("tester/count.txt"), "{:?}", f.path);
    assert_eq!(shown(&h, 1), "box.item.counter.stale(icon:bell text.value=\"--\")");
    h.io_reply(reqs[0].0, 1, IoEventKind::File(Ok("42\n".into())));
    assert_eq!(shown(&h, 1), "box.item.counter(icon:bell text.value=\"42\")");
    h.io_reply(reqs[0].0, 1, IoEventKind::File(Ok("not a number".into())));
    assert_eq!(h.texts(1), vec!["--"]);
    h.io_reply(reqs[0].0, 1, IoEventKind::File(Err(IoError::NotFound)));
    assert_eq!(shown(&h, 1), "box.item.counter.stale(icon:bell text.value=\"--\")");
}

#[test]
fn every_complete_widget_in_authoring_md_loads_and_runs_without_a_plate() {
    let doc = std::fs::read_to_string(docs().join("authoring.md")).expect("docs/authoring.md").replace("\r\n", "\n");
    let blocks: Vec<&str> = doc
        .split("```lua\n")
        .skip(1)
        .filter_map(|rest| rest.split("```").next())
        .filter(|b| b.contains("return rsb.widget"))
        .collect();
    assert!(blocks.len() >= 3, "the guide shows at least three complete widgets, found {}", blocks.len());
    for (i, block) in blocks.iter().enumerate() {
        let src = if block.contains("require(\"@rsb\")") {
            block.to_string()
        } else {
            format!("local rsb = require(\"@rsb\")\n{block}")
        };
        let name = format!("my/guide{i}");
        let mut h = bench();
        h.user(&format!("widgets/{name}.luau"), &src);
        h.start(&[(1, &name)]);
        assert!(h.plate(1).is_none(), "guide widget {i} shows a plate: {:?}\n{block}", h.plate(1));
        assert_eq!(h.state_of(1), WidgetState::Running, "guide widget {i}\n{block}");
        // A click must work too: the weather widget's click opens a page, which needs its host granted.
        if let Some(click) = h.root(1).and_then(first_click) {
            h.click(1, click);
            assert!(h.plate(1).is_none(), "guide widget {i} shows a plate after a click: {:?}\n{block}", h.plate(1));
        }
    }
}

#[test]
fn authoring_md_names_every_property_api_path_bar_toml_key_and_built_in() {
    let doc = std::fs::read_to_string(docs().join("authoring.md")).expect("docs/authoring.md");
    let mut missing = Vec::new();
    for p in rsb_core::css::PROPERTIES {
        if !doc.contains(&format!("`{}`", p.name)) {
            missing.push(format!("property {}", p.name));
        }
    }
    for item in rsb_script::api::API_ITEMS {
        if !doc.contains(&format!("rsb.{}", item.path)) {
            missing.push(format!("api rsb.{}", item.path));
        }
    }
    for key in rsb_core::config::TOP_KEYS.iter().chain(rsb_core::config::BAR_KEYS) {
        if !doc.contains(&format!("`{key}`")) {
            missing.push(format!("bar.toml key {key}"));
        }
    }
    for w in BUILTINS {
        if !doc.contains(&format!("`{w}`")) {
            missing.push(format!("built-in {w}"));
        }
    }
    for icon in ICON_NAMES {
        if !doc.contains(&format!("`{icon}`")) {
            missing.push(format!("icon {icon}"));
        }
    }
    for name in rsb_script::api::RESERVED {
        if !doc.contains(&format!("rsb.{name}")) {
            missing.push(format!("reserved {name}"));
        }
    }
    for needle in [
        "@import",
        "var(",
        ":root",
        "optional",
        "nil :: string?",
        "luau-lsp",
        "rsb eject",
        "STOPPED",
        "PERMISSION",
        "can run any program",
        "--!strict",
        "$option:",
        "read =",
    ] {
        if !doc.contains(needle) {
            missing.push(format!("topic {needle}"));
        }
    }
    assert!(missing.is_empty(), "authoring.md is missing: {missing:?}");
    assert!(!doc.contains("TODO"), "authoring.md has a TODO");
    assert!(
        doc.is_ascii() || doc.chars().all(|c| c.is_ascii() || c == '\u{b0}'),
        "non-ASCII characters in authoring.md"
    );
}

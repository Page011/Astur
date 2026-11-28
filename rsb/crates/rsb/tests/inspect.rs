//! `rsb inspect` without a bar: replies built from snapshots, paging, cuts,
//! redaction, secrets, the text and JSON output, and the exit codes through
//! a fake pipe.

use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

use rsb_app::inspect::{
    BarView, Facts, InspectOpts, InspectQuery, PopupView, REPLY_LIMIT, UiBar, UiInstance, UiPart, cost_order,
    reply_for, run_with, table, ui_part,
};
use rsb_app::pipe::SendError;
use rsb_app::status::{self, Snapshot};
use rsb_core::config::{WidgetRef, default_config};
use rsb_core::css::{MediaEnv, Stylesheet};
use rsb_core::element::{Element, ElementKind, InputProps, InstanceId, WidgetTree};
use rsb_core::icons::IconFont;
use rsb_core::ipc::{Command, Reply};
use rsb_core::layout::FixedMeasure;
use rsb_core::msg::{ServiceInfo, WidgetSource, WidgetState, WidgetStatus};
use rsb_core::scene::{BarGeometry, InstanceCost, Scene, SceneEnv, SlotSpec, ZoneSlots};
use rsb_core::services::{ServiceId, ServiceStatus};
use rsb_script::inspect::{InspectInfo, ReadInfo, ScriptTimes};
use serde_json::{Value, json};

// ---------------------------------------------------------------- fixtures

fn facts() -> Facts {
    Facts {
        uptime: Duration::from_secs(3725),
        wall_ms: 1_795_000_000_000,
        session: "rdp",
        cpu_ms: 31_000.0,
        cpu_last_60s: 420.0,
        cpu_window_ms: 60_000,
    }
}

fn status_of(id: u32, widget: &str, bar: &str, source: WidgetSource) -> WidgetStatus {
    WidgetStatus {
        instance: InstanceId(id),
        widget: widget.into(),
        bar: bar.into(),
        state: WidgetState::Running,
        script_time: Duration::from_micros(38_200),
        script_time_max: Duration::from_micros(900),
        calls: 3611,
        vm_bytes: 2410 * 1024,
        shared_vm: true,
        source,
        replaces_builtin: false,
        errors: 0,
        restarts: 0,
    }
}

fn row(id: u32, last_60s: f64, options: Value) -> (InstanceId, Value) {
    let info = InspectInfo {
        script: ScriptTimes { ms: 38.2, ms_max: 0.9, calls: 3611, ms_last_s: 0.3, ms_last_60s: last_60s },
        trees: 1204,
        last_tree_ms_ago: Some(400),
        last_run_ms_ago: Some(400),
        reads: ReadInfo {
            service_fields: vec!["clock.time".into()],
            signals: 1,
            computeds: 1,
            effects: 4,
            timers: 1,
            ..Default::default()
        },
        options,
        subscriptions: vec!["clock".into()],
    };
    (InstanceId(id), info.to_json())
}

fn service(id: ServiceId, poll_ms: u64, last_60s_ms: u64) -> ServiceInfo {
    ServiceInfo {
        id,
        status: ServiceStatus::Ok,
        subscribers: 1,
        interval: None,
        version: 1,
        last_update_ago: Some(Duration::from_millis(400)),
        error: None,
        poll_total: Duration::from_millis(poll_ms),
        poll_last_60s: Duration::from_millis(last_60s_ms),
    }
}

/// `n` widgets named w0.. each with one option set by hand, a service and a bar behind them.
fn many(n: usize) -> Snapshot {
    let widgets: Vec<WidgetStatus> =
        (0..n).map(|i| status_of(i as u32 + 1, &format!("w{i}"), "main", WidgetSource::Builtin)).collect();
    let rows = (0..n)
        .map(|i| {
            row(
                i as u32 + 1,
                (i % 17) as f64,
                json!({"format": {"value": "HH:mm", "source": "user", "at": "bar.toml:31:1"}}),
            )
        })
        .collect();
    let instances = (0..n)
        .map(|i| UiInstance {
            id: InstanceId(i as u32 + 1),
            widget: format!("w{i}"),
            cost: InstanceCost { own: 1190, own_us: 14_000, own_us_max: 400, shared: 12, shared_us: 300 },
            nodes: 6,
            pending: false,
            tree: None,
        })
        .collect();
    let bar = UiBar {
        name: "main".into(),
        monitor: 0,
        scale: 1.5,
        scheme: "dark",
        nodes: 52,
        frames: 1214,
        style_us: 120_000,
        layout_us: 110_000,
        paint_us: 100_000,
        frame_us_max: 1100,
        no_tree_frames: 24,
        last_frame_ms_ago: Some(400),
        us_last_60s: 40_000,
        root_vars: vec![("accent".into(), Some("#e5a50a".into()))],
        root: None,
        instances,
    };
    Snapshot {
        widgets: Some(widgets),
        services: Some(vec![service(ServiceId::System, 5200, 180)]),
        bars: None,
        script: Some(rows),
        ui: Some(UiPart { bars: vec![bar] }),
    }
}

fn parse(reply: &Reply) -> Value {
    match reply {
        Reply::Ok(text) => {
            assert!(text.len() <= REPLY_LIMIT, "{} bytes", text.len());
            assert!(text.bytes().all(|b| (0x20..=0x7e).contains(&b)), "printable ASCII only");
            serde_json::from_str(text).expect("one JSON document")
        }
        Reply::Err(e) => panic!("refused: {e}"),
    }
}

fn instance_ids(doc: &Value) -> Vec<u64> {
    doc["widgets"].as_array().unwrap().iter().map(|w| w["instance"].as_u64().unwrap()).collect()
}

// ---------------------------------------------------------------- scene helpers

fn scene_with(trees: Vec<(u32, &str, Element)>) -> Scene {
    let mut bar = default_config().bars.remove(0);
    bar.height = 32;
    let mut s = Scene::new(
        &bar,
        BarGeometry { width_px: 1600, height_px: 32, scale: 1.0 },
        SceneEnv { media: MediaEnv { dark: true }, icon_font: IconFont::Fluent, fullscreen: false },
    );
    s.set_stylesheet(Arc::new(
        Stylesheet::parse(":root { --accent: #e5a50a; --fg: #e6e6e6; --bg: #202020 }", "style.css").0,
    ));
    let mut slots = ZoneSlots::default();
    for (n, class, _) in &trees {
        slots.left.push(SlotSpec { instance: InstanceId(*n), class: (*class).into(), id: (*class).into() });
    }
    s.set_slots(slots);
    for (n, _, root) in trees {
        s.set_tree(Arc::new(WidgetTree {
            instance: InstanceId(n),
            widget: WidgetRef::parse("w").unwrap(),
            version: 1,
            root: Some(root),
            grant: None,
        }));
    }
    s.frame(Instant::now(), &mut FixedMeasure);
    s
}

fn text(s: &str) -> Element {
    let mut e = Element::new(ElementKind::Text);
    e.text = Some(Arc::from(s));
    e
}

fn boxed(children: Vec<Element>) -> Element {
    let mut e = Element::new(ElementKind::Box);
    e.children = children;
    e
}

fn status_for_scene(id: u32, widget: &str) -> Snapshot {
    Snapshot {
        widgets: Some(vec![status_of(id, widget, "main", WidgetSource::Builtin)]),
        services: Some(vec![]),
        bars: None,
        script: Some(vec![row(id, 1.0, json!({}))]),
        ui: None,
    }
}

fn ui_for(scene: &Scene, widget: &str, id: u32, query: &InspectQuery) -> rsb_app::inspect::UiPart {
    let r = WidgetRef::parse(widget).unwrap();
    let view = BarView { name: "main", monitor: 0, scale: 1.0, scene, instances: vec![(InstanceId(id), &r)] };
    ui_part(query, &[view], &[], &["accent", "fg", "bg", "nope"], Instant::now())
}

fn query_for(widget: &str) -> InspectQuery {
    InspectQuery { widget: Some(WidgetRef::parse(widget).unwrap()), ..InspectQuery::default() }
}

// ---------------------------------------------------------------- paging

/// Reads every page of a summary: (widgets per page, bytes per page, instance ids in the order seen).
fn read_pages(snap: &Snapshot) -> (Vec<usize>, Vec<usize>, Vec<u64>) {
    let (mut counts, mut sizes, mut seen) = (Vec::new(), Vec::new(), Vec::new());
    let mut from = 0;
    loop {
        let reply = reply_for(&InspectQuery { from, ..InspectQuery::default() }, snap, &facts());
        let doc = parse(&reply);
        let Reply::Ok(line) = &reply else { unreachable!() };
        counts.push(doc["widgets"].as_array().unwrap().len());
        sizes.push(line.len());
        seen.extend(instance_ids(&doc));
        match doc["next"].as_u64() {
            Some(n) => {
                assert!(n > u64::from(from), "the next page starts later");
                from = n as u32;
            }
            None => break,
        }
        assert!(counts.len() < 20);
    }
    (counts, sizes, seen)
}

#[test]
fn a_page_holds_as_many_widgets_as_fit_and_stays_under_the_limit() {
    let (counts, sizes, seen) = read_pages(&many(100));
    println!("inspect paging: 100 widgets in pages of {counts:?} widgets, {sizes:?} bytes");
    assert_eq!(counts.iter().sum::<usize>(), 100);
    assert!(counts[0] >= 85, "a page is nearly full: {counts:?}");
    assert!(sizes.iter().all(|s| *s <= REPLY_LIMIT));
    assert_eq!(seen.len(), 100);
}

#[test]
fn three_hundred_widgets_take_a_few_pages_with_none_twice_or_missing() {
    let (counts, sizes, seen) = read_pages(&many(300));
    println!("inspect paging: 300 widgets in pages of {counts:?} widgets, {sizes:?} bytes");
    assert_eq!(counts.iter().sum::<usize>(), 300);
    assert!(counts[..counts.len() - 1].iter().all(|c| *c >= 85), "every page but the last is nearly full: {counts:?}");
    assert!(sizes.iter().all(|s| *s <= REPLY_LIMIT));
    let mut sorted = seen.clone();
    sorted.sort();
    assert_eq!(sorted, (1..=300).collect::<Vec<u64>>(), "every instance once");
    assert_eq!(seen.len(), 300);
}

#[test]
fn only_the_first_page_carries_the_bars_services_and_process() {
    let snap = many(300);
    let first = parse(&reply_for(&InspectQuery::default(), &snap, &facts()));
    assert!(first["process"].is_object() && first["services"].is_array() && first["bars"].is_array());
    let later = parse(&reply_for(
        &InspectQuery { from: first["next"].as_u64().unwrap() as u32, ..Default::default() },
        &snap,
        &facts(),
    ));
    assert!(later.get("process").is_none() && later.get("services").is_none());
    assert_eq!(later["contract"], 1);
}

// ---------------------------------------------------------------- order

#[test]
fn costliest_order() {
    assert_eq!(cost_order((21.0, "clock"), (9.0, "media")), std::cmp::Ordering::Less);
    assert_eq!(cost_order((9.0, "b"), (9.0, "a")), std::cmp::Ordering::Greater, "ties go by name");

    let widgets = vec![
        status_of(1, "media", "main", WidgetSource::Builtin),
        status_of(2, "clock", "main", WidgetSource::Builtin),
        status_of(3, "battery", "main", WidgetSource::Builtin),
        status_of(4, "apps", "main", WidgetSource::Builtin),
    ];
    let snap = Snapshot {
        widgets: Some(widgets),
        services: Some(vec![service(ServiceId::System, 5200, 180), service(ServiceId::Audio, 40, 3)]),
        bars: None,
        script: Some(vec![
            row(1, 9.0, json!({})),
            row(2, 21.0, json!({})),
            row(3, 9.0, json!({})),
            row(4, 0.0, json!({})),
        ]),
        ui: None,
    };
    let doc = parse(&reply_for(&InspectQuery::default(), &snap, &facts()));
    let names: Vec<&str> = doc["widgets"].as_array().unwrap().iter().map(|w| w["widget"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["clock", "battery", "media", "apps"]);
    let costliest: Vec<(String, f64)> = doc["costliest"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                format!("{} {}", c["kind"].as_str().unwrap(), c["name"].as_str().unwrap()),
                c["ms_last_60s"].as_f64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        costliest,
        vec![
            ("service system".to_string(), 180.0),
            ("widget clock".to_string(), 21.0),
            ("widget battery".to_string(), 9.0),
            ("widget media".to_string(), 9.0),
            ("service audio".to_string(), 3.0)
        ]
    );
    let services: Vec<&str> = doc["services"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(services, vec!["system", "audio"], "services by poll time in the last minute");
}

// ---------------------------------------------------------------- one snapshot for status and inspect

#[test]
fn the_summary_agrees_with_status_on_calls_script_time_and_vm_size() {
    let snap = many(3);
    let doc = parse(&reply_for(&InspectQuery::default(), &snap, &facts()));
    let st = status::build(10, None, snap.widgets.as_deref(), snap.services.as_deref(), (1.0, 2.0));
    for w in doc["widgets"].as_array().unwrap() {
        let name = w["widget"].as_str().unwrap();
        let s = st["widgets"].as_array().unwrap().iter().find(|s| s["widget"] == name).unwrap();
        assert_eq!(w["script"]["calls"], s["calls"], "{name}");
        assert_eq!(w["memory"]["vm_kb"], s["vm_kb"], "{name}");
        assert_eq!(
            (w["script"]["ms"].as_f64().unwrap() * 10.0).round(),
            (s["script_ms"].as_f64().unwrap() * 10.0).round()
        );
        assert_eq!(w["errors"], s["errors"]);
        assert_eq!(w["state"], s["state"]);
    }
}

#[test]
fn a_part_that_did_not_answer_says_so_and_leaves_its_fields_null() {
    let mut snap = many(2);
    snap.ui = None;
    snap.script = None;
    let doc = parse(&reply_for(&InspectQuery::default(), &snap, &facts()));
    assert_eq!(doc["parts"], json!({"script": "timeout", "ui": "timeout", "hub": "ok"}));
    let w = &doc["widgets"][0];
    assert_eq!(w["frames"], Value::Null);
    assert_eq!(w["memory"]["nodes"], Value::Null);
    assert_eq!(w["reads"], Value::Null);
    assert_eq!(w["script"]["calls"], 3611, "what the status part knew is still there");
    let t = table(&doc);
    assert!(t.contains("did not answer: the script thread, the bar's UI thread"), "{t}");
    assert!(t.contains("layout + paint not known"), "{t}");
}

// ---------------------------------------------------------------- trees and cuts

fn two_thousand() -> Element {
    boxed((0..2000).map(|i| text(&format!("item number {i}"))).collect())
}

#[test]
fn inspect_2000_nodes_cut() {
    let scene = scene_with(vec![(1, "big", two_thousand())]);
    let q = query_for("big");
    let snap = Snapshot { ui: Some(ui_for(&scene, "big", 1, &q)), ..status_for_scene(1, "big") };
    let reply = reply_for(&q, &snap, &facts());
    let doc = parse(&reply);
    let w = &doc["widgets"][0];
    let cut = &w["cut"];
    let (shown, hidden, total) =
        (cut["shown"].as_u64().unwrap(), cut["not_shown"].as_u64().unwrap(), cut["total"].as_u64().unwrap());
    assert_eq!(total, 2001, "the box and its 2000 texts");
    assert_eq!(shown + hidden, total, "shown + cut == total");
    assert!(shown > 100 && shown < total);
    assert_eq!(cut["reason"], "reply limit");
    // The nodes in the JSON are the shown ones, and the root says how many children it lost.
    fn count(n: &Value) -> u64 {
        1 + n["children"].as_array().map_or(0, |c| c.iter().map(count).sum())
    }
    assert_eq!(count(&w["tree"]), shown);
    assert_eq!(w["tree"]["children_cut"].as_u64().unwrap(), 2000 - (shown - 1));
    if let Reply::Ok(t) = &reply {
        println!("inspect 2000 nodes: reply {} bytes, {shown} of {total} nodes shown", t.len());
        assert!(t.len() <= 65_536);
    }
    let t = table(&doc);
    assert!(t.contains("more nodes not shown (reply limit)"), "{t}");
}

#[test]
fn a_small_tree_is_never_cut_and_shows_its_rects_text_and_handlers() {
    let mut root = boxed(vec![text("11:10"), text("sec")]);
    root.classes = vec![Arc::from("clock")];
    root.handlers.click = Some(rsb_core::element::HandlerId(3));
    let scene = scene_with(vec![(1, "clock", root)]);
    let q = query_for("clock");
    let snap = Snapshot { ui: Some(ui_for(&scene, "clock", 1, &q)), ..status_for_scene(1, "clock") };
    let doc = parse(&reply_for(&q, &snap, &facts()));
    let w = &doc["widgets"][0];
    assert!(w.get("cut").is_none(), "nothing was cut");
    assert_eq!(w["pending"], false);
    assert_eq!(w["tree"]["kind"], "box");
    assert_eq!(w["tree"]["handlers"], json!(["click"]));
    assert_eq!(w["tree"]["children"][0]["text"], "11:10");
    assert_eq!(w["tree"]["children"][0]["text_len"], 5);
    assert!(w["tree"]["children"][0]["rect"].is_array());
    assert_eq!(w["memory"]["nodes"], 3);
    assert_eq!(
        (w["frames"]["own"].as_u64(), w["frames"]["shared"].as_u64()),
        (Some(1), Some(0)),
        "the first frame followed this widget's tree alone"
    );
    let t = table(&doc);
    assert!(t.contains("tree (as of the frame"), "{t}");
    assert!(t.contains("box.clock"), "{t}");
    assert!(t.contains("text \"11:10\" x "), "{t}");
    assert!(t.contains("on click"), "{t}");
}

#[test]
fn depth_shows_that_many_levels_and_counts_the_rest() {
    let deep = boxed(vec![boxed(vec![boxed(vec![text("bottom")])])]);
    let scene = scene_with(vec![(1, "deep", deep)]);
    let q = InspectQuery { depth: 2, ..query_for("deep") };
    let snap = Snapshot { ui: Some(ui_for(&scene, "deep", 1, &q)), ..status_for_scene(1, "deep") };
    let doc = parse(&reply_for(&q, &snap, &facts()));
    let w = &doc["widgets"][0];
    assert_eq!(w["cut"], json!({"shown": 2, "not_shown": 2, "total": 4, "reason": "depth"}));
    assert_eq!(w["tree"]["children"][0]["children"], json!([]));
    assert_eq!(w["tree"]["children"][0]["children_cut"], 1);
}

#[test]
fn style_lists_what_differs_and_the_bar_root_comes_with_bar_and_style() {
    let mut root = boxed(vec![]);
    root.style = vec![("gap".to_string(), "4px".to_string())];
    let scene = scene_with(vec![(1, "st", root)]);
    let q = InspectQuery { style: true, ..query_for("st") };
    let snap = Snapshot { ui: Some(ui_for(&scene, "st", 1, &q)), ..status_for_scene(1, "st") };
    let doc = parse(&reply_for(&q, &snap, &facts()));
    assert_eq!(doc["widgets"][0]["tree"]["style"]["gap"], "4px");

    let qb = InspectQuery { style: true, bar: Some("main".into()), ..InspectQuery::default() };
    let mut snap = status_for_scene(1, "st");
    snap.ui = Some(ui_for(&scene, "st", 1, &qb));
    let doc = parse(&reply_for(&qb, &snap, &facts()));
    let bar = &doc["bars"][0];
    assert_eq!(bar["name"], "main");
    assert_eq!(bar["scheme"], "dark");
    assert_eq!(bar["root_vars"]["accent"], "#e5a50a");
    assert_eq!(bar["root_vars"]["bg"], "#202020");
    assert_eq!(bar["root_vars"]["nope"], Value::Null);
    assert_eq!(bar["root"]["kind"], "bar");
    assert!(bar["root"]["style"].is_object(), "the root node's computed style");
    assert!(doc["widgets"][0].get("tree").is_none(), "no widget trees with --bar");
    let t = table(&doc);
    assert!(t.contains("colours: accent #e5a50a, bg #202020"), "{t}");
}

// ---------------------------------------------------------------- privacy

const SENTINEL: &str = "hunter2-SENTINEL";

fn private_scene() -> (Scene, Element) {
    let mut password = Element::new(ElementKind::Input);
    password.input = Some(Box::new(InputProps { value: Arc::from(SENTINEL), password: true, ..InputProps::default() }));
    let mut plain = Element::new(ElementKind::Input);
    plain.input = Some(Box::new(InputProps { value: Arc::from("typed words"), ..InputProps::default() }));
    let mut image = Element::new(ElementKind::Image);
    image.image = Some(rsb_core::element::ImageRef {
        path: Arc::from("C:\\pictures\\holiday.png"),
        width: Some(10.0),
        height: Some(10.0),
    });
    let root = boxed(vec![text("visible text"), password, plain, image]);
    (scene_with(vec![(1, "priv", root.clone())]), root)
}

#[test]
fn password_never_sent() {
    let (scene, _) = private_scene();
    for redact in [false, true] {
        for style in [false, true] {
            let q = InspectQuery { redact, style, ..query_for("priv") };
            let snap = Snapshot { ui: Some(ui_for(&scene, "priv", 1, &q)), ..status_for_scene(1, "priv") };
            let reply = reply_for(&q, &snap, &facts());
            let Reply::Ok(line) = &reply else { panic!("refused") };
            assert!(!line.contains(SENTINEL), "redact {redact} style {style}");
            let doc = parse(&reply);
            let inputs: Vec<&Value> = doc["widgets"][0]["tree"]["children"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|c| c["kind"] == "input")
                .collect();
            assert!(
                inputs.iter().any(|n| n["text"] == format!("<{} chars>", SENTINEL.len())),
                "the password shows its length only"
            );
            assert!(!table(&doc).contains(SENTINEL));
        }
    }
}

#[test]
fn redact_hides_text_input_values_image_paths_and_outside_widget_paths() {
    let (scene, _) = private_scene();
    let mut snap = status_for_scene(1, "priv");
    snap.widgets = Some(vec![status_of(1, "priv", "main", WidgetSource::User("C:\\elsewhere\\priv.luau".into()))]);
    let plain = InspectQuery { ..query_for("priv") };
    snap.ui = Some(ui_for(&scene, "priv", 1, &plain));
    let Reply::Ok(open) = reply_for(&plain, &snap, &facts()) else { panic!() };
    assert!(open.contains("visible text") && open.contains("typed words") && open.contains("holiday.png"));
    assert!(open.contains("C:\\\\elsewhere\\\\priv.luau"));

    let q = InspectQuery { redact: true, ..query_for("priv") };
    snap.ui = Some(ui_for(&scene, "priv", 1, &q));
    let Reply::Ok(hidden) = reply_for(&q, &snap, &facts()) else { panic!() };
    for secret in ["visible text", "typed words", "holiday.png", "elsewhere", "priv.luau"] {
        assert!(!hidden.contains(secret), "{secret} survived redact");
    }
    assert!(hidden.contains("<12 chars>") && hidden.contains("<11 chars>") && hidden.contains("<path>"));
}

#[test]
fn secret_option_never_sent() {
    let options = json!({
        "api_token": {"value": "tok-SENTINEL-1", "source": "user", "at": "bar.toml:5:1"},
        "Password": {"value": "pw-SENTINEL-2", "source": "user", "at": "bar.toml:6:1"},
        "my_SECRET_key": {"value": "sec-SENTINEL-3", "source": "default", "at": null},
        "format": {"value": "HH:mm", "source": "default", "at": null},
    });
    let snap = Snapshot {
        widgets: Some(vec![status_of(1, "w", "main", WidgetSource::Builtin)]),
        services: Some(vec![]),
        bars: None,
        script: Some(vec![row(1, 1.0, options)]),
        ui: None,
    };
    for q in [InspectQuery::default(), InspectQuery { redact: true, ..Default::default() }, query_for("w")] {
        let reply = reply_for(&q, &snap, &facts());
        let Reply::Ok(line) = &reply else { panic!() };
        assert!(!line.contains("SENTINEL"), "a secret option value reached the reply");
        let doc = parse(&reply);
        let o = &doc["widgets"][0]["options"];
        assert_eq!(o["api_token"]["value"], "<14 chars>");
        let ordinary = if q.redact { json!("<5 chars>") } else { json!("HH:mm") };
        assert_eq!(o["format"]["value"], ordinary, "an ordinary option stays, unless --redact hides all text");
        assert!(!table(&doc).contains("SENTINEL"));
    }
}

#[test]
fn nested_secret_options_never_sent() {
    let options = json!({
        "auth": {"value": {"password": "nested-SENTINEL-1", "user": "u"}, "source": "user", "at": "bar.toml:5:1"},
        "service": {"value": {"settings": {"api_key": "deep-SENTINEL-2", "Passwd": ["list-SENTINEL-3"]}}, "source": "user", "at": null},
        "list": {"value": [{"cookie": "cookie-SENTINEL-4"}], "source": "user", "at": null},
        "format": {"value": "HH:mm", "source": "default", "at": null},
    });
    let snap = Snapshot {
        widgets: Some(vec![status_of(1, "w", "main", WidgetSource::Builtin)]),
        services: Some(vec![]),
        bars: None,
        script: Some(vec![row(1, 1.0, options)]),
        ui: None,
    };
    for q in [InspectQuery::default(), InspectQuery { redact: true, ..Default::default() }, query_for("w")] {
        let reply = reply_for(&q, &snap, &facts());
        let Reply::Ok(line) = &reply else { panic!() };
        assert!(!line.contains("SENTINEL"), "a nested secret reached the reply: {line}");
        let doc = parse(&reply);
        let o = &doc["widgets"][0]["options"];
        assert!(o["auth"]["value"].as_str().is_some_and(|t| t.starts_with('<') && t.ends_with(" chars>")), "{o}");
        assert_eq!(o["service"]["value"]["settings"]["api_key"], "<15 chars>");
        assert!(!table(&doc).contains("SENTINEL"));
        if !q.redact {
            assert_eq!(o["format"]["value"], "HH:mm", "an ordinary option stays");
        }
    }
}

#[test]
fn redact_hides_every_option_text_and_the_words_of_an_error() {
    let options = json!({
        "city": {"value": "Sydney-SENTINEL-5", "source": "user", "at": "bar.toml:5:1"},
        "nested": {"value": {"where": "Perth-SENTINEL-6"}, "source": "user", "at": null},
        "size": {"value": 14, "source": "user", "at": null},
    });
    let mut stopped = status_of(1, "w", "main", WidgetSource::User("widgets/boom.luau".into()));
    stopped.state = WidgetState::Stopped("widgets/boom.luau:6 SENTINEL-ERR-7 failed".into());
    let snap = Snapshot {
        widgets: Some(vec![stopped]),
        services: Some(vec![]),
        bars: None,
        script: Some(vec![row(1, 1.0, options)]),
        ui: None,
    };
    let open = reply_for(&InspectQuery::default(), &snap, &facts());
    let Reply::Ok(line) = &open else { panic!() };
    assert!(line.contains("Sydney-SENTINEL-5") && line.contains("SENTINEL-ERR-7"), "without --redact it is all there");
    let q = InspectQuery { redact: true, ..Default::default() };
    let hidden = reply_for(&q, &snap, &facts());
    let Reply::Ok(line) = &hidden else { panic!() };
    assert!(!line.contains("SENTINEL"), "{line}");
    let doc = parse(&hidden);
    let w = &doc["widgets"][0];
    assert_eq!(w["options"]["city"]["value"], "<17 chars>");
    assert_eq!(w["options"]["nested"]["value"]["where"], "<16 chars>");
    assert_eq!(w["options"]["size"]["value"], 14, "a number is not text");
    assert_eq!(w["reason"], "widgets/boom.luau:6", "the place stays, the words do not");
    assert!(!table(&doc).contains("SENTINEL"));
}

// A logger that keeps what this crate logs while a request is served.
struct Capture;
static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static LOGGER: Once = Once::new();
impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        LINES.lock().unwrap().push(format!("{} {}: {}", r.level(), r.target(), r.args()));
    }
    fn flush(&self) {}
}

fn capture() {
    LOGGER.call_once(|| {
        let _ = log::set_logger(&Capture);
        log::set_max_level(log::LevelFilter::Trace);
    });
}

#[test]
fn inspect_logs_nothing() {
    capture();
    LINES.lock().unwrap().clear();
    let (scene, _) = private_scene();
    let q = InspectQuery { style: true, ..query_for("priv") };
    let snap = Snapshot { ui: Some(ui_for(&scene, "priv", 1, &q)), ..status_for_scene(1, "priv") };
    for query in [InspectQuery::default(), q.clone(), InspectQuery { redact: true, ..q.clone() }] {
        assert!(matches!(reply_for(&query, &snap, &facts()), Reply::Ok(_)));
    }
    assert_eq!(LINES.lock().unwrap().len(), 0, "{:?}", LINES.lock().unwrap());

    // A single widget whose entry alone cannot fit: refused, one line, no widget text in it.
    let huge = status_of(1, "big", "main", WidgetSource::Builtin);
    let opts: serde_json::Map<String, Value> = (0..400)
        .map(|i| (format!("option_{i}"), json!({"value": "x".repeat(200), "source": "user", "at": "bar.toml:1:1"})))
        .collect();
    let snap = Snapshot {
        widgets: Some(vec![huge]),
        services: Some(vec![]),
        bars: None,
        script: Some(vec![row(1, 1.0, Value::Object(opts))]),
        ui: None,
    };
    let reply = reply_for(&InspectQuery::default(), &snap, &facts());
    assert_eq!(reply, Reply::Err("inspect refused: reply limit".into()));
    let lines = LINES.lock().unwrap().clone();
    assert_eq!(lines, vec!["WARN rsb_app::inspect: inspect refused: reply limit".to_string()]);
}

// ---------------------------------------------------------------- the command line

fn opts(widget: Option<&str>) -> InspectOpts {
    InspectOpts {
        widget: widget.map(|w| WidgetRef::parse(w).unwrap()),
        bar: None,
        style: false,
        depth: 64,
        redact: false,
        json: false,
    }
}

/// A pipe that answers inspect lines from a snapshot, as the bar would.
fn fake_pipe(snap: Snapshot) -> impl FnMut(&Command) -> Result<Reply, SendError> {
    move |c| match c {
        Command::Inspect { widget, bar, from, depth, style, redact } => Ok(reply_for(
            &InspectQuery {
                widget: widget.clone(),
                bar: bar.clone(),
                from: *from,
                depth: *depth,
                style: *style,
                redact: *redact,
            },
            &snap,
            &facts(),
        )),
        other => panic!("unexpected {other:?}"),
    }
}

fn run(o: &InspectOpts, send: &mut dyn FnMut(&Command) -> Result<Reply, SendError>) -> (u8, String, String) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = run_with(o, send, &mut out, &mut err);
    (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

#[test]
fn no_bar_is_exit_3_and_an_unreachable_one_is_exit_4() {
    let (code, out, err) = run(&opts(None), &mut |_| Err(SendError::NoBar));
    assert_eq!((code, out.as_str(), err.trim()), (3, "", "rsb inspect: no bar running"));
    let (code, _, err) = run(&opts(None), &mut |_| Err(SendError::Refused("timed out".into())));
    assert_eq!(code, 4);
    assert!(err.contains("could not be reached: timed out"), "{err}");
    let (code, _, err) = run(&opts(None), &mut |_| Ok(Reply::Err("inspect refused: reply limit".into())));
    assert_eq!((code, err.trim()), (1, "rsb inspect: inspect refused: reply limit"));
}

#[test]
fn style_without_a_widget_or_bar_is_a_usage_error() {
    let o = InspectOpts { style: true, ..opts(None) };
    let (code, _, err) = run(&o, &mut |_| panic!("nothing is sent"));
    assert_eq!(code, 2);
    assert!(err.contains("--style needs a widget or --bar"), "{err}");
}

#[test]
fn the_command_line_joins_the_pages_and_orders_by_cost() {
    let mut calls = 0;
    let mut pipe = fake_pipe(many(300));
    let (code, out, _) = run(&InspectOpts { json: true, ..opts(None) }, &mut |c| {
        calls += 1;
        pipe(c)
    });
    assert_eq!(code, 0);
    assert!((3..=4).contains(&calls), "a few pages: {calls}");
    let doc: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(doc["widgets"].as_array().unwrap().len(), 300);
    assert_eq!(doc["next"], Value::Null);
    let costs: Vec<f64> =
        doc["widgets"].as_array().unwrap().iter().map(|w| w["script"]["ms_last_60s"].as_f64().unwrap()).collect();
    assert!(costs.windows(2).all(|p| p[0] >= p[1]), "costliest first over all pages");
    let (_, text, _) = run(&opts(None), &mut fake_pipe(many(5)));
    assert!(text.starts_with("rsb "), "{text}");
    assert!(text.contains("process CPU 0.42 s in the last 60 s, 31.00 s since start (uptime 1 h 2 min)"), "{text}");
    assert!(text.contains("costliest in the last 60 s: service system 180 ms; "), "{text}");
    assert!(text.contains("w3  on main, running, built-in, 0 errors, 0 restarts"), "{text}");
    assert!(
        text.contains(
            "options: format \"HH:mm\" (bar.toml:31:1)
"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "reads: clock.time; 1 signal of its own, 1 computed, 4 effects, 1 timer, 0 event listeners, 0 running tasks"
        ),
        "{text}"
    );
}

#[test]
fn an_unknown_widget_is_exit_1_with_a_suggestion() {
    let snap = Snapshot {
        widgets: Some(vec![status_of(1, "clock", "main", WidgetSource::Builtin)]),
        services: Some(vec![]),
        bars: None,
        script: Some(vec![row(1, 1.0, json!({}))]),
        ui: None,
    };
    let (code, out, err) = run(&opts(Some("clok")), &mut fake_pipe(snap));
    assert_eq!((code, out.as_str()), (1, ""));
    assert_eq!(err.trim(), "rsb inspect: no widget 'clok' on the bar; did you mean 'clock'?");
}

#[test]
fn an_unknown_bar_is_exit_1() {
    let snap = Snapshot {
        widgets: Some(vec![]),
        services: Some(vec![]),
        bars: None,
        script: Some(vec![]),
        ui: Some(ui_part(&InspectQuery::default(), &[], &[], &[], Instant::now())),
    };
    let o = InspectOpts { bar: Some("side".into()), ..opts(None) };
    let (code, _, err) = run(&o, &mut fake_pipe(snap));
    assert_eq!((code, err.trim()), (1, "rsb inspect: no bar named 'side'"));
}

#[test]
fn a_cut_is_still_exit_0_and_the_text_says_what_to_do() {
    let scene = scene_with(vec![(1, "big", two_thousand())]);
    let q = query_for("big");
    let snap = Snapshot { ui: Some(ui_for(&scene, "big", 1, &q)), ..status_for_scene(1, "big") };
    let (code, out, _) = run(&opts(Some("big")), &mut fake_pipe(snap));
    assert_eq!(code, 0);
    assert!(out.contains("more nodes not shown (reply limit). Use --depth 3, or leave out --style."), "{out}");
}

#[test]
fn the_popup_content_of_an_open_popup_comes_with_its_scene() {
    use rsb_core::popup::PopupProps;
    let mut popup = Element::new(ElementKind::Popup);
    popup.popup =
        Some(Box::new(PopupProps { names: vec![Arc::from("calendar")], open: false, ..PopupProps::default() }));
    popup.children = vec![text("a"), text("b")];
    let scene = scene_with(vec![(1, "cal", boxed(vec![text("anchor"), popup]))]);
    let q = query_for("cal");
    let snap = Snapshot { ui: Some(ui_for(&scene, "cal", 1, &q)), ..status_for_scene(1, "cal") };
    let doc = parse(&reply_for(&q, &snap, &facts()));
    let kids = doc["widgets"][0]["tree"]["children"].as_array().unwrap();
    let p = kids.iter().find(|k| k["kind"] == "popup").expect("the popup is listed");
    assert_eq!(
        (p["name"].as_str(), p["open"].as_bool(), p["nodes"].as_u64()),
        (Some("calendar"), Some(false), Some(3))
    );
    assert!(table(&doc).contains("popup calendar closed, 3 nodes (open it to see them)"));
    let _ = PopupView { instance: InstanceId(1), name: "calendar", scene: &scene };
}

#[test]
fn the_text_shows_each_option_with_where_it_came_from() {
    let options = json!({
        "format": {"value": "HH:mm", "source": "user", "at": "bar.toml:31:1"},
        "timezone": {"value": "local", "source": "default", "at": null},
    });
    let snap = Snapshot {
        widgets: Some(vec![status_of(1, "clock", "main", WidgetSource::Builtin)]),
        services: Some(vec![]),
        bars: None,
        script: Some(vec![row(1, 21.0, options)]),
        ui: None,
    };
    let doc = parse(&reply_for(&InspectQuery::default(), &snap, &facts()));
    assert!(table(&doc).contains("options: format \"HH:mm\" (bar.toml:31:1), timezone \"local\" (default)"));
}

// ---------------------------------------------------------------- the bar's threads, faked

mod threads {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc::channel;

    use rsb_app::msg::UiMsg;
    use rsb_app::status::Sources;
    use rsb_core::msg::ScriptIn;
    use rsb_core::wake::NoWake;
    use rsb_services::fake::FakeWorld;
    use rsb_services::{Hub, HubOptions};

    pub struct Fake {
        pub sources: Sources,
        pub status_asked: Arc<AtomicU32>,
        pub inspect_asked: Arc<AtomicU32>,
        _hub: Hub,
        _keep: Vec<Box<dyn std::any::Any + Send>>,
    }

    /// A bar whose threads answer from fixtures. A silent one reads its requests and never replies.
    pub fn fake(script_answers: bool, ui_answers: bool) -> Fake {
        let hub = Hub::start(HubOptions::default(), Box::new(FakeWorld::new()), Box::new(|_| {}));
        let (script_tx, script_rx) = channel::<ScriptIn>();
        let (ui_tx, ui_rx) = channel::<UiMsg>();
        let (status_asked, inspect_asked) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
        let (sa, ia) = (status_asked.clone(), inspect_asked.clone());
        let mut keep: Vec<Box<dyn std::any::Any + Send>> = Vec::new();
        let script = std::thread::spawn(move || {
            let mut held: Vec<Box<dyn std::any::Any + Send>> = Vec::new();
            while let Ok(msg) = script_rx.recv() {
                match msg {
                    ScriptIn::Status { reply } => {
                        sa.fetch_add(1, Ordering::SeqCst);
                        if script_answers {
                            let _ = reply.send(vec![status_of(1, "clock", "main", WidgetSource::Builtin)]);
                        } else {
                            held.push(Box::new(reply));
                        }
                    }
                    ScriptIn::Inspect { reply } => {
                        ia.fetch_add(1, Ordering::SeqCst);
                        if script_answers {
                            let _ = reply.send(vec![row(1, 5.0, json!({}))]);
                        } else {
                            held.push(Box::new(reply));
                        }
                    }
                    ScriptIn::Shutdown => break,
                    _ => {}
                }
            }
        });
        let ui = std::thread::spawn(move || {
            let mut held: Vec<Box<dyn std::any::Any + Send>> = Vec::new();
            while let Ok(msg) = ui_rx.recv() {
                match msg {
                    UiMsg::Inspect { reply, .. } => {
                        if ui_answers {
                            let _ = reply.send(UiPart::default());
                        } else {
                            held.push(Box::new(reply));
                        }
                    }
                    UiMsg::BarsQuery { reply } => {
                        let _ = reply.send(Vec::new());
                    }
                    _ => {}
                }
            }
        });
        keep.push(Box::new(script));
        keep.push(Box::new(ui));
        let sources = Sources { script_tx, hub: hub.port(), ui_tx, waker: Box::new(NoWake), started: Instant::now() };
        Fake { sources, status_asked, inspect_asked, _hub: hub, _keep: keep }
    }
}

#[test]
fn serve_asks_each_thread_once_through_the_same_snapshot_as_status() {
    let bar = threads::fake(true, true);
    let status_line = status::gather(&bar.sources);
    let st: Value = serde_json::from_str(&status_line).unwrap();
    assert_eq!(st["widgets"][0]["widget"], "clock");
    assert_eq!(bar.status_asked.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        bar.inspect_asked.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "status does not ask for inspect numbers"
    );

    let doc = parse(&rsb_app::inspect::serve(&InspectQuery::default(), &bar.sources));
    assert_eq!(doc["parts"], json!({"script": "ok", "ui": "ok", "hub": "ok"}));
    assert_eq!(doc["widgets"][0]["widget"], "clock");
    assert_eq!(doc["widgets"][0]["script"]["ms_last_60s"], 5.0);
    assert_eq!(bar.status_asked.load(std::sync::atomic::Ordering::SeqCst), 2, "one more status request, no more");
    assert_eq!(bar.inspect_asked.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(doc["process"]["cpu_ms"].as_f64().unwrap() >= 0.0, "a number, however coarse the process clock is");
}

#[test]
fn a_thread_that_does_not_answer_costs_one_second_not_more_and_reads_timeout() {
    for (script, ui) in [(false, true), (true, false)] {
        let bar = threads::fake(script, ui);
        let t = Instant::now();
        let doc = parse(&rsb_app::inspect::serve(&InspectQuery::default(), &bar.sources));
        let waited = t.elapsed();
        assert!(waited >= Duration::from_millis(900) && waited < Duration::from_millis(1800), "{waited:?}");
        assert_eq!(doc["parts"]["script"], if script { "ok" } else { "timeout" });
        assert_eq!(doc["parts"]["ui"], if ui { "ok" } else { "timeout" });
        assert_eq!(doc["parts"]["hub"], "ok");
    }
}

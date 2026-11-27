//! What `rsb inspect` reads from the script thread: script time and its
//! windows, when a widget last ran and sent a tree, and what its reactive
//! scope holds.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use common::*;
use rsb_core::element::{Element, InstanceId};
use rsb_core::msg::{HubEvent, ScriptIn};
use rsb_core::services::{ServiceEvent, ServiceId, ServiceStatus};
use rsb_core::time::ThreadTime;
use rsb_core::value::Value;
use rsb_script::Limits;
use serde_json::Value as Json;

fn widget(body: &str) -> String {
    format!(
        "local rsb = require(\"@rsb\")\nreturn rsb.widget {{\n  permissions = {{}},\n  setup = function(props, ctx)\n{body}\n  end,\n}}\n"
    )
}

fn step(ms: u64) -> Option<Arc<dyn ThreadTime>> {
    Some(Arc::new(StepTime { now: Default::default(), step: Duration::from_millis(ms) }))
}

/// A thread clock whose step a test changes while it runs.
struct Switch {
    now: AtomicU64,
    step_ms: Arc<AtomicU64>,
}

impl ThreadTime for Switch {
    fn now(&self) -> Duration {
        let step = self.step_ms.load(Ordering::SeqCst);
        Duration::from_millis(self.now.fetch_add(step, Ordering::SeqCst) + step)
    }
}

fn inspect(h: &mut Harness) -> Vec<(InstanceId, Json)> {
    let (tx, rx) = channel();
    h.send(ScriptIn::Inspect { reply: tx });
    rx.recv().expect("the script thread answers")
}

fn row(h: &mut Harness, id: u32) -> Json {
    inspect(h).into_iter().find(|(i, _)| i.0 == id).map(|(_, v)| v).expect("the instance is listed")
}

fn click_id(e: &Element) -> Option<u32> {
    e.handlers.click.map(|h| h.0).or_else(|| e.children.iter().find_map(click_id))
}

fn num(v: &Json, path: &[&str]) -> f64 {
    let mut at = v;
    for p in path {
        at = &at[*p];
    }
    at.as_f64().unwrap_or_else(|| panic!("{path:?} is not a number in {v}"))
}

const QUIET: &str = "return rsb.row { rsb.text 'x', on_click = function() end }";

#[test]
fn script_numbers_follow_the_thread_clock_and_age_out_of_their_windows() {
    // Every call into the VM reads the thread clock twice and the clock steps 5 ms per read: 5 ms a call.
    // The step is 0 while the widget loads, so the longest call is one of the clicks.
    let step_ms = Arc::new(AtomicU64::new(0));
    let clock: Arc<dyn ThreadTime> = Arc::new(Switch { now: AtomicU64::new(0), step_ms: step_ms.clone() });
    let mut h = Harness::with(Limits::default(), Some(clock));
    h.user("widgets/q.luau", &widget(QUIET));
    h.start(&[(1, "q")]);
    let id = click_id(h.root(1).unwrap()).expect("a click handler");
    h.advance(1100);
    step_ms.store(5, Ordering::SeqCst);
    let before = row(&mut h, 1);
    assert_eq!(num(&before, &["script", "ms_last_s"]), 0.0, "setup is more than a second old");

    for _ in 0..3 {
        h.click(1, id);
    }
    let after = row(&mut h, 1);
    assert_eq!(num(&after, &["script", "calls"]) - num(&before, &["script", "calls"]), 3.0);
    assert_eq!(num(&after, &["script", "ms"]) - num(&before, &["script", "ms"]), 15.0);
    assert_eq!(num(&after, &["script", "ms_max"]), 5.0);
    assert_eq!(num(&after, &["script", "ms_last_s"]), 15.0);
    let total = num(&after, &["script", "ms"]);
    assert_eq!(num(&after, &["script", "ms_last_60s"]), total, "everything so far is inside the minute");

    h.advance(1100);
    let later = row(&mut h, 1);
    assert_eq!(num(&later, &["script", "ms_last_s"]), 0.0, "0 after 1.1 s");
    assert_eq!(num(&later, &["script", "ms_last_60s"]), total);
    h.advance(60_000);
    let gone = row(&mut h, 1);
    assert_eq!(num(&gone, &["script", "ms_last_60s"]), 0.0, "and out of the minute after 60 s");
    assert_eq!(num(&gone, &["script", "ms"]), total, "the total never forgets");
}

#[test]
fn last_tree_and_last_run_come_from_the_clock_the_runtime_already_reads() {
    let mut h = Harness::with(Limits::default(), step(1));
    h.user(
        "widgets/c.luau",
        &widget(
            "local n = rsb.state(0)
             return rsb.row { rsb.text { text = function() return tostring(n.value) end },
               on_click = function() n.value += 1 end,
               on_right_click = function() end }",
        ),
    );
    h.start(&[(1, "c")]);
    let first = row(&mut h, 1);
    assert_eq!(first["last_tree_ms_ago"], 0, "the first tree was cut at the start time");
    assert_eq!(first["last_run_ms_ago"], 0);
    assert!(num(&first, &["trees"]) >= 1.0);

    h.advance(250);
    let idle = row(&mut h, 1);
    assert_eq!((idle["last_tree_ms_ago"].as_u64(), idle["last_run_ms_ago"].as_u64()), (Some(250), Some(250)));

    let click = click_id(h.root(1).unwrap()).expect("handler");
    h.click(1, click);
    let changed = row(&mut h, 1);
    assert_eq!((changed["last_tree_ms_ago"].as_u64(), changed["last_run_ms_ago"].as_u64()), (Some(0), Some(0)));
    assert!(num(&changed, &["trees"]) > num(&first, &["trees"]));

    h.advance(400);
    let later = row(&mut h, 1);
    assert_eq!((later["last_tree_ms_ago"].as_u64(), later["last_run_ms_ago"].as_u64()), (Some(400), Some(400)));
}

#[test]
fn a_run_that_changes_nothing_moves_last_run_and_not_last_tree() {
    let mut h = Harness::with(Limits::default(), step(1));
    h.user("widgets/q.luau", &widget(QUIET));
    h.start(&[(1, "q")]);
    let id = click_id(h.root(1).unwrap()).unwrap();
    h.advance(500);
    h.click(1, id);
    let r = row(&mut h, 1);
    assert_eq!(r["last_run_ms_ago"], 0);
    assert_eq!(r["last_tree_ms_ago"], 500, "the click changed nothing drawn");
}

#[test]
fn a_widget_that_draws_nothing_is_listed_with_its_setup_call() {
    let mut h = Harness::new();
    h.user("widgets/q.luau", &widget("return nil"));
    h.start(&[(1, "q")]);
    let r = row(&mut h, 1);
    assert_eq!(r["reads"]["walk_cut"], false);
    assert!(r["script"]["calls"].as_u64().unwrap() >= 1, "the setup call is counted");
}

const BUSY: &str = "
    local sys = rsb.services.system
    local mine = rsb.state(1)
    local doubled = rsb.computed(function() return mine.value * 2 end)
    rsb.effect(function() local _ = doubled.value + (sys.cpu or 0) end)
    rsb.timer.every('1s', function() end)
    rsb.timer.after(5, function() end)
    rsb.events.on('ping', function() end)
    rsb.events.on('pong', function() end)
    rsb.task.spawn(function() rsb.task.sleep(100) end)
    return rsb.text { text = function() return tostring(mine.value) .. tostring(sys.stale) end }";

#[test]
fn the_walk_counts_what_the_widget_reads_and_owns() {
    let mut h = Harness::new();
    h.user("widgets/busy.luau", &widget(BUSY));
    h.user("widgets/idle.luau", &widget("return rsb.text 'still'"));
    h.start(&[(1, "busy"), (2, "idle")]);
    h.send(ScriptIn::Hub(HubEvent::Service(ServiceEvent {
        service: ServiceId::System,
        version: 1,
        status: ServiceStatus::Ok,
        value: Some(map(&[("cpu", Value::Num(12.0))])),
        error: None,
    })));
    let reads = row(&mut h, 1)["reads"].clone();
    assert_eq!(reads["service_fields"], serde_json::json!(["system.cpu", "system.stale"]));
    assert_eq!(reads["signals"], 1, "mine; the service signals are named, not counted");
    assert_eq!(reads["computeds"], 1);
    assert_eq!(reads["effects"], 2, "the written effect and the text binding");
    assert_eq!(reads["timers"], 2);
    assert_eq!(reads["listeners"], 2);
    assert_eq!(reads["tasks"], 1);
    assert_eq!(reads["walk_cut"], false);

    let idle = row(&mut h, 2)["reads"].clone();
    for key in ["signals", "computeds", "effects", "timers", "listeners", "tasks"] {
        assert_eq!(idle[key], 0, "{key} of a widget that reads nothing");
    }
    assert_eq!(idle["service_fields"], serde_json::json!([]));
}

#[test]
fn an_instance_that_was_removed_is_no_longer_listed() {
    let mut h = Harness::new();
    h.user("widgets/q.luau", &widget(QUIET));
    h.start(&[(1, "q"), (2, "q")]);
    assert_eq!(inspect(&mut h).len(), 2);
    h.start(&[(1, "q")]);
    let rows = inspect(&mut h);
    assert_eq!(rows.iter().map(|(i, _)| i.0).collect::<Vec<_>>(), vec![1]);
}

#[test]
fn options_say_where_each_value_came_from_and_services_are_named() {
    let mut h = Harness::new();
    h.user(
        "widgets/opt.luau",
        "local rsb = require(\"@rsb\")
return rsb.widget {
  options = { format = \"HH:mm\", count = 3 },
  optional = { \"city\" },
  permissions = {},
  setup = function(props, ctx)
    local sys = rsb.services.system
    return rsb.text { text = function() return props.format .. tostring(sys.cpu) end }
  end,
}
",
    );
    let spec = Harness::spec_with(
        1,
        "opt",
        &[
            ("format", Value::Str("mm".into())),
            ("city", Value::Str("Oslo".into())),
            ("count", Value::Str("many".into())),
            ("nope", Value::Int(1)),
        ],
    );
    h.start_specs(vec![spec]);
    let r = row(&mut h, 1);
    let o = &r["options"];
    assert_eq!(o["format"], serde_json::json!({"value": "mm", "source": "user", "at": "bar.toml:7:3"}));
    assert_eq!(o["count"]["source"], "default", "a value of the wrong type is not in effect");
    assert_eq!(o["count"]["value"], 3);
    assert_eq!(o["city"]["value"], "Oslo", "an optional option with no default");
    assert_eq!(o["city"]["source"], "user");
    assert!(o.get("nope").is_none(), "an unknown option is not an option");
    assert_eq!(r["subscriptions"], serde_json::json!(["system"]));
}

fn big_limits() -> Limits {
    Limits {
        setup_budget: Duration::from_secs(20),
        callback_budget: Duration::from_secs(20),
        hard_wall_budget: Duration::from_secs(40),
        cpu_window_budget: Duration::from_secs(60),
        max_effect_runs_per_turn: 1_000_000,
        ..Limits::default()
    }
}

fn many_nodes(effects: usize, computeds: usize) -> String {
    widget(&format!(
        "local s = rsb.state(0)
         for i = 1, {effects} do rsb.effect(function() local _ = s.value end) end
         local keep = {{}}
         for i = 1, {computeds} do keep[i] = rsb.computed(function() return s.value + i end) end
         rsb.effect(function() for i = 1, #keep do local _ = keep[i].value end end)
         return rsb.text 'many'"
    ))
}

#[test]
fn a_walk_over_the_node_limit_says_so() {
    let mut h = Harness::with(big_limits(), None);
    h.user("widgets/many.luau", &many_nodes(10_300, 0));
    h.start(&[(1, "many")]);
    assert!(h.plate(1).is_none(), "the widget loaded: {:?}", h.plate(1));
    let reads = row(&mut h, 1)["reads"].clone();
    assert_eq!(reads["walk_cut"], true);
    let effects = reads["effects"].as_u64().unwrap();
    assert!((9_990..=10_000).contains(&effects), "stopped at the limit: {effects} effects");

    let mut small = Harness::with(big_limits(), None);
    small.user("widgets/many.luau", &many_nodes(500, 0));
    small.start(&[(1, "many")]);
    let reads = row(&mut small, 1)["reads"].clone();
    assert_eq!(reads["walk_cut"], false);
    assert_eq!(reads["effects"], 501, "500 and the one that reads the computeds");
}

#[test]
#[ignore = "timing: run in release"]
fn inspect_walk_time_10000_nodes() {
    let mut h = Harness::with(big_limits(), None);
    h.user("widgets/many.luau", &many_nodes(5_000, 4_998));
    h.start(&[(1, "many")]);
    assert!(h.plate(1).is_none(), "{:?}", h.plate(1));
    let first = row(&mut h, 1)["reads"].clone();
    let counted = first["effects"].as_u64().unwrap() + first["computeds"].as_u64().unwrap();
    let mut times = Vec::new();
    for _ in 0..200 {
        let (tx, rx) = channel();
        let t = Instant::now();
        h.send(ScriptIn::Inspect { reply: tx });
        let rows = rx.recv().unwrap();
        times.push(t.elapsed());
        std::hint::black_box(rows);
    }
    times.sort();
    let p = |q: f64| times[((times.len() as f64 * q).ceil() as usize).clamp(1, times.len()) - 1];
    let session = if std::env::var("SESSIONNAME").is_ok_and(|s| s.starts_with("RDP")) { "rdp" } else { "console" };
    println!(
        "inspect_walk_time_10000_nodes ({session}): {counted} effects and computeds counted, walk_cut {}, p50 {:?}, p95 {:?}, max {:?}",
        first["walk_cut"],
        p(0.5),
        p(0.95),
        times.last().unwrap()
    );
    assert!(p(0.95) <= Duration::from_millis(2), "p95 {:?} is over the 2 ms budget", p(0.95));
}

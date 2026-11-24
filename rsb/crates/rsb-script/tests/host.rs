//! The script thread: messages in, trees out, timers fire without messages,
//! and shutdown joins.

mod common;

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use rsb_core::msg::{Heartbeat, ScriptIn, ScriptOut, ScriptSink, WidgetStatus};
use rsb_script::{Limits, ScriptHost, ScriptOptions};
use rsb_services::clock::SystemClock;

struct ChannelSink(Mutex<Sender<ScriptOut>>);

impl ScriptSink for ChannelSink {
    fn send(&self, out: ScriptOut) {
        let _ = self.0.lock().unwrap().send(out);
    }
}

fn start(store: Arc<MemStore>) -> (ScriptHost, Sender<ScriptIn>, Receiver<ScriptOut>, Arc<Heartbeat>) {
    init_log();
    let (out_tx, out_rx) = channel();
    let heartbeat = Arc::new(Heartbeat::new());
    let hub = Arc::new(Mutex::new(Vec::new()));
    let busy = Arc::new(Mutex::new(Vec::new()));
    let (host, tx) = ScriptHost::start(
        ScriptOptions {
            sources: store,
            clock: Arc::new(SystemClock),
            config_dir: std::path::PathBuf::from("C:\\Users\\tester\\AppData\\Roaming\\rsb"),
            path_env: path_env(),
            limits: Limits::default(),
            dark: true,
            thread_time: None,
            heartbeat: heartbeat.clone(),
        },
        Box::new(FakeHub(hub, heartbeat.clone(), busy)),
        Box::new(ChannelSink(Mutex::new(out_tx))),
    );
    (host, tx, out_rx, heartbeat)
}

fn store_with(files: &[(&str, &str)]) -> Arc<MemStore> {
    let store = Arc::new(MemStore { user: Default::default(), builtin: Default::default() });
    for (k, v) in files {
        store.user.lock().unwrap().insert(k.to_string(), v.to_string());
    }
    store
}

fn first_text(rx: &Receiver<ScriptOut>, within: Duration) -> Option<(String, Instant)> {
    let end = Instant::now() + within;
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(ScriptOut::Trees(trees)) => {
                let mut texts = Vec::new();
                for t in trees {
                    if let Some(r) = &t.root {
                        collect_text(r, &mut texts);
                    }
                }
                if !texts.is_empty() {
                    return Some((texts.join("|"), Instant::now()));
                }
            }
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

#[test]
fn the_thread_runs_instances_fires_timers_by_itself_and_joins() {
    let src = "local rsb = require(\"@rsb\")\nreturn rsb.widget { setup = function()\n  local n = rsb.state(0)\n  rsb.timer.every(\"100ms\", function() n.value += 1 end)\n  return rsb.text { text = function() return `n={n.value}` end }\nend }\n";
    let (host, tx, rx, heartbeat) = start(store_with(&[("widgets/tick.luau", src)]));
    let t0 = Instant::now();
    tx.send(ScriptIn::SetInstances(vec![Harness::spec(1, "tick")])).unwrap();
    let (first, _) = first_text(&rx, Duration::from_secs(5)).expect("a first tree");
    assert_eq!(first, "n=0");
    let mut seen = vec![first];
    while seen.len() < 4 {
        let (t, _) = first_text(&rx, Duration::from_secs(5)).expect("a tick without any message");
        seen.push(t);
    }
    assert_eq!(seen, vec!["n=0", "n=1", "n=2", "n=3"]);
    eprintln!("first tree and three timer ticks took {:?} (real clock)", t0.elapsed());
    assert!(heartbeat.busy(Instant::now()).is_none(), "idle between calls");
    let (reply_tx, reply_rx) = channel::<Vec<WidgetStatus>>();
    tx.send(ScriptIn::Status { reply: reply_tx }).unwrap();
    let st = reply_rx.recv_timeout(Duration::from_secs(2)).expect("status is answered by the script thread");
    assert_eq!(st.len(), 1);
    let t = Instant::now();
    host.join(Duration::from_secs(1));
    assert!(t.elapsed() < Duration::from_millis(500), "{:?}", t.elapsed());
    assert!(tx.send(ScriptIn::Theme { dark: false }).is_err(), "the thread is gone");
}

#[test]
fn a_runaway_widget_does_not_stop_the_thread_from_serving_others() {
    let bad = "local rsb = require(\"@rsb\")\nreturn rsb.widget { setup = function()\n  rsb.timer.every(\"100ms\", function() while true do end end)\n  return rsb.text 'bad'\nend }\n";
    let good = "local rsb = require(\"@rsb\")\nreturn rsb.widget { setup = function()\n  local n = rsb.state(0)\n  rsb.timer.every(\"100ms\", function() n.value += 1 end)\n  return rsb.text { text = function() return `good={n.value}` end }\nend }\n";
    let (host, tx, rx, _hb) = start(store_with(&[("widgets/bad.luau", bad), ("widgets/good.luau", good)]));
    tx.send(ScriptIn::SetInstances(vec![Harness::spec(1, "bad"), Harness::spec(2, "good")])).unwrap();
    let mut max_good = 0;
    let mut plate = None;
    let end = Instant::now() + Duration::from_secs(3);
    while Instant::now() < end && (max_good < 3 || plate.is_none()) {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(ScriptOut::Trees(trees)) => {
                for t in trees {
                    if let Some(r) = &t.root {
                        let mut v = Vec::new();
                        collect_text(r, &mut v);
                        if let Some(n) = v.first().and_then(|s| s.strip_prefix("good=")) {
                            max_good = max_good.max(n.parse::<u32>().unwrap());
                        }
                    }
                }
            }
            Ok(ScriptOut::Plate { instance, plate: Some(p) }) => plate = Some((instance.0, p.display_text())),
            _ => {}
        }
    }
    assert!(max_good >= 3, "the other widget kept ticking: {max_good}");
    assert_eq!(plate, Some((1, "STOPPED - bad ran over 10 ms; fix it and save".to_string())));
    host.join(Duration::from_secs(1));
}

#[test]
fn dropping_the_sender_ends_the_thread() {
    let (host, tx, _rx, _hb) = start(store_with(&[]));
    drop(tx);
    // `join` sends Shutdown to a closed channel and waits for the thread: it must still return quickly.
    let t = Instant::now();
    host.join(Duration::from_secs(1));
    assert!(t.elapsed() < Duration::from_millis(900));
}

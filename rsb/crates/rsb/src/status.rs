//! `rsb status`: one line of JSON describing the running bar (`rsb status
//! --json`), shown as a table for people by default. The pipe
//! thread gathers it: widgets from the script thread and services from the
//! hub (1 s timeout each), bars from the UI thread (its own state, answered
//! at once). A part that does not answer reads `"timeout"`; the UI thread is
//! never made to wait for anything.

use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use rsb_core::msg::{HubPort, HubRequest, ScriptIn, ServiceInfo, WidgetSource, WidgetState, WidgetStatus};
use rsb_core::wake::Wake;
use rsb_services::HubSender;
use serde_json::{Value, json};

use crate::msg::{BarInfo, UiMsg};
use crate::stats;

const PART_TIMEOUT: Duration = Duration::from_secs(1);

/// Everything the pipe thread needs to ask the other threads.
pub struct Sources {
    pub script_tx: Sender<ScriptIn>,
    pub hub: HubSender,
    pub ui_tx: Sender<UiMsg>,
    pub waker: Box<dyn Wake>,
    pub started: Instant,
}

/// Printable ASCII only: the wire format allows nothing else.
pub fn ascii_json(v: &Value) -> String {
    let text = v.to_string();
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut buf = [0u16; 2];
            for unit in c.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

fn millis(d: Duration) -> f64 {
    (d.as_secs_f64() * 1000.0 * 100.0).round() / 100.0
}

fn widget_json(w: &WidgetStatus) -> Value {
    let (state, reason) = match &w.state {
        WidgetState::Loading => ("loading", None),
        WidgetState::Running => ("running", None),
        WidgetState::Stopped(text) => ("stopped", Some(text.clone())),
    };
    let source = match &w.source {
        WidgetSource::Builtin => "builtin".to_string(),
        WidgetSource::User(path) => path.clone(),
    };
    let mut v = json!({
        "widget": w.widget, "bar": w.bar, "state": state,
        "script_ms": millis(w.script_time), "script_ms_max": millis(w.script_time_max),
        "calls": w.calls, "errors": w.errors, "restarts": w.restarts,
        "vm_kb": w.vm_bytes / 1024, "shared_vm": w.shared_vm, "source": source,
    });
    if let Some(r) = reason {
        v["reason"] = json!(r);
    }
    if w.replaces_builtin {
        v["replaces_builtin"] = json!(true);
    }
    v
}

fn service_json(s: &ServiceInfo) -> Value {
    json!({
        "name": s.id.name(), "status": s.status.name(), "subscribers": s.subscribers,
        "interval_ms": s.interval.map(|d| d.as_millis() as u64),
        "age_ms": s.last_update_ago.map(|d| d.as_millis() as u64),
    })
}

fn bar_json(b: &BarInfo) -> Value {
    json!({
        "name": b.name, "monitor": b.monitor, "visible": b.visible, "reserved": b.reserved,
        "render": if b.render_failed { "failed" } else { "ok" },
    })
}

/// The reply body from whatever parts answered (None = timed out).
pub fn build(
    uptime_s: u64,
    bars: Option<&[BarInfo]>,
    widgets: Option<&[WidgetStatus]>,
    services: Option<&[ServiceInfo]>,
    memory: (f64, f64),
) -> Value {
    let part = |v: Option<Value>| v.unwrap_or_else(|| json!("timeout"));
    let plates = bars.map(|b| {
        b.iter().filter(|x| x.plate.is_some()).count()
            + widgets.map_or(0, |w| w.iter().filter(|x| matches!(x.state, WidgetState::Stopped(_))).count())
    });
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "session": stats::session_name(),
        "uptime_s": uptime_s,
        "bars": part(bars.map(|b| Value::Array(b.iter().map(bar_json).collect()))),
        "widgets": part(widgets.map(|w| Value::Array(w.iter().map(widget_json).collect()))),
        "services": part(services.map(|s| Value::Array(s.iter().map(service_json).collect()))),
        "memory": {"private_mb": memory.0, "working_set_mb": memory.1},
        "plates": plates.map_or_else(|| json!("timeout"), |n| json!(n)),
    })
}

fn duration_text(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min {} s", secs / 60, secs % 60),
        3600..=86399 => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
        _ => format!("{} d {} h", secs / 86400, secs % 86400 / 3600),
    }
}

/// Rows with every column padded to its widest cell, indented two spaces.
fn aligned(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|c| rows.iter().filter_map(|r| r.get(c)).map(|s| s.chars().count()).max().unwrap_or(0))
        .collect();
    let mut out = String::new();
    for row in rows {
        let cells: Vec<String> = row.iter().enumerate().map(|(c, s)| format!("{s:<w$}", w = widths[c])).collect();
        out.push_str("  ");
        out.push_str(cells.join("  ").trim_end());
        out.push('\n');
    }
    out
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "-".into(),
        other => other.to_string(),
    }
}

/// The reply of `build` as a table for people. Text that is not a status
/// reply comes back unchanged.
pub fn table(reply: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(reply) else { return reply.to_string() };
    let Some(version) = v["version"].as_str() else { return reply.to_string() };
    let silent = |part: &Value| part.as_str() == Some("timeout");
    let mut out = String::new();
    let plates = match &v["plates"] {
        Value::Number(n) => format!("{n} on screen"),
        _ => "not known".to_string(),
    };
    out.push_str(&format!("rsb {version}, running for {}\n", duration_text(v["uptime_s"].as_u64().unwrap_or(0))));
    out.push_str(&format!(
        "memory {:.0} MB private, {:.0} MB working set; error plates: {plates}\n",
        v["memory"]["private_mb"].as_f64().unwrap_or(0.0),
        v["memory"]["working_set_mb"].as_f64().unwrap_or(0.0)
    ));

    out.push_str("\nbars\n");
    match v["bars"].as_array() {
        _ if silent(&v["bars"]) => out.push_str("  did not answer\n"),
        None => out.push_str("  none\n"),
        Some(bars) => {
            let rows: Vec<Vec<String>> = bars
                .iter()
                .map(|b| {
                    vec![
                        text_of(&b["name"]),
                        format!("monitor {}", text_of(&b["monitor"])),
                        if b["visible"].as_bool() == Some(true) { "shown" } else { "hidden" }.into(),
                        if b["reserved"].as_bool() == Some(true) { "space reserved" } else { "no space reserved" }
                            .into(),
                        if b["render"].as_str() == Some("ok") { "drawing ok" } else { "DRAWING FAILED" }.into(),
                    ]
                })
                .collect();
            out.push_str(&aligned(&rows));
        }
    }

    out.push_str("\nwidgets\n");
    match v["widgets"].as_array() {
        _ if silent(&v["widgets"]) => out.push_str("  did not answer\n"),
        None => out.push_str("  none\n"),
        Some(widgets) => {
            let head = ["WIDGET", "BAR", "STATE", "CALLS", "ERRORS", "RESTARTS", "MEMORY"];
            let mut rows = vec![head.map(String::from).to_vec()];
            for w in widgets {
                rows.push(vec![
                    text_of(&w["widget"]),
                    text_of(&w["bar"]),
                    text_of(&w["state"]),
                    text_of(&w["calls"]),
                    text_of(&w["errors"]),
                    text_of(&w["restarts"]),
                    if w["shared_vm"].as_bool() == Some(true) {
                        "shared".into()
                    } else {
                        format!("{} KB", text_of(&w["vm_kb"]))
                    },
                ]);
            }
            out.push_str(&aligned(&rows));
            // Widgets in one VM all report its one size: say it once per VM.
            // The built-ins share one; a user widget placed more than once
            // has one VM for all its copies.
            let mut vms: Vec<(String, usize, u64)> = Vec::new();
            for w in widgets.iter().filter(|w| w["shared_vm"].as_bool() == Some(true)) {
                let source = text_of(&w["source"]);
                let kb = w["vm_kb"].as_u64().unwrap_or(0);
                match vms.iter_mut().find(|v| v.0 == source) {
                    Some(v) => {
                        v.1 += 1;
                        v.2 = v.2.max(kb);
                    }
                    None => vms.push((source, 1, kb)),
                }
            }
            for (source, copies, kb) in vms {
                if source == "builtin" {
                    out.push_str(&format!("  shared = one script VM for all built-in widgets: {kb} KB in all\n"));
                } else {
                    out.push_str(&format!(
                        "  shared = one script VM for {copies} copies of {source}: {kb} KB in all\n"
                    ));
                }
            }
            for w in widgets {
                if w["replaces_builtin"].as_bool() == Some(true) {
                    out.push_str(&format!(
                        "  {}: {} replaces the built-in widget of that name\n",
                        text_of(&w["widget"]),
                        text_of(&w["source"])
                    ));
                }
                if let Some(reason) = w["reason"].as_str() {
                    out.push_str(&format!("  {}: {reason}\n", text_of(&w["widget"])));
                }
            }
        }
    }

    out.push_str("\nservices\n");
    match v["services"].as_array() {
        _ if silent(&v["services"]) => out.push_str("  did not answer\n"),
        None => out.push_str("  none\n"),
        Some(services) => {
            let mut rows = vec![["SERVICE", "STATUS", "WIDGETS USING IT", "LAST VALUE"].map(String::from).to_vec()];
            for s in services {
                let ago = s["age_ms"]
                    .as_u64()
                    .map_or_else(|| "-".to_string(), |ms| format!("{:.1} s ago", ms as f64 / 1000.0));
                rows.push(vec![text_of(&s["name"]), text_of(&s["status"]), text_of(&s["subscribers"]), ago]);
            }
            out.push_str(&aligned(&rows));
        }
    }
    out.trim_end().to_string()
}

/// Asks the three threads and builds the reply line.
pub fn gather(src: &Sources) -> String {
    let (wtx, wrx) = channel();
    let _ = src.script_tx.send(ScriptIn::Status { reply: wtx });
    let (stx, srx) = channel();
    src.hub.send(HubRequest::Status { reply: stx });
    let (btx, brx) = channel();
    let asked_ui = src.ui_tx.send(UiMsg::BarsQuery { reply: btx }).is_ok();
    if asked_ui {
        src.waker.wake();
    }
    // The three parts are working at the same time: wait for each in turn
    // against one shared deadline.
    let deadline = Instant::now() + PART_TIMEOUT;
    let left = || deadline.saturating_duration_since(Instant::now());
    let widgets = wrx.recv_timeout(left()).ok();
    let services = srx.recv_timeout(left()).ok();
    let bars = brx.recv_timeout(left()).ok();
    let body = build(
        src.started.elapsed().as_secs(),
        bars.as_deref(),
        widgets.as_deref(),
        services.as_deref(),
        stats::memory_mb(),
    );
    ascii_json(&body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsb_core::element::InstanceId;
    use rsb_core::services::{ServiceId, ServiceStatus};

    fn widget(state: WidgetState) -> WidgetStatus {
        WidgetStatus {
            instance: InstanceId(1),
            widget: "clock".into(),
            bar: "main".into(),
            state,
            script_time: Duration::from_micros(410),
            script_time_max: Duration::from_micros(50),
            calls: 12,
            vm_bytes: 610 * 1024,
            shared_vm: true,
            source: WidgetSource::Builtin,
            replaces_builtin: false,
            errors: 0,
            restarts: 0,
        }
    }

    fn bar(plate: Option<&str>) -> BarInfo {
        BarInfo {
            name: "main".into(),
            monitor: 0,
            visible: true,
            reserved: true,
            render_failed: false,
            plate: plate.map(str::to_string),
        }
    }

    #[test]
    fn the_reply_has_the_documented_shape() {
        let svc = ServiceInfo {
            id: ServiceId::Audio,
            status: ServiceStatus::Ok,
            subscribers: 1,
            interval: None,
            version: 3,
            last_update_ago: Some(Duration::from_millis(1200)),
            error: None,
        };
        let v = build(12, Some(&[bar(None)]), Some(&[widget(WidgetState::Running)]), Some(&[svc]), (11.8, 19.4));
        assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(v["uptime_s"], 12);
        assert_eq!(v["bars"][0], json!({"name":"main","monitor":0,"visible":true,"reserved":true,"render":"ok"}));
        let w = &v["widgets"][0];
        assert_eq!(
            (w["widget"].as_str(), w["state"].as_str(), w["calls"].as_u64()),
            (Some("clock"), Some("running"), Some(12))
        );
        assert_eq!(
            (w["script_ms"].as_f64(), w["vm_kb"].as_u64(), w["shared_vm"].as_bool()),
            (Some(0.41), Some(610), Some(true))
        );
        assert_eq!(w["source"], "builtin");
        assert_eq!(
            v["services"][0],
            json!({"name":"audio","status":"ok","subscribers":1,"interval_ms":null,"age_ms":1200})
        );
        assert_eq!(v["memory"], json!({"private_mb": 11.8, "working_set_mb": 19.4}));
        assert_eq!(v["plates"], 0);
    }

    #[test]
    fn a_part_that_did_not_answer_reads_timeout_and_plates_are_counted() {
        let stopped = widget(WidgetState::Stopped("STOPPED - x ran over 10 ms; fix it and save".into()));
        let v = build(1, Some(&[bar(Some("RSB ERROR - a"))]), Some(&[stopped]), None, (1.0, 2.0));
        assert_eq!(v["services"], "timeout");
        assert_eq!(v["plates"], 2, "one bar plate and one stopped widget");
        assert_eq!(v["widgets"][0]["reason"], "STOPPED - x ran over 10 ms; fix it and save");
        let v = build(1, None, None, None, (0.0, 0.0));
        assert_eq!(
            (v["bars"].as_str(), v["widgets"].as_str(), v["plates"].as_str()),
            (Some("timeout"), Some("timeout"), Some("timeout"))
        );
    }

    #[test]
    fn the_table_for_people_names_every_part_and_says_what_failed() {
        let svc = ServiceInfo {
            id: ServiceId::Audio,
            status: ServiceStatus::Ok,
            subscribers: 1,
            interval: None,
            version: 3,
            last_update_ago: Some(Duration::from_millis(1200)),
            error: None,
        };
        let mut stopped = widget(WidgetState::Stopped("STOPPED - x ran over 10 ms; fix it and save".into()));
        stopped.widget = "notes".into();
        stopped.shared_vm = false;
        stopped.vm_bytes = 300 * 1024;
        stopped.source = WidgetSource::User("widgets/notes.luau".into());
        let v =
            build(3725, Some(&[bar(None)]), Some(&[widget(WidgetState::Running), stopped]), Some(&[svc]), (11.8, 19.4));
        let t = table(&v.to_string());
        for want in [
            "running for 1 h 2 min",
            "memory 12 MB private, 19 MB working set; error plates: 1 on screen",
            "main  monitor 0  shown  space reserved  drawing ok",
            "WIDGET  BAR   STATE    CALLS  ERRORS  RESTARTS  MEMORY",
            "clock   main  running  12     0       0         shared",
            "notes   main  stopped  12     0       0         300 KB",
            "shared = one script VM for all built-in widgets: 610 KB in all",
            "notes: STOPPED - x ran over 10 ms; fix it and save",
            "audio    ok      1                 1.2 s ago",
        ] {
            assert!(t.contains(want), "{want:?} missing from\n{t}");
        }
        assert!(!t.contains('{'), "no JSON left in the table:\n{t}");
        assert_eq!(t.matches("KB").count(), 2, "one own size and the shared size once:\n{t}");

        // One user widget placed on two bars runs in one VM: its size is
        // printed once, and a widget with a VM of its own still shows its size.
        let copy = |bar: &str, shared: bool| {
            let mut w = widget(WidgetState::Running);
            w.widget = "notes".into();
            w.bar = bar.into();
            w.shared_vm = shared;
            w.vm_bytes = 300 * 1024;
            w.source = WidgetSource::User("widgets/notes.luau".into());
            w
        };
        let two = build(1, None, Some(&[copy("left", true), copy("right", true)]), None, (1.0, 2.0));
        let t = table(&two.to_string());
        assert_eq!(t.matches("KB").count(), 1, "{t}");
        assert_eq!(t.matches("shared\n").count(), 2, "both rows say shared:\n{t}");
        assert!(t.contains("shared = one script VM for 2 copies of widgets/notes.luau: 300 KB in all"), "{t}");
        let one = build(1, None, Some(&[copy("left", false)]), None, (1.0, 2.0));
        assert!(table(&one.to_string()).contains("300 KB"));

        // A user file that stands in for a built-in widget is listed as such, in the table and in the JSON.
        let mut mine = copy("left", false);
        mine.widget = "power".into();
        mine.source = WidgetSource::User("widgets/power.luau".into());
        mine.replaces_builtin = true;
        let replaced = build(1, None, Some(&[mine]), None, (1.0, 2.0));
        assert_eq!(replaced["widgets"][0]["replaces_builtin"], true, "{replaced}");
        let t = table(&replaced.to_string());
        assert!(t.contains("power: widgets/power.luau replaces the built-in widget of that name"), "{t}");
        assert!(widget_json(&widget(WidgetState::Running)).get("replaces_builtin").is_none());

        let silent = table(&build(1, None, None, None, (0.0, 0.0)).to_string());
        assert_eq!(silent.matches("did not answer").count(), 3, "{silent}");
        assert!(silent.contains("error plates: not known"), "{silent}");
        assert_eq!(table("not json"), "not json");
        assert_eq!(table("{\"other\":1}"), "{\"other\":1}");
    }

    #[test]
    fn durations_read_in_the_largest_two_units() {
        assert_eq!(duration_text(59), "59 s");
        assert_eq!(duration_text(61), "1 min 1 s");
        assert_eq!(duration_text(7200), "2 h 0 min");
        assert_eq!(duration_text(90_000), "1 d 1 h");
    }

    #[test]
    fn the_wire_form_is_printable_ascii() {
        let line = ascii_json(&json!({"w": "caf\u{e9} \u{1F600}"}));
        assert!(line.bytes().all(|b| (0x20..=0x7e).contains(&b)), "{line}");
        assert_eq!(line, "{\"w\":\"caf\\u00e9 \\ud83d\\ude00\"}");
    }
}

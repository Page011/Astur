//! The inspect document as text for people. It reads the JSON document, so
//! what is printed is what `--json` would have given.

use rsb_core::time::{Clock, LocalTime, TimeZone};
use serde_json::Value;

const NOT_KNOWN: &str = "not known";

/// 1214 -> `1,214`.
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn count(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_f64().map(|f| f.max(0.0) as u64))
}

fn gnum(v: &Value) -> String {
    count(v).map_or_else(|| NOT_KNOWN.to_string(), grouped)
}

/// Milliseconds with one decimal, `ms` when it is under a second.
fn ms1(v: &Value) -> String {
    v.as_f64().map_or_else(|| NOT_KNOWN.to_string(), |f| format!("{f:.1} ms"))
}

/// A time ago: `0.4 s ago`, `3 min 2 s ago`, or `never`.
fn ago(v: &Value) -> String {
    match v.as_u64() {
        None => "never".to_string(),
        Some(ms) if ms < 60_000 => format!("{:.1} s ago", ms as f64 / 1000.0),
        Some(ms) => format!("{} ago", duration(ms / 1000)),
    }
}

fn duration(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min {} s", secs / 60, secs % 60),
        3600..=86399 => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
        _ => format!("{} d {} h", secs / 86400, secs % 86400 / 3600),
    }
}

fn px(v: &Value) -> String {
    let f = v.as_f64().unwrap_or(0.0);
    if f.fract() == 0.0 { format!("{f:.0}") } else { format!("{f:.1}") }
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A number from the reply with its noun, `1 tree` or `1,204 trees`.
fn counted(v: &Value, one: &str, many: &str) -> String {
    format!("{} {}", gnum(v), if v.as_u64() == Some(1) { one } else { many })
}

fn clock_text(wall_ms: u64) -> String {
    let offset = rsb_services::clock::SystemClock.now(TimeZone::Local).utc_offset_min;
    let t = LocalTime::from_epoch_ms(wall_ms as i64, offset);
    format!("{:02}:{:02}:{:02}", t.hour, t.minute, t.second)
}

fn header(doc: &Value, out: &mut String) {
    let version = doc["version"].as_str().unwrap_or("?");
    out.push_str(&format!(
        "rsb {version}, as of {} (session {})\n",
        doc["wall_ms"].as_u64().map_or_else(|| NOT_KNOWN.to_string(), clock_text),
        doc["session"].as_str().unwrap_or("?")
    ));
    let p = &doc["process"];
    if p.is_object() {
        let window = p["cpu_window_ms"].as_u64().unwrap_or(0);
        out.push_str(&format!(
            "process CPU {:.2} s in the last {} s, {:.2} s since start (uptime {})\n",
            p["cpu_ms_last_60s"].as_f64().unwrap_or(0.0) / 1000.0,
            (window as f64 / 1000.0).round() as u64,
            p["cpu_ms"].as_f64().unwrap_or(0.0) / 1000.0,
            duration(doc["uptime_ms"].as_u64().unwrap_or(0) / 1000),
        ));
    }
    let silent: Vec<&str> =
        ["script", "ui", "hub"].into_iter().filter(|k| doc["parts"][*k].as_str() == Some("timeout")).collect();
    if !silent.is_empty() {
        let names: Vec<&str> = silent
            .iter()
            .map(|k| match *k {
                "script" => "the script thread",
                "ui" => "the bar's UI thread",
                _ => "the services hub",
            })
            .collect();
        out.push_str(&format!("did not answer: {}\n", names.join(", ")));
    }
    if let Some(items) = doc["costliest"].as_array() {
        let parts: Vec<String> = items
            .iter()
            .map(|i| {
                let kind = if i["kind"] == "service" { "service " } else { "" };
                format!(
                    "{kind}{} {:.0} ms",
                    i["name"].as_str().unwrap_or("?"),
                    i["ms_last_60s"].as_f64().unwrap_or(0.0)
                )
            })
            .collect();
        if parts.is_empty() {
            out.push_str("costliest in the last 60 s: nothing measured\n");
        } else {
            out.push_str(&format!("costliest in the last 60 s: {}\n", parts.join("; ")));
        }
    }
    if let Some(bars) = doc["bars"].as_array() {
        for b in bars {
            out.push_str(&format!(
                "bar {}  monitor {}  scale {}  {} nodes  {}: style {:.2} s, layout {:.2} s, paint {:.2} s; slowest {:.1} ms; last {}; {:.0} ms in the last 60 s\n",
                b["name"].as_str().unwrap_or("?"),
                b["monitor"],
                b["scale"].as_f64().unwrap_or(1.0),
                b["nodes"],
                counted(&b["frames"], "frame", "frames"),
                b["style_us"].as_f64().unwrap_or(0.0) / 1e6,
                b["layout_us"].as_f64().unwrap_or(0.0) / 1e6,
                b["paint_us"].as_f64().unwrap_or(0.0) / 1e6,
                b["frame_us_max"].as_f64().unwrap_or(0.0) / 1000.0,
                ago(&b["last_frame_ms_ago"]),
                b["ms_last_60s"].as_f64().unwrap_or(0.0),
            ));
            if let Some(vars) = b["root_vars"].as_object() {
                let list: Vec<String> =
                    vars.iter().map(|(k, v)| format!("{k} {}", v.as_str().unwrap_or("-"))).collect();
                if !list.is_empty() {
                    out.push_str(&format!(
                        "  colours: {} ({})\n",
                        list.join(", "),
                        b["scheme"].as_str().unwrap_or("?")
                    ));
                }
            }
            if b["root"].is_object() {
                node_lines(&b["root"], 0, out);
            }
        }
    }
}

fn option_text(options: &Value) -> String {
    let Some(map) = options.as_object() else { return "none".to_string() };
    if map.is_empty() {
        return "none".to_string();
    }
    map.iter()
        .map(|(k, o)| {
            let at = match o["source"].as_str() {
                Some("user") | Some("preset") => o["at"].as_str().unwrap_or("set").to_string(),
                _ => "default".to_string(),
            };
            format!("{k} {} ({at})", o["value"])
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn reads_text(reads: &Value) -> String {
    if !reads.is_object() {
        return NOT_KNOWN.to_string();
    }
    let n = |k: &str| reads[k].as_u64().unwrap_or(0);
    let fields: Vec<&str> =
        reads["service_fields"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let list = if fields.is_empty() { "no service fields".to_string() } else { fields.join(", ") };
    let least = if reads["walk_cut"] == true { "at least " } else { "" };
    format!(
        "{list}; {least}{}, {}, {}, {}, {}, {}",
        plural(n("signals"), "signal of its own", "signals of its own"),
        plural(n("computeds"), "computed", "computeds"),
        plural(n("effects"), "effect", "effects"),
        plural(n("timers"), "timer", "timers"),
        plural(n("listeners"), "event listener", "event listeners"),
        plural(n("tasks"), "running task", "running tasks"),
    )
}

fn services_text(w: &Value, services: &[Value]) -> String {
    let Some(list) = w["services"].as_array() else { return NOT_KNOWN.to_string() };
    if list.is_empty() {
        return "none".to_string();
    }
    list.iter()
        .map(|s| {
            let name = s["name"].as_str().unwrap_or("?");
            let poll = services.iter().find(|t| t["name"] == s["name"]).and_then(|t| t["poll_ms_last_60s"].as_f64());
            let value = match s["age_ms"].as_u64() {
                Some(ms) => format!("last value {}", ago(&Value::from(ms))),
                None => "no value yet".to_string(),
            };
            match poll {
                Some(p) => format!(
                    "{name} {} ({value}; polls {p:.1} ms in the last 60 s)",
                    s["status"].as_str().unwrap_or("?")
                ),
                None => format!("{name} {} ({value})", s["status"].as_str().unwrap_or("?")),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn widget_lines(w: &Value, services: &[Value], out: &mut String) {
    let source = match w["source"].as_str() {
        Some("builtin") => "built-in".to_string(),
        Some(p) => p.to_string(),
        None => NOT_KNOWN.to_string(),
    };
    out.push_str(&format!(
        "{}  on {}, {}, {source}, {}, {}\n",
        w["widget"].as_str().unwrap_or("?"),
        w["bar"].as_str().unwrap_or("?"),
        w["state"].as_str().unwrap_or("?"),
        plural(w["errors"].as_u64().unwrap_or(0), "error", "errors"),
        plural(w["restarts"].as_u64().unwrap_or(0), "restart", "restarts"),
    ));
    if let Some(reason) = w["reason"].as_str() {
        out.push_str(&format!("  {reason}\n"));
    }
    out.push_str(&format!(
        "  last drawn {} ({} since load); script last ran {}\n",
        ago(&w["last_tree_ms_ago"]),
        counted(&w["trees"], "tree", "trees"),
        ago(&w["last_run_ms_ago"]),
    ));
    let s = &w["script"];
    out.push_str(&format!(
        "  script {} in {} calls, slowest {}, {} in the last 60 s, {} in the last second\n",
        ms1(&s["ms"]),
        gnum(&s["calls"]),
        ms1(&s["ms_max"]),
        ms1(&s["ms_last_60s"]),
        ms1(&s["ms_last_s"]),
    ));
    out.push_str(&format!("  options: {}\n", option_text(&w["options"])));
    let f = &w["frames"];
    if f.is_object() {
        out.push_str(&format!(
            "  layout + paint {:.1} ms over {} of its own, slowest {:.1} ms; {} shared with other widgets\n",
            f["own_us"].as_f64().unwrap_or(0.0) / 1000.0,
            counted(&f["own"], "frame", "frames"),
            f["own_us_max"].as_f64().unwrap_or(0.0) / 1000.0,
            counted(&f["shared"], "frame", "frames"),
        ));
    } else {
        out.push_str("  layout + paint not known (the bar did not answer)\n");
    }
    let m = &w["memory"];
    let vm = if w["source"] == "builtin" {
        format!("script VM shared by all built-ins ({} KB in all)", gnum(&m["vm_kb"]))
    } else if m["shared_vm"] == true {
        format!("script VM shared with other copies ({} KB in all)", gnum(&m["vm_kb"]))
    } else {
        format!("script VM {} KB", gnum(&m["vm_kb"]))
    };
    let nodes = m["nodes"].as_u64().map_or_else(|| "nodes not known".to_string(), |n| format!("{n} nodes on the bar"));
    out.push_str(&format!("  memory: {vm}; {nodes}\n"));
    out.push_str(&format!("  services: {}\n", services_text(w, services)));
    out.push_str(&format!("  reads: {}\n", reads_text(&w["reads"])));
    if w["replaces_builtin"] == true {
        out.push_str("  this file replaces the built-in widget of that name\n");
    }
}

fn node_line(n: &Value) -> String {
    let mut line = n["kind"].as_str().unwrap_or("?").to_string();
    if let Some(id) = n["id"].as_str() {
        line.push('#');
        line.push_str(id);
    }
    for c in n["classes"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        line.push('.');
        line.push_str(c);
    }
    if let Some(t) = n["text"].as_str() {
        line.push_str(&format!(" {:?}", t));
    }
    if let Some(src) = n["src"].as_str() {
        line.push_str(&format!(" src {src}"));
    }
    if n["hidden"] == true {
        line.push_str(" hidden");
    } else if let Some(r) = n["rect"].as_array() {
        line.push_str(&format!(" x {} y {} w {} h {}", px(&r[0]), px(&r[1]), px(&r[2]), px(&r[3])));
    }
    let states: Vec<&str> = n["states"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    if !states.is_empty() {
        line.push(' ');
        line.push_str(&states.join(" "));
    }
    let handlers: Vec<&str> = n["handlers"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    if !handlers.is_empty() {
        line.push_str(&format!(" on {}", handlers.join(", ")));
    }
    line
}

fn node_lines(n: &Value, depth: usize, out: &mut String) {
    let pad = "  ".repeat(depth + 1);
    if n["kind"] == "popup" && n["popup_kind"].is_string() {
        let name = n["name"].as_str().filter(|s| !s.is_empty()).unwrap_or("(unnamed)");
        let kind = n["popup_kind"].as_str().unwrap_or("popup");
        if n["open"] == true && n["children"].as_array().is_some_and(|c| !c.is_empty()) {
            out.push_str(&format!("{pad}{kind} {name} open, {} nodes\n", n["nodes"]));
        } else {
            out.push_str(&format!("{pad}{kind} {name} closed, {} nodes (open it to see them)\n", n["nodes"]));
        }
    } else {
        out.push_str(&format!("{pad}{}\n", node_line(n)));
        if let Some(style) = n["style"].as_object() {
            for (k, v) in style {
                out.push_str(&format!("{pad}  {k}: {}\n", v.as_str().unwrap_or("")));
            }
        }
    }
    if let Some(kids) = n["children"].as_array() {
        for k in kids {
            node_lines(k, depth + 1, out);
        }
    }
    if let Some(cut) = n["children_cut"].as_u64().filter(|c| *c > 0) {
        out.push_str(&format!("{pad}  ({cut} more children not shown)\n"));
    }
}

fn tree_lines(w: &Value, out: &mut String) {
    let tree = &w["tree"];
    if tree.is_null() {
        return;
    }
    out.push_str(&format!("  tree (as of the frame {}; logical px)\n", ago(&w["frame_ms_ago"])));
    if w["pending"] == true {
        out.push_str("  (a newer tree or style waits for the next frame)\n");
    }
    match tree {
        Value::Array(roots) => roots.iter().for_each(|r| node_lines(r, 1, out)),
        root => node_lines(root, 1, out),
    }
    if let Some(cut) = w["cut"].as_object() {
        let hidden = cut["not_shown"].as_u64().unwrap_or(0);
        let reason = cut["reason"].as_str().unwrap_or("limit");
        let advice = match reason {
            "reply limit" => ". Use --depth 3, or leave out --style.",
            "depth" => ". Use a larger --depth.",
            _ => ".",
        };
        out.push_str(&format!("  ... {} more nodes not shown ({reason}){advice}\n", grouped(hidden)));
    }
}

/// The text for one joined document (a summary, or a widget reply).
pub fn table(doc: &Value) -> String {
    let mut out = String::new();
    header(doc, &mut out);
    let services: Vec<Value> = doc["services"].as_array().cloned().unwrap_or_default();
    let widgets = doc["widgets"].as_array().cloned().unwrap_or_default();
    for w in &widgets {
        out.push('\n');
        widget_lines(w, &services, &mut out);
        tree_lines(w, &mut out);
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_group_in_threes() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1214), "1,214");
        assert_eq!(grouped(1_820_000), "1,820,000");
    }

    #[test]
    fn a_time_ago_reads_in_seconds_then_minutes() {
        assert_eq!(ago(&Value::from(400)), "0.4 s ago");
        assert_eq!(ago(&Value::from(125_000)), "2 min 5 s ago");
        assert_eq!(ago(&Value::Null), "never");
    }

    #[test]
    fn one_is_not_plural() {
        assert_eq!(counted(&Value::from(1), "tree", "trees"), "1 tree");
        assert_eq!(counted(&Value::from(1204), "tree", "trees"), "1,204 trees");
        assert_eq!(counted(&Value::from(0), "frame", "frames"), "0 frames");
    }
}

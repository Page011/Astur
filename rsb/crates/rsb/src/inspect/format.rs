//! The inspect reply as JSON: one document per request, every one under the
//! pipe's reply limit. The summary is paged by widget instance; a widget
//! request carries each instance's node tree, cut breadth first at a byte
//! budget, with every cut counted.

use std::collections::HashMap;
use std::time::Duration;

use rsb_core::css_text;
use rsb_core::ipc::MAX_REPLY_BYTES;
use rsb_core::msg::{ServiceInfo, WidgetSource, WidgetState, WidgetStatus};
use rsb_core::scene::{InspectNode, InstanceNodes};
use rsb_core::style::ComputedStyle;
use serde_json::{Map, Value, json};

use super::InspectQuery;
use super::ui::{UiBar, UiInstance, UiTree};
use crate::status::{Snapshot, ascii_json};

/// The version every document carries.
pub const CONTRACT: u32 = 1;
/// Bytes of one page of widgets, or of the widgets of one reply: well under
/// the pipe's limit, so the rest of the document always fits.
pub const PAGE_BUDGET: usize = 60_000;
/// The longest the pipe lets an `ok` line be: the limit less `ok ` and the newline.
pub const REPLY_LIMIT: usize = MAX_REPLY_BYTES - 4;
/// Characters of a node's text shown before it is cut.
pub const MAX_TEXT: usize = 80;
/// Names costliest is shown for.
const COSTLIEST: usize = 5;

/// What the document needs besides the snapshot.
pub struct Facts {
    pub uptime: Duration,
    pub wall_ms: u64,
    pub session: &'static str,
    /// Process CPU since start, in ms.
    pub cpu_ms: f64,
    /// Process CPU in the last minute and the ms of run that covers.
    pub cpu_last_60s: f64,
    pub cpu_window_ms: u64,
}

/// A reply that cannot be made to fit: the reason in a few words.
#[derive(Debug, PartialEq, Eq)]
pub struct Refused(pub &'static str);

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn ms(d: Duration) -> f64 {
    r1(d.as_secs_f64() * 1000.0)
}

/// An option whose name says it holds a secret.
pub fn secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    ["password", "passwd", "pwd", "secret", "token", "apikey", "api_key", "api-key", "credential", "cookie", "auth"]
        .iter()
        .any(|w| k.contains(w))
}

fn chars_text(n: usize) -> String {
    format!("<{n} chars>")
}

/// Orders widgets and the `costliest` line: the most milliseconds in the last
/// minute first, then by name.
pub fn cost_order(a: (f64, &str), b: (f64, &str)) -> std::cmp::Ordering {
    b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.1.cmp(b.1))
}

fn cost_of(entry: &Value) -> f64 {
    entry["script"]["ms_last_60s"].as_f64().unwrap_or(0.0)
}

fn name_of(entry: &Value) -> &str {
    entry["widget"].as_str().unwrap_or("")
}

fn state_parts(w: &WidgetStatus) -> (&'static str, Option<String>) {
    match &w.state {
        WidgetState::Loading => ("loading", None),
        WidgetState::Running => ("running", None),
        WidgetState::Stopped(text) => ("stopped", Some(text.clone())),
    }
}

/// A user widget's path when it is not inside the config folder: a drive,
/// a root or a step up.
fn outside_config(path: &str) -> bool {
    path.contains(':') || path.starts_with(['/', '\\']) || path.split(['/', '\\']).any(|p| p == "..")
}

/// How long a value is, for `<N chars>`.
fn length_of(v: &Value) -> usize {
    match v {
        Value::String(s) => s.chars().count(),
        other => other.to_string().chars().count(),
    }
}

/// An option value with every secret under it hidden, at any depth: a value
/// held by a key that names a secret becomes its length. With `all_strings`
/// (redact) every other string goes the same way.
fn scrub(v: &mut Value, all_strings: bool) {
    match v {
        Value::Object(map) => {
            for (k, inner) in map.iter_mut() {
                if secret_key(k) {
                    *inner = json!(chars_text(length_of(inner)));
                } else {
                    scrub(inner, all_strings);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|i| scrub(i, all_strings)),
        Value::String(s) if all_strings => *v = json!(chars_text(s.chars().count())),
        _ => {}
    }
}

/// The options of a widget as sent: a secret's value is its length only, and
/// with `redact` so is every other text value.
fn options_value(options: &Value, redact: bool) -> Value {
    let Some(map) = options.as_object() else { return json!({}) };
    let mut out = Map::new();
    for (k, v) in map {
        let mut v = v.clone();
        if secret_key(k) {
            v["value"] = json!(chars_text(length_of(&v["value"])));
        } else {
            scrub(&mut v["value"], redact);
        }
        out.insert(k.clone(), v);
    }
    Value::Object(out)
}

/// What stays of a stopped widget's reason under `redact`: the place of the
/// failing line (`widgets/clock.luau:6`), never the error text, which can
/// quote a value. A path outside the config folder is hidden too.
fn reason_place(text: &str) -> String {
    for word in text.split_whitespace() {
        let Some(i) = word.find(".luau:") else { continue };
        let digits: String = word[i + 6..].chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            continue;
        }
        let path = &word[..i + 5];
        let path = if outside_config(path) { "<path>" } else { path };
        return format!("{path}:{digits}");
    }
    chars_text(text.chars().count())
}

/// One widget instance as the summary shows it.
fn widget_entry(
    w: &WidgetStatus,
    row: Option<&Value>,
    ui: Option<&UiInstance>,
    services: &[ServiceInfo],
    redact: bool,
) -> Value {
    let (state, reason) = state_parts(w);
    let source = match &w.source {
        WidgetSource::Builtin => "builtin".to_string(),
        WidgetSource::User(p) if redact && outside_config(p) => "<path>".to_string(),
        WidgetSource::User(p) => p.clone(),
    };
    let null = Value::Null;
    let from_row = |k: &str| row.map_or(&null, |r| &r[k]).clone();
    let script = match row {
        Some(r) => r["script"].clone(),
        None => json!({"ms": ms(w.script_time), "ms_max": ms(w.script_time_max), "calls": w.calls,
                       "ms_last_s": null, "ms_last_60s": null}),
    };
    let subscriptions: Vec<String> = row
        .and_then(|r| r["subscriptions"].as_array())
        .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let service_list: Vec<Value> = subscriptions
        .iter()
        .map(|name| match services.iter().find(|s| s.id.name() == name) {
            Some(s) => json!({"name": name, "status": s.status.name(),
                              "age_ms": s.last_update_ago.map(|d| d.as_millis() as u64)}),
            None => json!({"name": name, "status": "off", "age_ms": null}),
        })
        .collect();
    let frames = match ui {
        Some(u) => json!({"own": u.cost.own, "own_us": u.cost.own_us, "own_us_max": u.cost.own_us_max,
                          "shared": u.cost.shared, "shared_us": u.cost.shared_us}),
        None => Value::Null,
    };
    let mut e = json!({
        "instance": w.instance.0,
        "widget": w.widget,
        "bar": w.bar,
        "state": state,
        "source": source,
        "errors": w.errors,
        "restarts": w.restarts,
        "last_tree_ms_ago": from_row("last_tree_ms_ago"),
        "trees": from_row("trees"),
        "last_run_ms_ago": from_row("last_run_ms_ago"),
        "script": script,
        "frames": frames,
        "memory": {"vm_kb": w.vm_bytes / 1024, "shared_vm": w.shared_vm, "nodes": ui.map(|u| u.nodes)},
        "options": options_value(row.map_or(&null, |r| &r["options"]), redact),
        "services": service_list,
        "reads": from_row("reads"),
    });
    // A widget that is not stopped has no reason: the key is left out, not sent as null.
    if let Some(reason) = reason {
        e["reason"] = json!(if redact { reason_place(&reason) } else { reason });
    }
    if w.replaces_builtin {
        e["replaces_builtin"] = json!(true);
    }
    e
}

fn ui_instance(snap: &Snapshot, id: u32) -> Option<(&UiBar, &UiInstance)> {
    snap.ui.as_ref()?.bars.iter().find_map(|b| b.instances.iter().find(|i| i.id.0 == id).map(|i| (b, i)))
}

fn row_of(snap: &Snapshot, id: u32) -> Option<&Value> {
    snap.script.as_ref()?.iter().find(|(i, _)| i.0 == id).map(|(_, v)| v)
}

/// Widgets of the query, in instance order.
fn matching<'a>(snap: &'a Snapshot, q: &InspectQuery) -> Vec<&'a WidgetStatus> {
    let mut v: Vec<&WidgetStatus> = snap
        .widgets
        .iter()
        .flatten()
        .filter(|w| q.bar.as_deref().is_none_or(|b| w.bar == b))
        .filter(|w| q.widget.as_ref().is_none_or(|r| w.widget == r.to_string()))
        .collect();
    v.sort_by_key(|w| w.instance.0);
    v
}

fn parts(snap: &Snapshot) -> Value {
    let ok = |b: bool| if b { "ok" } else { "timeout" };
    json!({
        "script": ok(snap.widgets.is_some() && snap.script.is_some()),
        "ui": ok(snap.ui.is_some()),
        "hub": ok(snap.services.is_some()),
    })
}

fn bar_entry(b: &UiBar) -> Value {
    let mut vars = Map::new();
    for (name, v) in &b.root_vars {
        vars.insert(name.clone(), json!(v));
    }
    let mut e = json!({
        "name": b.name, "monitor": b.monitor, "scale": b.scale, "nodes": b.nodes,
        "frames": b.frames, "style_us": b.style_us, "layout_us": b.layout_us, "paint_us": b.paint_us,
        "frame_us_max": b.frame_us_max, "no_tree_frames": b.no_tree_frames,
        "last_frame_ms_ago": b.last_frame_ms_ago, "ms_last_60s": r1(b.us_last_60s as f64 / 1000.0),
        "scheme": b.scheme, "root_vars": Value::Object(vars),
    });
    if let Some(root) = &b.root {
        let base = ComputedStyle::default();
        e["root"] = node_base(root, Some(&base), false, true);
    }
    e
}

fn service_entries(snap: &Snapshot) -> Vec<Value> {
    let mut v: Vec<&ServiceInfo> =
        snap.services.iter().flatten().filter(|s| s.subscribers > 0 || s.poll_total > Duration::ZERO).collect();
    v.sort_by(|a, b| cost_order((ms(a.poll_last_60s), a.id.name()), (ms(b.poll_last_60s), b.id.name())));
    v.iter()
        .map(|s| {
            json!({"name": s.id.name(), "status": s.status.name(), "poll_ms": ms(s.poll_total),
                   "poll_ms_last_60s": ms(s.poll_last_60s)})
        })
        .collect()
}

fn costliest(snap: &Snapshot, all: &[Value]) -> Vec<Value> {
    let mut items: Vec<(f64, String, Value)> = Vec::new();
    for s in snap.services.iter().flatten() {
        let v = ms(s.poll_last_60s);
        if v > 0.0 {
            items.push((v, s.id.name().to_string(), json!({"kind": "service", "name": s.id.name(), "ms_last_60s": v})));
        }
    }
    for e in all {
        let v = cost_of(e);
        if v > 0.0 {
            items.push((
                v,
                name_of(e).to_string(),
                json!({"kind": "widget", "instance": e["instance"], "name": e["widget"], "ms_last_60s": v}),
            ));
        }
    }
    items.sort_by(|a, b| cost_order((a.0, &a.1), (b.0, &b.1)));
    items.into_iter().take(COSTLIEST).map(|i| i.2).collect()
}

fn header(f: &Facts) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("contract".into(), json!(CONTRACT));
    m.insert("version".into(), json!(env!("CARGO_PKG_VERSION")));
    m.insert("session".into(), json!(f.session));
    m.insert("wall_ms".into(), json!(f.wall_ms));
    m.insert("uptime_ms".into(), json!(f.uptime.as_millis() as u64));
    m
}

fn finish(doc: Value) -> Result<String, Refused> {
    let line = ascii_json(&doc);
    if line.len() > REPLY_LIMIT { Err(Refused("reply limit")) } else { Ok(line) }
}

/// One page of the summary: every widget, no tree. `from` counts instances
/// in instance order; a page is as many as fit the byte budget, sorted by
/// cost inside the page. Returns the line and the `from` of the next page.
pub fn summary(f: &Facts, snap: &Snapshot, q: &InspectQuery) -> Result<(String, Option<u32>), Refused> {
    let ws = matching(snap, q);
    let services = snap.services.as_deref().unwrap_or(&[]);
    let entries: Vec<Value> = ws
        .iter()
        .map(|w| {
            widget_entry(
                w,
                row_of(snap, w.instance.0),
                ui_instance(snap, w.instance.0).map(|(_, i)| i),
                services,
                q.redact,
            )
        })
        .collect();
    let first = q.from == 0;
    let mut doc = header(f);
    if first {
        doc.insert(
            "process".into(),
            json!({"cpu_ms": r1(f.cpu_ms), "cpu_ms_last_60s": r1(f.cpu_last_60s), "cpu_window_ms": f.cpu_window_ms}),
        );
        doc.insert("costliest".into(), Value::Array(costliest(snap, &entries)));
        doc.insert("bars".into(), Value::Array(snap.ui.iter().flat_map(|u| &u.bars).map(bar_entry).collect()));
        doc.insert("services".into(), Value::Array(service_entries(snap)));
    }
    doc.insert("parts".into(), parts(snap));
    let base = ascii_json(&Value::Object(doc.clone())).len() + r#","widgets":[],"next":null"#.len();
    let budget = PAGE_BUDGET.saturating_sub(base);
    let from = (q.from as usize).min(entries.len());
    let (mut page, mut used, mut next) = (Vec::new(), 0usize, None);
    for (k, e) in entries[from..].iter().enumerate() {
        let len = ascii_json(e).len() + 1;
        if !page.is_empty() && used + len > budget {
            next = Some((from + k) as u32);
            break;
        }
        used += len;
        page.push(e.clone());
    }
    page.sort_by(|a, b| cost_order((cost_of(a), name_of(a)), (cost_of(b), name_of(b))));
    doc.insert("widgets".into(), Value::Array(page));
    doc.insert("next".into(), json!(next));
    finish(Value::Object(doc)).map(|line| (line, next))
}

// ------------------------------------------------------------------ trees

/// A node without its children: kind, names, text, box, states, handlers
/// and, when asked for, its style (what differs from the initial style or,
/// for an inherited property, from the parent).
fn node_base(n: &InspectNode, parent_style: Option<&ComputedStyle>, redact: bool, root: bool) -> Value {
    let mut m = Map::new();
    m.insert("kind".into(), json!(n.kind.css_name()));
    m.insert("id".into(), json!(n.id.as_deref()));
    m.insert("classes".into(), json!(n.classes.iter().map(|c| c.as_ref()).collect::<Vec<_>>()));
    let has_text = n.text.is_some() || n.password;
    let text = if !has_text {
        Value::Null
    } else if redact || n.password {
        json!(chars_text(n.text_len))
    } else {
        let t = n.text.as_deref().unwrap_or("");
        if t.chars().count() > MAX_TEXT {
            json!(format!("{}...", t.chars().take(MAX_TEXT).collect::<String>()))
        } else {
            json!(t)
        }
    };
    m.insert("text".into(), text);
    m.insert("text_len".into(), json!(n.text_len));
    if let Some(src) = &n.image {
        m.insert("src".into(), if redact { json!(chars_text(src.chars().count())) } else { json!(src.as_ref()) });
    }
    let rect =
        if n.hidden && !root { Value::Null } else { json!(n.rect.map(|v| (f64::from(v) * 10.0).round() / 10.0)) };
    m.insert("rect".into(), rect);
    m.insert("hidden".into(), json!(n.hidden));
    m.insert("states".into(), json!(n.state_names()));
    m.insert("handlers".into(), json!(n.handler_names()));
    if let Some(style) = &n.style {
        let initial = ComputedStyle::default();
        let mut s = Map::new();
        for (name, value) in css_text::differing(style, &initial, parent_style) {
            s.insert(name.to_string(), json!(value));
        }
        m.insert("style".into(), Value::Object(s));
    }
    Value::Object(m)
}

/// The result of cutting one list of nodes to a byte budget.
struct Cut {
    value: Value,
    shown: usize,
    /// Nodes dropped because they did not fit the budget.
    dropped: bool,
}

/// Builds the nested JSON of `nodes` (breadth first, so a prefix is always a
/// tree), keeping as many nodes as fit `budget` bytes. `extras` are entries
/// added as children of the node at that index (declared popups).
fn build_nodes(nodes: &InstanceNodes, budget: usize, redact: bool, extras: &mut HashMap<usize, Vec<Value>>) -> Cut {
    let n = nodes.nodes.len();
    let bases: Vec<Value> = nodes
        .nodes
        .iter()
        .map(|x| {
            let parent = x.parent.and_then(|p| nodes.nodes.get(p as usize)).and_then(|p| p.style.as_deref());
            let parent = parent.or(if x.parent.is_none() { nodes.base_style.as_deref() } else { None });
            node_base(x, parent, redact, false)
        })
        .collect();
    // A node costs its own text, the key of its child list and a comma.
    let sizes: Vec<usize> = bases.iter().map(|b| ascii_json(b).len() + 36).collect();
    let (mut keep, mut used) = (0usize, 0usize);
    while keep < n && (keep == 0 || used + sizes[keep] <= budget) {
        used += sizes[keep];
        keep += 1;
    }
    let mut kids: Vec<Vec<usize>> = vec![Vec::new(); keep];
    for (i, x) in nodes.nodes.iter().enumerate().take(keep) {
        if let Some(p) = x.parent
            && (p as usize) < keep
        {
            kids[p as usize].push(i);
        }
    }
    let mut built: Vec<Option<Value>> = vec![None; keep];
    for i in (0..keep).rev() {
        let mut children: Vec<Value> = kids[i].iter().filter_map(|c| built[*c].take()).collect();
        let kept = children.len() as u32;
        if let Some(extra) = extras.remove(&i) {
            children.extend(extra);
        }
        let mut v = bases[i].clone();
        v["children"] = Value::Array(children);
        v["children_cut"] = json!(nodes.nodes[i].children.saturating_sub(kept));
        built[i] = Some(v);
    }
    let mut roots: Vec<Value> = nodes
        .nodes
        .iter()
        .enumerate()
        .take(keep)
        .filter(|(_, x)| x.parent.is_none())
        .filter_map(|(i, _)| built[i].take())
        .collect();
    let value = match roots.len() {
        0 => Value::Null,
        1 => roots.remove(0),
        _ => Value::Array(roots),
    };
    Cut { value, shown: keep, dropped: keep < n }
}

struct TreeOut {
    value: Value,
    shown: usize,
    total: usize,
    reason: Option<&'static str>,
}

fn tree_value(t: &UiTree, budget: usize, redact: bool) -> TreeOut {
    // Declared popups first: a closed one is a short line, an open one carries its own nodes.
    let mut extras: HashMap<usize, Vec<Value>> = HashMap::new();
    let (mut shown, mut total, mut dropped) = (0usize, 0usize, false);
    let mut spent = 0usize;
    let open_budget = budget / 4;
    let open_count = t.nodes.popups.iter().zip(&t.open).filter(|(p, o)| p.open && o.is_some()).count().max(1);
    for (p, content) in t.nodes.popups.iter().zip(&t.open) {
        let mut entry = json!({"kind": "popup", "name": p.name, "popup_kind": p.kind.css_name(),
                               "open": p.open, "nodes": p.nodes});
        if let Some(c) = content {
            let mut none = HashMap::new();
            let cut = build_nodes(c, open_budget / open_count, redact, &mut none);
            entry["children"] = Value::Array(vec![cut.value]);
            shown += cut.shown;
            total += c.total;
            dropped |= cut.dropped;
        }
        spent += ascii_json(&entry).len() + 1;
        // A popup not hanging from a copied node hangs from the widget root.
        extras.entry(p.anchor.unwrap_or(0) as usize).or_default().push(entry);
    }
    let main = build_nodes(&t.nodes, budget.saturating_sub(spent), redact, &mut extras);
    shown += main.shown;
    total += t.nodes.total;
    dropped |= main.dropped;
    let reason = if dropped {
        Some("reply limit")
    } else if t.nodes.limit_hit {
        Some("node limit")
    } else if shown < total {
        Some("depth")
    } else {
        None
    };
    TreeOut { value: main.value, shown, total, reason }
}

/// The reply to a widget request: each matching instance with its tree.
pub fn widgets(f: &Facts, snap: &Snapshot, q: &InspectQuery) -> Result<String, Refused> {
    let ws = matching(snap, q);
    let services = snap.services.as_deref().unwrap_or(&[]);
    let mut per = if ws.is_empty() { PAGE_BUDGET } else { PAGE_BUDGET / ws.len() };
    for _ in 0..12 {
        let mut doc = header(f);
        let mut list = Vec::new();
        for w in &ws {
            let ui = ui_instance(snap, w.instance.0);
            let mut e = widget_entry(w, row_of(snap, w.instance.0), ui.map(|(_, i)| i), services, q.redact);
            e["tree"] = Value::Null;
            e["pending"] = Value::Null;
            e["frame_ms_ago"] = Value::Null;
            if let Some((bar, inst)) = ui {
                e["pending"] = json!(inst.pending);
                e["frame_ms_ago"] = json!(bar.last_frame_ms_ago);
                if let Some(tree) = &inst.tree {
                    let own = ascii_json(&e).len();
                    let out = tree_value(tree, per.saturating_sub(own), q.redact);
                    e["tree"] = out.value;
                    if let Some(reason) = out.reason {
                        e["cut"] = json!({"shown": out.shown, "not_shown": out.total - out.shown,
                                          "total": out.total, "reason": reason});
                    }
                }
            }
            list.push(e);
        }
        doc.insert("widgets".into(), Value::Array(list));
        doc.insert("parts".into(), parts(snap));
        doc.insert("next".into(), Value::Null);
        match finish(Value::Object(doc)) {
            Ok(line) => return Ok(line),
            Err(_) => per = per * 85 / 100,
        }
    }
    Err(Refused("reply limit"))
}

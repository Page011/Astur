//! `rsb inspect`: what each widget draws and costs.
//!
//! The command line pages the summary and asks for one widget at a time;
//! the running bar gathers each answer from the script, service and UI
//! threads on its pipe thread. Nothing here writes to the log when it
//! serves a request.

use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

use rsb_core::config::WidgetRef;
use rsb_core::ipc::{Command, MAX_INSPECT_DEPTH, Reply};
use serde_json::Value;

use crate::pipe::{self, SendError};
use crate::stats;
use crate::status::{self, Snapshot, Sources, ascii_json};

mod format;
mod text;
mod ui;

pub use format::{Facts, PAGE_BUDGET, REPLY_LIMIT, Refused, cost_order, secret_key};
pub use text::table;
pub use ui::{BarView, DEFAULT_ROOT_VARS, MAX_NODES, PopupView, UiBar, UiInstance, UiPart, UiTree, ui_part};

/// Most summary pages the command line asks for; each holds about a hundred
/// widgets, so this is never reached by a real bar.
const MAX_PAGES: usize = 100;

/// One request to the running bar: the fields of `ipc::Command::Inspect`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectQuery {
    /// None: the summary of every widget, from the `from`th.
    pub widget: Option<WidgetRef>,
    pub bar: Option<String>,
    pub from: u32,
    /// 1..=`MAX_INSPECT_DEPTH`.
    pub depth: u8,
    pub style: bool,
    pub redact: bool,
}

impl Default for InspectQuery {
    fn default() -> Self {
        InspectQuery { widget: None, bar: None, from: 0, depth: MAX_INSPECT_DEPTH, style: false, redact: false }
    }
}

impl InspectQuery {
    fn command(&self) -> Command {
        Command::Inspect {
            widget: self.widget.clone(),
            bar: self.bar.clone(),
            from: self.from,
            depth: self.depth,
            style: self.style,
            redact: self.redact,
        }
    }
}

/// What `rsb inspect` was given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectOpts {
    pub widget: Option<WidgetRef>,
    pub bar: Option<String>,
    pub style: bool,
    pub depth: u8,
    pub redact: bool,
    /// One JSON document on stdout instead of the text.
    pub json: bool,
}

/// `rsb inspect`: asks the running bar (paging the summary), prints the text
/// or the JSON. Exit codes: 0 shown (a cut is still 0, and said), 1 no widget
/// matches, 2 bad usage, 3 no bar is running, 4 the bar could not be reached.
pub fn run(opts: &InspectOpts) -> ExitCode {
    let mut send = |c: &Command| pipe::send(c, Duration::from_secs(2));
    let code = run_with(opts, &mut send, &mut std::io::stdout(), &mut std::io::stderr());
    ExitCode::from(code)
}

/// `run` with the pipe, stdout and stderr replaced, so the exit codes and
/// the output are testable without a bar.
pub fn run_with(
    opts: &InspectOpts,
    send: &mut dyn FnMut(&Command) -> Result<Reply, SendError>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    if opts.style && opts.widget.is_none() && opts.bar.is_none() {
        let _ = writeln!(err, "rsb inspect: --style needs a widget or --bar");
        return 2;
    }
    let base = InspectQuery {
        widget: opts.widget.clone(),
        bar: opts.bar.clone(),
        from: 0,
        depth: opts.depth.clamp(1, MAX_INSPECT_DEPTH),
        style: opts.style,
        redact: opts.redact,
    };
    let mut ask = |query: &InspectQuery| -> Result<Value, u8> {
        match send(&query.command()) {
            Ok(Reply::Ok(text)) => serde_json::from_str::<Value>(&text).map_err(|_| {
                let _ = writeln!(err, "rsb inspect: the bar's answer could not be read");
                1
            }),
            Ok(Reply::Err(text)) => {
                let _ = writeln!(err, "rsb inspect: {text}");
                Err(1)
            }
            Err(SendError::NoBar) => {
                let _ = writeln!(err, "rsb inspect: no bar running");
                Err(3)
            }
            Err(SendError::Refused(why)) => {
                let _ = writeln!(err, "rsb inspect: the bar could not be reached: {why}");
                Err(4)
            }
        }
    };

    let mut doc = if opts.widget.is_some() {
        match ask(&base) {
            Ok(d) => d,
            Err(code) => return code,
        }
    } else {
        match summary_pages(&base, &mut ask) {
            Ok(d) => d,
            Err(code) => return code,
        }
    };
    sort_widgets(&mut doc);

    let empty = doc["widgets"].as_array().is_none_or(Vec::is_empty);
    if let (Some(w), true) = (&opts.widget, empty) {
        // Name what is there, so a typo gets its suggestion.
        let all = InspectQuery { widget: None, bar: None, from: 0, ..base.clone() };
        let names: Vec<String> = match summary_pages(&all, &mut ask) {
            Ok(d) => d["widgets"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| e["widget"].as_str().map(str::to_string))
                .collect(),
            Err(code) => return code,
        };
        let on = opts.bar.as_deref().map_or(String::new(), |b| format!(" on bar '{b}'"));
        let hint = rsb_core::suggest::hint(&w.to_string(), names.iter().map(String::as_str));
        let _ = writeln!(err, "rsb inspect: no widget '{w}' on the bar{on}{hint}");
        return 1;
    }
    if opts.widget.is_none() && opts.bar.is_some() && doc["bars"].as_array().is_none_or(Vec::is_empty) {
        let _ = writeln!(err, "rsb inspect: no bar named '{}'", opts.bar.as_deref().unwrap_or(""));
        return 1;
    }

    let shown = if opts.json { ascii_json(&doc) } else { table(&doc) };
    let _ = writeln!(out, "{shown}");
    0
}

/// Asks for pages of the summary until there is no next one and joins
/// them into one document: the first page with every page's widgets.
fn summary_pages(base: &InspectQuery, ask: &mut dyn FnMut(&InspectQuery) -> Result<Value, u8>) -> Result<Value, u8> {
    let mut query = InspectQuery { widget: None, from: 0, ..base.clone() };
    let mut doc = ask(&query)?;
    let mut widgets: Vec<Value> = doc["widgets"].as_array().cloned().unwrap_or_default();
    let mut seen: Vec<u64> = widgets.iter().filter_map(|w| w["instance"].as_u64()).collect();
    let mut next = doc["next"].as_u64();
    for _ in 0..MAX_PAGES {
        let Some(from) = next else { break };
        query.from = from as u32;
        let page = ask(&query)?;
        for w in page["widgets"].as_array().into_iter().flatten() {
            // A widget the bar listed twice (it moved between pages) is shown once.
            if w["instance"].as_u64().is_none_or(|i| !seen.contains(&i)) {
                seen.extend(w["instance"].as_u64());
                widgets.push(w.clone());
            }
        }
        let after = page["next"].as_u64();
        if after.is_some_and(|a| a <= u64::from(query.from)) {
            break;
        }
        next = after;
    }
    doc["widgets"] = Value::Array(widgets);
    doc["next"] = Value::Null;
    Ok(doc)
}

/// Costliest first, then by name; the same rule in the text and the JSON.
fn sort_widgets(doc: &mut Value) {
    if let Some(list) = doc["widgets"].as_array_mut() {
        list.sort_by(|a, b| {
            let cost = |w: &Value| w["script"]["ms_last_60s"].as_f64().unwrap_or(0.0);
            let name = |w: &Value| w["widget"].as_str().unwrap_or("").to_string();
            cost_order((cost(a), &name(a)), (cost(b), &name(b)))
                .then_with(|| a["instance"].as_u64().cmp(&b["instance"].as_u64()))
        });
    }
}

/// The reply to one inspect line, on the pipe thread. `Reply::Ok` holds one
/// line of ASCII JSON under 65,536 bytes.
pub fn serve(query: &InspectQuery, sources: &Sources) -> Reply {
    let snap = status::snapshot(sources, Some(query));
    let (cpu_last_60s, cpu_window_ms) = stats::cpu_last_minute();
    let facts = Facts {
        uptime: sources.started.elapsed(),
        wall_ms: stats::wall_ms(),
        session: stats::session_name(),
        cpu_ms: stats::cpu_ms(),
        cpu_last_60s,
        cpu_window_ms,
    };
    reply_for(query, &snap, &facts)
}

/// The reply for a snapshot already taken. A reply that cannot be made to fit
/// is refused with one log line that names only the reason.
pub fn reply_for(query: &InspectQuery, snap: &Snapshot, facts: &Facts) -> Reply {
    let made = if query.widget.is_some() {
        format::widgets(facts, snap, query)
    } else {
        format::summary(facts, snap, query).map(|(line, _)| line)
    };
    match made {
        Ok(line) => Reply::Ok(line),
        Err(Refused(why)) => {
            log::warn!("inspect refused: {why}");
            Reply::Err(format!("inspect refused: {why}"))
        }
    }
}

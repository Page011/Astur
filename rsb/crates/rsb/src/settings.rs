//! `rsb settings`: the verbs a settings page or a script calls.
//!
//! One function, `serve`, answers every request. The command line runs it in
//! its own process (so it works with no bar running); the bar's pipe thread
//! runs it for a `settings` line. The two give the same answer except for
//! what depends on the transport: `widgets` is answered by the command line
//! only, a write on the pipe never waits for the lock and is served only to a
//! client at the bar's own level of rights, and only the command line waits
//! for the bar to show a change.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;

use rsb_core::config::BUILTIN_PRESETS;
use rsb_core::contract::{ErrorCode, Reply, Request, SettingsError, Verb, check_contract};
use serde_json::{Value, json};

use crate::msg::AppliedState;
use crate::preview::PreviewOpts;

pub mod backend;
pub mod bar;
mod change;
pub mod grammar;
pub mod read;
pub mod real;
pub mod run;
pub mod schema;
pub mod widgets;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod goldens;
#[cfg(test)]
mod tests;

use backend::Backend;
use grammar::PreviewArgs;

/// Who is asking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// `rsb settings ...` in its own process.
    Cli,
    /// A `settings` line on the running bar's pipe.
    Pipe,
}

/// What a request is served against.
#[derive(Clone, Debug)]
pub struct Ctx {
    pub transport: Transport,
    /// The config folder the request reads and writes.
    pub config_dir: PathBuf,
    /// Where the log line of a write goes (the command line writes it; the
    /// bar logs its own).
    pub log_dir: PathBuf,
    /// The bar's own record of the files it shows, when this process is the
    /// bar (the pipe); the command line asks the bar with `applied`.
    pub applied: Option<Arc<AppliedState>>,
    /// `--no-wait`: do not ask the bar whether it shows a change.
    pub no_wait: bool,
    /// Over the pipe: the client's token is at or above the bar's level of
    /// rights, which a write needs. Always true for the command line.
    pub pipe_client_ok: bool,
}

impl Ctx {
    /// The command line in its own process.
    pub fn cli(config_dir: PathBuf, log_dir: PathBuf, no_wait: bool) -> Ctx {
        Ctx { transport: Transport::Cli, config_dir, log_dir, applied: None, no_wait, pipe_client_ok: true }
    }

    /// A request on the bar's pipe thread. Reads who the client is from the
    /// connection being served, so call it from the pipe handler only.
    pub fn pipe(config_dir: PathBuf, log_dir: PathBuf, applied: Arc<AppliedState>) -> Ctx {
        Ctx {
            transport: Transport::Pipe,
            config_dir,
            log_dir,
            applied: Some(applied),
            no_wait: true,
            pipe_client_ok: crate::pipe::client_may_write(),
        }
    }
}

fn internal() -> SettingsError {
    SettingsError::new(ErrorCode::Internal, "the request could not be answered; nothing was written")
        .with_next("report it with rsb.log")
}

/// Answers one request. Never panics on a bad request: every refusal is a
/// `Reply::Err` with its code, and a fault inside is `internal`, so the
/// bar's pipe thread survives it.
pub fn serve(request: Request, ctx: &Ctx) -> Reply {
    guarded(&request, ctx, &real::Real)
}

/// `serve_with`, with a fault inside turned into `internal`.
fn guarded(request: &Request, ctx: &Ctx, be: &dyn Backend) -> Reply {
    catch_unwind(AssertUnwindSafe(|| serve_with(request, ctx, be))).unwrap_or_else(|_| Reply::Err(internal()))
}

/// The pipe line `settings <json>`: reads the request, then `serve`s it.
pub fn serve_line(text: &str, ctx: &Ctx) -> Reply {
    line_with(text, ctx, &real::Real)
}

/// The longest reply text the pipe carries: `ok `, the text and the newline
/// fit in `MAX_REPLY_BYTES`.
const PIPE_TEXT_LIMIT: usize = rsb_core::ipc::MAX_REPLY_BYTES - 4;

/// A reply too long for the pipe is turned into a refusal that names the
/// command line, so the caller gets a code instead of the pipe's bare text.
fn fit_pipe(reply: Reply, ctx: &Ctx) -> Reply {
    if ctx.transport == Transport::Pipe && reply.to_line().len() > PIPE_TEXT_LIMIT {
        return Reply::Err(
            SettingsError::new(ErrorCode::CliOnly, "the answer is too long for the pipe")
                .with_next("run rsb settings with the same words, or ask for fewer entries with --last"),
        );
    }
    reply
}

/// `serve_line` against any backend (tests pass a fake).
fn line_with(text: &str, ctx: &Ctx, be: &dyn Backend) -> Reply {
    // `preview` draws PNGs, which is not work for the bar's process: it is not
    // a verb of the request, so it is named here to be refused as the
    // command line only.
    let asks_for_preview =
        serde_json::from_str::<Value>(text).is_ok_and(|v| v.get("verb").and_then(Value::as_str) == Some("preview"));
    if asks_for_preview && ctx.transport == Transport::Pipe {
        return Reply::Err(
            SettingsError::new(ErrorCode::CliOnly, "preview is answered by rsb.exe, not over the pipe")
                .with_next("run rsb settings preview"),
        );
    }
    match Request::from_json(text) {
        Ok(request) => fit_pipe(guarded(&request, ctx, be), ctx),
        Err(e) => Reply::Err(e),
    }
}

/// `serve` against any backend (tests pass a fake).
pub fn serve_with(request: &Request, ctx: &Ctx, be: &dyn Backend) -> Reply {
    if let Err(e) = check_contract(request.contract) {
        return Reply::Err(e);
    }
    let verb = match (request.verb, request.changes.is_empty()) {
        (Some(v), _) => v,
        (None, false) => Verb::Change,
        (None, true) => {
            return Reply::Err(
                SettingsError::new(ErrorCode::BadUsage, "the request has no verb")
                    .with_next("name one of get, presets, widgets, schema, change, undo, restore, log"),
            );
        }
    };
    match verb {
        Verb::Get => read::get(request, ctx, be),
        Verb::Presets => read::presets(ctx, be),
        Verb::Schema => read::schema_reply(),
        Verb::Log => read::log(request, ctx, be),
        Verb::Widgets if ctx.transport == Transport::Pipe => Reply::Err(
            SettingsError::new(ErrorCode::CliOnly, "widgets is answered by rsb.exe, not over the pipe")
                .with_next("run rsb settings widgets"),
        ),
        Verb::Widgets => read::widgets(ctx, be),
        Verb::Change => change::change(request, ctx, be),
        Verb::Undo => change::undo(request, ctx, be),
        Verb::Restore => change::restore(request, ctx, be),
    }
}

fn round2(v: f32) -> f64 {
    (f64::from(v) * 100.0).round() / 100.0
}

/// `rsb settings preview`: the command line only.
pub fn preview(args: &PreviewArgs, ctx: &Ctx, be: &dyn Backend) -> Reply {
    let names: Vec<String> =
        if args.all { BUILTIN_PRESETS.iter().map(|s| (*s).to_string()).collect() } else { args.presets.clone() };
    for name in &names {
        if !BUILTIN_PRESETS.contains(&name.as_str()) {
            let hint = rsb_core::suggest::hint(name, BUILTIN_PRESETS.iter().copied());
            return Reply::Err(
                SettingsError::new(ErrorCode::UnknownPreset, format!("no preset '{name}'{hint}"))
                    .with_next("rsb settings presets lists them"),
            );
        }
    }
    let opts = PreviewOpts {
        config_dir: ctx.config_dir.clone(),
        cache_dir: args.cache_dir.clone(),
        themes: args.themes.clone(),
        width: args.width,
        scale: args.scale,
        mine: args.mine,
        zones_from_preset: args.zones_from_preset,
    };
    match be.previews(&names, &opts) {
        Ok(previews) => Reply::Ok(json!({
            "kind": "preview",
            "previews": previews.iter().map(|p| json!({
                "name": p.name,
                "theme": p.theme.name(),
                "mine": p.mine,
                "zones": if p.zones_from_preset { "preset" } else { "mine" },
                "path": p.path.to_string_lossy(),
                "width": p.width,
                "height": p.height,
                "scale": round2(p.scale),
                "cached": p.cached,
            })).collect::<Vec<Value>>(),
        })),
        Err(e) => Reply::Err(e),
    }
}

//! The writing verbs: `change` (and every verb that builds one), `undo` and
//! `restore`. Each is checked, planned by the edit engine, written by the
//! writer, logged, and then the bar is asked whether it shows the files.

use std::path::Path;

use rsb_core::contract::{ConfigFile, ErrorCode, FileHash, Notice, Op, Reply, Request, SettingsError, fixes};
use serde_json::{Value, json};

use super::backend::Backend;
use super::bar;
use super::{Ctx, Transport};
use crate::cfgwrite::{self, Applied, WriteOpts};

fn bad_usage(message: impl Into<String>, next: &str) -> SettingsError {
    SettingsError::new(ErrorCode::BadUsage, message).with_next(next)
}

/// Writes go to the default config folder, to a folder that does not exist
/// yet, or to one that already holds `bar.toml` or `style.css`: a
/// misspelt `--config-dir` must not turn another folder into a config
/// folder.
fn check_folder(dir: &Path) -> Result<(), SettingsError> {
    let Ok(meta) = std::fs::metadata(dir) else { return Ok(()) };
    let hint = "pass the folder that holds bar.toml, or leave out --config-dir";
    if !meta.is_dir() {
        return Err(bad_usage(format!("{} is not a folder", cfgwrite::shown_path(dir)), hint));
    }
    if cfgwrite::is_config_folder(dir) {
        return Ok(());
    }
    Err(bad_usage(format!("{} is not an RSB config folder", cfgwrite::shown_path(dir)), hint))
}

/// What a folder or a link is doing where a config file belongs, if it is.
fn link_kind(dir: &Path, file: ConfigFile) -> Option<&'static str> {
    let meta = dir.join(file.name()).symlink_metadata().ok()?;
    if meta.file_type().is_symlink() {
        Some("a link to another file")
    } else if meta.is_dir() {
        Some("a folder")
    } else {
        None
    }
}

/// `file_broken` names a way out: `restore` only when a copy that loaded
/// clean is kept.
fn broken_next(mut e: SettingsError, ctx: &Ctx, be: &dyn Backend) -> SettingsError {
    if e.code != ErrorCode::FileBroken {
        return e;
    }
    let Some(file) = e.file else { return e };
    // A broken file that style.css pulls in is fixed where it is; restore puts back style.css only.
    let elsewhere = e.at.as_deref().filter(|at| !at.starts_with(file.name()));
    let open = |name: &str| fixes::open(&cfgwrite::shown_path(&ctx.config_dir.join(name)));
    if let Some(at) = elsewhere {
        let name = at.split(':').next().unwrap_or(file.name()).to_string();
        e.next = Some(format!("fix that line in {at}"));
        e.fix = Some(Box::new(open(&name)));
    } else if be.has_last_good(&ctx.config_dir, file) {
        e.next = Some(format!("fix that line, or run: rsb settings restore {file} (your broken copy is kept)"));
        e.fix = Some(Box::new(fixes::restore(file)));
    } else {
        e.next = Some("fix that line by hand".to_string());
        e.fix = Some(Box::new(open(file.name())));
    }
    e
}

/// The remedy of a refusal as data, where it has one a page can act on.
fn with_fix(mut e: SettingsError, ctx: &Ctx) -> SettingsError {
    if e.fix.is_some() {
        return e;
    }
    let fix = match (e.code, e.file) {
        (ErrorCode::FileChanged, _) => Some(fixes::verb("get")),
        (
            ErrorCode::ColorsBlockEdited
            | ErrorCode::ColorsBlockBroken
            | ErrorCode::NotUtf8
            | ErrorCode::ReadOnly
            | ErrorCode::NothingToRestore
            | ErrorCode::LinkedFile,
            Some(file),
        ) => Some(fixes::open(&cfgwrite::shown_path(&ctx.config_dir.join(file.name())))),
        _ => None,
    };
    e.fix = fix.map(Box::new);
    e
}

/// A write over the pipe is served only to a client that runs at the bar's
/// integrity level or above.
fn check_client(ctx: &Ctx) -> Result<(), SettingsError> {
    if ctx.transport == Transport::Pipe && !ctx.pipe_client_ok {
        return Err(SettingsError::new(
            ErrorCode::CliOnly,
            "settings are changed over the pipe only by a program that runs at the bar's own level of rights",
        )
        .with_next("run rsb settings from a normal window"));
    }
    Ok(())
}

fn log_config(ctx: &Ctx, be: &dyn Backend, message: &str) {
    match ctx.transport {
        Transport::Pipe => log::info!(target: "config", "{message}"),
        Transport::Cli => be.append_log(&ctx.log_dir, message),
    }
}

/// One log line for a refusal that is not a mistake in the request.
fn log_refusal(ctx: &Ctx, be: &dyn Backend, e: &SettingsError, dry_run: bool) {
    if dry_run || e.exit_code() == 2 {
        return;
    }
    if let Some(line) = cfgwrite::refusal_line(e) {
        log_config(ctx, be, &line);
    }
}

fn hash_text(h: FileHash) -> String {
    h.to_string()
}

/// What a write left behind, in the shape of the change result.
struct Outcome<'a> {
    applied: &'a Applied,
    plan_notices: Vec<Notice>,
    preset_switch: Value,
    accent: Value,
    /// What the bar had swapped in before the write (see `bar::after_change`).
    loads_before: Option<u64>,
}

fn finish(r: Outcome<'_>, request: &Request, ctx: &Ctx, be: &dyn Backend) -> Reply {
    let applied = r.applied;
    let files: serde_json::Map<String, Value> = applied
        .files
        .iter()
        .map(|f| {
            (
                f.file.name().to_string(),
                json!({"hash": hash_text(f.hash), "written": f.written, "edited_by_hand": f.edited_by_hand}),
            )
        })
        .collect();
    let all: Vec<(ConfigFile, Option<FileHash>)> = applied.files.iter().map(|f| (f.file, Some(f.hash))).collect();
    let written: Vec<(ConfigFile, Option<FileHash>)> =
        applied.files.iter().filter(|f| f.written).map(|f| (f.file, Some(f.hash))).collect();
    let mut notices = r.plan_notices;
    notices.extend(applied.notices.clone());
    // The bar's own thread is the one asking over the pipe, so the answer is its record as of
    // the write; `--no-wait` and a dry run ask nobody.
    let asks = !request.dry_run && (ctx.transport == Transport::Pipe || !ctx.no_wait);
    let bar = if !asks {
        bar::not_asked()
    } else {
        let after = bar::after_change(be, ctx, &all, &written, ctx.transport == Transport::Cli, r.loads_before);
        notices.extend(after.notices);
        after.bar
    };
    Reply::Ok(json!({
        "kind": "change",
        "dry_run": request.dry_run,
        "files": files,
        "undo": applied.undo,
        "preset_switch": r.preset_switch,
        "accent": r.accent,
        "kept_copy": applied.kept_copy,
        "notices": notices.iter().map(|n| serde_json::to_value(n).unwrap_or(Value::Null)).collect::<Vec<_>>(),
        "bar": bar,
    }))
}

fn write_opts(request: &Request, ctx: &Ctx) -> WriteOpts {
    WriteOpts { dry_run: request.dry_run, wait_lock: ctx.transport == Transport::Cli, expect: request.expect.clone() }
}

pub fn change(request: &Request, ctx: &Ctx, be: &dyn Backend) -> Reply {
    match change_inner(request, ctx, be) {
        Ok(reply) => reply,
        Err(e) => {
            log_refusal(ctx, be, &e, request.dry_run);
            Reply::Err(with_fix(e, ctx))
        }
    }
}

fn change_inner(request: &Request, ctx: &Ctx, be: &dyn Backend) -> Result<Reply, SettingsError> {
    if request.changes.is_empty() {
        return Err(bad_usage(
            "the request has no changes",
            "send {\"changes\":[{\"op\":\"set\",...}]} as in docs\\settings.md",
        ));
    }
    if request.changes.iter().any(|c| matches!(c, Op::Undo { .. } | Op::Restore { .. })) {
        return Err(bad_usage(
            "undo and restore are commands of their own, not changes",
            "use rsb settings undo or rsb settings restore",
        ));
    }
    check_client(ctx)?;
    check_folder(&ctx.config_dir)?;
    let loaded = be.load(&ctx.config_dir, false);
    let linked: Vec<(ConfigFile, &'static str)> =
        ConfigFile::ALL.iter().filter_map(|f| link_kind(&ctx.config_dir, *f).map(|k| (*f, k))).collect();
    for file in ConfigFile::ALL {
        if linked.iter().any(|(f, _)| *f == file) {
            continue;
        }
        if let Some(why) = &loaded.file(file).unreadable {
            return Err(SettingsError::new(ErrorCode::WriteFailed, format!("cannot read {file}: {why}"))
                .with_file(file)
                .with_next("check the file is not locked by another program"));
        }
    }
    let plan = be
        .plan(&ctx.config_dir, &loaded.texts(), &request.changes, request.reset, ctx.transport == Transport::Cli)
        .map_err(|e| broken_next(e, ctx, be))?;
    for fp in &plan.files {
        if let Some((_, kind)) = linked.iter().find(|(f, _)| *f == fp.file) {
            return Err(SettingsError::new(ErrorCode::LinkedFile, format!("{} is {kind}", fp.file))
                .with_file(fp.file)
                .with_next("change the file it points to by hand"));
        }
        if !loaded.file(fp.file).utf8 {
            return Err(SettingsError::new(ErrorCode::NotUtf8, format!("{} is not UTF-8", fp.file))
                .with_file(fp.file)
                .with_next(format!("save {} as UTF-8", fp.file)));
        }
    }
    // The bar keeps its last good sheet when a file that style.css pulls in is broken, so a
    // change to style.css would be written and never shown.
    if plan.file(ConfigFile::StyleCss).is_some()
        && let Some(err) = &loaded.style_error
    {
        return Err(broken_next(
            SettingsError::new(ErrorCode::FileBroken, format!("{}: {}", err.at, err.message))
                .with_file(ConfigFile::StyleCss)
                .with_at(err.at.clone()),
            ctx,
            be,
        ));
    }
    let loads_before = bar::loads_before(be, ctx, request.dry_run);
    let applied = be.apply(&ctx.config_dir, &plan, &write_opts(request, ctx))?;
    if !request.dry_run {
        for line in cfgwrite::change_lines(&plan, &applied) {
            log_config(ctx, be, &line);
        }
    }
    let preset_switch =
        plan.preset_switch.as_ref().map_or(Value::Null, |p| serde_json::to_value(p).unwrap_or(Value::Null));
    let accent = plan.accent.as_ref().map_or(Value::Null, |a| serde_json::to_value(a).unwrap_or(Value::Null));
    Ok(finish(
        Outcome { applied: &applied, plan_notices: plan.notices.clone(), preset_switch, accent, loads_before },
        request,
        ctx,
        be,
    ))
}

pub fn undo(request: &Request, ctx: &Ctx, be: &dyn Backend) -> Reply {
    match undo_inner(request, ctx, be) {
        Ok(reply) => reply,
        Err(e) => {
            log_refusal(ctx, be, &e, request.dry_run);
            Reply::Err(with_fix(e, ctx))
        }
    }
}

fn undo_inner(request: &Request, ctx: &Ctx, be: &dyn Backend) -> Result<Reply, SettingsError> {
    check_client(ctx)?;
    check_folder(&ctx.config_dir)?;
    let newest = be.log_entries(&ctx.config_dir, 1).ok().and_then(|e| e.first().map(|e| e.n));
    let loads_before = bar::loads_before(be, ctx, request.dry_run);
    let applied = be.undo(&ctx.config_dir, request.n, &write_opts(request, ctx))?;
    if !request.dry_run
        && let Some(n) = request.n.or(newest)
    {
        for line in cfgwrite::undo_lines(n, &applied) {
            log_config(ctx, be, &line);
        }
    }
    Ok(finish(
        Outcome {
            applied: &applied,
            plan_notices: Vec::new(),
            preset_switch: Value::Null,
            accent: Value::Null,
            loads_before,
        },
        request,
        ctx,
        be,
    ))
}

pub fn restore(request: &Request, ctx: &Ctx, be: &dyn Backend) -> Reply {
    match restore_inner(request, ctx, be) {
        Ok(reply) => reply,
        Err(e) => {
            log_refusal(ctx, be, &e, request.dry_run);
            Reply::Err(with_fix(e, ctx))
        }
    }
}

fn restore_inner(request: &Request, ctx: &Ctx, be: &dyn Backend) -> Result<Reply, SettingsError> {
    let Some(file) = request.file else {
        return Err(bad_usage("restore needs a file", "usage: rsb settings restore <bar.toml|style.css>"));
    };
    check_client(ctx)?;
    check_folder(&ctx.config_dir)?;
    let loads_before = bar::loads_before(be, ctx, request.dry_run);
    let applied = be.restore(&ctx.config_dir, file, &write_opts(request, ctx))?;
    if !request.dry_run {
        for line in cfgwrite::restore_lines(&applied) {
            log_config(ctx, be, &line);
        }
    }
    Ok(finish(
        Outcome {
            applied: &applied,
            plan_notices: Vec::new(),
            preset_switch: Value::Null,
            accent: Value::Null,
            loads_before,
        },
        request,
        ctx,
        be,
    ))
}

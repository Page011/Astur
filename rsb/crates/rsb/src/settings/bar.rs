//! The running bar's side of a settings call: does it read this folder, does
//! it show these files, and for the command line, waiting until it does.

use std::path::Path;

use rsb_core::contract::{AppliedReply, ConfigFile, FileHash, Notice, NoticeCode};
use serde_json::{Value, json};

use super::backend::{Backend, BarState};
use super::{Ctx, Transport};

/// How soon the command line first asks the bar after a write, how often it
/// asks after that, and the longest it waits. The bar reloads within about 35
/// ms, so a first look at 5 ms and then steps of 25 ms keep `ms` close to the
/// bar's own time.
pub const FIRST_POLL_MS: u64 = 5;
pub const POLL_MS: u64 = 25;
pub const WAIT_CAP_MS: u64 = 2000;

fn normalise(path: &str) -> String {
    let path = path.replace('/', "\\");
    let path = path.strip_prefix(r"\\?\").unwrap_or(&path);
    path.trim_end_matches('\\').to_ascii_lowercase()
}

/// Whether the folder the bar reads is the one this call works in. The bar
/// reports its folder canonical; ours is canonicalised the same way when it
/// exists.
pub fn same_folder(ours: &Path, bars: &str) -> bool {
    let ours = std::fs::canonicalize(ours).unwrap_or_else(|_| ours.to_path_buf());
    normalise(&ours.to_string_lossy()) == normalise(bars)
}

fn applied_hash(a: &AppliedReply, file: ConfigFile) -> Option<FileHash> {
    match file {
        ConfigFile::BarToml => a.bar_toml,
        ConfigFile::StyleCss => a.style_css,
    }
}

/// The bar shows every one of these versions.
pub fn shows(a: &AppliedReply, expected: &[(ConfigFile, Option<FileHash>)]) -> bool {
    expected.iter().all(|(file, hash)| hash.is_some() && applied_hash(a, *file) == *hash)
}

/// The `bar` object of `get`.
pub fn get_json(state: &BarState, ctx: &Ctx, files: &[(ConfigFile, Option<FileHash>)]) -> Value {
    match state {
        BarState::NotRunning => {
            json!({"running": false, "shows_files": null, "diagnostics": null, "config_dir": null})
        }
        BarState::Unreachable => {
            json!({"running": true, "shows_files": false, "diagnostics": null, "config_dir": null})
        }
        BarState::Running(a) => {
            let same = same_folder(&ctx.config_dir, &a.config_dir);
            json!({
                "running": true,
                "shows_files": if same { Some(shows(a, files)) } else { None },
                "diagnostics": a.diagnostics,
                "config_dir": a.config_dir,
            })
        }
    }
}

/// The `bar` object of a change, and the notices the wait produced.
pub struct AfterChange {
    pub bar: Value,
    pub notices: Vec<Notice>,
}

fn notice(code: NoticeCode, message: &str, next: &str) -> Notice {
    Notice::new(code, message).with_next(next)
}

/// How many configs the bar had swapped in before a write, asked of it only when the command line is going to wait
/// for the bar afterwards.
pub fn loads_before(be: &dyn Backend, ctx: &Ctx, dry_run: bool) -> Option<u64> {
    (!dry_run && ctx.transport == Transport::Cli && !ctx.no_wait).then(|| be.bar_loads(ctx)).flatten()
}

/// Asks the bar about a change just written. `files` are the versions of
/// both files now; `written` are the ones this change wrote, which the bar
/// must show. With `wait` (the command line) it asks after `FIRST_POLL_MS`,
/// then every `POLL_MS`, until it does, at most `WAIT_CAP_MS`; otherwise it
/// asks once. A bar that already showed those versions (a restore of what it
/// shows) has also to have swapped a config in since `loads_before`, or the
/// diagnostics it reports are still those of the file the write replaced; if
/// that does not happen within the cap the answer is given as it stands, with
/// no notice, because bytes the bar read last are not reloaded.
pub fn after_change(
    be: &dyn Backend,
    ctx: &Ctx,
    files: &[(ConfigFile, Option<FileHash>)],
    written: &[(ConfigFile, Option<FileHash>)],
    wait: bool,
    loads_before: Option<u64>,
) -> AfterChange {
    let started = be.now_ms();
    loop {
        let state = be.bar(ctx);
        let mut notices = Vec::new();
        let bar = match &state {
            BarState::NotRunning => {
                json!({"running": false, "shows_files": null, "ms": null, "diagnostics": null, "config_dir": null})
            }
            BarState::Unreachable => {
                notices.push(notice(
                    NoticeCode::BarUnreachable,
                    "the bar did not answer",
                    "the change is saved; run rsb status",
                ));
                json!({"running": true, "shows_files": false, "ms": null, "diagnostics": null, "config_dir": null})
            }
            BarState::Running(a) if !same_folder(&ctx.config_dir, &a.config_dir) => {
                notices.push(
                    notice(
                        NoticeCode::BarUsesOtherFolder,
                        "the running bar reads another config folder",
                        &format!(
                            "the running bar reads {}; start it without --config-dir, or point the settings page at that folder",
                            a.config_dir
                        ),
                    )
                    .with_extra("bar_dir", Value::String(a.config_dir.clone())),
                );
                json!({"running": true, "shows_files": null, "ms": null, "diagnostics": a.diagnostics, "config_dir": a.config_dir})
            }
            BarState::Running(a) => {
                let target = if written.is_empty() { files } else { written };
                let now_shown = shows(a, target);
                let landed = loads_before.is_none_or(|before| a.loads > before);
                let elapsed = be.now_ms().saturating_sub(started);
                if wait && !(now_shown && landed) && !written.is_empty() {
                    if elapsed < WAIT_CAP_MS {
                        let step = if elapsed == 0 { FIRST_POLL_MS } else { POLL_MS };
                        be.sleep_ms(step.min(WAIT_CAP_MS - elapsed));
                        continue;
                    }
                    if !now_shown {
                        notices.push(notice(
                            NoticeCode::NotConfirmed,
                            "the bar did not show the new files within 2000 ms",
                            "the change is saved; if the bar still shows the old look, run rsb status",
                        ));
                    }
                }
                let ms = (wait && now_shown && !written.is_empty()).then_some(elapsed);
                json!({"running": true, "shows_files": now_shown, "ms": ms, "diagnostics": a.diagnostics, "config_dir": a.config_dir})
            }
        };
        return AfterChange { bar, notices };
    }
}

/// The `bar` object when nobody was asked (`--no-wait`, a dry run).
pub fn not_asked() -> Value {
    json!({"running": null, "shows_files": null, "ms": null, "diagnostics": null, "config_dir": null})
}

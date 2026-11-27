//! `rsb settings ...` as a process: builds the request from the words (a
//! `change` file or standard input is read here), serves it in this process
//! and prints the one line of JSON the caller reads.

use std::io::{Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::process::ExitCode;

use rsb_core::contract::{ConfigFile, ErrorCode, Reply, Request, SettingsError, Verb};
use rsb_core::ipc::MAX_REPLY_BYTES;

use super::grammar::{Call, SettingsCmd};
use super::real::Real;
use super::{Ctx, preview, serve};
use crate::paths;

/// Most bytes of a change request read from a file or standard input.
pub const MAX_CHANGE_BYTES: usize = MAX_REPLY_BYTES;

/// The request in `text`, with what the options on the command line gave:
/// the options win.
pub fn merge_change(text: &str, flags: &Request) -> Result<Request, SettingsError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut request = Request::from_json(text)?;
    if request.verb.is_some_and(|v| v != Verb::Change) {
        return Err(SettingsError::new(ErrorCode::BadUsage, "a change file holds changes, not another verb")
            .with_next("usage: rsb settings change <file.json | ->"));
    }
    request.verb = Some(Verb::Change);
    request.contract = flags.contract.or(request.contract);
    for file in ConfigFile::ALL {
        if let Some(hash) = flags.expect.get(file) {
            request.expect.set(file, hash);
        }
    }
    request.dry_run |= flags.dry_run;
    Ok(request)
}

fn read_request(from: Option<&Path>) -> Result<String, SettingsError> {
    let too_big = || {
        SettingsError::new(ErrorCode::BadUsage, format!("the change request is larger than {MAX_CHANGE_BYTES} bytes"))
            .with_next("send fewer changes at once")
    };
    let mut bytes = Vec::new();
    let read = match from {
        Some(path) => {
            std::fs::File::open(path).and_then(|f| f.take(MAX_CHANGE_BYTES as u64 + 1).read_to_end(&mut bytes))
        }
        None => std::io::stdin().lock().take(MAX_CHANGE_BYTES as u64 + 1).read_to_end(&mut bytes),
    };
    read.map_err(|e| {
        SettingsError::new(ErrorCode::BadUsage, format!("cannot read the change request: {e}"))
            .with_next("usage: rsb settings change <file.json | ->")
    })?;
    if bytes.len() > MAX_CHANGE_BYTES {
        return Err(too_big());
    }
    String::from_utf8(bytes).map_err(|_| {
        SettingsError::new(ErrorCode::BadJson, "the change request is not UTF-8 text")
            .with_next("send one JSON object as in docs\\settings.md")
    })
}

/// Whether a path names a DOS device (`CON`, `NUL`, `COM1`, ... with any extension) or the
/// device namespace (`\\.\...`, `\\?\GLOBALROOT`). Opening one can block for good.
fn is_device_path(path: &Path) -> bool {
    let lower = path.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
    if lower.starts_with(r"\\.\") || lower.starts_with(r"\\?\globalroot") {
        return true;
    }
    let last = lower.rsplit('\\').next().unwrap_or_default();
    let stem = last.split('.').next().unwrap_or_default().trim_end_matches(' ');
    let numbered = |prefix: &str| stem.strip_prefix(prefix).is_some_and(|n| matches!(n.as_bytes(), [b'1'..=b'9']));
    matches!(stem, "con" | "prn" | "aux" | "nul" | "conin$" | "conout$") || numbered("com") || numbered("lpt")
}

fn device_refusal(what: &str) -> SettingsError {
    SettingsError::new(ErrorCode::BadUsage, format!("the {what} folder is a device name, not a folder"))
        .with_next("pass a normal folder path, or leave the option out")
}

/// The reply to the words, without printing it.
pub fn reply_for(cmd: &SettingsCmd, config_dir: Option<&Path>, log_dir: Option<&Path>) -> Reply {
    let (config, log) = (paths::config_dir(config_dir), paths::log_dir(log_dir));
    for (path, what) in [(&config, "config"), (&log, "log")] {
        if is_device_path(path) {
            return Reply::Err(device_refusal(what));
        }
    }
    let ctx = Ctx::cli(config, log, cmd.no_wait);
    match &cmd.call {
        Call::Refused(e) => Reply::Err(e.clone()),
        Call::Request(request) => serve(request.clone(), &ctx),
        Call::Change { from, flags } => match read_request(from.as_deref()).and_then(|t| merge_change(&t, flags)) {
            Ok(request) => serve(request, &ctx),
            Err(e) => Reply::Err(e),
        },
        Call::Preview(args) => catch_unwind(AssertUnwindSafe(|| preview(args, &ctx, &Real))).unwrap_or_else(|_| {
            Reply::Err(
                SettingsError::new(ErrorCode::Internal, "the preview could not be drawn")
                    .with_next("report it with rsb.log"),
            )
        }),
    }
}

/// Runs the command: one line of JSON on stdout, and for a refusal also
/// `rsb: <message>` on stderr. Exit 0, 1 or 2.
pub fn run(config_dir: Option<&Path>, log_dir: Option<&Path>, cmd: &SettingsCmd) -> ExitCode {
    let reply = reply_for(cmd, config_dir, log_dir);
    let _ = writeln!(std::io::stdout(), "{}", reply.to_line());
    if let Reply::Err(e) = &reply {
        eprintln!("rsb: {}", e.message);
    }
    ExitCode::from(reply.exit_code())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsb_core::contract::{FileHash, Op};

    #[test]
    fn options_win_over_the_file_and_the_verb_may_be_left_out() {
        let flags = Request {
            contract: Some(1),
            dry_run: true,
            expect: {
                let mut e = rsb_core::contract::Expect::default();
                e.set(ConfigFile::BarToml, FileHash::Fnv(7));
                e
            },
            ..Request::default()
        };
        let text = "\u{feff}{\"expect\":{\"bar.toml\":\"fnv1a64:0000000000000001\",\"style.css\":\"none\"},\"changes\":[{\"op\":\"unset\",\"key\":\"bar.main.height\"}]}";
        let r = merge_change(text, &flags).unwrap();
        assert_eq!(r.verb, Some(Verb::Change));
        assert!(r.dry_run);
        assert_eq!(r.contract, Some(1));
        assert_eq!(r.expect.get(ConfigFile::BarToml), Some(FileHash::Fnv(7)), "the option wins");
        assert_eq!(r.expect.get(ConfigFile::StyleCss), Some(FileHash::Missing), "the file's other entry stays");
        assert_eq!(r.changes, [Op::Unset { key: "bar.main.height".into() }]);
    }

    #[test]
    fn device_names_are_not_folders() {
        let bad =
            ["COM1", "com9.txt", r"C:\x\NUL", "lpt3", "CON", r"\\.\pipe\x", r"\\?\GLOBALROOT\Device\x", "aux.cfg"];
        for b in bad {
            assert!(is_device_path(Path::new(b)), "{b}");
        }
        for fine in ["COM", "COM10", "com0", r"C:\Users\x\rsb", r"\\?\C:\Users\x", "console", "a.nul", "LPT"] {
            assert!(!is_device_path(Path::new(fine)), "{fine}");
        }
        let get = Request { verb: Some(Verb::Get), ..Request::default() };
        let cmd = SettingsCmd { call: Call::Request(get), no_wait: true };
        let Reply::Err(e) = reply_for(&cmd, Some(Path::new("COM1")), None) else { panic!("served") };
        assert_eq!(e.code, ErrorCode::BadUsage);
    }

    #[test]
    fn a_change_file_must_be_one_json_object_of_changes() {
        let flags = Request::default();
        assert_eq!(merge_change("{nope", &flags).unwrap_err().code, ErrorCode::BadJson);
        assert_eq!(merge_change("[1,2]", &flags).unwrap_err().code, ErrorCode::BadJson);
        assert_eq!(merge_change("{\"verb\":\"get\"}", &flags).unwrap_err().code, ErrorCode::BadUsage);
        assert_eq!(merge_change("{\"verb\":\"change\",\"changes\":5}", &flags).unwrap_err().code, ErrorCode::BadJson);
    }
}

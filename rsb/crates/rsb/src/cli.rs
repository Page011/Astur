//! The commands that are not the bar itself: sending to a running bar,
//! `eject`, `types`. Each returns the process exit code (0 done, 1 failed,
//! 3 no bar running, 4 the bar could not be reached).

use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use rsb_core::ipc::{Command, Reply};

use crate::firstrun;
use crate::pipe::{self, SendError};

/// A command to the running bar; the reply goes to stdout, a refusal to stderr.
pub fn send(command: &Command) -> ExitCode {
    send_with(command, |c| pipe::send(c, Duration::from_secs(2)))
}

/// `rsb status`: the table for people, or with `json` the one line the bar sent.
pub fn status(json: bool) -> ExitCode {
    send_shaped(
        &Command::Status,
        |c| pipe::send(c, Duration::from_secs(2)),
        |text| if json { text.to_string() } else { crate::status::table(text) },
    )
}

/// The same with the transport replaced, so the exit codes are testable.
pub fn send_with(command: &Command, transport: impl FnOnce(&Command) -> Result<Reply, SendError>) -> ExitCode {
    send_shaped(command, transport, str::to_string)
}

fn send_shaped(
    command: &Command,
    transport: impl FnOnce(&Command) -> Result<Reply, SendError>,
    shape: impl FnOnce(&str) -> String,
) -> ExitCode {
    match transport(command) {
        Ok(Reply::Ok(text)) => {
            println!("{}", shape(&text));
            ExitCode::SUCCESS
        }
        Ok(Reply::Err(text)) => {
            eprintln!("rsb: {text}");
            ExitCode::from(1)
        }
        Err(SendError::NoBar) => {
            eprintln!("rsb: no bar is running (start it with: rsb)");
            ExitCode::from(3)
        }
        Err(SendError::Refused(why)) => {
            eprintln!("rsb: the bar could not be reached: {why}");
            ExitCode::from(4)
        }
    }
}

pub fn eject(config_dir: &Path, widget: &str) -> ExitCode {
    match firstrun::eject(config_dir, widget) {
        Ok(done) => {
            for f in &done.copied {
                println!("copied {}", f.replace('/', "\\"));
            }
            for f in &done.kept {
                println!("kept your {}", f.replace('/', "\\"));
            }
            println!("save it and the bar picks the change up; delete the file to go back to the built-in");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("rsb: {e}");
            ExitCode::from(1)
        }
    }
}

pub fn types(config_dir: &Path) -> ExitCode {
    if let Err(e) = std::fs::create_dir_all(config_dir) {
        eprintln!("rsb: cannot create the config folder: {e}");
        return ExitCode::from(1);
    }
    match firstrun::write_types(config_dir) {
        Ok(wrote) if wrote.is_empty() => {
            println!("types are up to date");
            ExitCode::SUCCESS
        }
        Ok(wrote) => {
            for f in wrote {
                println!("wrote {}", f.replace('/', "\\"));
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("rsb: cannot write the type files: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ExitCode has no Eq: compare through its debug text.
    fn code(c: ExitCode) -> String {
        format!("{c:?}")
    }

    #[test]
    fn replies_and_refusals_map_to_the_documented_exit_codes() {
        let cmd = Command::Reload;
        assert_eq!(code(send_with(&cmd, |_| Ok(Reply::Ok("done".into())))), code(ExitCode::SUCCESS));
        assert_eq!(code(send_with(&cmd, |_| Ok(Reply::Err("no".into())))), code(ExitCode::from(1)));
        assert_eq!(code(send_with(&cmd, |_| Err(SendError::NoBar))), code(ExitCode::from(3)));
        assert_eq!(code(send_with(&cmd, |_| Err(SendError::Refused("another user".into())))), code(ExitCode::from(4)));
    }
}

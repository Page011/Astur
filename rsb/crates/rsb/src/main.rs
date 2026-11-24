//! rsb.exe: a console-subsystem program (so `%ERRORLEVEL%` in cmd.exe is
//! rsb's own exit code) that hands everything to the library.

fn main() -> std::process::ExitCode {
    rsb_app::main_with(std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect())
}

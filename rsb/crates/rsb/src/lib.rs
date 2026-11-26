//! rsb.exe's wiring: the command line, the pipe, the single instance, logs,
//! first run, the config reader, the UI-thread app and the stop order.
//! The engine pieces (config, scene, renderer, windows, services, scripts)
//! live in their own crates; this one connects them.

use std::process::ExitCode;

pub mod app;
pub mod args;
pub mod assets;
pub mod capture;
pub mod cfgload;
pub mod cli;
pub mod clock;
pub mod console;
pub mod engine;
pub mod fallback;
pub mod firstrun;
pub mod fonts;
pub mod gate;
pub mod gesture;
pub mod headless;
pub mod icons;
pub mod images;
pub mod instance;
pub mod logger;
pub mod msg;
pub mod paths;
pub mod pipe;
pub mod plan;
pub mod pngread;
pub mod popups;
pub mod reader;
pub mod render_cmd;
pub mod run;
pub mod service_images;
pub mod sim;
pub mod stats;
pub mod status;
pub mod timeline;
pub mod timeline_run;
pub mod watchdog;
pub mod winlayer;

#[cfg(test)]
mod testutil;

/// The program: `args` are the words after the program name. Returns the
/// exit code a person sees in `%ERRORLEVEL%`.
pub fn main_with(args: Vec<String>) -> ExitCode {
    // The Start menu reader is this program started again by the apps service; it is not a command.
    if args.first().map(String::as_str) == Some(rsb_services::apps_reader::FLAG) {
        return ExitCode::from(rsb_services::apps_reader::main(&args[1..]) as u8);
    }
    let parsed = match args::parse(&args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("rsb: {e}\nrun rsb --help for the commands");
            return ExitCode::from(2);
        }
    };
    match &parsed.cmd {
        args::Cmd::Help => {
            println!("{}", args::USAGE);
            ExitCode::SUCCESS
        }
        args::Cmd::Version => {
            println!("rsb {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        args::Cmd::Send(command) => cli::send(command),
        args::Cmd::Status { json } => cli::status(*json),
        args::Cmd::Eject(widget) => cli::eject(&paths::config_dir(parsed.config_dir.as_deref()), widget),
        args::Cmd::Types => cli::types(&paths::config_dir(parsed.config_dir.as_deref())),
        args::Cmd::Render(opts) => render_cmd::run(parsed.config_dir.as_deref(), parsed.log_dir.as_deref(), opts),
        args::Cmd::Timeline(opts) => timeline_run::run(parsed.config_dir.as_deref(), parsed.log_dir.as_deref(), opts),
        args::Cmd::Run(opts) => run::run(&parsed, opts),
    }
}

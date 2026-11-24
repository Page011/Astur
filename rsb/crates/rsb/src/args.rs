//! The command line: public commands, their options, and the hidden test
//! flags a measurement run uses. Everything here is a pure function of the
//! argument list, so the whole grammar is unit-tested without a process.

use std::path::PathBuf;

use rsb_core::ipc;

pub const USAGE: &str = "\
rsb: a status bar for Windows

usage:
  rsb                          run the bar (or say it is already running)
  rsb reload                   re-read bar.toml, style.css and every widget file
  rsb quit                     stop the bar and give the screen space back
  rsb toggle [bar]             hide or show every bar, or one bar by name
  rsb status [--json]          widgets, services, bars and memory as a table (--json: one line
                               of JSON, for scripts)
  rsb trigger <event> [json]   send an event to rsb.events.on(<event>)
  rsb eject <widget>           copy a built-in widget into widgets\\ so you can change it
  rsb types                    write types\\rsb.d.luau for luau-lsp
  rsb render --out <file.png> [--width N] [--scale S] [--theme dark|light]
             [--preset NAME] [--live] [--wait-ms N]
                               draw the bar to a PNG without a window (sample data;
                               --live reads this PC; --preset draws that preset as shipped)
  rsb --help | --version

options:
  --config-dir <dir>           where bar.toml, style.css and widgets\\ live
                               (default: %APPDATA%\\rsb, or RSB_CONFIG_DIR)
  --log-dir <dir>              where rsb.log is written (default: %LOCALAPPDATA%\\rsb\\logs)

exit codes: 0 done, 1 the command failed, 2 bad usage, 3 no bar is running,
4 the bar could not be reached, 5 the bar could not start";

/// What the user asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    Run(Box<RunOpts>),
    /// reload, quit, toggle, trigger: sent to the running bar.
    Send(ipc::Command),
    /// `status`: asked of the running bar and shown as a table, or as the raw JSON line.
    Status {
        json: bool,
    },
    Eject(String),
    Types,
    Render(RenderOpts),
    Help,
    Version,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub config_dir: Option<PathBuf>,
    pub log_dir: Option<PathBuf>,
    pub cmd: Cmd,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderOpts {
    pub out: PathBuf,
    pub width: u32,
    pub scale: f32,
    pub dark: bool,
    pub preset: Option<String>,
    pub live: bool,
    pub wait_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockKind {
    Time,
    Resume,
}

/// The hidden flags (not in `--help`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunOpts {
    pub seconds: Option<f64>,
    pub no_reserve: bool,
    pub stats_out: Option<PathBuf>,
    pub stats_frames: bool,
    pub probe_ui: bool,
    pub fake_services: bool,
    pub fake_glazewm: bool,
    pub service_storm: Option<(f64, f64)>,
    pub simulate_taskbar_created: Vec<u64>,
    pub simulate_dpi: Vec<(u64, u32)>,
    pub simulate_display_change: Vec<u64>,
    pub simulate_theme: Vec<(u64, bool)>,
    pub simulate_fullscreen: Vec<(u64, bool)>,
    pub simulate_clock_change: Vec<(u64, ClockKind)>,
    pub test_fullscreen_window: Option<(u64, u64)>,
    pub simulate_pointer: Option<PathBuf>,
    pub test_script_stall: Option<(String, u64)>,
    pub test_offscreen: bool,
    pub test_render_failure: Option<u64>,
    pub test_panic: Option<u64>,
}

fn number<T: std::str::FromStr>(flag: &str, text: &str) -> Result<T, String> {
    text.parse().map_err(|_| format!("{flag}: '{text}' is not a valid number"))
}

/// `ms:value[,ms:value...]` with `value` parsed by `f`.
fn timed<T>(flag: &str, text: &str, f: impl Fn(&str) -> Option<T>) -> Result<Vec<(u64, T)>, String> {
    text.split(',')
        .map(|part| {
            let (ms, value) = part.split_once(':').ok_or_else(|| format!("{flag}: '{part}' is not <ms>:<value>"))?;
            let value = f(value).ok_or_else(|| format!("{flag}: '{value}' is not a valid value"))?;
            Ok((number::<u64>(flag, ms)?, value))
        })
        .collect()
}

fn parse_run_flag(opts: &mut RunOpts, flag: &str, next: &mut dyn FnMut() -> Option<String>) -> Result<bool, String> {
    let mut value = |flag: &str| next().ok_or_else(|| format!("{flag} needs a value"));
    match flag {
        "--seconds" => opts.seconds = Some(number(flag, &value(flag)?)?),
        "--no-reserve" => opts.no_reserve = true,
        "--stats-out" => opts.stats_out = Some(PathBuf::from(value(flag)?)),
        "--stats-frames" => opts.stats_frames = true,
        "--probe-ui" => opts.probe_ui = true,
        "--fake-services" => opts.fake_services = true,
        "--fake-glazewm" => opts.fake_glazewm = true,
        "--service-storm" => {
            let v = value(flag)?;
            let (hz, secs) = v.split_once(':').ok_or_else(|| format!("{flag}: use <hz>:<secs>"))?;
            opts.service_storm = Some((number(flag, hz)?, number(flag, secs)?));
        }
        "--simulate-taskbar-created" => opts.simulate_taskbar_created.push(number(flag, &value(flag)?)?),
        "--simulate-display-change" => opts.simulate_display_change.push(number(flag, &value(flag)?)?),
        "--simulate-dpi" => {
            opts.simulate_dpi.extend(timed(flag, &value(flag)?, |v| v.parse::<u32>().ok().filter(|d| *d >= 48))?)
        }
        "--simulate-theme" => opts.simulate_theme.extend(timed(flag, &value(flag)?, |v| match v {
            "dark" => Some(true),
            "light" => Some(false),
            _ => None,
        })?),
        "--simulate-fullscreen" => opts.simulate_fullscreen.extend(timed(flag, &value(flag)?, |v| match v {
            "on" => Some(true),
            "off" => Some(false),
            _ => None,
        })?),
        "--simulate-clock-change" => opts.simulate_clock_change.extend(timed(flag, &value(flag)?, |v| match v {
            "time" => Some(ClockKind::Time),
            "resume" => Some(ClockKind::Resume),
            _ => None,
        })?),
        "--test-fullscreen-window" => {
            let v = value(flag)?;
            let (at, dur) = v.split_once(':').ok_or_else(|| format!("{flag}: use <ms>:<duration ms>"))?;
            opts.test_fullscreen_window = Some((number(flag, at)?, number::<u64>(flag, dur)?.min(1000)));
        }
        "--simulate-pointer" => opts.simulate_pointer = Some(PathBuf::from(value(flag)?)),
        "--test-script-stall" => {
            let v = value(flag)?;
            let (widget, ms) = v.rsplit_once(':').ok_or_else(|| format!("{flag}: use <widget>:<ms>"))?;
            opts.test_script_stall = Some((widget.to_string(), number(flag, ms)?));
        }
        "--test-offscreen" => opts.test_offscreen = true,
        "--test-render-failure" => opts.test_render_failure = Some(number(flag, &value(flag)?)?),
        "--test-panic" => opts.test_panic = Some(number(flag, &value(flag)?)?),
        _ => return Ok(false),
    }
    Ok(true)
}

fn parse_render(rest: &[String]) -> Result<RenderOpts, String> {
    let mut out = None;
    let mut opts = RenderOpts {
        out: PathBuf::new(),
        width: 1920,
        scale: 1.0,
        dark: true,
        preset: None,
        live: false,
        wait_ms: None,
    };
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| it.next().cloned().ok_or_else(|| format!("{flag} needs a value"));
        match arg.as_str() {
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--width" => opts.width = number("--width", &value("--width")?)?,
            "--scale" => opts.scale = number("--scale", &value("--scale")?)?,
            "--theme" => {
                opts.dark = match value("--theme")?.as_str() {
                    "dark" => true,
                    "light" => false,
                    other => return Err(format!("--theme: '{other}' is not dark or light")),
                }
            }
            "--preset" => opts.preset = Some(value("--preset")?),
            "--wait-ms" => opts.wait_ms = Some(number("--wait-ms", &value("--wait-ms")?)?),
            "--live" => opts.live = true,
            other => return Err(format!("render: unknown option '{other}'")),
        }
    }
    opts.out = out.ok_or("render needs --out <file.png>")?;
    if !(64..=16384).contains(&opts.width) {
        return Err("--width must be 64 to 16384".into());
    }
    if !(0.5..=4.0).contains(&opts.scale) {
        return Err("--scale must be 0.5 to 4".into());
    }
    Ok(opts)
}

/// Parses the arguments after the program name. An `Err` is a usage error
/// (exit code 2): the text says what to fix.
pub fn parse(args: &[String]) -> Result<Parsed, String> {
    let mut config_dir = None;
    let mut log_dir = None;
    let mut help = false;
    let mut version = false;
    let mut rest: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config-dir" => config_dir = Some(PathBuf::from(it.next().ok_or("--config-dir needs a folder")?)),
            "--log-dir" => log_dir = Some(PathBuf::from(it.next().ok_or("--log-dir needs a folder")?)),
            "--help" | "-h" | "/?" => help = true,
            "--version" | "-V" => version = true,
            _ => rest.push(arg.clone()),
        }
    }
    let done = |cmd| Ok(Parsed { config_dir: config_dir.clone(), log_dir: log_dir.clone(), cmd });
    if help {
        return done(Cmd::Help);
    }
    if version {
        return done(Cmd::Version);
    }
    let command = rest.first().filter(|w| !w.starts_with("--")).cloned();
    match command.as_deref() {
        None => {
            let mut opts = RunOpts::default();
            let mut args = rest.into_iter();
            while let Some(flag) = args.next() {
                let mut next = || args.next();
                if !parse_run_flag(&mut opts, &flag, &mut next)? {
                    return Err(format!("unknown option '{flag}'"));
                }
            }
            if opts.service_storm.is_some() && !opts.fake_services {
                return Err("--service-storm needs --fake-services".into());
            }
            done(Cmd::Run(Box::new(opts)))
        }
        Some("status") => match &rest[1..] {
            [] => done(Cmd::Status { json: false }),
            [flag] if flag == "--json" => done(Cmd::Status { json: true }),
            _ => Err("usage: rsb status [--json]".into()),
        },
        Some("reload" | "quit" | "toggle" | "trigger") => {
            let cmd = ipc::Command::from_args(&rest).map_err(|e| e.to_string())?;
            done(Cmd::Send(cmd))
        }
        Some("eject") => match &rest[1..] {
            [name] => done(Cmd::Eject(name.clone())),
            _ => Err("usage: rsb eject <widget>".into()),
        },
        Some("types") if rest.len() == 1 => done(Cmd::Types),
        Some("types") => Err("usage: rsb types".into()),
        Some("render") => done(Cmd::Render(parse_render(&rest[1..])?)),
        Some(other) => {
            let hint = rsb_core::suggest::hint(
                other,
                ["reload", "quit", "toggle", "status", "trigger", "eject", "types", "render"],
            );
            Err(format!(
                "unknown command '{other}'{hint} (reload, quit, toggle, status, trigger, eject, types, render)"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(words: &[&str]) -> Result<Parsed, String> {
        parse(&words.iter().map(|w| w.to_string()).collect::<Vec<_>>())
    }

    fn run(words: &[&str]) -> RunOpts {
        match p(words).unwrap().cmd {
            Cmd::Run(o) => *o,
            other => panic!("not a run: {other:?}"),
        }
    }

    #[test]
    fn no_arguments_runs_the_bar() {
        assert_eq!(p(&[]).unwrap().cmd, Cmd::Run(Box::default()));
    }

    #[test]
    fn folders_and_help_and_version() {
        let parsed = p(&["--config-dir", "c", "--log-dir", "l", "status"]).unwrap();
        assert_eq!(parsed.config_dir, Some(PathBuf::from("c")));
        assert_eq!(parsed.log_dir, Some(PathBuf::from("l")));
        assert_eq!(parsed.cmd, Cmd::Status { json: false });
        assert_eq!(p(&["status", "--json"]).unwrap().cmd, Cmd::Status { json: true });
        assert!(p(&["status", "--yaml"]).unwrap_err().contains("rsb status [--json]"));
        assert!(p(&["status", "--json", "x"]).is_err());
        assert_eq!(p(&["--help"]).unwrap().cmd, Cmd::Help);
        assert_eq!(p(&["status", "--help"]).unwrap().cmd, Cmd::Help);
        assert_eq!(p(&["-V"]).unwrap().cmd, Cmd::Version);
        assert!(p(&["--config-dir"]).is_err());
    }

    #[test]
    fn pipe_commands_use_the_wire_grammar() {
        assert_eq!(p(&["reload"]).unwrap().cmd, Cmd::Send(ipc::Command::Reload));
        assert_eq!(p(&["quit"]).unwrap().cmd, Cmd::Send(ipc::Command::Quit));
        assert_eq!(p(&["toggle"]).unwrap().cmd, Cmd::Send(ipc::Command::Toggle(None)));
        assert_eq!(p(&["toggle", "top"]).unwrap().cmd, Cmd::Send(ipc::Command::Toggle(Some("top".into()))));
        assert!(p(&["toggle", "Bad Name"]).is_err());
        assert!(p(&["toggle", "a", "b"]).is_err());
        assert!(p(&["reload", "now"]).is_err());
        let t = p(&["trigger", "deploy", "{\"ok\":true}"]).unwrap().cmd;
        assert!(matches!(t, Cmd::Send(ipc::Command::Trigger { json: Some(_), .. })));
        assert!(p(&["trigger", "deploy", "{nope"]).is_err());
        assert!(p(&["trigger"]).is_err());
    }

    #[test]
    fn eject_types_and_unknown_commands() {
        assert_eq!(p(&["eject", "clock"]).unwrap().cmd, Cmd::Eject("clock".into()));
        assert!(p(&["eject"]).is_err());
        assert!(p(&["eject", "a", "b"]).is_err());
        assert_eq!(p(&["types"]).unwrap().cmd, Cmd::Types);
        assert!(p(&["types", "x"]).is_err());
        assert!(p(&["frobnicate"]).unwrap_err().contains("unknown command 'frobnicate'"));
        assert!(p(&["relaod"]).unwrap_err().contains("unknown command 'relaod'; did you mean 'reload'?"));
        assert!(p(&["--bogus"]).unwrap_err().contains("unknown option '--bogus'"));
    }

    #[test]
    fn render_options_and_limits() {
        let Cmd::Render(r) = p(&["render", "--out", "a.png"]).unwrap().cmd else { panic!() };
        assert_eq!(
            (r.width, r.scale, r.dark, r.live, r.preset.clone(), r.wait_ms),
            (1920, 1.0, true, false, None, None)
        );
        let Cmd::Render(r) = p(&[
            "render",
            "--out",
            "a.png",
            "--width",
            "800",
            "--scale",
            "1.5",
            "--theme",
            "light",
            "--preset",
            "glass",
            "--live",
            "--wait-ms",
            "900",
        ])
        .unwrap()
        .cmd
        else {
            panic!()
        };
        assert_eq!((r.width, r.scale, r.dark, r.live, r.wait_ms), (800, 1.5, false, true, Some(900)));
        assert_eq!(r.preset.as_deref(), Some("glass"));
        assert!(p(&["render"]).unwrap_err().contains("--out"));
        assert!(p(&["render", "--out", "a.png", "--width", "10"]).is_err());
        assert!(p(&["render", "--out", "a.png", "--scale", "9"]).is_err());
        assert!(p(&["render", "--out", "a.png", "--theme", "blue"]).is_err());
        assert!(p(&["render", "--out", "a.png", "--seconds", "3"]).is_err());
    }

    #[test]
    fn hidden_run_flags() {
        let o = run(&[
            "--seconds",
            "15",
            "--no-reserve",
            "--stats-out",
            "s.jsonl",
            "--stats-frames",
            "--probe-ui",
            "--fake-services",
            "--fake-glazewm",
            "--service-storm",
            "20:5",
            "--simulate-taskbar-created",
            "300",
            "--simulate-dpi",
            "100:120,200:96",
            "--simulate-display-change",
            "400",
            "--simulate-theme",
            "100:light,200:dark",
            "--simulate-fullscreen",
            "100:on,300:off",
            "--simulate-clock-change",
            "100:time,200:resume",
            "--test-fullscreen-window",
            "500:5000",
            "--simulate-pointer",
            "p.txt",
            "--test-script-stall",
            "my/x:4000",
            "--test-offscreen",
            "--test-panic",
            "700",
        ]);
        assert_eq!(o.seconds, Some(15.0));
        assert!(o.no_reserve && o.stats_frames && o.probe_ui && o.fake_services && o.fake_glazewm && o.test_offscreen);
        assert_eq!(o.service_storm, Some((20.0, 5.0)));
        assert_eq!(o.simulate_taskbar_created, [300]);
        assert_eq!(o.simulate_dpi, [(100, 120), (200, 96)]);
        assert_eq!(o.simulate_display_change, [400]);
        assert_eq!(o.simulate_theme, [(100, false), (200, true)]);
        assert_eq!(o.simulate_fullscreen, [(100, true), (300, false)]);
        assert_eq!(o.simulate_clock_change, [(100, ClockKind::Time), (200, ClockKind::Resume)]);
        assert_eq!(o.test_fullscreen_window, Some((500, 1000)), "the window never stays longer than a second");
        assert_eq!(o.simulate_pointer, Some(PathBuf::from("p.txt")));
        assert_eq!(o.test_script_stall, Some(("my/x".into(), 4000)));
        assert_eq!(o.test_panic, Some(700));
    }

    #[test]
    fn bad_hidden_flag_values_are_usage_errors() {
        assert!(p(&["--seconds"]).is_err());
        assert!(p(&["--seconds", "soon"]).is_err());
        assert!(p(&["--simulate-dpi", "100"]).is_err());
        assert!(p(&["--simulate-dpi", "100:5"]).is_err());
        assert!(p(&["--simulate-theme", "100:grey"]).is_err());
        assert!(p(&["--service-storm", "20:5"]).unwrap_err().contains("--fake-services"));
        assert!(p(&["--test-script-stall", "nocolon"]).is_err());
    }
}

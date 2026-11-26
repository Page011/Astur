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
  rsb popup <name> [open|close|toggle]
                               open, close or toggle the popup with that name on the monitor
                               under the pointer (toggle when left out)
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
    /// reload, quit, toggle, trigger, popup: sent to the running bar.
    Send(ipc::Command),
    /// `status`: asked of the running bar and shown as a table, or as the raw JSON line.
    Status {
        json: bool,
    },
    Eject(String),
    Types,
    Render(RenderOpts),
    /// `render --timeline`: a scripted run with no window, captured to pictures.
    Timeline(TimelineOpts),
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
    /// Draw as if the local time were this (the clock and date then look
    /// the same in every run). Not in `--help`.
    pub now: Option<rsb_core::time::LocalTime>,
}

/// `rsb render --timeline <file> --out-dir <dir>` (not in `--help`).
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineOpts {
    pub file: PathBuf,
    pub out_dir: PathBuf,
    /// The one screen the bars and popups sit on, physical px.
    pub screen: (u32, u32),
    pub scale: f32,
    pub dark: bool,
    pub preset: Option<String>,
    /// A PNG path or `#rrggbb`; None = a flat grey.
    pub background: Option<String>,
    /// The wall clock for the whole run.
    pub now: rsb_core::time::LocalTime,
    /// Stats lines (a `frame` line per presented frame, `popup` lines, one `timeline` line per capture).
    pub stats_out: Option<PathBuf>,
}

/// The wall time a timeline run uses when `--now` is not given.
pub const DEFAULT_NOW: &str = "2026-11-25T11:10:00";

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
    /// `--simulate-key <file>`: keys, text and IME events for popup windows.
    pub simulate_key: Option<PathBuf>,
    /// `--simulate-popup <ms>:<name>:open|close|toggle`: as `rsb popup`.
    pub simulate_popup: Vec<(u64, String, rsb_core::popup::PopupAction)>,
    /// `--simulate-outside-press <ms>`: a press outside every window.
    pub simulate_outside_press: Vec<u64>,
    /// `--simulate-deactivate <ms>:<popup>`: focus goes to another app.
    pub simulate_deactivate: Vec<(u64, String)>,
    /// `--simulate-session <ms>:lock|unlock`.
    pub simulate_session: Vec<(u64, bool)>,
    /// `--simulate-popup-dpi <ms>:<popup>:<dpi>`.
    pub simulate_popup_dpi: Vec<(u64, String, u32)>,
    /// `--test-ui-stall <ms>:<for ms>`: the UI thread sleeps, so the watchdog's overlay check can be seen.
    pub test_ui_stall: Option<(u64, u64)>,
    /// `--no-hook`: never install the outside-press hook.
    pub no_hook: bool,
    /// `--no-activate`: never give a popup keyboard focus.
    pub no_activate: bool,
    /// `--capture <dir>`: every presented frame written there (offscreen runs with fake services only).
    pub capture: Option<PathBuf>,
    /// `--simulate-monitors <WxH@dpi>[,...]`: monitors that do not exist, for offscreen runs
    /// (width, height, dpi).
    pub simulate_monitors: Vec<(u32, u32, u32)>,
    /// Some flag drives the bar for a test (see `gates_commands`): service
    /// commands are recorded and the power actions are stubbed.
    pub gated: bool,
}

/// Flags that exist for tests and measurements that click, type or change
/// the environment: every one of them keeps commands away from the real
/// services. `--seconds`, `--no-reserve`, `--stats-*` and `--probe-ui` only
/// measure a normal run.
pub fn gates_commands(flag: &str) -> bool {
    flag.starts_with("--simulate-")
        || flag.starts_with("--test-")
        || matches!(
            flag,
            "--fake-services" | "--fake-glazewm" | "--service-storm" | "--no-hook" | "--no-activate" | "--capture"
        )
}

/// `1920x1080@144` (the `@dpi` part is optional and defaults to 96).
fn monitor_spec(flag: &str, text: &str) -> Result<(u32, u32, u32), String> {
    let bad = || format!("{flag}: '{text}' is not <width>x<height>[@dpi]");
    let (size, dpi) = text.split_once('@').map_or((text, None), |(s, d)| (s, Some(d)));
    let (w, h) = size.split_once('x').ok_or_else(bad)?;
    let (w, h): (u32, u32) = (w.parse().map_err(|_| bad())?, h.parse().map_err(|_| bad())?);
    let dpi: u32 = dpi.map_or(Ok(96), |d| d.parse().map_err(|_| bad()))?;
    if !(320..=16384).contains(&w) || !(200..=16384).contains(&h) || !(48..=960).contains(&dpi) {
        return Err(format!("{flag}: '{text}' is outside 320x200 to 16384x16384 at 48 to 960 dpi"));
    }
    Ok((w, h, dpi))
}

/// `YYYY-MM-DDTHH:MM[:SS]`, read as local time.
fn local_time(flag: &str, text: &str) -> Result<rsb_core::time::LocalTime, String> {
    let bad = || format!("{flag}: '{text}' is not YYYY-MM-DDTHH:MM[:SS]");
    let (date, time) = text.split_once('T').ok_or_else(bad)?;
    let d: Vec<&str> = date.split('-').collect();
    let t: Vec<&str> = time.split(':').collect();
    if d.len() != 3 || !(2..=3).contains(&t.len()) {
        return Err(bad());
    }
    let num = |s: &str, lo: u32, hi: u32| s.parse::<u32>().ok().filter(|n| (lo..=hi).contains(n)).ok_or_else(bad);
    let t = rsb_core::time::LocalTime {
        year: num(d[0], 1970, 9999)? as i32,
        month: num(d[1], 1, 12)? as u8,
        day: num(d[2], 1, 31)? as u8,
        hour: num(t[0], 0, 23)? as u8,
        minute: num(t[1], 0, 59)? as u8,
        second: t.get(2).map_or(Ok(0), |s| num(s, 0, 59))? as u8,
        ..Default::default()
    };
    // Normalises the weekday (and rejects 31 February by changing it).
    let n = rsb_core::time::LocalTime::from_epoch_ms(t.to_epoch_ms(), 0);
    if (n.year, n.month, n.day) != (t.year, t.month, t.day) {
        return Err(bad());
    }
    Ok(n)
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
        "--simulate-key" => opts.simulate_key = Some(PathBuf::from(value(flag)?)),
        "--simulate-popup" => {
            for part in value(flag)?.split(',') {
                let (ms, rest) =
                    part.split_once(':').ok_or_else(|| format!("{flag}: '{part}' is not <ms>:<name>:<action>"))?;
                let (name, action) =
                    rest.rsplit_once(':').ok_or_else(|| format!("{flag}: '{part}' is not <ms>:<name>:<action>"))?;
                let action = rsb_core::popup::PopupAction::from_name(action)
                    .ok_or_else(|| format!("{flag}: '{action}' is not open, close or toggle"))?;
                opts.simulate_popup.push((number(flag, ms)?, name.to_string(), action));
            }
        }
        "--simulate-outside-press" => {
            for part in value(flag)?.split(',') {
                opts.simulate_outside_press.push(number(flag, part)?);
            }
        }
        "--simulate-deactivate" => {
            opts.simulate_deactivate.extend(timed(flag, &value(flag)?, |v| (!v.is_empty()).then(|| v.to_string()))?)
        }
        "--simulate-session" => opts.simulate_session.extend(timed(flag, &value(flag)?, |v| match v {
            "lock" => Some(true),
            "unlock" => Some(false),
            _ => None,
        })?),
        "--simulate-popup-dpi" => {
            for part in value(flag)?.split(',') {
                let (ms, rest) =
                    part.split_once(':').ok_or_else(|| format!("{flag}: '{part}' is not <ms>:<popup>:<dpi>"))?;
                let (name, dpi) =
                    rest.rsplit_once(':').ok_or_else(|| format!("{flag}: '{part}' is not <ms>:<popup>:<dpi>"))?;
                let dpi: u32 = number(flag, dpi)?;
                if dpi < 48 {
                    return Err(format!("{flag}: '{dpi}' is not a valid dpi"));
                }
                opts.simulate_popup_dpi.push((number(flag, ms)?, name.to_string(), dpi));
            }
        }
        "--test-ui-stall" => {
            let v = value(flag)?;
            let (at, dur) = v.split_once(':').ok_or_else(|| format!("{flag}: use <ms>:<duration ms>"))?;
            opts.test_ui_stall = Some((number(flag, at)?, number::<u64>(flag, dur)?.min(10_000)));
        }
        "--no-hook" => opts.no_hook = true,
        "--no-activate" => opts.no_activate = true,
        "--capture" => opts.capture = Some(PathBuf::from(value(flag)?)),
        "--simulate-monitors" => {
            for part in value(flag)?.split(',') {
                opts.simulate_monitors.push(monitor_spec(flag, part)?);
            }
            if opts.simulate_monitors.len() > 4 {
                return Err(format!("{flag}: at most 4 monitors"));
            }
        }
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
        now: None,
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
            "--now" => opts.now = Some(local_time("--now", &value("--now")?)?),
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

fn parse_timeline(rest: &[String]) -> Result<TimelineOpts, String> {
    let (mut file, mut out_dir) = (None, None);
    let mut opts = TimelineOpts {
        file: PathBuf::new(),
        out_dir: PathBuf::new(),
        screen: (1920, 1200),
        scale: 1.0,
        dark: true,
        preset: None,
        background: None,
        now: local_time("--now", DEFAULT_NOW)?,
        stats_out: None,
    };
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| it.next().cloned().ok_or_else(|| format!("{flag} needs a value"));
        match arg.as_str() {
            "--timeline" => file = Some(PathBuf::from(value("--timeline")?)),
            "--out-dir" => out_dir = Some(PathBuf::from(value("--out-dir")?)),
            "--screen" => {
                let text = value("--screen")?;
                let (w, h, _) = monitor_spec("--screen", &text)?;
                opts.screen = (w, h);
            }
            "--scale" => opts.scale = number("--scale", &value("--scale")?)?,
            "--theme" => {
                opts.dark = match value("--theme")?.as_str() {
                    "dark" => true,
                    "light" => false,
                    other => return Err(format!("--theme: '{other}' is not dark or light")),
                }
            }
            "--preset" => opts.preset = Some(value("--preset")?),
            "--background" => opts.background = Some(value("--background")?),
            "--now" => opts.now = local_time("--now", &value("--now")?)?,
            "--stats-out" => opts.stats_out = Some(PathBuf::from(value("--stats-out")?)),
            "--live" => return Err("--live cannot be used with --timeline: a timeline reads fake services only".into()),
            "--out" | "--width" | "--wait-ms" => {
                return Err(format!("{arg} belongs to a single picture; a timeline uses --out-dir and --screen"));
            }
            other => return Err(format!("render --timeline: unknown option '{other}'")),
        }
    }
    opts.file = file.ok_or("--timeline needs a file")?;
    opts.out_dir = out_dir.ok_or("render --timeline needs --out-dir <folder>")?;
    if !crate::capture::dir_allowed(&opts.out_dir) {
        return Err("--out-dir writes only under a scratch or target folder".into());
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
                opts.gated |= gates_commands(&flag);
            }
            if opts.service_storm.is_some() && !opts.fake_services {
                return Err("--service-storm needs --fake-services".into());
            }
            if let Some(dir) = &opts.capture {
                if !opts.fake_services {
                    return Err("--capture needs --fake-services".into());
                }
                if !opts.test_offscreen {
                    return Err("--capture needs --test-offscreen".into());
                }
                if !crate::capture::dir_allowed(dir) {
                    return Err("--capture writes only under a scratch or target folder".into());
                }
            }
            if !opts.simulate_monitors.is_empty() && !opts.test_offscreen {
                return Err("--simulate-monitors needs --test-offscreen".into());
            }
            done(Cmd::Run(Box::new(opts)))
        }
        Some("status") => match &rest[1..] {
            [] => done(Cmd::Status { json: false }),
            [flag] if flag == "--json" => done(Cmd::Status { json: true }),
            _ => Err("usage: rsb status [--json]".into()),
        },
        Some("reload" | "quit" | "toggle" | "trigger" | "popup") => {
            let cmd = ipc::Command::from_args(&rest).map_err(|e| e.to_string())?;
            done(Cmd::Send(cmd))
        }
        Some("eject") => match &rest[1..] {
            [name] => done(Cmd::Eject(name.clone())),
            _ => Err("usage: rsb eject <widget>".into()),
        },
        Some("types") if rest.len() == 1 => done(Cmd::Types),
        Some("types") => Err("usage: rsb types".into()),
        Some("render") if rest.iter().any(|a| a == "--timeline") => done(Cmd::Timeline(parse_timeline(&rest[1..])?)),
        Some("render") => done(Cmd::Render(parse_render(&rest[1..])?)),
        Some(other) => {
            let hint = rsb_core::suggest::hint(
                other,
                ["reload", "quit", "toggle", "status", "trigger", "popup", "eject", "types", "render"],
            );
            Err(format!(
                "unknown command '{other}'{hint} (reload, quit, toggle, status, trigger, popup, eject, types, render)"
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
    fn test_flags_mark_the_run_as_gated_and_measurement_flags_do_not() {
        assert!(!run(&[]).gated);
        for words in [&["--seconds", "2", "--no-reserve", "--stats-frames", "--probe-ui"][..], &["--stats-out", "s"]] {
            assert!(!run(words).gated, "{words:?}");
        }
        for words in [
            &["--fake-services"][..],
            &["--test-offscreen"],
            &["--simulate-dpi", "1:96"],
            &["--simulate-pointer", "p.txt"],
            &["--no-hook"],
            &["--no-activate"],
            &["--test-panic", "5"],
        ] {
            assert!(run(words).gated, "{words:?}");
        }
        let o = run(&["--no-hook", "--no-activate"]);
        assert!(o.no_hook && o.no_activate);
    }

    #[test]
    fn a_capture_needs_the_fakes_and_a_scratch_or_target_folder() {
        let ok = run(&["--capture", "C:\\work\\target\\frames", "--fake-services", "--test-offscreen"]);
        assert_eq!(ok.capture, Some(PathBuf::from("C:\\work\\target\\frames")));
        assert_eq!(p(&["--capture", "C:\\work\\target\\frames"]).unwrap_err(), "--capture needs --fake-services");
        assert_eq!(
            p(&["--capture", "C:\\work\\target\\frames", "--fake-services"]).unwrap_err(),
            "--capture needs --test-offscreen"
        );
        assert!(
            p(&["--capture", "D:\\photos", "--fake-services", "--test-offscreen"])
                .unwrap_err()
                .contains("scratch or target")
        );
        assert!(p(&["--capture"]).unwrap_err().contains("needs a value"));
    }

    #[test]
    fn the_popup_test_flags_parse_and_a_bad_value_is_refused() {
        use rsb_core::popup::PopupAction;
        let o = run(&[
            "--simulate-popup",
            "100:launcher:open,200:power:toggle",
            "--simulate-outside-press",
            "300,400",
            "--simulate-deactivate",
            "500:launcher",
            "--simulate-session",
            "600:lock,700:unlock",
            "--simulate-popup-dpi",
            "800:launcher:144",
            "--simulate-key",
            "keys.txt",
            "--test-ui-stall",
            "900:99999",
        ]);
        assert_eq!(
            o.simulate_popup,
            [(100, "launcher".to_string(), PopupAction::Open), (200, "power".to_string(), PopupAction::Toggle)]
        );
        assert_eq!(o.simulate_outside_press, [300, 400]);
        assert_eq!(o.simulate_deactivate, [(500, "launcher".to_string())]);
        assert_eq!(o.simulate_session, [(600, true), (700, false)]);
        assert_eq!(o.simulate_popup_dpi, [(800, "launcher".to_string(), 144)]);
        assert_eq!(o.simulate_key, Some(PathBuf::from("keys.txt")));
        assert_eq!(o.test_ui_stall, Some((900, 10_000)), "a stall is capped at 10 s");
        assert!(o.gated);
        for (words, needle) in [
            (&["--simulate-popup", "100:x:sideways"][..], "open, close or toggle"),
            (&["--simulate-popup", "100"], "<ms>:<name>:<action>"),
            (&["--simulate-session", "100:sleep"], "not a valid value"),
            (&["--simulate-popup-dpi", "100:x:10"], "not a valid dpi"),
            (&["--simulate-outside-press", "soon"], "not a valid number"),
        ] {
            let e = p(words).unwrap_err();
            assert!(e.contains(needle), "{words:?}: {e}");
        }
    }

    #[test]
    fn rsb_popup_is_a_command_for_the_running_bar_and_a_bad_use_is_a_usage_error() {
        use rsb_core::popup::PopupAction;
        let send = |words: &[&str]| match p(words).unwrap().cmd {
            Cmd::Send(c) => c,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            send(&["popup", "launcher"]),
            ipc::Command::Popup { name: "launcher".into(), action: PopupAction::Toggle }
        );
        assert_eq!(
            send(&["popup", "launcher", "close"]),
            ipc::Command::Popup { name: "launcher".into(), action: PopupAction::Close }
        );
        for words in [&["popup"][..], &["popup", "a", "b"], &["popup", "a", "open", "x"], &["popup", "Bad Name"]] {
            let e = p(words).unwrap_err();
            assert!(!e.is_empty(), "{words:?} is refused (exit 2)");
        }
        assert!(p(&["popup"]).unwrap_err().contains("usage: rsb popup <name> [open|close|toggle]"));
        assert!(USAGE.contains("rsb popup <name> [open|close|toggle]"));
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
        let Cmd::Render(r) = p(&["render", "--out", "a.png", "--now", "2026-11-25T11:10"]).unwrap().cmd else {
            panic!()
        };
        let now = r.now.unwrap();
        assert_eq!((now.year, now.month, now.day, now.weekday, now.hour, now.minute), (2026, 11, 25, 3, 11, 10));
        assert!(p(&["render", "--out", "a.png", "--now", "2026-02-31T11:10"]).is_err());
        assert!(p(&["render", "--out", "a.png", "--now", "11:10"]).is_err());
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

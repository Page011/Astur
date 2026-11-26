//! Running the bar: the start-up order, the wiring of every thread, and the
//! stop order. See `docs/architecture.md` for the picture.

use std::process::ExitCode;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use rsb_core::config::{default_config, load_config};
use rsb_core::geom::RectPx;
use rsb_core::ipc::{Command, Reply};
use rsb_core::msg::{ScriptOut, ScriptSink};
use rsb_core::services::{ServiceCommand, ServiceId};
use rsb_core::wake::Wake;
use rsb_render::{FontConfig, Renderer};
use rsb_services::fake::FakeWorld;
use rsb_services::fake::glazewm::{FakeGlazeServer, FakeWorkspace};
use rsb_win::{MonitorInfo, Ui, UiOptions, UiWaker};
use serde_json::json;

use crate::app::{App, AppOptions};
use crate::args::{Parsed, RunOpts};
use crate::capture::Capture;
use crate::fonts::{self, FontCache};
use crate::gate::{self, Gate};
use crate::images::ImageLoader;
use crate::instance::{self, Instance};
use crate::msg::{ConfigUpdate, ReloadKind, UiMsg};
use crate::pipe::{Handler, PipeServer};
use crate::popups::OverlayWatch;
use crate::reader::Reader;
use crate::stats::{self, Stats, wall_ms};
use crate::status::{self, Sources};
use crate::watchdog::{OverlayGuard, Watchdog};
use crate::{assets, cfgload, console, engine, firstrun, logger, paths, sim};

/// Exit code: the bar could not start (instance mutex, window creation,
/// config folder not writable on first run).
pub const EXIT_CANNOT_START: u8 = 5;

struct UiSink {
    tx: Sender<UiMsg>,
    waker: UiWaker,
}

impl ScriptSink for UiSink {
    fn send(&self, out: ScriptOut) {
        if self.tx.send(UiMsg::Script(out)).is_ok() {
            self.waker.wake();
        }
    }
}

/// `Connect("...")` -> `Connect`.
fn variant(debug: &str) -> String {
    debug.split(['(', ' ', '{']).next().unwrap_or_default().to_string()
}

/// Writes the commands a `FakeWorld` has received since the last call.
fn report_commands(stats: &Stats, world: &FakeWorld, seen: &mut usize) {
    let all = world.commands();
    for c in &all[(*seen).min(all.len())..] {
        let (service, command) = match c {
            ServiceCommand::Audio(a) => ("audio", format!("{a:?}")),
            ServiceCommand::Media(m) => ("media", format!("{m:?}")),
            ServiceCommand::Workspaces(w) => ("workspaces", format!("{w:?}")),
            // Device ids and network names stay out of the stats: the command's name only.
            ServiceCommand::AudioDevice(d) => ("audio", variant(&format!("{d:?}"))),
            ServiceCommand::Microphone(m) => ("microphone", format!("{m:?}")),
            ServiceCommand::Bluetooth(b) => ("bluetooth", variant(&format!("{b:?}"))),
            ServiceCommand::Apps(a) => ("apps", variant(&format!("{a:?}"))),
            ServiceCommand::Power(p) => ("power", format!("{p:?}")),
            ServiceCommand::Wifi(w) => ("wifi", variant(&format!("{w:?}"))),
        };
        stats.emit("service_command", json!({"service": service, "command": command}));
    }
    *seen = all.len();
}

/// With stats on: `mem` every 10 s, and the fakes' commands as they appear.
struct Ticker {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<std::thread::JoinHandle<usize>>,
}

const TICK: Duration = Duration::from_secs(10);

impl Ticker {
    fn start(stats: Arc<Stats>, world: Option<FakeWorld>, period: Duration) -> Ticker {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("rsb-stats".into())
            .spawn(move || {
                let mut seen = 0;
                loop {
                    let (lock, cond) = &*thread_stop;
                    let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                    let (guard, _) = cond.wait_timeout_while(guard, period, |s| !*s).unwrap_or_else(|e| e.into_inner());
                    if *guard {
                        break;
                    }
                    drop(guard);
                    let (private_mb, working_set_mb) = stats::memory_mb();
                    stats.emit("mem", json!({"private_mb": private_mb, "working_set_mb": working_set_mb}));
                    if let Some(w) = &world {
                        report_commands(&stats, w, &mut seen);
                    }
                }
                seen
            })
            .expect("the stats thread starts");
        Ticker { stop, thread: Some(thread) }
    }

    /// Stops the thread; returns how many of the fakes' commands it has already reported.
    fn stop(mut self) -> usize {
        let (lock, cond) = &*self.stop;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cond.notify_all();
        self.thread.take().and_then(|t| t.join().ok()).unwrap_or(0)
    }
}

/// The five workspaces the fake window manager reports, the second focused.
fn start_fake_glaze() -> Option<FakeGlazeServer> {
    let server = FakeGlazeServer::start().ok()?;
    let workspaces: Vec<FakeWorkspace> = (1..=5)
        .map(|i| FakeWorkspace {
            name: i.to_string(),
            display_name: i.to_string(),
            windows: if i == 2 { 1 } else { 0 },
        })
        .collect();
    server.set_state(vec![workspaces], (0, 1));
    Some(server)
}

pub fn run(parsed: &Parsed, opts: &RunOpts) -> ExitCode {
    let capture = match opts.capture.as_deref().map(Capture::create) {
        Some(Ok(c)) => Some(c),
        Some(Err(e)) => {
            eprintln!("rsb: {e}");
            return ExitCode::from(2);
        }
        None => None,
    };
    let start = Instant::now();
    console::detach_if_own_console();
    let _guard = match instance::acquire() {
        Ok(Instance::First(g)) => g,
        Ok(Instance::AlreadyRunning) => {
            println!("rsb is already running");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("rsb: cannot start: {e}");
            return ExitCode::from(EXIT_CANNOT_START);
        }
    };
    let log_dir = paths::log_dir(parsed.log_dir.as_deref());
    let log_file = match logger::init(&log_dir) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("rsb: {e} (running without a log)");
            logger::file_path(&log_dir, 0)
        }
    };
    log::info!("rsb {} starting (session {})", env!("CARGO_PKG_VERSION"), stats::session_name());
    let config_dir = paths::config_dir(parsed.config_dir.as_deref());

    let stats = match Stats::open(opts.stats_out.as_deref(), opts.stats_frames, start) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("rsb: {e}");
            return ExitCode::from(2);
        }
    };
    stats.emit("start", json!({"version": env!("CARGO_PKG_VERSION")}));
    let pointer_script = match &opts.simulate_pointer {
        Some(path) => {
            match std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|t| sim::parse_pointer_script(&t)) {
                Ok(list) => list,
                Err(e) => {
                    eprintln!("rsb: {e}");
                    return ExitCode::from(2);
                }
            }
        }
        None => Vec::new(),
    };
    let key_script = match &opts.simulate_key {
        Some(path) => {
            match std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|t| sim::parse_key_script(&t)) {
                Ok(list) => list,
                Err(e) => {
                    eprintln!("rsb: {e}");
                    return ExitCode::from(2);
                }
            }
        }
        None => Vec::new(),
    };

    match firstrun::prepare(&config_dir) {
        Ok(p) if p.first_run => log::info!("first run: the config files were written"),
        Ok(_) => {}
        Err(e) => {
            log::error!("cannot prepare the config folder: {e}");
            eprintln!("rsb: cannot prepare the config folder: {e}");
            return ExitCode::from(EXIT_CANNOT_START);
        }
    }

    // Read and parse before the loop starts: nothing here is UI-thread I/O.
    let mut font_cache = FontCache::default();
    let mut built = cfgload::build_cached(&config_dir, None, "default", &mut font_cache);
    if built.config.is_none() {
        // A broken bar.toml on the very first start still gets a bar; the plate says why.
        built.config = load_config(None, &|n| assets::preset_toml(n).map(str::to_string))
            .config
            .or_else(|| Some(default_config()));
    }
    let preset = built.config.as_ref().map_or_else(|| "default".to_string(), |c| c.preset.clone());
    let first = ConfigUpdate { kind: ReloadKind::Initial, built, notified_wall_ms: wall_ms() };

    let extra_monitors = opts
        .simulate_monitors
        .iter()
        .map(|&(w, h, dpi)| {
            let rect = RectPx::new(0, 0, w as i32, h as i32);
            MonitorInfo { index: 0, handle: 0, device: String::new(), primary: false, rect, work: rect, dpi }
        })
        .collect();
    let mut ui = match Ui::new(UiOptions { probe: opts.probe_ui, extra_monitors }) {
        Ok(ui) => ui,
        Err(e) => {
            log::error!("cannot create the UI: {e}");
            eprintln!("rsb: cannot start: {e}");
            return ExitCode::from(EXIT_CANNOT_START);
        }
    };
    let dark = ui.system_dark();
    let waker = ui.waker();
    let wake: Arc<dyn Wake> = Arc::new(waker.clone());
    let (ui_tx, ui_rx) = channel::<UiMsg>();

    let world = opts.fake_services.then(FakeWorld::with_sample_values);
    if let (Some(w), Some(_)) = (&world, opts.service_storm) {
        // The storm ends in silence: with a short limit the hub marks the source stale seconds later, so the
        // stale path (log line, placeholder text, class) can be seen in a short run.
        w.set_stale_after(ServiceId::System, Duration::from_secs(3));
    }
    let glaze = if opts.fake_glazewm { start_fake_glaze() } else { None };
    // A run driven by a test flag, or under a test tag, never reaches the real PC's services.
    let selection = gate::select_run(opts, gate::tag_set());
    stats.emit("gate", json!({"recorder": selection.is_some(), "forward": selection.is_some_and(|s| s.forward)}));
    let engine = engine::start(
        engine::EngineOptions {
            clock: None,
            config_dir: config_dir.clone(),
            dark,
            world: world.clone(),
            glaze_url: glaze.as_ref().map(FakeGlazeServer::url),
            test_stall: opts.test_script_stall.as_ref().map(|(w, _)| w.clone()),
            gate: selection.map(|s| Gate::new(s, Arc::clone(&stats))),
            images: Some(engine::ImageRoute { tx: ui_tx.clone(), wake: Arc::clone(&wake) }),
            observe: None,
        },
        Box::new(UiSink { tx: ui_tx.clone(), waker: waker.clone() }),
    );

    let overlays = Arc::new(OverlayWatch::default());
    let mut renderer = Renderer::new(FontConfig::default());
    if let Err(e) = fonts::register_bundled(&mut renderer) {
        log::warn!("the bundled font could not be registered: {e}");
    }
    let mut app = App::new(
        AppOptions {
            config_dir: config_dir.clone(),
            log_file,
            no_reserve: opts.no_reserve,
            offscreen: opts.test_offscreen,
            simulated_pointer: opts.simulate_pointer.is_some(),
            no_hook: opts.no_hook,
            no_activate: opts.no_activate,
            gated: selection.is_some(),
            overlays: Arc::clone(&overlays),
        },
        Arc::clone(&stats),
        ui_rx,
        engine.script_tx.clone(),
        ImageLoader::start(ui_tx.clone(), Arc::clone(&wake)),
        renderer,
        dark,
    );
    if let Some(c) = capture {
        app.set_capture(c);
    }
    app.set_reader(Reader::start(
        config_dir,
        preset,
        ui_tx.clone(),
        engine.script_tx.clone(),
        Arc::clone(&wake),
        font_cache,
    ));

    // The command pipe: everything but `status` is answered by the UI thread.
    let sources = Arc::new(Sources {
        script_tx: engine.script_tx.clone(),
        hub: engine.hub_port.clone(),
        ui_tx: ui_tx.clone(),
        waker: Box::new(waker.clone()),
        started: start,
    });
    let handler: Handler = {
        let (tx, waker) = (ui_tx.clone(), waker.clone());
        Arc::new(move |command: Command| match command {
            Command::Status => Reply::Ok(status::gather(&sources)),
            other => {
                let (reply_tx, reply_rx) = channel();
                if tx.send(UiMsg::Pipe { command: other, reply: reply_tx }).is_err() {
                    return Reply::Err("the bar is stopping".into());
                }
                waker.wake();
                reply_rx
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap_or_else(|_| Reply::Err("the bar did not answer in time".into()))
            }
        })
    };
    let pipe = match PipeServer::start(handler) {
        Ok(p) => Some(p),
        Err(e) => {
            log::error!("the command pipe could not start: {e}");
            app.set_pipe_failure("rsb commands will not reach this bar; see log");
            None
        }
    };
    let stall = ui.stall_guard();
    let hide = stall.clone();
    let overlay_guard = OverlayGuard {
        watch: Arc::clone(&overlays),
        busy_for: Box::new(move || stall.busy_for(Instant::now())),
        hide: Box::new(move || hide.hide_overlays()),
        stats: Arc::clone(&stats),
    };
    let watchdog =
        Watchdog::start(Arc::clone(&engine.heartbeat), ui_tx.clone(), Arc::clone(&wake), Some(overlay_guard));
    {
        let (tx, waker) = (ui_tx.clone(), waker.clone());
        console::install_ctrl_handler(move || {
            let _ = tx.send(UiMsg::Quit);
            waker.wake();
        });
    }

    app.start(&mut ui, first);

    let plan = sim::schedule(opts, pointer_script, key_script);
    let player = (!plan.is_empty()).then(|| sim::Player::start(plan, ui_tx.clone(), Arc::clone(&wake)));
    let storm = match (opts.service_storm, &world) {
        (Some((hz, secs)), Some(w)) => Some(sim::service_storm(w.clone(), hz, secs)),
        _ => None,
    };
    let ticker = stats.enabled().then(|| Ticker::start(Arc::clone(&stats), world.clone(), TICK));

    let result = ui.run(&mut app);
    let code = match &result {
        Ok(()) => 0,
        Err(e) => {
            log::error!("the message loop failed: {e}");
            1
        }
    };

    // Stop order. 1: the loop has returned, so every bar is gone and the
    // work area is back.
    app.finish_capture();
    app.emit_workarea(&ui, "unregister");
    app.emit_workarea(&ui, "exit");
    if let Some(p) = ui.probe_stats() {
        stats.emit(
            "probe",
            json!({"p50_ms": p.p50_ms, "p95_ms": p.p95_ms, "worst_ms": p.worst_ms,
                   "worst_after_first_frame_ms": p.worst_after_first_frame_ms, "samples": p.samples}),
        );
    }
    stats.emit(
        "wakes",
        json!({"ui": ui.loop_wakes(), "script": rsb_script::host_wakes(), "hub": engine.hub.wakes(),
               "watchdog": watchdog.wakes(), "sources": rsb_services::source_wakes()}),
    );
    let (private_mb, working_set_mb) = stats::memory_mb();
    stats.emit("mem", json!({"private_mb": private_mb, "working_set_mb": working_set_mb}));
    stats.emit_pacing(ui.compositor_period());
    stats.emit("cpu", json!({"ms": stats::cpu_ms(), "resolution_ms": stats::CPU_TIME_RESOLUTION_MS}));
    if let Some(h) = ui.hook_stats() {
        stats.emit(
            "hook",
            json!({"on": false, "wanted": false, "events": h.events, "max_us": h.max_us,
                   "p99_delivery_us": h.p99_delivery_us, "reinstalls": h.reinstalls, "final": true}),
        );
    }
    // Commands the ticker already reported are not reported again.
    let mut reported = ticker.map_or(0, Ticker::stop);
    if let Some(w) = &world {
        report_commands(&stats, w, &mut reported);
    }
    // 2: the pipe thread.
    if let Some(p) = pipe {
        p.stop();
    }
    if let Some(p) = player {
        p.stop();
    }
    if let Some(s) = storm {
        s.stop();
    }
    // 3 and 4: the script thread, then the hub.
    engine.stop();
    // 5: watcher and config reader, then the watchdog.
    if let Some(r) = app.take_reader() {
        r.stop();
    }
    watchdog.stop();
    drop(glaze);
    // 6: the log and the exit line.
    stats.emit("exit", json!({"code": code}));
    log::info!("rsb stopped (code {code})");
    console::mark_stopped();
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use rsb_core::services::{AudioCommand, ServiceCommand};
    use rsb_services::{HubOptions, ServiceFactory};

    use super::*;
    use crate::testutil::TempDir;

    fn lines(path: &std::path::Path) -> usize {
        std::fs::read_to_string(path).map_or(0, |t| t.lines().filter(|l| l.contains("service_command")).count())
    }

    #[test]
    fn commands_the_ticker_reported_are_not_reported_again_at_exit() {
        let dir = TempDir::new("ticker");
        let file = dir.path().join("stats.jsonl");
        let stats = Arc::new(Stats::open(Some(&file), false, Instant::now()).unwrap());
        let world = FakeWorld::new();
        let mut audio = world.create(ServiceId::Audio, &HubOptions::default());
        let ticker = Ticker::start(Arc::clone(&stats), Some(world.clone()), Duration::from_millis(20));
        for n in 0..2u8 {
            audio.command(ServiceCommand::Audio(AudioCommand::ChangeVolume(f32::from(n)))).unwrap();
        }
        let until = Instant::now() + Duration::from_secs(10);
        while lines(&file) < 2 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut reported = ticker.stop();
        assert_eq!(reported, 2);
        report_commands(&stats, &world, &mut reported);
        assert_eq!(lines(&file), 2, "the exit report adds nothing the ticker already said");

        audio.command(ServiceCommand::Audio(AudioCommand::ChangeVolume(9.0_f32))).unwrap();
        report_commands(&stats, &world, &mut reported);
        assert_eq!(lines(&file), 3, "a command after the ticker stopped is still reported once");
    }
}

//! `rsb render --timeline`: the real app (scenes, popup manager, script
//! thread, fake services) driven by a script on a virtual clock, with no
//! window. Bars and popups are drawn into memory by `Headless`; a `capture`
//! step writes what is on screen at that moment as a PNG. The same placement,
//! attach and animation code runs as in the live bar, so a picture at 400 ms
//! shows what the live bar shows 400 ms after the same input.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rsb_core::geom::RectPx;
use rsb_core::msg::{ScriptIn, ScriptOut, ScriptSink};
use rsb_core::services::ServiceId;
use rsb_core::time::{Clock, LocalTime, TimeZone};
use rsb_core::value::Value;
use rsb_core::wake::Wake;
use rsb_render::{Canvas, FontConfig, Renderer};
use rsb_services::fake::FakeWorld;
use rsb_win::UiEvent;
use serde_json::json;

use crate::app::{App, AppOptions};
use crate::args::TimelineOpts;
use crate::engine::{self, Engine, EngineOptions, ImageRoute};
use crate::fonts::{self, FontCache};
use crate::gate::{self, Gate};
use crate::headless::{Headless, Layer};
use crate::images::ImageLoader;
use crate::msg::{ConfigUpdate, ReloadKind, UiMsg};
use crate::pngread::{self, Picture};
use crate::popups::OverlayWatch;
use crate::stats::Stats;
use crate::timeline::{self, Entry, Step, Target};

use crate::{assets, cfgload, clock, logger, paths};

/// Exit code of a run that finished but could not do every step (a capture of
/// something that was not there).
pub const EXIT_STEP_FAILED: u8 = 1;
/// Exit code for a timeline file or option that cannot be used.
pub const EXIT_USAGE: u8 = 2;

/// Gap between two animation frames the stepper will not go below, so a scene
/// that asks for "now" again cannot hold virtual time still.
const MIN_STEP: Duration = Duration::from_micros(500);
/// Real time a service change gets to travel from the fake to the script thread.
const FAKE_SETTLE: Duration = Duration::from_millis(30);
/// Most frame steps one stretch between two timeline lines may take.
const MAX_STEPS_PER_GAP: u32 = 20_000;

/// The script thread's clock: monotonic time moves only when the run says so,
/// the wall clock never moves.
struct VirtualClock {
    base: Instant,
    offset_us: AtomicU64,
    wall: LocalTime,
}

impl VirtualClock {
    fn set(&self, at: Duration) {
        self.offset_us.store(at.as_micros() as u64, Ordering::Release);
    }
}

impl Clock for VirtualClock {
    fn mono(&self) -> Instant {
        self.base + Duration::from_micros(self.offset_us.load(Ordering::Acquire))
    }

    fn now(&self, zone: TimeZone) -> LocalTime {
        match zone {
            TimeZone::Local => self.wall,
            TimeZone::Utc => LocalTime::from_epoch_ms(self.wall.to_epoch_ms(), 0),
            TimeZone::Offset(m) => LocalTime::from_epoch_ms(self.wall.to_epoch_ms(), m),
        }
    }
}

/// Counts what arrives for the UI thread, so the stepper can tell when
/// everything that was going to happen has happened.
#[derive(Default)]
struct Activity(AtomicU64);

impl Activity {
    fn count(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
}

struct CountingWake(Arc<Activity>);

impl Wake for CountingWake {
    fn wake(&self) {
        self.0.0.fetch_add(1, Ordering::AcqRel);
    }
}

struct Sink {
    tx: Sender<UiMsg>,
    activity: Arc<Activity>,
}

impl ScriptSink for Sink {
    fn send(&self, out: ScriptOut) {
        if self.tx.send(UiMsg::Script(out)).is_ok() {
            self.activity.0.fetch_add(1, Ordering::AcqRel);
        }
    }
}

/// What the screen shows where no window is.
#[derive(Debug)]
pub enum Background {
    Flat([u8; 3]),
    Image(Picture),
}

impl Background {
    pub const DEFAULT: [u8; 3] = [0x30, 0x36, 0x40];

    /// `#rrggbb` or the path of a PNG.
    pub fn parse(text: Option<&str>) -> Result<Background, String> {
        let Some(text) = text else { return Ok(Background::Flat(Background::DEFAULT)) };
        if let Some(hex) = text.strip_prefix('#') {
            let ok = hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit());
            if !ok {
                return Err(format!("--background: '{text}' is not #rrggbb or a PNG file"));
            }
            let v = u32::from_str_radix(hex, 16).map_err(|e| e.to_string())?;
            return Ok(Background::Flat([(v >> 16) as u8, (v >> 8) as u8, v as u8]));
        }
        let image = pngread::read(Path::new(text)).map_err(|e| format!("--background: {e}"))?;
        Ok(Background::Image(image))
    }

    /// The colour under screen pixel (x, y), opaque.
    fn at(&self, x: i32, y: i32) -> [u8; 3] {
        match self {
            Background::Flat(c) => *c,
            Background::Image(img) => {
                if x < 0 || y < 0 || x as u32 >= img.width || y as u32 >= img.height {
                    return Background::DEFAULT;
                }
                let i = (y as usize * img.width as usize + x as usize) * 4;
                let (a, p) = (u32::from(img.rgba[i + 3]), &img.rgba[i..i + 4]);
                // Premultiplied over the flat grey, so a picture with holes still gives an opaque screen.
                let under = Background::DEFAULT;
                let mix = |c: u8, u: u8| (u32::from(c) + u32::from(u) * (255 - a) / 255).min(255) as u8;
                [mix(p[0], under[0]), mix(p[1], under[1]), mix(p[2], under[2])]
            }
        }
    }
}

/// `src` over `dst`, both premultiplied, as 0..=255 channels.
fn over(src: u8, a: u8, dst: u8) -> u8 {
    (u32::from(src) + (u32::from(dst) * (255 - u32::from(a)) + 127) / 255).min(255) as u8
}

/// Every layer drawn at its place on an opaque background: the screen.
pub fn compose(layers: &[Layer<'_>], screen: RectPx, bg: &Background) -> Canvas {
    let (w, h) = (screen.width().max(1) as u32, screen.height().max(1) as u32);
    let mut out = Canvas::new(w, h);
    let row_bytes = w as usize * 4;
    match bg {
        // One row of the flat colour, copied down the screen.
        Background::Flat([r, g, b]) => {
            for px in out.data[..row_bytes].as_chunks_mut::<4>().0 {
                *px = [*b, *g, *r, 255];
            }
            for y in 1..h as usize {
                out.data.copy_within(0..row_bytes, y * row_bytes);
            }
        }
        Background::Image(_) => {
            for y in 0..h as i32 {
                for x in 0..w as i32 {
                    let [r, g, b] = bg.at(screen.x0 + x, screen.y0 + y);
                    let i = (y as usize * w as usize + x as usize) * 4;
                    out.data[i..i + 4].copy_from_slice(&[b, g, r, 255]);
                }
            }
        }
    }
    for layer in layers {
        let r = layer.rect;
        let (x0, y0) = (r.x0.max(screen.x0), r.y0.max(screen.y0));
        let (x1, y1) = (r.x1.min(screen.x1), r.y1.min(screen.y1));
        for y in y0..y1 {
            for x in x0..x1 {
                let o = ((y - screen.y0) as usize * w as usize + (x - screen.x0) as usize) * 4;
                if let Some(c) = layer.dim {
                    let a = c.a.clamp(0.0, 1.0);
                    let mix =
                        |dst: u8, c: f32| (f32::from(dst) * (1.0 - a) + c.clamp(0.0, 1.0) * 255.0 * a).round() as u8;
                    out.data[o] = mix(out.data[o], c.b);
                    out.data[o + 1] = mix(out.data[o + 1], c.g);
                    out.data[o + 2] = mix(out.data[o + 2], c.r);
                } else if let Some(px) = layer.pixels {
                    let s = ((y - r.y0) as usize * r.width() as usize + (x - r.x0) as usize) * 4;
                    let Some(src) = px.get(s..s + 4) else { continue };
                    let a = src[3];
                    if a == 0 {
                        continue;
                    }
                    for (dst, &s) in out.data[o..o + 3].iter_mut().zip(&src[..3]) {
                        *dst = over(s, a, *dst);
                    }
                }
            }
        }
    }
    out
}

/// A picture the run has written.
struct Written {
    t_ms: u64,
    label: String,
    target: String,
    size: (u32, u32),
}

struct Run {
    app: App,
    win: Headless,
    engine: Option<Engine>,
    clock: Arc<VirtualClock>,
    base: Instant,
    at: Duration,
    activity: Arc<Activity>,
    world: FakeWorld,
    shadow: Arc<Mutex<HashMap<ServiceId, Value>>>,
    bg: Background,
    out_dir: PathBuf,
    written: Vec<Written>,
    problems: Vec<String>,
    stats: Arc<Stats>,
}

impl Run {
    fn set_time(&mut self, at: Duration) {
        self.at = at;
        self.clock.set(at);
        clock::set_virtual(Some(self.base + at));
    }

    /// Hands one event to the app, then every event that handling produced.
    fn deliver(&mut self, event: UiEvent) {
        self.app.handle_in(&mut self.win, event);
        self.pump_events();
    }

    fn pump_events(&mut self) {
        while let Some(e) = self.win.next_event() {
            self.app.handle_in(&mut self.win, e);
        }
    }

    /// One round trip to the script thread: when it answers, everything sent
    /// before it has been taken in and the turn that ran it is over.
    fn round_trip(&self) -> bool {
        let Some(engine) = &self.engine else { return false };
        let (tx, rx) = channel();
        if engine.script_tx.send(ScriptIn::Status { reply: tx }).is_err() {
            return false;
        }
        rx.recv_timeout(Duration::from_secs(5)).is_ok()
    }

    /// Lets the script thread, the loader threads and the hub finish what the
    /// last step started: round trips to the script thread and a wake of the
    /// app until nothing new arrives twice running. Timers that came due at
    /// the virtual time also run (any message makes the script thread run its
    /// due timers).
    fn settle(&mut self) {
        let mut quiet = 0;
        for _ in 0..400 {
            let before = self.activity.count();
            if !self.round_trip() {
                break;
            }
            std::thread::sleep(Duration::from_micros(300));
            self.deliver(UiEvent::Wake);
            if self.activity.count() == before {
                quiet += 1;
                if quiet >= 2 {
                    return;
                }
            } else {
                quiet = 0;
            }
        }
        log::warn!("the timeline run did not settle within its round limit");
    }

    /// The first frame of every bar: waits (in real time) for each widget to
    /// report, then lets the first-frame grace period pass in virtual time.
    fn first_frames(&mut self) {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            self.settle();
            if self.app.awaiting() == 0 || Instant::now() >= until {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // A bar whose widgets never all reported draws after its grace period.
        for _ in 0..4 {
            if self.app.all_presented() {
                break;
            }
            let next = self.at + Duration::from_millis(100);
            self.set_time(next);
            self.deliver(UiEvent::FrameDue);
            self.settle();
        }
    }

    /// Plays animation frames up to virtual time `to`, one per frame deadline
    /// the app asked for.
    fn advance_to(&mut self, to: Duration) {
        let target = self.base + to;
        let mut steps = 0;
        while let Some(due) = self.win.deadline() {
            if due > target {
                break;
            }
            steps += 1;
            if steps > MAX_STEPS_PER_GAP {
                self.problems.push(format!(
                    "more than {MAX_STEPS_PER_GAP} frames between two steps at {} ms",
                    self.at.as_millis()
                ));
                self.win.take_deadline();
                break;
            }
            let at = due.saturating_duration_since(self.base).max(self.at + MIN_STEP);
            if at > to {
                break;
            }
            self.set_time(at);
            self.win.take_deadline();
            self.deliver(UiEvent::FrameDue);
            self.settle();
        }
        if to > self.at {
            self.set_time(to);
        }
        self.settle();
    }

    fn apply_fake(&mut self, service: ServiceId, fields: &serde_json::Map<String, serde_json::Value>) {
        let mut shadow = self.shadow.lock().unwrap_or_else(|e| e.into_inner());
        let mut value = shadow.get(&service).cloned().unwrap_or_else(|| Value::Map(BTreeMap::new()));
        if !matches!(value, Value::Map(_)) {
            value = Value::Map(BTreeMap::new());
        }
        if let Value::Map(m) = &mut value {
            for (k, v) in fields {
                m.insert(k.clone(), Value::from_json(v));
            }
        }
        shadow.insert(service, value.clone());
        drop(shadow);
        self.world.set(service, value);
        std::thread::sleep(FAKE_SETTLE);
        self.settle();
    }

    fn capture(&mut self, e: &Entry, label: &str, target: &Target) {
        let (name, found) = match target {
            Target::Screen => {
                let screen = self.win.screen();
                let layers = self.win.layers();
                ("screen".to_string(), Some(compose(&layers, screen, &self.bg)))
            }
            Target::Bar(n) => (format!("bar:{n}"), self.win.bar_canvas(n).map(|(_, c)| c)),
            Target::Popup(n) => (format!("popup:{n}"), self.win.popup_canvas(n).map(|(_, c)| c)),
        };
        let Some(canvas) = found else {
            let why = format!("line {}: capture '{label}': {name} is not on screen at {} ms", e.line, e.ms);
            eprintln!("rsb: {why}");
            self.problems.push(why);
            return;
        };
        let file = format!("{label}.png");
        let result = canvas
            .to_png()
            .map_err(|e| e.to_string())
            .and_then(|png| std::fs::write(self.out_dir.join(&file), png).map_err(|e| e.to_string()));
        match result {
            Ok(()) => {
                self.stats.emit("timeline", json!({"label": label, "at_ms": e.ms, "target": name}));
                self.written.push(Written {
                    t_ms: e.ms,
                    label: label.to_string(),
                    target: name,
                    size: (canvas.width, canvas.height),
                });
            }
            Err(why) => {
                let why = format!("line {}: capture '{label}': {why}", e.line);
                eprintln!("rsb: {why}");
                self.problems.push(why);
            }
        }
    }

    fn step(&mut self, e: &Entry) {
        match &e.step {
            Step::Sim(action) => {
                if let Some(problem) = self.app.on_sim(&mut self.win, action.clone()) {
                    let why = format!("line {}: {problem} (at {} ms)", e.line, e.ms);
                    eprintln!("rsb: {why}");
                    self.problems.push(why);
                }
                self.pump_events();
                self.deliver(UiEvent::Wake);
                self.settle();
            }
            Step::Fake { service, fields } => self.apply_fake(*service, fields),
            Step::Capture { label, target } => self.capture(e, label, target),
        }
    }
}

fn fail(code: u8, text: &str) -> ExitCode {
    eprintln!("rsb: {text}");
    ExitCode::from(code)
}

pub fn run(config_dir: Option<&Path>, log_dir: Option<&Path>, o: &TimelineOpts) -> ExitCode {
    let text = match std::fs::read_to_string(&o.file) {
        Ok(t) => t,
        Err(e) => return fail(EXIT_USAGE, &format!("cannot read the timeline: {e}")),
    };
    let entries = match timeline::parse(&text) {
        Ok(v) => v,
        Err(e) => return fail(EXIT_USAGE, &e),
    };
    let mut labels: Vec<&str> = Vec::new();
    for e in &entries {
        if let Step::Capture { label, .. } = &e.step {
            if labels.contains(&label.as_str()) {
                return fail(EXIT_USAGE, &format!("timeline line {}: the label '{label}' is used twice", e.line));
            }
            labels.push(label);
        }
    }
    let bg = match Background::parse(o.background.as_deref()) {
        Ok(b) => b,
        Err(e) => return fail(EXIT_USAGE, &e),
    };
    if let Err(e) = std::fs::create_dir_all(&o.out_dir) {
        return fail(EXIT_USAGE, &format!("cannot create the output folder: {e}"));
    }
    let index = match File::create(o.out_dir.join("index.jsonl")) {
        Ok(f) => BufWriter::new(f),
        Err(e) => return fail(EXIT_USAGE, &format!("cannot create index.jsonl: {e}")),
    };

    let log_dir = paths::log_dir(log_dir);
    let log_file = match logger::init(&log_dir) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("rsb: {e} (continuing without a log)");
            logger::file_path(&log_dir, 0)
        }
    };
    let config_dir = paths::config_dir(config_dir);
    let mut font_cache = FontCache::default();
    let mut built = cfgload::build_cached(&config_dir, o.preset.as_deref(), "default", &mut font_cache);
    if built.config.is_none() {
        built.config = rsb_core::config::load_config(None, &|n| assets::preset_toml(n).map(str::to_string))
            .config
            .or_else(|| Some(rsb_core::config::default_config()));
    }
    let first = ConfigUpdate { kind: ReloadKind::Initial, hashes: built.hashes, built, notified_wall_ms: 0 };

    let (width, height) = (o.screen.0 as i32, o.screen.1 as i32);
    let dpi = (96.0 * o.scale).round() as u32;
    let win = Headless::new(&[(width, height, dpi)]);

    let stats = match Stats::open(o.stats_out.as_deref(), true, Instant::now()) {
        Ok(s) => Arc::new(s),
        Err(e) => return fail(EXIT_USAGE, &e),
    };
    stats.emit("start", json!({"version": env!("CARGO_PKG_VERSION"), "timeline": true}));
    let base = Instant::now();
    let clock_src = Arc::new(VirtualClock { base, offset_us: AtomicU64::new(0), wall: o.now });
    let activity = Arc::new(Activity::default());
    let wake: Arc<dyn Wake> = Arc::new(CountingWake(Arc::clone(&activity)));
    let (ui_tx, ui_rx) = channel::<UiMsg>();
    let world = FakeWorld::with_sample_values();
    let shadow: Arc<Mutex<HashMap<ServiceId, Value>>> = Arc::new(Mutex::new(HashMap::new()));
    let observer = {
        let shadow = Arc::clone(&shadow);
        Arc::new(move |ev: &rsb_core::services::ServiceEvent| {
            if let Some(v) = &ev.value {
                shadow.lock().unwrap_or_else(|e| e.into_inner()).insert(ev.service, v.clone());
            }
        })
    };
    let engine = engine::start(
        EngineOptions {
            clock: Some(Arc::clone(&clock_src) as Arc<dyn Clock>),
            config_dir: config_dir.clone(),
            dark: o.dark,
            world: Some(world.clone()),
            glaze_url: None,
            test_stall: None,
            // A timeline reads fake services only; commands are recorded and the fakes take them.
            gate: Some(Gate::new(gate::select_render(false), Arc::new(Stats::off()))),
            images: Some(ImageRoute { tx: ui_tx.clone(), wake: Arc::clone(&wake) }),
            observe: Some(observer),
        },
        Box::new(Sink { tx: ui_tx.clone(), activity: Arc::clone(&activity) }),
    );

    let mut renderer = Renderer::new(FontConfig::default());
    if let Err(e) = fonts::register_bundled(&mut renderer) {
        log::warn!("the bundled font could not be registered: {e}");
    }
    let app = App::new(
        AppOptions {
            config_dir,
            log_file,
            no_reserve: false,
            offscreen: false,
            simulated_pointer: true,
            no_hook: true,
            no_activate: true,
            gated: true,
            overlays: Arc::new(OverlayWatch::default()),
        },
        Arc::clone(&stats),
        ui_rx,
        engine.script_tx.clone(),
        ImageLoader::start(ui_tx.clone(), Arc::clone(&wake)),
        renderer,
        o.dark,
    );
    let mut r = Run {
        app,
        win,
        engine: Some(engine),
        clock: clock_src,
        base,
        at: Duration::ZERO,
        activity,
        world,
        shadow,
        bg,
        out_dir: o.out_dir.clone(),
        written: Vec::new(),
        problems: Vec::new(),
        stats,
    };
    r.set_time(Duration::ZERO);
    r.app.start_in(&mut r.win, first);
    r.pump_events();
    r.first_frames();

    for e in &entries {
        r.advance_to(Duration::from_millis(e.ms));
        r.step(e);
    }

    clock::set_virtual(None);
    let (written, problems) = (std::mem::take(&mut r.written), std::mem::take(&mut r.problems));
    let mut index = index;
    for w in &written {
        let line = json!({"t_ms": w.t_ms, "label": w.label, "target": w.target, "size": [w.size.0, w.size.1],
                          "file": format!("{}.png", w.label)});
        let _ = writeln!(index, "{line}");
    }
    let _ = index.flush();
    if let Some(engine) = r.engine.take() {
        engine.stop();
    }
    println!("wrote {} picture(s), {} problem(s)", written.len(), problems.len());
    if problems.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(EXIT_STEP_FAILED) }
}

#[cfg(test)]
mod tests {
    use rsb_core::color::Color;

    use super::*;

    fn flat(c: [u8; 3]) -> Background {
        Background::Flat(c)
    }

    #[test]
    fn the_background_is_a_flat_colour_or_a_png() {
        assert!(matches!(Background::parse(None), Ok(Background::Flat(c)) if c == Background::DEFAULT));
        assert!(matches!(Background::parse(Some("#102030")), Ok(Background::Flat([0x10, 0x20, 0x30]))));
        assert!(Background::parse(Some("#12")).is_err());
        assert!(Background::parse(Some("#gggggg")).is_err());
        assert!(Background::parse(Some("no-such-file.png")).unwrap_err().contains("cannot read"));
    }

    #[test]
    fn layers_are_composed_over_the_background_in_order() {
        // A 2x2 opaque red block at (1, 0), then a half-transparent white one over its right half.
        let red = [0u8, 0, 255, 255].repeat(4);
        let half = [128u8, 128, 128, 128].repeat(4);
        let layers = [
            Layer { rect: RectPx::new(1, 0, 3, 2), pixels: Some(&red), dim: None },
            Layer { rect: RectPx::new(2, 0, 4, 2), pixels: Some(&half), dim: None },
        ];
        let out = compose(&layers, RectPx::new(0, 0, 4, 2), &flat([10, 20, 30]));
        let px = |x: usize, y: usize| {
            let i = (y * 4 + x) * 4;
            [out.data[i + 2], out.data[i + 1], out.data[i], out.data[i + 3]]
        };
        assert_eq!(px(0, 0), [10, 20, 30, 255], "background where nothing is");
        assert_eq!(px(1, 1), [255, 0, 0, 255], "an opaque pixel replaces it");
        assert_eq!(px(3, 0), [133, 138, 143, 255], "premultiplied over");
        let both = px(2, 0);
        assert!(both[0] > 128 && both[1] > 60 && both[2] > 60, "{both:?}: red under the half white");
    }

    #[test]
    fn a_dim_darkens_everything_below_it() {
        let red = [0u8, 0, 255, 255].repeat(4);
        let layers = [
            Layer { rect: RectPx::new(0, 0, 2, 2), pixels: Some(&red), dim: None },
            Layer { rect: RectPx::new(0, 0, 4, 2), pixels: None, dim: Some(Color { r: 0.0, g: 0.0, b: 0.0, a: 0.5 }) },
        ];
        let out = compose(&layers, RectPx::new(0, 0, 4, 2), &flat([100, 100, 100]));
        assert_eq!([out.data[2], out.data[1], out.data[0]], [128, 0, 0], "red halved");
        let i = 3 * 4;
        assert_eq!([out.data[i + 2], out.data[i + 1], out.data[i]], [50, 50, 50], "background halved");
    }

    #[test]
    fn a_layer_partly_off_the_screen_is_clipped() {
        let blue = [255u8, 0, 0, 255].repeat(16);
        let layers = [Layer { rect: RectPx::new(-2, -2, 2, 2), pixels: Some(&blue), dim: None }];
        let out = compose(&layers, RectPx::new(0, 0, 3, 3), &flat([0, 0, 0]));
        assert_eq!(out.data[0], 255, "(0,0) is inside the layer");
        assert_eq!(out.data[2 * 4], 0, "(2,0) is outside it");
    }

    #[test]
    fn the_virtual_clock_moves_only_when_told_and_the_wall_clock_never() {
        let wall = crate::args::parse(&[
            "render".into(),
            "--timeline".into(),
            "t.txt".into(),
            "--out-dir".into(),
            "C:\\x\\target\\o".into(),
        ])
        .map(|p| match p.cmd {
            crate::args::Cmd::Timeline(t) => t.now,
            other => panic!("{other:?}"),
        })
        .unwrap();
        let c = VirtualClock { base: Instant::now(), offset_us: AtomicU64::new(0), wall };
        let t0 = c.mono();
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(c.mono(), t0);
        c.set(Duration::from_millis(250));
        assert_eq!(c.mono() - t0, Duration::from_millis(250));
        assert_eq!(c.now(TimeZone::Local), wall);
        assert_eq!(c.now(TimeZone::Utc).to_epoch_ms(), wall.to_epoch_ms());
        assert_eq!((wall.year, wall.month, wall.day, wall.hour, wall.minute), (2026, 11, 25, 11, 10));
    }
}

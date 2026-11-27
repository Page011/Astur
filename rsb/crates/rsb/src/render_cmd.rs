//! `rsb render`: draws the bar to a PNG without a window. The same engine
//! runs (script thread, services hub) with sample values, or with `--live`
//! the real ones; the scene and renderer are the ones the bar uses.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use rsb_core::config::{BUILTIN_PRESETS, default_config, load_config};
use rsb_core::css::MediaEnv;
use rsb_core::diag::{DiagKind, Diagnostic, Plate};
use rsb_core::element::{InstanceId, WidgetTree};
use rsb_core::geom::RectPx;
use rsb_core::msg::{ScriptIn, ScriptOut, ScriptSink};
use rsb_core::scene::{BarGeometry, Scene, SceneEnv};
use rsb_render::{FontConfig, Renderer, render_offscreen};
use rsb_services::fake::FakeWorld;
use rsb_win::MonitorInfo;

use crate::args::RenderOpts;
use crate::cfgload::Built;
use crate::fonts::{self, FontRegistry};
use crate::gate::{self, Gate};
use crate::icons::BundledIcons;
use crate::images::Kind;
use crate::plan::{BarPlan, InstanceTable, build_instances};
use crate::stats::Stats;
use crate::{assets, cfgload, engine, images, logger, paths};

/// How long a drawing stays open for further updates once every widget has
/// drawn, when no wait was asked for.
const SETTLE: Duration = Duration::from_millis(80);

struct ChannelSink(Sender<ScriptOut>);

/// Real monotonic time with the wall clock held at one moment (`--now`), so
/// every run draws the same clock and date.
struct FixedWall(rsb_core::time::LocalTime);

impl rsb_core::time::Clock for FixedWall {
    fn mono(&self) -> Instant {
        Instant::now()
    }

    fn now(&self, zone: rsb_core::time::TimeZone) -> rsb_core::time::LocalTime {
        use rsb_core::time::{LocalTime, TimeZone};
        match zone {
            TimeZone::Local => self.0,
            TimeZone::Utc => LocalTime::from_epoch_ms(self.0.to_epoch_ms(), 0),
            TimeZone::Offset(m) => LocalTime::from_epoch_ms(self.0.to_epoch_ms(), m),
        }
    }
}

impl ScriptSink for ChannelSink {
    fn send(&self, out: ScriptOut) {
        let _ = self.0.send(out);
    }
}

/// What the run collected for one bar.
struct Collected {
    trees: HashMap<InstanceId, Arc<WidgetTree>>,
    plates: HashMap<InstanceId, Plate>,
    reported: HashSet<InstanceId>,
}

/// One bar drawn to a PNG.
pub struct Drawn {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Widget instances on the bar.
    pub widgets: usize,
    /// The text of the bar's own plate, when it shows one.
    pub bar_plate: Option<String>,
    /// (widget name, plate text) for each widget that shows a plate.
    pub widget_plates: Vec<(String, String)>,
}

pub fn run(config_dir: Option<&Path>, log_dir: Option<&Path>, o: &RenderOpts) -> ExitCode {
    let log_dir = paths::log_dir(log_dir);
    if let Err(e) = logger::init(&log_dir) {
        eprintln!("rsb: {e} (continuing without a log)");
    }
    if let Some(p) = &o.preset
        && !BUILTIN_PRESETS.contains(&p.as_str())
    {
        eprintln!("rsb: unknown preset '{p}' (presets: {})", BUILTIN_PRESETS.join(", "));
        return ExitCode::from(1);
    }
    let config_dir = paths::config_dir(config_dir);
    let built = cfgload::build(&config_dir, o.preset.as_deref(), "default");
    let drawn = match draw(&config_dir, &built, o) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("rsb: {e}");
            return ExitCode::from(1);
        }
    };
    if let Err(e) = std::fs::write(&o.out, &drawn.png) {
        eprintln!("rsb: could not write the picture: {e}");
        return ExitCode::from(1);
    }
    println!(
        "wrote {} ({}x{} px, {}, {})",
        o.out.display(),
        drawn.width,
        drawn.height,
        counted(drawn.widgets, "widget"),
        counted(drawn.widget_plates.len() + usize::from(drawn.bar_plate.is_some()), "plate")
    );
    if let Some(plate) = &drawn.bar_plate {
        println!("plate: bar: {plate}");
    }
    for (name, plate) in &drawn.widget_plates {
        println!("plate: {name}: {plate}");
    }
    ExitCode::SUCCESS
}

/// Draws the bar that `built` describes, with the config folder `config_dir`
/// as the place user widgets are read from. The same engine, scene and
/// renderer as the running bar; the options are those of `rsb render` (its
/// `out` and `preset` are not used here). No log is opened and nothing is
/// written: the caller gets the PNG bytes.
pub fn draw(config_dir: &Path, built: &Built, o: &RenderOpts) -> Result<Drawn, String> {
    for d in &built.diagnostics {
        log::warn!("{d}");
    }
    let config = built
        .config
        .clone()
        .or_else(|| load_config(None, &|n| assets::preset_toml(n).map(str::to_string)).config)
        .unwrap_or_else(default_config);
    let sheet = built.sheet.clone().unwrap_or_else(|| built.base_sheet.clone());
    let Some(bar) = config.bars.first().cloned() else {
        return Err("the config has no bars to draw".to_string());
    };

    let mut renderer = Renderer::new(FontConfig::default());
    if let Err(e) = fonts::register_bundled(&mut renderer) {
        log::warn!("the bundled font could not be registered: {e}");
    }
    let mut diags: Vec<Diagnostic> = built.diagnostics.clone();
    let mut font_diags = built.font_diagnostics.clone();
    font_diags.extend(FontRegistry::default().apply(&mut renderer, &built.font_files).diagnostics);
    for (name, pos) in &built.fonts {
        if !renderer.has_family(name) {
            font_diags.push(Diagnostic::warning(
                DiagKind::Style,
                Some(pos.clone()),
                format!("font '{name}' is not installed; using Segoe UI"),
            ));
        }
    }
    for d in &font_diags {
        log::warn!("{d}");
    }
    diags.extend(font_diags);

    let height = ((bar.height + if bar.floating { 2 * bar.margin } else { 0 }) as f32 * o.scale).round() as u32;
    let monitor = MonitorInfo {
        index: 0,
        handle: 1,
        device: "\\\\.\\DISPLAY1".into(),
        primary: true,
        rect: RectPx::new(0, 0, o.width as i32, height as i32),
        work: RectPx::new(0, 0, o.width as i32, height as i32),
        dpi: (96.0 * o.scale).round() as u32,
    };
    let plan = BarPlan { spec: bar.clone(), monitor };
    let mut table = InstanceTable::new();
    let insts = build_instances(&config, std::slice::from_ref(&plan), &[o.scale], &mut table).remove(0);

    let (tx, rx) = channel::<ScriptOut>();
    let world = (!o.live).then(FakeWorld::with_sample_values);
    let engine = engine::start(
        engine::EngineOptions {
            clock: o.now.map(|t| Arc::new(FixedWall(t)) as Arc<dyn rsb_core::time::Clock>),
            config_dir: config_dir.to_path_buf(),
            dark: o.dark,
            world,
            glaze_url: None,
            test_stall: None,
            // Every render records commands; only the fakes take them.
            gate: Some(Gate::new(gate::select_render(o.live), Arc::new(Stats::off()))),
            images: None,
            observe: None,
        },
        Box::new(ChannelSink(tx)),
    );
    let specs = insts.all().map(|p| p.spec.clone()).collect();
    let _ = engine.script_tx.send(ScriptIn::SetInstances(specs));

    let wait = Duration::from_millis(o.wait_ms.unwrap_or(if o.live { 3000 } else { 400 }));
    let mut deadline = Instant::now() + wait;
    // With sample values and no explicit wait, stop once every widget has
    // drawn and nothing more has arrived for a moment, instead of sitting
    // out the whole wait.
    let settle = (o.wait_ms.is_none() && !o.live).then_some(SETTLE);
    let expected: HashSet<InstanceId> = insts.all().map(|p| p.spec.instance).collect();
    let mut got = Collected { trees: HashMap::new(), plates: HashMap::new(), reported: HashSet::new() };
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(ScriptOut::Trees(batch)) => {
                for t in batch {
                    got.reported.insert(t.instance);
                    got.trees.insert(t.instance, t);
                }
            }
            Ok(ScriptOut::Plate { instance, plate }) => {
                got.reported.insert(instance);
                match plate {
                    Some(p) => {
                        got.plates.insert(instance, p);
                    }
                    None => {
                        got.plates.remove(&instance);
                    }
                }
            }
            Ok(ScriptOut::Diagnostics(d)) => diags.extend(d),
            Ok(ScriptOut::ErrorNote { .. }) => {}
            Err(_) => break,
        }
        if let Some(settle) = settle
            && expected.is_subset(&got.reported)
        {
            deadline = deadline.min(Instant::now() + settle);
        }
    }
    let names: HashMap<InstanceId, String> =
        insts.all().map(|p| (p.spec.instance, p.spec.widget.to_string())).collect();
    let silent = names.keys().filter(|i| !got.reported.contains(i)).count();
    if silent > 0 {
        log::warn!("{silent} widget(s) had not drawn anything after {} ms", wait.as_millis());
    }

    let env = SceneEnv { media: MediaEnv { dark: o.dark }, icon_font: renderer.icon_font(), fullscreen: false };
    let mut scene = Scene::new(&bar, BarGeometry { width_px: o.width, height_px: height, scale: o.scale }, env);
    scene.set_stylesheet(sheet);
    scene.set_slots(insts.slots());
    for tree in got.trees.values() {
        scene.set_tree(tree.clone());
        let mut found = Vec::new();
        if let Some(root) = &tree.root {
            images::files_of(root, &mut found);
        }
        for (path, kind) in found {
            let loaded = match (&tree.grant, kind) {
                (None, _) => Err("the widget has no file permission".to_string()),
                (Some(g), Kind::Png) => images::load(&path, g).map(|img| {
                    let size = (img.width, img.height);
                    renderer.set_image(Arc::clone(&path), img);
                    size
                }),
                (Some(g), Kind::Svg) => images::load_svg(&path, g).map(|svg| {
                    let (w, h) = renderer.set_parsed_svg(Arc::clone(&path), svg);
                    (w.ceil() as u32, h.ceil() as u32)
                }),
            };
            match loaded {
                Ok((w, h)) => scene.set_image_size(&path, w, h),
                Err(e) => {
                    log::warn!("an image could not be loaded: {e}");
                    scene.image_failed(&path);
                }
            }
        }
    }
    for (id, plate) in &got.plates {
        scene.set_widget_plate(*id, Some(plate.clone()));
        log::warn!("plate on {}: {}", names.get(id).map_or("a widget", String::as_str), plate.display_text());
    }
    let out = scene.frame(Instant::now(), &mut renderer);
    for d in scene.take_diagnostics() {
        log::warn!("{}", d.diagnostic);
        // As on the running bar, a style problem inside a widget reaches the bar plate too.
        if !diags.iter().any(|x| x.to_string() == d.diagnostic.to_string()) {
            diags.push(d.diagnostic);
        }
    }
    // Bar-level problems join the plate; draw again with it in place.
    let bar_plate = Plate::from_diagnostics(&diags);
    if let Some(plate) = &bar_plate {
        log::warn!("plate on the bar: {}", plate.display_text());
        scene.set_bar_plate(Some(plate.clone()));
    }
    let out = if scene.needs_frame(Instant::now()) { scene.frame(Instant::now(), &mut renderer) } else { out };

    BundledIcons::default().ensure(&mut renderer, &out.draw);
    let canvas = match render_offscreen(&mut renderer, &out.draw) {
        Ok(c) => c,
        Err(e) => {
            engine.stop();
            return Err(format!("could not draw the bar: {e}"));
        }
    };
    let png = canvas.to_png().map_err(|e| format!("could not write the picture: {e}"));
    engine.stop();
    Ok(Drawn {
        png: png?,
        width: canvas.width,
        height: canvas.height,
        widgets: names.len(),
        bar_plate: bar_plate.map(|p| p.display_text()),
        widget_plates: got
            .plates
            .iter()
            .map(|(id, plate)| (names.get(id).map_or("a widget", String::as_str).to_string(), plate.display_text()))
            .collect(),
    })
}

/// `1 plate`, `0 plates`, `10 plates`.
fn counted(n: usize, noun: &str) -> String {
    if n == 1 { format!("1 {noun}") } else { format!("{n} {noun}s") }
}

#[cfg(test)]
mod tests {
    use super::{counted, draw};
    use crate::args::RenderOpts;
    use crate::cfgload;
    use crate::testutil::TempDir;
    use rsb_core::config::BUILTIN_PRESETS;
    use rsb_core::time::LocalTime;
    use std::path::PathBuf;

    /// Stopping once every widget has drawn and the bar has gone quiet gives
    /// the picture the full 400 ms wait gives.
    #[test]
    fn finishing_early_draws_the_same_picture_as_waiting_it_out() {
        let dir = TempDir::new("rc");
        let t = LocalTime { year: 2026, month: 11, day: 25, hour: 10, minute: 42, ..Default::default() };
        let now = Some(LocalTime::from_epoch_ms(t.to_epoch_ms(), 0));
        for name in BUILTIN_PRESETS {
            let built = cfgload::build(dir.path(), Some(name), "default");
            for dark in [true, false] {
                let opts = |wait_ms| RenderOpts {
                    out: PathBuf::new(),
                    width: 1920,
                    scale: 1.0,
                    dark,
                    preset: None,
                    live: false,
                    wait_ms,
                    now,
                };
                let early = draw(dir.path(), &built, &opts(None)).expect("draws");
                let full = draw(dir.path(), &built, &opts(Some(400))).expect("draws");
                assert!(early.png == full.png, "{name} dark={dark}: the pictures differ");
            }
        }
    }

    #[test]
    fn a_count_of_one_is_singular() {
        assert_eq!(counted(0, "plate"), "0 plates");
        assert_eq!(counted(1, "plate"), "1 plate");
        assert_eq!(counted(1, "widget"), "1 widget");
        assert_eq!(counted(10, "widget"), "10 widgets");
    }
}

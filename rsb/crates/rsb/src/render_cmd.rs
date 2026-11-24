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
use rsb_core::element::{Element, ElementKind, InstanceId, WidgetTree};
use rsb_core::geom::RectPx;
use rsb_core::msg::{ScriptIn, ScriptOut, ScriptSink};
use rsb_core::scene::{BarGeometry, Scene, SceneEnv};
use rsb_render::{FontConfig, Renderer, render_offscreen};
use rsb_services::fake::FakeWorld;
use rsb_win::MonitorInfo;

use crate::args::RenderOpts;
use crate::plan::{BarPlan, InstanceTable, build_instances};
use crate::{assets, cfgload, engine, images, logger, paths};

struct ChannelSink(Sender<ScriptOut>);

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

fn image_paths(e: &Element, out: &mut Vec<Arc<str>>) {
    if e.kind == ElementKind::Image
        && let Some(img) = &e.image
    {
        out.push(img.path.clone());
    }
    for c in &e.children {
        image_paths(c, out);
    }
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
        eprintln!("rsb: the config has no bars to draw");
        return ExitCode::from(1);
    };

    let mut renderer = Renderer::new(FontConfig::default());
    let mut diags: Vec<Diagnostic> = built.diagnostics.clone();
    for (name, pos) in &built.fonts {
        if !renderer.has_family(name) {
            diags.push(Diagnostic::warning(
                DiagKind::Style,
                Some(pos.clone()),
                format!("font '{name}' is not installed; using Segoe UI"),
            ));
        }
    }

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
            config_dir: config_dir.clone(),
            dark: o.dark,
            world,
            glaze_url: None,
            test_stall: None,
        },
        Box::new(ChannelSink(tx)),
    );
    let specs = insts.all().map(|p| p.spec.clone()).collect();
    let _ = engine.script_tx.send(ScriptIn::SetInstances(specs));

    let wait = Duration::from_millis(o.wait_ms.unwrap_or(if o.live { 3000 } else { 400 }));
    let deadline = Instant::now() + wait;
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
            Err(_) => break,
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
            image_paths(root, &mut found);
        }
        for path in found {
            match tree.grant.as_ref().map(|g| images::load(&path, g)) {
                Some(Ok(img)) => {
                    scene.set_image_size(&path, img.width, img.height);
                    renderer.set_image(path, img);
                }
                Some(Err(e)) => {
                    log::warn!("an image could not be loaded: {e}");
                    scene.image_failed(&path);
                }
                None => scene.image_failed(&path),
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

    let canvas = match render_offscreen(&mut renderer, &out.draw) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rsb: could not draw the bar: {e}");
            engine.stop();
            return ExitCode::from(1);
        }
    };
    let result = canvas
        .to_png()
        .map_err(|e| e.to_string())
        .and_then(|png| std::fs::write(&o.out, png).map_err(|e| e.to_string()));
    engine.stop();
    if let Err(e) = result {
        eprintln!("rsb: could not write the picture: {e}");
        return ExitCode::from(1);
    }
    println!(
        "wrote {} ({}x{} px, {}, {})",
        o.out.display(),
        canvas.width,
        canvas.height,
        counted(names.len(), "widget"),
        counted(got.plates.len() + usize::from(bar_plate.is_some()), "plate")
    );
    if let Some(plate) = &bar_plate {
        println!("plate: bar: {}", plate.display_text());
    }
    for (id, plate) in &got.plates {
        println!("plate: {}: {}", names.get(id).map_or("a widget", String::as_str), plate.display_text());
    }
    ExitCode::SUCCESS
}

/// `1 plate`, `0 plates`, `10 plates`.
fn counted(n: usize, noun: &str) -> String {
    if n == 1 { format!("1 {noun}") } else { format!("{n} {noun}s") }
}

#[cfg(test)]
mod tests {
    use super::counted;

    #[test]
    fn a_count_of_one_is_singular() {
        assert_eq!(counted(0, "plate"), "0 plates");
        assert_eq!(counted(1, "plate"), "1 plate");
        assert_eq!(counted(1, "widget"), "1 widget");
        assert_eq!(counted(10, "widget"), "10 widgets");
    }
}

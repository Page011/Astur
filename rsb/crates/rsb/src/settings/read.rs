//! The read verbs: `get`, `presets`, `widgets`, `schema` and `log`.

use std::collections::BTreeMap;

use rsb_core::config::{
    BarSources, ConfigSources, FullscreenMode, MonitorSelector, Position, Sourced, WidgetRef, ZoneItem,
};
use rsb_core::contract::{ConfigFile, ErrorCode, Reply, Request, SettingsError, Source};
use serde_json::{Value, json};

use super::backend::{AccentView, Backend, FileError, Loaded, PresetView, SchemeAccent, TuneView};
use super::bar;
use super::{Ctx, schema};

/// The bar keys a page may change: `editable` in `get`.
pub const EDITABLE_BAR_KEYS: [&str; 6] = ["position", "height", "floating", "left", "center", "right"];

pub fn source_name(s: Source) -> &'static str {
    match s {
        Source::Default => "default",
        Source::Preset => "preset",
        Source::User => "user",
    }
}

/// A widget ref as it is written in `bar.toml`.
pub fn ref_text(r: &WidgetRef) -> String {
    match &r.instance {
        Some(i) => format!("{}#{i}", r.name),
        None => r.name.clone(),
    }
}

fn zone_value(items: &[ZoneItem]) -> Value {
    Value::Array(items.iter().map(|i| Value::String(ref_text(&i.widget))).collect())
}

fn monitors_value(list: &[MonitorSelector]) -> Value {
    Value::Array(
        list.iter()
            .map(|m| match m {
                MonitorSelector::All => json!("*"),
                MonitorSelector::Primary => json!("primary"),
                MonitorSelector::Index(i) => json!(i),
                MonitorSelector::Device(d) => json!(d),
            })
            .collect(),
    )
}

fn sourced<T>(s: &Sourced<T>, value: Value, key: &str) -> Value {
    json!({
        "value": value,
        "source": source_name(s.source),
        "at": s.at.as_ref().map(ToString::to_string),
        "editable": EDITABLE_BAR_KEYS.contains(&key),
    })
}

fn error_json(e: Option<&FileError>) -> Value {
    e.map_or(Value::Null, |e| json!({"at": e.at, "message": e.message}))
}

fn bar_json(b: &BarSources, index: usize) -> Value {
    json!({
        "name": b.name.value,
        "index": index,
        "position": sourced(&b.position, json!(match b.position.value { Position::Top => "top", Position::Bottom => "bottom" }), "position"),
        "height": sourced(&b.height, json!(b.height.value), "height"),
        "floating": sourced(&b.floating, json!(b.floating.value), "floating"),
        "left": sourced(&b.left, zone_value(&b.left.value), "left"),
        "center": sourced(&b.center, zone_value(&b.center.value), "center"),
        "right": sourced(&b.right, zone_value(&b.right.value), "right"),
        "center_on": sourced(&b.center_on, json!(b.center_on.value), "center_on"),
        "monitors": sourced(&b.monitors, monitors_value(&b.monitors.value), "monitors"),
        "margin": sourced(&b.margin, json!(b.margin.value), "margin"),
        "radius": sourced(&b.radius, json!(b.radius.value), "radius"),
        "reserve": sourced(&b.reserve, json!(b.reserve.value), "reserve"),
        "fullscreen": sourced(&b.fullscreen, json!(match b.fullscreen.value {
            FullscreenMode::Below => "below",
            FullscreenMode::Hide => "hide",
            FullscreenMode::Stay => "stay",
        }), "fullscreen"),
    })
}

fn scheme_accent_json(a: &SchemeAccent) -> Value {
    json!({
        "value": a.value,
        "source": source_name(a.source),
        "at": a.at,
        "shadowed_by": a.shadowed_by,
        "overridden": a.overridden,
    })
}

fn accent_json(a: &AccentView) -> Value {
    json!({"accent": {
        "label": "Accent",
        "editable": true,
        "dark": scheme_accent_json(&a.dark),
        "light": scheme_accent_json(&a.light),
    }})
}

/// The bars a request names: all, or the one whose name (first) or index
/// matches.
fn pick_bars<'a>(
    sources: &'a ConfigSources,
    want: Option<&str>,
) -> Result<Vec<(usize, &'a BarSources)>, SettingsError> {
    let all = sources.bars.iter().enumerate();
    let Some(want) = want else { return Ok(all.collect()) };
    let by_name: Vec<_> = sources.bars.iter().enumerate().filter(|(_, b)| b.name.value == want).collect();
    if !by_name.is_empty() {
        return Ok(by_name);
    }
    if let Ok(i) = want.parse::<usize>()
        && let Some(b) = sources.bars.get(i)
    {
        return Ok(vec![(i, b)]);
    }
    let names: Vec<&str> = sources.bars.iter().map(|b| b.name.value.as_str()).collect();
    Err(SettingsError::new(ErrorCode::UnknownBar, format!("no bar called '{want}' (bars: {})", names.join(", ")))
        .with_next("pick one of the bars listed"))
}

/// The state of the files, shared by `get` and the change result.
pub fn edited_by_hand(loaded: &Loaded, file: ConfigFile, written: Option<rsb_core::contract::FileHash>) -> bool {
    match (written, loaded.file(file).hash) {
        (Some(w), Some(h)) => w != h,
        _ => false,
    }
}

pub fn get(req: &Request, ctx: &Ctx, be: &dyn Backend) -> Reply {
    let loaded = be.load(&ctx.config_dir, true);
    let written = be.written(&ctx.config_dir);
    let mut files = serde_json::Map::new();
    for file in ConfigFile::ALL {
        let w = match file {
            ConfigFile::BarToml => written.bar_toml,
            ConfigFile::StyleCss => written.style_css,
        };
        files.insert(
            file.name().into(),
            json!({
                "hash": loaded.file(file).hash.map(|h| h.to_string()),
                "error": error_json(loaded.error(file)),
                "edited_by_hand": edited_by_hand(&loaded, file, w),
                "last_good": be.has_last_good(&ctx.config_dir, file),
            }),
        );
    }
    let (preset, bars) = match &loaded.sources {
        Some(s) => {
            let picked = match pick_bars(s, req.bar.as_deref()) {
                Ok(p) => p,
                Err(e) => return Reply::Err(e),
            };
            let at = s.preset.at.as_ref().map(ToString::to_string);
            (
                json!({"value": s.preset.value, "source": source_name(s.preset.source), "at": at}),
                Value::Array(picked.into_iter().map(|(i, b)| bar_json(b, i)).collect()),
            )
        }
        None => (Value::Null, Value::Null),
    };
    let hashes = [(ConfigFile::BarToml, loaded.bar_toml.hash), (ConfigFile::StyleCss, loaded.style_css.hash)];
    let bar = bar::get_json(&be.bar(ctx), ctx, &hashes);
    Reply::Ok(json!({
        "kind": "settings",
        "config_dir": ctx.config_dir.to_string_lossy(),
        "files": files,
        "preset": preset,
        "bars": bars,
        "colors": loaded.accent.as_ref().map_or(Value::Null, accent_json),
        "high_contrast": be.high_contrast(),
        "bar": bar,
    }))
}

fn tune_json(t: &TuneView) -> Value {
    json!({"var": t.var, "label": t.label, "editable": t.editable, "dark": t.dark, "light": t.light})
}

fn preset_json(p: &PresetView) -> Value {
    let contrast = serde_json::to_value(&p.contrast).unwrap_or(Value::Null);
    json!({
        "name": p.name,
        "title": p.title,
        "description": p.description,
        "schemes": p.schemes.iter().map(|s| s.name()).collect::<Vec<_>>(),
        "bars": p.bars,
        "bar": {"position": p.position, "height": p.height, "floating": p.floating},
        "tune": p.tune.iter().map(tune_json).collect::<Vec<_>>(),
        "contrast": contrast,
    })
}

pub fn presets(ctx: &Ctx, be: &dyn Backend) -> Reply {
    let loaded = be.load(&ctx.config_dir, false);
    let presets = match be.presets(&ctx.config_dir) {
        Ok(p) => p,
        Err(e) => return Reply::Err(e),
    };
    Reply::Ok(json!({
        "kind": "presets",
        "current": loaded.sources.as_ref().map(|s| s.preset.value.clone()),
        "high_contrast": be.high_contrast(),
        "presets": presets.iter().map(preset_json).collect::<Vec<_>>(),
    }))
}

pub fn schema_reply() -> Reply {
    Reply::Ok(schema::schema())
}

pub fn log(req: &Request, ctx: &Ctx, be: &dyn Backend) -> Reply {
    let last = req.last.map_or(usize::MAX, |n| n as usize);
    let entries = match be.log_entries(&ctx.config_dir, last) {
        Ok(e) => e,
        Err(e) => return Reply::Err(e),
    };
    let rows: Vec<Value> = entries
        .iter()
        .map(|e| {
            let files: BTreeMap<&str, Value> = e
                .files
                .iter()
                .map(|(f, before, after)| (f.name(), json!({"before": before.to_string(), "after": after.to_string()})))
                .collect();
            json!({
                "n": e.n,
                "time": e.time,
                "what": e.what,
                "changes": e.changes.iter().map(|c| serde_json::to_value(c).unwrap_or(Value::Null)).collect::<Vec<_>>(),
                "files": files,
            })
        })
        .collect();
    Reply::Ok(json!({"kind": "log", "entries": rows}))
}

/// Where each widget is placed in the effective config.
fn used_by_name(sources: Option<&ConfigSources>) -> BTreeMap<String, Vec<Value>> {
    let mut used: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let Some(s) = sources else { return used };
    for b in &s.bars {
        for (zone, items) in [("left", &b.left.value), ("center", &b.center.value), ("right", &b.right.value)] {
            for (index, item) in items.iter().enumerate() {
                used.entry(item.widget.name.clone())
                    .or_default()
                    .push(json!({"bar": b.name.value, "zone": zone, "index": index, "ref": ref_text(&item.widget)}));
            }
        }
    }
    used
}

pub fn widgets(ctx: &Ctx, be: &dyn Backend) -> Reply {
    let listing = be.widgets(&ctx.config_dir);
    let loaded = be.load(&ctx.config_dir, false);
    let used = used_by_name(loaded.sources.as_ref());
    let rows: Vec<Value> = listing
        .widgets
        .iter()
        .map(|w| {
            let about = &w.about;
            json!({
                "name": w.name,
                "source": if w.builtin { "builtin" } else { "user" },
                "replaces_builtin": w.replaces_builtin,
                "title": about["title"],
                "visible": about["visible"],
                "needs_options": about["needs_options"],
                "opens": about["opens"],
                "summary": about["summary"],
                "options": about["options"],
                "used": used.get(&w.name).cloned().unwrap_or_default(),
            })
        })
        .collect();
    let failed: Vec<Value> =
        listing.failed.iter().map(|f| json!({"name": f.name, "source": f.source, "message": f.message})).collect();
    Reply::Ok(json!({"kind": "widgets", "widgets": rows, "failed": failed}))
}

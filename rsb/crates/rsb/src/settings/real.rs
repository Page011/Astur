//! The real wiring behind `Backend`: the files, the edit engine, the writer,
//! the contrast checks, the preview renderer, the script sandbox and the
//! pipe to the bar.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use rsb_core::cfgedit::{self, Conflicts, FileTexts, Plan, PlanContext};
use rsb_core::color::Color;
use rsb_core::config::{BUILTIN_PRESETS, DEFAULT_PRESET, Position, WidgetRef, load_config, load_config_sourced};
use rsb_core::contract::{AppliedReply, ConfigFile, ErrorCode, Op, PresetContrast, SettingsError, Source};
use rsb_core::contrast::{self, AccentSurfaces, Layer, Vars};
use rsb_core::css::Stylesheet;
use rsb_core::diag::{Diagnostic, Severity};
use rsb_core::ipc::{self, Command};
use rsb_core::paths::{MAX_SOURCE_BYTES, widget_files};
use rsb_core::preset::{About, Scheme, parse_about};

use super::backend::*;
use crate::cfgload;
use crate::cfgwrite::{self, Applied, LogEntry, WriteOpts, Written};
use crate::pipe::{self, SendError};
use crate::preview::{self, Preview, PreviewOpts};
use crate::settings::Ctx;
use crate::{assets, settings::widgets};

pub struct Real;

fn internal(message: impl Into<String>) -> SettingsError {
    SettingsError::new(ErrorCode::Internal, message).with_next("report it with rsb.log")
}

/// `#rrggbb`, or `#rrggbbaa` for a translucent colour.
pub fn hex_of(c: Color) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let (r, g, b, a) = (byte(c.r), byte(c.g), byte(c.b), byte(c.a));
    if a == 255 { format!("#{r:02x}{g:02x}{b:02x}") } else { format!("#{r:02x}{g:02x}{b:02x}{a:02x}") }
}

fn read_file(dir: &Path, file: ConfigFile) -> FileRead {
    match cfgload::read_bytes_capped_hashed(&dir.join(file.name()), MAX_SOURCE_BYTES) {
        Ok(None) => FileRead::missing(),
        Ok(Some((bytes, hash))) => {
            let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(&bytes);
            match std::str::from_utf8(body) {
                Ok(text) => FileRead { hash: Some(hash), text: Some(text.to_string()), utf8: true, unreadable: None },
                Err(_) => FileRead {
                    hash: Some(hash),
                    text: Some(String::from_utf8_lossy(body).into_owned()),
                    utf8: false,
                    unreadable: None,
                },
            }
        }
        Err(e) => FileRead { hash: None, text: None, utf8: true, unreadable: Some(e) },
    }
}

fn layer_source(layer: Layer) -> Source {
    match layer {
        Layer::Base => Source::Default,
        Layer::Preset => Source::Preset,
        Layer::User => Source::User,
    }
}

fn scheme_accent(sheet: &Stylesheet, scheme: Scheme, conflicts: Option<&Conflicts>) -> SchemeAccent {
    let vars = contrast::resolve_vars(sheet, scheme);
    let mut out =
        SchemeAccent { value: None, source: Source::Default, at: None, shadowed_by: None, overridden: Vec::new() };
    if let Some((color, origin)) = vars.get("accent") {
        out.value = Some(hex_of(*color));
        out.source = layer_source(origin.layer);
        if origin.layer == Layer::User {
            out.at = Some(origin.pos.to_string());
        }
    }
    // The same places the write names in its notices: one function reads them.
    if let Some(c) = conflicts {
        out.shadowed_by = c.shadowed.first().map(|c| c.at.clone());
        out.overridden = c.replaced.iter().map(|c| c.at.clone()).collect();
    }
    out
}

fn accent_view(sheet: &Stylesheet, style_text: Option<&str>) -> AccentView {
    let conflicts = style_text.and_then(|t| cfgedit::accent_conflicts(sheet, t));
    AccentView {
        dark: scheme_accent(sheet, Scheme::Dark, conflicts.as_ref()),
        light: scheme_accent(sheet, Scheme::Light, conflicts.as_ref()),
    }
}

fn first_error(diags: &[Diagnostic], wanted: impl Fn(&str) -> bool) -> Option<FileError> {
    diags.iter().filter(|d| d.severity == Severity::Error).find(|d| d.pos.as_ref().is_none_or(|p| wanted(&p.file))).map(
        |d| FileError { at: d.pos.as_ref().map(|p| p.to_string()).unwrap_or_default(), message: d.message.clone() },
    )
}

fn preset_text(name: &str) -> Option<String> {
    assets::preset_toml(name).map(str::to_string)
}

/// What a built-in preset defines: its metadata and the colours of each
/// scheme it lists.
struct PresetColors {
    about: About,
    dark: Option<Vars>,
    light: Option<Vars>,
}

impl PresetColors {
    fn vars(&self, scheme: Scheme) -> Option<&Vars> {
        match scheme {
            Scheme::Dark => self.dark.as_ref(),
            Scheme::Light => self.light.as_ref(),
        }
    }
}

fn preset_colors(dir: &Path, name: &str) -> Result<PresetColors, SettingsError> {
    let toml = assets::preset_toml(name).ok_or_else(|| internal(format!("no preset {name}")))?;
    let about = parse_about(toml, "preset.toml").map_err(|d| internal(format!("preset {name}: {}", d.message)))?;
    let built = cfgload::build(dir, Some(name), name);
    let sheet = built.sheet.ok_or_else(|| internal(format!("the style of preset {name} does not parse")))?;
    let pick = |scheme: Scheme| about.schemes.contains(&scheme).then(|| contrast::resolve_vars(&sheet, scheme));
    let (dark, light) = (pick(Scheme::Dark), pick(Scheme::Light));
    Ok(PresetColors { about, dark, light })
}

impl Backend for Real {
    fn load(&self, dir: &Path, with_accent: bool) -> Loaded {
        let bar_toml = read_file(dir, ConfigFile::BarToml);
        let style_css = read_file(dir, ConfigFile::StyleCss);
        let user = bar_toml.text.as_deref().map(|t| (t, "bar.toml"));
        let sourced = load_config_sourced(user, &preset_text);
        let built = cfgload::build(dir, None, DEFAULT_PRESET);
        let mut bar_error = first_error(&sourced.load.diagnostics, |f| f == "bar.toml");
        if let Some(why) = &bar_toml.unreadable {
            bar_error = Some(FileError { at: "bar.toml".into(), message: format!("cannot read the file: {why}") });
        }
        let mut style_error = first_error(&built.diagnostics, |f| f != "bar.toml");
        if let Some(why) = &style_css.unreadable {
            style_error = Some(FileError { at: "style.css".into(), message: format!("cannot read the file: {why}") });
        }
        // No sheet because bar.toml could not be read is bar.toml's trouble, not style.css's.
        if style_error.is_none() && built.sheet.is_none() && bar_error.is_none() {
            style_error = Some(FileError { at: "style.css".into(), message: "the style sheet cannot be used".into() });
        }
        let accent = match (&style_error, &built.sheet) {
            (None, Some(sheet)) if with_accent => Some(accent_view(sheet, style_css.text.as_deref())),
            _ => None,
        };
        Loaded {
            sources: if bar_error.is_some() { None } else { sourced.sources },
            bar_toml,
            style_css,
            bar_error,
            style_error,
            accent,
        }
    }

    fn plan(
        &self,
        dir: &Path,
        texts: &FileTexts,
        ops: &[Op],
        reset: bool,
        popups: bool,
    ) -> Result<Plan, SettingsError> {
        // Unset and reset judge the colours block against the shades its own accent gives, so they need the surfaces too.
        let wants_accent =
            reset || ops.iter().any(|o| matches!(o, Op::Set { key, .. } | Op::Unset { key } if key == "color.accent"));
        let none = AccentSurfaces::default();
        let surfaces = if wants_accent { assets::accent_surfaces() } else { &none };
        let builtin = assets::builtin_widget_names();
        let exists = |r: &str| {
            WidgetRef::parse(r).is_ok_and(|w| {
                builtin.contains(&w.name) || widget_files(&w).iter().any(|f| dir.join(f.replace('/', "\\")).is_file())
            })
        };
        let cache: RefCell<HashMap<String, Vec<String>>> = RefCell::new(HashMap::new());
        let opens = |r: &str| -> Vec<String> {
            let Ok(w) = WidgetRef::parse(r) else { return Vec::new() };
            if !popups {
                return Vec::new();
            }
            if let Some(hit) = cache.borrow().get(&w.name) {
                return hit.clone();
            }
            let found = widgets::opens_of(dir, &w.name);
            cache.borrow_mut().insert(w.name, found.clone());
            found
        };
        let style_chain = |text: &str| Some(cfgload::style_chain_of(dir, text));
        let template = |f: ConfigFile| {
            let rel = match f {
                ConfigFile::BarToml => "config/bar.toml",
                ConfigFile::StyleCss => "config/style.css",
            };
            assets::file(rel).unwrap_or("").to_string()
        };
        let ctx = PlanContext {
            reset,
            preset_toml: &preset_text,
            widget_exists: &exists,
            widget_opens: &opens,
            template: &template,
            surfaces,
            style_chain: &style_chain,
        };
        cfgedit::plan_change(texts, ops, &ctx)
    }

    fn apply(&self, dir: &Path, plan: &Plan, opts: &WriteOpts) -> Result<Applied, SettingsError> {
        cfgwrite::apply(dir, plan, opts)
    }

    fn undo(&self, dir: &Path, n: Option<u32>, opts: &WriteOpts) -> Result<Applied, SettingsError> {
        cfgwrite::undo(dir, n, opts)
    }

    fn restore(&self, dir: &Path, file: ConfigFile, opts: &WriteOpts) -> Result<Applied, SettingsError> {
        cfgwrite::restore(dir, file, opts)
    }

    fn log_entries(&self, dir: &Path, last: usize) -> Result<Vec<LogEntry>, SettingsError> {
        cfgwrite::log_entries(dir, last)
    }

    fn written(&self, dir: &Path) -> Written {
        cfgwrite::written(dir)
    }

    fn has_last_good(&self, dir: &Path, file: ConfigFile) -> bool {
        cfgwrite::has_last_good(dir, file)
    }

    fn append_log(&self, log_dir: &Path, message: &str) {
        cfgwrite::append_log_line(log_dir, message);
    }

    fn presets(&self, dir: &Path) -> Result<Vec<PresetView>, SettingsError> {
        let mut out = Vec::new();
        for name in BUILTIN_PRESETS {
            let colors = preset_colors(dir, name)?;
            let mut contrast_of = PresetContrast::default();
            for scheme in Scheme::ALL {
                let Some(vars) = colors.vars(scheme) else { continue };
                let summary = contrast::summary(&contrast::pairs(&colors.about, vars));
                match scheme {
                    Scheme::Dark => contrast_of.dark = Some(summary),
                    Scheme::Light => contrast_of.light = Some(summary),
                }
            }
            let hex_in =
                |scheme: Scheme, var: &str| colors.vars(scheme).and_then(|v| v.get(var)).map(|(c, _)| hex_of(*c));
            let tune = colors
                .about
                .tune
                .iter()
                .map(|t| TuneView {
                    var: t.var.clone(),
                    label: t.label.clone(),
                    editable: t.var == "accent",
                    dark: hex_in(Scheme::Dark, &t.var),
                    light: hex_in(Scheme::Light, &t.var),
                })
                .collect();
            let user = format!("preset = \"{name}\"\n");
            let config = load_config(Some((&user, "bar.toml")), &preset_text)
                .config
                .ok_or_else(|| internal(format!("preset {name} does not load")))?;
            let first = config.bars.first().ok_or_else(|| internal(format!("preset {name} has no bar")))?;
            out.push(PresetView {
                name: (*name).to_string(),
                title: colors.about.title.clone(),
                description: colors.about.description.clone(),
                schemes: colors.about.schemes.clone(),
                bars: config.bars.len() as u32,
                position: match first.position {
                    Position::Top => "top".into(),
                    Position::Bottom => "bottom".into(),
                },
                height: first.height,
                floating: first.floating,
                tune,
                contrast: contrast_of,
            });
        }
        Ok(out)
    }

    fn widgets(&self, dir: &Path) -> Listing {
        widgets::list(dir)
    }

    fn previews(&self, presets: &[String], opts: &PreviewOpts) -> Result<Vec<Preview>, SettingsError> {
        preview::render(presets, opts)
    }

    fn bar(&self, ctx: &Ctx) -> BarState {
        if let Some(applied) = &ctx.applied {
            return BarState::Running(applied.reply());
        }
        match pipe::send(&Command::Applied, Duration::from_secs(1)) {
            Ok(ipc::Reply::Ok(text)) => {
                serde_json::from_str::<AppliedReply>(&text).map_or(BarState::Unreachable, BarState::Running)
            }
            Ok(ipc::Reply::Err(_)) => BarState::Unreachable,
            Err(SendError::NoBar) => BarState::NotRunning,
            Err(SendError::Refused(_)) => BarState::Unreachable,
        }
    }

    fn high_contrast(&self) -> bool {
        rsb_win::hicontrast::is_on()
    }

    fn now_ms(&self) -> u64 {
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_millis() as u64
    }

    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsb_core::diag::SourcePos;

    fn dir(tag: &str) -> std::path::PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target")
            .join("test-tmp")
            .join(format!("real-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_colour_is_written_as_hex_with_alpha_only_when_translucent() {
        assert_eq!(hex_of(Color::rgba8(0xe5, 0xa5, 0x0a, 255)), "#e5a50a");
        assert_eq!(hex_of(Color::BLACK), "#000000");
        assert_eq!(hex_of(Color::WHITE), "#ffffff");
        assert_eq!(hex_of(Color::rgba8(1, 2, 3, 128)), "#01020380");
    }

    #[test]
    fn a_file_is_read_with_its_version_and_its_encoding_noted() {
        let d = dir("read");
        let none = read_file(&d, ConfigFile::BarToml);
        assert_eq!(
            (none.hash, none.text.clone(), none.utf8),
            (Some(rsb_core::contract::FileHash::Missing), None, true)
        );
        std::fs::write(d.join("bar.toml"), "preset = \"glass\"\n").unwrap();
        let plain = read_file(&d, ConfigFile::BarToml);
        assert_eq!(plain.text.as_deref(), Some("preset = \"glass\"\n"));
        assert_eq!(plain.hash, Some(rsb_core::contract::FileHash::of(b"preset = \"glass\"\n")));
        // A byte order mark is part of the version but not of the text.
        std::fs::write(d.join("bar.toml"), b"\xEF\xBB\xBFpreset = \"glass\"\n").unwrap();
        let bom = read_file(&d, ConfigFile::BarToml);
        assert_eq!(bom.text.as_deref(), Some("preset = \"glass\"\n"));
        assert_ne!(bom.hash, plain.hash);
        assert_eq!(bom.hash, Some(rsb_core::contract::FileHash::of(b"\xEF\xBB\xBFpreset = \"glass\"\n")));
        // UTF-16 and invalid bytes are flagged, never silently lossy.
        std::fs::write(d.join("bar.toml"), b"\xFF\xFEp\0r\0").unwrap();
        let wide = read_file(&d, ConfigFile::BarToml);
        assert!(!wide.utf8 && wide.unreadable.is_none());
        // Too large to read at all.
        std::fs::write(d.join("style.css"), vec![b'x'; MAX_SOURCE_BYTES + 1]).unwrap();
        let big = read_file(&d, ConfigFile::StyleCss);
        assert!(big.unreadable.is_some() && big.hash.is_none() && big.text.is_none());
    }

    #[test]
    fn an_error_is_taken_from_the_diagnostics_of_the_file_it_belongs_to() {
        use rsb_core::diag::{DiagKind, Diagnostic};
        let diags = vec![
            Diagnostic::warning(DiagKind::Config, Some(SourcePos::new("bar.toml", 2, 1)), "just a warning"),
            Diagnostic::error(DiagKind::Style, Some(SourcePos::new("style.css", 3, 5)), "stray }"),
            Diagnostic::error(DiagKind::Config, Some(SourcePos::new("bar.toml", 7, 3)), "missing closing quote"),
        ];
        let bar = first_error(&diags, |f| f == "bar.toml").unwrap();
        assert_eq!((bar.at.as_str(), bar.message.as_str()), ("bar.toml:7:3", "missing closing quote"));
        let style = first_error(&diags, |f| f != "bar.toml").unwrap();
        assert_eq!(style.at, "style.css:3:5");
        assert!(first_error(&diags[..1], |_| true).is_none(), "a warning is not an error");
    }
}

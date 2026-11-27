//! Preset previews: a PNG of each preset's bar in each colour scheme, drawn
//! by the same path as `rsb render`, with sample values and a held clock so
//! the picture is stable. Rendered on demand and kept in a cache folder, so
//! a page opens warm.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use rsb_core::config::BUILTIN_PRESETS;
use rsb_core::contract::{ConfigFile, ErrorCode, FileHash, SettingsError, fixes};
use rsb_core::fnv::Fnv1a64;
use rsb_core::paths::{MAX_SOURCE_BYTES, WIDGETS_DIR};
use rsb_core::preset::Scheme;
use rsb_core::time::LocalTime;

use crate::args::RenderOpts;
use crate::cfgload::{self, Built};
use crate::{assets, paths, render_cmd};

/// The clock every preview shows: 10:42 on a Wednesday.
const PREVIEW_NOW: (i32, u8, u8, u8, u8) = (2026, 11, 25, 10, 42);
/// Most pictures drawn at the same time.
const MAX_PARALLEL: usize = 8;
/// A PNG ends with this chunk: a file that does not is cut short.
const PNG_END: [u8; 12] = [0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82];
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// What to draw and where to keep it.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewOpts {
    /// The user's config folder: read for `mine`, and the place the cache
    /// key looks at.
    pub config_dir: PathBuf,
    /// `--cache-dir`, else `%LOCALAPPDATA%\rsb\previews`.
    pub cache_dir: Option<PathBuf>,
    /// The schemes to draw, in order.
    pub themes: Vec<Scheme>,
    /// Width of the picture in pixels, as `rsb render --width`.
    pub width: u32,
    pub scale: f32,
    /// The user's `bar.toml` and `style.css` with only the preset swapped:
    /// what switching would show. Otherwise the preset alone, as shipped.
    pub mine: bool,
    /// With `mine`: the zone lists are the preset's own, the rest (accent,
    /// height, floating, ...) the user's. What switching to the preset and
    /// choosing its layout would show.
    pub zones_from_preset: bool,
}

/// One drawn preview.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub name: String,
    pub theme: Scheme,
    pub mine: bool,
    pub zones_from_preset: bool,
    pub path: PathBuf,
    /// The PNG's size in pixels.
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    /// Read from the cache instead of drawn.
    pub cached: bool,
}

/// The folder previews are kept in when no `--cache-dir` is given.
pub fn default_cache_dir() -> PathBuf {
    let logs = paths::log_dir(None);
    logs.parent().unwrap_or(&logs).join("previews")
}

/// The key a cached picture is filed under: everything the picture depends
/// on that its file name does not already say. `user_files` (the files the
/// user's folder contributed, with their versions) is None for the preset as
/// shipped.
pub fn cache_key(preset: &str, user_files: Option<&[(String, FileHash)]>) -> Option<u64> {
    let mut h = Fnv1a64::default();
    let mut piece = |bytes: &[u8]| {
        h.write(&(bytes.len() as u64).to_le_bytes());
        h.write(bytes);
    };
    piece(env!("CARGO_PKG_VERSION").as_bytes());
    piece(assets::base_css().as_bytes());
    piece(assets::preset_toml(preset)?.as_bytes());
    piece(assets::preset_css(preset)?.as_bytes());
    match user_files {
        None => piece(b"shipped"),
        Some(files) => {
            piece(b"mine");
            for (name, hash) in files {
                piece(name.as_bytes());
                piece(hash.to_string().as_bytes());
            }
        }
    }
    Some(h.finish())
}

/// Most widget files looked at, and how deep, so a stray folder cannot make
/// a preview slow.
const MAX_WIDGET_FILES: usize = 256;
const MAX_WIDGET_DEPTH: usize = 3;

/// Every `.luau` file under the user's `widgets/` folder (helpers in
/// `widgets/lib/` included) with its version, sorted by name, as the bar
/// reads them. A `--mine` picture draws with these, so they belong in its
/// key; a file too large to read counts as missing.
fn widget_files(config_dir: &Path) -> Vec<(String, FileHash)> {
    fn walk(dir: &Path, rel: &str, depth: usize, out: &mut Vec<(String, FileHash)>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            if out.len() >= MAX_WIDGET_FILES {
                return;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let rel_name = format!("{rel}/{name}");
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() && depth < MAX_WIDGET_DEPTH {
                walk(&entry.path(), &rel_name, depth + 1, out);
            } else if kind.is_file() && name.ends_with(".luau") {
                let hash = cfgload::read_bytes_capped_hashed(&entry.path(), MAX_SOURCE_BYTES)
                    .ok()
                    .flatten()
                    .map_or(FileHash::Missing, |(_, h)| h);
                out.push((rel_name, hash));
            }
        }
    }
    let mut out = Vec::new();
    walk(&config_dir.join(WIDGETS_DIR), WIDGETS_DIR, 1, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The files a `--mine` picture depends on: the build's own list (`bar.toml`,
/// `style.css` and its imports) and the user's widget scripts.
fn mine_files(config_dir: &Path, built: &Built) -> Vec<(String, FileHash)> {
    let mut files = built.user_files.clone();
    files.extend(widget_files(config_dir));
    files
}

/// The refusal for a file of the user's that does not parse, told as a write tells it: the file it is (`style.css`
/// for one that it pulls in) and where to open the file named at the place.
fn broken_file(dir: &Path, d: &rsb_core::diag::Diagnostic) -> SettingsError {
    let at = d.pos.as_ref().map_or(String::new(), |p| p.to_string());
    let mut e = SettingsError::new(ErrorCode::FileBroken, format!("{at}: {}", d.message)).with_at(at.clone());
    e.next = Some("fix that line by hand, then ask again".to_string());
    let Some(name) = at.split(':').next().filter(|n| !n.is_empty()) else { return e };
    e = e.with_file(if name == rsb_core::paths::BAR_TOML { ConfigFile::BarToml } else { ConfigFile::StyleCss });
    if name != e.file.map_or("", ConfigFile::name) {
        e.next = Some(format!("fix that line in {at}, then ask again"));
    }
    e.fix = Some(Box::new(fixes::open(&crate::cfgwrite::shown_path(&dir.join(name)))));
    e
}

/// The user's build with each bar's zone lists taken from the preset's own bars.
fn with_preset_zones(built: &mut Built, preset: &str) {
    let text = format!("preset = \"{preset}\"\n");
    let shipped =
        rsb_core::config::load_config(Some((&text, "bar.toml")), &|n| assets::preset_toml(n).map(str::to_string));
    let (Some(config), Some(own)) = (built.config.as_mut(), shipped.config) else { return };
    for (bar, theirs) in config.bars.iter_mut().zip(own.bars) {
        bar.left = theirs.left;
        bar.center = theirs.center;
        bar.right = theirs.right;
    }
}

/// `glass-dark-1920x100-` or `glass-mine-dark-1920x125-`: the part of a file
/// name before the key.
fn file_prefix(name: &str, theme: Scheme, mine: bool, zones_from_preset: bool, width: u32, scale: f32) -> String {
    let kind = match (mine, zones_from_preset) {
        (false, _) => "",
        (true, false) => "-mine",
        (true, true) => "-mine-zones",
    };
    format!("{name}{kind}-{}-{width}x{}-", theme.name(), (scale * 100.0).round() as u32)
}

/// Width and height of the PNG at `path` when it is whole (signature, header
/// and closing chunk), else None.
fn png_size(path: &Path) -> Option<(u32, u32)> {
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len < 33 + PNG_END.len() as u64 {
        return None;
    }
    let mut head = [0u8; 24];
    file.read_exact(&mut head).ok()?;
    if head[..8] != PNG_SIGNATURE || &head[12..16] != b"IHDR" {
        return None;
    }
    let mut tail = [0u8; 12];
    file.seek(SeekFrom::End(-(PNG_END.len() as i64))).ok()?;
    file.read_exact(&mut tail).ok()?;
    if tail != PNG_END {
        return None;
    }
    let be = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    Some((be(&head[16..20]), be(&head[20..24])))
}

/// Writes `bytes` to `target` through a temporary name, so a reader never
/// sees half a file.
fn write_atomically(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let name = target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let temp = target.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Removes the earlier pictures of the same preset, scheme and size: the
/// names that are `prefix`, 16 hex digits and `.png`, except `keep`. Anything
/// else in the folder is left alone.
fn remove_older(dir: &Path, prefix: &str, keep: &str) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let old = name
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix(".png"))
            .is_some_and(|key| key.len() == 16 && key.bytes().all(|b| b.is_ascii_hexdigit()));
        if old && name != keep {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn preview_clock() -> LocalTime {
    let (year, month, day, hour, minute) = PREVIEW_NOW;
    let t = LocalTime { year, month, day, hour, minute, ..Default::default() };
    LocalTime::from_epoch_ms(t.to_epoch_ms(), 0)
}

/// One picture to find or draw.
struct Job {
    index: usize,
    name: String,
    theme: Scheme,
    path: PathBuf,
    prefix: String,
    built: std::sync::Arc<Built>,
}

fn draw_job(job: &Job, opts: &PreviewOpts, widget_dir: &Path) -> Result<(u32, u32), SettingsError> {
    let render_opts = RenderOpts {
        out: PathBuf::new(),
        width: opts.width,
        scale: opts.scale,
        dark: job.theme == Scheme::Dark,
        preset: None,
        live: false,
        wait_ms: None,
        now: Some(preview_clock()),
    };
    let failed = |why: String| {
        SettingsError::new(ErrorCode::RenderFailed, format!("could not draw the {} preview: {why}", job.name))
            .with_next(format!("run: rsb render --preset {} --out preview.png", job.name))
    };
    let drawn = render_cmd::draw(widget_dir, &job.built, &render_opts).map_err(failed)?;
    let dir = job.path.parent().unwrap_or(Path::new("."));
    write_atomically(&job.path, &drawn.png).map_err(|e| failed(format!("the picture could not be saved: {e}")))?;
    let file = job.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    remove_older(dir, &job.prefix, &file);
    Ok((drawn.width, drawn.height))
}

/// Draws (or finds in the cache) `presets` in every scheme of `opts`. The
/// cache file name holds the preset, scheme, size and a key over everything
/// the picture depends on; older files of the same name and size are removed
/// after a new one is written. Errors: `unknown_preset`, `render_failed`.
pub fn render(presets: &[String], opts: &PreviewOpts) -> Result<Vec<Preview>, SettingsError> {
    for p in presets {
        if !BUILTIN_PRESETS.contains(&p.as_str()) {
            return Err(SettingsError::new(
                ErrorCode::UnknownPreset,
                format!("unknown preset '{p}' (presets: {})", BUILTIN_PRESETS.join(", ")),
            ));
        }
    }
    if opts.width == 0 || opts.width > 16384 || !(opts.scale > 0.0 && opts.scale <= 8.0) {
        return Err(SettingsError::new(ErrorCode::BadValue, "the preview width or scale is out of range"));
    }
    let cache = opts.cache_dir.clone().unwrap_or_else(default_cache_dir);
    fs::create_dir_all(&cache).map_err(|e| {
        SettingsError::new(ErrorCode::RenderFailed, format!("the preview folder could not be made: {e}"))
            .with_next("pass --cache-dir with a folder you can write to")
    })?;
    // Shipped previews never read the user's folder, widgets included.
    let widget_dir = if opts.mine { opts.config_dir.clone() } else { cache.join("no-user-widgets") };

    let mut found: Vec<Option<Preview>> = Vec::new();
    let mut jobs: Vec<Job> = Vec::new();
    for name in presets {
        let mut built = if opts.mine {
            cfgload::build_as(&opts.config_dir, name)
        } else {
            cfgload::build(&widget_dir, Some(name), "default")
        };
        if opts.mine {
            // A broken file would be drawn as an error plate with the preset's own colours.
            if let Some(d) = built.diagnostics.iter().find(|d| d.severity == rsb_core::diag::Severity::Error) {
                return Err(broken_file(&opts.config_dir, d));
            }
            if opts.zones_from_preset {
                with_preset_zones(&mut built, name);
            }
        }
        let built = std::sync::Arc::new(built);
        let mut mine = opts.mine.then(|| mine_files(&opts.config_dir, &built));
        if let (Some(files), true) = (mine.as_mut(), opts.zones_from_preset) {
            files.push(("zones=preset".to_string(), FileHash::Fnv(1)));
        }
        let key = cache_key(name, mine.as_deref())
            .ok_or_else(|| SettingsError::new(ErrorCode::Internal, "a built-in preset has no files"))?;
        for theme in &opts.themes {
            let prefix = file_prefix(name, *theme, opts.mine, opts.zones_from_preset, opts.width, opts.scale);
            let path = cache.join(format!("{prefix}{key:016x}.png"));
            let mut preview = Preview {
                name: name.clone(),
                theme: *theme,
                mine: opts.mine,
                zones_from_preset: opts.mine && opts.zones_from_preset,
                path: path.clone(),
                width: 0,
                height: 0,
                scale: opts.scale,
                cached: true,
            };
            // A file by that name that is not a picture of the size asked for is drawn again.
            match png_size(&path).filter(|(w, h)| *w == opts.width && (1..=16384).contains(h)) {
                Some((w, h)) => (preview.width, preview.height) = (w, h),
                None => {
                    preview.cached = false;
                    jobs.push(Job {
                        index: found.len(),
                        name: name.clone(),
                        theme: *theme,
                        path,
                        prefix,
                        built: built.clone(),
                    });
                }
            }
            found.push(Some(preview));
        }
    }
    for batch in jobs.chunks(MAX_PARALLEL) {
        let sizes: Vec<Result<(u32, u32), SettingsError>> = std::thread::scope(|scope| {
            let handles: Vec<_> = batch.iter().map(|job| scope.spawn(|| draw_job(job, opts, &widget_dir))).collect();
            handles
                .into_iter()
                .zip(batch)
                .map(|(h, job)| {
                    h.join().unwrap_or_else(|_| {
                        Err(SettingsError::new(
                            ErrorCode::RenderFailed,
                            format!("drawing the {} preview stopped unexpectedly", job.name),
                        ))
                    })
                })
                .collect()
        });
        for (job, size) in batch.iter().zip(sizes) {
            let (w, h) = size?;
            if let Some(p) = found[job.index].as_mut() {
                (p.width, p.height) = (w, h);
            }
        }
    }
    Ok(found.into_iter().flatten().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn write(dir: &TempDir, name: &str, text: &str) {
        std::fs::write(dir.path().join(name), text).unwrap();
    }

    fn key_of(dir: &TempDir, preset: &str) -> u64 {
        let built = cfgload::build_as(dir.path(), preset);
        cache_key(preset, Some(&mine_files(dir.path(), &built))).unwrap()
    }

    #[test]
    fn preview_cache_key_changes_with_user_file() {
        let dir = TempDir::new("pv");
        let empty = key_of(&dir, "glass");
        write(&dir, "style.css", ":root { --accent: #e5a50a; }\n");
        let styled = key_of(&dir, "glass");
        assert_ne!(empty, styled, "a new style.css changes the key");
        write(&dir, "style.css", ":root { --accent: #e5a50b; }\n");
        let edited = key_of(&dir, "glass");
        assert_ne!(styled, edited, "one changed byte changes the key");
        assert_eq!(edited, key_of(&dir, "glass"), "the same files give the same key");
        write(&dir, "style.css", "@import \"colors.css\";\n");
        let with_import = key_of(&dir, "glass");
        write(&dir, "colors.css", "bar { color: red }\n");
        let import_added = key_of(&dir, "glass");
        write(&dir, "colors.css", "bar { color: blue }\n");
        assert_ne!(with_import, import_added, "a file that an import finds now changes the key");
        assert_ne!(import_added, key_of(&dir, "glass"), "so does an edit to it");
        write(&dir, "bar.toml", "config_version = 1\n[[bar]]\nheight = 40\n");
        assert_ne!(import_added, key_of(&dir, "glass"), "bar.toml counts too");
        assert_ne!(key_of(&dir, "glass"), key_of(&dir, "fluent"), "each preset has its own key");
    }

    #[test]
    fn preview_cache_key_changes_with_a_user_widget_file() {
        let dir = TempDir::new("pv");
        let empty = key_of(&dir, "glass");
        std::fs::create_dir_all(dir.path().join("widgets/lib")).unwrap();
        write(
            &dir,
            "widgets/clock.luau",
            "return {}
",
        );
        let added = key_of(&dir, "glass");
        assert_ne!(empty, added, "a widget file of the user's changes the key");
        write(
            &dir,
            "widgets/clock.luau",
            "return {} -- x
",
        );
        let edited = key_of(&dir, "glass");
        assert_ne!(added, edited, "so does an edit to it");
        assert_eq!(edited, key_of(&dir, "glass"), "the same files give the same key");
        write(
            &dir,
            "widgets/lib/helper.luau",
            "return {}
",
        );
        let helper = key_of(&dir, "glass");
        assert_ne!(edited, helper, "a helper under lib counts");
        write(
            &dir,
            "widgets/notes.txt",
            "not a script
",
        );
        assert_eq!(helper, key_of(&dir, "glass"), "a file that is not a script does not");
    }

    #[test]
    fn the_shipped_key_ignores_the_users_files() {
        let dir = TempDir::new("pv");
        let shipped = cache_key("glass", None).unwrap();
        write(&dir, "style.css", ":root { --accent: #e5a50a; }\n");
        assert_eq!(shipped, cache_key("glass", None).unwrap());
        let built = cfgload::build_as(dir.path(), "glass");
        assert_ne!(
            shipped,
            cache_key("glass", Some(&mine_files(dir.path(), &built))).unwrap(),
            "mine and shipped never share a file"
        );
        assert_eq!(cache_key("no-such-preset", None), None);
    }

    #[test]
    fn file_names_say_preset_scheme_size_and_whose_files() {
        assert_eq!(file_prefix("glass", Scheme::Dark, false, false, 1920, 1.0), "glass-dark-1920x100-");
        assert_eq!(file_prefix("glass", Scheme::Light, true, false, 1920, 1.25), "glass-mine-light-1920x125-");
    }

    #[test]
    fn only_whole_files_of_the_right_name_are_cache_hits_and_older_ones_go() {
        let dir = TempDir::new("pv");
        let png = {
            let mut v = PNG_SIGNATURE.to_vec();
            v.extend([0, 0, 0, 13]);
            v.extend(b"IHDR");
            v.extend(1920u32.to_be_bytes());
            v.extend(36u32.to_be_bytes());
            v.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
            v.extend(PNG_END);
            v
        };
        let whole = dir.path().join("a.png");
        std::fs::write(&whole, &png).unwrap();
        assert_eq!(png_size(&whole), Some((1920, 36)));
        let cut = dir.path().join("cut.png");
        std::fs::write(&cut, &png[..png.len() - 5]).unwrap();
        assert_eq!(png_size(&cut), None, "a file cut short is drawn again");
        std::fs::write(dir.path().join("text.png"), b"not a picture at all, not a picture at all, no").unwrap();
        assert_eq!(png_size(&dir.path().join("text.png")), None);
        assert_eq!(png_size(&dir.path().join("none.png")), None);

        let prefix = "glass-dark-1920x100-";
        for name in [
            "glass-dark-1920x100-0123456789abcdef.png",
            "glass-dark-1920x100-fedcba9876543210.png",
            "glass-dark-1920x200-0123456789abcdef.png",
            "glass-mine-dark-1920x100-0123456789abcdef.png",
            "glass-dark-1920x100-notakey.png",
            "glass-dark-1920x100-0123456789abcdef.txt",
            "notes.png",
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        remove_older(dir.path(), prefix, "glass-dark-1920x100-fedcba9876543210.png");
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        left.sort();
        let mut expected = [
            "a.png",
            "cut.png",
            "glass-dark-1920x100-fedcba9876543210.png",
            "glass-dark-1920x100-notakey.png",
            "glass-dark-1920x100-0123456789abcdef.txt",
            "glass-dark-1920x200-0123456789abcdef.png",
            "glass-mine-dark-1920x100-0123456789abcdef.png",
            "notes.png",
            "text.png",
        ]
        .map(String::from);
        expected.sort();
        assert_eq!(left, expected, "only the older picture of the same preset, scheme and size went");
    }

    #[test]
    fn an_unknown_preset_or_size_is_refused_before_any_drawing() {
        let dir = TempDir::new("pv");
        let opts = PreviewOpts {
            config_dir: dir.path().to_path_buf(),
            cache_dir: Some(dir.path().join("cache")),
            themes: vec![Scheme::Dark],
            width: 1920,
            scale: 1.0,
            mine: false,
            zones_from_preset: false,
        };
        let err = render(&["nope".to_string()], &opts).unwrap_err();
        assert_eq!(err.code, ErrorCode::UnknownPreset);
        assert!(!dir.path().join("cache").exists(), "nothing was made");
        let err = render(&["glass".to_string()], &PreviewOpts { width: 0, ..opts.clone() }).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadValue);
        let err = render(&["glass".to_string()], &PreviewOpts { scale: f32::NAN, ..opts }).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadValue);
    }

    #[test]
    fn the_preview_clock_is_wednesday_1042() {
        let t = preview_clock();
        assert_eq!((t.year, t.month, t.day, t.hour, t.minute, t.second, t.weekday), (2026, 11, 25, 10, 42, 0, 3));
    }
}

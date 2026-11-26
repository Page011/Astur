//! Fonts. The bundled monospace face is registered at start. The files a
//! style sheet's `@font-face` rules name are read on the config reader
//! thread (never from outside the config folder's `fonts` folder, at most 16
//! files of at most 16 MiB) and registered with the renderer on the UI
//! thread; a file that cannot be used becomes a diagnostic for the bar plate
//! and the log, never a missing font that nobody hears about.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use rsb_core::css::{FONT_FILE_MAX_BYTES, FONT_FILES_MAX, FontFace};
use rsb_core::diag::{DiagKind, Diagnostic};
use rsb_render::Renderer;

use crate::assets;
use crate::cfgload::read_bytes_capped;

/// The embedded variable font every preset may name as "JetBrains Mono".
pub const BUNDLED_FONT: &str = "fonts/JetBrainsMono-wght.ttf";

/// Adds the bundled font to the renderer; returns the family names it
/// brought. Done once, before the first frame.
pub fn register_bundled(renderer: &mut Renderer) -> Result<Vec<String>, String> {
    let bytes = assets::bytes(BUNDLED_FONT).ok_or("the bundled font is not embedded")?;
    renderer.register_font(Arc::from(bytes), None).map_err(|e| e.to_string())
}

/// One `@font-face` rule and its file. `bytes` is None when the reader
/// already handed this exact file over (same size and time) with an earlier
/// reading: the UI keeps what it learned then.
#[derive(Clone)]
pub struct FontFile {
    pub face: FontFace,
    pub bytes: Option<Arc<[u8]>>,
}

impl std::fmt::Debug for FontFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontFile")
            .field("family", &self.face.family)
            .field("bytes", &self.bytes.as_ref().map(|b| b.len()))
            .finish()
    }
}

#[derive(Debug, Default)]
pub struct FontLoad {
    pub files: Vec<FontFile>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Which files (by name and family) were handed over, and what they looked
/// like then. Lives on the config reader thread, so a style edit does not
/// read and register up to 256 MiB of fonts again.
#[derive(Clone, Debug, Default)]
pub struct FontCache {
    sent: HashMap<(String, String), (u64, Option<SystemTime>)>,
}

/// The file inside `root` (the canonical `fonts` folder) a face names, only
/// when it really lives there: one plain file name, and after links are
/// resolved still under `root`.
fn inside(root: &Path, name: &str) -> Option<PathBuf> {
    let plain = !name.is_empty()
        && !name.contains(['\\', '/', ':'])
        && Path::new(name).components().all(|c| matches!(c, Component::Normal(_)));
    if !plain {
        return None;
    }
    let real = std::fs::canonicalize(root.join(name)).ok()?;
    real.starts_with(root).then_some(real)
}

impl FontCache {
    /// Reads the file of every face (in sheet order) from `dir\fonts`.
    pub fn load(&mut self, dir: &Path, faces: &[FontFace]) -> FontLoad {
        let mut out = FontLoad::default();
        if faces.is_empty() {
            self.sent.clear();
            return out;
        }
        let root = std::fs::canonicalize(dir.join("fonts")).ok();
        let mut next = HashMap::new();
        for (i, face) in faces.iter().enumerate() {
            if i >= FONT_FILES_MAX {
                out.diagnostics.push(face.over_the_limit());
                continue;
            }
            let name = face.file_name();
            let Some(path) = root.as_deref().and_then(|r| inside(r, name)) else {
                out.diagnostics.push(face.not_found());
                continue;
            };
            let meta = match std::fs::metadata(&path) {
                Ok(m) if m.is_file() => m,
                _ => {
                    out.diagnostics.push(face.not_found());
                    continue;
                }
            };
            if meta.len() > FONT_FILE_MAX_BYTES {
                out.diagnostics.push(face.too_large());
                continue;
            }
            let id = (meta.len(), meta.modified().ok());
            let key = (name.to_ascii_lowercase(), face.family.clone());
            if self.sent.get(&key) == Some(&id) {
                next.insert(key, id);
                out.files.push(FontFile { face: face.clone(), bytes: None });
                continue;
            }
            match read_bytes_capped(&path, FONT_FILE_MAX_BYTES as usize) {
                Ok(Some(bytes)) => {
                    next.insert(key, id);
                    out.files.push(FontFile { face: face.clone(), bytes: Some(Arc::from(bytes)) });
                }
                Ok(None) => out.diagnostics.push(face.not_found()),
                Err(e) => out.diagnostics.push(Diagnostic::warning(
                    DiagKind::Style,
                    Some(face.pos.clone()),
                    format!("font file 'fonts/{name}' could not be read: {e}"),
                )),
            }
        }
        self.sent = next;
        out
    }
}

/// What registering a batch of files did.
#[derive(Debug, Default)]
pub struct Applied {
    pub diagnostics: Vec<Diagnostic>,
    /// Faces the renderer did not have before: layouts made with a fallback
    /// font are stale, so every scene must measure again.
    pub added: usize,
}

/// UI-thread side: registers files and remembers which ones the renderer
/// refused, so a reading that did not resend a refused file still says so.
#[derive(Debug, Default)]
pub struct FontRegistry {
    refused: HashMap<(String, String), bool>,
}

impl FontRegistry {
    pub fn apply(&mut self, renderer: &mut Renderer, files: &[FontFile]) -> Applied {
        let before = renderer.stats().fonts_registered;
        let mut out = Applied::default();
        let mut now = HashMap::new();
        for file in files {
            let key = (file.face.file_name().to_ascii_lowercase(), file.face.family.clone());
            let (refused, too_big) = match &file.bytes {
                Some(bytes) if bytes.len() as u64 > FONT_FILE_MAX_BYTES => (true, true),
                Some(bytes) => (renderer.register_font(Arc::clone(bytes), Some(&file.face.family)).is_err(), false),
                None => (self.refused.get(&key).copied().unwrap_or(false), false),
            };
            if too_big {
                out.diagnostics.push(file.face.too_large());
            } else if refused {
                out.diagnostics.push(file.face.not_a_font());
            }
            now.insert(key, refused);
        }
        self.refused = now;
        out.added = renderer.stats().fonts_registered.saturating_sub(before);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use rsb_core::css::Stylesheet;
    use rsb_render::FontConfig;

    fn faces(css: &str) -> Vec<FontFace> {
        let (sheet, diags) = Stylesheet::parse(css, "style.css");
        assert!(diags.is_empty(), "{diags:?}");
        sheet.font_faces()
    }

    fn face_css(family: &str, file: &str) -> String {
        format!("@font-face {{ font-family: \"{family}\"; src: url(\"fonts/{file}\"); }}\n")
    }

    fn folder_with(files: &[(&str, &[u8])]) -> TempDir {
        let dir = TempDir::new("fonts");
        std::fs::create_dir_all(dir.path().join("fonts")).unwrap();
        for (name, bytes) in files {
            std::fs::write(dir.path().join("fonts").join(name), bytes).unwrap();
        }
        dir
    }

    fn texts(list: &[Diagnostic]) -> Vec<String> {
        list.iter().map(|d| d.message.clone()).collect()
    }

    #[test]
    fn the_bundled_font_is_registered_and_known_by_name() {
        let mut renderer = Renderer::new(FontConfig::default());
        let names = register_bundled(&mut renderer).expect("the bundled font registers");
        assert!(names.iter().any(|n| n == "JetBrains Mono"), "{names:?}");
        assert!(renderer.has_family("JetBrains Mono"), "the missing-font check sees it");
        assert_eq!(renderer.stats().fonts_registered, 1);
        // Again: the same bytes add nothing.
        register_bundled(&mut renderer).unwrap();
        assert_eq!(renderer.stats().fonts_registered, 1);
    }

    #[test]
    fn a_face_loads_from_the_fonts_folder_and_registers_under_its_family() {
        let bundled = assets::bytes(BUNDLED_FONT).unwrap();
        let dir = folder_with(&[("mine.ttf", bundled)]);
        let mut cache = FontCache::default();
        let load = cache.load(dir.path(), &faces(&face_css("Office Mono", "mine.ttf")));
        assert!(load.diagnostics.is_empty(), "{:?}", load.diagnostics);
        assert_eq!(load.files.len(), 1);
        let mut renderer = Renderer::new(FontConfig::default());
        assert!(!renderer.has_family("Office Mono"));
        let applied = FontRegistry::default().apply(&mut renderer, &load.files);
        assert!(applied.diagnostics.is_empty() && applied.added == 1, "{applied:?}");
        assert!(renderer.has_family("Office Mono"));
    }

    #[test]
    fn a_file_that_is_missing_oversized_or_not_a_font_is_a_diagnostic() {
        let dir = folder_with(&[("text.ttf", b"this is not a font")]);
        let big = std::fs::File::create(dir.path().join("fonts").join("big.ttf")).unwrap();
        big.set_len(FONT_FILE_MAX_BYTES + 1).unwrap();
        drop(big);
        let css = [("A", "gone.ttf"), ("B", "big.ttf"), ("C", "text.ttf")].map(|(f, n)| face_css(f, n)).concat();
        let mut cache = FontCache::default();
        let load = cache.load(dir.path(), &faces(&css));
        let msgs = texts(&load.diagnostics);
        assert!(msgs.iter().any(|m| m.contains("'fonts/gone.ttf' was not found")), "{msgs:?}");
        assert!(msgs.iter().any(|m| m.contains("'fonts/big.ttf' is larger than 16 MiB")), "{msgs:?}");
        assert_eq!(load.files.len(), 1, "the unreadable ones are not handed on");

        let mut renderer = Renderer::new(FontConfig::default());
        let mut registry = FontRegistry::default();
        let applied = registry.apply(&mut renderer, &load.files);
        assert_eq!(texts(&applied.diagnostics), ["font file 'fonts/text.ttf' is not a font"]);
        assert_eq!(applied.added, 0);
        // A reading that does not resend the refused file still reports it.
        let again = cache.load(dir.path(), &faces(&css));
        assert!(again.files[0].bytes.is_none(), "unchanged files travel without bytes");
        let applied = registry.apply(&mut renderer, &again.files);
        assert_eq!(texts(&applied.diagnostics), ["font file 'fonts/text.ttf' is not a font"]);
    }

    #[test]
    fn a_name_that_leaves_the_fonts_folder_is_not_found() {
        let dir = folder_with(&[("ok.ttf", b"x")]);
        std::fs::write(dir.path().join("secret.ttf"), b"x").unwrap();
        for src in ["../secret.ttf", "..\\secret.ttf", "C:/Windows/Fonts/arial.ttf", "sub/x.ttf", "."] {
            let f = FontFace {
                family: "X".into(),
                src: src.into(),
                weight: (400, 400),
                italic: false,
                pos: rsb_core::diag::SourcePos::new("style.css", 1, 1),
            };
            let load = FontCache::default().load(dir.path(), &[f]);
            assert!(load.files.is_empty() && load.diagnostics[0].message.contains("was not found"), "{src}");
        }
    }

    #[test]
    fn a_seventeenth_file_is_skipped_with_the_limit_named() {
        let dir = folder_with(&[]);
        let css: String = (0..17).map(|i| face_css(&format!("F{i}"), &format!("f{i}.ttf"))).collect();
        let load = FontCache::default().load(dir.path(), &faces(&css));
        let msgs = texts(&load.diagnostics);
        assert_eq!(msgs.len(), 17, "sixteen missing files and the one over the limit");
        assert!(msgs[16].contains("more than 16 font files") && msgs[16].contains("f16.ttf"), "{msgs:?}");
    }

    #[test]
    fn a_changed_file_is_sent_again_and_an_unchanged_one_is_not() {
        let bundled = assets::bytes(BUNDLED_FONT).unwrap();
        let dir = folder_with(&[("mine.ttf", bundled)]);
        let css = face_css("Mine", "mine.ttf");
        let mut cache = FontCache::default();
        assert!(cache.load(dir.path(), &faces(&css)).files[0].bytes.is_some());
        assert!(cache.load(dir.path(), &faces(&css)).files[0].bytes.is_none());
        let mut longer = bundled.to_vec();
        longer.push(0);
        std::fs::write(dir.path().join("fonts").join("mine.ttf"), longer).unwrap();
        assert!(cache.load(dir.path(), &faces(&css)).files[0].bytes.is_some(), "its size changed");
        // Dropping the rule forgets the file, so the next rule for it reads it again.
        assert!(cache.load(dir.path(), &[]).files.is_empty());
        assert!(cache.load(dir.path(), &faces(&css)).files[0].bytes.is_some());
    }
}

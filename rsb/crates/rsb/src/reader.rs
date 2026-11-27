//! The config reader: a folder watcher (recursive, 25 ms debounce) feeds a
//! thread that reads and parses what changed, so the UI thread never touches
//! a file. A changed `bar.toml` or `.css` is rebuilt as one `ConfigUpdate`;
//! a changed `widgets\**\*.luau` goes straight to the script thread as
//! `ScriptIn::FileChanged`. `rsb reload` and the menu send `ReadAll`.

use std::collections::{BTreeSet, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use rsb_core::contract::{ConfigFile, FileHash};
use rsb_core::msg::ScriptIn;
use rsb_core::paths::{BAR_TOML, MAX_SOURCE_BYTES, WIDGETS_DIR};
use rsb_core::wake::Wake;
use rsb_services::watch::DirWatcher;

use crate::cfgload::{self, Built, read_bytes_capped_hashed, read_capped};
use crate::cfgwrite;
use crate::fonts::FontCache;
use crate::msg::{ConfigUpdate, ReloadKind, UiMsg};
use crate::stats::wall_ms;

enum ReaderMsg {
    Changed(Vec<PathBuf>, u64),
    ReadAll(u64),
    Stop,
}

/// What a batch of changed paths touches.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Touched {
    pub toml: bool,
    pub css: bool,
    /// A file in `fonts\` changed: the sheet's `@font-face` rules read again.
    pub fonts: bool,
    /// `widgets/...` paths, `/` separators, relative to the config folder.
    pub widgets: BTreeSet<String>,
    /// Something was created, renamed or saved inside `widgets\`. Windows
    /// reports that on the folder itself, so it says only "look again": the
    /// files that really changed come with their own paths.
    pub widgets_folder: bool,
    /// The watcher lost track (buffer overflow = the folder itself): read everything.
    pub everything: bool,
}

/// Sorts changed absolute paths into what they mean. Paths outside the
/// folder, the type files, the editor folder and the undo folder are ignored.
/// A temporary file of a settings write (`.bar.toml.rsb-1-0.tmp`) is neither
/// `bar.toml` nor a `.css` file, so only the rename that finishes the write
/// counts.
pub fn classify(dir: &Path, paths: &[PathBuf]) -> Touched {
    let mut t = Touched::default();
    for p in paths {
        let Ok(rel) = p.strip_prefix(dir) else { continue };
        let parts: Vec<String> = rel
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        let Some(first) = parts.first() else {
            t.everything = true;
            continue;
        };
        let name = parts.join("/");
        let lower = name.to_ascii_lowercase();
        let in_widgets = first.eq_ignore_ascii_case(WIDGETS_DIR);
        if first.eq_ignore_ascii_case("types")
            || first.eq_ignore_ascii_case(".vscode")
            || first.eq_ignore_ascii_case(cfgwrite::UNDO_DIR)
        {
            continue;
        }
        if first.eq_ignore_ascii_case("fonts") && parts.len() > 1 {
            t.fonts = true;
            continue;
        }
        if parts.len() == 1 && lower == BAR_TOML {
            t.toml = true;
        } else if lower.ends_with(".css") {
            t.css = true;
        } else if in_widgets && parts.len() == 1 {
            t.widgets_folder = true;
        } else if in_widgets && lower.ends_with(".luau") {
            // The script thread keys files by the folder's canonical spelling.
            t.widgets.insert(format!("{WIDGETS_DIR}/{}", parts[1..].join("/")));
        }
    }
    t
}

fn widget_files(dir: &Path) -> BTreeSet<String> {
    fn walk(root: &Path, at: &Path, depth: u32, out: &mut BTreeSet<String>) {
        let Ok(entries) = std::fs::read_dir(at) else { return };
        for e in entries.filter_map(Result::ok) {
            let path = e.path();
            if path.is_dir() {
                if depth < 8 {
                    walk(root, &path, depth + 1, out);
                }
            } else if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("luau"))
                && let Ok(rel) = path.strip_prefix(root)
            {
                out.insert(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(dir, &dir.join(WIDGETS_DIR), 0, &mut out);
    out
}

/// The version of every file a rebuild reads: `bar.toml` and each `.css` file
/// under the folder (the style chain, its imports included), found by name
/// and in path order. A rebuild of the same bytes shows nothing new.
type Print = Vec<(String, FileHash)>;

/// Most style files looked at, so a folder full of them stays cheap.
const MAX_PRINT_FILES: usize = 32;

fn fingerprint(dir: &Path) -> Print {
    fn walk(root: &Path, at: &Path, depth: u32, out: &mut Print) {
        let Ok(entries) = std::fs::read_dir(at) else { return };
        let mut names: Vec<_> = entries.filter_map(Result::ok).collect();
        names.sort_by_key(|e| e.file_name());
        for e in names {
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            if path.is_dir() {
                let skip = depth >= 4 || matches!(name.as_str(), ".undo" | ".vscode" | "types" | "widgets" | "fonts");
                if !skip {
                    walk(root, &path, depth + 1, out);
                }
            } else if name.ends_with(".css")
                && out.len() < MAX_PRINT_FILES
                && let (Ok(rel), Ok(Some((_, hash)))) =
                    (path.strip_prefix(root), read_bytes_capped_hashed(&path, MAX_SOURCE_BYTES))
            {
                out.push((rel.to_string_lossy().replace('\\', "/"), hash));
            }
        }
    }
    let mut out = Print::new();
    let toml = read_bytes_capped_hashed(&dir.join(BAR_TOML), MAX_SOURCE_BYTES).ok().flatten();
    out.push((BAR_TOML.to_string(), toml.map_or(FileHash::Missing, |(_, h)| h)));
    walk(dir, dir, 0, &mut out);
    out
}

fn print_hash(print: &Print, name: &str) -> Option<FileHash> {
    print.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, h)| *h)
}

/// Keeps the text of each file of `built` that loaded clean, for `restore`.
/// `expected` says which version of the file `built` was made from; a file
/// that changed since (or whose version is unknown) is left alone, so a copy
/// is never text that was not parsed.
fn keep_clean(dir: &Path, built: &Built, expected: &dyn Fn(ConfigFile) -> Option<FileHash>) {
    keep_clean_of(dir, built.config.is_some(), built.sheet.is_some(), expected);
}

fn keep_clean_of(dir: &Path, bar_clean: bool, css_clean: bool, expected: &dyn Fn(ConfigFile) -> Option<FileHash>) {
    for (file, clean) in [(ConfigFile::BarToml, bar_clean), (ConfigFile::StyleCss, css_clean)] {
        if !clean {
            continue;
        }
        let Some(want) = expected(file) else { continue };
        if want == FileHash::Missing {
            continue;
        }
        if let Ok(Some((bytes, hash))) = read_bytes_capped_hashed(&dir.join(file.name()), MAX_SOURCE_BYTES)
            && hash == want
        {
            cfgwrite::keep_last_good(dir, file, &bytes);
        }
    }
}

/// For the first start: keeps what the very first read loaded clean. A file
/// the build did not report a version for is left alone.
pub fn keep_last_good(dir: &Path, built: &Built) {
    keep_clean(dir, built, &|f| match f {
        ConfigFile::BarToml => built.hashes.bar_toml,
        ConfigFile::StyleCss => built.hashes.style_css,
    });
}

/// `keep_last_good` once the bar has had time to put its first frame up: the
/// copy is a file write with a flush to disk, which has no place on the way to
/// the first frame.
pub fn keep_last_good_soon(dir: &Path, built: &Built) {
    let (dir, hashes) = (dir.to_path_buf(), built.hashes);
    let (bar, css) = (built.config.is_some(), built.sheet.is_some());
    let _ = std::thread::Builder::new().name("rsb-keep-copy".into()).spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        keep_clean_of(&dir, bar, css, &|f| match f {
            ConfigFile::BarToml => hashes.bar_toml,
            ConfigFile::StyleCss => hashes.style_css,
        });
    });
}

fn text_hash(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

struct Worker {
    dir: PathBuf,
    ui_tx: Sender<UiMsg>,
    script_tx: Sender<ScriptIn>,
    waker: Arc<dyn Wake>,
    preset: String,
    known: BTreeSet<String>,
    /// Hash of the source last sent for each widget file, so a folder-level
    /// notice does not restart widgets whose file did not change.
    sent: HashMap<String, u64>,
    /// Which font files the UI already has.
    fonts: FontCache,
    /// What the last rebuild read; the same bytes again are not rebuilt.
    print: Option<Print>,
}

impl Worker {
    fn send_ui(&self, msg: UiMsg) {
        if self.ui_tx.send(msg).is_ok() {
            self.waker.wake();
        }
    }

    /// `skip_equal`: a change notice, not a request: when every file is the
    /// very bytes of the last build (Windows often reports one save twice),
    /// nothing is rebuilt.
    fn rebuild(&mut self, kind: ReloadKind, notified: u64, skip_equal: bool) {
        // The versions are read before the build, so a file that changes
        // during it is read again on the notice that change brings.
        let print = fingerprint(&self.dir);
        if skip_equal && self.print.as_ref() == Some(&print) {
            return;
        }
        let built = cfgload::build_cached(&self.dir, None, &self.preset, &mut self.fonts);
        if let Some(cfg) = &built.config {
            self.preset = cfg.preset.clone();
        }
        keep_clean(&self.dir, &built, &|f| print_hash(&print, f.name()));
        self.print = Some(print);
        self.send_ui(UiMsg::Config(ConfigUpdate { kind, hashes: built.hashes, built, notified_wall_ms: notified }));
    }

    /// `only_if_changed`: a scan rather than a named file, so a file whose
    /// source is the one already sent is left alone.
    fn send_widget(&mut self, rel: &str, notified: u64, only_if_changed: bool) {
        match read_capped(&self.dir.join(rel), MAX_SOURCE_BYTES) {
            Ok(source) => {
                let hash = source.as_deref().map(text_hash);
                if only_if_changed && hash.is_some() && self.sent.get(rel).copied() == hash {
                    return;
                }
                match hash {
                    Some(h) => {
                        self.known.insert(rel.to_string());
                        self.sent.insert(rel.to_string(), h);
                    }
                    None => {
                        self.known.remove(rel);
                        self.sent.remove(rel);
                    }
                }
                let _ = self.script_tx.send(ScriptIn::FileChanged { rel_path: rel.to_string(), source });
                self.send_ui(UiMsg::WidgetFile { notified_wall_ms: notified });
            }
            // Unreadable (too large, locked for good): the old copy keeps
            // running and the log says why.
            Err(e) => log::warn!("{rel}: {e}; the widget keeps running its last good copy"),
        }
    }

    /// Looks at every widget file; `only_if_changed` leaves the ones whose
    /// source was already sent.
    fn scan_widgets(&mut self, notified: u64, only_if_changed: bool) {
        let now = widget_files(&self.dir);
        for gone in self.known.difference(&now).cloned().collect::<Vec<_>>() {
            self.send_widget(&gone, notified, false);
        }
        for rel in now {
            self.send_widget(&rel, notified, only_if_changed);
        }
    }

    /// `rsb reload` and the menu: everything restarts.
    fn read_all(&mut self, notified: u64) {
        self.rebuild(ReloadKind::All, notified, false);
        self.scan_widgets(notified, false);
    }

    fn changed(&mut self, paths: &[PathBuf], notified: u64) {
        let t = classify(&self.dir, paths);
        if t.everything {
            // The watcher lost track: read it all, but restart only what differs.
            self.rebuild(ReloadKind::All, notified, false);
            return self.scan_widgets(notified, true);
        }
        for rel in &t.widgets {
            self.send_widget(rel, notified, false);
        }
        if t.widgets_folder {
            self.scan_widgets(notified, true);
        }
        if t.toml || t.css || t.fonts {
            let kind = if t.toml { ReloadKind::Toml } else { ReloadKind::Css };
            self.rebuild(kind, notified, !t.fonts);
        }
    }
}

pub struct Reader {
    tx: Sender<ReaderMsg>,
    thread: Option<JoinHandle<()>>,
    watcher: Option<DirWatcher>,
    done: Receiver<()>,
}

impl Reader {
    /// `known_widgets` are the user widget files the script thread already
    /// has (so a later deletion is noticed).
    pub fn start(
        dir: PathBuf,
        preset: String,
        ui_tx: Sender<UiMsg>,
        script_tx: Sender<ScriptIn>,
        waker: Arc<dyn Wake>,
        fonts: FontCache,
    ) -> Reader {
        let (tx, rx) = channel::<ReaderMsg>();
        let (done_tx, done) = channel();
        let watcher = {
            let tx = tx.clone();
            match DirWatcher::start(
                &dir,
                true,
                Duration::from_millis(25),
                Box::new(move |paths| {
                    let _ = tx.send(ReaderMsg::Changed(paths, wall_ms()));
                }),
            ) {
                Ok(w) => Some(w),
                Err(e) => {
                    log::error!("the config folder cannot be watched: {e}; edits will not apply until rsb reload");
                    None
                }
            }
        };
        let known = widget_files(&dir);
        // What the script thread loaded at start, so the first folder-level notice finds nothing new.
        let sent = known
            .iter()
            .filter_map(|rel| match read_capped(&dir.join(rel), MAX_SOURCE_BYTES) {
                Ok(Some(source)) => Some((rel.clone(), text_hash(&source))),
                _ => None,
            })
            .collect();
        let mut worker = Worker { known, sent, dir, ui_tx, script_tx, waker, preset, fonts, print: None };
        let thread = std::thread::Builder::new()
            .name("rsb-config".into())
            .spawn(move || {
                while let Ok(msg) = rx.recv() {
                    match msg {
                        ReaderMsg::Stop => break,
                        ReaderMsg::Changed(paths, at) => worker.changed(&paths, at),
                        ReaderMsg::ReadAll(at) => worker.read_all(at),
                    }
                }
                let _ = done_tx.send(());
            })
            .expect("the config reader thread starts");
        Reader { tx, thread: Some(thread), watcher, done }
    }

    /// `rsb reload` and the menu: read everything again.
    pub fn read_all(&self) {
        let _ = self.tx.send(ReaderMsg::ReadAll(wall_ms()));
    }

    /// Stop order, step 5: the watcher (1 s), then the reader (1 s).
    pub fn stop(mut self) {
        self.watcher.take();
        let _ = self.tx.send(ReaderMsg::Stop);
        if self.done.recv_timeout(Duration::from_secs(1)).is_ok() {
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        } else {
            log::warn!("the config reader did not stop within 1000 ms; leaving it to exit with the process");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsb_core::contract::{ConfigFile, FileHash};

    fn p(dir: &Path, rel: &str) -> PathBuf {
        dir.join(rel.replace('/', "\\"))
    }

    #[test]
    fn paths_are_sorted_into_what_they_change() {
        let dir = PathBuf::from(r"C:\cfg");
        let t = classify(
            &dir,
            &[
                p(&dir, "bar.toml"),
                p(&dir, "colors.css"),
                p(&dir, "themes/extra.css"),
                p(&dir, "widgets/my/weather.luau"),
                p(&dir, "widgets/lib/format.luau"),
                p(&dir, "widgets/notes.txt"),
                p(&dir, "types/rsb.d.luau"),
                p(&dir, ".vscode/settings.json"),
                PathBuf::from(r"D:\elsewhere\bar.toml"),
                p(&dir, "sub/bar.toml"),
            ],
        );
        assert!(t.toml && t.css && !t.everything);
        assert_eq!(
            t.widgets.iter().map(String::as_str).collect::<Vec<_>>(),
            ["widgets/lib/format.luau", "widgets/my/weather.luau"]
        );
    }

    #[test]
    fn a_file_in_the_fonts_folder_means_the_font_rules_read_again() {
        let dir = PathBuf::from(r"C:\cfg");
        let t = classify(&dir, &[p(&dir, "fonts/Mine.ttf")]);
        assert!(t.fonts && !t.toml && !t.css && !t.everything);
        let t = classify(&dir, &[p(&dir, "Fonts/sub/x.otf"), p(&dir, "fonts")]);
        assert!(t.fonts, "any case, any depth");
        assert!(!classify(&dir, &[p(&dir, "fonts")]).fonts, "the folder alone says nothing");
    }

    #[test]
    fn the_folder_itself_means_read_everything_and_the_widgets_folder_only_a_look() {
        let dir = PathBuf::from(r"C:\cfg");
        assert!(classify(&dir, std::slice::from_ref(&dir)).everything);
        let t = classify(&dir, &[p(&dir, "widgets")]);
        assert!(t.widgets_folder && !t.everything && t.widgets.is_empty());
        assert_eq!(classify(&dir, &[p(&dir, "types")]), Touched::default());
    }

    #[test]
    fn folder_names_match_in_any_case() {
        let dir = PathBuf::from(r"C:\cfg");
        let t = classify(&dir, &[p(&dir, "Widgets/My/a.luau"), p(&dir, "TYPES/rsb.d.luau"), p(&dir, "BAR.TOML")]);
        assert_eq!(t.widgets.iter().map(String::as_str).collect::<Vec<_>>(), ["widgets/My/a.luau"]);
        assert!(t.toml);
    }

    struct Rig {
        dir: crate::testutil::TempDir,
        worker: Worker,
        script: std::sync::mpsc::Receiver<ScriptIn>,
        ui: std::sync::mpsc::Receiver<UiMsg>,
    }

    fn rig(files: &[(&str, &str)]) -> Rig {
        let dir = crate::testutil::TempDir::new("reader");
        std::fs::create_dir_all(dir.path().join("widgets")).unwrap();
        for (rel, text) in files {
            std::fs::write(dir.path().join(rel.replace('/', "\\")), text).unwrap();
        }
        let (ui_tx, ui_rx) = channel();
        let (script_tx, script) = channel();
        let known = widget_files(dir.path());
        let sent = known
            .iter()
            .map(|rel| (rel.clone(), text_hash(&std::fs::read_to_string(dir.path().join(rel)).unwrap())))
            .collect();
        let worker = Worker {
            dir: dir.path().to_path_buf(),
            ui_tx,
            script_tx,
            waker: Arc::new(rsb_core::wake::NoWake),
            preset: String::new(),
            known,
            sent,
            fonts: FontCache::default(),
            print: None,
        };
        Rig { dir, worker, script, ui: ui_rx }
    }

    fn sent_files(rig: &Rig) -> Vec<String> {
        rig.script
            .try_iter()
            .filter_map(|m| match m {
                ScriptIn::FileChanged { rel_path, .. } => Some(rel_path),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_notice_on_the_widgets_folder_restarts_only_widgets_whose_file_changed() {
        let mut r = rig(&[("widgets/alpha.luau", "a1"), ("widgets/beta.luau", "b1")]);
        let folder = vec![r.dir.path().join("widgets")];
        // A save by temp file and rename: the folder is marked modified, beta is untouched.
        r.worker.changed(&folder, 0);
        assert_eq!(sent_files(&r), Vec::<String>::new());
        std::fs::write(r.dir.path().join("widgets\\alpha.luau"), "a2").unwrap();
        r.worker.changed(&folder, 0);
        assert_eq!(sent_files(&r), ["widgets/alpha.luau"]);
        // The same notice again finds nothing new.
        r.worker.changed(&folder, 0);
        assert_eq!(sent_files(&r), Vec::<String>::new());
    }

    #[test]
    fn a_new_and_a_deleted_widget_file_are_sent_by_a_folder_notice() {
        let mut r = rig(&[("widgets/alpha.luau", "a1"), ("widgets/beta.luau", "b1")]);
        let folder = vec![r.dir.path().join("widgets")];
        std::fs::write(r.dir.path().join("widgets\\gamma.luau"), "g1").unwrap();
        std::fs::remove_file(r.dir.path().join("widgets\\beta.luau")).unwrap();
        r.worker.changed(&folder, 0);
        let mut got = sent_files(&r);
        got.sort();
        assert_eq!(got, ["widgets/beta.luau", "widgets/gamma.luau"]);
    }

    #[test]
    fn a_named_file_and_an_explicit_reload_always_restart() {
        let mut r = rig(&[("widgets/alpha.luau", "a1"), ("widgets/beta.luau", "b1")]);
        r.worker.changed(&[r.dir.path().join("widgets\\alpha.luau")], 0);
        assert_eq!(sent_files(&r), ["widgets/alpha.luau"]);
        r.worker.read_all(0);
        assert_eq!(sent_files(&r), ["widgets/alpha.luau", "widgets/beta.luau"]);
    }

    fn config_updates(r: &Rig) -> usize {
        r.ui.try_iter().filter(|m| matches!(m, UiMsg::Config(_))).count()
    }

    const BAR_A: &str = "config_version = 1\n[[bar]]\nheight = 40\n";
    const BAR_B: &str = "config_version = 1\n[[bar]]\nheight = 44\n";
    const CSS_A: &str = "bar { color: red }\n";
    const CSS_B: &str = "bar { color: blue }\n";

    fn settle(r: &Rig) {
        // The rig's folder holds both files at their new text, as after a settings write.
        let _ = r;
    }

    #[test]
    fn the_undo_folder_and_the_temporary_files_of_a_write_are_not_changes() {
        let dir = PathBuf::from(r"C:\cfg");
        let t = classify(
            &dir,
            &[
                p(&dir, ".undo"),
                p(&dir, ".undo/1/before.bar.toml"),
                p(&dir, ".undo/last-good.style.css.txt"),
                p(&dir, ".undo/lock"),
                p(&dir, ".undo/written"),
                p(&dir, ".undo/1.part/change.json"),
                p(&dir, ".bar.toml.rsb-1234-0.tmp"),
                p(&dir, ".style.css.rsb-1234-1.tmp"),
            ],
        );
        assert_eq!(t, Touched::default());
        let t = classify(
            &dir,
            &[p(&dir, ".undo"), p(&dir, "bar.toml"), p(&dir, ".bar.toml.rsb-1-0.tmp"), p(&dir, "style.css")],
        );
        assert!(t.toml && t.css && !t.everything && t.widgets.is_empty());
    }

    #[test]
    fn one_change_one_config_update() {
        let mut r = rig(&[("bar.toml", BAR_A), ("style.css", CSS_A)]);
        let d = r.dir.path().to_path_buf();
        // A settings write of both files: two renames and the entry's files in one batch.
        std::fs::write(d.join("bar.toml"), BAR_B).unwrap();
        std::fs::write(d.join("style.css"), CSS_B).unwrap();
        let batch = vec![
            d.join(".bar.toml.rsb-1-0.tmp"),
            d.join("bar.toml"),
            d.join(".style.css.rsb-1-1.tmp"),
            d.join("style.css"),
            d.join(".undo"),
            d.join(".undo\\1.part"),
            d.join(".undo\\written"),
        ];
        r.worker.changed(&batch, 0);
        assert_eq!(config_updates(&r), 1);
        // The same two renames reported in two batches: the second finds nothing new.
        std::fs::write(d.join("bar.toml"), BAR_A).unwrap();
        std::fs::write(d.join("style.css"), CSS_A).unwrap();
        r.worker.changed(&[d.join("bar.toml")], 0);
        r.worker.changed(&[d.join("style.css")], 0);
        assert_eq!(config_updates(&r), 1, "one update for one change, however it was reported");
        settle(&r);
    }

    #[test]
    fn a_real_settings_write_reaches_the_reader_as_one_update() {
        let mut r = rig(&[("bar.toml", BAR_A), ("style.css", CSS_A)]);
        let d = r.dir.path().to_path_buf();
        r.worker.changed(&[d.join("bar.toml")], 0);
        let _ = config_updates(&r);
        let (tx, rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
        let _watcher = DirWatcher::start(
            &d,
            true,
            Duration::from_millis(25),
            Box::new(move |paths| {
                let _ = tx.send(paths);
            }),
        )
        .unwrap();
        let plan = rsb_core::cfgedit::Plan {
            files: [(ConfigFile::BarToml, BAR_A, BAR_B), (ConfigFile::StyleCss, CSS_A, CSS_B)]
                .into_iter()
                .map(|(file, old, new)| rsb_core::cfgedit::FilePlan {
                    file,
                    base: FileHash::of(old.as_bytes()),
                    created: false,
                    bom: false,
                    splices: Vec::new(),
                    after: new.to_string(),
                    what: "test".into(),
                })
                .collect(),
            changes: Vec::new(),
            notices: Vec::new(),
            preset_switch: None,
            accent: None,
        };
        cfgwrite::apply(&d, &plan, &cfgwrite::WriteOpts::default()).unwrap();
        // Everything the watcher reports for the write, after it finished.
        let mut batches = Vec::new();
        while let Ok(b) = rx.recv_timeout(Duration::from_millis(400)) {
            batches.push(b);
        }
        assert!(!batches.is_empty());
        for b in &batches {
            let t = classify(&d, b);
            assert!(t.toml || t.css || t == Touched::default(), "{t:?}");
        }
        for b in &batches {
            r.worker.changed(b, 0);
        }
        assert_eq!(config_updates(&r), 1, "{} batches reported", batches.len());
        assert_eq!(std::fs::read_to_string(d.join("bar.toml")).unwrap(), BAR_B);
    }

    #[test]
    fn a_batch_that_fails_and_is_put_back_is_at_most_one_update() {
        use std::cell::RefCell;
        use std::fs::OpenOptions;
        use std::os::windows::fs::OpenOptionsExt;
        use std::rc::Rc;
        let mut r = rig(&[("bar.toml", BAR_A), ("style.css", CSS_A)]);
        let d = r.dir.path().to_path_buf();
        r.worker.changed(&[d.join("bar.toml")], 0);
        let _ = config_updates(&r);
        let (tx, rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
        let _watcher = DirWatcher::start(
            &d,
            true,
            Duration::from_millis(25),
            Box::new(move |paths| {
                let _ = tx.send(paths);
            }),
        )
        .unwrap();
        let plan = rsb_core::cfgedit::Plan {
            files: [(ConfigFile::BarToml, BAR_A, BAR_B), (ConfigFile::StyleCss, CSS_A, CSS_B)]
                .into_iter()
                .map(|(file, old, new)| rsb_core::cfgedit::FilePlan {
                    file,
                    base: FileHash::of(old.as_bytes()),
                    created: false,
                    bom: false,
                    splices: Vec::new(),
                    after: new.to_string(),
                    what: "test".into(),
                })
                .collect(),
            changes: Vec::new(),
            notices: Vec::new(),
            preset_switch: None,
            accent: None,
        };
        // Someone holds style.css open, so the second replace fails and bar.toml is put back.
        let held: Rc<RefCell<Option<std::fs::File>>> = Rc::new(RefCell::new(None));
        let h = held.clone();
        let style = d.join("style.css");
        cfgwrite::BETWEEN_COMMITS.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                *h.borrow_mut() = OpenOptions::new().read(true).share_mode(3).open(&style).ok();
            }))
        });
        assert!(cfgwrite::apply(&d, &plan, &cfgwrite::WriteOpts::default()).is_err());
        drop(held);
        assert_eq!(std::fs::read_to_string(d.join("bar.toml")).unwrap(), BAR_A, "put back");
        let mut batches = Vec::new();
        while let Ok(b) = rx.recv_timeout(Duration::from_millis(400)) {
            batches.push(b);
        }
        assert!(!batches.is_empty());
        for b in &batches {
            r.worker.changed(b, 0);
        }
        assert!(config_updates(&r) <= 1, "{} batches reported", batches.len());
    }

    #[test]
    fn equal_text_not_rebuilt() {
        let mut r = rig(&[("bar.toml", BAR_A), ("style.css", CSS_A), ("colors.css", "bar { background: blue }\n")]);
        let d = r.dir.path().to_path_buf();
        let bar = [d.join("bar.toml")];
        r.worker.changed(&bar, 0);
        assert_eq!(config_updates(&r), 1, "the first notice builds");
        // Windows reports one save twice: nothing is rebuilt for the same bytes.
        r.worker.changed(&bar, 0);
        std::fs::write(d.join("bar.toml"), BAR_A).unwrap();
        r.worker.changed(&bar, 0);
        r.worker.changed(&[d.join("style.css")], 0);
        assert_eq!(config_updates(&r), 0);
        // A changed byte, in either file or in an imported one, is a change.
        std::fs::write(d.join("bar.toml"), BAR_B).unwrap();
        r.worker.changed(&bar, 0);
        assert_eq!(config_updates(&r), 1);
        std::fs::write(d.join("colors.css"), "bar { background: red }\n").unwrap();
        r.worker.changed(&[d.join("colors.css")], 0);
        assert_eq!(config_updates(&r), 1);
        r.worker.changed(&[d.join("colors.css")], 0);
        assert_eq!(config_updates(&r), 0);
        // A request to reload always rebuilds.
        r.worker.read_all(0);
        assert_eq!(config_updates(&r), 1);
        // A font file notice always does too: the bytes of fonts are not in the print.
        r.worker.changed(&[d.join("fonts\\Mine.ttf")], 0);
        assert_eq!(config_updates(&r), 1);
        // Deleting a file is a change as well.
        std::fs::remove_file(d.join("colors.css")).unwrap();
        r.worker.changed(&[d.join("colors.css")], 0);
        assert_eq!(config_updates(&r), 1);
    }

    #[test]
    fn the_last_text_that_loaded_clean_is_kept_for_restore() {
        let mut r = rig(&[("bar.toml", BAR_A), ("style.css", CSS_A)]);
        let d = r.dir.path().to_path_buf();
        r.worker.changed(&[d.join("bar.toml")], 0);
        let kept = |name: &str| std::fs::read_to_string(d.join(".undo").join(name)).ok();
        assert_eq!(kept("last-good.bar.toml").as_deref(), Some(BAR_A));
        assert_eq!(kept("last-good.style.css.txt").as_deref(), Some(CSS_A));
        // A broken save keeps the old copy; a clean one replaces it.
        std::fs::write(d.join("bar.toml"), "config_version = 1\n[[bar]]\nheight = \n").unwrap();
        std::fs::write(d.join("style.css"), "bar {\n  color: red\n").unwrap();
        r.worker.changed(&[d.join("bar.toml"), d.join("style.css")], 0);
        assert_eq!(kept("last-good.bar.toml").as_deref(), Some(BAR_A));
        assert_eq!(kept("last-good.style.css.txt").as_deref(), Some(CSS_A));
        std::fs::write(d.join("bar.toml"), BAR_B).unwrap();
        r.worker.changed(&[d.join("bar.toml")], 0);
        assert_eq!(kept("last-good.bar.toml").as_deref(), Some(BAR_B));
        assert_eq!(kept("last-good.style.css.txt").as_deref(), Some(CSS_A), "a still broken style keeps its copy");
        assert!(cfgwrite::has_last_good(&d, ConfigFile::StyleCss));
        // The copies are not read as changes.
        let t = classify(&d, &[d.join(".undo").join("last-good.bar.toml")]);
        assert_eq!(t, Touched::default());
    }

    #[test]
    fn the_first_start_keeps_a_copy_only_for_a_version_the_build_reported() {
        let r = rig(&[("bar.toml", BAR_A), ("style.css", CSS_A)]);
        let d = r.dir.path().to_path_buf();
        let mut built = cfgload::build(&d, None, "default");
        built.hashes = crate::msg::AppliedHashes::default();
        keep_last_good(&d, &built);
        assert!(!d.join(".undo").exists(), "no version reported, nothing proven, nothing kept");
        built.hashes = crate::msg::AppliedHashes {
            bar_toml: Some(FileHash::of(BAR_A.as_bytes())),
            style_css: Some(FileHash::of(b"not the file that was read")),
        };
        keep_last_good(&d, &built);
        assert!(d.join(".undo").join("last-good.bar.toml").exists());
        assert!(!d.join(".undo").join("last-good.style.css.txt").exists());
    }

    #[test]
    fn a_batch_that_is_rolled_back_applies_nothing_new() {
        use std::os::windows::fs::OpenOptionsExt;
        let mut r = rig(&[("bar.toml", BAR_A), ("style.css", CSS_A)]);
        let d = r.dir.path().to_path_buf();
        r.worker.changed(&[d.join("bar.toml")], 0);
        assert_eq!(config_updates(&r), 1);
        let (tx, rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
        let _watcher = DirWatcher::start(
            &d,
            true,
            Duration::from_millis(25),
            Box::new(move |paths| {
                let _ = tx.send(paths);
            }),
        )
        .unwrap();
        let plan = rsb_core::cfgedit::Plan {
            files: [(ConfigFile::BarToml, BAR_A, BAR_B), (ConfigFile::StyleCss, CSS_A, CSS_B)]
                .into_iter()
                .map(|(file, old, new)| rsb_core::cfgedit::FilePlan {
                    file,
                    base: FileHash::of(old.as_bytes()),
                    created: false,
                    bom: false,
                    splices: Vec::new(),
                    after: new.to_string(),
                    what: "test".into(),
                })
                .collect(),
            changes: Vec::new(),
            notices: Vec::new(),
            preset_switch: None,
            accent: None,
        };
        // The second file cannot be replaced: someone holds it without sharing delete.
        let style = d.join("style.css");
        let held = std::rc::Rc::new(std::cell::RefCell::new(None));
        let h = held.clone();
        cfgwrite::BETWEEN_COMMITS.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                *h.borrow_mut() = std::fs::OpenOptions::new().read(true).share_mode(3).open(&style).ok();
            }))
        });
        assert!(cfgwrite::apply(&d, &plan, &cfgwrite::WriteOpts::default()).is_err());
        drop(held);
        assert_eq!(std::fs::read_to_string(d.join("bar.toml")).unwrap(), BAR_A, "rolled back");
        let mut batches = Vec::new();
        while let Ok(b) = rx.recv_timeout(Duration::from_millis(400)) {
            batches.push(b);
        }
        assert!(!batches.is_empty(), "the replace and the rollback were both seen");
        for b in &batches {
            r.worker.changed(b, 0);
        }
        assert_eq!(config_updates(&r), 0, "the files are the bytes of the last build");
    }
}

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

use rsb_core::msg::ScriptIn;
use rsb_core::paths::{BAR_TOML, MAX_SOURCE_BYTES, WIDGETS_DIR};
use rsb_core::wake::Wake;
use rsb_services::watch::DirWatcher;

use crate::cfgload::{self, read_capped};
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
/// folder, the type files and the editor folder are ignored.
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
        if first.eq_ignore_ascii_case("types") || first.eq_ignore_ascii_case(".vscode") {
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
}

impl Worker {
    fn send_ui(&self, msg: UiMsg) {
        if self.ui_tx.send(msg).is_ok() {
            self.waker.wake();
        }
    }

    fn rebuild(&mut self, kind: ReloadKind, notified: u64) {
        let built = cfgload::build(&self.dir, None, &self.preset);
        if let Some(cfg) = &built.config {
            self.preset = cfg.preset.clone();
        }
        self.send_ui(UiMsg::Config(ConfigUpdate { kind, built, notified_wall_ms: notified }));
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
        self.rebuild(ReloadKind::All, notified);
        self.scan_widgets(notified, false);
    }

    fn changed(&mut self, paths: &[PathBuf], notified: u64) {
        let t = classify(&self.dir, paths);
        if t.everything {
            // The watcher lost track: read it all, but restart only what differs.
            self.rebuild(ReloadKind::All, notified);
            return self.scan_widgets(notified, true);
        }
        for rel in &t.widgets {
            self.send_widget(rel, notified, false);
        }
        if t.widgets_folder {
            self.scan_widgets(notified, true);
        }
        if t.toml || t.css {
            let kind = if t.toml { ReloadKind::Toml } else { ReloadKind::Css };
            self.rebuild(kind, notified);
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
        let mut worker = Worker { known, sent, dir, ui_tx, script_tx, waker, preset };
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
    }

    fn rig(files: &[(&str, &str)]) -> Rig {
        let dir = crate::testutil::TempDir::new("reader");
        std::fs::create_dir_all(dir.path().join("widgets")).unwrap();
        for (rel, text) in files {
            std::fs::write(dir.path().join(rel.replace('/', "\\")), text).unwrap();
        }
        let (ui_tx, _ui_rx) = channel();
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
        };
        Rig { dir, worker, script }
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
}

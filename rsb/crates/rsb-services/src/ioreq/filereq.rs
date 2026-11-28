//! `rsb.file.read` and `rsb.file.watch`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rsb_core::element::InstanceId;
use rsb_core::io::{FileRequest, IoError, IoEventKind, IoGrant, IoId, limits};

use super::send;
use crate::Sink;
use crate::files::{open_permitted, read_text};
use crate::watch::DirWatcher;

/// One-shot reads running at once, all widgets together.
const MAX_READS: usize = 16;

enum Kind {
    Read,
    Watch,
}

struct Entry {
    owner: InstanceId,
    kind: Kind,
    /// Results are dropped once set (cancelled).
    silent: Arc<AtomicBool>,
    /// Watches: tells the watch thread to stop.
    stop: Option<Sender<()>>,
}

#[derive(Default)]
struct State {
    entries: HashMap<IoId, Entry>,
}

pub(crate) struct FileManager {
    sink: Arc<Sink>,
    state: Arc<Mutex<State>>,
}

fn lock(s: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

fn read_once(req: &FileRequest, grant: &IoGrant) -> Result<String, IoError> {
    let file = open_permitted(&req.path, grant)?;
    read_text(file, req.max_bytes)
}

impl FileManager {
    pub(crate) fn new(sink: Arc<Sink>) -> FileManager {
        FileManager { sink, state: Arc::default() }
    }

    pub(crate) fn read(&self, id: IoId, owner: InstanceId, req: FileRequest, grant: Arc<IoGrant>) {
        let silent = Arc::new(AtomicBool::new(false));
        {
            let mut st = lock(&self.state);
            let reads = st.entries.values().filter(|e| matches!(e.kind, Kind::Read)).count();
            if reads >= MAX_READS {
                drop(st);
                send(&self.sink, id, owner, IoEventKind::File(Err(IoError::Busy)));
                return;
            }
            st.entries.insert(id, Entry { owner, kind: Kind::Read, silent: silent.clone(), stop: None });
        }
        let sink = self.sink.clone();
        let state = self.state.clone();
        let spawned = std::thread::Builder::new().name("rsb-file-read".into()).spawn(move || {
            let result = read_once(&req, &grant);
            lock(&state).entries.remove(&id);
            if !silent.load(Ordering::Acquire) {
                send(&sink, id, owner, IoEventKind::File(result));
            }
        });
        if spawned.is_err() {
            lock(&self.state).entries.remove(&id);
            send(&self.sink, id, owner, IoEventKind::File(Err(IoError::Busy)));
        }
    }

    pub(crate) fn watch(&self, id: IoId, owner: InstanceId, req: FileRequest, grant: Arc<IoGrant>) {
        let silent = Arc::new(AtomicBool::new(false));
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        {
            let mut st = lock(&self.state);
            let watches = st.entries.values().filter(|e| matches!(e.kind, Kind::Watch));
            let (mine, total) = watches.fold((0, 0), |(m, t), e| (m + usize::from(e.owner == owner), t + 1));
            if mine >= limits::FILE_MAX_WATCHES_PER_INSTANCE || total >= limits::FILE_MAX_WATCHES_TOTAL {
                drop(st);
                send(&self.sink, id, owner, IoEventKind::File(Err(IoError::Busy)));
                return;
            }
            st.entries.insert(id, Entry { owner, kind: Kind::Watch, silent: silent.clone(), stop: Some(stop_tx) });
        }
        let sink = self.sink.clone();
        let state = self.state.clone();
        let spawned = std::thread::Builder::new().name("rsb-file-watch".into()).spawn(move || {
            watch_thread(&sink, id, owner, &req, &grant, &silent, &stop_rx);
            lock(&state).entries.remove(&id);
        });
        if spawned.is_err() {
            lock(&self.state).entries.remove(&id);
            send(&self.sink, id, owner, IoEventKind::File(Err(IoError::Busy)));
        }
    }

    pub(crate) fn cancel(&self, id: IoId) {
        if let Some(e) = lock(&self.state).entries.get(&id) {
            stop_entry(e);
        }
    }

    pub(crate) fn cancel_owner(&self, owner: InstanceId) {
        for e in lock(&self.state).entries.values().filter(|e| e.owner == owner) {
            stop_entry(e);
        }
    }

    pub(crate) fn shutdown(&self) {
        for e in lock(&self.state).entries.values() {
            stop_entry(e);
        }
        super::wait_until(Duration::from_millis(500), || lock(&self.state).entries.is_empty());
    }
}

fn stop_entry(e: &Entry) {
    e.silent.store(true, Ordering::Release);
    if let Some(stop) = &e.stop {
        let _ = stop.send(());
    }
}

/// Sends the content now and after every change to the file until stopped.
fn watch_thread(
    sink: &Arc<Sink>,
    id: IoId,
    owner: InstanceId,
    req: &FileRequest,
    grant: &Arc<IoGrant>,
    silent: &Arc<AtomicBool>,
    stop: &mpsc::Receiver<()>,
) {
    let Some(parent) = req.path.parent().map(|p| p.to_path_buf()) else {
        send(sink, id, owner, IoEventKind::File(Err(IoError::NotFound)));
        return;
    };
    let file_name = req.path.file_name().map(|n| n.to_ascii_lowercase()).unwrap_or_default();
    // Reads and their events happen under one lock, so a change that arrives
    // while the first read is running cannot be overtaken by it.
    let gate = Arc::new(Mutex::new(()));
    let emit = {
        let (sink, req, grant, silent, gate) = (sink.clone(), req.clone(), grant.clone(), silent.clone(), gate.clone());
        move || {
            let _g = gate.lock().unwrap_or_else(|e| e.into_inner());
            let result = read_once(&req, &grant);
            if !silent.load(Ordering::Acquire) {
                send(&sink, id, owner, IoEventKind::File(result));
            }
        }
    };
    let on_change = {
        let emit = emit.clone();
        move |paths: Vec<PathBuf>| {
            let hit =
                paths.iter().any(|p| p.file_name().is_none_or(|n| n.to_ascii_lowercase() == file_name) || p == &parent);
            if hit {
                emit();
            }
        }
    };
    let parent_dir = req.path.parent().map(PathBuf::from).unwrap_or_default();
    let watcher = DirWatcher::start(&parent_dir, false, limits::FILE_WATCH_DEBOUNCE, Box::new(on_change));
    let _watcher = match watcher {
        Ok(w) => w,
        Err(_) => {
            // The folder is not there (yet): report once and end.
            send(sink, id, owner, IoEventKind::File(Err(IoError::NotFound)));
            return;
        }
    };
    emit();
    // Park until cancelled; dropping the watcher joins its thread.
    let _ = stop.recv();
}

//! Start-menu apps from the shell's AppsFolder, their icons as in-memory images, launching by
//! id, and opening Windows' own pages (ms-settings:, ms-screenclip:). The list and the icons are
//! read in a short-lived child process (`reader`), so the shell libraries that reading loads, and
//! their memory, leave with it.
//!
//! The ids are launch targets: a desktop app's id is a path form, so an id taken from a script
//! would start any program and bypass `exec`. `launch` therefore runs only an id that is in the
//! list this service published (exact match), and `open_uri` only the two schemes below; both
//! checks run before any shell call.

mod cache;
pub mod reader;
mod shell;

use std::collections::{HashSet, VecDeque};
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use rsb_core::pixels::RgbaImage;
use rsb_core::services::{AppsCommand, ServiceCommand, ServiceId, ServiceStatus};
use rsb_core::value::Value;
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT, TranslateMessage, WM_APP,
};

use self::cache::{IconCache, RELEASE_AFTER, icon_key};
use self::shell::{AppEntry, Shell, StartMenuWatch, SystemShell};
use super::Worker;
use crate::util::map;
use crate::win::{Owned, auto_event, signal};
use crate::{Publisher, Service};

/// Longest launch id accepted or published, in chars.
pub(crate) const MAX_ID_CHARS: usize = 1024;
/// Longest address `open_uri` takes.
const MAX_URI_CHARS: usize = 2048;
/// The only schemes `open_uri` opens.
const URI_SCHEMES: [&str; 2] = ["ms-settings:", "ms-screenclip:"];
/// The shell's change notices arrive in bursts (an installer writes many files): wait for quiet.
const SETTLE: Duration = Duration::from_millis(500);
/// Icons that arrive within this long of each other are published together.
const ICON_BATCH: Duration = Duration::from_millis(300);
/// After a failed list read.
const RETRY: Duration = Duration::from_secs(30);
/// Posted to the service's own message window by the Start menu watch.
const WM_START_MENU: u32 = WM_APP + 1;

/// The program that reads the Start menu: this one started again with `--apps-reader`. A host that
/// is not `rsb.exe` (a test program) names another with `RSB_APPS_READER_EXE`; `rsb.exe` itself
/// ignores the variable, so it never starts a program the environment names.
fn reader_exe() -> PathBuf {
    pick_reader(std::env::current_exe().ok(), std::env::var_os("RSB_APPS_READER_EXE"))
}

fn pick_reader(current: Option<PathBuf>, named: Option<std::ffi::OsString>) -> PathBuf {
    let is_rsb = current.as_deref().and_then(Path::file_stem).is_some_and(|s| s.eq_ignore_ascii_case("rsb"));
    named.filter(|v| !v.is_empty() && !is_rsb).map(PathBuf::from).or(current).unwrap_or_default()
}

/// An id the shell could start again: not empty, no control characters, not over `max` chars.
pub(crate) fn launchable_id(id: &str, max: usize) -> bool {
    !id.is_empty() && id.chars().count() <= max && !id.chars().any(char::is_control)
}

/// `launch`: the id must be in the published list, exactly. Anything else, including a path, a
/// `{GUID}\program.exe` form or a `..`, is refused with no shell call.
pub(crate) fn check_launch_id(id: &str, published: &HashSet<String>) -> Result<(), String> {
    if launchable_id(id, MAX_ID_CHARS) && published.contains(id) { Ok(()) } else { Err("no app with that id".into()) }
}

/// `open_uri`: `ms-settings:` or `ms-screenclip:` and nothing else.
pub(crate) fn check_uri(uri: &str) -> Result<(), String> {
    let scheme_ok = URI_SCHEMES
        .iter()
        .any(|s| uri.len() >= s.len() && uri.is_char_boundary(s.len()) && uri[..s.len()].eq_ignore_ascii_case(s));
    if scheme_ok && uri.chars().count() <= MAX_URI_CHARS && !uri.chars().any(|c| c.is_control() || c == ' ') {
        Ok(())
    } else {
        Err("only Windows settings and screen clip pages can be opened".into())
    }
}

/// Opens one of Windows' own pages: the check, then the shell. The Bluetooth service opens its
/// settings page through this.
pub(crate) fn open_page(uri: &str) -> Result<(), String> {
    check_uri(uri)?;
    SystemShell.open(uri)
}

enum Action {
    Launch(String),
    OpenUri(String),
}

/// Runs one checked action. The launch id is checked again against the list as it is now.
fn run_action(action: Action, listed: &HashSet<String>, shell: &dyn Shell) -> Result<(), String> {
    match action {
        Action::Launch(id) => {
            check_launch_id(&id, listed)?;
            shell.launch(&id)
        }
        Action::OpenUri(uri) => {
            check_uri(&uri)?;
            shell.open(&uri)
        }
    }
}

struct Shared {
    wake: Owned,
    actions: Mutex<VecDeque<Action>>,
    /// The ids of the list as last published, for the checks on the hub thread.
    ids: Mutex<HashSet<String>>,
}

/// Frees the icon cache a while after the service stopped, unless it started again.
struct Release {
    generation: Mutex<u64>,
    changed: Condvar,
}

impl Release {
    /// A new generation: any waiting release thread of an older one gives up.
    fn bump(&self) -> u64 {
        let mut g = self.generation.lock().unwrap_or_else(|e| e.into_inner());
        *g += 1;
        self.changed.notify_all();
        *g
    }

    /// Waits `after` for the generation to move on; true when it did not (release the cache).
    fn wait_unchanged(&self, generation: u64, after: Duration) -> bool {
        let g = self.generation.lock().unwrap_or_else(|e| e.into_inner());
        let (g, _) = self.changed.wait_timeout_while(g, after, |g| *g == generation).unwrap_or_else(|e| e.into_inner());
        *g == generation
    }
}

fn publish(out: &Publisher, ready: bool, apps: &[AppEntry], cache: &Mutex<IconCache>) {
    let cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    let list = apps
        .iter()
        .map(|a| {
            let key = icon_key(&a.id);
            let icon = if cache.contains(&key) { Value::Str(key) } else { Value::Nil };
            map([("id", Value::Str(a.id.clone())), ("name", Value::Str(a.name.clone())), ("icon", icon)])
        })
        .collect();
    out.publish(map([("ready", Value::Bool(ready)), ("apps", Value::List(list))]));
}

/// What the reader thread passes on from the child.
enum Msg {
    List(Vec<AppEntry>),
    Icon(String, RgbaImage),
    End,
    Failed(String),
}

/// One run of the child process that reads the Start menu.
struct ReadJob {
    child: Child,
    rx: Receiver<Msg>,
    /// The last time the child said anything (or started): a child that goes quiet for too long is ended.
    heard: Instant,
}

/// A child that says nothing for this long is killed and the read counts as failed.
const QUIET_LIMIT: Duration = Duration::from_secs(60);
/// Below normal priority, no console window.
const CHILD_FLAGS: u32 = 0x0000_4000 | 0x0800_0000;

impl ReadJob {
    /// Starts `exe --apps-reader <side>` and a thread that turns what it writes into messages and
    /// signals the service's wake event for each. `skip` are the icon keys already held.
    fn start(exe: &Path, side: i32, skip: Vec<String>, shared: Arc<Shared>) -> Result<ReadJob, String> {
        let mut child = Command::new(exe)
            .arg(reader::FLAG)
            .arg(side.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CHILD_FLAGS)
            .spawn()
            .map_err(|e| format!("the Start menu reader could not start: {e}"))?;
        let (stdin, stdout) = (child.stdin.take(), child.stdout.take());
        let (Some(stdin), Some(stdout)) = (stdin, stdout) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("the Start menu reader has no pipes".into());
        };
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new().name("rsb-apps-pipe".into()).spawn(move || {
            // The keys first (the child reads them before it lists anything), then the frames.
            let mut stdin = stdin;
            let _ = stdin.write_all(skip.join("\n").as_bytes());
            drop(stdin);
            let mut out = std::io::BufReader::new(stdout);
            loop {
                let msg = match reader::read_frame(&mut out) {
                    Ok(reader::Frame::List(list)) => Msg::List(list),
                    Ok(reader::Frame::Icon { id, image }) => Msg::Icon(id, image),
                    Ok(reader::Frame::End) => Msg::End,
                    Err(e) => Msg::Failed(e.to_string()),
                };
                let last = matches!(msg, Msg::End | Msg::Failed(_));
                if tx.send(msg).is_err() {
                    break;
                }
                signal(&shared.wake);
                if last {
                    break;
                }
            }
        });
        if let Err(e) = spawned {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("the Start menu reader's pipe thread did not start: {e}"));
        }
        Ok(ReadJob { child, rx, heard: Instant::now() })
    }

    /// Ends the child if it is still running and reaps it, off the service's thread.
    fn retire(mut self) {
        let reaped = std::thread::Builder::new().name("rsb-apps-reap".into()).spawn(move || {
            let _ = self.child.kill();
            let _ = self.child.wait();
        });
        if reaped.is_err() {
            log::debug!("apps: the reader child could not be reaped off thread");
        }
    }
}

/// Drains this thread's messages; true when a Start menu notice was among them.
fn pump() -> bool {
    let mut changed = false;
    let mut msg = MSG::default();
    // SAFETY: a plain message loop over this thread's own queue.
    while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
        if msg.message == WM_START_MENU {
            changed = true;
        } else {
            // SAFETY: `msg` was filled by PeekMessageW.
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    changed
}

/// What the loop learned from one drain of the child's messages.
#[derive(Default)]
struct Drained {
    list: Option<Vec<AppEntry>>,
    stored: usize,
    ended: bool,
    failed: Option<String>,
    /// The icon cache is full: the child need not go on.
    full: bool,
}

/// Takes every message that is waiting. Icons go to the cache and out to the renderer as they come.
fn drain_job(job: &mut ReadJob, out: &Publisher, cache: &Mutex<IconCache>, ids: &mut HashSet<String>) -> Drained {
    let mut d = Drained::default();
    while let Ok(msg) = job.rx.try_recv() {
        job.heard = Instant::now();
        match msg {
            Msg::List(list) => {
                *ids = list.iter().map(|a| a.id.clone()).collect();
                d.list = Some(list);
            }
            Msg::Icon(id, image) => {
                // An icon for an app the list does not hold is not kept.
                if !ids.contains(&id) || d.full {
                    continue;
                }
                let key = icon_key(&id);
                if cache.lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone(), image.clone()) {
                    out.publish_image(key.as_str().into(), image);
                    d.stored += 1;
                } else {
                    d.full = true;
                }
            }
            Msg::End => d.ended = true,
            Msg::Failed(e) => d.failed = Some(e),
        }
    }
    d
}

fn run(
    out: Publisher,
    shared: Arc<Shared>,
    stop: Arc<Owned>,
    cache: Arc<Mutex<IconCache>>,
    shell: Arc<dyn Shell>,
    exe: PathBuf,
) {
    // SAFETY: initialises COM for this thread as an STA (the Start menu watch's window needs a pump); balanced below.
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let watch = StartMenuWatch::new(WM_START_MENU);
    if watch.is_none() {
        log::debug!("apps: no Start menu change notices; the list is read again every 30 s after an error only");
    }
    let side = shell::icon_side();
    let mut entries: Vec<AppEntry> = Vec::new();
    let mut ids: HashSet<String> = HashSet::new();
    let mut job: Option<ReadJob> = None;
    let mut ready = false;
    let mut first_list = true;
    let mut refresh = true;
    let mut settle_at: Option<Instant> = None;
    let mut retry_at: Option<Instant> = None;
    // Icons arrive one by one: the list that says which have one goes out in batches, not per icon.
    let mut publish_at: Option<Instant> = None;
    publish(&out, false, &[], &cache);
    loop {
        crate::count_source_wake();
        if refresh {
            refresh = false;
            if let Some(old) = job.take() {
                old.retire();
            }
            let skip: Vec<String> = ids
                .iter()
                .map(|id| icon_key(id))
                .filter(|k| cache.lock().unwrap_or_else(|e| e.into_inner()).contains(k))
                .collect();
            match ReadJob::start(&exe, side, skip, shared.clone()) {
                Ok(j) => job = Some(j),
                Err(e) => {
                    log::warn!("apps: {e}");
                    out.status(ServiceStatus::Error, Some("the app list could not be read".into()));
                    retry_at = Some(Instant::now() + RETRY);
                }
            }
        }

        let actions: Vec<Action> = shared.actions.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
        if !actions.is_empty() {
            let published = shared.ids.lock().unwrap_or_else(|e| e.into_inner()).clone();
            for a in actions {
                if let Err(e) = run_action(a, &published, shell.as_ref()) {
                    log::warn!("apps command failed: {e}");
                }
            }
        }

        if let Some(j) = job.as_mut() {
            let d = drain_job(j, &out, &cache, &mut ids);
            let mut changed = false;
            if d.stored > 0 && publish_at.is_none() {
                publish_at = Some(Instant::now() + ICON_BATCH);
            }
            if let Some(list) = d.list {
                *shared.ids.lock().unwrap_or_else(|e| e.into_inner()) = ids.clone();
                let keys: HashSet<String> = ids.iter().map(|id| icon_key(id)).collect();
                cache.lock().unwrap_or_else(|e| e.into_inner()).retain_keys(&keys);
                entries = list;
                ready = true;
                retry_at = None;
                changed = true;
                if first_list {
                    first_list = false;
                    // Icons kept from an earlier run go to the renderer again: it may have dropped them.
                    let held = cache.lock().unwrap_or_else(|e| e.into_inner());
                    for a in &entries {
                        let key = icon_key(&a.id);
                        if let Some(image) = held.get(&key) {
                            out.publish_image(key.as_str().into(), image.clone());
                        }
                    }
                }
            }
            changed |= d.ended || d.full || publish_at.is_some_and(|t| Instant::now() >= t);
            if changed {
                publish_at = None;
                publish(&out, ready, &entries, &cache);
            }
            let quiet = j.heard.elapsed() >= QUIET_LIMIT;
            let trouble = d.failed.or_else(|| quiet.then(|| "the Start menu reader went quiet".to_string()));
            if d.ended || d.full || trouble.is_some() {
                if let Some(done) = job.take() {
                    done.retire();
                }
                // The end, or a full cache, is the normal way to finish; anything else is a read to try again.
                if !(d.ended || d.full)
                    && let Some(why) = trouble
                {
                    log::warn!("apps: {why}");
                    if !ready {
                        out.status(ServiceStatus::Error, Some("the app list could not be read".into()));
                    }
                    retry_at = Some(Instant::now() + RETRY);
                }
            }
        }

        let now = Instant::now();
        let timeout = [settle_at, retry_at, publish_at, job.as_ref().map(|j| j.heard + QUIET_LIMIT)]
            .into_iter()
            .flatten()
            .map(|t| t.saturating_duration_since(now))
            .min()
            .unwrap_or(Duration::from_secs(60 * 60 * 24));
        // SAFETY: both handles are live events; the queue is this thread's own.
        let waited = unsafe {
            MsgWaitForMultipleObjects(
                Some(&[stop.get(), shared.wake.get()]),
                false,
                timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32,
                QS_ALLINPUT,
            )
        };
        if waited == WAIT_OBJECT_0 {
            break;
        }
        if pump() {
            settle_at = Some(Instant::now() + SETTLE);
        }
        let now = Instant::now();
        if settle_at.is_some_and(|t| now >= t) {
            settle_at = None;
            refresh = true;
        }
        if retry_at.is_some_and(|t| now >= t) {
            retry_at = None;
            refresh = true;
        }
    }
    if let Some(j) = job.take() {
        j.retire();
    }
    drop(watch);
    if com.is_ok() {
        // SAFETY: balances the successful CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
}

pub(crate) struct AppsService {
    shell: Arc<dyn Shell>,
    /// The program started again to read the Start menu (`--apps-reader`).
    exe: PathBuf,
    cache: Arc<Mutex<IconCache>>,
    release: Arc<Release>,
    release_after: Duration,
    shared: Option<Arc<Shared>>,
    worker: Option<Worker>,
}

impl AppsService {
    pub(crate) fn new() -> AppsService {
        AppsService::with_parts(Arc::new(SystemShell), RELEASE_AFTER)
    }

    /// With another shell (tests pass a recorder) and another release delay.
    pub(crate) fn with_parts(shell: Arc<dyn Shell>, release_after: Duration) -> AppsService {
        AppsService {
            shell,
            exe: reader_exe(),
            cache: Arc::new(Mutex::new(IconCache::default())),
            release: Arc::new(Release { generation: Mutex::new(0), changed: Condvar::new() }),
            release_after,
            shared: None,
            worker: None,
        }
    }

    /// The state the thread and `command` share, made once.
    fn prepare(&mut self) -> Result<Arc<Shared>, String> {
        if let Some(s) = &self.shared {
            return Ok(s.clone());
        }
        let shared = Arc::new(Shared {
            wake: auto_event()?,
            actions: Mutex::new(VecDeque::new()),
            ids: Mutex::new(HashSet::new()),
        });
        self.shared = Some(shared.clone());
        Ok(shared)
    }

    /// Ends the idle clock; a release thread waiting for it gives up.
    fn cancel_release(&self) {
        self.release.bump();
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).mark_active();
    }

    /// Starts the idle clock: `release_after` from now the icon memory is freed, unless the
    /// service starts again first.
    fn schedule_release(&self) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).mark_idle(Instant::now());
        let generation = self.release.bump();
        let (release, cache, after) = (self.release.clone(), self.cache.clone(), self.release_after);
        let spawned = std::thread::Builder::new().name("rsb-apps-release".into()).spawn(move || {
            if release.wait_unchanged(generation, after) {
                let freed = cache.lock().unwrap_or_else(|e| e.into_inner()).release_due(Instant::now(), after);
                if freed {
                    crate::win::trim_heap_soon();
                }
            }
        });
        if spawned.is_err() {
            // No thread: free now rather than hold the memory for ever.
            self.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
        }
    }
}

impl Drop for AppsService {
    fn drop(&mut self) {
        // Wakes a release thread so it ends now; the cache goes with the last Arc.
        self.release.bump();
    }
}

impl Service for AppsService {
    fn id(&self) -> ServiceId {
        ServiceId::Apps
    }

    fn start(&mut self, out: Publisher) -> Result<(), String> {
        let shared = self.prepare()?;
        self.cancel_release();
        let (cache, shell, exe) = (self.cache.clone(), self.shell.clone(), self.exe.clone());
        self.worker = Some(Worker::spawn("rsb-apps", move |stop| run(out, shared, stop, cache, shell, exe))?);
        Ok(())
    }

    fn command(&mut self, command: ServiceCommand) -> Result<(), String> {
        let (ServiceCommand::Apps(c), Some(shared)) = (command, &self.shared) else {
            return Err("not an apps command, or the service is stopped".into());
        };
        let action = match c {
            AppsCommand::Launch(id) => {
                check_launch_id(&id, &shared.ids.lock().unwrap_or_else(|e| e.into_inner()))?;
                Action::Launch(id)
            }
            AppsCommand::OpenUri(uri) => {
                check_uri(&uri)?;
                Action::OpenUri(uri)
            }
        };
        shared.actions.lock().unwrap_or_else(|e| e.into_inner()).push_back(action);
        signal(&shared.wake);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(mut w) = self.worker.take() {
            w.stop();
            self.schedule_release();
        }
    }

    /// A list, not a feed: it is read again when the Start menu changes, and `ready` says whether
    /// it has been read. A month keeps the hub's stale check out of the way.
    fn stale_after(&self) -> Duration {
        Duration::from_secs(60 * 60 * 24 * 30)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;

    #[test]
    fn the_shipped_program_ignores_the_reader_variable_and_a_test_host_honours_it() {
        let named = Some(std::ffi::OsString::from("other-reader.exe"));
        for me in ["rsb.exe", "RSB.EXE"] {
            let got = pick_reader(Some(PathBuf::from(me)), named.clone());
            assert_eq!(got, PathBuf::from(me), "{me} starts itself");
        }
        let host = PathBuf::from("apps-1a2b3c.exe");
        assert_eq!(pick_reader(Some(host.clone()), named), PathBuf::from("other-reader.exe"));
        assert_eq!(pick_reader(Some(host.clone()), None), host, "no variable: itself");
        assert_eq!(
            pick_reader(Some(host.clone()), Some(std::ffi::OsString::new())),
            host,
            "an empty variable counts as none"
        );
    }

    /// Records what it was asked to start; starts nothing.
    #[derive(Default)]
    struct Recorder {
        calls: StdMutex<Vec<String>>,
    }

    impl Shell for Recorder {
        fn launch(&self, id: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("launch {id}"));
            Ok(())
        }
        fn open(&self, uri: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("open {uri}"));
            Ok(())
        }
    }

    fn service_with(published: &[&str]) -> (AppsService, Arc<Recorder>, Arc<Shared>) {
        let shell = Arc::new(Recorder::default());
        let mut svc = AppsService::with_parts(shell.clone(), Duration::from_millis(150));
        let shared = svc.prepare().unwrap();
        *shared.ids.lock().unwrap() = published.iter().map(|s| s.to_string()).collect();
        (svc, shell, shared)
    }

    fn launch(svc: &mut AppsService, id: &str) -> Result<(), String> {
        svc.command(ServiceCommand::Apps(AppsCommand::Launch(id.into())))
    }

    #[test]
    fn launch_refuses_a_path_a_guid_program_and_a_dotdot_id_before_any_shell_call() {
        let (mut svc, shell, shared) =
            service_with(&["Sample.App_1!App", "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\sample.exe"]);
        for bad in [
            "C:\\Windows\\System32\\cmd.exe",
            "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\cmd.exe",
            "..\\..\\Windows\\System32\\cmd.exe",
            "..",
            "sample.app_1!app",
            "Sample.App_1!App\n",
            "",
            &"a".repeat(1025),
        ] {
            assert_eq!(launch(&mut svc, bad), Err("no app with that id".into()), "{}", bad.len());
        }
        assert!(shared.actions.lock().unwrap().is_empty(), "nothing is queued");
        // Defence in depth: even a queued action is checked again against the list as it is then.
        let ids: HashSet<String> = ["Sample.App_1!App".to_string()].into();
        assert!(run_action(Action::Launch("C:\\Windows\\System32\\cmd.exe".into()), &ids, shell.as_ref()).is_err());
        assert!(run_action(Action::Launch("{GUID}\\cmd.exe".into()), &ids, shell.as_ref()).is_err());
        assert!(run_action(Action::Launch("..".into()), &ids, shell.as_ref()).is_err());
        assert!(shell.calls.lock().unwrap().is_empty(), "no shell call for any refused id");
    }

    #[test]
    fn launch_accepts_exactly_the_published_ids() {
        let (mut svc, shell, shared) =
            service_with(&["Sample.App_1!App", "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\sample.exe"]);
        assert!(launch(&mut svc, "Sample.App_1!App").is_ok());
        assert!(launch(&mut svc, "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\sample.exe").is_ok());
        let queued: Vec<Action> = shared.actions.lock().unwrap().drain(..).collect();
        assert_eq!(queued.len(), 2);
        let ids = shared.ids.lock().unwrap().clone();
        for a in queued {
            run_action(a, &ids, shell.as_ref()).unwrap();
        }
        assert_eq!(shell.calls.lock().unwrap().len(), 2);
        assert_eq!(shell.calls.lock().unwrap()[0], "launch Sample.App_1!App");
    }

    #[test]
    fn open_uri_takes_settings_and_screen_clip_pages_only() {
        for ok in ["ms-settings:bluetooth", "ms-settings:", "MS-SETTINGS:network-wifi", "ms-screenclip:?type=recording"]
        {
            assert!(check_uri(ok).is_ok(), "{ok}");
        }
        for bad in [
            "https://example.com",
            "http://example.com",
            "file:///C:/Windows/System32/cmd.exe",
            "C:\\Windows\\System32\\cmd.exe",
            "cmd.exe",
            "ms-settings",
            "ms-settingsx:bluetooth",
            "xms-settings:bluetooth",
            " ms-settings:bluetooth",
            "ms-settings:bluetooth ms-settings:",
            "ms-settings:\u{0}",
            "ms-screenclip:\r\nx",
            "shell:AppsFolder\\x",
            "ms-calculator:",
            "\u{e9}s-settings:",
            "",
        ] {
            assert!(check_uri(bad).is_err(), "{bad:?}");
        }
        assert!(check_uri(&format!("ms-settings:{}", "a".repeat(3000))).is_err(), "too long");

        let (mut svc, shell, shared) = service_with(&[]);
        assert!(svc.command(ServiceCommand::Apps(AppsCommand::OpenUri("https://example.com".into()))).is_err());
        assert!(shared.actions.lock().unwrap().is_empty());
        let none = HashSet::new();
        assert!(run_action(Action::OpenUri("https://example.com".into()), &none, shell.as_ref()).is_err());
        assert!(shell.calls.lock().unwrap().is_empty(), "refused before any call");
        svc.command(ServiceCommand::Apps(AppsCommand::OpenUri("ms-settings:bluetooth".into()))).unwrap();
        let queued: Vec<Action> = shared.actions.lock().unwrap().drain(..).collect();
        run_action(queued.into_iter().next().unwrap(), &none, shell.as_ref()).unwrap();
        assert_eq!(shell.calls.lock().unwrap().as_slice(), ["open ms-settings:bluetooth"]);
    }

    #[test]
    fn the_shell_list_is_read_sorted_and_every_id_could_be_launched_again() {
        let (count, sorted, unique, usable, with_icon) = std::thread::spawn(|| {
            // SAFETY: COM for this thread as an STA, balanced below.
            let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            let list = shell::list_apps(MAX_ID_CHARS).expect("the apps folder is read");
            let sorted = list.windows(2).all(|w| w[0].entry.name.to_lowercase() <= w[1].entry.name.to_lowercase());
            let unique = list.iter().map(|l| &l.entry.id).collect::<HashSet<_>>().len() == list.len();
            let usable = list.iter().all(|l| launchable_id(&l.entry.id, MAX_ID_CHARS) && !l.entry.name.is_empty());
            // How many of the first 40 items the shell already has an icon bitmap for (read only).
            let with_icon =
                list.iter().take(40).filter(|l| shell::has_cached_icon(&l.item, shell::icon_side())).count();
            let n = list.len();
            drop(list);
            if com.is_ok() {
                // SAFETY: balances the successful CoInitializeEx.
                unsafe { CoUninitialize() };
            }
            (n, sorted, unique, usable, with_icon)
        })
        .join()
        .unwrap();
        println!("apps shell list: {count} items, {with_icon} of the first 40 with a cached icon");
        assert!(count > 0 && sorted && unique && usable);
    }

    #[test]
    fn a_stopped_service_takes_no_commands() {
        let mut svc = AppsService::with_parts(Arc::new(Recorder::default()), RELEASE_AFTER);
        assert!(launch(&mut svc, "x").is_err());
    }

    #[test]
    fn the_icon_memory_is_freed_after_the_delay_and_not_when_the_service_comes_back() {
        let (svc, _, _) = service_with(&[]);
        let image = rsb_core::pixels::RgbaImage::new(2, 2, vec![0; 16]).unwrap();
        svc.cache.lock().unwrap().insert("k".into(), image.clone());
        // Stopped and left alone: freed after the delay (150 ms here).
        svc.schedule_release();
        assert_eq!(svc.cache.lock().unwrap().len(), 1, "not yet");
        let t = Instant::now();
        while svc.cache.lock().unwrap().len() > 0 && t.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(svc.cache.lock().unwrap().len(), 0, "freed");
        assert!(t.elapsed() >= Duration::from_millis(140), "{:?}", t.elapsed());

        // Stopped, then back before the delay: kept.
        svc.cache.lock().unwrap().insert("k".into(), image);
        svc.schedule_release();
        svc.cancel_release();
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(svc.cache.lock().unwrap().len(), 1, "a restart keeps the icons");
    }
}

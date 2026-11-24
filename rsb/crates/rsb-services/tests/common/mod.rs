//! Shared helpers for the integration tests.
#![allow(dead_code)]

pub mod server;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rsb_core::element::InstanceId;
use rsb_core::io::{ExecSpec, HttpMethod, HttpRequest, IoEventKind, IoGrant, IoId, IoRequest, ProgramRef};
use rsb_core::msg::{HubEvent, HubPort, HubRequest};
use rsb_core::permissions::{PathEnv, Permissions};
use rsb_core::services::{ServiceEvent, ServiceId};
use rsb_services::fake::FakeWorld;
use rsb_services::{Hub, HubOptions, HubTiming, ServiceFactory};

pub const WAIT: Duration = Duration::from_secs(5);

/// A fresh folder under the build directory (never the user's folders). Each
/// call gets its own folder, so tests that ask for the same name in parallel
/// never delete each other's.
pub fn test_dir(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let exe = std::env::current_exe().expect("test exe path");
    let unique = format!("{name}-{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let dir = exe.parent().and_then(Path::parent).expect("target folder").join("rsb-services-tests").join(unique);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the test folder");
    // Resolve through any link so paths compare with final paths.
    std::fs::canonicalize(&dir).map(strip_verbatim).unwrap_or(dir)
}

pub fn strip_verbatim(p: PathBuf) -> PathBuf {
    let t = p.to_string_lossy().into_owned();
    PathBuf::from(t.strip_prefix("\\\\?\\").unwrap_or(&t).to_string())
}

/// The helper program built from `examples/helper.rs`.
pub fn helper_exe() -> PathBuf {
    let exe = std::env::current_exe().expect("test exe path");
    let p = exe.parent().and_then(Path::parent).expect("target folder").join("examples").join("helper.exe");
    assert!(
        p.is_file(),
        "the helper example is not built: run `cargo test` (it builds examples) or `cargo build --examples`"
    );
    p
}

/// What events a hub produced, in order.
pub struct Collector {
    pub rx: Receiver<HubEvent>,
    pub seen: Mutex<Vec<HubEvent>>,
}

impl Collector {
    /// The next event within `timeout`.
    pub fn next(&self, timeout: Duration) -> Option<HubEvent> {
        let ev = self.rx.recv_timeout(timeout).ok()?;
        self.seen.lock().unwrap().push(ev.clone());
        Some(ev)
    }

    /// Reads events until `pick` returns something, or `timeout` passes.
    pub fn wait<T>(&self, timeout: Duration, mut pick: impl FnMut(&HubEvent) -> Option<T>) -> Option<T> {
        let end = Instant::now() + timeout;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            let ev = self.next(left)?;
            if let Some(t) = pick(&ev) {
                return Some(t);
            }
        }
    }

    /// The next service event of `id` within `timeout`.
    pub fn service(&self, id: ServiceId, timeout: Duration) -> Option<ServiceEvent> {
        self.wait(timeout, |e| match e {
            HubEvent::Service(s) if s.service == id => Some(s.clone()),
            _ => None,
        })
    }

    /// Waits for a service event with a value and returns it.
    pub fn value(&self, id: ServiceId, timeout: Duration) -> Option<ServiceEvent> {
        let end = Instant::now() + timeout;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            let ev = self.service(id, left)?;
            if ev.value.is_some() {
                return Some(ev);
            }
        }
    }

    /// Everything that arrives within `quiet` of the last event.
    pub fn drain(&self, quiet: Duration) -> Vec<HubEvent> {
        let mut out = Vec::new();
        while let Some(e) = self.next(quiet) {
            out.push(e);
        }
        out
    }

    /// Every service event of `id` seen so far plus what arrives within `quiet`.
    pub fn service_events(&self, id: ServiceId, quiet: Duration) -> Vec<ServiceEvent> {
        let _ = self.drain(quiet);
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                HubEvent::Service(s) if s.service == id => Some(s.clone()),
                _ => None,
            })
            .collect()
    }
}

pub fn hub_with(factory: Box<dyn ServiceFactory>, timing: HubTiming) -> (Hub, Collector) {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let hub = Hub::start_with(
        HubOptions::default(),
        factory,
        Box::new(move |e| {
            let _ = tx.lock().unwrap().send(e);
        }),
        timing,
    );
    (hub, Collector { rx, seen: Mutex::new(Vec::new()) })
}

pub fn fast_timing() -> HubTiming {
    HubTiming {
        stop_delay: Duration::from_millis(250),
        retry_first: Duration::from_millis(60),
        retry_max: Duration::from_millis(500),
    }
}

pub fn fake_hub(world: &FakeWorld) -> (Hub, Collector) {
    hub_with(Box::new(world.clone()), fast_timing())
}

pub fn subscribe(hub: &Hub, service: ServiceId, owner: u32, interval: Option<Duration>) {
    hub.port().send(HubRequest::Subscribe { service, owner: InstanceId(owner), interval });
}

pub fn unsubscribe(hub: &Hub, service: ServiceId, owner: u32) {
    hub.port().send(HubRequest::Unsubscribe { service, owner: InstanceId(owner) });
}

pub fn env_in(dir: &Path) -> PathEnv {
    PathEnv {
        home: dir.join("home"),
        appdata: dir.join("home").join("AppData").join("Roaming"),
        local_appdata: dir.join("home").join("AppData").join("Local"),
        temp: dir.join("home").join("AppData").join("Local").join("Temp"),
    }
}

/// A grant for widget `widget` living in `widget_dir`.
pub fn grant(widget: &str, widget_dir: &Path, permissions: Permissions) -> Arc<IoGrant> {
    Arc::new(IoGrant {
        widget: widget.to_string(),
        permissions,
        widget_dir: widget_dir.to_path_buf(),
        env: env_in(widget_dir.parent().unwrap_or(widget_dir)),
    })
}

pub fn perms(exec: &[&str], file: &[&str], net: &[&str]) -> Permissions {
    Permissions {
        exec: exec.iter().map(|s| s.to_string()).collect(),
        file: file.iter().map(|s| s.to_string()).collect(),
        net: net.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

pub fn io(hub: &Hub, id: u64, owner: u32, request: IoRequest, grant: &Arc<IoGrant>) {
    hub.port().send(HubRequest::Io { id: IoId(id), owner: InstanceId(owner), request, grant: grant.clone() });
}

/// The final event kind of request `id` (ExecDone, Http, File or Opened),
/// with the lines that came before it.
pub fn finish(col: &Collector, id: u64, timeout: Duration) -> Option<(Vec<IoEventKind>, IoEventKind)> {
    let mut before = Vec::new();
    col.wait(timeout, |e| match e {
        HubEvent::Io(ev) if ev.id == IoId(id) => match &ev.kind {
            IoEventKind::ExecLine { .. } => {
                before.push(ev.kind.clone());
                None
            }
            k => Some(k.clone()),
        },
        _ => None,
    })
    .map(|k| (before, k))
}

/// A recorder for log output across a test binary.
pub mod logs {
    use std::sync::Mutex;

    static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    struct Capture;

    impl log::Log for Capture {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, r: &log::Record) {
            LINES.lock().unwrap().push(format!("{} {}", r.level(), r.args()));
        }
        fn flush(&self) {}
    }

    pub fn install() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let _ = log::set_logger(&Capture);
            log::set_max_level(log::LevelFilter::Trace);
        });
    }

    pub fn all() -> Vec<String> {
        LINES.lock().unwrap().clone()
    }
}

/// Whether a process with this id is still running.
pub fn process_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    // SAFETY: plain flag and id; the handle is closed below.
    let Ok(h) = (unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }) else { return false };
    // SAFETY: `h` is a live process handle.
    let alive = unsafe { WaitForSingleObject(h, 0) } == WAIT_TIMEOUT;
    // SAFETY: closes the handle opened above.
    let _ = unsafe { CloseHandle(h) };
    alive
}

/// Polls `check` every 10 ms until it holds or `timeout` passes.
pub fn eventually(timeout: Duration, mut check: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + timeout;
    loop {
        if check() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A request to run the helper program with these arguments.
pub fn helper_spec(args: &[&str], timeout: Option<Duration>, max_output: usize) -> ExecSpec {
    ExecSpec {
        program: ProgramRef::Path(helper_exe()),
        args: args.iter().map(|s| s.to_string()).collect(),
        raw_tail: None,
        cwd: None,
        timeout,
        max_output,
    }
}

/// Permissions for exactly the helper program.
pub fn helper_perms() -> rsb_core::permissions::Permissions {
    perms(&[helper_exe().to_string_lossy().as_ref()], &[], &[])
}

pub fn get(url: &str) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Get,
        url: url.to_string(),
        headers: Vec::new(),
        body: None,
        timeout: Duration::from_secs(5),
        max_bytes: 1 << 20,
        cache_for: None,
    }
}

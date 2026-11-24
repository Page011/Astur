//! Script I/O: exec, HTTP, files and opening a URL. Requests arrive on the
//! hub thread, which only hands them over; everything that can wait runs on
//! worker threads. Every request carries the widget's grant, and what only
//! this crate can see (a redirect target, the final path of an opened file,
//! the program found on PATH) is checked against it again.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rsb_core::element::InstanceId;
use rsb_core::io::{IoError, IoEvent, IoEventKind, IoGrant, IoId, IoRequest};
use rsb_core::msg::HubEvent;
use rsb_core::permissions::parse_url;

use crate::Sink;

pub(crate) mod exec;
pub(crate) mod filereq;
pub(crate) mod http;

/// Opens a URL in the user's browser. The real one is `ShellExecuteExW`;
/// the fakes record the URL instead.
pub trait UrlOpener: Send + Sync {
    fn open(&self, url: &str) -> Result<(), String>;
}

pub(crate) fn send(sink: &Arc<Sink>, id: IoId, owner: InstanceId, kind: IoEventKind) {
    sink(HubEvent::Io(IoEvent { id, owner, kind }));
}

/// Routes requests to the exec, HTTP and file workers.
pub(crate) struct IoManager {
    sink: Arc<Sink>,
    exec: exec::ExecManager,
    http: http::HttpManager,
    files: filereq::FileManager,
    opener: Arc<dyn UrlOpener>,
    /// Open-URL workers still running, so shutdown can wait for them.
    openers: Arc<Mutex<u32>>,
}

impl IoManager {
    pub(crate) fn new(sink: Arc<Sink>, opener: Option<Arc<dyn UrlOpener>>) -> IoManager {
        IoManager {
            exec: exec::ExecManager::new(sink.clone()),
            http: http::HttpManager::new(sink.clone()),
            files: filereq::FileManager::new(sink.clone()),
            opener: opener.unwrap_or_else(|| Arc::new(ShellOpener)),
            openers: Arc::new(Mutex::new(0)),
            sink,
        }
    }

    /// Never blocks: checks, queues or spawns, and returns.
    pub(crate) fn request(&self, id: IoId, owner: InstanceId, request: IoRequest, grant: Arc<IoGrant>) {
        match request {
            IoRequest::ExecRun(spec) => self.exec.start(id, owner, spec, grant, false),
            IoRequest::ExecSpawn(spec) => self.exec.start(id, owner, spec, grant, true),
            IoRequest::Http(req) => self.http.start(id, owner, req, grant),
            IoRequest::ReadFile(req) => self.files.read(id, owner, req, grant),
            IoRequest::WatchFile(req) => self.files.watch(id, owner, req, grant),
            IoRequest::OpenUrl(url) => self.open_url(id, owner, &url, &grant),
        }
    }

    pub(crate) fn cancel(&self, id: IoId) {
        self.exec.cancel(id);
        self.http.cancel(id);
        self.files.cancel(id);
    }

    pub(crate) fn cancel_owner(&self, owner: InstanceId) {
        self.exec.cancel_owner(owner);
        self.http.cancel_owner(owner);
        self.files.cancel_owner(owner);
    }

    pub(crate) fn shutdown(&self) {
        self.exec.shutdown();
        self.http.shutdown();
        self.files.shutdown();
    }

    fn open_url(&self, id: IoId, owner: InstanceId, url: &str, grant: &IoGrant) {
        let result = (|| {
            let parsed = parse_url(url).map_err(IoError::BadRequest)?;
            if !parsed.https {
                return Err(IoError::BadRequest("only https:// addresses can be opened".into()));
            }
            rsb_core::permissions::open_url_text_ok(url).map_err(IoError::BadRequest)?;
            grant.permissions.check_net(&parsed).map_err(|need| IoError::NotPermitted(need.to_string()))
        })();
        if let Err(e) = result {
            send(&self.sink, id, owner, IoEventKind::Opened(Err(e)));
            return;
        }
        // ShellExecute can take a moment and needs an STA: its own short-lived thread.
        let sink = self.sink.clone();
        let opener = self.opener.clone();
        let url = url.to_string();
        let count = self.openers.clone();
        *count.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        let spawned = std::thread::Builder::new().name("rsb-open-url".into()).spawn(move || {
            let r = opener.open(&url).map_err(IoError::Io);
            send(&sink, id, owner, IoEventKind::Opened(r));
            *count.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        });
        if spawned.is_err() {
            send(&self.sink, id, owner, IoEventKind::Opened(Err(IoError::Busy)));
        }
    }
}

struct ShellOpener;

impl UrlOpener for ShellOpener {
    fn open(&self, url: &str) -> Result<(), String> {
        use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
        use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW};
        use windows::core::{PCWSTR, w};

        let wide_url = crate::win::wide(url);
        // SAFETY: initialises COM for this short-lived thread; undone below.
        let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOASYNC,
            lpVerb: w!("open"),
            lpFile: PCWSTR(wide_url.as_ptr()),
            nShow: 1,
            ..Default::default()
        };
        // SAFETY: `info` is fully initialised and `wide_url` outlives the call.
        let r = unsafe { ShellExecuteExW(&mut info) };
        if com.is_ok() {
            // SAFETY: balances the successful CoInitializeEx above.
            unsafe { CoUninitialize() };
        }
        r.map_err(|e| format!("the shell could not open the address ({e})"))
    }
}

/// Waits up to `limit` for `done` to become true, sleeping in short steps.
/// Used only at shutdown, where there is nothing to wait on.
pub(crate) fn wait_until(limit: Duration, done: impl Fn() -> bool) {
    let end = std::time::Instant::now() + limit;
    while !done() && std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
}

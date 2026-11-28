//! `rsb.http.get` / `post` over WinHTTP.
//!
//! WinHTTP is told to use the system proxy settings (a second session with
//! no proxy serves loopback) and the Windows certificate store, never to
//! follow redirects, keep cookies or send Windows credentials on its own.
//! Redirects are followed here: at most 5, every hop checked against the
//! widget's grant, never from https to http.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rsb_core::element::InstanceId;
use rsb_core::io::{HttpMethod, HttpRequest, HttpResponse, IoError, IoEventKind, IoGrant, IoId, limits};
use rsb_core::permissions::{ParsedUrl, parse_url};
use windows::Win32::Networking::WinHttp::{
    ERROR_WINHTTP_CANNOT_CONNECT, ERROR_WINHTTP_CONNECTION_ERROR, ERROR_WINHTTP_NAME_NOT_RESOLVED,
    ERROR_WINHTTP_OPERATION_CANCELLED, ERROR_WINHTTP_SECURE_FAILURE, ERROR_WINHTTP_TIMEOUT, URL_COMPONENTS,
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_ADDREQ_FLAG_ADD,
    WINHTTP_ADDREQ_FLAG_REPLACE, WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH, WINHTTP_DISABLE_AUTHENTICATION,
    WINHTTP_DISABLE_COOKIES, WINHTTP_DISABLE_REDIRECTS, WINHTTP_FLAG_SECURE, WINHTTP_OPEN_REQUEST_FLAGS,
    WINHTTP_OPTION_AUTOLOGON_POLICY, WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_OPTION_REDIRECT_POLICY,
    WINHTTP_OPTION_REDIRECT_POLICY_NEVER, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_RAW_HEADERS_CRLF,
    WINHTTP_QUERY_STATUS_CODE, WinHttpAddRequestHeaders, WinHttpCloseHandle, WinHttpConnect, WinHttpCrackUrl,
    WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpReadData,
    WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption, WinHttpSetTimeouts,
};
use windows::core::PCWSTR;

use super::send;
use crate::Sink;
use crate::win::wide;

/// A WinHTTP handle (session, connection or request).
struct Inet(*mut c_void);

// SAFETY: WinHTTP handles may be used and closed from any thread.
unsafe impl Send for Inet {}
// SAFETY: as above.
unsafe impl Sync for Inet {}

impl Drop for Inet {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the handle came from WinHTTP and is closed once.
            let _ = unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

struct Sessions {
    /// System proxy settings (automatic detection and the user's setting).
    proxied: Inet,
    /// No proxy: loopback addresses.
    direct: Inet,
}

fn open_session(access: windows::Win32::Networking::WinHttp::WINHTTP_ACCESS_TYPE) -> Option<Inet> {
    let agent = wide(&format!("rsb/{}", env!("CARGO_PKG_VERSION")));
    // SAFETY: `agent` is NUL-terminated and outlives the call.
    let h = unsafe { WinHttpOpen(PCWSTR(agent.as_ptr()), access, PCWSTR::null(), PCWSTR::null(), 0) };
    if h.is_null() { None } else { Some(Inet(h)) }
}

// ----------------------------------------------------------------- manager

struct Job {
    id: IoId,
    owner: InstanceId,
    req: HttpRequest,
    grant: Arc<IoGrant>,
    host_key: String,
    cancelled: Arc<AtomicBool>,
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    busy_hosts: HashSet<String>,
    /// Running jobs: owner, cancel flag and the handles to close on cancel.
    running: HashMap<IoId, Running>,
    started: bool,
}

struct Running {
    owner: InstanceId,
    cancelled: Arc<AtomicBool>,
    handles: Arc<Mutex<Vec<isize>>>,
    /// When the whole request (redirects included) must be over. WinHTTP's
    /// own timeouts are per phase and can run several times longer for the
    /// response headers, so the reaper thread enforces this one.
    deadline: Instant,
    timed_out: Arc<AtomicBool>,
}

struct Cached {
    at: Instant,
    response: HttpResponse,
}

#[derive(Default)]
struct Cache {
    map: HashMap<String, Cached>,
    order: VecDeque<String>,
    bytes: usize,
}

struct Failure {
    count: u32,
    until: Instant,
}

struct Shared {
    sink: Arc<Sink>,
    queue: Mutex<Queue>,
    wake: Condvar,
    /// Wakes the reaper: a job started running, or shutdown.
    reap_wake: Condvar,
    sessions: OnceLock<Option<Sessions>>,
    cache: Mutex<Cache>,
    backoff: Mutex<HashMap<(String, String), Failure>>,
    shutdown: AtomicBool,
    live_workers: Mutex<usize>,
}

pub(crate) struct HttpManager {
    shared: Arc<Shared>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn cache_key(widget: &str, req: &HttpRequest) -> String {
    let method = match req.method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
    };
    let mut headers: Vec<String> =
        req.headers.iter().map(|(k, v)| format!("{}:{}", k.to_ascii_lowercase(), v)).collect();
    headers.sort();
    format!("{widget}\n{method}\n{}\n{}", req.url, headers.join("\n"))
}

impl HttpManager {
    pub(crate) fn new(sink: Arc<Sink>) -> HttpManager {
        HttpManager {
            shared: Arc::new(Shared {
                sink,
                queue: Mutex::default(),
                wake: Condvar::new(),
                reap_wake: Condvar::new(),
                sessions: OnceLock::new(),
                cache: Mutex::default(),
                backoff: Mutex::default(),
                shutdown: AtomicBool::new(false),
                live_workers: Mutex::new(0),
            }),
        }
    }

    pub(crate) fn start(&self, id: IoId, owner: InstanceId, req: HttpRequest, grant: Arc<IoGrant>) {
        let sh = &self.shared;
        let fail = |e: IoError| send(&sh.sink, id, owner, IoEventKind::Http(Err(e)));
        let parsed = match parse_url(&req.url) {
            Ok(p) => p,
            Err(e) => return fail(IoError::BadRequest(e)),
        };
        if let Err(need) = grant.permissions.check_net(&parsed) {
            return fail(IoError::NotPermitted(need.to_string()));
        }
        if !parsed.https && !parsed.is_loopback() {
            return fail(IoError::NotPermitted(format!(
                "net:{} (plain http is only allowed to this computer)",
                parsed.authority()
            )));
        }
        if let Err(e) = rsb_core::io::check_headers(&req.headers) {
            return fail(IoError::BadRequest(e));
        }
        let mut req = req;
        req.timeout = req.timeout.clamp(Duration::from_millis(100), limits::HTTP_MAX_TIMEOUT);
        req.max_bytes = req.max_bytes.clamp(1, limits::HTTP_MAX_BYTES);

        // Served from the cache without touching the network.
        let key = cache_key(&grant.widget, &req);
        if let Some(max_age) = req.cache_for {
            let cache = lock(&sh.cache);
            if let Some(hit) = cache.map.get(&key)
                && hit.at.elapsed() < max_age
            {
                let mut response = hit.response.clone();
                response.from_cache = true;
                drop(cache);
                send(&sh.sink, id, owner, IoEventKind::Http(Ok(response)));
                return;
            }
        }
        // A URL that keeps failing is not retried until its delay is over.
        if let Some(f) = lock(&sh.backoff).get(&(grant.widget.clone(), req.url.clone()))
            && f.until > Instant::now()
        {
            return fail(IoError::Backoff { retry_in: f.until.saturating_duration_since(Instant::now()) });
        }

        let mut q = lock(&sh.queue);
        let mine = q.jobs.iter().filter(|j| j.owner == owner).count()
            + q.running.values().filter(|r| r.owner == owner).count();
        if mine >= limits::HTTP_MAX_IN_FLIGHT_PER_INSTANCE || sh.shutdown.load(Ordering::Acquire) {
            drop(q);
            return fail(IoError::Busy);
        }
        q.jobs.push_back(Job {
            id,
            owner,
            req,
            grant,
            host_key: parsed.authority(),
            cancelled: Arc::new(AtomicBool::new(false)),
        });
        if !q.started {
            q.started = true;
            drop(q);
            self.spawn_workers();
        } else {
            drop(q);
        }
        sh.wake.notify_one();
    }

    fn spawn_workers(&self) {
        for n in 0..limits::HTTP_WORKERS {
            let sh = self.shared.clone();
            *lock(&sh.live_workers) += 1;
            let r = std::thread::Builder::new().name(format!("rsb-http-{n}")).spawn({
                let sh = sh.clone();
                move || {
                    worker(&sh);
                    *lock(&sh.live_workers) -= 1;
                }
            });
            if r.is_err() {
                *lock(&self.shared.live_workers) -= 1;
            }
        }
        let sh = self.shared.clone();
        *lock(&sh.live_workers) += 1;
        let r = std::thread::Builder::new().name("rsb-http-reaper".into()).spawn({
            let sh = sh.clone();
            move || {
                reaper(&sh);
                *lock(&sh.live_workers) -= 1;
            }
        });
        if r.is_err() {
            *lock(&self.shared.live_workers) -= 1;
        }
    }

    pub(crate) fn cancel(&self, id: IoId) {
        let mut q = lock(&self.shared.queue);
        q.jobs.retain(|j| j.id != id);
        if let Some(r) = q.running.get(&id) {
            abort(r);
        }
    }

    pub(crate) fn cancel_owner(&self, owner: InstanceId) {
        let mut q = lock(&self.shared.queue);
        q.jobs.retain(|j| j.owner != owner);
        for r in q.running.values().filter(|r| r.owner == owner) {
            abort(r);
        }
    }

    pub(crate) fn shutdown(&self) {
        let sh = &self.shared;
        sh.shutdown.store(true, Ordering::Release);
        {
            let mut q = lock(&sh.queue);
            q.jobs.clear();
            for r in q.running.values() {
                abort(r);
            }
        }
        sh.wake.notify_all();
        sh.reap_wake.notify_all();
        super::wait_until(Duration::from_millis(800), || *lock(&sh.live_workers) == 0);
    }
}

/// The result of an abandoned request is dropped; closing its handles
/// makes the blocked WinHTTP call return at once.
fn abort(r: &Running) {
    r.cancelled.store(true, Ordering::Release);
    close_handles(r);
}

fn close_handles(r: &Running) {
    for h in lock(&r.handles).drain(..) {
        // SAFETY: a handle registered by the worker that has not been closed since (entries are removed when it closes them).
        let _ = unsafe { WinHttpCloseHandle(h as *mut c_void) };
    }
}

/// Closes the handles of any request past its deadline, which makes its
/// blocked WinHTTP call return; the worker then reports `Timeout`.
fn reaper(sh: &Arc<Shared>) {
    let mut q = lock(&sh.queue);
    loop {
        if sh.shutdown.load(Ordering::Acquire) {
            return;
        }
        let now = Instant::now();
        let mut next: Option<Instant> = None;
        for r in q.running.values() {
            if r.timed_out.load(Ordering::Acquire) {
                continue;
            }
            if r.deadline <= now {
                r.timed_out.store(true, Ordering::Release);
                close_handles(r);
            } else {
                next = Some(next.map_or(r.deadline, |n| n.min(r.deadline)));
            }
        }
        q = match next {
            Some(t) => {
                let wait = t.saturating_duration_since(Instant::now()) + Duration::from_millis(2);
                sh.reap_wake.wait_timeout(q, wait).unwrap_or_else(|e| e.into_inner()).0
            }
            None => sh.reap_wake.wait(q).unwrap_or_else(|e| e.into_inner()),
        };
    }
}

fn worker(sh: &Arc<Shared>) {
    loop {
        let (job, handles, timed_out, deadline) = {
            let mut q = lock(&sh.queue);
            loop {
                if sh.shutdown.load(Ordering::Acquire) {
                    return;
                }
                let pos = q.jobs.iter().position(|j| !q.busy_hosts.contains(&j.host_key));
                if let Some(i) = pos {
                    let job = q.jobs.remove(i).expect("position is in range");
                    q.busy_hosts.insert(job.host_key.clone());
                    // The job moves from the queue to the running set under
                    // this one lock: in between it would count towards no
                    // limit and a cancel would find nothing to stop.
                    let handles = Arc::new(Mutex::new(Vec::new()));
                    let timed_out = Arc::new(AtomicBool::new(false));
                    let deadline = Instant::now() + job.req.timeout;
                    q.running.insert(
                        job.id,
                        Running {
                            owner: job.owner,
                            cancelled: job.cancelled.clone(),
                            handles: handles.clone(),
                            deadline,
                            timed_out: timed_out.clone(),
                        },
                    );
                    break (job, handles, timed_out, deadline);
                }
                q = sh.wake.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        sh.reap_wake.notify_one();
        let mut result = if job.cancelled.load(Ordering::Acquire) {
            Err(IoError::Cancelled)
        } else {
            perform(sh, &job, &handles, deadline)
        };
        if timed_out.load(Ordering::Acquire) && !job.cancelled.load(Ordering::Acquire) {
            result = Err(IoError::Timeout);
        }
        {
            let mut q = lock(&sh.queue);
            q.running.remove(&job.id);
            q.busy_hosts.remove(&job.host_key);
        }
        sh.wake.notify_all();
        record(sh, &job, &result);
        if !job.cancelled.load(Ordering::Acquire) {
            send(&sh.sink, job.id, job.owner, IoEventKind::Http(result));
        }
    }
}

/// Cache a good GET answer; count the outcome for the backoff.
fn record(sh: &Shared, job: &Job, result: &Result<HttpResponse, IoError>) {
    let key = (job.grant.widget.clone(), job.req.url.clone());
    let failed = match result {
        Ok(r) => r.status >= 500,
        Err(IoError::Network(_) | IoError::Timeout) => true,
        Err(_) => false,
    };
    let mut backoff = lock(&sh.backoff);
    if failed {
        let f = backoff.entry(key).or_insert(Failure { count: 0, until: Instant::now() });
        f.count += 1;
        let delay = Duration::from_secs(1u64 << (f.count - 1).min(9)).min(limits::HTTP_MAX_BACKOFF);
        f.until = Instant::now() + delay;
    } else if !matches!(result, Err(IoError::Cancelled)) {
        backoff.remove(&key);
    }
    drop(backoff);

    if let (Ok(r), HttpMethod::Get, Some(_)) = (result, job.req.method, job.req.cache_for)
        && (200..300).contains(&r.status)
    {
        let size = r.body.len();
        if size > limits::HTTP_CACHE_MAX_BYTES / 2 {
            return;
        }
        let k = cache_key(&job.grant.widget, &job.req);
        let mut c = lock(&sh.cache);
        if let Some(old) = c.map.remove(&k) {
            c.bytes -= old.response.body.len();
            c.order.retain(|o| o != &k);
        }
        c.bytes += size;
        c.order.push_back(k.clone());
        c.map.insert(k, Cached { at: Instant::now(), response: r.clone() });
        while c.map.len() > limits::HTTP_CACHE_MAX_ENTRIES || c.bytes > limits::HTTP_CACHE_MAX_BYTES {
            let Some(oldest) = c.order.pop_front() else { break };
            if let Some(old) = c.map.remove(&oldest) {
                c.bytes -= old.response.body.len();
            }
        }
    }
}

// ----------------------------------------------------------------- request

struct Raw {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn perform(
    sh: &Shared,
    job: &Job,
    handles: &Arc<Mutex<Vec<isize>>>,
    deadline: Instant,
) -> Result<HttpResponse, IoError> {
    let sessions = sh
        .sessions
        .get_or_init(|| {
            let proxied = open_session(WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY)
                .or_else(|| open_session(windows::Win32::Networking::WinHttp::WINHTTP_ACCESS_TYPE_DEFAULT_PROXY))?;
            Some(Sessions { proxied, direct: open_session(WINHTTP_ACCESS_TYPE_NO_PROXY)? })
        })
        .as_ref()
        .ok_or_else(|| IoError::Network("the system HTTP service could not be opened".into()))?;

    let mut url = parse_url(&job.req.url).map_err(IoError::BadRequest)?;
    let mut method = job.req.method;
    let mut body = job.req.body.clone();
    let mut headers = job.req.headers.clone();
    let mut hops = 0;
    loop {
        if job.cancelled.load(Ordering::Acquire) {
            return Err(IoError::Cancelled);
        }
        let session = if url.is_loopback() { &sessions.direct } else { &sessions.proxied };
        let raw = exchange(session, &url, method, &headers, body.as_deref(), deadline, job.req.max_bytes, handles)?;
        let redirect = matches!(raw.status, 301 | 302 | 303 | 307 | 308);
        let location = raw.headers.iter().find(|(k, _)| k == "location").map(|(_, v)| v.clone());
        let (true, Some(location)) = (redirect, location) else {
            return Ok(HttpResponse { status: raw.status, headers: raw.headers, body: raw.body, from_cache: false });
        };
        hops += 1;
        if hops > limits::HTTP_MAX_REDIRECTS {
            return Err(IoError::Network(format!("more than {} redirects", limits::HTTP_MAX_REDIRECTS)));
        }
        let next_text = resolve_location(&url, &location);
        let next = parse_url(&next_text).map_err(|e| IoError::BadRequest(format!("redirect: {e}")))?;
        check_hop(&url, &next, &job.grant)?;
        if next.authority() != url.authority() {
            // Whatever the widget added (tokens, keys) stays with the host it was meant for.
            headers.clear();
        }
        if raw.status == 303 || (matches!(raw.status, 301 | 302) && method == HttpMethod::Post) {
            method = HttpMethod::Get;
            body = None;
        }
        url = next;
    }
}

/// Whether a redirect from `from` to `to` may be followed: never from https
/// to http, only to hosts the grant names, and plain http only on this
/// computer.
fn check_hop(from: &ParsedUrl, to: &ParsedUrl, grant: &IoGrant) -> Result<(), IoError> {
    if from.https && !to.https {
        return Err(IoError::NotPermitted(format!("net:{} (a redirect from https to http)", to.authority())));
    }
    if let Err(need) = grant.permissions.check_net(to) {
        return Err(IoError::NotPermitted(need.to_string()));
    }
    if !to.https && !to.is_loopback() {
        return Err(IoError::NotPermitted(format!(
            "net:{} (plain http is only allowed to this computer)",
            to.authority()
        )));
    }
    Ok(())
}

/// An absolute URL for a `Location` value seen at `base`.
pub(crate) fn resolve_location(base: &ParsedUrl, location: &str) -> String {
    let scheme = if base.https { "https" } else { "http" };
    let authority = if base.explicit_port { format!("{}:{}", base.host, base.port) } else { base.host.clone() };
    if location.starts_with("https://") || location.starts_with("http://") {
        location.to_string()
    } else if let Some(rest) = location.strip_prefix("//") {
        format!("{scheme}://{rest}")
    } else if location.starts_with('/') {
        format!("{scheme}://{authority}{location}")
    } else {
        let path = base.path_and_query.split('?').next().unwrap_or("/");
        let dir = &path[..path.rfind('/').map_or(0, |i| i + 1)];
        format!("{scheme}://{authority}{dir}{location}")
    }
}

fn map_error(code: u32) -> IoError {
    match code {
        c if c == ERROR_WINHTTP_TIMEOUT => IoError::Timeout,
        c if c == ERROR_WINHTTP_NAME_NOT_RESOLVED => IoError::Network("the name could not be resolved".into()),
        c if c == ERROR_WINHTTP_CANNOT_CONNECT => IoError::Network("could not connect".into()),
        c if c == ERROR_WINHTTP_CONNECTION_ERROR => IoError::Network("the connection was lost".into()),
        c if c == ERROR_WINHTTP_SECURE_FAILURE => IoError::Network("the secure connection failed".into()),
        c if c == ERROR_WINHTTP_OPERATION_CANCELLED => IoError::Cancelled,
        c => IoError::Network(format!("WinHTTP error {c}")),
    }
}

fn last_error(e: &windows::core::Error) -> IoError {
    map_error((e.code().0 & 0xFFFF) as u32)
}

/// Registers a handle so a cancel can close it; closes and unregisters on drop.
struct Tracked<'a> {
    inet: Inet,
    handles: &'a Arc<Mutex<Vec<isize>>>,
}

impl<'a> Tracked<'a> {
    fn new(inet: Inet, handles: &'a Arc<Mutex<Vec<isize>>>) -> Tracked<'a> {
        lock(handles).push(inet.0 as isize);
        Tracked { inet, handles }
    }
}

impl Drop for Tracked<'_> {
    fn drop(&mut self) {
        // If a cancel already closed it, the entry is gone: do not close twice.
        let mut h = lock(self.handles);
        let key = self.inet.0 as isize;
        if let Some(i) = h.iter().position(|x| *x == key) {
            h.remove(i);
        } else {
            self.inet.0 = std::ptr::null_mut();
        }
    }
}

fn millis(d: Duration) -> i32 {
    d.as_millis().min(i32::MAX as u128) as i32
}

#[allow(clippy::too_many_arguments)]
fn exchange(
    session: &Inet,
    url: &ParsedUrl,
    method: HttpMethod,
    headers: &[(String, String)],
    body: Option<&[u8]>,
    deadline: Instant,
    max_bytes: usize,
    handles: &Arc<Mutex<Vec<isize>>>,
) -> Result<Raw, IoError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(IoError::Timeout);
    }
    check_cracked(url)?;
    let host = url.host.trim_start_matches('[').trim_end_matches(']');
    let whost = wide(host);
    // SAFETY: the session handle is live and `whost` is NUL-terminated.
    let connect = unsafe { WinHttpConnect(session.0, PCWSTR(whost.as_ptr()), url.port, 0) };
    if connect.is_null() {
        return Err(last_os_error());
    }
    let connect = Tracked::new(Inet(connect), handles);

    let verb = wide(match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
    });
    let path = wide(&url.path_and_query);
    let flags = if url.https { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
    // SAFETY: the connection handle is live; the strings are NUL-terminated; no accept types are passed.
    let request = unsafe {
        WinHttpOpenRequest(
            connect.inet.0,
            PCWSTR(verb.as_ptr()),
            PCWSTR(path.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            flags,
        )
    };
    if request.is_null() {
        return Err(last_os_error());
    }
    let request = Tracked::new(Inet(request), handles);
    let req = request.inet.0;

    // No redirects, no cookies, no automatic Windows credentials. The autologon
    // policy alone is not enough: WinHTTP ignores it for IP addresses and
    // `localhost`, so authentication is switched off outright as well.
    let disable = (WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_AUTHENTICATION).to_le_bytes();
    let never = WINHTTP_OPTION_REDIRECT_POLICY_NEVER.to_le_bytes();
    let autologon = WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH.to_le_bytes();
    // SAFETY: `req` is a live request handle; each buffer is a 4-byte DWORD as these options require.
    unsafe {
        WinHttpSetOption(Some(req), WINHTTP_OPTION_DISABLE_FEATURE, Some(&disable)).map_err(|e| last_error(&e))?;
        WinHttpSetOption(Some(req), WINHTTP_OPTION_REDIRECT_POLICY, Some(&never)).map_err(|e| last_error(&e))?;
        WinHttpSetOption(Some(req), WINHTTP_OPTION_AUTOLOGON_POLICY, Some(&autologon)).map_err(|e| last_error(&e))?;
    }
    let ms = millis(remaining);
    // SAFETY: `req` is a live request handle.
    unsafe { WinHttpSetTimeouts(req, ms, ms, ms, ms) }.map_err(|e| last_error(&e))?;

    if !headers.is_empty() {
        let text: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
        let w: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: `req` is a live request handle and `w` a UTF-16 slice.
        unsafe { WinHttpAddRequestHeaders(req, &w, WINHTTP_ADDREQ_FLAG_ADD | WINHTTP_ADDREQ_FLAG_REPLACE) }
            .map_err(|e| last_error(&e))?;
    }

    let (data, len) = match body {
        Some(b) => (Some(b.as_ptr().cast::<c_void>()), b.len() as u32),
        None => (None, 0),
    };
    // SAFETY: `req` is live; `data` points at `len` bytes that outlive the (synchronous) call.
    unsafe { WinHttpSendRequest(req, None, data, len, len, 0) }.map_err(|e| last_error(&e))?;
    // SAFETY: `req` is live and the request was sent.
    unsafe { WinHttpReceiveResponse(req, std::ptr::null_mut()) }.map_err(|e| last_error(&e))?;

    let mut status = 0u32;
    let mut size = 4u32;
    // SAFETY: `status` is a 4-byte out buffer as WINHTTP_QUERY_FLAG_NUMBER requires.
    unsafe {
        WinHttpQueryHeaders(
            req,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&raw mut status).cast()),
            &mut size,
            std::ptr::null_mut(),
        )
    }
    .map_err(|e| last_error(&e))?;
    let headers = read_headers(req)?;

    if let Some((_, len)) = headers.iter().find(|(k, _)| k == "content-length")
        && len.trim().parse::<u64>().is_ok_and(|n| n > max_bytes as u64)
    {
        return Err(IoError::TooLarge { limit: max_bytes });
    }
    let mut body = Vec::new();
    loop {
        if Instant::now() >= deadline {
            return Err(IoError::Timeout);
        }
        let mut available = 0u32;
        // SAFETY: `req` is live and `available` a live out pointer.
        unsafe { WinHttpQueryDataAvailable(req, &mut available) }.map_err(|e| last_error(&e))?;
        if available == 0 {
            break;
        }
        let take = (available as usize).min(64 * 1024);
        if body.len() + take > max_bytes + 64 * 1024 {
            return Err(IoError::TooLarge { limit: max_bytes });
        }
        let at = body.len();
        body.resize(at + take, 0);
        let mut got = 0u32;
        // SAFETY: the buffer slice is `take` writable bytes inside `body`.
        unsafe { WinHttpReadData(req, body[at..].as_mut_ptr().cast(), take as u32, &mut got) }
            .map_err(|e| last_error(&e))?;
        body.truncate(at + got as usize);
        if body.len() > max_bytes {
            return Err(IoError::TooLarge { limit: max_bytes });
        }
        if got == 0 {
            break;
        }
    }
    drop(request);
    drop(connect);
    Ok(Raw { status: status as u16, headers, body })
}

fn last_os_error() -> IoError {
    map_error(std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32)
}

/// The system's reading of the URL must be the one we checked.
fn check_cracked(url: &ParsedUrl) -> Result<(), IoError> {
    let scheme = if url.https { "https" } else { "http" };
    let authority = if url.explicit_port { format!("{}:{}", url.host, url.port) } else { url.host.clone() };
    let text = format!("{scheme}://{authority}{}", url.path_and_query);
    let w: Vec<u16> = text.encode_utf16().collect();
    let mut host = [0u16; 256];
    let mut parts = URL_COMPONENTS {
        dwStructSize: size_of::<URL_COMPONENTS>() as u32,
        lpszHostName: windows::core::PWSTR(host.as_mut_ptr()),
        dwHostNameLength: host.len() as u32,
        ..Default::default()
    };
    // SAFETY: `w` is a valid UTF-16 slice; the component buffer outlives the call.
    unsafe { WinHttpCrackUrl(&w, 0, &mut parts) }
        .map_err(|_| IoError::BadRequest("the system cannot read this address".into()))?;
    let cracked = String::from_utf16_lossy(&host[..parts.dwHostNameLength as usize]).to_ascii_lowercase();
    let wanted = url.host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    if cracked != wanted || parts.nPort != url.port {
        return Err(IoError::BadRequest("the address is read differently by the system; refused".into()));
    }
    Ok(())
}

fn read_headers(req: *mut c_void) -> Result<Vec<(String, String)>, IoError> {
    let mut size = 0u32;
    // SAFETY: a size query with no buffer; it fails with ERROR_INSUFFICIENT_BUFFER and sets `size` in bytes.
    let _ = unsafe {
        WinHttpQueryHeaders(req, WINHTTP_QUERY_RAW_HEADERS_CRLF, PCWSTR::null(), None, &mut size, std::ptr::null_mut())
    };
    if size == 0 {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u16; (size as usize).div_ceil(2)];
    // SAFETY: `buf` holds at least `size` bytes.
    unsafe {
        WinHttpQueryHeaders(
            req,
            WINHTTP_QUERY_RAW_HEADERS_CRLF,
            PCWSTR::null(),
            Some(buf.as_mut_ptr().cast()),
            &mut size,
            std::ptr::null_mut(),
        )
    }
    .map_err(|e| last_error(&e))?;
    let text = String::from_utf16_lossy(&buf[..(size as usize / 2).min(buf.len())]);
    let mut out = Vec::new();
    // The first line is the status line.
    for line in text.split("\r\n").skip(1) {
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().to_ascii_lowercase();
            if !k.is_empty() {
                out.push((k, v.trim().to_string()));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use rsb_core::permissions::{PathEnv, Permissions};

    use super::*;

    fn grant(net: &[&str]) -> IoGrant {
        IoGrant {
            widget: "t/w".into(),
            permissions: Permissions { net: net.iter().map(|s| s.to_string()).collect(), ..Default::default() },
            widget_dir: "C:\\w".into(),
            env: PathEnv {
                home: "C:\\h".into(),
                appdata: "C:\\a".into(),
                local_appdata: "C:\\l".into(),
                temp: "C:\\t".into(),
            },
        }
    }

    fn url(u: &str) -> ParsedUrl {
        parse_url(u).expect("a valid test address")
    }

    #[test]
    fn a_redirect_from_https_to_http_is_refused_even_to_a_listed_host() {
        let g = grant(&["api.example.com", "127.0.0.1:9000"]);
        let e = check_hop(&url("https://api.example.com/a"), &url("http://127.0.0.1:9000/b"), &g).unwrap_err();
        assert!(matches!(&e, IoError::NotPermitted(n) if n.contains("https to http")), "{e:?}");
    }

    #[test]
    fn a_redirect_follows_the_grant_only() {
        let g = grant(&["api.example.com", "*.cdn.example.com", "127.0.0.1:9000"]);
        let from = url("https://api.example.com/a");
        assert!(check_hop(&from, &url("https://api.example.com/b"), &g).is_ok());
        assert!(check_hop(&from, &url("https://img.cdn.example.com/b"), &g).is_ok());
        assert!(check_hop(&from, &url("https://example.com/b"), &g).is_err(), "a wildcard is not the bare domain");
        assert!(
            matches!(check_hop(&from, &url("https://evil.example/b"), &g), Err(IoError::NotPermitted(n)) if n == "net:evil.example")
        );
        assert!(check_hop(&url("http://127.0.0.1:9000/"), &url("http://127.0.0.1:9000/x"), &g).is_ok());
        assert!(
            check_hop(&url("http://127.0.0.1:9000/"), &url("http://127.0.0.1:9001/x"), &g).is_err(),
            "another port is another entry"
        );
    }

    #[test]
    fn locations_resolve_against_the_current_address() {
        let base = url("https://api.example.com/v1/items?x=1");
        assert_eq!(resolve_location(&base, "https://other.example/z"), "https://other.example/z");
        assert_eq!(resolve_location(&base, "//other.example/z"), "https://other.example/z");
        assert_eq!(resolve_location(&base, "/root"), "https://api.example.com/root");
        assert_eq!(resolve_location(&base, "next"), "https://api.example.com/v1/next");
        let local = url("http://127.0.0.1:8080/a/b");
        assert_eq!(resolve_location(&local, "/c"), "http://127.0.0.1:8080/c");
    }

    #[test]
    fn the_cache_key_separates_widgets_methods_urls_and_headers() {
        let req = |method, url: &str, headers: &[(&str, &str)]| HttpRequest {
            method,
            url: url.to_string(),
            headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            body: None,
            timeout: Duration::from_secs(1),
            max_bytes: 1,
            cache_for: None,
        };
        let base = cache_key("a", &req(HttpMethod::Get, "https://x.example/p", &[]));
        assert_ne!(base, cache_key("b", &req(HttpMethod::Get, "https://x.example/p", &[])));
        assert_ne!(base, cache_key("a", &req(HttpMethod::Post, "https://x.example/p", &[])));
        assert_ne!(base, cache_key("a", &req(HttpMethod::Get, "https://x.example/q", &[])));
        assert_ne!(base, cache_key("a", &req(HttpMethod::Get, "https://x.example/p", &[("X-K", "1")])));
        let one = cache_key("a", &req(HttpMethod::Get, "https://x.example/p", &[("A", "1"), ("B", "2")]));
        let two = cache_key("a", &req(HttpMethod::Get, "https://x.example/p", &[("b", "2"), ("a", "1")]));
        assert_eq!(one, two, "header order and name case do not matter");
    }
}

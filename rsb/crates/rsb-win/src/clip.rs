//! The clipboard worker.
//!
//! Reading or writing the clipboard can block for as long as the app that
//! owns it likes, so it never happens on the UI thread. One worker thread
//! takes one request at a time; the UI thread gets an answer within
//! `ANSWER_LIMIT` whatever the worker is doing (`None` when it was too
//! slow). While a request is stuck inside another app no second worker is
//! started: later requests are answered `None` at once until the stuck one
//! returns.
//!
//! The Win32 calls sit behind the small `ClipApi` trait so the worker's rules
//! are tested with a fake: the real clipboard is the user's, and even
//! writing a private format empties it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
};
use windows::core::w;

/// The UI thread has an answer this long after asking, at the latest.
pub(crate) const ANSWER_LIMIT: Duration = Duration::from_millis(500);
/// Opening the clipboard fails while another app has it open: try this often,
/// this far apart.
pub(crate) const OPEN_TRIES: u32 = 5;
pub(crate) const OPEN_GAP: Duration = Duration::from_millis(10);
/// Longest text read from or written to the clipboard, in UTF-16 units.
const MAX_UNITS: usize = 4 * 1024 * 1024;

/// What the worker needs from the system.
pub(crate) trait ClipApi {
    /// Opens the clipboard; false while another app holds it.
    fn open(&mut self) -> bool;
    fn close(&mut self);
    /// The clipboard's text (only while open); None when it holds none.
    fn read_text(&mut self) -> Option<String>;
    /// Replaces the clipboard's contents with `text` (only while open).
    fn write_text(&mut self, text: &str) -> bool;
}

fn open_with_retries(api: &mut dyn ClipApi) -> bool {
    for attempt in 0..OPEN_TRIES {
        if api.open() {
            return true;
        }
        if attempt + 1 < OPEN_TRIES {
            std::thread::sleep(OPEN_GAP);
        }
    }
    false
}

fn do_read(api: &mut dyn ClipApi) -> Option<String> {
    if !open_with_retries(api) {
        return None;
    }
    let text = api.read_text();
    api.close();
    text
}

fn do_write(api: &mut dyn ClipApi, text: &str) -> bool {
    if !open_with_retries(api) {
        return false;
    }
    let ok = api.write_text(text);
    api.close();
    ok
}

enum Job {
    Read { token: u64, deadline: Instant },
    Write { text: String, deadline: Instant },
}

struct Shared {
    /// Finished reads: token and text.
    done: Mutex<VecDeque<(u64, Option<String>)>>,
    /// When the request being worked on started.
    busy_since: Mutex<Option<Instant>>,
    /// Tells the UI thread something finished (posts a message).
    notify: Box<dyn Fn() + Send + Sync>,
    /// Workers started, for tests.
    spawned: AtomicUsize,
}

type Factory = Box<dyn FnOnce() -> Box<dyn ClipApi> + Send>;

pub(crate) struct ClipClient {
    shared: Arc<Shared>,
    make: Box<dyn Fn() -> Factory + Send>,
    tx: Option<Sender<Job>>,
    join: Option<JoinHandle<()>>,
    /// Reads asked for and not yet answered: token and answer-by time.
    waiting: Vec<(u64, Instant)>,
    limit: Duration,
}

impl ClipClient {
    /// `make` builds the factory each worker runs on its own thread (the
    /// real one creates a message-only window there).
    pub(crate) fn new(make: Box<dyn Fn() -> Factory + Send>, notify: Box<dyn Fn() + Send + Sync>) -> ClipClient {
        ClipClient {
            shared: Arc::new(Shared {
                done: Mutex::new(VecDeque::new()),
                busy_since: Mutex::new(None),
                notify,
                spawned: AtomicUsize::new(0),
            }),
            make,
            tx: None,
            join: None,
            waiting: Vec::new(),
            limit: ANSWER_LIMIT,
        }
    }

    /// The system clipboard.
    pub(crate) fn real(notify: Box<dyn Fn() + Send + Sync>) -> ClipClient {
        ClipClient::new(Box::new(|| Box::new(|| Box::new(RealClip::new()) as Box<dyn ClipApi>)), notify)
    }

    #[cfg(test)]
    pub(crate) fn with_limit(mut self, limit: Duration) -> ClipClient {
        self.limit = limit;
        self
    }

    #[cfg(test)]
    pub(crate) fn workers_started(&self) -> usize {
        self.shared.spawned.load(Ordering::SeqCst)
    }

    fn is_stuck(&self, now: Instant) -> bool {
        let since = self.shared.busy_since.lock().map(|g| *g).unwrap_or(None);
        since.is_some_and(|t| now.saturating_duration_since(t) > self.limit)
    }

    fn worker_alive(&self) -> bool {
        self.tx.is_some() && self.join.as_ref().is_some_and(|j| !j.is_finished())
    }

    /// Starts the worker unless one runs. Never called while one is stuck
    /// (a stuck worker is alive).
    fn ensure_worker(&mut self) -> bool {
        if self.worker_alive() {
            return true;
        }
        let (tx, rx) = mpsc::channel::<Job>();
        let shared = Arc::clone(&self.shared);
        let factory = (self.make)();
        let spawned =
            std::thread::Builder::new().name("rsb-clipboard".into()).spawn(move || worker(factory, rx, shared));
        match spawned {
            Ok(j) => {
                self.shared.spawned.fetch_add(1, Ordering::SeqCst);
                self.tx = Some(tx);
                self.join = Some(j);
                true
            }
            Err(e) => {
                log::warn!("clipboard worker did not start: {e}");
                false
            }
        }
    }

    /// Asks for the clipboard's text. Answers that are already known (the
    /// worker is stuck) come back at once; the rest arrive from `poll`.
    pub(crate) fn read(&mut self, token: u64, now: Instant) -> Vec<(u64, Option<String>)> {
        if self.is_stuck(now) || !self.ensure_worker() {
            return vec![(token, None)];
        }
        let deadline = now + self.limit;
        let sent = self.tx.as_ref().is_some_and(|tx| tx.send(Job::Read { token, deadline }).is_ok());
        if !sent {
            return vec![(token, None)];
        }
        self.waiting.push((token, deadline));
        Vec::new()
    }

    /// Puts text on the clipboard; dropped while the worker is stuck.
    pub(crate) fn write(&mut self, text: String, now: Instant) {
        if self.is_stuck(now) || !self.ensure_worker() {
            log::debug!("clipboard write dropped: the worker is busy");
            return;
        }
        let deadline = now + self.limit;
        if let Some(tx) = &self.tx {
            let _ = tx.send(Job::Write { text, deadline });
        }
    }

    /// Answers: finished reads, and `None` for those past their time.
    pub(crate) fn poll(&mut self, now: Instant) -> Vec<(u64, Option<String>)> {
        let mut out = Vec::new();
        let finished: Vec<(u64, Option<String>)> =
            self.shared.done.lock().map(|mut q| q.drain(..).collect()).unwrap_or_default();
        for (token, text) in finished {
            if let Some(i) = self.waiting.iter().position(|(t, _)| *t == token) {
                self.waiting.remove(i);
                out.push((token, text));
            }
        }
        let mut i = 0;
        while i < self.waiting.len() {
            if self.waiting[i].1 <= now {
                out.push((self.waiting.remove(i).0, None));
            } else {
                i += 1;
            }
        }
        out
    }

    /// When `poll` must run next to keep the 500 ms promise.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.waiting.iter().map(|(_, d)| *d).min()
    }
}

impl Drop for ClipClient {
    fn drop(&mut self) {
        // Closing the channel ends an idle worker; a stuck one is left to end
        // on its own (joining it could block for as long as the other app).
        self.tx = None;
        if let Some(j) = self.join.take()
            && j.is_finished()
        {
            let _ = j.join();
        }
    }
}

fn worker(make: Factory, rx: Receiver<Job>, shared: Arc<Shared>) {
    let mut api = make();
    while let Ok(job) = rx.recv() {
        let started = Instant::now();
        let deadline = match &job {
            Job::Read { deadline, .. } | Job::Write { deadline, .. } => *deadline,
        };
        // Asked for too long ago to matter: the UI thread already answered.
        if deadline <= started {
            continue;
        }
        if let Ok(mut b) = shared.busy_since.lock() {
            *b = Some(started);
        }
        match job {
            Job::Read { token, .. } => {
                let text = do_read(api.as_mut());
                if let Ok(mut q) = shared.done.lock() {
                    q.push_back((token, text));
                }
            }
            Job::Write { text, .. } => {
                if !do_write(api.as_mut(), &text) {
                    log::debug!("clipboard write failed");
                }
            }
        }
        if let Ok(mut b) = shared.busy_since.lock() {
            *b = None;
        }
        (shared.notify)();
    }
}

/// The system clipboard, opened with a message-only window of the worker's
/// own: `SetClipboardData` after `EmptyClipboard` needs an owner window.
struct RealClip {
    hwnd: HWND,
    is_open: bool,
}

impl RealClip {
    fn new() -> RealClip {
        // SAFETY: the predefined class needs no registration; the parent is
        // the message-only root; the module handle is this process's.
        let hwnd = unsafe {
            let hinst = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!(""),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(hinst),
                None,
            )
        }
        .unwrap_or_default();
        RealClip { hwnd, is_open: false }
    }
}

impl Drop for RealClip {
    fn drop(&mut self) {
        if self.is_open {
            self.close();
        }
        if !self.hwnd.0.is_null() {
            // SAFETY: the window belongs to this thread.
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

impl ClipApi for RealClip {
    fn open(&mut self) -> bool {
        let owner = (!self.hwnd.0.is_null()).then_some(self.hwnd);
        // SAFETY: the owner is our window or none.
        let ok = unsafe { OpenClipboard(owner) }.is_ok();
        self.is_open = ok;
        ok
    }

    fn close(&mut self) {
        // SAFETY: closes the clipboard this thread opened.
        unsafe {
            let _ = CloseClipboard();
        }
        self.is_open = false;
    }

    fn read_text(&mut self) -> Option<String> {
        // SAFETY: the clipboard is open on this thread; the memory behind the
        // handle stays valid until it is closed, and is locked while read.
        unsafe {
            IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).ok()?;
            let h = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let mem = HGLOBAL(h.0);
            let size = GlobalSize(mem);
            let p = GlobalLock(mem) as *const u16;
            if p.is_null() {
                return None;
            }
            let units = (size / 2).min(MAX_UNITS);
            let slice = std::slice::from_raw_parts(p, units);
            let len = slice.iter().position(|&u| u == 0).unwrap_or(units);
            let text = String::from_utf16_lossy(&slice[..len]);
            let _ = GlobalUnlock(mem);
            Some(text)
        }
    }

    fn write_text(&mut self, text: &str) -> bool {
        if self.hwnd.0.is_null() {
            return false;
        }
        let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        if units.len() > MAX_UNITS {
            return false;
        }
        let bytes = units.len() * 2;
        // SAFETY: the memory is allocated here, filled while locked and then
        // either handed to the clipboard (which owns it on success) or freed.
        unsafe {
            let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, bytes) else { return false };
            let p = GlobalLock(mem) as *mut u16;
            if p.is_null() {
                let _ = windows::Win32::Foundation::GlobalFree(Some(mem));
                return false;
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), p, units.len());
            let _ = GlobalUnlock(mem);
            if EmptyClipboard().is_err() {
                let _ = windows::Win32::Foundation::GlobalFree(Some(mem));
                return false;
            }
            if SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(mem.0))).is_err() {
                let _ = windows::Win32::Foundation::GlobalFree(Some(mem));
                return false;
            }
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::{Condvar, Mutex};

    use super::*;

    /// A clipboard with a text, counting opens, optionally refusing the
    /// first `refuse` opens, optionally blocking reads until released.
    struct Fake {
        text: Arc<Mutex<Option<String>>>,
        opens: Arc<AtomicUsize>,
        refuse: usize,
        gate: Option<Arc<(Mutex<bool>, Condvar)>>,
        reading: Arc<AtomicBool>,
    }

    impl ClipApi for Fake {
        fn open(&mut self) -> bool {
            let n = self.opens.fetch_add(1, Ordering::SeqCst);
            n >= self.refuse
        }
        fn close(&mut self) {}
        fn read_text(&mut self) -> Option<String> {
            self.reading.store(true, Ordering::SeqCst);
            if let Some(g) = &self.gate {
                let (lock, cv) = &**g;
                let mut open = lock.lock().unwrap();
                while !*open {
                    open = cv.wait(open).unwrap();
                }
            }
            self.reading.store(false, Ordering::SeqCst);
            self.text.lock().unwrap().clone()
        }
        fn write_text(&mut self, text: &str) -> bool {
            *self.text.lock().unwrap() = Some(text.to_string());
            true
        }
    }

    struct Rig {
        client: ClipClient,
        text: Arc<Mutex<Option<String>>>,
        opens: Arc<AtomicUsize>,
        gate: Arc<(Mutex<bool>, Condvar)>,
        reading: Arc<AtomicBool>,
        notified: Arc<AtomicUsize>,
    }

    fn rig(refuse: usize, blocked: bool, limit: Duration) -> Rig {
        let text = Arc::new(Mutex::new(Some("hello".to_string())));
        let opens = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new((Mutex::new(!blocked), Condvar::new()));
        let reading = Arc::new(AtomicBool::new(false));
        let notified = Arc::new(AtomicUsize::new(0));
        let (t, o, g, r) = (Arc::clone(&text), Arc::clone(&opens), Arc::clone(&gate), Arc::clone(&reading));
        let n = Arc::clone(&notified);
        let client = ClipClient::new(
            Box::new(move || {
                let (t, o, g, r) = (Arc::clone(&t), Arc::clone(&o), Arc::clone(&g), Arc::clone(&r));
                Box::new(move || {
                    Box::new(Fake { text: t, opens: o, refuse, gate: Some(g), reading: r }) as Box<dyn ClipApi>
                })
            }),
            Box::new(move || {
                n.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .with_limit(limit);
        Rig { client, text, opens, gate, reading, notified }
    }

    fn release(g: &Arc<(Mutex<bool>, Condvar)>) {
        *g.0.lock().unwrap() = true;
        g.1.notify_all();
    }

    fn poll_until(c: &mut ClipClient, limit: Duration) -> Vec<(u64, Option<String>)> {
        let end = Instant::now() + limit;
        loop {
            let got = c.poll(Instant::now());
            if !got.is_empty() || Instant::now() >= end {
                return got;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn a_read_answers_with_the_text_and_tells_the_ui_thread() {
        let mut r = rig(0, false, ANSWER_LIMIT);
        assert!(r.client.read(7, Instant::now()).is_empty(), "not answered at once");
        let got = poll_until(&mut r.client, Duration::from_secs(2));
        assert_eq!(got, vec![(7, Some("hello".to_string()))]);
        assert!(r.notified.load(Ordering::SeqCst) >= 1);
        assert_eq!(r.client.workers_started(), 1);
        assert!(r.client.next_deadline().is_none());
    }

    #[test]
    fn a_write_reaches_the_clipboard_and_reads_back() {
        let mut r = rig(0, false, ANSWER_LIMIT);
        r.client.write("new text".into(), Instant::now());
        r.client.read(1, Instant::now());
        let got = poll_until(&mut r.client, Duration::from_secs(2));
        assert_eq!(got, vec![(1, Some("new text".to_string()))]);
        assert_eq!(r.text.lock().unwrap().as_deref(), Some("new text"));
    }

    #[test]
    fn opening_is_tried_five_times_ten_ms_apart_then_the_answer_is_none() {
        let mut r = rig(usize::MAX, false, Duration::from_secs(5));
        let t = Instant::now();
        r.client.read(3, t);
        let got = poll_until(&mut r.client, Duration::from_secs(2));
        let took = t.elapsed();
        assert_eq!(got, vec![(3, None)]);
        assert_eq!(r.opens.load(Ordering::SeqCst), OPEN_TRIES as usize);
        assert!(took >= Duration::from_millis(35), "{took:?}: the retries are 10 ms apart");
        assert!(took < Duration::from_millis(450), "{took:?}");
    }

    #[test]
    fn a_busy_clipboard_that_frees_up_during_the_retries_is_read() {
        let mut r = rig(2, false, ANSWER_LIMIT);
        r.client.read(4, Instant::now());
        let got = poll_until(&mut r.client, Duration::from_secs(2));
        assert_eq!(got, vec![(4, Some("hello".to_string()))]);
        assert_eq!(r.opens.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_stuck_worker_gets_the_answer_none_after_500_ms_and_no_second_worker_starts() {
        let mut r = rig(0, true, ANSWER_LIMIT);
        let t0 = Instant::now();
        assert!(r.client.read(1, t0).is_empty());
        // Wait until the worker is inside the blocked read.
        while !r.reading.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        // Before the limit nothing is answered.
        assert!(r.client.poll(t0 + Duration::from_millis(400)).is_empty());
        assert_eq!(r.client.next_deadline(), Some(t0 + ANSWER_LIMIT));
        // At the limit the UI side answers None itself.
        let got = r.client.poll(t0 + ANSWER_LIMIT);
        assert_eq!(got, vec![(1, None)]);
        assert!(r.client.next_deadline().is_none());

        // Later requests are answered None at once; none reaches the worker.
        let later = t0 + Duration::from_millis(600);
        assert_eq!(r.client.read(2, later), vec![(2, None)]);
        assert_eq!(r.client.read(3, later), vec![(3, None)]);
        r.client.write("dropped".into(), later);
        assert_eq!(r.client.workers_started(), 1, "never a second worker");
        assert_eq!(r.opens.load(Ordering::SeqCst), 1, "only the stuck request opened the clipboard");

        // The worker returns: the stale answer is dropped, new requests work.
        release(&r.gate);
        while r.shared_busy() {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(r.client.poll(Instant::now()).is_empty(), "the late result of an answered request is dropped");
        assert!(r.client.read(9, Instant::now()).is_empty());
        let got = poll_until(&mut r.client, Duration::from_secs(2));
        assert_eq!(got, vec![(9, Some("hello".to_string()))]);
        assert_eq!(r.text.lock().unwrap().as_deref(), Some("hello"), "the dropped write never ran");
        assert_eq!(r.client.workers_started(), 1);
    }

    impl Rig {
        fn shared_busy(&self) -> bool {
            self.client.shared.busy_since.lock().unwrap().is_some()
        }
    }

    #[test]
    fn a_request_queued_behind_a_slow_one_is_answered_when_it_gets_its_turn_if_in_time() {
        let mut r = rig(0, true, Duration::from_millis(500));
        let t0 = Instant::now();
        r.client.read(1, t0);
        while !r.reading.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        // Not yet stuck (100 ms in): the second request is queued, not refused.
        assert!(r.client.read(2, t0 + Duration::from_millis(100)).is_empty());
        release(&r.gate);
        let mut seen = Vec::new();
        let end = Instant::now() + Duration::from_secs(2);
        while seen.len() < 2 && Instant::now() < end {
            seen.extend(r.client.poll(Instant::now()));
            std::thread::sleep(Duration::from_millis(2));
        }
        seen.sort_by_key(|(t, _)| *t);
        assert_eq!(seen, vec![(1, Some("hello".to_string())), (2, Some("hello".to_string()))]);
    }

    #[test]
    fn an_ended_worker_is_replaced_but_a_stuck_one_is_not() {
        let mut r = rig(0, false, ANSWER_LIMIT);
        r.client.read(1, Instant::now());
        poll_until(&mut r.client, Duration::from_secs(2));
        assert_eq!(r.client.workers_started(), 1);
        // Same worker for the next request.
        r.client.read(2, Instant::now());
        poll_until(&mut r.client, Duration::from_secs(2));
        assert_eq!(r.client.workers_started(), 1);
    }

    #[test]
    fn dropping_the_client_with_a_stuck_worker_does_not_hang() {
        let r = rig(0, true, ANSWER_LIMIT);
        let gate = Arc::clone(&r.gate);
        let mut client = r.client;
        client.read(1, Instant::now());
        while !r.reading.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        let t = Instant::now();
        drop(client);
        assert!(t.elapsed() < Duration::from_millis(200), "drop waited for the stuck worker");
        release(&gate);
    }
}

//! The outside-press watch: a low-level mouse hook that reports a button
//! going down outside every RSB window, so popups that hold no keyboard focus
//! can close.
//!
//! The hook lives on a thread of its own that does nothing but wait for
//! messages (Windows calls a low-level hook through the installing thread's
//! message wait), so a busy UI turn never delays the pointer anywhere on the
//! desktop. The hook only observes: it never swallows or changes an event
//! and always calls the next hook.
//!
//! In the procedure: no allocation, no lock per event, no calls into other
//! code. The UI thread publishes the hit regions as an `Arc` snapshot behind
//! a generation counter; the procedure compares the counter on each event
//! and takes the lock only when it changed (once per change, a pointer-sized
//! copy). Windows removes a hook that answers too slowly without telling the
//! process, so the thread looks at the pointer twice a second and installs
//! the hook again when the pointer moved but the hook saw nothing.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetCursorPos, GetMessageW, HHOOK, KillTimer, MSG, MSLLHOOKSTRUCT, PostThreadMessageW, SetTimer,
    SetWindowsHookExW, UnhookWindowsHookEx, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_QUIT, WM_RBUTTONDOWN,
    WM_TIMER, WM_XBUTTONDOWN,
};

use crate::regions::Regions;
use crate::{HookStats, WinError, sys};

/// The hook procedure's message to the hidden window: wParam = x, lParam = y
/// (both as 32-bit values) of a button press outside every region.
pub(crate) const WM_OUTSIDE_PRESS: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 0x12;

/// How often the thread checks that the hook still gets events.
const QUIET_CHECK_MS: u32 = 500;
/// Delivery histogram: one bin per millisecond, the last one open-ended.
const BINS: usize = 64;
/// How long starting waits for the thread to install the hook.
const START_WAIT: Duration = Duration::from_secs(2);
/// How long stopping waits for the thread before giving up on it.
const STOP_WAIT: Duration = Duration::from_secs(1);

/// What the hook thread and the UI thread share.
pub(crate) struct HookShared {
    /// Bumped when `regions` was replaced.
    generation: AtomicU64,
    regions: Mutex<Arc<Regions>>,
    /// Snapshots the UI thread replaced. The hook procedure may still read
    /// one, so it never holds the last reference: the UI thread drops them on
    /// its next `publish`, when nothing else has them.
    retired: Mutex<Vec<Arc<Regions>>>,
    /// Set when the watch is being stopped; the thread ends at its next look.
    quit: AtomicBool,
    /// The hidden window the press message goes to.
    target: AtomicIsize,
    /// The installed hook (0 = none), for the panic path.
    hhook: AtomicIsize,
    thread_id: AtomicU32,
    installed: AtomicBool,
    ever_on: AtomicBool,
    events: AtomicU64,
    max_us: AtomicU32,
    reinstalls: AtomicU32,
    delivery_ms: [AtomicU32; BINS],
    /// QueryPerformanceFrequency, read once.
    freq: AtomicI64,
}

impl HookShared {
    pub(crate) fn new(target: isize) -> Arc<HookShared> {
        Arc::new(HookShared {
            generation: AtomicU64::new(0),
            regions: Mutex::new(Arc::new(Regions::default())),
            retired: Mutex::new(Vec::new()),
            quit: AtomicBool::new(false),
            target: AtomicIsize::new(target),
            hhook: AtomicIsize::new(0),
            thread_id: AtomicU32::new(0),
            installed: AtomicBool::new(false),
            ever_on: AtomicBool::new(false),
            events: AtomicU64::new(0),
            max_us: AtomicU32::new(0),
            reinstalls: AtomicU32::new(0),
            delivery_ms: std::array::from_fn(|_| AtomicU32::new(0)),
            freq: AtomicI64::new(0),
        })
    }

    /// Swaps in a new snapshot; the hook picks it up on its next event.
    pub(crate) fn publish(&self, regions: Regions) {
        let snapshot = Arc::new(regions);
        let old = self.regions.lock().ok().map(|mut slot| std::mem::replace(&mut *slot, snapshot));
        self.generation.fetch_add(1, Ordering::Release);
        if let Ok(mut retired) = self.retired.lock() {
            retired.extend(old);
            // Dropped here, on the UI thread, once only this list holds them.
            retired.retain(|r| Arc::strong_count(r) > 1);
        }
    }

    pub(crate) fn snapshot(&self) -> Arc<Regions> {
        self.regions.lock().map(|g| Arc::clone(&g)).unwrap_or_default()
    }

    /// The hook is installed right now.
    #[cfg(test)]
    pub(crate) fn is_installed(&self) -> bool {
        self.installed.load(Ordering::Acquire)
    }

    pub(crate) fn ever_on(&self) -> bool {
        self.ever_on.load(Ordering::Acquire)
    }

    pub(crate) fn stats(&self) -> HookStats {
        let bins: Vec<u32> = self.delivery_ms.iter().map(|b| b.load(Ordering::Relaxed)).collect();
        HookStats {
            events: self.events.load(Ordering::Relaxed),
            max_us: self.max_us.load(Ordering::Relaxed),
            p99_delivery_us: percentile_ms(&bins, 0.99).saturating_mul(1000),
            reinstalls: self.reinstalls.load(Ordering::Relaxed),
        }
    }
}

/// The millisecond bin below which `p` of the samples fall (0 with none).
pub(crate) fn percentile_ms(bins: &[u32], p: f64) -> u32 {
    let total: u64 = bins.iter().map(|&b| b as u64).sum();
    if total == 0 {
        return 0;
    }
    let need = ((total as f64) * p).ceil() as u64;
    let mut seen = 0u64;
    for (ms, &n) in bins.iter().enumerate() {
        seen += n as u64;
        if seen >= need {
            return ms as u32;
        }
    }
    bins.len().saturating_sub(1) as u32
}

/// Whether the hook should be installed again: the pointer moved since the
/// last look but the hook saw no event at all.
pub(crate) fn needs_reinstall(last_pos: (i32, i32), pos: (i32, i32), last_events: u64, events: u64) -> bool {
    pos != last_pos && events == last_events
}

/// Whether a mouse message is a button press.
fn is_button_down(msg: u32) -> bool {
    matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN)
}

struct Ctx {
    shared: Arc<HookShared>,
    cache: Arc<Regions>,
    generation: u64,
}

thread_local! {
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

/// Every running watch, so the panic path can remove them all.
static ACTIVE: Mutex<Vec<Arc<HookShared>>> = Mutex::new(Vec::new());

fn qpc() -> i64 {
    let mut v = 0i64;
    // SAFETY: a live out pointer.
    unsafe {
        let _ = QueryPerformanceCounter(&mut v);
    }
    v
}

unsafe extern "system" fn hook_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == 0 && lp.0 != 0 {
        // SAFETY: for HC_ACTION in a WH_MOUSE_LL hook, lParam points at an MSLLHOOKSTRUCT.
        let info = unsafe { &*(lp.0 as *const MSLLHOOKSTRUCT) };
        on_event(wp.0 as u32, info);
    }
    // SAFETY: passing the arguments Windows gave us to the next hook.
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

fn on_event(msg: u32, info: &MSLLHOOKSTRUCT) {
    let t0 = qpc();
    CTX.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else { return };
        let Some(ctx) = guard.as_mut() else { return };
        let sh = &ctx.shared;
        sh.events.fetch_add(1, Ordering::Relaxed);
        // Event timestamp to now, in the system tick's resolution.
        // SAFETY: no arguments.
        let delay = unsafe { GetTickCount() }.wrapping_sub(info.time) as usize;
        sh.delivery_ms[delay.min(BINS - 1)].fetch_add(1, Ordering::Relaxed);
        if is_button_down(msg) {
            let generation = sh.generation.load(Ordering::Acquire);
            if generation != ctx.generation
                && let Ok(slot) = sh.regions.lock()
            {
                ctx.cache = Arc::clone(&slot);
                ctx.generation = generation;
            }
            if !ctx.cache.contains(info.pt.x, info.pt.y) {
                let target = sh.target.load(Ordering::Acquire);
                if target != 0 {
                    sys::post(target, WM_OUTSIDE_PRESS, info.pt.x as u32 as usize, info.pt.y as u32 as isize);
                }
            }
        }
        let freq = sh.freq.load(Ordering::Relaxed).max(1);
        let us = ((qpc() - t0).max(0) as i128 * 1_000_000 / freq as i128) as u32;
        sh.max_us.fetch_max(us, Ordering::Relaxed);
    });
}

fn install() -> Result<HHOOK, WinError> {
    // SAFETY: the procedure is a plain function of this module; hmod is this
    // module and the thread id 0 asks for a global low-level hook.
    unsafe {
        let hinst = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
        SetWindowsHookExW(WH_MOUSE_LL, Some(hook_proc), Some(hinst), 0)
    }
    .map_err(|e| sys::call_error("SetWindowsHookExW", &e))
}

fn cursor() -> (i32, i32) {
    let mut p = POINT::default();
    // SAFETY: a live out pointer.
    let ok = unsafe { GetCursorPos(&mut p) }.is_ok();
    if ok { (p.x, p.y) } else { (i32::MIN, i32::MIN) }
}

fn thread_main(shared: Arc<HookShared>, ready: mpsc::Sender<Result<u32, WinError>>) {
    let mut freq = 0i64;
    // SAFETY: a live out pointer.
    unsafe {
        let _ = QueryPerformanceFrequency(&mut freq);
    }
    shared.freq.store(freq.max(1), Ordering::Relaxed);
    CTX.with(|c| {
        *c.borrow_mut() = Some(Ctx { shared: Arc::clone(&shared), cache: shared.snapshot(), generation: u64::MAX });
    });
    let mut hook = match install() {
        Ok(h) => h,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    // SAFETY: no arguments.
    let tid = unsafe { GetCurrentThreadId() };
    shared.thread_id.store(tid, Ordering::Release);
    shared.hhook.store(hook.0 as isize, Ordering::Release);
    shared.installed.store(true, Ordering::Release);
    let _ = ready.send(Ok(tid));
    if shared.quit.load(Ordering::Acquire) {
        // Stopped while it was still installing: nobody else knows this hook.
        // SAFETY: the handle was returned by SetWindowsHookExW.
        unsafe {
            let _ = UnhookWindowsHookEx(hook);
        }
        shared.installed.store(false, Ordering::Release);
        shared.hhook.store(0, Ordering::Release);
        CTX.with(|c| *c.borrow_mut() = None);
        return;
    }
    // A thread timer (no window): its messages come through the same wait.
    // SAFETY: no window, no callback.
    let timer = unsafe { SetTimer(None, 0, QUIET_CHECK_MS, None) };
    let mut last_pos = cursor();
    let mut last_events = shared.events.load(Ordering::Relaxed);
    let mut msg = MSG::default();
    loop {
        // SAFETY: `msg` is a live out pointer; no filter.
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if got.0 <= 0 {
            break;
        }
        if msg.message == WM_TIMER {
            if shared.quit.load(Ordering::Acquire) {
                break;
            }
            let pos = cursor();
            let events = shared.events.load(Ordering::Relaxed);
            if needs_reinstall(last_pos, pos, last_events, events) {
                // SAFETY: the handle was returned by SetWindowsHookExW.
                unsafe {
                    let _ = UnhookWindowsHookEx(hook);
                }
                shared.installed.store(false, Ordering::Release);
                match install() {
                    Ok(h) => {
                        hook = h;
                        shared.hhook.store(hook.0 as isize, Ordering::Release);
                        shared.installed.store(true, Ordering::Release);
                        shared.reinstalls.fetch_add(1, Ordering::Relaxed);
                        log::debug!("outside-press hook installed again");
                    }
                    Err(e) => {
                        shared.hhook.store(0, Ordering::Release);
                        log::warn!("outside-press hook could not be installed again: {e}");
                        break;
                    }
                }
            }
            last_pos = pos;
            last_events = shared.events.load(Ordering::Relaxed);
        }
    }
    if shared.installed.swap(false, Ordering::AcqRel) {
        let h = shared.hhook.swap(0, Ordering::AcqRel);
        if h != 0 {
            // SAFETY: the handle was returned by SetWindowsHookExW.
            unsafe {
                let _ = UnhookWindowsHookEx(HHOOK(h as *mut std::ffi::c_void));
            }
        }
    }
    if timer != 0 {
        // SAFETY: the timer id SetTimer returned.
        unsafe {
            let _ = KillTimer(None, timer);
        }
    }
    shared.thread_id.store(0, Ordering::Release);
    CTX.with(|c| *c.borrow_mut() = None);
}

/// A running watch.
pub(crate) struct Hook {
    shared: Arc<HookShared>,
    join: Option<JoinHandle<()>>,
}

impl Hook {
    /// Starts the thread and waits (briefly) for it to install the hook.
    pub(crate) fn start(shared: &Arc<HookShared>) -> Result<Hook, WinError> {
        Hook::start_waiting(shared, START_WAIT)
    }

    /// `start` with its own wait. When the thread is slower than that the
    /// watch is kept anyway: it is listed and stopped like any other, so a
    /// hook installed late is never left behind with nobody to remove it.
    pub(crate) fn start_waiting(shared: &Arc<HookShared>, wait: Duration) -> Result<Hook, WinError> {
        shared.quit.store(false, Ordering::Release);
        shared.thread_id.store(0, Ordering::Release);
        let (tx, rx) = mpsc::channel();
        let sh = Arc::clone(shared);
        let join = std::thread::Builder::new()
            .name("rsb-outside-press".into())
            .spawn(move || thread_main(sh, tx))
            .map_err(|e| WinError::Call { call: "thread spawn", message: e.to_string() })?;
        match rx.recv_timeout(wait) {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                let _ = join.join();
                return Err(e);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                log::warn!("the outside-press hook is slow to start; it is kept and stopped with the watch");
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // The thread ended without answering (it could not install).
                let _ = join.join();
                return Err(WinError::Call {
                    call: "SetWindowsHookExW",
                    message: "the hook thread ended without a hook".into(),
                });
            }
        }
        shared.ever_on.store(true, Ordering::Release);
        if let Ok(mut all) = ACTIVE.lock() {
            all.push(Arc::clone(shared));
        }
        Ok(Hook { shared: Arc::clone(shared), join: Some(join) })
    }

    /// The thread is still running.
    pub(crate) fn is_running(&self) -> bool {
        self.join.as_ref().is_some_and(|j| !j.is_finished())
    }

    /// Removes the hook and ends the thread.
    pub(crate) fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        self.shared.quit.store(true, Ordering::Release);
        let tid = self.shared.thread_id.load(Ordering::Acquire);
        if tid != 0 {
            // SAFETY: plain value arguments; a thread that is gone only makes the call fail.
            unsafe {
                let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        if let Ok(mut all) = ACTIVE.lock() {
            all.retain(|s| !Arc::ptr_eq(s, &self.shared));
        }
        if let Some(join) = self.join.take() {
            let end = Instant::now() + STOP_WAIT;
            while !join.is_finished() && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(2));
            }
            if join.is_finished() {
                let _ = join.join();
            } else {
                // Take the hook down from here; the thread is stuck somewhere else.
                log::warn!("the outside-press thread did not stop in time");
                remove_hook_of(&self.shared);
            }
        }
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        if self.join.is_some() {
            self.stop_inner();
        }
    }
}

fn remove_hook_of(shared: &HookShared) {
    shared.installed.store(false, Ordering::Release);
    let h = shared.hhook.swap(0, Ordering::AcqRel);
    if h != 0 {
        // SAFETY: the handle came from SetWindowsHookExW; unhooking from another thread is allowed.
        unsafe {
            let _ = UnhookWindowsHookEx(HHOOK(h as *mut std::ffi::c_void));
        }
    }
    let tid = shared.thread_id.load(Ordering::Acquire);
    if tid != 0 {
        // SAFETY: plain value arguments.
        unsafe {
            let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

/// How many watches are running (tests).
#[cfg(test)]
pub(crate) fn active_count() -> usize {
    ACTIVE.lock().map(|a| a.len()).unwrap_or(0)
}

/// Removes every running watch at once, from any thread, without waiting:
/// for the panic hook and a console close. The process may be about to end,
/// so nothing here joins or allocates beyond the list lock (`try_lock`: a
/// lock held by the panicking thread itself must not hang the hook).
pub fn remove_outside_press_hooks() {
    if let Ok(all) = ACTIVE.try_lock() {
        for shared in all.iter() {
            remove_hook_of(shared);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replaced_snapshot_is_dropped_by_the_ui_thread_never_left_to_the_hook() {
        let shared = HookShared::new(0);
        // The hook procedure holds the snapshot it is reading.
        let in_use = shared.snapshot();
        shared.publish(Regions::default());
        assert_eq!(shared.retired.lock().unwrap().len(), 1, "kept while the hook may still read it");
        drop(in_use);
        shared.publish(Regions::default());
        assert_eq!(shared.retired.lock().unwrap().len(), 0, "dropped on the next publish, here");
    }

    #[test]
    fn percentile_of_a_histogram() {
        assert_eq!(percentile_ms(&[0; 8], 0.99), 0, "no samples");
        let mut bins = [0u32; 8];
        bins[0] = 990;
        bins[3] = 10;
        assert_eq!(percentile_ms(&bins, 0.99), 0, "99 of 100 are in bin 0");
        assert_eq!(percentile_ms(&bins, 0.995), 3);
        bins[0] = 98;
        bins[3] = 2;
        assert_eq!(percentile_ms(&bins, 0.99), 3);
        assert_eq!(percentile_ms(&bins, 0.5), 0);
        assert_eq!(percentile_ms(&[0, 0, 5], 1.0), 2);
    }

    #[test]
    fn reinstall_only_when_the_pointer_moved_and_the_hook_saw_nothing() {
        assert!(needs_reinstall((0, 0), (5, 5), 10, 10));
        assert!(!needs_reinstall((0, 0), (5, 5), 10, 11), "the hook saw events");
        assert!(!needs_reinstall((5, 5), (5, 5), 10, 10), "the pointer did not move");
        assert!(!needs_reinstall((0, 0), (0, 0), 3, 9));
    }

    #[test]
    fn button_downs_are_recognised() {
        for m in [WM_LBUTTONDOWN, WM_RBUTTONDOWN, WM_MBUTTONDOWN, WM_XBUTTONDOWN] {
            assert!(is_button_down(m));
        }
        assert!(!is_button_down(0x200), "WM_MOUSEMOVE");
        assert!(!is_button_down(0x202), "WM_LBUTTONUP");
        assert!(!is_button_down(0x20A), "WM_MOUSEWHEEL");
    }

    #[test]
    fn the_snapshot_swap_is_seen_by_the_next_reader_and_old_readers_keep_theirs() {
        let sh = HookShared::new(0);
        let before = sh.snapshot();
        assert!(!before.contains(5, 5));
        let g0 = sh.generation.load(Ordering::Acquire);
        sh.publish(Regions { bars: vec![rsb_core::geom::RectPx::new(0, 0, 10, 10)], popups: Vec::new() });
        assert_eq!(sh.generation.load(Ordering::Acquire), g0 + 1);
        assert!(sh.snapshot().contains(5, 5));
        assert!(!before.contains(5, 5), "a reader holding the old snapshot is not changed");
    }

    #[test]
    fn swaps_from_one_thread_while_another_reads_never_tear() {
        use rsb_core::geom::RectPx;
        let sh = HookShared::new(0);
        let stop = Arc::new(AtomicBool::new(false));
        let reads = Arc::new(AtomicU64::new(0));
        let reader = {
            let (sh, stop, reads) = (Arc::clone(&sh), Arc::clone(&stop), Arc::clone(&reads));
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let s = sh.snapshot();
                    // Every published snapshot holds `k` bars, all starting at x = k.
                    if let Some(first) = s.bars.first() {
                        assert!(s.bars.iter().all(|b| b.x0 == first.x0), "a torn snapshot");
                        assert_eq!(s.bars.len() as i32, first.x0);
                    }
                    reads.fetch_add(1, Ordering::Relaxed);
                }
            })
        };
        // Swap until the reader has looked at a good number of snapshots.
        let mut n = 0i32;
        while reads.load(Ordering::Relaxed) < 2000 && n < 5_000_000 {
            n += 1;
            let k = n % 7 + 1;
            sh.publish(Regions { bars: (0..k).map(|_| RectPx::new(k, 0, 100, 1)).collect(), popups: Vec::new() });
        }
        stop.store(true, Ordering::Relaxed);
        reader.join().unwrap();
        assert!(reads.load(Ordering::Relaxed) >= 2000, "the reader never ran");
    }
}

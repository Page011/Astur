//! Luau widgets.
//!
//! One script thread runs every VM. Built-in widgets share one VM; each user
//! widget file gets its own (about 1 MB each); several instances of one
//! file share its VM but each runs in its own sandboxed thread
//! (`Thread::sandbox`), so their globals never mix. Every VM is sandboxed
//! (`loadstring`, `getfenv`, `setfenv` and the host `print` / `require`
//! removed before `sandbox(true)`), has a memory cap, and every call into it
//! runs under a time budget enforced by an interrupt. Chunk names are set
//! explicitly (`=widgets/my/weather.luau`, `=builtin/clock.luau`) so no host
//! path ever reaches a message.
//!
//! Stopping is decided in Rust, never by a Luau error alone: once an
//! instance is over a limit, a flag makes every later interrupt and every
//! callback into Rust raise again, so `pcall` and `xpcall` cannot swallow
//! it. What stops an instance (plate, log line, `CancelOwner` to the hub):
//! an error while loading or in `setup`, a time budget, the memory cap, a
//! permission denial, a tree over the node or depth cap, an update loop,
//! too much CPU over a second, or 10 uncaught errors within 10 s. Any other
//! uncaught error (in a handler, timer, task, poll, effect or function
//! prop) is logged (rate-limited), adds the class `error` to the widget's
//! root element for 10 s, and the widget keeps running. A stopped built-in
//! restarts by itself after 1 s, doubling to 5 min; a stopped user widget
//! waits for its file to be saved or `rsb reload`.
//!
//! The scheduler is fair: one turn runs due work for each instance in
//! round-robin order and ends after about `Limits::turn_budget`, leaving the
//! rest for the next turn; events (`rsb.events.emit`, `rsb trigger`) are
//! delivered on the next turn; a task that yields with no request
//! (`coroutine.yield()`) resumes on the next turn.
//!
//! Popups, menus, tooltips, inputs and sliders are elements. The script decides when a popup is open (its `open`
//! state, which the runtime also writes false when the UI closes the popup) and builds the popup's content in a
//! scope that exists only while it is open. A click or key the UI stamped with an id is a gesture for as long as
//! its handler runs up to its first wait: popups opened then carry the id (the UI gives focus only for those), and
//! commands that start apps, change devices or end the session run only then.
//!
//! Script code never waits: I/O calls (`rsb.http.get`, `rsb.exec.run`,
//! `rsb.file.read`, `rsb.task.sleep`) yield the calling task's coroutine and the
//! scheduler resumes it when the result arrives from the services hub.
//!
//! `Runtime` is the whole engine without a thread, driven by messages and a
//! clock (tests use it directly); `ScriptHost` runs a `Runtime` on its own
//! thread.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use rsb_core::msg::{Heartbeat, HubPort, ScriptIn, ScriptSink};
use rsb_core::permissions::PathEnv;
use rsb_core::time::{Clock, ThreadTime};

pub mod api;
mod convert;
mod natives;
mod runtime;
mod shared;
mod snapshot;
pub mod types;
mod vm;

/// Where widget sources come from.
pub trait SourceStore: Send + Sync {
    /// A built-in file by path relative to the assets folder, `/`
    /// separators: `widgets/clock.luau`, `widgets/lib/format.luau`.
    fn builtin(&self, rel_path: &str) -> Option<Cow<'static, str>>;
    /// Every built-in widget name (`clock`, `cpu`, ...).
    fn builtin_widgets(&self) -> Vec<String>;
    /// A user file relative to the config folder (`widgets/my/weather.luau`).
    /// `Ok(None)` when it does not exist. Files over
    /// `rsb_core::paths::MAX_SOURCE_BYTES` are an error.
    fn user(&self, rel_path: &str) -> std::io::Result<Option<String>>;
}

/// Sandbox and scheduler limits. `Default` is what the bar runs with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    /// Per call into a VM (handler, timer, effect, task resume): CPU time of
    /// the script thread when `ScriptOptions::thread_time` is set (a call
    /// that was only preempted is not stopped), else wall time.
    pub callback_budget: Duration,
    /// Loading a widget file plus running its `setup` (same clock).
    pub setup_budget: Duration,
    /// Wall time after which a call is stopped whatever its CPU time.
    pub hard_wall_budget: Duration,
    /// CPU time one instance may use within any `cpu_window`.
    pub cpu_window: Duration,
    pub cpu_window_budget: Duration,
    /// One script turn ends after about this; remaining work runs next turn.
    pub turn_budget: Duration,
    /// Effect and computed re-runs within one turn before the instance is
    /// stopped as an update loop.
    pub max_effect_runs_per_turn: u32,
    /// Events an instance may emit per second; more are dropped and counted.
    pub max_events_per_sec: u32,
    /// Uncaught errors within `error_window` that stop an instance.
    pub max_errors: u32,
    pub error_window: Duration,
    pub builtin_restart_min: Duration,
    pub builtin_restart_max: Duration,
    pub user_vm_memory: usize,
    pub shared_vm_memory: usize,
    /// Every VM together; a new user VM is refused (plate) past this.
    pub total_vm_memory: usize,
    pub max_element_depth: usize,
    /// `table.sort` without a comparator refuses longer tables (it cannot
    /// be interrupted).
    pub max_sort_without_comparator: usize,
    /// Timers faster than this are clamped (and warned about once).
    pub min_timer_interval: Duration,
    pub max_tasks_per_instance: usize,
    pub max_timers_per_instance: usize,
    pub max_nodes_per_instance: usize,
    pub max_text_chars: usize,
    /// `print` / `rsb.log` lines per second per instance; more are dropped
    /// and counted. Script text is logged with CR, LF and other control
    /// characters escaped, so it cannot forge log lines.
    pub max_log_lines_per_sec: u32,
    /// Script log lines per second for all instances together.
    pub max_log_lines_total_per_sec: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            callback_budget: Duration::from_millis(10),
            setup_budget: Duration::from_millis(50),
            hard_wall_budget: Duration::from_millis(250),
            cpu_window: Duration::from_secs(1),
            cpu_window_budget: Duration::from_millis(250),
            turn_budget: Duration::from_millis(20),
            max_effect_runs_per_turn: 1000,
            max_events_per_sec: 50,
            max_errors: 10,
            error_window: Duration::from_secs(10),
            builtin_restart_min: Duration::from_secs(1),
            builtin_restart_max: Duration::from_secs(300),
            user_vm_memory: 32 << 20,
            shared_vm_memory: 64 << 20,
            total_vm_memory: 256 << 20,
            max_element_depth: rsb_core::element::MAX_DEPTH,
            max_sort_without_comparator: 100_000,
            min_timer_interval: Duration::from_millis(100),
            max_tasks_per_instance: 64,
            max_timers_per_instance: 64,
            max_nodes_per_instance: 2000,
            max_text_chars: 4096,
            max_log_lines_per_sec: 20,
            max_log_lines_total_per_sec: 200,
        }
    }
}

pub struct ScriptOptions {
    pub sources: Arc<dyn SourceStore>,
    pub clock: Arc<dyn Clock>,
    /// Absolute config folder; user widget folders are under it.
    pub config_dir: PathBuf,
    pub path_env: PathEnv,
    pub limits: Limits,
    /// Windows' app theme at start (`rsb.theme.dark`).
    pub dark: bool,
    /// CPU time of the script thread (rsb-services `ThreadCycleTime`). None
    /// in tests: budgets then use wall time.
    pub thread_time: Option<Arc<dyn ThreadTime>>,
    /// Updated around every call into a VM, for the binary's watchdog.
    pub heartbeat: Arc<Heartbeat>,
}

/// Times the script thread returned from its wait (a message or a due
/// timer), for the binary's wake counters.
static HOST_WAKES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Wakes of the script thread since the process started.
pub fn host_wakes() -> u64 {
    HOST_WAKES.load(std::sync::atomic::Ordering::Relaxed)
}

/// The widget whose VM gets the hidden `__test_stall(ms)` function.
static STALL_WIDGET: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Test hook for the watchdog: VMs created after this for the user file of
/// `widget` (or, for a built-in name, the shared VM) get a global
/// `__test_stall(ms)` that sleeps without looking at its deadline, which is
/// the one kind of stall the interrupt cannot see. Never set in normal runs.
pub fn set_test_stall_widget(widget: Option<&str>) {
    *STALL_WIDGET.lock().unwrap_or_else(|e| e.into_inner()) = widget.map(str::to_string);
}

pub(crate) fn test_stall_enabled_for(widget: Option<&str>) -> bool {
    let guard = STALL_WIDGET.lock().unwrap_or_else(|e| e.into_inner());
    match (&*guard, widget) {
        (Some(want), Some(have)) => want == have,
        (Some(_), None) => true,
        _ => false,
    }
}

/// The engine without a thread.
pub struct Runtime {
    core: runtime::Core,
}

impl Runtime {
    pub fn new(options: ScriptOptions, hub: Box<dyn HubPort>, out: Box<dyn ScriptSink>) -> Runtime {
        Runtime { core: runtime::Core::new(options, hub, out) }
    }

    /// Handles one message, then runs everything due and flushes changed
    /// trees as one `ScriptOut::Trees`. Returns false after `Shutdown`.
    pub fn handle(&mut self, message: ScriptIn) -> bool {
        self.core.handle(message)
    }

    /// Fires timers due at the clock's `mono()` time and flushes.
    pub fn run_due(&mut self) {
        self.core.run_due();
    }

    /// When `run_due` next has work, for the thread's receive timeout.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.core.next_deadline()
    }
}

/// A `Runtime` on its own thread.
pub struct ScriptHost {
    thread: Option<std::thread::JoinHandle<()>>,
    tx: Sender<ScriptIn>,
    done: Receiver<()>,
}

impl ScriptHost {
    /// Starts the script thread. The VMs are created on it.
    pub fn start(
        options: ScriptOptions,
        hub: Box<dyn HubPort>,
        out: Box<dyn ScriptSink>,
    ) -> (ScriptHost, Sender<ScriptIn>) {
        let (tx, rx) = channel::<ScriptIn>();
        let (done_tx, done) = channel::<()>();
        let clock = options.clock.clone();
        let thread = std::thread::Builder::new()
            .name("rsb-script".into())
            .spawn(move || {
                let mut rt = Runtime::new(options, hub, out);
                loop {
                    let wait = match rt.next_deadline() {
                        Some(at) => at.saturating_duration_since(clock.mono()),
                        None => Duration::from_secs(3600),
                    };
                    let received = rx.recv_timeout(wait);
                    HOST_WAKES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    match received {
                        Ok(msg) => {
                            if !rt.core.ingest(msg) {
                                break;
                            }
                            // Messages that queued up while we worked are one turn.
                            let mut open = true;
                            for _ in 0..64 {
                                match rx.try_recv() {
                                    Ok(m) => {
                                        if !rt.core.ingest(m) {
                                            open = false;
                                            break;
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                            if !open {
                                break;
                            }
                            rt.core.turn();
                        }
                        Err(RecvTimeoutError::Timeout) => rt.run_due(),
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                drop(rt);
                let _ = done_tx.send(());
            })
            .expect("the script thread could not be started");
        (ScriptHost { thread: Some(thread), tx: tx.clone(), done }, tx)
    }

    /// Sends `Shutdown` and joins, at most `timeout` (a runaway script is
    /// interrupted by its budget long before).
    pub fn join(mut self, timeout: Duration) {
        let _ = self.tx.send(ScriptIn::Shutdown);
        if self.done.recv_timeout(timeout).is_ok() {
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        } else {
            log::warn!(
                "the script thread did not stop within {} ms; leaving it to exit with the process",
                timeout.as_millis()
            );
        }
    }
}

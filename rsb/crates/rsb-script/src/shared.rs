//! State shared by the runtime and the native functions its VMs call. All of
//! it lives on the script thread (`Rc`, `Cell`, `RefCell`).
//!
//! The one rule that keeps `RefCell` safe: nothing holds a borrow of `st`
//! while it calls into Luau. Natives take short borrows, finish them, and
//! only then run anything that could re-enter.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mlua::{Error, Function, Lua, Thread, Value as Lv};
use rsb_core::diag::{DiagKind, Diagnostic, Plate};
use rsb_core::element::InstanceId;
use rsb_core::hit::{ElementCall, HandlerCall};
use rsb_core::io::{ExecResult, HttpResponse, IoError, IoId};
use rsb_core::msg::{Heartbeat, HubPort, InstanceSpec, ScriptSink, WidgetSource, WidgetState};
use rsb_core::permissions::{PathEnv, Permissions};
use rsb_core::popup::PopupAction;
use rsb_core::services::{ServiceEvent, ServiceId};
use rsb_core::time::{Clock, Precision, ThreadTime, TimeZone};
use rsb_core::value::Value;

use crate::{Limits, SourceStore};

pub(crate) type VmId = u32;

/// Why an instance was stopped.
#[derive(Clone, Debug)]
pub(crate) enum Stop {
    Time {
        budget: Duration,
        used: Duration,
        label: &'static str,
        loading: bool,
    },
    Cpu {
        used: Duration,
    },
    Memory {
        limit: usize,
    },
    /// `net:host`, `exec:git`, ... as `Need` prints it.
    Permission(String),
    UpdateLoop,
    Nodes(usize),
    Depth(usize),
    Errors,
}

/// `text` followed by what to do next. After a question (a did-you-mean) the
/// tail starts a new sentence: `...did you mean rsb.text? Fix it and save`.
pub(crate) fn with_tail(text: &str, tail: &str) -> String {
    if text.ends_with('?') {
        let mut chars = tail.chars();
        let first: String = chars.next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
        format!("{text} {first}{}", chars.as_str())
    } else {
        format!("{text}; {tail}")
    }
}

impl Stop {
    pub(crate) fn diagnostic(&self, widget: &str, restart: bool) -> Diagnostic {
        let tail = if restart { "; restarting" } else { "; fix it and save" };
        match self {
            Stop::Time { budget, loading: false, .. } => {
                Diagnostic::error(DiagKind::Stopped, None, format!("{widget} ran over {} ms{tail}", budget.as_millis()))
            }
            Stop::Time { budget, loading: true, .. } => Diagnostic::error(
                DiagKind::Stopped,
                None,
                format!("{widget} ran over {} ms while loading{tail}", budget.as_millis()),
            ),
            Stop::Cpu { .. } => Diagnostic::error(DiagKind::Stopped, None, format!("{widget} used too much CPU{tail}")),
            Stop::Memory { limit } => {
                Diagnostic::error(DiagKind::Stopped, None, format!("{widget} used over {} MB{tail}", limit >> 20))
            }
            // A widget that starts again has no permissions list to edit and
            // no file to fix: it only says that it restarts.
            Stop::Permission(need) if restart => {
                Diagnostic::error(DiagKind::Permission, None, format!("{widget} needs {need}{tail}"))
            }
            Stop::Permission(need) => {
                Diagnostic::error(DiagKind::Permission, None, format!("{widget} needs {need}; add it to permissions"))
            }
            Stop::UpdateLoop => {
                let after = if restart { tail } else { "" };
                Diagnostic::error(
                    DiagKind::Stopped,
                    None,
                    format!("{widget} is in an update loop (an effect changes what it reads){after}"),
                )
            }
            Stop::Nodes(n) => {
                Diagnostic::error(DiagKind::Stopped, None, format!("{widget} draws over {n} elements{tail}"))
            }
            Stop::Depth(n) => Diagnostic::error(
                DiagKind::Stopped,
                None,
                format!("{widget} nests elements deeper than {n} levels{tail}"),
            ),
            Stop::Errors => Diagnostic::error(DiagKind::Stopped, None, format!("{widget} had too many errors{tail}")),
        }
    }

    pub(crate) fn log_line(&self, widget: &str) -> String {
        match self {
            Stop::Time { used, label, .. } => {
                format!("stopped {widget}: {:.1} ms in {label}", used.as_secs_f64() * 1000.0)
            }
            Stop::Cpu { used } => format!("stopped {widget}: {} ms CPU in 1 s", used.as_millis()),
            Stop::Memory { limit } => format!("stopped {widget}: used over {} MB", limit >> 20),
            Stop::Permission(need) => format!("permission denied: {widget} needs {need}"),
            Stop::UpdateLoop => format!("stopped {widget}: update loop"),
            Stop::Nodes(n) => format!("stopped {widget}: more than {n} elements"),
            Stop::Depth(n) => format!("stopped {widget}: elements nested over {n} levels"),
            Stop::Errors => format!("stopped {widget}: too many errors"),
        }
    }
}

/// What `Shared::guarded` needs to know about one call into a VM.
pub(crate) struct Call<'a> {
    pub inst: Option<InstanceId>,
    /// Names the call in logs ("on_click", "timer").
    pub label: &'static str,
    pub budget: Duration,
    /// A file load: the "while loading" wording in the plate.
    pub loading: bool,
    /// The VM's memory cap and the soft mark the interrupt compares against.
    pub mem: (usize, usize),
    /// Repairs the VM's bookkeeping after a failed call.
    pub reset: Option<&'a Function>,
}

pub(crate) enum Outcome<R> {
    Ok(R),
    /// An uncaught script error (message first line, no traceback).
    Err(String),
    Stopped(Stop),
}

/// Fixed one-second window counter (log lines, events).
#[derive(Debug)]
pub(crate) struct Window {
    start: Option<Instant>,
    n: u32,
    pub dropped: u64,
}

impl Window {
    pub(crate) fn new() -> Window {
        Window { start: None, n: 0, dropped: 0 }
    }

    pub(crate) fn allow(&mut self, now: Instant, max: u32) -> bool {
        self.allow_cost(now, max, 1)
    }

    /// Like `allow` for something that costs `cost` of the `max` allowance.
    pub(crate) fn allow_cost(&mut self, now: Instant, max: u32, cost: u32) -> bool {
        match self.start {
            Some(s) if now.saturating_duration_since(s) < Duration::from_secs(1) => {}
            _ => {
                self.start = Some(now);
                self.n = 0;
            }
        }
        if self.n.saturating_add(cost) <= max {
            self.n += cost;
            true
        } else {
            self.dropped += 1;
            false
        }
    }
}

pub(crate) struct Inst {
    pub id: InstanceId,
    pub spec: InstanceSpec,
    pub vm: VmId,
    /// `my/weather`, `clock#utc`.
    pub ref_text: String,
    pub source: WidgetSource,
    pub builtin: bool,
    /// A user file standing in for the built-in widget of the same name.
    pub replaces_builtin: bool,
    /// Relative paths (entry file and required modules) the instance loaded.
    pub files: HashSet<String>,
    /// Folder of the entry file relative to the config (or assets) folder.
    pub entry: String,
    pub perms: Permissions,
    pub grant: Option<Arc<rsb_core::io::IoGrant>>,
    pub state: WidgetState,
    pub plate: Option<Plate>,
    pub calls: u64,
    pub script_time: Duration,
    pub script_time_max: Duration,
    pub errors: u64,
    pub restarts: u32,
    pub backoff: Duration,
    pub restart_at: Option<Instant>,
    pub started_at: Instant,
    pub version: u64,
    pub dirty: bool,
    pub error_until: Option<Instant>,
    /// The failure behind `error_until`, one line without a path: what an
    /// empty popup shows in place of its content.
    pub last_error: Option<String>,
    pub err_times: VecDeque<Instant>,
    pub cpu: VecDeque<(Instant, Duration)>,
    /// Sum of `cpu`, kept up to date so a check costs the same however many
    /// calls fit in the window.
    pub cpu_sum: Duration,
    pub log_win: Window,
    pub log_reported: Option<Instant>,
    pub event_win: Window,
    /// Bytes of event data emitted per second.
    pub event_bytes_win: Window,
    pub next_handler: u32,
    pub handlers: HashMap<u32, Function>,
    pub subs: HashSet<ServiceId>,
    pub modules: HashMap<String, Lv>,
    pub loading: Vec<String>,
    pub timer_warned: bool,
    pub widget_name: String,
    /// When the last gesture-only command ran, for the one-per-2-seconds limit.
    pub gesture_cmd_at: Option<Instant>,
}

pub(crate) struct Task {
    pub inst: InstanceId,
    pub thread: Thread,
    pub wait: Wait,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Wait {
    Running,
    Sleep(Instant),
    Io(IoId),
    Turn,
}

pub(crate) enum TimerKind {
    Every { period: Duration, aligned: bool },
    After,
    Clock { precision: Precision, zone: TimeZone },
}

pub(crate) struct TimerEnt {
    pub inst: InstanceId,
    pub kind: TimerKind,
    /// None while a firing waits in the ready queue (one-shot timers).
    pub next: Option<Instant>,
    pub cb: Function,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ResultKind {
    Exec,
    Http,
    File,
}

pub(crate) enum IoWait {
    Task { task: u64, kind: ResultKind },
    Spawn { on_line: Option<Function>, on_exit: Option<Function> },
    Watch { cb: Function },
    Open,
}

pub(crate) struct EventSub {
    pub inst: InstanceId,
    pub name: String,
    pub cb: Function,
}

pub(crate) enum Payload {
    Nothing,
    Exec(Result<ExecResult, IoError>),
    Http(Result<HttpResponse, IoError>),
    File(Result<String, IoError>),
}

pub(crate) enum Work {
    Timer(u64),
    Resume {
        task: u64,
        payload: Payload,
    },
    Event {
        sub: u64,
        data: Rc<Value>,
    },
    Input(HandlerCall),
    /// A key, text, value, focus, scroll or popup event for one handler.
    Element(ElementCall),
    /// `rsb popup` or `rsb.popups.*`: open, close or toggle one popup of the
    /// instance; `gesture` is the user input the command came from, if any.
    Popup {
        pid: u64,
        action: PopupAction,
        name: String,
        gesture: Option<u64>,
    },
    Flush,
    Callback {
        f: Function,
        args: Vec<Value>,
    },
}

#[derive(Default)]
pub(crate) struct State {
    pub instances: BTreeMap<InstanceId, Inst>,
    pub tasks: HashMap<u64, Task>,
    pub next_task: u64,
    pub timers: HashMap<u64, TimerEnt>,
    pub next_timer: u64,
    pub io: HashMap<IoId, (InstanceId, IoWait)>,
    pub next_io: u64,
    pub ready: BTreeMap<InstanceId, VecDeque<Work>>,
    pub deferred: Vec<(InstanceId, Work)>,
    pub pending_flush: BTreeSet<InstanceId>,
    pub svc: HashMap<ServiceId, ServiceEvent>,
    pub user_src: HashMap<String, Option<String>>,
    /// User files already reported as replacing a built-in widget.
    pub replaced_reported: HashSet<String>,
    pub cur: Option<Cur>,
    pub cur_task: Option<u64>,
    pub diags: Vec<Diagnostic>,
    pub log_total: Option<Window>,
    /// When lines dropped by the shared allowance were last reported.
    pub log_total_reported: Option<Instant>,
    pub event_subs: HashMap<u64, EventSub>,
    pub next_sub: u64,
    pub turn: u64,
    /// Set while the first run of a click or key handler (or `rsb popup`) is
    /// on the stack: the id of that user input. The only time a
    /// gesture-only command may run (`rsb.open_url`, power actions, ...).
    pub gesture: Option<u64>,
    pub last_open: Option<Instant>,
    /// Popups made by `rsb.popup` and `rsb.menu`, by id.
    pub popups: BTreeMap<u64, PopReg>,
    pub next_popup: u64,
    /// Open states to push into every VM: (`name@monitor`, open).
    pub popup_events: Vec<(String, bool)>,
    /// `name@monitor` keys of popups that are open now.
    pub popup_open: HashSet<String>,
    pub dark: bool,
    pub vm_mem: HashMap<VmId, usize>,
    /// Plates and trees queued for the end of the turn.
    pub plates: Vec<(InstanceId, Option<Plate>)>,
    pub cleared: Vec<InstanceId>,
    /// Failures to tell the bar plate about, and the ones that ended.
    pub error_notes: Vec<(InstanceId, Option<Diagnostic>)>,
    /// Instances the UI currently shows a plate for, so a later good load
    /// can tell it to clear.
    pub plates_shown: BTreeSet<InstanceId>,
}

/// One popup or menu a widget made, as the command lookup sees it.
#[derive(Clone, Debug)]
pub(crate) struct PopReg {
    pub inst: InstanceId,
    /// Monitor handle of the instance, for `name@monitor` keys.
    pub monitor: u64,
    pub names: Vec<String>,
    pub open: bool,
}

impl State {
    /// Recomputes whether each of `names` is open on `monitor` and queues a
    /// push into every VM for the ones that changed.
    pub(crate) fn popup_sync(&mut self, names: &[String], monitor: u64) {
        for name in names {
            let open = self.popups.values().any(|r| r.monitor == monitor && r.open && r.names.contains(name));
            let key = format!("{name}@{monitor}");
            let was = self.popup_open.contains(&key);
            if open != was {
                if open {
                    self.popup_open.insert(key.clone());
                } else {
                    self.popup_open.remove(&key);
                }
                self.popup_events.push((key, open));
            }
        }
    }

    /// Forgets one popup (its widget stopped or the popup was disposed).
    pub(crate) fn popup_drop(&mut self, pid: u64) {
        if let Some(reg) = self.popups.remove(&pid) {
            self.popup_sync(&reg.names, reg.monitor);
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Cur {
    pub inst: Option<InstanceId>,
}

pub(crate) struct Env {
    pub sources: Arc<dyn SourceStore>,
    pub clock: Arc<dyn Clock>,
    pub config_dir: PathBuf,
    pub path_env: PathEnv,
    pub limits: Limits,
    pub thread_time: Option<Arc<dyn ThreadTime>>,
    pub heartbeat: Arc<Heartbeat>,
}

/// Cells read by the interrupt on every Luau safepoint.
pub(crate) struct Budget {
    pub active: Cell<bool>,
    pub tick: Cell<u32>,
    pub started: Cell<Option<Instant>>,
    pub started_cpu: Cell<Duration>,
    /// Time of finished slices of the running call not yet added to the
    /// instance's account.
    pub pending: Cell<Duration>,
    pub soft: Cell<Duration>,
    pub hard: Cell<Duration>,
    pub label: Cell<&'static str>,
    pub loading: Cell<bool>,
    pub mem_soft: Cell<usize>,
    pub mem_limit: Cell<usize>,
    pub killed: Cell<bool>,
    pub stop: RefCell<Option<Stop>>,
}

pub(crate) struct Shared {
    pub env: Env,
    pub hub: Box<dyn HubPort>,
    pub out: Box<dyn ScriptSink>,
    pub st: RefCell<State>,
    pub bud: Budget,
}

pub(crate) fn stopped_error() -> Error {
    Error::runtime("script stopped")
}

/// First line of an error, without the traceback mlua adds.
pub(crate) fn error_text(e: &Error) -> String {
    let full = match e {
        Error::CallbackError { cause, .. } => error_text(cause),
        Error::WithContext { cause, .. } => error_text(cause),
        Error::RuntimeError(s) | Error::MemoryError(s) => s.clone(),
        Error::SyntaxError { message, .. } => message.clone(),
        other => other.to_string(),
    };
    let cut = full.split("\nstack traceback").next().unwrap_or(&full);
    let line = cut.trim();
    let mut out: String = line.chars().take(400).collect();
    if out.is_empty() {
        out = "unknown error".into();
    }
    out
}

pub(crate) fn is_memory_error(e: &Error) -> bool {
    match e {
        Error::MemoryError(_) => true,
        Error::CallbackError { cause, .. } | Error::WithContext { cause, .. } => is_memory_error(cause),
        _ => false,
    }
}

impl Shared {
    pub(crate) fn new(env: Env, hub: Box<dyn HubPort>, out: Box<dyn ScriptSink>) -> Shared {
        let st = State { dark: false, ..State::default() };
        Shared {
            env,
            hub,
            out,
            st: RefCell::new(st),
            bud: Budget {
                active: Cell::new(false),
                tick: Cell::new(0),
                started: Cell::new(None),
                started_cpu: Cell::new(Duration::ZERO),
                pending: Cell::new(Duration::ZERO),
                soft: Cell::new(Duration::ZERO),
                hard: Cell::new(Duration::ZERO),
                label: Cell::new("call"),
                loading: Cell::new(false),
                mem_soft: Cell::new(usize::MAX),
                mem_limit: Cell::new(usize::MAX),
                killed: Cell::new(false),
                stop: RefCell::new(None),
            },
        }
    }

    pub(crate) fn mono(&self) -> Instant {
        self.env.clock.mono()
    }

    /// Raises in a native once an instance has been told to stop, so a
    /// script that caught the first error with pcall cannot go on using the
    /// API.
    pub(crate) fn guard(&self) -> mlua::Result<()> {
        if self.bud.killed.get() { Err(stopped_error()) } else { Ok(()) }
    }

    /// Marks the running call as over a limit and returns the error to raise.
    pub(crate) fn kill(&self, stop: Stop) -> Error {
        if !self.bud.killed.get() {
            self.bud.killed.set(true);
            *self.bud.stop.borrow_mut() = Some(stop);
        }
        stopped_error()
    }

    /// Looks at the clock; raises (and marks the kill) when a limit is
    /// passed. Called by the interrupt every few safepoints and by Rust
    /// callbacks that walk big inputs.
    pub(crate) fn check_limits(&self, lua: Option<&Lua>) -> mlua::Result<()> {
        let b = &self.bud;
        if b.killed.get() {
            return Err(stopped_error());
        }
        if !b.active.get() {
            return Ok(());
        }
        let Some(started) = b.started.get() else { return Ok(()) };
        let now = Instant::now();
        let wall = now.saturating_duration_since(started);
        if wall >= b.hard.get() {
            return Err(self.kill(Stop::Time {
                budget: b.hard.get(),
                used: wall,
                label: b.label.get(),
                loading: b.loading.get(),
            }));
        }
        let used = match &self.env.thread_time {
            Some(tt) => tt.now().saturating_sub(b.started_cpu.get()),
            None => wall,
        };
        if used >= b.soft.get() {
            return Err(self.kill(Stop::Time {
                budget: b.soft.get(),
                used,
                label: b.label.get(),
                loading: b.loading.get(),
            }));
        }
        if let Some(lua) = lua
            && lua.used_memory() >= b.mem_soft.get()
        {
            return Err(self.kill(Stop::Memory { limit: b.mem_limit.get() }));
        }
        Ok(())
    }

    fn begin(&self, budget: Duration, label: &'static str, loading: bool) {
        let b = &self.bud;
        b.started.set(Some(Instant::now()));
        b.started_cpu.set(self.env.thread_time.as_ref().map_or(Duration::ZERO, |t| t.now()));
        b.soft.set(budget);
        b.hard.set(self.env.limits.hard_wall_budget.max(budget));
        b.label.set(label);
        b.loading.set(loading);
        b.active.set(true);
    }

    /// Time used since `begin`: (wall, cpu-or-wall).
    fn used(&self) -> (Duration, Duration) {
        let wall = self.bud.started.get().map_or(Duration::ZERO, |s| s.elapsed());
        let cpu = match &self.env.thread_time {
            Some(tt) => tt.now().saturating_sub(self.bud.started_cpu.get()),
            None => wall,
        };
        (wall, cpu)
    }

    /// Adds the time since the last mark to the instance's account and
    /// returns a stop if its CPU window is over.
    fn account(&self, inst: Option<InstanceId>, cpu: Duration, calls: u64) -> Option<Stop> {
        let inst = inst?;
        let now = self.mono();
        let lim = &self.env.limits;
        let mut st = self.st.borrow_mut();
        let i = st.instances.get_mut(&inst)?;
        i.calls += calls;
        i.script_time += cpu;
        i.script_time_max = i.script_time_max.max(cpu);
        i.cpu.push_back((now, cpu));
        i.cpu_sum += cpu;
        while let Some((t, d)) = i.cpu.front() {
            if now.saturating_duration_since(*t) > lim.cpu_window {
                i.cpu_sum = i.cpu_sum.saturating_sub(*d);
                i.cpu.pop_front();
            } else {
                break;
            }
        }
        let sum = i.cpu_sum;
        if sum > lim.cpu_window_budget { Some(Stop::Cpu { used: sum }) } else { None }
    }

    /// Splits a long call into budget slices (between effect runs): the
    /// time so far goes to the instance and the budget restarts. The CPU
    /// window still sees all of it.
    pub(crate) fn slice(&self) -> mlua::Result<()> {
        self.guard()?;
        if !self.bud.active.get() {
            return Ok(());
        }
        let (_, cpu) = self.used();
        let label = self.bud.label.get();
        let soft = self.bud.soft.get();
        // Short slices are added up and accounted together: this runs once
        // per effect, and the account is the expensive part.
        let pending = self.bud.pending.get() + cpu;
        if pending >= Duration::from_millis(1) {
            self.bud.pending.set(Duration::ZERO);
            let inst = self.st.borrow().cur.and_then(|c| c.inst);
            if let Some(stop) = self.account(inst, pending, 0) {
                return Err(self.kill(stop));
            }
        } else {
            self.bud.pending.set(pending);
        }
        self.begin(soft, label, self.bud.loading.get());
        Ok(())
    }

    /// Runs `f` as one call into a VM: heartbeat, budget, accounting, and
    /// classification of the result. `reset` repairs the VM's bookkeeping
    /// after a failure.
    pub(crate) fn guarded<R>(&self, call: Call, f: impl FnOnce() -> mlua::Result<R>) -> Outcome<R> {
        let Call { inst, label, budget, loading, mem, reset } = call;
        self.bud.killed.set(false);
        *self.bud.stop.borrow_mut() = None;
        self.bud.tick.set(0);
        self.bud.pending.set(Duration::ZERO);
        self.bud.mem_limit.set(mem.0);
        self.bud.mem_soft.set(mem.1);
        self.st.borrow_mut().cur = Some(Cur { inst });
        self.env.heartbeat.begin(inst.unwrap_or(InstanceId(0)), self.mono());
        self.begin(budget, label, loading);
        let r = f();
        let (wall, cpu) = self.used();
        let cpu = cpu + self.bud.pending.replace(Duration::ZERO);
        self.bud.active.set(false);
        self.env.heartbeat.end();
        self.st.borrow_mut().cur = None;
        let window_stop = self.account(inst, cpu, 1);
        let mut stop = self.bud.stop.borrow_mut().take();
        self.bud.killed.set(false);
        if stop.is_none() && wall > self.env.limits.hard_wall_budget.max(budget) {
            stop = Some(Stop::Time { budget: self.env.limits.hard_wall_budget, used: wall, label, loading });
        }
        if stop.is_none() {
            stop = window_stop;
        }
        let outcome = match (stop, r) {
            (Some(s), _) => Outcome::Stopped(s),
            (None, Ok(v)) => Outcome::Ok(v),
            (None, Err(e)) if is_memory_error(&e) => Outcome::Stopped(Stop::Memory { limit: mem.0 }),
            (None, Err(e)) => Outcome::Err(error_text(&e)),
        };
        if !matches!(outcome, Outcome::Ok(_))
            && let Some(f) = reset
        {
            let _ = f.call::<()>(());
        }
        outcome
    }

    pub(crate) fn current_instance(&self) -> Option<InstanceId> {
        self.st.borrow().cur.and_then(|c| c.inst)
    }

    /// Writes a line to the log on behalf of a widget.
    pub(crate) fn script_log(&self, inst: Option<InstanceId>, level: log::Level, text: &str) {
        let now = self.mono();
        let lim = &self.env.limits;
        let mut st = self.st.borrow_mut();
        let (name, ok, report) = match inst.and_then(|i| st.instances.get_mut(&i)) {
            Some(i) => {
                let ok = i.log_win.allow(now, lim.max_log_lines_per_sec);
                let report = if i.log_win.dropped > 0
                    && i.log_reported.is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_secs(60))
                {
                    i.log_reported = Some(now);
                    Some(i.log_win.dropped)
                } else {
                    None
                };
                (i.ref_text.clone(), ok, report)
            }
            None => (String::new(), true, None),
        };
        // Only a line its own widget may write counts against the shared allowance, so one chatty widget cannot
        // use it up with lines that were dropped anyway.
        let total_ok = ok && st.log_total.get_or_insert_with(Window::new).allow(now, lim.max_log_lines_total_per_sec);
        drop(st);
        if let Some(n) = report {
            log::warn!("{name}: {n} log lines dropped (limit {} a second)", lim.max_log_lines_per_sec);
        }
        if !total_ok {
            // Say so now and then, or a flood across many widgets would vanish without a trace.
            let dropped = {
                let mut st = self.st.borrow_mut();
                let due =
                    st.log_total_reported.is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_secs(60));
                let n = st.log_total.as_ref().map_or(0, |w| w.dropped);
                if due && n > 0 {
                    st.log_total_reported = Some(now);
                    Some(n)
                } else {
                    None
                }
            };
            if let Some(n) = dropped {
                log::warn!(
                    "{n} script log lines dropped across all widgets (limit {} a second)",
                    lim.max_log_lines_total_per_sec
                );
            }
            return;
        }
        let clean = escape_log(text, 200);
        if name.is_empty() {
            log::log!(target: "script", level, "{clean}");
        } else {
            log::log!(target: "script", level, "{name}: {clean}");
        }
    }

    /// An uncaught error outside load and setup: logged, class `error` on
    /// the widget's root for 10 s, and a stop after 10 within 10 s.
    pub(crate) fn record_error(&self, inst: Option<InstanceId>, msg: &str) -> Option<Stop> {
        let inst = inst?;
        let now = self.mono();
        let lim = &self.env.limits;
        let (name, stop) = {
            let mut st = self.st.borrow_mut();
            let i = st.instances.get_mut(&inst)?;
            i.errors += 1;
            i.dirty = true;
            i.error_until = Some(now + Duration::from_secs(10));
            let (pos, text) = error_line(msg);
            i.last_error = Some(match &pos {
                Some(p) => format!("{p} {text}"),
                None => text.clone(),
            });
            let note = Diagnostic::warning(DiagKind::Script, pos, text);
            st.error_notes.push((inst, Some(note)));
            let i = st.instances.get_mut(&inst)?;
            i.err_times.push_back(now);
            while let Some(t) = i.err_times.front() {
                if now.saturating_duration_since(*t) > lim.error_window {
                    i.err_times.pop_front();
                } else {
                    break;
                }
            }
            let stop = if i.err_times.len() as u32 >= lim.max_errors { Some(Stop::Errors) } else { None };
            (i.ref_text.clone(), stop)
        };
        self.script_log(Some(inst), log::Level::Error, &format!("error: {msg}"));
        let _ = name;
        stop
    }

    pub(crate) fn next_io_id(&self) -> IoId {
        let mut st = self.st.borrow_mut();
        st.next_io += 1;
        IoId(st.next_io)
    }
}

/// `widgets/doc/broken.luau:20: attempt to index nil` as a position (`doc/broken.luau`, line 20) and the
/// message, cut and cleaned for the screen. A message with no such start keeps its text.
pub(crate) fn error_line(msg: &str) -> (Option<rsb_core::diag::SourcePos>, String) {
    let first = msg.lines().next().unwrap_or("");
    if let Some((file, rest)) = first.split_once(".luau:")
        && let Some((line, text)) = rest.split_once(':')
        && let Ok(line) = line.trim().parse::<u32>()
        && !file.contains(['\\', ':'])
    {
        let name = file.strip_prefix("widgets/").unwrap_or(file);
        return (Some(rsb_core::diag::SourcePos::new(&format!("{name}.luau"), line, 0)), escape_log(text.trim(), 100));
    }
    (None, escape_log(first.trim(), 100))
}

/// Control characters and line breaks are escaped so a script cannot forge
/// log lines; long text is cut.
pub(crate) fn escape_log(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (n, c) in text.chars().enumerate() {
        if n >= max_chars {
            out.push_str("...");
            break;
        }
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() && (c as u32) < 0x100 => out.push_str(&format!("\\x{:02x}", c as u32)),
            // Line separators and bidirectional controls can reorder or split what a reader sees.
            c if c.is_control() || rsb_core::diag::is_invisible_control(c) => {
                out.push_str(&format!("\\u{{{:04x}}}", c as u32))
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_text_is_escaped_and_cut() {
        assert_eq!(escape_log("a\nb\r\x1b[31m", 50), "a\\nb\\r\\x1b[31m");
        assert_eq!(escape_log("a\u{2028}b\u{202e}c\u{2066}d\u{85}", 50), "a\\u{2028}b\\u{202e}c\\u{2066}d\\x85");
        let long = "x".repeat(300);
        let cut = escape_log(&long, 200);
        assert_eq!(cut.len(), 203);
        assert!(cut.ends_with("..."));
    }

    #[test]
    fn a_tail_after_a_question_starts_a_new_sentence() {
        assert_eq!(with_tail("bad value", "fix it and save"), "bad value; fix it and save");
        assert_eq!(
            with_tail("rsb.txt does not exist; did you mean rsb.text?", "fix it and save"),
            "rsb.txt does not exist; did you mean rsb.text? Fix it and save"
        );
        assert_eq!(with_tail("x?", ""), "x? ");
        assert!(!with_tail("did you mean y?", "restarting - check its settings in bar.toml").contains("?;"));
    }

    #[test]
    fn window_limits_per_second() {
        let mut w = Window::new();
        let t = Instant::now();
        let allowed = (0..30).filter(|_| w.allow(t, 20)).count();
        assert_eq!(allowed, 20);
        assert_eq!(w.dropped, 10);
        assert!(w.allow(t + Duration::from_millis(1100), 20));
    }

    #[test]
    fn stop_texts_match_the_documented_copy() {
        let s = Stop::Time {
            budget: Duration::from_millis(10),
            used: Duration::from_micros(10400),
            label: "on_click",
            loading: false,
        };
        assert_eq!(
            s.diagnostic("my/weather", false).plate_text(),
            "STOPPED - my/weather ran over 10 ms; fix it and save"
        );
        assert_eq!(s.diagnostic("clock", true).plate_text(), "STOPPED - clock ran over 10 ms; restarting");
        assert_eq!(s.log_line("my/weather"), "stopped my/weather: 10.4 ms in on_click");
        let p = Stop::Permission("net:api.open-meteo.com".into());
        assert_eq!(
            p.diagnostic("my/weather", false).plate_text(),
            "PERMISSION - my/weather needs net:api.open-meteo.com; add it to permissions"
        );
        assert_eq!(
            p.diagnostic("clock", true).plate_text(),
            "PERMISSION - clock needs net:api.open-meteo.com; restarting"
        );
        assert_eq!(p.log_line("my/weather"), "permission denied: my/weather needs net:api.open-meteo.com");
        assert_eq!(
            Stop::Memory { limit: 32 << 20 }.diagnostic("my/weather", false).plate_text(),
            "STOPPED - my/weather used over 32 MB; fix it and save"
        );
        assert_eq!(Stop::Cpu { used: Duration::from_millis(262) }.log_line("my/x"), "stopped my/x: 262 ms CPU in 1 s");
        assert_eq!(
            Stop::UpdateLoop.diagnostic("my/x", false).plate_text(),
            "STOPPED - my/x is in an update loop (an effect changes what it reads)"
        );
        assert_eq!(
            Stop::UpdateLoop.diagnostic("clock", true).plate_text(),
            "STOPPED - clock is in an update loop (an effect changes what it reads); restarting"
        );
    }
}

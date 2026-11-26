//! Messages between threads that live in different crates. Every channel is
//! a `std::sync::mpsc` channel owned by the receiving thread; senders never
//! block (unbounded), so the receiving side is responsible for coalescing.
//!
//! ```text
//! services hub --HubEvent--> script thread --ScriptOut--> UI thread
//!      ^                         |    ^                      |
//!      +-------HubRequest--------+    +------ScriptIn--------+
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use crate::config::{WidgetOptions, WidgetRef};
use crate::diag::{Diagnostic, Plate, SourcePos};
use crate::element::{InstanceId, WidgetTree};
use crate::hit::{ElementCall, HandlerCall};
use crate::io::{IoEvent, IoGrant, IoId, IoRequest};
use crate::services::{ServiceCommand, ServiceEvent, ServiceId, ServiceStatus};
use crate::value::Value;

// ---------------------------------------------------------------- services

/// To the services hub.
///
/// Subscriptions are a set keyed by (service, owner), not a count: a
/// repeated `Subscribe` from the same owner replaces its interval (that is
/// how `rsb.services.system:interval("1s")` changes it), and one
/// `Unsubscribe` or `CancelOwner` removes it whatever came before. A
/// service's interval is the minimum over its set; it stops 2 s after the
/// set becomes empty, and its last value is forgotten then, so the first
/// value after a restart is always sent even if it equals the old one.
#[derive(Debug)]
pub enum HubRequest {
    /// `interval`: wanted sampling period for sampled services (None = the
    /// service default).
    Subscribe {
        service: ServiceId,
        owner: InstanceId,
        interval: Option<Duration>,
    },
    Unsubscribe {
        service: ServiceId,
        owner: InstanceId,
    },
    Command {
        owner: InstanceId,
        command: ServiceCommand,
    },
    /// `grant` is what the widget may do; the services crate checks redirect
    /// hops and opened files' final paths against it.
    Io {
        id: IoId,
        owner: InstanceId,
        request: IoRequest,
        grant: Arc<IoGrant>,
    },
    /// Stop one request (kills a process, drops a watch).
    Cancel {
        id: IoId,
    },
    /// Everything an instance started: subscriptions, processes, watches,
    /// requests. Sent when an instance unloads or is stopped.
    CancelOwner {
        owner: InstanceId,
    },
    Status {
        reply: Sender<Vec<ServiceInfo>>,
    },
    Shutdown,
}

/// From the services hub.
#[derive(Clone, Debug, PartialEq)]
pub enum HubEvent {
    Service(ServiceEvent),
    Io(IoEvent),
    /// An image a service made (album art, an app icon), keyed
    /// `rsb-image:<service>/<id>` as its value names it. Held in memory
    /// only. The binary routes it to the UI thread's renderer, never to
    /// the script thread.
    Image {
        key: Arc<str>,
        image: crate::pixels::RgbaImage,
    },
}

/// Sending side of the hub, as the script runtime sees it.
pub trait HubPort: Send {
    fn send(&self, request: HubRequest);
}

#[derive(Clone, Debug, PartialEq)]
pub struct ServiceInfo {
    pub id: ServiceId,
    pub status: ServiceStatus,
    pub subscribers: u32,
    pub interval: Option<Duration>,
    pub version: u64,
    /// Since the last value or liveness signal.
    pub last_update_ago: Option<Duration>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------- scripts

/// A monitor as a widget sees it (`ctx.monitor`).
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorRef {
    pub index: u32,
    /// HMONITOR value; GlazeWM-compatible window managers report the same
    /// number as `handle`.
    pub handle: u64,
    pub device: String,
    pub primary: bool,
    pub scale: f32,
}

/// One widget instance to run.
///
/// Identity (what decides "same instance, keep its state") is everything
/// except `monitor.scale` and `pos`: a DPI change or an edit that only moves
/// the line updates `ctx.monitor.scale` / the position in place instead of
/// restarting the widget.
#[derive(Clone, Debug, PartialEq)]
pub struct InstanceSpec {
    pub instance: InstanceId,
    pub widget: WidgetRef,
    pub options: WidgetOptions,
    /// Bar name (`ctx.bar`).
    pub bar: String,
    pub monitor: MonitorRef,
    /// Where the ref was written in `bar.toml`, for "unknown widget" and
    /// option warnings. None when it came from defaults or a preset.
    pub pos: Option<SourcePos>,
}

impl InstanceSpec {
    /// Same instance for `SetInstances`: equal apart from scale and position.
    pub fn same_identity(&self, other: &InstanceSpec) -> bool {
        self.instance == other.instance
            && self.widget == other.widget
            && self.options == other.options
            && self.bar == other.bar
            && self.monitor.index == other.monitor.index
            && self.monitor.handle == other.monitor.handle
            && self.monitor.device == other.monitor.device
            && self.monitor.primary == other.monitor.primary
    }
}

/// To the script thread.
#[derive(Debug)]
pub enum ScriptIn {
    /// The full set of instances that should run. Instances already running
    /// with the same identity (`InstanceSpec::same_identity`) keep their
    /// state; others start or stop.
    SetInstances(Vec<InstanceSpec>),
    /// A file under `widgets\` changed. `rel_path` uses `/`, relative to the
    /// config folder; `source` None = deleted. Instances using that file
    /// (or a module it requires) reload; others are untouched.
    FileChanged {
        rel_path: String,
        source: Option<String>,
    },
    Hub(HubEvent),
    Input(HandlerCall),
    /// From `rsb trigger <event> [json]`.
    Trigger {
        event: String,
        data: Value,
    },
    Theme {
        dark: bool,
    },
    /// The wall clock jumped (resume from sleep, a manual time or time zone
    /// change). Wall-aligned timers are re-armed and `rsb.clock` signals
    /// refresh at once: waits do not count time spent asleep, so without
    /// this a minute clock could show the old time for up to a minute.
    ClockChanged,
    /// Answered by the script thread itself; the pipe thread waits at most
    /// 1 s for it, never the UI thread.
    Status {
        reply: Sender<Vec<WidgetStatus>>,
    },
    Shutdown,
    /// A key, text, value, focus, scroll or popup-closed event for one
    /// handler (from the scenes, like `Input`).
    Element(ElementCall),
    /// `rsb popup <name> [action]`: open, close or toggle the popup with
    /// that name; `monitor` (HMONITOR value) picks among instances on
    /// several monitors, None = the primary monitor's. `gesture` is the id
    /// the UI gave this command (a hotkey tool running `rsb popup` is a
    /// user's action); the runtime handles it inside that gesture.
    Popup {
        name: String,
        action: crate::popup::PopupAction,
        monitor: Option<u64>,
        gesture: u64,
    },
}

/// From the script thread. Sent at the end of each script turn, so one
/// burst of changes is one message.
#[derive(Debug)]
pub enum ScriptOut {
    Trees(Vec<Arc<WidgetTree>>),
    /// A widget failed or recovered (None clears the plate).
    Plate {
        instance: InstanceId,
        plate: Option<Plate>,
    },
    /// Already logged by the script thread; the UI decides what is shown.
    Diagnostics(Vec<Diagnostic>),
    /// A widget's code failed (a handler, an effect, a popup's content): the
    /// bar plate counts it for as long as the widget shows its error state.
    /// None clears it.
    ErrorNote {
        instance: InstanceId,
        note: Option<Diagnostic>,
    },
}

/// Receiving side of the UI, as the script thread sees it (a channel plus a
/// wake of the UI thread).
pub trait ScriptSink: Send {
    fn send(&self, out: ScriptOut);
}

#[derive(Clone, Debug, PartialEq)]
pub enum WidgetState {
    Loading,
    Running,
    /// Stopped for a budget, an error or a permission; the text is the
    /// plate text.
    Stopped(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct WidgetStatus {
    pub instance: InstanceId,
    pub widget: String,
    pub bar: String,
    pub state: WidgetState,
    /// Script time spent in this instance since it loaded.
    pub script_time: Duration,
    pub script_time_max: Duration,
    pub calls: u64,
    /// Memory of the VM the instance runs in, bytes.
    pub vm_bytes: usize,
    /// Several instances share that VM: the built-ins, or a user widget
    /// placed more than once.
    pub shared_vm: bool,
    pub source: WidgetSource,
    /// A user file stands in for the built-in widget of the same name (its
    /// popups and hotkey included).
    pub replaces_builtin: bool,
    /// Uncaught errors in handlers, timers, tasks, effects and function
    /// props since load (these do not stop the widget unless 10 come within
    /// 10 s).
    pub errors: u64,
    /// Times a built-in was restarted after being stopped.
    pub restarts: u32,
}

/// Lets a watchdog see a script call that never returns. The script thread
/// calls `begin` before every call into a VM and `end` after it; it never
/// blocks. The binary's watchdog thread checks `busy` once a second and,
/// after 2 s, shows the bar plate `STOPPED - widgets are not responding
/// (<ref>); see log` and logs once; the instance is stopped when the call
/// finally returns. This covers what the interrupt cannot see (a long call
/// inside a C library function or a Rust callback).
#[derive(Debug)]
pub struct Heartbeat {
    base: Instant,
    /// Milliseconds since `base` when the current call began, plus 1
    /// (0 = idle).
    started: AtomicU64,
    instance: AtomicU32,
    /// Counts calls, so a watchdog can tell one long call from many short ones.
    calls: AtomicU64,
}

impl Default for Heartbeat {
    fn default() -> Self {
        Heartbeat::new()
    }
}

impl Heartbeat {
    pub fn new() -> Heartbeat {
        Heartbeat {
            base: Instant::now(),
            started: AtomicU64::new(0),
            instance: AtomicU32::new(0),
            calls: AtomicU64::new(0),
        }
    }

    fn ms(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.base).as_millis() as u64
    }

    /// `now` is the script thread's monotonic time (`Clock::mono`).
    pub fn begin(&self, instance: InstanceId, now: Instant) {
        self.instance.store(instance.0, Ordering::Relaxed);
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.started.store(self.ms(now) + 1, Ordering::Release);
    }

    pub fn end(&self) {
        self.started.store(0, Ordering::Release);
    }

    /// The instance in a call and how long it has run, if a call is running.
    /// The third value identifies the call.
    pub fn busy(&self, now: Instant) -> Option<(InstanceId, Duration, u64)> {
        let started = self.started.load(Ordering::Acquire);
        if started == 0 {
            return None;
        }
        let calls = self.calls.load(Ordering::Relaxed);
        let instance = InstanceId(self.instance.load(Ordering::Relaxed));
        Some((instance, Duration::from_millis(self.ms(now).saturating_sub(started - 1)), calls))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WidgetSource {
    Builtin,
    /// Relative to the config folder, `/` separators.
    User(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_reports_the_running_call() {
        let h = Heartbeat::new();
        let t = Instant::now();
        assert_eq!(h.busy(t), None);
        h.begin(InstanceId(7), t);
        let (i, d, call) = h.busy(t + Duration::from_millis(2500)).unwrap();
        assert_eq!((i, d), (InstanceId(7), Duration::from_millis(2500)));
        h.end();
        assert_eq!(h.busy(t), None);
        h.begin(InstanceId(7), t);
        assert!(h.busy(t).is_some_and(|b| b.2 > call));
    }
}

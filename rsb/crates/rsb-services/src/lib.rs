//! Services and script I/O.
//!
//! The `Hub` runs on its own thread. It starts a service when its first
//! subscriber arrives and stops it when the last one leaves, stamps every
//! value with a version, forwards it to the sink, and marks a service stale
//! when it has sent neither a value nor a liveness signal within its
//! `stale_after`. Each service runs on threads of its own (COM and WinRT
//! work on dedicated MTA threads, WinEvent hooks on a thread with a message
//! loop), so a slow or hung source never delays another.
//!
//! Script I/O (exec, HTTP, file read and watch, opening a URL) also enters
//! through the hub and runs on worker threads with the caps in
//! `rsb_core::io::limits`. Every request carries the widget's `IoGrant`:
//! HTTP never lets WinHTTP follow redirects (each hop is followed here, at
//! most 5, only to hosts the grant allows, never from https to http) and
//! files are checked again by their final path (`files::open_permitted`).
//!
//! Provider data is untrusted: the GlazeWM-compatible client caps messages
//! and frames, the komorebi reader caps each JSON object, and both cut the
//! model to the limits below. The komorebi pipe is created with
//! `FILE_FLAG_FIRST_PIPE_INSTANCE` and `PIPE_REJECT_REMOTE_CLIENTS` and an
//! owner-only DACL; the WebSocket provider is used only when the process
//! listening on its port runs as the same user in the same session.
//!
//! Everything that would change the system (volume, mute, media playback,
//! workspace focus) has a fake in `fake` and is tested only against it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

use rsb_core::msg::{HubEvent, HubPort, HubRequest};
use rsb_core::services::{ServiceCommand, ServiceId, ServiceStatus};
use rsb_core::value::Value;

pub mod clock;
pub mod fake;
pub mod files;
pub mod watch;

mod hub;
mod ioreq;
mod real;
mod util;
mod win;
mod workspaces;

pub use hub::{Hub, HubTiming};
pub use ioreq::UrlOpener;
pub use real::RealFactory;
pub use workspaces::{KomorebiControl, workspaces_service};

/// Largest WebSocket message and frame accepted from a window manager.
pub const WS_MAX_MESSAGE: usize = 1 << 20;
/// Largest single JSON object accepted from komorebi.
pub const KOMOREBI_MAX_OBJECT: usize = 4 << 20;
/// Workspaces model caps: monitors, workspaces per monitor, name length
/// (chars; longer names are cut).
pub const MAX_MONITORS: usize = 16;
pub const MAX_WORKSPACES_PER_MONITOR: usize = 64;
pub const MAX_WORKSPACE_NAME: usize = 64;

/// Sampling period used when no subscriber asked for one, and the shortest
/// one honoured.
pub const DEFAULT_SAMPLE_INTERVAL: Duration = Duration::from_secs(2);
pub const MIN_SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

/// Wakes of every source thread and connection, all services together
/// (reported by the binary as `wakes.sources`). A wake is one return from
/// the wait a source thread sits in.
static SOURCE_WAKES: AtomicU64 = AtomicU64::new(0);

/// Total source-thread wakes since the process started.
pub fn source_wakes() -> u64 {
    SOURCE_WAKES.load(Ordering::Relaxed)
}

pub(crate) fn count_source_wake() {
    SOURCE_WAKES.fetch_add(1, Ordering::Relaxed);
}

/// One data source. Implementations are driven by the hub thread: every
/// method returns quickly; real work happens on the service's own threads,
/// which report through the `Publisher`.
pub trait Service: Send {
    fn id(&self) -> ServiceId;
    /// Start sampling or listening. An `Err` is reported as status `Error`
    /// and retried with backoff (1 s doubling to 60 s).
    fn start(&mut self, out: Publisher) -> Result<(), String>;
    /// Sampled services: the period to sample at (fastest subscriber).
    fn set_interval(&mut self, interval: Duration) {
        let _ = interval;
    }
    fn command(&mut self, command: ServiceCommand) -> Result<(), String>;
    /// Stop and release everything; threads exit within 1 s.
    fn stop(&mut self);
    /// No value or `alive` for this long = stale.
    fn stale_after(&self) -> Duration;
    /// Services that need no thread of their own ask the hub for a timer:
    /// how long until the next `tick` (None = no timer). Asked after `start`,
    /// `set_interval` and every `tick`. A tick must return quickly.
    fn tick_after(&self) -> Option<Duration> {
        None
    }
    fn tick(&mut self) {}
}

/// What a service tells the hub. Private: services only see `Publisher`.
pub(crate) enum HubMsg {
    Request(HubRequest),
    Publish { service: ServiceId, generation: u64, value: Value, status: Option<(ServiceStatus, Option<String>)> },
    Alive { service: ServiceId, generation: u64 },
    Status { service: ServiceId, generation: u64, status: ServiceStatus, reason: Option<String> },
}

/// Where a service reports. Cloneable and `Send` for its threads. A
/// publisher belongs to one run of its service: after the service is
/// stopped, whatever an old thread still sends is ignored.
#[derive(Clone)]
pub struct Publisher {
    tx: Sender<HubMsg>,
    service: ServiceId,
    generation: u64,
}

impl Publisher {
    pub(crate) fn new(tx: Sender<HubMsg>, service: ServiceId, generation: u64) -> Publisher {
        Publisher { tx, service, generation }
    }

    /// The full current value (schema-shaped map). Unchanged values are
    /// dropped by the hub, but still count as liveness.
    pub fn publish(&self, value: Value) {
        let _ =
            self.tx.send(HubMsg::Publish { service: self.service, generation: self.generation, value, status: None });
    }

    /// A value together with a status, as one event: for a service whose
    /// value is meaningful while it is `Unavailable` (workspaces with no
    /// window manager, audio with no device).
    pub fn publish_as(&self, value: Value, status: ServiceStatus, reason: Option<String>) {
        let _ = self.tx.send(HubMsg::Publish {
            service: self.service,
            generation: self.generation,
            value,
            status: Some((status, reason)),
        });
    }

    /// "Still working, nothing changed."
    pub fn alive(&self) {
        let _ = self.tx.send(HubMsg::Alive { service: self.service, generation: self.generation });
    }

    /// `Unavailable` (nothing to report here) or `Error`, with a short reason.
    pub fn status(&self, status: ServiceStatus, reason: Option<String>) {
        let _ = self.tx.send(HubMsg::Status { service: self.service, generation: self.generation, status, reason });
    }
}

/// Makes services. `Real` builds the Windows-backed ones; tests pass a
/// `fake::FakeWorld`.
pub trait ServiceFactory: Send {
    fn create(&self, id: ServiceId, options: &HubOptions) -> Box<dyn Service>;

    /// What opens URLs for `rsb.open_url`; None = the system's default
    /// browser. Fakes return a recorder, so no test ever opens a browser.
    fn url_opener(&self) -> Option<std::sync::Arc<dyn UrlOpener>> {
        None
    }
}

#[derive(Clone, Debug)]
pub struct WorkspaceOptions {
    /// GlazeWM-compatible WebSocket (Astur, GlazeWM).
    pub glazewm_url: String,
    /// komorebi's command-line client, found on PATH.
    pub komorebic: String,
    /// Our pipe name for komorebi's `subscribe-pipe` (without `\\.\pipe\`).
    pub komorebi_pipe: String,
    /// Between provider checks while none is connected.
    pub detect_every: Duration,
}

impl Default for WorkspaceOptions {
    fn default() -> Self {
        WorkspaceOptions {
            glazewm_url: "ws://127.0.0.1:6123".into(),
            komorebic: "komorebic".into(),
            komorebi_pipe: format!("rsb-komorebi-{}", std::process::id()),
            detect_every: Duration::from_secs(5),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct HubOptions {
    pub workspaces: WorkspaceOptions,
}

/// Sending side of the hub.
#[derive(Clone)]
pub struct HubSender {
    tx: Sender<HubMsg>,
}

impl HubPort for HubSender {
    fn send(&self, request: HubRequest) {
        // A closed hub means shutdown is under way; requests are moot.
        let _ = self.tx.send(HubMsg::Request(request));
    }
}

/// The callback the hub reports through.
pub(crate) type Sink = dyn Fn(HubEvent) + Send + Sync;

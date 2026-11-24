//! The hub thread: subscriptions, versions, staleness, restarts and the
//! routing of script I/O. It never waits on a source; sources talk to it
//! through `Publisher` and it talks to them through short method calls.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rsb_core::element::InstanceId;
use rsb_core::msg::{HubEvent, HubRequest, ServiceInfo};
use rsb_core::services::{ServiceCommand, ServiceEvent, ServiceId, ServiceStatus, check_value};
use rsb_core::value::Value;

use crate::ioreq::IoManager;
use crate::{
    DEFAULT_SAMPLE_INTERVAL, HubMsg, HubOptions, HubSender, MIN_SAMPLE_INTERVAL, Publisher, Service, ServiceFactory,
    Sink,
};

/// Timings the hub uses. The defaults are the documented behaviour; tests
/// shorten them.
#[derive(Clone, Copy, Debug)]
pub struct HubTiming {
    /// A service stops this long after its last subscriber left.
    pub stop_delay: Duration,
    /// First retry after a failed start; doubles up to `retry_max`.
    pub retry_first: Duration,
    pub retry_max: Duration,
}

impl Default for HubTiming {
    fn default() -> Self {
        HubTiming {
            stop_delay: Duration::from_secs(2),
            retry_first: Duration::from_secs(1),
            retry_max: Duration::from_secs(60),
        }
    }
}

/// Handle to the running hub thread.
pub struct Hub {
    tx: Sender<HubMsg>,
    thread: Option<JoinHandle<()>>,
    done: Receiver<()>,
    wakes: Arc<AtomicU64>,
}

impl Hub {
    /// Starts the hub thread. `sink` is called from hub and worker threads
    /// and must not block (the binary forwards into the script channel).
    pub fn start(
        options: HubOptions,
        factory: Box<dyn ServiceFactory>,
        sink: Box<dyn Fn(HubEvent) + Send + Sync>,
    ) -> Hub {
        Hub::start_with(options, factory, sink, HubTiming::default())
    }

    pub fn start_with(
        options: HubOptions,
        factory: Box<dyn ServiceFactory>,
        sink: Box<dyn Fn(HubEvent) + Send + Sync>,
        timing: HubTiming,
    ) -> Hub {
        let (tx, rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let wakes = Arc::new(AtomicU64::new(0));
        let sink: Arc<Sink> = Arc::from(sink);
        let io = IoManager::new(sink.clone(), factory.url_opener());
        let core = Core::new(options, factory, sink, io, tx.clone(), timing);
        let wakes_in = wakes.clone();
        let thread = std::thread::Builder::new()
            .name("rsb-hub".into())
            .spawn(move || {
                run(core, rx, &wakes_in);
                let _ = done_tx.send(());
            })
            .expect("the hub thread starts");
        Hub { tx, thread: Some(thread), done, wakes }
    }

    /// The real services.
    pub fn real_factory() -> Box<dyn ServiceFactory> {
        Box::new(crate::real::RealFactory)
    }

    pub fn port(&self) -> HubSender {
        HubSender { tx: self.tx.clone() }
    }

    /// Times the hub thread woke up (a message or a timer), for the
    /// binary's wake counters.
    pub fn wakes(&self) -> u64 {
        self.wakes.load(Ordering::Relaxed)
    }

    /// Stops every service and worker; returns within `timeout` even if a
    /// source hangs (its thread is then left to exit on its own).
    pub fn shutdown(mut self, timeout: Duration) {
        let _ = self.tx.send(HubMsg::Request(HubRequest::Shutdown));
        match self.done.recv_timeout(timeout) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                if let Some(t) = self.thread.take() {
                    let _ = t.join();
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                log::warn!(
                    "services hub did not stop within {} ms; leaving it to exit with the process",
                    timeout.as_millis()
                );
            }
        }
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        // Dropped without `shutdown`: still stop the thread.
        if self.thread.is_some() {
            let _ = self.tx.send(HubMsg::Request(HubRequest::Shutdown));
        }
    }
}

struct Slot {
    id: ServiceId,
    service: Option<Box<dyn Service>>,
    subs: BTreeMap<InstanceId, Option<Duration>>,
    running: bool,
    generation: u64,
    status: ServiceStatus,
    version: u64,
    last_value: Option<Value>,
    /// Last value, liveness signal or start.
    last_update: Instant,
    error: Option<String>,
    stop_at: Option<Instant>,
    retry_at: Option<Instant>,
    retry_delay: Duration,
    /// Period last handed to the service.
    interval: Option<Duration>,
    tick_at: Option<Instant>,
    stale_after: Duration,
    stale_since: Option<Instant>,
}

impl Slot {
    fn new(id: ServiceId, timing: &HubTiming) -> Slot {
        Slot {
            id,
            service: None,
            subs: BTreeMap::new(),
            running: false,
            generation: 0,
            status: ServiceStatus::Off,
            version: 0,
            last_value: None,
            last_update: Instant::now(),
            error: None,
            stop_at: None,
            retry_at: None,
            retry_delay: timing.retry_first,
            interval: None,
            tick_at: None,
            stale_after: Duration::from_secs(180),
            stale_since: None,
        }
    }

    /// The fastest period any subscriber asked for.
    fn wanted_interval(&self) -> Option<Duration> {
        self.subs.values().flatten().min().map(|d| (*d).max(MIN_SAMPLE_INTERVAL))
    }
}

struct Core {
    options: HubOptions,
    factory: Box<dyn ServiceFactory>,
    sink: Arc<Sink>,
    io: IoManager,
    tx: Sender<HubMsg>,
    slots: Vec<Slot>,
    next_generation: u64,
    timing: HubTiming,
}

fn command_service(command: &ServiceCommand) -> ServiceId {
    match command {
        ServiceCommand::Audio(_) => ServiceId::Audio,
        ServiceCommand::Media(_) => ServiceId::Media,
        ServiceCommand::Workspaces(_) => ServiceId::Workspaces,
    }
}

impl Core {
    fn new(
        options: HubOptions,
        factory: Box<dyn ServiceFactory>,
        sink: Arc<Sink>,
        io: IoManager,
        tx: Sender<HubMsg>,
        timing: HubTiming,
    ) -> Core {
        let slots = ServiceId::ALL.iter().map(|id| Slot::new(*id, &timing)).collect();
        Core { options, factory, sink, io, tx, slots, next_generation: 0, timing }
    }

    fn index(id: ServiceId) -> usize {
        ServiceId::ALL.iter().position(|s| *s == id).expect("every service has a slot")
    }

    // ---------------------------------------------------------- events

    fn emit(&mut self, i: usize, value: Option<Value>) {
        let slot = &mut self.slots[i];
        slot.version += 1;
        let event = ServiceEvent {
            service: slot.id,
            version: slot.version,
            status: slot.status,
            value,
            error: slot.error.clone(),
        };
        (self.sink)(HubEvent::Service(event));
    }

    /// Changes status and reason; one event and (for the states widgets care
    /// about) one log line per change. Never logs a value.
    fn set_status(&mut self, i: usize, status: ServiceStatus, reason: Option<String>, now: Instant) {
        let slot = &mut self.slots[i];
        if slot.status == status && slot.error == reason {
            return;
        }
        let name = slot.id.name();
        let was = slot.status;
        match status {
            ServiceStatus::Stale => {
                let secs = now.saturating_duration_since(slot.last_update).as_secs();
                log::warn!("service {name} stale after {secs} s");
                slot.stale_since = Some(now);
            }
            ServiceStatus::Error => {
                log::warn!("service {name} error: {}", reason.as_deref().unwrap_or("unknown"));
            }
            ServiceStatus::Unavailable => {
                log::info!("service {name} unavailable: {}", reason.as_deref().unwrap_or("nothing to report"));
            }
            ServiceStatus::Ok
                if matches!(was, ServiceStatus::Stale | ServiceStatus::Error | ServiceStatus::Unavailable) =>
            {
                log::info!("service {name} ok again");
                slot.stale_since = None;
            }
            ServiceStatus::Off | ServiceStatus::Starting | ServiceStatus::Ok => {
                log::debug!("service {name} {}", status.name());
            }
        }
        slot.status = status;
        slot.error = reason;
        self.emit(i, None);
    }

    // ------------------------------------------------------- lifecycle

    fn start_service(&mut self, i: usize, now: Instant) {
        let id = self.slots[i].id;
        self.next_generation += 1;
        let generation = self.next_generation;
        let publisher = Publisher::new(self.tx.clone(), id, generation);
        let interval = self.slots[i].wanted_interval().unwrap_or(DEFAULT_SAMPLE_INTERVAL);
        if self.slots[i].service.is_none() {
            let service = self.factory.create(id, &self.options);
            self.slots[i].service = Some(service);
        }
        let result = {
            let svc = self.slots[i].service.as_mut().expect("just created");
            svc.set_interval(interval);
            svc.start(publisher)
        };
        match result {
            Ok(()) => {
                let slot = &mut self.slots[i];
                slot.generation = generation;
                slot.running = true;
                slot.retry_at = None;
                slot.retry_delay = self.timing.retry_first;
                slot.last_update = now;
                slot.interval = slot.wanted_interval();
                let svc = slot.service.as_ref().expect("started");
                slot.stale_after = svc.stale_after();
                slot.tick_at = svc.tick_after().map(|d| now + d);
                self.set_status(i, ServiceStatus::Starting, None, now);
            }
            Err(reason) => {
                let slot = &mut self.slots[i];
                if let Some(svc) = slot.service.as_mut() {
                    svc.stop();
                }
                slot.generation = generation;
                slot.running = false;
                let delay = slot.retry_delay;
                slot.retry_at = Some(now + delay);
                slot.retry_delay = (delay * 2).min(self.timing.retry_max);
                self.set_status(i, ServiceStatus::Error, Some(reason), now);
            }
        }
    }

    fn stop_service(&mut self, i: usize, now: Instant) {
        let slot = &mut self.slots[i];
        slot.stop_at = None;
        slot.retry_at = None;
        slot.tick_at = None;
        slot.interval = None;
        // Publishers of the old run are ignored from here on.
        slot.generation = 0;
        if slot.running {
            slot.running = false;
            if let Some(svc) = slot.service.as_mut() {
                svc.stop();
            }
        }
        // The first value after a restart is sent even when it equals the
        // old one.
        slot.last_value = None;
        slot.stale_since = None;
        self.set_status(i, ServiceStatus::Off, None, now);
    }

    /// After subscriptions changed: starts, retunes, or schedules the stop.
    fn settle(&mut self, i: usize, now: Instant) {
        if self.slots[i].subs.is_empty() {
            let slot = &mut self.slots[i];
            if slot.running {
                if slot.stop_at.is_none() {
                    slot.stop_at = Some(now + self.timing.stop_delay);
                }
            } else {
                // Nothing running and nobody left: drop a pending retry.
                slot.retry_at = None;
                if slot.status != ServiceStatus::Off {
                    self.set_status(i, ServiceStatus::Off, None, now);
                }
            }
            return;
        }
        self.slots[i].stop_at = None;
        if !self.slots[i].running {
            if self.slots[i].retry_at.is_none() {
                self.start_service(i, now);
            }
            return;
        }
        let wanted = self.slots[i].wanted_interval();
        if wanted != self.slots[i].interval {
            let slot = &mut self.slots[i];
            slot.interval = wanted;
            if let Some(svc) = slot.service.as_mut() {
                svc.set_interval(wanted.unwrap_or(DEFAULT_SAMPLE_INTERVAL));
                slot.tick_at = svc.tick_after().map(|d| now + d);
            }
        }
    }

    // -------------------------------------------------------- requests

    fn handle_request(&mut self, request: HubRequest, now: Instant) -> bool {
        match request {
            HubRequest::Subscribe { service, owner, interval } => {
                let i = Core::index(service);
                let is_new = !self.slots[i].subs.contains_key(&owner);
                self.slots[i].subs.insert(owner, interval);
                self.settle(i, now);
                // A new reader of a running service gets the value it
                // missed (a reload re-subscribes within the stop delay).
                if is_new
                    && self.slots[i].running
                    && let Some(v) = self.slots[i].last_value.clone()
                    && matches!(self.slots[i].status, ServiceStatus::Ok | ServiceStatus::Stale)
                {
                    self.emit(i, Some(v));
                }
            }
            HubRequest::Unsubscribe { service, owner } => {
                let i = Core::index(service);
                self.slots[i].subs.remove(&owner);
                self.settle(i, now);
            }
            HubRequest::Command { command, .. } => self.command(command, now),
            HubRequest::Io { id, owner, request, grant } => self.io.request(id, owner, request, grant),
            HubRequest::Cancel { id } => self.io.cancel(id),
            HubRequest::CancelOwner { owner } => {
                for i in 0..self.slots.len() {
                    if self.slots[i].subs.remove(&owner).is_some() {
                        self.settle(i, now);
                    }
                }
                self.io.cancel_owner(owner);
            }
            HubRequest::Status { reply } => {
                let _ = reply.send(self.infos(now));
            }
            HubRequest::Shutdown => return false,
        }
        true
    }

    fn command(&mut self, command: ServiceCommand, now: Instant) {
        let id = command_service(&command);
        let i = Core::index(id);
        if !self.slots[i].running {
            self.start_service(i, now);
        }
        if self.slots[i].running {
            if self.slots[i].subs.is_empty() {
                self.slots[i].stop_at = Some(now + self.timing.stop_delay);
            }
            let result = self.slots[i].service.as_mut().expect("running").command(command);
            if let Err(e) = result {
                log::warn!("service {} command failed: {e}", id.name());
            }
        } else {
            log::warn!("service {} command dropped: the service could not start", id.name());
        }
    }

    fn infos(&self, now: Instant) -> Vec<ServiceInfo> {
        self.slots
            .iter()
            .map(|s| ServiceInfo {
                id: s.id,
                status: s.status,
                subscribers: s.subs.len() as u32,
                interval: s.interval,
                version: s.version,
                last_update_ago: s.running.then(|| now.saturating_duration_since(s.last_update)),
                error: s.error.clone(),
            })
            .collect()
    }

    // --------------------------------------------------- source reports

    fn handle_publish(
        &mut self,
        service: ServiceId,
        generation: u64,
        value: Value,
        status: Option<(ServiceStatus, Option<String>)>,
        now: Instant,
    ) {
        let i = Core::index(service);
        if !self.slots[i].running || self.slots[i].generation != generation {
            return;
        }
        if let Err(e) = check_value(service, &value) {
            log::error!("service {} produced a value that does not fit its schema: {e}", service.name());
            return;
        }
        self.slots[i].last_update = now;
        let (new_status, new_reason) = status.unwrap_or((ServiceStatus::Ok, None));
        let changed = self.slots[i].last_value.as_ref() != Some(&value);
        if !changed {
            // Same value: still alive; a status change is its own event.
            self.set_status(i, new_status, new_reason, now);
            return;
        }
        self.slots[i].last_value = Some(value.clone());
        let slot = &mut self.slots[i];
        let was = slot.status;
        if new_status != slot.status || new_reason != slot.error {
            // One event carries the value and the new status.
            let name = slot.id.name();
            match new_status {
                ServiceStatus::Unavailable => {
                    log::info!("service {name} unavailable: {}", new_reason.as_deref().unwrap_or("nothing to report"))
                }
                ServiceStatus::Error => {
                    log::warn!("service {name} error: {}", new_reason.as_deref().unwrap_or("unknown"))
                }
                ServiceStatus::Ok
                    if matches!(was, ServiceStatus::Stale | ServiceStatus::Error | ServiceStatus::Unavailable) =>
                {
                    log::info!("service {name} ok again")
                }
                _ => {}
            }
            slot.status = new_status;
            slot.error = new_reason;
            slot.stale_since = None;
        }
        self.emit(i, Some(value));
    }

    fn handle_alive(&mut self, service: ServiceId, generation: u64, now: Instant) {
        let i = Core::index(service);
        if !self.slots[i].running || self.slots[i].generation != generation {
            return;
        }
        self.slots[i].last_update = now;
        if self.slots[i].status == ServiceStatus::Stale {
            self.set_status(i, ServiceStatus::Ok, None, now);
        }
    }

    fn handle_status(
        &mut self,
        service: ServiceId,
        generation: u64,
        status: ServiceStatus,
        reason: Option<String>,
        now: Instant,
    ) {
        let i = Core::index(service);
        if !self.slots[i].running || self.slots[i].generation != generation {
            return;
        }
        self.slots[i].last_update = now;
        self.set_status(i, status, reason, now);
    }

    // ----------------------------------------------------------- timers

    fn deadline(&self) -> Option<Instant> {
        let mut best: Option<Instant> = None;
        let mut consider = |t: Option<Instant>| {
            if let Some(t) = t {
                best = Some(best.map_or(t, |b| b.min(t)));
            }
        };
        for s in &self.slots {
            consider(s.stop_at);
            consider(s.retry_at);
            if s.running {
                consider(s.tick_at);
                if matches!(s.status, ServiceStatus::Ok | ServiceStatus::Starting) {
                    consider(Some(s.last_update + s.stale_after));
                }
            }
        }
        best
    }

    fn timers(&mut self, now: Instant) {
        for i in 0..self.slots.len() {
            if self.slots[i].stop_at.is_some_and(|t| t <= now) && self.slots[i].subs.is_empty() {
                self.stop_service(i, now);
                continue;
            }
            if self.slots[i].retry_at.is_some_and(|t| t <= now) {
                if self.slots[i].subs.is_empty() {
                    self.slots[i].retry_at = None;
                } else {
                    self.start_service(i, now);
                }
            }
            if self.slots[i].running && self.slots[i].tick_at.is_some_and(|t| t <= now) {
                let slot = &mut self.slots[i];
                let svc = slot.service.as_mut().expect("running");
                svc.tick();
                slot.tick_at = svc.tick_after().map(|d| now + d);
            }
            let slot = &self.slots[i];
            if slot.running
                && matches!(slot.status, ServiceStatus::Ok | ServiceStatus::Starting)
                && now.saturating_duration_since(slot.last_update) >= slot.stale_after
            {
                self.set_status(i, ServiceStatus::Stale, None, now);
            }
        }
    }

    fn shutdown(&mut self) {
        let now = Instant::now();
        for i in 0..self.slots.len() {
            if self.slots[i].running || self.slots[i].status != ServiceStatus::Off {
                self.stop_service(i, now);
            }
        }
        self.io.shutdown();
    }
}

fn run(mut core: Core, rx: Receiver<HubMsg>, wakes: &AtomicU64) {
    loop {
        let msg = match core.deadline() {
            Some(t) => rx.recv_timeout(t.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        wakes.fetch_add(1, Ordering::Relaxed);
        let now = Instant::now();
        match msg {
            Ok(HubMsg::Request(r)) => {
                if !core.handle_request(r, now) {
                    break;
                }
            }
            Ok(HubMsg::Publish { service, generation, value, status }) => {
                core.handle_publish(service, generation, value, status, now)
            }
            Ok(HubMsg::Alive { service, generation }) => core.handle_alive(service, generation, now),
            Ok(HubMsg::Status { service, generation, status, reason }) => {
                core.handle_status(service, generation, status, reason, now)
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        core.timers(Instant::now());
    }
    core.shutdown();
}

//! Fakes for tests and for the binary's `--fake-services` flag.
//!
//! - `FakeWorld`: a `ServiceFactory` whose services publish whatever a test
//!   sets, record every command, and can be told to go silent (stale tests).
//! - `glazewm::FakeGlazeServer`: a GlazeWM-compatible WebSocket server on
//!   127.0.0.1 speaking the messages Astur's server speaks.
//! - `komorebi::FakeKomorebi`: connects to our subscribe pipe and writes
//!   komorebi-shaped notifications, like komorebi does.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rsb_core::services::{AudioCommand, MediaCommand, ServiceCommand, ServiceId, ServiceStatus, WorkspaceCommand};
use rsb_core::value::Value;

use crate::util::{map, text};
use crate::{HubOptions, Publisher, Service, ServiceFactory, UrlOpener};

pub mod glazewm;
pub mod komorebi;

struct FakeSlot {
    value: Option<Value>,
    status: Option<(ServiceStatus, Option<String>)>,
    silent: bool,
    publisher: Option<Publisher>,
    starts: u32,
    stops: u32,
    interval: Option<Duration>,
    fail_starts: u32,
    fail_reason: String,
    stale_after: Duration,
}

impl Default for FakeSlot {
    fn default() -> Self {
        FakeSlot {
            value: None,
            status: None,
            silent: false,
            publisher: None,
            starts: 0,
            stops: 0,
            interval: None,
            fail_starts: 0,
            fail_reason: String::new(),
            stale_after: Duration::from_secs(180),
        }
    }
}

#[derive(Default)]
struct WorldState {
    commands: Vec<ServiceCommand>,
    opened: Vec<String>,
    slots: HashMap<ServiceId, FakeSlot>,
}

/// Shared handle: clone it into the hub and keep one in the test.
#[derive(Clone, Default)]
pub struct FakeWorld {
    state: Arc<Mutex<WorldState>>,
}

impl FakeWorld {
    pub fn new() -> FakeWorld {
        FakeWorld::default()
    }

    fn lock(&self) -> MutexGuard<'_, WorldState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Sets a service's value; a running fake publishes it at once.
    pub fn set(&self, id: ServiceId, value: Value) {
        let mut st = self.lock();
        let slot = st.slots.entry(id).or_default();
        slot.value = Some(value);
        push(slot);
    }

    pub fn set_status(&self, id: ServiceId, status: ServiceStatus, reason: Option<String>) {
        let mut st = self.lock();
        let slot = st.slots.entry(id).or_default();
        slot.status = Some((status, reason.clone()));
        if let (Some(p), false) = (&slot.publisher, slot.silent) {
            match &slot.value {
                Some(v) => p.publish_as(v.clone(), status, reason),
                None => p.status(status, reason),
            }
        }
    }

    /// Stop sending values and liveness, so the hub marks it stale.
    pub fn go_silent(&self, id: ServiceId) {
        self.lock().slots.entry(id).or_default().silent = true;
    }

    /// Undoes `go_silent`: sends the current value and one liveness signal.
    pub fn resume(&self, id: ServiceId) {
        let mut st = self.lock();
        let slot = st.slots.entry(id).or_default();
        slot.silent = false;
        push(slot);
        if let Some(p) = &slot.publisher {
            p.alive();
        }
    }

    /// One liveness signal (a value that did not change).
    pub fn alive(&self, id: ServiceId) {
        if let Some(p) = self.lock().slots.entry(id).or_default().publisher.as_ref() {
            p.alive();
        }
    }

    /// The next `n` starts of this service fail with `reason`.
    pub fn fail_starts(&self, id: ServiceId, n: u32, reason: &str) {
        let mut st = self.lock();
        let slot = st.slots.entry(id).or_default();
        slot.fail_starts = n;
        slot.fail_reason = reason.to_string();
    }

    /// How long without a value or liveness before the hub calls it stale
    /// (default 180 s, as the real ones).
    pub fn set_stale_after(&self, id: ServiceId, after: Duration) {
        self.lock().slots.entry(id).or_default().stale_after = after;
    }

    /// Times the hub started this service (counting failed starts).
    pub fn starts(&self, id: ServiceId) -> u32 {
        self.lock().slots.get(&id).map_or(0, |s| s.starts)
    }

    /// Times the hub stopped a running service.
    pub fn stops(&self, id: ServiceId) -> u32 {
        self.lock().slots.get(&id).map_or(0, |s| s.stops)
    }

    /// The sampling period the hub last handed to this service.
    pub fn interval(&self, id: ServiceId) -> Option<Duration> {
        self.lock().slots.get(&id).and_then(|s| s.interval)
    }

    /// Addresses `rsb.open_url` asked to open, oldest first. Nothing is opened.
    pub fn opened_urls(&self) -> Vec<String> {
        self.lock().opened.clone()
    }

    /// Commands received so far, oldest first.
    pub fn commands(&self) -> Vec<ServiceCommand> {
        self.lock().commands.clone()
    }

    /// Plausible values for every service (used by `--fake-services` and
    /// the offscreen renders): 23 % cpu, 41 % memory, 87 % battery
    /// discharging, volume 35 unmuted, ethernet connected, a playing track,
    /// a window, workspaces 1-5 with 2 focused.
    pub fn with_sample_values() -> FakeWorld {
        let w = FakeWorld::new();
        w.set(
            ServiceId::System,
            map([
                ("cpu", Value::Num(23.0)),
                ("memory", Value::Num(41.0)),
                ("memory_used", Value::Int(6_900_000_000)),
                ("memory_total", Value::Int(16_900_000_000)),
            ]),
        );
        w.set(
            ServiceId::Battery,
            map([
                ("present", Value::Bool(true)),
                ("percent", Value::Num(87.0)),
                ("charging", Value::Bool(false)),
                ("plugged", Value::Bool(false)),
                ("saver", Value::Bool(false)),
                ("seconds_left", Value::Int(14_400)),
            ]),
        );
        w.set(
            ServiceId::Audio,
            map([("available", Value::Bool(true)), ("volume", Value::Num(35.0)), ("muted", Value::Bool(false))]),
        );
        w.set(
            ServiceId::Network,
            map([
                ("connected", Value::Bool(true)),
                ("internet", Value::Bool(true)),
                ("kind", text("ethernet")),
                ("ssid", Value::Nil),
                ("signal", Value::Nil),
                ("down_bps", Value::Int(250_000)),
                ("up_bps", Value::Int(40_000)),
            ]),
        );
        w.set(
            ServiceId::Media,
            map([
                ("available", Value::Bool(true)),
                ("playing", Value::Bool(true)),
                ("title", text("Sample Track")),
                ("artist", text("Sample Artist")),
                ("album", text("Sample Album")),
                ("source", text("sample.player")),
                ("can_play_pause", Value::Bool(true)),
                ("can_next", Value::Bool(true)),
                ("can_previous", Value::Bool(true)),
            ]),
        );
        w.set(
            ServiceId::Window,
            map([
                ("has_window", Value::Bool(true)),
                ("title", text("Sample document")),
                ("process", text("sample.exe")),
            ]),
        );
        w.set(ServiceId::Workspaces, sample_workspaces(1));
        w
    }
}

/// Workspaces 1..=5 on one monitor; `focused` is the 0-based focused one.
fn sample_workspaces(focused: usize) -> Value {
    let list = (0..5)
        .map(|i| {
            let name = (i + 1).to_string();
            map([
                ("name", text(&name)),
                ("display_name", text(&name)),
                ("focused", Value::Bool(i == focused)),
                ("visible", Value::Bool(i == focused)),
                ("has_windows", Value::Bool(i < 3)),
                ("window_count", Value::Int(if i < 3 { i as i64 + 1 } else { 0 })),
            ])
        })
        .collect();
    map([
        ("provider", text("glazewm")),
        ("connected", Value::Bool(true)),
        (
            "monitors",
            Value::List(vec![map([
                ("index", Value::Int(0)),
                ("name", text("Monitor 1")),
                ("handle", Value::Nil),
                ("device", Value::Nil),
                ("workspaces", Value::List(list)),
            ])]),
        ),
    ])
}

fn push(slot: &FakeSlot) {
    if slot.silent {
        return;
    }
    if let (Some(p), Some(v)) = (&slot.publisher, &slot.value) {
        match &slot.status {
            Some((s, r)) => p.publish_as(v.clone(), *s, r.clone()),
            None => p.publish(v.clone()),
        }
    }
}

impl ServiceFactory for FakeWorld {
    fn create(&self, id: ServiceId, _options: &HubOptions) -> Box<dyn Service> {
        Box::new(FakeService { id, world: self.clone() })
    }

    fn url_opener(&self) -> Option<Arc<dyn UrlOpener>> {
        Some(Arc::new(self.clone()))
    }
}

impl UrlOpener for FakeWorld {
    fn open(&self, url: &str) -> Result<(), String> {
        self.lock().opened.push(url.to_string());
        Ok(())
    }
}

struct FakeService {
    id: ServiceId,
    world: FakeWorld,
}

fn set_field(value: &mut Value, key: &str, new: Value) {
    if let Value::Map(m) = value {
        m.insert(key.to_string(), new);
    }
}

/// The fake behaves like the real thing for the commands it receives, so a
/// demo run with fake services reacts to the wheel and the clicks.
fn apply(slot: &mut FakeSlot, command: &ServiceCommand) {
    let Some(value) = slot.value.as_mut() else { return };
    match command {
        ServiceCommand::Audio(a) => {
            let volume = value.get("volume").and_then(Value::as_f64).unwrap_or(0.0);
            let muted = value.get("muted").and_then(Value::as_bool).unwrap_or(false);
            match a {
                AudioCommand::SetVolume(v) => set_field(value, "volume", Value::Num(f64::from(*v).clamp(0.0, 100.0))),
                AudioCommand::ChangeVolume(d) => {
                    set_field(value, "volume", Value::Num((volume + f64::from(*d)).clamp(0.0, 100.0)))
                }
                AudioCommand::SetMute(m) => set_field(value, "muted", Value::Bool(*m)),
                AudioCommand::ToggleMute => set_field(value, "muted", Value::Bool(!muted)),
            }
        }
        ServiceCommand::Media(m) => {
            let playing = value.get("playing").and_then(Value::as_bool).unwrap_or(false);
            match m {
                MediaCommand::PlayPause => set_field(value, "playing", Value::Bool(!playing)),
                MediaCommand::Play => set_field(value, "playing", Value::Bool(true)),
                MediaCommand::Pause => set_field(value, "playing", Value::Bool(false)),
                MediaCommand::Next | MediaCommand::Previous => {}
            }
        }
        ServiceCommand::Workspaces(WorkspaceCommand::Focus(name)) => {
            if let Some(index) = (0..5).find(|i| (i + 1).to_string() == *name) {
                *value = sample_workspaces(index);
            }
        }
        ServiceCommand::Workspaces(_) => {}
    }
}

impl Service for FakeService {
    fn id(&self) -> ServiceId {
        self.id
    }

    fn start(&mut self, out: Publisher) -> Result<(), String> {
        let mut st = self.world.lock();
        let slot = st.slots.entry(self.id).or_default();
        slot.starts += 1;
        if slot.fail_starts > 0 {
            slot.fail_starts -= 1;
            return Err(slot.fail_reason.clone());
        }
        slot.publisher = Some(out);
        push(slot);
        Ok(())
    }

    fn set_interval(&mut self, interval: Duration) {
        self.world.lock().slots.entry(self.id).or_default().interval = Some(interval);
    }

    fn command(&mut self, command: ServiceCommand) -> Result<(), String> {
        let mut st = self.world.lock();
        st.commands.push(command.clone());
        let slot = st.slots.entry(self.id).or_default();
        apply(slot, &command);
        push(slot);
        Ok(())
    }

    fn stop(&mut self) {
        let mut st = self.world.lock();
        let slot = st.slots.entry(self.id).or_default();
        if slot.publisher.take().is_some() {
            slot.stops += 1;
        }
    }

    fn stale_after(&self) -> Duration {
        self.world.lock().slots.get(&self.id).map_or(Duration::from_secs(180), |s| s.stale_after)
    }
}

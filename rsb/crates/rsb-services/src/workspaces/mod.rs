//! Workspaces from a running window manager: a GlazeWM-compatible WebSocket
//! (Astur, GlazeWM) when one answers, else komorebi's subscribe pipe. One
//! thread manages the providers; with none running the value says
//! `provider = "none"` and the status is `Unavailable` (not an error).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rsb_core::services::{ServiceCommand, ServiceId, ServiceStatus, WorkspaceCommand};

use crate::real::Worker;
use crate::win::{Owned, auto_event, signal};
use crate::{Publisher, Service, WorkspaceOptions};

pub(crate) mod glaze;
pub(crate) mod komorebi;
pub(crate) mod model;

pub use komorebi::KomorebiControl;

use glaze::End;
use model::{Model, Provider};

/// Disconnected from the window manager for this long = provider none.
const GRACE: Duration = Duration::from_secs(5);
const RECONNECT_MAX: Duration = Duration::from_secs(30);
/// Provider detection runs every `detect_every` for this long, then every 30 s.
const DETECT_FAST_FOR: Duration = Duration::from_secs(60);

/// What the connection threads and the command path share.
pub(crate) struct Shared {
    current: Mutex<Current>,
    pub(crate) glaze_cmds: Mutex<VecDeque<String>>,
    pub(crate) glaze_wake: Owned,
}

struct Current {
    provider: Provider,
    model: Model,
}

/// Remembers the model for computed commands and publishes it.
pub(crate) fn publish_model(shared: &Arc<Shared>, out: &Publisher, model: Model, status: ServiceStatus) {
    let value = model.to_value();
    {
        let mut c = shared.current.lock().unwrap_or_else(|e| e.into_inner());
        c.provider = model.provider;
        c.model = model;
    }
    out.publish_as(value, status, None);
}

fn publish_none(shared: &Arc<Shared>, out: &Publisher, status: ServiceStatus, reason: &str) {
    let model = Model::none();
    let value = model.to_value();
    {
        let mut c = shared.current.lock().unwrap_or_else(|e| e.into_inner());
        c.provider = Provider::None;
        c.model = model;
    }
    out.publish_as(value, status, Some(reason.to_string()));
}

fn wait_for(stop: &Owned, until: Instant) -> bool {
    use windows::Win32::Foundation::WAIT_OBJECT_0;
    use windows::Win32::System::Threading::WaitForSingleObject;
    let ms = until.saturating_duration_since(Instant::now()).as_millis().min(u128::from(u32::MAX - 1)) as u32;
    crate::count_source_wake();
    // SAFETY: `stop` is a live event.
    unsafe { WaitForSingleObject(stop.get(), ms) == WAIT_OBJECT_0 }
}

fn manage(
    shared: Arc<Shared>,
    out: Publisher,
    stop: Arc<Owned>,
    opts: WorkspaceOptions,
    control: Arc<dyn KomorebiControl>,
) {
    let port = match glaze::parse_url(&opts.glazewm_url) {
        Ok(p) => Some(p),
        Err(e) => {
            log::warn!("workspaces: {e}");
            None
        }
    };
    let started = Instant::now();
    let now = Instant::now();
    let (mut glaze_next, mut glaze_delay) = (now, Duration::from_secs(1));
    let (mut komo_next, mut komo_delay) = (now, Duration::from_secs(1));
    let mut lost_since: Option<Instant> = None;
    let mut published_none = false;
    let mut refused: Option<String> = None;

    loop {
        let now = Instant::now();
        // The GlazeWM-compatible server first.
        if let Some(port) = port.filter(|_| now >= glaze_next) {
            let began = Instant::now();
            match glaze::run_session(&shared, &out, &stop, port) {
                End::Stopped => return,
                End::NotRunning => {
                    refused = None;
                    glaze_next = Instant::now() + glaze_delay;
                    glaze_delay = (glaze_delay * 2).min(RECONNECT_MAX);
                }
                End::Refused(reason) => {
                    log::warn!("workspaces: {reason}");
                    refused = Some(reason);
                    glaze_next = Instant::now() + RECONNECT_MAX;
                }
                End::Lost(why) => {
                    let lasted = began.elapsed();
                    refused = None;
                    log::info!(
                        "workspaces: connection to the window manager ended after {} s ({why})",
                        lasted.as_secs()
                    );
                    if lasted > GRACE {
                        // It was a real connection: start losing it now, retry quickly.
                        lost_since = Some(Instant::now());
                        glaze_delay = Duration::from_secs(1);
                        published_none = false;
                    }
                    glaze_next = Instant::now() + glaze_delay;
                    glaze_delay = (glaze_delay * 2).min(RECONNECT_MAX);
                }
            }
            continue;
        }
        // Then komorebi.
        if Instant::now() >= komo_next {
            if control.running() {
                let began = Instant::now();
                match komorebi::run_session(&shared, &out, &stop, &control, &opts.komorebi_pipe) {
                    End::Stopped => return,
                    End::Lost(why) | End::Refused(why) => {
                        let lasted = began.elapsed();
                        log::info!("workspaces: the komorebi connection ended after {} s ({why})", lasted.as_secs());
                        if lasted > GRACE {
                            lost_since = Some(Instant::now());
                            komo_delay = Duration::from_secs(1);
                            published_none = false;
                        }
                        komo_next = Instant::now() + komo_delay;
                        komo_delay = (komo_delay * 2).min(RECONNECT_MAX);
                    }
                    End::NotRunning => komo_next = Instant::now() + opts.detect_every,
                }
                continue;
            }
            let every = if started.elapsed() < DETECT_FAST_FOR {
                opts.detect_every
            } else {
                opts.detect_every.max(Duration::from_secs(30))
            };
            komo_next = Instant::now() + every;
        }

        // Neither is connected.
        let now = Instant::now();
        let grace_end = lost_since.map(|t| t + GRACE);
        if !published_none && grace_end.is_none_or(|t| now >= t) {
            match &refused {
                Some(reason) => publish_none(&shared, &out, ServiceStatus::Error, reason),
                None => publish_none(&shared, &out, ServiceStatus::Unavailable, "no window manager found"),
            }
            published_none = true;
            lost_since = None;
        }
        let mut next = komo_next;
        if port.is_some() {
            next = next.min(glaze_next);
        }
        if let Some(t) = grace_end.filter(|_| !published_none) {
            next = next.min(t);
        }
        if wait_for(&stop, next) {
            return;
        }
    }
}

pub(crate) struct WorkspacesService {
    options: WorkspaceOptions,
    control: Arc<dyn KomorebiControl>,
    shared: Option<Arc<Shared>>,
    worker: Option<Worker>,
}

impl WorkspacesService {
    pub(crate) fn new(options: WorkspaceOptions) -> WorkspacesService {
        let control = Arc::new(komorebi::RealControl { komorebic: options.komorebic.clone() });
        WorkspacesService::with_control(options, control)
    }

    pub(crate) fn with_control(options: WorkspaceOptions, control: Arc<dyn KomorebiControl>) -> WorkspacesService {
        WorkspacesService { options, control, shared: None, worker: None }
    }
}

/// The workspaces service with komorebi's side replaced: tests pass a fake
/// for `komorebic` and the process list; None uses the real ones.
pub fn workspaces_service(options: WorkspaceOptions, control: Option<Arc<dyn KomorebiControl>>) -> Box<dyn Service> {
    match control {
        Some(c) => Box::new(WorkspacesService::with_control(options, c)),
        None => Box::new(WorkspacesService::new(options)),
    }
}

impl Service for WorkspacesService {
    fn id(&self) -> ServiceId {
        ServiceId::Workspaces
    }

    fn start(&mut self, out: Publisher) -> Result<(), String> {
        let shared = Arc::new(Shared {
            current: Mutex::new(Current { provider: Provider::None, model: Model::none() }),
            glaze_cmds: Mutex::new(VecDeque::new()),
            glaze_wake: auto_event()?,
        });
        let (s, opts, control) = (shared.clone(), self.options.clone(), self.control.clone());
        self.worker = Some(Worker::spawn("rsb-workspaces", move |stop| manage(s, out, stop, opts, control))?);
        self.shared = Some(shared);
        Ok(())
    }

    fn command(&mut self, command: ServiceCommand) -> Result<(), String> {
        let ServiceCommand::Workspaces(c) = command else { return Err("not a workspaces command".into()) };
        let shared = self.shared.as_ref().ok_or("the service is stopped")?;
        let (provider, model) = {
            let cur = shared.current.lock().unwrap_or_else(|e| e.into_inner());
            (cur.provider, cur.model.clone())
        };
        let target = match c {
            WorkspaceCommand::Focus(name) => Some(name),
            WorkspaceCommand::FocusNext(m) => model.cycle_target(m, 1),
            WorkspaceCommand::FocusPrevious(m) => model.cycle_target(m, -1),
        }
        .ok_or("there is no workspace to switch to")?;
        match provider {
            Provider::None => Err("no window manager is connected".into()),
            Provider::Glaze => {
                if !glaze::command_name_ok(&target) {
                    return Err("the workspace name has spaces or control characters".into());
                }
                shared
                    .glaze_cmds
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push_back(format!("command focus --workspace {target}"));
                signal(&shared.glaze_wake);
                Ok(())
            }
            Provider::Komorebi => {
                // komorebic takes a moment: its own short-lived thread.
                let control = self.control.clone();
                std::thread::Builder::new()
                    .name("rsb-komorebic".into())
                    .spawn(move || {
                        if let Err(e) = control.focus_named(&target) {
                            log::warn!("workspaces: switching workspace failed ({e})");
                        }
                    })
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
        }
    }

    fn stop(&mut self) {
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
        self.shared = None;
    }

    /// Connection based: the ping every minute answers it.
    fn stale_after(&self) -> Duration {
        Duration::from_secs(180)
    }
}

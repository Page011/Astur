//! The services hub and the script thread, wired to each other. Used by the
//! running bar and by `rsb render`.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rsb_core::msg::{Heartbeat, ScriptIn, ScriptSink};
use rsb_core::time::ThreadTime;
use rsb_script::{Limits, ScriptHost, ScriptOptions};
use rsb_services::clock::{SystemClock, ThreadCycleTime};
use rsb_services::fake::FakeWorld;
use rsb_services::{Hub, HubOptions, HubSender, ServiceFactory, WorkspaceOptions};

use crate::assets::Store;

pub struct EngineOptions {
    pub config_dir: PathBuf,
    pub dark: bool,
    /// Fake services instead of the real ones.
    pub world: Option<FakeWorld>,
    /// The workspaces service connects here instead of Astur's port.
    pub glaze_url: Option<String>,
    /// The widget whose VM gets the hidden `__test_stall`.
    pub test_stall: Option<String>,
}

pub struct Engine {
    pub hub: Hub,
    pub hub_port: HubSender,
    pub script: ScriptHost,
    pub script_tx: Sender<ScriptIn>,
    pub heartbeat: Arc<Heartbeat>,
}

/// The script thread's CPU clock: calibrated on first use, which happens on
/// the script thread (about 20 ms), so the UI thread never pays for it.
struct LazyCycleTime(OnceLock<ThreadCycleTime>);

impl ThreadTime for LazyCycleTime {
    fn now(&self) -> Duration {
        self.0.get_or_init(ThreadCycleTime::new).now()
    }
}

/// Starts the hub, then the script thread; hub events reach the script
/// thread through `ScriptIn::Hub`, and the script thread's requests reach
/// the hub through its port.
pub fn start(options: EngineOptions, out: Box<dyn ScriptSink>) -> Engine {
    let slot: Arc<OnceLock<Sender<ScriptIn>>> = Arc::new(OnceLock::new());
    let sink_slot = Arc::clone(&slot);
    let factory: Box<dyn ServiceFactory> = match options.world {
        Some(world) => Box::new(world),
        None => Hub::real_factory(),
    };
    let mut hub_options = HubOptions::default();
    if let Some(url) = options.glaze_url {
        hub_options.workspaces = WorkspaceOptions { glazewm_url: url, ..WorkspaceOptions::default() };
    }
    let hub = Hub::start(
        hub_options,
        factory,
        Box::new(move |event| {
            if let Some(tx) = sink_slot.get() {
                let _ = tx.send(ScriptIn::Hub(event));
            }
        }),
    );
    let hub_port = hub.port();
    let heartbeat = Arc::new(Heartbeat::new());
    rsb_script::set_test_stall_widget(options.test_stall.as_deref());
    let (script, script_tx) = ScriptHost::start(
        ScriptOptions {
            sources: Arc::new(Store { config_dir: options.config_dir.clone() }),
            clock: Arc::new(SystemClock),
            config_dir: options.config_dir,
            path_env: crate::paths::path_env(),
            limits: Limits::default(),
            dark: options.dark,
            thread_time: Some(Arc::new(LazyCycleTime(OnceLock::new()))),
            heartbeat: Arc::clone(&heartbeat),
        },
        Box::new(hub_port.clone()),
        out,
    );
    let _ = slot.set(script_tx.clone());
    Engine { hub, hub_port, script, script_tx, heartbeat }
}

impl Engine {
    /// Stop order, steps 3 and 4: the script thread (1 s), then the hub (1 s).
    pub fn stop(self) {
        self.script.join(Duration::from_secs(1));
        self.hub.shutdown(Duration::from_secs(1));
    }
}

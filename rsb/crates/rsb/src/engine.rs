//! The services hub and the script thread, wired to each other. Used by the
//! running bar and by `rsb render`.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rsb_core::msg::{Heartbeat, HubEvent, ScriptIn, ScriptSink};
use rsb_core::services::ServiceEvent;
use rsb_core::time::ThreadTime;
use rsb_core::wake::Wake;
use rsb_script::{Limits, ScriptHost, ScriptOptions};
use rsb_services::clock::{SystemClock, ThreadCycleTime};
use rsb_services::fake::FakeWorld;
use rsb_services::{Hub, HubOptions, HubSender, ServiceFactory, WorkspaceOptions};

use crate::assets::Store;
use crate::gate::Gate;
use crate::msg::UiMsg;
use crate::service_images::{ArtWatch, apps_stopped, power_failed};

/// Where service pictures go: the UI thread's inbox, then a wake.
pub struct ImageRoute {
    pub tx: Sender<UiMsg>,
    pub wake: Arc<dyn Wake>,
}

impl ImageRoute {
    fn send(&self, msg: UiMsg) {
        if self.tx.send(msg).is_ok() {
            self.wake.wake();
        }
    }
}

pub struct EngineOptions {
    pub config_dir: PathBuf,
    pub dark: bool,
    /// Fake services instead of the real ones.
    pub world: Option<FakeWorld>,
    /// The workspaces service connects here instead of Astur's port.
    pub glaze_url: Option<String>,
    /// The widget whose VM gets the hidden `__test_stall`.
    pub test_stall: Option<String>,
    /// The script thread's clock; None = the system clock.
    pub clock: Option<Arc<dyn rsb_core::time::Clock>>,
    /// The command recorder and the power stub; None = a plain run, whose
    /// commands reach the services and whose power actions are Windows'.
    pub gate: Option<Gate>,
    /// Service pictures go here; None = they are dropped (a headless run).
    pub images: Option<ImageRoute>,
    /// Sees every service event on its way to the script thread (a timeline run keeps the last values).
    pub observe: Option<Observer>,
}

/// Called on the hub's thread for each service event.
pub type Observer = Arc<dyn Fn(&ServiceEvent) + Send + Sync>;

/// The hub's options for a run.
fn hub_options(glaze_url: Option<String>, gate: Option<Gate>) -> HubOptions {
    let mut options = HubOptions::default();
    if let Some(url) = glaze_url {
        options.workspaces = WorkspaceOptions { glazewm_url: url, ..WorkspaceOptions::default() };
    }
    if let Some(gate) = gate {
        options.command_gate = Some(gate.commands);
        options.power_actions = Some(gate.power);
    }
    options
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
    let factory: Box<dyn ServiceFactory> = match &options.world {
        Some(world) => Box::new(world.clone()),
        None => Hub::real_factory(),
    };
    start_with(options, factory, out)
}

/// `start` with the services' factory chosen by the caller.
pub fn start_with(options: EngineOptions, factory: Box<dyn ServiceFactory>, out: Box<dyn ScriptSink>) -> Engine {
    let slot: Arc<OnceLock<Sender<ScriptIn>>> = Arc::new(OnceLock::new());
    let sink_slot = Arc::clone(&slot);
    let hub_options = hub_options(options.glaze_url, options.gate);
    let (route, art, observe) = (options.images, Mutex::new(ArtWatch::default()), options.observe);
    let hub = Hub::start(
        hub_options,
        factory,
        Box::new(move |event| match event {
            // Pictures belong to the renderer: the script thread never sees pixels.
            HubEvent::Image { key, image } => {
                if let Some(route) = &route {
                    route.send(UiMsg::ServiceImage { key, image });
                }
            }
            other => {
                if let (HubEvent::Service(ev), Some(observe)) = (&other, &observe) {
                    observe(ev);
                }
                if let (HubEvent::Service(ev), Some(route)) = (&other, &route) {
                    let dropped = art.lock().unwrap_or_else(|e| e.into_inner()).observe(ev);
                    if let Some(key) = dropped {
                        route.send(UiMsg::DropServiceImage { key });
                    }
                    // Icons held for a list nobody reads are dead weight.
                    if apps_stopped(ev) {
                        route.send(UiMsg::DropAppImages);
                    }
                    // A lock that fails must not leave the overlay's curtain up.
                    if power_failed(ev) {
                        route.send(UiMsg::PowerError);
                    }
                }
                if let Some(tx) = sink_slot.get() {
                    let _ = tx.send(ScriptIn::Hub(other));
                }
            }
        }),
    );
    let hub_port = hub.port();
    let heartbeat = Arc::new(Heartbeat::new());
    rsb_script::set_test_stall_widget(options.test_stall.as_deref());
    let (script, script_tx) = ScriptHost::start(
        ScriptOptions {
            sources: Arc::new(Store { config_dir: options.config_dir.clone() }),
            clock: options.clock.clone().unwrap_or_else(|| Arc::new(SystemClock)),
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::mpsc::{Receiver, channel};
    use std::time::Instant;

    use rsb_core::element::InstanceId;
    use rsb_core::msg::{HubPort, HubRequest, ScriptOut};
    use rsb_core::pixels::RgbaImage;
    use rsb_core::services::{AudioCommand, ServiceCommand, ServiceId};
    use rsb_core::value::Value;

    use super::*;
    use crate::gate::Selection;
    use crate::stats::Stats;
    use crate::testutil::TempDir;

    struct NullSink;

    impl ScriptSink for NullSink {
        fn send(&self, _: ScriptOut) {}
    }

    fn options(world: &FakeWorld, gate: Option<Gate>, images: Option<ImageRoute>) -> EngineOptions {
        EngineOptions {
            config_dir: std::env::temp_dir(),
            dark: true,
            world: Some(world.clone()),
            glaze_url: None,
            test_stall: None,
            clock: None,
            gate,
            images,
            observe: None,
        }
    }

    fn wait(what: &str, mut done: impl FnMut() -> bool) {
        let until = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < until, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn command(engine: &Engine, c: ServiceCommand) {
        engine.hub_port.send(HubRequest::Command { owner: InstanceId(1), command: c });
    }

    fn command_lines(file: &std::path::Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(file)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|v| v["event"] == "command")
            .collect()
    }

    /// Two commands through a run with a gate: returns the stats lines, the
    /// times the fake audio service was started and the commands it took.
    fn gated_run(forward: bool) -> (Vec<serde_json::Value>, u32, usize) {
        let dir = TempDir::new("engine-gate");
        let file = dir.path().join("s.jsonl");
        let stats = Arc::new(Stats::open(Some(&file), false, Instant::now()).unwrap());
        let world = FakeWorld::with_sample_values();
        let engine = start(options(&world, Some(Gate::new(Selection { forward }, stats)), None), Box::new(NullSink));
        command(&engine, ServiceCommand::Audio(AudioCommand::ChangeVolume(0.0)));
        command(&engine, ServiceCommand::Audio(AudioCommand::ToggleMute));
        wait("both commands to pass the gate", || command_lines(&file).len() == 2);
        if forward {
            wait("the fake to take them", || world.commands().len() == 2);
        }
        let out = (command_lines(&file), world.starts(ServiceId::Audio), world.commands().len());
        engine.stop();
        out
    }

    #[test]
    fn with_the_recorder_and_no_forwarding_a_command_stops_before_its_service_starts() {
        let (lines, starts, taken) = gated_run(false);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l["forwarded"] == false && l["service"] == "audio"), "{lines:?}");
        assert_eq!(
            (lines[0]["kind"].as_str(), lines[1]["kind"].as_str()),
            (Some("change_volume"), Some("toggle_mute"))
        );
        assert_eq!((starts, taken), (0, 0), "no service was even created for them");
    }

    #[test]
    fn with_the_recorder_and_the_fakes_a_command_reaches_the_fake_and_is_still_recorded() {
        let (lines, starts, taken) = gated_run(true);
        assert!(lines.iter().all(|l| l["forwarded"] == true), "{lines:?}");
        assert_eq!((starts, taken), (1, 2));
    }

    #[test]
    fn a_plain_run_has_no_gate_so_its_commands_reach_the_services() {
        let hub = hub_options(None, None);
        assert!(hub.command_gate.is_none() && hub.power_actions.is_none(), "{hub:?}");
        let world = FakeWorld::with_sample_values();
        let engine = start(options(&world, None, None), Box::new(NullSink));
        command(&engine, ServiceCommand::Audio(AudioCommand::ToggleMute));
        wait("the service to take the command", || world.commands().len() == 1);
        assert_eq!(world.starts(ServiceId::Audio), 1);
        engine.stop();
    }

    #[test]
    fn a_gate_sets_both_hub_seams() {
        let gate = Gate::new(Selection { forward: false }, Arc::new(Stats::off()));
        let options = hub_options(Some("ws://127.0.0.1:1".into()), Some(gate));
        assert!(options.command_gate.is_some() && options.power_actions.is_some());
        assert_eq!(options.workspaces.glazewm_url, "ws://127.0.0.1:1");
    }

    fn media_value(art: Option<&str>) -> Value {
        let mut m = BTreeMap::new();
        for (k, v) in [("available", true), ("playing", true), ("can_play_pause", true), ("can_next", true)] {
            m.insert(k.to_string(), Value::Bool(v));
        }
        m.insert("can_previous".into(), Value::Bool(true));
        for k in ["title", "artist", "album", "source"] {
            m.insert(k.to_string(), Value::Str(String::new()));
        }
        if let Some(a) = art {
            m.insert("art".into(), Value::Str(a.to_string()));
        }
        Value::Map(m)
    }

    fn drain(rx: &Receiver<UiMsg>) -> Vec<UiMsg> {
        rx.try_iter().collect()
    }

    #[test]
    fn service_pictures_go_to_the_ui_inbox_and_the_old_art_key_is_dropped_when_the_art_changes() {
        let world = FakeWorld::with_sample_values();
        let (tx, rx) = channel();
        let route = ImageRoute { tx, wake: Arc::new(rsb_core::wake::NoWake) };
        let engine = start(options(&world, None, Some(route)), Box::new(NullSink));
        engine.hub_port.send(HubRequest::Subscribe { service: ServiceId::Media, owner: InstanceId(1), interval: None });
        wait("the media service to start", || world.starts(ServiceId::Media) == 1);

        let art = |n: u32| format!("rsb-image:media/{n}");
        let mut seen: Vec<UiMsg> = Vec::new();
        for n in 1..=2 {
            let pixel = RgbaImage::new(1, 1, vec![9, 9, 9, 255]).unwrap();
            world.publish_image(ServiceId::Media, &art(n), pixel);
            world.set(ServiceId::Media, media_value(Some(&art(n))));
            let mut have_image = false;
            wait("the picture to reach the inbox", || {
                seen.extend(drain(&rx));
                have_image = seen.iter().any(|m| matches!(m, UiMsg::ServiceImage { key, .. } if **key == art(n)));
                have_image
            });
        }
        wait("the first key to be dropped", || {
            seen.extend(drain(&rx));
            seen.iter().any(|m| matches!(m, UiMsg::DropServiceImage { key } if **key == art(1)))
        });
        let drops = seen.iter().filter(|m| matches!(m, UiMsg::DropServiceImage { .. })).count();
        assert_eq!(drops, 1, "only the old key goes, once");
        engine.stop();
    }

    #[test]
    fn when_the_last_widget_leaves_the_app_list_the_icons_are_dropped_from_the_renderer() {
        let world = FakeWorld::with_sample_values();
        let (tx, rx) = channel();
        let route = ImageRoute { tx, wake: Arc::new(rsb_core::wake::NoWake) };
        let engine = start(options(&world, None, Some(route)), Box::new(NullSink));
        engine.hub_port.send(HubRequest::Subscribe { service: ServiceId::Apps, owner: InstanceId(1), interval: None });
        wait("the apps service to start", || world.starts(ServiceId::Apps) == 1);
        std::thread::sleep(Duration::from_millis(100));
        assert!(!drain(&rx).iter().any(|m| matches!(m, UiMsg::DropAppImages)), "nothing is dropped while it runs");
        engine.hub_port.send(HubRequest::Unsubscribe { service: ServiceId::Apps, owner: InstanceId(1) });
        let mut seen: Vec<UiMsg> = Vec::new();
        wait("the icons to be dropped once the service stopped", || {
            seen.extend(drain(&rx));
            seen.iter().any(|m| matches!(m, UiMsg::DropAppImages))
        });
        engine.stop();
    }
}

//! A slow power action never holds the hub: every service's commands run on the one hub thread, so
//! a sleep that returns only when the PC wakes must not make another service's command wait. The
//! actions here are recorders; nothing touches the session.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::*;
use rsb_core::element::InstanceId;
use rsb_core::msg::{HubEvent, HubPort, HubRequest};
use rsb_core::services::{AudioCommand, PowerCommand, ServiceCommand, ServiceId, ServiceStatus};
use rsb_services::fake::FakeWorld;
use rsb_services::{Hub, HubOptions, PowerActions, RealFactory, Service, ServiceFactory};

/// Takes `delay` to answer every action, then answers `result`.
#[derive(Debug)]
struct Slow {
    delay: Duration,
    result: Result<(), String>,
    entered: AtomicBool,
}

impl Slow {
    fn new(delay: Duration, result: Result<(), String>) -> Arc<Slow> {
        Arc::new(Slow { delay, result, entered: AtomicBool::new(false) })
    }

    fn act(&self) -> Result<(), String> {
        self.entered.store(true, Ordering::Release);
        std::thread::sleep(self.delay);
        self.result.clone()
    }
}

impl PowerActions for Slow {
    fn lock(&self) -> Result<(), String> {
        self.act()
    }
    fn sleep(&self) -> Result<(), String> {
        self.act()
    }
    fn restart(&self) -> Result<(), String> {
        self.act()
    }
    fn shut_down(&self) -> Result<(), String> {
        self.act()
    }
    fn sign_out(&self) -> Result<(), String> {
        self.act()
    }
}

/// The real power service, every other service a fake.
struct Mixed(FakeWorld);

impl ServiceFactory for Mixed {
    fn create(&self, id: ServiceId, options: &HubOptions) -> Box<dyn Service> {
        if id == ServiceId::Power { RealFactory.create(id, options) } else { self.0.create(id, options) }
    }
}

fn mixed_hub(world: &FakeWorld, actions: Arc<Slow>) -> (Hub, Collector) {
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let options = HubOptions { power_actions: Some(actions), ..HubOptions::default() };
    let hub = Hub::start_with(
        options,
        Box::new(Mixed(world.clone())),
        Box::new(move |e| {
            let _ = tx.lock().unwrap().send(e);
        }),
        fast_timing(),
    );
    (hub, Collector { rx, seen: std::sync::Mutex::new(Vec::new()) })
}

fn send(hub: &Hub, command: ServiceCommand) {
    hub.port().send(HubRequest::Command { owner: InstanceId(1), command });
}

#[test]
fn a_command_to_another_service_is_handled_in_under_20_ms_while_a_power_action_runs() {
    let world = FakeWorld::with_sample_values();
    let actions = Slow::new(Duration::from_millis(900), Ok(()));
    let (hub, col) = mixed_hub(&world, actions.clone());
    subscribe(&hub, ServiceId::Power, 1, None);
    subscribe(&hub, ServiceId::Audio, 1, None);
    col.value(ServiceId::Power, WAIT).expect("the power service starts");
    assert!(eventually(WAIT, || world.starts(ServiceId::Audio) == 1), "the audio fake starts");

    let mut times = Vec::new();
    for trial in 0..5usize {
        actions.entered.store(false, Ordering::Release);
        send(&hub, ServiceCommand::Power(PowerCommand::Sleep));
        assert!(eventually(WAIT, || actions.entered.load(Ordering::Acquire)), "the action started");
        let before = world.commands().len();
        let t = Instant::now();
        send(&hub, ServiceCommand::Audio(AudioCommand::ToggleMute));
        let handled = loop {
            if world.commands().len() > before {
                break t.elapsed();
            }
            assert!(t.elapsed() < WAIT, "trial {trial}: the audio command was never handled");
            std::thread::sleep(Duration::from_millis(1));
        };
        times.push(handled);
        // The next trial starts only when this action is over, so two never overlap.
        std::thread::sleep(Duration::from_millis(950));
    }
    times.sort();
    println!("audio command handled during a 900 ms power action, 5 trials ({}): {times:?}", session_name());
    // The old wait held the hub for 200 ms in every trial.
    assert!(times[2] < Duration::from_millis(20), "median {:?}, all {times:?}", times[2]);
    hub.shutdown(WAIT);
}

#[test]
fn a_failure_that_comes_late_is_an_error_status_and_the_command_returned_at_once() {
    let world = FakeWorld::with_sample_values();
    let actions = Slow::new(Duration::from_millis(400), Err("Windows refused the request".into()));
    let (hub, col) = mixed_hub(&world, actions.clone());
    subscribe(&hub, ServiceId::Power, 1, None);
    col.value(ServiceId::Power, WAIT).expect("the power service starts");

    let t = Instant::now();
    send(&hub, ServiceCommand::Power(PowerCommand::Lock));
    let error = col
        .wait(WAIT, |e| match e {
            HubEvent::Service(s) if s.service == ServiceId::Power && s.status == ServiceStatus::Error => {
                Some((t.elapsed(), s.error.clone()))
            }
            _ => None,
        })
        .expect("the failure is reported");
    println!("late power failure reported after {:?} ({})", error.0, session_name());
    assert_eq!(error.1.as_deref(), Some("Windows refused the request"));
    assert!(error.0 >= Duration::from_millis(400), "reported when the action failed, not before: {:?}", error.0);

    // A success reports nothing.
    let ok = Slow::new(Duration::from_millis(50), Ok(()));
    let (hub2, col2) = mixed_hub(&world, ok.clone());
    subscribe(&hub2, ServiceId::Power, 1, None);
    col2.value(ServiceId::Power, WAIT).unwrap();
    send(&hub2, ServiceCommand::Power(PowerCommand::Sleep));
    assert!(eventually(WAIT, || ok.entered.load(Ordering::Acquire)));
    let after = col2.service_events(ServiceId::Power, Duration::from_millis(400));
    assert!(after.iter().all(|e| e.status != ServiceStatus::Error), "{after:?}");
    hub.shutdown(WAIT);
    hub2.shutdown(WAIT);
}

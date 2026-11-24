//! Idle cost of each source: how often its thread wakes while nothing
//! changes. This file has a single test so no other test's threads add to
//! the process-wide wake counter.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use rsb_core::services::ServiceId;
use rsb_services::fake::glazewm::{FakeGlazeServer, FakeWorkspace};
use rsb_services::fake::komorebi::{FakeKomorebi, FakeKomorebiControl};
use rsb_services::{
    Hub, HubOptions, HubTiming, Service, ServiceFactory, WorkspaceOptions, source_wakes, workspaces_service,
};

const WINDOW: Duration = Duration::from_secs(3);

struct Mixed {
    options: WorkspaceOptions,
    control: Arc<FakeKomorebiControl>,
}

impl ServiceFactory for Mixed {
    fn create(&self, id: ServiceId, options: &HubOptions) -> Box<dyn Service> {
        match id {
            ServiceId::Workspaces => workspaces_service(self.options.clone(), Some(self.control.clone())),
            other => rsb_services::RealFactory.create(other, options),
        }
    }
}

fn measure(hub: &Hub, col: &Collector, id: ServiceId, interval: Option<Duration>) -> (u64, u64) {
    subscribe(hub, id, 1, interval);
    assert!(col.value(id, Duration::from_secs(10)).is_some(), "{} gave a value", id.name());
    std::thread::sleep(Duration::from_millis(700));
    let (w0, h0) = (source_wakes(), hub.wakes());
    std::thread::sleep(WINDOW);
    let (w1, h1) = (source_wakes() - w0, hub.wakes() - h0);
    unsubscribe(hub, id, 1);
    std::thread::sleep(Duration::from_millis(2600));
    (w1, h1)
}

#[test]
fn idle_sources_do_not_poll() {
    let server = FakeGlazeServer::start().unwrap();
    server.set_state(vec![vec![FakeWorkspace { name: "1".into(), display_name: "".into(), windows: 1 }]], (0, 0));
    let control = Arc::new(FakeKomorebiControl::new(false));
    let options = WorkspaceOptions {
        glazewm_url: server.url(),
        komorebic: "komorebic".into(),
        komorebi_pipe: format!("rsb-test-komorebi-{}-wakes", std::process::id()),
        detect_every: Duration::from_secs(5),
    };
    let (hub, col) =
        hub_with(Box::new(Mixed { options: options.clone(), control: control.clone() }), HubTiming::default());

    let mut report = Vec::new();
    // A connected GlazeWM-compatible connection: no poll loop, one wake a minute plus the events.
    let (wakes, hub_wakes) = measure(&hub, &col, ServiceId::Workspaces, None);
    println!("wakes in {} s idle: workspaces (glazewm connected) sources={wakes} hub={hub_wakes}", WINDOW.as_secs());
    assert!(wakes <= 2, "an idle connection waits on its events: {wakes} wakes in {} s", WINDOW.as_secs());
    report.push(("workspaces/glazewm", wakes));
    drop(server);

    // komorebi connected through its pipe.
    let control2 = Arc::new(FakeKomorebiControl::new(true));
    let options2 = WorkspaceOptions {
        glazewm_url: "ws://127.0.0.1:1".into(),
        komorebi_pipe: format!("rsb-test-komorebi-{}-wakes2", std::process::id()),
        ..options
    };
    hub.shutdown(WAIT);
    let (hub, col) =
        hub_with(Box::new(Mixed { options: options2.clone(), control: control2.clone() }), HubTiming::default());
    subscribe(&hub, ServiceId::Workspaces, 1, None);
    assert!(eventually(Duration::from_secs(10), || !control2.subscribed().is_empty()));
    let mut komorebi = FakeKomorebi::connect(&control2.subscribed()[0]).unwrap();
    komorebi.send_state(&[vec![("a", 1)]], (0, 0)).unwrap();
    assert!(col.value(ServiceId::Workspaces, Duration::from_secs(10)).is_some());
    std::thread::sleep(Duration::from_millis(700));
    let w0 = source_wakes();
    std::thread::sleep(WINDOW);
    let wakes = source_wakes() - w0;
    println!("wakes in {} s idle: workspaces (komorebi pipe connected) sources={wakes}", WINDOW.as_secs());
    assert!(wakes <= 2, "{wakes}");
    report.push(("workspaces/komorebi", wakes));
    drop(komorebi);
    hub.shutdown(WAIT);

    // The real sources.
    let (hub, col) = hub_with(Box::new(rsb_services::RealFactory), HubTiming::default());
    for (id, interval) in [
        (ServiceId::Battery, None),
        (ServiceId::Audio, None),
        (ServiceId::Media, None),
        (ServiceId::Window, None),
        (ServiceId::Network, Some(Duration::from_secs(2))),
        (ServiceId::System, Some(Duration::from_secs(2))),
    ] {
        let (wakes, hub_wakes) = measure(&hub, &col, id, interval);
        println!("wakes in {} s idle: {} sources={wakes} hub={hub_wakes}", WINDOW.as_secs(), id.name());
        report.push((id.name(), wakes));
        match id {
            // Event driven: nothing changes, nothing wakes (a name-change event from some other window may).
            ServiceId::Battery | ServiceId::Audio | ServiceId::Media | ServiceId::Window => {
                assert!(wakes <= 6, "{}: {wakes} wakes in {} s", id.name(), WINDOW.as_secs());
            }
            // Sampled at the subscriber's period: about one wake per period, never a tight loop.
            ServiceId::Network => assert!(wakes <= 2 * WINDOW.as_secs() / 2 + 6, "network: {wakes}"),
            // No thread of its own: the hub's timer does it.
            ServiceId::System => {
                assert_eq!(wakes, 0, "the system sampler has no source thread");
                assert!(hub_wakes <= 2 * WINDOW.as_secs() / 2 + 3, "system: hub woke {hub_wakes} times");
            }
            ServiceId::Workspaces => unreachable!(),
        }
    }
    println!("idle wake report ({} s each): {report:?}", WINDOW.as_secs());
    hub.shutdown(WAIT);
}

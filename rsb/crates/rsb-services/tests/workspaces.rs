//! The workspaces service against `FakeGlazeServer` (Astur's protocol) and
//! `FakeKomorebi` (komorebi's subscribe pipe): mapping, commands, event
//! re-queries, reconnects, provider `none`, and the size caps.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use rsb_core::element::InstanceId;
use rsb_core::msg::{HubPort, HubRequest};
use rsb_core::services::{ServiceCommand, ServiceEvent, ServiceId, ServiceStatus, WorkspaceCommand, check_value};
use rsb_core::value::Value;
use rsb_services::fake::glazewm::{FakeGlazeServer, FakeWorkspace};
use rsb_services::fake::komorebi::{FakeKomorebi, FakeKomorebiControl};
use rsb_services::{
    Hub, HubOptions, HubTiming, Service, ServiceFactory, WS_MAX_MESSAGE, WorkspaceOptions, workspaces_service,
};

struct WsFactory {
    options: WorkspaceOptions,
    control: Arc<FakeKomorebiControl>,
}

impl ServiceFactory for WsFactory {
    fn create(&self, id: ServiceId, _options: &HubOptions) -> Box<dyn Service> {
        assert_eq!(id, ServiceId::Workspaces, "only workspaces is subscribed here");
        workspaces_service(self.options.clone(), Some(self.control.clone()))
    }
}

fn pipe_name(tag: &str) -> String {
    format!("rsb-test-komorebi-{}-{tag}", std::process::id())
}

/// A port nothing listens on.
fn dead_port() -> u16 {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    l.local_addr().unwrap().port()
}

fn options(glaze_port: u16, tag: &str) -> WorkspaceOptions {
    WorkspaceOptions {
        glazewm_url: format!("ws://127.0.0.1:{glaze_port}"),
        komorebic: "komorebic".into(),
        komorebi_pipe: pipe_name(tag),
        detect_every: Duration::from_millis(100),
    }
}

fn start(options: WorkspaceOptions, control: Arc<FakeKomorebiControl>) -> (Hub, Collector) {
    let (hub, col) = hub_with(Box::new(WsFactory { options, control }), HubTiming::default());
    subscribe(&hub, ServiceId::Workspaces, 1, None);
    (hub, col)
}

fn ws(name: &str, display: &str, windows: u32) -> FakeWorkspace {
    FakeWorkspace { name: name.into(), display_name: display.into(), windows }
}

fn two_monitors() -> (Vec<Vec<FakeWorkspace>>, (usize, usize)) {
    (vec![vec![ws("1", "", 3), ws("2", "Web", 0), ws("3", "", 1)], vec![ws("4", "", 2), ws("5", "", 0)]], (0, 0))
}

fn workspaces_of(e: &ServiceEvent) -> Vec<(usize, String, bool, bool, i64)> {
    let mut out = Vec::new();
    let v = e.value.as_ref().expect("a value");
    let Value::List(monitors) = v.get("monitors").expect("monitors") else { panic!("monitors is a list") };
    for (mi, m) in monitors.iter().enumerate() {
        let Value::List(list) = m.get("workspaces").unwrap() else { panic!() };
        for w in list {
            out.push((
                mi,
                w.get("name").unwrap().as_str().unwrap().to_string(),
                w.get("focused").unwrap().as_bool().unwrap(),
                w.get("visible").unwrap().as_bool().unwrap(),
                w.get("window_count").unwrap().as_f64().unwrap() as i64,
            ));
        }
    }
    out
}

fn next_with<F: Fn(&ServiceEvent) -> bool>(col: &Collector, timeout: Duration, pick: F) -> Option<ServiceEvent> {
    let end = Instant::now() + timeout;
    loop {
        let left = end.saturating_duration_since(Instant::now());
        let e = col.value(ServiceId::Workspaces, left)?;
        if pick(&e) {
            return Some(e);
        }
    }
}

fn provider(e: &ServiceEvent) -> String {
    e.value.as_ref().and_then(|v| v.get("provider")).and_then(Value::as_str).unwrap_or("").to_string()
}

fn focused_name(e: &ServiceEvent) -> Option<String> {
    workspaces_of(e).into_iter().find(|w| w.2).map(|w| w.1)
}

// ---------------------------------------------------------------- glazewm

#[test]
fn glaze_state_maps_to_the_model_and_passes_the_schema() {
    let server = FakeGlazeServer::start().unwrap();
    let (m, f) = two_monitors();
    server.set_state(m, f);
    let (hub, col) = start(options(server.port(), "g1"), Arc::new(FakeKomorebiControl::new(false)));
    let e = next_with(&col, WAIT, |e| provider(e) == "glazewm").expect("a glazewm value");
    assert_eq!(e.status, ServiceStatus::Ok);
    assert_eq!(check_value(ServiceId::Workspaces, e.value.as_ref().unwrap()), Ok(()));
    assert_eq!(e.value.as_ref().unwrap().get("connected").and_then(Value::as_bool), Some(true));
    let got = workspaces_of(&e);
    assert_eq!(
        got,
        vec![
            (0, "1".into(), true, true, 3),
            (0, "2".into(), false, false, 0),
            (0, "3".into(), false, false, 1),
            (1, "4".into(), false, true, 2),
            (1, "5".into(), false, false, 0),
        ]
    );
    // The display name falls back to the name.
    let Value::List(monitors) = e.value.as_ref().unwrap().get("monitors").unwrap() else { panic!() };
    let Value::List(first) = monitors[0].get("workspaces").unwrap() else { panic!() };
    assert_eq!(first[0].get("display_name").and_then(Value::as_str), Some("1"));
    assert_eq!(first[1].get("display_name").and_then(Value::as_str), Some("Web"));
    hub.shutdown(WAIT);
}

#[test]
fn the_subscription_and_query_go_out_verbatim() {
    let server = FakeGlazeServer::start().unwrap();
    let (m, f) = two_monitors();
    server.set_state(m, f);
    let (hub, col) = start(options(server.port(), "g2"), Arc::new(FakeKomorebiControl::new(false)));
    next_with(&col, WAIT, |e| provider(e) == "glazewm").unwrap();
    let got = server.received();
    assert_eq!(got[0], "sub -e workspace_activated workspace_deactivated workspace_updated focus_changed");
    assert_eq!(got[1], "query monitors");
    hub.shutdown(WAIT);
}

#[test]
fn focus_goes_out_verbatim_and_the_new_state_comes_back() {
    let server = FakeGlazeServer::start().unwrap();
    let (m, f) = two_monitors();
    server.set_state(m, f);
    let (hub, col) = start(options(server.port(), "g3"), Arc::new(FakeKomorebiControl::new(false)));
    next_with(&col, WAIT, |e| provider(e) == "glazewm").unwrap();
    hub.port().send(HubRequest::Command {
        owner: InstanceId(1),
        command: ServiceCommand::Workspaces(WorkspaceCommand::Focus("3".into())),
    });
    let e = next_with(&col, WAIT, |e| focused_name(e).as_deref() == Some("3")).expect("workspace 3 becomes focused");
    assert_eq!(e.status, ServiceStatus::Ok);
    assert!(server.received().iter().any(|c| c == "command focus --workspace 3"), "{:?}", server.received());
    hub.shutdown(WAIT);
}

#[test]
fn next_and_previous_are_worked_out_from_the_model() {
    let server = FakeGlazeServer::start().unwrap();
    let (m, _) = two_monitors();
    server.set_state(m, (0, 0));
    let (hub, col) = start(options(server.port(), "g4"), Arc::new(FakeKomorebiControl::new(false)));
    next_with(&col, WAIT, |e| provider(e) == "glazewm").unwrap();
    let send = |c: WorkspaceCommand| {
        hub.port().send(HubRequest::Command { owner: InstanceId(1), command: ServiceCommand::Workspaces(c) });
    };
    let sent = |what: &str| eventually(WAIT, || server.received().iter().any(|c| c == what));

    send(WorkspaceCommand::FocusNext(None));
    assert!(sent("command focus --workspace 2"), "{:?}", server.received());
    next_with(&col, WAIT, |e| focused_name(e).as_deref() == Some("2")).unwrap();

    send(WorkspaceCommand::FocusPrevious(None));
    assert!(eventually(WAIT, || server.received().iter().filter(|c| *c == "command focus --workspace 1").count() == 1));
    next_with(&col, WAIT, |e| focused_name(e).as_deref() == Some("1")).unwrap();

    // Previous from the first wraps to the last.
    send(WorkspaceCommand::FocusPrevious(None));
    assert!(sent("command focus --workspace 3"), "{:?}", server.received());
    next_with(&col, WAIT, |e| focused_name(e).as_deref() == Some("3")).unwrap();

    // A monitor that does not have focus is addressed by index: its shown workspace is 4, so next is 5.
    send(WorkspaceCommand::FocusNext(Some(1)));
    assert!(sent("command focus --workspace 5"), "{:?}", server.received());
    hub.shutdown(WAIT);
}

#[test]
fn an_event_makes_one_new_query_and_a_burst_is_merged() {
    let server = FakeGlazeServer::start().unwrap();
    let (m, f) = two_monitors();
    server.set_state(m.clone(), f);
    let (hub, col) = start(options(server.port(), "g5"), Arc::new(FakeKomorebiControl::new(false)));
    next_with(&col, WAIT, |e| provider(e) == "glazewm").unwrap();
    let queries = |s: &FakeGlazeServer| s.received().iter().filter(|c| *c == "query monitors").count();
    let before = queries(&server);
    // Five events in a row, the last state wins.
    for i in 0..5usize {
        let mut changed = m.clone();
        changed[0][0].windows = 10 + i as u32;
        server.set_state(changed, (0, i % 3));
    }
    let e = next_with(&col, WAIT, |e| workspaces_of(e)[0].4 == 14).expect("the last state arrives");
    assert_eq!(workspaces_of(&e)[0].4, 14);
    std::thread::sleep(Duration::from_millis(200));
    let extra = queries(&server) - before;
    println!("event burst: 5 events -> {extra} extra queries");
    assert!((1..=3).contains(&extra), "{extra}");
    hub.shutdown(WAIT);
}

#[test]
fn a_dropped_connection_is_reconnected_and_state_flows_again() {
    let server = FakeGlazeServer::start().unwrap();
    let (m, f) = two_monitors();
    server.set_state(m.clone(), f);
    let (hub, col) = start(options(server.port(), "g6"), Arc::new(FakeKomorebiControl::new(false)));
    next_with(&col, WAIT, |e| provider(e) == "glazewm").unwrap();
    assert!(eventually(WAIT, || server.client_count() == 1));
    let subs_before = server.received().iter().filter(|c| c.starts_with("sub ")).count();

    server.drop_clients();
    assert!(eventually(Duration::from_secs(8), || server.client_count() == 1), "reconnected");
    assert!(
        eventually(WAIT, || server.received().iter().filter(|c| c.starts_with("sub ")).count() == subs_before + 1),
        "subscribed again on the new connection"
    );
    let mut changed = m;
    changed[1][1].windows = 7;
    server.set_state(changed, (1, 1));
    let e =
        next_with(&col, WAIT, |e| focused_name(e).as_deref() == Some("5")).expect("updates flow after the reconnect");
    assert_eq!(e.status, ServiceStatus::Ok);
    hub.shutdown(WAIT);
}

#[test]
fn no_server_means_provider_none_and_unavailable_not_an_error() {
    let (hub, col) = start(options(dead_port(), "g7"), Arc::new(FakeKomorebiControl::new(false)));
    let t = Instant::now();
    let e = col.value(ServiceId::Workspaces, WAIT).expect("a value");
    println!("no window manager: first answer after {} ms", t.elapsed().as_millis());
    assert_eq!(provider(&e), "none");
    assert_eq!(e.status, ServiceStatus::Unavailable);
    assert_eq!(e.value.as_ref().unwrap().get("connected").and_then(Value::as_bool), Some(false));
    assert_eq!(e.value.as_ref().unwrap().get("monitors"), Some(&Value::List(vec![])));
    assert_eq!(check_value(ServiceId::Workspaces, e.value.as_ref().unwrap()), Ok(()));
    hub.shutdown(WAIT);
}

#[test]
fn a_server_that_goes_away_for_good_becomes_provider_none_after_the_grace() {
    let server = FakeGlazeServer::start().unwrap();
    let (m, f) = two_monitors();
    server.set_state(m, f);
    let port = server.port();
    let (hub, col) = start(options(port, "g8"), Arc::new(FakeKomorebiControl::new(false)));
    next_with(&col, WAIT, |e| provider(e) == "glazewm").unwrap();
    // Let the connection count as a real one (lasting longer than the 5 s grace), then end the server.
    std::thread::sleep(Duration::from_millis(5300));
    drop(server);
    let t = Instant::now();
    let e = next_with(&col, Duration::from_secs(15), |e| provider(e) == "none").expect("provider none");
    println!("server gone: provider none after {} ms", t.elapsed().as_millis());
    assert_eq!(e.status, ServiceStatus::Unavailable);
    assert!(t.elapsed() >= Duration::from_secs(4), "not before the 5 s grace: {:?}", t.elapsed());
    hub.shutdown(WAIT);
}

#[test]
fn a_message_over_the_cap_is_refused_and_the_connection_is_started_again() {
    let server = FakeGlazeServer::start().unwrap();
    let (m, f) = two_monitors();
    server.set_state(m, f);
    let (hub, col) = start(options(server.port(), "g9"), Arc::new(FakeKomorebiControl::new(false)));
    let first = next_with(&col, WAIT, |e| provider(e) == "glazewm").unwrap();
    let subs_before = server.received().iter().filter(|c| c.starts_with("sub ")).count();
    server.send_raw("x".repeat(WS_MAX_MESSAGE + 1000));
    assert!(
        eventually(Duration::from_secs(8), || server.received().iter().filter(|c| c.starts_with("sub ")).count()
            > subs_before),
        "the client dropped the connection and made a new one"
    );
    // Nothing from the oversize text reached the model.
    let events = col.service_events(ServiceId::Workspaces, Duration::from_millis(300));
    assert!(events.iter().all(|e| e.value.as_ref().is_none_or(|v| check_value(ServiceId::Workspaces, v).is_ok())));
    assert_eq!(first.status, ServiceStatus::Ok);
    hub.shutdown(WAIT);
}

#[test]
fn the_server_is_trusted_only_in_our_own_session_and_account() {
    // Our own process listens here, so the check that the listener is ours passes: this is the same-user case
    // every other test in this file relies on. The refusal paths are covered in the crate's own unit tests.
    let server = FakeGlazeServer::start().unwrap();
    let (hub, col) = start(options(server.port(), "g10"), Arc::new(FakeKomorebiControl::new(false)));
    let e = col.value(ServiceId::Workspaces, WAIT).unwrap();
    assert_ne!(e.status, ServiceStatus::Error, "{:?}", e.error);
    hub.shutdown(WAIT);
}

// --------------------------------------------------------------- komorebi

fn wait_subscribed(control: &FakeKomorebiControl, n: usize) -> String {
    assert!(
        eventually(Duration::from_secs(8), || control.subscribed().len() >= n),
        "komorebic subscribe-pipe was asked for"
    );
    control.subscribed()[n - 1].clone()
}

#[test]
fn komorebi_state_maps_and_focus_commands_use_komorebic() {
    let control = Arc::new(FakeKomorebiControl::new(true));
    let (hub, col) = start(options(dead_port(), "k1"), control.clone());
    let name = wait_subscribed(&control, 1);
    assert_eq!(name, pipe_name("k1"), "the pipe name is the one in the options");
    let mut komorebi = FakeKomorebi::connect(&name).expect("connect to the service's pipe");
    komorebi.send_state(&[vec![("main", 2), ("", 0), ("code", 1)], vec![("a", 0), ("b", 4)]], (0, 0)).unwrap();
    let e = next_with(&col, WAIT, |e| provider(e) == "komorebi").expect("a komorebi value");
    assert_eq!(e.status, ServiceStatus::Ok);
    assert_eq!(check_value(ServiceId::Workspaces, e.value.as_ref().unwrap()), Ok(()));
    assert_eq!(
        workspaces_of(&e),
        vec![
            (0, "main".into(), true, true, 2),
            (0, "2".into(), false, false, 0),
            (0, "code".into(), false, false, 1),
            (1, "a".into(), false, true, 0),
            (1, "b".into(), false, false, 4),
        ]
    );

    let send = |c: WorkspaceCommand| {
        hub.port().send(HubRequest::Command { owner: InstanceId(1), command: ServiceCommand::Workspaces(c) });
    };
    send(WorkspaceCommand::Focus("code".into()));
    send(WorkspaceCommand::FocusNext(None));
    send(WorkspaceCommand::FocusPrevious(Some(1)));
    assert!(eventually(WAIT, || control.focus_commands().len() == 3), "{:?}", control.focus_commands());
    let mut got = control.focus_commands();
    got.sort();
    assert_eq!(got, vec!["2".to_string(), "b".to_string(), "code".to_string()]);
    hub.shutdown(WAIT);
    assert!(control.unsubscribed.lock().unwrap().contains(&name), "it unsubscribes when it stops");
}

#[test]
fn a_closed_pipe_is_subscribed_again() {
    let control = Arc::new(FakeKomorebiControl::new(true));
    let (hub, col) = start(options(dead_port(), "k2"), control.clone());
    let name = wait_subscribed(&control, 1);
    let mut first = FakeKomorebi::connect(&name).unwrap();
    first.send_state(&[vec![("one", 1)]], (0, 0)).unwrap();
    next_with(&col, WAIT, |e| provider(e) == "komorebi").unwrap();
    first.close();

    let name2 = wait_subscribed(&control, 2);
    assert_eq!(name2, name);
    let mut second = FakeKomorebi::connect(&name2).expect("the pipe exists again");
    second.send_state(&[vec![("two", 5)]], (0, 0)).unwrap();
    let e = next_with(&col, WAIT, |e| workspaces_of(e).first().is_some_and(|w| w.1 == "two"))
        .expect("the new state arrives");
    assert_eq!(workspaces_of(&e)[0].4, 5);
    hub.shutdown(WAIT);
}

#[test]
fn an_object_over_the_cap_is_refused_and_the_pipe_is_started_again() {
    let control = Arc::new(FakeKomorebiControl::new(true));
    let (hub, col) = start(options(dead_port(), "k3"), control.clone());
    let name = wait_subscribed(&control, 1);
    let mut komorebi = FakeKomorebi::connect(&name).unwrap();
    komorebi.send_state(&[vec![("ok", 1)]], (0, 0)).unwrap();
    next_with(&col, WAIT, |e| provider(e) == "komorebi").unwrap();
    let mut huge = b"{\"state\":{\"pad\":\"".to_vec();
    huge.resize(rsb_services::KOMOREBI_MAX_OBJECT + 4096, b'x');
    // The service stops reading at the cap and closes its end: the write may fail part-way, which is the point.
    let _ = komorebi.send_raw(&huge);
    wait_subscribed(&control, 2);
    let events = col.service_events(ServiceId::Workspaces, Duration::from_millis(300));
    assert!(events.iter().all(|e| e.value.as_ref().is_none_or(|v| check_value(ServiceId::Workspaces, v).is_ok())));
    hub.shutdown(WAIT);
}

#[test]
fn komorebi_not_running_means_none() {
    let control = Arc::new(FakeKomorebiControl::new(false));
    let (hub, col) = start(options(dead_port(), "k4"), control.clone());
    let e = col.value(ServiceId::Workspaces, WAIT).unwrap();
    assert_eq!((provider(&e).as_str(), e.status), ("none", ServiceStatus::Unavailable));
    assert!(control.subscribed().is_empty(), "komorebic is not run when komorebi is not");
    // It starts later: detected within the detection period, no restart of the hub needed.
    control.set_running(true);
    let name = wait_subscribed(&control, 1);
    let mut komorebi = FakeKomorebi::connect(&name).unwrap();
    komorebi.send_state(&[vec![("late", 1)]], (0, 0)).unwrap();
    next_with(&col, WAIT, |e| provider(e) == "komorebi").expect("komorebi found once it runs");
    hub.shutdown(WAIT);
}

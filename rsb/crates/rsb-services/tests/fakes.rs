//! The fakes themselves: `FakeWorld` (also behind `--fake-services`),
//! `FakeGlazeServer` and `FakeKomorebi`.

mod common;

use std::time::Duration;

use common::*;
use rsb_core::element::InstanceId;
use rsb_core::msg::{HubPort, HubRequest};
use rsb_core::services::{AudioCommand, MediaCommand, ServiceCommand, ServiceId, WorkspaceCommand, check_value};
use rsb_core::value::Value;
use rsb_services::fake::FakeWorld;
use rsb_services::fake::glazewm::{FakeGlazeServer, FakeWorkspace};
use rsb_services::fake::komorebi::FakeKomorebi;
use tungstenite::client::IntoClientRequest;

#[test]
fn the_sample_world_has_a_valid_value_for_every_service() {
    let world = FakeWorld::with_sample_values();
    let (hub, col) = fake_hub(&world);
    for (n, id) in ServiceId::ALL.into_iter().enumerate() {
        subscribe(&hub, id, n as u32 + 1, None);
    }
    for id in ServiceId::ALL {
        let e = col.value(id, WAIT).unwrap_or_else(|| panic!("{} has a sample value", id.name()));
        assert_eq!(check_value(id, e.value.as_ref().unwrap()), Ok(()), "{}", id.name());
    }
    hub.shutdown(WAIT);
}

#[test]
fn fake_commands_are_recorded_and_change_the_sample_state() {
    let world = FakeWorld::with_sample_values();
    let (hub, col) = fake_hub(&world);
    let send = |c: ServiceCommand| hub.port().send(HubRequest::Command { owner: InstanceId(1), command: c });
    subscribe(&hub, ServiceId::Audio, 1, None);
    subscribe(&hub, ServiceId::Media, 1, None);
    subscribe(&hub, ServiceId::Workspaces, 1, None);
    col.value(ServiceId::Audio, WAIT).unwrap();

    send(ServiceCommand::Audio(AudioCommand::ChangeVolume(5.0)));
    let e = col.wait(WAIT, |e| match e {
        rsb_core::msg::HubEvent::Service(s) if s.service == ServiceId::Audio => {
            s.value.as_ref().filter(|v| v.get("volume").and_then(Value::as_f64) == Some(40.0)).map(|_| ())
        }
        _ => None,
    });
    assert!(e.is_some(), "35 + 5 = 40");
    send(ServiceCommand::Audio(AudioCommand::ToggleMute));
    send(ServiceCommand::Media(MediaCommand::PlayPause));
    send(ServiceCommand::Workspaces(WorkspaceCommand::Focus("4".into())));
    assert!(eventually(WAIT, || world.commands().len() == 4), "{:?}", world.commands());
    assert_eq!(world.commands()[0], ServiceCommand::Audio(AudioCommand::ChangeVolume(5.0)));
    assert_eq!(world.commands()[3], ServiceCommand::Workspaces(WorkspaceCommand::Focus("4".into())));
    hub.shutdown(WAIT);
}

#[test]
fn the_glaze_fake_refuses_browser_origins_like_astur() {
    let server = FakeGlazeServer::start().unwrap();
    let stream = std::net::TcpStream::connect(("127.0.0.1", server.port())).unwrap();
    let mut request = server.url().into_client_request().unwrap();
    request.headers_mut().insert("origin", "https://evil.example".parse().unwrap());
    assert!(tungstenite::client(request, stream).is_err(), "a handshake with an Origin header is refused");
    assert_eq!(server.client_count(), 0);

    let stream = std::net::TcpStream::connect(("127.0.0.1", server.port())).unwrap();
    let (mut ws, _) =
        tungstenite::client(server.url().into_client_request().unwrap(), stream).expect("no origin: accepted");
    ws.send(tungstenite::Message::text("query monitors")).unwrap();
    let reply = ws.read().unwrap();
    let json: serde_json::Value = serde_json::from_str(reply.to_text().unwrap()).unwrap();
    assert_eq!(json["messageType"], "client_response");
    assert_eq!(json["clientMessage"], "query monitors");
    assert_eq!(json["success"], true);
    ws.send(tungstenite::Message::text("command shell rm -rf")).unwrap();
    let reply: serde_json::Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(reply["success"], false, "anything beyond workspace queries is refused");
}

#[test]
fn the_glaze_fake_nests_windows_like_glazewm() {
    let server = FakeGlazeServer::start().unwrap();
    server.set_state(vec![vec![FakeWorkspace { name: "1".into(), display_name: "".into(), windows: 3 }]], (0, 0));
    let stream = std::net::TcpStream::connect(("127.0.0.1", server.port())).unwrap();
    let (mut ws, _) = tungstenite::client(server.url().into_client_request().unwrap(), stream).unwrap();
    ws.send(tungstenite::Message::text("query monitors")).unwrap();
    let reply: serde_json::Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
    let children = &reply["data"]["monitors"][0]["children"][0]["children"];
    assert_eq!(children[0]["type"], "window");
    assert_eq!(children[1]["type"], "split", "the other windows sit in a nested container");
    assert_eq!(reply["data"]["monitors"][0]["children"][0]["displayName"], serde_json::Value::Null);
}

#[test]
fn the_komorebi_fake_gives_up_when_nothing_listens() {
    let name = format!("rsb-test-fake-komorebi-{}", std::process::id());
    let t = std::time::Instant::now();
    assert!(FakeKomorebi::connect(&name).is_err(), "nothing listens on that pipe name");
    assert!(t.elapsed() < Duration::from_secs(4), "the retries are bounded: {:?}", t.elapsed());
}

#[test]
fn silence_and_resume_are_visible_to_the_hub() {
    let world = FakeWorld::with_sample_values();
    world.set_stale_after(ServiceId::System, Duration::from_millis(200));
    let (hub, col) = fake_hub(&world);
    subscribe(&hub, ServiceId::System, 1, None);
    col.value(ServiceId::System, WAIT).unwrap();
    world.go_silent(ServiceId::System);
    let stale = col.service(ServiceId::System, WAIT).expect("a status event");
    assert_eq!(stale.status, rsb_core::services::ServiceStatus::Stale);
    world.resume(ServiceId::System);
    let back = col.service(ServiceId::System, WAIT).expect("back again");
    assert_eq!(back.status, rsb_core::services::ServiceStatus::Ok);
    hub.shutdown(WAIT);
}

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

fn current(col: &Collector, id: ServiceId) -> Value {
    let events = col.service_events(id, Duration::from_millis(150));
    events.iter().rev().find_map(|e| e.value.clone()).expect("a value was published")
}

fn field(v: &Value, key: &str) -> Value {
    v.get(key).cloned().unwrap_or(Value::Nil)
}

fn device_ids(v: &Value, key: &str) -> Vec<String> {
    match v.get(key) {
        Some(Value::List(l)) => {
            l.iter().filter_map(|d| d.get("id").and_then(Value::as_str).map(str::to_string)).collect()
        }
        _ => Vec::new(),
    }
}

#[test]
fn the_fake_switches_the_default_output_and_refuses_ids_it_never_published() {
    use rsb_core::services::AudioDeviceCommand;
    let world = FakeWorld::with_sample_values();
    let (hub, col) = fake_hub(&world);
    let send = |c: AudioDeviceCommand| {
        hub.port().send(HubRequest::Command { owner: InstanceId(1), command: ServiceCommand::AudioDevice(c) })
    };
    subscribe(&hub, ServiceId::Audio, 1, None);
    let first = col.value(ServiceId::Audio, WAIT).unwrap().value.unwrap();
    let outputs = device_ids(&first, "devices");
    assert_eq!(outputs.len(), 2);
    assert_eq!(
        first.get("devices").and_then(|d| if let Value::List(l) = d { l[0].get("default") } else { None }),
        Some(&Value::Bool(true))
    );
    assert_eq!(field(&first, "switch_available"), Value::Bool(true));

    // Ids that were never published: a path, a GUID with a program, a parent folder, a control character, a huge id.
    let long = "a".repeat(2000);
    let bad = [
        r"C:\Windows\System32\cmd.exe",
        r"{00000000-0000-0000-0000-000000000000}\cmd.exe",
        r"..\..\x",
        "fake-out-headset\n",
        "FAKE-OUT-HEADSET",
        "",
        long.as_str(),
    ];
    for id in bad {
        send(AudioDeviceCommand::SetDefault(id.into()));
        send(AudioDeviceCommand::SetVolume(id.into(), 10.0));
        send(AudioDeviceCommand::SetMuted(id.into(), true));
    }
    std::thread::sleep(Duration::from_millis(300));
    assert!(world.commands().is_empty(), "no refused command reached the fake's record");

    send(AudioDeviceCommand::SetVolume("fake-out-headset".into(), 42.0));
    send(AudioDeviceCommand::SetMuted("fake-in-mic".into(), true));
    send(AudioDeviceCommand::SetDefault("fake-out-headset".into()));
    assert!(eventually(WAIT, || world.commands().len() == 3), "{:?}", world.commands());
    let now = current(&col, ServiceId::Audio);
    assert_eq!(device_ids(&now, "devices"), ["fake-out-headset", "fake-out-speakers"], "the new default comes first");
    assert_eq!(field(&now, "volume"), Value::Num(42.0), "the plain volume follows the default output");
    let inputs = match now.get("inputs") {
        Some(Value::List(l)) => l.clone(),
        _ => panic!("inputs"),
    };
    assert_eq!(inputs[0].get("muted"), Some(&Value::Bool(true)));
    assert_eq!(check_value(ServiceId::Audio, &now), Ok(()));
    hub.shutdown(WAIT);
}

#[test]
fn the_fake_media_seeks_inside_the_track_and_advances_only_when_told() {
    use rsb_core::services::{MediaCommand, ServiceStatus};
    let world = FakeWorld::with_sample_values();
    let (hub, col) = fake_hub(&world);
    subscribe(&hub, ServiceId::Media, 1, None);
    let first = col.value(ServiceId::Media, WAIT).unwrap().value.unwrap();
    assert_eq!(field(&first, "position"), Value::Num(60.0));
    assert_eq!(field(&first, "art"), Value::Nil);
    assert_eq!(field(&first, "app_icon"), Value::Nil);
    assert_eq!(check_value(ServiceId::Media, &first), Ok(()));

    std::thread::sleep(Duration::from_millis(300));
    let idle = col.service_events(ServiceId::Media, Duration::from_millis(200));
    assert!(
        idle.iter().filter_map(|e| e.value.as_ref()).all(|v| field(v, "position") == Value::Num(60.0)),
        "time does not pass by itself"
    );
    world.advance_media(5.0);
    let moved = col.value(ServiceId::Media, WAIT).unwrap().value.unwrap();
    assert_eq!(field(&moved, "position"), Value::Num(65.0));
    world.advance_media(1000.0);
    let end = col.value(ServiceId::Media, WAIT).unwrap().value.unwrap();
    assert_eq!(field(&end, "position"), Value::Num(240.0), "never past the end");

    hub.port()
        .send(HubRequest::Command { owner: InstanceId(1), command: ServiceCommand::Media(MediaCommand::Seek(30_000)) });
    let sought = col.value(ServiceId::Media, WAIT).unwrap().value.unwrap();
    assert_eq!(field(&sought, "position"), Value::Num(30.0));

    // A player that cannot seek refuses: the value stays and the command is not recorded.
    let mut v = sought.clone();
    if let Value::Map(m) = &mut v {
        m.insert("can_seek".into(), Value::Bool(false));
    }
    world.set(ServiceId::Media, v);
    col.value(ServiceId::Media, WAIT).unwrap();
    hub.port()
        .send(HubRequest::Command { owner: InstanceId(1), command: ServiceCommand::Media(MediaCommand::Seek(90_000)) });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(world.commands().len(), 1, "only the first seek was accepted");
    let _ = ServiceStatus::Ok;
    hub.shutdown(WAIT);
}

#[test]
fn the_fake_power_changes_its_mode_and_only_records_the_session_actions() {
    use rsb_core::services::{PowerCommand, PowerMode};
    let world = FakeWorld::with_sample_values();
    let (hub, col) = fake_hub(&world);
    subscribe(&hub, ServiceId::Power, 1, None);
    subscribe(&hub, ServiceId::Microphone, 1, None);
    col.value(ServiceId::Power, WAIT).unwrap();
    let send = |c: ServiceCommand| hub.port().send(HubRequest::Command { owner: InstanceId(1), command: c });
    send(ServiceCommand::Power(PowerCommand::SetMode(PowerMode::Performance)));
    let v = col.value(ServiceId::Power, WAIT).unwrap().value.unwrap();
    assert_eq!(field(&v, "mode"), Value::Str("performance".into()));
    for c in
        [PowerCommand::Lock, PowerCommand::Sleep, PowerCommand::Restart, PowerCommand::ShutDown, PowerCommand::SignOut]
    {
        send(ServiceCommand::Power(c));
    }
    send(ServiceCommand::Microphone(rsb_core::services::MicrophoneCommand::ToggleMute));
    assert!(eventually(WAIT, || world.commands().len() == 7), "{:?}", world.commands());
    let after = current(&col, ServiceId::Power);
    assert_eq!(field(&after, "mode"), Value::Str("performance".into()), "an action never changes the value");
    let mic = current(&col, ServiceId::Microphone);
    assert_eq!(field(&mic, "muted"), Value::Bool(true));

    // A PC without the interface: the fake refuses the mode like the real service.
    let mut unavailable = after.clone();
    if let Value::Map(m) = &mut unavailable {
        m.insert("mode_available".into(), Value::Bool(false));
    }
    world.set(ServiceId::Power, unavailable);
    col.value(ServiceId::Power, WAIT).unwrap();
    send(ServiceCommand::Power(PowerCommand::SetMode(PowerMode::Efficiency)));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(world.commands().len(), 7, "the refused mode change is not recorded");
    hub.shutdown(WAIT);
}

#[test]
fn the_hub_drops_an_image_whose_key_is_not_its_services() {
    use rsb_core::msg::HubEvent;
    use rsb_core::pixels::RgbaImage;
    let world = FakeWorld::with_sample_values();
    let (hub, col) = fake_hub(&world);
    subscribe(&hub, ServiceId::Media, 1, None);
    col.value(ServiceId::Media, WAIT).unwrap();
    let image = || RgbaImage::new(2, 2, vec![255; 16]).unwrap();
    for key in ["rsb-image:apps/1", "rsb-image:media", "rsb-image:medi/1", "media/1", "rsb-image:Media/1", ""] {
        world.publish_image(ServiceId::Media, key, image());
    }
    world.publish_image(ServiceId::Media, "rsb-image:media/7", image());
    let key = col
        .wait(WAIT, |e| match e {
            HubEvent::Image { key, .. } => Some(key.to_string()),
            _ => None,
        })
        .expect("the service's own key arrives");
    assert_eq!(key, "rsb-image:media/7", "every foreign prefix was dropped before it");
    assert!(col.drain(Duration::from_millis(200)).iter().all(|e| !matches!(e, HubEvent::Image { .. })));
    hub.shutdown(WAIT);
}

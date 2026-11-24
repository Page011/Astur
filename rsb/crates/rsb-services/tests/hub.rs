//! The hub against a fake source: lazy start and stop, subscription sets,
//! intervals, versions, staleness, restarts and commands.

mod common;

use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::*;
use rsb_core::element::InstanceId;
use rsb_core::msg::{HubEvent, HubPort, HubRequest};
use rsb_core::services::{AudioCommand, MediaCommand, ServiceCommand, ServiceId, ServiceStatus, WorkspaceCommand};
use rsb_core::value::Value;
use rsb_services::fake::FakeWorld;

fn system_value(cpu: f64) -> Value {
    Value::Map(
        [
            ("cpu".to_string(), Value::Num(cpu)),
            ("memory".to_string(), Value::Num(40.0)),
            ("memory_used".to_string(), Value::Int(4_000)),
            ("memory_total".to_string(), Value::Int(10_000)),
        ]
        .into_iter()
        .collect(),
    )
}

fn sleep(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

#[test]
fn a_service_starts_with_its_first_subscriber_and_stops_after_the_last() {
    let world = FakeWorld::new();
    world.set(ServiceId::System, system_value(10.0));
    let (hub, col) = fake_hub(&world);
    sleep(100);
    assert_eq!(world.starts(ServiceId::System), 0, "nothing runs before someone subscribes");

    subscribe(&hub, ServiceId::System, 1, None);
    let first = col.value(ServiceId::System, WAIT).expect("a value arrives");
    assert_eq!(world.starts(ServiceId::System), 1);
    assert_eq!(first.status, ServiceStatus::Ok);

    unsubscribe(&hub, ServiceId::System, 1);
    sleep(100);
    assert_eq!(world.stops(ServiceId::System), 0, "a reload re-subscribes within the stop delay: not stopped yet");
    sleep(400);
    assert_eq!(world.stops(ServiceId::System), 1, "stopped once the delay (250 ms here) passed");
    let off = col.service_events(ServiceId::System, Duration::from_millis(100));
    assert_eq!(off.last().map(|e| e.status), Some(ServiceStatus::Off));
    hub.shutdown(WAIT);
}

#[test]
fn two_subscribes_then_one_unsubscribe_stops_two_seconds_later() {
    // The real timing: no HubTiming override.
    let world = FakeWorld::new();
    world.set(ServiceId::System, system_value(10.0));
    let (tx, rx) = mpsc::channel();
    let hub = rsb_services::Hub::start(
        rsb_services::HubOptions::default(),
        Box::new(world.clone()),
        Box::new({
            let tx = std::sync::Mutex::new(tx);
            move |e| {
                let _ = tx.lock().unwrap().send(e);
            }
        }),
    );
    let _keep = rx;
    // The same owner twice is one subscription; a second owner is another.
    subscribe(&hub, ServiceId::System, 1, None);
    subscribe(&hub, ServiceId::System, 1, None);
    sleep(100);
    assert_eq!(world.starts(ServiceId::System), 1, "a repeated Subscribe does not start a second run");
    unsubscribe(&hub, ServiceId::System, 1);
    let t0 = Instant::now();
    sleep(1500);
    assert_eq!(world.stops(ServiceId::System), 0, "1.5 s after the last Unsubscribe it still runs");
    while world.stops(ServiceId::System) == 0 && t0.elapsed() < Duration::from_secs(4) {
        sleep(20);
    }
    let after = t0.elapsed();
    assert_eq!(world.stops(ServiceId::System), 1);
    assert!(
        after >= Duration::from_millis(1900) && after < Duration::from_millis(2600),
        "stopped {after:?} after the unsubscribe"
    );
    println!("stop delay measured: {} ms (target 2000)", after.as_millis());
    hub.shutdown(WAIT);
}

#[test]
fn the_set_is_keyed_by_owner() {
    let world = FakeWorld::new();
    world.set(ServiceId::Audio, audio(50.0));
    let (hub, col) = fake_hub(&world);
    subscribe(&hub, ServiceId::Audio, 1, None);
    subscribe(&hub, ServiceId::Audio, 2, None);
    col.value(ServiceId::Audio, WAIT).unwrap();
    unsubscribe(&hub, ServiceId::Audio, 1);
    sleep(500);
    assert_eq!(world.stops(ServiceId::Audio), 0, "owner 2 still reads it");
    hub.port().send(HubRequest::CancelOwner { owner: InstanceId(2) });
    sleep(600);
    assert_eq!(world.stops(ServiceId::Audio), 1, "CancelOwner removed the last subscription");
    // Unsubscribing something never subscribed is harmless.
    unsubscribe(&hub, ServiceId::Media, 9);
    sleep(50);
    hub.shutdown(WAIT);
}

fn audio(volume: f64) -> Value {
    Value::Map(
        [
            ("available".to_string(), Value::Bool(true)),
            ("volume".to_string(), Value::Num(volume)),
            ("muted".to_string(), Value::Bool(false)),
        ]
        .into_iter()
        .collect(),
    )
}

#[test]
fn the_interval_is_the_fastest_subscriber() {
    let world = FakeWorld::new();
    world.set(ServiceId::System, system_value(1.0));
    let (hub, col) = fake_hub(&world);
    let secs = Duration::from_secs;
    subscribe(&hub, ServiceId::System, 1, Some(secs(4)));
    col.value(ServiceId::System, WAIT).unwrap();
    assert_eq!(world.interval(ServiceId::System), Some(secs(4)));
    subscribe(&hub, ServiceId::System, 2, Some(secs(1)));
    sleep(80);
    assert_eq!(world.interval(ServiceId::System), Some(secs(1)), "the faster one wins");
    unsubscribe(&hub, ServiceId::System, 2);
    sleep(80);
    assert_eq!(world.interval(ServiceId::System), Some(secs(4)), "back to the remaining subscriber");
    subscribe(&hub, ServiceId::System, 1, Some(secs(3)));
    sleep(80);
    assert_eq!(world.interval(ServiceId::System), Some(secs(3)), "a repeated Subscribe replaces that owner's interval");
    subscribe(&hub, ServiceId::System, 1, Some(Duration::from_millis(100)));
    sleep(80);
    assert_eq!(world.interval(ServiceId::System), Some(Duration::from_millis(500)), "never faster than 0.5 s");
    subscribe(&hub, ServiceId::System, 1, None);
    sleep(80);
    assert_eq!(world.interval(ServiceId::System), Some(secs(2)), "no wish: the 2 s default");
    hub.shutdown(WAIT);
}

#[test]
fn versions_grow_and_unchanged_values_are_dropped() {
    let world = FakeWorld::new();
    world.set(ServiceId::System, system_value(10.0));
    let (hub, col) = fake_hub(&world);
    subscribe(&hub, ServiceId::System, 1, None);
    col.value(ServiceId::System, WAIT).unwrap();
    world.set(ServiceId::System, system_value(10.0));
    world.set(ServiceId::System, system_value(10.0));
    world.set(ServiceId::System, system_value(20.0));
    world.set(ServiceId::System, system_value(20.0));
    world.set(ServiceId::System, system_value(30.0));
    let events = col.service_events(ServiceId::System, Duration::from_millis(300));
    let with_value: Vec<_> = events.iter().filter(|e| e.value.is_some()).collect();
    assert_eq!(with_value.len(), 3, "10, 20 and 30: the repeats were dropped");
    let versions: Vec<u64> = events.iter().map(|e| e.version).collect();
    assert!(versions.windows(2).all(|w| w[1] == w[0] + 1), "versions {versions:?} grow by one per event");
    hub.shutdown(WAIT);
}

#[test]
fn the_first_value_after_a_restart_is_sent_even_when_equal() {
    let world = FakeWorld::new();
    world.set(ServiceId::System, system_value(10.0));
    let (hub, col) = fake_hub(&world);
    subscribe(&hub, ServiceId::System, 1, None);
    let a = col.value(ServiceId::System, WAIT).unwrap();
    unsubscribe(&hub, ServiceId::System, 1);
    sleep(600);
    assert_eq!(world.stops(ServiceId::System), 1);
    subscribe(&hub, ServiceId::System, 1, None);
    let b = col.value(ServiceId::System, WAIT).expect("the same value is sent again after a restart");
    assert_eq!(a.value, b.value);
    assert!(b.version > a.version);
    assert_eq!(world.starts(ServiceId::System), 2);
    hub.shutdown(WAIT);
}

#[test]
fn a_new_reader_of_a_running_service_gets_the_value_it_missed() {
    let world = FakeWorld::new();
    world.set(ServiceId::System, system_value(10.0));
    let (hub, col) = fake_hub(&world);
    subscribe(&hub, ServiceId::System, 1, None);
    col.value(ServiceId::System, WAIT).unwrap();
    subscribe(&hub, ServiceId::System, 2, None);
    let again = col.value(ServiceId::System, WAIT).expect("the new subscriber is sent the current value");
    assert_eq!(again.status, ServiceStatus::Ok);
    hub.shutdown(WAIT);
}

#[test]
fn silence_makes_it_stale_and_a_signal_brings_it_back() {
    let world = FakeWorld::new();
    world.set(ServiceId::System, system_value(10.0));
    world.set_stale_after(ServiceId::System, Duration::from_millis(250));
    let (hub, col) = fake_hub(&world);
    subscribe(&hub, ServiceId::System, 1, None);
    col.value(ServiceId::System, WAIT).unwrap();
    world.go_silent(ServiceId::System);
    let t0 = Instant::now();
    let stale = col.wait(WAIT, |e| match e {
        HubEvent::Service(s) if s.status == ServiceStatus::Stale => Some(s.clone()),
        _ => None,
    });
    let stale = stale.expect("goes stale");
    assert!(stale.value.is_none(), "a status-only event carries no value");
    let after = t0.elapsed();
    assert!(after >= Duration::from_millis(150) && after < Duration::from_millis(900), "stale after {after:?}");
    // Liveness without a new value recovers it.
    world.resume(ServiceId::System);
    let ok = col.wait(WAIT, |e| match e {
        HubEvent::Service(s) if s.status == ServiceStatus::Ok => Some(s.clone()),
        _ => None,
    });
    assert!(ok.is_some(), "back to Ok on the next signal");
    hub.shutdown(WAIT);
}

#[test]
fn alive_alone_keeps_a_service_fresh() {
    let world = FakeWorld::new();
    world.set(ServiceId::Battery, battery());
    world.set_stale_after(ServiceId::Battery, Duration::from_millis(300));
    let (hub, col) = fake_hub(&world);
    subscribe(&hub, ServiceId::Battery, 1, None);
    col.value(ServiceId::Battery, WAIT).unwrap();
    for _ in 0..8 {
        sleep(100);
        world.alive(ServiceId::Battery);
    }
    let events = col.service_events(ServiceId::Battery, Duration::from_millis(50));
    assert!(
        events.iter().all(|e| e.status != ServiceStatus::Stale),
        "liveness signals every 100 ms kept it fresh for 800 ms"
    );
    hub.shutdown(WAIT);
}

fn battery() -> Value {
    Value::Map(
        [
            ("present".to_string(), Value::Bool(false)),
            ("charging".to_string(), Value::Bool(false)),
            ("plugged".to_string(), Value::Bool(true)),
            ("saver".to_string(), Value::Bool(false)),
        ]
        .into_iter()
        .collect(),
    )
}

#[test]
fn status_changes_are_events_with_a_reason() {
    let world = FakeWorld::new();
    world.set(ServiceId::Audio, audio(30.0));
    let (hub, col) = fake_hub(&world);
    subscribe(&hub, ServiceId::Audio, 1, None);
    col.value(ServiceId::Audio, WAIT).unwrap();
    world.set_status(ServiceId::Audio, ServiceStatus::Unavailable, Some("no audio output device".into()));
    let e = col.wait(WAIT, |e| match e {
        HubEvent::Service(s) if s.status == ServiceStatus::Unavailable => Some(s.clone()),
        _ => None,
    });
    assert_eq!(e.and_then(|e| e.error).as_deref(), Some("no audio output device"));
    hub.shutdown(WAIT);
}

#[test]
fn a_value_that_breaks_its_schema_is_dropped() {
    let world = FakeWorld::new();
    world.set(ServiceId::System, Value::Map([("cpu".to_string(), Value::Str("lots".into()))].into_iter().collect()));
    let (hub, col) = fake_hub(&world);
    subscribe(&hub, ServiceId::System, 1, None);
    assert!(col.value(ServiceId::System, Duration::from_millis(400)).is_none(), "nothing wrong reaches a widget");
    world.set(ServiceId::System, system_value(5.0));
    assert!(col.value(ServiceId::System, WAIT).is_some(), "a good value after a bad one is delivered");
    hub.shutdown(WAIT);
}

#[test]
fn a_failed_start_retries_with_doubling_delay() {
    let world = FakeWorld::new();
    world.set(ServiceId::System, system_value(1.0));
    world.fail_starts(ServiceId::System, 3, "audio engine not running");
    let (hub, col) = fake_hub(&world);
    let t0 = Instant::now();
    subscribe(&hub, ServiceId::System, 1, None);
    let err = col.service(ServiceId::System, WAIT).expect("an error event");
    assert_eq!(err.status, ServiceStatus::Error);
    assert_eq!(err.error.as_deref(), Some("audio engine not running"));
    let ok = col.value(ServiceId::System, WAIT).expect("a value once a start works");
    assert_eq!(ok.status, ServiceStatus::Ok);
    let elapsed = t0.elapsed();
    // retry_first 60 ms doubling: attempts at 0, 60, 180 and 420 ms.
    assert_eq!(world.starts(ServiceId::System), 4, "three failures then a success");
    assert!(
        elapsed >= Duration::from_millis(380) && elapsed < Duration::from_millis(1500),
        "recovered after {elapsed:?}"
    );
    println!("start retries: 4 starts, recovered after {} ms (60+120+240 = 420 expected)", elapsed.as_millis());
    hub.shutdown(WAIT);
}

#[test]
fn leaving_during_a_retry_cancels_it() {
    let world = FakeWorld::new();
    world.fail_starts(ServiceId::System, 100, "never");
    let (hub, col) = fake_hub(&world);
    subscribe(&hub, ServiceId::System, 1, None);
    col.service(ServiceId::System, WAIT).unwrap();
    unsubscribe(&hub, ServiceId::System, 1);
    sleep(100);
    let starts = world.starts(ServiceId::System);
    sleep(700);
    assert_eq!(world.starts(ServiceId::System), starts, "no more attempts once nobody wants it");
    hub.shutdown(WAIT);
}

#[test]
fn status_lists_every_service() {
    let world = FakeWorld::new();
    world.set(ServiceId::System, system_value(1.0));
    let (hub, col) = fake_hub(&world);
    subscribe(&hub, ServiceId::System, 1, Some(Duration::from_secs(1)));
    subscribe(&hub, ServiceId::System, 2, None);
    col.value(ServiceId::System, WAIT).unwrap();
    let (tx, rx) = mpsc::channel();
    hub.port().send(HubRequest::Status { reply: tx });
    let infos = rx.recv_timeout(WAIT).expect("a reply");
    assert_eq!(infos.len(), 7);
    let sys = infos.iter().find(|i| i.id == ServiceId::System).unwrap();
    assert_eq!((sys.status, sys.subscribers, sys.interval), (ServiceStatus::Ok, 2, Some(Duration::from_secs(1))));
    assert!(sys.last_update_ago.is_some() && sys.version >= 2);
    let media = infos.iter().find(|i| i.id == ServiceId::Media).unwrap();
    assert_eq!((media.status, media.subscribers), (ServiceStatus::Off, 0));
    hub.shutdown(WAIT);
}

#[test]
fn commands_start_a_stopped_service_and_reach_it_in_order() {
    let world = FakeWorld::new();
    world.set(ServiceId::Audio, audio(50.0));
    let (hub, col) = fake_hub(&world);
    let cmds = [
        ServiceCommand::Audio(AudioCommand::ChangeVolume(5.0)),
        ServiceCommand::Audio(AudioCommand::ToggleMute),
        ServiceCommand::Audio(AudioCommand::SetVolume(20.0)),
        ServiceCommand::Audio(AudioCommand::SetMute(false)),
        ServiceCommand::Media(MediaCommand::PlayPause),
        ServiceCommand::Workspaces(WorkspaceCommand::Focus("2".into())),
    ];
    for c in &cmds {
        hub.port().send(HubRequest::Command { owner: InstanceId(1), command: c.clone() });
    }
    sleep(200);
    assert_eq!(world.commands(), cmds.to_vec(), "every command arrived, in order, exactly as sent");
    assert_eq!(world.starts(ServiceId::Audio), 1, "the audio service was started by the first command");
    let v = col.value(ServiceId::Audio, WAIT).unwrap();
    let after = col.service_events(ServiceId::Audio, Duration::from_millis(100));
    let last = after.iter().rev().find_map(|e| e.value.clone()).unwrap_or(v.value.unwrap());
    assert_eq!(last.get("volume").and_then(Value::as_f64), Some(20.0), "the fake applied them like the real one would");
    hub.shutdown(WAIT);
}

#[test]
fn shutdown_stops_every_service_within_its_bound() {
    let world = FakeWorld::new();
    world.set(ServiceId::System, system_value(1.0));
    world.set(ServiceId::Audio, audio(1.0));
    let (hub, col) = fake_hub(&world);
    subscribe(&hub, ServiceId::System, 1, None);
    subscribe(&hub, ServiceId::Audio, 1, None);
    col.value(ServiceId::System, WAIT).unwrap();
    col.value(ServiceId::Audio, WAIT).unwrap();
    let t0 = Instant::now();
    hub.shutdown(Duration::from_secs(1));
    assert!(t0.elapsed() < Duration::from_millis(500), "shutdown took {:?}", t0.elapsed());
    assert_eq!((world.stops(ServiceId::System), world.stops(ServiceId::Audio)), (1, 1));
}

#[test]
fn the_hub_sleeps_when_there_is_nothing_to_do() {
    let world = FakeWorld::new();
    let (hub, _col) = fake_hub(&world);
    sleep(100);
    let before = hub.wakes();
    sleep(1000);
    assert_eq!(hub.wakes() - before, 0, "an idle hub with no subscribers does not wake at all");
    hub.shutdown(WAIT);
}

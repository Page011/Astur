//! The Bluetooth service: the real reader once against an independent read of the radio, and every
//! command against the fake only. No pairing, connection or radio is ever changed here.

mod common;

use std::time::Duration;

use common::*;
use rsb_core::element::InstanceId;
use rsb_core::msg::{HubPort, HubRequest};
use rsb_core::services::{BluetoothCommand, ServiceCommand, ServiceId, check_value};
use rsb_core::value::Value;
use rsb_services::fake::FakeWorld;
use rsb_services::{Hub, HubTiming};
use windows::Win32::Devices::Bluetooth::{
    BLUETOOTH_FIND_RADIO_PARAMS, BluetoothFindFirstRadio, BluetoothFindRadioClose,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE};

fn radio_present() -> bool {
    let params = BLUETOOTH_FIND_RADIO_PARAMS { dwSize: size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32 };
    let mut radio = HANDLE::default();
    // SAFETY: live pointers; the handles are closed below.
    match unsafe { BluetoothFindFirstRadio(&params, &mut radio) } {
        Ok(find) => {
            // SAFETY: both handles came from the call above and are closed once.
            unsafe {
                let _ = CloseHandle(radio);
                let _ = BluetoothFindRadioClose(find);
            }
            true
        }
        Err(_) => false,
    }
}

fn devices(v: &Value) -> Vec<Value> {
    match v.get("devices") {
        Some(Value::List(l)) => l.clone(),
        _ => panic!("devices is a list"),
    }
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or_else(|| panic!("{key} is a boolean"))
}

fn command(hub: &Hub, c: BluetoothCommand) {
    hub.port().send(HubRequest::Command { owner: InstanceId(1), command: ServiceCommand::Bluetooth(c) });
}

/// The newest Bluetooth value after the hub has been quiet for `quiet`.
fn latest(col: &Collector, quiet: Duration) -> Value {
    col.service_events(ServiceId::Bluetooth, quiet)
        .into_iter()
        .rev()
        .find_map(|e| e.value)
        .expect("the fake published a value")
}

#[test]
fn the_real_service_matches_an_independent_radio_read() {
    logs::install();
    let (hub, col) = hub_with(Hub::real_factory(), HubTiming::default());
    subscribe(&hub, ServiceId::Bluetooth, 1, None);
    let e = col.value(ServiceId::Bluetooth, Duration::from_secs(20)).expect("a Bluetooth value in 20 s");
    let v = e.value.expect("a value");
    assert_eq!(check_value(ServiceId::Bluetooth, &v), Ok(()));
    assert_eq!(flag(&v, "available"), radio_present(), "radio presence");
    let list = devices(&v);
    for d in &list {
        if let Some(Value::Num(b)) = d.get("battery") {
            assert!((0.0..=100.0).contains(b));
        }
        if d.get("can_connect").and_then(Value::as_bool) == Some(true) {
            assert_eq!(d.get("kind").and_then(Value::as_str), Some("audio"), "only audio devices connect");
        }
    }
    if !flag(&v, "available") {
        assert!(list.is_empty(), "no radio, no devices");
    }
    println!(
        "bluetooth real: available {}, enabled {}, can_toggle {}, {} paired, {} connected, {} with a battery",
        flag(&v, "available"),
        flag(&v, "enabled"),
        flag(&v, "can_toggle"),
        list.len(),
        list.iter().filter(|d| d.get("connected").and_then(Value::as_bool) == Some(true)).count(),
        list.iter().filter(|d| matches!(d.get("battery"), Some(Value::Num(_)))).count()
    );
    // Real device names never reach the log.
    for d in &list {
        if let Some(Value::Str(name)) = d.get("name").filter(|n| matches!(n, Value::Str(s) if s.chars().count() >= 4)) {
            assert!(logs::all().iter().all(|l| !l.contains(name.as_str())), "a log line holds a device name");
        }
    }
    hub.shutdown(WAIT);
}

#[test]
fn the_fake_value_fits_the_schema_and_commands_change_it() {
    let world = FakeWorld::with_sample_values();
    let (hub, col) = fake_hub(&world);
    subscribe(&hub, ServiceId::Bluetooth, 1, None);
    let start = latest(&col, Duration::from_millis(150));
    assert_eq!(check_value(ServiceId::Bluetooth, &start), Ok(()));
    let list = devices(&start);
    assert_eq!(list.len(), 3);
    let id_of = |d: &Value| d.get("id").and_then(Value::as_str).unwrap().to_string();
    let connected = |v: &Value, id: &str| {
        devices(v).iter().find(|d| id_of(d) == id).and_then(|d| d.get("connected").and_then(Value::as_bool)).unwrap()
    };

    // A device that can connect: the mark moves, and only for that device.
    command(&hub, BluetoothCommand::Connect("bt-3".into()));
    let v = latest(&col, Duration::from_millis(150));
    assert!(connected(&v, "bt-3") && connected(&v, "bt-1"));
    command(&hub, BluetoothCommand::Disconnect("bt-3".into()));
    assert!(!connected(&latest(&col, Duration::from_millis(150)), "bt-3"));

    // An unknown id and a device that cannot connect change nothing.
    let before = latest(&col, Duration::from_millis(100));
    for id in ["", "nope", "..", "C:\\Windows\\System32\\cmd.exe", "{GUID}\\cmd.exe"] {
        command(&hub, BluetoothCommand::Connect(id.into()));
        command(&hub, BluetoothCommand::Forget(id.into()));
    }
    command(&hub, BluetoothCommand::Connect("bt-2".into()));
    assert_eq!(latest(&col, Duration::from_millis(150)), before);

    // Radio off: nothing is connected and a connect does nothing.
    command(&hub, BluetoothCommand::SetEnabled(false));
    let off = latest(&col, Duration::from_millis(150));
    assert!(!flag(&off, "enabled"));
    assert!(devices(&off).iter().all(|d| d.get("connected").and_then(Value::as_bool) == Some(false)));
    command(&hub, BluetoothCommand::Connect("bt-1".into()));
    assert!(!connected(&latest(&col, Duration::from_millis(150)), "bt-1"));
    command(&hub, BluetoothCommand::SetEnabled(true));
    assert!(flag(&latest(&col, Duration::from_millis(150)), "enabled"));

    // Forget drops the pairing; the settings page is a recorded request and changes no value.
    command(&hub, BluetoothCommand::Forget("bt-2".into()));
    let after = latest(&col, Duration::from_millis(150));
    assert_eq!(devices(&after).len(), 2);
    command(&hub, BluetoothCommand::OpenSettings);
    assert_eq!(latest(&col, Duration::from_millis(150)), after);
    assert_eq!(check_value(ServiceId::Bluetooth, &after), Ok(()));

    let kinds: Vec<&'static str> = world
        .commands()
        .iter()
        .filter_map(|c| match c {
            ServiceCommand::Bluetooth(_) => Some(c.kind()),
            _ => None,
        })
        .collect();
    assert_eq!(kinds.iter().filter(|k| **k == "open_settings").count(), 1, "recorded: {kinds:?}");
    hub.shutdown(WAIT);
}

#[test]
fn fake_device_names_never_reach_the_log() {
    logs::install();
    let world = FakeWorld::with_sample_values();
    let (hub, col) = fake_hub(&world);
    subscribe(&hub, ServiceId::Bluetooth, 1, None);
    command(&hub, BluetoothCommand::Connect("bt-3".into()));
    command(&hub, BluetoothCommand::Connect("not-a-device".into()));
    command(&hub, BluetoothCommand::Forget("bt-2".into()));
    let _ = col.drain(Duration::from_millis(300));
    for line in logs::all() {
        for name in ["Sample Headphones", "Sample Mouse", "Sample Speaker"] {
            assert!(!line.contains(name), "a log line holds a device name");
        }
    }
    hub.shutdown(WAIT);
}

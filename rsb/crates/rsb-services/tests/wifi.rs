//! The Wi-Fi service: the real reader once against independent reads (adapter count, location
//! access), and every command against the fake only. No scan, connect or disconnect is ever sent
//! to a real adapter here.

mod common;

use std::time::Duration;

use common::*;
use rsb_core::element::InstanceId;
use rsb_core::msg::{HubPort, HubRequest};
use rsb_core::services::{ServiceCommand, ServiceId, WifiCommand, check_value};
use rsb_core::value::Value;
use rsb_services::fake::FakeWorld;
use rsb_services::{Hub, HubTiming};
use windows::Security::Authorization::AppCapabilityAccess::{AppCapability, AppCapabilityAccessStatus};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::NetworkManagement::WiFi::{
    WLAN_INTERFACE_INFO_LIST, WlanCloseHandle, WlanEnumInterfaces, WlanFreeMemory, WlanOpenHandle,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::core::HSTRING;

fn adapters() -> usize {
    let (mut negotiated, mut handle) = (0u32, HANDLE::default());
    // SAFETY: live out pointers.
    if unsafe { WlanOpenHandle(2, None, &mut negotiated, &mut handle) } != 0 {
        return 0;
    }
    let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
    // SAFETY: a live handle and out pointer.
    let n = if unsafe { WlanEnumInterfaces(handle, None, &mut list) } == 0 && !list.is_null() {
        // SAFETY: the list is valid until freed below.
        let n = unsafe { (*list).dwNumberOfItems } as usize;
        // SAFETY: freed once.
        unsafe { WlanFreeMemory(list.cast()) };
        n
    } else {
        0
    };
    // SAFETY: closes the handle opened above.
    let _ = unsafe { WlanCloseHandle(handle, None) };
    n
}

fn location_allowed() -> bool {
    // SAFETY: COM for the test thread, left initialised (the thread ends with the test).
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    AppCapability::Create(&HSTRING::from("wiFiControl"))
        .and_then(|c| c.CheckAccess())
        .is_ok_and(|s| s == AppCapabilityAccessStatus::Allowed)
}

fn networks(v: &Value) -> Vec<Value> {
    match v.get("networks") {
        Some(Value::List(l)) => l.clone(),
        _ => panic!("networks is a list"),
    }
}

fn command(hub: &Hub, c: WifiCommand) {
    hub.port().send(HubRequest::Command { owner: InstanceId(1), command: ServiceCommand::Wifi(c) });
}

fn latest(col: &Collector) -> Value {
    col.service_events(ServiceId::Wifi, Duration::from_millis(150))
        .into_iter()
        .rev()
        .find_map(|e| e.value)
        .expect("the fake published a value")
}

fn connected_name(v: &Value) -> Option<String> {
    networks(v)
        .iter()
        .find(|n| n.get("connected").and_then(Value::as_bool) == Some(true))
        .and_then(|n| n.get("ssid").and_then(Value::as_str).map(str::to_string))
}

#[test]
fn the_real_service_matches_the_adapter_count_and_the_location_access() {
    logs::install();
    let (hub, col) = hub_with(Hub::real_factory(), HubTiming::default());
    subscribe(&hub, ServiceId::Wifi, 1, None);
    let e = col.value(ServiceId::Wifi, Duration::from_secs(20)).expect("a Wi-Fi value in 20 s");
    let v = e.value.expect("a value");
    assert_eq!(check_value(ServiceId::Wifi, &v), Ok(()));
    let adapters = adapters();
    let allowed = location_allowed();
    assert_eq!(v.get("available").and_then(Value::as_bool), Some(adapters > 0), "adapter count {adapters}");
    let access = v.get("access").and_then(Value::as_str).unwrap_or_default();
    // The service re-checks on every read, so a setting changed during the test could differ once.
    assert_eq!(access, if allowed { "allowed" } else { "needs_location" }, "location access");
    if access == "needs_location" {
        assert!(networks(&v).is_empty(), "no list without access");
        assert_eq!(v.get("scanning").and_then(Value::as_bool), Some(false));
    }
    for n in networks(&v) {
        let s = n.get("signal").and_then(Value::as_f64).unwrap();
        assert!((0.0..=100.0).contains(&s));
    }
    println!("wifi real: {adapters} adapters, access {access}, {} networks listed", networks(&v).len());
    // Network names never reach the log.
    for n in networks(&v) {
        if let Some(Value::Str(name)) = n.get("ssid").filter(|s| matches!(s, Value::Str(x) if x.chars().count() >= 4)) {
            assert!(logs::all().iter().all(|l| !l.contains(name.as_str())), "a log line holds a network name");
        }
    }
    hub.shutdown(WAIT);
}

#[test]
fn the_fake_value_fits_the_schema_and_commands_change_it() {
    logs::install();
    let world = FakeWorld::with_sample_values();
    let (hub, col) = fake_hub(&world);
    subscribe(&hub, ServiceId::Wifi, 1, None);
    let start = latest(&col);
    assert_eq!(check_value(ServiceId::Wifi, &start), Ok(()));
    assert_eq!(connected_name(&start).as_deref(), Some("Sample Home"));

    command(&hub, WifiCommand::Connect("Sample Cafe".into()));
    let v = latest(&col);
    assert_eq!(connected_name(&v).as_deref(), Some("Sample Cafe"));
    assert_eq!(check_value(ServiceId::Wifi, &v), Ok(()));

    // Not saved, and not in range: neither changes anything, and the fake refuses both as the real service does.
    command(&hub, WifiCommand::Connect("Sample Guest".into()));
    command(&hub, WifiCommand::Connect("no such network".into()));
    command(&hub, WifiCommand::Connect(String::new()));
    assert_eq!(connected_name(&latest(&col)).as_deref(), Some("Sample Cafe"));

    command(&hub, WifiCommand::Scan);
    assert_eq!(check_value(ServiceId::Wifi, &latest(&col)), Ok(()));
    command(&hub, WifiCommand::Disconnect);
    assert_eq!(connected_name(&latest(&col)), None);
    assert_eq!(world.commands().len(), 3, "connect, scan and disconnect; the three refused connects are not recorded");

    for line in logs::all() {
        for name in ["Sample Home", "Sample Cafe", "Sample Guest"] {
            assert!(!line.contains(name), "a log line holds a network name");
        }
    }
    hub.shutdown(WAIT);
}

#[test]
fn a_fake_without_location_access_publishes_no_networks() {
    let world = FakeWorld::with_sample_values();
    let mut v = world_value(&world);
    if let Value::Map(m) = &mut v {
        m.insert("networks".into(), Value::List(Vec::new()));
        m.insert("access".into(), Value::Str("needs_location".into()));
    }
    assert_eq!(check_value(ServiceId::Wifi, &v), Ok(()), "the needs_location shape is valid");
    world.set(ServiceId::Wifi, v.clone());
    let (hub, col) = fake_hub(&world);
    subscribe(&hub, ServiceId::Wifi, 1, None);
    let got = latest(&col);
    assert_eq!(got.get("access").and_then(Value::as_str), Some("needs_location"));
    assert!(networks(&got).is_empty());
    hub.shutdown(WAIT);
}

fn world_value(world: &FakeWorld) -> Value {
    let (hub, col) = fake_hub(world);
    subscribe(&hub, ServiceId::Wifi, 9, None);
    let v = latest(&col);
    hub.shutdown(WAIT);
    v
}

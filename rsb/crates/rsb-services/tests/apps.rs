//! The apps service: the real list once against a direct read of the shell's AppsFolder, icon
//! bounds on what the real service publishes, and commands against the fake only. Nothing here
//! starts an app or opens a page.

mod common;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use common::*;
use rsb_core::element::InstanceId;
use rsb_core::msg::{HubEvent, HubPort, HubRequest};
use rsb_core::services::{AppsCommand, ServiceCommand, ServiceId, check_value};
use rsb_core::value::Value;
use rsb_services::fake::FakeWorld;
use rsb_services::{Hub, HubTiming};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, IEnumShellItems, IShellItem, SHCreateItemFromParsingName, SIGDN, SIGDN_NORMALDISPLAY,
    SIGDN_PARENTRELATIVEPARSING,
};
use windows::core::w;

fn name_of(item: &IShellItem, kind: SIGDN) -> Option<String> {
    // SAFETY: a plain COM call; the string is freed below.
    let p = unsafe { item.GetDisplayName(kind) }.ok()?;
    // SAFETY: `p` is a NUL-terminated string until freed.
    let s = unsafe { p.to_string() }.ok();
    // SAFETY: frees what GetDisplayName allocated, once.
    unsafe { CoTaskMemFree(Some(p.0.cast_const().cast())) };
    s
}

/// Items of the AppsFolder with a name and a usable id, counted the way the service lists them
/// (no empty names, no ids with control characters or over 1024 chars, one entry per id).
fn direct_count() -> usize {
    std::thread::spawn(|| {
        // SAFETY: COM for this thread as an STA, balanced below.
        let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let mut ids = HashSet::new();
        {
            // SAFETY: standard shell calls on an initialised STA thread.
            let folder: IShellItem = unsafe { SHCreateItemFromParsingName(w!("shell:AppsFolder"), None) }.unwrap();
            // SAFETY: as above.
            let items: IEnumShellItems = unsafe { folder.BindToHandler(None, &BHID_EnumItems) }.unwrap();
            loop {
                let mut next = [None];
                let mut fetched = 0u32;
                // SAFETY: one slot and a live out pointer.
                let hr = unsafe { items.Next(&mut next, Some(&mut fetched)) };
                let Some(item) = next[0].take().filter(|_| hr.is_ok() && fetched == 1) else { break };
                let (Some(name), Some(id)) =
                    (name_of(&item, SIGDN_NORMALDISPLAY), name_of(&item, SIGDN_PARENTRELATIVEPARSING))
                else {
                    continue;
                };
                if !name.trim().is_empty()
                    && !id.is_empty()
                    && id.chars().count() <= 1024
                    && !id.chars().any(char::is_control)
                {
                    ids.insert(id);
                }
            }
        }
        if com.is_ok() {
            // SAFETY: balances the successful CoInitializeEx.
            unsafe { CoUninitialize() };
        }
        ids.len()
    })
    .join()
    .unwrap()
}

fn apps(v: &Value) -> Vec<Value> {
    match v.get("apps") {
        Some(Value::List(l)) => l.clone(),
        _ => panic!("apps is a list"),
    }
}

fn command(hub: &Hub, c: AppsCommand) {
    hub.port().send(HubRequest::Command { owner: InstanceId(1), command: ServiceCommand::Apps(c) });
}

#[test]
fn the_real_list_matches_a_direct_read_and_the_icons_stay_inside_their_bounds() {
    logs::install();
    // The service reads the Start menu in a child process: a test program cannot be that child.
    // SAFETY: set before any other thread of this test starts a service; the other test never reads it.
    unsafe { std::env::set_var("RSB_APPS_READER_EXE", env!("CARGO_BIN_EXE_apps_reader_host")) };
    let (hub, col) = hub_with(Hub::real_factory(), HubTiming::default());
    subscribe(&hub, ServiceId::Apps, 1, None);
    // The first value says `ready = false`; the list follows.
    let ready = col
        .wait(Duration::from_secs(60), |e| match e {
            HubEvent::Service(s) if s.service == ServiceId::Apps => {
                s.value.clone().filter(|v| v.get("ready").and_then(Value::as_bool) == Some(true))
            }
            _ => None,
        })
        .expect("a ready app list in 60 s");
    assert_eq!(check_value(ServiceId::Apps, &ready), Ok(()));
    let direct = direct_count();
    let listed = apps(&ready).len();
    println!("apps real: {listed} listed, {direct} counted directly");
    // The Start menu can change between the two reads.
    assert!(listed.abs_diff(direct) <= 3, "{listed} vs {direct}");

    // Let the icon extraction run, then look at what arrived.
    let t = Instant::now();
    let mut last = ready;
    while t.elapsed() < Duration::from_secs(8) {
        if let Some(HubEvent::Service(s)) = col.next(Duration::from_millis(200))
            && s.service == ServiceId::Apps
            && let Some(v) = s.value
        {
            last = v;
        }
    }
    assert_eq!(check_value(ServiceId::Apps, &last), Ok(()));
    let mut images = 0usize;
    let mut bytes = 0usize;
    for e in col.seen.lock().unwrap().iter() {
        if let HubEvent::Image { key, image, .. } = e {
            assert!(key.starts_with("rsb-image:apps/"), "icons use the service's own prefix");
            images += 1;
            bytes += image.bytes();
        }
    }
    let keyed = apps(&last)
        .iter()
        .filter(|a| matches!(a.get("icon"), Some(Value::Str(k)) if k.starts_with("rsb-image:apps/")))
        .count();
    println!("apps real: {keyed} apps with an icon key, {images} images ({bytes} bytes) sent");
    assert!(keyed <= 512 && images <= 512, "at most 512 icons");
    assert!(bytes <= 8 << 20, "at most 8 MiB");
    for a in apps(&last) {
        match a.get("icon") {
            Some(Value::Nil) | None => {}
            Some(Value::Str(k)) => assert!(k.starts_with("rsb-image:apps/")),
            other => panic!("icon is a key or nil, not {other:?}"),
        }
    }
    // Names never reach the log.
    for a in apps(&last).iter().take(200) {
        if let Some(Value::Str(name)) = a.get("name").filter(|n| matches!(n, Value::Str(s) if s.chars().count() >= 6)) {
            assert!(logs::all().iter().all(|l| !l.contains(name.as_str())), "a log line holds an app name");
        }
    }
    hub.shutdown(WAIT);
}

#[test]
fn the_fake_list_fits_the_schema_and_commands_are_recorded_only() {
    logs::install();
    let world = FakeWorld::with_sample_values();
    let (hub, col) = fake_hub(&world);
    subscribe(&hub, ServiceId::Apps, 1, None);
    let events = col.service_events(ServiceId::Apps, Duration::from_millis(150));
    let value = events.iter().rev().find_map(|e| e.value.clone()).expect("a value");
    assert_eq!(check_value(ServiceId::Apps, &value), Ok(()));
    assert_eq!(apps(&value).len(), 3);

    command(&hub, AppsCommand::Launch("sample.calculator".into()));
    command(&hub, AppsCommand::OpenUri("ms-settings:bluetooth".into()));
    let _ = col.drain(Duration::from_millis(150));
    let seen: Vec<&'static str> = world.commands().iter().map(ServiceCommand::kind).collect();
    assert_eq!(seen, ["launch", "open_uri"]);
    assert_eq!(world.starts(ServiceId::Apps), 1, "one fake service ran; nothing real was started");

    for line in logs::all() {
        for id in ["sample.calculator", "sample.editor", "sample.terminal"] {
            assert!(!line.contains(id), "a log line holds an app id");
        }
    }
    hub.shutdown(WAIT);
}

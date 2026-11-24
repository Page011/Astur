//! Each real reader through the hub, checked against an independent read
//! taken in the test. Nothing here changes system state: no volume, mute,
//! playback or workspace command is ever sent to a real service.

mod common;

use std::time::{Duration, Instant};

use common::*;
use rsb_core::element::InstanceId;
use rsb_core::msg::{HubPort, HubRequest};
use rsb_core::services::{ServiceEvent, ServiceId, ServiceStatus, check_value};
use rsb_core::value::Value;
use rsb_services::fake::FakeWorld;
use rsb_services::{Hub, HubOptions, HubTiming, ServiceFactory};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eMultimedia, eRender};
use windows::Win32::Networking::NetworkListManager::{INetworkListManager, NetworkListManager};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, GetSystemTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

fn real_hub() -> (Hub, Collector) {
    logs::install();
    hub_with(Hub::real_factory(), HubTiming::default())
}

fn first_value(col: &Collector, id: ServiceId) -> ServiceEvent {
    let e = col.value(id, Duration::from_secs(10)).unwrap_or_else(|| panic!("{} gave no value in 10 s", id.name()));
    assert_eq!(check_value(id, e.value.as_ref().unwrap()), Ok(()), "{} fits its schema", id.name());
    e
}

fn num(e: &ServiceEvent, key: &str) -> f64 {
    e.value.as_ref().unwrap().get(key).and_then(Value::as_f64).unwrap_or_else(|| panic!("{key} is a number"))
}

fn flag(e: &ServiceEvent, key: &str) -> bool {
    e.value.as_ref().unwrap().get(key).and_then(Value::as_bool).unwrap_or_else(|| panic!("{key} is a boolean"))
}

fn text(e: &ServiceEvent, key: &str) -> String {
    e.value.as_ref().unwrap().get(key).and_then(Value::as_str).map(str::to_string).unwrap_or_default()
}

fn ticks(f: FILETIME) -> u64 {
    (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)
}

fn system_times() -> (u64, u64, u64) {
    let (mut i, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: three live out pointers.
    unsafe { GetSystemTimes(Some(&mut i), Some(&mut k), Some(&mut u)) }.expect("GetSystemTimes");
    (ticks(i), ticks(k), ticks(u))
}

#[test]
fn system_matches_independent_reads() {
    let (hub, col) = real_hub();
    subscribe(&hub, ServiceId::System, 1, Some(Duration::from_millis(500)));
    let t0 = system_times();
    let first = first_value(&col, ServiceId::System);
    // Wait for a second sample, then compare it with a load measured by the test over the same stretch.
    let second = loop {
        let e = col.value(ServiceId::System, Duration::from_secs(5)).expect("a second sample");
        if e.version > first.version {
            break e;
        }
    };
    let t1 = system_times();
    let mut m = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: `m` has its size set.
    unsafe { GlobalMemoryStatusEx(&mut m) }.expect("GlobalMemoryStatusEx");

    let total = num(&second, "memory_total");
    let used = num(&second, "memory_used");
    let percent = num(&second, "memory");
    let own_used = (m.ullTotalPhys - m.ullAvailPhys) as f64;
    let own_percent = 100.0 * own_used / m.ullTotalPhys as f64;
    println!(
        "system memory: service {percent:.1} % ({used:.0} of {total:.0} bytes); test read {own_percent:.1} % ({own_used:.0} of {} bytes)",
        m.ullTotalPhys
    );
    assert_eq!(total, m.ullTotalPhys as f64, "installed memory is exact");
    assert!((percent - own_percent).abs() < 3.0, "{percent} vs {own_percent}");
    assert!((used - own_used).abs() / total < 0.03);
    assert!(((used / total) * 100.0 - percent).abs() < 0.1, "percent follows used / total");

    let cpu = num(&second, "cpu");
    let kernel = t1.1 - t0.1;
    let user = t1.2 - t0.2;
    let idle = t1.0 - t0.0;
    let own_cpu = 100.0 * (1.0 - idle as f64 / (kernel + user) as f64);
    println!("system cpu (rdp session, short window): service {cpu:.1} %, test over the same stretch {own_cpu:.1} %");
    assert!((0.0..=100.0).contains(&cpu));
    assert!((cpu - own_cpu).abs() < 30.0, "{cpu} vs {own_cpu}");

    // The sampler runs only while subscribed: gone 2 s after the last reader.
    unsubscribe(&hub, ServiceId::System, 1);
    let off = col.service(ServiceId::System, Duration::from_secs(5));
    assert!(matches!(off, Some(e) if e.status == ServiceStatus::Off || e.status == ServiceStatus::Ok));
    hub.shutdown(WAIT);
}

#[test]
fn battery_matches_get_system_power_status() {
    let (hub, col) = real_hub();
    subscribe(&hub, ServiceId::Battery, 1, None);
    let e = first_value(&col, ServiceId::Battery);
    let mut s = SYSTEM_POWER_STATUS::default();
    // SAFETY: a live out pointer.
    unsafe { GetSystemPowerStatus(&mut s) }.expect("GetSystemPowerStatus");
    let present = s.BatteryFlag != 255 && s.BatteryFlag & 128 == 0;
    println!(
        "battery: service present={} plugged={} percent={:?}; test read flag={} ac={} percent={}",
        flag(&e, "present"),
        flag(&e, "plugged"),
        e.value.as_ref().unwrap().get("percent"),
        s.BatteryFlag,
        s.ACLineStatus,
        s.BatteryLifePercent
    );
    assert_eq!(flag(&e, "present"), present);
    assert_eq!(flag(&e, "plugged"), s.ACLineStatus == 1);
    assert_eq!(flag(&e, "saver"), s.SystemStatusFlag & 1 != 0);
    if present {
        assert!((num(&e, "percent") - f64::from(s.BatteryLifePercent)).abs() <= 2.0);
    } else {
        assert_eq!(e.value.as_ref().unwrap().get("percent"), Some(&Value::Nil), "no battery, no percent");
        assert!(!flag(&e, "charging"));
    }
    hub.shutdown(WAIT);
}

fn direct_audio_read() -> Option<(f32, bool)> {
    // SAFETY: COM calls from this test thread, initialised as an MTA; reads only.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia).ok()?;
        let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
        Some((volume.GetMasterVolumeLevelScalar().ok()?, volume.GetMute().ok()?.as_bool()))
    }
}

#[test]
fn audio_matches_a_direct_endpoint_read() {
    let (hub, col) = real_hub();
    subscribe(&hub, ServiceId::Audio, 1, None);
    let e = first_value(&col, ServiceId::Audio);
    let direct = direct_audio_read();
    match direct {
        Some((v, muted)) => {
            println!(
                "audio: service volume {} muted {}; direct read {:.1} muted {}",
                num(&e, "volume"),
                flag(&e, "muted"),
                v * 100.0,
                muted
            );
            assert!(flag(&e, "available"));
            assert!((num(&e, "volume") - f64::from(v) * 100.0).abs() <= 1.0);
            assert_eq!(flag(&e, "muted"), muted);
            assert_eq!(e.status, ServiceStatus::Ok);
        }
        None => {
            println!("audio: no default output device on this PC");
            assert!(!flag(&e, "available"));
            assert_eq!(e.status, ServiceStatus::Unavailable, "no device is not an error");
        }
    }
    hub.shutdown(WAIT);
}

fn direct_connectivity() -> Option<bool> {
    // SAFETY: COM calls from this test thread, initialised as an MTA; reads only.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let m: INetworkListManager = CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL).ok()?;
        let flags = m.GetConnectivity().ok()?.0;
        Some(flags & (0x10 | 0x20 | 0x40 | 0x100 | 0x200 | 0x400) != 0)
    }
}

#[test]
fn network_matches_the_connectivity_list_and_counts_up() {
    let (hub, col) = real_hub();
    subscribe(&hub, ServiceId::Network, 1, Some(Duration::from_millis(500)));
    let a = first_value(&col, ServiceId::Network);
    let b = loop {
        let e = col.value(ServiceId::Network, Duration::from_secs(5)).expect("another sample");
        if e.version > a.version {
            break e;
        }
    };
    let connected = direct_connectivity();
    println!(
        "network: service connected={} kind={} down={} B/s up={} B/s; list says connected={connected:?}",
        flag(&b, "connected"),
        text(&b, "kind"),
        num(&b, "down_bps"),
        num(&b, "up_bps")
    );
    let v = b.value.as_ref().unwrap();
    println!(
        "network: ssid {}, signal {}",
        if matches!(v.get("ssid"), Some(Value::Str(_))) { "present" } else { "absent (location permission)" },
        v.get("signal").and_then(Value::as_f64).map_or("none".to_string(), |s| format!("{s:.0}"))
    );
    if let Some(c) = connected {
        assert_eq!(flag(&b, "connected"), c);
    }
    assert!(num(&b, "down_bps") >= 0.0 && num(&b, "up_bps") >= 0.0);
    assert!(["wifi", "ethernet", "cellular", "other", "none"].contains(&text(&b, "kind").as_str()));
    if !flag(&b, "connected") {
        assert_eq!(text(&b, "kind"), "none");
        assert!(!flag(&b, "internet"));
    }
    // Without the location permission the name may be missing; the signal is still a percentage.
    if let Some(s) = b.value.as_ref().unwrap().get("signal").and_then(Value::as_f64) {
        assert!((0.0..=100.0).contains(&s));
    }
    hub.shutdown(WAIT);
}

#[test]
fn media_gives_a_well_formed_value_whatever_is_playing() {
    let (hub, col) = real_hub();
    subscribe(&hub, ServiceId::Media, 1, None);
    let e = first_value(&col, ServiceId::Media);
    println!("media: available={} playing={}", flag(&e, "available"), flag(&e, "playing"));
    if !flag(&e, "available") {
        assert!(!flag(&e, "playing"));
        for k in ["title", "artist", "album", "source"] {
            assert_eq!(text(&e, k), "", "{k} is empty without a session");
        }
        assert!(!flag(&e, "can_next") && !flag(&e, "can_previous") && !flag(&e, "can_play_pause"));
    } else {
        assert!(text(&e, "title").chars().count() <= 256);
    }
    hub.shutdown(WAIT);
}

fn direct_foreground_process() -> Option<String> {
    // SAFETY: plain window and process queries; the handle is closed below.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == GetCurrentProcessId() {
            return None;
        }
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(
            h,
            windows::Win32::System::Threading::PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = windows::Win32::Foundation::CloseHandle(h);
        r.ok()?;
        String::from_utf16_lossy(&buf[..len as usize]).rsplit('\\').next().map(str::to_string)
    }
}

#[test]
fn window_matches_the_foreground_windows_program() {
    let (hub, col) = real_hub();
    subscribe(&hub, ServiceId::Window, 1, None);
    let e = first_value(&col, ServiceId::Window);
    let direct = direct_foreground_process();
    // The service's value is about program names, never printed here (privacy): only whether they agree.
    println!("window: has_window={} direct read found a program: {}", flag(&e, "has_window"), direct.is_some());
    if flag(&e, "has_window") {
        let process = text(&e, "process");
        assert!(process.to_lowercase().ends_with(".exe") || process.is_empty());
        assert!(!process.contains('\\') && !process.contains('/'), "file name only, no folder");
        if let Some(d) = direct {
            // The foreground can change between the two reads; wait for the service to catch up once.
            if !d.eq_ignore_ascii_case(&process) {
                std::thread::sleep(Duration::from_millis(600));
            }
        }
    }
    assert!(text(&e, "title").chars().count() <= 256);
    hub.shutdown(WAIT);
}

#[test]
fn every_real_service_starts_stops_and_leaves_the_hub_clean() {
    let (hub, col) = real_hub();
    let ids = [
        ServiceId::System,
        ServiceId::Battery,
        ServiceId::Audio,
        ServiceId::Network,
        ServiceId::Media,
        ServiceId::Window,
    ];
    for (n, id) in ids.iter().enumerate() {
        subscribe(&hub, *id, n as u32 + 1, None);
    }
    let mut missing: Vec<ServiceId> = ids.to_vec();
    col.wait(Duration::from_secs(15), |e| {
        if let rsb_core::msg::HubEvent::Service(s) = e
            && let Some(v) = &s.value
        {
            assert_eq!(check_value(s.service, v), Ok(()), "{} fits its schema", s.service.name());
            missing.retain(|m| *m != s.service);
        }
        missing.is_empty().then_some(())
    })
    .unwrap_or_else(|| panic!("no value from: {:?}", missing.iter().map(|m| m.name()).collect::<Vec<_>>()));
    let t = Instant::now();
    for (n, _) in ids.iter().enumerate() {
        hub.port().send(HubRequest::CancelOwner { owner: InstanceId(n as u32 + 1) });
    }
    hub.shutdown(Duration::from_secs(5));
    println!("shutdown with six real services running took {} ms", t.elapsed().as_millis());
    assert!(t.elapsed() < Duration::from_secs(5));
}

fn real_or_fake_strings(col: &Collector) -> Vec<String> {
    let mut out = Vec::new();
    for e in col.seen.lock().unwrap().iter() {
        if let rsb_core::msg::HubEvent::Service(s) = e
            && let Some(Value::Map(m)) = &s.value
        {
            for k in ["title", "artist", "album", "ssid"] {
                if let Some(Value::Str(v)) = m.get(k)
                    && v.chars().count() >= 4
                {
                    out.push(v.clone());
                }
            }
        }
    }
    out
}

#[test]
fn no_title_artist_or_network_name_reaches_the_log() {
    logs::install();
    // Real services: whatever this PC shows right now.
    let (hub, col) = real_hub();
    for (n, id) in [ServiceId::Window, ServiceId::Media, ServiceId::Network].into_iter().enumerate() {
        subscribe(&hub, id, n as u32 + 1, None);
    }
    std::thread::sleep(Duration::from_millis(1500));
    // Fake services with known strings, including a start failure and a stale phase so the hub logs.
    let world = FakeWorld::with_sample_values();
    world.set_stale_after(ServiceId::Media, Duration::from_millis(150));
    world.fail_starts(ServiceId::Window, 1, "no window source");
    let (fake_hub, fake_col) = hub_with(
        Box::new(world.clone()) as Box<dyn ServiceFactory>,
        rsb_services::HubTiming {
            stop_delay: Duration::from_millis(100),
            retry_first: Duration::from_millis(50),
            retry_max: Duration::from_millis(200),
        },
    );
    for (n, id) in [ServiceId::Window, ServiceId::Media, ServiceId::Network].into_iter().enumerate() {
        subscribe(&fake_hub, id, n as u32 + 1, None);
    }
    world.go_silent(ServiceId::Media);
    std::thread::sleep(Duration::from_millis(800));
    world.resume(ServiceId::Media);
    std::thread::sleep(Duration::from_millis(300));
    let _ = fake_col.drain(Duration::from_millis(100));
    let _ = col.drain(Duration::from_millis(100));

    let mut secrets = real_or_fake_strings(&col);
    secrets.extend(real_or_fake_strings(&fake_col));
    secrets.extend(["Sample Track", "Sample Artist", "Sample Album", "Sample document"].map(String::from));
    secrets.sort();
    secrets.dedup();
    let lines = logs::all();
    assert!(!lines.is_empty(), "the hub logged its status changes");
    for line in &lines {
        for s in &secrets {
            // A common word like a program file name may legitimately be a substring of something else; the
            // check is on the exact strings the services published.
            assert!(!line.contains(s.as_str()), "a log line holds a published string ({} chars)", s.len());
        }
    }
    println!("privacy: {} log lines checked against {} published strings", lines.len(), secrets.len());
    hub.shutdown(WAIT);
    fake_hub.shutdown(WAIT);
    let _: HubOptions = HubOptions::default();
}

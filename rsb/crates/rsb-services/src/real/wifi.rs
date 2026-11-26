//! Wi-Fi networks in range (WLAN API scan list; connect and disconnect use saved profiles).
//!
//! Windows shares network lists with desktop apps only when location access is on, and shows a
//! one-time system prompt the first time an app is blocked. So the access is checked first
//! (`AppCapability("wiFiControl").CheckAccess()`), and without it no list, scan, connect or
//! disconnect call is made at all: the value has no networks and `access = "needs_location"`. The
//! check is repeated when Windows reports a change and on every re-read.

mod api;

use std::collections::{HashSet, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rsb_core::services::{ServiceCommand, ServiceId, ServiceStatus, WifiCommand};
use rsb_core::value::Value;
use windows::Foundation::TypedEventHandler;
use windows::Security::Authorization::AppCapabilityAccess::{AppCapability, AppCapabilityAccessStatus};
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::NetworkManagement::WiFi::{
    L2_NOTIFICATION_DATA, WLAN_NOTIFICATION_SOURCE_ACM, wlan_notification_acm_scan_complete,
    wlan_notification_acm_scan_fail,
};
use windows::Win32::System::Threading::{SetEvent, WaitForMultipleObjects};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::core::{GUID, HSTRING};

use self::api::{Merged, SystemWlan, WlanApi, merge};
use super::Worker;
use crate::util::{map, text};
use crate::win::{Owned, auto_event, signal};
use crate::{Publisher, Service};

const REREAD: Duration = Duration::from_secs(60);
/// A scan that never reports back stops showing as running after this long.
const SCAN_DEADLINE: Duration = Duration::from_secs(6);
/// Burst of notifications (scan results arrive as several) settle before a re-read.
const SETTLE: Duration = Duration::from_millis(100);
/// Longest SSID, in bytes.
const MAX_SSID_BYTES: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Access {
    Allowed,
    NeedsLocation,
}

impl Access {
    fn name(self) -> &'static str {
        match self {
            Access::Allowed => "allowed",
            Access::NeedsLocation => "needs_location",
        }
    }
}

/// `AppCapability::CheckAccess` for `wiFiControl`; anything but `Allowed` (including a Windows
/// that does not know the capability) reads as "needs location": no call that could prompt.
fn check_access(capability: Option<&AppCapability>) -> Access {
    match capability.map(AppCapability::CheckAccess) {
        Some(Ok(AppCapabilityAccessStatus::Allowed)) => Access::Allowed,
        _ => Access::NeedsLocation,
    }
}

/// Everything one read learned.
pub(crate) struct Snapshot {
    pub interfaces: Vec<GUID>,
    pub access: Access,
    pub networks: Vec<Merged>,
}

/// Reads the adapters and, only with access, their scan lists. The adapter count is read in every
/// case; nothing that can raise the system prompt is called without `Access::Allowed`.
pub(crate) fn read(api: &mut dyn WlanApi, access: Access) -> Result<Snapshot, String> {
    let interfaces = api.interfaces()?;
    let mut networks = Vec::new();
    if access == Access::Allowed {
        let mut found = Vec::new();
        for guid in &interfaces {
            // One adapter failing (radio off) leaves the others' networks in the list.
            if let Ok(list) = api.list(guid) {
                found.extend(list.into_iter().map(|n| (*guid, n)));
            }
        }
        networks = merge(found);
    }
    Ok(Snapshot { interfaces, access, networks })
}

fn need_access(access: Access) -> Result<(), String> {
    if access == Access::Allowed { Ok(()) } else { Err("location access is needed for Wi-Fi".into()) }
}

pub(crate) fn start_scan(api: &mut dyn WlanApi, snapshot: &Snapshot) -> Result<(), String> {
    need_access(snapshot.access)?;
    let mut last = Ok(());
    let mut any = false;
    for guid in &snapshot.interfaces {
        match api.scan(guid) {
            Ok(()) => any = true,
            Err(e) => last = Err(e),
        }
    }
    if any { Ok(()) } else { last }
}

pub(crate) fn connect(api: &mut dyn WlanApi, snapshot: &Snapshot, ssid: &str) -> Result<(), String> {
    need_access(snapshot.access)?;
    let net = snapshot.networks.iter().find(|n| n.ssid == ssid && n.known).ok_or("no saved network with that name")?;
    let profile = net.profile.as_deref().ok_or("no saved network with that name")?;
    api.connect(&net.interface, profile)
}

pub(crate) fn disconnect(api: &mut dyn WlanApi, snapshot: &Snapshot) -> Result<(), String> {
    need_access(snapshot.access)?;
    let mut last = Ok(());
    for guid in snapshot.networks.iter().filter(|n| n.connected).map(|n| n.interface) {
        if let Err(e) = api.disconnect(&guid) {
            last = Err(e);
        }
    }
    last
}

fn value_of(snapshot: &Snapshot, scanning: bool) -> Value {
    let networks = snapshot
        .networks
        .iter()
        .map(|n| {
            map([
                ("ssid", Value::Str(n.ssid.clone())),
                ("signal", Value::Num(f64::from(n.signal))),
                ("secure", Value::Bool(n.secure)),
                ("connected", Value::Bool(n.connected)),
                ("known", Value::Bool(n.known)),
            ])
        })
        .collect();
    map([
        ("available", Value::Bool(!snapshot.interfaces.is_empty())),
        ("scanning", Value::Bool(scanning && snapshot.access == Access::Allowed)),
        ("networks", Value::List(networks)),
        ("access", text(snapshot.access.name())),
    ])
}

/// The command checks that need no adapter: run on the hub thread, so a refusal never waits and
/// never reaches the WLAN API. The errors name nothing.
pub(crate) fn check_command(command: &WifiCommand, allowed: bool, known: &HashSet<String>) -> Result<(), String> {
    match command {
        WifiCommand::Scan | WifiCommand::Disconnect if !allowed => Err("location access is needed for Wi-Fi".into()),
        WifiCommand::Scan | WifiCommand::Disconnect => Ok(()),
        WifiCommand::Connect(ssid) => {
            if ssid.len() > MAX_SSID_BYTES || !known.contains(ssid) {
                Err("no saved network with that name".into())
            } else {
                Ok(())
            }
        }
    }
}

struct Shared {
    wake: Owned,
    commands: Mutex<VecDeque<WifiCommand>>,
    /// Names the last read published with `known = true`, for `check_command`.
    known: Mutex<HashSet<String>>,
    allowed: AtomicBool,
    scan_done: AtomicBool,
}

fn wake(s: &Shared) {
    // SAFETY: `wake` is a live event owned by the shared state.
    let _ = unsafe { SetEvent(s.wake.get()) };
}

unsafe extern "system" fn on_wlan(data: *mut L2_NOTIFICATION_DATA, context: *mut c_void) {
    // SAFETY: `context` is the pointer from Arc::into_raw in `run`, valid until the WLAN handle is closed.
    let shared = unsafe { &*context.cast_const().cast::<Shared>() };
    // SAFETY: the API hands a valid notification for the duration of the callback.
    if let Some(d) = unsafe { data.as_ref() }
        && d.NotificationSource == WLAN_NOTIFICATION_SOURCE_ACM
        && (d.NotificationCode == wlan_notification_acm_scan_complete.0 as u32
            || d.NotificationCode == wlan_notification_acm_scan_fail.0 as u32)
    {
        shared.scan_done.store(true, Ordering::Release);
    }
    wake(shared);
}

fn run(out: Publisher, shared: Arc<Shared>, stop: Arc<Owned>) {
    // SAFETY: initialises the WinRT apartment of this thread as MTA; balanced at the end.
    let ro = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    let capability = AppCapability::Create(&HSTRING::from("wiFiControl")).ok();
    let s = shared.clone();
    let token = capability.as_ref().and_then(|c| {
        c.AccessChanged(&TypedEventHandler::new(move |_, _| {
            wake(&s);
            Ok(())
        }))
        .ok()
    });

    let context = Arc::into_raw(shared.clone());
    let mut wlan: Option<SystemWlan> = None;
    let mut scanning_until: Option<Instant> = None;
    let mut snapshot: Option<Snapshot> = None;
    loop {
        crate::count_source_wake();
        let access = check_access(capability.as_ref());
        shared.allowed.store(access == Access::Allowed, Ordering::Release);
        if wlan.is_none() {
            wlan = SystemWlan::open(on_wlan, context.cast());
        }

        let pending: Vec<WifiCommand> = shared.commands.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
        if let (Some(api), false) = (wlan.as_mut(), pending.is_empty()) {
            // The commands were checked on the hub thread; the list they refer to is read afresh here.
            let current = read(api, access);
            for c in pending {
                let result = match (&c, &current) {
                    (_, Err(e)) => Err(e.clone()),
                    (WifiCommand::Scan, Ok(s)) => start_scan(api, s).inspect(|()| {
                        scanning_until = Some(Instant::now() + SCAN_DEADLINE);
                    }),
                    (WifiCommand::Connect(ssid), Ok(s)) => connect(api, s, ssid),
                    (WifiCommand::Disconnect, Ok(s)) => disconnect(api, s),
                };
                if let Err(e) = result {
                    log::warn!("wifi command failed: {e}");
                }
            }
        }
        if shared.scan_done.swap(false, Ordering::AcqRel) || scanning_until.is_some_and(|t| Instant::now() >= t) {
            scanning_until = None;
        }

        match wlan.as_mut().map(|api| read(api, access)) {
            Some(Ok(s)) => {
                *shared.known.lock().unwrap_or_else(|e| e.into_inner()) =
                    s.networks.iter().filter(|n| n.known).map(|n| n.ssid.clone()).collect();
                let value = value_of(&s, scanning_until.is_some());
                if s.interfaces.is_empty() {
                    out.publish_as(value, ServiceStatus::Unavailable, Some("no Wi-Fi adapter".into()));
                } else {
                    out.publish(value);
                }
                snapshot = Some(s);
            }
            Some(Err(e)) => {
                log::debug!("wifi: the adapters could not be read: {e}");
                wlan = None;
                out.status(ServiceStatus::Error, Some("the Wi-Fi adapters could not be read".into()));
            }
            None => {
                let none = Snapshot { interfaces: Vec::new(), access, networks: Vec::new() };
                out.publish_as(value_of(&none, false), ServiceStatus::Unavailable, Some("no Wi-Fi service".into()));
            }
        }

        let timeout = scanning_until
            .map_or(REREAD, |t| t.saturating_duration_since(Instant::now()).max(Duration::from_millis(50)))
            .min(REREAD);
        // SAFETY: both handles are live events.
        let waited =
            unsafe { WaitForMultipleObjects(&[stop.get(), shared.wake.get()], false, timeout.as_millis() as u32) };
        if waited == WAIT_OBJECT_0 {
            break;
        }
        std::thread::sleep(SETTLE);
    }
    drop(snapshot);
    drop(wlan);
    // SAFETY: balances Arc::into_raw above; the WLAN handle is closed, so no callback can run.
    drop(unsafe { Arc::from_raw(context) });
    if let (Some(c), Some(t)) = (&capability, token) {
        let _ = c.RemoveAccessChanged(t);
    }
    drop(capability);
    if ro.is_ok() {
        // SAFETY: balances RoInitialize.
        unsafe { RoUninitialize() };
    }
}

pub(crate) struct WifiService {
    shared: Option<Arc<Shared>>,
    worker: Option<Worker>,
}

impl WifiService {
    pub(crate) fn new() -> WifiService {
        WifiService { shared: None, worker: None }
    }
}

impl Service for WifiService {
    fn id(&self) -> ServiceId {
        ServiceId::Wifi
    }

    fn start(&mut self, out: Publisher) -> Result<(), String> {
        let shared = Arc::new(Shared {
            wake: auto_event()?,
            commands: Mutex::new(VecDeque::new()),
            known: Mutex::new(HashSet::new()),
            allowed: AtomicBool::new(false),
            scan_done: AtomicBool::new(false),
        });
        let thread_shared = shared.clone();
        self.worker = Some(Worker::spawn("rsb-wifi", move |stop| run(out, thread_shared, stop))?);
        self.shared = Some(shared);
        Ok(())
    }

    fn command(&mut self, command: ServiceCommand) -> Result<(), String> {
        let (ServiceCommand::Wifi(c), Some(shared)) = (command, &self.shared) else {
            return Err("not a Wi-Fi command, or the service is stopped".into());
        };
        let known = shared.known.lock().unwrap_or_else(|e| e.into_inner());
        check_command(&c, shared.allowed.load(Ordering::Acquire), &known)?;
        drop(known);
        shared.commands.lock().unwrap_or_else(|e| e.into_inner()).push_back(c);
        signal(&shared.wake);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
        self.shared = None;
    }

    fn stale_after(&self) -> Duration {
        Duration::from_secs(180)
    }
}

#[cfg(test)]
mod tests {
    use rsb_core::services::check_value;

    use super::api::RawNet;
    use super::*;

    /// Counts every call; the list and the scan return made-up networks.
    #[derive(Default)]
    struct Counting {
        interfaces: u32,
        list: u32,
        scan: u32,
        connect: Vec<String>,
        disconnect: u32,
    }

    impl WlanApi for Counting {
        fn interfaces(&mut self) -> Result<Vec<GUID>, String> {
            self.interfaces += 1;
            Ok(vec![GUID::from_u128(7)])
        }
        fn list(&mut self, _: &GUID) -> Result<Vec<RawNet>, String> {
            self.list += 1;
            Ok(vec![
                RawNet { ssid: "alpha".into(), signal: 80, secure: true, connected: true, profile: Some("p1".into()) },
                RawNet { ssid: "beta".into(), signal: 30, secure: false, connected: false, profile: None },
            ])
        }
        fn scan(&mut self, _: &GUID) -> Result<(), String> {
            self.scan += 1;
            Ok(())
        }
        fn connect(&mut self, _: &GUID, profile: &str) -> Result<(), String> {
            self.connect.push(profile.to_string());
            Ok(())
        }
        fn disconnect(&mut self, _: &GUID) -> Result<(), String> {
            self.disconnect += 1;
            Ok(())
        }
    }

    #[test]
    fn without_location_access_no_list_scan_connect_or_disconnect_call_is_made() {
        let mut api = Counting::default();
        let snapshot = read(&mut api, Access::NeedsLocation).unwrap();
        assert!(snapshot.networks.is_empty());
        assert!(start_scan(&mut api, &snapshot).is_err());
        assert!(connect(&mut api, &snapshot, "alpha").is_err());
        assert!(disconnect(&mut api, &snapshot).is_err());
        assert_eq!((api.list, api.scan, api.connect.len(), api.disconnect), (0, 0, 0, 0));
        assert_eq!(api.interfaces, 1, "only the adapter count is read");
        let value = value_of(&snapshot, true);
        assert_eq!(check_value(ServiceId::Wifi, &value), Ok(()));
        assert_eq!(value.get("access").and_then(Value::as_str), Some("needs_location"));
        assert_eq!(value.get("scanning").and_then(Value::as_bool), Some(false), "no scan runs without access");
    }

    #[test]
    fn with_access_the_list_is_read_and_only_saved_networks_connect() {
        let mut api = Counting::default();
        let snapshot = read(&mut api, Access::Allowed).unwrap();
        assert_eq!(api.list, 1);
        assert_eq!(snapshot.networks.len(), 2);
        assert!(connect(&mut api, &snapshot, "beta").is_err(), "no saved profile");
        assert!(connect(&mut api, &snapshot, "gamma").is_err(), "not in range");
        assert!(api.connect.is_empty());
        connect(&mut api, &snapshot, "alpha").unwrap();
        assert_eq!(api.connect, vec!["p1".to_string()]);
        start_scan(&mut api, &snapshot).unwrap();
        disconnect(&mut api, &snapshot).unwrap();
        assert_eq!((api.scan, api.disconnect), (1, 1));
        let value = value_of(&snapshot, true);
        assert_eq!(check_value(ServiceId::Wifi, &value), Ok(()));
        assert_eq!(value.get("access").and_then(Value::as_str), Some("allowed"));
    }

    #[test]
    fn commands_are_checked_against_the_published_names_before_anything_is_queued() {
        let known: HashSet<String> = ["alpha".to_string()].into();
        assert!(check_command(&WifiCommand::Connect("alpha".into()), true, &known).is_ok());
        assert!(check_command(&WifiCommand::Connect("beta".into()), true, &known).is_err());
        assert!(check_command(&WifiCommand::Connect("a".repeat(40)), true, &known).is_err());
        assert!(check_command(&WifiCommand::Scan, false, &known).is_err());
        assert!(check_command(&WifiCommand::Disconnect, false, &known).is_err());
        assert!(check_command(&WifiCommand::Scan, true, &known).is_ok());
        let e = check_command(&WifiCommand::Connect("secret-name".into()), true, &known).unwrap_err();
        assert!(!e.contains("secret-name"), "errors never carry names");
    }
}

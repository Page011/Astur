//! Connection state and throughput.
//!
//! One MTA thread owns the sources: interface byte counters (`GetIfTable2`,
//! sampled at the subscriber's interval), connectivity events
//! (`INetworkListManager`), interface changes (`NotifyIpInterfaceChange`) and
//! the WLAN API for the Wi-Fi name and signal. Every callback only wakes the
//! thread, which then re-reads.
//!
//! Wi-Fi name: Windows 11 24H2 and later may refuse it to desktop apps that
//! lack the location permission; RSB never asks for it. Then `ssid` is nil
//! and `signal` comes from the radio's RSSI instead.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rsb_core::services::{ServiceCommand, ServiceId};
use rsb_core::value::Value;
use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE, WAIT_OBJECT_0};
use windows::Win32::NetworkManagement::IpHelper::{
    CancelMibChangeNotify2, FreeMibTable, GetBestInterface, GetIfTable2, MIB_IF_TABLE2, MIB_IPINTERFACE_ROW,
    MIB_NOTIFICATION_TYPE, NotifyIpInterfaceChange,
};
use windows::Win32::NetworkManagement::WiFi::{
    L2_NOTIFICATION_DATA, WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO_LIST, WLAN_NOTIFICATION_SOURCE_ACM,
    WlanCloseHandle, WlanEnumInterfaces, WlanFreeMemory, WlanOpenHandle, WlanQueryInterface, WlanRegisterNotification,
    wlan_interface_state_connected, wlan_intf_opcode_current_connection, wlan_intf_opcode_rssi,
};
use windows::Win32::Networking::NetworkListManager::{
    INetworkListManager, INetworkListManagerEvents, INetworkListManagerEvents_Impl, NLM_CONNECTIVITY,
    NetworkListManager,
};
use windows::Win32::Networking::WinSock::AF_UNSPEC;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, IConnectionPointContainer,
};
use windows::Win32::System::Threading::{SetEvent, WaitForMultipleObjects};
use windows::core::{GUID, Interface, implement};

use super::Worker;
use crate::util::{map, text};
use crate::win::{Owned, auto_event};
use crate::{DEFAULT_SAMPLE_INTERVAL, Publisher, Service};

// Interface types (ifdef.h).
const IF_TYPE_ETHERNET: u32 = 6;
const IF_TYPE_WIFI: u32 = 71;
const IF_TYPE_WWANPP: u32 = 243;
const IF_TYPE_WWANPP2: u32 = 244;
const IF_OPER_UP: i32 = 1;

// NLM_CONNECTIVITY bits.
const NLM_IPV4_SUBNET: i32 = 0x10;
const NLM_IPV4_LOCAL: i32 = 0x20;
const NLM_IPV4_INTERNET: i32 = 0x40;
const NLM_IPV6_SUBNET: i32 = 0x100;
const NLM_IPV6_LOCAL: i32 = 0x200;
const NLM_IPV6_INTERNET: i32 = 0x400;

struct Shared {
    wake: Owned,
    interval_ms: AtomicU64,
}

fn wake(s: &Shared) {
    // SAFETY: `wake` is a live event owned by the shared state.
    let _ = unsafe { SetEvent(s.wake.get()) };
}

#[implement(INetworkListManagerEvents)]
struct ConnectivityEvents {
    shared: Arc<Shared>,
}

impl INetworkListManagerEvents_Impl for ConnectivityEvents_Impl {
    fn ConnectivityChanged(&self, _new: NLM_CONNECTIVITY) -> windows::core::Result<()> {
        wake(&self.shared);
        Ok(())
    }
}

unsafe extern "system" fn on_interface_change(
    context: *const c_void,
    _row: *const MIB_IPINTERFACE_ROW,
    _kind: MIB_NOTIFICATION_TYPE,
) {
    // SAFETY: `context` is the pointer from Arc::into_raw in `run`, valid until the registration is cancelled.
    let shared = unsafe { &*context.cast::<Shared>() };
    wake(shared);
}

unsafe extern "system" fn on_wlan(_data: *mut L2_NOTIFICATION_DATA, context: *mut c_void) {
    // SAFETY: `context` is the pointer from Arc::into_raw in `run`, valid until the WLAN handle is closed.
    let shared = unsafe { &*context.cast_const().cast::<Shared>() };
    wake(shared);
}

/// One interface as far as the service cares.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Link {
    pub index: u32,
    pub kind: u32,
    pub up: bool,
    pub hardware: bool,
    pub in_octets: u64,
    pub out_octets: u64,
}

fn read_links() -> Vec<Link> {
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: a live out pointer; the table is freed below.
    if unsafe { GetIfTable2(&mut table) } != ERROR_SUCCESS || table.is_null() {
        return Vec::new();
    }
    // SAFETY: GetIfTable2 returned a table of `NumEntries` rows starting at `Table`.
    let rows = unsafe { std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize) };
    let links = rows
        .iter()
        .map(|r| Link {
            index: r.InterfaceIndex,
            kind: r.Type,
            up: r.OperStatus.0 == IF_OPER_UP,
            // Bit 0 of the flags is HardwareInterface: loopback and virtual adapters are excluded.
            hardware: r.InterfaceAndOperStatusFlags._bitfield & 1 != 0,
            in_octets: r.InOctets,
            out_octets: r.OutOctets,
        })
        .collect();
    // SAFETY: `table` came from GetIfTable2 and is freed once.
    unsafe { FreeMibTable(table.cast()) };
    links
}

/// Bytes received and sent by all hardware interfaces that are up.
pub(crate) fn totals(links: &[Link]) -> (u64, u64) {
    links.iter().filter(|l| l.hardware && l.up).fold((0, 0), |(i, o), l| (i + l.in_octets, o + l.out_octets))
}

/// "wifi", "ethernet", "cellular" or "other" for the interface that carries
/// the default route, else the first hardware interface that is up.
pub(crate) fn kind_of(links: &[Link]) -> &'static str {
    let mut best = 0u32;
    // SAFETY: a plain address (8.8.8.8, a route lookup only) and a live out pointer; nothing is sent.
    let routed = unsafe { GetBestInterface(u32::from_ne_bytes([8, 8, 8, 8]), &mut best) } == 0;
    let chosen =
        links.iter().find(|l| routed && l.index == best && l.up).or_else(|| links.iter().find(|l| l.hardware && l.up));
    match chosen.map(|l| l.kind) {
        Some(IF_TYPE_WIFI) => "wifi",
        Some(IF_TYPE_ETHERNET) => "ethernet",
        Some(IF_TYPE_WWANPP | IF_TYPE_WWANPP2) => "cellular",
        Some(_) => "other",
        None => "none",
    }
}

/// (connected, internet) from NLM connectivity flags.
pub(crate) fn connectivity(flags: i32) -> (bool, bool) {
    let any =
        NLM_IPV4_SUBNET | NLM_IPV4_LOCAL | NLM_IPV4_INTERNET | NLM_IPV6_SUBNET | NLM_IPV6_LOCAL | NLM_IPV6_INTERNET;
    (flags & any != 0, flags & (NLM_IPV4_INTERNET | NLM_IPV6_INTERNET) != 0)
}

/// The WLAN client handle.
struct Wlan {
    handle: HANDLE,
}

impl Wlan {
    fn open(shared: &Arc<Shared>) -> Option<(Wlan, *const Shared)> {
        let mut negotiated = 0u32;
        let mut handle = HANDLE::default();
        // SAFETY: live out pointers; version 2 is the Vista-and-later client.
        if unsafe { WlanOpenHandle(2, None, &mut negotiated, &mut handle) } != 0 {
            // No WLAN service: a PC without Wi-Fi.
            return None;
        }
        let context = Arc::into_raw(shared.clone());
        // SAFETY: `context` stays valid until `Drop` closes the handle; the callback only reads it.
        let r = unsafe {
            WlanRegisterNotification(
                handle,
                WLAN_NOTIFICATION_SOURCE_ACM,
                true,
                Some(on_wlan),
                Some(context.cast()),
                None,
                None,
            )
        };
        if r != 0 {
            log::debug!("network: no Wi-Fi notifications ({r}); the radio is read at each sample");
        }
        Some((Wlan { handle }, context))
    }

    /// (ssid, signal 0..=100) of the first connected Wi-Fi interface.
    fn current(&self) -> Option<(Option<String>, Option<f64>)> {
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        // SAFETY: a live out pointer; the list is freed below.
        if unsafe { WlanEnumInterfaces(self.handle, None, &mut list) } != 0 || list.is_null() {
            return None;
        }
        // SAFETY: the list holds `dwNumberOfItems` entries from `InterfaceInfo`.
        let items =
            unsafe { std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), (*list).dwNumberOfItems as usize) };
        let connected: Vec<GUID> =
            items.iter().filter(|i| i.isState == wlan_interface_state_connected).map(|i| i.InterfaceGuid).collect();
        // SAFETY: the list came from WlanEnumInterfaces and is freed once.
        unsafe { WlanFreeMemory(list.cast()) };
        let guid = connected.first()?;

        let mut size = 0u32;
        let mut data: *mut c_void = std::ptr::null_mut();
        // SAFETY: live out pointers; `data` is freed below.
        let r = unsafe {
            WlanQueryInterface(self.handle, guid, wlan_intf_opcode_current_connection, None, &mut size, &mut data, None)
        };
        if r == 0 && !data.is_null() {
            // SAFETY: a successful current-connection query returns a WLAN_CONNECTION_ATTRIBUTES.
            let attrs = unsafe { &*data.cast::<WLAN_CONNECTION_ATTRIBUTES>() };
            let ssid = &attrs.wlanAssociationAttributes.dot11Ssid;
            let len = (ssid.uSSIDLength as usize).min(ssid.ucSSID.len());
            let name = (len > 0).then(|| String::from_utf8_lossy(&ssid.ucSSID[..len]).into_owned());
            let quality = f64::from(attrs.wlanAssociationAttributes.wlanSignalQuality);
            // SAFETY: freeing the query result once.
            unsafe { WlanFreeMemory(data) };
            return Some((name, Some(quality)));
        }
        // Refused (location permission): the radio's RSSI still answers.
        let mut data: *mut c_void = std::ptr::null_mut();
        // SAFETY: live out pointers; `data` is freed below.
        let r =
            unsafe { WlanQueryInterface(self.handle, guid, wlan_intf_opcode_rssi, None, &mut size, &mut data, None) };
        if r == 0 && !data.is_null() {
            // SAFETY: the RSSI query returns a LONG in dBm.
            let rssi = unsafe { *data.cast::<i32>() };
            // SAFETY: freeing the query result once.
            unsafe { WlanFreeMemory(data) };
            return Some((None, Some(f64::from(2 * (rssi + 100)).clamp(0.0, 100.0))));
        }
        Some((None, None))
    }
}

impl Drop for Wlan {
    fn drop(&mut self) {
        // SAFETY: the handle came from WlanOpenHandle and is closed once; this waits for callbacks in flight.
        let _ = unsafe { WlanCloseHandle(self.handle, None) };
    }
}

struct Sample {
    at: Instant,
    down: u64,
    up: u64,
}

fn run(out: Publisher, shared: Arc<Shared>, stop: Arc<Owned>) {
    // SAFETY: initialises COM for this thread as an MTA; balanced at the end.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };

    // Connectivity events (optional: without them the thread still samples).
    // SAFETY: standard COM activation from an initialised thread.
    let manager: Option<INetworkListManager> = unsafe { CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL) }.ok();
    let mut advised = None;
    if let Some(m) = &manager {
        let sink: INetworkListManagerEvents = ConnectivityEvents { shared: shared.clone() }.into();
        // SAFETY: COM calls on live interfaces; the cookie is used to unadvise below.
        advised = unsafe {
            m.cast::<IConnectionPointContainer>()
                .and_then(|c| c.FindConnectionPoint(&INetworkListManagerEvents::IID))
                .and_then(|p| p.Advise(&sink).map(|cookie| (p, cookie)))
        }
        .ok();
    }

    // Interface changes.
    let context = Arc::into_raw(shared.clone());
    let mut change_handle = HANDLE::default();
    // SAFETY: `context` stays valid until the notification is cancelled below.
    let notified = unsafe {
        NotifyIpInterfaceChange(AF_UNSPEC, Some(on_interface_change), Some(context.cast()), false, &mut change_handle)
    };

    let wlan = Wlan::open(&shared);

    let mut prev: Option<Sample> = None;
    loop {
        let links = read_links();
        let (down_total, up_total) = totals(&links);
        let now = Instant::now();
        let (down, up) = match &prev {
            Some(p) => {
                let secs = now.duration_since(p.at).as_secs_f64().max(0.001);
                (
                    (down_total.saturating_sub(p.down) as f64 / secs).round() as i64,
                    (up_total.saturating_sub(p.up) as f64 / secs).round() as i64,
                )
            }
            None => (0, 0),
        };
        prev = Some(Sample { at: now, down: down_total, up: up_total });

        let flags = manager.as_ref().and_then(|m| {
            // SAFETY: a plain COM call on a live interface.
            unsafe { m.GetConnectivity() }.ok().map(|c| c.0)
        });
        let kind = kind_of(&links);
        let (connected, internet) = match flags {
            Some(f) => connectivity(f),
            None => (kind != "none", kind != "none"),
        };
        let (ssid, signal) = match (&wlan, kind) {
            (Some((w, _)), "wifi") => w.current().unwrap_or((None, None)),
            _ => (None, None),
        };
        out.publish(map([
            ("connected", Value::Bool(connected)),
            ("internet", Value::Bool(internet)),
            ("kind", text(if connected { kind } else { "none" })),
            ("ssid", ssid.map_or(Value::Nil, Value::Str)),
            ("signal", signal.map_or(Value::Nil, Value::Num)),
            ("down_bps", Value::Int(down)),
            ("up_bps", Value::Int(up)),
        ]));

        let wait = shared.interval_ms.load(Ordering::Relaxed).clamp(500, 60_000) as u32;
        crate::count_source_wake();
        // SAFETY: both handles are live events.
        let waited = unsafe { WaitForMultipleObjects(&[stop.get(), shared.wake.get()], false, wait) };
        if waited == WAIT_OBJECT_0 {
            break;
        }
        // An event burst (an adapter coming up) arrives as several callbacks: let them settle.
        if waited.0 == WAIT_OBJECT_0.0 + 1 {
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    if notified == ERROR_SUCCESS {
        // SAFETY: the handle came from NotifyIpInterfaceChange; cancelling waits for callbacks in flight.
        let _ = unsafe { CancelMibChangeNotify2(change_handle) };
    }
    // SAFETY: balances Arc::into_raw above; no callback can run any more.
    drop(unsafe { Arc::from_raw(context) });
    if let Some((point, cookie)) = advised {
        // SAFETY: unadvises the cookie from Advise above.
        let _ = unsafe { point.Unadvise(cookie) };
    }
    if let Some((w, ctx)) = wlan {
        drop(w);
        // SAFETY: balances Arc::into_raw in `Wlan::open`; the handle is closed, so no callback can run.
        drop(unsafe { Arc::from_raw(ctx) });
    }
    // The COM objects are released while the apartment still exists: releasing them after CoUninitialize
    // crashed the process at exit.
    drop(manager);
    if com.is_ok() {
        // SAFETY: balances the successful CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
}

pub(crate) struct NetworkService {
    shared: Option<Arc<Shared>>,
    interval: Mutex<Duration>,
    worker: Option<Worker>,
}

impl NetworkService {
    pub(crate) fn new() -> NetworkService {
        NetworkService { shared: None, interval: Mutex::new(DEFAULT_SAMPLE_INTERVAL), worker: None }
    }

    fn interval(&self) -> Duration {
        *self.interval.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Service for NetworkService {
    fn id(&self) -> ServiceId {
        ServiceId::Network
    }

    fn start(&mut self, out: Publisher) -> Result<(), String> {
        let shared =
            Arc::new(Shared { wake: auto_event()?, interval_ms: AtomicU64::new(self.interval().as_millis() as u64) });
        let thread_shared = shared.clone();
        self.worker = Some(Worker::spawn("rsb-network", move |stop| run(out, thread_shared, stop))?);
        self.shared = Some(shared);
        Ok(())
    }

    fn set_interval(&mut self, interval: Duration) {
        *self.interval.lock().unwrap_or_else(|e| e.into_inner()) = interval;
        if let Some(s) = &self.shared {
            s.interval_ms.store(interval.as_millis() as u64, Ordering::Relaxed);
            wake(s);
        }
    }

    fn command(&mut self, _command: ServiceCommand) -> Result<(), String> {
        Err("the network service has no commands".into())
    }

    fn stop(&mut self) {
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
        self.shared = None;
    }

    fn stale_after(&self) -> Duration {
        (self.interval() * 5).max(Duration::from_secs(10))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(index: u32, kind: u32, up: bool, hardware: bool, i: u64, o: u64) -> Link {
        Link { index, kind, up, hardware, in_octets: i, out_octets: o }
    }

    #[test]
    fn only_hardware_interfaces_that_are_up_are_counted() {
        let links = [
            link(1, IF_TYPE_ETHERNET, true, true, 100, 10),
            link(2, IF_TYPE_WIFI, true, true, 200, 20),
            link(3, IF_TYPE_ETHERNET, false, true, 1000, 1000),
            link(4, 24, true, false, 5000, 5000),
        ];
        assert_eq!(totals(&links), (300, 30));
    }

    #[test]
    fn connectivity_flags_split_into_connected_and_internet() {
        assert_eq!(connectivity(0), (false, false));
        assert_eq!(connectivity(NLM_IPV4_LOCAL), (true, false));
        assert_eq!(connectivity(NLM_IPV4_SUBNET | NLM_IPV6_LOCAL), (true, false));
        assert_eq!(connectivity(NLM_IPV4_INTERNET | NLM_IPV4_SUBNET), (true, true));
        assert_eq!(connectivity(NLM_IPV6_INTERNET), (true, true));
    }

    #[test]
    fn no_interface_means_none() {
        assert_eq!(kind_of(&[]), "none");
        assert_eq!(kind_of(&[link(1, IF_TYPE_ETHERNET, false, true, 0, 0)]), "none");
    }

    #[test]
    fn an_up_hardware_interface_names_its_kind() {
        let kind = kind_of(&[link(u32::MAX - 1, IF_TYPE_WWANPP, true, true, 0, 0)]);
        assert!(["cellular", "wifi", "ethernet", "other"].contains(&kind), "{kind}");
    }

    #[test]
    fn the_real_counters_only_go_up() {
        let first = totals(&read_links());
        std::thread::sleep(Duration::from_millis(300));
        let second = totals(&read_links());
        println!("interface counters: {first:?} then {second:?}");
        assert!(second.0 >= first.0 && second.1 >= first.1, "{first:?} -> {second:?}");
        assert!(!read_links().is_empty(), "this PC has interfaces");
    }
}

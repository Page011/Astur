//! Bluetooth radio and paired devices: Windows.Devices.Radios, a DeviceWatcher over paired
//! devices, battery from the device property store, connect and disconnect through the
//! audio endpoint's KS one-shot properties, forget through BluetoothRemoveDevice.
//!
//! Reads never change anything. Every command names a device by the id this service published
//! (checked on the hub thread, before anything is queued) and runs on the service thread, whose
//! waits all have deadlines. Names and addresses are values only: never logged.

mod audio;
mod battery;
mod winrt;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rsb_core::services::{BluetoothCommand, ServiceCommand, ServiceId, ServiceStatus};
use rsb_core::value::Value;
use windows::Win32::Devices::Bluetooth::{BLUETOOTH_ADDRESS, BluetoothRemoveDevice};
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::System::Threading::WaitForMultipleObjects;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::core::GUID;

use self::winrt::{RadioInfo, RadioWatch, Watch};
use super::Worker;
use crate::util::{map, text};
use crate::win::{Owned, auto_event, signal};
use crate::{Publisher, Service};

/// Devices kept after the classic and LE lists are merged.
const MAX_DEVICES: usize = 64;
/// A full re-read happens at least this often; the watchers and the radio event wake it sooner.
const REREAD: Duration = Duration::from_secs(60);
/// A burst of watcher events (the first enumeration is one per device) settles before a re-read.
const SETTLE: Duration = Duration::from_millis(500);
const SETTINGS_PAGE: &str = "ms-settings:bluetooth";

/// Opens a Windows page by address.
pub(crate) type Opener = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// One paired device as read.
#[derive(Clone, Debug)]
pub(crate) struct Dev {
    pub id: String,
    pub name: String,
    pub connected: bool,
    pub container: Option<GUID>,
    pub kind: &'static str,
    pub battery: Option<f64>,
    pub can_connect: bool,
    pub address: Option<u64>,
    pub le: bool,
}

/// The "aa:bb:cc:dd:ee:ff" (or dashed, or bare) form of a device address.
pub(crate) fn parse_mac(s: &str) -> Option<u64> {
    let hex: String = s.chars().filter(|c| *c != ':' && *c != '-').collect();
    if hex.len() == 12 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        u64::from_str_radix(&hex, 16).ok()
    } else {
        None
    }
}

/// WinRT device ids end in the address: `Bluetooth#Bluetooth<radio>-<address>`.
pub(crate) fn address_from_id(id: &str) -> Option<u64> {
    id.rsplit_once('-').and_then(|(_, tail)| parse_mac(tail))
}

/// From the class of device's major class (Bluetooth Assigned Numbers).
pub(crate) fn kind_from_major(major: i32) -> &'static str {
    match major {
        1 => "computer",
        2 => "phone",
        4 => "audio",
        5 => "input",
        7 => "wearable",
        _ => "other",
    }
}

/// From an LE appearance category.
pub(crate) fn kind_from_appearance(category: u16) -> &'static str {
    match category {
        1 => "phone",
        2 => "computer",
        3 | 7 => "wearable",
        10 => "audio",
        15 => "input",
        _ => "other",
    }
}

/// One entry per device: a dual-mode device shows up in both lists with different ids, and the
/// classic entry (the one that carries audio) wins. Entries without an address are kept as they
/// are. Connected on either list counts as connected.
pub(crate) fn merge(classic: Vec<Dev>, le: Vec<Dev>) -> Vec<Dev> {
    let mut out = classic;
    for d in le {
        match d.address.and_then(|a| out.iter_mut().find(|o| o.address == Some(a))) {
            Some(o) => o.connected |= d.connected,
            None => out.push(d),
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.id.cmp(&b.id)));
    out.truncate(MAX_DEVICES);
    out
}

/// Everything one read learned.
pub(crate) struct Snapshot {
    pub radio: RadioInfo,
    pub can_toggle: bool,
    pub devices: Vec<Dev>,
}

/// What the service asks of the system. A seam so the decisions are tested with a recorder and no
/// test touches a radio, a pairing or a connection.
pub(crate) trait Bt {
    fn radios(&mut self) -> Result<RadioInfo, String>;
    fn can_toggle(&mut self) -> bool;
    fn paired(&mut self, le: bool) -> Result<Vec<Dev>, String>;
    fn kind(&mut self, id: &str, le: bool) -> Result<&'static str, String>;
    fn battery(&mut self, address: u64) -> Option<f64>;
    fn set_radios(&mut self, on: bool) -> Result<(), String>;
    fn set_connected(&mut self, container: &GUID, connect: bool) -> Result<(), String>;
    fn forget(&mut self, dev: &Dev) -> Result<(), String>;
}

/// Reads the radios and, when there is one, the paired devices. `kinds` remembers each device's
/// kind (opening the device object to read it is the slow part).
pub(crate) fn read(bt: &mut dyn Bt, kinds: &mut HashMap<String, &'static str>) -> Result<Snapshot, String> {
    let radio = bt.radios()?;
    if !radio.available {
        return Ok(Snapshot { radio, can_toggle: false, devices: Vec::new() });
    }
    let can_toggle = bt.can_toggle();
    // One list failing (LE off) leaves the other's devices in.
    let classic = bt.paired(false);
    let le = bt.paired(true);
    if let (Err(e), Err(_)) = (&classic, &le) {
        return Err(e.clone());
    }
    let mut devices = merge(classic.unwrap_or_default(), le.unwrap_or_default());
    for d in &mut devices {
        d.kind = *kinds.entry(d.id.clone()).or_insert_with(|| bt.kind(&d.id, d.le).unwrap_or("other"));
        d.can_connect = d.kind == "audio" && d.container.is_some();
        d.battery = d.address.and_then(|a| bt.battery(a));
    }
    Ok(Snapshot { radio, can_toggle, devices })
}

fn value_of(s: &Snapshot) -> Value {
    let devices = s
        .devices
        .iter()
        .map(|d| {
            map([
                ("id", Value::Str(d.id.clone())),
                ("name", Value::Str(d.name.clone())),
                ("kind", text(d.kind)),
                ("connected", Value::Bool(d.connected && s.radio.enabled)),
                ("battery", d.battery.map_or(Value::Nil, Value::Num)),
                ("can_connect", Value::Bool(d.can_connect)),
            ])
        })
        .collect();
    map([
        ("available", Value::Bool(s.radio.available)),
        ("enabled", Value::Bool(s.radio.enabled)),
        ("can_toggle", Value::Bool(s.can_toggle)),
        ("devices", Value::List(devices)),
    ])
}

/// What the last read published, for the checks on the hub thread.
#[derive(Default)]
pub(crate) struct Published {
    /// id -> can_connect.
    pub(crate) devices: HashMap<String, bool>,
    pub(crate) can_toggle: bool,
}

/// The id and radio checks that need no system call: a refusal never waits and never reaches a
/// radio, a pairing or a connection. The errors name nothing.
pub(crate) fn check_command(command: &BluetoothCommand, p: &Published) -> Result<(), String> {
    let known = |id: &str| p.devices.get(id).copied().ok_or_else(|| "no Bluetooth device with that id".to_string());
    match command {
        BluetoothCommand::SetEnabled(_) if !p.can_toggle => {
            Err("Windows does not let this app switch Bluetooth".into())
        }
        BluetoothCommand::SetEnabled(_) | BluetoothCommand::OpenSettings | BluetoothCommand::Rescan => Ok(()),
        BluetoothCommand::Connect(id) | BluetoothCommand::Disconnect(id) => {
            if known(id)? {
                Ok(())
            } else {
                Err("that device cannot be connected from here".into())
            }
        }
        BluetoothCommand::Forget(id) => known(id).map(|_| ()),
    }
}

/// Runs one checked command against the list as it is now.
pub(crate) fn execute(bt: &mut dyn Bt, command: &BluetoothCommand, devices: &[Dev]) -> Result<(), String> {
    let find =
        |id: &str| devices.iter().find(|d| d.id == id).ok_or_else(|| "no Bluetooth device with that id".to_string());
    match command {
        BluetoothCommand::SetEnabled(on) => bt.set_radios(*on),
        BluetoothCommand::Connect(id) | BluetoothCommand::Disconnect(id) => {
            let dev = find(id)?;
            let container =
                dev.container.filter(|_| dev.can_connect).ok_or("that device cannot be connected from here")?;
            bt.set_connected(&container, matches!(command, BluetoothCommand::Connect(_)))
        }
        BluetoothCommand::Forget(id) => bt.forget(find(id)?),
        // The re-read that follows every command is the whole effect.
        BluetoothCommand::OpenSettings | BluetoothCommand::Rescan => Ok(()),
    }
}

/// The system behind `Bt`.
struct System {
    watch: winrt::RadioWatch,
    wake: Arc<Shared>,
    allowed: Option<bool>,
}

impl Bt for System {
    fn radios(&mut self) -> Result<RadioInfo, String> {
        let radios = winrt::bluetooth_radios()?;
        let shared = self.wake.clone();
        self.watch.follow(&radios, move || signal(&shared.wake));
        Ok(winrt::radio_info(&radios))
    }

    fn can_toggle(&mut self) -> bool {
        // Asked again only while the answer is no.
        if self.allowed != Some(true) {
            self.allowed = Some(winrt::radio_access_allowed());
        }
        self.allowed == Some(true)
    }

    fn paired(&mut self, le: bool) -> Result<Vec<Dev>, String> {
        winrt::paired(le)
    }

    fn kind(&mut self, id: &str, le: bool) -> Result<&'static str, String> {
        winrt::kind_of(id, le)
    }

    fn battery(&mut self, address: u64) -> Option<f64> {
        battery::percent(address)
    }

    fn set_radios(&mut self, on: bool) -> Result<(), String> {
        winrt::set_radios(on)
    }

    fn set_connected(&mut self, container: &GUID, connect: bool) -> Result<(), String> {
        audio::set_connected(container, connect)
    }

    fn forget(&mut self, dev: &Dev) -> Result<(), String> {
        if let Some(address) = dev.address {
            let target = BLUETOOTH_ADDRESS {
                Anonymous: windows::Win32::Devices::Bluetooth::BLUETOOTH_ADDRESS_0 { ullLong: address },
            };
            // SAFETY: a plain call with a live address struct.
            if unsafe { BluetoothRemoveDevice(&target) } == 0 {
                return Ok(());
            }
        }
        winrt::unpair(&dev.id)
    }
}

struct Shared {
    wake: Owned,
    commands: Mutex<VecDeque<BluetoothCommand>>,
    published: Mutex<Published>,
}

fn run(out: Publisher, shared: Arc<Shared>, stop: Arc<Owned>) {
    // SAFETY: initialises the WinRT apartment of this thread as MTA (COM for the audio endpoints
    // is then usable too); balanced at the end.
    let ro = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    let mut bt = System { watch: RadioWatch::default(), wake: shared.clone(), allowed: None };
    let mut kinds: HashMap<String, &'static str> = HashMap::new();
    let mut watchers: Vec<Watch> = Vec::new();
    let mut watchers_started = false;
    let mut devices: Vec<Dev> = Vec::new();
    loop {
        crate::count_source_wake();
        let pending: Vec<BluetoothCommand> =
            shared.commands.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
        for c in pending {
            if c == BluetoothCommand::Rescan {
                // Device kinds are looked up once per id; a rescan asks again.
                kinds.clear();
            }
            if let Err(e) = execute(&mut bt, &c, &devices) {
                log::warn!("bluetooth command failed: {e}");
            }
        }

        match read(&mut bt, &mut kinds) {
            Ok(s) => {
                let mut p = shared.published.lock().unwrap_or_else(|e| e.into_inner());
                p.devices = s.devices.iter().map(|d| (d.id.clone(), d.can_connect)).collect();
                p.can_toggle = s.can_toggle;
                drop(p);
                kinds.retain(|id, _| s.devices.iter().any(|d| &d.id == id));
                let value = value_of(&s);
                if s.radio.available {
                    out.publish(value);
                    if !watchers_started {
                        watchers_started = true;
                        for le in [false, true] {
                            let wake = shared.clone();
                            match Watch::start(le, move || signal(&wake.wake)) {
                                Ok(w) => watchers.push(w),
                                Err(e) => log::debug!("bluetooth: {e}"),
                            }
                        }
                    }
                } else {
                    out.publish_as(value, ServiceStatus::Unavailable, Some("no Bluetooth radio".into()));
                }
                devices = s.devices;
            }
            Err(e) => {
                log::debug!("bluetooth: {e}");
                out.status(ServiceStatus::Error, Some("Bluetooth could not be read".into()));
            }
        }

        // SAFETY: both handles are live events.
        let waited =
            unsafe { WaitForMultipleObjects(&[stop.get(), shared.wake.get()], false, REREAD.as_millis() as u32) };
        if waited == WAIT_OBJECT_0 {
            break;
        }
        std::thread::sleep(SETTLE);
    }
    drop(watchers);
    drop(bt);
    if ro.is_ok() {
        // SAFETY: balances RoInitialize.
        unsafe { RoUninitialize() };
    }
}

pub(crate) struct BluetoothService {
    shared: Option<Arc<Shared>>,
    worker: Option<Worker>,
    /// Opens the settings page; the apps service's checked opener unless a test passes another.
    opener: Opener,
}

impl BluetoothService {
    pub(crate) fn new() -> BluetoothService {
        BluetoothService::with_opener(Arc::new(super::apps::open_page))
    }

    pub(crate) fn with_opener(opener: Opener) -> BluetoothService {
        BluetoothService { shared: None, worker: None, opener }
    }
}

impl Service for BluetoothService {
    fn id(&self) -> ServiceId {
        ServiceId::Bluetooth
    }

    fn start(&mut self, out: Publisher) -> Result<(), String> {
        let shared = Arc::new(Shared {
            wake: auto_event()?,
            commands: Mutex::new(VecDeque::new()),
            published: Mutex::new(Published::default()),
        });
        let thread_shared = shared.clone();
        self.worker = Some(Worker::spawn("rsb-bluetooth", move |stop| run(out, thread_shared, stop))?);
        self.shared = Some(shared);
        Ok(())
    }

    fn command(&mut self, command: ServiceCommand) -> Result<(), String> {
        let (ServiceCommand::Bluetooth(c), Some(shared)) = (command, &self.shared) else {
            return Err("not a Bluetooth command, or the service is stopped".into());
        };
        {
            let p = shared.published.lock().unwrap_or_else(|e| e.into_inner());
            check_command(&c, &p)?;
        }
        if c == BluetoothCommand::OpenSettings {
            return (self.opener)(SETTINGS_PAGE);
        }
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
mod tests;

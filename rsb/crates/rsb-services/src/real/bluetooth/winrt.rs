//! The WinRT side of the Bluetooth service: radios, the paired-device lists and the device
//! watchers. Every call that waits on Windows has a deadline, so a stuck radio driver never holds
//! the service thread.

use std::time::Duration;

use windows::Devices::Bluetooth::{BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationUpdate, DevicePicker, DeviceWatcher};
use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};
use windows::Foundation::{IReference, TypedEventHandler};
use windows::core::{GUID, HSTRING, Interface, RuntimeType};

use super::{Dev, address_from_id, kind_from_appearance, kind_from_major, parse_mac};
use crate::real::winrt::await_op;

const LIST_WAIT: Duration = Duration::from_secs(4);
const RADIO_WAIT: Duration = Duration::from_secs(5);
const KIND_WAIT: Duration = Duration::from_secs(2);

const KEY_CONNECTED: &str = "System.Devices.Aep.IsConnected";
const KEY_ADDRESS: &str = "System.Devices.Aep.DeviceAddress";
const KEY_CONTAINER: &str = "System.Devices.Aep.ContainerId";

/// What the radios say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RadioInfo {
    pub available: bool,
    pub enabled: bool,
}

pub(crate) fn bluetooth_radios() -> Result<Vec<Radio>, String> {
    let op = Radio::GetRadiosAsync().map_err(|e| format!("the radio list could not be asked for: {e}"))?;
    let all = await_op!(op, RADIO_WAIT).map_err(|e| format!("the radio list could not be read: {e}"))?;
    Ok(all.into_iter().filter(|r| r.Kind().is_ok_and(|k| k == RadioKind::Bluetooth)).collect())
}

pub(crate) fn radio_info(radios: &[Radio]) -> RadioInfo {
    RadioInfo {
        available: !radios.is_empty(),
        enabled: radios.iter().any(|r| r.State().is_ok_and(|s| s == RadioState::On)),
    }
}

/// Whether Windows lets this app switch radios (`Radio.RequestAccessAsync`).
pub(crate) fn radio_access_allowed() -> bool {
    let Ok(op) = Radio::RequestAccessAsync() else { return false };
    await_op!(op, RADIO_WAIT).is_ok_and(|s| s == RadioAccessStatus::Allowed)
}

/// Turns every Bluetooth radio on or off.
pub(crate) fn set_radios(on: bool) -> Result<(), String> {
    let radios = bluetooth_radios()?;
    if radios.is_empty() {
        return Err("no Bluetooth radio".into());
    }
    let state = if on { RadioState::On } else { RadioState::Off };
    let mut last = Ok(());
    for r in radios {
        let result = r
            .SetStateAsync(state)
            .and_then(|op| await_op!(op, RADIO_WAIT))
            .map_err(|e| format!("the radio could not be switched: {e}"))
            .and_then(
                |s| if s == RadioAccessStatus::Allowed { Ok(()) } else { Err("Windows refused the switch".into()) },
            );
        if result.is_err() {
            last = result;
        }
    }
    last
}

fn property<T: RuntimeType + 'static>(info: &DeviceInformation, key: &str) -> Option<T> {
    info.Properties().ok()?.Lookup(&HSTRING::from(key)).ok()?.cast::<IReference<T>>().ok()?.Value().ok()
}

/// The extra properties asked of every device, as the collection type the enumeration calls take.
/// That type is not nameable from this crate, so the plain list an activatable class hands out is
/// filled and used as it is; a macro, because its type cannot be written in a signature.
macro_rules! requested {
    () => {
        (|| -> windows::core::Result<_> {
            let list = DevicePicker::new()?.Filter()?.SupportedDeviceSelectors()?;
            for key in [KEY_CONNECTED, KEY_ADDRESS, KEY_CONTAINER] {
                list.Append(&HSTRING::from(key))?;
            }
            Ok(list)
        })()
    };
}

fn selector(le: bool) -> windows::core::Result<HSTRING> {
    if le {
        BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)
    } else {
        BluetoothDevice::GetDeviceSelectorFromPairingState(true)
    }
}

/// The paired devices of one kind (classic or LE) with the properties the value needs, except the
/// kind and the battery.
pub(crate) fn paired(le: bool) -> Result<Vec<Dev>, String> {
    let filter = selector(le).map_err(|e| format!("no device selector: {e}"))?;
    let extra = requested!().map_err(|e| format!("the property list could not be made: {e}"))?;
    let op = DeviceInformation::FindAllAsyncAqsFilterAndAdditionalProperties(&filter, &extra)
        .map_err(|e| format!("the paired devices could not be asked for: {e}"))?;
    let found = await_op!(op, LIST_WAIT).map_err(|e| format!("the paired devices could not be read: {e}"))?;
    let mut out = Vec::new();
    for info in found {
        let id = info.Id().map(|h| h.to_string()).unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        let address = property::<HSTRING>(&info, KEY_ADDRESS)
            .and_then(|a| parse_mac(&a.to_string()))
            .or_else(|| address_from_id(&id));
        out.push(Dev {
            name: info.Name().map(|h| h.to_string()).unwrap_or_default(),
            connected: property::<bool>(&info, KEY_CONNECTED).unwrap_or(false),
            container: property::<GUID>(&info, KEY_CONTAINER).filter(|g| *g != GUID::zeroed()),
            kind: "other",
            battery: None,
            can_connect: false,
            address,
            le,
            id,
        });
    }
    Ok(out)
}

/// What sort of device this is, from its class of device (classic) or appearance (LE). The device
/// object is opened for the reading and closed again.
pub(crate) fn kind_of(id: &str, le: bool) -> Result<&'static str, String> {
    let id = HSTRING::from(id);
    let failed = |e: windows::core::Error| format!("the device kind could not be read: {e}");
    if le {
        let op = BluetoothLEDevice::FromIdAsync(&id).map_err(failed)?;
        let device = await_op!(op, KIND_WAIT).map_err(failed)?;
        let category = device.Appearance().and_then(|a| a.Category());
        let _ = device.Close();
        Ok(kind_from_appearance(category.map_err(failed)?))
    } else {
        let op = BluetoothDevice::FromIdAsync(&id).map_err(failed)?;
        let device = await_op!(op, KIND_WAIT).map_err(failed)?;
        let major = device.ClassOfDevice().and_then(|c| c.MajorClass());
        let _ = device.Close();
        Ok(kind_from_major(major.map_err(failed)?.0))
    }
}

/// Removes a pairing through WinRT (the fallback for devices the Win32 call does not know).
pub(crate) fn unpair(id: &str) -> Result<(), String> {
    let failed = |e: windows::core::Error| format!("the pairing could not be removed: {e}");
    let op = DeviceInformation::CreateFromIdAsync(&HSTRING::from(id)).map_err(failed)?;
    let info = await_op!(op, LIST_WAIT).map_err(failed)?;
    let op = info.Pairing().and_then(|p| p.UnpairAsync()).map_err(failed)?;
    let result = await_op!(op, RADIO_WAIT).map_err(failed)?;
    match result.Status().map_err(failed)? {
        windows::Devices::Enumeration::DeviceUnpairingResultStatus::Unpaired => Ok(()),
        _ => Err("Windows would not remove the pairing".into()),
    }
}

/// A device watcher whose events only wake the service; the list is re-read when it does.
pub(crate) struct Watch {
    watcher: DeviceWatcher,
    tokens: [i64; 3],
}

impl Watch {
    pub(crate) fn start(le: bool, wake: impl Fn() + Clone + Send + 'static) -> Result<Watch, String> {
        let failed = |e: windows::core::Error| format!("the device watcher could not start: {e}");
        let extra = requested!().map_err(failed)?;
        let watcher =
            DeviceInformation::CreateWatcherAqsFilterAndAdditionalProperties(&selector(le).map_err(failed)?, &extra)
                .map_err(failed)?;
        let (w1, w2, w3) = (wake.clone(), wake.clone(), wake);
        let added = watcher
            .Added(&TypedEventHandler::<DeviceWatcher, DeviceInformation>::new(move |_, _| {
                w1();
                Ok(())
            }))
            .map_err(failed)?;
        let updated = watcher
            .Updated(&TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, _| {
                w2();
                Ok(())
            }))
            .map_err(failed)?;
        let removed = watcher
            .Removed(&TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, _| {
                w3();
                Ok(())
            }))
            .map_err(failed)?;
        let watch = Watch { watcher, tokens: [added, updated, removed] };
        watch.watcher.Start().map_err(failed)?;
        Ok(watch)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.watcher.Stop();
        let _ = self.watcher.RemoveAdded(self.tokens[0]);
        let _ = self.watcher.RemoveUpdated(self.tokens[1]);
        let _ = self.watcher.RemoveRemoved(self.tokens[2]);
    }
}

/// Follows the state of every Bluetooth radio so a switch made in Windows wakes the service.
#[derive(Default)]
pub(crate) struct RadioWatch {
    names: Vec<String>,
    followed: Vec<(Radio, i64)>,
}

impl RadioWatch {
    /// Registers on these radios unless they are the ones already followed (same names).
    pub(crate) fn follow(&mut self, radios: &[Radio], wake: impl Fn() + Clone + Send + 'static) {
        let names: Vec<String> = radios.iter().map(|r| r.Name().map(|n| n.to_string()).unwrap_or_default()).collect();
        if names == self.names {
            return;
        }
        self.release();
        for r in radios {
            let wake = wake.clone();
            let handler = TypedEventHandler::<Radio, windows::core::IInspectable>::new(move |_, _| {
                wake();
                Ok(())
            });
            if let Ok(token) = r.StateChanged(&handler) {
                self.followed.push((r.clone(), token));
            }
        }
        self.names = names;
    }

    fn release(&mut self) {
        for (r, token) in self.followed.drain(..) {
            let _ = r.RemoveStateChanged(token);
        }
    }
}

impl Drop for RadioWatch {
    fn drop(&mut self) {
        self.release();
    }
}

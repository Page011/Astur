//! Audio endpoints (MMDevice / Core Audio) as the audio and microphone
//! services use them: listing the active ones with their volume callbacks,
//! reading and setting a level, and the callbacks that wake a service
//! thread. A callback only signals; the service thread does every COM call,
//! so a callback never re-enters the audio engine.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE, DEVICE_STATE_ACTIVE, EDataFlow, ERole, IMMDevice,
    IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoTaskMemFree, STGM_READ};
use windows::Win32::System::Threading::SetEvent;
use windows::core::{GUID, PCWSTR, implement};

use crate::win::Owned;

/// At most this many devices of each kind are listed and watched.
pub(crate) const MAX_DEVICES: usize = 16;

/// `PKEY_Device_FriendlyName`.
const PKEY_FRIENDLY_NAME: PROPERTYKEY =
    PROPERTYKEY { fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0), pid: 14 };
/// `PKEY_Device_EnumeratorName`: `BTHENUM` / `BTHLEENUM` for Bluetooth endpoints.
const PKEY_ENUMERATOR_NAME: PROPERTYKEY =
    PROPERTYKEY { fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0), pid: 24 };

/// What the callbacks share with the service thread.
pub(crate) struct Signals {
    pub(crate) wake: Owned,
    /// A device came, went, changed state or became the default: list again.
    pub(crate) device_changed: AtomicBool,
}

impl Signals {
    pub(crate) fn wake(&self) {
        // SAFETY: `wake` is a live event owned by the shared state.
        let _ = unsafe { SetEvent(self.wake.get()) };
    }
}

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeCallback {
    signals: Arc<Signals>,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
    fn OnNotify(&self, _data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        self.signals.wake();
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct DeviceCallback {
    signals: Arc<Signals>,
}

impl DeviceCallback_Impl {
    fn changed(&self) -> windows::core::Result<()> {
        self.signals.device_changed.store(true, Ordering::Release);
        self.signals.wake();
        Ok(())
    }
}

impl IMMNotificationClient_Impl for DeviceCallback_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> windows::core::Result<()> {
        self.changed()
    }
    fn OnDeviceAdded(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        self.changed()
    }
    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        self.changed()
    }
    fn OnDefaultDeviceChanged(&self, _flow: EDataFlow, _role: ERole, _id: &PCWSTR) -> windows::core::Result<()> {
        self.changed()
    }
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> windows::core::Result<()> {
        Ok(())
    }
}

/// The client to hand to `RegisterEndpointNotificationCallback`.
pub(crate) fn device_client(signals: &Arc<Signals>) -> IMMNotificationClient {
    DeviceCallback { signals: signals.clone() }.into()
}

/// One active endpoint with its volume interface and the callback
/// registered on it (unregistered on drop).
pub(crate) struct Endpoint {
    pub(crate) id: String,
    /// Never logged.
    pub(crate) name: String,
    pub(crate) bluetooth: bool,
    pub(crate) default: bool,
    volume: IAudioEndpointVolume,
    callback: IAudioEndpointVolumeCallback,
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        // SAFETY: the callback was registered on this endpoint when it was opened.
        let _ = unsafe { self.volume.UnregisterControlChangeNotify(&self.callback) };
    }
}

impl Endpoint {
    /// (volume 0.0..=1.0, muted).
    pub(crate) fn level(&self) -> Option<(f32, bool)> {
        // SAFETY: plain COM calls on a live interface.
        unsafe { Some((self.volume.GetMasterVolumeLevelScalar().ok()?, self.volume.GetMute().ok()?.as_bool())) }
    }

    pub(crate) fn set_volume(&self, percent: f32) {
        // SAFETY: a plain COM call on a live interface; a null event context is allowed.
        let _ = unsafe { self.volume.SetMasterVolumeLevelScalar((percent / 100.0).clamp(0.0, 1.0), std::ptr::null()) };
    }

    pub(crate) fn set_muted(&self, muted: bool) {
        // SAFETY: a plain COM call on a live interface; a null event context is allowed.
        let _ = unsafe { self.volume.SetMute(muted, std::ptr::null()) };
    }
}

fn id_of(device: &IMMDevice) -> Option<String> {
    // SAFETY: GetId returns a CoTaskMem string, freed once after it is copied.
    unsafe {
        let p = device.GetId().ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
        s
    }
}

fn property_text(device: &IMMDevice, key: &PROPERTYKEY) -> String {
    // SAFETY: plain COM calls; the PROPVARIANT frees itself on drop.
    unsafe {
        device
            .OpenPropertyStore(STGM_READ)
            .and_then(|store| store.GetValue(key))
            .map(|v| v.to_string())
            .unwrap_or_default()
    }
}

/// Whether an `EnumeratorName` value marks a Bluetooth endpoint.
pub(crate) fn is_bluetooth_enumerator(name: &str) -> bool {
    name.eq_ignore_ascii_case("BTHENUM") || name.eq_ignore_ascii_case("BTHLEENUM")
}

/// The id of the default endpoint of `flow` for `role`, when there is one.
pub(crate) fn default_id(enumerator: &IMMDeviceEnumerator, flow: EDataFlow, role: ERole) -> Option<String> {
    // SAFETY: a plain COM call on a live interface.
    unsafe { enumerator.GetDefaultAudioEndpoint(flow, role).ok().and_then(|d| id_of(&d)) }
}

/// Puts the entry that `is_default` names first (when present), keeps the rest in order and cuts the list to
/// `limit`: the default is never the one that falls off.
pub(crate) fn order_default_first<T>(mut items: Vec<T>, is_default: impl Fn(&T) -> bool, limit: usize) -> Vec<T> {
    if let Some(i) = items.iter().position(&is_default) {
        let d = items.remove(i);
        items.insert(0, d);
    }
    items.truncate(limit);
    items
}

/// The active endpoints of `flow`, the default for `role` first, at most
/// `MAX_DEVICES`, each with a volume callback registered. A device without a
/// volume interface is skipped.
pub(crate) fn list(
    enumerator: &IMMDeviceEnumerator,
    flow: EDataFlow,
    role: ERole,
    signals: &Arc<Signals>,
) -> Vec<Endpoint> {
    let default = default_id(enumerator, flow, role);
    // SAFETY: plain COM calls on live interfaces from an MTA thread.
    let devices: Vec<(String, IMMDevice)> = unsafe {
        let Ok(collection) = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) else { return Vec::new() };
        let count = collection.GetCount().unwrap_or(0);
        (0..count).filter_map(|i| collection.Item(i).ok()).filter_map(|d| id_of(&d).map(|id| (id, d))).collect()
    };
    let devices = order_default_first(devices, |(id, _)| default.as_deref() == Some(id.as_str()), MAX_DEVICES);
    devices
        .into_iter()
        .filter_map(|(id, device)| {
            // SAFETY: plain COM calls on live interfaces; the callback is unregistered when the Endpoint drops.
            unsafe {
                let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
                let callback: IAudioEndpointVolumeCallback = VolumeCallback { signals: signals.clone() }.into();
                volume.RegisterControlChangeNotify(&callback).ok()?;
                Some(Endpoint {
                    name: property_text(&device, &PKEY_FRIENDLY_NAME),
                    bluetooth: is_bluetooth_enumerator(&property_text(&device, &PKEY_ENUMERATOR_NAME)),
                    default: default.as_deref() == Some(id.as_str()),
                    id,
                    volume,
                    callback,
                })
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bluetooth_endpoints_are_the_two_bluetooth_enumerators() {
        assert!(is_bluetooth_enumerator("BTHENUM"));
        assert!(is_bluetooth_enumerator("bthleenum"));
        assert!(!is_bluetooth_enumerator("USB"));
        assert!(!is_bluetooth_enumerator("HDAUDIO"));
        assert!(!is_bluetooth_enumerator(""));
    }

    #[test]
    fn the_default_comes_first_and_survives_the_cut() {
        let items: Vec<u32> = (0..20).collect();
        let cut = order_default_first(items, |i| *i == 18, MAX_DEVICES);
        assert_eq!(cut.len(), MAX_DEVICES);
        assert_eq!(cut[0], 18, "a default past the limit is kept");
        assert_eq!(&cut[1..4], &[0, 1, 2]);
        let none = order_default_first(vec![1, 2, 3], |_| false, 16);
        assert_eq!(none, vec![1, 2, 3], "no default leaves the order alone");
    }
}

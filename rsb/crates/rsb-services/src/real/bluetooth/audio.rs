//! Connecting and disconnecting a Bluetooth audio device: the Bluetooth audio driver's one-shot
//! `KSPROPERTY_ONESHOT_RECONNECT` / `KSPROPERTY_ONESHOT_DISCONNECT` (documented for that driver),
//! sent through the `IKsControl` of the part the device's audio endpoint is connected to. The
//! endpoint belongs to the paired device by their shared container id.

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::{
    DEVICE_STATE, DEVICE_STATEMASK_ALL, EDataFlow, IConnector, IDeviceTopology, IMMDevice, IMMDeviceEnumerator, IPart,
    MMDeviceEnumerator, eCapture, eRender,
};
use windows::Win32::Media::KernelStreaming::{
    IKsControl, KSIDENTIFIER, KSIDENTIFIER_0, KSIDENTIFIER_0_0, KSPROPERTY_ONESHOT_DISCONNECT,
    KSPROPERTY_ONESHOT_RECONNECT, KSPROPERTY_TYPE_GET, KSPROPSETID_BtAudio,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToGUID;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, STGM_READ};
use windows::core::{GUID, Interface};

/// `PKEY_Device_ContainerId`.
const CONTAINER_ID: PROPERTYKEY =
    PROPERTYKEY { fmtid: GUID::from_u128(0x8c7ed206_3f8a_4827_b3ab_ae9e1faefc6c), pid: 2 };

fn container_of(device: &IMMDevice) -> Option<GUID> {
    // SAFETY: plain COM calls on a live endpoint; the variant is cleared by its Drop.
    unsafe {
        let store = device.OpenPropertyStore(STGM_READ).ok()?;
        let value = store.GetValue(&CONTAINER_ID).ok()?;
        PropVariantToGUID(&value).ok()
    }
}

fn send(device: &IMMDevice, connect: bool) -> windows::core::Result<()> {
    // SAFETY: plain COM calls on live interfaces from an initialised thread; the property carries
    // no input or output data.
    unsafe {
        let topology: IDeviceTopology = device.Activate(CLSCTX_ALL, None)?;
        let connector: IConnector = topology.GetConnector(0)?;
        let other: IConnector = connector.GetConnectedTo()?;
        let part: IPart = other.cast()?;
        let mut raw = std::ptr::null_mut();
        part.Activate(CLSCTX_ALL.0, &IKsControl::IID, Some(&mut raw))?;
        let control = IKsControl::from_raw(raw);
        let id = KSIDENTIFIER {
            Anonymous: KSIDENTIFIER_0 {
                Anonymous: KSIDENTIFIER_0_0 {
                    Set: KSPROPSETID_BtAudio,
                    Id: if connect { KSPROPERTY_ONESHOT_RECONNECT.0 } else { KSPROPERTY_ONESHOT_DISCONNECT.0 } as u32,
                    Flags: KSPROPERTY_TYPE_GET,
                },
            },
        };
        let mut returned = 0u32;
        control.KsProperty(&id, size_of::<KSIDENTIFIER>() as u32, std::ptr::null_mut(), 0, &mut returned)
    }
}

/// Reconnects or disconnects the audio side of the device whose container id this is. Must run
/// on a thread with COM initialised.
pub(crate) fn set_connected(container: &GUID, connect: bool) -> Result<(), String> {
    // SAFETY: standard COM activation from an initialised thread.
    let enumerator: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .map_err(|e| format!("the audio devices could not be opened: {e}"))?;
    let mut last = Err("no audio endpoint for that device".to_string());
    for flow in [eRender, eCapture] {
        // Every state: a disconnected headset's endpoint is "not present", and reconnecting is
        // what brings it back.
        // SAFETY: plain COM calls on a live enumerator.
        let Ok(list) =
            (unsafe { enumerator.EnumAudioEndpoints(EDataFlow(flow.0), DEVICE_STATE(DEVICE_STATEMASK_ALL)) })
        else {
            continue;
        };
        // SAFETY: as above.
        let count = unsafe { list.GetCount() }.unwrap_or(0);
        for i in 0..count {
            // SAFETY: as above.
            let Ok(device) = (unsafe { list.Item(i) }) else { continue };
            if container_of(&device).as_ref() != Some(container) {
                continue;
            }
            match send(&device, connect) {
                Ok(()) => return Ok(()),
                Err(e) => last = Err(format!("the Bluetooth audio driver refused: {e}")),
            }
        }
    }
    last
}

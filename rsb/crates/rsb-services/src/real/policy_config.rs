//! Making an audio output the default. Windows has no documented call for
//! it; this uses the `IPolicyConfig` COM interface every output switcher
//! uses (EarTrumpet, SoundSwitch), created from its class id on the audio
//! thread. When the class or interface is missing, `set_default` fails and
//! the audio service reports `switch_available = false`.

use windows::Win32::Media::Audio::{eCommunications, eConsole, eMultimedia};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::core::{GUID, PCWSTR};

/// `CPolicyConfigClient`.
const POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

/// The method names are Windows' own.
#[allow(non_snake_case)]
mod iface {
    use std::ffi::c_void;

    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::ERole;
    use windows::core::{HRESULT, IUnknown, IUnknown_Vtbl, PCWSTR};

    /// The Windows 7 and later layout of the interface. Only `SetDefaultEndpoint`
    /// is called; the others are placeholders that keep the vtable slots in place.
    #[windows::core::interface("F8679F50-850A-41CF-9C72-430F290290C8")]
    pub unsafe trait IPolicyConfig: IUnknown {
        unsafe fn GetMixFormat(&self, device: PCWSTR, format: *mut *mut c_void) -> HRESULT;
        unsafe fn GetDeviceFormat(&self, device: PCWSTR, default: i32, format: *mut *mut c_void) -> HRESULT;
        unsafe fn ResetDeviceFormat(&self, device: PCWSTR) -> HRESULT;
        unsafe fn SetDeviceFormat(&self, device: PCWSTR, endpoint: *mut c_void, mix: *mut c_void) -> HRESULT;
        unsafe fn GetProcessingPeriod(
            &self,
            device: PCWSTR,
            default: i32,
            first: *mut i64,
            second: *mut i64,
        ) -> HRESULT;
        unsafe fn SetProcessingPeriod(&self, device: PCWSTR, period: *mut i64) -> HRESULT;
        unsafe fn GetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
        unsafe fn SetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
        unsafe fn GetPropertyValue(
            &self,
            device: PCWSTR,
            fx: i32,
            key: *const PROPERTYKEY,
            value: *mut c_void,
        ) -> HRESULT;
        unsafe fn SetPropertyValue(
            &self,
            device: PCWSTR,
            fx: i32,
            key: *const PROPERTYKEY,
            value: *const c_void,
        ) -> HRESULT;
        unsafe fn SetDefaultEndpoint(&self, device: PCWSTR, role: ERole) -> HRESULT;
        unsafe fn SetEndpointVisibility(&self, device: PCWSTR, visible: i32) -> HRESULT;
    }

    /// Calls `SetDefaultEndpoint` (the trait's methods are private to this module).
    pub(super) fn set_default_endpoint(policy: &IPolicyConfig, device: PCWSTR, role: ERole) -> HRESULT {
        // SAFETY: `device` is a NUL-terminated string that outlives the call; the interface is live.
        unsafe { policy.SetDefaultEndpoint(device, role) }
    }
}

use iface::IPolicyConfig;

fn create() -> Option<IPolicyConfig> {
    // SAFETY: standard COM activation from a thread that initialised COM; a missing class is an Err.
    unsafe { CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL) }.ok()
}

/// What a switch is refused with when the interface is missing (the fake says the same).
pub(crate) const UNAVAILABLE: &str = "switching the output is not available on this Windows";

/// Makes the endpoint with this id the default for every role (console,
/// multimedia, communications).
pub(crate) fn set_default(endpoint_id: &str) -> Result<(), String> {
    let policy = create().ok_or(UNAVAILABLE)?;
    let wide = crate::win::wide(endpoint_id);
    for role in [eConsole, eMultimedia, eCommunications] {
        let hr = iface::set_default_endpoint(&policy, PCWSTR(wide.as_ptr()), role);
        if hr.is_err() {
            return Err(format!("switching the output failed (0x{:08X})", hr.0 as u32));
        }
    }
    Ok(())
}

/// Whether the interface can be created on this Windows (COM must be
/// initialised on the calling thread). Creating it changes nothing.
pub(crate) fn available() -> bool {
    create().is_some()
}

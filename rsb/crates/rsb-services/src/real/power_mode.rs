//! Changing the power mode (best power efficiency, balanced, best
//! performance). Windows has no documented call for it: this uses
//! `PowerSetActiveOverlayScheme` and `PowerGetEffectiveOverlayScheme`,
//! exported by powrprof.dll, loaded at run time. When either is missing or
//! fails, the power service reports `mode_available = false` instead of
//! failing.

use std::sync::OnceLock;

use rsb_core::services::PowerMode;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
use windows::core::{GUID, s, w};

/// Best power efficiency.
const EFFICIENCY: GUID = GUID::from_u128(0x961cc777_2547_4f9d_8174_7d86181b8a7a);
/// Balanced is the absence of an overlay: the nil GUID.
const BALANCED: GUID = GUID::from_u128(0);
/// Best performance.
const PERFORMANCE: GUID = GUID::from_u128(0xded574b5_45a0_4f42_8737_46345c09c238);

type SetOverlay = unsafe extern "system" fn(GUID) -> u32;
type GetOverlay = unsafe extern "system" fn(*mut GUID) -> u32;

/// The two exports, looked up once.
fn exports() -> Option<(SetOverlay, GetOverlay)> {
    static EXPORTS: OnceLock<Option<(SetOverlay, GetOverlay)>> = OnceLock::new();
    *EXPORTS.get_or_init(|| {
        // SAFETY: loads a system DLL by name from System32 only; the module stays loaded, so the pointers stay valid.
        unsafe {
            let module = LoadLibraryExW(w!("powrprof.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
            let set = GetProcAddress(module, s!("PowerSetActiveOverlayScheme"))?;
            let get = GetProcAddress(module, s!("PowerGetEffectiveOverlayScheme"))?;
            // SAFETY: both exports have these C signatures (DWORD f(GUID) and DWORD f(GUID *)).
            Some((
                std::mem::transmute::<unsafe extern "system" fn() -> isize, SetOverlay>(set),
                std::mem::transmute::<unsafe extern "system" fn() -> isize, GetOverlay>(get),
            ))
        }
    })
}

pub(crate) fn guid_of(mode: PowerMode) -> GUID {
    match mode {
        PowerMode::Efficiency => EFFICIENCY,
        PowerMode::Balanced => BALANCED,
        PowerMode::Performance => PERFORMANCE,
    }
}

/// The mode an overlay scheme stands for; None for one of the others
/// Windows has (it reads as "unknown").
pub(crate) fn mode_of(guid: &GUID) -> Option<PowerMode> {
    [PowerMode::Efficiency, PowerMode::Balanced, PowerMode::Performance].into_iter().find(|m| guid_of(*m) == *guid)
}

/// What a mode change is refused with when the export is missing (the fake says the same).
pub(crate) const UNAVAILABLE: &str = "changing the power mode is not available on this Windows";

/// Sets the overlay scheme for `mode`. Err when the export is missing or
/// the call fails.
pub(crate) fn set_mode(mode: PowerMode) -> Result<(), String> {
    let (set, _) = exports().ok_or(UNAVAILABLE)?;
    // SAFETY: the export takes the scheme GUID by value and has no other preconditions.
    let code = unsafe { set(guid_of(mode)) };
    if code == 0 { Ok(()) } else { Err(format!("changing the power mode failed (code {code})")) }
}

/// The overlay scheme in effect now: Some(mode), or None when it is not
/// one of the three or the call failed. Reads only.
pub(crate) fn current() -> Option<PowerMode> {
    let (_, get) = exports()?;
    let mut guid = GUID::zeroed();
    // SAFETY: the export writes one GUID to the pointer it is given.
    let code = unsafe { get(&mut guid) };
    if code == 0 { mode_of(&guid) } else { None }
}

/// Whether the exports exist on this Windows and the read works.
pub(crate) fn available() -> bool {
    let Some((_, get)) = exports() else { return false };
    let mut guid = GUID::zeroed();
    // SAFETY: the export writes one GUID to the pointer it is given.
    unsafe { get(&mut guid) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_modes_map_to_their_overlay_guids_and_back() {
        for m in [PowerMode::Efficiency, PowerMode::Balanced, PowerMode::Performance] {
            assert_eq!(mode_of(&guid_of(m)), Some(m));
        }
        assert_eq!(guid_of(PowerMode::Efficiency), GUID::from_u128(0x961cc777_2547_4f9d_8174_7d86181b8a7a));
        assert_eq!(guid_of(PowerMode::Balanced), GUID::zeroed());
        assert_eq!(guid_of(PowerMode::Performance), GUID::from_u128(0xded574b5_45a0_4f42_8737_46345c09c238));
        assert_eq!(
            mode_of(&GUID::from_u128(0x3af9b8d9_7c97_431d_ad78_34a8bfea439f)),
            None,
            "another overlay is unknown"
        );
    }
}

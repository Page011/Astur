//! The WLAN client API behind a small trait, so the code that decides what may be called (and the
//! per-network merge) is tested without an adapter. Nothing here logs names.

use std::ffi::c_void;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::NetworkManagement::WiFi::{
    L2_NOTIFICATION_DATA, WLAN_AVAILABLE_NETWORK_LIST, WLAN_CONNECTION_PARAMETERS, WLAN_INTERFACE_INFO_LIST,
    WLAN_NOTIFICATION_SOURCE_ACM, WlanCloseHandle, WlanConnect, WlanDisconnect, WlanEnumInterfaces, WlanFreeMemory,
    WlanGetAvailableNetworkList, WlanOpenHandle, WlanRegisterNotification, WlanScan, dot11_BSS_type_any,
    wlan_connection_mode_profile,
};
use windows::core::{GUID, PCWSTR};

use crate::win::wide;

/// Networks kept after the per-name merge.
pub(crate) const MAX_NETWORKS: usize = 64;

/// `WLAN_AVAILABLE_NETWORK_CONNECTED` and `WLAN_AVAILABLE_NETWORK_HAS_PROFILE`.
const FLAG_CONNECTED: u32 = 1;
const FLAG_HAS_PROFILE: u32 = 2;

/// One scan-list entry of one adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawNet {
    pub ssid: String,
    pub signal: u32,
    pub secure: bool,
    pub connected: bool,
    pub profile: Option<String>,
}

/// What the service may ask of the adapter. `list`, `scan`, `connect` and `disconnect` are the calls
/// that need location access; the callers check that first (and the tests count the calls).
pub(crate) trait WlanApi {
    fn interfaces(&mut self) -> Result<Vec<GUID>, String>;
    fn list(&mut self, interface: &GUID) -> Result<Vec<RawNet>, String>;
    fn scan(&mut self, interface: &GUID) -> Result<(), String>;
    fn connect(&mut self, interface: &GUID, profile: &str) -> Result<(), String>;
    fn disconnect(&mut self, interface: &GUID) -> Result<(), String>;
}

/// One name after the merge: the strongest signal over all adapters, connected or known on any,
/// and the adapter and saved profile `connect` should use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Merged {
    pub ssid: String,
    pub signal: u32,
    pub secure: bool,
    pub connected: bool,
    pub known: bool,
    pub interface: GUID,
    pub profile: Option<String>,
}

/// Merges per SSID, strongest first (ties by name), at most `MAX_NETWORKS`. Hidden networks (empty
/// name) are dropped.
pub(crate) fn merge(found: Vec<(GUID, RawNet)>) -> Vec<Merged> {
    let mut found: Vec<(GUID, RawNet)> = found.into_iter().filter(|(_, n)| !n.ssid.is_empty()).collect();
    found.sort_by(|a, b| b.1.signal.cmp(&a.1.signal).then_with(|| a.1.ssid.cmp(&b.1.ssid)));
    let mut out: Vec<Merged> = Vec::new();
    for (interface, net) in found {
        match out.iter_mut().find(|m| m.ssid == net.ssid) {
            Some(m) => {
                m.connected |= net.connected;
                m.secure |= net.secure;
                if m.profile.is_none() && net.profile.is_some() {
                    m.known = true;
                    m.profile = net.profile;
                    m.interface = interface;
                }
            }
            None => out.push(Merged {
                known: net.profile.is_some(),
                ssid: net.ssid,
                signal: net.signal.min(100),
                secure: net.secure,
                connected: net.connected,
                interface,
                profile: net.profile,
            }),
        }
    }
    out.truncate(MAX_NETWORKS);
    out
}

/// The WLAN client handle with its ACM notification registration.
pub(crate) struct SystemWlan {
    handle: HANDLE,
}

impl SystemWlan {
    /// Opens the client. `callback` gets ACM notifications only (the MSM source needs the Wi-Fi
    /// capability, which is why it is not asked for); `context` is passed back to it and must stay
    /// valid until this value is dropped. None = no WLAN service on this PC.
    pub(crate) fn open(
        callback: unsafe extern "system" fn(*mut L2_NOTIFICATION_DATA, *mut c_void),
        context: *const c_void,
    ) -> Option<SystemWlan> {
        let mut negotiated = 0u32;
        let mut handle = HANDLE::default();
        // SAFETY: live out pointers; version 2 is the Vista-and-later client.
        if unsafe { WlanOpenHandle(2, None, &mut negotiated, &mut handle) } != 0 {
            return None;
        }
        // SAFETY: `context` stays valid until `Drop` closes the handle; the callback only reads it.
        let r = unsafe {
            WlanRegisterNotification(
                handle,
                WLAN_NOTIFICATION_SOURCE_ACM,
                true,
                Some(callback),
                Some(context.cast_mut()),
                None,
                None,
            )
        };
        if r != 0 {
            log::debug!("wifi: no adapter notifications ({r}); the list is read on a timer");
        }
        Some(SystemWlan { handle })
    }
}

impl Drop for SystemWlan {
    fn drop(&mut self) {
        // SAFETY: the handle came from WlanOpenHandle and is closed once; this waits for callbacks in flight.
        let _ = unsafe { WlanCloseHandle(self.handle, None) };
    }
}

fn code(call: &str, r: u32) -> Result<(), String> {
    if r == 0 { Ok(()) } else { Err(format!("{call} failed (code {r})")) }
}

impl WlanApi for SystemWlan {
    fn interfaces(&mut self) -> Result<Vec<GUID>, String> {
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        // SAFETY: a live out pointer; the list is freed below.
        code("WlanEnumInterfaces", unsafe { WlanEnumInterfaces(self.handle, None, &mut list) })?;
        if list.is_null() {
            return Ok(Vec::new());
        }
        // SAFETY: the list holds `dwNumberOfItems` entries from `InterfaceInfo`.
        let guids = unsafe {
            std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), (*list).dwNumberOfItems as usize)
                .iter()
                .map(|i| i.InterfaceGuid)
                .collect()
        };
        // SAFETY: the list came from WlanEnumInterfaces and is freed once.
        unsafe { WlanFreeMemory(list.cast()) };
        Ok(guids)
    }

    fn list(&mut self, interface: &GUID) -> Result<Vec<RawNet>, String> {
        let mut list: *mut WLAN_AVAILABLE_NETWORK_LIST = std::ptr::null_mut();
        // SAFETY: live pointers; the list is freed below.
        code("WlanGetAvailableNetworkList", unsafe {
            WlanGetAvailableNetworkList(self.handle, interface, 0, None, &mut list)
        })?;
        if list.is_null() {
            return Ok(Vec::new());
        }
        // SAFETY: the list holds `dwNumberOfItems` entries from `Network`.
        let nets = unsafe { std::slice::from_raw_parts((*list).Network.as_ptr(), (*list).dwNumberOfItems as usize) }
            .iter()
            .map(|n| {
                let len = (n.dot11Ssid.uSSIDLength as usize).min(n.dot11Ssid.ucSSID.len());
                let profile_end = n.strProfileName.iter().position(|&c| c == 0).unwrap_or(n.strProfileName.len());
                let profile = String::from_utf16_lossy(&n.strProfileName[..profile_end]);
                RawNet {
                    ssid: String::from_utf8_lossy(&n.dot11Ssid.ucSSID[..len]).into_owned(),
                    signal: n.wlanSignalQuality,
                    secure: n.bSecurityEnabled.as_bool(),
                    connected: n.dwFlags & FLAG_CONNECTED != 0,
                    profile: (n.dwFlags & FLAG_HAS_PROFILE != 0 && !profile.is_empty()).then_some(profile),
                }
            })
            .collect();
        // SAFETY: the list came from WlanGetAvailableNetworkList and is freed once.
        unsafe { WlanFreeMemory(list.cast()) };
        Ok(nets)
    }

    fn scan(&mut self, interface: &GUID) -> Result<(), String> {
        // SAFETY: live handle and GUID; no SSID, no extra data.
        code("WlanScan", unsafe { WlanScan(self.handle, interface, None, None, None) })
    }

    fn connect(&mut self, interface: &GUID, profile: &str) -> Result<(), String> {
        let name = wide(profile);
        let params = WLAN_CONNECTION_PARAMETERS {
            wlanConnectionMode: wlan_connection_mode_profile,
            strProfile: PCWSTR(name.as_ptr()),
            pDot11Ssid: std::ptr::null_mut(),
            pDesiredBssidList: std::ptr::null_mut(),
            dot11BssType: dot11_BSS_type_any,
            dwFlags: 0,
        };
        // SAFETY: `params` and the profile name outlive the call, which copies what it needs.
        code("WlanConnect", unsafe { WlanConnect(self.handle, interface, &params, None) })
    }

    fn disconnect(&mut self, interface: &GUID) -> Result<(), String> {
        // SAFETY: live handle and GUID.
        code("WlanDisconnect", unsafe { WlanDisconnect(self.handle, interface, None) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(ssid: &str, signal: u32, connected: bool, profile: Option<&str>) -> RawNet {
        RawNet { ssid: ssid.into(), signal, secure: true, connected, profile: profile.map(str::to_string) }
    }

    #[test]
    fn a_name_seen_on_two_adapters_is_merged_with_the_strongest_signal() {
        let (a, b) = (GUID::from_u128(1), GUID::from_u128(2));
        let merged = merge(vec![
            (a, net("one", 40, false, None)),
            (b, net("one", 70, false, Some("saved"))),
            (a, net("two", 55, true, None)),
            (a, net("", 90, false, None)),
        ]);
        assert_eq!(merged.len(), 2, "the hidden network is dropped, the duplicate merged");
        assert_eq!(merged[0].ssid, "one");
        assert_eq!(merged[0].signal, 70);
        assert!(merged[0].known && merged[0].interface == b, "connect uses the adapter holding the profile");
        assert!(merged[1].connected && !merged[1].known);
    }

    #[test]
    fn the_list_is_capped_and_strongest_first() {
        let a = GUID::from_u128(1);
        let many = (0..200).map(|i| (a, net(&format!("n{i}"), i % 101, false, None))).collect();
        let merged = merge(many);
        assert_eq!(merged.len(), MAX_NETWORKS);
        assert!(merged.windows(2).all(|w| w[0].signal >= w[1].signal));
    }
}

//! A device's battery level from the property Settings shows,
//! `{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2` on the device's Bluetooth device node. The property
//! is not documented as an API: any failure reads as "no battery reported".

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_GETIDLIST_FILTER_ENUMERATOR, CM_GETIDLIST_FILTER_PRESENT, CM_Get_DevNode_PropertyW, CM_Get_Device_ID_List_SizeW,
    CM_Get_Device_ID_ListW, CM_LOCATE_DEVNODE_NORMAL, CM_Locate_DevNodeW, CR_SUCCESS,
};
use windows::Win32::Devices::Properties::{DEVPROP_TYPE_BYTE, DEVPROPTYPE};
use windows::Win32::Foundation::DEVPROPKEY;
use windows::core::{GUID, PCWSTR};

use crate::win::wide;

const BATTERY: DEVPROPKEY = DEVPROPKEY { fmtid: GUID::from_u128(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5), pid: 2 };

/// The device-node ids of one enumerator (`BTHENUM` for classic, `BTHLE` for LE) that are present.
fn present_ids(enumerator: &str) -> Vec<String> {
    let filter = wide(enumerator);
    let flags = CM_GETIDLIST_FILTER_ENUMERATOR | CM_GETIDLIST_FILTER_PRESENT;
    let mut len = 0u32;
    // SAFETY: a size query with a live out pointer and a NUL-terminated filter.
    if unsafe { CM_Get_Device_ID_List_SizeW(&mut len, PCWSTR(filter.as_ptr()), flags) } != CR_SUCCESS || len < 2 {
        return Vec::new();
    }
    let mut buffer = vec![0u16; len as usize];
    // SAFETY: the buffer holds the length the size query asked for.
    if unsafe { CM_Get_Device_ID_ListW(PCWSTR(filter.as_ptr()), &mut buffer, flags) } != CR_SUCCESS {
        return Vec::new();
    }
    // A multi-string: ids separated by NULs, ended by an empty one.
    buffer.split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
}

/// The battery percentage a device with this address reports, if any.
pub(crate) fn percent(address: u64) -> Option<f64> {
    let needle = format!("DEV_{address:012X}");
    for enumerator in ["BTHENUM", "BTHLE"] {
        for id in present_ids(enumerator).into_iter().filter(|id| id.to_ascii_uppercase().contains(&needle)) {
            let name = wide(&id);
            let mut node = 0u32;
            // SAFETY: a NUL-terminated id and a live out pointer.
            if unsafe { CM_Locate_DevNodeW(&mut node, PCWSTR(name.as_ptr()), CM_LOCATE_DEVNODE_NORMAL) } != CR_SUCCESS {
                continue;
            }
            let (mut kind, mut value, mut size) = (DEVPROPTYPE(0), [0u8; 4], 4u32);
            // SAFETY: the buffer is 4 bytes and its size is passed.
            let r =
                unsafe { CM_Get_DevNode_PropertyW(node, &BATTERY, &mut kind, Some(value.as_mut_ptr()), &mut size, 0) };
            if r == CR_SUCCESS && kind == DEVPROP_TYPE_BYTE && size >= 1 && value[0] <= 100 {
                return Some(f64::from(value[0]));
            }
        }
    }
    None
}
